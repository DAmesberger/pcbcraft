//! Wire paths and wire-to-wire connectivity.
//!
//! A wire is a path: its first end, its corners, then its second end, each
//! pair of neighbouring vertices joined by a straight segment
//! ([`wire_path`]). Splitting a path at a point on it gives two paths that
//! draw exactly the original, so a wire end landing on a segment can split
//! that wire at a junction without moving any drawn line.
//!
//! Connectivity follows the schematic convention: wires join only where an
//! end lands on a terminal, a junction, a label, a sheet pin or another
//! wire's path (which makes a junction there). A plain crossing never joins,
//! and neither does a corner lying on another wire. The junction dot is
//! derived, never stored: [`junction_dots`] lists every point where three or
//! more conductors meet (a pin counts as one). [`tidy`] keeps the stored
//! topology in its normal form after every edit.
use super::{Error, MAX_CORNERS, Result, invalid, not_found, pin_position};
use crate::geometry::{Point, point_valid};
use crate::ids::{Endpoint, Id, SheetId};
use crate::instances;
use crate::model::{
    Corner, FreeEnd, Junction, JunctionRef, LabelRef, Net, Project, SheetPinRef, Wire, WireEnd,
};
use crate::nets;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_JUNCTIONS: usize = super::MAX_WIRES;

/// How far a point given as "on a wire" may lie from its path, and how
/// near a path end a point counts as that end, in millimetres.
pub const JUNCTION_TOLERANCE_MM: f64 = 1e-3;

/// A point on an existing wire's path.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WirePoint {
    pub wire: Id,
    pub at: Point,
}

/// Where a new wire end lands: a terminal (`{"instance", "terminal"}`), a
/// junction (`{"junction"}`), a label or power port (`{"label"}`), a sheet
/// pin (`{"sheet_pin"}`), a point on a wire (`{"wire", "at"}`), which
/// splits that wire at a new junction, or, for `wire.draw`, a free end
/// (`{"free": [x, y]}`). A point at a wire's end is that end.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum WireTarget {
    Terminal(Endpoint),
    Junction(JunctionRef),
    Label(LabelRef),
    SheetPin(SheetPinRef),
    OnWire(WirePoint),
    Free(FreeEnd),
}

impl From<Endpoint> for WireTarget {
    fn from(endpoint: Endpoint) -> Self {
        Self::Terminal(endpoint)
    }
}

impl From<WireEnd> for WireTarget {
    fn from(end: WireEnd) -> Self {
        match end {
            WireEnd::Terminal(endpoint) => Self::Terminal(endpoint),
            WireEnd::Junction(junction) => Self::Junction(junction),
            WireEnd::Label(label) => Self::Label(label),
            WireEnd::SheetPin(pin) => Self::SheetPin(pin),
            WireEnd::Free(free) => Self::Free(free),
        }
    }
}

/// Where a wire end sits on its sheet. `None` for a terminal whose unit is
/// not placed or a missing object.
pub fn end_position(project: &Project, end: WireEnd) -> Option<Point> {
    match end {
        WireEnd::Terminal(endpoint) => pin_position(project, endpoint),
        WireEnd::Junction(j) => project.junction(j.junction).map(|j| j.position),
        WireEnd::Label(l) => project
            .label(l.label)
            .map(|l| super::labels::anchor(project, l)),
        WireEnd::SheetPin(p) => project
            .sheet_pin(p.sheet_pin)
            .map(|(symbol, pin)| symbol.pin_position(pin)),
        WireEnd::Free(f) => Some(f.point()),
    }
}

/// The sheet the object a wire end is attached to lies on. `None` for a
/// free end, a terminal whose unit is not placed or a missing object.
pub fn end_sheet(project: &Project, end: WireEnd) -> Option<SheetId> {
    match end {
        WireEnd::Terminal(endpoint) => {
            let instance = project.instance(endpoint.instance)?;
            let symbol = &instances::resolve(&project.library, instance)
                .ok()?
                .symbol
                .definition;
            let unit = instances::owning_unit(symbol, endpoint.terminal)?;
            project
                .occurrences_of(instance.id)
                .find(|o| o.unit == unit)
                .map(|o| o.sheet)
        }
        WireEnd::Junction(j) => project.junction(j.junction).map(|j| j.sheet),
        WireEnd::Label(l) => project.label(l.label).map(|l| l.sheet),
        WireEnd::SheetPin(p) => project.sheet_pin(p.sheet_pin).map(|(s, _)| s.sheet),
        WireEnd::Free(_) => None,
    }
}

/// Both end positions of a wire.
pub fn wire_ends(project: &Project, wire: &Wire) -> Option<(Point, Point)> {
    end_position(project, wire.from).zip(end_position(project, wire.to))
}

/// Every vertex of a wire's path, from its first end through its corners to
/// its second end. `None` while an end is not drawn.
pub fn wire_path(project: &Project, wire: &Wire) -> Option<Vec<Point>> {
    let (a, b) = wire_ends(project, wire)?;
    let mut path = Vec::with_capacity(wire.corners.len() + 2);
    path.push(a);
    path.extend(wire.corners.iter().map(|c| c.at));
    path.push(b);
    Some(path)
}

fn nearest_on_segment(p: Point, a: Point, b: Point) -> Point {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    [a[0] + ab[0] * t, a[1] + ab[1] * t]
}

