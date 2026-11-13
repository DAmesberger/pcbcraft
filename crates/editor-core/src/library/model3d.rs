//! Slice D1: the Model definition kind, the "3D library"
//! (founder decision F6). A model is a revisioned library lineage shared by
//! packages: GLB for appearance, STEP AP214/242 for mechanical truth,
//! `.solidcraft` for parametric models. Bytes are packed by content hash
//! (D7; the D0 container raises the sizes).
//!
//! Every mapping is explicit: bodies to materials, lead tags to
//! designations, and package lead maps; an assist only proposes entries
//! in a preview. Hints (`mount`, `pins`, `pitch`, family text) serve search
//! only and are never identity.
use super::Library;
use super::bundle::MAX_MEDIA_TYPE;
use super::package::PackageDefinition;
use super::package_model::PackageModel;
use super::part::{MAX_URL, Provenance};
use super::{MAX_LABEL, printable};
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{ContentHash, DefinitionId, ModelRef, PackageTerminalId};
use crate::materials::MaterialId;
use crate::units::Nm;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
/// Largest pin-count hint.
pub const MAX_HINT_PINS: u32 = 65_535;

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
    /// Added after scaling.
    pub origin_offset_nm: [Nm; 3],
}

impl Default for SourceFrame {
    fn default() -> Self {
        Self {
            unit_mm: 1.0,
            up: UpAxis::Z,
            origin_offset_nm: [Nm::ZERO; 3],
        }
    }
}

impl SourceFrame {
    /// The offset added after scaling, mm.
    pub fn origin_offset(&self) -> [f64; 3] {
        self.origin_offset_nm.map(Nm::to_mm_f64)
    }
}

/// The wire form of [`SourceFrame`]: the origin offset in millimetres.
/// Written by hand, not derived, because the payloads name it
/// `origin_offset` (the stored name has no unit suffix the derive could
/// drop).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceFrameParams {
    /// Millimetres per source unit.
    pub unit_mm: f64,
    pub up: UpAxis,
    /// Added after scaling, mm.
    pub origin_offset: [crate::units::Length; 3],
}

impl From<SourceFrame> for SourceFrameParams {
    fn from(f: SourceFrame) -> Self {
        Self {
            unit_mm: f.unit_mm,
            up: f.up,
            origin_offset: f.origin_offset_nm.map(crate::units::Length),
        }
    }
}

impl From<SourceFrameParams> for SourceFrame {
    fn from(f: SourceFrameParams) -> Self {
        Self {
            unit_mm: f.unit_mm,
            up: f.up,
            origin_offset_nm: f.origin_offset.map(|l| l.0),
        }
    }
}

/// A computed point or size, mm, as stored: the one crossing
/// ([`Nm::from_mm_f64_rounded`]) for each coordinate.
pub fn triple_nm(mm: [f64; 3]) -> Result<[Nm; 3]> {
    let [x, y, z] = mm;
    Ok([
        Nm::from_mm_f64_rounded(x)?,
        Nm::from_mm_f64_rounded(y)?,
        Nm::from_mm_f64_rounded(z)?,
    ])
}

/// Computed bounds `[min, max]`, mm, as stored ([`triple_nm`]).
pub fn bounds_nm(mm: [[f64; 3]; 2]) -> Result<[[Nm; 3]; 2]> {
    Ok([triple_nm(mm[0])?, triple_nm(mm[1])?])
}

/// Stored bounds `[min, max]` in millimetres.
pub fn bounds_mm(nm: [[Nm; 3]; 2]) -> [[f64; 3]; 2] {
    nm.map(|p| p.map(Nm::to_mm_f64))
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
/// extensions Sparksmith reads).
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
    pub contact_nm: [Nm; 3],
}

impl LeadTag {
    /// The contact point, mm.
    pub fn contact(&self) -> [f64; 3] {
        self.contact_nm.map(Nm::to_mm_f64)
    }
}

