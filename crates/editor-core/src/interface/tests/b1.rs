//! Slice B1: the slot table in the document, the automatic rules, the
//! `interface.*` commands and `interface.list` through the one engine,
//! and persistence (docs/architecture/board-interface.md, "Slices and their
//! tests"). The gate: slots survive undo, redo, save and reopen, and a
//! retired slot is never used again.
use super::Fallible;
use crate::format::{self, EditorFile};
use crate::ids::{ContentHash, Id, InterfaceSourceId};
use crate::interface::report::InterfaceList;
use crate::interface::{DatumGeometry, DatumStatus, ExportTarget, InterfaceCommand};
use crate::library::Outline;
use crate::library::footprint_source::{
    Expr, FootprintParam, ParamName, ParamUnit, SketchBlob, SketchCurveId, SketchPointId, SketchRef,
};
use crate::library::{Segment, datum::DatumName};
use crate::model::board::{
    BoardFeatureId, BoardShape, BoardSketch, CutoutKind, CutoutRole, MountingHoleRole, SolvedBoard,
    SolvedCutout, SolvedMountingHole,
};
use crate::model::dimension::{BoardRef, FeatureRef, FollowedFaceId, PadPoint};
use crate::model::interface::{
    AutoRule, DatumId, DatumSource, PublishedDatum, PublishedRecord, Slot, Tier,
};
use crate::{Engine, Error, InstanceCommand, Operation, Query, QueryResult, schematic};
use std::collections::BTreeSet;

fn list(engine: &Engine) -> Fallible<InterfaceList> {
    match engine.query(Query::InterfaceList {})? {
        QueryResult::InterfaceList(list) => Ok(*list),
        other => Err(format!("not a list: {other:?}").into()),
    }
}

fn apply(engine: &mut Engine, command: InterfaceCommand) -> crate::Result<crate::Outcome> {
    engine.apply(engine.revision(), Operation::Interface(command))
}

fn export(engine: &mut Engine, target: ExportTarget) -> crate::Result<crate::Outcome> {
    apply(engine, InterfaceCommand::Export { target, name: None })
}

/// (id, slot, token) of every live datum, in slot order.
fn slots(engine: &Engine) -> Fallible<Vec<(DatumId, Slot, String)>> {
    Ok(list(engine)?
        .datums
        .into_iter()
        .map(|d| (d.id, d.slot, d.token))
        .collect())
}

fn first_instance(engine: &Engine) -> Fallible<Id> {
    Ok(engine.project().instances.first().ok_or("no part")?.id)
}

fn square(min: [f64; 2], max: [f64; 2]) -> Outline {
    Outline {
        start: min,
        segments: vec![
            Segment::Line {
                to: [max[0], min[1]],
            },
            Segment::Line { to: max },
            Segment::Line {
                to: [min[0], max[1]],
            },
            Segment::Line { to: min },
        ],
    }
}

/// The demo project on a sketched 60 × 40 mm board with one cutout (id 3,
/// its loop curves `l7` to `l10`), two mounting holes (ids 1 and 2) and a
/// `width` parameter, as a solved sketch stores them. Opened as a file, so
/// its interface table is empty until the first change.
fn sketched() -> Fallible<Engine> {
    let demo = Engine::demo()?;
    let mut file = demo.to_file();
    let mut project = file.project.clone();
    project.interface = Default::default();
    let hole = |id, at: [f64; 2]| SolvedMountingHole {
        id: BoardFeatureId(id),
        at,
        drill: 3.2,
        plated: false,
        pad_diameter: None,
    };
    let role = |id, center: &str| MountingHoleRole {
        id: BoardFeatureId(id),
        center: SketchPointId(center.into()),
        drill: Expr("3.2 mm".into()),
        plated: false,
        pad_diameter: None,
    };
    let curves = ["l9", "l7", "l8", "l10"].map(|c| SketchCurveId(c.into()));
    project.board_shape = BoardShape::Sketch(Box::new(BoardSketch {
        evaluator: Default::default(),
        sketch: SketchBlob(
            r#"{"points":[{"id":"p1"},{"id":"p2"}],"curves":[{"id":"l7"},{"id":"l8"},{"id":"l9"},{"id":"l10"}]}"#
                .into(),
        ),
        parameters: vec![FootprintParam {
            name: ParamName("width".into()),
            expr: Expr("60".into()),
            unit: ParamUnit::Mm,
            comment: String::new(),
        }],
        cutouts: vec![CutoutRole {
            id: BoardFeatureId(3),
            kind: CutoutKind::Cutout,
            curves: curves.to_vec(),
        }],
        holes: vec![role(1, "p1"), role(2, "p2")],
        edges: Vec::new(),
        next_feature: 4,
        solve: Default::default(),
        solved: SolvedBoard {
            outer: square([0.0, 0.0], [60.0, 40.0]),
            outer_curves: Vec::new(),
            cutouts: vec![SolvedCutout {
                id: Some(BoardFeatureId(3)),
                kind: CutoutKind::Cutout,
                outline: square([30.0, 20.0], [34.0, 24.0]),
                curves: curves.to_vec(),
            }],
            holes: vec![hole(1, [4.0, 4.0]), hole(2, [56.0, 4.0])],
            edges: Vec::new(),
        },
        outline_fit: 0.01,
        fault: None,
        idle: Vec::new(),
    }));
    project.board_size = [60.0, 40.0];
    file.project = project;
    Ok(Engine::from_file(file)?)
}

