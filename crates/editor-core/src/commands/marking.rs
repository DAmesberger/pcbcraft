//! Slice D4: handler of `instance.set_marking` and the `instance.marking`
//! query (the resolved layout D5 draws). Component and Part marking specs
//! are edited through `library.edit` like any other body data.
use super::Outcome;
use crate::diagnostics::{Error, ObjectRef, Result};
use crate::fields::{FieldValue, ToleranceBasis, ValueType};
use crate::ids::Id;
use crate::instances;
use crate::library::marking::{self, AreaChoice, InstanceMarking, MarkingInputs, MarkingLayout};
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
/// `None` when nothing is marked (hidden, or no spec anywhere).
pub(crate) fn layout(project: &Project, instance: Id) -> Result<Option<MarkingLayout>> {
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
    let area = match spec.area {
        AreaChoice::Explicit { size_mm, .. } => Some(size_mm),
        AreaChoice::FromModel => match target.physical {
            Some(choice) => {
                let view = instances::resolve_physical(library, &component.definition, choice)?;
                marking::package_area(library, &view.package.definition)
            }
            None => None,
        },
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
        area_mm: area.unwrap_or([0.0, 0.0]),
    };
    let mut laid = marking::layout(spec, &inputs)?;
    for finding in &mut laid.findings {
        finding.objects.push(ObjectRef::Instance { id: instance });
    }
    Ok(Some(laid))
}
