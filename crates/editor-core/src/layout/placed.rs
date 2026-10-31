//! Footprint content in board coordinates: the one placement transform and
//! the placed pad and graphic queries every view and check reads.
use crate::geometry::{Point, Shape, Transform2, cos_sin};
use crate::ids::{Endpoint, GraphicId, Id};
use crate::instances;
use crate::library::graphics::{
    DrawingLayer, Fill, Item, PlacedText, Side, TextContent, TextMetric,
};
use crate::library::{Datum, PAD_ARC_TOLERANCE_MM, Pad, PadKind, PadSide};
use crate::model::{Layer, Placement, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Most stored graphic vertices placed on one board (a target). Enforced by
/// `layout::validate`, so [`placed_graphics`] stays bounded on a valid
/// document.
pub const MAX_PLACED_GRAPHIC_VERTICES: usize = 262_144;

/// Footprint-local to board coordinates for one placement: move the package
/// datum to the origin and undo its rotation, reflect across the x-axis on
/// the bottom (rigid Rx(pi)), rotate by the placement, then move to the
/// placement position.
pub fn placement_transform(placement: &Placement, datum: &Datum) -> Transform2 {
    let to_datum =
        Transform2::translation([-datum.offset[0], -datum.offset[1]]).then(&Transform2 {
            translate: [0.0, 0.0],
            rotation_deg: -datum.rotation_deg,
            mirror: false,
        });
    to_datum.then(&Transform2 {
        translate: placement.position,
        rotation_deg: placement.rotation_deg,
        mirror: placement.layer == Layer::Bottom,
    })
}

/// Undo the datum rotation about the datum (identity at 0°).
pub(super) fn undo_datum_rotation(p: Point, datum: &Datum) -> Point {
    if datum.rotation_deg == 0.0 {
        return p;
    }
    let (c, s) = cos_sin(-datum.rotation_deg);
    let d = [p[0] - datum.offset[0], p[1] - datum.offset[1]];
    [
        datum.offset[0] + c * d[0] - s * d[1],
        datum.offset[1] + s * d[0] + c * d[1],
    ]
}

/// One placed pad: the footprint-local pad and its transform to the board.
/// Unconnected and terminal-less pads are included.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacedPad {
    pub instance: Id,
    /// The pad as stored, footprint-local.
    pub pad: Pad,
    /// Footprint-local to board coordinates.
    pub transform: Transform2,
    /// The copper layer of a surface pad after the side flip; a
    /// through-hole pad records the placement layer and spans all layers.
    pub layer: Layer,
    pub terminal: Option<Endpoint>,
    pub net: Option<Id>,
}

/// Every pad of every placed part, in placement then pad order.
pub fn placed_pads(project: &Project) -> Vec<PlacedPad> {
    let nets = net_by_terminal(project);
    let mut result = Vec::new();
    for placement in &project.placements {
        let Some(instance) = project.instance(placement.instance) else {
            continue;
        };
        let Some(physical) = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
        else {
            continue;
        };
        let footprint = &physical.footprint.definition;
        let transform = placement_transform(placement, &footprint.datum);
        for pad in &footprint.pads {
            let terminal = pad
                .terminal
                .and_then(|t| physical.binding.definition.terminal_of(t))
                .map(|terminal| Endpoint {
                    instance: instance.id,
                    terminal,
                });
            let layer = match pad.side {
                PadSide::Top => placement.layer,
                PadSide::Bottom => opposite(placement.layer),
            };
            result.push(PlacedPad {
                instance: instance.id,
                pad: pad.clone(),
                transform,
                layer,
                net: terminal.and_then(|t| nets.get(&t).copied()),
                terminal,
            });
        }
    }
    result
}

/// The board layer a footprint graphic is drawn on.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoardDrawing {
    Silkscreen(Layer),
    Courtyard(Layer),
    Assembly(Layer),
}

/// A footprint graphic's content in board coordinates.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PlacedItem {
    Shape {
        shape: Shape,
        width: f64,
        filled: bool,
    },
    Text(PlacedText),
}

/// One footprint graphic of one placed part.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacedGraphic {
    pub instance: Id,
    pub graphic: GraphicId,
    pub layer: BoardDrawing,
    pub item: PlacedItem,
}

