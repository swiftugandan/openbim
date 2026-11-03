//! Whole `.bcfzip` archives: [`read_bcfzip`] and [`write_bcfzip`].
//!
//! Layout written (and expected on reading): `bcf.version` and an optional `extensions.xml` at the
//! root, then one folder per topic named by the topic's Guid, holding `markup.bcf` and one
//! `<viewpoint guid>.bcfv` per viewpoint. Reading also accepts a topic folder with any name, a
//! viewpoint file with any name (the markup's `Viewpoint` element says which), and other files
//! (snapshots, `project.bcfp`, `documents.xml`, …), which it ignores.

use std::collections::{BTreeMap, HashMap};

use crate::markup::{parse_markup, parse_visinfo, viewpoint_file, write_extensions, write_markup, write_version, write_visinfo, VERSION};
use crate::model::{Topic, Viewpoint};
use crate::xml;
use crate::zip::{read_zip, write_zip};
use crate::BcfError;

const MARKUP: &str = "markup.bcf";
const VERSION_FILE: &str = "bcf.version";

/// Reads a BCF 3.0 archive: every topic, sorted by Guid.
///
/// Strict: a file that is not a ZIP archive, an archive that is not BCF 3.0 (no `bcf.version`, or
/// another version in it), a `markup.bcf` outside a topic folder, malformed XML, a topic
/// that breaks the rules of [`crate::markup`], a viewpoint file the markup names but the archive
/// lacks, and a topic Guid used twice are all errors that name what is wrong.
pub fn read_bcfzip(bytes: &[u8]) -> Result<Vec<Topic>, BcfError> {
    let files: HashMap<String, Vec<u8>> = read_zip(bytes)?.into_iter().collect();
    let in_folder = format!("/{MARKUP}");
    let markups: Vec<&String> = {
        let mut names: Vec<&String> = files.keys().filter(|name| *name == MARKUP || name.ends_with(&in_folder)).collect();
        names.sort();
        names
    };
    let Some(version_file) = files.get(VERSION_FILE) else {
        return Err(BcfError::new(format!("not a BCF archive: {VERSION_FILE} is missing")));
    };
    let root = xml::parse(version_file).map_err(|e| BcfError::new(format!("{VERSION_FILE}: {e}")))?;
    let version = root.attr("VersionId").map(str::trim);
    if root.name != "Version" || version != Some(VERSION) {
        return Err(BcfError::new(format!(
            "unsupported BCF version {} (only {VERSION})",
            version.map_or_else(|| "(none)".to_string(), |v| format!("`{v}`"))
        )));
    }

    let mut topics: BTreeMap<String, Topic> = BTreeMap::new();
    for name in markups {
        let Some(folder) = name.strip_suffix(&in_folder).filter(|f| !f.is_empty() && !f.contains('/')) else {
            return Err(BcfError::new(format!("{name}: a topic's markup.bcf belongs directly inside its own folder")));
        };
        let data = files.get(name).map(Vec::as_slice).unwrap_or_default();
        let root = xml::parse(data).map_err(|e| BcfError::new(format!("{name}: {e}")))?;
        let parsed = parse_markup(&root).map_err(|e| BcfError::new(format!("{name}: {e}")))?;
        let mut topic = parsed.topic;
        for (guid, file) in parsed.viewpoint_files {
            let (components, camera) = match file {
                Some(file) => {
                    let path = format!("{folder}/{file}");
                    let data = files
                        .get(&path)
                        .ok_or_else(|| BcfError::new(format!("topic {}: viewpoint file {path} is missing from the archive", topic.guid)))?;
                    let root = xml::parse(data).map_err(|e| BcfError::new(format!("{path}: {e}")))?;
                    parse_visinfo(&root).map_err(|e| BcfError::new(format!("{path}: {e}")))?
                }
                None => (Vec::new(), None),
            };
            topic.viewpoints.push(Viewpoint { guid, components, camera });
        }
        let guid = topic.guid.clone();
        if topics.insert(guid.clone(), topic).is_some() {
            return Err(BcfError::new(format!("topic {guid} appears in more than one folder")));
        }
    }
    Ok(topics.into_values().collect())
}

