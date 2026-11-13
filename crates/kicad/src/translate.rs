//! KiCad footprints and pads in Sparksmith's frame.
//!
//! **Axes.** KiCad's y grows downward and ours upward: a board point
//! becomes `(x − ox, −(y − oy))` mm about the auxiliary origin `o` (or the
//! file origin when none is set), and a footprint- or pad-local vector
//! `(x, y)` becomes `(x, −y)`. Angles keep their value: both count
//! counter-clockwise as seen on screen.
//!
//! **Frames.** A pad's `at` is in its footprint's frame (before the
//! footprint's turn), and its angle is its orientation on the board, the
//! footprint's included. A back-side footprint's pads and primitives are
//! stored as seen from the top (already mirrored). So a pad's board pose
//! `W` is known without any mirroring, and the definition pad is
//! `P⁻¹ ∘ W`, where `P` is our placement (a reflection across x on the
//! bottom, then the footprint's turn): the reflection that lands in the
//! definition is baked into the pad-local geometry (custom outlines,
//! hole offsets), which is why a definition is computed per placement and
//! then shared when equal. These are hypotheses confirmed by `kicad-cli`
//! plots (`tests/oracle.rs`), not by documentation.
use crate::board::{Board, Footprint, KPad, PadShapeK, PadType, PointNm, Primitive};
use crate::report::{Case, Report};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};
use sparksmith_core::Layer;
use sparksmith_core::geometry::{Bounds, Point, Transform2};
use sparksmith_core::library::{
    ApertureId, CornerMask, Hole, HoleShape, MAX_OUTLINE_SEGMENTS, MaskRule, Outline, Pad, PadKind,
    PadShape, PadSide, PasteRule, PasteWindow, Segment, ShapeNm,
};
use sparksmith_core::units::{Angle, Nm};
use sparksmith_core::{PackageTerminalId, PadId};

/// Largest distance of a custom pad's chord from its round primitive, mm.
pub const CHORD_MM: f64 = 0.000_25;
/// Most segments one round primitive becomes.
const MAX_CIRCLE_SEGMENTS: usize = 256;

fn mm(nm: i64) -> f64 {
    nm as f64 / 1e6
}

/// Millimetres rounded to whole nanometres, so equal definitions computed
/// from different placements compare equal.
fn snap(v: f64) -> f64 {
    let r = (v * 1e6).round() / 1e6;
    if r == 0.0 { 0.0 } else { r }
}

fn snap_point(p: Point) -> Point {
    [snap(p[0]), snap(p[1])]
}

/// Millimetres in whole nanometres; beyond the coordinate bound the pad
/// is refused.
fn nm(v: f64) -> Result<Nm, Case> {
    Nm::from_mm_f64_rounded(v).map_err(|_| Case::PadOutOfRange)
}

/// A point in whole nanometres (see [`nm`]).
fn nm_point(p: Point) -> Result<sparksmith_core::units::PointNm, Case> {
    sparksmith_core::units::PointNm::from_mm_f64_rounded(p).map_err(|_| Case::PadOutOfRange)
}

/// A KiCad length (whole nanometres already) within the coordinate bound.
fn nm_exact(v: i64) -> Result<Nm, Case> {
    Nm::new(v).map_err(|_| Case::PadOutOfRange)
}

/// An angle in [0, 360) to a billionth of a degree.
fn snap_angle(deg: f64) -> f64 {
    let r = ((deg.rem_euclid(360.0)) * 1e9).round() / 1e9;
    if r >= 360.0 { 0.0 } else { r }
}

/// A KiCad board point in our board frame, mm.
pub fn board_point(board: &Board, p: PointNm) -> Point {
    let o = board.aux_origin.unwrap_or_default();
    [mm(p[0] - o[0]), -mm(p[1] - o[1])]
}

/// A KiCad local vector in our orientation, mm.
fn local(p: PointNm) -> Point {
    [mm(p[0]), -mm(p[1])]
}

/// Our placement of a footprint.
pub fn placement(board: &Board, fp: &Footprint) -> (Point, f64, Layer) {
    (
        board_point(board, fp.at),
        snap_angle(fp.angle),
        if fp.back { Layer::Bottom } else { Layer::Top },
    )
}

