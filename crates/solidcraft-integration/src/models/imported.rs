//! Slice D2: build a library model (`PackageModel::Library`) into a native
//! definition: the packed asset read through Solidcraft's public readers,
//! the source frame applied, every body given its mapped role and
//! material, and lead contacts derived from the tagged geometry. Bounds
//! and lead contacts are rechecked against the model's recorded values; a
//! mismatch refuses and names the asset. One shared definition per package
//! revision, like generated models. The board hook lives in `project.rs`
//! and is wired at integration (that file is owned elsewhere during the
//! fan-out); [`preview`] builds the Component 3D view on its own.
//!
//! **The model frame.** A file is read in its reader's millimetres: STEP
//! states its units, glTF is in metres (1000 mm per unit) with Y up. The
//! [`SourceFrame`] then scales by `unit_mm / format_unit_mm(format)` (1 for
//! a file whose units are right), turns Y up into Z up (a quarter turn
//! about +x: the file's +y becomes +z) and adds `origin_offset`, in that
//! order. The result has z = 0 on the seating plane, as generated models.
//!
//! **Keys.** A body's key is its path in the file: a STEP body's product
//! path and solid name joined by `/`, a glTF body's mesh name (with its
//! material when a mesh has several); a repeated key gets `#2`, `#3`, … in
//! file order. A lead tag names a body key, or one face of a body as
//! `<body key>@face:<n>` (the B-rep face index, from 0).
//!
//! `.solidcraft` models are inspected ([`super::inspect`]) but not built
//! yet: keeping their parameters in the board needs a design insert from
//! bytes, which Solidcraft's command API does not offer.
use super::ModelDefinition as BuiltDefinition;
use super::{GeneratedBody, PARAMETER_PREFIX, look};
use crate::material::{BodyKind, Detail, linear_to_srgb8};
use crate::native::{Look, Native};
use crate::scene::Geometry;
use crate::{Error, Result};
use pcbcraft_core::geometry::Transform3;
use pcbcraft_core::library::ModelPlacement;
use pcbcraft_core::library::bundle::{MAX_ASSET_BYTES, asset_hash};
use pcbcraft_core::library::model3d::{
    BodyRole, CustomMaterial, LibraryModel, ModelDefinition, ModelFormat, ModelMaterial,
    SourceFrame, UpAxis,
};
use serde_json::json;
use solidcraft_engine::doc::FeatureKind;
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::render::clear_coat::ClearCoat;
use solidcraft_engine::render::surface::{MIN_ROUGHNESS, OpaqueSurface};
use std::collections::{BTreeMap, BTreeSet};

/// Built bounds and lead contacts must match the recorded ones this
/// closely, mm (the same reader and frame give the same numbers; the
/// tolerance only absorbs float noise).
pub const RECHECK_TOLERANCE_MM: f64 = 0.01;
/// A lead's contact is the lowest geometry of its tag: the points within
/// this height of the lowest one, mm.
pub const CONTACT_BAND_MM: f64 = 0.01;
/// The face-key separator of a lead tag.
pub const FACE_KEY: &str = "@face:";
/// Bytes of a body key ([`pcbcraft_core::library::model3d::MAX_BODY_KEY`]);
/// a longer file path is cut, then made unique.
const MAX_KEY: usize = pcbcraft_core::library::model3d::MAX_BODY_KEY - 16;
/// Longest definition name (as for generated models).
const MAX_STEM: usize = 64;
/// The file name readers are given: it names nothing the keys depend on
/// beyond a body without any name of its own.
const FILE_STEM: &str = "model";
/// Metals ignore the dielectric IOR; Solidcraft still requires one in [1, 3].
const DEFAULT_IOR: f64 = 1.5;
/// glTF's clear-coat roughness default is 0; Solidcraft's lowest is this.
const COAT_ROUGHNESS: f64 = MIN_ROUGHNESS;

