//! Copper fills in the native model: each fill's computed copper
//! ([`pcbcraft_core::model::fill::FilledCopper`]) as saved copper on its
//! layer, one body per region in one component per fill, each extruded
//! once from its boundary and holes (inserted into the sketch in one
//! command), so pad clearances cost no Boolean and no command each. The
//! bodies are built last, after the rest of the board: every later
//! command would pay for re-evaluating them. Over an outer-layer fill the
//! display-only mask conforms as a cap, as over a track; it is never
//! saved.
//!
//! An inner-layer fill is not cut from the laminate, as inner tracks
//! are: its copper overlaps the laminate it is embedded in. The cut is a
//! Boolean of the pour's every clearance and thermal against the drilled
//! laminate, and in Solidcraft's kernel it cost about 45 s on the 44 ×
//! 28 mm showcase (four times the rest of the build), against the
//! founder's instant 3D view. The overlap is the foil's thickness over
//! the plane's area (0.035 mm × the pour), so the laminate's mass
//! properties are high by that slab; interference checks of the saved
//! file report the plane against the substrate.
//!
//! Each region's loops are simplified to [`MODEL_CHORD_MM`] before they
//! are extruded: the document's 5 µm flattening gives a pour thousands of
//! points, each a face to tessellate, measure and save.
//!
//! The fill copper is the copper the document stores: the board view and
//! the 3D view show the same regions, stale or not, and a refill (a new
//! document revision) rebuilds them.
use super::{Builder, Source, copper_param, inner_base, next_pick};
use crate::Result;
use crate::appearance::{self, Board, Land, MASK_CAP_MM, Region, ViaMark};
use crate::material::BodyKind;
use crate::native::{BoardJoint, Expr, Mount};
use crate::scene::Geometry;
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

/// Every fill's copper ([`bodies`]).
pub(super) fn build(
    b: &mut Builder,
    project: &Project,
    board: u64,
    sources: &mut Vec<Source>,
) -> Result<Vec<FillMark>> {
    bodies(b, project, board, sources, built(project))
}

/// Each fill's copper as one body in a component of its own, held upright
/// on the board at the origin; its source names the fill, its net and
/// layer. Returns what the display layers draw over each.
fn bodies(
    b: &mut Builder,
    project: &Project,
    board: u64,
    sources: &mut Vec<Source>,
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
            let loops = loops(region);
            if loops.is_empty() {
                continue;
            }
            let body = format!("{name}_{k}");
            b.native
                .polyline_body(&body, offset.clone(), &loops, height.as_str().into())?;
            b.record(&body, BodyKind::FillCopper)?;
        }
        b.native.board_joint(BoardJoint {
            board,
            occurrence: created.occurrence,
            x: 0.into(),
            y: 0.into(),
            angle: 0.into(),
            mount: Mount::Height(0.into()),
        })?;
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
