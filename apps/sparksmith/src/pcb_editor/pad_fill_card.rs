//! The Fill connection section of the Placement card: how copper fills
//! join the selected part's pads, over each fill's own settings. "Applies
//! to" chooses the whole part or one pad (a pad's own setting wins over
//! the part's); each row follows the fill unless set here. Edits commit
//! live as one `fill.set_pad_override` each: a choice at once, typed text
//! once no field has focus.
use super::PcbEditor;
use crate::kit;
use crate::refusal::{self, Origin};
use egui::Ui;
use sparksmith_core::layout::fill::FillCommand;
use sparksmith_core::model::fill::PadFillOverride;
use sparksmith_core::rules::PadConnection;
use sparksmith_core::units::{Angle, Nm};
use sparksmith_core::{Id, Operation, PadId, Placement, Project};

/// Lengths an override takes, mm.
const MM_RANGE: std::ops::RangeInclusive<f64> = 0.0..=10.0;
/// Spoke widths, mm (above zero).
const SPOKE_RANGE: std::ops::RangeInclusive<f64> = 0.01..=10.0;
const ANGLE_RANGE: std::ops::RangeInclusive<f64> = -360.0..=360.0;

/// The override being edited and the one last sent, for one part and pad
/// (`None`: the whole part) at one revision.
pub(super) struct PadFillDraft {
    key: (u64, Id, Option<PadId>),
    settings: PadFillOverride,
    sent: Option<PadFillOverride>,
}

fn connection_name(c: PadConnection) -> &'static str {
    match c {
        PadConnection::Solid => "Solid",
        PadConnection::Thermal => "Thermal relief",
        PadConnection::None => "Not connected",
    }
}

/// A length or angle that follows the fill (`None`) or is set here.
fn optional_row(
    ui: &mut Ui,
    label: &str,
    value: &mut Option<f64>,
    start: f64,
    unit: &str,
    range: std::ops::RangeInclusive<f64>,
    follows: &str,
) {
    let mut own = value.is_some();
    let options = [(false, follows.to_owned()), (true, "Set here".to_owned())];
    kit::choice_row(ui, label, &mut own, &options);
    *value = own.then(|| value.unwrap_or(start));
    if let Some(v) = value {
        kit::number_row(ui, "Value", v, unit, range);
    }
}

impl PcbEditor {
    /// FILL CONNECTION of the placed part `placement`.
    pub(super) fn pad_fill_section(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        placement: &Placement,
    ) -> Option<Operation> {
        let instance = placement.instance;
        let pads: Vec<(PadId, String)> = self
            .board_pads
            .iter()
            .map(|b| &b.placed)
            .filter(|p| p.instance == instance)
            .map(|p| {
                let name = if p.pad.number.is_empty() {
                    format!("Pad #{}", p.pad.id)
                } else {
                    format!("Pad {}", p.pad.number)
                };
                (p.pad.id, name)
            })
            .collect();
        if pads.is_empty() {
            return None;
        }
        if self
            .pad_fill_target
            .is_some_and(|(i, pad)| i != instance || !pads.iter().any(|(p, _)| *p == pad))
        {
            self.pad_fill_target = None;
        }
        let mut target = self.pad_fill_target.map(|(_, pad)| pad);
        let stored = match target {
            None => placement.fill_override,
            Some(pad) => placement
                .pad_fill_overrides
                .iter()
                .find(|e| e.pad == pad)
                .map(|e| e.settings)
                .unwrap_or_default(),
        };
        let key = (self.cached_revision.unwrap_or(0), instance, target);
        if self.pad_fill.as_ref().is_none_or(|d| d.key != key) {
            self.pad_fill = Some(PadFillDraft {
                key,
                settings: stored,
                sent: None,
            });
        }
        let mut d = self.pad_fill.as_ref()?.settings;
        let rules = project.rules.effective().fill;
        // What an unset row follows: the part's own setting on a pad, else
        // each fill's.
        let under = match target {
            Some(_) => placement.fill_override,
            None => PadFillOverride::default(),
        };
        let follows = |set: Option<String>| match set {
            Some(v) => format!("Part · {v}"),
            None => "Each fill".to_owned(),
        };
        let mm = |v: f64| format!("{} mm", kit::number_text(v));
        kit::section(ui, "pcb-pad-fill", "Fill connection", false, None, |ui| {
            let options: Vec<(Option<PadId>, String)> =
                std::iter::once((None, "Whole part".to_owned()))
                    .chain(pads.iter().map(|(id, name)| (Some(*id), name.clone())))
                    .collect();
            kit::choice_row(ui, "Applies to", &mut target, &options);
            let connections: Vec<(Option<PadConnection>, String)> = std::iter::once((
                None,
                follows(under.connection.map(|c| connection_name(c).to_owned())),
            ))
            .chain(
                [
                    PadConnection::Thermal,
                    PadConnection::Solid,
                    PadConnection::None,
                ]
                .map(|c| (Some(c), connection_name(c).to_owned())),
            )
            .collect();
            kit::choice_row(ui, "Connection", &mut d.connection, &connections);
            let length = |ui: &mut Ui,
                          label: &str,
                          v: &mut Option<Nm>,
                          under: Option<Nm>,
                          rule: f64,
                          range| {
                let under = under.map(Nm::to_mm_f64);
                let mut mm_value = v.map(Nm::to_mm_f64);
                optional_row(
                    ui,
                    label,
                    &mut mm_value,
                    under.unwrap_or(rule),
                    "mm",
                    range,
                    &follows(under.map(mm)),
                );
                *v = mm_value.and_then(|x| Nm::from_mm_f64_rounded(x).ok());
            };
            length(
                ui,
                "Spoke width",
                &mut d.spoke_width,
                under.spoke_width,
                rules.thermal.spoke_width,
                SPOKE_RANGE,
            );
            let under_angle = under.spoke_angle.map(Angle::to_degrees_f64);
            let mut angle = d.spoke_angle.map(Angle::to_degrees_f64);
            optional_row(
                ui,
                "Spoke angle",
                &mut angle,
                under_angle.unwrap_or(rules.thermal.angle_deg),
                "°",
                ANGLE_RANGE,
                &follows(under_angle.map(|a| format!("{}°", kit::number_text(a)))),
            );
            d.spoke_angle = angle.and_then(|a| Angle::from_degrees_f64_rounded(a).ok());
            length(
                ui,
                "Relief gap",
                &mut d.thermal_gap,
                under.thermal_gap,
                rules.thermal.gap,
                MM_RANGE,
            );
            length(
                ui,
                "Clearance",
                &mut d.clearance,
                under.clearance,
                rules.clearance,
                MM_RANGE,
            );
        });
        if target != self.pad_fill_target.map(|(_, pad)| pad) {
            // Another pad (or the whole part): its own draft next frame.
            self.pad_fill_target = target.map(|pad| (instance, pad));
            return None;
        }
        let draft = self.pad_fill.as_mut()?;
        draft.settings = d;
        let typing = ui.memory(|m| m.focused()).is_some();
        if d == stored || typing || draft.sent == Some(d) {
            return None;
        }
        draft.sent = Some(d);
        refusal::mark(ui.ctx(), Origin::Command);
        Some(Operation::Fill(FillCommand::SetPadOverride {
            instance,
            pad: target,
            settings: d,
        }))
    }
}
