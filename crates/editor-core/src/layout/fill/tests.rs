//! Copper fill computation, connectivity, checks, caching and scale.
//! Expected values come from the rules and the original showcase board,
//! measured independently of the fill code (exact segment distances).
use super::{FILL_CHORD_MM, FillCache, FillCommand, RefillTarget, compute_stale, inputs_key};
use crate::diagnostics::ObjectRef;
use crate::geometry::{Point, segment_distance, segments_cross};
use crate::ids::Id;
use crate::layout::{self, placed_pads};
use crate::library::{Outline, Segment};
use crate::model::fill::{
    FillSettings, FillSpec, Hatch, IslandRemoval, KeepoutLayers, KeepoutRules, KeepoutSpec,
    PadConnections,
};
use crate::model::{CopperLayer, Fill, Project};
use crate::rules::{CopperObject, NetClassSpec, PadConnection, RuleKey, RuleValue, RulesCommand};
use crate::{Engine, Operation, Outcome, Query, QueryResult, format};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The showcase board without its pours: the expected values here were
/// measured on the board the fills are added to, so the showcase's own
/// bottom pour and ground plane are taken out first.
fn showcase() -> Fallible<Engine> {
    let mut file = Engine::showcase()?.to_file();
    file.project.fills.clear();
    Ok(Engine::from_file(file)?)
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Outline {
    Outline {
        start: [x0, y0],
        segments: vec![
            Segment::Line { to: [x1, y0] },
            Segment::Line { to: [x1, y1] },
            Segment::Line { to: [x0, y1] },
            Segment::Line { to: [x0, y0] },
        ],
    }
}

fn net(engine: &Engine, name: &str) -> Fallible<Id> {
    Ok(engine
        .project()
        .nets
        .iter()
        .find(|n| n.name == name)
        .map(|n| n.id)
        .ok_or(format!("no net {name}"))?)
}

fn spec(net: Option<Id>, layer: CopperLayer, outline: Outline) -> FillSpec {
    FillSpec {
        region: crate::model::fill::FillRegion::Drawn,
        name: String::new(),
        net,
        layer,
        outline,
        priority: 0,
        settings: FillSettings::default(),
    }
}

fn add(engine: &mut Engine, spec: FillSpec) -> Fallible<Id> {
    match engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Add {
            fill: Box::new(spec),
        }),
    )? {
        Outcome::FillAdded { fill } => Ok(fill),
        other => Err(format!("unexpected {other:?}").into()),
    }
}

fn refill(engine: &mut Engine) -> Fallible<()> {
    engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Refill {
            target: RefillTarget::All,
        }),
    )?;
    Ok(())
}

fn fill(project: &Project, id: Id) -> Fallible<&Fill> {
    Ok(project.fill(id).ok_or("no fill")?)
}

/// The board copper the copper graph joins to an island of fill `id`.
fn joined_to(project: &Project, id: Id) -> Vec<ObjectRef> {
    use layout::connectivity::{CopperGraph, EdgeKind, ElementKind};
    let graph = CopperGraph::of(project);
    let kind = |v: usize| {
        let e = graph.vertices().get(v)?.element;
        Some(graph.elements().get(e)?.kind)
    };
    let mut out = Vec::new();
    for e in graph.edges().iter().filter(|e| e.kind == EdgeKind::Joint) {
        let (Some(a), Some(b)) = (kind(e.a), kind(e.b)) else {
            continue;
        };
        let other = match (a, b) {
            (ElementKind::Fill { fill, .. }, other) | (other, ElementKind::Fill { fill, .. })
                if fill == id =>
            {
                other
            }
            _ => continue,
        };
        let object = match other {
            ElementKind::Track { index } => project
                .tracks
                .get(index)
                .map(|t| ObjectRef::Track { id: t.id }),
            ElementKind::Via { index } => {
                project.vias.get(index).map(|v| ObjectRef::Via { id: v.id })
            }
            ElementKind::Pad { index } => graph.pads().get(index).map(|p| ObjectRef::Pad {
                instance: p.instance,
                pad: p.pad,
            }),
            ElementKind::Fill { .. } => None,
        };
        if let Some(o) = object
            && !out.contains(&o)
        {
            out.push(o);
        }
    }
    out
}

/// Whether fill copper lies at `radius` from `center` in direction `deg`.
fn copper_towards(f: &Fill, center: Point, radius: f64, deg: f64) -> bool {
    let (s, c) = deg.to_radians().sin_cos();
    inside(f, [center[0] + c * radius, center[1] + s * radius])
}

/// The relief spokes of `relief` around a via that carry fill copper,
/// probed halfway across the relief gap, and whether the gap between
/// every pair of neighbouring spokes is clear of copper.
fn via_relief(
    f: &Fill,
    via: &crate::model::Via,
    relief: crate::rules::ThermalRelief,
) -> (usize, bool) {
    let r = via.diameter / 2.0 + relief.gap / 2.0;
    let n = relief.spoke_count.max(1);
    let step = 360.0 / f64::from(n);
    let angle = |k: u32| relief.angle_deg + step * f64::from(k);
    let spokes = (0..n)
        .filter(|k| copper_towards(f, via.position, r, angle(*k)))
        .count();
    let gaps = (0..n).all(|k| !copper_towards(f, via.position, r, angle(k) + step / 2.0));
    (spokes, gaps)
}

/// A whole-board GND fill on `layer` of the showcase.
fn showcase_with(layer: CopperLayer) -> Fallible<(Engine, Id, Id)> {
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    let id = add(
        &mut engine,
        spec(Some(gnd), layer, rect(0.0, 0.0, 44.0, 28.0)),
    )?;
    refill(&mut engine)?;
    Ok((engine, id, gnd))
}

fn edges(fill: &Fill) -> Vec<(Point, Point)> {
    let mut out = Vec::new();
    for r in fill.computed.iter().flat_map(|c| &c.regions) {
        for ring in std::iter::once(&r.outer).chain(&r.holes) {
            for i in 0..ring.len() {
                out.push((ring[i], ring[(i + 1) % ring.len()]));
            }
        }
    }
    out
}

