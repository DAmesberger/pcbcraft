use super::*;
use crate::layout::CopperGraph;
use crate::layout::LayoutCommand;
use crate::model::{Stackup, Track};
use crate::{Engine, Operation, Outcome};

fn layout(engine: &mut Engine, command: LayoutCommand) -> Result<Outcome> {
    engine.apply(engine.revision(), Operation::Layout(command))
}

#[test]
fn a_via_is_added_moved_tented_and_deleted_as_a_document_object() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = engine.project().nets[0].id;
    let Outcome::ViaAdded { via } = layout(
        &mut engine,
        LayoutCommand::AddVia {
            via: ViaRequest::through(net, [20.0, 20.0]),
        },
    )?
    else {
        return Err(Error::Invalid("no via outcome".into()));
    };
    assert_eq!(engine.project().via(via).map(|v| v.tented), Some(true));
    layout(
        &mut engine,
        LayoutCommand::MoveVia {
            id: via,
            position: [21.0, 20.0],
        },
    )?;
    layout(
        &mut engine,
        LayoutCommand::SetViaTented {
            id: via,
            tented: false,
        },
    )?;
    let moved = engine.project().via(via).cloned();
    assert_eq!(
        moved.map(|v| (v.position, v.tented)),
        Some(([21.0, 20.0], false))
    );
    layout(&mut engine, LayoutCommand::DeleteVia { id: via })?;
    assert!(engine.project().vias.is_empty());
    Ok(())
}

#[test]
fn invalid_vias_leave_the_document_unchanged() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = engine.project().nets[0].id;
    let before = engine.project().clone();
    let outside = ViaRequest::through(net, [0.1, 0.1]);
    let mut drill = ViaRequest::through(net, [20.0, 20.0]);
    drill.drill = drill.diameter;
    let mut blind = ViaRequest::through(net, [20.0, 20.0]);
    blind.span = ViaSpan::Between {
        from: CopperLayer::Top,
        to: CopperLayer::Inner(1),
    };
    for via in [
        outside,
        drill,
        blind,
        ViaRequest::through(9_999, [20.0, 20.0]),
    ] {
        assert!(layout(&mut engine, LayoutCommand::AddVia { via }).is_err());
    }
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn the_stackup_is_set_and_checked_and_names_its_layers() -> Result<()> {
    let mut engine = Engine::demo()?;
    assert_eq!(engine.project().stackup, Stackup::default());
    let four = Stackup::preset(4)?;
    assert_eq!(
        four.layers(),
        [
            CopperLayer::Top,
            CopperLayer::Inner(1),
            CopperLayer::Inner(2),
            CopperLayer::Bottom
        ]
    );
    layout(&mut engine, LayoutCommand::SetStackup(four.clone()))?;
    assert_eq!(engine.project().stackup, four);
    let mut three = four.clone();
    three.copper.pop();
    assert!(layout(&mut engine, LayoutCommand::SetStackup(three)).is_err());
    assert_eq!(engine.project().stackup, four);
    Ok(())
}

/// Demo pads: R1 at (8, 8) and R2 at (16, 8) with pads at ±0.8 mm,
/// 1 × 1.4 mm; R1.2 (8.8, 8) and R2.1 (15.2, 8) share the net.
fn shared(engine: &Engine) -> Result<Id> {
    let r1 = crate::tests::support::by_designator(engine, "R1")?;
    engine
        .project()
        .net_of(crate::ids::Endpoint {
            instance: r1,
            terminal: crate::TerminalId(2),
        })
        .map(|n| n.id)
        .ok_or_else(|| Error::NotFound("shared net".into()))
}

fn other_net(engine: &Engine, net: Id) -> Result<Id> {
    engine
        .project()
        .nets
        .iter()
        .find(|n| n.id != net)
        .map(|n| n.id)
        .ok_or_else(|| Error::NotFound("other net".into()))
}

