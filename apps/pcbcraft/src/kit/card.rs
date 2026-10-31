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
use solidcraft_ui_egui::theme::Tokens;

/// The pinned footer's height, in points.
pub const FOOTER_HEIGHT: f32 = 48.0;

/// Body rows the subject keeps while Issues is expanded (upstream
/// `stack_first_bounds`: header plus ten rows).
pub const SUBJECT_ROWS: f32 = 10.0;

/// What the subject slot holds. Exactly one at a time.
#[allow(
    dead_code,
    reason = "Properties is the only subject until the R and W slices add the others"
)]
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

/// Split the column: collapsed, Issues is a [`ISSUES_BAR_HEIGHT`] bar at the
/// bottom; expanded, the subject keeps its header and [`SUBJECT_ROWS`] rows,
/// or half the column if that is more, and Issues takes the rest. The
/// Issues bar always keeps its full height, however short the column; the
/// subject gives way first.
pub fn column(rect: Rect, issues_expanded: bool) -> Column {
    let t = Tokens::get();
    let height = rect.height().max(0.0);
    let bar = ISSUES_BAR_HEIGHT.min(height);
    // Room above the bar for the subject; never negative.
    let room = (height - bar - gap()).max(0.0);
    let subject_height = if issues_expanded {
        let rows = t.form_header_height + SUBJECT_ROWS * t.control_height + 2.0 * t.space_8;
        rows.max((height / 2.0).floor()).min(room)
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
    let header = &inspector.header;
    super::inspector_header(ui, header.icon, &header.title, header.kind);
    let slot_height = t.control_height + ui.spacing().item_spacing.y;
    let footer_height = if inspector.footer { FOOTER_HEIGHT } else { 0.0 };
    let body_height = (ui.available_height() - slot_height - footer_height).max(0.0);
    let key = ui.id().with("card-body");
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(key)
        .max_height(body_height)
        .min_scrolled_height(body_height)
        .auto_shrink([false, false]);
    if reset_scroll(ui, key, inspector.subject) {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    let mut output = scroll
        .show(ui, |ui| {
            ui.add_space(t.space_4);
            let inner = egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(t.space_8 as i8, 0))
                .show(ui, |ui| super::form(ui, |ui| content(ui, Region::Body)))
                .inner;
            ui.add_space(t.space_8);
            inner
        })
        .inner;
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
        let c = column(rect, false);
        assert_eq!(c.issues.height(), ISSUES_BAR_HEIGHT);
        assert_eq!(c.issues.bottom(), rect.bottom());
        assert_eq!(c.subject.top(), rect.top());
        assert!((c.issues.top() - c.subject.bottom() - gap()).abs() < 1e-3);
        assert_eq!(c.subject.width(), rect.width());
        assert_eq!(c.issues.width(), rect.width());
    }

    #[test]
    fn expanded_issues_shares_the_column_and_the_subject_keeps_its_rows() {
        let t = Tokens::get();
        let rows = t.form_header_height + SUBJECT_ROWS * t.control_height + 2.0 * t.space_8;
        // Tall: half the column is more than the rows.
        let tall = column(column_rect(1081.0), true);
        assert!((tall.subject.height() - 540.0).abs() < 1e-3);
        assert!(tall.issues.height() > 500.0);
        // Short: the subject keeps its header and ten rows.
        let short = column(column_rect(rows + 200.0), true);
        assert!((short.subject.height() - rows).abs() < 1e-3);
        assert!(short.issues.height() >= ISSUES_BAR_HEIGHT);
    }

    /// However short the window, the bar never shrinks or leaves the
    /// column, so blockers stay counted on screen.
    #[test]
    fn the_issues_bar_survives_any_column_height() {
        for height in [0.0, 10.0, ISSUES_BAR_HEIGHT, 40.0, 120.0, 400.0] {
            for expanded in [false, true] {
                let rect = column_rect(height);
                let c = column(rect, expanded);
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
            subject: egui::Id::new(subject),
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
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
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
        let key = egui::Id::new("k");
        let mut seen = Vec::new();
        for subject in ["a", "a", "b", "b", "a"] {
            run(&ctx, |ui| {
                seen.push(reset_scroll(ui, key, egui::Id::new(subject)))
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
        };
        assert!(!slot.is_empty());
    }
}