/// Save and reopen through the document format.
fn reopened(engine: &Engine) -> Fallible<Engine> {
    let bytes = format::encode(&engine.to_file())?;
    let file: EditorFile = format::decode(&bytes)?;
    Ok(Engine::from_file(file)?)
}

/// A new board exports its frame, top, bottom, thickness, outline, every
/// cutout and mounting hole, and the tallest part per side, each in its own
/// slot, through `interface.list`.
#[test]
fn b1_automatic_rules_export_the_board() -> Fallible {
    let engine = sketched()?;
    let listed = list(&engine)?;
    let tokens: Vec<&str> = listed.datums.iter().map(|d| d.token.as_str()).collect();
    assert_eq!(
        tokens,
        [
            "board.frame",
            "board.top",
            "board.bottom",
            "board.thickness",
            "board.outline",
            "cut3",
            "h1",
            "h2",
            "side.bottom",
            "side.top",
        ]
    );
    let slots: Vec<u32> = listed.datums.iter().map(|d| d.slot.0).collect();
    assert_eq!(
        slots,
        (0..10).collect::<Vec<_>>(),
        "one slot each, in order"
    );
    assert!(
        listed.rules.iter().all(|r| r.on),
        "every rule on by default"
    );
    assert!(listed.retired.is_empty());
    assert_eq!(listed.published, None);
    assert_eq!(listed.changed, 10, "nothing published yet");
    for row in &listed.datums {
        assert_eq!(row.status, DatumStatus::Resolved, "{row:?}");
        let geometry = row.geometry.as_ref().ok_or("geometry")?;
        assert!(geometry.valid(), "{row:?}");
        assert_eq!(geometry.kind(), row.kind, "{row:?}");
        assert!(matches!(row.source, DatumSource::Auto { .. }));
    }
    let t = engine.project().stackup.thickness_mm();
    let by = |token: &str| listed.datums.iter().find(|d| d.token == token);
    assert_eq!(
        by("h2").and_then(|d| d.geometry.clone()),
        Some(DatumGeometry::Axis {
            point: crate::interface::Vec3([56.0, 4.0, 0.0]),
            direction: crate::interface::Vec3([0.0, 0.0, 1.0]),
            diameter: Some(3.2),
        })
    );
    assert_eq!(
        by("board.thickness").and_then(|d| d.geometry.clone()),
        Some(DatumGeometry::Parameter {
            value: t,
            unit: "mm".into()
        })
    );
    assert_eq!(by("h1").map(|d| d.name.as_str()), Some("Mounting hole 1"));
    assert_eq!(by("cut3").map(|d| d.name.as_str()), Some("Cutout 1"));
    // The tallest top part is the demo's two-terminal body.
    let Some(DatumGeometry::Volume { height, .. }) =
        by("side.top").and_then(|d| d.geometry.clone())
    else {
        return Err("side.top is a volume".into());
    };
    assert!(height > 0.0, "{height}");

    // Listing stored nothing; the first change stores exactly what was
    // listed.
    assert!(engine.project().interface.is_empty());
    let mut engine = engine;
    let part = first_instance(&engine)?;
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    let after = list(&engine)?;
    assert_eq!(after.datums[..10], listed.datums[..10]);
    assert_eq!(after.datums[10].slot, Slot(10));
    Ok(())
}

