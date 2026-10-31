//! Derived completeness. Nothing stores a "complete" or "verified" flag; a
//! state holds when no current finding blocks it.
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::fields::{self, EffectiveField};
use crate::functions::{Direction, Resolution};
use crate::ids::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, DefinitionRef, Endpoint, FootprintRef,
    Id, PackageRef, Revision, SymbolRef, UnitId,
};
use crate::instances::{self, UnitState};
use crate::library::{self, Draft, Drafts, Grid, Library};
use crate::model::Project;
use crate::nets;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod purchasing;
pub use purchasing::{
    BomLine, BomReport, BomValue, PurchasingReport, PurchasingState, ResolveGroup, ResolveReport,
    bom, purchasing, requirement_query, resolve_groups,
};

/// The exact physical revisions an instance resolves to.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PhysicalReport {
    pub binding: BindingRef,
    pub footprint: FootprintRef,
    pub package: PackageRef,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UnitReport {
    pub unit: UnitId,
    pub label: String,
    /// Designator plus unit label when the representation has several units.
    pub display: String,
    pub state: UnitState,
}

/// `component.inspect`: exact pins, resolved choices, unit states and
/// derived completeness of one instance.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct InstanceReport {
    pub instance: Id,
    pub designator: String,
    pub component: ComponentRef,
    pub representation: SymbolRef,
    pub physical: Option<PhysicalReport>,
    pub units: Vec<UnitReport>,
    /// Effective value and origin of every selected field.
    pub properties: Vec<EffectiveField>,
    /// Active, unresolved and incomplete terminal functions. Conflicts are
    /// rejected invariants, so this bucket is always empty here.
    pub functions: Resolution,
    pub occurrences: usize,
    /// 0 or 1.
    pub board_placements: usize,
    /// Always 1 per instance, however many units it draws.
    pub bom_quantity: u32,
    pub board_placeable: bool,
    pub electrically_resolved: bool,
    /// Purchasing completeness: the chosen Part, if any, and what disagrees
    /// with it. Its findings are advisory and not repeated in `findings`.
    pub purchasing: PurchasingReport,
    pub findings: Vec<Finding>,
}

/// What `component.check` examines.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "target", content = "id", rename_all = "snake_case")]
pub enum CheckTarget {
    Draft(DefinitionId),
    Revision(DefinitionRef),
}

/// `component.check`: the publish findings of exactly one draft or revision.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DefinitionReport {
    pub target: CheckTarget,
    pub kind: DefinitionKind,
    /// The revision checked, or the draft's pending revision.
    pub revision: DefinitionRef,
    pub publishable: bool,
    pub findings: Vec<Finding>,
}

/// Completeness of one instance.
pub fn instance(project: &Project, id: Id) -> Result<InstanceReport> {
    let item = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    let resolved = instances::resolve(&project.library, item)?;
    let symbol = &resolved.symbol.definition;
    let mut findings = Vec::new();
    let mut units = Vec::new();
    for (unit, state) in instances::unit_states(project, item)? {
        let display = instances::display_label(project, item, unit);
        if state == UnitState::Unplaced {
            findings.push(Finding::blocking(
                "instance.unit_unplaced",
                format!("{display} is neither placed nor marked unused."),
                vec![ObjectRef::Unit { instance: id, unit }],
                Stage::ElectricallyResolved,
            ));
        }
        units.push(UnitReport {
            unit,
            label: symbol
                .unit(unit)
                .map(|u| u.label.clone())
                .unwrap_or_default(),
            display,
            state,
        });
    }
    let functions = instances::function_resolution(&project.library, item)?;
    for terminal in &resolved.component.definition.terminals {
        let endpoint = Endpoint {
            instance: id,
            terminal: terminal.id,
        };
        let object = vec![ObjectRef::Terminal { endpoint }];
        if !nets::accounted_for(project, endpoint) {
            findings.push(Finding::blocking(
                "instance.terminal_unaccounted",
                format!(
                    "{} terminal {} ‘{}’ is neither connected nor marked no-connect.",
                    item.designator, terminal.id, terminal.name
                ),
                object,
                Stage::ElectricallyResolved,
            ));
        } else if item.no_connect.contains(&terminal.id)
            && functions
                .active(terminal.id)
                .is_some_and(|f| f.direction == Direction::PowerInput)
        {
            findings.push(Finding::blocking(
                "instance.required_terminal_no_connect",
                format!(
                    "{} terminal {} ‘{}’ is a required power input; no-connect cannot satisfy it.",
                    item.designator, terminal.id, terminal.name
                ),
                object,
                Stage::ElectricallyResolved,
            ));
        }
    }
    for terminal in &functions.unresolved {
        let name = resolved
            .component
            .definition
            .terminal(*terminal)
            .map(|t| t.name.as_str())
            .unwrap_or("?");
        findings.push(Finding::blocking(
            "config.function_unresolved",
            format!(
                "{} terminal {terminal} ‘{name}’ has no selected function and no default.",
                item.designator
            ),
            vec![ObjectRef::Terminal {
                endpoint: Endpoint {
                    instance: id,
                    terminal: *terminal,
                },
            }],
            Stage::ElectricallyResolved,
        ));
    }
    for requirement in &functions.incomplete {
        let mut objects = vec![ObjectRef::Instance { id }];
        objects.extend(
            requirement
                .terminals()
                .into_iter()
                .map(|terminal| ObjectRef::Terminal {
                    endpoint: Endpoint {
                        instance: id,
                        terminal,
                    },
                }),
        );
        findings.push(Finding::blocking(
            "config.requirement_unmet",
            format!("{}: {}", item.designator, requirement.message()),
            objects,
            Stage::ElectricallyResolved,
        ));
    }
    let properties = instances::effective_fields(&project.library, item)?;
    for field in &properties {
        if let Some(stage) = fields::unmet(field) {
            findings.push(Finding::blocking(
                "instance.field_required",
                format!(
                    "{} needs a value for required field ‘{}’.",
                    item.designator, field.label
                ),
                vec![ObjectRef::Property {
                    instance: id,
                    field: field.field.id,
                }],
                stage,
            ));
        }
    }
    if resolved.physical.is_none() {
        findings.push(Finding::blocking(
            "instance.no_physical_choice",
            format!("{} has no physical implementation chosen.", item.designator),
            vec![ObjectRef::Instance { id }],
            Stage::BoardPlaceable,
        ));
    }
    let blocked = |stage| findings.iter().any(|f: &Finding| f.blocks_stage(stage));
    Ok(InstanceReport {
        instance: id,
        designator: item.designator.clone(),
        component: item.component,
        representation: SymbolRef::new(resolved.symbol.id, resolved.symbol.revision),
        physical: resolved.physical.map(|p| PhysicalReport {
            binding: BindingRef::new(p.binding.id, p.binding.revision),
            footprint: FootprintRef::new(p.footprint.id, p.footprint.revision),
            package: PackageRef::new(p.package.id, p.package.revision),
        }),
        occurrences: project.occurrences_of(id).count(),
        board_placements: usize::from(project.placement_of(id).is_some()),
        bom_quantity: 1,
        board_placeable: !blocked(Stage::BoardPlaceable),
        electrically_resolved: !blocked(Stage::ElectricallyResolved),
        purchasing: purchasing(project, item),
        units,
        properties,
        functions,
        findings,
    })
}

