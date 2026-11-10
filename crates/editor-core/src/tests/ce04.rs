//! CE-04: two transistors in one package. Expected positions are the
//! occurrence position plus the hand-written fixture pin positions.
use super::support::*;
use crate::instances::{NewInstance, UnitState};
use crate::layout::{self, LayoutCommand, RouteRequest};
use crate::schematic::{SchematicCommand, pin_position};
use crate::updates::{Change, InstanceChange};
use crate::{
    Engine, Error, Id, InstanceCommand, Layer, ObjectRef, Operation, Outcome, Result, Stage,
    UnitId, format,
};

const A: UnitId = UnitId(1);
const B: UnitId = UnitId(2);

fn spec() -> Spec<'static> {
    let pins = |first: u32| {
        vec![
            (first, [-5.0, 0.0]),
            (first + 1, [5.0, 2.5]),
            (first + 2, [5.0, -2.5]),
        ]
    };
    Spec {
        name: "Dual transistor",
        terminals: vec![
            (1, "B_A", passive()),
            (2, "C_A", passive()),
            (3, "E_A", passive()),
            (4, "B_B", passive()),
            (5, "C_B", passive()),
            (6, "E_B", passive()),
        ],
        units: vec![(1, "A", pins(1)), (2, "B", pins(4))],
        package: vec![(1, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5"), (6, "6")],
        // Terminal 1→"2", 2→"1", 3→"3", 4→"5", 5→"6", 6→"4".
        mapping: vec![(2, 1), (1, 2), (3, 3), (5, 4), (6, 5), (4, 6)],
        physical: true,
    }
}

fn load() -> Spec<'static> {
    Spec {
        name: "Load",
        terminals: vec![(1, "1", passive()), (2, "2", passive())],
        units: vec![(1, "A", vec![(1, [-2.0, 0.0]), (2, [2.0, 0.0])])],
        package: vec![(1, "1"), (2, "2")],
        mapping: vec![(1, 1), (2, 2)],
        physical: true,
    }
}

/// Apply, then prove undo restores the previous document and redo the new one.
fn step(engine: &mut Engine, operation: Operation) -> Result<Outcome> {
    let before = engine.document().clone();
    let outcome = apply(engine, operation)?;
    let after = engine.document().clone();
    engine.undo()?;
    assert_eq!(engine.document(), &before);
    engine.redo()?;
    assert_eq!(engine.document(), &after);
    Ok(outcome)
}

fn positions(engine: &Engine, q: Id) -> Vec<Option<[f64; 2]>> {
    (1..=6)
        .map(|t| pin_position(engine.project(), ep(q, t)))
        .collect()
}

fn states(engine: &Engine, q: Id) -> Result<Vec<(UnitId, UnitState)>> {
    Ok(inspect(engine, q)?
        .units
        .into_iter()
        .map(|u| (u.unit, u.state))
        .collect())
}

