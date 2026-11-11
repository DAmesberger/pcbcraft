use super::*;
use crate::geometry::distance;
use crate::library::ApertureId;
use crate::library::{
    CornerMask, FootprintGraphic, Hole, HoleShape, Outline, PadKind, PadShape, PadVia, Segment,
    designator_placeholder,
};
use crate::tests::support::{by_designator, nm, pt, shape_nm};
use crate::{Engine, Error, Result};

/// The demo board with R1 at `position`, turned `rotation_deg`, on `layer`,
/// and its footprint (shared with R2) replaced by `edit`. Exact snapshots
/// are immutable through commands, so the stored footprint is substituted
/// directly to exercise the placed queries alone.
fn board(
    position: Point,
    rotation_deg: f64,
    layer: Layer,
    edit: impl FnOnce(&mut crate::library::FootprintDefinition),
) -> Result<(Project, Id)> {
    let engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let mut project = engine.project().clone();
    edit(&mut project.library.footprints[0].definition);
    let placement = project
        .placements
        .iter_mut()
        .find(|p| p.instance == r1)
        .ok_or_else(|| Error::NotFound("R1 placement".into()))?;
    placement.position = position;
    placement.rotation_deg = rotation_deg;
    placement.layer = layer;
    Ok((project, r1))
}

fn first_pad(project: &Project, instance: Id) -> Result<PlacedPad> {
    placed_pads(project)
        .into_iter()
        .find(|p| p.instance == instance)
        .ok_or_else(|| Error::NotFound("placed pad".into()))
}

/// A pad shape as the PCB view draws it, in its pad frame.
fn pad_outline(shape: PadShape) -> Shape {
    shape.outline(PAD_ARC_TOLERANCE_MM)
}

fn close(a: Point, b: Point) -> bool {
    distance(a, b) < 1e-9
}

#[test]
fn rotated_rect_pad_on_the_bottom_lands_where_the_transform_puts_it() -> Result<()> {
    let (project, r1) = board([10.0, 20.0], 90.0, Layer::Bottom, |f| {
        f.datum.offset = [0.0, 0.0];
        f.pads[0].position = [2.0, 0.0];
        f.pads[0].rotation_deg = 90.0;
        f.pads[0].shape = PadShape::Rect { size: [2.0, 1.0] };
    })?;
    let pad = first_pad(&project, r1)?;
    // Bottom flip keeps x, then 90° turns (2, 0) to (0, 2).
    assert!(close(pad.center(), [10.0, 22.0]));
    // Pad turn 90° then flip (reverses it) then +90°: the long side runs
    // along x again.
    let ring = pad.outline().flatten(0.01);
    assert_eq!(ring.len(), 4);
    let b = crate::geometry::Bounds::of(ring).ok_or_else(|| Error::Invalid("ring".into()))?;
    assert!(close(b.size(), [2.0, 1.0]), "{b:?}");
    assert!(pad.contains([10.9, 22.4]));
    assert!(!pad.contains([10.0, 22.6]));
    assert!(pad.on_copper(Layer::Bottom) && !pad.on_copper(Layer::Top));
    Ok(())
}

#[test]
fn custom_outline_arcs_flatten_on_their_circle_and_bulge_the_right_way() -> Result<()> {
    // A half disc: up the y-axis, then a counter-clockwise half circle
    // through (-1, 0) back to the start.
    let outline = Outline {
        start: [0.0, -1.0],
        segments: vec![
            Segment::Line { to: [0.0, 1.0] },
            Segment::Arc {
                to: [0.0, -1.0],
                bulge: 1.0,
            },
        ],
    };
    let shape = pad_outline(PadShape::Custom { outline });
    let ring = shape.flatten(0.01);
    assert!(ring.len() > 8);
    for p in &ring {
        assert!(p[0] <= 1e-12, "the arc bulges left: {p:?}");
        let d = distance(*p, [0.0, 0.0]);
        assert!(p[0].abs() < 1e-12 || (d - 1.0).abs() < 1e-9, "{p:?}");
    }
    // Chord error stays within the display tolerance.
    for w in ring.windows(2).skip(1) {
        let mid = [(w[0][0] + w[1][0]) / 2.0, (w[0][1] + w[1][1]) / 2.0];
        assert!(1.0 - distance(mid, [0.0, 0.0]) <= PAD_ARC_TOLERANCE_MM + 1e-12);
    }
    assert_eq!(shape.distance([-0.5, 0.0], true), 0.0);
    assert!(shape.distance([0.5, 0.0], true) > 0.0);
    // A clockwise bulge puts the arc on the other side.
    let cw = pad_outline(PadShape::Custom {
        outline: Outline {
            start: [0.0, -1.0],
            segments: vec![
                Segment::Line { to: [0.0, 1.0] },
                Segment::Arc {
                    to: [0.0, -1.0],
                    bulge: -1.0,
                },
            ],
        },
    });
    assert_eq!(cw.distance([0.5, 0.0], true), 0.0);
    Ok(())
}

