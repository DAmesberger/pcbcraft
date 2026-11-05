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
        layer: Layer::Top.into(),
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
            sheet: None,
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
    // The refusal names the part and the way out, and a canvas asking
    // before a drag gets the same words.
    let refused = apply(
        &mut project,
        LayoutCommand::Move {
            instance: r1,
            position: [20.0, 20.0],
            rotation_deg: 0.0,
            layer: Layer::Bottom,
        },
    )
    .err()
    .map(|e| e.to_string());
    let want = Error::Conflict("R1 is locked; unlock it to move it".into()).to_string();
    assert_eq!(refused.as_deref(), Some(want.as_str()));
    assert_eq!(
        move_refusal(&project, r1, None).map(|e| e.to_string()),
        Some(want)
    );
    assert_eq!(project, before);
    let (_, r2) = ids()?;
    assert!(move_refusal(&project, r2, None).is_none(), "R2 is free");
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

/// The pads of `designator` that DRC reports past the board outline.
fn off_board_pads(project: &Project, designator: &str) -> Vec<String> {
    checks(project)
        .into_iter()
        .filter(|f| f.code == "board.outside_outline" && f.message.starts_with(designator))
        .map(|f| f.message)
        .collect()
}

/// A part may sit off the board, anywhere in the working area: the board's
/// surroundings are its staging area. DRC reports its pads past the edge;
/// a datum outside the working area is refused and changes nothing.
#[test]
fn a_part_may_sit_off_the_board_and_drc_reports_it() -> Result<()> {
    let mut project = fixture()?;
    let (_, r2) = ids()?;
    assert!(off_board_pads(&project, "R2").is_empty());
    let mv = |position: Point| LayoutCommand::Move {
        instance: r2,
        position,
        rotation_deg: 0.0,
        layer: Layer::Top,
    };
    // Left of the board, as when staged beside it.
    apply(&mut project, mv([-30.0, 10.0]))?;
    let placed = project.placements.iter().find(|p| p.instance == r2);
    assert_eq!(placed.map(|p| p.position), Some([-30.0, 10.0]));
    assert_eq!(off_board_pads(&project, "R2").len(), 2, "both pads");
    // The working area's corner is allowed; one step past it is not.
    let [lo, hi] = WORKING_AREA_MM;
    apply(&mut project, mv([lo, hi]))?;
    let before = project.clone();
    let refused = apply(&mut project, mv([hi + 1.0, 10.0])).err();
    assert_eq!(
        refused.map(|e| e.to_string()),
        Some(
            Error::Invalid("R2 must stay within -1000 to 2000 mm on each axis".into()).to_string()
        )
    );
    assert_eq!(project, before);
    Ok(())
}

