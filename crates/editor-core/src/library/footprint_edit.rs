//! Editing a parametric footprint draft (`library.edit_footprint`), its
//! preview, terminal assignment and the publish re-evaluation. Every edit
//! runs on a clone; the host's [`FootprintEvaluator`] applies sketch edits
//! and solves once at the end; source and result are committed together
//! or not at all. The core never interprets the sketch.
use super::drafts::Drafts;
use super::footprint::{FootprintDefinition, PadDefaults, PadKind};
use super::footprint_source::{
    ArrayId, ArrayLayout, DatumRole, Evaluation, EvaluationRequest, FootprintEdit,
    FootprintEvaluator, FootprintParam, FootprintSource, GraphicSource, KeyedOverride, MAX_EXPR,
    MAX_FOOTPRINT_EDITS, MAX_PARAMETERS, PadArray, PadGeometry, PadRole, PasteRuleExpr, SketchEdit,
    SketchRef, commit, package_values,
};
use super::package::PackageDefinition;
use super::{Definition, Library, MAX_LABEL};
use crate::diagnostics::{Error, Finding, Result};
use crate::ids::{GraphicId, PackageRef, PackageTerminalId, PadId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// What an `EditFootprint` (or its preview) produced.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FootprintEditReport {
    /// Sketch entities the edits created, in edit order.
    pub created: Vec<SketchRef>,
    /// Pad IDs allocated (template pads, then new array instances).
    pub pads: Vec<PadId>,
    /// Graphic-role IDs allocated.
    pub graphics: Vec<GraphicId>,
    /// Array IDs allocated.
    pub arrays: Vec<ArrayId>,
    /// The evaluation's findings (advisory for a committed edit; a preview
    /// also lists the blocking ones).
    pub findings: Vec<Finding>,
}

/// A previewed edit: the footprint it would produce and its report.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FootprintPreview {
    pub definition: FootprintDefinition,
    pub report: FootprintEditReport,
    /// Whether `library.edit_footprint` would commit it.
    pub committable: bool,
}

/// The package a footprint lands: published, or the draft whose pending
/// revision it names.
pub fn footprint_package<'a>(
    library: &'a Library,
    drafts: &'a Drafts,
    r: PackageRef,
) -> Option<&'a PackageDefinition> {
    if let Some(p) = library.package(r) {
        return Some(&p.definition);
    }
    drafts.get(r.id).and_then(|d| match &d.definition {
        Definition::Package(p) if d.pending() == Some(r.revision) => Some(p),
        _ => None,
    })
}

fn allocate(next: &mut FootprintDefinition) -> Result<u32> {
    let id = next.next_local;
    next.next_local = id
        .checked_add(1)
        .ok_or_else(|| Error::Limit("footprint local identities exhausted".into()))?;
    Ok(id)
}

fn source_mut(next: &mut FootprintDefinition) -> Result<&mut FootprintSource> {
    next.source
        .as_deref_mut()
        .ok_or_else(|| Error::Invalid("the footprint was detached from its source".into()))
}

fn conflict(code: &str, message: impl std::fmt::Display) -> Error {
    Error::Conflict(format!("{code}: {message}"))
}

/// Start a source on an empty explicit footprint.
fn ensure_source(next: &mut FootprintDefinition, evaluator: &dyn FootprintEvaluator) -> Result<()> {
    if next.source.is_some() {
        return Ok(());
    }
    if !next.pads.is_empty() {
        return Err(conflict(
            "footprint.explicit",
            "this footprint has explicit pads; a parametric source starts on a footprint without pads",
        ));
    }
    next.source = Some(Box::new(FootprintSource {
        evaluator: evaluator.evaluator(),
        sketch: evaluator.new_sketch()?,
        ..FootprintSource::default()
    }));
    Ok(())
}

fn ids_of(refs: &[SketchRef], point: bool) -> BTreeSet<String> {
    refs.iter()
        .filter_map(|r| match r {
            SketchRef::Point(p) if point => Some(p.0.clone()),
            SketchRef::Curve(c) if !point => Some(c.0.clone()),
            _ => None,
        })
        .collect()
}

