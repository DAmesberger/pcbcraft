//! Staged completion and atomic failure of the instance commands.
use super::support::*;
use crate::layout::LayoutCommand;
use crate::schematic::SchematicCommand;
use crate::{
    Engine, InstanceCommand, ObjectRef, Operation, PhysicalChoice, Result, Stage, TerminalId,
    UnitId, instances::NewInstance,
};
use serde_json::json;

#[test]
fn a_component_without_implementations_is_placeable_but_not_board_placeable() -> Result<()> {
    let mut engine = Engine::default();
    let f = component(
        &mut engine,
        &Spec {
            name: "Schematic only",
            terminals: vec![(1, "A", passive())],
            units: vec![(1, "A", vec![(1, [0.0, 0.0])])],
            package: vec![(1, "1")],
            mapping: vec![(1, 1)],
            physical: false,
        },
    )?;
    let (instance, _) = place(&mut engine, &f, "J1", 1, [0.0, 0.0])?;
    let revision = engine.revision();
    apply(&mut engine, Operation::Layout(LayoutCommand::Sync))?;
    assert!(engine.project().placements.is_empty());
    assert_eq!(engine.revision(), revision + 1);
    let report = inspect(&engine, instance)?;
    assert!(!report.board_placeable);
    let board: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.blocks_stage(Stage::BoardPlaceable))
        .map(|f| (f.code.as_str(), f.objects.clone()))
        .collect();
    assert_eq!(
        board,
        vec![(
            "instance.no_physical_choice",
            vec![ObjectRef::Instance { id: instance }]
        )]
    );
    assert_eq!(report.physical, None);
    assert_eq!(
        report.purchasing.state,
        crate::completeness::PurchasingState::NoPart
    );
    let place = engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Place {
            instance,
            position: [10.0, 10.0],
            rotation_deg: 0.0,
            layer: crate::Layer::Top,
        }),
    );
    assert_eq!(place.err().map(|e| e.code()), Some("conflict"));
    Ok(())
}

#[test]
fn new_instance_commands_fail_atomically_on_stale_malformed_and_unknown_input() -> Result<()> {
    let mut engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let r1 = by_designator(&engine, "R1")?;
    let occurrence = engine
        .project()
        .occurrences_of(r1)
        .next()
        .map(|o| o.id)
        .unwrap_or(0);
    prime_history(&mut engine)?;
    let kept = Snapshot::of(&engine);
    let revision = kept.revision();
    let operations = vec![
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: f.r1(),
            designator: Some("R1".into()),
            properties: vec![],
            representation: f.symbol,
            unit: UnitId(1),
            position: [0.0, 0.0],
            physical: None,
        })),
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: f.r1(),
            designator: Some("R5".into()),
            properties: vec![],
            representation: f.package,
            unit: UnitId(1),
            position: [0.0, 0.0],
            physical: None,
        })),
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: f.r1(),
            designator: Some("R5".into()),
            properties: vec![],
            representation: f.symbol,
            unit: UnitId(9),
            position: [0.0, 0.0],
            physical: None,
        })),
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: f.r1(),
            designator: Some("R5".into()),
            properties: vec![],
            representation: f.symbol,
            unit: UnitId(1),
            position: [0.0, 0.0],
            physical: Some(PhysicalChoice {
                binding: f.binding,
                footprint: f.package,
            }),
        })),
        Operation::Instance(InstanceCommand::Remove { instance: 999 }),
        Operation::Instance(InstanceCommand::Remove { instance: r1 }),
        Operation::Instance(InstanceCommand::SetDesignator {
            instance: r1,
            designator: "R 1".into(),
        }),
        Operation::Instance(InstanceCommand::SetDesignator {
            instance: r1,
            designator: "r2".into(),
        }),
        Operation::Instance(InstanceCommand::SetProperties {
            instance: 999,
            edits: vec![],
        }),
        Operation::Instance(InstanceCommand::SetUnitUnused {
            instance: r1,
            unit: UnitId(1),
            unused: true,
        }),
        Operation::Instance(InstanceCommand::SetUnitUnused {
            instance: r1,
            unit: UnitId(7),
            unused: true,
        }),
        Operation::Instance(InstanceCommand::SetNoConnect {
            instance: r1,
            terminals: vec![TerminalId(9)],
            no_connect: true,
        }),
        Operation::Instance(InstanceCommand::SetNoConnect {
            instance: r1,
            terminals: vec![],
            no_connect: true,
        }),
        Operation::Instance(InstanceCommand::SetPhysical {
            instance: r1,
            choice: None,
        }),
        Operation::Schematic(SchematicCommand::PlaceUnit {
            sheet: None,
            instance: r1,
            unit: UnitId(1),
            position: [0.0, 0.0],
        }),
        Operation::Schematic(SchematicCommand::UnplaceUnit { occurrence }),
        Operation::Schematic(SchematicCommand::UnplaceUnit { occurrence: 999 }),
    ];
    for operation in operations {
        assert!(engine.apply(revision + 1, operation.clone()).is_err());
        let result = engine.apply(revision, operation.clone());
        assert!(result.is_err(), "{operation:?} was accepted");
        kept.assert_kept(&engine, &format!("{operation:?}"));
    }
    for malformed in [
        json!({"editor":"Instance","operation":{"command":"component.place","params":{"component":{"id":1,"revision":1}}}}),
        json!({"editor":"Instance","operation":{"command":"instance.set_unit_unused","params":{"instance":1,"unit":1,"unused":true,"extra":0}}}),
        json!({"editor":"Schematic","operation":{"command":"schematic.place","params":{}}}),
    ] {
        assert!(serde_json::from_value::<Operation>(malformed).is_err());
    }
    Ok(())
}

