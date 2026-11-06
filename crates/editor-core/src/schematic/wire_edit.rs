//! Rubber-banding: how a wire's path follows when an object one of its ends
//! is attached to moves, turns or mirrors (docs/schematic-wires.md, "Moving
//! parts, labels and sheet symbols").
//!
//! The segment on a moved end keeps its direction: the corner next to it
//! slides along the following segment when that keeps every segment
//! horizontal or vertical, and a new corner is added only when the segment
//! cannot stay straight. Every other vertex stays, and a segment that
//! shrinks to nothing is dropped. [`follow`] is pure, so the canvas's drag
//! preview and the stored result are the same function.
//!
//! The path edits (docs/schematic-wires.md, "Editing a selected path") work
//! the same way: [`move_corner`], [`move_segment`], [`insert_corner`],
//! [`remove_corner`], [`move_end`] and [`split_at`] are pure functions of a
//! path, which the canvas previews and the `wire.*` commands store.
use super::wiring::{self, JUNCTION_TOLERANCE_MM, WireTarget, end_position, simplified};
use super::{Error, MAX_CORNERS, MAX_WIRES, Project, Result, invalid, not_found};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::instances;
use crate::model::{Corner, Wire, WireEnd};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One end of a wire.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WireSide {
    From,
    To,
}

/// Whether a path's first and last vertices may move: a free end may, and
/// an end attached to an object may not.
pub type Movable = [bool; 2];

/// Whether each end of `wire` is free.
pub fn movable(wire: &Wire) -> Movable {
    [wire.from, wire.to].map(|end| end.free_point().is_some())
}

fn corners_of(path: &[Point]) -> Vec<Corner> {
    path.get(1..path.len().saturating_sub(1))
        .unwrap_or_default()
        .iter()
        .map(|&at| Corner { at })
        .collect()
}

/// `path` without corners that add nothing; its ends stay.
fn cleaned(path: &[Point]) -> Vec<Point> {
    let (Some(&first), Some(&last)) = (path.first(), path.last()) else {
        return path.to_vec();
    };
    let mut kept = vec![first];
    kept.extend(
        simplified(first, corners_of(path), last)
            .into_iter()
            .map(|c| c.at),
    );
    kept.push(last);
    kept
}

/// `rest`, the vertices from one neighbour of a vertex at `c` out to the
/// path's end, after that vertex moves by `d`: the segment from `c` keeps its
/// direction, so the neighbour takes the offset across it, together with the
/// run of vertices in line with it. An end that cannot move gets a new corner
/// before it, so the segment steps instead of slanting.
fn carried(c: Point, rest: &[Point], end_moves: bool, d: Point) -> Vec<Point> {
    let mut out = rest.to_vec();
    let Some(&first) = rest.first() else {
        return out;
    };
    let along = axis(c, first);
    let shift = match along {
        Axis::Horizontal => [0.0, d[1]],
        Axis::Vertical => [d[0], 0.0],
        Axis::Other => return out,
    };
    for (i, &here) in rest.iter().enumerate() {
        let next = rest.get(i + 1).copied();
        let moved = [here[0] + shift[0], here[1] + shift[1]];
        if next.is_none() && !end_moves {
            out.insert(i, moved);
            break;
        }
        out[i] = moved;
        if next.is_none_or(|n| axis(here, n) != along) {
            break;
        }
    }
    out
}

/// `path` with corner `index` (vertex `index + 1`) at `to`; `None` when there
/// is no such corner. Unless `free`, both of its segments keep their
/// directions ([`carried`]). A corner in line between its neighbours, as a
/// new one is, breaks the line into a step: the segment before it stops at
/// the corner's new position and turns to reach it.
pub fn move_corner(
    path: &[Point],
    movable: Movable,
    index: usize,
    to: Point,
    free: bool,
) -> Option<Vec<Point>> {
    let v = index.checked_add(1)?;
    let (before, after) = (path.get(..v)?, path.get(v + 1..)?);
    let (c, next) = (*path.get(v)?, *after.first()?);
    if free {
        let mut out = path.to_vec();
        *out.get_mut(v)? = to;
        return Some(cleaned(&out));
    }
    let d = [to[0] - c[0], to[1] - c[1]];
    let reversed: Vec<Point> = before.iter().rev().copied().collect();
    let previous = *reversed.first()?;
    let mut out = if axis(previous, c) == axis(c, next) {
        let turn = match axis(previous, c) {
            Axis::Horizontal => [to[0], previous[1]],
            Axis::Vertical => [previous[0], to[1]],
            Axis::Other => previous,
        };
        let mut out = before.to_vec();
        out.push(turn);
        out
    } else {
        let mut out = carried(c, &reversed, movable[0], d);
        out.reverse();
        out
    };
    out.push(to);
    out.extend(carried(c, after, movable[1], d));
    Some(cleaned(&out))
}

