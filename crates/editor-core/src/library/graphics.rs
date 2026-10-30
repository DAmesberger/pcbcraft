//! Drawn definition content: primitives and text with stable local IDs,
//! symbol pins, the authoring grid and advisory drawing lint. Shapes and
//! their maths live in `geometry`; this module owns what a library draws and
//! the rules for it. Text is a structured reference (designator, field,
//! name), never a placeholder string, so it is never the authority for a
//! value.
use super::drafts::{Draft, Drafts};
use super::publish::{issue, object};
use super::{Definition, Library, MAX_LABEL};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{
    Bounds, Point, Shape, cos_sin, finite_within, point_valid, polygon_self_intersects,
};
use crate::ids::{DefinitionId, GraphicId, TerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod edit;
pub use edit::{GraphicEdit, GraphicTarget, MAX_GRAPHIC_EDITS, apply};

/// Graphics in one symbol unit.
pub const MAX_GRAPHICS_PER_UNIT: usize = 256;
/// Graphics in one definition.
pub const MAX_GRAPHICS: usize = 1024;
/// Points in one polyline or polygon.
pub const MAX_SHAPE_POINTS: usize = 1024;
/// Stored points in one definition's graphics; bounds its encoded size.
pub const MAX_VERTICES: usize = 8192;
/// Widest stroke, in millimetres.
pub const MAX_STROKE_MM: f64 = 5.0;
/// Text cap heights, in millimetres.
pub const TEXT_HEIGHT_MM: std::ops::RangeInclusive<f64> = 0.1..=50.0;
/// Longest pin, in millimetres.
pub const MAX_PIN_LENGTH_MM: f64 = 50.0;
/// Largest pin name or number offset, in millimetres.
pub const MAX_PIN_TEXT_OFFSET_MM: f64 = 10.0;
/// A radius below this is degenerate.
pub const MIN_RADIUS_MM: f64 = 0.01;
/// The symbol module: pin pitch of the templates and the size of pin
/// decorations (IEC TS 63064 / ISO 81714 concept of a 2.5 mm module).
pub const SYMBOL_MODULE_MM: f64 = 2.5;
/// Default symbol stroke width.
pub const SYMBOL_STROKE_MM: f64 = 0.25;
/// Default pin name and number height.
pub const PIN_TEXT_HEIGHT_MM: f64 = 1.25;
/// Default designator and name text height on symbols.
pub const SYMBOL_TEXT_HEIGHT_MM: f64 = 1.5;
/// How far a point may sit from a grid line and still be on it.
pub const GRID_TOLERANCE_MM: f64 = 1e-6;
/// The schematic sheet grid symbols are placed on.
pub const SHEET_GRID_MM: f64 = 5.0;
/// Lines one text may have.
pub const MAX_TEXT_LINES: usize = 16;
/// Default distance between text baselines, in cap heights.
pub const TEXT_LINE_SPACING: f64 = 1.6;
/// Line spacings a text may use, in cap heights.
pub const LINE_SPACING: std::ops::RangeInclusive<f64> = 1.0..=4.0;

/// A drawn primitive or text with its stable local identity.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Graphic {
    pub id: GraphicId,
    pub item: Item,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Item {
    /// A shape stroked with `width` millimetres (0 draws no outline).
    Shape {
        shape: Shape,
        width: f64,
        fill: Fill,
    },
    Text(Text),
}

/// How a closed shape's interior is drawn.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    #[default]
    None,
    /// The theme's symbol background (symbols only).
    Body,
    /// The stroke colour.
    Solid,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub content: TextContent,
    /// The justification anchor.
    pub at: Point,
    /// Cap height in millimetres.
    pub height: f64,
    pub rotation: Rotation,
    pub justify: Justify,
    pub visible: bool,
    /// Distance from one line to the next, in cap heights (1–4). Literal
    /// text breaks lines at `\n`.
    pub line_spacing: f64,
}