/// Apply a sketch edit and keep the roles consistent with what it deleted:
/// a deleted anchor retires its pad (and its remaining owned curves);
/// deleting an owned curve of a live template pad is refused.
fn sketch_edit(
    next: &mut FootprintDefinition,
    edit: &SketchEdit,
    evaluator: &dyn FootprintEvaluator,
    report: &mut FootprintEditReport,
) -> Result<()> {
    let source = source_mut(next)?;
    let outcome = evaluator.apply_sketch(&source.sketch, edit)?;
    let points = ids_of(&outcome.deleted, true);
    let curves = ids_of(&outcome.deleted, false);
    let retired: Vec<PadId> = source
        .pads
        .iter()
        .filter(|r| points.contains(&r.anchor.0))
        .map(|r| r.pad)
        .collect();
    for role in &source.pads {
        if retired.contains(&role.pad) {
            continue;
        }
        if let PadGeometry::Template { owned_curves, .. } = &role.geometry
            && let Some(c) = owned_curves.iter().find(|c| curves.contains(&c.0))
        {
            return Err(conflict(
                "footprint.owned_curve",
                format!(
                    "curve {} belongs to pad {}'s template; remove the pad or convert it to custom",
                    c.0, role.pad
                ),
            ));
        }
    }
    if let Some(a) = source.arrays.iter().find(|a| retired.contains(&a.seed)) {
        return Err(conflict(
            "footprint.array_seed",
            format!(
                "pad {} seeds array {}; remove the array first",
                a.seed, a.id.0
            ),
        ));
    }
    for a in &source.arrays {
        if let ArrayLayout::Linear { direction, .. } | ArrayLayout::DualRow { direction, .. } =
            &a.layout
            && curves.contains(&direction.0)
        {
            return Err(conflict(
                "footprint.array_direction",
                format!("curve {} is array {}'s direction line", direction.0, a.id.0),
            ));
        }
    }
    source.sketch = outcome.sketch;
    report.created.extend(outcome.created);
    // Prune references to deleted entities.
    for role in &mut source.pads {
        if let PadGeometry::Template { owned_curves, .. } = &mut role.geometry {
            owned_curves.retain(|c| !curves.contains(&c.0));
        }
        if role
            .orientation
            .as_ref()
            .is_some_and(|o| curves.contains(&o.0))
        {
            role.orientation = None;
        }
        if let Some(h) = &mut role.hole {
            h.curves.retain(|c| !curves.contains(&c.0));
        }
        if role.hole.as_ref().is_some_and(|h| h.curves.is_empty()) {
            role.hole = None;
        }
        if let PasteRuleExpr::Windows(w) = &mut role.paste {
            w.retain(|c| !curves.contains(&c.0));
        }
    }
    let mut dropped_graphics = Vec::new();
    source.graphics.retain_mut(|g| {
        if let GraphicSource::Curves(ids) = &mut g.source {
            ids.retain(|c| !curves.contains(&c.0));
            if ids.is_empty() {
                dropped_graphics.push(g.id);
                return false;
            }
        }
        true
    });
    if let Some(d) = &mut source.datum {
        if points.contains(&d.point.0) {
            source.datum = None;
        } else if d
            .orientation
            .as_ref()
            .is_some_and(|o| curves.contains(&o.0))
        {
            d.orientation = None;
        }
    }
    next.graphics.retain(|g| !dropped_graphics.contains(&g.id));
    for pad in retired {
        remove_pad(next, pad, evaluator, false)?;
    }
    Ok(())
}

/// Remove a role pad and whatever of its own geometry is left (its owned
/// curves, hole curves and, unless already deleted, its anchor).
fn remove_pad(
    next: &mut FootprintDefinition,
    pad: PadId,
    evaluator: &dyn FootprintEvaluator,
    anchor_exists: bool,
) -> Result<()> {
    let source = source_mut(next)?;
    let index = source
        .pads
        .iter()
        .position(|r| r.pad == pad)
        .ok_or_else(|| Error::NotFound(format!("pad {pad} has no role in the source")))?;
    if let Some(a) = source.arrays.iter().find(|a| a.seed == pad) {
        return Err(conflict(
            "footprint.array_seed",
            format!("pad {pad} seeds array {}; remove the array first", a.id.0),
        ));
    }
    let role = source.pads.remove(index);
    let mut ids: Vec<SketchRef> = Vec::new();
    if let PadGeometry::Template { owned_curves, .. } = &role.geometry {
        ids.extend(owned_curves.iter().cloned().map(SketchRef::Curve));
    }
    if let Some(h) = &role.hole {
        ids.extend(h.curves.iter().cloned().map(SketchRef::Curve));
    }
    if anchor_exists {
        ids.push(SketchRef::Point(role.anchor.clone()));
    }
    if !ids.is_empty() {
        source.sketch = evaluator
            .apply_sketch(&source.sketch, &SketchEdit::Delete { ids })?
            .sketch;
    }
    if next.polarity.as_ref().is_some_and(|p| p.pad == pad) {
        next.polarity = None;
    }
    Ok(())
}

