//! Revision-bound review shared by all editors. Uses Solidcraft's panel controls;
//! like its library consumers, a delayed action belongs to its original document.
use pcbcraft_core::{
    Engine, Operation,
    updates::{ApplyUpdate, Change, UpdatePlan},
};
use solidcraft_ui_egui::{
    theme::Tokens,
    ui_kit::{self, Button},
};

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
        let t = Tokens::get();
        ui.strong("Review update");
        for change in &plan.changes {
            ui.label(change);
        }
        for warning in &plan.warnings {
            ui.colored_label(t.form_primary, warning);
        }
        for blocker in &plan.blockers {
            ui.colored_label(t.form_error, blocker);
        }
        if !plan.conflicting_tracks.is_empty() {
            ui.separator();
            ui.label(format!(
                "Conflicting tracks: {}",
                plan.conflicting_tracks
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            ui.checkbox(&mut self.remove_tracks, "Remove these conflicting tracks");
            ui.weak("Other copper stays in place. You can undo the entire update.");
        }
        let stale = plan.revision != engine.revision();
        if stale {
            ui.colored_label(
                t.form_error,
                "The project changed. Recompute this preview before applying.",
            );
        }
        let mut action = None;
        let mut cancel = false;
        let mut recompute = false;
        ui.horizontal_wrapped(|ui| {
            if stale && ui_kit::button(ui, "Recompute", Button::Normal).clicked() {
                recompute = true;
            }
            if ui_kit::button(
                ui,
                "Apply update",
                Button::Enabled(
                    !stale
                        && plan.blockers.is_empty()
                        && (plan.conflicting_tracks.is_empty() || self.remove_tracks),
                ),
            )
            .clicked()
            {
                action = Some(Operation::Update(Box::new(ApplyUpdate {
                    expected_revision: plan.revision,
                    change: plan.change.clone(),
                    remove_conflicting_tracks: self.remove_tracks,
                })));
            }
            cancel = ui_kit::button(ui, "Cancel", Button::Ghost).clicked();
        });
        if recompute {
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
