//! Footprint definitions: the solved copper lands, holes, mask/paste rules
//! and drawings of one exact package revision. Consumers read only this
//! solved data; an optional [`FootprintSource`] records how it was derived
//! and is interpreted by a [`FootprintEvaluator`](super::footprint_source::FootprintEvaluator)
//! outside the core. Opening a project never needs the solver.
use super::footprint_source::FootprintSource;
use super::graphics::{FootprintGraphic, Polarity, ShapeNm};
use super::package::PackageDefinition;
use super::publish::{issue, object};
use super::{Library, MAX_PADS};
use crate::diagnostics::{Finding, ObjectRef, Result};
use crate::geometry::{Bounds, Point, finite_within, point_valid};
use crate::ids::{DefinitionId, PackageRef, PackageTerminalId, PadId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod checks;
mod geometry;
pub use checks::{MIN_MASK_WEB_MM, MIN_SILK_TEXT_HEIGHT_MM, MIN_SILK_WIDTH_MM};
pub use geometry::{CHECK_CHORD_MM, PAD_ARC_TOLERANCE_MM, TerminalRule, bulge_arc, offset_polygon};

/// Default solder-mask expansion around a pad, in mm (an estimated prototype
/// default, not a manufacturer qualification).
pub const MASK_EXPANSION_MM: f64 = 0.05;
/// Segments in one custom pad [`Outline`].
pub const MAX_OUTLINE_SEGMENTS: usize = 512;
/// Paste windows on one pad.
pub const MAX_PASTE_WINDOWS: usize = 64;
/// Pad-owned vias on one pad.
pub const MAX_PAD_VIAS: usize = 256;
/// Largest |bulge| of an outline arc (a bulge of 1 is a half circle).
pub const MAX_BULGE: f64 = 1e3;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FootprintDefinition {
    /// The package revision whose terminals the pads land.
    pub package: PackageRef,
    /// Next free local ID of this lineage (pads, graphics and source arrays
    /// share the namespace).
    pub next_local: u32,
    /// How the solved data was derived. `None` is an explicit footprint
    /// (numeric edits, Rhai, `Detach`); with a source, `pads` and the
    /// sketch-role graphics are derived and stored together with it.
    pub source: Option<Box<FootprintSource>>,
    pub pads: Vec<Pad>,
    /// Silkscreen, courtyard and assembly drawings, in draw order.
    pub graphics: Vec<FootprintGraphic>,
    /// The authored pin-1 pad and the graphics that illustrate it.
    pub polarity: Option<Polarity>,
    /// The package datum in the footprint frame.
    pub datum: Datum,
    /// Footprint-wide mask and paste rules pads inherit.
    pub defaults: PadDefaults,
    /// Copper keep-out regions the part needs on the host board (an
    /// antenna's clearance), in the footprint frame. Never inferred.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keepouts: Vec<FootprintKeepout>,
}

/// Most keep-out regions of one footprint.
pub const MAX_FOOTPRINT_KEEPOUTS: usize = 16;

/// Which copper layers a footprint keep-out covers.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeepoutScope {
    /// Every copper layer of the board.
    #[default]
    AllCopper,
    /// The outer layer the part is placed on.
    ComponentSide,
}

/// A copper keep-out a footprint carries: its closed outline in the
/// footprint frame. Placed, it forbids fill copper, tracks and vias (and,
/// with `forbid_pads`, other parts' pads) inside the outline on its
/// layers, exactly like a board keepout.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FootprintKeepout {
    /// Display name, e.g. "antenna".
    #[serde(default)]
    pub name: String,
    pub outline: Outline,
    #[serde(default)]
    pub scope: KeepoutScope,
    #[serde(default)]
    pub forbid_pads: bool,
}