fn check_param_name(name: &super::footprint_source::ParamName) -> Result<()> {
    if !name.valid() {
        return Err(Error::Invalid(format!(
            "footprint.param_name: `{}` is not a parameter name",
            name.0
        )));
    }
    if name.is_package() {
        return Err(Error::Invalid(format!(
            "footprint.param_name: `{}` is in the reserved package namespace",
            name.0
        )));
    }
    Ok(())
}

fn apply_one(
    next: &mut FootprintDefinition,
    edit: &FootprintEdit,
    package: &PackageDefinition,
    evaluator: &dyn FootprintEvaluator,
    report: &mut FootprintEditReport,
) -> Result<()> {
    match edit {
        FootprintEdit::SetDefaults(defaults) => return set_defaults(next, (*defaults).into()),
        FootprintEdit::Detach => {
            if next.source.take().is_none() {
                return Err(Error::Invalid(
                    "the footprint has no source to detach".into(),
                ));
            }
            return Ok(());
        }
        _ => ensure_source(next, evaluator)?,
    }
    match edit {
        FootprintEdit::Sketch(e) => sketch_edit(next, e, evaluator, report)?,
        FootprintEdit::SetParameter { name, expr, unit } => {
            check_param_name(name)?;
            if expr.0.len() > MAX_EXPR {
                return Err(Error::Limit(
                    "a parameter expression is at most 256 bytes".into(),
                ));
            }
            let source = source_mut(next)?;
            match source.parameters.iter_mut().find(|p| p.name == *name) {
                Some(p) => {
                    p.expr = expr.clone();
                    p.unit = *unit;
                }
                None => {
                    if source.parameters.len() >= MAX_PARAMETERS {
                        return Err(Error::Limit(
                            "a footprint has at most 256 parameters".into(),
                        ));
                    }
                    source.parameters.push(FootprintParam {
                        name: name.clone(),
                        expr: expr.clone(),
                        unit: *unit,
                        comment: String::new(),
                    });
                }
            }
        }
        FootprintEdit::RenameParameter { from, to } => {
            check_param_name(to)?;
            let source = source_mut(next)?;
            *source = evaluator.rename_parameter(source, from, to)?;
        }
        FootprintEdit::RemoveParameter { name } => {
            let source = source_mut(next)?;
            if !source.parameters.iter().any(|p| p.name == *name) {
                return Err(Error::NotFound(format!("no parameter `{}`", name.0)));
            }
            if evaluator.references(source, name)? {
                return Err(conflict(
                    "footprint.param_in_use",
                    format!("parameter `{}` is still referenced", name.0),
                ));
            }
            source.parameters.retain(|p| p.name != *name);
        }
        FootprintEdit::AddPad {
            template,
            at,
            params,
        } => {
            if params.number.len() > MAX_LABEL {
                return Err(Error::Limit("a pad number is at most 256 bytes".into()));
            }
            let id = PadId(allocate(next)?);
            let values = package_values(package);
            let source = source_mut(next)?;
            let drawn = evaluator.add_template_pad(source, &values, *template, *at, params)?;
            source.sketch = drawn.outcome.sketch;
            source.pads.push(PadRole {
                pad: id,
                anchor: drawn.anchor,
                geometry: PadGeometry::Template {
                    template: *template,
                    owned_curves: drawn.owned_curves,
                },
                orientation: None,
                kind: params.kind,
                side: params.side,
                terminal: params.terminal,
                number: params.number.clone(),
                hole: None,
                mask: Default::default(),
                paste: Default::default(),
                vias: None,
            });
            report.created.extend(drawn.outcome.created);
            report.pads.push(id);
        }
        FootprintEdit::SetPadRole { role } => {
            let source = source_mut(next)?;
            let slot = source
                .pads
                .iter_mut()
                .find(|r| r.pad == role.pad)
                .ok_or_else(|| Error::NotFound(format!("pad {} has no role", role.pad)))?;
            *slot = role.clone();
        }
        FootprintEdit::RemovePad { pad } => remove_pad(next, *pad, evaluator, true)?,
        FootprintEdit::ConvertToCustom { pad } => {
            let source = source_mut(next)?;
            let role = source
                .pads
                .iter_mut()
                .find(|r| r.pad == *pad)
                .ok_or_else(|| Error::NotFound(format!("pad {pad} has no role")))?;
            role.geometry = PadGeometry::Profile;
        }
        FootprintEdit::AddArray {
            seed,
            layout,
            numbering,
            skip,
            first_number,
        } => {
            let id = ArrayId(allocate(next)?);
            let source = source_mut(next)?;
            if !source.pads.iter().any(|r| r.pad == *seed) {
                return Err(Error::NotFound(format!(
                    "pad {seed} has no role to seed an array"
                )));
            }
            if source.arrays.iter().any(|a| a.seed == *seed) {
                return Err(conflict(
                    "footprint.array_seed",
                    format!("pad {seed} already seeds an array"),
                ));
            }
            if source.arrays.len() >= super::footprint_source::MAX_ARRAYS {
                return Err(Error::Limit("a footprint has at most 32 arrays".into()));
            }
            source.arrays.push(PadArray {
                id,
                seed: *seed,
                layout: layout.clone(),
                numbering: numbering.clone(),
                skip: skip.clone(),
                first_number: first_number.clone(),
            });
            report.arrays.push(id);
        }
        FootprintEdit::EditArray { array } => {
            let source = source_mut(next)?;
            if !source.pads.iter().any(|r| r.pad == array.seed) {
                return Err(Error::NotFound(format!("pad {} has no role", array.seed)));
            }
            if source
                .arrays
                .iter()
                .any(|a| a.seed == array.seed && a.id != array.id)
            {
                return Err(conflict(
                    "footprint.array_seed",
                    format!("pad {} already seeds an array", array.seed),
                ));
            }
            let slot = source
                .arrays
                .iter_mut()
                .find(|a| a.id == array.id)
                .ok_or_else(|| Error::NotFound("That pad array no longer exists".into()))?;
            *slot = array.clone();
        }
        FootprintEdit::RemoveArray { id } => {
            let source = source_mut(next)?;
            let before = source.arrays.len();
            source.arrays.retain(|a| a.id != *id);
            if source.arrays.len() == before {
                return Err(Error::NotFound(format!("no array {}", id.0)));
            }
            let in_array = |k: &super::footprint_source::GenerationKey| {
                let super::footprint_source::GenerationKey::Array { array, .. } = k;
                array == id
            };
            source.overrides.retain(|o| !in_array(&o.key));
            source.keys.retain(|k| !in_array(&k.key));
        }
        FootprintEdit::SetOverride { key, value } => {
            let source = source_mut(next)?;
            match source.overrides.iter_mut().find(|o| o.key == *key) {
                Some(o) => o.value = value.clone(),
                None => source.overrides.push(KeyedOverride {
                    key: *key,
                    value: value.clone(),
                }),
            }
        }
        FootprintEdit::ClearOverride { key } => {
            let source = source_mut(next)?;
            let before = source.overrides.len();
            source.overrides.retain(|o| o.key != *key);
            if source.overrides.len() == before {
                return Err(Error::NotFound(
                    "That array pad has no override to clear.".into(),
                ));
            }
        }
        FootprintEdit::SetGraphicRole { role } => {
            let known = next
                .source
                .as_ref()
                .is_some_and(|s| s.graphics.iter().any(|g| g.id == role.id));
            if known {
                let source = source_mut(next)?;
                if let Some(slot) = source.graphics.iter_mut().find(|g| g.id == role.id) {
                    *slot = role.clone();
                }
            } else {
                let id = GraphicId(allocate(next)?);
                let source = source_mut(next)?;
                if source.graphics.len() >= super::graphics::MAX_GRAPHICS {
                    return Err(Error::Limit("too many graphic roles".into()));
                }
                let mut role = role.clone();
                role.id = id;
                source.graphics.push(role);
                report.graphics.push(id);
            }
        }
        FootprintEdit::RemoveGraphicRole { id } => {
            let source = source_mut(next)?;
            let before = source.graphics.len();
            source.graphics.retain(|g| g.id != *id);
            if source.graphics.len() == before {
                return Err(Error::NotFound("That graphic has no role to clear".into()));
            }
            next.graphics.retain(|g| g.id != *id);
            if let Some(p) = &mut next.polarity {
                p.markers.retain(|m| m != id);
            }
        }
        FootprintEdit::SetDatum { point, orientation } => {
            source_mut(next)?.datum = Some(DatumRole {
                point: point.clone(),
                orientation: orientation.clone(),
            });
        }
        FootprintEdit::SetDefaults(_) | FootprintEdit::Detach => {}
    }
    Ok(())
}