fn distance(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The point of a path nearest `p` and the index of its segment (from
/// vertex `index` to `index + 1`); the first segment wins a tie.
fn nearest_on_path(path: &[Point], p: Point) -> Option<(Point, usize)> {
    path.windows(2)
        .enumerate()
        .map(|(i, s)| (nearest_on_segment(p, s[0], s[1]), i))
        .min_by(|x, y| distance(p, x.0).total_cmp(&distance(p, y.0)))
}

/// The point of a wire's drawn path nearest `p`, for picking a point to
/// land a wire end on. `None` for an unknown or undrawn wire.
pub fn nearest_on_wire(project: &Project, wire: Id, p: Point) -> Option<Point> {
    let path = wire_path(project, project.wire(wire)?)?;
    nearest_on_path(&path, p).map(|(at, _)| at)
}

/// `corners` between `start` and `end` without corners that add nothing:
/// one on the vertex before it, or one in line between its neighbours.
pub(super) fn simplified(start: Point, corners: Vec<Corner>, end: Point) -> Vec<Corner> {
    let mut kept: Vec<Point> = Vec::with_capacity(corners.len());
    let previous = |kept: &Vec<Point>| kept.last().copied().unwrap_or(start);
    for corner in corners.iter().map(|c| c.at).chain([end]) {
        if distance(previous(&kept), corner) <= JUNCTION_TOLERANCE_MM {
            continue;
        }
        // The last kept corner lies in line between its neighbours.
        while let Some(&middle) = kept.last() {
            let before = kept
                .len()
                .checked_sub(2)
                .and_then(|i| kept.get(i))
                .copied()
                .unwrap_or(start);
            let u = [middle[0] - before[0], middle[1] - before[1]];
            let v = [corner[0] - middle[0], corner[1] - middle[1]];
            let onward = u[0] * v[0] + u[1] * v[1] > 0.0;
            if onward && segment_distance_mm(middle, before, corner) <= JUNCTION_TOLERANCE_MM {
                kept.pop();
            } else {
                break;
            }
        }
        kept.push(corner);
    }
    // The end was pushed last; it is not a corner.
    kept.pop();
    kept.into_iter().map(|at| Corner { at }).collect()
}

/// A wire's corners without those that add nothing, when its ends are
/// drawn.
pub(super) fn simplify(project: &Project, wire: &mut Wire) {
    if let Some((a, b)) = wire_ends(project, wire) {
        wire.corners = simplified(a, std::mem::take(&mut wire.corners), b);
    }
}

/// The wire end a target names, splitting a wire for a point on it.
pub(super) fn resolve(project: &mut Project, target: WireTarget) -> Result<WireEnd> {
    match target {
        WireTarget::Terminal(endpoint) => Ok(WireEnd::Terminal(endpoint)),
        WireTarget::Junction(j) => project
            .junction(j.junction)
            .map(|_| WireEnd::Junction(j))
            .ok_or_else(|| not_found("Unknown junction.")),
        WireTarget::Label(l) => project
            .label(l.label)
            .map(|_| WireEnd::Label(l))
            .ok_or_else(|| not_found("Unknown label.")),
        WireTarget::SheetPin(p) => project
            .sheet_pin(p.sheet_pin)
            .map(|_| WireEnd::SheetPin(p))
            .ok_or_else(|| not_found("Unknown sheet pin.")),
        WireTarget::OnWire(point) => split(project, point, None),
        WireTarget::Free(free) => Ok(WireEnd::Free(free)),
    }
}

/// Where a point on a wire lies on its path: the path point, the segment
/// it is on, and the wire end it is, if it is one.
pub(super) fn on_path(
    project: &Project,
    point: WirePoint,
) -> Result<(Point, usize, Option<WireEnd>)> {
    if !point_valid(point.at) {
        return Err(invalid(
            "Schematic position must be finite and within ±10000 mm.",
        ));
    }
    let wire = project
        .wire(point.wire)
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    let path =
        wire_path(project, wire).ok_or_else(|| invalid("This wire is not drawn on the sheet."))?;
    let (at, segment) =
        nearest_on_path(&path, point.at).ok_or_else(|| invalid("The point is not on the wire."))?;
    if distance(at, point.at) > JUNCTION_TOLERANCE_MM {
        return Err(invalid("The point is not on the wire."));
    }
    let (Some(a), Some(b)) = (path.first(), path.last()) else {
        return Err(invalid("This wire is not drawn on the sheet."));
    };
    if distance(at, *a) <= JUNCTION_TOLERANCE_MM {
        return Ok((*a, segment, Some(wire.from)));
    }
    if distance(at, *b) <= JUNCTION_TOLERANCE_MM {
        return Ok((*b, segment, Some(wire.to)));
    }
    Ok((at, segment, None))
}

/// A new junction point (not placed) on `sheet` at `position`.
fn new_junction(project: &mut Project, sheet: SheetId, position: Point) -> Result<Id> {
    if project.junctions.len() >= MAX_JUNCTIONS {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires and 16384 junctions.".into(),
        ));
    }
    let id = project.allocate()?;
    project.junctions.push(Junction {
        id,
        sheet,
        position,
        placed: false,
    });
    Ok(id)
}

