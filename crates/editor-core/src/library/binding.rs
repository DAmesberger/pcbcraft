//! Bindings: the explicit mapping from one component lineage's terminals to
//! one exact package revision, with the footprint variants it allows.
use super::component::unique_lineages;
use super::publish::{issue, object};
use super::{Library, MAX_FOOTPRINTS_PER_BINDING, MAX_PACKAGE_TERMINALS};
use crate::diagnostics::{Finding, ObjectRef};
use crate::functions::MAX_UNAVAILABLE;
use crate::ids::{
    DefinitionId, DefinitionKind, FootprintRef, FunctionKey, PackageRef, PackageTerminalId,
    TerminalId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BindingDefinition {
    /// The owning component lineage. Only its revisions may list this binding.
    pub component: DefinitionId,
    pub package: PackageRef,
    pub mappings: Vec<TerminalMap>,
    /// Allowed footprint variants, each pinning `package`.
    pub footprints: Vec<FootprintRef>,
    /// Functions this implementation cannot provide. Whether each names a
    /// real function is checked by the owning component's publication.
    pub unavailable: Vec<FunctionKey>,
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
            && self.unavailable.len() <= MAX_UNAVAILABLE
    }

    pub(super) fn findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        let named = |r: crate::ids::DefinitionRef| super::names::capitalised(&library.describe(r));
        // The owner is a component lineage that is published, or staged in
        // the same publish set; never unset, unknown or this lineage itself.
        if !matches!(
            library.latest(self.component),
            Some((DefinitionKind::Component, _))
        ) || self.component == id
        {
            out.push(issue(
                "reference.owner_mismatch",
                "The owner is not a component that is published or published with it.".into(),
                vec![this],
            ));
        }
        let mut keys = BTreeSet::new();
        if !self.unavailable.iter().all(|k| keys.insert(*k)) {
            out.push(issue(
                "binding.unavailable_duplicate",
                "A binding lists each unavailable function once.".into(),
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
                    format!("{} is missing from the project library.", named(r.erased())),
                    vec![this, ObjectRef::Definition { id: r.id }],
                )),
                Some(footprint) if footprint.definition.package != self.package => out.push(issue(
                    "binding.footprint_package",
                    format!(
                        "{} lands {}, not this binding's {}.",
                        named(r.erased()),
                        library.describe(footprint.definition.package.erased()),
                        library.describe(self.package.erased())
                    ),
                    vec![this, ObjectRef::Definition { id: r.id }],
                )),
                Some(_) => {}
            }
        }
        let Some(package) = library.package(self.package) else {
            out.push(issue(
                "reference.missing",
                format!(
                    "{} is missing from the project library.",
                    named(self.package.erased())
                ),
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
                    "A mapping names a package terminal the package does not have.".into(),
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
                        "Package terminal ‘{}’ needs exactly one electrical terminal ({count} mapped).",
                        terminal.designation
                    ),
                    vec![this, object(self.package.id, terminal.id.0)],
                ));
            }
        }
    }
}