/// Millimetres per unit of what a format's reader produces before the
/// frame: STEP and `.solidcraft` state their units (read as mm), glTF is in
/// metres.
pub fn format_unit_mm(format: ModelFormat) -> f64 {
    match format {
        ModelFormat::Gltf => 1000.0,
        ModelFormat::Step | ModelFormat::Solidcraft => 1.0,
    }
}

/// The frame a file of `format` has when it follows its format's
/// conventions: glTF metres with Y up, STEP and `.solidcraft` as stated
/// with Z up. The import wizard starts from it.
pub fn default_frame(format: ModelFormat) -> SourceFrame {
    SourceFrame {
        unit_mm: format_unit_mm(format),
        up: match format {
            ModelFormat::Gltf => UpAxis::Y,
            ModelFormat::Step | ModelFormat::Solidcraft => UpAxis::Z,
        },
        origin_offset: [0.0; 3],
    }
}

/// The frame that leaves the reader's millimetres as they are (scale 1, Z
/// up, no offset): an inspection in it reports source bounds, which the
/// import wizard moves into any frame with [`frame_bounds`].
pub fn reader_frame(format: ModelFormat) -> SourceFrame {
    SourceFrame {
        unit_mm: format_unit_mm(format),
        up: UpAxis::Z,
        origin_offset: [0.0; 3],
    }
}

/// The frame's checked scale from the reader's millimetres.
fn frame_scale(frame: &SourceFrame, format: ModelFormat) -> Result<f64> {
    let scale = frame.unit_mm / format_unit_mm(format);
    let offset_ok = frame
        .origin_offset
        .iter()
        .all(|v| v.is_finite() && v.abs() <= 10_000.0);
    let scale_ok = scale.is_finite() && (1e-6..=1e6).contains(&scale) && scale > 1e-6;
    if !scale_ok || !offset_ok {
        return Err(Error::Invalid(
            "model.frame: the unit must be positive and the origin offset within ±10000 mm".into(),
        ));
    }
    Ok(scale)
}

/// A point of the reader's frame in the model frame.
pub fn frame_point(frame: &SourceFrame, format: ModelFormat, p: [f64; 3]) -> Result<[f64; 3]> {
    let s = frame_scale(frame, format)?;
    let [x, y, z] = p.map(|v| v * s);
    let turned = match frame.up {
        UpAxis::Z => [x, y, z],
        UpAxis::Y => [x, -z, y],
    };
    let o = frame.origin_offset;
    Ok([turned[0] + o[0], turned[1] + o[1], turned[2] + o[2]])
}

