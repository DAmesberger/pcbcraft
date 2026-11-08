//! `schematic.arrange`: several sheet objects moved, turned or mirrored
//! together, as one undo step.
//!
//! A wire with both ends in the group keeps its shape. One end in the group
//! rubber-bands ([`wire_edit`]). An end is in the group when it is on a named
//! symbol or label, on a label that places itself on a named symbol's pin,
//! on a junction a named wire ends on, or a free end of a named wire. So
//! junctions only ever come along with wires.
use super::clipboard::{ClipObject, MAX_CLIP_OBJECTS};
use super::labels::{self, check_point};
use super::wire_edit;
use super::wiring::{end_position, wire_path};
use super::{Result, invalid, mirrored_on_sheet, not_found, placed_unit, rotated_on_sheet, sheets};
use crate::diagnostics::Error;
use crate::geometry::{Bounds, Point};
use crate::ids::{Id, SheetId};
use crate::library::Rotation;
use crate::model::{LabelKind, Project, SheetItem, Wire, WireEnd};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// What `schematic.arrange` does to the whole group.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Arrangement {
    /// Shift by `by`, in mm.
    Move { by: Point },
    /// A quarter turn counter-clockwise about the point `about`.
    Rotate { about: Point },
    /// Left to right about the vertical line through `about`.
    Mirror { about: Point },
}

impl Arrangement {
    /// Where the arrangement takes a sheet point.
    pub fn point(self, p: Point) -> Point {
        match self {
            Self::Move { by } => [p[0] + by[0], p[1] + by[1]],
            Self::Rotate { about } => [about[0] - (p[1] - about[1]), about[1] + (p[0] - about[0])],
            Self::Mirror { about } => [2.0 * about[0] - p[0], p[1]],
        }
    }

    /// A label's rotation afterwards. A mirrored label keeps what it points
    /// at mirrored: a flag along x turns round, a supply or ground port
    /// standing along y stays as it is.
    fn label(self, kind: LabelKind, rotation: Rotation) -> Rotation {
        match self {
            Self::Move { .. } => rotation,
            Self::Rotate { .. } => rotation.turned(1),
            Self::Mirror { .. } => {
                let along_x = i64::from(labels::auto_rotation(kind, [1.0, 0.0]).quarter_turns());
                Rotation::from_quarter_turns(2 * along_x + 2 - i64::from(rotation.quarter_turns()))
            }
        }
    }

    fn item(self, item: &SheetItem) -> SheetItem {
        match item.clone() {
            SheetItem::Text {
                at,
                text,
                height,
                rotation,
            } => SheetItem::Text {
                at: self.point(at),
                text,
                height,
                rotation: match self {
                    Self::Rotate { .. } => rotation.turned(1),
                    // Text stays readable.
                    Self::Move { .. } | Self::Mirror { .. } => rotation,
                },
            },
            SheetItem::Line { from, to } => SheetItem::Line {
                from: self.point(from),
                to: self.point(to),
            },
            SheetItem::Rect { min, max } => {
                let (a, b) = (self.point(min), self.point(max));
                SheetItem::Rect {
                    min: [a[0].min(b[0]), a[1].min(b[1])],
                    max: [a[0].max(b[0]), a[1].max(b[1])],
                }
            }
        }
    }
}

/// The named objects, checked to lie on the sheet.
#[derive(Default)]
struct Named {
    instances: BTreeSet<Id>,
    labels: BTreeSet<Id>,
    drawings: BTreeSet<Id>,
    wires: BTreeSet<Id>,
}

