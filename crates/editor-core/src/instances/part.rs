//! Instance rules for the optional exact manufacturer part (S2).
//!
//! The only invariant is that a pinned `PartRef` resolves in the project
//! snapshot. Agreement with the instance's component revision, physical
//! choice and requirements is a finding ([`part_findings`]), because a
//! component update may leave a Part behind and the editor never re-points
//! it silently. `instance.assign_part` ([`assign_part`]) refuses a Part that
//! does not agree.
use super::{check_instance, effective_fields, instance_mut, invalid};
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::fields::{self, FieldValue, Requirement, ToleranceBasis, ValueType};
use crate::ids::{BindingRef, ComponentRef, Id, PartRef};
use crate::library::{Library, PartDefinition};
use crate::model::{ComponentInstance, Project};
use std::collections::BTreeSet;

/// How a binding reads in messages: its package or first footprint name and
/// revision, never its definition id alone.
pub fn binding_name(library: &Library, b: BindingRef) -> String {
    let Some(binding) = library.binding(b) else {
        return format!("binding {b}");
    };
    let package = library
        .package(binding.definition.package)
        .map(|p| p.meta.name.clone());
    let footprint = binding
        .definition
        .footprints
        .first()
        .and_then(|f| library.footprint(*f))
        .map(|f| f.meta.name.clone());
    let name = [Some(binding.meta.name.clone()), package, footprint]
        .into_iter()
        .flatten()
        .find(|n| !n.trim().is_empty());
    match name {
        Some(name) => format!("{name} {}", b.revision),
        None => format!("binding {b}"),
    }
}

/// Instances one `instance.assign_part` changes together (one Resolve-parts
/// group).
pub const MAX_ASSIGN_INSTANCES: usize = 256;

/// The invariant: a pinned Part revision is in the project library.
pub(super) fn check_part(library: &Library, instance: &ComponentInstance) -> Result<()> {
    match instance.part {
        Some(r) if library.part(r).is_none() => Err(invalid(format!(
            "{} pins Part {r}, which is not in the project library.",
            instance.designator
        ))),
        _ => Ok(()),
    }
}

/// The instance's requirements for comparison with rated specs: every
/// selected field with an effective value, in selection order.
///
/// A tolerance carries the nominal value it applies to when it can be
/// compared with a tolerance of another basis: the effective value of the
/// one selected quantity field measured in the absolute basis's quantity
/// kind lineage. That basis is the requirement's own or, for a relative
/// requirement, that of the pinned Part's rated tolerance. Without exactly
/// one such field the nominal is absent.
pub fn requirements(library: &Library, instance: &ComponentInstance) -> Result<Vec<Requirement>> {
    let part = instance
        .part
        .and_then(|r| library.part(r))
        .map(|p| &p.definition);
    requirements_for(library, instance, part)
}

fn requirements_for(
    library: &Library,
    instance: &ComponentInstance,
    part: Option<&PartDefinition>,
) -> Result<Vec<Requirement>> {
    let mut out = Vec::new();
    for field in effective_fields(library, instance)? {
        let Some(value) = field.value else {
            continue;
        };
        let view = library.field_view(field.field).ok_or_else(|| {
            invalid(format!(
                "Field {} or its quantity kind is not in the project library.",
                field.field
            ))
        })?;
        out.push(Requirement {
            field: field.field,
            label: field.label,
            value,
            value_type: view.field.definition.value_type.clone(),
            nominal: None,
        });
    }
    let nominals: Vec<Option<f64>> = out
        .iter()
        .map(|requirement| {
            let ValueType::Tolerance { basis } = &requirement.value_type else {
                return None;
            };
            let kind = match basis {
                ToleranceBasis::Absolute { kind } => Some(kind.id),
                ToleranceBasis::Relative => part
                    .and_then(|p| p.specs.iter().find(|s| s.field.id == requirement.field.id))
                    .and_then(|s| library.field_view(s.field))
                    .and_then(|v| match &v.field.definition.value_type {
                        ValueType::Tolerance {
                            basis: ToleranceBasis::Absolute { kind },
                        } => Some(kind.id),
                        _ => None,
                    }),
            }?;
            let mut measured = out.iter().filter_map(|r| match (&r.value_type, &r.value) {
                (ValueType::Quantity { kind: k, .. }, FieldValue::Quantity { si, .. })
                    if k.id == kind =>
                {
                    Some(*si)
                }
                _ => None,
            });
            match (measured.next(), measured.next()) {
                (Some(si), None) => Some(si),
                _ => None,
            }
        })
        .collect();
    for (requirement, nominal) in out.iter_mut().zip(nominals) {
        requirement.nominal = nominal;
    }
    Ok(out)
}

