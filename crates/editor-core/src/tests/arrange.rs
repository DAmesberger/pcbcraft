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
        arrange_op(instances(&[u1, u2]), Arrangement::Move { by: pt(by) }),
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
        Arrangement::Rotate {
            about: pt([15.0, 5.0]),
        },
        Arrangement::Mirror {
            about: pt([15.0, 5.0]),
        },
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

/// Whether `path` runs back on itself: a segment straight back along the
/// one before, or opposite the one two before (a U-turn).
fn doubles_back(path: &[Point]) -> bool {
    let d: Vec<Point> = path
        .windows(2)
        .map(|s| [s[1][0] - s[0][0], s[1][1] - s[0][1]])
        .collect();
    let opposite = |a: Point, b: Point| {
        (a[0] * b[1] - a[1] * b[0]).abs() < 1e-9 && a[0] * b[0] + a[1] * b[1] < 0.0
    };
    d.windows(2).any(|w| opposite(w[0], w[1])) || d.windows(3).any(|w| opposite(w[0], w[2]))
}

/// VM check #6: a group turned so a wire's moved end passes the corner it
/// would slide leaves no U-turn: the wire leaving the group is drawn again
/// as one clean step between the new pin and the end that stayed.
#[test]
fn a_turned_group_leaves_no_u_turn() -> Result<()> {
    let Sheet {
        mut engine,
        u1,
        u2,
        u3,
        outer,
        ..
    } = sheet()?;
    // U2.B (35, 15) right to x = 45, down, right to U3.A (55, 0).
    apply(
        &mut engine,
        Operation::Schematic(C::SetWirePath {
            wire: outer,
            corners: vec![
                Corner::sharp(pt([45.0, 15.0])),
                Corner::sharp(pt([45.0, 0.0])),
            ],
            from: None,
            to: None,
        }),
    )?;
    let drawn = path(&engine, outer)?;
    assert!(!doubles_back(&drawn), "{drawn:?}");
    // About (40, 30) U2.B lands at (55, 25), past the corner's x = 45.
    let how = Arrangement::Rotate {
        about: pt([40.0, 30.0]),
    };
    apply(&mut engine, arrange_op(instances(&[u1, u2]), how))?;
    let band = path(&engine, outer)?;
    assert!(orthogonal(&band), "{band:?}");
    assert!(!doubles_back(&band), "{band:?}");
    assert_eq!(band.first(), pins(&engine, u2).get(1), "on the turned pin");
    assert_eq!(
        band.last(),
        pins(&engine, u3).first(),
        "the end outside stays"
    );
    Ok(())
}

