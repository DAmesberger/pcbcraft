use super::*;
use crate::tests::support::{by_designator, demo_fixture, ep};
use crate::{Engine, Operation, PadId, TerminalId, UnitId, instances, nets};

fn ids() -> Result<(Id, Id)> {
    let engine = Engine::demo()?;
    Ok((by_designator(&engine, "R1")?, by_designator(&engine, "R2")?))
}

fn fixture() -> Result<Project> {
    let mut project = Engine::demo()?.project().clone();
    let (r1, r2) = ids()?;
    let net = |terminal| project.net_of(ep(r1, terminal)).map(|n| n.id);
    assert_eq!((net(1), net(2)), (Some(R1_1), Some(SHARED)));
    for (instance, position) in [(r1, [10.0, 10.0]), (r2, [25.0, 10.0])] {
        apply(
            &mut project,
            LayoutCommand::Move {
                instance,
                position,
                rotation_deg: 0.0,
                layer: Layer::Top,
            },
        )?;
    }
    Ok(project)
}

/// Demo net IDs: R1 terminals got 1 and 2, R2 got 3 and 4; connecting
/// R1.2–R2.1 merged 3 into the older 2.
const SHARED: Id = 2;
const R1_1: Id = 1;

fn request() -> RouteRequest {
    RouteRequest {
        net: SHARED,
        start: [11.2, 10.0],
        end: [23.8, 10.0],
        width: 0.3,
        layer: Layer::Top,
    }
}

#[test]
fn physical_connectivity_changes_when_a_pad_moves() -> Result<()> {
    let mut project = fixture()?;
    assert_eq!(airwires(&project).len(), 1);
    assert_eq!(
        checks(&project)
            .iter()
            .filter(|s| s.code == "board.unrouted")
            .count(),
        2
    );
    apply(&mut project, LayoutCommand::Route { request: request() })?;
    assert!(checks(&project).is_empty());
    assert!(airwires(&project).is_empty());
    let track = project.tracks[0].clone();
    apply(
        &mut project,
        LayoutCommand::Move {
            instance: ids()?.1,
            position: [25.0, 13.0],
            rotation_deg: 0.0,
            layer: Layer::Top,
        },
    )?;
    assert_eq!(project.tracks[0], track);
    assert_eq!(airwires(&project).len(), 1);
    assert_eq!(
        checks(&project)
            .iter()
            .filter(|s| s.code == "board.unrouted")
            .count(),
        2
    );
    Ok(())
}

#[test]
fn sync_adds_missing_instances_without_moving_or_ripping_up() -> Result<()> {
    let mut project = fixture()?;
    apply(&mut project, LayoutCommand::Route { request: request() })?;
    let old_placements = project.placements.clone();
    let old_tracks = project.tracks.clone();
    let f = demo_fixture(&Engine::demo()?)?;
    let (expected, _) = instances::place(
        &mut project,
        instances::NewInstance {
            component: f.r1(),
            designator: "R3".into(),
            properties: vec![],
            representation: f.symbol,
            unit: UnitId(1),
            position: [40.0, 0.0],
            physical: Some(f.physical()),
        },
    )?;
    nets::reconcile(&mut project)?;
    assert_eq!(sync_preview(&project), vec![expected]);
    apply(&mut project, LayoutCommand::Sync)?;
    assert_eq!(&project.placements[..2], old_placements);
    assert_eq!(project.tracks, old_tracks);
    assert!(sync_preview(&project).is_empty());
    Ok(())
}

#[test]
fn bottom_transform_uses_datum_then_rigid_flip_then_rotation() -> Result<()> {
    let mut project = fixture()?;
    let (r1, _) = ids()?;
    // Exact snapshots are immutable through commands; this fixture substitutes
    // a stored footprint to exercise the placement transform alone.
    let footprint = &mut project.library.footprints[0].definition;
    footprint.datum_offset = [1.0, 1.0];
    footprint.pads[0].position = [3.0, 4.0];
    apply(
        &mut project,
        LayoutCommand::Move {
            instance: r1,
            position: [10.0, 20.0],
            rotation_deg: 90.0,
            layer: Layer::Bottom,
        },
    )?;
    let world = pads(&project);
    let pad = &world[0];
    assert!((pad.position[0] - 13.0).abs() < 1e-8);
    assert!((pad.position[1] - 22.0).abs() < 1e-8);
    assert_eq!(pad.layer, Layer::Bottom);
    assert_eq!(pad.terminal, ep(r1, 1));
    assert_eq!(pad.net, R1_1);
    Ok(())
}

