//! Design rules: the manufacturing limits every board check, estimator,
//! router and fill reads, grouped into a [`RuleProfile`] (a named,
//! provenance-carrying set of [`DesignRules`]), per-project overrides and
//! net classes ([`netclass`]).
//!
//! A profile is a file with a stated source ([`profile`]); the project
//! embeds a snapshot of the one it uses ([`ProjectRules::profile`]), so a
//! file opens with the same rules on any machine. A user profile library
//! (JSON files in the user config folder, read by `pcbcraft-io`) supplies
//! profiles to choose. A project without a profile has no rules: nothing
//! is checked, the production check blocks and tools that need a default
//! refuse ([`ProjectRules::require`]).
//!
//! Resolution order, lowest to highest precedence ([`RuleSource`]):
//! profile value, project override, then (for copper rules that name nets)
//! net class values and the class-to-class clearance matrix. Regions are a
//! reserved later step. Clearances between two objects take the larger of
//! every applicable value; widths take the net class value over the
//! effective rule.
//!
//! Every value here is pure data: no files and no fabricator data;
//! Pcbcraft ships no built-in profile.
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod formula;
pub mod netclass;
pub mod profile;
pub mod signal;

pub use profile::{
    DrillFiles, PROFILE_FILE_FORMAT, ProfileFile, ProfileSource, RuleEntry, RuleProfile, Setting,
};

pub use netclass::{
    ClassClearance, MAX_NET_CLASSES, NetClass, NetClassAssignment, NetClassId, NetClassPattern,
    NetClassSpec, glob_matches,
};

/// Largest length rule, in mm.
pub const MAX_RULE_MM: f64 = 25.0;
/// `board.outline_fit`'s default, mm: the chord tolerance the board solve
/// used before free-form curves.
pub const DEFAULT_OUTLINE_FIT_MM: f64 = 0.01;
/// The range `board.outline_fit` takes, mm: finer fits need more arcs than
/// an outline holds, coarser ones would move the board edge visibly.
pub const OUTLINE_FIT_RANGE_MM: std::ops::RangeInclusive<f64> = 0.001..=1.0;
/// Most overrides one project holds (one per [`RuleKey`] at most).
pub const MAX_OVERRIDES: usize = 256;
/// Bytes of a profile's name, id or one provenance text.
pub const MAX_PROFILE_TEXT: usize = 256;
/// Bytes of a profile's notes.
pub const MAX_PROFILE_NOTES: usize = 4096;

/// The kinds of copper-related object a clearance rule pairs.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum CopperObject {
    Track,
    Pad,
    Via,
    Fill,
    /// The board outline and every cutout or milled slot edge.
    BoardEdge,
    /// A via's drilled hole wall, and any unplated hole's (an unplated pad
    /// or mounting hole).
    ViaHole,
    /// A plated pad or mounting hole's wall (PTH): fabricators keep more
    /// copper and drill spacing around these than around via holes.
    PthHole,
}

impl CopperObject {
    pub const ALL: [Self; 7] = [
        Self::Track,
        Self::Pad,
        Self::Via,
        Self::Fill,
        Self::BoardEdge,
        Self::ViaHole,
        Self::PthHole,
    ];

    /// The hole kind of a drilled hole: plated pad and mounting holes are
    /// [`Self::PthHole`]; via drills and unplated holes [`Self::ViaHole`].
    pub fn hole(plated: bool) -> Self {
        if plated { Self::PthHole } else { Self::ViaHole }
    }

    /// A hole wall, not copper.
    pub fn is_hole(self) -> bool {
        matches!(self, Self::ViaHole | Self::PthHole)
    }

    /// Copper (track, pad, via or fill): not a hole wall or a board edge.
    pub fn is_copper(self) -> bool {
        !self.is_hole() && self != Self::BoardEdge
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Pad => "pad",
            Self::Via => "via",
            Self::Fill => "fill",
            Self::BoardEdge => "board edge",
            Self::ViaHole => "via hole",
            Self::PthHole => "plated hole",
        }
    }
}

/// One clearance of the object-pair matrix, in mm; the pair is unordered
/// and stored with `a <= b`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PairClearance {
    pub a: CopperObject,
    pub b: CopperObject,
    pub mm: f64,
}

/// Least gap between copper of different nets (and holes and edges), by
/// object pair. Every unordered pair of [`CopperObject`] has exactly one
/// entry (28 entries), in [`CopperObject::ALL`] order.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClearanceMatrix {
    pub pairs: Vec<PairClearance>,
}

impl ClearanceMatrix {
    /// Every pair at `copper` mm, with the hole and edge pairs given (both
    /// hole kinds alike).
    pub fn uniform(copper: f64, hole_to_copper: f64, hole_to_hole: f64, edge: f64) -> Self {
        let mut pairs = Vec::new();
        for (i, a) in CopperObject::ALL.iter().enumerate() {
            for b in &CopperObject::ALL[i..] {
                let mm = match (a, b) {
                    (_, CopperObject::BoardEdge) | (CopperObject::BoardEdge, _) => edge,
                    (a, b) if a.is_hole() && b.is_hole() => hole_to_hole,
                    (a, b) if a.is_hole() || b.is_hole() => hole_to_copper,
                    _ => copper,
                };
                pairs.push(PairClearance { a: *a, b: *b, mm });
            }
        }
        Self { pairs }
    }

    fn ordered(a: CopperObject, b: CopperObject) -> (CopperObject, CopperObject) {
        if a <= b { (a, b) } else { (b, a) }
    }

    /// The clearance of an unordered pair (0 if the matrix lacks it, which
    /// [`DesignRules::validate`] refuses).
    pub fn get(&self, a: CopperObject, b: CopperObject) -> f64 {
        let (a, b) = Self::ordered(a, b);
        self.pairs
            .iter()
            .find(|p| p.a == a && p.b == b)
            .map_or(0.0, |p| p.mm)
    }

    pub fn set(&mut self, a: CopperObject, b: CopperObject, mm: f64) {
        let (a, b) = Self::ordered(a, b);
        match self.pairs.iter_mut().find(|p| p.a == a && p.b == b) {
            Some(p) => p.mm = mm,
            None => self.pairs.push(PairClearance { a, b, mm }),
        }
    }

    fn complete(&self) -> bool {
        self.pairs.len() == 28
            && CopperObject::ALL.iter().enumerate().all(|(i, a)| {
                CopperObject::ALL[i..]
                    .iter()
                    .all(|b| self.pairs.iter().filter(|p| p.a == *a && p.b == *b).count() == 1)
            })
    }
}

/// How a fill joins a pad or via of its own net.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PadConnection {
    /// Fully flooded.
    Solid,
    /// Spokes across a relief gap ([`ThermalRelief`]).
    #[default]
    Thermal,
    /// Not connected: cleared like another net.
    None,
}

/// Thermal relief geometry.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThermalRelief {
    /// Spoke width, mm.
    pub spoke_width: f64,
    /// Spokes per pad, 1–8.
    pub spoke_count: u32,
    /// Relief gap between pad and fill, mm.
    pub gap: f64,
    /// First spoke's angle in the pad frame, degrees.
    pub angle_deg: f64,
}

/// Copper routing limits.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CopperRules {
    pub track_width_min: f64,
    /// What the Route tool starts with when no net class says otherwise.
    pub track_width_default: f64,
    pub clearance: ClearanceMatrix,
}

