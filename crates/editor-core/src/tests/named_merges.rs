//! Two nets that each carry a name of their own (VBUS and GND) never merge
//! silently. Whatever joins them (a label or power port placed or renamed,
//! a wire end, a junction, a paste, a sheet pin or a moved symbol), a direct
//! command is refused with both names and the document is unchanged; the
//! preview lists the merge, and only an update that asks for it after that
//! review merges them, the oldest net surviving.
use super::support::*;
use crate::model::{LabelKind, PowerStyle, SheetSide, WireEnd};
use crate::schematic::{ClipObject, SchematicCommand as C, WirePoint, WireTarget};
use crate::updates::{ApplyUpdate, Change};
use crate::{Engine, Error, Id, ObjectRef, Operation, Outcome, Point, Query, QueryResult, Result};

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

fn ground() -> LabelKind {
    LabelKind::Power {
        style: PowerStyle::Ground,
    }
}

fn supply() -> LabelKind {
    LabelKind::Power {
        style: PowerStyle::Bar,
    }
}

fn run(engine: &mut Engine, command: C) -> Result<Outcome> {
    apply(engine, Operation::Schematic(command))
}

fn port(engine: &mut Engine, name: &str, kind: LabelKind, at: WireTarget) -> Result<Id> {
    match run(
        engine,
        C::PlaceLabel {
            sheet: None,
            name: name.into(),
            kind,
            position: [0.0, 0.0],
            rotation: Default::default(),
            attach: Some(at),
        },
    )? {
        Outcome::LabelPlaced { label } => Ok(label),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

/// U1.B wired to U2.A along y = 0 from x = 5 to 15, named VBUS by a power
/// port on U2.A; U3.A at (-5, -40) named GND by a ground port.
struct Board {
    engine: Engine,
    f: Fixture,
    u1: Id,
    u2: Id,
    u3: Id,
    wire: Id,
    vbus_port: Id,
    gnd_port: Id,
    vbus: Id,
    gnd: Id,
}

fn board() -> Result<Board> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &f, "U2", 1, [20.0, 0.0])?;
    let (u3, _) = place(&mut engine, &f, "U3", 1, [0.0, -40.0])?;
    connect(&mut engine, ep(u1, 2), ep(u2, 1))?;
    let wire = wire_between(&engine, ep(u1, 2), ep(u2, 1))?;
    let vbus_port = port(&mut engine, "VBUS", supply(), ep(u2, 1).into())?;
    let gnd_port = port(&mut engine, "GND", ground(), ep(u3, 1).into())?;
    let vbus = net_of(&engine, ep(u1, 2))?;
    let gnd = net_of(&engine, ep(u3, 1))?;
    assert_eq!(engine.project().net_name(vbus), "VBUS");
    assert_eq!(engine.project().net_name(gnd), "GND");
    assert!(vbus < gnd, "VBUS is the older net");
    Ok(Board {
        engine,
        f,
        u1,
        u2,
        u3,
        wire,
        vbus_port,
        gnd_port,
        vbus,
        gnd,
    })
}

/// The GND port dropped on the VBUS wire at (10, 0).
fn gnd_on_vbus(wire: Id) -> C {
    C::PlaceLabel {
        sheet: None,
        name: "GND".into(),
        kind: ground(),
        position: [10.0, 0.0],
        rotation: Default::default(),
        attach: Some(WireTarget::OnWire(WirePoint {
            wire,
            at: [10.0, 0.0],
        })),
    }
}

/// `command` is refused as a named merge naming both nets, and nothing
/// changes.
fn refused(engine: &mut Engine, command: C, what: &str) -> Result<String> {
    let kept = Snapshot::of(engine);
    let error = match run(engine, command) {
        Err(Error::Conflict(message)) => message,
        other => return Err(Error::Invalid(format!("{what}: {other:?}"))),
    };
    kept.assert_kept(engine, what);
    assert!(error.starts_with("net.named_merge:"), "{what}: {error}");
    assert!(
        error.contains("VBUS") && error.contains("GND"),
        "{what}: {error}"
    );
    assert!(error.contains("merge_named_nets"), "{what}: {error}");
    Ok(error)
}

fn reviewed(engine: &mut Engine, command: C, merge: bool) -> Result<Outcome> {
    let revision = engine.revision();
    apply(
        engine,
        Operation::Update(Box::new(ApplyUpdate {
            expected_revision: revision,
            change: Change::Schematic { command },
            remove_conflicting_tracks: false,
            merge_named_nets: merge,
        })),
    )
}

