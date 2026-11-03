//! The XML files of a BCF 3.0 archive: how a [`Topic`] becomes `markup.bcf` and a [`Viewpoint`]
//! becomes a `.bcfv` file, and back.
//!
//! The writers emit elements in the order the buildingSMART XSDs demand (a `sequence` is ordered)
//! and omit every optional element that would be empty or blank, because the schemas' string type
//! (`NonEmptyOrBlankString`) rejects a blank value. Writers need these invariants of their input,
//! which [`crate::mirror::topic_from_issue`] and the readers guarantee: GUIDs match
//! [`guid::is_guid`], the title, type, status, author and every comment's author are not blank, and
//! dates are valid `xs:dateTime`s. Readers are strict about the same things and report a
//! [`BcfError`] naming the topic.

use std::collections::{BTreeSet, HashSet};

use crate::datetime::is_xs_datetime;
use crate::guid::normalize_guid;
use crate::model::{Camera, Comment, Projection, Topic, Viewpoint};
use crate::xml::{is_blank, Element, Writer};
use crate::BcfError;

/// The BCF version this crate reads and writes.
pub const VERSION: &str = "3.0";

/// The file name, inside the topic folder, that holds the viewpoint `guid`.
pub fn viewpoint_file(guid: &str) -> String {
    format!("{guid}.bcfv")
}

/// `bcf.version`.
pub fn write_version() -> String {
    let mut w = Writer::new();
    w.empty("Version", &[("VersionId", VERSION)]);
    w.finish()
}

/// `markup.bcf` of `topic`: its fields, comments and the `.bcfv` file name of each viewpoint (the
/// files themselves come from [`write_visinfo`]). A comment's viewpoint link is written only if it
/// names one of the topic's viewpoints.
pub fn write_markup(topic: &Topic) -> String {
    let mut w = Writer::new();
    w.open("Markup", &[]);
    w.open("Topic", &[("Guid", &topic.guid), ("TopicType", &topic.topic_type), ("TopicStatus", &topic.status)]);
    w.leaf("Title", &topic.title);
    if topic.labels.iter().any(|label| !is_blank(label)) {
        w.open("Labels", &[]);
        for label in topic.labels.iter().filter(|label| !is_blank(label)) {
            w.leaf("Label", label);
        }
        w.close();
    }
    w.leaf("CreationDate", &topic.creation_date);
    w.leaf("CreationAuthor", &topic.creation_author);
    if let Some(description) = topic.description.as_deref().filter(|d| !is_blank(d)) {
        w.leaf("Description", description);
    }
    if !topic.comments.is_empty() {
        let viewpoint_guids: HashSet<&str> = topic.viewpoints.iter().map(|v| v.guid.as_str()).collect();
        w.open("Comments", &[]);
        for comment in &topic.comments {
            w.open("Comment", &[("Guid", &comment.guid)]);
            w.leaf("Date", &comment.date);
            w.leaf("Author", &comment.author);
            if !is_blank(&comment.text) {
                w.leaf("Comment", &comment.text);
            }
            if let Some(link) = comment.viewpoint.as_deref().filter(|link| viewpoint_guids.contains(link)) {
                w.empty("Viewpoint", &[("Guid", link)]);
            }
            w.close();
        }
        w.close();
    }
    if !topic.viewpoints.is_empty() {
        w.open("Viewpoints", &[]);
        for viewpoint in &topic.viewpoints {
            w.open("ViewPoint", &[("Guid", &viewpoint.guid)]);
            w.leaf("Viewpoint", &viewpoint_file(&viewpoint.guid));
            w.close();
        }
        w.close();
    }
    w.finish()
}

fn write_vector(w: &mut Writer, name: &str, v: [f64; 3]) {
    w.open(name, &[]);
    w.number("X", v[0]);
    w.number("Y", v[1]);
    w.number("Z", v[2]);
    w.close();
}

