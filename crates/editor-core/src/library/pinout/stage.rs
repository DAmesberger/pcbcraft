//! Slice C3: turning a checked [`PinoutDocument`] into draft bodies, the
//! planned changes and the review items. Pure over the library and the
//! drafts; the command layer allocates lineages and stores the result.
use super::{
    ChangeKind, MAX_PINOUT_NOTE, MAX_PINOUT_PERIPHERALS, MAX_PINOUT_PINS, MAX_PINOUT_RULES,
    MAX_PINOUT_VARIANTS, PINOUT_FORMAT, PinoutAccept, PinoutChange, PinoutClass, PinoutDocument,
    PinoutPin, PinoutTarget, RULE_ROW_PREFIX, ReviewItem, ReviewReason,
};
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::functions::rules::{MAX_RULE_SOURCE, PinRule, PinRuleScript};
use crate::functions::{
    Function, MAX_FUNCTIONS, MAX_ROLES, MuxCode, PinOption, Resource, ResourceRole, Role,
    TerminalFunctions,
};
use crate::ids::{
    BindingRef, DefinitionId, FunctionId, MAX_LABEL, PackageRef, PackageTerminalId, ResourceId,
    Revision, RoleId, RuleId, SymbolRef, TerminalId,
};
use crate::library::{
    BindingDefinition, ComponentDefinition, Definition, DefinitionMeta, Drafts, Library, MAX_NOTE,
    PackageDefinition, PackageForm, PackageTerminal, SymbolDefinition, Terminal, TerminalClass,
    TerminalMap,
};
use std::collections::{BTreeMap, BTreeSet};

/// One draft body the import writes.
#[derive(Clone, Debug, PartialEq)]
pub struct StagedDraft {
    pub id: DefinitionId,
    /// A new lineage (`true`) or a replacement of the existing draft.
    pub create: bool,
    pub meta: DefinitionMeta,
    pub definition: Definition,
}

/// Everything an import would write and report, before findings of the
/// resulting drafts are added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StagedImport {
    /// Component first, then (for a new component) its symbol, and one
    /// package and binding per variant.
    pub drafts: Vec<StagedDraft>,
    pub changes: Vec<PinoutChange>,
    /// Findings of the document itself (rows, numbers, peripherals).
    pub findings: Vec<Finding>,
    pub review: Vec<ReviewItem>,
    /// Rows written, in document order.
    pub written: Vec<String>,
}

fn invalid(message: String) -> Error {
    Error::Invalid(format!("pinout.document: {message}"))
}

fn text_ok(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.len() <= max
}