/// Bounds of the reader's frame in the model frame (the box of the moved
/// corners: the turn is a quarter turn, so the box is exact).
pub fn frame_bounds(
    frame: &SourceFrame,
    format: ModelFormat,
    [lo, hi]: [[f64; 3]; 2],
) -> Result<[[f64; 3]; 2]> {
    let a = frame_point(frame, format, lo)?;
    let b = frame_point(frame, format, hi)?;
    Ok([
        [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
        [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
    ])
}

/// A model file read for building: Solidcraft's import feature and each
/// body's key and colour hint, in the order the feature builds them.
pub(crate) struct Source {
    pub kind: FeatureKind,
    pub keys: Vec<String>,
    /// The file's colour of each body (linear RGBA), a hint only.
    pub colours: Vec<Option<[f64; 4]>>,
    /// glTF: each body's look from the reader (to match its material).
    pub looks: Vec<Option<solidcraft_engine::doc::appearance::Look>>,
    pub warnings: Vec<String>,
}

/// Read `bytes` as `format` with Solidcraft's readers. `.solidcraft` is
/// read by [`super::inspect`] directly.
pub(crate) fn read_source(bytes: &[u8], format: ModelFormat) -> Result<Source> {
    if bytes.len() > MAX_ASSET_BYTES {
        return Err(Error::Invalid(format!(
            "model.asset: the file has {} bytes; a model asset has at most {MAX_ASSET_BYTES}",
            bytes.len()
        )));
    }
    let io = |e: solidcraft_engine::io::IoError| Error::Invalid(format!("model.asset: {e}"));
    match format {
        ModelFormat::Step => {
            let feature = solidcraft_engine::io::step_import_feature_named(
                bytes,
                &format!("{FILE_STEM}.step"),
                solidcraft_engine::io::StepNames::default(),
            )
            .map_err(io)?;
            let raw: Vec<String> = feature
                .import
                .bodies
                .iter()
                .map(|b| {
                    let mut parts: Vec<&str> = b.path.iter().map(String::as_str).collect();
                    if parts.last() != Some(&b.name.as_str()) && !b.name.trim().is_empty() {
                        parts.push(&b.name);
                    }
                    parts.join("/")
                })
                .collect();
            let colours = feature
                .import
                .bodies
                .iter()
                .map(|b| {
                    b.color
                        .map(|[r, g, bl]| [f64::from(r), f64::from(g), f64::from(bl), 1.0])
                })
                .collect();
            let count = raw.len();
            Ok(Source {
                kind: feature.kind,
                keys: unique_keys(raw),
                colours,
                looks: vec![None; count],
                warnings: feature.warnings,
            })
        }
        ModelFormat::Gltf => {
            let mut no_files = |name: &str| Err(format!("`{name}` is outside the model file"));
            let mut read =
                solidcraft_engine::io::read_gltf(bytes, &format!("{FILE_STEM}.glb"), &mut no_files)
                    .map_err(io)?;
            read.flatten();
            let FeatureKind::MeshImport { meshes, .. } = &read.feature.kind else {
                return Err(Error::Verification("glTF read no mesh feature".into()));
            };
            let raw = meshes.iter().map(|m| m.name.clone()).collect();
            let colours = meshes
                .iter()
                .map(|m| {
                    m.color
                        .map(|[r, g, b]| [f64::from(r), f64::from(g), f64::from(b), 1.0])
                })
                .collect();
            Ok(Source {
                kind: read.feature.kind,
                keys: unique_keys(raw),
                colours,
                looks: read.looks,
                warnings: read.feature.warnings,
            })
        }
        ModelFormat::Solidcraft => Err(Error::Invalid(
            "model.unsupported: `.solidcraft` models are inspected but not built yet".into(),
        )),
    }
}

/// Keys cut to their bound, then made unique in file order.
fn unique_keys(raw: Vec<String>) -> Vec<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for key in raw {
        let mut base: String = key.trim().to_owned();
        if base.is_empty() {
            base = "body".into();
        }
        if base.len() > MAX_KEY {
            let mut cut = MAX_KEY;
            while !base.is_char_boundary(cut) {
                cut -= 1;
            }
            base.truncate(cut);
        }
        let mut n = seen.get(&base).copied().unwrap_or(1);
        let mut candidate = if n == 1 {
            base.clone()
        } else {
            format!("{base}#{n}")
        };
        while taken.contains(&candidate) {
            n += 1;
            candidate = format!("{base}#{n}");
        }
        seen.insert(base, n + 1);
        taken.insert(candidate.clone());
        out.push(candidate);
    }
    out
}

/// Add `source` as the definition `component` in `native`, in the model
/// frame; returns the native body names in key order. Leaves the root
/// component active.
pub(crate) fn add_source(
    native: &mut Native,
    component: &str,
    source: Source,
    frame: &SourceFrame,
    format: ModelFormat,
) -> Result<Vec<String>> {
    let scale = frame_scale(frame, format)?;
    let created = native.component(component, false)?;
    let names: Vec<String> = (1..=source.keys.len())
        .map(|k| format!("{component} m{k}"))
        .collect();
    let session = native.session_mut();
    let engine = |e: solidcraft_engine::doc::DocError| Error::Engine {
        command: "import".into(),
        message: e.to_string(),
    };
    let id = session
        .doc_mut()
        .add_feature(source.kind, Some(&format!("{component} import")))
        .map_err(engine)?;
    if let Some(f) = session.doc_mut().feature_mut(id) {
        f.component = created.component;
        f.body_names.clone_from(&names);
        f.inserted = true;
    }
    session.refresh();
    if let Some(error) = session.model.result(id).and_then(|r| r.error.clone()) {
        return Err(Error::Invalid(format!(
            "model.asset: the file did not build: {error}"
        )));
    }
    let built: Vec<String> = session
        .model
        .state()
        .bodies
        .iter()
        .filter(|b| b.feature == id)
        .map(|b| b.name.clone())
        .collect();
    if built != names {
        return Err(Error::Verification(format!(
            "model.asset: the file built {} bodies as {:?}, expected {}",
            built.len(),
            built.iter().take(4).collect::<Vec<_>>(),
            names.len()
        )));
    }
    if !names.is_empty() {
        if (scale - 1.0).abs() > 1e-12 {
            native.call(
                "solid.scale",
                json!({"bodies": names, "factor": scale, "origin": [0, 0, 0],
                    "name": format!("{component} unit")}),
            )?;
        }
        if frame.up == UpAxis::Y {
            native.call(
                "solid.move",
                json!({"bodies": names, "axis": [1, 0, 0], "angle": "90 deg",
                    "name": format!("{component} up")}),
            )?;
        }
        if frame.origin_offset.iter().any(|v| *v != 0.0) {
            native.call(
                "solid.move",
                json!({"bodies": names, "translate": frame.origin_offset,
                    "name": format!("{component} origin")}),
            )?;
        }
    }
    native.activate_root()?;
    if let Some(error) = native.evaluation_error() {
        return Err(Error::Verification(format!(
            "model.build: {component} did not evaluate: {error}"
        )));
    }
    Ok(names)
}

/// The union bounds of `names` in their definition's frame.
pub(crate) fn bounds_of(native: &Native, names: &[String]) -> Result<[[f64; 3]; 2]> {
    let state = native.session().model.state();
    let mut out: Option<[[f64; 3]; 2]> = None;
    for name in names {
        let body = state
            .body(name)
            .ok_or_else(|| Error::Verification(format!("model.build: no body {name}")))?;
        let b = body.outline_bounds();
        let (lo, hi) = ([b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z]);
        out = Some(match out {
            None => [lo, hi],
            Some([l, h]) => [
                [l[0].min(lo[0]), l[1].min(lo[1]), l[2].min(lo[2])],
                [h[0].max(hi[0]), h[1].max(hi[1]), h[2].max(hi[2])],
            ],
        });
    }
    out.ok_or_else(|| Error::Invalid("model.asset: the file has no bodies".into()))
}

/// The body key and optional face index a lead tag names.
fn split_tag<'a>(tag: &'a str, keys: &[String]) -> Result<(&'a str, Option<u32>)> {
    if keys.iter().any(|k| k == tag) {
        return Ok((tag, None));
    }
    if let Some((body, face)) = tag.rsplit_once(FACE_KEY)
        && let Ok(face) = face.parse::<u32>()
        && keys.iter().any(|k| k == body)
    {
        return Ok((body, Some(face)));
    }
    Err(Error::Invalid(format!(
        "model.leads: the lead tag `{tag}` names no body of the file"
    )))
}

