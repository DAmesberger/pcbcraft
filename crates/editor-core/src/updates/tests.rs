use super::*;
use crate::library::Definition;
use crate::tests::support::*;
use crate::{
    DefinitionKind, Engine, InstanceCommand, Layer, Operation, Revision, SymbolRef,
    layout::{LayoutCommand, RouteRequest},
    schematic::SchematicCommand,
};

/// Route on the net of `terminal`.
fn route(engine: &mut Engine, terminal: Endpoint, start: [f64; 2], end: [f64; 2]) -> Result<Id> {
    let id = engine.project().next_id;
    let net = net_of(engine, terminal)?;
    apply(
        engine,
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net,
                start,
                end,
                width: 0.3,
                layer: Layer::Top.into(),
            },
        }),
    )?;
    Ok(id)
}

struct Routed {
    engine: Engine,
    fixture: Fixture,
    r1: Id,
    shared: Id,
    unaffected: Id,
}

fn routed() -> Result<Routed> {
    let mut engine = Engine::demo()?;
    let fixture = demo_fixture(&engine)?;
    let (r1, r2) = (by_designator(&engine, "R1")?, by_designator(&engine, "R2")?);
    let shared = route(&mut engine, ep(r1, 2), [9.2, 8.0], [14.8, 8.0])?;
    let unaffected = route(&mut engine, ep(r2, 2), [17.2, 8.0], [21.0, 8.0])?;
    Ok(Routed {
        engine,
        fixture,
        r1,
        shared,
        unaffected,
    })
}

fn revision_change(instance: Id, target: ComponentRef) -> Change {
    Change::ComponentRevision {
        instances: vec![instance],
        target,
        resolutions: Resolutions::default(),
    }
}

#[test]
fn compatible_geometry_preview_is_read_only_preserves_copper_and_survives_offline_reopen()
-> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.footprint, footprint_body, |fp| {
            fp.pads[1].position = [1.2, 2.0]
        })?;
        with(e, f.package, package_body, |p| {
            // A refusal would leave the envelope stale, which publishing rejects.
            let _ = p.resize_envelope([p.envelope[0], p.envelope[1], 2.0]);
        })
    })?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let plan = engine.preview_update(revision_change(r1, target));
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(plan.conflicting_tracks.is_empty());
    assert!(plan.warnings.iter().any(|w| w.contains("detached")));
    assert!(
        plan.changes
            .iter()
            .any(|c| c.starts_with("R1: body ") && c.contains("→") && c.contains("(L×W×H)")),
        "{:?}",
        plan.changes
    );
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
    assert_eq!(engine.project().instances[0].component, target);
    assert_eq!(
        engine.project().instances[1].component.revision,
        Revision(1)
    );
    let after = engine.project().clone();
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    engine.redo()?;
    assert_eq!(engine.project(), &after);
    let bytes = crate::format::encode(&engine.to_file())?;
    let reopened = Engine::from_file(crate::format::decode(&bytes)?)?;
    assert_eq!(reopened.project(), &after);
    Ok(())
}