/// Findings of an instance's Part: wrong component lineage or revision
/// (`part.component_mismatch`), an older component revision than the
/// instance pins (`part.stale_component`), a newer one the instance must be
/// updated to first (`part.newer_component`), a binding other than the physical
/// choice's (`part.binding_mismatch`), a requirement the Part does not rate
/// (`part.spec_unrated`), and every [`crate::fields::satisfies`] finding of a
/// requirement it rates. Empty without a Part. Advisory:
/// they block no stage.
pub fn part_findings(library: &Library, instance: &ComponentInstance) -> Vec<Finding> {
    let Some(r) = instance.part else {
        return Vec::new();
    };
    let Some(part) = library.part(r) else {
        return vec![Finding::new(
            "reference.missing",
            format!(
                "{}: Part {r} is not in the project library.",
                instance.designator
            ),
            vec![ObjectRef::Instance { id: instance.id }],
        )];
    };
    findings_with(library, instance, r, &part.definition)
}

/// Every disagreement between `instance` and the Part revision `r`.
fn findings_with(
    library: &Library,
    instance: &ComponentInstance,
    r: PartRef,
    part: &PartDefinition,
) -> Vec<Finding> {
    let name = &instance.designator;
    let mpn = &part.mpn;
    let objects = vec![
        ObjectRef::Instance { id: instance.id },
        ObjectRef::Definition { id: r.id },
    ];
    let mut out = Vec::new();
    let pinned = instance.component;
    let component = |c: ComponentRef| {
        library.component(c).map_or_else(
            || c.to_string(),
            |p| format!("{} {}", p.meta.name, c.revision),
        )
    };
    let binding = |b: BindingRef| binding_name(library, b);
    if part.component.id == pinned.id && part.component.revision < pinned.revision {
        out.push(Finding::new(
            "part.stale_component",
            format!(
                "{name}: Part {mpn} is for {}, older than the instance's {}; keep it unresolved or choose a Part for {}.",
                component(part.component),
                component(pinned),
                component(pinned)
            ),
            objects.clone(),
        ));
    } else if part.component.id == pinned.id && part.component.revision > pinned.revision {
        out.push(Finding::new(
            "part.newer_component",
            format!(
                "{name}: Part {mpn} is for {}; update {name} from {} to it first.",
                component(part.component),
                component(pinned)
            ),
            objects.clone(),
        ));
    } else if part.component != pinned {
        out.push(Finding::new(
            "part.component_mismatch",
            format!(
                "{name}: Part {mpn} is for {}, not the instance's {}.",
                component(part.component),
                component(pinned)
            ),
            objects.clone(),
        ));
    }
    let listed = library.component(pinned).and_then(|c| {
        instance
            .physical
            .and_then(|p| c.definition.implementation(p.binding))
    });
    // A newer component revision lists its own binding revisions; the
    // binding is compared once the instance is on the Part's revision.
    let newer = part.component.id == pinned.id && part.component.revision > pinned.revision;
    if !newer && listed != Some(part.binding) {
        let chosen = match (instance.physical, listed) {
            (None, _) => "no physical implementation".to_owned(),
            (Some(_), Some(b)) => binding(b),
            (Some(p), None) => format!("an unlisted implementation ({})", p.binding),
        };
        out.push(Finding::new(
            "part.binding_mismatch",
            format!(
                "{name}: Part {mpn} is implemented by {}, but the instance uses {chosen}.",
                binding(part.binding)
            ),
            objects.clone(),
        ));
    }
    match requirements_for(library, instance, Some(part)) {
        Err(error) => out.push(Finding::new(
            "part.spec_uncomparable",
            format!("{name}: requirements cannot be read: {error}"),
            objects,
        )),
        Ok(requirements) => {
            for requirement in &requirements {
                let field_objects = vec![
                    ObjectRef::Property {
                        instance: instance.id,
                        field: requirement.field.id,
                    },
                    ObjectRef::Definition { id: r.id },
                ];
                let Some(spec) = part
                    .specs
                    .iter()
                    .find(|s| s.field.id == requirement.field.id)
                else {
                    // An unrated requirement is never an implicit pass:
                    // nothing says the Part meets it.
                    out.push(Finding::new(
                        "part.spec_unrated",
                        format!(
                            "{name}: {mpn} has no rated {}; rate it on the Part or clear the value on the instance.",
                            requirement.label
                        ),
                        field_objects,
                    ));
                    continue;
                };
                let Some(view) = library.field_view(spec.field) else {
                    out.push(Finding::new(
                        "part.spec_uncomparable",
                        format!(
                            "{name}: the rated {} of {mpn} names field {}, which is not in the project library.",
                            requirement.label, spec.field
                        ),
                        field_objects,
                    ));
                    continue;
                };
                let spec_type = &view.field.definition.value_type;
                let kind = |t: &ValueType| {
                    t.kind()
                        .and_then(|k| library.quantity_kind(k))
                        .map(|k| &k.definition)
                };
                for finding in requirement.satisfied_by(spec, spec_type) {
                    let required = fields::display(
                        &requirement.value,
                        &requirement.value_type,
                        kind(&requirement.value_type),
                    );
                    let rated = fields::display(&spec.value, spec_type, kind(spec_type));
                    let verb = match finding.code.as_str() {
                        "part.spec_mismatch" => "is not met by",
                        _ => "cannot be compared with",
                    };
                    out.push(Finding {
                        message: format!(
                            "{name}: {} {required} {verb} the rated {rated} ({}) of {mpn}.",
                            requirement.label,
                            spec.meaning.label()
                        ),
                        objects: field_objects.clone(),
                        ..finding
                    });
                }
            }
        }
    }
    out
}

