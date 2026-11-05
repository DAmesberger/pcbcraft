use super::Outcome;
use crate::diagnostics::Result;
use crate::fields::PropertyEdit;
use crate::functions::FunctionEdit;
use crate::functions::pin_config::PinConfigEdit;
use crate::ids::{GraphicId, Id, PartRef, TerminalId, UnitId};
use crate::instances::pins::AssignPeripheral;
use crate::instances::{self, NewInstance};
use crate::library::marking::InstanceMarking;
use crate::model::{EditPolicy, PhysicalChoice, Project, TextPlacement};
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
    /// C2: enable (or clear) one peripheral: its roles placed explicitly
    /// or by the solver, compiled to one `select_functions` batch, one
    /// undo step.
    #[serde(rename = "instance.assign_peripheral")]
    AssignPeripheral(AssignPeripheral),
    /// C2: set or clear 1–256 terminals' pin configuration, validated
    /// against their declared options. Never changes function ERC
    /// directly; pin rules may read it through `option(…)`.
    #[serde(rename = "instance.set_pin_config")]
    SetPinConfig {
        instance: Id,
        edits: Vec<PinConfigEdit>,
    },
    /// D4: the instance's marking choices (hide, variables, replace).
    #[serde(rename = "instance.set_marking")]
    SetMarking {
        instance: Id,
        marking: InstanceMarking,
    },
    /// Move or turn one footprint text on this instance only, such as a
    /// designator a rotated part prints off the board. `graphic` is the
    /// footprint's text, by default its silkscreen designator. Without a
    /// `placement` the text returns to where its footprint prints it.
    /// Restating the current placement is not an undo step.
    #[serde(rename = "instance.set_text_placement")]
    SetTextPlacement {
        instance: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        graphic: Option<GraphicId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placement: Option<TextPlacement>,
    },
}

// A new command chooses its permission class here.
declare_classes!(InstanceCommand {
    Place "component.place" Edit,
    Remove "component.remove" Edit,
    SetDesignator "instance.set_designator" Edit,
    SetProperties "instance.set_properties" Edit,
    SelectFunctions "instance.select_functions" Edit,
    SetUnitUnused "instance.set_unit_unused" Edit,
    SetNoConnect "instance.set_no_connect" Edit,
    SetPhysical "instance.set_physical" Edit,
    AssignPart "instance.assign_part" Edit,
    AssignPeripheral "instance.assign_peripheral" Edit,
    SetPinConfig "instance.set_pin_config" Edit,
    SetMarking "instance.set_marking" Edit,
    SetTextPlacement "instance.set_text_placement" Edit,
});

impl InstanceCommand {
    /// Whether a valid command would leave the project as it is; a refused
    /// one answers its refusal.
    pub(crate) fn restates(&self, project: &Project) -> Result<bool> {
        match self {
            Self::SetTextPlacement {
                instance,
                graphic,
                placement,
            } => instances::text::restated(project, *instance, *graphic, *placement),
            _ => Ok(false),
        }
    }
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
        InstanceCommand::AssignPeripheral(request) => {
            let resolution = instances::pins::assign_peripheral(project, &request)?;
            return Ok(Outcome::FunctionsSelected { resolution });
        }
        InstanceCommand::SetPinConfig { instance, edits } => {
            instances::pins::set_pin_config(project, instance, &edits)?
        }
        InstanceCommand::SetMarking { instance, marking } => {
            return super::marking::set_marking(project, instance, marking);
        }
        InstanceCommand::SetTextPlacement {
            instance,
            graphic,
            placement,
        } => instances::text::set(project, instance, graphic, placement)?,
    }
    Ok(Outcome::None)
}
