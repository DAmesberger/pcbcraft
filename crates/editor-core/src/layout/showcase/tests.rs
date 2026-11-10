use super::BOARD_MM;
use crate::diagnostics::{Error, Result};
use crate::layout::{self, LayoutCommand};
use crate::library::{DrawingLayer, Item, TextContent};
use crate::model::{CopperLayer, Layer, Stackup};
use crate::{Engine, Operation, erc, format, instances};

fn net(engine: &Engine, name: &str) -> Result<crate::Id> {
    engine
        .project()
        .nets
        .iter()
        .find(|n| n.name == name)
        .map(|n| n.id)
        .ok_or_else(|| Error::NotFound(name.into()))
}

#[test]
fn the_showcase_is_a_routed_four_layer_board_with_tented_and_open_vias() -> Result<()> {
    let engine = Engine::showcase()?;
    let project = engine.project();
    assert_eq!(project.stackup, Stackup::preset(4)?);
    assert_eq!(project.board_size, BOARD_MM);
    assert_eq!(project.placements.len(), 9);
    assert!(project.placements.iter().any(|p| p.layer == Layer::Bottom));
    for layer in project.stackup.layers() {
        assert!(
            project.tracks.iter().any(|t| t.layer == layer),
            "no track on {}",
            layer.name()
        );
    }
    assert!(project.vias.len() >= 10);
    assert!(project.vias.iter().any(|v| v.tented));
    assert!(project.vias.iter().filter(|v| !v.tented).count() >= 3);
    // Wide ground traces under a bottom pour and an inner ground plane.
    let gnd = net(&engine, "GND")?;
    let pours: Vec<_> = project.fills.iter().map(|f| (f.net, f.layer)).collect();
    assert_eq!(
        pours,
        [
            (Some(gnd), CopperLayer::Bottom),
            (Some(gnd), CopperLayer::Inner(1))
        ]
    );
    // Stitching vias on the ground traces tie the pours together.
    assert!(project.vias.iter().filter(|v| v.net == gnd).count() >= 15);
    assert!(
        project
            .tracks
            .iter()
            .any(|t| t.net == gnd && t.layer == CopperLayer::Bottom && t.width >= 1.0)
    );
    // Fully routed and clean: no board, via or electrical findings, no
    // airwires. Connectivity runs through the vias.
    let checks = layout::checks(project);
    assert!(checks.is_empty(), "{checks:#?}");
    assert!(layout::airwires(project).is_empty());
    let erc = erc::checks(project);
    assert!(erc.is_empty(), "{erc:#?}");
    // Built as one document with no undo history.
    assert!(!engine.can_undo());
    Ok(())
}

#[test]
fn the_showcase_connects_through_its_vias() -> Result<()> {
    let mut engine = Engine::showcase()?;
    let scl = net(&engine, "SCL")?;
    // The open SCL via joins the top run to the bottom run up to R2.
    let via = engine
        .project()
        .vias
        .iter()
        .find(|v| v.net == scl && !v.tented)
        .map(|v| v.id)
        .ok_or_else(|| Error::NotFound("open SCL via".into()))?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::DeleteVia { id: via }),
    )?;
    let airwires = layout::airwires(engine.project());
    assert_eq!(airwires.len(), 1);
    assert!(airwires.iter().all(|(a, b)| a.net == scl && b.net == scl));
    assert!(
        layout::checks(engine.project())
            .iter()
            .any(|f| f.code == "board.unrouted")
    );
    Ok(())
}

#[test]
fn the_showcase_footprints_carry_silkscreen_outlines_designators_and_pin_one() -> Result<()> {
    let engine = Engine::showcase()?;
    let project = engine.project();
    for placement in &project.placements {
        let instance = project
            .instance(placement.instance)
            .ok_or_else(|| Error::NotFound("instance".into()))?;
        let physical = instances::resolve(&project.library, instance)?
            .physical
            .ok_or_else(|| Error::NotFound("physical".into()))?;
        let footprint = &physical.footprint.definition;
        let silk = |g: &&crate::library::FootprintGraphic| g.layer == DrawingLayer::Silkscreen;
        // Small chips have no outline left once it is broken around their
        // pads; the packages and the tantalum keep theirs.
        let marked = ["J1", "U1", "U2", "C3"].contains(&instance.designator.as_str());
        assert_eq!(
            footprint
                .graphics
                .iter()
                .filter(silk)
                .any(|g| matches!(&g.item, Item::Shape { .. })),
            marked,
            "{} silkscreen outline",
            instance.designator
        );
        assert!(
            footprint
                .graphics
                .iter()
                .filter(silk)
                .any(|g| matches!(&g.item, Item::Text(t) if t.content == TextContent::Designator)),
            "{} has no silkscreen designator",
            instance.designator
        );
        assert_eq!(
            footprint.polarity.is_some(),
            marked,
            "{} pin-1 marking",
            instance.designator
        );
    }
    // Silkscreen lands on both sides: the bottom-side resistor's designator
    // is printed on the bottom.
    let drawing = layout::placed_graphics(project);
    for side in [Layer::Top, Layer::Bottom] {
        assert!(drawing.iter().any(|g| {
            g.layer == layout::BoardDrawing::Silkscreen(side)
                && matches!(&g.item, layout::PlacedItem::Text(_))
        }));
    }
    Ok(())
}

#[test]
fn the_showcase_saves_and_reopens_unchanged() -> Result<()> {
    let engine = Engine::showcase()?;
    let file = engine.to_file();
    let text = serde_json::to_string(&file).map_err(|e| Error::Invalid(format!("encode: {e}")))?;
    let decoded: format::EditorFile =
        serde_json::from_str(&text).map_err(|e| Error::Invalid(format!("decode: {e}")))?;
    let reopened = Engine::from_file(decoded)?;
    assert_eq!(reopened.project(), engine.project());
    assert_eq!(reopened.project().vias, engine.project().vias);
    assert_eq!(reopened.project().stackup, Stackup::preset(4)?);
    Ok(())
}

/// Demo run 2, findings 7 and 11: the showcase has its own name, and its
/// pours follow the board, so a wider board refills to the new edge.
#[test]
fn the_showcase_is_named_and_its_pours_follow_a_wider_board() -> Result<()> {
    use crate::layout::fill::{FillCommand, RefillTarget};
    use crate::model::fill::FillRegion;
    let mut engine = Engine::showcase()?;
    assert_eq!(engine.project().name, super::PROJECT_NAME);
    assert!(!engine.project().fills.is_empty());
    assert!(
        engine
            .project()
            .fills
            .iter()
            .all(|f| f.region == FillRegion::Board),
        "every showcase pour follows the board"
    );
    // The right edge of a fill's copper, mm.
    let reach = |engine: &Engine| -> Vec<f64> {
        engine
            .project()
            .fills
            .iter()
            .map(|f| {
                f.computed
                    .iter()
                    .flat_map(|c| &c.regions)
                    .flat_map(|r| &r.outer)
                    .map(|p| p[0])
                    .fold(f64::NEG_INFINITY, f64::max)
            })
            .collect()
    };
    let [w, h] = BOARD_MM;
    assert!(
        reach(&engine).iter().all(|x| *x < w),
        "{:?}",
        reach(&engine)
    );
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetBoardSize { size: [w + 8.0, h] }),
    )?;
    engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Refill {
            target: RefillTarget::All,
        }),
    )?;
    let after = reach(&engine);
    assert!(
        after.iter().all(|x| *x > w + 6.0 && *x < w + 8.0),
        "the refilled pours reach the new right edge: {after:?}"
    );
    Ok(())
}