fn inside(fill: &Fill, p: Point) -> bool {
    fill.computed.iter().flat_map(|c| &c.regions).any(|r| {
        crate::geometry::polygon_contains(&r.outer, p)
            && !r
                .holes
                .iter()
                .any(|h| crate::geometry::polygon_contains(h, p))
    })
}

/// The exact gap from fill copper to segment `a`–`b` (0 when they cross or
/// the segment starts inside copper).
fn gap_to_segment(fill: &Fill, a: Point, b: Point) -> f64 {
    if inside(fill, a) {
        return 0.0;
    }
    edges(fill)
        .into_iter()
        .map(|(c, d)| {
            if segments_cross(a, b, c, d) {
                0.0
            } else {
                segment_distance(a, c, d)
                    .min(segment_distance(b, c, d))
                    .min(segment_distance(c, a, b))
                    .min(segment_distance(d, a, b))
            }
        })
        .fold(f64::INFINITY, f64::min)
}

/// Every other-net pad, via and track on the fill's layer keeps at least
/// `rule` from its copper (measured on their exact outlines).
fn assert_clear(project: &Project, f: &Fill, rule: impl Fn(CopperObject) -> f64) {
    let layers = project.stackup.layers();
    for v in &project.vias {
        if Some(v.net) != f.net && v.on(f.layer, &layers) {
            let gap = gap_to_segment(f, v.position, v.position) - v.diameter / 2.0;
            assert!(gap >= rule(CopperObject::Via) - 1e-6, "via {}: {gap}", v.id);
        }
    }
    for t in &project.tracks {
        if Some(t.net) != f.net && t.layer == f.layer {
            let gap = gap_to_segment(f, t.start, t.end) - t.width / 2.0;
            assert!(
                gap >= rule(CopperObject::Track) - 1e-6,
                "track {}: {gap}",
                t.id
            );
        }
    }
    for pad in placed_pads(project) {
        if pad.net != f.net && pad.on_copper(f.layer) {
            // The flattened outline is inscribed in the true one, so the
            // true gap is at least this less the chord tolerance.
            let ring = pad.outline().flatten(FILL_CHORD_MM / 10.0);
            let gap = (0..ring.len())
                .map(|i| gap_to_segment(f, ring[i], ring[(i + 1) % ring.len()]))
                .fold(f64::INFINITY, f64::min);
            assert!(
                gap >= rule(CopperObject::Pad) - FILL_CHORD_MM / 10.0 - 1e-6,
                "pad {:?}: {gap}",
                pad.pad.id
            );
        }
    }
}

#[test]
fn a_ground_plane_keeps_the_rule_clearance_to_every_other_net() -> Fallible<()> {
    let (engine, id, _) = showcase_with(CopperLayer::Bottom)?;
    let project = engine.project();
    let f = fill(project, id)?;
    let copper = f.computed.as_ref().ok_or("not filled")?;
    assert!(!copper.regions.is_empty());
    assert_clear(project, f, |_| 0.2);
    // The board edge clearance is 0 by default: copper reaches the edge
    // but never leaves the board.
    for (a, _) in edges(f) {
        assert!(a[0] >= -1e-6 && a[1] >= -1e-6 && a[0] <= 44.0 + 1e-6 && a[1] <= 28.0 + 1e-6);
    }
    // Deterministic: computing again gives identical copper.
    assert_eq!(&super::compute(project, id)?, copper);
    Ok(())
}

#[test]
fn net_class_and_rule_clearances_widen_the_gap() -> Fallible<()> {
    let (mut engine, id, gnd) = showcase_with(CopperLayer::Bottom)?;
    let class = match engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "Ground".into(),
                clearance: Some(0.5),
                ..Default::default()
            },
        }),
    )? {
        Outcome::NetClassAdded { class } => class,
        other => return Err(format!("{other:?}").into()),
    };
    let before = inputs_key(engine.project(), id)?;
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AssignNets {
            nets: vec![gnd],
            class: Some(class),
        }),
    )?;
    assert_ne!(
        inputs_key(engine.project(), id)?,
        before,
        "the class makes it stale"
    );
    refill(&mut engine)?;
    assert_clear(engine.project(), fill(engine.project(), id)?, |_| 0.5);

    // A larger fill-to-track rule applies to tracks only.
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::SetOverride {
            key: RuleKey::Clearance {
                a: CopperObject::Fill,
                b: CopperObject::Track,
            },
            value: RuleValue::Length(0.8),
        }),
    )?;
    refill(&mut engine)?;
    let p = engine.project();
    assert_clear(p, fill(p, id)?, |o| {
        if o == CopperObject::Track { 0.8 } else { 0.5 }
    });
    Ok(())
}