fn change(
    net: Id,
    layer: CopperLayer,
    start: Point,
    at: Point,
    to_layer: CopperLayer,
    end: Point,
) -> LayoutCommand {
    LayoutCommand::RouteLayerChange {
        request: LayerChangeRequest {
            request: crate::layout::RouteRequest {
                net,
                start,
                end: at,
                width: 0.3,
                layer,
            },
            via: ViaRequest::through(net, at),
            to_layer,
            end,
        },
    }
}

#[test]
fn a_layer_change_routes_through_a_via_and_joins_the_copper() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = shared(&engine)?;
    assert_eq!(crate::layout::airwires(engine.project()).len(), 1);
    let first = layout(
        &mut engine,
        change(
            net,
            CopperLayer::Top,
            [8.8, 8.0],
            [11.0, 10.0],
            CopperLayer::Bottom,
            [13.0, 10.0],
        ),
    )?;
    assert!(matches!(first, Outcome::ViaAdded { .. }));
    let one_via = engine.project().clone();
    assert_eq!((one_via.tracks.len(), one_via.vias.len()), (2, 1));
    assert_eq!(
        one_via.tracks.iter().map(|t| t.layer).collect::<Vec<_>>(),
        [CopperLayer::Top, CopperLayer::Bottom]
    );
    // Bottom copper does not reach R2's top pad yet.
    assert_eq!(crate::layout::airwires(&one_via).len(), 1);
    let Outcome::ViaAdded { via } = layout(
        &mut engine,
        change(
            net,
            CopperLayer::Bottom,
            [12.5, 10.0],
            [13.0, 10.0],
            CopperLayer::Top,
            [15.2, 8.0],
        ),
    )?
    else {
        return Err(Error::Invalid("no via outcome".into()));
    };
    assert!(crate::layout::airwires(engine.project()).is_empty());
    let findings = crate::layout::checks(engine.project());
    assert!(findings.is_empty(), "{findings:#?}");
    // Without the second via the bottom run is cut off from R2 again.
    let mut cut = engine.project().clone();
    cut.vias.retain(|v| v.id != via);
    assert_eq!(crate::layout::airwires(&cut).len(), 1);
    // One undo step removes both tracks and the via.
    engine.undo()?;
    assert_eq!(engine.project(), &one_via);
    Ok(())
}

#[test]
fn invalid_layer_changes_leave_the_document_unchanged() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = shared(&engine)?;
    let other = other_net(&engine, net)?;
    let before = engine.project().clone();
    let mut displaced = change(
        net,
        CopperLayer::Top,
        [8.8, 8.0],
        [11.0, 10.0],
        CopperLayer::Bottom,
        [13.0, 10.0],
    );
    if let LayoutCommand::RouteLayerChange { request } = &mut displaced {
        request.via.position = [11.5, 10.0];
    }
    let mut foreign = change(
        net,
        CopperLayer::Top,
        [8.8, 8.0],
        [11.0, 10.0],
        CopperLayer::Bottom,
        [13.0, 10.0],
    );
    if let LayoutCommand::RouteLayerChange { request } = &mut foreign {
        request.via.net = other;
    }
    for command in [
        displaced,
        foreign,
        change(
            net,
            CopperLayer::Top,
            [8.8, 8.0],
            [11.0, 10.0],
            CopperLayer::Top,
            [13.0, 10.0],
        ),
        // The default stackup has no inner layers.
        change(
            net,
            CopperLayer::Top,
            [8.8, 8.0],
            [11.0, 10.0],
            CopperLayer::Inner(1),
            [13.0, 10.0],
        ),
    ] {
        assert!(layout(&mut engine, command).is_err());
    }
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn vias_and_routes_keep_clear_of_other_nets() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = shared(&engine)?;
    let other = other_net(&engine, net)?;
    let before = engine.project().clone();
    // R2.2 is at (16.8, 8) with its pad's top edge at y = 8.7.
    let close = ViaRequest::through(net, [16.8, 9.1]);
    assert!(matches!(
        layout(&mut engine, LayoutCommand::AddVia { via: close }),
        Err(Error::Conflict(_))
    ));
    assert_eq!(engine.project(), &before);
    layout(
        &mut engine,
        LayoutCommand::AddVia {
            via: ViaRequest::through(other, [30.0, 20.0]),
        },
    )?;
    let with_via = engine.project().clone();
    let past = crate::layout::RouteRequest {
        net,
        start: [25.0, 20.5],
        end: [35.0, 20.5],
        width: 0.3,
        layer: CopperLayer::Bottom,
    };
    assert!(matches!(
        layout(&mut engine, LayoutCommand::Route { request: past }),
        Err(Error::Conflict(_))
    ));
    assert_eq!(engine.project(), &with_via);
    Ok(())
}

