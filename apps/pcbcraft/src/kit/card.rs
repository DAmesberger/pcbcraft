//! The right column: one subject card (Properties, tool card, dialog or
//! review) stacked above the always-visible Issues card.
//!
//! Contract from slice S0 (docs/sidebar-ia.md, "Right column"); slice K1
//! owns the final layout and wires it into `app/chrome.rs`. Adapted from
//! Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/panel.rs`
//! (`workspace_with_header`, `editor_footer`, `stack_first_bounds`,
//! `stack_next_bounds`), MIT option; see ATTRIBUTION.md.
#![allow(dead_code, reason = "S0 contract; slice K1 and the W slices use it")]

use super::issues::{ISSUES_BAR_HEIGHT, issue_line};
use egui::{Rect, Ui, pos2};
use pcbcraft_core::issues::IssueCounts;
use solidcraft_ui_egui::theme::Tokens;

/// The pinned footer's height, in points.
pub const FOOTER_HEIGHT: f32 = 48.0;

/// Body rows the subject keeps while Issues is expanded (upstream
/// `stack_first_bounds`: header plus ten rows, or half the column).
pub const SUBJECT_ROWS: f32 = 10.0;

/// What the subject slot holds. Exactly one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Subject {
    Properties,
    Tool,
    Dialog,
    Review,
}

/// The card's one-line object-issue slot: the counts of issues on the
/// inspected object and the words that follow them ("on R13").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueSlot {
    pub counts: IssueCounts,
    pub label: String,
}

/// What each editor returns for its Properties card. The card draws the
/// header, then asks the editor's content for its [`Region`]s.
pub struct Inspector {
    pub header: crate::shell::Header,
    /// Identifies what is inspected; the body's scroll offset resets when it
    /// changes and is kept while it stays.
    pub subject: egui::Id,
    /// Whether the card has a pinned footer (object actions such as
    /// Remove pin); never Apply.
    pub footer: bool,
    /// The reserved issue slot; always drawn, empty when `None`.
    pub issues: Option<IssueSlot>,
}

/// The part of the card an editor's content is asked to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// The scrolling body, the card's only scroll.
    Body,
    /// The pinned footer.
    Footer,
}

/// What the card returned this frame.
pub struct CardResponse<R> {
    /// The first output any region produced (a typed operation).
    pub output: Option<R>,
    /// The issue slot was clicked: show Issues scoped to this object.
    pub show_issues: bool,
}

/// The right column's two rectangles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    pub subject: Rect,
    pub issues: Rect,
}

/// Split the column: collapsed, Issues is a bar at the bottom; expanded,
/// the subject keeps its header and [`SUBJECT_ROWS`] rows, or half the
/// column if that is more, and Issues takes the rest.
pub fn column(rect: Rect, issues_expanded: bool) -> Column {
    let t = Tokens::get();
    let bar = ISSUES_BAR_HEIGHT.min(rect.height());
    let subject_height = if issues_expanded {
        let rows = t.form_header_height + SUBJECT_ROWS * t.control_height;
        rows.max(rect.height() / 2.0).min(rect.height() - bar)
    } else {
        rect.height() - bar
    };
    let split = rect.top() + subject_height.max(0.0);
    Column {
        subject: Rect::from_min_max(rect.min, pos2(rect.right(), split)),
        issues: Rect::from_min_max(pos2(rect.left(), split), rect.max),
    }
}

/// Draw a Properties card in `ui`: header, the scrolling body, the reserved
/// issue slot and the pinned footer. `content` is asked once per region.
pub fn card<R>(
    ui: &mut Ui,
    inspector: &Inspector,
    mut content: impl FnMut(&mut Ui, Region) -> Option<R>,
) -> CardResponse<R> {
    let t = Tokens::get();
    let header = &inspector.header;
    super::inspector_header(ui, header.icon, &header.title, header.kind);
    let reserved = t.control_height + if inspector.footer { FOOTER_HEIGHT } else { 0.0 };
    let mut output = egui::ScrollArea::vertical()
        .id_salt(inspector.subject)
        .max_height((ui.available_height() - reserved).max(0.0))
        .auto_shrink([false, false])
        .show(ui, |ui| super::form(ui, |ui| content(ui, Region::Body)))
        .inner;
    let slot = inspector.issues.clone().unwrap_or_default();
    let show_issues = issue_line(ui, &slot.counts, &slot.label).clicked();
    if inspector.footer {
        let mut footer = None;
        super::actions(ui, |ui| footer = content(ui, Region::Footer));
        output = output.or(footer);
    }
    CardResponse {
        output,
        show_issues,
    }
}
