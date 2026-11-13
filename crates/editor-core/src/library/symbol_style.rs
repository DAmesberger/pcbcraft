//! Kind-specific schematic symbol drawings: original geometry following the
//! general conventions of IEC 60617 and IEEE 315 as published (a rectangle
//! or zigzag resistor, parallel capacitor plates, a triangle and bar diode,
//! …). The shapes and their coordinates are data, one file per kind in the
//! shipped library's `standards/symbols/` ([`SymbolDrawings`]); this module
//! draws them. Pure and wasm-safe: a generator returns an ordinary
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
use super::standards::{self, check_format};
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

/// The shared sizes of generated symbols (`standards/symbols/style.json`),
/// in millimetres.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SymbolStyle {
    pub format: u32,
    #[serde(default)]
    pub about: String,
    /// The stroke of capacitor and crystal plates and a transistor's bars.
    pub plate_stroke_mm: f64,
    /// Arrow head length and half width.
    pub arrow_mm: [f64; 2],
    /// The outline stroke of a solid arrow head.
    pub arrow_stroke_mm: f64,
    /// The radius of a switch's open contact.
    pub contact_radius_mm: f64,
    /// The gap between a drawing and its centred labels, and between a
    /// rail's bar and its name.
    pub label_gap_mm: f64,
    /// Half the gap between stacked side labels.
    pub side_label_gap_mm: f64,
    /// Where a rail keeps its hidden designator.
    pub rail_designator_mm: Point,
}

/// One step of a kind's drawing, in millimetres. Terminals count from 1 in
/// the order the module documents.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum DrawOp {
    /// Terminal 1 at x = −5 and 2 at x = +5 on y = 0, their stubs reaching
    /// the body at −`left` and +`right`.
    TwoPins { left: f64, right: f64 },
    /// One pin: its connection end `at`, pointing into the body.
    Pin {
        terminal: usize,
        at: Point,
        direction: PinDirection,
        length: f64,
    },
    /// A shape with the symbol stroke unless `width` says otherwise.
    Shape {
        shape: Shape,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
        #[serde(default)]
        fill: Fill,
    },
    /// A straight line with the symbol stroke.
    Line { a: Point, b: Point },
    /// A straight line with the plate stroke.
    Plate { a: Point, b: Point },
    /// A small open contact circle at a switch terminal.
    Contact { center: Point },
    /// A solid arrow head with its tip at `tip`, pointing away from `from`.
    ArrowHead { from: Point, tip: Point },
    /// A shaft from `from` to `tip` with a solid head at `tip`.
    Arrow { from: Point, tip: Point },
    /// The designator above the drawing's `top` and the name below its
    /// `bottom`, centred.
    Labels { top: f64, bottom: f64 },
    /// The designator and name stacked, left-aligned, right of `at`.
    SideLabels { at: Point },
    /// A rail's name beyond its bar at `y`; the designator kept but hidden.
    RailLabels { y: f64 },
    /// Steps drawn only under one resistor convention.
    Styled {
        resistor: ResistorStyle,
        ops: Vec<DrawOp>,
    },
}

/// One kind's drawing (`standards/symbols/<kind>.json`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KindDrawing {
    pub format: u32,
    pub kind: SymbolKind,
    /// The published convention the drawing follows.
    #[serde(default)]
    pub convention: String,
    #[serde(default)]
    pub about: String,
    /// Pins show their numbers (transistors); two-terminal parts and rails
    /// hide them.
    #[serde(default)]
    pub numbered: bool,
    pub ops: Vec<DrawOp>,
}

/// Steps of one kind's drawing, nested ones included.
const MAX_OPS: usize = 64;

/// Every drawn kind's drawing and the shared style, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolDrawings {
    style: SymbolStyle,
    kinds: Vec<KindDrawing>,
}

fn finite(point: Point) -> bool {
    point.iter().all(|v| v.is_finite())
}

impl SymbolStyle {
    /// Finite sizes; strokes, arrow and contact positive.
    pub fn check(&self) -> Result<()> {
        check_format(self.format, "the symbol style")?;
        let sizes = [
            self.plate_stroke_mm,
            self.arrow_mm[0],
            self.arrow_mm[1],
            self.arrow_stroke_mm,
            self.contact_radius_mm,
        ];
        if sizes.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || !self.label_gap_mm.is_finite()
            || !self.side_label_gap_mm.is_finite()
            || !finite(self.rail_designator_mm)
        {
            return Err(Error::Invalid(
                "the symbol style needs finite, positive sizes".into(),
            ));
        }
        Ok(())
    }
}