#[test]
fn mapping_changes_require_listed_track_removal_without_retagging_others() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        shared,
        unaffected,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.binding, binding_body, |b| {
            b.mappings[0].terminal = crate::TerminalId(2);
            b.mappings[1].terminal = crate::TerminalId(1);
        })
    })?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let plan = engine.preview_update(revision_change(r1, target));
    assert_eq!(
        schematic::nets(&before),
        schematic::nets(&stage_change(&before, &plan.change)?)
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
    let Routed {
        mut engine,
        r1,
        shared,
        unaffected,
        ..
    } = routed()?;
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
    let plan = engine.preview_update(Change::Instance {
        edit: InstanceChange::Remove { instance: r1 },
    });
    assert_eq!(plan.removed_placements, vec![r1]);
    assert_eq!(plan.removed_wires, vec![wire]);
    assert_eq!(plan.conflicting_tracks, vec![shared]);
    engine.apply_update(plan.revision, plan.change, true)?;
    assert!(engine.project().instance(r1).is_none());
    assert!(engine.project().occurrences_of(r1).next().is_none());
    assert_eq!(engine.project().placements.len(), 1);
    assert_eq!(engine.project().tracks[0].id, unaffected);
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn stale_preview_and_malformed_changes_do_not_touch_history() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        let draft = e
            .drafts()
            .get(f.component)
            .cloned()
            .ok_or_else(|| invalid("draft"))?;
        edit(
            e,
            vec![crate::library::DraftEdit {
                id: f.component,
                meta: crate::library::DefinitionMeta {
                    name: "Renamed".into(),
                    ..draft.meta
                },
                definition: draft.definition,
            }],
        )
    })?;
    let plan = engine.preview_update(revision_change(r1, target));
    apply(
        &mut engine,
        Operation::Instance(InstanceCommand::SetDesignator {
            instance: r1,
            designator: "R9".into(),
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
    let bad = revision_change(999, target);
    assert!(!engine.preview_update(bad.clone()).blockers.is_empty());
    assert!(engine.apply_update(revision, bad, true).is_err());
    assert_eq!(engine.project(), &before);
    assert_eq!(engine.revision(), revision);
    engine.undo()?;
    assert_eq!(engine.project().instances[0].designator, "R1");
    Ok(())
}

#[test]
fn locked_affected_tracks_and_placements_block_even_with_removal_opt_in() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        shared,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.footprint, footprint_body, |fp| {
            fp.pads[1].position = [1.2, 2.0]
        })
    })?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetTrackLock {
            id: shared,
            locked: true,
        }),
    )?;
    let plan = engine.preview_update(revision_change(r1, target));
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
            instance: r1,
            locked: true,
        }),
    )?;
    let plan = engine.preview_update(revision_change(r1, target));
    assert!(plan.blockers.iter().any(|b| b.contains("Placement of R1")));
    assert!(
        engine
            .apply_update(plan.revision, plan.change, true)
            .is_err()
    );
    Ok(())
}

#[test]
fn moved_foreign_pad_cannot_silently_short_retained_track() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        shared,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.footprint, footprint_body, |fp| {
            fp.pads[0].position = [3.0, 0.0]
        })
    })?;
    let plan = engine.preview_update(revision_change(r1, target));
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
    let Routed {
        mut engine,
        fixture: f,
        r1,
        shared,
        ..
    } = routed()?;
    let twenty = crate::TerminalId(20);
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.component, component_body, |c| {
            c.terminals[1].id = twenty;
            c.next_local = 21;
        })?;
        with(e, f.symbol, symbol_body, |s| {
            s.units[0].pins[1].terminal = twenty
        })?;
        with(e, f.binding, binding_body, |b| {
            b.mappings[1].terminal = twenty
        })
    })?;
    let plan = engine.preview_update(revision_change(r1, target));
    assert_eq!(plan.removed_wires, vec![engine.project().wires[0].id]);
    assert_eq!(plan.conflicting_tracks, vec![shared]);
    engine.apply_update(plan.revision, plan.change, true)?;
    assert!(engine.project().wires.is_empty());
    assert!(engine.project().net_of(ep(r1, 20)).is_some());
    Ok(())
}

#[test]
fn preview_and_apply_roundtrip_through_serialized_operations_and_schema() -> Result<()> {
    let mut engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let r1 = by_designator(&engine, "R1")?;
    let target = cascade(&mut engine, &f, |_| Ok(()))?;
    let revision = engine.revision();
    let value = serde_json::to_value(engine.preview_update(revision_change(r1, target)))?;
    let plan: UpdatePlan = serde_json::from_value(value)?;
    assert_eq!(plan.revision, revision);
    let operation: Operation = serde_json::from_value(serde_json::to_value(Operation::Update(
        Box::new(ApplyUpdate {
            expected_revision: plan.revision,
            change: plan.change,
            remove_conflicting_tracks: false,
        }),
    ))?)?;
    apply(&mut engine, operation)?;
    assert_eq!(engine.project().instances[0].component, target);
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
    let Routed {
        mut engine,
        fixture: f,
        r1,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.component, component_body, |c| {
            c.terminals[1].name = "Signal".into()
        })
    })?;
    let before = engine.project().clone();
    let plan = engine.preview_update(revision_change(r1, target));
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
    let Routed { mut engine, r1, .. } = routed()?;
    let other = component(
        &mut engine,
        &Spec {
            name: "Resistor",
            terminals: vec![(1, "1", passive()), (2, "2", passive())],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: true,
        },
    )?;
    let before = engine.project().clone();
    let plan = engine.preview_update(revision_change(r1, other.r1()));
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.contains("definition lineage"))
    );
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
    let Routed {
        mut engine,
        fixture: f,
        ..
    } = routed()?;
    let (id, _) = place(&mut engine, &f, "R3", 1, [30.0, 0.0])?;
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
    let Routed {
        mut engine,
        fixture: f,
        r1,
        ..
    } = routed()?;
    let nc_track = route(&mut engine, ep(r1, 1), [6.8, 8.0], [6.8, 5.0])?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.component, component_body, |c| {
            c.terminals[0].class = crate::library::TerminalClass::DeviceNc
        })
    })?;
    let plan = engine.preview_update(revision_change(r1, target));
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

