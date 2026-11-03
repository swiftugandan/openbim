//! ifcJSON 0.0.1 export and import (`bim export <dir> --format ifcjson`,
//! `bim import <dir> <file.json>`).
//!
//! **The format.** `{"type":"ifcJSON","version":"0.0.1","schemaIdentifier":"IFC4","data":[…]}`.
//! `data` holds one object per `IfcRoot` instance, relationships last. `type` is the IFC entity
//! name; `globalId` is the GlobalId in UUID form (the 22-character form is read too and names the
//! same entity, [`guid::parse_global_id`]); the other members are the attributes in lowerCamelCase,
//! in schema order, with unset and derived (`$`, `*`) attributes left out. A reference to another
//! `IfcRoot` object is `{"type":…,"ref":<globalId>}`; every other entity is nested inline, and
//! repeated wherever it is used. Typed values are `{"type":"IfcLabel","value":…}`, enumerations
//! plain strings, `IfcBoolean`s JSON booleans, a logical `UNKNOWN` the string `"UNKNOWN"`, reals
//! always written with a decimal point or an exponent and integers without.
//!
//! **Export** ([`export_ifcjson`]) is the generic ifcJSON spelling of the IFC4 file of
//! [`export_ifc4`] ([`step_to_ifcjson`]), so other tools read the building as IFC. That file does not
//! hold the document: a WallType, a Material or a SlabOpening has no IFC object, and reading the
//! IFC back (`doc → IFC → doc`) changes names, units and derived entities. So every document
//! entity also appears once as an extension member [`EXTENSION_KEY`] (`openbimEntity`) holding
//! its kind, its name and its props exactly (numbers as JSON numbers, refs as
//! `{"ref":<uuid>}`): on the `IfcRoot` object whose GlobalId is the entity's id, or, for an entity
//! the IFC export has no object for, on an `IfcProxy` object with that GlobalId. The output is a
//! pure function of the document's content.
//!
//! **Import** ([`import_ifcjson`]) has two modes:
//!
//! * a file with `openbimEntity` members (an export of `bim`) *rebuilds the document*: one entity
//!   per member, with the id of its GlobalId, the recorded kind, props and name. Doing `doc →
//!   ifcJSON → doc` preserves `content_hash` (provenance props excepted, as everywhere) for every
//!   document whose entities have a `_name` (as every entity `bim` makes) and `_schema` 1. The IFC
//!   content of those files is not read, objects without the member are ignored;
//! * any other file is IFC content: it is turned into a [`StepFile`] ([`ifcjson_to_step`]) and
//!   imported like the `.ifc` it stands for, [`import_ifc`], so ifcJSON made by other tools has
//!   the same entities and volumes as the IFC it was made from.
//!
//! Malformed input (truncated or invalid JSON, a document that is not ifcJSON, a bad GlobalId, a
//! duplicate GlobalId, a malformed `openbimEntity`) is an [`ImportError`]; nothing is imported
//! then.
//!
//! **Known gaps of the IFC reading.** The schema tables name the enumeration-typed attributes
//! ([`ENUM_ATTRIBUTES`]) so that their string values become Part 21 enumerations; a binary
//! attribute (`IfcPixelTexture.Pixel`) stays a string; entities outside the tables are kept with
//! positional attributes only.

use std::collections::{BTreeMap, HashMap};

use bim_doc::{Document, Entity, EntityId, Value, PROVENANCE_PROPS};
use serde_json::{Map, Number, Value as Json};

use crate::export::export_ifc4;
use crate::guid;
use crate::import::{import_ifc, ImportError, Imported, ImportedEntity};
use crate::model::{Instance, Param, StepFile};
use crate::schema::{EntityDef, Schema};

/// The ifcJSON version written.
pub const VERSION: &str = "0.0.1";

/// The member of an exported object that holds the document entity (see the module docs).
pub const EXTENSION_KEY: &str = "openbimEntity";

const ORIGINATING_SYSTEM: &str = "openbim bim-ifc";

/// Deepest nesting of objects and lists written below the `data` list. `serde_json` reads at most
/// 127 levels, so a file with deeper nesting could not be read back.
const MAX_DEPTH: usize = 100;

/// A bound on the recursion of the reader. `serde_json` has limited the input to 127 levels
/// already, so this is never reached.
const MAX_READ_DEPTH: usize = 127;

/// Instance ids from here up stand for references the file does not define (never inserted).
const DANGLING_BASE: u64 = 1 << 62;

type Object = Map<String, Json>;

// ------------------------------------------------------------------ StepFile -> ifcJSON

/// The ifcJSON document of an IFC file (no extension members): one object per `IfcRoot` instance,
/// relationships last, in file order otherwise. Fails for an entity outside the schema tables
/// (complex instances included), a malformed GlobalId, a real without a JSON form (NaN, infinity)
/// and objects nested deeper than 100 levels.
pub fn step_to_ifcjson(file: &StepFile) -> Result<Json, String> {
    let (plain, rels) = objects(file)?;
    Ok(document(file, plain.into_iter().chain(rels).map(Json::Object).collect()))
}

/// The ifcJSON document of `doc`: [`step_to_ifcjson`] of its IFC4 export, with every entity's exact
/// content as an [`EXTENSION_KEY`] member (module docs). Fails when a prop has no JSON form (a
/// number that is NaN or infinite) or the IFC export does ([`step_to_ifcjson`]).
pub fn export_ifcjson(doc: &Document) -> Result<Json, String> {
    let file = export_ifc4(doc, "");
    let (mut plain, rels) = objects(&file)?;
    // The objects an entity's id is the GlobalId of; each carries at most one entity.
    let mut carriers: HashMap<u128, usize> = HashMap::new();
    for (k, object) in plain.iter().enumerate() {
        if let Some(id) = object.get("globalId").and_then(Json::as_str).and_then(guid::parse_uuid) {
            carriers.entry(id).or_insert(k);
        }
    }
    let mut proxies = Vec::new();
    for e in doc.entities() {
        let payload = Json::Object(payload_of(e)?);
        match carriers.remove(&e.id.0) {
            Some(k) => {
                plain[k].insert(EXTENSION_KEY.to_string(), payload);
            }
            None => proxies.push(proxy_of(e, payload)),
        }
    }
    Ok(document(&file, plain.into_iter().chain(proxies).chain(rels).map(Json::Object).collect()))
}

/// [`export_ifcjson`] as text: indented JSON ending in a newline.
pub fn export_ifcjson_string(doc: &Document) -> Result<String, String> {
    let json = export_ifcjson(doc)?;
    let mut text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

fn document(file: &StepFile, data: Vec<Json>) -> Json {
    let identifier = file.schema_identifiers().into_iter().next().unwrap_or_else(|| file.schema().identifier().to_string());
    let mut root = Object::new();
    root.insert("type".into(), "ifcJSON".into());
    root.insert("version".into(), VERSION.into());
    root.insert("schemaIdentifier".into(), identifier.into());
    root.insert("originatingSystem".into(), ORIGINATING_SYSTEM.into());
    root.insert("data".into(), Json::Array(data));
    Json::Object(root)
}

/// The top-level objects of `file`: the non-relationship `IfcRoot` instances and the relationships,
/// each in file order.
fn objects(file: &StepFile) -> Result<(Vec<Object>, Vec<Object>), String> {
    let schema = file.schema();
    let conv = ToJson { file, schema };
    let (mut plain, mut rels) = (Vec::new(), Vec::new());
    for inst in file.instances().filter(|i| conv.is_root(i)) {
        let object = conv.object(inst, 0)?;
        if schema.is_subtype_of(&inst.entity, "IfcRelationship") {
            rels.push(object);
        } else {
            plain.push(object);
        }
    }
    Ok((plain, rels))
}

struct ToJson<'a> {
    file: &'a StepFile,
    schema: Schema,
}