fn named(project: &Project, sheet: SheetId, objects: &[ClipObject]) -> Result<Named> {
    if objects.is_empty() {
        return Err(invalid("Name at least one object to arrange."));
    }
    if objects.len() > MAX_CLIP_OBJECTS {
        return Err(Error::Limit(
            "Arrange at most 4096 objects at a time.".into(),
        ));
    }
    labels::check_sheet(project, sheet)?;
    let mut out = Named::default();
    for object in objects {
        match *object {
            ClipObject::Instance { id } => {
                if !project.occurrences_of(id).any(|o| o.sheet == sheet) {
                    return Err(not_found("No such symbol on this sheet."));
                }
                out.instances.insert(id);
            }
            ClipObject::Label { id } => {
                project
                    .label(id)
                    .filter(|l| l.sheet == sheet)
                    .ok_or_else(|| not_found("No such label on this sheet."))?;
                out.labels.insert(id);
            }
            ClipObject::Drawing { id } => {
                project
                    .drawing(id)
                    .filter(|d| d.sheet == sheet)
                    .ok_or_else(|| not_found("No such drawing on this sheet."))?;
                out.drawings.insert(id);
            }
            ClipObject::Wire { id } => {
                project
                    .wire(id)
                    .filter(|w| w.sheet == sheet)
                    .ok_or_else(|| not_found("No such wire on this sheet."))?;
                out.wires.insert(id);
            }
        }
    }
    Ok(out)
}

/// Apply `how` to `objects` of `sheet`. The caller discards the candidate on
/// failure.
pub fn arrange(
    project: &mut Project,
    sheet: SheetId,
    objects: &[ClipObject],
    how: Arrangement,
) -> Result<()> {
    let group = named(project, sheet, objects)?;
    match how {
        Arrangement::Move { by: p }
        | Arrangement::Rotate { about: p }
        | Arrangement::Mirror { about: p } => check_point(p)?,
    }
    // All of this reads the sheet as it is before anything moves.
    let before = wire_edit::ends(project);
    let junctions: BTreeSet<Id> = project
        .wires
        .iter()
        .filter(|w| group.wires.contains(&w.id))
        .flat_map(|w| [w.from, w.to])
        .filter_map(WireEnd::junction_id)
        .collect();
    // A label on a named symbol's pin stays on that pin.
    let follows_pin: BTreeSet<Id> = project
        .labels
        .iter()
        .filter(|l| {
            labels::attached_pin(project, l).is_some_and(|e| group.instances.contains(&e.instance))
        })
        .map(|l| l.id)
        .collect();
    let in_group = |wire: &Wire, end: WireEnd| match end {
        WireEnd::Terminal(endpoint) => group.instances.contains(&endpoint.instance),
        WireEnd::Label(l) => group.labels.contains(&l.label) || follows_pin.contains(&l.label),
        WireEnd::Free(_) => group.wires.contains(&wire.id),
        WireEnd::Junction(j) => junctions.contains(&j.junction),
        WireEnd::SheetPin(_) => false,
    };
    let rigid: BTreeSet<Id> = project
        .wires
        .iter()
        .filter(|w| w.sheet == sheet && in_group(w, w.from) && in_group(w, w.to))
        .map(|w| w.id)
        .collect();
    let moved_labels = group
        .labels
        .iter()
        .filter(|id| !follows_pin.contains(id))
        .map(|&id| {
            let label = project
                .label(id)
                .ok_or_else(|| not_found("Unknown label."))?;
            let rotation = how.label(label.kind, labels::current_rotation(project, id)?);
            Ok((id, how.point(labels::anchor(project, label)), rotation))
        })
        .collect::<Result<Vec<_>>>()?;

    for occurrence in project
        .occurrences
        .iter_mut()
        .filter(|o| o.sheet == sheet && group.instances.contains(&o.instance))
    {
        occurrence.position = how.point(occurrence.position);
        check_point(occurrence.position)?;
        (occurrence.rotation, occurrence.mirror) = match how {
            Arrangement::Move { .. } => (occurrence.rotation, occurrence.mirror),
            Arrangement::Rotate { .. } => rotated_on_sheet(occurrence.rotation, occurrence.mirror),
            Arrangement::Mirror { .. } => mirrored_on_sheet(occurrence.rotation, occurrence.mirror),
        };
    }
    for (id, position, rotation) in moved_labels {
        check_point(position)?;
        if let Some(label) = project.labels.iter_mut().find(|l| l.id == id) {
            label.position = position;
            label.rotation = rotation;
            label.manual = true;
        }
    }
    for id in &group.drawings {
        let item = project
            .drawing(*id)
            .map(|d| how.item(&d.item))
            .ok_or_else(|| not_found("Unknown drawing."))?;
        sheets::edit_drawing(project, *id, item)?;
    }
    for junction in project
        .junctions
        .iter_mut()
        .filter(|j| junctions.contains(&j.id))
    {
        junction.position = how.point(junction.position);
        check_point(junction.position)?;
    }

    // Named wires' free ends; rigid wires' corners with them.
    let free_end = |end: WireEnd| match end.free_point() {
        Some(p) => WireEnd::free(how.point(p))
            .ok_or_else(|| invalid("Schematic position must be finite and within ±10000 mm.")),
        None => Ok(end),
    };
    for wire in &mut project.wires {
        if group.wires.contains(&wire.id) {
            wire.from = free_end(wire.from)?;
            wire.to = free_end(wire.to)?;
        }
        if rigid.contains(&wire.id) {
            for corner in &mut wire.corners {
                corner.at = how.point(corner.at);
                check_point(corner.at)?;
            }
        }
    }
    // A named wire with an end held outside the group follows its free end
    // here, because its stored ends no longer read as before; everything
    // else that moved rubber-bands in `follow_moved`.
    let mut rest = before;
    let mut followed = Vec::new();
    for (index, wire) in project.wires.iter().enumerate() {
        if rigid.contains(&wire.id) {
            rest.remove(&wire.id);
        } else if group.wires.contains(&wire.id)
            && let Some([(_, Some(a)), (_, Some(b))]) = rest.remove(&wire.id)
            && let (Some(na), Some(nb)) = (
                end_position(project, wire.from),
                end_position(project, wire.to),
            )
            && let Some(corners) = wire_edit::followed(wire, a, b, na, nb)
        {
            followed.push((index, corners));
        }
    }
    for (index, corners) in followed {
        if let Some(wire) = project.wires.get_mut(index) {
            wire.corners = corners;
        }
    }
    wire_edit::follow_moved(project, &rest);
    Ok(())
}

