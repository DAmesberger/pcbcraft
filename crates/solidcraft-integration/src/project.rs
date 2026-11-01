//! Electronics → native Solidcraft. Parametric pads, traces and envelopes are
//! ordinary native features and planar joints, editable without Pcbcraft.
//!
//! The native model holds only physical bodies: the laminate, exact copper
//! and package envelopes. They are disjoint except where copper joins copper
//! (a track ending in its pad), so Solidcraft's interference checks and mass
//! properties of the saved file are physical. Soldermask, plated finish,
//! terminations and solder fillets are display-only layers
//! ([`crate::appearance`]) added to the drawn geometry, never to the file.
use crate::appearance::{self, Body, Land, Part, Path, ViaMark};
use crate::material::{self, BodyKind};
use crate::models::{self, ModelDefinition, look};
use crate::native::{BoardJoint, BoxBody, Created, Expr, JointFrame, Mount, Native, ProfileBody};
use crate::pad_outline::{self, Contour, Copper, FLATTEN_TOLERANCE_MM, PLATING_MM};
use crate::scene::{Geometry, Volume};
use crate::{Error, Result};
use pcbcraft_core::geometry::{Transform3, cos_sin};
use pcbcraft_core::library::{ModelPlacement, PackageDefinition, Pad, PadDefaults, PadShape};
use pcbcraft_core::{
    CopperLayer, Fabrication, Id, Layer, PadId, Placement, Point, Project, Stackup, ViaSpan,
    instances, layout,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

const METADATA_KEY: &str = "org.pcbcraft.electronics";
/// Body kinds: `org.pcbcraft.electronics.bodies.0`, `.1`, … each
/// `{kind: [body names]}`, below Solidcraft's 64 KiB value limit.
const BODIES_KEY: &str = "org.pcbcraft.electronics.bodies.";
const BODIES_CHUNK_BYTES: usize = 48 * 1024;
/// Solidcraft allows 256 metadata keys; leave room for others.
const MAX_BODY_CHUNKS: usize = 200;

/// Which Pcbcraft object a native body came from. Stored in the native file's
/// metadata so identity survives reopening. Absent fields are left out of
/// the file, so a board's sources fit Solidcraft's bounded metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub pick_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad: Option<PadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<Id>,
    /// The board via a via body came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<CopperLayer>,
    pub label: String,
    pub component: u64,
    /// The occurrence of a shared generated-model definition this source
    /// is; `None` for a component of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<u64>,
}

pub struct NativeModel {
    native: Native,
    pub geometry: Vec<Geometry>,
    pub sources: Vec<Source>,
    /// The generated package-model definitions, one per package revision.
    pub models: Vec<ModelDefinition>,
}

impl NativeModel {
    /// Write the model as a new `.solidcraft` file; refuses existing files.
    pub fn save(&self, path: &str) -> Result<()> {
        self.native.save_new(path)
    }

    /// The native design the model was built as.
    pub fn native(&self) -> &Native {
        &self.native
    }
}

/// A native design under construction, recording each body's kind.
struct Builder {
    native: Native,
    bodies: BTreeMap<String, BodyKind>,
    /// Generated package-model definitions built so far.
    models: Vec<ModelDefinition>,
    /// Definitions whose first occurrence an instance already holds.
    placed: BTreeSet<String>,
}

impl Builder {
    fn new(name: &str) -> Result<Self> {
        Ok(Self {
            native: Native::new(name)?,
            bodies: BTreeMap::new(),
            models: Vec::new(),
            placed: BTreeSet::new(),
        })
    }

    /// An occurrence of the generated definition `stem` of `package`,
    /// unmoved: the definition is built on first use (its first occurrence
    /// returned), then every further call places it once more.
    fn model(&mut self, stem: &str, package: &PackageDefinition) -> Result<Created> {
        if !self.models.iter().any(|d| d.component == stem) {
            let definition = models::build_definition(&mut self.native, stem, package)?;
            for body in &definition.bodies {
                self.record(&body.name, body.kind())?;
            }
            self.models.push(definition);
        }
        let created = self.native.find_component(stem).ok_or_else(|| {
            Error::Verification(format!("model definition {stem} has no occurrence"))
        })?;
        if self.placed.insert(stem.to_owned()) {
            return Ok(created);
        }
        Ok(Created {
            component: created.component,
            occurrence: self.native.copy_occurrence(created.component)?,
        })
    }

    fn param(&mut self, name: &str, value: f64, unit: &str) -> Result<()> {
        self.native.parameter(name, format!("{value} {unit}"))
    }

    /// Many parameters in one command (one by one, each addition evaluates
    /// every parameter and joint again).
    fn params(&mut self, params: &[(String, f64, &str)]) -> Result<()> {
        for chunk in params.chunks(MAX_IMPORTED_PARAMS) {
            let rows: Vec<serde_json::Value> = chunk
                .iter()
                .map(|(name, value, unit)| json!({"name": name, "expression": format!("{value} {unit}")}))
                .collect();
            self.native
                .import_parameters(&serde_json::to_string(&rows)?)?;
        }
        Ok(())
    }

    fn body(&mut self, kind: BodyKind, body: BoxBody<'_>) -> Result<()> {
        let name = body.name.to_owned();
        self.native.solid_box(body)?;
        self.record(&name, kind)
    }

    /// Record a created body's kind.
    fn record(&mut self, name: &str, kind: BodyKind) -> Result<()> {
        if self.bodies.insert(name.to_owned(), kind).is_some() {
            return Err(Error::Verification(format!("duplicate body name {name}")));
        }
        Ok(())
    }

    /// Store each kind's physical look (one assignment per kind) and the
    /// body → kind map, then evaluate.
    fn finish(mut self, sources: Vec<Source>) -> Result<NativeModel> {
        self.native.evaluate_deferred()?;
        self.native.activate_root()?;
        // Generated bodies already carry their materials' looks.
        for kind in BodyKind::ALL.into_iter().filter(|k| !k.is_generated()) {
            let names: Vec<&str> = self
                .bodies
                .iter()
                .filter(|(_, k)| **k == kind)
                .map(|(name, _)| name.as_str())
                .collect();
            // Saved bodies look the same for every fabrication; only the
            // display-only layers take the project's mask and finish.
            if !names.is_empty() {
                self.native
                    .appearance(&names, &material::material(kind, &Fabrication::default()))?;
            }
        }
        // Vias are recorded compactly on their own (a board has thousands
        // and Solidcraft's metadata is bounded); the rest one by one.
        let others: Vec<&Source> = sources.iter().filter(|s| s.via.is_none()).collect();
        let listed = source_chunks(&others)?;
        let mut listed = listed.into_iter();
        let first = listed.next().unwrap_or_default();
        self.native
            .set_metadata(METADATA_KEY, json!({"version": 1, "sources": first}))?;
        for (index, chunk) in listed.enumerate() {
            self.native.set_metadata(
                &format!("{SOURCES_KEY}{}", index + 1),
                json!({"sources": chunk}),
            )?;
        }
        for (index, chunk) in saved_vias(&sources)?.iter().enumerate() {
            self.native
                .set_metadata(&format!("{VIAS_KEY}{index}"), json!(chunk))?;
        }
        let chunks = kind_chunks(&self.bodies)?;
        for (index, chunk) in chunks.iter().enumerate() {
            self.native
                .set_metadata(&format!("{BODIES_KEY}{index}"), json!(chunk))?;
        }
        models::record(&mut self.native, &self.models)?;
        finish_with(self.native, sources, self.models)
    }
}

/// Board vias as the native file records them: `org.pcbcraft.electronics.vias.0`,
/// `.1`, … each a run of vias of one definition, in placement order, with
/// occurrence and via ids as differences from the one before and nets as
/// runs, so ten thousand vias take a few tens of KiB.
const VIAS_KEY: &str = "org.pcbcraft.electronics.vias.";
/// Vias per record: well below Solidcraft's 64 KiB a value even with
/// 20-digit ids and a net change at every via.
const VIAS_PER_RECORD: usize = 512;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct SavedVias {
    component: u64,
    /// Each occurrence less the one before (the first less 0).
    occurrences: Vec<i128>,
    /// Each via id less the one before.
    vias: Vec<i128>,
    /// `(net, count)`: the next `count` vias are on `net`.
    nets: Vec<(Id, usize)>,
}

