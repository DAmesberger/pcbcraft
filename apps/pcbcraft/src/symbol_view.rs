//! Paints a resolved symbol unit. The schematic and the Component editor's
//! Symbol view both draw only what core resolves
//! ([`schematic::placed_unit`], [`schematic::definition_unit`]); this module
//! maps millimetres to pixels, applies the upright text rule from core and
//! picks theme colours. It never decides geometry.
use egui::epaint::{Mesh, TextShape};
use egui::{Color32, Painter, Pos2, Stroke, pos2};
use pcbcraft_core::Point;
use pcbcraft_core::geometry::{Shape, signed_area2};
use pcbcraft_core::library::graphics::SYMBOL_STROKE_MM;
use pcbcraft_core::library::{Fill, HAlign, VAlign};
use pcbcraft_core::schematic::{PlacedText, PlacedUnit};
use solidcraft_ui_egui::fonts;

/// Cap height over font size for the bundled UI font, so a text's height in
/// millimetres is the drawn cap height.
const CAP_HEIGHT: f32 = 0.7;
/// Text smaller than this many pixels is not drawn; it would not be legible.
const MIN_TEXT_PX: f32 = 3.0;
/// Curves are flattened to this chord error in pixels.
const CHORD_PX: f64 = 0.25;

/// Millimetres (y up) to screen pixels (y down).
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Screen position of the millimetre origin.
    pub origin: Pos2,
    /// Pixels per millimetre.
    pub scale: f32,
}

impl View {
    pub fn screen(&self, p: Point) -> Pos2 {
        pos2(
            self.origin.x + p[0] as f32 * self.scale,
            self.origin.y - p[1] as f32 * self.scale,
        )
    }
    pub fn world(&self, p: Pos2) -> Point {
        [
            f64::from((p.x - self.origin.x) / self.scale),
            f64::from((self.origin.y - p.y) / self.scale),
        ]
    }
    /// Stroke width in pixels for `mm`, at least one pixel when drawn.
    fn width(&self, mm: f64) -> f32 {
        (mm as f32 * self.scale).max(1.0)
    }
}

/// Theme colours for one drawn unit.
#[derive(Clone, Copy, Debug)]
pub struct Ink {
    /// Outlines and `Fill::Solid`.
    pub stroke: Color32,
    /// `Fill::Body`: the theme's symbol background.
    pub body: Color32,
    /// Graphic text such as designator and values.
    pub text: Color32,
    /// Pin stubs and decorations.
    pub pin: Color32,
    /// Pin names and numbers.
    pub pin_text: Color32,
}

/// Paint everything `unit` resolves to, in order: shapes, pins, text.
pub fn paint(painter: &Painter, view: View, unit: &PlacedUnit, ink: Ink) {
    for shape in &unit.shapes {
        let fill = match shape.fill {
            Fill::None => None,
            Fill::Body => Some(ink.body),
            Fill::Solid => Some(ink.stroke),
        };
        let stroke = (shape.width > 0.0).then(|| Stroke::new(view.width(shape.width), ink.stroke));
        paint_shape(painter, view, &shape.shape, fill, stroke);
    }
    let pin_stroke = Stroke::new(view.width(SYMBOL_STROKE_MM), ink.pin);
    for pin in &unit.pins {
        painter.line_segment(
            [view.screen(pin.end), view.screen(pin.line_end)],
            pin_stroke,
        );
        for decoration in &pin.decorations {
            paint_shape(painter, view, decoration, None, Some(pin_stroke));
        }
        for text in pin.name.iter().chain(&pin.number) {
            paint_text(painter, view, text, ink.pin_text);
        }
    }
    for text in unit.texts.iter().chain(&unit.fields) {
        paint_text(painter, view, text, ink.text);
    }
}

/// Paint one core shape, flattened to sub-pixel chords.
pub fn paint_shape(
    painter: &Painter,
    view: View,
    shape: &Shape,
    fill: Option<Color32>,
    stroke: Option<Stroke>,
) {
    let tolerance = (CHORD_PX / f64::from(view.scale.max(f32::EPSILON))).max(1e-4);
    let points = shape.flatten(tolerance);
    let screen: Vec<Pos2> = points.iter().map(|p| view.screen(*p)).collect();
    if screen.len() < 2 {
        return;
    }
    let closed = shape.is_closed();
    if closed && let Some(color) = fill {
        fill_polygon(painter, &points, &screen, color);
    }
    if let Some(stroke) = stroke {
        painter.add(if closed {
            egui::Shape::closed_line(screen, stroke)
        } else {
            egui::Shape::line(screen, stroke)
        });
    }
}

