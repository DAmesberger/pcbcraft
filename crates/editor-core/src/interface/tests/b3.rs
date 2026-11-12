//! Slice B3 in the core: the sidecar a publish writes, the revision it
//! records and the publish preview's diff (`interface::publish`), on the
//! demo board with an external opening on its package. The exported design
//! itself, the files and the enclosure that follows them are tested where
//! Solidcraft runs (`pcbcraft-solidcraft` `interface` and the automation
//! session's `interface_publish_tests`).
use super::Fallible;
use super::package_datums::{demo, usb4105_opening};
use crate::ids::{Id, InterfaceSourceId};
use crate::interface::publish::{candidate, preview, record, summary};
use crate::interface::report::{ChangeKind, PublishPreview};
use crate::interface::{InterfaceCommand, Sidecar, Vec3};
use crate::layout::LayoutCommand;
use crate::library::datum::PackageDatumOrigin;
use crate::library::package_model::{PackageEdit, edit_package};
use crate::model::interface::Tier;
use crate::{Engine, InstanceCommand, Operation};

const SOURCE: InterfaceSourceId = InterfaceSourceId(0x005e_edb3);

/// The demo with an authored external opening on its package, opened as
/// a file so its interface starts empty: each placement exports one
/// opening.
fn with_openings() -> Fallible<Engine> {
    let mut project = demo()?;
    let entry = project.library.packages.first_mut().ok_or("package")?;
    let opening = crate::library::datum::PackageDatum {
        origin: PackageDatumOrigin::Authored,
        ..usb4105_opening()
    };
    entry.definition = edit_package(&entry.definition, &[PackageEdit::AddDatum(opening)])?;
    project.interface = Default::default();
    let mut file = Engine::demo()?.to_file();
    file.project = project;
    Ok(Engine::from_file(file)?)
}

