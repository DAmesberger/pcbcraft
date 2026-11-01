//! Slice C2 (contract C0): instance-side pin mutations.
//! `instance.assign_peripheral` compiles to one `select_functions` batch;
//! `instance.set_pin_config` validates against the terminals' declared
//! options. Both are one undo step and leave the project unchanged on any
//! failure (the engine stages on a candidate).
use crate::diagnostics::{Error, Result};
use crate::functions::Resolution;
use crate::functions::pin_config::PinConfigEdit;
use crate::ids::{Id, ResourceId, RoleId, TerminalId};
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One role placement of `instance.assign_peripheral`: `None` lets the
/// solver choose the terminal.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RolePlacement {
    pub role: RoleId,
    pub terminal: Option<TerminalId>,
}

/// The `instance.assign_peripheral` request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssignPeripheral {
    pub instance: Id,
    pub peripheral: ResourceId,
    #[serde(default)]
    pub roles: Vec<RolePlacement>,
    /// `false` clears every function of the peripheral.
    pub enable: bool,
}

/// Contract stub until C2.
pub fn assign_peripheral(
    _project: &mut Project,
    _request: &AssignPeripheral,
) -> Result<Resolution> {
    Err(unsupported())
}

/// Contract stub until C2.
pub fn set_pin_config(
    _project: &mut Project,
    _instance: Id,
    _edits: &[PinConfigEdit],
) -> Result<()> {
    Err(unsupported())
}

fn unsupported() -> Error {
    Error::Unsupported(
        "pin_config.unsupported: peripheral assignment and pin configuration are not available yet (slice C2)"
            .into(),
    )
}
