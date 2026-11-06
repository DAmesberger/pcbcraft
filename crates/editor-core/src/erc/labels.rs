//! Label, hierarchy, power-port and wire-end rules
//! (docs/schematic-capture.md, "ERC").
//!
//! - `erc.wire_end_free`: a wire end attached to nothing.
//! - `erc.label_unconnected`: a label or power port with no wire on it.
//! - `erc.global_label_single`: a global label whose name no other global
//!   label or power port shares, so it joins nothing off its sheet.
//! - `erc.hierarchy_mismatch`: a sheet pin without a hierarchical label of
//!   its name on the child sheet, or a hierarchical label without a sheet
//!   pin of its name on the symbol that uses its sheet.
//! - `erc.label_conflict`: a net carrying two or more different global
//!   names (power ports and global labels), or two or more different local
//!   label names on one sheet. The net takes the first name by the naming
//!   rule; the others are probably a mistake.
//! - `erc.power_port_undriven`: a net with a power port and power inputs
//!   but nothing that can supply it (a power output, output, bidirectional
//!   or passive terminal). It replaces `erc.undriven_power_input` on such a
//!   net, so each cause is reported once.
use crate::diagnostics::{Finding, ObjectRef};
use crate::functions::{Direction, Resolution};
use crate::ids::{Id, SheetId};
use crate::model::{LabelKind, Project, WireEnd};
use crate::nets::{self, NameKind};
use std::collections::{BTreeMap, BTreeSet};

fn sheet_name(project: &Project, sheet: SheetId) -> String {
    project
        .sheet(sheet)
        .map_or_else(|| "a missing sheet".into(), |s| s.name.clone())
}

/// Nets carrying a power port.
pub(super) fn power_nets(project: &Project) -> BTreeSet<Id> {
    nets::name_sources(project)
        .into_iter()
        .filter(|(_, sources)| sources.iter().any(|s| s.kind == NameKind::Power))
        .map(|(net, _)| net)
        .collect()
}

pub(super) fn checks(
    project: &Project,
    resolutions: &BTreeMap<Id, Resolution>,
    out: &mut Vec<Finding>,
) {
    let wired: BTreeSet<WireEnd> = project
        .wires
        .iter()
        .flat_map(|w| [w.from, w.to])
        .filter(|e| e.terminal().is_none())
        .collect();
    for label in &project.labels {
        if !wired.contains(&WireEnd::label(label.id)) {
            out.push(Finding::new(
                "erc.label_unconnected",
                format!(
                    "{} {} on {} has no wire.",
                    capitalised(label.kind.noun()),
                    label.name,
                    sheet_name(project, label.sheet)
                ),
                vec![ObjectRef::Label { id: label.id }],
            ));
        }
    }
    // Global names and how many global labels and power ports carry each.
    let mut global: BTreeMap<String, (usize, bool)> = BTreeMap::new();
    for label in &project.labels {
        if label.kind.is_global() {
            let entry = global.entry(label.name.to_ascii_lowercase()).or_default();
            entry.0 += 1;
            entry.1 |= matches!(label.kind, LabelKind::Power { .. });
        }
    }
    for label in &project.labels {
        if matches!(label.kind, LabelKind::Global { .. })
            && global.get(&label.name.to_ascii_lowercase()) == Some(&(1, false))
        {
            out.push(Finding::new(
                "erc.global_label_single",
                format!(
                    "Global label {} on {} is used only once.",
                    label.name,
                    sheet_name(project, label.sheet)
                ),
                vec![ObjectRef::Label { id: label.id }],
            ));
        }
    }
    for wire in &project.wires {
        for at in [wire.from, wire.to]
            .into_iter()
            .filter_map(WireEnd::free_point)
        {
            out.push(Finding::new(
                "erc.wire_end_free",
                format!(
                    "Wire end at ({}, {}) mm on {} is not connected.",
                    at[0],
                    at[1],
                    sheet_name(project, wire.sheet)
                ),
                vec![ObjectRef::Wire { id: wire.id }],
            ));
        }
    }
    hierarchy(project, out);
    conflicts(project, out);
    power(project, resolutions, out);
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn hierarchy(project: &Project, out: &mut Vec<Finding>) {
    let mut ports: BTreeSet<(SheetId, String)> = BTreeSet::new();
    for label in &project.labels {
        if matches!(label.kind, LabelKind::Hierarchical { .. }) {
            ports.insert((label.sheet, label.name.to_ascii_lowercase()));
        }
    }
    for symbol in &project.sheet_symbols {
        let child = sheet_name(project, symbol.child);
        for pin in &symbol.pins {
            if !ports.contains(&(symbol.child, pin.name.to_ascii_lowercase())) {
                out.push(Finding::new(
                    "erc.hierarchy_mismatch",
                    format!(
                        "Sheet pin {} has no hierarchical label on {child}.",
                        pin.name
                    ),
                    vec![ObjectRef::SheetPin { id: pin.id }],
                ));
            }
        }
    }
    for label in &project.labels {
        if !matches!(label.kind, LabelKind::Hierarchical { .. }) {
            continue;
        }
        let pin = project.parent_symbol(label.sheet).and_then(|s| {
            s.pins
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(&label.name))
        });
        if pin.is_none() {
            out.push(Finding::new(
                "erc.hierarchy_mismatch",
                format!(
                    "Hierarchical label {} on {} has no sheet pin.",
                    label.name,
                    sheet_name(project, label.sheet)
                ),
                vec![ObjectRef::Label { id: label.id }],
            ));
        }
    }
}