/// A second binding lineage of the demo component on the same package and
/// footprint, published in component r2 as its only implementation.
fn alternative_binding(engine: &mut Engine, f: &Fixture) -> Result<(DefinitionId, ComponentRef)> {
    let binding = new_draft(engine, DefinitionKind::Binding)?;
    let body = match body(engine, f.binding)? {
        Definition::Binding(b) => b,
        _ => return Err(invalid("binding draft")),
    };
    rewrite(engine, binding, |d| *d = Definition::Binding(body))?;
    let draft = engine
        .drafts()
        .get(binding)
        .cloned()
        .ok_or_else(|| invalid("draft"))?;
    edit(
        engine,
        vec![crate::library::DraftEdit {
            id: binding,
            meta: meta("Alternative binding"),
            definition: draft.definition,
        }],
    )?;
    with(engine, f.component, component_body, |c| {
        c.implementations = vec![crate::BindingRef::new(binding, Revision(1))]
    })?;
    let revisions = publish(engine, vec![binding, f.component])?;
    let target = revisions
        .into_iter()
        .find(|r| r.id == f.component)
        .map(|r| ComponentRef::new(r.id, r.revision))
        .ok_or_else(|| invalid("component revision"))?;
    Ok((binding, target))
}

#[test]
fn a_choice_no_longer_offered_blocks_until_resolved() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        ..
    } = routed()?;
    let (binding, target) = alternative_binding(&mut engine, &f)?;
    let before = engine.document().clone();
    let plan = engine.preview_update(revision_change(r1, target));
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.contains("not offered") && b.contains("ChoiceResolution")),
        "{:?}",
        plan.blockers
    );
    assert!(
        engine
            .apply_update(plan.revision, plan.change, true)
            .is_err()
    );
    assert_eq!(engine.document(), &before);
    let choice = PhysicalChoice {
        binding,
        footprint: f.footprint,
    };
    let resolved = Change::ComponentRevision {
        instances: vec![r1],
        target,
        resolutions: Resolutions {
            choices: vec![ChoiceResolution {
                instance: r1,
                representation: None,
                physical: Some(Some(choice)),
            }],
            ..Default::default()
        },
    };
    // The resolution survives the transport form, including `physical`.
    let resolved: Change = serde_json::from_value(serde_json::to_value(&resolved)?)?;
    let plan = engine.preview_update(resolved);
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        plan.changes
            .iter()
            .any(|c| c.contains("physical implementation"))
    );
    engine.apply_update(plan.revision, plan.change, false)?;
    assert_eq!(
        engine.project().instance(r1).and_then(|i| i.physical),
        Some(choice)
    );
    // Direct implementation changes are refused while the instance is placed.
    let r2 = by_designator(&engine, "R2")?;
    let direct = engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::SetPhysical {
            instance: r2,
            choice: Some(choice),
        }),
    );
    assert_eq!(direct.err().map(|e| e.code()), Some("conflict"));
    Ok(())
}