impl KindDrawing {
    /// A drawn kind (not a box), bounded steps of finite sizes, no nested
    /// `styled` step, and one pin for each terminal under every resistor
    /// convention.
    pub fn check(&self) -> Result<()> {
        let Some(count) = self.kind.terminal_count() else {
            return Err(Error::Invalid(format!(
                "the {} is laid out from its pins; it takes no drawing",
                self.kind.label()
            )));
        };
        check_drawing(self, count)
    }
}

impl SymbolDrawings {
    /// The drawings, after the style's and each drawing's own check, with
    /// exactly one drawing of each drawn kind (every kind but the boxes).
    pub fn new(style: SymbolStyle, kinds: Vec<KindDrawing>) -> Result<Self> {
        style.check()?;
        for drawing in &kinds {
            drawing.check()?;
        }
        for kind in SymbolKind::ALL
            .into_iter()
            .filter(|k| k.terminal_count().is_some())
        {
            if kinds.iter().filter(|k| k.kind == kind).count() != 1 {
                return Err(Error::Invalid(format!(
                    "the symbol drawings need exactly one {} drawing",
                    kind.label()
                )));
            }
        }
        Ok(Self { style, kinds })
    }

    /// The shared sizes.
    pub fn style(&self) -> &SymbolStyle {
        &self.style
    }

    /// The drawing of `kind`; `None` for the boxes.
    pub fn drawing(&self, kind: SymbolKind) -> Option<&KindDrawing> {
        self.kinds.iter().find(|k| k.kind == kind)
    }
}

/// One drawing's checks: format, bounded steps, finite sizes, no nested
/// `styled` step, and each terminal 1..=`count` pinned under every
/// resistor convention.
fn check_drawing(drawing: &KindDrawing, count: usize) -> Result<()> {
    let what = drawing.kind.label();
    check_format(drawing.format, &format!("the {what} drawing"))?;
    let invalid = |why: &str| Error::Invalid(format!("the {what} drawing: {why}"));
    let mut total = 0usize;
    for op in &drawing.ops {
        total += 1;
        if let DrawOp::Styled { ops, .. } = op {
            total += ops.len();
            if ops.iter().any(|o| matches!(o, DrawOp::Styled { .. })) {
                return Err(invalid("a styled step cannot nest"));
            }
        }
    }
    if total > MAX_OPS {
        return Err(invalid(&format!("more than {MAX_OPS} steps")));
    }
    let all = drawing.ops.iter().flat_map(|op| match op {
        DrawOp::Styled { ops, .. } => ops.iter().collect::<Vec<_>>(),
        other => vec![other],
    });
    for op in all {
        let sizes_finite = match op {
            DrawOp::TwoPins { left, right } => left.is_finite() && right.is_finite(),
            DrawOp::Pin { at, length, .. } => finite(*at) && length.is_finite(),
            DrawOp::Shape { width, .. } => width.is_none_or(f64::is_finite),
            DrawOp::Line { a, b } | DrawOp::Plate { a, b } => finite(*a) && finite(*b),
            DrawOp::Contact { center } => finite(*center),
            DrawOp::ArrowHead { from, tip } | DrawOp::Arrow { from, tip } => {
                finite(*from) && finite(*tip)
            }
            DrawOp::Labels { top, bottom } => top.is_finite() && bottom.is_finite(),
            DrawOp::SideLabels { at } => finite(*at),
            DrawOp::RailLabels { y } => y.is_finite(),
            DrawOp::Styled { .. } => true,
        };
        if !sizes_finite {
            return Err(invalid("a size is not a finite number"));
        }
    }
    for style in [ResistorStyle::Iec, ResistorStyle::Us] {
        let mut pinned = Vec::new();
        for op in drawn_ops(&drawing.ops, style) {
            match op {
                DrawOp::TwoPins { .. } => pinned.extend([1, 2]),
                DrawOp::Pin { terminal, .. } => pinned.push(*terminal),
                _ => {}
            }
        }
        pinned.sort_unstable();
        if pinned != (1..=count).collect::<Vec<_>>() {
            return Err(invalid(&format!(
                "it needs exactly one pin for each of terminals 1–{count}"
            )));
        }
    }
    Ok(())
}

/// The steps drawn under the resistor `style`, in order.
fn drawn_ops(ops: &[DrawOp], style: ResistorStyle) -> impl Iterator<Item = &DrawOp> {
    ops.iter().flat_map(move |op| match op {
        DrawOp::Styled { resistor, ops } if *resistor == style => ops.iter().collect(),
        DrawOp::Styled { .. } => Vec::new(),
        other => vec![other],
    })
}

