//! Where a marking sits on its package model: the surface a decal maps
//! onto and how its 2D layout is placed there.
//!
//! **Charts.** Every marking is laid out flat, in a 2D *chart* (mm, origin
//! at the chart centre), and a surface map takes chart points onto the
//! model. A planar face's chart is the face itself ([`MarkingArea`]). A
//! cylinder's chart is its side unrolled by arc length: one chart
//! millimetre is one millimetre on the surface, around and along, so text
//! keeps its proportions ([`CylinderArea`], [`Wrap`]). A freeform surface
//! is the next map of the same kind: a chart parameterised over the face
//! (a least-squares conformal unwrap or a projection along a direction);
//! the layout, placement and clip stay as they are, only
//! [`SurfaceGeometry`] gains a variant. Curved surfaces other than
//! cylinders are not mapped yet.
//!
//! **Surfaces.** [`SurfaceRef::Model`] is the model's marking area (the top
//! face of the outermost body, as before); [`SurfaceRef::Face`] a box face
//! of the model's outermost body (top, bottom or a side); and
//! [`SurfaceRef::Anchor`] a named place a generated family offers
//! ([`anchors`]): a module's `can_top`, `pcb_top` (the board beyond the
//! can) and `antenna_area`, a radial part's `top` and `sleeve`, an axial
//! part's or a MELF's `body`. Anchors are exact, from the form; box faces
//! of an imported model are found in its built mesh.
//!
//! **Placement.** [`Placement`] moves the laid-out marking inside its
//! chart region: an offset, a rotation, a scale and a mirror, then a clip
//! to the region. The layout area is the region seen through the
//! placement (a quarter turn swaps its sides, a scale divides it), so
//! auto-fitted text fills the placed region.
use super::super::model3d::MarkingArea;
use super::super::package_form::PackageForm;
use super::super::package_form::molded::MoldedLead;
use super::super::package_form::tht::ThtBody;
use super::MAX_AREA_NM;
use crate::diagnostics::Result;
use crate::units::{Angle, MILLIDEGREES_PER_TURN, Nm, PointNm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Bytes of an anchor name.
pub const MAX_ANCHOR_NAME: usize = 32;
/// Copies one wrap may repeat around its cylinder.
pub const MAX_WRAP_REPEAT: u32 = 8;
/// Accepted placement scales.
pub const SCALE: std::ops::RangeInclusive<f64> = 0.05..=20.0;
/// Clear margin a module's can-top anchor keeps from the can's rounded
/// edge, mm (its bend radius, est.).
pub const CAN_EDGE_MM: f64 = 0.3;
/// Thickness of a generated module's pads under its board, mm (est.:
/// ENIG over 35 µm copper); the module model builder stands the board on
/// them, so the board's top is this plus its thickness.
pub const MODULE_PAD_MM: f64 = 0.035;
/// Thickness of a generated module's printed antenna trace, mm (est.: 35 µm
/// copper). Board anchors (`pcb_top`, `antenna_area`) stand on it, so a
/// silkscreen prints over the trace as it does over the mask on it.
pub const MODULE_TRACE_MM: f64 = 0.035;
/// The share of a round can top's diameter the `top` anchor's rectangle
/// spans across and along (a rectangle inscribed in the disc).
const DISC_RECT: [f64; 2] = [0.8, 0.6];

/// A box face of the model, in the package frame (z up from the seating
/// plane, +x along the package).
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum BoxFace {
    Top,
    Bottom,
    PosX,
    NegX,
    PosY,
    NegY,
}

impl BoxFace {
    pub const ALL: [Self; 6] = [
        Self::Top,
        Self::Bottom,
        Self::PosX,
        Self::NegX,
        Self::PosY,
        Self::NegY,
    ];

    /// The outward normal and the chart's u axis: a side's text reads
    /// upright (v towards +z) seen from outside; the top's u is +x and
    /// the bottom's −x (its v is then +y seen from below, mirrored as
    /// seen through the board).
    pub fn frame(self) -> ([f64; 3], [f64; 3]) {
        match self {
            Self::Top => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            Self::Bottom => ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0]),
            Self::PosX => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            Self::NegX => ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            Self::PosY => ([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]),
            Self::NegY => ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
        }
    }

    /// The face of an `[length, width, height]` box (z from 0), mm: its
    /// origin and size.
    fn box_face(self, size: [f64; 3]) -> ([f64; 3], [f64; 2]) {
        let [l, w, h] = size;
        match self {
            Self::Top => ([0.0, 0.0, h], [l, w]),
            Self::Bottom => ([0.0, 0.0, 0.0], [l, w]),
            Self::PosX => ([l / 2.0, 0.0, h / 2.0], [w, h]),
            Self::NegX => ([-l / 2.0, 0.0, h / 2.0], [w, h]),
            Self::PosY => ([0.0, w / 2.0, h / 2.0], [l, h]),
            Self::NegY => ([0.0, -w / 2.0, h / 2.0], [l, h]),
        }
    }

    /// The size of the face of an `[length, width, height]` box, mm.
    pub fn of_box_size(self, size: [f64; 3]) -> [f64; 2] {
        self.box_face(size).1
    }

    /// The face of an `[length, width, height]` box (z from 0) as an area.
    pub fn of_box(self, size: [f64; 3]) -> Result<MarkingArea> {
        let (normal, u_axis) = self.frame();
        let (origin, size) = self.box_face(size);
        MarkingArea::from_mm(origin, normal, u_axis, size)
    }
}

