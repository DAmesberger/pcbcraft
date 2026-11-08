//! `schematic.arrange` and `Change::RemoveObjects` through the public
//! commands: a group moves, turns and mirrors as one undo step, wires with
//! both ends in the group keep their shape, the others rubber-band, and a
//! group's removal is reviewed with everything listed.
use super::support::*;
use crate::ids::TerminalId;
use crate::library::Rotation;
use crate::model::{Corner, LabelKind, PowerStyle, SheetItem};
use crate::schematic::{
    self, Arrangement, ClipObject, SchematicCommand as C, WireTarget, arrange, labels,
};
use crate::updates::Change;
use crate::{Endpoint, Engine, Error, Id, Operation, Outcome, Point, Result, WireEnd};

fn two_pin(engine: &mut Engine) -> Result<Fixture> {
    component(
        engine,
        &Spec {
            name: "Two terminal",
            terminals: vec![(1, "A", passive()), (2, "B", passive())],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 5.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: true,
        },
    )
}

fn pin(instance: Id, terminal: u32) -> Endpoint {
    Endpoint {
        instance,
        terminal: TerminalId(terminal),
    }
}

/// U1 at the origin, U2 at (30, 10) and U3 at (60, 0): U1.B to U2.A inside
/// the group the tests name, U2.B to U3.A leaving it. Each wire has a corner.
struct Sheet {
    engine: Engine,
    u1: Id,
    u2: Id,
    u3: Id,
    inner: Id,
    outer: Id,
}

fn sheet() -> Result<Sheet> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &f, "U2", 1, [30.0, 10.0])?;
    let (u3, _) = place(&mut engine, &f, "U3", 1, [60.0, 0.0])?;
    connect(&mut engine, pin(u1, 2), pin(u2, 1))?;
    connect(&mut engine, pin(u2, 2), pin(u3, 1))?;
    let inner = wire_between(&engine, pin(u1, 2), pin(u2, 1))?;
    let outer = wire_between(&engine, pin(u2, 2), pin(u3, 1))?;
    Ok(Sheet {
        engine,
        u1,
        u2,
        u3,
        inner,
        outer,
    })
}

fn arrange_op(objects: Vec<ClipObject>, action: Arrangement) -> Operation {
    Operation::Schematic(C::Arrange {
        sheet: None,
        objects,
        action,
    })
}

fn path(engine: &Engine, wire: Id) -> Result<Vec<Point>> {
    let project = engine.project();
    project
        .wire(wire)
        .and_then(|w| schematic::wire_path(project, w))
        .ok_or_else(|| Error::Invalid("no wire".into()))
}

fn orthogonal(path: &[Point]) -> bool {
    path.windows(2)
        .all(|s| (s[0][0] - s[1][0]).abs() < 1e-9 || (s[0][1] - s[1][1]).abs() < 1e-9)
}

fn close(a: &[Point], b: &[Point]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(p, q)| (p[0] - q[0]).abs() < 1e-6 && (p[1] - q[1]).abs() < 1e-6)
}

fn pins(engine: &Engine, instance: Id) -> Vec<Point> {
    [1, 2]
        .into_iter()
        .filter_map(|t| schematic::pin_position(engine.project(), pin(instance, t)))
        .collect()
}

fn instances(ids: &[Id]) -> Vec<ClipObject> {
    ids.iter().map(|&id| ClipObject::Instance { id }).collect()
}