impl FootprintKeepout {
    /// An axis-aligned rectangle keep-out on every copper layer.
    pub fn rect(name: &str, min: Point, max: Point) -> Self {
        Self {
            name: name.to_owned(),
            outline: Outline {
                start: min,
                segments: vec![
                    Segment::Line {
                        to: [max[0], min[1]],
                    },
                    Segment::Line { to: max },
                    Segment::Line {
                        to: [min[0], max[1]],
                    },
                    Segment::Line { to: min },
                ],
            },
            scope: KeepoutScope::AllCopper,
            forbid_pads: false,
        }
    }
}

/// The package datum (the placement origin) in the footprint frame: the
/// placed footprint is moved by `-offset`, then turned by `-rotation_deg`
/// so the datum's axes line up with the placement's.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Datum {
    pub offset: Point,
    /// Counter-clockwise rotation of the datum axes in the footprint frame.
    pub rotation_deg: f64,
}

/// Footprint-wide pad rules that [`MaskRule::Inherit`] and
/// [`PasteRule::Inherit`] resolve to.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadDefaults {
    /// Mask opening grows by this much on every side, in mm.
    pub mask_expansion_mm: f64,
    /// Paste opening shrinks by this much on every side, in mm.
    pub paste_reduction_mm: f64,
}

impl Default for PadDefaults {
    fn default() -> Self {
        Self {
            mask_expansion_mm: MASK_EXPANSION_MM,
            paste_reduction_mm: 0.0,
        }
    }
}

/// One solved land (or mechanical hole) of a footprint, footprint-local.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    pub id: PadId,
    pub kind: PadKind,
    /// The package terminal landed. Required for `Smd` and `PlatedThrough`,
    /// forbidden for `NonPlatedHole` and `Fiducial`, optional for
    /// `Mechanical`; never inferred from `number`.
    pub terminal: Option<PackageTerminalId>,
    /// Display label ("1", "A1", "EP", "MH1"); not unique, not identity.
    pub number: String,
    /// The pad origin (shape centre) in the footprint frame.
    pub position: Point,
    /// Counter-clockwise rotation of the shape about `position`,
    /// canonical per shape symmetry (see [`PadShape::canonical_rotation`]).
    pub rotation_deg: f64,
    /// The copper side of a surface pad. Through-hole pads span all copper
    /// layers and record `Top`.
    pub side: PadSide,
    pub shape: PadShape,
    pub hole: Option<Hole>,
    pub mask: MaskRule,
    pub paste: PasteRule,
    /// Pad-owned plated holes; they inherit the pad's terminal.
    pub vias: Vec<PadVia>,
}

/// What a pad is for.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PadKind {
    /// Surface copper on one side.
    #[default]
    Smd,
    /// Copper on every layer around a plated hole.
    PlatedThrough,
    /// A bare drilled hole with no copper (`shape` is the keep-out land).
    NonPlatedHole,
    /// An optical alignment mark; copper, no terminal, no paste.
    Fiducial,
    /// Copper with an explicit decision not to land a terminal (mounting
    /// tabs, shield legs).
    Mechanical,
}

impl PadKind {
    /// The kind's name in messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Smd => "SMD",
            Self::PlatedThrough => "plated through-hole",
            Self::NonPlatedHole => "non-plated hole",
            Self::Fiducial => "fiducial",
            Self::Mechanical => "mechanical",
        }
    }
    /// Whether this kind is drilled through the board (plated or bare).
    /// Its copper is [`Self::copper`].
    pub fn through_hole(self) -> bool {
        matches!(self, Self::PlatedThrough | Self::NonPlatedHole)
    }

    /// Which copper a pad of this kind has: the one definition, which
    /// `layout::pad_copper_on` turns into copper layers. A bare hole has
    /// none.
    pub fn copper(self) -> PadCopper {
        match self {
            Self::NonPlatedHole => PadCopper::None,
            Self::PlatedThrough => PadCopper::Through,
            Self::Smd | Self::Fiducial | Self::Mechanical => PadCopper::OneSide,
        }
    }
}

/// Which copper a pad has ([`PadKind::copper`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadCopper {
    /// No copper: a bare drilled hole.
    None,
    /// A land on the pad's own side.
    OneSide,
    /// A land on every copper layer, joined by a plated barrel.
    Through,
}