/// The top face area markings use: a plane at `origin` with the unit
/// `normal` and `u_axis`, `size` along u and v. Lengths are stored in
/// nanometres; [`Self::origin`] and [`Self::size`] give millimetres.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MarkingArea {
    pub origin_nm: [Nm; 3],
    pub normal: [f64; 3],
    pub u_axis: [f64; 3],
    pub size_nm: [Nm; 2],
}

impl MarkingArea {
    /// An area from millimetres, through the one crossing into stored
    /// lengths ([`Nm::from_mm_f64_rounded`]).
    pub fn from_mm(
        origin: [f64; 3],
        normal: [f64; 3],
        u_axis: [f64; 3],
        size: [f64; 2],
    ) -> Result<Self> {
        Ok(Self {
            origin_nm: triple_nm(origin)?,
            normal,
            u_axis,
            size_nm: [
                Nm::from_mm_f64_rounded(size[0])?,
                Nm::from_mm_f64_rounded(size[1])?,
            ],
        })
    }

    /// The origin, mm.
    pub fn origin(&self) -> [f64; 3] {
        self.origin_nm.map(Nm::to_mm_f64)
    }

    /// The size along u and v, mm.
    pub fn size(&self) -> [f64; 2] {
        self.size_nm.map(Nm::to_mm_f64)
    }
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
    pub pitch_nm: Option<Nm>,
    #[serde(default)]
    pub family: Option<String>,
}

impl ModelHints {
    /// The pitch hint, mm.
    pub fn pitch_mm(&self) -> Option<f64> {
        self.pitch_nm.map(Nm::to_mm_f64)
    }
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
    /// the asset): `[min, max]`.
    pub bounds_nm: [[Nm; 3]; 2],
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

impl ModelFormat {
    /// The media type a staged asset of this format gets.
    pub fn media_type(self) -> &'static str {
        self.media_types()[0]
    }

    /// Media types an asset of this format may carry: the canonical one
    /// first (STEP `model/step`, RFC 9568's glTF types, SolidCraft's
    /// native model).
    pub fn media_types(self) -> &'static [&'static str] {
        match self {
            Self::Step => &["model/step", "application/step"],
            Self::Gltf => &["model/gltf-binary", "model/gltf+json"],
            Self::Solidcraft => &["application/x-solidcraft"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Step => "STEP",
            Self::Gltf => "glTF",
            Self::Solidcraft => "SolidCraft",
        }
    }
}

impl Redistribution {
    pub fn label(self) -> &'static str {
        match self {
            Self::Permitted => "permitted",
            Self::Forbidden => "forbidden",
            Self::Unknown => "unknown",
        }
    }
}

impl Mount {
    pub fn label(self) -> &'static str {
        match self {
            Self::Smd => "SMD",
            Self::Tht => "THT",
        }
    }
}

/// Whether `v` is finite and within the geometry range.
fn finite_all(v: &[f64]) -> bool {
    v.iter()
        .all(|x| x.is_finite() && x.abs() <= MAX_COORDINATE_MM)
}

/// Coordinates a model may use, mm (the document's geometry range).
const MAX_COORDINATE_MM: f64 = 10_000.0;