fn transform(board: &Board, fp: &Footprint) -> Transform2 {
    let (translate, rotation_deg, layer) = placement(board, fp);
    Transform2 {
        translate,
        rotation_deg,
        mirror: layer == Layer::Bottom,
    }
}

/// A pad of a definition, without its id and terminal, and a short class
/// name for difference reports (no names or values from the file).
#[derive(Clone, Debug, PartialEq)]
pub struct DefPad {
    pub pad: Pad,
    pub class: String,
    /// The pad's box on the board, mm.
    pub world: Bounds,
    /// Round primitives of a custom pad on the board (centre, radius, mm),
    /// with the radii of their mask and paste openings: where KiCad's
    /// chords may differ from ours.
    pub arcs: Vec<(Point, f64)>,
    /// Sharp corners of the land on the board, with how far a mask or
    /// paste opening grows or shrinks there (mm).
    pub corners: Vec<(Point, f64)>,
}

/// How a pad maps: into a definition pad, or refused with its case.
pub enum Mapped {
    Pad(Box<DefPad>),
    /// A pad with paste and no copper: a paste window for the pad under it.
    PasteOnly(Box<DefPad>),
    Refused(Case),
}

/// The pad-local geometry map: KiCad's local `(x, y)` to the definition
/// pad's frame. With the placement's reflection baked in (a back-side
/// footprint) it is the identity; otherwise y flips.
#[derive(Clone, Copy)]
struct Local {
    flip: bool,
}

impl Local {
    fn point(self, p: Point) -> Point {
        // `p` is already in our orientation (y up); the definition frame
        // of a mirrored pad reflects it back.
        if self.flip { [p[0], -p[1]] } else { p }
    }
    /// A bulge drawn in our orientation, in the definition frame.
    fn bulge(self, b: f64) -> f64 {
        if self.flip { -b } else { b }
    }
}

/// Translate one pad of a placed footprint.
pub fn pad(board: &Board, fp: &Footprint, kpad: &KPad, report: &mut Report) -> Mapped {
    pad_or_refusal(board, fp, kpad, report).unwrap_or_else(Mapped::Refused)
}

