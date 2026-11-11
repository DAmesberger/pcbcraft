//! The core's geometry rasterised directly, without the writer: what each
//! layer must show. Copper is every pad with copper on the layer (its exact
//! outline), every track (an exact capsule), every via and plated mounting
//! hole ring (an exact disc) and every computed fill region (outer ring and
//! holes, even-odd). Mask openings and paste follow the resolution rule
//! stated in `fabrication`: an inherited mask expansion or paste reduction
//! is the effective design rule's; explicit pad rules are as authored;
//! a via opens only when untented. The profile is the outer outline less
//! every cutout and slot.
use super::Fallible;
use super::raster::{Bitmap, Shape, arc_points};
use crate::geometry::{Point, Shape as CoreShape};
use crate::layout;
use crate::library::{MaskRule, PadKind, PasteRule};
use crate::model::board::{CutoutKind, flatten};
use crate::model::{CopperLayer, Layer, Project};

/// Chord tolerance of flattened curves here, mm.
pub(crate) const TOLERANCE_MM: f64 = 0.0005;

/// A bitmap over the board's bounds grown by `margin` mm, at `pixel` mm.
pub(crate) fn board_bitmap(project: &Project, margin: f64, pixel: f64) -> Fallible<Bitmap> {
    let b = project.board().bounds();
    Bitmap::new(
        [b.min[0] - margin, b.min[1] - margin],
        [b.max[0] + margin, b.max[1] + margin],
        pixel,
    )
}

/// A core shape as a raster shape: circles stay exact discs, everything
/// else is flattened within [`TOLERANCE_MM`].
pub(crate) fn raster_shape(shape: &CoreShape) -> Shape {
    match shape {
        CoreShape::Circle { center, radius } => Shape::Disc {
            c: *center,
            r: *radius,
        },
        other => Shape::Poly {
            contours: vec![other.flatten(TOLERANCE_MM)],
        },
    }
}

/// The copper of `layer`, dark on `bitmap`.
pub(crate) fn copper(project: &Project, layer: CopperLayer, bitmap: &mut Bitmap) -> Fallible<()> {
    for fill in project.fills.iter().filter(|f| f.layer == layer) {
        let Some(computed) = &fill.computed else {
            continue;
        };
        for region in &computed.regions {
            let mut contours = vec![region.outer.clone()];
            contours.extend(region.holes.iter().cloned());
            bitmap.render(&Shape::Poly { contours }, true);
        }
    }
    for pad in layout::placed_pads(project) {
        if pad.on_copper(layer) {
            bitmap.render(&raster_shape(&pad.outline()), true);
        }
    }
    for track in project.tracks.iter().filter(|t| t.layer == layer) {
        bitmap.render(
            &Shape::Capsule {
                a: track.start,
                b: track.end,
                r: track.width / 2.0,
            },
            true,
        );
    }
    for via in &project.vias {
        bitmap.render(
            &Shape::Disc {
                c: via.position,
                r: via.diameter / 2.0,
            },
            true,
        );
    }
    for hole in &project.board().holes {
        if let (true, Some(d)) = (hole.plated, hole.pad_diameter) {
            bitmap.render(
                &Shape::Disc {
                    c: hole.at,
                    r: d / 2.0,
                },
                true,
            );
        }
    }
    Ok(())
}

