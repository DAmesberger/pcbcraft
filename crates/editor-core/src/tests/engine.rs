//! Ported engine, schematic and net-identity behaviour on the instance model.
use super::support::*;
use crate::layout::{LayoutCommand, RouteRequest};
use crate::schematic::{self, SchematicCommand};
use crate::{Engine, Error, InstanceCommand, Layer, Operation, Result, Track, erc, format};
use serde_json::json;
use std::sync::Arc;

fn two_terminal(engine: &mut Engine) -> Result<Fixture> {
    component(
        engine,
        &Spec {
            name: "Two terminal",
            terminals: vec![(1, "A", input()), (2, "B", output())],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: true,
        },
    )
}

fn designator_edit(instance: u64, designator: &str) -> Operation {
    Operation::Instance(InstanceCommand::SetDesignator {
        instance,
        designator: designator.into(),
    })
}

#[test]
fn public_command_workflow_authors_connects_places_routes_and_undoes() -> Result<()> {
    let mut engine = Engine::demo()?;
    let (r1, r2) = (by_designator(&engine, "R1")?, by_designator(&engine, "R2")?);
    assert_eq!(engine.project().placements.len(), 2);
    assert_eq!(schematic::nets(engine.project()).len(), 3);
    let shared = net_of(&engine, ep(r1, 2))?;
    let shared_net = engine
        .project()
        .net(shared)
        .ok_or_else(|| Error::NotFound("shared net".into()))?;
    assert_eq!(shared_net.terminals, [ep(r1, 2), ep(r2, 1)]);
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
                layer: Layer::Top.into(),
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
    let r1 = by_designator(&engine, "R1")?;
    apply(&mut engine, designator_edit(r1, "R10"))?;
    engine.undo()?;
    let before = engine.document().clone();
    let revision = engine.revision();
    let operation = Operation::Schematic(SchematicCommand::MoveUnit {
        occurrence: 999,
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
    let unknown_field = json!({"editor":"Schematic","operation":{"command":"symbol.move_unit","params":{"occurrence":2,"position":[1,2],"unexpected":true}}});
    assert!(serde_json::from_value::<Operation>(unknown_field).is_err());
    let removed = json!({"editor":"Part","operation":{"command":"part.publish","params":{}}});
    assert!(serde_json::from_value::<Operation>(removed).is_err());
    assert_eq!(engine.document(), &before);
    assert_eq!(engine.revision(), revision);
    assert!(engine.can_redo());
    engine.redo()?;
    assert_eq!(engine.project().instances[0].designator, "R10");
    Ok(())
}

#[test]
fn error_codes_classify_failures() -> Result<()> {
    let mut engine = Engine::demo()?;
    let (r1, r2) = (by_designator(&engine, "R1")?, by_designator(&engine, "R2")?);
    let revision = engine.revision();
    let duplicate = engine.apply(
        revision,
        Operation::Schematic(SchematicCommand::Connect {
            from: ep(r1, 2).into(),
            to: ep(r2, 1).into(),
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
    let fixture = demo_fixture(&engine)?;
    let r1 = by_designator(&engine, "R1")?;
    let shared = net_of(&engine, ep(r1, 2))?;
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::RenameNet {
            net: shared,
            name: "VOUT".into(),
        }),
    )?;
    place(&mut engine, &fixture, "R3", 1, [30.0, 0.0])?;
    let occurrence = engine
        .project()
        .occurrences_of(r1)
        .next()
        .map(|o| o.id)
        .ok_or_else(|| Error::NotFound("occurrence".into()))?;
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::MoveUnit {
            occurrence,
            position: [-40.0, 5.0],
        }),
    )?;
    assert_eq!(net_of(&engine, ep(r1, 2))?, shared);
    assert_eq!(engine.project().net_name(shared), "VOUT");
    let rename_clash = engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::RenameNet {
            net: net_of(&engine, ep(r1, 1))?,
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
    let fixture = demo_fixture(&engine)?;
    let (r1, r2) = (by_designator(&engine, "R1")?, by_designator(&engine, "R2")?);
    let (r3, _) = place(&mut engine, &fixture, "R3", 1, [30.0, 0.0])?;
    let shared = net_of(&engine, ep(r1, 2))?;
    let lone = net_of(&engine, ep(r3, 1))?;
    assert!(lone > shared);
    connect(&mut engine, ep(r2, 1), ep(r3, 1))?;
    assert_eq!(net_of(&engine, ep(r3, 1))?, shared);
    assert!(engine.project().net(lone).is_none());
    // The new wire may bend around R2's other pin through junction points;
    // removing its leg at R3.1 removes the whole run.
    let wire = engine
        .project()
        .wires_at(ep(r3, 1))
        .next()
        .map(|w| w.id)
        .ok_or_else(|| Error::NotFound("wire".into()))?;
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Disconnect { wire }),
    )?;
    // The larger group keeps the ID; the split-off terminal gets a fresh one.
    assert_eq!(net_of(&engine, ep(r1, 2))?, shared);
    let split = net_of(&engine, ep(r3, 1))?;
    assert!(split != shared && split != lone);
    Ok(())
}

