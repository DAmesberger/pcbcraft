//! Slice B5 in the core: the two-way check's rules on stored data, on the
//! demo board with an external opening on its package (as in `b3` and
//! `b4`). The older-board check compares the hash a linked design keeps on its
//! link with the files the board published; consumers are checked by their
//! slot; features on unexported faces are named in the user's words. The
//! halves that read a linked design run in the automation session
//! (`session::two_way_tests`), and the board side of the cycle graph and
//! the driven edge where the sketch evaluator runs (`pcbcraft-solidcraft`
//! `board::fixed_tests`).
use super::Fallible;
use super::b3::{first_placed, with_openings};
use crate::fabrication::preflight;
use crate::ids::{ContentHash, InterfaceSourceId};
use crate::interface::assembly_link::{
    self, Agreement, AssemblyCommand, AssemblyRead, FileCheck, LinkState,
};
use crate::interface::findings::{
    ASSEMBLY_BOTH_CHANGED, ASSEMBLY_OLDER_BOARD, ASSEMBLY_REFERENCE_LOST, ASSEMBLY_STALE,
    ASSEMBLY_UNEXPORTED_FACE,
};
use crate::interface::publish::{candidate, record};
use crate::interface::{InterfaceCommand, Sidecar};
use crate::issues::Severity;
use crate::layout::LayoutCommand;
use crate::model::assembly_link::{Consumer, UnexportedUse, UseKind};
use crate::model::interface::{MAX_PUBLISHED_FILES, Slot, Tier};
use crate::{Engine, Operation};

const SOURCE: InterfaceSourceId = InterfaceSourceId(0x005e_edb5);
/// The linked design file's hash when the board read it.
const LINKED_DESIGN: ContentHash = ContentHash([7; 32]);
/// The linked design file's hash after it was edited and saved.
const EDITED: ContentHash = ContentHash([8; 32]);

/// A design file's hash, as SolidCraft keeps it on the linked design's link.
fn file(n: u8) -> ContentHash {
    ContentHash([0x40 + n; 32])
}

/// Publish as the host does, recording `written` as the design file's
/// hash. Returns the sidecar.
fn publish(engine: &mut Engine, written: ContentHash) -> Fallible<Sidecar> {
    let sidecar = candidate(engine.project(), Tier::Interface, SOURCE)?;
    let mut record = record(engine.project(), &sidecar)?;
    record.design_sha256 = Some(written);
    let revision = engine.revision();
    engine.apply(
        revision,
        Operation::Interface(InterfaceCommand::RecordPublish {
            source_id: SOURCE,
            record,
        }),
    )?;
    Ok(sidecar)
}

fn read(file_sha256: ContentHash, board: Option<ContentHash>) -> AssemblyRead {
    AssemblyRead {
        assembly_hint: "assembly.solidcraft".into(),
        design_hint: "board.solidcraft".into(),
        component: "Board".into(),
        file_sha256,
        board_revision: None,
        references: Vec::new(),
        bindings: Vec::new(),
        consumers: Vec::new(),
        board_sha256: board,
        unexported: Vec::new(),
    }
}

fn apply(engine: &mut Engine, operation: Operation) -> crate::Result<()> {
    let revision = engine.revision();
    engine.apply(revision, operation).map(|_| ())
}

/// Move the first placed part by 2 mm, there and back, so the next
/// publish is a new revision.
fn nudge(engine: &mut Engine) -> Fallible {
    let (instance, position, rotation_deg, layer) = first_placed(engine)?;
    let dx = if engine.revision() % 4 < 2 { 2.0 } else { -2.0 };
    apply(
        engine,
        Operation::Layout(LayoutCommand::Move {
            instance,
            position: [position[0] + dx, position[1]],
            rotation_deg,
            layer,
        }),
    )?;
    Ok(())
}

fn codes(engine: &Engine) -> Vec<String> {
    assembly_link::checks(engine.project())
        .into_iter()
        .map(|f| f.code)
        .collect()
}