fn text_ok(text: &Option<String>, max: usize) -> bool {
    text.as_ref().is_none_or(|t| t.len() <= max)
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The problem with one asset reference, if any.
fn asset_problem(asset: &AssetRef, media_types: &[&str]) -> Option<String> {
    if asset.hash == ContentHash::default() {
        return Some("names no packed asset (its hash is unset)".into());
    }
    if asset.bytes == 0 {
        return Some(format!("asset {} is empty", asset.hash));
    }
    if !printable(&asset.file_name) {
        return Some(format!("asset {} needs its original file name", asset.hash));
    }
    if !media_types.contains(&asset.media_type.as_str()) {
        return Some(format!(
            "asset ‘{}’ has media type ‘{}’; expected {}",
            asset.file_name,
            asset.media_type,
            media_types.join(" or ")
        ));
    }
    None
}

impl ModelDefinition {
    pub(super) fn within_bounds(&self) -> bool {
        let asset_ok = |a: &AssetRef| {
            a.file_name.len() <= MAX_FILE_NAME && a.media_type.len() <= MAX_MEDIA_TYPE
        };
        let p = &self.provenance;
        self.bodies.len() <= MAX_MODEL_BODIES
            && self.custom_materials.len() <= MAX_CUSTOM_MATERIALS
            && self.leads.len() <= MAX_LEAD_TAGS
            && asset_ok(&self.asset)
            && self.pair.as_ref().is_none_or(asset_ok)
            && self.bodies.iter().all(|b| b.key.len() <= MAX_BODY_KEY)
            && self
                .leads
                .iter()
                .all(|l| l.key.len() <= MAX_BODY_KEY && l.designation.len() <= MAX_LABEL)
            && self
                .custom_materials
                .iter()
                .all(|m| m.name.len() <= MAX_LABEL)
            && text_ok(&self.hints.family, MAX_LABEL)
            && self.hints.pins.is_none_or(|n| n <= MAX_HINT_PINS)
            && text_ok(&p.vendor, MAX_LABEL)
            && text_ok(&p.url, MAX_URL)
            && text_ok(&p.retrieved, MAX_LABEL)
            && text_ok(&p.record.licence, MAX_LABEL)
    }

    pub(super) fn values_finite(&self) -> bool {
        let materials = self.custom_materials.iter().all(|m| {
            finite_all(&m.base_colour)
                && finite_all(&[m.metallic, m.roughness])
                && finite_all(&[
                    m.clearcoat.unwrap_or(0.0),
                    m.transmission.unwrap_or(0.0),
                    m.ior.unwrap_or(1.0),
                ])
                && m.emissive.is_none_or(|e| finite_all(&e))
        });
        // Stored lengths are whole nanometres within ±2 m, finite by type.
        let area = self
            .marking_area
            .is_none_or(|a| finite_all(&a.normal) && finite_all(&a.u_axis));
        self.frame.unit_mm.is_finite() && materials && area
    }

    /// The bounds `[min, max]`, mm.
    pub fn bounds(&self) -> [[f64; 3]; 2] {
        bounds_mm(self.bounds_nm)
    }

    /// The bounds' length, width and height, mm.
    pub fn size_mm(&self) -> [f64; 3] {
        let [lo, hi] = self.bounds();
        [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]
    }

    /// The pin count search uses: the hint, else the distinct lead
    /// designations. Never identity.
    pub fn pin_count(&self) -> Option<u32> {
        self.hints.pins.or_else(|| {
            let distinct: BTreeSet<&str> =
                self.leads.iter().map(|l| l.designation.as_str()).collect();
            u32::try_from(distinct.len()).ok().filter(|n| *n > 0)
        })
    }

    /// The file has textures this build does not show.
    pub fn has_textures(&self) -> bool {
        self.custom_materials.iter().any(|m| m.textures_ignored)
    }

    /// The assets the model names: its asset, then its pair.
    pub fn assets(&self) -> impl Iterator<Item = &AssetRef> {
        std::iter::once(&self.asset).chain(self.pair.as_ref())
    }

    /// Publish findings: assets named, every body mapped, licence stated,
    /// leads well formed (`model.bounds`, `model.licence`, `model.leads`,
    /// `model.body_unmapped`, `model.asset`). Whether the assets are packed
    /// is a snapshot rule ([`snapshot_problems`]).
    pub(super) fn findings(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        let this = || vec![ObjectRef::Definition { id }];
        let block =
            |code: &str, message: String| Finding::blocking(code, message, this(), Stage::Publish);
        // Assets.
        if let Some(problem) = asset_problem(&self.asset, self.format.media_types()) {
            out.push(block("model.asset", format!("The model {problem}.")));
        }
        if let Some(pair) = &self.pair {
            if self.format != ModelFormat::Step {
                out.push(block(
                    "model.asset",
                    "Only a STEP model takes an appearance pair.".into(),
                ));
            } else if pair.hash == self.asset.hash {
                out.push(block(
                    "model.asset",
                    "The appearance pair is the model's own asset.".into(),
                ));
            } else if let Some(problem) = asset_problem(pair, ModelFormat::Gltf.media_types()) {
                out.push(block(
                    "model.asset",
                    format!("The appearance pair {problem}."),
                ));
            }
        }
        // Frame and bounds.
        if self.frame.unit_mm.is_nan() || self.frame.unit_mm <= 0.0 {
            out.push(block(
                "model.bounds",
                "The source frame needs a positive unit scale.".into(),
            ));
        }
        let [lo, hi] = self.bounds();
        if (0..3).any(|i| hi[i] <= lo[i]) {
            out.push(block(
                "model.bounds",
                "The model bounds must have positive length, width and height.".into(),
            ));
        }
        let inside = |p: [f64; 3]| {
            (0..3)
                .all(|i| p[i] >= lo[i] - BOUNDS_TOLERANCE_MM && p[i] <= hi[i] + BOUNDS_TOLERANCE_MM)
        };
        if let Some(area) = self.marking_area {
            let normal = length(area.normal);
            let u = length(area.u_axis);
            let unit = |l: f64| (l - 1.0).abs() <= 1e-6;
            if !unit(normal)
                || !unit(u)
                || dot(area.normal, area.u_axis).abs() > 1e-6
                || area.size_nm.iter().any(|s| *s <= Nm::ZERO)
                || !inside(area.origin())
            {
                out.push(block(
                    "model.bounds",
                    "The marking area needs unit, perpendicular normal and u axes, a positive size and an origin within the model bounds.".into(),
                ));
            }
        }
        // Bodies and materials.
        // A part file's looks come from its appearance names (§11 L2).
        if self.bodies.is_empty() && self.format != ModelFormat::Solidcraft {
            out.push(block(
                "model.body_unmapped",
                "The model maps no body to a role and material.".into(),
            ));
        }
        let mut keys = BTreeSet::new();
        for body in &self.bodies {
            if body.key.trim().is_empty() || !keys.insert(body.key.as_str()) {
                out.push(block(
                    "model.body_unmapped",
                    format!("Body key ‘{}’ is empty or mapped more than once.", body.key),
                ));
            }
            if let ModelMaterial::Custom(index) = body.material
                && usize::from(index) >= self.custom_materials.len()
            {
                out.push(block(
                    "model.body_unmapped",
                    format!(
                        "Body ‘{}’ names custom material {index}, which the model does not carry.",
                        body.key
                    ),
                ));
            }
        }
        for material in &self.custom_materials {
            let unit = |v: f64| (0.0..=1.0).contains(&v);
            let valid = printable(&material.name)
                && material.base_colour.iter().all(|c| unit(*c))
                && unit(material.metallic)
                && unit(material.roughness)
                && material.clearcoat.is_none_or(unit)
                && material.transmission.is_none_or(unit)
                && material.ior.is_none_or(|i| (1.0..=4.0).contains(&i))
                && material
                    .emissive
                    .is_none_or(|e| e.iter().all(|c| *c >= 0.0));
            if !valid {
                out.push(block(
                    "model.body_unmapped",
                    format!(
                        "Custom material ‘{}’ needs a name, factors within 0–1 and an index of refraction within 1–4.",
                        material.name
                    ),
                ));
            }
        }
        // Leads.
        let mut lead_keys = BTreeSet::new();
        for lead in &self.leads {
            if lead.key.trim().is_empty() || !lead_keys.insert(lead.key.as_str()) {
                out.push(block(
                    "model.leads",
                    format!("Lead key ‘{}’ is empty or tagged twice.", lead.key),
                ));
            }
            if !printable(&lead.designation) {
                out.push(block(
                    "model.leads",
                    format!("Lead ‘{}’ needs a printable designation.", lead.key),
                ));
            }
            if !inside(lead.contact()) {
                out.push(block(
                    "model.leads",
                    format!(
                        "The contact of lead ‘{}’ lies outside the model bounds.",
                        lead.key
                    ),
                ));
            }
        }
        if self.leads.len() > usize::from(u16::MAX) {
            out.push(block(
                "model.leads",
                "A lead map indexes at most 65535 leads.".into(),
            ));
        }
        // Licence and provenance.
        let licence = self.provenance.record.licence.as_deref().map(str::trim);
        if licence.is_none_or(str::is_empty) {
            out.push(block(
                "model.licence",
                "The model needs its licence stated.".into(),
            ));
        }
        if self.provenance.redistribution == Redistribution::Unknown {
            out.push(Finding::new(
                "model.licence",
                "Whether the model may be redistributed is unknown; it cannot be promoted or shared until stated.".into(),
                this(),
            ));
        }
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
/// packed-asset limits) and the wizard's definition. Creates a Model draft
/// and, with `assign`, uses it in a package draft in the same undo step.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImportModel {
    pub name: String,
    pub assets: Vec<super::PackedAsset>,
    pub definition: Box<ModelDefinition>,
    /// The package draft that uses the new model, with its explicit lead
    /// map, as `library.edit_package { SetModel }` would set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assign: Option<AssignModel>,
}

/// Where `library.import_model` assigns its new model.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssignModel {
    /// A package draft.
    pub package: DefinitionId,
    pub lead_map: Vec<LeadMapEntry>,
    /// The new model is a part file (format `solidcraft`) the package uses
    /// at these values (`PackageModel::PartFile`); the lead map is then
    /// empty (docs/part-templates.md §11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<AssignPart>,
}

/// The part-file use `library.import_model` assigns: values and what the
/// host evaluated them to.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssignPart {
    #[serde(default)]
    pub values: std::collections::BTreeMap<String, super::package_model::PartValue>,
    pub derived: super::package_model::PartDerived,
}

