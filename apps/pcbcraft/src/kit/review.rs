//! The docked impact-review panel (Update PCB, update instances,
//! disconnect, remove, library update).
//!
//! Slice K4 (docs/sidebar-ia.md, "Dialogs and reviews"); R1 supplies its
//! content from slice C2's structured plan. Order: a one-line summary, the
//! blockers with their acknowledgements, collapsed change groups, then the
//! gated Apply with the reason it is gated beside it. A stale preview turns
//! the summary into "× The project changed · Recompute". The canvas keeps
//! the preview visible. Adapted from Solidcraft 4f3ac07a
//! `crates/ui-egui/src/ui_kit/review.rs` (`ReviewAndApply`: impact rows,
//! acknowledgement and gated apply; a modal `Confirm` there, docked here),
//! MIT option; see ATTRIBUTION.md.
#![allow(dead_code, reason = "slice K4 kit; slice R1 uses it")]

use super::dialog::{DialogAction, DialogPanel};
use super::severity::{Level, severity_glyph};
use egui::{Align, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit;

/// One blocker or acknowledgement row.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewLine {
    pub level: Level,
    /// One line, at most 80 characters, naming objects.
    pub title: String,
    /// The acknowledgement that clears this blocker ("Remove that track
    /// (Top, 4.2 mm)"), if one does.
    pub ack: Option<String>,
    /// Whether the line names objects the canvas can show ("Show" →
    /// [`ReviewAction::Locate`]).
    pub locate: bool,
}

/// One collapsed group of changes ("CHANGES · 6 placements · 12 pads").
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
    pub groups: &'a [ReviewGroup],
    pub apply: &'a str,
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
    pub fn show(&self, ui: &mut Ui, acked: &mut [bool]) -> Option<ReviewAction> {
        let mut action = None;
        let gate = apply_gate(self.stale, self.blockers, acked);
        let mut panel = DialogPanel::new(self.id, self.icon, self.title, self.apply);
        panel.kind = "Review";
        // The review owns the subject slot: its body takes the slot's height.
        panel.body_rows = f32::INFINITY;
        panel.primary_disabled = gate;
        panel.status = gate;
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
            for (n, group) in self.groups.iter().enumerate() {
                let expanded_id = self.id.with(("group_all", n));
                super::section(
                    ui,
                    (self.id, n),
                    &group.title,
                    false,
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

/// One clipped body line, with an optional severity glyph before it and an
/// optional action at the right. Returns whether the action was clicked.
fn glyph_line(ui: &mut Ui, level: Option<Level>, text: &str, action: Option<&str>) -> bool {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), t.control_height),
        Sense::hover(),
    );
    let mut left = rect.left();
    if let Some(level) = level {
        severity_glyph(
            ui.painter(),
            pos2(left + t.font_label / 2.0, rect.center().y),
            t.font_label,
            level,
        );
        left += t.font_label + t.space_8;
    }
    let mut right = rect.right();
    let mut clicked = false;
    if let Some(action) = action {
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("line_action", action))
                .max_rect(rect)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                let button = ui_kit::button(ui, action, ui_kit::Button::Enabled(true));
                clicked = button.clicked();
                right = button.rect.left() - t.space_8;
            },
        );
    }
    let text_rect =
        Rect::from_min_max(pos2(left, rect.top()), pos2(right.max(left), rect.bottom()));
    let cut = super::rows::paint_clipped(
        ui,
        text_rect,
        text,
        FontId::proportional(t.font_label),
        t.form_ink,
    );
    super::rows::line_hover(response, text, "", cut);
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

    #[test]
    fn the_review_reads_summary_blockers_then_groups_and_gates_apply() {
        use crate::kit::rows::tests::{Harness, at, find};
        use std::cell::{Cell, RefCell};
        let mut ui = Harness::new(380.0, 700.0);
        let blockers = [line(Level::Error, true)];
        let groups = [ReviewGroup {
            title: "Changes".to_owned(),
            summary: "6 placements · 12 pads".to_owned(),
            lines: (17..40).map(|n| format!("Adds R{n}")).collect(),
        }];
        let stale = Cell::new(false);
        let acked = RefCell::new([false]);
        let action = Cell::new(None);
        let mut add = |ui: &mut Ui| {
            let panel = ReviewPanel {
                id: egui::Id::new("review"),
                icon: "library",
                title: "Update PCB",
                summary: "Adds 6 placements: R17–R22",
                stale: stale.get(),
                blockers: &blockers,
                groups: &groups,
                apply: "Apply update",
            };
            action.set(panel.show(ui, &mut acked.borrow_mut()[..]));
        };
        let painted = ui.frame(vec![], &mut add);
        let y = |text: &str| at(&painted, text).y;
        assert!(y("Adds 6 placements: R17–R22") < y("1 track on N22 would join two nets"));
        assert!(y("1 track on N22 would join two nets") < y("CHANGES"));
        assert!(
            find(&painted, "Adds R17").is_none(),
            "groups start collapsed"
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
        let open = ui.click(at(&painted, "CHANGES"), &mut add);
        assert!(find(&open, "Adds R24").is_some(), "{open:?}");
        assert!(find(&open, "Adds R25").is_none(), "bounded at 8 lines");
        assert!(find(&open, "8 of 23").is_some());
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
