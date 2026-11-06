//! Wire-to-wire connectivity: a wire end landing on another wire's segment
//! splits it at a junction and joins the nets (a T); a plain crossing joins
//! nothing; wires ending on one terminal or junction meet end on end.
//! Junction dots are derived where three or more conductors meet.
use super::support::*;
use crate::schematic::{self, SchematicCommand, WirePoint, WireTarget};
use crate::{Engine, Error, Id, Operation, Point, Result, WireEnd, format};
use serde_json::json;

/// Two passive terminals, pins at x = ±5 mm of the unit origin.
fn resistor(engine: &mut Engine) -> Result<Fixture> {
    component(
        engine,
        &Spec {
            name: "Two terminal",
            terminals: vec![(1, "A", passive()), (2, "B", passive())],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: false,
        },
    )
}

/// U1 and U2 on y = 0 wired U1.2 (5, 0) → U2.1 (25, 0), plus U3 above
/// (pins at (10, 20) and (20, 20)) and U4 below ((10, -20) and (20, -20)).
struct Sheet {
    engine: Engine,
    u: [Id; 4],
    wire: Id,
}

fn sheet() -> Result<Sheet> {
    let mut engine = Engine::default();
    let fixture = resistor(&mut engine)?;
    let mut u = [0; 4];
    for (i, (name, at)) in [
        ("U1", [0.0, 0.0]),
        ("U2", [30.0, 0.0]),
        ("U3", [15.0, 20.0]),
        ("U4", [15.0, -20.0]),
    ]
    .into_iter()
    .enumerate()
    {
        u[i] = place(&mut engine, &fixture, name, 1, at)?.0;
    }
    connect(&mut engine, ep(u[0], 2), ep(u[1], 1))?;
    let wire = wire_between(&engine, ep(u[0], 2), ep(u[1], 1))?;
    Ok(Sheet { engine, u, wire })
}

fn connect_to(engine: &mut Engine, from: WireTarget, to: WireTarget) -> Result<()> {
    apply(
        engine,
        Operation::Schematic(SchematicCommand::Connect { from, to }),
    )
    .map(|_| ())
}

fn on(wire: Id, at: Point) -> WireTarget {
    WireTarget::OnWire(WirePoint { wire, at })
}

/// Every wire's drawn path, in sheet millimetres.
fn paths(engine: &Engine) -> Vec<Vec<Point>> {
    let project = engine.project();
    project
        .wires
        .iter()
        .filter_map(|w| schematic::wire_path(project, w))
        .collect()
}

fn dots(engine: &Engine) -> Vec<Point> {
    schematic::junction_dots(engine.project())
        .into_iter()
        .map(|(_, p)| p)
        .collect()
}

#[test]
fn a_wire_ending_on_a_segment_splits_it_at_a_junction_and_joins_the_net() -> Result<()> {
    let Sheet {
        mut engine,
        u,
        wire,
    } = sheet()?;
    let before = engine.document().clone();
    connect_to(&mut engine, ep(u[2], 1).into(), on(wire, [15.0, 0.0]))?;
    let project = engine.project();
    assert_eq!(project.junctions.len(), 1);
    let junction = project.junctions[0].clone();
    assert_eq!(junction.position, [15.0, 0.0]);
    let at = WireEnd::junction(junction.id);
    assert_eq!(project.wires.len(), 3);
    assert_eq!(project.wires_at(at).count(), 3);
    // The split wire keeps its ID up to the junction.
    let kept = project
        .wire(wire)
        .ok_or_else(|| Error::NotFound("wire".into()))?;
    assert_eq!((kept.from, kept.to), (ep(u[0], 2).into(), at));
    // The T joins all three terminals into one net.
    let net = net_of(&engine, ep(u[0], 2))?;
    for end in [ep(u[1], 1), ep(u[2], 1)] {
        assert_eq!(net_of(&engine, end)?, net);
    }
    assert_ne!(net_of(&engine, ep(u[3], 1))?, net);
    // One solid dot, on the junction; the two-wire pin ends get none.
    assert_eq!(dots(&engine), vec![[15.0, 0.0]]);
    // The drawn path of the split wire is unchanged: both halves lie on y = 0.
    assert!(
        paths(&engine)
            .iter()
            .filter(|r| r.iter().all(|p| p[1] == 0.0))
            .count()
            == 2
    );
    // One undo step restores the unsplit wire and no junction.
    engine.undo()?;
    assert_eq!(engine.document(), &before);
    Ok(())
}

