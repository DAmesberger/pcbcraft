//! Purchasing completeness, the BOM and Resolve-parts groups (S2). All
//! derived from the instances, their Parts and `fields::satisfies`; nothing
//! here is stored.
use crate::diagnostics::{Error, Finding, Result};
use crate::fields::Requirement;
use crate::ids::{ComponentRef, DefinitionId, FootprintRef, Id, PartRef};
use crate::instances;
use crate::model::{ComponentInstance, PhysicalChoice, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether an instance names an exact orderable part that agrees with it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PurchasingState {
    /// Generic: no Part is chosen yet.
    NoPart,
    /// A Part is chosen but a finding remains (component or binding
    /// mismatch, an unmet requirement, a stale Part revision).
    Unresolved,
    /// A Part is chosen and nothing disagrees with it.
    Resolved,
}

/// The purchasing part of `component.inspect`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PurchasingReport {
    pub state: PurchasingState,
    pub part: Option<PartRef>,
    pub manufacturer: Option<String>,
    pub mpn: Option<String>,
    /// Why the state is not `resolved`; advisory, they block no stage.
    pub findings: Vec<Finding>,
}

/// Purchasing completeness of one instance.
pub fn purchasing(project: &Project, instance: &ComponentInstance) -> PurchasingReport {
    let Some(r) = instance.part else {
        return PurchasingReport {
            state: PurchasingState::NoPart,
            part: None,
            manufacturer: None,
            mpn: None,
            findings: Vec::new(),
        };
    };
    let part = project.library.part(r);
    let mut findings = instances::part_findings(&project.library, instance);
    // Contract: agreement checks arrive with S2 task 1, so a chosen Part is
    // never reported resolved before they exist.
    findings.push(Finding::new(
        "part.unchecked",
        format!(
            "{}: agreement with Part {r} is not checked yet.",
            instance.designator
        ),
        vec![crate::diagnostics::ObjectRef::Instance { id: instance.id }],
    ));
    PurchasingReport {
        state: if findings.is_empty() {
            PurchasingState::Resolved
        } else {
            PurchasingState::Unresolved
        },
        part: Some(r),
        manufacturer: part.map(|p| p.definition.manufacturer.clone()),
        mpn: part.map(|p| p.definition.mpn.clone()),
        findings,
    }
}

/// One effective value printed on a BOM line.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BomValue {
    pub field: DefinitionId,
    pub label: String,
    pub text: String,
}

/// One BOM line: instances that buy the same thing. Resolved instances
/// group by Part; unresolved ones by component revision, footprint and
/// identical requirements, exactly as Resolve parts groups them.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BomLine {
    pub component: ComponentRef,
    pub footprint: Option<FootprintRef>,
    pub part: Option<PartRef>,
    pub manufacturer: Option<String>,
    pub mpn: Option<String>,
    pub values: Vec<BomValue>,
    /// In designator order.
    pub designators: Vec<String>,
    pub instances: Vec<Id>,
    pub quantity: u32,
    /// The line has no Part, or its Part has a purchasing finding.
    pub unresolved: bool,
    pub state: PurchasingState,
}

/// `project.bom`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BomReport {
    pub lines: Vec<BomLine>,
    /// Lines with `unresolved`.
    pub unresolved: usize,
}

/// The project BOM.
///
/// Contract stub: S2 task 1 implements the grouping.
pub fn bom(_project: &Project) -> Result<BomReport> {
    Err(Error::Unsupported(
        "project.bom is not implemented yet (S2 task 1)".into(),
    ))
}

/// Unresolved instances with identical requirements and footprint, which one
/// `instance.assign_part` can resolve together.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ResolveGroup {
    pub component: ComponentRef,
    pub physical: Option<PhysicalChoice>,
    pub footprint: Option<FootprintRef>,
    /// Shared by every instance of the group.
    pub requirements: Vec<Requirement>,
    /// In designator order.
    pub instances: Vec<Id>,
    pub designators: Vec<String>,
    /// A `library.search` query that finds Parts for the group: `kind:part`
    /// plus the requirements as field predicates. Pre-fills the Place
    /// palette's Choose….
    pub query: String,
}

/// `library.resolve_groups`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ResolveReport {
    pub groups: Vec<ResolveGroup>,
}

/// Resolve-parts groups, in BOM order.
///
/// Contract stub: S2 task 1 implements the grouping.
pub fn resolve_groups(_project: &Project) -> Result<ResolveReport> {
    Err(Error::Unsupported(
        "library.resolve_groups is not implemented yet (S2 task 1)".into(),
    ))
}