/// [`pad`], with a refusal as the error.
fn pad_or_refusal(
    board: &Board,
    fp: &Footprint,
    kpad: &KPad,
    report: &mut Report,
) -> Result<Mapped, Case> {
    if !kpad.unknown.is_empty() {
        for path in &kpad.unknown {
            report.add_detail(Case::PadUnknownToken, &fp.reference, &kpad.number, path);
        }
        return Err(Case::PadUnknownToken);
    }
    let place = transform(board, fp);
    let (c, s) = sparksmith_core::geometry::cos_sin(place.rotation_deg);
    // The pad's `at` is its hole; a drill `offset` moves the copper shape
    // from the hole, in the pad's frame (oracle). Our pad origin is the
    // shape's centre, with the hole offset back from it.
    let p = local(kpad.at);
    let anchor = [
        place.translate[0] + c * p[0] - s * p[1],
        place.translate[1] + s * p[0] + c * p[1],
    ];
    let shift = local(kpad.drill.map(|d| d.offset).unwrap_or_default());
    let (pc, ps) = sparksmith_core::geometry::cos_sin(snap_angle(kpad.angle));
    let center = [
        anchor[0] + pc * shift[0] - ps * shift[1],
        anchor[1] + ps * shift[0] + pc * shift[1],
    ];
    let world = Transform2 {
        translate: center,
        rotation_deg: snap_angle(kpad.angle),
        mirror: false,
    };
    let def = world.then(&place.inverse());
    let map = Local { flip: def.mirror };
    let front = kpad.on("F.Cu");
    let back = kpad.on("B.Cu");
    let side = if fp.back { "B" } else { "F" };
    let size = [mm(kpad.size[0]), mm(kpad.size[1])];
    let (kind, pad_side) = match kpad.kind {
        PadType::NpThroughHole => (PadKind::NonPlatedHole, PadSide::Top),
        PadType::ThroughHole => (PadKind::PlatedThrough, PadSide::Top),
        PadType::Smd | PadType::Connect => {
            let (on_component, on_other) = if fp.back {
                (back, front)
            } else {
                (front, back)
            };
            let (paste_component, paste_other) = if fp.back {
                (kpad.on("B.Paste"), kpad.on("F.Paste"))
            } else {
                (kpad.on("F.Paste"), kpad.on("B.Paste"))
            };
            let pad_side = match (on_component, on_other) {
                (true, true) => return Err(Case::PadOnBothSides),
                (false, false) => match (paste_component, paste_other) {
                    (true, false) => PadSide::Top,
                    (false, true) => PadSide::Bottom,
                    _ => return Err(Case::PadWithoutCopper),
                },
                (true, false) => PadSide::Top,
                (false, true) => PadSide::Bottom,
            };
            let kind = if kpad.fiducial {
                PadKind::Fiducial
            } else if kpad.number.trim().is_empty() {
                PadKind::Mechanical
            } else {
                PadKind::Smd
            };
            (kind, pad_side)
        }
    };
    // The copper side this pad's mask and paste are on, as KiCad names it.
    let copper_side = match (pad_side, side) {
        (PadSide::Top, s) => s,
        (PadSide::Bottom, "F") => "B",
        (PadSide::Bottom, _) => "F",
    };
    let shape = pad_shape(kpad, size, map, report, fp)?;
    let hole = match kpad.drill {
        Some(d) => {
            let back = local(d.offset);
            let offset = map.point([-back[0], -back[1]]);
            let (w, h) = (nm_exact(d.size[0])?, nm_exact(d.size[1])?);
            let shape = if d.oval && w != h {
                HoleShape::Slot {
                    width_nm: w.min(h),
                    length_nm: w.max(h),
                    angle_mdeg: if w >= h {
                        Angle::ZERO
                    } else {
                        Angle::normalized(90_000)
                    },
                }
            } else {
                HoleShape::Round { diameter_nm: w }
            };
            Some(Hole {
                shape,
                plated: kpad.kind == PadType::ThroughHole,
                offset: nm_point(offset)?,
            })
        }
        None => None,
    };
    let mask = mask_rule(board, fp, kpad, copper_side, report)?;
    let paste = paste_rule(board, fp, kpad, copper_side, size, report)?;
    if matches!(&shape, PadShape::Custom { .. })
        && (matches!(mask, MaskRule::Expansion(e) if e != Nm::ZERO)
            || matches!(paste, PasteRule::Reduction(r) if r != Nm::ZERO))
    {
        report.add(Case::CustomPadOffset);
    }
    let mut pad = Pad {
        id: PadId(0),
        kind,
        terminal: None,
        number: kpad.number.clone(),
        position: nm_point(def.translate)?,
        rotation_mdeg: Angle::from_degrees_f64_rounded(def.rotation_deg)
            .map_err(|_| Case::PadOutOfRange)?,
        side: pad_side,
        shape,
        hole,
        mask,
        paste,
        vias: Vec::new(),
    };
    // A non-plated hole needs a land at least as large as the hole.
    if let (PadKind::NonPlatedHole, Some(h)) = (pad.kind, &pad.hole)
        && let HoleShape::Round { diameter_nm } = h.shape
        && let PadShape::Circle { diameter_nm: land } = &mut pad.shape
        && *land < diameter_nm
    {
        *land = diameter_nm;
    }
    if pad.fold_rotation().is_err() {
        return Err(Case::CustomPadNotOneOutline);
    }
    let mut class = class_of(kpad, fp);
    if paste_ratio_approximate(board, fp, kpad, size) {
        class.push_str(" paste-ratio");
    }
    let ring = pad.shape.flatten(0.001);
    let t = Transform2 {
        translate: pad.position(),
        rotation_deg: pad.rotation_deg(),
        mirror: false,
    }
    .then(&place);
    let world_box = Bounds::of(ring.iter().map(|p| t.apply(*p))).unwrap_or(Bounds::point(center));
    let grow = match pad.mask {
        MaskRule::Expansion(e) => e.to_mm_f64(),
        _ => 0.0,
    };
    let shrink = match pad.paste {
        PasteRule::Reduction(r) => r.to_mm_f64(),
        _ => 0.0,
    };
    // Sharp corners of the land, where a grown or shrunk opening may be
    // rounded (a true offset) or not.
    let vertices: Vec<Point> = match &pad.shape {
        PadShape::Rect { size } => {
            let size = size.to_mm_f64();
            let (x, y) = (size[0] / 2.0, size[1] / 2.0);
            vec![[-x, -y], [x, -y], [x, y], [-x, y]]
        }
        PadShape::Custom { outline } => std::iter::once(outline.start)
            .chain(outline.segments.iter().map(Segment::to))
            .collect(),
        _ => Vec::new(),
    };
    let mut corners = Vec::new();
    for v in vertices {
        let at = t.apply(v);
        for reach in [grow.abs(), shrink.abs()] {
            if reach > 0.0 {
                corners.push((at, reach));
            }
        }
    }
    let mut arcs = Vec::new();
    if kpad.chamfer_ratio > 0.0 && kpad.shape == PadShapeK::RoundRect && kpad.rratio > 0.0 {
        // A chamfered round rect's rounded corners: KiCad draws them as
        // chords.
        let short = size[0].min(size[1]);
        let r = kpad.rratio.clamp(0.0, 0.5) * short;
        let (x, y) = (size[0] / 2.0 - r, size[1] / 2.0 - r);
        for c in [[-x, -y], [x, -y], [x, y], [-x, y]] {
            let at = world.apply(c);
            for radius in [r, r + grow, r - shrink] {
                arcs.push((at, radius));
            }
        }
    }
    if kpad.shape == PadShapeK::Custom {
        // Round primitives, and the round joins and ends of stroked ones.
        let mut round: Vec<(Point, f64)> = Vec::new();
        for primitive in &kpad.primitives {
            match primitive {
                Primitive::Circle {
                    center,
                    radius,
                    width,
                    ..
                } => {
                    round.push((local(*center), radius / 1e6 + mm(*width) / 2.0));
                }
                Primitive::Poly { points, width } if *width > 0 => {
                    round.extend(points.iter().map(|p| (local(*p), mm(*width) / 2.0)));
                }
                Primitive::Rect {
                    start, end, width, ..
                } if *width > 0 => {
                    let (a, b) = (local(*start), local(*end));
                    for c in [a, [b[0], a[1]], b, [a[0], b[1]]] {
                        round.push((c, mm(*width) / 2.0));
                    }
                }
                Primitive::Line { start, end, width } => {
                    round.push((local(*start), mm(*width) / 2.0));
                    round.push((local(*end), mm(*width) / 2.0));
                }
                _ => {}
            }
        }
        if kpad.anchor_circle {
            round.push(([0.0, 0.0], size[0] / 2.0));
        }
        for (c, r) in round {
            let at = world.apply(c);
            for radius in [r, r + grow, r - shrink] {
                arcs.push((at, radius));
            }
        }
    }
    let def = Box::new(DefPad {
        pad,
        class,
        world: world_box,
        arcs,
        corners,
    });
    Ok(if kpad.kind == PadType::Smd && !front && !back {
        Mapped::PasteOnly(def)
    } else {
        Mapped::Pad(def)
    })
}

