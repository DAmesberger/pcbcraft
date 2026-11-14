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
use crate::appearance::{self, Board, CAP_MARGIN_MM, Land, Memo, Path, Region, ViaMark, slab};
use crate::material::BodyKind;
use crate::scene::Geometry;
use crate::stack::{INK_SINK_SHARE, Slot};
use sparksmith_core::geometry::{Bounds, Shape};
use sparksmith_core::layout::grid::BoundsGrid;
use sparksmith_core::layout::{self, BoardDrawing, PlacedItem};
use sparksmith_core::library::graphics::PlacedText;
use sparksmith_core::library::offset_polygon;
use sparksmith_core::rules::SilkscreenRules;
use sparksmith_core::{CopperLayer, Id, Layer, Point, Project};
use std::collections::BTreeMap;

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
    /// Outer-layer pour regions (outer loop, then its holes), whose mask
    /// the legend climbs too.
    pub pours: &'a [(Layer, Vec<Vec<Point>>)],
    /// The pick identity of each placed instance, which its legend shares.
    pub picks: &'a BTreeMap<Id, u64>,
}

/// The silkscreen films of `project`, cut at the mask openings of
/// `legend.lands` grown by the rules' silkscreen-to-pad clearance
/// (`rules.silkscreen.pad_clearance`), as fabricators clip legend. Each film is a [`BodyKind::Silkscreen`] geometry whose
/// `layer` names its side's outer copper layer: one film per placed part and
/// side, plus its pieces standing on mask caps. A part's films whose
/// inputs `memo` (the last call's) holds are taken from it instead of made
/// again; the memo of this call is returned with the films, so an edit
/// remakes only the legend it changed.
pub fn films(
    project: &Project,
    legend: &Legend<'_>,
    memo: &Memo,
) -> Result<(Vec<Geometry>, Memo)> {
    let mut next = Memo::default();
    let stackup = &project.stackup;
    let mut strokes: BTreeMap<(Id, Layer), Vec<Vec<Point>>> = BTreeMap::new();
    let silk = project.rules.effective().silkscreen;
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
            PlacedItem::Text(text) => text_loops(out, text, &silk),
        }
    }
    let board = legend.board;
    // Each side's openings (grown once) and covers, filed by bounds, so a
    // part's legend meets only what lies under it: a board's parts cost
    // their own ink, not parts × board.
    let mut sides: BTreeMap<Layer, Near> = BTreeMap::new();
    let mut films = Vec::new();
    for ((instance, side), loops) in strokes {
        if loops.is_empty() {
            continue;
        }
        let Some(reach) = Bounds::of(loops.iter().flatten().copied()) else {
            continue;
        };
        let reach = reach.inflated(NEAR_MM);
        let near = sides
            .entry(side)
            .or_insert_with(|| Near::of(legend, side, silk.pad_clearance));
        let item = legend.picks.get(&instance).copied();
        let layer = Some(CopperLayer::from(side));
        let cuts = near.cuts(&reach);
        // On the mask (or the bare laminate): the stack's legend slot; and
        // on the crown of the mask (or the finish) over copper.
        let Some([z0, z1]) = board.stack.span_of(Slot::Silk(side)) else {
            continue;
        };
        let crown = if board.mask(side) {
            board.crown(side)
        } else {
            board.copper(side) + board.stack.finish_mm()
        };
        let ink = board.stack.ink_mm();
        let sink = INK_SINK_SHARE * ink;
        let flat = Region {
            fills: loops,
            cuts,
        };
        let (c0, c1) = board.span(side, crown - sink, crown + ink);
        let covers = near.covers(&reach);
        let inputs = (board, item, layer, [z0, z1, c0, c1], &flat, &covers);
        memo.unit(&mut next, &mut films, &inputs, |films| {
            display_only(appearance::push(
                films,
                Some(board),
                BodyKind::Silkscreen,
                item,
                layer,
                slab(&flat, z0, z1, FILM_TILE_MM),
            ))?;
            for raised in raised(&flat.fills, &covers) {
                display_only(appearance::push(
                    films,
                    Some(board),
                    BodyKind::Silkscreen,
                    item,
                    layer,
                    slab(
                        &Region {
                            fills: raised.fills,
                            cuts: raised.cuts.into_iter().chain(flat.cuts.clone()).collect(),
                        },
                        c0,
                        c1,
                        FILM_TILE_MM,
                    ),
                ))?;
            }
            Ok(())
        })?;
    }
    Ok((films, next))
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