/// The unit normal a segment's offset is measured along: +y for a horizontal
/// segment, +x for a vertical one, the side of +x for a slanted one, whichever
/// way it runs. `None` for a segment of no length.
pub fn segment_normal(a: Point, b: Point) -> Option<Point> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx.hypot(dy);
    if length <= JUNCTION_TOLERANCE_MM {
        return None;
    }
    let n = [-dy / length, dx / length];
    Some(if n[0] < -1e-12 || (n[0].abs() <= 1e-12 && n[1] < 0.0) {
        [-n[0], -n[1]]
    } else {
        n
    })
}

/// `path` with segment `index` (vertex `index` to `index + 1`) moved by
/// `offset` along its [`segment_normal`]; `None` when there is no such
/// segment or it has no length. Its neighbours stretch. An end that cannot
/// move, or a neighbour in line with it, stays and gets a short perpendicular
/// segment instead.
pub fn move_segment(
    path: &[Point],
    movable: Movable,
    index: usize,
    offset: f64,
) -> Option<Vec<Point>> {
    let far = index.checked_add(1)?;
    let (a, b) = (*path.get(index)?, *path.get(far)?);
    let n = segment_normal(a, b)?;
    let moved = |p: Point| [p[0] + n[0] * offset, p[1] + n[1] * offset];
    let along = axis(a, b);
    let in_line = |q: &Point, p: Point| along != Axis::Other && axis(*q, p) == along;
    let keeps = |neighbour: Option<&Point>, end_moves: bool, p: Point| match neighbour {
        None => !end_moves,
        Some(q) => in_line(q, p),
    };
    let previous = index.checked_sub(1).and_then(|i| path.get(i));
    let mut out = path.get(..index)?.to_vec();
    if keeps(previous, movable[0], a) {
        out.push(a);
    }
    out.push(moved(a));
    out.push(moved(b));
    if keeps(path.get(far + 1), movable[1], b) {
        out.push(b);
    }
    out.extend_from_slice(path.get(far + 1..)?);
    Some(cleaned(&out))
}

/// `path` with a corner at `at`, a point on it away from its vertices, and
/// the new corner's index; the shape is unchanged. `None` off the path or on
/// a vertex.
pub fn insert_corner(path: &[Point], at: Point) -> Option<(Vec<Point>, usize)> {
    let (on, segment) = wiring::nearest_on_path(path, at)?;
    if !same(on, at) || path.iter().any(|v| same(*v, on)) {
        return None;
    }
    let mut out = path.to_vec();
    out.insert(segment + 1, on);
    Some((out, segment))
}

/// `path` without corner `index`; `None` when there is no such corner.
pub fn remove_corner(path: &[Point], index: usize) -> Option<Vec<Point>> {
    let v = index.checked_add(1)?;
    if v + 1 >= path.len() {
        return None;
    }
    let mut out = path.to_vec();
    out.remove(v);
    Some(cleaned(&out))
}

/// `path` with the end `side` at `to`, rubber-banded as when the object an
/// end is on moves ([`follow`]).
pub fn move_end(path: &[Point], side: WireSide, to: Point) -> Vec<Point> {
    let (Some(&first), Some(&last)) = (path.first(), path.last()) else {
        return path.to_vec();
    };
    match side {
        WireSide::From => follow(path, to, last),
        WireSide::To => follow(path, first, to),
    }
}

/// What is left when a segment goes: the path up to its first vertex, and
/// from its second, each `None` when it has no segment.
pub type Split = (Option<Vec<Point>>, Option<Vec<Point>>);

/// The two paths left without segment `index`; `None` when there is no such
/// segment.
pub fn split_at(path: &[Point], index: usize) -> Option<Split> {
    let far = index.checked_add(1).filter(|i| *i < path.len())?;
    let piece = |p: &[Point]| (p.len() >= 2).then(|| p.to_vec());
    Some((piece(path.get(..=index)?), piece(path.get(far..)?)))
}

