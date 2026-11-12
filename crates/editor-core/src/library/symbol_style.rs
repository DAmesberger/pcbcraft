//! Kind-specific schematic symbol drawings: original geometry following the
//! general conventions of IEC 60617 and IEEE 315 as published (a rectangle
//! or zigzag resistor, parallel capacitor plates, a triangle and bar diode,
//! …). Pure and wasm-safe: a generator returns an ordinary
//! [`SymbolDefinition`] the author then edits; nothing here is a category
//! the engine reasons about.
//!
//! Every pin's connection end lies on the 2.5 mm symbol module
//! ([`graphics::SYMBOL_MODULE_MM`]), so wires snap. Two-terminal kinds put
//! their connection ends at x = ±5 mm on y = 0 and hide pin names and
//! numbers; transistors show their numbers, because the terminal order
//! (below) is a convention to verify against the datasheet.
//!
//! Terminal order, which [`generate`] maps to `terminals` in turn:
//! - polarised capacitor: 1 = +, 2 = −;
//! - diodes and LEDs: 1 = cathode (the band end of a molded package),
//!   2 = anode;
//! - BJTs: 1 = base, 2 = emitter, 3 = collector; MOSFETs: 1 = gate,
//!   2 = source, 3 = drain (the common SOT-23 assignment);
//! - every other two-terminal kind: 1 left, 2 right.
use super::graphics::{
    self, Fill, Graphic, HAlign, Item, Justify, PIN_TEXT_HEIGHT, Pin, PinDirection, Rotation,
    SYMBOL_STROKE_MM, SYMBOL_TEXT_HEIGHT, ShapeNm, Text, TextContent, VAlign,
};
use super::{SymbolDefinition, Unit};
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, Shape};
use crate::ids::{DefinitionId, GraphicId, TerminalId, UnitId};
use crate::units::{Nm, PointNm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a generated symbol draws. A starting drawing, never an electrical
/// claim: the component's terminals and functions stay the authority.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Resistor,
    /// Non-polarised capacitor: two straight plates.
    Capacitor,
    /// Polarised capacitor: a straight + plate, a curved − plate.
    PolarisedCapacitor,
    Inductor,
    FerriteBead,
    Diode,
    Zener,
    Schottky,
    Led,
    /// Crystal or resonator.
    Crystal,
    Fuse,
    /// Single-pole switch, normally open.
    Switch,
    /// Push button, normally open.
    Pushbutton,
    /// One terminal: a probe pad.
    TestPoint,
    NpnBjt,
    PnpBjt,
    NMosfet,
    PMosfet,
    /// One terminal: the ground reference.
    Ground,
    /// One terminal: a supply rail.
    Power,
    /// A rectangle with pins down both sides (any count).
    IcBox,
    /// A narrow body with every pin down the left (any count).
    Connector,
}

impl SymbolKind {
    /// Every kind, in menu order.
    pub const ALL: [Self; 22] = [
        Self::Resistor,
        Self::Capacitor,
        Self::PolarisedCapacitor,
        Self::Inductor,
        Self::FerriteBead,
        Self::Diode,
        Self::Zener,
        Self::Schottky,
        Self::Led,
        Self::Crystal,
        Self::Fuse,
        Self::Switch,
        Self::Pushbutton,
        Self::TestPoint,
        Self::NpnBjt,
        Self::PnpBjt,
        Self::NMosfet,
        Self::PMosfet,
        Self::Ground,
        Self::Power,
        Self::IcBox,
        Self::Connector,
    ];

    /// The terminal count the drawing has pins for; `None` for the boxes,
    /// which take any count.
    pub fn terminal_count(self) -> Option<usize> {
        match self {
            Self::TestPoint | Self::Ground | Self::Power => Some(1),
            Self::NpnBjt | Self::PnpBjt | Self::NMosfet | Self::PMosfet => Some(3),
            Self::IcBox | Self::Connector => None,
            _ => Some(2),
        }
    }

