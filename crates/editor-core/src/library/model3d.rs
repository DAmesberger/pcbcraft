//! Slice D1 (contract C0): the Model definition kind, the "3D library"
//! (founder decision F6). A model is a revisioned library lineage shared by
//! packages: GLB for appearance, STEP AP214/242 for mechanical truth,
//! `.solidcraft` for parametric models. Bytes are packed by content hash
//! (D7; the D0 container raises the sizes).
//!
//! Every mapping is explicit: bodies to materials, lead tags to
//! designations, and package lead maps; an assist only proposes entries
//! in a preview. Hints (`mount`, `pins`, `pitch`, family text) serve search
//! only and are never identity.
use super::package::PackageDefinition;
use super::part::Provenance;
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{ContentHash, DefinitionId, ModelRef, PackageTerminalId};
use crate::materials::MaterialId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use super::package_model::BodyRole;

/// Bodies one model maps.
pub const MAX_MODEL_BODIES: usize = 1024;
/// Custom materials one model carries (from glTF extensions).
pub const MAX_CUSTOM_MATERIALS: usize = 64;
/// Lead tags one model carries.
pub const MAX_LEAD_TAGS: usize = 2048;
/// Bytes of a body or face key.
pub const MAX_BODY_KEY: usize = 256;
/// Bytes of an original file name.
pub const MAX_FILE_NAME: usize = 256;
/// Allowed excess of a model's bounds over the package envelope, mm
/// (finding `model.bounds`).
pub const BOUNDS_TOLERANCE_MM: f64 = 0.1;

/// One packed asset a model names.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetRef {
    pub hash: ContentHash,
    pub bytes: u64,
    pub media_type: String,
    /// The original file name, for people.
    pub file_name: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    #[default]
    Step,
    Gltf,
    Solidcraft,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpAxis {
    #[default]
    Z,
    Y,
}

/// The source file's frame, stated in the import wizard.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceFrame {
    /// Millimetres per source unit.
    pub unit_mm: f64,
    pub up: UpAxis,
    /// Added after scaling, mm.
    pub origin_offset: [f64; 3],
}

impl Default for SourceFrame {
    fn default() -> Self {
        Self {
            unit_mm: 1.0,
            up: UpAxis::Z,
            origin_offset: [0.0; 3],
        }
    }
}

/// A body's material: a cited built-in or one of the model's own.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "material", content = "id", rename_all = "snake_case")]
pub enum ModelMaterial {
    Builtin(MaterialId),
    Custom(u16),
}

/// One body of the source file, by its inspection key.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelBody {
    pub key: String,
    pub role: BodyRole,
    pub material: ModelMaterial,
}

/// A PBR material from a glTF file (metallic-roughness plus the KHR
/// extensions Pcbcraft reads).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CustomMaterial {
    pub name: String,
    pub base_colour: [f64; 4],
    pub metallic: f64,
    pub roughness: f64,
    #[serde(default)]
    pub clearcoat: Option<f64>,
    #[serde(default)]
    pub transmission: Option<f64>,
    #[serde(default)]
    pub ior: Option<f64>,
    #[serde(default)]
    pub emissive: Option<[f64; 3]>,
    /// The file has textures this build does not show (until SolidCraft
    /// persists textures, R2).
    #[serde(default)]
    pub textures_ignored: bool,
}

/// A lead tagged by the user: the body or face and its designation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeadTag {
    pub key: String,
    pub designation: String,
    /// Lowest point of the tagged geometry in the model frame, derived at
    /// import and rechecked on reopen.
    pub contact: [f64; 3],
}

/// The top face area markings use.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MarkingArea {
    pub origin: [f64; 3],
    pub normal: [f64; 3],
    pub u_axis: [f64; 3],
    pub size_mm: [f64; 2],
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mount {
    Smd,
    Tht,
}

/// Search hints: never identity, never compatibility.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelHints {
    #[serde(default)]
    pub mount: Option<Mount>,
    #[serde(default)]
    pub pins: Option<u32>,
    #[serde(default)]
    pub pitch_mm: Option<f64>,
    #[serde(default)]
    pub family: Option<String>,
}