/// Structural checks of a document: its format, bounds, unique keys and
/// labels, and numbers keyed by declared variants. Anything here makes
/// the document unreadable as a plan, so it refuses rather than reports.
pub fn check_document(document: &PinoutDocument, accept: &PinoutAccept) -> Result<()> {
    if document.format != PINOUT_FORMAT {
        return Err(Error::Unsupported(format!(
            "pinout.format: this build reads pinout documents of format {PINOUT_FORMAT}, not {}",
            document.format
        )));
    }
    if document.pins.is_empty() || document.pins.len() > MAX_PINOUT_PINS {
        return Err(Error::Limit(format!(
            "pinout.document: a pinout lists 1–{MAX_PINOUT_PINS} pins"
        )));
    }
    if document.variants.is_empty() || document.variants.len() > MAX_PINOUT_VARIANTS {
        return Err(Error::Limit(format!(
            "pinout.document: a pinout lists 1–{MAX_PINOUT_VARIANTS} package variants"
        )));
    }
    if document.peripherals.len() > MAX_PINOUT_PERIPHERALS {
        return Err(Error::Limit(format!(
            "pinout.document: a pinout lists at most {MAX_PINOUT_PERIPHERALS} peripherals"
        )));
    }
    if document.rules.len() > MAX_PINOUT_RULES {
        return Err(Error::Limit(format!(
            "pinout.document: a pinout lists at most {MAX_PINOUT_RULES} rules"
        )));
    }
    let device = &document.device;
    let reference = &device.reference;
    if !text_ok(&device.name, MAX_LABEL)
        || !text_ok(&reference.title, MAX_LABEL)
        || reference
            .document_revision
            .as_ref()
            .is_some_and(|r| r.len() > MAX_LABEL)
        || reference
            .url
            .as_ref()
            .is_some_and(|u| u.len() > crate::library::MAX_URL)
    {
        return Err(invalid(
            "the device needs a name and a reference title of 1–256 bytes".into(),
        ));
    }
    let mut variants = BTreeSet::new();
    for variant in &document.variants {
        if !text_ok(&variant.name, MAX_LABEL) || !variants.insert(variant.name.as_str()) {
            return Err(invalid(format!(
                "package variant ‘{}’ needs a unique name of 1–256 bytes",
                variant.name
            )));
        }
        if variant.terminal_count as usize > crate::library::MAX_PACKAGE_TERMINALS {
            return Err(Error::Limit(format!(
                "pinout.document: variant ‘{}’ has more than {} terminals",
                variant.name,
                crate::library::MAX_PACKAGE_TERMINALS
            )));
        }
    }
    let mut peripherals = BTreeSet::new();
    for peripheral in &document.peripherals {
        if !text_ok(&peripheral.label, MAX_LABEL) || !peripherals.insert(peripheral.label.as_str())
        {
            return Err(invalid(format!(
                "peripheral ‘{}’ needs a unique label of 1–256 bytes",
                peripheral.label
            )));
        }
        if peripheral.roles.len() > MAX_ROLES {
            return Err(Error::Limit(format!(
                "pinout.document: peripheral ‘{}’ has more than {MAX_ROLES} roles",
                peripheral.label
            )));
        }
        let mut roles = BTreeSet::new();
        for role in &peripheral.roles {
            if !text_ok(&role.label, MAX_LABEL) || !roles.insert(role.label.as_str()) {
                return Err(invalid(format!(
                    "role ‘{}’ of peripheral ‘{}’ needs a unique label",
                    role.label, peripheral.label
                )));
            }
        }
    }
    let mut rows = BTreeSet::new();
    for pin in &document.pins {
        let row = pin.row.as_str();
        if !text_ok(row, MAX_LABEL) || row.starts_with(RULE_ROW_PREFIX) || !rows.insert(row) {
            return Err(invalid(format!(
                "row key ‘{row}’ must be unique, 1–256 bytes and not start with ‘{RULE_ROW_PREFIX}’"
            )));
        }
        if pin.name.len() > MAX_LABEL
            || pin.note.as_ref().is_some_and(|n| n.len() > MAX_PINOUT_NOTE)
        {
            return Err(Error::Limit(format!(
                "pinout.document: row ‘{row}’ has a name or note that is too long"
            )));
        }
        for (variant, number) in &pin.number {
            if !variants.contains(variant.as_str()) {
                return Err(invalid(format!(
                    "row ‘{row}’ numbers variant ‘{variant}’, which the document does not declare"
                )));
            }
            if !text_ok(number, MAX_LABEL) {
                return Err(invalid(format!(
                    "row ‘{row}’ needs a number of 1–256 bytes in variant ‘{variant}’"
                )));
            }
        }
        if pin.class != PinoutClass::Electrical && !pin.functions.is_empty() {
            return Err(invalid(format!(
                "row ‘{row}’ is device-NC or reserved and cannot list functions"
            )));
        }
        if pin.functions.len() > MAX_FUNCTIONS {
            return Err(Error::Limit(format!(
                "pinout.document: row ‘{row}’ lists more than {MAX_FUNCTIONS} functions"
            )));
        }
    }
    let mut labels = BTreeSet::new();
    for rule in &document.rules {
        if !text_ok(&rule.label, MAX_LABEL) || !labels.insert(rule.label.as_str()) {
            return Err(invalid(format!(
                "rule ‘{}’ needs a unique label of 1–256 bytes",
                rule.label
            )));
        }
        if rule.script.len() > MAX_RULE_SOURCE {
            return Err(Error::Limit(format!(
                "pinout.document: rule ‘{}’ is longer than {MAX_RULE_SOURCE} bytes",
                rule.label
            )));
        }
    }
    if let PinoutAccept::Except(excepted) = accept {
        for key in excepted {
            let known = match key.strip_prefix(RULE_ROW_PREFIX) {
                Some(label) => labels.contains(label),
                None => rows.contains(key.as_str()),
            };
            if !known {
                return Err(invalid(format!(
                    "‘{key}’ is neither a row nor a ‘{RULE_ROW_PREFIX}’ label of the document"
                )));
            }
        }
    }
    Ok(())
}

fn accepted(accept: &PinoutAccept, key: &str) -> bool {
    match accept {
        PinoutAccept::All => true,
        PinoutAccept::Except(rows) => !rows.iter().any(|r| r == key),
    }
}

fn local(definition: DefinitionId, local: u32) -> ObjectRef {
    ObjectRef::LibraryObject { definition, local }
}

/// The component-local ID counter.
struct Locals(u32);

impl Locals {
    fn next(&mut self) -> Result<u32> {
        let id = self.0;
        self.0 = id
            .checked_add(1)
            .ok_or_else(|| Error::Limit("the component's local identities are exhausted".into()))?;
        Ok(id)
    }
}