    /// Whether the drawing tells its terminals apart (pin 1 matters).
    pub fn polarised(self) -> bool {
        matches!(
            self,
            Self::PolarisedCapacitor | Self::Diode | Self::Zener | Self::Schottky | Self::Led
        )
    }

    /// The customary designator prefix: a starting value the user edits.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Resistor => "R",
            Self::Capacitor | Self::PolarisedCapacitor => "C",
            Self::Inductor => "L",
            Self::FerriteBead => "FB",
            Self::Diode | Self::Zener | Self::Schottky | Self::Led => "D",
            Self::Crystal => "Y",
            Self::Fuse => "F",
            Self::Switch | Self::Pushbutton => "SW",
            Self::TestPoint => "TP",
            Self::NpnBjt | Self::PnpBjt | Self::NMosfet | Self::PMosfet => "Q",
            Self::Ground | Self::Power => "PWR",
            Self::IcBox => "U",
            Self::Connector => "J",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Resistor => "Resistor",
            Self::Capacitor => "Capacitor",
            Self::PolarisedCapacitor => "Polarised capacitor",
            Self::Inductor => "Inductor",
            Self::FerriteBead => "Ferrite bead",
            Self::Diode => "Diode",
            Self::Zener => "Zener diode",
            Self::Schottky => "Schottky diode",
            Self::Led => "LED",
            Self::Crystal => "Crystal",
            Self::Fuse => "Fuse",
            Self::Switch => "Switch",
            Self::Pushbutton => "Push button",
            Self::TestPoint => "Test point",
            Self::NpnBjt => "NPN transistor",
            Self::PnpBjt => "PNP transistor",
            Self::NMosfet => "N-channel MOSFET",
            Self::PMosfet => "P-channel MOSFET",
            Self::Ground => "Ground",
            Self::Power => "Power",
            Self::IcBox => "IC box",
            Self::Connector => "Connector",
        }
    }

    /// The two-terminal kind a designator prefix customarily names
    /// (case-insensitive), polarised or not; `None` for any other prefix.
    /// A presentation default the request can override, never a check.
    pub fn for_prefix(prefix: &str, polarised: bool) -> Option<Self> {
        Some(match prefix.trim().to_ascii_uppercase().as_str() {
            "R" | "RV" => Self::Resistor,
            "C" if polarised => Self::PolarisedCapacitor,
            "C" => Self::Capacitor,
            "L" => Self::Inductor,
            "FB" | "FL" => Self::FerriteBead,
            "D" | "CR" => Self::Diode,
            "LED" => Self::Led,
            "Z" | "ZD" => Self::Zener,
            "Y" | "X" | "XTAL" => Self::Crystal,
            "F" => Self::Fuse,
            "SW" | "S" => Self::Switch,
            _ => return None,
        })
    }
}

/// How resistors are drawn: the IEC 60617 rectangle or the IEEE 315
/// zigzag.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ResistorStyle {
    #[default]
    Iec,
    Us,
}

impl ResistorStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Iec => "IEC (rectangle)",
            Self::Us => "US (zigzag)",
        }
    }
}

/// A project's symbol drawing conventions: an authoring setting saved with
/// the drafts (`library.set_symbol_conventions`) that new generated symbols
/// follow. Existing symbols never change.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SymbolConventions {
    #[serde(default)]
    pub resistor: ResistorStyle,
}

