//! Sparksmith's dense card kit: form rows, unit fields, dropdowns, sections,
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
//! `item_row` is the one row of every Properties list (name, badge, value,
//! end action).
//!
//! The sidebar redesign (docs/sidebar-ia.md) adds the column card, issue,
//! feedback and dialog kits below; slice S0 fixed their signatures.
pub mod card;
pub mod dialog;
mod form;
pub mod issues;
pub mod issues_list;
mod item_row;
pub mod list;
pub mod messages;
pub mod pick;
pub mod review;
mod rows;
mod search;
pub mod severity;
pub mod text;
pub mod toast;
mod tree;

pub use form::*;
pub use issues::{count_badge, issues_bar};
pub use item_row::*;
pub use pick::{PickResponse, PickRow, Tick, pick_row};
pub use rows::*;
pub use search::*;
pub use toast::{pill, status_chip};
pub use tree::*;
pub use {dialog::DialogPanel, issues::issue_line, severity::severity_glyph};

use egui::{Color32, Painter, Pos2, Rect, vec2};
use sparksmith_ui::icons;
use sparksmith_ui::theme::Tokens;

/// A count and its noun: "1 rule", "0 rules", "3 entries". `one` is the
/// singular, `many` the plural, so irregular plurals read right.
pub fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

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

/// The WCAG contrast ratio of two opaque colours, for tests against the
/// theme's tokens.
#[cfg(test)]
pub fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
    let channel = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = |c: egui::Color32| {
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The height of the canvas's floating navigation bar (fit view and view
/// options) and of the gap under it: the band at the canvas's bottom that
/// a fitted view keeps clear.
pub fn navigation_band() -> f32 {
    let t = Tokens::get();
    navigation_bar_height() + t.space_16
}

/// The navigation bar's own height: a button and its padding.
pub fn navigation_bar_height() -> f32 {
    let t = Tokens::get();
    t.control_height + 2.0 * t.space_4
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
    // On the panel colour, as the canvas's hover tags are, so the line
    // stays readable over copper or anything else drawn under it.
    let back =
        egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(t.space_4, t.space_2));
    painter.rect_filled(back, t.radius_sm, t.panel);
    painter.galley(at, galley, t.text_dim);
}
