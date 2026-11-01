//! Board via commands, the document rules over vias, routing through a
//! layer change, and the via manufacturing checks (drill size, annular
//! ring, aspect ratio, drill to copper, hole to hole, drill to board edge,
//! and mounting holes against vias and copper). Every limit comes from the
//! project's design rules and net classes ([`BoardRules`]); the constants
//! here are the default profile's values. Connectivity through vias lives
//! in [`BoardCopper`], which treats a through via as copper on every layer.
use super::board_edge::{BoardEdges, EdgeVerdict};
use super::limits::{BoardRules, Need, mm};
use super::{
    BoardCopper, CopperRing, EPS, Error, MAX_CHECK_PAIRS, Result, RouteEditSet, RouteRequest,
    WorldPad, distance, invalid,
};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::model::{CopperLayer, Project, Stackup, Track, Via, ViaSpan};
use crate::rules::{CopperObject, RuleKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Most vias on one board.
pub const MAX_VIAS: usize = 4_096;
/// Default through-via drill and pad, in mm (a common fabricator standard
/// via, not a qualification).
pub const DEFAULT_VIA_DRILL_MM: f64 = 0.3;
pub const DEFAULT_VIA_DIAMETER_MM: f64 = 0.6;
const MIN_DRILL_MM: f64 = 0.1;
const MAX_DIAMETER_MM: f64 = 6.0;
/// The default profile's smallest via annular ring, in mm (an estimated
/// prototype default, not a fabricator qualification); the check reads
/// the project's `min via annular ring`.
pub const MIN_ANNULAR_RING_MM: f64 = 0.1;
/// The default profile's least gap from a drilled hole to another net's
/// copper on any layer, in mm (estimated); the check reads the `hole`
/// pairs of the project's clearance matrix.
pub const DRILL_TO_COPPER_MM: f64 = 0.25;
/// The default profile's least gap between two drilled holes, any nets,
/// in mm (estimated); the check reads `hole to hole clearance`.
pub const HOLE_TO_HOLE_MM: f64 = 0.25;

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
    /// A default through via on `net` at `position`, tented.
    pub fn through(net: Id, position: Point) -> Self {
        Self {
            net,
            position,
            drill: DEFAULT_VIA_DRILL_MM,
            diameter: DEFAULT_VIA_DIAMETER_MM,
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

fn insert(project: &mut Project, request: ViaRequest) -> Result<Id> {
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
    let via = project
        .vias
        .iter_mut()
        .find(|v| v.id == id)
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

/// Refuse a stackup that drops a copper layer tracks still use, naming it.
pub(super) fn check_stackup_change(project: &Project, stackup: &Stackup) -> Result<()> {
    stackup.check()?;
    let mut used = std::collections::BTreeMap::<CopperLayer, usize>::new();
    for track in &project.tracks {
        if !stackup.has(track.layer) {
            *used.entry(track.layer).or_default() += 1;
        }
    }
    if let Some((layer, count)) = used.into_iter().next() {
        return Err(Error::Conflict(format!(
            "{} still carries {count} track{}; delete or move them before removing the layer",
            layer.name(),
            if count == 1 { "" } else { "s" }
        )));
    }
    for via in &project.vias {
        if let ViaSpan::Between { from, to } = via.span
            && (!stackup.has(from) || !stackup.has(to))
        {
            return Err(Error::Conflict(format!(
                "via {} spans a copper layer the new stackup lacks",
                via.id
            )));
        }
    }
    Ok(())
}

/// Whether a via has copper on any layer a pad has copper on.
fn shares_layer(via: &Via, pad: &WorldPad, layers: &[CopperLayer]) -> bool {
    pad.through || via.on(pad.layer.into(), layers)
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

/// Whether a via's copper touches a pad's exact outline on a shared layer.
pub fn via_touches_pad(via: &Via, pad: &WorldPad, layers: &[CopperLayer]) -> bool {
    shares_layer(via, pad, layers) && CopperRing::of_via(via).touches(&CopperRing::of_pad(pad))
}

fn budget(pairs: &mut usize) -> Result<()> {
    *pairs += 1;
    if *pairs > MAX_CHECK_PAIRS {
        return Err(Error::Limit(
            "via clearance budget exceeded; split this edit into smaller transactions".into(),
        ));
    }
    Ok(())
}

/// Refuse a via closer than the clearance rule to another net's copper on
/// a layer it joins, as routing refuses a track.
fn check_clearance(project: &Project, id: Id) -> Result<()> {
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
    let mut pairs = 0usize;
    for track in project.tracks.iter().filter(|t| t.net != via.net) {
        budget(&mut pairs)?;
        let need = limits.clearance(me.0, me.1, CopperObject::Track, Some(track.net));
        if via.on(track.layer, &layers) && ring.closer_than(&CopperRing::of_track(track), need) {
            return refuse("track", CopperObject::Track, track.net);
        }
    }
    for pad in super::pads(project).iter().filter(|p| p.net != via.net) {
        budget(&mut pairs)?;
        let need = limits.clearance(me.0, me.1, CopperObject::Pad, Some(pad.net));
        if shares_layer(via, pad, &layers) && ring.closer_than(&CopperRing::of_pad(pad), need) {
            return refuse("pad", CopperObject::Pad, pad.net);
        }
    }
    for other in project.vias.iter().filter(|v| v.net != via.net) {
        budget(&mut pairs)?;
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

/// The first via on another net whose clearance the track breaks, with
/// the clearance it needs: the via side of routing's clearance check.
pub(super) fn track_violates_via<'a>(
    project: &'a Project,
    limits: &BoardRules<'_>,
    track: &Track,
    ring: &CopperRing,
) -> Option<(&'a Via, Need)> {
    let layers = project.stackup.layers();
    let (t, v) = (CopperObject::Track, CopperObject::Via);
    project
        .vias
        .iter()
        .find(|via| {
            via.net != track.net
                && via.on(track.layer, &layers)
                && ring.closer_than(
                    &CopperRing::of_via(via),
                    limits.clearance(t, Some(track.net), v, Some(via.net)),
                )
        })
        .map(|via| {
            (
                via,
                limits.clearance_need(t, Some(track.net), v, Some(via.net)),
            )
        })
}

/// The via manufacturing checks over `copper`'s vias: drill size, annular
/// ring, aspect ratio, drilled hole to another net's copper on any layer,
/// hole to hole and drill to board edge; then each mounting hole of the
/// board shape against via drills and all copper. Every limit is the
/// project's ([`BoardRules`]). Copper clearance and connectivity through
/// vias are part of [`BoardCopper::checks`].
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
    let over = |pairs: &mut usize, issues: &mut Vec<Finding>| {
        *pairs += 1;
        let over = *pairs > MAX_CHECK_PAIRS;
        if over {
            issues.push(Finding::new(
                "board.check_limit",
                "Via check limit reached; remaining vias are unchecked".into(),
                vec![],
            ));
        }
        over
    };
    // Other-net copper of every kind a drill is measured against.
    let others = |net: Option<Id>| {
        let pads = copper
            .pads()
            .iter()
            .enumerate()
            .filter(move |(_, p)| Some(p.net) != net)
            .filter_map(|(k, p)| {
                Some((
                    copper.ring(k)?,
                    ObjectRef::Pad {
                        instance: p.instance,
                        pad: p.pad,
                    },
                    CopperObject::Pad,
                    p.net,
                ))
            });
        let vias = vias
            .iter()
            .enumerate()
            .filter(move |(_, v)| Some(v.net) != net)
            .filter_map(|(k, v)| {
                Some((
                    copper.via_ring(k)?,
                    ObjectRef::Via { id: v.id },
                    CopperObject::Via,
                    v.net,
                ))
            });
        let wires = tracks
            .iter()
            .zip(&track_rings)
            .filter(move |(t, _)| Some(t.net) != net)
            .map(|(t, ring)| {
                (
                    ring,
                    ObjectRef::Track { id: t.id },
                    CopperObject::Track,
                    t.net,
                )
            });
        pads.chain(vias).chain(wires)
    };
    let hole = CopperObject::Hole;
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
        let edge = limits.clearance(hole, None, CopperObject::BoardEdge, None);
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
        for (ring, object, kind, net) in others(Some(via.net)) {
            if over(&mut pairs, &mut issues) {
                return issues;
            }
            let need = limits.clearance(hole, Some(via.net), kind, Some(net));
            if drill.closer_than(ring, need) {
                let need = limits.clearance_need(hole, Some(via.net), kind, Some(net));
                issues.push(Finding::new(
                    "board.via_drill_clearance",
                    format!(
                        "A via on {name}: drill is {} mm from a {} on {}; {}",
                        mm(drill.gap(ring, need.mm + 1.0)),
                        kind.label(),
                        project.net_name(net),
                        need.text()
                    ),
                    vec![me, object],
                ));
            }
        }
        let spacing = limits.clearance(hole, None, hole, None);
        for (other, other_drill) in vias.iter().zip(&drills).skip(i + 1) {
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
        let ring = CopperRing::of_circle(mount.at, mount.drill);
        let edge = limits.clearance(hole, None, CopperObject::BoardEdge, None);
        if let EdgeVerdict::TooClose { gap, near } = edges.verdict(&ring, edge) {
            let need = limits.clearance_need(hole, None, CopperObject::BoardEdge, None);
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
        // Its own plated ring (if any) is not board copper here.
        for (other, object, kind, net) in others(None) {
            if over(&mut pairs, &mut issues) {
                return issues;
            }
            let need = limits.clearance(hole, None, kind, Some(net));
            if ring.closer_than(other, need) {
                let need = limits.clearance_need(hole, None, kind, Some(net));
                issues.push(Finding::new(
                    "board.hole_clearance",
                    format!(
                        "A mounting hole is {} mm from a {} on {}; {}",
                        mm(ring.gap(other, need.mm + 1.0)),
                        kind.label(),
                        project.net_name(net),
                        need.text()
                    ),
                    vec![ObjectRef::Board, object],
                ));
            }
        }
        let spacing = limits.clearance(hole, None, hole, None);
        for (via, drill) in vias.iter().zip(&drills) {
            if over(&mut pairs, &mut issues) {
                return issues;
            }
            if ring.closer_than(drill, spacing) {
                let need = limits.clearance_need(hole, None, hole, None);
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

/// Document rules over vias: bounded count, unique identities, known nets,
/// sane geometry inside the board, through vias only for now.
pub(super) fn validate(project: &Project, nets: &BTreeSet<Id>) -> Result<()> {
    if project.vias.len() > MAX_VIAS {
        return invalid("prototype supports at most 4096 vias");
    }
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
            || via.drill < MIN_DRILL_MM
            || via.diameter > MAX_DIAMETER_MM
            || via.diameter <= via.drill + EPS
        {
            return invalid("via drill must be at least 0.1 mm and smaller than its pad");
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
mod tests;
