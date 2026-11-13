//! The exported board interface: the `.solidcraft` design a linked design
//! links (docs/architecture/board-interface.md, "The export format"; slice
//! B3). Built with Solidcraft's own commands and saved by its own writer.
//!
//! **Section 1, the slots, always first and in slot order.** Each slot uses
//! [`SLOT_FEATURE_BUDGET`] feature ids, padded with features made and
//! deleted, so a slot's ids, and with them its faces' names
//! (`F<feature>:side:<curve>`, `F<feature>:start`, `F<feature>:end`),
//! follow from its position alone: the same in every build, on every
//! machine and after every restart, whatever happened to other slots. A
//! retired or unresolved slot uses its ids too, and holds nothing.
//!
//! What carries each datum, as solid bodies named by the datum's token:
//!
//! | Datum | Carrier |
//! | --- | --- |
//! | the board outline | one slab body `board` with every cutout and mounting hole, its side faces named after the board sketch's curves and hole ids |
//! | board top and bottom, cutouts, holes | the slab's caps, cutout walls and hole walls (with no outline datum, a cutout is a prism and a hole a cylinder of its own) |
//! | a volume (an opening, an envelope, a side height) | a prism from its frame's plane along its z |
//! | an axis | a thin cylinder along it |
//! | another profile | a thin prism |
//! | a frame, plane or parameter | the sidecar only |
//!
//! The sidecar's `faces` name each datum's faces in this design; a copy
//! of the sidecar is kept in the design's metadata.
//!
//! **Section 2, after the slots,** for the mechanical and full tiers: the
//! board's model as [`crate::project`] builds it from the board plan,
//! without copper beyond the pads (mechanical) or with all of it (full).
//! Its feature ids follow the slots' and may change from build to build.
use crate::native::{NamedLoop, Native};
use crate::project;
use crate::{Error, Result};
use pcbcraft_core::interface::{DatumGeometry, Frame, Sidecar, SidecarDatum};
use pcbcraft_core::library::Outline;
use pcbcraft_core::model::Project;
use pcbcraft_core::model::board::RECT_EDGES;
use pcbcraft_core::model::interface::{DatumOwner, SLOT_FEATURE_BUDGET};
use std::collections::{BTreeMap, BTreeSet};

/// The board slab's body name.
pub const SLAB: &str = "board";
/// The component holding the slots' bodies, named as reference geometry so
/// a linked design can leave them out of mass, interference and its bill of
/// materials.
pub const REFERENCE_COMPONENT: &str = "Board interface (reference geometry)";
/// The design metadata key holding the sidecar: its format and chunk
/// count; the chunks follow under `<key>.0`, `<key>.1`, …, each as
/// `{"text": …}` (Solidcraft keeps a tool's values as objects).
pub const METADATA_KEY: &str = "org.pcbcraft.electronics.interface";
/// Bytes of sidecar text per metadata value, well under Solidcraft's
/// 64 KiB limit on one value (escaping a quote doubles it).
const CHUNK_BYTES: usize = 30_000;
/// Length and default diameter of an axis's cylinder, mm.
const AXIS_LENGTH_MM: f64 = 1.0;
const AXIS_DIAMETER_MM: f64 = 0.2;
/// Depth of a profile's thin prism, mm.
const PROFILE_DEPTH_MM: f64 = 0.01;
/// Named faces one datum lists (the sidecar's bound).
const MAX_FACES: usize = pcbcraft_core::interface::sidecar::MAX_FACES;

/// A built interface design and the sidecar describing it, faces named.
pub struct Exported {
    design: Design,
    pub sidecar: Sidecar,
    /// Section 2's bodies checked against the board plan
    /// ([`crate::export::check`]); `None` for the interface tier.
    pub check: Option<crate::export::Check>,
}

enum Design {
    Slots(Box<Native>),
    Board(Box<Native>),
}

impl Exported {
    /// The native design.
    pub fn native(&self) -> &Native {
        match &self.design {
            Design::Slots(native) => native,
            Design::Board(native) => native,
        }
    }

    /// The interface tier's design, to inspect it.
    #[cfg(test)]
    fn slots_mut(&mut self) -> Option<&mut Native> {
        match &mut self.design {
            Design::Slots(native) => Some(native),
            Design::Board(_) => None,
        }
    }

    /// Write the design as a new `.solidcraft` file; refuses existing files.
    pub fn save_new(&self, path: &str) -> Result<()> {
        self.native().save_new(path)
    }
}

/// What carries one slot's datum.
enum Carrier {
    /// Nothing in the design: the sidecar only.
    None,
    /// Carried by the board slab (built in the outline's slot).
    Slab,
    /// The slab itself.
    Board,
    /// A body of its own.
    Own,
}