fn conflicts(project: &Project, out: &mut Vec<Finding>) {
    for (net, sources) in nets::name_sources(project) {
        let mut global: BTreeMap<String, &str> = BTreeMap::new();
        let mut local: BTreeMap<SheetId, BTreeMap<String, &str>> = BTreeMap::new();
        for source in &sources {
            let lower = source.name.to_ascii_lowercase();
            match source.kind {
                NameKind::Power | NameKind::Global => {
                    global.entry(lower).or_insert(&source.name);
                }
                NameKind::Local => {
                    local
                        .entry(source.sheet)
                        .or_default()
                        .entry(lower)
                        .or_insert(&source.name);
                }
                NameKind::Hierarchical => {}
            }
        }
        let mut report = |names: Vec<&str>| {
            let mut objects = vec![ObjectRef::Net { id: net }];
            objects.extend(sources.iter().filter_map(|s| match s.end {
                WireEnd::Label(l) if names.iter().any(|n| n.eq_ignore_ascii_case(&s.name)) => {
                    Some(ObjectRef::Label { id: l.label })
                }
                _ => None,
            }));
            out.push(Finding::new(
                "erc.label_conflict",
                format!(
                    "{}: labels name it {}.",
                    project.net_name(net),
                    names.join(" and ")
                ),
                objects,
            ));
        };
        if global.len() >= 2 {
            report(global.values().copied().collect());
        }
        for names in local.values().filter(|n| n.len() >= 2) {
            report(names.values().copied().collect());
        }
    }
}

fn power(project: &Project, resolutions: &BTreeMap<Id, Resolution>, out: &mut Vec<Finding>) {
    for net_id in power_nets(project) {
        let Some(net) = project.net(net_id) else {
            continue;
        };
        let mut inputs = 0usize;
        let mut source = false;
        for endpoint in &net.terminals {
            let Some(active) = resolutions
                .get(&endpoint.instance)
                .and_then(|r| r.active(endpoint.terminal))
            else {
                continue;
            };
            match active.direction {
                Direction::PowerInput => inputs += 1,
                Direction::PowerOutput
                | Direction::Output
                | Direction::Bidirectional
                | Direction::Passive => source = true,
                Direction::Input => {}
            }
        }
        if inputs > 0 && !source {
            out.push(Finding::new(
                "erc.power_port_undriven",
                format!("{}: power inputs have no source on the net.", net.name),
                vec![ObjectRef::Net { id: net.id }],
            ));
        }
    }
}
