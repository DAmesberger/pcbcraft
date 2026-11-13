//! The board interface's datums drawn on the Layout canvas and in PCB 3D
//! (slice B6): one painter over a view's board → screen mapping
//! ([`Surface`]), so both views draw the same thing from the same
//! `interface.list` rows. Datums are drawn from the document, never from
//! the published file, as the design note's "Reference display in 3D"
//! asks; nothing here builds geometry, so selection and the camera only
//! repaint.
//!
//! A frame is its two axes, an axis its line and (seen end-on) its circle,
//! a profile its loops, a volume its two loops and the edges between them;
//! a parameter has no place to draw. The board's own frame, faces and
//! outline would only trace the board again: they show while their row is
//! hovered or selected. A part's datum and the hot datum carry a chip with
//! their name, the chip on exported objects of the design note.
use super::dimensions::{Surface, arc_points};
use egui::{Color32, FontId, Pos2, Rect, Shape, Stroke, vec2};
use sparksmith_core::interface::{DatumGeometry, DatumRow, Frame, Vec3};
use sparksmith_core::library::{Outline, Segment};
use sparksmith_ui::theme::Tokens;

/// Length of a frame's drawn axes, mm.
const FRAME_AXIS_MM: f64 = 3.0;
/// Length drawn of an axis with no hole to bound it, mm.
const AXIS_MM: f64 = 2.0;
/// Points drawn per loop at most, so a pathological profile stays cheap.
const MAX_LOOP_POINTS: usize = 4096;

/// How one datum is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Look {
    /// Shown with the rest of the interface.
    Quiet,
    /// Its row is hovered or selected.
    Hot,
}

/// Whether a row is the board itself (its frame, faces, thickness or
/// outline), drawn only while hot.
pub(super) fn traces_the_board(row: &DatumRow) -> bool {
    matches!(
        row.role.as_str(),
        "board.frame" | "board.top" | "board.bottom" | "board.thickness" | "board.outline"
    )
}

/// Whether a row carries its name on the canvas when quiet: a part's own
/// datums do (an opening, an axis, an envelope), the many holes and
/// cutouts only when hot.
fn chipped(row: &DatumRow) -> bool {
    row.role.starts_with("part.")
}

fn add(a: Vec3, b: Vec3, s: f64) -> [f64; 3] {
    [
        a.0[0] + b.0[0] * s,
        a.0[1] + b.0[1] * s,
        a.0[2] + b.0[2] * s,
    ]
}

/// A frame's x–y plane point at height `z` along its normal.
fn in_frame(frame: &Frame, p: [f64; 2], z: f64) -> [f64; 3] {
    let n = cross(frame.x, frame.y);
    let o = frame.origin.0;
    let (x, y) = (frame.x.0, frame.y.0);
    [
        o[0] + x[0] * p[0] + y[0] * p[1] + n[0] * z,
        o[1] + x[1] * p[0] + y[1] * p[1] + n[1] * z,
        o[2] + x[2] * p[0] + y[2] * p[1] + n[2] * z,
    ]
}

