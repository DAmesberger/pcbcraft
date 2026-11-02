//! Net classes in the PCB editor: the Net classes card (classes with their
//! width, clearance and via, class-to-class clearances, members and the
//! name-pattern rules), the Class row of the Track and Via cards and the
//! schematic Net card, and the Route tool's class width and via.
//!
//! Every field commits live through one `net_class.*` command and one undo
//! step (docs/sidebar-ia.md, "The card commits live"). A class comes only
//! from an explicit assignment or a pattern the user wrote; nothing here
//! infers one.
use super::PcbEditor;
use crate::kit::{self, card::CardResponse};
use crate::refusal::{self, Origin};
use egui::Ui;
use pcbcraft_core::rules::netclass::{ClassBasis, NetClassReport, report};
use pcbcraft_core::rules::{
    NetClass, NetClassId, NetClassPattern, NetClassSpec, PadConnection, RulesCommand,
};
use pcbcraft_core::{Engine, Id, Operation, Project, layout};
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::collections::HashMap;

/// The open Net classes card's own state: the class it edits and the text
/// of the field being typed in.
#[derive(Debug, Default)]
pub struct NetClassesCard {
    selected: Option<NetClassId>,
    /// Typed text by field id while the field has focus.
    staged: HashMap<egui::Id, String>,
    /// The text last committed per field, so a refused commit is not
    /// resent every frame.
    sent: HashMap<egui::Id, String>,
    refused: Option<(egui::Id, String)>,
    members_all: bool,
    new_pattern: String,
}

/// The current net class of every net, by the core's report.
fn class_report(project: &Project) -> NetClassReport {
    report(
        &project.rules,
        project.nets.iter().map(|n| (n.id, n.name.as_str())),
    )
}

fn mm(value: f64) -> String {
    format!("{} mm", kit::number_text((value * 1000.0).round() / 1000.0))
}

/// A class name no other class has: "Class 2", "Class 3", ….
fn fresh_name(project: &Project) -> String {
    let taken = |name: &str| {
        project
            .rules
            .net_classes
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(name))
    };
    (1..=project.rules.net_classes.len() + 1)
        .map(|n| format!("Class {n}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| "New class".into())
}

/// What a Class row asked for.
#[derive(Debug, Default)]
pub struct ClassRow {
    pub operation: Option<Operation>,
    /// "Net classes…" was clicked.
    pub edit: bool,
}

/// The Class row of a net: a dropdown of its explicit class ("None" or
/// "By pattern: Power" when only a pattern assigns one) that commits
/// `net_class.assign`, and the class's routing values, with "Net classes…"
/// when `edit` offers the Net classes card.
pub fn class_row(ui: &mut Ui, project: &Project, net: Id, edit: bool) -> ClassRow {
    let mut out = ClassRow::default();
    let rules = &project.rules;
    let explicit = rules
        .assignments
        .iter()
        .find(|a| a.net == net)
        .map(|a| a.class);
    let name = project.net_name(net);
    let by_pattern = rules
        .patterns
        .iter()
        .find(|p| pcbcraft_core::rules::glob_matches(&p.pattern, name))
        .and_then(|p| rules.class(p.class));
    let none = match by_pattern {
        Some(class) => format!("By pattern: {}", class.name),
        None => "None".to_owned(),
    };
    let mut options = vec![(None, none)];
    options.extend(
        rules
            .net_classes
            .iter()
            .map(|c| (Some(c.id), c.name.clone())),
    );
    let mut chosen = explicit;
    let changed = kit::choice_row(ui, "Class", &mut chosen, &options);
    if changed && chosen != explicit {
        refusal::mark(ui.ctx(), Origin::Command);
        out.operation = Some(Operation::Rules(RulesCommand::AssignNets {
            nets: vec![net],
            class: chosen,
        }));
    }
    let class = layout::net_class(project, net);
    let width = rules.track_width(class).value;
    let (drill, pad) = rules.via_size(class).value;
    kit::row(ui, "", |ui| {
        out.edit = kit::summary_row(
            ui,
            &format!(
                "{} · via {}/{}",
                mm(width),
                kit::number_text(drill),
                kit::number_text(pad)
            ),
            edit.then_some("Net classes…"),
        );
    });
    out
}

/// One optional length field: empty means "use the rules". Returns the
/// committed value (`Some(None)` clears it).
fn optional_mm(
    ui: &mut Ui,
    card: &mut NetClassesCard,
    label: &str,
    value: Option<f64>,
    inherited: f64,
) -> Option<Option<f64>> {
    optional_value(
        ui,
        card,
        label,
        value,
        "mm",
        &mm(inherited),
        "Enter a length in mm, or leave it empty for the rules",
    )
}

/// One optional number in `unit` with `hint` shown while empty.
fn optional_value(
    ui: &mut Ui,
    card: &mut NetClassesCard,
    label: &str,
    value: Option<f64>,
    unit: &str,
    hint: &str,
    error: &'static str,
) -> Option<Option<f64>> {
    let current = value.map(kit::number_text).unwrap_or_default();
    let id = ui.id().with(("net-class-mm", label));
    let mut text = card
        .staged
        .get(&id)
        .cloned()
        .unwrap_or_else(|| current.clone());
    let parsed = |text: &str| -> Result<Option<f64>, &'static str> {
        if text.trim().is_empty() {
            return Ok(None);
        }
        match kit::parse_number(text, unit) {
            Some(v) if v.is_finite() && v >= 0.0 => Ok(Some(v)),
            _ => Err(error),
        }
    };
    let invalid = parsed(&text).err();
    let hint = hint.to_owned();
    let response = kit::row(ui, label, |ui| {
        kit::text_field(ui, label, &mut text, &hint, invalid.is_some())
    });
    live_field(ui, card, id, &response, text, current, invalid).and_then(|t| parsed(&t).ok())
}

