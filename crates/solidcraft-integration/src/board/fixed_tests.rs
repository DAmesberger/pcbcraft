//! Fixed geometry an assembly link owns in the board sketch (slice B4,
//! `interface::assembly_link`): a followed face's trace enters the sketch as
//! a construction line between fixed points; a board edge dimensioned to
//! it follows when an update moves it; a trace that changed shape is lost
//! and keeps its last place; removing the reference removes the line and
//! the dimension to it. Slice B5: a board edit never moves that line,
//! and a loop between the board and the linked design is found and blocks
//! fabrication output.
use super::tests::{Fallible, apply, created_curves_of, edit, sketch, sketched};
use pcbcraft_core::fabrication::preflight;
use pcbcraft_core::ids::ContentHash;
use pcbcraft_core::interface::assembly_link::{
    self, AssemblyCommand, AssemblyRead, FileCheck, LinkState, NewReference, ReferenceRead,
};
use pcbcraft_core::interface::findings;
use pcbcraft_core::issues::Severity;
use pcbcraft_core::layout::outline::BoardEdit;
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, SketchConstraintId, SketchCurveId, SketchEdit, SketchPointId, SketchRef,
};
use pcbcraft_core::model::assembly_link::{Consumer, UseKind};
use pcbcraft_core::model::dimension::{
    BodyName, FaceName, FeatureGeometry, FollowedFaceId, ReferenceStatus,
};
use pcbcraft_core::model::interface::DatumRole;
use pcbcraft_core::{Engine, Operation};

fn read(references: Vec<ReferenceRead>) -> AssemblyRead {
    AssemblyRead {
        assembly_hint: "assembly.solidcraft".into(),
        design_hint: "board.solidcraft".into(),
        component: "Board".into(),
        file_sha256: ContentHash([1; 32]),
        board_revision: None,
        references,
        bindings: Vec::new(),
        consumers: Vec::new(),
        board_sha256: None,
        unexported: Vec::new(),
    }
}

fn line(y: f64, width: f64) -> FeatureGeometry {
    FeatureGeometry::Line {
        a: [-5.0, y],
        b: [width + 5.0, y],
    }
}

fn top(engine: &Engine) -> Fallible<f64> {
    let s = sketch(engine)?;
    let e = s
        .solved
        .edges
        .iter()
        .find(|e| e.name.0 == "top")
        .ok_or("top")?;
    Ok(e.a[1])
}

/// The sketch's ids that `pick` chooses from its canonical JSON.
fn find(
    engine: &Engine,
    list: &str,
    pick: impl Fn(&serde_json::Value) -> bool,
) -> Fallible<String> {
    let s = sketch(engine)?;
    let json: serde_json::Value = serde_json::from_str(&s.sketch.0)?;
    Ok(json[list]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| pick(v))
        .and_then(|v| v["id"].as_str())
        .ok_or(list.to_owned())?
        .to_owned())
}