/// Fill a simple polygon. Convex outlines go to egui directly; concave ones
/// are ear-clipped into a mesh, since egui fills only convex paths.
fn fill_polygon(painter: &Painter, points: &[Point], screen: &[Pos2], color: Color32) {
    if is_convex(points) {
        painter.add(egui::Shape::convex_polygon(
            screen.to_vec(),
            color,
            Stroke::NONE,
        ));
        return;
    }
    let mut mesh = Mesh::default();
    for p in screen {
        mesh.colored_vertex(*p, color);
    }
    for [a, b, c] in ear_clip(points) {
        mesh.add_triangle(a, b, c);
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn is_convex(points: &[Point]) -> bool {
    let n = points.len();
    let mut sign = 0.0_f64;
    for i in 0..n {
        let c = cross(points[i], points[(i + 1) % n], points[(i + 2) % n]);
        if c.abs() <= 1e-12 {
            continue;
        }
        if sign != 0.0 && c.signum() != sign {
            return false;
        }
        sign = c.signum();
    }
    true
}

/// Triangles of a simple polygon by ear clipping. Bounded by the core's
/// point limits; stops (leaving the rest unfilled) if no ear is found, as
/// on a self-intersecting outline that publishing would refuse anyway.
fn ear_clip(points: &[Point]) -> Vec<[u32; 3]> {
    let ccw = signed_area2(points) >= 0.0;
    let mut remaining: Vec<usize> = (0..points.len()).collect();
    if !ccw {
        remaining.reverse();
    }
    let mut out = Vec::new();
    while remaining.len() > 3 {
        let n = remaining.len();
        let ear = (0..n).find(|&i| {
            let (a, b, c) = (
                remaining[(i + n - 1) % n],
                remaining[i],
                remaining[(i + 1) % n],
            );
            let (pa, pb, pc) = (points[a], points[b], points[c]);
            cross(pa, pb, pc) > 1e-12
                && remaining.iter().all(|&k| {
                    k == a
                        || k == b
                        || k == c
                        || !(cross(pa, pb, points[k]) >= 0.0
                            && cross(pb, pc, points[k]) >= 0.0
                            && cross(pc, pa, points[k]) >= 0.0)
                })
        });
        let Some(i) = ear else {
            return out;
        };
        let (a, b, c) = (
            remaining[(i + n - 1) % n],
            remaining[i],
            remaining[(i + 1) % n],
        );
        out.push([a as u32, b as u32, c as u32]);
        remaining.remove(i);
    }
    if let [a, b, c] = remaining[..] {
        out.push([a as u32, b as u32, c as u32]);
    }
    out
}

/// Paint text at its justification anchor, turned upright by the core rule.
/// Each line of a multi-line text goes where the core layout puts it
/// ([`PlacedText::lines`]), so line spacing and justification match the
/// core metric. Reflected text (a future occurrence mirror) is drawn
/// unreflected, which keeps it legible.
pub fn paint_text(painter: &Painter, view: View, text: &PlacedText, color: Color32) {
    let size = text.height as f32 * view.scale / CAP_HEIGHT;
    if text.text.trim().is_empty() || !size.is_finite() || size < MIN_TEXT_PX {
        return;
    }
    let (rotation_deg, lines) = text.lines();
    let justify = text.line_justify();
    for line in lines {
        if line.text.is_empty() {
            continue;
        }
        let galley = painter.layout_no_wrap(line.text.to_owned(), fonts::medium(size), color);
        let box_size = galley.size();
        // Offset from the line's anchor to the galley's top-left corner,
        // unrotated, screen axes (y down); every line centres vertically.
        let dx = match justify.h {
            HAlign::Left => 0.0,
            HAlign::Center => -box_size.x / 2.0,
            HAlign::Right => -box_size.x,
        };
        let dy = match justify.v {
            VAlign::Top => 0.0,
            VAlign::Middle => -box_size.y / 2.0,
            VAlign::Bottom => -box_size.y,
        };
        // World counter-clockwise is screen clockwise-negative.
        let angle = -(rotation_deg as f32).to_radians();
        let (s, c) = angle.sin_cos();
        let anchor = view.screen(line.at);
        let pos = anchor + egui::vec2(dx * c - dy * s, dx * s + dy * c);
        painter.add(TextShape::new(pos, galley, color).with_angle(angle));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(points: &[Point], triangles: &[[u32; 3]]) -> f64 {
        triangles
            .iter()
            .map(|[a, b, c]| {
                cross(
                    points[*a as usize],
                    points[*b as usize],
                    points[*c as usize],
                )
                .abs()
                    / 2.0
            })
            .sum()
    }

    #[test]
    fn concave_outlines_triangulate_to_their_area_in_either_winding() {
        // An L of area 3 (three unit squares).
        let mut l = vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        assert!(!is_convex(&l));
        let triangles = ear_clip(&l);
        assert_eq!(triangles.len(), 4);
        assert!((area(&l, &triangles) - 3.0).abs() < 1e-9);
        l.reverse();
        let triangles = ear_clip(&l);
        assert!((area(&l, &triangles) - 3.0).abs() < 1e-9);
        assert!(is_convex(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]));
    }

    #[test]
    fn view_maps_millimetres_y_up_and_back() {
        let view = View {
            origin: pos2(100.0, 50.0),
            scale: 4.0,
        };
        assert_eq!(view.screen([2.5, 1.0]), pos2(110.0, 46.0));
        assert_eq!(view.world(pos2(110.0, 46.0)), [2.5, 1.0]);
        assert_eq!(view.width(0.1), 1.0, "at least one pixel");
        assert_eq!(view.width(0.5), 2.0);
    }
}
