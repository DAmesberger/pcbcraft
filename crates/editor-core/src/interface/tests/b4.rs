//! Slice B4 in the core: the assembly link's document commands, its
//! findings and status, on the demo board with an external opening on its
//! package (as in `b3`). Reading a linked design and the dry run run where
//! Solidcraft does: the automation session's `assembly_link_tests`, over the
//! ESP32 example with a headless SolidCraft assembly.
use super::Fallible;
use super::b3::{first_placed, publish, with_openings};
use crate::fabrication::preflight;
use crate::ids::ContentHash;
use crate::interface::assembly_link::{
    self, AssemblyCommand, AssemblyRead, FileCheck, LinkState, NewReference,
};
use crate::interface::findings::{ASSEMBLY_REFERENCE_INEXACT, ASSEMBLY_REFERENCE_LOST};
use crate::interface::list;
use crate::issues::Severity;
use crate::library::footprint_source::{SketchCurveId, SketchRef};
use crate::model::assembly_link::{Consumer, FollowedFace};
use crate::model::dimension::{
    BodyName, FaceName, FeatureGeometry, FollowedFaceId, ReferenceStatus,
};
use crate::model::interface::Tier;
use crate::units::Nm;
use crate::{Engine, Operation};

const HASH: ContentHash = ContentHash([7; 32]);

/// What a host reads of a linked design whose ‘USB opening’ uses `consumer`.
fn read(consumers: Vec<Consumer>) -> AssemblyRead {
    AssemblyRead {
        assembly_hint: "assembly.solidcraft".into(),
        design_hint: "board.solidcraft".into(),
        component: "Board".into(),
        file_sha256: HASH,
        board_revision: Some(1),
        references: Vec::new(),
        bindings: Vec::new(),
        consumers,
        board_sha256: None,
        unexported: Vec::new(),
    }
}

fn apply(engine: &mut Engine, command: AssemblyCommand) -> Fallible {
    let revision = engine.revision();
    engine.apply(revision, Operation::Assembly(command))?;
    Ok(())
}

/// The demo, published, with an assembly linked whose ‘USB opening’ uses
/// the first placement's opening. Returns the engine and that instance.
fn linked() -> Fallible<(Engine, u64)> {
    let mut engine = with_openings()?;
    let published = publish(&mut engine, Tier::Interface)?;
    let (instance, ..) = first_placed(&engine)?;
    let token = format!("i{instance}.opening");
    let datum = published
        .datums
        .iter()
        .find(|d| d.token == token)
        .ok_or("the opening was published")?;
    let consumer = Consumer {
        feature: "USB opening".into(),
        datum: datum.id,
        slot: datum.slot,
        via: Default::default(),
        token,
        name: datum.name.clone(),
    };
    apply(
        &mut engine,
        AssemblyCommand::RecordLink {
            read: read(vec![consumer]),
        },
    )?;
    Ok((engine, instance))
}

fn codes(engine: &Engine) -> Vec<String> {
    assembly_link::checks(engine.project())
        .into_iter()
        .map(|f| f.code)
        .collect()
}