/// Whether the model's licence allows sharing it.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Redistribution {
    Permitted,
    /// Blocks promote and share (FD11).
    Forbidden,
    #[default]
    Unknown,
}

/// Where a model came from.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelProvenance {
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// ISO date the file was retrieved.
    #[serde(default)]
    pub retrieved: Option<String>,
    pub redistribution: Redistribution,
    pub record: Provenance,
}

/// One model revision.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelDefinition {
    pub asset: AssetRef,
    /// An optional appearance GLB paired with a STEP asset.
    #[serde(default)]
    pub pair: Option<AssetRef>,
    pub format: ModelFormat,
    pub frame: SourceFrame,
    /// From inspection, rechecked on reopen (a mismatch refuses and names
    /// the asset).
    pub bounds_mm: [[f64; 3]; 2],
    pub bodies: Vec<ModelBody>,
    #[serde(default)]
    pub custom_materials: Vec<CustomMaterial>,
    #[serde(default)]
    pub leads: Vec<LeadTag>,
    #[serde(default)]
    pub marking_area: Option<MarkingArea>,
    #[serde(default)]
    pub hints: ModelHints,
    pub provenance: ModelProvenance,
}

impl ModelDefinition {
    pub(super) fn within_bounds(&self) -> bool {
        self.bodies.len() <= MAX_MODEL_BODIES
            && self.custom_materials.len() <= MAX_CUSTOM_MATERIALS
            && self.leads.len() <= MAX_LEAD_TAGS
            && self.asset.file_name.len() <= MAX_FILE_NAME
            && self.bodies.iter().all(|b| b.key.len() <= MAX_BODY_KEY)
            && self.leads.iter().all(|l| l.key.len() <= MAX_BODY_KEY)
    }

    pub(super) fn values_finite(&self) -> bool {
        let finite = |v: &[f64]| v.iter().all(|x| x.is_finite());
        finite(&self.bounds_mm[0])
            && finite(&self.bounds_mm[1])
            && self.frame.unit_mm.is_finite()
            && finite(&self.frame.origin_offset)
            && self.leads.iter().all(|l| finite(&l.contact))
    }

    /// Publish findings: assets packed, every body mapped, licence stated,
    /// leads well formed (`model.bounds`, `model.licence`, `model.leads`,
    /// `model.body_unmapped`, `model.asset`). Contract: refused with
    /// `model.unsupported` until D1.
    pub(super) fn findings(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        out.push(Finding::blocking(
            "model.unsupported",
            "3D library models cannot be published yet.".into(),
            vec![ObjectRef::Definition { id }],
            Stage::Publish,
        ));
    }
}

/// A package's library model: which model revision, and the explicit map
/// from model lead tags (by index) to package terminals. Placement stays
/// `PackageDefinition::placement`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LibraryModel {
    pub model: ModelRef,
    pub lead_map: Vec<LeadMapEntry>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LeadMapEntry {
    /// Index into `ModelDefinition::leads`.
    pub lead: u16,
    pub terminal: PackageTerminalId,
}

/// The `library.import_model` request: inline asset bytes (bounded by the
/// packed-asset limits) and the wizard's definition. Creates a Model draft.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImportModel {
    pub name: String,
    pub assets: Vec<super::PackedAsset>,
    pub definition: Box<ModelDefinition>,
}

/// Findings of a package that uses a library model (lead map, bounds
/// against the envelope). Contract: refused until D1.
pub fn package_findings(
    _package: &PackageDefinition,
    _model: &LibraryModel,
    id: DefinitionId,
    out: &mut Vec<Finding>,
) {
    out.push(Finding::blocking(
        "package.model_unsupported",
        "Library 3D models cannot be used yet.".into(),
        vec![ObjectRef::Definition { id }],
        Stage::Publish,
    ));
}

/// Stage `library.import_model`. Contract stub until D1.
pub fn import_model(_request: &ImportModel) -> Result<(super::DefinitionMeta, super::Definition)> {
    Err(Error::Unsupported(
        "model.unsupported: importing 3D library models is not available yet (slice D1)".into(),
    ))
}
