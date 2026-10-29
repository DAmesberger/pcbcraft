use crate::{
    Endpoint, Engine, Error, Id, Layer, Operation, PartRef, Project, Result,
    layout::{LayoutCommand, RouteRequest},
    parts::{self, PartCommand},
    schematic::{self, SchematicCommand},
};
use serde_json::json;
use std::sync::Arc;

fn apply(engine: &mut Engine, operation: Operation) -> Result<()> {
    engine.apply(engine.revision(), operation)
}

fn endpoint(instance: Id, terminal: Id) -> Endpoint {
    Endpoint { instance, terminal }
}

fn net_of(project: &Project, terminal: Endpoint) -> Result<Id> {
    project
        .net_of(terminal)
        .map(|net| net.id)
        .ok_or_else(|| Error::NotFound("terminal has no net".into()))
}

fn place(engine: &mut Engine, reference: &str, position: [f64; 2]) -> Result<()> {
    apply(
        engine,
        Operation::Schematic(SchematicCommand::Place {
            part: PartRef { id: 1, revision: 1 },
            reference: reference.into(),
            value: "10k".into(),
            position,
        }),
    )
}

fn connect(engine: &mut Engine, from: Endpoint, to: Endpoint) -> Result<()> {
    apply(
        engine,
        Operation::Schematic(SchematicCommand::Connect { from, to }),
    )
}

#[test]
fn public_command_workflow_authors_connects_places_routes_and_undoes() -> Result<()> {
    let mut engine = Engine::default();
    apply(
        &mut engine,
        Operation::Part(PartCommand::Publish {
            draft: Some(parts::passive_template("10k resistor")),
        }),
    )?;
    place(&mut engine, "R1", [-10.0, 0.0])?;
    place(&mut engine, "R2", [10.0, 0.0])?;
    connect(&mut engine, endpoint(2, 2), endpoint(3, 1))?;
    apply(&mut engine, Operation::Layout(LayoutCommand::Sync))?;
    assert_eq!(engine.project().placements.len(), 2);
    let nets = schematic::nets(engine.project());
    assert_eq!(nets.len(), 3);
    let shared = net_of(engine.project(), endpoint(2, 2))?;
    let shared_net = engine
        .project()
        .net(shared)
        .ok_or_else(|| Error::NotFound("shared net".into()))?;
    assert_eq!(shared_net.terminals, [endpoint(2, 2), endpoint(3, 1)]);
    assert_eq!(shared_net.name, format!("N{shared}"));
    let before_route = engine.project().clone();
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net: shared,
                start: [9.2, 8.0],
                end: [14.8, 8.0],
                width: 0.3,
                layer: Layer::Top,
            },
        }),
    )?;
    assert_eq!(engine.project().tracks.len(), 1);
    assert_eq!(engine.project().tracks[0].net, shared);
    let routed = engine.project().clone();
    engine.undo()?;
    assert_eq!(engine.project(), &before_route);
    engine.redo()?;
    assert_eq!(engine.project(), &routed);
    Ok(())
}

#[test]
fn invalid_and_stale_commands_preserve_document_revision_and_redo() -> Result<()> {
    let mut engine = Engine::demo()?;
    apply(
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
    assert!(matches!(
        engine.apply(revision, operation),
        Err(Error::NotFound(_))
    ));
    let unknown_field = json!({"editor":"Schematic","operation":{"command":"schematic.move","params":{"instance":2,"position":[1,2],"unexpected":true}}});
    assert!(serde_json::from_value::<Operation>(unknown_field).is_err());
    assert_eq!(engine.project(), &before);
    assert_eq!(engine.revision(), revision);
    assert!(engine.can_redo());
    engine.redo()?;
    assert_eq!(engine.project().instances[0].reference, "R10");
    Ok(())
}

#[test]
fn error_codes_classify_failures() -> Result<()> {
    let mut engine = Engine::demo()?;
    let revision = engine.revision();
    let duplicate = engine.apply(
        revision,
        Operation::Schematic(SchematicCommand::Connect {
            from: endpoint(2, 2),
            to: endpoint(3, 1),
        }),
    );
    assert_eq!(duplicate.err().map(|e| e.code()), Some("conflict"));
    let stale = engine.apply(revision + 5, Operation::Layout(LayoutCommand::Sync));
    let diagnostic = stale.err().map(|e| e.diagnostic());
    assert_eq!(
        diagnostic.as_ref().map(|d| d.code.as_str()),
        Some("stale_revision")
    );
    assert_eq!(diagnostic.and_then(|d| d.actual_revision), Some(revision));
    let empty = Engine::default().undo();
    assert_eq!(empty.err().map(|e| e.code()), Some("history_empty"));
    Ok(())
}

#[test]
fn net_identity_survives_unrelated_edits_renames_and_history() -> Result<()> {
    let mut engine = Engine::demo()?;
    let shared = net_of(engine.project(), endpoint(2, 2))?;
    // Removing a lower-numbered terminal's instance once renamed derived nets;
    // persistent IDs stay with the routed group regardless of numbering.
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::RenameNet {
            net: shared,
            name: "VOUT".into(),
        }),
    )?;
    place(&mut engine, "R3", [30.0, 0.0])?;
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Move {
            instance: 2,
            position: [-40.0, 5.0],
        }),
    )?;
    assert_eq!(net_of(engine.project(), endpoint(2, 2))?, shared);
    assert_eq!(engine.project().net_name(shared), "VOUT");
    let rename_clash = engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::RenameNet {
            net: net_of(engine.project(), endpoint(2, 1))?,
            name: "vout".into(),
        }),
    );
    assert_eq!(rename_clash.err().map(|e| e.code()), Some("conflict"));
    for _ in 0..3 {
        engine.undo()?;
    }
    assert_eq!(engine.project().net_name(shared), format!("N{shared}"));
    engine.redo()?;
    assert_eq!(engine.project().net_name(shared), "VOUT");
    Ok(())
}

