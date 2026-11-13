//! The live view's board: the board plan's flat bodies ([`super::plan`])
//! meshed directly, with the mesher that draws the soldermask
//! ([`crate::appearance::slab`], and [`crate::appearance::prism`] for a
//! body of one convex loop), not built in the kernel. Each body is the
//! plan body's loops, flattened at [`FLATTEN_TOLERANCE_MM`], extruded
//! between its two heights; nothing is added, dropped or reshaped. The
//! laminate is its slabs, one body per slab as the exact build makes them,
//! each carrying its dielectric ([`Geometry::dielectric`]) so the 3D view
//! peels the stack one dielectric at a time.
//!
//! A body is meshed in its own frame and placed by the frame's turn and
//! move, so every via of a size, and every track of a length and width,
//! shares one mesh. [`Memo`] keeps each mesh by what it was made from, so
//! an edit meshes only the bodies it changed.
//!
//! Every mesh is picked as the document object its plan body came from:
//! its geometry carries that object's pick ([`super::Source`]); a
//! laminate slab carries none (the board). [`parity`] checks the meshes
//! against the plan.
use super::Source;
use super::plan::{BoardPlan, Level, Origin, PlanBody};
use crate::appearance::{Region, SMOOTH_TURN, prism, prism_points, slab};
use crate::geom::prelude::*;
use crate::geom::{Mesh, Vec3};
use crate::pad_outline::{Contour, FLATTEN_TOLERANCE_MM};
use crate::scene::{Geometry, Place, Volume};
use crate::{Error, Result};
use sparksmith_core::{Id, PadId, Point};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Faces of a large body (the laminate, a fill tile) are triangulated per
/// square of this size, as the soldermask's are: the slab mesher's cost
/// per tile grows faster than the edges in it.
pub const TILE_MM: f64 = 4.0;

/// A meshed body as made, before it is placed.
#[derive(Clone)]
struct Made {
    mesh: Arc<Mesh>,
    /// The mesh's box, in the body's frame.
    min: [f64; 3],
    max: [f64; 3],
    volume: f64,
    /// The flattened loops' area (the outer less the holes) and length.
    area: f64,
    perimeter: f64,
}

/// Meshes made before, by a digest of what each was made from: its kind,
/// heights and loops in its own frame. [`bodies`] takes each from here
/// instead of meshing it again and leaves here the meshes it used.
#[derive(Clone, Default)]
pub struct Memo {
    made: HashMap<u64, Made>,
}

impl Memo {
    /// Distinct meshes held.
    pub fn len(&self) -> usize {
        self.made.len()
    }

    pub fn is_empty(&self) -> bool {
        self.made.is_empty()
    }
}

/// One meshed flat body.
#[derive(Clone)]
pub struct Meshed {
    /// The plan body it was meshed from.
    pub source: Origin,
    pub geometry: Geometry,
    /// The flattened loops' area (the outer less each hole: a foil level's
    /// holes overlap, so not its area), mm², and their length, mm.
    pub area: f64,
    pub perimeter: f64,
}

/// What a call of [`bodies`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Bodies placed.
    pub bodies: usize,
    /// Distinct meshes made anew.
    pub meshed: usize,
    /// Distinct meshes taken from the memo.
    pub reused: usize,
}

/// The document object a plan body is picked as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Object {
    Pad(Id, PadId),
    Track(Id),
    Via(Id),
    Fill(Id),
}

fn object(origin: &Origin) -> Option<Object> {
    match *origin {
        Origin::Laminate(_) => None,
        Origin::Pad { instance, pad, .. } => Some(Object::Pad(instance, pad)),
        Origin::Track(track) => Some(Object::Track(track)),
        Origin::Via { via, .. } => Some(Object::Via(via)),
        Origin::Fill { fill, .. } => Some(Object::Fill(fill)),
    }
}

/// Each flat object's pick, from the sources that name it.
fn picks(sources: &[Source]) -> HashMap<Object, u64> {
    let mut out = HashMap::new();
    for s in sources {
        let object = match (s.instance, s.pad, s.track, s.via, s.fill) {
            (Some(instance), Some(pad), ..) => Object::Pad(instance, pad),
            (_, _, Some(track), ..) => Object::Track(track),
            (_, _, _, Some(via), _) => Object::Via(via),
            (_, _, _, _, Some(fill)) => Object::Fill(fill),
            _ => continue,
        };
        out.entry(object).or_insert(s.pick_id);
    }
    out
}