impl SymbolConventions {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The two connection ends of a horizontal two-terminal symbol.
pub const TWO_TERMINAL_PIN_X_MM: f64 = 2.0 * graphics::SYMBOL_MODULE_MM;

/// Redraw `symbol` as `kind`, with its pins on `terminals` in the order the
/// module documents (`library.restyle_symbol`). The unit keeps its identity
/// and label, so placed instances still name it, and shown field text stays
/// where it was; every other graphic and pin is replaced. New graphics take
/// fresh IDs: a retired one is never reused. A symbol of several units, or a
/// terminal count the drawing does not have, is refused.
pub fn restyle(
    symbol: &SymbolDefinition,
    kind: SymbolKind,
    conventions: SymbolConventions,
    terminals: &[TerminalId],
) -> Result<SymbolDefinition> {
    let [unit] = symbol.units.as_slice() else {
        return Err(Error::Invalid(format!(
            "symbol.restyle_units: a {} symbol draws one unit; this symbol has {}",
            kind.label().to_lowercase(),
            symbol.units.len()
        )));
    };
    let mut drawn = match kind {
        SymbolKind::IcBox => super::samples::block_symbol(symbol.component, terminals)?,
        SymbolKind::Connector => super::samples::column_symbol(symbol.component, terminals)?,
        _ => generate(kind, conventions, symbol.component, terminals)?,
    };
    let exhausted = || Error::Limit("the symbol's local identities are exhausted".into());
    let base = symbol.next_local.max(unit.id.0.saturating_add(1));
    let next_local = base.checked_add(drawn.next_local).ok_or_else(exhausted)?;
    let Some(fresh) = drawn.units.pop() else {
        return Err(Error::Invalid(format!(
            "symbol.kind: the {} drawing has no unit",
            kind.label()
        )));
    };
    let mut graphics: Vec<Graphic> = unit
        .graphics
        .iter()
        .filter(|g| matches!(&g.item, Item::Text(t) if matches!(t.content, TextContent::Field(_))))
        .cloned()
        .collect();
    for mut graphic in fresh.graphics {
        graphic.id = GraphicId(base.checked_add(graphic.id.0).ok_or_else(exhausted)?);
        graphics.push(graphic);
    }
    Ok(SymbolDefinition {
        component: symbol.component,
        next_local,
        pin_text_height_nm: symbol.pin_text_height_nm,
        units: vec![Unit {
            id: unit.id,
            label: unit.label.clone(),
            graphics,
            pins: fresh.pins,
        }],
    })
}

/// The symbol of `kind` for `component`, its pins on `terminals` in the
/// order the module documents. One unit `A` with local ID 1, graphics from
/// ID 2 (a resistor's IEC rectangle is the body graphic, ID 2), the
/// designator and component name as text. Refuses the box kinds (the
/// samples lay those out) and a terminal count the drawing does not have.
pub fn generate(
    kind: SymbolKind,
    conventions: SymbolConventions,
    component: DefinitionId,
    terminals: &[TerminalId],
) -> Result<SymbolDefinition> {
    let Some(count) = kind.terminal_count() else {
        return Err(Error::Invalid(format!(
            "symbol.kind: the {} layout depends on the package; it is not drawn here",
            kind.label()
        )));
    };
    if terminals.len() != count {
        return Err(Error::Invalid(format!(
            "template.symbol: a {} symbol needs {count} terminal{}; this component has {}",
            kind.label().to_lowercase(),
            if count == 1 { "" } else { "s" },
            terminals.len()
        )));
    }
    let mut d = Drawing::default();
    let t = |k: usize| terminals.get(k).copied().unwrap_or(TerminalId(0));
    match kind {
        SymbolKind::Resistor => {
            d.two_pins(t(0), t(1), 2.5, 2.5);
            match conventions.resistor {
                ResistorStyle::Iec => d.shape(
                    Shape::rect([-2.5, -1.0], [2.5, 1.0]),
                    SYMBOL_STROKE_MM,
                    Fill::Body,
                ),
                ResistorStyle::Us => {
                    let mut points = vec![[-2.5, 0.0]];
                    for k in 0..6 {
                        let x = -2.5 + 5.0 * (2 * k + 1) as f64 / 12.0;
                        points.push([x, if k % 2 == 0 { 1.0 } else { -1.0 }]);
                    }
                    points.push([2.5, 0.0]);
                    d.open(Shape::polyline(points));
                }
            }
            d.labels(1.0, -1.0);
        }
        SymbolKind::Capacitor => {
            d.two_pins(t(0), t(1), 0.5, 0.5);
            d.plate([-0.5, -1.75], [-0.5, 1.75]);
            d.plate([0.5, -1.75], [0.5, 1.75]);
            d.labels(1.75, -1.75);
        }
        SymbolKind::PolarisedCapacitor => {
            d.two_pins(t(0), t(1), 0.5, 0.5);
            d.plate([-0.5, -1.75], [-0.5, 1.75]);
            // The − plate curves away from the + plate: an arc through
            // (0.5, 0) whose ends reach ±1.75 mm.
            let radius: f64 = 2.5;
            let half = (1.75 / radius).asin().to_degrees();
            d.shape(
                Shape::Arc {
                    center: [0.5 + radius, 0.0],
                    radius,
                    start_deg: 180.0 - half,
                    sweep_deg: 2.0 * half,
                },
                0.35,
                Fill::None,
            );
            // The + mark beside the + plate.
            d.line([-2.25, 1.5], [-1.25, 1.5]);
            d.line([-1.75, 1.0], [-1.75, 2.0]);
            d.labels(2.0, -1.75);
        }
        SymbolKind::Inductor => {
            d.two_pins(t(0), t(1), 2.5, 2.5);
            for k in 0..4 {
                d.open(Shape::Arc {
                    center: [-1.875 + 1.25 * k as f64, 0.0],
                    radius: 0.625,
                    start_deg: 180.0,
                    sweep_deg: -180.0,
                });
            }
            d.labels(0.625, -0.5);
        }
        SymbolKind::FerriteBead => {
            d.two_pins(t(0), t(1), 2.5, 2.5);
            d.line([-2.5, 0.0], [2.5, 0.0]);
            // A slanted solid block across the line.
            let (c, s) = (
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            );
            let corner = |u: f64, v: f64| [u * c - v * s, u * s + v * c];
            d.shape(
                Shape::polygon(vec![
                    corner(-0.6, -1.6),
                    corner(0.6, -1.6),
                    corner(0.6, 1.6),
                    corner(-0.6, 1.6),
                ]),
                SYMBOL_STROKE_MM,
                Fill::Solid,
            );
            d.labels(1.6, -1.6);
        }
        SymbolKind::Diode | SymbolKind::Zener | SymbolKind::Schottky | SymbolKind::Led => {
            // Anode (terminal 2) left, cathode (terminal 1) right.
            d.pin(
                t(1),
                [-TWO_TERMINAL_PIN_X_MM, 0.0],
                PinDirection::Right,
                3.75,
            );
            d.pin(t(0), [TWO_TERMINAL_PIN_X_MM, 0.0], PinDirection::Left, 3.75);
            d.shape(
                Shape::polygon(vec![[-1.25, 1.25], [-1.25, -1.25], [1.25, 0.0]]),
                SYMBOL_STROKE_MM,
                Fill::Body,
            );
            match kind {
                SymbolKind::Zener => d.open(Shape::polyline(vec![
                    [0.75, -1.6],
                    [1.25, -1.25],
                    [1.25, 1.25],
                    [1.75, 1.6],
                ])),
                SymbolKind::Schottky => d.open(Shape::polyline(vec![
                    [0.75, -0.85],
                    [0.75, -1.25],
                    [1.25, -1.25],
                    [1.25, 1.25],
                    [1.75, 1.25],
                    [1.75, 0.85],
                ])),
                _ => d.line([1.25, -1.25], [1.25, 1.25]),
            }
            if kind == SymbolKind::Led {
                // Two arrows leaving the junction, up and to the right.
                for dx in [-0.5, 0.5] {
                    d.arrow([dx - 0.25, 1.25], [dx + 0.75, 2.25]);
                }
                d.labels(2.5, -1.25);
            } else {
                d.labels(1.6, -1.6);
            }
        }
        SymbolKind::Crystal => {
            d.two_pins(t(0), t(1), 1.25, 1.25);
            d.plate([-1.25, -1.25], [-1.25, 1.25]);
            d.plate([1.25, -1.25], [1.25, 1.25]);
            d.shape(
                Shape::rect([-0.6, -1.5], [0.6, 1.5]),
                SYMBOL_STROKE_MM,
                Fill::Body,
            );
            d.labels(1.5, -1.5);
        }
        SymbolKind::Fuse => {
            d.two_pins(t(0), t(1), 2.5, 2.5);
            d.shape(
                Shape::rect([-2.5, -0.875], [2.5, 0.875]),
                SYMBOL_STROKE_MM,
                Fill::Body,
            );
            d.line([-2.5, 0.0], [2.5, 0.0]);
            d.labels(0.875, -0.875);
        }
        SymbolKind::Switch => {
            d.two_pins(t(0), t(1), 2.5, 2.5);
            d.contact([-2.5, 0.0]);
            d.contact([2.5, 0.0]);
            d.line([-2.15, 0.2], [2.25, 1.6]);
            d.labels(1.75, -0.5);
        }
        SymbolKind::Pushbutton => {
            d.two_pins(t(0), t(1), 2.5, 2.5);
            d.contact([-2.5, 0.0]);
            d.contact([2.5, 0.0]);
            d.line([-2.75, 1.0], [2.75, 1.0]);
            d.line([0.0, 1.0], [0.0, 2.25]);
            d.line([-0.75, 2.25], [0.75, 2.25]);
            d.labels(2.25, -0.5);
        }
        SymbolKind::TestPoint => {
            d.pin(t(0), [0.0, -2.5], PinDirection::Up, 1.25);
            d.shape(
                Shape::Circle {
                    center: [0.0, -0.5],
                    radius: 0.75,
                },
                SYMBOL_STROKE_MM,
                Fill::Body,
            );
            d.side_labels([1.25, -0.5]);
        }
        SymbolKind::Ground => {
            d.pin(t(0), [0.0, 0.0], PinDirection::Down, 2.5);
            d.line([-1.5, -2.5], [1.5, -2.5]);
            d.line([-1.0, -3.1], [1.0, -3.1]);
            d.line([-0.5, -3.7], [0.5, -3.7]);
            d.rail_labels(-3.7);
        }
        SymbolKind::Power => {
            d.pin(t(0), [0.0, 0.0], PinDirection::Up, 2.5);
            d.line([-1.5, 2.5], [1.5, 2.5]);
            d.rail_labels(2.5);
        }
        SymbolKind::NpnBjt | SymbolKind::PnpBjt => {
            d.numbered = true;
            d.pin(t(0), [-5.0, 0.0], PinDirection::Right, 3.75);
            d.pin(t(1), [2.5, -5.0], PinDirection::Up, 2.5);
            d.pin(t(2), [2.5, 5.0], PinDirection::Down, 2.5);
            d.plate([-1.25, -1.5], [-1.25, 1.5]);
            d.line([-1.25, 0.75], [2.5, 2.5]);
            let (base, emitter) = ([-1.25, -0.75], [2.5, -2.5]);
            d.line(base, emitter);
            let along = |t: f64| {
                [
                    base[0] + (emitter[0] - base[0]) * t,
                    base[1] + (emitter[1] - base[1]) * t,
                ]
            };
            if kind == SymbolKind::NpnBjt {
                d.arrow_head(along(0.45), along(0.8));
            } else {
                d.arrow_head(along(0.75), along(0.4));
            }
            d.shape(
                Shape::Circle {
                    center: [0.75, 0.0],
                    radius: 3.25,
                },
                SYMBOL_STROKE_MM,
                Fill::None,
            );
            d.side_labels([4.5, 0.0]);
        }
        SymbolKind::NMosfet | SymbolKind::PMosfet => {
            d.numbered = true;
            d.pin(t(0), [-5.0, 0.0], PinDirection::Right, 3.5);
            d.pin(t(1), [2.5, -5.0], PinDirection::Up, 3.5);
            d.pin(t(2), [2.5, 5.0], PinDirection::Down, 3.5);
            // Gate electrode, then the broken enhancement channel.
            d.line([-1.5, -1.75], [-1.5, 1.75]);
            for (a, b) in [(1.0, 2.0), (-0.5, 0.5), (-2.0, -1.0)] {
                d.plate([-0.75, a], [-0.75, b]);
            }
            d.line([-0.75, 1.5], [2.5, 1.5]);
            d.line([-0.75, -1.5], [2.5, -1.5]);
            // The body joins the source.
            d.line([-0.75, 0.0], [2.5, 0.0]);
            d.line([2.5, 0.0], [2.5, -1.5]);
            if kind == SymbolKind::NMosfet {
                d.arrow_head([0.75, 0.0], [-0.6, 0.0]);
            } else {
                d.arrow_head([-0.6, 0.0], [0.75, 0.0]);
            }
            d.side_labels([4.0, 0.0]);
        }
        SymbolKind::IcBox | SymbolKind::Connector => {}
    }
    d.finish(component)
}

/// Collects one unit's graphics and pins with sequential local IDs. It
/// draws in millimetres and stores each item rounded to nanometres; the
/// first value that cannot be stored fails [`Drawing::finish`].
struct Drawing {
    graphics: Vec<Graphic>,
    pins: Vec<Pin>,
    next: u32,
    /// Pins show their numbers (transistors); two-terminal parts and rails
    /// hide them.
    numbered: bool,
    error: Option<Error>,
}

impl Default for Drawing {
    fn default() -> Self {
        Self {
            graphics: Vec::new(),
            pins: Vec::new(),
            // Local ID 1 is the unit.
            next: 2,
            numbered: false,
            error: None,
        }
    }
}

/// The stroke of capacitor and crystal plates and the BJT base bar.
const PLATE_STROKE_MM: f64 = 0.35;
/// Arrow head length and half width.
const ARROW_MM: [f64; 2] = [0.6, 0.3];

impl Drawing {
    fn push(&mut self, item: Item) {
        self.graphics.push(Graphic {
            id: GraphicId(self.next),
            item,
        });
        self.next += 1;
    }

