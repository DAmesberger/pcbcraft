//! The board of the PCB realism showcase (`Engine::showcase`): a 4-layer
//! 44 × 28 mm board with its placements, routing and vias as layout
//! commands, so the showcase is built through the same engine as any
//! design. Original layout; nets and parts come from
//! `library::samples::SHOWCASE_NETS` and `SHOWCASE_INSTANCES`.
//!
//! The routing is on top and bottom with through vias, tented and open; a
//! layer change is routed with `layout.route_layer_change`. Ground runs as
//! wide bottom traces (there are no zones yet) and an inner-layer strap,
//! the supply on an inner layer between its vias.
use super::{LayerChangeRequest, LayoutCommand, RouteRequest, ViaRequest};
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use crate::ids::{Endpoint, Id, TerminalId};
use crate::model::{CopperLayer, Layer, Project, Stackup};

/// The showcase board outline, in mm.
pub const BOARD_MM: Point = [44.0, 28.0];

const TOP: CopperLayer = CopperLayer::Top;
const BOTTOM: CopperLayer = CopperLayer::Bottom;
const GND_PLANE: CopperLayer = CopperLayer::Inner(1);
const SUPPLY_PLANE: CopperLayer = CopperLayer::Inner(2);

/// (designator, position, rotation in degrees, side).
const PLACEMENTS: &[(&str, Point, f64, Layer)] = &[
    ("J1", [6.0, 15.0], 90.0, Layer::Top),
    ("U1", [20.0, 15.0], 90.0, Layer::Top),
    ("U2", [36.0, 15.0], 0.0, Layer::Top),
    ("C1", [17.3, 10.5], 90.0, Layer::Top),
    ("C2", [32.5, 18.0], 0.0, Layer::Top),
    ("C3", [11.0, 6.0], 0.0, Layer::Top),
    ("R1", [10.5, 20.5], 90.0, Layer::Top),
    ("R2", [14.0, 20.5], 90.0, Layer::Top),
    ("R3", [24.0, 24.0], 0.0, Layer::Bottom),
];

/// A routed path: (net, layer, width in mm, points). Each consecutive pair
/// is one track; tracks meeting at a corner are lengthened to fill it.
type Path = (&'static str, CopperLayer, f64, &'static [Point]);