impl ToJson<'_> {
    fn is_root(&self, inst: &Instance) -> bool {
        self.schema.is_subtype_of(&inst.entity, "IfcRoot")
    }

    /// The schema declaration of `inst`, or an error naming it.
    fn def(&self, inst: &Instance) -> Result<&'static EntityDef, String> {
        self.schema.entity(&inst.entity).ok_or_else(|| {
            let what = if inst.is_complex() { "a complex instance".to_string() } else { inst.entity.clone() };
            format!("#{}: {what} is not an entity of the {} tables", inst.id, self.schema.identifier())
        })
    }

    /// The UUID of an `IfcRoot` instance's GlobalId.
    fn uuid(&self, inst: &Instance, def: &EntityDef) -> Result<Option<String>, String> {
        let Some(k) = def.attribute_index("GlobalId") else { return Ok(None) };
        match inst.params.get(k) {
            None | Some(Param::Unset | Param::Derived) => Ok(None),
            Some(Param::String(s)) => match guid::expand(s) {
                Some(v) => Ok(Some(guid::to_uuid(v))),
                None => Err(format!("#{} {}: malformed GlobalId {s:?}", inst.id, def.name)),
            },
            Some(_) => Err(format!("#{} {}: GlobalId is not a string", inst.id, def.name)),
        }
    }

    /// The object of `inst`: `type`, `globalId`, then its attributes. `depth` counts the nesting.
    fn object(&self, inst: &Instance, depth: usize) -> Result<Object, String> {
        if depth > MAX_DEPTH {
            return Err(format!("#{} {}: nested more than {MAX_DEPTH} levels deep", inst.id, inst.entity));
        }
        let def = self.def(inst)?;
        let mut object = Object::new();
        object.insert("type".into(), def.name.into());
        for (k, attribute) in def.attributes.iter().enumerate() {
            let Some(param) = inst.params.get(k) else { continue };
            if *attribute == "GlobalId" {
                if let Some(uuid) = self.uuid(inst, def)? {
                    object.insert("globalId".into(), uuid.into());
                }
            } else if let Some(value) = self.param(param, depth + 1)? {
                object.insert(lower_camel(attribute), value);
            }
        }
        Ok(object)
    }

    /// The JSON value of a parameter; `None` for what is left out (unset, derived, a dangling
    /// reference), which a list drops.
    fn param(&self, p: &Param, depth: usize) -> Result<Option<Json>, String> {
        if depth > MAX_DEPTH {
            return Err(format!("values nested more than {MAX_DEPTH} levels deep"));
        }
        Ok(Some(match p {
            Param::Unset | Param::Derived => return Ok(None),
            Param::Integer(i) => Json::from(*i),
            Param::Real(x) => Json::Number(Number::from_f64(*x).ok_or_else(|| format!("the real {x} has no JSON form"))?),
            Param::String(s) | Param::Binary(s) => Json::String(s.clone()),
            Param::Enum(e) => match e.as_str() {
                "T" => Json::Bool(true),
                "F" => Json::Bool(false),
                "U" => Json::String("UNKNOWN".into()),
                other => Json::String(other.into()),
            },
            Param::Ref(id) => match self.file.get(*id) {
                None => return Ok(None),
                Some(target) if self.is_root(target) => self.root_ref(target)?,
                Some(target) => Json::Object(self.object(target, depth)?),
            },
            Param::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.extend(self.param(item, depth + 1)?);
                }
                Json::Array(out)
            }
            Param::Typed(name, inner) => {
                let Some(value) = self.param(inner, depth + 1)? else { return Ok(None) };
                let mut typed = Object::new();
                typed.insert("type".into(), defined_type_name(name).into());
                typed.insert("value".into(), value);
                Json::Object(typed)
            }
        }))
    }

    /// `{"type":…,"ref":<uuid>}` for an `IfcRoot` instance.
    fn root_ref(&self, target: &Instance) -> Result<Json, String> {
        let def = self.def(target)?;
        let uuid = self.uuid(target, def)?.ok_or_else(|| format!("#{} {}: no GlobalId", target.id, def.name))?;
        let mut reference = Object::new();
        reference.insert("type".into(), def.name.into());
        reference.insert("ref".into(), uuid.into());
        Ok(Json::Object(reference))
    }
}

/// An attribute name as ifcJSON spells it: the first letter in lower case (`XDim` is `xDim`).
fn lower_camel(attribute: &str) -> String {
    let mut chars = attribute.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// The IFC4 defined types that occur as typed values, in schema spelling.
const DEFINED_TYPES: &[&str] = &[
    "IfcAbsorbedDoseMeasure",
    "IfcAccelerationMeasure",
    "IfcAmountOfSubstanceMeasure",
    "IfcAngularVelocityMeasure",
    "IfcArcIndex",
    "IfcAreaDensityMeasure",
    "IfcAreaMeasure",
    "IfcBinary",
    "IfcBoolean",
    "IfcContextDependentMeasure",
    "IfcCountMeasure",
    "IfcCurvatureMeasure",
    "IfcDate",
    "IfcDateTime",
    "IfcDescriptiveMeasure",
    "IfcDoseEquivalentMeasure",
    "IfcDuration",
    "IfcDynamicViscosityMeasure",
    "IfcElectricCapacitanceMeasure",
    "IfcElectricChargeMeasure",
    "IfcElectricConductanceMeasure",
    "IfcElectricCurrentMeasure",
    "IfcElectricResistanceMeasure",
    "IfcElectricVoltageMeasure",
    "IfcEnergyMeasure",
    "IfcForceMeasure",
    "IfcFrequencyMeasure",
    "IfcGloballyUniqueId",
    "IfcHeatFluxDensityMeasure",
    "IfcHeatingValueMeasure",
    "IfcIdentifier",
    "IfcIlluminanceMeasure",
    "IfcInductanceMeasure",
    "IfcInteger",
    "IfcIntegerCountRateMeasure",
    "IfcIonConcentrationMeasure",
    "IfcIsothermalMoistureCapacityMeasure",
    "IfcKinematicViscosityMeasure",
    "IfcLabel",
    "IfcLengthMeasure",
    "IfcLineIndex",
    "IfcLinearForceMeasure",
    "IfcLinearMomentMeasure",
    "IfcLinearStiffnessMeasure",
    "IfcLinearVelocityMeasure",
    "IfcLogical",
    "IfcLuminousFluxMeasure",
    "IfcLuminousIntensityMeasure",
    "IfcMagneticFluxDensityMeasure",
    "IfcMagneticFluxMeasure",
    "IfcMassDensityMeasure",
    "IfcMassFlowRateMeasure",
    "IfcMassMeasure",
    "IfcMassPerLengthMeasure",
    "IfcModulusOfElasticityMeasure",
    "IfcMoistureDiffusivityMeasure",
    "IfcMolecularWeightMeasure",
    "IfcMomentOfInertiaMeasure",
    "IfcMonetaryMeasure",
    "IfcNonNegativeLengthMeasure",
    "IfcNormalisedRatioMeasure",
    "IfcNumericMeasure",
    "IfcParameterValue",
    "IfcPlanarForceMeasure",
    "IfcPlaneAngleMeasure",
    "IfcPositiveLengthMeasure",
    "IfcPositivePlaneAngleMeasure",
    "IfcPositiveRatioMeasure",
    "IfcPowerMeasure",
    "IfcPressureMeasure",
    "IfcRadioActivityMeasure",
    "IfcRatioMeasure",
    "IfcReal",
    "IfcRotationalFrequencyMeasure",
    "IfcRotationalMassMeasure",
    "IfcSectionModulusMeasure",
    "IfcShearModulusMeasure",
    "IfcSolidAngleMeasure",
    "IfcSoundPowerMeasure",
    "IfcSoundPressureMeasure",
    "IfcSpecificHeatCapacityMeasure",
    "IfcTemperatureGradientMeasure",
    "IfcText",
    "IfcThermalAdmittanceMeasure",
    "IfcThermalConductivityMeasure",
    "IfcThermalExpansionCoefficientMeasure",
    "IfcThermalResistanceMeasure",
    "IfcThermalTransmittanceMeasure",
    "IfcThermodynamicTemperatureMeasure",
    "IfcTime",
    "IfcTimeMeasure",
    "IfcTimeStamp",
    "IfcTorqueMeasure",
    "IfcURIReference",
    "IfcVaporPermeabilityMeasure",
    "IfcVolumeMeasure",
    "IfcVolumetricFlowRateMeasure",
    "IfcWarpingConstantMeasure",
    "IfcWarpingMomentMeasure",
];

/// The schema spelling of a defined type given in upper case (`IFCLABEL` is `IfcLabel`). A name
/// the list lacks is capitalised (`IfcFoobar`): the import reads type names case-insensitively.
fn defined_type_name(upper: &str) -> String {
    if let Some(name) = DEFINED_TYPES.iter().find(|n| n.eq_ignore_ascii_case(upper)) {
        return (*name).to_string();
    }
    let lower = upper.to_ascii_lowercase();
    match lower.strip_prefix("ifc") {
        Some(rest) if !rest.is_empty() => {
            let mut chars = rest.chars();
            let first = chars.next().map(|c| c.to_ascii_uppercase()).unwrap_or_default();
            format!("Ifc{first}{}", chars.as_str())
        }
        _ => lower,
    }
}

// ------------------------------------------------------------------ the document entities

/// `{"kind","name","props"}` of an entity: the props without the name and the provenance props.
fn payload_of(e: &Entity) -> Result<Object, String> {
    let name = bim_model::entity_name(e);
    let at = |what: &str| format!("entity {name}: {what}");
    let mut payload = Object::new();
    payload.insert("kind".into(), e.kind.clone().into());
    if let Some(Value::Text(text)) = e.props.get("_name") {
        payload.insert("name".into(), text.clone().into());
    }
    let mut props = Object::new();
    for (key, value) in &e.props {
        let named = key == "_name" && matches!(value, Value::Text(_));
        if named || PROVENANCE_PROPS.contains(&key.as_str()) {
            continue;
        }
        props.insert(key.clone(), encode_value(value, 0).map_err(|m| at(&format!("prop `{key}`: {m}")))?);
    }
    payload.insert("props".into(), Json::Object(props));
    Ok(payload)
}

/// The JSON encoding of spec/schema.md (number, string, boolean, array, `{"ref"}`), refs as UUIDs.
fn encode_value(value: &Value, depth: usize) -> Result<Json, String> {
    if depth > MAX_DEPTH {
        return Err(format!("lists nested more than {MAX_DEPTH} levels deep"));
    }
    Ok(match value {
        Value::Num(x) => Json::Number(Number::from_f64(*x).ok_or_else(|| format!("the number {x} has no JSON form"))?),
        Value::Text(s) => Json::String(s.clone()),
        Value::Bool(b) => Json::Bool(*b),
        Value::Ref(id) => {
            let mut reference = Object::new();
            reference.insert("ref".into(), guid::to_uuid(id.0).into());
            Json::Object(reference)
        }
        Value::List(items) => Json::Array(items.iter().map(|i| encode_value(i, depth + 1)).collect::<Result<_, _>>()?),
    })
}

/// The `IfcProxy` object that carries an entity the IFC export has no object for.
fn proxy_of(e: &Entity, payload: Json) -> Object {
    let mut proxy = Object::new();
    proxy.insert("type".into(), "IfcProxy".into());
    proxy.insert("globalId".into(), guid::to_uuid(e.id.0).into());
    proxy.insert("name".into(), bim_model::entity_name(e).into());
    proxy.insert("objectType".into(), e.kind.clone().into());
    proxy.insert("proxyType".into(), "NOTDEFINED".into());
    proxy.insert(EXTENSION_KEY.into(), payload);
    proxy
}

// ------------------------------------------------------------------ ifcJSON -> entities

fn bad(message: impl Into<String>) -> ImportError {
    ImportError(message.into())
}

/// Imports ifcJSON text: the document of an `openbimEntity` file, else the entities of the IFC
/// content (module docs). Nothing is imported from a file that is not valid ifcJSON.
pub fn import_ifcjson(text: &str) -> Result<Imported, ImportError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let root: Json = serde_json::from_str(text).map_err(|e| bad(format!("not valid JSON: {e}")))?;
    let data = data_of(&root)?;
    if data.iter().any(|o| o.contains_key(EXTENSION_KEY)) {
        import_entities(&data)
    } else {
        import_ifc(&ifcjson_to_step(&root)?)
    }
}

