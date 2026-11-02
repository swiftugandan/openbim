use crate::error::{ErrorKind, StepError};
use std::collections::HashMap;

/// One Part 21 parameter value.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    /// `$`: no value.
    Unset,
    /// `*`: value derived by a subtype (redeclared attribute).
    Derived,
    Integer(i64),
    Real(f64),
    /// A string literal, decoded (escape directives resolved).
    String(String),
    /// A binary literal as a string of `0`/`1`, the unused leading bits dropped.
    Binary(String),
    /// An enumeration value (incl. booleans/logicals `T`, `F`, `U`) without the dots.
    Enum(String),
    /// `#id`: an entity instance reference.
    Ref(u64),
    List(Vec<Param>),
    /// A typed value such as `IFCLABEL('x')`; the type name is upper-case.
    Typed(String, Box<Param>),
}

impl Param {
    pub fn string(s: impl Into<String>) -> Param {
        Param::String(s.into())
    }
    pub fn enumeration(s: impl Into<String>) -> Param {
        Param::Enum(s.into())
    }
    /// `.T.` / `.F.`
    pub fn boolean(b: bool) -> Param {
        Param::Enum(if b { "T" } else { "F" }.into())
    }
    /// A typed value; the type name is stored upper-case.
    pub fn typed(name: &str, inner: Param) -> Param {
        Param::Typed(name.to_ascii_uppercase(), Box::new(inner))
    }
    /// A list of references.
    pub fn refs(ids: impl IntoIterator<Item = u64>) -> Param {
        Param::List(ids.into_iter().map(Param::Ref).collect())
    }
    /// A list of reals.
    pub fn reals(xs: impl IntoIterator<Item = f64>) -> Param {
        Param::List(xs.into_iter().map(Param::Real).collect())
    }

    pub fn is_unset(&self) -> bool {
        matches!(self, Param::Unset)
    }
    /// The referenced instance id, looking through a typed wrapper.
    pub fn as_ref_id(&self) -> Option<u64> {
        match self {
            Param::Ref(id) => Some(*id),
            Param::Typed(_, inner) => inner.as_ref_id(),
            _ => None,
        }
    }
    /// The decoded string, looking through a typed wrapper (`IFCLABEL('x')` gives `x`).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Param::String(s) => Some(s),
            Param::Typed(_, inner) => inner.as_str(),
            _ => None,
        }
    }
    /// A real or an integer as `f64`, looking through a typed wrapper.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Param::Real(x) => Some(*x),
            Param::Integer(i) => Some(*i as f64),
            Param::Typed(_, inner) => inner.as_f64(),
            _ => None,
        }
    }
    /// An integer, looking through a typed wrapper.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Param::Integer(i) => Some(*i),
            Param::Typed(_, inner) => inner.as_i64(),
            _ => None,
        }
    }
    /// An enumeration value, looking through a typed wrapper.
    pub fn as_enum(&self) -> Option<&str> {
        match self {
            Param::Enum(e) => Some(e),
            Param::Typed(_, inner) => inner.as_enum(),
            _ => None,
        }
    }
    /// `.T.` → true, `.F.` → false (looking through a typed wrapper); anything else → None.
    pub fn as_bool(&self) -> Option<bool> {
        match self.as_enum() {
            Some("T") => Some(true),
            Some("F") => Some(false),
            _ => None,
        }
    }
    /// The items of a list, looking through a typed wrapper.
    pub fn as_list(&self) -> Option<&[Param]> {
        match self {
            Param::List(items) => Some(items),
            Param::Typed(_, inner) => inner.as_list(),
            _ => None,
        }
    }
    /// The referenced ids of a list (non-reference items are skipped).
    pub fn ref_ids(&self) -> Vec<u64> {
        self.as_list().map(|l| l.iter().filter_map(Param::as_ref_id).collect()).unwrap_or_default()
    }
    /// The numbers of a list (non-numeric items are skipped).
    pub fn f64s(&self) -> Vec<f64> {
        self.as_list().map(|l| l.iter().filter_map(Param::as_f64).collect()).unwrap_or_default()
    }
    /// The type name and inner value of a typed value.
    pub fn as_typed(&self) -> Option<(&str, &Param)> {
        match self {
            Param::Typed(name, inner) => Some((name, inner)),
            _ => None,
        }
    }
}

/// One entity instance of the DATA section.
///
/// A complex (multi-leaf) instance `#5=(A(1)B(2));` is stored with an empty `entity` and one
/// `Param::Typed(NAME, Box::new(Param::List(params)))` per partial entity, in file order; see
/// [`Instance::complex_parts`]. The writer turns that form back into the complex syntax.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub id: u64,
    /// Upper-case entity name (empty for a complex instance).
    pub entity: String,
    pub params: Vec<Param>,
}

