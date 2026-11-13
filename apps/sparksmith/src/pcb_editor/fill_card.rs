//! The Fill card of the selected fill: name, net, layer, region (the
//! board outline or drawn), priority and lock; its copper's state;
//! clearance and minimum width, pad and via connection styles, thermal reliefs, solid or hatched copper and island
//! removal, each following the design rules unless set here. Refill and
//! Delete fill are in the footer (docs/sidebar-ia.md, "Properties card by
//! subject"). Edits commit live as one `fill.edit` each: a choice or check
//! at once, typed text once no field has focus.
use super::{PcbEditor, copper_options, fill_paint::FillDisplay};
use crate::kit;
use crate::refusal::{self, Origin};
use egui::Ui;
use sparksmith_core::layout::fill::{FillCommand, RefillTarget};
use sparksmith_core::model::fill::{FillRegion, Hatch, IslandRemoval};
use sparksmith_core::rules::{PadConnection, ThermalRelief};
use sparksmith_core::{Fill, Id, Operation, Project};
use sparksmith_ui::ui_kit::{self, Button};

/// Why a locked fill's fields, Refill and Delete are disabled.
const LOCKED: &str = "Unlock the fill to change it";
/// Lengths a fill setting takes, mm.
const MM_RANGE: std::ops::RangeInclusive<f64> = 0.0..=10.0;
/// Hatch line widths and gaps, mm (above zero).
const HATCH_RANGE: std::ops::RangeInclusive<f64> = 0.01..=10.0;
const ANGLE_RANGE: std::ops::RangeInclusive<f64> = -360.0..=360.0;
const PRIORITY_RANGE: std::ops::RangeInclusive<f64> = 0.0..=1000.0;
const SPOKES_RANGE: std::ops::RangeInclusive<f64> = 1.0..=8.0;
const AREA_RANGE: std::ops::RangeInclusive<f64> = 0.0..=10_000.0;

/// The fill being edited in the card and the definition last sent.
pub(super) struct FillDraft {
    key: (u64, Id),
    fill: Fill,
    sent: Option<Fill>,
}

fn mm(v: f64) -> String {
    format!("{} mm", kit::number_text(v))
}

fn connection_name(c: PadConnection) -> &'static str {
    match c {
        PadConnection::Solid => "Solid",
        PadConnection::Thermal => "Thermal relief",
        PadConnection::None => "Not connected",
    }
}

/// A setting that follows the rules (`None`) or is set here.
fn rule_or_own(ui: &mut Ui, label: &str, own: &mut bool, rule: &str) -> bool {
    let options = [
        (false, format!("Rules · {rule}")),
        (true, "Set here".to_owned()),
    ];
    kit::choice_row(ui, label, own, &options)
}