/// Switching a rule off retires the slots of its datums; on again
/// allocates new slots, never the retired ones.
#[test]
fn b1_switching_a_rule_retires_and_never_reuses() -> Fallible {
    let mut engine = sketched()?;
    let before = slots(&engine)?;
    let holes: Vec<Slot> = before
        .iter()
        .filter(|(_, _, t)| t.starts_with('h'))
        .map(|(_, s, _)| *s)
        .collect();
    assert_eq!(holes, [Slot(6), Slot(7)]);
    let revision = engine.revision();
    apply(
        &mut engine,
        InterfaceCommand::Auto {
            rule: AutoRule::Holes,
            on: false,
        },
    )?;
    assert_eq!(engine.revision(), revision + 1, "one undo step");
    let off = list(&engine)?;
    assert_eq!(off.retired, holes);
    assert!(off.datums.iter().all(|d| !d.token.starts_with('h')));
    assert!(off.rules.iter().any(|r| r.rule == AutoRule::Holes && !r.on));
    // Off again changes nothing and is no step.
    let revision = engine.revision();
    apply(
        &mut engine,
        InterfaceCommand::Auto {
            rule: AutoRule::Holes,
            on: false,
        },
    )?;
    assert_eq!(engine.revision(), revision);

    apply(
        &mut engine,
        InterfaceCommand::Auto {
            rule: AutoRule::Holes,
            on: true,
        },
    )?;
    let on = list(&engine)?;
    let again: Vec<(Slot, &str)> = on
        .datums
        .iter()
        .filter(|d| d.token.starts_with('h'))
        .map(|d| (d.slot, d.token.as_str()))
        .collect();
    assert_eq!(again, [(Slot(10), "h1"), (Slot(11), "h2")]);
    assert_eq!(on.retired, holes, "the retired slots stay retired");
    let old_ids: BTreeSet<DatumId> = before.iter().map(|(id, _, _)| *id).collect();
    assert!(
        on.datums
            .iter()
            .filter(|d| d.token.starts_with('h'))
            .all(|d| !old_ids.contains(&d.id)),
        "new ids too"
    );
    Ok(())
}

/// `interface.export` of a part face, a pad, a board curve, a part frame
/// and envelope and a board parameter allocates one slot each and is one
/// undo step each; exporting the same thing twice is refused unchanged.
/// (A part face needs the host's model trace; the session test over the
/// ESP32 board exports one.)
#[test]
fn b1_export_allocates_one_slot_per_datum() -> Fallible {
    let mut engine = sketched()?;
    let part = first_instance(&engine)?;
    let pad = crate::layout::pads(engine.project())
        .into_iter()
        .find(|p| p.instance == part)
        .ok_or("a pad")?
        .pad;
    let targets = [
        ExportTarget::Feature {
            feature: BoardRef::Part(FeatureRef::Pad {
                placement: part,
                pad,
                at: PadPoint::Centre,
            }),
        },
        ExportTarget::Feature {
            feature: BoardRef::Board(SketchRef::Curve(SketchCurveId("l8".into()))),
        },
        ExportTarget::PartFrame { instance: part },
        ExportTarget::PartEnvelope { instance: part },
        ExportTarget::BoardParameter {
            name: ParamName("width".into()),
        },
    ];
    for (n, target) in targets.iter().enumerate() {
        let revision = engine.revision();
        let undo = engine.can_undo();
        export(&mut engine, target.clone())?;
        assert_eq!(engine.revision(), revision + 1);
        assert!(engine.can_undo() || undo);
        let listed = list(&engine)?;
        let last = listed.datums.last().ok_or("a datum")?;
        assert_eq!(last.slot, Slot(10 + n as u32), "{target:?}");
        assert_eq!(last.source, DatumSource::User);
        assert_eq!(last.status, DatumStatus::Resolved, "{last:?}");
    }
    let listed = list(&engine)?;
    let tail: Vec<&str> = listed.datums[10..]
        .iter()
        .map(|d| d.token.as_str())
        .collect();
    let pad_token = format!("d{}", listed.datums[10].id.0);
    let curve_token = format!("d{}", listed.datums[11].id.0);
    let frame = format!("i{part}.frame");
    let envelope = format!("i{part}.envelope");
    assert_eq!(
        tail,
        [
            pad_token.as_str(),
            curve_token.as_str(),
            frame.as_str(),
            envelope.as_str(),
            "board.parameter.width"
        ]
    );
    let geometry = |i: usize| listed.datums[i].geometry.clone();
    assert!(matches!(geometry(10), Some(DatumGeometry::Axis { .. })));
    assert!(matches!(geometry(12), Some(DatumGeometry::Frame { .. })));
    assert!(matches!(geometry(13), Some(DatumGeometry::Volume { .. })));
    assert_eq!(
        geometry(14),
        Some(DatumGeometry::Parameter {
            value: 60.0,
            unit: "mm".into()
        })
    );
    // One undo takes back exactly the last export.
    engine.undo()?;
    assert_eq!(list(&engine)?.datums.len(), 14);
    engine.redo()?;
    // Twice is refused, and nothing changes.
    for target in targets {
        let revision = engine.revision();
        let project = engine.project_arc().clone();
        let refused = export(&mut engine, target);
        assert!(
            matches!(&refused, Err(Error::Conflict(m)) if m.starts_with("interface.exported")),
            "{refused:?}"
        );
        assert_eq!(engine.revision(), revision);
        assert!(std::sync::Arc::ptr_eq(&project, engine.project_arc()));
    }
    Ok(())
}