fn plane(frame: &Frame) -> [[f64; 3]; 3] {
    [frame.origin.0, frame.x.0, frame.y.0]
}

/// An id Solidcraft can write into a face name whole, made unique in its
/// sketch (`l3`, then `l3_2` for a second segment of the same curve).
fn unique_id(wanted: &str, fallback: String, taken: &mut BTreeSet<String>) -> String {
    let fits = !wanted.is_empty()
        && wanted.len() <= 64
        && wanted
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    let base = if fits { wanted.to_owned() } else { fallback };
    let mut id = base.clone();
    let mut k = 2;
    while !taken.insert(id.clone()) {
        id = format!("{base}_{k}");
        k += 1;
    }
    id
}

/// One id per segment of `outline`: the board's curve ids when there is
/// one per segment, otherwise `<prefix><n>`.
fn loop_ids(
    outline: &Outline,
    curves: &[String],
    prefix: &str,
    taken: &mut BTreeSet<String>,
) -> Vec<String> {
    let given = curves.len() == outline.segments.len();
    (0..outline.segments.len())
        .map(|k| {
            let fallback = format!("{prefix}{}", k + 1);
            let wanted = if given {
                curves.get(k).cloned().unwrap_or_default()
            } else {
                fallback.clone()
            };
            unique_id(&wanted, fallback, taken)
        })
        .collect()
}

/// The board's outline curve ids: the sketch's, or the rectangle's edge
/// names.
fn outer_curves(project: &Project) -> Vec<String> {
    let board = project.board();
    if board.outer_curves.is_empty() && board.outer.segments.len() == RECT_EDGES.len() {
        return RECT_EDGES.iter().map(|e| (*e).to_owned()).collect();
    }
    board.outer_curves.iter().map(|c| c.0.clone()).collect()
}

/// The faces of an extrusion `feature` of named curves, as the design
/// names them: the cap on the sketch plane (`F<feature>:start`) as
/// `caps[0]`, the far cap as `caps[1]`, and a side per curve
/// (`F<feature>:side:<id>`) as `side:<id>`, at most [`MAX_FACES`]. Only
/// faces the body really has are listed.
fn prism_faces(
    native: &mut Native,
    body: &str,
    feature: u64,
    caps: [&str; 2],
    ids: &[String],
) -> Result<BTreeMap<String, String>> {
    let names: BTreeSet<String> = native.face_names(body)?.into_iter().collect();
    let start = format!("F{feature}:start");
    let side = format!("F{feature}:side:");
    let mut faces = BTreeMap::new();
    if names.contains(&start) {
        faces.insert(caps[0].to_owned(), start.clone());
    }
    let far: Vec<&String> = names
        .iter()
        .filter(|n| n.starts_with(&format!("F{feature}:")) && **n != start && !n.starts_with(&side))
        .collect();
    if let [far] = far.as_slice() {
        faces.insert(caps[1].to_owned(), (*far).clone());
    }
    for id in ids {
        if faces.len() >= MAX_FACES {
            break;
        }
        let name = format!("{side}{id}");
        if names.contains(&name) {
            faces.insert(format!("side:{id}"), name);
        }
    }
    Ok(faces)
}

/// Where the slab's pieces are named, once it is built.
#[derive(Default)]
struct Slab {
    feature: u64,
    /// The slab's faces by role (`top`, `bottom`, `side:<curve>`) and every
    /// other name its body has.
    faces: BTreeMap<String, String>,
    names: BTreeSet<String>,
    /// Cutout id → the cutout's curve ids.
    cutouts: BTreeMap<u32, Vec<String>>,
    /// Hole id → its circle's id.
    holes: BTreeMap<u32, String>,
}