/// Shared live-commit bookkeeping of one text field: keeps the typed text
/// while focused, reverts on Esc, and returns the text to commit when the
/// field is left with a changed, valid value that was not just sent.
fn live_field(
    ui: &mut Ui,
    card: &mut NetClassesCard,
    id: egui::Id,
    response: &egui::Response,
    text: String,
    current: String,
    invalid: Option<&str>,
) -> Option<String> {
    if let Some(message) = refusal::claim(ui.ctx(), Origin::Field(id)) {
        card.refused = Some((id, message));
    }
    let own = card.refused.as_ref().is_some_and(|(r, _)| *r == id);
    if response.changed() && own {
        card.refused = None;
    }
    if let Some(message) = invalid {
        kit::field_error(ui, response, message, true);
    } else if let Some((_, message)) = card.refused.as_ref().filter(|(r, _)| *r == id) {
        kit::field_error(ui, response, message, true);
    }
    if response.has_focus() {
        card.staged.insert(id, text);
        return None;
    }
    card.staged.remove(&id);
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if !response.lost_focus() || escape || invalid.is_some() || text == current {
        if text == current {
            card.sent.remove(&id);
        }
        return None;
    }
    if card.sent.get(&id) == Some(&text) {
        return None;
    }
    card.sent.insert(id, text.clone());
    refusal::mark(ui.ctx(), Origin::Field(id));
    Some(text)
}

impl PcbEditor {
    /// Open the Net classes card, on `class` when given.
    pub fn open_net_classes(&mut self, class: Option<NetClassId>) {
        self.setup = None;
        self.rules = None;
        self.signals = None;
        self.end_route();
        self.net_classes = Some(NetClassesCard {
            selected: class,
            ..Default::default()
        });
    }

    /// Whether the Net classes card is open.
    #[cfg(test)]
    pub fn net_classes_open(&self) -> bool {
        self.net_classes.is_some()
    }

    /// The Route tool's width follows the routed net's class: a new net,
    /// or a new class width for it, sets the width; a width typed by hand
    /// for the same net stays.
    pub(super) fn follow_class_width(&mut self, project: &Project) {
        let Some(net) = self.net.filter(|n| project.net(*n).is_some()) else {
            return;
        };
        // A class impedance target sets the width per layer.
        let width = layout::impedance::route_width_on(project, net, self.layer);
        let key = (net, width.to_bits());
        if self.width_from != Some(key) {
            self.width = width;
            self.width_from = Some(key);
        }
    }

