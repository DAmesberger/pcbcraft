//! Rubber-banding through the public commands (docs/schematic-wires.md,
//! slice 2): when a part, label or sheet symbol moves, turns or mirrors, the
//! wires on it follow in the same undo step. The segment on the moved end
//! stays horizontal or vertical, the corner next to it slides, a corner is
//! added only where a straight segment cannot stay straight, and the nets do
//! not change. The canvas previews a drag with the same core function.
use super::support::*;
use crate::library::Rotation;
use crate::model::{LabelKind, PortShape, SheetSide};
use crate::schematic::{self, SchematicCommand as C, WireTarget, wire_edit};
use crate::{Engine, Error, Id, Operation, Outcome, Point, Result};

fn two_pin(engine: &mut Engine) -> Result<Fixture> {
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

/// Pin 2 off the unit's x-axis, so a mirror moves it.
fn offset_pin(engine: &mut Engine) -> Result<Fixture> {
    component(
        engine,
        &Spec {
            name: "Offset pin",
            terminals: vec![(1, "A", passive()), (2, "B", passive())],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 5.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: false,
        },
    )
}

fn run(engine: &mut Engine, command: C) -> Result<Outcome> {
    apply(engine, Operation::Schematic(command))
}

/// The one wire's corners.
fn corners(engine: &Engine) -> Result<Vec<Point>> {
    match &engine.project().wires[..] {
        [wire] => Ok(wire.corners.iter().map(|c| c.at).collect()),
        wires => Err(Error::Invalid(format!("expected one wire, got {wires:?}"))),
    }
}

/// Every segment of every wire is horizontal or vertical.
fn orthogonal(engine: &Engine) -> bool {
    let project = engine.project();
    project.wires.iter().all(|w| {
        schematic::wire_path(project, w).is_some_and(|path| {
            path.windows(2)
                .all(|s| (s[0][0] - s[1][0]).abs() < 1e-9 || (s[0][1] - s[1][1]).abs() < 1e-9)
        })
    })
}

/// Each wire's drawn path, by ID.
fn paths(engine: &Engine) -> Vec<(Id, Vec<Point>)> {
    let project = engine.project();
    project
        .wires
        .iter()
        .filter_map(|w| Some((w.id, schematic::wire_path(project, w)?)))
        .collect()
}

/// What the canvas previews for a translation by `d` of the objects whose
/// wire ends are listed in `moving`: each wire's path through
/// `wire_edit::follow` with the moved ends shifted.
fn preview(engine: &Engine, moving: &[crate::WireEnd], d: Point) -> Vec<(Id, Vec<Point>)> {
    let project = engine.project();
    let shift = |end, p: Point| {
        if moving.contains(&end) {
            [p[0] + d[0], p[1] + d[1]]
        } else {
            p
        }
    };
    project
        .wires
        .iter()
        .filter_map(|w| {
            let path = schematic::wire_path(project, w)?;
            let (a, b) = (*path.first()?, *path.last()?);
            Some((
                w.id,
                wire_edit::follow(&path, shift(w.from, a), shift(w.to, b)),
            ))
        })
        .collect()
}

/// U1 at the origin wired from its pin 2 (5, 0) to pin 1 of U2 at `u2`,
/// then U1 moved to `to`: the wire's corners, in one undo step, with nets
/// and the preview unchanged from the result.
#[test]
fn a_moved_part_keeps_its_wires_orthogonal() -> Result<()> {
    // (case, U2 at, corners first, U1 moved to, corners after)
    type Case = (&'static str, Point, Vec<Point>, Point, Vec<Point>);
    let cases: Vec<Case> = vec![
        (
            "horizontal: the first segment stretches",
            [40.0, 20.0],
            vec![[35.0, 0.0]],
            [-10.0, 0.0],
            vec![[35.0, 0.0]],
        ),
        (
            "vertical: the corner slides up the second segment",
            [40.0, 20.0],
            vec![[35.0, 0.0]],
            [0.0, 5.0],
            vec![[35.0, 5.0]],
        ),
        (
            "diagonal: the corner slides, nothing is added",
            [40.0, 20.0],
            vec![[35.0, 0.0]],
            [-5.0, 10.0],
            vec![[35.0, 10.0]],
        ),
        (
            "a straight wire moved off its line gets one corner",
            [40.0, 0.0],
            vec![],
            [0.0, 10.0],
            vec![[35.0, 10.0]],
        ),
        (
            "a straight wire moved along its line stays straight",
            [40.0, 0.0],
            vec![],
            [-7.5, 0.0],
            vec![],
        ),
    ];
    for (case, u2_at, first, to, after) in cases {
        let mut engine = Engine::default();
        let f = two_pin(&mut engine)?;
        let (u1, occurrence) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
        let (u2, _) = place(&mut engine, &f, "U2", 1, u2_at)?;
        connect(&mut engine, ep(u1, 2), ep(u2, 1))?;
        assert_eq!(corners(&engine)?, first, "{case}: before");
        let wires = engine.project().wires.clone();
        let nets = engine.project().nets.clone();
        let expected = preview(&engine, &[ep(u1, 1).into(), ep(u1, 2).into()], to);
        run(
            &mut engine,
            C::MoveUnit {
                occurrence,
                position: to,
            },
        )?;
        assert_eq!(corners(&engine)?, after, "{case}");
        assert!(orthogonal(&engine), "{case}: {:?}", paths(&engine));
        assert_eq!(
            paths(&engine),
            expected,
            "{case}: the preview is the result"
        );
        assert_eq!(engine.project().nets, nets, "{case}: nets");
        engine.undo()?;
        assert_eq!(engine.project().wires, wires, "{case}: one undo");
    }
    Ok(())
}

#[test]
fn a_turned_or_mirrored_part_keeps_its_wires_orthogonal() -> Result<()> {
    // (case, rotation, mirror)
    let cases = [
        ("turned a quarter", Rotation::R90, false),
        ("turned three quarters", Rotation::R270, false),
        ("mirrored", Rotation::R0, true),
    ];
    for (case, rotation, mirror) in cases {
        let mut engine = Engine::default();
        let f = offset_pin(&mut engine)?;
        let g = two_pin(&mut engine)?;
        let (u1, occurrence) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
        let (u2, _) = place(&mut engine, &g, "U2", 1, [40.0, 25.0])?;
        connect(&mut engine, ep(u1, 2), ep(u2, 1))?;
        assert_eq!(corners(&engine)?, vec![[35.0, 5.0]], "{case}: before");
        let wires = engine.project().wires.clone();
        let nets = engine.project().nets.clone();
        run(
            &mut engine,
            C::OrientUnit {
                occurrence,
                rotation,
                mirror,
            },
        )?;
        let pin = schematic::pin_position(engine.project(), ep(u1, 2))
            .ok_or(Error::NotFound("pin".into()))?;
        assert_ne!(pin, [5.0, 5.0], "{case}: the pin moved");
        // The pin's segment stays horizontal; the corner slides to it.
        assert_eq!(corners(&engine)?, vec![[35.0, pin[1]]], "{case}");
        assert!(orthogonal(&engine), "{case}: {:?}", paths(&engine));
        assert_eq!(engine.project().nets, nets, "{case}: nets");
        engine.undo()?;
        assert_eq!(engine.project().wires, wires, "{case}: one undo");
    }
    Ok(())
}

#[test]
fn a_moved_label_drags_its_wire_along() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    let label = match run(
        &mut engine,
        C::PlaceLabel {
            sheet: None,
            name: "SIG".into(),
            kind: LabelKind::Global {
                shape: PortShape::Bidirectional,
            },
            position: [20.0, 10.0],
            rotation: Rotation::R0,
            attach: None,
        },
    )? {
        Outcome::LabelPlaced { label } => label,
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    run(
        &mut engine,
        C::Connect {
            from: ep(u1, 2).into(),
            to: WireTarget::Label(crate::model::LabelRef { label }),
        },
    )?;
    assert_eq!(corners(&engine)?, vec![[20.0, 0.0]]);
    let wires = engine.project().wires.clone();
    let nets = engine.project().nets.clone();
    let d = [5.0, 5.0];
    let expected = preview(&engine, &[crate::WireEnd::label(label)], d);
    run(
        &mut engine,
        C::MoveLabel {
            label,
            position: [25.0, 15.0],
        },
    )?;
    // The label's vertical segment stays vertical; its corner slides.
    assert_eq!(corners(&engine)?, vec![[25.0, 0.0]]);
    assert!(orthogonal(&engine));
    assert_eq!(paths(&engine), expected, "the preview is the result");
    assert_eq!(engine.project().nets, nets);
    engine.undo()?;
    assert_eq!(engine.project().wires, wires);
    Ok(())
}

#[test]
fn a_moved_sheet_symbol_drags_the_wires_on_all_its_pins() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [20.0, -35.0])?;
    let (u2, _) = place(&mut engine, &f, "U2", 1, [20.0, -50.0])?;
    let symbol = match run(
        &mut engine,
        C::PlaceSheetSymbol {
            sheet: None,
            child: None,
            name: Some("Sensor".into()),
            position: [60.0, -30.0],
            size: [30.0, 20.0],
        },
    )? {
        Outcome::SheetSymbolPlaced { symbol, .. } => symbol,
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    let mut pins = Vec::new();
    for (name, offset) in [("A", 5.0), ("B", 15.0)] {
        match run(
            &mut engine,
            C::AddSheetPin {
                symbol,
                name: name.into(),
                shape: PortShape::Input,
                side: SheetSide::Left,
                offset,
            },
        )? {
            Outcome::SheetPinsAdded { pins: added } => pins.extend(added),
            other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
        }
    }
    let [a, b] = pins[..] else {
        return Err(Error::Invalid(format!("pins {pins:?}")));
    };
    let sheet_pin = |pin| WireTarget::SheetPin(crate::model::SheetPinRef { sheet_pin: pin });
    for (from, pin) in [(ep(u1, 2), a), (ep(u2, 2), b)] {
        run(
            &mut engine,
            C::Connect {
                from: from.into(),
                to: sheet_pin(pin),
            },
        )?;
    }
    let corners_of = |engine: &Engine, from: crate::WireEnd| -> Vec<Vec<Point>> {
        engine
            .project()
            .wires_at(from)
            .map(|w| w.corners.iter().map(|c| c.at).collect())
            .collect()
    };
    // U1.2 (25, −35) runs straight to A (60, −35); U2.2 (25, −50) turns up
    // to B (60, −45).
    assert_eq!(
        corners_of(&engine, ep(u1, 2).into()),
        vec![Vec::<Point>::new()]
    );
    assert_eq!(
        corners_of(&engine, ep(u2, 2).into()),
        vec![vec![[60.0, -50.0]]]
    );
    let wires = engine.project().wires.clone();
    let nets = engine.project().nets.clone();
    let d = [10.0, 5.0];
    let moving = [crate::WireEnd::sheet_pin(a), crate::WireEnd::sheet_pin(b)];
    let expected = preview(&engine, &moving, d);
    run(
        &mut engine,
        C::MoveSheetSymbol {
            symbol,
            position: [70.0, -25.0],
        },
    )?;
    // A straight wire steps once, keeping A's segment horizontal; the
    // other's corner slides along its first segment.
    assert_eq!(
        corners_of(&engine, ep(u1, 2).into()),
        vec![vec![[25.0, -30.0]]]
    );
    assert_eq!(
        corners_of(&engine, ep(u2, 2).into()),
        vec![vec![[70.0, -50.0]]]
    );
    assert!(orthogonal(&engine), "{:?}", paths(&engine));
    assert_eq!(paths(&engine), expected, "the preview is the result");
    assert_eq!(engine.project().nets, nets);
    engine.undo()?;
    assert_eq!(engine.project().wires, wires);
    Ok(())
}

/// A free end stays where it is when the part on the wire's other end
/// moves.
#[test]
fn a_free_end_does_not_move() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, occurrence) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    run(
        &mut engine,
        C::DrawWire {
            sheet: None,
            from: ep(u1, 2).into(),
            corners: vec![crate::model::Corner { at: [20.0, 0.0] }],
            to: WireTarget::Free(
                crate::model::FreeEnd::new([20.0, 10.0]).ok_or(Error::Invalid("free".into()))?,
            ),
        },
    )?;
    run(
        &mut engine,
        C::MoveUnit {
            occurrence,
            position: [0.0, -5.0],
        },
    )?;
    let [(_, path)] = &paths(&engine)[..] else {
        return Err(Error::Invalid("one wire".into()));
    };
    assert_eq!(path, &vec![[5.0, -5.0], [20.0, -5.0], [20.0, 10.0]]);
    Ok(())
}
