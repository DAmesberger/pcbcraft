//! Copper fill (zone) and keepout data. Computation, caching and checks
//! live in `layout::fill`; this is the saved definition and the saved
//! (derivable) computed copper.
use super::CopperLayer;
use crate::geometry::Point;
use crate::ids::Id;
use crate::library::Outline;
use crate::rules::{PadConnection, ThermalRelief};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Hatched instead of solid copper.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Hatch {
    /// Copper line width, mm.
    pub width: f64,
    /// Opening between lines, mm.
    pub gap: f64,
    pub angle_deg: f64,
}

/// Which copper islands a fill removes.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "remove", content = "area_mm2")]
pub enum IslandRemoval {
    /// Keep every island.
    Never,
    /// Remove islands that touch no pad, via or track of the fill's net.
    #[default]
    Unconnected,
    /// Remove unconnected islands and connected ones below this area.
    BelowArea(f64),
}

/// Pad connection style per pad class; `None` uses the net class, then
/// the effective fill rule.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadConnections {
    #[serde(default)]
    pub smd: Option<PadConnection>,
    #[serde(default)]
    pub through_hole: Option<PadConnection>,
    #[serde(default)]
    pub via: Option<PadConnection>,
}

/// How copper fills join one placed part's pads, over each fill's own
/// settings: a part's (footprint-wide) override applies to its pads, and a
/// pad's own override wins over the part's, field by field. Every field is
/// optional; unset follows the next level (pad, part, fill, net class,
/// rule). Set with `fill.set_pad_override`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadFillOverride {
    /// Solid, thermal spokes or not joined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<PadConnection>,
    /// Thermal spoke width, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spoke_width: Option<f64>,
    /// First spoke's angle in the pad frame, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spoke_angle_deg: Option<f64>,
    /// Thermal relief gap between the pad and the fill, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal_gap: Option<f64>,
    /// Gap from fill copper to the pad where the fill does not join it, mm,
    /// in place of the fill's own clearance; the clearance rules still set
    /// the floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance: Option<f64>,
}

impl PadFillOverride {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `self`'s fields over `under`'s.
    pub fn over(self, under: Self) -> Self {
        Self {
            connection: self.connection.or(under.connection),
            spoke_width: self.spoke_width.or(under.spoke_width),
            spoke_angle_deg: self.spoke_angle_deg.or(under.spoke_angle_deg),
            thermal_gap: self.thermal_gap.or(under.thermal_gap),
            clearance: self.clearance.or(under.clearance),
        }
    }

    /// Lengths within 0–[`crate::rules::MAX_RULE_MM`] (a spoke wider than
    /// 0), an angle within ±360°.
    pub fn valid(&self) -> bool {
        let mm = |v: Option<f64>| {
            v.is_none_or(|v| v.is_finite() && (0.0..=crate::rules::MAX_RULE_MM).contains(&v))
        };
        mm(self.spoke_width)
            && self.spoke_width.is_none_or(|w| w > 0.0)
            && mm(self.thermal_gap)
            && mm(self.clearance)
            && self
                .spoke_angle_deg
                .is_none_or(|a| a.is_finite() && (-360.0..=360.0).contains(&a))
    }

    /// The thermal relief `base` with this override's spoke and gap.
    pub fn thermal(&self, base: ThermalRelief) -> ThermalRelief {
        ThermalRelief {
            spoke_width: self.spoke_width.unwrap_or(base.spoke_width),
            spoke_count: base.spoke_count,
            gap: self.thermal_gap.unwrap_or(base.gap),
            angle_deg: self.spoke_angle_deg.unwrap_or(base.angle_deg),
        }
    }
}

/// One pad's [`PadFillOverride`] on a placed part.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadFillOverrideEntry {
    pub pad: crate::ids::PadId,
    pub settings: PadFillOverride,
}

/// A fill's departures from the effective fill rules.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FillSettings {
    #[serde(default)]
    pub clearance: Option<f64>,
    #[serde(default)]
    pub width_min: Option<f64>,
    #[serde(default)]
    pub thermal: Option<ThermalRelief>,
    #[serde(default)]
    pub connections: PadConnections,
    #[serde(default)]
    pub hatch: Option<Hatch>,
    #[serde(default)]
    pub islands: IslandRemoval,
}

