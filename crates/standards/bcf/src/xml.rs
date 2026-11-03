//! The XML a `.bcfzip` needs: a small reader and a writer.
//!
//! [`parse`] reads well-formed XML 1.0 in UTF-8 into an [`Element`] tree. It handles the XML
//! declaration (either quote style), comments, processing instructions, a `DOCTYPE` (skipped),
//! CDATA, the five predefined entities and numeric character references, attributes in either
//! quote style, and namespace prefixes (dropped: BCF has no namespaces). It does not expand
//! custom entities (an unknown `&name;` is an error), so a hostile `DOCTYPE` can do nothing, and
//! it limits nesting to [`MAX_DEPTH`]. It never panics: all positions advance over whole characters
//! and every failure is a [`BcfError`] naming the line.
//!
//! [`Writer`] writes the files this crate produces: indented, double-quoted attributes, text
//! escaped by [`escape_text`] and [`escape_attr`] so that [`parse`] reads back exactly what was
//! written (carriage returns, tabs and newlines in attributes included).

use crate::BcfError;

/// Deepest element nesting [`parse`] accepts.
pub const MAX_DEPTH: usize = 256;

/// An XML element: its (local) name, attributes in document order, and content.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// The name without any namespace prefix.
    pub name: String,
    /// Attributes in document order, names without prefix; `xmlns` declarations are dropped.
    pub attrs: Vec<(String, String)>,
    /// Child elements and text, in document order.
    pub children: Vec<Node>,
}

/// A child of an [`Element`].
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A child element.
    Element(Element),
    /// Character data: references decoded, CDATA included, never empty.
    Text(String),
}

impl Element {
    /// The value of attribute `name`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    /// Every child element, in order.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            Node::Text(_) => None,
        })
    }

    /// The first child element called `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|element| element.name == name)
    }

    /// Every child element called `name`, in order.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.elements().filter(move |element| element.name == name)
    }

    /// The concatenated direct character data (for a leaf element, its text exactly).
    pub fn text(&self) -> String {
        self.children
            .iter()
            .filter_map(|node| match node {
                Node::Text(text) => Some(text.as_str()),
                Node::Element(_) => None,
            })
            .collect()
    }
}

