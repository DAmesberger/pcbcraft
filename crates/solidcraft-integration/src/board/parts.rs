//! The parts solve on Solidcraft's sketch solver
//! (docs/board-sketch-references.md §5.2): one scratch sketch per connected
//! component of dimensioned parts, with the named board geometry held
//! fixed, per part an origin and a construction axis, its features bound to
//! that frame with `PatternPoint` (y mirrored on the bottom side), and the
//! dimensions, whose expressions evaluate over the board parameters.
//!
//! Slice S2 implements [`solve`] and deletes [`edge_constraint_values`]
//! with `EdgeConstraint`.
use crate::footprint::params::Values;
use pcbcraft_core::diagnostics::{Finding, ObjectRef, Result};
use pcbcraft_core::layout::dimensions::{PartsRequest, PartsSolution, not_yet};
use pcbcraft_core::layout::outline::{CONSTRAINT_UNRESOLVED, ConstraintValues};
use pcbcraft_core::model::EdgeConstraint;

/// Solve one component of dimensioned parts.
pub(super) fn solve(_request: &PartsRequest<'_>) -> Result<PartsSolution> {
    Err(not_yet())
}

/// Every edge constraint's `along` and `offset` in mm, evaluated against
/// the board parameters; a finding for each that does not evaluate.
pub(super) fn edge_constraint_values(
    values: &Values,
    constraints: &[EdgeConstraint],
) -> (Vec<ConstraintValues>, Vec<Finding>) {
    let mut findings = Vec::new();
    let mut solved = Vec::new();
    for c in constraints {
        let along = values.signed_length(&c.along.0);
        let offset = values.signed_length(&c.offset.0);
        match (along, offset) {
            (Ok(along), Ok(offset)) => solved.push(ConstraintValues {
                constraint: c.id,
                along,
                offset,
            }),
            (Err(p), _) | (_, Err(p)) => findings.push(Finding::new(
                CONSTRAINT_UNRESOLVED,
                format!("edge constraint {}: {}", c.id, p.message),
                vec![ObjectRef::Board],
            )),
        }
    }
    (solved, findings)
}

#[cfg(test)]
mod tests;
