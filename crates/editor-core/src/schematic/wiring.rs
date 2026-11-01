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
use crate::ids::{Endpoint, Id};
use crate::model::{Junction, JunctionRef, Net, Project, Wire, WireEnd};
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
/// junction (`{"junction"}`) or a point on a wire (`{"wire", "at"}`), which
/// splits that wire at a new junction. A point at a wire's end is that end.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum WireTarget {
    Terminal(Endpoint),
    Junction(JunctionRef),
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
        }
    }
}

/// The drawn path of a wire from `a` to `b`: along x first, then along y.
pub fn wire_route(a: Point, b: Point) -> [Point; 3] {
    [a, [b[0], a[1]], b]
}

/// Where a wire end sits on the sheet. `None` for a terminal whose unit is
/// not placed or a missing junction.
pub fn end_position(project: &Project, end: WireEnd) -> Option<Point> {
    match end {
        WireEnd::Terminal(endpoint) => pin_position(project, endpoint),
        WireEnd::Junction(j) => project.junction(j.junction).map(|j| j.position),
    }
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
        WireTarget::OnWire(point) => split(project, point),
    }
}

/// Split a wire at a point on its route: the wire keeps its ID from its
/// first end to a new junction, and a new wire runs from the junction to its
/// second end, so the drawn path is unchanged. A point at an end is that
/// end and splits nothing.
fn split(project: &mut Project, point: WirePoint) -> Result<WireEnd> {
    if !point_valid(point.at) {
        return Err(invalid(
            "Schematic position must be finite and within ±10000 mm.",
        ));
    }
    let wire = project
        .wire(point.wire)
        .cloned()
        .ok_or_else(|| not_found("Unknown schematic wire."))?;
    let (a, b) =
        wire_ends(project, &wire).ok_or_else(|| invalid("This wire is not drawn on the sheet."))?;
    let at = nearest_on_route(&wire_route(a, b), point.at);
    if distance(at, point.at) > JUNCTION_TOLERANCE_MM {
        return Err(invalid("The point is not on the wire."));
    }
    if distance(at, a) <= JUNCTION_TOLERANCE_MM {
        return Ok(wire.from);
    }
    if distance(at, b) <= JUNCTION_TOLERANCE_MM {
        return Ok(wire.to);
    }
    if project.junctions.len() >= MAX_JUNCTIONS || project.wires.len() >= super::MAX_WIRES {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires and 16384 junctions.".into(),
        ));
    }
    let junction = project.allocate()?;
    let tail = project.allocate()?;
    let end = WireEnd::junction(junction);
    project.junctions.push(Junction {
        id: junction,
        position: at,
    });
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

/// Remove junctions left with fewer than two wires, and the stub wire of a
/// junction with one, repeating along the chain it frees. A dangling stub
/// joins nothing. A junction with two wires stays as a bend point.
pub fn tidy(project: &mut Project) {
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

/// Where three or more conductors meet: a junction with three or more
/// wires, or a terminal with two or more (the pin is the third). Each is
/// drawn as a solid dot; a terminal end of a single wire and a two-wire bend
/// get none. Terminals whose unit is not placed are skipped.
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
            WireEnd::Junction(_) => *wires >= 3,
        })
        .filter_map(|(end, _)| Some((end, end_position(project, end)?)))
        .collect()
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
            };
            if !known {
                return Err(invalid("A wire references a missing terminal or junction."));
            }
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
/// through junctions. `None` when its group reaches no terminal.
pub fn wire_net<'a>(project: &'a Project, wire: &Wire) -> Option<&'a Net> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![wire.from, wire.to];
    while let Some(end) = stack.pop() {
        match end {
            WireEnd::Terminal(endpoint) => return project.net_of(endpoint),
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
    None
}
