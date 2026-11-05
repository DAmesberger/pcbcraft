//! Every drilled and routed hole of the board, and the tool table: through
//! vias, pad holes and slots, pad-owned vias, board mounting holes and
//! plated board slots. Non-plated board cutouts and slots are milled along
//! the profile layer instead (`artwork`).
use crate::geometry::{Point, distance};
use crate::ids::Id;
use crate::layout::{self};
use crate::library::{HoleShape, Outline, Segment};
use crate::model::Project;
use crate::model::board::{CutoutKind, arc_geometry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Diameters within this many mm share one tool.
pub const TOOL_RESOLUTION_MM: f64 = 0.001;

/// A round hit or a routed slot (round ends of `width`, centre line from
/// `from` to `to`), board millimetres.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HoleFeature {
    Round { at: Point, diameter: f64 },
    Slot { from: Point, to: Point, width: f64 },
}

impl HoleFeature {
    /// The tool diameter: the drill or the slot width.
    pub fn diameter(&self) -> f64 {
        match *self {
            Self::Round { diameter, .. } => diameter,
            Self::Slot { width, .. } => width,
        }
    }
    pub fn is_slot(&self) -> bool {
        matches!(self, Self::Slot { .. })
    }
}

/// What a hole is for (the X2 `.AperFunction` of drill files).
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum HoleFunction {
    /// A board via (`ViaDrill`).
    Via,
    /// A component lead or pad hole, or a pad-owned via (`ComponentDrill`).
    ComponentPin,
    /// A board mounting hole or a footprint's bare hole (`MechanicalDrill`).
    Mechanical,
}

impl HoleFunction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Via => "via",
            Self::ComponentPin => "component",
            Self::Mechanical => "mechanical",
        }
    }
    /// The Gerber X2 aperture function of the hole.
    pub fn aper_function(self) -> &'static str {
        match self {
            Self::Via => "ViaDrill",
            Self::ComponentPin => "ComponentDrill",
            Self::Mechanical => "MechanicalDrill",
        }
    }
}

/// One hole and what it belongs to.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DrillHole {
    pub feature: HoleFeature,
    pub plated: bool,
    pub function: HoleFunction,
    /// The net of a plated hole, when it has one.
    #[serde(default)]
    pub net: Option<Id>,
    /// The placed instance and pad label of a pad hole.
    #[serde(default)]
    pub component: Option<Id>,
    #[serde(default)]
    pub pad: Option<String>,
}

/// One tool: holes of one diameter, plating, kind (hit or slot) and
/// function. `number` counts from 1 across the board, plated tools first,
/// so one number names one tool in the drill table, the drill map and both
/// Excellon files.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DrillTool {
    pub number: u32,
    pub diameter: f64,
    pub plated: bool,
    pub slot: bool,
    pub function: HoleFunction,
    pub count: usize,
}

/// The tools of a board, plated first, each group by diameter.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DrillTable {
    pub tools: Vec<DrillTool>,
}

impl DrillTable {
    /// The tool a hole is drilled with.
    pub fn tool_of(&self, hole: &DrillHole) -> Option<&DrillTool> {
        let key = tool_key(hole);
        self.tools
            .iter()
            .find(|t| (t.plated, nm(t.diameter), t.slot, t.function) == key)
    }
    pub fn plated(&self) -> impl Iterator<Item = &DrillTool> {
        self.tools.iter().filter(|t| t.plated)
    }
    pub fn non_plated(&self) -> impl Iterator<Item = &DrillTool> {
        self.tools.iter().filter(|t| !t.plated)
    }
}

/// A diameter in whole tool-resolution steps.
fn nm(diameter: f64) -> i64 {
    // Bounded: diameters are validated to well under 1 m.
    (diameter / TOOL_RESOLUTION_MM).round() as i64
}

fn tool_key(hole: &DrillHole) -> (bool, i64, bool, HoleFunction) {
    (
        hole.plated,
        nm(hole.feature.diameter()),
        hole.feature.is_slot(),
        hole.function,
    )
}