/// A class name: type, shape, side and turn, never a name or value.
fn class_of(kpad: &KPad, fp: &Footprint) -> String {
    let kind = match kpad.kind {
        PadType::Smd => "smd",
        PadType::ThroughHole => "pth",
        PadType::NpThroughHole => "npth",
        PadType::Connect => "connect",
    };
    let shape = match kpad.shape {
        PadShapeK::Rect => "rect",
        PadShapeK::RoundRect => "roundrect",
        PadShapeK::Circle => "circle",
        PadShapeK::Oval => "oval",
        PadShapeK::Trapezoid => "trapezoid",
        PadShapeK::Custom => "custom",
    };
    let turn = kpad.angle.rem_euclid(360.0);
    let turn = if (turn / 90.0 - (turn / 90.0).round()).abs() < 1e-9 {
        format!("{}", (turn / 90.0).round() as i64 * 90)
    } else {
        "free".to_owned()
    };
    let chamfer = if kpad.chamfer_ratio > 0.0 && kpad.shape == PadShapeK::RoundRect {
        " chamfer"
    } else {
        ""
    };
    format!(
        "{kind} {shape}{chamfer} {} rot{turn}{}",
        if fp.back { "back" } else { "front" },
        if kpad.drill.is_some_and(|d| d.offset != [0, 0]) {
            " offset-drill"
        } else {
            ""
        }
    )
}