impl PcbEditor {
    /// FILL, RULES, CONNECTIONS, STYLE and ISLANDS of `fill`.
    pub(super) fn fill_section(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        revision: u64,
        fill: &Fill,
    ) -> Option<Operation> {
        let key = (revision, fill.id);
        if self.fill_card.as_ref().is_none_or(|d| d.key != key) {
            self.fill_card = Some(FillDraft {
                key,
                fill: fill.clone(),
                sent: None,
            });
        }
        let draft = self.fill_card.as_mut()?;
        let mut d = draft.fill.clone();
        let locked = fill.locked;
        let rules = project.rules.effective().fill;
        kit::section(ui, "pcb-fill", "Fill", true, None, |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                kit::text_row(ui, "Name", &mut d.name);
                let nets = super::net_options(project, "No net");
                kit::choice_row(ui, "Net", &mut d.net, &nets);
                let layers = copper_options(&project.stackup.layers());
                kit::choice_row(ui, "Layer", &mut d.layer, &layers);
                let regions = [
                    (FillRegion::Board, "Board outline".to_owned()),
                    (FillRegion::Drawn, "Drawn".to_owned()),
                ];
                kit::choice_row(ui, "Region", &mut d.region, &regions);
                let mut priority = f64::from(d.priority);
                kit::number_row(ui, "Priority", &mut priority, "", PRIORITY_RANGE);
                d.priority = priority.round().clamp(0.0, 1000.0) as u32;
            })
            .response
            .on_disabled_hover_text(LOCKED);
            kit::value_row(ui, "Copper", &self.fill_state(fill));
            kit::check_row(ui, "Locked", &mut d.locked);
        });
        ui.add_enabled_ui(!locked, |ui| {
            kit::section(ui, "pcb-fill-rules", "Clearance", true, None, |ui| {
                let s = &mut d.settings;
                let mut own = s.clearance.is_some();
                rule_or_own(ui, "Clearance", &mut own, &mm(rules.clearance));
                s.clearance = own.then(|| s.clearance.unwrap_or(rules.clearance));
                if let Some(v) = &mut s.clearance {
                    kit::number_row(ui, "Gap", v, "mm", MM_RANGE);
                }
                let mut own = s.width_min.is_some();
                rule_or_own(ui, "Min width", &mut own, &mm(rules.width_min));
                s.width_min = own.then(|| s.width_min.unwrap_or(rules.width_min));
                if let Some(v) = &mut s.width_min {
                    kit::number_row(ui, "Width", v, "mm", MM_RANGE);
                }
            });
            kit::section(ui, "pcb-fill-connect", "Connections", true, None, |ui| {
                let options: Vec<(Option<PadConnection>, String)> = std::iter::once((
                    None,
                    format!("Rules · {}", connection_name(rules.connection)),
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
                let c = &mut d.settings.connections;
                kit::choice_row(ui, "SMD pads", &mut c.smd, &options);
                kit::choice_row(ui, "Through-hole", &mut c.through_hole, &options);
                kit::choice_row(ui, "Vias", &mut c.via, &options);
                let s = &mut d.settings;
                let mut own = s.thermal.is_some();
                let rule = format!(
                    "{} × {}",
                    rules.thermal.spoke_count,
                    mm(rules.thermal.spoke_width)
                );
                rule_or_own(ui, "Thermals", &mut own, &rule);
                s.thermal = own.then(|| s.thermal.unwrap_or(rules.thermal));
                if let Some(t) = &mut s.thermal {
                    thermal_rows(ui, t);
                }
            });
            kit::section(ui, "pcb-fill-style", "Style", true, None, |ui| {
                let s = &mut d.settings;
                let mut hatched = s.hatch.is_some();
                let options = [(false, "Solid".to_owned()), (true, "Hatched".to_owned())];
                kit::choice_row(ui, "Copper", &mut hatched, &options);
                s.hatch = hatched.then(|| {
                    s.hatch.unwrap_or(Hatch {
                        width: 0.25,
                        gap: 0.5,
                        angle_deg: 45.0,
                    })
                });
                if let Some(h) = &mut s.hatch {
                    kit::number_row(ui, "Line width", &mut h.width, "mm", HATCH_RANGE);
                    kit::number_row(ui, "Opening", &mut h.gap, "mm", HATCH_RANGE);
                    kit::number_row(ui, "Angle", &mut h.angle_deg, "°", ANGLE_RANGE);
                }
                island_rows(ui, &mut s.islands, rules.island_area_min);
            });
        })
        .response
        .on_disabled_hover_text(LOCKED);
        let draft = self.fill_card.as_mut()?;
        draft.fill = d;
        let typing = ui.memory(|m| m.focused()).is_some();
        if draft.fill == *fill || typing || draft.sent.as_ref() == Some(&draft.fill) {
            return None;
        }
        draft.sent = Some(draft.fill.clone());
        refusal::mark(ui.ctx(), Origin::Command);
        Some(Operation::Fill(FillCommand::Edit {
            fill: Box::new(draft.fill.clone()),
        }))
    }

    /// "3 regions", "3 regions · 1 island removed", "Out of date" or "Not
    /// filled": what the fill's copper is.
    pub(super) fn fill_state(&self, fill: &Fill) -> String {
        match &fill.computed {
            None => "Not filled · Refill computes it".to_owned(),
            Some(_) if self.stale_fills.contains(&fill.id) => {
                "Out of date · Refill updates it".to_owned()
            }
            Some(c) => {
                let regions = match c.regions.len() {
                    1 => "1 region".to_owned(),
                    n => format!("{n} regions"),
                };
                match c.removed_islands {
                    0 => regions,
                    1 => format!("{regions} · 1 island removed"),
                    n => format!("{regions} · {n} islands removed"),
                }
            }
        }
    }

    /// The view popover's Fills row: solid, hatched or outlines only.
    pub(super) fn fill_view_row(&mut self, ui: &mut Ui) {
        kit::choice_row(ui, "Fills", &mut self.fill_display, &FillDisplay::options());
    }
}

