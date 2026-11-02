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
use pcbcraft_core::library::{Fill, HAlign, TextMetric, VAlign};
use pcbcraft_core::schematic::{PlacedText, PlacedUnit};
use solidcraft_ui_egui::fonts;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, OnceLock};

/// Cap height over font size for the bundled UI font, so a text's height in
/// millimetres is the drawn cap height.
const CAP_HEIGHT: f32 = 0.7;
/// Text smaller than this many pixels is not drawn; it would not be legible.
const MIN_TEXT_PX: f32 = 3.0;
/// Curves are flattened to this chord error in pixels.
const CHORD_PX: f64 = 0.25;

/// The text box rule views pick and outline text with: the core metric
/// with the bundled font's mean upper-case advance per cap height, never
/// narrower than the core default. The core's approximate advance is
/// narrower than this font's capitals, so text drawn with it overran its
/// selection box; picking, box selection and the outline all use this one
/// metric, so the box that picks is the box drawn.
pub fn metric(painter: &Painter) -> TextMetric {
    const SAMPLE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let size = 100.0;
    let width = painter
        .layout_no_wrap(SAMPLE.to_owned(), fonts::medium(size), Color32::WHITE)
        .size()
        .x;
    let advance = f64::from(width / SAMPLE.len() as f32 / (size * CAP_HEIGHT));
    let core = TextMetric::default();
    TextMetric {
        advance: if advance.is_finite() {
            advance.max(core.advance)
        } else {
            core.advance
        },
    }
}

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

/// Paint one core shape, flattened to sub-pixel chords. The flattened ring
/// and a concave fill's triangles come from a per-frame cache in
/// millimetres ([`flat`]), so an unchanged shape is flattened and
/// triangulated once per zoom step, not on every frame.
pub fn paint_shape(
    painter: &Painter,
    view: View,
    shape: &Shape,
    fill: Option<Color32>,
    stroke: Option<Stroke>,
) {
    let flat = flat(painter.ctx(), shape, view.scale);
    let screen: Vec<Pos2> = flat.points.iter().map(|p| view.screen(*p)).collect();
    if screen.len() < 2 {
        return;
    }
    let closed = shape.is_closed();
    if closed && let Some(color) = fill {
        fill_polygon(painter, &flat, &screen, color);
    }
    if let Some(stroke) = stroke {
        painter.add(if closed {
            egui::Shape::closed_line(screen, stroke)
        } else {
            egui::Shape::line(screen, stroke)
        });
    }
}

/// A shape's outline as a dashed line (courtyards): `dash` and `gap` in
/// screen points. No fill.
pub fn paint_dashed(
    painter: &Painter,
    view: View,
    shape: &Shape,
    stroke: Stroke,
    dash: f32,
    gap: f32,
) {
    let flat = flat(painter.ctx(), shape, view.scale);
    let mut screen: Vec<Pos2> = flat.points.iter().map(|p| view.screen(*p)).collect();
    if screen.len() < 2 {
        return;
    }
    if shape.is_closed()
        && let Some(first) = screen.first().copied()
    {
        screen.push(first);
    }
    painter.extend(egui::Shape::dashed_line(&screen, stroke, dash, gap));
}

/// A shape flattened for one zoom step, in millimetres.
struct Flat {
    /// The shape flattened, kept to tell hash collisions apart.
    shape: Shape,
    points: Vec<Point>,
    convex: bool,
    /// Ear-clipped triangles of a concave ring, built when first filled.
    triangles: OnceLock<Vec<[u32; 3]>>,
}

/// Flattened shapes by shape hash and zoom step. Entries live while they
/// are drawn: each frame keeps what it used and drops what the frame
/// before used and this one did not, like egui's frame caches.
#[derive(Clone, Default)]
struct FlatCache {
    frame: u64,
    current: HashMap<(u64, i32), Arc<Flat>>,
    previous: HashMap<(u64, i32), Arc<Flat>>,
}

/// Zoom steps per doubling of scale: the chord stays under [`CHORD_PX`]
/// within a step, so a cached ring is never coarser than drawing it anew.
const ZOOM_STEPS_PER_OCTAVE: f32 = 4.0;