/// Split a wire at a point on its path: the wire keeps its ID from its
/// first end to the split point, and a new wire runs from there to its
/// second end, so the drawn path is unchanged. The split point is `node`
/// (a label already at that point) or a new junction. A point at an end is
/// that end and splits nothing.
pub(super) fn split(
    project: &mut Project,
    point: WirePoint,
    node: Option<WireEnd>,
) -> Result<WireEnd> {
    let (at, segment, end) = on_path(project, point)?;
    if let Some(end) = end {
        return Ok(end);
    }
    let wire = project
        .wire(point.wire)
        .cloned()
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    if project.wires.len() >= super::MAX_WIRES {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires and 16384 junctions.".into(),
        ));
    }
    let end = match node {
        Some(node) => node,
        None => WireEnd::junction(new_junction(project, wire.sheet, at)?),
    };
    let tail = project.allocate()?;
    let index = project
        .wires
        .iter()
        .position(|w| w.id == wire.id)
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    // Path vertex k (0 < k < last) is corner k − 1; the split segment runs
    // from vertex `segment` to `segment + 1`.
    let cut = segment.min(wire.corners.len());
    let (head, rest) = wire.corners.split_at(cut);
    let mut first = Wire {
        to: end,
        corners: head.to_vec(),
        ..wire.clone()
    };
    let mut second = Wire {
        id: tail,
        sheet: wire.sheet,
        from: end,
        to: wire.to,
        corners: rest.to_vec(),
    };
    simplify(project, &mut first);
    simplify(project, &mut second);
    project.wires[index] = first;
    project.wires.insert(index + 1, second);
    Ok(end)
}

/// Every connection point drawn on a sheet: placed pin ends, junctions,
/// labels and sheet pins, with their sheet and position. Free wire ends are
/// not connection points.
fn connection_points(project: &Project) -> Vec<(SheetId, Point, WireEnd)> {
    let instances: BTreeMap<Id, &crate::model::ComponentInstance> =
        project.instances.iter().map(|i| (i.id, i)).collect();
    let mut points = Vec::new();
    for occurrence in &project.occurrences {
        let Some(instance) = instances.get(&occurrence.instance) else {
            continue;
        };
        let Ok(resolved) = instances::resolve(&project.library, instance) else {
            continue;
        };
        let symbol = &resolved.symbol.definition;
        let Some(unit) = symbol.unit(occurrence.unit) else {
            continue;
        };
        let transform = super::occurrence_transform(occurrence);
        for pin in &unit.pins {
            // The same owner `pin_position` uses.
            if instances::owning_unit(symbol, pin.terminal) != Some(unit.id) {
                continue;
            }
            let end = WireEnd::Terminal(Endpoint {
                instance: instance.id,
                terminal: pin.terminal,
            });
            points.push((occurrence.sheet, transform.apply(pin.position), end));
        }
    }
    points.extend(
        project
            .junctions
            .iter()
            .map(|j| (j.sheet, j.position, WireEnd::junction(j.id))),
    );
    points.extend(project.labels.iter().map(|l| {
        (
            l.sheet,
            super::labels::anchor(project, l),
            WireEnd::label(l.id),
        )
    }));
    for symbol in &project.sheet_symbols {
        for pin in &symbol.pins {
            points.push((
                symbol.sheet,
                symbol.pin_position(pin),
                WireEnd::sheet_pin(pin.id),
            ));
        }
    }
    points.retain(|(_, p, _)| point_valid(*p));
    points
}

/// A coordinate in units of [`JUNCTION_TOLERANCE_MM`], for bucketing.
/// Callers pass valid sheet coordinates (within ±10000 mm), so it fits.
fn bucket(v: f64) -> i64 {
    (v / JUNCTION_TOLERANCE_MM).round() as i64
}

/// Connection points along horizontal and vertical lines: by (sheet,
/// bucketed y) with x, and by (sheet, bucketed x) with y, each sorted along
/// the line; and every point by sheet, for slanted segments.
struct LineIndex {
    rows: BTreeMap<(SheetId, i64), Vec<(f64, usize)>>,
    columns: BTreeMap<(SheetId, i64), Vec<(f64, usize)>>,
    sheets: BTreeMap<SheetId, Vec<usize>>,
}

impl LineIndex {
    fn new(points: &[(SheetId, Point, WireEnd)]) -> Self {
        let mut index = Self {
            rows: BTreeMap::new(),
            columns: BTreeMap::new(),
            sheets: BTreeMap::new(),
        };
        for (i, (sheet, p, _)) in points.iter().enumerate() {
            index
                .rows
                .entry((*sheet, bucket(p[1])))
                .or_default()
                .push((p[0], i));
            index
                .columns
                .entry((*sheet, bucket(p[0])))
                .or_default()
                .push((p[1], i));
            index.sheets.entry(*sheet).or_default().push(i);
        }
        for list in index.rows.values_mut().chain(index.columns.values_mut()) {
            list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        }
        index
    }
    /// Points within tolerance of the segment from `a` to `b` on `sheet`.
    fn on_segment(
        &self,
        points: &[(SheetId, Point, WireEnd)],
        sheet: SheetId,
        a: Point,
        b: Point,
        out: &mut Vec<usize>,
    ) {
        let near = |i: &usize| {
            points
                .get(*i)
                .is_some_and(|(_, p, _)| segment_distance_mm(*p, a, b) <= JUNCTION_TOLERANCE_MM)
        };
        let horizontal = (a[1] - b[1]).abs() <= JUNCTION_TOLERANCE_MM;
        let vertical = (a[0] - b[0]).abs() <= JUNCTION_TOLERANCE_MM;
        if !horizontal && !vertical {
            // A slanted segment: every point on the sheet within its box.
            let (lo, hi) = (
                [a[0].min(b[0]), a[1].min(b[1])],
                [a[0].max(b[0]), a[1].max(b[1])],
            );
            let within = |i: &usize| {
                points.get(*i).is_some_and(|(_, p, _)| {
                    (lo[0] - JUNCTION_TOLERANCE_MM..=hi[0] + JUNCTION_TOLERANCE_MM).contains(&p[0])
                        && (lo[1] - JUNCTION_TOLERANCE_MM..=hi[1] + JUNCTION_TOLERANCE_MM)
                            .contains(&p[1])
                })
            };
            if let Some(list) = self.sheets.get(&sheet) {
                out.extend(list.iter().filter(|i| within(i) && near(i)));
            }
            return;
        }
        let (lines, across, along) = if horizontal {
            (&self.rows, a[1], [a[0], b[0]])
        } else {
            (&self.columns, a[0], [a[1], b[1]])
        };
        let (lo, hi) = (
            along[0].min(along[1]) - JUNCTION_TOLERANCE_MM,
            along[0].max(along[1]) + JUNCTION_TOLERANCE_MM,
        );
        let key = bucket(across);
        for k in key.saturating_sub(1)..=key.saturating_add(1) {
            let Some(list) = lines.get(&(sheet, k)) else {
                continue;
            };
            let start = list.partition_point(|(v, _)| *v < lo);
            for (v, i) in &list[start..] {
                if *v > hi {
                    break;
                }
                if near(i) {
                    out.push(*i);
                }
            }
        }
    }
}

