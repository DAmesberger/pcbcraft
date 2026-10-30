//! Bindings: the explicit mapping from one component lineage's terminals to
//! one exact package revision, with the footprint variants it allows.
use super::component::unique_lineages;
use super::publish::{issue, object};
use super::{Library, MAX_FOOTPRINTS_PER_BINDING, MAX_PACKAGE_TERMINALS};
use crate::diagnostics::{Finding, ObjectRef};
use crate::ids::{
    DefinitionId, DefinitionKind, FootprintRef, PackageRef, PackageTerminalId, TerminalId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BindingDefinition {
    /// The owning component lineage. Only its revisions may list this binding.
    pub component: DefinitionId,
    pub package: PackageRef,
    pub mappings: Vec<TerminalMap>,
    /// Allowed footprint variants, each pinning `package`.
    pub footprints: Vec<FootprintRef>,
}

/// One package terminal carries one electrical terminal. Several package
/// terminals may carry the same electrical terminal.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalMap {
    pub package_terminal: PackageTerminalId,
    pub terminal: TerminalId,
}

impl BindingDefinition {
    /// The electrical terminal a package terminal carries.
    pub fn terminal_of(&self, package_terminal: PackageTerminalId) -> Option<TerminalId> {
        self.mappings
            .iter()
            .find(|m| m.package_terminal == package_terminal)
            .map(|m| m.terminal)
    }
    /// The listed revision of a footprint lineage.
    pub fn footprint(&self, lineage: DefinitionId) -> Option<FootprintRef> {
        self.footprints.iter().copied().find(|f| f.id == lineage)
    }

    pub(super) fn within_bounds(&self) -> bool {
        self.mappings.len() <= MAX_PACKAGE_TERMINALS
            && self.footprints.len() <= MAX_FOOTPRINTS_PER_BINDING
    }

    pub(super) fn findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        // The owner is a component lineage that is published, or staged in
        // the same publish set; never unset, unknown or this lineage itself.
        if !matches!(
            library.latest(self.component),
            Some((DefinitionKind::Component, _))
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
        if !(1..=MAX_FOOTPRINTS_PER_BINDING).contains(&self.footprints.len()) {
            out.push(issue(
                "binding.footprint_count",
                "A binding lists 1–16 footprint variants.".into(),
                vec![this],
            ));
        }
        if !unique_lineages(self.footprints.iter().map(|f| f.id)) {
            out.push(issue(
                "binding.duplicate_footprint_lineage",
                "A binding lists at most one revision of each footprint lineage.".into(),
                vec![this],
            ));
        }
        for r in &self.footprints {
            match library.footprint(*r) {
                None => out.push(issue(
                    "reference.missing",
                    format!("Footprint {r} is not in the project library."),
                    vec![this, ObjectRef::Definition { id: r.id }],
                )),
                Some(footprint) if footprint.definition.package != self.package => out.push(issue(
                    "binding.footprint_package",
                    format!(
                        "Footprint {r} lands package {}, not this binding's {}.",
                        footprint.definition.package, self.package
                    ),
                    vec![this, ObjectRef::Definition { id: r.id }],
                )),
                Some(_) => {}
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
        let mut mapped: BTreeMap<PackageTerminalId, usize> = BTreeMap::new();
        for mapping in &self.mappings {
            if package
                .definition
                .terminal(mapping.package_terminal)
                .is_none()
            {
                out.push(issue(
                    "binding.mapping_unknown",
                    format!(
                        "Mapping names unknown package terminal {}.",
                        mapping.package_terminal
                    ),
                    vec![this],
                ));
            }
            *mapped.entry(mapping.package_terminal).or_default() += 1;
        }
        for terminal in &package.definition.terminals {
            let count = mapped.get(&terminal.id).copied().unwrap_or(0);
            if count != 1 {
                out.push(issue(
                    "binding.mapping_missing",
                    format!(
                        "Package terminal {} ‘{}’ needs exactly one electrical terminal ({count} mapped).",
                        terminal.id, terminal.designation
                    ),
                    vec![this, object(self.package.id, terminal.id.0)],
                ));
            }
        }
    }
}
