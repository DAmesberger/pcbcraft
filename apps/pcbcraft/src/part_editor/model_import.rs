//! Slice D3: the 3D model import wizard. Import model file… picks a STEP,
//! GLB or `.solidcraft` file; reading (bounded) and inspection run off the
//! UI thread through the integration crate. Six steps follow in the 3D
//! tab's card: File, Frame, Materials, Leads, Licence and Review. Every
//! mapping is explicit: suggestions (materials from the file's colours,
//! designations from the nearest pads) fill a field only when the person
//! asks, and an unmapped body or an unnamed lead blocks the step that owns
//! it. Finish sends `library.import_model` (the bytes inline, one undo
//! step) and, once the Model draft exists, one `library.edit_package
//! { SetModel }` naming its pending revision with the lead map previewed in
//! Review. File reading stays outside undo.
use super::PartEditor;
use crate::kit;
use crate::kit::severity::Level;
use crate::refusal::{self, Origin};
use egui::Ui;
use pcbcraft_core::library::MAX_LABEL;
use pcbcraft_core::library::bundle::{AssetBytes, asset_hash};
use pcbcraft_core::library::model3d::{
    self, AssetRef, AssignModel, CustomMaterial, ImportModel, LeadMapEntry, LeadTag, MarkingArea,
    ModelBody, ModelDefinition, ModelFormat, ModelHints, ModelMaterial, ModelProvenance, Mount,
    Redistribution, SourceFrame, UpAxis, triple_nm,
};
use pcbcraft_core::library::{
    Basis, BodyRole, Definition, FootprintDefinition, PackageDefinition, PackedAsset, Provenance,
};
use pcbcraft_core::materials::MaterialId;
use pcbcraft_core::units::Nm;
use pcbcraft_core::{DefinitionId, Engine, LibraryCommand, Operation};
use pcbcraft_solidcraft::models::imported;
use pcbcraft_solidcraft::models::inspect::{self, InspectedBody, ModelInspection};
use pcbcraft_ui::ui_kit::{self, Button};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Bodies the list shows before "Show all".
const LIST_ROWS: usize = 8;
/// Millimetres per source unit the frame accepts.
const UNIT_MM: std::ops::RangeInclusive<f64> = 1e-6..=1e6;
/// Bytes of a designation (a package terminal's designation is a label).
const MAX_DESIGNATION: usize = 64;

/// The wizard's steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    File,
    Frame,
    Materials,
    Leads,
    Licence,
    Review,
}

impl Step {
    pub const ALL: [Self; 6] = [
        Self::File,
        Self::Frame,
        Self::Materials,
        Self::Leads,
        Self::Licence,
        Self::Review,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Frame => "Frame",
            Self::Materials => "Materials",
            Self::Leads => "Leads",
            Self::Licence => "Licence",
            Self::Review => "Review",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    fn back(self) -> Option<Self> {
        self.index()
            .checked_sub(1)
            .and_then(|i| Self::ALL.get(i).copied())
    }
}

/// The format a file name names, by extension.
pub fn format_of(path: &Path) -> Option<ModelFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "step" | "stp" => Some(ModelFormat::Step),
        "glb" | "gltf" => Some(ModelFormat::Gltf),
        "solidcraft" => Some(ModelFormat::Solidcraft),
        _ => None,
    }
}

/// The packed asset's media type: the format's canonical one
/// ([`ModelFormat::media_type`]), `model/gltf+json` for a `.gltf` file.
pub fn media_type(format: ModelFormat, file_name: &str) -> &'static str {
    match format {
        ModelFormat::Gltf if file_name.to_ascii_lowercase().ends_with(".gltf") => "model/gltf+json",
        format => format.media_type(),
    }
}

/// A body's estimated contact: the bottom centre of its framed bounds,
/// shown while leads are named and used to propose designations. Finish
/// replaces it with the integration crate's measured contact
/// ([`imported::lead_contacts`]), which a build rechecks.
pub fn contact(
    frame: &SourceFrame,
    format: ModelFormat,
    bounds: [[f64; 3]; 2],
) -> Option<[f64; 3]> {
    let b = imported::frame_bounds(frame, format, bounds).ok()?;
    Some([
        (b[0][0] + b[1][0]) / 2.0,
        (b[0][1] + b[1][1]) / 2.0,
        b[0][2],
    ])
}

/// Measure every lead tag's contact on the file's geometry, as a build
/// does, so the recorded contacts are the ones a build rechecks. Runs off
/// the UI thread. `.solidcraft` models are not built yet; their estimates
/// stay.
pub fn measure(mut request: ImportModel) -> Result<ImportModel, String> {
    let definition = &mut request.definition;
    if definition.leads.is_empty() || definition.format == ModelFormat::Solidcraft {
        return Ok(request);
    }
    let bytes = request
        .assets
        .first()
        .map(|a| a.data.as_slice())
        .ok_or("The import carries no file")?;
    let contacts = imported::lead_contacts(definition, bytes)
        .map_err(|e| format!("Measuring the lead contacts failed: {e}"))?;
    if contacts.len() != definition.leads.len() {
        return Err("Measuring the lead contacts gave a different lead count".into());
    }
    for (lead, at) in definition.leads.iter_mut().zip(contacts) {
        lead.contact_nm = triple_nm(at).map_err(|e| e.to_string())?;
    }
    Ok(request)
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The built-in material nearest a body's source colour (sRGB, as STEP and
/// display colours are): a suggestion only.
pub fn nearest_builtin(
    colour: [f64; 4],
    materials: &[pcbcraft_core::materials::MaterialDef],
) -> Option<MaterialId> {
    let linear = [
        srgb_to_linear(colour[0]),
        srgb_to_linear(colour[1]),
        srgb_to_linear(colour[2]),
    ];
    materials
        .iter()
        .map(|m| {
            let d: f64 = (0..3).map(|i| (m.base_linear[i] - linear[i]).powi(2)).sum();
            (d, m.id)
        })
        .filter(|(d, _)| d.is_finite())
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

/// One body's mapping while the wizard runs; `None` is unmapped.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyChoice {
    pub source: InspectedBody,
    pub role: Option<BodyRole>,
    pub material: Option<ModelMaterial>,
    pub designation: String,
}

/// What the person states about the model's origin and use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub name: String,
    pub vendor: String,
    pub url: String,
    pub retrieved: String,
    pub licence: String,
    pub redistribution: Redistribution,
    pub mount: Option<Mount>,
    pub family: String,
}

/// A read file and its inspection, going through the steps. The
/// inspection is in the reader's frame ([`imported::reader_frame`]); the
/// wizard's frame moves its bounds with [`imported::frame_bounds`], the
/// same transform a build applies.
pub struct Wizard {
    pub file_name: String,
    pub bytes: Arc<Vec<u8>>,
    pub format: ModelFormat,
    pub inspection: ModelInspection,
    pub step: Step,
    pub frame: SourceFrame,
    pub bodies: Vec<BodyChoice>,
    pub selected: usize,
    pub show_all: bool,
    pub marking_on_top: bool,
    pub meta: Metadata,
}