/// Exporting a followed face, a missing instance or an unknown package
/// datum is refused and leaves document, revision and history unchanged.
#[test]
fn b1_refused_exports_change_nothing() -> Fallible {
    let mut engine = sketched()?;
    let part = first_instance(&engine)?;
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    let refusals = [
        ExportTarget::Feature {
            feature: BoardRef::FollowedFace(FollowedFaceId(1)),
        },
        ExportTarget::PartFrame { instance: 999_999 },
        ExportTarget::PartEnvelope { instance: 999_999 },
        ExportTarget::PackageDatum {
            instance: part,
            name: DatumName("opening".into()),
        },
        ExportTarget::PackageDatum {
            instance: 999_999,
            name: DatumName("opening".into()),
        },
        ExportTarget::BoardParameter {
            name: ParamName("depth".into()),
        },
        ExportTarget::Feature {
            feature: BoardRef::Board(SketchRef::Curve(SketchCurveId("l99".into()))),
        },
    ];
    for target in refusals {
        let revision = engine.revision();
        let project = engine.project_arc().clone();
        let undo = engine.undo_step();
        let refused = export(&mut engine, target.clone());
        assert!(refused.is_err(), "{target:?}");
        assert_eq!(engine.revision(), revision, "{target:?}");
        assert!(std::sync::Arc::ptr_eq(&project, engine.project_arc()));
        assert_eq!(engine.undo_step(), undo);
    }
    // A blank display name and the other commands' refusals too.
    let revision = engine.revision();
    let bad = [
        InterfaceCommand::Export {
            target: ExportTarget::PartEnvelope { instance: part },
            name: Some("  ".into()),
        },
        InterfaceCommand::Unexport {
            datum: DatumId(999_999),
        },
        InterfaceCommand::Rename {
            datum: DatumId(999_999),
            name: None,
        },
    ];
    for command in bad {
        assert!(apply(&mut engine, command).is_err());
        assert_eq!(engine.revision(), revision);
    }
    // An automatic datum is withdrawn by its rule, not by unexport.
    let frame = list(&engine)?.datums[0].id;
    let refused = apply(&mut engine, InterfaceCommand::Unexport { datum: frame });
    assert!(matches!(&refused, Err(Error::Conflict(m)) if m.starts_with("interface.automatic")));
    assert_eq!(engine.revision(), revision);
    Ok(())
}

/// `interface.unexport` retires the slot; a later export takes a new one.
#[test]
fn b1_unexport_retires_the_slot() -> Fallible {
    let mut engine = sketched()?;
    let part = first_instance(&engine)?;
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    let exported = list(&engine)?.datums.last().cloned().ok_or("exported")?;
    assert_eq!(exported.slot, Slot(10));
    apply(
        &mut engine,
        InterfaceCommand::Unexport { datum: exported.id },
    )?;
    let after = list(&engine)?;
    assert_eq!(after.retired, [Slot(10)]);
    assert!(after.datums.iter().all(|d| d.id != exported.id));
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    let again = list(&engine)?.datums.last().cloned().ok_or("again")?;
    assert_eq!(again.slot, Slot(11));
    assert_ne!(again.id, exported.id);
    assert_eq!(again.token, exported.token, "the token is built from ids");
    Ok(())
}

