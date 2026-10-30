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
}
impl UpdatePreview {
    pub fn begin(&mut self, engine: &Engine, change: Change) {
        self.plan = Some(engine.preview_update(change));
        self.remove_tracks = false;
    }
    pub fn is_open(&self) -> bool {
        self.plan.is_some()
    }
    pub fn show(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        let plan = self.plan.as_ref()?;
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
            && plan.blockers.is_empty()
            && (plan.conflicting_tracks.is_empty() || remove_tracks);
        let mut action = None;
        let mut cancel = false;
        kit::rule(ui);
        kit::actions(ui, |ui| {
            let apply = ui_kit::button(
                ui,
                "Apply update",
                if ready {
                    Button::Primary
                } else {
                    Button::Enabled(false)
                },
            );
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
        if recompute && let Some(plan) = &self.plan {
            let change = plan.change.clone();
            self.begin(engine, change);
        }
        if cancel || action.is_some() {
            self.plan = None;
            self.remove_tracks = false;
        }
        action
    }
}
