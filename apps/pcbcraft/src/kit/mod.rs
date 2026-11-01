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
pub use issues::{count_badge, issues_bar, issues_list};
pub use rows::*;
pub use search::*;
pub use toast::pill;
pub use tree::*;
pub use {dialog::DialogPanel, issues::issue_line, severity::severity_glyph};

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

/// The canvas's one dim hint line at its bottom-left corner, `inset` from
/// the edges. It ends before the centred canvas buttons (fit view and view
/// options, at most two), with an ellipsis when cut; the full text stays in
/// the editor's own hover help.
pub fn canvas_hint(painter: &Painter, rect: Rect, inset: f32, text: &str) {
    let t = Tokens::get();
    // Half the widest navigation bar (two buttons) and a gap.
    let buttons = t.control_height + 1.5 * t.space_4 + t.space_8;
    let left = rect.left() + inset;
    let width = (rect.center().x - buttons - left).max(0.0);
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        egui::FontId::proportional(t.font_small),
        t.text_dim,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let galley = painter.layout_job(job);
    let at = egui::pos2(left, rect.bottom() - inset - galley.size().y);
    painter.galley(at, galley, t.text_dim);
}