fn segment_distance_mm(p: Point, a: Point, b: Point) -> f64 {
    distance(p, nearest_on_segment(p, a, b))
}

/// The step between dogleg candidates, and how many steps a dogleg may
/// reach beyond either end.
const DOGLEG_STEP_MM: f64 = crate::library::graphics::SYMBOL_MODULE_MM;
const DOGLEG_STEPS: i32 = 12;

/// A wire end as a person names it: `U1 pin 3 (GND)`, `label VBUS`.
pub(super) fn describe(project: &Project, end: WireEnd) -> String {
    match end {
        WireEnd::Terminal(endpoint) => {
            let Some(instance) = project.instance(endpoint.instance) else {
                return "a pin".into();
            };
            let name = project
                .library
                .component(instance.component)
                .and_then(|c| c.definition.terminal(endpoint.terminal))
                .map(|t| t.name.clone())
                .unwrap_or_default();
            format!("{} pin {name}", instance.designator)
        }
        WireEnd::Junction(_) => "a junction point".into(),
        WireEnd::Label(l) => project
            .label(l.label)
            .map_or_else(|| "a label".into(), |l| format!("label {}", l.name)),
        WireEnd::SheetPin(p) => project.sheet_pin(p.sheet_pin).map_or_else(
            || "a sheet pin".into(),
            |(_, p)| format!("sheet pin {}", p.name),
        ),
        WireEnd::Free(_) => "a free wire end".into(),
    }
}

/// The path of a new wire from `a` to `b` (its `ends`): the horizontal-first
/// L when it runs over no other connection point; else the vertical-first L
/// or the shortest clean dogleg, with no corner on another wire. Refused,
/// naming the point in the way, when no such path is clean.
pub(super) fn clean_route(
    project: &Project,
    sheet: SheetId,
    a: Point,
    b: Point,
    ends: [WireEnd; 2],
) -> Result<Vec<Point>> {
    let points: Vec<_> = connection_points(project)
        .into_iter()
        .filter(|(s, _, e)| *s == sheet && !ends.contains(e))
        .collect();
    let index = LineIndex::new(&points);
    // The first point on the path other than at its ends.
    let in_way = |path: &[Point]| {
        let mut hits = Vec::new();
        for leg in path.windows(2) {
            index.on_segment(&points, sheet, leg[0], leg[1], &mut hits);
        }
        hits.iter()
            .filter_map(|i| points.get(*i))
            .find(|(_, p, _)| {
                distance(*p, a) > JUNCTION_TOLERANCE_MM && distance(*p, b) > JUNCTION_TOLERANCE_MM
            })
            .map(|(_, _, end)| *end)
    };
    let first = vec![a, [b[0], a[1]], b];
    let Some(blocker) = in_way(&first) else {
        return Ok(first);
    };
    // A corner on a wire would read as a T without joining it.
    let paths: Vec<Vec<Point>> = project
        .wires
        .iter()
        .filter(|w| w.sheet == sheet)
        .filter_map(|w| wire_path(project, w))
        .collect();
    let on_wire = |corner: Point| {
        paths.iter().any(|path| {
            nearest_on_path(path, corner)
                .is_some_and(|(p, _)| distance(p, corner) <= JUNCTION_TOLERANCE_MM)
        })
    };
    // The vertical-first L, then three-legged doglegs whose middle leg runs
    // up to 30 mm beyond either end, shortest first.
    let mut candidates = vec![vec![a, [a[0], b[1]], b]];
    let mut doglegs = Vec::new();
    for k in 1..=DOGLEG_STEPS {
        let d = f64::from(k) * DOGLEG_STEP_MM;
        for x in [a[0] + d, a[0] - d, b[0] + d, b[0] - d] {
            doglegs.push(vec![a, [x, a[1]], [x, b[1]], b]);
        }
        for y in [a[1] + d, a[1] - d, b[1] + d, b[1] - d] {
            doglegs.push(vec![a, [a[0], y], [b[0], y], b]);
        }
    }
    let length = |p: &Vec<Point>| -> f64 { p.windows(2).map(|s| distance(s[0], s[1])).sum() };
    doglegs.sort_by(|x, y| length(x).total_cmp(&length(y)));
    candidates.extend(doglegs);
    for mut path in candidates {
        // Drop corners that coincide with an end or another corner.
        path.dedup_by(|p, q| distance(*p, *q) <= JUNCTION_TOLERANCE_MM);
        let corners = path
            .get(1..path.len().saturating_sub(1))
            .unwrap_or_default();
        if path.len() > 2
            && path.iter().all(|p| point_valid(*p))
            && in_way(&path).is_none()
            && !corners.iter().any(|p| on_wire(*p))
        {
            return Ok(path);
        }
    }
    Err(super::conflict(&format!(
        "This wire from {} to {} would run over {}, which would connect to it. Move a part, or join them with net labels.",
        describe(project, ends[0]),
        describe(project, ends[1]),
        describe(project, blocker),
    )))
}

