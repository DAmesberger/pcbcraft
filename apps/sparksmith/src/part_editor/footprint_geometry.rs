//! What the Footprint tab picks, and the pad kind and shape conversions
//! the inspector offers. Outlines, holes and the display-only mask and
//! paste openings come from the core's pad geometry
//! (`Pad::land`, `Pad::mask_opening`, `Pad::paste_openings`); nothing here
//! is saved and the stored pad stays the authority.
use sparksmith_core::PackageTerminalId;
use sparksmith_core::Point;
use sparksmith_core::library::{
    CornerMask, Hole, HoleShape, MaskRule, Pad, PadKind, PadShape, PadSide, PasteRule, TerminalRule,
};
use sparksmith_core::units::{Angle, Nm, PointNm};

/// The topmost pad whose land is within `tolerance_mm` of `p`.
pub fn pick(pads: &[Pad], p: Point, tolerance_mm: f64, chord_mm: f64) -> Option<usize> {
    pads.iter().rposition(|pad| {
        pad.bounds().inflated(tolerance_mm).contains(p)
            && pad.land(chord_mm).distance(p, true) <= tolerance_mm
    })
}

/// The pad shape families the inspector offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeChoice {
    Rect,
    RoundRect,
    Circle,
    Oblong,
    Custom,
}

impl ShapeChoice {
    pub fn of(shape: &PadShape) -> Self {
        match shape {
            PadShape::Rect { .. } => Self::Rect,
            PadShape::RoundRect { .. } => Self::RoundRect,
            PadShape::Circle { .. } => Self::Circle,
            PadShape::Oblong { .. } => Self::Oblong,
            PadShape::Custom { .. } => Self::Custom,
        }
    }
    /// The editable families; an exact outline comes from a sketch.
    pub const EDITABLE: [(Self, &'static str); 4] = [
        (Self::Rect, "Rectangle"),
        (Self::RoundRect, "Rounded rectangle"),
        (Self::Circle, "Circle"),
        (Self::Oblong, "Oblong"),
    ];
}

/// The shape of family `to` with the same extent (a circle takes the short
/// side). `None` for an exact outline, which only a sketch makes.
pub fn convert_shape(shape: &PadShape, to: ShapeChoice) -> Option<PadShape> {
    let size = PointNm::from_mm_f64_rounded(shape.extent()).ok()?;
    Some(match to {
        ShapeChoice::Rect => PadShape::Rect { size },
        ShapeChoice::RoundRect => match shape {
            PadShape::RoundRect { .. } => shape.clone(),
            _ => PadShape::RoundRect {
                size,
                radius_ratio: 0.25,
                max_radius_nm: None,
                corners: CornerMask::ALL,
            },
        },
        ShapeChoice::Circle => PadShape::Circle {
            diameter_nm: size.x_nm.min(size.y_nm),
        },
        ShapeChoice::Oblong => PadShape::Oblong { size },
        ShapeChoice::Custom => return None,
    })
}

/// Set a pad's shape and keep its rotation canonical for the new symmetry
/// (a fold whose turned paste windows would leave the coordinate range
/// leaves the rotation as it is).
pub fn set_shape(pad: &mut Pad, shape: PadShape) {
    pad.shape = shape;
    let _unfolded = pad.fold_rotation();
}

/// A default drill for a land: half its short side.
fn default_drill(pad: &Pad) -> f64 {
    let [w, h] = pad.shape.extent();
    let d = 0.5 * w.min(h);
    // Round to 0.05 mm so the default reads as a drill size.
    ((d / 0.05).round() * 0.05).max(0.1)
}

/// Change what a pad is for, keeping the pad consistent with its kind:
/// copper kinds that land a terminal get `fallback` when they have none,
/// terminal-forbidding kinds drop theirs, through-hole kinds get a hole of
/// the right plating and both copper layers, surface kinds lose their hole.
/// A terminal is never inferred from the pad number.
pub fn set_kind(pad: &mut Pad, kind: PadKind, fallback: Option<PackageTerminalId>) {
    pad.kind = kind;
    match kind {
        PadKind::Smd => {
            pad.hole = None;
            pad.terminal = pad.terminal.or(fallback);
        }
        PadKind::PlatedThrough | PadKind::NonPlatedHole => {
            // A drill rounded to 0.05 mm is a whole number of nanometres.
            let diameter_nm = Nm::from_mm_f64_rounded(default_drill(pad)).unwrap_or(Nm::ZERO);
            let hole = pad.hole.get_or_insert(Hole {
                shape: HoleShape::Round { diameter_nm },
                plated: true,
                offset: PointNm::ORIGIN,
            });
            hole.plated = kind == PadKind::PlatedThrough;
            pad.side = PadSide::Top;
            if kind == PadKind::PlatedThrough {
                pad.terminal = pad.terminal.or(fallback);
            } else {
                pad.terminal = None;
                pad.paste = PasteRule::None;
            }
        }
        PadKind::Fiducial => {
            pad.terminal = None;
            pad.hole = None;
            pad.paste = PasteRule::None;
        }
        PadKind::Mechanical => {}
    }
}

pub const KINDS: [(PadKind, &str); 5] = [
    (PadKind::Smd, "SMD"),
    (PadKind::PlatedThrough, "Plated through-hole"),
    (PadKind::NonPlatedHole, "Non-plated hole"),
    (PadKind::Fiducial, "Fiducial"),
    (PadKind::Mechanical, "Mechanical"),
];

pub fn kind_label(kind: PadKind) -> &'static str {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("Pad", |(_, l)| l)
}

