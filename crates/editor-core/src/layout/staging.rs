//! Where Update PCB puts the parts it adds: the staging area beside the
//! board. New parts go right of the board outline, top-aligned with it, in
//! rows packed by each part's footprint box (courtyard, pads, drawings and
//! text), clear of each other, of the board and of everything already
//! there. Each schematic sheet's parts come in page order, and within a
//! sheet each kind (designator prefix: C, D, J, R, U…) starts its own row,
//! in designator order. Parts already placed never move.
use super::WORKING_AREA_MM;
use super::placed::{PlacedItem, placed_graphics, placed_pads};
use crate::diagnostics::{Error, Result};
use crate::geometry::{Bounds, Point};
use crate::ids::Id;
use crate::library::graphics::TextMetric;
use crate::model::{Layer, Placement, Project, board::outline_bounds};
use std::collections::BTreeMap;

/// The staging area starts this far right of the board's outline, mm: room
/// for the edge names and board dimensions drawn beside it.
pub const BOARD_MARGIN_MM: f64 = 6.0;
/// The least gap between a staged part's box and any other box, mm: room
/// for its designator between rows.
pub const GAP_MM: f64 = 2.0;
/// A row of staged parts is at least this long, mm (a small board's rows
/// would otherwise hold one part each).
const MIN_ROW_MM: f64 = 40.0;
/// Staged datums lie on this grid, mm.
const GRID_MM: f64 = 0.5;
/// The box of a part with no drawn content, about its datum, mm.
const EMPTY_HALF_MM: f64 = 1.0;
/// Most placement attempts in one staging; past it the staging is refused.
const MAX_STEPS: usize = 1_000_000;

/// Place `missing` (instances without a placement, in any order) on the
/// top side in the staging area, appending their placements. Fails, with
/// `project` unchanged, when they do not fit in the working area.
pub(crate) fn stage(project: &mut Project, missing: Vec<Id>) -> Result<()> {
    if missing.is_empty() {
        return Ok(());
    }
    let existing = project.placements.len();
    // Each new part at the origin, to measure its box about its datum.
    for &instance in &missing {
        project.placements.push(Placement {
            instance,
            position: [0.0, 0.0],
            rotation_deg: 0.0,
            layer: Layer::Top,
            locked: false,
            fill_override: Default::default(),
            pad_fill_overrides: Vec::new(),
        });
    }
    let boxes = part_boxes(project);
    let mut obstacles: Vec<Bounds> = project.placements[..existing]
        .iter()
        .filter_map(|p| boxes.get(&p.instance).copied())
        .collect();
    obstacles.extend(board_objects(project));
    let board = project.board().bounds();
    obstacles.push(board.inflated(BOARD_MARGIN_MM - GAP_MM));
    let order = groups(project, &missing);
    let left = board.max[0] + BOARD_MARGIN_MM;
    let right = left + (board.max[0] - board.min[0]).max(MIN_ROW_MM);
    let mut positions: BTreeMap<Id, Point> = BTreeMap::new();
    let mut top = board.max[1];
    let mut steps = 0usize;
    for group in order {
        let mut x = left;
        let mut row_bottom = top;
        let mut row_used = false;
        for instance in group {
            let local = boxes
                .get(&instance)
                .copied()
                .unwrap_or(Bounds::point([0.0, 0.0]).inflated(EMPTY_HALF_MM));
            let width = local.max[0] - local.min[0];
            loop {
                steps += 1;
                if steps > MAX_STEPS {
                    return no_room(project, existing);
                }
                if row_used && x + width > right {
                    top = row_bottom - GAP_MM;
                    x = left;
                    row_bottom = top;
                    row_used = false;
                }
                let at = [
                    snap(x - local.min[0], f64::ceil),
                    snap(top - local.max[1], f64::floor),
                ];
                let placed = translated(local, at);
                if let Some(hit) = obstacles
                    .iter()
                    .find(|o| overlaps(o, &placed.inflated(GAP_MM)))
                {
                    x = hit.max[0] + GAP_MM;
                    if !row_used && x + width > right {
                        // Nothing fits beside it in this row: go below it.
                        top = (hit.min[1] - GAP_MM).min(top - GRID_MM);
                        x = left;
                        row_bottom = top;
                    }
                    continue;
                }
                if !at
                    .iter()
                    .all(|v| (WORKING_AREA_MM[0]..=WORKING_AREA_MM[1]).contains(v))
                {
                    return no_room(project, existing);
                }
                positions.insert(instance, at);
                obstacles.push(placed);
                row_bottom = row_bottom.min(placed.min[1]);
                x = placed.max[0] + GAP_MM;
                row_used = true;
                break;
            }
        }
        if row_used {
            top = row_bottom - GAP_MM;
        }
    }
    for placement in &mut project.placements[existing..] {
        if let Some(at) = positions.get(&placement.instance) {
            placement.position = *at;
        }
    }
    Ok(())
}

