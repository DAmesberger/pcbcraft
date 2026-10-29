use crate::{
    Endpoint, Engine, Error, Layer, Operation, PartRef, Request, Result,
    layout::{LayoutCommand, RouteRequest},
    parts::{self, PartCommand},
    schematic::{self, SchematicCommand},
};
use serde_json::json;

fn dispatch(engine: &mut Engine, operation: Operation) -> Result<()> {
    engine.dispatch(serde_json::to_value(Request::Apply {
        expected_revision: engine.revision(),
        operation: Box::new(operation),
    })?)?;
    Ok(())
}

#[test]
fn public_command_workflow_authors_connects_places_routes_and_undoes() -> Result<()> {
    let mut engine = Engine::default();
    dispatch(
        &mut engine,
        Operation::Part(PartCommand::Publish {
            draft: parts::passive_template("10k resistor"),
        }),
    )?;
    for (reference, position) in [("R1", [-10.0, 0.0]), ("R2", [10.0, 0.0])] {
        dispatch(
            &mut engine,
            Operation::Schematic(SchematicCommand::Place {
                part: PartRef { id: 1, revision: 1 },
                reference: reference.into(),
                value: "10k".into(),
                position,
            }),
        )?;
    }
    dispatch(
        &mut engine,
        Operation::Schematic(SchematicCommand::Connect {
            from: Endpoint {
                instance: 2,
                terminal: 2,
            },
            to: Endpoint {
                instance: 3,
                terminal: 1,
            },
        }),
    )?;
    dispatch(&mut engine, Operation::Layout(LayoutCommand::Sync))?;
    assert_eq!(engine.project().placements.len(), 2);
    let nets = schematic::nets(engine.project());
    assert_eq!(nets.len(), 3);
    assert_eq!(nets[1].id, "net:2:2");
    assert_eq!(
        nets[1].terminals,
        [
            Endpoint {
                instance: 2,
                terminal: 2
            },
            Endpoint {
                instance: 3,
                terminal: 1
            }
        ]
    );
    let before_route = engine.project().clone();
    dispatch(
        &mut engine,
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net: "net:2:2".into(),
                start: [9.2, 8.0],
                end: [14.8, 8.0],
                width: 0.3,
                layer: Layer::Top,
            },
        }),
    )?;
    assert_eq!(engine.project().tracks.len(), 1);
    assert_eq!(engine.project().tracks[0].net, "net:2:2");
    let routed = engine.project().clone();
    engine.dispatch(json!({"command":"project.undo","params":{}}))?;
    assert_eq!(engine.project(), &before_route);
    engine.dispatch(json!({"command":"project.redo","params":{}}))?;
    assert_eq!(engine.project(), &routed);
    Ok(())
}

#[test]
fn invalid_and_stale_commands_preserve_document_revision_and_redo() -> Result<()> {
    let mut engine = Engine::demo()?;
    dispatch(
        &mut engine,
        Operation::Schematic(SchematicCommand::Edit {
            instance: 2,
            reference: "R10".into(),
            value: "22k".into(),
        }),
    )?;
    engine.undo()?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let operation = Operation::Schematic(SchematicCommand::Move {
        instance: 999,
        position: [1.0, 2.0],
    });
    assert!(matches!(
        engine.apply(0, operation.clone()),
        Err(Error::Stale { .. })
    ));
    assert!(engine.apply(revision, operation).is_err());
    assert!(engine.dispatch(json!({"command":"project.apply","params":{"expected_revision":revision,"operation":{"editor":"Schematic","operation":{"command":"schematic.move","params":{"instance":2,"position":[1,2],"unexpected":true}}}}})).is_err());
    assert!(
        engine
            .dispatch(json!({"command":"project.unknown","params":{}}))
            .is_err()
    );
    assert_eq!(engine.project(), &before);
    assert_eq!(engine.revision(), revision);
    assert!(engine.can_redo());
    engine.redo()?;
    assert_eq!(engine.project().instances[0].reference, "R10");
    Ok(())
}

#[test]
fn saved_project_reopens_offline_with_exact_revisions_and_no_prior_history() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("offline.pcbcraft");
    let mut engine = Engine::demo()?;
    let mut revised = engine.project().parts[0].clone();
    revised.name = "Changed library definition".into();
    revised.footprint.pads[0].position = [-2.0, 0.0];
    dispatch(
        &mut engine,
        Operation::Part(PartCommand::Publish { draft: revised }),
    )?;
    let before = engine.project().clone();
    let revision = engine.revision();
    engine.dispatch(json!({"command":"project.save","params":{"path":path}}))?;
    assert_eq!(engine.revision(), revision, "saving is an external effect");
    drop(engine);
    let mut reopened = Engine::demo()?;
    reopened.dispatch(json!({"command":"project.open","params":{"path":path}}))?;
    assert_eq!(reopened.project(), &before);
    assert_eq!(reopened.project().parts.len(), 2);
    assert!(
        reopened
            .project()
            .instances
            .iter()
            .all(|i| i.part.revision == 1)
    );
    assert!(!reopened.can_undo());
    assert!(!reopened.can_redo());
    assert_eq!(
        crate::layout::pads(reopened.project())[0].position,
        [6.8, 8.0]
    );
    Ok(())
}

#[test]
fn invalid_files_leave_live_project_and_history_unchanged() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("bad.pcbcraft");
    let mut engine = Engine::demo()?;
    dispatch(
        &mut engine,
        Operation::Schematic(SchematicCommand::Edit {
            instance: 2,
            reference: "R10".into(),
            value: "22k".into(),
        }),
    )?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let mut newer = serde_json::to_value(&before)?;
    newer["version"] = json!(999);
    let mut unknown_feature = serde_json::to_value(&before)?;
    unknown_feature["required_features"] = json!(["future-copper"]);
    let mut missing_part = serde_json::to_value(&before)?;
    missing_part["instances"][0]["part"]["revision"] = json!(999);
    for bytes in [
        b"not json".to_vec(),
        serde_json::to_vec(&newer)?,
        serde_json::to_vec(&unknown_feature)?,
        serde_json::to_vec(&missing_part)?,
    ] {
        std::fs::write(&path, bytes)?;
        assert!(
            engine
                .dispatch(json!({"command":"project.open","params":{"path":path}}))
                .is_err()
        );
        assert_eq!(engine.project(), &before);
        assert_eq!(engine.revision(), revision);
        assert!(engine.can_undo());
    }
    let large = std::fs::File::create(&path)?;
    large.set_len(16 * 1024 * 1024 + 1)?;
    assert!(engine.open(&path).is_err());
    assert_eq!(engine.project(), &before);
    assert_eq!(engine.revision(), revision);
    engine.undo()?;
    assert_eq!(engine.project().instances[0].reference, "R1");
    Ok(())
}

#[test]
fn failed_save_preserves_existing_target_and_cleans_temporary_file() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("occupied.pcbcraft");
    std::fs::create_dir(&target)?;
    std::fs::write(target.join("sentinel"), b"preserve")?;
    let engine = Engine::demo()?;
    let before = engine.project().clone();
    let revision = engine.revision();
    assert!(engine.save(&target).is_err());
    assert_eq!(std::fs::read(target.join("sentinel"))?, b"preserve");
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
    assert_eq!(engine.project(), &before);
    assert_eq!(engine.revision(), revision);
    Ok(())
}