/// Shrinking the board past its parts is allowed; the parts left outside
/// are DRC errors, not a refusal.
#[test]
fn shrinking_the_board_past_parts_leaves_them_off_the_board() -> Result<()> {
    let mut project = fixture()?;
    let (r1, r2) = ids()?;
    // Only R1 (10, 10) and R2 (25, 10) are placed.
    project
        .placements
        .retain(|p| p.instance == r1 || p.instance == r2);
    project.tracks.clear();
    let before = project.placements.clone();
    // 20 mm wide: R2's datum is 5 mm past the right edge.
    apply(
        &mut project,
        LayoutCommand::SetBoardSize { size: [20.0, 30.0] },
    )?;
    assert_eq!(project.board_size, [20.0, 30.0]);
    assert_eq!(project.placements, before, "no part moved");
    assert_eq!(off_board_pads(&project, "R2").len(), 2, "both pads");
    assert!(off_board_pads(&project, "R1").is_empty());
    // The smallest board leaves both parts outside.
    apply(
        &mut project,
        LayoutCommand::SetBoardSize { size: [5.0, 5.0] },
    )?;
    assert!(!off_board_pads(&project, "R1").is_empty());
    assert_eq!(project.placements, before);
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
                    // 0.3 mm track's round cap reaches 0.15 mm before its
                    // start, so starting here it is 0.19 mm from the land.
                    start: [10.04, 10.0],
                    ..request()
                }
            }
        )
        .is_err()
    );
    assert_eq!(project, before);
    // Exactly 0.2 mm: the cap is measured exactly, not flattened.
    apply(
        &mut project,
        LayoutCommand::Route {
            request: RouteRequest {
                start: [10.05, 10.0],
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
                layer: Layer::Bottom.into(),
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
                layer: Layer::Bottom.into(),
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

/// Give every demo pad `shape` (both resistors share the footprint).
fn with_pad_shape(mut project: Project, shape: PadShape) -> Project {
    for pad in &mut project.library.footprints[0].definition.pads {
        pad.shape = shape.clone();
    }
    project
}

fn circle() -> PadShape {
    PadShape::Circle { diameter: 1.0 }
}

fn square() -> PadShape {
    PadShape::Rect { size: [1.0, 1.0] }
}

fn shared_track(start: Point, end: Point) -> LayoutCommand {
    LayoutCommand::Route {
        request: RouteRequest {
            start,
            end,
            ..request()
        },
    }
}

#[test]
fn clearance_measures_a_round_pad_not_its_extent() -> Result<()> {
    // R1 pad 1 (another net) sits at (9.2, 10). A shared-net track passes
    // its upper-right corner diagonally 0.1 mm out in x and y: 0.141 mm
    // from a square land, 0.349 mm from a round one of the same extent.
    let pass = shared_track([9.8, 10.75], [10.8, 10.75]);
    let mut square_board = with_pad_shape(fixture()?, square());
    let before = square_board.clone();
    assert!(apply(&mut square_board, pass.clone()).is_err());
    assert_eq!(square_board, before, "a rejected route changes nothing");
    let mut round_board = with_pad_shape(fixture()?, circle());
    apply(&mut round_board, pass)?;
    assert!(
        !checks(&round_board)
            .iter()
            .any(|f| f.code == "board.clearance"),
        "{:?}",
        checks(&round_board)
    );
    Ok(())
}

#[test]
fn round_pad_clearance_holds_either_side_of_the_rule() -> Result<()> {
    // A horizontal 0.3 mm track above R1 pad 1 (r 0.5 at (9.2, 10)):
    // its lower edge is 0.21 mm, then 0.19 mm, from the circle.
    let at = |gap: f64| 10.0 + 0.5 + gap + 0.15;
    let mut project = with_pad_shape(fixture()?, circle());
    let before = project.clone();
    assert!(
        apply(
            &mut project,
            shared_track([7.4, at(0.19)], [10.8, at(0.19)])
        )
        .is_err()
    );
    assert_eq!(project, before);
    apply(
        &mut project,
        shared_track([7.4, at(0.21)], [10.8, at(0.21)]),
    )?;
    let track = project.tracks[0].clone();
    let pad = pads(&project)
        .into_iter()
        .find(|p| p.net == R1_1)
        .ok_or_else(|| Error::NotFound("R1 pad 1".into()))?;
    assert!(!track_pad_clearance_conflict(
        &track,
        &pad,
        &limits::BoardRules::of(&project)
    ));
    assert!(!track_touches_pad(&track, &pad));
    Ok(())
}

#[test]
fn connectivity_follows_the_pad_outline_into_corners() -> Result<()> {
    // The track's round cap reaches R1 pad 2's corner region (tip at
    // (11.25, 10.45)): inside a square land (to 11.3, 10.5), 0.1 mm outside
    // a round one (r 0.5 about (10.8, 10); the cap's centre is 0.75 away).
    let route = shared_track([24.2, 10.45], [11.4, 10.45]);
    let mut square_board = with_pad_shape(fixture()?, square());
    apply(&mut square_board, route.clone())?;
    assert!(airwires(&square_board).is_empty());
    let mut round_board = with_pad_shape(fixture()?, circle());
    apply(&mut round_board, route)?;
    assert_eq!(airwires(&round_board).len(), 1);
    assert_eq!(
        checks(&round_board)
            .iter()
            .filter(|f| f.code == "board.unrouted")
            .count(),
        2,
        "both shared-net pads report the open connection"
    );
    Ok(())
}

#[test]
fn a_track_tangent_to_round_pads_connects_them_and_a_small_gap_does_not() -> Result<()> {
    // The track's lower edge at y = 10.5 is tangent to both shared-net
    // circles; flattening must not open that contact.
    let mut tangent = with_pad_shape(fixture()?, circle());
    apply(&mut tangent, shared_track([10.8, 10.65], [24.2, 10.65]))?;
    assert!(airwires(&tangent).is_empty(), "{:?}", checks(&tangent));
    let mut apart = with_pad_shape(fixture()?, circle());
    apply(&mut apart, shared_track([10.8, 10.66], [24.2, 10.66]))?;
    assert_eq!(airwires(&apart).len(), 1);
    Ok(())
}

#[test]
fn board_outlines_match_the_placed_pad_outlines() -> Result<()> {
    use crate::library::{CornerMask, Outline, Segment};
    let shapes = [
        PadShape::Rect { size: [1.0, 0.6] },
        PadShape::RoundRect {
            size: [1.2, 0.8],
            radius_ratio: 0.25,
            max_radius: None,
            corners: CornerMask::ALL,
        },
        PadShape::Oblong { size: [1.4, 0.7] },
        // Neither symmetric about the pad's x-axis: the bottom side must
        // mirror them, not only turn them.
        PadShape::RoundRect {
            size: [1.2, 0.8],
            radius_ratio: 0.5,
            max_radius: None,
            corners: CornerMask {
                top_left: true,
                top_right: true,
                bottom_right: false,
                bottom_left: false,
            },
        },
        PadShape::Custom {
            outline: Outline {
                start: [-0.5, -0.4],
                segments: vec![
                    Segment::Line { to: [0.5, -0.4] },
                    Segment::Arc {
                        to: [0.5, 0.4],
                        bulge: 0.5,
                    },
                    Segment::Line { to: [-0.2, 0.4] },
                    Segment::Line { to: [-0.5, -0.4] },
                ],
            },
        },
    ];
    for shape in shapes {
        let mut project = with_pad_shape(fixture()?, shape.clone());
        let footprint = &mut project.library.footprints[0].definition;
        footprint.datum.rotation_deg = 30.0;
        footprint.datum.offset = [0.3, -0.2];
        footprint.pads[0].rotation_deg = 15.0;
        apply(
            &mut project,
            LayoutCommand::Move {
                instance: ids()?.0,
                position: [20.0, 20.0],
                rotation_deg: 70.0,
                layer: Layer::Bottom,
            },
        )?;
        let placed = placed_pads(&project);
        for pad in pads(&project) {
            let exact = placed
                .iter()
                .find(|p| p.instance == pad.instance && p.pad.id == pad.pad)
                .ok_or_else(|| Error::NotFound("placed pad".into()))?;
            // The same land; the bottom-side placed outline runs the other
            // way round (it is mirrored), so compare as point sets.
            let a = pad.outline().flatten(CURVE_SLACK_MM);
            let b = exact.outline().flatten(CURVE_SLACK_MM);
            assert_eq!(a.len(), b.len(), "{shape:?}");
            for p in &a {
                assert!(
                    b.iter()
                        .any(|q| (p[0] - q[0]).abs() < 1e-9 && (p[1] - q[1]).abs() < 1e-9),
                    "{shape:?}: {p:?} is not on the placed outline"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn moved_copper_gives_the_same_airwires_as_a_moved_placement() -> Result<()> {
    let mut project = with_pad_shape(fixture()?, circle());
    apply(&mut project, shared_track([11.2, 10.0], [23.8, 10.0]))?;
    let copper = BoardCopper::of(&project);
    assert_eq!(copper.airwires(&project.tracks), airwires(&project));
    assert!(copper.airwires(&project.tracks).is_empty());
    let r2 = ids()?.1;
    for delta in [[0.0, 0.3], [0.0, 0.7], [1.0, -2.0]] {
        let mut candidate = project.clone();
        for placement in &mut candidate.placements {
            if placement.instance == r2 {
                placement.position = [
                    placement.position[0] + delta[0],
                    placement.position[1] + delta[1],
                ];
            }
        }
        let preview = copper.moved(r2, delta);
        assert_eq!(preview.pads(), BoardCopper::of(&candidate).pads());
        assert_eq!(
            preview.airwires(&candidate.tracks),
            airwires(&candidate),
            "{delta:?}"
        );
    }
    // Moved up 0.3 mm the round pad still overlaps the track's upper edge
    // (y 10.15); at 0.7 mm it clears it by 0.05 mm, where the old 1.4 mm
    // tall extent would still have connected.
    assert!(
        copper
            .moved(r2, [0.0, 0.3])
            .airwires(&project.tracks)
            .is_empty()
    );
    assert_eq!(
        copper.moved(r2, [0.0, 0.7]).airwires(&project.tracks).len(),
        1
    );
    Ok(())
}

#[test]
fn copper_ring_gaps_are_exact_for_straight_edges_and_bounded_for_curves() {
    let square = CopperRing::of_shape(&Shape::rect([0.0, 0.0], [1.0, 1.0]), false);
    let beside = CopperRing::of_shape(&Shape::rect([1.2, 0.0], [2.0, 1.0]), false);
    assert!((square.gap(&beside, 1.0) - 0.2).abs() < 1e-12);
    assert!(!square.violates_clearance(&beside), "exact boundary passes");
    let disc = |x: f64| {
        CopperRing::of_shape(
            &Shape::Circle {
                center: [x, 0.5],
                radius: 0.5,
            },
            true,
        )
    };
    // Exact gap 0.25 from the square's right edge.
    let gap = square.gap(&disc(1.75), 1.0);
    assert!((0.25..=0.25 + CURVE_SLACK_MM).contains(&gap), "{gap}");
    assert!(!square.violates_clearance(&disc(1.75)));
    assert!(square.violates_clearance(&disc(1.65)));
    assert!(square.touches(&disc(1.5)), "tangent");
    assert!(!square.touches(&disc(1.51)));
    // Bounds alone settle distant pairs.
    assert!(square.gap(&disc(9.0), 0.3) >= 0.3);
    let empty = CopperRing::of_shape(&Shape::polygon(Vec::new()), false);
    assert_eq!(square.gap(&empty, 1.0), f64::INFINITY);
    assert!(empty.inside([1.0, 1.0]));
}

fn track_set(id: Id, width: Option<f64>, layer: Option<Layer>) -> LayoutCommand {
    LayoutCommand::SetTrack {
        id,
        width,
        layer: layer.map(Into::into),
    }
}

/// `layout.track_set` changes only the width, the layer or both: the
/// track keeps its identity, net and ends.
#[test]
fn track_set_changes_width_and_layer_in_place() -> Result<()> {
    let mut project = fixture()?;
    apply(&mut project, LayoutCommand::Route { request: request() })?;
    let original = project.tracks[0].clone();
    apply(&mut project, track_set(original.id, Some(0.5), None))?;
    assert_eq!(
        project.tracks,
        vec![Track {
            width: 0.5,
            ..original.clone()
        }]
    );
    apply(
        &mut project,
        track_set(original.id, None, Some(Layer::Bottom)),
    )?;
    assert_eq!(
        project.tracks,
        vec![Track {
            width: 0.5,
            layer: Layer::Bottom.into(),
            ..original.clone()
        }]
    );
    apply(
        &mut project,
        track_set(original.id, Some(0.25), Some(Layer::Top)),
    )?;
    assert_eq!(
        project.tracks,
        vec![Track {
            width: 0.25,
            ..original
        }]
    );
    Ok(())
}

/// Every refusal leaves the project exactly as it was: nothing to change,
/// an unknown or locked track, a width out of range or not finite, and a
/// width that breaks clearance to another net's pad.
#[test]
fn track_set_refusals_change_nothing() -> Result<()> {
    let mut project = fixture()?;
    apply(&mut project, LayoutCommand::Route { request: request() })?;
    let id = project.tracks[0].id;
    let before = project.clone();
    let refused = |project: &mut Project, command| {
        let result = apply(project, command);
        assert!(result.is_err(), "{result:?}");
        result.err()
    };
    assert!(matches!(
        refused(&mut project, track_set(id, None, None)),
        Some(Error::Invalid(_))
    ));
    assert!(matches!(
        refused(&mut project, track_set(id + 999, Some(0.4), None)),
        Some(Error::NotFound(_))
    ));
    for width in [0.01, 5.5, f64::NAN, f64::INFINITY, -0.3] {
        assert!(matches!(
            refused(&mut project, track_set(id, Some(width), None)),
            Some(Error::Invalid(_))
        ));
    }
    assert_eq!(project, before);
    // Another net's tracks: one on top 2 mm above the track's centre line,
    // one on the bottom right under it.
    for (y, layer) in [(12.0, Layer::Top), (10.0, Layer::Bottom)] {
        apply(
            &mut project,
            LayoutCommand::Route {
                request: RouteRequest {
                    net: R1_1,
                    start: [14.0, y],
                    end: [20.0, y],
                    width: 0.3,
                    layer: layer.into(),
                },
            },
        )?;
    }
    let before = project.clone();
    // 3.5 mm wide, its edge is 0.1 mm from the top track. The refusal
    // names what is too close in the message style.
    let Some(Error::Conflict(text)) = refused(&mut project, track_set(id, Some(3.5), None)) else {
        return Err(Error::Invalid("a clearance conflict expected".into()));
    };
    assert!(text.starts_with("Too close to "), "{text}");
    // The refusal names the limit and where it comes from.
    assert!(
        text.ends_with(": needs 0.2 mm (Pcbcraft default)"),
        "{text}"
    );
    assert_eq!(crate::issues::style::check(&text), vec![], "{text}");
    // On the bottom it would overlap the other net's track.
    assert!(matches!(
        refused(&mut project, track_set(id, None, Some(Layer::Bottom))),
        Some(Error::Conflict(_))
    ));
    assert_eq!(project, before);
    // 2.6 mm fits: its round start cap stops 0.2 mm short of R1 pad 1's
    // land at x = 9.7 (11.2 − 1.3).
    apply(&mut project, track_set(id, Some(2.6), None))?;
    apply(
        &mut project,
        LayoutCommand::SetTrackLock { id, locked: true },
    )?;
    let locked = project.clone();
    assert!(matches!(
        refused(&mut project, track_set(id, None, Some(Layer::Bottom))),
        Some(Error::Conflict(_))
    ));
    assert_eq!(project, locked);
    Ok(())
}

/// Through the engine `layout.track_set` is one revision and one undo
/// step, refuses a stale revision, and has a typed wire form in the
/// generated schemas that CLI and MCP serve.
#[test]
fn track_set_is_one_undo_step_with_a_typed_wire_form() -> Result<()> {
    let mut engine = Engine::demo()?;
    engine.apply(engine.revision(), Operation::Layout(LayoutCommand::Sync))?;
    let (r1, r2) = ids()?;
    for (instance, position) in [(r1, [10.0, 10.0]), (r2, [25.0, 10.0])] {
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Move {
                instance,
                position,
                rotation_deg: 0.0,
                layer: Layer::Top,
            }),
        )?;
    }
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route { request: request() }),
    )?;
    let before = engine.project().clone();
    let id = before.tracks[0].id;
    let revision = engine.revision();
    let wire = serde_json::json!({
        "editor": "Layout",
        "operation": {"command": "layout.track_set",
            "params": {"id": id, "width": 0.6}}
    });
    let operation: Operation = serde_json::from_value(wire.clone())?;
    engine.apply(revision, operation.clone())?;
    assert_eq!(engine.revision(), revision + 1);
    assert_eq!(engine.project().tracks[0].width, 0.6);
    assert_eq!(engine.project().tracks[0].layer, Layer::Top);
    assert!(matches!(
        engine.apply(revision, operation),
        Err(Error::Stale { .. })
    ));
    engine.undo()?;
    assert_eq!(engine.project(), &before, "one undo step");

    let mut extra = wire;
    extra["operation"]["params"]["net"] = serde_json::json!(1);
    assert!(serde_json::from_value::<Operation>(extra).is_err());
    let schemas = serde_json::to_string(&crate::schemas())?;
    assert!(schemas.contains("layout.track_set"));
    Ok(())
}

/// A route step with a layer change is one `layout.route_apply` with its
/// tracks and via: one undo step. Unwinding it removes exactly those tracks
/// and the via in one more; a locked via refuses the unwind and nothing
/// changes, and a via too close to another net refuses the whole step.
#[test]
fn a_route_step_with_a_via_applies_and_unwinds_atomically() -> Result<()> {
    let mut engine = Engine::demo()?;
    let edits = posture::step(&posture::StepRequest {
        net: SHARED,
        width: 0.3,
        posture: posture::Posture::Diagonal,
        flipped: false,
        head: [9.2, 8.0],
        layer: CopperLayer::Top,
        via: Some((
            ViaRequest::through(SHARED, [12.0, 11.0]),
            CopperLayer::Bottom,
        )),
        to: [16.0, 13.0],
    });
    assert_eq!(edits.add.len(), 4, "two elbows of two legs");
    let first_new = engine.project().next_id;
    let revision = engine.revision();
    engine.apply(
        revision,
        Operation::Layout(LayoutCommand::ApplyRoute { edits }),
    )?;
    assert_eq!(engine.revision(), revision + 1);
    let tracks: Vec<Id> = engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.id >= first_new)
        .map(|t| t.id)
        .collect();
    let vias: Vec<Id> = engine.project().vias.iter().map(|v| v.id).collect();
    assert_eq!((tracks.len(), vias.len()), (4, 1));
    let unwind = |vias: Vec<Id>| {
        Operation::Layout(LayoutCommand::ApplyRoute {
            edits: RouteEditSet {
                remove: tracks.clone(),
                remove_vias: vias,
                ..Default::default()
            },
        })
    };
    let via = vias[0];
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetViaLock {
            id: via,
            locked: true,
        }),
    )?;
    let locked = engine.project().clone();
    assert!(engine.apply(engine.revision(), unwind(vec![via])).is_err());
    assert_eq!(engine.project(), &locked);
    engine.undo()?;
    assert!(
        engine
            .apply(engine.revision(), unwind(vec![via, via]))
            .is_err()
    );
    engine.apply(engine.revision(), unwind(vec![via]))?;
    assert!(engine.project().vias.is_empty());
    assert!(engine.project().tracks.iter().all(|t| t.id < first_new));
    // One undo brings the whole step back.
    engine.undo()?;
    assert_eq!(engine.project().vias.len(), 1);
    engine.redo()?;
    let refused = posture::step(&posture::StepRequest {
        net: SHARED,
        width: 0.3,
        posture: posture::Posture::Free,
        flipped: false,
        head: [9.2, 8.0],
        layer: CopperLayer::Top,
        via: Some((ViaRequest::through(SHARED, [6.8, 8.0]), CopperLayer::Bottom)),
        to: [6.8, 12.0],
    });
    let before = engine.project().clone();
    assert!(
        engine
            .apply(
                engine.revision(),
                Operation::Layout(LayoutCommand::ApplyRoute { edits: refused })
            )
            .is_err()
    );
    assert_eq!(engine.project(), &before);
    Ok(())
}

/// The Route tool's width and via presets come from the rules: the net's
/// own values, the defaults and every class's, never below the minimums.
#[test]
fn route_presets_follow_the_rules_and_classes() -> Result<()> {
    let mut project = Engine::demo()?.project().clone();
    assert_eq!(route_widths(&project, SHARED), vec![0.3]);
    assert_eq!(via_sizes(&project, SHARED), vec![(0.3, 0.6)]);
    project
        .rules
        .net_classes
        .push(crate::rules::NetClass::from_spec(
            crate::rules::NetClassId(1),
            crate::rules::NetClassSpec {
                name: "Power".into(),
                track_width: Some(0.8),
                via_drill: Some(0.4),
                via_diameter: Some(0.9),
                ..Default::default()
            },
        ));
    project
        .rules
        .net_classes
        .push(crate::rules::NetClass::from_spec(
            crate::rules::NetClassId(2),
            crate::rules::NetClassSpec {
                name: "Bad via".into(),
                track_width: Some(0.01),
                via_drill: Some(0.3),
                via_diameter: Some(0.35),
                ..Default::default()
            },
        ));
    assert_eq!(route_widths(&project, SHARED), vec![0.3, 0.8]);
    assert_eq!(via_sizes(&project, SHARED), vec![(0.3, 0.6), (0.4, 0.9)]);
    assert!(via_size_fits(&project, 0.3, 0.4).is_err());
    assert!(via_size_fits(&project, 0.05, 0.6).is_err());
    assert!(via_size_fits(&project, 0.3, 0.5).is_ok());
    Ok(())
}