#[test]
fn a_representation_lineage_change_needs_explicit_unit_resolutions() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        ..
    } = routed()?;
    // R3 has only an unused flag, no occurrence.
    let (r3, occurrence) = place(&mut engine, &f, "R3", 1, [40.0, 0.0])?;
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::UnplaceUnit { occurrence }),
    )?;
    apply(
        &mut engine,
        Operation::Instance(InstanceCommand::SetUnitUnused {
            instance: r3,
            unit: UnitId(1),
            unused: true,
        }),
    )?;
    // A new symbol lineage whose only unit also has ID 1.
    let symbol = new_draft(&mut engine, DefinitionKind::Symbol)?;
    let body = match self::body(&engine, f.symbol)? {
        Definition::Symbol(s) => s,
        _ => return Err(invalid("symbol draft")),
    };
    edit(
        &mut engine,
        vec![crate::library::DraftEdit {
            id: symbol,
            meta: meta("Alternative symbol"),
            definition: Definition::Symbol(body),
        }],
    )?;
    with(&mut engine, f.component, component_body, |c| {
        c.representations = vec![SymbolRef::new(symbol, Revision(1))]
    })?;
    let revisions = publish(&mut engine, vec![symbol, f.component])?;
    let target = revisions
        .into_iter()
        .find(|r| r.id == f.component)
        .map(|r| ComponentRef::new(r.id, r.revision))
        .ok_or_else(|| invalid("component revision"))?;
    for (instance, drop) in [(r1, false), (r3, true)] {
        let choice = ChoiceResolution {
            instance,
            representation: Some(symbol),
            physical: None,
        };
        let change = |units| Change::ComponentRevision {
            instances: vec![instance],
            target,
            resolutions: Resolutions {
                choices: vec![choice.clone()],
                units,
                regroups: vec![],
                fields: vec![],
                functions: vec![],
                parts: vec![],
            },
        };
        let blocked = engine.preview_update(change(vec![]));
        assert!(
            blocked
                .blockers
                .iter()
                .any(|b| b.contains("UnitResolution")),
            "{instance}: {:?}",
            blocked.blockers
        );
        let resolution = UnitResolution {
            instance,
            unit: UnitId(1),
            target: if drop {
                UnitTarget::Drop
            } else {
                UnitTarget::Unit(UnitId(1))
            },
        };
        let plan = engine.preview_update(change(vec![resolution]));
        assert!(plan.blockers.is_empty(), "{instance}: {:?}", plan.blockers);
        engine.apply_update(plan.revision, plan.change, false)?;
        let after = engine
            .project()
            .instance(instance)
            .cloned()
            .ok_or_else(|| invalid("instance"))?;
        assert_eq!(after.representation, symbol);
        assert!(after.unused_units.is_empty());
        assert_eq!(
            engine
                .project()
                .occurrences_of(instance)
                .map(|o| o.unit)
                .collect::<Vec<_>>(),
            if drop { vec![] } else { vec![UnitId(1)] }
        );
    }
    Ok(())
}

#[test]
fn removing_an_unused_unit_in_the_same_lineage_lists_the_flag_without_blocking() -> Result<()> {
    let mut engine = Engine::default();
    let f = component(
        &mut engine,
        &Spec {
            name: "Split pair",
            terminals: vec![(1, "1", passive()), (2, "2", passive())],
            units: vec![
                (1, "A", vec![(1, [-5.0, 0.0])]),
                (2, "B", vec![(2, [5.0, 0.0])]),
            ],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: true,
        },
    )?;
    let (x1, _) = place(&mut engine, &f, "X1", 1, [0.0, 0.0])?;
    apply(
        &mut engine,
        Operation::Instance(InstanceCommand::SetUnitUnused {
            instance: x1,
            unit: UnitId(2),
            unused: true,
        }),
    )?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.symbol, symbol_body, |s| {
            let moved: Vec<_> = s.units[1].pins.drain(..).collect();
            s.units[0].pins.extend(moved);
            s.units.remove(1);
        })
    })?;
    let plan = engine.preview_update(revision_change(x1, target));
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        plan.changes
            .iter()
            .any(|c| c == "X1: unit 2 is no longer marked unused."),
        "{:#?}",
        plan.changes
    );
    assert!(
        plan.changes
            .iter()
            .any(|c| c.contains("unit B (2) is removed"))
    );
    engine.apply_update(plan.revision, plan.change, false)?;
    assert!(
        engine
            .project()
            .instance(x1)
            .is_some_and(|i| i.unused_units.is_empty())
    );
    Ok(())
}

