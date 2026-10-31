//! What the Footprint tab draws and picks, in the footprint frame, read
//! from the typed pad data: each [`PadShape`] and [`Hole`] as a core
//! [`Shape`] placed by the pad's position and rotation, the display-only
//! mask and paste openings its rules resolve to, and pad kind and shape
//! conversions the inspector offers. Nothing here is saved; the stored pad
//! stays the authority.
use pcbcraft_core::PackageTerminalId;
use pcbcraft_core::Point;
use pcbcraft_core::geometry::{Shape, Transform2};
use pcbcraft_core::library::{
    CornerMask, Hole, HoleShape, MaskRule, Outline, Pad, PadDefaults, PadKind, PadShape, PadSide,
    PasteRule, Segment,
};

/// Pad frame to footprint frame: rotate about the pad origin, then move it.
pub fn pad_frame(pad: &Pad) -> Transform2 {
    Transform2 {
        translate: pad.position,
        rotation_deg: pad.rotation_deg,
        mirror: false,
    }
}

/// The corner radius of a rounded rectangle: `ratio` (clamped to 0–0.5) of
/// the short side, capped at `max` mm.
pub fn corner_radius(size: Point, ratio: f64, max: Option<f64>) -> f64 {
    let r = ratio.clamp(0.0, 0.5) * size[0].min(size[1]).max(0.0);
    max.map_or(r, |m| r.min(m.max(0.0)))
}

/// A `size` box centred on the origin, grown by `by` on every side, with
/// `radius` on the corners `corners` selects. Growing rounds every corner
/// by at least `by`, as an exact offset does. `None` when it vanishes.
fn rounded_box(size: Point, radius: f64, corners: CornerMask, by: f64) -> Option<Shape> {
    let (w, h) = (size[0] + 2.0 * by, size[1] + 2.0 * by);
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let cap = 0.5 * w.min(h);
    let rounded = (radius + by).clamp(0.0, cap);
    let sharp = by.clamp(0.0, cap);
    let (a, b) = ([-w / 2.0, -h / 2.0], [w / 2.0, h / 2.0]);
    if corners == CornerMask::ALL || rounded == sharp {
        return Some(Shape::Rect {
            a,
            b,
            radius: rounded,
        });
    }
    let pick = |on: bool| if on { rounded } else { sharp };
    // Counter-clockwise from the bottom-left corner, as `Bounds::corners`.
    Some(Shape::Polygon {
        points: vec![a, [b[0], a[1]], b, [a[0], b[1]]],
        radii: vec![
            pick(corners.bottom_left),
            pick(corners.bottom_right),
            pick(corners.top_right),
            pick(corners.top_left),
        ],
    })
}

/// An exact outline as one closed polygon, arcs flattened within
/// `chord_mm`. At most `MAX_OUTLINE_SEGMENTS` arcs of bounded vertices.
pub fn outline_points(outline: &Outline, chord_mm: f64) -> Vec<Point> {
    let mut points = vec![outline.start];
    let mut from = outline.start;
    for segment in &outline.segments {
        let to = segment.to();
        if let Segment::Arc { bulge, .. } = segment
            && bulge.abs() > 1e-12
            && let Some(arc) = bulge_arc(from, to, *bulge)
        {
            points.extend(arc.flatten(chord_mm).into_iter().skip(1));
        } else {
            points.push(to);
        }
        from = to;
    }
    // The last segment closes on the start.
    if points.len() > 1
        && points
            .first()
            .zip(points.last())
            .is_some_and(|(a, b)| pcbcraft_core::geometry::distance(*a, *b) <= 1e-9)
    {
        points.pop();
    }
    points
}

/// The circular arc from `a` to `b` with `bulge = tan(sweep / 4)`.
fn bulge_arc(a: Point, b: Point, bulge: f64) -> Option<Shape> {
    let chord = [b[0] - a[0], b[1] - a[1]];
    let length = chord[0].hypot(chord[1]);
    if length.is_nan() || length <= 1e-12 || !bulge.is_finite() {
        return None;
    }
    // The centre sits on the chord's perpendicular, left of a → b for a
    // counter-clockwise (positive) bulge below a half turn.
    let k = (1.0 - bulge * bulge) / (4.0 * bulge);
    let center = [
        0.5 * (a[0] + b[0]) - chord[1] * k,
        0.5 * (a[1] + b[1]) + chord[0] * k,
    ];
    let radius = (a[0] - center[0]).hypot(a[1] - center[1]);
    let sweep_deg = 4.0 * bulge.atan().to_degrees();
    Some(Shape::Arc {
        center,
        radius,
        start_deg: (a[1] - center[1]).atan2(a[0] - center[0]).to_degrees(),
        sweep_deg,
    })
}

