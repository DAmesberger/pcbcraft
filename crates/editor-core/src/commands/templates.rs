//! Slice B1: handlers of `library.new_from_template`,
//! `library.package_from_template` and `library.save_package_row`, and the
//! `library.package_plan` preview. The dispatch arms in `library.rs` and
//! the crate root call these. Each command is one engine operation, so it
//! is one undo step, and it works on the engine's candidate document, so a
//! refusal leaves the document unchanged.
use super::Outcome;
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{
    BindingRef, DefinitionId, FootprintRef, PackageRef, PackageTerminalId, PadId, Revision,
    SymbolRef,
};
use crate::library::package_fit;
use crate::library::samples::{self, SampleRefs};
use crate::library::templates::catalog::{
    self, FamilyKind, NewFromTemplate, PackageFromTemplate, PackagePlan, Renumber, RowFrom,
    SavePackageRow, TerminalMatch, TerminalMatchReport,
};
use crate::library::templates::footprint;
use crate::library::templates::footprint_family;
use crate::library::{
    Definition, DraftEdit, FootprintDefinition, FootprintEvaluator, GeneratedMaterials,
    ModelPlacement, PackageDefinition, PackageModel, PackageTerminal, drafts,
};
use crate::model::Document;
use std::sync::Arc;

/// `library.new_from_template`: five drafts in one undo step
/// (`Outcome::DraftsCreated`, component first). The drafts are new
/// lineages; nothing is published or imported.
pub(crate) fn new_from_template(
    document: &mut Document,
    request: NewFromTemplate,
    _evaluator: Option<&dyn FootprintEvaluator>,
) -> Result<Outcome> {
    let project = Arc::make_mut(&mut document.project);
    let mut ids = [DefinitionId(0); catalog::NEW_FROM_TEMPLATE_DRAFTS];
    for id in &mut ids {
        *id = DefinitionId(project.allocate()?);
    }
    let [component, symbol, package, footprint, binding] = ids;
    let refs = SampleRefs {
        component,
        symbol: SymbolRef::new(symbol, Revision::FIRST),
        package: PackageRef::new(package, Revision::FIRST),
        footprint: FootprintRef::new(footprint, Revision::FIRST),
        binding: BindingRef::new(binding, Revision::FIRST),
    };
    let rules = project.rules.effective();
    let edits = catalog::new_from_template_with(
        &project.library,
        &request,
        refs,
        &rules,
        document.drafts.symbols,
    )?;
    let drafts = Arc::make_mut(&mut document.drafts);
    for edit in edits {
        drafts::create(
            drafts,
            &project.library,
            edit.id,
            None,
            edit.meta,
            edit.definition,
        )?;
    }
    Ok(Outcome::DraftsCreated { ids: ids.to_vec() })
}

/// `library.package_from_template` (`Outcome::PackageReplaced`): the
/// package draft's form, terminals, envelope and generated model, and its
/// one landing footprint draft's pads and drawings, as one step. When no
/// footprint draft lands the package, the step also creates one and lists
/// it in the package's binding draft when exactly one binding draft
/// implements the package (else `binding.footprint_unbound` says which to
/// edit).
pub(crate) fn package_from_template(
    document: &mut Document,
    request: PackageFromTemplate,
    _evaluator: Option<&dyn FootprintEvaluator>,
) -> Result<Outcome> {
    let fresh = DefinitionId(document.project.next_id);
    let Candidate {
        edits,
        created,
        plan,
    } = candidate(document, &request, fresh)?;
    if let Some(created) = created {
        let project = Arc::make_mut(&mut document.project);
        let id = DefinitionId(project.allocate()?);
        if id != created.id {
            return Err(Error::Conflict(
                "template.footprint: the footprint identity changed while it was planned".into(),
            ));
        }
        drafts::create(
            Arc::make_mut(&mut document.drafts),
            &project.library,
            created.id,
            None,
            created.meta,
            created.definition,
        )?;
    }
    drafts::edit(Arc::make_mut(&mut document.drafts), edits)?;
    Ok(Outcome::PackageReplaced { plan })
}