#[test]
fn pads_vias_and_tracks_of_the_net_join_the_fill_with_their_connection_style() -> Fallible<()> {
    let (mut engine, id, gnd) = showcase_with(CopperLayer::Bottom)?;
    let p = engine.project();
    fill(p, id)?.computed.as_ref().ok_or("not filled")?;
    // Default: thermal reliefs with four spokes; every GND via joins.
    let joined = joined_to(p, id);
    let gnd_vias: Vec<_> = p.vias.iter().filter(|v| v.net == gnd).collect();
    assert!(
        gnd_vias
            .iter()
            .all(|v| joined.contains(&ObjectRef::Via { id: v.id })),
        "{joined:?}"
    );
    assert!(
        p.tracks
            .iter()
            .any(|t| t.net == gnd && joined.contains(&ObjectRef::Track { id: t.id }))
    );
    // A thermal relief leaves copper only in its spokes around a via: some
    // via has all four spokes, and no via has copper between its spokes.
    let f = fill(p, id)?;
    let t = p.rules.effective().fill.thermal;
    let reliefs: Vec<(usize, bool)> = gnd_vias.iter().map(|v| via_relief(f, v, t)).collect();
    assert!(reliefs.iter().any(|(s, _)| *s == 4), "{reliefs:?}");
    assert!(reliefs.iter().all(|(_, gaps)| *gaps), "{reliefs:?}");
    // The header's ground pin (through-hole) joins with spokes: copper
    // halfway across the relief gap along a spoke, none between spokes.
    let header = placed_pads(p)
        .into_iter()
        .find(|pad| pad.net == Some(gnd) && pad.pad.kind == crate::library::PadKind::PlatedThrough)
        .ok_or("no through-hole ground pin")?;
    assert!(joined.contains(&ObjectRef::Pad {
        instance: header.instance,
        pad: header.pad.id,
    }));
    let frame = header.pad_transform();
    let reach = header.pad.shape.extent();
    let centre = frame.apply([0.0, 0.0]);
    let outline = header.outline().flatten(0.001);
    // The distance from the centre to the pad's edge in direction `deg`.
    let edge = |deg: f64| {
        let (s, c) = deg.to_radians().sin_cos();
        let mut d = 0.0;
        while d < reach[0].hypot(reach[1])
            && crate::geometry::polygon_contains(&outline, [centre[0] + c * d, centre[1] + s * d])
        {
            d += 0.001;
        }
        d
    };
    let spoke = frame.rotation_deg + t.angle_deg;
    let at = |deg: f64| copper_towards(f, centre, edge(deg) + t.gap / 2.0, deg);
    let spokes = (0..t.spoke_count)
        .filter(|k| at(spoke + 360.0 * f64::from(*k) / f64::from(t.spoke_count)))
        .count();
    assert!(spokes >= 1, "the header pin has no spoke");
    let between = 180.0 / f64::from(t.spoke_count);
    assert!(
        (0..t.spoke_count)
            .all(|k| !at(spoke + between + 360.0 * f64::from(k) / f64::from(t.spoke_count))),
        "copper between the header pin's spokes"
    );
    // Every island of the fill joins copper of its net.
    assert!(
        !layout::checks(p)
            .iter()
            .any(|f| f.code == "fill.island_unconnected")
    );
    let via = (*gnd_vias.first().ok_or("no via")?).clone();

    // Solid vias flood; "none" clears them like another net and joins nothing.
    let mut edited = fill(engine.project(), id)?.clone();
    edited.settings.connections = PadConnections {
        via: Some(PadConnection::Solid),
        ..Default::default()
    };
    edit(&mut engine, edited.clone())?;
    let p = engine.project();
    assert!(inside(fill(p, id)?, via.position) || via.drill > 0.0);
    let ring = via.diameter / 2.0 + 0.01;
    assert!(inside(
        fill(p, id)?,
        [via.position[0] + ring, via.position[1]]
    ));
    edited.settings.connections.via = Some(PadConnection::None);
    edit(&mut engine, edited)?;
    let p = engine.project();
    let f = fill(p, id)?;
    assert!(
        !joined_to(p, id)
            .iter()
            .any(|o| matches!(o, ObjectRef::Via { .. }))
    );
    assert_clear(p, f, |_| 0.2);
    let gap = gap_to_segment(f, via.position, via.position) - via.diameter / 2.0;
    assert!(gap >= 0.2 - 1e-6, "{gap}");
    Ok(())
}

fn edit(engine: &mut Engine, fill: Fill) -> Fallible<()> {
    engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Edit {
            fill: Box::new(fill),
        }),
    )?;
    refill(engine)
}

#[test]
fn a_plane_connects_what_its_track_used_to() -> Fallible<()> {
    let engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    let strap = engine
        .project()
        .tracks
        .iter()
        .find(|t| t.layer == CopperLayer::Inner(1) && t.net == gnd)
        .map(|t| t.id)
        .ok_or("no strap")?;
    // Without the strap (and the bottom ground tracks joining the two
    // vias) the ground net splits.
    let bottom: Vec<Id> = engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.net == gnd && t.layer != CopperLayer::Top)
        .map(|t| t.id)
        .collect();
    let mut project = engine.project().clone();
    project
        .tracks
        .retain(|t| t.id != strap && !bottom.contains(&t.id));
    let split = layout::airwires(&project).len();
    assert!(split > 0, "removing ground tracks leaves airwires");
    // An inner ground plane joins them again, through the vias.
    let id = project.allocate()?;
    project.fills.push(Fill::from_spec(
        id,
        spec(Some(gnd), CopperLayer::Inner(1), rect(0.0, 0.0, 44.0, 28.0)),
    ));
    let copper = super::compute(&project, id)?;
    project.fills.retain(|f| f.id != id);
    let mut filled = project.clone();
    filled.fills.push(Fill {
        computed: Some(copper),
        ..Fill::from_spec(
            id,
            spec(Some(gnd), CopperLayer::Inner(1), rect(0.0, 0.0, 44.0, 28.0)),
        )
    });
    let after = layout::airwires(&filled).len();
    assert!(after < split, "{after} of {split}");
    let unrouted = |p: &Project| {
        layout::checks(p)
            .into_iter()
            .filter(|f| {
                f.code == "board.unrouted" && f.objects.contains(&ObjectRef::Net { id: gnd })
            })
            .count()
    };
    assert!(unrouted(&filled) < unrouted(&project));
    // Vias joined only by the plane are not dangling.
    assert!(
        layout::checks(&filled)
            .iter()
            .filter(|f| f.code == "board.via_dangling")
            .all(|f| !f.objects.contains(&ObjectRef::Net { id: gnd }))
    );
    // A joined via nudged (its tracks still reach it) makes the plane
    // stale: its joins stop counting at once, so the airwires the plane
    // closed are back before any refill.
    let mut moved = filled.clone();
    let via = moved
        .vias
        .iter_mut()
        .find(|v| v.net == gnd && v.position == [22.7, 18.5])
        .ok_or("strap via")?;
    via.position[0] += 0.02;
    assert!(super::is_stale(&moved, &moved.fills[moved.fills.len() - 1]));
    assert!(joined_to(&moved, id).is_empty(), "stale joins are dropped");
    assert!(!joined_to(&filled, id).is_empty());
    assert_eq!(layout::airwires(&moved).len(), split);
    Ok(())
}

