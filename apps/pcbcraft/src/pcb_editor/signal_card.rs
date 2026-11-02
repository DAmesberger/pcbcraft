//! The High speed card: differential pairs (with pairs suggested from net
//! names), length-matching groups and the length report (each routed,
//! paired or grouped net's length, tracks plus via barrels, against its
//! target). It opens from the Board card's High speed row.
//!
//! Every change is one typed `diff_pair.*` or `length_group.*` command and
//! one undo step; numbers commit when their field is left. The lengths
//! come from the core's `layout.length_report` (`layout::length::report`),
//! computed once per document revision.
use super::PcbEditor;
use crate::kit::{self, card::CardResponse};
use crate::refusal::{self, Origin};
use egui::Ui;
use pcbcraft_core::layout::length::{self, LengthReport};
use pcbcraft_core::rules::RulesCommand;
use pcbcraft_core::rules::signal::{
    self, DiffPairId, DiffPairSpec, LengthGroup, LengthGroupId, LengthGroupSpec,
};
use pcbcraft_core::{Engine, Id, Operation, Project};
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::collections::HashMap;

/// The open card's state.
#[derive(Debug, Default)]
pub struct SignalCard {
    /// The selected pair or group.
    pair: Option<DiffPairId>,
    group: Option<LengthGroupId>,
    /// A number being typed, by field.
    staged: HashMap<egui::Id, f64>,
    nets_all: bool,
    /// The length report and the revision it is of.
    report: Option<(u64, LengthReport)>,
}

fn mm(value: f64) -> String {
    super::impedance_view::mm(value)
}

impl PcbEditor {
    /// Open the High speed card.
    pub fn open_signals(&mut self) {
        self.setup = None;
        self.rules = None;
        self.net_classes = None;
        self.end_route();
        self.signals = Some(SignalCard::default());
    }

    /// The High speed card in the subject slot.
    pub(super) fn signals_card(&mut self, ui: &mut Ui, engine: &Engine) -> CardResponse<Operation> {
        let Some(mut card) = self.signals.take() else {
            return CardResponse::output(None);
        };
        let project = engine.project();
        if card.report.as_ref().map(|(r, _)| *r) != Some(engine.revision()) {
            card.report = Some((engine.revision(), length::report(project)));
        }
        let inspector = kit::card::Inspector {
            header: crate::shell::Header::new("trace", "High speed", "Rules"),
            subject: egui::Id::new("pcb-signals"),
            footer: true,
            issues: None,
        };
        let mut close = false;
        let mut lit = None;
        let response = kit::card::card(ui, &inspector, |ui, region| match region {
            kit::card::Region::Body => {
                let (operation, net) = body(ui, project, &mut card, self.highlighted);
                lit = net.or(lit);
                operation
            }
            kit::card::Region::Footer => {
                let mut operation = None;
                kit::actions(ui, |ui| {
                    close = kit::primary(ui, "Done", true).clicked();
                    if let Some(pair) = card.pair
                        && ui_kit::button(ui, "Delete pair", Button::Small).clicked()
                    {
                        refusal::mark(ui.ctx(), Origin::Command);
                        operation = Some(Operation::Rules(RulesCommand::DeleteDiffPair { pair }));
                        card.pair = None;
                    }
                    if let Some(group) = card.group
                        && ui_kit::button(ui, "Delete group", Button::Small).clicked()
                    {
                        refusal::mark(ui.ctx(), Origin::Command);
                        operation =
                            Some(Operation::Rules(RulesCommand::DeleteLengthGroup { group }));
                        card.group = None;
                    }
                });
                operation
            }
        });
        if let Some(net) = lit {
            self.net = Some(net);
            self.highlighted = Some(net);
        }
        if !close {
            self.signals = Some(card);
        }
        response
    }
}

