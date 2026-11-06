//! Connection by drawn position: a pin, junction, label or sheet pin whose
//! connection point lies on a wire's drawn path joins that wire, at its end
//! or anywhere along a segment (a T, which gets a junction dot). Wires that
//! merely cross join nothing until a junction point is put on the crossing.
//! The engine normalises every edit, so nets, ERC, the netlist and every
//! view read one stored topology. `schematic.connect` routes a new wire
//! around every connection point it does not end on, so drawing a wire
//! never joins anything by accident.
use super::support::*;
use crate::model::LabelKind;
use crate::schematic::{self, SchematicCommand};
use crate::{Engine, Error, Id, Operation, Outcome, Point, Result, WireEnd};

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

/// The founder's 1 × 5 header: pins 1–5 straight down at 2.54 mm pitch, so
/// the pins are off a millimetre grid and their positions not exact in f64.
fn header(engine: &mut Engine) -> Result<Fixture> {
    let pitch = 2.54;
    component(
        engine,
        &Spec {
            name: "Header 1x5",
            terminals: (1..=5)
                .map(|i| (i, ["1", "2", "3", "4", "5"][i as usize - 1], passive()))
                .collect(),
            units: vec![(
                1,
                "A",
                (1..=5)
                    .map(|i| (i, [5.0, -pitch * f64::from(i - 1)]))
                    .collect(),
            )],
            package: vec![(1, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5")],
            mapping: (1..=5).map(|i| (i, i)).collect(),
            physical: false,
        },
    )
}

fn dots(engine: &Engine) -> Vec<Point> {
    schematic::junction_dots(engine.project())
        .into_iter()
        .map(|(_, p)| p)
        .collect()
}

fn near(a: Point, b: Point) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6
}

fn q(p: Point) -> [i64; 2] {
    [(p[0] * 1e4).round() as i64, (p[1] * 1e4).round() as i64]
}

/// Every drawn segment of every wire, as unordered pairs of points with
/// zero-length pieces dropped and pieces of one straight run merged,
/// sorted: the picture the wires make.
fn drawn(engine: &Engine) -> Vec<[[i64; 2]; 2]> {
    let project = engine.project();
    // (vertical?, the fixed coordinate) → spans along the line.
    let mut lines: std::collections::BTreeMap<(bool, i64), Vec<[i64; 2]>> = Default::default();
    for path in project
        .wires
        .iter()
        .filter_map(|w| schematic::wire_path(project, w))
    {
        for leg in path.windows(2) {
            let (p, s) = (q(leg[0]), q(leg[1]));
            if p == s {
                continue;
            }
            assert!(p[0] == s[0] || p[1] == s[1], "a slanted segment");
            let vertical = p[0] == s[0];
            let (fixed, along) = if vertical {
                (p[0], [p[1].min(s[1]), p[1].max(s[1])])
            } else {
                (p[1], [p[0].min(s[0]), p[0].max(s[0])])
            };
            lines.entry((vertical, fixed)).or_default().push(along);
        }
    }
    let mut out = Vec::new();
    for ((vertical, fixed), mut spans) in lines {
        spans.sort_unstable();
        let mut merged: Vec<[i64; 2]> = Vec::new();
        for s in spans {
            match merged.last_mut() {
                Some(last) if s[0] <= last[1] => last[1] = last[1].max(s[1]),
                _ => merged.push(s),
            }
        }
        for [lo, hi] in merged {
            out.push(if vertical {
                [[fixed, lo], [fixed, hi]]
            } else {
                [[lo, fixed], [hi, fixed]]
            });
        }
    }
    out.sort_unstable();
    out
}

fn junction_place(engine: &mut Engine, position: Point) -> Result<Id> {
    match apply(
        engine,
        Operation::Schematic(SchematicCommand::PlaceJunction {
            sheet: None,
            position,
        }),
    )? {
        Outcome::JunctionPlaced { junction } => Ok(junction),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

#[test]
fn the_founders_header_joins_all_five_pins_a_wire_runs_along() -> Result<()> {
    let mut engine = Engine::default();
    let fixture = header(&mut engine)?;
    let point = component(&mut engine, &probe("Test point", passive()))?;
    // A wire straight down x = 5 between two test points…
    let (top, _) = place(&mut engine, &point, "TP1", 1, [5.0, 5.0])?;
    let (bottom, _) = place(&mut engine, &point, "TP2", 1, [5.0, -15.0])?;
    connect(&mut engine, ep(top, 1), ep(bottom, 1))?;
    assert_eq!(engine.project().wires.len(), 1);
    // …and the header placed so the ends of its pins 1–5 lie on it.
    let (j1, _) = place(&mut engine, &fixture, "J1", 1, [0.0, 0.0])?;
    let net = net_of(&engine, ep(top, 1))?;
    for pin in 1..=5 {
        assert_eq!(net_of(&engine, ep(j1, pin))?, net, "J1.{pin}");
        assert!(crate::nets::accounted_for(engine.project(), ep(j1, pin)));
    }
    assert_eq!(net_of(&engine, ep(bottom, 1))?, net);
    let project = engine.project();
    assert_eq!(project.nets.len(), 1);
    // The wire is split at each pin, and the drawing is unchanged.
    assert_eq!(project.wires.len(), 6);
    assert_eq!(drawn(&engine), vec![[q([5.0, -15.0]), q([5.0, 5.0])]]);
    // A dot on each T: the pin and the wire on either side.
    let dots = dots(&engine);
    assert_eq!(dots.len(), 5, "{dots:?}");
    for pin in 1..=5 {
        let at = schematic::pin_position(engine.project(), ep(j1, pin))
            .ok_or(Error::NotFound("pin".into()))?;
        assert!(dots.iter().any(|d| near(*d, at)), "J1.{pin}");
    }
    Ok(())
}

#[test]
fn a_document_with_a_wire_over_pins_connects_them_when_normalised() -> Result<()> {
    // The founder's picture: one wire from J1.5 to J1.1 drawn along the
    // ends of pins 2–4, as a document from before this rule holds it.
    let mut engine = Engine::default();
    let fixture = header(&mut engine)?;
    let (j1, _) = place(&mut engine, &fixture, "J1", 1, [0.0, 0.0])?;
    let mut project = engine.project().clone();
    let id = project.allocate()?;
    project.wires.push(crate::model::Wire {
        id,
        sheet: Default::default(),
        from: ep(j1, 5).into(),
        to: ep(j1, 1).into(),
        corners: Vec::new(),
    });
    schematic::tidy(&mut project)?;
    crate::nets::reconcile(&mut project)?;
    let net = project.net_of(ep(j1, 1)).map(|n| n.id);
    for pin in 2..=5 {
        assert_eq!(project.net_of(ep(j1, pin)).map(|n| n.id), net, "J1.{pin}");
    }
    assert_eq!(project.wires.len(), 4);
    assert_eq!(schematic::junction_dots(&project).len(), 3);
    Ok(())
}

#[test]
fn a_pin_moved_onto_a_wires_middle_joins_it_with_a_dot() -> Result<()> {
    let mut engine = Engine::default();
    let resistor = two_pin(&mut engine)?;
    let point = component(&mut engine, &probe("Test point", passive()))?;
    let (u1, _) = place(&mut engine, &resistor, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &resistor, "U2", 1, [40.0, 0.0])?;
    connect(&mut engine, ep(u1, 2), ep(u2, 1))?;
    let (tp, occurrence) = place(&mut engine, &point, "TP1", 1, [20.0, 10.0])?;
    assert_ne!(net_of(&engine, ep(tp, 1))?, net_of(&engine, ep(u1, 2))?);
    let before = drawn(&engine);
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::MoveUnit {
            occurrence,
            position: [20.0, 0.0],
        }),
    )?;
    assert_eq!(net_of(&engine, ep(tp, 1))?, net_of(&engine, ep(u1, 2))?);
    assert_eq!(net_of(&engine, ep(tp, 1))?, net_of(&engine, ep(u2, 1))?);
    assert_eq!(engine.project().wires_at(ep(tp, 1)).count(), 2);
    assert_eq!(dots(&engine), vec![[20.0, 0.0]]);
    assert_eq!(drawn(&engine), before);
    // One undo step takes the join back with the move.
    engine.undo()?;
    assert_ne!(net_of(&engine, ep(tp, 1))?, net_of(&engine, ep(u1, 2))?);
    Ok(())
}

#[test]
fn a_new_wire_takes_the_other_l_or_a_dogleg_around_pins_it_does_not_end_on() -> Result<()> {
    let mut engine = Engine::default();
    let resistor = two_pin(&mut engine)?;
    let point = component(&mut engine, &probe("Test point", passive()))?;
    let (u1, _) = place(&mut engine, &resistor, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &resistor, "U2", 1, [40.0, 20.0])?;
    // TP1 sits on the corner of the horizontal-first L from U1.2 (5, 0)
    // to U2.1 (35, 20): the wire takes the vertical-first L instead. Its
    // bend is a corner of the one wire, where it used to be a junction
    // between two straight wires.
    let (tp1, _) = place(&mut engine, &point, "TP1", 1, [35.0, 0.0])?;
    connect(&mut engine, ep(u1, 2), ep(u2, 1))?;
    assert_eq!(net_of(&engine, ep(u1, 2))?, net_of(&engine, ep(u2, 1))?);
    assert_ne!(net_of(&engine, ep(tp1, 1))?, net_of(&engine, ep(u1, 2))?);
    let project = engine.project();
    assert!(project.junctions.is_empty());
    assert_eq!(project.wires.len(), 1);
    assert_eq!(
        project.wires[0].corners,
        vec![crate::model::Corner { at: [5.0, 20.0] }]
    );
    assert!(dots(&engine).is_empty(), "a bend has no dot");
    // With both L corners taken, a three-legged dogleg goes around.
    engine.undo()?;
    let (tp2, _) = place(&mut engine, &point, "TP2", 1, [5.0, 20.0])?;
    connect(&mut engine, ep(u1, 2), ep(u2, 1))?;
    assert_eq!(net_of(&engine, ep(u1, 2))?, net_of(&engine, ep(u2, 1))?);
    for tp in [tp1, tp2] {
        assert_ne!(net_of(&engine, ep(tp, 1))?, net_of(&engine, ep(u1, 2))?);
    }
    // One wire with two corners (it used to be three wires and two
    // junctions).
    let project = engine.project();
    assert!(project.junctions.is_empty());
    assert_eq!(project.wires.len(), 1);
    assert_eq!(project.wires[0].corners.len(), 2);
    let path =
        schematic::wire_path(project, &project.wires[0]).ok_or(Error::NotFound("path".into()))?;
    for leg in path.windows(2) {
        let (a, b) = (leg[0], leg[1]);
        assert!(
            a[0] == b[0] || a[1] == b[1],
            "every leg is straight: {a:?} → {b:?}"
        );
    }
    // Disconnecting the wire takes the whole path with its corners.
    let leg = project.wires[0].id;
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Disconnect { wire: leg }),
    )?;
    assert!(engine.project().wires.is_empty());
    assert!(engine.project().junctions.is_empty());
    Ok(())
}