/// The `data` objects of an ifcJSON document, each with a string `type`.
fn data_of(root: &Json) -> Result<Vec<&Object>, ImportError> {
    let Some(top) = root.as_object() else { return Err(bad("not an ifcJSON file: the top level is not an object")) };
    match top.get("type") {
        Some(Json::String(t)) if t == "ifcJSON" => {}
        Some(other) => return Err(bad(format!("not an ifcJSON file: \"type\" is {other}, not \"ifcJSON\""))),
        None => return Err(bad("not an ifcJSON file: no \"type\"")),
    }
    let Some(Json::Array(items)) = top.get("data") else { return Err(bad("not an ifcJSON file: \"data\" is not a list")) };
    items
        .iter()
        .enumerate()
        .map(|(k, item)| match item.as_object() {
            Some(o) if o.get("type").is_some_and(Json::is_string) => Ok(o),
            Some(_) => Err(bad(format!("data[{k}] has no string \"type\""))),
            None => Err(bad(format!("data[{k}] is not an object"))),
        })
        .collect()
}

/// The GlobalId an object carries, as a value; `None` without one.
fn global_id_of(o: &Object) -> Result<Option<u128>, ImportError> {
    match o.get("globalId") {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) => {
            guid::parse_global_id(s).map(Some).ok_or_else(|| bad(format!("malformed globalId {s:?} (a UUID or a 22-character GlobalId)")))
        }
        Some(other) => Err(bad(format!("globalId {other} is not a string"))),
    }
}

/// What an object is called in a message.
fn label(k: usize, o: &Object) -> String {
    format!("data[{k}] ({})", o.get("type").and_then(Json::as_str).unwrap_or("?"))
}

/// Payload mode: the entities of the `openbimEntity` members, in id order.
fn import_entities(data: &[&Object]) -> Result<Imported, ImportError> {
    let mut entities: BTreeMap<EntityId, ImportedEntity> = BTreeMap::new();
    for (k, o) in data.iter().enumerate() {
        let Some(payload) = o.get(EXTENSION_KEY) else { continue };
        let at = |what: String| bad(format!("{}: {what}", label(k, o)));
        let id = match global_id_of(o).map_err(|e| at(e.0))? {
            Some(v) => EntityId(v),
            None => return Err(at(format!("an {EXTENSION_KEY} member needs a globalId"))),
        };
        let Some(payload) = payload.as_object() else { return Err(at(format!("{EXTENSION_KEY} is not an object"))) };
        let kind = match payload.get("kind") {
            Some(Json::String(kind)) if !kind.is_empty() => kind.clone(),
            _ => return Err(at(format!("{EXTENSION_KEY} needs a \"kind\" (a non-empty string)"))),
        };
        let mut props = match payload.get("props") {
            None => Vec::new(),
            Some(Json::Object(props)) => decode_props(props).map_err(at)?,
            Some(_) => return Err(at(format!("{EXTENSION_KEY} \"props\" is not an object"))),
        };
        // The name the entity is created with: its ULID or seed, when that names the id. Any other
        // name (an entity `bim` did not make) is set after the create.
        let seed = match payload.get("name") {
            None => id.to_ulid_string(),
            Some(Json::String(name)) if EntityId::parse(name) == id => name.clone(),
            Some(Json::String(name)) => {
                props.insert(0, ("_name".to_string(), Value::Text(name.clone())));
                id.to_ulid_string()
            }
            Some(_) => return Err(at(format!("{EXTENSION_KEY} \"name\" is not a string"))),
        };
        if entities.insert(id, ImportedEntity { seed, id, kind, props }).is_some() {
            return Err(at(format!("a second entity with the id {}", guid::to_uuid(id.0))));
        }
    }
    Ok(Imported { entities: entities.into_values().collect(), warnings: Vec::new() })
}

/// The props of a payload. `_name` (it is `name` in the member) and the provenance props are not
/// carried; `_schema` is the current schema version, which a create sets; any other reserved prop
/// (`_…`) is refused, as it is in an ops file.
fn decode_props(props: &Object) -> Result<Vec<(String, Value)>, String> {
    let mut out = Vec::with_capacity(props.len());
    for (key, value) in props {
        if key.starts_with('_') {
            match key.as_str() {
                "_name" | "_created_by" | "_stamp" => continue,
                "_schema" if value.as_f64() == Some(f64::from(Document::SCHEMA_VERSION)) => continue,
                _ => return Err(format!("prop `{key}` is reserved")),
            }
        }
        out.push((key.clone(), decode_value(value).map_err(|m| format!("prop `{key}`: {m}"))?));
    }
    Ok(out)
}

fn decode_value(v: &Json) -> Result<Value, String> {
    Ok(match v {
        Json::Number(n) => Value::Num(n.as_f64().ok_or_else(|| format!("the number {n} is not a finite real"))?),
        Json::String(s) => Value::Text(s.clone()),
        Json::Bool(b) => Value::Bool(*b),
        Json::Array(items) => Value::List(items.iter().map(decode_value).collect::<Result<_, _>>()?),
        Json::Object(o) => match (o.get("ref"), o.len()) {
            (Some(Json::String(target)), 1) => Value::Ref(parse_ref(target)?),
            (Some(Json::String(target)), 2) if o.get("type").is_some_and(Json::is_string) => Value::Ref(parse_ref(target)?),
            _ => return Err("an object value must be {\"ref\": <id>}".into()),
        },
        Json::Null => return Err("null is not a value".into()),
    })
}

/// The entity a payload ref names: a UUID, a 22-character GlobalId or a 26-character ULID.
fn parse_ref(s: &str) -> Result<EntityId, String> {
    guid::parse_global_id(s)
        .map(EntityId)
        .or_else(|| EntityId::parse_ulid(s))
        .ok_or_else(|| format!("ref {s:?} is not a UUID, a GlobalId or a ULID"))
}

// ------------------------------------------------------------------ IFC content -> StepFile

/// The attributes the IFC4 and IFC2X3 tables of [`crate::schema`] declare with an enumeration (or
/// logical) type, by name (compared case-insensitively); ifcJSON writes their values as plain strings.
/// `Name` is one of them only for `IfcSIUnit`. Every other string is a string. `ProxyType` is the
/// enumeration of `IfcProxy`, which the tables lack.
pub const ENUM_ATTRIBUTES: &[&str] = &[
    "PredefinedType",
    "CompositionType",
    "ProfileType",
    "UnitType",
    "Prefix",
    "TargetView",
    "LayerSetDirection",
    "DirectionSense",
    "State",
    "ChangeAction",
    "Operator",
    "OperationType",
    "PartitioningType",
    "ConstructionType",
    "PhysicalOrVirtualBoundary",
    "InternalOrExternalBoundary",
    "RelatedObjectsType",
    "Side",
    "ReflectanceMethod",
    "ShapeType",
    "InteriorOrExteriorSpace",
    "TextureType",
    "ProxyType",
    "IsVentilated",
];

fn is_enum_attribute(schema: Schema, entity: &str, attribute: &str) -> bool {
    ENUM_ATTRIBUTES.iter().any(|a| a.eq_ignore_ascii_case(attribute))
        || (attribute.eq_ignore_ascii_case("Name") && schema.is_subtype_of(entity, "IfcSIUnit"))
}

