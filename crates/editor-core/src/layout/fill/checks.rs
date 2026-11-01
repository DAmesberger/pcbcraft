//! Fill and keepout checks, and the connectivity fills add to the board.
//!
//! Fresh fill copper meets its clearances by construction, so the
//! clearance check measures only stale copper (copper whose inputs moved
//! on and that has not been refilled yet).
use super::FILL_CHORD_MM;
use super::compute::{Ctx, pad_on};
use super::geom::{self, Region, RegionIndex};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Point, polygons_overlap};
use crate::ids::Id;
use crate::model::fill::{FillJoin, IslandRemoval};
use crate::model::{Fill, Project};
use crate::rules::{CopperObject, PadConnection};
use std::collections::BTreeMap;

/// How a fill is named in messages: its name, else its net and layer.
pub fn fill_label(project: &Project, fill: &Fill) -> String {
    if !fill.name.is_empty() {
        return format!("fill {}", fill.name);
    }
    let net = fill.net.map_or("no net", |n| project.net_name(n));
    format!("{net} fill on {}", fill.layer.name())
}

fn regions_of(fill: &Fill) -> Vec<Region> {
    fill.computed
        .iter()
        .flat_map(|c| &c.regions)
        .map(|r| {
            let mut region = vec![r.outer.clone()];
            region.extend(r.holes.iter().cloned());
            region
        })
        .collect()
}

/// Groups of copper objects each fill island connects: every pad, via and
/// track joined to one island. The board connectivity unions each group.
pub(crate) fn links(project: &Project) -> Vec<Vec<ObjectRef>> {
    let mut out = Vec::new();
    for fill in &project.fills {
        let Some(copper) = &fill.computed else {
            continue;
        };
        let mut groups: BTreeMap<u32, Vec<ObjectRef>> = BTreeMap::new();
        for join in &copper.joins {
            let Some(region) = join.joined_region() else {
                continue;
            };
            let object = match *join {
                FillJoin::Pad { instance, pad, .. } => ObjectRef::Pad { instance, pad },
                FillJoin::Via { via, .. } => ObjectRef::Via { id: via },
                FillJoin::Track { track, .. } => ObjectRef::Track { id: track },
            };
            groups.entry(region).or_default().push(object);
        }
        out.extend(groups.into_values());
    }
    out
}

/// Fill and keepout findings.
pub fn findings(project: &Project) -> Vec<Finding> {
    let mut out = Vec::new();
    if project.fills.is_empty() && project.keepouts.is_empty() {
        return out;
    }
    let mut ctx = Ctx::new(project);
    let labels = super::super::pad_labels(project);
    let pad_name = |instance: Id, pad| {
        format!(
            "{} pad {}",
            project
                .instance(instance)
                .map_or("component", |i| i.designator.as_str()),
            labels
                .get(&(instance, pad))
                .cloned()
                .unwrap_or_else(|| pad.to_string())
        )
    };
    for fill in &project.fills {
        let me = ObjectRef::Fill { id: fill.id };
        let label = fill_label(project, fill);
        if let Some(net) = fill.net
            && project.net(net).is_none()
        {
            out.push(Finding::new(
                "fill.net_missing",
                format!("{label} names a net that no longer exists"),
                vec![me],
            ));
        }
        let key = ctx.key(fill.id).ok();
        let Some(copper) = &fill.computed else {
            out.push(Finding::new(
                "fill.stale",
                format!("{label} is not filled yet"),
                vec![me],
            ));
            continue;
        };
        let stale = key.as_ref() != Some(&copper.key);
        if stale {
            out.push(Finding::new(
                "fill.stale",
                format!("{label} needs refilling: the board changed"),
                vec![me],
            ));
        }
        if copper.regions.is_empty() {
            out.push(Finding::new(
                "fill.empty",
                format!("{label} has no copper"),
                vec![me],
            ));
        }
        let wanted = fill
            .settings
            .thermal
            .unwrap_or(ctx.rules.fill.thermal)
            .spoke_count;
        for join in &copper.joins {
            let (object, connection, spokes, name) = match *join {
                FillJoin::Pad {
                    instance,
                    pad,
                    connection,
                    spokes,
                    ..
                } => (
                    ObjectRef::Pad { instance, pad },
                    connection,
                    spokes,
                    pad_name(instance, pad),
                ),
                FillJoin::Via {
                    via,
                    connection,
                    spokes,
                    ..
                } => (
                    ObjectRef::Via { id: via },
                    connection,
                    spokes,
                    format!("a via on {}", fill.net.map_or("", |n| project.net_name(n))),
                ),
                FillJoin::Track { .. } => continue,
            };
            if connection == PadConnection::Thermal && spokes < wanted {
                out.push(Finding::new(
                    "fill.thermal_starved",
                    format!("Starved thermal: {name} has {spokes} of {wanted} spokes in {label}"),
                    vec![object, me],
                ));
            }
        }
        if fill.settings.islands == IslandRemoval::Never && fill.net.is_some() {
            let mut joined = vec![false; copper.regions.len()];
            for j in &copper.joins {
                if let Some(flag) = j.joined_region().and_then(|r| joined.get_mut(r as usize)) {
                    *flag = true;
                }
            }
            let dead = joined.iter().filter(|j| !**j).count();
            if dead > 0 {
                out.push(Finding::new(
                    "fill.island_unconnected",
                    format!("{label} has {dead} unconnected copper islands"),
                    vec![me],
                ));
            }
        }
        if stale {
            clearance(&mut ctx, fill, &label, &mut out);
        }
    }
    keepouts(&ctx, &mut out);
    out
}