/// Designation → terminal over every binding of `component` (its drafts
/// and the published implementations it lists); `None` where two bindings
/// disagree.
fn numbered_terminals(
    library: &Library,
    drafts: &Drafts,
    component: DefinitionId,
    existing: &ComponentDefinition,
) -> BTreeMap<String, Option<TerminalId>> {
    let mut bindings: Vec<BindingDefinition> = drafts
        .items
        .iter()
        .filter_map(|d| match &d.definition {
            Definition::Binding(b) if b.component == component => Some(b.clone()),
            _ => None,
        })
        .collect();
    for r in &existing.implementations {
        if drafts.get(r.id).is_none()
            && let Some(published) = library.binding(*r)
        {
            bindings.push(published.definition.clone());
        }
    }
    let mut map: BTreeMap<String, Option<TerminalId>> = BTreeMap::new();
    for binding in &bindings {
        let package = match drafts.get(binding.package.id).map(|d| &d.definition) {
            Some(Definition::Package(p)) => Some(p),
            _ => library.package(binding.package).map(|p| &p.definition),
        };
        let Some(package) = package else {
            continue;
        };
        for mapping in &binding.mappings {
            let Some(terminal) = package
                .terminals
                .iter()
                .find(|t| t.id == mapping.package_terminal)
            else {
                continue;
            };
            let entry = map
                .entry(terminal.designation.clone())
                .or_insert(Some(mapping.terminal));
            if *entry != Some(mapping.terminal) {
                *entry = None;
            }
        }
    }
    map
}

/// Row → existing terminal: by number through the component's bindings,
/// or, for a component with no binding at all, by exact terminal name.
/// Each terminal matches at most one row, the first.
fn match_rows(
    library: &Library,
    drafts: &Drafts,
    component: DefinitionId,
    existing: &ComponentDefinition,
    document: &PinoutDocument,
) -> BTreeMap<String, TerminalId> {
    let numbers = numbered_terminals(library, drafts, component, existing);
    let mut used = BTreeSet::new();
    let mut matched = BTreeMap::new();
    for pin in &document.pins {
        let found = if numbers.is_empty() {
            existing
                .terminals
                .iter()
                .find(|t| t.name == pin.name && !used.contains(&t.id))
                .map(|t| t.id)
        } else {
            document.variants.iter().find_map(|v| {
                pin.number
                    .get(&v.name)
                    .and_then(|n| numbers.get(n).copied().flatten())
                    .filter(|t| !used.contains(t) && existing.terminal(*t).is_some())
            })
        };
        if let Some(terminal) = found {
            used.insert(terminal);
            matched.insert(pin.row.clone(), terminal);
        }
    }
    matched
}

fn class_kind(class: &TerminalClass) -> u8 {
    match class {
        TerminalClass::Electrical(_) => 0,
        TerminalClass::DeviceNc => 1,
        TerminalClass::Reserved => 2,
    }
}

/// The options a row declares, in a fixed order.
fn options(pin: &PinoutPin) -> Vec<PinOption> {
    let mut out = Vec::new();
    if let Some(pull) = &pin.options.pull {
        out.push(PinOption::Pull(pull.clone()));
    }
    if let Some(speed) = &pin.options.speed {
        out.push(PinOption::Speed(speed.clone()));
    }
    if pin.options.initial_level {
        out.push(PinOption::InitialLevel);
    }
    if pin.options.schmitt {
        out.push(PinOption::Schmitt);
    }
    out
}

struct Builder<'a> {
    id: DefinitionId,
    document: &'a PinoutDocument,
    existing: Option<&'a ComponentDefinition>,
    locals: Locals,
    findings: Vec<Finding>,
}

