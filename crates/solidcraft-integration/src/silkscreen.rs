//! Display-only silkscreen films (graphics design G5): the placed footprint
//! legend (`layout::placed_graphics`, silkscreen layers) as thin matte-ink
//! films on the mask, in the board's silkscreen colour, clipped at mask
//! openings, top and bottom. Never saved in the `.solidcraft` file.
//!
//! Ink is printed over the mask and follows it: where a stroke crosses a
//! track's mask cap or a tented via's dome, that part of the film stands
//! on the cap's crown instead, so copper under the mask never shows
//! through the legend.
use crate::Result;
use crate::appearance::{self, Board, CAP_MARGIN_MM, Land, Path, Region, ViaMark, slab};
use crate::material::BodyKind;
use crate::scene::Geometry;
use crate::stack::{FINISH_MM, INK_MM, INK_SINK_SHARE, Slot};
use pcbcraft_core::geometry::Shape;
use pcbcraft_core::layout::{self, BoardDrawing, PlacedItem};
use pcbcraft_core::library::graphics::PlacedText;
use pcbcraft_core::library::offset_polygon;
use pcbcraft_core::{CopperLayer, Id, Layer, Point, Project};
use std::collections::BTreeMap;

/// Fabricators clip legend this far clear of mask openings (est.; quoted
/// silkscreen-to-pad clearances are 0.05–0.15 mm).
pub const OPENING_CLEARANCE_MM: f64 = 0.05;
/// Chord tolerance of curves in the legend.
const LEGEND_TOLERANCE_MM: f64 = 0.01;
/// Segments of a stroke's round end.
const CAP_STEPS: usize = 4;
/// Square tiles of the film's faces.
const FILM_TILE_MM: f64 = 2.0;
/// What the legend is printed over and around.
pub struct Legend<'a> {
    pub board: &'a Board,
    /// Mask openings, per side.
    pub lands: &'a [Land],
    /// Outer tracks, whose mask caps the legend climbs.
    pub paths: &'a [Path],
    /// Board vias, whose tented domes the legend climbs.
    pub vias: &'a [ViaMark],
    /// The pick identity of each placed instance, which its legend shares.
    pub picks: &'a BTreeMap<Id, u64>,
}

/// The silkscreen films of `project`, cut at the mask openings of
/// `legend.lands`. Each film is a [`BodyKind::Silkscreen`] geometry whose
/// `layer` names its side's outer copper layer: one film per placed part and
/// side, plus its pieces standing on mask caps.
pub fn films(project: &Project, legend: &Legend<'_>) -> Result<Vec<Geometry>> {
    let stackup = &project.stackup;
    let mut strokes: BTreeMap<(Id, Layer), Vec<Vec<Point>>> = BTreeMap::new();
    for graphic in layout::placed_graphics(project) {
        let BoardDrawing::Silkscreen(side) = graphic.layer else {
            continue;
        };
        let printed = match side {
            Layer::Top => stackup.silkscreen_top,
            Layer::Bottom => stackup.silkscreen_bottom,
        };
        if !printed {
            continue;
        }
        let out = strokes.entry((graphic.instance, side)).or_default();
        match &graphic.item {
            PlacedItem::Shape {
                shape,
                width,
                filled,
            } => shape_loops(out, shape, *width, *filled),
            PlacedItem::Text(text) => text_loops(out, text),
        }
    }
    let board = legend.board;
    let mut films = Vec::new();
    for ((instance, side), loops) in strokes {
        if loops.is_empty() {
            continue;
        }
        let item = legend.picks.get(&instance).copied();
        let layer = Some(CopperLayer::from(side));
        let cuts: Vec<Vec<Point>> = legend
            .lands
            .iter()
            .filter(|land| land.layer == side)
            .flat_map(|land| land.openings.iter())
            .map(|o| offset_polygon(o, OPENING_CLEARANCE_MM).unwrap_or_else(|| o.clone()))
            .collect();
        // On the mask (or the bare laminate): the stack's legend slot; and
        // on the crown of the mask (or the finish) over copper.
        let Some([z0, z1]) = board.stack.span_of(Slot::Silk(side)) else {
            continue;
        };
        let crown = if board.mask(side) {
            board.crown(side)
        } else {
            board.copper(side) + FINISH_MM
        };
        let sink = INK_SINK_SHARE * INK_MM;
        let flat = Region {
            fills: loops.clone(),
            cuts: cuts.clone(),
        };
        display_only(appearance::push(
            &mut films,
            Some(board),
            BodyKind::Silkscreen,
            item,
            layer,
            slab(&flat, z0, z1, FILM_TILE_MM),
        ))?;
        let covers = covers(legend, side);
        let raised: Vec<Vec<Point>> = loops
            .iter()
            .flat_map(|stroke| {
                covers
                    .iter()
                    .filter_map(move |cover| clip_convex(stroke, cover))
            })
            .collect();
        if raised.is_empty() {
            continue;
        }
        // One stroke clipped by overlapping covers (a track's cap and a
        // via's dome) gives coincident pieces, whose shared edges the
        // slab cannot pair: merged first, they are one region.
        let raised = appearance::union(&raised).unwrap_or(raised);
        let (c0, c1) = board.span(side, crown - sink, crown + INK_MM);
        display_only(appearance::push(
            &mut films,
            Some(board),
            BodyKind::Silkscreen,
            item,
            layer,
            slab(
                &Region {
                    fills: raised,
                    cuts,
                },
                c0,
                c1,
                FILM_TILE_MM,
            ),
        ))?;
    }
    Ok(films)
}

