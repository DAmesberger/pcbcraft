//! `schematic.copy` and `schematic.paste`: what a clip holds, what a paste
//! creates, nets staying separate, one undo step, and refusals.
use super::support::*;
use crate::ids::TerminalId;
use crate::library::Rotation;
use crate::model::{LabelKind, SheetItem};
use crate::schematic::{ClipObject, Pasted, SchematicCommand as C, SheetClip, WireTarget};
use crate::{Endpoint, Engine, Error, Id, Operation, Outcome, Point, Query, QueryResult, Result};

fn two_pin(engine: &mut Engine) -> Result<Fixture> {
    component(
        engine,
        &Spec {
            name: "Two terminal",
            terminals: vec![(1, "A", passive()), (2, "B", passive())],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: true,
        },
    )
}

/// U1 at the origin and U2 at x = 20, with one wire U1.B to U2.A.
struct Sheet {
    engine: Engine,
    u1: Id,
    u2: Id,
    wire: Id,
}

fn pin(instance: Id, terminal: u32) -> Endpoint {
    Endpoint {
        instance,
        terminal: TerminalId(terminal),
    }
}

fn wired() -> Result<Sheet> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &f, "U2", 1, [20.0, 0.0])?;
    connect(&mut engine, pin(u1, 2), pin(u2, 1))?;
    let wire = wire_between(&engine, pin(u1, 2), pin(u2, 1))?;
    Ok(Sheet {
        engine,
        u1,
        u2,
        wire,
    })
}