/// `library.save_package_row` (`Outcome::DraftCreated`): a new package-row
/// draft from a package draft's form or from explicit values.
pub(crate) fn save_package_row(
    document: &mut Document,
    request: SavePackageRow,
) -> Result<Outcome> {
    let values = match &request.from {
        RowFrom::Values { values } => values.clone(),
        RowFrom::Package { draft } => {
            let current = document
                .drafts
                .get(*draft)
                .ok_or_else(|| Error::NotFound("That draft no longer exists".into()))?;
            let Definition::Package(package) = &current.definition else {
                return Err(Error::Invalid(format!(
                    "The draft ‘{}’ is not a package",
                    current.meta.name
                )));
            };
            package.form.clone()
        }
    };
    let (meta, definition) = catalog::row_draft(&request, values)?;
    let project = Arc::make_mut(&mut document.project);
    let id = DefinitionId(project.allocate()?);
    let drafts = Arc::make_mut(&mut document.drafts);
    drafts::create(drafts, &project.library, id, None, meta, definition)?;
    Ok(Outcome::DraftCreated { id })
}

/// `library.package_plan`: what `package_from_template` would change.
pub(crate) fn package_plan(
    document: &Document,
    request: &PackageFromTemplate,
    _evaluator: Option<&dyn FootprintEvaluator>,
) -> Result<PackagePlan> {
    candidate(document, request, DefinitionId(document.project.next_id)).map(|c| c.plan)
}

/// What `package_from_template` would change: the edits of existing
/// drafts (package first), the footprint draft it creates, if any, and
/// the plan.
struct Candidate {
    edits: Vec<DraftEdit>,
    created: Option<DraftEdit>,
    plan: PackagePlan,
}

/// The draft edits of `request` and their plan, without changing anything.
/// `fresh` is the identity a created footprint draft would take.
fn candidate(
    document: &Document,
    request: &PackageFromTemplate,
    fresh: DefinitionId,
) -> Result<Candidate> {
    let current = document
        .drafts
        .get(request.draft)
        .ok_or_else(|| Error::NotFound("That draft no longer exists".into()))?;
    let Definition::Package(package) = &current.definition else {
        return Err(Error::Invalid(format!(
            "The draft ‘{}’ is not a package",
            current.meta.name
        )));
    };
    let resolved = catalog::resolve_row(&document.project.library, &request.row)?;
    let (form, as_row) = catalog::checked_values(&resolved.row, request.values.as_ref())?;
    let family_rev = FamilyKind::of(&form).map(FamilyKind::family_rev);
    let mut landing = document
        .drafts
        .items
        .iter()
        .filter_map(|d| match &d.definition {
            Definition::Footprint(f) if f.package.id == request.draft => Some((d, f)),
            _ => None,
        });
    let landing = match (landing.next(), landing.next()) {
        (None, _) => None,
        (Some(one), None) => Some(one),
        (Some(_), Some(_)) => {
            return Err(Error::Invalid(format!(
                "package.footprint_ambiguous: several footprint drafts land the package ‘{}’; open one working set",
                current.meta.name
            )));
        }
    };
    if let Some((draft, footprint)) = landing
        && !request.replace
        && (!footprint.pads.is_empty() || footprint.source.is_some())
    {
        return Err(Error::Conflict(format!(
            "footprint.template_not_empty: the footprint ‘{}’ already has pads; pass replace to rebuild it from the template",
            draft.meta.name
        )));
    }
    let floor = package_floor(document, current, package);
    let replaced = replaced_package(package, form, floor, request.terminals, &request.renumber)?;
    // A package without terminals has nothing to match.
    let unmatched = if package.terminals.is_empty() {
        None
    } else {
        unmatched_finding(request.draft, &replaced)
    };
    let terminals = replaced.report;
    let mut next = replaced.package;
    if let Some(rev) = family_rev {
        // The generated model of the new family; a family without a
        // builder keeps its model.
        let materials = match &package.model {
            PackageModel::Generated { materials, .. } => materials.clone(),
            _ => GeneratedMaterials::default(),
        };
        next.model = PackageModel::Generated {
            family_rev: rev,
            materials,
        };
        next.placement = ModelPlacement::default();
    }
    finish(
        document,
        request,
        Replacement {
            current,
            before: package,
            next,
            landing,
            as_row,
            resolved: &resolved,
            family_rev,
            fresh,
            terminals,
            unmatched,
        },
    )
}