/// A wire's drawn path and which ends may move.
fn editable(project: &Project, id: Id) -> Result<(Vec<Point>, Movable)> {
    let wire = project
        .wire(id)
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    let path = wiring::wire_path(project, wire)
        .ok_or_else(|| invalid("This wire is not drawn on the sheet."))?;
    Ok((path, movable(wire)))
}

fn wire_mut(project: &mut Project, id: Id) -> Result<&mut Wire> {
    project
        .wires
        .iter_mut()
        .find(|w| w.id == id)
        .ok_or_else(|| not_found("Unknown schematic wire."))
}

/// Store `path` as wire `id`'s geometry: its corners, and where its free
/// ends are. An attached end's position stays with its object.
fn store(project: &mut Project, id: Id, path: &[Point]) -> Result<()> {
    if !path.iter().all(|p| point_valid(*p)) {
        return Err(off_sheet());
    }
    let corners = corners_of(path);
    if corners.len() > MAX_CORNERS {
        return Err(Error::Limit("A wire has at most 256 corners.".into()));
    }
    let (Some(&first), Some(&last)) = (path.first(), path.last()) else {
        return Err(invalid("A wire must connect two different points."));
    };
    let wire = wire_mut(project, id)?;
    if wire.from.free_point().is_some() {
        wire.from = WireEnd::free(first).ok_or_else(off_sheet)?;
    }
    if wire.to.free_point().is_some() {
        wire.to = WireEnd::free(last).ok_or_else(off_sheet)?;
    }
    wire.corners = corners;
    if wire.from == wire.to && wire.corners.is_empty() {
        return Err(invalid("A wire must connect two different points."));
    }
    Ok(())
}

/// The end a wire takes for `target`, on the wire's own sheet: never a point
/// on the wire itself.
fn new_end(project: &mut Project, id: Id, target: WireTarget) -> Result<WireEnd> {
    if matches!(target, WireTarget::OnWire(p) if p.wire == id) {
        return Err(invalid("A wire cannot join two points of the same wire."));
    }
    let sheet = wire_mut(project, id)?.sheet;
    let end = wiring::resolve(project, wiring::on_its_pin(project, target))?;
    if wiring::end_sheet(project, end).is_some_and(|s| s != sheet) {
        return Err(invalid(
            "A wire joins points on one sheet; connect sheets with labels.",
        ));
    }
    Ok(end)
}

/// `check_instance` owns terminal validity, as when a wire is drawn.
fn check_ends(project: &Project, id: Id) -> Result<()> {
    let wire = project
        .wire(id)
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    for end in [wire.from, wire.to] {
        if let Some(endpoint) = end.terminal() {
            instances::check_instance(project, endpoint.instance)?;
        }
    }
    Ok(())
}

fn off_sheet() -> Error {
    invalid("Schematic position must be finite and within ±10000 mm.")
}

fn no_corner() -> Error {
    invalid("This wire has no such corner.")
}

fn no_segment() -> Error {
    invalid("This wire has no such segment.")
}

pub(super) fn set_path(
    project: &mut Project,
    id: Id,
    corners: Vec<Corner>,
    from: Option<WireTarget>,
    to: Option<WireTarget>,
) -> Result<()> {
    if corners.len() > MAX_CORNERS {
        return Err(Error::Limit("A wire has at most 256 corners.".into()));
    }
    if !corners.iter().all(|c| point_valid(c.at)) {
        return Err(off_sheet());
    }
    if let Some(target) = from {
        let end = new_end(project, id, target)?;
        wire_mut(project, id)?.from = end;
    }
    if let Some(target) = to {
        let end = new_end(project, id, target)?;
        wire_mut(project, id)?.to = end;
    }
    let wire = wire_mut(project, id)?;
    wire.corners = corners;
    if wire.from == wire.to && wire.corners.is_empty() {
        return Err(invalid("A wire must connect two different points."));
    }
    check_ends(project, id)
}

pub(super) fn apply_move_corner(
    project: &mut Project,
    id: Id,
    index: usize,
    to: Point,
    free: bool,
) -> Result<()> {
    if !point_valid(to) {
        return Err(off_sheet());
    }
    let (path, movable) = editable(project, id)?;
    let path = move_corner(&path, movable, index, to, free).ok_or_else(no_corner)?;
    store(project, id, &path)
}