    /// `value` stored, or the first error kept for [`Drawing::finish`].
    fn stored<T>(&mut self, value: Result<T>) -> Option<T> {
        match value {
            Ok(v) => Some(v),
            Err(e) => {
                self.error.get_or_insert(e);
                None
            }
        }
    }

    fn shape(&mut self, shape: Shape, width: f64, fill: Fill) {
        let stored = ShapeNm::from_mm_rounded(&shape)
            .and_then(|shape| Ok((shape, Nm::from_mm_f64_rounded(width)?)));
        if let Some((shape, width_nm)) = self.stored(stored) {
            self.push(Item::Shape {
                shape,
                width_nm,
                fill,
            });
        }
    }

    fn open(&mut self, shape: Shape) {
        self.shape(shape, SYMBOL_STROKE_MM, Fill::None);
    }

    fn line(&mut self, a: Point, b: Point) {
        self.open(Shape::Line { a, b });
    }

    fn plate(&mut self, a: Point, b: Point) {
        self.shape(Shape::Line { a, b }, PLATE_STROKE_MM, Fill::None);
    }

    /// A small open contact circle at a switch terminal.
    fn contact(&mut self, center: Point) {
        self.shape(
            Shape::Circle {
                center,
                radius: 0.3,
            },
            SYMBOL_STROKE_MM,
            Fill::Body,
        );
    }