#[test]
fn via_checks_report_ring_drill_and_hole_spacing() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = shared(&engine)?;
    let other = other_net(&engine, net)?;
    let mut add = |net, position: Point, drill, diameter| {
        let mut via = ViaRequest::through(net, position);
        via.drill = drill;
        via.diameter = diameter;
        layout(&mut engine, LayoutCommand::AddVia { via })
    };
    // A narrow ring.
    add(net, [30.0, 30.0], 0.35, 0.5)?;
    // Two vias whose drills are 0.2 mm apart.
    add(net, [40.0, 30.0], 0.3, 0.6)?;
    add(net, [40.5, 30.0], 0.3, 0.6)?;
    // Copper clear of a thin-ring via (0.22 mm), but its drill 0.24 mm
    // from the other net's via pad.
    add(net, [50.0, 30.0], 0.56, 0.6)?;
    add(other, [50.82, 30.0], 0.3, 0.6)?;
    let codes: Vec<String> = crate::layout::checks(engine.project())
        .into_iter()
        .map(|f| f.code)
        .collect();
    for code in [
        "board.via_annular_ring",
        "board.via_hole_spacing",
        "board.via_drill_clearance",
        "board.via_dangling",
    ] {
        assert!(codes.iter().any(|c| c == code), "{code} in {codes:?}");
    }
    assert!(!codes.iter().any(|c| c == "board.clearance"), "{codes:?}");
    Ok(())
}

#[test]
fn a_stackup_change_cannot_drop_a_used_layer() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = shared(&engine)?;
    layout(&mut engine, LayoutCommand::SetStackup(Stackup::preset(4)?))?;
    layout(
        &mut engine,
        LayoutCommand::Route {
            request: crate::layout::RouteRequest {
                net,
                start: [20.0, 20.0],
                end: [30.0, 20.0],
                width: 0.3,
                layer: CopperLayer::Inner(2),
            },
        },
    )?;
    let before = engine.project().clone();
    let refused = layout(&mut engine, LayoutCommand::SetStackup(Stackup::default()));
    assert!(
        matches!(&refused, Err(Error::Conflict(m)) if m.contains("Inner 2")),
        "{refused:?}"
    );
    assert_eq!(engine.project(), &before);
    let track = before.tracks[0].id;
    layout(&mut engine, LayoutCommand::DeleteTrack { id: track })?;
    layout(&mut engine, LayoutCommand::SetStackup(Stackup::default()))?;
    assert_eq!(engine.project().stackup, Stackup::default());
    Ok(())
}

#[test]
fn the_board_cannot_shrink_past_a_via() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = shared(&engine)?;
    layout(
        &mut engine,
        LayoutCommand::AddVia {
            via: ViaRequest::through(net, [50.0, 30.0]),
        },
    )?;
    let before = engine.project().clone();
    assert!(
        layout(
            &mut engine,
            LayoutCommand::SetBoardSize { size: [40.0, 40.0] }
        )
        .is_err()
    );
    assert_eq!(engine.project(), &before);
    layout(
        &mut engine,
        LayoutCommand::SetBoardSize { size: [55.0, 35.0] },
    )?;
    assert_eq!(engine.project().board_size, [55.0, 35.0]);
    Ok(())
}