/// Differences of consecutive values, the first from 0.
fn deltas(values: impl Iterator<Item = u64>) -> Vec<i128> {
    let mut last = 0i128;
    values
        .map(|v| {
            let v = i128::from(v);
            let d = v - last;
            last = v;
            d
        })
        .collect()
}

/// The values [`deltas`] were taken of; `None` when one leaves `u64`.
fn undelta(deltas: &[i128]) -> Option<Vec<u64>> {
    let mut last = 0i128;
    deltas
        .iter()
        .map(|d| {
            last = last.checked_add(*d)?;
            u64::try_from(last).ok()
        })
        .collect()
}

/// The via sources as records of [`VIAS_PER_RECORD`] vias of one
/// definition each.
fn saved_vias(sources: &[Source]) -> Result<Vec<SavedVias>> {
    let vias: Vec<&Source> = sources.iter().filter(|s| s.via.is_some()).collect();
    let mut out = Vec::new();
    let mut runs: BTreeMap<u64, Vec<&Source>> = BTreeMap::new();
    for source in vias {
        runs.entry(source.component).or_default().push(source);
    }
    for (component, run) in runs {
        for chunk in run.chunks(VIAS_PER_RECORD) {
            let occurrence = |s: &Source| {
                s.occurrence
                    .ok_or_else(|| Error::Verification(format!("{} has no occurrence", s.label)))
            };
            let occurrences = chunk
                .iter()
                .map(|s| occurrence(s))
                .collect::<Result<Vec<_>>>()?;
            let mut nets: Vec<(Id, usize)> = Vec::new();
            for s in chunk {
                let net = s.net.unwrap_or_default();
                match nets.last_mut() {
                    Some((last, count)) if *last == net => *count += 1,
                    _ => nets.push((net, 1)),
                }
            }
            out.push(SavedVias {
                component,
                occurrences: deltas(occurrences.into_iter()),
                vias: deltas(chunk.iter().filter_map(|s| s.via)),
                nets,
            });
        }
    }
    Ok(out)
}

/// Sources listed one by one past the first value's: `org.pcbcraft.electronics.sources.1`, `.2`, …
const SOURCES_KEY: &str = "org.pcbcraft.electronics.sources.";

/// `sources` split into lists of at most [`BODIES_CHUNK_BYTES`] serialized.
fn source_chunks<'a>(sources: &[&'a Source]) -> Result<Vec<Vec<&'a Source>>> {
    let mut chunks: Vec<Vec<&Source>> = vec![Vec::new()];
    let mut size = 2;
    for source in sources {
        let entry = serde_json::to_string(source)?.len() + 1;
        if entry + 2 > BODIES_CHUNK_BYTES {
            return Err(Error::Invalid(format!(
                "{} is too large to record",
                source.label
            )));
        }
        if size + entry > BODIES_CHUNK_BYTES {
            chunks.push(Vec::new());
            size = 2;
        }
        if let Some(chunk) = chunks.last_mut() {
            chunk.push(source);
            size += entry;
        }
    }
    Ok(chunks)
}

/// Every source a saved native model records: the objects listed one by
/// one, then its vias, numbered on from them.
pub fn saved_sources(native: &Native) -> Result<Vec<Source>> {
    let metadata = &native.session().doc.metadata.0;
    let listed = metadata
        .get(METADATA_KEY)
        .and_then(|m| m.get("sources"))
        .cloned()
        .ok_or_else(|| Error::Verification("native model has no Pcbcraft sources".into()))?;
    let mut sources: Vec<Source> = serde_json::from_value(listed)?;
    for index in 1usize.. {
        let Some(value) = metadata.get(&format!("{SOURCES_KEY}{index}")) else {
            break;
        };
        let listed = value.get("sources").cloned().unwrap_or_default();
        sources.extend(serde_json::from_value::<Vec<Source>>(listed)?);
    }
    for index in 0usize.. {
        let Some(value) = metadata.get(&format!("{VIAS_KEY}{index}")) else {
            break;
        };
        let saved: SavedVias = serde_json::from_value(value.clone())?;
        let broken = || Error::Verification(format!("via record {index} is inconsistent"));
        let occurrences = undelta(&saved.occurrences).ok_or_else(broken)?;
        let vias = undelta(&saved.vias).ok_or_else(broken)?;
        let nets: Vec<Id> = saved
            .nets
            .iter()
            .flat_map(|(net, count)| std::iter::repeat_n(*net, *count))
            .take(vias.len().saturating_add(1))
            .collect();
        if occurrences.len() != vias.len() || nets.len() != vias.len() {
            return Err(broken());
        }
        let mut pick = sources.iter().map(|s| s.pick_id).max().unwrap_or(0);
        for ((occurrence, via), net) in occurrences.into_iter().zip(vias).zip(nets) {
            pick = pick.saturating_add(1);
            sources.push(Source {
                pick_id: pick,
                instance: None,
                pad: None,
                track: None,
                via: Some(via),
                net: Some(net),
                layer: None,
                label: format!("Via {via}"),
                component: saved.component,
                occurrence: Some(occurrence),
            });
        }
    }
    Ok(sources)
}

/// The body → kind map split over metadata values: Solidcraft caps each
/// value at 64 KiB. Sizes are measured as serialized (escaping included).
fn kind_chunks(bodies: &BTreeMap<String, BodyKind>) -> Result<Vec<BTreeMap<BodyKind, Vec<&str>>>> {
    let mut chunks: Vec<BTreeMap<BodyKind, Vec<&str>>> = vec![BTreeMap::new()];
    let mut size = 2;
    for (name, kind) in bodies {
        // The escaped name and its comma, plus the kind's key, colon and
        // brackets when it is the chunk's first body of that kind.
        let entry = serde_json::to_string(name)?.len() + 1;
        let key = serde_json::to_string(kind)?.len() + 4;
        let new_kind = chunks.last().is_some_and(|c| !c.contains_key(kind));
        let needed = entry + if new_kind { key } else { 0 };
        if needed + 2 > BODIES_CHUNK_BYTES {
            return Err(Error::Invalid(format!("body name too long: {name}")));
        }
        if size + needed > BODIES_CHUNK_BYTES {
            chunks.push(BTreeMap::new());
            size = 2;
        }
        if let Some(chunk) = chunks.last_mut() {
            if !chunk.contains_key(kind) {
                size += key;
            }
            chunk.entry(*kind).or_default().push(name);
            size += entry;
        }
    }
    if chunks.len() > MAX_BODY_CHUNKS {
        return Err(Error::Invalid(
            "the model has too many bodies to record their kinds".into(),
        ));
    }
    Ok(chunks)
}

/// The body → kind map stored with the model.
fn body_kinds(native: &Native) -> Result<BTreeMap<String, BodyKind>> {
    let metadata = serde_json::to_value(&native.session().doc.metadata)?;
    let mut kinds = BTreeMap::new();
    for (key, value) in metadata.as_object().into_iter().flatten() {
        if !key.starts_with(BODIES_KEY) {
            continue;
        }
        let chunk: BTreeMap<BodyKind, Vec<String>> = serde_json::from_value(value.clone())?;
        for (kind, names) in chunk {
            for name in names {
                kinds.insert(name, kind);
            }
        }
    }
    if kinds.is_empty() {
        return Err(Error::Verification(
            "native model has no Pcbcraft body kinds".into(),
        ));
    }
    Ok(kinds)
}