#[test]
fn symbol_drawing_changes_are_reported_as_presentation_only() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        shared,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.symbol, symbol_body, |s| {
            s.next_local = 6;
            // Move the designator, add a line and decorate pin 1 without
            // moving its connection end.
            if let crate::library::Item::Text(t) = &mut s.units[0].graphics[1].item {
                t.at[1] += 1.0;
            }
            s.units[0].graphics.push(crate::library::Graphic {
                id: crate::GraphicId(5),
                item: crate::library::Item::Shape {
                    shape: crate::geometry::Shape::Line {
                        a: [-2.5, 0.0],
                        b: [2.5, 0.0],
                    },
                    width: 0.25,
                    fill: crate::library::Fill::None,
                },
            });
            s.units[0].pins[0].decorations.inverted = true;
        })
    })?;
    let plan = engine.preview_update(revision_change(r1, target));
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let graphics = "R1: unit A graphics: 1 added, 0 removed, 1 changed (presentation only; terminals and nets unchanged).";
    assert!(
        plan.changes.iter().any(|c| c == graphics),
        "{:?}",
        plan.changes
    );
    assert!(
        plan.changes
            .iter()
            .any(|c| c.starts_with("R1 terminal 1 pin in unit A: direction")),
        "{:?}",
        plan.changes
    );
    // No connection point moved, so no pin move is reported and the routed
    // copper is untouched.
    assert!(
        !plan
            .changes
            .iter()
            .any(|c| c.contains("pin in unit") && c.contains(" → ")),
        "{:?}",
        plan.changes
    );
    assert!(plan.conflicting_tracks.is_empty());
    engine.apply_update(engine.revision(), revision_change(r1, target), false)?;
    assert!(engine.project().tracks.iter().any(|t| t.id == shared));
    Ok(())
}

#[test]
fn a_pin_that_moves_and_changes_drawing_is_not_called_presentation_only() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.symbol, symbol_body, |s| {
            let pin = &mut s.units[0].pins[0];
            pin.position[1] += 2.5;
            pin.decorations.inverted = true;
        })
    })?;
    let plan = engine.preview_update(revision_change(r1, target));
    let presentation: Vec<_> = plan
        .changes
        .iter()
        .filter(|c| c.starts_with("R1 terminal 1 pin in unit A: direction"))
        .collect();
    assert_eq!(presentation.len(), 1, "{:?}", plan.changes);
    assert!(
        presentation
            .iter()
            .all(|c| !c.contains("connection point") && !c.contains("presentation only")),
        "{:?}",
        plan.changes
    );
    assert!(
        plan.changes
            .iter()
            .any(|c| c.starts_with("R1 terminal 1 pin in unit A [") && c.contains(" → ")),
        "{:?}",
        plan.changes
    );
    Ok(())
}

