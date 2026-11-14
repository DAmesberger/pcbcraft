//! Board via commands, the document rules over vias, routing through a
//! layer change, and the via manufacturing checks (drill size, annular
//! ring, aspect ratio, drill to copper, hole to hole, drill to board edge,
//! and mounting holes against vias and copper). Every limit comes from the
//! project's design rules and net classes ([`BoardRules`]); the constants
//! here are the default profile's values. Connectivity through vias lives
//! in the copper graph ([`super::connectivity`]): one land per layer the
//! via reaches, joined by its barrel.
use super::board_edge::{BoardEdges, EdgeVerdict};
use super::copper::check_reach;
use super::grid::BoundsGrid;
use super::limits::{BoardRules, mm};
use super::{
    BoardCopper, CopperRing, EPS, Error, Result, RouteEditSet, RouteRequest,
    WorldPad, check_pair_budget, distance, invalid,
};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::model::{CopperLayer, Project, Via, ViaSpan};
use crate::rules::{CopperObject, RuleKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Largest via pad, mm: an input bound on the document, not a rule (the
/// fab profile's `min drill` and `min via annular ring` are the limits
/// vias are placed and checked against).
pub const MAX_DIAMETER_MM: f64 = 6.0;

/// A via to add: everything but its identity.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ViaRequest {
    pub net: Id,
    pub position: Point,
    pub drill: f64,
    pub diameter: f64,
    pub span: ViaSpan,
    pub tented: bool,
}

impl ViaRequest {
    /// A through via on `net` at `position` of the rules' default via
    /// drill and pad, tented as `tent vias` says.
    pub fn through(rules: &crate::rules::DesignRules, net: Id, position: Point) -> Self {
        Self {
            net,
            position,
            drill: rules.holes.via_drill_default,
            diameter: rules.holes.via_diameter_default,
            span: ViaSpan::Through,
            tented: rules.mask.tent_vias,
        }
    }

    /// A test fixture, never in the product: the test prototype profile's
    /// default via (0.3 mm drill, 0.6 mm pad, tented).
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn fixture(net: Id, position: Point) -> Self {
        Self {
            net,
            position,
            drill: 0.3,
            diameter: 0.6,
            span: ViaSpan::Through,
            tented: true,
        }
    }
}

/// A route that changes layer: a track on `request.layer` to
/// `request.end`, a via there, then a track on `to_layer` to `end`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerChangeRequest {
    pub request: super::RouteRequest,
    pub via: ViaRequest,
    pub to_layer: CopperLayer,
    pub end: Point,
}

/// Add a via clear of other nets' copper; returns its identity.
pub(super) fn add(project: &mut Project, request: ViaRequest) -> Result<Id> {
    let id = insert(project, request)?;
    check_clearance(project, id)?;
    Ok(id)
}

pub(super) fn insert(project: &mut Project, request: ViaRequest) -> Result<Id> {
    let id = project.allocate()?;
    project.vias.push(Via {
        id,
        net: request.net,
        position: request.position,
        drill: request.drill,
        diameter: request.diameter,
        span: request.span,
        tented: request.tented,
        locked: false,
    });
    Ok(id)
}

fn unlocked(project: &mut Project, id: Id) -> Result<&mut Via> {
    let at = project.vias.iter().position(|v| v.id == id);
    unlocked_at(project, at)
}

/// The unlocked via at index `at` of the project's vias (as an id index
/// found it); `None` is an unknown via.
pub(super) fn unlocked_at(project: &mut Project, at: Option<usize>) -> Result<&mut Via> {
    let via = at
        .and_then(|k| project.vias.get_mut(k))
        .ok_or_else(|| Error::NotFound("unknown via".into()))?;
    if via.locked {
        return Err(Error::Conflict("via is locked".into()));
    }
    Ok(via)
}

pub(super) fn move_to(project: &mut Project, id: Id, position: Point) -> Result<()> {
    unlocked(project, id)?.position = position;
    check_clearance(project, id)
}