/// Build the board slab: the outline, every cutout and every mounting
/// hole of the board, extruded through the board's thickness.
fn slab(native: &mut Native, project: &Project, datum: &SidecarDatum) -> Result<Slab> {
    let DatumGeometry::Profile { frame, loops } = &datum.geometry else {
        return Err(Error::Invalid(
            "the board outline datum is not a profile".into(),
        ));
    };
    let outer = loops
        .first()
        .ok_or_else(|| Error::Invalid("the board outline datum has no loop".into()))?;
    let board = project.board();
    let mut taken = BTreeSet::new();
    let outer_ids = loop_ids(outer, &outer_curves(project), "o", &mut taken);
    let mut named = vec![NamedLoop::Outline {
        outline: outer,
        ids: outer_ids.clone(),
    }];
    let mut cutouts = BTreeMap::new();
    for (k, cutout) in board.cutouts.iter().enumerate() {
        let curves: Vec<String> = cutout.curves.iter().map(|c| c.0.clone()).collect();
        let ids = loop_ids(
            &cutout.outline,
            &curves,
            &format!("c{}e", k + 1),
            &mut taken,
        );
        if let Some(id) = cutout.id {
            cutouts.insert(id.0, ids.clone());
        }
        named.push(NamedLoop::Outline {
            outline: &cutout.outline,
            ids,
        });
    }
    let mut holes = BTreeMap::new();
    for hole in &board.holes {
        let id = unique_id(
            &format!("h{}", hole.id.0),
            format!("h{}", hole.id.0),
            &mut taken,
        );
        holes.insert(hole.id.0, id.clone());
        named.push(NamedLoop::Circle {
            center: hole.at,
            radius: hole.drill / 2.0,
            id,
        });
    }
    let sketch = native.named_sketch(&format!("{} sketch", datum.token), plane(frame), &named)?;
    let feature = native.extrude_new(sketch, project.stackup.thickness_mm(), &datum.token, SLAB)?;
    let faces = prism_faces(native, SLAB, feature, ["bottom", "top"], &outer_ids)?;
    let names = native.face_names(SLAB)?.into_iter().collect();
    Ok(Slab {
        feature,
        faces,
        names,

        cutouts,
        holes,
    })
}

/// A frame whose z is `direction`, through `point`.
fn frame_along(point: [f64; 3], direction: [f64; 3]) -> Result<[[f64; 3]; 3]> {
    let len = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
    if len.is_nan() || len <= 1e-12 {
        return Err(Error::Invalid("an axis datum has no direction".into()));
    }
    let z = direction.map(|v| v / len);
    // Any axis not along z, made perpendicular to it.
    let seed = if z[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let d = seed[0] * z[0] + seed[1] * z[1] + seed[2] * z[2];
    let x = [seed[0] - d * z[0], seed[1] - d * z[1], seed[2] - d * z[2]];
    let lx = x.iter().map(|v| v * v).sum::<f64>().sqrt();
    let x = x.map(|v| v / lx);
    let y = [
        z[1] * x[2] - z[2] * x[1],
        z[2] * x[0] - z[0] * x[2],
        z[0] * x[1] - z[1] * x[0],
    ];
    Ok([point, x, y])
}

/// A body of its own for `datum`; returns its faces.
fn own(
    native: &mut Native,
    project: &Project,
    datum: &SidecarDatum,
) -> Result<BTreeMap<String, String>> {
    let token = datum.token.as_str();
    let sketch_name = format!("{token} sketch");
    let mut taken = BTreeSet::new();
    match &datum.geometry {
        DatumGeometry::Volume {
            frame,
            loops,
            height,
        } => {
            let ids = named_loops(loops, &mut taken);
            let named = as_named(loops, &ids);
            let sketch = native.named_sketch(&sketch_name, plane(frame), &named)?;
            let feature = native.extrude_new(sketch, *height, token, token)?;
            prism_faces(native, token, feature, ["front", "back"], &ids.concat())
        }
        DatumGeometry::Profile { frame, loops } => {
            // A cutout without the slab keeps the board sketch's curve ids,
            // and goes through the board.
            let (ids, depth) = match &datum.owner {
                DatumOwner::Cutout { id } => {
                    let board = project.board();
                    let curves: Vec<String> = board
                        .cutouts
                        .iter()
                        .find(|c| c.id == Some(*id))
                        .map(|c| c.curves.iter().map(|c| c.0.clone()).collect())
                        .unwrap_or_default();
                    let ids = match loops.first() {
                        Some(outline) => vec![loop_ids(outline, &curves, "e", &mut taken)],
                        None => Vec::new(),
                    };
                    (ids, project.stackup.thickness_mm())
                }
                _ => (named_loops(loops, &mut taken), PROFILE_DEPTH_MM),
            };
            let named = as_named(loops, &ids);
            let sketch = native.named_sketch(&sketch_name, plane(frame), &named)?;
            let feature = native.extrude_new(sketch, depth, token, token)?;
            prism_faces(native, token, feature, ["front", "back"], &ids.concat())
        }
        DatumGeometry::Axis {
            point,
            direction,
            diameter,
        } => {
            let (id, length) = match &datum.owner {
                DatumOwner::Hole { id } => (format!("h{}", id.0), project.stackup.thickness_mm()),
                _ => ("axis".to_owned(), AXIS_LENGTH_MM),
            };
            let radius = diameter.unwrap_or(AXIS_DIAMETER_MM) / 2.0;
            let named = [NamedLoop::Circle {
                center: [0.0, 0.0],
                radius,
                id: id.clone(),
            }];
            let sketch =
                native.named_sketch(&sketch_name, frame_along(point.0, direction.0)?, &named)?;
            let feature = native.extrude_new(sketch, length, token, token)?;
            prism_faces(native, token, feature, ["start", "end"], &[id])
        }
        DatumGeometry::Frame { .. }
        | DatumGeometry::Plane { .. }
        | DatumGeometry::Parameter { .. } => Ok(BTreeMap::new()),
    }
}

/// Generated curve ids for a datum's own loops: `e1`, … for the first,
/// `e2_1`, … for the others.
fn named_loops(loops: &[Outline], taken: &mut BTreeSet<String>) -> Vec<Vec<String>> {
    loops
        .iter()
        .enumerate()
        .map(|(k, outline)| {
            let prefix = if k == 0 {
                "e".to_owned()
            } else {
                format!("e{}_", k + 1)
            };
            loop_ids(outline, &[], &prefix, taken)
        })
        .collect()
}

fn as_named<'a>(loops: &'a [Outline], ids: &[Vec<String>]) -> Vec<NamedLoop<'a>> {
    loops
        .iter()
        .zip(ids.iter())
        .map(|(outline, ids)| NamedLoop::Outline {
            outline,
            ids: ids.clone(),
        })
        .collect()
}