/// A `.bcfv` file: the selected components and the camera. The schema requires a camera, so a
/// viewpoint without a valid one gets [`Camera::default_view`].
pub fn write_visinfo(viewpoint: &Viewpoint) -> String {
    let camera = viewpoint.camera.filter(Camera::is_valid).unwrap_or_else(Camera::default_view);
    let mut w = Writer::new();
    w.open("VisualizationInfo", &[("Guid", &viewpoint.guid)]);
    if !viewpoint.components.is_empty() {
        w.open("Components", &[]);
        w.open("Selection", &[]);
        for ifc_guid in &viewpoint.components {
            w.empty("Component", &[("IfcGuid", ifc_guid)]);
        }
        w.close();
        w.close();
    }
    let element = match camera.projection {
        Projection::Perspective { .. } => "PerspectiveCamera",
        Projection::Orthogonal { .. } => "OrthogonalCamera",
    };
    w.open(element, &[]);
    write_vector(&mut w, "CameraViewPoint", camera.view_point);
    write_vector(&mut w, "CameraDirection", camera.direction);
    write_vector(&mut w, "CameraUpVector", camera.up);
    match camera.projection {
        Projection::Perspective { field_of_view } => w.number("FieldOfView", field_of_view),
        Projection::Orthogonal { view_to_world_scale } => w.number("ViewToWorldScale", view_to_world_scale),
    }
    w.number("AspectRatio", camera.aspect_ratio);
    w.close();
    w.finish()
}

/// `extensions.xml`: the distinct topic types, statuses and labels in use (sorted), so a reader
/// that restricts values to the declared ones accepts every topic. `None` when there are none.
pub fn write_extensions(topics: &[Topic]) -> Option<String> {
    let mut types = BTreeSet::new();
    let mut statuses = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for topic in topics {
        types.insert(topic.topic_type.as_str());
        statuses.insert(topic.status.as_str());
        labels.extend(topic.labels.iter().map(String::as_str));
    }
    let mut w = Writer::new();
    w.open("Extensions", &[]);
    let mut any = false;
    for (container, item, values) in [("TopicTypes", "TopicType", types), ("TopicStatuses", "TopicStatus", statuses), ("TopicLabels", "TopicLabel", labels)] {
        let values: Vec<&str> = values.into_iter().filter(|v| !is_blank(v)).collect();
        if values.is_empty() {
            continue;
        }
        any = true;
        w.open(container, &[]);
        for value in values {
            w.leaf(item, value);
        }
        w.close();
    }
    any.then(|| w.finish())
}

/// A topic read from `markup.bcf`, before its viewpoint files are read.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedMarkup {
    /// The topic with `viewpoints` empty.
    pub topic: Topic,
    /// Each `ViewPoint`: its Guid and the file name its `Viewpoint` child gives, if it has one.
    pub viewpoint_files: Vec<(String, Option<String>)>,
}

fn fail<T>(guid: &str, what: impl std::fmt::Display) -> Result<T, BcfError> {
    Err(BcfError::new(format!("topic {guid}: {what}")))
}

/// A required child element's text: present and not blank, returned exactly as written.
fn required_text(parent: &Element, guid: &str, name: &str) -> Result<String, BcfError> {
    match parent.child(name).map(Element::text) {
        Some(text) if !is_blank(&text) => Ok(text),
        Some(_) => fail(guid, format!("<{name}> is blank")),
        None => fail(guid, format!("<{name}> is missing")),
    }
}

/// A required `xs:dateTime` child: its trimmed lexical form.
fn required_date(parent: &Element, guid: &str, name: &str) -> Result<String, BcfError> {
    let text = parent.child(name).map(Element::text);
    match text.as_deref().map(str::trim) {
        Some(date) if is_xs_datetime(date) => Ok(date.to_string()),
        Some(date) => fail(guid, format!("<{name}> `{date}` is not an xs:dateTime")),
        None => fail(guid, format!("<{name}> is missing")),
    }
}

fn required_attr(element: &Element, guid: &str, name: &str) -> Result<String, BcfError> {
    match element.attr(name) {
        Some(value) if !is_blank(value) => Ok(value.to_string()),
        Some(_) => fail(guid, format!("{name} is blank")),
        None => fail(guid, format!("{name} is missing")),
    }
}