#[test]
fn a_t_on_the_vertical_leg_of_a_bent_wire_keeps_its_path() -> Result<()> {
    let Sheet { mut engine, .. } = sheet()?;
    // Away from the others (whose pins a new wire must not run over):
    // U5.2 (-15, 30) → U6.1 (25, 50), along x to (25, 30), then up; U7.1
    // (40, 40) lands on that vertical leg.
    let fixture = resistor(&mut engine)?;
    let mut u = [0; 3];
    for (i, (name, at)) in [
        ("U5", [-20.0, 30.0]),
        ("U6", [30.0, 50.0]),
        ("U7", [45.0, 40.0]),
    ]
    .into_iter()
    .enumerate()
    {
        u[i] = place(&mut engine, &fixture, name, 1, at)?.0;
    }
    connect(&mut engine, ep(u[0], 2), ep(u[1], 1))?;
    let bent = wire_between(&engine, ep(u[0], 2), ep(u[1], 1))?;
    // One wire, its bend a corner.
    assert_eq!(
        engine.project().wire(bent).map(|w| w.corners.clone()),
        Some(vec![crate::model::Corner { at: [25.0, 30.0] }])
    );
    connect_to(&mut engine, ep(u[2], 1).into(), on(bent, [25.0, 40.0]))?;
    let project = engine.project();
    let junction = WireEnd::junction(project.junctions[0].id);
    let halves: Vec<Vec<Point>> = project
        .wires
        .iter()
        .filter(|w| w.id == bent || w.from == junction)
        .filter_map(|w| schematic::wire_path(project, w))
        .collect();
    assert_eq!(
        halves,
        vec![
            vec![[-15.0, 30.0], [25.0, 30.0], [25.0, 40.0]],
            vec![[25.0, 40.0], [25.0, 50.0]]
        ]
    );
    assert_eq!(net_of(&engine, ep(u[2], 1))?, net_of(&engine, ep(u[0], 2))?);
    Ok(())
}

#[test]
fn a_plain_crossing_joins_nothing_and_draws_no_dot() -> Result<()> {
    let Sheet { mut engine, u, .. } = sheet()?;
    // U4.2 (20, -20) → U3.2 (20, 20) crosses U1.2–U2.1 at (20, 0).
    connect(&mut engine, ep(u[3], 2), ep(u[2], 2))?;
    assert!(engine.project().junctions.is_empty());
    assert_ne!(net_of(&engine, ep(u[3], 2))?, net_of(&engine, ep(u[0], 2))?);
    assert_eq!(net_of(&engine, ep(u[3], 2))?, net_of(&engine, ep(u[2], 2))?);
    assert!(dots(&engine).is_empty());
    Ok(())
}

#[test]
fn ends_on_one_terminal_or_junction_meet_and_three_conductors_get_a_dot() -> Result<()> {
    let Sheet {
        mut engine,
        u,
        wire,
    } = sheet()?;
    // A point at a wire's end is that end: no junction, a second wire on
    // U2.1, and a dot there (the pin plus two wires). The wires below come
    // from pins whose horizontal-first L runs over no other pin.
    connect_to(&mut engine, ep(u[2], 2).into(), on(wire, [25.0, 0.0]))?;
    assert!(engine.project().junctions.is_empty());
    assert_eq!(engine.project().wires_at(ep(u[1], 1)).count(), 2);
    assert_eq!(dots(&engine), vec![[25.0, 0.0]]);
    assert_eq!(net_of(&engine, ep(u[2], 2))?, net_of(&engine, ep(u[0], 2))?);
    // A T makes a junction with a dot; a fourth wire lands on it too.
    connect_to(&mut engine, ep(u[2], 1).into(), on(wire, [15.0, 0.0]))?;
    let junction = engine.project().junctions[0].id;
    assert_eq!(dots(&engine).len(), 2);
    connect_to(
        &mut engine,
        ep(u[3], 2).into(),
        WireEnd::junction(junction).into(),
    )?;
    assert_eq!(
        engine
            .project()
            .wires_at(WireEnd::junction(junction))
            .count(),
        4
    );
    assert_eq!(net_of(&engine, ep(u[3], 2))?, net_of(&engine, ep(u[0], 2))?);
    Ok(())
}