/// The copper side of a surface pad, relative to the placed component.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PadSide {
    /// The side the component is placed on.
    #[default]
    Top,
    /// The opposite side.
    Bottom,
}

/// Which corners of a rounded rectangle are rounded, in the pad frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CornerMask {
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_right: bool,
    pub bottom_left: bool,
}

impl CornerMask {
    pub const ALL: Self = Self {
        top_left: true,
        top_right: true,
        bottom_right: true,
        bottom_left: true,
    };
}

/// A pad's copper outline in the pad frame, centred on the pad origin.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PadShape {
    Rect {
        size: Point,
    },
    /// Rounded corners of `radius_ratio` (0–0.5) of the short side, capped
    /// at `max_radius` mm. A D-shape is two corners at 0.5.
    RoundRect {
        size: Point,
        radius_ratio: f64,
        max_radius: Option<f64>,
        corners: CornerMask,
    },
    Circle {
        diameter: f64,
    },
    /// A stadium: semicircular ends on the long axis.
    Oblong {
        size: Point,
    },
    /// An exact closed outline of lines and arcs.
    Custom {
        outline: Outline,
    },
}

impl PadShape {
    /// The axis-aligned extent in the pad frame (unrotated), centred on
    /// the pad origin for primitives. Custom outlines use their vertex box,
    /// which is exact for lines; arcs may bulge past it.
    pub fn extent(&self) -> Point {
        match self {
            Self::Rect { size } | Self::RoundRect { size, .. } | Self::Oblong { size } => *size,
            Self::Circle { diameter } => [*diameter, *diameter],
            Self::Custom { outline } => outline.vertex_bounds().size(),
        }
    }

    /// Replace the extent of a sized primitive. Returns false (unchanged)
    /// for circles and custom outlines.
    pub fn set_size(&mut self, new: Point) -> bool {
        match self {
            Self::Rect { size } | Self::RoundRect { size, .. } | Self::Oblong { size } => {
                *size = new;
                true
            }
            Self::Circle { .. } | Self::Custom { .. } => false,
        }
    }

    /// Rotation normalised per shape symmetry: rect, rounded rect (all
    /// corners) and oblong to [0°, 180°), circle to 0, custom and
    /// partially rounded rects to [0°, 360°). Diffs compare canonical values.
    pub fn canonical_rotation(&self, deg: f64) -> f64 {
        match self {
            Self::Circle { .. } => 0.0,
            Self::Rect { .. } | Self::Oblong { .. } => deg.rem_euclid(180.0),
            Self::RoundRect { corners, .. } if *corners == CornerMask::ALL => deg.rem_euclid(180.0),
            Self::RoundRect { .. } | Self::Custom { .. } => deg.rem_euclid(360.0),
        }
    }

    pub(super) fn finite(&self) -> bool {
        match self {
            Self::Rect { size } | Self::Oblong { size } => finite_within(size),
            Self::RoundRect {
                size,
                radius_ratio,
                max_radius,
                ..
            } => {
                finite_within(size)
                    && radius_ratio.is_finite()
                    && max_radius.is_none_or(f64::is_finite)
            }
            Self::Circle { diameter } => finite_within(&[*diameter]),
            Self::Custom { outline } => outline.finite(),
        }
    }
}

/// A closed exact outline in the pad frame: from `start`, each segment runs
/// to its `to`; the last must end at `start`. At most
/// [`MAX_OUTLINE_SEGMENTS`] segments. Flattening happens only for display
/// and checks, at an explicit tolerance.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Outline {
    pub start: Point,
    pub segments: Vec<Segment>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Segment {
    Line {
        to: Point,
    },
    /// A circular arc in bulge form: `bulge = tan(sweep / 4)`, positive
    /// counter-clockwise; 0 is a straight line.
    Arc {
        to: Point,
        bulge: f64,
    },
}

impl Segment {
    pub fn to(&self) -> Point {
        match self {
            Self::Line { to } | Self::Arc { to, .. } => *to,
        }
    }
}