/// The demo published once (file 1) and linked by a linked design holding
/// that file.
fn agreed() -> Fallible<Engine> {
    let mut engine = with_openings()?;
    publish(&mut engine, file(1))?;
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordLink {
            read: read(LINKED_DESIGN, Some(file(1))),
        }),
    )?;
    Ok(engine)
}

/// Conflict row "the linked design holds an older board": the hash on the
/// linked design's link is not the last published file's. The board names the
/// revision the linked design holds and what to do, as an advisory that does
/// not block fabrication; once the linked design ran Update Links and the board
/// read it again, it is gone. A hash the board never published reads as
/// an older board without a revision.
#[test]
fn b5_a_linked_design_holding_an_older_board_is_reported() -> Fallible {
    let mut engine = agreed()?;
    assert!(codes(&engine).is_empty());
    assert_eq!(
        assembly_link::status(engine.project(), FileCheck::Found(LINKED_DESIGN)).state,
        LinkState::InSync
    );
    nudge(&mut engine)?;
    let second = publish(&mut engine, file(2))?;
    assert_eq!(second.revision, 2);
    let findings = assembly_link::checks(engine.project());
    let [older] = findings.as_slice() else {
        return Err(format!("{findings:?}").into());
    };
    assert_eq!(older.code, ASSEMBLY_OLDER_BOARD);
    assert_eq!(
        older.message,
        "The linked design still uses board revision 1. Run Update Links in SolidCraft, then check \
         again"
    );
    let status = assembly_link::status(engine.project(), FileCheck::Found(LINKED_DESIGN));
    assert_eq!(status.state, LinkState::OlderBoard);
    assert_eq!(status.words, "linked design holds an older board");
    assert_eq!(
        assembly_link::status(engine.project(), FileCheck::NotChecked).state,
        LinkState::OlderBoard
    );
    let report = preflight::report(engine.project(), engine.drafts(), None)?;
    let item = report
        .items
        .iter()
        .find(|i| i.code == ASSEMBLY_OLDER_BOARD)
        .ok_or("the production check lists it")?;
    assert_eq!(item.severity, Severity::Warning, "advisory");

    // The linked design ran Update Links and was saved: read again, it agrees.
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: read(LINKED_DESIGN, Some(file(2))),
            resolve: Vec::new(),
        }),
    )?;
    assert!(codes(&engine).is_empty());

    // A file this board never wrote.
    let mut project = engine.project().clone();
    if let Some(link) = project.assembly_link.as_deref_mut() {
        link.board_sha256 = Some(file(9));
    }
    assert_eq!(
        assembly_link::agreement(&project, Some(file(9))),
        Agreement::Older { revision: None }
    );
    let findings = assembly_link::checks(&project);
    assert_eq!(
        findings.first().map(|f| f.message.as_str()),
        Some(
            "The linked design still uses an older board. Run Update Links in SolidCraft, then check \
             again"
        )
    );
    // No hash on the link (a linked design saved before SolidCraft kept one):
    // nothing is claimed.
    assert_eq!(assembly_link::agreement(&project, None), Agreement::Unknown);
    Ok(())
}

