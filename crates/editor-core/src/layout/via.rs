//! Board via commands, the document rules over vias, routing through a
//! layer change, and the via manufacturing checks (annular ring, drill to
//! copper, hole to hole). Connectivity through vias lives in
//! [`BoardCopper`], which treats a through via as copper on every layer.
use super::{
    BoardCopper, CopperRing, EPS, Error, MAX_CHECK_PAIRS, Result, RouteEditSet, RouteRequest,
    WorldPad, distance, invalid,
};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::model::{CopperLayer, Project, Stackup, Track, Via, ViaSpan};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Most vias on one board.
pub const MAX_VIAS: usize = 16_384;
/// Default through-via drill and pad, in mm (a common fabricator standard
/// via, not a qualification).
pub const DEFAULT_VIA_DRILL_MM: f64 = 0.3;
pub const DEFAULT_VIA_DIAMETER_MM: f64 = 0.6;
const MIN_DRILL_MM: f64 = 0.1;
const MAX_DIAMETER_MM: f64 = 6.0;
/// Smallest annular ring the check accepts, in mm. Like
/// [`super::CLEARANCE_MM`], an estimated prototype default until a rules
/// document replaces it, not a fabricator qualification.
pub const MIN_ANNULAR_RING_MM: f64 = 0.1;
/// Smallest gap from a via's drilled hole to another net's copper on any
/// layer, in mm (estimated prototype default).
pub const DRILL_TO_COPPER_MM: f64 = 0.25;
/// Smallest gap between two vias' drilled holes, any nets, in mm
/// (estimated prototype default).
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

/// Whether a via is closer than the clearance rule to another net's pad:
/// the via side of a revision impact plan.
pub fn via_pad_clearance_conflict(via: &Via, pad: &WorldPad, layers: &[CopperLayer]) -> bool {
    via.net != pad.net
        && shares_layer(via, pad, layers)
        && CopperRing::of_via(via).violates_clearance(&CopperRing::of_pad(pad))
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
    let ring = CopperRing::of_via(via);
    let mut pairs = 0usize;
    for track in project.tracks.iter().filter(|t| t.net != via.net) {
        budget(&mut pairs)?;
        if via.on(track.layer, &layers) && ring.violates_clearance(&CopperRing::of_track(track)) {
            return Err(Error::Conflict(format!(
                "Too close to a track on {}: needs 0.2 mm",
                project.net_name(track.net)
            )));
        }
    }
    for pad in super::pads(project).iter().filter(|p| p.net != via.net) {
        budget(&mut pairs)?;
        if via_pad_clearance_conflict(via, pad, &layers) {
            return Err(Error::Conflict(format!(
                "Too close to a pad on {}: needs 0.2 mm",
                project.net_name(pad.net)
            )));
        }
    }
    for other in project.vias.iter().filter(|v| v.net != via.net) {
        budget(&mut pairs)?;
        if layers
            .iter()
            .any(|l| via.on(*l, &layers) && other.on(*l, &layers))
            && ring.violates_clearance(&CopperRing::of_via(other))
        {
            return Err(Error::Conflict(format!(
                "Too close to a via on {}: needs 0.2 mm",
                project.net_name(other.net)
            )));
        }
    }
    Ok(())
}

/// The first via on another net whose clearance rule the track breaks:
/// the via side of routing's clearance check.
pub(super) fn track_violates_via<'a>(
    project: &'a Project,
    track: &Track,
    ring: &CopperRing,
) -> Option<&'a Via> {
    let layers = project.stackup.layers();
    project.vias.iter().find(|via| {
        via.net != track.net
            && via.on(track.layer, &layers)
            && ring.violates_clearance(&CopperRing::of_via(via))
    })
}

/// The via manufacturing checks over `copper`'s vias: annular ring, drilled
/// hole to another net's copper on any layer, and hole to hole. Copper
/// clearance and connectivity through vias are part of
/// [`BoardCopper::checks`].
pub(super) fn findings(copper: &BoardCopper, project: &Project) -> Vec<Finding> {
    let tracks = &project.tracks;
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
    for (i, (via, drill)) in vias.iter().zip(&drills).enumerate() {
        let me = ObjectRef::Via { id: via.id };
        if via.annular_ring() < MIN_ANNULAR_RING_MM - EPS {
            issues.push(Finding::new(
                "board.via_annular_ring",
                format!(
                    "A via on {}: annular ring {:.3} mm is under {MIN_ANNULAR_RING_MM} mm",
                    project.net_name(via.net),
                    via.annular_ring()
                ),
                vec![me],
            ));
        }
        let pads = copper
            .pads()
            .iter()
            .enumerate()
            .filter(|(_, p)| p.net != via.net)
            .filter_map(|(k, p)| {
                Some((
                    copper.ring(k)?,
                    ObjectRef::Pad {
                        instance: p.instance,
                        pad: p.pad,
                    },
                    "pad",
                    p.net,
                ))
            });
        let others = vias
            .iter()
            .enumerate()
            .filter(|(_, v)| v.net != via.net)
            .filter_map(|(k, v)| {
                Some((
                    copper.via_ring(k)?,
                    ObjectRef::Via { id: v.id },
                    "via",
                    v.net,
                ))
            });
        let wires = tracks
            .iter()
            .zip(&track_rings)
            .filter(|(t, _)| t.net != via.net)
            .map(|(t, ring)| (ring, ObjectRef::Track { id: t.id }, "track", t.net));
        for (ring, object, kind, net) in pads.chain(others).chain(wires) {
            if over(&mut pairs, &mut issues) {
                return issues;
            }
            if drill.closer_than(ring, DRILL_TO_COPPER_MM) {
                issues.push(Finding::new(
                    "board.via_drill_clearance",
                    format!(
                        "A via on {}: drill is within {DRILL_TO_COPPER_MM} mm of a {kind} on {}",
                        project.net_name(via.net),
                        project.net_name(net)
                    ),
                    vec![me, object],
                ));
            }
        }
        for (other, other_drill) in vias.iter().zip(&drills).skip(i + 1) {
            if over(&mut pairs, &mut issues) {
                return issues;
            }
            if drill.closer_than(other_drill, HOLE_TO_HOLE_MM) {
                issues.push(Finding::new(
                    "board.via_hole_spacing",
                    format!(
                        "Vias on {} and {}: drills are closer than {HOLE_TO_HOLE_MM} mm",
                        project.net_name(via.net),
                        project.net_name(other.net)
                    ),
                    vec![me, ObjectRef::Via { id: other.id }],
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
        return invalid(&format!("prototype supports at most {MAX_VIAS} vias"));
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
