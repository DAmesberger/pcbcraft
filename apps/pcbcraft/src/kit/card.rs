//! The right column: one subject card (Properties, tool card, dialog or
//! review) stacked above the always-visible Issues card.
//!
//! Slice K1 of docs/sidebar-ia.md ("Layout"). [`column`] splits the column
//! and [`card`] draws a Properties card that fills its rectangle: header,
//! the one scrolling body, the reserved issue slot and the pinned footer.
//! Adapted from Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/panel.rs`
//! (`workspace_with_header`, `editor_footer`, `stack_first_bounds`,
//! `stack_next_bounds`), MIT option; see ATTRIBUTION.md. Upstream keeps them
//! crate-private, so the geometry is reproduced on the public tokens.

use super::issues::{ISSUES_BAR_HEIGHT, issue_line};
use egui::{Rect, Ui, pos2, vec2};
use pcbcraft_core::issues::IssueCounts;
use pcbcraft_ui::theme::Tokens;

/// The pinned footer's height, in points.
pub const FOOTER_HEIGHT: f32 = 48.0;

/// Body rows the subject keeps while Issues is expanded (upstream
/// `stack_first_bounds`: header plus ten rows).
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
    /// The slot counts the whole workspace (the Board or sheet card), not
    /// the selected object: clicking it lists the workspace's issues.
    pub whole: bool,
}

impl IssueSlot {
    /// Whether the slot names any issue (an empty slot keeps its row but
    /// shows and does nothing).
    pub fn is_empty(&self) -> bool {
        let c = &self.counts;
        c.error + c.warning + c.to_do + c.info == 0
    }
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
    /// The clicked slot counted the whole workspace ([`IssueSlot::whole`]).
    pub whole: bool,
}

impl<R> CardResponse<R> {
    /// The response of a subject without an issue slot (a dialog, a tool
    /// card or a docked review).
    pub fn output(output: Option<R>) -> Self {
        Self {
            output,
            show_issues: false,
            whole: false,
        }
    }
}

/// The right column's two rectangles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    pub subject: Rect,
    pub issues: Rect,
}

/// The gap between the stacked cards (upstream `stack_next_bounds`).
fn gap() -> f32 {
    Tokens::get().space_8
}

/// The heights the two cards' content asked for last frame, in points;
/// `None` while unmeasured (taken as "as much as there is").
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wants {
    pub subject: Option<f32>,
    pub issues: Option<f32>,
    /// A docked dialog's height (header, body up to its `body_rows`,
    /// footer); `None` when no dialog drew last frame.
    pub dialog: Option<f32>,
}

/// The subject card's least height while Issues is expanded: its header
/// and [`SUBJECT_ROWS`] rows (upstream `stack_first_bounds`).
pub fn subject_rows_height() -> f32 {
    let t = Tokens::get();
    t.form_header_height + SUBJECT_ROWS * t.control_height + 2.0 * t.space_8
}

/// Split the column: collapsed, Issues is a [`ISSUES_BAR_HEIGHT`] bar at the
/// bottom and the subject has the rest. Expanded, Issues takes only the
/// height its content uses and the subject the rest, but the subject
/// keeps its header and [`SUBJECT_ROWS`] rows (or its whole content, when
/// that is shorter), so a short card gives Issues the room. A docked dialog
/// keeps the height it asks for, so its form shows without scrolling;
/// Issues gets the rest. The Issues bar
/// always keeps its full height, however short the column; the subject
/// gives way first.
pub fn column(rect: Rect, issues_expanded: bool, wants: Wants) -> Column {
    let height = rect.height().max(0.0);
    let bar = ISSUES_BAR_HEIGHT.min(height);
    // Room above the bar for the subject; never negative.
    let room = (height - bar - gap()).max(0.0);
    let subject_height = if issues_expanded {
        let usable = |h: Option<f32>| h.filter(|h| h.is_finite() && *h >= 0.0);
        let issues = usable(wants.issues).unwrap_or(f32::INFINITY).max(bar);
        let keep = match usable(wants.dialog) {
            Some(dialog) => dialog,
            None => usable(wants.subject)
                .unwrap_or(f32::INFINITY)
                .min(subject_rows_height()),
        }
        .min(room);
        (height - gap() - issues).clamp(keep, room)
    } else {
        room
    };
    let split = rect.top() + subject_height;
    let issues_top = (split + gap()).min(rect.bottom() - bar);
    Column {
        subject: Rect::from_min_max(rect.min, pos2(rect.right(), split)),
        issues: Rect::from_min_max(pos2(rect.left(), issues_top), rect.max),
    }
}