/// Drill, via and annular-ring limits. Hole-to-copper and hole-to-hole
/// gaps live in [`CopperRules::clearance`] (`via_hole` and `pth_hole`
/// pairs).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HoleRules {
    pub drill_min: f64,
    pub via_annular_ring_min: f64,
    pub pth_annular_ring_min: f64,
    /// Board thickness over drill diameter; `None` is not checked.
    pub aspect_ratio_max: Option<f64>,
    pub via_drill_default: f64,
    pub via_diameter_default: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaskRules {
    /// Pad mask opening growth on every side, mm.
    pub expansion: f64,
    pub web_min: f64,
    /// Whether a new via is tented.
    pub tent_vias: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SilkscreenRules {
    pub line_width_min: f64,
    pub text_height_min: f64,
    /// Least gap from silkscreen to an exposed (mask-open) pad, mm.
    pub pad_clearance: f64,
    /// Least gap from silkscreen to the board edge and cutouts, mm (0: not
    /// checked).
    pub edge_clearance: f64,
    /// Legend text stroke width as a share of its height; the stroke is
    /// never thinner than `line_width_min`.
    pub text_stroke_share: f64,
}

impl SilkscreenRules {
    /// The pen width legend text of `height` mm prints with.
    pub fn text_stroke(&self, height: f64) -> f64 {
        (self.text_stroke_share * height).max(self.line_width_min)
    }
}

/// Differential-pair defaults a pair's class may override.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PairRules {
    /// How far a coupled stretch's edge gap may stray from the pair gap
    /// and still count as coupled, as a share of the gap…
    pub gap_tolerance_share: f64,
    /// …and at least this, mm.
    pub gap_tolerance: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PasteRules {
    /// Paste opening shrink on every side, mm.
    pub reduction: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CourtyardRules {
    /// Least gap between two placed courtyards, mm (0: may abut).
    pub clearance: f64,
}

/// Defaults every copper fill starts from (a fill may override each).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FillRules {
    /// Extra gap from fill copper to other-net copper, taken as the larger
    /// with the `fill` pairs of the clearance matrix.
    pub clearance: f64,
    /// Narrowest fill neck kept, mm (thinner copper is eroded away).
    pub width_min: f64,
    pub thermal: ThermalRelief,
    pub connection: PadConnection,
    /// Islands smaller than this many mm² are removed (0: keep all).
    pub island_area_min: f64,
}

/// Etch-hazard checks (`board.acute_copper`, `board.copper_sliver`). No
/// published standard or fabricator note gives a citable number for either,
/// so both start unset: 0 means not checked. Enter the fabricator's value to
/// check them.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EtchRules {
    /// Least angle, degrees, of a wedge between joined copper edges on one
    /// layer (track to track, track to pad, track to fill); 0: not checked.
    #[serde(default)]
    pub copper_angle_min: f64,
    /// Least width, mm, of a bare gap between copper of other nets (or
    /// copper and the board edge) inside a fill that did not flow into it;
    /// 0: not checked.
    #[serde(default)]
    pub sliver_width_min: f64,
}

impl EtchRules {
    /// Neither check is set (the default, left out of saved rules).
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }
}

/// Land-pattern estimator margins (original rules of thumb, not a
/// land-pattern standard), read by `library::samples` and the footprint
/// templates.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LandRules {
    /// Gull-wing land width over lead width, mm.
    pub gull_wing_width_extra: f64,
    /// Least gap between the two lands of a chip, mm.
    pub chip_gap_min: f64,
    /// Exposed pad to signal land gap, mm.
    pub exposed_pad_clearance: f64,
    /// Outward shift of a gull-wing or molded land's centre from its
    /// foot's centre (toe-heavy lands), mm.
    pub gull_wing_toe_shift: f64,
    /// Through-hole drill over the lead's largest cross-section, mm.
    pub tht_drill_clearance: f64,
    /// Through-hole land diameter over its drill (both sides), mm.
    pub tht_land_extra: f64,
    /// Copper kept between neighbouring through-hole lands, mm.
    pub tht_land_gap: f64,
}

/// One complete set of design rules.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignRules {
    pub copper: CopperRules,
    pub holes: HoleRules,
    pub mask: MaskRules,
    pub silkscreen: SilkscreenRules,
    pub paste: PasteRules,
    pub courtyard: CourtyardRules,
    pub fill: FillRules,
    pub land: LandRules,
    pub pair: PairRules,
    #[serde(default, skip_serializing_if = "EtchRules::is_unset")]
    pub etch: EtchRules,
    #[serde(default, skip_serializing_if = "BoardRules::is_default")]
    pub board: BoardRules,
}

/// The board shape's rules.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoardRules {
    /// How far, mm, the lines and arcs that stand for a free-form board
    /// curve (spline, ellipse, conic) may stray from it: what fills,
    /// checks, fabrication output and the 3D laminate read.
    pub outline_fit: f64,
}

impl Default for BoardRules {
    fn default() -> Self {
        Self {
            outline_fit: DEFAULT_OUTLINE_FIT_MM,
        }
    }
}

impl BoardRules {
    /// The default (left out of saved rules).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl DesignRules {
    /// No rules: every limit 0 (not checked), every default 0 and no via
    /// tenting. A project without a profile has these; nothing that needs
    /// a default may use them ([`ProjectRules::require`]).
    pub fn unset() -> Self {
        Self {
            copper: CopperRules {
                track_width_min: 0.0,
                track_width_default: 0.0,
                clearance: ClearanceMatrix::uniform(0.0, 0.0, 0.0, 0.0),
            },
            holes: HoleRules {
                drill_min: 0.0,
                via_annular_ring_min: 0.0,
                pth_annular_ring_min: 0.0,
                aspect_ratio_max: None,
                via_drill_default: 0.0,
                via_diameter_default: 0.0,
            },
            mask: MaskRules {
                expansion: 0.0,
                web_min: 0.0,
                tent_vias: false,
            },
            silkscreen: SilkscreenRules {
                line_width_min: 0.0,
                text_height_min: 0.0,
                pad_clearance: 0.0,
                edge_clearance: 0.0,
                text_stroke_share: 0.0,
            },
            paste: PasteRules { reduction: 0.0 },
            courtyard: CourtyardRules { clearance: 0.0 },
            fill: FillRules {
                clearance: 0.0,
                width_min: 0.0,
                thermal: ThermalRelief {
                    spoke_width: 0.0,
                    spoke_count: 0,
                    gap: 0.0,
                    angle_deg: 0.0,
                },
                connection: PadConnection::Thermal,
                island_area_min: 0.0,
            },
            land: LandRules {
                gull_wing_width_extra: 0.0,
                chip_gap_min: 0.0,
                exposed_pad_clearance: 0.0,
                gull_wing_toe_shift: 0.0,
                tht_drill_clearance: 0.0,
                tht_land_extra: 0.0,
                tht_land_gap: 0.0,
            },
            pair: PairRules {
                gap_tolerance_share: 0.0,
                gap_tolerance: 0.0,
            },
            etch: EtchRules::default(),
            board: BoardRules { outline_fit: 0.0 },
        }
    }
}

impl Default for DesignRules {
    /// TODO(profile-files): only the library's land estimators
    /// (`library::samples`, `package_form`, `templates`) still read these,
    /// for parts made outside a project; they move to the board's profile
    /// once units U2 lands. No project, profile or check reads them.
    ///
    /// The prototype values (estimated, not a fabricator qualification),
    /// which equal the limits from before rules were configurable except
    /// where a change was chosen: 0.2 mm copper clearance, copper at least
    /// 0.3 mm from the board edge (it was unchecked, which let fill copper
    /// reach 5 µm from the edge and still pass the production check, so a
    /// new project could not be fabricated as shipped), 0.25 mm drill to
    /// copper and hole to hole, 0.1 mm annular
    /// ring and drill, 0.05 mm mask expansion, 0.1 mm mask web and silk
    /// width, 0.6 mm silk text, 0.3/0.6 mm vias, tented, and the land
    /// estimators' margins (`library::samples`, `package_form::tht`).
    fn default() -> Self {
        Self {
            copper: CopperRules {
                track_width_min: 0.05,
                track_width_default: 0.3,
                clearance: ClearanceMatrix::uniform(0.2, 0.25, 0.25, 0.3),
            },
            holes: HoleRules {
                drill_min: 0.1,
                via_annular_ring_min: 0.1,
                pth_annular_ring_min: 0.1,
                aspect_ratio_max: None,
                via_drill_default: 0.3,
                via_diameter_default: 0.6,
            },
            mask: MaskRules {
                expansion: 0.05,
                web_min: 0.1,
                tent_vias: true,
            },
            silkscreen: SilkscreenRules {
                line_width_min: 0.1,
                text_height_min: 0.6,
                pad_clearance: 0.0,
                edge_clearance: 0.0,
                text_stroke_share: 1.0 / 7.0,
            },
            paste: PasteRules { reduction: 0.0 },
            courtyard: CourtyardRules { clearance: 0.0 },
            fill: FillRules {
                clearance: 0.2,
                width_min: 0.2,
                thermal: ThermalRelief {
                    spoke_width: 0.3,
                    spoke_count: 4,
                    gap: 0.25,
                    angle_deg: 45.0,
                },
                connection: PadConnection::Thermal,
                island_area_min: 0.0,
            },
            land: LandRules {
                gull_wing_width_extra: 0.2,
                chip_gap_min: 0.2,
                exposed_pad_clearance: 0.2,
                gull_wing_toe_shift: 0.1,
                tht_drill_clearance: 0.3,
                tht_land_extra: 0.6,
                tht_land_gap: 0.25,
            },
            pair: PairRules {
                gap_tolerance_share: 0.1,
                gap_tolerance: 0.02,
            },
            etch: EtchRules::default(),
            board: BoardRules::default(),
        }
    }
}