/// The middle of what `objects` of `sheet` draw, from their pins, anchors
/// and vertices: the point a group turns or mirrors about. Text does not
/// count, so a turn leaves the middle where it is.
pub fn centre(project: &Project, sheet: SheetId, objects: &[ClipObject]) -> Option<Point> {
    Bounds::of(points(project, sheet, objects)).map(|b| b.center())
}

/// The pins, anchors and vertices of what `objects` of `sheet` draw.
fn points(project: &Project, sheet: SheetId, objects: &[ClipObject]) -> Vec<Point> {
    let mut points: Vec<Point> = Vec::new();
    for object in objects {
        match *object {
            ClipObject::Instance { id } => {
                for occurrence in project.occurrences_of(id).filter(|o| o.sheet == sheet) {
                    points.push(occurrence.position);
                    if let Ok(unit) = placed_unit(project, occurrence.id) {
                        points.extend(unit.pins.iter().map(|p| p.end));
                    }
                }
            }
            ClipObject::Label { id } => {
                points.extend(project.label(id).map(|l| labels::anchor(project, l)));
            }
            ClipObject::Drawing { id } => match project.drawing(id).map(|d| &d.item) {
                Some(SheetItem::Text { at, .. }) => points.push(*at),
                Some(SheetItem::Line { from, to }) => points.extend([*from, *to]),
                Some(SheetItem::Rect { min, max }) => points.extend([*min, *max]),
                None => {}
            },
            ClipObject::Wire { id } => {
                points.extend(
                    project
                        .wire(id)
                        .and_then(|w| wire_path(project, w))
                        .unwrap_or_default(),
                );
            }
        }
    }
    points
}

