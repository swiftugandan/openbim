//! `bim bcf <dir> create|export|import`: BCF 3.0 issue tracking over the document (spec/cli.md).
//! The format is [`bim_bcf`]; this module is the command line around it.
//!
//! * `bim bcf <dir> create --title <t> --entity <id> [<id>…]` makes a topic about those entities
//!   (type `Issue`, status `Open`, created now by `$USER`, one viewpoint whose camera frames the
//!   entities that have geometry) and mirrors it as an `Issue` entity: the entity's id is the new
//!   topic Guid used as a seed, `status` the TopicStatus and `entities` Refs to the components
//!   (see [`bim_bcf::mirror`] for the other props). Prints the topic Guid, lowercase, as its only
//!   line of output.
//! * `bim bcf <dir> export <file.bcfzip>` writes every `Issue` entity of the project as a BCF 3.0
//!   archive. Component `IfcGuid`s are those of the IFC4 export. The document is not changed.
//! * `bim bcf <dir> import <file.bcfzip>` mirrors every topic of the archive the same way (a topic
//!   already in the project is updated to the archive's, so importing a file twice changes nothing
//!   the second time).
//!
//! Every command checks everything first and then applies its ops in one transaction
//! ([`Project::apply_ops`]). A wrong command line, a file that is not a BCF 3.0 archive, a title
//! or id that cannot be used, and a component that is not an entity of the project are exit 2
//! with nothing applied.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt::Display;
use std::fs;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use bim_bcf::xml::{is_blank, sanitize};
use bim_bcf::{datetime, guid, mirror, BcfError, Camera, Topic, Viewpoint};
use bim_doc::{Document, Entity, EntityId, Value};
use bim_model::Model;

use crate::geometry::write_atomically;
use crate::ops::{resolve_id, LineOp, LineValue};
use crate::project::Project;
use crate::CliError;

type Outcome = Result<u8, CliError>;

const FORMS: &str = "bim bcf <dir> create --title <t> --entity <id> [<id>…] | bim bcf <dir> export <file.bcfzip> | bim bcf <dir> import <file.bcfzip>";

fn usage(what: impl Display) -> CliError {
    CliError::Usage(format!("`{FORMS}`: {what}"))
}

/// The error for a file that cannot be read or written as BCF.
fn bcf_error(file: &Path, error: BcfError) -> CliError {
    CliError::Input(format!("{}: {error}", file.display()))
}

/// `bim bcf <dir> create|export|import …`.
pub(crate) fn bcf(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let [dir, subcommand, rest @ ..] = args else {
        return Err(usage("needs a directory and a subcommand"));
    };
    let dir = Path::new(dir);
    match subcommand.to_str() {
        Some("create") => create(dir, rest, out),
        Some("export") => export(dir, one_file(rest, "export")?),
        Some("import") => import(dir, one_file(rest, "import")?),
        _ => Err(usage(format!("unknown subcommand `{}` (expected create, export or import)", subcommand.to_string_lossy()))),
    }
}

fn one_file<'a>(rest: &'a [OsString], subcommand: &str) -> Result<&'a Path, CliError> {
    match rest {
        [file] => Ok(Path::new(file)),
        _ => Err(usage(format!("`{subcommand}` takes one file, got {} operand(s)", rest.len()))),
    }
}

/// The arguments of `create`: `--title <t>` and `--entity <id> [<id>…]` in either order, the
/// entity form repeatable; every id after `--entity` up to the next `--flag` belongs to it.
struct CreateArgs {
    title: String,
    entities: Vec<String>,
}

fn utf8(arg: &OsString, what: &str) -> Result<String, CliError> {
    arg.to_str().map(str::to_string).ok_or_else(|| CliError::Input(format!("{what} `{}` is not valid UTF-8", arg.to_string_lossy())))
}

