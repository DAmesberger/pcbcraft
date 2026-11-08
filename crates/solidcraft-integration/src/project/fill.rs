//! Copper fills in the native model: each fill's computed copper
//! ([`pcbcraft_core::model::fill::FilledCopper`]) as saved copper on its
//! layer, one body per region in one component per fill, each extruded
//! once from its boundary and holes (inserted into the sketch in one
//! command, [`crate::native::Native::batched_body`]), so pad clearances
//! cost no Boolean and no command each. The bodies come before the parts
//! and the rest of the board's copper, which they do not read (so an edit
//! to those reuses them), and the laminate comes last; their components
//! are joined to the board with every other part's joint at the end. Over an
//! outer-layer fill the display-only mask conforms as a cap, as over a
//! track; it is never saved.
//!
//! An inner-layer fill lies in its own foil level of the laminate, as
//! inner tracks and via rings do ([`foil_level`]): that level's slab is
//! extruded with the fill's copper as a hole, so the saved bodies stay
//! disjoint (no interference with the laminate) and the laminate's mass
//! is physical, without a Boolean.
//!
//! Each region's loops are simplified to [`MODEL_CHORD_MM`] before they
//! are extruded: the document's 5 µm flattening gives a pour thousands of
//! points, each a face to tessellate, measure and save.
//!
//! The fill copper is the copper the document stores: the board view and
//! the 3D view show the same regions, stale or not, and a refill (a new
//! document revision) rebuilds them.
use super::{Builder, Source, copper_param, inner_base, next_pick, polygon, track_outline};
use crate::Result;
use crate::appearance::{self, Board, Land, MASK_CAP_MM, Region, ViaMark};
use crate::material::BodyKind;
use crate::native::{BoardJoint, Expr, JointFrame, Mount, ProfileBody};
use crate::pad_outline::{Contour, FLATTEN_TOLERANCE_MM};
use crate::scene::Geometry;
use pcbcraft_core::layout::fill::{FillContact, copper_union};
use pcbcraft_core::model::Fill;
use pcbcraft_core::model::fill::FilledRegion;
use pcbcraft_core::{CopperLayer, Point, Project};

/// Mask cap tiles over a fill, as the board mask's.
const CAP_TILE_MM: f64 = 4.0;

/// A fill as the display layers see it: its pick, layer and regions.
pub(super) struct FillMark {
    pub pick: u64,
    pub layer: CopperLayer,
    pub regions: Vec<FilledRegion>,
}

/// The fills with copper to build, in document order, each region
/// simplified for the model ([`simplified`]).
fn built(project: &Project) -> Vec<(&Fill, Vec<FilledRegion>)> {
    project
        .fills
        .iter()
        .filter_map(|f| {
            let regions = f.computed.as_ref()?.regions.as_slice();
            (!regions.is_empty() && project.stackup.has(f.layer))
                .then(|| (f, regions.iter().map(simplified).collect()))
        })
        .collect()
}

/// The chord tolerance a fill's boundary is simplified to in the native
/// model, mm. The document's copper is flattened at
/// [`pcbcraft_core::layout::fill::FILL_CHORD_MM`] (5 µm) for the checks;
/// at that resolution a pour of thermals and clearance arcs holds
/// thousands of points, each a face of the saved body and of the
/// laminate's cut. 10 µm, a quarter of the thinnest foil, is far below
/// what the 3D view shows, as the track caps' chords are (2 % of the
/// radius).
pub(super) const MODEL_CHORD_MM: f64 = 0.01;

/// `region` with every loop simplified to [`MODEL_CHORD_MM`]: collinear
/// and near-collinear points dropped (Douglas–Peucker), each loop kept
/// when simplifying would leave fewer than three points.
fn simplified(region: &FilledRegion) -> FilledRegion {
    FilledRegion {
        outer: simplify_loop(&region.outer, MODEL_CHORD_MM),
        holes: region
            .holes
            .iter()
            .map(|h| simplify_loop(h, MODEL_CHORD_MM))
            .collect(),
    }
}