impl Builder<'_> {
    fn resources(&mut self) -> Result<Vec<Resource>> {
        let mut out = Vec::new();
        for peripheral in &self.document.peripherals {
            let old = self
                .existing
                .and_then(|c| c.resources.iter().find(|r| r.label == peripheral.label));
            let id = match old {
                Some(r) => r.id,
                None => ResourceId(self.locals.next()?),
            };
            let mut roles = Vec::new();
            for role in &peripheral.roles {
                let id = match old.and_then(|r| r.roles.iter().find(|x| x.label == role.label)) {
                    Some(r) => r.id,
                    None => RoleId(self.locals.next()?),
                };
                roles.push(Role {
                    id,
                    label: role.label.clone(),
                    capacity: role.capacity,
                    required_when_used: role.required_when_used,
                    signal: role.signal.clone(),
                });
            }
            out.push(Resource {
                id,
                label: peripheral.label.clone(),
                kind: peripheral.kind.clone(),
                roles,
            });
        }
        Ok(out)
    }

    /// The terminal a row describes, keeping `old`'s terminal and function
    /// IDs where labels match.
    fn terminal(
        &mut self,
        pin: &PinoutPin,
        old: Option<&Terminal>,
        resources: &[Resource],
    ) -> Result<Terminal> {
        let id = match old {
            Some(t) => t.id,
            None => TerminalId(self.locals.next()?),
        };
        let class = match pin.class {
            PinoutClass::DeviceNc => TerminalClass::DeviceNc,
            PinoutClass::Reserved => TerminalClass::Reserved,
            PinoutClass::Electrical => {
                let previous = old.and_then(Terminal::functions);
                let mut functions = Vec::new();
                for f in &pin.functions {
                    let fid = match previous
                        .and_then(|p| p.functions.iter().find(|x| x.label == f.label))
                    {
                        Some(x) => x.id,
                        None => FunctionId(self.locals.next()?),
                    };
                    let resource_role = self.resource_role(pin, id, fid, f, resources);
                    functions.push(Function {
                        id: fid,
                        label: f.label.clone(),
                        direction: f.direction,
                        signal: f.signal,
                        drive: f.drive,
                        resource_role,
                        mux: f.mux.clone().map(MuxCode),
                        mux_table: f.mux_table.clone(),
                        source: pin.source.clone(),
                    });
                }
                let design_default = match &pin.default {
                    None => None,
                    Some(label) => {
                        let found = functions.iter().find(|f| &f.label == label).map(|f| f.id);
                        if found.is_none() {
                            self.findings.push(Finding::blocking(
                                "function.design_default",
                                format!(
                                    "Row ‘{}’ names ‘{label}’ as its default, which is not one of its functions.",
                                    pin.row
                                ),
                                vec![local(self.id, id.0)],
                                Stage::Publish,
                            ));
                        }
                        found
                    }
                };
                TerminalClass::Electrical(TerminalFunctions {
                    functions,
                    design_default,
                    gpio: pin.gpio.clone(),
                    options: options(pin),
                })
            }
        };
        Ok(Terminal {
            id,
            name: pin.name.clone(),
            class,
        })
    }

    /// The explicit peripheral and role of a function. A label the
    /// document does not declare is a finding; it is never guessed.
    fn resource_role(
        &mut self,
        pin: &PinoutPin,
        terminal: TerminalId,
        function: FunctionId,
        f: &super::PinoutFunction,
        resources: &[Resource],
    ) -> Option<ResourceRole> {
        let (peripheral, role) = match (&f.peripheral, &f.role) {
            (None, None) => return None,
            (Some(p), Some(r)) => (p, r),
            _ => {
                self.findings.push(Finding::blocking(
                    "function.resource_unknown",
                    format!(
                        "Function ‘{}’ of row ‘{}’ names a peripheral or a role but not both.",
                        f.label, pin.row
                    ),
                    vec![local(self.id, function.0)],
                    Stage::Publish,
                ));
                return None;
            }
        };
        let found = resources
            .iter()
            .find(|r| &r.label == peripheral)
            .and_then(|r| {
                r.roles
                    .iter()
                    .find(|x| &x.label == role)
                    .map(|x| ResourceRole {
                        resource: r.id,
                        role: x.id,
                    })
            });
        if found.is_none() {
            self.findings.push(Finding::blocking(
                "function.resource_unknown",
                format!(
                    "Function ‘{}’ of row ‘{}’ takes role ‘{role}’ of peripheral ‘{peripheral}’, which the document does not declare.",
                    f.label, pin.row
                ),
                vec![local(self.id, function.0), local(self.id, terminal.0)],
                Stage::Publish,
            ));
        }
        found
    }

    fn rules(&mut self, accept: &PinoutAccept) -> Result<Vec<PinRule>> {
        let mut out = Vec::new();
        for rule in &self.document.rules {
            let old = self
                .existing
                .and_then(|c| c.pin_rules.iter().find(|r| r.label == rule.label));
            if !accepted(accept, &format!("{RULE_ROW_PREFIX}{}", rule.label)) {
                out.extend(old.cloned());
                continue;
            }
            let id = match old {
                Some(r) => r.id,
                None => RuleId(self.locals.next()?),
            };
            out.push(PinRule {
                id,
                label: rule.label.clone(),
                script: PinRuleScript {
                    source: rule.script.clone(),
                },
                source: rule.source.clone(),
            });
        }
        Ok(out)
    }
}