pub(super) fn apply_move_segment(
    project: &mut Project,
    id: Id,
    index: usize,
    offset: f64,
) -> Result<()> {
    if !offset.is_finite() {
        return Err(invalid("A segment's offset must be finite."));
    }
    let (path, movable) = editable(project, id)?;
    let path = move_segment(&path, movable, index, offset).ok_or_else(no_segment)?;
    store(project, id, &path)
}

pub(super) fn apply_insert_corner(project: &mut Project, id: Id, at: Point) -> Result<()> {
    if !point_valid(at) {
        return Err(off_sheet());
    }
    let (path, _) = editable(project, id)?;
    let (path, _) = insert_corner(&path, at)
        .ok_or_else(|| invalid("A new corner goes on the wire, away from its corners and ends."))?;
    store(project, id, &path)
}

pub(super) fn apply_remove_corner(project: &mut Project, id: Id, index: usize) -> Result<()> {
    let (path, _) = editable(project, id)?;
    let path = remove_corner(&path, index).ok_or_else(no_corner)?;
    store(project, id, &path)
}

/// Remove a segment: the part before it keeps the wire's ID, and the part
/// after is a new wire, with free ends at the gap. Without a part before, the
/// part after keeps the ID; with neither, the wire goes.
pub(super) fn remove_segment(project: &mut Project, id: Id, index: usize) -> Result<()> {
    let (path, _) = editable(project, id)?;
    let (head, tail) = split_at(&path, index).ok_or_else(no_segment)?;
    if head.is_some() && tail.is_some() && project.wires.len() >= MAX_WIRES {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires.".into(),
        ));
    }
    let position = project
        .wires
        .iter()
        .position(|w| w.id == id)
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    let wire = project.wires.remove(position);
    let free = |p: Option<&Point>| p.and_then(|p| WireEnd::free(*p)).ok_or_else(off_sheet);
    let mut at = position;
    if let Some(head) = &head {
        let piece = Wire {
            to: free(head.last())?,
            corners: corners_of(head),
            ..wire.clone()
        };
        project.wires.insert(at, piece);
        at += 1;
    }
    if let Some(tail) = &tail {
        let piece = Wire {
            id: if head.is_some() {
                project.allocate()?
            } else {
                id
            },
            sheet: wire.sheet,
            from: free(tail.first())?,
            to: wire.to,
            corners: corners_of(tail),
        };
        project.wires.insert(at, piece);
    }
    Ok(())
}

pub(super) fn apply_move_end(
    project: &mut Project,
    id: Id,
    side: WireSide,
    to: WireTarget,
) -> Result<()> {
    let (path, _) = editable(project, id)?;
    let end = new_end(project, id, to)?;
    let at = end_position(project, end).ok_or_else(|| invalid("That point is not drawn."))?;
    let moved = move_end(&path, side, at);
    let wire = wire_mut(project, id)?;
    match side {
        WireSide::From => wire.from = end,
        WireSide::To => wire.to = end,
    }
    store(project, id, &moved)?;
    check_ends(project, id)
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Horizontal,
    Vertical,
    /// Slanted or of no length.
    Other,
}

fn axis(a: Point, b: Point) -> Axis {
    let (dx, dy) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
    match (dx > JUNCTION_TOLERANCE_MM, dy > JUNCTION_TOLERANCE_MM) {
        (true, false) => Axis::Horizontal,
        (false, true) => Axis::Vertical,
        _ => Axis::Other,
    }
}

fn same(a: Point, b: Point) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= JUNCTION_TOLERANCE_MM
}

/// `path` (from its first vertex) after that first vertex moves to `to`.
pub fn follow_end(path: &[Point], to: Point) -> Vec<Point> {
    let mut out = path.to_vec();
    let (Some(&first), Some(&next)) = (path.first(), path.get(1)) else {
        return out;
    };
    out[0] = to;
    let first_axis = axis(first, next);
    // The next vertex is a corner: slide it along the second segment.
    if let Some(&after) = path.get(2) {
        match (first_axis, axis(next, after)) {
            (Axis::Horizontal, Axis::Vertical) => {
                out[1] = [next[0], to[1]];
                return out;
            }
            (Axis::Vertical, Axis::Horizontal) => {
                out[1] = [to[0], next[1]];
                return out;
            }
            _ => {}
        }
    }
    // The next vertex stays. A segment that is still straight stays as it
    // is; otherwise it steps through a new corner, leaving the moved end in
    // its old direction (horizontal first when it had none).
    if matches!(axis(to, next), Axis::Horizontal | Axis::Vertical) || same(to, next) {
        return out;
    }
    let corner = match first_axis {
        Axis::Vertical => [to[0], next[1]],
        Axis::Horizontal | Axis::Other => [next[0], to[1]],
    };
    out.insert(1, corner);
    out
}

