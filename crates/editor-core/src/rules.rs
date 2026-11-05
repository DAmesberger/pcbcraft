//! Design rules: the manufacturing limits every board check, estimator,
//! router and fill reads, grouped into a [`RuleProfile`] (a named,
//! provenance-carrying set of [`DesignRules`]), per-project overrides and
//! net classes ([`netclass`]).
//!
//! The project embeds a snapshot of the profile it uses
//! ([`ProjectRules::profile`]), so a file opens with the same rules on any
//! machine; a user profile library (JSON files in the user config folder,
//! read by `pcbcraft-io`) only supplies profiles to switch to.
//!
//! Resolution order, lowest to highest precedence ([`RuleSource`]):
//! profile value, project override, then (for copper rules that name nets)
//! net class values and the class-to-class clearance matrix. Regions are a
//! reserved later step. Clearances between two objects take the larger of
//! every applicable value; widths take the net class value over the
//! effective rule.
//!
//! Every value here is pure data: no files, no fabricator data. Built-in
//! profiles ([`builtin`]) are generic and labelled estimated.
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod builtin;
pub mod netclass;
pub mod signal;

pub use netclass::{
    ClassClearance, MAX_NET_CLASSES, NetClass, NetClassAssignment, NetClassId, NetClassPattern,
    NetClassSpec, glob_matches,
};

/// Largest length rule, in mm.
pub const MAX_RULE_MM: f64 = 25.0;
/// Most overrides one project holds (one per [`RuleKey`] at most).
pub const MAX_OVERRIDES: usize = 256;
/// Bytes of a profile's name, id or one provenance text.
pub const MAX_PROFILE_TEXT: usize = 256;
/// Bytes of a profile's notes.
pub const MAX_PROFILE_NOTES: usize = 4096;
/// The user profile library file's format; any other version is refused.
pub const PROFILE_FILE_FORMAT: u32 = 1;

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
    /// A drilled or milled hole's wall (any plating).
    Hole,
}

