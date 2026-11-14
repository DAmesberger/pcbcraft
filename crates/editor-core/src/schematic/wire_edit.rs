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
//! path, which the canvas previews and the `wire.*` commands store. They
//! take bare points or [`Bend`]s ([`Vertex`]): a corner keeps its radius
//! when an edit moves it, and a corner an edit adds is sharp. They compute
//! in millimetres; a stored [`Corner`] is whole nanometres, read as a
//! [`Bend`] and stored again through [`Bend::stored`].
use super::wiring::{self, JUNCTION_TOLERANCE_MM, WireTarget, end_position, simplified};
use super::{Error, MAX_CORNERS, Project, Result, invalid, not_found};
use crate::geometry::{self, Point};
use crate::ids::Id;
use crate::instances;
use crate::model::{Corner, FreeEnd, Wire, WireEnd};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A wire path vertex as wire geometry computes it, in millimetres: a
/// stored [`Corner`] read across the units boundary, or one an edit makes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bend {
    pub at: Point,
    /// Fillet radius; 0 is sharp.
    pub radius: f64,
}

impl Bend {
    pub const fn sharp(at: Point) -> Self {
        Self { at, radius: 0.0 }
    }

    pub fn of(corner: &Corner) -> Self {
        Self {
            at: corner.at_mm(),
            radius: corner.radius_mm(),
        }
    }

    /// The corner as the document stores it, in whole nanometres.
    pub fn stored(self) -> Result<Corner> {
        Ok(Corner {
            at: super::sheet_point(self.at)?,
            radius_nm: crate::units::Nm::from_mm_f64_rounded(self.radius)
                .map_err(|_| bad_radius())?,
        })
    }
}

/// A wire's stored corners as [`Bend`]s.
pub fn bends(corners: &[Corner]) -> Vec<Bend> {
    corners.iter().map(Bend::of).collect()
}

/// [`Bend`]s as stored corners.
pub fn stored(bends: &[Bend]) -> Result<Vec<Corner>> {
    bends.iter().map(|b| b.stored()).collect()
}

/// A path vertex: a bare point, or a [`Bend`] with its radius.
pub trait Vertex: Copy {
    fn at(&self) -> Point;
    /// A new vertex at `at`: a sharp corner.
    fn sharp(at: Point) -> Self;
    /// This vertex moved to `at`, keeping its radius.
    fn moved(self, at: Point) -> Self;
    fn bend(&self) -> Bend;
}

impl Vertex for Point {
    fn at(&self) -> Point {
        *self
    }
    fn sharp(at: Point) -> Self {
        at
    }
    fn moved(self, at: Point) -> Self {
        at
    }
    fn bend(&self) -> Bend {
        Bend::sharp(*self)
    }
}

impl Vertex for Bend {
    fn at(&self) -> Point {
        self.at
    }
    fn sharp(at: Point) -> Self {
        Bend::sharp(at)
    }
    fn moved(self, at: Point) -> Self {
        Bend { at, ..self }
    }
    fn bend(&self) -> Bend {
        *self
    }
}

fn points_of<V: Vertex>(path: &[V]) -> Vec<Point> {
    path.iter().map(Vertex::at).collect()
}

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

fn corners_of<V: Vertex>(path: &[V]) -> Vec<Bend> {
    path.get(1..path.len().saturating_sub(1))
        .unwrap_or_default()
        .iter()
        .map(Vertex::bend)
        .collect()
}

/// `path` without corners that add nothing; its ends stay.
fn cleaned<V: Vertex>(path: &[V]) -> Vec<V> {
    let (Some(&first), Some(&last)) = (path.first(), path.last()) else {
        return path.to_vec();
    };
    let interior = path
        .get(1..path.len().saturating_sub(1))
        .unwrap_or_default()
        .to_vec();
    let mut kept = vec![first];
    kept.extend(simplified(first.at(), interior, last.at(), Vertex::at));
    kept.push(last);
    kept
}

