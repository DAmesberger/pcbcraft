//! Slice D2: inspection of an imported model file (STEP AP214/242, GLB,
//! `.solidcraft`) through Solidcraft's public readers: bodies with their
//! keys, source colours and glTF materials, bounds and texture presence.
//! The import wizard (D3) shows the result; nothing is mapped
//! automatically (material and lead suggestions are only proposals).
//!
//! STEP and glTF bodies are read exactly as [`super::imported`] builds
//! them, so the keys, bounds and contacts an inspection reports are the
//! ones a build rechecks. Bounds are in the model frame of the given
//! [`SourceFrame`] (the format's default frame for [`inspect`]).
use super::gltf_ext;
use super::imported::{self, Source};
use crate::native::Native;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sparksmith_core::library::model3d::{
    CustomMaterial, MAX_MODEL_BODIES, ModelFormat, SourceFrame,
};

/// Bytes one inspected file may have (the packed-asset bound, raised by
/// the D0 container).
pub const MAX_INSPECT_BYTES: usize = sparksmith_core::library::bundle::MAX_ASSET_BYTES;

/// One body of the source file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InspectedBody {
    /// Stable body path from the file's structure (see
    /// [`super::imported`] for the form).
    pub key: String,
    /// The file's colour for the body (linear RGBA), a hint only.
    pub colour: Option<[f64; 4]>,
    /// Index into `ModelInspection::materials` (glTF): the material whose
    /// factors the body carries, a suggestion only.
    pub material: Option<usize>,
    /// Model frame, mm.
    pub bounds_mm: [[f64; 3]; 2],
}

/// What a model file contains.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelInspection {
    pub format: ModelFormat,
    pub bodies: Vec<InspectedBody>,
    pub materials: Vec<CustomMaterial>,
    /// Union of the bodies, model frame, mm: what `ModelDefinition::
    /// bounds_mm` records.
    pub bounds_mm: [[f64; 3]; 2],
    pub has_textures: bool,
    /// The frame the bounds are in.
    #[serde(default)]
    pub frame: SourceFrame,
    /// What the reader could not read, and look extensions not shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Inspect `bytes` as `format` in the format's default frame
/// ([`imported::default_frame`]).
pub fn inspect(bytes: &[u8], format: ModelFormat) -> Result<ModelInspection> {
    inspect_in(bytes, format, &imported::default_frame(format))
}

/// Inspect `bytes` as `format` with bounds in `frame`.
pub fn inspect_in(
    bytes: &[u8],
    format: ModelFormat,
    frame: &SourceFrame,
) -> Result<ModelInspection> {
    if bytes.len() > MAX_INSPECT_BYTES {
        return Err(Error::Invalid(format!(
            "model.asset: the file has {} bytes; a model file has at most {MAX_INSPECT_BYTES}",
            bytes.len()
        )));
    }
    if bytes.is_empty() {
        return Err(Error::Invalid("model.asset: the file is empty".into()));
    }
    match format {
        ModelFormat::Step | ModelFormat::Gltf => read_built(bytes, format, frame),
        ModelFormat::Solidcraft => read_design(bytes, frame),
    }
}

/// STEP and glTF: built once in a scratch design, as a build does.
fn read_built(bytes: &[u8], format: ModelFormat, frame: &SourceFrame) -> Result<ModelInspection> {
    let looks = match format {
        ModelFormat::Gltf => gltf_ext::looks(gltf_ext::json_chunk(bytes)?)?,
        _ => gltf_ext::GltfLooks::default(),
    };
    let source: Source = imported::read_source(bytes, format)?;
    if source.keys.len() > MAX_MODEL_BODIES {
        return Err(too_many(source.keys.len()));
    }
    let keys = source.keys.clone();
    let colours = source.colours.clone();
    let body_looks = source.looks.clone();
    let mut warnings = source.warnings.clone();
    let mut native = Native::new("Inspection")?;
    let names = imported::add_source(&mut native, "pkginspect", source, frame, format)?;
    let state = native.session().model.state();
    let mut bodies = Vec::with_capacity(names.len());
    for (k, (key, name)) in keys.iter().zip(&names).enumerate() {
        let body = state
            .body(name)
            .ok_or_else(|| Error::Verification(format!("inspection: no body {name}")))?;
        let b = body.outline_bounds();
        let look = body_looks.get(k).and_then(Option::as_ref);
        bodies.push(InspectedBody {
            key: key.clone(),
            colour: colours.get(k).copied().flatten(),
            material: look.and_then(|l| material_of(l, &looks.materials)),
            bounds_mm: [[b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z]],
        });
    }
    let bounds_mm = imported::bounds_of(&native, &names)?;
    for e in &looks.ignored_extensions {
        warnings.push(format!("{e} is not shown"));
    }
    if looks.has_textures {
        warnings.push("textures are not shown: bodies take their materials' factors".into());
    }
    Ok(ModelInspection {
        format,
        bodies,
        materials: looks.materials,
        bounds_mm,
        has_textures: looks.has_textures,
        frame: *frame,
        warnings,
    })
}