/// Conflict row "both changed": the linked design holds an older board and
/// its own file changed. Its status says so once, instead of "changed"
/// and "older", and asks for the preview. An update from it is not refused
/// for that (slice P1): with no item both changed there is nothing to
/// choose, so it is applied. A choice for an item that is no conflict is
/// refused, changing nothing. Conflicting items and the choice between
/// sides are tested where the sketch evaluator runs (`pcbcraft-solidcraft`
/// `board::fixed_tests`).
#[test]
fn b5_both_changed_asks_for_the_preview() -> Fallible {
    let mut engine = agreed()?;
    nudge(&mut engine)?;
    publish(&mut engine, file(2))?;
    let status = assembly_link::status(engine.project(), FileCheck::Found(EDITED));
    assert_eq!(status.state, LinkState::BothChanged);
    let codes: Vec<&str> = status.findings.iter().map(|f| f.code.as_str()).collect();
    assert_eq!(codes, [ASSEMBLY_BOTH_CHANGED], "{codes:?}");
    assert_eq!(
        status.findings.first().map(|f| f.message.as_str()),
        Some(
            "The board and the linked design both changed since they last agreed. Preview the \
             update and choose which side wins where they conflict"
        )
    );
    // The linked design changed alone: only stale.
    let mut alone = engine.project().clone();
    if let Some(link) = alone.assembly_link.as_deref_mut() {
        link.board_sha256 = Some(file(2));
    }
    let codes: Vec<String> = assembly_link::status(&alone, FileCheck::Found(EDITED))
        .findings
        .into_iter()
        .map(|f| f.code)
        .collect();
    assert_eq!(codes, [ASSEMBLY_STALE]);

    let before = engine.project().clone();
    let revision = engine.revision();
    let stray = engine.apply(
        revision,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: read(EDITED, Some(file(1))),
            resolve: vec![assembly_link::Resolution {
                item: assembly_link::ConflictItem::Reference(
                    crate::model::dimension::FollowedFaceId(1),
                ),
                wins: assembly_link::Side::Board,
            }],
        }),
    );
    let Err(error) = stray else {
        return Err("a choice for an item that is no conflict".into());
    };
    assert_eq!(error.rule(), Some(ASSEMBLY_BOTH_CHANGED));
    assert_eq!(engine.revision(), revision);
    assert_eq!(*engine.project(), before);
    assert!(assembly_link::conflicts(engine.project(), &read(EDITED, Some(file(1)))).is_empty());
    // Nothing both changed: applied. The linked design still holds the
    // earlier board.
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: read(EDITED, Some(file(1))),
            resolve: Vec::new(),
        }),
    )?;
    assert_eq!(
        assembly_link::status(engine.project(), FileCheck::Found(EDITED)).state,
        LinkState::OlderBoard
    );
    // Updated in SolidCraft: in sync.
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordUpdate {
            read: read(EDITED, Some(file(2))),
            resolve: Vec::new(),
        }),
    )?;
    assert_eq!(
        assembly_link::status(engine.project(), FileCheck::Found(EDITED)).state,
        LinkState::InSync
    );
    Ok(())
}

/// The board remembers the files it published, so it can name the
/// revision a linked design holds: a publish with no datum changed keeps its
/// revision whatever bytes it wrote; the memory is bounded, oldest first;
/// and it is the board's own, never the host's.
#[test]
fn b5_the_board_remembers_its_published_files() -> Fallible {
    let mut engine = with_openings()?;
    publish(&mut engine, file(1))?;
    // The same datums written again with other bytes: still revision 1,
    // and a linked design holding either file holds the last publish.
    publish(&mut engine, file(2))?;
    let record = engine
        .project()
        .interface
        .published
        .clone()
        .ok_or("published")?;
    assert_eq!(record.revision, 1);
    for held in [file(1), file(2)] {
        assert_eq!(
            assembly_link::agreement(engine.project(), Some(held)),
            Agreement::Current
        );
    }
    let sidecar = candidate(engine.project(), Tier::Interface, SOURCE)?;
    let mut forged = crate::interface::publish::record(engine.project(), &sidecar)?;
    forged.earlier = record.earlier.clone();
    forged.design_sha256 = Some(file(3));
    let revision = engine.revision();
    assert!(
        engine
            .apply(
                revision,
                Operation::Interface(InterfaceCommand::RecordPublish {
                    source_id: SOURCE,
                    record: forged,
                }),
            )
            .is_err(),
        "the earlier files are the board's own record"
    );
    for n in 0..=u8::try_from(MAX_PUBLISHED_FILES)? {
        nudge(&mut engine)?;
        publish(&mut engine, ContentHash([n; 32]))?;
    }
    let record = engine
        .project()
        .interface
        .published
        .clone()
        .ok_or("published")?;
    assert_eq!(record.earlier.len(), MAX_PUBLISHED_FILES);
    assert!(
        record
            .earlier
            .windows(2)
            .all(|w| w[0].revision < w[1].revision),
        "oldest first"
    );
    assert_eq!(
        assembly_link::agreement(engine.project(), Some(file(1))),
        Agreement::Older { revision: None },
        "forgotten past the bound"
    );
    Ok(())
}