/// The surface a marking maps onto.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "surface", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceRef {
    /// The model's marking area (its top face, or the spec's explicit
    /// area on it).
    #[default]
    Model,
    /// A box face of the model's outermost body.
    Face { face: BoxFace },
    /// A named anchor of the package family ([`anchors`]).
    Anchor { name: String },
}

/// How a marking wraps around a cylinder. Angles are about the cylinder's
/// axis from its anchor's zero direction ([`CylinderArea`]), in whole
/// millidegrees; commands carry degrees and millimetres ([`WrapParams`]).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Wrap {
    /// Where the chart's start edge sits. The chart centre is at
    /// `seam + wrap / 2`.
    #[serde(default)]
    pub seam_mdeg: Angle,
    /// How far round the chart reaches, above 0 and at most a full turn
    /// (360,000): a sweep, which [`Angle`] cannot hold (it reduces a full
    /// turn to 0). [`Placement::problem`] checks the range.
    #[serde(default = "full_turn")]
    pub wrap_mdeg: i32,
    /// The part of the side used, along the axis from its start; the whole
    /// side when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub along_nm: Option<[Nm; 2]>,
    /// Copies of the layout side by side round the wrap (1–8): each lays
    /// out in `1 / repeat` of it.
    #[serde(default = "one")]
    pub repeat: u32,
}

pub(super) fn full_turn() -> i32 {
    MILLIDEGREES_PER_TURN
}
pub(super) fn one() -> u32 {
    1
}
pub(super) fn unit_scale() -> f64 {
    1.0
}
pub(super) fn yes() -> bool {
    true
}

impl Default for Wrap {
    fn default() -> Self {
        Self {
            seam_mdeg: Angle::ZERO,
            wrap_mdeg: full_turn(),
            along_nm: None,
            repeat: 1,
        }
    }
}

impl Wrap {
    /// Where the chart's start edge sits, degrees.
    pub fn seam_deg(&self) -> f64 {
        self.seam_mdeg.to_degrees_f64()
    }

    /// How far round the chart reaches, degrees.
    pub fn wrap_deg(&self) -> f64 {
        f64::from(self.wrap_mdeg) / 1_000.0
    }

    /// The part of the side used, mm along the axis from its start.
    pub fn along_mm(&self) -> Option<[f64; 2]> {
        self.along_nm.map(|a| a.map(Nm::to_mm_f64))
    }
}

/// Where and how a marking's layout is placed on its surface. Stored in
/// nanometres and millidegrees; commands carry millimetres and degrees
/// ([`PlacementParams`]).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    #[serde(default)]
    pub on: SurfaceRef,
    /// Shift of the layout's centre from the region's, in the chart.
    #[serde(default)]
    pub offset: PointNm,
    /// Counter-clockwise turn of the layout, seen from outside.
    #[serde(default)]
    pub rotation_mdeg: Angle,
    #[serde(default = "unit_scale")]
    pub scale: f64,
    /// Mirror the layout across its v axis (read through a clear body or
    /// from below).
    #[serde(default)]
    pub mirror: bool,
    /// Cut away what reaches beyond the region.
    #[serde(default = "yes")]
    pub clip: bool,
    /// The wrap on a cylindrical surface; ignored on a planar one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<Wrap>,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            on: SurfaceRef::Model,
            offset: PointNm::ORIGIN,
            rotation_mdeg: Angle::ZERO,
            scale: 1.0,
            mirror: false,
            clip: true,
            wrap: None,
        }
    }
}

