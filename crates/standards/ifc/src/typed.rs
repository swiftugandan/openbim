//! Typed, by-attribute-name access to IFC instances and schema-driven instance construction.

use crate::error::{ErrorKind, StepError};
use crate::model::{Instance, Param, StepFile};
use crate::schema::{EntityDef, Schema};

/// A view of one instance together with its schema declaration.
#[derive(Debug, Clone, Copy)]
pub struct Entity<'a> {
    pub file: &'a StepFile,
    pub instance: &'a Instance,
    /// The declaration in the file's schema (None for entities outside the tables).
    pub def: Option<&'static EntityDef>,
}

impl<'a> Entity<'a> {
    pub fn id(&self) -> u64 {
        self.instance.id
    }

    /// The upper-case entity name as written in the file.
    pub fn entity(&self) -> &'a str {
        &self.instance.entity
    }

    /// The schema spelling of the entity name (e.g. `IfcWall`), or the file's spelling.
    pub fn type_name(&self) -> &'a str {
        self.def.map(|d| d.name).unwrap_or(&self.instance.entity)
    }

    /// Is this instance of `ancestor` or one of its subtypes?
    pub fn is_a(&self, ancestor: &str) -> bool {
        self.instance.is(ancestor) || self.file.schema().is_subtype_of(&self.instance.entity, ancestor)
    }

    /// The parameter at a position.
    pub fn param(&self, index: usize) -> Option<&'a Param> {
        self.instance.params.get(index)
    }

    /// The value of a named attribute; `None` when the entity or attribute is unknown, and also
    /// when the value is `$` or `*`.
    pub fn get(&self, attribute: &str) -> Option<&'a Param> {
        let k = self.def?.attribute_index(attribute)?;
        match self.instance.params.get(k)? {
            Param::Unset | Param::Derived => None,
            p => Some(p),
        }
    }

    pub fn get_str(&self, attribute: &str) -> Option<&'a str> {
        self.get(attribute)?.as_str()
    }
    pub fn get_f64(&self, attribute: &str) -> Option<f64> {
        self.get(attribute)?.as_f64()
    }
    pub fn get_i64(&self, attribute: &str) -> Option<i64> {
        self.get(attribute)?.as_i64()
    }
    pub fn get_enum(&self, attribute: &str) -> Option<&'a str> {
        self.get(attribute)?.as_enum()
    }
    pub fn get_bool(&self, attribute: &str) -> Option<bool> {
        self.get(attribute)?.as_bool()
    }
    pub fn get_list(&self, attribute: &str) -> Option<&'a [Param]> {
        self.get(attribute)?.as_list()
    }
    pub fn get_ref(&self, attribute: &str) -> Option<u64> {
        self.get(attribute)?.as_ref_id()
    }
    /// The numbers of a list attribute (e.g. `IfcCartesianPoint.Coordinates`).
    pub fn get_f64s(&self, attribute: &str) -> Vec<f64> {
        self.get(attribute).map(Param::f64s).unwrap_or_default()
    }
    /// The referenced ids of a list attribute (e.g. `IfcRelAggregates.RelatedObjects`).
    pub fn get_refs(&self, attribute: &str) -> Vec<u64> {
        self.get(attribute).map(Param::ref_ids).unwrap_or_default()
    }
    /// The entity referenced by an attribute.
    pub fn get_entity(&self, attribute: &str) -> Option<Entity<'a>> {
        self.file.entity(self.get_ref(attribute)?)
    }
    /// The entities referenced by a list attribute (dangling references skipped).
    pub fn get_entities(&self, attribute: &str) -> Vec<Entity<'a>> {
        self.get_refs(attribute).into_iter().filter_map(|id| self.file.entity(id)).collect()
    }
}

impl StepFile {
    /// A typed view of the instance with this id.
    pub fn entity(&self, id: u64) -> Option<Entity<'_>> {
        let instance = self.get(id)?;
        let def = self.schema().entity(&instance.entity);
        Some(Entity { file: self, instance, def })
    }

    /// Typed views of every instance of `ancestor` or one of its subtypes (per the file's
    /// schema), in file order.
    pub fn entities_of<'a>(&'a self, ancestor: &'a str) -> impl Iterator<Item = Entity<'a>> + 'a {
        let schema = self.schema();
        self.instances().filter_map(move |inst| {
            if inst.is(ancestor) || schema.is_subtype_of(&inst.entity, ancestor) {
                Some(Entity { file: self, instance: inst, def: schema.entity(&inst.entity) })
            } else {
                None
            }
        })
    }

    /// The value of a named attribute of an instance (see [`Entity::get`]).
    pub fn attr(&self, id: u64, attribute: &str) -> Option<&Param> {
        let inst = self.get(id)?;
        let k = self.schema().attribute_index(&inst.entity, attribute)?;
        match inst.params.get(k)? {
            Param::Unset | Param::Derived => None,
            p => Some(p),
        }
    }

    /// Builds the parameter list of `entity` in `schema` from named attribute values; attributes
    /// not given are `$`. Unknown entities or attribute names are errors.
    pub fn build_params(schema: Schema, entity: &str, attributes: &[(&str, Param)]) -> Result<Vec<Param>, StepError> {
        let def = schema.entity(entity).ok_or_else(|| {
            StepError::new(ErrorKind::Schema, 0, format!("{entity} is not in the {} table", schema.identifier()))
        })?;
        let mut params = vec![Param::Unset; def.attributes.len()];
        for (name, value) in attributes {
            let k = def.attribute_index(name).ok_or_else(|| {
                StepError::new(ErrorKind::Schema, 0, format!("{} has no attribute {name}", def.name))
            })?;
            params[k] = value.clone();
        }
        Ok(params)
    }

    /// Adds an instance of `entity` (in the file's schema) from named attribute values, with the
    /// next free id; attributes not given are `$`.
    pub fn add_entity(&mut self, entity: &str, attributes: &[(&str, Param)]) -> Result<u64, StepError> {
        let params = StepFile::build_params(self.schema(), entity, attributes)?;
        Ok(self.add(entity, params))
    }

    /// Sets a named attribute of an existing instance.
    pub fn set_attr(&mut self, id: u64, attribute: &str, value: Param) -> Result<(), StepError> {
        let schema = self.schema();
        let inst = self
            .get_mut(id)
            .ok_or_else(|| StepError::new(ErrorKind::Schema, 0, format!("no instance #{id}")))?;
        let k = schema.attribute_index(&inst.entity, attribute).ok_or_else(|| {
            StepError::new(ErrorKind::Schema, 0, format!("{} has no attribute {attribute}", inst.entity))
        })?;
        if inst.params.len() <= k {
            inst.params.resize(k + 1, Param::Unset);
        }
        inst.params[k] = value;
        Ok(())
    }
}