#[test]
fn refused_targets_leave_the_document_unchanged() -> Result<()> {
    let Sheet {
        mut engine,
        u,
        wire,
    } = sheet()?;
    let kept = Snapshot::of(&engine);
    for (from, to, code) in [
        // Off the wire's path.
        (ep(u[2], 1).into(), on(wire, [15.0, 3.0]), "invalid_input"),
        // Both ends on the same wire.
        (
            on(wire, [10.0, 0.0]),
            on(wire, [20.0, 0.0]),
            "invalid_input",
        ),
        (ep(u[2], 1).into(), on(999, [15.0, 0.0]), "not_found"),
        (
            ep(u[2], 1).into(),
            WireEnd::junction(999).into(),
            "not_found",
        ),
        (
            ep(u[2], 1).into(),
            on(wire, [f64::NAN, 0.0]),
            "invalid_input",
        ),
        // Splitting succeeds before the terminal check refuses: still atomic.
        (ep(999, 1).into(), on(wire, [15.0, 0.0]), "not_found"),
    ] {
        let result = connect_to(&mut engine, from, to);
        assert!(result.is_err(), "{from:?} → {to:?}");
        let got = result.err().map(|e| e.code());
        assert_eq!(got, Some(code), "{from:?} → {to:?}");
        kept.assert_kept(&engine, "refused connect");
        assert!(engine.project().junctions.is_empty());
    }
    Ok(())
}

/// Before wires were paths, the stem's removal left a two-wire junction
/// as a bend and the second removal deleted the stub with it. Now a junction
/// with two wires becomes a corner of one path, and a path keeps its ends.
#[test]
fn removing_wires_tidies_junctions_into_corners_or_away() -> Result<()> {
    let Sheet {
        mut engine,
        u,
        wire,
    } = sheet()?;
    connect_to(&mut engine, ep(u[2], 1).into(), on(wire, [15.0, 0.0]))?;
    let stem = engine
        .project()
        .wires_at(ep(u[2], 1))
        .next()
        .map(|w| w.id)
        .ok_or_else(|| Error::NotFound("stem".into()))?;
    // Removing the stem leaves the two halves, which merge back into the
    // one wire (its in-line corner dropped).
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Disconnect { wire: stem }),
    )?;
    assert!(engine.project().junctions.is_empty());
    assert_eq!(engine.project().wires.len(), 1);
    assert_eq!(
        engine
            .project()
            .wire(wire)
            .map(|w| (w.from, w.to, w.corners.len())),
        Some((ep(u[0], 2).into(), ep(u[1], 1).into(), 0))
    );
    assert!(dots(&engine).is_empty());
    assert_eq!(net_of(&engine, ep(u[0], 2))?, net_of(&engine, ep(u[1], 1))?);
    assert_ne!(net_of(&engine, ep(u[2], 1))?, net_of(&engine, ep(u[0], 2))?);
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Disconnect { wire }),
    )?;
    assert!(engine.project().junctions.is_empty());
    assert!(engine.project().wires.is_empty());
    assert_ne!(net_of(&engine, ep(u[0], 2))?, net_of(&engine, ep(u[1], 1))?);
    Ok(())
}

#[test]
fn junction_wires_round_trip_and_the_command_json_names_targets() -> Result<()> {
    let Sheet {
        mut engine,
        u,
        wire,
    } = sheet()?;
    let command: SchematicCommand = serde_json::from_value(json!({
        "command": "schematic.connect",
        "params": {
            "from": { "instance": u[2], "terminal": 1 },
            "to": { "wire": wire, "at": [15.0, 0.0] }
        }
    }))?;
    apply(&mut engine, Operation::Schematic(command))?;
    let junction = engine.project().junctions[0].id;
    let saved = serde_json::to_value(engine.project())?;
    assert_eq!(
        saved["junctions"],
        json!([{ "id": junction, "position": [15.0, 0.0] }])
    );
    assert!(
        saved["wires"]
            .as_array()
            .is_some_and(|w| w.iter().any(|w| w["to"] == json!({ "junction": junction })))
    );
    let bytes = format::encode(&engine.to_file())?;
    let opened = Engine::from_file(format::decode(&bytes)?)?;
    assert_eq!(opened.project(), engine.project());
    // A junction end with an unknown field is refused, not dropped.
    let bad: std::result::Result<WireEnd, _> =
        serde_json::from_value(json!({ "junction": 1, "x": 0 }));
    assert!(bad.is_err());
    Ok(())
}

fn free(p: Point) -> Result<WireTarget> {
    crate::model::FreeEnd::new(p)
        .map(WireTarget::Free)
        .ok_or_else(|| Error::Invalid("off the sheet".into()))
}

