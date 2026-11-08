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

/// Most fills and keepouts on one board.
pub const MAX_FILLS: usize = 512;
pub const MAX_KEEPOUTS: usize = 512;

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

/// A fill to add: everything but its identity and computed copper.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FillSpec {
    #[serde(default)]
    pub name: String,
    /// `None`: unconnected copper (no net).
    pub net: Option<Id>,
    pub layer: CopperLayer,
    /// The region the fill may occupy, with arcs.
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