/// Moving U1 and U2 carries the wire between them unchanged and rubber-bands
/// the wire leaving the group, in one undo step, with the nets as they were.
#[test]
fn a_moved_group_carries_inner_wires_and_rubber_bands_the_rest() -> Result<()> {
    let Sheet {
        mut engine,
        u1,
        u2,
        u3,
        inner,
        outer,
    } = sheet()?;
    let before = engine.project().clone();
    let (inner_path, u3_pins) = (path(&engine, inner)?, pins(&engine, u3));
    let by = [10.0, -5.0];
    let revision = engine.revision();
    apply(
        &mut engine,
        arrange_op(instances(&[u1, u2]), Arrangement::Move { by }),
    )?;
    assert_eq!(engine.revision(), revision + 1);
    let shifted: Vec<Point> = inner_path
        .iter()
        .map(|p| [p[0] + by[0], p[1] + by[1]])
        .collect();
    assert!(close(&path(&engine, inner)?, &shifted), "rigid");
    let band = path(&engine, outer)?;
    assert!(orthogonal(&band), "{band:?}");
    assert_eq!(band.last(), u3_pins.first(), "the end outside stays");
    assert_eq!(engine.project().nets, before.nets);
    engine.undo()?;
    assert_eq!(engine.project().occurrences, before.occurrences);
    assert_eq!(engine.project().wires, before.wires, "one undo step");
    Ok(())
}

/// A turned or mirrored group lands exactly where the arrangement takes each
/// pin, the wire between them keeps its shape and a wire leaving it stays
/// orthogonal.
#[test]
fn a_turned_or_mirrored_group_keeps_its_shape_about_the_point() -> Result<()> {
    for how in [
        Arrangement::Rotate { about: [15.0, 5.0] },
        Arrangement::Mirror { about: [15.0, 5.0] },
    ] {
        let Sheet {
            mut engine,
            u1,
            u2,
            inner,
            outer,
            ..
        } = sheet()?;
        let before = engine.project().clone();
        let expected: Vec<Vec<Point>> = [u1, u2]
            .iter()
            .map(|&id| {
                pins(&engine, id)
                    .into_iter()
                    .map(|p| how.point(p))
                    .collect()
            })
            .collect();
        let inner_path: Vec<Point> = path(&engine, inner)?
            .into_iter()
            .map(|p| how.point(p))
            .collect();
        apply(&mut engine, arrange_op(instances(&[u1, u2]), how))?;
        assert!(close(&pins(&engine, u1), &expected[0]), "{how:?} U1");
        assert!(close(&pins(&engine, u2), &expected[1]), "{how:?} U2");
        assert!(close(&path(&engine, inner)?, &inner_path), "{how:?} wire");
        assert!(orthogonal(&path(&engine, outer)?), "{how:?} outer");
        engine.undo()?;
        assert_eq!(engine.project().occurrences, before.occurrences);
        assert_eq!(engine.project().wires, before.wires);
    }
    Ok(())
}

/// A named wire brings its free end: rigid with its symbol, and with the end
/// on an unnamed symbol held, only the free end goes and the path follows.
#[test]
fn a_named_wire_brings_its_free_end() -> Result<()> {
    let Sheet { mut engine, u1, .. } = sheet()?;
    let start = schematic::pin_position(engine.project(), pin(u1, 1))
        .ok_or_else(|| Error::Invalid("pin".into()))?;
    apply(
        &mut engine,
        Operation::Schematic(C::DrawWire {
            sheet: None,
            from: WireTarget::Terminal(pin(u1, 1)),
            corners: vec![Corner::sharp([start[0], -20.0])],
            to: WireEnd::free([-40.0, -20.0])
                .ok_or_else(|| Error::Invalid("free".into()))?
                .into(),
        }),
    )?;
    let free = wire_between_free(&engine)?;
    let drawn = path(&engine, free)?;
    let by = [0.0, -10.0];
    apply(
        &mut engine,
        arrange_op(
            vec![
                ClipObject::Instance { id: u1 },
                ClipObject::Wire { id: free },
            ],
            Arrangement::Move { by },
        ),
    )?;
    let shifted: Vec<Point> = drawn.iter().map(|p| [p[0] + by[0], p[1] + by[1]]).collect();
    assert!(
        close(&path(&engine, free)?, &shifted),
        "rigid with its symbol"
    );
    engine.undo()?;
    apply(
        &mut engine,
        arrange_op(
            vec![ClipObject::Wire { id: free }],
            Arrangement::Move { by },
        ),
    )?;
    let held = path(&engine, free)?;
    assert_eq!(held.first(), Some(&start), "the pin end stays");
    assert_eq!(held.last(), Some(&[-40.0, -30.0]), "the free end goes");
    assert!(orthogonal(&held), "{held:?}");
    Ok(())
}