    /// A solid arrow head with its tip at `tip`, pointing away from `from`.
    fn arrow_head(&mut self, from: Point, tip: Point) {
        let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
        let length = dx.hypot(dy);
        if length <= f64::EPSILON {
            return;
        }
        let (ux, uy) = (dx / length, dy / length);
        let [l, w] = ARROW_MM;
        let back = [tip[0] - ux * l, tip[1] - uy * l];
        self.shape(
            Shape::polygon(vec![
                tip,
                [back[0] - uy * w, back[1] + ux * w],
                [back[0] + uy * w, back[1] - ux * w],
            ]),
            0.1,
            Fill::Solid,
        );
    }

    /// A shaft from `from` to `tip` with a solid head at `tip`.
    fn arrow(&mut self, from: Point, tip: Point) {
        let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
        let length = dx.hypot(dy);
        if length <= ARROW_MM[0] {
            return;
        }
        let shaft = 1.0 - ARROW_MM[0] / length;
        self.line(from, [from[0] + dx * shaft, from[1] + dy * shaft]);
        self.arrow_head(from, tip);
    }

    fn pin(&mut self, terminal: TerminalId, position: Point, direction: PinDirection, length: f64) {
        let stored = PointNm::from_mm_f64_rounded(position)
            .and_then(|p| Ok((p, Nm::from_mm_f64_rounded(length)?)));
        if let Some((position, length_nm)) = self.stored(stored) {
            let mut pin = Pin::new(terminal, position, direction, length_nm);
            pin.name.visible = false;
            pin.number.visible = self.numbered;
            self.pins.push(pin);
        }
    }