#[test]
fn connecting_merges_into_the_older_net_and_disconnecting_splits_off_new_ids() -> Result<()> {
    let mut engine = Engine::demo()?;
    place(&mut engine, "R3", [30.0, 0.0])?;
    let r3 = engine
        .project()
        .instances
        .last()
        .map(|i| i.id)
        .ok_or_else(|| Error::NotFound("R3".into()))?;
    let shared = net_of(engine.project(), endpoint(2, 2))?;
    let lone = net_of(engine.project(), endpoint(r3, 1))?;
    assert!(lone > shared);
    connect(&mut engine, endpoint(3, 1), endpoint(r3, 1))?;
    assert_eq!(net_of(engine.project(), endpoint(r3, 1))?, shared);
    assert!(engine.project().net(lone).is_none());
    let wire = engine
        .project()
        .wires
        .iter()
        .find(|w| w.to == endpoint(r3, 1))
        .map(|w| w.id)
        .ok_or_else(|| Error::NotFound("wire".into()))?;
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Disconnect { wire }),
    )?;
    // The larger group keeps the ID; the split-off terminal gets a fresh one,
    // never a recycled ID.
    assert_eq!(net_of(engine.project(), endpoint(2, 2))?, shared);
    let split = net_of(engine.project(), endpoint(r3, 1))?;
    assert!(split != shared && split != lone);
    Ok(())
}

#[test]
fn drafts_are_engine_state_with_shared_undo_and_no_project_copy() -> Result<()> {
    let mut engine = Engine::demo()?;
    let project = Arc::clone(engine.project_arc());
    let original_draft = engine.draft().cloned();
    let mut draft = parts::passive_template("Capacitor");
    draft.binding.mappings.clear();
    apply(
        &mut engine,
        Operation::Part(PartCommand::SetDraft {
            draft: draft.clone(),
        }),
    )?;
    assert!(
        Arc::ptr_eq(&project, engine.project_arc()),
        "a draft edit shares the unchanged project"
    );
    assert_eq!(engine.draft(), Some(&draft));
    let revision = engine.revision();
    apply(
        &mut engine,
        Operation::Part(PartCommand::SetDraft {
            draft: draft.clone(),
        }),
    )?;
    assert_eq!(engine.revision(), revision, "no-op edits add no history");
    assert!(
        engine
            .apply(
                engine.revision(),
                Operation::Part(PartCommand::Publish { draft: None })
            )
            .is_err(),
        "an unfinished draft cannot be published"
    );
    draft = parts::passive_template("Capacitor");
    apply(
        &mut engine,
        Operation::Part(PartCommand::SetDraft { draft }),
    )?;
    apply(
        &mut engine,
        Operation::Part(PartCommand::Publish { draft: None }),
    )?;
    let published = engine
        .draft()
        .map(|d| PartRef {
            id: d.id,
            revision: d.revision,
        })
        .ok_or_else(|| Error::NotFound("draft".into()))?;
    assert!(engine.project().part(published).is_some());
    engine.undo()?;
    assert!(engine.project().part(published).is_none());
    engine.undo()?;
    engine.undo()?;
    assert_eq!(engine.draft(), original_draft.as_ref());
    assert!(Arc::ptr_eq(&project, engine.project_arc()));
    Ok(())
}

#[test]
fn undo_history_is_bounded() -> Result<()> {
    let mut engine = Engine::demo()?;
    for i in 0..(crate::MAX_UNDO + 20) {
        apply(
            &mut engine,
            Operation::Schematic(SchematicCommand::Move {
                instance: 2,
                position: [i as f64, 0.0],
            }),
        )?;
    }
    let mut undone = 0;
    while engine.undo().is_ok() {
        undone += 1;
    }
    assert_eq!(undone, crate::MAX_UNDO);
    Ok(())
}
