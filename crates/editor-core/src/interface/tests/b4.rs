//! Slice B4 in the core: the enclosure link's document commands, its
//! findings and status, on the demo board with an external opening on its
//! package (as in `b3`). Reading an enclosure and the dry run run where
//! Solidcraft does: the automation session's `housing_tests`, over the
//! ESP32 example with a headless SolidCraft enclosure.
use super::Fallible;
use super::b3::{first_placed, publish, with_openings};
use crate::fabrication::preflight;
use crate::ids::ContentHash;
use crate::interface::findings::{HOUSING_REFERENCE_INEXACT, HOUSING_REFERENCE_LOST};
use crate::interface::housing::{
    self, FileCheck, HousingCommand, HousingRead, LinkState, NewReference,
};
use crate::interface::list;
use crate::issues::Severity;
use crate::library::footprint_source::{SketchCurveId, SketchRef};
use crate::model::dimension::{BodyName, FaceName, FeatureGeometry, HousingRefId, ReferenceStatus};
use crate::model::housing::{Consumer, HousingRef};
use crate::model::interface::Tier;
use crate::units::Nm;
use crate::{Engine, Operation};

const HASH: ContentHash = ContentHash([7; 32]);

/// What a host reads of an enclosure whose ‘USB opening’ uses `consumer`.
fn read(consumers: Vec<Consumer>) -> HousingRead {
    HousingRead {
        enclosure_hint: "housing.solidcraft".into(),
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

fn apply(engine: &mut Engine, command: HousingCommand) -> Fallible {
    let revision = engine.revision();
    engine.apply(revision, Operation::Housing(command))?;
    Ok(())
}

/// The demo, published, with an enclosure linked whose ‘USB opening’ uses
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
        HousingCommand::RecordLink {
            read: read(vec![consumer]),
        },
    )?;
    Ok((engine, instance))
}

fn codes(engine: &Engine) -> Vec<String> {
    housing::checks(engine.project())
        .into_iter()
        .map(|f| f.code)
        .collect()
}

/// Acceptance 6 in the core: the enclosure's cutout uses a part's opening;
/// deleting the part makes it a broken reference that names the cutout
/// and the part, reads "broken reference", is an error in the production
/// check that no waiver lifts, and undo takes it away.
#[test]
fn b4_deleting_the_part_breaks_the_enclosure_reference() -> Fallible {
    let (mut engine, instance) = linked()?;
    assert!(codes(&engine).is_empty());
    let status = housing::status(engine.project(), FileCheck::Found(HASH));
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
    let findings = housing::checks(engine.project());
    let [broken] = findings.as_slice() else {
        return Err(format!("{findings:?}").into());
    };
    assert_eq!(broken.code, HOUSING_REFERENCE_LOST);
    assert_eq!(
        broken.message,
        format!(
            "The enclosure's ‘USB opening’ uses {designator} opening. {designator} was deleted, \
             so it would fail. Restore {designator}, or pick another reference in SolidCraft"
        )
    );
    let status = housing::status(engine.project(), FileCheck::Found(HASH));
    assert_eq!(status.state, LinkState::BrokenReference);
    assert_eq!(status.words, "broken reference");

    let report = preflight::report(engine.project(), engine.drafts(), None)?;
    let item = report
        .items
        .iter()
        .find(|i| i.code == HOUSING_REFERENCE_LOST)
        .ok_or("the production check lists it")?;
    assert_eq!(item.severity, Severity::Error);
    assert!(!report.ready());
    let revision = engine.revision();
    let waived = engine.apply(
        revision,
        Operation::Fabrication(crate::fabrication::FabricationCommand::Waive {
            keys: vec![item.key.clone()],
            reason: "the enclosure is redone".into(),
        }),
    );
    assert!(waived.is_err(), "never waived");

    engine.undo()?;
    assert_eq!(*engine.project(), before);
    assert!(codes(&engine).is_empty());
    Ok(())
}

