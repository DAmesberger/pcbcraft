//! The board of the PCB realism showcase (`Engine::showcase`): a 4-layer
//! 44 × 28 mm board with its placements, routing and vias as layout
//! commands, so the showcase is built through the same engine as any
//! design. Original layout; nets and parts come from
//! `library::samples::SHOWCASE_NETS` and `SHOWCASE_INSTANCES`.
//!
//! The routing is on top and bottom with through vias, tented and open; a
//! layer change is routed with `layout.route_layer_change`. Ground runs as
//! wide bottom traces under a bottom ground pour, an inner ground plane and
//! an inner-layer strap, tied together by stitching vias on the ground
//! traces; the supply runs on an inner layer between its vias.
//!
//! [`crate::Engine::showcase_hosted`] adds what the shipped showcase file
//! shows of design rules and board shape on top: a Power net class over
//! GND and VCC, a top GND pour, and, with a board sketch evaluator, a
//! sketched outline with rounded corners (`corner_r`), a round cutout, a
//! milled slot and the header J1 held to the left and bottom edges by part
//! dimensions over board parameters ([`board_edits`], [`j1_dimensions`]).
use super::fill::FillCommand;
use super::outline::BoardEdit;
use super::{LayerChangeRequest, LayoutCommand, RouteRequest, ViaRequest};
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use crate::ids::{Endpoint, Id, TerminalId};
use crate::library::Outline;
use crate::library::footprint_source::{
    DimKind, DimValue, Expr, FootprintParam, ParamName, ParamUnit, SketchEdit, SketchRef,
};
use crate::model::board::{BoardShape, ConstraintTarget};
use crate::model::dimension::{BoardRef, FeatureRef, PartDimensionSpec, PartRelation};
use crate::model::fill::{FillSettings, FillSpec, PadConnections};
use crate::model::{CopperLayer, Layer, Project, Stackup};
use crate::rules::{NetClassSpec, PadConnection};

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
    // Ground stitching on the ground traces: they tie the bottom pour,
    // the inner ground plane and the top together.
    ("GND", [2.2, 12.0], true),
    ("GND", [2.2, 15.0], true),
    ("GND", [2.2, 18.0], true),
    ("GND", [7.0, 4.0], true),
    ("GND", [15.5, 4.0], true),
    ("GND", [20.0, 8.2], true),
    ("GND", [26.0, 8.2], true),
    ("GND", [33.0, 8.2], true),
    ("GND", [22.7, 10.0], true),
    ("GND", [30.2, 10.0], true),
];

/// The pours: (name, net, layer, priority).
const POURS: &[(&str, &str, CopperLayer, u32)] = &[
    ("GND bottom pour", "GND", BOTTOM, 0),
    ("GND plane", "GND", GND_PLANE, 0),
];

/// The hosted showcase's top pour: thermals to the header's GND pins and
/// the vias; surface pads join solid (the QFN's exposed pad, ringed by
/// its pins, leaves no room for spokes).
const TOP_POUR: (&str, &str, CopperLayer, u32) = ("GND top pour", "GND", TOP, 0);

/// The showcase project's name.
pub const PROJECT_NAME: &str = "Showcase";
/// The hosted showcase's board corner radius, mm.
pub const CORNER_MM: f64 = 2.0;
/// The hosted showcase's round cutout: (centre, radius), mm.
pub const CUTOUT: (Point, f64) = ([40.0, 4.5], 1.75);
/// The hosted showcase's milled slot: (cap centres, width), mm.
pub const SLOT: (Point, Point, f64) = ([2.5, 25.0], [5.5, 25.0], 1.0);
/// The net class of the supply and ground nets, and its member nets.
pub const POWER_NETS: [&str; 2] = ["GND", "VCC"];

/// The Power class: wider default tracks and vias for routing, no
/// narrower than the showcase's thinnest supply stub (the QFN's supply
/// pin). Its clearance is the profile's: the supply stub runs 0.2 mm from
/// the QFN's next pin.
pub fn power_class() -> NetClassSpec {
    NetClassSpec {
        name: "Power".into(),
        track_width: Some(0.5),
        track_width_min: Some(0.25),
        via_drill: Some(0.4),
        via_diameter: Some(0.8),
        ..NetClassSpec::default()
    }
}

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