/// The pads the ribbon adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewPad {
    /// A 1 × 1.4 mm rectangular surface pad.
    Smd,
    /// A 1.7 mm round land with a 1 mm plated drill.
    PlatedThrough,
    /// A 1 mm non-plated hole with a 1.2 mm keep-out land.
    Hole,
    /// A 1 mm fiducial with a 0.5 mm mask clearance.
    Fiducial,
}

impl NewPad {
    pub fn kind(self) -> PadKind {
        match self {
            Self::Smd => PadKind::Smd,
            Self::PlatedThrough => PadKind::PlatedThrough,
            Self::Hole => PadKind::NonPlatedHole,
            Self::Fiducial => PadKind::Fiducial,
        }
    }
    pub fn lands_terminal(self) -> bool {
        self.kind().terminal_rule() == TerminalRule::Required
    }
    /// The number prefix of a pad that lands no terminal.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Fiducial => "FID",
            _ => "MH",
        }
    }
}

/// A new pad of `which` at `position` (mm; [`free_spot`]'s spot, always
/// within the stored range, else the origin). `terminal` is used only by
/// kinds that land one.
pub fn new_pad(
    which: NewPad,
    id: sparksmith_core::PadId,
    terminal: Option<PackageTerminalId>,
    number: String,
    position: Point,
) -> Pad {
    let nm = |mm: i64| Nm::new(mm).unwrap_or(Nm::ZERO);
    let mut pad = Pad {
        id,
        kind: which.kind(),
        terminal: terminal.filter(|_| which.lands_terminal()),
        number,
        position: PointNm::from_mm_f64_rounded(position).unwrap_or(PointNm::ORIGIN),
        rotation_mdeg: Angle::ZERO,
        side: PadSide::Top,
        shape: PadShape::Rect {
            size: PointNm::new(nm(1_000_000), nm(1_400_000)),
        },
        hole: None,
        mask: MaskRule::Inherit,
        paste: PasteRule::Inherit,
        vias: Vec::new(),
    };
    let round = |diameter_nm: Nm, plated: bool| Hole {
        shape: HoleShape::Round { diameter_nm },
        plated,
        offset: PointNm::ORIGIN,
    };
    match which {
        NewPad::Smd => {}
        NewPad::PlatedThrough => {
            pad.shape = PadShape::Circle {
                diameter_nm: nm(1_700_000),
            };
            pad.hole = Some(round(nm(1_000_000), true));
        }
        NewPad::Hole => {
            pad.shape = PadShape::Circle {
                diameter_nm: nm(1_200_000),
            };
            pad.hole = Some(round(nm(1_000_000), false));
            pad.paste = PasteRule::None;
        }
        NewPad::Fiducial => {
            pad.shape = PadShape::Circle {
                diameter_nm: nm(1_000_000),
            };
            pad.mask = MaskRule::Expansion(nm(500_000));
            pad.paste = PasteRule::None;
        }
    }
    pad
}