#[test]
fn drafts_are_engine_state_with_shared_undo_and_no_project_copy() -> Result<()> {
    let mut engine = Engine::demo()?;
    let fixture = demo_fixture(&engine)?;
    let project = Arc::clone(engine.project_arc());
    let original = engine.drafts().clone();
    with(&mut engine, fixture.binding, binding_body, |b| {
        b.mappings.clear()
    })?;
    assert!(
        Arc::ptr_eq(&project, engine.project_arc()),
        "a draft edit shares the unchanged project"
    );
    let unfinished = engine.drafts().clone();
    let revision = engine.revision();
    with(&mut engine, fixture.binding, binding_body, |b| {
        b.mappings.clear()
    })?;
    assert_eq!(engine.revision(), revision, "no-op edits add no history");
    let refused = publish(&mut engine, fixture.all());
    assert_eq!(refused.err().map(|e| e.code()), Some("conflict"));
    assert_eq!(
        engine.revision(),
        revision,
        "an unfinished draft cannot be published"
    );
    assert_eq!(engine.drafts(), &unfinished);
    engine.undo()?;
    assert_eq!(engine.drafts(), &original);
    retarget(&mut engine, &fixture)?;
    let published = publish(&mut engine, fixture.all())?;
    assert!(published.iter().all(|r| r.revision.0 == 2));
    assert_eq!(engine.project().library.components.len(), 2);
    engine.undo()?;
    assert_eq!(engine.project().library.components.len(), 1);
    engine.undo()?;
    assert_eq!(engine.drafts(), &original);
    assert!(Arc::ptr_eq(&project, engine.project_arc()));
    Ok(())
}