/// A closed loop simplified to `tolerance`: split at its first point and
/// the point farthest from it, each half simplified by Douglas–Peucker.
pub(super) fn simplify_loop(points: &[Point], tolerance: f64) -> Vec<Point> {
    let n = points.len();
    if n < 4 {
        return points.to_vec();
    }
    let first = points[0];
    let far = (1..n)
        .max_by(|&a, &b| {
            let d = |k: usize| (points[k][0] - first[0]).hypot(points[k][1] - first[1]);
            d(a).total_cmp(&d(b))
        })
        .unwrap_or(n / 2);
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    let ring: Vec<Point> = points
        .iter()
        .copied()
        .chain(std::iter::once(first))
        .collect();
    douglas_peucker(&ring, 0, far, tolerance, &mut keep);
    let mut tail = vec![false; n + 1];
    douglas_peucker(&ring, far, n, tolerance, &mut tail);
    for (k, kept) in tail.into_iter().enumerate().take(n) {
        keep[k] |= kept;
    }
    let out: Vec<Point> = points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect();
    if out.len() < 3 { points.to_vec() } else { out }
}

/// Mark the points of `points[a..=b]` Douglas–Peucker keeps at
/// `tolerance` (the ends excluded), iteratively.
fn douglas_peucker(points: &[Point], a: usize, b: usize, tolerance: f64, keep: &mut [bool]) {
    let mut stack = vec![(a, b)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (p, q) = (points[a], points[b]);
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        let len = dx.hypot(dy);
        let distance = |r: Point| {
            if len <= f64::EPSILON {
                (r[0] - p[0]).hypot(r[1] - p[1])
            } else {
                ((r[0] - p[0]) * dy - (r[1] - p[1]) * dx).abs() / len
            }
        };
        let (k, d) = (a + 1..b)
            .map(|k| (k, distance(points[k])))
            .fold((a, 0.0_f64), |best, c| if c.1 > best.1 { c } else { best });
        if d > tolerance {
            if let Some(slot) = keep.get_mut(k) {
                *slot = true;
            }
            stack.push((a, k));
            stack.push((k, b));
        }
    }
}

/// A region's outer boundary and holes, or nothing for a region of
/// fewer than three points.
fn loops(region: &FilledRegion) -> Vec<&[Point]> {
    if region.outer.len() < 3 {
        return Vec::new();
    }
    std::iter::once(region.outer.as_slice())
        .chain(
            region
                .holes
                .iter()
                .filter(|h| h.len() >= 3)
                .map(Vec::as_slice),
        )
        .collect()
}

fn body_name(fill: &Fill) -> String {
    format!("fill_{}", fill.id)
}

/// The board joints [`build`] adds, made with every other part's.
pub(super) type Joints = Vec<(BoardJoint, JointFrame, Option<String>)>;

/// Every fill's copper ([`bodies`]).
pub(super) fn build(
    b: &mut Builder,
    project: &Project,
    board: u64,
    sources: &mut Vec<Source>,
    joints: &mut Joints,
) -> Result<Vec<FillMark>> {
    bodies(b, project, board, sources, joints, built(project))
}