/// `rest`, the vertices from one neighbour of a vertex at `c` out to the
/// path's end, after that vertex moves by `d`: the segment from `c` keeps its
/// direction, so the neighbour takes the offset across it, together with the
/// run of vertices in line with it. An end that cannot move gets a new corner
/// before it, so the segment steps instead of slanting.
fn carried<V: Vertex>(c: Point, rest: &[V], end_moves: bool, d: Point) -> Vec<V> {
    let mut out = rest.to_vec();
    let Some(first) = rest.first() else {
        return out;
    };
    let along = axis(c, first.at());
    let shift = match along {
        Axis::Horizontal => [0.0, d[1]],
        Axis::Vertical => [d[0], 0.0],
        Axis::Other => return out,
    };
    for (i, vertex) in rest.iter().enumerate() {
        let here = vertex.at();
        let next = rest.get(i + 1).map(Vertex::at);
        let moved = [here[0] + shift[0], here[1] + shift[1]];
        if next.is_none() && !end_moves {
            out.insert(i, V::sharp(moved));
            break;
        }
        out[i] = vertex.moved(moved);
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
pub fn move_corner<V: Vertex>(
    path: &[V],
    movable: Movable,
    index: usize,
    to: Point,
    free: bool,
) -> Option<Vec<V>> {
    let v = index.checked_add(1)?;
    let (before, after) = (path.get(..v)?, path.get(v + 1..)?);
    let (corner, next) = (*path.get(v)?, after.first()?.at());
    let c = corner.at();
    if free {
        let mut out = path.to_vec();
        *out.get_mut(v)? = corner.moved(to);
        return Some(cleaned(&out));
    }
    let d = [to[0] - c[0], to[1] - c[1]];
    let reversed: Vec<V> = before.iter().rev().copied().collect();
    let previous = reversed.first()?.at();
    let mut out = if axis(previous, c) == axis(c, next) {
        let turn = match axis(previous, c) {
            Axis::Horizontal => [to[0], previous[1]],
            Axis::Vertical => [previous[0], to[1]],
            Axis::Other => previous,
        };
        let mut out = before.to_vec();
        out.push(V::sharp(turn));
        out
    } else {
        let mut out = carried(c, &reversed, movable[0], d);
        out.reverse();
        out
    };
    out.push(corner.moved(to));
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
pub fn move_segment<V: Vertex>(
    path: &[V],
    movable: Movable,
    index: usize,
    offset: f64,
) -> Option<Vec<V>> {
    let far = index.checked_add(1)?;
    let (a, b) = (*path.get(index)?, *path.get(far)?);
    let n = segment_normal(a.at(), b.at())?;
    let moved = |v: V| {
        let p = v.at();
        v.moved([p[0] + n[0] * offset, p[1] + n[1] * offset])
    };
    let along = axis(a.at(), b.at());
    let in_line = |q: &V, p: Point| along != Axis::Other && axis(q.at(), p) == along;
    let keeps = |neighbour: Option<&V>, end_moves: bool, p: Point| match neighbour {
        None => !end_moves,
        Some(q) => in_line(q, p),
    };
    let previous = index.checked_sub(1).and_then(|i| path.get(i));
    let mut out = path.get(..index)?.to_vec();
    if keeps(previous, movable[0], a.at()) {
        out.push(a);
    }
    out.push(moved(a));
    out.push(moved(b));
    if keeps(path.get(far + 1), movable[1], b.at()) {
        out.push(b);
    }
    out.extend_from_slice(path.get(far + 1..)?);
    Some(cleaned(&out))
}

/// `path` with a sharp corner at `at`, a point on it away from its vertices,
/// and the new corner's index; the shape is unchanged. `None` off the path or
/// on a vertex.
pub fn insert_corner<V: Vertex>(path: &[V], at: Point) -> Option<(Vec<V>, usize)> {
    let points = points_of(path);
    let (on, segment) = wiring::nearest_on_path(&points, at)?;
    if !same(on, at) || points.iter().any(|v| same(*v, on)) {
        return None;
    }
    let mut out = path.to_vec();
    out.insert(segment + 1, V::sharp(on));
    Some((out, segment))
}

/// `path` without corner `index`; `None` when there is no such corner.
pub fn remove_corner<V: Vertex>(path: &[V], index: usize) -> Option<Vec<V>> {
    let v = index.checked_add(1)?;
    if v + 1 >= path.len() {
        return None;
    }
    let mut out = path.to_vec();
    out.remove(v);
    Some(cleaned(&out))
}

/// `path` with corner `index` rounded by `radius` (0: sharp); `None` when
/// there is no such corner or the radius does not fit ([`radius_limit`]).
pub fn set_radius(path: &[Bend], index: usize, radius: f64) -> Option<Vec<Bend>> {
    let v = index.checked_add(1).filter(|v| v + 1 < path.len())?;
    if !(radius.is_finite() && radius >= 0.0)
        || radius > radius_limit(path, index)? + JUNCTION_TOLERANCE_MM
    {
        return None;
    }
    let mut out = path.to_vec();
    out.get_mut(v)?.radius = radius;
    Some(out)
}

/// `path` with each radius reduced to what is drawn: where neighbouring arcs
/// would overlap on a segment, both shrink in proportion. Rounded to the
/// micrometre, so a radius halved on a grid segment reads as a round number.
pub fn fitted(path: &[Bend]) -> Vec<Bend> {
    let points = points_of(path);
    let radii: Vec<f64> = path.iter().map(|c| c.radius).collect();
    let fit = geometry::fit_corner_radii(&points, &radii, false);
    path.iter()
        .enumerate()
        .map(|(i, c)| {
            let r = fit.get(i).copied().unwrap_or(0.0).min(c.radius);
            Bend {
                radius: ((r * 1000.0 + 1e-3).floor() / 1000.0).max(0.0),
                ..*c
            }
        })
        .collect()
}

/// The largest radius corner `index` of `path` takes: what fits its two
/// segments beside the arcs of its neighbours as drawn. 0 where its segments
/// run in line or fold back; `None` when there is no such corner.
pub fn radius_limit(path: &[Bend], index: usize) -> Option<f64> {
    let v = index.checked_add(1).filter(|v| v + 1 < path.len())?;
    let points = points_of(path);
    let radii: Vec<f64> = path.iter().map(|c| c.radius).collect();
    Some(geometry::corner_radius_limit(&points, &radii, v).min(wiring::MAX_CORNER_RADIUS_MM))
}

/// `path` with the end `side` at `to`, rubber-banded as when the object an
/// end is on moves ([`follow`]).
pub fn move_end<V: Vertex>(path: &[V], side: WireSide, to: Point) -> Vec<V> {
    let (Some(first), Some(last)) = (path.first(), path.last()) else {
        return path.to_vec();
    };
    match side {
        WireSide::From => follow(path, to, last.at()),
        WireSide::To => follow(path, first.at(), to),
    }
}

/// What is left when a segment goes: the path up to its first vertex, and
/// from its second, each `None` when it has no segment.
pub type Split<V> = (Option<Vec<V>>, Option<Vec<V>>);

/// The two paths left without segment `index`; `None` when there is no such
/// segment.
pub fn split_at<V: Vertex>(path: &[V], index: usize) -> Option<Split<V>> {
    let far = index.checked_add(1).filter(|i| *i < path.len())?;
    let piece = |p: &[V]| (p.len() >= 2).then(|| p.to_vec());
    Some((piece(path.get(..=index)?), piece(path.get(far..)?)))
}

/// A wire's drawn path, with its corners' radii, and which ends may move.
fn editable(project: &Project, id: Id) -> Result<(Vec<Bend>, Movable)> {
    let wire = project
        .wire(id)
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    let path = wiring::wire_vertices(project, wire)
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
fn store(project: &mut Project, id: Id, path: &[Bend]) -> Result<()> {
    let corners = stored(&corners_of(path))?;
    if corners.len() > MAX_CORNERS {
        return Err(Error::Limit("A wire has at most 256 corners.".into()));
    }
    let (Some(first), Some(last)) = (path.first().map(|v| v.at), path.last().map(|v| v.at)) else {
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
    invalid(super::POINT_RANGE)
}

fn no_corner() -> Error {
    invalid("This wire has no such corner.")
}

fn no_segment() -> Error {
    invalid("This wire has no such segment.")
}

pub(super) fn bad_radius() -> Error {
    invalid("A corner's radius must be from 0 to 1000 mm.")
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
    if !wiring::radii_valid(&corners) {
        return Err(bad_radius());
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
    super::sheet_point(to)?;
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
    super::sheet_point(at)?;
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

/// Round corner `index` by `radius`, or make it sharp with 0. Refused, not
/// reduced, when the radius does not fit its segments.
pub(super) fn apply_set_radius(
    project: &mut Project,
    id: Id,
    index: usize,
    radius: crate::units::Nm,
) -> Result<()> {
    if !wiring::radii_valid(&[Corner {
        radius_nm: radius,
        ..Corner::sharp(crate::units::PointNm::ORIGIN)
    }]) {
        return Err(bad_radius());
    }
    let radius = radius.to_mm_f64();
    let (path, _) = editable(project, id)?;
    let limit = radius_limit(&path, index).ok_or_else(no_corner)?;
    let path = set_radius(&path, index, radius).ok_or_else(|| {
        if limit <= JUNCTION_TOLERANCE_MM {
            invalid("This corner cannot be rounded: its segments run in line or fold back.")
        } else {
            invalid(&format!(
                "A {} mm radius does not fit this corner's segments; at most {} mm fits.",
                mm(radius),
                mm(limit)
            ))
        }
    })?;
    store(project, id, &path)
}

/// A length for a message: at most three decimals, without trailing zeros.
fn mm(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Remove a segment: the part before it keeps the wire's ID, and the part
/// after is a new wire, with free ends at the gap. Without a part before, the
/// part after keeps the ID; with neither, the wire goes.
pub(super) fn remove_segment(project: &mut Project, id: Id, index: usize) -> Result<()> {
    let (path, _) = editable(project, id)?;
    let (head, tail) = split_at(&path, index).ok_or_else(no_segment)?;
    let position = project
        .wires
        .iter()
        .position(|w| w.id == id)
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    let wire = project.wires.remove(position);
    let free = |p: Option<&Bend>| p.and_then(|p| WireEnd::free(p.at)).ok_or_else(off_sheet);
    let mut at = position;
    if let Some(head) = &head {
        let piece = Wire {
            to: free(head.last())?,
            corners: stored(&corners_of(head))?,
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
            corners: stored(&corners_of(tail))?,
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
pub fn follow_end<V: Vertex>(path: &[V], to: Point) -> Vec<V> {
    let mut out = path.to_vec();
    let (Some(&start), Some(&second)) = (path.first(), path.get(1)) else {
        return out;
    };
    out[0] = start.moved(to);
    let (first, next) = (start.at(), second.at());
    let first_axis = axis(first, next);
    // The next vertex is a corner: slide it along the second segment.
    if let Some(after) = path.get(2).map(Vertex::at) {
        match (first_axis, axis(next, after)) {
            (Axis::Horizontal, Axis::Vertical) => {
                out[1] = second.moved([next[0], to[1]]);
                return out;
            }
            (Axis::Vertical, Axis::Horizontal) => {
                out[1] = second.moved([to[0], next[1]]);
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
    out.insert(1, V::sharp(corner));
    out
}

/// How often `points` doubles back: a segment straight back along the one
/// before it, or one that runs opposite the segment two before it across a
/// single step (a U-turn).
fn reversals(points: &[Point]) -> usize {
    let d: Vec<Point> = points
        .windows(2)
        .map(|s| [s[1][0] - s[0][0], s[1][1] - s[0][1]])
        .collect();
    let opposite = |a: Point, b: Point| {
        let cross = a[0] * b[1] - a[1] * b[0];
        cross.abs() <= 1e-9 * (a[0].hypot(a[1]) * b[0].hypot(b[1])).max(1.0)
            && a[0] * b[0] + a[1] * b[1] < 0.0
    };
    let back = d.windows(2).filter(|w| opposite(w[0], w[1])).count();
    let u = d.windows(3).filter(|w| opposite(w[0], w[2])).count();
    back + u
}

/// A wire's drawn path `path` after its ends move to `from` and `to`,
/// without corners that add nothing. Both ends moved alike (a unit with a
/// wire between two of its own pins) carries the whole path along. A path
/// the move would make double back where it did not (a group turned past a
/// corner) is drawn again as one step instead, keeping the direction it
/// had at the end that stayed.
pub fn follow<V: Vertex>(path: &[V], from: Point, to: Point) -> Vec<V> {
    let out = followed_path(path, from, to);
    let (old, new) = (points_of(path), points_of(&out));
    if reversals(&new) <= reversals(&old) {
        return out;
    }
    // Which way the path ran at the end that stayed; horizontal first
    // when both moved.
    let along_first = |a: Point, b: Point| axis(a, b) != Axis::Vertical;
    let horizontal_first = match (old.first(), old.get(1), old.iter().rev().nth(1), old.last()) {
        (Some(&a), Some(&b), _, _) if same(a, from) => along_first(a, b),
        (_, _, Some(&a), Some(&b)) if same(b, to) => !along_first(a, b),
        _ => true,
    };
    let corner = if horizontal_first {
        [to[0], from[1]]
    } else {
        [from[0], to[1]]
    };
    let (Some(&first), Some(&last)) = (out.first(), out.last()) else {
        return out;
    };
    cleaned(&[first, V::sharp(corner), last])
}

fn followed_path<V: Vertex>(path: &[V], from: Point, to: Point) -> Vec<V> {
    let (Some(a), Some(b)) = (path.first().map(Vertex::at), path.last().map(Vertex::at)) else {
        return path.to_vec();
    };
    let (da, db) = (
        [from[0] - a[0], from[1] - a[1]],
        [to[0] - b[0], to[1] - b[1]],
    );
    let mut out = if same(da, db) {
        path.iter()
            .map(|v| {
                let p = v.at();
                v.moved([p[0] + da[0], p[1] + da[1]])
            })
            .collect()
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
        *first = first.moved(from);
    }
    if let Some(last) = out.last_mut() {
        *last = last.moved(to);
    }
    cleaned(&out)
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
/// are at `na` and `nb`; `None` past the corner limit or the sheet's range.
pub(super) fn followed(
    wire: &Wire,
    a: Point,
    b: Point,
    na: Point,
    nb: Point,
) -> Option<Vec<Corner>> {
    let mut path = Vec::with_capacity(wire.corners.len() + 2);
    path.push(Bend::sharp(a));
    path.extend(wire.corners.iter().map(Bend::of));
    path.push(Bend::sharp(b));
    let corners = corners_of(&follow(&path, na, nb));
    if corners.len() > MAX_CORNERS {
        return None;
    }
    stored(&corners).ok()
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
        slide_t(project, index);
    }
}

/// A followed wire that now leaves an unplaced junction back along another
/// wire there hides the T where the two really part. Its end there is
/// freed at that point, so [`wiring::tidy`] moves the T, and its dot, to
/// where the drawing shows it.
fn slide_t(project: &mut Project, index: usize) -> Option<()> {
    let wire = project.wires.get(index)?.clone();
    for (end, first) in [(wire.from, true), (wire.to, false)] {
        let Some(junction) = end.junction_id() else {
            continue;
        };
        if project.junction(junction).is_none_or(|j| j.placed) {
            continue;
        }
        let mut path = wiring::wire_path(project, &wire)?;
        if !first {
            path.reverse();
        }
        let (&s, &e) = (path.first()?, path.get(1)?);
        let length = geometry::distance(s, e);
        if length <= JUNCTION_TOLERANCE_MM {
            continue;
        }
        let dir = [(e[0] - s[0]) / length, (e[1] - s[1]) / length];
        // How far along its first segment the wire runs on another one.
        let reach = project
            .wires
            .iter()
            .filter(|w| w.id != wire.id && (w.from == end || w.to == end))
            .filter_map(|w| {
                let mut other = wiring::wire_path(project, w)?;
                if w.from != end {
                    other.reverse();
                }
                let q = *other.get(1)?;
                let along = (q[0] - s[0]) * dir[0] + (q[1] - s[1]) * dir[1];
                let off = (q[0] - s[0]) * dir[1] - (q[1] - s[1]) * dir[0];
                (along > 0.0 && off.abs() <= JUNCTION_TOLERANCE_MM).then_some(along.min(length))
            })
            .fold(0.0, f64::max);
        // Nothing shared, or the whole wire lies on the other.
        if reach <= JUNCTION_TOLERANCE_MM
            || (path.len() == 2 && reach >= length - JUNCTION_TOLERANCE_MM)
        {
            continue;
        }
        let at = [s[0] + dir[0] * reach, s[1] + dir[1] * reach];
        let free = WireEnd::Free(FreeEnd::new(at)?);
        let mut corners = wire.corners.clone();
        if !first {
            corners.reverse();
        }
        if reach >= length - JUNCTION_TOLERANCE_MM && !corners.is_empty() {
            corners.remove(0);
        }
        if !first {
            corners.reverse();
        }
        let wire = project.wires.get_mut(index)?;
        if first {
            wire.from = free;
        } else {
            wire.to = free;
        }
        wire.corners = corners;
        return Some(());
    }
    Some(())
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
            (
                "the end passes the corner it slides: one step, no U-turn, \
                 horizontal into the end that stayed",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [20.0, 10.0]],
                [15.0, 5.0],
                [20.0, 10.0],
                vec![[15.0, 5.0], [15.0, 10.0], [20.0, 10.0]],
            ),
            (
                "a U-turn the wire already had is kept",
                vec![[0.0, 0.0], [0.0, 5.0], [10.0, 5.0], [10.0, 0.0]],
                [0.0, -2.5],
                [10.0, 0.0],
                vec![[0.0, -2.5], [0.0, 5.0], [10.0, 5.0], [10.0, 0.0]],
            ),
        ];
        for (case, path, from, to, expected) in cases {
            assert_eq!(follow(&path, from, to), expected, "{case}");
        }
    }
}