/// Join every connection point that lies on a wire's drawn path to that
/// wire, as the schematic convention reads a drawing: a point on a
/// segment or corner splits the wire there into pieces that draw exactly
/// the original path (a T, whose point then carries three conductors), and
/// a point on a wire's end that is not that end is joined to it by a
/// zero-length wire. Crossing wires have no point at the crossing, so they
/// stay apart. Idempotent: afterwards no point lies inside any wire's path.
fn join_touching(project: &mut Project) -> Result<()> {
    /// Points inside a path, with their distance along it.
    type Stops = Vec<(f64, WireEnd)>;
    if project.wires.is_empty() {
        return Ok(());
    }
    let points = connection_points(project);
    let index = LineIndex::new(&points);
    // Per wire: its path, and the points inside it with their distance
    // along it; the points on either end.
    let mut splits: BTreeMap<Id, (Vec<Point>, Stops)> = BTreeMap::new();
    let mut joins: Vec<(SheetId, WireEnd, WireEnd)> = Vec::new();
    let mut hits = Vec::new();
    for wire in &project.wires {
        let Some(path) = wire_path(project, wire) else {
            continue;
        };
        let (Some(&a), Some(&b)) = (path.first(), path.last()) else {
            continue;
        };
        let mut inside: BTreeMap<WireEnd, f64> = BTreeMap::new();
        let mut walked = 0.0;
        for leg in path.windows(2) {
            hits.clear();
            index.on_segment(&points, wire.sheet, leg[0], leg[1], &mut hits);
            for i in &hits {
                let Some((_, p, end)) = points.get(*i) else {
                    continue;
                };
                if *end == wire.from || *end == wire.to {
                    continue;
                }
                if distance(*p, a) <= JUNCTION_TOLERANCE_MM {
                    joins.push((wire.sheet, wire.from, *end));
                } else if distance(*p, b) <= JUNCTION_TOLERANCE_MM {
                    joins.push((wire.sheet, wire.to, *end));
                } else {
                    // The first time along the path it is passed.
                    inside.entry(*end).or_insert(walked + distance(leg[0], *p));
                }
            }
            walked += distance(leg[0], leg[1]);
        }
        if !inside.is_empty() {
            let mut order: Vec<(f64, WireEnd)> = inside.into_iter().map(|(e, s)| (s, e)).collect();
            order.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            splits.insert(wire.id, (path, order));
        }
    }
    joins.retain(|(_, end, point)| {
        !project
            .wires
            .iter()
            .any(|w| (w.from == *end && w.to == *point) || (w.from == *point && w.to == *end))
    });
    if splits.is_empty() && joins.is_empty() {
        return Ok(());
    }
    let positions: BTreeMap<WireEnd, Point> = points.iter().map(|(_, p, e)| (*e, *p)).collect();
    let old = std::mem::take(&mut project.wires);
    let mut wires = Vec::with_capacity(old.len());
    for wire in old {
        let Some((path, through)) = splits.get(&wire.id) else {
            wires.push(wire);
            continue;
        };
        // Each vertex with its distance along the path.
        let mut along = Vec::with_capacity(path.len());
        let mut walked = 0.0;
        for (i, p) in path.iter().enumerate() {
            if i > 0 {
                walked += distance(path[i - 1], *p);
            }
            along.push(walked);
        }
        let corners: Vec<(f64, Point)> = along
            .iter()
            .copied()
            .zip(path.iter().copied())
            .skip(1)
            .take(path.len().saturating_sub(2))
            .collect();
        let mut stops = Vec::with_capacity(through.len() + 2);
        stops.push((0.0, wire.from));
        stops.extend(through.iter().copied());
        stops.push((walked, wire.to));
        let mut first = true;
        for leg in stops.windows(2) {
            let ((s0, from), (s1, to)) = (leg[0], leg[1]);
            let between: Vec<Corner> = corners
                .iter()
                .filter(|(s, _)| *s > s0 && *s < s1)
                .map(|(_, at)| Corner { at: *at })
                .collect();
            let start = positions
                .get(&from)
                .copied()
                .or_else(|| path.first().copied());
            let end = positions.get(&to).copied().or_else(|| path.last().copied());
            let corners = match (start, end) {
                (Some(p), Some(q)) => simplified(p, between, q),
                _ => between,
            };
            // A piece from a point back to itself with nothing between
            // draws nothing.
            if from == to && corners.is_empty() {
                continue;
            }
            let id = if first { wire.id } else { project.allocate()? };
            first = false;
            wires.push(Wire {
                id,
                sheet: wire.sheet,
                from,
                to,
                corners,
            });
        }
    }
    // A point on a wire's end joins that end with a wire of no length.
    let mut taken = BTreeSet::new();
    for (sheet, end, point) in joins {
        let pair = if end < point {
            (end, point)
        } else {
            (point, end)
        };
        if taken.insert(pair) {
            let id = project.allocate()?;
            wires.push(Wire {
                id,
                sheet,
                from: end,
                to: point,
                corners: Vec::new(),
            });
        }
    }
    if wires.len() > super::MAX_WIRES {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires and 16384 junctions.".into(),
        ));
    }
    project.wires = wires;
    Ok(())
}