/// The terminals of a symbol's owning component: its draft when there is
/// one, else its latest published revision.
pub fn owner_terminals(
    drafts: &Drafts,
    library: &Library,
    component: DefinitionId,
) -> std::collections::BTreeSet<TerminalId> {
    if let Some(draft) = drafts.get(component) {
        return match &draft.definition {
            Definition::Component(c) => c.terminals.iter().map(|t| t.id).collect(),
            _ => Default::default(),
        };
    }
    library
        .latest_components()
        .into_iter()
        .find(|c| c.id == component)
        .map(|c| c.definition.terminals.iter().map(|t| t.id).collect())
        .unwrap_or_default()
}

/// Whether literal text is acceptable: not blank, at most 256 bytes and 16
/// lines, and no control characters except line breaks.
pub fn printable_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= MAX_LABEL
        && text.split('\n').count() <= MAX_TEXT_LINES
        && !text.chars().any(|c| c.is_control() && c != '\n')
}

/// What a text shows. Everything except `Literal` resolves on the placed
/// instance. A later `ActiveFunction(TerminalId)` may show a terminal's
/// selected function name.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TextContent {
    /// Printable text of at most 256 bytes; `\n` breaks lines (at most 16).
    Literal(String),
    /// The instance designator (plus unit label on multi-unit symbols). In
    /// the editor, the component's designator prefix and `?`.
    Designator,
    /// The component's name.
    ComponentName,
    /// The unit's label.
    UnitLabel,
    /// The effective value of a field lineage the component selects; the
    /// author chooses which field is the "value".
    Field(DefinitionId),
}

/// A quarter-turn text rotation, counter-clockwise.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn degrees(self) -> f64 {
        match self {
            Self::R0 => 0.0,
            Self::R90 => 90.0,
            Self::R180 => 180.0,
            Self::R270 => 270.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// Where the anchor sits on the text box, before rotation.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Justify {
    pub h: HAlign,
    pub v: VAlign,
}

impl Justify {
    /// The same anchor seen from the opposite side: left and right, top
    /// and bottom swapped.
    pub fn turned(self) -> Self {
        Self {
            h: match self.h {
                HAlign::Left => HAlign::Right,
                HAlign::Center => HAlign::Center,
                HAlign::Right => HAlign::Left,
            },
            v: match self.v {
                VAlign::Top => VAlign::Bottom,
                VAlign::Middle => VAlign::Middle,
                VAlign::Bottom => VAlign::Top,
            },
        }
    }
}

/// The upright rule every view applies: text that would read downward or
/// leftward (a rotation in (90°, 270°]) is turned 180° and its
/// justification swapped, so it reads left to right or bottom to top and
/// covers the same box. Returns the rotation in `[0, 360)`.
pub fn upright(rotation_deg: f64, justify: Justify) -> (f64, Justify) {
    let r = rotation_deg.rem_euclid(360.0);
    const EPS: f64 = 1e-9;
    if r > 90.0 + EPS && r <= 270.0 + EPS {
        ((r - 180.0).rem_euclid(360.0), justify.turned())
    } else {
        (r, justify)
    }
}

/// A symbol pin: where a terminal connects and how its stub is drawn. A pin
/// is identified by its terminal; it stores no electrical type (direction
/// lives on the terminal's functions) and no number (that comes from the
/// instance's binding).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub terminal: TerminalId,
    /// The connection end, relative to the unit origin; wires attach here.
    pub position: Point,
    /// From the connection end into the body.
    pub direction: PinDirection,
    /// 0–50 mm.
    pub length: f64,
    pub decorations: Decorations,
    /// Shows the terminal's name.
    pub name: PinText,
    /// Shows the designations the instance's binding maps to the terminal.
    pub number: PinText,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PinDirection {
    Right,
    Up,
    Left,
    Down,
}

