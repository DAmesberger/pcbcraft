//! Component definitions: electrical terminals and the exact symbols,
//! bindings and fields a revision allows.
use super::publish::{issue, object};
use super::{Library, MAX_COMPONENT_LISTS, MAX_LABEL, MAX_TERMINALS, printable};
use crate::diagnostics::{Finding, ObjectRef};
use crate::fields::{self, FieldSelection, MAX_FIELDS};
use crate::functions::{
    self, FunctionConstraint, Resource, TerminalDeclaration, TerminalFunctions,
};
use crate::ids::{BindingRef, DefinitionId, DefinitionRef, SymbolRef, TerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Longest designator prefix, in letters.
pub const MAX_DESIGNATOR_PREFIX: usize = 8;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ComponentDefinition {
    /// Next unused local ID. Retired IDs are never reused.
    pub next_local: u32,
    /// The letters `component.place` numbers new instances under when no
    /// designator is given (`R` → R1, R2, …): 1–8 uppercase ASCII letters,
    /// for example `R`, `C`, `L`, `D`, `Q`, `U` or `J`. Authored data; the
    /// editor never derives it from names or values.
    pub designator_prefix: String,
    pub terminals: Vec<Terminal>,
    /// Shared resources whose roles terminal functions take.
    pub resources: Vec<Resource>,
    /// Rules between functions, scoped to one instance.
    pub constraints: Vec<FunctionConstraint>,
    /// Allowed representations, at most one revision per symbol lineage.
    pub representations: Vec<SymbolRef>,
    /// Allowed physical implementations, at most one revision per binding
    /// lineage. A revision with none is schematic-usable only.
    pub implementations: Vec<BindingRef>,
    /// Selected fields, at most one revision per field lineage, with
    /// component defaults.
    pub fields: Vec<FieldSelection>,
    /// C1: sandboxed Rhai pin rules, part of the revision (F2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pin_rules: Vec<functions::PinRule>,
    /// D4: the generic marking of every instance (an exact Part's top mark
    /// takes precedence).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marking: Option<Box<super::marking::MarkingSpec>>,
    /// C3/C5: import review state (F8); `None` when nothing is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<Box<super::pinout::ImportReview>>,
    /// The generating parameters of an on-demand generic part: what a
    /// search for the same part finds instead of building another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic: Option<Box<super::templates::generic::GenericSpec>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub id: TerminalId,
    pub name: String,
    pub class: TerminalClass,
}

/// What a terminal is, as the definition classifies it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalClass {
    /// An electrical terminal with one or more alternative functions.
    Electrical(TerminalFunctions),
    /// Device-declared not connected: no functions, never wired, needs no
    /// project decision.
    DeviceNc,
    /// Reserved by the device: no functions, never wired, needs no project
    /// decision.
    Reserved,
}

impl Terminal {
    /// The terminal's functions; `None` for device-NC and reserved terminals.
    pub fn functions(&self) -> Option<&TerminalFunctions> {
        match &self.class {
            TerminalClass::Electrical(functions) => Some(functions),
            TerminalClass::DeviceNc | TerminalClass::Reserved => None,
        }
    }
    pub fn is_electrical(&self) -> bool {
        self.functions().is_some()
    }
}

