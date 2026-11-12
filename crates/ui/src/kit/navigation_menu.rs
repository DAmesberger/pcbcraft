//! Popup and tooltip skin from the approved navigation boards.
use super::{Container, navigation::*};

pub fn navigation_popup_frame() -> egui::Frame {
    Container::menu_frame().inner_margin(egui::Margin::same(NAV_POPUP_PADDING))
}