/// The table groups of the Design rules panel.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum RuleGroup {
    Copper,
    HolesAndVias,
    Mask,
    Silkscreen,
    Paste,
    Fills,
    BoardEdge,
    Placement,
    Lands,
}

/// One scalar rule, the unit of overrides and of issue texts. In JSON a
/// key is `{"rule":"drill_min"}`, or for a clearance pair
/// `{"rule":"clearance","a":"track","b":"via"}` (either order). Keys are
/// serialized by name: new keys are appended, none is renamed.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case", tag = "rule")]
pub enum RuleKey {
    TrackWidthMin,
    TrackWidthDefault,
    Clearance { a: CopperObject, b: CopperObject },
    DrillMin,
    ViaAnnularRingMin,
    PthAnnularRingMin,
    AspectRatioMax,
    ViaDrillDefault,
    ViaDiameterDefault,
    MaskExpansion,
    MaskWebMin,
    TentVias,
    SilkLineWidthMin,
    SilkTextHeightMin,
    SilkPadClearance,
    PasteReduction,
    CourtyardClearance,
    FillClearance,
    FillWidthMin,
    ThermalSpokeWidth,
    ThermalSpokeCount,
    ThermalGap,
    ThermalAngle,
    FillConnection,
    IslandAreaMin,
    GullWingWidthExtra,
    ChipGapMin,
    ExposedPadClearance,
    // Appended keys (serialized by name; never rename or reuse one).
    SilkEdgeClearance,
    GullWingToeShift,
    ThtDrillClearance,
    ThtLandExtra,
    ThtLandGap,
    CopperAngleMin,
    SliverWidthMin,
    BoardOutlineFit,
    SilkTextStrokeShare,
    PairGapTolerance,
    PairGapToleranceShare,
}

/// A rule's value.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum RuleValue {
    /// Millimetres.
    Length(f64),
    /// Square millimetres.
    Area(f64),
    /// Degrees.
    Angle(f64),
    /// A share, 0–1.
    Fraction(f64),
    /// A dimensionless ratio; `None` is not checked.
    Ratio(Option<f64>),
    Count(u32),
    Flag(bool),
    Connection(PadConnection),
}

impl RuleKey {
    /// Every key, in panel order.
    pub fn all() -> Vec<Self> {
        let mut keys = vec![Self::TrackWidthMin, Self::TrackWidthDefault];
        for (i, a) in CopperObject::ALL.iter().enumerate() {
            for b in &CopperObject::ALL[i..] {
                keys.push(Self::Clearance { a: *a, b: *b });
            }
        }
        keys.extend([
            Self::DrillMin,
            Self::ViaAnnularRingMin,
            Self::PthAnnularRingMin,
            Self::AspectRatioMax,
            Self::ViaDrillDefault,
            Self::ViaDiameterDefault,
            Self::MaskExpansion,
            Self::MaskWebMin,
            Self::TentVias,
            Self::SilkLineWidthMin,
            Self::SilkTextHeightMin,
            Self::SilkPadClearance,
            Self::SilkEdgeClearance,
            Self::PasteReduction,
            Self::CourtyardClearance,
            Self::FillClearance,
            Self::FillWidthMin,
            Self::ThermalSpokeWidth,
            Self::ThermalSpokeCount,
            Self::ThermalGap,
            Self::ThermalAngle,
            Self::FillConnection,
            Self::IslandAreaMin,
            Self::GullWingWidthExtra,
            Self::ChipGapMin,
            Self::ExposedPadClearance,
            Self::GullWingToeShift,
            Self::ThtDrillClearance,
            Self::ThtLandExtra,
            Self::ThtLandGap,
            Self::CopperAngleMin,
            Self::SliverWidthMin,
            Self::BoardOutlineFit,
            Self::SilkTextStrokeShare,
            Self::PairGapTolerance,
            Self::PairGapToleranceShare,
        ]);
        keys
    }

    /// The key's name in profile files and formulas: snake case, a
    /// clearance pair `clearance_<a>_<b>` (`clearance_track_board_edge`).
    pub fn name(self) -> String {
        match self.normalized() {
            Self::Clearance { a, b } => {
                format!("clearance_{}_{}", object_name(a), object_name(b))
            }
            other => serde_json::to_value(other)
                .ok()
                .and_then(|v| v.get("rule").and_then(|r| r.as_str()).map(str::to_owned))
                .unwrap_or_default(),
        }
    }

