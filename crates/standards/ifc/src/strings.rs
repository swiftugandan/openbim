//! Part 21 string literal encoding and decoding.

/// Decodes the raw content of a Part 21 string literal (the bytes between the delimiting
/// apostrophes, with doubled apostrophes still doubled) into a Rust string.
///
/// Handles `''`, `\\`, `\X2\hhhh…\X0\` (UTF-16, surrogate pairs combined), `\X4\hhhhhhhh…\X0\`
/// (UCS-4), `\X\hh` (an ISO 8859-1 code point), `\S\c` (the character `c` + 128 of the current
/// code page) and `\Px\` page switches. Code pages other than `A` (ISO 8859-1) are decoded as
/// ISO 8859-1. Bytes >= 0x80 written directly are read as UTF-8, or as ISO 8859-1 when they are
/// not valid UTF-8. A backslash that starts no known directive is kept literally.
pub fn decode_string(raw: &[u8]) -> Result<String, String> {
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        match b {
            b'\'' => {
                out.push('\'');
                i += if raw.get(i + 1) == Some(&b'\'') { 2 } else { 1 };
            }
            b'\\' => {
                let rest = &raw[i + 1..];
                if rest.first() == Some(&b'\\') {
                    out.push('\\');
                    i += 2;
                } else if rest.starts_with(b"X2\\") {
                    i += 4;
                    let (units, next) = hex_run(raw, i, 4)?;
                    i = next;
                    for c in char::decode_utf16(units.into_iter().map(|u| u as u16)) {
                        match c {
                            Ok(c) => out.push(c),
                            Err(e) => return Err(format!("unpaired UTF-16 surrogate {:04X} in \\X2\\", e.unpaired_surrogate())),
                        }
                    }
                } else if rest.starts_with(b"X4\\") {
                    i += 4;
                    let (units, next) = hex_run(raw, i, 8)?;
                    i = next;
                    for u in units {
                        match char::from_u32(u) {
                            Some(c) => out.push(c),
                            None => return Err(format!("invalid code point {u:08X} in \\X4\\")),
                        }
                    }
                } else if rest.starts_with(b"X\\") {
                    let h = raw.get(i + 3..i + 5).ok_or("truncated \\X\\ directive")?;
                    let v = hex_value(h).ok_or("invalid hex digits in \\X\\ directive")?;
                    out.push(char::from(v as u8));
                    i += 5;
                } else if rest.starts_with(b"S\\") {
                    let c = *raw.get(i + 3).ok_or("truncated \\S\\ directive")?;
                    if !(0x20..=0x7e).contains(&c) {
                        return Err("\\S\\ must be followed by a basic character".into());
                    }
                    out.push(char::from(c + 128));
                    i += 4;
                    // a doubled apostrophe after \S\ (some writers double it)
                    if c == b'\'' && raw.get(i) == Some(&b'\'') {
                        i += 1;
                    }
                } else if rest.len() >= 3 && rest[0] == b'P' && (b'A'..=b'I').contains(&rest[1]) && rest[2] == b'\\' {
                    i += 4; // code page switch; pages are decoded as ISO 8859-1
                } else {
                    out.push('\\');
                    i += 1;
                }
            }
            0x80..=0xff => {
                let start = i;
                while i < raw.len() && raw[i] >= 0x80 {
                    i += 1;
                }
                match std::str::from_utf8(&raw[start..i]) {
                    Ok(s) => out.push_str(s),
                    Err(_) => out.extend(raw[start..i].iter().map(|&c| char::from(c))),
                }
            }
            _ => {
                out.push(char::from(b));
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Reads groups of `width` hex digits starting at `i` up to the closing `\X0\`; returns the
/// values and the index after `\X0\`.
fn hex_run(raw: &[u8], mut i: usize, width: usize) -> Result<(Vec<u32>, usize), String> {
    let mut units = Vec::new();
    loop {
        match raw.get(i) {
            Some(b'\\') => break,
            Some(_) => {
                let group = raw.get(i..i + width).ok_or("truncated hex group in \\X2\\/\\X4\\ directive")?;
                units.push(hex_value(group).ok_or("invalid hex digits in \\X2\\/\\X4\\ directive")?);
                i += width;
            }
            None => return Err("\\X2\\/\\X4\\ directive not closed by \\X0\\".into()),
        }
    }
    if raw.get(i..i + 4) != Some(b"\\X0\\") {
        return Err("\\X2\\/\\X4\\ directive not closed by \\X0\\".into());
    }
    Ok((units, i + 4))
}

fn hex_value(digits: &[u8]) -> Option<u32> {
    let mut v: u32 = 0;
    for &d in digits {
        v = v.checked_mul(16)? + (d as char).to_digit(16)?;
    }
    Some(v)
}

/// Encodes a string as the content of a Part 21 string literal (without the delimiting
/// apostrophes): `'` → `''`, `\` → `\\`, printable ASCII as is, every other character in
/// `\X2\…\X0\` (basic multilingual plane) or `\X4\…\X0\` runs.
pub fn encode_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    encode_string_into(s, &mut out);
    out
}

pub(crate) fn encode_string_into(s: &str, out: &mut String) {
    #[derive(PartialEq)]
    enum Run {
        None,
        X2,
        X4,
    }
    let mut run = Run::None;
    for c in s.chars() {
        let want = match c {
            ' '..='~' => Run::None,
            c if (c as u32) <= 0xffff => Run::X2,
            _ => Run::X4,
        };
        if want != run {
            if run != Run::None {
                out.push_str("\\X0\\");
            }
            match want {
                Run::X2 => out.push_str("\\X2\\"),
                Run::X4 => out.push_str("\\X4\\"),
                Run::None => {}
            }
            run = want;
        }
        match run {
            Run::None => match c {
                '\'' => out.push_str("''"),
                '\\' => out.push_str("\\\\"),
                c => out.push(c),
            },
            Run::X2 => push_hex(out, c as u32, 4),
            Run::X4 => push_hex(out, c as u32, 8),
        }
    }
    if run != Run::None {
        out.push_str("\\X0\\");
    }
}

fn push_hex(out: &mut String, v: u32, width: usize) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for k in (0..width).rev() {
        out.push(HEX[((v >> (4 * k)) & 0xf) as usize] as char);
    }
}