/// Findings of a package that uses a library model and that need only the
/// package: the lead map names each lead and each terminal of the package
/// at most once (`model.leads`). [`library_findings`] adds the rules that
/// need the model revision.
pub fn package_findings(
    package: &PackageDefinition,
    model: &LibraryModel,
    id: DefinitionId,
    out: &mut Vec<Finding>,
) {
    let this = || vec![ObjectRef::Definition { id }];
    let mut leads = BTreeSet::new();
    let mut terminals = BTreeSet::new();
    for entry in &model.lead_map {
        if !leads.insert(entry.lead) {
            out.push(Finding::blocking(
                "model.leads",
                format!("Model lead {} is mapped more than once.", entry.lead),
                this(),
                Stage::Publish,
            ));
        }
        if package.terminal(entry.terminal).is_none() {
            out.push(Finding::blocking(
                "model.leads",
                format!(
                    "The lead map names terminal {}, which the package does not have.",
                    entry.terminal
                ),
                this(),
                Stage::Publish,
            ));
        } else if !terminals.insert(entry.terminal) {
            out.push(Finding::blocking(
                "model.leads",
                format!(
                    "Terminal {} takes more than one model lead.",
                    entry.terminal
                ),
                this(),
                Stage::Publish,
            ));
        }
    }
}