/// A legend film that is not a closed outward volume is left out rather
/// than failing the whole model: the legend is display-only and never
/// saved, so a degenerate piece of ink must not cost the 3D view or the
/// export. Other errors still fail.
fn display_only(pushed: Result<()>) -> Result<()> {
    match pushed {
        Err(crate::Error::Verification(_)) => Ok(()),
        other => other,
    }
}

/// The convex outlines on `side` where the mask stands over copper: track
/// caps and tented via domes' crowns (copper itself when there is no mask).
fn covers(legend: &Legend<'_>, side: Layer) -> Vec<Vec<Point>> {
    let mut out: Vec<Vec<Point>> = legend
        .paths
        .iter()
        .filter(|p| p.layer == side)
        .map(|p| stadium(p.start, p.end, p.width / 2.0 + CAP_MARGIN_MM))
        .collect();
    out.extend(
        legend
            .vias
            .iter()
            .filter(|v| v.covered(legend.board, side))
            .map(|v| appearance::circle(v.at, v.diameter / 2.0)),
    );
    out
}

/// The loops of a stroked (and possibly filled) legend shape.
fn shape_loops(out: &mut Vec<Vec<Point>>, shape: &Shape, width: f64, filled: bool) {
    let points = shape.flatten(LEGEND_TOLERANCE_MM);
    if points.is_empty() {
        return;
    }
    let closed = shape.is_closed();
    if filled && closed && points.len() >= 3 {
        out.push(points.clone());
    }
    if width > 0.0 {
        stroke(out, &points, closed, width);
    }
}

/// A polyline stroked `width` wide: a stadium per segment, so joins and
/// ends are round.
fn stroke(out: &mut Vec<Vec<Point>>, points: &[Point], closed: bool, width: f64) {
    let half = width / 2.0;
    if let [only] = points {
        out.push(stadium(*only, *only, half));
        return;
    }
    let n = points.len();
    let segments = if closed { n } else { n - 1 };
    for i in 0..segments {
        out.push(stadium(points[i], points[(i + 1) % n], half));
    }
}

/// The loops of placed legend text: the core's legend strokes
/// (`layout::legend`, Pcbcraft's stroke font laid out by the text metric),
/// each stroked with its round pen.
fn text_loops(out: &mut Vec<Vec<Point>>, text: &PlacedText) {
    let strokes = layout::legend::text_strokes(text);
    for points in &strokes.strokes {
        stroke(out, points, false, strokes.width);
    }
}

/// A stadium (a segment grown by `half` with round ends), counter-clockwise;
/// a disc for a point.
fn stadium(a: Point, b: Point, half: f64) -> Vec<Point> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    let u = if len > 1e-12 {
        [d[0] / len, d[1] / len]
    } else {
        [1.0, 0.0]
    };
    let base = u[1].atan2(u[0]);
    let mut out = Vec::with_capacity(2 * CAP_STEPS + 2);
    for (center, from) in [(b, -0.5), (a, 0.5)] {
        for k in 0..=CAP_STEPS {
            let angle = base + std::f64::consts::PI * (from + k as f64 / CAP_STEPS as f64);
            out.push([
                center[0] + half * angle.cos(),
                center[1] + half * angle.sin(),
            ]);
        }
    }
    out
}

/// Sutherland–Hodgman clip of `subject` to the convex loop `clip` (either
/// orientation); `None` when nothing is left.
fn clip_convex(subject: &[Point], clip: &[Point]) -> Option<Vec<Point>> {
    let area = |p: &[Point]| -> f64 {
        let n = p.len();
        (0..n)
            .map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1])
            .sum()
    };
    let bounds = |p: &[Point]| {
        p.iter()
            .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, q| {
                [
                    b[0].min(q[0]),
                    b[1].min(q[1]),
                    b[2].max(q[0]),
                    b[3].max(q[1]),
                ]
            })
    };
    let (bs, bc) = (bounds(subject), bounds(clip));
    if bs[2] < bc[0] || bc[2] < bs[0] || bs[3] < bc[1] || bc[3] < bs[1] {
        return None;
    }
    let sign = if area(clip) < 0.0 { -1.0 } else { 1.0 };
    let mut out: Vec<Point> = subject.to_vec();
    let n = clip.len();
    for i in 0..n {
        let (a, b) = (clip[i], clip[(i + 1) % n]);
        let side =
            |p: Point| sign * ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]));
        let input = std::mem::take(&mut out);
        let m = input.len();
        for j in 0..m {
            let (p, q) = (input[(j + m - 1) % m], input[j]);
            let (sp, sq) = (side(p), side(q));
            if sq >= 0.0 {
                if sp < 0.0 {
                    let t = sp / (sp - sq);
                    out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
                }
                out.push(q);
            } else if sp >= 0.0 {
                let t = sp / (sp - sq);
                out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
            }
        }
        if out.len() < 3 {
            return None;
        }
    }
    (area(&out).abs() > 1e-12).then_some(out)
}

#[cfg(test)]
mod tests;
