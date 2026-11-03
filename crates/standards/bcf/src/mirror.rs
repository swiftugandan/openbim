//! Topics in the document: every BCF topic is mirrored as an `Issue` entity (spec/schema.md), and an
//! `Issue` entity can be exported as a topic.
//!
//! **Identity.** The Issue's id is the topic Guid used as a seed (`EntityId::derive(guid)`, the
//! lowercase text), and its reserved `_name` is that Guid, so `bim show <dir> <guid>` finds it.
//! **Components** are entities: a viewpoint's `IfcGuid` is the 22-character form of the entity's
//! 128-bit id (`bim_ifc::export::entity_guid`, the GlobalId of the IFC4 export), and
//! [`resolve_component`] is its inverse.
//!
//! **Props of an Issue** (the document holds the whole topic, so a project that is copied, synced
//! or reopened in a new process exports the same BCF):
//!
//! | prop | value | meaning |
//! |---|---|---|
//! | `title` | Text | `Topic/Title` |
//! | `status` | Text | `Topic@TopicStatus` |
//! | `entities` | List of Ref | every viewpoint's components, deduplicated, in order of appearance |
//! | `bcf_type` | Text | `Topic@TopicType` |
//! | `bcf_date` | Text | `Topic/CreationDate`, lexical |
//! | `bcf_author` | Text | `Topic/CreationAuthor` |
//! | `bcf_description` | Text | `Topic/Description`; empty when there is none |
//! | `bcf_labels` | List of Text | `Topic/Labels/Label` |
//! | `bcf_comments` | List of `[guid, date, author, text, viewpoint guid]` (all Text; empty text or link = none) | `Topic/Comments` |
//! | `bcf_viewpoints` | List of `[guid, List of Ref, camera]` | the viewpoints: components and camera |
//!
//! A camera is the empty List (none) or `[Text "perspective" or "orthogonal", Num view point x y z,
//! Num direction x y z, Num up x y z, Num field of view (or view-to-world scale), Num aspect ratio]`;
//! every Num is finite. [`issue_props`] writes all of them every time (there is no way to unset a
//! prop), and [`topic_from_issue`] reads whatever an Issue has, so Issues made by `bim apply` with
//! only `title`, `status` and `entities` export too.

use std::collections::HashSet;

use bim_doc::{Entity, EntityId, Value};

use crate::guid::{guid_from_u128, is_guid};
use crate::model::{Camera, Comment, Projection, Topic, Viewpoint};
use crate::xml::{is_blank, sanitize};
use crate::{datetime::is_xs_datetime, BcfError};

/// The entity kind that mirrors a topic.
pub const ISSUE: &str = "Issue";
/// `Topic/Title`.
pub const TITLE: &str = "title";
/// `Topic@TopicStatus`.
pub const STATUS: &str = "status";
/// The components of all viewpoints, as Refs.
pub const ENTITIES: &str = "entities";
/// `Topic@TopicType`.
pub const BCF_TYPE: &str = "bcf_type";
/// `Topic/CreationDate`.
pub const BCF_DATE: &str = "bcf_date";
/// `Topic/CreationAuthor`.
pub const BCF_AUTHOR: &str = "bcf_author";
/// `Topic/Description`.
pub const BCF_DESCRIPTION: &str = "bcf_description";
/// `Topic/Labels`.
pub const BCF_LABELS: &str = "bcf_labels";
/// `Topic/Comments`.
pub const BCF_COMMENTS: &str = "bcf_comments";
/// The viewpoints.
pub const BCF_VIEWPOINTS: &str = "bcf_viewpoints";

/// Title of an Issue that has none, status, type and author of one that lacks them.
const DEFAULT_TITLE: &str = "Untitled issue";
const DEFAULT_STATUS: &str = "Open";
const DEFAULT_TYPE: &str = "Issue";
const DEFAULT_AUTHOR: &str = "bim";

/// The `IfcGuid` of the component that is entity `id`.
pub fn component_guid(id: EntityId) -> String {
    bim_ifc::export::entity_guid(id)
}

/// The entity id a component's `IfcGuid` stands for; `None` if it is not a well-formed IFC GlobalId.
/// (Whether that entity exists is for the caller to check.)
pub fn resolve_component(ifc_guid: &str) -> Option<EntityId> {
    bim_ifc::guid::expand(ifc_guid.trim()).map(EntityId)
}

