//! Parts in the live view's model ([`super::Flat::Meshed`]): each package
//! revision's model is a Solidcraft design of its own (its generated
//! model, library model, part file or envelope box, unmoved in a session
//! that holds nothing else), evaluated and meshed once and shared by
//! every placement of that revision. A placement draws the design's
//! meshes where the document's transform puts them
//! ([`pcbcraft_core::interface::package::model_to_board`]): the pose the
//! exact build's board joints solve, so no design of the board is made.
//!
//! [`Memo`] keeps the designs between models, so an edit evaluates no
//! part again, and a design is made again only for a package revision
//! (or model) it has not seen.
use super::{Builder, NativeModel, PartModel, PlacedPart, body_layer, digest, on_threads};
use crate::appearance::Body;
use crate::geom::{Mesh, Vec3};
use crate::models::ModelDefinition;
use crate::models::imported::SourceCache;
use crate::native::BoxBody;
use crate::scene::{Geometry, Place};
use crate::{Error, Result};
use pcbcraft_core::CopperLayer;
use pcbcraft_core::geometry::Transform3;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

/// The part designs made before, by a digest of what each was made from
/// ([`key`]). [`prepare`] takes each from here instead of making it again
/// and leaves here the designs it used.
#[derive(Clone, Default)]
pub struct Memo {
    held: HashMap<u64, Arc<Design>>,
}