/// Splitting a net offers for removal the vias of the copper that would
/// join pads now on different nets; a via joining no pad stays (slice 4 of
/// `docs/copper-graph.md`).
#[test]
fn electrical_changes_to_a_net_with_vias_are_reviewed_updates() -> Result<()> {
    use crate::updates::{ApplyUpdate, Change};
    let mut engine = Engine::demo()?;
    let net = shared(&engine)?;
    // R1.2 to R2.1 through two vias, as a layer change routes it.
    let mut routed = Vec::new();
    for command in [
        change(
            net,
            CopperLayer::Top,
            [8.8, 8.0],
            [11.0, 10.0],
            CopperLayer::Bottom,
            [13.0, 10.0],
        ),
        change(
            net,
            CopperLayer::Bottom,
            [12.5, 10.0],
            [13.0, 10.0],
            CopperLayer::Top,
            [15.2, 8.0],
        ),
    ] {
        let Outcome::ViaAdded { via } = layout(&mut engine, command)? else {
            return Err(Error::Invalid("no via outcome".into()));
        };
        routed.push(via);
    }
    let Outcome::ViaAdded { via: lone } = layout(
        &mut engine,
        LayoutCommand::AddVia {
            via: ViaRequest::through(net, [12.0, 20.0]),
        },
    )?
    else {
        return Err(Error::Invalid("no via outcome".into()));
    };
    let wire = engine.project().wires[0].id;
    let disconnect = crate::schematic::SchematicCommand::Disconnect { wire };
    let before = engine.project().clone();
    assert!(
        engine
            .apply(engine.revision(), Operation::Schematic(disconnect.clone()))
            .is_err()
    );
    assert_eq!(engine.project(), &before);
    let change = Change::Schematic {
        command: disconnect,
    };
    let plan = engine.preview_update(change.clone());
    assert_eq!(plan.conflicting_vias, routed);
    assert_eq!(plan.conflicting_tracks.len(), 4);
    let update = |remove| {
        Operation::Update(Box::new(ApplyUpdate {
            expected_revision: engine.revision(),
            change: change.clone(),
            remove_conflicting_tracks: remove,
        }))
    };
    let keep = update(false);
    let remove = update(true);
    assert!(engine.apply(engine.revision(), keep).is_err());
    engine.apply(engine.revision(), remove)?;
    assert!(engine.project().tracks.is_empty());
    assert_eq!(
        engine
            .project()
            .vias
            .iter()
            .map(|v| v.id)
            .collect::<Vec<_>>(),
        [lone]
    );
    Ok(())
}

#[test]
fn stackup_heights_stack_from_the_bottom_dielectric_face() -> Result<()> {
    let missing = || Error::Invalid("missing layer".into());
    let two = Stackup::default();
    assert_eq!(two.copper_span(CopperLayer::Bottom), Some((-0.035, 0.0)));
    let top = two.copper_span(CopperLayer::Top).ok_or_else(missing)?;
    assert!((top.0 - 1.6).abs() < 1e-12 && (top.1 - 1.635).abs() < 1e-12);
    let four = Stackup::preset(4)?;
    let inner = four
        .copper_span(CopperLayer::Inner(2))
        .ok_or_else(missing)?;
    assert!((inner.1 - 0.2).abs() < 1e-12, "{inner:?}");
    assert_eq!(four.copper_span(CopperLayer::Inner(3)), None);
    assert!((four.thickness_mm() - 1.74).abs() < 1e-12);
    Ok(())
}

