//! CE-05: dual amplifier with a separate power unit, and regrouping a
//! terminal into another unit through a reviewed revision update.
use super::support::*;
use crate::instances::{NewInstance, UnitState};
use crate::schematic::SchematicCommand;
use crate::updates::{
    Change, RegroupAction, RegroupResolution, Resolutions, UnitResolution, UnitTarget,
};
use crate::{
    DefinitionKind, Engine, Error, Id, InstanceCommand, ObjectRef, Operation, Result, Stage,
    SymbolRef, TerminalId, UnitId, format,
};

const A: UnitId = UnitId(1);
const B: UnitId = UnitId(2);
const P: UnitId = UnitId(3);

fn spec() -> Spec<'static> {
    let amp = |first: u32| {
        vec![
            (first, [-5.0, 2.0]),
            (first + 1, [-5.0, -2.0]),
            (first + 2, [5.0, 0.0]),
        ]
    };
    Spec {
        name: "Dual amplifier",
        terminals: vec![
            (1, "A+", input()),
            (2, "A-", input()),
            (3, "Aout", output()),
            (4, "B+", input()),
            (5, "B-", input()),
            (6, "Bout", output()),
            (7, "V+", power_in()),
            (8, "V-", power_in()),
        ],
        units: vec![
            (1, "A", amp(1)),
            (2, "B", amp(4)),
            (3, "P", vec![(7, [0.0, 5.0]), (8, [0.0, -5.0])]),
        ],
        package: (1..=8)
            .map(|i| (i, ["1", "2", "3", "4", "5", "6", "7", "8"][i as usize - 1]))
            .collect(),
        mapping: vec![
            (1, 3),
            (2, 2),
            (3, 1),
            (4, 8),
            (5, 4),
            (6, 5),
            (7, 6),
            (8, 7),
        ],
        physical: true,
    }
}

/// Apply one operation and check that it is one undo step: undo restores the
/// exact prior document and redo the exact result.
fn one_step(engine: &mut Engine, operation: Operation) -> Result<()> {
    let before = engine.document().clone();
    apply(engine, operation)?;
    let after = engine.document().clone();
    engine.undo()?;
    assert_eq!(engine.document(), &before, "undo restores the prior state");
    engine.redo()?;
    assert_eq!(engine.document(), &after, "redo restores the result");
    Ok(())
}

fn unused(engine: &mut Engine, instance: Id, unit: UnitId) -> Result<()> {
    one_step(
        engine,
        Operation::Instance(InstanceCommand::SetUnitUnused {
            instance,
            unit,
            unused: true,
        }),
    )
}

fn no_connect(engine: &mut Engine, instance: Id, terminals: &[u32]) -> Result<()> {
    one_step(
        engine,
        Operation::Instance(InstanceCommand::SetNoConnect {
            instance,
            terminals: terminals.iter().map(|t| TerminalId(*t)).collect(),
            no_connect: true,
        }),
    )
}

fn electrical(engine: &Engine, instance: Id) -> Result<Vec<(String, Vec<ObjectRef>)>> {
    let report = inspect(engine, instance)?;
    let findings: Vec<_> = report
        .findings
        .into_iter()
        .filter(|f| f.blocks_stage(Stage::ElectricallyResolved))
        .collect();
    Ok(codes(&findings))
}

fn unit(instance: Id, unit: UnitId) -> ObjectRef {
    ObjectRef::Unit { instance, unit }
}
fn terminal(instance: Id, t: u32) -> ObjectRef {
    ObjectRef::Terminal {
        endpoint: ep(instance, t),
    }
}
fn expect(mut items: Vec<(&str, ObjectRef)>) -> Vec<(String, Vec<ObjectRef>)> {
    items.sort_by(|a, b| format!("{:?}", (a.0, vec![a.1])).cmp(&format!("{:?}", (b.0, vec![b.1]))));
    let mut all: Vec<_> = items
        .into_iter()
        .map(|(code, object)| (code.to_string(), vec![object]))
        .collect();
    all.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    all
}

