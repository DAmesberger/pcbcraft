//! The docked impact-review panel (Update PCB, update instances,
//! disconnect, remove, library update).
//!
//! Contract from slice S0 (docs/sidebar-ia.md, "Review panel"); slice K4
//! owns the look and R1 its content from slice C2's structured plan. Order:
//! a one-line summary, the blockers, collapsed change groups, then the
//! acknowledgement beside the gated Apply. A stale preview turns the
//! summary into a Recompute row. The canvas keeps the preview visible.
//! Adapted from Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/review.rs`
//! (`ReviewAndApply`, there a modal), MIT option; see ATTRIBUTION.md.
#![allow(dead_code, reason = "S0 contract; slices K4 and R1 use it")]

use super::dialog::DialogPanel;
use super::severity::Level;
use egui::Ui;

/// One blocker or acknowledgement row.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewLine {
    pub level: Level,
    /// One line, at most 80 characters, naming objects.
    pub title: String,
    /// The acknowledgement that clears this blocker ("Remove that track
    /// (Top, 4.2 mm)"), if one does.
    pub ack: Option<String>,
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
}

impl ReviewPanel<'_> {
    /// Draw the review. `acked[n]` holds blocker `n`'s acknowledgement;
    /// Apply is enabled only when the preview is current and every
    /// acknowledgeable blocker is acknowledged and no other blocker remains.
    pub fn show(&self, ui: &mut Ui, acked: &mut [bool]) -> Option<ReviewAction> {
        let mut action = None;
        let blocked = self.stale
            || self.blockers.iter().enumerate().any(|(n, line)| {
                line.level == Level::Error
                    && (line.ack.is_none() || !acked.get(n).copied().unwrap_or(false))
            });
        let mut panel = DialogPanel::new(self.id, self.icon, self.title, self.apply);
        panel.kind = "Review";
        if blocked {
            panel.primary_disabled = Some(if self.stale {
                "The project changed; recompute the review"
            } else {
                "Resolve the blockers to apply"
            });
        }
        let footer = panel.show(ui, |ui| {
            if self.stale {
                if super::summary_row(ui, "The project changed", Some("Recompute")) {
                    action = Some(ReviewAction::Recompute);
                }
            } else {
                super::hint(ui, self.summary, self.summary);
            }
            for (n, line) in self.blockers.iter().enumerate() {
                super::hint(ui, &line.title, &line.title);
                if let (Some(ack), Some(value)) = (&line.ack, acked.get_mut(n))
                    && super::check_row(ui, ack, value)
                {
                    action = Some(ReviewAction::Acknowledge(n));
                }
            }
            for (n, group) in self.groups.iter().enumerate() {
                super::section(
                    ui,
                    (self.id, n),
                    &group.title,
                    false,
                    Some(group.lines.len()),
                    |ui| {
                        super::hint(ui, &group.summary, &group.summary);
                        for line in &group.lines {
                            super::hint(ui, line, line);
                        }
                    },
                );
            }
            if super::summary_row(ui, "", Some("Copy report")) {
                action = Some(ReviewAction::CopyReport);
            }
        });
        match footer {
            Some(super::dialog::DialogAction::Primary) => Some(ReviewAction::Apply),
            Some(super::dialog::DialogAction::Cancel) => Some(ReviewAction::Cancel),
            None => action,
        }
    }
}