/// Every hole of `project` in a fixed order: vias, then pad holes in
/// placement and pad order, then board mounting holes and plated slots.
pub fn holes(project: &Project) -> Vec<DrillHole> {
    let mut out: Vec<DrillHole> = project
        .vias
        .iter()
        .map(|via| DrillHole {
            feature: HoleFeature::Round {
                at: via.position,
                diameter: via.drill,
            },
            plated: true,
            function: HoleFunction::Via,
            net: Some(via.net),
            component: None,
            pad: None,
        })
        .collect();
    let labels = layout::pad_labels(project);
    for placed in layout::placed_pads(project) {
        let t = placed.pad_transform();
        let label = labels.get(&(placed.instance, placed.pad.id)).cloned();
        if let Some(hole) = &placed.pad.hole {
            let feature = match hole.shape {
                HoleShape::Round { diameter } => HoleFeature::Round {
                    at: t.apply(hole.offset),
                    diameter,
                },
                HoleShape::Slot {
                    width,
                    length,
                    angle_deg,
                } => {
                    let half = (length.max(width) - width) / 2.0;
                    let (c, s) = crate::geometry::cos_sin(angle_deg);
                    let o = hole.offset;
                    HoleFeature::Slot {
                        from: t.apply([o[0] - c * half, o[1] - s * half]),
                        to: t.apply([o[0] + c * half, o[1] + s * half]),
                        width,
                    }
                }
            };
            out.push(DrillHole {
                feature,
                plated: hole.plated,
                function: if hole.plated {
                    HoleFunction::ComponentPin
                } else {
                    HoleFunction::Mechanical
                },
                net: placed.net.filter(|_| hole.plated),
                component: Some(placed.instance),
                pad: label.clone(),
            });
        }
        for via in &placed.pad.vias {
            out.push(DrillHole {
                feature: HoleFeature::Round {
                    at: t.apply(via.at),
                    diameter: via.drill,
                },
                plated: true,
                function: HoleFunction::ComponentPin,
                net: placed.net,
                component: Some(placed.instance),
                pad: label.clone(),
            });
        }
    }
    let board = project.board();
    for hole in &board.holes {
        out.push(DrillHole {
            feature: HoleFeature::Round {
                at: hole.at,
                diameter: hole.drill,
            },
            plated: hole.plated,
            function: HoleFunction::Mechanical,
            net: None,
            component: None,
            pad: None,
        });
    }
    for cutout in &board.cutouts {
        if cutout.kind == (CutoutKind::Slot { plated: true })
            && let Some(feature) = stadium(&cutout.outline)
        {
            out.push(DrillHole {
                feature,
                plated: true,
                function: HoleFunction::Mechanical,
                net: None,
                component: None,
                pad: None,
            });
        }
    }
    out
}

/// A closed outline that is a straight slot (two half-turn arcs joined by
/// two lines, or a circle of two half turns) as a routed slot or a round
/// hit; `None` for any other shape.
pub fn stadium(outline: &Outline) -> Option<HoleFeature> {
    let mut from = outline.start;
    let mut arcs = Vec::new();
    let mut lines = 0;
    for s in &outline.segments {
        let to = s.to();
        match *s {
            Segment::Line { .. } if distance(from, to) > 1e-9 => lines += 1,
            Segment::Line { .. } => {}
            Segment::Arc { bulge, .. } => {
                let arc = arc_geometry(from, to, bulge)?;
                if (arc.sweep.abs() - std::f64::consts::PI).abs() > 1e-6 {
                    return None;
                }
                arcs.push(arc);
            }
        }
        from = to;
    }
    let [a, b] = arcs.as_slice() else {
        return None;
    };
    if (a.radius - b.radius).abs() > 1e-6 || !(lines == 2 || lines == 0) {
        return None;
    }
    if distance(a.center, b.center) <= 1e-6 {
        return Some(HoleFeature::Round {
            at: a.center,
            diameter: 2.0 * a.radius,
        });
    }
    (lines == 2).then_some(HoleFeature::Slot {
        from: a.center,
        to: b.center,
        width: 2.0 * a.radius,
    })
}

/// The tool table of `holes`: plated tools first, then non-plated, each by
/// diameter, then hits before slots, then function; numbered from 1
/// across both, so the non-plated file continues where the plated one
/// stops (T1–T3 plated, T4 non-plated).
pub fn table(holes: &[DrillHole]) -> DrillTable {
    let mut counts: std::collections::BTreeMap<(bool, i64, bool, HoleFunction), (f64, usize)> =
        Default::default();
    for hole in holes {
        let entry = counts
            .entry(tool_key(hole))
            .or_insert((hole.feature.diameter(), 0));
        entry.1 += 1;
    }
    let mut keys: Vec<_> = counts.into_iter().collect();
    // Plated (true) first.
    keys.sort_by_key(|((plated, d, slot, f), _)| (!*plated, *d, *slot, *f));
    let mut tools = Vec::with_capacity(keys.len());
    let mut n = 0u32;
    for ((plated, d, slot, function), (_, count)) in keys {
        n = n.saturating_add(1);
        tools.push(DrillTool {
            number: n,
            diameter: d as f64 * TOOL_RESOLUTION_MM,
            plated,
            slot,
            function,
            count,
        });
    }
    DrillTable { tools }
}