impl PinDirection {
    /// The unit vector from the connection end into the body.
    pub fn vector(self) -> Point {
        match self {
            Self::Right => [1.0, 0.0],
            Self::Up => [0.0, 1.0],
            Self::Left => [-1.0, 0.0],
            Self::Down => [0.0, -1.0],
        }
    }
    pub fn is_horizontal(self) -> bool {
        matches!(self, Self::Right | Self::Left)
    }
}

/// Reading aids only; they never imply an electrical function (IEEE 91 /
/// IEC 60617-12 negation and dynamic-input qualifiers, as concepts).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Decorations {
    pub inverted: bool,
    pub clock: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinText {
    pub visible: bool,
    /// Distance from the pin, in millimetres (|offset| ≤ 10).
    pub offset: f64,
}

impl Default for PinText {
    fn default() -> Self {
        Self {
            visible: true,
            offset: 0.5,
        }
    }
}

/// The authoring grid: an editor setting saved with the drafts, never part
/// of revision data and never a blocking rule.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    /// 0.01–10 mm; 2.5 mm by default.
    pub symbol_mm: f64,
    /// 0.01–10 mm; 0.05 mm by default.
    pub footprint_mm: f64,
}

impl Default for Grid {
    fn default() -> Self {
        Self {
            symbol_mm: SYMBOL_MODULE_MM,
            footprint_mm: 0.05,
        }
    }
}

impl Grid {
    pub const RANGE_MM: std::ops::RangeInclusive<f64> = 0.01..=10.0;
    pub fn valid(&self) -> bool {
        [self.symbol_mm, self.footprint_mm]
            .iter()
            .all(|v| v.is_finite() && Self::RANGE_MM.contains(v))
    }
}

/// Whether `p` lies on a grid of pitch `step` through the origin.
pub fn on_grid(p: Point, step: f64) -> bool {
    p.iter()
        .all(|v| (v - (v / step).round() * step).abs() <= GRID_TOLERANCE_MM)
}

/// The core's text extent rule: an **approximate** box per line of `chars ×
/// advance × height` by `height`, lines `line_spacing × height` apart. Core
/// has no fonts; exact glyphs exist only in views.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetric {
    pub advance: f64,
}

impl Default for TextMetric {
    fn default() -> Self {
        Self { advance: 0.6 }
    }
}

/// Characters assumed for text whose content resolves on an instance.
pub const PLACEHOLDER_CHARS: usize = 4;

/// The extent of a text block in characters and lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextSize {
    /// Characters on the longest line.
    pub columns: usize,
    pub lines: usize,
    /// Distance between lines, in cap heights.
    pub line_spacing: f64,
}

impl TextSize {
    /// One line of `chars` characters.
    pub fn line(chars: usize) -> Self {
        Self {
            columns: chars,
            lines: 1,
            line_spacing: TEXT_LINE_SPACING,
        }
    }
    /// The block `text` lays out, broken at `\n`.
    pub fn of(text: &str, line_spacing: f64) -> Self {
        Self {
            columns: text
                .split('\n')
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0),
            lines: text.split('\n').count().clamp(1, MAX_TEXT_LINES),
            line_spacing,
        }
    }
    /// Block height for a cap height of `height`.
    fn block_height(&self, height: f64) -> f64 {
        let spacing = if self.line_spacing.is_finite() {
            self.line_spacing
                .clamp(*LINE_SPACING.start(), *LINE_SPACING.end())
        } else {
            TEXT_LINE_SPACING
        };
        height * (1.0 + (self.lines.clamp(1, MAX_TEXT_LINES) - 1) as f64 * spacing)
    }
}

/// One line of a text block: what it shows and where its centre-left,
/// centre or centre-right anchor (by the block's horizontal justification)
/// sits, already rotated into place.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine<'a> {
    pub text: &'a str,
    pub at: Point,
}

