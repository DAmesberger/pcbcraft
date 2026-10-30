//! Symbol definitions: units, each a drawable body with terminal anchors. A
//! symbol belongs to one component lineage, whose terminal IDs its anchors use.
use super::publish::{issue, object};
use super::{Library, MAX_LABEL, MAX_TERMINALS, MAX_UNITS, printable};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Point, finite_within, point_valid, size_valid};
use crate::ids::{DefinitionId, TerminalId, UnitId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SymbolDefinition {
    /// The owning component lineage; anchors use its terminal IDs.
    pub component: DefinitionId,
    pub next_local: u32,
    pub units: Vec<Unit>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub id: UnitId,
    /// Presentation label appended to the designator, e.g. `A` in `Q7A`.
    pub label: String,
    pub body_size: Point,
    pub anchors: Vec<Anchor>,
}

/// Where a terminal attaches, relative to its unit's origin.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub terminal: TerminalId,
    pub position: Point,
}

impl SymbolDefinition {
    pub fn unit(&self, id: UnitId) -> Option<&Unit> {
        self.units.iter().find(|u| u.id == id)
    }
    /// The unit drawing `terminal` and its anchor.
    pub fn anchor(&self, terminal: TerminalId) -> Option<(&Unit, &Anchor)> {
        self.units.iter().find_map(|unit| {
            unit.anchors
                .iter()
                .find(|a| a.terminal == terminal)
                .map(|anchor| (unit, anchor))
        })
    }

    pub(super) fn within_bounds(&self) -> bool {
        self.units.len() <= MAX_UNITS
            && self.units.iter().map(|u| u.anchors.len()).sum::<usize>() <= MAX_TERMINALS
            && self.units.iter().all(|u| u.label.len() <= MAX_LABEL)
    }

    pub(super) fn geometry_finite(&self) -> bool {
        self.units.iter().all(|u| {
            finite_within(&u.body_size) && u.anchors.iter().all(|a| point_valid(a.position))
        })
    }

    pub(super) fn local_ids(&self) -> Vec<u32> {
        self.units.iter().map(|u| u.id.0).collect()
    }

    pub(super) fn findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        // The owner is a component lineage that is published, or staged in
        // the same publish set; never unset, unknown or this lineage itself.
        if !matches!(
            library.latest(self.component),
            Some((crate::ids::DefinitionKind::Component, _))
        ) || self.component == id
        {
            out.push(issue(
                "reference.owner_mismatch",
                format!(
                    "Owner {} is not a published or co-published component lineage.",
                    self.component
                ),
                vec![this],
            ));
        }
        if !(1..=MAX_UNITS).contains(&self.units.len()) {
            out.push(issue(
                "symbol.unit_count",
                "A symbol needs 1–64 units.".into(),
                vec![this],
            ));
        }
        let mut labels = BTreeSet::new();
        let mut anchored = BTreeSet::new();
        for unit in &self.units {
            let unit_object = object(id, unit.id.0);
            if !printable(&unit.label) || !labels.insert(unit.label.to_lowercase()) {
                out.push(issue(
                    "symbol.unit_label",
                    format!("Unit {} needs a unique printable label.", unit.id),
                    vec![unit_object],
                ));
            }
            if !size_valid(&unit.body_size) {
                out.push(issue(
                    "symbol.body",
                    format!("Unit {} needs a positive body size.", unit.id),
                    vec![unit_object],
                ));
            }
            for anchor in &unit.anchors {
                if anchor.terminal.0 == 0 || !anchored.insert(anchor.terminal) {
                    out.push(issue(
                        "symbol.anchor_duplicate",
                        format!(
                            "Terminal {} is anchored more than once or has ID zero.",
                            anchor.terminal
                        ),
                        vec![unit_object],
                    ));
                }
            }
        }
    }
}