/// Every footprint graphic of every placed part, including parts without
/// nets. `Side::Component` maps to the placement layer; on the bottom, text
/// is marked mirrored and arcs reverse (through `Shape::transformed`).
/// Designator text shows the instance designator and literal text itself;
/// other text content resolves to empty text for now. Hidden text is left
/// out.
pub fn placed_graphics(project: &Project) -> Vec<PlacedGraphic> {
    let mut result = Vec::new();
    for placement in &project.placements {
        let Some(instance) = project.instance(placement.instance) else {
            continue;
        };
        let Some(physical) = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
        else {
            continue;
        };
        let footprint = &physical.footprint.definition;
        let t = placement_transform(placement, &footprint.datum);
        for graphic in &footprint.graphics {
            let side = match graphic.side {
                Side::Component => placement.layer,
                Side::Opposite => opposite(placement.layer),
            };
            let layer = match graphic.layer {
                DrawingLayer::Silkscreen => BoardDrawing::Silkscreen(side),
                DrawingLayer::Courtyard => BoardDrawing::Courtyard(side),
                DrawingLayer::Assembly => BoardDrawing::Assembly(side),
            };
            let item = match &graphic.item {
                Item::Shape { shape, width, fill } => PlacedItem::Shape {
                    shape: shape.transformed(&t),
                    width: *width,
                    filled: *fill != Fill::None,
                },
                Item::Text(text) => {
                    if !text.visible {
                        continue;
                    }
                    let content = match &text.content {
                        TextContent::Literal(s) => s.clone(),
                        TextContent::Designator => instance.designator.clone(),
                        _ => String::new(),
                    };
                    PlacedItem::Text(PlacedText {
                        graphic: Some(graphic.id),
                        text: content,
                        at: t.apply(text.at),
                        height: text.height,
                        rotation_deg: t.apply_angle(text.rotation.degrees()).rem_euclid(360.0),
                        justify: text.justify,
                        mirrored: t.mirror,
                        line_spacing: text.line_spacing,
                    })
                }
            };
            result.push(PlacedGraphic {
                instance: instance.id,
                graphic: graphic.id,
                layer,
                item,
            });
        }
    }
    result
}

pub(crate) fn opposite(layer: Layer) -> Layer {
    match layer {
        Layer::Top => Layer::Bottom,
        Layer::Bottom => Layer::Top,
    }
}

fn net_by_terminal(project: &Project) -> BTreeMap<Endpoint, Id> {
    project
        .nets
        .iter()
        .flat_map(|net| {
            net.terminals
                .iter()
                .map(move |terminal| (*terminal, net.id))
        })
        .collect()
}

/// A drilled hole of a placed pad in board coordinates.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacedHole {
    /// The drill outline: a circle, or a stadium for a slot.
    pub shape: Shape,
    pub plated: bool,
}

impl PlacedPad {
    /// The pad origin (shape centre) on the board.
    pub fn center(&self) -> Point {
        self.transform.apply(self.pad.position)
    }

    /// Pad frame to board coordinates.
    pub fn pad_transform(&self) -> Transform2 {
        self.pad.frame().then(&self.transform)
    }

    /// Whether the pad has copper on `layer`: plated through-hole pads on
    /// every layer, surface kinds on their own, a bare hole on none.
    pub fn on_copper(&self, layer: Layer) -> bool {
        match self.pad.kind {
            PadKind::PlatedThrough => true,
            PadKind::NonPlatedHole => false,
            PadKind::Smd | PadKind::Fiducial | PadKind::Mechanical => self.layer == layer,
        }
    }

    /// The copper (or, for a bare hole, keep-out) outline on the board.
    pub fn outline(&self) -> Shape {
        self.pad
            .shape
            .outline(PAD_ARC_TOLERANCE_MM)
            .transformed(&self.pad_transform())
    }

    /// The pad's drilled holes on the board: its own hole, then its vias.
    pub fn holes(&self) -> Vec<PlacedHole> {
        let t = self.pad_transform();
        self.pad
            .hole
            .iter()
            .map(|hole| PlacedHole {
                shape: hole.shape().transformed(&t),
                plated: hole.plated,
            })
            .chain(self.pad.vias.iter().map(|via| PlacedHole {
                shape: Shape::Circle {
                    center: t.apply(via.at),
                    radius: via.drill / 2.0,
                },
                plated: true,
            }))
            .collect()
    }

    /// Whether `p` (board coordinates) lies on the pad's outline area.
    pub fn contains(&self, p: Point) -> bool {
        self.outline().distance(p, true) <= 0.0
    }
}

/// The topmost (last drawn) pad under `p` among the pads `visible` accepts.
pub fn pick_pad(
    pads: &[PlacedPad],
    p: Point,
    visible: impl Fn(&PlacedPad) -> bool,
) -> Option<&PlacedPad> {
    pads.iter()
        .rev()
        .find(|pad| visible(pad) && pad.contains(p))
}

/// The topmost (last drawn) graphic within `tolerance` mm of `p` among the
/// graphics `visible` accepts. Strokes pick within half their width, filled
/// shapes inside, text inside its box under `metric`.
pub fn pick_graphic(
    graphics: &[PlacedGraphic],
    p: Point,
    tolerance: f64,
    metric: TextMetric,
    visible: impl Fn(&PlacedGraphic) -> bool,
) -> Option<&PlacedGraphic> {
    graphics.iter().rev().find(|g| {
        visible(g)
            && match &g.item {
                PlacedItem::Shape {
                    shape,
                    width,
                    filled,
                } => shape.distance(p, *filled) <= width / 2.0 + tolerance,
                PlacedItem::Text(text) => text.bounds(metric).inflated(tolerance).contains(p),
            }
    })
}

#[cfg(test)]
mod tests;