#[test]
fn undo_history_is_bounded() -> Result<()> {
    let mut engine = Engine::demo()?;
    let occurrence = engine.project().occurrences[0].id;
    for i in 0..(crate::MAX_UNDO + 20) {
        apply(
            &mut engine,
            Operation::Schematic(SchematicCommand::MoveUnit {
                occurrence,
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

/// A step that finishes the one before joins it: one undo takes back both.
/// Joining is refused, with history unchanged, unless the latest step was
/// applied at the given revision and nothing was undone or redone since.
#[test]
fn a_joined_step_undoes_with_the_one_before() -> Result<()> {
    let mut engine = Engine::demo()?;
    let occurrence = engine.project().occurrences[0].id;
    let moved = |x: f64| {
        Operation::Schematic(SchematicCommand::MoveUnit {
            occurrence,
            position: [x, 0.0],
        })
    };
    apply(&mut engine, moved(1.0))?;
    let before = engine.project().clone();
    apply(&mut engine, moved(2.0))?;
    let at = engine.revision();
    apply(&mut engine, moved(3.0))?;
    assert!(engine.join_last_step(at + 1).is_err(), "another revision");
    engine.join_last_step(at)?;
    assert!(engine.join_last_step(at).is_err(), "only once");
    engine.undo()?;
    assert_eq!(*engine.project(), before, "both steps went");
    engine.redo()?;
    let at = engine.revision();
    assert!(engine.join_last_step(at).is_err(), "not after a redo");
    engine.undo()?;
    assert_eq!(*engine.project(), before, "the history is as it was");
    Ok(())
}

#[test]
fn topology_uses_ids_and_survives_moving_and_reannotation() -> Result<()> {
    let mut engine = Engine::default();
    let fixture = two_terminal(&mut engine)?;
    let (a, occurrence) = place(&mut engine, &fixture, "U1", 1, [0.0, 0.0])?;
    let (b, _) = place(&mut engine, &fixture, "U2", 1, [0.0, 0.0])?;
    assert_eq!(
        schematic::nets(engine.project()).len(),
        4,
        "coincident symbols do not short terminals"
    );
    connect(&mut engine, ep(a, 2), ep(b, 1))?;
    let before = schematic::nets(engine.project()).to_vec();
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::MoveUnit {
            occurrence,
            position: [20.0, 10.0],
        }),
    )?;
    apply(&mut engine, designator_edit(a, "AMP7"))?;
    assert_eq!(schematic::nets(engine.project()), before);
    assert_eq!(engine.project().instances[0].component, fixture.r1());
    assert_eq!(
        schematic::pin_position(engine.project(), ep(a, 2)),
        Some([25.0, 10.0])
    );
    Ok(())
}

#[test]
fn malformed_references_and_routed_connectivity_changes_leave_state_unchanged() -> Result<()> {
    let mut engine = Engine::default();
    let fixture = two_terminal(&mut engine)?;
    let (a, occurrence) = place(&mut engine, &fixture, "U1", 1, [0.0, 0.0])?;
    let (b, _) = place(&mut engine, &fixture, "U2", 1, [20.0, 0.0])?;
    let before = engine.document().clone();
    let revision = engine.revision();
    for operation in [
        Operation::Schematic(SchematicCommand::Connect {
            from: ep(a, 2).into(),
            to: ep(999, 1).into(),
        }),
        Operation::Schematic(SchematicCommand::MoveUnit {
            occurrence,
            position: [f64::NAN, 0.0],
        }),
    ] {
        assert!(engine.apply(revision, operation).is_err());
        assert_eq!(engine.document(), &before);
        assert_eq!(engine.revision(), revision);
    }
    connect(&mut engine, ep(a, 2), ep(b, 1))?;
    crate::tests::support::sync_onto_board(&mut engine)?;
    let net = net_of(&engine, ep(a, 2))?;
    let pad = crate::layout::pads(engine.project())
        .into_iter()
        .find(|p| p.terminal == ep(a, 2))
        .ok_or_else(|| Error::NotFound("pad".into()))?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net,
                start: pad.position,
                end: [pad.position[0] + 2.0, pad.position[1]],
                width: 0.3,
                layer: Layer::Top.into(),
            },
        }),
    )?;
    let before = engine.document().clone();
    let revision = engine.revision();
    let wire = engine.project().wires[0].id;
    assert!(
        apply(
            &mut engine,
            Operation::Schematic(SchematicCommand::Disconnect { wire })
        )
        .is_err()
    );
    assert_eq!(engine.document(), &before);
    assert_eq!(engine.revision(), revision);
    Ok(())
}

#[test]
fn a_lone_input_is_a_completeness_finding_not_an_erc_finding() -> Result<()> {
    let mut engine = Engine::default();
    let fixture = two_terminal(&mut engine)?;
    let (a, _) = place(&mut engine, &fixture, "U1", 1, [0.0, 0.0])?;
    let (b, _) = place(&mut engine, &fixture, "U2", 1, [20.0, 0.0])?;
    assert!(erc::checks(engine.project()).is_empty());
    let report = inspect(&engine, a)?;
    let unaccounted: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.code == "instance.terminal_unaccounted")
        .map(|f| f.objects.clone())
        .collect();
    assert_eq!(
        unaccounted,
        vec![
            vec![crate::ObjectRef::Terminal { endpoint: ep(a, 1) }],
            vec![crate::ObjectRef::Terminal { endpoint: ep(a, 2) }],
        ]
    );
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.blocks_stage(crate::Stage::ElectricallyResolved))
    );
    connect(&mut engine, ep(a, 2), ep(b, 2))?;
    let codes: Vec<_> = erc::checks(engine.project())
        .into_iter()
        .map(|f| f.code)
        .collect();
    assert_eq!(codes, ["erc.driver_conflict"]);
    Ok(())
}

