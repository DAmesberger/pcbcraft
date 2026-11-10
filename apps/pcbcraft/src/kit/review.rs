//! The docked impact-review panel (Update PCB, update instances,
//! disconnect, remove, library update).
//!
//! Slice K4 (docs/sidebar-ia.md, "Dialogs and reviews"); R1 supplies its
//! content from slice C2's structured plan. Order: a one-line summary, the
//! blockers with their acknowledgements, the change groups (open when they
//! list changes, so the moves are read without a click), then the
//! gated Apply with the reason it is gated beside it. A stale preview turns
//! the summary into "× The project changed · Recompute". The canvas keeps
//! the preview visible. Adapted from Solidcraft 4f3ac07a
//! `crates/ui-egui/src/ui_kit/review.rs` (`ReviewAndApply`: impact rows,
//! acknowledgement and gated apply; a modal `Confirm` there, docked here),
//! MIT option; see ATTRIBUTION.md.

use super::dialog::{DialogAction, DialogPanel};
use super::severity::{Level, severity_glyph};
use egui::{Align, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::ui_kit;

/// One blocker or acknowledgement row.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewLine {
    pub level: Level,
    /// One line naming objects, cut with its full text on hover.
    pub title: String,
    /// The acknowledgement that clears this blocker ("Remove that track
    /// (Top, 4.2 mm)"), if one does.
    pub ack: Option<String>,
    /// Whether the line names objects the canvas can show ("Show" →
    /// [`ReviewAction::Locate`]).
    pub locate: bool,
}

/// One group of changes ("CHANGES · 6 placements · 12 pads"), open by
/// default when it lists any.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewGroup {
    pub title: String,
    pub summary: String,
    pub lines: Vec<String>,
}

/// What the review shows.
pub struct ReviewPanel<'a> {
    pub id: egui::Id,
    pub icon: &'a str,
    pub title: &'a str,
    pub summary: &'a str,
    /// The document changed since the preview: offer Recompute.
    pub stale: bool,
    pub blockers: &'a [ReviewLine],
    /// What applying leaves behind (new errors), under the blockers: shown
    /// with Show, counted beside Apply, never gating it.
    pub after: &'a [ReviewLine],
    pub groups: &'a [ReviewGroup],
    pub apply: &'a str,
    /// The primary removes work ("Remove R1"): the destructive variant.
    pub destructive: bool,
}

/// What the review's controls did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewAction {
    Cancel,
    Apply,
    Recompute,
    CopyReport,
    /// The acknowledgement of blocker `n` was toggled.
    Acknowledge(usize),
    /// Show blocker `n`'s objects on the canvas.
    Locate(usize),
    /// Show the objects of `after[n]`.
    LocateAfter(usize),
}

/// Why Apply is disabled, or `None` when it may run: the preview is
/// current, every blocker with an acknowledgement is acknowledged, and no
/// Error blocker lacks one.
pub fn apply_gate(stale: bool, blockers: &[ReviewLine], acked: &[bool]) -> Option<&'static str> {
    if stale {
        return Some("The project changed: recompute to apply");
    }
    if blockers
        .iter()
        .any(|line| line.level == Level::Error && line.ack.is_none())
    {
        return Some("Fix the errors above to apply");
    }
    if blockers
        .iter()
        .enumerate()
        .any(|(n, line)| line.ack.is_some() && !acked.get(n).copied().unwrap_or(false))
    {
        return Some("Confirm each blocker above to apply");
    }
    None
}

