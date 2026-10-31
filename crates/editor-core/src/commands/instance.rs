use super::Outcome;
use crate::diagnostics::Result;
use crate::fields::PropertyEdit;
use crate::functions::FunctionEdit;
use crate::ids::{Id, PartRef, TerminalId, UnitId};
use crate::instances::{self, NewInstance};
use crate::model::{EditPolicy, PhysicalChoice, Project};
use crate::updates::{self, InstanceChange};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum InstanceCommand {
    /// Create one instance and the occurrence of its first unit. Without a
    /// designator the core numbers it under the component's prefix.
    #[serde(rename = "component.place")]
    Place(NewInstance),
    /// Direct only without wires or a placement; otherwise preview
    /// `Change::Instance { Remove }`.
    #[serde(rename = "component.remove")]
    Remove { instance: Id },
    #[serde(rename = "instance.set_designator")]
    SetDesignator { instance: Id, designator: String },
    /// Apply 1–64 typed field edits atomically. Text is parsed in core; any
    /// invalid edit rejects the whole batch.
    #[serde(rename = "instance.set_properties")]
    SetProperties {
        instance: Id,
        edits: Vec<PropertyEdit>,
    },
    /// Apply 1–256 function selection edits atomically. `None` clears an
    /// explicit selection. The final state is validated once.
    #[serde(rename = "instance.select_functions")]
    SelectFunctions {
        instance: Id,
        edits: Vec<FunctionEdit>,
    },
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
    /// Pin one exact Part on 1–256 instances atomically, or clear it with
    /// `None`. The Part must pin each instance's component revision and
    /// the binding of its physical choice, and meet its requirements
    /// (`fields::satisfies`); otherwise nothing changes. Resolve parts
    /// assigns one Part to one group with one command.
    #[serde(rename = "instance.assign_part")]
    AssignPart {
        instances: Vec<Id>,
        part: Option<PartRef>,
    },
}

pub(crate) fn apply_instance(project: &mut Project, command: InstanceCommand) -> Result<Outcome> {
    match command {
        InstanceCommand::Place(request) => {
            let (instance, occurrence) = instances::place(project, request)?;
            let designator = project
                .instance(instance)
                .map(|i| i.designator.clone())
                .unwrap_or_default();
            return Ok(Outcome::InstancePlaced {
                instance,
                occurrence,
                designator,
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
        InstanceCommand::SetProperties { instance, edits } => {
            let effective = instances::set_properties(project, instance, &edits)?;
            return Ok(Outcome::PropertiesSet { effective });
        }
        InstanceCommand::SelectFunctions { instance, edits } => {
            let resolution = instances::select_functions(project, instance, &edits)?;
            return Ok(Outcome::FunctionsSelected { resolution });
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
        InstanceCommand::AssignPart { instances, part } => {
            instances::assign_part(project, &instances, part)?
        }
        InstanceCommand::SetPhysical { instance, choice } => updates::stage(
            project,
            &InstanceChange::SetPhysical { instance, choice },
            EditPolicy::Direct,
        )?,
    }
    Ok(Outcome::None)
}