/// Each fill's copper as one body in a component of its own, held upright
/// on the board at the origin; its source names the fill, its net and
/// layer. Returns what the display layers draw over each.
fn bodies(
    b: &mut Builder,
    project: &Project,
    board: u64,
    sources: &mut Vec<Source>,
    joints: &mut Joints,
    built: Vec<(&Fill, Vec<FilledRegion>)>,
) -> Result<Vec<FillMark>> {
    let mut marks = Vec::new();
    for (fill, regions) in built {
        if regions.iter().all(|r| loops(r).is_empty()) {
            continue;
        }
        let name = body_name(fill);
        let height = copper_param(fill.layer);
        let offset: Expr = match fill.layer {
            CopperLayer::Top => "board_thickness".into(),
            CopperLayer::Bottom => format!("- {height}").into(),
            CopperLayer::Inner(_) => inner_base(&project.stackup, fill.layer)?.into(),
        };
        let created = b.native.component(&name, false)?;
        // One body per region: the regions are disjoint, so each sketch
        // makes exactly one body of a known name.
        for (k, region) in regions.iter().enumerate() {
            let contours: Vec<Contour> = loops(region).into_iter().filter_map(polygon).collect();
            if contours.is_empty() {
                continue;
            }
            let body = format!("{name}_{k}");
            b.native.batched_body(ProfileBody {
                name: &body,
                offset: offset.clone(),
                contours: &contours,
                height: height.as_str().into(),
            })?;
            b.record(&body, BodyKind::FillCopper)?;
        }
        joints.push((
            BoardJoint {
                board,
                occurrence: created.occurrence,
                x: 0.into(),
                y: 0.into(),
                angle: 0.into(),
                mount: Mount::Height(0.into()),
            },
            JointFrame::ORIGIN,
            None,
        ));
        let pick = next_pick(sources);
        let label = if fill.name.is_empty() {
            format!(
                "Fill · {}",
                fill.net.map_or("No net", |n| project.net_name(n))
            )
        } else {
            format!("Fill · {}", fill.name)
        };
        sources.push(Source {
            pick_id: pick,
            instance: None,
            pad: None,
            track: None,
            via: None,
            fill: Some(fill.id),
            net: fill.net,
            layer: Some(fill.layer),
            label,
            component: created.component,
            occurrence: None,
        });
        marks.push(FillMark {
            pick,
            layer: fill.layer,
            regions,
        });
    }
    Ok(marks)
}

/// Sides of the polygon a round outline joins the fill copper's union as
/// (circumscribed, so the laminate's hole holds the exact circle).
const UNION_SEGMENTS: usize = 64;

/// How close copper must come to a fill to join its union, mm: shapes
/// that meet the fill's edge count as touching.
const CONTACT_MM: f64 = 1e-4;

/// The holes of inner `layer`'s foil level when fills lie on it, or
/// `None` without one (the laminate then cuts the layer's tracks and via
/// rings as it does without fills).
///
/// `holes` are the level's non-copper holes (pad drills, cutouts, slots
/// and mounting holes). The fill regions (simplified as their bodies),
/// with every track, via ring and hole that overlaps them (directly or
/// through other copper), are united into disjoint regions; their outer
/// boundaries and holes go into the slab's sketch, whose even-odd nesting
/// leaves laminate in each clearance and copper where the union is. A
/// round outline joins the union as a circumscribed polygon, so the
/// laminate stays clear of its exact circle. Everything else on the layer
/// stays as the laminate cuts it without fills, exact circles included.
pub(super) fn foil_level(
    project: &Project,
    layer: CopperLayer,
    holes: &[Contour],
) -> Option<Vec<Contour>> {
    let regions: Vec<FilledRegion> = built(project)
        .into_iter()
        .filter(|(f, _)| f.layer == layer)
        .flat_map(|(_, regions)| regions)
        .filter(|r| r.outer.len() >= 3)
        .collect();
    if regions.is_empty() {
        return None;
    }
    let layers = project.stackup.layers();
    // Each shape as a round-capped spine (a disc when its ends meet),
    // the contour the laminate cuts it by, and its outline in a union.
    let mut items: Vec<Item> = Vec::new();
    for track in project.tracks.iter().filter(|t| t.layer == layer) {
        let ring = track_outline(track.start, track.end, track.width);
        let Some(contour) = polygon(&ring) else {
            continue;
        };
        items.push(Item {
            a: track.start,
            b: track.end,
            radius: track.width / 2.0,
            contour,
            ring,
        });
    }
    for via in project.vias.iter().filter(|v| v.on(layer, &layers)) {
        let radius = via.diameter / 2.0;
        items.push(Item {
            a: via.position,
            b: via.position,
            radius,
            contour: Contour::Circle {
                center: via.position,
                radius,
            },
            ring: circumscribed(via.position, radius),
        });
    }
    items.extend(holes.iter().map(Item::of_hole));
    let contact = FillContact::new(&regions);
    let mut joined: Vec<bool> = items
        .iter()
        .map(|i| contact.touches(i.a, i.b, i.radius, CONTACT_MM))
        .collect();
    spread(&items, &mut joined);
    let rings: Vec<Vec<Point>> = items
        .iter()
        .zip(&joined)
        .filter(|(_, j)| **j)
        .map(|(i, _)| i.ring.clone())
        .collect();
    let mut level: Vec<Contour> = Vec::new();
    for region in copper_union(&regions, &rings) {
        level.extend(polygon(&region.outer));
        level.extend(region.holes.iter().filter_map(|h| polygon(h)));
    }
    let rest: Vec<Contour> = items
        .into_iter()
        .zip(joined)
        .filter(|(_, j)| !*j)
        .map(|(i, _)| i.contour)
        .collect();
    level.extend(super::without_nested(rest));
    Some(level)
}