    /// The key [`Self::name`] gives `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|k| k.name() == name)
    }

    /// The kind of value this key takes (its value in [`DesignRules::unset`]).
    pub fn kind(self) -> RuleValue {
        self.get(&DesignRules::unset())
    }

    /// The key with an unordered clearance pair stored `a <= b`.
    pub fn normalized(self) -> Self {
        match self {
            Self::Clearance { a, b } if b < a => Self::Clearance { a: b, b: a },
            other => other,
        }
    }

    pub fn group(self) -> RuleGroup {
        match self {
            Self::Clearance { a, b }
                if a == CopperObject::BoardEdge || b == CopperObject::BoardEdge =>
            {
                RuleGroup::BoardEdge
            }
            Self::Clearance { a, b } if a.is_hole() || b.is_hole() => {
                RuleGroup::HolesAndVias
            }
            Self::BoardOutlineFit => RuleGroup::BoardEdge,
            Self::TrackWidthMin
            | Self::TrackWidthDefault
            | Self::Clearance { .. }
            | Self::CopperAngleMin => RuleGroup::Copper,
            Self::DrillMin
            | Self::ViaAnnularRingMin
            | Self::PthAnnularRingMin
            | Self::AspectRatioMax
            | Self::ViaDrillDefault
            | Self::ViaDiameterDefault => RuleGroup::HolesAndVias,
            Self::MaskExpansion | Self::MaskWebMin | Self::TentVias => RuleGroup::Mask,
            Self::SilkLineWidthMin
            | Self::SilkTextHeightMin
            | Self::SilkPadClearance
            | Self::SilkEdgeClearance
            | Self::SilkTextStrokeShare => RuleGroup::Silkscreen,
            Self::PairGapTolerance | Self::PairGapToleranceShare => RuleGroup::Copper,
            Self::PasteReduction => RuleGroup::Paste,
            Self::CourtyardClearance => RuleGroup::Placement,
            Self::FillClearance
            | Self::FillWidthMin
            | Self::ThermalSpokeWidth
            | Self::ThermalSpokeCount
            | Self::ThermalGap
            | Self::ThermalAngle
            | Self::FillConnection
            | Self::IslandAreaMin
            | Self::SliverWidthMin => RuleGroup::Fills,
            Self::GullWingWidthExtra
            | Self::ChipGapMin
            | Self::ExposedPadClearance
            | Self::GullWingToeShift
            | Self::ThtDrillClearance
            | Self::ThtLandExtra
            | Self::ThtLandGap => RuleGroup::Lands,
        }
    }

    /// The rule's name in issue texts and the panel ("min mask web").
    pub fn label(self) -> String {
        match self {
            Self::Clearance { a, b } => format!("{} to {} clearance", a.label(), b.label()),
            other => match other {
                Self::TrackWidthMin => "min track width",
                Self::TrackWidthDefault => "default track width",
                Self::DrillMin => "min drill",
                Self::ViaAnnularRingMin => "min via annular ring",
                Self::PthAnnularRingMin => "min PTH annular ring",
                Self::AspectRatioMax => "max aspect ratio",
                Self::ViaDrillDefault => "default via drill",
                Self::ViaDiameterDefault => "default via diameter",
                Self::MaskExpansion => "mask expansion",
                Self::MaskWebMin => "min mask web",
                Self::TentVias => "tent vias",
                Self::SilkLineWidthMin => "min silkscreen width",
                Self::SilkTextHeightMin => "min silkscreen text height",
                Self::SilkPadClearance => "silkscreen to pad clearance",
                Self::PasteReduction => "paste reduction",
                Self::CourtyardClearance => "courtyard clearance",
                Self::FillClearance => "fill clearance",
                Self::FillWidthMin => "min fill width",
                Self::ThermalSpokeWidth => "thermal spoke width",
                Self::ThermalSpokeCount => "thermal spoke count",
                Self::ThermalGap => "thermal gap",
                Self::ThermalAngle => "thermal spoke angle",
                Self::FillConnection => "fill pad connection",
                Self::IslandAreaMin => "min island area",
                Self::GullWingWidthExtra => "gull-wing land extra width",
                Self::ChipGapMin => "min chip land gap",
                Self::ExposedPadClearance => "exposed pad clearance",
                Self::SilkEdgeClearance => "silkscreen to board edge clearance",
                Self::GullWingToeShift => "gull-wing land toe shift",
                Self::ThtDrillClearance => "THT drill over lead",
                Self::ThtLandExtra => "THT land over drill",
                Self::ThtLandGap => "min THT land gap",
                Self::CopperAngleMin => "min copper angle",
                Self::SliverWidthMin => "min sliver width",
                Self::BoardOutlineFit => "board outline fit",
                Self::SilkTextStrokeShare => "silkscreen text stroke share",
                Self::PairGapTolerance => "min pair gap tolerance",
                Self::PairGapToleranceShare => "pair gap tolerance share",
                Self::Clearance { .. } => "clearance",
            }
            .to_string(),
        }
    }

    /// This key's value in `rules`.
    pub fn get(self, rules: &DesignRules) -> RuleValue {
        use RuleValue::*;
        match self {
            Self::TrackWidthMin => Length(rules.copper.track_width_min),
            Self::TrackWidthDefault => Length(rules.copper.track_width_default),
            Self::Clearance { a, b } => Length(rules.copper.clearance.get(a, b)),
            Self::DrillMin => Length(rules.holes.drill_min),
            Self::ViaAnnularRingMin => Length(rules.holes.via_annular_ring_min),
            Self::PthAnnularRingMin => Length(rules.holes.pth_annular_ring_min),
            Self::AspectRatioMax => Ratio(rules.holes.aspect_ratio_max),
            Self::ViaDrillDefault => Length(rules.holes.via_drill_default),
            Self::ViaDiameterDefault => Length(rules.holes.via_diameter_default),
            Self::MaskExpansion => Length(rules.mask.expansion),
            Self::MaskWebMin => Length(rules.mask.web_min),
            Self::TentVias => Flag(rules.mask.tent_vias),
            Self::SilkLineWidthMin => Length(rules.silkscreen.line_width_min),
            Self::SilkTextHeightMin => Length(rules.silkscreen.text_height_min),
            Self::SilkPadClearance => Length(rules.silkscreen.pad_clearance),
            Self::PasteReduction => Length(rules.paste.reduction),
            Self::CourtyardClearance => Length(rules.courtyard.clearance),
            Self::FillClearance => Length(rules.fill.clearance),
            Self::FillWidthMin => Length(rules.fill.width_min),
            Self::ThermalSpokeWidth => Length(rules.fill.thermal.spoke_width),
            Self::ThermalSpokeCount => Count(rules.fill.thermal.spoke_count),
            Self::ThermalGap => Length(rules.fill.thermal.gap),
            Self::ThermalAngle => Angle(rules.fill.thermal.angle_deg),
            Self::FillConnection => Connection(rules.fill.connection),
            Self::IslandAreaMin => Area(rules.fill.island_area_min),
            Self::GullWingWidthExtra => Length(rules.land.gull_wing_width_extra),
            Self::ChipGapMin => Length(rules.land.chip_gap_min),
            Self::ExposedPadClearance => Length(rules.land.exposed_pad_clearance),
            Self::SilkEdgeClearance => Length(rules.silkscreen.edge_clearance),
            Self::GullWingToeShift => Length(rules.land.gull_wing_toe_shift),
            Self::ThtDrillClearance => Length(rules.land.tht_drill_clearance),
            Self::ThtLandExtra => Length(rules.land.tht_land_extra),
            Self::ThtLandGap => Length(rules.land.tht_land_gap),
            Self::CopperAngleMin => Angle(rules.etch.copper_angle_min),
            Self::SliverWidthMin => Length(rules.etch.sliver_width_min),
            Self::BoardOutlineFit => Length(rules.board.outline_fit),
            Self::SilkTextStrokeShare => Fraction(rules.silkscreen.text_stroke_share),
            Self::PairGapTolerance => Length(rules.pair.gap_tolerance),
            Self::PairGapToleranceShare => Fraction(rules.pair.gap_tolerance_share),
        }
    }

    /// Set this key in `rules`; a value of the wrong kind is refused.
    pub fn set(self, rules: &mut DesignRules, value: RuleValue) -> Result<()> {
        use RuleValue::*;
        let wrong = || {
            Err(Error::Invalid(format!(
                "rules.value_kind: {} takes a different kind of value",
                self.label()
            )))
        };
        let length = |v: RuleValue| match v {
            Length(mm) => Some(mm),
            _ => None,
        };
        let slot: &mut f64 = match self {
            Self::TrackWidthMin => &mut rules.copper.track_width_min,
            Self::TrackWidthDefault => &mut rules.copper.track_width_default,
            Self::Clearance { a, b } => {
                let Some(mm) = length(value) else {
                    return wrong();
                };
                rules.copper.clearance.set(a, b, mm);
                return Ok(());
            }
            Self::DrillMin => &mut rules.holes.drill_min,
            Self::ViaAnnularRingMin => &mut rules.holes.via_annular_ring_min,
            Self::PthAnnularRingMin => &mut rules.holes.pth_annular_ring_min,
            Self::AspectRatioMax => {
                let Ratio(r) = value else { return wrong() };
                rules.holes.aspect_ratio_max = r;
                return Ok(());
            }
            Self::ViaDrillDefault => &mut rules.holes.via_drill_default,
            Self::ViaDiameterDefault => &mut rules.holes.via_diameter_default,
            Self::MaskExpansion => &mut rules.mask.expansion,
            Self::MaskWebMin => &mut rules.mask.web_min,
            Self::TentVias => {
                let Flag(f) = value else { return wrong() };
                rules.mask.tent_vias = f;
                return Ok(());
            }
            Self::SilkLineWidthMin => &mut rules.silkscreen.line_width_min,
            Self::SilkTextHeightMin => &mut rules.silkscreen.text_height_min,
            Self::SilkPadClearance => &mut rules.silkscreen.pad_clearance,
            Self::PasteReduction => &mut rules.paste.reduction,
            Self::CourtyardClearance => &mut rules.courtyard.clearance,
            Self::FillClearance => &mut rules.fill.clearance,
            Self::FillWidthMin => &mut rules.fill.width_min,
            Self::ThermalSpokeWidth => &mut rules.fill.thermal.spoke_width,
            Self::ThermalSpokeCount => {
                let Count(n) = value else { return wrong() };
                rules.fill.thermal.spoke_count = n;
                return Ok(());
            }
            Self::ThermalGap => &mut rules.fill.thermal.gap,
            Self::ThermalAngle => {
                let Angle(deg) = value else { return wrong() };
                rules.fill.thermal.angle_deg = deg;
                return Ok(());
            }
            Self::FillConnection => {
                let Connection(c) = value else { return wrong() };
                rules.fill.connection = c;
                return Ok(());
            }
            Self::IslandAreaMin => {
                let Area(a) = value else { return wrong() };
                rules.fill.island_area_min = a;
                return Ok(());
            }
            Self::GullWingWidthExtra => &mut rules.land.gull_wing_width_extra,
            Self::ChipGapMin => &mut rules.land.chip_gap_min,
            Self::ExposedPadClearance => &mut rules.land.exposed_pad_clearance,
            Self::SilkEdgeClearance => &mut rules.silkscreen.edge_clearance,
            Self::GullWingToeShift => &mut rules.land.gull_wing_toe_shift,
            Self::ThtDrillClearance => &mut rules.land.tht_drill_clearance,
            Self::ThtLandExtra => &mut rules.land.tht_land_extra,
            Self::ThtLandGap => &mut rules.land.tht_land_gap,
            Self::CopperAngleMin => {
                let Angle(deg) = value else { return wrong() };
                rules.etch.copper_angle_min = deg;
                return Ok(());
            }
            Self::SliverWidthMin => &mut rules.etch.sliver_width_min,
            Self::BoardOutlineFit => &mut rules.board.outline_fit,
            Self::SilkTextStrokeShare => {
                let Fraction(f) = value else { return wrong() };
                rules.silkscreen.text_stroke_share = f;
                return Ok(());
            }
            Self::PairGapTolerance => &mut rules.pair.gap_tolerance,
            Self::PairGapToleranceShare => {
                let Fraction(f) = value else { return wrong() };
                rules.pair.gap_tolerance_share = f;
                return Ok(());
            }
        };
        let Some(mm) = length(value) else {
            return wrong();
        };
        *slot = mm;
        Ok(())
    }
}