const PATHS: &[Path] = &[
    // Supply on top from the header to U1, its decoupling and the
    // tantalum reservoir capacitor.
    (
        "VCC",
        TOP,
        0.5,
        &[[7.27, 9.92], [11.0, 9.92], [14.175, 13.095], [17.3, 13.095]],
    ),
    ("VCC", TOP, 0.5, &[[9.65, 9.92], [9.65, 6.0]]),
    ("VCC", TOP, 0.5, &[[17.3, 11.3], [17.3, 13.095]]),
    ("VCC", TOP, 0.5, &[[17.3, 11.3], [19.2, 11.3]]),
    ("VCC", TOP, 0.4, &[[10.5, 21.14], [15.6, 21.14]]),
    ("VCC", TOP, 0.5, &[[33.3, 18.0], [34.6, 18.0]]),
    (
        "VCC",
        TOP,
        0.25,
        &[[33.3, 18.0], [33.3, 15.75], [34.7, 15.75]],
    ),
    // The supply on the inner supply layer between its vias.
    (
        "VCC",
        SUPPLY_PLANE,
        0.5,
        &[
            [15.6, 21.14],
            [19.2, 17.54],
            [19.2, 11.3],
            [34.6, 11.3],
            [34.6, 18.0],
        ],
    ),
    // Ground stubs from top pads to their vias.
    ("GND", TOP, 0.6, &[[12.35, 6.0], [14.0, 6.0]]),
    ("GND", TOP, 0.6, &[[17.3, 9.7], [17.3, 8.2]]),
    ("GND", TOP, 0.5, &[[22.7, 16.905], [22.7, 18.5]]),
    ("GND", TOP, 0.5, &[[31.7, 18.0], [30.2, 18.0]]),
    // Wide ground on the bottom.
    (
        "GND",
        BOTTOM,
        1.0,
        &[[4.73, 20.08], [2.2, 20.08], [2.2, 9.92], [4.73, 9.92]],
    ),
    (
        "GND",
        BOTTOM,
        1.0,
        &[
            [4.73, 9.92],
            [4.73, 4.0],
            [17.3, 4.0],
            [17.3, 8.2],
            [36.0, 8.2],
            [36.0, 15.0],
        ],
    ),
    ("GND", BOTTOM, 1.0, &[[14.0, 4.0], [14.0, 6.0]]),
    ("GND", BOTTOM, 1.0, &[[22.7, 8.2], [22.7, 18.5]]),
    ("GND", BOTTOM, 1.0, &[[30.2, 8.2], [30.2, 18.0]]),
    // A ground strap on the inner ground layer.
    ("GND", GND_PLANE, 1.5, &[[22.7, 18.5], [30.2, 18.0]]),
    // Header to U1 on top, fanning in without crossings.
    (
        "EN",
        TOP,
        0.25,
        &[
            [7.27, 12.46],
            [10.0, 12.46],
            [11.905, 14.365],
            [17.3, 14.365],
        ],
    ),
    (
        "SCL",
        TOP,
        0.25,
        &[[7.27, 15.0], [12.0, 15.0], [12.635, 15.635], [17.3, 15.635]],
    ),
    ("SCL", BOTTOM, 0.25, &[[9.5, 15.0], [13.1, 18.6]]),
    (
        "SDA",
        TOP,
        0.25,
        &[
            [7.27, 17.54],
            [12.0, 17.54],
            [12.635, 16.905],
            [17.3, 16.905],
        ],
    ),
    ("SDA", TOP, 0.25, &[[10.5, 17.54], [10.5, 19.86]]),
    // U1 to the QFN, converging to its 0.5 mm pitch.
    (
        "D0",
        TOP,
        0.2,
        &[
            [22.7, 13.095],
            [31.0, 13.095],
            [32.155, 14.25],
            [34.7, 14.25],
        ],
    ),
    (
        "D1",
        TOP,
        0.2,
        &[
            [22.7, 14.365],
            [30.0, 14.365],
            [30.385, 14.75],
            [34.7, 14.75],
        ],
    ),
    (
        "D2",
        TOP,
        0.2,
        &[
            [22.7, 15.635],
            [30.0, 15.635],
            [30.385, 15.25],
            [34.7, 15.25],
        ],
    ),
    // The interrupt runs on the bottom through the bottom-side resistor
    // back to the header.
    (
        "INT",
        BOTTOM,
        0.25,
        &[[38.6, 22.0], [36.6, 24.0], [24.64, 24.0]],
    ),
    (
        "OUT",
        BOTTOM,
        0.25,
        &[[23.36, 24.0], [11.3, 24.0], [7.27, 20.08]],
    ),
];

/// (net, position, tented). Open vias show their finished ring.
const VIAS: &[(&str, Point, bool)] = &[
    ("GND", [14.0, 6.0], true),
    ("GND", [17.3, 8.2], true),
    ("GND", [22.7, 18.5], true),
    ("GND", [30.2, 18.0], true),
    // In the QFN's exposed pad.
    ("GND", [36.0, 15.0], false),
    ("VCC", [19.2, 11.3], true),
    ("VCC", [15.6, 21.14], true),
    ("VCC", [34.6, 18.0], true),
    ("SCL", [9.5, 15.0], false),
];

/// Layer changes: (net, first layer, start, via, tented, second layer,
/// end, width).
type LayerChange = (
    &'static str,
    CopperLayer,
    Point,
    Point,
    bool,
    CopperLayer,
    Point,
    f64,
);

const LAYER_CHANGES: &[LayerChange] = &[
    (
        "INT",
        TOP,
        [37.3, 14.75],
        [38.6, 14.75],
        false,
        BOTTOM,
        [38.6, 22.0],
        0.2,
    ),
    (
        "SCL",
        BOTTOM,
        [13.1, 18.6],
        [14.0, 18.6],
        true,
        TOP,
        [14.0, 19.86],
        0.25,
    ),
];

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn unit(v: Point) -> Point {
    let length = v[0].hypot(v[1]);
    if length > 0.0 {
        [v[0] / length, v[1] / length]
    } else {
        [0.0, 0.0]
    }
}

