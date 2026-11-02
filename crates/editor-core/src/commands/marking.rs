//! Slice D4: handler of `instance.set_marking` and the `instance.marking`
//! query (the resolved layout D5 draws). Component and Part marking specs
//! are edited through `library.edit` like any other body data.
use super::Outcome;
use crate::diagnostics::{Error, ObjectRef, Result};
use crate::fields::{FieldValue, ToleranceBasis, ValueType};
use crate::ids::Id;
use crate::instances;
use crate::library::marking::{self, InstanceMarking, MarkingInputs, MarkingLayout};

use crate::model::Project;
use std::collections::BTreeMap;

fn invalid(message: String) -> Error {
    Error::Invalid(message)
}

/// `instance.set_marking`: replace the instance's marking choices. The
/// variables and a replacing spec are checked first; a refusal changes
/// nothing. Display-only: no geometry, net or fabrication data changes.
pub(crate) fn set_marking(
    project: &mut Project,
    instance: Id,
    marking: InstanceMarking,
) -> Result<Outcome> {
    let designator = project
        .instance(instance)
        .map(|i| i.designator.clone())
        .ok_or_else(|| invalid(format!("Instance {instance} does not exist.")))?;
    if marking.vars.len() > marking::MAX_MARKING_VARS {
        return Err(invalid(format!(
            "marking.template: {designator} sets more than {} marking variables",
            marking::MAX_MARKING_VARS
        )));
    }
    if !marking.within_bounds() {
        return Err(invalid(format!(
            "marking.template: {designator}: marking variable names are a-z, 0-9 and _ \
             and names and values are at most {} bytes without control characters, and a \
             replacing marking has at most {} items",
            marking::MAX_MARKING_VAR,
            marking::MAX_MARKING_ITEMS
        )));
    }
    if let Some(spec) = &marking.replace
        && let Some(finding) = marking::spec_findings(crate::ids::DefinitionId(instance), spec)
            .into_iter()
            .next()
    {
        return Err(invalid(format!(
            "marking.template: {designator}: {}",
            finding.message
        )));
    }
    if let Some(spec) = &marking.replace
        && let Some(finding) =
            marking::artwork_findings(&project.library, crate::ids::DefinitionId(instance), spec)
                .into_iter()
                .next()
    {
        return Err(invalid(format!(
            "marking.artwork: {designator}: {}",
            finding.message
        )));
    }
    let target = instances::instance_mut(project, instance)?;
    target.marking = marking;
    Ok(Outcome::None)
}

/// The SI value a code or band reads from a field value: a quantity's SI
/// value, an integer, or a symmetric relative tolerance as a fraction.
fn numeric(value: &FieldValue, value_type: Option<&ValueType>) -> Option<f64> {
    match value {
        FieldValue::Quantity { si, .. } => Some(*si),
        FieldValue::Integer(v) => Some(*v as f64),
        FieldValue::Tolerance { minus, plus } if minus == plus => match value_type {
            Some(ValueType::Tolerance {
                basis: ToleranceBasis::Absolute { .. },
            }) => None,
            _ => Some(*plus),
        },
        _ => None,
    }
}