/// Clearance from a fill's stored copper to other-net copper on its layer.
fn clearance(ctx: &mut Ctx<'_>, fill: &Fill, label: &str, out: &mut Vec<Finding>) {
    let project = ctx.project;
    let regions = regions_of(fill);
    let Some(bounds) = geom::bounds(regions.iter().flatten()) else {
        return;
    };
    let index = RegionIndex::new(&regions);
    let me = ObjectRef::Fill { id: fill.id };
    let other = |net: Option<Id>| fill.net.is_none() || net != fill.net;
    // (object, outline ring, required gap)
    let mut objects: Vec<(ObjectRef, Vec<Point>, f64)> = Vec::new();
    for pad in &ctx.pads {
        if pad_on(pad, fill.layer) && other(pad.net) {
            let gap = ctx.required(fill, CopperObject::Pad, pad.net);
            objects.push((
                ObjectRef::Pad {
                    instance: pad.instance,
                    pad: pad.pad.id,
                },
                pad.outline().flatten(FILL_CHORD_MM),
                gap,
            ));
        }
    }
    for v in &project.vias {
        if v.on(fill.layer, &ctx.layers) && other(Some(v.net)) {
            let gap = ctx.required(fill, CopperObject::Via, Some(v.net));
            objects.push((
                ObjectRef::Via { id: v.id },
                geom::circle(v.position, v.diameter / 2.0, FILL_CHORD_MM),
                gap,
            ));
        }
    }
    for t in &project.tracks {
        if t.layer == fill.layer && other(Some(t.net)) {
            let gap = ctx.required(fill, CopperObject::Track, Some(t.net));
            objects.push((
                ObjectRef::Track { id: t.id },
                geom::stadium(t.start, t.end, t.width / 2.0, FILL_CHORD_MM),
                gap,
            ));
        }
    }
    let mut hits = 0usize;
    for (object, ring, gap) in objects {
        let Some(b) = geom::bounds([&ring]) else {
            continue;
        };
        if !geom::overlaps(bounds, b, gap) {
            continue;
        }
        let limit = gap - FILL_CHORD_MM;
        let n = ring.len();
        let inside = ring.first().and_then(|p| index.region_at(*p)).is_some();
        let near = inside
            || (0..n).any(|i| index.segment_distance(ring[i], ring[(i + 1) % n], limit) < limit);
        if near {
            hits += 1;
            if hits > 256 {
                break;
            }
            out.push(Finding::new(
                "fill.clearance",
                format!(
                    "Clearance < {gap:.3} mm: {label} (stale) / {}",
                    describe(project, &object)
                ),
                vec![me, object],
            ));
        }
    }
}

fn describe(project: &Project, object: &ObjectRef) -> String {
    match *object {
        ObjectRef::Track { id } => project.tracks.iter().find(|t| t.id == id).map_or_else(
            || "a track".into(),
            |t| format!("a track on {}", project.net_name(t.net)),
        ),
        ObjectRef::Via { id } => project.via(id).map_or_else(
            || "a via".into(),
            |v| format!("a via on {}", project.net_name(v.net)),
        ),
        ObjectRef::Pad { instance, .. } => format!(
            "a pad of {}",
            project
                .instance(instance)
                .map_or("component", |i| i.designator.as_str())
        ),
        _ => "copper".into(),
    }
}

/// Tracks, vias and pads inside keepouts that forbid them on their layer.
fn keepouts(ctx: &Ctx<'_>, out: &mut Vec<Finding>) {
    let project = ctx.project;
    for k in &project.keepouts {
        let Some(ring) = geom::ccw(k.outline.flatten(FILL_CHORD_MM)) else {
            continue;
        };
        let Some(kb) = geom::bounds([&ring]) else {
            continue;
        };
        let me = ObjectRef::Keepout { id: k.id };
        let name = if k.name.is_empty() {
            "a keepout".to_owned()
        } else {
            format!("keepout {}", k.name)
        };
        let inside = |other: &[Point]| {
            geom::bounds([&other.to_vec()]).is_some_and(|b| geom::overlaps(kb, b, 0.0))
                && polygons_overlap(&ring, other)
        };
        if k.forbid.tracks {
            for t in project.tracks.iter().filter(|t| k.on(t.layer)) {
                let copper = geom::stadium(t.start, t.end, t.width / 2.0, FILL_CHORD_MM);
                if inside(&copper) {
                    out.push(Finding::new(
                        "keepout.track",
                        format!("A track on {} enters {name}", project.net_name(t.net)),
                        vec![ObjectRef::Track { id: t.id }, me],
                    ));
                }
            }
        }
        if k.forbid.vias {
            for v in &project.vias {
                let on = ctx.layers.iter().any(|l| k.on(*l) && v.on(*l, &ctx.layers));
                if on && inside(&geom::circle(v.position, v.diameter / 2.0, FILL_CHORD_MM)) {
                    out.push(Finding::new(
                        "keepout.via",
                        format!("A via on {} lies in {name}", project.net_name(v.net)),
                        vec![ObjectRef::Via { id: v.id }, me],
                    ));
                }
            }
        }
        if k.forbid.pads {
            for pad in &ctx.pads {
                let on = ctx.layers.iter().any(|l| k.on(*l) && pad_on(pad, *l));
                if on && inside(&pad.outline().flatten(FILL_CHORD_MM)) {
                    out.push(Finding::new(
                        "keepout.pad",
                        format!(
                            "A pad of {} lies in {name}",
                            project
                                .instance(pad.instance)
                                .map_or("component", |i| i.designator.as_str())
                        ),
                        vec![
                            ObjectRef::Pad {
                                instance: pad.instance,
                                pad: pad.pad.id,
                            },
                            me,
                        ],
                    ));
                }
            }
        }
    }
}