/// Whether `date` reads as an ISO date (YYYY-MM-DD).
fn iso_date(date: &str) -> bool {
    let b = date.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

impl Wizard {
    pub fn new(
        file_name: String,
        bytes: Arc<Vec<u8>>,
        format: ModelFormat,
        inspection: ModelInspection,
    ) -> Self {
        let name = Path::new(&file_name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bodies = inspection
            .bodies
            .iter()
            .map(|b| BodyChoice {
                source: b.clone(),
                role: None,
                material: None,
                designation: String::new(),
            })
            .collect();
        Self {
            file_name,
            bytes,
            format,
            inspection,
            step: Step::File,
            frame: imported::default_frame(format),
            bodies,
            selected: 0,
            show_all: false,
            marking_on_top: false,
            meta: Metadata {
                name,
                ..Metadata::default()
            },
        }
    }

    /// The model's bounds in the wizard's frame; refused while the frame
    /// is out of range.
    pub fn bounds(&self) -> Result<[[f64; 3]; 2], String> {
        imported::frame_bounds(&self.frame, self.format, self.inspection.bounds_mm)
            .map_err(|e| e.to_string())
    }

    /// Seat the model on the board: its lowest point at z = 0.
    pub fn seat(&mut self) {
        let Ok(bounds) = self.bounds() else {
            return;
        };
        let low = bounds[0][2];
        if low.is_finite() {
            self.set_offset(2, self.frame.origin_offset()[2] - low);
        }
    }

    /// Set one axis of the frame's origin offset, mm, through the one
    /// crossing; a value it refuses leaves the offset as it is.
    fn set_offset(&mut self, axis: usize, mm: f64) {
        if let (Some(slot), Ok(nm)) = (
            self.frame.origin_offset_nm.get_mut(axis),
            Nm::from_mm_f64_rounded(mm),
        ) {
            *slot = nm;
        }
    }

    /// Centre the model's bounds on the origin in x and y.
    pub fn centre(&mut self) {
        let Ok([low, high]) = self.bounds() else {
            return;
        };
        let offset = self.frame.origin_offset();
        for axis in 0..2 {
            let mid = (low[axis] + high[axis]) / 2.0;
            if mid.is_finite() {
                self.set_offset(axis, offset[axis] - mid);
            }
        }
    }

    /// The suggested mapping of one body: a glTF material as itself, a
    /// metallic one or a metal-coloured body as a lead.
    pub fn suggestion(
        &self,
        body: &InspectedBody,
        builtins: &[pcbcraft_core::materials::MaterialDef],
    ) -> (BodyRole, ModelMaterial) {
        if let Some(index) = body.material
            && let Some(material) = self.inspection.materials.get(index)
            && let Ok(custom) = u16::try_from(index)
        {
            let role = if material.metallic >= 0.5 {
                BodyRole::Lead
            } else {
                BodyRole::Body
            };
            return (role, ModelMaterial::Custom(custom));
        }
        let id = body
            .colour
            .and_then(|c| nearest_builtin(c, builtins))
            .unwrap_or(MaterialId::EpoxyMould);
        let role = if id.category() == pcbcraft_core::materials::MaterialCategory::Metal {
            BodyRole::Lead
        } else {
            BodyRole::Body
        };
        (role, ModelMaterial::Builtin(id))
    }

    /// Fill every unmapped role and material with its suggestion; mapped
    /// ones stay. Returns how many fields changed.
    pub fn use_suggestions(&mut self, builtins: &[pcbcraft_core::materials::MaterialDef]) -> usize {
        let suggestions: Vec<_> = self
            .bodies
            .iter()
            .map(|b| self.suggestion(&b.source, builtins))
            .collect();
        let mut changed = 0;
        for (body, (role, material)) in self.bodies.iter_mut().zip(suggestions) {
            if body.role.is_none() {
                body.role = Some(role);
                changed += 1;
            }
            if body.material.is_none() {
                body.material = Some(material);
                changed += 1;
            }
        }
        changed
    }

    fn leads(&self) -> impl Iterator<Item = (usize, &BodyChoice)> {
        self.bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.role == Some(BodyRole::Lead))
    }

    /// Propose each unnamed lead's designation from the nearest land pad
    /// of the footprint (in its datum frame); named leads stay. Returns
    /// how many were proposed.
    pub fn propose_from_pads(
        &mut self,
        footprint: &FootprintDefinition,
        package: &PackageDefinition,
    ) -> usize {
        let pads: Vec<([f64; 2], String)> = footprint
            .pads
            .iter()
            .filter_map(|pad| {
                let terminal = package.terminal(pad.terminal?)?;
                let at =
                    pcbcraft_core::library::package_fit::to_datum(&footprint.datum, pad.position());
                Some((at, terminal.designation.clone()))
            })
            .collect();
        let (frame, format) = (self.frame, self.format);
        let mut proposed = 0;
        for body in self
            .bodies
            .iter_mut()
            .filter(|b| b.role == Some(BodyRole::Lead) && b.designation.trim().is_empty())
        {
            let Some(c) = contact(&frame, format, body.source.bounds_mm) else {
                continue;
            };
            let nearest = pads
                .iter()
                .map(|(p, d)| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2), d))
                .filter(|(d, _)| d.is_finite())
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, designation)) = nearest {
                body.designation = designation.clone();
                proposed += 1;
            }
        }
        proposed
    }

    /// Why `step` cannot be left forward, in shown wording.
    pub fn problem(&self, step: Step) -> Option<String> {
        match step {
            Step::File => self
                .inspection
                .bodies
                .is_empty()
                .then(|| "The file has no bodies".into()),
            Step::Frame => {
                let ok = self.frame.unit_mm.is_finite()
                    && UNIT_MM.contains(&self.frame.unit_mm)
                    && self.frame.origin_offset().iter().all(|v| v.is_finite())
                    && self.bounds().is_ok();
                (!ok).then(|| "The unit and offset must be finite numbers in range".into())
            }
            Step::Materials => {
                let unmapped = self
                    .bodies
                    .iter()
                    .filter(|b| b.role.is_none() || b.material.is_none())
                    .count();
                if self.inspection.materials.len()
                    > pcbcraft_core::library::model3d::MAX_CUSTOM_MATERIALS
                {
                    return Some("The file has more materials than a model may carry".into());
                }
                match unmapped {
                    0 => None,
                    1 => Some("1 body has no role or material".into()),
                    n => Some(format!("{n} bodies have no role or material")),
                }
            }
            Step::Leads => {
                let unnamed = self
                    .leads()
                    .filter(|(_, b)| b.designation.trim().is_empty())
                    .count();
                if self
                    .leads()
                    .any(|(_, b)| b.designation.len() > MAX_DESIGNATION)
                {
                    return Some("A designation is longer than 64 bytes".into());
                }
                match unnamed {
                    0 => None,
                    1 => Some("1 lead has no designation".into()),
                    n => Some(format!("{n} leads have no designation")),
                }
            }
            Step::Licence => {
                let m = &self.meta;
                if m.name.trim().is_empty() {
                    Some("Name the model".into())
                } else if m.licence.trim().is_empty() {
                    Some("State the model's licence".into())
                } else if !m.retrieved.trim().is_empty() && !iso_date(m.retrieved.trim()) {
                    Some("Write the retrieval date as YYYY-MM-DD".into())
                } else if [&m.name, &m.vendor, &m.licence, &m.family]
                    .iter()
                    .any(|t| t.len() > MAX_LABEL)
                    || m.url.len() > pcbcraft_core::library::MAX_URL
                {
                    Some("A field is longer than its limit".into())
                } else {
                    None
                }
            }
            Step::Review => Step::ALL
                .iter()
                .take_while(|s| **s != Step::Review)
                .find_map(|s| self.problem(*s)),
        }
    }

    /// The model definition the steps describe; refused while a step has
    /// a problem.
    pub fn definition(&self) -> Result<ModelDefinition, String> {
        if let Some(problem) = self.problem(Step::Review) {
            return Err(problem);
        }
        let bodies = self
            .bodies
            .iter()
            .map(|b| match (b.role, b.material) {
                (Some(role), Some(material)) => Ok(ModelBody {
                    key: b.source.key.clone(),
                    role,
                    material,
                }),
                _ => Err("A body has no role or material".to_owned()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let leads: Vec<LeadTag> = self
            .leads()
            .map(|(_, b)| {
                Ok(LeadTag {
                    key: b.source.key.clone(),
                    designation: b.designation.trim().to_owned(),
                    contact_nm: contact(&self.frame, self.format, b.source.bounds_mm)
                        .and_then(|c| triple_nm(c).ok())
                        .ok_or("The frame is out of range")?,
                })
            })
            .collect::<Result<_, String>>()?;
        let bounds = self.bounds()?;
        let marking_area = self
            .marking_on_top
            .then(|| {
                MarkingArea::from_mm(
                    [
                        (bounds[0][0] + bounds[1][0]) / 2.0,
                        (bounds[0][1] + bounds[1][1]) / 2.0,
                        bounds[1][2],
                    ],
                    [0.0, 0.0, 1.0],
                    [1.0, 0.0, 0.0],
                    [
                        (bounds[1][0] - bounds[0][0]) * 0.8,
                        (bounds[1][1] - bounds[0][1]) * 0.8,
                    ],
                )
            })
            .transpose()
            .map_err(|e| e.to_string())?;
        let text = |t: &str| {
            let t = t.trim();
            (!t.is_empty()).then(|| t.to_owned())
        };
        let m = &self.meta;
        let pins = u32::try_from(leads.len()).ok().filter(|n| *n > 0);
        Ok(ModelDefinition {
            asset: AssetRef {
                hash: asset_hash(&self.bytes),
                bytes: u64::try_from(self.bytes.len()).map_err(|_| "The file is too large")?,
                media_type: media_type(self.format, &self.file_name).into(),
                file_name: self.file_name.clone(),
            },
            pair: None,
            format: self.format,
            frame: self.frame,
            bounds_nm: model3d::bounds_nm(bounds).map_err(|e| e.to_string())?,
            bodies,
            custom_materials: self.inspection.materials.clone(),
            leads,
            marking_area,
            hints: ModelHints {
                mount: m.mount,
                pins,
                pitch_nm: None,
                family: text(&m.family),
            },
            provenance: ModelProvenance {
                vendor: text(&m.vendor),
                url: text(&m.url),
                retrieved: text(&m.retrieved),
                redistribution: m.redistribution,
                record: Provenance {
                    origin: None,
                    basis: Basis::Imported,
                    licence: text(&m.licence),
                },
            },
        })
    }

    /// The `library.import_model` request: the file's bytes inline and
    /// the definition.
    pub fn request(&self) -> Result<ImportModel, String> {
        let definition = self.definition()?;
        Ok(ImportModel {
            name: self.meta.name.trim().to_owned(),
            assets: vec![PackedAsset {
                hash: definition.asset.hash,
                media_type: definition.asset.media_type.clone(),
                data: AssetBytes::new(self.bytes.as_ref().clone()),
            }],
            definition: Box::new(definition),
            assign: None,
        })
    }
}

/// The request assigning its new model to the package draft with the lead
/// map previewed in Review: Finish is one gesture, so import and
/// assignment are one command and one undo step.
pub fn assigning(
    mut request: ImportModel,
    package_id: DefinitionId,
    lead_map: Vec<LeadMapEntry>,
) -> ImportModel {
    request.assign = Some(AssignModel {
        package: package_id,
        lead_map,
        part: None,
    });
    request
}

/// Model drafts in the engine, in draft order.
fn model_drafts(engine: &Engine) -> Vec<DefinitionId> {
    engine
        .drafts()
        .items
        .iter()
        .filter(|d| matches!(d.definition, Definition::Model(_)))
        .map(|d| d.id)
        .collect()
}

/// A read and inspected file.
struct Loaded {
    file_name: String,
    bytes: Arc<Vec<u8>>,
    format: ModelFormat,
    inspection: ModelInspection,
}

fn read(path: &Path) -> Result<Loaded, String> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let format = format_of(path)
        .ok_or_else(|| format!("{file_name} is not a STEP, GLB or .solidcraft file"))?;
    let length = std::fs::metadata(path)
        .map_err(|e| format!("Reading {file_name} failed: {e}"))?
        .len();
    if usize::try_from(length).map_or(true, |n| n > inspect::MAX_INSPECT_BYTES) {
        return Err(format!(
            "{file_name} is larger than {} MiB",
            inspect::MAX_INSPECT_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Reading {file_name} failed: {e}"))?;
    if bytes.len() > inspect::MAX_INSPECT_BYTES {
        return Err(format!("{file_name} grew while it was read"));
    }
    let inspection = inspect::inspect_in(&bytes, format, &imported::reader_frame(format))
        .map_err(|e| format!("{file_name} could not be inspected: {e}"))?;
    Ok(Loaded {
        file_name,
        bytes: Arc::new(bytes),
        format,
        inspection,
    })
}

enum State {
    Idle,
    Picking(Receiver<Option<PathBuf>>),
    Reading(Receiver<Result<Loaded, String>>),
    Steps(Box<Wizard>),
    /// Finish pressed: the lead contacts are being measured off the UI
    /// thread before `library.import_model` is sent.
    Measuring {
        wizard: Box<Wizard>,
        package: DefinitionId,
        /// The package as the Leads step saw it: the lead map's target.
        terminals: Box<PackageDefinition>,
        receiver: Receiver<Result<ImportModel, String>>,
    },
    /// `library.import_model` (with its assignment) was sent; waiting for
    /// its Model draft.
    Importing {
        wizard: Box<Wizard>,
        before: Vec<DefinitionId>,
        revision: u64,
    },
}

/// The wizard's step and staged file.
pub struct ModelImport {
    state: State,
    refusal: Option<String>,
}

impl Default for ModelImport {
    fn default() -> Self {
        Self {
            state: State::Idle,
            refusal: None,
        }
    }
}

fn wizard_id() -> egui::Id {
    egui::Id::unique("model-import")
}

impl ModelImport {
    /// Whether the wizard's steps fill the 3D tab's card.
    pub fn is_open(&self) -> bool {
        matches!(
            self.state,
            State::Steps(_) | State::Measuring { .. } | State::Importing { .. }
        )
    }

    /// Whether a file is being picked or read.
    pub fn busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Ask for a file with the native picker (made here, awaited on a
    /// worker, as the other pickers are).
    pub fn start(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let future = rfd::AsyncFileDialog::new()
            .set_title("Import a 3D model")
            .add_filter("3D model", &["step", "stp", "glb", "gltf", "solidcraft"])
            .pick_file();
        let (sender, receiver) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picked = pollster::block_on(future).map(|h| h.path().to_path_buf());
            let _ = sender.send(picked);
            ctx.request_repaint();
        });
        self.refusal = None;
        self.state = State::Picking(receiver);
    }

    /// Open the steps on an inspected file (also the tests' entry).
    pub fn open(&mut self, wizard: Wizard) {
        self.refusal = None;
        self.state = State::Steps(Box::new(wizard));
    }

    #[cfg(test)]
    pub fn wizard(&self) -> Option<&Wizard> {
        match &self.state {
            State::Steps(w)
            | State::Measuring { wizard: w, .. }
            | State::Importing { wizard: w, .. } => Some(w),
            _ => None,
        }
    }

    /// Advance picking and reading; a failure is a toast.
    fn poll(&mut self, ctx: &egui::Context) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Picking(picked) => match picked.try_recv() {
                Ok(Some(path)) => {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    let repaint = ctx.clone();
                    std::thread::spawn(move || {
                        let _ = sender.send(read(&path));
                        repaint.request_repaint();
                    });
                    self.state = State::Reading(receiver);
                }
                Ok(None) => {}
                Err(TryRecvError::Empty) => self.state = State::Picking(picked),
                Err(TryRecvError::Disconnected) => {
                    kit::toast::post(
                        ctx,
                        Level::Error,
                        "The file picker closed unexpectedly",
                        Vec::new(),
                    );
                }
            },
            State::Reading(reading) => match reading.try_recv() {
                Ok(Ok(loaded)) => self.open(Wizard::new(
                    loaded.file_name,
                    loaded.bytes,
                    loaded.format,
                    loaded.inspection,
                )),
                Ok(Err(error)) => kit::toast::post(ctx, Level::Error, error, Vec::new()),
                Err(TryRecvError::Empty) => self.state = State::Reading(reading),
                Err(TryRecvError::Disconnected) => {
                    kit::toast::post(
                        ctx,
                        Level::Error,
                        "Reading the model stopped unexpectedly",
                        Vec::new(),
                    );
                }
            },
            other => self.state = other,
        }
    }

    /// Send `library.import_model` once the lead contacts are measured; a
    /// failure returns to Review with the reason shown.
    fn measured(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        let State::Measuring { receiver, .. } = &self.state else {
            return None;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Err("Measuring the lead contacts stopped unexpectedly".into())
            }
        };
        let State::Measuring {
            wizard,
            package,
            terminals,
            ..
        } = std::mem::replace(&mut self.state, State::Idle)
        else {
            return None;
        };
        match result {
            Ok(request) => {
                let lead_map = lead_map_of(&request.definition, &terminals);
                self.state = State::Importing {
                    wizard,
                    before: model_drafts(engine),
                    revision: engine.revision(),
                };
                self.refusal = None;
                refusal::mark(ctx, Origin::Dialog(wizard_id()));
                Some(Operation::Library(LibraryCommand::ImportModel(Box::new(
                    assigning(request, package, lead_map),
                ))))
            }
            Err(why) => {
                self.refusal = Some(why);
                self.state = State::Steps(wizard);
                None
            }
        }
    }

    /// Wait for the Model draft `library.import_model` creates (already
    /// assigned to the package) and close; a refusal returns to Review
    /// with the refusal shown.
    fn settle(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        let State::Importing { revision, .. } = &self.state else {
            return None;
        };
        let refused = refusal::claim(ctx, Origin::Dialog(wizard_id()));
        let changed = engine.revision() != *revision;
        if refused.is_none() && !changed {
            return None;
        }
        let State::Importing { wizard, before, .. } =
            std::mem::replace(&mut self.state, State::Idle)
        else {
            return None;
        };
        let created = model_drafts(engine)
            .into_iter()
            .any(|id| !before.contains(&id));
        match (refused, created) {
            (None, true) => {
                kit::toast::post(
                    ctx,
                    Level::Info,
                    format!(
                        "Imported {}; fit its leads under Fit leads to pads",
                        wizard.meta.name.trim()
                    ),
                    Vec::new(),
                );
                self.refusal = None;
                None
            }
            (refused, _) => {
                self.refusal =
                    Some(refused.unwrap_or_else(|| "The import created no model draft".into()));
                self.state = State::Steps(wizard);
                None
            }
        }
    }
}