fn set_defaults(next: &mut FootprintDefinition, defaults: PadDefaults) -> Result<()> {
    next.defaults = defaults;
    Ok(())
}

/// Apply the edits on a clone and evaluate once: the edited footprint
/// (uncommitted) and its evaluation, when it still has a source.
fn stage(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    edits: &[FootprintEdit],
    evaluator: &dyn FootprintEvaluator,
) -> Result<(FootprintDefinition, Option<Evaluation>, FootprintEditReport)> {
    if edits.is_empty() || edits.len() > MAX_FOOTPRINT_EDITS {
        return Err(Error::Invalid(
            "edit a footprint with 1–256 edits at a time".into(),
        ));
    }
    let mut next = footprint.clone();
    let mut report = FootprintEditReport::default();
    for edit in edits {
        apply_one(&mut next, edit, package, evaluator, &mut report)?;
    }
    let evaluation = match &next.source {
        Some(source) => Some(evaluate(source, package, footprint, evaluator)?),
        None => None,
    };
    Ok((next, evaluation, report))
}

fn evaluate(
    source: &FootprintSource,
    package: &PackageDefinition,
    previous: &FootprintDefinition,
    evaluator: &dyn FootprintEvaluator,
) -> Result<Evaluation> {
    let values = package_values(package);
    evaluator.evaluate(EvaluationRequest {
        source,
        package,
        package_values: &values,
        previous,
    })
}