/// A source naming each flat object of `plan`, picked 1, 2, … in plan
/// order: for meshing a plan without a model (the parity check, timings).
pub fn plan_sources(plan: &BoardPlan) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for body in plan.bodies() {
        let Some(object) = object(&body.source) else {
            continue;
        };
        if !seen.insert(object) {
            continue;
        }
        let mut source = Source {
            pick_id: out.len() as u64 + 1,
            instance: None,
            pad: None,
            track: None,
            via: None,
            fill: None,
            net: body.net,
            layer: body.layer,
            label: body.name.clone(),
            component: super::NO_COMPONENT,
            occurrence: None,
        };
        match object {
            Object::Pad(instance, pad) => {
                source.instance = Some(instance);
                source.pad = Some(pad);
            }
            Object::Track(track) => source.track = Some(track),
            Object::Via(via) => source.via = Some(via),
            Object::Fill(fill) => source.fill = Some(fill),
        }
        out.push(source);
    }
    out
}

/// A body's loops in its own frame: mirrored when the frame turns it
/// over, so the placement left is a turn and a move.
fn local_loops(body: &PlanBody) -> Vec<Contour> {
    if body.frame.flipped {
        body.loops
            .iter()
            .map(|c| c.mapped(|p| [p[0], -p[1]], true))
            .collect()
    } else {
        body.loops.clone()
    }
}

fn hash_point(h: &mut impl Hasher, p: Point) {
    p[0].to_bits().hash(h);
    p[1].to_bits().hash(h);
}

/// What a body's mesh is made from, digested: kind, heights and loops in
/// its own frame (every number exactly).
fn key(body: &PlanBody) -> u64 {
    use sparksmith_core::library::Segment;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", body.kind).hash(&mut h);
    body.z[0].to_bits().hash(&mut h);
    body.z[1].to_bits().hash(&mut h);
    body.frame.flipped.hash(&mut h);
    body.loops.len().hash(&mut h);
    for contour in &body.loops {
        match contour {
            Contour::Circle { center, radius } => {
                0u8.hash(&mut h);
                hash_point(&mut h, *center);
                radius.to_bits().hash(&mut h);
            }
            Contour::Outline(outline) => {
                1u8.hash(&mut h);
                hash_point(&mut h, outline.start);
                outline.segments.len().hash(&mut h);
                for segment in &outline.segments {
                    match *segment {
                        Segment::Line { to } => {
                            0u8.hash(&mut h);
                            hash_point(&mut h, to);
                        }
                        Segment::Arc { to, bulge } => {
                            1u8.hash(&mut h);
                            hash_point(&mut h, to);
                            bulge.to_bits().hash(&mut h);
                        }
                    }
                }
            }
        }
    }
    h.finish()
}

/// Twice a polygon's signed area.
fn twice_area(points: &[Point]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

fn length(points: &[Point]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            (b[0] - a[0]).hypot(b[1] - a[1])
        })
        .sum()
}