impl PartEditor {
    /// The docked wizard steps while an import runs: draws nothing when no
    /// import is open.
    pub(super) fn import_wizard(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let ctx = ui.ctx().clone();
        if std::mem::take(&mut self.model_chooser.import) {
            self.model_import.start(&ctx);
        }
        self.model_import.poll(&ctx);
        if let Some(operation) = self.model_import.settle(&ctx, engine) {
            return Some(operation);
        }
        if let Some(operation) = self.model_import.measured(&ctx, engine) {
            return Some(operation);
        }
        let set = self.draft.as_ref()?;
        let package_id = set.ids.package;
        let package = set.package.clone();
        let footprint = set.footprint.clone();
        let import = &mut self.model_import;
        let importing = matches!(
            import.state,
            State::Measuring { .. } | State::Importing { .. }
        );
        let (State::Steps(wizard)
        | State::Measuring { wizard, .. }
        | State::Importing { wizard, .. }) = &mut import.state
        else {
            return None;
        };
        let action = steps(
            ui,
            wizard,
            &package,
            &footprint,
            import.refusal.as_deref(),
            importing,
        );
        match action {
            Some(Action::Cancel) => {
                import.state = State::Idle;
                import.refusal = None;
                None
            }
            Some(Action::Finish) if !importing => match wizard.request() {
                Ok(request) => {
                    let State::Steps(wizard) = std::mem::replace(&mut import.state, State::Idle)
                    else {
                        return None;
                    };
                    let (sender, receiver) = std::sync::mpsc::channel();
                    let repaint = ctx.clone();
                    std::thread::spawn(move || {
                        let _ = sender.send(measure(request));
                        repaint.request_repaint();
                    });
                    import.state = State::Measuring {
                        wizard,
                        package: package_id,
                        terminals: Box::new(package.clone()),
                        receiver,
                    };
                    import.refusal = None;
                    None
                }
                Err(why) => {
                    import.refusal = Some(why);
                    None
                }
            },
            Some(Action::Finish) | None => None,
        }
    }
}