impl CopperObject {
    pub const ALL: [Self; 6] = [
        Self::Track,
        Self::Pad,
        Self::Via,
        Self::Fill,
        Self::BoardEdge,
        Self::Hole,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Pad => "pad",
            Self::Via => "via",
            Self::Fill => "fill",
            Self::BoardEdge => "board edge",
            Self::Hole => "hole",
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
/// entry (21 entries), in [`CopperObject::ALL`] order.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClearanceMatrix {
    pub pairs: Vec<PairClearance>,
}

impl ClearanceMatrix {
    /// Every pair at `copper` mm, with the hole and edge pairs given.
    pub fn uniform(copper: f64, hole_to_copper: f64, hole_to_hole: f64, edge: f64) -> Self {
        let mut pairs = Vec::new();
        for (i, a) in CopperObject::ALL.iter().enumerate() {
            for b in &CopperObject::ALL[i..] {
                let mm = match (a, b) {
                    (_, CopperObject::BoardEdge) | (CopperObject::BoardEdge, _) => edge,
                    (CopperObject::Hole, CopperObject::Hole) => hole_to_hole,
                    (CopperObject::Hole, _) | (_, CopperObject::Hole) => hole_to_copper,
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
        self.pairs.len() == 21
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
/// gaps live in [`CopperRules::clearance`] (`hole` pairs).
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
}

impl Default for DesignRules {
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
        ]);
        keys
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
            Self::Clearance { a, b } if a == CopperObject::Hole || b == CopperObject::Hole => {
                RuleGroup::HolesAndVias
            }
            Self::TrackWidthMin | Self::TrackWidthDefault | Self::Clearance { .. } => {
                RuleGroup::Copper
            }
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
            | Self::SilkEdgeClearance => RuleGroup::Silkscreen,
            Self::PasteReduction => RuleGroup::Paste,
            Self::CourtyardClearance => RuleGroup::Placement,
            Self::FillClearance
            | Self::FillWidthMin
            | Self::ThermalSpokeWidth
            | Self::ThermalSpokeCount
            | Self::ThermalGap
            | Self::ThermalAngle
            | Self::FillConnection
            | Self::IslandAreaMin => RuleGroup::Fills,
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
    /// angles ±360°; track, fill, spoke and silkscreen widths, silkscreen
    /// text height, drills and the default via positive; the default track
    /// width at least the minimum; the default via drill at least the
    /// minimum drill and its diameter covering the drill plus two minimum
    /// rings; spokes 1–8; aspect ratio 1–50 or unchecked; mask expansion
    /// −1 mm to 25 mm (negative shrinks the opening). The error names the
    /// rule (`rules.bounds: min drill is out of range`).
    pub fn validate(&self) -> Result<()> {
        let bad = |what: &str| Err(Error::Invalid(format!("rules.bounds: {what}")));
        for key in RuleKey::all() {
            let ok = match key.get(self) {
                RuleValue::Length(mm) if key == RuleKey::MaskExpansion => {
                    mm.is_finite() && (-1.0..=MAX_RULE_MM).contains(&mm)
                }
                RuleValue::Length(mm) => length_ok(mm),
                RuleValue::Area(a) => a.is_finite() && (0.0..=1.0e6).contains(&a),
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

/// Where a profile came from.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOrigin {
    /// Shipped with Pcbcraft ([`builtin`]).
    #[default]
    BuiltIn,
    /// Made by the user (saved from a project or edited).
    User,
    /// Imported from a profile file.
    Imported,
}

/// What a profile's numbers rest on.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileBasis {
    /// Generic estimates, not any fabricator's qualification.
    #[default]
    Estimated,
    /// Entered by the user from a fabricator's published capabilities.
    FabricatorEntered,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileProvenance {
    pub origin: ProfileOrigin,
    pub basis: ProfileBasis,
    /// The fabricator or document the numbers come from ("" when none).
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub url: String,
    /// ISO date (YYYY-MM-DD) the numbers were taken, or "".
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub notes: String,
}

/// A named, versionable set of design rules.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleProfile {
    /// Stable identity in the profile library (`std-6-6`, a user slug).
    pub id: String,
    /// Shown name ("Standard 6/6").
    pub name: String,
    pub provenance: ProfileProvenance,
    pub rules: DesignRules,
}

impl Default for RuleProfile {
    fn default() -> Self {
        builtin::default_profile()
    }
}

impl RuleProfile {
    /// Text bounds, then [`DesignRules::validate`].
    pub fn validate(&self) -> Result<()> {
        let p = &self.provenance;
        let short = [&self.id, &self.name, &p.source, &p.url, &p.date];
        if self.id.trim().is_empty()
            || self.name.trim().is_empty()
            || short.iter().any(|t| t.len() > MAX_PROFILE_TEXT)
            || p.notes.len() > MAX_PROFILE_NOTES
            || short
                .iter()
                .chain([&&p.notes])
                .any(|t| t.chars().any(|c| c.is_control() && c != '\n'))
        {
            return Err(Error::Invalid(
                "rules.profile_text: a profile needs an id and a name; texts are bounded and printable"
                    .into(),
            ));
        }
        self.rules.validate()
    }
}

/// One profile in the user profile library file (`pcbcraft-io` writes it).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfileFile {
    /// [`PROFILE_FILE_FORMAT`]; any other version is refused.
    pub format: u32,
    pub profile: RuleProfile,
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
    /// The project's profile snapshot.
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
    /// The profile snapshot the project uses (switching replaces it).
    pub profile: RuleProfile,
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

    /// The profile with every override applied.
    pub fn effective(&self) -> DesignRules {
        let mut rules = self.profile.rules.clone();
        for o in &self.overrides {
            // Overrides are validated on entry; a mismatched kind cannot
            // be stored, so a failure here leaves the profile value.
            let _ = o.key.set(&mut rules, o.value);
        }
        rules
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
                name: self.profile.name.clone(),
            }
        }
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
    /// (copper objects only) and the class-pair matrix entry.
    pub fn clearance(&self, a: ClearanceSide, b: ClearanceSide) -> Resolved<f64> {
        let key = RuleKey::Clearance {
            a: a.object,
            b: b.object,
        };
        let mut best = Resolved {
            value: self.effective().copper.clearance.get(a.object, b.object),
            source: self.source_of(key),
        };
        let copper = |o: CopperObject| !matches!(o, CopperObject::BoardEdge | CopperObject::Hole);
        for side in [a, b] {
            if !copper(side.object) {
                continue;
            }
            if let Some(class) = side.class.and_then(|c| self.class(c))
                && let Some(mm) = class.clearance
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
        self.profile.validate()?;
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
            let mut probe = self.profile.rules.clone();
            o.key.set(&mut probe, o.value)?;
        }
        self.effective().validate()?;
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
    /// Replace the project's profile snapshot. `keep_overrides` keeps the
    /// project overrides, which must still give valid rules on the new
    /// profile (`rules.override_conflict` otherwise); without it they are
    /// dropped. A built-in profile must carry its shipped values
    /// (`rules.builtin_modified`); changed values go in a user profile.
    /// Preview with `rules.preview_profile`.
    #[serde(rename = "rules.set_profile")]
    SetProfile {
        profile: Box<RuleProfile>,
        #[serde(default)]
        keep_overrides: bool,
    },
    /// Set (or replace) the project override of one rule. The value's kind
    /// must match the rule (`rules.value_kind`) and the effective rules
    /// must stay valid (`rules.bounds`). An override equal to the profile
    /// value is kept: it pins the value across profile switches.
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
    #[serde(rename = "length_group.delete")]
    DeleteLengthGroup { group: signal::LengthGroupId },
}

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

/// `rules.set_profile`: the profile itself must be valid, a built-in
/// profile must carry its shipped values, and kept overrides must leave
/// valid effective rules (the error names the overrides to clear first).
fn set_profile(rules: &mut ProjectRules, profile: RuleProfile, keep_overrides: bool) -> Result<()> {
    profile.validate()?;
    if profile.provenance.origin == ProfileOrigin::BuiltIn
        && builtin::by_id(&profile.id).is_none_or(|b| b.rules != profile.rules)
    {
        return Err(Error::Invalid(format!(
            "rules.builtin_modified: {} is not a shipped built-in profile; save changed values as a user profile",
            profile.name
        )));
    }
    let mut next = rules.clone();
    next.profile = profile;
    if !keep_overrides {
        next.overrides.clear();
    }
    if let Err(error) = next.effective().validate() {
        let keys: Vec<String> = next.overrides.iter().map(|o| o.key.label()).collect();
        return Err(Error::Invalid(format!(
            "rules.override_conflict: keeping the overrides ({}) on {} gives invalid rules: {error}",
            keys.join(", "),
            next.profile.name
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
    /// The effective value (override or profile).
    pub value: RuleValue,
    pub source: RuleSource,
}

/// `rules.effective`: the profile, every rule row and the net classes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RulesReport {
    pub profile_id: String,
    pub profile_name: String,
    pub provenance: ProfileProvenance,
    pub effective: DesignRules,
    pub rows: Vec<RuleRow>,
    pub net_classes: Vec<NetClass>,
    pub assignments: Vec<NetClassAssignment>,
    pub patterns: Vec<NetClassPattern>,
    pub class_clearances: Vec<ClassClearance>,
}

pub fn report(rules: &ProjectRules) -> RulesReport {
    let effective = rules.effective();
    let rows = RuleKey::all()
        .into_iter()
        .map(|key| RuleRow {
            key,
            group: key.group(),
            label: key.label(),
            profile: key.get(&rules.profile.rules),
            value: key.get(&effective),
            source: rules.source_of(key),
        })
        .collect();
    RulesReport {
        profile_id: rules.profile.id.clone(),
        profile_name: rules.profile.name.clone(),
        provenance: rules.profile.provenance.clone(),
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
            candidate.profile = profile.clone();
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

#[cfg(test)]
mod tests;