/// `interface.rename` and renaming the part (J1 → J7) change the display
/// name only: id, slot and token stay.
#[test]
fn b1_renames_change_the_display_name_only() -> Fallible {
    let mut engine = sketched()?;
    let part = first_instance(&engine)?;
    let designator = engine
        .project()
        .instance(part)
        .ok_or("part")?
        .designator
        .clone();
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    let row = list(&engine)?.datums.last().cloned().ok_or("row")?;
    assert_eq!(row.name, format!("{designator} frame"));
    let hash = crate::interface::list::datum_hash(&row)?;

    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::SetDesignator {
            instance: part,
            designator: "J7".into(),
        }),
    )?;
    let renamed = list(&engine)?.datums.last().cloned().ok_or("row")?;
    assert_eq!(renamed.name, "J7 frame");
    assert_eq!(
        (renamed.id, renamed.slot, &renamed.token),
        (row.id, row.slot, &row.token)
    );
    assert_eq!(crate::interface::list::datum_hash(&renamed)?, hash);

    apply(
        &mut engine,
        InterfaceCommand::Rename {
            datum: row.id,
            name: Some("Mounting reference".into()),
        },
    )?;
    let named = list(&engine)?.datums.last().cloned().ok_or("row")?;
    assert_eq!(named.name, "Mounting reference");
    assert_eq!(
        (named.id, named.slot, &named.token),
        (row.id, row.slot, &row.token)
    );
    // The same name again is no step; no name returns to the derived one.
    let revision = engine.revision();
    apply(
        &mut engine,
        InterfaceCommand::Rename {
            datum: row.id,
            name: Some("Mounting reference".into()),
        },
    )?;
    assert_eq!(engine.revision(), revision);
    apply(
        &mut engine,
        InterfaceCommand::Rename {
            datum: row.id,
            name: None,
        },
    )?;
    assert_eq!(list(&engine)?.datums.last().ok_or("row")?.name, "J7 frame");
    Ok(())
}

/// Slots, retired slots and ids are the same after undo, redo, save and
/// reopen; undoing an export does not give its slot to the next export.
#[test]
fn b1_slots_survive_undo_redo_save_and_reopen() -> Fallible {
    let mut engine = sketched()?;
    let part = first_instance(&engine)?;
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    export(&mut engine, ExportTarget::PartEnvelope { instance: part })?;
    let two = slots(&engine)?;

    // Undo the envelope: its slot is retired, not free.
    engine.undo()?;
    let one = list(&engine)?;
    assert_eq!(one.datums.len(), 11);
    assert_eq!(one.retired, [Slot(11)]);
    // Redo brings it back in the same slot, with the same id.
    engine.redo()?;
    assert_eq!(slots(&engine)?, two);
    assert!(list(&engine)?.retired.is_empty());

    // Undo it again and export something else: a new slot and id.
    engine.undo()?;
    export(
        &mut engine,
        ExportTarget::BoardParameter {
            name: ParamName("width".into()),
        },
    )?;
    let after = list(&engine)?;
    let added = after.datums.last().ok_or("added")?;
    assert_eq!(added.slot, Slot(12));
    assert!(two.iter().all(|(id, _, _)| *id != added.id), "a fresh id");
    assert_eq!(after.retired, [Slot(11)]);

    // Undo everything, the board's own datums included: they keep their
    // slots, and every slot handed out is retired.
    while engine.can_undo() {
        engine.undo()?;
    }
    let undone = list(&engine)?;
    let board: Vec<(DatumId, Slot, String)> = two[..10].to_vec();
    assert_eq!(
        undone
            .datums
            .iter()
            .map(|d| (d.id, d.slot, d.token.clone()))
            .collect::<Vec<_>>(),
        board
    );
    assert_eq!(undone.retired, [Slot(10), Slot(11), Slot(12)]);
    while engine.can_redo() {
        engine.redo()?;
    }
    let redone = list(&engine)?;
    assert_eq!(redone, after);

    // Save and reopen: the same table, and the counters keep going.
    let mut opened = reopened(&engine)?;
    assert_eq!(opened.project().interface, engine.project().interface);
    assert_eq!(list(&opened)?, redone);
    export(&mut opened, ExportTarget::PartEnvelope { instance: part })?;
    let next = list(&opened)?;
    let last = next.datums.last().ok_or("last")?;
    assert_eq!(last.slot, Slot(13));
    assert!(next.retired.contains(&Slot(11)));
    Ok(())
}