fn parse_create(rest: &[OsString]) -> Result<CreateArgs, CliError> {
    let is_flag = |arg: &OsString| arg.to_str().is_some_and(|s| s.starts_with("--"));
    let (mut title, mut entities) = (None, Vec::new());
    let mut args = rest.iter().peekable();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--title") => {
                let value = args.next().ok_or_else(|| usage("`--title` needs a value"))?;
                if title.replace(value).is_some() {
                    return Err(usage("`--title` given twice"));
                }
            }
            Some("--entity") => {
                let before = entities.len();
                while let Some(id) = args.next_if(|next| !is_flag(next)) {
                    entities.push(id);
                }
                if entities.len() == before {
                    return Err(usage("`--entity` needs at least one id"));
                }
            }
            _ => return Err(usage(format!("unexpected argument `{}`", arg.to_string_lossy()))),
        }
    }
    let title = title.ok_or_else(|| usage("`--title` is required"))?;
    if entities.is_empty() {
        return Err(usage("`--entity` is required"));
    }
    Ok(CreateArgs {
        title: utf8(title, "title")?,
        entities: entities.into_iter().map(|id| utf8(id, "entity id")).collect::<Result<_, _>>()?,
    })
}

/// `bim bcf <dir> create --title <t> --entity <id> [<id>…]`.
fn create(dir: &Path, rest: &[OsString], out: &mut dyn Write) -> Outcome {
    let args = parse_create(rest)?;
    if is_blank(&args.title) {
        return Err(CliError::Input("the title must not be blank".to_string()));
    }
    if sanitize(&args.title) != args.title {
        return Err(CliError::Input("the title contains characters that XML cannot represent".to_string()));
    }
    let project = Project::open(dir)?;
    let mut ids: Vec<EntityId> = Vec::new();
    let mut seen_ids: HashSet<EntityId> = HashSet::new();
    for name in &args.entities {
        let id = resolve_id(name);
        if project.doc.get(id).is_none() {
            return Err(CliError::Input(format!("entity `{name}` not found")));
        }
        if seen_ids.insert(id) {
            ids.push(id);
        }
    }

    let topic_guid = (0..8)
        .map(|_| guid::new_guid())
        .find(|candidate| project.doc.get(EntityId::derive(candidate)).is_none())
        .ok_or_else(|| CliError::Internal("could not draw an unused topic Guid".to_string()))?;
    let topic = Topic {
        guid: topic_guid,
        title: args.title,
        topic_type: "Issue".to_string(),
        status: "Open".to_string(),
        creation_date: datetime::now_utc(),
        creation_author: author(),
        description: None,
        labels: Vec::new(),
        comments: Vec::new(),
        viewpoints: vec![Viewpoint {
            guid: guid::new_guid(),
            components: ids.iter().map(|&id| mirror::component_guid(id)).collect(),
            camera: Some(frame(&project, &ids)),
        }],
    };
    let props = mirror::issue_props(&topic, &[ids]);
    project.kinds.check_create(mirror::ISSUE, &[]).map_err(|e| CliError::Input(e.to_string()))?;
    let ops = issue_ops(&project, &topic.guid, props, None);
    let mut after = project.doc.clone();
    apply_all(&mut after, &ops)?;
    project.apply_ops(&ops, &after)?;
    writeln!(out, "{}", topic.guid).map_err(|e| CliError::Input(format!("cannot write to standard output: {e}")))?;
    Ok(0)
}

/// The topic author: the user's login name, else `bim`.
fn author() -> String {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|name| sanitize(name.trim()))
        .find(|name| !is_blank(name))
        .unwrap_or_else(|| "bim".to_string())
}

/// A camera that frames the geometry of `ids`, or the default view when none of them has any (or
/// the geometry cannot be compiled: a viewpoint is worth having even then).
fn frame(project: &Project, ids: &[EntityId]) -> Camera {
    let bounds = panic::catch_unwind(AssertUnwindSafe(|| {
        let model = Model::compile(&project.doc);
        let mut bounds: Option<([f64; 3], [f64; 3])> = None;
        for &id in ids {
            let Some((lo, hi)) = model.element(id).and_then(|element| element.mesh.bounds()) else { continue };
            let (lo, hi) = ([lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z]);
            bounds = Some(match bounds {
                None => (lo, hi),
                Some((min, max)) => (
                    [min[0].min(lo[0]), min[1].min(lo[1]), min[2].min(lo[2])],
                    [max[0].max(hi[0]), max[1].max(hi[1]), max[2].max(hi[2])],
                ),
            });
        }
        bounds
    }));
    match bounds {
        Ok(Some((min, max))) => Camera::fit(min, max),
        _ => Camera::default_view(),
    }
}