/// The lowest point of a body (or one of its faces) in the definition
/// frame: the centre of the points within [`CONTACT_BAND_MM`] of the
/// lowest, at that height.
fn contact(native: &Native, name: &str, face: Option<u32>, tag: &str) -> Result<[f64; 3]> {
    let state = native.session().model.state();
    let body = state
        .body(name)
        .ok_or_else(|| Error::Verification(format!("model.build: no body {name}")))?;
    let mesh = body.mesh();
    let points: Vec<Vec3> = match face {
        None => mesh.positions.clone(),
        Some(face) => mesh
            .triangles
            .iter()
            .zip(&mesh.tri_face)
            .filter(|(_, f)| **f == face)
            .flat_map(|(t, _)| t.iter())
            .filter_map(|i| usize::try_from(*i).ok().and_then(|i| mesh.positions.get(i)))
            .copied()
            .collect(),
    };
    let low = points
        .iter()
        .map(|p| p.z)
        .filter(|z| z.is_finite())
        .min_by(f64::total_cmp)
        .ok_or_else(|| {
            Error::Invalid(format!("model.leads: the lead tag `{tag}` has no geometry"))
        })?;
    let band: Vec<&Vec3> = points
        .iter()
        .filter(|p| p.z <= low + CONTACT_BAND_MM)
        .collect();
    let fold = |f: fn(f64, f64) -> f64, start: f64, axis: fn(&Vec3) -> f64| {
        band.iter().map(|p| axis(p)).fold(start, f)
    };
    let (x0, x1) = (
        fold(f64::min, f64::INFINITY, |p| p.x),
        fold(f64::max, f64::NEG_INFINITY, |p| p.x),
    );
    let (y0, y1) = (
        fold(f64::min, f64::INFINITY, |p| p.y),
        fold(f64::max, f64::NEG_INFINITY, |p| p.y),
    );
    Ok([(x0 + x1) / 2.0, (y0 + y1) / 2.0, low])
}

