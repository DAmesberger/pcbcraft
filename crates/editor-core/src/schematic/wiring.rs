//! Wire geometry and wire-to-wire connectivity.
//!
//! A wire is drawn as the horizontal-first route [`wire_route`]: from its
//! first end along x, then along y to its second end. Splitting a route at a
//! point on it gives two routes that draw exactly the original path, so a
//! wire end landing on a segment can split that wire at a junction without
//! moving any drawn line.
//!
//! Connectivity follows the schematic convention: wires join only where an
//! end lands on a terminal, a junction or another wire's segment (which
//! makes a junction there). A plain crossing never joins. The junction dot
//! is derived, never stored: [`junction_dots`] lists every point where three
//! or more conductors meet (a pin counts as one).
use super::{Error, Result, invalid, not_found, pin_position};
use crate::geometry::{Point, point_valid};
use crate::ids::{Endpoint, Id, SheetId};
use crate::instances;
use crate::model::{Junction, JunctionRef, LabelRef, Net, Project, SheetPinRef, Wire, WireEnd};
use crate::nets;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_JUNCTIONS: usize = super::MAX_WIRES;

/// How far a point given as "on a wire" may lie from its route, and how
/// near a route end a point counts as that end, in millimetres.
pub const JUNCTION_TOLERANCE_MM: f64 = 1e-3;

/// A point on an existing wire's route.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WirePoint {
    pub wire: Id,
    pub at: Point,
}

/// Where a new wire end lands: a terminal (`{"instance", "terminal"}`), a
/// junction (`{"junction"}`), a label or power port (`{"label"}`), a sheet
/// pin (`{"sheet_pin"}`) or a point on a wire (`{"wire", "at"}`), which
/// splits that wire at a new junction. A point at a wire's end is that end.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum WireTarget {
    Terminal(Endpoint),
    Junction(JunctionRef),
    Label(LabelRef),
    SheetPin(SheetPinRef),
    OnWire(WirePoint),
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
        }
    }
}

/// The drawn path of a wire from `a` to `b`: along x first, then along y.
pub fn wire_route(a: Point, b: Point) -> [Point; 3] {
    [a, [b[0], a[1]], b]
}

/// Where a wire end sits on its sheet. `None` for a terminal whose unit is
/// not placed or a missing object.
pub fn end_position(project: &Project, end: WireEnd) -> Option<Point> {
    match end {
        WireEnd::Terminal(endpoint) => pin_position(project, endpoint),
        WireEnd::Junction(j) => project.junction(j.junction).map(|j| j.position),
        WireEnd::Label(l) => project.label(l.label).map(|l| l.position),
        WireEnd::SheetPin(p) => project
            .sheet_pin(p.sheet_pin)
            .map(|(symbol, pin)| symbol.pin_position(pin)),
    }
}

/// The sheet a wire end lies on. `None` for a terminal whose unit is not
/// placed or a missing object.
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
    }
}

/// The sheet a wire is drawn on: that of its first end.
pub fn wire_sheet(project: &Project, wire: &Wire) -> Option<SheetId> {
    end_sheet(project, wire.from)
}

/// Both end positions of a wire.
pub fn wire_ends(project: &Project, wire: &Wire) -> Option<(Point, Point)> {
    end_position(project, wire.from).zip(end_position(project, wire.to))
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

/// The point of a route nearest `p`.
fn nearest_on_route(route: &[Point; 3], p: Point) -> Point {
    let first = nearest_on_segment(p, route[0], route[1]);
    let second = nearest_on_segment(p, route[1], route[2]);
    if distance(p, first) <= distance(p, second) {
        first
    } else {
        second
    }
}

/// The point of a wire's drawn route nearest `p`, for picking a point to
/// land a wire end on. `None` for an unknown or undrawn wire.
pub fn nearest_on_wire(project: &Project, wire: Id, p: Point) -> Option<Point> {
    let (a, b) = wire_ends(project, project.wire(wire)?)?;
    Some(nearest_on_route(&wire_route(a, b), p))
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
    }
}