/// Whether `c` may appear in an XML 1.0 document (the `Char` production).
pub fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// Whether `text` is empty or only whitespace: what the BCF schemas' `NonEmptyOrBlankString`
/// (whitespace collapsed, at least one character) rejects. Whitespace here is Unicode's, a superset
/// of XML's four characters, because validators such as Python's `xmlschema` collapse all of it.
pub fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// Parses `bytes` (UTF-8, optional byte order mark) as one XML document and returns its root.
pub fn parse(bytes: &[u8]) -> Result<Element, BcfError> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if bytes.starts_with(&[0xFE, 0xFF]) || bytes.starts_with(&[0xFF, 0xFE]) {
        return Err(BcfError::new("XML in UTF-16 is not supported"));
    }
    let text = std::str::from_utf8(bytes).map_err(|e| BcfError::new(format!("XML is not valid UTF-8 (byte {})", e.valid_up_to())))?;
    if let Some((line, c)) = text.split('\n').enumerate().find_map(|(n, l)| l.chars().find(|&c| !is_xml_char(c)).map(|c| (n + 1, c))) {
        return Err(BcfError::new(format!("line {line}: character U+{:04X} is not allowed in XML", c as u32)));
    }
    // XML 1.0 section 2.11: a line break is one \n whatever the file used.
    let normalized;
    let text = if text.contains('\r') {
        normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        text
    };
    let mut parser = Parser { s: text, i: 0 };
    parser.prolog()?;
    let root = parser.element(0)?;
    parser.misc()?;
    if parser.i < parser.s.len() {
        return parser.fail("content after the root element");
    }
    Ok(root)
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        self.s.get(self.i..).unwrap_or("")
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    fn line(&self) -> usize {
        1 + self.s.get(..self.i).unwrap_or("").bytes().filter(|&b| b == b'\n').count()
    }

    fn fail<T>(&self, what: &str) -> Result<T, BcfError> {
        Err(BcfError::new(format!("line {}: {what}", self.line())))
    }

    fn skip_space(&mut self) {
        let rest = self.rest();
        let n = rest.find(|c| !is_xml_space(c)).unwrap_or(rest.len());
        self.i += n;
    }

    /// Advances over `prefix`, which must be next.
    fn eat(&mut self, prefix: &str) -> bool {
        if self.starts_with(prefix) {
            self.i += prefix.len();
            true
        } else {
            false
        }
    }

    /// Advances past the next `end`, returning the text before it.
    fn until(&mut self, end: &str, what: &str) -> Result<&'a str, BcfError> {
        let rest = self.rest();
        match rest.find(end) {
            Some(n) => {
                self.i += n + end.len();
                Ok(rest.get(..n).unwrap_or(""))
            }
            None => self.fail(&format!("unterminated {what}")),
        }
    }

    /// The XML declaration, comments, processing instructions and a `DOCTYPE` before the root.
    fn prolog(&mut self) -> Result<(), BcfError> {
        if self.starts_with("<?xml") && self.rest().as_bytes().get(5).is_some_and(|&b| b == b'?' || is_xml_space(b as char)) {
            self.i += 2;
            let declaration = self.until("?>", "XML declaration")?;
            check_declaration(declaration).or_else(|what| self.fail(&what))?;
        }
        loop {
            self.skip_space();
            if self.eat("<?") {
                self.until("?>", "processing instruction")?;
            } else if self.eat("<!--") {
                self.until("-->", "comment")?;
            } else if self.eat("<!DOCTYPE") {
                self.doctype()?;
            } else {
                return Ok(());
            }
        }
    }

    /// After the root: only whitespace, comments and processing instructions.
    fn misc(&mut self) -> Result<(), BcfError> {
        loop {
            self.skip_space();
            if self.eat("<?") {
                self.until("?>", "processing instruction")?;
            } else if self.eat("<!--") {
                self.until("-->", "comment")?;
            } else {
                return Ok(());
            }
        }
    }

    /// Skips a `DOCTYPE` declaration (after `<!DOCTYPE`): up to the `>` outside quotes and brackets.
    fn doctype(&mut self) -> Result<(), BcfError> {
        let mut quote: Option<u8> = None;
        let mut depth = 0usize;
        while let Some(b) = self.peek() {
            self.i += 1;
            match (quote, b) {
                (Some(q), b) if b == q => quote = None,
                (Some(_), _) => {}
                (None, b'"' | b'\'') => quote = Some(b),
                (None, b'[') => depth += 1,
                (None, b']') => depth = depth.saturating_sub(1),
                (None, b'>') if depth == 0 => return Ok(()),
                (None, _) => {}
            }
        }
        self.fail("unterminated DOCTYPE")
    }

    /// An XML name up to whitespace or one of `/ > = < " ' & ? !`.
    fn name(&mut self) -> Result<&'a str, BcfError> {
        let rest = self.rest();
        let n = rest
            .find(|c: char| is_xml_space(c) || matches!(c, '/' | '>' | '=' | '<' | '"' | '\'' | '&' | '?' | '!'))
            .unwrap_or(rest.len());
        if n == 0 {
            return self.fail("expected a name");
        }
        self.i += n;
        Ok(rest.get(..n).unwrap_or(""))
    }

    fn element(&mut self, depth: usize) -> Result<Element, BcfError> {
        if depth >= MAX_DEPTH {
            return self.fail(&format!("elements nested deeper than {MAX_DEPTH}"));
        }
        if !self.eat("<") {
            return self.fail("expected an element");
        }
        let qualified = self.name()?;
        let name = local_name(qualified);
        if name.is_empty() {
            return self.fail("empty element name");
        }
        let mut attrs: Vec<(String, String)> = Vec::new();
        loop {
            self.skip_space();
            match self.peek() {
                Some(b'/') => {
                    self.i += 1;
                    if !self.eat(">") {
                        return self.fail("expected `>` after `/`");
                    }
                    return Ok(Element { name: name.to_string(), attrs, children: Vec::new() });
                }
                Some(b'>') => {
                    self.i += 1;
                    break;
                }
                Some(_) => {
                    let attr = self.name()?;
                    self.skip_space();
                    if !self.eat("=") {
                        return self.fail(&format!("attribute `{attr}` has no value"));
                    }
                    self.skip_space();
                    let quote = match self.peek() {
                        Some(q @ (b'"' | b'\'')) => q,
                        _ => return self.fail(&format!("attribute `{attr}` value is not quoted")),
                    };
                    self.i += 1;
                    let value = self.attr_value(quote)?;
                    if attr == "xmlns" || attr.starts_with("xmlns:") {
                        continue;
                    }
                    let key = local_name(attr);
                    if attrs.iter().any(|(k, _)| k == key) {
                        return self.fail(&format!("attribute `{key}` given twice"));
                    }
                    attrs.push((key.to_string(), value));
                }
                None => return self.fail("unexpected end of input in a start tag"),
            }
        }
        let children = self.content(qualified, depth)?;
        Ok(Element { name: name.to_string(), attrs, children })
    }

    /// Content up to and including the end tag of `qualified`.
    fn content(&mut self, qualified: &str, depth: usize) -> Result<Vec<Node>, BcfError> {
        let mut children = Vec::new();
        let mut text = String::new();
        fn flush(text: &mut String, children: &mut Vec<Node>) {
            if !text.is_empty() {
                children.push(Node::Text(std::mem::take(text)));
            }
        }
        loop {
            match self.peek() {
                None => return self.fail(&format!("unexpected end of input inside <{qualified}>")),
                Some(b'<') => {
                    if self.eat("</") {
                        let close = self.name()?;
                        self.skip_space();
                        if !self.eat(">") {
                            return self.fail("expected `>` in an end tag");
                        }
                        if close != qualified {
                            return self.fail(&format!("</{close}> closes <{qualified}>"));
                        }
                        flush(&mut text, &mut children);
                        return Ok(children);
                    } else if self.eat("<!--") {
                        self.until("-->", "comment")?;
                    } else if self.eat("<![CDATA[") {
                        text.push_str(self.until("]]>", "CDATA section")?);
                    } else if self.eat("<?") {
                        self.until("?>", "processing instruction")?;
                    } else if self.starts_with("<!") {
                        return self.fail("a declaration inside an element");
                    } else {
                        flush(&mut text, &mut children);
                        children.push(Node::Element(self.element(depth + 1)?));
                    }
                }
                Some(b'&') => self.reference(&mut text)?,
                Some(_) => {
                    let rest = self.rest();
                    let n = rest.find(['<', '&']).unwrap_or(rest.len());
                    text.push_str(rest.get(..n).unwrap_or(""));
                    self.i += n;
                }
            }
        }
    }

    /// An attribute value up to the closing `quote`, references decoded and tabs and line breaks
    /// turned into spaces (attribute-value normalization, XML 1.0 section 3.3.3).
    fn attr_value(&mut self, quote: u8) -> Result<String, BcfError> {
        let mut out = String::new();
        loop {
            let rest = self.rest();
            let stop = rest.find(|c: char| c == char::from(quote) || c == '<' || c == '&');
            let Some(n) = stop else {
                return self.fail("unterminated attribute value");
            };
            out.extend(rest.get(..n).unwrap_or("").chars().map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c }));
            self.i += n;
            match self.peek() {
                Some(b'&') => self.reference(&mut out)?,
                Some(b'<') => return self.fail("`<` in an attribute value"),
                _ => {
                    self.i += 1; // the closing quote
                    return Ok(out);
                }
            }
        }
    }

    /// A reference at `&`: a predefined entity or a character reference, appended to `out`.
    fn reference(&mut self, out: &mut String) -> Result<(), BcfError> {
        let rest = self.rest();
        let end = rest.as_bytes().iter().take(12).position(|&b| b == b';');
        let Some(end) = end else {
            return self.fail("`&` does not start a reference (write `&amp;`)");
        };
        let body = rest.get(1..end).unwrap_or("");
        let decoded = match body {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => body.strip_prefix('#').and_then(|digits| {
                let (digits, radix) = match digits.strip_prefix('x') {
                    Some(hex) => (hex, 16),
                    None => (digits, 10),
                };
                if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                    return None;
                }
                u32::from_str_radix(digits, radix).ok().and_then(char::from_u32).filter(|&c| is_xml_char(c))
            }),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                self.i += end + 1;
                Ok(())
            }
            None => self.fail(&format!("unknown or invalid reference `&{body};`")),
        }
    }
}