/// The mask margin a pad takes: its own, else its footprint's, else the
/// board's (a stored 0 counts as not set).
fn mask_margin(board: &Board, fp: &Footprint, kpad: &KPad) -> i64 {
    kpad.margins
        .mask
        .filter(|v| *v != 0)
        .or(fp.margins.mask.filter(|v| *v != 0))
        .unwrap_or(board.setup.mask_margin)
}

fn mask_rule(
    board: &Board,
    fp: &Footprint,
    kpad: &KPad,
    copper_side: &str,
    report: &mut Report,
) -> Result<MaskRule, Case> {
    let margin = nm_exact(mask_margin(board, fp, kpad))?;
    Ok(match kpad.kind {
        PadType::ThroughHole | PadType::NpThroughHole => {
            match (kpad.on("F.Mask"), kpad.on("B.Mask")) {
                (false, false) => MaskRule::Tented,
                (true, true) => MaskRule::Expansion(margin),
                _ => {
                    report.add_detail(Case::OneSidedThroughMask, &fp.reference, &kpad.number, "");
                    MaskRule::Expansion(margin)
                }
            }
        }
        PadType::Smd | PadType::Connect => {
            if kpad.on(&format!("{copper_side}.Mask")) {
                MaskRule::Expansion(margin)
            } else {
                MaskRule::Tented
            }
        }
    })
}

/// Whether a relative paste margin applies to a pad that is not square.
fn paste_ratio_approximate(board: &Board, fp: &Footprint, kpad: &KPad, size: Point) -> bool {
    let ratio = kpad
        .margins
        .paste_ratio
        .filter(|v| *v != 0.0)
        .or(fp.margins.paste_ratio.filter(|v| *v != 0.0))
        .unwrap_or(board.setup.paste_ratio);
    ratio != 0.0 && (size[0] - size[1]).abs() > 1e-9
}

fn paste_rule(
    board: &Board,
    fp: &Footprint,
    kpad: &KPad,
    copper_side: &str,
    size: Point,
    report: &mut Report,
) -> Result<PasteRule, Case> {
    if !kpad.on(&format!("{copper_side}.Paste")) {
        return Ok(PasteRule::None);
    }
    let margin = kpad
        .margins
        .paste
        .filter(|v| *v != 0)
        .or(fp.margins.paste.filter(|v| *v != 0))
        .unwrap_or(board.setup.paste_margin);
    let ratio = kpad
        .margins
        .paste_ratio
        .filter(|v| *v != 0.0)
        .or(fp.margins.paste_ratio.filter(|v| *v != 0.0))
        .unwrap_or(board.setup.paste_ratio);
    let short = size[0].min(size[1]);
    if ratio != 0.0 && (size[0] - size[1]).abs() > 1e-9 {
        report.add_detail(Case::PasteRatio, &fp.reference, &kpad.number, "");
    }
    Ok(PasteRule::Reduction(nm(-(mm(margin) + ratio * short))?))
}