    /// Terminal `a` at x = −5 and `b` at x = +5 on y = 0, their stubs
    /// reaching the body at −`left` and +`right`.
    fn two_pins(&mut self, a: TerminalId, b: TerminalId, left: f64, right: f64) {
        let x = TWO_TERMINAL_PIN_X_MM;
        self.pin(a, [-x, 0.0], PinDirection::Right, x - left);
        self.pin(b, [x, 0.0], PinDirection::Left, x - right);
    }

    fn text(&mut self, content: TextContent, at: Point, h: HAlign, v: VAlign, visible: bool) {
        let Some(at) = self.stored(PointNm::from_mm_f64_rounded(at)) else {
            return;
        };
        self.push(Item::Text(Text {
            content,
            at,
            height_nm: SYMBOL_TEXT_HEIGHT,
            rotation: Rotation::R0,
            justify: Justify { h, v },
            visible,
            line_spacing: graphics::TEXT_LINE_SPACING,
        }));
    }

    /// The designator above the drawing's `top` and the name below its
    /// `bottom`, centred.
    fn labels(&mut self, top: f64, bottom: f64) {
        let gap = 0.5;
        self.text(
            TextContent::Designator,
            [0.0, top + gap],
            HAlign::Center,
            VAlign::Bottom,
            true,
        );
        self.text(
            TextContent::ComponentName,
            [0.0, bottom - gap],
            HAlign::Center,
            VAlign::Top,
            true,
        );
    }

