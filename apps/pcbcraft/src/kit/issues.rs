//! The Issues surface widgets: rows, the grouped list, the card's one-line
//! issue slot, the always-visible bar, the canvas count badge and the
//! disclosed details.
//!
//! Contract from slice S0 (docs/sidebar-ia.md); slice K2 owns the final
//! look, A1 the content. Rows are buttons (a click locates, never selects
//! text); the list is grouped, uncapped and virtualised. Adapted from
//! Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/status.rs`
//! (`check_status_line`), `reports.rs` (`report_list`), `tree.rs`
//! (`tree_header`) and `checks.rs`, MIT option; see ATTRIBUTION.md.
#![allow(dead_code, reason = "S0 contract; slices K2 and A1 use it")]

use super::severity::{Level, severity_glyph};
use egui::{Response, RichText, Sense, Ui, vec2};
use pcbcraft_core::issues::{IssueCounts, IssueDetail};
use solidcraft_ui_egui::theme::Tokens;

/// The Issues bar's height while collapsed, in points.
pub const ISSUES_BAR_HEIGHT: f32 = 28.0;

/// One issue row as shown: glyph, one-line title, a ×n pill when `count`
/// is above one, locate, an optional Fix and a disclosure.
#[derive(Clone, Debug, PartialEq)]
pub struct IssueRowView {
    pub level: Level,
    /// At most 80 characters (core's `MAX_TITLE`); clipped to the row with
    /// the full text on hover.
    pub title: String,
    pub count: usize,
    /// The Fix button's label, only when a typed command or a dialog that
    /// issues one resolves the issue.
    pub fix: Option<String>,
    /// Whether the details are disclosed.
    pub open: bool,
}

/// What a row's click did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IssueRowResponse {
    /// The row itself: select and frame every referenced object.
    pub locate: bool,
    pub fix: bool,
    /// The disclosure.
    pub toggle: bool,
}

/// One issue row. `id` keeps its interaction stable while the list scrolls.
pub fn issue_row(ui: &mut Ui, id: egui::Id, row: &IssueRowView) -> IssueRowResponse {
    let t = Tokens::get();
    let mut out = IssueRowResponse::default();
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            let (glyph, toggle) =
                ui.allocate_exact_size(vec2(t.control_height, t.control_height), Sense::click());
            severity_glyph(ui.painter(), glyph.center(), t.font_label, row.level);
            out.toggle = toggle.clicked();
            let mut text = row.title.clone();
            if row.count > 1 {
                text = format!("{text} ×{}", row.count);
            }
            let title = ui
                .add(
                    egui::Button::new(RichText::new(text).size(t.font_label).color(t.text))
                        .frame_when_inactive(false)
                        .truncate(),
                )
                .on_hover_text(&row.title);
            out.locate = title.clicked();
            if let Some(fix) = &row.fix {
                out.fix = super::primary(ui, fix, true).clicked();
            }
        });
    });
    out
}

/// One group of the list: its header and how many rows it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct IssueGroupView {
    pub level: Level,
    pub title: String,
    pub rows: usize,
    pub open: bool,
    /// A group-wide Fix, such as "Update PCB…" for "Not placed".
    pub fix: Option<String>,
}

/// What happened in the list this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueListEvent {
    /// A group header's disclosure.
    Group(usize),
    /// A group's Fix.
    GroupFix(usize),
    Row {
        group: usize,
        row: usize,
        action: IssueRowResponse,
    },
}

/// The grouped list: every group and every row, never capped. `row(group,
/// index)` gives a row's view only when it is drawn.
pub fn issues_list(
    ui: &mut Ui,
    id: egui::Id,
    groups: &[IssueGroupView],
    mut row: impl FnMut(usize, usize) -> IssueRowView,
) -> Option<IssueListEvent> {
    let mut event = None;
    for (g, group) in groups.iter().enumerate() {
        let header = IssueRowView {
            level: group.level,
            title: group.title.clone(),
            count: group.rows,
            fix: group.fix.clone(),
            open: group.open,
        };
        let action = issue_row(ui, id.with(("group", g)), &header);
        if action.toggle || action.locate {
            event = Some(IssueListEvent::Group(g));
        } else if action.fix {
            event = Some(IssueListEvent::GroupFix(g));
        }
        if !group.open {
            continue;
        }
        for r in 0..group.rows {
            let action = issue_row(ui, id.with(("row", g, r)), &row(g, r));
            if action != IssueRowResponse::default() {
                event = Some(IssueListEvent::Row {
                    group: g,
                    row: r,
                    action,
                });
            }
        }
    }
    event
}

fn counts_text(counts: &IssueCounts, to_do: bool) -> String {
    let mut parts = Vec::new();
    for (level, n) in [
        (Level::Error, counts.error),
        (Level::Warning, counts.warning),
        (Level::ToDo, if to_do { counts.to_do } else { 0 }),
    ] {
        if n > 0 {
            parts.push(format!("{} {n}", glyph_name(level)));
        }
    }
    parts.join("  ")
}

/// The accessible name of a glyph, for text that cannot draw it.
fn glyph_name(level: Level) -> &'static str {
    match level {
        Level::Error => "errors",
        Level::Warning => "warnings",
        Level::ToDo => "to do",
        Level::Info => "info",
        Level::Ok => "ok",
    }
}

/// The card's reserved one-line object-issue slot ("× 1 · ! 1 on R1 ›").
/// It always takes its row, so the card does not reflow when issues appear.
/// A click opens Issues scoped to this object.
pub fn issue_line(ui: &mut Ui, counts: &IssueCounts, label: &str) -> Response {
    let t = Tokens::get();
    let text = counts_text(counts, true);
    let text = if text.is_empty() {
        String::new()
    } else {
        format!("{text} {label}")
    };
    ui.add_sized(
        vec2(ui.available_width(), t.control_height),
        egui::Button::new(RichText::new(text).size(t.font_label).color(t.text_dim))
            .frame_when_inactive(false)
            .truncate(),
    )
}

/// The always-visible Issues bar: title, counts and, when another area
/// holds blockers, its `elsewhere` line. A click expands or collapses the
/// card.
pub fn issues_bar(
    ui: &mut Ui,
    counts: &IssueCounts,
    expanded: bool,
    elsewhere: Option<&str>,
) -> Response {
    let t = Tokens::get();
    let mut text = format!("Issues  {}", counts_text(counts, true));
    if let Some(elsewhere) = elsewhere {
        text = format!("{text}  ● {elsewhere}");
    }
    ui.add_sized(
        vec2(ui.available_width(), ISSUES_BAR_HEIGHT),
        egui::Button::new(RichText::new(text).size(t.font_label).color(t.text))
            .frame_when_inactive(false)
            .selected(expanded)
            .truncate(),
    )
}

/// The counts as a compact badge (ribbon toggle, canvas). To-do is hidden
/// while errors exist when `hide_to_do_with_errors`; the bar never hides it.
pub fn count_badge(ui: &mut Ui, counts: &IssueCounts, hide_to_do_with_errors: bool) -> Response {
    let t = Tokens::get();
    let to_do = !(hide_to_do_with_errors && counts.error > 0);
    ui.add(
        egui::Button::new(
            RichText::new(counts_text(counts, to_do))
                .size(t.font_small)
                .color(t.text),
        )
        .small(),
    )
}

/// A row's disclosed details: cause and fix, one line each.
pub fn detail(ui: &mut Ui, detail: &IssueDetail) {
    for line in [&detail.cause, &detail.fix].into_iter().flatten() {
        super::hint(ui, line, line);
    }
}