/// Deleting the part an exported datum belongs to reports the datum as
/// lost (`interface.datum_lost`) and keeps its slot until it is
/// unexported.
#[test]
fn b1_a_deleted_owner_makes_the_datum_lost() -> Fallible {
    let mut engine = sketched()?;
    let part = first_instance(&engine)?;
    let designator = engine
        .project()
        .instance(part)
        .ok_or("part")?
        .designator
        .clone();
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    let exported = list(&engine)?.datums.last().cloned().ok_or("row")?;
    engine.apply_update(
        engine.revision(),
        crate::updates::Change::Instance {
            edit: crate::updates::InstanceChange::Remove { instance: part },
        },
        false,
    )?;
    assert!(engine.project().instance(part).is_none());
    let lost = list(&engine)?;
    let row = lost
        .datums
        .iter()
        .find(|d| d.id == exported.id)
        .ok_or("the datum stays")?;
    assert_eq!(row.slot, exported.slot);
    assert_eq!(row.status, DatumStatus::Lost);
    assert_eq!(row.geometry, None);
    assert!(lost.retired.is_empty());
    let issues = crate::issues::report(
        engine.project(),
        engine.drafts(),
        &crate::issues::IssueScope::All,
    )?;
    let finding = issues
        .rows
        .iter()
        .flat_map(|r| std::iter::once(&r.finding).chain(r.also.iter()))
        .find(|f| f.code == crate::interface::findings::DATUM_LOST)
        .ok_or("interface.datum_lost")?;
    assert!(
        finding.message.starts_with("A deleted part's frame"),
        "{finding:?}"
    );
    assert!(!finding.message.contains(&designator));

    apply(
        &mut engine,
        InterfaceCommand::Unexport { datum: exported.id },
    )?;
    assert_eq!(list(&engine)?.retired, [exported.slot]);
    Ok(())
}

/// Selecting, hovering and moving the camera never touch the table: they
/// are view state, so the engine sees only queries, and no query changes
/// the document, its revision or its history.
#[test]
fn b1_selection_never_changes_the_interface() -> Fallible {
    let mut engine = sketched()?;
    let part = first_instance(&engine)?;
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    let project = engine.project_arc().clone();
    let revision = engine.revision();
    let undo = engine.undo_step();
    for _ in 0..3 {
        list(&engine)?;
        engine.query(Query::BoardShape {})?;
        engine.query(Query::Issues {
            scope: crate::issues::IssueScope::All,
        })?;
    }
    assert!(std::sync::Arc::ptr_eq(&project, engine.project_arc()));
    assert_eq!(engine.revision(), revision);
    assert_eq!(engine.undo_step(), undo);
    Ok(())
}

/// A change that does not touch the board keeps the interface section
/// shared with the step before it (no copy per edit), and a document that
/// never had an interface saves none until a change stores it.
#[test]
fn unrelated_edits_leave_the_table_shared() -> Fallible {
    let mut engine = sketched()?;
    assert!(
        !String::from_utf8(format::encode(&engine.to_file())?)?.contains("\"interface\""),
        "an untouched document writes no interface"
    );
    let part = first_instance(&engine)?;
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    let before = engine.project().interface.clone();
    engine.apply(
        engine.revision(),
        Operation::Schematic(schematic::SchematicCommand::RenameProject {
            name: "Renamed".into(),
        }),
    )?;
    assert!(crate::shared::Shared::ptr_eq(
        &before,
        &engine.project().interface
    ));
    Ok(())
}