fn blocking(evaluation: &Evaluation) -> Option<Error> {
    evaluation
        .findings
        .iter()
        .find(|f| f.blocks.is_some())
        .map(|f| conflict(&f.code, &f.message))
}

/// Apply `edits` in order on a clone without evaluating or committing:
/// the early steps of a multi-step build (a family template whose later
/// edits name entities that earlier edits created). The caller finishes
/// with [`edit`], which evaluates once and commits source and result.
pub(crate) fn apply_unevaluated(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    edits: &[FootprintEdit],
    evaluator: &dyn FootprintEvaluator,
) -> Result<(FootprintDefinition, FootprintEditReport)> {
    if edits.is_empty() || edits.len() > MAX_FOOTPRINT_EDITS {
        return Err(Error::Invalid(
            "edit a footprint with 1–256 edits at a time".into(),
        ));
    }
    let mut next = footprint.clone();
    let mut report = FootprintEditReport::default();
    for edit in edits {
        apply_one(&mut next, edit, package, evaluator, &mut report)?;
    }
    Ok((next, report))
}

/// `library.edit_footprint`: the edited footprint, or an error and nothing.
pub fn edit(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    edits: &[FootprintEdit],
    evaluator: &dyn FootprintEvaluator,
) -> Result<(FootprintDefinition, FootprintEditReport)> {
    let (mut next, evaluation, mut report) = stage(footprint, package, edits, evaluator)?;
    if let Some(evaluation) = evaluation {
        if let Some(e) = blocking(&evaluation) {
            return Err(e);
        }
        report.findings = evaluation.findings.clone();
        report.pads.extend(commit(&mut next, evaluation)?);
    }
    Ok((next, report))
}

/// `library.footprint_preview`: what [`edit`] would do, including blocking
/// findings, without changing anything.
pub fn preview(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    edits: &[FootprintEdit],
    evaluator: &dyn FootprintEvaluator,
) -> Result<FootprintPreview> {
    let (mut next, evaluation, mut report) = stage(footprint, package, edits, evaluator)?;
    let mut committable = true;
    if let Some(evaluation) = evaluation {
        report.findings = evaluation.findings.clone();
        if blocking(&evaluation).is_some() {
            committable = false;
        } else {
            report.pads.extend(commit(&mut next, evaluation)?);
        }
    }
    Ok(FootprintPreview {
        definition: next,
        report,
        committable,
    })
}