/// The tracks of one path. Butt-capped tracks leave a notch at the outside
/// of a corner, so at every interior corner both tracks are lengthened by
/// `width / 2 · tan(turn / 2)`, which fills the mitre.
fn tracks(net: Id, layer: CopperLayer, width: f64, points: &[Point]) -> Vec<RouteRequest> {
    let directions: Vec<Point> = points.windows(2).map(|w| unit(sub(w[1], w[0]))).collect();
    let extension = |k: usize| -> f64 {
        let (Some(a), Some(b)) = (directions.get(k), directions.get(k + 1)) else {
            return 0.0;
        };
        let turn = (a[0] * b[0] + a[1] * b[1]).clamp(-1.0, 1.0).acos();
        width / 2.0 * (turn / 2.0).tan().min(1.0)
    };
    points
        .windows(2)
        .enumerate()
        .map(|(k, w)| {
            let d = directions.get(k).copied().unwrap_or([0.0, 0.0]);
            let before = if k == 0 { 0.0 } else { extension(k - 1) };
            let after = extension(k);
            RouteRequest {
                net,
                start: [w[0][0] - d[0] * before, w[0][1] - d[1] * before],
                end: [w[1][0] + d[0] * after, w[1][1] + d[1] * after],
                width,
                layer,
            }
        })
        .collect()
}

/// The layout commands that build the showcase board over `project`,
/// whose instances and nets carry the showcase designators and net names.
pub fn commands(project: &Project) -> Result<Vec<LayoutCommand>> {
    let instance = |designator: &str| {
        project
            .instance_by_designator(designator)
            .map(|i| i.id)
            .ok_or_else(|| Error::NotFound(format!("showcase instance {designator}")))
    };
    let net = |name: &str| {
        project
            .nets
            .iter()
            .find(|n| n.name == name)
            .map(|n| n.id)
            .ok_or_else(|| Error::NotFound(format!("showcase net {name}")))
    };
    let mut commands = vec![
        LayoutCommand::SetStackup(Stackup::preset(4)?),
        LayoutCommand::SetBoardSize { size: BOARD_MM },
    ];
    for (designator, position, rotation_deg, layer) in PLACEMENTS {
        commands.push(LayoutCommand::Place {
            instance: instance(designator)?,
            position: *position,
            rotation_deg: *rotation_deg,
            layer: *layer,
        });
    }
    for (name, position, tented) in VIAS {
        let mut via = ViaRequest::through(net(name)?, *position);
        via.tented = *tented;
        commands.push(LayoutCommand::AddVia { via });
    }
    for (name, layer, width, points) in PATHS {
        for request in tracks(net(name)?, *layer, *width, points) {
            commands.push(LayoutCommand::Route { request });
        }
    }
    for (name, layer, start, at, tented, to_layer, end, width) in LAYER_CHANGES {
        let net = net(name)?;
        let mut via = ViaRequest::through(net, *at);
        via.tented = *tented;
        commands.push(LayoutCommand::RouteLayerChange {
            request: LayerChangeRequest {
                request: RouteRequest {
                    net,
                    start: *start,
                    end: *at,
                    width: *width,
                    layer: *layer,
                },
                via,
                to_layer: *to_layer,
                end: *end,
            },
        });
    }
    Ok(commands)
}

/// A showcase terminal, `designator.terminal`, as an endpoint of `project`.
pub fn endpoint(project: &Project, designator: &str, terminal: u32) -> Result<Endpoint> {
    let instance = project
        .instance_by_designator(designator)
        .ok_or_else(|| Error::NotFound(format!("showcase instance {designator}")))?;
    Ok(Endpoint {
        instance: instance.id,
        terminal: TerminalId(terminal),
    })
}

#[cfg(test)]
mod tests;