fn no_room(project: &mut Project, existing: usize) -> Result<()> {
    project.placements.truncate(existing);
    Err(Error::Conflict(
        "No room beside the board to stage the new parts".into(),
    ))
}

/// Whether two boxes share interior (touching is not overlap).
fn overlaps(a: &Bounds, b: &Bounds) -> bool {
    const EPS: f64 = 1e-6;
    a.min[0] < b.max[0] - EPS
        && b.min[0] < a.max[0] - EPS
        && a.min[1] < b.max[1] - EPS
        && b.min[1] < a.max[1] - EPS
}

fn snap(v: f64, round: fn(f64) -> f64) -> f64 {
    round(v / GRID_MM) * GRID_MM
}

fn translated(b: Bounds, by: Point) -> Bounds {
    Bounds {
        min: [b.min[0] + by[0], b.min[1] + by[1]],
        max: [b.max[0] + by[0], b.max[1] + by[1]],
    }
}

/// Each placed part's box on the board: its pads, footprint drawings
/// (courtyard, silkscreen, assembly) and text.
pub fn part_boxes(project: &Project) -> BTreeMap<Id, Bounds> {
    let mut boxes: BTreeMap<Id, Bounds> = BTreeMap::new();
    let mut add = |instance: Id, b: Bounds| {
        if b.min.iter().chain(&b.max).all(|v| v.is_finite()) {
            boxes
                .entry(instance)
                .and_modify(|e| *e = e.union(b))
                .or_insert(b);
        }
    };
    for pad in placed_pads(project) {
        add(pad.instance, pad.outline().bounds(0.0));
    }
    let metric = TextMetric::default();
    for graphic in placed_graphics(project) {
        let b = match &graphic.item {
            PlacedItem::Shape { shape, width, .. } => shape.bounds(*width),
            PlacedItem::Text(text) => text.bounds(metric),
        };
        add(graphic.instance, b);
    }
    boxes
}

/// The boxes of the board's own objects that may lie beside it: tracks,
/// vias, fills and keepouts.
fn board_objects(project: &Project) -> impl Iterator<Item = Bounds> + '_ {
    let tracks = project.tracks.iter().map(|t| {
        Bounds::point(t.start)
            .including(t.end)
            .inflated(t.width / 2.0)
    });
    let vias = project
        .vias
        .iter()
        .map(|v| Bounds::point(v.position).inflated(v.diameter / 2.0));
    let fills = project.fills.iter().map(|f| outline_bounds(&f.outline));
    let keepouts = project.keepouts.iter().map(|k| outline_bounds(&k.outline));
    tracks.chain(vias).chain(fills).chain(keepouts)
}

/// The new parts in staging order: by the page of the sheet that first
/// shows them (parts on no sheet last), then by kind (designator prefix),
/// each kind of each sheet one group, in designator order.
fn groups(project: &Project, new: &[Id]) -> Vec<Vec<Id>> {
    let page = |instance: Id| {
        project
            .occurrences
            .iter()
            .filter(|o| o.instance == instance)
            .filter_map(|o| project.sheets.list.iter().position(|s| s.id == o.sheet))
            .min()
            .unwrap_or(usize::MAX)
    };
    /// A part's group (sheet page, kind), its place in it, and itself.
    type Keyed = ((usize, String), (u64, String), Id);
    let mut keyed: Vec<Keyed> = new
        .iter()
        .map(|&id| {
            let designator = project
                .instance(id)
                .map(|i| i.designator.clone())
                .unwrap_or_default();
            let (prefix, number, _) = crate::instances::designator_key(&designator);
            ((page(id), prefix), (number, designator), id)
        })
        .collect();
    keyed.sort();
    let mut out: Vec<Vec<Id>> = Vec::new();
    let mut last = None;
    for (group, _, id) in keyed {
        if last.as_ref() != Some(&group) {
            out.push(Vec::new());
            last = Some(group);
        }
        if let Some(g) = out.last_mut() {
            g.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests;