fn shape_hash(shape: &Shape) -> u64 {
    let mut h = DefaultHasher::new();
    let mut put = |values: &[f64]| values.iter().for_each(|v| v.to_bits().hash(&mut h));
    match shape {
        Shape::Line { a, b } => put(&[0.0, a[0], a[1], b[0], b[1]]),
        Shape::Rect { a, b, radius } => put(&[1.0, a[0], a[1], b[0], b[1], *radius]),
        Shape::Circle { center, radius } => put(&[2.0, center[0], center[1], *radius]),
        Shape::Arc {
            center,
            radius,
            start_deg,
            sweep_deg,
        } => put(&[3.0, center[0], center[1], *radius, *start_deg, *sweep_deg]),
        Shape::Polyline { points, radii } | Shape::Polygon { points, radii } => {
            put(&[if shape.is_closed() { 5.0 } else { 4.0 }]);
            points.iter().for_each(|p| put(p));
            put(&[-1.0]);
            put(radii);
        }
    }
    h.finish()
}

/// `shape` flattened for `scale` pixels per millimetre, from the cache.
fn flat(ctx: &egui::Context, shape: &Shape, scale: f32) -> Arc<Flat> {
    let step = (scale.max(f32::EPSILON).log2() * ZOOM_STEPS_PER_OCTAVE).ceil();
    let step = if step.is_finite() {
        step.clamp(-400.0, 400.0)
    } else {
        0.0
    };
    // The finest scale of the step, so its chord error stays sub-pixel.
    let step_scale = 2f64.powf(f64::from(step / ZOOM_STEPS_PER_OCTAVE));
    let key = (shape_hash(shape), step as i32);
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<FlatCache>(egui::Id::new("symbol-view-flat"));
        if cache.frame != frame {
            cache.previous = std::mem::take(&mut cache.current);
            cache.frame = frame;
        }
        let reuse = |f: &Arc<Flat>| f.shape == *shape;
        if let Some(f) = cache.current.get(&key).filter(|f| reuse(f)) {
            return Arc::clone(f);
        }
        let f = match cache.previous.remove(&key).filter(reuse) {
            Some(f) => f,
            None => {
                let tolerance = (CHORD_PX / step_scale).max(1e-4);
                let points = shape.flatten(tolerance);
                Arc::new(Flat {
                    shape: shape.clone(),
                    convex: is_convex(&points),
                    points,
                    triangles: OnceLock::new(),
                })
            }
        };
        cache.current.insert(key, Arc::clone(&f));
        f
    })
}