/// Findings of a package's library model that need the model revision:
/// the revision exists (`reference.missing`), every lead index names a lead
/// tag (`model.leads`, blocking), every terminal has a lead when the model
/// tags leads (`model.leads`, advisory: fit and pin-1 checks skip the
/// rest), and the model's bounds exceed the envelope by at most
/// [`BOUNDS_TOLERANCE_MM`] in each axis (`model.bounds`, advisory).
pub fn library_findings(
    library: &Library,
    package: &PackageDefinition,
    model: &LibraryModel,
    id: DefinitionId,
    out: &mut Vec<Finding>,
) {
    let this = || vec![ObjectRef::Definition { id }];
    let Some(published) = library.model(model.model) else {
        out.push(Finding::blocking(
            "reference.missing",
            format!(
                "{} is missing from the project library.",
                super::names::capitalised(&library.describe(model.model.erased()))
            ),
            vec![
                ObjectRef::Definition { id },
                ObjectRef::Definition { id: model.model.id },
            ],
            Stage::Publish,
        ));
        return;
    };
    let definition = &published.definition;
    for entry in &model.lead_map {
        if usize::from(entry.lead) >= definition.leads.len() {
            out.push(Finding::blocking(
                "model.leads",
                format!(
                    "The lead map names lead {}, but ‘{}’ tags {} leads.",
                    entry.lead,
                    published.meta.name,
                    definition.leads.len()
                ),
                this(),
                Stage::Publish,
            ));
        }
    }
    if !definition.leads.is_empty() {
        let unmapped = package
            .terminals
            .iter()
            .filter(|t| model.lead_map.iter().all(|e| e.terminal != t.id))
            .count();
        if unmapped > 0 {
            out.push(Finding::new(
                "model.leads",
                format!(
                    "{unmapped} package terminal(s) have no model lead; fitting and pin-1 checks skip them."
                ),
                this(),
            ));
        }
    }
    let excess = bounds_excess(definition, package.envelope());
    if excess > BOUNDS_TOLERANCE_MM {
        let size = definition.size_mm();
        out.push(Finding::new(
            "model.bounds",
            format!(
                "The model ({:.3} × {:.3} × {:.3} mm) exceeds the package envelope by {excess:.3} mm.",
                size[0], size[1], size[2]
            ),
            this(),
        ));
    }
}