/// Pairs, suggestions, groups and lengths. Returns an operation and a net
/// to light.
fn body(
    ui: &mut Ui,
    project: &Project,
    card: &mut SignalCard,
    highlighted: Option<Id>,
) -> (Option<Operation>, Option<Id>) {
    let mut operation = None;
    let mut lit = None;
    let rules = &project.rules;
    let report = card
        .report
        .as_ref()
        .map(|(_, r)| r.clone())
        .unwrap_or_default();
    kit::section(
        ui,
        "signal-pairs",
        "Differential pairs",
        true,
        Some(rules.diff_pairs.len()),
        |ui| {
            if rules.diff_pairs.is_empty() {
                kit::hint(
                    ui,
                    "No pairs: add one from the suggestions below",
                    "A pair routes its two nets together at the pair gap",
                );
            }
            kit::list_frame(
                ui,
                rules.diff_pairs.len(),
                rules.diff_pairs.len(),
                |ui, i| {
                    let Some(pair) = rules.diff_pairs.get(i) else {
                        return;
                    };
                    let row = report.pairs.iter().find(|r| r.pair == pair.id);
                    let secondary = format!(
                        "{} / {}{}",
                        project.net_name(pair.positive),
                        project.net_name(pair.negative),
                        row.map_or_else(String::new, |r| format!(" · skew {}", mm(r.skew)))
                    );
                    let issue = row
                        .is_some_and(|r| r.max_skew.is_some_and(|m| r.skew > m + 1e-6))
                        .then_some(kit::severity::Level::Warning);
                    let tree = kit::Row {
                        icon: "trace",
                        label: &pair.name,
                        secondary: &secondary,
                        selected: card.pair == Some(pair.id),
                        issue,
                        tip: "Select this pair and highlight its positive net",
                        ..Default::default()
                    };
                    if kit::tree_row(ui, ui.id().with(("pair", pair.id)), &tree).clicked {
                        card.pair = Some(pair.id);
                        card.group = None;
                        lit = Some(pair.positive);
                    }
                },
            );
        },
    );
    let suggested = signal::suggest(rules, project.nets.iter().map(|n| (n.id, n.name.as_str())));
    if !suggested.is_empty() {
        kit::section(
            ui,
            "signal-suggest",
            "Suggested pairs",
            true,
            Some(suggested.len()),
            |ui| {
                kit::list_frame(ui, suggested.len(), kit::LIST_MAX_ROWS, |ui, i| {
                    let Some(s) = suggested.get(i) else {
                        return;
                    };
                    let text = format!("{} + / − {}", s.positive_name, s.negative_name);
                    kit::row(ui, &s.name, |ui| {
                        if kit::summary_row(ui, &text, Some("Add")) {
                            refusal::mark(ui.ctx(), Origin::Command);
                            operation = Some(Operation::Rules(RulesCommand::AddDiffPair {
                                pair: DiffPairSpec {
                                    name: s.name.clone(),
                                    positive: s.positive,
                                    negative: s.negative,
                                },
                            }));
                        }
                    });
                });
            },
        );
    }
    let groups = &rules.length_groups;
    kit::section(
        ui,
        "signal-groups",
        "Length groups",
        true,
        Some(groups.len()),
        |ui| {
            kit::list_frame(ui, groups.len(), groups.len(), |ui, i| {
                let Some(group) = groups.get(i) else {
                    return;
                };
                let row = report.groups.iter().find(|r| r.group == group.id);
                let secondary = row.map_or_else(String::new, |r| {
                    format!(
                        "{} ± {} · {} nets",
                        mm(r.target),
                        mm(r.tolerance),
                        r.members.len()
                    )
                });
                let tree = kit::Row {
                    icon: "dimension",
                    label: &group.name,
                    secondary: &secondary,
                    selected: card.group == Some(group.id),
                    issue: row.filter(|r| !r.ok).map(|_| kit::severity::Level::Warning),
                    tip: "Edit this length group",
                    ..Default::default()
                };
                if kit::tree_row(ui, ui.id().with(("group", group.id)), &tree).clicked {
                    card.group = Some(group.id);
                    card.pair = None;
                }
            });
            kit::actions(ui, |ui| {
                let from = card
                    .pair
                    .map(|p| (vec![], vec![p]))
                    .or(highlighted.map(|n| (vec![n], vec![])));
                let label = if card.pair.is_some() {
                    "New group with the pair"
                } else {
                    "New group with the net"
                };
                if ui_kit::button(ui, label, Button::Enabled(from.is_some()))
                    .on_disabled_hover_text("Select a pair or highlight a net first")
                    .clicked()
                    && let Some((nets, pairs)) = from
                {
                    card.group = Some(LengthGroupId(rules.next_group));
                    refusal::mark(ui.ctx(), Origin::Command);
                    operation = Some(Operation::Rules(RulesCommand::AddLengthGroup {
                        group: LengthGroupSpec {
                            name: fresh_group(project),
                            nets,
                            pairs,
                            target: None,
                            tolerance: 0.5,
                        },
                    }));
                }
            });
        },
    );
    if let Some(group) = card.group.and_then(|g| rules.length_group(g)).cloned() {
        operation = group_section(ui, project, card, &group, highlighted).or(operation);
    }
    kit::section(
        ui,
        "signal-lengths",
        "Lengths",
        true,
        Some(report.nets.len()),
        |ui| {
            let max = if card.nets_all {
                report.nets.len()
            } else {
                kit::LIST_MAX_ROWS
            };
            let shown = kit::list_frame(ui, report.nets.len(), max, |ui, i| {
                let Some(row) = report.nets.get(i) else {
                    return;
                };
                let mut secondary = mm(row.length.total);
                if let (Some(target), Some(delta)) = (row.target, row.delta) {
                    secondary = format!("{secondary} · target {} ({:+.3})", mm(target), delta);
                }
                let tree = kit::Row {
                    icon: "trace",
                    label: &row.name,
                    secondary: &secondary,
                    issue: (row.ok == Some(false)).then_some(kit::severity::Level::Warning),
                    tip: "Highlight this net; lengths include via barrels",
                    ..Default::default()
                };
                if kit::tree_row(ui, ui.id().with(("length", row.net)), &tree).clicked {
                    lit = Some(row.net);
                }
            });
            if shown.show_all {
                card.nets_all = true;
            }
        },
    );
    (operation, lit)
}