fn change(
    kind: ChangeKind,
    object: &str,
    row: Option<&str>,
    terminal: Option<TerminalId>,
    summary: String,
) -> PinoutChange {
    PinoutChange {
        kind,
        object: object.into(),
        row: row.map(str::to_owned),
        terminal,
        summary,
    }
}

/// Function-level changes of one terminal.
fn function_changes(
    row: &str,
    old: Option<&Terminal>,
    new: &Terminal,
    out: &mut Vec<PinoutChange>,
) -> bool {
    let empty = Vec::new();
    let before = old
        .and_then(Terminal::functions)
        .map_or(&empty, |f| &f.functions);
    let after = new.functions().map_or(&empty, |f| &f.functions);
    let mut any = false;
    for f in after {
        let summary = |verb: &str| format!("Function ‘{}’ of ‘{}’ {verb}", f.label, new.name);
        match before.iter().find(|x| x.id == f.id) {
            None => {
                out.push(change(
                    ChangeKind::Added,
                    "function",
                    Some(row),
                    Some(new.id),
                    summary("added"),
                ));
                any = true;
            }
            Some(x) if x != f => {
                out.push(change(
                    ChangeKind::Changed,
                    "function",
                    Some(row),
                    Some(new.id),
                    summary("changed"),
                ));
                any = true;
            }
            Some(_) => {}
        }
    }
    for f in before {
        if !after.iter().any(|x| x.id == f.id) {
            out.push(change(
                ChangeKind::Removed,
                "function",
                Some(row),
                Some(new.id),
                format!("Function ‘{}’ of ‘{}’ removed", f.label, new.name),
            ));
            any = true;
        }
    }
    any
}

/// Whether two terminals differ apart from their function lists.
fn terminal_differs(old: &Terminal, new: &Terminal) -> bool {
    if old.name != new.name || class_kind(&old.class) != class_kind(&new.class) {
        return true;
    }
    match (old.functions(), new.functions()) {
        (Some(a), Some(b)) => {
            a.gpio != b.gpio || a.options != b.options || a.design_default != b.design_default
        }
        _ => false,
    }
}