const SUBJECT_WANTS: &str = "column-subject-wants";
const ISSUES_WANTS: &str = "column-issues-wants";
const DIALOG_WANTS: &str = "column-dialog-wants";

/// Keep the height a card's content wants for the next frame's
/// [`column`], repainting when it changed.
fn remember_wants(ctx: &egui::Context, key: &str, height: f32) {
    let id = egui::Id::unique(key);
    let pass = ctx.cumulative_pass_nr();
    let previous = ctx.data(|d| d.get_temp::<(u64, f32)>(id));
    if previous.is_none_or(|(_, h)| (h - height).abs() > 0.5) {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(id, (pass, height)));
}

/// The Issues card's content height this frame (bar, toolbar, list and
/// footer), for the next frame's [`column`].
pub fn remember_issues_wants(ctx: &egui::Context, height: f32) {
    remember_wants(ctx, ISSUES_WANTS, height);
}

/// A docked dialog's height this frame, for the next frame's [`column`].
pub fn remember_dialog_wants(ctx: &egui::Context, height: f32) {
    remember_wants(ctx, DIALOG_WANTS, height);
}

/// What the cards asked for in the last frame that drew them, plus each
/// card's frame margin (`margin`, vertical, in points); a card that was not
/// drawn last frame (a dialog took the slot) asks for nothing.
pub fn wants(ctx: &egui::Context, margin: f32) -> Wants {
    let pass = ctx.cumulative_pass_nr();
    let read = |key: &str| {
        ctx.data(|d| d.get_temp::<(u64, f32)>(egui::Id::unique(key)))
            .filter(|(at, _)| at.saturating_add(1) >= pass)
            .map(|(_, h)| h + margin)
    };
    Wants {
        subject: read(SUBJECT_WANTS),
        issues: read(ISSUES_WANTS),
        dialog: read(DIALOG_WANTS),
    }
}

/// The body's scroll offset to force this frame: zero when the subject
/// changed since the last frame `key` was drawn, so a new subject starts at
/// the top while one subject keeps its offset while it is edited.
fn reset_scroll(ui: &Ui, key: egui::Id, subject: egui::Id) -> bool {
    let id = key.with("subject");
    let previous = ui.data(|d| d.get_temp::<egui::Id>(id));
    ui.data_mut(|d| d.insert_temp(id, subject));
    previous.is_some_and(|p| p != subject)
}