fn lead_map_of(model: &ModelDefinition, package: &PackageDefinition) -> Vec<LeadMapEntry> {
    crate::model_library::lead_map_by_designation(model, package)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Cancel,
    Finish,
}

fn role_label(role: BodyRole) -> &'static str {
    match role {
        BodyRole::Body => "Body",
        BodyRole::Lead => "Lead",
        BodyRole::Termination => "Termination",
        BodyRole::ExposedPad => "Exposed pad",
        BodyRole::Lens => "Lens",
        BodyRole::Sleeve => "Sleeve",
        BodyRole::Marking => "Marking",
        BodyRole::Other => "Other",
    }
}

fn material_label(
    material: ModelMaterial,
    builtins: &[pcbcraft_core::materials::MaterialDef],
    customs: &[CustomMaterial],
) -> String {
    match material {
        ModelMaterial::Builtin(id) => builtins
            .iter()
            .find(|m| m.id == id)
            .map_or_else(|| format!("{id:?}"), |m| m.name.clone()),
        ModelMaterial::Custom(index) => customs.get(usize::from(index)).map_or_else(
            || "Missing file material".into(),
            |m| format!("{} (file)", m.name),
        ),
    }
}

fn size_line(b: [[f64; 3]; 2]) -> String {
    format!(
        "{:.2} × {:.2} × {:.2} mm",
        b[1][0] - b[0][0],
        b[1][1] - b[0][1],
        b[1][2] - b[0][2]
    )
}

/// The current step's rows and the wizard's actions.
fn steps(
    ui: &mut Ui,
    wizard: &mut Wizard,
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
    refusal: Option<&str>,
    importing: bool,
) -> Option<Action> {
    let builtins = pcbcraft_core::materials::all().unwrap_or_default();
    let step = wizard.step;
    let title = format!(
        "Import model · {} of {} · {}",
        step.index() + 1,
        Step::ALL.len(),
        step.label()
    );
    let mut action = None;
    kit::section(ui, "model-import", &title, true, None, |ui| {
        ui.horizontal_wrapped(|ui| {
            for s in Step::ALL {
                let look = if s == step {
                    Button::SmallSelected
                } else {
                    Button::Small
                };
                // Earlier steps can be revisited; later ones open in order.
                let reachable = s.index() <= step.index();
                let response = ui_kit::button(
                    ui,
                    s.label(),
                    if reachable {
                        look
                    } else {
                        Button::Enabled(false)
                    },
                );
                if reachable && response.clicked() {
                    wizard.step = s;
                }
            }
        });
        match step {
            Step::File => file_rows(ui, wizard),
            Step::Frame => frame_rows(ui, wizard),
            Step::Materials => material_rows(ui, wizard, &builtins),
            Step::Leads => lead_rows(ui, wizard, package, footprint),
            Step::Licence => licence_rows(ui, wizard),
            Step::Review => review_rows(ui, wizard, package, &builtins),
        }
        let problem = wizard.problem(step);
        if let (Some(refusal), Step::Review) = (refusal, step) {
            kit::refusal(ui, refusal);
        }
        kit::actions(ui, |ui| {
            let last = step == Step::Review;
            let label = if last { "Import model" } else { "Next" };
            let why = if importing {
                Some("Importing…".to_owned())
            } else {
                problem.clone()
            };
            let response = kit::primary(ui, label, why.is_none());
            let response = match &why {
                Some(why) => response.on_disabled_hover_text(why),
                None => response,
            };
            if response.clicked() && why.is_none() {
                match step.next() {
                    Some(next) => wizard.step = next,
                    None => action = Some(Action::Finish),
                }
            }
            if let Some(back) = step.back()
                && ui_kit::button(ui, "Back", Button::Enabled(!importing)).clicked()
            {
                wizard.step = back;
            }
            if !importing && ui_kit::button(ui, "Cancel", Button::Ghost).clicked() {
                action = Some(Action::Cancel);
            }
        });
        if let Some(problem) = &problem {
            kit::hint(ui, problem, "Finish this step to go on");
        }
    });
    action
}

fn file_rows(ui: &mut Ui, wizard: &Wizard) {
    kit::value_row(ui, "File", &wizard.file_name);
    kit::value_row(
        ui,
        "Format",
        crate::model_library::format_label(wizard.format),
    );
    kit::value_row(
        ui,
        "Size",
        &format!("{:.1} KiB", wizard.bytes.len() as f64 / 1024.0),
    );
    kit::value_row(ui, "Bodies", &wizard.inspection.bodies.len().to_string());
    kit::value_row(
        ui,
        "Materials",
        &wizard.inspection.materials.len().to_string(),
    );
    kit::value_row(ui, "Source bounds", &size_line(wizard.inspection.bounds_mm));
    if wizard.inspection.has_textures {
        kit::hint(
            ui,
            "Textures are not shown yet",
            "The model shows its material colours and finishes; image textures arrive later",
        );
    }
}

fn frame_rows(ui: &mut Ui, wizard: &mut Wizard) {
    kit::number_row(ui, "Unit", &mut wizard.frame.unit_mm, "mm", UNIT_MM);
    let options = [
        (UpAxis::Z, "Z up".to_owned()),
        (UpAxis::Y, "Y up".to_owned()),
    ];
    kit::choice_row(ui, "Up axis", &mut wizard.frame.up, &options);
    let mut offset = wizard.frame.origin_offset();
    for (axis, label) in ["Offset X", "Offset Y", "Offset Z"].into_iter().enumerate() {
        if kit::number_row(ui, label, &mut offset[axis], "mm", -1000.0..=1000.0) {
            wizard.set_offset(axis, offset[axis]);
        }
    }
    match wizard.bounds() {
        Ok(b) => {
            kit::value_row(ui, "Size", &size_line(b));
            kit::value_row(ui, "Bottom at", &format!("z = {:.3} mm", b[0][2]));
        }
        Err(why) => {
            kit::hint(ui, "The frame is out of range", &why);
        }
    }
    kit::actions(ui, |ui| {
        if ui_kit::button(ui, "Seat on board", Button::Small)
            .on_hover_text("Move the model so its lowest point is on the board (z = 0)")
            .clicked()
        {
            wizard.seat();
        }
        if ui_kit::button(ui, "Centre", Button::Small)
            .on_hover_text("Centre the model's bounds on the footprint origin in X and Y")
            .clicked()
        {
            wizard.centre();
        }
    });
}

/// A bounded, clickable list of bodies; returns the clicked index.
fn body_list(ui: &mut Ui, wizard: &mut Wizard, rows: &[(usize, String)]) {
    let shown = if wizard.show_all {
        rows.len()
    } else {
        LIST_ROWS
    };
    let mut clicked = None;
    let response = kit::list_frame(ui, rows.len(), shown, |ui, i| {
        if let Some((index, text)) = rows.get(i)
            && ui
                .selectable_label(*index == wizard.selected, text.as_str())
                .clicked()
        {
            clicked = Some(*index);
        }
    });
    if response.show_all {
        wizard.show_all = true;
    }
    if let Some(index) = clicked {
        wizard.selected = index;
    }
}

fn material_rows(
    ui: &mut Ui,
    wizard: &mut Wizard,
    builtins: &[pcbcraft_core::materials::MaterialDef],
) {
    let customs = wizard.inspection.materials.clone();
    let rows: Vec<(usize, String)> = wizard
        .bodies
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let mapping = match (b.role, b.material) {
                (Some(role), Some(material)) => format!(
                    "{} · {}",
                    role_label(role),
                    material_label(material, builtins, &customs)
                ),
                _ => "unmapped".into(),
            };
            (i, format!("{} · {mapping}", b.source.key))
        })
        .collect();
    kit::actions(ui, |ui| {
        if ui_kit::button(ui, "Use suggestions", Button::Small)
            .on_hover_text(
                "Fill unmapped roles and materials from the file's colours and materials",
            )
            .clicked()
        {
            wizard.use_suggestions(builtins);
        }
    });
    body_list(ui, wizard, &rows);
    let selected = wizard.selected.min(wizard.bodies.len().saturating_sub(1));
    let suggestion = wizard
        .bodies
        .get(selected)
        .map(|b| wizard.suggestion(&b.source, builtins));
    let Some(body) = wizard.bodies.get_mut(selected) else {
        return;
    };
    kit::value_row(ui, "Body", &body.source.key);
    let roles: Vec<(Option<BodyRole>, String)> = std::iter::once((None, "—".to_owned()))
        .chain(BodyRole::ALL.map(|r| (Some(r), role_label(r).to_owned())))
        .collect();
    kit::choice_row(ui, "Role", &mut body.role, &roles);
    let materials: Vec<(Option<ModelMaterial>, String)> = std::iter::once((None, "—".to_owned()))
        .chain(customs.iter().enumerate().filter_map(|(i, m)| {
            Some((
                Some(ModelMaterial::Custom(u16::try_from(i).ok()?)),
                format!("{} (file)", m.name),
            ))
        }))
        .chain(
            builtins
                .iter()
                .map(|m| (Some(ModelMaterial::Builtin(m.id)), m.name.clone())),
        )
        .collect();
    kit::choice_row(ui, "Material", &mut body.material, &materials);
    if let Some((role, material)) = suggestion {
        kit::hint(
            ui,
            &format!(
                "Suggested: {} · {}",
                role_label(role),
                material_label(material, builtins, &customs)
            ),
            "A suggestion from the file; nothing is mapped until you choose",
        );
    }
}