#[test]
fn footprint_pad_diffs_name_each_aspect_by_pad_id() -> Result<()> {
    use crate::library::{
        DrawingLayer, Fill, FootprintGraphic, Hole, HoleShape, Item, MaskRule, PadKind, PadShape,
        Side,
    };
    let Routed {
        mut engine,
        fixture: f,
        r1,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.footprint, footprint_body, |fp| {
            // Pad 1: reshaped, renumbered and given a tented mask; a 180°
            // turn of a rectangle is no turn. Pad 2: a plated hole, so it
            // becomes plated-through. A new mechanical pad with no terminal.
            let p1 = &mut fp.pads[0];
            p1.shape = PadShape::Oblong { size: [1.0, 1.4] };
            p1.number = "A".into();
            p1.mask = MaskRule::Tented;
            p1.rotation_deg = 180.0;
            let p2 = &mut fp.pads[1];
            p2.kind = PadKind::PlatedThrough;
            p2.hole = Some(Hole {
                shape: HoleShape::Round { diameter: 0.5 },
                plated: true,
                offset: [0.0, 0.0],
            });
            let mut tab = fp.pads[0].clone();
            tab.id = crate::PadId(fp.next_local);
            tab.kind = PadKind::Mechanical;
            tab.terminal = None;
            tab.number = "MP".into();
            tab.position = [0.0, 3.0];
            // One assembly graphic changes, one silkscreen line is added.
            if let Some(FootprintGraphic {
                item: Item::Shape { width, .. },
                ..
            }) = fp
                .graphics
                .iter_mut()
                .find(|g| g.layer == DrawingLayer::Assembly)
            {
                *width = 0.12;
            }
            fp.graphics.push(FootprintGraphic {
                id: crate::GraphicId(fp.next_local + 1),
                layer: DrawingLayer::Silkscreen,
                side: Side::Component,
                item: Item::Shape {
                    shape: crate::geometry::Shape::Line {
                        a: [-1.0, 2.0],
                        b: [1.0, 2.0],
                    },
                    width: 0.15,
                    fill: Fill::None,
                },
            });
            fp.next_local += 2;
            fp.pads.push(tab);
        })
    })?;
    let plan = engine.preview_update(revision_change(r1, target));
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let has = |needle: &str| plan.changes.iter().any(|c| c.contains(needle));
    for needle in [
        "footprint pad ‘A’ is reshaped rect [1.0, 1.4] mm → oblong [1.0, 1.4] mm",
        "footprint pad ‘A’ is renumbered ‘1’ → ‘A’ (display only)",
        "footprint pad ‘A’ mask Inherit → Tented",
        "footprint pad ‘2’ kind Smd on Top → PlatedThrough on Top",
        "footprint pad ‘2’ hole none → drill ⌀0.5 mm",
        "add footprint pad ‘MP’: mechanical oblong",
        "at [0.0, 3.0] mm",
        "R1 pad A world geometry: rect",
        "→ oblong [1.0, 1.4] mm",
        "footprint Silkscreen graphics: 1 added, 0 removed, 0 changed (presentation only",
        "footprint Assembly graphics: 0 added, 0 removed, 1 changed (presentation only",
    ] {
        assert!(has(needle), "{needle}: {:#?}", plan.changes);
    }
    assert!(
        !has("turns"),
        "a rectangle turned 180° is unchanged: {:#?}",
        plan.changes
    );
    assert!(!has("unresolved"), "a mechanical pad needs no terminal");
    assert!(!has("moves"), "{:#?}", plan.changes);
    Ok(())
}

#[test]
fn a_retired_pad_lists_the_copper_touching_it() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        shared,
        ..
    } = routed()?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.footprint, footprint_body, |fp| {
            // Pad 2 is retired; a new pad with a new ID lands its terminal.
            let mut replacement = fp.pads[1].clone();
            replacement.id = crate::PadId(fp.next_local);
            fp.next_local += 1;
            fp.pads[1] = replacement;
        })
    })?;
    let plan = engine.preview_update(revision_change(r1, target));
    assert!(
        plan.changes
            .iter()
            .any(|c| c.starts_with("R1: footprint pad ‘2’ is retired")),
        "{:#?}",
        plan.changes
    );
    assert!(
        plan.warnings.iter().any(|w| w.starts_with(&format!(
            "Track {shared} touches R1 pad 2, which the update retires"
        ))),
        "{:#?}",
        plan.warnings
    );
    Ok(())
}

#[test]
fn array_growth_and_shrink_list_new_and_retired_pads() {
    use crate::library::footprint_source::{
        ArrayId, FootprintSource, GenerationKey, KeyedPad, SketchBlob, SolveSummary,
    };
    use crate::library::{FootprintDefinition, Pad};
    use crate::{PackageTerminalId, PadId};
    let key = |i| GenerationKey::Array {
        array: ArrayId(9),
        i,
        j: 0,
    };
    let footprint = |pads: &[(u32, u16, Option<u32>)]| FootprintDefinition {
        pads: pads
            .iter()
            .map(|(id, _, t)| {
                let mut pad =
                    Pad::smd_rect(PadId(*id), PackageTerminalId(1), "", [0.0, 0.0], [1.0, 1.0]);
                pad.terminal = t.map(PackageTerminalId);
                pad
            })
            .collect(),
        source: Some(Box::new(FootprintSource {
            evaluator: Default::default(),
            sketch: SketchBlob("{}".into()),
            parameters: vec![],
            pads: vec![],
            arrays: vec![],
            graphics: vec![],
            keys: pads
                .iter()
                .map(|(id, i, _)| KeyedPad {
                    key: key(*i),
                    pad: PadId(*id),
                })
                .collect(),
            overrides: vec![],
            solve: SolveSummary::default(),
            datum: None,
        })),
        ..FootprintDefinition::default()
    };
    let mut plan = UpdatePlan {
        revision: 0,
        change: revision_change(1, ComponentRef::default()),
        changes: vec![],
        affected_wires: vec![],
        removed_wires: vec![],
        affected_placements: vec![],
        removed_placements: vec![],
        conflicting_tracks: vec![],
        blockers: vec![],
        warnings: vec![],
    };
    let a = footprint(&[(1, 0, Some(1)), (2, 1, Some(2)), (3, 2, Some(3))]);
    let grown = footprint(&[
        (1, 0, Some(1)),
        (2, 1, Some(2)),
        (3, 2, Some(3)),
        (4, 3, None),
        (5, 4, None),
    ]);
    footprint_details(&mut plan, "U1", &a, &grown);
    let shrunk = footprint(&[(1, 0, Some(1))]);
    footprint_details(&mut plan, "U1", &a, &shrunk);
    assert_eq!(
        plan.changes,
        vec![
            "U1: footprint array 9 3 → 5 instances; new pads 4, 5 (2 without a package terminal), retired pads none.".to_string(),
            "U1: footprint array 9 3 → 1 instances; new pads none (0 without a package terminal), retired pads 2, 3.".to_string(),
        ]
    );
}

