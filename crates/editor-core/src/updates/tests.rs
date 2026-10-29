use super::*;
use crate::{
    Engine, Layer, Operation, Part, Request,
    layout::{LayoutCommand, RouteRequest},
    parts::PartCommand,
    schematic::SchematicCommand,
};

fn apply(engine: &mut Engine, operation: Operation) -> Result<()> {
    engine.apply(engine.revision(), operation)
}
fn route(engine: &mut Engine, net: &str, start: [f64; 2], end: [f64; 2]) -> Result<Id> {
    let id = engine.project().next_id;
    apply(
        engine,
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net: net.into(),
                start,
                end,
                width: 0.3,
                layer: Layer::Top,
            },
        }),
    )?;
    Ok(id)
}
fn routed() -> Result<(Engine, Id, Id)> {
    let mut engine = Engine::demo()?;
    let shared = route(&mut engine, "net:2:2", [9.2, 8.0], [14.8, 8.0])?;
    let unaffected = route(&mut engine, "net:3:2", [17.2, 8.0], [21.0, 8.0])?;
    Ok((engine, shared, unaffected))
}
fn publish(engine: &mut Engine, edit: impl FnOnce(&mut Part)) -> Result<PartRef> {
    let mut draft = engine
        .project()
        .parts
        .last()
        .ok_or_else(|| invalid("test part missing"))?
        .clone();
    edit(&mut draft);
    let target = PartRef {
        id: draft.id,
        revision: draft.revision + 1,
    };
    apply(engine, Operation::Part(PartCommand::Publish { draft }))?;
    Ok(target)
}
fn revision_change(target: PartRef) -> Change {
    Change::PartRevision {
        instances: vec![2],
        target,
    }
}

#[test]
fn compatible_geometry_preview_is_read_only_preserves_copper_and_survives_offline_reopen()
-> Result<()> {
    let (mut engine, _, _) = routed()?;
    let target = publish(&mut engine, |part| {
        part.footprint.pads[1].position = [1.2, 2.0];
        part.package.body_size[2] = 2.0;
    })?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let plan = engine.preview_update(revision_change(target));
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(plan.conflicting_tracks.is_empty());
    assert!(plan.warnings.iter().any(|w| w.contains("detached")));
    assert!(plan.changes.iter().any(|c| c.contains("model dimensions")));
    assert!(
        plan.changes
            .iter()
            .any(|c| c.contains("world geometry") && c.contains("9.2, 10.0"))
    );
    assert_eq!(
        engine.project(),
        &before,
        "cancel is simply discarding a pure plan"
    );
    assert_eq!(engine.revision(), revision);
    engine.apply_update(plan.revision, plan.change, false)?;
    assert_eq!(engine.project().tracks, before.tracks);
    assert_eq!(engine.project().instances[0].part, target);
    assert_eq!(engine.project().instances[1].part.revision, 1);
    let after = engine.project().clone();
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    engine.redo()?;
    assert_eq!(engine.project(), &after);
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("updated.pcbcraft");
    engine.save(&path)?;
    let mut reopened = Engine::default();
    reopened.open(&path)?;
    assert_eq!(reopened.project(), &after);
    Ok(())
}