fn length_ok(mm: f64) -> bool {
    mm.is_finite() && (0.0..=MAX_RULE_MM).contains(&mm)
}

impl DesignRules {
    /// Bounds of every value and the cross-field relations a fabricator
    /// could not build otherwise. Lengths 0–25 mm; areas 0–10⁶ mm²;
    /// angles ±360° (the copper angle 0–180°); track, fill, spoke and silkscreen widths, silkscreen
    /// text height, drills and the default via positive; the default track
    /// width at least the minimum; the default via drill at least the
    /// minimum drill and its diameter covering the drill plus two minimum
    /// rings; spokes 1–8; aspect ratio 1–50 or unchecked; mask expansion
    /// −1 mm to 25 mm (negative shrinks the opening); the board outline fit
    /// 0.001–1 mm ([`OUTLINE_FIT_RANGE_MM`]). The error names the
    /// rule (`rules.bounds: min drill is out of range`).
    pub fn validate(&self) -> Result<()> {
        let bad = |what: &str| Err(Error::Invalid(format!("rules.bounds: {what}")));
        for key in RuleKey::all() {
            let ok = match key.get(self) {
                RuleValue::Length(mm) if key == RuleKey::MaskExpansion => {
                    mm.is_finite() && (-1.0..=MAX_RULE_MM).contains(&mm)
                }
                RuleValue::Length(mm) if key == RuleKey::BoardOutlineFit => {
                    OUTLINE_FIT_RANGE_MM.contains(&mm)
                }
                RuleValue::Length(mm) => length_ok(mm),
                RuleValue::Area(a) => a.is_finite() && (0.0..=1.0e6).contains(&a),
                RuleValue::Fraction(f) => f.is_finite() && (0.0..=1.0).contains(&f),
                RuleValue::Angle(d) if key == RuleKey::CopperAngleMin => {
                    d.is_finite() && (0.0..=180.0).contains(&d)
                }
                RuleValue::Angle(d) => d.is_finite() && (-360.0..=360.0).contains(&d),
                RuleValue::Ratio(r) => r.is_none_or(|r| r.is_finite() && (1.0..=50.0).contains(&r)),
                RuleValue::Count(n) => key != RuleKey::ThermalSpokeCount || (1..=8).contains(&n),
                RuleValue::Flag(_) | RuleValue::Connection(_) => true,
            };
            if !ok {
                return bad(&format!("{} is out of range", key.label()));
            }
        }
        if !self.copper.clearance.complete() {
            return bad("the clearance matrix must list every object pair once");
        }
        let c = &self.copper;
        let h = &self.holes;
        let positive = [
            (RuleKey::TrackWidthMin, c.track_width_min),
            (RuleKey::DrillMin, h.drill_min),
            (RuleKey::ViaDrillDefault, h.via_drill_default),
            (RuleKey::FillWidthMin, self.fill.width_min),
            (RuleKey::ThermalSpokeWidth, self.fill.thermal.spoke_width),
            (RuleKey::SilkLineWidthMin, self.silkscreen.line_width_min),
            (RuleKey::SilkTextHeightMin, self.silkscreen.text_height_min),
        ];
        if let Some((key, _)) = positive.iter().find(|(_, v)| *v <= 0.0) {
            return bad(&format!("{} must be positive", key.label()));
        }
        if c.track_width_default < c.track_width_min {
            return bad("the default track width is below the minimum");
        }
        if h.via_drill_default < h.drill_min {
            return bad("the default via drill is below the minimum drill");
        }
        if h.via_diameter_default + 1e-9 < h.via_drill_default + 2.0 * h.via_annular_ring_min {
            return bad("the default via diameter leaves less than the minimum annular ring");
        }
        Ok(())
    }
}

/// A project's departure from its profile for one rule.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleOverride {
    pub key: RuleKey,
    pub value: RuleValue,
}

/// Which step of the resolution order a value came from.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "source")]
pub enum RuleSource {
    /// The project's profile snapshot (none: the project has no profile).
    Profile { name: String },
    /// A project override.
    Override,
    /// A net class's own value.
    NetClass { class: String },
    /// The class-to-class clearance matrix.
    ClassPair { a: String, b: String },
    /// Reserved for rule regions (a board area with its own values); no
    /// resolution step produces it yet.
    Region { name: String },
}

impl RuleSource {
    /// The parenthesised source in issue texts: `Fine 4/4`, `override`,
    /// `net class Power`.
    pub fn describe(&self) -> String {
        match self {
            Self::Profile { name } => name.clone(),
            Self::Override => "project override".into(),
            Self::NetClass { class } => format!("net class {class}"),
            Self::ClassPair { a, b } => format!("net classes {a} / {b}"),
            Self::Region { name } => format!("region {name}"),
        }
    }

    /// The step of the resolution order, lowest first: profile 0,
    /// override 1, net class 2, class pair 3, region 4. A later step's
    /// value replaces an earlier one for widths and sizes; clearances take
    /// the largest value and report the step it came from.
    pub fn precedence(&self) -> u8 {
        match self {
            Self::Profile { .. } => 0,
            Self::Override => 1,
            Self::NetClass { .. } => 2,
            Self::ClassPair { .. } => 3,
            Self::Region { .. } => 4,
        }
    }
}

/// A resolved value with where it came from.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Resolved<T> {
    pub value: T,
    pub source: RuleSource,
}

/// One side of a resolved clearance: the object kind and its net's class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearanceSide {
    pub object: CopperObject,
    pub class: Option<NetClassId>,
}