/// Where a point on a wire lies on its route: the route point and whether
/// it is one of the wire's ends.
pub(super) fn on_route(project: &Project, point: WirePoint) -> Result<(Point, Option<WireEnd>)> {
    if !point_valid(point.at) {
        return Err(invalid(
            "Schematic position must be finite and within ±10000 mm.",
        ));
    }
    let wire = project
        .wire(point.wire)
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    let (a, b) =
        wire_ends(project, wire).ok_or_else(|| invalid("This wire is not drawn on the sheet."))?;
    let at = nearest_on_route(&wire_route(a, b), point.at);
    if distance(at, point.at) > JUNCTION_TOLERANCE_MM {
        return Err(invalid("The point is not on the wire."));
    }
    if distance(at, a) <= JUNCTION_TOLERANCE_MM {
        return Ok((a, Some(wire.from)));
    }
    if distance(at, b) <= JUNCTION_TOLERANCE_MM {
        return Ok((b, Some(wire.to)));
    }
    Ok((at, None))
}

/// Split a wire at a point on its route: the wire keeps its ID from its
/// first end to the split point, and a new wire runs from there to its
/// second end, so the drawn path is unchanged. The split point is `node`
/// (a label already at that point) or a new junction. A point at an end is
/// that end and splits nothing.
pub(super) fn split(
    project: &mut Project,
    point: WirePoint,
    node: Option<WireEnd>,
) -> Result<WireEnd> {
    if !point_valid(point.at) {
        return Err(invalid(
            "Schematic position must be finite and within ±10000 mm.",
        ));
    }
    let wire = project
        .wire(point.wire)
        .cloned()
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    let (at, end) = on_route(project, point)?;
    if let Some(end) = end {
        return Ok(end);
    }
    if project.junctions.len() >= MAX_JUNCTIONS || project.wires.len() >= super::MAX_WIRES {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires and 16384 junctions.".into(),
        ));
    }
    let end = match node {
        Some(node) => node,
        None => {
            let junction = project.allocate()?;
            let sheet = wire_sheet(project, &wire).unwrap_or_default();
            project.junctions.push(Junction {
                id: junction,
                sheet,
                position: at,
                placed: false,
            });
            WireEnd::junction(junction)
        }
    };
    let tail = project.allocate()?;
    let index = project
        .wires
        .iter()
        .position(|w| w.id == wire.id)
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    project.wires[index].to = end;
    project.wires.insert(
        index + 1,
        Wire {
            id: tail,
            from: end,
            to: wire.to,
        },
    );
    Ok(end)
}

/// Wires on each junction, by junction ID.
fn junction_wires(project: &Project) -> BTreeMap<Id, Vec<Id>> {
    let mut on: BTreeMap<Id, Vec<Id>> = project
        .junctions
        .iter()
        .map(|j| (j.id, Vec::new()))
        .collect();
    for wire in &project.wires {
        for end in [wire.from, wire.to] {
            if let Some(id) = end.junction_id()
                && let Some(list) = on.get_mut(&id)
            {
                list.push(wire.id);
            }
        }
    }
    on
}

/// Every connection point drawn on a sheet: placed pin ends, junctions,
/// labels and sheet pins, with their sheet and position.
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
    points.extend(
        project
            .labels
            .iter()
            .map(|l| (l.sheet, l.position, WireEnd::label(l.id))),
    );
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
/// the line.
struct LineIndex {
    rows: BTreeMap<(SheetId, i64), Vec<(f64, usize)>>,
    columns: BTreeMap<(SheetId, i64), Vec<(f64, usize)>>,
}