/// A wire on a no-connect terminal is accepted and reported: the terminal
/// is no longer unconnected, it is a no-connect pin that is connected.
#[test]
fn a_wire_on_a_no_connect_terminal_is_a_completeness_error() -> Result<()> {
    let mut engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let (r3, _) = place(&mut engine, &f, "R3", 1, [30.0, 0.0])?;
    let r2 = by_designator(&engine, "R2")?;
    let codes = |engine: &Engine| -> Result<Vec<String>> {
        Ok(inspect(engine, r3)?
            .findings
            .into_iter()
            .filter(|f| {
                f.objects
                    == [crate::ObjectRef::Terminal {
                        endpoint: ep(r3, 1),
                    }]
            })
            .map(|f| f.code)
            .collect())
    };
    assert_eq!(codes(&engine)?, ["instance.terminal_unaccounted"]);
    apply(
        &mut engine,
        Operation::Instance(InstanceCommand::SetNoConnect {
            instance: r3,
            terminals: vec![TerminalId(1)],
            no_connect: true,
        }),
    )?;
    // Intentionally unused: no finding at all.
    assert!(codes(&engine)?.is_empty());
    connect(&mut engine, ep(r2, 2), ep(r3, 1))?;
    assert_eq!(codes(&engine)?, ["instance.no_connect_connected"]);
    // The marker and the wire survive save and load.
    let bytes = crate::format::encode(&engine.to_file())?;
    let reopened = Engine::from_file(crate::format::decode(&bytes)?)?;
    assert_eq!(reopened.document(), engine.document());
    assert_eq!(codes(&reopened)?, ["instance.no_connect_connected"]);
    let issues = crate::issues::report(
        engine.project(),
        &Default::default(),
        &crate::issues::IssueScope::All,
    )?;
    let row = issues
        .rows
        .iter()
        .find(|r| r.group.code == "instance.no_connect_connected")
        .ok_or_else(|| crate::Error::Invalid("no issue row".into()))?;
    assert_eq!(row.severity, crate::issues::Severity::Error);
    assert!(
        crate::issues::style::check(&row.title).is_empty(),
        "{}",
        row.title
    );
    Ok(())
}

#[test]
fn schemas_name_the_new_operations_queries_and_drafts() -> Result<()> {
    let schemas = serde_json::to_string(&crate::schemas())?;
    for name in [
        "library.new_draft",
        "library.publish",
        "library.generate_pads",
        "component.place",
        "component.remove",
        "instance.set_unit_unused",
        "instance.set_no_connect",
        "instance.set_physical",
        "instance.set_properties",
        "properties_set",
        "quantity_kind",
        "FieldResolution",
        "symbol.place_unit",
        "symbol.unplace_unit",
        "component.inspect",
        "component.check",
        "drafts",
        "ComponentRevision",
    ] {
        assert!(schemas.contains(name), "{name}");
    }
    for removed in [
        "part.publish",
        "schematic.place",
        "PartRevision",
        "authoring_draft",
        "instance.set_value",
        "UnitDefinition",
    ] {
        assert!(!schemas.contains(removed), "{removed}");
    }
    Ok(())
}