/// `instance.marking`: the effective spec laid out for one instance;
/// `None` when nothing is marked (hidden, or no spec anywhere). Reads only
/// `project`; it is the query's whole implementation.
pub fn layout(project: &Project, instance: Id) -> Result<Option<MarkingLayout>> {
    let target = project
        .instance(instance)
        .ok_or_else(|| invalid(format!("Instance {instance} does not exist.")))?;
    let library = &project.library;
    let component = library.component(target.component).ok_or_else(|| {
        invalid(format!(
            "{} pins component {}, which is not in the project library.",
            target.designator, target.component
        ))
    })?;
    let part = target.part.and_then(|p| library.part(p));
    let Some(spec) = marking::effective(
        &target.marking,
        part.and_then(|p| p.definition.top_mark.as_deref()),
        component.definition.marking.as_deref(),
    ) else {
        return Ok(None);
    };
    let package = match target.physical {
        Some(choice) => Some(
            instances::resolve_physical(library, &component.definition, choice)?
                .package
                .definition
                .clone(),
        ),
        None => None,
    };
    let mut fields = BTreeMap::new();
    for field in instances::effective_fields(library, target)? {
        let (Some(value), Some(text)) = (&field.value, &field.text) else {
            continue;
        };
        let value_type = library.field(field.field).map(|f| &f.definition.value_type);
        fields.insert(field.field.id, (text.clone(), numeric(value, value_type)));
    }
    let inputs = MarkingInputs {
        designator: target.designator.clone(),
        mpn: part
            .map(|p| p.definition.mpn.clone())
            .filter(|s| !s.is_empty()),
        manufacturer: part
            .map(|p| p.definition.manufacturer.clone())
            .filter(|s| !s.is_empty()),
        fields,
        vars: target.marking.vars.clone(),
        area_mm: [0.0, 0.0],
        chart: marking::Chart::Flat,
    };
    let region = |s: &marking::MarkingSpec| {
        marking::surface_region(library, package.as_ref(), s.area, &s.placement)
    };
    let mut laid = marking::layout_with(spec, &inputs, &region)?;
    for finding in &mut laid.findings {
        finding.objects.push(ObjectRef::Instance { id: instance });
    }
    Ok(Some(laid))
}

/// `library.pack_artwork`: the edits through `library.edit`'s path, then
/// each staged SVG packed by hash (one already packed as SVG is reused;
/// one packed as another media type is refused). Assets the edited drafts
/// stopped naming are dropped in the same step.
pub(crate) fn pack_artwork(
    document: &mut crate::model::Document,
    edits: Vec<crate::library::DraftEdit>,
    artwork: Vec<crate::library::PackedAsset>,
) -> Result<Outcome> {
    use crate::library::{Definition, bundle, drafts};
    use std::collections::BTreeSet;
    use std::sync::Arc;
    if artwork.is_empty() || artwork.len() > marking::MAX_MARKING_ITEMS {
        return Err(invalid(format!(
            "marking.artwork: pack 1–{} artwork files at a time",
            marking::MAX_MARKING_ITEMS
        )));
    }
    bundle::validate_assets(&artwork)?;
    let named: BTreeSet<crate::ids::ContentHash> = edits
        .iter()
        .flat_map(|e| match &e.definition {
            Definition::Component(c) => c.marking.iter().flat_map(|m| m.artwork()).collect(),
            Definition::Part(p) => p.top_mark.iter().flat_map(|m| m.artwork()).collect(),
            _ => Vec::new(),
        })
        .collect();
    for asset in &artwork {
        if asset.media_type != marking::ARTWORK_MEDIA_TYPE
            || asset.data.len() > marking::MAX_ARTWORK_BYTES
            || std::str::from_utf8(asset.data.as_slice()).is_err()
        {
            return Err(invalid(format!(
                "marking.artwork: {} is not SVG artwork ({} text of at most {} KiB)",
                asset.hash,
                marking::ARTWORK_MEDIA_TYPE,
                marking::MAX_ARTWORK_BYTES >> 10
            )));
        }
        if !named.contains(&asset.hash) {
            return Err(invalid(format!(
                "marking.artwork: artwork {} is staged but no edited marking names it",
                asset.hash
            )));
        }
        if let Some(held) = document
            .project
            .library
            .assets
            .iter()
            .find(|a| a.hash == asset.hash)
            && held.media_type != asset.media_type
        {
            return Err(invalid(format!(
                "marking.artwork: {} is already packed as {}",
                asset.hash, held.media_type
            )));
        }
    }
    let before = super::assets::named_by_drafts(&document.drafts, edits.iter().map(|e| e.id));
    drafts::edit(Arc::make_mut(&mut document.drafts), edits)?;
    let project = Arc::make_mut(&mut document.project);
    for asset in artwork {
        if project.library.assets.iter().all(|a| a.hash != asset.hash) {
            project.library.assets.push(asset);
        }
    }
    super::assets::drop_unnamed(document, before);
    Ok(Outcome::None)
}