/// The symbol of `kind` for `component`, its pins on `terminals` in the
/// order the module documents, drawn from the installed standards'
/// drawing of the kind. One unit `A` with local ID 1, graphics from ID 2
/// (a resistor's IEC rectangle is the body graphic, ID 2), the designator
/// and component name as text. Refuses the box kinds (the samples lay those
/// out) and a terminal count the drawing does not have.
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
    let drawings = &standards::get()?.symbols;
    let drawing = drawings
        .drawing(kind)
        .ok_or_else(|| Error::NotFound(format!("symbol.kind: no {} drawing", kind.label())))?;
    let mut d = Drawing::new(drawings.style());
    d.numbered = drawing.numbered;
    let t = |n: usize| {
        n.checked_sub(1)
            .and_then(|k| terminals.get(k))
            .copied()
            .unwrap_or(TerminalId(0))
    };
    for op in drawn_ops(&drawing.ops, conventions.resistor) {
        match op {
            DrawOp::TwoPins { left, right } => d.two_pins(t(1), t(2), *left, *right),
            DrawOp::Pin {
                terminal,
                at,
                direction,
                length,
            } => d.pin(t(*terminal), *at, *direction, *length),
            DrawOp::Shape { shape, width, fill } => {
                d.shape(shape.clone(), width.unwrap_or(SYMBOL_STROKE_MM), *fill);
            }
            DrawOp::Line { a, b } => d.line(*a, *b),
            DrawOp::Plate { a, b } => d.plate(*a, *b),
            DrawOp::Contact { center } => d.contact(*center),
            DrawOp::ArrowHead { from, tip } => d.arrow_head(*from, *tip),
            DrawOp::Arrow { from, tip } => d.arrow(*from, *tip),
            DrawOp::Labels { top, bottom } => d.labels(*top, *bottom),
            DrawOp::SideLabels { at } => d.side_labels(*at),
            DrawOp::RailLabels { y } => d.rail_labels(*y),
            // `drawn_ops` has opened the styled steps that apply.
            DrawOp::Styled { .. } => {}
        }
    }
    d.finish(component)
}

/// Collects one unit's graphics and pins with sequential local IDs. It
/// draws in millimetres and stores each item rounded to nanometres; the
/// first value that cannot be stored fails [`Drawing::finish`].
struct Drawing<'a> {
    style: &'a SymbolStyle,
    graphics: Vec<Graphic>,
    pins: Vec<Pin>,
    next: u32,
    /// Pins show their numbers (transistors); two-terminal parts and rails
    /// hide them.
    numbered: bool,
    error: Option<Error>,
}

impl<'a> Drawing<'a> {
    fn new(style: &'a SymbolStyle) -> Self {
        Self {
            style,
            graphics: Vec::new(),
            pins: Vec::new(),
            // Local ID 1 is the unit.
            next: 2,
            numbered: false,
            error: None,
        }
    }

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

    fn line(&mut self, a: Point, b: Point) {
        self.shape(Shape::Line { a, b }, SYMBOL_STROKE_MM, Fill::None);
    }

    fn plate(&mut self, a: Point, b: Point) {
        self.shape(Shape::Line { a, b }, self.style.plate_stroke_mm, Fill::None);
    }

    fn contact(&mut self, center: Point) {
        self.shape(
            Shape::Circle {
                center,
                radius: self.style.contact_radius_mm,
            },
            SYMBOL_STROKE_MM,
            Fill::Body,
        );
    }

    fn arrow_head(&mut self, from: Point, tip: Point) {
        let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
        let length = dx.hypot(dy);
        if length <= f64::EPSILON {
            return;
        }
        let (ux, uy) = (dx / length, dy / length);
        let [l, w] = self.style.arrow_mm;
        let back = [tip[0] - ux * l, tip[1] - uy * l];
        self.shape(
            Shape::polygon(vec![
                tip,
                [back[0] - uy * w, back[1] + ux * w],
                [back[0] + uy * w, back[1] - ux * w],
            ]),
            self.style.arrow_stroke_mm,
            Fill::Solid,
        );
    }

    fn arrow(&mut self, from: Point, tip: Point) {
        let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
        let length = dx.hypot(dy);
        let head = self.style.arrow_mm[0];
        if length <= head {
            return;
        }
        let shaft = 1.0 - head / length;
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

    fn labels(&mut self, top: f64, bottom: f64) {
        let gap = self.style.label_gap_mm;
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

    fn side_labels(&mut self, at: Point) {
        let gap = self.style.side_label_gap_mm;
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

    fn rail_labels(&mut self, y: f64) {
        let gap = self.style.label_gap_mm;
        let (at, v) = if y < 0.0 {
            ([0.0, y - gap], VAlign::Top)
        } else {
            ([0.0, y + gap], VAlign::Bottom)
        };
        self.text(TextContent::ComponentName, at, HAlign::Center, v, true);
        self.text(
            TextContent::Designator,
            self.style.rail_designator_mm,
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
