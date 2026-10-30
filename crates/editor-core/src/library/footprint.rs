//! Footprint definitions: copper lands of one exact package revision.
use super::publish::{issue, object};
use super::{Library, MAX_PADS};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Point, finite_within, point_valid, size_valid};
use crate::ids::{DefinitionId, PackageRef, PackageTerminalId, PadId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FootprintDefinition {
    /// The package revision whose terminals the pads land.
    pub package: PackageRef,
    pub next_local: u32,
    pub pads: Vec<Pad>,
    /// The package datum's location relative to the footprint origin.
    pub datum_offset: Point,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    pub id: PadId,
    pub terminal: PackageTerminalId,
    pub position: Point,
    pub size: Point,
}

impl FootprintDefinition {
    pub fn pad(&self, id: PadId) -> Option<&Pad> {
        self.pads.iter().find(|p| p.id == id)
    }

    pub(super) fn within_bounds(&self) -> bool {
        self.pads.len() <= MAX_PADS
    }

    pub(super) fn geometry_finite(&self) -> bool {
        point_valid(self.datum_offset)
            && self
                .pads
                .iter()
                .all(|p| point_valid(p.position) && finite_within(&p.size))
    }

    pub(super) fn local_ids(&self) -> Vec<u32> {
        self.pads.iter().map(|p| p.id.0).collect()
    }

    pub(super) fn findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        if !(1..=MAX_PADS).contains(&self.pads.len()) {
            out.push(issue(
                "footprint.pad_count",
                "A footprint needs 1–1024 pads.".into(),
                vec![this],
            ));
        }
        for pad in &self.pads {
            if !size_valid(&pad.size) {
                out.push(issue(
                    "footprint.pad_size",
                    format!("Pad {} needs a positive finite size.", pad.id),
                    vec![object(id, pad.id.0)],
                ));
            }
        }
        let Some(package) = library.package(self.package) else {
            out.push(issue(
                "reference.missing",
                format!("Package {} is not in the project library.", self.package),
                vec![
                    this,
                    ObjectRef::Definition {
                        id: self.package.id,
                    },
                ],
            ));
            return;
        };
        let terminals: BTreeSet<PackageTerminalId> =
            package.definition.terminals.iter().map(|t| t.id).collect();
        let mut landed = BTreeSet::new();
        for pad in &self.pads {
            if terminals.contains(&pad.terminal) {
                landed.insert(pad.terminal);
            } else {
                out.push(issue(
                    "footprint.pad_terminal_unknown",
                    format!(
                        "Pad {} lands unknown package terminal {}.",
                        pad.id, pad.terminal
                    ),
                    vec![object(id, pad.id.0)],
                ));
            }
        }
        for terminal in terminals.difference(&landed) {
            out.push(issue(
                "footprint.terminal_without_pad",
                format!("Package terminal {terminal} has no pad."),
                vec![this, object(self.package.id, terminal.0)],
            ));
        }
    }
}