/// Whether a polygon turns one way only (either way round).
fn convex(points: &[Point]) -> bool {
    let n = points.len();
    if n < 3 {
        return false;
    }
    let mut sign = 0.0_f64;
    for i in 0..n {
        let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() <= 1e-12 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    sign != 0.0
}

/// Mesh `body` in its own frame.
fn make(body: &PlanBody, name: &str) -> Result<Made> {
    let loops: Vec<Vec<Point>> = local_loops(body)
        .iter()
        .map(|c| c.flatten(FLATTEN_TOLERANCE_MM))
        .collect::<Result<_>>()?;
    let (outer, holes) = loops
        .split_first()
        .ok_or_else(|| Error::Verification(format!("plan body {name} has no loop")))?;
    let (z0, z1) = (body.z[0], body.z[1]);
    let mesh = if holes.is_empty() && convex(outer) {
        let mut mesh = prism(outer, z0, z1);
        let corners = prism_points(outer);
        let walls = (0..corners.len())
            .map(|i| 2 + u32::try_from(i).unwrap_or(u32::MAX - 2))
            .collect::<Vec<_>>();
        add_edges(&mut mesh, &corners, &walls, [z0, z1]);
        mesh
    } else {
        let mut mesh = slab(
            &Region {
                fills: vec![outer.clone()],
                cuts: holes.to_vec(),
            },
            z0,
            z1,
            TILE_MM,
        );
        // The slab mesher makes every wall face 2.
        for points in &loops {
            let corners = prism_points(points);
            add_edges(&mut mesh, &corners, &vec![2; corners.len()], [z0, z1]);
        }
        mesh
    };
    let volume = mesh.measure().volume;
    if mesh.triangles.is_empty() || !(volume > 0.0 && volume.is_finite()) {
        return Err(Error::Verification(format!(
            "plan body {name} meshes to no closed volume"
        )));
    }
    let b = mesh.bounds();
    Ok(Made {
        mesh: Arc::new(mesh),
        min: [b.min.x, b.min.y, b.min.z],
        max: [b.max.x, b.max.y, b.max.z],
        volume,
        area: (twice_area(outer).abs() - holes.iter().map(|h| twice_area(h).abs()).sum::<f64>())
            / 2.0,
        perimeter: loops.iter().map(|l| length(l)).sum(),
    })
}

/// The edges of a prism's loop `points` (corner `i` to the next is wall
/// face `walls[i]`; the top is face 0, the bottom face 1) from `z[0]` up
/// to `z[1]`, as the kernel's B-rep edges of the same extrusion: the top
/// and bottom outlines, one polyline per run of walls joined at a smooth
/// turn (a flattened arc is one edge), and an upright edge at each sharp
/// corner. Each edge names the faces it bounds, so picks and highlights
/// reach the board's outline as on the exact model.
fn add_edges(mesh: &mut Mesh, points: &[Point], walls: &[u32], z: [f64; 2]) {
    let n = points.len();
    if n < 3 || walls.len() != n {
        return;
    }
    let direction = |i: usize| {
        let (a, b) = (points[i % n], points[(i + 1) % n]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        if l > 0.0 {
            [dx / l, dy / l]
        } else {
            [0.0, 0.0]
        }
    };
    // Corner `i` is sharp where the walls either side of it turn more than
    // the smooth turn.
    let sharp = |i: usize| {
        let (a, b) = (direction(i + n - 1), direction(i));
        a[0] * b[0] + a[1] * b[1] < SMOOTH_TURN.cos()
    };
    let corners: Vec<usize> = (0..n).filter(|&i| sharp(i)).collect();
    // Runs of walls between sharp corners; one closed run where there are
    // none (a round hole).
    let runs: Vec<(usize, usize)> = if corners.is_empty() {
        vec![(0, n)]
    } else {
        corners
            .iter()
            .enumerate()
            .map(|(k, &start)| {
                let next = corners[(k + 1) % corners.len()];
                let len = (next + n - start - 1) % n + 1;
                (start, len)
            })
            .collect()
    };
    for (start, len) in runs {
        let faces: Vec<u32> = (start..start + len).map(|i| walls[i % n]).collect();
        for (cap, height) in [(0u32, z[1]), (1u32, z[0])] {
            let polyline = (start..=start + len)
                .map(|i| {
                    let p = points[i % n];
                    Vec3::new(p[0], p[1], height)
                })
                .collect();
            let mut bounds = vec![cap];
            for face in &faces {
                if !bounds.contains(face) {
                    bounds.push(*face);
                }
            }
            mesh.edges.push(polyline);
            mesh.edge_faces.push(bounds);
        }
    }
    for i in corners {
        let p = points[i];
        let mut bounds = vec![walls[(i + n - 1) % n]];
        if !bounds.contains(&walls[i]) {
            bounds.push(walls[i]);
        }
        mesh.edges.push(vec![
            Vec3::new(p[0], p[1], z[0]),
            Vec3::new(p[0], p[1], z[1]),
        ]);
        mesh.edge_faces.push(bounds);
    }
}

/// The board's flat bodies of `plan`, meshed: one per plan body, the
/// laminate's slabs included. Each body's geometry is picked as its
/// object's source in `sources`; a slab's names its dielectric. Unchanged meshes come from
/// `memo`, which keeps this call's.
pub fn bodies(
    plan: &BoardPlan,
    sources: &[Source],
    memo: &mut Memo,
) -> Result<(Vec<Meshed>, Stats)> {
    let picks = picks(sources);
    let planned: Vec<(&PlanBody, Origin)> = plan.bodies().map(|b| (b, b.source)).collect();
    let keys: Vec<u64> = planned.iter().map(|(b, _)| key(b)).collect();
    // Each mesh not held, once, made on every core.
    let mut wanted: Vec<(u64, &PlanBody)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ((body, _), key) in planned.iter().zip(&keys) {
        if !memo.made.contains_key(key) && seen.insert(*key) {
            wanted.push((*key, body));
        }
    }
    let slots: Vec<std::sync::OnceLock<Result<Made>>> =
        wanted.iter().map(|_| std::sync::OnceLock::new()).collect();
    let indexed: Vec<usize> = (0..wanted.len()).collect();
    super::on_threads(&indexed, |&i| {
        if let (Some((_, body)), Some(slot)) = (wanted.get(i), slots.get(i)) {
            let _ = slot.set(make(body, &body.name));
        }
    });
    let mut next = Memo::default();
    let mut stats = Stats {
        bodies: planned.len(),
        meshed: wanted.len(),
        reused: 0,
    };
    for ((key, body), slot) in wanted.iter().zip(slots) {
        let made = slot.into_inner().ok_or_else(|| {
            Error::Verification(format!("plan body {} was not meshed", body.name))
        })??;
        next.made.insert(*key, made);
    }
    let mut out = Vec::with_capacity(planned.len());
    for ((body, source), key) in planned.into_iter().zip(keys) {
        let made = match next.made.get(&key) {
            Some(made) => made.clone(),
            None => {
                let made = memo.made.get(&key).cloned().ok_or_else(|| {
                    Error::Verification(format!("plan body {} has no mesh", body.name))
                })?;
                stats.reused += 1;
                next.made.insert(key, made.clone());
                made
            }
        };
        let item = match object(&source) {
            Some(object) => Some(*picks.get(&object).ok_or_else(|| {
                Error::Verification(format!("plan body {} has no source", body.name))
            })?),
            None => None,
        };
        let frame = body.frame;
        let (c, s) = sparksmith_core::geometry::cos_sin(frame.angle_deg);
        let [ox, oy] = frame.origin;
        let place = (frame.origin != [0.0, 0.0] || c != 1.0 || s != 0.0)
            .then(|| Place([[c, -s, 0.0, ox], [s, c, 0.0, oy], [0.0, 0.0, 1.0, 0.0]]));
        let (min, max) = match place {
            None => (made.min, made.max),
            // A move only shifts the box.
            Some(_) if c == 1.0 && s == 0.0 => (
                [made.min[0] + ox, made.min[1] + oy, made.min[2]],
                [made.max[0] + ox, made.max[1] + oy, made.max[2]],
            ),
            Some(place) => placed_bounds(&made.mesh, &place),
        };
        out.push(Meshed {
            source,
            area: made.area,
            perimeter: made.perimeter,
            geometry: Geometry {
                item,
                kind: body.kind,
                layer: body.layer,
                dielectric: match source {
                    Origin::Laminate(level) => Some(level.dielectric()),
                    _ => None,
                },
                surface: body.look.surface()?,
                detail: body.look.detail,
                mesh: made.mesh,
                place,
                volume: Volume::known(made.volume),
                min,
                max,
            },
        });
    }
    *memo = next;
    Ok((out, stats))
}

/// The box of `mesh` placed at `place`.
fn placed_bounds(mesh: &Mesh, place: &Place) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for p in &mesh.positions {
        let q: Vec3 = place.point(*p);
        for (k, v) in [q.x, q.y, q.z].into_iter().enumerate() {
            lo[k] = lo[k].min(v);
            hi[k] = hi[k].max(v);
        }
    }
    (lo, hi)
}

/// How far a meshed body's box may lie inside its plan body's, mm: a
/// chord of a flattened arc lies at most [`FLATTEN_TOLERANCE_MM`] inside
/// it (the plan bounds an arc by its points at a tenth of that).
pub const PARITY_MM: f64 = FLATTEN_TOLERANCE_MM * 1.1 + 1e-9;

/// How far a mesh's volume may lie from its flattened loops' area times
/// its thickness, as a fraction: the mesher is exact on its polygons.
pub const EXACT_VOLUME: f64 = 1e-9;

/// Where the meshed bodies `meshed` differ from `plan`'s: one mesh per plan
/// body, the laminate's slabs included, of its kind, source and layer (and
/// a slab of its dielectric). Each mesh's box lies within [`PARITY_MM`] of
/// its plan body's; its volume is its flattened loops' area times its
/// thickness (to [`EXACT_VOLUME`]), and lies from the plan body's exact
/// volume by no more than flattening explains: the loops' length times
/// [`FLATTEN_TOLERANCE_MM`] times the thickness. A slab's volume, which
/// the plan states none of, is checked against the outline less its holes
/// where those are disjoint. Empty when they agree.
pub fn parity(plan: &BoardPlan, meshed: &[Meshed]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut by_source: HashMap<String, &Meshed> = HashMap::new();
    for m in meshed {
        let key = format!("{:?}", m.source);
        if by_source.insert(key.clone(), m).is_some() {
            out.push(format!("meshed twice: {key}"));
        }
    }
    for body in plan.bodies() {
        let key = format!("{:?}", body.source);
        let Some(m) = by_source.remove(&key) else {
            out.push(format!(
                "planned, not meshed: {} {:?}",
                body.name, body.source
            ));
            continue;
        };
        let g = &m.geometry;
        let dielectric = match body.source {
            Origin::Laminate(level) => Some(level.dielectric()),
            _ => None,
        };
        if g.kind != body.kind || g.layer != body.layer || g.dielectric != dielectric {
            out.push(format!(
                "{}: meshed as {:?} on {:?} in dielectric {:?}, planned {:?} on {:?} in {dielectric:?}",
                body.name, g.kind, g.layer, g.dielectric, body.kind, body.layer
            ));
        }
        let (min, max) = body.bounds()?;
        let off = min
            .iter()
            .chain(&max)
            .zip(g.min.iter().chain(&g.max))
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        if off > PARITY_MM {
            out.push(format!(
                "{}: box {:?}–{:?}, planned {min:?}–{max:?} ({off} mm off)",
                body.name, g.min, g.max
            ));
        }
        let thickness = body.z[1] - body.z[0];
        let volume = g.volume();
        let flat = m.area * thickness;
        // A foil level's holes are that layer's copper outlines, which
        // overlap (a ring on a track): the outer less each hole is no area
        // of it, so only its exact volume is checked, where the holes lie
        // apart.
        let overlapping = matches!(body.source, Origin::Laminate(Level::Foil { .. }));
        if !overlapping && (volume - flat).abs() > EXACT_VOLUME * flat.abs().max(1.0) {
            out.push(format!(
                "{}: mesh volume {volume}, its loops' {flat}",
                body.name
            ));
        }
        let exact = body.volume().or_else(|| disjoint_volume(body));
        if let Some(exact) = exact {
            let allowed = m.perimeter * FLATTEN_TOLERANCE_MM * thickness + 1e-9;
            if (volume - exact).abs() > allowed {
                out.push(format!(
                    "{}: mesh volume {volume}, planned {exact} (flattening allows {allowed})",
                    body.name
                ));
            }
        }
    }
    for (key, _) in by_source {
        out.push(format!("meshed, not planned: {key}"));
    }
    Ok(out)
}

/// A body's exact volume when its holes lie apart (their boxes do not
/// meet): the outer area less the holes'.
fn disjoint_volume(body: &PlanBody) -> Option<f64> {
    let (outer, holes) = body.loops.split_first()?;
    let mut boxes: Vec<(Point, Point)> = holes
        .iter()
        .map(|h| h.bounds(FLATTEN_TOLERANCE_MM))
        .collect::<Result<_>>()
        .ok()?;
    boxes.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
    for (i, a) in boxes.iter().enumerate() {
        for b in &boxes[i + 1..] {
            if b.0[0] > a.1[0] {
                break;
            }
            if b.0[1] <= a.1[1] && a.0[1] <= b.1[1] {
                return None;
            }
        }
    }
    let area = outer.area() - holes.iter().map(Contour::area).sum::<f64>();
    Some(area * (body.z[1] - body.z[0]))
}

#[cfg(test)]
mod tests;
