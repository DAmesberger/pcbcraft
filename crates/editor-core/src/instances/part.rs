//! Instance rules for the optional exact manufacturer part (S2).
//!
//! The only invariant is that a pinned `PartRef` resolves in the project
//! snapshot. Agreement with the instance's component revision, physical
//! choice and requirements is a finding ([`part_findings`]), because a
//! component update may leave a Part behind and the editor never re-points
//! it silently. `instance.assign_part` ([`assign_part`]) refuses a Part that
//! does not agree.
use super::{instance_mut, invalid};
use crate::diagnostics::{Error, Finding, Result};
use crate::fields::Requirement;
use crate::ids::{Id, PartRef};
use crate::library::Library;
use crate::model::{ComponentInstance, Project};

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
/// Contract stub: S2 task 1 builds these from the effective fields.
pub fn requirements(_library: &Library, _instance: &ComponentInstance) -> Result<Vec<Requirement>> {
    Err(Error::Unsupported(
        "instance requirements for Parts are not implemented yet (S2 task 1)".into(),
    ))
}

/// Findings of an instance's Part: wrong component revision
/// (`part.component_mismatch`), a binding other than the physical choice
/// (`part.binding_mismatch`), and every [`crate::fields::satisfies`] finding.
/// Empty without a Part.
///
/// Contract stub: S2 task 1 implements the comparison.
pub fn part_findings(_library: &Library, instance: &ComponentInstance) -> Vec<Finding> {
    let _ = instance.part;
    Vec::new()
}

/// `instance.assign_part`: set (or with `None` clear) the Part of 1–256
/// instances atomically. A Part must pin each instance's component revision
/// and the binding of its physical choice, and meet its requirements;
/// otherwise the whole command fails and nothing changes.
pub fn assign_part(project: &mut Project, instances: &[Id], part: Option<PartRef>) -> Result<()> {
    if instances.is_empty() || instances.len() > MAX_ASSIGN_INSTANCES {
        return Err(invalid(format!(
            "Assign a Part to between 1 and {MAX_ASSIGN_INSTANCES} instances at a time."
        )));
    }
    if part.is_some() {
        return Err(Error::Unsupported(
            "instance.assign_part with a Part is not implemented yet (S2 task 1)".into(),
        ));
    }
    for id in instances {
        instance_mut(project, *id)?.part = None;
    }
    Ok(())
}