/// Lay a block of `text` out line by line. The block's justification places
/// the whole block against `at`; every line keeps the block's horizontal
/// justification and is centred vertically on its own cap box, so a
/// one-line block puts its single line exactly where a one-line text goes.
pub fn text_lines(
    text: &str,
    at: Point,
    height: f64,
    rotation_deg: f64,
    justify: Justify,
    line_spacing: f64,
) -> Vec<TextLine<'_>> {
    let size = TextSize::of(text, line_spacing);
    let block = size.block_height(height);
    let pitch = if size.lines > 1 {
        (block - height) / (size.lines - 1) as f64
    } else {
        0.0
    };
    let top = match justify.v {
        VAlign::Top => 0.0,
        VAlign::Middle => block / 2.0,
        VAlign::Bottom => block,
    };
    let (c, s) = cos_sin(rotation_deg);
    text.split('\n')
        .take(MAX_TEXT_LINES)
        .enumerate()
        .map(|(i, line)| {
            let y = top - height / 2.0 - i as f64 * pitch;
            TextLine {
                text: line,
                at: [at[0] - s * y, at[1] + c * y],
            }
        })
        .collect()
}

impl TextMetric {
    /// The approximate box of a text block anchored at `at`.
    pub fn bounds(
        &self,
        at: Point,
        height: f64,
        rotation_deg: f64,
        justify: Justify,
        size: TextSize,
    ) -> Bounds {
        let width = height * self.advance * size.columns.min(4096) as f64;
        let block = size.block_height(height);
        let x0 = match justify.h {
            HAlign::Left => 0.0,
            HAlign::Center => -width / 2.0,
            HAlign::Right => -width,
        };
        let y0 = match justify.v {
            VAlign::Bottom => 0.0,
            VAlign::Middle => -block / 2.0,
            VAlign::Top => -block,
        };
        let (c, s) = cos_sin(rotation_deg);
        let corners = [
            [x0, y0],
            [x0 + width, y0],
            [x0 + width, y0 + block],
            [x0, y0 + block],
        ];
        Bounds::of(
            corners
                .iter()
                .map(|[x, y]| [at[0] + c * x - s * y, at[1] + s * x + c * y]),
        )
        .unwrap_or(Bounds::point(at))
    }
}

impl Text {
    /// The block the text shows in a definition, before resolution;
    /// placeholders are one line of [`PLACEHOLDER_CHARS`].
    pub fn size(&self) -> TextSize {
        match &self.content {
            TextContent::Literal(text) => TextSize::of(text, self.line_spacing),
            _ => TextSize::line(PLACEHOLDER_CHARS),
        }
    }
    pub fn bounds(&self, m: TextMetric) -> Bounds {
        m.bounds(
            self.at,
            self.height,
            self.rotation.degrees(),
            self.justify,
            self.size(),
        )
    }
}

impl Item {
    /// The item moved by `delta` millimetres.
    pub fn translated(&self, delta: Point) -> Item {
        match self {
            Self::Shape { shape, width, fill } => Self::Shape {
                shape: shape.translated(delta),
                width: *width,
                fill: *fill,
            },
            Self::Text(text) => Self::Text(Text {
                at: [text.at[0] + delta[0], text.at[1] + delta[1]],
                ..text.clone()
            }),
        }
    }

    /// Points the item stores.
    pub fn vertices(&self) -> usize {
        match self {
            Self::Shape { shape, .. } => shape.stored_points(),
            Self::Text(_) => 1,
        }
    }
    pub(super) fn within_bounds(&self) -> bool {
        match self {
            Self::Shape { shape, .. } => shape.stored_points() <= MAX_SHAPE_POINTS,
            Self::Text(text) => match &text.content {
                TextContent::Literal(s) => s.len() <= MAX_LABEL,
                _ => true,
            },
        }
    }
    /// Finite geometry within the supported ranges.
    pub(super) fn finite(&self) -> bool {
        match self {
            Self::Shape { shape, width, .. } => {
                shape.finite() && width.is_finite() && (0.0..=MAX_STROKE_MM).contains(width)
            }
            Self::Text(text) => {
                point_valid(text.at)
                    && text.height.is_finite()
                    && TEXT_HEIGHT_MM.contains(&text.height)
                    && text.line_spacing.is_finite()
                    && LINE_SPACING.contains(&text.line_spacing)
            }
        }
    }
    /// Approximate extent; text uses `m`.
    pub fn bounds(&self, m: TextMetric) -> Bounds {
        match self {
            Self::Shape { shape, width, .. } => shape.bounds(*width),
            Self::Text(text) => text.bounds(m),
        }
    }
}