    /// The designator and name stacked, left-aligned, right of `at`.
    fn side_labels(&mut self, at: Point) {
        let gap = 0.25;
        self.text(
            TextContent::Designator,
            [at[0], at[1] + gap],
            HAlign::Left,
            VAlign::Bottom,
            true,
        );
        self.text(
            TextContent::ComponentName,
            [at[0], at[1] - gap],
            HAlign::Left,
            VAlign::Top,
            true,
        );
    }

    /// A rail shows its name beyond the bar at `y`; the designator is kept
    /// but hidden.
    fn rail_labels(&mut self, y: f64) {
        let gap = 0.5;
        let (at, v) = if y < 0.0 {
            ([0.0, y - gap], VAlign::Top)
        } else {
            ([0.0, y + gap], VAlign::Bottom)
        };
        self.text(TextContent::ComponentName, at, HAlign::Center, v, true);
        self.text(
            TextContent::Designator,
            [1.75, 0.0],
            HAlign::Left,
            VAlign::Middle,
            false,
        );
    }

    fn finish(self, component: DefinitionId) -> Result<SymbolDefinition> {
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(SymbolDefinition {
            component,
            next_local: self.next,
            pin_text_height_nm: PIN_TEXT_HEIGHT,
            units: vec![Unit {
                id: UnitId(1),
                label: "A".into(),
                graphics: self.graphics,
                pins: self.pins,
            }],
        })
    }
}

#[cfg(test)]
mod tests;
