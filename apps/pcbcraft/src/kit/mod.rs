//! Pcbcraft's dense card kit: form rows, unit fields, dropdowns, sections,
//! status lines, Browser tree rows and the search sheet.
//!
//! Adapted from Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/layout.rs`
//! (rows, sections, status lines), `fields.rs` (dropdowns, cell editors),
//! `disclosure.rs`, `panel.rs` (headers), `tree.rs` (Browser rows) and
//! `search.rs` (command-search sheet, field and rows), MIT option; see
//! ATTRIBUTION.md. Upstream keeps these crate-private, so the geometry is
//! reproduced here on top of the public tokens, fonts and icons. Feature
//! panels supply content and commands only; chrome and control looks come
//! from this module.
//!
//! The sidebar redesign (docs/sidebar-ia.md) adds the column card, issue,
//! feedback and dialog kits below; slice S0 fixed their signatures.
pub mod card;
pub mod dialog;
mod form;
pub mod issues;
pub mod messages;
pub mod review;
mod rows;
mod search;
pub mod severity;
pub mod toast;
mod tree;

pub use form::*;
pub use search::*;
pub use tree::*;
#[allow(unused_imports, reason = "S0 contract; slices K1–K4 and A1 use it")]
pub use {
    dialog::{DialogPanel, confirm},
    issues::{count_badge, detail, issue_line, issue_row, issues_bar, issues_list},
    review::ReviewPanel,
    rows::*,
    severity::severity_glyph,
    toast::pill,
};

use egui::{Color32, Painter, Pos2, Rect, vec2};
use solidcraft_ui_egui::{icons, theme::Tokens};

/// The kit chevron: down when open, right when folded.
pub fn disclosure(painter: &Painter, center: Pos2, open: bool, ink: Color32) {
    let size = Tokens::get().form_section_icon_size;
    let rect = Rect::from_center_size(center, vec2(size, size));
    chevron(painter, rect, open, ink);
}

/// Two strokes in `ink`, so the chevron follows the caller's semantic colour
/// (the upstream single-colour painter is crate-private).
pub fn chevron(painter: &Painter, rect: Rect, down: bool, ink: Color32) {
    let c = rect.center();
    let h = rect.width().min(rect.height()) * 0.22;
    let stroke = egui::Stroke::new(1.4, ink);
    let points = if down {
        [
            c + vec2(-h * 1.6, -h * 0.8),
            c + vec2(0.0, h * 0.8),
            c + vec2(h * 1.6, -h * 0.8),
        ]
    } else {
        [
            c + vec2(-h * 0.8, -h * 1.6),
            c + vec2(h * 0.8, 0.0),
            c + vec2(-h * 0.8, h * 1.6),
        ]
    };
    painter.line_segment([points[0], points[1]], stroke);
    painter.line_segment([points[1], points[2]], stroke);
}

/// A full-colour icon in a square of `size` centred at `center`.
pub fn icon(painter: &Painter, center: Pos2, size: f32, name: &str, opacity: f32) {
    icons::paint_icon(
        painter,
        Rect::from_center_size(center, vec2(size, size)),
        name,
        opacity,
    );
}