#[test]
fn a_gnd_port_dropped_on_a_vbus_wire_is_refused_until_a_review_merges_them() -> Result<()> {
    let Board {
        mut engine,
        u1,
        u3,
        wire,
        vbus,
        gnd,
        ..
    } = board()?;
    let error = refused(&mut engine, gnd_on_vbus(wire), "GND port on VBUS")?;
    assert!(error.contains("the power port GND"), "{error}");

    // The preview names both nets, the net they become and what joins them.
    let plan = engine.preview_update(Change::Schematic {
        command: gnd_on_vbus(wire),
    });
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(plan.needs_review() && !plan.touches_routed());
    let [merge] = plan.merges_named.as_slice() else {
        return Err(Error::Invalid(format!("{:?}", plan.merges_named)));
    };
    let names: Vec<(Id, &str)> = merge
        .nets
        .iter()
        .map(|n| (n.net, n.name.as_str()))
        .collect();
    assert_eq!(names, vec![(vbus, "VBUS"), (gnd, "GND")]);
    assert_eq!(merge.net, vbus, "the oldest net survives");
    assert_eq!(merge.name, "GND", "a ground port wins the name");
    assert!(
        matches!(merge.joined_by.first(), Some(ObjectRef::Label { .. })),
        "{:?}",
        merge.joined_by
    );
    let line = plan
        .warning_lines
        .iter()
        .find(|l| l.line == "Joins VBUS and GND into one net, GND")
        .ok_or_else(|| Error::Invalid(format!("{:?}", plan.warning_lines)))?;
    assert!(line.refs.contains(&ObjectRef::Net { id: vbus }));
    assert!(line.refs.contains(&ObjectRef::Net { id: gnd }));

    // An update that does not ask for the merge is refused too.
    let kept = Snapshot::of(&engine);
    let unasked = reviewed(&mut engine, gnd_on_vbus(wire), false);
    assert!(
        matches!(&unasked, Err(Error::Conflict(m)) if m.starts_with("net.named_merge:")),
        "{unasked:?}"
    );
    kept.assert_kept(&engine, "an update without merge_named_nets");

    // Asked for after the review, it merges them: one net, the oldest ID.
    reviewed(&mut engine, gnd_on_vbus(wire), true)?;
    assert_eq!(net_of(&engine, ep(u1, 2))?, vbus);
    assert_eq!(net_of(&engine, ep(u3, 1))?, vbus);
    assert!(engine.project().net(gnd).is_none());
    assert_eq!(engine.project().net_name(vbus), "GND");
    engine.undo()?;
    assert_eq!(net_of(&engine, ep(u3, 1))?, gnd);
    Ok(())
}

#[test]
fn joining_an_unnamed_net_or_the_same_name_stays_direct() -> Result<()> {
    let Board {
        mut engine,
        f,
        u1,
        u3,
        wire,
        vbus,
        gnd,
        ..
    } = board()?;
    // A second VBUS port on the VBUS wire merges nothing.
    run(
        &mut engine,
        C::PlaceLabel {
            sheet: None,
            name: "VBUS".into(),
            kind: supply(),
            position: [10.0, 0.0],
            rotation: Default::default(),
            attach: Some(WireTarget::OnWire(WirePoint {
                wire,
                at: [10.0, 0.0],
            })),
        },
    )?;
    // An unnamed net joins a named one at once (U1.A has no label).
    connect(&mut engine, ep(u1, 1), ep(u3, 1))?;
    let joined = net_of(&engine, ep(u3, 1))?;
    assert_eq!(net_of(&engine, ep(u1, 1))?, joined);
    assert!(joined < gnd, "the older, unnamed net's ID survives");
    assert_eq!(engine.project().net_name(joined), "GND");
    // A ground port on an unnamed pin elsewhere joins the GND net.
    let (u4, _) = place(&mut engine, &f, "U4", 1, [60.0, 0.0])?;
    port(&mut engine, "GND", ground(), ep(u4, 1).into())?;
    assert_eq!(net_of(&engine, ep(u4, 1))?, joined);
    assert_eq!(net_of(&engine, ep(u1, 2))?, vbus);
    Ok(())
}