#[test]
fn a_wire_with_no_clean_path_is_refused_naming_the_point_in_the_way() -> Result<()> {
    let mut engine = Engine::default();
    let resistor = two_pin(&mut engine)?;
    let point = component(&mut engine, &probe("Test point", passive()))?;
    let (u1, _) = place(&mut engine, &resistor, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &resistor, "U2", 1, [40.0, 0.0])?;
    // A fence of test points across x = 20 on every candidate track.
    for k in -40..=40 {
        place(
            &mut engine,
            &point,
            &format!("TP{}", k + 41),
            1,
            [20.0, 1.25 * f64::from(k)],
        )?;
    }
    let kept = Snapshot::of(&engine);
    let result = connect(&mut engine, ep(u1, 2), ep(u2, 1));
    let Err(Error::Conflict(message)) = result else {
        return Err(Error::Invalid(format!(
            "expected a refusal, got {result:?}"
        )));
    };
    assert!(message.contains("U1 pin B"), "{message}");
    assert!(message.contains("TP41 pin P"), "{message}");
    kept.assert_kept(&engine, "refused connect");
    Ok(())
}

#[test]
fn a_wire_ending_on_a_label_that_lies_on_another_wire_joins_with_a_junction_dot() -> Result<()> {
    let mut engine = Engine::default();
    let resistor = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &resistor, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &resistor, "U2", 1, [40.0, 0.0])?;
    let (u3, _) = place(&mut engine, &resistor, "U3", 1, [25.0, 20.0])?;
    connect(&mut engine, ep(u1, 2), ep(u2, 1))?;
    // A local label dropped on the wire's middle, without attaching it.
    let label = match apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "MID".into(),
            kind: LabelKind::Local,
            position: [20.0, 0.0],
            rotation: Default::default(),
            attach: None,
        }),
    )? {
        Outcome::LabelPlaced { label } => label,
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    // A wire from U3.1 (20, 20) ending on the label lands on the wire.
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Connect {
            from: ep(u3, 1).into(),
            to: WireEnd::label(label).into(),
        }),
    )?;
    let net = net_of(&engine, ep(u1, 2))?;
    assert_eq!(net_of(&engine, ep(u2, 1))?, net);
    assert_eq!(net_of(&engine, ep(u3, 1))?, net);
    assert_eq!(engine.project().wires_at(WireEnd::label(label)).count(), 3);
    assert_eq!(dots(&engine), vec![[20.0, 0.0]]);
    Ok(())
}