impl Instance {
    pub fn new(id: u64, entity: &str, params: Vec<Param>) -> Instance {
        Instance { id, entity: entity.to_ascii_uppercase(), params }
    }
    pub fn is_complex(&self) -> bool {
        self.entity.is_empty()
    }
    /// The partial entities `(NAME, params)` of a complex instance (empty for a simple one).
    pub fn complex_parts(&self) -> Vec<(&str, &[Param])> {
        if !self.is_complex() {
            return Vec::new();
        }
        self.params
            .iter()
            .filter_map(|p| match p {
                Param::Typed(name, inner) => match inner.as_ref() {
                    Param::List(ps) => Some((name.as_str(), ps.as_slice())),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }
    /// Is this instance (case-insensitively) of exactly the given entity type?
    pub fn is(&self, entity: &str) -> bool {
        self.entity.eq_ignore_ascii_case(entity)
    }
}

/// One header-section entry such as `FILE_SCHEMA(('IFC4'));`.
#[derive(Debug, Clone, PartialEq)]
pub struct HeaderEntry {
    /// Upper-case entity name (`FILE_DESCRIPTION`, `FILE_NAME`, `FILE_SCHEMA`, ...).
    pub entity: String,
    pub params: Vec<Param>,
}

impl HeaderEntry {
    pub fn new(entity: &str, params: Vec<Param>) -> HeaderEntry {
        HeaderEntry { entity: entity.to_ascii_uppercase(), params }
    }
}

/// A STEP physical file: header entries and the instances of its DATA section(s), in file order,
/// indexed by id.
#[derive(Debug, Clone, Default)]
pub struct StepFile {
    pub header: Vec<HeaderEntry>,
    instances: Vec<Instance>,
    index: HashMap<u64, usize>,
    max_id: u64,
}

impl StepFile {
    /// An empty file with the given header entries.
    pub fn new(header: Vec<HeaderEntry>) -> StepFile {
        StepFile { header, ..StepFile::default() }
    }

    /// An empty file with the standard header for `schema` (e.g. `IFC4`, `IFC2X3`).
    pub fn with_schema(schema: &str, file_name: &str, time_stamp: &str, originating_system: &str) -> StepFile {
        StepFile::new(standard_header(schema, file_name, time_stamp, originating_system))
    }

    /// All instances in file / insertion order.
    pub fn instances(&self) -> impl Iterator<Item = &Instance> + '_ {
        self.instances.iter()
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// The instance with this id.
    pub fn get(&self, id: u64) -> Option<&Instance> {
        self.index.get(&id).and_then(|&k| self.instances.get(k))
    }

    /// The instance with this id, mutably (its id must not be changed).
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Instance> {
        match self.index.get(&id) {
            Some(&k) => self.instances.get_mut(k),
            None => None,
        }
    }

    /// Instances of exactly this entity type (case-insensitive), in file order.
    pub fn instances_of<'a>(&'a self, entity: &'a str) -> impl Iterator<Item = &'a Instance> + 'a {
        self.instances.iter().filter(move |i| i.is(entity))
    }

    /// The largest instance id in use (0 for an empty file).
    pub fn max_id(&self) -> u64 {
        self.max_id
    }

    /// Adds an instance with the next free id (one more than the largest id in use) and returns
    /// that id.
    pub fn add(&mut self, entity: &str, params: Vec<Param>) -> u64 {
        let id = self.max_id + 1;
        self.push_unchecked(Instance::new(id, entity, params));
        id
    }

    /// Adds an instance with its own id; an id already in use is an error.
    pub fn insert(&mut self, instance: Instance) -> Result<(), StepError> {
        if self.index.contains_key(&instance.id) {
            return Err(StepError::new(
                ErrorKind::DuplicateId,
                0,
                format!("instance #{} already exists", instance.id),
            ));
        }
        self.push_unchecked(instance);
        Ok(())
    }

    pub(crate) fn push_unchecked(&mut self, instance: Instance) {
        self.max_id = self.max_id.max(instance.id);
        self.index.insert(instance.id, self.instances.len());
        self.instances.push(instance);
    }

    /// The header entry with this name (case-insensitive).
    pub fn header_entry(&self, entity: &str) -> Option<&HeaderEntry> {
        self.header.iter().find(|h| h.entity.eq_ignore_ascii_case(entity))
    }

    /// The schema identifiers of `FILE_SCHEMA` (e.g. `["IFC4"]`).
    pub fn schema_identifiers(&self) -> Vec<String> {
        self.header_entry("FILE_SCHEMA")
            .and_then(|h| h.params.first())
            .and_then(Param::as_list)
            .map(|l| l.iter().filter_map(|p| p.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    /// Serialises the file to a string.
    pub fn to_step_string(&self) -> String {
        let mut out = Vec::new();
        // Writing to a Vec cannot fail except for unrepresentable values (NaN).
        match self.write(&mut out) {
            Ok(()) => String::from_utf8(out).unwrap_or_default(),
            Err(_) => String::new(),
        }
    }
}

/// The three mandatory header entries for a file of `schema`.
pub fn standard_header(schema: &str, file_name: &str, time_stamp: &str, originating_system: &str) -> Vec<HeaderEntry> {
    vec![
        HeaderEntry::new(
            "FILE_DESCRIPTION",
            vec![Param::List(vec![Param::string("ViewDefinition [CoordinationView]")]), Param::string("2;1")],
        ),
        HeaderEntry::new(
            "FILE_NAME",
            vec![
                Param::string(file_name),
                Param::string(time_stamp),
                Param::List(vec![Param::string("")]),
                Param::List(vec![Param::string("")]),
                Param::string(originating_system),
                Param::string(originating_system),
                Param::string(""),
            ],
        ),
        HeaderEntry::new("FILE_SCHEMA", vec![Param::List(vec![Param::string(schema)])]),
    ]
}