/// Publish's determinism check: a sourced footprint must re-evaluate to
/// its stored result (`footprint.stale` otherwise).
pub fn check_fresh(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    evaluator: &dyn FootprintEvaluator,
) -> Result<()> {
    let Some(source) = &footprint.source else {
        return Ok(());
    };
    let evaluation = evaluate(source, package, footprint, evaluator)?;
    if let Some(e) = blocking(&evaluation) {
        return Err(e);
    }
    let hash = evaluation.summary.result_hash.clone();
    // The hash alone does not prove the stored data: compare what the
    // source derives (pads without their authored terminals, derived
    // graphics, datum) with what is stored.
    let mut fresh = footprint.clone();
    commit(&mut fresh, evaluation)?;
    let pads = |d: &FootprintDefinition| {
        let mut pads = d.pads.clone();
        for p in &mut pads {
            p.terminal = None;
        }
        pads.sort_by_key(|p| p.id);
        pads
    };
    let graphics = |d: &FootprintDefinition| {
        let mut g = d.graphics.clone();
        g.sort_by_key(|g| g.id);
        g
    };
    if hash != source.solve.result_hash
        || pads(&fresh) != pads(footprint)
        || graphics(&fresh) != graphics(footprint)
        || fresh.datum != footprint.datum
    {
        return Err(conflict(
            "footprint.stale",
            "the stored pads differ from re-evaluating the source; edit the footprint to re-solve it",
        ));
    }
    Ok(())
}

/// Which terminals `AssignTerminals` lands.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AssignRule {
    /// Each pad without a terminal takes the package terminal whose
    /// designation equals its number, when exactly one does and no other
    /// pad lands it. Explicit terminals are never changed.
    ByPackageDesignation,
    /// These pads take exactly these terminals (`None` clears one).
    Explicit(Vec<PadTerminal>),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PadTerminal {
    pub pad: PadId,
    pub terminal: Option<PackageTerminalId>,
}

/// One pad whose terminal the assignment changes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalChange {
    pub pad: PadId,
    pub number: String,
    pub from: Option<PackageTerminalId>,
    pub to: Option<PackageTerminalId>,
}

/// The changes `rule` makes, and the footprint with them applied.
pub fn assign_terminals(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    rule: &AssignRule,
) -> Result<(FootprintDefinition, Vec<TerminalChange>)> {
    let mut wanted: Vec<(PadId, Option<PackageTerminalId>)> = Vec::new();
    match rule {
        AssignRule::ByPackageDesignation => {
            let mut landed: BTreeSet<PackageTerminalId> =
                footprint.pads.iter().filter_map(|p| p.terminal).collect();
            for pad in &footprint.pads {
                if pad.terminal.is_some()
                    || matches!(pad.kind, PadKind::NonPlatedHole | PadKind::Fiducial)
                {
                    continue;
                }
                let mut matching = package
                    .terminals
                    .iter()
                    .filter(|t| !pad.number.is_empty() && t.designation == pad.number);
                if let (Some(t), None) = (matching.next(), matching.next())
                    && landed.insert(t.id)
                {
                    wanted.push((pad.id, Some(t.id)));
                }
            }
        }
        AssignRule::Explicit(list) => {
            if list.len() > super::MAX_PADS {
                return Err(Error::Limit("at most 1024 assignments".into()));
            }
            for a in list {
                if footprint.pad(a.pad).is_none() {
                    return Err(Error::NotFound(format!("no pad {}", a.pad)));
                }
                if let Some(t) = a.terminal
                    && package.terminal(t).is_none()
                {
                    return Err(Error::NotFound(format!(
                        "package terminal {t} does not exist"
                    )));
                }
                wanted.push((a.pad, a.terminal));
            }
        }
    }
    let mut next = footprint.clone();
    let mut changes = Vec::new();
    for (id, to) in wanted {
        let Some(pad) = next.pads.iter_mut().find(|p| p.id == id) else {
            continue;
        };
        if pad.terminal == to {
            continue;
        }
        changes.push(TerminalChange {
            pad: id,
            number: pad.number.clone(),
            from: pad.terminal,
            to,
        });
        pad.terminal = to;
        if let Some(role) = next
            .source
            .as_deref_mut()
            .and_then(|s| s.pads.iter_mut().find(|r| r.pad == id))
        {
            role.terminal = to;
        }
    }
    Ok((next, changes))
}
