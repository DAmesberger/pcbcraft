//! Per-instance text placement (`instance.set_text_placement`): where one
//! placed part prints a text of its footprint, such as a designator moved
//! back onto the board. The footprint stays as published; the offset and
//! turn live on the instance and are applied by `layout::placed_graphics`,
//! which 2D, the legend Gerber, the drawings and the 3D legend all read.
use crate::diagnostics::{Error, Result};
use crate::geometry::point_valid;
use crate::ids::{GraphicId, Id};
use crate::library::graphics::{DrawingLayer, Item, TextContent};
use crate::model::{Project, TextPlacement};

/// Farthest a text may move from where its footprint prints it, mm: the
/// largest board side `layout::validate` accepts.
pub const MAX_TEXT_OFFSET_MM: f64 = 1000.0;
/// Most moved texts on one instance.
pub const MAX_TEXT_PLACEMENTS: usize = 64;

/// Whether an offset is finite and within [`MAX_TEXT_OFFSET_MM`].
pub fn offset_valid(placement: &TextPlacement) -> bool {
    point_valid(placement.offset)
        && placement
            .offset
            .iter()
            .all(|v| v.abs() <= MAX_TEXT_OFFSET_MM)
}

/// The text `graphic` names on the instance's footprint, or without one its
/// silkscreen designator.
pub fn target(project: &Project, id: Id, graphic: Option<GraphicId>) -> Result<GraphicId> {
    let instance = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("Unknown component instance".into()))?;
    let name = &instance.designator;
    let footprint = super::resolve(&project.library, instance)?
        .physical
        .map(|physical| &physical.footprint.definition)
        .ok_or_else(|| Error::Invalid(format!("{name} has no footprint, so no text to move")))?;
    let found = match graphic {
        Some(graphic) => footprint.graphics.iter().find(|g| g.id == graphic),
        None => footprint.graphics.iter().find(|g| {
            g.layer == DrawingLayer::Silkscreen
                && matches!(&g.item, Item::Text(t) if t.content == TextContent::Designator)
        }),
    };
    match found {
        Some(g) if matches!(g.item, Item::Text(_)) => Ok(g.id),
        Some(_) => Err(Error::Invalid(format!(
            "That graphic of {name} is not a text"
        ))),
        None if graphic.is_some() => Err(Error::NotFound(format!(
            "The footprint of {name} has no such graphic"
        ))),
        None => Err(Error::NotFound(format!(
            "The footprint of {name} has no silkscreen designator"
        ))),
    }
}

/// The stored form of a request: no placement at all for a text left where
/// its footprint prints it, so clearing returns the document to what it was.
fn stored(placement: Option<TextPlacement>) -> Option<TextPlacement> {
    placement.filter(|p| *p != TextPlacement::default())
}

/// Whether the request names the placement the text already has. A refused
/// request is refused here too, so it never reads as a restatement.
pub fn restated(
    project: &Project,
    id: Id,
    graphic: Option<GraphicId>,
    placement: Option<TextPlacement>,
) -> Result<bool> {
    let graphic = target(project, id, graphic)?;
    Ok(project
        .instance(id)
        .is_some_and(|i| i.text_placements.get(&graphic).copied() == stored(placement)))
}

/// `instance.set_text_placement`: move or turn one text of the instance, or
/// with no placement print it where its footprint does.
pub fn set(
    project: &mut Project,
    id: Id,
    graphic: Option<GraphicId>,
    placement: Option<TextPlacement>,
) -> Result<()> {
    if let Some(p) = &placement
        && !offset_valid(p)
    {
        return Err(Error::Invalid(format!(
            "A text moves at most {MAX_TEXT_OFFSET_MM} mm"
        )));
    }
    let graphic = target(project, id, graphic)?;
    let instance = super::instance_mut(project, id)?;
    match stored(placement) {
        Some(p) => {
            if instance.text_placements.len() >= MAX_TEXT_PLACEMENTS
                && !instance.text_placements.contains_key(&graphic)
            {
                return Err(Error::Limit(format!(
                    "One part moves at most {MAX_TEXT_PLACEMENTS} texts"
                )));
            }
            instance.text_placements.insert(graphic, p);
        }
        None => {
            instance.text_placements.remove(&graphic);
        }
    }
    Ok(())
}

/// After the instance's physical choice changed from `old_footprint`: keep
/// a placement only for a text that still exists in the same footprint
/// lineage. Graphic ids mean nothing across lineages.
pub(crate) fn follow_footprint(
    project: &mut Project,
    id: Id,
    old_footprint: Option<crate::ids::DefinitionId>,
) {
    let Some(instance) = project.instance(id) else {
        return;
    };
    if instance.text_placements.is_empty() {
        return;
    }
    let same = instance.physical.map(|p| p.footprint) == old_footprint;
    let texts: Vec<GraphicId> = super::resolve(&project.library, instance)
        .ok()
        .and_then(|r| r.physical)
        .filter(|_| same)
        .map(|physical| {
            physical
                .footprint
                .definition
                .graphics
                .iter()
                .filter(|g| matches!(g.item, Item::Text(_)))
                .map(|g| g.id)
                .collect()
        })
        .unwrap_or_default();
    if let Ok(instance) = super::instance_mut(project, id) {
        instance.text_placements.retain(|g, _| texts.contains(g));
    }
}

/// Document validation: every stored placement is in bounds.
pub fn check(project: &Project) -> Result<()> {
    for instance in &project.instances {
        if instance.text_placements.len() > MAX_TEXT_PLACEMENTS
            || !instance.text_placements.values().all(offset_valid)
        {
            return Err(Error::Invalid(format!(
                "The moved texts of {} are out of range",
                instance.designator
            )));
        }
    }
    Ok(())
}