/// The tracks of one path, joined end to end at its corners: the round
/// end caps fill every corner.
fn tracks(net: Id, layer: CopperLayer, width: f64, points: &[Point]) -> Vec<RouteRequest> {
    points
        .windows(2)
        .map(|w| RouteRequest {
            net,
            start: w[0],
            end: w[1],
            width,
            layer,
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

/// The fill commands that add the showcase pours over `project` (a
/// bottom ground pour and the inner ground plane). Each follows the board
/// outline (`region: board`), so a board parameter or shape edit moves it
/// and the next refill fills the new shape. Their copper is computed by a
/// `fill.refill` after them.
pub fn fill_commands(project: &Project) -> Result<Vec<FillCommand>> {
    pours(project, POURS)
}

/// The hosted showcase's top GND pour, as [`fill_commands`].
pub fn top_fill_commands(project: &Project) -> Result<Vec<FillCommand>> {
    pours(project, std::slice::from_ref(&TOP_POUR))
}

fn pours(project: &Project, pours: &[(&str, &str, CopperLayer, u32)]) -> Result<Vec<FillCommand>> {
    let top = TOP_POUR.0;
    pours
        .iter()
        .map(|(name, net, layer, priority)| {
            let net = project
                .nets
                .iter()
                .find(|n| n.name == *net)
                .map(|n| n.id)
                .ok_or_else(|| Error::NotFound(format!("showcase net {net}")))?;
            Ok(FillCommand::Add {
                fill: Box::new(FillSpec {
                    region: crate::model::fill::FillRegion::Board,
                    name: (*name).into(),
                    net: Some(net),
                    layer: *layer,
                    // Derived from the board after every operation.
                    outline: Outline {
                        start: [0.0, 0.0],
                        segments: Vec::new(),
                    },
                    priority: *priority,
                    settings: if *name == top {
                        FillSettings {
                            connections: PadConnections {
                                smd: Some(PadConnection::Solid),
                                ..PadConnections::default()
                            },
                            ..FillSettings::default()
                        }
                    } else {
                        FillSettings::default()
                    },
                }),
            })
        })
        .collect()
}

/// The board edits that shape the hosted showcase's sketched board (a
/// rectangle sketch of [`BOARD_MM`], see `board.start_sketch`): first the
/// round [`CUTOUT`] (a circle inside the outline is a cutout) and the
/// [`SLOT`], then [`CORNER_MM`] fillets on every corner through the
/// `corner_r` parameter. Each inner list is one `board.edit`.
pub fn board_edits() -> Vec<Vec<BoardEdit>> {
    let (center, radius) = CUTOUT;
    let circle = SketchEdit::AddCircle {
        center,
        radius,
        construction: false,
    };
    let (a, b, width) = SLOT;
    vec![
        vec![
            BoardEdit::Sketch { edit: circle },
            BoardEdit::AddSlot {
                a,
                b,
                width: DimValue::Fixed(width),
                plated: false,
            },
        ],
        vec![
            BoardEdit::SetParameter {
                param: FootprintParam {
                    name: ParamName("corner_r".into()),
                    expr: Expr(format!("{CORNER_MM} mm")),
                    unit: ParamUnit::Mm,
                    comment: "Board corner radius".into(),
                },
            },
            BoardEdit::Fillet {
                corners: Vec::new(),
                radius: DimValue::Param(ParamName("corner_r".into())),
            },
        ],
    ]
}

/// The header J1 held where the showcase places it, [`PLACEMENTS`]' (6, 15):
/// 6 mm from the left edge and half the board height plus 1 mm above the
/// bottom edge, so it stays centred on the left edge when the `height`
/// parameter changes. Its rotation is not dimensioned, so it keeps 90°.
pub fn j1_dimensions(project: &Project) -> Result<Vec<PartDimensionSpec>> {
    let j1 = project
        .instance_by_designator("J1")
        .ok_or_else(|| Error::NotFound("showcase instance J1".into()))?;
    let BoardShape::Sketch(sketch) = &project.board_shape else {
        return Err(Error::Invalid("showcase board sketch".into()));
    };
    let edge = |name: &str| {
        sketch
            .edges
            .iter()
            .find(|e| e.name.0 == name)
            .map(|e| BoardRef::Board(SketchRef::Curve(e.curve.clone())))
            .ok_or_else(|| Error::NotFound(format!("showcase board edge {name}")))
    };
    let origin = BoardRef::Part(FeatureRef::Origin {
        target: ConstraintTarget::Placement(j1.id),
    });
    let distance = |to: BoardRef, value: DimValue| PartDimensionSpec {
        kind: PartRelation::Dim(DimKind::Distance),
        refs: vec![origin.clone(), to],
        value: Some(value),
        reference: false,
    };
    Ok(vec![
        distance(edge("left")?, DimValue::Fixed(6.0)),
        distance(
            edge("bottom")?,
            DimValue::Expr(Expr("height / 2 + 1 mm".into())),
        ),
    ])
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