#[test]
fn rejects_foreign_nets_malformed_geometry_and_short_circuits_atomically() -> Result<()> {
    let mut project = fixture()?;
    let before = project.clone();
    for bad in [
        RouteRequest {
            net: 999,
            ..request()
        },
        RouteRequest {
            width: f64::NAN,
            ..request()
        },
        RouteRequest {
            start: [8.8, 10.0],
            ..request()
        },
    ] {
        assert!(apply(&mut project, LayoutCommand::Route { request: bad }).is_err());
        assert_eq!(project, before);
    }
    Ok(())
}

#[test]
fn locked_placements_and_tracks_reject_edits() -> Result<()> {
    let mut project = fixture()?;
    let (r1, _) = ids()?;
    apply(
        &mut project,
        LayoutCommand::SetPlacementLock {
            instance: r1,
            locked: true,
        },
    )?;
    let before = project.clone();
    assert!(
        apply(
            &mut project,
            LayoutCommand::Move {
                instance: r1,
                position: [20.0, 20.0],
                rotation_deg: 0.0,
                layer: Layer::Bottom
            }
        )
        .is_err()
    );
    assert_eq!(project, before);
    apply(&mut project, LayoutCommand::Route { request: request() })?;
    let id = project.tracks[0].id;
    apply(
        &mut project,
        LayoutCommand::SetTrackLock { id, locked: true },
    )?;
    let before = project.clone();
    assert!(apply(&mut project, LayoutCommand::DeleteTrack { id }).is_err());
    let locked_track = project.tracks[0].clone();
    assert!(
        apply(
            &mut project,
            LayoutCommand::ApplyRoute {
                edits: RouteEditSet {
                    replace: vec![locked_track],
                    ..Default::default()
                }
            }
        )
        .is_err()
    );
    assert_eq!(project, before);
    Ok(())
}

#[test]
fn multi_segment_edit_is_one_atomic_engine_operation() -> Result<()> {
    let mut engine = Engine::demo()?;
    let before = engine.project().clone();
    let revision = engine.revision();
    assert!(
        engine
            .apply(
                revision,
                Operation::Layout(LayoutCommand::ApplyRoute {
                    edits: RouteEditSet {
                        add: vec![
                            RouteRequest {
                                start: [9.2, 8.0],
                                end: [12.0, 8.0],
                                ..request()
                            },
                            RouteRequest {
                                net: 999,
                                ..request()
                            }
                        ],
                        ..Default::default()
                    }
                })
            )
            .is_err()
    );
    assert_eq!(engine.revision(), revision);
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn electrical_mapping_connects_the_right_physical_pads() -> Result<()> {
    let mut project = fixture()?;
    // Exact snapshots are immutable through commands; this fixture substitutes
    // a valid alternate mapping to exercise geometry's explicit mapping.
    let binding = &mut project.library.bindings[0].definition;
    binding.mappings[0].terminal = TerminalId(2);
    binding.mappings[1].terminal = TerminalId(1);
    let world = pads(&project);
    assert_eq!(world[0].pad, PadId(1));
    assert_eq!(world[0].terminal.terminal, TerminalId(2));
    assert_eq!(world[0].net, SHARED);
    Ok(())
}

#[test]
fn minimum_clearance_uses_copper_edges_and_accepts_exact_boundary() -> Result<()> {
    let mut project = fixture()?;
    let before = project.clone();
    assert!(
        apply(
            &mut project,
            LayoutCommand::Route {
                request: RouteRequest {
                    start: [9.49, 10.0],
                    ..request()
                }
            }
        )
        .is_err()
    );
    assert_eq!(project, before);
    apply(
        &mut project,
        LayoutCommand::Route {
            request: RouteRequest {
                start: [9.5, 10.0],
                ..request()
            },
        },
    )?;
    assert!(checks(&project).is_empty());
    Ok(())
}

#[test]
fn matching_xy_on_opposite_layers_does_not_connect_copper() -> Result<()> {
    let mut project = fixture()?;
    apply(
        &mut project,
        LayoutCommand::Move {
            instance: ids()?.1,
            position: [25.0, 10.0],
            rotation_deg: 0.0,
            layer: Layer::Bottom,
        },
    )?;
    apply(&mut project, LayoutCommand::Route { request: request() })?;
    assert_eq!(airwires(&project).len(), 1);
    assert_eq!(
        checks(&project)
            .iter()
            .filter(|s| s.code == "board.unrouted")
            .count(),
        2
    );
    Ok(())
}