#[test]
fn higher_priority_fills_claim_overlap_and_lower_ones_keep_clear() -> Fallible<()> {
    let mut engine = showcase()?;
    let (gnd, vcc) = (net(&engine, "GND")?, net(&engine, "VCC")?);
    let low = add(
        &mut engine,
        spec(Some(gnd), CopperLayer::Inner(2), rect(0.0, 0.0, 44.0, 28.0)),
    )?;
    let mut high = spec(
        Some(vcc),
        CopperLayer::Inner(2),
        rect(10.0, 5.0, 30.0, 20.0),
    );
    high.priority = 2;
    let high = add(&mut engine, high)?;
    refill(&mut engine)?;
    let p = engine.project();
    let (fl, fh) = (fill(p, low)?, fill(p, high)?);
    assert!(inside(fh, [20.5, 6.0]), "the higher fill holds its area");
    assert!(!inside(fl, [20.5, 6.0]));
    // The lower fill keeps the fill-to-fill clearance from the higher one.
    let gap = edges(fh)
        .into_iter()
        .map(|(a, b)| gap_to_segment(fl, a, b))
        .fold(f64::INFINITY, f64::min);
    assert!(gap >= 0.2 - 1e-6, "{gap}");
    // Changing only the higher fill makes the lower one stale too.
    let key = inputs_key(p, low)?;
    let mut moved = fh.clone();
    moved.outline = rect(12.0, 5.0, 30.0, 20.0);
    engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Edit {
            fill: Box::new(moved),
        }),
    )?;
    assert_ne!(inputs_key(engine.project(), low)?, key);
    Ok(())
}

#[test]
fn islands_without_a_connection_are_removed_unless_kept() -> Fallible<()> {
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    // A corner of the top layer away from every ground object.
    let mut s = spec(Some(gnd), CopperLayer::Top, rect(40.0, 24.0, 44.0, 28.0));
    let id = add(&mut engine, s.clone())?;
    refill(&mut engine)?;
    let p = engine.project();
    let c = fill(p, id)?.computed.clone().ok_or("not filled")?;
    assert!(c.regions.is_empty() && c.removed_islands == 1, "{c:?}");
    assert!(layout::checks(p).iter().any(|f| f.code == "fill.empty"));
    // Kept on request, and then reported.
    s.settings.islands = IslandRemoval::Never;
    let kept = add(&mut engine, s)?;
    refill(&mut engine)?;
    let p = engine.project();
    assert_eq!(
        fill(p, kept)?.computed.as_ref().map(|c| c.regions.len()),
        Some(1)
    );
    assert!(
        layout::checks(p)
            .iter()
            .any(|f| f.code == "fill.island_unconnected"
                && f.objects.contains(&ObjectRef::Fill { id: kept }))
    );
    // A fill with no net keeps its islands.
    let none = add(
        &mut engine,
        spec(None, CopperLayer::Top, rect(40.0, 0.0, 44.0, 3.0)),
    )?;
    refill(&mut engine)?;
    assert!(
        !fill(engine.project(), none)?
            .computed
            .as_ref()
            .ok_or("not filled")?
            .regions
            .is_empty()
    );
    Ok(())
}

#[test]
fn copper_narrower_than_the_minimum_width_is_removed() -> Fallible<()> {
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    // A dumbbell: two pads of copper joined by a 0.15 mm neck.
    let outline = Outline {
        start: [36.0, 22.0],
        segments: vec![
            Segment::Line { to: [38.0, 22.0] },
            Segment::Line { to: [38.0, 24.0] },
            Segment::Line { to: [40.0, 24.0] },
            Segment::Line { to: [40.0, 22.0] },
            Segment::Line { to: [42.0, 22.0] },
            Segment::Line { to: [42.0, 24.15] },
            Segment::Line { to: [36.0, 24.15] },
            Segment::Line { to: [36.0, 22.0] },
        ],
    };
    let mut s = spec(Some(gnd), CopperLayer::Top, outline);
    s.settings.islands = IslandRemoval::Never;
    s.settings.width_min = Some(0.1);
    let id = add(&mut engine, s.clone())?;
    refill(&mut engine)?;
    let thin = fill(engine.project(), id)?.clone();
    assert_eq!(thin.computed.as_ref().map(|c| c.regions.len()), Some(1));
    // A minimum width above the neck splits it into two islands.
    let mut wide = thin.clone();
    wide.settings.width_min = Some(0.3);
    let _ = s;
    edit(&mut engine, wide)?;
    let c = fill(engine.project(), id)?
        .computed
        .clone()
        .ok_or("not filled")?;
    assert_eq!(c.regions.len(), 2, "{:?}", c.regions.len());
    Ok(())
}

#[test]
fn keepouts_clear_fill_copper_and_flag_tracks() -> Fallible<()> {
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    let id = add(
        &mut engine,
        spec(Some(gnd), CopperLayer::Bottom, rect(0.0, 0.0, 44.0, 28.0)),
    )?;
    let Outcome::KeepoutAdded { keepout } = engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::AddKeepout {
            keepout: Box::new(KeepoutSpec {
                name: "Antenna".into(),
                outline: rect(38.0, 20.0, 44.0, 28.0),
                layers: KeepoutLayers::Only(vec![CopperLayer::Bottom]),
                forbid: KeepoutRules::default(),
            }),
        }),
    )?
    else {
        return Err("no keepout".into());
    };
    refill(&mut engine)?;
    let p = engine.project();
    assert!(!inside(fill(p, id)?, [41.0, 24.0]));
    assert!(inside(fill(p, id)?, [41.0, 18.0]) || inside(fill(p, id)?, [3.0, 26.0]));
    // A track through the keepout on its layer is a finding.
    let mut q = p.clone();
    let track = q.allocate()?;
    q.tracks.push(crate::model::Track {
        id: track,
        net: gnd,
        start: [37.0, 25.0],
        end: [43.0, 25.0],
        width: 0.3,
        layer: CopperLayer::Bottom,
        locked: false,
    });
    let findings = layout::checks(&q);
    assert!(
        findings.iter().any(|f| f.code == "keepout.track"
            && f.objects.contains(&ObjectRef::Keepout { id: keepout }))
    );
    // The new track makes the fill stale, and stale copper is measured.
    assert!(findings.iter().any(|f| f.code == "fill.stale"));
    Ok(())
}

