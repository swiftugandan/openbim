//! IFC schema tables: for each supported entity its supertype and all attribute names in order
//! (inherited attributes first), generated from the IFC4 and IFC2X3 EXPRESS schemas.

use crate::model::StepFile;
use std::collections::HashMap;
use std::sync::OnceLock;

/// One entity declaration of an IFC schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityDef {
    /// The entity name in schema spelling (e.g. `IfcWall`).
    pub name: &'static str,
    pub supertype: Option<&'static str>,
    pub is_abstract: bool,
    /// All explicit attributes in Part 21 order, inherited ones first.
    pub attributes: &'static [&'static str],
}

impl EntityDef {
    /// The position of an attribute (case-insensitive).
    pub fn attribute_index(&self, name: &str) -> Option<usize> {
        self.attributes.iter().position(|a| a.eq_ignore_ascii_case(name))
    }
}

/// A supported IFC schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Schema {
    Ifc4,
    Ifc2x3,
}

impl Schema {
    /// The schema for a `FILE_SCHEMA` identifier (`IFC4`, `IFC4X…` read as IFC4; `IFC2X3`).
    pub fn from_identifier(id: &str) -> Option<Schema> {
        let id = id.trim().to_ascii_uppercase();
        if id.starts_with("IFC2X3") {
            Some(Schema::Ifc2x3)
        } else if id.starts_with("IFC4") {
            Some(Schema::Ifc4)
        } else {
            None
        }
    }

    /// The `FILE_SCHEMA` identifier.
    pub fn identifier(self) -> &'static str {
        match self {
            Schema::Ifc4 => "IFC4",
            Schema::Ifc2x3 => "IFC2X3",
        }
    }

    /// All entity declarations of the table.
    pub fn entities(self) -> &'static [EntityDef] {
        match self {
            Schema::Ifc4 => IFC4,
            Schema::Ifc2x3 => IFC2X3,
        }
    }

    fn lookup(self) -> &'static HashMap<String, &'static EntityDef> {
        static L4: OnceLock<HashMap<String, &'static EntityDef>> = OnceLock::new();
        static L2: OnceLock<HashMap<String, &'static EntityDef>> = OnceLock::new();
        let cell = match self {
            Schema::Ifc4 => &L4,
            Schema::Ifc2x3 => &L2,
        };
        cell.get_or_init(|| self.entities().iter().map(|e| (e.name.to_ascii_uppercase(), e)).collect())
    }

    /// The declaration of an entity (case-insensitive name).
    pub fn entity(self, name: &str) -> Option<&'static EntityDef> {
        self.lookup().get(&name.to_ascii_uppercase()).copied()
    }

    /// Is `entity` the same as or a subtype of `ancestor` (case-insensitive; false when `entity`
    /// is not in the table)?
    pub fn is_subtype_of(self, entity: &str, ancestor: &str) -> bool {
        let mut cur = self.entity(entity);
        while let Some(e) = cur {
            if e.name.eq_ignore_ascii_case(ancestor) {
                return true;
            }
            cur = e.supertype.and_then(|s| self.entity(s));
        }
        false
    }

    /// The position of attribute `attribute` of `entity`.
    pub fn attribute_index(self, entity: &str, attribute: &str) -> Option<usize> {
        self.entity(entity)?.attribute_index(attribute)
    }
}

impl StepFile {
    /// The IFC schema named by `FILE_SCHEMA` (IFC4 when absent or unknown).
    pub fn schema(&self) -> Schema {
        self.schema_identifiers().iter().find_map(|s| Schema::from_identifier(s)).unwrap_or(Schema::Ifc4)
    }
}