/// Publish the document as the host does: the candidate sidecar, then
/// `interface.record_publish` with its record. Returns the sidecar.
fn publish(engine: &mut Engine, tier: Tier) -> Fallible<Sidecar> {
    let sidecar = candidate(engine.project(), tier, SOURCE)?;
    let record = record(engine.project(), &sidecar)?;
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

fn first_placed(engine: &Engine) -> Fallible<(Id, [f64; 2], f64, crate::Layer)> {
    let p = engine.project().placements.first().ok_or("placement")?;
    Ok((p.instance, p.position, p.rotation_deg, p.layer))
}

fn move_by(engine: &mut Engine, instance: Id, by: [f64; 2]) -> Fallible {
    let p = engine
        .project()
        .placement_of(instance)
        .ok_or("placement")?
        .clone();
    let revision = engine.revision();
    engine.apply(
        revision,
        Operation::Layout(LayoutCommand::Move {
            instance,
            position: [p.position[0] + by[0], p.position[1] + by[1]],
            rotation_deg: p.rotation_deg,
            layer: p.layer,
        }),
    )?;
    Ok(())
}

fn changed(preview: &PublishPreview) -> Vec<&crate::interface::DatumChange> {
    preview
        .changes
        .iter()
        .filter(|c| c.change != ChangeKind::Unchanged)
        .collect()
}

/// Acceptance 1 and 2, the diff: J1 moved 2 mm along X by `layout.move`;
/// the preview, made against the published sidecar, lists exactly one
/// changed datum, the part's opening, moved by (2, 0, 0) without a turn,
/// and reads "… opening, 2 mm along X".
pub(super) fn the_preview_lists_the_one_moved_datum() -> Fallible {
    let mut engine = with_openings()?;
    let published = publish(&mut engine, Tier::Interface)?;
    let (instance, ..) = first_placed(&engine)?;
    let designator = engine
        .project()
        .instance(instance)
        .ok_or("instance")?
        .designator
        .clone();
    move_by(&mut engine, instance, [2.0, 0.0])?;
    let preview = preview(engine.project(), None, Some(&published), engine.revision())?;
    assert_eq!(preview.from, Some(1));
    assert_eq!(preview.to, 2);
    assert_eq!(preview.tier, Tier::Interface, "the last published tier");
    let moved = changed(&preview);
    let [only] = moved.as_slice() else {
        return Err(format!("{} changed datums: {moved:?}", moved.len()).into());
    };
    assert_eq!(only.name, format!("{designator} opening"));
    let ChangeKind::Moved { offset, turn_deg } = &only.change else {
        return Err(format!("not a move: {only:?}").into());
    };
    for (axis, want) in offset.0.iter().zip([2.0, 0.0, 0.0]) {
        assert!((axis - want).abs() <= 1e-9, "{offset:?}");
    }
    assert_eq!(*turn_deg, 0.0);
    assert_eq!(only.summary, format!("{designator} opening, 2 mm along X"));
    assert_eq!(
        preview.changes.len(),
        published.datums.len(),
        "every datum is listed"
    );

    // Without the published sidecar the datum reads only as changed.
    let blind = super::super::publish::preview(engine.project(), None, None, engine.revision())?;
    let blind = changed(&blind);
    assert_eq!(blind.len(), 1);
    assert_eq!(blind.first().map(|c| &c.change), Some(&ChangeKind::Changed));
    Ok(())
}

/// The revision counts only publishes whose content hash differs: the same
/// board published again keeps revision 1, a rename keeps it (display
/// names are not content), a move makes revision 2; and a record that does
/// not follow the recorded revision is refused.
pub(super) fn publish_records_a_revision_only_when_the_hash_changes() -> Fallible {
    let mut engine = with_openings()?;
    let first = publish(&mut engine, Tier::Interface)?;
    assert_eq!(first.revision, 1);
    let again = candidate(engine.project(), Tier::Mechanical, SOURCE)?;
    assert_eq!(
        (again.revision, again.hash),
        (1, first.hash),
        "tier is not content"
    );

    let (instance, ..) = first_placed(&engine)?;
    let revision = engine.revision();
    engine.apply(
        revision,
        Operation::Instance(InstanceCommand::SetDesignator {
            instance,
            designator: "J9".into(),
        }),
    )?;
    let renamed = candidate(engine.project(), Tier::Interface, SOURCE)?;
    assert_eq!((renamed.revision, renamed.hash), (1, first.hash));
    assert!(
        renamed.datums.iter().any(|d| d.name == "J9 opening"),
        "the sidecar carries the new display name"
    );
    let unchanged = preview(engine.project(), None, Some(&first), engine.revision())?;
    assert!(changed(&unchanged).is_empty(), "{unchanged:?}");

    move_by(&mut engine, instance, [0.0, -1.5])?;
    let second = publish(&mut engine, Tier::Interface)?;
    assert_eq!(second.revision, 2);
    assert_ne!(second.hash, first.hash);

    // A record that skips a revision is refused, changing nothing.
    let mut stale = record(engine.project(), &second)?;
    stale.revision = 4;
    let revision = engine.revision();
    let refused = engine.apply(
        revision,
        Operation::Interface(InterfaceCommand::RecordPublish {
            source_id: SOURCE,
            record: stale,
        }),
    );
    assert!(refused.is_err());
    assert_eq!(engine.revision(), revision);
    // Another board's identity is refused before anything is built.
    assert!(candidate(engine.project(), Tier::Interface, InterfaceSourceId(7)).is_err());
    Ok(())
}

/// Deleting the part reports its opening as removed, by the name it was
/// published under, in its slot; the sidecar lists that slot as retired and
/// keeps every other datum's slot.
#[test]
fn b3_a_deleted_part_is_reported() -> Fallible {
    let mut engine = with_openings()?;
    let published = publish(&mut engine, Tier::Interface)?;
    let (instance, ..) = first_placed(&engine)?;
    let token = format!("i{instance}.opening");
    let gone = published
        .datums
        .iter()
        .find(|d| d.token == token)
        .ok_or("the opening was published")?
        .clone();
    let revision = engine.revision();
    engine.apply_update(
        revision,
        crate::updates::Change::Instance {
            edit: crate::updates::InstanceChange::Remove { instance },
        },
        false,
    )?;
    let preview = preview(engine.project(), None, Some(&published), engine.revision())?;
    let rows = changed(&preview);
    let removed: Vec<_> = rows
        .iter()
        .filter(|c| c.change == ChangeKind::Retired)
        .collect();
    let [row] = removed.as_slice() else {
        return Err(format!("{rows:?}").into());
    };
    assert_eq!((row.datum, row.slot), (gone.id, gone.slot));
    assert_eq!(row.name, gone.name);
    assert!(row.summary.contains("will fail"), "{}", row.summary);
    let next = candidate(engine.project(), Tier::Interface, SOURCE)?;
    assert!(next.retired_slots.contains(&gone.slot));
    for kept in published.datums.iter().filter(|d| d.token != token) {
        let now = next
            .datums
            .iter()
            .find(|d| d.token == kept.token)
            .ok_or("kept")?;
        assert_eq!(now.slot, kept.slot, "{}", kept.token);
    }
    Ok(())
}

/// The sidecar is the same bytes for the same document, and reads back as
/// written.
#[test]
fn b3_the_sidecar_text_reads_back_and_repeats() -> Fallible {
    let engine = with_openings()?;
    let a = candidate(engine.project(), Tier::Interface, SOURCE)?.text()?;
    let reopened = Engine::from_file(engine.to_file())?;
    let b = candidate(reopened.project(), Tier::Interface, SOURCE)?.text()?;
    assert_eq!(a, b);
    assert_eq!(Sidecar::read(&a)?.text()?, a);
    Ok(())
}

#[test]
fn b3_summaries_read_as_the_user_reads_them() {
    let moved = |x, y, z, turn| ChangeKind::Moved {
        offset: Vec3([x, y, z]),
        turn_deg: turn,
    };
    assert_eq!(
        summary("J1 opening", &moved(2.0, 0.0, 0.0, 0.0)),
        "J1 opening, 2 mm along X"
    );
    assert_eq!(
        summary("J1 opening", &moved(0.0, -1.25, 0.0, 0.0)),
        "J1 opening, -1.25 mm along Y"
    );
    assert_eq!(
        summary("J1 opening", &moved(3.0, 4.0, 0.0, 90.0)),
        "J1 opening, 5 mm (3 along X, 4 along Y), turned 90°"
    );
    assert_eq!(summary("H1", &ChangeKind::Unchanged), "H1, unchanged");
}
