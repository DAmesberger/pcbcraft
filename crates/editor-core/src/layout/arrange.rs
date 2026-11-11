//! Several parts moved together: `layout.move_parts` (a group drag or flip),
//! `layout.align` and `layout.distribute`. Each is one undo step; the engine
//! carries the copper attached to every moved part at once
//! (`dimensions::carry`), so a track between two parts that move alike
//! moves with both.
//!
//! Align and distribute work on each part's outline: the package envelope
//! the canvas draws and picks ([`part_envelope`]). A part that would move
//! and is locked, or whose dimensions hold it on the axis it would move
//! along, refuses the whole command, naming the part.
use super::dimensions::{self, PartFreedom};
use super::outline::{self, BoardShapeEvaluator};
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::instances;
use crate::model::board::ConstraintTarget;
use crate::model::{Layer, Placement, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Most parts one command moves.
pub const MAX_ARRANGED: usize = 4_096;

/// Coordinates closer than this (mm) count as the same: a part already in
/// line does not move.
const SAME_MM: f64 = 1e-9;

/// One part's new pose in `layout.move_parts`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PartMove {
    pub instance: Id,
    pub position: Point,
    pub rotation_deg: f64,
    pub layer: Layer,
}

/// Which outline edge or centre line `layout.align` lines the parts up on.
/// The board's y axis points up: the top edge is the largest y.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AlignTo {
    /// Every left edge on the leftmost one.
    Left,
    /// Every right edge on the rightmost one.
    Right,
    /// Every top edge on the topmost one.
    Top,
    /// Every bottom edge on the lowest one.
    Bottom,
    /// Every centre on the vertical line through the middle of the
    /// parts' extent (centres in a column).
    CentreX,
    /// Every centre on the horizontal line through the middle of the
    /// parts' extent (centres in a row).
    CentreY,
}

/// The direction `layout.distribute` spaces the parts along.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// The board corners of a placed part's outline: its package envelope
/// through the placement transform. `None` for a part without a physical
/// implementation.
pub fn part_envelope(project: &Project, placement: &Placement) -> Option<[Point; 4]> {
    let instance = project.instance(placement.instance)?;
    let physical = instances::resolve(&project.library, instance)
        .ok()?
        .physical?;
    let footprint = &physical.footprint.definition;
    let envelope = footprint.envelope_outline(&physical.package.definition);
    let t = super::placement_transform(placement, &footprint.datum);
    Some(envelope.corners().map(|p| t.apply(p)))
}

/// A part's outline extent on the board: (min, max).
fn extent(project: &Project, placement: &Placement) -> Result<(Point, Point)> {
    let corners = part_envelope(project, placement).ok_or_else(|| {
        Error::Invalid(format!(
            "{} has no outline to line up",
            outline::part_name(project, placement.instance)
        ))
    })?;
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for p in corners {
        for k in 0..2 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    Ok((min, max))
}

/// The placements `instances` names, each once and on the board, with
/// their extents; at least `fewest`.
fn parts<'a>(
    project: &'a Project,
    instances: &[Id],
    fewest: usize,
    what: &str,
) -> Result<Vec<(&'a Placement, (Point, Point))>> {
    parts_named(project, instances, what)?;
    let mut out = Vec::with_capacity(instances.len());
    for &id in instances {
        if let Some(placement) = project.placement_of(id) {
            out.push((placement, extent(project, placement)?));
        }
    }
    if out.len() < fewest {
        return Err(Error::Invalid(format!(
            "{what} needs at least {fewest} parts"
        )));
    }
    Ok(out)
}

