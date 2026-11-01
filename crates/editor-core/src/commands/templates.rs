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
    self, FamilyKind, NewFromTemplate, PackageFromTemplate, PackagePlan, RowFrom, SavePackageRow,
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
    let edits = catalog::new_from_template(&project.library, &request, refs)?;
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
/// one landing footprint draft's pads and drawings, as one step.
pub(crate) fn package_from_template(
    document: &mut Document,
    request: PackageFromTemplate,
    _evaluator: Option<&dyn FootprintEvaluator>,
) -> Result<Outcome> {
    let (edits, plan) = candidate(document, &request)?;
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
    candidate(document, request).map(|(_, plan)| plan)
}

/// The draft edits of `request` and their plan, without changing anything.
fn candidate(
    document: &Document,
    request: &PackageFromTemplate,
) -> Result<(Vec<DraftEdit>, PackagePlan)> {
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
    let family_rev = FamilyKind::of(&form).and_then(FamilyKind::family_rev);
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
    let mut next = replaced_package(package, form)?;
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
}

fn finish(
    document: &Document,
    request: &PackageFromTemplate,
    replacement: Replacement<'_>,
) -> Result<(Vec<DraftEdit>, PackagePlan)> {
    let Replacement {
        current,
        before,
        next,
        landing,
        as_row,
        resolved,
        family_rev,
    } = replacement;
    let mut findings = Vec::new();
    let mut edits = Vec::new();
    let mut meta = current.meta.clone();
    meta.provenance = catalog::provenance(resolved, as_row);
    let mut pads_after = 0;
    if let Some((draft, footprint)) = landing {
        let lands = resolved.row.lands.as_ref().filter(|_| as_row);
        let rebuilt = rebuilt_footprint(footprint, &next, lands)?;
        pads_after = u32::try_from(rebuilt.pads.len()).unwrap_or(u32::MAX);
        findings.extend(package_fit::placement_findings(&next, &rebuilt, draft.id));
        let mut edit = vec![DraftEdit {
            id: draft.id,
            meta: draft.meta.clone(),
            definition: Definition::Footprint(rebuilt),
        }];
        if next.terminals.len() > 2 {
            samples::mark_pin_one(&mut edit);
        }
        edits.extend(edit);
    }
    findings.extend(mapping_findings(document, request.draft, &next));
    findings.extend(retained_findings(document, request.draft, before, &next));
    let plan = PackagePlan {
        terminals_before: u32::try_from(before.terminals.len()).unwrap_or(u32::MAX),
        terminals_after: u32::try_from(next.terminals.len()).unwrap_or(u32::MAX),
        pads_after,
        model_family_rev: family_rev,
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
    Ok((edits, plan))
}

/// `package` with `form`: its envelope, and its terminals for the form's
/// count. Terminals the new count keeps keep their identities, in order;
/// terminals beyond it are removed; new ones get fresh identities. Every
/// terminal is designated from the new form ([`samples::form_designations`]:
/// lead numbers then `EP`, or a grid's ball names), so designations never
/// repeat. Mappings change only through `library.assign_terminals` and
/// binding edits.
fn replaced_package(
    package: &PackageDefinition,
    form: crate::library::PackageForm,
) -> Result<PackageDefinition> {
    let count = samples::form_terminal_count(&form);
    let mut next = package.clone();
    let keep = usize::try_from(count)
        .unwrap_or(usize::MAX)
        .min(next.terminals.len());
    next.terminals.truncate(keep);
    while next.terminals.len() < usize::try_from(count).unwrap_or(0) {
        let id = PackageTerminalId(next.next_local);
        next.next_local = next
            .next_local
            .checked_add(1)
            .ok_or_else(|| Error::Limit("package local identities exhausted".into()))?;
        next.terminals.push(PackageTerminal {
            id,
            designation: String::new(),
        });
    }
    let designations = samples::form_designations(&form, next.terminals.len());
    for (terminal, designation) in next.terminals.iter_mut().zip(designations) {
        terminal.designation = designation;
    }
    next.envelope = form.envelope().unwrap_or(next.envelope);
    next.form = form;
    Ok(next)
}

/// `footprint` with its template content replaced: form-derived lands for
/// `package`'s terminals in lead order (with cited land overrides), new
/// pad identities from its counter, and the standard drawings. Authored
/// mask and paste defaults and the datum stay; identities never repeat.
fn rebuilt_footprint(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    lands: Option<&catalog::LandValues>,
) -> Result<FootprintDefinition> {
    let mut next = footprint_family::cleared(footprint);
    next.graphics.clear();
    let terminals: Vec<PackageTerminalId> = package.terminals.iter().map(|t| t.id).collect();
    let mut pads = samples::template_lands(&package.form, &terminals, lands).ok_or_else(|| {
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

/// Advice when the package changes family or lead count but a binding
/// draft keeps mapping retained terminals: those terminals are reused by
/// position (`binding.mapping_retained`), so each mapping should be
/// checked against the new pinout before publishing.
fn retained_findings(
    document: &Document,
    package: DefinitionId,
    before: &PackageDefinition,
    next: &PackageDefinition,
) -> Vec<Finding> {
    let family = |p: &PackageDefinition| catalog::FamilyKind::of(&p.form);
    if family(before) == family(next) && before.terminals.len() == next.terminals.len() {
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
                    "{kept} mapping(s) of ‘{}’ keep package terminals by position while the package changes from {} terminal(s) ({}) to {} ({}); check each against the new pinout.",
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