/// Route `request.request` to the via, place the via at its end and route
/// on `to_layer` to `request.end`, with one width: one transaction that
/// creates two tracks and the via. Returns the via.
pub(super) fn route_layer_change(project: &mut Project, request: LayerChangeRequest) -> Result<Id> {
    let LayerChangeRequest {
        request: first,
        via,
        to_layer,
        end,
    } = request;
    if !point_valid(via.position) || distance(via.position, first.end) > EPS {
        return invalid("the via must sit at the end of the first route");
    }
    if via.net != first.net {
        return invalid("the via must be on the routed net");
    }
    if to_layer == first.layer {
        return invalid("a layer change needs a different copper layer to route on");
    }
    for layer in [first.layer, to_layer] {
        if !project.stackup.has(layer) {
            return Err(Error::Invalid(format!(
                "the stackup has no {} copper layer",
                layer.name()
            )));
        }
    }
    let layers = project.stackup.layers();
    let probe = Via {
        id: 0,
        net: via.net,
        position: via.position,
        drill: via.drill,
        diameter: via.diameter,
        span: via.span,
        tented: via.tented,
        locked: false,
    };
    if !probe.on(first.layer, &layers) || !probe.on(to_layer, &layers) {
        return invalid("the via must join both routed layers");
    }
    let id = insert(project, via)?;
    let second = RouteRequest {
        net: first.net,
        start: first.end,
        end,
        width: first.width,
        layer: to_layer,
    };
    super::apply_routes(
        project,
        RouteEditSet {
            add: vec![first, second],
            ..Default::default()
        },
    )?;
    check_clearance(project, id)?;
    Ok(id)
}

/// Whether a via has copper on any layer a pad has copper on.
fn shares_layer(via: &Via, pad: &WorldPad, layers: &[CopperLayer]) -> bool {
    layers.iter().any(|l| pad.on(*l) && via.on(*l, layers))
}

/// Whether a via is closer than the effective clearance (rules, net
/// classes and class pairs) to another net's pad: the via side of a
/// revision impact plan.
pub fn via_pad_clearance_conflict(
    via: &Via,
    pad: &WorldPad,
    layers: &[CopperLayer],
    rules: &super::limits::BoardRules<'_>,
) -> bool {
    use crate::rules::CopperObject;
    via.net != pad.net
        && shares_layer(via, pad, layers)
        && CopperRing::of_via(via).closer_than(
            &CopperRing::of_pad(pad),
            rules.clearance(
                CopperObject::Via,
                Some(via.net),
                CopperObject::Pad,
                Some(pad.net),
            ),
        )
}

/// Refuse a via closer than the clearance rule to another net's copper on
/// a layer it joins, as routing refuses a track.
pub(super) fn check_clearance(project: &Project, id: Id) -> Result<()> {
    let via = project
        .via(id)
        .ok_or_else(|| Error::NotFound("unknown via".into()))?;
    let layers = project.stackup.layers();
    let limits = BoardRules::of(project);
    let ring = CopperRing::of_via(via);
    let me = (CopperObject::Via, Some(via.net));
    let refuse = |kind: &str, object: CopperObject, net: Id| {
        Err(Error::Conflict(format!(
            "Too close to a {kind} on {}: {}",
            project.net_name(net),
            limits.clearance_need(me.0, me.1, object, Some(net)).short()
        )))
    };
    for track in project.tracks.iter().filter(|t| t.net != via.net) {
        let need = limits.clearance(me.0, me.1, CopperObject::Track, Some(track.net));
        if via.on(track.layer, &layers) && ring.closer_than(&CopperRing::of_track(track), need) {
            return refuse("track", CopperObject::Track, track.net);
        }
    }
    for pad in super::pads(project).iter().filter(|p| p.net != via.net) {
        let need = limits.clearance(me.0, me.1, CopperObject::Pad, Some(pad.net));
        if shares_layer(via, pad, &layers) && ring.closer_than(&CopperRing::of_pad(pad), need) {
            return refuse("pad", CopperObject::Pad, pad.net);
        }
    }
    for other in project.vias.iter().filter(|v| v.net != via.net) {
        let need = limits.clearance(me.0, me.1, CopperObject::Via, Some(other.net));
        if layers
            .iter()
            .any(|l| via.on(*l, &layers) && other.on(*l, &layers))
            && ring.closer_than(&CopperRing::of_via(other), need)
        {
            return refuse("via", CopperObject::Via, other.net);
        }
    }
    Ok(())
}