/// `bim bcf <dir> export <file>`.
fn export(dir: &Path, file: &Path) -> Outcome {
    let project = Project::open(dir)?;
    let now = datetime::now_utc();
    let live = |id: EntityId| project.doc.get(id).is_some();
    let mut seen: HashSet<String> = HashSet::new();
    let mut topics: Vec<Topic> = Vec::new();
    for entity in project.doc.entities().filter(|entity| entity.kind == mirror::ISSUE) {
        let topic = mirror::topic_from_issue(entity, &project.name(entity.id), &live, &now);
        if seen.insert(topic.guid.clone()) {
            topics.push(topic);
        }
    }
    let bytes = bim_bcf::write_bcfzip(&topics).map_err(|e| bcf_error(file, e))?;
    write_atomically(file, &bytes)?;
    Ok(0)
}

/// `bim bcf <dir> import <file>`.
fn import(dir: &Path, file: &Path) -> Outcome {
    let project = Project::open(dir)?;
    let bytes = fs::read(file).map_err(|e| CliError::Input(format!("{}: {e}", file.display())))?;
    let topics = bim_bcf::read_bcfzip(&bytes).map_err(|e| bcf_error(file, e))?;
    let live = |id: EntityId| project.doc.get(id).is_some();
    let mut ops: Vec<LineOp> = Vec::new();
    let mut after = project.doc.clone();
    for topic in &topics {
        let components = mirror::resolve_components(topic, &live).map_err(|e| bcf_error(file, e))?;
        let existing = match after.get(resolve_id(&topic.guid)) {
            Some(entity) if entity.kind == mirror::ISSUE => Some(entity.clone()),
            Some(entity) => {
                return Err(CliError::Input(format!(
                    "{}: topic {} cannot be imported: that name is already a {}, not an Issue",
                    file.display(),
                    topic.guid,
                    entity.kind
                )))
            }
            None => None,
        };
        if existing.is_none() {
            project.kinds.check_create(mirror::ISSUE, &[]).map_err(|e| CliError::Input(e.to_string()))?;
        }
        let topic_ops = issue_ops(&project, &topic.guid, mirror::issue_props(topic, &components), existing.as_ref());
        apply_all(&mut after, &topic_ops)?;
        ops.extend(topic_ops);
    }
    project.apply_ops(&ops, &after)?;
    Ok(0)
}

/// The ops that make the Issue named `seed` hold `props`: a create (when it does not exist yet)
/// and one set per prop whose value differs from the entity's current one.
fn issue_ops(project: &Project, seed: &str, props: Vec<(String, Value)>, existing: Option<&Entity>) -> Vec<LineOp> {
    let mut ops = Vec::new();
    if existing.is_none() {
        ops.push(LineOp::Create { seed: seed.to_string(), kind: mirror::ISSUE.to_string() });
    }
    for (key, value) in props {
        if existing.and_then(|entity| entity.props.get(&key)) == Some(&value) {
            continue;
        }
        ops.push(LineOp::Set { seed: seed.to_string(), key, value: line_value(project, &value) });
    }
    ops
}

/// The op form of a document value; a Ref names its target as the project knows it.
fn line_value(project: &Project, value: &Value) -> LineValue {
    match value {
        Value::Num(x) => LineValue::Num(*x),
        Value::Text(s) => LineValue::Text(s.clone()),
        Value::Bool(b) => LineValue::Bool(*b),
        Value::Ref(id) => LineValue::Ref(ref_name(project, *id)),
        Value::List(items) => LineValue::List(items.iter().map(|item| line_value(project, item)).collect()),
    }
}

/// The name a Ref to `id` is written with in the journal: the entity's own name, unless that does
/// not stand for `id` (then its ULID form, which always does).
fn ref_name(project: &Project, id: EntityId) -> String {
    let name = project.name(id);
    if resolve_id(&name) == id {
        name
    } else {
        id.to_ulid_string()
    }
}