/// Findings of a package's part file that need the model revision: the
/// revision exists (`reference.missing`) and is a Solidcraft design
/// (`part_file.model`, blocking), and the derived bounds exceed the envelope by
/// at most [`BOUNDS_TOLERANCE_MM`] (`model.bounds`, advisory).
pub fn part_findings(
    library: &Library,
    package: &PackageDefinition,
    part: &super::package_model::PartFileModel,
    id: DefinitionId,
    out: &mut Vec<Finding>,
) {
    let this = || vec![ObjectRef::Definition { id }];
    let Some(published) = library.model(part.model) else {
        out.push(Finding::blocking(
            "reference.missing",
            format!(
                "{} is missing from the project library.",
                super::names::capitalised(&library.describe(part.model.erased()))
            ),
            vec![
                ObjectRef::Definition { id },
                ObjectRef::Definition { id: part.model.id },
            ],
            Stage::Publish,
        ));
        return;
    };
    if published.definition.format != ModelFormat::Solidcraft {
        out.push(Finding::blocking(
            "part_file.model",
            format!(
                "‘{}’ is a {} model; a part file is a SolidCraft design.",
                published.meta.name,
                published.definition.format.label()
            ),
            this(),
            Stage::Publish,
        ));
    }
    let [lo, hi] = part.derived.bounds();
    let excess = (0..3)
        .map(|i| (hi[i] - lo[i]) - package.envelope()[i])
        .fold(f64::NEG_INFINITY, f64::max);
    if excess > BOUNDS_TOLERANCE_MM {
        out.push(Finding::new(
            "model.bounds",
            format!(
                "The part ({:.3} × {:.3} × {:.3} mm) exceeds the package envelope by {excess:.3} mm.",
                hi[0] - lo[0],
                hi[1] - lo[1],
                hi[2] - lo[2]
            ),
            this(),
        ));
    }
}