/// Every copper shape a drilled hole is checked against, with its object,
/// kind and net, in the order findings name them (pads, vias, tracks).
/// `track_rings` are `project`'s tracks' outlines, in document order.
pub(super) fn drilled_copper<'a>(
    copper: &'a BoardCopper,
    project: &Project,
    track_rings: &'a [CopperRing],
) -> Vec<(&'a CopperRing, ObjectRef, CopperObject, Id)> {
    let mut shapes = Vec::new();
    for (k, pad) in copper.pads().iter().enumerate() {
        if let Some(ring) = copper.ring(k) {
            let object = ObjectRef::Pad {
                instance: pad.instance,
                pad: pad.pad,
            };
            shapes.push((ring, object, CopperObject::Pad, pad.net));
        }
    }
    for (k, via) in copper.vias().iter().enumerate() {
        if let Some(ring) = copper.via_ring(k) {
            shapes.push((
                ring,
                ObjectRef::Via { id: via.id },
                CopperObject::Via,
                via.net,
            ));
        }
    }
    for (track, ring) in project.tracks.iter().zip(track_rings) {
        let object = ObjectRef::Track { id: track.id };
        shapes.push((ring, object, CopperObject::Track, track.net));
    }
    shapes
}

/// The via manufacturing checks over `copper`'s vias: drill size, annular
/// ring, aspect ratio, drilled hole to another net's copper on any layer,
/// hole to hole and drill to board edge; then each mounting hole of the
/// board shape against via drills and all copper. Every limit is the
/// project's ([`BoardRules`]); pairs are found through bounds grids that
/// reach as far as the largest clearance. Copper clearance is part of
/// [`BoardCopper::checks`]; connectivity through vias is the copper
/// graph's.
pub(super) fn findings(copper: &BoardCopper, project: &Project) -> Vec<Finding> {
    let tracks = &project.tracks;
    let limits = BoardRules::of(project);
    let edges = BoardEdges::of(project);
    let drill_min = limits.need(RuleKey::DrillMin);
    let ring_min = limits.need(RuleKey::ViaAnnularRingMin);
    let aspect_max = limits.rules().holes.aspect_ratio_max;
    let thickness = project.stackup.thickness_mm();
    let mut issues = Vec::new();
    let track_rings: Vec<CopperRing> = tracks.iter().map(CopperRing::of_track).collect();
    let vias = copper.vias();
    let drills: Vec<CopperRing> = vias.iter().map(CopperRing::of_drill).collect();
    let mut pairs = 0usize;
    let budget = check_pair_budget(
        copper
            .pads()
            .len()
            .saturating_add(vias.len())
            .saturating_add(tracks.len()),
    );
    let over = |pairs: &mut usize, issues: &mut Vec<Finding>| {
        *pairs += 1;
        let over = *pairs > budget;
        if over {
            issues.push(Finding::new(
                "board.check_limit",
                "Via check limit reached; remaining vias are unchecked".into(),
                vec![],
            ));
        }
        over
    };
    let copper_shapes = drilled_copper(copper, project, &track_rings);
    let reach = check_reach(&limits);
    let shapes = BoundsGrid::new(copper_shapes.iter().map(|s| s.0.bounds), reach);
    let holes = BoundsGrid::new(drills.iter().map(|d| d.bounds), reach);
    let mut near = Vec::new();
    // Via drills keep the via hole rules; a mounting hole its own kind's
    // (plated or not, `CopperObject::hole`).
    let hole = CopperObject::ViaHole;
    let edge = limits.clearance(hole, None, CopperObject::BoardEdge, None);
    let spacing = limits.clearance(hole, None, hole, None);
    for (i, (via, drill)) in vias.iter().zip(&drills).enumerate() {
        let me = ObjectRef::Via { id: via.id };
        let name = project.net_name(via.net);
        if via.drill < drill_min.mm - EPS {
            issues.push(Finding::new(
                "board.via_drill_size",
                format!(
                    "A via on {name}: drill {} mm is under the minimum; {}",
                    mm(via.drill),
                    drill_min.text()
                ),
                vec![me],
            ));
        }
        if via.annular_ring() < ring_min.mm - EPS {
            issues.push(Finding::new(
                "board.via_annular_ring",
                format!(
                    "A via on {name}: annular ring {:.3} mm is under the minimum; {}",
                    via.annular_ring(),
                    ring_min.text()
                ),
                vec![me],
            ));
        }
        if let Some(max) = aspect_max
            && via.drill > 0.0
            && thickness / via.drill > max + 1e-9
        {
            issues.push(Finding::new(
                "board.via_aspect_ratio",
                format!(
                    "A via on {name}: aspect ratio {:.1}:1 ({} mm board, {} mm drill) is over the maximum; needs at most {}:1 ({})",
                    thickness / via.drill,
                    mm(thickness),
                    mm(via.drill),
                    mm(max),
                    super::limits::cite(limits.project(), RuleKey::AspectRatioMax)
                ),
                vec![me],
            ));
        }
        if let EdgeVerdict::TooClose { gap, near } = edges.verdict(drill, edge) {
            let need = limits.clearance_need(hole, None, CopperObject::BoardEdge, None);
            issues.push(Finding::new(
                "board.edge_clearance",
                format!(
                    "A via on {name}: drill is {} mm from {}; {}",
                    mm(gap),
                    near.describe(),
                    need.text()
                ),
                vec![me],
            ));
        }
        let Some(bounds) = drill.bounds else {
            continue;
        };
        shapes.near(&bounds, reach, &mut near);
        for (ring, object, kind, net) in near
            .iter()
            .filter_map(|k| copper_shapes.get(*k))
            .filter(|s| s.3 != via.net)
        {
            if over(&mut pairs, &mut issues) {
                return issues;
            }
            let need = limits.clearance(hole, Some(via.net), *kind, Some(*net));
            if drill.closer_than(ring, need) {
                let need = limits.clearance_need(hole, Some(via.net), *kind, Some(*net));
                issues.push(Finding::new(
                    "board.via_drill_clearance",
                    format!(
                        "A via on {name}: drill is {} mm from a {} on {}; {}",
                        mm(drill.gap(ring, need.mm + 1.0)),
                        kind.label(),
                        project.net_name(*net),
                        need.text()
                    ),
                    vec![me, *object],
                ));
            }
        }
        holes.near(&bounds, reach, &mut near);
        for (other, other_drill) in near
            .iter()
            .filter(|j| **j > i)
            .filter_map(|j| Some((vias.get(*j)?, drills.get(*j)?)))
        {
            if over(&mut pairs, &mut issues) {
                return issues;
            }
            if drill.closer_than(other_drill, spacing) {
                let need = limits.clearance_need(hole, None, hole, None);
                issues.push(Finding::new(
                    "board.via_hole_spacing",
                    format!(
                        "Vias on {name} and {}: drills are {} mm apart; {}",
                        project.net_name(other.net),
                        mm(drill.gap(other_drill, need.mm + 1.0)),
                        need.text()
                    ),
                    vec![me, ObjectRef::Via { id: other.id }],
                ));
            }
        }
    }
    // Mounting holes have no net: every copper object and via drill is
    // measured against them.
    let board = project.board();
    for mount in &board.holes {
        let kind = CopperObject::hole(mount.plated);
        let ring = CopperRing::of_circle(mount.at, mount.drill);
        let edge = limits.clearance(kind, None, CopperObject::BoardEdge, None);
        if let EdgeVerdict::TooClose { gap, near } = edges.verdict(&ring, edge) {
            let need = limits.clearance_need(kind, None, CopperObject::BoardEdge, None);
            issues.push(Finding::new(
                "board.edge_clearance",
                format!(
                    "A mounting hole is {} mm from {}; {}",
                    mm(gap),
                    near.describe(),
                    need.text()
                ),
                vec![ObjectRef::Board],
            ));
        }
        let Some(bounds) = ring.bounds else {
            continue;
        };
        // Its own plated ring (if any) is not board copper here.
        shapes.near(&bounds, reach, &mut near);
        for (other, object, other_kind, net) in near.iter().filter_map(|k| copper_shapes.get(*k)) {
            if over(&mut pairs, &mut issues) {
                return issues;
            }
            let need = limits.clearance(kind, None, *other_kind, Some(*net));
            if ring.closer_than(other, need) {
                let need = limits.clearance_need(kind, None, *other_kind, Some(*net));
                let what = copper_words(project, object, *other_kind, *net);
                let gap = ring.gap(other, need.mm + 1.0);
                // Touching or overlapping copper is said so, not "0 mm".
                let how = if gap <= EPS {
                    format!("overlaps {what}")
                } else {
                    format!("is {} mm from {what}", mm(gap))
                };
                issues.push(Finding::new(
                    "board.hole_clearance",
                    format!("A mounting hole {how}; {}", need.text()),
                    vec![ObjectRef::Board, *object],
                ));
            }
        }
        holes.near(&bounds, reach, &mut near);
        for (via, drill) in near
            .iter()
            .filter_map(|j| Some((vias.get(*j)?, drills.get(*j)?)))
        {
            if over(&mut pairs, &mut issues) {
                return issues;
            }
            let spacing = limits.clearance(kind, None, hole, None);
            if ring.closer_than(drill, spacing) {
                let need = limits.clearance_need(kind, None, hole, None);
                issues.push(Finding::new(
                    "board.hole_clearance",
                    format!(
                        "A mounting hole and a via on {}: drills are {} mm apart; {}",
                        project.net_name(via.net),
                        mm(ring.gap(drill, need.mm + 1.0)),
                        need.text()
                    ),
                    vec![ObjectRef::Board, ObjectRef::Via { id: via.id }],
                ));
            }
        }
    }
    issues
}

