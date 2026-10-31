//! The Component 3D tab's placement inspector: numeric translation and
//! rotation, one-click actions, "fit leads to pads" with its residuals, a
//! point-to-point move and the placement findings. Each control sends one
//! typed command; typed values are previewed and commit when the field is
//! left (Enter or a click elsewhere).
use super::PartEditor;
use super::model_place::{self, PointToPoint};
use crate::kit::{self, Tone};
use egui::Ui;
use pcbcraft_core::library::package_fit;
use pcbcraft_core::library::{Axis, PackageEdit, QuickAction};
use pcbcraft_core::{Engine, Operation};
use solidcraft_ui_egui::ui_kit::{self, Button};

const LENGTH: std::ops::RangeInclusive<f64> = -10000.0..=10000.0;
/// A fitted error above this is explained (mm, estimate).
const FIT_TOLERANCE_MM: f64 = 0.05;
const ANGLE: std::ops::RangeInclusive<f64> = -360.0..=360.0;

/// The quick actions offered, in order, with their labels.
const QUICK: [(&str, QuickAction); 11] = [
    ("X +90°", rotate(Axis::X, 1)),
    ("X −90°", rotate(Axis::X, -1)),
    ("Y +90°", rotate(Axis::Y, 1)),
    ("Y −90°", rotate(Axis::Y, -1)),
    ("Z +90°", rotate(Axis::Z, 1)),
    ("Z −90°", rotate(Axis::Z, -1)),
    ("Flip", QuickAction::Flip),
    ("Stand up", QuickAction::StandUp),
    ("Seat", QuickAction::Seat),
    ("Centre on pads", QuickAction::CentreOnPads),
    ("Pin 1 → pad 1", QuickAction::Pin1ToPad1),
];

const fn rotate(axis: Axis, quarter_turns: i8) -> QuickAction {
    QuickAction::Rotate {
        axis,
        quarter_turns,
    }
}

impl PartEditor {
    /// The placement sections of the 3D tab.
    pub(super) fn placement_inspector(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> Option<Operation> {
        let set = self.draft.as_ref()?;
        let draft = set.ids.package;
        let package = set.package.clone();
        let footprint = set.footprint.clone();
        let revision = engine.revision();
        let mut operation = None;
        let tab = &mut self.model;
        kit::section(ui, "model-placement", "Model offset", true, None, |ui| {
            let mut values = tab.shown(&package);
            let mut changed = false;
            for (index, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                changed |= kit::number_row(ui, label, &mut values.translation[index], "mm", LENGTH);
            }
            for (index, label) in ["Rotate X", "Rotate Y", "Rotate Z"].into_iter().enumerate() {
                changed |= kit::number_row(ui, label, &mut values.rotation_deg[index], "°", ANGLE);
            }
            if changed && tab.drag.is_none() {
                tab.fields = Some(values);
            }
            kit::hint(
                ui,
                "Shift in mm, then turns about X, Y and Z",
                "The model frame in the package datum: a shift in mm, then turns about X, Y and Z",
            );
        });
        let focused = ui.ctx().memory(|m| m.focused().is_some());
        if !focused && let Some(values) = tab.fields.take() {
            operation = tab.set(draft, &package.placement, values, revision);
        }
        kit::section(ui, "model-quick", "Quick actions", true, None, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, action) in QUICK {
                    if ui_kit::button(ui, label, Button::Small).clicked() {
                        operation = Some(model_place::edit(
                            draft,
                            vec![PackageEdit::ApplyQuick(action)],
                        ));
                    }
                }
            });
            let armed = tab.point_to_point != PointToPoint::Off;
            let label = match tab.point_to_point {
                PointToPoint::Off => "Point to point",
                PointToPoint::Source => "Pick a model point…",
                PointToPoint::Target(_) => "Pick a pad point or the datum…",
            };
            let style = if armed {
                Button::SmallSelected
            } else {
                Button::Small
            };
            if ui_kit::button(ui, label, style).clicked() {
                tab.point_to_point = if armed {
                    PointToPoint::Off
                } else {
                    PointToPoint::Source
                };
            }
            kit::hint(
                ui,
                "Drag the triad to move or turn the model",
                &format!(
                    "Drag the triad to move or turn the model; Esc cancels a drag. Over the view, arrow keys nudge {} mm (Shift ×10), Page Up/Down lift.",
                    model_place::NUDGE_MM
                ),
            );
        });
        kit::section(ui, "model-fit", "Fit leads to pads", true, None, |ui| {
            let options = [
                (false, "Free rotation".to_owned()),
                (true, "Quarter turns".to_owned()),
            ];
            kit::choice_row(ui, "Rotation", &mut tab.quarter, &options);
            let rotation = tab.rotation();
            let pairs = package_fit::form_pairs(&package, &footprint);
            let fit = match &pairs {
                Ok(pairs) => package_fit::fit_leads(&package.placement, pairs, rotation),
                Err(error) => Err(pcbcraft_core::Error::Invalid(error.to_string())),
            };
            match &fit {
                Ok(fit) => {
                    kit::value_row(ui, "Fitted error", &format!("{:.3} mm rms", fit.rms_mm));
                    if let Some(worst) = fit
                        .residuals
                        .iter()
                        .max_by(|a, b| a.error_mm.total_cmp(&b.error_mm))
                    {
                        let name = package
                            .terminal(worst.terminal)
                            .map_or_else(|| worst.terminal.to_string(), |t| t.designation.clone());
                        kit::value_row(
                            ui,
                            "Worst lead",
                            &format!("{name}: {:.3} mm", worst.error_mm),
                        );
                    }
                    if fit.mirrored_better {
                        kit::status(
                            ui,
                            "A mirrored pin order fits better: check the footprint's numbering.",
                            Tone::Warning,
                        );
                    }
                    // A residual no placement removes: the body and the
                    // lands disagree.
                    if fit.rms_mm > FIT_TOLERANCE_MM
                        && let Ok(pairs) = &pairs
                        && let Some((leads, pads)) =
                            package_fit::spacing_mismatch(pairs, FIT_TOLERANCE_MM)
                    {
                        kit::status(
                            ui,
                            &format!(
                                "Lead spacing {leads:.2} mm vs pad spacing {pads:.2} mm: placement cannot close this. Change the body, or use Lands from body."
                            ),
                            Tone::Warning,
                        );
                    }
                    let unchanged = fit.placement == package.placement;
                    kit::actions(ui, |ui| {
                        if kit::primary(ui, "Fit", !unchanged).clicked() {
                            tab.pending = Some((fit.placement, revision));
                            operation = Some(model_place::fit(draft, rotation));
                        }
                    });
                }
                Err(error) => {
                    let text = fit_message(&error.to_string());
                    kit::hint(ui, &text, &text);
                }
            }
        });
        // Placement checks (leads off their lands, body below the board)
        // are footprint findings: they show in Issues, not here.
        operation
    }
}

/// Plain guidance for a fit that cannot run, without the rule code.
fn fit_message(error: &str) -> String {
    let text = match error.split_once(": ") {
        Some((code, rest)) if code.contains('.') && !code.contains(' ') => rest,
        _ => error,
    };
    format!("Cannot fit: {text}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_messages_drop_the_rule_code() {
        assert_eq!(
            fit_message(
                "package.fit_no_leads: a Generic package has no typed lead contacts; pass lead pairs"
            ),
            "Cannot fit: a Generic package has no typed lead contacts; pass lead pairs."
        );
    }
}