#[test]
fn a_board_edge_follows_the_fixed_line_a_reference_owns() -> Fallible<()> {
    let mut engine = sketched()?;
    let [width, height] = engine.project().board_size;
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordLink {
            read: read(Vec::new()),
        }),
    )?;
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordReference {
            reference: NewReference {
                body: BodyName("Far wall".into()),
                face: FaceName("F1:side:e1".into()),
                pick: [0.0, height + 1.0, 0.0],
                name: "Far wall · face".into(),
                geometry: line(height + 1.0, width),
                built_on: Vec::new(),
            },
        }),
    )?;
    let owned = engine
        .project()
        .assembly_link
        .as_ref()
        .and_then(|l| l.references.first())
        .map(|r| r.sketch.clone())
        .ok_or("the reference owns geometry")?;
    let SketchRef::Curve(curve) = owned.clone() else {
        return Err("a line".into());
    };
    // The fixed line cannot be dragged by the sketch's own edits.
    let point = find(&engine, "points", |p| {
        p["fixed"] == true && p["id"] != "origin"
    })?;
    assert!(
        edit(
            &mut engine,
            vec![BoardEdit::Sketch {
                edit: SketchEdit::Drag {
                    point: SketchPointId(point),
                    to: [0.0, 0.0],
                },
            }],
        )
        .is_err()
    );
    // The height gives way to 1 mm from the top-right corner to the line.
    let height_k = find(&engine, "constraints", |k| k["param"] == "height")?;
    let corner = find(&engine, "points", |p| {
        p["pos"][0].as_f64() == Some(width) && p["pos"][1].as_f64() == Some(height)
    })?;
    edit(
        &mut engine,
        vec![
            BoardEdit::Sketch {
                edit: SketchEdit::Delete {
                    ids: vec![SketchRef::Constraint(SketchConstraintId(height_k))],
                },
            },
            BoardEdit::Sketch {
                edit: SketchEdit::AddDimension {
                    kind: DimKind::Distance,
                    refs: vec![
                        SketchRef::Point(SketchPointId(corner)),
                        SketchRef::Curve(curve),
                    ],
                    value: DimValue::Fixed(1.0),
                },
            },
        ],
    )?;
    assert!((top(&engine)? - height).abs() <= 1e-9);

    // The wall moves 2 mm: the edge follows.
    let moved = ReferenceRead {
        id: FollowedFaceId(1),
        status: ReferenceStatus::Resolved,
        pick: Some([0.0, height + 3.0, 0.0]),
        geometry: Some(line(height + 3.0, width)),
        note: String::new(),
        built_on: Vec::new(),
    };
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: read(vec![moved]),
            resolve: Vec::new(),
        }),
    )?;
    assert!((top(&engine)? - (height + 2.0)).abs() <= 1e-9);
    assert!((engine.project().board_size[1] - (height + 2.0)).abs() <= 1e-9);

    // A trace that changed shape is lost and keeps its last place.
    let changed = ReferenceRead {
        id: FollowedFaceId(1),
        status: ReferenceStatus::Resolved,
        pick: None,
        geometry: Some(FeatureGeometry::Point { at: [0.0, 0.0] }),
        note: String::new(),
        built_on: Vec::new(),
    };
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: read(vec![changed]),
            resolve: Vec::new(),
        }),
    )?;
    let reference = engine
        .project()
        .assembly_link
        .as_ref()
        .and_then(|l| l.references.first().cloned())
        .ok_or("kept")?;
    assert_eq!(reference.status, ReferenceStatus::Lost);
    assert_eq!(reference.geometry, line(height + 3.0, width));
    assert!((top(&engine)? - (height + 2.0)).abs() <= 1e-9);

    // Removed: the line and the dimension to it go; the board keeps its
    // shape.
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RemoveReference {
            id: FollowedFaceId(1),
        }),
    )?;
    let s = sketch(&engine)?;
    assert!(!s.sketch.0.contains(&format!(
        "\"{}\"",
        match &owned {
            SketchRef::Curve(c) => c.0.as_str(),
            _ => "",
        }
    )));
    assert!((top(&engine)? - (height + 2.0)).abs() <= 1e-9);
    Ok(())
}

// Slice B5: the two-way check on the board side.

/// The demo board sketched and linked, its top edge held 1 mm below the
/// fixed line a followed face owns; the reference's linked design geometry
/// is built on `built_on`. Returns the engine and the owned line.
fn followed(built_on: Vec<Consumer>) -> Fallible<(Engine, SketchCurveId)> {
    followed_on(sketched()?, built_on)
}