/// Contacts of every lead tag of `model`, in tag order, from the bodies
/// `names` (in key order) of a built definition.
fn contacts(
    native: &Native,
    model: &ModelDefinition,
    keys: &[String],
    names: &[String],
) -> Result<Vec<[f64; 3]>> {
    model
        .leads
        .iter()
        .map(|lead| {
            let (key, face) = split_tag(&lead.key, keys)?;
            let at = keys
                .iter()
                .position(|k| k == key)
                .ok_or_else(|| Error::Verification(format!("model.leads: no body {key}")))?;
            let name = names
                .get(at)
                .ok_or_else(|| Error::Verification(format!("model.leads: no body {key}")))?;
            contact(native, name, face, &lead.key)
        })
        .collect()
}

/// The lead contacts of the tagged geometry, model frame, in tag order:
/// what the import wizard records in each `LeadTag::contact` (the
/// recorded contacts themselves are not read).
pub fn lead_contacts(model: &ModelDefinition, bytes: &[u8]) -> Result<Vec<[f64; 3]>> {
    let source = read_source(bytes, model.format)?;
    let keys = source.keys.clone();
    let mut native = Native::new("Lead contacts")?;
    let names = add_source(
        &mut native,
        "pkglibleads",
        source,
        &model.frame,
        model.format,
    )?;
    contacts(&native, model, &keys, &names)
}

/// One built body of a library model.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryBody {
    /// The native body name, unique within the board design.
    pub name: String,
    /// Its key in the file.
    pub key: String,
    pub role: BodyRole,
    pub material: ModelMaterial,
    /// The package terminal its lead tag maps to (`LibraryModel::lead_map`).
    pub terminal: Option<u32>,
}

impl LibraryBody {
    /// How the body is drawn and checked, as for generated bodies.
    pub fn kind(&self) -> BodyKind {
        match self.role {
            BodyRole::Lead | BodyRole::Termination | BodyRole::ExposedPad => BodyKind::ModelLead,
            _ => BodyKind::ModelBody,
        }
    }
}

/// A built library-model definition. Its bodies with built-in materials
/// are recorded like generated bodies ([`Self::generated`]); a body with
/// one of the model's own materials is drawn with [`surface`].
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryDefinition {
    pub component: String,
    pub bodies: Vec<LibraryBody>,
}