#[test]
fn exact_revisions_and_connectivity_survive_engine_history() -> Result<()> {
    let mut engine = Engine::demo()?;
    let fixture = demo_fixture(&engine)?;
    let (r1, r2) = (by_designator(&engine, "R1")?, by_designator(&engine, "R2")?);
    let (r3, _) = place(&mut engine, &fixture, "R3", 1, [40.0, 0.0])?;
    connect(&mut engine, ep(r2, 1), ep(r3, 1))?;
    let connected = schematic::nets(engine.project()).to_vec();
    assert_eq!(
        connected
            .iter()
            .filter(|net| net.terminals.len() == 3)
            .count(),
        1
    );
    engine.undo()?;
    assert_eq!(schematic::nets(engine.project()).len(), connected.len() + 1);
    engine.redo()?;
    assert_eq!(schematic::nets(engine.project()), connected);
    let anchor = schematic::pin_position(engine.project(), ep(r1, 2));
    cascade(&mut engine, &fixture, |engine| {
        with(engine, fixture.symbol, symbol_body, |s| {
            s.units[0].pins[1].position = [15.0, 0.0]
        })
    })?;
    assert_eq!(engine.project().library.symbols.len(), 2);
    assert_eq!(schematic::pin_position(engine.project(), ep(r1, 2)), anchor);
    assert_eq!(engine.project().instances[0].component, fixture.r1());
    assert_eq!(schematic::nets(engine.project()), connected);
    let before = engine.document().clone();
    let revision = engine.revision();
    let missing = place_ref(
        &mut engine,
        &fixture,
        crate::ComponentRef::new(fixture.component, crate::Revision(99)),
        "R99",
        1,
        [0.0, 0.0],
    );
    assert!(missing.is_err());
    assert_eq!(engine.document(), &before);
    assert_eq!(engine.revision(), revision);
    Ok(())
}

#[test]
fn routed_project_and_incomplete_drafts_roundtrip_through_the_document() -> Result<()> {
    let mut engine = Engine::demo()?;
    let fixture = demo_fixture(&engine)?;
    let r1 = by_designator(&engine, "R1")?;
    let shared = net_of(&engine, ep(r1, 2))?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net: shared,
                start: [9.2, 8.0],
                end: [14.8, 8.0],
                width: 0.3,
                layer: Layer::Top.into(),
            },
        }),
    )?;
    let empty = new_draft(&mut engine, crate::DefinitionKind::Component)?;
    with(&mut engine, fixture.symbol, symbol_body, |s| {
        s.units[0].pins[0].terminal = crate::TerminalId(999);
        // Finite but degenerate: a draft keeps it; publishing would not.
        s.units[0].graphics.push(crate::library::Graphic {
            id: crate::GraphicId(999),
            item: crate::library::Item::Shape {
                shape: crate::geometry::Shape::rect([0.0, 0.0], [0.0, -1.0]),
                width: 0.0,
                fill: crate::library::Fill::Solid,
            },
        });
    })?;
    let bytes = format::encode(&engine.to_file())?;
    let opened = Engine::from_file(format::decode(&bytes)?)?;
    assert_eq!(opened.drafts(), engine.drafts());
    assert!(opened.drafts().get(empty).is_some());
    assert_eq!(opened.project(), engine.project());
    assert_eq!(opened.project().tracks.len(), 1);
    assert!(!opened.can_undo());
    Ok(())
}