/// [`followed`] on `engine`, a sketched board.
fn followed_on(mut engine: Engine, built_on: Vec<Consumer>) -> Fallible<(Engine, SketchCurveId)> {
    let [width, height] = engine.project().board_size;
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordLink {
            read: read(Vec::new()),
        }),
    )?;
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordReference {
            reference: NewReference {
                body: BodyName("Far wall".into()),
                face: FaceName("F1:side:e1".into()),
                pick: [0.0, height + 1.0, 0.0],
                name: "Far wall · face".into(),
                geometry: line(height + 1.0, width),
                built_on,
            },
        }),
    )?;
    let owned = engine
        .project()
        .assembly_link
        .as_ref()
        .and_then(|l| l.references.first())
        .map(|r| r.sketch.clone())
        .ok_or("the reference owns geometry")?;
    let SketchRef::Curve(curve) = owned else {
        return Err("a line".into());
    };
    let height_k = find(&engine, "constraints", |k| k["param"] == "height")?;
    let corner = find(&engine, "points", |p| {
        p["pos"][0].as_f64() == Some(width) && p["pos"][1].as_f64() == Some(height)
    })?;
    edit(
        &mut engine,
        vec![
            BoardEdit::Sketch {
                edit: SketchEdit::Delete {
                    ids: vec![SketchRef::Constraint(SketchConstraintId(height_k))],
                },
            },
            BoardEdit::Sketch {
                edit: SketchEdit::AddDimension {
                    kind: DimKind::Distance,
                    refs: vec![
                        SketchRef::Point(SketchPointId(corner)),
                        SketchRef::Curve(curve.clone()),
                    ],
                    value: DimValue::Fixed(1.0),
                },
            },
        ],
    )?;
    assert!((top(&engine)? - height).abs() <= 1e-9);
    Ok((engine, curve))
}

/// The board's datum with `role`, as a linked design feature would use it.
fn uses(engine: &Engine, role: DatumRole, feature: &str) -> Fallible<Consumer> {
    let table = pcbcraft_core::interface::list::table(engine.project())?;
    let datum = table
        .datums
        .iter()
        .find(|d| d.key.role == role)
        .ok_or("the board exports it")?;
    let row = pcbcraft_core::interface::list::rows(engine.project(), &table)?
        .into_iter()
        .find(|r| r.id == datum.id)
        .ok_or("a row")?;
    Ok(Consumer {
        feature: feature.into(),
        datum: datum.id,
        slot: datum.slot,
        via: UseKind::Sketch,
        token: row.token,
        name: row.name,
    })
}

/// Conflict row "the board's driven outline was edited locally": a board
/// edit that deletes or moves the fixed line the linked design drives, or one
/// of its ends, is refused with `assembly.driven_edge` in the user's words,
/// changing nothing; an edit that leaves it alone is applied.
#[test]
fn b5_a_board_edit_never_moves_what_the_linked_design_drives() -> Fallible<()> {
    let (mut engine, curve) = followed(Vec::new())?;
    let before = engine.project().clone();
    let revision = engine.revision();
    let ends: Vec<String> = {
        let s = sketch(&engine)?;
        let json: serde_json::Value = serde_json::from_str(&s.sketch.0)?;
        let line = json["curves"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c["id"] == curve.0.as_str())
            .ok_or("the owned line")?
            .clone();
        let points = json["points"].as_array().cloned().unwrap_or_default();
        ["a", "b"]
            .iter()
            .filter_map(|end| {
                let index = line[end].as_u64()?;
                points.get(usize::try_from(index).ok()?)?["id"]
                    .as_str()
                    .map(str::to_owned)
            })
            .collect()
    };
    assert_eq!(ends.len(), 2, "the line's two fixed ends");
    let mut attempts = vec![
        SketchEdit::Delete {
            ids: vec![SketchRef::Curve(curve.clone())],
        },
        SketchEdit::Move {
            ids: vec![SketchRef::Curve(curve.clone())],
            delta: [0.0, 2.0],
        },
    ];
    for end in &ends {
        attempts.push(SketchEdit::Delete {
            ids: vec![SketchRef::Point(SketchPointId(end.clone()))],
        });
        attempts.push(SketchEdit::Drag {
            point: SketchPointId(end.clone()),
            to: [0.0, 0.0],
        });
    }
    for attempt in attempts {
        let refused = edit(
            &mut engine,
            vec![BoardEdit::Sketch {
                edit: attempt.clone(),
            }],
        );
        let Err(error) = refused else {
            return Err(format!("{attempt:?} was applied").into());
        };
        assert_eq!(
            error.rule(),
            Some(findings::ASSEMBLY_DRIVEN_EDGE),
            "{attempt:?}: {error}"
        );
        assert_eq!(
            error.to_string(),
            "This edge follows the linked design's wall. Break the link to edit it here",
            "{attempt:?}"
        );
        assert_eq!(engine.revision(), revision, "{attempt:?}");
        assert_eq!(*engine.project(), before, "{attempt:?}");
    }
    // An edit that leaves the line alone is the board's own.
    edit(&mut engine, vec![super::tests::set("width", "52 mm")])?;
    assert_eq!(engine.revision(), revision + 1);
    Ok(())
}