impl ReviewPanel<'_> {
    /// Draw the review. `acked[n]` holds blocker `n`'s acknowledgement.
    #[allow(
        dead_code,
        reason = "the contract's plain form; the update review uses show_with"
    )]
    pub fn show(&self, ui: &mut Ui, acked: &mut [bool]) -> Option<ReviewAction> {
        self.show_with(ui, acked, None, |_| {})
    }

    /// [`Self::show`] with the engine's refusal of the last Apply as the
    /// body's last row (it does not gate a retry), and `inputs` drawn after
    /// the blockers and before the change groups (a review's own choices,
    /// such as which instances an update moves). Slice R1, additive.
    pub fn show_with(
        &self,
        ui: &mut Ui,
        acked: &mut [bool],
        refusal: Option<&str>,
        inputs: impl FnOnce(&mut Ui),
    ) -> Option<ReviewAction> {
        let mut action = None;
        let gate = apply_gate(self.stale, self.blockers, acked);
        let new_errors = self
            .after
            .iter()
            .filter(|l| l.level == Level::Error)
            .count();
        let leaves = match new_errors {
            0 => None,
            1 => Some("Leaves 1 new error".to_owned()),
            n => Some(format!("Leaves {n} new errors")),
        };
        let mut panel = DialogPanel::new(self.id, self.icon, self.title, self.apply);
        panel.kind = "Review";
        if self.destructive {
            panel.primary_kind = super::dialog::PrimaryKind::Destructive;
        }
        // The review owns the subject slot: its body takes the slot's height.
        panel.body_rows = f32::INFINITY;
        panel.primary_disabled = gate;
        panel.status = gate.or(leaves.as_deref());
        panel.refusal = refusal;
        let footer = panel.show(ui, |ui| {
            let t = Tokens::get();
            if self.stale {
                if glyph_line(
                    ui,
                    Some(Level::Error),
                    "The project changed",
                    Some("Recompute"),
                ) {
                    action = Some(ReviewAction::Recompute);
                }
            } else {
                glyph_line(ui, None, self.summary, None);
            }
            for (n, line) in self.blockers.iter().enumerate() {
                ui.push_id(("blocker", n), |ui| {
                    if glyph_line(
                        ui,
                        Some(line.level),
                        &line.title,
                        line.locate.then_some("Show"),
                    ) {
                        action = Some(ReviewAction::Locate(n));
                    }
                    if let (Some(ack), Some(value)) = (&line.ack, acked.get_mut(n)) {
                        ui.horizontal(|ui| {
                            ui.add_space(t.font_label + t.space_8);
                            if ui.checkbox(value, ack.as_str()).changed() {
                                action = Some(ReviewAction::Acknowledge(n));
                            }
                        });
                    }
                });
            }
            for (n, line) in self.after.iter().enumerate() {
                ui.push_id(("after", n), |ui| {
                    if glyph_line(
                        ui,
                        Some(line.level),
                        &line.title,
                        line.locate.then_some("Show"),
                    ) {
                        action = Some(ReviewAction::LocateAfter(n));
                    }
                });
            }
            inputs(ui);
            for (n, group) in self.groups.iter().enumerate() {
                let expanded_id = self.id.with(("group_all", n));
                super::section(
                    ui,
                    (self.id, n),
                    &group.title,
                    !group.lines.is_empty(),
                    Some(group.lines.len()),
                    |ui| {
                        if !group.summary.is_empty() {
                            glyph_line(ui, None, &group.summary, None);
                        }
                        let all = ui
                            .data(|d| d.get_temp::<bool>(expanded_id))
                            .unwrap_or(false);
                        let max = if all {
                            group.lines.len()
                        } else {
                            super::LIST_MAX_ROWS
                        };
                        let shown = super::list_frame(ui, group.lines.len(), max, |ui, i| {
                            if let Some(line) = group.lines.get(i) {
                                glyph_line(ui, None, line, None);
                            }
                        });
                        if shown.show_all {
                            ui.data_mut(|d| d.insert_temp(expanded_id, true));
                        }
                    },
                );
            }
            if super::summary_row(ui, "", Some("Copy report")) {
                action = Some(ReviewAction::CopyReport);
            }
        });
        match footer {
            Some(DialogAction::Primary) => Some(ReviewAction::Apply),
            Some(DialogAction::Cancel) => Some(ReviewAction::Cancel),
            None => action,
        }
    }
}

/// Rows a review line wraps to before it is cut (with the full text on
/// hover).
const LINE_MAX_ROWS: usize = 6;