/// Consumers by slot: a linked design feature is checked by the slot its
/// link names. A consumer whose slot holds another datum, or no datum, is
/// a broken reference even when its datum id is still on the board.
#[test]
fn b5_consumers_are_checked_by_slot() -> Fallible {
    let mut engine = with_openings()?;
    let published = publish(&mut engine, file(1))?;
    let (instance, ..) = first_placed(&engine)?;
    let token = format!("i{instance}.opening");
    let datum = published
        .datums
        .iter()
        .find(|d| d.token == token)
        .ok_or("the opening was published")?;
    let consumer = |slot: Slot| Consumer {
        feature: "USB opening".into(),
        datum: datum.id,
        slot,
        via: UseKind::Sketch,
        token: token.clone(),
        name: datum.name.clone(),
    };
    let mut linked = read(LINKED_DESIGN, Some(file(1)));
    linked.consumers = vec![consumer(datum.slot)];
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordLink { read: linked }),
    )?;
    assert!(assembly_link::broken(engine.project()).is_empty());
    let other = published
        .datums
        .iter()
        .find(|d| d.slot != datum.slot)
        .ok_or("another slot")?
        .slot;
    for slot in [other, Slot(published.slot_count() + 7)] {
        let mut project = engine.project().clone();
        if let Some(link) = project.assembly_link.as_deref_mut() {
            link.consumers = vec![consumer(slot)];
        }
        let broken = assembly_link::broken(&project);
        assert_eq!(broken.len(), 1, "{slot}");
        assert!(
            assembly_link::checks(&project)
                .iter()
                .any(|f| f.code == ASSEMBLY_REFERENCE_LOST),
            "{slot}"
        );
    }
    Ok(())
}

/// Conflict row "a reference is on an unexported face": the linked design
/// feature and the part are named in the user's words, the part is
/// located, and the advisory does not block fabrication. A face of the
/// board's own bodies that is no part's reads as the board's.
#[test]
fn b5_a_feature_on_an_unexported_face_is_named() -> Fallible {
    let mut engine = with_openings()?;
    publish(&mut engine, file(1))?;
    let (instance, ..) = first_placed(&engine)?;
    let designator = engine
        .project()
        .instance(instance)
        .ok_or("instance")?
        .designator
        .clone();
    let mut linked = read(LINKED_DESIGN, Some(file(1)));
    linked.unexported = vec![
        UnexportedUse {
            feature: "Lid rib".into(),
            body: format!("{designator} model"),
        },
        UnexportedUse {
            feature: "Clip".into(),
            body: "Laminate".into(),
        },
    ];
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordLink { read: linked }),
    )?;
    let findings: Vec<_> = assembly_link::checks(engine.project())
        .into_iter()
        .filter(|f| f.code == ASSEMBLY_UNEXPORTED_FACE)
        .collect();
    let [clip, rib] = findings.as_slice() else {
        return Err(format!("{findings:?}").into());
    };
    assert_eq!(
        rib.message,
        format!(
            "The linked design's ‘Lid rib’ is attached to a face of {designator} that is not \
             exported. It may move to another part when the board changes. Export the face to \
             keep it stable"
        )
    );
    assert_eq!(
        rib.objects,
        vec![crate::diagnostics::ObjectRef::Instance { id: instance }]
    );
    assert!(
        clip.message.starts_with(
            "The linked design's ‘Clip’ is attached to a face of the board's ‘Laminate’"
        ),
        "{}",
        clip.message
    );
    let report = preflight::report(engine.project(), engine.drafts(), None)?;
    assert!(
        report
            .items
            .iter()
            .filter(|i| i.code == ASSEMBLY_UNEXPORTED_FACE)
            .all(|i| i.severity == Severity::Warning)
    );
    Ok(())
}