fn lead_rows(
    ui: &mut Ui,
    wizard: &mut Wizard,
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
) {
    let rows: Vec<(usize, String)> = wizard
        .leads()
        .map(|(i, b)| {
            let name = if b.designation.trim().is_empty() {
                "unnamed".to_owned()
            } else {
                b.designation.clone()
            };
            (i, format!("{} · {name}", b.source.key))
        })
        .collect();
    if rows.is_empty() {
        kit::hint(
            ui,
            "No body has the Lead role",
            "Give lead bodies the Lead role under Materials to tag them",
        );
    } else {
        kit::actions(ui, |ui| {
            if ui_kit::button(ui, "Propose from pads", Button::Small)
                .on_hover_text(
                    "Name each unnamed lead after the nearest land pad's terminal; check them",
                )
                .clicked()
            {
                wizard.propose_from_pads(footprint, package);
            }
        });
        if !rows.iter().any(|(i, _)| *i == wizard.selected)
            && let Some((first, _)) = rows.first()
        {
            wizard.selected = *first;
        }
        body_list(ui, wizard, &rows);
        let (frame, format) = (wizard.frame, wizard.format);
        if let Some(body) = wizard.bodies.get_mut(wizard.selected) {
            kit::value_row(ui, "Lead", &body.source.key);
            let response = kit::text_row(ui, "Designation", &mut body.designation);
            if body.designation.len() > MAX_DESIGNATION {
                kit::field_error(ui, &response, "At most 64 bytes", false);
            }
            if let Some(c) = contact(&frame, format, body.source.bounds_mm) {
                kit::value_row(
                    ui,
                    "Contact (estimate)",
                    &format!("{:.3}, {:.3}, {:.3} mm", c[0], c[1], c[2]),
                );
            }
        }
    }
    kit::check_row(
        ui,
        "Marking area on the top face",
        &mut wizard.marking_on_top,
    );
}

fn licence_rows(ui: &mut Ui, wizard: &mut Wizard) {
    let m = &mut wizard.meta;
    kit::text_row(ui, "Name", &mut m.name);
    kit::text_row(ui, "Vendor", &mut m.vendor);
    kit::text_row(ui, "Source URL", &mut m.url);
    kit::text_row(ui, "Retrieved", &mut m.retrieved);
    kit::text_row(ui, "Licence", &mut m.licence);
    let options = [
        (Redistribution::Unknown, "Unknown".to_owned()),
        (Redistribution::Permitted, "Permitted".to_owned()),
        (Redistribution::Forbidden, "Forbidden".to_owned()),
    ];
    kit::choice_row(ui, "Sharing", &mut m.redistribution, &options);
    if m.redistribution == Redistribution::Forbidden {
        kit::hint(
            ui,
            "This model cannot be promoted or shared",
            "Its licence forbids redistribution; it stays in this project",
        );
    }
    let mounts = [
        (None, "—".to_owned()),
        (Some(Mount::Smd), "SMD".to_owned()),
        (Some(Mount::Tht), "Through-hole".to_owned()),
    ];
    kit::choice_row(ui, "Mount", &mut m.mount, &mounts);
    kit::text_row(ui, "Family", &mut m.family);
}