#[test]
fn stale_copper_is_checked_for_clearance() -> Fallible<()> {
    let (engine, id, _) = showcase_with(CopperLayer::Bottom)?;
    let mut p = engine.project().clone();
    let vcc = p
        .nets
        .iter()
        .find(|n| n.name == "VCC")
        .map(|n| n.id)
        .ok_or("vcc")?;
    let t = p.allocate()?;
    p.tracks.push(crate::model::Track {
        id: t,
        net: vcc,
        start: [40.0, 25.0],
        end: [42.0, 25.0],
        width: 0.3,
        layer: CopperLayer::Bottom,
        locked: false,
    });
    assert!(super::is_stale(&p, fill(&p, id)?));
    let findings = layout::checks(&p);
    assert!(
        findings
            .iter()
            .any(|f| f.code == "fill.clearance" && f.objects.contains(&ObjectRef::Track { id: t })),
        "{findings:#?}"
    );
    Ok(())
}

#[test]
fn hatched_fills_keep_a_border_and_grid() -> Fallible<()> {
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    let solid = add(
        &mut engine,
        spec(Some(gnd), CopperLayer::Bottom, rect(0.0, 0.0, 44.0, 28.0)),
    )?;
    refill(&mut engine)?;
    let area = |f: &Fill| -> f64 {
        f.computed
            .iter()
            .flat_map(|c| &c.regions)
            .map(|r| {
                let mut region = vec![r.outer.clone()];
                region.extend(r.holes.iter().cloned());
                super::geom::region_area(&region)
            })
            .sum()
    };
    let full = area(fill(engine.project(), solid)?);
    let mut hatched = fill(engine.project(), solid)?.clone();
    hatched.settings.hatch = Some(Hatch {
        width: 0.3,
        gap: 0.7,
        angle_deg: 45.0,
    });
    edit(&mut engine, hatched)?;
    let h = area(fill(engine.project(), solid)?);
    assert!(h < full * 0.8 && h > full * 0.3, "{h} of {full}");
    assert_clear(engine.project(), fill(engine.project(), solid)?, |_| 0.2);
    Ok(())
}

/// `p`'s board as a solved sketch of `solved`.
fn with_solved_board(p: &mut Project, solved: crate::model::board::SolvedBoard) {
    use crate::library::footprint_source::{EvaluatorRef, SketchBlob, SolveSummary};
    use crate::model::board::{BoardShape, BoardSketch};
    let evaluator = EvaluatorRef {
        engine: "test".into(),
        rev: "0".into(),
    };
    p.board_shape = BoardShape::Sketch(Box::new(BoardSketch {
        evaluator: evaluator.clone(),
        sketch: SketchBlob("{}".into()),
        parameters: Vec::new(),
        cutouts: Vec::new(),
        holes: Vec::new(),
        edges: Vec::new(),
        next_feature: 3,
        solve: SolveSummary {
            evaluator,
            status: Default::default(),
            dof: 0,
            result_hash: String::new(),
        },
        solved,
        outline_fit: crate::rules::DEFAULT_OUTLINE_FIT_MM,
        fault: None,
        idle: Vec::new(),
    }));
}

#[test]
fn cutouts_mounting_holes_and_edge_clearance_shape_the_fill() -> Fallible<()> {
    use crate::model::board::{
        BoardFeatureId, CutoutKind, SolvedBoard, SolvedCutout, SolvedMountingHole,
    };
    let (engine, id, _) = showcase_with(CopperLayer::Bottom)?;
    let mut p = engine.project().clone();
    let mut solved = SolvedBoard::rectangle(p.board_size);
    solved.cutouts.push(SolvedCutout {
        id: Some(BoardFeatureId(1)),
        kind: CutoutKind::Cutout,
        outline: rect(39.0, 22.0, 42.0, 26.0),
        curves: Vec::new(),
    });
    solved.holes.push(SolvedMountingHole {
        id: BoardFeatureId(2),
        at: [3.0, 25.0],
        drill: 3.2,
        plated: false,
        pad_diameter: None,
    });
    with_solved_board(&mut p, solved);
    p.rules.overrides.push(crate::rules::RuleOverride {
        key: RuleKey::Clearance {
            a: CopperObject::Fill,
            b: CopperObject::BoardEdge,
        },
        value: RuleValue::Length(0.5),
    });
    let c = super::compute(&p, id)?;
    let mut f = fill(&p, id)?.clone();
    f.computed = Some(c);
    // Edge clearance: nothing within 0.5 mm of the outline or the cutout.
    for (a, _) in edges(&f) {
        assert!(a[0] >= 0.5 - 1e-6 && a[1] >= 0.5 - 1e-6);
        assert!(a[0] <= 43.5 + 1e-6 && a[1] <= 27.5 + 1e-6);
        // Distance to the cutout rectangle [39, 42] × [22, 26].
        let dx = (39.0 - a[0]).max(a[0] - 42.0).max(0.0);
        let dy = (22.0 - a[1]).max(a[1] - 26.0).max(0.0);
        assert!(dx.hypot(dy) >= 0.5 - 1e-6, "{a:?}");
    }
    assert!(!inside(&f, [40.5, 24.0]));
    // The mounting hole keeps the hole clearance (0.25 mm by default).
    let gap = gap_to_segment(&f, [3.0, 25.0], [3.0, 25.0]) - 1.6;
    assert!(gap >= 0.25 - 1e-6, "{gap}");
    Ok(())
}