impl LibraryDefinition {
    /// The bodies with built-in materials, as a generated definition (for
    /// `models::record`, which keeps role, material and terminal).
    pub fn generated(&self) -> BuiltDefinition {
        BuiltDefinition {
            component: self.component.clone(),
            bodies: self
                .bodies
                .iter()
                .filter_map(|b| match b.material {
                    ModelMaterial::Builtin(material) => Some(GeneratedBody {
                        name: b.name.clone(),
                        role: b.role,
                        material,
                        terminal: b.terminal,
                    }),
                    ModelMaterial::Custom(_) => None,
                })
                .collect(),
        }
    }
}

/// The product look of a body's material: a built-in's full look, or a
/// model material's PBR factors and clear coat. Transmission and emission
/// are not drawn by the opaque surface; base-colour alpha is ignored
/// (opaque, as a saved look must be).
pub fn surface(
    model: &ModelDefinition,
    material: ModelMaterial,
) -> Result<(OpaqueSurface, Detail)> {
    match material {
        ModelMaterial::Builtin(id) => {
            let def = look::definition(id)?;
            Ok((look::surface(&def)?, look::detail(&def)))
        }
        ModelMaterial::Custom(index) => {
            let m = custom(model, index)?;
            let invalid = |e: solidcraft_engine::render::surface::SurfaceError| {
                Error::Invalid(format!("model.material: {}: {e}", m.name))
            };
            let [r, g, b, _] = m.base_colour.map(|c| c.clamp(0.0, 1.0));
            let surface = OpaqueSurface::new(
                Vec3::new(r, g, b),
                m.metallic.clamp(0.0, 1.0),
                m.roughness.clamp(MIN_ROUGHNESS, 1.0),
                dielectric_ior(m),
            )
            .map_err(invalid)?;
            let surface = match m.clearcoat.filter(|c| *c > 0.0) {
                Some(weight) => surface.with_clear_coat(
                    ClearCoat::new(weight.clamp(0.0, 1.0), COAT_ROUGHNESS, DEFAULT_IOR)
                        .map_err(invalid)?,
                ),
                None => surface,
            };
            Ok((surface, Detail::NONE))
        }
    }
}

fn custom(model: &ModelDefinition, index: u16) -> Result<&CustomMaterial> {
    model
        .custom_materials
        .get(usize::from(index))
        .ok_or_else(|| {
            Error::Invalid(format!(
                "model.body_unmapped: custom material {index} does not exist ({} defined)",
                model.custom_materials.len()
            ))
        })
}

fn dielectric_ior(m: &CustomMaterial) -> f64 {
    match m.ior {
        Some(ior) if ior >= 1.0 => ior.min(3.0),
        _ => DEFAULT_IOR,
    }
}

/// The opaque look a native file saves for a model material.
fn saved_custom(m: &CustomMaterial) -> Look<'_> {
    let [r, g, b, _] = m.base_colour;
    Look {
        name: &m.name,
        srgb8: [r, g, b].map(linear_to_srgb8),
        metallic: m.metallic.clamp(0.0, 1.0),
        roughness: m.roughness.clamp(MIN_ROUGHNESS, 1.0),
        ior: dielectric_ior(m),
    }
}

/// The packed asset is the one the model names: its length and hash.
pub fn check_asset(model: &ModelDefinition, bytes: &[u8]) -> Result<()> {
    let length_ok = u64::try_from(bytes.len()).is_ok_and(|n| n == model.asset.bytes);
    if !length_ok || asset_hash(bytes) != model.asset.hash {
        return Err(Error::Invalid(format!(
            "model.asset: the packed asset {} ({}) does not match its bytes",
            model.asset.file_name, model.asset.hash
        )));
    }
    Ok(())
}