/// Publish findings of one draft or published revision. A draft is checked
/// as if published together with every other stageable draft, so pending
/// references between drafts resolve as they would in one publish set.
pub fn definition(
    project: &Project,
    drafts: &Drafts,
    target: CheckTarget,
) -> Result<DefinitionReport> {
    match target {
        CheckTarget::Revision(r) => {
            if project.library.definition(r).is_none() {
                return Err(Error::NotFound(format!(
                    "{:?} {} {} is not in the project library.",
                    r.kind, r.id, r.revision
                )));
            }
            let findings = library::publish_findings(&project.library, r);
            Ok(report(target, r, findings))
        }
        CheckTarget::Draft(id) => {
            let draft = drafts
                .get(id)
                .ok_or_else(|| Error::NotFound(format!("there is no draft of {id}")))?;
            let staged = Staged::new(&project.library, drafts)?;
            Ok(staged.report(draft, &drafts.grid))
        }
    }
}

/// [`definition`] for every draft, staging the drafts once.
pub fn drafts(project: &Project, drafts: &Drafts) -> Result<Vec<DefinitionReport>> {
    let staged = Staged::new(&project.library, drafts)?;
    Ok(drafts
        .items
        .iter()
        .map(|d| staged.report(d, &drafts.grid))
        .collect())
}

fn report(
    target: CheckTarget,
    revision: DefinitionRef,
    findings: Vec<Finding>,
) -> DefinitionReport {
    DefinitionReport {
        target,
        kind: revision.kind,
        revision,
        publishable: !findings.iter().any(|f| f.blocks_stage(Stage::Publish)),
        findings,
    }
}

/// The snapshot plus every stageable draft as its pending revision, and the
/// staging result of each draft.
struct Staged {
    library: Library,
    pending: Vec<(DefinitionId, std::result::Result<DefinitionRef, Finding>)>,
}

impl Staged {
    fn new(library: &Library, drafts: &Drafts) -> Result<Self> {
        let pending: Vec<_> = drafts
            .items
            .iter()
            .map(|d| (d.id, library::stageable(library, d)))
            .collect();
        let ids: Vec<DefinitionId> = pending
            .iter()
            .filter(|(_, r)| r.is_ok())
            .map(|(id, _)| *id)
            .collect();
        let (library, _) = library::candidate(library, drafts, &ids)?;
        Ok(Self { library, pending })
    }

    /// A draft that cannot be staged reports only why; it never borrows the
    /// findings of a stored revision with the same number. A staged draft
    /// also carries the advisory drawing lint against the authoring grid.
    fn report(&self, draft: &Draft, grid: &Grid) -> DefinitionReport {
        let target = CheckTarget::Draft(draft.id);
        let fallback = DefinitionRef {
            kind: draft.definition.kind(),
            id: draft.id,
            revision: draft.pending().or(draft.base).unwrap_or(Revision::FIRST),
        };
        match self.pending.iter().find(|(id, _)| *id == draft.id) {
            Some((_, Ok(r))) => {
                let mut findings = library::publish_findings(&self.library, *r);
                findings.extend(library::lint(draft, &self.library, grid));
                report(target, *r, findings)
            }
            Some((_, Err(finding))) => report(target, fallback, vec![finding.clone()]),
            None => report(target, fallback, vec![]),
        }
    }
}