#[test]
fn ce05_power_unit_decisions_and_regrouping_are_explicit() -> Result<()> {
    let mut engine = Engine::default();
    // A typed gain field, selected by the amplifier component.
    let gain = publish_field(
        &mut engine,
        "Gain",
        crate::fields::FieldDefinition {
            next_local: 1,
            value_type: crate::fields::ValueType::Integer { min: 1, max: 1000 },
            default: Some(crate::fields::FieldValue::Integer(1)),
            required_at: None,
        },
    )?;
    let amp = component_with_fields(&mut engine, &spec(), vec![select(gain)])?;
    let supply = component(&mut engine, &probe("Supply", power_out()))?;
    let probe = component(&mut engine, &probe("Probe", passive()))?;

    // Two independent instances.
    let (u1, _) = place(&mut engine, &amp, "U1", 1, [0.0, 0.0])?;
    set_properties(&mut engine, u1, vec![text(gain, "10")])?;

    // U1A + U1P, B unplaced.
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::PlaceUnit {
            sheet: None,
            instance: u1,
            unit: P,
            position: pt([0.0, 20.0]),
        }),
    )?;
    let report = inspect(&engine, u1)?;
    assert_eq!(report.units[1].state, UnitState::Unplaced);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == "instance.unit_unplaced" && f.objects == vec![unit(u1, B)])
    );

    // Mark B unused; its terminals still need explicit decisions.
    unused(&mut engine, u1, B)?;
    assert_eq!(inspect(&engine, u1)?.units[1].state, UnitState::Unused);
    assert!(
        engine
            .project()
            .instance(u1)
            .is_some_and(|i| i.no_connect.is_empty())
    );
    let findings = electrical(&engine, u1)?;
    for t in [4, 5, 6] {
        assert!(findings.contains(&(
            "instance.terminal_unaccounted".to_string(),
            vec![terminal(u1, t)]
        )));
    }
    no_connect(&mut engine, u1, &[4, 5, 6])?;

    // Wire U1: signals to passive probes, supplies to sources on VP and VN.
    for (i, t) in [1u32, 2, 3].into_iter().enumerate() {
        let (p, _) = place(
            &mut engine,
            &probe,
            &format!("TP{t}"),
            1,
            [20.0, i as f64 * 5.0],
        )?;
        connect(&mut engine, ep(u1, t), ep(p, 1))?;
    }
    for (t, name) in [(7, "VP"), (8, "VN")] {
        let (s, _) = place(
            &mut engine,
            &supply,
            &format!("S{t}"),
            1,
            [-20.0, t as f64 * 5.0],
        )?;
        connect(&mut engine, ep(u1, t), ep(s, 1))?;
        let net = net_of(&engine, ep(u1, t))?;
        apply(
            &mut engine,
            Operation::Schematic(SchematicCommand::RenameNet {
                net,
                name: name.into(),
            }),
        )?;
        assert_eq!(
            engine.project().net(net).map(|n| n.terminals.len()),
            Some(2)
        );
    }
    assert_eq!(electrical(&engine, u1)?, vec![]);
    assert!(inspect(&engine, u1)?.electrically_resolved);

    // U2B without U2P.
    let Ok(crate::Outcome::InstancePlaced { instance: u2, .. }) = apply(
        &mut engine,
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: amp.r1(),
            designator: Some("U2".into()),
            properties: vec![text(gain, "2")],
            representation: amp.symbol,
            unit: B,
            position: pt([60.0, 0.0]),
            physical: Some(amp.physical()),
        })),
    ) else {
        return Err(Error::Invalid("U2 not placed".into()));
    };
    for (i, t) in [4u32, 5, 6].into_iter().enumerate() {
        let (p, _) = place(
            &mut engine,
            &probe,
            &format!("TQ{t}"),
            1,
            [80.0, i as f64 * 5.0],
        )?;
        connect(&mut engine, ep(u2, t), ep(p, 1))?;
    }
    let gain_of = |engine: &Engine, instance| -> Result<_> {
        Ok(inspect(engine, instance)?
            .properties
            .into_iter()
            .map(|f| (f.value, f.origin))
            .collect::<Vec<_>>())
    };
    use crate::fields::{FieldValue, Origin};
    assert_eq!(
        gain_of(&engine, u1)?,
        vec![(Some(FieldValue::Integer(10)), Origin::Assigned)]
    );
    assert_eq!(
        gain_of(&engine, u2)?,
        vec![(Some(FieldValue::Integer(2)), Origin::Assigned)]
    );
    // Editing one instance leaves the other's effective values unchanged.
    set_properties(
        &mut engine,
        u2,
        vec![crate::fields::PropertyEdit {
            field: gain.id,
            input: crate::fields::PropertyInput::Inherit,
        }],
    )?;
    assert_eq!(
        gain_of(&engine, u2)?,
        vec![(Some(FieldValue::Integer(1)), Origin::FieldDefault)]
    );
    assert_eq!(
        gain_of(&engine, u1)?,
        vec![(Some(FieldValue::Integer(10)), Origin::Assigned)]
    );
    assert_eq!(
        electrical(&engine, u2)?,
        expect(vec![
            ("instance.unit_unplaced", unit(u2, A)),
            ("instance.unit_unplaced", unit(u2, P)),
            ("instance.terminal_unaccounted", terminal(u2, 1)),
            ("instance.terminal_unaccounted", terminal(u2, 2)),
            ("instance.terminal_unaccounted", terminal(u2, 3)),
            ("instance.terminal_unaccounted", terminal(u2, 7)),
            ("instance.terminal_unaccounted", terminal(u2, 8)),
        ])
    );
    unused(&mut engine, u2, A)?;
    unused(&mut engine, u2, P)?;
    no_connect(&mut engine, u2, &[1, 2, 3, 7, 8])?;
    assert_eq!(
        electrical(&engine, u2)?,
        expect(vec![
            ("instance.required_terminal_no_connect", terminal(u2, 7)),
            ("instance.required_terminal_no_connect", terminal(u2, 8)),
        ])
    );
    assert!(electrical(&engine, u1)?.is_empty(), "U1 is unaffected");

    // However many units each draws, every instance is one package: one
    // physical identity, one board placement, one BOM line.
    apply(
        &mut engine,
        Operation::Layout(crate::layout::LayoutCommand::Sync),
    )?;
    for (instance, occurrences) in [(u1, 2), (u2, 1)] {
        let report = inspect(&engine, instance)?;
        assert_eq!(report.occurrences, occurrences);
        assert_eq!(report.board_placements, 1);
        assert_eq!(report.bom_quantity, 1);
        assert_eq!(
            report.physical.map(|p| p.package),
            Some(crate::PackageRef::new(amp.package, crate::Revision::FIRST))
        );
        let pads: Vec<_> = crate::layout::pads(engine.project())
            .into_iter()
            .filter(|p| p.instance == instance)
            .map(|p| p.pad)
            .collect();
        assert_eq!(pads.len(), 8, "one footprint's pads, not one per unit");
    }
    assert_eq!(
        engine
            .project()
            .placements
            .iter()
            .filter(|p| p.instance == u1 || p.instance == u2)
            .count(),
        2
    );

    // Regroup terminal 6 from unit B into unit A in a symbol r2.
    for id in [amp.symbol, amp.component] {
        apply(
            &mut engine,
            Operation::Library(crate::LibraryCommand::DiscardDraft { id }),
        )?;
    }
    draft_from(
        &mut engine,
        crate::DefinitionRef {
            kind: DefinitionKind::Symbol,
            id: amp.symbol,
            revision: crate::Revision(1),
        },
    )?;
    draft_from(&mut engine, amp.r1().erased())?;
    with(&mut engine, amp.symbol, symbol_body, |s| {
        if let Some(pin) = s.units[1]
            .pins
            .iter()
            .position(|a| a.terminal == TerminalId(6))
        {
            s.units[1].pins.remove(pin);
        }
        s.units[0].pins.push(crate::library::Pin::new(
            TerminalId(6),
            pt([5.0, -4.0]),
            crate::library::PinDirection::Left,
            nm(2.5),
        ));
    })?;
    with(&mut engine, amp.component, component_body, |c| {
        c.representations = vec![SymbolRef::new(amp.symbol, crate::Revision(2))]
    })?;
    let published = publish(&mut engine, vec![amp.symbol, amp.component])?;
    let target = crate::ComponentRef::new(amp.component, crate::Revision(2));
    assert!(published.contains(&target.erased()));

    let revise = |instances: Vec<Id>, resolutions: Resolutions| Change::ComponentRevision {
        instances,
        target,
        resolutions,
    };
    let plan = engine.preview_update(revise(vec![u1], Resolutions::default()));
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        plan.changes
            .iter()
            .any(|c| c
                == "U1 terminal 6 moves from unit B to unit A; pin [5.0, 0.0] → [5.0, -4.0] mm."),
        "{:#?}",
        plan.changes
    );
    let nets = engine.project().nets.clone();
    let before = engine.document().clone();
    engine.apply_update(plan.revision, plan.change, false)?;
    let after = engine.document().clone();
    engine.undo()?;
    assert_eq!(
        engine.document(),
        &before,
        "one undo step reverts the regroup"
    );
    engine.redo()?;
    assert_eq!(engine.document(), &after, "redo restores the regroup");
    let after = engine
        .project()
        .instance(u1)
        .cloned()
        .ok_or_else(|| Error::NotFound("U1".into()))?;
    assert_eq!(after.component, target);
    assert!(after.no_connect.contains(&TerminalId(6)));
    assert_eq!(engine.project().nets, nets);

    // U2's terminal 6 is wired and unit A is unused: blocked until resolved.
    let blocked = engine.preview_update(revise(vec![u2], Resolutions::default()));
    assert!(
        blocked
            .blockers
            .iter()
            .any(|b| b.contains("terminal 6") && b.contains("RegroupResolution")),
        "{:?}",
        blocked.blockers
    );
    let before = engine.document().clone();
    let revision = engine.revision();
    assert!(engine.apply_update(revision, blocked.change, true).is_err());
    assert_eq!(engine.document(), &before);
    let wire6 = engine
        .project()
        .wires_at(ep(u2, 6))
        .map(|w| w.id)
        .next()
        .ok_or_else(|| Error::NotFound("wire".into()))?;
    let regroup = |action| Resolutions {
        regroups: vec![RegroupResolution {
            instance: u2,
            terminal: TerminalId(6),
            action,
        }],
        ..Default::default()
    };
    let disconnect =
        engine.preview_update(revise(vec![u2], regroup(RegroupAction::AcceptDisconnect)));
    assert!(disconnect.blockers.is_empty(), "{:?}", disconnect.blockers);
    // The wire from U2.6, with the rest of its run when it bends around
    // other pins through junction points.
    assert!(disconnect.removed_wires.contains(&wire6));
    let mut removed = disconnect.removed_wires.clone();
    removed.sort_unstable();
    assert_eq!(removed, wire_run(&engine, ep(u2, 6)));
    let placed = engine.preview_update(revise(
        vec![u2],
        // Not straight below U2's Aout: terminal 6's wire follows it from
        // (65, 10), and a wire running over the no-connect pin would join it.
        regroup(RegroupAction::PlaceUnit {
            position: pt([40.0, -20.0]),
        }),
    ));
    assert!(placed.blockers.is_empty(), "{:?}", placed.blockers);
    assert!(placed.removed_wires.is_empty());
    engine.apply_update(placed.revision, placed.change, false)?;
    let report = inspect(&engine, u2)?;
    assert!(matches!(report.units[0].state, UnitState::Placed { .. }));
    assert!(
        engine
            .project()
            .instance(u2)
            .is_some_and(|i| !i.unused_units.contains(&A)),
        "placed and unused are exclusive"
    );
    engine.undo()?;
    assert_eq!(engine.document(), &before);

    // A unit with an occurrence disappears: blocked until a UnitResolution.
    with(&mut engine, amp.symbol, symbol_body, |s| {
        let moved: Vec<_> = s.units[1].pins.drain(..).collect();
        s.units[0].pins.extend(moved);
        s.units.remove(1);
    })?;
    with(&mut engine, amp.component, component_body, |c| {
        c.representations = vec![SymbolRef::new(amp.symbol, crate::Revision(3))]
    })?;
    publish(&mut engine, vec![amp.symbol, amp.component])?;
    let target3 = crate::ComponentRef::new(amp.component, crate::Revision(3));
    let gone = |resolutions| Change::ComponentRevision {
        instances: vec![u2],
        target: target3,
        resolutions,
    };
    let blocked = engine.preview_update(gone(Resolutions::default()));
    assert!(
        blocked
            .blockers
            .iter()
            .any(|b| b.contains("UnitResolution")),
        "{:?}",
        blocked.blockers
    );
    let dropped = engine.preview_update(gone(Resolutions {
        units: vec![UnitResolution {
            instance: u2,
            unit: B,
            target: UnitTarget::Drop,
        }],
        ..Default::default()
    }));
    assert!(dropped.blockers.is_empty(), "{:?}", dropped.blockers);
    // The three wires of terminals 4–6, each with the rest of its run
    // where it bends around other pins.
    let mut removed = dropped.removed_wires.clone();
    removed.sort_unstable();
    let mut runs: Vec<Id> = (4..=6).flat_map(|t| wire_run(&engine, ep(u2, t))).collect();
    runs.sort_unstable();
    assert_eq!(removed, runs);
    assert!((4..=6).all(|t| !wire_run(&engine, ep(u2, t)).is_empty()));

    // Save and reopen keeps pins, unit states and markers.
    let reopened = Engine::from_file(format::decode(&format::encode(&engine.to_file())?)?)?;
    assert_eq!(reopened.project(), engine.project());
    assert_eq!(inspect(&reopened, u2)?, inspect(&engine, u2)?);
    Ok(())
}