/// Everything a project says about its rules. Saved with the project.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectRules {
    /// The profile snapshot the project uses (switching replaces it);
    /// none until one is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<RuleProfile>,
    /// At most one per key, in [`RuleKey::all`] order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<RuleOverride>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub net_classes: Vec<NetClass>,
    /// Explicit net-to-class assignments; they win over patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignments: Vec<NetClassAssignment>,
    /// Name-pattern rules, evaluated in order; the first match assigns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<NetClassPattern>,
    /// Class-to-class clearances (unordered pairs, `a <= b`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub class_clearances: Vec<ClassClearance>,
    /// Next [`NetClassId`]; ids are never reused.
    #[serde(default = "netclass::first_id")]
    pub next_class: u32,
    /// Differential pairs (`diff_pair.*`; [`signal`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff_pairs: Vec<signal::DiffPair>,
    /// Length-matching groups (`length_group.*`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub length_groups: Vec<signal::LengthGroup>,
    /// Next [`signal::DiffPairId`]; ids are never reused.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub next_pair: u32,
    /// Next [`signal::LengthGroupId`]; ids are never reused.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub next_group: u32,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl ProjectRules {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The project's profile, or `rules.no_profile` for a tool or output
    /// that needs a default or a limit the profile sets.
    pub fn require(&self) -> Result<&RuleProfile> {
        self.profile.as_ref().ok_or_else(|| {
            Error::Invalid(
                "rules.no_profile: this board has no rule profile; choose one (Board › Rules) \
                 before routing, filling or making production files"
                    .into(),
            )
        })
    }

    /// The effective rules: the profile with every override in place of
    /// its entry and its formulas evaluated after, the one resolution every
    /// check, tool default and output reads. With no profile,
    /// [`DesignRules::unset`].
    pub fn effective(&self) -> DesignRules {
        let Some(profile) = &self.profile else {
            return DesignRules::unset();
        };
        if self.overrides.is_empty() {
            return profile.rules().clone();
        }
        // Overrides are validated on entry, so the resolution succeeds; a
        // failure leaves the profile's own values.
        profile
            .resolve_with(&self.overrides)
            .unwrap_or_else(|_| profile.rules().clone())
    }

    /// The rules the land estimators read: [`Self::effective`] with a
    /// profile, else the library's interim values.
    ///
    /// TODO(profile-files): once units U2 lands, the estimators
    /// (`library::samples`, `package_form`, the footprint templates) take
    /// their margins only from a profile and refuse without one.
    pub fn land_estimators(&self) -> DesignRules {
        if self.profile.is_some() {
            self.effective()
        } else {
            DesignRules::default()
        }
    }

    /// These rules less the etch-hazard checks ([`EtchRules`]), which only
    /// checks read: what fill copper depends on, so a check threshold
    /// leaves fills fresh. Borrowed when no etch rule is set.
    pub fn without_checks(&self) -> std::borrow::Cow<'_, ProjectRules> {
        let etch = |k: RuleKey| matches!(k, RuleKey::CopperAngleMin | RuleKey::SliverWidthMin);
        let Some(profile) = &self.profile else {
            return std::borrow::Cow::Borrowed(self);
        };
        if profile.rules().etch.is_unset() && !self.overrides.iter().any(|o| etch(o.key)) {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut rules = self.clone();
        rules.overrides.retain(|o| !etch(o.key));
        if let Some(p) = &mut rules.profile {
            // Zero (not checked) is in bounds for both, so neither fails;
            // each keeps its source, so the result is the same whatever the
            // checks were set to.
            for (key, zero) in [
                (RuleKey::CopperAngleMin, RuleValue::Angle(0.0)),
                (RuleKey::SliverWidthMin, RuleValue::Length(0.0)),
            ] {
                let source = p.entry(key).map(|e| e.source.clone()).unwrap_or_default();
                let _ = p.set(key, RuleEntry::value(zero, &source));
            }
        }
        std::borrow::Cow::Owned(rules)
    }

    pub fn override_of(&self, key: RuleKey) -> Option<&RuleOverride> {
        let key = key.normalized();
        self.overrides.iter().find(|o| o.key.normalized() == key)
    }

    /// Where `key`'s effective value comes from (profile or override).
    pub fn source_of(&self, key: RuleKey) -> RuleSource {
        if self.override_of(key).is_some() {
            RuleSource::Override
        } else {
            RuleSource::Profile {
                name: self.profile_name(),
            }
        }
    }

    /// The profile's name, or "no rule profile".
    pub fn profile_name(&self) -> String {
        self.profile
            .as_ref()
            .map_or_else(|| "no rule profile".into(), |p| p.name.clone())
    }

    /// One effective rule with its source.
    pub fn resolve(&self, key: RuleKey) -> Resolved<RuleValue> {
        Resolved {
            value: key.get(&self.effective()),
            source: self.source_of(key),
        }
    }

    pub fn class(&self, id: NetClassId) -> Option<&NetClass> {
        self.net_classes.iter().find(|c| c.id == id)
    }

    /// The class of a net: its explicit assignment, else the first
    /// matching name pattern, else none. Never inferred from anything but
    /// these explicit entries.
    pub fn class_of(&self, net: crate::ids::Id, net_name: &str) -> Option<NetClassId> {
        self.assignments
            .iter()
            .find(|a| a.net == net)
            .map(|a| a.class)
            .or_else(|| {
                self.patterns
                    .iter()
                    .find(|p| glob_matches(&p.pattern, net_name))
                    .map(|p| p.class)
            })
    }

    /// The clearance between two objects of different nets: the largest
    /// of the effective object-pair value, each side's class clearance
    /// (copper objects only; its fill clearance, when set, if either
    /// object is a fill) and the class-pair matrix entry.
    pub fn clearance(&self, a: ClearanceSide, b: ClearanceSide) -> Resolved<f64> {
        let key = RuleKey::Clearance {
            a: a.object,
            b: b.object,
        };
        let mut best = Resolved {
            value: self.effective().copper.clearance.get(a.object, b.object),
            source: self.source_of(key),
        };
        let copper = CopperObject::is_copper;
        let fill = a.object == CopperObject::Fill || b.object == CopperObject::Fill;
        for side in [a, b] {
            if !copper(side.object) {
                continue;
            }
            if let Some(class) = side.class.and_then(|c| self.class(c))
                && let Some(mm) = class.clearance_for(fill)
                && mm > best.value
            {
                best = Resolved {
                    value: mm,
                    source: RuleSource::NetClass {
                        class: class.name.clone(),
                    },
                };
            }
        }
        if copper(a.object)
            && copper(b.object)
            && let (Some(ca), Some(cb)) = (a.class, b.class)
            && let Some(pair) = self.class_clearances.iter().find(|p| p.is(ca, cb))
            && pair.mm > best.value
        {
            let name = |c| self.class(c).map_or_else(String::new, |c| c.name.clone());
            best = Resolved {
                value: pair.mm,
                source: RuleSource::ClassPair {
                    a: name(ca),
                    b: name(cb),
                },
            };
        }
        best
    }

    /// The Route tool's width for a net of `class`.
    pub fn track_width(&self, class: Option<NetClassId>) -> Resolved<f64> {
        match class.and_then(|c| self.class(c)) {
            Some(c) if c.track_width.is_some() => Resolved {
                value: c.track_width.unwrap_or_default(),
                source: RuleSource::NetClass {
                    class: c.name.clone(),
                },
            },
            _ => Resolved {
                value: self.effective().copper.track_width_default,
                source: self.source_of(RuleKey::TrackWidthDefault),
            },
        }
    }

    /// The default via (drill, diameter) for a net of `class`.
    pub fn via_size(&self, class: Option<NetClassId>) -> Resolved<(f64, f64)> {
        let rules = self.effective();
        let base = (
            rules.holes.via_drill_default,
            rules.holes.via_diameter_default,
        );
        match class.and_then(|c| self.class(c)) {
            Some(c) if c.via_drill.is_some() || c.via_diameter.is_some() => Resolved {
                value: (
                    c.via_drill.unwrap_or(base.0),
                    c.via_diameter.unwrap_or(base.1),
                ),
                source: RuleSource::NetClass {
                    class: c.name.clone(),
                },
            },
            _ => Resolved {
                value: base,
                source: self.source_of(RuleKey::ViaDrillDefault),
            },
        }
    }

    /// Profile, overrides, effective rules, classes and pairs are within
    /// bounds and consistent (unique keys, existing class references).
    pub fn validate(&self) -> Result<()> {
        let Some(profile) = &self.profile else {
            if !self.overrides.is_empty() {
                return Err(Error::Invalid(
                    "rules.no_profile: overrides need a rule profile to depart from".into(),
                ));
            }
            netclass::validate(self)?;
            return signal::validate(self);
        };
        profile.validate()?;
        if self.overrides.len() > MAX_OVERRIDES {
            return Err(Error::Limit(format!(
                "rules.overrides: at most {MAX_OVERRIDES} overrides"
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for o in &self.overrides {
            if !seen.insert(o.key.normalized()) {
                return Err(Error::Invalid(
                    "rules.duplicate_override: one override per rule".into(),
                ));
            }
            let mut probe = profile.rules().clone();
            o.key.set(&mut probe, o.value)?;
        }
        profile.resolve_with(&self.overrides)?.validate()?;
        netclass::validate(self)?;
        signal::validate(self)
    }
}

/// Every design-rule and net-class mutation. Each is one undo step and
/// leaves the project unchanged on failure; a command that changes
/// nothing (the same profile again, an override to its current value) is
/// no undo step.
///
/// JSON examples (the `params` of `Operation::Rules`):
/// `{"command":"rules.set_profile","params":{"profile":{...},"keep_overrides":true}}`
/// with a profile from `rules.profiles`;
/// `{"command":"rules.set_override","params":{"key":{"rule":"clearance","a":"track","b":"via"},"value":{"kind":"length","value":0.15}}}`;
/// `{"command":"rules.clear_override","params":{"key":{"rule":"mask_web_min"}}}`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum RulesCommand {
    /// Replace the project's profile snapshot (or choose the first). The
    /// profile is a whole profile file's `profile` object with its source.
    /// `keep_overrides` keeps the project overrides, which must still give
    /// valid rules on the new profile (`rules.override_conflict`
    /// otherwise); without it they are dropped. Preview with
    /// `rules.preview_profile`.
    #[serde(rename = "rules.set_profile")]
    SetProfile {
        profile: Box<RuleProfile>,
        #[serde(default)]
        keep_overrides: bool,
    },
    /// Set (or replace) the project override of one rule. The value's kind
    /// must match the rule (`rules.value_kind`) and the effective rules
    /// must stay valid (`rules.bounds`). An override equal to the profile
    /// value is kept: it pins the value across profile switches. An
    /// override of a formula's rule replaces the formula; formulas that
    /// name the rule follow the override.
    #[serde(rename = "rules.set_override")]
    SetOverride { key: RuleKey, value: RuleValue },
    /// Drop the project override of one rule (back to the profile value).
    #[serde(rename = "rules.clear_override")]
    ClearOverride { key: RuleKey },
    /// Add a net class; the outcome names it (`Outcome::NetClassAdded`).
    #[serde(rename = "net_class.add")]
    AddNetClass { class: NetClassSpec },
    /// Replace one class's values (its id stays).
    #[serde(rename = "net_class.edit")]
    EditNetClass { class: NetClass },
    /// Delete a class with its assignments, patterns and pairs.
    #[serde(rename = "net_class.delete")]
    DeleteNetClass { class: NetClassId },
    /// Assign nets to a class explicitly (`None`: remove the explicit
    /// assignment, so patterns apply again).
    #[serde(rename = "net_class.assign")]
    AssignNets {
        nets: Vec<crate::ids::Id>,
        class: Option<NetClassId>,
    },
    /// Replace the ordered name-pattern rules.
    #[serde(rename = "net_class.set_patterns")]
    SetPatterns { patterns: Vec<NetClassPattern> },
    /// Set (`Some`) or remove (`None`) a class-to-class clearance.
    #[serde(rename = "net_class.set_clearance")]
    SetClassClearance {
        a: NetClassId,
        b: NetClassId,
        mm: Option<f64>,
    },
    /// Make two nets a differential pair with an explicit polarity; the
    /// outcome names it (`Outcome::DiffPairAdded`). A net is in at most
    /// one pair. `diff_pair.suggest` proposes pairs from net names.
    #[serde(rename = "diff_pair.add")]
    AddDiffPair { pair: signal::DiffPairSpec },
    /// Replace one pair's name or nets (its id stays).
    #[serde(rename = "diff_pair.edit")]
    EditDiffPair { pair: signal::DiffPair },
    /// Delete a pair; length groups drop it (and a group left empty goes).
    #[serde(rename = "diff_pair.delete")]
    DeleteDiffPair { pair: signal::DiffPairId },
    /// Add a length-matching group; the outcome names it
    /// (`Outcome::LengthGroupAdded`).
    #[serde(rename = "length_group.add")]
    AddLengthGroup { group: signal::LengthGroupSpec },
    /// Replace one group's values (its id stays).
    #[serde(rename = "length_group.edit")]
    EditLengthGroup { group: signal::LengthGroup },
    /// Delete a length group.
    #[serde(rename = "length_group.delete")]
    DeleteLengthGroup { group: signal::LengthGroupId },
}

// A new command chooses its permission class here.
declare_classes!(RulesCommand {
    SetProfile "rules.set_profile" Edit "Set the rule profile",
    SetOverride "rules.set_override" Edit "Override a rule",
    ClearOverride "rules.clear_override" Edit "Clear a rule override",
    AddNetClass "net_class.add" Edit "Add a net class",
    EditNetClass "net_class.edit" Edit "Edit a net class",
    DeleteNetClass "net_class.delete" Edit "Delete a net class",
    AssignNets "net_class.assign" Edit "Assign nets to a class",
    SetPatterns "net_class.set_patterns" Edit "Set net class patterns",
    SetClassClearance "net_class.set_clearance" Edit "Set a class-to-class clearance",
    AddDiffPair "diff_pair.add" Edit "Add a differential pair",
    EditDiffPair "diff_pair.edit" Edit "Edit a differential pair",
    DeleteDiffPair "diff_pair.delete" Edit "Delete a differential pair",
    AddLengthGroup "length_group.add" Edit "Add a length group",
    EditLengthGroup "length_group.edit" Edit "Edit a length group",
    DeleteLengthGroup "length_group.delete" Edit "Delete a length group",
});

/// Apply one rules command. On failure `rules` is unchanged. Net
/// existence of `AssignNets` is the caller's check (the engine's, which
/// sees nets). Returns the class `AddNetClass` created; see
/// [`apply_added`] for what the pair and group commands create.
pub fn apply(rules: &mut ProjectRules, command: RulesCommand) -> Result<Option<NetClassId>> {
    Ok(match apply_added(rules, command)? {
        Some(signal::Added::Class(class)) => Some(class),
        _ => None,
    })
}

/// [`apply`], returning whatever the command created: a class, a pair or
/// a length group. Whether a pair's or group's nets exist is the layout
/// validation's check.
pub fn apply_added(
    rules: &mut ProjectRules,
    command: RulesCommand,
) -> Result<Option<signal::Added>> {
    let mut next = rules.clone();
    let added = apply_to(&mut next, command)?;
    *rules = next;
    Ok(added)
}

fn apply_to(rules: &mut ProjectRules, command: RulesCommand) -> Result<Option<signal::Added>> {
    let mut added = None;
    match command {
        command @ (RulesCommand::AddDiffPair { .. }
        | RulesCommand::EditDiffPair { .. }
        | RulesCommand::DeleteDiffPair { .. }
        | RulesCommand::AddLengthGroup { .. }
        | RulesCommand::EditLengthGroup { .. }
        | RulesCommand::DeleteLengthGroup { .. }) => added = signal::apply(rules, command)?,
        RulesCommand::SetProfile {
            profile,
            keep_overrides,
        } => {
            set_profile(rules, *profile, keep_overrides)?;
            return Ok(None);
        }
        RulesCommand::SetOverride { key, value } => {
            let key = key.normalized();
            rules.overrides.retain(|o| o.key.normalized() != key);
            rules.overrides.push(RuleOverride { key, value });
            let order = RuleKey::all();
            rules
                .overrides
                .sort_by_key(|o| order.iter().position(|k| *k == o.key));
        }
        RulesCommand::ClearOverride { key } => {
            let key = key.normalized();
            let before = rules.overrides.len();
            rules.overrides.retain(|o| o.key.normalized() != key);
            if rules.overrides.len() == before {
                return Err(Error::NotFound(format!(
                    "rules.no_override: {} has no project override",
                    key.label()
                )));
            }
        }
        other => added = netclass::apply(rules, other)?.map(signal::Added::Class),
    }
    rules.validate()?;
    Ok(added)
}

/// `rules.set_profile`: the profile itself must be valid and kept
/// overrides must leave valid effective rules (the error names the
/// overrides to clear first).
fn set_profile(rules: &mut ProjectRules, profile: RuleProfile, keep_overrides: bool) -> Result<()> {
    profile.validate()?;
    let mut next = rules.clone();
    let name = profile.name.clone();
    let checked = profile.resolve_with(if keep_overrides { &next.overrides } else { &[] });
    next.profile = Some(profile);
    if !keep_overrides {
        next.overrides.clear();
    }
    if let Err(error) = checked.and_then(|r| r.validate()) {
        let keys: Vec<String> = next.overrides.iter().map(|o| o.key.label()).collect();
        return Err(Error::Invalid(format!(
            "rules.override_conflict: keeping the overrides ({}) on {name} gives invalid rules: {error}",
            keys.join(", "),
        )));
    }
    next.validate()?;
    *rules = next;
    Ok(())
}

/// One row of the Design rules panel and `rules.effective`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RuleRow {
    pub key: RuleKey,
    pub group: RuleGroup,
    pub label: String,
    /// The profile snapshot's value.
    pub profile: RuleValue,
    /// The profile's formula for this rule, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// Where the profile's value comes from ("" for the profile's source).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value_source: String,
    /// The effective value (override or profile).
    pub value: RuleValue,
    pub source: RuleSource,
}

/// `rules.effective`: the profile, every rule row and the net classes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RulesReport {
    /// The profile's id, name, source and drill-file reading; none when
    /// the project has no profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileSummary>,
    pub effective: DesignRules,
    pub rows: Vec<RuleRow>,
    pub net_classes: Vec<NetClass>,
    pub assignments: Vec<NetClassAssignment>,
    pub patterns: Vec<NetClassPattern>,
    pub class_clearances: Vec<ClassClearance>,
}