/// How far past a legend's bounds openings and covers are taken, mm: any
/// that reach its ink lie within its bounds.
const NEAR_MM: f64 = 0.01;

/// One side's mask openings (grown by the legend clearance) and covers,
/// each outline and each cover's hole filed by its bounds.
struct Near {
    cuts: Vec<Vec<Point>>,
    cut_grid: BoundsGrid,
    /// Each cover's outer loop.
    outers: Vec<Vec<Point>>,
    outer_grid: BoundsGrid,
    /// Every cover's holes, with the cover they belong to.
    holes: Vec<(usize, Vec<Point>)>,
    hole_grid: BoundsGrid,
    scratch: Vec<usize>,
}

impl Near {
    fn of(legend: &Legend<'_>, side: Layer, clearance: f64) -> Self {
        let cuts: Vec<Vec<Point>> = legend
            .lands
            .iter()
            .filter(|land| land.layer == side)
            .flat_map(|land| land.openings.iter())
            .map(|o| offset_polygon(o, clearance).unwrap_or_else(|| o.clone()))
            .collect();
        let mut outers = Vec::new();
        let mut holes = Vec::new();
        for cover in covers(legend, side) {
            let mut rings = cover.into_iter();
            let Some(outer) = rings.next() else {
                continue;
            };
            holes.extend(rings.map(|h| (outers.len(), h)));
            outers.push(outer);
        }
        let bounds = |ring: &Vec<Point>| Bounds::of(ring.iter().copied());
        Self {
            cut_grid: BoundsGrid::new(cuts.iter().map(bounds), NEAR_MM),
            outer_grid: BoundsGrid::new(outers.iter().map(bounds), NEAR_MM),
            hole_grid: BoundsGrid::new(holes.iter().map(|(_, h)| bounds(h)), NEAR_MM),
            cuts,
            outers,
            holes,
            scratch: Vec::new(),
        }
    }

    /// The openings whose bounds meet `reach`, in their order.
    fn cuts(&mut self, reach: &Bounds) -> Vec<Vec<Point>> {
        self.cut_grid.near(reach, 0.0, &mut self.scratch);
        self.scratch
            .iter()
            .filter_map(|k| self.cuts.get(*k).cloned())
            .collect()
    }

    /// The covers whose outer loop meets `reach`, each with only the holes
    /// that meet it too: a hole elsewhere cannot change the ink's
    /// intersection with its cover.
    fn covers(&mut self, reach: &Bounds) -> Vec<Vec<Vec<Point>>> {
        self.outer_grid.near(reach, 0.0, &mut self.scratch);
        let mut covers: BTreeMap<usize, Vec<Vec<Point>>> = self
            .scratch
            .iter()
            .filter_map(|k| Some((*k, vec![self.outers.get(*k)?.clone()])))
            .collect();
        self.hole_grid.near(reach, 0.0, &mut self.scratch);
        for (cover, hole) in self.scratch.iter().filter_map(|k| self.holes.get(*k)) {
            if let Some(rings) = covers.get_mut(cover) {
                rings.push(hole.clone());
            }
        }
        covers.into_values().collect()
    }
}