impl Placement {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Shift of the layout's centre from the region's, chart mm.
    pub fn offset_mm(&self) -> [f64; 2] {
        self.offset.to_mm_f64()
    }

    /// Counter-clockwise turn of the layout, degrees.
    pub fn rotation_deg(&self) -> f64 {
        self.rotation_mdeg.to_degrees_f64()
    }

    /// The layout area for a region `region` in size: the region seen
    /// through the turn (a quarter turn swaps the sides) and the scale.
    pub fn layout_area(&self, region: [f64; 2]) -> [f64; 2] {
        let (s, c) = self.rotation_deg().to_radians().sin_cos();
        let size = if s.abs() > c.abs() {
            [region[1], region[0]]
        } else {
            region
        };
        let k = if self.scale.is_finite() && self.scale > 0.0 {
            self.scale
        } else {
            1.0
        };
        [size[0] / k, size[1] / k]
    }

    /// Layout → chart: scale, mirror, turn, then shift.
    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        let x = if self.mirror { -p[0] } else { p[0] } * self.scale;
        let y = p[1] * self.scale;
        let (s, c) = self.rotation_deg().to_radians().sin_cos();
        let offset = self.offset_mm();
        [c * x - s * y + offset[0], s * x + c * y + offset[1]]
    }

    /// Why the placement cannot be used, if it cannot.
    pub fn problem(&self) -> Option<String> {
        if !(self.scale.is_finite() && SCALE.contains(&self.scale)) {
            return Some(format!(
                "a marking's scale is {}–{}",
                SCALE.start(),
                SCALE.end()
            ));
        }
        if !(self.offset.x_nm.abs() <= MAX_AREA_NM && self.offset.y_nm.abs() <= MAX_AREA_NM) {
            return Some("a marking's offset and turn must be finite".into());
        }
        if let SurfaceRef::Anchor { name } = &self.on
            && (name.is_empty()
                || name.len() > MAX_ANCHOR_NAME
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
        {
            return Some(format!(
                "an anchor name is 1–{MAX_ANCHOR_NAME} bytes of a-z, 0-9 and _"
            ));
        }
        if let Some(w) = &self.wrap {
            if !(w.wrap_mdeg > 0 && w.wrap_mdeg <= MILLIDEGREES_PER_TURN) {
                return Some("a wrap reaches 1–360°".into());
            }
            if !(1..=MAX_WRAP_REPEAT).contains(&w.repeat) {
                return Some(format!("a wrap repeats 1–{MAX_WRAP_REPEAT} times"));
            }
            if let Some([a, b]) = w.along_nm
                && !(a >= Nm::ZERO && b > a && b <= MAX_AREA_NM)
            {
                return Some("a wrap's height range runs from a start to a later end".into());
            }
        }
        None
    }
}

/// A cylinder's side, in the package frame: the axis from `base` along the
/// unit `axis` for `length`, of `radius`. Angle θ about the axis runs from
/// the unit `zero` (square to the axis) towards `axis × zero`. Lengths are
/// stored in nanometres; [`Self::base`], [`Self::radius`] and
/// [`Self::length`] give millimetres.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CylinderArea {
    pub base_nm: [Nm; 3],
    pub axis: [f64; 3],
    pub zero: [f64; 3],
    pub radius_nm: Nm,
    pub length_nm: Nm,
    /// The chart's u runs along the axis (an axial body's text and bands)
    /// rather than round it (a radial sleeve's).
    pub along_u: bool,
}

