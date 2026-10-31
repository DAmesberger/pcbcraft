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
            designator: Some("R3".into()),
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
    footprint.datum.offset = [1.0, 1.0];
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
    // The placement transform and the placed-pad query agree with it.
    let placed = placed_pads(&project);
    let first = placed
        .iter()
        .find(|p| p.instance == r1 && p.pad.id == pad.pad)
        .ok_or_else(|| Error::NotFound("placed pad".into()))?;
    let at = first.transform.apply(first.pad.position);
    assert!((at[0] - 13.0).abs() < 1e-8 && (at[1] - 22.0).abs() < 1e-8);
    assert_eq!(first.net, Some(R1_1));
    assert_eq!(first.layer, Layer::Bottom);
    let placement = project
        .placements
        .iter()
        .find(|p| p.instance == r1)
        .ok_or_else(|| Error::NotFound("placement".into()))?;
    let t = placement_transform(placement, &footprint_datum(&project));
    assert_eq!(t, first.transform);
    Ok(())
}

fn footprint_datum(project: &Project) -> crate::library::Datum {
    project.library.footprints[0].definition.datum
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
                    // R1.1's land ends at 9.7 (pad at 9.2, 1 mm wide): a
                    // 0.3 mm track starting here is 0.04 mm from it.
                    start: [9.89, 10.0],
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
                start: [9.9, 10.0],
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

fn blue_matte_osp() -> Fabrication {
    use crate::model::{MaskColour, MaskFinish, PadFinish, SilkscreenColour};
    Fabrication {
        mask_colour: MaskColour::Blue,
        mask_finish: MaskFinish::Matte,
        pad_finish: PadFinish::Osp,
        silkscreen_colour: SilkscreenColour::Yellow,
    }
}

#[test]
fn fabrication_defaults_to_green_gloss_enig_with_white_legend() {
    use crate::model::{MaskColour, MaskFinish, PadFinish, SilkscreenColour};
    assert_eq!(
        Project::default().fabrication,
        Fabrication {
            mask_colour: MaskColour::Green,
            mask_finish: MaskFinish::Gloss,
            pad_finish: PadFinish::Enig,
            silkscreen_colour: SilkscreenColour::White,
        }
    );
}

/// `layout.set_fabrication` is one undo step through the engine, survives a
/// save and reopen, and a stale request changes nothing.
#[test]
fn set_fabrication_is_one_undo_step_and_round_trips_through_the_file() -> Result<()> {
    let mut engine = Engine::demo()?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let set = Operation::Layout(LayoutCommand::SetFabrication(blue_matte_osp()));
    engine.apply(revision, set.clone())?;
    assert_eq!(engine.project().fabrication, blue_matte_osp());
    assert_eq!(engine.revision(), revision + 1);
    // Only the fabrication changed.
    let mut expected = before.clone();
    expected.fabrication = blue_matte_osp();
    assert_eq!(engine.project(), &expected);

    let bytes = crate::format::encode(&engine.to_file())?;
    let text = String::from_utf8(bytes.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
    for value in [
        "\"mask_colour\": \"blue\"",
        "\"mask_finish\": \"matte\"",
        "\"pad_finish\": \"osp\"",
        "\"silkscreen_colour\": \"yellow\"",
    ] {
        assert!(text.contains(value), "{value} missing from the saved file");
    }
    let reopened = Engine::from_file(crate::format::decode(&bytes)?)?;
    assert_eq!(reopened.project(), engine.project());

    let current = engine.revision();
    assert!(matches!(
        engine.apply(revision, set),
        Err(Error::Stale { .. })
    ));
    assert_eq!(engine.revision(), current);

    engine.undo()?;
    assert_eq!(engine.project(), &before);
    engine.redo()?;
    assert_eq!(engine.project().fabrication, blue_matte_osp());
    Ok(())
}

/// Restating the current fabrication from the CLI or MCP is a no-op: no
/// revision, no undo step, the same project.
#[test]
fn unchanged_fabrication_takes_no_revision_or_undo_step() -> Result<()> {
    let mut engine = Engine::demo()?;
    let revision = engine.revision();
    let current = engine.project().fabrication;
    assert!(!engine.can_undo());
    engine.apply(
        revision,
        Operation::Layout(LayoutCommand::SetFabrication(current)),
    )?;
    assert_eq!(engine.revision(), revision);
    assert!(!engine.can_undo());
    assert_eq!(engine.project().fabrication, current);
    Ok(())
}

/// The wire form is typed: unknown colours, finishes or extra fields are
/// refused before anything is applied, and a file without the attributes is
/// not this format version's file.
#[test]
fn fabrication_rejects_unknown_values_and_files_without_it() -> Result<()> {
    let wire = serde_json::json!({
        "editor": "Layout",
        "operation": {"command": "layout.set_fabrication", "params": {
            "mask_colour": "purple", "mask_finish": "satin",
            "pad_finish": "lead_free_hasl", "silkscreen_colour": "black"
        }}
    });
    let operation: Operation = serde_json::from_value(wire.clone())?;
    let Operation::Layout(LayoutCommand::SetFabrication(fabrication)) = operation else {
        return Err(Error::Invalid("decoded another operation".into()));
    };
    assert_eq!(
        serde_json::to_value(fabrication)?,
        wire["operation"]["params"]
    );
    for (key, value) in [
        ("mask_colour", serde_json::json!("orange")),
        ("pad_finish", serde_json::json!("enepig")),
        ("extra", serde_json::json!(true)),
    ] {
        let mut bad = wire.clone();
        bad["operation"]["params"][key] = value;
        assert!(serde_json::from_value::<Operation>(bad).is_err(), "{key}");
    }
    let mut missing = wire;
    if let Some(params) = missing["operation"]["params"].as_object_mut() {
        params.remove("silkscreen_colour");
    }
    assert!(serde_json::from_value::<Operation>(missing).is_err());

    let file = Engine::demo()?.to_file();
    let mut value = serde_json::to_value(&file)?;
    if let Some(project) = value["project"].as_object_mut() {
        project.remove("fabrication");
    }
    let bytes = serde_json::to_vec(&value)?;
    assert!(crate::format::decode(&bytes).is_err());
    Ok(())
}

#[test]
fn schemas_list_the_fabrication_operation_and_every_choice() -> Result<()> {
    let schemas = serde_json::to_string(&crate::schemas())?;
    for name in [
        "layout.set_fabrication",
        "mask_colour",
        "silkscreen_colour",
        "\"clear\"",
        "\"purple\"",
        "\"satin\"",
        "\"lead_free_hasl\"",
        "\"immersion_silver\"",
        "\"osp\"",
    ] {
        assert!(schemas.contains(name), "{name} missing from the schemas");
    }
    Ok(())
}

#[test]
fn a_plated_through_pad_connects_a_bottom_track() -> Result<()> {
    let mut project = fixture()?;
    for pad in &mut project.library.footprints[0].definition.pads {
        pad.kind = crate::library::PadKind::PlatedThrough;
        pad.hole = Some(crate::library::Hole {
            shape: crate::library::HoleShape::Round { diameter: 0.4 },
            plated: true,
            offset: [0.0, 0.0],
        });
    }
    assert!(
        pads(&project)
            .iter()
            .all(|p| p.on(Layer::Top) && p.on(Layer::Bottom))
    );
    apply(
        &mut project,
        LayoutCommand::Route {
            request: RouteRequest {
                layer: Layer::Bottom,
                ..request()
            },
        },
    )?;
    assert!(airwires(&project).is_empty());
    assert!(
        !checks(&project).iter().any(|f| f.code == "board.unrouted"),
        "{:?}",
        checks(&project)
    );
    Ok(())
}

#[test]
fn an_opposite_side_smd_pad_is_on_the_other_layer() -> Result<()> {
    let mut project = fixture()?;
    for pad in &mut project.library.footprints[0].definition.pads {
        pad.side = crate::library::PadSide::Bottom;
    }
    assert!(
        pads(&project)
            .iter()
            .all(|p| p.layer == Layer::Bottom && !p.through)
    );
    // A top track no longer reaches them; a bottom track does.
    apply(&mut project, LayoutCommand::Route { request: request() })?;
    assert_eq!(airwires(&project).len(), 1);
    project.tracks.clear();
    apply(
        &mut project,
        LayoutCommand::Route {
            request: RouteRequest {
                layer: Layer::Bottom,
                ..request()
            },
        },
    )?;
    assert!(airwires(&project).is_empty());
    Ok(())
}

#[test]
fn board_checks_label_pads_by_number() -> Result<()> {
    let mut project = fixture()?;
    for pad in &mut project.library.footprints[0].definition.pads {
        pad.number = format!("N{}", pad.id);
    }
    let unrouted: Vec<_> = checks(&project)
        .into_iter()
        .filter(|f| f.code == "board.unrouted")
        .collect();
    assert!(
        unrouted.iter().all(|f| f.message.contains(" pad N")),
        "{unrouted:?}"
    );
    Ok(())
}