/// A wire's drawn path `path` after its ends move to `from` and `to`,
/// without corners that add nothing. Both ends moved alike (a unit with a
/// wire between two of its own pins) carries the whole path along.
pub fn follow(path: &[Point], from: Point, to: Point) -> Vec<Point> {
    let (Some(&a), Some(&b)) = (path.first(), path.last()) else {
        return path.to_vec();
    };
    let (da, db) = (
        [from[0] - a[0], from[1] - a[1]],
        [to[0] - b[0], to[1] - b[1]],
    );
    let mut out = if same(da, db) {
        path.iter().map(|p| [p[0] + da[0], p[1] + da[1]]).collect()
    } else {
        let mut out = if same(a, from) {
            path.to_vec()
        } else {
            follow_end(path, from)
        };
        if !same(b, to) {
            out.reverse();
            out = follow_end(&out, to);
            out.reverse();
        }
        out
    };
    // The ends are where the objects are, exactly.
    if let Some(first) = out.first_mut() {
        *first = from;
    }
    if let Some(last) = out.last_mut() {
        *last = to;
    }
    let corners = out
        .get(1..out.len().saturating_sub(1))
        .unwrap_or_default()
        .iter()
        .map(|&at| Corner { at })
        .collect();
    let mut kept = vec![from];
    kept.extend(simplified(from, corners, to).into_iter().map(|c| c.at));
    kept.push(to);
    kept
}

/// Each wire's ends and where they are drawn, taken before an edit that
/// moves objects.
pub(super) type Ends = BTreeMap<Id, [(WireEnd, Option<Point>); 2]>;

pub(super) fn ends(project: &Project) -> Ends {
    project
        .wires
        .iter()
        .map(|w| {
            (
                w.id,
                [
                    (w.from, end_position(project, w.from)),
                    (w.to, end_position(project, w.to)),
                ],
            )
        })
        .collect()
}

/// `wire`'s corners once its ends, drawn at `a` and `b` with these corners,
/// are at `na` and `nb`; `None` past the corner limit.
pub(super) fn followed(
    wire: &Wire,
    a: Point,
    b: Point,
    na: Point,
    nb: Point,
) -> Option<Vec<Corner>> {
    let mut path = Vec::with_capacity(wire.corners.len() + 2);
    path.push(a);
    path.extend(wire.corners.iter().map(|c| c.at));
    path.push(b);
    let path = follow(&path, na, nb);
    let corners: Vec<Corner> = path
        .get(1..path.len().saturating_sub(1))
        .unwrap_or_default()
        .iter()
        .map(|&at| Corner { at })
        .collect();
    (corners.len() <= MAX_CORNERS).then_some(corners)
}