fn wire_between_free(engine: &Engine) -> Result<Id> {
    engine
        .project()
        .wires
        .iter()
        .find(|w| w.to.free_point().is_some() || w.from.free_point().is_some())
        .map(|w| w.id)
        .ok_or_else(|| Error::Invalid("no free wire".into()))
}

/// A junction comes along with a named wire that ends on it, and only then;
/// the wires on it that are not named rubber-band.
#[test]
fn a_junction_comes_along_with_a_named_wire_only() -> Result<()> {
    let Sheet {
        mut engine,
        u1,
        u2,
        u3,
        inner,
        ..
    } = sheet()?;
    let on = path(&engine, inner)?;
    let at = on[1];
    apply(
        &mut engine,
        Operation::Schematic(C::Connect {
            from: WireTarget::Terminal(pin(u3, 2)),
            to: WireTarget::OnWire(schematic::WirePoint {
                wire: inner,
                at: [at[0], at[1]],
            }),
        }),
    )?;
    let junction = engine
        .project()
        .junctions
        .first()
        .map(|j| (j.id, j.position))
        .ok_or_else(|| Error::Invalid("no junction".into()))?;
    let by = [0.0, 5.0];
    let spot = |engine: &Engine| {
        engine
            .project()
            .junction(junction.0)
            .map(|j| j.position)
            .ok_or_else(|| Error::Invalid("gone".into()))
    };
    // The symbols alone leave it where it is.
    apply(
        &mut engine,
        arrange_op(instances(&[u1, u2]), Arrangement::Move { by }),
    )?;
    assert_eq!(spot(&engine)?, junction.1);
    engine.undo()?;
    // A wire on it brings it along.
    let wire = engine
        .project()
        .wires_at(WireEnd::junction(junction.0))
        .next()
        .map(|w| w.id)
        .ok_or_else(|| Error::Invalid("no wire".into()))?;
    apply(
        &mut engine,
        arrange_op(
            vec![ClipObject::Wire { id: wire }],
            Arrangement::Move { by },
        ),
    )?;
    assert_eq!(spot(&engine)?, [junction.1[0], junction.1[1] + by[1]]);
    Ok(())
}