fn guid_attr(element: &Element, what: &str, topic: &str) -> Result<String, BcfError> {
    element
        .attr("Guid")
        .and_then(normalize_guid)
        .map_or_else(|| fail(topic, format!("{what} has no valid Guid")), Ok)
}

/// Reads `markup.bcf` (its root element). Strict: see the module docs.
pub(crate) fn parse_markup(root: &Element) -> Result<ParsedMarkup, BcfError> {
    if root.name != "Markup" {
        return Err(BcfError::new(format!("markup.bcf: expected <Markup>, found <{}>", root.name)));
    }
    let element = root.child("Topic").ok_or_else(|| BcfError::new("markup.bcf: <Topic> is missing"))?;
    let guid = element
        .attr("Guid")
        .and_then(normalize_guid)
        .ok_or_else(|| BcfError::new("markup.bcf: Topic has no valid Guid"))?;
    let topic_type = required_attr(element, &guid, "TopicType")?;
    let status = required_attr(element, &guid, "TopicStatus")?;
    let title = required_text(element, &guid, "Title")?;
    let creation_date = required_date(element, &guid, "CreationDate")?;
    let creation_author = required_text(element, &guid, "CreationAuthor")?;
    let description = element.child("Description").map(Element::text).filter(|text| !is_blank(text));
    let labels = element
        .child("Labels")
        .map(|labels| labels.children_named("Label").map(Element::text).filter(|text| !is_blank(text)).collect())
        .unwrap_or_default();

    let mut viewpoint_files: Vec<(String, Option<String>)> = Vec::new();
    let mut viewpoint_guids: HashSet<String> = HashSet::new();
    for viewpoint in element.child("Viewpoints").into_iter().flat_map(|v| v.children_named("ViewPoint")) {
        let vp_guid = guid_attr(viewpoint, "a ViewPoint", &guid)?;
        if !viewpoint_guids.insert(vp_guid.clone()) {
            return fail(&guid, format!("viewpoint {vp_guid} is listed twice"));
        }
        let file = viewpoint.child("Viewpoint").map(|f| f.text().trim().to_string()).filter(|f| !f.is_empty());
        viewpoint_files.push((vp_guid, file));
    }

    let mut comments: Vec<Comment> = Vec::new();
    let mut comment_guids: HashSet<String> = HashSet::new();
    for comment in element.child("Comments").into_iter().flat_map(|c| c.children_named("Comment")) {
        let comment_guid = guid_attr(comment, "a Comment", &guid)?;
        if !comment_guids.insert(comment_guid.clone()) {
            return fail(&guid, format!("comment {comment_guid} is listed twice"));
        }
        let date = required_date(comment, &comment_guid, "Date")?;
        let author = required_text(comment, &comment_guid, "Author")?;
        let text = comment.child("Comment").map(Element::text).filter(|text| !is_blank(text)).unwrap_or_default();
        let viewpoint = match comment.child("Viewpoint") {
            Some(link) => {
                let link_guid = guid_attr(link, "a comment's Viewpoint", &guid)?;
                if !viewpoint_guids.contains(&link_guid) {
                    return fail(&guid, format!("comment {comment_guid} refers to viewpoint {link_guid}, which the topic does not have"));
                }
                Some(link_guid)
            }
            None => None,
        };
        comments.push(Comment { guid: comment_guid, date, author, text, viewpoint });
    }

    let topic = Topic {
        guid,
        title,
        topic_type,
        status,
        creation_date,
        creation_author,
        description,
        labels,
        comments,
        viewpoints: Vec::new(),
    };
    Ok(ParsedMarkup { topic, viewpoint_files })
}