fn copied(engine: &Engine, objects: Vec<ClipObject>) -> Result<SheetClip> {
    match engine.query(Query::SheetCopy {
        sheet: None,
        objects,
    })? {
        QueryResult::SheetClip(clip) => Ok(*clip),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

fn paste_op(clip: SheetClip, offset: Point) -> Operation {
    Operation::Schematic(C::Paste {
        sheet: None,
        clip: Box::new(clip),
        offset,
    })
}

fn pasted(engine: &mut Engine, clip: SheetClip, offset: Point) -> Result<Pasted> {
    match apply(engine, paste_op(clip, offset))? {
        Outcome::Pasted { report } => Ok(report),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

/// A symbol: a new instance of the same component with the next designator
/// and its orientation, at the offset, unconnected; one undo takes it back.
#[test]
fn a_symbol_pastes_as_a_new_instance_with_a_new_designator() -> Result<()> {
    let Sheet {
        mut engine,
        u1,
        wire,
        ..
    } = wired()?;
    let occurrence = engine.project().occurrences_of(u1).next().map(|o| o.id);
    apply(
        &mut engine,
        Operation::Schematic(C::OrientUnit {
            occurrence: occurrence.ok_or(Error::Invalid("no unit".into()))?,
            rotation: Rotation::R90,
            mirror: true,
        }),
    )?;
    let before = engine.project().clone();
    let clip = copied(&engine, vec![ClipObject::Instance { id: u1 }])?;
    assert!(clip.wires.is_empty(), "the wire to U2 stays behind");
    let report = pasted(&mut engine, clip, [0.0, -30.0])?;
    let [symbol] = report.symbols.as_slice() else {
        return Err(Error::Invalid("one symbol".into()));
    };
    assert_eq!(symbol.designator, "U3");
    let project = engine.project();
    let (old, new) = (project.instance(u1), project.instance(symbol.instance));
    assert_eq!(new.map(|i| i.component), old.map(|i| i.component));
    let placed = project
        .occurrence(symbol.occurrence)
        .ok_or(Error::Invalid("no occurrence".into()))?;
    assert_eq!(placed.position, [0.0, -30.0]);
    assert_eq!((placed.rotation, placed.mirror), (Rotation::R90, true));
    assert_ne!(
        net_of(&engine, pin(symbol.instance, 2))?,
        net_of(&engine, pin(u1, 2))?
    );
    assert!(engine.project().wire(wire).is_some());
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

/// A label keeps its name and so joins that net by name, as any label does.
#[test]
fn a_label_keeps_its_name_and_joins_its_net() -> Result<()> {
    let Sheet {
        mut engine, u1, u2, ..
    } = wired()?;
    let sig = signal_label(&mut engine, u1)?;
    let clip = copied(&engine, vec![ClipObject::Label { id: sig }])?;
    let report = pasted(&mut engine, clip, [0.0, -30.0])?;
    let [new] = report.labels.as_slice() else {
        return Err(Error::Invalid("one label".into()));
    };
    let label = engine
        .project()
        .label(*new)
        .ok_or(Error::Invalid("no label".into()))?;
    assert_eq!(label.name, "SIG");
    assert_eq!(label.position, [-5.0, -30.0], "where the source is drawn");
    assert!(label.manual, "alone it stays where it was pasted");
    apply(
        &mut engine,
        Operation::Schematic(C::Connect {
            from: WireTarget::Label(crate::model::LabelRef { label: *new }),
            to: WireTarget::Terminal(pin(u2, 2)),
        }),
    )?;
    assert_eq!(net_of(&engine, pin(u2, 2))?, net_of(&engine, pin(u1, 1))?);
    Ok(())
}

/// A local label "SIG" on U1.A, drawn on that pin.
fn signal_label(engine: &mut Engine, u1: Id) -> Result<Id> {
    match apply(
        engine,
        Operation::Schematic(C::PlaceLabel {
            sheet: None,
            name: "SIG".into(),
            kind: LabelKind::Local,
            position: [100.0, 100.0],
            rotation: Rotation::R0,
            attach: Some(WireTarget::Terminal(pin(u1, 1))),
        }),
    )? {
        Outcome::LabelPlaced { label } => Ok(label),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

/// Both ends selected: the wire comes along without being named and joins
/// the two new pins, on a net of their own.
#[test]
fn a_wire_with_both_ends_selected_joins_the_pasted_pins() -> Result<()> {
    let Sheet {
        mut engine, u1, u2, ..
    } = wired()?;
    let clip = copied(
        &engine,
        vec![
            ClipObject::Instance { id: u1 },
            ClipObject::Instance { id: u2 },
        ],
    )?;
    assert_eq!(clip.wires.len(), 1);
    let report = pasted(&mut engine, clip, [0.0, -30.0])?;
    let [a, b] = report.symbols.as_slice() else {
        return Err(Error::Invalid("two symbols".into()));
    };
    let (a, b) = (a.instance, b.instance);
    wire_between(&engine, pin(a, 2), pin(b, 1))?;
    assert_eq!(net_of(&engine, pin(a, 2))?, net_of(&engine, pin(b, 1))?);
    assert_ne!(net_of(&engine, pin(a, 2))?, net_of(&engine, pin(u1, 2))?);
    assert_eq!(net_of(&engine, pin(u1, 2))?, net_of(&engine, pin(u2, 1))?);
    Ok(())
}

/// One end selected: the named wire follows the pasted symbol there and has
/// a free end, moved by the offset, where it lost the other.
#[test]
fn a_wire_with_one_end_selected_gets_a_free_end() -> Result<()> {
    let Sheet {
        mut engine,
        u1,
        wire,
        ..
    } = wired()?;
    let clip = copied(
        &engine,
        vec![
            ClipObject::Instance { id: u1 },
            ClipObject::Wire { id: wire },
        ],
    )?;
    let report = pasted(&mut engine, clip, [0.0, -30.0])?;
    let new = report.symbols.first().map(|s| s.instance);
    let new = new.ok_or(Error::Invalid("no symbol".into()))?;
    let at = crate::WireEnd::Terminal(pin(new, 2));
    let wire = engine.project().wires_at(at).next().cloned();
    let wire = wire.ok_or(Error::Invalid("no wire on the new pin".into()))?;
    let other = if wire.from == at { wire.to } else { wire.from };
    assert_eq!(other.free_point(), Some([15.0, -30.0]));
    Ok(())
}

/// A wire alone: both its ends are free, its shape kept.
#[test]
fn a_wire_alone_pastes_with_two_free_ends() -> Result<()> {
    let Sheet {
        mut engine, wire, ..
    } = wired()?;
    let clip = copied(&engine, vec![ClipObject::Wire { id: wire }])?;
    let report = pasted(&mut engine, clip, [0.0, -30.0])?;
    let new = report.wires.first().and_then(|w| engine.project().wire(*w));
    let new = new.ok_or(Error::Invalid("no wire".into()))?;
    assert_eq!(new.from.free_point(), Some([5.0, -30.0]));
    assert_eq!(new.to.free_point(), Some([15.0, -30.0]));
    Ok(())
}

/// A clip stands alone: what it copied may be gone, as after a cut, and a
/// drawing moves by the offset.
#[test]
fn a_clip_outlives_its_objects_and_drawings_shift() -> Result<()> {
    let mut engine = Engine::default();
    let drawing = match apply(
        &mut engine,
        Operation::Schematic(C::AddDrawing {
            sheet: None,
            item: SheetItem::Line {
                from: [0.0, 0.0],
                to: [5.0, 0.0],
            },
        }),
    )? {
        Outcome::DrawingAdded { drawing } => drawing,
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    let clip = copied(&engine, vec![ClipObject::Drawing { id: drawing }])?;
    apply(
        &mut engine,
        Operation::Schematic(C::RemoveDrawing { drawing }),
    )?;
    let report = pasted(&mut engine, clip, [1.0, 2.0])?;
    let new = report
        .drawings
        .first()
        .and_then(|d| engine.project().drawing(*d));
    assert_eq!(
        new.map(|d| d.item.clone()),
        Some(SheetItem::Line {
            from: [1.0, 2.0],
            to: [6.0, 2.0],
        })
    );
    Ok(())
}

/// The copy query and every refused paste leave the document, its revision
/// and its history as they were; so does a stale revision.
#[test]
fn refusals_change_nothing() -> Result<()> {
    let Sheet {
        mut engine,
        u1,
        wire,
        ..
    } = wired()?;
    let symbol = copied(&engine, vec![ClipObject::Instance { id: u1 }])?;
    let lost = copied(&engine, vec![ClipObject::Wire { id: wire }])?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let mut unknown_component = symbol.clone();
    for s in &mut unknown_component.symbols {
        s.instance.component.id = crate::DefinitionId(u64::MAX - 1);
    }
    let mut no_units = symbol.clone();
    for s in &mut no_units.symbols {
        s.occurrences.clear();
    }
    let mut foreign_end = lost.clone();
    for w in &mut foreign_end.wires {
        w.from = crate::WireEnd::Terminal(pin(u1, 1));
    }
    for (name, clip, offset) in [
        ("nothing", SheetClip::default(), [0.0, 0.0]),
        ("off range", symbol.clone(), [1e9, 0.0]),
        ("not finite", symbol.clone(), [f64::NAN, 0.0]),
        ("unknown component", unknown_component, [0.0, 10.0]),
        ("no units", no_units, [0.0, 10.0]),
        ("foreign wire end", foreign_end, [0.0, 10.0]),
    ] {
        assert!(
            engine.apply(revision, paste_op(clip, offset)).is_err(),
            "{name}"
        );
        assert_eq!(engine.project(), &before, "{name}");
        assert_eq!(engine.revision(), revision, "{name}");
    }
    let unknown_sheet = Operation::Schematic(C::Paste {
        sheet: Some(crate::SheetId(9)),
        clip: Box::new(symbol.clone()),
        offset: [0.0, 10.0],
    });
    assert!(engine.apply(revision, unknown_sheet).is_err());
    assert!(matches!(
        engine.apply(revision + 1, paste_op(symbol, [0.0, 10.0])),
        Err(Error::Stale { .. })
    ));
    assert_eq!(engine.project(), &before);
    // The query names only what exists on the sheet.
    assert!(copied(&engine, vec![]).is_err());
    assert!(copied(&engine, vec![ClipObject::Label { id: u1 }]).is_err());
    assert!(copied(&engine, vec![ClipObject::Wire { id: u1 }]).is_err());
    Ok(())
}

/// The command and query as a transport writes them: JSON in, JSON out, with
/// unknown parameters refused.
#[test]
fn the_wire_format_round_trips() -> Result<()> {
    let Sheet { mut engine, u1, .. } = wired()?;
    let query: Query = serde_json::from_value(serde_json::json!({
        "query": "schematic.copy",
        "params": { "objects": [{ "kind": "instance", "id": u1 }] },
    }))
    .map_err(|e| Error::Invalid(e.to_string()))?;
    let QueryResult::SheetClip(clip) = engine.query(query)? else {
        return Err(Error::Invalid("not a clip".into()));
    };
    let operation: Operation = serde_json::from_value(serde_json::json!({
        "editor": "Schematic",
        "operation": {
            "command": "schematic.paste",
            "params": { "clip": clip, "offset": [0.0, -30.0] },
        },
    }))
    .map_err(|e| Error::Invalid(e.to_string()))?;
    assert!(matches!(
        apply(&mut engine, operation)?,
        Outcome::Pasted { .. }
    ));
    assert!(
        serde_json::from_value::<Operation>(serde_json::json!({
            "editor": "Schematic",
            "operation": {
                "command": "schematic.paste",
                "params": { "clip": {}, "offset": [0.0, 0.0], "extra": 1 },
            },
        }))
        .is_err()
    );
    Ok(())
}
