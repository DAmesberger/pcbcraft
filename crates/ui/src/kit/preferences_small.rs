//! Whether a control is on a settings page, and a row's help (the rest of SolidCraft's
//! preferences_small.rs is not vendored).
use egui::Ui;

const SETTINGS_PAGE: &str = "sc_settings_page";

pub fn settings_page_active(ui: &Ui) -> bool {
    ui.stack().iter().any(|node| node.tags().contains(SETTINGS_PAGE))
}