#[test]
fn every_path_that_joins_named_nets_goes_through_the_same_review() -> Result<()> {
    let Board {
        mut engine,
        u1,
        u2,
        u3,
        wire,
        vbus_port,
        gnd_port,
        ..
    } = board()?;
    // Renaming the VBUS port to GND.
    refused(
        &mut engine,
        C::EditLabel {
            label: vbus_port,
            name: Some("GND".into()),
            kind: None,
            rotation: None,
        },
        "renaming a port",
    )?;
    // A wire from a GND pin to a VBUS pin.
    refused(
        &mut engine,
        C::Connect {
            from: ep(u3, 1).into(),
            to: ep(u2, 1).into(),
        },
        "a wire end",
    )?;
    // A wire end moved onto the other net.
    refused(
        &mut engine,
        C::Connect {
            from: WireTarget::Label(crate::model::LabelRef { label: gnd_port }),
            to: WireTarget::OnWire(WirePoint {
                wire,
                at: [10.0, 0.0],
            }),
        },
        "a wire from the GND port onto the VBUS wire",
    )?;
    // A junction where a GND wire crosses the VBUS wire. The crossing alone
    // joins nothing.
    run(
        &mut engine,
        C::DrawWire {
            sheet: None,
            from: ep(u3, 1).into(),
            corners: vec![
                crate::model::Corner::sharp([-5.0, -50.0]),
                crate::model::Corner::sharp([10.0, -50.0]),
            ],
            to: crate::model::FreeEnd::new([10.0, 10.0])
                .map(WireTarget::Free)
                .ok_or_else(|| Error::Invalid("free end".into()))?,
        },
    )?;
    assert_ne!(net_of(&engine, ep(u1, 2))?, net_of(&engine, ep(u3, 1))?);
    refused(
        &mut engine,
        C::PlaceJunction {
            sheet: None,
            position: [10.0, 0.0],
        },
        "a junction on the crossing",
    )?;
    // Pasting a ground stub whose free end lands on the VBUS wire.
    let stub = engine
        .project()
        .wires_at(WireEnd::label(gnd_port))
        .next()
        .map(|w| w.id)
        .ok_or_else(|| Error::Invalid("the GND port's wire".into()))?;
    let clip = match engine.query(Query::SheetCopy {
        sheet: None,
        objects: vec![
            ClipObject::Label { id: gnd_port },
            ClipObject::Wire { id: stub },
        ],
    })? {
        QueryResult::SheetClip(clip) => clip,
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    let pin: Point = crate::schematic::pin_position(engine.project(), ep(u3, 1))
        .ok_or_else(|| Error::Invalid("pin".into()))?;
    refused(
        &mut engine,
        C::Paste {
            sheet: None,
            clip,
            offset: [8.0 - pin[0], -pin[1]],
        },
        "a pasted ground stub",
    )?;
    Ok(())
}

#[test]
fn renaming_a_sheet_pin_onto_another_named_net_is_reviewed() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [0.0, -80.0])?;
    port(&mut engine, "VBUS", supply(), ep(u1, 2).into())?;
    let (symbol, child) = match run(
        &mut engine,
        C::PlaceSheetSymbol {
            sheet: None,
            child: None,
            name: Some("Power".into()),
            position: [60.0, -70.0],
            size: [30.0, 20.0],
        },
    )? {
        Outcome::SheetSymbolPlaced { symbol, child } => (symbol, child),
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    let u2 = match apply(
        &mut engine,
        Operation::Instance(crate::InstanceCommand::Place(
            crate::instances::NewInstance {
                sheet: Some(child),
                component: f.r1(),
                designator: Some("U2".into()),
                properties: vec![],
                representation: f.symbol,
                unit: crate::UnitId(1),
                position: [40.0, -40.0],
                physical: Some(f.physical()),
            },
        )),
    )? {
        Outcome::InstancePlaced { instance, .. } => instance,
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    };
    let hier = LabelKind::Hierarchical {
        shape: Default::default(),
    };
    for (name, terminal) in [("VBUS", 1), ("GND", 2)] {
        run(
            &mut engine,
            C::PlaceLabel {
                sheet: Some(child),
                name: name.into(),
                kind: hier,
                position: [0.0, 0.0],
                rotation: Default::default(),
                attach: Some(ep(u2, terminal).into()),
            },
        )?;
    }
    let pin = match run(
        &mut engine,
        C::AddSheetPin {
            symbol,
            name: "VBUS".into(),
            shape: Default::default(),
            side: SheetSide::Left,
            offset: 10.0,
        },
    )? {
        Outcome::SheetPinsAdded { pins } => pins.first().copied(),
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
    .ok_or_else(|| Error::Invalid("pin".into()))?;
    // The same name on both sides of the pin joins directly.
    run(
        &mut engine,
        C::Connect {
            from: ep(u1, 2).into(),
            to: WireTarget::SheetPin(crate::model::SheetPinRef { sheet_pin: pin }),
        },
    )?;
    assert_eq!(net_of(&engine, ep(u1, 2))?, net_of(&engine, ep(u2, 1))?);
    // Renamed GND, it would join the parent's VBUS to the child's GND.
    let rename = C::EditSheetPin {
        pin,
        name: Some("GND".into()),
        shape: None,
        side: None,
        offset: None,
    };
    let error = refused(&mut engine, rename.clone(), "a renamed sheet pin")?;
    assert!(error.contains("the sheet pin GND"), "{error}");
    reviewed(&mut engine, rename, true)?;
    assert_eq!(net_of(&engine, ep(u1, 2))?, net_of(&engine, ep(u2, 2))?);
    Ok(())
}