#[test]
fn ce04_two_units_share_one_package_placement_and_identity() -> Result<()> {
    let mut engine = Engine::default();
    let q = component(&mut engine, &spec())?;
    let l = component(&mut engine, &load())?;
    assert_eq!(engine.project().library.components.len(), 2);

    // Place Q1A; Q1B stays unplaced.
    let outcome = step(
        &mut engine,
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: q.r1(),
            designator: Some("Q1".into()),
            properties: vec![],
            representation: q.symbol,
            unit: A,
            position: pt([10.0, 10.0]),
            physical: None,
        })),
    )?;
    let Outcome::InstancePlaced {
        instance: q1,
        occurrence: occ_a,
        ..
    } = outcome
    else {
        return Err(Error::Invalid("no placement outcome".into()));
    };
    assert_eq!(
        states(&engine, q1)?,
        vec![
            (A, UnitState::Placed { occurrence: occ_a }),
            (B, UnitState::Unplaced)
        ]
    );
    let unplaced: Vec<_> = inspect(&engine, q1)?
        .findings
        .into_iter()
        .filter(|f| f.code == "instance.unit_unplaced")
        .collect();
    assert_eq!(unplaced.len(), 1);
    assert_eq!(
        unplaced[0].objects,
        vec![ObjectRef::Unit {
            instance: q1,
            unit: B
        }]
    );
    assert_eq!(unplaced[0].blocks, Some(Stage::ElectricallyResolved));

    // Place B elsewhere.
    let Outcome::UnitPlaced { occurrence: occ_b } = step(
        &mut engine,
        Operation::Schematic(SchematicCommand::PlaceUnit {
            sheet: None,
            instance: q1,
            unit: B,
            position: pt([30.0, 10.0]),
        }),
    )?
    else {
        return Err(Error::Invalid("no unit outcome".into()));
    };
    assert_eq!(
        positions(&engine, q1),
        [
            [5.0, 10.0],
            [15.0, 12.5],
            [15.0, 7.5],
            [25.0, 10.0],
            [35.0, 12.5],
            [35.0, 7.5]
        ]
        .map(Some)
    );

    // Two circuits, one board placement.
    let (l1, _) = place(&mut engine, &l, "L1", 1, [0.0, 30.0])?;
    let (l2, _) = place(&mut engine, &l, "L2", 1, [40.0, 30.0])?;
    for (from, to) in [
        (ep(q1, 1), ep(l1, 1)),
        (ep(q1, 2), ep(l1, 2)),
        (ep(q1, 4), ep(l2, 1)),
        (ep(q1, 5), ep(l2, 2)),
    ] {
        connect(&mut engine, from, to)?;
    }
    step(
        &mut engine,
        Operation::Instance(InstanceCommand::SetPhysical {
            instance: q1,
            choice: Some(q.physical()),
        }),
    )?;
    crate::tests::support::sync_onto_board(&mut engine)?;
    let placements: Vec<_> = engine
        .project()
        .placements
        .iter()
        .filter(|p| p.instance == q1)
        .collect();
    assert_eq!(placements.len(), 1);
    let report = inspect(&engine, q1)?;
    assert_eq!(
        (
            report.occurrences,
            report.board_placements,
            report.bom_quantity
        ),
        (2, 1, 1)
    );
    assert!(report.board_placeable);
    // Terminal 1 lands on package terminal "2": pad 2 carries net of Q1.1.
    let pads = layout::pads(engine.project());
    let pad_of = |terminal: u32| {
        pads.iter()
            .filter(|p| p.terminal == ep(q1, terminal))
            .map(|p| p.pad.0)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        (1..=6).map(pad_of).collect::<Vec<_>>(),
        vec![vec![2], vec![1], vec![3], vec![5], vec![6], vec![4]]
    );
    // Route a stub on Q1.1's net. It joins Q1.1's pad alone, so unplacing
    // A or removing Q1 leaves it in place: only copper that would join pads
    // of different nets is offered for removal (slice 4 of
    // `docs/copper-graph.md`).
    let land = pads
        .iter()
        .find(|p| p.terminal == ep(q1, 1))
        .ok_or_else(|| Error::NotFound("pad".into()))?;
    let net_a = net_of(&engine, ep(q1, 1))?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net: net_a,
                start: land.position,
                end: [land.position[0], land.position[1] - 2.0],
                width: 0.3,
                layer: Layer::Top.into(),
            },
        }),
    )?;
    let track = engine.project().tracks[0].id;

    // Move both units: presentation only. (Exchanging them would drag A's
    // wires across B's pins, which would connect them.)
    let nets = engine.project().nets.clone();
    let pads = layout::pads(engine.project());
    for (occurrence, position) in [(occ_a, [10.0, -10.0]), (occ_b, [30.0, -10.0])] {
        step(
            &mut engine,
            Operation::Schematic(SchematicCommand::MoveUnit {
                occurrence,
                position: pt(position),
            }),
        )?;
    }
    assert_eq!(engine.project().nets, nets);
    assert_eq!(layout::pads(engine.project()), pads);
    assert_eq!(
        positions(&engine, q1),
        [
            [5.0, -10.0],
            [15.0, -7.5],
            [15.0, -12.5],
            [25.0, -10.0],
            [35.0, -7.5],
            [35.0, -12.5]
        ]
        .map(Some)
    );

    // Reannotate: every identity stays.
    let ids = |e: &Engine| {
        (
            e.project()
                .instances
                .iter()
                .map(|i| i.id)
                .collect::<Vec<_>>(),
            e.project().occurrences.clone(),
            e.project().nets.clone(),
            e.project().tracks.clone(),
            e.project().wires.clone(),
        )
    };
    let before = ids(&engine);
    step(
        &mut engine,
        Operation::Instance(InstanceCommand::SetDesignator {
            instance: q1,
            designator: "Q7".into(),
        }),
    )?;
    assert_eq!(ids(&engine), before);
    assert_eq!(layout::pads(engine.project()), pads);
    let labels: Vec<_> = inspect(&engine, q1)?
        .units
        .into_iter()
        .map(|u| u.display)
        .collect();
    assert_eq!(labels, ["Q7A", "Q7B"]);

    // Unplace an unwired unit: disconnect B's wires first.
    for (from, to) in [(ep(q1, 4), ep(l2, 1)), (ep(q1, 5), ep(l2, 2))] {
        let wire = wire_between(&engine, from, to)?;
        apply(
            &mut engine,
            Operation::Schematic(SchematicCommand::Disconnect { wire }),
        )?;
    }
    step(
        &mut engine,
        Operation::Schematic(SchematicCommand::UnplaceUnit { occurrence: occ_b }),
    )?;
    assert_eq!(states(&engine, q1)?[1], (B, UnitState::Unplaced));
    assert_eq!(pin_position(engine.project(), ep(q1, 4)), None);
    // Unplacing hides no electrical obligation: B's disconnected terminals
    // are still reported.
    let report = inspect(&engine, q1)?;
    for terminal in [4, 5] {
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code == "instance.terminal_unaccounted"
                    && f.objects
                        == vec![crate::ObjectRef::Terminal {
                            endpoint: ep(q1, terminal)
                        }]),
            "terminal {terminal} stays unaccounted"
        );
    }
    assert!(engine.project().placement_of(q1).is_some());
    assert_eq!(
        layout::pads(engine.project())
            .iter()
            .filter(|p| p.instance == q1)
            .count(),
        6,
        "physical pads remain"
    );
    let Outcome::UnitPlaced { occurrence: occ_b } = apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::PlaceUnit {
            sheet: None,
            instance: q1,
            unit: B,
            position: pt([10.0, 10.0]),
        }),
    )?
    else {
        return Err(Error::Invalid("no unit outcome".into()));
    };

    // A wired unit cannot be unplaced directly.
    let before = engine.document().clone();
    let revision = engine.revision();
    let can_redo = engine.can_redo();
    let refused = engine.apply(
        revision,
        Operation::Schematic(SchematicCommand::UnplaceUnit { occurrence: occ_a }),
    );
    assert_eq!(refused.err().map(|e| e.code()), Some("conflict"));
    assert_eq!(engine.document(), &before);
    assert_eq!(engine.revision(), revision);
    assert_eq!(engine.can_redo(), can_redo);
    let a_wires = vec![
        wire_between(&engine, ep(q1, 1), ep(l1, 1))?,
        wire_between(&engine, ep(q1, 2), ep(l1, 2))?,
    ];
    let change = Change::Schematic {
        command: SchematicCommand::UnplaceUnit { occurrence: occ_a },
    };
    let plan = engine.preview_update(change.clone());
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let mut removed = plan.removed_wires.clone();
    removed.sort_unstable();
    let mut expected = a_wires.clone();
    expected.sort_unstable();
    assert_eq!(removed, expected);
    assert!(plan.conflicting_tracks.is_empty(), "{:?}", plan.conflicts);
    for terminal in [1, 2] {
        let name = engine
            .project()
            .net_of(ep(q1, terminal))
            .map(|n| n.name.clone())
            .unwrap_or_default();
        assert!(
            plan.changes
                .iter()
                .any(|c| c.starts_with(&format!("Net {name}:"))),
            "{name} listed"
        );
    }
    assert_eq!(engine.document(), &before, "preview is read-only");
    engine.apply_update(plan.revision, plan.change, true)?;
    assert_eq!(engine.revision(), revision + 1);
    assert!(engine.project().occurrence(occ_a).is_none());
    assert_eq!(
        engine
            .project()
            .tracks
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        [track]
    );
    let after = engine.document().clone();
    engine.undo()?;
    assert_eq!(engine.document(), &before);
    engine.redo()?;
    assert_eq!(engine.document(), &after);
    engine.undo()?;

    // Whole-component removal is previewed, never direct.
    let refused = engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::Remove { instance: q1 }),
    );
    assert_eq!(refused.err().map(|e| e.code()), Some("conflict"));
    let plan = engine.preview_update(Change::Instance {
        edit: InstanceChange::Remove { instance: q1 },
    });
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    for occurrence in [occ_a, occ_b] {
        assert!(
            plan.changes
                .iter()
                .any(|c| c.starts_with(&format!("Remove symbol occurrence {occurrence} ")))
        );
    }
    let mut removed = plan.removed_wires.clone();
    removed.sort_unstable();
    assert_eq!(removed, expected);
    assert_eq!(plan.removed_placements, vec![q1]);
    assert!(plan.conflicting_tracks.is_empty(), "{:?}", plan.conflicts);
    // Every net Q1 leaves is listed as an electrical change.
    let mut listed = 0;
    for terminal in 1..=6 {
        if let Some(net) = engine.project().net_of(ep(q1, terminal)) {
            listed += 1;
            let name = net.name.clone();
            assert!(
                plan.changes
                    .iter()
                    .any(|c| c.starts_with(&format!("Net {name}:"))),
                "removal lists affected net {name}"
            );
        }
    }
    assert!(listed >= 2, "Q1A's wired terminals are on nets");
    assert_eq!(engine.document(), &before, "cancel is not applying");

    // Save and reopen keeps every ID, pin, unit state and marker.
    let reopened = Engine::from_file(format::decode(&format::encode(&engine.to_file())?)?)?;
    assert_eq!(reopened.project(), engine.project());
    assert_eq!(states(&reopened, q1)?, states(&engine, q1)?);
    Ok(())
}