#[test]
fn rounded_shapes_follow_their_corners_and_mask() {
    let oblong = pad_outline(PadShape::Oblong { size: [2.0, 1.0] });
    assert_eq!(oblong.distance([0.9, 0.0], true), 0.0);
    assert!(oblong.distance([0.97, 0.47], true) > 0.0, "corner is round");
    let d = pad_outline(PadShape::RoundRect {
        size: [2.0, 1.0],
        radius_ratio: 0.5,
        max_radius: None,
        corners: CornerMask {
            top_left: false,
            top_right: true,
            bottom_right: true,
            bottom_left: false,
        },
    });
    assert!(d.distance([0.97, 0.47], true) > 0.0, "right corners round");
    assert_eq!(d.distance([-0.97, 0.47], true), 0.0, "left corners sharp");
    let capped = pad_outline(PadShape::RoundRect {
        size: [2.0, 1.0],
        radius_ratio: 0.5,
        max_radius: Some(0.1),
        corners: CornerMask::ALL,
    });
    assert_eq!(capped.distance([0.97, 0.47], true), 0.0, "radius capped");
    let circle = pad_outline(PadShape::Circle { diameter: 1.0 });
    assert!(circle.distance([0.36, 0.36], true) > 0.0);
}

#[test]
fn through_hole_pads_span_every_layer_with_their_slot_and_vias() -> Result<()> {
    let (project, r1) = board([10.0, 10.0], 90.0, Layer::Top, |f| {
        f.datum.offset = [0.0, 0.0];
        let pad = &mut f.pads[0];
        pad.position = [0.0, 0.0];
        pad.kind = PadKind::PlatedThrough;
        pad.shape = PadShape::Oblong { size: [3.0, 1.5] };
        pad.hole = Some(Hole {
            shape: HoleShape::Slot {
                width: 0.8,
                length: 2.0,
                angle_deg: 0.0,
            },
            plated: true,
            offset: [0.0, 0.0],
        });
        pad.vias = vec![PadVia {
            id: ApertureId(1),
            at: [1.0, 0.0],
            drill: 0.3,
        }];
        f.pads[1].kind = PadKind::NonPlatedHole;
        f.pads[1].hole = Some(Hole {
            shape: HoleShape::Round { diameter: 1.0 },
            plated: false,
            offset: [0.0, 0.0],
        });
    })?;
    let pads: Vec<PlacedPad> = placed_pads(&project)
        .into_iter()
        .filter(|p| p.instance == r1)
        .collect();
    let [pth, npth] = &pads[..] else {
        return Err(Error::Invalid("two pads expected".into()));
    };
    assert!(pth.on_copper(Layer::Top) && pth.on_copper(Layer::Bottom));
    assert!(!npth.on_copper(Layer::Top) && !npth.on_copper(Layer::Bottom));
    let holes = pth.holes();
    assert_eq!(holes.len(), 2);
    assert!(holes.iter().all(|h| h.plated));
    // The slot turns with the placement: long along y.
    let slot = crate::geometry::Bounds::of(holes[0].shape.flatten(0.001))
        .ok_or_else(|| Error::Invalid("slot".into()))?;
    assert!((slot.size()[1] - 2.0).abs() < 1e-3 && (slot.size()[0] - 0.8).abs() < 1e-3);
    let Shape::Circle { center, radius } = holes[1].shape else {
        return Err(Error::Invalid("via is a circle".into()));
    };
    assert!(close(center, [10.0, 11.0]) && (radius - 0.15).abs() < 1e-12);
    assert!(!npth.holes()[0].plated);
    Ok(())
}

#[test]
fn pad_picking_is_topmost_first_and_respects_visibility() -> Result<()> {
    let (project, r1) = board([10.0, 10.0], 0.0, Layer::Top, |_| {})?;
    let pads = placed_pads(&project);
    let pad = first_pad(&project, r1)?;
    let at = pad.center();
    let picked = pick_pad(&pads, at, |_| true).ok_or_else(|| Error::NotFound("pick".into()))?;
    assert_eq!((picked.instance, picked.pad.id), (r1, pad.pad.id));
    assert!(pick_pad(&pads, at, |p| p.on_copper(Layer::Bottom)).is_none());
    assert!(pick_pad(&pads, [-50.0, -50.0], |_| true).is_none());
    Ok(())
}