/// VM check #6: a wide group near the top of the sheet, turned, lands
/// inside the frame: the turn point moves it down by whole grid steps. A
/// group with room turns about its middle.
#[test]
fn a_turned_group_stays_inside_the_sheet_frame() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [50.0, -20.0])?;
    let (u2, _) = place(&mut engine, &f, "U2", 1, [110.0, -20.0])?;
    let sheet = crate::SheetId::FIRST;
    let objects = instances(&[u1, u2]);
    let grid = 2.5;
    let middle = arrange::centre(engine.project(), sheet, &objects)
        .ok_or_else(|| Error::Invalid("no centre".into()))?
        .map(|v| (v / grid).round() * grid);
    let about = arrange::turn_about(engine.project(), sheet, &objects, middle, grid);
    assert_ne!(about, middle, "about its middle it would leave the frame");
    for v in about {
        assert!(
            (v / grid - (v / grid).round()).abs() < 1e-9,
            "on the grid: {about:?}"
        );
    }
    apply(
        &mut engine,
        arrange_op(objects.clone(), Arrangement::Rotate { about: pt(about) }),
    )?;
    let project = engine.project();
    let [w, h] = project
        .sheet(sheet)
        .ok_or_else(|| Error::Invalid("sheet".into()))?
        .size_mm();
    let inner = schematic::scene::FRAME_INNER_MM;
    let m = crate::library::TextMetric::default();
    for occurrence in project.occurrences.iter() {
        let b = schematic::placed_unit(project, occurrence.id)?
            .bounds(m)
            .ok_or_else(|| Error::Invalid("drawn".into()))?;
        assert!(
            b.min[0] >= inner
                && b.max[0] <= w - inner
                && b.min[1] >= -(h - inner)
                && b.max[1] <= -inner,
            "{b:?} inside the frame"
        );
    }
    // Moved down to the middle of the sheet, the group turns where it is.
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [100.0, -100.0])?;
    let (u2, _) = place(&mut engine, &f, "U2", 1, [160.0, -100.0])?;
    let objects = instances(&[u1, u2]);
    let middle = [130.0, -97.5];
    assert_eq!(
        arrange::turn_about(engine.project(), sheet, &objects, middle, grid),
        middle
    );
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
            corners: vec![Corner::sharp(pt([start[0], -20.0]))],
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
            Arrangement::Move { by: pt(by) },
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
            Arrangement::Move { by: pt(by) },
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
                at: pt([at[0], at[1]]),
            }),
        }),
    )?;
    let junction = engine
        .project()
        .junctions
        .first()
        .map(|j| (j.id, j.position))
        .ok_or_else(|| Error::Invalid("no junction".into()))?;
    // Down, so U1's wire does not come back up along U2's onto the T
    // (which would move the T to where they part).
    let by = [0.0, -2.5];
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
        arrange_op(instances(&[u1, u2]), Arrangement::Move { by: pt(by) }),
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
            Arrangement::Move { by: pt(by) },
        ),
    )?;
    let [x, y] = junction.1.to_mm_f64();
    assert_eq!(spot(&engine)?, pt([x, y + by[1]]));
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
                position: pt([-20.0, 20.0]),
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
            min: pt([10.0, 10.0]),
            max: pt([20.0, 15.0]),
        },
    )?;
    let note = drawing(
        &mut engine,
        SheetItem::Text {
            at: pt([10.0, 20.0]),
            text: "Note".into(),
            height_nm: nm(2.5),
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
        arrange_op(
            objects.clone(),
            Arrangement::Mirror {
                about: pt([0.0, 0.0]),
            },
        ),
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
    assert_eq!(label(flag)?.position, pt([20.0, 20.0]));
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
            min: pt([-20.0, 10.0]),
            max: pt([-10.0, 15.0]),
        }
    );
    assert!(matches!(
        item(note)?,
        SheetItem::Text {
            at,
            rotation: Rotation::R0,
            ..
        } if at == pt([-10.0, 20.0])
    ));
    apply(
        &mut engine,
        arrange_op(
            objects,
            Arrangement::Rotate {
                about: pt([0.0, 0.0]),
            },
        ),
    )?;
    let project = engine.project();
    let note_rotation = project.drawing(note).map(|d| d.item.clone());
    assert!(matches!(
        note_rotation,
        Some(SheetItem::Text {
            at,
            rotation: Rotation::R90,
            ..
        }) if at == pt([-20.0, -10.0])
    ));
    assert_eq!(
        project.label(flag).map(|l| (l.position, l.rotation)),
        Some((pt([-20.0, 20.0]), Rotation::R270)),
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
        (instances(&[u1, u2]), [1_999.0, 0.0]),
    ] {
        assert!(
            engine
                .apply(
                    revision,
                    arrange_op(objects, Arrangement::Move { by: pt(by) })
                )
                .is_err()
        );
        assert_eq!(engine.revision(), revision);
    }
    // A shift beyond the range, or not finite, is refused where the command
    // is read.
    for by in [
        serde_json::json!([20_000.0, 0.0]),
        serde_json::json!(["NaN", 0.0]),
    ] {
        let command = serde_json::json!({"command": "schematic.arrange", "params": {
            "objects": [{"kind": "instance", "id": u1}],
            "action": {"kind": "move", "by": by}
        }});
        assert!(serde_json::from_value::<C>(command).is_err(), "{by}");
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
            action: Arrangement::Rotate { about },
            ..
        } if about == pt([1.0, 2.0])
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
            position: pt([-20.0, 20.0]),
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
                from: pt([0.0, 30.0]),
                to: pt([10.0, 30.0]),
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