#[test]
fn crossing_wires_join_only_through_a_junction_point_on_the_crossing() -> Result<()> {
    let mut engine = Engine::default();
    let resistor = two_pin(&mut engine)?;
    let ids: Vec<Id> = [
        ("U1", [0.0, 0.0]),
        ("U2", [40.0, 0.0]),
        ("U3", [25.0, 20.0]),
        ("U4", [25.0, -20.0]),
    ]
    .into_iter()
    .map(|(name, at)| place(&mut engine, &resistor, name, 1, at).map(|p| p.0))
    .collect::<Result<_>>()?;
    connect(&mut engine, ep(ids[0], 2), ep(ids[1], 1))?;
    // U3.1 (20, 20) → U4.1 (20, -20) crosses the first wire at (20, 0).
    connect(&mut engine, ep(ids[2], 1), ep(ids[3], 1))?;
    let apart = |engine: &Engine| -> Result<bool> {
        Ok(net_of(engine, ep(ids[2], 1))? != net_of(engine, ep(ids[0], 2))?)
    };
    assert!(apart(&engine)?);
    assert_eq!(engine.project().wires.len(), 2);
    assert!(engine.project().junctions.is_empty());
    assert!(dots(&engine).is_empty());
    let before = drawn(&engine);
    // A junction point on the crossing joins all four ends, with a dot.
    let junction = junction_place(&mut engine, [20.0, 0.0])?;
    assert!(!apart(&engine)?);
    assert_eq!(
        engine
            .project()
            .wires_at(WireEnd::junction(junction))
            .count(),
        4
    );
    assert_eq!(dots(&engine), vec![[20.0, 0.0]]);
    assert_eq!(drawn(&engine), before);
    // Removing it restores the plain crossing.
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::RemoveJunction { junction }),
    )?;
    assert!(apart(&engine)?);
    assert_eq!(engine.project().wires.len(), 2);
    assert!(dots(&engine).is_empty());
    assert_eq!(drawn(&engine), before);
    Ok(())
}