/// The largest excess of a model's bounds over an envelope, mm (negative:
/// inside in every axis). Both are in the model frame.
pub fn bounds_excess(model: &ModelDefinition, envelope: [f64; 3]) -> f64 {
    let size = model.size_mm();
    (0..3)
        .map(|i| size[i] - envelope[i])
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Snapshot rules of models (`Library::validate`): every model's assets
/// are packed with the size and media type it records, and every package's
/// library model passes the blocking [`library_findings`]. A mismatch
/// refuses the document and names the asset.
pub fn snapshot_problems(library: &Library) -> Result<()> {
    for model in &library.models {
        for asset in model.definition.assets() {
            let packed = library.assets.iter().find(|a| a.hash == asset.hash);
            let fits = packed.is_some_and(|p| {
                u64::try_from(p.data.len()).is_ok_and(|n| n == asset.bytes)
                    && p.media_type == asset.media_type
            });
            if !fits {
                return Err(Error::Invalid(format!(
                    "model.asset: ‘{}’ of 3D model ‘{}’ {} is {} the project",
                    asset.file_name,
                    model.meta.name,
                    model.revision,
                    if packed.is_some() {
                        "packed with another size or media type in"
                    } else {
                        "not packed in"
                    }
                )));
            }
        }
    }
    for package in &library.packages {
        let mut found = Vec::new();
        match &package.definition.model {
            PackageModel::Library(model) => {
                library_findings(library, &package.definition, model, package.id, &mut found);
            }
            PackageModel::PartFile(part) => {
                part_findings(library, &package.definition, part, package.id, &mut found);
            }
            _ => continue,
        }
        if let Some(blocking) = found.iter().find(|f| f.blocks_stage(Stage::Publish)) {
            return Err(Error::Invalid(format!(
                "{}: package ‘{}’ {}: {}",
                blocking.code, package.meta.name, package.revision, blocking.message
            )));
        }
    }
    Ok(())
}

/// Assets one `library.import_model` stages (the model and its pair).
pub const MAX_IMPORT_ASSETS: usize = 2;

/// Check `library.import_model`'s request on its own and return the draft
/// it creates: a printable name, the definition within bounds, and each
/// inline asset named by the definition with the size and media type it
/// records (hashes are checked by [`super::bundle::validate_assets`]).
/// Whether named assets the request omits are already packed is
/// [`assets_to_pack`]'s.
pub fn import_model(request: &ImportModel) -> Result<(super::DefinitionMeta, super::Definition)> {
    if !printable(&request.name) {
        return Err(Error::Invalid(
            "model.import: a model needs a printable name of 1–256 bytes".into(),
        ));
    }
    if request.assets.len() > MAX_IMPORT_ASSETS {
        return Err(Error::Limit(format!(
            "model.import: an import stages at most {MAX_IMPORT_ASSETS} assets (the model and its appearance pair)"
        )));
    }
    super::bundle::validate_assets(&request.assets)?;
    let definition = &request.definition;
    for packed in &request.assets {
        let named = definition
            .assets()
            .find(|a| a.hash == packed.hash)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "model.asset: the request stages asset {}, which the model does not name",
                    packed.hash
                ))
            })?;
        if !u64::try_from(packed.data.len()).is_ok_and(|n| n == named.bytes)
            || packed.media_type != named.media_type
        {
            return Err(Error::Invalid(format!(
                "model.asset: ‘{}’ is staged with another size or media type than the model records",
                named.file_name
            )));
        }
    }
    let file = &definition.asset.file_name;
    let meta = super::DefinitionMeta {
        name: request.name.clone(),
        description: String::new(),
        provenance: if file.trim().is_empty() {
            String::new()
        } else {
            format!("Imported from ‘{file}’.")
        },
    };
    let body = super::Definition::Model(definition.clone());
    super::check_bounds(&meta, &body)?;
    Ok((meta, body))
}

/// The inline assets of `request` that `library` does not pack yet, in
/// request order. A named asset the request omits must already be packed
/// with the recorded size and media type.
pub fn assets_to_pack(
    library: &super::Library,
    request: &ImportModel,
) -> Result<Vec<super::PackedAsset>> {
    for named in request.definition.assets() {
        if request.assets.iter().any(|a| a.hash == named.hash) {
            continue;
        }
        let held = library.assets.iter().find(|a| a.hash == named.hash);
        let fits = held.is_some_and(|p| {
            u64::try_from(p.data.len()).is_ok_and(|n| n == named.bytes)
                && p.media_type == named.media_type
        });
        if !fits {
            return Err(Error::Invalid(format!(
                "model.asset: ‘{}’ ({}) is neither staged nor packed in the project",
                named.file_name, named.hash
            )));
        }
    }
    let mut fresh = Vec::new();
    for staged in &request.assets {
        match library.assets.iter().find(|held| held.hash == staged.hash) {
            None => fresh.push(staged.clone()),
            Some(held) if held.media_type != staged.media_type => {
                return Err(Error::Invalid(format!(
                    "model.asset: {} is already packed as {}, not {}",
                    staged.hash, held.media_type, staged.media_type
                )));
            }
            Some(_) => {}
        }
    }
    Ok(fresh)
}

#[cfg(test)]
pub(crate) mod tests;