impl ComponentDefinition {
    pub fn terminal(&self, id: TerminalId) -> Option<&Terminal> {
        self.terminals.iter().find(|t| t.id == id)
    }
    /// Every terminal as the function resolver sees it.
    pub fn declarations(&self) -> Vec<TerminalDeclaration<'_>> {
        self.terminals
            .iter()
            .map(|t| TerminalDeclaration {
                terminal: t.id,
                name: &t.name,
                functions: t.functions(),
            })
            .collect()
    }
    /// The listed revision of a symbol lineage.
    pub fn representation(&self, lineage: DefinitionId) -> Option<SymbolRef> {
        self.representations
            .iter()
            .copied()
            .find(|r| r.id == lineage)
    }
    /// The listed revision of a binding lineage.
    pub fn implementation(&self, lineage: DefinitionId) -> Option<BindingRef> {
        self.implementations
            .iter()
            .copied()
            .find(|r| r.id == lineage)
    }

    /// The selection of a field lineage.
    pub fn field(&self, lineage: DefinitionId) -> Option<&FieldSelection> {
        self.fields.iter().find(|f| f.field.id == lineage)
    }

    pub(super) fn within_bounds(&self) -> bool {
        self.designator_prefix.len() <= MAX_LABEL
            && self.terminals.len() <= MAX_TERMINALS
            && self.representations.len() <= MAX_COMPONENT_LISTS
            && self.implementations.len() <= MAX_COMPONENT_LISTS
            && self.fields.len() <= MAX_FIELDS
            && self.terminals.iter().all(|t| {
                t.name.len() <= MAX_LABEL && t.functions().is_none_or(|f| f.within_bounds())
            })
            && functions::declarations_within_bounds(&self.resources, &self.constraints)
            && functions::rules::within_bounds(&self.pin_rules)
            && self.marking.as_ref().is_none_or(|m| m.within_bounds())
            && self
                .fields
                .iter()
                .all(|f| f.default.as_ref().is_none_or(|d| d.within_bounds()))
    }

    pub(super) fn values_finite(&self) -> bool {
        self.fields
            .iter()
            .all(|f| f.default.as_ref().is_none_or(|d| d.values_finite()))
    }

    /// Terminal, function, resource and role IDs: one namespace.
    pub(super) fn local_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.terminals.iter().map(|t| t.id.0).collect();
        for terminal in &self.terminals {
            ids.extend(terminal.functions().into_iter().flat_map(|f| f.ids()));
        }
        ids.extend(self.resources.iter().flat_map(|r| r.ids()));
        ids.extend(self.pin_rules.iter().map(|r| r.id.0));
        ids
    }

    pub(super) fn findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        let named = |r: DefinitionRef| super::names::capitalised(&library.describe(r));
        if !designator_prefix_valid(&self.designator_prefix) {
            out.push(issue(
                "component.designator_prefix",
                "A component needs a designator prefix of 1–8 uppercase letters A–Z, such as R, C or U."
                    .into(),
                vec![this],
            ));
        }
        // A mechanical part (docs/mechanical-parts.md) may have none.
        let least = usize::from(!super::mechanical::implemented_mechanically(library, self));
        if !(least..=MAX_TERMINALS).contains(&self.terminals.len()) {
            out.push(issue(
                "component.terminal_count",
                format!("A component needs {least}–{MAX_TERMINALS} electrical terminals."),
                vec![this],
            ));
        }
        for terminal in &self.terminals {
            if !printable(&terminal.name) {
                out.push(issue(
                    "component.terminal_name",
                    "A terminal needs a printable name.".into(),
                    vec![object(id, terminal.id.0)],
                ));
            }
        }
        let terminals: BTreeSet<TerminalId> = self.terminals.iter().map(|t| t.id).collect();
        let declarations = self.declarations();
        out.extend(functions::declaration_findings(
            id,
            &declarations,
            &self.resources,
            &self.constraints,
        ));
        out.extend(
            functions::pin_config::declaration_problems(&declarations, &self.resources)
                .into_iter()
                .map(|p| issue(p.code, p.message, vec![object(id, p.local)])),
        );
        // Compilation, references and evaluation: without selections and
        // over the samples each rule names.
        out.extend(functions::rules::publish_findings(
            id,
            Some((&declarations, &self.resources)),
            &self.pin_rules,
        ));
        if let Some(marking) = &self.marking {
            out.extend(super::marking::spec_findings(id, marking));
            out.extend(super::marking::artwork_findings(library, id, marking));
        }
        if let Some(review) = &self.review {
            out.extend(super::pinout::review_findings(id, review));
        }
        if self.representations.is_empty() {
            out.push(issue(
                "component.no_representation",
                "A component needs at least one symbol representation.".into(),
                vec![this],
            ));
        }
        if !unique_lineages(self.representations.iter().map(|r| r.id)) {
            out.push(issue(
                "component.duplicate_symbol_lineage",
                "A component lists at most one revision of each symbol lineage.".into(),
                vec![this],
            ));
        }
        for r in &self.representations {
            let Some(symbol) = library.symbol(*r) else {
                out.push(issue(
                    "reference.missing",
                    format!("{} is missing from the project library.", named(r.erased())),
                    vec![this, ObjectRef::Definition { id: r.id }],
                ));
                continue;
            };
            if symbol.definition.component != id {
                out.push(issue(
                    "reference.owner_mismatch",
                    format!("{} belongs to another component.", named(r.erased())),
                    vec![this, ObjectRef::Definition { id: r.id }],
                ));
            }
            let pinned: BTreeSet<TerminalId> = symbol
                .definition
                .units
                .iter()
                .flat_map(|u| u.pins.iter().map(|p| p.terminal))
                .collect();
            for terminal in &self.terminals {
                if !pinned.contains(&terminal.id) {
                    out.push(issue(
                        "component.pin_missing",
                        // The terminal first: the rows of one symbol
                        // differ at their start.
                        format!(
                            "Terminal ‘{}’ has no pin in {}.",
                            terminal.name,
                            library.describe(r.erased())
                        ),
                        vec![
                            object(id, terminal.id.0),
                            ObjectRef::Definition { id: r.id },
                        ],
                    ));
                }
            }
            if pinned.difference(&terminals).next().is_some() {
                out.push(issue(
                    "component.pin_unknown",
                    format!(
                        "{} has a pin for a terminal this component does not have.",
                        named(r.erased())
                    ),
                    vec![this, ObjectRef::Definition { id: r.id }],
                ));
            }
            for field in symbol.definition.text_fields() {
                if self.field(field).is_none() {
                    out.push(issue(
                        "component.text_field_unknown",
                        format!(
                            "{} shows {}, which this component does not select.",
                            named(r.erased()),
                            library.describe_lineage(crate::ids::DefinitionKind::Field, field)
                        ),
                        vec![this, ObjectRef::Definition { id: field }],
                    ));
                }
            }
        }
        if !unique_lineages(self.implementations.iter().map(|r| r.id)) {
            out.push(issue(
                "component.duplicate_binding_lineage",
                "A component lists at most one revision of each binding lineage.".into(),
                vec![this],
            ));
        }
        for r in &self.implementations {
            let Some(binding) = library.binding(*r) else {
                out.push(issue(
                    "reference.missing",
                    format!("{} is missing from the project library.", named(r.erased())),
                    vec![this, ObjectRef::Definition { id: r.id }],
                ));
                continue;
            };
            if binding.definition.component != id {
                out.push(issue(
                    "reference.owner_mismatch",
                    format!("{} belongs to another component.", named(r.erased())),
                    vec![this, ObjectRef::Definition { id: r.id }],
                ));
            }
            let mapped: BTreeSet<TerminalId> = binding
                .definition
                .mappings
                .iter()
                .map(|m| m.terminal)
                .collect();
            for terminal in &self.terminals {
                if !mapped.contains(&terminal.id) {
                    out.push(issue(
                        "component.mapping_missing",
                        format!(
                            "{} maps no package terminal to terminal ‘{}’.",
                            named(r.erased()),
                            terminal.name
                        ),
                        vec![
                            object(id, terminal.id.0),
                            ObjectRef::Definition { id: r.id },
                        ],
                    ));
                }
            }
            if mapped.difference(&terminals).next().is_some() {
                out.push(issue(
                    "component.mapping_unknown",
                    format!(
                        "{} maps a terminal this component does not have.",
                        named(r.erased())
                    ),
                    vec![this, ObjectRef::Definition { id: r.id }],
                ));
            }
            out.extend(functions::binding_findings(
                id,
                r.id,
                &declarations,
                &binding.definition.unavailable,
            ));
        }
        self.field_findings(id, library, out);
        if self.implementations.is_empty() {
            out.push(Finding::new(
                "component.no_implementation",
                "No physical implementation: instances are schematic-only until one is added."
                    .into(),
                vec![this],
            ));
        }
    }
}

