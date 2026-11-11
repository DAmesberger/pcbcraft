//! Footprint drawing helpers in nm: the courtyard, the silkscreen outline
//! broken around the pads, and the assembly outline, with the same
//! estimated defaults as the host's footprint templates (not a standard
//! compliance claim).
use crate::units::round_to;
use alloc::vec;
use alloc::vec::Vec;
use pcbcraft_generator_abi::v1::{
    Fill, Graphic, HAlign, Opening, Pad, PadShape, Point, Primitive, TextContent, VAlign,
};

/// Courtyard margin around pads and body (estimated default).
pub const COURTYARD_EXCESS_NM: i64 = 250_000;
/// Courtyard coordinates round outward to this grid.
pub const COURTYARD_ROUND_NM: i64 = 50_000;
/// Assembly-drawing stroke.
pub const ASSEMBLY_STROKE_NM: i64 = 100_000;
/// Silkscreen stroke.
pub const SILK_STROKE_NM: i64 = 150_000;
/// How far the silkscreen outline sits outside the body.
pub const SILK_OUTSET_NM: i64 = 150_000;
/// Silkscreen clearance from a pad's mask opening (estimated default;
/// fabricators state their own).
pub const SILK_PAD_CLEARANCE_NM: i64 = 100_000;
/// Silkscreen pieces shorter than this are left out.
pub const MIN_SILK_SEGMENT_NM: i64 = 200_000;
/// The mask expansion a footprint's pads inherit unless they state their
/// own (the host's footprint default, 0.05 mm); the silkscreen keeps clear of it.
pub const DEFAULT_MASK_NM: i64 = 50_000;
/// Footprint text height.
pub const TEXT_HEIGHT_NM: i64 = 1_000_000;
/// Smallest assembly designator height.
pub const MIN_ASSEMBLY_TEXT_NM: i64 = 100_000;

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Box2 {
    pub min: Point,
    pub max: Point,
}

impl Box2 {
    /// The box of size `size` centred on `at`.
    pub fn centred(at: Point, size: Point) -> Self {
        let half = [size[0] / 2, size[1] / 2];
        Self {
            min: [at[0] - half[0], at[1] - half[1]],
            max: [at[0] + (size[0] - half[0]), at[1] + (size[1] - half[1])],
        }
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }

    pub fn inflated(self, by: i64) -> Self {
        Self {
            min: [self.min[0] - by, self.min[1] - by],
            max: [self.max[0] + by, self.max[1] + by],
        }
    }

    pub fn center(self) -> Point {
        [
            self.min[0] + (self.max[0] - self.min[0]) / 2,
            self.min[1] + (self.max[1] - self.min[1]) / 2,
        ]
    }

    pub fn size(self) -> Point {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }

    /// Corners counter-clockwise from the minimum.
    pub fn corners(self) -> [Point; 4] {
        [
            self.min,
            [self.max[0], self.min[1]],
            self.max,
            [self.min[0], self.max[1]],
        ]
    }
}

/// A pad's copper box. Pads rotated by other than a multiple of 90° use the
/// box of their circumscribed circle.
pub fn pad_box(pad: &Pad) -> Box2 {
    let size = match &pad.shape {
        PadShape::Rect { size } | PadShape::RoundRect { size, .. } | PadShape::Oval { size } => {
            *size
        }
        PadShape::Circle { diameter_nm } => [*diameter_nm, *diameter_nm],
        PadShape::Polygon { points } => {
            let reach = points
                .iter()
                .map(|p| p[0].abs().max(p[1].abs()))
                .max()
                .unwrap_or(0);
            [2 * reach, 2 * reach]
        }
    };
    let quarter = pad.rot_mdeg.rem_euclid(90_000) == 0;
    let size = if !quarter {
        let d = size[0].max(size[1]) * 3 / 2;
        [d, d]
    } else if pad.rot_mdeg.rem_euclid(180_000) == 90_000 {
        [size[1], size[0]]
    } else {
        size
    };
    Box2::centred(pad.at, size)
}

/// The courtyard: the box round the pads and `body`, grown by
/// [`COURTYARD_EXCESS_NM`] and rounded outward to [`COURTYARD_ROUND_NM`].
/// `None` when there is nothing to enclose.
pub fn courtyard(pads: &[Pad], body: Option<Box2>) -> Option<Vec<Point>> {
    let boxed = pads
        .iter()
        .map(pad_box)
        .chain(body)
        .reduce(Box2::union)?
        .inflated(COURTYARD_EXCESS_NM);
    let r = |v, up| round_to(v, COURTYARD_ROUND_NM, up);
    let snapped = Box2 {
        min: [r(boxed.min[0], false)?, r(boxed.min[1], false)?],
        max: [r(boxed.max[0], true)?, r(boxed.max[1], true)?],
    };
    Some(snapped.corners().to_vec())
}

/// The silkscreen keep-out of a pad: its box grown by its mask expansion
/// (`default_mask_nm` where the pad follows the project's rule), the
/// clearance and half the stroke.
fn keep_out(pad: &Pad, default_mask_nm: i64) -> Box2 {
    let grow = match pad.mask {
        Opening::Expansion(e) => e,
        Opening::Default => default_mask_nm,
        Opening::None => 0,
    };
    pad_box(pad).inflated(grow.max(0) + SILK_PAD_CLEARANCE_NM + SILK_STROKE_NM / 2)
}