#[test]
fn package_details_name_each_physical_change() {
    use crate::library::{
        ChipForm, ChipKind, GeneratedMaterials, ModelPlacement, PackageForm, PackageModel,
        RoleMaterial,
    };
    use crate::materials::MaterialId;
    let chip = |kind| PackageDefinition {
        form: PackageForm::Chip(Box::new(ChipForm::new(kind, [1.6, 0.8, 0.45]))),
        envelope: [1.6, 0.8, 0.45],
        model: PackageModel::Generated {
            family_rev: 1,
            materials: GeneratedMaterials::default(),
        },
        ..PackageDefinition::default()
    };
    let a = chip(ChipKind::Resistor);
    assert!(package_details(&a, &a).is_empty());
    // A thicker termination: one changed pkg value, nothing else.
    let mut b = a.clone();
    if let PackageForm::Chip(c) = &mut b.form {
        c.termination = 0.4;
    }
    let lines = package_details(&a, &b);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("package parameter pkg_a ") && lines[0].ends_with("→ 0.4 mm."));
    // Another chip kind changes the body's default material too.
    let lines = package_details(&a, &chip(ChipKind::Capacitor));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("chip kind Resistor → Capacitor")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Body material Alumina96 → BariumTitanate")),
        "{lines:?}"
    );
    // A family change, a model source change, a role override, a placement.
    let generic = PackageDefinition {
        envelope: [1.6, 0.8, 0.45],
        ..PackageDefinition::default()
    };
    let lines = package_details(&a, &generic);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("package form Chip → Generic")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("3D model generated")),
        "{lines:?}"
    );
    let mut c = a.clone();
    c.model = PackageModel::Generated {
        family_rev: 1,
        materials: GeneratedMaterials {
            roles: vec![RoleMaterial {
                role: BodyRole::Termination,
                material: MaterialId::Gold,
            }],
        },
    };
    c.placement = ModelPlacement {
        translation: [0.0, 0.0, 0.05],
        rotation_deg: [0.0, 0.0, 90.0],
    };
    let lines = package_details(&a, &c);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("Termination material TinMatte → Gold"));
    assert!(lines[1].starts_with("model placement"));
}

#[test]
fn a_package_change_is_listed_once_for_every_instance_it_reaches() -> Result<()> {
    let Routed {
        mut engine,
        fixture: f,
        r1,
        ..
    } = routed()?;
    let r2 = by_designator(&engine, "R2")?;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.package, package_body, |p| {
            let _ = p.resize_envelope([1.6, 0.8, 0.45]);
        })
    })?;
    let plan = engine.preview_update(Change::ComponentRevision {
        instances: vec![r1, r2],
        target,
        resolutions: Resolutions::default(),
    });
    let body: Vec<&String> = plan
        .changes
        .iter()
        .filter(|c| c.contains(": body "))
        .collect();
    assert_eq!(body.len(), 1, "{:?}", plan.changes);
    assert!(
        body[0].starts_with("R1, R2: body 2.0×1.2×0.6 → 1.6×0.8×0.45 mm"),
        "{body:?}"
    );
    Ok(())
}