/// Fill a simple polygon. Convex outlines go to egui directly; concave ones
/// are ear-clipped into a mesh once per cached ring, since egui fills only
/// convex paths.
fn fill_polygon(painter: &Painter, flat: &Flat, screen: &[Pos2], color: Color32) {
    if flat.convex {
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
    for [a, b, c] in flat.triangles.get_or_init(|| ear_clip(&flat.points)) {
        mesh.add_triangle(*a, *b, *c);
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
    use solidcraft_ui_egui::fonts;

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

    /// Every generated symbol kind resolves and paints: its arcs, solid
    /// arrow heads and body-filled outlines become egui shapes with finite
    /// positions, and its designator and name are drawn.
    #[test]
    fn every_generated_symbol_kind_paints() -> pcbcraft_core::Result<()> {
        use pcbcraft_core::functions::{Direction, TerminalFunctions};
        use pcbcraft_core::library::symbol_style::{self, SymbolConventions, SymbolKind};
        use pcbcraft_core::library::{ComponentDefinition, Library, Terminal, TerminalClass};
        use pcbcraft_core::schematic::{DefinitionUnit, definition_unit};
        use pcbcraft_core::{DefinitionId, FunctionId, TerminalId, UnitId};

        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let view = View {
            origin: pos2(200.0, 200.0),
            scale: 12.0,
        };
        let ink = Ink {
            stroke: Color32::BLACK,
            body: Color32::LIGHT_YELLOW,
            text: Color32::DARK_GRAY,
            pin: Color32::BLACK,
            pin_text: Color32::GRAY,
        };
        let library = Library::default();
        for kind in SymbolKind::ALL {
            let Some(count) = kind.terminal_count() else {
                continue;
            };
            let terminals: Vec<TerminalId> = (1..=count as u32).map(TerminalId).collect();
            let symbol = symbol_style::generate(
                kind,
                SymbolConventions::default(),
                DefinitionId(1),
                &terminals,
            )?;
            let component = ComponentDefinition {
                designator_prefix: kind.prefix().to_owned(),
                terminals: terminals
                    .iter()
                    .map(|id| Terminal {
                        id: *id,
                        name: id.to_string(),
                        class: TerminalClass::Electrical(TerminalFunctions::fixed(
                            FunctionId(id.0 + 10),
                            Direction::Passive,
                        )),
                    })
                    .collect(),
                ..ComponentDefinition::default()
            };
            let placed = definition_unit(
                &library,
                DefinitionUnit {
                    symbol: &symbol,
                    unit: UnitId(1),
                    component: &component,
                    component_name: kind.label(),
                    physical: None,
                },
            )?;
            assert_eq!(placed.pins.len(), count, "{kind:?}");
            assert!(!placed.texts.is_empty(), "{kind:?} texts");
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                paint(ui.painter(), view, &placed, ink);
            });
            output.textures_delta.clear();
            let painted = output.shapes.len();
            assert!(
                painted >= placed.shapes.len() + placed.pins.len(),
                "{kind:?}: {painted} shapes painted"
            );
            for clipped in &output.shapes {
                let rect = clipped.shape.visual_bounding_rect();
                assert!(
                    !rect.is_positive() || (rect.min.x.is_finite() && rect.max.y.is_finite()),
                    "{kind:?}: {rect:?}"
                );
            }
        }
        Ok(())
    }

    /// A concave comb at the core's point limit, every corner rounded: the
    /// worst outline to flatten and ear-clip.
    fn comb() -> Shape {
        let n = pcbcraft_core::library::graphics::MAX_SHAPE_POINTS;
        let teeth = (n - 2) / 4;
        let mut points = vec![[0.0, -5.0]];
        for i in 0..teeth {
            let x = i as f64 * 2.0;
            points.extend([[x, 0.0], [x, 4.0], [x + 1.0, 4.0], [x + 1.0, 0.0]]);
        }
        points.push([teeth as f64 * 2.0, -5.0]);
        let radii = vec![0.2; points.len()];
        Shape::Polygon { points, radii }
    }

    /// Painting reuses the flattened ring and triangles across frames: a
    /// second frame of the same symbol neither flattens nor ear-clips
    /// again. Timings are printed as measurements, not asserted targets.
    #[test]
    fn repaints_reuse_flattened_shapes_and_triangles() {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let shape = comb();
        let view = View {
            origin: pos2(0.0, 0.0),
            scale: 20.0,
        };
        let mut rings = Vec::new();
        let mut times = Vec::new();
        for _ in 0..3 {
            let start = std::time::Instant::now();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                // Sixteen occurrences of one symbol share one cached ring.
                for _ in 0..16 {
                    paint_shape(
                        ui.painter(),
                        view,
                        &shape,
                        Some(Color32::WHITE),
                        Some(Stroke::new(1.0, Color32::BLACK)),
                    );
                }
                rings.push(flat(ui.ctx(), &shape, view.scale));
            });
            times.push(start.elapsed());
            output.textures_delta.clear();
        }
        let first = &rings[0];
        assert!(first.points.len() > pcbcraft_core::library::graphics::MAX_SHAPE_POINTS);
        assert!(!first.convex);
        assert!(first.triangles.get().is_some(), "filled once");
        assert!(Arc::ptr_eq(first, &rings[1]) && Arc::ptr_eq(first, &rings[2]));
        // Another zoom step flattens again; the old one is dropped once a
        // frame passes without it.
        let mut zoomed = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            zoomed = Some(flat(ui.ctx(), &shape, view.scale * 4.0));
        });
        output.textures_delta.clear();
        assert!(zoomed.is_some_and(|z| !Arc::ptr_eq(first, &z)));
        eprintln!(
            "comb of {} flattened points, 16 copies: cold frame {:?}, warm frames {:?} and {:?}",
            first.points.len(),
            times[0],
            times[1],
            times[2]
        );
    }
}