/// Blocking findings of one graphic shared by every kind that draws.
pub(super) fn graphic_findings(
    definition: DefinitionId,
    graphic: &Graphic,
    out: &mut Vec<Finding>,
) {
    let this = vec![object(definition, graphic.id.0)];
    match &graphic.item {
        Item::Shape { shape, width, fill } => {
            if degenerate(shape) {
                out.push(issue(
                    "graphic.degenerate",
                    format!(
                        "Graphic {} has no length or area, too few points or a radius below 0.01 mm.",
                        graphic.id
                    ),
                    this.clone(),
                ));
            }
            if !shape.corners_fit() {
                out.push(issue(
                    "graphic.radius_too_large",
                    format!(
                        "Graphic {} rounds a corner more than its edges allow, or rounds an open end.",
                        graphic.id
                    ),
                    this.clone(),
                ));
            }
            if let Shape::Polygon { points, .. } = shape
                && polygon_self_intersects(points)
            {
                out.push(issue(
                    "graphic.polygon_self_intersecting",
                    format!("Polygon {} crosses itself.", graphic.id),
                    this.clone(),
                ));
            }
            if *width <= 0.0 && *fill == Fill::None {
                out.push(issue(
                    "graphic.invisible",
                    format!("Graphic {} has no stroke and no fill.", graphic.id),
                    this.clone(),
                ));
            }
            if *fill != Fill::None && !shape.is_closed() {
                out.push(issue(
                    "graphic.fill_open",
                    format!("Graphic {} is open and cannot be filled.", graphic.id),
                    this,
                ));
            }
        }
        Item::Text(text) => {
            if let TextContent::Literal(s) = &text.content
                && !printable_text(s)
            {
                let too_long = s.len() > MAX_LABEL || s.split('\n').count() > MAX_TEXT_LINES;
                out.push(if too_long {
                    issue(
                        "text.too_long",
                        format!(
                            "Text {} is longer than {MAX_LABEL} bytes or {MAX_TEXT_LINES} lines.",
                            graphic.id
                        ),
                        this,
                    )
                } else {
                    issue(
                        "text.empty",
                        format!("Text {} needs printable content.", graphic.id),
                        this,
                    )
                });
            }
        }
    }
}

fn degenerate(shape: &Shape) -> bool {
    const EPS: f64 = 1e-9;
    match shape {
        Shape::Line { a, b } => crate::geometry::distance(*a, *b) <= EPS,
        Shape::Polyline { points, .. } => {
            points.len() < 2
                || points
                    .windows(2)
                    .all(|w| crate::geometry::distance(w[0], w[1]) <= EPS)
        }
        Shape::Rect { a, b, .. } => (a[0] - b[0]).abs() <= EPS || (a[1] - b[1]).abs() <= EPS,
        Shape::Circle { radius, .. } => *radius < MIN_RADIUS_MM,
        Shape::Arc {
            radius, sweep_deg, ..
        } => *radius < MIN_RADIUS_MM || sweep_deg.abs() <= EPS,
        Shape::Polygon { points, .. } => {
            points.len() < 3 || crate::geometry::signed_area2(points).abs() <= EPS
        }
    }
}