/// A free end on a connection point attaches to it; one on another wire's
/// path, away from that wire's ends, attaches to a new junction there,
/// which [`join_touching`] then splits that wire at (a T).
fn attach_free_ends(project: &mut Project) -> Result<()> {
    let free = |w: &Wire| matches!(w.from, WireEnd::Free(_)) || matches!(w.to, WireEnd::Free(_));
    if !project.wires.iter().any(free) {
        return Ok(());
    }
    let points = connection_points(project);
    let mut at_point: BTreeMap<(SheetId, i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, (sheet, p, _)) in points.iter().enumerate() {
        at_point
            .entry((*sheet, bucket(p[0]), bucket(p[1])))
            .or_default()
            .push(i);
    }
    let point_at = |sheet: SheetId, p: Point| {
        let (x, y) = (bucket(p[0]), bucket(p[1]));
        (x - 1..=x + 1)
            .flat_map(|i| (y - 1..=y + 1).map(move |j| (i, j)))
            .filter_map(|(i, j)| at_point.get(&(sheet, i, j)))
            .flatten()
            .filter_map(|i| points.get(*i))
            .filter(|(_, q, _)| distance(*q, p) <= JUNCTION_TOLERANCE_MM)
            .map(|(_, _, end)| *end)
            .next()
    };
    let paths: Vec<Option<Vec<Point>>> = project
        .wires
        .iter()
        .map(|w| wire_path(project, w))
        .collect();
    // Junctions made here, so a second end at the same point shares one.
    let mut made: Vec<(SheetId, Point, WireEnd)> = Vec::new();
    for i in 0..project.wires.len() {
        for side in 0..2 {
            let (sheet, end) = match project.wires.get(i) {
                Some(w) => (w.sheet, if side == 0 { w.from } else { w.to }),
                None => continue,
            };
            let Some(p) = end.free_point() else {
                continue;
            };
            let found = point_at(sheet, p).or_else(|| {
                made.iter()
                    .find(|(s, q, _)| *s == sheet && distance(*q, p) <= JUNCTION_TOLERANCE_MM)
                    .map(|(_, _, end)| *end)
            });
            let found = match found {
                Some(end) => Some(end),
                None => {
                    let on = paths.iter().enumerate().find_map(|(k, path)| {
                        let path = path.as_ref()?;
                        if k == i || project.wires.get(k)?.sheet != sheet {
                            return None;
                        }
                        let (q, _) = nearest_on_path(path, p)?;
                        let inside = distance(q, p) <= JUNCTION_TOLERANCE_MM
                            && path.first().zip(path.last()).is_some_and(|(a, b)| {
                                distance(*a, q) > JUNCTION_TOLERANCE_MM
                                    && distance(*b, q) > JUNCTION_TOLERANCE_MM
                            });
                        inside.then_some(q)
                    });
                    match on {
                        Some(q) => {
                            let junction = WireEnd::junction(new_junction(project, sheet, q)?);
                            made.push((sheet, q, junction));
                            Some(junction)
                        }
                        None => None,
                    }
                }
            };
            if let (Some(to), Some(w)) = (found, project.wires.get_mut(i)) {
                if side == 0 {
                    w.from = to;
                } else {
                    w.to = to;
                }
            }
        }
    }
    Ok(())
}

/// Join wires `a` and `b` where both end at `at` (drawn at `point`): one
/// path from `a`'s other end through `point`, now a corner, to `b`'s other
/// end, under `a`'s ID. Nothing for the same wire or a wire that does not
/// end there.
fn merge(project: &mut Project, a: Id, b: Id, at: WireEnd, point: Point) {
    if a == b {
        return;
    }
    let (Some(ia), Some(ib)) = (
        project.wires.iter().position(|w| w.id == a),
        project.wires.iter().position(|w| w.id == b),
    ) else {
        return;
    };
    let (wa, wb) = (&project.wires[ia], &project.wires[ib]);
    let reversed = |w: &Wire| w.corners.iter().rev().copied().collect::<Vec<_>>();
    // `a` oriented to end at `at`, `b` to start there.
    let (start, mut corners) = if wa.to == at {
        (wa.from, wa.corners.clone())
    } else if wa.from == at {
        (wa.to, reversed(wa))
    } else {
        return;
    };
    let (end, rest) = if wb.from == at {
        (wb.to, wb.corners.clone())
    } else if wb.to == at {
        (wb.from, reversed(wb))
    } else {
        return;
    };
    corners.push(Corner { at: point });
    corners.extend(rest);
    let mut merged = Wire {
        id: a,
        sheet: wa.sheet,
        from: start,
        to: end,
        corners,
    };
    simplify(project, &mut merged);
    project.wires[ia] = merged;
    project.wires.remove(ib);
}

/// The wire a merged-away wire now lives in.
fn survivor(alias: &BTreeMap<Id, Id>, mut id: Id) -> Id {
    while let Some(next) = alias.get(&id) {
        id = *next;
    }
    id
}

/// Set every end of wire `id` that is `from` to `to`.
fn reattach(project: &mut Project, id: Id, from: WireEnd, to: WireEnd) {
    if let Some(w) = project.wires.iter_mut().find(|w| w.id == id) {
        if w.from == from {
            w.from = to;
        }
        if w.to == from {
            w.to = to;
        }
    }
}