fn number(element: &Element, name: &str) -> Option<f64> {
    element.child(name)?.text().trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn vector(element: &Element, name: &str) -> Option<[f64; 3]> {
    let v = element.child(name)?;
    Some([number(v, "X")?, number(v, "Y")?, number(v, "Z")?])
}

fn camera(element: &Element, perspective: bool) -> Option<Camera> {
    let projection = if perspective {
        Projection::Perspective { field_of_view: number(element, "FieldOfView")? }
    } else {
        Projection::Orthogonal { view_to_world_scale: number(element, "ViewToWorldScale")? }
    };
    let camera = Camera {
        projection,
        view_point: vector(element, "CameraViewPoint")?,
        direction: vector(element, "CameraDirection")?,
        up: vector(element, "CameraUpVector")?,
        aspect_ratio: number(element, "AspectRatio")?,
    };
    camera.is_valid().then_some(camera)
}

/// Reads a `.bcfv` file (its root element): the `IfcGuid` of every selected component (components
/// without one are skipped) and the first camera, if it is complete and valid.
pub(crate) fn parse_visinfo(root: &Element) -> Result<(Vec<String>, Option<Camera>), BcfError> {
    if root.name != "VisualizationInfo" {
        return Err(BcfError::new(format!("expected <VisualizationInfo>, found <{}>", root.name)));
    }
    let components = root
        .child("Components")
        .and_then(|components| components.child("Selection"))
        .map(|selection| {
            selection
                .children_named("Component")
                .filter_map(|component| component.attr("IfcGuid"))
                .map(|ifc_guid| ifc_guid.trim().to_string())
                .filter(|ifc_guid| !ifc_guid.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let camera = root
        .elements()
        .find(|e| e.name == "PerspectiveCamera" || e.name == "OrthogonalCamera")
        .and_then(|e| camera(e, e.name == "PerspectiveCamera"));
    Ok((components, camera))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse;

    const G1: &str = "11111111-1111-4111-8111-111111111111";
    const G2: &str = "22222222-2222-4222-8222-222222222222";
    const G3: &str = "33333333-3333-4333-8333-333333333333";
    const G4: &str = "44444444-4444-4444-8444-444444444444";

    fn topic() -> Topic {
        Topic {
            guid: G1.to_string(),
            title: "Wall W1 clashes with column – prüfen".to_string(),
            topic_type: "Clash".to_string(),
            status: "Open".to_string(),
            creation_date: "2026-03-01T09:30:00+00:00".to_string(),
            creation_author: "alice@example.com".to_string(),
            description: Some("Säule/Wand <&> overlap".to_string()),
            labels: vec!["Structural".to_string(), "Architecture".to_string()],
            comments: vec![
                Comment {
                    guid: G3.to_string(),
                    date: "2026-03-01T10:00:00+00:00".to_string(),
                    author: "bob@example.com".to_string(),
                    text: "Confirmed on site 'north' – 2 cm\r\n  \"ok\"".to_string(),
                    viewpoint: Some(G2.to_string()),
                },
                Comment {
                    guid: G4.to_string(),
                    date: "2026-03-02T08:15:30+01:00".to_string(),
                    author: "alice@example.com".to_string(),
                    text: String::new(),
                    viewpoint: None,
                },
            ],
            viewpoints: vec![Viewpoint { guid: G2.to_string(), components: vec!["0aaaaaaaaaaaaaaaaaaaaa".to_string()], camera: None }],
        }
    }

    #[test]
    fn markup_round_trips_through_the_reader() {
        let topic = topic();
        let xml = write_markup(&topic);
        let parsed = parse_markup(&parse(xml.as_bytes()).unwrap()).unwrap();
        assert_eq!(parsed.viewpoint_files, vec![(G2.to_string(), Some(format!("{G2}.bcfv")))]);
        let mut expected = topic;
        expected.viewpoints.clear();
        assert_eq!(parsed.topic, expected);
    }

    #[test]
    fn optional_elements_are_left_out_when_empty_or_blank() {
        let mut t = topic();
        t.description = Some(" \n\t".to_string());
        t.labels = vec![String::new(), "  ".to_string()];
        t.comments.clear();
        t.viewpoints.clear();
        let xml = write_markup(&t);
        for absent in ["Description", "Labels", "Comments", "Viewpoints"] {
            assert!(!xml.contains(&format!("<{absent}>")), "{absent} in {xml}");
        }
        let mut t = topic();
        t.comments[1].text = "  ".to_string();
        t.comments[0].viewpoint = Some(G4.to_string()); // not a viewpoint of this topic
        let xml = write_markup(&t);
        assert_eq!(xml.matches("<Comment>").count(), 1, "{xml}");
        assert!(!xml.contains("<Viewpoint Guid"), "{xml}");
    }

    #[test]
    fn element_order_follows_the_schema() {
        let xml = write_markup(&topic());
        let order = [
            "<Title>", "<Labels>", "<CreationDate>", "<CreationAuthor>", "<Description>", "<Comments>", "<Viewpoints>",
        ];
        let positions: Vec<usize> = order.iter().map(|tag| xml.find(tag).unwrap()).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
        let comment = xml.find("<Comment Guid").unwrap();
        let (date, author, text, link) =
            (xml[comment..].find("<Date>").unwrap(), xml[comment..].find("<Author>").unwrap(), xml[comment..].find("<Comment>").unwrap(), xml[comment..].find("<Viewpoint Guid").unwrap());
        assert!(date < author && author < text && text < link);
    }

    #[test]
    fn visinfo_round_trips_both_cameras() {
        let base = Camera::default_view();
        let cameras = [
            Some(Camera { view_point: [10.5, -4.25, 1.7], projection: Projection::Perspective { field_of_view: 45.5 }, ..base }),
            Some(Camera { projection: Projection::Orthogonal { view_to_world_scale: 12.5 }, aspect_ratio: 1.7777777777777777, ..base }),
            Some(Camera { view_point: [1e-7, -0.0, 1e21], ..base }),
        ];
        for camera in cameras {
            let viewpoint = Viewpoint { guid: G2.to_string(), components: vec!["0aaaaaaaaaaaaaaaaaaaaa".to_string(), "0aaaaaaaaaaaaaaaaaaaaa".to_string(), "3$_zZ9aaaaaaaaaaaaaaaa".to_string()], camera };
            let xml = write_visinfo(&viewpoint);
            let (components, read) = parse_visinfo(&parse(xml.as_bytes()).unwrap()).unwrap();
            assert_eq!(components, viewpoint.components);
            assert_eq!(read, camera);
        }
    }

    #[test]
    fn a_viewpoint_without_a_usable_camera_gets_the_default_one() {
        let bad = Camera { aspect_ratio: f64::NAN, ..Camera::default_view() };
        for camera in [None, Some(bad)] {
            let xml = write_visinfo(&Viewpoint { guid: G2.to_string(), components: Vec::new(), camera });
            assert!(!xml.contains("<Components>"), "{xml}");
            assert!(!xml.contains("NaN") && !xml.contains("inf"), "{xml}");
            let (components, read) = parse_visinfo(&parse(xml.as_bytes()).unwrap()).unwrap();
            assert!(components.is_empty());
            assert_eq!(read, Some(Camera::default_view()));
        }
    }

    #[test]
    fn unusable_cameras_in_files_are_dropped_not_fatal() {
        let doc = |camera: &str| format!("<VisualizationInfo Guid='{G2}'><Components><Selection><Component IfcGuid=' 0aaaaaaaaaaaaaaaaaaaaa '/><Component/><Component IfcGuid=''/></Selection></Components>{camera}</VisualizationInfo>");
        let point = |tag: &str, v: &str| format!("<{tag}><X>{v}</X><Y>0</Y><Z>1</Z></{tag}>");
        let camera = |extra: &str, fov: &str, aspect: &str, x: &str| {
            format!("<PerspectiveCamera>{}{}{}<FieldOfView>{fov}</FieldOfView><AspectRatio>{aspect}</AspectRatio>{extra}</PerspectiveCamera>", point("CameraViewPoint", x), point("CameraDirection", "0"), point("CameraUpVector", "0"))
        };
        let read = |xml: String| parse_visinfo(&parse(xml.as_bytes()).unwrap()).unwrap();
        let (components, good) = read(doc(&camera("", "60", "1.5", "2")));
        assert_eq!(components, vec!["0aaaaaaaaaaaaaaaaaaaaa".to_string()], "trimmed, and components without a guid skipped");
        assert!(good.is_some());
        for (fov, aspect, x) in [("0", "1", "0"), ("180", "1", "0"), ("60", "0", "0"), ("60", "-1", "0"), ("INF", "1", "0"), ("60", "NaN", "0"), ("60", "1", "inf"), ("60", "1", "abc"), ("", "1", "0")] {
            assert_eq!(read(doc(&camera("", fov, aspect, x))).1, None, "{fov} {aspect} {x}");
        }
        assert_eq!(read(doc("")).1, None, "no camera element");
        assert!(parse_visinfo(&parse(b"<Other/>").unwrap()).is_err());
    }

    #[test]
    fn extensions_list_the_values_in_use() {
        let mut other = topic();
        other.guid = G2.to_string();
        other.topic_type = "Issue".to_string();
        other.labels = vec!["Structural".to_string()];
        let xml = write_extensions(&[topic(), other]).unwrap();
        let root = parse(xml.as_bytes()).unwrap();
        let values = |container: &str| -> Vec<String> { root.child(container).unwrap().elements().map(Element::text).collect() };
        assert_eq!(values("TopicTypes"), vec!["Clash", "Issue"]);
        assert_eq!(values("TopicStatuses"), vec!["Open"]);
        assert_eq!(values("TopicLabels"), vec!["Architecture", "Structural"]);
        let mut bare = topic();
        bare.labels.clear();
        let xml = write_extensions(&[bare]).unwrap();
        assert!(!xml.contains("TopicLabels"));
        assert_eq!(write_extensions(&[]), None);
    }

    #[test]
    fn version_file() {
        let root = parse(write_version().as_bytes()).unwrap();
        assert_eq!((root.name.as_str(), root.attr("VersionId")), ("Version", Some("3.0")));
    }

    #[test]
    fn mutated_documents_never_panic() {
        let seeds = [
            write_markup(&topic()),
            write_visinfo(&topic().viewpoints[0]),
            write_version(),
            write_extensions(&[topic()]).unwrap(),
        ];
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let interesting = b"<>&;\"'/!?-[]= \n\r\t\x00\xff\xc3#x0123456789abcdef";
        let mut parsed = 0;
        for seed in &seeds {
            for _ in 0..3000 {
                let mut m = seed.as_bytes().to_vec();
                for _ in 0..=(next() % 4) {
                    if m.is_empty() {
                        break;
                    }
                    let at = (next() as usize) % m.len();
                    let byte = interesting[(next() as usize) % interesting.len()];
                    match next() % 4 {
                        0 => {
                            m.remove(at);
                        }
                        1 => m.insert(at, byte),
                        2 => m[at] = byte,
                        _ => m.truncate(at),
                    }
                }
                if let Ok(root) = parse(&m) {
                    parsed += 1;
                    let _ = parse_markup(&root);
                    let _ = parse_visinfo(&root);
                }
            }
        }
        assert!(parsed > 100, "the mutations should leave some documents well-formed ({parsed})");
    }

    fn markup(topic_children: &str, topic_attrs: &str) -> Result<ParsedMarkup, BcfError> {
        let xml = format!("<Markup><Topic {topic_attrs}>{topic_children}</Topic></Markup>");
        parse_markup(&parse(xml.as_bytes()).unwrap())
    }

    #[test]
    fn markup_reading_is_strict() {
        let attrs = format!("Guid='{G1}' TopicType='Issue' TopicStatus='Open'");
        let ok = "<Title>t</Title><CreationDate>2026-03-01T09:30:00Z</CreationDate><CreationAuthor>a</CreationAuthor>";
        assert!(markup(ok, &attrs).is_ok());
        let upper = format!("Guid='{}' TopicType='Issue' TopicStatus='Open'", G1.to_uppercase());
        assert_eq!(markup(ok, &upper).unwrap().topic.guid, G1, "guids are lowercased");
        let cases: Vec<(String, String)> = vec![
            (ok.to_string(), "TopicType='Issue' TopicStatus='Open'".to_string()),
            (ok.to_string(), "Guid='nope' TopicType='Issue' TopicStatus='Open'".to_string()),
            (ok.to_string(), format!("Guid='{G1}' TopicStatus='Open'")),
            (ok.to_string(), format!("Guid='{G1}' TopicType='  ' TopicStatus='Open'")),
            (ok.to_string(), format!("Guid='{G1}' TopicType='Issue'")),
            (ok.replace("<Title>t</Title>", ""), attrs.clone()),
            (ok.replace("<Title>t</Title>", "<Title> \n </Title>"), attrs.clone()),
            (ok.replace("<CreationDate>2026-03-01T09:30:00Z</CreationDate>", ""), attrs.clone()),
            (ok.replace("2026-03-01T09:30:00Z", "yesterday"), attrs.clone()),
            (ok.replace("2026-03-01T09:30:00Z", "2026-02-30T09:30:00Z"), attrs.clone()),
            (ok.replace("<CreationAuthor>a</CreationAuthor>", ""), attrs.clone()),
            (ok.replace("<CreationAuthor>a</CreationAuthor>", "<CreationAuthor/>"), attrs.clone()),
            (format!("{ok}<Comments><Comment Guid='{G3}'><Author>a</Author></Comment></Comments>"), attrs.clone()),
            (format!("{ok}<Comments><Comment Guid='{G3}'><Date>2026-03-01T09:30:00Z</Date></Comment></Comments>"), attrs.clone()),
            (format!("{ok}<Comments><Comment><Date>2026-03-01T09:30:00Z</Date><Author>a</Author></Comment></Comments>"), attrs.clone()),
            (format!("{ok}<Comments><Comment Guid='{G3}'><Date>2026-03-01T09:30:00Z</Date><Author>a</Author></Comment><Comment Guid='{G3}'><Date>2026-03-01T09:30:00Z</Date><Author>a</Author></Comment></Comments>"), attrs.clone()),
            (format!("{ok}<Comments><Comment Guid='{G3}'><Date>2026-03-01T09:30:00Z</Date><Author>a</Author><Viewpoint Guid='{G2}'/></Comment></Comments>"), attrs.clone()),
            (format!("{ok}<Comments><Comment Guid='{G3}'><Date>2026-03-01T09:30:00Z</Date><Author>a</Author><Viewpoint/></Comment></Comments>"), attrs.clone()),
            (format!("{ok}<Viewpoints><ViewPoint/></Viewpoints>"), attrs.clone()),
            (format!("{ok}<Viewpoints><ViewPoint Guid='{G2}'/><ViewPoint Guid='{G2}'/></Viewpoints>"), attrs.clone()),
        ];
        for (children, attrs) in cases {
            assert!(markup(&children, &attrs).is_err(), "{attrs} {children}");
        }
        assert!(parse_markup(&parse(b"<Other/>").unwrap()).is_err());
        assert!(parse_markup(&parse(b"<Markup/>").unwrap()).is_err());
        let message = markup(&ok.replace("<Title>t</Title>", ""), &attrs).unwrap_err().to_string();
        assert!(message.contains(G1) && message.contains("Title"), "{message}");
    }

    #[test]
    fn markup_reading_keeps_text_exactly_and_ignores_unknown_elements() {
        let attrs = format!("Guid='{G1}' TopicType=' Issue ' TopicStatus='Open'");
        let children = "<Priority>High</Priority><Title>  padded  </Title><Labels><Label> a </Label><Label>  </Label></Labels><CreationDate> 2026-03-01T09:30:00+01:00 </CreationDate><CreationAuthor>a</CreationAuthor><AssignedTo>x</AssignedTo><Description>  </Description><Comments><Comment Guid='33333333-3333-4333-8333-333333333333'><Date>2026-03-01T09:30:00Z</Date><Author>a</Author><Comment>  </Comment></Comment></Comments>";
        let topic = markup(children, &attrs).unwrap().topic;
        assert_eq!(topic.title, "  padded  ");
        assert_eq!(topic.topic_type, " Issue ");
        assert_eq!(topic.labels, vec![" a ".to_string()]);
        assert_eq!(topic.creation_date, "2026-03-01T09:30:00+01:00", "dates are trimmed but keep their offset");
        assert_eq!(topic.description, None);
        assert_eq!(topic.comments[0].text, "");
    }
}