impl Pin {
    /// A pin with visible name and number and no decorations.
    pub fn new(
        terminal: TerminalId,
        position: Point,
        direction: PinDirection,
        length: f64,
    ) -> Self {
        Self {
            terminal,
            position,
            direction,
            length,
            decorations: Decorations::default(),
            name: PinText::default(),
            number: PinText::default(),
        }
    }

    /// Where the pin meets the body.
    pub fn body_end(&self) -> Point {
        let d = self.direction.vector();
        [
            self.position[0] + d[0] * self.length,
            self.position[1] + d[1] * self.length,
        ]
    }

    /// Diameter of the inversion bubble and size of the clock wedge.
    pub fn decoration_size() -> f64 {
        0.4 * SYMBOL_MODULE_MM
    }

    /// The drawn stub: from the connection end to the body, stopping short
    /// of an inversion bubble.
    pub fn line(&self) -> [Point; 2] {
        let d = self.direction.vector();
        let cut = if self.decorations.inverted {
            Self::decoration_size().min(self.length)
        } else {
            0.0
        };
        let body = self.body_end();
        [self.position, [body[0] - d[0] * cut, body[1] - d[1] * cut]]
    }

    /// The decorations every view draws the same way: the bubble replaces
    /// the body end of the stub; the clock wedge sits inside the body edge.
    pub fn decoration_shapes(&self) -> Vec<Shape> {
        let size = Self::decoration_size();
        let d = self.direction.vector();
        let n = [-d[1], d[0]];
        let body = self.body_end();
        let mut out = Vec::new();
        if self.decorations.inverted {
            let r = size.min(self.length) / 2.0;
            if r >= MIN_RADIUS_MM {
                out.push(Shape::Circle {
                    center: [body[0] - d[0] * r, body[1] - d[1] * r],
                    radius: r,
                });
            }
        }
        if self.decorations.clock {
            let h = size / 2.0;
            out.push(Shape::polyline(vec![
                [body[0] + n[0] * h, body[1] + n[1] * h],
                [body[0] + d[0] * size, body[1] + d[1] * size],
                [body[0] - n[0] * h, body[1] - n[1] * h],
            ]));
        }
        out
    }

    pub(super) fn finite(&self) -> bool {
        point_valid(self.position)
            && self.length.is_finite()
            && (0.0..=MAX_PIN_LENGTH_MM).contains(&self.length)
            && [self.name.offset, self.number.offset]
                .iter()
                .all(|o| finite_within(&[*o]) && o.abs() <= MAX_PIN_TEXT_OFFSET_MM)
    }

    /// Where a pin name sits: just inside the body, reading along the pin.
    pub fn name_anchor(&self) -> (Point, f64, Justify) {
        let d = self.direction.vector();
        let body = self.body_end();
        let at = [
            body[0] + d[0] * self.name.offset,
            body[1] + d[1] * self.name.offset,
        ];
        let (rotation, h) = match self.direction {
            PinDirection::Right => (0.0, HAlign::Left),
            PinDirection::Left => (0.0, HAlign::Right),
            PinDirection::Up => (90.0, HAlign::Left),
            PinDirection::Down => (90.0, HAlign::Right),
        };
        (
            at,
            rotation,
            Justify {
                h,
                v: VAlign::Middle,
            },
        )
    }

    /// Where a pin number sits: centred along the stub, on its upper (or,
    /// for vertical pins, left) side.
    pub fn number_anchor(&self) -> (Point, f64, Justify) {
        let body = self.body_end();
        let mid = [
            (self.position[0] + body[0]) / 2.0,
            (self.position[1] + body[1]) / 2.0,
        ];
        let o = self.number.offset;
        let (at, rotation) = if self.direction.is_horizontal() {
            ([mid[0], mid[1] + o], 0.0)
        } else {
            ([mid[0] - o, mid[1]], 90.0)
        };
        (
            at,
            rotation,
            Justify {
                h: HAlign::Center,
                v: VAlign::Bottom,
            },
        )
    }
}