/// A datum the enclosure uses that the last publish no longer carries is
/// broken too, even while it is live again on the board.
#[test]
fn b4_a_datum_missing_from_the_publish_is_broken() -> Fallible {
    let (engine, _) = linked()?;
    let mut project = engine.project().clone();
    let consumer = project
        .housing
        .as_ref()
        .and_then(|l| l.consumers.first().cloned())
        .ok_or("a consumer")?;
    let record = project.interface.published.as_mut().ok_or("published")?;
    record.datums.retain(|d| d.id != consumer.datum);
    assert_eq!(housing::broken(&project).len(), 1);
    // The board itself still has the datum.
    assert!(list::table(&project)?.datum(consumer.datum).is_some());
    Ok(())
}

/// Lost and inexact housing references are reported in the user's words
/// and block fabrication output.
#[test]
fn b4_lost_and_inexact_references_are_reported() -> Fallible {
    let (engine, _) = linked()?;
    let mut project = engine.project().clone();
    let link = project.housing.as_deref_mut().ok_or("linked")?;
    let reference = |id: u32, status: ReferenceStatus, note: &str| -> Fallible<HousingRef> {
        Ok(HousingRef {
            id: HousingRefId(id),
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
    housing::check_link(link)?;
    let findings = housing::checks(&project);
    let lost = findings
        .iter()
        .find(|f| f.code == HOUSING_REFERENCE_LOST)
        .ok_or("lost")?;
    assert_eq!(
        lost.message,
        "The board follows the enclosure's Far wall · face 1, which is no longer there: the face \
         is gone. Pick it again, or stop following it"
    );
    assert!(findings.iter().any(|f| f.code == HOUSING_REFERENCE_INEXACT));
    let report = preflight::report(&project, engine.drafts(), None)?;
    for code in [HOUSING_REFERENCE_LOST, HOUSING_REFERENCE_INEXACT] {
        let item = report.items.iter().find(|i| i.code == code).ok_or(code)?;
        assert_eq!(item.severity, Severity::Error, "{code}");
    }
    assert_eq!(
        housing::status(&project, FileCheck::NotChecked).state,
        LinkState::BrokenReference
    );
    Ok(())
}

/// A second link is refused (relink instead), a new link carries no
/// references, and with no link every housing command but a link is
/// refused, changing nothing.
#[test]
fn b4_refused_link_commands_change_nothing() -> Fallible {
    let (mut engine, _) = linked()?;
    let revision = engine.revision();
    let again = engine.apply(
        revision,
        Operation::Housing(HousingCommand::RecordLink {
            read: read(Vec::new()),
        }),
    );
    assert!(again.is_err());
    assert_eq!(engine.revision(), revision);
    apply(&mut engine, HousingCommand::Unlink)?;
    assert!(engine.project().housing.is_none());
    let revision = engine.revision();
    for command in [
        HousingCommand::Unlink,
        HousingCommand::RecordUpdate {
            read: read(Vec::new()),
        },
        HousingCommand::RemoveReference {
            id: HousingRefId(1),
        },
        HousingCommand::RecordReference {
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
                .apply(revision, Operation::Housing(command.clone()))
                .is_err(),
            "{command:?}"
        );
        assert_eq!(engine.revision(), revision);
    }
    let mut with_reference = read(Vec::new());
    with_reference.references.push(housing::ReferenceRead {
        id: HousingRefId(1),
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
                Operation::Housing(HousingCommand::RecordLink {
                    read: with_reference,
                }),
            )
            .is_err()
    );
    // Undo brings the link back.
    engine.undo()?;
    assert!(engine.project().housing.is_some());
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
    assert_eq!(reopened.project().housing, engine.project().housing);
    let mut bad = engine.to_file();
    if let Some(link) = bad.project.housing.as_deref_mut() {
        link.component = "\u{7}".into();
    }
    assert!(Engine::from_file(bad).is_err());
    Ok(())
}