struct Replacement<'a> {
    current: &'a crate::library::Draft,
    before: &'a PackageDefinition,
    next: PackageDefinition,
    landing: Option<(&'a crate::library::Draft, &'a FootprintDefinition)>,
    as_row: bool,
    resolved: &'a catalog::ResolvedRow,
    family_rev: Option<u16>,
    fresh: DefinitionId,
    terminals: TerminalMatchReport,
    unmatched: Option<Finding>,
}

fn finish(
    document: &Document,
    request: &PackageFromTemplate,
    replacement: Replacement<'_>,
) -> Result<Candidate> {
    let Replacement {
        current,
        before,
        next,
        landing,
        as_row,
        resolved,
        family_rev,
        fresh,
        terminals,
        unmatched,
    } = replacement;
    let mut findings = Vec::new();
    let mut edits = Vec::new();
    let mut meta = current.meta.clone();
    meta.provenance = catalog::provenance(resolved, as_row);
    let lands = resolved.row.lands.as_ref().filter(|_| as_row);
    let (footprint_id, footprint_meta, footprint, floor, create) = match landing {
        Some((draft, footprint)) => (
            draft.id,
            draft.meta.clone(),
            footprint.clone(),
            footprint_floor(document, draft, footprint),
            false,
        ),
        None => {
            let revision = current.pending().ok_or_else(|| {
                Error::Limit(format!(
                    "the package ‘{}’ has no next revision",
                    current.meta.name
                ))
            })?;
            let footprint = FootprintDefinition {
                package: PackageRef::new(request.draft, revision),
                next_local: 1,
                ..FootprintDefinition::default()
            };
            let mut footprint_meta = meta.clone();
            footprint_meta.name = footprint_name(&current.meta.name);
            (fresh, footprint_meta, footprint, 1, true)
        }
    };
    let rules = document.project.rules.effective();
    let mut rebuilt = rebuilt_footprint(&footprint, &next, lands, floor, &rules)?;
    let pads_after = u32::try_from(rebuilt.pads.len()).unwrap_or(u32::MAX);
    findings.extend(package_fit::placement_findings(
        &next,
        &rebuilt,
        footprint_id,
    ));
    if next.terminals.len() > 2 {
        // The pin-1 pad lands the first terminal in lead order (pad
        // identities start at the footprint's floor, not at 1).
        let first = next.terminals.first().map(|t| t.id);
        if let Some(pad) = rebuilt
            .pads
            .iter()
            .find(|p| p.terminal == first)
            .map(|p| p.id)
        {
            samples::mark_polarity(&mut rebuilt, pad);
        }
    }
    let footprint_edit = DraftEdit {
        id: footprint_id,
        meta: footprint_meta,
        definition: Definition::Footprint(rebuilt),
    };
    let mut bound = None;
    let created = if create {
        let reference = FootprintRef::new(footprint_id, Revision::FIRST);
        match bind_created(document, request.draft, reference) {
            Ok(edit) => {
                bound = Some(edit.id);
                edits.push(edit);
            }
            Err(finding) => findings.push(finding),
        }
        Some(footprint_edit)
    } else {
        edits.push(footprint_edit);
        None
    };
    findings.extend(unmatched);
    findings.extend(mapping_findings(document, request.draft, &next));
    if terminals.strategy == TerminalMatch::ByPosition {
        findings.extend(retained_findings(
            document,
            request.draft,
            before,
            &next,
            terminals.renumbered,
        ));
    }
    let plan = PackagePlan {
        terminals_before: u32::try_from(before.terminals.len()).unwrap_or(u32::MAX),
        terminals_after: u32::try_from(next.terminals.len()).unwrap_or(u32::MAX),
        pads_after,
        model_family_rev: family_rev,
        footprint: Some(footprint_id),
        footprint_created: created.is_some(),
        footprint_bound: bound,
        terminals,
        findings,
    };
    edits.insert(
        0,
        DraftEdit {
            id: request.draft,
            meta,
            definition: Definition::Package(next),
        },
    );
    Ok(Candidate {
        edits,
        created,
        plan,
    })
}