fn carrier(datum: &SidecarDatum, has_slab: bool) -> Carrier {
    match datum.role.as_str() {
        "board.outline" => Carrier::Board,
        "board.top" | "board.bottom" if has_slab => Carrier::Slab,
        "board.cutout" | "hole" if has_slab => Carrier::Slab,
        _ => match &datum.geometry {
            DatumGeometry::Volume { height, .. } if *height > 0.0 => Carrier::Own,
            DatumGeometry::Profile { loops, .. } | DatumGeometry::Volume { loops, .. }
                if loops.is_empty() =>
            {
                Carrier::None
            }
            DatumGeometry::Profile { .. } | DatumGeometry::Axis { .. } => Carrier::Own,
            _ => Carrier::None,
        },
    }
}

/// The faces of a datum the slab carries, as its body names them.
fn slab_faces(datum: &SidecarDatum, slab: &Slab) -> BTreeMap<String, String> {
    let f = slab.feature;
    let mut faces = BTreeMap::new();
    let mut put = |role: String, name: String| {
        if slab.names.contains(&name) && faces.len() < MAX_FACES {
            faces.insert(role, name);
        }
    };
    match (datum.role.as_str(), &datum.owner) {
        ("board.top", _) => {
            if let Some(top) = slab.faces.get("top") {
                put("face".into(), top.clone());
            }
        }
        ("board.bottom", _) => {
            if let Some(bottom) = slab.faces.get("bottom") {
                put("face".into(), bottom.clone());
            }
        }
        ("hole", DatumOwner::Hole { id }) => {
            if let Some(circle) = slab.holes.get(&id.0) {
                put("wall".into(), format!("F{f}:side:{circle}"));
            }
        }
        ("board.cutout", DatumOwner::Cutout { id }) => {
            for curve in slab.cutouts.get(&id.0).into_iter().flatten() {
                put(format!("side:{curve}"), format!("F{f}:side:{curve}"));
            }
        }
        _ => {}
    }
    faces
}

/// The sidecar's text in metadata values: the header, then the chunks.
fn metadata(text: &str) -> Vec<(String, serde_json::Value)> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(CHUNK_BYTES);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (chunk, tail) = rest.split_at(end);
        chunks.push(chunk);
        rest = tail;
    }
    let mut values = vec![(
        METADATA_KEY.to_owned(),
        serde_json::json!({
            "format": pcbcraft_core::interface::FORMAT,
            "chunks": chunks.len(),
        }),
    )];
    for (k, chunk) in chunks.into_iter().enumerate() {
        values.push((
            format!("{METADATA_KEY}.{k}"),
            serde_json::json!({ "text": chunk }),
        ));
    }
    values
}

/// The sidecar a design's metadata holds, if it holds one.
pub fn read_metadata(native: &Native) -> Result<Option<Sidecar>> {
    let metadata = &native.session().doc.metadata.0;
    let Some(header) = metadata.get(METADATA_KEY) else {
        return Ok(None);
    };
    let count = header
        .get("chunks")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::Verification("the interface metadata has no chunk count".into()))?;
    let mut text = String::new();
    for k in 0..count.min(1024) {
        let chunk = metadata
            .get(&format!("{METADATA_KEY}.{k}"))
            .and_then(|v| v.get("text"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                Error::Verification(format!("interface metadata chunk {k} is missing"))
            })?;
        text.push_str(chunk);
    }
    Sidecar::read(&text)
        .map(Some)
        .map_err(|e| Error::Verification(e.to_string()))
}