/// A wrap resolved on its cylinder: the chart region and its angles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WrapChart {
    /// The chart region, mm: round × along (or along × round when the
    /// cylinder's u runs along the axis).
    pub region: [f64; 2],
    /// Angle of the chart centre, radians.
    pub centre: f64,
    /// Angle the chart spans, radians.
    pub span: f64,
    /// Axis position of the chart centre, mm from the base.
    pub mid: f64,
    /// Axis length used, mm.
    pub along: f64,
    pub repeat: u32,
}

impl CylinderArea {
    /// A cylinder from millimetres, through the one crossing into stored
    /// lengths ([`Nm::from_mm_f64_rounded`]).
    pub fn from_mm(
        base: [f64; 3],
        axis: [f64; 3],
        zero: [f64; 3],
        radius: f64,
        length: f64,
        along_u: bool,
    ) -> Result<Self> {
        let [x, y, z] = base;
        Ok(Self {
            base_nm: [
                Nm::from_mm_f64_rounded(x)?,
                Nm::from_mm_f64_rounded(y)?,
                Nm::from_mm_f64_rounded(z)?,
            ],
            axis,
            zero,
            radius_nm: Nm::from_mm_f64_rounded(radius)?,
            length_nm: Nm::from_mm_f64_rounded(length)?,
            along_u,
        })
    }

    /// The axis' start, mm.
    pub fn base(&self) -> [f64; 3] {
        self.base_nm.map(Nm::to_mm_f64)
    }

    /// The radius, mm.
    pub fn radius(&self) -> f64 {
        self.radius_nm.to_mm_f64()
    }

    /// The side's length along the axis, mm.
    pub fn length(&self) -> f64 {
        self.length_nm.to_mm_f64()
    }

    /// The chart of `wrap` on this cylinder; `None` when its height range
    /// misses the side.
    pub fn chart(&self, wrap: &Wrap) -> Option<WrapChart> {
        let (radius, length) = (self.radius(), self.length());
        let [a, b] = wrap.along_mm().unwrap_or([0.0, length]);
        let (a, b) = (a.max(0.0), b.min(length));
        let along = b - a;
        let span = wrap.wrap_deg().clamp(1.0, 360.0).to_radians();
        let round = radius * span;
        if !(along > 1e-6 && round > 1e-6 && radius > 0.0) {
            return None;
        }
        let region = if self.along_u {
            [along, round]
        } else {
            [round, along]
        };
        Some(WrapChart {
            region,
            centre: wrap.seam_deg().to_radians() + span / 2.0,
            span,
            mid: (a + b) / 2.0,
            along,
            repeat: wrap.repeat.clamp(1, MAX_WRAP_REPEAT),
        })
    }

    /// The angle and axis position of chart point `p` (mm from the chart
    /// centre) under `chart`: arc length round, length along.
    pub fn chart_to_surface(&self, chart: &WrapChart, p: [f64; 2]) -> (f64, f64) {
        let radius = self.radius();
        if self.along_u {
            (chart.centre - p[1] / radius, chart.mid + p[0])
        } else {
            (chart.centre + p[0] / radius, chart.mid + p[1])
        }
    }

    /// The chart point (mm from the chart centre) of the surface point at
    /// angle `theta` (radians) and axis position `t`, the angle taken
    /// within half a turn of the chart centre.
    pub fn surface_to_chart(&self, chart: &WrapChart, theta: f64, t: f64) -> [f64; 2] {
        let tau = std::f64::consts::TAU;
        let d =
            (theta - chart.centre + std::f64::consts::PI).rem_euclid(tau) - std::f64::consts::PI;
        let arc = d * self.radius();
        if self.along_u {
            [t - chart.mid, -arc]
        } else {
            [arc, t - chart.mid]
        }
    }

    /// The point and outward normal at angle `theta` and axis position
    /// `t`, `lift` mm off the side.
    pub fn point(&self, theta: f64, t: f64, lift: f64) -> ([f64; 3], [f64; 3]) {
        let side = cross(self.axis, self.zero);
        let (s, c) = theta.sin_cos();
        let n: [f64; 3] = std::array::from_fn(|k| c * self.zero[k] + s * side[k]);
        let r = self.radius() + lift;
        let base = self.base();
        (
            std::array::from_fn(|k| base[k] + self.axis[k] * t + n[k] * r),
            n,
        )
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A resolved marking surface in the package frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "map", rename_all = "snake_case")]
pub enum SurfaceGeometry {
    Plane(MarkingArea),
    Cylinder(CylinderArea),
}