/// Turns the IFC content of an ifcJSON document into a STEP file: top-level object *i* becomes
/// instance `#i+1`, nested objects get the ids after them, one instance per occurrence (an
/// inline copy of an `IfcRoot` object with a known GlobalId is a reference to it); references
/// become `#id`s of the object with that GlobalId (to an id the file does not define when there is
/// none, as a dangling `#id` would be); keys become attributes by name; JSON booleans become `.T.`
/// and `.F.`, strings of enumeration attributes ([`ENUM_ATTRIBUTES`]) enumerations, integers
/// integers and every other number a real, `{"type","value"}` typed values. The file's
/// `FILE_SCHEMA` is the document's `schemaIdentifier` (IFC4 when absent), so [`import_ifc`]
/// refuses the schemas it refuses for a `.ifc`.
pub fn ifcjson_to_step(root: &Json) -> Result<StepFile, ImportError> {
    let data = data_of(root)?;
    let identifier = match root.get("schemaIdentifier") {
        None | Some(Json::Null) => "IFC4",
        Some(Json::String(s)) => s.as_str(),
        Some(other) => return Err(bad(format!("schemaIdentifier {other} is not a string"))),
    };
    let mut builder = ToStep {
        schema: Schema::from_identifier(identifier).unwrap_or(Schema::Ifc4),
        file: StepFile::with_schema(identifier, "", "", ""),
        by_global: HashMap::new(),
        next_id: data.len() as u64 + 1,
        next_dangling: DANGLING_BASE,
    };
    for (k, o) in data.iter().enumerate() {
        if let Some(g) = global_id_of(o).map_err(|e| bad(format!("{}: {e}", label(k, o))))? {
            if builder.by_global.insert(g, k as u64 + 1).is_some() {
                return Err(bad(format!("{}: the globalId {} is used by two objects", label(k, o), guid::to_uuid(g))));
            }
        }
    }
    for (k, o) in data.iter().enumerate() {
        builder.build(o, k as u64 + 1, 0).map_err(|e| bad(format!("{}: {e}", label(k, o))))?;
    }
    Ok(builder.file)
}

struct ToStep {
    schema: Schema,
    file: StepFile,
    /// The instance of every object with a GlobalId.
    by_global: HashMap<u128, u64>,
    next_id: u64,
    next_dangling: u64,
}

/// The entity and attribute a value belongs to.
#[derive(Clone, Copy)]
struct Slot<'a> {
    entity: &'a str,
    attribute: &'a str,
}

impl ToStep {
    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Adds instance `#id` for `object`.
    fn build(&mut self, object: &Object, id: u64, depth: usize) -> Result<(), ImportError> {
        if depth > MAX_READ_DEPTH {
            return Err(bad(format!("objects nested more than {MAX_READ_DEPTH} levels deep")));
        }
        let Some(entity) = object.get("type").and_then(Json::as_str) else { return Err(bad("an object without a string \"type\"")) };
        let def = self.schema.entity(entity);
        let mut params = vec![Param::Unset; def.map_or(0, |d| d.attributes.len())];
        for (key, value) in object {
            if key == "type" {
                continue;
            }
            // The position of the attribute; a member the entity lacks (an inverse attribute, a
            // vendor extension) is ignored. An entity outside the tables gets its members in order.
            let (position, attribute) = match def {
                Some(def) => match def.attribute_index(key) {
                    Some(k) => (Some(k), def.attributes[k]),
                    None => continue,
                },
                None => (None, key.as_str()),
            };
            let param = if attribute.eq_ignore_ascii_case("GlobalId") {
                match global_id_of(object)? {
                    Some(v) => Param::String(guid::compress(v)),
                    None => continue,
                }
            } else {
                self.convert(value, Slot { entity, attribute }, depth + 1)?
            };
            match position {
                Some(k) => params[k] = param,
                None => params.push(param),
            }
        }
        self.file.insert(Instance::new(id, entity, params)).map_err(|e| bad(e.to_string()))
    }

    fn convert(&mut self, v: &Json, slot: Slot<'_>, depth: usize) -> Result<Param, ImportError> {
        if depth > MAX_READ_DEPTH {
            return Err(bad(format!("values nested more than {MAX_READ_DEPTH} levels deep")));
        }
        Ok(match v {
            Json::Null => Param::Unset,
            Json::Bool(b) => Param::boolean(*b),
            Json::Number(n) => number(n)?,
            Json::String(s) if is_enum_attribute(self.schema, slot.entity, slot.attribute) => enumeration(s, slot.attribute),
            Json::String(s) => Param::String(s.clone()),
            Json::Array(items) => {
                Param::List(items.iter().map(|i| self.convert(i, slot, depth + 1)).collect::<Result<Vec<_>, _>>()?)
            }
            Json::Object(o) => self.convert_object(o, depth)?,
        })
    }

    fn convert_object(&mut self, o: &Object, depth: usize) -> Result<Param, ImportError> {
        // A reference to an IfcRoot object, by GlobalId.
        if let Some(reference) = o.get("ref") {
            let g = reference.as_str().and_then(guid::parse_global_id);
            let g = g.ok_or_else(|| bad(format!("ref {reference} is not a GlobalId (a UUID or 22 characters)")))?;
            return Ok(Param::Ref(match self.by_global.get(&g) {
                Some(&id) => id,
                None => {
                    self.next_dangling += 1;
                    self.next_dangling - 1
                }
            }));
        }
        let Some(entity) = o.get("type").and_then(Json::as_str) else { return Err(bad("an object value without \"type\" or \"ref\"")) };
        // A typed value: {"type": "IfcLabel", "value": …}.
        if o.len() == 2 && o.contains_key("value") && self.schema.entity(entity).is_none() {
            return typed(entity, &o["value"]);
        }
        // An entity nested inline. One with a GlobalId that is known already is that object.
        let global = global_id_of(o)?;
        if let Some(&id) = global.and_then(|g| self.by_global.get(&g)) {
            return Ok(Param::Ref(id));
        }
        let id = self.fresh_id();
        if let Some(g) = global {
            self.by_global.insert(g, id);
        }
        self.build(o, id, depth)?;
        Ok(Param::Ref(id))
    }
}

/// A JSON number as a Part 21 value: an integer that fits `i64` is an integer, every other number
/// a real.
fn number(n: &Number) -> Result<Param, ImportError> {
    match (n.as_i64(), n.as_f64()) {
        (Some(i), _) => Ok(Param::Integer(i)),
        (None, Some(x)) if x.is_finite() => Ok(Param::Real(x)),
        _ => Err(bad(format!("the number {n} is not a finite real"))),
    }
}

/// An enumeration value as the Part 21 reader gives it: upper case. The value `UNKNOWN` of a
/// logical attribute (`IsVentilated`) is `.U.`.
fn enumeration(s: &str, attribute: &str) -> Param {
    match s.to_ascii_uppercase().as_str() {
        "" => Param::Unset,
        "UNKNOWN" if attribute.eq_ignore_ascii_case("IsVentilated") => Param::Enum("U".into()),
        other => Param::Enum(other.into()),
    }
}