/// A foil-level shape: its spine `a`–`b` and the radius round it that
/// holds it, its laminate contour and its outline for a union.
struct Item {
    a: Point,
    b: Point,
    radius: f64,
    contour: Contour,
    ring: Vec<Point>,
}

impl Item {
    /// A hole: a circle as itself, an outline by the disc round its box
    /// (so it counts as touching whenever it may) and its flattened
    /// outline grown by the flattening tolerance (as flattened, should
    /// growing it fail).
    fn of_hole(hole: &Contour) -> Self {
        match hole {
            Contour::Circle { center, radius } => Self {
                a: *center,
                b: *center,
                radius: *radius,
                contour: hole.clone(),
                ring: circumscribed(*center, *radius),
            },
            Contour::Outline(outline) => {
                let points = outline.flatten(FLATTEN_TOLERANCE_MM);
                let ring = pcbcraft_core::library::offset_polygon(&points, FLATTEN_TOLERANCE_MM)
                    .unwrap_or(points);
                let [lo, hi] = bounds(&ring);
                let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
                Self {
                    a: centre,
                    b: centre,
                    radius: (hi[0] - lo[0]).hypot(hi[1] - lo[1]) / 2.0,
                    contour: hole.clone(),
                    ring,
                }
            }
        }
    }
}

/// A regular polygon of [`UNION_SEGMENTS`] sides round a circle.
fn circumscribed(center: Point, radius: f64) -> Vec<Point> {
    let r = radius / (std::f64::consts::PI / UNION_SEGMENTS as f64).cos() + 1e-6;
    (0..UNION_SEGMENTS)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / UNION_SEGMENTS as f64;
            [center[0] + r * a.cos(), center[1] + r * a.sin()]
        })
        .collect()
}