/// Acceptance 6 in the core: the linked design's cutout uses a part's opening;
/// deleting the part makes it a broken reference that names the cutout
/// and the part, reads "broken reference", is a warning in Issues that
/// blocks publishing to the linked design but not fabrication output, and
/// undo takes it away.
#[test]
fn b4_deleting_the_part_breaks_the_linked_design_reference() -> Fallible {
    let (mut engine, instance) = linked()?;
    assert!(codes(&engine).is_empty());
    let status = assembly_link::status(engine.project(), FileCheck::Found(HASH));
    assert_eq!(status.state, LinkState::InSync);
    let designator = engine
        .project()
        .instances
        .iter()
        .find(|i| i.id == instance)
        .map(|i| i.designator.clone())
        .ok_or("the part")?;
    let before = engine.project().clone();

    let revision = engine.revision();
    engine.apply_update(
        revision,
        crate::updates::Change::Instance {
            edit: crate::updates::InstanceChange::Remove { instance },
        },
        false,
    )?;
    let findings = assembly_link::checks(engine.project());
    let [broken] = findings.as_slice() else {
        return Err(format!("{findings:?}").into());
    };
    assert_eq!(broken.code, ASSEMBLY_REFERENCE_LOST);
    assert_eq!(
        broken.message,
        format!(
            "The linked design's ‘USB opening’ uses {designator} opening. {designator} was deleted, \
             so it would fail. Restore {designator}, or pick another reference in SolidCraft"
        )
    );
    let status = assembly_link::status(engine.project(), FileCheck::Found(HASH));
    assert_eq!(status.state, LinkState::BrokenReference);
    assert_eq!(status.words, "broken reference");

    // A warning in Issues at once. Fabrication output goes ahead;
    // publishing to the linked design waits until it is resolved.
    let issues = crate::issues::report(
        engine.project(),
        engine.drafts(),
        &crate::issues::IssueScope::All,
    )?;
    let row = issues
        .rows
        .iter()
        .find(|r| r.finding.code == ASSEMBLY_REFERENCE_LOST)
        .ok_or("Issues lists it")?;
    assert_eq!(row.severity, Severity::Warning);
    assert_eq!(row.blocks(), None);
    let report = preflight::report(engine.project(), engine.drafts(), None)?;
    assert!(
        report
            .items
            .iter()
            .filter(|i| i.code == ASSEMBLY_REFERENCE_LOST)
            .all(|i| !i.blocks()),
        "fabrication output is not blocked"
    );
    assert_eq!(
        assembly_link::publish_blockers(engine.project()),
        std::slice::from_ref(&broken.message)
    );
    let preview =
        crate::interface::publish::preview(engine.project(), None, None, engine.revision())?;
    assert_eq!(preview.blocked, std::slice::from_ref(&broken.message));

    engine.undo()?;
    assert_eq!(*engine.project(), before);
    assert!(codes(&engine).is_empty());
    Ok(())
}

/// A datum the linked design uses that the last publish no longer carries is
/// broken too, even while it is live again on the board.
#[test]
fn b4_a_datum_missing_from_the_publish_is_broken() -> Fallible {
    let (engine, _) = linked()?;
    let mut project = engine.project().clone();
    let consumer = project
        .assembly_link
        .as_ref()
        .and_then(|l| l.consumers.first().cloned())
        .ok_or("a consumer")?;
    let record = project.interface.published.as_mut().ok_or("published")?;
    record.datums.retain(|d| d.id != consumer.datum);
    assert_eq!(assembly_link::broken(&project).len(), 1);
    // The board itself still has the datum.
    assert!(list::table(&project)?.datum(consumer.datum).is_some());
    Ok(())
}

/// Lost and inexact followed faces are reported in the user's words as
/// warnings; the lost one blocks publishing to the linked design.
#[test]
fn b4_lost_and_inexact_references_are_reported() -> Fallible {
    let (engine, _) = linked()?;
    let mut project = engine.project().clone();
    let link = project.assembly_link.as_deref_mut().ok_or("linked")?;
    let reference = |id: u32, status: ReferenceStatus, note: &str| -> Fallible<FollowedFace> {
        Ok(FollowedFace {
            id: FollowedFaceId(id),
            body: BodyName("Far wall".into()),
            face: FaceName(format!("F{id}:side:e1")),
            pick: [Nm::ZERO, Nm::new(70_000_000)?, Nm::new(1_000_000)?],
            name: format!("Far wall · face {id}"),
            geometry: FeatureGeometry::Line {
                a: [0.0, 70.0],
                b: [40.0, 70.0],
            },
            status,
            note: note.into(),
            sketch: SketchRef::Curve(SketchCurveId(format!("l{}", 10 + id))),
            built_on: Vec::new(),
        })
    };
    link.references = vec![
        reference(1, ReferenceStatus::Lost, "the face is gone")?,
        reference(2, ReferenceStatus::Inexact, "")?,
    ];
    link.next_ref = 3;
    assembly_link::check_link(link)?;
    let findings = assembly_link::checks(&project);
    let lost = findings
        .iter()
        .find(|f| f.code == ASSEMBLY_REFERENCE_LOST)
        .ok_or("lost")?;
    assert_eq!(
        lost.message,
        "The board follows the linked design's Far wall · face 1, which is no longer there: the face \
         is gone. Pick it again, or stop following it"
    );
    assert!(
        findings
            .iter()
            .any(|f| f.code == ASSEMBLY_REFERENCE_INEXACT)
    );
    // Warnings: neither blocks fabrication output; only the lost one
    // blocks publishing to the linked design.
    let report = preflight::report(&project, engine.drafts(), None)?;
    for code in [ASSEMBLY_REFERENCE_LOST, ASSEMBLY_REFERENCE_INEXACT] {
        let item = report.items.iter().find(|i| i.code == code).ok_or(code)?;
        assert_eq!(item.severity, Severity::Warning, "{code}");
        assert!(!item.blocks(), "{code}");
    }
    assert_eq!(
        assembly_link::publish_blockers(&project),
        std::slice::from_ref(&lost.message)
    );
    assert_eq!(
        assembly_link::status(&project, FileCheck::NotChecked).state,
        LinkState::BrokenReference
    );
    Ok(())
}