/// A plated hole keeps the plated-hole clearance, an unplated one the via
/// hole clearance. Under the shipped JLCPCB example a fill clears an
/// unplated mounting hole by 0.2 mm ("NPTH to Track") and the same hole
/// plated by 0.28 mm ("PTH to Track"); one hole rule cleared both by 0.2.
#[test]
fn a_fill_keeps_the_plated_hole_clearance_from_a_plated_hole() -> Fallible<()> {
    use crate::model::board::{BoardFeatureId, SolvedBoard, SolvedMountingHole};
    let (engine, id, _) = showcase_with(CopperLayer::Bottom)?;
    let mut gaps = Vec::new();
    for plated in [false, true] {
        let mut p = engine.project().clone();
        crate::rules::apply(
            &mut p.rules,
            RulesCommand::SetProfile {
                profile: Box::new(crate::rules::fixtures::jlcpcb_example()?),
                keep_overrides: false,
            },
        )?;
        let mut solved = SolvedBoard::rectangle(p.board_size);
        solved.holes.push(SolvedMountingHole {
            id: BoardFeatureId(2),
            at: [3.0, 25.0],
            drill: 3.2,
            plated,
            pad_diameter: None,
        });
        with_solved_board(&mut p, solved);
        let mut f = fill(&p, id)?.clone();
        f.computed = Some(super::compute(&p, id)?);
        gaps.push(gap_to_segment(&f, [3.0, 25.0], [3.0, 25.0]) - 1.6);
    }
    let tolerance = FILL_CHORD_MM + 1e-6;
    let [unplated, plated] = gaps[..] else {
        return Err("two fills".into());
    };
    assert!(
        unplated >= 0.2 - 1e-6 && unplated < 0.28 - tolerance,
        "{unplated}"
    );
    assert!(plated >= 0.28 - 1e-6, "{plated}");
    Ok(())
}

/// Under each test profile and the shipped example, a whole-board fill
/// stays that profile's edge clearance inside the rectangle. Under the first default
/// (0) the same fill reached the edge and the production check passed.
#[test]
fn the_fill_keeps_the_profile_edge() -> Fallible<()> {
    use crate::rules::fixtures;
    for profile in [
        fixtures::prototype()?,
        fixtures::fine()?,
        fixtures::jlcpcb_example()?,
    ] {
        let edge = profile
            .rules()
            .copper
            .clearance
            .get(CopperObject::Fill, CopperObject::BoardEdge);
        assert!(edge > 0.0, "{}", profile.id);
        let (engine, id, _) = showcase_with(CopperLayer::Bottom)?;
        let mut p = engine.project().clone();
        let [w, h] = p.board_size;
        crate::rules::apply(
            &mut p.rules,
            crate::rules::RulesCommand::SetProfile {
                profile: Box::new(profile.clone()),
                keep_overrides: false,
            },
        )?;
        let c = super::compute(&p, id)?;
        let mut f = fill(&p, id)?.clone();
        f.computed = Some(c);
        assert!(!edges(&f).is_empty(), "{}", profile.id);
        for (a, _) in edges(&f) {
            let gap = a[0].min(a[1]).min(w - a[0]).min(h - a[1]);
            assert!(gap >= edge - 1e-6, "{}: {a:?} is {gap}", profile.id);
        }
    }
    Ok(())
}

#[test]
fn the_cache_key_ignores_edits_out_of_reach() -> Fallible<()> {
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    let id = add(
        &mut engine,
        spec(Some(gnd), CopperLayer::Top, rect(30.0, 20.0, 44.0, 28.0)),
    )?;
    refill(&mut engine)?;
    let p = engine.project();
    let key = inputs_key(p, id)?;
    let mut q = p.clone();
    // A track far away on the same layer, and one under the fill on
    // another layer: the key holds.
    let far = q.allocate()?;
    q.tracks.push(crate::model::Track {
        id: far,
        net: gnd,
        start: [1.0, 1.0],
        end: [3.0, 1.0],
        width: 0.3,
        layer: CopperLayer::Top,
        locked: false,
    });
    let under = q.allocate()?;
    q.tracks.push(crate::model::Track {
        id: under,
        net: gnd,
        start: [35.0, 25.0],
        end: [40.0, 25.0],
        width: 0.3,
        layer: CopperLayer::Bottom,
        locked: false,
    });
    assert_eq!(inputs_key(&q, id)?, key);
    assert!(!super::is_stale(&q, fill(&q, id)?));
    // One inside it on its layer changes it.
    if let Some(t) = q.tracks.iter_mut().find(|t| t.id == under) {
        t.layer = CopperLayer::Top;
    }
    assert_ne!(inputs_key(&q, id)?, key);
    Ok(())
}

#[test]
fn store_commits_only_current_copper_and_refill_is_one_undo_step() -> Fallible<()> {
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    let id = add(
        &mut engine,
        spec(Some(gnd), CopperLayer::Bottom, rect(0.0, 0.0, 44.0, 28.0)),
    )?;
    let copper = super::compute(engine.project(), id)?;
    // Stale copper is refused and leaves the document.
    let mut stale = copper.clone();
    stale.key = "0000000000000000".into();
    let rev = engine.revision();
    assert!(
        engine
            .apply(
                rev,
                Operation::Fill(FillCommand::Store {
                    fill: id,
                    copper: Box::new(stale),
                }),
            )
            .is_err()
    );
    assert_eq!(engine.revision(), rev);
    engine.apply(
        rev,
        Operation::Fill(FillCommand::Store {
            fill: id,
            copper: Box::new(copper.clone()),
        }),
    )?;
    assert_eq!(fill(engine.project(), id)?.computed.as_ref(), Some(&copper));
    engine.undo()?;
    assert!(fill(engine.project(), id)?.computed.is_none());
    // fill.status reports staleness.
    let QueryResult::FillStatus(status) = engine.query(Query::FillStatus {})? else {
        return Err("status".into());
    };
    assert!(status.iter().all(|s| s.stale));
    // A host computes off-thread and stores derived copper without an
    // undo step.
    let mut cache = FillCache::default();
    let results = compute_stale(engine.project(), &mut cache)?;
    assert_eq!(results.len(), 1);
    assert_eq!(cache.len(), 1);
    let undo_before = engine.can_undo();
    let stored = engine.store_fills(engine.revision(), results)?;
    assert_eq!(stored, 1);
    assert_eq!(engine.can_undo(), undo_before);
    assert!(compute_stale(engine.project(), &mut cache)?.is_empty());
    Ok(())
}