/// The copper outline in the definition pad frame.
fn pad_shape(
    kpad: &KPad,
    size: Point,
    map: Local,
    report: &mut Report,
    fp: &Footprint,
) -> Result<PadShape, Case> {
    let chamfered = kpad.chamfer_ratio > 0.0
        && (kpad.chamfer.top_left
            || kpad.chamfer.top_right
            || kpad.chamfer.bottom_left
            || kpad.chamfer.bottom_right);
    let exact =
        sparksmith_core::units::PointNm::new(nm_exact(kpad.size[0])?, nm_exact(kpad.size[1])?);
    Ok(match kpad.shape {
        // KiCad writes chamfers on round-rect pads (a ratio of 0 for
        // sharp corners).
        PadShapeK::RoundRect if chamfered => PadShape::Custom {
            outline: chamfered_outline(kpad, size, map),
        },
        PadShapeK::Rect => PadShape::Rect { size: exact },
        PadShapeK::RoundRect => PadShape::RoundRect {
            size: exact,
            radius_ratio: kpad.rratio.clamp(0.0, 0.5),
            max_radius_nm: None,
            corners: CornerMask::ALL,
        },
        PadShapeK::Circle => PadShape::Circle {
            diameter_nm: exact.x_nm,
        },
        PadShapeK::Oval if kpad.size[0] == kpad.size[1] => PadShape::Circle {
            diameter_nm: exact.x_nm,
        },
        PadShapeK::Oval => PadShape::Oblong { size: exact },
        PadShapeK::Trapezoid => {
            report.add_detail(Case::PadShapeUnsupported, &fp.reference, &kpad.number, "");
            return Err(Case::PadShapeUnsupported);
        }
        PadShapeK::Custom => {
            if kpad.unsupported_primitives > 0 {
                report.add_detail(Case::CustomPadPrimitive, &fp.reference, &kpad.number, "");
                return Err(Case::CustomPadPrimitive);
            }
            let (ring, chords) = custom_ring(kpad, size)?;
            if chords {
                report.add(Case::CustomPadChords);
            }
            let ring: Vec<Point> = ring.iter().map(|p| map.point(*p)).collect();
            if ring.len() > MAX_OUTLINE_SEGMENTS {
                report.add_detail(
                    Case::CustomPadNotOneOutline,
                    &fp.reference,
                    &kpad.number,
                    "",
                );
                return Err(Case::CustomPadNotOneOutline);
            }
            PadShape::Custom {
                outline: polygon_outline(&ring),
            }
        }
    })
}

/// A closed outline of straight segments, counter-clockwise.
fn polygon_outline(ring: &[Point]) -> Outline {
    let mut ring: Vec<Point> = ring.iter().map(|p| snap_point(*p)).collect();
    let area: f64 = (0..ring.len())
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if area < 0.0 {
        ring.reverse();
    }
    let start = ring.first().copied().unwrap_or_default();
    let mut segments: Vec<Segment> = ring
        .iter()
        .skip(1)
        .map(|p| Segment::Line { to: *p })
        .collect();
    segments.push(Segment::Line { to: start });
    Outline { start, segments }
}

/// A rectangle with KiCad's chamfered corners cut and its other corners
/// rounded by the round-rect ratio (in our orientation, then mapped).
fn chamfered_outline(kpad: &KPad, size: Point, map: Local) -> Outline {
    let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
    let short = size[0].min(size[1]);
    let c = kpad.chamfer_ratio.clamp(0.0, 0.5) * short;
    let r = if kpad.shape == PadShapeK::RoundRect {
        kpad.rratio.clamp(0.0, 0.5) * short
    } else {
        0.0
    };
    // Corners counter-clockwise in our orientation (y up): KiCad's "top"
    // is −y in its frame, +y in ours.
    let corners = [
        ([-hx, -hy], kpad.chamfer.bottom_left, [1.0, 0.0], [0.0, 1.0]),
        (
            [hx, -hy],
            kpad.chamfer.bottom_right,
            [0.0, 1.0],
            [-1.0, 0.0],
        ),
        ([hx, hy], kpad.chamfer.top_right, [-1.0, 0.0], [0.0, -1.0]),
        ([-hx, hy], kpad.chamfer.top_left, [0.0, -1.0], [1.0, 0.0]),
    ];
    // Each corner: from a point on the incoming edge to one on the
    // outgoing edge, cut straight (chamfer) or by a quarter arc.
    let quarter = (std::f64::consts::PI / 8.0).tan();
    let mut points: Vec<(Point, f64)> = Vec::new();
    for (corner, cut, out_dir, in_dir) in corners {
        let d = if cut { c } else { r };
        if d <= 0.0 {
            points.push((corner, 0.0));
            continue;
        }
        let enter = [corner[0] + in_dir[0] * d, corner[1] + in_dir[1] * d];
        let leave = [corner[0] + out_dir[0] * d, corner[1] + out_dir[1] * d];
        points.push((enter, if cut { 0.0 } else { quarter }));
        points.push((leave, 0.0));
    }
    let mapped: Vec<(Point, f64)> = points
        .iter()
        .map(|(p, b)| (snap_point(map.point(*p)), map.bulge(*b)))
        .collect();
    let (start, _) = mapped.first().copied().unwrap_or_default();
    let mut segments = Vec::new();
    for k in 0..mapped.len() {
        let (_, bulge) = mapped[k];
        let to = mapped[(k + 1) % mapped.len()].0;
        segments.push(if bulge != 0.0 {
            Segment::Arc { to, bulge }
        } else {
            Segment::Line { to }
        });
    }
    let mut outline = Outline { start, segments };
    if map.flip {
        // A reflection turns the ring clockwise: reverse it.
        outline = reversed(&outline);
    }
    outline
}