/// The pick of every world body that places a shared generated-model
/// definition, by world body index. Solidcraft lists a design's world
/// bodies placement by placement (`placements()` order), each placement's
/// bodies in definition order; each run is checked against the
/// placement's transform.
fn shared_picks(native: &Native, sources: &[Source]) -> Result<BTreeMap<usize, u64>> {
    let by_occurrence: BTreeMap<u64, u64> = sources
        .iter()
        .filter_map(|s| s.occurrence.map(|o| (o, s.pick_id)))
        .collect();
    let shared: BTreeSet<u64> = sources
        .iter()
        .filter(|s| s.occurrence.is_some())
        .map(|s| s.component)
        .collect();
    let mut out = BTreeMap::new();
    if shared.is_empty() {
        return Ok(out);
    }
    let doc = &native.session().doc;
    let world = native.session().world_state();
    let placements = doc.placements();
    for component in shared {
        let paths: Vec<&Vec<u64>> = placements
            .iter()
            .filter(|(c, _, _)| *c == component)
            .map(|(_, path, _)| path)
            .collect();
        let bodies: Vec<usize> = world
            .bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.component == component)
            .map(|(i, _)| i)
            .collect();
        if paths.is_empty() || !bodies.len().is_multiple_of(paths.len()) {
            return Err(Error::Verification(format!(
                "component {component}: {} bodies over {} placements",
                bodies.len(),
                paths.len()
            )));
        }
        let per = bodies.len() / paths.len();
        for (k, index) in bodies.iter().enumerate() {
            let path = paths
                .get(k / per.max(1))
                .ok_or_else(|| Error::Verification("placement run out of range".into()))?;
            let body = world
                .bodies
                .get(*index)
                .ok_or_else(|| Error::Verification("world body out of range".into()))?;
            let expected = doc.path_transform(component, path);
            let matches = expected.is_some_and(|m| {
                m.iter()
                    .flatten()
                    .zip(body.source_transform.iter().flatten())
                    .all(|(a, b)| (a - b).abs() <= 1e-9 * (1.0 + a.abs()))
            });
            if !matches {
                return Err(Error::Verification(format!(
                    "body {} does not sit at placement {path:?}",
                    body.name
                )));
            }
            let pick = path
                .last()
                .and_then(|o| by_occurrence.get(o))
                .copied()
                .ok_or_else(|| {
                    Error::Verification(format!("placement {path:?} has no Pcbcraft source"))
                })?;
            out.insert(*index, pick);
        }
    }
    Ok(out)
}

#[cfg(test)]
fn finish(native: Native, sources: Vec<Source>) -> Result<NativeModel> {
    finish_with(native, sources, Vec::new())
}

/// A definition body's measure at one placement: volume, box (low, high)
/// and where that placement moved it.
type Measured = (Volume, [f64; 3], [f64; 3], [f64; 3]);

/// Via definitions by drill and diameter (their bits).
type ViaKey = (u64, u64);

/// [`finish`] for a model whose generated definitions are `models`.
fn finish_with(
    native: Native,
    sources: Vec<Source>,
    models: Vec<ModelDefinition>,
) -> Result<NativeModel> {
    if let Some(error) = native.evaluation_error() {
        return Err(Error::Verification(error));
    }
    let kinds = body_kinds(&native)?;
    let generated = models::recorded(&native)?;
    let shared = shared_picks(&native, &sources)?;
    let doc = &native.session().doc;
    // The first source of each component of its own, and each source by pick.
    let mut own: BTreeMap<u64, u64> = BTreeMap::new();
    let mut by_pick: BTreeMap<u64, &Source> = BTreeMap::new();
    for s in &sources {
        if s.occurrence.is_none() {
            own.entry(s.component).or_insert(s.pick_id);
        }
        by_pick.entry(s.pick_id).or_insert(s);
    }
    // Placements that only move a definition (every via) measure the same:
    // each definition body is measured once, at its first placement.
    let mut measured: BTreeMap<(u64, String), Measured> = BTreeMap::new();
    let mut geometry = vec![];
    // Every body's display mesh at once, on all cores (each the mesh it makes alone).
    native.session().world_state().warm_meshes(|| true);
    for (index, body) in native.session().world_state().bodies.iter().enumerate() {
        let name = body.definition_name();
        let kind = *kinds
            .get(name)
            .ok_or_else(|| Error::Verification(format!("body {name} has no Pcbcraft kind")))?;
        let saved = doc
            .model_body_look(body)
            .ok_or_else(|| Error::Verification(format!("body {name} has no look")))?
            .opaque_surface()
            .map_err(|e| Error::Verification(e.to_string()))?
            .ok_or_else(|| Error::Verification(format!("body {name} has no physical look")))?;
        // A generated body is drawn with its material's full product look
        // (coat, pattern); the file keeps the opaque projection.
        let (surface, detail) = if kind.is_generated() {
            let body = generated.get(name).ok_or_else(|| {
                Error::Verification(format!("generated body {name} has no recorded material"))
            })?;
            let def = look::definition(body.material)?;
            (look::surface(&def)?, look::detail(&def))
        } else {
            // Saved bodies' detail does not depend on the fabrication.
            (
                saved,
                crate::material::detail(kind, &pcbcraft_core::Fabrication::default()),
            )
        };
        let t = body.source_transform;
        let moved = t[3];
        let translation = t[0] == [1.0, 0.0, 0.0, 0.0]
            && t[1] == [0.0, 1.0, 0.0, 0.0]
            && t[2] == [0.0, 0.0, 1.0, 0.0]
            && moved[3] == 1.0;
        let key = (body.component, name.to_owned());
        let mesh = body.mesh();
        let (volume, min, max) = match measured.get(&key).filter(|_| translation) {
            Some((volume, min, max, at)) => {
                let shift = |p: &[f64; 3]| std::array::from_fn(|i| p[i] + moved[i] - at[i]);
                (volume.clone(), shift(min), shift(max))
            }
            None => {
                let b = mesh.bounds();
                let outline = body.body.outline_bounds();
                let span = b.min.dist(b.max).max(1.0);
                let near = |p: solidcraft_engine::geom::Vec3, q: solidcraft_engine::geom::Vec3| {
                    p.dist(q) <= span * 1e-12
                };
                // A box the mesh reaches exactly: the body's vertices and edges' controls
                // span no more than the mesh does (a laminate slab, a pad, a track),
                // so that box is the body's own. Its volume (meshing the body more
                // finely three times) is measured only if asked for; any other body
                // is measured now.
                let (volume, min, max) = if kind == BodyKind::Substrate
                    || (near(b.min, outline.min) && near(b.max, outline.max))
                {
                    let body = body.clone();
                    (
                        Volume::lazy(move || body.measure().map_or(f64::NAN, |m| m.volume)),
                        [b.min.x, b.min.y, b.min.z],
                        [b.max.x, b.max.y, b.max.z],
                    )
                } else {
                    let m = body
                        .measure()
                        .map_err(|e| Error::Verification(e.to_string()))?;
                    (
                        Volume::known(m.volume),
                        [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
                        [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
                    )
                };
                if translation {
                    measured.insert(
                        key,
                        (volume.clone(), min, max, [moved[0], moved[1], moved[2]]),
                    );
                }
                (volume, min, max)
            }
        };
        let item = shared
            .get(&index)
            .copied()
            .or_else(|| own.get(&body.component).copied());
        let layer = item
            .and_then(|pick| by_pick.get(&pick).copied())
            .filter(|_| !kind.is_envelope() && !kind.is_generated())
            .and_then(|s| s.layer)
            .and_then(|layer| body_layer(name, layer));
        geometry.push(Geometry {
            item,
            kind,
            layer,
            surface,
            detail,
            mesh,
            volume,
            min,
            max,
        });
    }
    Ok(NativeModel {
        native,
        geometry,
        sources,
        models,
    })
}

/// The copper layer of one body of a source on `layer`: a through-hole
/// pad's opposite land lies on the other side, and its plated barrels on
/// no single layer (they span the board, like a via's).
fn body_layer(name: &str, layer: CopperLayer) -> Option<CopperLayer> {
    if name.contains("_barrel_") {
        None
    } else if name.ends_with("_opposite") {
        layer.side().map(|side| other(side).into())
    } else {
        Some(layer)
    }
}

/// The joint origin that holds a generated definition (drawn in its model
/// frame) so the footprint frame lands on the board: the footprint frame
/// expressed in model coordinates, the inverse of `frame` (model →
/// footprint).
fn holding_frame(frame: &Transform3) -> JointFrame {
    let r = frame.rotation;
    let back = |v: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|i| r[0][i] * v[0] + r[1][i] * v[1] + r[2][i] * v[2])
    };
    let o = back(frame.translation);
    JointFrame {
        point: [-o[0], -o[1], -o[2]],
        z: back([0.0, 0.0, 1.0]),
        x: back([1.0, 0.0, 0.0]),
    }
}