/// `wire.draw` from `from` through `corners` to `to`.
fn draw(engine: &mut Engine, from: WireTarget, corners: &[Point], to: WireTarget) -> Result<()> {
    apply(
        engine,
        Operation::Schematic(SchematicCommand::DrawWire {
            sheet: None,
            from,
            corners: corners
                .iter()
                .map(|at| crate::model::Corner { at: *at })
                .collect(),
            to,
        }),
    )
    .map(|_| ())
}

/// The objects of the free-end findings.
fn free_ends(engine: &Engine) -> Vec<crate::ObjectRef> {
    crate::erc::checks(engine.project())
        .into_iter()
        .filter(|f| f.code == "erc.wire_end_free")
        .flat_map(|f| f.objects)
        .collect()
}

#[test]
fn wire_draw_stores_corners_and_a_free_end_that_saves_and_warns() -> Result<()> {
    let Sheet { mut engine, u, .. } = sheet()?;
    let before = engine.document().clone();
    // From U3.1 (10, 20) up, then right, ending in empty space.
    draw(
        &mut engine,
        ep(u[2], 1).into(),
        &[[10.0, 30.0]],
        free([40.0, 30.0])?,
    )?;
    let project = engine.project();
    assert_eq!(project.wires.len(), 2);
    let wire = project
        .wires_at(ep(u[2], 1))
        .next()
        .cloned()
        .ok_or_else(|| Error::NotFound("wire".into()))?;
    assert_eq!(
        schematic::wire_path(project, &wire),
        Some(vec![[10.0, 20.0], [10.0, 30.0], [40.0, 30.0]])
    );
    assert_eq!(
        free_ends(&engine),
        vec![crate::ObjectRef::Wire { id: wire.id }]
    );
    // Saved as `{"free": [x, y]}` and read back unchanged.
    let saved = serde_json::to_value(engine.project())?;
    assert!(saved["wires"].as_array().is_some_and(|w| w.iter().any(|w| {
        w["to"] == json!({ "free": [40.0, 30.0] })
            && w["corners"] == json!([{ "at": [10.0, 30.0] }])
    })));
    let bytes = format::encode(&engine.to_file())?;
    let opened = Engine::from_file(format::decode(&bytes)?)?;
    assert_eq!(opened.project(), engine.project());
    // One undo step takes the whole wire back.
    engine.undo()?;
    assert_eq!(engine.document(), &before);
    Ok(())
}

/// The tidy rules for wire ends (docs/schematic-wires.md, "Connectivity"),
/// one drawing each.
#[test]
fn free_ends_join_paths_land_as_ts_and_crossings_stay_apart() -> Result<()> {
    // Free ends that meet join into one path, with a corner there.
    let Sheet { mut engine, u, .. } = sheet()?;
    draw(&mut engine, ep(u[2], 1).into(), &[], free([10.0, 30.0])?)?;
    draw(&mut engine, free([10.0, 30.0])?, &[], free([40.0, 30.0])?)?;
    draw(
        &mut engine,
        free([40.0, 30.0])?,
        &[[40.0, 0.0]],
        ep(u[1], 2).into(),
    )?;
    let project = engine.project();
    assert_eq!(project.wires.len(), 2);
    let joined = project
        .wires_at(ep(u[2], 1))
        .next()
        .ok_or_else(|| Error::NotFound("wire".into()))?;
    assert_eq!(joined.to, ep(u[1], 2).into());
    assert_eq!(
        schematic::wire_path(project, joined),
        Some(vec![
            [10.0, 20.0],
            [10.0, 30.0],
            [40.0, 30.0],
            [40.0, 0.0],
            [35.0, 0.0]
        ])
    );
    assert!(free_ends(&engine).is_empty());
    assert_eq!(net_of(&engine, ep(u[2], 1))?, net_of(&engine, ep(u[1], 2))?);

    // A free end on another wire's segment makes a T with a dot.
    let Sheet {
        mut engine,
        u,
        wire,
    } = sheet()?;
    draw(&mut engine, ep(u[2], 1).into(), &[], free([10.0, 0.0])?)?;
    let project = engine.project();
    assert_eq!(project.junctions.len(), 1);
    assert_eq!(project.junctions[0].position, [10.0, 0.0]);
    assert_eq!(project.wires.len(), 3);
    assert!(project.wire(wire).is_some(), "the split wire keeps its ID");
    assert_eq!(dots(&engine), vec![[10.0, 0.0]]);
    assert_eq!(net_of(&engine, ep(u[2], 1))?, net_of(&engine, ep(u[0], 2))?);
    assert!(free_ends(&engine).is_empty());

    // A free end on another wire's corner makes a T there too.
    draw(
        &mut engine,
        free([50.0, 10.0])?,
        &[[60.0, 10.0]],
        free([60.0, 20.0])?,
    )?;
    draw(&mut engine, free([70.0, 10.0])?, &[], free([60.0, 10.0])?)?;
    assert_eq!(dots(&engine), vec![[10.0, 0.0], [60.0, 10.0]]);
    assert_eq!(free_ends(&engine).len(), 3);

    // A free end drawn onto a pin attaches to it.
    draw(&mut engine, free([0.0, 30.0])?, &[], free([20.0, 30.0])?)?;
    draw(&mut engine, free([20.0, 30.0])?, &[], free([20.0, 20.0])?)?;
    let project = engine.project();
    let on_pin = project
        .wires_at(ep(u[2], 2))
        .next()
        .ok_or_else(|| Error::NotFound("wire on U3.2".into()))?;
    assert_eq!(on_pin.corners.len(), 1, "the free ends at (20, 30) joined");
    assert_eq!(free_ends(&engine).len(), 4);

    // A wire crossing another joins nothing.
    let Sheet { mut engine, u, .. } = sheet()?;
    draw(&mut engine, ep(u[3], 2).into(), &[], ep(u[2], 2).into())?;
    assert!(engine.project().junctions.is_empty());
    assert!(dots(&engine).is_empty());
    assert_ne!(net_of(&engine, ep(u[3], 2))?, net_of(&engine, ep(u[0], 2))?);
    Ok(())
}