/// Rubber-band every wire whose attached end moved since `before` was
/// taken. A wire that would pass the corner limit keeps its corners.
pub(super) fn follow_moved(project: &mut Project, before: &Ends) {
    let mut moved = Vec::new();
    for (index, wire) in project.wires.iter().enumerate() {
        let Some([(from, Some(a)), (to, Some(b))]) = before.get(&wire.id) else {
            continue;
        };
        if *from != wire.from || *to != wire.to {
            continue;
        }
        let (Some(na), Some(nb)) = (end_position(project, *from), end_position(project, *to))
        else {
            continue;
        };
        if same(*a, na) && same(*b, nb) {
            continue;
        }
        if let Some(corners) = followed(wire, *a, *b, na, nb) {
            moved.push((index, corners));
        }
    }
    for (index, corners) in moved {
        if let Some(wire) = project.wires.get_mut(index) {
            wire.corners = corners;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (case, path before, new first end, new last end, path after)
    type Case = (&'static str, Vec<Point>, Point, Point, Vec<Point>);

    /// (case, path before, movable ends, corner, new position, path after)
    type CornerCase = (&'static str, Vec<Point>, Movable, usize, Point, Vec<Point>);

    #[test]
    fn a_corner_drag_keeps_both_segments_directions() {
        let attached = [false, false];
        let cases: Vec<CornerCase> = vec![
            (
                "dogleg between attached ends, corner 0 moved across: neighbour corner comes along",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [20.0, 10.0]],
                attached,
                0,
                [12.5, 2.5],
                vec![
                    [0.0, 0.0],
                    [0.0, 2.5],
                    [12.5, 2.5],
                    [12.5, 10.0],
                    [20.0, 10.0],
                ],
            ),
            (
                "L between attached ends: each end steps",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                attached,
                0,
                [15.0, 5.0],
                vec![
                    [0.0, 0.0],
                    [0.0, 5.0],
                    [15.0, 5.0],
                    [15.0, 10.0],
                    [10.0, 10.0],
                ],
            ),
            (
                "L, moved along one segment only: no step on that side",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                attached,
                0,
                [15.0, 0.0],
                vec![[0.0, 0.0], [15.0, 0.0], [15.0, 10.0], [10.0, 10.0]],
            ),
            (
                "L with free far end: the free end comes along",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [false, true],
                0,
                [15.0, 0.0],
                vec![[0.0, 0.0], [15.0, 0.0], [15.0, 10.0]],
            ),
            (
                "both ends free: nothing is added",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [true, true],
                0,
                [15.0, 5.0],
                vec![[0.0, 5.0], [15.0, 5.0], [15.0, 10.0]],
            ),
            (
                "a corner in line breaks the line into a step",
                vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]],
                attached,
                0,
                [10.0, 5.0],
                vec![
                    [0.0, 0.0],
                    [10.0, 0.0],
                    [10.0, 5.0],
                    [20.0, 5.0],
                    [20.0, 0.0],
                ],
            ),
            (
                "a corner in line, a corner next: the step ends on that corner",
                vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [20.0, 10.0]],
                attached,
                0,
                [10.0, 5.0],
                vec![
                    [0.0, 0.0],
                    [10.0, 0.0],
                    [10.0, 5.0],
                    [20.0, 5.0],
                    [20.0, 10.0],
                ],
            ),
            (
                "a corner in line moved along it, or back: it goes",
                vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]],
                attached,
                0,
                [12.5, 0.0],
                vec![[0.0, 0.0], [20.0, 0.0]],
            ),
            (
                "the far corner is in line: the run moves together",
                vec![
                    [0.0, 0.0],
                    [0.0, 10.0],
                    [5.0, 10.0],
                    [10.0, 10.0],
                    [10.0, 0.0],
                ],
                attached,
                2,
                [10.0, 12.5],
                vec![[0.0, 0.0], [0.0, 12.5], [10.0, 12.5], [10.0, 0.0]],
            ),
            (
                "a slanted segment stays slanted, its far vertex stays",
                vec![[0.0, 0.0], [10.0, 5.0], [20.0, 5.0]],
                attached,
                0,
                [10.0, 7.5],
                vec![[0.0, 0.0], [10.0, 7.5], [20.0, 7.5], [20.0, 5.0]],
            ),
            (
                "dragged onto its next corner: the empty segment goes",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [20.0, 10.0]],
                attached,
                0,
                [10.0, 10.0],
                vec![[0.0, 0.0], [0.0, 10.0], [20.0, 10.0]],
            ),
        ];
        for (case, path, movable, index, to, expected) in cases {
            assert_eq!(
                move_corner(&path, movable, index, to, false),
                Some(expected),
                "{case}"
            );
        }
    }

    #[test]
    fn a_free_corner_drag_moves_only_the_corner() {
        let path = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        assert_eq!(
            move_corner(&path, [false, false], 0, [7.5, 2.5], true),
            Some(vec![[0.0, 0.0], [7.5, 2.5], [10.0, 10.0]])
        );
    }

    #[test]
    fn a_corner_index_past_the_corners_is_none() {
        let path = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        for index in [1, 2, usize::MAX] {
            assert_eq!(
                move_corner(&path, [true, true], index, [1.0, 1.0], false),
                None
            );
            assert_eq!(remove_corner(&path, index), None);
        }
    }

    /// (case, path before, movable ends, segment, offset, path after)
    type SegmentCase = (&'static str, Vec<Point>, Movable, usize, f64, Vec<Point>);

    #[test]
    fn a_segment_drag_moves_it_along_its_normal() {
        let attached = [false, false];
        let cases: Vec<SegmentCase> = vec![
            (
                "the middle of a dogleg: neighbours stretch",
                vec![[0.0, 0.0], [5.0, 0.0], [5.0, 10.0], [20.0, 10.0]],
                attached,
                1,
                2.5,
                vec![[0.0, 0.0], [7.5, 0.0], [7.5, 10.0], [20.0, 10.0]],
            ),
            (
                "on an attached end: a short perpendicular segment is added",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                attached,
                0,
                2.5,
                vec![[0.0, 0.0], [0.0, 2.5], [10.0, 2.5], [10.0, 10.0]],
            ),
            (
                "straight between attached ends: both ends step",
                vec![[0.0, 0.0], [10.0, 0.0]],
                attached,
                0,
                -5.0,
                vec![[0.0, 0.0], [0.0, -5.0], [10.0, -5.0], [10.0, 0.0]],
            ),
            (
                "on a free end: the free end moves",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [true, false],
                0,
                2.5,
                vec![[0.0, 2.5], [10.0, 2.5], [10.0, 10.0]],
            ),
            (
                "the offset's sign does not depend on the way the segment runs",
                vec![[10.0, 0.0], [0.0, 0.0]],
                [true, true],
                0,
                2.5,
                vec![[10.0, 2.5], [0.0, 2.5]],
            ),
            (
                "a vertical segment's offset is in x",
                vec![[0.0, 10.0], [0.0, 0.0]],
                [true, true],
                0,
                2.5,
                vec![[2.5, 10.0], [2.5, 0.0]],
            ),
            (
                "with an in-line neighbour: that corner stays and a step is added",
                vec![[0.0, 0.0], [5.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                attached,
                1,
                2.5,
                vec![
                    [0.0, 0.0],
                    [5.0, 0.0],
                    [5.0, 2.5],
                    [10.0, 2.5],
                    [10.0, 10.0],
                ],
            ),
            (
                "moved onto a neighbour's far end's line: the empty segment goes",
                vec![[0.0, 0.0], [5.0, 0.0], [5.0, 10.0], [20.0, 10.0]],
                attached,
                1,
                -5.0,
                vec![[0.0, 0.0], [0.0, 10.0], [20.0, 10.0]],
            ),
        ];
        for (case, path, movable, index, offset, expected) in cases {
            assert_eq!(
                move_segment(&path, movable, index, offset),
                Some(expected),
                "{case}"
            );
        }
        let path = [[0.0, 0.0], [10.0, 0.0]];
        assert_eq!(move_segment(&path, attached, 1, 1.0), None, "no segment 1");
        let empty = [[0.0, 0.0], [0.0, 0.0]];
        assert_eq!(move_segment(&empty, attached, 0, 1.0), None, "no length");
    }

    #[test]
    fn inserting_a_corner_keeps_the_shape() {
        let path = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        assert_eq!(
            insert_corner(&path, [5.0, 0.0]),
            Some((vec![[0.0, 0.0], [5.0, 0.0], [10.0, 0.0], [10.0, 10.0]], 0))
        );
        assert_eq!(
            insert_corner(&path, [10.0, 5.0]),
            Some((vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [10.0, 10.0]], 1))
        );
        for (case, at) in [
            ("off the path", [5.0, 5.0]),
            ("on a corner", [10.0, 0.0]),
            ("on an end", [0.0, 0.0]),
        ] {
            assert_eq!(insert_corner(&path, at), None, "{case}");
        }
    }

    #[test]
    fn removing_a_corner_merges_its_segments() {
        let path = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        assert_eq!(
            remove_corner(&path, 0),
            Some(vec![[0.0, 0.0], [10.0, 10.0]]),
            "the merged segment may slant"
        );
        let in_line = [[0.0, 0.0], [5.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        assert_eq!(
            remove_corner(&in_line, 0),
            Some(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]])
        );
    }

    #[test]
    fn moving_an_end_rubber_bands_as_a_moved_object_does() {
        let path = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        assert_eq!(
            move_end(&path, WireSide::To, [15.0, 10.0]),
            vec![[0.0, 0.0], [15.0, 0.0], [15.0, 10.0]],
            "the last segment keeps its direction"
        );
        assert_eq!(
            move_end(&path, WireSide::From, [0.0, 5.0]),
            vec![[0.0, 5.0], [10.0, 5.0], [10.0, 10.0]]
        );
    }

    #[test]
    fn removing_a_segment_leaves_the_pieces_either_side() {
        let path = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [20.0, 10.0]];
        let cases = [
            (
                "the middle: two pieces",
                1,
                Some(vec![[0.0, 0.0], [10.0, 0.0]]),
                Some(vec![[10.0, 10.0], [20.0, 10.0]]),
            ),
            (
                "the first: only the rest",
                0,
                None,
                Some(vec![[10.0, 0.0], [10.0, 10.0], [20.0, 10.0]]),
            ),
            (
                "the last: only the start",
                2,
                Some(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]),
                None,
            ),
        ];
        for (case, index, head, tail) in cases {
            assert_eq!(split_at(&path, index), Some((head, tail)), "{case}");
        }
        assert_eq!(split_at(&path, 3), None);
        assert_eq!(split_at(&[[0.0, 0.0], [1.0, 0.0]], 0), Some((None, None)));
    }

    #[test]
    fn follow_keeps_the_moved_segment_orthogonal() {
        let cases: Vec<Case> = vec![
            (
                "straight, moved along itself: stays straight",
                vec![[0.0, 0.0], [10.0, 0.0]],
                [-5.0, 0.0],
                [10.0, 0.0],
                vec![[-5.0, 0.0], [10.0, 0.0]],
            ),
            (
                "straight, moved off its line: one corner, the moved end keeps its direction",
                vec![[0.0, 0.0], [10.0, 0.0]],
                [0.0, 5.0],
                [10.0, 0.0],
                vec![[0.0, 5.0], [10.0, 5.0], [10.0, 0.0]],
            ),
            (
                "vertical straight, moved sideways",
                vec![[0.0, 0.0], [0.0, 10.0]],
                [5.0, 0.0],
                [0.0, 10.0],
                vec![[5.0, 0.0], [5.0, 10.0], [0.0, 10.0]],
            ),
            (
                "L, horizontal move: the segment stretches",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [3.0, 0.0],
                [10.0, 10.0],
                vec![[3.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
            ),
            (
                "L, vertical move: the corner slides along the second segment",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [0.0, 4.0],
                [10.0, 10.0],
                vec![[0.0, 4.0], [10.0, 4.0], [10.0, 10.0]],
            ),
            (
                "L, diagonal move: the corner slides, nothing is added",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [-2.5, 2.5],
                [10.0, 10.0],
                vec![[-2.5, 2.5], [10.0, 2.5], [10.0, 10.0]],
            ),
            (
                "dogleg, the far end moved: only its corner slides",
                vec![[0.0, 0.0], [5.0, 0.0], [5.0, 10.0], [20.0, 10.0]],
                [0.0, 0.0],
                [20.0, 15.0],
                vec![[0.0, 0.0], [5.0, 0.0], [5.0, 15.0], [20.0, 15.0]],
            ),
            (
                "the slid corner meets the next one: the empty segment is dropped",
                vec![[0.0, 0.0], [5.0, 0.0], [5.0, 10.0], [20.0, 10.0]],
                [0.0, 10.0],
                [20.0, 10.0],
                vec![[0.0, 10.0], [20.0, 10.0]],
            ),
            (
                "slanted first segment: a horizontal-first step",
                vec![[0.0, 0.0], [10.0, 5.0], [20.0, 5.0]],
                [0.0, 2.0],
                [20.0, 5.0],
                vec![[0.0, 2.0], [10.0, 2.0], [10.0, 5.0], [20.0, 5.0]],
            ),
            (
                "both ends moved alike: the path is carried along",
                vec![[0.0, 0.0], [0.0, 5.0], [10.0, 5.0], [10.0, 0.0]],
                [2.5, -2.5],
                [12.5, -2.5],
                vec![[2.5, -2.5], [2.5, 2.5], [12.5, 2.5], [12.5, -2.5]],
            ),
            (
                "both ends moved differently, one corner: each end moves it",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [0.0, 2.0],
                [15.0, 10.0],
                vec![[0.0, 2.0], [15.0, 2.0], [15.0, 10.0]],
            ),
            (
                "nothing moved",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [0.0, 0.0],
                [10.0, 10.0],
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
            ),
        ];
        for (case, path, from, to, expected) in cases {
            assert_eq!(follow(&path, from, to), expected, "{case}");
        }
    }
}