fn review_rows(
    ui: &mut Ui,
    wizard: &Wizard,
    package: &PackageDefinition,
    builtins: &[pcbcraft_core::materials::MaterialDef],
) {
    let _ = builtins;
    kit::value_row(ui, "Name", wizard.meta.name.trim());
    kit::value_row(ui, "File", &wizard.file_name);
    if let Ok(b) = wizard.bounds() {
        kit::value_row(ui, "Size", &size_line(b));
    }
    let mapped = wizard
        .bodies
        .iter()
        .filter(|b| b.role.is_some() && b.material.is_some())
        .count();
    kit::value_row(
        ui,
        "Bodies",
        &format!("{mapped} of {} mapped", wizard.bodies.len()),
    );
    let leads = wizard.leads().count();
    kit::value_row(ui, "Lead tags", &leads.to_string());
    if let Ok(definition) = wizard.definition() {
        let map = lead_map_of(&definition, package);
        kit::hint(
            ui,
            &crate::model_library::lead_map_line(&map, leads, package.terminals.len()),
            "Each lead whose designation equals a terminal's maps to it; check the rest in Issues",
        );
    }
    kit::value_row(ui, "Licence", wizard.meta.licence.trim());
    kit::value_row(ui, "Save to", "This project");
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::PackageTerminalId;
    use pcbcraft_core::PadId;
    use pcbcraft_core::library::LibraryModel;
    use pcbcraft_core::library::{PackageTerminal, Pad};
    use crate::schematic_editor::{pt};

    /// An original two-lead chip-like file: a body block and two metal
    /// end caps, as the integration crate's inspection reports them.
    fn inspection() -> ModelInspection {
        let body = |key: &str, colour: [f64; 4], b: [[f64; 3]; 2]| InspectedBody {
            key: key.into(),
            colour: Some(colour),
            material: None,
            bounds_mm: b,
        };
        ModelInspection {
            format: ModelFormat::Step,
            bodies: vec![
                body(
                    "body",
                    [0.05, 0.05, 0.05, 1.0],
                    [[-0.8, -0.4, 0.0], [0.8, 0.4, 0.45]],
                ),
                body(
                    "cap-left",
                    [0.8, 0.8, 0.82, 1.0],
                    [[-1.0, -0.4, 0.0], [-0.8, 0.4, 0.45]],
                ),
                body(
                    "cap-right",
                    [0.8, 0.8, 0.82, 1.0],
                    [[0.8, -0.4, 0.0], [1.0, 0.4, 0.45]],
                ),
            ],
            materials: Vec::new(),
            bounds_mm: [[-1.0, -0.4, 0.0], [1.0, 0.4, 0.45]],
            has_textures: false,
            frame: imported::reader_frame(ModelFormat::Step),
            warnings: Vec::new(),
        }
    }

    fn wizard() -> Wizard {
        Wizard::new(
            "chip-0805.step".into(),
            Arc::new(b"ISO-10303-21; original test bytes".to_vec()),
            ModelFormat::Step,
            inspection(),
        )
    }

    fn package() -> (PackageDefinition, FootprintDefinition) {
        let package = PackageDefinition {
            terminals: ["1", "2"]
                .iter()
                .enumerate()
                .map(|(i, d)| PackageTerminal {
                    id: PackageTerminalId(i as u32 + 1),
                    designation: (*d).into(),
                })
                .collect(),
            ..PackageDefinition::default()
        };
        let mut footprint = FootprintDefinition::default();
        for (id, t, x) in [(1, 1, -0.9), (2, 2, 0.9)] {
            footprint.pads.push(Pad::smd_rect(
                PadId(id),
                PackageTerminalId(t),
                t.to_string(),
                pt([x, 0.0]),
                pt([0.6, 0.9]),
            ));
        }
        (package, footprint)
    }

    #[test]
    fn files_are_known_by_extension_and_typed_by_media_type() {
        assert_eq!(format_of(Path::new("a/B.STP")), Some(ModelFormat::Step));
        assert_eq!(format_of(Path::new("x.glb")), Some(ModelFormat::Gltf));
        assert_eq!(
            format_of(Path::new("x.solidcraft")),
            Some(ModelFormat::Solidcraft)
        );
        assert_eq!(format_of(Path::new("x.wrl")), None);
        assert_eq!(format_of(Path::new("noext")), None);
        assert_eq!(media_type(ModelFormat::Gltf, "a.gltf"), "model/gltf+json");
        assert_eq!(media_type(ModelFormat::Gltf, "a.glb"), "model/gltf-binary");
        assert_eq!(media_type(ModelFormat::Step, "a.step"), "model/step");
        for format in [
            ModelFormat::Step,
            ModelFormat::Gltf,
            ModelFormat::Solidcraft,
        ] {
            assert!(
                format.media_types().contains(&media_type(format, "a.x")),
                "{format:?}: the publish rules accept the media type"
            );
        }
    }

    #[test]
    fn the_frame_scales_turns_and_offsets() -> Result<(), Box<dyn std::error::Error>> {
        let inch = SourceFrame {
            unit_mm: 25.4,
            up: UpAxis::Y,
            origin_offset_nm: triple_nm([1.0, 2.0, 3.0])?,
        };
        // Y-up (0, 1, 0) is Z-up (0, 0, 1), then scaled and offset.
        let point = |p| imported::frame_point(&inch, ModelFormat::Step, p);
        assert_eq!(point([0.0, 1.0, 0.0])?, [1.0, 2.0, 28.4]);
        assert_eq!(point([0.0, 0.0, 1.0])?, [1.0, 2.0 - 25.4, 3.0]);
        let b = imported::frame_bounds(&inch, ModelFormat::Step, [[0.0; 3], [1.0, 2.0, 3.0]])?;
        assert_eq!(b[0], [1.0, 2.0 - 3.0 * 25.4, 3.0]);
        assert_eq!(b[1], [1.0 + 25.4, 2.0, 3.0 + 2.0 * 25.4]);
        // A glTF file starts in metres with Y up: its reader's millimetres
        // stay unscaled and turn to Z up.
        let mut w = wizard();
        w.format = ModelFormat::Gltf;
        w.frame = imported::default_frame(ModelFormat::Gltf);
        let [low, high] = w.bounds()?;
        assert_eq!((low[2], high[2]), (-0.4, 0.4), "file y is board z");
        Ok(())
    }

    #[test]
    fn seat_and_centre_put_the_model_on_the_origin() -> Result<(), String> {
        let mut w = wizard();
        w.frame.origin_offset_nm = triple_nm([3.0, -2.0, 1.5]).map_err(|e| e.to_string())?;
        w.seat();
        w.centre();
        let b = w.bounds()?;
        assert!(b[0][2].abs() < 1e-12);
        assert!((b[0][0] + b[1][0]).abs() < 1e-12);
        assert!((b[0][1] + b[1][1]).abs() < 1e-12);
        Ok(())
    }

    #[test]
    fn unmapped_bodies_block_and_suggestions_fill_only_when_asked() {
        let mut w = wizard();
        assert_eq!(w.problem(Step::File), None);
        assert_eq!(
            w.problem(Step::Materials).as_deref(),
            Some("3 bodies have no role or material")
        );
        assert!(w.definition().is_err(), "never defaulted silently");
        // A mapping the person chose stays.
        w.bodies[0].role = Some(BodyRole::Body);
        w.bodies[0].material = Some(ModelMaterial::Builtin(MaterialId::Ferrite));
        let builtins = pcbcraft_core::materials::all().unwrap_or_default();
        assert_eq!(w.use_suggestions(&builtins), 4);
        assert_eq!(
            w.bodies[0].material,
            Some(ModelMaterial::Builtin(MaterialId::Ferrite))
        );
        // The metal-coloured caps are suggested as leads of a metal.
        for cap in &w.bodies[1..] {
            assert_eq!(cap.role, Some(BodyRole::Lead));
            assert!(matches!(
                cap.material,
                Some(ModelMaterial::Builtin(id))
                    if id.category() == pcbcraft_core::materials::MaterialCategory::Metal
            ));
        }
        assert_eq!(w.problem(Step::Materials), None);
        assert_eq!(
            w.problem(Step::Leads).as_deref(),
            Some("2 leads have no designation")
        );
    }

    #[test]
    fn a_glb_material_is_suggested_as_itself() {
        let mut i = inspection();
        i.bodies[1].material = Some(0);
        i.materials.push(CustomMaterial {
            name: "Tin".into(),
            base_colour: [0.8, 0.8, 0.8, 1.0],
            metallic: 1.0,
            roughness: 0.3,
            clearcoat: None,
            transmission: None,
            ior: None,
            emissive: None,
            textures_ignored: false,
        });
        let w = Wizard::new(
            "a.glb".into(),
            Arc::new(vec![1, 2, 3]),
            ModelFormat::Gltf,
            i,
        );
        let (role, material) = w.suggestion(&w.bodies[1].source, &[]);
        assert_eq!((role, material), (BodyRole::Lead, ModelMaterial::Custom(0)));
    }

    #[test]
    fn leads_are_proposed_from_the_nearest_pads_and_named_ones_stay() {
        let (package, footprint) = package();
        let mut w = wizard();
        let builtins = pcbcraft_core::materials::all().unwrap_or_default();
        w.use_suggestions(&builtins);
        w.bodies[2].designation = "X".into();
        assert_eq!(w.propose_from_pads(&footprint, &package), 1);
        assert_eq!(w.bodies[1].designation, "1");
        assert_eq!(w.bodies[2].designation, "X", "a named lead is kept");
        w.bodies[2].designation.clear();
        w.propose_from_pads(&footprint, &package);
        assert_eq!(w.bodies[2].designation, "2");
        assert_eq!(w.problem(Step::Leads), None);
    }

    /// Closed boxes as one triangle mesh (glTF file units, Y up).
    fn boxes(list: &[([f32; 3], [f32; 3])]) -> (Vec<f32>, Vec<u32>) {
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        for (lo, hi) in list {
            let base = u32::try_from(positions.len() / 3).unwrap_or(0);
            for i in 0..8u32 {
                for (axis, bit) in [1u32, 2, 4].into_iter().enumerate() {
                    positions.push(if i & bit == 0 { lo[axis] } else { hi[axis] });
                }
            }
            for [a, b, c, d] in [
                [0, 2, 3, 1],
                [4, 5, 7, 6],
                [0, 1, 5, 4],
                [2, 6, 7, 3],
                [0, 4, 6, 2],
                [1, 3, 7, 5],
            ] {
                indices.extend([a, b, c, a, c, d].map(|v| base + v));
            }
        }
        (positions, indices)
    }

    /// One box of a test mesh: low and high corners, mm.
    type TestBox = ([f32; 3], [f32; 3]);

    /// A GLB of named meshes (each a list of boxes in mm, glTF axes: Y
    /// up), each with an optional material index into `materials`.
    pub(super) fn glb(
        meshes: &[(&str, Vec<TestBox>, Option<usize>)],
        materials: serde_json::Value,
    ) -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        let (mut views, mut accessors, mut gltf_meshes, mut nodes) =
            (vec![], vec![], vec![], vec![]);
        for (k, (name, list, material)) in meshes.iter().enumerate() {
            let metres: Vec<_> = list
                .iter()
                .map(|(lo, hi)| (lo.map(|c| c / 1000.0), hi.map(|c| c / 1000.0)))
                .collect();
            let (positions, indices) = boxes(&metres);
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in positions.chunks(3) {
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
            let start = bin.len();
            bin.extend(positions.iter().flat_map(|v| v.to_le_bytes()));
            views.push(serde_json::json!({"buffer": 0, "byteOffset": start,
                "byteLength": positions.len() * 4}));
            accessors.push(
                serde_json::json!({"bufferView": 2 * k, "componentType": 5126,
                "count": positions.len() / 3, "type": "VEC3", "min": lo, "max": hi}),
            );
            let start = bin.len();
            bin.extend(indices.iter().flat_map(|v| v.to_le_bytes()));
            views.push(serde_json::json!({"buffer": 0, "byteOffset": start,
                "byteLength": indices.len() * 4}));
            accessors.push(
                serde_json::json!({"bufferView": 2 * k + 1, "componentType": 5125,
                "count": indices.len(), "type": "SCALAR"}),
            );
            let mut primitive = serde_json::json!({
                "attributes": {"POSITION": 2 * k}, "indices": 2 * k + 1});
            if let Some(m) = material {
                primitive["material"] = serde_json::json!(m);
            }
            gltf_meshes.push(serde_json::json!({"name": name, "primitives": [primitive]}));
            nodes.push(serde_json::json!({"mesh": k, "name": name}));
        }
        let node_ids: Vec<usize> = (0..nodes.len()).collect();
        let mut document = serde_json::json!({
            "asset": {"version": "2.0", "generator": "pcbcraft test"},
            "scene": 0,
            "scenes": [{"nodes": node_ids}],
            "nodes": nodes,
            "meshes": gltf_meshes,
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": views,
            "accessors": accessors,
        });
        if materials.as_array().is_some_and(|m| !m.is_empty()) {
            document["materials"] = materials;
        }
        let mut json = document.to_string().into_bytes();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let word = |n: usize| u32::try_from(n).unwrap_or(0).to_le_bytes();
        let mut out = Vec::with_capacity(total);
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend(word(total));
        out.extend(word(json.len()));
        out.extend(b"JSON");
        out.extend(json);
        out.extend(word(bin.len()));
        out.extend(b"BIN\0");
        out.extend(bin);
        out
    }

    /// An original GLB: a moulded body and one gull-wing lead whose foot
    /// (x 2.0…2.5 mm, on the ground) lies beside a raised shoulder (x
    /// 1.8…2.0 mm), so the lowest point is not the bottom centre of the
    /// lead's bounds.
    fn gull_wing_glb() -> Vec<u8> {
        glb(
            &[
                ("Body", vec![([-1.8, 0.2, -1.0], [1.8, 1.0, 1.0])], None),
                (
                    "Lead",
                    vec![
                        ([2.0, 0.0, -0.2], [2.5, 0.1, 0.2]),
                        ([1.8, 0.2, -0.2], [2.0, 0.5, 0.2]),
                    ],
                    None,
                ),
            ],
            serde_json::json!([]),
        )
    }

    /// An original 0805-like chip GLB: an epoxy body and two tin end caps
    /// (glTF materials), the caps over the pads at x = ±0.9 mm.
    pub(super) fn chip_glb() -> Vec<u8> {
        glb(
            &[
                ("body", vec![([-0.8, 0.0, -0.4], [0.8, 0.45, 0.4])], Some(0)),
                (
                    "cap-left",
                    vec![([-1.0, 0.0, -0.4], [-0.8, 0.45, 0.4])],
                    Some(1),
                ),
                (
                    "cap-right",
                    vec![([0.8, 0.0, -0.4], [1.0, 0.45, 0.4])],
                    Some(1),
                ),
            ],
            serde_json::json!([
                {"name": "Epoxy", "pbrMetallicRoughness": {
                    "baseColorFactor": [0.02, 0.02, 0.02, 1.0],
                    "metallicFactor": 0.0, "roughnessFactor": 0.5}},
                {"name": "Tin", "pbrMetallicRoughness": {
                    "baseColorFactor": [0.8, 0.8, 0.8, 1.0],
                    "metallicFactor": 1.0, "roughnessFactor": 0.3}}
            ]),
        )
    }

    /// The wizard on a real file: the reader-frame inspection is framed as
    /// a build frames it, the estimated contact is replaced by the
    /// measured one at Finish, and the result passes a build's recheck,
    /// which the estimate alone does not.
    #[test]
    fn finish_measures_the_contacts_a_build_rechecks() -> Result<(), Box<dyn std::error::Error>> {
        let bytes = gull_wing_glb();
        let format = ModelFormat::Gltf;
        let seen = inspect::inspect_in(&bytes, format, &imported::reader_frame(format))?;
        let mut w = Wizard::new("gull.glb".into(), Arc::new(bytes.clone()), format, seen);
        let framed = inspect::inspect(&bytes, format)?;
        let b = w.bounds()?;
        for (got, want) in b.iter().flatten().zip(framed.bounds_mm.iter().flatten()) {
            assert!((got - want).abs() < 1e-6, "{b:?} vs {:?}", framed.bounds_mm);
        }
        for body in &mut w.bodies {
            let lead = body.source.key.contains("Lead");
            body.role = Some(if lead { BodyRole::Lead } else { BodyRole::Body });
            body.material = Some(ModelMaterial::Builtin(if lead {
                MaterialId::TinMatte
            } else {
                MaterialId::EpoxyMould
            }));
            if lead {
                body.designation = "1".into();
            }
        }
        w.meta.licence = "CC0-1.0".into();
        let estimated = w.request()?;
        assert!((estimated.definition.leads[0].contact()[0] - 2.15).abs() < 1e-6);
        let measured = measure(estimated.clone())?;
        let c = measured.definition.leads[0].contact();
        assert!((c[0] - 2.25).abs() < 1e-4 && c[2].abs() < 1e-4, "{c:?}");
        let use_ = LibraryModel {
            lead_map: vec![LeadMapEntry {
                lead: 0,
                terminal: PackageTerminalId(1),
            }],
            ..LibraryModel::default()
        };
        let placement = pcbcraft_core::library::ModelPlacement::default();
        let shown = imported::preview(&measured.definition, &use_, &bytes, &placement)?;
        assert_eq!(shown.len(), 2);
        assert!(
            imported::preview(&estimated.definition, &use_, &bytes, &placement).is_err(),
            "the bounds estimate is not what a build measures"
        );
        Ok(())
    }

    #[test]
    fn the_request_carries_the_bytes_and_an_explicit_definition() -> Result<(), String> {
        let (package, footprint) = package();
        let mut w = wizard();
        let builtins = pcbcraft_core::materials::all().unwrap_or_default();
        w.use_suggestions(&builtins);
        w.propose_from_pads(&footprint, &package);
        assert_eq!(
            w.problem(Step::Licence).as_deref(),
            Some("State the model's licence")
        );
        w.meta.licence = "CC0-1.0".into();
        w.meta.retrieved = "1 Nov".into();
        assert!(w.problem(Step::Licence).is_some());
        w.meta.retrieved = "2026-11-01".into();
        w.marking_on_top = true;
        assert_eq!(w.problem(Step::Review), None);
        let request = w.request()?;
        assert_eq!(request.name, "chip-0805");
        assert_eq!(request.assets.len(), 1);
        assert_eq!(request.assets[0].data.to_vec(), w.bytes.as_ref().clone());
        assert_eq!(request.assets[0].hash, asset_hash(&w.bytes));
        let d = &request.definition;
        assert_eq!(d.asset.hash, request.assets[0].hash);
        assert_eq!(d.asset.bytes, w.bytes.len() as u64);
        assert_eq!(d.bodies.len(), 3);
        assert_eq!(d.leads.len(), 2);
        assert_eq!(d.leads[0].designation, "1");
        // Contacts are estimated as the bottom centres of the lead bodies;
        // Finish measures them on the geometry (`measure`).
        assert_eq!(d.leads[0].contact(), [-0.9, 0.0, 0.0]);
        assert_eq!(d.hints.pins, Some(2));
        assert_eq!(d.provenance.record.licence.as_deref(), Some("CC0-1.0"));
        assert_eq!(d.provenance.record.basis, Basis::Imported);
        assert_eq!(d.provenance.redistribution, Redistribution::Unknown);
        let area = d.marking_area.ok_or("expected a marking area")?;
        assert_eq!(area.normal, [0.0, 0.0, 1.0]);
        assert!((area.origin()[2] - 0.45).abs() < 1e-12);
        // The previewed lead map: both leads, by exact designation.
        let map = lead_map_of(d, &package);
        assert_eq!(map.len(), 2);
        let request = assigning(w.request()?, DefinitionId(7), map);
        assert!(matches!(
            &request.assign,
            Some(AssignModel { package: DefinitionId(7), lead_map, .. }) if lead_map.len() == 2
        ));
        Ok(())
    }

    #[test]
    fn steps_go_forward_and_back_in_order() {
        assert_eq!(Step::File.back(), None);
        assert_eq!(Step::File.next(), Some(Step::Frame));
        assert_eq!(Step::Review.next(), None);
        assert_eq!(Step::Review.back(), Some(Step::Licence));
        assert!(iso_date("2026-11-01"));
        assert!(!iso_date("2026-1-01"));
    }

    /// The 3D tab driven through real egui frames: the model card's
    /// actions, the docked steps, the import command, its refusal coming
    /// back to Review, and the assignment once the Model draft exists.
    mod frames {
        use super::super::super::*;
        use super::super::{State, Step, Wizard, wizard_id};
        use crate::refusal::{Origin, Refused};
        use egui::{Pos2, Rect};
        use pcbcraft_core::library::model3d::{ModelFormat, ModelMaterial};
        use pcbcraft_core::library::{BodyRole, PackageDefinition, PackageModel};
        use pcbcraft_core::{LibraryCommand, ModelRef, Revision};
        use pcbcraft_solidcraft::models::{imported, inspect};

        type TestResult = Result<(), Box<dyn std::error::Error>>;

        struct Harness {
            ctx: egui::Context,
            engine: Engine,
            editor: PartEditor,
        }

        impl Harness {
            fn new() -> Result<Self, Box<dyn std::error::Error>> {
                let ctx = egui::Context::default();
                pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
                let mut engine = Engine::default();
                let mut editor = PartEditor::default();
                editor.load_template(Template::TwoTerminal);
                if let Some(request) = editor.take_request() {
                    run(&mut engine, request)?;
                }
                editor.commit(&ctx, &engine);
                editor.on_tab("model");
                Ok(Self {
                    ctx,
                    engine,
                    editor,
                })
            }

            fn card(
                &mut self,
                events: Vec<egui::Event>,
            ) -> (Vec<(String, Pos2)>, Option<Operation>) {
                let mut operation = None;
                let mut output = self.ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(360.0, 2400.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| operation = self.editor.inspector(ui, &self.engine),
                );
                output.textures_delta.clear();
                fn texts(shape: &egui::Shape, out: &mut Vec<(String, Pos2)>) {
                    match shape {
                        egui::Shape::Text(text) => out.push((
                            text.galley.job.text.clone(),
                            text.pos + text.galley.size() * 0.5,
                        )),
                        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                        _ => {}
                    }
                }
                let mut out = Vec::new();
                for shape in &output.shapes {
                    texts(&shape.shape, &mut out);
                }
                (out, operation)
            }

            fn texts(&mut self) -> Vec<String> {
                self.card(vec![]).0.into_iter().map(|(t, _)| t).collect()
            }

            fn click(
                &mut self,
                label: &str,
            ) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
                let (shown, _) = self.card(vec![]);
                let at = shown
                    .iter()
                    .find(|(t, _)| t == label)
                    .map(|(_, p)| *p)
                    .ok_or_else(|| {
                        format!(
                            "{label} not in {:?}",
                            shown.iter().map(|(t, _)| t).collect::<Vec<_>>()
                        )
                    })?;
                let button = |pressed| egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                self.card(vec![egui::Event::PointerMoved(at)]);
                let (_, a) = self.card(vec![button(true)]);
                let (_, b) = self.card(vec![button(false)]);
                Ok(a.or(b))
            }

            fn package(&self) -> Result<DefinitionId, Box<dyn std::error::Error>> {
                Ok(self
                    .editor
                    .draft
                    .as_ref()
                    .ok_or("no working set")?
                    .ids
                    .package)
            }

            fn step(&self) -> Option<Step> {
                self.editor.model_import.wizard().map(|w| w.step)
            }

            fn package_definition(&self) -> Result<PackageDefinition, Box<dyn std::error::Error>> {
                Ok(self
                    .editor
                    .draft
                    .as_ref()
                    .ok_or("no working set")?
                    .package
                    .clone())
            }
        }

        #[test]
        fn the_model_card_offers_the_library_and_the_import() -> TestResult {
            let mut h = Harness::new()?;
            let shown = h.texts();
            for label in ["MODEL", "Choose model…", "Import model file…"] {
                assert!(
                    shown.iter().any(|t| t == label),
                    "{label} missing from {shown:?}"
                );
            }
            assert!(
                h.click("Choose model…")?.is_none(),
                "opening the sheet changes nothing"
            );
            assert_eq!(h.editor.model_chooser.open, Some(h.package()?));
            Ok(())
        }

        #[test]
        fn the_chooser_sheet_searches_and_shows_a_refused_search_without_assigning() -> TestResult {
            let mut h = Harness::new()?;
            let package = h.package()?;
            h.editor.model_chooser.open_for(package);
            let mut sent = None;
            let mut output = h.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                    ..Default::default()
                },
                |ui| sent = h.editor.model_chooser.show(ui.ctx(), &h.engine),
            );
            output.textures_delta.clear();
            assert!(sent.is_none());
            assert_eq!(
                h.editor.model_chooser.open,
                Some(package),
                "the sheet stays open"
            );
            // Esc closes it without a change.
            let mut output = h.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                    events: vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ui| sent = h.editor.model_chooser.show(ui.ctx(), &h.engine),
            );
            output.textures_delta.clear();
            assert!(sent.is_none());
            assert_eq!(h.editor.model_chooser.open, None);
            Ok(())
        }

        /// The chip GLB read as the worker reads a picked file.
        fn chip_wizard() -> Result<Wizard, Box<dyn std::error::Error>> {
            let bytes = super::chip_glb();
            let format = ModelFormat::Gltf;
            let seen = inspect::inspect_in(&bytes, format, &imported::reader_frame(format))?;
            Ok(Wizard::new(
                "chip-0805.glb".into(),
                std::sync::Arc::new(bytes),
                format,
                seen,
            ))
        }

        /// Frames until the wizard sends an operation or settles (the lead
        /// contacts are measured on a worker thread).
        fn settle(h: &mut Harness) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
            for _ in 0..2000 {
                let (_, operation) = h.card(vec![]);
                if operation.is_some() {
                    return Ok(operation);
                }
                if !matches!(h.editor.model_import.state, State::Measuring { .. }) {
                    return Ok(None);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err("the lead contacts were never measured".into())
        }

        fn walk_to_review(h: &mut Harness, wizard: Wizard) -> TestResult {
            h.editor.model_import.open(wizard);
            let shown = h.texts();
            assert!(
                shown.iter().any(|t| t.contains("1 OF 6")),
                "the steps fill the card: {shown:?}"
            );
            assert!(
                !shown.iter().any(|t| t == "Choose model…"),
                "the card is replaced"
            );
            h.click("Next")?;
            assert_eq!(h.step(), Some(Step::Frame));
            h.click("Seat on board")?;
            h.click("Next")?;
            assert_eq!(h.step(), Some(Step::Materials));
            // Unmapped bodies keep Next disabled.
            h.click("Next")?;
            assert_eq!(h.step(), Some(Step::Materials));
            h.click("Use suggestions")?;
            h.click("Next")?;
            assert_eq!(h.step(), Some(Step::Leads));
            h.click("Propose from pads")?;
            h.click("Next")?;
            assert_eq!(h.step(), Some(Step::Licence));
            if let State::Steps(w) = &mut h.editor.model_import.state {
                w.meta.licence = "CC0-1.0".into();
            }
            h.click("Next")?;
            assert_eq!(h.step(), Some(Step::Review));
            Ok(())
        }

        #[test]
        fn the_wizard_imports_then_assigns_the_new_model_with_its_lead_map() -> TestResult {
            let mut h = Harness::new()?;
            let package = h.package()?;
            walk_to_review(&mut h, chip_wizard()?)?;
            let shown = h.texts();
            assert!(
                shown
                    .iter()
                    .any(|t| t == "2 of 2 terminals map by designation (2 lead tags)"),
                "the lead map is previewed: {shown:?}"
            );
            assert!(
                h.click("Import model")?.is_none(),
                "the contacts are measured first"
            );
            let Some(Operation::Library(LibraryCommand::ImportModel(request))) = settle(&mut h)?
            else {
                return Err("expected library.import_model".into());
            };
            let definition = &request.definition;
            assert_eq!(definition.bodies.len(), 3);
            // The file's own materials, mapped explicitly by Use suggestions.
            assert_eq!(definition.custom_materials.len(), 2);
            assert!(
                definition
                    .bodies
                    .iter()
                    .all(|b| matches!(b.material, ModelMaterial::Custom(_)))
            );
            assert_eq!(
                definition
                    .bodies
                    .iter()
                    .filter(|b| b.role == BodyRole::Lead)
                    .count(),
                2
            );
            // The measured contacts are the caps' footprints on the board.
            for (lead, x) in definition.leads.iter().zip([-0.9, 0.9]) {
                assert!(
                    (lead.contact()[0] - x).abs() < 1e-4 && lead.contact()[2].abs() < 1e-4,
                    "{:?}",
                    lead.contact()
                );
            }
            // Finish assigns the model in the same command.
            assert_eq!(
                request
                    .assign
                    .as_ref()
                    .map(|a| (a.package, a.lead_map.len())),
                Some((package, 2))
            );
            // The app applies it: D1 packs the bytes, creates the draft and
            // sets it on the package, as one undo step.
            let before = h.engine.to_file();
            let created = h.engine.apply(
                h.engine.revision(),
                Operation::Library(LibraryCommand::ImportModel(request.clone())),
            )?;
            let pcbcraft_core::Outcome::DraftCreated { id } = created else {
                return Err("no draft".into());
            };
            assert!(
                h.engine
                    .project()
                    .library
                    .assets
                    .iter()
                    .any(|a| a.hash == request.definition.asset.hash),
                "the file is packed in the project"
            );
            let (_, after) = h.card(vec![]);
            assert!(after.is_none(), "nothing more is sent: {after:?}");
            let PackageModel::Library(model) = h.package_definition()?.model else {
                return Err("expected the package to use the library model".into());
            };
            let model = &model;
            assert_eq!(model.model, ModelRef::new(id, Revision::FIRST));
            assert_eq!(model.lead_map.len(), 2);
            assert!(!h.editor.model_import.is_open(), "the wizard closes");
            // The Component 3D preview resolves the draft model and its
            // packed bytes, and builds it.
            let mut previewed = h.package_definition()?;
            previewed.model = PackageModel::Library(model.clone());
            let asset =
                crate::scene::library_asset(&h.engine, &previewed)?.ok_or("no library asset")?;
            let shown = imported::preview(
                &asset.model,
                model,
                &asset.bytes,
                &pcbcraft_core::library::ModelPlacement::default(),
            )?;
            assert_eq!(shown.len(), 3);
            // One undo after Finish restores the state before the import.
            h.engine.undo()?;
            assert_eq!(h.engine.to_file(), before);
            Ok(())
        }

        /// A refusal returns to Review with the reason shown: a file whose
        /// contacts cannot be measured, and an import the engine refuses.
        #[test]
        fn a_refused_import_returns_to_review_with_the_refusal() -> TestResult {
            let mut h = Harness::new()?;
            walk_to_review(
                &mut h,
                Wizard::new(
                    "chip-0805.step".into(),
                    std::sync::Arc::new(b"ISO-10303-21; original test bytes".to_vec()),
                    ModelFormat::Step,
                    super::inspection(),
                ),
            )?;
            assert!(h.click("Import model")?.is_none());
            assert!(settle(&mut h)?.is_none(), "nothing is sent");
            assert_eq!(h.step(), Some(Step::Review));
            assert!(
                h.editor
                    .model_import
                    .refusal
                    .as_deref()
                    .is_some_and(|r| r.starts_with("Measuring the lead contacts failed")),
                "{:?}",
                h.editor.model_import.refusal
            );
            assert!(matches!(h.editor.model_import.state, State::Steps(_)));
            // A measured import the engine refuses (here: a size that does
            // not match the packed file).
            let mut h = Harness::new()?;
            walk_to_review(&mut h, chip_wizard()?)?;
            h.click("Import model")?;
            let Some(Operation::Library(LibraryCommand::ImportModel(mut request))) =
                settle(&mut h)?
            else {
                return Err("expected library.import_model".into());
            };
            request.definition.asset.bytes += 1;
            let error = h
                .engine
                .apply(
                    h.engine.revision(),
                    Operation::Library(LibraryCommand::ImportModel(request)),
                )
                .err()
                .ok_or("the engine accepted a mismatched size")?;
            // As App::apply does: hand the refusal back during a frame.
            let mut error = Some(error);
            let mut output = h.ctx.run_ui(egui::RawInput::default(), |ui| {
                if let Some(error) = error.take() {
                    crate::refusal::hand_back(
                        ui.ctx(),
                        Refused {
                            origin: Origin::Dialog(wizard_id()),
                            error,
                        },
                        false,
                    );
                }
            });
            output.textures_delta.clear();
            let (_, operation) = h.card(vec![]);
            assert!(operation.is_none());
            assert_eq!(h.step(), Some(Step::Review));
            assert!(
                h.editor.model_import.refusal.is_some(),
                "the refusal shows in Review"
            );
            assert!(matches!(h.editor.model_import.state, State::Steps(_)));
            Ok(())
        }
    }
}
