//! Slice C2: the structured review data of an update plan.
use super::*;
use crate::issues::style;

/// Every plan of `routed()` the review shows, with the copper still in place.
fn plans(r: &mut Routed) -> Result<Vec<UpdatePlan>> {
    let f = r.fixture;
    let moved = cascade(&mut r.engine, &f, |e| {
        with(e, f.footprint, footprint_body, |fp| {
            fp.pads[0].position = [3.0, 0.0]
        })
    })?;
    let remapped = cascade(&mut r.engine, &f, |e| {
        with(e, f.binding, binding_body, |b| {
            b.mappings[0].terminal = crate::TerminalId(2);
            b.mappings[1].terminal = crate::TerminalId(1);
        })
    })?;
    let wire = r.engine.project().wires[0].id;
    Ok(vec![
        r.engine.preview_update(revision_change(r.r1, moved)),
        r.engine.preview_update(revision_change(r.r1, remapped)),
        r.engine.preview_update(Change::Schematic {
            command: SchematicCommand::Disconnect { wire },
        }),
        r.engine.preview_update(Change::Instance {
            edit: InstanceChange::Remove { instance: r.r1 },
        }),
        r.engine.preview_update(Change::SyncPcb {}),
        r.engine.preview_update(revision_change(999, moved)),
    ])
}

fn assert_style(what: &str, text: &str) {
    assert_eq!(style::check(text), vec![], "{what}: {text:?}");
}