fn cross(a: Vec3, b: Vec3) -> [f64; 3] {
    let (a, b) = (a.0, b.0);
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A loop's points in its own plane, arcs flattened.
pub(super) fn loop_points(outline: &Outline) -> Vec<[f64; 2]> {
    let mut points = vec![outline.start];
    let mut at = outline.start;
    for segment in &outline.segments {
        match *segment {
            Segment::Line { to } => points.push(to),
            Segment::Arc { to, bulge } => {
                points.extend(arc_points(at, to, bulge).into_iter().skip(1))
            }
        }
        at = segment.to();
        if points.len() >= MAX_LOOP_POINTS {
            break;
        }
    }
    points
}

/// The point a datum's chip is put beside.
pub(super) fn anchor(geometry: &DatumGeometry) -> Option<[f64; 3]> {
    match geometry {
        DatumGeometry::Frame { frame } => Some(frame.origin.0),
        DatumGeometry::Plane { origin, .. } => Some(origin.0),
        DatumGeometry::Axis { point, .. } => Some(point.0),
        DatumGeometry::Profile { frame, .. } | DatumGeometry::Volume { frame, .. } => {
            Some(frame.origin.0)
        }
        DatumGeometry::Parameter { .. } => None,
    }
}

fn polyline(
    painter: &egui::Painter,
    surface: &dyn Surface,
    points: impl IntoIterator<Item = [f64; 3]>,
    stroke: Stroke,
) {
    let screen: Option<Vec<Pos2>> = points.into_iter().map(|p| surface.to_screen(p)).collect();
    if let Some(screen) = screen.filter(|s| s.len() >= 2) {
        painter.add(Shape::line(screen, stroke));
    }
}

fn segment(painter: &egui::Painter, surface: &dyn Surface, a: [f64; 3], b: [f64; 3], s: Stroke) {
    if let (Some(a), Some(b)) = (surface.to_screen(a), surface.to_screen(b)) {
        painter.line_segment([a, b], s);
    }
}

/// Draw one datum's geometry; its chip is [`chip`].
pub(super) fn geometry(
    painter: &egui::Painter,
    surface: &dyn Surface,
    geometry: &DatumGeometry,
    stroke: Stroke,
) {
    match geometry {
        DatumGeometry::Frame { frame } => {
            let o = frame.origin;
            segment(
                painter,
                surface,
                o.0,
                add(o, frame.x, FRAME_AXIS_MM),
                stroke,
            );
            segment(
                painter,
                surface,
                o.0,
                add(o, frame.y, FRAME_AXIS_MM),
                stroke,
            );
        }
        DatumGeometry::Plane { origin, normal } => {
            // A small cross on the plane at its origin, and its normal.
            if let Some(c) = surface.to_screen(origin.0) {
                let r = 4.0;
                painter.line_segment([c - vec2(r, 0.0), c + vec2(r, 0.0)], stroke);
                painter.line_segment([c - vec2(0.0, r), c + vec2(0.0, r)], stroke);
            }
            segment(
                painter,
                surface,
                origin.0,
                add(*origin, *normal, AXIS_MM),
                stroke,
            );
        }
        DatumGeometry::Axis {
            point,
            direction,
            diameter,
        } => {
            let p = *point;
            segment(painter, surface, p.0, add(p, *direction, AXIS_MM), stroke);
            // Seen end-on in Layout, an axis is its circle and its centre.
            if let Some(c) = surface.to_screen(p.0) {
                let radius = diameter
                    .map(|d| [p.0[0] + d / 2.0, p.0[1], p.0[2]])
                    .and_then(|edge| surface.to_screen(edge))
                    .map_or(0.0, |e| (e - c).length());
                if radius > 1.0 {
                    painter.circle_stroke(c, radius, stroke);
                }
                let r = (radius * 0.6).clamp(3.0, 6.0);
                painter.line_segment([c - vec2(r, 0.0), c + vec2(r, 0.0)], stroke);
                painter.line_segment([c - vec2(0.0, r), c + vec2(0.0, r)], stroke);
            }
        }
        DatumGeometry::Profile { frame, loops } => {
            for outline in loops {
                let points = loop_points(outline);
                polyline(
                    painter,
                    surface,
                    points.iter().map(|p| in_frame(frame, *p, 0.0)),
                    stroke,
                );
            }
        }
        DatumGeometry::Volume {
            frame,
            loops,
            height,
        } => {
            for outline in loops {
                let points = loop_points(outline);
                for z in [0.0, *height] {
                    polyline(
                        painter,
                        surface,
                        points.iter().map(|p| in_frame(frame, *p, z)),
                        stroke,
                    );
                }
                // The edges between the two loops at the loop's corners.
                for p in outline
                    .segments
                    .iter()
                    .filter(|s| matches!(s, Segment::Line { .. }))
                    .map(Segment::to)
                {
                    segment(
                        painter,
                        surface,
                        in_frame(frame, p, 0.0),
                        in_frame(frame, p, *height),
                        stroke,
                    );
                }
            }
        }
        DatumGeometry::Parameter { .. } => {}
    }
}

/// A datum's name in a small chip beside its anchor; the chip's rect.
pub(super) fn chip(
    painter: &egui::Painter,
    surface: &dyn Surface,
    geometry: &DatumGeometry,
    name: &str,
    ink: Color32,
) -> Option<Rect> {
    let t = Tokens::get();
    let at = surface.to_screen(anchor(geometry)?)?;
    let galley = painter.layout_no_wrap(name.to_owned(), FontId::proportional(t.font_small), ink);
    let min = at + vec2(t.space_8, -t.space_8 - galley.size().y);
    let back = Rect::from_min_size(min, galley.size() + vec2(t.space_8, t.space_4));
    painter.rect_filled(back, t.radius_sm, t.panel);
    painter.rect_stroke(
        back,
        t.radius_sm,
        Stroke::new(1.0, ink.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    painter.galley(back.min + vec2(t.space_4, t.space_2), galley, ink);
    Some(back)
}

/// Draw `rows` with their looks: geometry first, then the chips above it.
/// Returns the chips drawn, by datum, for hit tests.
pub(super) fn paint(
    painter: &egui::Painter,
    surface: &dyn Surface,
    rows: &[(&DatumRow, Look)],
) -> Vec<(sparksmith_core::model::interface::DatumId, Rect)> {
    let t = Tokens::get();
    let quiet = Stroke::new(1.25, t.construction_geom);
    let hot = Stroke::new(2.0, t.accent);
    for (row, look) in rows {
        if let Some(g) = &row.geometry {
            let stroke = if *look == Look::Hot { hot } else { quiet };
            geometry(painter, surface, g, stroke);
        }
    }
    let mut chips = Vec::new();
    for (row, look) in rows {
        let Some(g) = &row.geometry else { continue };
        if *look == Look::Hot || chipped(row) {
            let ink = if *look == Look::Hot {
                t.accent
            } else {
                t.construction_geom
            };
            if let Some(rect) = chip(painter, surface, g, &row.name, ink) {
                chips.push((row.id, rect));
            }
        }
    }
    chips
}
