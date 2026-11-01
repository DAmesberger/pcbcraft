//! The Footprint view canvas: draws the working footprint from its typed
//! pads and graphics (every pad shape and rotation, holes, display-only
//! mask and paste openings, silkscreen, courtyard and assembly drawings,
//! the package envelope from `FootprintDefinition::envelope_outline`) and
//! picks pads by their real land, not their box. A drag moves the picked
//! pad on the footprint grid; the editor commits the finished drag once.
//! Scroll zooms about the pointer, and a middle drag (or Space + drag)
//! pans, as in the Symbol view.
use super::PartEditor;
use super::footprint_geometry as geo;
use super::symbol_canvas::ViewState;
use crate::symbol_view::{self, View};
use egui::{Align2, Color32, Rect, Response, Stroke, Ui, vec2};
use pcbcraft_core::geometry::{Bounds, Shape};
use pcbcraft_core::library::graphics::{DrawingLayer, Fill, Item, TextContent};
use pcbcraft_core::library::{PadKind, PadSide, PlacedText};
use pcbcraft_core::{PadId, Point};
use solidcraft_ui_egui::{fonts, theme::Tokens};

/// Pick distance around a land, in pixels.
const PICK_PX: f32 = 3.0;
/// Curves are flattened to this chord error, in pixels.
const CHORD_PX: f32 = 0.25;

/// Which display layers the Footprint view shows. Copper, holes and the
/// package envelope are always shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layers {
    pub mask: bool,
    pub paste: bool,
    pub silkscreen: bool,
    pub courtyard: bool,
    pub assembly: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            mask: true,
            paste: true,
            silkscreen: true,
            courtyard: true,
            assembly: true,
        }
    }
}

/// The ribbon's layer toggles: id, icon, label, tip.
pub(super) const LAYER_TOOLS: [(&str, &str, &str, &str); 5] = [
    (
        "fp-mask",
        "offset_face",
        "Mask",
        "Show the solder-mask openings (display only)",
    ),
    (
        "fp-paste",
        "paste",
        "Paste",
        "Show the solder-paste openings (display only)",
    ),
    ("fp-silk", "text", "Silk", "Show the silkscreen drawings"),
    (
        "fp-courtyard",
        "bounding_solid",
        "Courtyard",
        "Show the courtyard",
    ),
    (
        "fp-assembly",
        "component",
        "Assembly",
        "Show the assembly drawing",
    ),
];

impl Layers {
    pub fn get(&self, id: &str) -> Option<bool> {
        Some(match id {
            "fp-mask" => self.mask,
            "fp-paste" => self.paste,
            "fp-silk" => self.silkscreen,
            "fp-courtyard" => self.courtyard,
            "fp-assembly" => self.assembly,
            _ => return None,
        })
    }
    /// Flip one layer; false when `id` is not a layer toggle.
    pub fn toggle(&mut self, id: &str) -> bool {
        let flag = match id {
            "fp-mask" => &mut self.mask,
            "fp-paste" => &mut self.paste,
            "fp-silk" => &mut self.silkscreen,
            "fp-courtyard" => &mut self.courtyard,
            "fp-assembly" => &mut self.assembly,
            _ => return false,
        };
        *flag = !*flag;
        true
    }
    fn drawing(&self, layer: DrawingLayer) -> bool {
        match layer {
            DrawingLayer::Silkscreen => self.silkscreen,
            DrawingLayer::Courtyard => self.courtyard,
            DrawingLayer::Assembly => self.assembly,
        }
    }
}

/// A pad drag in progress: the pad, where it started and where it was
/// grabbed, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PadDrag {
    pad: PadId,
    start: Point,
    grab: Point,
}

/// Domain colours of the footprint layers (theme-independent overlays).
struct Ink {
    mask: Color32,
    paste: Color32,
    courtyard: Color32,
    assembly: Color32,
    silk: Color32,
    hole: Color32,
}

fn ink() -> Ink {
    let t = Tokens::get();
    Ink {
        mask: Color32::from_rgba_unmultiplied(64, 168, 96, 70),
        paste: Color32::from_rgba_unmultiplied(170, 170, 178, 80),
        courtyard: Color32::from_rgb(196, 86, 214),
        assembly: t.text_dim,
        silk: t.text,
        hole: t.panel,
    }
}

/// Scroll pixels per e-fold of zoom: one wheel notch (about 50 px) zooms
/// by about 13 %.
const ZOOM_PX: f32 = 400.0;
/// Share of the canvas the fitted footprint fills.
const FIT_FILL: f64 = 0.7;