#[test]
fn every_title_follows_the_message_style_and_mirrors_the_flat_lists() -> Result<()> {
    let mut r = routed()?;
    for plan in plans(&mut r)? {
        assert_style("summary", &plan.summary);
        assert_style("findings", &plan.findings.line);
        assert_eq!(
            plan.changes,
            plan.items
                .iter()
                .map(|c| c.detail.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            plan.blockers,
            plan.blocker_lines
                .iter()
                .map(|l| l.detail.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            plan.warnings,
            plan.warning_lines
                .iter()
                .map(|l| l.detail.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            plan.conflicting_tracks,
            plan.conflicts.iter().map(|c| c.track).collect::<Vec<_>>()
        );
        for change in &plan.items {
            assert_style("change", &change.line);
        }
        for line in plan.blocker_lines.iter().chain(&plan.warning_lines) {
            assert_style("blocker or warning", &line.line);
        }
        for conflict in &plan.conflicts {
            assert_style("conflict", &conflict.line);
            assert_style("ack", &conflict.ack);
        }
        // The raw check dump is gone: findings are a diff, not warnings.
        assert!(
            plan.warnings.iter().all(|w| !w.starts_with("Candidate")),
            "{:#?}",
            plan.warnings
        );
    }
    Ok(())
}

#[test]
fn a_removal_is_listed_by_area_kind_and_object_with_refs() -> Result<()> {
    let Routed {
        engine, r1, shared, ..
    } = routed()?;
    let wire = engine.project().wires[0].clone();
    let plan = engine.preview_update(Change::Instance {
        edit: InstanceChange::Remove { instance: r1 },
    });
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let find = |area, kind, object| {
        plan.items
            .iter()
            .find(|c| c.area == area && c.kind == kind && c.object == object)
            .ok_or_else(|| invalid("change missing"))
    };
    let instance = find(
        ChangeArea::Schematic,
        ChangeKind::Remove,
        ChangeObject::Instance,
    )?;
    assert_eq!(instance.line, "Removes R1");
    assert_eq!(instance.refs, [ObjectRef::Instance { id: r1 }]);
    assert!(
        instance.detail.contains("(ID "),
        "detail keeps the raw text"
    );
    let removed = find(
        ChangeArea::Schematic,
        ChangeKind::Remove,
        ChangeObject::Wire,
    )?;
    assert_eq!(
        removed.refs,
        [
            ObjectRef::Terminal {
                endpoint: wire
                    .from
                    .terminal()
                    .ok_or_else(|| Error::NotFound("terminal end".into()))?
            },
            ObjectRef::Terminal {
                endpoint: wire
                    .to
                    .terminal()
                    .ok_or_else(|| Error::NotFound("terminal end".into()))?
            }
        ]
    );
    assert!(
        removed.line.starts_with("Removes the wire from "),
        "{}",
        removed.line
    );
    let placement = find(
        ChangeArea::Board,
        ChangeKind::Remove,
        ChangeObject::Placement,
    )?;
    assert_eq!(placement.line, "Removes R1 from the board");
    assert!(
        plan.items
            .iter()
            .any(|c| c.area == ChangeArea::Nets && c.object == ChangeObject::Net)
    );
    // The summary names the instance and what goes with it a user knows.
    assert_eq!(plan.summary, "Removes R1, 1 wire and its PCB placement");
    // The shared track then joins R2.1 alone, of its own net: it stays.
    assert!(plan.conflicts.is_empty(), "{:?}", plan.conflicts);
    // Disconnecting the wire puts R1.2 and R2.1 on different nets: the
    // track joining them conflicts, named by net, layer and length.
    let plan = engine.preview_update(Change::Schematic {
        command: SchematicCommand::Disconnect { wire: wire.id },
    });
    let [conflict] = plan.conflicts.as_slice() else {
        return Err(invalid("one conflict expected"));
    };
    let track = engine
        .project()
        .tracks
        .iter()
        .find(|t| t.id == shared)
        .ok_or_else(|| invalid("track"))?;
    assert_eq!(conflict.track, shared);
    assert_eq!(conflict.net, track.net);
    assert_eq!(conflict.net_name, engine.project().net_name(track.net));
    assert_eq!(conflict.layer, Layer::Top);
    assert!(
        (conflict.length_mm - 5.6).abs() < 1e-9,
        "{}",
        conflict.length_mm
    );
    assert_eq!(conflict.reason, ConflictReason::NetChanged);
    assert!(
        matches!(conflict.pad, Some(ObjectRef::Pad { .. })),
        "the pad on the other net is named: {:?}",
        conflict.pad
    );
    assert_eq!(conflict.ack, "Remove that track (Top, 5.6 mm)");
    assert_eq!(
        conflict.line,
        format!(
            "Track on {} would join pads the net no longer joins",
            conflict.net_name
        )
    );
    Ok(())
}

#[test]
fn a_moved_pad_conflict_names_the_pad_of_the_other_net() -> Result<()> {
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
    let [conflict] = plan.conflicts.as_slice() else {
        return Err(invalid("one conflict expected"));
    };
    assert_eq!(conflict.track, shared);
    assert_eq!(conflict.reason, ConflictReason::Clearance);
    let Some(ObjectRef::Pad { instance, .. }) = conflict.pad else {
        return Err(invalid("the conflicting pad is named"));
    };
    assert_eq!(instance, r1);
    assert!(
        conflict.line.contains("would touch R1 pad ") && conflict.line.ends_with("of another net"),
        "{}",
        conflict.line
    );
    // Applying without the acknowledgement names the tracks plainly.
    let Err(Error::Conflict(message)) = engine.apply_update(plan.revision, plan.change, false)
    else {
        return Err(invalid("refused without the acknowledgement"));
    };
    assert_eq!(
        message,
        format!(
            "This update removes 1 conflicting track on {}; confirm their removal to apply it.",
            conflict.net_name
        )
    );
    Ok(())
}

#[test]
fn blockers_have_plain_titles_and_locate_their_objects() -> Result<()> {
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
    for operation in [
        LayoutCommand::SetTrackLock {
            id: shared,
            locked: true,
        },
        LayoutCommand::SetPlacementLock {
            instance: r1,
            locked: true,
        },
    ] {
        apply(&mut engine, Operation::Layout(operation))?;
    }
    let plan = engine.preview_update(revision_change(r1, target));
    let lines: Vec<(&str, &[ObjectRef])> = plan
        .blocker_lines
        .iter()
        .map(|l| (l.line.as_str(), l.refs.as_slice()))
        .collect();
    let net = engine.project().net_name(
        engine
            .project()
            .tracks
            .iter()
            .find(|t| t.id == shared)
            .ok_or_else(|| invalid("track"))?
            .net,
    );
    assert!(
        lines.contains(&(
            "R1 is locked on the board; unlock it to update",
            &[ObjectRef::Instance { id: r1 }][..]
        )),
        "{lines:#?}"
    );
    let locked = format!("A locked track on {net} is affected; unlock it to update");
    assert!(
        lines.contains(&(locked.as_str(), &[ObjectRef::Track { id: shared }][..])),
        "{lines:#?}"
    );
    // A change that cannot be staged is titled by its kind of error and
    // still keeps the engine's wording.
    let bad = engine.preview_update(revision_change(999, target));
    let [line] = bad.blocker_lines.as_slice() else {
        return Err(invalid("one blocker expected"));
    };
    assert_eq!(
        line.line,
        "This update refers to something that no longer exists"
    );
    assert_eq!(line.detail, "Update refers to an unknown instance.");
    assert_eq!(line.refs, [ObjectRef::Instance { id: 999 }]);
    assert_eq!(bad.summary, "No changes");
    Ok(())
}

#[test]
fn findings_diff_counts_new_and_resolved_by_code_and_objects() {
    let pad = |n| ObjectRef::Instance { id: n };
    let finding = |code: &str, message: &str, n| Finding::new(code, message.into(), vec![pad(n)]);
    let diff = review::findings_diff(
        vec![
            finding("board.clearance", "0.1 mm apart", 1),
            finding("board.unrouted", "N2 is unrouted", 2),
            finding("board.unrouted", "N2 is unrouted", 2),
        ],
        vec![
            // Re-measured, not new.
            finding("board.clearance", "0.15 mm apart", 1),
            finding("board.unrouted", "N2 is unrouted", 2),
            finding("erc.conflict", "two outputs", 3),
        ],
    );
    assert_eq!(diff.new, vec![finding("erc.conflict", "two outputs", 3)]);
    assert_eq!(
        diff.resolved,
        vec![finding("board.unrouted", "N2 is unrouted", 2)]
    );
    assert_eq!(diff.line, "1 new issue, 1 resolved");
    assert_eq!(review::findings_diff(vec![], vec![]).line, "No new issues");
}

#[test]
fn a_removal_diff_resolves_every_finding_it_no_longer_has() -> Result<()> {
    let Routed { engine, r1, .. } = routed()?;
    let before = findings(engine.project());
    let (plan, next) = preview(
        engine.project(),
        &engine.copper_graph(),
        engine.revision(),
        Change::Instance {
            edit: InstanceChange::Remove { instance: r1 },
        },
    );
    let mut next = next.ok_or_else(|| invalid("staged"))?;
    next.tracks
        .retain(|t| !plan.conflicting_tracks.contains(&t.id));
    let after = findings(&next);
    let FindingsDiff { new, resolved, .. } = &plan.findings;
    assert_eq!(
        before.len() - resolved.len(),
        after.len() - new.len(),
        "kept findings agree from both sides"
    );
    assert!(
        new.iter()
            .all(|f| !f.objects.contains(&ObjectRef::Instance { id: r1 })),
        "{new:#?}"
    );
    Ok(())
}

#[test]
fn summaries_count_objects_once_and_stay_within_a_title() {
    let change = |kind, object, refs: Vec<ObjectRef>| PlanChange {
        area: ChangeArea::Board,
        kind,
        object,
        refs,
        line: String::new(),
        detail: String::new(),
    };
    let pad = |n| {
        vec![ObjectRef::Pad {
            instance: 1,
            pad: crate::PadId(n),
        }]
    };
    let items = [
        change(
            ChangeKind::Add,
            ChangeObject::Placement,
            vec![ObjectRef::Instance { id: 1 }],
        ),
        change(
            ChangeKind::Add,
            ChangeObject::Placement,
            vec![ObjectRef::Instance { id: 2 }],
        ),
        change(ChangeKind::Remove, ChangeObject::Wire, vec![]),
        // Two aspects of one pad count once.
        change(ChangeKind::Change, ChangeObject::Pad, pad(1)),
        change(ChangeKind::Change, ChangeObject::Pad, pad(1)),
        change(ChangeKind::Change, ChangeObject::Pad, pad(2)),
        change(ChangeKind::Change, ChangeObject::Net, vec![]),
    ];
    assert_eq!(
        summarize(&items),
        "Adds 2 placements, removes 1 wire, changes 1 net, 2 pads"
    );
    let many: Vec<PlanChange> = [
        ChangeObject::Instance,
        ChangeObject::Component,
        ChangeObject::Unit,
        ChangeObject::Pin,
        ChangeObject::Terminal,
        ChangeObject::Function,
        ChangeObject::Field,
        ChangeObject::Part,
        ChangeObject::Package,
        ChangeObject::Footprint,
    ]
    .into_iter()
    .map(|o| change(ChangeKind::Change, o, vec![]))
    .collect();
    let long = summarize(&many);
    assert!(long.ends_with(" and more"), "{long}");
    assert_eq!(style::check(&long), vec![], "{long}");
}

#[test]
fn a_plan_without_the_review_fields_still_reads() -> Result<()> {
    let Routed { engine, r1, .. } = routed()?;
    let plan = engine.preview_update(Change::Instance {
        edit: InstanceChange::Remove { instance: r1 },
    });
    let mut json = serde_json::to_value(&plan)?;
    let object = json
        .as_object_mut()
        .ok_or_else(|| invalid("plan is an object"))?;
    for field in [
        "summary",
        "items",
        "blocker_lines",
        "warning_lines",
        "conflicts",
        "findings",
    ] {
        assert!(object.remove(field).is_some(), "{field}");
    }
    let read: UpdatePlan = serde_json::from_value(json)?;
    assert_eq!(read.changes, plan.changes);
    assert!(read.items.is_empty());
    Ok(())
}

/// Update PCB names the placements it adds, folds their pads into them, and
/// titles what it adds and resolves as Issues does.
#[test]
fn update_pcb_names_its_placements_and_titles_findings_as_issues() -> Result<()> {
    let mut engine = crate::Engine::demo()?;
    let first = engine.project().instances[0].clone();
    for (designator, x) in [("R3", 30.0), ("R4", 34.0), ("R5", 38.0)] {
        engine.apply(
            engine.revision(),
            crate::Operation::Instance(crate::InstanceCommand::Place(
                crate::instances::NewInstance {
                    sheet: None,
                    component: first.component,
                    designator: Some(designator.into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: crate::UnitId(1),
                    position: [x, 0.0],
                    physical: first.physical,
                },
            )),
        )?;
    }
    let plan = engine.preview_update(Change::SyncPcb {});
    assert!(
        plan.summary.starts_with("Adds 3 placements: R3–R5"),
        "{}",
        plan.summary
    );
    assert!(!plan.summary.contains("pad"), "{}", plan.summary);
    assert_eq!(plan.planned_placements.len(), 3);
    let folded: Vec<_> = folded_items(&plan, engine.project()).collect();
    assert!(
        folded.iter().all(|i| i.object != ChangeObject::Pad),
        "{folded:#?}"
    );
    assert_eq!(plan.findings.new_rows.len(), plan.findings.new.len());
    assert_eq!(
        plan.findings.resolved_rows.len(),
        plan.findings.resolved.len()
    );
    for row in plan
        .findings
        .new_rows
        .iter()
        .chain(&plan.findings.resolved_rows)
    {
        assert_style("finding row", &row.title);
    }
    Ok(())
}

#[test]
fn designator_lists_read_as_a_range_or_a_short_list() {
    let names = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        review::designator_list(&names(&["R17", "R18", "R19"])),
        "R17–R19"
    );
    assert_eq!(review::designator_list(&names(&["R1", "R4"])), "R1 and R4");
    assert_eq!(
        review::designator_list(&names(&["R1", "R4", "C2"])),
        "R1, R4 and C2"
    );
    assert_eq!(
        review::designator_list(&names(&["R1", "R4", "C2", "C9"])),
        "R1, R4 and 2 more"
    );
}