/// The shapes on `side` where the mask stands over copper: track caps,
/// tented via domes' crowns (copper itself when there is no mask) and the
/// pours, each an outer loop counter-clockwise and its holes clockwise.
fn covers(legend: &Legend<'_>, side: Layer) -> Vec<Vec<Vec<Point>>> {
    let mut out: Vec<Vec<Vec<Point>>> = legend
        .paths
        .iter()
        .filter(|p| p.layer == side)
        .map(|p| vec![stadium(p.start, p.end, p.width / 2.0 + CAP_MARGIN_MM)])
        .collect();
    out.extend(
        legend
            .vias
            .iter()
            .filter(|v| v.covered(legend.board, side))
            .map(|v| vec![oriented(appearance::circle(v.at, v.diameter / 2.0), true)]),
    );
    out.extend(
        legend
            .pours
            .iter()
            .filter(|(layer, _)| *layer == side)
            .map(|(_, loops)| {
                loops
                    .iter()
                    .enumerate()
                    .map(|(k, l)| oriented(l.clone(), k == 0))
                    .collect()
            }),
    );
    out
}

/// One raised film: its pieces and their holes.
struct Raised {
    fills: Vec<Vec<Point>>,
    cuts: Vec<Vec<Point>>,
}

/// The ink of `loops` over `covers`, exactly (`i_overlay`'s non-zero
/// intersection, so overlapping covers merge and a track in a pour's
/// clearance gap stands on its own cap). A piece inside another piece's
/// hole would be cut by that hole in one slab, so pieces go to one film
/// per nesting depth; nearly always there is one.
fn raised(loops: &[Vec<Point>], covers: &[Vec<Vec<Point>>]) -> Vec<Raised> {
    use i_overlay::core::fill_rule::FillRule;
    use i_overlay::core::overlay_rule::OverlayRule;
    use i_overlay::float::single::SingleFloatOverlay;
    if loops.is_empty() || covers.is_empty() {
        return Vec::new();
    }
    let ink: Vec<Vec<Vec<Point>>> = loops
        .iter()
        .map(|l| vec![oriented(l.clone(), true)])
        .collect();
    let pieces: Vec<Vec<Vec<Point>>> = ink
        .overlay(&covers.to_vec(), OverlayRule::Intersect, FillRule::NonZero)
        .into_iter()
        .filter(|shape| shape.first().is_some_and(|outer| outer.len() >= 3))
        .collect();
    let depths: Vec<usize> = pieces
        .iter()
        .map(|shape| {
            let probe = shape.first().and_then(|outer| outer.first()).copied();
            probe.map_or(0, |p| {
                pieces
                    .iter()
                    .flat_map(|s| s.iter().skip(1))
                    .filter(|h| inside(p, h))
                    .count()
            })
        })
        .collect();
    let mut films: Vec<Raised> = Vec::new();
    for (shape, depth) in pieces.into_iter().zip(depths) {
        while films.len() <= depth {
            films.push(Raised {
                fills: Vec::new(),
                cuts: Vec::new(),
            });
        }
        let mut rings = shape.into_iter();
        if let (Some(film), Some(outer)) = (films.get_mut(depth), rings.next()) {
            film.fills.push(outer);
            film.cuts.extend(rings);
        }
    }
    films.retain(|f| !f.fills.is_empty());
    films
}

/// `ring` counter-clockwise when `ccw`, else clockwise.
fn oriented(mut ring: Vec<Point>, ccw: bool) -> Vec<Point> {
    let n = ring.len();
    let area: f64 = (0..n)
        .map(|i| ring[i][0] * ring[(i + 1) % n][1] - ring[(i + 1) % n][0] * ring[i][1])
        .sum();
    if (area > 0.0) != ccw {
        ring.reverse();
    }
    ring
}

/// Whether `p` lies strictly inside `ring` (even-odd).
fn inside(p: Point, ring: &[Point]) -> bool {
    let n = ring.len();
    let mut odd = false;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if x > p[0] {
                odd = !odd;
            }
        }
    }
    odd
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
/// (`layout::legend`, Sparksmith's stroke font laid out by the text metric),
/// each stroked with its round pen.
fn text_loops(out: &mut Vec<Vec<Point>>, text: &PlacedText, silk: &SilkscreenRules) {
    let strokes = layout::legend::text_strokes(text, silk);
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

#[cfg(test)]
mod tests;