/// `{"type": ty, "value": v}`.
fn typed(ty: &str, v: &Json) -> Result<Param, ImportError> {
    fn inner(ty: &str, v: &Json) -> Result<Param, ImportError> {
        Ok(match v {
            Json::Null => Param::Unset,
            Json::Bool(b) => Param::boolean(*b),
            Json::Number(n) => number(n)?,
            Json::String(s) if s == "UNKNOWN" && ty.eq_ignore_ascii_case("IfcLogical") => Param::Enum("U".into()),
            Json::String(s) => Param::String(s.clone()),
            Json::Array(items) => Param::List(items.iter().map(|i| inner(ty, i)).collect::<Result<Vec<_>, _>>()?),
            Json::Object(_) => return Err(bad(format!("the value of a {ty} is an object"))),
        })
    }
    Ok(match inner(ty, v)? {
        Param::Unset => Param::Unset,
        value => Param::typed(ty, value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bim_doc::Op;
    use serde_json::json;

    fn n(x: f64) -> Value {
        Value::Num(x)
    }
    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn nums(xs: &[f64]) -> Value {
        Value::List(xs.iter().map(|&x| Value::Num(x)).collect())
    }
    fn r(name: &str) -> Value {
        Value::Ref(EntityId::parse(name))
    }
    /// A ULID name: the id of an entity created with `bim apply` of a ULID.
    fn ulid(k: u64) -> String {
        EntityId::new_ulid(1_700_000_000_000 + k, u128::from(k) * 0x9e37_79b9_7f4a_7c15 + 1).to_ulid_string()
    }

    type Ent = (String, &'static str, Vec<(&'static str, Value)>);

    /// Pseudo-random finite doubles (every exponent, 17 significant digits when printed).
    fn random_doubles(count: usize) -> Vec<f64> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut out = Vec::new();
        while out.len() < count {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let x = f64::from_bits(state);
            if x.is_finite() {
                out.push(x);
            }
        }
        out
    }

    /// A small building (two levels, joined walls with a door and a window, a slab with a hole, a
    /// column, a beam, a space), entities IFC has no object for (a WallType, a Material, a
    /// SlabOpening, an extension kind), entities named by ULIDs, a wall the IFC export skips (no
    /// level), a dangling ref and awkward numbers.
    fn building() -> Vec<Ent> {
        let s = |x: &str| x.to_string();
        let wall = |level: &str, x2: f64, y2: f64| {
            vec![("level", r(level)), ("type", r("WT")), ("x1", n(0.0)), ("y1", n(0.0)), ("x2", n(x2)), ("y2", n(y2)), ("height", n(3.0))]
        };
        let mut awkward = vec![0.1 + 0.2, 5e-324, 1e300, -0.0, 123_456_789_012_345_680.0, f64::MAX, f64::MIN_POSITIVE, 1.0 / 3.0, 2.0];
        awkward.extend(random_doubles(500));
        vec![
            (s("L0"), "Level", vec![("elevation", n(0.0))]),
            (ulid(1), "Level", vec![("elevation", n(3.0))]),
            (s("WT"), "WallType", vec![("layer_thickness", nums(&[0.1, 0.2])), ("layer_material", Value::List(vec![t("Brick"), t("Block")]))]),
            (s("M"), "Material", vec![("name", t("Steel")), ("E", n(2.1e11)), ("density", n(7850.0))]),
            (s("W1"), "Wall", wall("L0", 5.0, 0.0)),
            (ulid(2), "Wall", {
                let mut p = wall("L0", 5.0, 4.0);
                p[2] = ("x1", n(5.0));
                p
            }),
            (s("D"), "Opening", vec![("host", r("W1")), ("kind", t("door")), ("offset", n(1.0)), ("sill", n(0.0)), ("width", n(0.9)), ("height", n(2.1))]),
            (s("N"), "Opening", vec![("host", r(&ulid(2))), ("kind", t("window")), ("offset", n(1.0)), ("sill", n(1.0)), ("width", n(1.0)), ("height", n(1.0))]),
            (s("S"), "Slab", vec![("level", r(&ulid(1))), ("boundary", nums(&[0.0, 0.0, 5.0, 0.0, 5.0, 4.0, 0.0, 4.0])), ("thickness", n(0.2))]),
            (s("H"), "SlabOpening", vec![("host", r("S")), ("boundary", nums(&[1.0, 1.0, 2.0, 1.0, 2.0, 2.0, 1.0, 2.0]))]),
            (s("C"), "Column", vec![("x", n(2.0)), ("y", n(2.0)), ("base", r("L0")), ("top", r(&ulid(1))), ("section_b", n(0.3)), ("section_h", n(0.3)), ("material", r("M"))]),
            (s("B"), "Beam", vec![("level", r(&ulid(1))), ("x1", n(0.0)), ("y1", n(2.0)), ("x2", n(5.0)), ("y2", n(2.0)), ("section_b", n(0.2)), ("section_h", n(0.4)), ("material", r("M"))]),
            (s("SP"), "Space", vec![("level", r("L0")), ("x", n(2.0)), ("y", n(2.0)), ("name", t("Hall \"A\" ü 😀")), ("use", t(""))]),
            // No level: the IFC export has no wall for it. Also a ref to an entity that is not there.
            (ulid(3), "Wall", vec![("x1", n(0.0)), ("y1", n(0.0)), ("x2", n(1.0)), ("y2", n(0.0)), ("type", r("missing"))]),
            (
                s("X"),
                "fire:Sprinkler",
                vec![
                    ("level", r("L0")),
                    ("flag", Value::Bool(true)),
                    ("off", Value::Bool(false)),
                    ("label", t("")),
                    ("nested", Value::List(vec![Value::List(vec![n(1.0), t("two"), Value::Bool(false)]), Value::List(vec![]), r("W1")])),
                    ("awkward", nums(&awkward)),
                ],
            ),
        ]
    }

    /// The document `bim apply` makes of `entities`: a create sets `_name` and `_schema`.
    fn doc_of(entities: &[Ent]) -> Document {
        let mut doc = Document::new();
        for (name, kind, props) in entities {
            let id = EntityId::parse(name);
            doc.apply(Op::Create { id, kind: kind.to_string() }).unwrap();
            doc.apply(Op::Set { id, key: "_name".into(), value: t(name) }).unwrap();
            doc.apply(Op::Set { id, key: "_schema".into(), value: n(1.0) }).unwrap();
            for (key, value) in props {
                doc.apply(Op::Set { id, key: key.to_string(), value: value.clone() }).unwrap();
            }
        }
        doc
    }

    /// The document `bim import` makes of an import into an empty project.
    fn doc_from(imported: &Imported) -> Document {
        let mut doc = Document::new();
        for e in &imported.entities {
            doc.apply(Op::Create { id: e.id, kind: e.kind.clone() }).unwrap();
            doc.apply(Op::Set { id: e.id, key: "_name".into(), value: Value::Text(e.seed.clone()) }).unwrap();
            doc.apply(Op::Set { id: e.id, key: "_schema".into(), value: n(1.0) }).unwrap();
            for (key, value) in &e.props {
                doc.apply(Op::Set { id: e.id, key: key.clone(), value: value.clone() }).unwrap();
            }
        }
        doc
    }

    fn data(json: &Json) -> &Vec<Json> {
        json["data"].as_array().unwrap()
    }

    #[test]
    fn a_round_trip_through_ifcjson_preserves_the_document() {
        let mut doc = doc_of(&building());
        // Provenance is not part of the content.
        let w1 = EntityId::parse("W1");
        doc.apply(Op::Set { id: w1, key: "_created_by".into(), value: n(7.0) }).unwrap();
        doc.apply(Op::Set { id: w1, key: "_stamp".into(), value: n(42.0) }).unwrap();
        let text = export_ifcjson_string(&doc).unwrap();
        let imported = import_ifcjson(&text).unwrap();
        assert!(imported.warnings.is_empty());
        let back = doc_from(&imported);
        assert_eq!(back.len(), doc.len());
        assert_eq!(back.content_hash(), doc.content_hash());
        for (a, b) in doc.entities().zip(back.entities()) {
            assert_eq!((a.id, &a.kind), (b.id, &b.kind));
            let keep = |e: &Entity| e.props.iter().filter(|(k, _)| !PROVENANCE_PROPS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect::<BTreeMap<_, _>>();
            assert_eq!(keep(a), keep(b), "{}", bim_model::entity_name(a));
        }
        // Numbers come back bit for bit (-0.0 and 5e-324 included).
        let bits = |d: &Document| match d.get(EntityId::parse("X")).unwrap().props.get("awkward") {
            Some(Value::List(l)) => l.iter().map(|v| if let Value::Num(x) = v { x.to_bits() } else { 0 }).collect::<Vec<_>>(),
            _ => panic!("no awkward"),
        };
        assert_eq!(bits(&back), bits(&doc));
        // Names: seeds stay seeds, ULIDs stay ULIDs; a dangling ref stays dangling.
        let seed = |id: EntityId| imported.entities.iter().find(|e| e.id == id).unwrap().seed.clone();
        assert_eq!(seed(EntityId::parse("W1")), "W1");
        assert_eq!(seed(EntityId::parse(&ulid(2))), ulid(2));
        assert_eq!(back.dangling_refs().len(), 1);
        // A second trip gives the same file.
        assert_eq!(export_ifcjson_string(&back).unwrap(), text);
    }

    #[test]
    fn a_name_that_does_not_map_to_the_id_survives_too() {
        // Entities not made by `bim`: ids and names unrelated, an empty name, a name that is an
        // other entity's ULID, and one without `_name` (which the import names after its id).
        let mut doc = Document::new();
        for (k, name) in [(7u128, Some("Seven")), (8, Some("")), (9, Some(ulid(1).as_str())), (10, None)] {
            let id = EntityId(k);
            doc.apply(Op::Create { id, kind: "Level".into() }).unwrap();
            if let Some(name) = name {
                doc.apply(Op::Set { id, key: "_name".into(), value: t(name) }).unwrap();
            }
            doc.apply(Op::Set { id, key: "_schema".into(), value: n(1.0) }).unwrap();
            doc.apply(Op::Set { id, key: "elevation".into(), value: n(k as f64) }).unwrap();
        }
        let back = doc_from(&import_ifcjson(&export_ifcjson_string(&doc).unwrap()).unwrap());
        for (k, expected) in [(7u128, Some("Seven")), (8, Some("")), (9, Some(ulid(1).as_str())), (10, Some(EntityId(10).to_ulid_string().as_str()))] {
            let e = back.get(EntityId(k)).unwrap();
            assert_eq!(e.props.get("_name"), expected.map(t).as_ref(), "{k}");
            assert_eq!(e.props.get("elevation"), Some(&n(k as f64)));
        }
    }

    #[test]
    fn the_export_is_a_function_of_the_content() {
        let a = doc_of(&building());
        let mut reversed = building();
        reversed.reverse();
        let b = doc_of(&reversed);
        assert_eq!(a.content_hash(), b.content_hash());
        assert_ne!(a.log(), b.log());
        assert_eq!(export_ifcjson_string(&a).unwrap(), export_ifcjson_string(&b).unwrap());
    }

    #[test]
    fn the_export_is_ifcjson_with_one_carrier_per_entity() {
        let doc = doc_of(&building());
        let json = export_ifcjson(&doc).unwrap();
        assert_eq!(json["type"], "ifcJSON");
        assert_eq!(json["version"], VERSION);
        assert_eq!(json["schemaIdentifier"], "IFC4");
        let objects = data(&json);
        assert!(objects.iter().all(|o| o["type"].is_string()));
        // Every GlobalId is used once, in UUID form.
        let mut gids: Vec<&str> = objects.iter().filter_map(|o| o["globalId"].as_str()).collect();
        let total = gids.len();
        assert_eq!(total, objects.len());
        gids.sort_unstable();
        gids.dedup();
        assert_eq!(gids.len(), total);
        assert!(gids.iter().all(|g| g.len() == 36 && guid::parse_uuid(g).is_some()));
        // Each document entity is carried once, by the object whose GlobalId is its id.
        let carriers: Vec<&Json> = objects.iter().filter(|o| o.get(EXTENSION_KEY).is_some()).collect();
        assert_eq!(carriers.len(), doc.len());
        for e in doc.entities() {
            let carrier = carriers.iter().find(|o| o["globalId"] == guid::to_uuid(e.id.0).as_str()).expect("a carrier");
            assert_eq!(carrier[EXTENSION_KEY]["kind"], e.kind.as_str());
            // The object the IFC export makes of the entity, else a proxy.
            let expected = match e.kind.as_str() {
                "Level" => "IfcBuildingStorey".to_string(),
                "Opening" => "IfcOpeningElement".to_string(),
                "Wall" | "Slab" | "Column" | "Beam" | "Space" if bim_model::entity_name(e) != ulid(3) => format!("Ifc{}", e.kind),
                _ => "IfcProxy".to_string(),
            };
            assert_eq!(carrier["type"], expected.as_str(), "{}", bim_model::entity_name(e));
        }
        // Counts of the IFC objects other tools look at; the wall without a level, the WallType, the
        // Material, the SlabOpening and the extension entity are proxies.
        let count = |name: &str| objects.iter().filter(|o| o["type"] == name).count();
        assert_eq!((count("IfcBuildingStorey"), count("IfcWall"), count("IfcDoor"), count("IfcWindow")), (2, 2, 1, 1));
        assert_eq!((count("IfcSlab"), count("IfcColumn"), count("IfcBeam"), count("IfcSpace"), count("IfcProxy")), (1, 1, 1, 1, 5));
        // The IFC objects are nested and referenced the ifcJSON way: roots by reference, relationships last.
        let rels = objects.iter().position(|o| o["type"] == "IfcRelAggregates").unwrap();
        assert!(objects[rels..].iter().all(|o| o["type"].as_str().unwrap().starts_with("IfcRel")));
        let voids = objects.iter().find(|o| o["type"] == "IfcRelVoidsElement").unwrap();
        assert_eq!(voids["relatingBuildingElement"]["type"], "IfcWall");
        assert!(voids["relatingBuildingElement"]["ref"].is_string());
        let wall = objects.iter().find(|o| o["type"] == "IfcWall").unwrap();
        assert_eq!(wall["objectPlacement"]["type"], "IfcLocalPlacement");
        assert_eq!(wall["representation"]["representations"][0]["representationType"], "SweptSolid");
    }

    /// Writes every GlobalId and ref in the 22-character form.
    fn respell(v: &mut Json) {
        match v {
            Json::Object(o) => {
                for (key, x) in o.iter_mut() {
                    match x {
                        Json::String(s) if key == "globalId" || key == "ref" => *s = guid::compress(guid::parse_uuid(s).unwrap()),
                        _ => respell(x),
                    }
                }
            }
            Json::Array(items) => items.iter_mut().for_each(respell),
            _ => {}
        }
    }

    #[test]
    fn the_uuid_and_the_22_character_global_id_name_the_same_entity() {
        let doc = doc_of(&building());
        let mut json = export_ifcjson(&doc).unwrap();
        let by_uuid = import_ifcjson(&serde_json::to_string(&json).unwrap()).unwrap();
        respell(&mut json);
        assert!(data(&json).iter().all(|o| o["globalId"].as_str().is_some_and(|g| g.len() == 22)));
        let by_global_id = import_ifcjson(&serde_json::to_string(&json).unwrap()).unwrap();
        assert_eq!(by_uuid, by_global_id);
        assert_eq!(doc_from(&by_global_id).content_hash(), doc.content_hash());
    }

    #[test]
    fn ifc_content_imports_like_the_ifc_it_stands_for() {
        let file = export_ifc4(&doc_of(&building()), "");
        let mut direct = import_ifc(&file).unwrap();
        let json = step_to_ifcjson(&file).unwrap();
        assert!(data(&json).iter().all(|o| o.get(EXTENSION_KEY).is_none()));
        let mut via_json = import_ifcjson(&serde_json::to_string(&json).unwrap()).unwrap();
        assert!(!direct.entities.is_empty());
        // The order of creation (and the instance ids warnings quote) may differ.
        for im in [&mut direct, &mut via_json] {
            im.entities.sort_by_key(|e| e.id);
            im.warnings.clear();
        }
        assert_eq!(direct, via_json);
        // ... also when the GlobalIds are spelled as 22 characters.
        let mut json = json;
        respell(&mut json);
        let mut again = import_ifcjson(&json.to_string()).unwrap();
        again.entities.sort_by_key(|e| e.id);
        assert_eq!(direct.entities, again.entities);
    }

    const MINI: &str = "ISO-10303-21;
HEADER;
FILE_DESCRIPTION((''),'2;1');
FILE_NAME('','',(''),(''),'','','');
FILE_SCHEMA(('IFC4'));
ENDSEC;
DATA;
#1=IFCPROJECT('0qhSLGUAa32dZHUwxM$WqP',$,'P',$,$,$,$,$,#2);
#2=IFCUNITASSIGNMENT((#3,#4));
#3=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);
#4=IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.);
#5=IFCPROPERTYSINGLEVALUE('Flag',$,IFCBOOLEAN(.T.),$);
#6=IFCPROPERTYSINGLEVALUE('Id',$,IFCIDENTIFIER('x'),$);
#7=IFCPROPERTYSINGLEVALUE('Unknown',$,IFCLOGICAL(.U.),$);
#8=IFCPROPERTYSINGLEVALUE('Len',$,IFCLENGTHMEASURE(2.5),$);
#9=IFCPROPERTYSET('1XUNaFVXITdwEa98ZBOV9i',$,'Pset',$,(#5,#6,#7,#8));
#10=IFCRELDEFINESBYPROPERTIES('2TCBgnI8S7ZbdmP757B70B',$,'',$,(#1),#9);
#11=IFCCARTESIANPOINT((0.,1.5,-2.E-5));
#12=IFCSITE('3IZDyUgiC2fO68U9iIXIMa',$,'S',$,$,#13,$,$,.ELEMENT.,$,$,$,$,$);
#13=IFCLOCALPLACEMENT($,#14);
#14=IFCAXIS2PLACEMENT3D(#11,$,$);
#15=IFCMATERIALLAYER($,0.1,.U.,'l',$,$,$);
ENDSEC;
END-ISO-10303-21;
";

    fn mini_json() -> Json {
        json!({
            "type": "ifcJSON", "version": "0.0.1", "schemaIdentifier": "IFC4", "originatingSystem": "openbim bim-ifc",
            "data": [
                {"type": "IfcProject", "globalId": "34adc550-78a9-030a-78d1-7baed6fe0d19", "name": "P",
                 "unitsInContext": {"type": "IfcUnitAssignment", "units": [
                    {"type": "IfcSIUnit", "unitType": "LENGTHUNIT", "prefix": "MILLI", "name": "METRE"},
                    {"type": "IfcSIUnit", "unitType": "AREAUNIT", "name": "SQUARE_METRE"}]}},
                {"type": "IfcSite", "globalId": guid::to_uuid(guid::expand("3IZDyUgiC2fO68U9iIXIMa").unwrap()), "name": "S",
                 "objectPlacement": {"type": "IfcLocalPlacement", "relativePlacement": {"type": "IfcAxis2Placement3D",
                    "location": {"type": "IfcCartesianPoint", "coordinates": [0.0, 1.5, -2e-5]}}},
                 "compositionType": "ELEMENT"},
                {"type": "IfcPropertySet", "globalId": guid::to_uuid(guid::expand("1XUNaFVXITdwEa98ZBOV9i").unwrap()), "name": "Pset",
                 "hasProperties": [
                    {"type": "IfcPropertySingleValue", "name": "Flag", "nominalValue": {"type": "IfcBoolean", "value": true}},
                    {"type": "IfcPropertySingleValue", "name": "Id", "nominalValue": {"type": "IfcIdentifier", "value": "x"}},
                    {"type": "IfcPropertySingleValue", "name": "Unknown", "nominalValue": {"type": "IfcLogical", "value": "UNKNOWN"}},
                    {"type": "IfcPropertySingleValue", "name": "Len", "nominalValue": {"type": "IfcLengthMeasure", "value": 2.5}}]},
                {"type": "IfcRelDefinesByProperties", "globalId": guid::to_uuid(guid::expand("2TCBgnI8S7ZbdmP757B70B").unwrap()), "name": "",
                 "relatedObjects": [{"type": "IfcProject", "ref": "34adc550-78a9-030a-78d1-7baed6fe0d19"}],
                 "relatingPropertyDefinition": {"type": "IfcPropertySet", "ref": guid::to_uuid(guid::expand("1XUNaFVXITdwEa98ZBOV9i").unwrap())}}
            ]
        })
    }

    #[test]
    fn a_step_file_becomes_ifcjson_the_ifcopenshell_way() {
        let json = step_to_ifcjson(&StepFile::parse_str(MINI).unwrap()).unwrap();
        let Json::Object(top) = &json else { panic!() };
        assert_eq!(top.keys().collect::<Vec<_>>(), ["type", "version", "schemaIdentifier", "originatingSystem", "data"]);
        let objects = data(&json);
        // Roots in file order, the relationship last; derived and unset attributes left out, an
        // empty string kept, nested entities inline, roots by reference.
        let types: Vec<&str> = objects.iter().map(|o| o["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["IfcProject", "IfcPropertySet", "IfcSite", "IfcRelDefinesByProperties"]);
        let expected = mini_json();
        let as_text = |o: &Json| serde_json::to_string(o).unwrap();
        for (made, wanted) in [(0, 0), (1, 2), (2, 1), (3, 3)] {
            assert_eq!(as_text(&objects[made]), as_text(&data(&expected)[wanted]));
        }
        // ... and it reads back as the STEP file it came from (the derived `*` is `$` then).
        let back = ifcjson_to_step(&json).unwrap();
        let unit = back.instances_of("IFCSIUNIT").next().unwrap();
        assert_eq!(unit.params, [Param::Unset, Param::Enum("LENGTHUNIT".into()), Param::Enum("MILLI".into()), Param::Enum("METRE".into())]);
        let value = |name: &str| {
            let p = back.instances_of("IFCPROPERTYSINGLEVALUE").find(|p| p.params[0].as_str() == Some(name)).unwrap();
            p.params[2].clone()
        };
        assert_eq!(value("Flag"), Param::typed("IfcBoolean", Param::boolean(true)));
        assert_eq!(value("Id"), Param::typed("IfcIdentifier", Param::string("x")));
        assert_eq!(value("Unknown"), Param::typed("IfcLogical", Param::Enum("U".into())));
        assert_eq!(value("Len"), Param::typed("IfcLengthMeasure", Param::Real(2.5)));
    }

    #[test]
    fn ifcjson_becomes_a_step_file_by_attribute_name() {
        let mut json = mini_json();
        // A 22-character GlobalId in a ref, an integer, an inline copy of a known root, a dangling
        // ref, a logical enumeration and members the entity does not have.
        json["data"][3]["relatedObjects"] = json!([
            {"type": "IfcProject", "ref": "0qhSLGUAa32dZHUwxM$WqP"},
            {"type": "IfcProject", "globalId": "34adc550-78a9-030a-78d1-7baed6fe0d19", "name": "an inline copy"},
            {"type": "IfcSite", "ref": "00000000-0000-0000-0000-000000000001"}
        ]);
        json["data"][3]["isDefinedBy"] = json!([{"type": "IfcRelDefinesByType", "ref": "00000000-0000-0000-0000-000000000002"}]);
        json["data"][1]["refLatitude"] = json!([40, 30, 0]);
        json["data"].as_array_mut().unwrap().push(json!({"type": "IfcMaterialLayer", "layerThickness": 0.1, "isVentilated": "UNKNOWN", "name": "l"}));
        let file = ifcjson_to_step(&json).unwrap();
        assert_eq!(file.schema_identifiers(), ["IFC4"]);
        // Top-level object i is instance #i+1; nested instances come after them.
        let project = file.get(1).unwrap();
        assert_eq!((project.entity.as_str(), &project.params[0], &project.params[2]), ("IFCPROJECT", &Param::string("0qhSLGUAa32dZHUwxM$WqP"), &Param::string("P")));
        assert_eq!(file.get(2).unwrap().entity, "IFCSITE");
        assert_eq!(file.get(2).unwrap().params[9], Param::List(vec![Param::Integer(40), Param::Integer(30), Param::Integer(0)]));
        let rel = file.get(4).unwrap();
        assert_eq!(rel.entity, "IFCRELDEFINESBYPROPERTIES");
        let related = rel.params[4].as_list().unwrap();
        assert_eq!(related[0], Param::Ref(1));
        assert_eq!(related[1], Param::Ref(1), "an inline copy of a known root is that root");
        assert!(matches!(related[2], Param::Ref(id) if id >= DANGLING_BASE && file.get(id).is_none()));
        assert_eq!(rel.params[5], Param::Ref(3));
        assert_eq!(rel.params.len(), 6, "no inverse attributes");
        let layer = file.instances_of("IFCMATERIALLAYER").next().unwrap();
        assert_eq!(layer.params[0..4], [Param::Unset, Param::Real(0.1), Param::Enum("U".into()), Param::string("l")]);
        // An integer is an integer, a real a real.
        let point = file.instances_of("IFCCARTESIANPOINT").next().unwrap();
        assert_eq!(point.params[0], Param::List(vec![Param::Real(0.0), Param::Real(1.5), Param::Real(-2e-5)]));
        // The file imports (nothing to build from it, but it is a valid IFC file for the importer).
        assert!(import_ifc(&file).unwrap().entities.is_empty());
    }

    #[test]
    fn a_string_is_an_enumeration_only_where_the_schema_says_so() {
        let json = json!({"type": "ifcJSON", "data": [
            {"type": "IfcSIUnit", "unitType": "LENGTHUNIT", "prefix": "milli", "name": "METRE"},
            {"type": "IfcConversionBasedUnit", "unitType": "LENGTHUNIT", "name": "FOOT"},
            {"type": "IfcBuildingStorey", "globalId": "34adc550-78a9-030a-78d1-7baed6fe0d19", "name": "METRE", "longName": "MODEL_VIEW",
             "compositionType": "ELEMENT"},
            {"type": "IfcFooBar", "globalId": "00000000-0000-0000-0000-000000000003", "name": "x", "predefinedType": "STANDARD", "n": 1}
        ]});
        let file = ifcjson_to_step(&json).unwrap();
        assert_eq!(file.get(1).unwrap().params, [Param::Unset, Param::Enum("LENGTHUNIT".into()), Param::Enum("MILLI".into()), Param::Enum("METRE".into())]);
        assert_eq!(file.get(2).unwrap().params[1..3], [Param::Enum("LENGTHUNIT".into()), Param::string("FOOT")]);
        let storey = &file.get(3).unwrap().params;
        assert_eq!(storey[2], Param::string("METRE"));
        assert_eq!(storey[7], Param::string("MODEL_VIEW"));
        assert_eq!(storey[8], Param::Enum("ELEMENT".into()));
        // Outside the tables the members are kept in order.
        let foo = file.get(4).unwrap();
        assert_eq!(foo.entity, "IFCFOOBAR");
        assert_eq!(foo.params, [Param::string(guid::compress(3)), Param::string("x"), Param::Enum("STANDARD".into()), Param::Integer(1)]);
    }

    #[test]
    fn ifc_content_keeps_its_units() {
        // The IFC export of the model with the length unit changed to millimetres: the importer
        // reads other lengths from it, and so does the import of its ifcJSON.
        let mut file = export_ifc4(&doc_of(&building()), "");
        let metres = import_ifc(&file).unwrap();
        let unit = file.instances().find(|i| i.entity == "IFCSIUNIT" && i.params.get(1) == Some(&Param::Enum("LENGTHUNIT".into()))).unwrap().id;
        file.set_attr(unit, "Prefix", Param::Enum("MILLI".into())).unwrap();
        let mut direct = import_ifc(&file).unwrap();
        let mut via_json = import_ifcjson(&step_to_ifcjson(&file).unwrap().to_string()).unwrap();
        for im in [&mut direct, &mut via_json] {
            im.entities.sort_by_key(|e| e.id);
            im.warnings.clear();
        }
        assert_ne!(direct.entities, { let mut m = metres; m.entities.sort_by_key(|e| e.id); m.entities }, "the unit changes the import");
        assert_eq!(direct, via_json);
    }

    #[test]
    fn malformed_input_is_an_error() {
        let doc = doc_of(&building());
        let text = export_ifcjson_string(&doc).unwrap();
        // Every truncation is invalid.
        for cut in [0, 1, 10, text.len() / 3, text.len() / 2, text.len() - 3] {
            let e = import_ifcjson(&text[..cut]).unwrap_err();
            assert!(e.0.starts_with("not valid JSON"), "{cut}: {e}");
        }
        let rejected = |text: &str, why: &str| {
            let e = import_ifcjson(text).expect_err(text).0;
            assert!(e.contains(why), "{text}: {e}");
        };
        rejected("", "not valid JSON");
        rejected("[]", "not an ifcJSON file");
        rejected("{\"schema\":1,\"ops\":[]}", "not an ifcJSON file");
        rejected("{\"type\":\"ifc\",\"data\":[]}", "not an ifcJSON file");
        rejected("{\"type\":\"ifcJSON\"}", "\"data\" is not a list");
        rejected("{\"type\":\"ifcJSON\",\"data\":{}}", "\"data\" is not a list");
        rejected("{\"type\":\"ifcJSON\",\"data\":[1]}", "data[0] is not an object");
        rejected("{\"type\":\"ifcJSON\",\"data\":[{}]}", "data[0] has no string \"type\"");
        rejected("{\"type\":\"ifcJSON\",\"data\":[{\"type\":\"IfcWall\",\"globalId\":\"not-a-guid\"}]}", "malformed globalId");
        rejected("{\"type\":\"ifcJSON\",\"data\":[{\"type\":\"IfcWall\",\"globalId\":5}]}", "globalId 5 is not a string");
        rejected("{\"type\":\"ifcJSON\",\"schemaIdentifier\":\"IFC4X3\",\"data\":[]}", "unsupported FILE_SCHEMA");
        rejected("{\"type\":\"ifcJSON\",\"schemaIdentifier\":4,\"data\":[]}", "schemaIdentifier");
        rejected("{\"type\":\"ifcJSON\",\"data\":[{\"type\":\"IfcProject\",\"unitsInContext\":{\"ref\":7}}]}", "not a GlobalId");
        rejected("{\"type\":\"ifcJSON\",\"data\":[{\"type\":\"IfcProject\",\"name\":{\"a\":1}}]}", "without \"type\" or \"ref\"");
        let g = "34adc550-78a9-030a-78d1-7baed6fe0d19";
        let twice = format!("{{\"type\":\"ifcJSON\",\"data\":[{{\"type\":\"IfcWall\",\"globalId\":\"{g}\"}},{{\"type\":\"IfcSlab\",\"globalId\":\"{}\"}}]}}", guid::compress(guid::parse_uuid(g).unwrap()));
        rejected(&twice, "used by two objects");
        // Nesting deeper than the JSON reader allows.
        let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
        rejected(&format!("{{\"type\":\"ifcJSON\",\"data\":[{{\"type\":\"IfcWall\",\"x\":{deep}}}]}}"), "not valid JSON");
        // The empty document and a byte order mark are fine.
        assert!(import_ifcjson("{\"type\":\"ifcJSON\",\"data\":[]}").unwrap().entities.is_empty());
        assert!(import_ifcjson(&format!("\u{feff}{text}")).is_ok());
    }

    #[test]
    fn a_malformed_entity_member_is_an_error() {
        let carrier = |payload: Json| {
            json!({"type": "ifcJSON", "data": [{"type": "IfcWall", "globalId": "34adc550-78a9-030a-78d1-7baed6fe0d19", EXTENSION_KEY: payload}]}).to_string()
        };
        let ok = import_ifcjson(&carrier(json!({"kind": "Level", "name": "L", "props": {"elevation": 1.5, "up": {"ref": "00000000-0000-0000-0000-000000000009"}}}))).unwrap();
        assert_eq!(ok.entities.len(), 1);
        let e = &ok.entities[0];
        assert_eq!((e.id, e.kind.as_str()), (EntityId(guid::parse_uuid("34adc550-78a9-030a-78d1-7baed6fe0d19").unwrap()), "Level"));
        // "L" does not map to the id: the entity is created under the id's ULID and named after.
        assert_eq!(e.seed, e.id.to_ulid_string());
        assert_eq!(
            e.props,
            [("_name".to_string(), t("L")), ("elevation".to_string(), n(1.5)), ("up".to_string(), Value::Ref(EntityId(9)))]
        );
        for (payload, why) in [
            (json!(5), "is not an object"),
            (json!({}), "needs a \"kind\""),
            (json!({"kind": ""}), "needs a \"kind\""),
            (json!({"kind": "Level", "name": 5}), "\"name\" is not a string"),
            (json!({"kind": "Level", "props": []}), "\"props\" is not an object"),
            (json!({"kind": "Level", "props": {"a": null}}), "prop `a`: null is not a value"),
            (json!({"kind": "Level", "props": {"a": {"x": 1}}}), "prop `a`: an object value must be"),
            (json!({"kind": "Level", "props": {"a": {"ref": "nope"}}}), "is not a UUID, a GlobalId or a ULID"),
            (json!({"kind": "Level", "props": {"a": [1, {"ref": 3}]}}), "an object value must be"),
            (json!({"kind": "Level", "props": {"_stamp": 1, "_x": 1}}), "prop `_x` is reserved"),
            (json!({"kind": "Level", "props": {"_schema": 2}}), "prop `_schema` is reserved"),
        ] {
            let e = import_ifcjson(&carrier(payload.clone())).expect_err(&payload.to_string()).0;
            assert!(e.contains(why), "{payload}: {e}");
        }
        // Reserved props the import itself sets are accepted and not carried.
        let ok = import_ifcjson(&carrier(json!({"kind": "Level", "props": {"_name": "x", "_schema": 1, "_stamp": 3, "_created_by": 4}}))).unwrap();
        assert!(ok.entities[0].props.is_empty());
        // A member needs a GlobalId, and two members cannot share one.
        let no_id = json!({"type": "ifcJSON", "data": [{"type": "IfcProxy", EXTENSION_KEY: {"kind": "Level"}}]}).to_string();
        assert!(import_ifcjson(&no_id).unwrap_err().0.contains("needs a globalId"));
        let g = "34adc550-78a9-030a-78d1-7baed6fe0d19";
        let two = json!({"type": "ifcJSON", "data": [
            {"type": "IfcProxy", "globalId": g, EXTENSION_KEY: {"kind": "Level"}},
            {"type": "IfcProxy", "globalId": guid::compress(guid::parse_uuid(g).unwrap()), EXTENSION_KEY: {"kind": "Level"}}]})
        .to_string();
        assert!(import_ifcjson(&two).unwrap_err().0.contains("a second entity"));
    }

    #[test]
    fn objects_without_the_member_do_not_matter_in_a_file_that_has_it() {
        let doc = doc_of(&building());
        let mut json = export_ifcjson(&doc).unwrap();
        let expected = import_ifcjson(&json.to_string()).unwrap();
        // Edit the IFC content (and add objects that are not valid IFC): the members rule.
        let objects = json["data"].as_array_mut().unwrap();
        objects.push(json!({"type": "IfcWall", "globalId": "00000000-0000-0000-0000-0000000000aa", "name": "stray"}));
        for o in objects.iter_mut() {
            if o["type"] == "IfcWall" {
                o["name"] = json!("renamed");
            }
        }
        assert_eq!(import_ifcjson(&json.to_string()).unwrap(), expected);
    }

    #[test]
    fn values_without_a_json_form_are_an_error() {
        let mut doc = doc_of(&building());
        let w1 = EntityId::parse("W1");
        doc.apply(Op::Set { id: w1, key: "bad".into(), value: Value::List(vec![n(1.0), n(f64::NAN)]) }).unwrap();
        assert!(export_ifcjson_string(&doc).unwrap_err().contains("no JSON form"));
        // A list nested deeper than the JSON reader accepts cannot be written.
        let mut doc = doc_of(&building());
        let mut deep = n(1.0);
        for _ in 0..150 {
            deep = Value::List(vec![deep]);
        }
        doc.apply(Op::Set { id: w1, key: "deep".into(), value: deep }).unwrap();
        assert!(export_ifcjson_string(&doc).unwrap_err().contains("nested"));
        // ... but a deep one that is allowed is read back.
        let mut deep = n(1.0);
        for _ in 0..90 {
            deep = Value::List(vec![deep]);
        }
        let mut doc = doc_of(&building());
        doc.apply(Op::Set { id: w1, key: "deep".into(), value: deep }).unwrap();
        let back = doc_from(&import_ifcjson(&export_ifcjson_string(&doc).unwrap()).unwrap());
        assert_eq!(back.content_hash(), doc.content_hash());
    }

    #[test]
    fn the_empty_document_exports_and_imports() {
        let text = export_ifcjson_string(&Document::new()).unwrap();
        assert!(text.ends_with("}\n"));
        let imported = import_ifcjson(&text).unwrap();
        assert!(imported.entities.is_empty());
    }

    #[test]
    fn a_step_file_with_a_complex_instance_is_not_converted() {
        let f = StepFile::parse_str(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('','',(''),(''),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1=IFCPROJECT('0qhSLGUAa32dZHUwxM$WqP',$,$,$,$,$,$,(#2),$);\n#2=(IFCA(1)IFCB(2));\nENDSEC;\nEND-ISO-10303-21;\n",
        )
        .unwrap();
        let e = step_to_ifcjson(&f).unwrap_err();
        assert!(e.contains("complex instance"), "{e}");
        let f = StepFile::parse_str(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('','',(''),(''),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1=IFCPROJECT('short',$,$,$,$,$,$,$,$);\nENDSEC;\nEND-ISO-10303-21;\n",
        )
        .unwrap();
        assert!(step_to_ifcjson(&f).unwrap_err().contains("malformed GlobalId"));
    }

    #[test]
    fn defined_types_are_spelled_as_the_schema_does() {
        assert_eq!(defined_type_name("IFCLABEL"), "IfcLabel");
        assert_eq!(defined_type_name("IFCPOSITIVELENGTHMEASURE"), "IfcPositiveLengthMeasure");
        assert_eq!(defined_type_name("IFCTHERMALTRANSMITTANCEMEASURE"), "IfcThermalTransmittanceMeasure");
        assert_eq!(defined_type_name("IFCSOMETHINGNEW"), "IfcSomethingnew");
        for name in DEFINED_TYPES {
            assert_eq!(defined_type_name(&name.to_ascii_uppercase()), *name);
        }
        assert_eq!(lower_camel("XDim"), "xDim");
        assert_eq!(lower_camel("GlobalId"), "globalId");
    }
}