/// Free ends of different wires at one point are connected: two join into
/// one path; three or more meet at a new junction.
fn join_free_ends(project: &mut Project) -> Result<()> {
    let mut groups: BTreeMap<(SheetId, FreeEnd), Vec<Id>> = BTreeMap::new();
    for wire in &project.wires {
        for end in [wire.from, wire.to] {
            if let WireEnd::Free(f) = end {
                groups.entry((wire.sheet, f)).or_default().push(wire.id);
            }
        }
    }
    let mut alias = BTreeMap::new();
    for ((sheet, free), wires) in groups {
        let end = WireEnd::Free(free);
        match wires[..] {
            [a, b] => {
                let (a, b) = (survivor(&alias, a), survivor(&alias, b));
                if a != b {
                    merge(project, a, b, end, free.point());
                    alias.insert(b, a);
                }
            }
            [_, _, _, ..] => {
                let junction = WireEnd::junction(new_junction(project, sheet, free.point())?);
                for id in wires {
                    reattach(project, survivor(&alias, id), end, junction);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Junction points by their wire ends: an unplaced one with two ends
/// becomes a corner of the one path the two wires merge into; one left
/// with a single end leaves a free end there; one with none goes. A placed
/// junction stays while any wire ends on it.
fn settle_junctions(project: &mut Project) -> Result<()> {
    let mut on: BTreeMap<Id, Vec<Id>> = project
        .junctions
        .iter()
        .map(|j| (j.id, Vec::new()))
        .collect();
    for wire in &project.wires {
        for end in [wire.from, wire.to] {
            if let Some(list) = end.junction_id().and_then(|id| on.get_mut(&id)) {
                list.push(wire.id);
            }
        }
    }
    let mut gone = BTreeSet::new();
    let mut alias = BTreeMap::new();
    for (id, wires) in on {
        let Some((position, placed)) = project.junction(id).map(|j| (j.position, j.placed)) else {
            continue;
        };
        let at = WireEnd::junction(id);
        let ids: Vec<Id> = wires.iter().map(|w| survivor(&alias, *w)).collect();
        match ids[..] {
            [] => {
                gone.insert(id);
            }
            // One end, or both ends of one wire: free there.
            [a] | [a, _] if !placed && ids.iter().all(|w| *w == a) => {
                let free = WireEnd::free(position)
                    .ok_or_else(|| invalid("A junction position is out of range."))?;
                reattach(project, a, at, free);
                gone.insert(id);
                // A wire of no length (a label's on its pin) then draws
                // and joins nothing: it goes.
                let empty = project
                    .wire(a)
                    .and_then(|w| wire_path(project, w))
                    .is_some_and(|p| {
                        p.windows(2)
                            .all(|s| distance(s[0], s[1]) <= JUNCTION_TOLERANCE_MM)
                    });
                if empty {
                    project.wires.retain(|w| w.id != a);
                }
            }
            [a, b] if !placed => {
                merge(project, a, b, at, position);
                alias.insert(b, a);
                gone.insert(id);
            }
            _ => {}
        }
    }
    if !gone.is_empty() {
        project.junctions.retain(|j| !gone.contains(&j.id));
    }
    Ok(())
}

/// Bring the wiring to its normal form after an edit: free ends on a
/// connection point or another wire attach there ([`attach_free_ends`]),
/// connection points lying on a wire join it ([`join_touching`]), free ends
/// that meet join ([`join_free_ends`]), and junctions settle
/// ([`settle_junctions`]): an unplaced one with two wires becomes a corner,
/// one with a single wire leaves a free end, which ERC reports.
pub fn tidy(project: &mut Project) -> Result<()> {
    attach_free_ends(project)?;
    join_touching(project)?;
    join_free_ends(project)?;
    settle_junctions(project)
}

/// The axis direction (unit steps in x and y) in which `wire` leaves its
/// end `at`, along its drawn path. `None` for a wire of no length or an
/// end that is not `at`.
fn leaving(project: &Project, wire: &Wire, at: WireEnd) -> Option<[i8; 2]> {
    let mut path = wire_path(project, wire)?;
    if wire.to == at {
        path.reverse();
    } else if wire.from != at {
        return None;
    }
    let start = *path.first()?;
    let q = path
        .into_iter()
        .find(|q| distance(start, *q) > JUNCTION_TOLERANCE_MM)?;
    let sign = |v: f64| {
        if v > JUNCTION_TOLERANCE_MM {
            1
        } else if v < -JUNCTION_TOLERANCE_MM {
            -1
        } else {
            0
        }
    };
    Some([sign(q[0] - start[0]), sign(q[1] - start[1])])
}

/// Where three or more conductors meet: a junction with three or more
/// wires, or a terminal with two or more (the pin is the third), and every
/// junction point put there with `junction.place`. Each is drawn as a
/// solid dot; a terminal end of a single wire and a free end get none.
/// Terminals whose unit is not placed are skipped.
pub fn junction_dots(project: &Project) -> Vec<(WireEnd, Point)> {
    let mut degree: BTreeMap<WireEnd, usize> = BTreeMap::new();
    for wire in &project.wires {
        for end in [wire.from, wire.to] {
            *degree.entry(end).or_default() += 1;
        }
    }
    degree
        .into_iter()
        .filter(|(end, wires)| match end {
            WireEnd::Terminal(_) => *wires >= 2,
            WireEnd::Junction(j) => {
                *wires >= 3 || project.junction(j.junction).is_some_and(|j| j.placed)
            }
            WireEnd::Free(_) => false,
            _ => *wires >= 3,
        })
        .filter_map(|(end, _)| Some((end, end_position(project, end)?)))
        .collect()
}

/// Put a junction point on the wires passing `position` on `sheet`
/// (`junction.place`); the engine's [`tidy`] then splits every wire there,
/// joining them. Refused off every wire and on an existing connection
/// point.
pub(super) fn place_junction(project: &mut Project, sheet: SheetId, position: Point) -> Result<Id> {
    if !point_valid(position) {
        return Err(invalid(
            "Schematic position must be finite and within ±10000 mm.",
        ));
    }
    if project.sheet(sheet).is_none() {
        return Err(not_found("Unknown sheet."));
    }
    if let Some((_, _, end)) = connection_points(project)
        .into_iter()
        .find(|(s, p, _)| *s == sheet && distance(*p, position) <= JUNCTION_TOLERANCE_MM)
    {
        return Err(super::conflict(&format!(
            "{} is already a connection point here.",
            describe(project, end)
        )));
    }
    let on = project
        .wires
        .iter()
        .filter(|w| w.sheet == sheet)
        .filter_map(|w| wire_path(project, w))
        .filter_map(|path| nearest_on_path(&path, position).map(|(p, _)| p))
        .find(|p| distance(*p, position) <= JUNCTION_TOLERANCE_MM)
        .ok_or_else(|| invalid("A junction point goes on a wire."))?;
    let id = new_junction(project, sheet, on)?;
    if let Some(junction) = project.junctions.iter_mut().find(|j| j.id == id) {
        junction.placed = true;
    }
    Ok(id)
}

/// Remove a junction point (`junction.remove`). With one or two wire ends
/// it stops being placed, so [`tidy`] merges the two wires into one path
/// (or frees the one end). Where more pass through it, each pair leaving
/// it in opposite directions becomes one path, so a crossing stops
/// joining; a T is refused.
pub(super) fn remove_junction(project: &mut Project, id: Id) -> Result<()> {
    let at = WireEnd::junction(id);
    let position = project
        .junction(id)
        .map(|j| j.position)
        .ok_or_else(|| not_found("Unknown junction."))?;
    let wires: Vec<Wire> = project.wires_at(at).cloned().collect();
    if wires.len() <= 2 {
        if let Some(junction) = project.junctions.iter_mut().find(|j| j.id == id) {
            junction.placed = false;
        }
        return Ok(());
    }
    let refused = || {
        super::conflict("This junction point is a T; remove a wire to change the drawing there.")
    };
    let mut legs = Vec::with_capacity(wires.len());
    for wire in &wires {
        legs.push((leaving(project, wire, at).ok_or_else(refused)?, wire.id));
    }
    let mut pairs = Vec::new();
    let mut used = vec![false; legs.len()];
    for i in 0..legs.len() {
        if used[i] {
            continue;
        }
        let (d, a) = legs[i];
        let partner = (0..legs.len())
            .find(|j| !used[*j] && *j != i && legs[*j].0 == [-d[0], -d[1]])
            .ok_or_else(refused)?;
        used[i] = true;
        used[partner] = true;
        pairs.push((a, legs[partner].1));
    }
    for (a, b) in pairs {
        merge(project, a, b, at, position);
    }
    project.junctions.retain(|j| j.id != id);
    Ok(())
}

/// Wire and junction records: limits, references, sheets, corners, no wire
/// from a point back to itself without corners, and every junction carrying
/// a wire end (an unplaced one three or more).
pub(super) fn validate(project: &Project) -> Result<()> {
    if project.junctions.len() > MAX_JUNCTIONS {
        return Err(invalid("Schematic exceeds supported item limits."));
    }
    for junction in &project.junctions {
        if !point_valid(junction.position) {
            return Err(invalid("A junction position is out of range."));
        }
    }
    let mut ends: BTreeMap<Id, usize> = BTreeMap::new();
    for wire in &project.wires {
        if wire.from == wire.to && wire.corners.is_empty() {
            return Err(invalid("A wire must connect two different points."));
        }
        if project.sheet(wire.sheet).is_none() {
            return Err(invalid("A wire is drawn on a missing sheet."));
        }
        if wire.corners.len() > MAX_CORNERS {
            return Err(invalid("A wire has more than 256 corners."));
        }
        if !wire.corners.iter().all(|c| point_valid(c.at)) {
            return Err(invalid("A wire corner is out of range."));
        }
        for end in [wire.from, wire.to] {
            let known = match end {
                WireEnd::Terminal(endpoint) => project.instance(endpoint.instance).is_some(),
                WireEnd::Junction(j) => {
                    *ends.entry(j.junction).or_default() += 1;
                    project.junction(j.junction).is_some()
                }
                WireEnd::Label(l) => project.label(l.label).is_some(),
                WireEnd::SheetPin(p) => project.sheet_pin(p.sheet_pin).is_some(),
                WireEnd::Free(_) => true,
            };
            if !known {
                return Err(invalid("A wire references a missing connection point."));
            }
            if end_sheet(project, end).is_some_and(|s| s != wire.sheet) {
                return Err(invalid("A wire must join two points on its own sheet."));
            }
        }
    }
    for junction in &project.junctions {
        let n = ends.get(&junction.id).copied().unwrap_or(0);
        if n == 0 || (!junction.placed && n < 3) {
            return Err(invalid(
                "A junction must join three or more wire ends, or be placed on a wire.",
            ));
        }
    }
    Ok(())
}

/// The net a wire carries: the net of the first terminal reached from it
/// through junctions, else, when it reaches a label or sheet pin, the net
/// that names join it to. `None` when its group reaches no terminal.
pub fn wire_net<'a>(project: &'a Project, wire: &Wire) -> Option<&'a Net> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![wire.from, wire.to];
    let mut named = None;
    while let Some(end) = stack.pop() {
        match end {
            WireEnd::Terminal(endpoint) => return project.net_of(endpoint),
            WireEnd::Label(_) | WireEnd::SheetPin(_) => {
                named.get_or_insert(end);
            }
            WireEnd::Junction(j) => {
                if !seen.insert(j.junction) {
                    continue;
                }
                for w in project.wires_at(end) {
                    stack.extend([w.from, w.to].into_iter().filter(|e| *e != end));
                }
            }
            WireEnd::Free(_) => {}
        }
    }
    let net = *nets::point_nets(project).get(&named?)?;
    project.net(net)
}
