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

/// The bottom side height lies under the board: its loop, taken through
/// its frame into board coordinates, is the board outline (vertex for
/// vertex), it turns the same way in its own frame as the top side's loop
/// in its frame, and it runs from the bottom surface (z 0) down to
/// −height.
#[test]
fn the_bottom_side_height_lies_under_the_board_outline() -> Fallible {
    use crate::interface::{DatumGeometry, list};
    use crate::model::interface::DatumRole;
    let mut engine = super::b3::with_openings()?;
    let (instance, position, rotation_deg, _) = first_placed(&engine)?;
    let revision = engine.revision();
    engine.apply(
        revision,
        Operation::Layout(crate::layout::LayoutCommand::Move {
            instance,
            position,
            rotation_deg,
            layer: crate::Layer::Bottom,
        }),
    )?;
    let project = engine.project();
    let table = list::table(project)?;
    let side = |name: &str| -> Fallible<DatumGeometry> {
        let datum = table
            .datums
            .iter()
            .find(|d| d.key.role == DatumRole::SideHeight && d.key.name.as_deref() == Some(name))
            .ok_or(format!("the {name} side height"))?;
        list::derive(project, datum)
            .geometry
            .ok_or_else(|| "geometry".into())
    };
    // A loop's vertices in board coordinates at frame height `z`, and its
    // signed area in the frame's own x–y.
    let world = |g: &DatumGeometry, z: f64| -> Fallible<(Vec<[f64; 3]>, f64)> {
        let DatumGeometry::Volume { frame, loops, .. } = g else {
            return Err("a volume".into());
        };
        let outline = loops.first().ok_or("a loop")?;
        let [o, x, y] = [frame.origin.0, frame.x.0, frame.y.0];
        let n = [
            x[1] * y[2] - x[2] * y[1],
            x[2] * y[0] - x[0] * y[2],
            x[0] * y[1] - x[1] * y[0],
        ];
        let local: Vec<[f64; 2]> = std::iter::once(outline.start)
            .chain(outline.segments.iter().map(crate::library::Segment::to))
            .collect();
        let points: Vec<[f64; 3]> = local
            .iter()
            .map(|p| std::array::from_fn(|i| o[i] + x[i] * p[0] + y[i] * p[1] + n[i] * z))
            .collect();
        let area = local
            .windows(2)
            .map(|w| w[0][0] * w[1][1] - w[1][0] * w[0][1])
            .sum::<f64>()
            / 2.0;
        Ok((points, area))
    };
    let bottom = side("bottom")?;
    let DatumGeometry::Volume { height, .. } = &bottom else {
        return Err("a volume".into());
    };
    assert!(*height > 0.0, "a part on the bottom");
    let outer = &project.board().outer;
    let mut board: Vec<[i64; 2]> = std::iter::once(outer.start)
        .chain(outer.segments.iter().map(crate::library::Segment::to))
        .map(|p| p.map(|v| (v * 1e6).round() as i64))
        .collect();
    let (at_surface, area) = world(&bottom, 0.0)?;
    let (at_depth, _) = world(&bottom, *height)?;
    let mut footprint: Vec<[i64; 2]> = at_surface
        .iter()
        .map(|p| [(p[0] * 1e6).round() as i64, (p[1] * 1e6).round() as i64])
        .collect();
    board.sort_unstable();
    footprint.sort_unstable();
    assert_eq!(footprint, board, "exactly under the board outline");
    assert!(at_surface.iter().all(|p| p[2].abs() <= 1e-9));
    assert!(at_depth.iter().all(|p| (p[2] + height).abs() <= 1e-9));
    // The same turning sense in its frame as the top side's in its own.
    let (_, top_area) = world(&side("top")?, 0.0)?;
    assert!(area * top_area > 0.0, "{area} vs {top_area}");
    Ok(())
}
