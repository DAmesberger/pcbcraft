//! Read-only report and library containers, sharing the form's typography and actions.
use crate::tokens::Tokens;
use egui::Ui;

/// List border around a caller-owned virtualised viewport. Tokens: dialog_rule, control_radius, space_10.
pub fn list_frame<R>(ui: &mut Ui, key: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    list_frame_filled(ui, key, egui::Color32::TRANSPARENT, 0.0, add)
}

pub(super) fn list_frame_filled<R>(ui: &mut Ui, key: &str, fill: egui::Color32, vertical: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get();
    let response = crate::kit::Container::show_backing(
        ui,
        egui::Frame::NONE
            .fill(fill)
            .stroke(t.container_border(t.dialog_rule))
            .corner_radius(t.control_radius)
            .inner_margin(egui::Margin::symmetric(t.space_10 as i8, vertical as i8)),
        |ui| {
            // Frame already subtracts its border and padding from the content width.
            ui.set_min_width(ui.available_width().max(0.0));
            add(ui)
        },
    );
    solidcraft_ui_registry::publish_rect(&format!("dialog:list:{key}"), response.response.rect);
    response.inner
}
