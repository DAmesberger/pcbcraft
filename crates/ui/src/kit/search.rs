//! Command-search composition: one expanding ribbon editor and a clipped results sheet.
//! Domain ranking and command dispatch stay with the caller.
use crate::tokens::Tokens;
use egui::{FontId, Rect, Ui, pos2, vec2};

#[derive(Clone, Copy)]
pub enum SearchBadge {
    Kind,
    Valid,
    Invalid,
}

/// Geometry uses the same already-eased progress as the field bounds.
pub struct SearchFieldGeometry {
    pub height: f32,
    pub radius: f32,
    pub progress: f32,
}

struct FieldSpacing {
    left: f32,
    right: f32,
    icon_gap: f32,
    key_gap: f32,
    key_padding: f32,
    editor_margin: f32,
}

impl FieldSpacing {}

pub struct CommandSearch;

impl CommandSearch {
    fn inner_radius() -> u8 {
        let t = Tokens::get();
        t.radius_xl.saturating_sub(t.space_6 as u8)
    }

    pub(super) fn key(ui: &Ui, right: egui::Pos2, label: &str) -> f32 {
        Self::key_with_padding(ui, right, label, Tokens::get().space_10)
    }

    fn key_with_padding(ui: &Ui, right: egui::Pos2, label: &str, padding: f32) -> f32 {
        let t = Tokens::get();
        let galley = ui.painter().layout_no_wrap(label.to_owned(), FontId::monospace(t.font_small), t.form_dim);
        let size = galley.size() + vec2(padding, t.space_4);
        let r = Rect::from_min_size(pos2(right.x - size.x, right.y - size.y / 2.0), size);
        t.container_rect(ui.painter(), r, Self::inner_radius(), t.dialog_footer, t.dialog_rule);
        ui.painter().galley(r.min + vec2(padding / 2.0, t.space_2), galley, t.form_dim);
        size.x
    }
}

impl super::MenuItem<'_> {}