/// The part of a qualified name after its last `:`.
fn local_name(qualified: &str) -> &str {
    qualified.rsplit(':').next().unwrap_or(qualified)
}

/// Accepts a declaration (the text between `<?` and `?>`) that does not name another encoding.
fn check_declaration(declaration: &str) -> Result<(), String> {
    let Some(at) = declaration.find("encoding") else {
        return Ok(());
    };
    let after = declaration.get(at + "encoding".len()..).unwrap_or("").trim_start();
    let after = after.strip_prefix('=').unwrap_or(after).trim_start();
    let mut chars = after.chars();
    let value = match chars.next() {
        Some(q @ ('"' | '\'')) => chars.as_str().split(q).next().unwrap_or(""),
        _ => "",
    };
    if value.eq_ignore_ascii_case("utf-8") {
        Ok(())
    } else {
        Err(format!("unsupported encoding `{value}` (only UTF-8)"))
    }
}

/// `text` without the characters XML 1.0 cannot carry.
pub fn sanitize(text: &str) -> String {
    text.chars().filter(|&c| is_xml_char(c)).collect()
}

/// `text` as element content: `&`, `<`, `>` and carriage returns escaped, characters XML 1.0
/// cannot carry removed.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            c if is_xml_char(c) => out.push(c),
            _ => {}
        }
    }
    out
}