/// Draw a Properties card filling `ui`'s height: header, the scrolling body
/// (the only scroll), the reserved issue slot and the pinned footer.
/// `content` is asked once per region; the footer is asked only when the
/// inspector has one.
pub fn card<R>(
    ui: &mut Ui,
    inspector: &Inspector,
    mut content: impl FnMut(&mut Ui, Region) -> Option<R>,
) -> CardResponse<R> {
    let t = Tokens::get();
    let top = ui.max_rect().top();
    let header = &inspector.header;
    super::inspector_header(ui, header.icon, &header.title, header.kind);
    let slot_height = t.control_height + ui.spacing().item_spacing.y;
    let footer_height = if inspector.footer { FOOTER_HEIGHT } else { 0.0 };
    let body_height = (ui.available_height() - slot_height - footer_height).max(0.0);
    let key = ui.scope_id().with("card-body");
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(key)
        .max_height(body_height)
        .min_scrolled_height(body_height)
        .auto_shrink([false, false]);
    if reset_scroll(ui, key, inspector.subject) {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    let body_top = ui.cursor().top();
    let shown = scroll.show(ui, |ui| {
        ui.add_space(t.space_4);
        let inner = egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(t.space_8 as i8, 0))
            .show(ui, |ui| super::form(ui, |ui| content(ui, Region::Body)))
            .inner;
        ui.add_space(t.space_8);
        inner
    });
    remember_wants(
        ui.ctx(),
        SUBJECT_WANTS,
        body_top - top + shown.content_size.y + slot_height + footer_height,
    );
    let mut output = shown.inner;
    let slot = inspector.issues.clone().unwrap_or_default();
    let line = ui
        .scope(|ui| {
            ui.set_height(t.control_height);
            issue_line(ui, &slot.counts, &slot.label)
        })
        .inner;
    let show_issues = line.clicked() && !slot.is_empty();
    if inspector.footer {
        let (band, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), FOOTER_HEIGHT),
            egui::Sense::hover(),
        );
        t.secondary_hline(ui.painter(), band.x_range(), band.top(), t.dialog_rule);
        let row = Rect::from_center_size(
            band.center(),
            vec2(band.width() - 2.0 * t.space_12, t.control_height),
        );
        let mut footer = None;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        super::form(&mut child, |ui| footer = content(ui, Region::Footer));
        output = output.or(footer);
    }
    CardResponse {
        output,
        show_issues,
        whole: slot.whole,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Header;

    fn column_rect(height: f32) -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(320.0, height))
    }

    #[test]
    fn collapsed_issues_is_a_bar_under_the_subject() {
        let rect = column_rect(1000.0);
        let c = column(rect, false, Wants::default());
        assert_eq!(c.issues.height(), ISSUES_BAR_HEIGHT);
        assert_eq!(c.issues.bottom(), rect.bottom());
        assert_eq!(c.subject.top(), rect.top());
        assert!((c.issues.top() - c.subject.bottom() - gap()).abs() < 1e-3);
        assert_eq!(c.subject.width(), rect.width());
        assert_eq!(c.issues.width(), rect.width());
    }

    #[test]
    fn expanded_issues_shares_the_column_and_the_subject_keeps_its_rows() {
        let rows = subject_rows_height();
        // Unmeasured: the subject keeps its rows and Issues takes the rest.
        let tall = column(column_rect(1081.0), true, Wants::default());
        assert!((tall.subject.height() - rows).abs() < 1e-3);
        assert!(tall.issues.height() > 500.0);
        // A short column: the subject keeps its header and ten rows.
        let short = column(column_rect(rows + 200.0), true, Wants::default());
        assert!((short.subject.height() - rows).abs() < 1e-3);
        assert!(short.issues.height() >= ISSUES_BAR_HEIGHT);
    }

    /// Issues gets only the height its list uses and the subject the rest;
    /// a tall list leaves the subject its rows; a short subject gives way.
    #[test]
    fn expanded_issues_takes_only_what_its_content_uses() {
        let rows = subject_rows_height();
        let rect = column_rect(780.0);
        // A three-row list beside a tall instance card (sch-09).
        let c = column(
            rect,
            true,
            Wants {
                subject: Some(900.0),
                issues: Some(200.0),
                dialog: None,
            },
        );
        assert!((c.issues.height() - 200.0).abs() < 1e-3, "{c:?}");
        assert!((c.subject.height() - (780.0 - 200.0 - gap())).abs() < 1e-3);
        // A long list: the subject keeps its rows; Issues scrolls the rest.
        let c = column(
            rect,
            true,
            Wants {
                subject: Some(900.0),
                issues: Some(5000.0),
                dialog: None,
            },
        );
        assert!((c.subject.height() - rows).abs() < 1e-3);
        // A short board card shrinks to its content to give Issues room.
        let c = column(
            rect,
            true,
            Wants {
                subject: Some(150.0),
                issues: Some(5000.0),
                dialog: None,
            },
        );
        assert!((c.subject.height() - 150.0).abs() < 1e-3);
        assert_eq!(c.issues.bottom(), rect.bottom());
        // An Issues card shorter than its bar still keeps the bar.
        let c = column(
            rect,
            true,
            Wants {
                subject: None,
                issues: Some(0.0),
                dialog: None,
            },
        );
        assert!(c.issues.height() >= ISSUES_BAR_HEIGHT);
        // A docked dialog keeps the height it asks for beside a long list.
        let c = column(
            rect,
            true,
            Wants {
                subject: None,
                issues: Some(5000.0),
                dialog: Some(600.0),
            },
        );
        assert!((c.subject.height() - 600.0).abs() < 1e-3, "{c:?}");
        assert!(c.issues.height() >= ISSUES_BAR_HEIGHT);
    }

    /// However short the window, the bar never shrinks or leaves the
    /// column, so blockers stay counted on screen.
    #[test]
    fn the_issues_bar_survives_any_column_height() {
        for height in [0.0, 10.0, ISSUES_BAR_HEIGHT, 40.0, 120.0, 400.0] {
            for expanded in [false, true] {
                let rect = column_rect(height);
                let c = column(rect, expanded, Wants::default());
                assert!(c.subject.height() >= 0.0, "{height} {expanded}");
                assert!(c.issues.top() >= rect.top(), "{height} {expanded}");
                assert_eq!(c.issues.bottom(), rect.bottom());
                assert!(c.issues.height() >= ISSUES_BAR_HEIGHT.min(height));
                assert!(c.subject.bottom() <= c.issues.top(), "{height} {expanded}");
            }
        }
    }

    fn run(ctx: &egui::Context, add: impl FnMut(&mut Ui)) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 600.0))),
            ..Default::default()
        };
        ctx.run_ui(input, add).textures_delta.clear();
    }

    fn inspector(subject: &str, footer: bool, issues: Option<IssueSlot>) -> Inspector {
        Inspector {
            header: Header::new("component", "R1", "Placement"),
            subject: egui::Id::unique(subject),
            footer,
            issues,
        }
    }

    /// The body fills the card above the slot and footer, so the issue slot
    /// and footer sit at the bottom whatever the body holds; the footer is
    /// asked only when there is one.
    #[test]
    fn the_card_fills_its_height_with_a_pinned_slot_and_footer() {
        let ctx = egui::Context::default();
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        let mut regions = Vec::new();
        let mut bottom = 0.0;
        let mut footer_rect = Rect::NOTHING;
        run(&ctx, |ui| {
            let card = card(ui, &inspector("a", true, None), |ui, region| {
                regions.push(region);
                if region == Region::Footer {
                    footer_rect = ui.max_rect();
                    return Some(1);
                }
                None
            });
            assert_eq!(card.output, Some(1));
            assert!(!card.show_issues);
            bottom = ui.min_rect().bottom();
        });
        assert_eq!(regions, [Region::Body, Region::Footer]);
        assert!(bottom >= 600.0 - 20.0, "card ends at {bottom}");
        assert!(
            footer_rect.bottom() >= 600.0 - FOOTER_HEIGHT,
            "{footer_rect:?}"
        );
        regions.clear();
        run(&ctx, |ui| {
            let _ = card(ui, &inspector("a", false, None), |ui, region| {
                regions.push(region);
                ui.label("x");
                None::<()>
            });
        });
        assert_eq!(regions, [Region::Body]);
    }

    /// A new subject starts at the top; the same subject keeps its offset.
    #[test]
    fn the_scroll_offset_resets_only_when_the_subject_changes() {
        let ctx = egui::Context::default();
        let key = egui::Id::unique("k");
        let mut seen = Vec::new();
        for subject in ["a", "a", "b", "b", "a"] {
            run(&ctx, |ui| {
                seen.push(reset_scroll(ui, key, egui::Id::unique(subject)))
            });
        }
        assert_eq!(seen, [false, false, true, false, true]);
    }

    #[test]
    fn an_empty_issue_slot_does_not_open_issues() {
        assert!(IssueSlot::default().is_empty());
        let slot = IssueSlot {
            counts: IssueCounts {
                error: 1,
                ..Default::default()
            },
            label: "on R1".into(),
            whole: false,
        };
        assert!(!slot.is_empty());
    }
}