fn valid_stem(stem: &str) -> bool {
    stem.len() <= MAX_STEM
        && stem.starts_with(PARAMETER_PREFIX)
        && stem.len() > PARAMETER_PREFIX.len()
        && stem.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// The model's body mapping against the file's keys: every file body
/// mapped exactly once, every mapping naming a file body, every custom
/// material defined. Never defaulted.
fn check_mapping(model: &ModelDefinition, keys: &[String]) -> Result<()> {
    let file = &model.asset.file_name;
    let mut mapped = BTreeSet::new();
    for body in &model.bodies {
        if !keys.contains(&body.key) {
            return Err(Error::Invalid(format!(
                "model.asset: {file} has no body `{}`",
                body.key
            )));
        }
        if !mapped.insert(body.key.as_str()) {
            return Err(Error::Invalid(format!(
                "model.body_unmapped: body `{}` is mapped twice",
                body.key
            )));
        }
        if let ModelMaterial::Custom(index) = body.material {
            custom(model, index)?;
        }
    }
    if let Some(key) = keys.iter().find(|k| !mapped.contains(k.as_str())) {
        return Err(Error::Invalid(format!(
            "model.body_unmapped: body `{key}` of {file} has no role and material"
        )));
    }
    Ok(())
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| (x - y).abs() <= RECHECK_TOLERANCE_MM)
}

/// Build `model` (with its asset `bytes`) as component `component` in
/// `native`: the bodies in the model frame with their mapped looks saved,
/// bounds and lead contacts rechecked, lead bodies bound to the package
/// terminals `use_.lead_map` names. Leaves the root component active.
pub fn build_library_model(
    native: &mut Native,
    component: &str,
    model: &ModelDefinition,
    use_: &LibraryModel,
    bytes: &[u8],
) -> Result<LibraryDefinition> {
    check_asset(model, bytes)?;
    if !valid_stem(component) {
        return Err(Error::Invalid(format!(
            "model.name: `{component}` must start with `{PARAMETER_PREFIX}` and use at most \
             {MAX_STEM} ASCII letters, digits and underscores"
        )));
    }
    if native.find_component(component).is_some() {
        return Err(Error::Invalid(format!(
            "model.name: the design already has a definition named {component}"
        )));
    }
    let source = read_source(bytes, model.format)?;
    let keys = source.keys.clone();
    check_mapping(model, &keys)?;
    // Terminals by body key, from the explicit lead map only.
    let mut terminals: BTreeMap<&str, u32> = BTreeMap::new();
    for entry in &use_.lead_map {
        let lead = model.leads.get(usize::from(entry.lead)).ok_or_else(|| {
            Error::Invalid(format!(
                "model.leads: the lead map names lead {} of {} tagged",
                entry.lead,
                model.leads.len()
            ))
        })?;
        let (key, _) = split_tag(&lead.key, &keys)?;
        if terminals
            .insert(key, entry.terminal.0)
            .is_some_and(|t| t != entry.terminal.0)
        {
            return Err(Error::Invalid(format!(
                "model.leads: body `{key}` is mapped to two terminals"
            )));
        }
    }
    let names = add_source(native, component, source, &model.frame, model.format)?;
    let file = &model.asset.file_name;
    let built = bounds_of(native, &names)?;
    if !(close(built[0], model.bounds_mm[0]) && close(built[1], model.bounds_mm[1])) {
        return Err(Error::Invalid(format!(
            "model.asset: {file} ({}) builds to bounds {built:?} mm, but the model records \
             {:?} mm",
            model.asset.hash, model.bounds_mm
        )));
    }
    for (lead, found) in model
        .leads
        .iter()
        .zip(contacts(native, model, &keys, &names)?)
    {
        if !close(found, lead.contact) {
            return Err(Error::Invalid(format!(
                "model.leads: lead `{}` of {file} touches down at {found:?} mm, but the model \
                 records {:?} mm",
                lead.designation, lead.contact
            )));
        }
    }
    let by_key: BTreeMap<&str, &pcbcraft_core::library::model3d::ModelBody> =
        model.bodies.iter().map(|b| (b.key.as_str(), b)).collect();
    let mut bodies = Vec::with_capacity(names.len());
    for (key, name) in keys.iter().zip(&names) {
        let mapping = by_key
            .get(key.as_str())
            .ok_or_else(|| Error::Verification(format!("model.build: {key} unmapped")))?;
        bodies.push(LibraryBody {
            name: name.clone(),
            key: key.clone(),
            role: mapping.role,
            material: mapping.material,
            terminal: terminals.get(key.as_str()).copied(),
        });
    }
    // One saved look per material.
    let mut by_material: BTreeMap<MaterialKey, Vec<&str>> = BTreeMap::new();
    for b in &bodies {
        by_material
            .entry(MaterialKey(b.material))
            .or_default()
            .push(&b.name);
    }
    for (MaterialKey(material), names) in by_material {
        match material {
            ModelMaterial::Builtin(id) => {
                let def = look::definition(id)?;
                native.look(&names, &look::saved(&def))?;
            }
            ModelMaterial::Custom(index) => {
                native.look(&names, &saved_custom(custom(model, index)?))?;
            }
        }
    }
    if let Some(error) = native.evaluation_error() {
        return Err(Error::Verification(format!(
            "model.build: {component} did not evaluate: {error}"
        )));
    }
    Ok(LibraryDefinition {
        component: component.to_owned(),
        bodies,
    })
}

