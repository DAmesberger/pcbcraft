//! The board shape and stackup in Sparksmith's terms: `Edge.Cuts`
//! drawings become board-sketch curves (the evaluator finds the closed
//! outline and its cutouts from whatever curves close, as for a drawn
//! board), and the physical stackup becomes the board's stackup.
use crate::board::{Board, Nm, PointNm};
use crate::items::{EdgeItem, StackLayer};
use crate::report::{Case, Report};
use crate::translate::board_point;
use sparksmith_core::StackupPresets;
use sparksmith_core::geometry::{Bounds, Point};
use sparksmith_core::library::footprint_source::SketchEdit;
use sparksmith_core::{CopperFoil, DielectricKind, Stackup};

fn mm(nm: Nm) -> f64 {
    nm as f64 / 1e6
}

/// The circle through three points, or `None` when they are in line.
pub fn circle_through(a: Point, b: Point, c: Point) -> Option<(Point, f64)> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 {
        return None;
    }
    let sq = |p: Point| p[0] * p[0] + p[1] * p[1];
    let x = (sq(a) * (b[1] - c[1]) + sq(b) * (c[1] - a[1]) + sq(c) * (a[1] - b[1])) / d;
    let y = (sq(a) * (c[0] - b[0]) + sq(b) * (a[0] - c[0]) + sq(c) * (b[0] - a[0])) / d;
    let center = [x, y];
    Some((center, (a[0] - x).hypot(a[1] - y)))
}

/// Whether `a` → `b` → `c` turns counter-clockwise.
fn ccw(a: Point, b: Point, c: Point) -> bool {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.0
}

/// The sketch curves of the board's `Edge.Cuts` drawings, in our frame.
/// Drawings that cannot be curves (an arc through three points in line)
/// are counted.
pub fn outline_edits(board: &Board, report: &mut Report) -> Vec<SketchEdit> {
    let p = |q: PointNm| board_point(board, q);
    let mut edits = Vec::with_capacity(board.items.edges.len());
    for item in &board.items.edges {
        edits.push(match item {
            EdgeItem::Line { start, end } => SketchEdit::AddLine {
                a: p(*start),
                b: p(*end),
                construction: false,
            },
            EdgeItem::Arc { start, mid, end } => {
                let (s, m, e) = (p(*start), p(*mid), p(*end));
                let Some((center, _)) = circle_through(s, m, e) else {
                    report.add(Case::OutlineDrawing);
                    continue;
                };
                // Our arcs run counter-clockwise from start to end.
                let (start, end) = if ccw(s, m, e) { (s, e) } else { (e, s) };
                SketchEdit::AddArc {
                    center,
                    start,
                    end,
                    construction: false,
                }
            }
            EdgeItem::Circle { center, radius } => SketchEdit::AddCircle {
                center: p(*center),
                radius: radius / 1e6,
                construction: false,
            },
            EdgeItem::Rect { start, end } => SketchEdit::AddRect {
                a: p(*start),
                b: p(*end),
                construction: false,
            },
            EdgeItem::Poly { points } => SketchEdit::AddPolyline {
                points: points.iter().map(|q| p(*q)).collect(),
                closed: true,
                construction: false,
            },
        });
    }
    report.add_n(Case::OutlineDrawing, board.items.edge_curves);
    report.add_n(Case::FootprintOutline, board.items.footprint_edges);
    edits
}

/// The box of the board's `Edge.Cuts` drawings in our frame (circles and
/// arcs by their full circles), or `None` without any.
pub fn outline_bounds(board: &Board) -> Option<Bounds> {
    let p = |q: PointNm| board_point(board, q);
    let mut points = Vec::new();
    for item in &board.items.edges {
        match item {
            EdgeItem::Line { start, end } | EdgeItem::Rect { start, end } => {
                points.extend([p(*start), p(*end)]);
            }
            EdgeItem::Arc { start, mid, end } => {
                let (s, m, e) = (p(*start), p(*mid), p(*end));
                points.extend([s, m, e]);
                if let Some((c, r)) = circle_through(s, m, e) {
                    points.extend([[c[0] - r, c[1] - r], [c[0] + r, c[1] + r]]);
                }
            }
            EdgeItem::Circle { center, radius } => {
                let (c, r) = (p(*center), radius / 1e6);
                points.extend([[c[0] - r, c[1] - r], [c[0] + r, c[1] + r]]);
            }
            EdgeItem::Poly { points: ring } => points.extend(ring.iter().map(|q| p(*q))),
        }
    }
    Bounds::of(points)
}

/// The board's stackup from the file's physical stackup: the preset of
/// its copper count with every stated copper, dielectric (kind, thickness,
/// Dk) and mask thickness taken over. `None` when the file states none or
/// its copper count differs from the board's layer list.
pub fn stackup(board: &Board, presets: &StackupPresets, report: &mut Report) -> Option<Stackup> {
    let layers = &board.items.stackup;
    let copper: Vec<Option<Nm>> = layers
        .iter()
        .filter_map(|l| match l {
            StackLayer::Copper { thickness, .. } => Some(*thickness),
            _ => None,
        })
        .collect();
    if copper.is_empty() || copper.len() != board.copper.len() {
        return None;
    }
    let mut stackup = presets.preset(copper.len()).ok()?;
    for (foil, thickness) in stackup.copper.iter_mut().zip(&copper) {
        if let Some(t) = thickness {
            *foil = CopperFoil {
                thickness_um: mm(*t) * 1000.0,
                plane: false,
            };
        }
    }
    // The dielectrics after each copper layer but the last, top down.
    let mut gaps: Vec<Vec<&StackLayer>> = vec![Vec::new(); copper.len().saturating_sub(1)];
    let mut k = 0usize;
    let mut started = false;
    for layer in layers {
        match layer {
            StackLayer::Copper { .. } => {
                if started {
                    k += 1;
                }
                started = true;
            }
            StackLayer::Dielectric { .. } if started => {
                if let Some(gap) = gaps.get_mut(k) {
                    gap.push(layer);
                }
            }
            _ => {}
        }
    }
    for (dielectric, gap) in stackup.dielectrics.iter_mut().zip(&gaps) {
        let mut thickness = 0;
        let mut stated = true;
        let mut parts = 0usize;
        let mut core = None;
        let mut dk = None;
        for layer in gap {
            if let StackLayer::Dielectric {
                core: is_core,
                thickness: t,
                epsilon_r,
                sublayers,
            } = layer
            {
                parts += sublayers;
                core.get_or_insert(*is_core);
                dk = dk.or(*epsilon_r);
                match t {
                    Some(t) => thickness += t,
                    None => stated = false,
                }
            }
        }
        if parts == 0 {
            continue;
        }
        if parts > 1 {
            report.add(Case::StackupSublayers);
        }
        if let Some(core) = core {
            dielectric.kind = if core {
                DielectricKind::Core
            } else {
                DielectricKind::Prepreg
            };
        }
        if stated && thickness > 0 {
            dielectric.thickness_mm = mm(thickness);
        }
        dielectric.dk = dk;
    }
    for layer in layers {
        if let StackLayer::Mask {
            thickness: Some(t), ..
        } = layer
        {
            stackup.mask_um = mm(*t) * 1000.0;
        }
    }
    if stackup.check().is_err() {
        report.add(Case::StackupRefused);
        return None;
    }
    report.add(Case::StackupImported);
    Some(stackup)
}
