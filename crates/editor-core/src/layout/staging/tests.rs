use super::*;
use crate::Engine;
use crate::layout::{LayoutCommand, apply, sync_preview};

/// The showcase with every other part taken off the board, so Update PCB
/// has parts of several kinds to stage beside parts that stay.
fn half_placed() -> Result<Project> {
    let mut project = Engine::showcase()?.project().clone();
    project.placements = project.placements.iter().step_by(2).cloned().collect();
    Ok(project)
}

fn assert_staged(project: &Project, before: &[Placement], new: &[Id]) {
    assert_eq!(
        &project.placements[..before.len()],
        before,
        "placed parts never move"
    );
    let boxes = part_boxes(project);
    let board = project.board();
    let outline = board.bounds();
    for id in new {
        let b = boxes.get(id).copied();
        assert!(b.is_some(), "staged part {id} has a box");
        let Some(b) = b else { continue };
        assert!(
            b.min[0] >= outline.max[0] + BOARD_MARGIN_MM - 1e-9,
            "{id} lies beside the board: {b:?} vs {outline:?}"
        );
        for corner in b.corners() {
            assert!(!board.contains(corner, 0.01), "{id} corner on the board");
        }
        let at = project.placement_of(*id).map(|p| p.position);
        assert!(at.is_some_and(|at| {
            at.iter()
                .all(|v| (WORKING_AREA_MM[0]..=WORKING_AREA_MM[1]).contains(v))
        }));
        // No staged part comes within the gap of any other part, staged
        // or placed.
        for (other, o) in &boxes {
            if other != id {
                assert!(
                    !overlaps(&b.inflated(GAP_MM - 1e-6), o),
                    "{id} {b:?} keeps {GAP_MM} mm from {other} {o:?}"
                );
            }
        }
    }
}

/// Update PCB stages new parts right of the board, clear of each other and
/// of the parts already placed, which keep their places.
#[test]
fn update_pcb_stages_new_parts_beside_the_board_without_overlap() -> Result<()> {
    let mut project = half_placed()?;
    let before = project.placements.clone();
    let missing = sync_preview(&project);
    assert!(missing.len() >= 4, "{} parts to stage", missing.len());
    apply(&mut project, LayoutCommand::Sync)?;
    assert!(sync_preview(&project).is_empty());
    assert_eq!(project.placements.len(), before.len() + missing.len());
    assert_staged(&project, &before, &missing);
    Ok(())
}

/// Parts already beside the board (dragged off it earlier) are obstacles:
/// new parts go around them, and they stay where they are.
#[test]
fn staging_avoids_parts_already_beside_the_board() -> Result<()> {
    let mut project = half_placed()?;
    let board = project.board().bounds();
    // Park three kept parts where staging would start.
    let first = board.max[0] + BOARD_MARGIN_MM + 3.0;
    for (k, placement) in project.placements.iter_mut().take(3).enumerate() {
        let k = k as f64;
        placement.position = [first + 6.0 * k, board.max[1] - 3.0 - 4.0 * k];
    }
    let before = project.placements.clone();
    let missing = sync_preview(&project);
    apply(&mut project, LayoutCommand::Sync)?;
    assert_staged(&project, &before, &missing);
    Ok(())
}

/// Each kind of part starts its own row, in designator order.
#[test]
fn each_kind_starts_a_row_in_designator_order() -> Result<()> {
    let mut project = half_placed()?;
    let missing = sync_preview(&project);
    apply(&mut project, LayoutCommand::Sync)?;
    let boxes = part_boxes(&project);
    let mut rows: Vec<(String, u64, Bounds)> = missing
        .iter()
        .filter_map(|id| {
            let (prefix, number, _) =
                crate::instances::designator_key(&project.instance(*id)?.designator);
            Some((prefix, number, *boxes.get(id)?))
        })
        .collect();
    assert_eq!(rows.len(), missing.len());
    let kinds: std::collections::BTreeSet<&String> = rows.iter().map(|r| &r.0).collect();
    assert!(kinds.len() >= 2, "{kinds:?}");
    rows.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    for pair in rows.windows(2) {
        let [(pa, _, a), (pb, _, b)] = pair else {
            continue;
        };
        if pa == pb {
            // Same kind: to the right in the same row, or in a row below.
            assert!(
                b.min[0] > a.max[0] || b.max[1] < a.min[1],
                "{pa}: {a:?} {b:?}"
            );
        } else {
            assert!(b.max[1] < a.min[1], "{pb} starts a row below {pa}");
        }
    }
    Ok(())
}
