use super::*;
use crate::diagnostics::Result;
use crate::ids::GraphicId;
use crate::layout::{LayoutCommand, apply};
use crate::library::graphics::{DrawingLayer, Fill, FootprintGraphic, Item, Side};
use crate::tests::support::{by_designator, nm, shape_nm};
use crate::{Engine, Error};

/// The demo board with every footprint given a 4 × 2 mm courtyard around
/// its datum, and R1 and R2 placed `gap` mm apart (centre to centre) on
/// `layers`.
fn board(gap: f64, layers: [Layer; 2]) -> Result<(Project, Id, Id)> {
    let engine = Engine::demo()?;
    let (r1, r2) = (by_designator(&engine, "R1")?, by_designator(&engine, "R2")?);
    let mut project = engine.project().clone();
    // Exact snapshots are immutable through commands; the fixture
    // substitutes stored footprints to exercise the board check alone.
    for footprint in &mut project.library.footprints {
        let fp = &mut footprint.definition;
        let id = GraphicId(fp.next_local);
        fp.next_local += 1;
        let [x, y] = fp.datum.offset();
        fp.graphics.push(FootprintGraphic {
            id,
            layer: DrawingLayer::Courtyard,
            side: Side::Component,
            item: Item::Shape {
                shape: shape_nm(Shape::rect([x - 2.0, y - 1.0], [x + 2.0, y + 1.0])),
                width_nm: nm(0.05),
                fill: Fill::None,
            },
        });
    }
    for (instance, x, layer) in [(r1, 10.0, layers[0]), (r2, 10.0 + gap, layers[1])] {
        apply(
            &mut project,
            LayoutCommand::Move {
                instance,
                position: [x, 10.0],
                rotation_deg: 0.0,
                layer,
            },
        )?;
    }
    Ok((project, r1, r2))
}

fn overlaps(project: &Project) -> Vec<Finding> {
    findings(project)
        .into_iter()
        .filter(|f| f.code == COURTYARD_OVERLAP)
        .collect()
}

#[test]
fn overlapping_courtyards_on_one_side_are_reported_once_per_pair() -> Result<()> {
    let (project, r1, r2) = board(3.0, [Layer::Top, Layer::Top])?;
    let found = overlaps(&project);
    assert_eq!(found.len(), 1, "{found:?}");
    let f = found
        .first()
        .ok_or_else(|| Error::NotFound("finding".into()))?;
    assert!(f.objects.contains(&ObjectRef::Instance { id: r1 }));
    assert!(f.objects.contains(&ObjectRef::Instance { id: r2 }));
    assert!(
        f.message.contains("R1") && f.message.contains("R2"),
        "{}",
        f.message
    );
    // The board checks report it too.
    assert!(
        crate::layout::checks(&project, &Default::default())
            .iter()
            .any(|f| f.code == COURTYARD_OVERLAP)
    );
    Ok(())
}

#[test]
fn abutting_separate_and_opposite_side_courtyards_pass() -> Result<()> {
    // Exactly touching edges (4 mm wide yards, 4 mm apart) is not overlap.
    for (gap, layers) in [
        (4.0, [Layer::Top, Layer::Top]),
        (6.0, [Layer::Top, Layer::Top]),
        (3.0, [Layer::Top, Layer::Bottom]),
        (6.0, [Layer::Bottom, Layer::Bottom]),
    ] {
        let (project, _, _) = board(gap, layers)?;
        assert!(overlaps(&project).is_empty(), "{gap} {layers:?}");
    }
    let (project, _, _) = board(1.0, [Layer::Bottom, Layer::Bottom])?;
    let found = overlaps(&project);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found.iter().all(|f| f.message.contains("bottom")));
    Ok(())
}

#[test]
fn parts_without_a_courtyard_are_not_checked() -> Result<()> {
    let (mut project, _, _) = board(0.0, [Layer::Top, Layer::Top])?;
    for footprint in &mut project.library.footprints {
        footprint
            .definition
            .graphics
            .retain(|g| g.layer != DrawingLayer::Courtyard);
    }
    assert!(placed_courtyards(&project).is_empty());
    assert!(overlaps(&project).is_empty());
    Ok(())
}

/// Reaching either bound says so instead of passing silently.
#[test]
fn courtyard_bounds_report_a_limit_finding() -> Result<()> {
    let (project, _, _) = board(3.0, [Layer::Top, Layer::Top])?;
    let limits = |found: &[Finding]| found.iter().filter(|f| f.code == COURTYARD_LIMIT).count();
    assert_eq!(limits(&findings(&project)), 0, "within bounds");
    let few_yards = bounded_findings(&project, 1, MAX_OVERLAP_FINDINGS);
    assert_eq!(limits(&few_yards), 1, "{few_yards:?}");
    let few_findings = bounded_findings(&project, MAX_CHECKED_COURTYARDS, 0);
    assert_eq!(limits(&few_findings), 1, "{few_findings:?}");
    assert!(few_findings.iter().all(|f| f.code != COURTYARD_OVERLAP));
    Ok(())
}