/// For each viewpoint of `topic`, the entities its components stand for. A component that is not
/// a well-formed GlobalId, or whose entity `live` does not know, is an error naming it.
pub fn resolve_components(topic: &Topic, live: &dyn Fn(EntityId) -> bool) -> Result<Vec<Vec<EntityId>>, BcfError> {
    topic
        .viewpoints
        .iter()
        .map(|viewpoint| {
            viewpoint
                .components
                .iter()
                .map(|ifc_guid| {
                    resolve_component(ifc_guid).filter(|&id| live(id)).ok_or_else(|| {
                        BcfError::new(format!(
                            "topic {}: viewpoint {}: component {ifc_guid} is not an entity of this project",
                            topic.guid, viewpoint.guid
                        ))
                    })
                })
                .collect()
        })
        .collect()
}

fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

fn texts(items: &[String]) -> Value {
    Value::List(items.iter().map(|s| text(s)).collect())
}

fn camera_value(camera: Option<Camera>) -> Value {
    let Some(camera) = camera.filter(Camera::is_valid) else {
        return Value::List(Vec::new());
    };
    let (kind, parameter) = match camera.projection {
        Projection::Perspective { field_of_view } => ("perspective", field_of_view),
        Projection::Orthogonal { view_to_world_scale } => ("orthogonal", view_to_world_scale),
    };
    let mut items = vec![text(kind)];
    items.extend(camera.view_point.iter().chain(&camera.direction).chain(&camera.up).map(|&v| Value::Num(v)));
    items.push(Value::Num(parameter));
    items.push(Value::Num(camera.aspect_ratio));
    Value::List(items)
}