/// Writes `topics` as a BCF 3.0 archive: `bcf.version`, `extensions.xml` (the types, statuses and
/// labels in use, if any), then the topics sorted by Guid, each as `<guid>/markup.bcf` followed by
/// its `<viewpoint guid>.bcfv` files. The same topics always give the same bytes. A topic Guid
/// listed twice is an error.
pub fn write_bcfzip(topics: &[Topic]) -> Result<Vec<u8>, BcfError> {
    let mut sorted: Vec<&Topic> = topics.iter().collect();
    sorted.sort_by(|a, b| a.guid.cmp(&b.guid));
    if let Some(pair) = sorted.windows(2).find(|pair| pair[0].guid == pair[1].guid) {
        return Err(BcfError::new(format!("topic {} is listed twice", pair[0].guid)));
    }
    let mut entries: Vec<(String, Vec<u8>)> = vec![(VERSION_FILE.to_string(), write_version().into_bytes())];
    if let Some(extensions) = write_extensions(topics) {
        entries.push(("extensions.xml".to_string(), extensions.into_bytes()));
    }
    for topic in sorted {
        entries.push((format!("{}/{MARKUP}", topic.guid), write_markup(topic).into_bytes()));
        for viewpoint in &topic.viewpoints {
            entries.push((format!("{}/{}", topic.guid, viewpoint_file(&viewpoint.guid)), write_visinfo(viewpoint).into_bytes()));
        }
    }
    write_zip(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Camera, Comment, Projection};

    const T1: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const T2: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const V1: &str = "11111111-1111-4111-8111-111111111111";
    const V2: &str = "22222222-2222-4222-8222-222222222222";
    const V3: &str = "33333333-3333-4333-8333-333333333333";
    const C1: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    const C2: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";

    fn topics() -> Vec<Topic> {
        let base = Camera::default_view();
        vec![
            Topic {
                guid: T1.to_string(),
                title: "Wall W1 clashes with column – prüfen".to_string(),
                topic_type: "Clash".to_string(),
                status: "Open".to_string(),
                creation_date: "2026-03-01T09:30:00+00:00".to_string(),
                creation_author: "alice@example.com".to_string(),
                description: Some("Säule/Wand <&> overlap".to_string()),
                labels: vec!["Structural".to_string(), "Architecture".to_string()],
                comments: vec![
                    Comment {
                        guid: C1.to_string(),
                        date: "2026-03-01T10:00:00+00:00".to_string(),
                        author: "bob@example.com".to_string(),
                        text: "Confirmed on site 'north' – 2 cm".to_string(),
                        viewpoint: Some(V1.to_string()),
                    },
                    Comment {
                        guid: C2.to_string(),
                        date: "2026-03-02T08:15:30+01:00".to_string(),
                        author: "alice@example.com".to_string(),
                        text: "Moved column by 50 mm".to_string(),
                        viewpoint: None,
                    },
                ],
                viewpoints: vec![
                    Viewpoint { guid: V1.to_string(), components: vec!["0aaaaaaaaaaaaaaaaaaaaa".to_string(), "1bbbbbbbbbbbbbbbbbbbbb".to_string()], camera: Some(base) },
                    Viewpoint {
                        guid: V2.to_string(),
                        components: vec!["2ccccccccccccccccccccc".to_string()],
                        camera: Some(Camera { projection: Projection::Orthogonal { view_to_world_scale: 12.5 }, ..base }),
                    },
                    Viewpoint { guid: V3.to_string(), components: Vec::new(), camera: None },
                ],
            },
            Topic {
                guid: T2.to_string(),
                title: "Missing fire rating".to_string(),
                topic_type: "Issue".to_string(),
                status: "Closed".to_string(),
                creation_date: "2026-04-11T14:00:00Z".to_string(),
                creation_author: "carol@example.com".to_string(),
                description: None,
                labels: Vec::new(),
                comments: Vec::new(),
                viewpoints: Vec::new(),
            },
        ]
    }

    /// What reading gives back for `topics()`: a viewpoint without a camera comes back with the default one.
    fn expected() -> Vec<Topic> {
        let mut topics = topics();
        if let Some(viewpoint) = topics.get_mut(0).and_then(|t| t.viewpoints.get_mut(2)) {
            viewpoint.camera = Some(Camera::default_view());
        }
        topics
    }

    fn names(zip: &[u8]) -> Vec<String> {
        read_zip(zip).unwrap().into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn archives_round_trip_and_are_deterministic() {
        let zip = write_bcfzip(&topics()).unwrap();
        assert_eq!(read_bcfzip(&zip).unwrap(), expected());
        let mut reversed = topics();
        reversed.reverse();
        assert_eq!(write_bcfzip(&reversed).unwrap(), zip, "topics are written sorted by Guid");
        assert_eq!(
            names(&zip),
            vec![
                "bcf.version".to_string(),
                "extensions.xml".to_string(),
                format!("{T1}/markup.bcf"),
                format!("{T1}/{V1}.bcfv"),
                format!("{T1}/{V2}.bcfv"),
                format!("{T1}/{V3}.bcfv"),
                format!("{T2}/markup.bcf"),
            ]
        );
    }

    #[test]
    fn an_archive_without_topics_is_just_the_version_file() {
        let zip = write_bcfzip(&[]).unwrap();
        assert_eq!(names(&zip), vec!["bcf.version".to_string()]);
        assert_eq!(read_bcfzip(&zip).unwrap(), Vec::new());
    }

    #[test]
    fn writing_refuses_a_topic_listed_twice() {
        let mut twice = topics();
        twice.push(topics().remove(0));
        assert!(write_bcfzip(&twice).unwrap_err().to_string().contains("listed twice"));
    }

    fn archive_with(topics: &[Topic], edit: impl Fn(&str, Vec<u8>) -> Option<Vec<u8>>) -> Vec<u8> {
        let entries: Vec<(String, Vec<u8>)> = read_zip(&write_bcfzip(topics).unwrap())
            .unwrap()
            .into_iter()
            .filter_map(|(name, data)| edit(&name, data).map(|data| (name, data)))
            .collect();
        write_zip(&entries).unwrap()
    }

    fn replace_in(name_suffix: &str, from: &str, to: &str) -> impl Fn(&str, Vec<u8>) -> Option<Vec<u8>> {
        let (name_suffix, from, to) = (name_suffix.to_string(), from.to_string(), to.to_string());
        move |name, data| {
            if name.ends_with(&name_suffix) {
                let text = String::from_utf8(data).unwrap();
                assert!(text.contains(&from), "{name} lacks {from}");
                Some(text.replace(&from, &to).into_bytes())
            } else {
                Some(data)
            }
        }
    }

    #[test]
    fn reading_is_strict_about_the_archive() {
        let t = topics();
        let err = |zip: Vec<u8>| read_bcfzip(&zip).unwrap_err().to_string();
        assert!(err(b"not a zip".to_vec()).contains("not a zip archive"));
        assert!(err(write_zip(&[("readme.txt".to_string(), b"hi".to_vec())]).unwrap()).contains("not a BCF archive"));
        assert!(err(archive_with(&t, |name, data| (name != "bcf.version").then_some(data))).contains("bcf.version is missing"));
        assert!(err(archive_with(&t, replace_in("bcf.version", "3.0", "2.1"))).contains("unsupported BCF version `2.1`"));
        assert!(err(archive_with(&t, replace_in("bcf.version", "VersionId", "Other"))).contains("unsupported BCF version (none)"));
        assert!(err(archive_with(&t, replace_in("bcf.version", "<Version ", "<Other "))).contains("unsupported"));
        assert!(err(archive_with(&t, |name, data| (!name.ends_with(".bcfv") || !name.contains(V1)).then_some(data))).contains("is missing from the archive"));
        assert!(err(archive_with(&t, replace_in(&format!("{T2}/markup.bcf"), "Missing fire rating", "  "))).contains("Title"));
        assert!(err(archive_with(&t, replace_in(&format!("{T1}/markup.bcf"), "<Title>", "<Title><b>"))).contains("markup.bcf"));
        // A markup.bcf at the root or too deep.
        let zip = write_bcfzip(&t).unwrap();
        for bad in ["markup.bcf", "a/b/markup.bcf", "/x/markup.bcf/"] {
            let mut entries = read_zip(&zip).unwrap();
            let data = entries.iter().find(|(n, _)| n.ends_with("markup.bcf")).unwrap().1.clone();
            entries.push((bad.to_string(), data));
            let zip = write_zip(&entries).unwrap();
            let result = read_bcfzip(&zip);
            if bad.ends_with('/') {
                assert!(result.is_ok(), "a directory entry is ignored");
            } else {
                assert!(result.unwrap_err().to_string().contains("own folder"), "{bad}");
            }
        }
        // The same topic in two folders.
        let mut entries = read_zip(&zip).unwrap();
        let data = entries.iter().find(|(n, _)| n.starts_with(T2) && n.ends_with("markup.bcf")).unwrap().1.clone();
        entries.push(("copy/markup.bcf".to_string(), data));
        assert!(read_bcfzip(&write_zip(&entries).unwrap()).unwrap_err().to_string().contains("more than one folder"));
    }

    #[test]
    fn reading_accepts_other_names_and_files() {
        let zip = write_bcfzip(&topics()).unwrap();
        let mut entries = read_zip(&zip).unwrap();
        entries.retain(|(name, _)| name != "extensions.xml");
        entries.push(("project.bcfp".to_string(), b"<ProjectExtension/>".to_vec()));
        entries.push((format!("{T1}/snapshot.png"), vec![0x89, b'P', b'N', b'G']));
        entries.push(("documents.xml".to_string(), b"<Documents/>".to_vec()));
        let read = read_bcfzip(&write_zip(&entries).unwrap()).unwrap();
        assert_eq!(read, expected(), "other files are ignored");

        // Folder and viewpoint file names are free; the markup names the file.
        let mut entries = read_zip(&zip).unwrap();
        for (name, data) in entries.iter_mut() {
            if name.starts_with(T1) {
                *name = name.replacen(T1, "my topic", 1).replace(".bcfv", ".xml");
                if name.ends_with("markup.bcf") {
                    *data = String::from_utf8(data.clone()).unwrap().replace(".bcfv", ".xml").into_bytes();
                }
            }
        }
        assert_eq!(read_bcfzip(&write_zip(&entries).unwrap()).unwrap(), expected());
    }
}