/// A label on a named symbol's pin stays on that pin, placing itself; a free
/// label moves and turns with the group; a mirrored flag turns round while a
/// ground port keeps standing; a rectangle stays a rectangle and a note stays
/// readable.
#[test]
fn labels_and_drawings_go_with_the_group() -> Result<()> {
    let Sheet { mut engine, u1, .. } = sheet()?;
    let place_label = |engine: &mut Engine, name: &str, kind, attach| -> Result<Id> {
        match apply(
            engine,
            Operation::Schematic(C::PlaceLabel {
                sheet: None,
                name: name.into(),
                kind,
                position: [-20.0, 20.0],
                rotation: Rotation::R0,
                attach,
            }),
        )? {
            Outcome::LabelPlaced { label } => Ok(label),
            other => Err(Error::Invalid(format!("unexpected {other:?}"))),
        }
    };
    let on_pin = place_label(
        &mut engine,
        "SIG",
        LabelKind::Local,
        Some(WireTarget::Terminal(pin(u1, 1))),
    )?;
    let flag = place_label(&mut engine, "FLAG", LabelKind::Local, None)?;
    let ground = place_label(
        &mut engine,
        "GND",
        LabelKind::Power {
            style: PowerStyle::Ground,
        },
        None,
    )?;
    let drawing = |engine: &mut Engine, item| -> Result<Id> {
        match apply(
            engine,
            Operation::Schematic(C::AddDrawing { sheet: None, item }),
        )? {
            Outcome::DrawingAdded { drawing } => Ok(drawing),
            other => Err(Error::Invalid(format!("unexpected {other:?}"))),
        }
    };
    let rect = drawing(
        &mut engine,
        SheetItem::Rect {
            min: [10.0, 10.0],
            max: [20.0, 15.0],
        },
    )?;
    let note = drawing(
        &mut engine,
        SheetItem::Text {
            at: [10.0, 20.0],
            text: "Note".into(),
            height: 2.5,
            rotation: Rotation::R0,
        },
    )?;
    let objects = vec![
        ClipObject::Instance { id: u1 },
        ClipObject::Label { id: on_pin },
        ClipObject::Label { id: flag },
        ClipObject::Label { id: ground },
        ClipObject::Drawing { id: rect },
        ClipObject::Drawing { id: note },
    ];
    apply(
        &mut engine,
        arrange_op(objects.clone(), Arrangement::Mirror { about: [0.0, 0.0] }),
    )?;
    let project = engine.project();
    let label = |id| {
        project
            .label(id)
            .ok_or_else(|| Error::Invalid("gone".into()))
    };
    assert!(!label(on_pin)?.manual, "still placing itself");
    assert_eq!(
        schematic::pin_position(project, pin(u1, 1)),
        Some(labels::anchor(project, label(on_pin)?)),
    );
    assert_eq!(label(flag)?.position, [20.0, 20.0]);
    assert_eq!(
        label(flag)?.rotation,
        Rotation::R180,
        "the flag turns round"
    );
    assert_eq!(
        label(ground)?.rotation,
        Rotation::R0,
        "ground keeps standing"
    );
    let item = |id| {
        project
            .drawing(id)
            .map(|d| d.item.clone())
            .ok_or_else(|| Error::Invalid("gone".into()))
    };
    assert_eq!(
        item(rect)?,
        SheetItem::Rect {
            min: [-20.0, 10.0],
            max: [-10.0, 15.0],
        }
    );
    assert!(matches!(
        item(note)?,
        SheetItem::Text {
            at: [-10.0, 20.0],
            rotation: Rotation::R0,
            ..
        }
    ));
    apply(
        &mut engine,
        arrange_op(objects, Arrangement::Rotate { about: [0.0, 0.0] }),
    )?;
    let project = engine.project();
    let note_rotation = project.drawing(note).map(|d| d.item.clone());
    assert!(matches!(
        note_rotation,
        Some(SheetItem::Text {
            at: [-20.0, -10.0],
            rotation: Rotation::R90,
            ..
        })
    ));
    assert_eq!(
        project.label(flag).map(|l| (l.position, l.rotation)),
        Some(([-20.0, 20.0], Rotation::R270)),
    );
    Ok(())
}

/// Nothing named, an object not on the sheet or a move off the sheet's range
/// is refused and leaves the document as it was. A wire and a junction are
/// bounded by the same range.
#[test]
fn a_refused_arrangement_leaves_the_document_unchanged() -> Result<()> {
    let Sheet {
        mut engine, u1, u2, ..
    } = sheet()?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let unknown: Id = 9_999;
    for (objects, by) in [
        (Vec::new(), [1.0, 0.0]),
        (instances(&[unknown]), [1.0, 0.0]),
        (instances(&[u1, u2]), [20_000.0, 0.0]),
        (instances(&[u1]), [f64::NAN, 0.0]),
    ] {
        assert!(
            engine
                .apply(revision, arrange_op(objects, Arrangement::Move { by }))
                .is_err()
        );
        assert_eq!(engine.revision(), revision);
    }
    assert_eq!(engine.project().occurrences, before.occurrences);
    Ok(())
}