/// The component an import produces, with its changes against `existing`
/// and the review items. `matched` maps rows to existing terminals.
fn build_component(
    id: DefinitionId,
    document: &PinoutDocument,
    accept: &PinoutAccept,
    existing: Option<&ComponentDefinition>,
    matched: &BTreeMap<String, TerminalId>,
    out: &mut StagedImport,
) -> Result<ComponentDefinition> {
    let mut builder = Builder {
        id,
        document,
        existing,
        locals: Locals(existing.map_or(1, |c| c.next_local.max(1))),
        findings: Vec::new(),
    };
    let mut resources = builder.resources()?;
    let mut terminals = Vec::new();
    let mut review = existing
        .and_then(|c| c.review.as_deref().cloned())
        .unwrap_or_default();
    for pin in &document.pins {
        let old = matched
            .get(&pin.row)
            .and_then(|t| existing.and_then(|c| c.terminal(*t)));
        if !accepted(accept, &pin.row) {
            // An excluded row keeps what the draft had.
            terminals.extend(old.cloned());
            continue;
        }
        let terminal = builder.terminal(pin, old, &resources)?;
        let mut written = false;
        match old {
            None => {
                out.changes.push(change(
                    ChangeKind::Added,
                    "terminal",
                    Some(&pin.row),
                    Some(terminal.id),
                    format!("Terminal ‘{}’ added", terminal.name),
                ));
                written = true;
            }
            Some(old) => {
                if terminal_differs(old, &terminal) {
                    out.changes.push(change(
                        ChangeKind::Changed,
                        "terminal",
                        Some(&pin.row),
                        Some(terminal.id),
                        format!("Terminal ‘{}’ changed", terminal.name),
                    ));
                    written = true;
                }
                if old.name != terminal.name
                    || class_kind(&old.class) != class_kind(&terminal.class)
                {
                    out.review.push(ReviewItem {
                        row: pin.row.clone(),
                        reason: ReviewReason::ContradictsDraft,
                        message: format!(
                            "The draft's terminal ‘{}’ has this row's number but a different name or class (‘{}’).",
                            old.name, terminal.name
                        ),
                    });
                }
            }
        }
        written |= function_changes(&pin.row, old, &terminal, &mut out.changes);
        if written {
            review.unreviewed.insert(terminal.id);
            out.written.push(pin.row.clone());
        }
        // An accepted row states the row's certainty afresh: a corrected
        // re-import that no longer flags it clears the flag (F8).
        if pin.uncertain {
            review.uncertain.insert(terminal.id);
        } else {
            review.uncertain.remove(&terminal.id);
        }
        terminals.push(terminal);
    }
    // Resources of the draft the document no longer lists stay while a
    // kept terminal still takes one of their roles.
    if let Some(existing) = existing {
        let taken: BTreeSet<ResourceId> = terminals
            .iter()
            .filter_map(Terminal::functions)
            .flat_map(|f| f.functions.iter().filter_map(|x| x.resource_role))
            .map(|rr| rr.resource)
            .collect();
        for old in &existing.resources {
            if !resources.iter().any(|r| r.id == old.id) && taken.contains(&old.id) {
                resources.push(old.clone());
            }
        }
    }
    let rules = builder.rules(accept)?;
    let before_resources = existing.map_or(&[][..], |c| c.resources.as_slice());
    for r in &resources {
        match before_resources.iter().find(|x| x.id == r.id) {
            None => out.changes.push(change(
                ChangeKind::Added,
                "peripheral",
                None,
                None,
                format!("Peripheral ‘{}’ added", r.label),
            )),
            Some(x) if x != r => out.changes.push(change(
                ChangeKind::Changed,
                "peripheral",
                None,
                None,
                format!("Peripheral ‘{}’ changed", r.label),
            )),
            Some(_) => {}
        }
    }
    for x in before_resources {
        if !resources.iter().any(|r| r.id == x.id) {
            out.changes.push(change(
                ChangeKind::Removed,
                "peripheral",
                None,
                None,
                format!("Peripheral ‘{}’ removed", x.label),
            ));
        }
    }
    let before_rules = existing.map_or(&[][..], |c| c.pin_rules.as_slice());
    for r in &rules {
        let row = format!("{RULE_ROW_PREFIX}{}", r.label);
        match before_rules.iter().find(|x| x.id == r.id) {
            None => out.changes.push(change(
                ChangeKind::Added,
                "rule",
                Some(&row),
                None,
                format!("Rule ‘{}’ added", r.label),
            )),
            Some(x) if x != r => out.changes.push(change(
                ChangeKind::Changed,
                "rule",
                Some(&row),
                None,
                format!("Rule ‘{}’ changed", r.label),
            )),
            Some(_) => {}
        }
    }
    for x in before_rules {
        if !rules.iter().any(|r| r.id == x.id) {
            out.changes.push(change(
                ChangeKind::Removed,
                "rule",
                Some(&format!("{RULE_ROW_PREFIX}{}", x.label)),
                None,
                format!("Rule ‘{}’ removed", x.label),
            ));
        }
    }
    if let Some(existing) = existing {
        for old in &existing.terminals {
            if !terminals.iter().any(|t| t.id == old.id) {
                out.changes.push(change(
                    ChangeKind::Removed,
                    "terminal",
                    None,
                    Some(old.id),
                    format!("Terminal ‘{}’ removed", old.name),
                ));
            }
        }
    }
    let present: BTreeSet<TerminalId> = terminals.iter().map(|t| t.id).collect();
    review.unreviewed.retain(|t| present.contains(t));
    review.uncertain.retain(|t| present.contains(t));
    out.findings.append(&mut builder.findings);
    let base = existing.cloned().unwrap_or_else(|| ComponentDefinition {
        designator_prefix: "U".into(),
        ..ComponentDefinition::default()
    });
    Ok(ComponentDefinition {
        next_local: builder.locals.0,
        terminals,
        resources,
        pin_rules: rules,
        review: (!review.is_empty()).then(|| Box::new(review)),
        ..base
    })
}

/// Rows the document flags, and rows numbered in some variants only.
fn document_review(document: &PinoutDocument, accept: &PinoutAccept, out: &mut StagedImport) {
    for pin in document.pins.iter().filter(|p| accepted(accept, &p.row)) {
        if pin.uncertain {
            out.review.push(ReviewItem {
                row: pin.row.clone(),
                reason: ReviewReason::Uncertain,
                message: match &pin.note {
                    Some(note) => format!("‘{}’ is flagged uncertain: {note}", pin.name),
                    None => format!("‘{}’ is flagged uncertain.", pin.name),
                },
            });
        }
        let missing: Vec<&str> = document
            .variants
            .iter()
            .filter(|v| !pin.number.contains_key(&v.name))
            .map(|v| v.name.as_str())
            .collect();
        if !missing.is_empty() && missing.len() < document.variants.len() {
            out.review.push(ReviewItem {
                row: pin.row.clone(),
                reason: ReviewReason::VariantsDisagree,
                message: format!("‘{}’ has no number in {}.", pin.name, missing.join(", ")),
            });
        }
    }
}