/// A profile's identity and source, without its rules.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub source: ProfileSource,
    pub drill_files: DrillFiles,
}

impl ProfileSummary {
    pub fn of(profile: &RuleProfile) -> Self {
        Self {
            id: profile.id.clone(),
            name: profile.name.clone(),
            source: profile.source.clone(),
            drill_files: profile.drill_files.clone(),
        }
    }
}

pub fn report(rules: &ProjectRules) -> RulesReport {
    let effective = rules.effective();
    let unset = DesignRules::unset();
    let rows = RuleKey::all()
        .into_iter()
        .map(|key| {
            let entry = rules.profile.as_ref().and_then(|p| p.entry(key));
            RuleRow {
                key,
                group: key.group(),
                label: key.label(),
                profile: key.get(rules.profile.as_ref().map_or(&unset, |p| p.rules())),
                formula: entry.and_then(|e| match &e.setting {
                    Setting::Formula(f) => Some(f.clone()),
                    Setting::Value(_) => None,
                }),
                value_source: entry.map(|e| e.source.clone()).unwrap_or_default(),
                value: key.get(&effective),
                source: rules.source_of(key),
            }
        })
        .collect();
    RulesReport {
        profile: rules.profile.as_ref().map(ProfileSummary::of),
        effective,
        rows,
        net_classes: rules.net_classes.clone(),
        assignments: rules.assignments.clone(),
        patterns: rules.patterns.clone(),
        class_clearances: rules.class_clearances.clone(),
    }
}