/// Conflict row "a cycle", on the board side of the graph: the top edge
/// follows the linked design's far wall, which the linked design builds from the
/// board outline. The outline moves when the wall does, so the loop is
/// found, named in both files' words, blocks fabrication output with no
/// waiver and reads in the link's status; the board's top face, which does
/// not move with the wall, is no loop; stopping following the wall ends
/// it, and undo brings it back.
#[test]
fn b5_a_loop_between_board_and_linked_design_blocks_fabrication() -> Fallible<()> {
    let engine = sketched()?;
    let outline = uses(&engine, DatumRole::BoardOutline, "Far wall")?;
    let top_face = uses(&engine, DatumRole::BoardTop, "Lid")?;
    let (mut engine, _) = followed(vec![outline.clone(), top_face])?;
    let link = engine.project().assembly_link.as_deref().ok_or("linked")?;
    let [cycle] = link.cycles.as_slice() else {
        return Err(format!("one loop: {:?}", link.cycles).into());
    };
    assert_eq!(cycle.datum, outline.datum);
    assert_eq!(cycle.feature, "Far wall");
    let found: Vec<_> = assembly_link::checks(engine.project())
        .into_iter()
        .filter(|f| f.code == findings::ASSEMBLY_CYCLE)
        .collect();
    let [finding] = found.as_slice() else {
        return Err(format!("{found:?}").into());
    };
    let name = &outline.name;
    assert_eq!(
        finding.message,
        format!(
            "{} follows the linked design's Far wall · face, and the linked design builds ‘Far wall’ \
             from {name}. Break the loop: stop following Far wall · face, or build ‘Far wall’ \
             in SolidCraft without {name}",
            capitalised(name)
        )
    );
    assert_eq!(
        assembly_link::status(engine.project(), FileCheck::NotChecked).state,
        LinkState::Cycle
    );
    let report = preflight::report(engine.project(), engine.drafts(), None)?;
    let item = report
        .items
        .iter()
        .find(|i| i.code == findings::ASSEMBLY_CYCLE)
        .ok_or("the production check lists it")?;
    assert_eq!(item.severity, Severity::Error);
    assert!(!report.ready());
    let revision = engine.revision();
    let waived = engine.apply(
        revision,
        Operation::Fabrication(pcbcraft_core::fabrication::FabricationCommand::Waive {
            keys: vec![item.key.clone()],
            reason: "known".into(),
        }),
    );
    assert!(waived.is_err(), "never waived");

    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RemoveReference {
            id: FollowedFaceId(1),
        }),
    )?;
    let link = engine.project().assembly_link.as_deref().ok_or("linked")?;
    assert!(link.cycles.is_empty());
    engine.undo()?;
    let link = engine.project().assembly_link.as_deref().ok_or("linked")?;
    assert_eq!(link.cycles.len(), 1);
    Ok(())
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Slice P1: a loop through extents only (the linked design projects the
/// board outline only to shape the profile of the sketch whose plane the
/// followed face is) settles, so it is a warning: named in both files'
/// words, not blocking fabrication output, and the link reads no loop.
#[test]
fn an_extent_only_loop_is_a_warning() -> Fallible<()> {
    let engine = sketched()?;
    let mut outline = uses(&engine, DatumRole::BoardOutline, "Far wall")?;
    outline.via = UseKind::Extent;
    let (engine, _) = followed_on(engine, vec![outline.clone()])?;
    let link = engine.project().assembly_link.as_deref().ok_or("linked")?;
    let [cycle] = link.cycles.as_slice() else {
        return Err(format!("one loop: {:?}", link.cycles).into());
    };
    assert_eq!(cycle.via, UseKind::Extent);
    let found = assembly_link::checks(engine.project());
    assert!(found.iter().all(|f| f.code != findings::ASSEMBLY_CYCLE));
    let extent: Vec<_> = found
        .iter()
        .filter(|f| f.code == findings::ASSEMBLY_EXTENT_CYCLE)
        .collect();
    let [finding] = extent.as_slice() else {
        return Err(format!("{found:?}").into());
    };
    let name = &outline.name;
    assert_eq!(
        finding.message,
        format!(
            "{} follows the linked design's Far wall · face, and the linked design takes the \
             extent of ‘Far wall’ from {name}, not its position. The loop settles; stop \
             following Far wall · face to break it",
            capitalised(name)
        )
    );
    assert_ne!(
        assembly_link::status(engine.project(), FileCheck::NotChecked).state,
        LinkState::Cycle
    );
    let issues = pcbcraft_core::issues::report(
        engine.project(),
        engine.drafts(),
        &pcbcraft_core::issues::IssueScope::All,
    )?;
    let row = issues
        .rows
        .iter()
        .find(|r| r.finding.code == findings::ASSEMBLY_EXTENT_CYCLE)
        .ok_or("Issues lists it")?;
    assert_eq!(row.severity, Severity::Warning);
    let report = preflight::report(engine.project(), engine.drafts(), None)?;
    assert!(
        report
            .items
            .iter()
            .filter(|i| i.code.starts_with("assembly."))
            .all(|i| !i.blocks()),
        "fabrication output is not blocked"
    );
    // Built on the outline by position as well: a real loop again.
    let engine = sketched()?;
    let position = uses(&engine, DatumRole::BoardOutline, "Far wall")?;
    let (engine, _) = followed_on(engine, vec![position])?;
    let link = engine.project().assembly_link.as_deref().ok_or("linked")?;
    assert!(link.cycles.iter().all(|c| c.via != UseKind::Extent));
    assert_eq!(
        assembly_link::status(engine.project(), FileCheck::NotChecked).state,
        LinkState::Cycle
    );
    // A linked design feature never uses a datum by extent alone.
    let mut by_extent = read(Vec::new());
    by_extent.consumers = vec![Consumer {
        via: UseKind::Extent,
        ..uses(&engine, DatumRole::BoardTop, "Lid")?
    }];
    let mut fresh = sketched()?;
    assert!(
        apply(
            &mut fresh,
            Operation::Assembly(AssemblyCommand::RecordLink { read: by_extent }),
        )
        .is_err()
    );
    Ok(())
}

/// A design file's hash, as SolidCraft keeps it on the linked design's link.
fn file(n: u8) -> ContentHash {
    ContentHash([0x40 + n; 32])
}

/// Publish as the host does, recording `written` as the design file's hash.
fn publish(engine: &mut Engine, written: ContentHash) -> Fallible<()> {
    use pcbcraft_core::interface::InterfaceCommand;
    use pcbcraft_core::interface::publish::{candidate, record};
    let source = pcbcraft_core::ids::InterfaceSourceId(0x005e_edb5);
    let sidecar = candidate(
        engine.project(),
        pcbcraft_core::model::interface::Tier::Interface,
        source,
    )?;
    let mut record = record(engine.project(), &sidecar)?;
    record.design_sha256 = Some(written);
    apply(
        engine,
        Operation::Interface(InterfaceCommand::RecordPublish {
            source_id: source,
            record,
        }),
    )?;
    Ok(())
}

/// Slice P1: the board and the linked design both changed since they last
/// agreed (the board moved the cutout the far wall is built on and
/// published; the linked design moved the wall while it still held the
/// earlier board). The update is not refused for that: the preview lists
/// both sides' changes and the conflicting item, the update without a
/// choice is refused changing nothing, and the user's choice decides,
/// through the typed command, which side wins.
#[test]
fn when_both_changed_the_user_chooses_which_side_wins() -> Fallible<()> {
    use assembly_link::{ConflictItem, Resolution, Side};
    let mut engine = sketched()?;
    let [width, height] = engine.project().board_size;
    let cut = created_curves_of(edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddRect {
                a: [10.0, 10.0],
                b: [16.0, 16.0],
                construction: false,
            },
        }],
    )?);
    publish(&mut engine, file(1))?;
    let cutout = uses(&engine, DatumRole::BoardCutout, "Far wall")?;
    let (mut engine, _) = followed_on(engine, vec![cutout.clone()])?;
    let link = engine.project().assembly_link.as_deref().ok_or("linked")?;
    assert!(
        link.cycles.is_empty(),
        "the cutout does not follow the wall"
    );
    assert_eq!(
        link.agreed.iter().map(|d| d.id).collect::<Vec<_>>(),
        [cutout.datum],
        "the datum the wall is built on, as last published"
    );
    // The board moves the cutout and publishes again.
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::Move {
                ids: cut.into_iter().map(SketchRef::Curve).collect(),
                delta: [0.0, 2.0],
            },
        }],
    )?;
    publish(&mut engine, file(2))?;
    // Meanwhile the linked design, holding the first publish, moved the wall
    // 2 mm.
    let mut both = read(vec![ReferenceRead {
        id: FollowedFaceId(1),
        status: ReferenceStatus::Resolved,
        pick: Some([0.0, height + 3.0, 0.0]),
        geometry: Some(line(height + 3.0, width)),
        note: String::new(),
        built_on: vec![cutout.clone()],
    }]);
    both.file_sha256 = ContentHash([2; 32]);
    both.board_sha256 = Some(file(1));
    assert!(assembly_link::both_changed(engine.project(), &both));
    let conflicts = assembly_link::conflicts(engine.project(), &both);
    let [conflict] = conflicts.as_slice() else {
        return Err(format!("one conflict: {conflicts:?}").into());
    };
    assert_eq!(conflict.item, ConflictItem::Reference(FollowedFaceId(1)));
    assert_eq!(conflict.name, "Far wall · face");
    assert_eq!(
        conflict.board,
        format!(
            "{} changed on the board",
            capitalised(&cutout.name.to_lowercase())
        )
    );
    assert_eq!(conflict.linked_design, "Far wall · face, 2 mm along Y");

    // The preview: both sides' changes, the conflict, and what the update
    // does with the linked design winning.
    let preview = engine.assembly_preview(both.clone(), super::tests::hosted());
    assert_eq!(preview.refused, None);
    assert_eq!(preview.conflicts, conflicts);
    assert_eq!(
        preview.board_changes.len(),
        1,
        "{:?}",
        preview.board_changes
    );
    assert_eq!(
        preview.findings.first().map(|f| f.code.as_str()),
        Some(findings::ASSEMBLY_BOTH_CHANGED)
    );
    assert!(!preview.references.is_empty(), "the wall's move is listed");

    // No choice: refused, nothing changed.
    let before = engine.project().clone();
    let revision = engine.revision();
    let refused = apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: both.clone(),
            resolve: Vec::new(),
        }),
    );
    let Err(error) = refused else {
        return Err("an update with an open conflict was applied".into());
    };
    assert_eq!(error.rule(), Some(findings::ASSEMBLY_BOTH_CHANGED));
    assert!(error.to_string().contains("Far wall · face"), "{error}");
    assert_eq!(engine.revision(), revision);
    assert_eq!(*engine.project(), before);
    // A choice for an item that is no conflict is refused.
    let stray = apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: both.clone(),
            resolve: vec![
                Resolution {
                    item: ConflictItem::Reference(FollowedFaceId(1)),
                    wins: Side::Board,
                },
                Resolution {
                    item: ConflictItem::Reference(FollowedFaceId(9)),
                    wins: Side::Board,
                },
            ],
        }),
    );
    assert!(stray.is_err());
    assert_eq!(*engine.project(), before);

    // The linked design wins: the top edge follows the wall.
    let mut theirs = engine.fork();
    apply(
        &mut theirs,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: both.clone(),
            resolve: vec![Resolution {
                item: ConflictItem::Reference(FollowedFaceId(1)),
                wins: Side::LinkedDesign,
            }],
        }),
    )?;
    assert!((top(&theirs)? - (height + 2.0)).abs() <= 1e-9);

    // The board wins: it keeps the wall where it was; the rest of the read
    // is taken.
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: both.clone(),
            resolve: vec![Resolution {
                item: ConflictItem::Reference(FollowedFaceId(1)),
                wins: Side::Board,
            }],
        }),
    )?;
    assert!((top(&engine)? - height).abs() <= 1e-9);
    let link = engine.project().assembly_link.as_deref().ok_or("linked")?;
    assert_eq!(
        link.references.first().map(|r| r.geometry),
        Some(line(height + 1.0, width))
    );
    assert_eq!(link.file_sha256, both.file_sha256);
    Ok(())
}