/// `interface.record_publish` stores the host's record: revision 1 first,
/// the same revision for the same hash, the next for a new one; another
/// source id, a skipped revision or a datum that is not live is refused.
/// After it, `interface.list` reads unchanged datums as unchanged.
#[test]
fn recording_a_publish_marks_what_changed_since() -> Fallible {
    let mut engine = sketched()?;
    let listed = list(&engine)?;
    let datums: Vec<PublishedDatum> = listed
        .datums
        .iter()
        .map(|d| {
            Ok(PublishedDatum {
                id: d.id,
                slot: d.slot,
                hash: crate::interface::list::datum_hash(d)?,
            })
        })
        .collect::<crate::Result<_>>()?;
    let source = InterfaceSourceId(7);
    let record = |revision, hash: u8| PublishedRecord {
        revision,
        hash: ContentHash([hash; 32]),
        tier: Tier::Interface,
        datums: datums.clone(),
        design_sha256: None,
        earlier: Vec::new(),
        design_hint: None,
    };
    let publish = |engine: &mut Engine, source_id, record| {
        apply(
            engine,
            InterfaceCommand::RecordPublish { source_id, record },
        )
    };
    assert!(
        publish(&mut engine, source, record(2, 1)).is_err(),
        "starts at 1"
    );
    publish(&mut engine, source, record(1, 1))?;
    let published = list(&engine)?;
    assert_eq!(published.published, Some(1));
    assert_eq!(published.source_id, Some(source));
    assert_eq!(published.changed, 0);
    assert!(published.datums.iter().all(|d| !d.changed));

    assert!(
        publish(&mut engine, InterfaceSourceId(8), record(2, 2)).is_err(),
        "another identity"
    );
    assert!(
        publish(&mut engine, source, record(3, 2)).is_err(),
        "skips 2"
    );
    let revision = engine.revision();
    publish(&mut engine, source, record(1, 1))?;
    assert_eq!(engine.revision(), revision, "the same publish again");
    let mut stale = record(2, 2);
    stale.datums.push(PublishedDatum {
        id: DatumId(999),
        slot: Slot(99),
        hash: ContentHash([0; 32]),
    });
    assert!(publish(&mut engine, source, stale).is_err(), "not live");

    // A new export is the one change since revision 1.
    let part = first_instance(&engine)?;
    export(&mut engine, ExportTarget::PartFrame { instance: part })?;
    let after = list(&engine)?;
    assert_eq!(after.changed, 1);
    assert_eq!(
        after.datums.iter().filter(|d| d.changed).count(),
        1,
        "only the new datum"
    );
    let finding = crate::interface::findings::unpublished(after.changed, after.published);
    assert_eq!(finding.message, "1 datum changed since revision 1");
    Ok(())
}

/// An undo restores a hole the board needs with the slot it had, and the
/// redo that deletes it again retires that slot again; neither renumbers
/// anything else (the table's own rules, without a board evaluator).
#[test]
fn undo_restores_an_automatic_datum_in_its_slot() -> Fallible {
    let engine = sketched()?;
    let mut board = engine.project().clone();
    let stored = crate::interface::auto::reconcile(&board.interface, &board)?.ok_or("stored")?;
    board.interface = stored.clone().into();
    // The hole goes: its slot is retired.
    let mut without = board.clone();
    if let BoardShape::Sketch(sketch) = &mut without.board_shape {
        sketch.holes.retain(|h| h.id != BoardFeatureId(1));
        sketch.solved.holes.retain(|h| h.id != BoardFeatureId(1));
    }
    let followed = crate::interface::auto::follow(&board, &without)?.ok_or("followed")?;
    assert_eq!(followed.retired, [Slot(6)]);
    assert!(followed.datums.iter().all(|d| d.slot != Slot(6)));
    // Undo to the board with the hole: it is live again in slot 6.
    let carried = crate::interface::auto::carry(&followed, &board)?;
    assert_eq!(carried, None, "nothing to carry: the counters match");
    // Undo past a datum allocated later: its slot is retired, the hole
    // keeps its own.
    let mut later = followed.clone();
    crate::interface::allocate_slot(&mut later)?;
    later.next_datum += 1;
    let mut restored = board.clone();
    restored.interface = stored.clone().into();
    let carried = crate::interface::auto::carry(&later, &restored)?.ok_or("carried")?;
    assert!(carried.datums.iter().any(|d| d.slot == Slot(6)));
    assert_eq!(carried.retired, [Slot(10)]);
    assert_eq!(carried.next_slot, 11);
    Ok(())
}