#[test]
fn a_junction_point_shows_on_a_lone_wire_and_is_refused_off_wires() -> Result<()> {
    let mut engine = Engine::default();
    let resistor = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &resistor, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &resistor, "U2", 1, [40.0, 20.0])?;
    connect(&mut engine, ep(u1, 2), ep(u2, 1))?;
    // On the straight first leg: a visible point, nothing else changes.
    let junction = junction_place(&mut engine, [20.0, 0.0])?;
    assert_eq!(dots(&engine), vec![[20.0, 0.0]]);
    assert_eq!(engine.project().nets.len(), 3);
    let kept = Snapshot::of(&engine);
    for (at, code) in [
        ([20.0, 5.0], "invalid_input"),
        ([20.0, 0.0], "conflict"),
        ([5.0, 0.0], "conflict"),
    ] {
        let result = junction_place(&mut engine, at);
        assert_eq!(result.err().map(|e| e.code()), Some(code), "{at:?}");
        kept.assert_kept(&engine, "refused junction");
    }
    // A point on the L's corner splits the wire there. Removing it used
    // to be refused, because a bend needed a junction; now the corner
    // simply stays a corner of the one wire.
    let corner = junction_place(&mut engine, [35.0, 0.0])?;
    assert_eq!(dots(&engine), vec![[20.0, 0.0], [35.0, 0.0]]);
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::RemoveJunction { junction: corner }),
    )?;
    assert_eq!(dots(&engine), vec![[20.0, 0.0]]);
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::RemoveJunction { junction }),
    )?;
    assert!(dots(&engine).is_empty());
    let project = engine.project();
    assert!(project.junctions.is_empty());
    assert_eq!(project.wires.len(), 1);
    assert_eq!(
        project.wires[0].corners,
        vec![crate::model::Corner { at: [35.0, 0.0] }]
    );
    Ok(())
}