/// Where a fill's region comes from. Stored, so whether a fill follows
/// the board is the designer's explicit choice, never inferred from its
/// outline's coordinates.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FillRegion {
    /// Its own drawn outline, at fixed board coordinates.
    #[default]
    Drawn,
    /// The board outline: its `outline` is the solved board outline,
    /// derived again after every operation (`layout::fill::follow_board`),
    /// so the fill follows board parameter and shape edits. The fill
    /// computation still keeps it the fill-to-edge clearance inside the
    /// edge and clears cutouts, slots, holes and keepouts.
    Board,
}

impl FillRegion {
    pub fn is_drawn(&self) -> bool {
        *self == Self::Drawn
    }
}

/// The outline of a fill spec that gives none: empty, which only a
/// board-following fill may have (its outline is derived).
fn no_outline() -> Outline {
    Outline {
        start: [0.0, 0.0],
        segments: Vec::new(),
    }
}

/// A fill to add: everything but its identity and computed copper.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FillSpec {
    #[serde(default)]
    pub name: String,
    /// `None`: unconnected copper (no net).
    pub net: Option<Id>,
    pub layer: CopperLayer,
    /// Drawn (the default) or the board outline.
    #[serde(default, skip_serializing_if = "FillRegion::is_drawn")]
    pub region: FillRegion,
    /// The region the fill may occupy, with arcs. A board-following fill
    /// may omit it: the board outline replaces it.
    #[serde(default = "no_outline")]
    pub outline: Outline,
    /// Higher fills claim overlapping area first.
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub settings: FillSettings,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Fill {
    pub id: Id,
    #[serde(default)]
    pub name: String,
    pub net: Option<Id>,
    pub layer: CopperLayer,
    #[serde(default, skip_serializing_if = "FillRegion::is_drawn")]
    pub region: FillRegion,
    /// The drawn outline, or for [`FillRegion::Board`] the board outline
    /// it was last derived from.
    pub outline: Outline,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub settings: FillSettings,
    #[serde(default)]
    pub locked: bool,
    /// The last computed copper. Derivable: refilled when its key no
    /// longer matches the inputs (`layout::fill::inputs_key`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computed: Option<FilledCopper>,
}

impl Fill {
    pub fn from_spec(id: Id, s: FillSpec) -> Self {
        Self {
            id,
            name: s.name,
            net: s.net,
            layer: s.layer,
            region: s.region,
            outline: s.outline,
            priority: s.priority,
            settings: s.settings,
            locked: false,
            computed: None,
        }
    }
}

/// One connected piece of computed fill copper, flattened at
/// `layout::fill::FILL_CHORD_MM`: an outer boundary (counter-clockwise)
/// and its holes (clockwise).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FilledRegion {
    pub outer: Vec<Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Vec<Point>>,
}

/// A fill's computed copper and what it was computed from.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FilledCopper {
    /// Hex hash of every input (`layout::fill::inputs_key`).
    pub key: String,
    /// The copper islands. Which copper they join is not stored: the
    /// copper graph (`layout::connectivity`) joins fresh islands by its one
    /// rule.
    pub regions: Vec<FilledRegion>,
    /// Islands removed by the island rule.
    #[serde(default)]
    pub removed_islands: u32,
}

/// Which copper layers a keepout applies to.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "layers", content = "only")]
pub enum KeepoutLayers {
    #[default]
    All,
    Only(Vec<CopperLayer>),
}

/// What a keepout forbids inside its outline.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeepoutRules {
    /// No fill copper.
    pub fills: bool,
    pub tracks: bool,
    pub vias: bool,
    /// No pads (placement keepout).
    pub pads: bool,
}

impl Default for KeepoutRules {
    fn default() -> Self {
        Self {
            fills: true,
            tracks: true,
            vias: true,
            pads: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeepoutSpec {
    #[serde(default)]
    pub name: String,
    pub outline: Outline,
    #[serde(default)]
    pub layers: KeepoutLayers,
    #[serde(default)]
    pub forbid: KeepoutRules,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Keepout {
    pub id: Id,
    #[serde(default)]
    pub name: String,
    pub outline: Outline,
    #[serde(default)]
    pub layers: KeepoutLayers,
    #[serde(default)]
    pub forbid: KeepoutRules,
}

impl Keepout {
    pub fn from_spec(id: Id, s: KeepoutSpec) -> Self {
        Self {
            id,
            name: s.name,
            outline: s.outline,
            layers: s.layers,
            forbid: s.forbid,
        }
    }

    pub fn on(&self, layer: CopperLayer) -> bool {
        match &self.layers {
            KeepoutLayers::All => true,
            KeepoutLayers::Only(layers) => layers.contains(&layer),
        }
    }
}