/// Numbers used twice in one variant, and variants whose stated terminal
/// count disagrees with the rows numbered in them.
fn number_findings(document: &PinoutDocument, component: DefinitionId, out: &mut StagedImport) {
    let this = vec![ObjectRef::Definition { id: component }];
    for variant in &document.variants {
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        let mut count = 0usize;
        for pin in &document.pins {
            let Some(number) = pin.number.get(&variant.name) else {
                continue;
            };
            count += 1;
            if let Some(first) = seen.insert(number, &pin.row) {
                out.findings.push(Finding::blocking(
                    "package.designation",
                    format!(
                        "Rows ‘{first}’ and ‘{}’ both use number {number} in {}.",
                        pin.row, variant.name
                    ),
                    this.clone(),
                    Stage::Publish,
                ));
            }
        }
        if count != variant.terminal_count as usize {
            out.findings.push(Finding::blocking(
                "package.terminal_count",
                format!(
                    "{} states {} terminals but {count} rows are numbered in it.",
                    variant.name, variant.terminal_count
                ),
                this.clone(),
                Stage::Publish,
            ));
        }
    }
}

/// The provenance text of an imported draft.
fn provenance(document: &PinoutDocument) -> String {
    let reference = &document.device.reference;
    let mut text = format!("Imported from a typed pinout of ‘{}’", reference.title);
    if let Some(revision) = &reference.document_revision {
        text.push_str(&format!(", revision {revision}"));
    }
    if let Some(url) = &reference.url {
        text.push_str(&format!(" ({url})"));
    }
    text.push_str(". Imported rows stay unreviewed until a person confirms them.");
    text.chars().take(MAX_NOTE / 4).collect()
}

/// The generated block symbol, wide enough for the pin names.
fn block_symbol(component: DefinitionId, terminals: &[Terminal]) -> SymbolDefinition {
    let named: Vec<(TerminalId, &str)> =
        terminals.iter().map(|t| (t.id, t.name.as_str())).collect();
    crate::library::samples::named_block_symbol(component, &named)
}