/// Slice P1, a bound parameter: the board set `width` to 52 mm and
/// published; the linked design, still holding the earlier board, changed
/// the parameter bound to it. The binding is the item both changed; the
/// user's choice decides the width.
#[test]
fn a_binding_both_sides_changed_is_chosen_too() -> Fallible<()> {
    use assembly_link::{ConflictItem, Resolution, Side};
    use pcbcraft_core::library::footprint_source::ParamName;
    use pcbcraft_core::model::assembly_link::AssemblyBinding;
    use pcbcraft_core::units::Nm;
    let mut engine = sketched()?;
    let width = engine.project().board_size[0];
    publish(&mut engine, file(1))?;
    let mut linked = read(Vec::new());
    linked.board_sha256 = Some(file(1));
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordLink { read: linked }),
    )?;
    let param = ParamName("width".into());
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordBinding {
            binding: AssemblyBinding {
                board_param: param.clone(),
                assembly_param: "inner".into(),
                value: Nm::from_mm_f64_rounded(width)?,
            },
        }),
    )?;
    edit(&mut engine, vec![super::tests::set("width", "52 mm")])?;
    publish(&mut engine, file(2))?;
    let mut both = read(Vec::new());
    both.file_sha256 = ContentHash([2; 32]);
    both.board_sha256 = Some(file(1));
    both.bindings = vec![assembly_link::BindingRead {
        board_param: param.clone(),
        value_mm: Some(70.0),
    }];
    let conflicts = assembly_link::conflicts(engine.project(), &both);
    let [conflict] = conflicts.as_slice() else {
        return Err(format!("one conflict: {conflicts:?}").into());
    };
    assert_eq!(conflict.item, ConflictItem::Binding(param.clone()));
    assert_eq!(conflict.name, "Board parameter width");
    assert_eq!(
        conflict.board,
        "Board parameter width set to 52 mm on the board"
    );
    assert_eq!(
        conflict.linked_design,
        "inner is 70 mm in the linked design"
    );
    let choose = |wins| {
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: both.clone(),
            resolve: vec![Resolution {
                item: ConflictItem::Binding(param.clone()),
                wins,
            }],
        })
    };
    let mut theirs = engine.fork();
    apply(&mut theirs, choose(Side::LinkedDesign))?;
    assert!((theirs.project().board_size[0] - 70.0).abs() <= 1e-9);
    apply(&mut engine, choose(Side::Board))?;
    assert!((engine.project().board_size[0] - 52.0).abs() <= 1e-9);
    Ok(())
}