/// The name of a footprint draft created for the package `name`, within
/// the 256-byte name bound.
fn footprint_name(name: &str) -> String {
    const SUFFIX: &str = " footprint";
    let mut end = name.len().min(256 - SUFFIX.len());
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{SUFFIX}", &name[..end])
}

/// The binding-draft edit that lists a created footprint, when exactly one
/// binding draft implements `package`; else the finding that says which
/// binding to edit (none, or several to choose from).
fn bind_created(
    document: &Document,
    package: DefinitionId,
    footprint: FootprintRef,
) -> std::result::Result<DraftEdit, Finding> {
    let bindings: Vec<_> = document
        .drafts
        .items
        .iter()
        .filter_map(|d| match &d.definition {
            Definition::Binding(b) if b.package.id == package => Some((d, b)),
            _ => None,
        })
        .collect();
    if let [(draft, binding)] = bindings.as_slice() {
        let mut body = (*binding).clone();
        body.footprints.push(footprint);
        return Ok(DraftEdit {
            id: draft.id,
            meta: draft.meta.clone(),
            definition: Definition::Binding(body),
        });
    }
    let mut objects = vec![ObjectRef::Definition { id: footprint.id }];
    let message = if bindings.is_empty() {
        "No binding draft implements the package; list the new footprint in the binding that does before publishing.".to_owned()
    } else {
        let names: Vec<&str> = bindings.iter().map(|(d, _)| d.meta.name.as_str()).collect();
        objects.extend(
            bindings
                .iter()
                .map(|(d, _)| ObjectRef::Definition { id: d.id }),
        );
        format!(
            "{} binding drafts implement the package ({}); list the new footprint in the ones that use it.",
            bindings.len(),
            names.join(", ")
        )
    };
    Err(Finding::new("binding.footprint_unbound", message, objects))
}

/// The first local identity a package draft may allocate: above its
/// counter, its terminals and its base revision's counter, and never 0.
fn package_floor(
    document: &Document,
    draft: &crate::library::Draft,
    package: &PackageDefinition,
) -> u32 {
    let base = draft
        .base
        .and_then(|revision| {
            document
                .project
                .library
                .package(PackageRef::new(draft.id, revision))
        })
        .map_or(0, |p| p.definition.next_local);
    floor(
        package.next_local,
        base,
        package.terminals.iter().map(|t| t.id.0),
    )
}

/// The first local identity a footprint draft may allocate once it is
/// cleared: above its counter, every identity it holds now (they retire
/// with the cleared pads and drawings) and its base revision's counter,
/// and never 0.
fn footprint_floor(
    document: &Document,
    draft: &crate::library::Draft,
    footprint: &FootprintDefinition,
) -> u32 {
    let base = draft
        .base
        .and_then(|revision| {
            document
                .project
                .library
                .footprint(FootprintRef::new(draft.id, revision))
        })
        .map_or(0, |p| p.definition.next_local);
    let arrays = footprint
        .source
        .as_deref()
        .into_iter()
        .flat_map(|s| s.arrays.iter().map(|a| a.id.0));
    let used = footprint
        .pads
        .iter()
        .map(|p| p.id.0)
        .chain(footprint.graphics.iter().map(|g| g.id.0))
        .chain(arrays);
    floor(footprint.next_local, base, used)
}

/// The largest of the counter, the base counter, one past every used
/// identity and 1, saturating: an exhausted space then refuses at the
/// first allocation and the document stays unchanged.
fn floor(counter: u32, base: u32, used: impl Iterator<Item = u32>) -> u32 {
    used.map(|id| id.saturating_add(1))
        .fold(counter.max(base).max(1), u32::max)
}