/// Stage an import. `allocate` reserves a new lineage; it is called only
/// for a [`PinoutTarget::New`] target, in a fixed order (component,
/// symbol, then a package and a binding per variant).
pub fn stage(
    library: &Library,
    drafts: &Drafts,
    target: &PinoutTarget,
    document: &PinoutDocument,
    accept: &PinoutAccept,
    allocate: &mut dyn FnMut() -> Result<DefinitionId>,
) -> Result<StagedImport> {
    check_document(document, accept)?;
    let mut out = StagedImport::default();
    match target {
        PinoutTarget::Draft { id } => {
            let draft = drafts
                .get(*id)
                .ok_or_else(|| Error::NotFound("That draft no longer exists".into()))?;
            let Definition::Component(existing) = &draft.definition else {
                return Err(Error::Invalid(
                    "a pinout imports into a component draft".into(),
                ));
            };
            let matched = match_rows(library, drafts, *id, existing, document);
            // Without a binding, terminal identity was kept by equal name
            // only: never trusted silently.
            if numbered_terminals(library, drafts, *id, existing).is_empty() {
                for pin in document
                    .pins
                    .iter()
                    .filter(|p| matched.contains_key(&p.row))
                {
                    out.review.push(ReviewItem {
                        row: pin.row.clone(),
                        reason: ReviewReason::MatchedByName,
                        message: format!(
                            "The draft has no binding, so this row kept terminal ‘{}’ only because the names are equal; confirm it is the same pin.",
                            pin.name
                        ),
                    });
                }
            }
            number_findings(document, *id, &mut out);
            document_review(document, accept, &mut out);
            let component =
                build_component(*id, document, accept, Some(existing), &matched, &mut out)?;
            let mut meta = draft.meta.clone();
            if meta.provenance.trim().is_empty() {
                meta.provenance = provenance(document);
            }
            // Renamed pins may no longer fit a box drawn for the old
            // names ("RESET" over "USB_DM"): the component's symbol drafts
            // widen to fit them, in the same import.
            let mut symbols = Vec::new();
            for representation in &component.representations {
                let Some(symbol_draft) = drafts.get(representation.id) else {
                    continue;
                };
                let Definition::Symbol(symbol) = &symbol_draft.definition else {
                    continue;
                };
                let mut symbol = symbol.clone();
                let name = |terminal: TerminalId| {
                    component
                        .terminals
                        .iter()
                        .find(|t| t.id == terminal)
                        .map(|t| t.name.clone())
                };
                if crate::library::samples::fit_pin_names(&mut symbol, name) {
                    symbols.push(StagedDraft {
                        id: representation.id,
                        create: false,
                        meta: symbol_draft.meta.clone(),
                        definition: Definition::Symbol(symbol),
                    });
                }
            }
            out.drafts.push(StagedDraft {
                id: *id,
                create: false,
                meta,
                definition: Definition::Component(component),
            });
            out.drafts.extend(symbols);
        }
        PinoutTarget::New { name, row } => {
            if !text_ok(name, MAX_LABEL) {
                return Err(Error::Invalid(
                    "a new component needs a name of 1–256 bytes".into(),
                ));
            }
            let forms = variant_forms(library, document, *row)?;
            let id = allocate()?;
            let symbol_id = allocate()?;
            let mut lineages = Vec::new();
            for _ in &document.variants {
                lineages.push((allocate()?, allocate()?));
            }
            number_findings(document, id, &mut out);
            document_review(document, accept, &mut out);
            let mut component =
                build_component(id, document, accept, None, &BTreeMap::new(), &mut out)?;
            let symbol = SymbolRef::new(symbol_id, Revision::FIRST);
            component.representations = vec![symbol];
            component.implementations = lineages
                .iter()
                .map(|(_, binding)| BindingRef::new(*binding, Revision::FIRST))
                .collect();
            let meta = |name: String| DefinitionMeta {
                name,
                description: document.device.name.clone(),
                provenance: provenance(document),
            };
            let mut staged = vec![
                StagedDraft {
                    id,
                    create: true,
                    meta: meta(name.clone()),
                    definition: Definition::Component(component.clone()),
                },
                StagedDraft {
                    id: symbol_id,
                    create: true,
                    meta: meta(format!("{name} symbol")),
                    definition: Definition::Symbol(block_symbol(id, &component.terminals)),
                },
            ];
            // Rows map to terminals in document order; an excluded row has
            // no terminal and so no package terminal.
            let rows: Vec<(&PinoutPin, &Terminal)> = document
                .pins
                .iter()
                .filter(|p| accepted(accept, &p.row))
                .zip(&component.terminals)
                .collect();
            for ((variant, form), (package_id, binding_id)) in
                document.variants.iter().zip(forms).zip(&lineages)
            {
                let mut terminals = Vec::new();
                let mut mappings = Vec::new();
                for (pin, terminal) in &rows {
                    let Some(number) = pin.number.get(&variant.name) else {
                        continue;
                    };
                    let next = PackageTerminalId(
                        u32::try_from(terminals.len() + 1)
                            .map_err(|_| Error::Limit("too many package terminals".into()))?,
                    );
                    terminals.push(PackageTerminal {
                        id: next,
                        designation: number.clone(),
                    });
                    mappings.push(TerminalMap {
                        package_terminal: next,
                        terminal: terminal.id,
                    });
                    out.changes.push(change(
                        ChangeKind::Added,
                        "package_terminal",
                        Some(&pin.row),
                        Some(terminal.id),
                        format!("{} terminal {number} carries ‘{}’", variant.name, pin.name),
                    ));
                }
                let next_local = u32::try_from(terminals.len() + 1)
                    .map_err(|_| Error::Limit("too many package terminals".into()))?;
                let envelope = form.envelope().unwrap_or_default();
                staged.push(StagedDraft {
                    id: *package_id,
                    create: true,
                    meta: meta(format!("{name} {}", variant.name)),
                    definition: Definition::Package(PackageDefinition {
                        next_local,
                        terminals,
                        form,
                        envelope,
                        ..PackageDefinition::default()
                    }),
                });
                staged.push(StagedDraft {
                    id: *binding_id,
                    create: true,
                    meta: meta(format!("{name} {} binding", variant.name)),
                    definition: Definition::Binding(BindingDefinition {
                        component: id,
                        package: PackageRef::new(*package_id, Revision::FIRST),
                        mappings,
                        footprints: Vec::new(),
                        unavailable: Vec::new(),
                    }),
                });
            }
            out.drafts = staged;
        }
    }
    Ok(out)
}

/// The package form of each variant: its own row, else (first variant
/// only) the target's row, else generic.
fn variant_forms(
    library: &Library,
    document: &PinoutDocument,
    target_row: Option<crate::ids::PackageRowRef>,
) -> Result<Vec<PackageForm>> {
    document
        .variants
        .iter()
        .enumerate()
        .map(|(k, variant)| {
            let row = variant
                .package_row
                .or(if k == 0 { target_row } else { None });
            match row {
                None => Ok(PackageForm::Generic),
                Some(r) => library
                    .package_row(r)
                    .map(|p| p.definition.values.clone())
                    .ok_or_else(|| {
                        Error::NotFound(format!(
                            "The package row of variant ‘{}’ is not in the project library",
                            variant.name
                        ))
                    }),
            }
        })
        .collect()
}