/// The point to turn `objects` of `sheet` a quarter about: `about` (their
/// middle, on the grid), moved so a group inside the sheet's frame lands
/// inside it again where it fits there. The group moves by whole steps of twice
/// `grid` (`about` by whole steps of `grid`), so what was on the grid stays
/// on it.
pub fn turn_about(
    project: &Project,
    sheet: SheetId,
    objects: &[ClipObject],
    about: Point,
    grid: f64,
) -> Point {
    let (Some(page), Some(drawn)) = (project.sheet(sheet), extent(project, sheet, objects)) else {
        return about;
    };
    let [w, h] = page.size_mm();
    let inner = super::scene::FRAME_INNER_MM;
    let frame = Bounds {
        min: [inner, -(h - inner)],
        max: [w - inner, -inner],
    };
    // A group already off the sheet is not pulled onto it.
    let inside = |b: &Bounds| {
        b.min[0] >= frame.min[0]
            && b.min[1] >= frame.min[1]
            && b.max[0] <= frame.max[0]
            && b.max[1] <= frame.max[1]
    };
    if !inside(&drawn) {
        return about;
    }
    let how = Arrangement::Rotate { about };
    let Some(turned) = Bounds::of(
        [
            drawn.min,
            drawn.max,
            [drawn.min[0], drawn.max[1]],
            [drawn.max[0], drawn.min[1]],
        ]
        .map(|p| how.point(p)),
    ) else {
        return about;
    };
    let step = if grid.is_finite() && grid > 0.0 {
        2.0 * grid
    } else {
        1.0
    };
    // The shift that brings each side in, in whole steps; none on an axis
    // the group is wider than the frame.
    let shift = |k: usize| {
        let (low, high) = (frame.min[k] - turned.min[k], frame.max[k] - turned.max[k]);
        if low > high {
            0.0
        } else if low > 0.0 {
            (low / step).ceil() * step
        } else if high < 0.0 {
            (high / step).floor() * step
        } else {
            0.0
        }
    };
    let t = [shift(0), shift(1)];
    // A turn about `about + c` lands the group `(c - turned c)` further:
    // c = ((tx - ty) / 2, (tx + ty) / 2) moves it by t.
    [
        about[0] + (t[0] - t[1]) / 2.0,
        about[1] + (t[0] + t[1]) / 2.0,
    ]
}

/// The extent of what `objects` of `sheet` draw, text included.
fn extent(project: &Project, sheet: SheetId, objects: &[ClipObject]) -> Option<Bounds> {
    let m = crate::library::TextMetric::default();
    let mut boxes: Vec<Bounds> = Vec::new();
    for object in objects {
        if let ClipObject::Instance { id } = *object {
            boxes.extend(
                project
                    .occurrences_of(id)
                    .filter(|o| o.sheet == sheet)
                    .filter_map(|o| placed_unit(project, o.id).ok()?.bounds(m)),
            );
        }
    }
    let middle = objects
        .iter()
        .filter(|o| !matches!(o, ClipObject::Instance { .. }))
        .copied()
        .collect::<Vec<_>>();
    boxes.extend(Bounds::of(points(project, sheet, &middle)));
    boxes.into_iter().reduce(Bounds::union)
}

/// Remove the wires, labels and drawings of `objects`, in that order (the
/// wires through a removed label stay joined), and return the named
/// instances, which the reviewed update removes with their wires. The caller
/// discards the candidate on failure.
pub fn remove_objects(project: &mut Project, objects: &[ClipObject]) -> Result<Vec<Id>> {
    if objects.is_empty() {
        return Err(invalid("Name at least one object to remove."));
    }
    if objects.len() > MAX_CLIP_OBJECTS {
        return Err(Error::Limit(
            "Remove at most 4096 objects at a time.".into(),
        ));
    }
    let mut instances = Vec::new();
    let mut seen = BTreeSet::new();
    let mut ordered: Vec<ClipObject> = objects.to_vec();
    ordered.sort_by_key(|o| match o {
        ClipObject::Wire { .. } => 0u8,
        ClipObject::Label { .. } => 1,
        ClipObject::Drawing { .. } => 2,
        ClipObject::Instance { .. } => 3,
    });
    for object in ordered {
        match object {
            ClipObject::Wire { id } if seen.insert(id) => super::disconnect(project, id)?,
            ClipObject::Label { id } if seen.insert(id) => labels::remove(project, id)?,
            ClipObject::Drawing { id } if seen.insert(id) => sheets::remove_drawing(project, id)?,
            ClipObject::Instance { id } if seen.insert(id) => instances.push(id),
            _ => {}
        }
    }
    Ok(instances)
}