impl Memo {
    /// Designs held.
    pub fn len(&self) -> usize {
        self.held.len()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

/// One package revision's model as an evaluated design of its own.
pub(super) struct Design {
    /// The part's own model: a session holding the part alone, unmoved,
    /// so its frame is the model frame. Its geometry's volumes measure
    /// the bodies it holds.
    model: NativeModel,
    /// Each body's name, in [`NativeModel::geometry`] order.
    names: Vec<String>,
}

impl Design {
    /// The generated, library-model or part-file definitions the design
    /// holds (none for an envelope).
    pub(super) fn definitions(&self) -> &[ModelDefinition] {
        &self.model.models
    }

    /// The design's bodies placed by `pose` (model frame → board) and
    /// picked as `pick`: each shares the design's mesh, and says where it
    /// lies. A body that is neither an envelope nor a generated body lies
    /// on the placement's copper `layer`, as the exact build files it.
    pub(super) fn place(&self, pose: &Transform3, pick: u64, layer: CopperLayer) -> Vec<Geometry> {
        let to_board = place_of(pose);
        self.model
            .geometry
            .iter()
            .zip(&self.names)
            .map(|(g, name)| {
                let place = g
                    .place
                    .as_ref()
                    .map_or(to_board, |inner| then(&to_board, inner));
                let (min, max) = bounds(&g.mesh, &place).unwrap_or_else(|| {
                    // No triangles: the design's own box, moved.
                    corners_bounds(&to_board, g.min, g.max)
                });
                let on_layer = !g.kind.is_envelope() && !g.kind.is_generated();
                Geometry {
                    item: Some(pick),
                    layer: on_layer.then(|| body_layer(name, layer)).flatten(),
                    place: Some(place),
                    min,
                    max,
                    ..g.clone()
                }
            })
            .collect()
    }
}

/// What a part's design is made from, as a digest: the package revision
/// (its definition, which holds the model's use and values) and, for a
/// library model, the model it names.
fn key(part: &PlacedPart<'_>) -> u64 {
    match part.model {
        PartModel::Library((model, use_, _)) => {
            digest(&("library", &part.stem, part.package, model, use_))
        }
        PartModel::PartFile(use_, _) => digest(&("part file", &part.stem, part.package, use_)),
        PartModel::Generated => digest(&("generated", &part.stem, part.package)),
        PartModel::Envelope => digest(&("envelope", &part.stem, part.package)),
    }
}

/// The design of each of `parts`, in order: taken from `held`, or made
/// (each design not held once, on every core). Returns the memo of the
/// designs used, for the next call.
pub(super) fn prepare(
    held: &Memo,
    parts: &[PlacedPart<'_>],
    library: &SourceCache,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<(Memo, Vec<Arc<Design>>)> {
    let keys: Vec<u64> = parts.iter().map(key).collect();
    let mut wanted: Vec<(u64, &PlacedPart<'_>)> = Vec::new();
    let mut seen = HashSet::new();
    for (part, key) in parts.iter().zip(&keys) {
        if !held.held.contains_key(key) && seen.insert(*key) {
            wanted.push((*key, part));
        }
    }
    let slots: Vec<OnceLock<Result<Design>>> = wanted.iter().map(|_| OnceLock::new()).collect();
    let indexed: Vec<usize> = (0..wanted.len()).collect();
    on_threads(&indexed, |&i| {
        if let (Some((_, part)), Some(slot)) = (wanted.get(i), slots.get(i)) {
            let _ = slot.set(make(part, library, cancel));
        }
    });
    let mut next = Memo::default();
    for ((key, part), slot) in wanted.iter().zip(slots) {
        let design = slot.into_inner().ok_or_else(|| {
            Error::Verification(format!("the design of {} was not made", part.stem))
        })??;
        next.held.insert(*key, Arc::new(design));
    }
    let mut designs = Vec::with_capacity(parts.len());
    for key in keys {
        let design = next
            .held
            .get(&key)
            .or_else(|| held.held.get(&key))
            .cloned()
            .ok_or_else(|| Error::Verification("a part design went missing".into()))?;
        next.held.insert(key, Arc::clone(&design));
        designs.push(design);
    }
    Ok((next, designs))
}

/// The design of `part`'s package revision, built in a session of its
/// own: the model's definition (built as the exact build builds it) and
/// its first occurrence, unmoved; or the envelope box in its model frame
/// (z = 0 the seating plane).
fn make(
    part: &PlacedPart<'_>,
    library: &SourceCache,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<Design> {
    let mut b = Builder::new(&part.stem, library)?;
    if let Some(cancel) = cancel {
        b.native.cancel_with(Arc::clone(cancel));
    }
    match part.model {
        PartModel::Library(model) => {
            b.library_model(&part.stem, model)?;
        }
        PartModel::PartFile(use_, bytes) => {
            let stem = b.define_part(&part.stem, use_, bytes)?;
            b.model_occurrence(&stem)?;
        }
        PartModel::Generated => {
            b.model(&part.stem, part.package)?;
        }
        PartModel::Envelope => {
            if !part.package.envelope_valid() {
                return Err(Error::Invalid(
                    "package envelope dimensions must be finite and positive".into(),
                ));
            }
            for (name, value) in ["length", "width", "height"]
                .into_iter()
                .zip(part.package.envelope())
            {
                b.param(name, value, "mm")?;
            }
            let name = format!("{} envelope", part.stem);
            b.native.component(&name, false)?;
            b.body(
                Body::of(part.package).kind(),
                BoxBody {
                    name: &name,
                    corner: ["-length/2".into(), "-width/2".into(), 0.into()],
                    size: ["length".into(), "width".into(), "height".into()],
                },
            )?;
        }
    }
    let model = b.finish(Vec::new())?;
    let world = model
        .world()
        .ok_or_else(|| Error::Verification(format!("the design of {} has no world", part.stem)))?;
    let names: Vec<String> = world
        .bodies
        .iter()
        .map(|b| b.definition_name().to_owned())
        .collect();
    if names.len() != model.geometry.len() {
        return Err(Error::Verification(format!(
            "the design of {} has {} bodies and {} meshes",
            part.stem,
            names.len(),
            model.geometry.len()
        )));
    }
    Ok(Design { model, names })
}

/// A transform as a placement matrix.
fn place_of(t: &Transform3) -> Place {
    let r = t.rotation;
    let p = t.translation;
    Place([
        [r[0][0], r[0][1], r[0][2], p[0]],
        [r[1][0], r[1][1], r[1][2], p[1]],
        [r[2][0], r[2][1], r[2][2], p[2]],
    ])
}

/// `outer ∘ inner`: `inner` first, then `outer`.
fn then(outer: &Place, inner: &Place) -> Place {
    let (a, b) = (&outer.0, &inner.0);
    Place(std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let turned: f64 = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            if j == 3 { turned + a[i][3] } else { turned }
        })
    }))
}

/// The box of `mesh` moved by `place`; `None` without points.
fn bounds(mesh: &Mesh, place: &Place) -> Option<([f64; 3], [f64; 3])> {
    let mut points = mesh.positions.iter().map(|p| place.point(*p));
    let first = points.next()?;
    let mut min = [first.x, first.y, first.z];
    let mut max = min;
    for p in points {
        for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
            min[k] = min[k].min(v);
            max[k] = max[k].max(v);
        }
    }
    Some((min, max))
}

/// The box of the box `min`..`max` moved by `place`.
fn corners_bounds(place: &Place, min: [f64; 3], max: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for corner in 0..8u8 {
        let pick = |k: u8, axis: usize| {
            if corner & k == 0 {
                min[axis]
            } else {
                max[axis]
            }
        };
        let p = place.point(Vec3::new(pick(1, 0), pick(2, 1), pick(4, 2)));
        for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
            lo[k] = lo[k].min(v);
            hi[k] = hi[k].max(v);
        }
    }
    (lo, hi)
}