/// The pieces of the horizontal or vertical segment `a`–`b` outside every box.
fn clipped(a: Point, b: Point, boxes: &[Box2]) -> Vec<Primitive> {
    let axis = usize::from((a[0] - b[0]).abs() < (a[1] - b[1]).abs());
    let other = 1 - axis;
    let (lo, hi) = (a[axis].min(b[axis]), a[axis].max(b[axis]));
    let mut cuts: Vec<(i64, i64)> = boxes
        .iter()
        .filter(|k| (k.min[other]..=k.max[other]).contains(&a[other]))
        .map(|k| (k.min[axis], k.max[axis]))
        .collect();
    cuts.sort_unstable();
    let mut pieces = Vec::new();
    let mut from = lo;
    for (c0, c1) in cuts {
        if c0 > from {
            pieces.push((from, c0.min(hi)));
        }
        from = from.max(c1);
        if from >= hi {
            break;
        }
    }
    if from < hi {
        pieces.push((from, hi));
    }
    pieces
        .into_iter()
        .filter(|(s, e)| e - s >= MIN_SILK_SEGMENT_NM)
        .map(|(s, e)| {
            let mut p = a;
            let mut q = a;
            p[axis] = s;
            q[axis] = e;
            Primitive::Line { a: p, b: q }
        })
        .collect()
}

/// The body outline [`SILK_OUTSET_NM`] outside `body`, as lines broken
/// around the pads' mask openings plus clearance.
pub fn silk_outline(pads: &[Pad], body: Box2, default_mask_nm: i64) -> Vec<Graphic> {
    let boxes: Vec<Box2> = pads.iter().map(|p| keep_out(p, default_mask_nm)).collect();
    let [c0, c1, c2, c3] = body.inflated(SILK_OUTSET_NM).corners();
    [(c0, c1), (c1, c2), (c2, c3), (c3, c0)]
        .into_iter()
        .flat_map(|(a, b)| clipped(a, b, &boxes))
        .map(|shape| Graphic {
            shape,
            stroke_nm: SILK_STROKE_NM,
            fill: Fill::None,
        })
        .collect()
}

/// The silkscreen designator, centred above `courtyard_top`.
pub fn silk_designator(body: Box2, courtyard_top: i64) -> Graphic {
    text(
        [
            body.center()[0],
            courtyard_top + TEXT_HEIGHT_NM / 2 + SILK_STROKE_NM,
        ],
        TEXT_HEIGHT_NM,
    )
}

/// The assembly outline of `body` and the designator inside it.
pub fn assembly(body: Box2) -> Vec<Graphic> {
    let [w, h] = body.size();
    let height = (w.min(h) / 2).clamp(MIN_ASSEMBLY_TEXT_NM, TEXT_HEIGHT_NM);
    vec![
        Graphic {
            shape: Primitive::Rect {
                min: body.min,
                max: body.max,
            },
            stroke_nm: ASSEMBLY_STROKE_NM,
            fill: Fill::None,
        },
        text(body.center(), height),
    ]
}

fn text(at: Point, height_nm: i64) -> Graphic {
    Graphic {
        shape: Primitive::Text {
            at,
            height_nm,
            content: TextContent::Designator,
            h_align: HAlign::Center,
            v_align: VAlign::Middle,
        },
        stroke_nm: 0,
        fill: Fill::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_generator_abi::v1::Side;

    fn rect(at: Point, size: Point) -> Pad {
        Pad {
            key: "1".into(),
            terminal: "1".into(),
            shape: PadShape::Rect { size },
            at,
            rot_mdeg: 0,
            side: Side::Top,
            hole: None,
            mask: Opening::Default,
            paste: Opening::Default,
        }
    }

    #[test]
    fn courtyard_encloses_pads_and_body_rounded_outward() {
        // Pads reach x ±1.1 mm, y ±0.5 mm; the body ±0.8 × ±0.4 mm.
        let pads = [
            rect([-650_000, 0], [900_000, 1_000_000]),
            rect([650_000, 0], [900_000, 1_000_000]),
        ];
        let body = Box2::centred([0, 0], [1_600_000, 800_000]);
        assert_eq!(
            courtyard(&pads, Some(body)),
            Some(vec![
                [-1_350_000, -750_000],
                [1_350_000, -750_000],
                [1_350_000, 750_000],
                [-1_350_000, 750_000]
            ])
        );
        // 1.1 + 0.25 = 1.35 is on the grid; 1.12 + 0.25 rounds out to 1.40, and
        // the 0.1 mm tall pad gives 0.05 + 0.25 = 0.30 in y.
        let wide = [rect([0, 0], [2_240_000, 100_000])];
        assert_eq!(
            courtyard(&wide, None).and_then(|c| c.first().copied()),
            Some([-1_400_000, -300_000])
        );
        assert_eq!(courtyard(&[], None), None);
    }

    #[test]
    fn silkscreen_breaks_around_mask_openings() {
        // A 4 × 2 mm body with one pad on its top edge: the top line is cut
        // round the pad's keep-out, the other three edges stay whole.
        let body = Box2::centred([0, 0], [4_000_000, 2_000_000]);
        let pads = [rect([0, 1_150_000], [1_000_000, 500_000])];
        let lines = silk_outline(&pads, body, 50_000);
        assert_eq!(lines.len(), 5);
        // Keep-out half width: 0.5 + 0.05 + 0.1 + 0.075 = 0.725 mm.
        assert!(lines.iter().any(|g| g.shape
            == Primitive::Line {
                a: [-2_150_000, 1_150_000],
                b: [-725_000, 1_150_000]
            }));
    }
}