impl LineIndex {
    fn new(points: &[(SheetId, Point, WireEnd)]) -> Self {
        let mut index = Self {
            rows: BTreeMap::new(),
            columns: BTreeMap::new(),
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
        }
        for list in index.rows.values_mut().chain(index.columns.values_mut()) {
            list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        }
        index
    }
    /// Points within tolerance of the axis-aligned segment from `a` to `b`
    /// on `sheet`.
    fn on_segment(
        &self,
        points: &[(SheetId, Point, WireEnd)],
        sheet: SheetId,
        a: Point,
        b: Point,
        out: &mut Vec<usize>,
    ) {
        let horizontal = (a[1] - b[1]).abs() <= (a[0] - b[0]).abs();
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
                if let Some((_, p, _)) = points.get(*i)
                    && segment_distance_mm(*p, a, b) <= JUNCTION_TOLERANCE_MM
                {
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
    }
}

/// Where a new wire from `a` to `b` (its `ends`) stops: just its two ends
/// when its horizontal-first L runs over no other connection point; else
/// also the bends of the vertical-first L or of the shortest clean dogleg,
/// each a junction point between straight wires (so not on a wire).
/// Refused, naming the point in the way, when no such path is clean.
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
    let Some(blocker) = in_way(&wire_route(a, b)) else {
        return Ok(vec![a, b]);
    };
    // A bend becomes a junction point, so it must not lie on a wire.
    let on_wire = |bend: Point| {
        project
            .wires
            .iter()
            .filter(|w| wire_sheet(project, w) == Some(sheet))
            .filter_map(|w| wire_ends(project, w))
            .any(|(p, q)| {
                distance(nearest_on_route(&wire_route(p, q), bend), bend) <= JUNCTION_TOLERANCE_MM
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
        // Drop bends that coincide with an end or another bend.
        path.dedup_by(|p, q| distance(*p, *q) <= JUNCTION_TOLERANCE_MM);
        let bends = path
            .get(1..path.len().saturating_sub(1))
            .unwrap_or_default();
        if path.len() > 2
            && path.iter().all(|p| point_valid(*p))
            && in_way(&path).is_none()
            && !bends.iter().any(|p| on_wire(*p))
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
/// segment's middle splits the wire there into pieces that draw exactly
/// the original path (a T, whose point then carries three conductors), and
/// a point on a wire's end that is not that end is joined to it by a
/// zero-length wire. Crossing wires have no
/// point at the crossing, so they stay apart. Idempotent: afterwards no
/// point lies inside any wire's path.
fn join_touching(project: &mut Project) -> Result<()> {
    if project.wires.is_empty() {
        return Ok(());
    }
    let points = connection_points(project);
    let index = LineIndex::new(&points);
    // Per wire: the points inside its path in path order, and the points
    // on either end.
    let mut splits: BTreeMap<Id, Vec<WireEnd>> = BTreeMap::new();
    let mut joins: Vec<(WireEnd, WireEnd)> = Vec::new();
    let mut hits = Vec::new();
    for wire in &project.wires {
        let (Some(sheet), Some((a, b))) = (wire_sheet(project, wire), wire_ends(project, wire))
        else {
            continue;
        };
        let route = wire_route(a, b);
        hits.clear();
        index.on_segment(&points, sheet, route[0], route[1], &mut hits);
        index.on_segment(&points, sheet, route[1], route[2], &mut hits);
        let mut inside: BTreeMap<WireEnd, f64> = BTreeMap::new();
        for i in &hits {
            let Some((_, p, end)) = points.get(*i) else {
                continue;
            };
            if *end == wire.from || *end == wire.to {
                continue;
            }
            if distance(*p, a) <= JUNCTION_TOLERANCE_MM {
                joins.push((wire.from, *end));
            } else if distance(*p, b) <= JUNCTION_TOLERANCE_MM {
                joins.push((wire.to, *end));
            } else {
                // Distance along the path from the first end.
                let along = if distance(nearest_on_segment(*p, route[0], route[1]), *p)
                    <= JUNCTION_TOLERANCE_MM
                {
                    distance(route[0], *p)
                } else {
                    distance(route[0], route[1]) + distance(route[1], *p)
                };
                inside.entry(*end).or_insert(along);
            }
        }
        if !inside.is_empty() {
            let mut order: Vec<(f64, WireEnd)> = inside.into_iter().map(|(e, s)| (s, e)).collect();
            order.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            splits.insert(wire.id, order.into_iter().map(|(_, e)| e).collect());
        }
    }
    if splits.is_empty() && joins.is_empty() {
        return Ok(());
    }
    let pair = |a: WireEnd, b: WireEnd| if a < b { (a, b) } else { (b, a) };
    let mut taken: BTreeSet<(WireEnd, WireEnd)> = project
        .wires
        .iter()
        .filter(|w| !splits.contains_key(&w.id))
        .map(|w| pair(w.from, w.to))
        .collect();
    let old = std::mem::take(&mut project.wires);
    let mut wires = Vec::with_capacity(old.len());
    for wire in old {
        let Some(through) = splits.get(&wire.id) else {
            wires.push(wire);
            continue;
        };
        let mut stops = Vec::with_capacity(through.len() + 2);
        stops.push(wire.from);
        stops.extend(through.iter().copied());
        stops.push(wire.to);
        let mut first = true;
        for leg in stops.windows(2) {
            let (from, to) = (leg[0], leg[1]);
            if from == to || !taken.insert(pair(from, to)) {
                continue;
            }
            let id = if first { wire.id } else { project.allocate()? };
            first = false;
            wires.push(Wire { id, from, to });
        }
    }
    // A point on a wire's end joins that end with a wire of no length.
    for (end, point) in joins {
        if taken.insert(pair(end, point)) {
            let id = project.allocate()?;
            wires.push(Wire {
                id,
                from: end,
                to: point,
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

/// Bring the wiring to its normal form after an edit: join every
/// connection point lying on a wire ([`join_touching`]), then remove
/// junctions left with fewer than two wires, and the stub wire of a
/// junction with one, repeating along the chain it frees. A dangling stub
/// joins nothing. A junction with two wires stays as a bend point.
pub fn tidy(project: &mut Project) -> Result<()> {
    join_touching(project)?;
    remove_stubs(project);
    Ok(())
}

fn remove_stubs(project: &mut Project) {
    let on = junction_wires(project);
    let mut removed_wires = BTreeSet::new();
    let mut removed_junctions = BTreeSet::new();
    let mut queue: Vec<Id> = on
        .iter()
        .filter(|(_, wires)| wires.len() < 2)
        .map(|(id, _)| *id)
        .collect();
    while let Some(id) = queue.pop() {
        if !removed_junctions.insert(id) {
            continue;
        }
        let wires: Vec<Id> = on
            .get(&id)
            .map(|w| {
                w.iter()
                    .filter(|w| !removed_wires.contains(*w))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        for wire in wires {
            removed_wires.insert(wire);
            let Some(w) = project.wire(wire) else {
                continue;
            };
            for other in [w.from, w.to].into_iter().filter_map(WireEnd::junction_id) {
                if other == id || removed_junctions.contains(&other) {
                    continue;
                }
                let live = on.get(&other).map_or(0, |list| {
                    list.iter().filter(|w| !removed_wires.contains(*w)).count()
                });
                if live < 2 {
                    queue.push(other);
                }
            }
        }
    }
    if removed_junctions.is_empty() {
        return;
    }
    project.wires.retain(|w| !removed_wires.contains(&w.id));
    project
        .junctions
        .retain(|j| !removed_junctions.contains(&j.id));
}

/// The axis direction (unit steps in x and y) in which `wire` leaves its
/// end `at`, along its drawn route. `None` for a wire of no length or an
/// end that is not `at`.
fn leaving(project: &Project, wire: &Wire, at: WireEnd) -> Option<[i8; 2]> {
    let (a, b) = wire_ends(project, wire)?;
    let route = wire_route(a, b);
    let legs: [[Point; 2]; 2] = if wire.from == at {
        [[route[0], route[1]], [route[1], route[2]]]
    } else if wire.to == at {
        [[route[2], route[1]], [route[1], route[0]]]
    } else {
        return None;
    };
    let [p, q] = legs
        .into_iter()
        .find(|[p, q]| distance(*p, *q) > JUNCTION_TOLERANCE_MM)?;
    let sign = |v: f64| {
        if v > JUNCTION_TOLERANCE_MM {
            1
        } else if v < -JUNCTION_TOLERANCE_MM {
            -1
        } else {
            0
        }
    };
    Some([sign(q[0] - p[0]), sign(q[1] - p[1])])
}

/// Where three or more conductors meet: a junction with three or more
/// wires, or a terminal with two or more (the pin is the third), and every
/// junction point put there with `junction.place`. Each is drawn as a
/// solid dot; a terminal end of a single wire and a two-wire bend get none.
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
        .filter(|w| wire_sheet(project, w) == Some(sheet))
        .filter_map(|w| wire_ends(project, w))
        .map(|(a, b)| nearest_on_route(&wire_route(a, b), position))
        .find(|p| distance(*p, position) <= JUNCTION_TOLERANCE_MM)
        .ok_or_else(|| invalid("A junction point goes on a wire."))?;
    if project.junctions.len() >= MAX_JUNCTIONS {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires and 16384 junctions.".into(),
        ));
    }
    let id = project.allocate()?;
    project.junctions.push(Junction {
        id,
        sheet,
        position: on,
        placed: true,
    });
    Ok(id)
}

/// Remove a junction point that straight wires pass through
/// (`junction.remove`): each pair leaving it in opposite directions, each
/// ending straight across from it, becomes one wire with the same drawing.
pub(super) fn remove_junction(project: &mut Project, id: Id) -> Result<()> {
    let at = WireEnd::junction(id);
    let position = project
        .junction(id)
        .map(|j| j.position)
        .ok_or_else(|| not_found("Unknown junction."))?;
    let refused = || {
        super::conflict(
            "This junction point is a bend or a T; remove a wire to change the drawing there.",
        )
    };
    let wires: Vec<Wire> = project.wires_at(at).cloned().collect();
    let mut legs = Vec::with_capacity(wires.len());
    for wire in &wires {
        let far = if wire.from == at { wire.to } else { wire.from };
        let far_at = end_position(project, far).ok_or_else(refused)?;
        let d = leaving(project, wire, at).ok_or_else(refused)?;
        // Straight across: the far end lies on the line the wire leaves on.
        let straight = if d[0] == 0 {
            (far_at[0] - position[0]).abs() <= JUNCTION_TOLERANCE_MM
        } else {
            (far_at[1] - position[1]).abs() <= JUNCTION_TOLERANCE_MM
        };
        if !straight {
            return Err(refused());
        }
        legs.push((d, far));
    }
    let mut merged = Vec::new();
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
        merged.push((a, legs[partner].1));
    }
    project.wires.retain(|w| w.from != at && w.to != at);
    project.junctions.retain(|j| j.id != id);
    for (from, to) in merged {
        let exists = project
            .wires
            .iter()
            .any(|w| (w.from == from && w.to == to) || (w.from == to && w.to == from));
        if from != to && !exists {
            let wire = project.allocate()?;
            project.wires.push(Wire { id: wire, from, to });
        }
    }
    Ok(())
}

/// Wire and junction records: limits, references, no self-loops or
/// duplicate connections, and every junction joining at least two wires.
pub(super) fn validate(project: &Project) -> Result<()> {
    if project.junctions.len() > MAX_JUNCTIONS {
        return Err(invalid("Schematic exceeds supported item limits."));
    }
    for junction in &project.junctions {
        if !point_valid(junction.position) {
            return Err(invalid("A junction position is out of range."));
        }
    }
    let mut pairs = BTreeSet::new();
    for wire in &project.wires {
        if wire.from == wire.to {
            return Err(invalid("A wire must connect two different points."));
        }
        for end in [wire.from, wire.to] {
            let known = match end {
                WireEnd::Terminal(endpoint) => project.instance(endpoint.instance).is_some(),
                WireEnd::Junction(j) => project.junction(j.junction).is_some(),
                WireEnd::Label(l) => project.label(l.label).is_some(),
                WireEnd::SheetPin(p) => project.sheet_pin(p.sheet_pin).is_some(),
            };
            if !known {
                return Err(invalid("A wire references a missing connection point."));
            }
        }
        if let (Some(a), Some(b)) = (end_sheet(project, wire.from), end_sheet(project, wire.to))
            && a != b
        {
            return Err(invalid("A wire must join two points on one sheet."));
        }
        let pair = if wire.from < wire.to {
            (wire.from, wire.to)
        } else {
            (wire.to, wire.from)
        };
        if !pairs.insert(pair) {
            return Err(invalid("Duplicate schematic connection."));
        }
    }
    if junction_wires(project).values().any(|w| w.len() < 2) {
        return Err(invalid("A junction must join at least two wires."));
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
        }
    }
    let net = *nets::point_nets(project).get(&named?)?;
    project.net(net)
}