/// What a marking's layout is told of its chart: flat, or wrapped round a
/// cylinder (for marks placed by angle, an electrolytic's stripe).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Chart {
    #[default]
    Flat,
    Wrap {
        radius_mm: f64,
        /// Angle of the layout centre, radians (0: the anchor's zero
        /// direction).
        centre: f64,
        /// Angle the layout area spans, radians.
        span: f64,
        along_u: bool,
    },
}

/// One named anchor a package offers.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AnchorInfo {
    pub name: String,
    /// What it is, for a person or an agent choosing one.
    pub description: String,
    pub geometry: SurfaceGeometry,
}

/// An upward plane of `size` at `origin`, mm. Computed from a form's
/// stored lengths, it always lies within `Nm`'s range; a value that could
/// not be stored offers no anchor.
fn plane(origin: [f64; 3], size: [f64; 2]) -> Option<MarkingArea> {
    MarkingArea::from_mm(origin, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], size).ok()
}

fn anchor(name: &str, description: &str, geometry: SurfaceGeometry) -> AnchorInfo {
    AnchorInfo {
        name: name.into(),
        description: description.into(),
        geometry,
    }
}

/// A plane with a positive size (at least 1 nm).
fn sized(a: &MarkingArea) -> bool {
    a.size_nm.iter().all(|s| *s > Nm::ZERO)
}

/// `area` as an anchor's geometry when it is sized.
fn sized_plane(area: Option<MarkingArea>) -> Option<SurfaceGeometry> {
    area.filter(sized).map(SurfaceGeometry::Plane)
}

/// A cylinder anchor's geometry from millimetres; a value that could not
/// be stored offers no anchor.
fn cylinder(
    base: [f64; 3],
    axis: [f64; 3],
    zero: [f64; 3],
    radius: f64,
    length: f64,
    along_u: bool,
) -> Option<SurfaceGeometry> {
    CylinderArea::from_mm(base, axis, zero, radius, length, along_u)
        .ok()
        .map(SurfaceGeometry::Cylinder)
}