/// The solder-mask openings of `side`, dark on `bitmap`.
pub(crate) fn mask(project: &Project, side: Layer, bitmap: &mut Bitmap) -> Fallible<()> {
    let expansion = project.rules.effective().mask.expansion;
    for pad in layout::placed_pads(project) {
        let opens_here = pad.on_copper(side) || pad.pad.kind == PadKind::NonPlatedHole;
        if !opens_here {
            continue;
        }
        let by = match pad.pad.mask {
            MaskRule::Inherit => expansion,
            MaskRule::Tented => continue,
            MaskRule::Expansion(e) => e,
        };
        if let Some(shape) = pad.pad.shape.grown(by, TOLERANCE_MM) {
            bitmap.render(
                &raster_shape(&shape.transformed(&pad.pad_transform())),
                true,
            );
        }
    }
    for via in project.vias.iter().filter(|v| !v.tented) {
        bitmap.render(
            &Shape::Disc {
                c: via.position,
                r: via.diameter / 2.0 + expansion,
            },
            true,
        );
    }
    // Board mounting holes open the mask round their ring (plated) or
    // drill.
    for hole in &project.board().holes {
        let size = if hole.plated {
            hole.pad_diameter.unwrap_or(hole.drill)
        } else {
            hole.drill
        };
        bitmap.render(
            &Shape::Disc {
                c: hole.at,
                r: size / 2.0 + expansion,
            },
            true,
        );
    }
    Ok(())
}

/// The solder-paste openings of `side`, dark on `bitmap`.
pub(crate) fn paste(project: &Project, side: Layer, bitmap: &mut Bitmap) -> Fallible<()> {
    let reduction = project.rules.effective().paste.reduction;
    for pad in layout::placed_pads(project) {
        if pad.layer != side {
            continue;
        }
        let t = pad.pad_transform();
        let shrink = |by: f64| pad.pad.shape.grown(-by, TOLERANCE_MM);
        let shapes: Vec<CoreShape> = match &pad.pad.paste {
            PasteRule::Inherit => match pad.pad.kind {
                PadKind::Smd | PadKind::Mechanical if pad.pad.hole.is_none() => {
                    shrink(reduction).into_iter().collect()
                }
                _ => Vec::new(),
            },
            PasteRule::None => Vec::new(),
            PasteRule::Reduction(r) => shrink(*r).into_iter().collect(),
            PasteRule::Windows(windows) => windows.iter().map(|w| w.shape.to_mm()).collect(),
        };
        for shape in shapes {
            bitmap.render(&raster_shape(&shape.transformed(&t)), true);
        }
    }
    Ok(())
}

/// The board: its outer outline less every cutout and slot, dark.
pub(crate) fn outline(project: &Project, bitmap: &mut Bitmap) -> Fallible<()> {
    let board = project.board();
    let mut contours = vec![flatten(&board.outer, TOLERANCE_MM)];
    contours.extend(
        board
            .cutouts
            .iter()
            .map(|c| flatten(&c.outline, TOLERANCE_MM)),
    );
    bitmap.render(&Shape::Poly { contours }, true);
    Ok(())
}

/// Every drilled hole that is (or is not) plated, dark: vias, pad holes and
/// pad vias, board mounting holes and, when plated, board slots.
pub(crate) fn drills(project: &Project, plated: bool, bitmap: &mut Bitmap) -> Fallible<()> {
    if plated {
        for via in &project.vias {
            bitmap.render(
                &Shape::Disc {
                    c: via.position,
                    r: via.drill / 2.0,
                },
                true,
            );
        }
    }
    for pad in layout::placed_pads(project) {
        for hole in pad.holes().into_iter().filter(|h| h.plated == plated) {
            bitmap.render(&raster_shape(&hole.shape), true);
        }
    }
    let board = project.board();
    for hole in board.holes.iter().filter(|h| h.plated == plated) {
        bitmap.render(
            &Shape::Disc {
                c: hole.at,
                r: hole.drill / 2.0,
            },
            true,
        );
    }
    if plated {
        for cutout in &board.cutouts {
            if cutout.kind == (CutoutKind::Slot { plated: true }) {
                bitmap.render(
                    &Shape::Poly {
                        contours: vec![flatten(&cutout.outline, TOLERANCE_MM)],
                    },
                    true,
                );
            }
        }
    }
    Ok(())
}

/// A circle as its arc points, for tests that need a flattened ring.
pub(crate) fn circle_ring(center: Point, r: f64) -> Vec<Point> {
    let start = [center[0] + r, center[1]];
    arc_points(start, start, center, false)
}