/// The package's terminals after a form change, and how they matched.
struct Replaced {
    package: PackageDefinition,
    report: TerminalMatchReport,
    /// Numbers of the new form no current terminal carried (fresh
    /// terminals), in lead order.
    added: Vec<String>,
    /// Numbers of current terminals the new package removes.
    removed: Vec<String>,
}

/// `package` with `form`: its envelope, and its terminals for the form's
/// count, each designated from the new form
/// ([`samples::form_designations`]: lead numbers then `EP`, or a grid's
/// ball names) and listed in lead order. `strategy` says which current
/// terminals carry over (their identities, and so the binding mappings
/// that name them): [`TerminalMatch::ByNumber`] keeps a terminal whose
/// designation the new form gives and nothing else;
/// [`TerminalMatch::ByPosition`] (only when asked) keeps the first
/// terminals in list order and renumbers them. Other new terminals get
/// fresh identities. Mappings change only through
/// `library.assign_terminals` and binding edits.
fn replaced_package(
    package: &PackageDefinition,
    form: crate::library::PackageForm,
    floor: u32,
    strategy: TerminalMatch,
    renumber: &[Renumber],
) -> Result<Replaced> {
    let count = usize::try_from(samples::form_terminal_count(&form)).unwrap_or(0);
    let designations = samples::form_designations(&form, count);
    check_renumber(package, &designations, strategy, renumber)?;
    // The current number a new number takes its terminal from: itself,
    // unless the request renumbers another one to it.
    let source = |designation: &str| -> String {
        renumber
            .iter()
            .find(|r| r.to.trim() == designation)
            .map_or_else(|| designation.to_owned(), |r| r.from.trim().to_owned())
    };
    let renamed = |current: &str| renumber.iter().any(|r| r.from.trim() == current);
    let mut next = package.clone();
    let mut local = floor;
    let mut fresh = || -> Result<PackageTerminalId> {
        let id = PackageTerminalId(local);
        local = local
            .checked_add(1)
            .ok_or_else(|| Error::Limit("package local identities exhausted".into()))?;
        Ok(id)
    };
    let mut terminals = Vec::with_capacity(count);
    let mut added = Vec::new();
    let mut renumbered = 0u32;
    let mut used = vec![false; package.terminals.len()];
    for (k, designation) in designations.iter().enumerate() {
        let found = match strategy {
            TerminalMatch::ByNumber => {
                let wanted = source(designation);
                package.terminals.iter().enumerate().position(|(j, t)| {
                    let current = t.designation.trim();
                    !used[j]
                        && !wanted.is_empty()
                        && current == wanted
                        // A renumbered terminal matches only its new number.
                        && (wanted != *designation || !renamed(current))
                })
            }
            TerminalMatch::ByPosition => (k < package.terminals.len()).then_some(k),
        };
        let id = match found.and_then(|j| package.terminals.get(j).map(|t| (j, t))) {
            Some((j, t)) => {
                used[j] = true;
                if t.designation.trim() != designation {
                    renumbered = renumbered.saturating_add(1);
                }
                t.id
            }
            None => {
                added.push(designation.clone());
                fresh()?
            }
        };
        terminals.push(PackageTerminal {
            id,
            designation: designation.clone(),
        });
    }
    next.next_local = local;
    let removed: Vec<String> = package
        .terminals
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|(t, _)| t.designation.clone())
        .collect();
    let to_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let kept = to_u32(used.iter().filter(|u| **u).count());
    let mut summary = format!(
        "Terminals matched {}: {kept} kept, {} added, {} removed",
        strategy.label(),
        added.len(),
        removed.len()
    );
    match strategy {
        TerminalMatch::ByPosition => summary.push_str(&format!(", {renumbered} renumbered")),
        TerminalMatch::ByNumber if renumbered > 0 => {
            let pairs: Vec<String> = renumber
                .iter()
                .map(|r| format!("{} → {}", r.from.trim(), r.to.trim()))
                .collect();
            summary.push_str(&format!(
                ", {renumbered} renumbered as requested ({})",
                pairs.join(", ")
            ));
        }
        TerminalMatch::ByNumber => {}
    }
    summary.push('.');
    let report = TerminalMatchReport {
        strategy,
        kept,
        added: to_u32(added.len()),
        removed: to_u32(removed.len()),
        renumbered,
        summary,
    };
    next.terminals = terminals;
    next.envelope = form.envelope().unwrap_or(next.envelope);
    next.form = form;
    Ok(Replaced {
        package: next,
        report,
        added,
        removed,
    })
}