/// Where a new pad goes: 2 mm right of every existing land, on the x axis
/// (the origin when there are none), on a 0.05 mm grid.
pub fn free_spot(pads: &[Pad]) -> Point {
    let right = pads
        .iter()
        .map(|p| p.bounds().max[0])
        .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))));
    match right {
        Some(x) if x.is_finite() => [((x + 2.0) / 0.05).round() * 0.05, 0.0],
        _ => [0.0, 0.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schematic_editor::{mdeg, nm, pt};
    use sparksmith_core::PadId;
    use sparksmith_core::geometry::{Bounds, Shape};
    use sparksmith_core::library::{Outline, PadDefaults, Segment};

    const CHORD: f64 = 0.001;

    fn pad(shape: PadShape) -> Pad {
        let mut pad = Pad::smd_rect(
            PadId(1),
            PackageTerminalId(1),
            "1",
            pt([2.0, 1.0]),
            pt([1.0, 1.0]),
        );
        pad.shape = shape;
        pad
    }

    fn bounds(shape: &Shape) -> Option<Bounds> {
        Bounds::of(shape.flatten(CHORD))
    }

    fn corner_radius(size: Point, radius_ratio: f64, max_radius: Option<f64>) -> f64 {
        PadShape::RoundRect {
            size: pt(size),
            radius_ratio,
            max_radius_nm: max_radius.map(nm),
            corners: CornerMask::ALL,
        }
        .corner_radius()
    }

    fn close(a: Point, b: Point) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn a_rotated_rect_land_spans_its_turned_extent_and_picks_inside_only() {
        let mut p = pad(PadShape::Rect {
            size: pt([2.0, 1.0]),
        });
        p.rotation_mdeg = mdeg(90.0);
        let b = bounds(&p.land(CHORD));
        assert_eq!(b, Some(p.bounds()));
        assert!(b.is_some_and(|b| close(b.size(), [1.0, 2.0])));
        let pads = [p];
        assert_eq!(pick(&pads, [2.0, 1.9], 0.0, CHORD), Some(0));
        assert_eq!(
            pick(&pads, [2.9, 1.0], 0.0, CHORD),
            None,
            "outside the turned land"
        );
        assert_eq!(
            pick(&pads, [2.6, 1.0], 0.15, CHORD),
            Some(0),
            "within tolerance"
        );
    }

    #[test]
    fn rounded_corners_follow_ratio_cap_and_mask_and_oblongs_are_stadiums() {
        assert_eq!(corner_radius([2.0, 1.0], 0.25, None), 0.25);
        assert_eq!(
            corner_radius([2.0, 1.0], 0.9, None),
            0.5,
            "ratio clamps at 0.5"
        );
        assert_eq!(corner_radius([2.0, 1.0], 0.25, Some(0.1)), 0.1);
        // Only the top-left corner rounded: the bottom-right corner is sharp.
        let d = pad(PadShape::RoundRect {
            size: pt([2.0, 2.0]),
            radius_ratio: 0.5,
            max_radius_nm: None,
            corners: CornerMask {
                top_left: true,
                top_right: false,
                bottom_right: false,
                bottom_left: false,
            },
        });
        let shape = Some(d.land(CHORD));
        let at = |q: Point| shape.as_ref().map(|s| s.distance(q, true));
        assert_eq!(
            at([3.0, 0.0]),
            Some(0.0),
            "bottom-right corner is on the land"
        );
        assert!(
            at([1.0, 2.0]).is_some_and(|d| d > 0.25),
            "top-left is rounded away"
        );
        // An oblong's ends are half circles: its tip is on the land, its
        // corner is not.
        let o = pad(PadShape::Oblong {
            size: pt([3.0, 1.0]),
        });
        let shape = Some(o.land(CHORD));
        let at = |q: Point| shape.as_ref().map(|s| s.distance(q, true));
        assert!(at([3.5, 1.0]).is_some_and(|d| d < 1e-3));
        assert!(at([3.5, 1.5]).is_some_and(|d| d > 0.1));
    }

    #[test]
    fn mask_grows_and_paste_shrinks_by_their_rules() {
        let defaults = PadDefaults {
            mask_expansion_nm: nm(0.1),
            paste_reduction_nm: nm(0.05),
        };
        let mut p = pad(PadShape::Rect {
            size: pt([1.0, 1.0]),
        });
        let size = |s: Option<Shape>| s.as_ref().and_then(bounds).map(|b| b.size());
        assert!(size(p.mask_opening(&defaults, CHORD)).is_some_and(|s| close(s, [1.2, 1.2])));
        let paste = p.paste_openings(&defaults, CHORD);
        assert_eq!(paste.len(), 1);
        assert!(size(paste.into_iter().next()).is_some_and(|s| close(s, [0.9, 0.9])));
        p.mask = MaskRule::Tented;
        assert!(p.mask_opening(&defaults, CHORD).is_none());
        p.mask = MaskRule::Expansion(nm(0.3));
        assert!(size(p.mask_opening(&defaults, CHORD)).is_some_and(|s| close(s, [1.6, 1.6])));
        p.paste = PasteRule::None;
        assert!(p.paste_openings(&defaults, CHORD).is_empty());
        // Inherited paste is none on a through-hole pad.
        p.paste = PasteRule::Inherit;
        set_kind(&mut p, PadKind::PlatedThrough, None);
        assert!(p.paste_openings(&defaults, CHORD).is_empty());
    }

    #[test]
    fn a_slot_hole_turns_with_its_angle_and_the_pad() {
        let mut p = pad(PadShape::Oblong {
            size: pt([3.0, 1.6]),
        });
        p.kind = PadKind::PlatedThrough;
        p.hole = Some(Hole {
            shape: HoleShape::Slot {
                width_nm: nm(0.8),
                length_nm: nm(2.0),
                angle_mdeg: mdeg(0.0),
            },
            plated: true,
            offset: pt([0.0, 0.0]),
        });
        let size = |p: &Pad| p.hole_shape().as_ref().and_then(bounds).map(|b| b.size());
        assert!(size(&p).is_some_and(|s| close(s, [2.0, 0.8])));
        p.rotation_mdeg = mdeg(90.0);
        assert!(size(&p).is_some_and(|s| close(s, [0.8, 2.0])));
        let centre = p.hole_shape().as_ref().and_then(bounds).map(|b| b.center());
        assert!(centre.is_some_and(|c| close(c, [2.0, 1.0])));
    }

    #[test]
    fn an_exact_outline_flattens_its_bulge_arcs() {
        // A 2 × 1 box whose right edge is a half circle bulging outward.
        let outline = Outline {
            start: [-1.0, -0.5],
            segments: vec![
                Segment::Line { to: [1.0, -0.5] },
                Segment::Arc {
                    to: [1.0, 0.5],
                    bulge: 1.0,
                },
                Segment::Line { to: [-1.0, 0.5] },
                Segment::Line { to: [-1.0, -0.5] },
            ],
        };
        let points = outline.flatten(CHORD);
        let b = Bounds::of(points.iter().copied());
        assert!(b.is_some_and(|b| close(b.max, [1.5, 0.5])), "{b:?}");
        assert!(points.iter().all(|p| p[0] <= 1.5 + 1e-9));
        // A clockwise (negative) bulge cuts in instead.
        let mut inward = outline.clone();
        inward.segments[1] = Segment::Arc {
            to: [1.0, 0.5],
            bulge: -1.0,
        };
        let b = Bounds::of(inward.flatten(CHORD));
        assert!(b.is_some_and(|b| close(b.max, [1.0, 0.5])));
    }

    #[test]
    fn kinds_keep_terminals_holes_and_paste_consistent() {
        let mut p = pad(PadShape::Circle {
            diameter_nm: nm(1.6),
        });
        set_kind(&mut p, PadKind::PlatedThrough, None);
        assert_eq!(p.terminal, Some(PackageTerminalId(1)), "kept");
        assert!(p.hole.is_some_and(|h| h.plated));
        assert_eq!(
            p.hole.map(|h| h.shape),
            Some(HoleShape::Round {
                diameter_nm: nm(0.8)
            })
        );
        set_kind(&mut p, PadKind::NonPlatedHole, None);
        assert_eq!(p.terminal, None, "a bare hole lands no terminal");
        assert!(p.hole.is_some_and(|h| !h.plated));
        // Back to copper: the terminal comes from the explicit fallback,
        // never from the number.
        set_kind(&mut p, PadKind::Smd, None);
        assert_eq!(p.terminal, None);
        assert!(p.hole.is_none());
        set_kind(&mut p, PadKind::Smd, Some(PackageTerminalId(4)));
        assert_eq!(p.terminal, Some(PackageTerminalId(4)));
        set_kind(&mut p, PadKind::Fiducial, None);
        assert_eq!((p.terminal, p.paste.clone()), (None, PasteRule::None));
    }

    #[test]
    fn new_pads_are_consistent_with_their_kinds_and_land_clear_of_others() {
        let terminal = Some(PackageTerminalId(2));
        let id = PadId(9);
        let smd = new_pad(NewPad::Smd, id, terminal, "2".into(), [0.0, 0.0]);
        assert_eq!(
            (smd.kind, smd.terminal, smd.hole),
            (PadKind::Smd, terminal, None)
        );
        let pth = new_pad(NewPad::PlatedThrough, id, terminal, "2".into(), [0.0, 0.0]);
        assert_eq!(pth.terminal, terminal);
        assert!(pth.hole.is_some_and(|h| h.plated));
        let hole = new_pad(NewPad::Hole, id, terminal, "MH1".into(), [0.0, 0.0]);
        assert_eq!(hole.terminal, None, "a bare hole never lands a terminal");
        assert!(hole.hole.is_some_and(|h| !h.plated));
        let fid = new_pad(NewPad::Fiducial, id, terminal, "FID1".into(), [0.0, 0.0]);
        assert_eq!((fid.terminal, fid.paste), (None, PasteRule::None));
        assert_eq!(free_spot(&[]), [0.0, 0.0]);
        let at = free_spot(&[smd, pth]);
        assert!((at[0] - 2.85).abs() < 1e-9, "{at:?}");
    }

    #[test]
    fn converting_a_shape_keeps_its_extent_and_rotation_canonical() {
        let mut p = pad(PadShape::Rect {
            size: pt([2.0, 1.0]),
        });
        p.rotation_mdeg = mdeg(270.0);
        let shape = p.shape.clone();
        set_shape(&mut p, shape);
        assert_eq!(
            p.rotation_deg(),
            90.0,
            "a rectangle repeats every half turn"
        );
        for (choice, _) in ShapeChoice::EDITABLE {
            let shape = convert_shape(&p.shape, choice);
            assert!(shape.as_ref().is_some_and(|s| ShapeChoice::of(s) == choice));
        }
        assert_eq!(
            convert_shape(&p.shape, ShapeChoice::Circle),
            Some(PadShape::Circle {
                diameter_nm: nm(1.0)
            })
        );
        assert_eq!(convert_shape(&p.shape, ShapeChoice::Custom), None);
        if let Some(circle) = convert_shape(&p.shape, ShapeChoice::Circle) {
            set_shape(&mut p, circle);
        }
        assert_eq!(p.rotation_deg(), 0.0);
    }
}