/// The model in the frame a board joint holds: the package datum frame.
/// The board places the footprint's datum (`layout::placement_transform`
/// undoes it), so `model_frame(datum, placement)` lands, footprint frame
/// first, at `placement`'s datum-frame pose; the 2D drawing and checks use
/// `model_frame` in the footprint frame.
fn datum_frame(package: &PackageDefinition) -> Transform3 {
    package.placement.transform()
}

/// The display-only terminations, overcoat and fillets draw in the package
/// frame shifted by the placement: right while the placement does not turn.
fn package_frame(frame: &Transform3) -> bool {
    frame.rotation == Transform3::IDENTITY.rotation
}

fn next_pick(sources: &[Source]) -> u64 {
    u64::try_from(sources.len()).map_or(u64::MAX, |n| n.saturating_add(1))
}

/// Most drilled holes (pad holes and pad vias) on one board.
pub const MAX_DRILLS: usize = 65_536;

/// One placed pad and the frames its contours are drawn in.
struct PlacedPad<'a> {
    placed: layout::PlacedPad,
    placement: &'a Placement,
    designator: &'a str,
    /// The footprint's mask and paste defaults.
    defaults: PadDefaults,
    /// The pad origin on the board.
    origin: Point,
}

impl PlacedPad<'_> {
    fn pad(&self) -> &Pad {
        &self.placed.pad
    }

    /// Pad frame → footprint frame, as a vector (no translation).
    fn turn(&self, v: Point) -> Point {
        let (c, s) = cos_sin(self.pad().rotation_deg);
        [c * v[0] - s * v[1], s * v[0] + c * v[1]]
    }

    /// A pad-frame contour on the board.
    fn board(&self, contour: &Contour) -> Contour {
        let t = self.placed.transform;
        let position = self.pad().position;
        contour.mapped(
            |q| {
                let v = self.turn(q);
                t.apply([position[0] + v[0], position[1] + v[1]])
            },
            t.mirror,
        )
    }

    /// A pad-frame contour in the pad component's frame: the component is
    /// jointed to its layer's board face at [`Self::origin`], turned by the
    /// placement angle (and turned over on the bottom face).
    fn local(&self, contour: &Contour) -> Contour {
        let t = self.placed.transform;
        let bottom = self.placed.layer == Layer::Bottom;
        let (c, s) = cos_sin(-self.placement.rotation_deg);
        contour.mapped(
            |q| {
                let b = t.apply_vector(self.turn(q));
                let r = [c * b[0] - s * b[1], s * b[0] + c * b[1]];
                if bottom { [r[0], -r[1]] } else { r }
            },
            t.mirror != bottom,
        )
    }

    /// The pad origin relative to the placement, in the placement's turned
    /// frame (what the joint's expressions rotate by the placement angle).
    fn offset(&self) -> Point {
        let p = self.placement.position;
        let d = [self.origin[0] - p[0], self.origin[1] - p[1]];
        let (c, s) = cos_sin(-self.placement.rotation_deg);
        [c * d[0] - s * d[1], s * d[0] + c * d[1]]
    }

    /// Drilled holes, pad frame: the pad's hole and its vias.
    fn drills(&self) -> Vec<Contour> {
        let pad = self.pad();
        let mut out: Vec<Contour> = pad
            .hole
            .iter()
            .filter_map(|h| pad_outline::hole_contour(h, 0.0))
            .collect();
        out.extend(pad.vias.iter().map(|v| Contour::Circle {
            center: v.at,
            radius: v.drill / 2.0,
        }));
        out
    }

    /// Finished bores, pad frame: drills less their plating.
    fn bores(&self) -> Vec<Contour> {
        let pad = self.pad();
        let mut out: Vec<Contour> = pad.hole.iter().filter_map(pad_outline::bore).collect();
        out.extend(pad.vias.iter().filter_map(|v| {
            let radius = v.drill / 2.0 - PLATING_MM;
            (radius > 0.0).then_some(Contour::Circle {
                center: v.at,
                radius,
            })
        }));
        out
    }

    /// The plated barrels (drill outside, bore inside), pad frame.
    fn barrels(&self) -> Vec<[Contour; 2]> {
        let pad = self.pad();
        let mut out = Vec::new();
        if let Some(hole) = pad.hole.filter(|h| h.plated)
            && let (Some(drill), Some(bore)) = (
                pad_outline::hole_contour(&hole, 0.0),
                pad_outline::bore(&hole),
            )
        {
            out.push([drill, bore]);
        }
        for via in &pad.vias {
            let radius = via.drill / 2.0;
            if radius - PLATING_MM > 0.0 {
                out.push([
                    Contour::Circle {
                        center: via.at,
                        radius,
                    },
                    Contour::Circle {
                        center: via.at,
                        radius: radius - PLATING_MM,
                    },
                ]);
            }
        }
        out
    }
}

/// Every footprint pad of every placement, with or without a net or
/// terminal: they are all copper (or holes) on the board.
fn placed_pads(project: &Project) -> Result<Vec<PlacedPad<'_>>> {
    let placed = layout::placed_pads(project);
    if placed.len() > layout::MAX_PLACED_PADS {
        return Err(Error::Invalid(format!(
            "the board has more than {} pads",
            layout::MAX_PLACED_PADS
        )));
    }
    let drills: usize = placed
        .iter()
        .map(|p| usize::from(p.pad.hole.is_some()) + p.pad.vias.len())
        .sum();
    if drills > MAX_DRILLS {
        return Err(Error::Invalid(format!(
            "the board has more than {MAX_DRILLS} drilled holes"
        )));
    }
    placed
        .into_iter()
        .map(|placed| {
            let placement = project
                .placements
                .iter()
                .find(|p| p.instance == placed.instance)
                .ok_or_else(|| Error::Invalid("missing pad placement".into()))?;
            let instance = project
                .instance(placed.instance)
                .ok_or_else(|| Error::Invalid("missing pad instance".into()))?;
            let defaults = instances::resolve(&project.library, instance)
                .ok()
                .and_then(|r| r.physical)
                .map(|p| p.footprint.definition.defaults)
                .ok_or_else(|| Error::Invalid("missing pad definition".into()))?;
            let designator = instance.designator.as_str();
            let origin = placed.transform.apply(placed.pad.position);
            Ok(PlacedPad {
                placed,
                placement,
                designator,
                defaults,
                origin,
            })
        })
        .collect()
}

/// The layer on the other side of the board.
fn other(layer: Layer) -> Layer {
    match layer {
        Layer::Top => Layer::Bottom,
        Layer::Bottom => Layer::Top,
    }
}

/// A rectangular land without holes whose sides follow the component's
/// axes: drawn as a box with editable size parameters. The flag says its
/// length runs along the component's y axis.
fn axis_box(pad: &PlacedPad<'_>) -> Option<bool> {
    let p = pad.pad();
    if !matches!(p.shape, PadShape::Rect { .. }) || p.hole.is_some() || !p.vias.is_empty() {
        return None;
    }
    let axis = Contour::Circle {
        center: [1.0, 0.0],
        radius: 1.0,
    };
    let Contour::Circle { center: c, .. } = pad.local(&axis) else {
        return None;
    };
    if c[1].abs() < 1e-12 {
        Some(false)
    } else if c[0].abs() < 1e-12 {
        Some(true)
    } else {
        None
    }
}

fn flat(contour: &Contour) -> Result<Vec<Point>> {
    contour.flatten(FLATTEN_TOLERANCE_MM)
}