fn camera_from_values(items: &[Value]) -> Option<Camera> {
    let (Value::Text(kind), numbers) = items.split_first()? else {
        return None;
    };
    let numbers: Vec<f64> = numbers
        .iter()
        .map(|v| match v {
            Value::Num(x) => Some(*x),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let [vx, vy, vz, dx, dy, dz, ux, uy, uz, parameter, aspect_ratio] = <[f64; 11]>::try_from(numbers).ok()?;
    let projection = match kind.as_str() {
        "perspective" => Projection::Perspective { field_of_view: parameter },
        "orthogonal" => Projection::Orthogonal { view_to_world_scale: parameter },
        _ => return None,
    };
    let camera = Camera { projection, view_point: [vx, vy, vz], direction: [dx, dy, dz], up: [ux, uy, uz], aspect_ratio };
    camera.is_valid().then_some(camera)
}

/// The props of the Issue that mirrors `topic` (see the module docs), in a fixed order.
/// `components[i]` are the entities of `topic.viewpoints[i]`'s components (missing entries count
/// as none); `entities` is their union.
pub fn issue_props(topic: &Topic, components: &[Vec<EntityId>]) -> Vec<(String, Value)> {
    let mut seen = HashSet::new();
    let entities: Vec<Value> = components
        .iter()
        .flatten()
        .filter(|&&id| seen.insert(id))
        .map(|&id| Value::Ref(id))
        .collect();
    let comments = topic
        .comments
        .iter()
        .map(|c| Value::List(vec![text(&c.guid), text(&c.date), text(&c.author), text(&c.text), text(c.viewpoint.as_deref().unwrap_or(""))]))
        .collect();
    let viewpoints = topic
        .viewpoints
        .iter()
        .enumerate()
        .map(|(i, viewpoint)| {
            let refs = components.get(i).map(Vec::as_slice).unwrap_or_default();
            Value::List(vec![text(&viewpoint.guid), Value::List(refs.iter().map(|&id| Value::Ref(id)).collect()), camera_value(viewpoint.camera)])
        })
        .collect();
    vec![
        (TITLE.to_string(), text(&topic.title)),
        (STATUS.to_string(), text(&topic.status)),
        (ENTITIES.to_string(), Value::List(entities)),
        (BCF_TYPE.to_string(), text(&topic.topic_type)),
        (BCF_DATE.to_string(), text(&topic.creation_date)),
        (BCF_AUTHOR.to_string(), text(&topic.creation_author)),
        (BCF_DESCRIPTION.to_string(), text(topic.description.as_deref().unwrap_or(""))),
        (BCF_LABELS.to_string(), texts(&topic.labels)),
        (BCF_COMMENTS.to_string(), Value::List(comments)),
        (BCF_VIEWPOINTS.to_string(), Value::List(viewpoints)),
    ]
}

/// The Text prop `key` of `entity`, cleaned of characters XML cannot carry; `None` if it is not
/// Text or is blank.
fn present_text(entity: &Entity, key: &str) -> Option<String> {
    match entity.props.get(key) {
        Some(Value::Text(t)) => Some(sanitize(t)).filter(|t| !is_blank(t)),
        _ => None,
    }
}

/// The topic that exports `entity`, an `Issue` named `name` (its `_name`). Tolerant of whatever
/// the props hold, since `bim apply` and `bim sync` can write anything into an Issue: missing or
/// unusable values become defaults so that the topic always satisfies the schema.
///
/// * Guid: `name` if it is a lowercase UUID, else one derived from the entity id (so importing the
///   export creates a second Issue under that Guid).
/// * Title, status, type, author: the prop if it is non-blank Text, else `Untitled issue`, `Open`,
///   `Issue`, `bim`. Date: `bcf_date` if it is an `xs:dateTime`, else `now` (the caller's clock;
///   the Unix epoch if that is not a date either).
/// * Comments: the well-formed entries with a unique Guid, a valid date and a non-blank author; a
///   viewpoint link is kept only if the topic has that viewpoint.
/// * Viewpoints: the entries of `bcf_viewpoints` with their cameras; a Guid that is missing or
///   repeated is replaced by a derived one. Components are the Refs to entities `live` accepts.
///   Without a `bcf_viewpoints` List (an Issue not made by BCF), one viewpoint selects the live
///   Refs of `entities`, if there are any. (With one, `entities` is only the mirror of what the
///   viewpoints select: changing `entities` alone does not change the exported topic.)
pub fn topic_from_issue(entity: &Entity, name: &str, live: &dyn Fn(EntityId) -> bool, now: &str) -> Topic {
    let guid = if is_guid(name) { name.to_string() } else { guid_from_u128(entity.id.0) };
    let creation_date = match entity.props.get(BCF_DATE) {
        Some(Value::Text(date)) if is_xs_datetime(date.trim()) => date.trim().to_string(),
        _ if is_xs_datetime(now) => now.to_string(),
        _ => "1970-01-01T00:00:00Z".to_string(),
    };

    let mut viewpoints: Vec<Viewpoint> = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    let live_ifc_guids = |refs: &[Value]| -> Vec<String> {
        refs.iter()
            .filter_map(|v| match v {
                Value::Ref(id) if live(*id) => Some(component_guid(*id)),
                _ => None,
            })
            .collect()
    };
    match entity.props.get(BCF_VIEWPOINTS) {
        Some(Value::List(items)) => {
            for (i, item) in items.iter().enumerate() {
                let Value::List(fields) = item else { continue };
                let [Value::Text(vp_guid), Value::List(refs), Value::List(camera)] = fields.as_slice() else { continue };
                let mut vp_guid = vp_guid.clone();
                let mut attempt = 0u32;
                while !is_guid(&vp_guid) || used.contains(&vp_guid) {
                    let seed = if attempt == 0 { format!("{guid}/{i}") } else { format!("{guid}/{i}/{attempt}") };
                    vp_guid = guid_from_u128(EntityId::derive(&seed).0);
                    attempt += 1;
                }
                used.insert(vp_guid.clone());
                viewpoints.push(Viewpoint { guid: vp_guid, components: live_ifc_guids(refs), camera: camera_from_values(camera) });
            }
        }
        _ => {
            if let Some(Value::List(refs)) = entity.props.get(ENTITIES) {
                let components = live_ifc_guids(refs);
                if !components.is_empty() {
                    let vp_guid = guid_from_u128(EntityId::derive(&format!("{guid}/viewpoint")).0);
                    used.insert(vp_guid.clone());
                    viewpoints.push(Viewpoint { guid: vp_guid, components, camera: None });
                }
            }
        }
    }

    let mut comments: Vec<Comment> = Vec::new();
    let mut comment_guids: HashSet<&str> = HashSet::new();
    if let Some(Value::List(items)) = entity.props.get(BCF_COMMENTS) {
        for item in items {
            let Value::List(fields) = item else { continue };
            let [Value::Text(c_guid), Value::Text(date), Value::Text(author), Value::Text(comment_text), Value::Text(link)] = fields.as_slice() else { continue };
            let (date, author) = (date.trim(), sanitize(author));
            if !is_guid(c_guid) || comment_guids.contains(c_guid.as_str()) || !is_xs_datetime(date) || is_blank(&author) {
                continue;
            }
            comment_guids.insert(c_guid.as_str());
            comments.push(Comment {
                guid: c_guid.clone(),
                date: date.to_string(),
                author,
                text: Some(sanitize(comment_text)).filter(|t| !is_blank(t)).unwrap_or_default(),
                viewpoint: Some(link.as_str()).filter(|l| used.contains(*l)).map(str::to_string),
            });
        }
    }

    Topic {
        guid,
        title: present_text(entity, TITLE).unwrap_or_else(|| DEFAULT_TITLE.to_string()),
        topic_type: present_text(entity, BCF_TYPE).unwrap_or_else(|| DEFAULT_TYPE.to_string()),
        status: present_text(entity, STATUS).unwrap_or_else(|| DEFAULT_STATUS.to_string()),
        creation_date,
        creation_author: present_text(entity, BCF_AUTHOR).unwrap_or_else(|| DEFAULT_AUTHOR.to_string()),
        description: present_text(entity, BCF_DESCRIPTION),
        labels: match entity.props.get(BCF_LABELS) {
            Some(Value::List(items)) => items
                .iter()
                .filter_map(|v| match v {
                    Value::Text(label) => Some(sanitize(label)).filter(|l| !is_blank(l)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        },
        comments,
        viewpoints,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const T: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const V1: &str = "11111111-1111-4111-8111-111111111111";
    const V2: &str = "22222222-2222-4222-8222-222222222222";
    const C1: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

    fn id(seed: &str) -> EntityId {
        EntityId::derive(seed)
    }

    fn topic() -> Topic {
        Topic {
            guid: T.to_string(),
            title: "Crack above door".to_string(),
            topic_type: "Issue".to_string(),
            status: "Open".to_string(),
            creation_date: "2026-03-02T08:15:30+01:00".to_string(),
            creation_author: "alice".to_string(),
            description: Some("d".to_string()),
            labels: vec!["a".to_string(), "b".to_string()],
            comments: vec![Comment {
                guid: C1.to_string(),
                date: "2026-03-02T09:00:00Z".to_string(),
                author: "bob".to_string(),
                text: "see viewpoint".to_string(),
                viewpoint: Some(V2.to_string()),
            }],
            viewpoints: vec![
                Viewpoint { guid: V1.to_string(), components: vec![component_guid(id("w1")), component_guid(id("c1"))], camera: None },
                Viewpoint { guid: V2.to_string(), components: vec![component_guid(id("w1"))], camera: Some(Camera::default_view()) },
            ],
        }
    }

    fn entity(props: Vec<(&str, Value)>) -> Entity {
        Entity { id: id(T), kind: ISSUE.to_string(), props: props.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<BTreeMap<_, _>>() }
    }

    fn entity_of(topic: &Topic, components: &[Vec<EntityId>]) -> Entity {
        Entity { id: id(&topic.guid), kind: ISSUE.to_string(), props: issue_props(topic, components).into_iter().collect() }
    }

    fn all_live(_: EntityId) -> bool {
        true
    }

    #[test]
    fn component_guids_are_the_ifc_export_guids_and_invert() {
        for seed in ["w1", "01ARZ3NDEKTSV4RRFFQ69G5FAV", ""] {
            let e = EntityId::parse(seed);
            let g = component_guid(e);
            assert_eq!(g.len(), 22);
            assert_eq!(g, bim_ifc::export::entity_guid(e));
            assert_eq!(resolve_component(&g), Some(e));
            assert_eq!(resolve_component(&format!("  {g}\n")), Some(e), "surrounding whitespace is tolerated");
        }
        for bad in ["", "short", "4aaaaaaaaaaaaaaaaaaaaa", "0aaaaaaaaaaaaaaaaaaaa!", "0aaaaaaaaaaaaaaaaaaaaaa"] {
            assert_eq!(resolve_component(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn resolving_components_names_the_first_unknown_one() {
        let t = topic();
        let known: HashSet<EntityId> = [id("w1"), id("c1")].into_iter().collect();
        let resolved = resolve_components(&t, &|e| known.contains(&e)).unwrap();
        assert_eq!(resolved, vec![vec![id("w1"), id("c1")], vec![id("w1")]]);
        let only_w1: HashSet<EntityId> = [id("w1")].into_iter().collect();
        let message = resolve_components(&t, &|e| only_w1.contains(&e)).unwrap_err().to_string();
        assert!(message.contains(T) && message.contains(V1) && message.contains(&component_guid(id("c1"))), "{message}");
        let mut malformed = topic();
        malformed.viewpoints[0].components.push("nope".to_string());
        assert!(resolve_components(&malformed, &all_live).unwrap_err().to_string().contains("nope"));
    }

    #[test]
    fn the_issue_props_carry_the_whole_topic() {
        let t = topic();
        let props = issue_props(&t, &[vec![id("w1"), id("c1")], vec![id("w1")]]);
        let keys: Vec<&str> = props.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["title", "status", "entities", "bcf_type", "bcf_date", "bcf_author", "bcf_description", "bcf_labels", "bcf_comments", "bcf_viewpoints"]);
        let get = |key: &str| props.iter().find(|(k, _)| k == key).unwrap().1.clone();
        assert_eq!(get("title"), Value::Text("Crack above door".to_string()));
        assert_eq!(get("status"), Value::Text("Open".to_string()));
        assert_eq!(get("entities"), Value::List(vec![Value::Ref(id("w1")), Value::Ref(id("c1"))]), "the union, once each, in order");
        assert_eq!(get("bcf_date"), Value::Text("2026-03-02T08:15:30+01:00".to_string()));
        let Value::List(viewpoints) = get("bcf_viewpoints") else { panic!() };
        let Value::List(first) = &viewpoints[0] else { panic!() };
        assert_eq!(first[2], Value::List(Vec::new()), "no camera");
        let Value::List(second) = &viewpoints[1] else { panic!() };
        let Value::List(camera) = &second[2] else { panic!() };
        assert_eq!(camera.len(), 12);
        assert!(camera.iter().skip(1).all(|v| matches!(v, Value::Num(x) if x.is_finite())));
        let Value::List(comments) = get("bcf_comments") else { panic!() };
        assert_eq!(comments.len(), 1);
        // No topic without description puts a missing prop in the document: the empty text.
        let mut bare = topic();
        bare.description = None;
        assert_eq!(issue_props(&bare, &[]).iter().find(|(k, _)| k == "bcf_description").unwrap().1, Value::Text(String::new()));
    }

    #[test]
    fn a_non_finite_camera_never_reaches_the_document() {
        let mut t = topic();
        t.viewpoints[1].camera = Some(Camera { aspect_ratio: f64::NAN, ..Camera::default_view() });
        let props = issue_props(&t, &[vec![], vec![]]);
        let Value::List(viewpoints) = &props.iter().find(|(k, _)| k == "bcf_viewpoints").unwrap().1 else { panic!() };
        let Value::List(second) = &viewpoints[1] else { panic!() };
        assert_eq!(second[2], Value::List(Vec::new()));
    }

    #[test]
    fn issue_to_topic_round_trips_what_issue_props_wrote() {
        let t = topic();
        let issue = entity_of(&t, &[vec![id("w1"), id("c1")], vec![id("w1")]]);
        let back = topic_from_issue(&issue, T, &all_live, "2000-01-01T00:00:00Z");
        assert_eq!(back, t);
    }

    #[test]
    fn dead_components_are_left_out_of_the_export() {
        let t = topic();
        let issue = entity_of(&t, &[vec![id("w1"), id("c1")], vec![id("w1")]]);
        let back = topic_from_issue(&issue, T, &|e| e != id("c1"), "now");
        assert_eq!(back.viewpoints[0].components, vec![component_guid(id("w1"))]);
        assert_eq!(back.viewpoints[1].components, vec![component_guid(id("w1"))]);
    }

    #[test]
    fn an_issue_without_bcf_props_exports_with_defaults() {
        let issue = entity(vec![
            ("title", Value::Text("From apply".to_string())),
            ("status", Value::Text("Closed".to_string())),
            ("entities", Value::List(vec![Value::Ref(id("w1")), Value::Num(3.0), Value::Ref(id("gone"))])),
            ("priority", Value::Text("high".to_string())),
        ]);
        let t = topic_from_issue(&issue, "I1", &|e| e != id("gone"), "2026-05-05T05:05:05Z");
        assert_eq!(t.guid, guid_from_u128(id(T).0), "a name that is not a Guid gives a derived one");
        assert!(is_guid(&t.guid));
        assert_eq!((t.title.as_str(), t.status.as_str(), t.topic_type.as_str(), t.creation_author.as_str()), ("From apply", "Closed", "Issue", "bim"));
        assert_eq!(t.creation_date, "2026-05-05T05:05:05Z");
        assert_eq!((t.description.clone(), t.labels.clone(), t.comments.len()), (None, Vec::new(), 0));
        assert_eq!(t.viewpoints.len(), 1);
        assert_eq!(t.viewpoints[0].components, vec![component_guid(id("w1"))]);
        assert!(is_guid(&t.viewpoints[0].guid) && t.viewpoints[0].camera.is_none());
        // The same Issue exports to the same topic every time.
        assert_eq!(topic_from_issue(&issue, "I1", &|e| e != id("gone"), "2026-05-05T05:05:05Z"), t);
        // A `bcf_viewpoints` that is not a List is no viewpoints at all: the entities are used.
        let broken = entity(vec![("entities", Value::List(vec![Value::Ref(id("w1"))])), ("bcf_viewpoints", Value::Text("nope".to_string()))]);
        let t = topic_from_issue(&broken, "I3", &all_live, "now");
        assert_eq!(t.viewpoints.len(), 1);
        assert_eq!(t.viewpoints[0].components, vec![component_guid(id("w1"))]);
        // No entities, no viewpoint.
        let bare = entity(vec![]);
        let t = topic_from_issue(&bare, "I2", &all_live, "now");
        assert_eq!((t.title.as_str(), t.status.as_str()), ("Untitled issue", "Open"));
        assert_eq!(t.creation_date, "1970-01-01T00:00:00Z", "a clock that is no date falls back to the epoch");
        assert!(t.viewpoints.is_empty());
    }

    #[test]
    fn a_name_that_is_a_guid_is_the_topic_guid() {
        let issue = entity(vec![]);
        assert_eq!(topic_from_issue(&issue, T, &all_live, "now").guid, T);
        assert_ne!(topic_from_issue(&issue, &T.to_uppercase(), &all_live, "now").guid, T.to_uppercase(), "the schema wants lowercase");
    }

    #[test]
    fn hand_edited_props_still_give_a_schema_valid_topic() {
        let issue = entity(vec![
            ("title", Value::Text(" \n ".to_string())),
            ("status", Value::Num(1.0)),
            ("bcf_type", Value::Text("\u{1}".to_string())),
            ("bcf_date", Value::Text("tomorrow".to_string())),
            ("bcf_author", Value::Text(String::new())),
            ("bcf_description", Value::Text("  ".to_string())),
            ("bcf_labels", Value::List(vec![Value::Text(" x ".to_string()), Value::Text("  ".to_string()), Value::Num(1.0)])),
            (
                "bcf_comments",
                Value::List(vec![
                    Value::Text("not a list".to_string()),
                    Value::List(vec![Value::Text(C1.to_string())]),
                    // valid
                    Value::List(vec![Value::Text(C1.to_string()), Value::Text("2026-01-01T00:00:00Z".to_string()), Value::Text("a".to_string()), Value::Text("t".to_string()), Value::Text(V1.to_string())]),
                    // same Guid again, bad date, blank author, bad Guid
                    Value::List(vec![Value::Text(C1.to_string()), Value::Text("2026-01-02T00:00:00Z".to_string()), Value::Text("a".to_string()), Value::Text("t".to_string()), Value::Text(String::new())]),
                    Value::List(vec![Value::Text(V2.to_string()), Value::Text("soon".to_string()), Value::Text("a".to_string()), Value::Text("t".to_string()), Value::Text(String::new())]),
                    Value::List(vec![Value::Text(V2.to_string()), Value::Text("2026-01-02T00:00:00Z".to_string()), Value::Text(" ".to_string()), Value::Text("t".to_string()), Value::Text(String::new())]),
                    Value::List(vec![Value::Text("X".to_string()), Value::Text("2026-01-02T00:00:00Z".to_string()), Value::Text("a".to_string()), Value::Text("t".to_string()), Value::Text(String::new())]),
                ]),
            ),
            (
                "bcf_viewpoints",
                Value::List(vec![
                    Value::Num(1.0),
                    Value::List(vec![Value::Text(V1.to_string()), Value::List(vec![Value::Ref(id("w1")), Value::Text("junk".to_string())]), Value::List(vec![Value::Text("sideways".to_string())])]),
                    Value::List(vec![Value::Text(V1.to_string()), Value::List(vec![]), Value::List(vec![])]),
                    Value::List(vec![Value::Text("bad".to_string()), Value::List(vec![]), Value::List(vec![])]),
                ]),
            ),
        ]);
        let t = topic_from_issue(&issue, T, &all_live, "2026-02-02T02:02:02Z");
        assert_eq!((t.title.as_str(), t.status.as_str(), t.topic_type.as_str()), ("Untitled issue", "Open", "Issue"));
        assert_eq!((t.creation_date.as_str(), t.creation_author.as_str(), t.description.as_deref()), ("2026-02-02T02:02:02Z", "bim", None));
        assert_eq!(t.labels, vec![" x ".to_string()]);
        assert_eq!(t.comments.len(), 1);
        assert_eq!(t.comments[0].viewpoint.as_deref(), Some(V1));
        assert_eq!(t.viewpoints.len(), 3, "entries that are not [guid, refs, camera] are skipped; a repeated or bad Guid is replaced");
        assert_eq!(t.viewpoints[0].guid, V1);
        assert_eq!(t.viewpoints[0].components, vec![component_guid(id("w1"))]);
        assert_eq!(t.viewpoints[0].camera, None);
        let guids: HashSet<&str> = t.viewpoints.iter().map(|v| v.guid.as_str()).collect();
        assert_eq!(guids.len(), 3);
        assert!(guids.iter().all(|g| is_guid(g)));
    }

    #[test]
    fn camera_lists_round_trip_and_reject_malformed_ones() {
        let base = Camera::default_view();
        for camera in [base, Camera { projection: Projection::Orthogonal { view_to_world_scale: 3.5 }, ..base }] {
            let Value::List(items) = camera_value(Some(camera)) else { panic!() };
            assert_eq!(camera_from_values(&items), Some(camera));
        }
        let Value::List(good) = camera_value(Some(base)) else { panic!() };
        assert_eq!(camera_from_values(&[]), None);
        assert_eq!(camera_from_values(&good[..11]), None);
        let mut extra = good.clone();
        extra.push(Value::Num(1.0));
        assert_eq!(camera_from_values(&extra), None);
        let mut bad_kind = good.clone();
        bad_kind[0] = Value::Text("fisheye".to_string());
        assert_eq!(camera_from_values(&bad_kind), None);
        let mut not_a_number = good.clone();
        not_a_number[3] = Value::Text("1".to_string());
        assert_eq!(camera_from_values(&not_a_number), None);
        let mut nan = good.clone();
        nan[3] = Value::Num(f64::NAN);
        assert_eq!(camera_from_values(&nan), None);
        let mut kind_first = good;
        kind_first[0] = Value::Num(0.0);
        assert_eq!(camera_from_values(&kind_first), None);
    }

    #[test]
    fn xml_unsafe_characters_are_cleaned_from_exported_text() {
        let issue = entity(vec![("title", Value::Text("a\u{1}b\u{ffff}c".to_string())), ("bcf_description", Value::Text("\u{2}".to_string()))]);
        let t = topic_from_issue(&issue, T, &all_live, "now");
        assert_eq!(t.title, "abc");
        assert_eq!(t.description, None, "text that is only unusable characters is no text");
    }
}