pub(crate) static IFC4: &[EntityDef] = &[
    EntityDef { name: "IfcApplication", supertype: None, is_abstract: false, attributes: &["ApplicationDeveloper", "Version", "ApplicationFullName", "ApplicationIdentifier"] },
    EntityDef { name: "IfcArbitraryClosedProfileDef", supertype: Some("IfcProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "OuterCurve"] },
    EntityDef { name: "IfcArbitraryProfileDefWithVoids", supertype: Some("IfcArbitraryClosedProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "OuterCurve", "InnerCurves"] },
    EntityDef { name: "IfcAxis2Placement2D", supertype: Some("IfcPlacement"), is_abstract: false, attributes: &["Location", "RefDirection"] },
    EntityDef { name: "IfcAxis2Placement3D", supertype: Some("IfcPlacement"), is_abstract: false, attributes: &["Location", "Axis", "RefDirection"] },
    EntityDef { name: "IfcBeam", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcBeamType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcBlobTexture", supertype: Some("IfcSurfaceTexture"), is_abstract: false, attributes: &["RepeatS", "RepeatT", "Mode", "TextureTransform", "Parameter", "RasterFormat", "RasterCode"] },
    EntityDef { name: "IfcBooleanClippingResult", supertype: Some("IfcBooleanResult"), is_abstract: false, attributes: &["Operator", "FirstOperand", "SecondOperand"] },
    EntityDef { name: "IfcBooleanResult", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: false, attributes: &["Operator", "FirstOperand", "SecondOperand"] },
    EntityDef { name: "IfcBoundedCurve", supertype: Some("IfcCurve"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcBuilding", supertype: Some("IfcSpatialStructureElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType", "ElevationOfRefHeight", "ElevationOfTerrain", "BuildingAddress"] },
    EntityDef { name: "IfcBuildingElement", supertype: Some("IfcElement"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcBuildingElementProxy", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcBuildingElementProxyType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcBuildingElementType", supertype: Some("IfcElementType"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType"] },
    EntityDef { name: "IfcBuildingStorey", supertype: Some("IfcSpatialStructureElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType", "Elevation"] },
    EntityDef { name: "IfcCartesianPoint", supertype: Some("IfcPoint"), is_abstract: false, attributes: &["Coordinates"] },
    EntityDef { name: "IfcCartesianPointList", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcCartesianPointList2D", supertype: Some("IfcCartesianPointList"), is_abstract: false, attributes: &["CoordList"] },
    EntityDef { name: "IfcCartesianPointList3D", supertype: Some("IfcCartesianPointList"), is_abstract: false, attributes: &["CoordList"] },
    EntityDef { name: "IfcCartesianTransformationOperator", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &["Axis1", "Axis2", "LocalOrigin", "Scale"] },
    EntityDef { name: "IfcCartesianTransformationOperator3D", supertype: Some("IfcCartesianTransformationOperator"), is_abstract: false, attributes: &["Axis1", "Axis2", "LocalOrigin", "Scale", "Axis3"] },
    EntityDef { name: "IfcCircleProfileDef", supertype: Some("IfcParameterizedProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "Position", "Radius"] },
    EntityDef { name: "IfcClosedShell", supertype: Some("IfcConnectedFaceSet"), is_abstract: false, attributes: &["CfsFaces"] },
    EntityDef { name: "IfcColourRgb", supertype: Some("IfcColourSpecification"), is_abstract: false, attributes: &["Name", "Red", "Green", "Blue"] },
    EntityDef { name: "IfcColourSpecification", supertype: Some("IfcPresentationItem"), is_abstract: true, attributes: &["Name"] },
    EntityDef { name: "IfcColumn", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcColumnType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcConnectedFaceSet", supertype: Some("IfcTopologicalRepresentationItem"), is_abstract: false, attributes: &["CfsFaces"] },
    EntityDef { name: "IfcContext", supertype: Some("IfcObjectDefinition"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "LongName", "Phase", "RepresentationContexts", "UnitsInContext"] },
    EntityDef { name: "IfcConversionBasedUnit", supertype: Some("IfcNamedUnit"), is_abstract: false, attributes: &["Dimensions", "UnitType", "Name", "ConversionFactor"] },
    EntityDef { name: "IfcCovering", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcCurtainWall", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcCurve", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcDimensionalExponents", supertype: None, is_abstract: false, attributes: &["LengthExponent", "MassExponent", "TimeExponent", "ElectricCurrentExponent", "ThermodynamicTemperatureExponent", "AmountOfSubstanceExponent", "LuminousIntensityExponent"] },
    EntityDef { name: "IfcDirection", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: false, attributes: &["DirectionRatios"] },
    EntityDef { name: "IfcDoor", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "OverallHeight", "OverallWidth", "PredefinedType", "OperationType", "UserDefinedOperationType"] },
    EntityDef { name: "IfcDoorType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType", "OperationType", "ParameterTakesPrecedence", "UserDefinedOperationType"] },
    EntityDef { name: "IfcElement", supertype: Some("IfcProduct"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcElementarySurface", supertype: Some("IfcSurface"), is_abstract: true, attributes: &["Position"] },
    EntityDef { name: "IfcElementQuantity", supertype: Some("IfcQuantitySet"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "MethodOfMeasurement", "Quantities"] },
    EntityDef { name: "IfcElementType", supertype: Some("IfcTypeProduct"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType"] },
    EntityDef { name: "IfcExtrudedAreaSolid", supertype: Some("IfcSweptAreaSolid"), is_abstract: false, attributes: &["SweptArea", "Position", "ExtrudedDirection", "Depth"] },
    EntityDef { name: "IfcFace", supertype: Some("IfcTopologicalRepresentationItem"), is_abstract: false, attributes: &["Bounds"] },
    EntityDef { name: "IfcFaceBound", supertype: Some("IfcTopologicalRepresentationItem"), is_abstract: false, attributes: &["Bound", "Orientation"] },
    EntityDef { name: "IfcFaceOuterBound", supertype: Some("IfcFaceBound"), is_abstract: false, attributes: &["Bound", "Orientation"] },
    EntityDef { name: "IfcFacetedBrep", supertype: Some("IfcManifoldSolidBrep"), is_abstract: false, attributes: &["Outer"] },
    EntityDef { name: "IfcFeatureElement", supertype: Some("IfcElement"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcFeatureElementSubtraction", supertype: Some("IfcFeatureElement"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcFooting", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcGeometricRepresentationContext", supertype: Some("IfcRepresentationContext"), is_abstract: false, attributes: &["ContextIdentifier", "ContextType", "CoordinateSpaceDimension", "Precision", "WorldCoordinateSystem", "TrueNorth"] },
    EntityDef { name: "IfcGeometricRepresentationItem", supertype: Some("IfcRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcGeometricRepresentationSubContext", supertype: Some("IfcGeometricRepresentationContext"), is_abstract: false, attributes: &["ContextIdentifier", "ContextType", "CoordinateSpaceDimension", "Precision", "WorldCoordinateSystem", "TrueNorth", "ParentContext", "TargetScale", "TargetView", "UserDefinedTargetView"] },
    EntityDef { name: "IfcGroup", supertype: Some("IfcObject"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType"] },
    EntityDef { name: "IfcHalfSpaceSolid", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: false, attributes: &["BaseSurface", "AgreementFlag"] },
    EntityDef { name: "IfcIndexedPolyCurve", supertype: Some("IfcBoundedCurve"), is_abstract: false, attributes: &["Points", "Segments", "SelfIntersect"] },
    EntityDef { name: "IfcIndexedPolygonalFace", supertype: Some("IfcTessellatedItem"), is_abstract: false, attributes: &["CoordIndex"] },
    EntityDef { name: "IfcIShapeProfileDef", supertype: Some("IfcParameterizedProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "Position", "OverallWidth", "OverallDepth", "WebThickness", "FlangeThickness", "FilletRadius", "FlangeEdgeRadius", "FlangeSlope"] },
    EntityDef { name: "IfcLocalPlacement", supertype: Some("IfcObjectPlacement"), is_abstract: false, attributes: &["PlacementRelTo", "RelativePlacement"] },
    EntityDef { name: "IfcLoop", supertype: Some("IfcTopologicalRepresentationItem"), is_abstract: false, attributes: &[] },
    EntityDef { name: "IfcManifoldSolidBrep", supertype: Some("IfcSolidModel"), is_abstract: true, attributes: &["Outer"] },
    EntityDef { name: "IfcMappedItem", supertype: Some("IfcRepresentationItem"), is_abstract: false, attributes: &["MappingSource", "MappingTarget"] },
    EntityDef { name: "IfcMaterial", supertype: Some("IfcMaterialDefinition"), is_abstract: false, attributes: &["Name", "Description", "Category"] },
    EntityDef { name: "IfcMaterialDefinition", supertype: None, is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcMaterialLayer", supertype: Some("IfcMaterialDefinition"), is_abstract: false, attributes: &["Material", "LayerThickness", "IsVentilated", "Name", "Description", "Category", "Priority"] },
    EntityDef { name: "IfcMaterialLayerSet", supertype: Some("IfcMaterialDefinition"), is_abstract: false, attributes: &["MaterialLayers", "LayerSetName", "Description"] },
    EntityDef { name: "IfcMaterialLayerSetUsage", supertype: Some("IfcMaterialUsageDefinition"), is_abstract: false, attributes: &["ForLayerSet", "LayerSetDirection", "DirectionSense", "OffsetFromReferenceLine", "ReferenceExtent"] },
    EntityDef { name: "IfcMaterialList", supertype: None, is_abstract: false, attributes: &["Materials"] },
    EntityDef { name: "IfcMaterialUsageDefinition", supertype: None, is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcMeasureWithUnit", supertype: None, is_abstract: false, attributes: &["ValueComponent", "UnitComponent"] },
    EntityDef { name: "IfcMember", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcMemberType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcNamedUnit", supertype: None, is_abstract: true, attributes: &["Dimensions", "UnitType"] },
    EntityDef { name: "IfcObject", supertype: Some("IfcObjectDefinition"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType"] },
    EntityDef { name: "IfcObjectDefinition", supertype: Some("IfcRoot"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcObjectPlacement", supertype: None, is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcOpeningElement", supertype: Some("IfcFeatureElementSubtraction"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcOpenShell", supertype: Some("IfcConnectedFaceSet"), is_abstract: false, attributes: &["CfsFaces"] },
    EntityDef { name: "IfcOrganization", supertype: None, is_abstract: false, attributes: &["Identification", "Name", "Description", "Roles", "Addresses"] },
    EntityDef { name: "IfcOwnerHistory", supertype: None, is_abstract: false, attributes: &["OwningUser", "OwningApplication", "State", "ChangeAction", "LastModifiedDate", "LastModifyingUser", "LastModifyingApplication", "CreationDate"] },
    EntityDef { name: "IfcParameterizedProfileDef", supertype: Some("IfcProfileDef"), is_abstract: true, attributes: &["ProfileType", "ProfileName", "Position"] },
    EntityDef { name: "IfcPerson", supertype: None, is_abstract: false, attributes: &["Identification", "FamilyName", "GivenName", "MiddleNames", "PrefixTitles", "SuffixTitles", "Roles", "Addresses"] },
    EntityDef { name: "IfcPersonAndOrganization", supertype: None, is_abstract: false, attributes: &["ThePerson", "TheOrganization", "Roles"] },
    EntityDef { name: "IfcPhysicalQuantity", supertype: None, is_abstract: true, attributes: &["Name", "Description"] },
    EntityDef { name: "IfcPhysicalSimpleQuantity", supertype: Some("IfcPhysicalQuantity"), is_abstract: true, attributes: &["Name", "Description", "Unit"] },
    EntityDef { name: "IfcPile", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType", "ConstructionType"] },
    EntityDef { name: "IfcPixelTexture", supertype: Some("IfcSurfaceTexture"), is_abstract: false, attributes: &["RepeatS", "RepeatT", "Mode", "TextureTransform", "Parameter", "Width", "Height", "ColourComponents", "Pixel"] },
    EntityDef { name: "IfcPlacement", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &["Location"] },
    EntityDef { name: "IfcPlane", supertype: Some("IfcElementarySurface"), is_abstract: false, attributes: &["Position"] },
    EntityDef { name: "IfcPlate", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcPlateType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcPoint", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcPolygonalBoundedHalfSpace", supertype: Some("IfcHalfSpaceSolid"), is_abstract: false, attributes: &["BaseSurface", "AgreementFlag", "Position", "PolygonalBoundary"] },
    EntityDef { name: "IfcPolygonalFaceSet", supertype: Some("IfcTessellatedFaceSet"), is_abstract: false, attributes: &["Coordinates", "Closed", "Faces", "PnIndex"] },
    EntityDef { name: "IfcPolyline", supertype: Some("IfcBoundedCurve"), is_abstract: false, attributes: &["Points"] },
    EntityDef { name: "IfcPolyLoop", supertype: Some("IfcLoop"), is_abstract: false, attributes: &["Polygon"] },
    EntityDef { name: "IfcPresentationItem", supertype: None, is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcPresentationStyle", supertype: None, is_abstract: true, attributes: &["Name"] },
    EntityDef { name: "IfcProduct", supertype: Some("IfcObject"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation"] },
    EntityDef { name: "IfcProductDefinitionShape", supertype: Some("IfcProductRepresentation"), is_abstract: false, attributes: &["Name", "Description", "Representations"] },
    EntityDef { name: "IfcProductRepresentation", supertype: None, is_abstract: true, attributes: &["Name", "Description", "Representations"] },
    EntityDef { name: "IfcProfileDef", supertype: None, is_abstract: false, attributes: &["ProfileType", "ProfileName"] },
    EntityDef { name: "IfcProject", supertype: Some("IfcContext"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "LongName", "Phase", "RepresentationContexts", "UnitsInContext"] },
    EntityDef { name: "IfcProperty", supertype: Some("IfcPropertyAbstraction"), is_abstract: true, attributes: &["Name", "Description"] },
    EntityDef { name: "IfcPropertyAbstraction", supertype: None, is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcPropertyBoundedValue", supertype: Some("IfcSimpleProperty"), is_abstract: false, attributes: &["Name", "Description", "UpperBoundValue", "LowerBoundValue", "Unit", "SetPointValue"] },
    EntityDef { name: "IfcPropertyDefinition", supertype: Some("IfcRoot"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcPropertyEnumeratedValue", supertype: Some("IfcSimpleProperty"), is_abstract: false, attributes: &["Name", "Description", "EnumerationValues", "EnumerationReference"] },
    EntityDef { name: "IfcPropertyListValue", supertype: Some("IfcSimpleProperty"), is_abstract: false, attributes: &["Name", "Description", "ListValues", "Unit"] },
    EntityDef { name: "IfcPropertySet", supertype: Some("IfcPropertySetDefinition"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "HasProperties"] },
    EntityDef { name: "IfcPropertySetDefinition", supertype: Some("IfcPropertyDefinition"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcPropertySingleValue", supertype: Some("IfcSimpleProperty"), is_abstract: false, attributes: &["Name", "Description", "NominalValue", "Unit"] },
    EntityDef { name: "IfcQuantityArea", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "AreaValue", "Formula"] },
    EntityDef { name: "IfcQuantityCount", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "CountValue", "Formula"] },
    EntityDef { name: "IfcQuantityLength", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "LengthValue", "Formula"] },
    EntityDef { name: "IfcQuantitySet", supertype: Some("IfcPropertySetDefinition"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcQuantityVolume", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "VolumeValue", "Formula"] },
    EntityDef { name: "IfcQuantityWeight", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "WeightValue", "Formula"] },
    EntityDef { name: "IfcRailing", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcRamp", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcRectangleProfileDef", supertype: Some("IfcParameterizedProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "Position", "XDim", "YDim"] },
    EntityDef { name: "IfcRelAggregates", supertype: Some("IfcRelDecomposes"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatingObject", "RelatedObjects"] },
    EntityDef { name: "IfcRelAssigns", supertype: Some("IfcRelationship"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatedObjectsType"] },
    EntityDef { name: "IfcRelAssignsToGroup", supertype: Some("IfcRelAssigns"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatedObjectsType", "RelatingGroup"] },
    EntityDef { name: "IfcRelAssociates", supertype: Some("IfcRelationship"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects"] },
    EntityDef { name: "IfcRelAssociatesMaterial", supertype: Some("IfcRelAssociates"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatingMaterial"] },
    EntityDef { name: "IfcRelationship", supertype: Some("IfcRoot"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcRelConnects", supertype: Some("IfcRelationship"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcRelContainedInSpatialStructure", supertype: Some("IfcRelConnects"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedElements", "RelatingStructure"] },
    EntityDef { name: "IfcRelDecomposes", supertype: Some("IfcRelationship"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcRelDefines", supertype: Some("IfcRelationship"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcRelDefinesByProperties", supertype: Some("IfcRelDefines"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatingPropertyDefinition"] },
    EntityDef { name: "IfcRelDefinesByType", supertype: Some("IfcRelDefines"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatingType"] },
    EntityDef { name: "IfcRelFillsElement", supertype: Some("IfcRelConnects"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatingOpeningElement", "RelatedBuildingElement"] },
    EntityDef { name: "IfcRelSpaceBoundary", supertype: Some("IfcRelConnects"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatingSpace", "RelatedBuildingElement", "ConnectionGeometry", "PhysicalOrVirtualBoundary", "InternalOrExternalBoundary"] },
    EntityDef { name: "IfcRelVoidsElement", supertype: Some("IfcRelDecomposes"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatingBuildingElement", "RelatedOpeningElement"] },
    EntityDef { name: "IfcRepresentation", supertype: None, is_abstract: true, attributes: &["ContextOfItems", "RepresentationIdentifier", "RepresentationType", "Items"] },
    EntityDef { name: "IfcRepresentationContext", supertype: None, is_abstract: true, attributes: &["ContextIdentifier", "ContextType"] },
    EntityDef { name: "IfcRepresentationItem", supertype: None, is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcRepresentationMap", supertype: None, is_abstract: false, attributes: &["MappingOrigin", "MappedRepresentation"] },
    EntityDef { name: "IfcRoof", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcRoot", supertype: None, is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcShapeModel", supertype: Some("IfcRepresentation"), is_abstract: true, attributes: &["ContextOfItems", "RepresentationIdentifier", "RepresentationType", "Items"] },
    EntityDef { name: "IfcShapeRepresentation", supertype: Some("IfcShapeModel"), is_abstract: false, attributes: &["ContextOfItems", "RepresentationIdentifier", "RepresentationType", "Items"] },
    EntityDef { name: "IfcSimpleProperty", supertype: Some("IfcProperty"), is_abstract: true, attributes: &["Name", "Description"] },
    EntityDef { name: "IfcSite", supertype: Some("IfcSpatialStructureElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType", "RefLatitude", "RefLongitude", "RefElevation", "LandTitleNumber", "SiteAddress"] },
    EntityDef { name: "IfcSIUnit", supertype: Some("IfcNamedUnit"), is_abstract: false, attributes: &["Dimensions", "UnitType", "Prefix", "Name"] },
    EntityDef { name: "IfcSlab", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcSlabType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcSolidModel", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcSpace", supertype: Some("IfcSpatialStructureElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType", "PredefinedType", "ElevationWithFlooring"] },
    EntityDef { name: "IfcSpatialElement", supertype: Some("IfcProduct"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName"] },
    EntityDef { name: "IfcSpatialStructureElement", supertype: Some("IfcSpatialElement"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType"] },
    EntityDef { name: "IfcStair", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcStairFlight", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "NumberOfRisers", "NumberOfTreads", "RiserHeight", "TreadLength", "PredefinedType"] },
    EntityDef { name: "IfcStyledItem", supertype: Some("IfcRepresentationItem"), is_abstract: false, attributes: &["Item", "Styles", "Name"] },
    EntityDef { name: "IfcSurface", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcSurfaceStyle", supertype: Some("IfcPresentationStyle"), is_abstract: false, attributes: &["Name", "Side", "Styles"] },
    EntityDef { name: "IfcSurfaceStyleRendering", supertype: Some("IfcSurfaceStyleShading"), is_abstract: false, attributes: &["SurfaceColour", "Transparency", "DiffuseColour", "TransmissionColour", "DiffuseTransmissionColour", "ReflectionColour", "SpecularColour", "SpecularHighlight", "ReflectanceMethod"] },
    EntityDef { name: "IfcSurfaceStyleShading", supertype: Some("IfcPresentationItem"), is_abstract: false, attributes: &["SurfaceColour", "Transparency"] },
    EntityDef { name: "IfcSurfaceTexture", supertype: Some("IfcPresentationItem"), is_abstract: true, attributes: &["RepeatS", "RepeatT", "Mode", "TextureTransform", "Parameter"] },
    EntityDef { name: "IfcSweptAreaSolid", supertype: Some("IfcSolidModel"), is_abstract: true, attributes: &["SweptArea", "Position"] },
    EntityDef { name: "IfcSystem", supertype: Some("IfcGroup"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType"] },
    EntityDef { name: "IfcTessellatedFaceSet", supertype: Some("IfcTessellatedItem"), is_abstract: true, attributes: &["Coordinates"] },
    EntityDef { name: "IfcTessellatedItem", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcTopologicalRepresentationItem", supertype: Some("IfcRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcTriangulatedFaceSet", supertype: Some("IfcTessellatedFaceSet"), is_abstract: false, attributes: &["Coordinates", "Normals", "Closed", "CoordIndex", "PnIndex"] },
    EntityDef { name: "IfcTypeObject", supertype: Some("IfcObjectDefinition"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets"] },
    EntityDef { name: "IfcTypeProduct", supertype: Some("IfcTypeObject"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag"] },
    EntityDef { name: "IfcUnitAssignment", supertype: None, is_abstract: false, attributes: &["Units"] },
    EntityDef { name: "IfcVector", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: false, attributes: &["Orientation", "Magnitude"] },
    EntityDef { name: "IfcWall", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcWallStandardCase", supertype: Some("IfcWall"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcWallType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcWindow", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "OverallHeight", "OverallWidth", "PredefinedType", "PartitioningType", "UserDefinedPartitioningType"] },
    EntityDef { name: "IfcWindowType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType", "PartitioningType", "ParameterTakesPrecedence", "UserDefinedPartitioningType"] },
    EntityDef { name: "IfcZone", supertype: Some("IfcSystem"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "LongName"] },
];

pub(crate) static IFC2X3: &[EntityDef] = &[
    EntityDef { name: "IfcApplication", supertype: None, is_abstract: false, attributes: &["ApplicationDeveloper", "Version", "ApplicationFullName", "ApplicationIdentifier"] },
    EntityDef { name: "IfcArbitraryClosedProfileDef", supertype: Some("IfcProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "OuterCurve"] },
    EntityDef { name: "IfcArbitraryProfileDefWithVoids", supertype: Some("IfcArbitraryClosedProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "OuterCurve", "InnerCurves"] },
    EntityDef { name: "IfcAxis2Placement2D", supertype: Some("IfcPlacement"), is_abstract: false, attributes: &["Location", "RefDirection"] },
    EntityDef { name: "IfcAxis2Placement3D", supertype: Some("IfcPlacement"), is_abstract: false, attributes: &["Location", "Axis", "RefDirection"] },
    EntityDef { name: "IfcBeam", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcBeamType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcBlobTexture", supertype: Some("IfcSurfaceTexture"), is_abstract: false, attributes: &["RepeatS", "RepeatT", "TextureType", "TextureTransform", "RasterFormat", "RasterCode"] },
    EntityDef { name: "IfcBooleanClippingResult", supertype: Some("IfcBooleanResult"), is_abstract: false, attributes: &["Operator", "FirstOperand", "SecondOperand"] },
    EntityDef { name: "IfcBooleanResult", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: false, attributes: &["Operator", "FirstOperand", "SecondOperand"] },
    EntityDef { name: "IfcBoundedCurve", supertype: Some("IfcCurve"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcBuilding", supertype: Some("IfcSpatialStructureElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType", "ElevationOfRefHeight", "ElevationOfTerrain", "BuildingAddress"] },
    EntityDef { name: "IfcBuildingElement", supertype: Some("IfcElement"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcBuildingElementProxy", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "CompositionType"] },
    EntityDef { name: "IfcBuildingElementProxyType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcBuildingElementType", supertype: Some("IfcElementType"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType"] },
    EntityDef { name: "IfcBuildingStorey", supertype: Some("IfcSpatialStructureElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType", "Elevation"] },
    EntityDef { name: "IfcCartesianPoint", supertype: Some("IfcPoint"), is_abstract: false, attributes: &["Coordinates"] },
    EntityDef { name: "IfcCartesianTransformationOperator", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &["Axis1", "Axis2", "LocalOrigin", "Scale"] },
    EntityDef { name: "IfcCartesianTransformationOperator3D", supertype: Some("IfcCartesianTransformationOperator"), is_abstract: false, attributes: &["Axis1", "Axis2", "LocalOrigin", "Scale", "Axis3"] },
    EntityDef { name: "IfcCircleProfileDef", supertype: Some("IfcParameterizedProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "Position", "Radius"] },
    EntityDef { name: "IfcClosedShell", supertype: Some("IfcConnectedFaceSet"), is_abstract: false, attributes: &["CfsFaces"] },
    EntityDef { name: "IfcColourRgb", supertype: Some("IfcColourSpecification"), is_abstract: false, attributes: &["Name", "Red", "Green", "Blue"] },
    EntityDef { name: "IfcColourSpecification", supertype: None, is_abstract: true, attributes: &["Name"] },
    EntityDef { name: "IfcColumn", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcColumnType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcConnectedFaceSet", supertype: Some("IfcTopologicalRepresentationItem"), is_abstract: false, attributes: &["CfsFaces"] },
    EntityDef { name: "IfcConversionBasedUnit", supertype: Some("IfcNamedUnit"), is_abstract: false, attributes: &["Dimensions", "UnitType", "Name", "ConversionFactor"] },
    EntityDef { name: "IfcCovering", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcCurtainWall", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcCurve", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcDimensionalExponents", supertype: None, is_abstract: false, attributes: &["LengthExponent", "MassExponent", "TimeExponent", "ElectricCurrentExponent", "ThermodynamicTemperatureExponent", "AmountOfSubstanceExponent", "LuminousIntensityExponent"] },
    EntityDef { name: "IfcDirection", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: false, attributes: &["DirectionRatios"] },
    EntityDef { name: "IfcDoor", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "OverallHeight", "OverallWidth"] },
    EntityDef { name: "IfcElement", supertype: Some("IfcProduct"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcElementarySurface", supertype: Some("IfcSurface"), is_abstract: true, attributes: &["Position"] },
    EntityDef { name: "IfcElementQuantity", supertype: Some("IfcPropertySetDefinition"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "MethodOfMeasurement", "Quantities"] },
    EntityDef { name: "IfcElementType", supertype: Some("IfcTypeProduct"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType"] },
    EntityDef { name: "IfcExtrudedAreaSolid", supertype: Some("IfcSweptAreaSolid"), is_abstract: false, attributes: &["SweptArea", "Position", "ExtrudedDirection", "Depth"] },
    EntityDef { name: "IfcFace", supertype: Some("IfcTopologicalRepresentationItem"), is_abstract: false, attributes: &["Bounds"] },
    EntityDef { name: "IfcFaceBound", supertype: Some("IfcTopologicalRepresentationItem"), is_abstract: false, attributes: &["Bound", "Orientation"] },
    EntityDef { name: "IfcFaceOuterBound", supertype: Some("IfcFaceBound"), is_abstract: false, attributes: &["Bound", "Orientation"] },
    EntityDef { name: "IfcFacetedBrep", supertype: Some("IfcManifoldSolidBrep"), is_abstract: false, attributes: &["Outer"] },
    EntityDef { name: "IfcFeatureElement", supertype: Some("IfcElement"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcFeatureElementSubtraction", supertype: Some("IfcFeatureElement"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcFooting", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcGeometricRepresentationContext", supertype: Some("IfcRepresentationContext"), is_abstract: false, attributes: &["ContextIdentifier", "ContextType", "CoordinateSpaceDimension", "Precision", "WorldCoordinateSystem", "TrueNorth"] },
    EntityDef { name: "IfcGeometricRepresentationItem", supertype: Some("IfcRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcGeometricRepresentationSubContext", supertype: Some("IfcGeometricRepresentationContext"), is_abstract: false, attributes: &["ContextIdentifier", "ContextType", "CoordinateSpaceDimension", "Precision", "WorldCoordinateSystem", "TrueNorth", "ParentContext", "TargetScale", "TargetView", "UserDefinedTargetView"] },
    EntityDef { name: "IfcGroup", supertype: Some("IfcObject"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType"] },
    EntityDef { name: "IfcHalfSpaceSolid", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: false, attributes: &["BaseSurface", "AgreementFlag"] },
    EntityDef { name: "IfcIShapeProfileDef", supertype: Some("IfcParameterizedProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "Position", "OverallWidth", "OverallDepth", "WebThickness", "FlangeThickness", "FilletRadius"] },
    EntityDef { name: "IfcLocalPlacement", supertype: Some("IfcObjectPlacement"), is_abstract: false, attributes: &["PlacementRelTo", "RelativePlacement"] },
    EntityDef { name: "IfcLoop", supertype: Some("IfcTopologicalRepresentationItem"), is_abstract: false, attributes: &[] },
    EntityDef { name: "IfcManifoldSolidBrep", supertype: Some("IfcSolidModel"), is_abstract: true, attributes: &["Outer"] },
    EntityDef { name: "IfcMappedItem", supertype: Some("IfcRepresentationItem"), is_abstract: false, attributes: &["MappingSource", "MappingTarget"] },
    EntityDef { name: "IfcMaterial", supertype: None, is_abstract: false, attributes: &["Name"] },
    EntityDef { name: "IfcMaterialLayer", supertype: None, is_abstract: false, attributes: &["Material", "LayerThickness", "IsVentilated"] },
    EntityDef { name: "IfcMaterialLayerSet", supertype: None, is_abstract: false, attributes: &["MaterialLayers", "LayerSetName"] },
    EntityDef { name: "IfcMaterialLayerSetUsage", supertype: None, is_abstract: false, attributes: &["ForLayerSet", "LayerSetDirection", "DirectionSense", "OffsetFromReferenceLine"] },
    EntityDef { name: "IfcMaterialList", supertype: None, is_abstract: false, attributes: &["Materials"] },
    EntityDef { name: "IfcMeasureWithUnit", supertype: None, is_abstract: false, attributes: &["ValueComponent", "UnitComponent"] },
    EntityDef { name: "IfcMember", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcMemberType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcNamedUnit", supertype: None, is_abstract: true, attributes: &["Dimensions", "UnitType"] },
    EntityDef { name: "IfcObject", supertype: Some("IfcObjectDefinition"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType"] },
    EntityDef { name: "IfcObjectDefinition", supertype: Some("IfcRoot"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcObjectPlacement", supertype: None, is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcOpeningElement", supertype: Some("IfcFeatureElementSubtraction"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcOpenShell", supertype: Some("IfcConnectedFaceSet"), is_abstract: false, attributes: &["CfsFaces"] },
    EntityDef { name: "IfcOrganization", supertype: None, is_abstract: false, attributes: &["Id", "Name", "Description", "Roles", "Addresses"] },
    EntityDef { name: "IfcOwnerHistory", supertype: None, is_abstract: false, attributes: &["OwningUser", "OwningApplication", "State", "ChangeAction", "LastModifiedDate", "LastModifyingUser", "LastModifyingApplication", "CreationDate"] },
    EntityDef { name: "IfcParameterizedProfileDef", supertype: Some("IfcProfileDef"), is_abstract: true, attributes: &["ProfileType", "ProfileName", "Position"] },
    EntityDef { name: "IfcPerson", supertype: None, is_abstract: false, attributes: &["Id", "FamilyName", "GivenName", "MiddleNames", "PrefixTitles", "SuffixTitles", "Roles", "Addresses"] },
    EntityDef { name: "IfcPersonAndOrganization", supertype: None, is_abstract: false, attributes: &["ThePerson", "TheOrganization", "Roles"] },
    EntityDef { name: "IfcPhysicalQuantity", supertype: None, is_abstract: true, attributes: &["Name", "Description"] },
    EntityDef { name: "IfcPhysicalSimpleQuantity", supertype: Some("IfcPhysicalQuantity"), is_abstract: true, attributes: &["Name", "Description", "Unit"] },
    EntityDef { name: "IfcPile", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType", "ConstructionType"] },
    EntityDef { name: "IfcPixelTexture", supertype: Some("IfcSurfaceTexture"), is_abstract: false, attributes: &["RepeatS", "RepeatT", "TextureType", "TextureTransform", "Width", "Height", "ColourComponents", "Pixel"] },
    EntityDef { name: "IfcPlacement", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &["Location"] },
    EntityDef { name: "IfcPlane", supertype: Some("IfcElementarySurface"), is_abstract: false, attributes: &["Position"] },
    EntityDef { name: "IfcPlate", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcPlateType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcPoint", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcPolygonalBoundedHalfSpace", supertype: Some("IfcHalfSpaceSolid"), is_abstract: false, attributes: &["BaseSurface", "AgreementFlag", "Position", "PolygonalBoundary"] },
    EntityDef { name: "IfcPolyline", supertype: Some("IfcBoundedCurve"), is_abstract: false, attributes: &["Points"] },
    EntityDef { name: "IfcPolyLoop", supertype: Some("IfcLoop"), is_abstract: false, attributes: &["Polygon"] },
    EntityDef { name: "IfcPresentationStyle", supertype: None, is_abstract: true, attributes: &["Name"] },
    EntityDef { name: "IfcProduct", supertype: Some("IfcObject"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation"] },
    EntityDef { name: "IfcProductDefinitionShape", supertype: Some("IfcProductRepresentation"), is_abstract: false, attributes: &["Name", "Description", "Representations"] },
    EntityDef { name: "IfcProductRepresentation", supertype: None, is_abstract: false, attributes: &["Name", "Description", "Representations"] },
    EntityDef { name: "IfcProfileDef", supertype: None, is_abstract: true, attributes: &["ProfileType", "ProfileName"] },
    EntityDef { name: "IfcProject", supertype: Some("IfcObject"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "LongName", "Phase", "RepresentationContexts", "UnitsInContext"] },
    EntityDef { name: "IfcProperty", supertype: None, is_abstract: true, attributes: &["Name", "Description"] },
    EntityDef { name: "IfcPropertyBoundedValue", supertype: Some("IfcSimpleProperty"), is_abstract: false, attributes: &["Name", "Description", "UpperBoundValue", "LowerBoundValue", "Unit"] },
    EntityDef { name: "IfcPropertyDefinition", supertype: Some("IfcRoot"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcPropertyEnumeratedValue", supertype: Some("IfcSimpleProperty"), is_abstract: false, attributes: &["Name", "Description", "EnumerationValues", "EnumerationReference"] },
    EntityDef { name: "IfcPropertyListValue", supertype: Some("IfcSimpleProperty"), is_abstract: false, attributes: &["Name", "Description", "ListValues", "Unit"] },
    EntityDef { name: "IfcPropertySet", supertype: Some("IfcPropertySetDefinition"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "HasProperties"] },
    EntityDef { name: "IfcPropertySetDefinition", supertype: Some("IfcPropertyDefinition"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcPropertySingleValue", supertype: Some("IfcSimpleProperty"), is_abstract: false, attributes: &["Name", "Description", "NominalValue", "Unit"] },
    EntityDef { name: "IfcQuantityArea", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "AreaValue"] },
    EntityDef { name: "IfcQuantityCount", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "CountValue"] },
    EntityDef { name: "IfcQuantityLength", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "LengthValue"] },
    EntityDef { name: "IfcQuantityVolume", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "VolumeValue"] },
    EntityDef { name: "IfcQuantityWeight", supertype: Some("IfcPhysicalSimpleQuantity"), is_abstract: false, attributes: &["Name", "Description", "Unit", "WeightValue"] },
    EntityDef { name: "IfcRailing", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcRamp", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "ShapeType"] },
    EntityDef { name: "IfcRectangleProfileDef", supertype: Some("IfcParameterizedProfileDef"), is_abstract: false, attributes: &["ProfileType", "ProfileName", "Position", "XDim", "YDim"] },
    EntityDef { name: "IfcRelAggregates", supertype: Some("IfcRelDecomposes"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatingObject", "RelatedObjects"] },
    EntityDef { name: "IfcRelAssigns", supertype: Some("IfcRelationship"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatedObjectsType"] },
    EntityDef { name: "IfcRelAssignsToGroup", supertype: Some("IfcRelAssigns"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatedObjectsType", "RelatingGroup"] },
    EntityDef { name: "IfcRelAssociates", supertype: Some("IfcRelationship"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects"] },
    EntityDef { name: "IfcRelAssociatesMaterial", supertype: Some("IfcRelAssociates"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatingMaterial"] },
    EntityDef { name: "IfcRelationship", supertype: Some("IfcRoot"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcRelConnects", supertype: Some("IfcRelationship"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcRelContainedInSpatialStructure", supertype: Some("IfcRelConnects"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedElements", "RelatingStructure"] },
    EntityDef { name: "IfcRelDecomposes", supertype: Some("IfcRelationship"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatingObject", "RelatedObjects"] },
    EntityDef { name: "IfcRelDefines", supertype: Some("IfcRelationship"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects"] },
    EntityDef { name: "IfcRelDefinesByProperties", supertype: Some("IfcRelDefines"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatingPropertyDefinition"] },
    EntityDef { name: "IfcRelDefinesByType", supertype: Some("IfcRelDefines"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatedObjects", "RelatingType"] },
    EntityDef { name: "IfcRelFillsElement", supertype: Some("IfcRelConnects"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatingOpeningElement", "RelatedBuildingElement"] },
    EntityDef { name: "IfcRelSpaceBoundary", supertype: Some("IfcRelConnects"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatingSpace", "RelatedBuildingElement", "ConnectionGeometry", "PhysicalOrVirtualBoundary", "InternalOrExternalBoundary"] },
    EntityDef { name: "IfcRelVoidsElement", supertype: Some("IfcRelConnects"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "RelatingBuildingElement", "RelatedOpeningElement"] },
    EntityDef { name: "IfcRepresentation", supertype: None, is_abstract: false, attributes: &["ContextOfItems", "RepresentationIdentifier", "RepresentationType", "Items"] },
    EntityDef { name: "IfcRepresentationContext", supertype: None, is_abstract: false, attributes: &["ContextIdentifier", "ContextType"] },
    EntityDef { name: "IfcRepresentationItem", supertype: None, is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcRepresentationMap", supertype: None, is_abstract: false, attributes: &["MappingOrigin", "MappedRepresentation"] },
    EntityDef { name: "IfcRoof", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "ShapeType"] },
    EntityDef { name: "IfcRoot", supertype: None, is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description"] },
    EntityDef { name: "IfcShapeModel", supertype: Some("IfcRepresentation"), is_abstract: true, attributes: &["ContextOfItems", "RepresentationIdentifier", "RepresentationType", "Items"] },
    EntityDef { name: "IfcShapeRepresentation", supertype: Some("IfcShapeModel"), is_abstract: false, attributes: &["ContextOfItems", "RepresentationIdentifier", "RepresentationType", "Items"] },
    EntityDef { name: "IfcSimpleProperty", supertype: Some("IfcProperty"), is_abstract: true, attributes: &["Name", "Description"] },
    EntityDef { name: "IfcSite", supertype: Some("IfcSpatialStructureElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType", "RefLatitude", "RefLongitude", "RefElevation", "LandTitleNumber", "SiteAddress"] },
    EntityDef { name: "IfcSIUnit", supertype: Some("IfcNamedUnit"), is_abstract: false, attributes: &["Dimensions", "UnitType", "Prefix", "Name"] },
    EntityDef { name: "IfcSlab", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "PredefinedType"] },
    EntityDef { name: "IfcSlabType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcSolidModel", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcSpace", supertype: Some("IfcSpatialStructureElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType", "InteriorOrExteriorSpace", "ElevationWithFlooring"] },
    EntityDef { name: "IfcSpatialStructureElement", supertype: Some("IfcProduct"), is_abstract: true, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "LongName", "CompositionType"] },
    EntityDef { name: "IfcStair", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "ShapeType"] },
    EntityDef { name: "IfcStairFlight", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "NumberOfRiser", "NumberOfTreads", "RiserHeight", "TreadLength"] },
    EntityDef { name: "IfcStyledItem", supertype: Some("IfcRepresentationItem"), is_abstract: false, attributes: &["Item", "Styles", "Name"] },
    EntityDef { name: "IfcSurface", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcSurfaceStyle", supertype: Some("IfcPresentationStyle"), is_abstract: false, attributes: &["Name", "Side", "Styles"] },
    EntityDef { name: "IfcSurfaceStyleRendering", supertype: Some("IfcSurfaceStyleShading"), is_abstract: false, attributes: &["SurfaceColour", "Transparency", "DiffuseColour", "TransmissionColour", "DiffuseTransmissionColour", "ReflectionColour", "SpecularColour", "SpecularHighlight", "ReflectanceMethod"] },
    EntityDef { name: "IfcSurfaceStyleShading", supertype: None, is_abstract: false, attributes: &["SurfaceColour"] },
    EntityDef { name: "IfcSurfaceTexture", supertype: None, is_abstract: true, attributes: &["RepeatS", "RepeatT", "TextureType", "TextureTransform"] },
    EntityDef { name: "IfcSweptAreaSolid", supertype: Some("IfcSolidModel"), is_abstract: true, attributes: &["SweptArea", "Position"] },
    EntityDef { name: "IfcTopologicalRepresentationItem", supertype: Some("IfcRepresentationItem"), is_abstract: true, attributes: &[] },
    EntityDef { name: "IfcTypeObject", supertype: Some("IfcObjectDefinition"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets"] },
    EntityDef { name: "IfcTypeProduct", supertype: Some("IfcTypeObject"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag"] },
    EntityDef { name: "IfcUnitAssignment", supertype: None, is_abstract: false, attributes: &["Units"] },
    EntityDef { name: "IfcVector", supertype: Some("IfcGeometricRepresentationItem"), is_abstract: false, attributes: &["Orientation", "Magnitude"] },
    EntityDef { name: "IfcWall", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcWallStandardCase", supertype: Some("IfcWall"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag"] },
    EntityDef { name: "IfcWallType", supertype: Some("IfcBuildingElementType"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ApplicableOccurrence", "HasPropertySets", "RepresentationMaps", "Tag", "ElementType", "PredefinedType"] },
    EntityDef { name: "IfcWindow", supertype: Some("IfcBuildingElement"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType", "ObjectPlacement", "Representation", "Tag", "OverallHeight", "OverallWidth"] },
    EntityDef { name: "IfcZone", supertype: Some("IfcGroup"), is_abstract: false, attributes: &["GlobalId", "OwnerHistory", "Name", "Description", "ObjectType"] },
];