/// The pad shape grown by `by` mm (negative shrinks), in the pad frame.
/// Custom outlines are shown unoffset (a display approximation).
pub fn local_shape(shape: &PadShape, by: f64, chord_mm: f64) -> Option<Shape> {
    match shape {
        PadShape::Rect { size } => rounded_box(*size, 0.0, CornerMask::ALL, by),
        PadShape::RoundRect {
            size,
            radius_ratio,
            max_radius,
            corners,
        } => rounded_box(
            *size,
            corner_radius(*size, *radius_ratio, *max_radius),
            *corners,
            by,
        ),
        PadShape::Circle { diameter } => {
            let radius = diameter / 2.0 + by;
            (radius > 0.0).then_some(Shape::Circle {
                center: [0.0, 0.0],
                radius,
            })
        }
        PadShape::Oblong { size } => {
            rounded_box(*size, 0.5 * size[0].min(size[1]), CornerMask::ALL, by)
        }
        PadShape::Custom { outline } => {
            let points = outline_points(outline, chord_mm);
            (points.len() >= 3).then(|| Shape::polygon(points))
        }
    }
}

/// The pad's land in the footprint frame.
pub fn pad_shape(pad: &Pad, chord_mm: f64) -> Option<Shape> {
    local_shape(&pad.shape, 0.0, chord_mm).map(|s| s.transformed(&pad_frame(pad)))
}

/// A hole's outline in the pad frame: a round drill, or a slot of `length`
/// overall with round ends of `width` along `angle_deg`.
pub fn hole_local(hole: &Hole) -> Option<Shape> {
    let shape = match hole.shape {
        HoleShape::Round { diameter } => (diameter > 0.0).then_some(Shape::Circle {
            center: [0.0, 0.0],
            radius: diameter / 2.0,
        })?,
        HoleShape::Slot {
            width,
            length,
            angle_deg,
        } => rounded_box(
            [length.max(width), width],
            width / 2.0,
            CornerMask::ALL,
            0.0,
        )?
        .transformed(&Transform2 {
            translate: [0.0, 0.0],
            rotation_deg: angle_deg,
            mirror: false,
        }),
    };
    Some(shape.translated(hole.offset))
}

/// The pad's hole in the footprint frame.
pub fn hole_shape(pad: &Pad) -> Option<Shape> {
    hole_local(pad.hole.as_ref()?).map(|s| s.transformed(&pad_frame(pad)))
}

/// The display-only solder-mask opening, footprint frame; `None` when
/// tented.
pub fn mask_shape(pad: &Pad, defaults: &PadDefaults, chord_mm: f64) -> Option<Shape> {
    let by = match pad.mask {
        MaskRule::Inherit => defaults.mask_expansion_mm,
        MaskRule::Tented => return None,
        MaskRule::Expansion(e) => e,
    };
    local_shape(&pad.shape, by, chord_mm).map(|s| s.transformed(&pad_frame(pad)))
}

/// The display-only paste openings, footprint frame. Inherited paste is
/// none for through-hole kinds and fiducials.
pub fn paste_shapes(pad: &Pad, defaults: &PadDefaults, chord_mm: f64) -> Vec<Shape> {
    let frame = pad_frame(pad);
    let shrink = |by: f64| {
        local_shape(&pad.shape, -by, chord_mm)
            .map(|s| s.transformed(&frame))
            .into_iter()
            .collect()
    };
    match &pad.paste {
        PasteRule::Inherit => match pad.kind {
            PadKind::Smd | PadKind::Mechanical if pad.hole.is_none() => {
                shrink(defaults.paste_reduction_mm)
            }
            _ => Vec::new(),
        },
        PasteRule::None => Vec::new(),
        PasteRule::Reduction(r) => shrink(*r),
        PasteRule::Windows(windows) => windows
            .iter()
            .map(|w| w.shape.transformed(&frame))
            .collect(),
    }
}