#[test]
fn a_pin_on_a_wires_end_joins_the_wire() -> Result<()> {
    let mut engine = Engine::default();
    let resistor = two_pin(&mut engine)?;
    let point = component(&mut engine, &probe("Test point", passive()))?;
    let (u1, _) = place(&mut engine, &resistor, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &resistor, "U2", 1, [40.0, 0.0])?;
    connect(&mut engine, ep(u1, 2), ep(u2, 1))?;
    // TP1's pin lands exactly on U2.1, where the wire ends.
    let (tp, _) = place(&mut engine, &point, "TP1", 1, [35.0, 0.0])?;
    assert_eq!(net_of(&engine, ep(tp, 1))?, net_of(&engine, ep(u1, 2))?);
    assert!(crate::nets::accounted_for(engine.project(), ep(tp, 1)));
    Ok(())
}

#[test]
fn the_showcase_keeps_its_netlist_and_no_wire_runs_over_a_pin() -> Result<()> {
    let engine = Engine::showcase()?;
    let project = engine.project();
    assert_eq!(project.nets.len(), 24);
    for (name, terminals) in crate::library::samples::SHOWCASE_NETS {
        let mut want: Vec<crate::Endpoint> = terminals
            .iter()
            .map(|(d, t)| crate::layout::showcase::endpoint(project, d, *t))
            .collect::<Result<_>>()?;
        want.sort();
        let net = project
            .nets
            .iter()
            .find(|n| n.name == *name)
            .ok_or_else(|| Error::NotFound((*name).into()))?;
        assert_eq!(net.terminals, want, "{name}");
    }
    // Normalising changes nothing: no wire runs over a point it does not
    // end on.
    let mut again = project.clone();
    schematic::tidy(&mut again)?;
    assert_eq!(again.wires, project.wires);
    Ok(())
}

