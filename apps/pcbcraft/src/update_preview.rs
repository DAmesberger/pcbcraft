//! Revision-bound review shared by all editors, laid out with the card kit
//! like Solidcraft's feature dialogs (sections, then a Cancel/OK footer).
//! Like its library consumers, a delayed action belongs to its original document.
use crate::kit::{self, Tone};
use pcbcraft_core::{
    Engine, Operation,
    updates::{ApplyUpdate, Change, UpdatePlan},
};
use solidcraft_ui_egui::ui_kit::{self, Button};

#[derive(Default)]
pub struct UpdatePreview {
    plan: Option<UpdatePlan>,
    remove_tracks: bool,
    /// A component-revision review that lets the user pick instances: each
    /// candidate with its designator and whether it moves.
    choices: Vec<(pcbcraft_core::Id, String, bool)>,
    /// Said above the changes (what already happened before the review).
    note: Option<String>,
    /// How the last review closed, for the host to act on once.
    closed: Option<Closed>,
}

/// How a review closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    Applied,
    Cancelled,
}

impl UpdatePreview {
    pub fn begin(&mut self, engine: &Engine, change: Change) {
        self.plan = Some(engine.preview_update(change));
        self.remove_tracks = false;
        self.choices.clear();
        self.note = None;
    }

    /// [`Self::begin`] for a component-revision change, listing each of its
    /// instances with a checkbox (all on), and `note` above the changes.
    pub fn begin_choosing(&mut self, engine: &Engine, change: Change, note: Option<String>) {
        let choices = match &change {
            Change::ComponentRevision { instances, .. } => instances
                .iter()
                .map(|id| {
                    let designator = engine
                        .project()
                        .instance(*id)
                        .map_or_else(|| id.to_string(), |i| i.designator.clone());
                    (*id, designator, true)
                })
                .collect(),
            _ => Vec::new(),
        };
        self.begin(engine, change);
        self.choices = choices;
        self.note = note;
    }

    /// How the last review closed, once.
    pub fn take_closed(&mut self) -> Option<Closed> {
        self.closed.take()
    }

    /// The plan again for the checked instances.
    fn rechoose(&mut self, engine: &Engine) {
        let Some(plan) = &self.plan else {
            return;
        };
        let Change::ComponentRevision {
            target,
            resolutions,
            ..
        } = &plan.change
        else {
            return;
        };
        let change = Change::ComponentRevision {
            instances: self
                .choices
                .iter()
                .filter(|(_, _, on)| *on)
                .map(|(id, _, _)| *id)
                .collect(),
            target: *target,
            resolutions: resolutions.clone(),
        };
        self.plan = Some(engine.preview_update(change));
    }
    pub fn is_open(&self) -> bool {
        self.plan.is_some()
    }
    pub fn show(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        self.plan.as_ref()?;
        if let Some(note) = &self.note {
            kit::status(ui, note, Tone::Note);
        }
        if !self.choices.is_empty() {
            let mut changed = false;
            let count = self.choices.len();
            kit::section(
                ui,
                "update-instances",
                "Instances",
                true,
                Some(count),
                |ui| {
                    for (_, designator, on) in &mut self.choices {
                        changed |= kit::check_row(ui, designator, on);
                    }
                    kit::note(
                        ui,
                        "Only the checked instances move; the others keep their revision.",
                    );
                },
            );
            if changed {
                self.rechoose(engine);
            }
        }
        let plan = self.plan.as_ref()?;
        let chosen = self.choices.is_empty() || self.choices.iter().any(|(_, _, on)| *on);
        kit::section(
            ui,
            "update-changes",
            "Changes",
            true,
            Some(plan.changes.len()),
            |ui| {
                if plan.changes.is_empty() {
                    kit::status(ui, "Nothing to change.", Tone::Dim);
                }
                for change in &plan.changes {
                    kit::status(ui, change, Tone::Dim);
                }
            },
        );
        for warning in &plan.warnings {
            kit::status(ui, warning, Tone::Warning);
        }
        for blocker in &plan.blockers {
            kit::status(ui, blocker, Tone::Error);
        }
        let mut remove_tracks = self.remove_tracks;
        if !plan.conflicting_tracks.is_empty() {
            kit::section(
                ui,
                "update-conflicts",
                "Conflicting tracks",
                true,
                Some(plan.conflicting_tracks.len()),
                |ui| {
                    let ids = plan
                        .conflicting_tracks
                        .iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    kit::value_row(ui, "Tracks", &ids);
                    kit::check_row(ui, "Remove these conflicting tracks", &mut remove_tracks);
                    kit::note(
                        ui,
                        "Other copper stays in place. You can undo the entire update.",
                    );
                },
            );
        }
        let stale = plan.revision != engine.revision();
        let mut recompute = false;
        if stale {
            kit::status(
                ui,
                "The project changed. Recompute this preview before applying.",
                Tone::Error,
            );
            kit::actions(ui, |ui| {
                recompute = ui_kit::button(ui, "Recompute", Button::Normal).clicked();
            });
        }
        let ready = !stale
            && chosen
            && plan.blockers.is_empty()
            && (plan.conflicting_tracks.is_empty() || remove_tracks);
        let mut action = None;
        let mut cancel = false;
        kit::rule(ui);
        kit::actions(ui, |ui| {
            let apply = kit::primary(ui, "Apply update", ready);
            if apply.clicked() && ready {
                action = Some(Operation::Update(Box::new(ApplyUpdate {
                    expected_revision: plan.revision,
                    change: plan.change.clone(),
                    remove_conflicting_tracks: remove_tracks,
                })));
            }
            cancel = ui_kit::button(ui, "Cancel", Button::Normal).clicked();
        });
        self.remove_tracks = remove_tracks;
        if recompute {
            // Keep the instance choice and the note across a recompute.
            if !self.choices.is_empty() {
                self.rechoose(engine);
            } else if let Some(plan) = &self.plan {
                self.plan = Some(engine.preview_update(plan.change.clone()));
            }
        }
        if cancel || action.is_some() {
            self.plan = None;
            self.remove_tracks = false;
            self.choices.clear();
            self.note = None;
            self.closed = Some(if cancel {
                Closed::Cancelled
            } else {
                Closed::Applied
            });
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A review that lets the user pick instances lists each one checked;
    /// unchecking one recomputes the plan for the others only.
    #[test]
    fn unchecked_instances_leave_the_reviewed_change() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let instances: Vec<_> = engine.project().instances.iter().map(|i| i.id).collect();
        let first = engine.project().instances.first().ok_or("an instance")?;
        assert!(instances.len() >= 2, "the demo places several instances");
        let change = Change::ComponentRevision {
            instances: instances.clone(),
            target: first.component,
            resolutions: Default::default(),
        };
        let mut review = UpdatePreview::default();
        review.begin_choosing(&engine, change, Some("copied".into()));
        assert_eq!(review.choices.len(), instances.len());
        assert!(review.choices.iter().all(|(_, _, on)| *on));
        assert_eq!(review.note.as_deref(), Some("copied"));
        review.choices[0].2 = false;
        review.rechoose(&engine);
        match review.plan.as_ref().map(|p| &p.change) {
            Some(Change::ComponentRevision {
                instances: kept, ..
            }) => {
                assert_eq!(kept.as_slice(), &instances[1..]);
            }
            other => return Err(format!("unexpected {other:?}").into()),
        }
        assert_eq!(review.take_closed(), None);
        Ok(())
    }
}