/// One rule whose effective value a profile switch changes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RuleChange {
    pub key: RuleKey,
    pub label: String,
    pub from: RuleValue,
    pub to: RuleValue,
}

/// `rules.preview_profile`: what switching would change, without changing
/// the document. Issue rows are the board issues that would appear or go.
/// A switch `rules.set_profile` would refuse is still previewed: the rule
/// changes are listed, `refusal` holds the reason and the issue lists stay
/// empty.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProfileImpact {
    /// Every rule whose effective value changes, in [`RuleKey::all`] order.
    pub changes: Vec<RuleChange>,
    /// Project overrides the switch drops (`keep_overrides: false`).
    pub dropped_overrides: Vec<RuleKey>,
    /// Titles of issues the switch would add (one per added issue).
    pub new_issues: Vec<String>,
    /// Titles of issues the switch would resolve (one per resolved issue).
    pub resolved_issues: Vec<String>,
    /// Why `rules.set_profile` would refuse this switch; `None` when it
    /// would succeed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

/// The rules a switch would give and why `rules.set_profile` would refuse
/// it (then the candidate is the unvalidated result, for listing changes).
pub fn preview_candidate(
    rules: &ProjectRules,
    profile: &RuleProfile,
    keep_overrides: bool,
) -> (ProjectRules, Option<String>) {
    let mut candidate = rules.clone();
    match set_profile(&mut candidate, profile.clone(), keep_overrides) {
        Ok(()) => (candidate, None),
        Err(error) => {
            candidate.profile = Some(profile.clone());
            if !keep_overrides {
                candidate.overrides.clear();
            }
            (candidate, Some(error.to_string()))
        }
    }
}

/// Fill `impact`'s issue lists from the issues before and after a switch,
/// each given as (identity, title). An issue is matched by identity (its
/// finding code and objects), not its title, so a text that only names a
/// new value is neither added nor resolved; repeated identities count.
pub fn diff_issues(
    impact: &mut ProfileImpact,
    before: impl IntoIterator<Item = (String, String)>,
    after: impl IntoIterator<Item = (String, String)>,
) {
    use std::collections::BTreeMap;
    let mut left: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, title) in before {
        left.entry(id).or_default().push(title);
    }
    for (id, title) in after {
        match left.get_mut(&id).and_then(|titles| titles.pop()) {
            Some(_) => {}
            None => impact.new_issues.push(title),
        }
    }
    impact.resolved_issues.extend(left.into_values().flatten());
}

/// The rule changes of switching `rules` to `candidate` (issue lists are
/// filled by the engine, which can run the checks).
pub fn rule_changes(rules: &ProjectRules, candidate: &ProjectRules) -> ProfileImpact {
    let (before, after) = (rules.effective(), candidate.effective());
    let changes = RuleKey::all()
        .into_iter()
        .filter_map(|key| {
            let (from, to) = (key.get(&before), key.get(&after));
            (from != to).then(|| RuleChange {
                key,
                label: key.label(),
                from,
                to,
            })
        })
        .collect();
    let dropped_overrides = rules
        .overrides
        .iter()
        .filter(|o| candidate.override_of(o.key).is_none())
        .map(|o| o.key)
        .collect();
    ProfileImpact {
        changes,
        dropped_overrides,
        new_issues: Vec::new(),
        resolved_issues: Vec::new(),
        refusal: None,
    }
}

/// A pad connection's name in profile files and formulas.
pub fn connection_name(c: PadConnection) -> &'static str {
    match c {
        PadConnection::Solid => "solid",
        PadConnection::Thermal => "thermal",
        PadConnection::None => "none",
    }
}

/// The pad connection [`connection_name`] gives `name`.
pub fn connection_from_name(name: &str) -> Option<PadConnection> {
    [
        PadConnection::Solid,
        PadConnection::Thermal,
        PadConnection::None,
    ]
    .into_iter()
    .find(|c| connection_name(*c) == name)
}

/// A clearance object's name in rule names.
fn object_name(o: CopperObject) -> &'static str {
    match o {
        CopperObject::Track => "track",
        CopperObject::Pad => "pad",
        CopperObject::Via => "via",
        CopperObject::Fill => "fill",
        CopperObject::BoardEdge => "board_edge",
        CopperObject::ViaHole => "via_hole",
        CopperObject::PthHole => "pth_hole",
    }
}

/// `mm` without trailing zeros (`0.1`, `0.15`).
fn trim(mm: f64) -> String {
    let text = format!("{mm:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixtures;
#[cfg(test)]
mod tests;