/// One body line, wrapped to the panel's width, with an optional severity
/// glyph before it and an optional action at the right of its first row.
/// Returns whether the action was clicked.
fn glyph_line(ui: &mut Ui, level: Option<Level>, text: &str, action: Option<&str>) -> bool {
    let t = Tokens::get();
    let font = FontId::proportional(t.font_label);
    let top = ui.available_rect_before_wrap();
    let first = Rect::from_min_size(
        top.min,
        vec2(ui.available_width().max(0.0), t.control_height),
    );
    let mut left = first.left();
    if level.is_some() {
        left += t.font_label + t.space_8;
    }
    let mut right = first.right();
    let mut clicked = false;
    if let Some(action) = action {
        // On the first row, beside the text; the row is allocated below.
        let mut row = ui.new_child(
            UiBuilder::new()
                .id_salt(("line_action", action))
                .max_rect(first)
                .layout(Layout::right_to_left(Align::Center)),
        );
        let button = ui_kit::button(&mut row, action, ui_kit::Button::Enabled(true));
        clicked = button.clicked();
        right = button.rect.left() - t.space_8;
    }
    let galley = super::text::wrapped(
        ui,
        text,
        font.clone(),
        t.form_ink,
        (right - left).max(1.0),
        LINE_MAX_ROWS,
    );
    let cut = galley.elided;
    let pad = ((t.control_height - ui.fonts_mut(|f| f.row_height(&font))) / 2.0).max(0.0);
    let (rect, response) = ui.allocate_exact_size(
        vec2(
            first.width(),
            (galley.size().y + 2.0 * pad).max(t.control_height),
        ),
        Sense::hover(),
    );
    if let Some(level) = level {
        severity_glyph(
            ui.painter(),
            pos2(
                rect.left() + t.font_label / 2.0,
                rect.top() + t.control_height / 2.0,
            ),
            t.font_label,
            level,
        );
    }
    let text_rect = Rect::from_min_size(pos2(left, rect.top() + pad), galley.size());
    ui.painter().galley(text_rect.min, galley, t.form_ink);
    // The hover covers the text only, so the action keeps its own.
    super::text::hover(
        ui.interact(text_rect, response.id.with("text"), Sense::hover()),
        text,
        "",
        cut,
    );
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(level: Level, ack: bool) -> ReviewLine {
        ReviewLine {
            level,
            title: "1 track on N22 would join two nets".to_owned(),
            ack: ack.then(|| "Remove that track (Top, 4.2 mm)".to_owned()),
            locate: true,
        }
    }

    #[test]
    fn apply_waits_for_a_current_preview_and_every_acknowledgement() {
        assert_eq!(apply_gate(false, &[], &[]), None);
        assert!(
            apply_gate(true, &[], &[]).is_some(),
            "a stale preview gates"
        );
        let error_unackable = [line(Level::Error, false)];
        assert!(apply_gate(false, &error_unackable, &[true]).is_some());
        let acked = [line(Level::Error, true), line(Level::Warning, true)];
        assert!(
            apply_gate(false, &acked, &[true, false]).is_some(),
            "a warning's ack counts"
        );
        assert!(
            apply_gate(false, &acked, &[true]).is_some(),
            "a missing ack is not given"
        );
        assert_eq!(apply_gate(false, &acked, &[true, true]), None);
        let info = [line(Level::Info, false)];
        assert_eq!(
            apply_gate(false, &info, &[]),
            None,
            "information never gates"
        );
    }

    /// Demo run 3: the Update review's long lines (the summary, a warning
    /// with Show, a component change, what the update leaves) wrap in the
    /// panel instead of being cut, and Show stays on the warning's line.
    #[test]
    fn long_review_lines_wrap_instead_of_being_cut() {
        use crate::kit::rows::tests::{Harness, at, find};
        use std::cell::Cell;
        let mut ui = Harness::new(250.0, 900.0);
        let summary = "Changes 1 component, 2 terminals, 2 pads and 1 instance of U1";
        let warning = "U1: Part ESP32-S31-WROOM-3 r1 keeps an older revision of its component";
        let change = "U1: a package terminal now maps to another electrical terminal";
        let left = "fill GND top needs refilling: the board changed under it";
        let blockers = [ReviewLine {
            level: Level::Warning,
            title: warning.to_owned(),
            ack: None,
            locate: true,
        }];
        let after = [ReviewLine {
            level: Level::Warning,
            title: left.to_owned(),
            ack: None,
            locate: false,
        }];
        let groups = [ReviewGroup {
            title: "Component".to_owned(),
            summary: String::new(),
            lines: vec![change.to_owned()],
        }];
        let action = Cell::new(None);
        let mut add = |ui: &mut Ui| {
            let panel = ReviewPanel {
                id: egui::Id::unique("review-wrap"),
                icon: "library",
                title: "Update instances",
                summary,
                stale: false,
                blockers: &blockers,
                after: &after,
                groups: &groups,
                apply: "Apply update",
                destructive: false,
            };
            action.set(panel.show(ui, &mut [][..]));
        };
        ui.frame(vec![], &mut add);
        let painted = ui.frame(vec![], &mut add);
        for text in [summary, warning, change, left] {
            let shown = find(&painted, text);
            assert!(shown.is_some(), "{text}: {painted:?}");
            let Some(shown) = shown else { return };
            assert!(!shown.elided, "{text} is shown whole: {shown:?}");
            assert!(shown.rows > 1, "{text} wraps: {shown:?}");
            assert!(shown.rect.right() <= 250.0 + 0.5, "{shown:?}");
        }
        // Show sits on the warning's first row, and still works.
        let show = at(&painted, "Show");
        let first = find(&painted, warning).map_or(0.0, |p| p.rect.top());
        assert!(show.y < first + 30.0, "{show:?} {first}");
        ui.click(show, &mut add);
        assert_eq!(action.get(), Some(ReviewAction::Locate(0)));
    }

    #[test]
    fn the_review_reads_summary_blockers_then_groups_and_gates_apply() {
        use crate::kit::rows::tests::{Harness, at, find};
        use std::cell::{Cell, RefCell};
        let mut ui = Harness::new(380.0, 700.0);
        let blockers = [line(Level::Error, true)];
        let groups = [
            ReviewGroup {
                title: "Changes".to_owned(),
                summary: "6 placements · 12 pads".to_owned(),
                lines: (17..40).map(|n| format!("Adds R{n}")).collect(),
            },
            ReviewGroup {
                title: "After this update".to_owned(),
                summary: "No new issues".to_owned(),
                lines: Vec::new(),
            },
        ];
        let stale = Cell::new(false);
        let acked = RefCell::new([false]);
        let action = Cell::new(None);
        let mut add = |ui: &mut Ui| {
            let panel = ReviewPanel {
                id: egui::Id::unique("review"),
                icon: "library",
                title: "Update PCB",
                summary: "Adds 6 placements: R17–R22",
                stale: stale.get(),
                blockers: &blockers,
                after: &[],
                groups: &groups,
                apply: "Apply update",
                destructive: false,
            };
            action.set(panel.show(ui, &mut acked.borrow_mut()[..]));
        };
        let painted = ui.frame(vec![], &mut add);
        let y = |text: &str| at(&painted, text).y;
        assert!(y("Adds 6 placements: R17–R22") < y("1 track on N22 would join two nets"));
        assert!(y("1 track on N22 would join two nets") < y("CHANGES"));
        // A group that lists changes opens by default, bounded at 8
        // lines; one with none stays folded.
        assert!(find(&painted, "Adds R17").is_some(), "{painted:?}");
        assert!(find(&painted, "Adds R25").is_none(), "bounded at 8 lines");
        assert!(find(&painted, "8 of 23").is_some());
        assert!(
            find(&painted, "No new issues").is_none(),
            "a group without changes stays folded"
        );
        assert!(
            find(&painted, "Confirm each blocker above to apply").is_some(),
            "{painted:?}"
        );
        ui.click(at(&painted, "Apply update"), &mut add);
        assert_eq!(action.get(), None, "gated");
        ui.click(at(&painted, "Remove that track (Top, 4.2 mm)"), &mut add);
        assert_eq!(action.get(), Some(ReviewAction::Acknowledge(0)));
        assert_eq!(*acked.borrow(), [true]);
        ui.click(at(&painted, "Apply update"), &mut add);
        assert_eq!(action.get(), Some(ReviewAction::Apply));
        ui.click(at(&painted, "Show"), &mut add);
        assert_eq!(action.get(), Some(ReviewAction::Locate(0)));
        let open = ui.frame(vec![], &mut add);
        let all = ui.click(at(&open, "Show all \u{203a}"), &mut add);
        let all = if find(&all, "Adds R39").is_some() {
            all
        } else {
            ui.frame(vec![], &mut add)
        };
        assert!(
            find(&all, "Adds R39").is_some(),
            "Show all lists every line"
        );

        stale.set(true);
        let painted = ui.frame(vec![], &mut add);
        assert!(find(&painted, "The project changed").is_some());
        assert!(find(&painted, "Adds 6 placements: R17–R22").is_none());
        ui.click(at(&painted, "Recompute"), &mut add);
        assert_eq!(action.get(), Some(ReviewAction::Recompute));
        ui.click(at(&painted, "Apply update"), &mut add);
        assert_eq!(action.get(), None, "a stale review never applies");
    }

    #[test]
    fn gate_reasons_follow_the_message_style() {
        for reason in [
            apply_gate(true, &[], &[]),
            apply_gate(false, &[line(Level::Error, false)], &[]),
            apply_gate(false, &[line(Level::Error, true)], &[false]),
        ] {
            let reason = reason.unwrap_or_default();
            assert!(!reason.is_empty());
            assert_eq!(
                pcbcraft_core::issues::style::check(reason),
                vec![],
                "{reason}"
            );
        }
    }
}