#[test]
fn corrupt_newer_and_invalid_documents_leave_engine_and_history_unchanged() -> Result<()> {
    let mut engine = Engine::demo()?;
    let fixture = demo_fixture(&engine)?;
    with(&mut engine, fixture.binding, binding_body, |b| {
        b.mappings.clear()
    })?;
    let r1 = by_designator(&engine, "R1")?;
    apply(&mut engine, designator_edit(r1, "R99"))?;
    let before = engine.to_file();
    let revision = engine.revision();
    let mut newer = serde_json::to_value(&before)?;
    newer["version"] = json!(999);
    let mut feature = serde_json::to_value(&before)?;
    feature["required_features"] = json!(["future-editor"]);
    let mut missing = serde_json::to_value(&before)?;
    missing["project"]["library"]["components"] = json!([]);
    let mut net = serde_json::to_value(&before)?;
    net["project"]["nets"][0]["terminals"] = json!([]);
    let mut draft = serde_json::to_value(&before)?;
    draft["drafts"]["items"][1]["definition"]["body"]["units"][0]["pins"][0]["position"] =
        json!([null, 2]);
    let mut legacy = serde_json::to_value(&before)?;
    legacy["authoring_draft"] = json!(null);
    let mut unplaced_wire = serde_json::to_value(&before)?;
    unplaced_wire["project"]["occurrences"] = json!([]);
    let mut stamp = serde_json::to_value(&before)?;
    stamp["project"]["library"]["components"][0]["verified"] = json!(true);
    assert!(matches!(
        format::decode(&serde_json::to_vec(&newer)?),
        Err(Error::Unsupported(_))
    ));
    // Package looks are read from the built-in material table: a project of
    // another table revision is refused, never reopened with other looks.
    let mut materials = serde_json::to_value(&before)?;
    materials["project"]["material_table"] =
        json!(crate::materials::BUILTIN_REVISION.wrapping_add(1));
    assert!(matches!(
        format::decode(&serde_json::to_vec(&materials)?),
        Err(Error::Unsupported(_))
    ));
    for bytes in [
        b"broken".to_vec(),
        serde_json::to_vec(&newer)?,
        serde_json::to_vec(&feature)?,
        serde_json::to_vec(&missing)?,
        serde_json::to_vec(&net)?,
        serde_json::to_vec(&draft)?,
        serde_json::to_vec(&legacy)?,
        serde_json::to_vec(&unplaced_wire)?,
        serde_json::to_vec(&stamp)?,
    ] {
        assert!(
            format::decode(&bytes)
                .and_then(|file| engine.replace_document(file))
                .is_err()
        );
        assert_eq!(engine.to_file(), before);
        assert_eq!(engine.revision(), revision);
        assert!(engine.can_undo());
    }
    let mut unsafe_draft = before.clone();
    for draft in &mut unsafe_draft.drafts.items {
        if let crate::library::Definition::Footprint(f) = &mut draft.definition {
            f.pads[0].position[0] = f64::INFINITY;
        }
    }
    assert!(engine.replace_document(unsafe_draft).is_err());
    assert_eq!(engine.to_file(), before);
    engine.undo()?;
    assert_eq!(engine.project().instances[0].designator, "R1");
    Ok(())
}

#[test]
fn encoding_is_bounded_and_validated() -> Result<()> {
    let engine = Engine::demo()?;
    let mut invalid = engine.to_file();
    invalid.drafts.items[0].meta.provenance = "x".repeat(4097);
    assert!(format::encode(&invalid).is_err());
    assert!(
        format::encode_bounded(&"x".repeat(format::MAX_FILE_BYTES)).is_err(),
        "serialization is bounded before writing"
    );
    assert!(format::decode(&vec![b' '; format::MAX_FILE_BYTES + 1]).is_err());
    Ok(())
}

#[test]
fn tracks_on_removed_terminals_fail_validation_instead_of_retagging() -> Result<()> {
    let mut engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let mut project = engine.project().clone();
    let net = net_of(&engine, ep(r1, 1))?;
    let id = project.allocate()?;
    project.tracks.push(Track {
        id,
        net: net + 1000,
        start: [5.0, 5.0],
        end: [6.0, 5.0],
        width: 0.3,
        layer: Layer::Top.into(),
        locked: false,
    });
    assert!(crate::validate::project(&project).is_err());
    let mut file = engine.to_file();
    file.project = project;
    assert!(engine.replace_document(file).is_err());
    Ok(())
}