/// Founder decision 2: a junction left with one wire keeps a free end
/// there, and ERC says so. Before, the stub was deleted silently.
#[test]
fn a_junction_left_with_one_wire_leaves_a_free_end_that_erc_reports() -> Result<()> {
    let Sheet { mut engine, u, .. } = sheet()?;
    let label = match apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "OUT".into(),
            kind: crate::model::LabelKind::Local,
            position: [50.0, 0.0],
            rotation: Default::default(),
            attach: None,
        }),
    )? {
        crate::Outcome::LabelPlaced { label } => label,
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    connect_to(
        &mut engine,
        ep(u[1], 2).into(),
        WireEnd::label(label).into(),
    )?;
    assert!(free_ends(&engine).is_empty());
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::RemoveLabel { label }),
    )?;
    let project = engine.project();
    assert!(project.junctions.is_empty());
    let wire = project
        .wires_at(ep(u[1], 2))
        .next()
        .ok_or_else(|| Error::NotFound("wire".into()))?;
    assert_eq!(wire.to.free_point(), Some([50.0, 0.0]));
    assert_eq!(
        free_ends(&engine),
        vec![crate::ObjectRef::Wire { id: wire.id }]
    );
    Ok(())
}

#[test]
fn refused_wire_draws_leave_the_document_unchanged() -> Result<()> {
    let Sheet {
        mut engine,
        u,
        wire,
    } = sheet()?;
    let kept = Snapshot::of(&engine);
    assert_eq!(schematic::MAX_CORNERS, 256);
    let many: Vec<Point> = (0..=256u32)
        .map(|i| [f64::from(i % 2) * 5.0 + 50.0, f64::from(i)])
        .collect();
    for (from, corners, to, code) in [
        (
            free([50.0, 0.0])?,
            many,
            free([60.0, 0.0])?,
            "limit_exceeded",
        ),
        (
            ep(u[2], 1).into(),
            vec![[f64::NAN, 0.0]],
            free([60.0, 0.0])?,
            "invalid_input",
        ),
        (
            free([50.0, 0.0])?,
            vec![],
            free([50.0, 0.0])?,
            "invalid_input",
        ),
        (
            on(wire, [10.0, 0.0]),
            vec![[10.0, 10.0], [20.0, 10.0]],
            on(wire, [20.0, 0.0]),
            "invalid_input",
        ),
        (
            ep(u[2], 1).into(),
            vec![],
            on(wire, [15.0, 3.0]),
            "invalid_input",
        ),
    ] {
        let result = draw(&mut engine, from, &corners, to);
        assert_eq!(
            result.err().map(|e| e.code()),
            Some(code),
            "{from:?} → {to:?}"
        );
        kept.assert_kept(&engine, "refused wire.draw");
    }
    // `schematic.connect` joins connection points only.
    let result = connect_to(&mut engine, ep(u[2], 1).into(), free([60.0, 0.0])?);
    assert_eq!(result.err().map(|e| e.code()), Some("invalid_input"));
    kept.assert_kept(&engine, "refused connect");
    Ok(())
}