/// The named anchors a package form offers, in the package frame. Empty
/// for families without any (their box faces and model area still serve).
pub fn anchors(form: &PackageForm) -> Vec<AnchorInfo> {
    let mut out = Vec::new();
    let mut push = |name: &str, what: &str, geometry: Option<SurfaceGeometry>| {
        if let Some(geometry) = geometry {
            out.push(anchor(name, what, geometry));
        }
    };
    match form {
        PackageForm::Module(m) => {
            let top = m.height();
            // The board stands on its pads; its top is the can's seat.
            let pcb = MODULE_PAD_MM + m.pcb_thickness();
            let [cw, cl] = m.can_size();
            let [cx, cy] = m.can_centre();
            let inset = 2.0 * CAN_EDGE_MM;
            let can = plane([cx, cy, top], [cw - inset, cl - inset]);
            push(
                "can_top",
                "the shield can's flat top (laser marking: model, logo, certifications, 2D code)",
                can.map(SurfaceGeometry::Plane),
            );
            for (name, face, origin, size, what) in [
                (
                    "can_pos_x",
                    BoxFace::PosX,
                    [cx + cw / 2.0, cy, (pcb + top) / 2.0],
                    [cl, top - pcb],
                    "the can's +x side",
                ),
                (
                    "can_neg_x",
                    BoxFace::NegX,
                    [cx - cw / 2.0, cy, (pcb + top) / 2.0],
                    [cl, top - pcb],
                    "the can's −x side",
                ),
                (
                    "can_pos_y",
                    BoxFace::PosY,
                    [cx, cy + cl / 2.0, (pcb + top) / 2.0],
                    [cw, top - pcb],
                    "the can's +y side, towards the antenna",
                ),
                (
                    "can_neg_y",
                    BoxFace::NegY,
                    [cx, cy - cl / 2.0, (pcb + top) / 2.0],
                    [cw, top - pcb],
                    "the can's −y side",
                ),
            ] {
                let (normal, u_axis) = face.frame();
                let area = MarkingArea::from_mm(origin, normal, u_axis, size).ok();
                push(name, what, sized_plane(area));
            }
            // The board beyond the can towards the antenna end.
            let (hl, hw) = (m.length() / 2.0, m.width() / 2.0);
            let from = (cy + cl / 2.0).max(-hl);
            let beyond = plane(
                [0.0, (from + hl) / 2.0, pcb + MODULE_TRACE_MM],
                [2.0 * hw, hl - from],
            );
            push(
                "pcb_top",
                "the module board's top beyond the can (silkscreen)",
                sized_plane(beyond),
            );
            if m.antenna_length() > 0.0 {
                let from = hl - m.antenna_length().min(m.length());
                let antenna = plane(
                    [0.0, (from + hl) / 2.0, pcb + MODULE_TRACE_MM],
                    [2.0 * hw, hl - from],
                );
                push(
                    "antenna_area",
                    "the antenna end of the module board (silkscreen beside the trace)",
                    sized_plane(antenna),
                );
            }
        }
        PackageForm::Tht(t) => match t.kind.mm() {
            ThtBody::Radial {
                body_diameter,
                body_height,
            } => {
                let s = t.standoff();
                let r = body_diameter / 2.0;
                // The top's round edge (the model's, est.) is left bare.
                let round = 0.3f64.min(0.1 * body_diameter).min(body_height / 6.0);
                let top = plane(
                    [0.0, 0.0, s + body_height],
                    [
                        DISC_RECT[0] * (body_diameter - 2.0 * round),
                        DISC_RECT[1] * (body_diameter - 2.0 * round),
                    ],
                );
                push(
                    "top",
                    "the can top (vent score, a planar mark)",
                    sized_plane(top),
                );
                if r > 0.0 && body_height > 2.0 * round {
                    push(
                        "sleeve",
                        "the sleeve round the body; angle 0 faces lead 2 (+x, the negative lead)",
                        cylinder(
                            [0.0, 0.0, s],
                            [0.0, 0.0, 1.0],
                            [1.0, 0.0, 0.0],
                            r,
                            body_height - round,
                            false,
                        ),
                    );
                }
            }
            ThtBody::Axial {
                body_diameter,
                body_length,
            } => {
                let r = body_diameter / 2.0;
                let round = 0.3f64.min(0.2 * body_diameter).min(body_length / 6.0);
                let zc = t.standoff() + r;
                if r > 0.0 && body_length > 2.0 * round {
                    push(
                        "body",
                        "the round body lying along x; u runs along it, angle 0 faces down",
                        cylinder(
                            [-body_length / 2.0 + round, 0.0, zc],
                            [1.0, 0.0, 0.0],
                            [0.0, 0.0, -1.0],
                            r,
                            body_length - 2.0 * round,
                            true,
                        ),
                    );
                }
            }
            _ => {}
        },
        PackageForm::Molded(m) if m.lead == MoldedLead::Melf => {
            let r = m.body_width() / 2.0;
            let half = (m.span() / 2.0 - m.terminal_length()).min(m.body_length() / 2.0);
            if r > 0.0 && half > 0.0 {
                push(
                    "body",
                    "the round MELF body between its caps; u runs along it, angle 0 faces down",
                    cylinder(
                        [-half, 0.0, m.body_bottom() + r],
                        [1.0, 0.0, 0.0],
                        [0.0, 0.0, -1.0],
                        r,
                        2.0 * half,
                        true,
                    ),
                );
            }
        }
        _ => {}
    }
    out
}

/// The named anchors a package offers: a part file's derived surfaces
/// (docs/part-templates.md §11), else its form's ([`anchors`]).
pub fn package_anchors(package: &super::super::PackageDefinition) -> Vec<AnchorInfo> {
    match &package.model {
        super::super::PackageModel::PartFile(part) => part.derived.surfaces.clone(),
        _ => anchors(&package.form),
    }
}

/// The anchor `name` of `package`, if it offers one ([`package_anchors`]).
pub fn package_anchor(package: &super::super::PackageDefinition, name: &str) -> Option<AnchorInfo> {
    package_anchors(package)
        .into_iter()
        .find(|a| a.name == name)
}

/// The anchor `name` of `form`, if it offers one.
pub fn anchor_of(form: &PackageForm, name: &str) -> Option<AnchorInfo> {
    anchors(form).into_iter().find(|a| a.name == name)
}