/// The same outline run the other way.
fn reversed(outline: &Outline) -> Outline {
    let mut points = vec![outline.start];
    points.extend(outline.segments.iter().map(Segment::to));
    let n = outline.segments.len();
    let mut segments = Vec::with_capacity(n);
    for k in (0..n).rev() {
        let to = points[k];
        segments.push(match outline.segments[k] {
            Segment::Line { .. } => Segment::Line { to },
            Segment::Arc { bulge, .. } => Segment::Arc { to, bulge: -bulge },
        });
    }
    Outline {
        start: points[n],
        segments,
    }
}

/// A circle as a ring within [`CHORD_MM`] (vertices on the circle).
fn circle(c: Point, r: f64) -> Vec<Point> {
    let step = if r > CHORD_MM {
        2.0 * (1.0 - CHORD_MM / r).acos()
    } else {
        std::f64::consts::FRAC_PI_2
    };
    let n = ((std::f64::consts::TAU / step).ceil() as usize).clamp(8, MAX_CIRCLE_SEGMENTS);
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

type Rings = Vec<Vec<Point>>;

/// `rings` (one region: an outer ring and its holes) grown by `by` mm with
/// round joins whose chords stay within [`CHORD_MM`].
fn grown(mut rings: Rings, by: f64) -> Vec<Rings> {
    if by <= 0.0 {
        return vec![rings];
    }
    // The offset reads a counter-clockwise ring as an outer boundary: a
    // ring flipped from KiCad's y-down frame runs clockwise.
    if let Some(outer) = rings.first_mut() {
        let area: f64 = (0..outer.len())
            .map(|i| {
                let (a, b) = (outer[i], outer[(i + 1) % outer.len()]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum();
        if area < 0.0 {
            outer.reverse();
        }
    }
    let step = 2.0 * (1.0 - (CHORD_MM / by).min(1.0)).acos().max(0.01);
    let style = OutlineStyle::new(by).line_join(LineJoin::Round(step));
    vec![rings].outline(&style)
}

/// Every point within `r` of the segment `a`–`b`.
fn capsule(a: Point, b: Point, r: f64) -> Vec<Point> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    if len < 1e-12 {
        return circle(a, r);
    }
    let heading = d[1].atan2(d[0]);
    let half = circle([0.0, 0.0], r).len() / 2;
    let cap = |c: Point, from: f64| {
        (0..=half).map(move |k| {
            let t = from + std::f64::consts::PI * k as f64 / half as f64;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        })
    };
    let quarter = std::f64::consts::FRAC_PI_2;
    cap(b, heading - quarter)
        .chain(cap(a, heading + quarter))
        .collect()
}

/// A custom pad's anchor and primitives united into one ring (our
/// orientation, pad frame), and whether a round primitive became chords.
fn custom_ring(kpad: &KPad, size: Point) -> Result<(Vec<Point>, bool), Case> {
    let mut parts: Vec<Rings> = Vec::new();
    let mut chords = false;
    if kpad.anchor_circle {
        parts.push(vec![circle([0.0, 0.0], size[0] / 2.0)]);
        chords = true;
    } else {
        let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
        parts.push(vec![vec![[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]]]);
    }
    for primitive in &kpad.primitives {
        match primitive {
            Primitive::Poly { points, width } => {
                let ring: Vec<Point> = points.iter().map(|p| local(*p)).collect();
                if *width > 0 {
                    chords = true;
                }
                parts.extend(grown(vec![ring], mm(*width) / 2.0));
            }
            Primitive::Circle {
                center,
                radius,
                width,
                fill,
            } => {
                chords = true;
                let c = local(*center);
                let r = radius / 1e6;
                let w = mm(*width) / 2.0;
                if *fill {
                    parts.push(vec![circle(c, r + w)]);
                } else {
                    let mut inner = circle(c, (r - w).max(0.0));
                    inner.reverse();
                    parts.push(vec![circle(c, r + w), inner]);
                }
            }
            Primitive::Rect {
                start,
                end,
                width,
                fill,
            } => {
                let (a, b) = (local(*start), local(*end));
                let ring = vec![a, [b[0], a[1]], b, [a[0], b[1]]];
                let w = mm(*width) / 2.0;
                if w > 0.0 {
                    chords = true;
                }
                if *fill {
                    parts.extend(grown(vec![ring], w));
                } else {
                    for side in 0..4 {
                        parts.push(vec![capsule(ring[side], ring[(side + 1) % 4], w)]);
                    }
                }
            }
            Primitive::Line { start, end, width } => {
                chords = true;
                parts.push(vec![capsule(local(*start), local(*end), mm(*width) / 2.0)]);
            }
        }
    }
    // Each part counter-clockwise, united under the non-zero rule.
    let mut subject: Rings = Vec::new();
    for part in parts {
        let cleaned: Vec<Rings> = part.overlay(
            &Vec::<Vec<Point>>::new(),
            OverlayRule::Subject,
            FillRule::EvenOdd,
        );
        subject.extend(cleaned.into_iter().flatten());
    }
    let union: Vec<Rings> = subject.overlay(
        &Vec::<Vec<Point>>::new(),
        OverlayRule::Subject,
        FillRule::NonZero,
    );
    match union.as_slice() {
        [only] if only.len() == 1 => Ok((only[0].clone(), chords)),
        _ => Err(Case::CustomPadNotOneOutline),
    }
}

/// The package terminal a pad lands, for the pads that need one.
pub fn needs_terminal(pad: &Pad) -> bool {
    matches!(pad.kind, PadKind::Smd | PadKind::PlatedThrough)
}

/// Package terminal ids are 1-based.
pub fn terminal_id(k: usize) -> PackageTerminalId {
    PackageTerminalId(u32::try_from(k + 1).unwrap_or(u32::MAX))
}

/// Add a paste-only pad's opening as a paste window of the pad under its
/// centre on the same side (keeping that pad's own paste opening as a
/// window). Returns false when no pad lies under it.
pub fn add_paste_window(pads: &mut [Pad], window: &Pad) -> bool {
    let reduction = match window.paste {
        PasteRule::Reduction(r) => r.to_mm_f64(),
        _ => 0.0,
    };
    let Some(opening) = window.shape.grown(-reduction, CHORD_MM) else {
        return false;
    };
    let frame = |p: &Pad| Transform2 {
        translate: p.position(),
        rotation_deg: p.rotation_deg(),
        mirror: false,
    };
    let Some(target) = pads.iter_mut().find(|p| {
        p.side == window.side
            && p.kind != PadKind::NonPlatedHole
            && sparksmith_core::geometry::polygon_contains(
                &p.shape.flatten(0.001),
                frame(p).inverse().apply(window.position()),
            )
    }) else {
        return false;
    };
    let into_target = frame(window).then(&frame(target).inverse());
    let mut windows = match &target.paste {
        PasteRule::Windows(w) => w.clone(),
        PasteRule::Reduction(r) => target
            .shape
            .grown(-r.to_mm_f64(), CHORD_MM)
            .and_then(|s| ShapeNm::from_mm_rounded(&s).ok())
            .map(|shape| PasteWindow {
                id: ApertureId(1),
                shape,
            })
            .into_iter()
            .collect(),
        PasteRule::None | PasteRule::Inherit => Vec::new(),
    };
    let Ok(shape) = ShapeNm::from_mm_rounded(&opening.transformed(&into_target)) else {
        return false;
    };
    let next = windows.iter().map(|w| w.id.0).max().unwrap_or(0) + 1;
    windows.push(PasteWindow {
        id: ApertureId(next),
        shape,
    });
    target.paste = PasteRule::Windows(windows);
    true
}