#[test]
fn computed_copper_persists_and_reloads_fresh() -> Fallible<()> {
    let (engine, id, _) = showcase_with(CopperLayer::Bottom)?;
    let text = serde_json::to_string(&engine.to_file())?;
    let file: format::EditorFile = serde_json::from_str(&text)?;
    let loaded = Engine::from_file(file)?;
    let f = fill(loaded.project(), id)?;
    assert_eq!(f, fill(engine.project(), id)?);
    assert!(!super::is_stale(loaded.project(), f));
    let mut cache = FillCache::default();
    assert!(compute_stale(loaded.project(), &mut cache)?.is_empty());
    Ok(())
}

/// A GND surface pad on the showcase's top layer, its centre and the fill
/// strip over it: tall enough to cover the pad, too thin for a via's four
/// spokes.
struct PadStrip {
    engine: Engine,
    gnd: Id,
    in_pad: Id,
    beside: Id,
    strip: Outline,
}

fn via_at(engine: &mut Engine, net: Id, at: Point) -> Fallible<Id> {
    match engine.apply(
        engine.revision(),
        Operation::Layout(layout::LayoutCommand::AddVia {
            via: layout::ViaRequest::fixture(net, at),
        }),
    )? {
        Outcome::ViaAdded { via } => Ok(via),
        other => Err(format!("{other:?}").into()),
    }
}

fn pad_strip() -> Fallible<PadStrip> {
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    // The widest room comes from a blank part of the board: a surface pad
    // placed there by hand would need a new footprint, so use a real one
    // and put the second via along the strip, outside the pad.
    let pad = placed_pads(engine.project())
        .into_iter()
        .find(|p| {
            p.net == Some(gnd)
                && p.pad.kind == crate::library::PadKind::Smd
                && CopperLayer::from(p.layer) == CopperLayer::Top
        })
        .ok_or("no GND surface pad")?;
    let center = pad.pad_transform().apply([0.0, 0.0]);
    let ring = pad.outline().flatten(FILL_CHORD_MM);
    let xs = ring.iter().map(|p| p[0]);
    let right = xs.clone().fold(f64::NEG_INFINITY, f64::max);
    let beside_at = [right + 1.0, center[1]];
    let in_pad = via_at(&mut engine, gnd, center)?;
    let beside = via_at(&mut engine, gnd, beside_at)?;
    let strip = rect(
        center[0] - 2.0,
        center[1] - 0.4,
        beside_at[0] + 2.0,
        center[1] + 0.4,
    );
    Ok(PadStrip {
        engine,
        gnd,
        in_pad,
        beside,
        strip,
    })
}

/// The strip fill on `layer` with the given connections: its vias thermal
/// with four spokes unless told otherwise.
fn strip_fill(
    fixture: &mut PadStrip,
    layer: CopperLayer,
    connections: PadConnections,
) -> Fallible<Id> {
    let mut s = spec(Some(fixture.gnd), layer, fixture.strip.clone());
    s.settings.connections = connections;
    s.settings.thermal = Some(STRIP_RELIEF);
    s.settings.width_min = Some(0.1);
    let id = add(&mut fixture.engine, s)?;
    refill(&mut fixture.engine)?;
    Ok(id)
}

/// Whether the copper graph joins via `via` to fill `fill_id`.
fn via_joined(project: &Project, fill_id: Id, via: Id) -> bool {
    joined_to(project, fill_id).contains(&ObjectRef::Via { id: via })
}

/// The strip fills' relief: four 0.3 mm spokes across a 0.25 mm gap.
const STRIP_RELIEF: crate::rules::ThermalRelief = crate::rules::ThermalRelief {
    spoke_width: 0.3,
    spoke_count: 4,
    gap: 0.25,
    angle_deg: 0.0,
};

/// The other half of gap 17: a fill can join vias solidly. It leaves no
/// relief to starve, and the vias still join the fill's copper.
#[test]
fn a_fill_can_join_its_vias_solidly() -> Fallible<()> {
    let mut f = pad_strip()?;
    let id = strip_fill(
        &mut f,
        CopperLayer::Top,
        PadConnections {
            smd: Some(PadConnection::Thermal),
            through_hole: None,
            via: Some(PadConnection::Solid),
        },
    )?;
    let p = f.engine.project();
    for via in [f.in_pad, f.beside] {
        assert!(via_joined(p, id, via), "via {via} joins the copper");
    }
    // Solid copper runs up to the via's drill: 0.05 mm off its barrel is
    // copper on the 0.6 mm pad's ring, where a thermal would have a gap.
    let v = p.via(f.beside).ok_or("via")?;
    let copper_at = [v.position[0], v.position[1] + v.drill / 2.0 + 0.05];
    assert!(inside(fill(p, id)?, copper_at));

    // Per fill: another fill's vias keep their thermals.
    let mut other = pad_strip()?;
    strip_fill(
        &mut other,
        CopperLayer::Top,
        PadConnections {
            smd: Some(PadConnection::Thermal),
            through_hole: None,
            via: Some(PadConnection::Thermal),
        },
    )?;
    let beside = other.beside;
    let other_id = other.engine.project().fills.last().ok_or("fill")?.id;
    let p = other.engine.project();
    let v = p.via(beside).ok_or("via")?;
    assert!(via_joined(p, other_id, beside));
    // A thermal relief: copper only along its spokes. The strip is 0.8 mm
    // tall, so only the two spokes along it reach copper.
    assert_eq!(
        via_relief(fill(p, other_id)?, v, STRIP_RELIEF),
        (2, true),
        "a thermal relief with its spokes along the strip"
    );
    Ok(())
}