/// `text` as a double-quoted attribute value: [`escape_text`] plus `"`, tab and line feed.
pub fn escape_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            c => out.push_str(&escape_text(c.encode_utf8(&mut [0; 4]))),
        }
    }
    out
}

/// Writes an XML document: one element per line, two spaces per level.
#[derive(Debug)]
pub struct Writer {
    out: String,
    open: Vec<String>,
}

impl Default for Writer {
    fn default() -> Writer {
        Writer::new()
    }
}

impl Writer {
    /// A writer that has written the XML declaration.
    pub fn new() -> Writer {
        Writer { out: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n".to_string(), open: Vec::new() }
    }

    fn start(&mut self, name: &str, attrs: &[(&str, &str)]) {
        for _ in 0..self.open.len() {
            self.out.push_str("  ");
        }
        self.out.push('<');
        self.out.push_str(name);
        for (key, value) in attrs {
            self.out.push(' ');
            self.out.push_str(key);
            self.out.push_str("=\"");
            self.out.push_str(&escape_attr(value));
            self.out.push('"');
        }
    }

    /// `<name attrs>` and indents what follows.
    pub fn open(&mut self, name: &str, attrs: &[(&str, &str)]) {
        self.start(name, attrs);
        self.out.push_str(">\n");
        self.open.push(name.to_string());
    }

    /// Closes the innermost open element.
    pub fn close(&mut self) {
        if let Some(name) = self.open.pop() {
            for _ in 0..self.open.len() {
                self.out.push_str("  ");
            }
            self.out.push_str("</");
            self.out.push_str(&name);
            self.out.push_str(">\n");
        }
    }

    /// `<name attrs/>`.
    pub fn empty(&mut self, name: &str, attrs: &[(&str, &str)]) {
        self.start(name, attrs);
        self.out.push_str("/>\n");
    }

    /// `<name>text</name>` on one line, `text` escaped.
    pub fn leaf(&mut self, name: &str, text: &str) {
        self.start(name, &[]);
        self.out.push('>');
        self.out.push_str(&escape_text(text));
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push_str(">\n");
    }

    /// `<name>` with a number as its text: Rust's shortest round-trip form, a valid `xs:double`
    /// for any finite value (a non-finite one is written as 0).
    pub fn number(&mut self, name: &str, value: f64) {
        let text = if value.is_finite() { format!("{value:?}") } else { "0.0".to_string() };
        self.leaf(name, &text);
    }

    /// Closes every open element and returns the document.
    pub fn finish(mut self) -> String {
        while !self.open.is_empty() {
            self.close();
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Result<Element, BcfError> {
        parse(s.as_bytes())
    }

    #[test]
    fn reads_an_elementtree_style_document() {
        let doc = "<?xml version='1.0' encoding='UTF-8'?>\n<Markup><Topic Guid='a' TopicType=\"Issue\"><Title>Wall W1 &amp; column – prüfen</Title><Description>Säule/Wand &lt;&amp;&gt; overlap</Description></Topic></Markup>";
        let root = parse_str(doc).unwrap();
        assert_eq!(root.name, "Markup");
        let topic = root.child("Topic").unwrap();
        assert_eq!(topic.attr("Guid"), Some("a"));
        assert_eq!(topic.attr("TopicType"), Some("Issue"));
        assert_eq!(topic.attr("Missing"), None);
        assert_eq!(topic.child("Title").unwrap().text(), "Wall W1 & column – prüfen");
        assert_eq!(topic.child("Description").unwrap().text(), "Säule/Wand <&> overlap");
    }

    #[test]
    fn prolog_comments_pis_doctype_cdata_and_references() {
        let doc = "\u{feff}<?xml version=\"1.0\"?><!-- c --><?pi x?><!DOCTYPE a [<!ENTITY e \"x>y\">]><a b='1' c=\"it's\"><!-- inner --><?pi?>t<![CDATA[<raw & >]]>&#233;&#xE9;&#x1F600;<e/></a><!-- tail --> ";
        let root = parse_str(doc).unwrap();
        assert_eq!(root.attr("b"), Some("1"));
        assert_eq!(root.attr("c"), Some("it's"));
        assert_eq!(root.text(), "t<raw & >éé😀");
        assert!(root.child("e").unwrap().children.is_empty());
    }

    #[test]
    fn namespace_prefixes_and_declarations_are_dropped() {
        let doc = r#"<bcf:Markup xmlns:bcf="urn:x" xmlns="urn:y" xsi:schemaLocation="q"><bcf:Topic bcf:Guid="g"/></bcf:Markup>"#;
        let root = parse_str(doc).unwrap();
        assert_eq!(root.name, "Markup");
        assert_eq!(root.attrs, vec![("schemaLocation".to_string(), "q".to_string())]);
        assert_eq!(root.child("Topic").unwrap().attr("Guid"), Some("g"));
    }

    #[test]
    fn line_breaks_and_attribute_whitespace_are_normalized_as_xml_says() {
        let root = parse_str("<a v=\"x\ty\nz\">l1\r\nl2\rl3&#13;l4</a>").unwrap();
        assert_eq!(root.attr("v"), Some("x y z"));
        assert_eq!(root.text(), "l1\nl2\nl3\rl4");
    }

    #[test]
    fn writer_output_reads_back_exactly() {
        let nasty = "Säule/Wand <&> overlap – 'q' \"x\" ]]> \r\n\ttail  ";
        let mut w = Writer::new();
        w.open("Root", &[("v", nasty), ("empty", "")]);
        w.leaf("Text", nasty);
        w.leaf("Blank", "   ");
        w.empty("E", &[("k", "a\tb\nc\rd")]);
        w.number("N", 1e-7);
        w.open("Inner", &[]);
        let doc = w.finish();
        assert!(doc.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        let root = parse_str(&doc).unwrap();
        // Attribute values come back exactly: the writer used character references.
        assert_eq!(root.attr("v"), Some(nasty));
        assert_eq!(root.attr("empty"), Some(""));
        assert_eq!(root.child("Text").unwrap().text(), nasty);
        assert_eq!(root.child("Blank").unwrap().text(), "   ");
        assert_eq!(root.child("E").unwrap().attr("k"), Some("a\tb\nc\rd"));
        assert_eq!(root.child("N").unwrap().text(), "1e-7");
        assert!(root.child("Inner").is_some());
    }

    #[test]
    fn escaping_drops_what_xml_cannot_carry() {
        assert_eq!(escape_text("a\u{0}b\u{1}c\u{b}d\u{c}e\u{1f}f\u{fffe}g\u{ffff}h"), "abcdefgh");
        assert_eq!(escape_text("tab\tnl\n"), "tab\tnl\n");
        assert_eq!(escape_attr("\"<>&"), "&quot;&lt;&gt;&amp;");
    }

    #[test]
    fn malformed_documents_are_errors_never_panics() {
        let cases = [
            "",
            "   ",
            "<",
            "<a",
            "<a>",
            "<a></b>",
            "<a><b></a></b>",
            "<a b=1/>",
            "<a b='1/>",
            "<a b='x<y'/>",
            "<a b='1' b='2'/>",
            "<a>&nope;</a>",
            "<a>&amp</a>",
            "<a>&#0;</a>",
            "<a>&#1;</a>",
            "<a>&#xD800;</a>",
            "<a>&#x110000;</a>",
            "<a>&#+65;</a>",
            "<a>&#x;</a>",
            "<a/><b/>",
            "<a/>text",
            "text<a/>",
            "<a><!-- unterminated</a>",
            "<a><![CDATA[x</a>",
            "<a><?pi</a>",
            "<?xml version='1.0' encoding='ISO-8859-1'?><a/>",
            "<?xml version='1.0'><a/>",
            "<!DOCTYPE a [<a/>",
            "<a><!DOCTYPE x></a>",
            "<a>\u{1}</a>",
            "<:/>",
            "</a>",
            "<a b/>",
            "<a =\"1\"/>",
            "<a / >",
        ];
        for case in cases {
            assert!(parse_str(case).is_err(), "{case:?} should be rejected");
        }
        assert!(parse(&[0xFF, 0xFE, b'<', 0, b'a', 0]).is_err());
        assert!(parse(&[b'<', b'a', b'>', 0xFF, b'<', b'/', b'a', b'>']).is_err());
        assert!(parse(&[0xFE, 0xFF, 0, b'<']).is_err());
        // Every prefix of a valid document is either a document or an error, never a panic.
        let doc = "<?xml version='1.0'?><a x=\"1\"><b>t&amp;é<![CDATA[c]]></b><c/><!--x--></a>";
        for n in 0..=doc.len() {
            if doc.is_char_boundary(n) {
                let _ = parse_str(&doc[..n]);
            }
            let _ = parse(&doc.as_bytes()[..n]);
        }
    }

    #[test]
    fn nesting_is_limited() {
        let deep = |n: usize| format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
        assert!(parse_str(&deep(MAX_DEPTH)).is_ok());
        assert!(parse_str(&deep(MAX_DEPTH + 1)).is_err());
        assert!(parse_str(&"<a>".repeat(100_000)).is_err());
    }

    #[test]
    fn errors_name_the_line() {
        let error = parse_str("<a>\n<b>\n</a>").unwrap_err().to_string();
        assert!(error.contains("line 3"), "{error}");
    }
}