/// The material whose factors a reader's look has: the same colour and
/// metallic-roughness, the same name preferred.
fn material_of(
    look: &solidcraft_engine::doc::appearance::Look,
    materials: &[CustomMaterial],
) -> Option<usize> {
    let srgb = |m: &CustomMaterial| {
        let [r, g, b, _] = m.base_colour;
        [r, g, b].map(crate::material::linear_to_srgb8)
    };
    let factors = |m: &CustomMaterial| match &look.pbr {
        Some(p) => {
            (p.metallic() - m.metallic).abs() < 1e-6 && (p.roughness() - m.roughness).abs() < 1e-6
        }
        None => true,
    };
    let candidates: Vec<usize> = materials
        .iter()
        .enumerate()
        .filter(|(_, m)| srgb(m) == look.color && factors(m))
        .map(|(i, _)| i)
        .collect();
    candidates
        .iter()
        .copied()
        .find(|i| materials.get(*i).is_some_and(|m| m.name == look.name))
        .or_else(|| candidates.first().copied())
}

/// `.solidcraft`: the design's evaluated bodies, keyed by body name.
fn read_design(bytes: &[u8], frame: &SourceFrame) -> Result<ModelInspection> {
    let doc = solidcraft_engine::io::read_design(bytes)
        .map_err(|e| Error::Invalid(format!("model.asset: {e}")))?;
    let session =
        solidcraft_engine::Session::opened_validating(doc, None, crate::native::build_validation());
    let mut warnings: Vec<String> = session
        .model
        .results
        .iter()
        .filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.name)))
        .collect();
    warnings.truncate(16);
    let state = crate::native::world_state(&session)?;
    let mut bodies = Vec::with_capacity(state.bodies.len());
    let mut union: Option<[[f64; 3]; 2]> = None;
    let mut seen = std::collections::BTreeSet::new();
    for body in &state.bodies {
        let key = body.name.clone();
        if !seen.insert(key.clone()) {
            return Err(Error::Invalid(format!(
                "model.asset: the design has two bodies named `{key}`"
            )));
        }
        let b = body.outline_bounds();
        let bounds = imported::frame_bounds(
            frame,
            ModelFormat::Solidcraft,
            [[b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z]],
        )?;
        union = Some(match union {
            None => bounds,
            Some([l, h]) => [
                [0, 1, 2].map(|i| l[i].min(bounds[0][i])),
                [0, 1, 2].map(|i| h[i].max(bounds[1][i])),
            ],
        });
        let colour = session.doc.model_body_look(body).map(|look| {
            let [r, g, b] = look.color.map(srgb8_to_linear);
            [r, g, b, look.opacity]
        });
        bodies.push(InspectedBody {
            key,
            colour,
            material: None,
            bounds_mm: bounds,
        });
    }
    let bounds_mm =
        union.ok_or_else(|| Error::Invalid("model.asset: the design has no bodies".into()))?;
    if bodies.len() > MAX_MODEL_BODIES {
        return Err(too_many(bodies.len()));
    }
    Ok(ModelInspection {
        format: ModelFormat::Solidcraft,
        bodies,
        materials: Vec::new(),
        bounds_mm,
        has_textures: false,
        frame: *frame,
        warnings,
    })
}

fn too_many(n: usize) -> Error {
    Error::Invalid(format!(
        "model.asset: the file has {n} bodies; a model maps at most {MAX_MODEL_BODIES}"
    ))
}

fn srgb8_to_linear(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