/// "Group 1", "Group 2", ….
fn fresh_group(project: &Project) -> String {
    let taken = |name: &str| {
        project
            .rules
            .length_groups
            .iter()
            .any(|g| g.name.eq_ignore_ascii_case(name))
    };
    (1..=project.rules.length_groups.len() + 1)
        .map(|n| format!("Group {n}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| "New group".into())
}

/// A number field that commits when it is left: returns the new value.
fn committed_number(
    ui: &mut Ui,
    card: &mut SignalCard,
    label: &str,
    current: f64,
    range: std::ops::RangeInclusive<f64>,
) -> Option<f64> {
    let id = ui.id().with(("signal-number", label));
    let mut value = card.staged.get(&id).copied().unwrap_or(current);
    let input = kit::row(ui, label, |ui| {
        kit::number_input(ui, label, &mut value, "mm", range)
    });
    if let Some(message) = &input.invalid {
        kit::field_error(ui, &input.response, message, true);
    }
    if input.response.has_focus() {
        card.staged.insert(id, value);
        return None;
    }
    card.staged.remove(&id);
    ((value - current).abs() > 1e-12 && input.invalid.is_none()).then_some(value)
}

/// The selected group: target, tolerance and members.
fn group_section(
    ui: &mut Ui,
    project: &Project,
    card: &mut SignalCard,
    group: &LengthGroup,
    highlighted: Option<Id>,
) -> Option<Operation> {
    let mut edited: Option<LengthGroup> = None;
    kit::section(
        ui,
        ("signal-group", group.id),
        &group.name,
        true,
        None,
        |ui| {
            let mut matched = group.target.is_none();
            if kit::check_row(ui, "Match longest", &mut matched) {
                let target = card
                    .report
                    .as_ref()
                    .and_then(|(_, r)| r.groups.iter().find(|g| g.group == group.id))
                    .map_or(0.0, |g| g.target);
                edited = Some(LengthGroup {
                    target: (!matched).then_some(target),
                    ..group.clone()
                });
            }
            if let Some(target) = group.target
                && let Some(value) = committed_number(ui, card, "Target", target, 0.0..=10_000.0)
            {
                edited = Some(LengthGroup {
                    target: Some(value),
                    ..group.clone()
                });
            }
            if let Some(value) =
                committed_number(ui, card, "Tolerance", group.tolerance, 0.0..=10_000.0)
            {
                edited = Some(LengthGroup {
                    tolerance: value,
                    ..group.clone()
                });
            }
            let rules = &project.rules;
            for pair in group.pairs.iter().filter_map(|p| rules.diff_pair(*p)) {
                kit::row(ui, "Pair", |ui| {
                    if kit::summary_row(ui, &pair.name, Some("Remove")) {
                        edited = Some(LengthGroup {
                            pairs: group
                                .pairs
                                .iter()
                                .copied()
                                .filter(|p| *p != pair.id)
                                .collect(),
                            ..group.clone()
                        });
                    }
                });
            }
            for net in &group.nets {
                kit::row(ui, "Net", |ui| {
                    if kit::summary_row(ui, project.net_name(*net), Some("Remove")) {
                        edited = Some(LengthGroup {
                            nets: group.nets.iter().copied().filter(|n| n != net).collect(),
                            ..group.clone()
                        });
                    }
                });
            }
            if let Some(net) = highlighted
                .filter(|n| !group.member_nets(rules).contains(n) && project.net(*n).is_some())
            {
                kit::actions(ui, |ui| {
                    let label = format!("Add {}", project.net_name(net));
                    if ui_kit::button(ui, &label, Button::Small).clicked() {
                        let mut nets = group.nets.clone();
                        nets.push(net);
                        edited = Some(LengthGroup {
                            nets,
                            ..group.clone()
                        });
                    }
                });
            }
        },
    );
    let group = edited?;
    refusal::mark(ui.ctx(), Origin::Command);
    Some(Operation::Rules(RulesCommand::EditLengthGroup { group }))
}