/// `instance.assign_part`: set (or with `None` clear) the Part of 1–256
/// instances atomically. A Part must pin each instance's component revision
/// and the binding of its physical choice, and meet its requirements;
/// otherwise the whole command fails with a conflict naming the instance
/// and the field, and nothing changes.
pub fn assign_part(project: &mut Project, instances: &[Id], part: Option<PartRef>) -> Result<()> {
    if instances.is_empty() || instances.len() > MAX_ASSIGN_INSTANCES {
        return Err(invalid(format!(
            "Assign a Part to between 1 and {MAX_ASSIGN_INSTANCES} instances at a time."
        )));
    }
    let unique: BTreeSet<Id> = instances.iter().copied().collect();
    if unique.len() != instances.len() {
        return Err(invalid("An instance is listed twice.".into()));
    }
    if let Some(r) = part {
        let definition = project
            .library
            .part(r)
            .map(|p| &p.definition)
            .ok_or_else(|| Error::NotFound(format!("Part {r} is not in the project library.")))?;
        let mut conflicts = Vec::new();
        for id in instances {
            let instance = project
                .instance(*id)
                .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
            conflicts.extend(
                findings_with(&project.library, instance, r, definition)
                    .into_iter()
                    .map(|f| f.message),
            );
        }
        if !conflicts.is_empty() {
            let shown = conflicts.len().min(16);
            let mut message = format!("No Part was assigned. {}", conflicts[..shown].join(" "));
            if conflicts.len() > shown {
                message.push_str(&format!(" ({} more)", conflicts.len() - shown));
            }
            return Err(Error::Conflict(message));
        }
    }
    for id in instances {
        instance_mut(project, *id)?.part = part;
    }
    for id in instances {
        check_instance(project, *id)?;
    }
    Ok(())
}