#[test]
fn mapping_changes_require_listed_track_removal_without_retagging_others() -> Result<()> {
    let (mut engine, shared, unaffected) = routed()?;
    let target = publish(&mut engine, |part| {
        part.binding.mappings[0].electrical = 2;
        part.binding.mappings[1].electrical = 1;
    })?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let plan = engine.preview_update(revision_change(target));
    assert_eq!(
        schematic::nets(&before),
        schematic::nets(&stage(&before, &plan.change)?)
    );
    assert_eq!(plan.conflicting_tracks, vec![shared]);
    assert!(plan.changes.iter().any(|c| c.contains("maps electrical")));
    assert!(
        engine
            .apply_update(plan.revision, plan.change.clone(), false)
            .is_err()
    );
    assert_eq!(engine.project(), &before);
    assert_eq!(engine.revision(), revision);
    engine.apply_update(plan.revision, plan.change, true)?;
    assert_eq!(engine.project().tracks.len(), 1);
    assert_eq!(
        engine.project().tracks[0],
        before
            .tracks
            .iter()
            .find(|t| t.id == unaffected)
            .ok_or_else(|| invalid("test track missing"))?
            .clone()
    );
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn disconnect_and_remove_plan_wires_placements_and_conflicts_atomically() -> Result<()> {
    let (mut engine, shared, unaffected) = routed()?;
    let wire = engine.project().wires[0].id;
    let plan = engine.preview_update(Change::Schematic {
        command: SchematicCommand::Disconnect { wire },
    });
    assert_eq!(plan.removed_wires, vec![wire]);
    assert_eq!(plan.conflicting_tracks, vec![shared]);
    let before = engine.project().clone();
    engine.apply_update(plan.revision, plan.change, true)?;
    assert!(engine.project().wires.is_empty());
    assert_eq!(engine.project().tracks[0].id, unaffected);
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    let plan = engine.preview_update(Change::Schematic {
        command: SchematicCommand::Remove { instance: 2 },
    });
    assert_eq!(plan.removed_placements, vec![2]);
    assert_eq!(plan.removed_wires, vec![wire]);
    assert_eq!(plan.conflicting_tracks, vec![shared]);
    engine.apply_update(plan.revision, plan.change, true)?;
    assert!(engine.project().instance(2).is_none());
    assert_eq!(engine.project().placements.len(), 1);
    assert_eq!(engine.project().tracks[0].id, unaffected);
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn stale_preview_and_malformed_changes_do_not_touch_history() -> Result<()> {
    let (mut engine, _, _) = routed()?;
    let target = publish(&mut engine, |part| part.name = "Renamed".into())?;
    let plan = engine.preview_update(revision_change(target));
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Edit {
            instance: 2,
            reference: "R9".into(),
            value: "1k".into(),
        }),
    )?;
    let before = engine.project().clone();
    let revision = engine.revision();
    assert!(matches!(
        engine.apply_update(plan.revision, plan.change.clone(), false),
        Err(Error::Stale { .. })
    ));
    assert!(matches!(
        engine.apply(
            revision,
            Operation::Update(Box::new(ApplyUpdate {
                expected_revision: plan.revision,
                change: plan.change,
                remove_conflicting_tracks: true
            }))
        ),
        Err(Error::Stale { .. })
    ));
    let bad = Change::PartRevision {
        instances: vec![999],
        target,
    };
    assert!(!engine.preview_update(bad.clone()).blockers.is_empty());
    assert!(engine.apply_update(revision, bad, true).is_err());
    assert_eq!(engine.project(), &before);
    assert_eq!(engine.revision(), revision);
    engine.undo()?;
    assert_eq!(engine.project().instances[0].reference, "R1");
    Ok(())
}

#[test]
fn locked_affected_tracks_and_placements_block_even_with_removal_opt_in() -> Result<()> {
    let (mut engine, shared, _) = routed()?;
    let target = publish(&mut engine, |part| {
        part.footprint.pads[1].position = [1.2, 2.0]
    })?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetTrackLock {
            id: shared,
            locked: true,
        }),
    )?;
    let plan = engine.preview_update(revision_change(target));
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.contains("Track") && b.contains("locked"))
    );
    let before = engine.project().clone();
    assert!(
        engine
            .apply_update(plan.revision, plan.change, true)
            .is_err()
    );
    assert_eq!(engine.project(), &before);
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetTrackLock {
            id: shared,
            locked: false,
        }),
    )?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: 2,
            locked: true,
        }),
    )?;
    let plan = engine.preview_update(revision_change(target));
    assert!(plan.blockers.iter().any(|b| b.contains("Placement 2")));
    assert!(
        engine
            .apply_update(plan.revision, plan.change, true)
            .is_err()
    );
    Ok(())
}