    /// The (drill, pad) of the via the Via tool or a layer change places
    /// on the current net: its class's, else the effective defaults.
    pub(super) fn class_via_size(&self, project: &Project) -> (f64, f64) {
        let class = self
            .net
            .filter(|n| project.net(*n).is_some())
            .and_then(|n| layout::net_class(project, n));
        project.rules.via_size(class).value
    }

    /// The Net classes card in the subject slot.
    pub(super) fn net_classes_card(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> CardResponse<Operation> {
        let project = engine.project();
        let Some(mut card) = self.net_classes.take() else {
            return CardResponse::output(None);
        };
        if card
            .selected
            .is_some_and(|id| project.rules.class(id).is_none())
        {
            card.selected = None;
        }
        if card.selected.is_none() {
            card.selected = project.rules.net_classes.first().map(|c| c.id);
        }
        let inspector = kit::card::Inspector {
            header: crate::shell::Header::new("plane", "Net classes", "Rules"),
            subject: egui::Id::new("pcb-net-classes"),
            footer: true,
            issues: None,
        };
        let mut close = false;
        let mut highlight = None;
        let response = kit::card::card(ui, &inspector, |ui, region| match region {
            kit::card::Region::Body => {
                let (operation, lit) = net_classes_body(ui, project, &mut card);
                highlight = lit.or(highlight);
                operation
            }
            kit::card::Region::Footer => {
                let (operation, done) = net_classes_footer(ui, project, &card);
                close |= done;
                operation
            }
        });
        if let Some(net) = highlight {
            self.net = Some(net);
            self.highlighted = Some(net);
        }
        if !close {
            self.net_classes = Some(card);
        }
        response
    }
}

/// The card body: classes, the selected class, its clearances and
/// members, then the patterns. Returns an operation and a net to light.
fn net_classes_body(
    ui: &mut Ui,
    project: &Project,
    card: &mut NetClassesCard,
) -> (Option<Operation>, Option<Id>) {
    let report = class_report(project);
    let mut operation = None;
    let mut lit = None;
    kit::section(ui, "net-classes", "Classes", true, None, |ui| {
        if report.classes.is_empty() {
            kit::hint(
                ui,
                "No net classes: every net follows the design rules",
                "A net class gives its nets their own width, clearance and via",
            );
        }
        let shown = kit::list_frame(ui, report.classes.len(), report.classes.len(), |ui, i| {
            let Some(row) = report.classes.get(i) else {
                return;
            };
            let secondary = format!("{} · {} nets", mm(row.track_width.value), row.members);
            let tree = kit::Row {
                icon: "trace",
                label: &row.class.name,
                secondary: &secondary,
                selected: card.selected == Some(row.class.id),
                tip: "Edit this net class",
                ..Default::default()
            };
            if kit::tree_row(ui, ui.id().with(("net-class", row.class.id)), &tree).clicked {
                card.selected = Some(row.class.id);
            }
        });
        let _ = shown;
        kit::actions(ui, |ui| {
            if ui_kit::button(ui, "Add net class", Button::Small).clicked() {
                // The engine gives the new class the next identity.
                card.selected = Some(NetClassId(project.rules.next_class));
                refusal::mark(ui.ctx(), Origin::Command);
                operation = Some(Operation::Rules(RulesCommand::AddNetClass {
                    class: NetClassSpec {
                        name: fresh_name(project),
                        ..Default::default()
                    },
                }));
            }
        });
    });
    if let Some(class) = card
        .selected
        .and_then(|id| project.rules.class(id))
        .cloned()
    {
        operation = class_section(ui, project, card, &class).or(operation);
        operation = signal_section(ui, project, card, &class).or(operation);
        operation = clearance_section(ui, project, card, &class).or(operation);
        lit = members_section(ui, card, &report, &class);
    }
    operation = patterns_section(ui, project, card).or(operation);
    (operation, lit)
}

/// Sets one optional length of a class.
type SetMm = fn(&mut NetClass, Option<f64>);

/// The selected class's name, widths, clearance, via and fill connection.
fn class_section(
    ui: &mut Ui,
    project: &Project,
    card: &mut NetClassesCard,
    class: &NetClass,
) -> Option<Operation> {
    let effective = project.rules.effective();
    let mut edited: Option<NetClass> = None;
    kit::section(ui, "net-class", "Class", true, None, |ui| {
        let id = ui.id().with(("net-class-name", class.id));
        let mut text = card
            .staged
            .get(&id)
            .cloned()
            .unwrap_or_else(|| class.name.clone());
        let blank = text.trim().is_empty().then_some("A net class needs a name");
        let response = kit::row(ui, "Name", |ui| {
            kit::text_field(ui, "Name", &mut text, "", blank.is_some())
        });
        if let Some(name) = live_field(ui, card, id, &response, text, class.name.clone(), blank) {
            edited = Some(NetClass {
                name: name.trim().to_owned(),
                ..class.clone()
            });
        }
        let fields: [(&str, Option<f64>, f64, SetMm); 5] = [
            (
                "Width",
                class.track_width,
                effective.copper.track_width_default,
                |c, v| c.track_width = v,
            ),
            (
                "Min width",
                class.track_width_min,
                effective.copper.track_width_min,
                |c, v| c.track_width_min = v,
            ),
            (
                "Clearance",
                class.clearance,
                effective.copper.clearance.get(
                    pcbcraft_core::rules::CopperObject::Track,
                    pcbcraft_core::rules::CopperObject::Track,
                ),
                |c, v| c.clearance = v,
            ),
            (
                "Via drill",
                class.via_drill,
                effective.holes.via_drill_default,
                |c, v| c.via_drill = v,
            ),
            (
                "Via pad",
                class.via_diameter,
                effective.holes.via_diameter_default,
                |c, v| c.via_diameter = v,
            ),
        ];
        for (label, value, inherited, set) in fields {
            if let Some(value) = optional_mm(ui, card, label, value, inherited) {
                let mut next = class.clone();
                set(&mut next, value);
                edited = Some(next);
            }
        }
        let mut connection = class.fill_connection;
        let options = [
            (None, "Design rules".to_owned()),
            (Some(PadConnection::Thermal), "Thermal relief".to_owned()),
            (Some(PadConnection::Solid), "Solid".to_owned()),
            (Some(PadConnection::None), "Not connected".to_owned()),
        ];
        if kit::choice_row(ui, "Fill joins", &mut connection, &options)
            && connection != class.fill_connection
        {
            edited = Some(NetClass {
                fill_connection: connection,
                ..class.clone()
            });
        }
    });
    let class = edited?;
    Some(Operation::Rules(RulesCommand::EditNetClass { class }))
}

/// The selected class's high-speed rules: impedance targets, pair width,
/// gap and limits, then the width and gap the targets need on each
/// layer of the stackup (`layout::impedance::derive`).
fn signal_section(
    ui: &mut Ui,
    project: &Project,
    card: &mut NetClassesCard,
    class: &NetClass,
) -> Option<Operation> {
    let mut edited: Option<NetClass> = None;
    let s = &class.signal;
    let summary = match (s.impedance, s.diff_impedance) {
        (Some(z), Some(d)) => format!("{} Ω · {} Ω pair", kit::number_text(z), kit::number_text(d)),
        (Some(z), None) => format!("{} Ω", kit::number_text(z)),
        (None, Some(d)) => format!("{} Ω pair", kit::number_text(d)),
        (None, None) => "No impedance target".to_owned(),
    };
    kit::summary_section(
        ui,
        ("net-class-signal", class.id),
        "High speed",
        s.impedance.is_some() || s.diff_impedance.is_some(),
        &summary,
        |ui| {
            type Set = fn(&mut NetClass, Option<f64>);
            // (label, value, unit, hint while empty, error, setter)
            type Field = (
                &'static str,
                Option<f64>,
                &'static str,
                &'static str,
                &'static str,
                Set,
            );
            let rows: [Field; 4] = [
                (
                    "Impedance",
                    s.impedance,
                    "Ω",
                    "No target",
                    "Enter ohms (10–200), or leave it empty",
                    |c, v| c.signal.impedance = v,
                ),
                (
                    "Pair impedance",
                    s.diff_impedance,
                    "Ω",
                    "No target",
                    "Enter ohms (20–300), or leave it empty",
                    |c, v| c.signal.diff_impedance = v,
                ),
                (
                    "Tolerance",
                    s.impedance_tolerance,
                    "%",
                    "10 %",
                    "Enter a percentage, or leave it empty for 10 %",
                    |c, v| c.signal.impedance_tolerance = v,
                ),
                (
                    "Max skew",
                    s.pair_max_skew,
                    "mm",
                    "Not checked",
                    "Enter a length in mm, or leave it empty",
                    |c, v| c.signal.pair_max_skew = v,
                ),
            ];
            for (label, value, unit, hint, error, set) in rows {
                if let Some(value) = optional_value(ui, card, label, value, unit, hint, error) {
                    let mut next = class.clone();
                    set(&mut next, value);
                    edited = Some(next);
                }
            }
            let rows: [(&str, Option<f64>, &str, Set); 3] = [
                (
                    "Pair width",
                    class.diff_pair_width,
                    "From target",
                    |c, v| c.diff_pair_width = v,
                ),
                ("Pair gap", class.diff_pair_gap, "From target", |c, v| {
                    c.diff_pair_gap = v
                }),
                (
                    "Max uncoupled",
                    s.pair_max_uncoupled,
                    "Not checked",
                    |c, v| c.signal.pair_max_uncoupled = v,
                ),
            ];
            for (label, value, hint, set) in rows {
                if let Some(value) = optional_value(
                    ui,
                    card,
                    label,
                    value,
                    "mm",
                    hint,
                    "Enter a length in mm, or leave it empty",
                ) {
                    let mut next = class.clone();
                    set(&mut next, value);
                    edited = Some(next);
                }
            }
            if s.impedance.is_some() || s.diff_impedance.is_some() {
                super::impedance_view::class_layers(ui, project, class.id);
            }
        },
    );
    let class = edited?;
    Some(Operation::Rules(RulesCommand::EditNetClass { class }))
}

/// Class-to-class clearances of the selected class: one optional length
/// per class (itself included: two different nets of this class).
fn clearance_section(
    ui: &mut Ui,
    project: &Project,
    card: &mut NetClassesCard,
    class: &NetClass,
) -> Option<Operation> {
    let mut operation = None;
    let base = project.rules.clearance(
        pcbcraft_core::rules::ClearanceSide {
            object: pcbcraft_core::rules::CopperObject::Track,
            class: Some(class.id),
        },
        pcbcraft_core::rules::ClearanceSide {
            object: pcbcraft_core::rules::CopperObject::Track,
            class: None,
        },
    );
    kit::section(
        ui,
        "net-class-pairs",
        "Clearance to class",
        true,
        None,
        |ui| {
            for other in &project.rules.net_classes {
                let pair = project
                    .rules
                    .class_clearances
                    .iter()
                    .find(|p| p.is(class.id, other.id))
                    .map(|p| p.mm);
                let label = format!("To {}", other.name);
                if let Some(mm) = ui
                    .push_id(("pair", other.id), |ui| {
                        optional_mm(ui, card, &label, pair, base.value)
                    })
                    .inner
                {
                    operation = Some(Operation::Rules(RulesCommand::SetClassClearance {
                        a: class.id,
                        b: other.id,
                        mm,
                    }));
                }
            }
        },
    );
    operation
}

/// The nets of the selected class, explicit or by pattern; a click lights
/// the net.
fn members_section(
    ui: &mut Ui,
    card: &mut NetClassesCard,
    report: &NetClassReport,
    class: &NetClass,
) -> Option<Id> {
    let members: Vec<_> = report
        .nets
        .iter()
        .filter(|m| m.class == Some(class.id))
        .collect();
    let mut lit = None;
    kit::section(ui, "net-class-members", "Nets", true, None, |ui| {
        kit::value_row(ui, "Members", &format!("{} nets", members.len()));
        let max = if card.members_all {
            members.len()
        } else {
            kit::LIST_MAX_ROWS
        };
        let shown = kit::list_frame(ui, members.len(), max, |ui, i| {
            let Some(member) = members.get(i) else {
                return;
            };
            let basis = match &member.basis {
                Some(ClassBasis::Pattern { pattern }) => format!("pattern {pattern}"),
                _ => "assigned".to_owned(),
            };
            let row = kit::Row {
                icon: "trace",
                label: &member.name,
                secondary: &basis,
                tip: "Highlight this net",
                ..Default::default()
            };
            if kit::tree_row(ui, ui.id().with(("member", member.net)), &row).clicked {
                lit = Some(member.net);
            }
        });
        if shown.show_all {
            card.members_all = true;
        }
    });
    lit
}

/// The ordered name patterns: each row's pattern, class and Remove; a new
/// pattern goes to the end. The first match assigns; an explicit class
/// wins over every pattern.
fn patterns_section(
    ui: &mut Ui,
    project: &Project,
    card: &mut NetClassesCard,
) -> Option<Operation> {
    let patterns = &project.rules.patterns;
    let classes: Vec<(NetClassId, String)> = project
        .rules
        .net_classes
        .iter()
        .map(|c| (c.id, c.name.clone()))
        .collect();
    let mut next: Option<Vec<NetClassPattern>> = None;
    kit::section(
        ui,
        "net-class-patterns",
        "Name patterns",
        true,
        None,
        |ui| {
            kit::hint(
                ui,
                "First match assigns; an explicit class wins",
                "* matches any run of characters and ? one character, case-sensitive",
            );
            for (index, pattern) in patterns.iter().enumerate() {
                ui.push_id(("pattern", index), |ui| {
                    let mut class = pattern.class;
                    if kit::choice_row(ui, &pattern.pattern, &mut class, &classes) {
                        let mut all = patterns.clone();
                        if let Some(p) = all.get_mut(index) {
                            p.class = class;
                        }
                        next = Some(all);
                    }
                    kit::row(ui, "", |ui| {
                        if ui_kit::button(ui, "Remove pattern", Button::Small).clicked() {
                            let mut all = patterns.clone();
                            all.remove(index);
                            next = Some(all);
                        }
                    });
                });
            }
            let target = card.selected.or_else(|| classes.first().map(|(id, _)| *id));
            let response = kit::row(ui, "New", |ui| {
                kit::text_field(ui, "new-pattern", &mut card.new_pattern, "e.g. +*V", false)
            });
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let mut add = false;
            kit::row(ui, "", |ui| {
                let label = match target.and_then(|id| project.rules.class(id)) {
                    Some(class) => format!("Add pattern for {}", class.name),
                    None => "Add a net class first".to_owned(),
                };
                let enabled = target.is_some() && !card.new_pattern.trim().is_empty();
                add = ui
                    .add_enabled_ui(enabled, |ui| {
                        ui_kit::button(ui, &label, Button::Small).clicked()
                    })
                    .inner;
            });
            if let Some(class) = target
                && (add || enter)
                && !card.new_pattern.trim().is_empty()
            {
                let mut all = patterns.clone();
                all.push(NetClassPattern {
                    pattern: card.new_pattern.trim().to_owned(),
                    class,
                });
                card.new_pattern.clear();
                next = Some(all);
            }
        },
    );
    let patterns = next?;
    refusal::mark(ui.ctx(), Origin::Command);
    Some(Operation::Rules(RulesCommand::SetPatterns { patterns }))
}

/// The pinned footer: Delete the selected class, and Done.
fn net_classes_footer(
    ui: &mut Ui,
    project: &Project,
    card: &NetClassesCard,
) -> (Option<Operation>, bool) {
    let mut operation = None;
    let mut done = false;
    kit::actions(ui, |ui| {
        if let Some(class) = card.selected.and_then(|id| project.rules.class(id))
            && ui_kit::button(ui, "Delete class", Button::Small)
                .on_hover_text("Delete this class with its assignments and patterns")
                .clicked()
        {
            refusal::mark(ui.ctx(), Origin::Command);
            operation = Some(Operation::Rules(RulesCommand::DeleteNetClass {
                class: class.id,
            }));
        }
        done = kit::primary(ui, "Done", true).clicked();
    });
    (operation, done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::layout::LayoutCommand;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn power(engine: &mut Engine) -> Result<NetClassId, Box<dyn std::error::Error>> {
        let outcome = engine.apply(
            engine.revision(),
            Operation::Rules(RulesCommand::AddNetClass {
                class: NetClassSpec {
                    name: "Power".into(),
                    track_width: Some(0.5),
                    via_drill: Some(0.4),
                    via_diameter: Some(0.8),
                    ..Default::default()
                },
            }),
        )?;
        match outcome {
            pcbcraft_core::Outcome::NetClassAdded { class } => Ok(class),
            other => Err(format!("unexpected {other:?}").into()),
        }
    }

    fn net(engine: &Engine) -> Result<Id, Box<dyn std::error::Error>> {
        Ok(engine.project().nets.first().ok_or("no net")?.id)
    }

    #[test]
    fn the_route_tool_takes_the_class_width_and_keeps_a_typed_one() -> TestResult {
        let mut engine = Engine::demo()?;
        let class = power(&mut engine)?;
        let net = net(&engine)?;
        let mut editor = PcbEditor {
            net: Some(net),
            ..Default::default()
        };
        editor.follow_class_width(engine.project());
        assert_eq!(editor.width, 0.3);
        engine.apply(
            engine.revision(),
            Operation::Rules(RulesCommand::AssignNets {
                nets: vec![net],
                class: Some(class),
            }),
        )?;
        editor.follow_class_width(engine.project());
        assert_eq!(editor.width, 0.5);
        assert_eq!(editor.class_via_size(engine.project()), (0.4, 0.8));
        editor.width = 0.7;
        editor.follow_class_width(engine.project());
        assert_eq!(editor.width, 0.7, "a typed width stays for the same net");
        Ok(())
    }

    #[test]
    fn the_via_tool_places_the_class_via() -> TestResult {
        let mut engine = Engine::demo()?;
        let class = power(&mut engine)?;
        let net = net(&engine)?;
        engine.apply(
            engine.revision(),
            Operation::Rules(RulesCommand::AssignNets {
                nets: vec![net],
                class: Some(class),
            }),
        )?;
        let mut editor = PcbEditor {
            net: Some(net),
            ..Default::default()
        };
        let operation = editor
            .place_via(engine.project(), [3.0, 3.0], 10.0)
            .ok_or("no via")?;
        let Operation::Layout(LayoutCommand::AddVia { via }) = operation else {
            return Err("not a via".into());
        };
        assert_eq!((via.drill, via.diameter, via.tented), (0.4, 0.8, true));
        Ok(())
    }

    #[test]
    fn opening_the_card_makes_it_the_subject_and_done_closes_it() -> TestResult {
        let mut engine = Engine::demo()?;
        power(&mut engine)?;
        let mut editor = PcbEditor::default();
        editor.open_net_classes(None);
        assert_eq!(editor.subject(), kit::card::Subject::Dialog);
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let _ = editor.subject_slot(ui, &engine);
        });
        output.textures_delta.clear();
        assert!(editor.net_classes_open());
        assert_eq!(
            editor.net_classes.as_ref().and_then(|c| c.selected),
            engine.project().rules.net_classes.first().map(|c| c.id)
        );
        editor.net_classes = None;
        assert_eq!(editor.subject(), kit::card::Subject::Properties);
        Ok(())
    }

    #[test]
    fn fresh_class_names_do_not_collide() -> TestResult {
        let mut engine = Engine::demo()?;
        assert_eq!(fresh_name(engine.project()), "Class 1");
        engine.apply(
            engine.revision(),
            Operation::Rules(RulesCommand::AddNetClass {
                class: NetClassSpec {
                    name: "class 1".into(),
                    ..Default::default()
                },
            }),
        )?;
        assert_eq!(fresh_name(engine.project()), "Class 2");
        Ok(())
    }
}