/// The end of the label's drawing farthest from its connection point.
fn far_end_of(engine: &Engine, label: Id) -> Result<Point> {
    let project = engine.project();
    let at = project
        .label(label)
        .ok_or(Error::Invalid("no label".into()))?;
    let anchor = schematic::labels::anchor(project, at);
    let drawings = schematic::scene::sheet_labels(project, at.sheet);
    let (_, drawing) = drawings
        .iter()
        .find(|(id, _)| *id == label)
        .ok_or(Error::Invalid("no drawing".into()))?;
    drawing
        .strokes
        .iter()
        .filter_map(|s| match s.shape {
            crate::geometry::Shape::Line { a, b } => Some([a, b]),
            _ => None,
        })
        .flatten()
        .max_by(|p, q| {
            let d = |r: &Point| (r[0] - anchor[0]).hypot(r[1] - anchor[1]);
            d(p).total_cmp(&d(q))
        })
        .ok_or(Error::Invalid("no line".into()))
}

/// e2e batch F2: a free end landing on a port's drawing is on that port: it
/// attaches, though the port's connection point is elsewhere. A port on a
/// pin stays there: the wire takes the pin. One standing alone takes the
/// wire itself. The wire keeps the point it reached.
#[test]
fn a_free_end_on_a_ports_drawing_attaches_to_it() -> Result<()> {
    let mut engine = Engine::default();
    let resistor = two_pin(&mut engine)?;
    let (r1, _) = place(&mut engine, &resistor, "R1", 1, [0.0, 0.0])?;
    let port = |name: &str, position, attach| {
        Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: name.into(),
            kind: LabelKind::Power {
                style: crate::model::PowerStyle::Ground,
            },
            position,
            rotation: Default::default(),
            attach,
        })
    };
    apply(
        &mut engine,
        port(
            "GND",
            [0.0, 0.0],
            Some(schematic::WireTarget::Terminal(ep(r1, 2))),
        ),
    )?;
    let on_pin = engine.project().labels[0].id;
    apply(&mut engine, port("AGND", [40.0, 0.0], None))?;
    let alone = engine.project().labels[1].id;
    let draw = |engine: &mut Engine, end: Point| {
        let free = |p: Point| {
            crate::model::FreeEnd::new(p)
                .map(schematic::WireTarget::Free)
                .ok_or(Error::Invalid("free".into()))
        };
        apply(
            engine,
            Operation::Schematic(SchematicCommand::DrawWire {
                sheet: None,
                from: free([end[0], end[1] - 30.0])?,
                corners: vec![],
                to: free(end)?,
            }),
        )
    };
    let on_ink = far_end_of(&engine, on_pin)?;
    assert!(!near(
        on_ink,
        schematic::pin_position(engine.project(), ep(r1, 2)).ok_or(Error::Invalid("pin".into()))?
    ));
    draw(&mut engine, on_ink)?;
    let project = engine.project();
    let wire = project
        .wires_at(WireEnd::Terminal(ep(r1, 2)))
        .find(|w| w.corners.iter().any(|c| near(c.at, on_ink)))
        .ok_or(Error::Invalid("the wire did not attach to the pin".into()))?;
    assert!(matches!(wire.from, WireEnd::Free(_)));
    let label = project
        .label(on_pin)
        .ok_or(Error::Invalid("port gone".into()))?;
    assert_eq!(
        schematic::labels::attached_pin(project, label),
        Some(ep(r1, 2)),
        "the port stays on its pin"
    );
    let on_ink = far_end_of(&engine, alone)?;
    draw(&mut engine, on_ink)?;
    assert_eq!(
        engine.project().wires_at(WireEnd::label(alone)).count(),
        1,
        "the wire did not attach to the port standing alone"
    );
    // Nearby but off the drawing stays free.
    draw(&mut engine, [70.0, 5.0])?;
    assert!(
        engine
            .project()
            .wires
            .iter()
            .any(|w| matches!(w.to, WireEnd::Free(_))),
    );
    Ok(())
}