/// Refuses a `renumber` list that is not an explicit one-to-one map from
/// current numbers to numbers of the new form, or that comes with
/// matching by position.
fn check_renumber(
    package: &PackageDefinition,
    designations: &[String],
    strategy: TerminalMatch,
    renumber: &[Renumber],
) -> Result<()> {
    if renumber.is_empty() {
        return Ok(());
    }
    if strategy == TerminalMatch::ByPosition {
        return Err(Error::Invalid(
            "template.renumber: renumber goes with terminals by_number".into(),
        ));
    }
    if renumber.len() > package.terminals.len() {
        return Err(Error::Invalid(
            "template.renumber: more entries than the package has terminals".into(),
        ));
    }
    for (k, r) in renumber.iter().enumerate() {
        let (from, to) = (r.from.trim(), r.to.trim());
        let current = package
            .terminals
            .iter()
            .filter(|t| t.designation.trim() == from)
            .count();
        if from.is_empty() || current != 1 {
            return Err(Error::Invalid(format!(
                "template.renumber: the package has no single terminal numbered ‘{from}’"
            )));
        }
        if !designations.iter().any(|d| d == to) {
            return Err(Error::Invalid(format!(
                "template.renumber: the new form has no terminal numbered ‘{to}’"
            )));
        }
        let repeated = renumber[..k]
            .iter()
            .any(|o| o.from.trim() == from || o.to.trim() == to);
        if repeated {
            return Err(Error::Invalid(format!(
                "template.renumber: ‘{from}’ or ‘{to}’ is named twice"
            )));
        }
    }
    Ok(())
}

/// At most this many numbers are listed in one finding.
const LISTED_NUMBERS: usize = 16;

/// `numbers` as a short list: the first [`LISTED_NUMBERS`], then a count.
fn number_list(numbers: &[String]) -> String {
    let shown: Vec<&str> = numbers
        .iter()
        .take(LISTED_NUMBERS)
        .map(|n| {
            if n.is_empty() {
                "(unnumbered)"
            } else {
                n.as_str()
            }
        })
        .collect();
    let mut out = shown.join(", ");
    if numbers.len() > LISTED_NUMBERS {
        out.push_str(&format!(" and {} more", numbers.len() - LISTED_NUMBERS));
    }
    out
}

/// The advisory finding naming the terminals a replacement left
/// unmatched: new numbers no current terminal carried (fresh and
/// unmapped) and current terminals the new package removes. `None` when
/// every terminal matched.
fn unmatched_finding(package: DefinitionId, replaced: &Replaced) -> Option<Finding> {
    if replaced.added.is_empty() && replaced.removed.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !replaced.added.is_empty() {
        parts.push(format!(
            "new terminal(s) {} match no current terminal and are unmapped",
            number_list(&replaced.added)
        ));
    }
    if !replaced.removed.is_empty() {
        parts.push(format!(
            "current terminal(s) {} are not in the new form and are removed",
            number_list(&replaced.removed)
        ));
    }
    Some(Finding::new(
        "package.terminals_unmatched",
        format!(
            "Terminals matched {}: {}. Nothing is filled by position: if a removed and a new number are the same pad (an exposed pad the datasheet numbers), pass renumber [{{from, to}}]; else map them explicitly.",
            replaced.report.strategy.label(),
            parts.join("; ")
        ),
        vec![ObjectRef::Definition { id: package }],
    ))
}