/// The showcase on four layers with `count` through vias on a 1.27 mm grid
/// below its parts, on two nets in pairs, and inner tracks joining pairs
/// until the board holds `tracks` tracks.
pub(crate) fn via_board(count: usize, tracks: usize) -> Result<Project> {
    let mut project = Engine::showcase()?.project().clone();
    project.stackup = Stackup::preset(4)?;
    let nets = [project.nets[0].id, project.nets[1].id];
    let columns = 100usize;
    let rows = count.div_ceil(columns);
    let [width, depth] = project.board_size;
    project.board_size = [
        width.max(columns as f64 * 1.27 + 6.0),
        depth + rows as f64 * 1.27 + 4.0,
    ];
    let mut wanted = tracks.saturating_sub(project.tracks.len());
    for k in 0..count {
        let position = [
            3.0 + (k % columns) as f64 * 1.27,
            depth + 2.0 + (k / columns) as f64 * 1.27,
        ];
        let net = nets[(k / 2) % 2];
        let id = project.allocate()?;
        project.vias.push(Via {
            id,
            net,
            position,
            drill: DEFAULT_VIA_DRILL_MM,
            diameter: DEFAULT_VIA_DIAMETER_MM,
            span: ViaSpan::Through,
            tented: k % 2 == 0,
            locked: false,
        });
        if wanted > 0 && k % 2 == 0 {
            wanted -= 1;
            let id = project.allocate()?;
            project.tracks.push(Track {
                id,
                net,
                start: position,
                end: [position[0] + 1.27, position[1]],
                width: 0.15,
                layer: CopperLayer::Inner(1 + u8::from(k % 4 == 0)),
                locked: false,
            });
        }
    }
    Ok(project)
}

fn engine_for(project: Project) -> Result<Engine> {
    let mut file = Engine::showcase()?.to_file();
    file.project = project;
    Engine::from_file(file)
}

/// Update PCB on the 10,000-via board: disconnecting a wire of a net with
/// 5,000 vias builds the updated board's copper graph once and previews
/// within a budget (slice 4 of `docs/copper-graph.md`).
#[test]
fn an_update_preview_on_ten_thousand_vias_stays_quick() -> Result<()> {
    use crate::updates::Change;
    use std::time::{Duration, Instant};
    let engine = engine_for(via_board(10_000, 2_000)?)?;
    let net = engine.project().nets[0].id;
    let wire = engine
        .project()
        .wires
        .iter()
        .find(|w| {
            [w.from, w.to].into_iter().any(|end| {
                end.terminal()
                    .and_then(|t| engine.project().net_of(t))
                    .is_some_and(|n| n.id == net)
            })
        })
        .map(|w| w.id)
        .ok_or_else(|| Error::NotFound("a wire on the first net".into()))?;
    let change = Change::Schematic {
        command: crate::schematic::SchematicCommand::Disconnect { wire },
    };
    let graph = engine.copper_graph();
    let pads = graph.pads().to_vec();
    let (mut build, mut preview) = (Duration::MAX, Duration::MAX);
    let mut plan = None;
    for _ in 0..5 {
        let start = Instant::now();
        let _ = CopperGraph::with_pads(engine.project(), pads.clone());
        build = build.min(start.elapsed());
        let start = Instant::now();
        plan = Some(engine.preview_update(change.clone()));
        preview = preview.min(start.elapsed());
    }
    eprintln!(
        "10,000 vias, 2,000 tracks: updated board's copper graph in {build:?}, \
         Update PCB preview (both graphs' checks included) in {preview:?}"
    );
    let plan = plan.ok_or_else(|| Error::NotFound("plan".into()))?;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    eprintln!(
        "offered for removal: {} tracks, {} vias",
        plan.conflicting_tracks.len(),
        plan.conflicting_vias.len()
    );
    assert!(preview < Duration::from_secs(1), "preview took {preview:?}");
    Ok(())
}