/// A pin for `terminal` at `position` pointing into `body`: its direction
/// comes from the side of the body it sits on, its length is the distance
/// to the body edge, or one module when that distance is 0.
pub fn pin_toward(terminal: TerminalId, position: Point, body: Bounds) -> Pin {
    let outside = [
        (body.min[0] - position[0], PinDirection::Right),
        (position[0] - body.max[0], PinDirection::Left),
        (body.min[1] - position[1], PinDirection::Up),
        (position[1] - body.max[1], PinDirection::Down),
    ];
    let (gap, direction) =
        outside
            .iter()
            .copied()
            .fold((f64::NEG_INFINITY, PinDirection::Right), |best, side| {
                if side.0 > best.0 { side } else { best }
            });
    let length = if gap > GRID_TOLERANCE_MM {
        gap.min(MAX_PIN_LENGTH_MM)
    } else {
        SYMBOL_MODULE_MM
    };
    Pin::new(terminal, position, direction, length)
}

/// A rectangle body graphic with the default symbol stroke and body fill.
pub fn body_rect(id: GraphicId, body: Bounds) -> Graphic {
    Graphic {
        id,
        item: Item::Shape {
            shape: Shape::rect(body.min, body.max),
            width: SYMBOL_STROKE_MM,
            fill: Fill::Body,
        },
    }
}

/// A visible text graphic at the default symbol text height.
pub fn text(id: GraphicId, content: TextContent, at: Point, justify: Justify) -> Graphic {
    Graphic {
        id,
        item: Item::Text(Text {
            content,
            at,
            height: SYMBOL_TEXT_HEIGHT_MM,
            rotation: Rotation::R0,
            justify,
            visible: true,
            line_spacing: TEXT_LINE_SPACING,
        }),
    }
}

/// What a point on a symbol unit hits. Pin name and number text belong to
/// their pin and are not separately selectable.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum SymbolHit {
    Pin(TerminalId),
    Graphic(GraphicId),
    /// A shown field value a placed unit stacks below its drawing; it has
    /// no graphic of its own and stands for the unit as a whole.
    Field,
}

/// Advisory findings for one draft against the authoring grid. They never
/// block publishing and never touch stored files.
pub fn lint(draft: &Draft, _library: &Library, grid: &Grid) -> Vec<Finding> {
    let mut out = Vec::new();
    let this = ObjectRef::Definition { id: draft.id };
    if let Definition::Symbol(symbol) = &draft.definition {
        let ratio = SHEET_GRID_MM / grid.symbol_mm;
        if (ratio - ratio.round()).abs() > 1e-6 {
            out.push(Finding::new(
                "symbol.grid_sheet",
                format!(
                    "The {} mm symbol grid does not divide the {SHEET_GRID_MM} mm sheet grid, so pins land off the sheet grid.",
                    grid.symbol_mm
                ),
                vec![this],
            ));
        }
        for unit in &symbol.units {
            for pin in &unit.pins {
                if !on_grid(pin.position, grid.symbol_mm) {
                    out.push(Finding::new(
                        "symbol.pin_off_grid",
                        format!(
                            "The pin of terminal {} at {:?} mm is off the {} mm grid.",
                            pin.terminal, pin.position, grid.symbol_mm
                        ),
                        vec![
                            object(draft.id, unit.id.0),
                            ObjectRef::LibraryObject {
                                definition: symbol.component,
                                local: pin.terminal.0,
                            },
                        ],
                    ));
                }
            }
            let has_designator = unit
                .graphics
                .iter()
                .any(|g| matches!(&g.item, Item::Text(t) if t.content == TextContent::Designator));
            if !has_designator {
                out.push(Finding::new(
                    "symbol.field_missing",
                    format!("Unit {} shows no designator text.", unit.label),
                    vec![object(draft.id, unit.id.0)],
                ));
            }
        }
    }
    out
}