/// The topmost pad whose land is within `tolerance_mm` of `p`.
pub fn pick(pads: &[Pad], p: Point, tolerance_mm: f64, chord_mm: f64) -> Option<usize> {
    pads.iter().rposition(|pad| {
        pad.bounds().inflated(tolerance_mm).contains(p)
            && pad_shape(pad, chord_mm).is_some_and(|s| s.distance(p, true) <= tolerance_mm)
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
    let size = shape.extent();
    Some(match to {
        ShapeChoice::Rect => PadShape::Rect { size },
        ShapeChoice::RoundRect => match shape {
            PadShape::RoundRect { .. } => shape.clone(),
            _ => PadShape::RoundRect {
                size,
                radius_ratio: 0.25,
                max_radius: None,
                corners: CornerMask::ALL,
            },
        },
        ShapeChoice::Circle => PadShape::Circle {
            diameter: size[0].min(size[1]),
        },
        ShapeChoice::Oblong => PadShape::Oblong { size },
        ShapeChoice::Custom => return None,
    })
}

/// Set a pad's shape and keep its rotation canonical for the new symmetry.
pub fn set_shape(pad: &mut Pad, shape: PadShape) {
    pad.rotation_deg = shape.canonical_rotation(pad.rotation_deg);
    pad.shape = shape;
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
            let diameter = default_drill(pad);
            let hole = pad.hole.get_or_insert(Hole {
                shape: HoleShape::Round { diameter },
                plated: true,
                offset: [0.0, 0.0],
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

/// Whether a kind lands a terminal: required, forbidden or optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalRule {
    Required,
    Forbidden,
    Optional,
}

pub fn terminal_rule(kind: PadKind) -> TerminalRule {
    match kind {
        PadKind::Smd | PadKind::PlatedThrough => TerminalRule::Required,
        PadKind::NonPlatedHole | PadKind::Fiducial => TerminalRule::Forbidden,
        PadKind::Mechanical => TerminalRule::Optional,
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
        terminal_rule(self.kind()) == TerminalRule::Required
    }
    /// The number prefix of a pad that lands no terminal.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Fiducial => "FID",
            _ => "MH",
        }
    }
}

/// A new pad of `which` at `position`. `terminal` is used only by kinds that
/// land one.
pub fn new_pad(
    which: NewPad,
    id: pcbcraft_core::PadId,
    terminal: Option<PackageTerminalId>,
    number: String,
    position: Point,
) -> Pad {
    let mut pad = Pad {
        id,
        kind: which.kind(),
        terminal: terminal.filter(|_| which.lands_terminal()),
        number,
        position,
        rotation_deg: 0.0,
        side: PadSide::Top,
        shape: PadShape::Rect { size: [1.0, 1.4] },
        hole: None,
        mask: MaskRule::Inherit,
        paste: PasteRule::Inherit,
        vias: Vec::new(),
    };
    let round = |diameter: f64, plated: bool| Hole {
        shape: HoleShape::Round { diameter },
        plated,
        offset: [0.0, 0.0],
    };
    match which {
        NewPad::Smd => {}
        NewPad::PlatedThrough => {
            pad.shape = PadShape::Circle { diameter: 1.7 };
            pad.hole = Some(round(1.0, true));
        }
        NewPad::Hole => {
            pad.shape = PadShape::Circle { diameter: 1.2 };
            pad.hole = Some(round(1.0, false));
            pad.paste = PasteRule::None;
        }
        NewPad::Fiducial => {
            pad.shape = PadShape::Circle { diameter: 1.0 };
            pad.mask = MaskRule::Expansion(0.5);
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
    use pcbcraft_core::PadId;
    use pcbcraft_core::geometry::Bounds;

    const CHORD: f64 = 0.001;

    fn pad(shape: PadShape) -> Pad {
        let mut pad = Pad::smd_rect(PadId(1), PackageTerminalId(1), "1", [2.0, 1.0], [1.0, 1.0]);
        pad.shape = shape;
        pad
    }

    fn bounds(shape: &Shape) -> Option<Bounds> {
        Bounds::of(shape.flatten(CHORD))
    }

    fn close(a: Point, b: Point) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn a_rotated_rect_land_spans_its_turned_extent_and_picks_inside_only() {
        let mut p = pad(PadShape::Rect { size: [2.0, 1.0] });
        p.rotation_deg = 90.0;
        let b = pad_shape(&p, CHORD).as_ref().and_then(bounds);
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
            size: [2.0, 2.0],
            radius_ratio: 0.5,
            max_radius: None,
            corners: CornerMask {
                top_left: true,
                top_right: false,
                bottom_right: false,
                bottom_left: false,
            },
        });
        let shape = pad_shape(&d, CHORD);
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
        let o = pad(PadShape::Oblong { size: [3.0, 1.0] });
        let shape = pad_shape(&o, CHORD);
        let at = |q: Point| shape.as_ref().map(|s| s.distance(q, true));
        assert!(at([3.5, 1.0]).is_some_and(|d| d < 1e-3));
        assert!(at([3.5, 1.5]).is_some_and(|d| d > 0.1));
    }

    #[test]
    fn mask_grows_and_paste_shrinks_by_their_rules() {
        let defaults = PadDefaults {
            mask_expansion_mm: 0.1,
            paste_reduction_mm: 0.05,
        };
        let mut p = pad(PadShape::Rect { size: [1.0, 1.0] });
        let size = |s: Option<Shape>| s.as_ref().and_then(bounds).map(|b| b.size());
        assert!(size(mask_shape(&p, &defaults, CHORD)).is_some_and(|s| close(s, [1.2, 1.2])));
        let paste = paste_shapes(&p, &defaults, CHORD);
        assert_eq!(paste.len(), 1);
        assert!(size(paste.into_iter().next()).is_some_and(|s| close(s, [0.9, 0.9])));
        p.mask = MaskRule::Tented;
        assert!(mask_shape(&p, &defaults, CHORD).is_none());
        p.mask = MaskRule::Expansion(0.3);
        assert!(size(mask_shape(&p, &defaults, CHORD)).is_some_and(|s| close(s, [1.6, 1.6])));
        p.paste = PasteRule::None;
        assert!(paste_shapes(&p, &defaults, CHORD).is_empty());
        // Inherited paste is none on a through-hole pad.
        p.paste = PasteRule::Inherit;
        set_kind(&mut p, PadKind::PlatedThrough, None);
        assert!(paste_shapes(&p, &defaults, CHORD).is_empty());
    }

    #[test]
    fn a_slot_hole_turns_with_its_angle_and_the_pad() {
        let mut p = pad(PadShape::Oblong { size: [3.0, 1.6] });
        p.kind = PadKind::PlatedThrough;
        p.hole = Some(Hole {
            shape: HoleShape::Slot {
                width: 0.8,
                length: 2.0,
                angle_deg: 0.0,
            },
            plated: true,
            offset: [0.0, 0.0],
        });
        let size = |p: &Pad| hole_shape(p).as_ref().and_then(bounds).map(|b| b.size());
        assert!(size(&p).is_some_and(|s| close(s, [2.0, 0.8])));
        p.rotation_deg = 90.0;
        assert!(size(&p).is_some_and(|s| close(s, [0.8, 2.0])));
        let centre = hole_shape(&p).as_ref().and_then(bounds).map(|b| b.center());
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
        let points = outline_points(&outline, CHORD);
        let b = Bounds::of(points.iter().copied());
        assert!(b.is_some_and(|b| close(b.max, [1.5, 0.5])), "{b:?}");
        assert!(points.iter().all(|p| p[0] <= 1.5 + 1e-9));
        // A clockwise (negative) bulge cuts in instead.
        let mut inward = outline.clone();
        inward.segments[1] = Segment::Arc {
            to: [1.0, 0.5],
            bulge: -1.0,
        };
        let b = Bounds::of(outline_points(&inward, CHORD));
        assert!(b.is_some_and(|b| close(b.max, [1.0, 0.5])));
    }

    #[test]
    fn kinds_keep_terminals_holes_and_paste_consistent() {
        let mut p = pad(PadShape::Circle { diameter: 1.6 });
        set_kind(&mut p, PadKind::PlatedThrough, None);
        assert_eq!(p.terminal, Some(PackageTerminalId(1)), "kept");
        assert!(p.hole.is_some_and(|h| h.plated));
        assert_eq!(
            p.hole.map(|h| h.shape),
            Some(HoleShape::Round { diameter: 0.8 })
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
        let mut p = pad(PadShape::Rect { size: [2.0, 1.0] });
        p.rotation_deg = 270.0;
        let shape = p.shape.clone();
        set_shape(&mut p, shape);
        assert_eq!(p.rotation_deg, 90.0, "a rectangle repeats every half turn");
        for (choice, _) in ShapeChoice::EDITABLE {
            let shape = convert_shape(&p.shape, choice);
            assert!(shape.as_ref().is_some_and(|s| ShapeChoice::of(s) == choice));
        }
        assert_eq!(
            convert_shape(&p.shape, ShapeChoice::Circle),
            Some(PadShape::Circle { diameter: 1.0 })
        );
        assert_eq!(convert_shape(&p.shape, ShapeChoice::Custom), None);
        if let Some(circle) = convert_shape(&p.shape, ShapeChoice::Circle) {
            set_shape(&mut p, circle);
        }
        assert_eq!(p.rotation_deg, 0.0);
    }
}