/// The chart region and layout chart of a resolved surface under
/// `placement`: a plane's face, or a cylinder's wrap (one repeat's share).
pub fn region(geometry: &SurfaceGeometry, placement: &Placement) -> Option<([f64; 2], Chart)> {
    match geometry {
        SurfaceGeometry::Plane(area) => sized(area).then_some((area.size(), Chart::Flat)),
        SurfaceGeometry::Cylinder(c) => {
            let wrap = placement.wrap.unwrap_or_default();
            let chart = c.chart(&wrap)?;
            let n = f64::from(chart.repeat);
            let region = if c.along_u {
                [chart.region[0], chart.region[1] / n]
            } else {
                [chart.region[0] / n, chart.region[1]]
            };
            Some((
                region,
                Chart::Wrap {
                    radius_mm: c.radius(),
                    centre: chart.centre,
                    span: chart.span / n,
                    along_u: c.along_u,
                },
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::package_form::module::ModuleForm;
    use crate::library::package_form::tht::{ThtForm, ThtKind};
    use crate::tests::support::nm;

    type Fallible = std::result::Result<(), Box<dyn std::error::Error>>;

    fn sleeve() -> CylinderArea {
        CylinderArea {
            base_nm: [nm(0.0), nm(0.0), nm(0.5)],
            axis: [0.0, 0.0, 1.0],
            zero: [1.0, 0.0, 0.0],
            radius_nm: nm(2.5),
            length_nm: nm(7.0),
            along_u: false,
        }
    }

    #[test]
    fn a_cylinder_chart_keeps_arc_length_and_closes_at_a_full_turn() -> Fallible {
        let c = sleeve();
        let chart = c.chart(&Wrap::default()).ok_or("chart")?;
        // A full turn unrolls to 2πr by the side's length.
        assert!((chart.region[0] - std::f64::consts::TAU * 2.5).abs() < 1e-12);
        assert!((chart.region[1] - 7.0).abs() < 1e-12);
        // One chart millimetre round is one millimetre of arc.
        let (a, _) = c.chart_to_surface(&chart, [0.0, 0.0]);
        let (b, _) = c.chart_to_surface(&chart, [1.0, 0.0]);
        assert!(((b - a) * 2.5 - 1.0).abs() < 1e-12);
        // The two edges of a full turn meet: no gap and no overlap.
        let w = chart.region[0] / 2.0;
        let (start, _) = c.chart_to_surface(&chart, [-w, 0.0]);
        let (end, _) = c.chart_to_surface(&chart, [w, 0.0]);
        let (p, _) = c.point(start, 1.0, 0.0);
        let (q, _) = c.point(end, 1.0, 0.0);
        assert!(p.iter().zip(q).all(|(a, b)| (a - b).abs() < 1e-9));
        assert!(((end - start) - std::f64::consts::TAU).abs() < 1e-12);
        // Back and forth.
        let (theta, t) = c.chart_to_surface(&chart, [3.0, -1.5]);
        let back = c.surface_to_chart(&chart, theta, t);
        assert!((back[0] - 3.0).abs() < 1e-9 && (back[1] + 1.5).abs() < 1e-9);
        // The default seam (0°) puts the chart centre opposite the zero
        // direction: a radial part's positive lead side, −x.
        let (centre, _) = c.chart_to_surface(&chart, [0.0, 0.0]);
        let (p, n) = c.point(centre, 0.0, 0.0);
        assert!((p[0] + 2.5).abs() < 1e-9 && (n[0] + 1.0).abs() < 1e-9);
        // A height range and a partial wrap shrink the region.
        let part = c
            .chart(&Wrap {
                wrap_mdeg: 90_000,
                along_nm: Some([nm(1.0), nm(4.0)]),
                ..Wrap::default()
            })
            .ok_or("partial")?;
        assert!((part.region[0] - 2.5 * std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!((part.region[1] - 3.0).abs() < 1e-12 && (part.mid - 2.5).abs() < 1e-12);
        Ok(())
    }

    #[test]
    fn module_anchors_are_the_can_top_and_the_board_beyond_it() -> Fallible {
        let m = ModuleForm::typical();
        let form = PackageForm::Module(Box::new(m.clone()));
        let names: Vec<String> = anchors(&form).into_iter().map(|a| a.name).collect();
        for n in ["can_top", "pcb_top", "antenna_area", "can_pos_x"] {
            assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
        }
        let Some(SurfaceGeometry::Plane(can)) = anchor_of(&form, "can_top").map(|a| a.geometry)
        else {
            return Err("can top".into());
        };
        assert_eq!(can.origin(), [0.0, -2.5, m.height()]);
        assert!((can.size()[0] - 16.0).abs() < 1e-9 && (can.size()[1] - 16.8).abs() < 1e-9);
        let Some(SurfaceGeometry::Plane(pcb)) = anchor_of(&form, "pcb_top").map(|a| a.geometry)
        else {
            return Err("pcb top".into());
        };
        // The can ends at y = −2.5 + 8.7 = 6.2; the board at 12.5.
        assert!((pcb.size()[1] - (12.5 - 6.2)).abs() < 1e-9);
        assert!(
            (pcb.origin()[2] - (MODULE_PAD_MM + m.pcb_thickness() + MODULE_TRACE_MM)).abs() < 1e-12
        );
        assert!((pcb.origin()[1] - (6.2 + 12.5) / 2.0).abs() < 1e-9);
        Ok(())
    }

    #[test]
    fn a_radial_sleeve_faces_its_negative_lead_at_angle_zero() -> Fallible {
        let form = PackageForm::Tht(Box::new(ThtForm {
            kind: ThtKind::Radial {
                body_diameter_nm: nm(5.0),
                body_height_nm: nm(11.0),
            },
            ..ThtForm::typical()
        }));
        let Some(SurfaceGeometry::Cylinder(c)) = anchor_of(&form, "sleeve").map(|a| a.geometry)
        else {
            return Err("sleeve".into());
        };
        assert_eq!(c.zero, [1.0, 0.0, 0.0]);
        assert_eq!(c.radius(), 2.5);
        let p = Placement {
            on: SurfaceRef::Anchor {
                name: "sleeve".into(),
            },
            wrap: Some(Wrap {
                repeat: 2,
                ..Wrap::default()
            }),
            ..Placement::default()
        };
        let (region, chart) = region(&SurfaceGeometry::Cylinder(c), &p).ok_or("region")?;
        assert!((region[0] - std::f64::consts::PI * 2.5).abs() < 1e-9);
        assert!(
            matches!(chart, Chart::Wrap { span, .. } if (span - std::f64::consts::PI).abs() < 1e-12)
        );
        Ok(())
    }

    #[test]
    fn placements_turn_scale_and_mirror_about_the_centre() {
        let p = Placement {
            rotation_mdeg: crate::units::Angle::from_degrees_f64_rounded(90.0).unwrap_or_default(),
            scale: 2.0,
            mirror: true,
            offset: PointNm::new(nm(1.0), nm(0.0)),
            ..Placement::default()
        };
        let q = p.apply([1.0, 0.0]);
        // Mirrored to −1, doubled to −2, turned to (0, −2), shifted by 1.
        assert!((q[0] - 1.0).abs() < 1e-12 && (q[1] + 2.0).abs() < 1e-12);
        // A quarter turn swaps the area's sides; the scale divides them.
        assert_eq!(p.layout_area([4.0, 2.0]), [1.0, 2.0]);
        assert!(p.problem().is_none());
        let bad = Placement {
            scale: 0.0,
            ..Placement::default()
        };
        assert!(bad.problem().is_some());
        let bad = Placement {
            on: SurfaceRef::Anchor {
                name: "Can Top".into(),
            },
            ..Placement::default()
        };
        assert!(bad.problem().is_some());
    }

    #[test]
    fn box_faces_read_upright_from_outside() {
        for face in BoxFace::ALL {
            let (n, u) = face.frame();
            let v = cross(n, u);
            // u × v = n: a right-handed chart seen along −n.
            let back = cross(u, v);
            assert!(
                back.iter().zip(n).all(|(a, b)| (a - b).abs() < 1e-12),
                "{face:?}"
            );
            if !matches!(face, BoxFace::Top | BoxFace::Bottom) {
                assert_eq!(v, [0.0, 0.0, 1.0], "{face:?} reads upright");
            }
        }
    }
}