/// A copper object in a mounting-hole message, named by its part or net:
/// "U1 pad 1 on GND", "a track on GND", "a via on VCC".
pub(super) fn copper_words(
    project: &Project,
    object: &ObjectRef,
    kind: CopperObject,
    net: Id,
) -> String {
    let net = project.net_name(net);
    match *object {
        ObjectRef::Pad { instance, pad } => {
            let part = super::outline::part_name(project, instance);
            let number = project
                .placement_of(instance)
                .and_then(|_| project.instance(instance))
                .and_then(|i| crate::instances::resolve(&project.library, i).ok())
                .and_then(|r| r.physical)
                .and_then(|p| {
                    p.footprint
                        .definition
                        .pads
                        .iter()
                        .find(|q| q.id == pad)
                        .map(|q| q.number.clone())
                })
                .filter(|n| !n.is_empty());
            match number {
                Some(n) => format!("{part} pad {n} on {net}"),
                None => format!("a pad of {part} on {net}"),
            }
        }
        _ => format!("a {} on {net}", kind.label()),
    }
}

pub(super) fn delete(project: &mut Project, id: Id) -> Result<()> {
    unlocked(project, id)?;
    project.vias.retain(|v| v.id != id);
    Ok(())
}

pub(super) fn set_tented(project: &mut Project, id: Id, tented: bool) -> Result<()> {
    unlocked(project, id)?.tented = tented;
    Ok(())
}