impl Outline {
    /// The box of the start and segment end points (arcs may bulge past it).
    pub fn vertex_bounds(&self) -> Bounds {
        self.segments
            .iter()
            .fold(Bounds::point(self.start), |b, s| b.including(s.to()))
    }

    pub(super) fn finite(&self) -> bool {
        point_valid(self.start)
            && self.segments.iter().all(|s| match s {
                Segment::Line { to } => point_valid(*to),
                Segment::Arc { to, bulge } => {
                    point_valid(*to) && bulge.is_finite() && bulge.abs() <= MAX_BULGE
                }
            })
    }
}

/// A drilled hole through the pad, in the pad frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Hole {
    pub shape: HoleShape,
    /// Whether the barrel is plated (copper on every layer joins it).
    pub plated: bool,
    /// Hole centre relative to the pad origin, in the pad frame.
    pub offset: Point,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum HoleShape {
    /// A round drill.
    Round { diameter: f64 },
    /// A milled slot: `length` overall along `angle_deg` (pad frame), with
    /// round ends of `width`.
    Slot {
        width: f64,
        length: f64,
        angle_deg: f64,
    },
}

impl HoleShape {
    fn finite(&self) -> bool {
        match self {
            Self::Round { diameter } => finite_within(&[*diameter]),
            Self::Slot {
                width,
                length,
                angle_deg,
            } => finite_within(&[*width, *length]) && angle_deg.is_finite(),
        }
    }
}

/// The pad's solder-mask opening.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MaskRule {
    /// [`PadDefaults::mask_expansion_mm`].
    #[default]
    Inherit,
    /// Covered by mask: no opening.
    Tented,
    /// The shape grown by this many mm (negative shrinks).
    Expansion(f64),
}

/// The pad's solder-paste opening.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PasteRule {
    /// [`PadDefaults::paste_reduction_mm`] (none for through-hole kinds and
    /// fiducials).
    #[default]
    Inherit,
    /// No paste.
    None,
    /// The shape shrunk by this many mm.
    Reduction(f64),
    /// Explicit windows (exposed pads), at most [`MAX_PASTE_WINDOWS`].
    Windows(Vec<PasteWindow>),
}

/// One paste aperture of a pad, in the pad frame.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PasteWindow {
    /// Pad-local aperture identity, stable across re-evaluations.
    pub id: ApertureId,
    pub shape: ShapeNm,
}

/// A pad-owned plated hole (thermal or via-in-pad), in the pad frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadVia {
    pub id: ApertureId,
    pub at: Point,
    pub drill: f64,
}

/// A paste window or pad via, unique within its pad.
#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct ApertureId(pub u32);