fn thermal_rows(ui: &mut Ui, t: &mut ThermalRelief) {
    kit::number_row(ui, "Spoke width", &mut t.spoke_width, "mm", MM_RANGE);
    kit::number_row(ui, "Relief gap", &mut t.gap, "mm", MM_RANGE);
    let mut count = f64::from(t.spoke_count);
    kit::number_row(ui, "Spokes", &mut count, "", SPOKES_RANGE);
    t.spoke_count = count.round().clamp(1.0, 8.0) as u32;
    kit::number_row(ui, "Spoke angle", &mut t.angle_deg, "°", ANGLE_RANGE);
}

fn island_rows(ui: &mut Ui, islands: &mut IslandRemoval, rule_area: f64) {
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        Never,
        Unconnected,
        Below,
    }
    let mut kind = match islands {
        IslandRemoval::Never => Kind::Never,
        IslandRemoval::Unconnected => Kind::Unconnected,
        IslandRemoval::BelowArea(_) => Kind::Below,
    };
    let options = [
        (Kind::Unconnected, "Unconnected".to_owned()),
        (Kind::Below, "Unconnected and small".to_owned()),
        (Kind::Never, "Keep all".to_owned()),
    ];
    kit::choice_row(ui, "Remove islands", &mut kind, &options);
    *islands = match (kind, *islands) {
        (Kind::Never, _) => IslandRemoval::Never,
        (Kind::Unconnected, _) => IslandRemoval::Unconnected,
        (Kind::Below, IslandRemoval::BelowArea(a)) => IslandRemoval::BelowArea(a),
        (Kind::Below, _) => IslandRemoval::BelowArea(rule_area.max(1.0)),
    };
    if let IslandRemoval::BelowArea(area) = islands {
        kit::number_row(ui, "Below", area, "mm²", AREA_RANGE);
    }
}

/// The fill footer: Refill (its copper now) and Delete fill, both
/// disabled while the fill is locked.
pub(super) fn fill_footer(ui: &mut Ui, fill: &Fill) -> Option<Operation> {
    let enabled = !fill.locked;
    let refill = ui_kit::button(ui, "Refill", Button::Enabled(enabled))
        .on_hover_text("Compute this fill's copper from the board as it is now")
        .on_disabled_hover_text(LOCKED)
        .clicked();
    let delete = ui_kit::button(ui, "Delete fill", Button::Enabled(enabled))
        .on_disabled_hover_text(LOCKED)
        .clicked();
    if !enabled {
        return None;
    }
    if refill {
        refusal::mark(ui.ctx(), Origin::Command);
        return Some(Operation::Fill(FillCommand::Refill {
            target: RefillTarget::One(fill.id),
        }));
    }
    delete.then_some(Operation::Fill(FillCommand::Delete { id: fill.id }))
}

#[cfg(test)]
impl FillDraft {
    /// Stage a priority as typing it would.
    pub(super) fn set_priority(&mut self, priority: u32) {
        self.fill.priority = priority;
    }
}