pub(super) fn set_lock(project: &mut Project, id: Id, locked: bool) -> Result<()> {
    project
        .vias
        .iter_mut()
        .find(|v| v.id == id)
        .ok_or_else(|| Error::NotFound("unknown via".into()))?
        .locked = locked;
    Ok(())
}

/// Document rules over vias: unique identities, known nets,
/// sane geometry inside the board, through vias only for now.
pub(super) fn validate(project: &Project, nets: &BTreeSet<Id>) -> Result<()> {
    let mut ids = BTreeSet::new();
    for via in &project.vias {
        if via.id == 0 || via.id >= project.next_id || !ids.insert(via.id) {
            return invalid("invalid or duplicate via identity");
        }
        if !nets.contains(&via.net) {
            return invalid("via references an unknown net");
        }
        if !point_valid(via.position)
            || !via.drill.is_finite()
            || !via.diameter.is_finite()
            || via.drill <= 0.0
            || via.diameter > MAX_DIAMETER_MM
            || via.diameter <= via.drill + EPS
        {
            return invalid(&format!(
                "via drill must be positive and smaller than its pad, at most {MAX_DIAMETER_MM} mm"
            ));
        }
        let r = via.diameter / 2.0;
        let [w, h] = project.board_size;
        let [x, y] = via.position;
        if x - r < -EPS || y - r < -EPS || x + r > w + EPS || y + r > h + EPS {
            return invalid("via copper must lie inside the board outline");
        }
        if let ViaSpan::Between { .. } = via.span {
            return Err(Error::Unsupported(
                "blind and buried vias are not supported yet; use a through via".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