/// `footprint` with its template content replaced: form-derived lands for
/// `package`'s terminals in lead order (estimated under `rules`, the
/// project's effective design rules, or with cited land overrides), new
/// pad identities from `floor` (see [`footprint_floor`]), and the
/// standard drawings. Authored
/// mask and paste defaults and the datum stay; identities never repeat.
fn rebuilt_footprint(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    lands: Option<&catalog::LandValues>,
    floor: u32,
    rules: &crate::rules::DesignRules,
) -> Result<FootprintDefinition> {
    let mut next = footprint_family::cleared(footprint);
    next.graphics.clear();
    next.next_local = floor;
    let terminals: Vec<PackageTerminalId> = package.terminals.iter().map(|t| t.id).collect();
    let mut pads = samples::template_lands_with(&package.form, &terminals, lands, rules)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "template.values: the {} values give no lands for {} terminals",
                package.form.label(),
                terminals.len()
            ))
        })?;
    for pad in &mut pads {
        pad.id = PadId(next.next_local);
        next.next_local = next
            .next_local
            .checked_add(1)
            .ok_or_else(|| Error::Limit("footprint local identities exhausted".into()))?;
    }
    next.pads = pads;
    let before = next.clone();
    if footprint::add_standard_graphics(&mut next, package).is_err() {
        next = before;
    }
    Ok(next)
}

/// Advice when the request chose [`TerminalMatch::ByPosition`] and the
/// package renumbers a kept terminal or changes family or lead count
/// while a binding draft maps kept terminals: those terminals are reused
/// by position (`binding.mapping_retained`), so each mapping should be
/// checked against the new pinout before publishing.
fn retained_findings(
    document: &Document,
    package: DefinitionId,
    before: &PackageDefinition,
    next: &PackageDefinition,
    renumbered: u32,
) -> Vec<Finding> {
    let family = |p: &PackageDefinition| catalog::FamilyKind::of(&p.form);
    if renumbered == 0
        && family(before) == family(next)
        && before.terminals.len() == next.terminals.len()
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    for draft in &document.drafts.items {
        let Definition::Binding(binding) = &draft.definition else {
            continue;
        };
        if binding.package.id != package {
            continue;
        }
        let kept = binding
            .mappings
            .iter()
            .filter(|m| next.terminal(m.package_terminal).is_some())
            .count();
        if kept > 0 {
            out.push(Finding::new(
                "binding.mapping_retained",
                format!(
                    "{kept} mapping(s) of ‘{}’ keep package terminals by position, as requested, while the package changes from {} terminal(s) ({}) to {} ({}); check each against the new pinout.",
                    draft.meta.name,
                    before.terminals.len(),
                    before.form.label(),
                    next.terminals.len(),
                    next.form.label(),
                ),
                vec![ObjectRef::Definition { id: draft.id }],
            ));
        }
    }
    out
}

/// Mapping findings the new terminals give each binding draft of the
/// package draft: terminals no mapping lands (`binding.mapping_missing`)
/// and mappings of removed terminals (`binding.mapping_unknown`).
fn mapping_findings(
    document: &Document,
    package: DefinitionId,
    next: &PackageDefinition,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for draft in &document.drafts.items {
        let Definition::Binding(binding) = &draft.definition else {
            continue;
        };
        if binding.package.id != package {
            continue;
        }
        let objects = vec![ObjectRef::Definition { id: draft.id }];
        let missing = next
            .terminals
            .iter()
            .filter(|t| !binding.mappings.iter().any(|m| m.package_terminal == t.id))
            .count();
        if missing > 0 {
            out.push(Finding::blocking(
                "binding.mapping_missing",
                format!(
                    "{missing} package terminal(s) of the new package are not mapped by ‘{}’; map them explicitly.",
                    draft.meta.name
                ),
                objects.clone(),
                Stage::Publish,
            ));
        }
        let unknown = binding
            .mappings
            .iter()
            .filter(|m| next.terminal(m.package_terminal).is_none())
            .count();
        if unknown > 0 {
            out.push(Finding::blocking(
                "binding.mapping_unknown",
                format!(
                    "{unknown} mapping(s) of ‘{}’ name package terminals the new package removes.",
                    draft.meta.name
                ),
                objects,
                Stage::Publish,
            ));
        }
    }
    out
}