/// A second link is refused (relink instead), a new link carries no
/// references, and with no link every assembly command but a link is
/// refused, changing nothing.
#[test]
fn b4_refused_link_commands_change_nothing() -> Fallible {
    let (mut engine, _) = linked()?;
    let revision = engine.revision();
    let again = engine.apply(
        revision,
        Operation::Assembly(AssemblyCommand::RecordLink {
            read: read(Vec::new()),
        }),
    );
    assert!(again.is_err());
    assert_eq!(engine.revision(), revision);
    apply(&mut engine, AssemblyCommand::Unlink)?;
    assert!(engine.project().assembly_link.is_none());
    let revision = engine.revision();
    for command in [
        AssemblyCommand::Unlink,
        AssemblyCommand::RecordUpdate {
            read: read(Vec::new()),
            resolve: Vec::new(),
        },
        AssemblyCommand::RemoveReference {
            id: FollowedFaceId(1),
        },
        AssemblyCommand::RecordReference {
            reference: NewReference {
                body: BodyName("Wall".into()),
                face: FaceName("F1:start".into()),
                pick: [0.0; 3],
                name: "Wall".into(),
                geometry: FeatureGeometry::Point { at: [1.0, 1.0] },
                built_on: Vec::new(),
            },
        },
    ] {
        assert!(
            engine
                .apply(revision, Operation::Assembly(command.clone()))
                .is_err(),
            "{command:?}"
        );
        assert_eq!(engine.revision(), revision);
    }
    let mut with_reference = read(Vec::new());
    with_reference
        .references
        .push(assembly_link::ReferenceRead {
            id: FollowedFaceId(1),
            status: ReferenceStatus::Resolved,
            pick: None,
            geometry: None,
            note: String::new(),
            built_on: Vec::new(),
        });
    assert!(
        engine
            .apply(
                revision,
                Operation::Assembly(AssemblyCommand::RecordLink {
                    read: with_reference,
                }),
            )
            .is_err()
    );
    // Undo brings the link back.
    engine.undo()?;
    assert!(engine.project().assembly_link.is_some());
    Ok(())
}

/// The link survives saving and reopening the document unchanged, and a
/// stored link that breaks its bounds is refused on open.
#[test]
fn b4_the_link_survives_save_and_reopen() -> Fallible {
    let (engine, _) = linked()?;
    let text = serde_json::to_string(&engine.to_file())?;
    let file: crate::format::EditorFile = serde_json::from_str(&text)?;
    let reopened = Engine::from_file(file)?;
    assert_eq!(
        reopened.project().assembly_link,
        engine.project().assembly_link
    );
    let mut bad = engine.to_file();
    if let Some(link) = bad.project.assembly_link.as_deref_mut() {
        link.component = "\u{7}".into();
    }
    assert!(Engine::from_file(bad).is_err());
    Ok(())
}