/// How far a placed part's drawn vertex may lie from where the exact
/// build's solved joint puts it, mm (1 µm).
pub const PLACEMENT_MM: f64 = 1e-3;

/// How the parts of a meshed model lie against the exact build's
/// ([`parity`]).
#[derive(Clone, Debug, Default)]
pub struct Parity {
    /// Where they differ, in words; empty when they agree.
    pub differences: Vec<String>,
    /// Part bodies compared.
    pub bodies: usize,
    /// Vertices compared.
    pub vertices: usize,
    /// The farthest a compared vertex lies from the exact build's, mm.
    pub worst_mm: f64,
}

/// Each placed part's bodies in `live` (a meshed model, placed by the
/// document's transforms) against `exact` (the exact build, placed by
/// Solidcraft's solved board joints): the same parts under the same picks,
/// each with the same bodies of the same kinds and layers, and every
/// vertex of every body within [`PLACEMENT_MM`] of the exact body's.
pub fn parity(exact: &NativeModel, live: &NativeModel) -> Result<Parity> {
    let (want, have) = (part_bodies(exact), part_bodies(live));
    let mut out = Parity::default();
    for (pick, (label, bodies)) in &want {
        let Some((live_label, live_bodies)) = have.get(pick) else {
            out.differences
                .push(format!("{label}: placed exactly, not drawn"));
            continue;
        };
        if live_label != label {
            out.differences
                .push(format!("pick {pick}: {label} exactly, {live_label} drawn"));
            continue;
        }
        if bodies.len() != live_bodies.len() {
            out.differences.push(format!(
                "{label}: {} bodies exactly, {} drawn",
                bodies.len(),
                live_bodies.len()
            ));
            continue;
        }
        for (index, (a, b)) in bodies.iter().zip(live_bodies).enumerate() {
            out.bodies += 1;
            if a.kind != b.kind || a.layer != b.layer {
                out.differences.push(format!(
                    "{label} body {index}: {:?} on {:?} exactly, {:?} on {:?} drawn",
                    a.kind, a.layer, b.kind, b.layer
                ));
                continue;
            }
            let (a, b) = (a.world_mesh(), b.world_mesh());
            if a.positions.len() != b.positions.len() || a.positions.is_empty() {
                out.differences.push(format!(
                    "{label} body {index}: {} vertices exactly, {} drawn",
                    a.positions.len(),
                    b.positions.len()
                ));
                continue;
            }
            let worst = a
                .positions
                .iter()
                .zip(&b.positions)
                .map(|(p, q)| (p.x - q.x).hypot(p.y - q.y).hypot(p.z - q.z))
                .fold(0.0_f64, f64::max);
            out.vertices += a.positions.len();
            out.worst_mm = out.worst_mm.max(worst);
            if worst.is_nan() || worst > PLACEMENT_MM {
                out.differences.push(format!(
                    "{label} body {index}: a vertex lies {worst:.6} mm from the exact build's"
                ));
            }
        }
    }
    for (pick, (label, _)) in &have {
        if !want.contains_key(pick) {
            out.differences
                .push(format!("{label}: drawn, not placed exactly"));
        }
    }
    Ok(out)
}

/// Each part's label and bodies (display layers left out), by pick.
fn part_bodies(model: &NativeModel) -> std::collections::BTreeMap<u64, (String, Vec<&Geometry>)> {
    let mut out: std::collections::BTreeMap<u64, (String, Vec<&Geometry>)> =
        std::collections::BTreeMap::new();
    for source in model
        .sources
        .iter()
        .filter(|s| s.instance.is_some() && s.pad.is_none())
    {
        out.insert(source.pick_id, (source.label.clone(), Vec::new()));
    }
    for g in model.geometry.iter().filter(|g| !g.kind.is_appearance()) {
        if let Some((_, bodies)) = g.item.and_then(|pick| out.get_mut(&pick)) {
            bodies.push(g);
        }
    }
    out
}

#[cfg(test)]
mod tests;
