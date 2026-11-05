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
    FillJoin, FillSettings, FillSpec, Hatch, IslandRemoval, KeepoutLayers, KeepoutRules,
    KeepoutSpec, PadConnections,
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
        if pad.net != f.net && super::compute::pad_on(&pad, f.layer) {
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
    let copper = fill(p, id)?.computed.clone().ok_or("not filled")?;
    // Default: thermal reliefs with four spokes; every GND via joins.
    let gnd_vias = p.vias.iter().filter(|v| v.net == gnd).count();
    let via_joins: Vec<_> = copper
        .joins
        .iter()
        .filter_map(|j| match j {
            FillJoin::Via {
                connection, spokes, ..
            } => Some((*connection, *spokes)),
            _ => None,
        })
        .collect();
    assert_eq!(via_joins.len(), gnd_vias);
    assert!(via_joins.iter().all(|(c, _)| *c == PadConnection::Thermal));
    assert!(via_joins.iter().any(|(_, s)| *s == 4), "{via_joins:?}");
    assert!(
        copper
            .joins
            .iter()
            .any(|j| matches!(j, FillJoin::Track { .. }))
    );
    // The header's ground pin (through-hole) joins with spokes.
    assert!(
        copper.joins.iter().any(|j| matches!(
            j,
            FillJoin::Pad {
                connection: PadConnection::Thermal,
                spokes: 1..,
                ..
            }
        )),
        "{:?}",
        copper.joins
    );
    // Every join names a kept island.
    assert!(
        copper
            .joins
            .iter()
            .filter_map(FillJoin::joined_region)
            .all(|r| (r as usize) < copper.regions.len())
    );
    // A thermal relief leaves copper only in its spokes around the via.
    let via = p
        .vias
        .iter()
        .find(|v| v.net == gnd)
        .cloned()
        .ok_or("no via")?;
    let f = fill(p, id)?;
    let t = p.rules.effective().fill.thermal;
    let r = via.diameter / 2.0 + t.gap / 2.0;
    let on_spoke = |deg: f64| {
        let (s, c) = deg.to_radians().sin_cos();
        inside(f, [via.position[0] + c * r, via.position[1] + s * r])
    };
    let spokes = copper
        .joins
        .iter()
        .find_map(|j| match j {
            FillJoin::Via { via: v, spokes, .. } if *v == via.id => Some(*spokes),
            _ => None,
        })
        .ok_or("via not joined")?;
    if spokes == 4 {
        for k in 0..4 {
            assert!(on_spoke(t.angle_deg + 90.0 * f64::from(k)));
            assert!(!on_spoke(f64::from(k) * 90.0), "gap between spokes");
        }
    }

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
        !f.computed
            .iter()
            .flat_map(|c| &c.joins)
            .any(|j| matches!(j, FillJoin::Via { .. }))
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
    assert!(super::links(&moved).is_empty(), "stale joins are dropped");
    assert!(!super::links(&filled).is_empty());
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

#[test]
fn cutouts_mounting_holes_and_edge_clearance_shape_the_fill() -> Fallible<()> {
    use crate::library::footprint_source::{EvaluatorRef, SketchBlob, SolveSummary};
    use crate::model::board::{
        BoardFeatureId, BoardShape, BoardSketch, CutoutKind, SolvedBoard, SolvedCutout,
        SolvedMountingHole,
    };
    let (engine, id, _) = showcase_with(CopperLayer::Bottom)?;
    let mut p = engine.project().clone();
    let mut solved = SolvedBoard::rectangle(p.board_size);
    solved.cutouts.push(SolvedCutout {
        id: BoardFeatureId(1),
        kind: CutoutKind::Cutout,
        outline: rect(39.0, 22.0, 42.0, 26.0),
    });
    solved.holes.push(SolvedMountingHole {
        id: BoardFeatureId(2),
        at: [3.0, 25.0],
        drill: 3.2,
        plated: false,
        pad_diameter: None,
    });
    let evaluator = EvaluatorRef {
        engine: "test".into(),
        rev: "0".into(),
    };
    p.board_shape = BoardShape::Sketch(Box::new(BoardSketch {
        evaluator: evaluator.clone(),
        sketch: SketchBlob("{}".into()),
        parameters: Vec::new(),
        outline: Vec::new(),
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
    }));
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

/// Under each built-in profile, unchanged, a whole-board fill stays that
/// profile's edge clearance inside the rectangle. Under the first default
/// (0) the same fill reached the edge and the production check passed.
#[test]
fn the_fill_keeps_the_profile_edge() -> Fallible<()> {
    for profile in crate::rules::builtin::all() {
        let edge = profile
            .rules
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

#[test]
fn starved_thermals_are_reported() -> Fallible<()> {
    let mut engine = showcase()?;
    let gnd = net(&engine, "GND")?;
    // A thin fill strip through one ground via reaches it from two sides.
    let via = engine
        .project()
        .vias
        .iter()
        .find(|v| v.net == gnd)
        .cloned()
        .ok_or("no via")?;
    let [x, y] = via.position;
    let mut s = spec(
        Some(gnd),
        CopperLayer::Inner(2),
        rect(x - 3.0, y - 0.4, x + 3.0, y + 0.4),
    );
    s.settings.thermal = Some(crate::rules::ThermalRelief {
        spoke_width: 0.3,
        spoke_count: 4,
        gap: 0.25,
        angle_deg: 0.0,
    });
    s.settings.width_min = Some(0.1);
    let id = add(&mut engine, s)?;
    refill(&mut engine)?;
    let p = engine.project();
    let joins = fill(p, id)?.computed.clone().ok_or("not filled")?.joins;
    let spokes = joins
        .iter()
        .find_map(|j| match j {
            FillJoin::Via { via: v, spokes, .. } if *v == via.id => Some(*spokes),
            _ => None,
        })
        .ok_or("via not joined")?;
    assert_eq!(spokes, 2, "{joins:?}");
    assert!(
        layout::checks(p)
            .iter()
            .any(|f| f.code == "fill.thermal_starved"
                && f.objects.contains(&ObjectRef::Via { id: via.id }))
    );
    Ok(())
}

/// A GND surface pad on the showcase's top layer, its centre and the fill
/// strip over it: tall enough to cover the pad, too thin for a via's four
/// spokes (`starved_thermals_are_reported` measures the same relief).
struct PadStrip {
    engine: Engine,
    gnd: Id,
    pad: ObjectRef,
    in_pad: Id,
    beside: Id,
    strip: Outline,
}

fn via_at(engine: &mut Engine, net: Id, at: Point) -> Fallible<Id> {
    match engine.apply(
        engine.revision(),
        Operation::Layout(layout::LayoutCommand::AddVia {
            via: layout::ViaRequest::through(net, at),
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
    let object = ObjectRef::Pad {
        instance: pad.instance,
        pad: pad.pad.id,
    };
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
        pad: object,
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
    s.settings.thermal = Some(crate::rules::ThermalRelief {
        spoke_width: 0.3,
        spoke_count: 4,
        gap: 0.25,
        angle_deg: 0.0,
    });
    s.settings.width_min = Some(0.1);
    let id = add(&mut fixture.engine, s)?;
    refill(&mut fixture.engine)?;
    Ok(id)
}

fn starved(project: &Project, object: ObjectRef) -> bool {
    layout::checks(project)
        .iter()
        .any(|f| f.code == "fill.thermal_starved" && f.objects.contains(&object))
}

fn via_spokes(
    project: &Project,
    fill_id: Id,
    via: Id,
) -> Fallible<(PadConnection, u32, Option<u32>)> {
    let joins = fill(project, fill_id)?
        .computed
        .clone()
        .ok_or("not filled")?
        .joins;
    Ok(joins
        .iter()
        .find_map(|j| match j {
            FillJoin::Via {
                via: v,
                connection,
                spokes,
                ..
            } if *v == via => Some((*connection, *spokes, j.joined_region())),
            _ => None,
        })
        .ok_or("via not joined")?)
}

/// Gap 17 of the E2E log. The pad carries a via in it and the strip passes
/// another beside it. Both reliefs are starved by the same geometry; only
/// the via in the joined pad is excused.
#[test]
fn a_via_in_a_joined_pad_is_not_starved_and_one_beside_it_still_is() -> Fallible<()> {
    let mut f = pad_strip()?;
    let id = strip_fill(
        &mut f,
        CopperLayer::Top,
        PadConnections {
            smd: Some(PadConnection::Solid),
            through_hole: None,
            via: Some(PadConnection::Thermal),
        },
    )?;
    let p = f.engine.project();
    let (in_pad, beside) = (
        ObjectRef::Via { id: f.in_pad },
        ObjectRef::Via { id: f.beside },
    );
    let (_, spokes, _) = via_spokes(p, id, f.in_pad)?;
    assert!(spokes < 4, "the via in the pad is starved: {spokes}");
    assert!(!starved(p, in_pad), "a via in a joined pad is not reported");
    let (_, spokes, _) = via_spokes(p, id, f.beside)?;
    assert_eq!(
        spokes, 2,
        "the strip reaches the via beside it from two sides"
    );
    assert!(starved(p, beside), "a real starved via is still reported");
    Ok(())
}

#[test]
fn a_via_in_a_pad_the_fill_does_not_join_is_still_reported() -> Fallible<()> {
    // The fill clears the pad, so the via is the fill's only way in.
    let mut f = pad_strip()?;
    strip_fill(
        &mut f,
        CopperLayer::Top,
        PadConnections {
            smd: Some(PadConnection::None),
            through_hole: None,
            via: Some(PadConnection::Thermal),
        },
    )?;
    assert!(starved(f.engine.project(), ObjectRef::Via { id: f.in_pad }));

    // A fill on another layer than the pad: the pad joins nothing here.
    let mut f = pad_strip()?;
    strip_fill(
        &mut f,
        CopperLayer::Bottom,
        PadConnections {
            smd: Some(PadConnection::Solid),
            through_hole: None,
            via: Some(PadConnection::Thermal),
        },
    )?;
    assert!(starved(f.engine.project(), ObjectRef::Via { id: f.in_pad }));
    Ok(())
}

#[test]
fn a_starved_pad_answers_for_the_via_inside_it() -> Fallible<()> {
    // The pad's own thermal is reported when it is starved, so excusing
    // the via hides nothing.
    let mut f = pad_strip()?;
    let id = strip_fill(
        &mut f,
        CopperLayer::Top,
        PadConnections {
            smd: Some(PadConnection::Thermal),
            through_hole: None,
            via: Some(PadConnection::Thermal),
        },
    )?;
    let p = f.engine.project();
    let copper = fill(p, id)?.computed.clone().ok_or("not filled")?;
    let (spokes, joined) = copper
        .joins
        .iter()
        .find_map(|j| match (*j, f.pad) {
            (
                FillJoin::Pad {
                    instance,
                    pad,
                    spokes,
                    ..
                },
                ObjectRef::Pad {
                    instance: i,
                    pad: q,
                },
            ) if instance == i && pad == q => Some((spokes, j.joined_region().is_some())),
            _ => None,
        })
        .ok_or("pad not joined")?;
    assert!(spokes < 4, "{spokes}");
    assert!(starved(p, f.pad), "the pad is reported");
    assert_eq!(
        starved(p, ObjectRef::Via { id: f.in_pad }),
        !joined,
        "the via is excused only while the pad reaches the fill"
    );
    Ok(())
}

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
        let (connection, _, region) = via_spokes(p, id, via)?;
        assert_eq!(connection, PadConnection::Solid);
        assert!(region.is_some(), "via {via} joins the copper");
        assert!(!starved(p, ObjectRef::Via { id: via }));
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
    assert!(starved(
        other.engine.project(),
        ObjectRef::Via { id: other.beside }
    ));
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
    let joined = copper
        .joins
        .iter()
        .filter(|j| j.joined_region().is_some())
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
