//! The package mini-view: the chosen footprint's pads drawn flat in 2D,
//! tinted by the peripheral each pad's terminal takes in the preview. No
//! 3D and no geometry rebuild; selection here is the panel's focus pin
//! only. Pads are seen from the top, +y up, scaled to fit.
use super::model::{Model, PadView, Preview};
use egui::{Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use sparksmith_core::ids::TerminalId;
use sparksmith_ui::theme::Tokens;

/// The categorical tints, from the shared workspace accents, by
/// peripheral declaration order.
pub fn tint(index: usize) -> Color32 {
    let t = Tokens::get();
    let palette = [
        t.sketch_accent,
        t.surface_accent,
        t.mesh_accent,
        t.form_accent,
        t.render_accent,
        t.axis_x,
        t.axis_y,
        t.axis_z,
    ];
    palette[index % palette.len()]
}

/// The footprint's pad bounds in mm, or `None` without pads.
pub fn bounds(pads: &[PadView]) -> Option<([f64; 2], [f64; 2])> {
    let mut iter = pads
        .iter()
        .filter(|p| p.center.iter().chain(p.size.iter()).all(|v| v.is_finite()));
    let first = iter.next()?;
    let edge = |p: &PadView, s: f64| {
        [
            p.center[0] + s * p.size[0] / 2.0,
            p.center[1] + s * p.size[1] / 2.0,
        ]
    };
    let (mut min, mut max) = (edge(first, -1.0), edge(first, 1.0));
    for pad in iter {
        let (lo, hi) = (edge(pad, -1.0), edge(pad, 1.0));
        min = [min[0].min(lo[0]), min[1].min(lo[1])];
        max = [max[0].max(hi[0]), max[1].max(hi[1])];
    }
    Some((min, max))
}

/// The screen rectangle of `pad` inside `area` for footprint `bounds`.
pub fn pad_rect(area: Rect, bounds: ([f64; 2], [f64; 2]), pad: &PadView) -> Rect {
    let (min, max) = bounds;
    let span = [(max[0] - min[0]).max(1e-6), (max[1] - min[1]).max(1e-6)];
    let scale = (f64::from(area.width()) / span[0]).min(f64::from(area.height()) / span[1]);
    let mid = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
    let at = |x: f64, y: f64| {
        pos2(
            area.center().x + ((x - mid[0]) * scale) as f32,
            area.center().y - ((y - mid[1]) * scale) as f32,
        )
    };
    let centre = at(pad.center[0], pad.center[1]);
    let size = vec2(
        ((pad.size[0] * scale) as f32).max(2.0),
        ((pad.size[1] * scale) as f32).max(2.0),
    );
    Rect::from_center_size(centre, size)
}

/// Draw the pads in a square of the available width (at most `max`) and
/// return the terminal of a clicked pad.
pub fn show(
    ui: &mut Ui,
    model: &Model,
    preview: &Preview,
    focus: Option<TerminalId>,
    max: f32,
) -> Option<TerminalId> {
    let t = Tokens::get();
    let side = ui.available_width().clamp(0.0, max.max(0.0));
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, t.control_radius, t.field);
    painter.rect_stroke(
        rect,
        t.control_radius,
        t.container_border(t.dialog_rule),
        egui::StrokeKind::Inside,
    );
    let extent = bounds(&model.pads)?;
    let area = rect.shrink(t.space_12);
    let mut clicked = None;
    let mut hovered = None;
    let pointer = response.hover_pos();
    for pad in &model.pads {
        let shape = pad_rect(area, extent, pad);
        let role = pad.terminal.and_then(|term| preview.role(model, term));
        let fill = match role.and_then(|r| model.tint_index(r.resource)) {
            Some(index) => tint(index),
            None => t.form_dim.gamma_multiply(0.45),
        };
        let radius = if pad.round {
            shape.width().min(shape.height()) / 2.0
        } else {
            1.0
        };
        painter.rect_filled(shape, radius, fill);
        let focused = focus.is_some() && pad.terminal == focus;
        let changed = pad
            .terminal
            .is_some_and(|term| preview.changed.contains(&term));
        if focused || changed {
            let stroke = if focused {
                Stroke::new(2.0, t.form_ink)
            } else {
                Stroke::new(1.0, t.form_primary)
            };
            painter.rect_stroke(
                shape.expand(1.5),
                radius + 1.5,
                stroke,
                egui::StrokeKind::Outside,
            );
        }
        if pointer.is_some_and(|p| shape.expand(1.0).contains(p)) {
            hovered = Some(pad);
        }
    }
    if let Some(pad) = hovered {
        let name = pad
            .terminal
            .and_then(|term| model.pin(term))
            .map_or("no terminal", |p| p.name.as_str());
        let function = pad
            .terminal
            .and_then(|term| preview.resolution.active(term))
            .map_or_else(String::new, |a| format!(" · {}", a.label));
        if response.clicked() {
            clicked = pad.terminal;
        }
        response.on_hover_text(format!("Pad {} · {name}{function}", pad.number));
    }
    clicked
}