/// Every via of a 10,000-via, 2,000-track board is checked (the checks
/// compare only nearby copper, so nothing stops at a pair limit), within a
/// time budget; a fault among the last vias is still found; and an edit on
/// that board checks its clearance quickly.
#[test]
fn ten_thousand_vias_are_all_checked_within_a_budget() -> Result<()> {
    use std::time::{Duration, Instant};
    let mut project = via_board(10_000, 2_000)?;
    assert_eq!(project.tracks.len(), 2_000);
    // The last two vias: different nets, their pads overlapping and their
    // drills 0.2 mm apart.
    let n = project.vias.len();
    let nets = [project.nets[0].id, project.nets[1].id];
    if let Some([a, b]) = project.vias.get_mut(n - 2..) {
        a.net = nets[0];
        b.net = nets[1];
        b.position = [a.position[0] + 0.5, a.position[1]];
    }
    let (first, last) = (project.vias[n - 2].id, project.vias[n - 1].id);
    let engine = engine_for(project)?;
    let project = engine.project();
    // Best of five: the machine is shared with other builds. The copper
    // graph's build, its connectivity findings and airwires are timed on
    // their own, then the whole board check (graph included).
    let mut best = [Duration::MAX; 4];
    let mut findings = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let graph = CopperGraph::of(project);
        let built = start.elapsed();
        let _ = graph.findings(project);
        let found = start.elapsed();
        let _ = graph.airwires();
        let wired = start.elapsed();
        findings = BoardCopper::of(project).checks(project, &graph);
        let checked = start.elapsed();
        for (slot, time) in
            best.iter_mut()
                .zip([built, found - built, wired - found, checked - wired + built])
        {
            *slot = (*slot).min(time);
        }
    }
    let [build, connectivity, wires, checks] = best;
    eprintln!(
        "10,000 vias, 2,000 tracks: copper graph built in {build:?}, its findings in \
         {connectivity:?}, airwires in {wires:?}; board checks (graph included) in {checks:?}"
    );
    let best = checks + wires;
    assert!(
        !findings.iter().any(|f| f.code == "board.check_limit"),
        "the checks stopped at a limit"
    );
    let names = |code: &str, id: Id| {
        findings
            .iter()
            .any(|f| f.code == code && f.objects.contains(&ObjectRef::Via { id }))
    };
    assert!(names("board.clearance", first) && names("board.clearance", last));
    assert!(names("board.via_hole_spacing", first) && names("board.via_hole_spacing", last));
    // Only the planted pair breaks a rule among the vias.
    let broken: Vec<&Finding> = findings
        .iter()
        .filter(|f| {
            matches!(
                f.code.as_str(),
                "board.clearance" | "board.via_hole_spacing" | "board.via_drill_clearance"
            ) && f.objects.iter().any(|o| matches!(o, ObjectRef::Via { .. }))
        })
        .collect();
    assert!(
        broken
            .iter()
            .all(|f| f.objects.contains(&ObjectRef::Via { id: first })
                || f.objects.contains(&ObjectRef::Via { id: last })),
        "{broken:?}"
    );
    assert!(best < Duration::from_secs(1), "checks took {best:?}");
    // Edits on that board check clearance against nearby copper only: a
    // wider inner track passes, a via moved onto another net's is refused.
    let mut engine = engine;
    let track = engine
        .project()
        .tracks
        .iter()
        .find(|t| t.layer == CopperLayer::Inner(1))
        .map(|t| t.id)
        .ok_or_else(|| Error::NotFound("inner track".into()))?;
    // The board's own vias come after the showcase's.
    let base = engine.project().vias.len() - 10_000;
    let vias = &engine.project().vias;
    let (via, onto) = (vias[base].id, vias[base + 2].position);
    let start = Instant::now();
    layout(
        &mut engine,
        LayoutCommand::SetTrack {
            id: track,
            width: Some(0.2),
            layer: None,
        },
    )?;
    let refused = layout(
        &mut engine,
        LayoutCommand::MoveVia {
            id: via,
            position: [onto[0], onto[1] + 0.5],
        },
    );
    assert!(matches!(refused, Err(Error::Conflict(_))), "{refused:?}");
    let elapsed = start.elapsed();
    eprintln!("two edits on that board in {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(1),
        "the edits took {elapsed:?}"
    );
    Ok(())
}