/// Applies `ops` to `doc` (a scratch copy), or gives the input error of the first op the document
/// refuses.
fn apply_all(doc: &mut Document, ops: &[LineOp]) -> Result<(), CliError> {
    for op in ops {
        op.apply_to(doc).map_err(|e| CliError::Input(op.describe_error(&e)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn run(args: &[&str]) -> (u8, String, String) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = crate::run(&args, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    const MODEL: &str = r#"{"schema":1,"ops":[
      {"op":"create","id":"L0","kind":"Level","props":{"elevation":0}},
      {"op":"create","id":"WT","kind":"WallType","props":{"layer_thickness":[0.2],"layer_material":["Concrete"]}},
      {"op":"create","id":"W1","kind":"Wall","props":{"level":{"ref":"L0"},"type":{"ref":"WT"},"x1":0,"y1":0,"x2":5,"y2":0,"height":3}},
      {"op":"create","id":"W2","kind":"Wall","props":{"level":{"ref":"L0"},"type":{"ref":"WT"},"x1":5,"y1":0,"x2":5,"y2":4,"height":3}},
      {"op":"create","id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","kind":"Level","props":{"elevation":3}}
    ]}"#;

    /// A project directory `name` holding `MODEL`, and the directory's journal path.
    fn project(tmp: &TempDir, name: &str) -> String {
        let dir = tmp.path().join(name).to_str().unwrap().to_string();
        assert_eq!(run(&["init", &dir]).0, 0);
        let ops = tmp.path().join(format!("{name}.json"));
        std::fs::write(&ops, MODEL).unwrap();
        assert_eq!(run(&["apply", &dir, ops.to_str().unwrap()]).0, 0);
        dir
    }

    fn journal(dir: &str) -> Vec<u8> {
        std::fs::read(Path::new(dir).join("doc.oplog")).unwrap()
    }

    fn create_topic(dir: &str, title: &str, entities: &[&str]) -> String {
        let mut args = vec!["bcf", dir, "create", "--title", title, "--entity"];
        args.extend(entities);
        let (code, out, err) = run(&args);
        assert_eq!(code, 0, "{err}");
        assert_eq!(out.lines().count(), 1, "{out:?}");
        let guid = out.trim_end().to_string();
        assert!(guid::is_guid(&guid), "{guid}");
        guid
    }

    fn show(dir: &str, guid: &str) -> String {
        let (code, out, err) = run(&["show", dir, guid, "--json"]);
        assert_eq!(code, 0, "{err}");
        out
    }

    #[test]
    fn create_mirrors_a_topic_as_an_issue() {
        let tmp = TempDir::new();
        let dir = project(&tmp, "p");
        let g1 = create_topic(&dir, "Crack above door", &["W1"]);
        let g2 = create_topic(&dir, "Wall \"W2\" <&> – prüfen", &["W2", "01ARZ3NDEKTSV4RRFFQ69G5FAV", "W2"]);
        assert_ne!(g1, g2);
        let first = show(&dir, &g1);
        assert!(first.starts_with(&format!("{{\"id\":\"{g1}\",\"kind\":\"Issue\",\"props\":{{")), "{first}");
        assert!(first.contains("\"title\":\"Crack above door\"") && first.contains("\"status\":\"Open\""), "{first}");
        assert!(first.contains("\"entities\":[{\"ref\":\"W1\"}]"), "{first}");
        let second = show(&dir, &g2);
        assert!(second.contains("\"entities\":[{\"ref\":\"W2\"},{\"ref\":\"01ARZ3NDEKTSV4RRFFQ69G5FAV\"}]"), "duplicates collapse: {second}");
        assert!(second.contains("\"title\":\"Wall \\\"W2\\\" <&> – prüfen\""), "{second}");
        // The journal replays: a fresh process (every command here) sees the Issue and no dangling ref.
        assert_eq!(run(&["validate", &dir]).0, 0);
    }

    #[test]
    fn export_then_import_into_another_project_keeps_the_issues() {
        let tmp = TempDir::new();
        let (a, b) = (project(&tmp, "a"), project(&tmp, "b"));
        create_topic(&a, "Crack above door", &["W1"]);
        create_topic(&a, "Two", &["W1", "W2"]);
        let file = tmp.path().join("a.bcfzip");
        let file = file.to_str().unwrap();
        let exported_from = journal(&a);
        let (code, out, err) = run(&["bcf", &a, "export", file]);
        assert_eq!((code, out.as_str()), (0, ""), "{err}");
        assert_eq!(journal(&a), exported_from, "export does not change the project");
        let before = journal(&b);
        assert_eq!(run(&["bcf", &b, "import", file]).0, 0);
        assert_ne!(journal(&b), before);
        // Same document in the Issues' props: the hashes of the two projects agree.
        let (code, out, _) = run(&["diff", &a, &b]);
        assert_eq!((code, out.as_str()), (0, "same\n"));
        // Importing the same file again applies nothing.
        let journal_after_first = journal(&b);
        assert_eq!(run(&["bcf", &b, "import", file]).0, 0);
        assert_eq!(journal(&b), journal_after_first);
        // And exporting again gives the same archive.
        let again = tmp.path().join("b.bcfzip");
        assert_eq!(run(&["bcf", &b, "export", again.to_str().unwrap()]).0, 0);
        assert_eq!(std::fs::read(file).unwrap(), std::fs::read(&again).unwrap());
    }

    #[test]
    fn import_updates_a_topic_that_is_already_there() {
        let tmp = TempDir::new();
        let (a, b) = (project(&tmp, "a"), project(&tmp, "b"));
        let g = create_topic(&a, "Original", &["W1"]);
        let file = tmp.path().join("a.bcfzip");
        let file = file.to_str().unwrap();
        assert_eq!(run(&["bcf", &a, "export", file]).0, 0);
        assert_eq!(run(&["bcf", &b, "import", file]).0, 0);
        // Someone edits the title in b and closes the topic; re-importing a's version restores it.
        let edit = tmp.path().join("edit.ops");
        std::fs::write(&edit, format!("set {g} status text Closed\nset {g} bcf_description text late\n")).unwrap();
        assert_eq!(run(&["apply", &b, edit.to_str().unwrap()]).0, 0);
        assert!(show(&b, &g).contains("\"status\":\"Closed\""));
        assert_eq!(run(&["bcf", &b, "import", file]).0, 0);
        let restored = show(&b, &g);
        assert!(restored.contains("\"status\":\"Open\"") && restored.contains("\"bcf_description\":\"\""), "{restored}");
        assert_eq!(run(&["diff", &a, &b]).1, "same\n");
    }

    #[test]
    fn an_issue_made_by_apply_exports_as_a_topic() {
        let tmp = TempDir::new();
        let dir = project(&tmp, "p");
        let ops = tmp.path().join("issue.json");
        std::fs::write(&ops, r#"{"schema":1,"ops":[{"op":"create","id":"I1","kind":"Issue","props":{"title":"From apply","status":"Closed","entities":[{"ref":"W1"}],"priority":"high"}}]}"#).unwrap();
        assert_eq!(run(&["apply", &dir, ops.to_str().unwrap()]).0, 0);
        let file = tmp.path().join("i.bcfzip");
        assert_eq!(run(&["bcf", &dir, "export", file.to_str().unwrap()]).0, 0);
        let topics = bim_bcf::read_bcfzip(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(topics.len(), 1);
        assert_eq!((topics[0].title.as_str(), topics[0].status.as_str()), ("From apply", "Closed"));
        assert_eq!(topics[0].viewpoints.len(), 1);
        assert_eq!(topics[0].viewpoints[0].components, vec![mirror::component_guid(EntityId::derive("W1"))]);
    }

    #[test]
    fn rejected_commands_exit_2_and_change_nothing() {
        let tmp = TempDir::new();
        let dir = project(&tmp, "p");
        create_topic(&dir, "Crack", &["W1"]);
        let good = tmp.path().join("good.bcfzip");
        assert_eq!(run(&["bcf", &dir, "export", good.to_str().unwrap()]).0, 0);
        let other = project(&tmp, "other");
        let before = (journal(&dir), journal(&other));
        let not_zip = tmp.path().join("bad.bcfzip");
        std::fs::write(&not_zip, "not a zip").unwrap();
        let missing_component = {
            // The same archive imported into a project that lacks the entities of its viewpoints.
            let bare = tmp.path().join("bare").to_str().unwrap().to_string();
            assert_eq!(run(&["init", &bare]).0, 0);
            bare
        };
        let missing_before = journal(&missing_component);
        let (g, p) = (good.to_str().unwrap(), not_zip.to_str().unwrap());
        for args in [
            vec!["bcf", &dir, "import", p],
            vec!["bcf", &dir, "import", "/nonexistent/x.bcfzip"],
            vec!["bcf", &missing_component, "import", g],
            vec!["bcf", &dir, "create", "--title", "x", "--entity", "no-such-entity"],
            vec!["bcf", &dir, "create", "--title", "x", "--entity", "W1", "no-such-entity"],
            vec!["bcf", &dir, "create", "--title", "  ", "--entity", "W1"],
            vec!["bcf", &dir, "create", "--title", "a\u{1}b", "--entity", "W1"],
            vec!["bcf", "/nonexistent/project", "export", "/tmp/x.bcfzip"],
            vec!["bcf", &dir, "export", "/nonexistent/dir/x.bcfzip"],
        ] {
            let (code, out, err) = run(&args);
            assert_eq!((code, out.as_str()), (2, ""), "{args:?}: {err}");
            assert!(err.starts_with("bim: "), "{err}");
        }
        assert_eq!((journal(&dir), journal(&other)), before);
        assert_eq!(journal(&missing_component), missing_before);
        let (_, _, err) = run(&["bcf", &missing_component, "import", g]);
        assert!(err.contains("is not an entity of this project"), "{err}");
    }

    #[test]
    fn a_failing_topic_leaves_every_topic_unapplied() {
        let tmp = TempDir::new();
        let (a, partial) = (project(&tmp, "a"), tmp.path().join("partial").to_str().unwrap().to_string());
        create_topic(&a, "needs W1", &["W1"]);
        create_topic(&a, "needs W2", &["W2"]);
        create_topic(&a, "needs both", &["W1", "W2"]);
        let file = tmp.path().join("a.bcfzip");
        assert_eq!(run(&["bcf", &a, "export", file.to_str().unwrap()]).0, 0);
        // `partial` has W1 but not W2: whichever topics come first, none may be applied.
        assert_eq!(run(&["init", &partial]).0, 0);
        let ops = tmp.path().join("partial.ops");
        std::fs::write(&ops, "create L0 Level\ncreate WT WallType\ncreate W1 Wall\n").unwrap();
        assert_eq!(run(&["apply", &partial, ops.to_str().unwrap()]).0, 0);
        let before = journal(&partial);
        let (code, out, err) = run(&["bcf", &partial, "import", file.to_str().unwrap()]);
        assert_eq!((code, out.as_str()), (2, ""), "{err}");
        assert!(err.contains("is not an entity of this project"), "{err}");
        assert_eq!(journal(&partial), before);
        assert_eq!(run(&["hash", &partial]).1.trim().len(), 16);
    }

    #[test]
    fn usage_errors() {
        let tmp = TempDir::new();
        let dir = project(&tmp, "p");
        let before = journal(&dir);
        for args in [
            vec!["bcf"],
            vec!["bcf", &dir],
            vec!["bcf", &dir, "frobnicate"],
            vec!["bcf", &dir, "create"],
            vec!["bcf", &dir, "create", "--title", "t"],
            vec!["bcf", &dir, "create", "--entity", "W1"],
            vec!["bcf", &dir, "create", "--title", "--entity", "W1"],
            vec!["bcf", &dir, "create", "--title", "t", "--entity"],
            vec!["bcf", &dir, "create", "--title", "t", "--entity", "--title", "u"],
            vec!["bcf", &dir, "create", "--title", "t", "--title", "u", "--entity", "W1"],
            vec!["bcf", &dir, "create", "stray", "--title", "t", "--entity", "W1"],
            vec!["bcf", &dir, "create", "--title", "t", "stray", "--entity", "W1"],
            vec!["bcf", &dir, "create", "--title", "t", "--entity", "W1", "--bogus"],
            vec!["bcf", &dir, "create", "t", "W1"],
            vec!["bcf", &dir, "export"],
            vec!["bcf", &dir, "export", "a", "b"],
            vec!["bcf", &dir, "import"],
            vec!["bcf", &dir, "import", "a", "b"],
        ] {
            let (code, out, err) = run(&args);
            assert_eq!((code, out.as_str()), (2, ""), "{args:?}: {err}");
            assert!(err.starts_with("bim: ") && err.contains("usage:"), "{args:?}: {err}");
        }
        assert_eq!(journal(&dir), before);
    }

    #[test]
    fn create_accepts_its_arguments_in_either_order_and_repeated() {
        let tmp = TempDir::new();
        let dir = project(&tmp, "p");
        let g = create_topic_raw(&dir, &["--entity", "W1", "--title", "t", "--entity", "W2"]);
        let shown = show(&dir, &g);
        assert!(shown.contains("\"entities\":[{\"ref\":\"W1\"},{\"ref\":\"W2\"}]"), "{shown}");
    }

    fn create_topic_raw(dir: &str, rest: &[&str]) -> String {
        let mut args = vec!["bcf", dir, "create"];
        args.extend(rest);
        let (code, out, err) = run(&args);
        assert_eq!(code, 0, "{err}");
        out.trim_end().to_string()
    }

    #[test]
    fn an_import_that_names_a_non_issue_entity_is_refused() {
        let tmp = TempDir::new();
        let (a, b) = (project(&tmp, "a"), project(&tmp, "b"));
        let g = create_topic(&a, "t", &["W1"]);
        let file = tmp.path().join("a.bcfzip");
        assert_eq!(run(&["bcf", &a, "export", file.to_str().unwrap()]).0, 0);
        let squat = tmp.path().join("squat.ops");
        std::fs::write(&squat, format!("create {g} Level\n")).unwrap();
        assert_eq!(run(&["apply", &b, squat.to_str().unwrap()]).0, 0);
        let before = journal(&b);
        let (code, _, err) = run(&["bcf", &b, "import", file.to_str().unwrap()]);
        assert_eq!(code, 2, "{err}");
        assert!(err.contains("not an Issue"), "{err}");
        assert_eq!(journal(&b), before);
    }

    #[test]
    fn an_empty_project_exports_an_archive_with_no_topics_and_imports_it() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("e").to_str().unwrap().to_string();
        assert_eq!(run(&["init", &dir]).0, 0);
        let file = tmp.path().join("e.bcfzip");
        assert_eq!(run(&["bcf", &dir, "export", file.to_str().unwrap()]).0, 0);
        assert_eq!(bim_bcf::read_bcfzip(&std::fs::read(&file).unwrap()).unwrap(), Vec::new());
        let before = journal(&dir);
        assert_eq!(run(&["bcf", &dir, "import", file.to_str().unwrap()]).0, 0);
        assert_eq!(journal(&dir), before);
    }

    #[test]
    fn the_camera_frames_the_entities_with_geometry() {
        let tmp = TempDir::new();
        let dir = project(&tmp, "p");
        let g = create_topic(&dir, "t", &["W1"]);
        let file = tmp.path().join("p.bcfzip");
        assert_eq!(run(&["bcf", &dir, "export", file.to_str().unwrap()]).0, 0);
        let topics = bim_bcf::read_bcfzip(&std::fs::read(&file).unwrap()).unwrap();
        let camera = topics[0].viewpoints[0].camera.unwrap();
        assert!(camera.is_valid());
        assert_ne!(camera, Camera::default_view(), "W1 has geometry, so the view is fitted to it");
        // An entity without geometry gets the default view.
        let g2 = create_topic(&dir, "level only", &["L0"]);
        assert_ne!(g, g2);
        assert_eq!(run(&["bcf", &dir, "export", file.to_str().unwrap()]).0, 0);
        let topics = bim_bcf::read_bcfzip(&std::fs::read(&file).unwrap()).unwrap();
        let level_topic = topics.iter().find(|t| t.guid == g2).unwrap();
        assert_eq!(level_topic.viewpoints[0].camera, Some(Camera::default_view()));
    }
}