/// Why `instance` may not move along x (`x`) or y: it is locked, or its
/// dimensions hold that coordinate. Without an evaluator a dimensioned part
/// cannot be solved, so cannot move.
fn refusal(
    project: &Project,
    instance: Id,
    x: bool,
    verb: &str,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Option<Error> {
    let placement = project.placement_of(instance)?;
    if placement.locked {
        return Some(super::locked(project, instance, verb));
    }
    let target = ConstraintTarget::Placement(instance);
    let Some(evaluator) = evaluator else {
        return dimensions::held(project, instance, None);
    };
    match dimensions::freedom(project, target, evaluator)? {
        Ok((PartFreedom { x: fx, y: fy, .. }, measured)) if !(if x { fx } else { fy }) => {
            let at = dimensions::board_sketch(project, evaluator)
                .ok()
                .and_then(|board| dimensions::held_at(project, &board, target, &measured));
            Some(Error::Conflict(dimensions::held_message(
                &outline::part_name(project, instance),
                at.as_deref(),
                verb,
            )))
        }
        Ok(_) => None,
        Err(e) => Some(e),
    }
}

/// Shift each part by its offset along one axis, refusing first when any
/// part that moves may not. Parts that do not move are not checked.
fn shift(
    project: &mut Project,
    offsets: Vec<(Id, f64)>,
    x: bool,
    verb: &str,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    let moving: Vec<(Id, f64)> = offsets
        .into_iter()
        .filter(|(_, by)| by.abs() > SAME_MM)
        .collect();
    for (id, _) in &moving {
        if let Some(error) = refusal(project, *id, x, verb, evaluator) {
            return Err(error);
        }
    }
    let k = usize::from(!x);
    for (id, by) in moving {
        if let Some(p) = project.placements.iter_mut().find(|p| p.instance == id) {
            p.position[k] += by;
            if !point_valid(p.position) {
                return Err(Error::Invalid("the arranged position is not finite".into()));
            }
        }
    }
    Ok(())
}

/// `layout.align`: line the parts' outlines up on one edge or centre line.
pub fn align(
    project: &mut Project,
    instances: &[Id],
    to: AlignTo,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    let parts = parts(project, instances, 2, "Align")?;
    let x = matches!(to, AlignTo::Left | AlignTo::Right | AlignTo::CentreX);
    let k = usize::from(!x);
    let mins = parts.iter().map(|(_, e)| e.0[k]);
    let maxs = parts.iter().map(|(_, e)| e.1[k]);
    let min = mins.fold(f64::INFINITY, f64::min);
    let max = maxs.fold(f64::NEG_INFINITY, f64::max);
    let offsets = parts
        .iter()
        .map(|(p, (lo, hi))| {
            let by = match to {
                AlignTo::Left | AlignTo::Bottom => min - lo[k],
                AlignTo::Right | AlignTo::Top => max - hi[k],
                AlignTo::CentreX | AlignTo::CentreY => (min + max) / 2.0 - (lo[k] + hi[k]) / 2.0,
            };
            (p.instance, by)
        })
        .collect();
    shift(project, offsets, x, "align", evaluator)
}

/// `layout.distribute`: the first and last parts along the axis (by their
/// centres) stay; the others move so the gaps between neighbouring
/// outlines are equal.
pub fn distribute(
    project: &mut Project,
    instances: &[Id],
    axis: Axis,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    let mut parts = parts(project, instances, 3, "Distribute")?;
    let x = axis == Axis::Horizontal;
    let k = usize::from(!x);
    let centre = |e: &(Point, Point)| (e.0[k] + e.1[k]) / 2.0;
    parts.sort_by(|a, b| {
        centre(&a.1)
            .total_cmp(&centre(&b.1))
            .then(a.0.instance.cmp(&b.0.instance))
    });
    let (Some(first), Some(last)) = (parts.first(), parts.last()) else {
        return Ok(());
    };
    let span = last.1.1[k] - first.1.0[k];
    let sizes: f64 = parts.iter().map(|(_, e)| e.1[k] - e.0[k]).sum();
    let count = parts.len() as f64;
    let gap = (span - sizes) / (count - 1.0);
    let mut at = first.1.0[k];
    let mut offsets = Vec::with_capacity(parts.len());
    for (p, (lo, hi)) in &parts {
        offsets.push((p.instance, at - lo[k]));
        at += hi[k] - lo[k] + gap;
    }
    shift(project, offsets, x, "distribute", evaluator)
}

/// `layout.move_parts`: each part to its new pose, refused as `layout.move`
/// refuses a locked part.
pub fn move_parts(project: &mut Project, moves: &[PartMove]) -> Result<()> {
    let instances: Vec<Id> = moves.iter().map(|m| m.instance).collect();
    if moves.is_empty() {
        return Err(Error::Invalid(
            "layout.move_parts needs at least one part".into(),
        ));
    }
    parts_named(project, &instances, "layout.move_parts")?;
    for m in moves {
        if !point_valid(m.position) || !m.rotation_deg.is_finite() {
            return Err(Error::Invalid(format!(
                "{}'s new pose is not finite",
                outline::part_name(project, m.instance)
            )));
        }
        if project.placement_of(m.instance).is_some_and(|p| p.locked) {
            return Err(super::locked(project, m.instance, "move"));
        }
    }
    for m in moves {
        if let Some(p) = project
            .placements
            .iter_mut()
            .find(|p| p.instance == m.instance)
        {
            p.position = m.position;
            p.rotation_deg = m.rotation_deg;
            p.layer = m.layer;
        }
    }
    Ok(())
}

/// Every named part once and placed (an outline is not needed to move it).
fn parts_named(project: &Project, instances: &[Id], what: &str) -> Result<()> {
    if instances.len() > MAX_ARRANGED {
        return Err(Error::Limit(format!(
            "{what} takes at most {MAX_ARRANGED} parts, not {}",
            instances.len()
        )));
    }
    let mut seen = BTreeSet::new();
    for &id in instances {
        if !seen.insert(id) {
            return Err(Error::Invalid(format!(
                "{} is named twice",
                outline::part_name(project, id)
            )));
        }
        if project.placement_of(id).is_none() {
            return Err(Error::NotFound(match project.instance(id) {
                Some(i) => format!("{} is not placed on the board", i.designator),
                None => format!("No component instance has ID {id}"),
            }));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
