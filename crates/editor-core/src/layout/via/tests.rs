use super::*;
use crate::layout::LayoutCommand;
use crate::model::Stackup;
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

#[test]
fn electrical_changes_to_a_net_with_vias_are_reviewed_updates() -> Result<()> {
    use crate::updates::{ApplyUpdate, Change};
    let mut engine = Engine::demo()?;
    let net = shared(&engine)?;
    let Outcome::ViaAdded { via } = layout(
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
    assert_eq!(plan.conflicting_vias, [via]);
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
    assert!(engine.project().vias.is_empty());
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