/// The least gap from a fill's copper to tracks of the nets `pick` accepts.
fn track_gap(project: &Project, f: &Fill, pick: impl Fn(Id) -> bool) -> f64 {
    project
        .tracks
        .iter()
        .filter(|t| t.layer == f.layer && pick(t.net))
        .map(|t| gap_to_segment(f, t.start, t.end) - t.width / 2.0)
        .fold(f64::INFINITY, f64::min)
}

/// Gap 13 of the E2E log: a class's clearance to fills moves the pour away
/// from that class's copper, and leaves other nets and the class's tracks
/// and pads to their own clearances.
#[test]
fn a_class_fill_clearance_moves_the_pour_and_nothing_else() -> Fallible<()> {
    let (mut engine, id, gnd) = showcase_with(CopperLayer::Bottom)?;
    let project = engine.project();
    let chosen = project
        .tracks
        .iter()
        .find(|t| t.layer == CopperLayer::Bottom && t.net != gnd)
        .map(|t| t.net)
        .ok_or("no bottom track")?;
    let f = fill(project, id)?;
    let others_before = track_gap(project, f, |n| n != gnd && n != chosen);
    let tolerance = 0.03;
    assert!((track_gap(project, f, |n| n == chosen) - 0.2).abs() < tolerance);

    let class = match engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "USB".into(),
                fill_clearance: Some(0.5),
                ..Default::default()
            },
        }),
    )? {
        Outcome::NetClassAdded { class } => class,
        other => return Err(format!("{other:?}").into()),
    };
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AssignNets {
            nets: vec![chosen],
            class: Some(class),
        }),
    )?;
    refill(&mut engine)?;
    let project = engine.project();
    let f = fill(project, id)?;
    let gap = track_gap(project, f, |n| n == chosen);
    assert!(gap >= 0.5 - 1e-6, "{gap}");
    assert!(gap < 0.5 + tolerance, "{gap}");
    let others = track_gap(project, f, |n| n != gnd && n != chosen);
    assert!(
        (others - others_before).abs() < tolerance,
        "other nets keep their gap: {others} against {others_before}"
    );
    Ok(())
}

/// 10,000 through vias under a whole-board plane: the measured time of
/// one full fill is printed; the bound is generous for debug builds.
#[test]
fn a_large_board_fills_in_seconds() -> Fallible<()> {
    let mut project = showcase()?.project().clone();
    let gnd = project
        .nets
        .iter()
        .find(|n| n.name == "GND")
        .map(|n| n.id)
        .ok_or("gnd")?;
    let vcc = project
        .nets
        .iter()
        .find(|n| n.name == "VCC")
        .map(|n| n.id)
        .ok_or("vcc")?;
    project.board_size = [200.0, 200.0];
    project.placements.clear();
    project.tracks.clear();
    project.vias.clear();
    for i in 0..100 {
        for j in 0..100 {
            let id = project.allocate()?;
            project.vias.push(crate::model::Via {
                id,
                net: if (i + j) % 2 == 0 { gnd } else { vcc },
                position: [1.0 + 1.98 * f64::from(i), 1.0 + 1.98 * f64::from(j)],
                drill: 0.3,
                diameter: 0.6,
                span: crate::model::ViaSpan::Through,
                tented: true,
                locked: false,
            });
        }
    }
    let id = project.allocate()?;
    project.fills.push(Fill::from_spec(
        id,
        spec(Some(gnd), CopperLayer::Bottom, rect(0.0, 0.0, 200.0, 200.0)),
    ));
    let start = std::time::Instant::now();
    let copper = super::compute(&project, id)?;
    let full = start.elapsed();
    if let Some(f) = project.fills.iter_mut().find(|f| f.id == id) {
        f.computed = Some(copper.clone());
    }
    let joined = joined_to(&project, id)
        .iter()
        .filter(|o| matches!(o, ObjectRef::Via { .. }))
        .count();
    assert_eq!(joined, 5000);
    // Inputs keys of every fill are cheap enough to compute per revision.
    let start = std::time::Instant::now();
    let _ = inputs_key(&project, id)?;
    let key = start.elapsed();
    println!(
        "10k vias: fill {full:?}, key {key:?}, {} regions",
        copper.regions.len()
    );
    assert!(full.as_secs_f64() < 30.0, "{full:?}");
    Ok(())
}

/// `fill.store_all` commits a host's off-thread refill of several fills as
/// one undo step, and refuses the whole batch (storing nothing) when one
/// fill's copper is stale or a fill is gone.
#[test]
fn store_all_is_one_atomic_undo_step() -> Fallible<()> {
    use super::StoredFill;
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    let a = add(
        &mut engine,
        spec(Some(gnd), CopperLayer::Bottom, rect(0.0, 0.0, 44.0, 28.0)),
    )?;
    let b = add(
        &mut engine,
        spec(Some(gnd), CopperLayer::Inner(1), rect(0.0, 0.0, 44.0, 28.0)),
    )?;
    let fresh = super::refill(engine.project(), RefillTarget::All)?;
    let batch = |copper: &[(Id, crate::model::fill::FilledCopper)]| {
        Operation::Fill(FillCommand::StoreAll {
            fills: copper
                .iter()
                .map(|(fill, copper)| StoredFill {
                    fill: *fill,
                    copper: copper.clone(),
                })
                .collect(),
        })
    };
    let rev = engine.revision();
    let before = engine.to_file();
    let mut stale = fresh.clone();
    if let Some((_, c)) = stale.last_mut() {
        c.key = "0000000000000000".into();
    }
    assert!(engine.apply(rev, batch(&stale)).is_err());
    let mut gone = fresh.clone();
    gone.push((9999, fresh[0].1.clone()));
    assert!(engine.apply(rev, batch(&gone)).is_err());
    assert_eq!(engine.revision(), rev);
    assert_eq!(engine.to_file(), before);
    engine.apply(rev, batch(&fresh))?;
    for id in [a, b] {
        assert!(!super::is_stale(
            engine.project(),
            fill(engine.project(), id)?
        ));
    }
    engine.undo()?;
    assert_eq!(engine.to_file(), before);
    Ok(())
}