/// The view that centres `bounds` and fills [`FIT_FILL`] of `rect`.
fn fitted(bounds: Bounds, rect: Rect) -> ViewState {
    let [w, h] = bounds.size();
    let scale = (rect.width() as f64 * FIT_FILL / w.max(0.5))
        .min(rect.height() as f64 * FIT_FILL / h.max(0.5))
        .clamp(0.5, 400.0) as f32;
    let [cx, cy] = bounds.center();
    ViewState {
        pan: vec2(-(cx as f32) * scale, cy as f32 * scale),
        scale,
    }
}

/// Everything the view fits: lands, the envelope and drawings.
fn drawn_bounds(set: &super::WorkingSet) -> Bounds {
    let envelope = set.footprint.envelope_outline(&set.package);
    set.footprint
        .pads
        .iter()
        .map(|p| p.bounds())
        .chain(set.footprint.graphics.iter().map(|g| match &g.item {
            Item::Shape { shape, width, .. } => shape.bounds(*width),
            Item::Text(text) => Bounds::point(text.at),
        }))
        .fold(envelope, Bounds::union)
}

impl PartEditor {
    pub(super) fn footprint_canvas(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        response: &Response,
        grid: f64,
    ) {
        let t = Tokens::get();
        self.post_footprint_refusals(ui.ctx());
        let Some(set) = &self.draft else {
            return;
        };
        let view_state = *self
            .footprint_view
            .get_or_insert_with(|| fitted(drawn_bounds(set), rect));
        let view = View {
            origin: rect.center() + view_state.pan,
            scale: view_state.scale,
        };
        self.footprint_navigation(ui, rect, response, view);
        let view = match self.footprint_view {
            Some(v) => View {
                origin: rect.center() + v.pan,
                scale: v.scale,
            },
            None => view,
        };
        let chord = f64::from(CHORD_PX / view.scale);
        let panning = response.dragged_by(egui::PointerButton::Middle)
            || (self.space_held && response.dragged_by(egui::PointerButton::Primary));
        // Pick on press, then move the picked pad while dragging.
        // A drag picks where the button went down, not where it started
        // counting as a drag.
        let sketch_took = self.footprint_sketch_input(ui, response, view, panning);
        let pressed_at = if response.drag_started() {
            ui.input(|i| i.pointer.press_origin())
        } else {
            response.interact_pointer_pos()
        };
        if !panning
            && !sketch_took
            && (response.clicked() || response.drag_started_by(egui::PointerButton::Primary))
            && let Some(p) = pressed_at
            && let Some(set) = &self.draft
        {
            let at = view.world(p);
            let tolerance = f64::from(PICK_PX / view.scale);
            let picked = geo::pick(&set.footprint.pads, at, tolerance, chord)
                .and_then(|i| set.footprint.pads.get(i));
            self.selected_pad = picked.map(|pad| pad.id);
            if let Some(electrical) = picked
                .and_then(|pad| pad.terminal)
                .and_then(|t| set.binding.terminal_of(t))
            {
                self.selected = Some(electrical);
            }
            self.pad_drag = picked
                .filter(|_| response.drag_started())
                .filter(|_| set.footprint.source.is_none())
                .map(|pad| PadDrag {
                    pad: pad.id,
                    start: pad.position,
                    grab: at,
                });
        }
        if let Some(drag) = self.pad_drag {
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(p) = response.interact_pointer_pos()
                && let Some(set) = &mut self.draft
                && let Some(pad) = set.footprint.pads.iter_mut().find(|q| q.id == drag.pad)
            {
                let at = view.world(p);
                let free = ui.input(|i| i.modifiers.alt);
                let snap = |v: f64| {
                    if free || !grid.is_finite() || grid <= 0.0 {
                        v
                    } else {
                        (v / grid).round() * grid
                    }
                };
                pad.position = [
                    snap(drag.start[0] + at[0] - drag.grab[0]),
                    snap(drag.start[1] + at[1] - drag.grab[1]),
                ];
            }
            if !response.dragged() {
                self.pad_drag = None;
            }
        }
        let Some(set) = &self.draft else {
            return;
        };
        let painter = ui.painter().with_clip_rect(rect);
        let ink = ink();
        let colors = crate::palette::board();
        let layers = self.footprint_layers;
        let selected_pad = self.selected_pad;
        let defaults = set.footprint.defaults;
        let thin = |color| Some(Stroke::new(1.0, color));
        // Package envelope and the datum cross.
        let envelope = set.footprint.envelope_outline(&set.package);
        let corners = envelope.corners();
        symbol_view::paint_shape(
            &painter,
            view,
            &Shape::polygon(corners.to_vec()),
            None,
            thin(t.text_dim),
        );
        let datum = view.screen(set.footprint.datum.offset);
        for d in [vec2(8.0, 0.0), vec2(0.0, 8.0)] {
            painter.line_segment([datum - d, datum + d], Stroke::new(1.0, t.text_dim));
        }
        // Assembly below the copper; silkscreen and courtyard above it.
        if layers.drawing(DrawingLayer::Assembly) {
            self.paint_drawings(&painter, view, DrawingLayer::Assembly, ink.assembly);
        }
        if layers.mask {
            for pad in &set.footprint.pads {
                if let Some(mask) = pad.mask_opening(&defaults, chord) {
                    symbol_view::paint_shape(&painter, view, &mask, Some(ink.mask), None);
                }
            }
        }
        for pad in &set.footprint.pads {
            let land = pad.land(chord);
            let copper = match pad.side {
                PadSide::Top => colors.copper_top,
                PadSide::Bottom => colors.copper_bottom,
            };
            let selected = selected_pad == Some(pad.id);
            if pad.kind == PadKind::NonPlatedHole {
                // A bare hole has no copper: its land is the keep-out.
                symbol_view::paint_shape(&painter, view, &land, None, thin(t.text_dim));
            } else {
                symbol_view::paint_shape(&painter, view, &land, Some(copper), None);
            }
            if selected {
                symbol_view::paint_shape(
                    &painter,
                    view,
                    &land,
                    None,
                    Some(Stroke::new(2.0, colors.selected)),
                );
            }
        }
        // The template picker's ghost: the pads Build would make.
        if let Some(ghost) = self.template_picker.ghost() {
            for pad in ghost {
                symbol_view::paint_shape(
                    &painter,
                    view,
                    &pad.land(chord),
                    Some(t.accent.gamma_multiply(0.18)),
                    Some(Stroke::new(1.0, t.accent)),
                );
            }
        }
        if layers.paste {
            for pad in &set.footprint.pads {
                for paste in pad.paste_openings(&defaults, chord) {
                    symbol_view::paint_shape(&painter, view, &paste, Some(ink.paste), None);
                }
            }
        }
        for pad in &set.footprint.pads {
            if let Some(hole) = pad.hole_shape() {
                symbol_view::paint_shape(&painter, view, &hole, Some(ink.hole), thin(t.text_dim));
            }
            for via in &pad.vias {
                let at = pad.to_footprint(via.at);
                painter.circle_filled(
                    view.screen(at),
                    (via.drill as f32 * 0.5 * view.scale).max(1.0),
                    ink.hole,
                );
            }
        }
        if layers.silkscreen {
            self.paint_drawings(&painter, view, DrawingLayer::Silkscreen, ink.silk);
        }
        if layers.drawing(DrawingLayer::Courtyard) {
            self.paint_drawings(&painter, view, DrawingLayer::Courtyard, ink.courtyard);
        }
        // The graphic a clicked finding names.
        if let Some(graphic) = self
            .finding_graphic
            .and_then(|id| set.footprint.graphics.iter().find(|g| g.id == id))
        {
            let ring = Stroke::new(2.0, colors.selected);
            match &graphic.item {
                Item::Shape { shape, .. } => {
                    symbol_view::paint_shape(&painter, view, shape, None, Some(ring));
                }
                Item::Text(text) => {
                    let at = view.screen(text.at);
                    let r = (text.height as f32 * view.scale).max(6.0);
                    painter.rect_stroke(
                        Rect::from_center_size(at, vec2(r * 3.0, r * 1.6)),
                        2.0,
                        ring,
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }
        // The sketch: curves, points, dimensions and constraint glyphs.
        let hover = response.hover_pos();
        self.paint_footprint_sketch(&painter, rect, view, hover);
        let Some(set) = &self.draft else {
            return;
        };
        // Pad numbers on top.
        for pad in &set.footprint.pads {
            let at = view.screen(pad.position);
            let [w, h] = pad.shape.extent();
            // Fit the number inside the land: about 0.8 em per capital.
            let chars = pad.number.chars().count().max(1) as f32;
            let room = (w.min(h) as f32 * view.scale * 0.6)
                .min(w as f32 * view.scale * 0.8 / (0.8 * chars))
                .min(t.font_label * 1.4);
            if room >= 6.0 && !pad.number.is_empty() {
                painter.text(
                    at,
                    Align2::CENTER_CENTER,
                    &pad.number,
                    fonts::semibold(room),
                    t.on_accent,
                );
            }
        }
        self.footprint_tools_frame(ui, rect, response, view, grid, sketch_took);
        painter.text(
            rect.left_top() + vec2(t.space_16, t.space_16),
            Align2::LEFT_TOP,
            "Footprint · copper lands",
            fonts::semibold(t.font_panel),
            t.text,
        );
        self.footprint_value_field(ui, view);
        if let Some(p) = response.hover_pos() {
            let at = view.world(p);
            painter.text(
                rect.left_bottom() + vec2(t.space_16, -t.space_16),
                Align2::LEFT_BOTTOM,
                format!(
                    "{} , {} mm   grid {} mm",
                    crate::kit::number_text(at[0]),
                    crate::kit::number_text(at[1]),
                    crate::kit::number_text(grid)
                ),
                fonts::medium(t.font_small),
                t.text_dim,
            );
        }
    }

    /// Scroll zoom about the pointer and middle (or Space) drag panning.
    fn footprint_navigation(&mut self, ui: &Ui, rect: Rect, response: &Response, view: View) {
        let typing = ui.ctx().egui_wants_keyboard_input();
        ui.input(|i| {
            for event in &i.events {
                if let egui::Event::Key {
                    key: egui::Key::Space,
                    pressed,
                    repeat: false,
                    ..
                } = event
                {
                    self.space_held = *pressed && !typing;
                }
            }
        });
        if typing || !ui.input(|i| i.key_down(egui::Key::Space)) {
            self.space_held = false;
        }
        let Some(state) = self.footprint_view else {
            return;
        };
        if let Some(p) = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p))
        {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let scale = (state.scale * (scroll / ZOOM_PX).exp()).clamp(0.5, 2000.0);
                let world = view.world(p);
                let origin = p - vec2(world[0] as f32 * scale, -(world[1] as f32) * scale);
                self.footprint_view = Some(ViewState {
                    pan: origin - rect.center(),
                    scale,
                });
            }
        }
        let panning = response.dragged_by(egui::PointerButton::Middle)
            || (self.space_held && response.dragged_by(egui::PointerButton::Primary));
        if panning && let Some(v) = &mut self.footprint_view {
            v.pan += response.drag_delta();
        }
    }

