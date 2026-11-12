//! Board vias: plated holes that join a net's copper across layers. Data
//! only; placement rules and checks live in `layout::via`.
use super::CopperLayer;
use crate::geometry::Point;
use crate::ids::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The copper layers a via's barrel joins.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ViaSpan {
    /// Drilled through the whole board; joins every copper layer.
    Through,
    /// Joins `from` to `to` only (blind when one is outer, buried when both
    /// are inner). Reserved: commands refuse it until the stackup records
    /// its drill pairs.
    Between { from: CopperLayer, to: CopperLayer },
}

/// A plated via on the board, a document object with its own identity.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Via {
    pub id: Id,
    pub net: Id,
    pub position: Point,
    /// Finished drill diameter in mm.
    pub drill: f64,
    /// Annular pad diameter in mm on every layer the via joins.
    pub diameter: f64,
    pub span: ViaSpan,
    /// Soldermask covers the via on both sides; an open via shows its ring.
    pub tented: bool,
    pub locked: bool,
}

impl Via {
    /// Whether the via has copper on `layer` of a board with `layers`
    /// (top to bottom).
    pub fn on(&self, layer: CopperLayer, layers: &[CopperLayer]) -> bool {
        match self.span {
            ViaSpan::Through => layers.contains(&layer),
            ViaSpan::Between { from, to } => {
                let (Some(a), Some(b), Some(at)) = (
                    layers.iter().position(|l| *l == from),
                    layers.iter().position(|l| *l == to),
                    layers.iter().position(|l| *l == layer),
                ) else {
                    return false;
                };
                (a.min(b)..=a.max(b)).contains(&at)
            }
        }
    }

    /// Annular ring width in mm.
    pub fn annular_ring(&self) -> f64 {
        (self.diameter - self.drill) / 2.0
    }

    /// The soldermask opening's diameter in mm over an open (untented)
    /// via: its ring grown on every side by `expansion`, the effective
    /// rules' `mask.expansion`. The Gerber mask and the 3D mask both read
    /// it.
    pub fn mask_opening(&self, expansion: f64) -> f64 {
        self.diameter + 2.0 * expansion
    }
}