/// Build the interface design of `project` from `sidecar` (the core's
/// `interface::publish::candidate`): the slots, then the tier's section 2.
/// Returns the design and the sidecar with every datum's faces named and
/// a copy in the design's metadata.
pub fn build(project: &Project, sidecar: Sidecar) -> Result<Exported> {
    build_watched(project, sidecar, &crate::export::Watch::new())
}

/// [`build`] counting its commands on `watch` and stopping with
/// [`Error::Cancelled`] once it is cancelled. Section 2's bodies are
/// checked against the board plan ([`crate::export::check`]) before the
/// design is returned.
pub fn build_watched(
    project: &Project,
    mut sidecar: Sidecar,
    watch: &std::sync::Arc<crate::export::Watch>,
) -> Result<Exported> {
    sidecar.check().map_err(|e| Error::Invalid(e.to_string()))?;
    watch.enter(crate::export::Stage::Building)?;
    let mut native = Native::new(&format!("{} interface", project.name))?;
    native.watch_with(std::sync::Arc::clone(watch));
    let reference = native.component(REFERENCE_COMPONENT, true)?;
    native.activate(reference.component)?;
    let budget = u64::from(SLOT_FEATURE_BUDGET);
    let base = native.next_id();
    let has_slab = sidecar.datums.iter().any(|d| d.role == "board.outline");
    let mut slab_built: Option<Slab> = None;
    let mut faces: BTreeMap<usize, BTreeMap<String, String>> = BTreeMap::new();
    let at = |slot: u32| -> Result<u64> {
        u64::from(slot)
            .checked_mul(budget)
            .and_then(|o| o.checked_add(base))
            .ok_or_else(|| Error::Invalid("interface: too many slots".into()))
    };
    let by_slot: BTreeMap<u32, usize> = sidecar
        .datums
        .iter()
        .enumerate()
        .map(|(index, d)| (d.slot.0, index))
        .collect();
    for slot in 0..sidecar.slot_count() {
        native.use_ids_to(at(slot)?)?;
        let found = by_slot
            .get(&slot)
            .and_then(|index| sidecar.datums.get(*index).map(|d| (*index, d)));
        if let Some((index, datum)) = found {
            match carrier(datum, has_slab) {
                Carrier::Board => {
                    let built = slab(&mut native, project, datum)?;
                    faces.insert(index, built.faces.clone());
                    slab_built = Some(built);
                }
                Carrier::Own => {
                    faces.insert(index, own(&mut native, project, datum)?);
                }
                Carrier::Slab | Carrier::None => {}
            }
        }
        native
            .use_ids_to(at(slot.saturating_add(1))?)
            .map_err(|_| {
                Error::Verification(format!(
                    "interface: slot {slot} needs more than its {SLOT_FEATURE_BUDGET} feature ids"
                ))
            })?;
    }
    native.activate_root()?;
    if let Some(error) = native.evaluation_error() {
        return Err(Error::Verification(format!(
            "the interface design does not evaluate: {error}"
        )));
    }
    for (index, datum) in sidecar.datums.iter_mut().enumerate() {
        datum.faces = match (carrier(datum, has_slab), &slab_built) {
            (Carrier::Slab, Some(slab)) => slab_faces(datum, slab),
            _ => faces.remove(&index).unwrap_or_default(),
        };
    }
    // Section 2, then the sidecar with the part each part body is (a
    // package's bodies are named by the package), into the design's
    // metadata.
    let (mut native, check, board) = match crate::export::section(project, sidecar.tier) {
        None => (native, None, false),
        Some(board) => {
            let model = project::build_onto(native, &board)?;
            let check = crate::export::check(&board, &model, watch)?;
            sidecar.parts = model.part_bodies(&board)?;
            let native = model.into_native().ok_or_else(|| {
                Error::Verification("the board's model has no native design".into())
            })?;
            (native, Some(check), true)
        }
    };
    sidecar
        .check()
        .map_err(|e| Error::Verification(e.to_string()))?;
    let text = sidecar
        .text()
        .map_err(|e| Error::Verification(e.to_string()))?;
    for (key, value) in metadata(&text) {
        native.set_metadata(&key, value)?;
    }
    let design = if board {
        Design::Board(Box::new(native))
    } else {
        Design::Slots(Box::new(native))
    };
    Ok(Exported {
        design,
        sidecar,
        check,
    })
}

#[cfg(test)]
mod tests;