#[test]
fn graphics_place_mirrored_on_the_bottom_and_pick_by_stroke_and_text_box() -> Result<()> {
    let (project, r1) = board([10.0, 10.0], 0.0, Layer::Bottom, |f| {
        f.datum.offset = [0.0, 0.0];
        f.graphics = vec![
            FootprintGraphic {
                id: crate::GraphicId(10),
                layer: DrawingLayer::Silkscreen,
                side: Side::Component,
                item: Item::Shape {
                    shape: shape_nm(Shape::Line {
                        a: [-1.0, 1.0],
                        b: [1.0, 1.0],
                    }),
                    width_nm: nm(0.15),
                    fill: Fill::None,
                },
            },
            designator_placeholder(
                crate::GraphicId(11),
                pt([0.0, 1.5]),
                DrawingLayer::Silkscreen,
            ),
            FootprintGraphic {
                id: crate::GraphicId(12),
                layer: DrawingLayer::Courtyard,
                side: Side::Opposite,
                item: Item::Shape {
                    shape: shape_nm(Shape::rect([-2.0, -2.0], [2.0, 2.0])),
                    width_nm: nm(0.05),
                    fill: Fill::None,
                },
            },
        ];
    })?;
    let graphics: Vec<PlacedGraphic> = placed_graphics(&project)
        .into_iter()
        .filter(|g| g.instance == r1)
        .collect();
    assert_eq!(graphics.len(), 3);
    assert_eq!(graphics[0].layer, BoardDrawing::Silkscreen(Layer::Bottom));
    assert_eq!(graphics[2].layer, BoardDrawing::Courtyard(Layer::Top));
    let PlacedItem::Text(text) = &graphics[1].item else {
        return Err(Error::Invalid("designator is text".into()));
    };
    assert_eq!(text.text, "R1");
    assert!(text.mirrored);
    assert!(close(text.at, [10.0, 8.5]), "{:?}", text.at);
    let m = TextMetric::default();
    // The silk line is mirrored to y = 9 on the bottom.
    let line = pick_graphic(&graphics, [10.5, 9.05], 0.0, m, |_| true)
        .ok_or_else(|| Error::NotFound("line pick".into()))?;
    assert_eq!(line.graphic, crate::GraphicId(10));
    assert!(
        pick_graphic(&graphics, [10.5, 11.0], 0.0, m, |g| g.graphic
            != crate::GraphicId(12))
        .is_none()
    );
    let label = pick_graphic(&graphics, [10.0, 8.5], 0.0, m, |_| true)
        .ok_or_else(|| Error::NotFound("text pick".into()))?;
    assert_eq!(label.graphic, crate::GraphicId(11));
    // Hidden layers do not pick; the courtyard picks on its stroke only.
    assert!(
        pick_graphic(&graphics, [10.0, 8.5], 0.0, m, |g| !matches!(
            g.layer,
            BoardDrawing::Silkscreen(_)
        ))
        .is_none()
    );
    let courtyard = pick_graphic(&graphics, [12.0, 10.0], 0.01, m, |_| true)
        .ok_or_else(|| Error::NotFound("courtyard pick".into()))?;
    assert_eq!(courtyard.graphic, crate::GraphicId(12));
    Ok(())
}

/// A bare hole has no copper on any layer, even when its footprint still
/// lands a terminal on it (an error the footprint check reports). One rule
/// (`layout::pad_copper_on`) answers for the placed pad, which fill
/// compute and fabrication read, and for the connected pads, which the
/// copper graph reads. The graph used to count such a hole as a land on
/// its side, where fill compute saw no copper.
#[test]
fn a_bare_hole_has_no_copper_on_any_layer() -> Result<()> {
    let (project, r1) = board([10.0, 10.0], 0.0, Layer::Top, |f| {
        f.pads[1].kind = PadKind::NonPlatedHole;
        f.pads[1].hole = Some(Hole {
            shape: HoleShape::Round { diameter: 1.0 },
            plated: false,
            offset: [0.0, 0.0],
        });
    })?;
    let hole = placed_pads(&project)
        .into_iter()
        .find(|p| p.instance == r1 && p.pad.kind == PadKind::NonPlatedHole)
        .ok_or_else(|| Error::NotFound("the hole".into()))?;
    assert!(
        hole.net.is_some(),
        "the hole still carries its terminal's net"
    );
    for layer in project.stackup.layers() {
        assert!(!hole.on_copper(layer), "{layer:?}");
    }
    let is_hole = |w: &crate::layout::WorldPad| w.instance == r1 && w.pad == hole.pad.id;
    assert!(!crate::layout::pads(&project).iter().any(is_hole));
    let graph = crate::layout::CopperGraph::of(&project);
    assert!(
        !graph.pads().iter().any(is_hole),
        "the graph has no land for it"
    );
    Ok(())
}