/// The parameter holding a copper layer's foil thickness. The top's is
/// `copper_thickness`; every other foil as thick follows it.
fn copper_param(layer: CopperLayer) -> String {
    match layer {
        CopperLayer::Top => "copper_thickness".into(),
        CopperLayer::Bottom => "copper_bottom_thickness".into(),
        CopperLayer::Inner(n) => format!("copper_inner_{n}_thickness"),
    }
}

/// The parameter holding `dielectrics[index]`'s thickness (from 1, top
/// down).
fn dielectric_param(index: usize) -> String {
    format!("dielectric_{}_thickness", index + 1)
}

/// The board height (an expression) of an inner layer's lower face: the
/// dielectrics below it, less its foil (inner foils lie at the top of the
/// dielectric below them, as `Stackup::copper_span` places them).
fn inner_base(stackup: &Stackup, layer: CopperLayer) -> Result<String> {
    let index = stackup
        .index(layer)
        .ok_or_else(|| Error::Invalid(format!("the stackup has no {} layer", layer.name())))?;
    let below: Vec<String> = (index..stackup.dielectrics.len())
        .map(dielectric_param)
        .collect();
    Ok(format!("{} - {}", below.join(" + "), copper_param(layer)))
}

/// The stackup's parameters: each dielectric and foil thickness, the board
/// (laminate) thickness as their sum, and the board outline.
fn stackup_params(b: &mut Builder, project: &Project) -> Result<()> {
    let stackup = &project.stackup;
    stackup.check().map_err(|e| Error::Invalid(e.to_string()))?;
    b.param("board_width", project.board_size[0], "mm")?;
    b.param("board_depth", project.board_size[1], "mm")?;
    if let [only] = stackup.dielectrics.as_slice() {
        b.param("board_thickness", only.thickness_mm, "mm")?;
    } else {
        let mut sum = Vec::new();
        for (index, dielectric) in stackup.dielectrics.iter().enumerate() {
            b.param(&dielectric_param(index), dielectric.thickness_mm, "mm")?;
            sum.push(dielectric_param(index));
        }
        b.native.parameter("board_thickness", sum.join(" + "))?;
    }
    let layers = stackup.layers();
    let top = stackup
        .copper
        .first()
        .map(|c| c.thickness_um)
        .ok_or_else(|| Error::Invalid("the stackup has no copper".into()))?;
    for (layer, foil) in layers.iter().zip(&stackup.copper) {
        let name = copper_param(*layer);
        if *layer == CopperLayer::Top {
            b.param(&name, top / 1000.0, "mm")?;
        } else if foil.thickness_um == top {
            b.native.parameter(&name, "copper_thickness")?;
        } else {
            b.param(&name, foil.thickness_um / 1000.0, "mm")?;
        }
    }
    Ok(())
}

/// A closed polyline as a sketch contour.
fn polygon(points: &[Point]) -> Option<Contour> {
    use pcbcraft_core::library::{Outline, Segment};
    let (first, rest) = points.split_first()?;
    let mut segments: Vec<Segment> = rest.iter().map(|&to| Segment::Line { to }).collect();
    segments.push(Segment::Line { to: *first });
    Some(Contour::Outline(Outline {
        start: *first,
        segments,
    }))
}

/// A track's copper rectangle on the board.
fn track_outline(start: Point, end: Point, width: f64) -> Vec<Point> {
    let d = [end[0] - start[0], end[1] - start[1]];
    let length = d[0].hypot(d[1]);
    if length <= 0.0 {
        return Vec::new();
    }
    let n = [-d[1] / length * width / 2.0, d[0] / length * width / 2.0];
    vec![
        [start[0] - n[0], start[1] - n[1]],
        [end[0] - n[0], end[1] - n[1]],
        [end[0] + n[0], end[1] + n[1]],
        [start[0] + n[0], start[1] + n[1]],
    ]
}

pub fn build(project: &Project) -> Result<NativeModel> {
    build_with(project, None)
}