    /// Paint the footprint graphics on one drawing layer. The designator
    /// placeholder reads the component prefix and `?`.
    fn paint_drawings(
        &self,
        painter: &egui::Painter,
        view: View,
        layer: DrawingLayer,
        color: Color32,
    ) {
        let Some(set) = &self.draft else {
            return;
        };
        for graphic in set.footprint.graphics.iter().filter(|g| g.layer == layer) {
            match &graphic.item {
                Item::Shape { shape, width, fill } => {
                    let stroke = (*width > 0.0)
                        .then(|| Stroke::new((*width as f32 * view.scale).max(1.0), color));
                    let fill = (*fill != Fill::None).then_some(color);
                    match stroke {
                        // Courtyards are dashed (graphics spec §6).
                        Some(stroke) if layer == DrawingLayer::Courtyard && fill.is_none() => {
                            symbol_view::paint_dashed(painter, view, shape, stroke, 6.0, 4.0);
                        }
                        _ => symbol_view::paint_shape(painter, view, shape, fill, stroke),
                    }
                }
                Item::Text(text) if text.visible => {
                    let content = match &text.content {
                        TextContent::Literal(s) => s.clone(),
                        TextContent::Designator => {
                            format!("{}?", set.component.designator_prefix)
                        }
                        TextContent::ComponentName => set.metas.component.name.clone(),
                        _ => String::new(),
                    };
                    let placed = PlacedText {
                        graphic: Some(graphic.id),
                        text: content,
                        at: text.at,
                        height: text.height,
                        rotation_deg: text.rotation.degrees(),
                        justify: text.justify,
                        mirrored: false,
                        line_spacing: text.line_spacing,
                    };
                    symbol_view::paint_text(painter, view, &placed, color);
                }
                _ => {}
            }
        }
    }
}

/// The screen position of a footprint point for a fitted view (tests).
#[cfg(test)]
pub(super) fn screen(editor: &PartEditor, rect: Rect, p: Point) -> Option<egui::Pos2> {
    editor.footprint_view.map(|v| {
        View {
            origin: rect.center() + v.pan,
            scale: v.scale,
        }
        .screen(p)
    })
}
