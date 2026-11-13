//! Slice P1 in the core: a publish, like a save, is no undo step, and its
//! preview compares against the file on disk; the project keeps where it
//! last published. Broken references, conflicts, extent loops and cutout
//! ids are tested in `b4`, `b5` and `pcbcraft-solidcraft`'s `board` tests.
use super::Fallible;
use super::b3::{first_placed, move_by, publish};
use crate::interface::publish::{candidate, preview, record};
use crate::interface::report::ChangeKind;
use crate::interface::{InterfaceCommand, check_table};
use crate::model::interface::Tier;
use crate::{Engine, Operation, format};

/// Publishing records the publish without an undo step: undo takes back
/// the edit before it and keeps the publish, redo too; a publish into an
/// empty history leaves nothing to undo.
#[test]
fn publishing_is_no_undo_step() -> Fallible {
    let mut engine = super::b3::with_openings()?;

    let (instance, ..) = first_placed(&engine)?;
    move_by(&mut engine, instance, [2.0, 0.0])?;
    let moved = engine.project().placements.clone();
    let step = engine.undo_step();
    let revision = engine.revision();
    publish(&mut engine, Tier::Interface)?;
    assert_eq!(engine.revision(), revision + 1, "a new revision");
    assert_eq!(engine.undo_step(), step, "no undo step of its own");
    let published = engine.project().interface.published.clone();
    assert!(published.is_some());

    engine.undo()?;
    assert_ne!(engine.project().placements, moved, "the move is undone");
    assert_eq!(
        engine.project().interface.published,
        published,
        "the publish is kept"
    );
    engine.redo()?;
    assert_eq!(engine.project().placements, moved);
    assert_eq!(engine.project().interface.published, published);

    // With nothing to undo, a publish leaves nothing to undo.
    let mut clean = Engine::from_file(engine.to_file())?;
    assert!(!clean.can_undo());
    move_by(&mut clean, instance, [2.0, 0.0])?;
    clean.undo()?;
    assert!(!clean.can_undo());
    publish(&mut clean, Tier::Interface)?;
    assert!(!clean.can_undo(), "{:?}", clean.undo_step());
    Ok(())
}

/// The preview compares with the sidecar on disk, what the linked design
/// sees, not with the record: with the file of an earlier publish on disk
/// the moved datum reads as moved, though the record holds the move.
#[test]
fn the_publish_preview_compares_against_the_file_on_disk() -> Fallible {
    let mut engine = super::b3::with_openings()?;
    let first = publish(&mut engine, Tier::Interface)?;
    let (instance, ..) = first_placed(&engine)?;
    move_by(&mut engine, instance, [2.0, 0.0])?;
    let second = publish(&mut engine, Tier::Interface)?;
    assert_eq!(second.revision, 2);

    let moved = |previous| -> Fallible<Vec<ChangeKind>> {
        Ok(
            preview(engine.project(), None, previous, engine.revision())?
                .changes
                .into_iter()
                .map(|c| c.change)
                .filter(|c| *c != ChangeKind::Unchanged)
                .collect(),
        )
    };
    // The earlier file on disk: the datum moved since.
    let on_disk = moved(Some(&first))?;
    let [ChangeKind::Moved { .. }] = on_disk.as_slice() else {
        return Err(format!("{on_disk:?}").into());
    };
    let from_disk = preview(engine.project(), None, Some(&first), engine.revision())?;
    assert_eq!(from_disk.from, Some(1), "the revision on disk");
    // The last file on disk, or none to read: nothing changed.
    assert!(moved(Some(&second))?.is_empty());
    assert!(moved(None)?.is_empty());
    Ok(())
}

/// The project keeps where it last published: a record with a path keeps
/// it through save and reopen, a later record without one keeps the last
/// path, and a path that is not text of 1–4096 bytes is refused.
#[test]
fn the_project_keeps_where_it_last_published() -> Fallible {
    let mut engine = super::b3::with_openings()?;
    let source = super::b3::SOURCE;
    let record_at = |engine: &Engine, path: Option<&str>| -> Fallible<_> {
        let sidecar = candidate(engine.project(), Tier::Interface, source)?;
        let mut r = record(engine.project(), &sidecar)?;
        r.design_hint = path.map(str::to_owned);
        Ok(r)
    };
    let apply = |engine: &mut Engine, record| {
        let revision = engine.revision();
        engine.apply(
            revision,
            Operation::Interface(InterfaceCommand::RecordPublish {
                source_id: source,
                record,
            }),
        )
    };
    let first = record_at(&engine, Some("shared/board-v2.solidcraft"))?;
    apply(&mut engine, first)?;
    let path = |engine: &Engine| {
        engine
            .project()
            .interface
            .published
            .as_ref()
            .and_then(|r| r.design_hint.clone())
    };
    assert_eq!(path(&engine).as_deref(), Some("shared/board-v2.solidcraft"));
    let reopened = Engine::from_file(format::decode(&format::encode(&engine.to_file())?)?)?;
    assert_eq!(path(&reopened), path(&engine));
    // A host that does not say where keeps the last path.
    let (instance, ..) = first_placed(&engine)?;
    move_by(&mut engine, instance, [2.0, 0.0])?;
    let again = record_at(&engine, None)?;
    apply(&mut engine, again)?;
    assert_eq!(path(&engine).as_deref(), Some("shared/board-v2.solidcraft"));

    let before = engine.project().clone();
    for bad in ["", "  ", "a\nb", &"x".repeat(4097)] {
        let refused = record_at(&engine, Some(bad))?;
        assert!(apply(&mut engine, refused).is_err(), "{bad:?}");
        assert_eq!(*engine.project(), before);
    }
    let mut table = engine.project().interface.clone();
    if let Some(r) = table.published.as_mut() {
        r.design_hint = Some("a\u{7}b".into());
    }
    assert!(check_table(&table).is_err());
    Ok(())
}