/// [`build`] that stops with [`Error::Cancelled`] at its next native
/// command once `cancel` is set (a newer board replaced this one).
pub fn build_cancellable(
    project: &Project,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<NativeModel> {
    build_with(project, Some(cancel))
}

fn build_with(
    project: &Project,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<NativeModel> {
    pcbcraft_core::validate::project(project).map_err(|e| Error::Invalid(e.to_string()))?;
    if project
        .vias
        .iter()
        .any(|v| !matches!(v.span, ViaSpan::Through))
    {
        return Err(Error::Invalid(
            "blind and buried vias are not built in the native model yet".into(),
        ));
    }
    let stackup = &project.stackup;
    let mut b = Builder::new(&project.name)?;
    if let Some(cancel) = cancel {
        b.native.cancel_with(cancel);
    }
    // The board is evaluated once its parts are in, not after every command.
    b.native.defer_evaluation()?;
    stackup_params(&mut b, project)?;
    let substrate = b.native.component("PCB substrate", true)?;
    let board = substrate.occurrence;
    let pads = placed_pads(project)?;
    b.params(&object_params(project, &pads)?)?;
    let mut sources = vec![];
    let mut parts = vec![];
    // Board joints of every part, made at the end (see there).
    let mut joints: Vec<(BoardJoint, JointFrame, Option<String>)> = vec![];
    for placement in &project.placements {
        let instance = project
            .instance(placement.instance)
            .ok_or_else(|| Error::Invalid("missing instance".into()))?;
        let physical = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
            .ok_or_else(|| {
                Error::Invalid("placed instance has no physical implementation".into())
            })?;
        let package = &physical.package.definition;
        let footprint = &physical.footprint.definition;
        let prefix = format!("instance_{}", instance.id);
        if models::is_generated(package) {
            let stem = models::definition_name(physical.package.id, physical.package.revision.0);
            let created = b.model(&stem, package)?;
            let frame = datum_frame(package);
            let pick = next_pick(&sources);
            // Display-only solder fillets of a chip, while its placement
            // does not turn it out of the package frame they draw in.
            if package.chip_kind().is_some() && package_frame(&frame) {
                parts.push(Part {
                    pick: Some(pick),
                    package,
                    footprint: Some(footprint),
                    body: Body::of(package),
                    placement: Some(placement),
                    generated: true,
                });
            }
            joints.push((
                BoardJoint {
                    board,
                    occurrence: created.occurrence,
                    x: format!("{prefix}_x").into(),
                    y: format!("{prefix}_y").into(),
                    angle: format!("{prefix}_angle").into(),
                    mount: placement.layer.into(),
                },
                holding_frame(&frame),
                Some(copper_param(placement.layer.into())),
            ));
            sources.push(Source {
                pick_id: pick,
                instance: Some(instance.id),
                pad: None,
                track: None,
                via: None,
                net: None,
                layer: Some(placement.layer.into()),
                label: format!("{} model", instance.designator),
                component: created.component,
                occurrence: Some(created.occurrence),
            });
            continue;
        }
        let label = format!("{} envelope", instance.designator);
        let created = b.native.component(&label, false)?;
        let body = Body::of(package);
        // The box in its model frame (z = 0 the seating plane), held at
        // its placement in the datum frame as a generated model is.
        b.body(
            body.kind(),
            BoxBody {
                name: &label,
                corner: [
                    format!("- {prefix}_length / 2").into(),
                    format!("- {prefix}_width / 2").into(),
                    0.into(),
                ],
                size: [
                    format!("{prefix}_length").into(),
                    format!("{prefix}_width").into(),
                    format!("{prefix}_height").into(),
                ],
            },
        )?;
        let frame = datum_frame(package);
        joints.push((
            BoardJoint {
                board,
                occurrence: created.occurrence,
                x: format!("{prefix}_x").into(),
                y: format!("{prefix}_y").into(),
                angle: format!("{prefix}_angle").into(),
                mount: placement.layer.into(),
            },
            holding_frame(&frame),
            Some(copper_param(placement.layer.into())),
        ));
        let pick = next_pick(&sources);
        // The display-only terminations, overcoat and fillets are drawn in
        // the package frame: only while the model frame is that frame.
        if package_frame(&frame) {
            parts.push(Part {
                pick: Some(pick),
                package,
                footprint: Some(footprint),
                body,
                placement: Some(placement),
                generated: false,
            });
        }
        sources.push(Source {
            pick_id: pick,
            instance: Some(instance.id),
            pad: None,
            track: None,
            via: None,
            net: None,
            layer: Some(placement.layer.into()),
            label,
            component: created.component,
            occurrence: None,
        });
    }
    let mut lands = Vec::with_capacity(pads.len());
    for pad in &pads {
        let local = pad.pad();
        let pick = next_pick(&sources);
        let copper = pad_outline::copper(local);
        let layers: Vec<Layer> = match copper {
            Copper::None => vec![],
            Copper::OneSide => vec![pad.placed.layer],
            Copper::Through => vec![pad.placed.layer, other(pad.placed.layer)],
        };
        // Display: copper, bores and mask openings per outer layer.
        let shape = pad_outline::shape_contour(&local.shape, 0.0);
        let mask = pad_outline::mask_opening(local, &pad.defaults);
        let drills: Vec<Vec<Point>> = pad
            .drills()
            .iter()
            .map(|d| flat(&pad.board(d)))
            .collect::<Result<_>>()?;
        let bores = pad.bores();
        let opening_layers = if drills.is_empty() && copper != Copper::None {
            layers.clone()
        } else {
            vec![Layer::Top, Layer::Bottom]
        };
        for layer in opening_layers {
            let has_copper = layers.contains(&layer);
            let mut openings = drills.clone();
            if (has_copper || copper == Copper::None)
                && let Some(mask) = &mask
            {
                openings.push(flat(&pad.board(mask))?);
            }
            let (copper_outline, bore) = match (&shape, has_copper) {
                (Some(shape), true) => (
                    Some(flat(&pad.board(shape))?),
                    match bores.first() {
                        Some(bore) if local.hole.is_some() => Some(flat(&pad.board(bore))?),
                        _ => None,
                    },
                ),
                _ => (None, None),
            };
            lands.push(Land {
                pick: has_copper.then_some(pick),
                layer,
                copper: copper_outline,
                bore,
                openings,
            });
        }
        if copper == Copper::None {
            continue;
        }
        let Some(shape) = shape else {
            return Err(Error::Invalid(format!(
                "{} pad {} has no copper outline",
                pad.designator, local.id
            )));
        };
        let prefix = format!("instance_{}", pad.placed.instance);
        let pp = format!("pad_{}_{}", pad.placed.instance, local.id);
        let created = b.native.component(&pp, false)?;
        let near_copper = copper_param(pad.placed.layer.into());
        let far_copper = copper_param(other(pad.placed.layer).into());
        let land = |b: &mut Builder, name: &str, offset: Expr, height: &str| -> Result<()> {
            let mut contours = vec![pad.local(&shape)];
            contours.extend(bores.iter().map(|c| pad.local(c)));
            b.native.profile_body(ProfileBody {
                name,
                offset,
                contours: &contours,
                height: height.into(),
            })?;
            b.record(name, BodyKind::PadCopper)
        };
        match axis_box(pad) {
            Some(along_y) => {
                let (x, y) = if along_y {
                    (format!("{pp}_width"), format!("{pp}_length"))
                } else {
                    (format!("{pp}_length"), format!("{pp}_width"))
                };
                b.body(
                    BodyKind::PadCopper,
                    BoxBody {
                        name: &pp,
                        corner: [
                            format!("-{x} / 2").into(),
                            format!("-{y} / 2").into(),
                            0.into(),
                        ],
                        size: [x.into(), y.into(), near_copper.as_str().into()],
                    },
                )?;
            }
            None => land(&mut b, &pp, 0.into(), &near_copper)?,
        }
        if copper == Copper::Through {
            land(
                &mut b,
                &format!("{pp}_opposite"),
                format!("- board_thickness - {far_copper}").into(),
                &far_copper,
            )?;
        }
        for (index, [drill, bore]) in pad.barrels().iter().enumerate() {
            let name = format!("{pp}_barrel_{index}");
            b.native.profile_body(ProfileBody {
                name: &name,
                offset: "- board_thickness".into(),
                contours: &[pad.local(drill), pad.local(bore)],
                height: "board_thickness".into(),
            })?;
            b.record(&name, BodyKind::PadCopper)?;
        }
        let [dx, dy] = pad.offset();
        let x =
            format!("{prefix}_x + cos({prefix}_angle) * {dx} mm - sin({prefix}_angle) * {dy} mm");
        let y =
            format!("{prefix}_y + sin({prefix}_angle) * {dx} mm + cos({prefix}_angle) * {dy} mm");
        joints.push((
            BoardJoint {
                board,
                occurrence: created.occurrence,
                x: x.into(),
                y: y.into(),
                angle: format!("{prefix}_angle").into(),
                mount: pad.placed.layer.into(),
            },
            JointFrame::ORIGIN,
            None,
        ));
        sources.push(Source {
            pick_id: pick,
            instance: Some(pad.placed.instance),
            pad: Some(local.id),
            track: None,
            via: None,
            net: pad.placed.net,
            layer: Some(pad.placed.layer.into()),
            label: format!("{} pad {}", pad.designator, local.number),
            component: created.component,
            occurrence: None,
        });
    }
    let mut paths = Vec::with_capacity(project.tracks.len());
    for track in &project.tracks {
        let mount = match track.layer.side() {
            Some(side) => Mount::Side(side),
            None => Mount::Height(inner_base(stackup, track.layer)?.into()),
        };
        let prefix = format!("trace_{}", track.id);
        let created = b.native.component(&prefix, false)?;
        let length = format!("sqrt(({prefix}_x1-{prefix}_x0)^2 + ({prefix}_y1-{prefix}_y0)^2)");
        let angle = format!("atan2({prefix}_y1-{prefix}_y0; {prefix}_x1-{prefix}_x0)");
        b.body(
            BodyKind::CopperTrack,
            BoxBody {
                name: &prefix,
                corner: [0.into(), format!("-{prefix}_width / 2").into(), 0.into()],
                size: [
                    length.into(),
                    format!("{prefix}_width").into(),
                    copper_param(track.layer).into(),
                ],
            },
        )?;
        joints.push((
            BoardJoint {
                board,
                occurrence: created.occurrence,
                x: format!("{prefix}_x0").into(),
                y: format!("{prefix}_y0").into(),
                angle: angle.into(),
                mount,
            },
            JointFrame::ORIGIN,
            None,
        ));
        let pick = next_pick(&sources);
        // Inner copper is buried: no mask cap or finish over it.
        if let Some(side) = track.layer.side() {
            paths.push(Path {
                pick,
                start: track.start,
                end: track.end,
                width: track.width,
                layer: side,
            });
        }
        sources.push(Source {
            pick_id: pick,
            instance: None,
            pad: None,
            track: Some(track.id),
            via: None,
            net: Some(track.net),
            layer: Some(track.layer),
            label: format!("Trace {}", track.id),
            component: created.component,
            occurrence: None,
        });
    }
    let display = appearance::Board::of(project);
    let marks = vias(&mut b, project, &mut sources)?;
    for mark in &marks {
        lands.extend(mark.lands(&display));
    }
    let picks: BTreeMap<Id, u64> = sources
        .iter()
        .filter(|s| s.pad.is_none())
        .filter_map(|s| s.instance.map(|i| (i, s.pick_id)))
        .collect();
    // The display-only layers (mask, finish, silkscreen, edge) need no native
    // model: they are drawn while the native one gets its laminate and joints
    // and is finished and meshed.
    let layers = || -> Result<Vec<Geometry>> {
        let mut layers = appearance::board(&display, &lands, &paths, &marks, &parts)?;
        layers.extend(crate::silkscreen::films(
            project,
            &crate::silkscreen::Legend {
                board: &display,
                lands: &lands,
                paths: &paths,
                vias: &marks,
                picks: &picks,
            },
        )?);
        layers.extend(crate::stackup_edge::bands(stackup, &display)?);
        Ok(layers)
    };
    let native = |mut b: Builder| -> Result<NativeModel> {
        // The laminate last: its sketches hold every drill, which each
        // command before it would otherwise read again.
        b.native.activate(substrate.component)?;
        laminate(&mut b, project, &pads)?;
        // Every joint goes in once the parts are built, solved together (one
        // by one, each would solve all the joints made before it); a copy of
        // a shared definition starts where its first occurrence stands.
        b.native.board_joints(&joints)?;
        b.finish(sources)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (model, layers) = std::thread::scope(|scope| {
        let drawn = scope.spawn(layers);
        let model = native(b);
        let layers = drawn.join().unwrap_or_else(|_| {
            Err(Error::Verification(
                "drawing the board's layers failed".into(),
            ))
        });
        (model, layers)
    });
    #[cfg(target_arch = "wasm32")]
    let (model, layers) = (native(b), layers());
    let mut model = model?;
    model.geometry.extend(layers?);
    Ok(model)
}

/// One via definition per drill and diameter: its plated barrel through
/// the laminate and an annular ring on each copper layer, standing on the
/// board's bottom face at its origin. Every via of that size is an
/// occurrence of it, so a board of thousands of vias builds (and meshes)
/// each size of via once and places the rest.
struct ViaStyle {
    component: u64,
    /// Where the definition's first occurrence stands; `None` while it is
    /// unplaced, with that occurrence.
    origin: Option<Point>,
    first: u64,
}

/// The definition of `via`'s size.
fn via_style(b: &mut Builder, project: &Project, via: &pcbcraft_core::Via) -> Result<ViaStyle> {
    let stackup = &project.stackup;
    let layers = stackup.layers();
    let name = format!("via_{}_{}", mm_slug(via.drill), mm_slug(via.diameter));
    let created = b.native.component(&name, false)?;
    let drill = via.drill / 2.0;
    let bore = drill - PLATING_MM;
    let ring = via.diameter / 2.0;
    let circle = |radius| Contour::Circle {
        center: [0.0, 0.0],
        radius,
    };
    if bore > 0.0 {
        let body = format!("{name}_barrel");
        b.native.profile_body(ProfileBody {
            name: &body,
            offset: 0.into(),
            contours: &[circle(drill), circle(bore)],
            height: "board_thickness".into(),
        })?;
        b.record(&body, BodyKind::ViaCopper)?;
    }
    // Through vias join every layer.
    for layer in layers.iter().filter(|l| via.on(**l, &layers)) {
        if bore <= 0.0 && layer.side().is_some() {
            continue;
        }
        let body = format!("{name}_ring_{}", layer_slug(*layer));
        let height = copper_param(*layer);
        // Outer rings cap the barrel; inner rings stand round it, in the
        // circle the laminate's foil level leaves for them.
        let (offset, contours): (Expr, [Contour; 2]) = match layer {
            CopperLayer::Top => ("board_thickness".into(), [circle(ring), circle(bore)]),
            CopperLayer::Bottom => (format!("- {height}").into(), [circle(ring), circle(bore)]),
            CopperLayer::Inner(_) => (
                inner_base(stackup, *layer)?.into(),
                [circle(ring), circle(drill)],
            ),
        };
        b.native.profile_body(ProfileBody {
            name: &body,
            offset,
            contours: &contours,
            height: height.as_str().into(),
        })?;
        b.record(&body, BodyKind::ViaCopper)?;
    }
    Ok(ViaStyle {
        component: created.component,
        origin: None,
        first: created.occurrence,
    })
}

/// A length in body and component names: `0.3` → `0p3`.
fn mm_slug(value: f64) -> String {
    format!("{value}").replace('.', "p").replace('-', "m")
}

/// Every board via as saved copper: an occurrence of its size's definition
/// ([`via_style`]) moved to the via. Like the drills, a via sits where it
/// is now. Returns what the display layers draw of each.
fn vias(b: &mut Builder, project: &Project, sources: &mut Vec<Source>) -> Result<Vec<ViaMark>> {
    let mut styles: BTreeMap<ViaKey, ViaStyle> = BTreeMap::new();
    // Each via's definition, and the copies each definition still needs:
    // (via index, offset from the definition's first occurrence).
    let mut style_of = Vec::with_capacity(project.vias.len());
    let mut copies: BTreeMap<ViaKey, Vec<(usize, [f64; 3])>> = BTreeMap::new();
    let mut occurrences: Vec<Option<u64>> = vec![None; project.vias.len()];
    for (index, via) in project.vias.iter().enumerate() {
        let key = (via.drill.to_bits(), via.diameter.to_bits());
        let style = match styles.entry(key) {
            std::collections::btree_map::Entry::Occupied(style) => style.into_mut(),
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(via_style(b, project, via)?)
            }
        };
        let [x, y] = via.position;
        match style.origin {
            None => {
                b.native.activate_root()?;
                b.native.move_occurrence(
                    style.first,
                    [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, 0.0]],
                )?;
                style.origin = Some(via.position);
                occurrences[index] = Some(style.first);
            }
            Some([x0, y0]) => copies
                .entry(key)
                .or_default()
                .push((index, [x - x0, y - y0, 0.0])),
        }
        style_of.push(style.component);
    }
    // Every further via of a size is placed in one command.
    for (key, wanted) in copies {
        let style = styles
            .get(&key)
            .ok_or_else(|| Error::Verification("via definition missing".into()))?;
        let offsets: Vec<[f64; 3]> = wanted.iter().map(|(_, t)| *t).collect();
        let placed = b.native.copy_occurrences(style.component, &offsets)?;
        if placed.len() != wanted.len() {
            return Err(Error::Verification("via copies went missing".into()));
        }
        for ((index, _), occurrence) in wanted.into_iter().zip(placed) {
            if let Some(slot) = occurrences.get_mut(index) {
                *slot = Some(occurrence);
            }
        }
    }
    let mut marks = Vec::with_capacity(project.vias.len());
    for ((via, component), occurrence) in project.vias.iter().zip(style_of).zip(occurrences) {
        let occurrence =
            occurrence.ok_or_else(|| Error::Verification(format!("via {} is unplaced", via.id)))?;
        let pick = next_pick(sources);
        sources.push(Source {
            pick_id: pick,
            instance: None,
            pad: None,
            track: None,
            via: Some(via.id),
            net: Some(via.net),
            layer: None,
            label: format!("Via {}", via.id),
            component,
            occurrence: Some(occurrence),
        });
        marks.push(ViaMark {
            pick,
            at: via.position,
            drill: via.drill,
            diameter: via.diameter,
            tented: via.tented,
        });
    }
    Ok(marks)
}

/// A copper layer's name in body names.
fn layer_slug(layer: CopperLayer) -> String {
    match layer {
        CopperLayer::Top => "top".into(),
        CopperLayer::Bottom => "bottom".into(),
        CopperLayer::Inner(n) => format!("inner_{n}"),
    }
}

/// Most parameters per import.
const MAX_IMPORTED_PARAMS: usize = 8192;

/// Every placement's, pad's and track's own parameters, added together
/// before the objects that use them: each placement's position and angle
/// (and an envelope's size), an axis-aligned land's size, and each track's
/// ends and width.
fn object_params(
    project: &Project,
    pads: &[PlacedPad<'_>],
) -> Result<Vec<(String, f64, &'static str)>> {
    let mut out = Vec::new();
    for placement in &project.placements {
        let instance = project
            .instance(placement.instance)
            .ok_or_else(|| Error::Invalid("missing instance".into()))?;
        let physical = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
            .ok_or_else(|| {
                Error::Invalid("placed instance has no physical implementation".into())
            })?;
        let prefix = format!("instance_{}", instance.id);
        out.push((format!("{prefix}_x"), placement.position[0], "mm"));
        out.push((format!("{prefix}_y"), placement.position[1], "mm"));
        out.push((format!("{prefix}_angle"), placement.rotation_deg, "deg"));
        let package = &physical.package.definition;
        if !models::is_generated(package) {
            for (axis, value) in ["length", "width", "height"]
                .into_iter()
                .zip(package.envelope)
            {
                out.push((format!("{prefix}_{axis}"), value, "mm"));
            }
        }
    }
    for pad in pads {
        let local = pad.pad();
        if pad_outline::copper(local) == Copper::None
            || pad_outline::shape_contour(&local.shape, 0.0).is_none()
            || axis_box(pad).is_none()
        {
            continue;
        }
        let pp = format!("pad_{}_{}", pad.placed.instance, local.id);
        let [length, width] = local.shape.extent();
        out.push((format!("{pp}_length"), length, "mm"));
        out.push((format!("{pp}_width"), width, "mm"));
    }
    for track in &project.tracks {
        let prefix = format!("trace_{}", track.id);
        for (suffix, value) in [
            ("x0", track.start[0]),
            ("y0", track.start[1]),
            ("x1", track.end[0]),
            ("y1", track.end[1]),
            ("width", track.width),
        ] {
            out.push((format!("{prefix}_{suffix}"), value, "mm"));
        }
    }
    Ok(out)
}

/// The sum of `dielectrics[range]`'s thickness parameters, `0` for none.
fn dielectric_sum(stackup: &Stackup, range: std::ops::Range<usize>) -> String {
    if let [_] = stackup.dielectrics.as_slice() {
        return if range.is_empty() {
            "0 mm".into()
        } else {
            "board_thickness".into()
        };
    }
    let terms: Vec<String> = range.map(dielectric_param).collect();
    if terms.is_empty() {
        "0 mm".into()
    } else {
        terms.join(" + ")
    }
}

/// The laminate as the stack it is made of: each dielectric a slab
/// extruded from the board outline with every drill as a hole, and where
/// an inner foil lies at the top of a dielectric, the foil's level a slab
/// of its own whose holes are also that layer's copper (tracks and via
/// rings, united by the sketch where they overlap). The saved bodies are
/// disjoint and the laminate's mass is physical, without a Boolean: a
/// board of thousands of vias is built in time linear in their number.
/// Like the drills, the outline and the holes sit where the board and its
/// copper are now (they do not follow parameters edited in Solidcraft);
/// the slabs' heights follow the stackup parameters.
fn laminate(b: &mut Builder, project: &Project, pads: &[PlacedPad<'_>]) -> Result<()> {
    let stackup = &project.stackup;
    let [width, depth] = project.board_size;
    let outline = polygon(&[[0.0, 0.0], [width, 0.0], [width, depth], [0.0, depth]])
        .ok_or_else(|| Error::Invalid("the board has no outline".into()))?;
    let pad_drills: Vec<Contour> = pads
        .iter()
        .flat_map(|pad| pad.drills().into_iter().map(|d| pad.board(&d)))
        .collect();
    if pad_drills.len().saturating_add(project.vias.len()) > MAX_DRILLS {
        return Err(Error::Invalid(format!(
            "the board has more than {MAX_DRILLS} drilled holes"
        )));
    }
    let via_drills = project.vias.iter().map(|via| Contour::Circle {
        center: via.position,
        radius: via.drill / 2.0,
    });
    let mut dielectric = vec![outline.clone()];
    dielectric.extend(pad_drills.iter().cloned());
    dielectric.extend(via_drills);
    let layers = stackup.layers();
    let count = stackup.dielectrics.len();
    let mut slab = 0usize;
    let mut name = || {
        slab += 1;
        if slab == 1 {
            "Substrate".to_owned()
        } else {
            format!("Substrate {slab}")
        }
    };
    for index in 0..count {
        let low = dielectric_sum(stackup, index + 1..count);
        let thickness = dielectric_sum(stackup, index..index + 1);
        // An inner foil at this dielectric's top (`inner_base`).
        let foil = layers
            .iter()
            .copied()
            .find(|l| l.side().is_none() && stackup.index(*l) == Some(index));
        let Some(layer) = foil else {
            let name = name();
            b.native.batched_body(ProfileBody {
                name: &name,
                offset: low.as_str().into(),
                contours: &dielectric,
                height: thickness.as_str().into(),
            })?;
            b.record(&name, BodyKind::Substrate)?;
            continue;
        };
        // The foil's level: its copper is cut out with the pad drills (via
        // drills lie inside their rings).
        let mut level = vec![outline.clone()];
        level.extend(pad_drills.iter().cloned());
        level.extend(
            project
                .tracks
                .iter()
                .filter(|t| t.layer == layer)
                .filter_map(|t| polygon(&track_outline(t.start, t.end, t.width))),
        );
        level.extend(
            project
                .vias
                .iter()
                .filter(|v| v.on(layer, &layers))
                .map(|v| Contour::Circle {
                    center: v.position,
                    radius: v.diameter / 2.0,
                }),
        );
        let foil_name = name();
        let foil_height = copper_param(layer);
        b.native.batched_body(ProfileBody {
            name: &foil_name,
            offset: inner_base(stackup, layer)?.into(),
            contours: &level,
            height: foil_height.as_str().into(),
        })?;
        b.record(&foil_name, BodyKind::Substrate)?;
        let below = name();
        b.native.batched_body(ProfileBody {
            name: &below,
            offset: low.as_str().into(),
            contours: &dielectric,
            height: format!("{thickness} - {foil_height}").into(),
        })?;
        b.record(&below, BodyKind::Substrate)?;
    }
    Ok(())
}

/// The native envelope of one package definition, draft or published, with
/// the same terminations and overcoat a placed chip part shows.
pub fn preview(package: &PackageDefinition) -> Result<NativeModel> {
    if models::is_generated(package) {
        return preview_model(package);
    }
    if !package.envelope_valid() {
        return Err(Error::Invalid(
            "package envelope dimensions must be finite and positive".into(),
        ));
    }
    let mut b = Builder::new("Package envelope")?;
    for (name, value) in ["length", "width", "height"]
        .into_iter()
        .zip(package.envelope)
    {
        b.param(name, value, "mm")?;
    }
    let body = Body::of(package);
    let kind = match body {
        Body::Moulded => BodyKind::PartBody,
        chip => chip.kind(),
    };
    // The box in its model frame, moved by the placement as a generated
    // model's occurrence is.
    let created = b.native.component("Package envelope", false)?;
    b.body(
        kind,
        BoxBody {
            name: "Package envelope",
            corner: ["-length/2".into(), "-width/2".into(), 0.into()],
            size: ["length".into(), "width".into(), "height".into()],
        },
    )?;
    b.native.activate_root()?;
    move_by(&mut b.native, created.occurrence, &package.placement)?;
    let mut model = b.finish(vec![])?;
    // Display-only terminations and overcoat are drawn in the package
    // frame, shifted but not turned.
    if package.placement.transform().rotation == Transform3::IDENTITY.rotation {
        model.geometry.extend(appearance::preview(&Part {
            pick: None,
            package,
            footprint: None,
            body,
            placement: None,
            generated: false,
        })?);
    }
    Ok(model)
}

/// Move an unmoved occurrence by a model placement (model → datum frame).
fn move_by(native: &mut Native, occurrence: u64, placement: &ModelPlacement) -> Result<()> {
    let t = placement.transform();
    if t == Transform3::IDENTITY {
        return Ok(());
    }
    let r = t.rotation;
    let p = t.translation;
    native.move_occurrence(
        occurrence,
        [
            [r[0][0], r[0][1], r[0][2], p[0]],
            [r[1][0], r[1][1], r[1][2], p[1]],
            [r[2][0], r[2][1], r[2][2], p[2]],
        ],
    )
}

/// The definition name of an unpublished (draft) package's model.
pub const DRAFT_MODEL: &str = "pkgdraft";

/// The generated model of one package, draft or published, at its
/// placement in the datum frame (z = 0 the seating plane).
fn preview_model(package: &PackageDefinition) -> Result<NativeModel> {
    if !package.placement.finite() {
        return Err(Error::Invalid(
            "package.placement: placement values must be finite and within ±10000".into(),
        ));
    }
    let mut b = Builder::new("Package model")?;
    let created = b.model(DRAFT_MODEL, package)?;
    move_by(&mut b.native, created.occurrence, &package.placement)?;
    b.finish(vec![])
}

#[cfg(test)]
mod realism_tests;
#[cfg(test)]
mod tests;