#[test]
fn moved_foreign_pad_cannot_silently_short_retained_track() -> Result<()> {
    let (mut engine, shared, _) = routed()?;
    let target = publish(&mut engine, |part| {
        part.footprint.pads[0].position = [3.0, 0.0]
    })?;
    let plan = engine.preview_update(revision_change(target));
    assert_eq!(plan.conflicting_tracks, vec![shared]);
    let before = engine.project().clone();
    assert!(
        engine
            .apply_update(plan.revision, plan.change, false)
            .is_err()
    );
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn removed_terminal_is_not_reconnected_by_name_and_lists_wire_loss() -> Result<()> {
    let (mut engine, shared, _) = routed()?;
    let target = publish(&mut engine, |part| {
        part.component.terminals[1].id = 20;
        part.symbol.pins[1].terminal = 20;
        part.binding.mappings[1].electrical = 20;
    })?;
    let plan = engine.preview_update(revision_change(target));
    assert_eq!(plan.removed_wires, vec![engine.project().wires[0].id]);
    assert_eq!(plan.conflicting_tracks, vec![shared]);
    engine.apply_update(plan.revision, plan.change, true)?;
    assert!(engine.project().wires.is_empty());
    assert!(
        schematic::nets(engine.project())
            .iter()
            .any(|n| n.id == "net:2:20")
    );
    Ok(())
}

#[test]
fn preview_and_apply_are_available_through_public_transport_and_schema() -> Result<()> {
    let mut engine = Engine::demo()?;
    let target = publish(&mut engine, |part| part.name = "New caption".into())?;
    let revision = engine.revision();
    let value = engine.dispatch(serde_json::to_value(Request::PreviewUpdate {
        change: revision_change(target),
    })?)?;
    let plan: UpdatePlan = serde_json::from_value(value)?;
    assert_eq!(plan.revision, revision);
    engine.dispatch(serde_json::to_value(Request::ApplyUpdate(ApplyUpdate {
        expected_revision: plan.revision,
        change: plan.change,
        remove_conflicting_tracks: false,
    }))?)?;
    assert_eq!(engine.project().instances[0].part, target);
    assert!(crate::schemas().get("update_plan").is_some());
    let before = engine.project().clone();
    let plan = engine.preview_update(Change::SyncPcb {});
    assert!(plan.blockers.is_empty());
    engine.apply_update(plan.revision, plan.change, false)?;
    assert_eq!(
        engine.project(),
        &before,
        "sync preserves existing placements"
    );
    Ok(())
}

#[test]
fn label_only_terminal_changes_warn_and_preserve_all_routed_copper() -> Result<()> {
    let (mut engine, _, _) = routed()?;
    let target = publish(&mut engine, |part| {
        part.component.terminals[1].name = "Signal".into()
    })?;
    let before = engine.project().clone();
    let plan = engine.preview_update(revision_change(target));
    assert!(plan.conflicting_tracks.is_empty());
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("renamed") && w.contains("different signal"))
    );
    engine.apply_update(plan.revision, plan.change, false)?;
    assert_eq!(engine.project().tracks, before.tracks);
    assert_eq!(schematic::nets(engine.project()), schematic::nets(&before));
    Ok(())
}

#[test]
fn unrelated_definition_with_matching_terminal_names_is_blocked() -> Result<()> {
    let (mut engine, _, _) = routed()?;
    let id = engine.project().next_id;
    apply(
        &mut engine,
        Operation::Part(PartCommand::Publish {
            draft: crate::parts::passive_template("Resistor"),
        }),
    )?;
    let before = engine.project().clone();
    let plan = engine.preview_update(revision_change(PartRef { id, revision: 1 }));
    assert!(plan.blockers.iter().any(|b| b.contains("definition ID")));
    assert!(
        engine
            .apply_update(plan.revision, plan.change, true)
            .is_err()
    );
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn pcb_sync_preview_adds_only_missing_placement_and_preserves_copper() -> Result<()> {
    let (mut engine, _, _) = routed()?;
    let id = engine.project().next_id;
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Place {
            part: PartRef { id: 1, revision: 1 },
            reference: "R3".into(),
            value: "10k".into(),
            position: [30.0, 0.0],
        }),
    )?;
    let before = engine.project().clone();
    let plan = engine.preview_update(Change::SyncPcb {});
    assert_eq!(plan.affected_placements, vec![id]);
    assert!(plan.conflicting_tracks.is_empty());
    assert!(plan.removed_placements.is_empty());
    engine.apply_update(plan.revision, plan.change, false)?;
    assert_eq!(&engine.project().placements[..2], &before.placements);
    assert_eq!(engine.project().tracks, before.tracks);
    assert_eq!(engine.project().placements[2].instance, id);
    Ok(())
}

#[test]
fn newly_device_declared_nc_removes_singleton_net_copper_only_explicitly() -> Result<()> {
    let (mut engine, _, _) = routed()?;
    let nc_track = route(&mut engine, "net:2:1", [6.8, 8.0], [6.8, 5.0])?;
    let target = publish(&mut engine, |part| {
        part.component.terminals[0].kind = PinKind::NotConnected
    })?;
    let plan = engine.preview_update(revision_change(target));
    assert_eq!(plan.conflicting_tracks, vec![nc_track]);
    let before = engine.project().clone();
    assert!(
        engine
            .apply_update(plan.revision, plan.change.clone(), false)
            .is_err()
    );
    assert_eq!(engine.project(), &before);
    engine.apply_update(plan.revision, plan.change, true)?;
    assert_eq!(engine.project().tracks.len(), 2);
    Ok(())
}