/// The middle a group turns about ignores text, so it does not walk as the
/// group turns.
#[test]
fn the_centre_is_the_middle_of_pins_and_vertices() -> Result<()> {
    let Sheet { engine, u1, u2, .. } = sheet()?;
    let centre = arrange::centre(
        engine.project(),
        crate::SheetId::FIRST,
        &instances(&[u1, u2]),
    )
    .ok_or_else(|| Error::Invalid("no centre".into()))?;
    assert_eq!(centre, [15.0, 7.5]);
    Ok(())
}

/// The wire format: the command's name and a tagged action.
#[test]
fn the_arrange_command_round_trips() -> Result<()> {
    let value = serde_json::json!({
        "command": "schematic.arrange",
        "params": {
            "objects": [{ "kind": "instance", "id": 7 }],
            "action": { "kind": "rotate", "about": [1.0, 2.0] },
        },
    });
    let command: C =
        serde_json::from_value(value.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
    assert!(matches!(
        command,
        C::Arrange {
            sheet: None,
            action: Arrangement::Rotate { about: [1.0, 2.0] },
            ..
        }
    ));
    assert_eq!(
        serde_json::to_value(&command).map_err(|e| Error::Invalid(e.to_string()))?,
        value
    );
    assert_eq!(command.class(), crate::permission::Class::Edit);
    Ok(())
}

/// The review lists every removed object by name and changes nothing; the
/// apply is one undo step; a wired symbol's wires go with it.
#[test]
fn removing_a_group_is_reviewed_with_everything_listed() -> Result<()> {
    let Sheet {
        mut engine,
        u1,
        u2,
        u3,
        inner,
        outer,
    } = sheet()?;
    let label = match apply(
        &mut engine,
        Operation::Schematic(C::PlaceLabel {
            sheet: None,
            name: "VCC".into(),
            kind: LabelKind::Power {
                style: PowerStyle::Bar,
            },
            position: [-20.0, 20.0],
            rotation: Rotation::R0,
            attach: None,
        }),
    )? {
        Outcome::LabelPlaced { label } => label,
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    let note = match apply(
        &mut engine,
        Operation::Schematic(C::AddDrawing {
            sheet: None,
            item: SheetItem::Line {
                from: [0.0, 30.0],
                to: [10.0, 30.0],
            },
        }),
    )? {
        Outcome::DrawingAdded { drawing } => drawing,
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    let before = engine.project().clone();
    let revision = engine.revision();
    let change = Change::RemoveObjects {
        objects: vec![
            ClipObject::Instance { id: u1 },
            ClipObject::Instance { id: u2 },
            ClipObject::Wire { id: inner },
            ClipObject::Label { id: label },
            ClipObject::Drawing { id: note },
        ],
    };
    let plan = engine.preview_update(change.clone());
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(engine.revision(), revision, "a preview changes nothing");
    let lines: Vec<&str> = plan.items.iter().map(|i| i.line.as_str()).collect();
    for expected in [
        "Removes U1",
        "Removes U2",
        "Removes power port VCC",
        "Removes a line",
    ] {
        assert!(lines.contains(&expected), "{expected} in {lines:?}");
    }
    assert!(plan.removed_wires.contains(&inner));
    assert!(
        plan.removed_wires.contains(&outer),
        "a removed symbol's wire"
    );
    engine.apply_update(revision, change, false)?;
    let project = engine.project();
    assert!(project.instance(u1).is_none() && project.instance(u2).is_none());
    assert!(project.instance(u3).is_some());
    assert!(project.label(label).is_none() && project.drawing(note).is_none());
    assert!(project.wires.is_empty());
    engine.undo()?;
    assert_eq!(
        engine.project().instances,
        before.instances,
        "one undo step"
    );
    assert_eq!(engine.project().wires, before.wires);
    let refused = engine.preview_update(Change::RemoveObjects {
        objects: vec![ClipObject::Wire { id: 9_999 }],
    });
    assert!(!refused.blockers.is_empty(), "an unknown object is refused");
    Ok(())
}
