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

/// Every wire's drawn route, in sheet millimetres.
fn routes(engine: &Engine) -> Vec<[Point; 3]> {
    let project = engine.project();
    project
        .wires
        .iter()
        .filter_map(|w| schematic::wire_ends(project, w))
        .map(|(a, b)| schematic::wire_route(a, b))
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
        routes(&engine)
            .iter()
            .filter(|r| r[0][1] == 0.0 && r[2][1] == 0.0)
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
    let Sheet { mut engine, u, .. } = sheet()?;
    // U1.1 (-5, 0) → U3.2 (20, 20): along x to (20, 0), then up.
    connect(&mut engine, ep(u[0], 1), ep(u[2], 2))?;
    let bent = wire_between(&engine, ep(u[0], 1), ep(u[2], 2))?;
    let path = schematic::wire_route([-5.0, 0.0], [20.0, 20.0]);
    connect_to(&mut engine, ep(u[3], 2).into(), on(bent, [20.0, 10.0]))?;
    let project = engine.project();
    let junction = WireEnd::junction(project.junctions[0].id);
    let halves: Vec<[Point; 3]> = project
        .wires
        .iter()
        .filter(|w| w.id == bent || w.from == junction)
        .filter_map(|w| schematic::wire_ends(project, w))
        .map(|(a, b)| schematic::wire_route(a, b))
        .collect();
    assert_eq!(
        halves,
        vec![
            [path[0], path[1], [20.0, 10.0]],
            [[20.0, 10.0], [20.0, 10.0], path[2]]
        ]
    );
    assert_eq!(net_of(&engine, ep(u[3], 2))?, net_of(&engine, ep(u[0], 1))?);
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
    // U2.1, and a dot there (the pin plus two wires).
    connect_to(&mut engine, ep(u[3], 1).into(), on(wire, [25.0, 0.0]))?;
    assert!(engine.project().junctions.is_empty());
    assert_eq!(engine.project().wires_at(ep(u[1], 1)).count(), 2);
    assert_eq!(dots(&engine), vec![[25.0, 0.0]]);
    assert_eq!(net_of(&engine, ep(u[3], 1))?, net_of(&engine, ep(u[0], 2))?);
    // A two-wire bend junction has no dot until a third wire lands on it.
    connect_to(&mut engine, ep(u[2], 1).into(), on(wire, [15.0, 0.0]))?;
    let junction = engine.project().junctions[0].id;
    assert_eq!(dots(&engine).len(), 2);
    connect_to(
        &mut engine,
        ep(u[2], 2).into(),
        WireEnd::junction(junction).into(),
    )?;
    assert_eq!(
        engine
            .project()
            .wires_at(WireEnd::junction(junction))
            .count(),
        4
    );
    assert_eq!(net_of(&engine, ep(u[2], 2))?, net_of(&engine, ep(u[0], 2))?);
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

#[test]
fn removing_wires_tidies_junctions_into_bends_or_away() -> Result<()> {
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
    // Removing the stem leaves a two-wire bend: still one wire electrically.
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Disconnect { wire: stem }),
    )?;
    assert_eq!(engine.project().junctions.len(), 1);
    assert!(dots(&engine).is_empty());
    assert_eq!(net_of(&engine, ep(u[0], 2))?, net_of(&engine, ep(u[1], 1))?);
    assert_ne!(net_of(&engine, ep(u[2], 1))?, net_of(&engine, ep(u[0], 2))?);
    // Removing one half leaves a stub that joins nothing; it goes with its
    // junction in the same step.
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