impl ComponentDefinition {
    /// Selections are unique per field lineage, resolve in the snapshot and
    /// have defaults that satisfy their field.
    fn field_findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let named = |r: DefinitionRef| super::names::capitalised(&library.describe(r));
        let this = ObjectRef::Definition { id };
        if !unique_lineages(self.fields.iter().map(|f| f.field.id)) {
            out.push(issue(
                "component.duplicate_field_lineage",
                "A component selects at most one revision of each field lineage.".into(),
                vec![this],
            ));
        }
        for selection in &self.fields {
            let r = selection.field;
            let Some(view) = library.field_view(r) else {
                out.push(issue(
                    "reference.missing",
                    format!(
                        "{} or its quantity kind is missing from the project library.",
                        named(r.erased())
                    ),
                    vec![this, ObjectRef::Definition { id: r.id }],
                ));
                continue;
            };
            if let Some(default) = &selection.default
                && let Err(error) = fields::check_value(
                    default,
                    &view.field.definition.value_type,
                    view.kind_body(),
                )
            {
                out.push(issue(
                    "component.field_default_invalid",
                    format!(
                        "The component default for field ‘{}’ is invalid: {error}.",
                        view.field.meta.name
                    ),
                    vec![this, ObjectRef::Definition { id: r.id }],
                ));
            }
        }
    }
}

/// 1–[`MAX_DESIGNATOR_PREFIX`] uppercase ASCII letters.
pub fn designator_prefix_valid(prefix: &str) -> bool {
    (1..=MAX_DESIGNATOR_PREFIX).contains(&prefix.len())
        && prefix.bytes().all(|b| b.is_ascii_uppercase())
}

pub(super) fn unique_lineages(ids: impl Iterator<Item = DefinitionId>) -> bool {
    let mut seen = BTreeSet::new();
    ids.into_iter().all(|id| seen.insert(id))
}