/// [`ModelMaterial`] ordered for grouping.
#[derive(PartialEq, Eq)]
struct MaterialKey(ModelMaterial);

impl MaterialKey {
    fn rank(&self) -> (u8, u16, String) {
        match self.0 {
            ModelMaterial::Builtin(id) => (0, 0, format!("{id:?}")),
            ModelMaterial::Custom(i) => (1, i, String::new()),
        }
    }
}

impl PartialOrd for MaterialKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MaterialKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

/// The definition name of a library model's preview.
pub const PREVIEW_MODEL: &str = "pkglibpreview";

/// The Component 3D preview of a package's library model: the model built
/// on its own, moved by the package's `placement` (model → datum frame),
/// each body drawn with its material's product look.
pub fn preview(
    model: &ModelDefinition,
    use_: &LibraryModel,
    bytes: &[u8],
    placement: &ModelPlacement,
) -> Result<Vec<Geometry>> {
    if !placement.finite() {
        return Err(Error::Invalid(
            "package.placement: placement values must be finite and within ±10000".into(),
        ));
    }
    let mut native = Native::new("Library model")?;
    let built = build_library_model(&mut native, PREVIEW_MODEL, model, use_, bytes)?;
    let created = native
        .find_component(PREVIEW_MODEL)
        .ok_or_else(|| Error::Verification("the preview model has no occurrence".into()))?;
    let t = placement.transform();
    if t != Transform3::IDENTITY {
        let (r, p) = (t.rotation, t.translation);
        native.move_occurrence(
            created.occurrence,
            [
                [r[0][0], r[0][1], r[0][2], p[0]],
                [r[1][0], r[1][1], r[1][2], p[1]],
                [r[2][0], r[2][1], r[2][2], p[2]],
            ],
        )?;
    }
    let by_name: BTreeMap<&str, &LibraryBody> =
        built.bodies.iter().map(|b| (b.name.as_str(), b)).collect();
    let mut out = Vec::with_capacity(built.bodies.len());
    for body in native.session().world_state().bodies.iter() {
        let name = body.definition_name();
        let Some(lib) = by_name.get(name) else {
            continue;
        };
        let (surface, detail) = surface(model, lib.material)?;
        let m = body
            .measure()
            .map_err(|e| Error::Verification(e.to_string()))?;
        out.push(Geometry {
            item: None,
            kind: lib.kind(),
            layer: None,
            surface,
            detail,
            mesh: body.mesh(),
            volume: m.volume,
            min: [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
            max: [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
        });
    }
    if out.len() != built.bodies.len() {
        return Err(Error::Verification(format!(
            "model.build: the preview drew {} of {} bodies",
            out.len(),
            built.bodies.len()
        )));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "imported_tests.rs"]
pub(crate) mod tests;
