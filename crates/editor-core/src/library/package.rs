//! Package definitions: physical terminals and the parametric envelope.
use super::publish::{issue, object};
use super::{MAX_LABEL, MAX_PACKAGE_TERMINALS, printable};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{finite_within, size_valid};
use crate::ids::{DefinitionId, PackageTerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageDefinition {
    pub next_local: u32,
    pub terminals: Vec<PackageTerminal>,
    /// Envelope length, width and height in mm.
    pub body_size: [f64; 3],
    pub model_offset: [f64; 3],
    /// What the envelope is made of, which sets its 3D look.
    pub body: PackageBody,
}

/// The physical construction of a package body. Authored data: the 3D view
/// never guesses it from designators, names or values.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageBody {
    /// A body moulded in black epoxy (ICs, transistors, diodes, most
    /// through-hole parts); pads under it show a low lead band.
    #[default]
    Moulded,
    /// A two-terminal thick-film chip resistor: white alumina substrate with
    /// a black overcoat on top and tinned terminations wrapped over both
    /// ends of the long axis.
    ChipResistor,
    /// A two-terminal multilayer ceramic chip (capacitor class): tan ceramic
    /// with tinned terminations wrapped over both ends of the long axis.
    ChipCapacitor,
}

impl PackageBody {
    pub const ALL: [Self; 3] = [Self::Moulded, Self::ChipResistor, Self::ChipCapacitor];

    pub fn label(self) -> &'static str {
        match self {
            Self::Moulded => "Moulded",
            Self::ChipResistor => "Chip resistor",
            Self::ChipCapacitor => "Chip capacitor",
        }
    }

    /// A two-terminal chip with wrapped end terminations.
    pub fn is_chip(self) -> bool {
        !matches!(self, Self::Moulded)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageTerminal {
    pub id: PackageTerminalId,
    /// Printed or datasheet designation, e.g. `1` or `EP`. Never identity.
    pub designation: String,
}

impl PackageDefinition {
    pub fn terminal(&self, id: PackageTerminalId) -> Option<&PackageTerminal> {
        self.terminals.iter().find(|t| t.id == id)
    }

    /// A positive finite envelope with a finite model offset: the condition
    /// for a native envelope model.
    pub fn envelope_valid(&self) -> bool {
        size_valid(&self.body_size) && finite_within(&self.model_offset)
    }

    pub(super) fn within_bounds(&self) -> bool {
        self.terminals.len() <= MAX_PACKAGE_TERMINALS
            && self
                .terminals
                .iter()
                .all(|t| t.designation.len() <= MAX_LABEL)
    }

    pub(super) fn geometry_finite(&self) -> bool {
        finite_within(&self.body_size) && finite_within(&self.model_offset)
    }

    pub(super) fn local_ids(&self) -> Vec<u32> {
        self.terminals.iter().map(|t| t.id.0).collect()
    }

    pub(super) fn findings(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        if !(1..=MAX_PACKAGE_TERMINALS).contains(&self.terminals.len()) {
            out.push(issue(
                "package.terminal_count",
                "A package needs 1–512 physical terminals.".into(),
                vec![this],
            ));
        }
        let mut designations = BTreeSet::new();
        for terminal in &self.terminals {
            if !printable(&terminal.designation) || !designations.insert(&terminal.designation) {
                out.push(issue(
                    "package.designation",
                    format!(
                        "Package terminal {} needs a unique printable designation.",
                        terminal.id
                    ),
                    vec![object(id, terminal.id.0)],
                ));
            }
        }
        if self.body.is_chip() && self.terminals.len() != 2 {
            out.push(issue(
                "package.body",
                format!(
                    "A {} body has exactly two terminals.",
                    self.body.label().to_lowercase()
                ),
                vec![this],
            ));
        }
        if !self.envelope_valid() {
            out.push(issue(
                "package.envelope",
                "Package envelope dimensions must be finite and positive.".into(),
                vec![this],
            ));
        }
    }
}