/// Mark every item overlapping a marked one, until none is left: copper
/// joined to the fill through other copper joins its union too. Items
/// are compared through a grid of their boxes.
fn spread(items: &[Item], joined: &mut [bool]) {
    use std::collections::HashMap;
    const CELL_MM: f64 = 1.0;
    /// Most cells one item is filed in; larger ones are always compared.
    const MAX_CELLS: i64 = 256;
    // Bounds are finite and boards at most a metre, so a cell index fits.
    let cell = |v: f64| (v / CELL_MM).floor() as i64;
    let cells = |lo: i64, hi: i64| hi.saturating_sub(lo).saturating_add(1);
    let reach = |i: &Item| {
        let r = i.radius + CONTACT_MM;
        (
            [i.a[0].min(i.b[0]) - r, i.a[1].min(i.b[1]) - r],
            [i.a[0].max(i.b[0]) + r, i.a[1].max(i.b[1]) + r],
        )
    };
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut large = Vec::new();
    for (k, item) in items.iter().enumerate() {
        let (lo, hi) = reach(item);
        let (x0, y0, x1, y1) = (cell(lo[0]), cell(lo[1]), cell(hi[0]), cell(hi[1]));
        if cells(x0, x1).saturating_mul(cells(y0, y1)) > MAX_CELLS {
            large.push(k);
            continue;
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                grid.entry((x, y)).or_default().push(k);
            }
        }
    }
    let mut queue: Vec<usize> = (0..items.len())
        .filter(|k| joined.get(*k).copied().unwrap_or(false))
        .collect();
    while let Some(k) = queue.pop() {
        let Some(item) = items.get(k) else {
            continue;
        };
        let (lo, hi) = reach(item);
        let mut near: Vec<usize> = large.clone();
        let (x0, y0, x1, y1) = (cell(lo[0]), cell(lo[1]), cell(hi[0]), cell(hi[1]));
        if cells(x0, x1).saturating_mul(cells(y0, y1)) > MAX_CELLS {
            near = (0..items.len()).collect();
        } else {
            for x in x0..=x1 {
                for y in y0..=y1 {
                    near.extend(grid.get(&(x, y)).into_iter().flatten());
                }
            }
        }
        for j in near {
            let (Some(other), Some(flag)) = (items.get(j), joined.get_mut(j)) else {
                continue;
            };
            if *flag {
                continue;
            }
            let gap = spine_distance(item.a, item.b, other.a, other.b);
            if gap < item.radius + other.radius + CONTACT_MM {
                *flag = true;
                queue.push(j);
            }
        }
    }
}

/// The distance between segments `a`–`b` and `c`–`d` (either may be a
/// point).
fn spine_distance(a: Point, b: Point, c: Point, d: Point) -> f64 {
    use pcbcraft_core::geometry::{segment_distance, segments_cross};
    if segments_cross(a, b, c, d) {
        return 0.0;
    }
    segment_distance(a, c, d)
        .min(segment_distance(b, c, d))
        .min(segment_distance(c, a, b))
        .min(segment_distance(d, a, b))
}

/// The display-only mask cap over each outer-layer fill region: from the
/// laminate to [`MASK_CAP_MM`] over the copper, opened where the pads' and
/// open vias' mask openings are and where a tented via's dome is the mask.
pub(super) fn films(
    board: &Board,
    lands: &[Land],
    vias: &[ViaMark],
    fills: &[FillMark],
) -> Result<Vec<Geometry>> {
    let mut out = Vec::new();
    for mark in fills {
        let Some(side) = mark.layer.side() else {
            continue;
        };
        if !board.mask(side) {
            continue;
        }
        let openings: Vec<Vec<Point>> = lands
            .iter()
            .filter(|land| land.layer == side)
            .flat_map(|land| land.openings.iter().cloned())
            .chain(
                vias.iter()
                    .filter(|v| v.covered(board, side))
                    .map(|v| appearance::circle(v.at, v.diameter / 2.0)),
            )
            .collect();
        let (z0, z1) = board.span(side, 0.0, board.copper(side) + MASK_CAP_MM);
        for region in mark.regions.iter().filter(|r| r.outer.len() >= 3) {
            let bounds = bounds(&region.outer);
            let region = Region {
                fills: vec![region.outer.clone()],
                cuts: region
                    .holes
                    .iter()
                    .cloned()
                    .chain(
                        openings
                            .iter()
                            .filter(|o| overlaps(bounds, self::bounds(o)))
                            .cloned(),
                    )
                    .collect(),
            };
            appearance::push(
                &mut out,
                &board.fabrication,
                BodyKind::MaskCap,
                Some(mark.pick),
                Some(side.into()),
                appearance::slab(&region, z0, z1, CAP_TILE_MM),
            )?;
        }
    }
    Ok(out)
}

/// The box of a loop.
fn bounds(points: &[Point]) -> [Point; 2] {
    points.iter().fold(
        [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]],
        |[lo, hi], p| {
            [
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            ]
        },
    )
}

fn overlaps(a: [Point; 2], b: [Point; 2]) -> bool {
    a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
}

#[cfg(test)]
mod tests;
