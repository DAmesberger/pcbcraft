use super::*;
use crate::{Engine, Operation, PartRef, parts, schematic};

fn fixture() -> Result<Project> {
    let mut project = Engine::demo()?.project().clone();
    for (instance, position) in [(2, [10.0, 10.0]), (3, [25.0, 10.0])] {
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

fn request() -> RouteRequest {
    RouteRequest {
        net: "net:2:2".into(),
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
            .filter(|s| s.starts_with("Unrouted:"))
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
            instance: 3,
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
            .filter(|s| s.starts_with("Unrouted:"))
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
    schematic::apply(
        &mut project,
        schematic::SchematicCommand::Place {
            part: PartRef { id: 1, revision: 1 },
            reference: "R3".into(),
            value: "1k".into(),
            position: [40.0, 0.0],
        },
    )?;
    let expected = project.instances[2].id;
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
    project.parts[0].footprint.datum_offset = [1.0, 1.0];
    project.parts[0].footprint.pads[0].position = [3.0, 4.0];
    apply(
        &mut project,
        LayoutCommand::Move {
            instance: 2,
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
    assert_eq!(
        pad.terminal,
        Endpoint {
            instance: 2,
            terminal: 1
        }
    );
    assert_eq!(pad.net, "net:2:1");
    Ok(())
}

#[test]
fn rejects_foreign_nets_malformed_geometry_and_short_circuits_atomically() -> Result<()> {
    let mut project = fixture()?;
    let before = project.clone();
    for bad in [
        RouteRequest {
            net: "foreign-net".into(),
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
    apply(
        &mut project,
        LayoutCommand::SetPlacementLock {
            instance: 2,
            locked: true,
        },
    )?;
    let before = project.clone();
    assert!(
        apply(
            &mut project,
            LayoutCommand::Move {
                instance: 2,
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
                                net: "unknown".into(),
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
    let mut part = parts::passive_template("Mapped");
    part.binding.mappings[0].physical = 2;
    part.binding.mappings[1].physical = 1;
    // Exact snapshots are immutable through commands; this fixture substitutes
    // a valid alternate definition to exercise geometry's explicit mapping.
    part.id = 1;
    part.revision = 1;
    project.parts[0] = part;
    let world = pads(&project);
    assert_eq!(world[0].terminal.terminal, 2);
    assert_eq!(world[0].net, "net:2:2");
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
            instance: 3,
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
            .filter(|s| s.starts_with("Unrouted:"))
            .count(),
        2
    );
    Ok(())
}