impl Pad {
    /// Normalise `rotation_deg` per shape symmetry without moving anything:
    /// the turn the fold removes is applied to the pad-frame features the
    /// shape symmetry does not cover (hole offset and slot angle, vias,
    /// paste windows), so the pad's world geometry is unchanged. A paste
    /// window turned by other than quarter turns is stored rounded to
    /// nanometres; an error, leaving the pad unchanged, when it leaves the
    /// coordinate bound.
    pub fn fold_rotation(&mut self) -> Result<()> {
        let canonical = self.shape.canonical_rotation(self.rotation_deg);
        let removed = (self.rotation_deg - canonical).rem_euclid(360.0);
        if removed.abs() < 1e-12 || (360.0 - removed).abs() < 1e-12 {
            self.rotation_deg = canonical;
            return Ok(());
        }
        let turn = crate::geometry::Transform2 {
            rotation_deg: removed,
            ..crate::geometry::Transform2::IDENTITY
        };
        let windows = match &self.paste {
            PasteRule::Windows(windows) => windows
                .iter()
                .map(|w| {
                    Ok(PasteWindow {
                        id: w.id,
                        shape: ShapeNm::from_mm_rounded(&w.shape.to_mm().transformed(&turn))?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            _ => Vec::new(),
        };
        self.rotation_deg = canonical;
        if let Some(hole) = &mut self.hole {
            hole.offset = turn.apply(hole.offset);
            if let HoleShape::Slot { angle_deg, .. } = &mut hole.shape {
                *angle_deg = (*angle_deg + removed).rem_euclid(180.0);
            }
        }
        for via in &mut self.vias {
            via.at = turn.apply(via.at);
        }
        if let PasteRule::Windows(w) = &mut self.paste {
            *w = windows;
        }
        Ok(())
    }

    /// A rectangular top-side SMD land with inherited mask and paste: the
    /// pad every footprint had before shapes were typed.
    pub fn smd_rect(
        id: PadId,
        terminal: PackageTerminalId,
        number: impl Into<String>,
        position: Point,
        size: Point,
    ) -> Self {
        Self {
            id,
            kind: PadKind::Smd,
            terminal: Some(terminal),
            number: number.into(),
            position,
            rotation_deg: 0.0,
            side: PadSide::Top,
            shape: PadShape::Rect { size },
            hole: None,
            mask: MaskRule::Inherit,
            paste: PasteRule::Inherit,
            vias: Vec::new(),
        }
    }

    /// The axis-aligned box of the pad's extent in the footprint frame
    /// (rotation included; custom arcs not bulged).
    pub fn bounds(&self) -> Bounds {
        let [w, h] = self.shape.extent();
        let (c, s) = crate::geometry::cos_sin(self.rotation_deg);
        let hw = 0.5 * (w * c.abs() + h * s.abs());
        let hh = 0.5 * (w * s.abs() + h * c.abs());
        Bounds {
            min: [self.position[0] - hw, self.position[1] - hh],
            max: [self.position[0] + hw, self.position[1] + hh],
        }
    }

    fn finite(&self) -> bool {
        point_valid(self.position)
            && self.rotation_deg.is_finite()
            && self.shape.finite()
            && self
                .hole
                .is_none_or(|h| h.shape.finite() && point_valid(h.offset))
            && match &self.mask {
                MaskRule::Expansion(v) => v.is_finite(),
                _ => true,
            }
            && match &self.paste {
                PasteRule::Reduction(v) => v.is_finite(),
                PasteRule::Windows(w) => w.iter().all(|w| w.shape.valid()),
                _ => true,
            }
            && self
                .vias
                .iter()
                .all(|v| point_valid(v.at) && finite_within(&[v.drill]))
    }

    fn within_bounds(&self) -> bool {
        self.number.len() <= super::MAX_LABEL
            && self.vias.len() <= MAX_PAD_VIAS
            && match &self.shape {
                PadShape::Custom { outline } => outline.segments.len() <= MAX_OUTLINE_SEGMENTS,
                _ => true,
            }
            && match &self.paste {
                PasteRule::Windows(w) => w.len() <= MAX_PASTE_WINDOWS,
                _ => true,
            }
    }
}

impl FootprintDefinition {
    pub fn pad(&self, id: PadId) -> Option<&Pad> {
        self.pads.iter().find(|p| p.id == id)
    }

    /// The authored pin-1 pad.
    pub fn polarity_pad(&self) -> Option<&Pad> {
        self.polarity.as_ref().and_then(|p| self.pad(p.pad))
    }

    /// The package envelope's footprint (its box seen from above) in the
    /// footprint-local frame: the envelope corners through
    /// [`model_frame`](super::model_frame), so the datum and the model
    /// placement both apply. The one frame the part editor, PCB 2D and the
    /// courtyard use. Exact at quarter turns.
    pub fn envelope_outline(&self, package: &PackageDefinition) -> Bounds {
        let frame = super::model_frame(&self.datum, &package.placement);
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for corner in package.envelope_corners() {
            let p = frame.apply(corner);
            for axis in 0..2 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        Bounds { min, max }
    }

    pub(super) fn within_bounds(&self) -> bool {
        self.pads.len() <= MAX_PADS
            && self.pads.iter().all(Pad::within_bounds)
            && self.graphics.len() <= super::graphics::MAX_GRAPHICS
            && self.graphics.iter().all(|g| g.item.within_bounds())
            && self
                .graphics
                .iter()
                .map(|g| g.item.vertices())
                .sum::<usize>()
                <= super::graphics::MAX_VERTICES
            && self
                .polarity
                .as_ref()
                .is_none_or(|p| p.markers.len() <= super::graphics::MAX_POLARITY_MARKERS)
            && self.source.as_ref().is_none_or(|s| s.within_bounds())
            && self.keepouts.len() <= MAX_FOOTPRINT_KEEPOUTS
            && self.keepouts.iter().all(|k| {
                k.outline.segments.len() <= MAX_OUTLINE_SEGMENTS
                    && k.name.len() <= crate::ids::MAX_LABEL
            })
    }

    pub(super) fn geometry_finite(&self) -> bool {
        point_valid(self.datum.offset)
            && self.datum.rotation_deg.is_finite()
            && finite_within(&[
                self.defaults.mask_expansion_mm,
                self.defaults.paste_reduction_mm,
            ])
            && self.pads.iter().all(Pad::finite)
            && self.graphics.iter().all(|g| g.item.finite())
            && self.keepouts.iter().all(|k| k.outline.finite())
    }

    pub(super) fn local_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.pads.iter().map(|p| p.id.0).collect();
        ids.extend(self.graphics.iter().map(|g| g.id.0));
        if let Some(source) = &self.source {
            ids.extend(source.arrays.iter().map(|a| a.id.0));
        }
        ids
    }

    pub(super) fn findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        // The footprint of a terminal-less package (a purely mechanical
        // part, docs/mechanical-parts.md) may have no pads.
        let least = usize::from(
            library
                .package(self.package)
                .is_none_or(|p| !p.definition.terminals.is_empty()),
        );
        if !(least..=MAX_PADS).contains(&self.pads.len()) {
            out.push(issue(
                "footprint.pad_count",
                format!("A footprint needs {least}–{MAX_PADS} pads."),
                vec![this],
            ));
        }
        self.local_findings(id, out);
        for (k, keepout) in self.keepouts.iter().enumerate() {
            if !keepout.outline.closed() || keepout.outline.segments.len() < 2 {
                out.push(issue(
                    "footprint.keepout_open",
                    format!(
                        "Keep-out {} needs a closed outline of at least two segments.",
                        if keepout.name.is_empty() {
                            (k + 1).to_string()
                        } else {
                            keepout.name.clone()
                        }
                    ),
                    vec![this],
                ));
            }
        }
        let Some(package) = library.package(self.package) else {
            out.push(issue(
                "reference.missing",
                format!(
                    "{} is missing from the project library.",
                    super::names::capitalised(&library.describe(self.package.erased()))
                ),
                vec![
                    this,
                    ObjectRef::Definition {
                        id: self.package.id,
                    },
                ],
            ));
            return;
        };
        let terminals: BTreeSet<PackageTerminalId> =
            package.definition.terminals.iter().map(|t| t.id).collect();
        let mut landed = BTreeSet::new();
        for pad in &self.pads {
            let Some(terminal) = pad.terminal else {
                continue;
            };
            if terminals.contains(&terminal) {
                landed.insert(terminal);
            } else {
                out.push(issue(
                    "footprint.pad_terminal_unknown",
                    format!(
                        "Pad {} lands a package terminal the package does not have.",
                        checks::pad_label(pad)
                    ),
                    vec![object(id, pad.id.0)],
                ));
            }
        }
        for terminal in terminals.difference(&landed) {
            let designation = package
                .definition
                .terminal(*terminal)
                .map_or_else(|| terminal.to_string(), |t| t.designation.clone());
            out.push(issue(
                "footprint.terminal_without_pad",
                format!("Package terminal ‘{designation}’ has no pad."),
                vec![this, object(self.package.id, terminal.0)],
            ));
        }
        // Advisory: where the package's 3D model sits on these lands.
        out.extend(super::package_fit::placement_findings(
            &package.definition,
            self,
            id,
        ));
    }
}
