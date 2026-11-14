//! Zones as copper fills and keepouts, through the one command engine.
//! A zone's definition (net, layers, outline, priority, clearance, least
//! width, pad connection, thermal relief, island removal) becomes a fill
//! per copper layer; its copper is then computed by our engine
//! (`fill.refill`), never taken from the file's stored fill (decision:
//! fills are recomputed). A rule area becomes a keepout.
use crate::board::{Board, Nm, PointNm};
use crate::import::{ImportError, Session};
use crate::items::{KZone, ZoneConnect};
use crate::report::{Case, Report};
use crate::translate::board_point;
use sparksmith_core::geometry::Point;
use sparksmith_core::layout::fill::{FillCommand, RefillTarget};
use sparksmith_core::library::{Outline, Segment};
use sparksmith_core::model::fill::{
    FillSettings, FillSpec, IslandRemoval, KeepoutLayers, KeepoutRules, KeepoutSpec, PadConnections,
};
use sparksmith_core::rules::{PadConnection, ThermalRelief};
use sparksmith_core::{CopperLayer, Id, Operation};
use std::collections::BTreeMap;

/// Spokes KiCad draws per thermal relief.
const SPOKES: u32 = 4;

fn mm(nm: Nm) -> f64 {
    nm as f64 / 1e6
}

/// The copper layers a zone's layer names cover (`F&B.Cu` and `*.Cu`
/// included), in board order.
fn layers(board: &Board, zone: &KZone) -> Vec<CopperLayer> {
    let last = board.copper.len().saturating_sub(1);
    board
        .copper
        .iter()
        .enumerate()
        .filter(|(k, name)| {
            zone.layers
                .iter()
                .any(|l| l == *name || l == "*.Cu" || (l == "F&B.Cu" && (*k == 0 || *k == last)))
        })
        .filter_map(|(k, _)| match k {
            0 => Some(CopperLayer::Top),
            k if k == last => Some(CopperLayer::Bottom),
            k => u8::try_from(k).ok().map(CopperLayer::Inner),
        })
        .collect()
}

/// A closed outline of straight segments through `ring` (our frame).
fn outline(ring: &[Point]) -> Option<Outline> {
    let (first, rest) = ring.split_first()?;
    if rest.len() < 2 {
        return None;
    }
    let mut segments: Vec<Segment> = rest.iter().map(|p| Segment::Line { to: *p }).collect();
    segments.push(Segment::Line { to: *first });
    Some(Outline {
        start: *first,
        segments,
    })
}

fn settings(zone: &KZone, report: &mut Report) -> FillSettings {
    let thermal = match (zone.thermal_gap, zone.thermal_bridge) {
        (Some(gap), Some(width)) => Some(ThermalRelief {
            spoke_width: mm(width),
            spoke_count: SPOKES,
            gap: mm(gap),
            angle_deg: 0.0,
        }),
        _ => None,
    };
    let (smd, through_hole) = match zone.connect {
        ZoneConnect::Thermal => (PadConnection::Thermal, PadConnection::Thermal),
        ZoneConnect::Solid => (PadConnection::Solid, PadConnection::Solid),
        ZoneConnect::None => (PadConnection::None, PadConnection::None),
        ZoneConnect::ThroughHoleOnly => (PadConnection::None, PadConnection::Thermal),
    };
    if zone.hatched {
        report.add(Case::ZoneHatched);
    }
    if zone.smoothing {
        report.add(Case::ZoneSmoothing);
    }
    FillSettings {
        clearance: zone.clearance.map(mm),
        width_min: zone.min_thickness.map(mm),
        thermal,
        connections: PadConnections {
            smd: Some(smd),
            through_hole: Some(through_hole),
            // Vias join their net's zones solidly.
            via: Some(PadConnection::Solid),
        },
        hatch: None,
        // KiCad's island removal: 0 always, 1 never, 2 below an area.
        islands: match zone.island_mode {
            Some(1) => IslandRemoval::Never,
            Some(2) => IslandRemoval::BelowArea(zone.island_area_min.unwrap_or(0.0)),
            _ => IslandRemoval::Unconnected,
        },
    }
}

/// Every zone as fills (one per copper layer) or a keepout, then the
/// fills computed by our engine.
pub(crate) fn fills(
    session: &mut Session<'_>,
    board: &Board,
    nets: &BTreeMap<u32, Id>,
    report: &mut Report,
) -> Result<(), ImportError> {
    if board.items.zones.is_empty() {
        return Ok(());
    }
    if session.engine.project().rules.require().is_err() {
        report.add_n(Case::ZoneRefused, board.items.zones.len());
        return Ok(());
    }
    let p = |q: PointNm| board_point(board, q);
    let mut ops = Vec::new();
    for zone in &board.items.zones {
        if zone.arcs > 0 {
            report.add(Case::ZoneRefused);
            continue;
        }
        let outlines: Vec<Outline> = zone
            .outlines
            .iter()
            .filter_map(|ring| outline(&ring.iter().map(|q| p(*q)).collect::<Vec<_>>()))
            .collect();
        if outlines.is_empty() {
            report.add(Case::ZoneRefused);
            continue;
        }
        if outlines.len() > 1 {
            report.add(Case::ZonePolygons);
        }
        let layers = layers(board, zone);
        if let Some([tracks, vias, pads, pour, _footprints]) = zone.keepout {
            for outline in outlines {
                ops.push(Operation::Fill(FillCommand::AddKeepout {
                    keepout: Box::new(KeepoutSpec {
                        name: String::new(),
                        outline,
                        layers: KeepoutLayers::Only(layers.clone()),
                        forbid: KeepoutRules {
                            fills: pour,
                            tracks,
                            vias,
                            pads,
                        },
                    }),
                }));
                report.add(Case::Keepout);
            }
            continue;
        }
        let net = match zone.net {
            0 => None,
            n => match nets.get(&n) {
                Some(id) => Some(*id),
                None => {
                    report.add(Case::ZoneRefused);
                    continue;
                }
            },
        };
        let settings = settings(zone, report);
        for layer in &layers {
            for outline in &outlines {
                ops.push(Operation::Fill(FillCommand::Add {
                    fill: Box::new(FillSpec {
                        name: String::new(),
                        net,
                        layer: *layer,
                        region: Default::default(),
                        outline: outline.clone(),
                        priority: zone.priority,
                        settings,
                    }),
                }));
                report.add(Case::Zone);
            }
        }
    }
    session.batches("fill.add", ops)?;
    session.apply(
        "fill.refill",
        Operation::Fill(FillCommand::Refill {
            target: RefillTarget::All,
        }),
    )?;
    report.counts.fills = session.engine.project().fills.len();
    Ok(())
}
