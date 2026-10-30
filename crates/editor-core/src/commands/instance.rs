use super::Outcome;
use crate::diagnostics::Result;
use crate::ids::{Id, TerminalId, UnitId};
use crate::instances::{self, NewInstance};
use crate::model::{EditPolicy, PhysicalChoice, Project};
use crate::updates::{self, InstanceChange};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum InstanceCommand {
    /// Create one instance and the occurrence of its first unit.
    #[serde(rename = "component.place")]
    Place(NewInstance),
    /// Direct only without wires or a placement; otherwise preview
    /// `Change::Instance { Remove }`.
    #[serde(rename = "component.remove")]
    Remove { instance: Id },
    #[serde(rename = "instance.set_designator")]
    SetDesignator { instance: Id, designator: String },
    /// Free-text value until typed fields replace it.
    #[serde(rename = "instance.set_value")]
    SetValue { instance: Id, value: String },
    #[serde(rename = "instance.set_unit_unused")]
    SetUnitUnused {
        instance: Id,
        unit: UnitId,
        unused: bool,
    },
    #[serde(rename = "instance.set_no_connect")]
    SetNoConnect {
        instance: Id,
        terminals: Vec<TerminalId>,
        no_connect: bool,
    },
    /// Direct only without a board placement; otherwise preview
    /// `Change::Instance { SetPhysical }`.
    #[serde(rename = "instance.set_physical")]
    SetPhysical {
        instance: Id,
        choice: Option<PhysicalChoice>,
    },
}

pub(crate) fn apply_instance(project: &mut Project, command: InstanceCommand) -> Result<Outcome> {
    match command {
        InstanceCommand::Place(request) => {
            let (instance, occurrence) = instances::place(project, request)?;
            return Ok(Outcome::InstancePlaced {
                instance,
                occurrence,
            });
        }
        InstanceCommand::Remove { instance } => updates::stage(
            project,
            &InstanceChange::Remove { instance },
            EditPolicy::Direct,
        )?,
        InstanceCommand::SetDesignator {
            instance,
            designator,
        } => instances::set_designator(project, instance, designator)?,
        InstanceCommand::SetValue { instance, value } => {
            instances::set_value(project, instance, value)?
        }
        InstanceCommand::SetUnitUnused {
            instance,
            unit,
            unused,
        } => instances::set_unit_unused(project, instance, unit, unused)?,
        InstanceCommand::SetNoConnect {
            instance,
            terminals,
            no_connect,
        } => instances::set_no_connect(project, instance, &terminals, no_connect)?,
        InstanceCommand::SetPhysical { instance, choice } => updates::stage(
            project,
            &InstanceChange::SetPhysical { instance, choice },
            EditPolicy::Direct,
        )?,
    }
    Ok(Outcome::None)
}
