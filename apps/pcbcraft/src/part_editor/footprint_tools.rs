//! The Footprint tab's sketch tools (slice F4) as a small state machine,
//! following the Symbol drawing tools (`drawing.rs`, adapted from
//! Solidcraft's sketch tools): clicked points collect until a gesture is
//! complete, Esc or a right click ends the gesture and then the tool, the
//! tool stays armed after a placement, and snapping happens before a point
//! reaches the tool. Nothing here touches the document: a finished gesture
//! returns the [`FootprintEdit`]s of one `library.edit_footprint` commit.
//!
//! A gesture that needs the ids of entities it creates in the same commit
//! (an array's seed pad and direction line, a drawn curve's graphic role,
//! a drill circle) stages its prefix with the core's pure
//! `footprint_edit::preview` and the evaluator's `apply_sketch`; both are
//! deterministic, so the engine allocates the same ids when the one commit
//! is applied.
//!
//! Dimension and constraint gestures belong to `footprint_dims`, which
//! queues its edits in the same outbox (docs/component-model.md, F4/F5 as
//! built).
use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::geometry::distance;
use pcbcraft_core::ids::GraphicId;
use pcbcraft_core::library::footprint_edit;
use pcbcraft_core::library::footprint_source::{
    ArrayLayout, DimKind, DimValue, Expr, FootprintEdit, FootprintEvaluator, FootprintSource,
    GenerationKey, GeomConstraint, GraphicRole, GraphicSource, HoleRole, NumberingScheme,
    PadGeometry, PadRole, PadTemplate, PadTemplateParams, ParamName, ParamUnit, Pin1Position,
    SketchBlob, SketchCurveId, SketchEdit, SketchPointId, SketchRef, SkipRule,
};
use pcbcraft_core::library::graphics::DrawingLayer;
use pcbcraft_core::library::{
    Fill, FootprintDefinition, PackageDefinition, PadKind, PadSide, Side,
};
use pcbcraft_core::{DefinitionId, LibraryCommand, Operation, PadId, Point};
use pcbcraft_solidcraft::footprint::view::{SketchView, ViewGeometry};

/// Pad array layouts the Array tool offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrayTool {
    Linear,
    DualRow,
    /// Four sides with per-side counts.
    Quad,
    /// Rows and columns with skips.
    Grid,
}

/// One ribbon tool of the Footprint tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FootprintTool {
    Select,
    /// A constrained template pad placed with one click.
    Pad(PadTemplate),
    /// Click inside a closed copper profile, then pick its anchor.
    CustomPad,
    Array(ArrayTool),
    /// A concentric round drill on a pad; a bare click makes an NPTH pad.
    Drill,
    Slot,
    Line,
    Arc,
    Circle,
    Rect,
    /// Driving dimension; gestures in `footprint_dims`.
    Dimension,
    /// Geometric constraint; gestures in `footprint_dims`.
    Constrain(GeomConstraint),
}

/// A keyboard shortcut: the key and whether Shift is held (see
/// [`FootprintTool::key`] for the keymap check).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub key: egui::Key,
    pub shift: bool,
}

const fn key(key: egui::Key) -> Option<Key> {
    Some(Key { key, shift: false })
}
const fn shift(key: egui::Key) -> Option<Key> {
    Some(Key { key, shift: true })
}

impl FootprintTool {
    /// Every tool, in ribbon order.
    pub const ALL: [Self; 22] = [
        Self::Select,
        Self::Pad(PadTemplate::Rect),
        Self::Pad(PadTemplate::RoundRect),
        Self::Pad(PadTemplate::Circle),
        Self::Pad(PadTemplate::Oblong),
        Self::CustomPad,
        Self::Array(ArrayTool::Linear),
        Self::Array(ArrayTool::DualRow),
        Self::Array(ArrayTool::Quad),
        Self::Array(ArrayTool::Grid),
        Self::Drill,
        Self::Slot,
        Self::Line,
        Self::Arc,
        Self::Circle,
        Self::Rect,
        Self::Dimension,
        Self::Constrain(GeomConstraint::Coincident),
        Self::Constrain(GeomConstraint::Horizontal),
        Self::Constrain(GeomConstraint::Vertical),
        Self::Constrain(GeomConstraint::Equal),
        Self::Constrain(GeomConstraint::Symmetric),
    ];

    /// Split-button families of the Pads group: the first is shown until
    /// another variant is armed.
    pub const FAMILIES: [&'static [Self]; 3] = [
        &[
            Self::Pad(PadTemplate::Rect),
            Self::Pad(PadTemplate::RoundRect),
            Self::Pad(PadTemplate::Circle),
            Self::Pad(PadTemplate::Oblong),
        ],
        &[
            Self::Array(ArrayTool::Linear),
            Self::Array(ArrayTool::DualRow),
            Self::Array(ArrayTool::Quad),
            Self::Array(ArrayTool::Grid),
        ],
        &[Self::Drill, Self::Slot],
    ];
    /// The Sketch group's drawing tools, in ribbon order.
    pub const SKETCH: [Self; 5] = [
        Self::Select,
        Self::Line,
        Self::Arc,
        Self::Circle,
        Self::Rect,
    ];

    /// The ribbon tool id (prefixed `fp-` so it never collides with the
    /// Symbol tab's ids).
    pub fn id(self) -> &'static str {
        match self {
            Self::Select => "fp-select",
            Self::Pad(PadTemplate::Rect) => "fp-pad-rect",
            Self::Pad(PadTemplate::RoundRect) => "fp-pad-roundrect",
            Self::Pad(PadTemplate::Circle) => "fp-pad-circle",
            Self::Pad(PadTemplate::Oblong) => "fp-pad-oblong",
            Self::CustomPad => "fp-pad-custom",
            Self::Array(ArrayTool::Linear) => "fp-array-linear",
            Self::Array(ArrayTool::DualRow) => "fp-array-dual",
            Self::Array(ArrayTool::Quad) => "fp-array-quad",
            Self::Array(ArrayTool::Grid) => "fp-array-grid",
            Self::Drill => "fp-drill",
            Self::Slot => "fp-slot",
            Self::Line => "fp-line",
            Self::Arc => "fp-arc",
            Self::Circle => "fp-circle",
            Self::Rect => "fp-rect",
            Self::Dimension => "fp-dimension",
            Self::Constrain(GeomConstraint::Coincident) => "fp-coincident",
            Self::Constrain(GeomConstraint::Horizontal) => "fp-horizontal",
            Self::Constrain(GeomConstraint::Vertical) => "fp-vertical",
            Self::Constrain(GeomConstraint::Parallel) => "fp-parallel",
            Self::Constrain(GeomConstraint::Perpendicular) => "fp-perpendicular",
            Self::Constrain(GeomConstraint::Equal) => "fp-equal",
            Self::Constrain(GeomConstraint::Tangent) => "fp-tangent",
            Self::Constrain(GeomConstraint::Midpoint) => "fp-midpoint",
            Self::Constrain(GeomConstraint::PointOnCurve) => "fp-on-curve",
            Self::Constrain(GeomConstraint::Concentric) => "fp-concentric",
            Self::Constrain(GeomConstraint::Symmetric) => "fp-symmetric",
            Self::Constrain(GeomConstraint::Fixed) => "fp-fix",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Pad(PadTemplate::Rect) => "Pad",
            Self::Pad(PadTemplate::RoundRect) => "Rounded",
            Self::Pad(PadTemplate::Circle) => "Round",
            Self::Pad(PadTemplate::Oblong) => "Oblong",
            Self::CustomPad => "Custom",
            Self::Array(ArrayTool::Linear) => "Row",
            Self::Array(ArrayTool::DualRow) => "Dual row",
            Self::Array(ArrayTool::Quad) => "Quad",
            Self::Array(ArrayTool::Grid) => "Grid",
            Self::Drill => "Drill",
            Self::Slot => "Slot",
            Self::Line => "Line",
            Self::Arc => "Arc",
            Self::Circle => "Circle",
            Self::Rect => "Rectangle",
            Self::Dimension => "Dimension",
            Self::Constrain(_) => "Constrain",
        }
    }

    /// The Solidcraft icon master drawn on the ribbon button.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Pad(PadTemplate::Rect) => "rect",
            Self::Pad(PadTemplate::RoundRect) => "rect_rounded",
            Self::Pad(PadTemplate::Circle) => "circle2",
            Self::Pad(PadTemplate::Oblong) => "slot_overall",
            Self::CustomPad => "polygon",
            Self::Array(ArrayTool::Linear) => "pattern",
            Self::Array(ArrayTool::DualRow) => "pattern_flip",
            Self::Array(ArrayTool::Quad) => "pattern_center",
            Self::Array(ArrayTool::Grid) => "pattern_rect",
            Self::Drill => "hole",
            Self::Slot => "slot",
            Self::Line => "line",
            Self::Arc => "arc_center",
            Self::Circle => "circle",
            Self::Rect => "rect_center",
            Self::Dimension => "dimension",
            Self::Constrain(_) => "constrainer",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Self::Select => "Select, drag to move, Delete removes (Esc)",
            Self::Pad(PadTemplate::Rect) => {
                "Rectangular pad: click to place; Space turns it a quarter"
            }
            Self::Pad(PadTemplate::RoundRect) => "Rounded-rectangle pad: click to place",
            Self::Pad(PadTemplate::Circle) => "Round pad: click to place",
            Self::Pad(PadTemplate::Oblong) => "Oblong pad: click to place; Space turns it",
            Self::CustomPad => {
                "Custom pad: click a template pad to free its outline, or click to start one"
            }
            Self::Array(ArrayTool::Linear) => "Pad row: pad 1, then the row direction",
            Self::Array(ArrayTool::DualRow) => {
                "Dual row (SOIC): pad 1, then the row direction; the second row faces it"
            }
            Self::Array(ArrayTool::Quad) => {
                "Quad (QFN): click the centre; per-side counts in Properties"
            }
            Self::Array(ArrayTool::Grid) => "Grid (BGA, header): click pad A1; skips in Properties",
            Self::Drill => "Drill: click a pad for a plated hole, or empty board for an NPTH hole",
            Self::Slot => "Slot: click both slot ends on a pad, or on empty board",
            Self::Line => "Line on the active layer: two clicks; chains from its end",
            Self::Arc => "Arc on the active layer: centre, start, end",
            Self::Circle => "Circle on the active layer: centre, then radius",
            Self::Rect => "Rectangle on the active layer from two corners",
            Self::Dimension => "Driving dimension",
            Self::Constrain(_) => "Geometric constraint",
        }
    }

    /// The tool's key. Checked against Solidcraft's keymap (sketch L, R,
    /// C, D, P, X, O, T, G, Shift+R, Delete, Esc; model A, B, E, F, H, I,
    /// J, M, Q; held Space pans) and the Symbol tab's keys: L, R, C and A
    /// draw as in the Symbol tab and Solidcraft's sketch, D is Solidcraft's
    /// Dimension, N places a pad as it places a pin in the Symbol tab, H is
    /// Solidcraft's Hole (only bound outside sketches there), and X keeps
    /// its Solidcraft meaning, Construction, as the active layer toggle.
    /// Space turns the next pad a quarter while a pad tool is armed.
    pub fn key(self) -> Option<Key> {
        use egui::Key as K;
        match self {
            Self::Line => key(K::L),
            Self::Arc => key(K::A),
            Self::Circle => key(K::C),
            Self::Rect => key(K::R),
            Self::Dimension => key(K::D),
            Self::Pad(PadTemplate::Rect) => key(K::N),
            Self::Array(ArrayTool::DualRow) => shift(K::N),
            Self::Drill => key(K::H),
            Self::Slot => shift(K::H),
            _ => None,
        }
    }

    /// Whether Space turns what this tool places.
    pub fn turns(self) -> bool {
        matches!(self, Self::Pad(_) | Self::CustomPad | Self::Array(_))
    }

    /// Whether the tool draws on the active layer.
    pub fn draws(self) -> bool {
        matches!(self, Self::Line | Self::Arc | Self::Circle | Self::Rect)
    }

    /// Clicks a gesture of this tool takes.
    fn clicks(self) -> usize {
        match self {
            Self::Line | Self::Circle | Self::Rect | Self::Slot => 2,
            Self::Array(ArrayTool::Linear | ArrayTool::DualRow) => 2,
            Self::Arc => 3,
            _ => 1,
        }
    }
}

/// The layer Line, Arc, Circle and Rect draw on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActiveLayer {
    /// Construction geometry (reference only, never output).
    #[default]
    Construction,
    Silkscreen,
    Assembly,
    Courtyard,
}

impl ActiveLayer {
    pub const ALL: [Self; 4] = [
        Self::Construction,
        Self::Silkscreen,
        Self::Assembly,
        Self::Courtyard,
    ];

    /// The footprint drawing layer a curve role records; `None` for
    /// construction geometry.
    pub fn drawing_layer(self) -> Option<DrawingLayer> {
        match self {
            Self::Construction => None,
            Self::Silkscreen => Some(DrawingLayer::Silkscreen),
            Self::Assembly => Some(DrawingLayer::Assembly),
            Self::Courtyard => Some(DrawingLayer::Courtyard),
        }
    }

    /// The ribbon id of the layer chooser (distinct from the Show toggles).
    pub fn id(self) -> &'static str {
        match self {
            Self::Construction => "fp-on-construction",
            Self::Silkscreen => "fp-on-silk",
            Self::Assembly => "fp-on-assembly",
            Self::Courtyard => "fp-on-courtyard",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Construction => "Construction",
            Self::Silkscreen => "Silk",
            Self::Assembly => "Assembly",
            Self::Courtyard => "Courtyard",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Construction => "construction",
            Self::Silkscreen => "sketch",
            Self::Assembly => "component",
            Self::Courtyard => "bounding_solid",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Self::Construction => "Draw construction geometry: reference only, never output (X)",
            Self::Silkscreen => "Draw on the silkscreen",
            Self::Assembly => "Draw on the assembly drawing",
            Self::Courtyard => "Draw the courtyard outline",
        }
    }

    /// Default stroke width of a drawn graphic, as an expression.
    fn width(self) -> &'static str {
        match self {
            Self::Construction => "0 mm",
            Self::Silkscreen => "0.15 mm",
            Self::Assembly => "0.1 mm",
            Self::Courtyard => "0.05 mm",
        }
    }
}

/// What a canvas event produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Gesture {
    /// Nothing to commit (a point collected, a hover, a cancelled gesture).
    #[default]
    None,
    /// One finished gesture: exactly one `library.edit_footprint` commit.
    Commit(Vec<FootprintEdit>),
}

/// The pad tools' options (Properties while a pad tool is armed).
#[derive(Clone, Debug, PartialEq)]
pub struct PadOptions {
    /// Width and height of rectangular, rounded and oblong pads, mm.
    pub size: [f64; 2],
    /// Round pad diameter, mm.
    pub diameter: f64,
    /// Rounded-rectangle corner ratio of the short side, 0–0.5.
    pub radius_ratio: f64,
    pub kind: PadKind,
    pub side: PadSide,
}

impl Default for PadOptions {
    fn default() -> Self {
        Self {
            size: [1.55, 0.6],
            diameter: 1.0,
            radius_ratio: 0.25,
            kind: PadKind::Smd,
            side: PadSide::Top,
        }
    }
}

/// The array tools' options: the popover of the design, shown in
/// Properties while an array tool is armed.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrayOptions {
    /// The seed pad's template.
    pub template: PadTemplate,
    /// Pads of a linear row.
    pub count: u16,
    /// Pads per row of a dual row.
    pub per_row: u16,
    /// Pad pitch along a row or side, mm.
    pub pitch: f64,
    /// Distance between the two rows' pad centres, mm.
    pub row_span: f64,
    /// Quad pads per bottom/top side and per left/right side.
    pub nx: u16,
    pub ny: u16,
    /// Quad distances between opposite pad centres, mm.
    pub span: [f64; 2],
    pub pin1: Pin1Position,
    pub rows: u16,
    pub cols: u16,
    /// Grid pitches, mm.
    pub grid_pitch: [f64; 2],
    pub stagger: bool,
    pub alpha: bool,
    /// A centred void of rows × columns (0 for none).
    pub void: [u16; 2],
    /// Keep only this many perimeter rings (0 keeps all).
    pub perimeter: u16,
}

impl Default for ArrayOptions {
    fn default() -> Self {
        Self {
            template: PadTemplate::Rect,
            count: 4,
            per_row: 4,
            pitch: 1.27,
            row_span: 5.4,
            nx: 4,
            ny: 4,
            span: [4.0, 4.0],
            pin1: Pin1Position::Left,
            rows: 3,
            cols: 3,
            grid_pitch: [2.54, 2.54],
            stagger: false,
            alpha: false,
            void: [0, 0],
            perimeter: 0,
        }
    }
}

/// Every tool's options.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOptions {
    pub pad: PadOptions,
    pub array: ArrayOptions,
    /// Drill diameter (and slot width), mm.
    pub drill: f64,
}

impl Default for ToolOptions {
    fn default() -> Self {
        Self {
            pad: PadOptions::default(),
            array: ArrayOptions::default(),
            drill: 0.8,
        }
    }
}

/// One selected thing of the Footprint sketch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// A role pad or a generated array instance.
    Pad(PadId),
    Point(SketchPointId),
    Curve(SketchCurveId),
}

/// What a gesture reads: the working footprint, its package, the sketch
/// view and the evaluator that stages edits (never the document).
pub struct Scene<'a> {
    pub footprint: &'a FootprintDefinition,
    pub package: &'a PackageDefinition,
    pub view: &'a SketchView,
    pub evaluator: &'a dyn FootprintEvaluator,
    /// Pick reach, mm.
    pub tolerance: f64,
    /// Curve flattening for picks, mm.
    pub chord: f64,
}

impl Scene<'_> {
    fn source(&self) -> Option<&FootprintSource> {
        self.footprint.source.as_deref()
    }
}

/// The armed tool and its pending points. Points are millimetres in the
/// footprint frame, already snapped.
#[derive(Clone, Debug)]
pub struct ToolState {
    pub tool: Option<FootprintTool>,
    pub layer: ActiveLayer,
    /// Points collected for the gesture in progress.
    pub pending: Vec<Point>,
    /// Quarter turns (Space) applied to the next placed pad.
    pub quarter_turns: u8,
    pub options: ToolOptions,
    /// The Select tool's selection.
    pub selection: Vec<Pick>,
    /// The last tool armed, for Shift+R.
    pub last: Option<FootprintTool>,
    /// The variant each Pads split button shows.
    pub shown: [FootprintTool; 3],
    /// Signed degrees the pointer went round an arc's centre.
    arc_turn: f64,
    /// A finished gesture waiting to be sent as one commit.
    pub outbox: Option<Vec<FootprintEdit>>,
    /// The working copy's footprint the held outbox was staged against,
    /// while that copy is sent first.
    pub held_for: Option<pcbcraft_core::library::FootprintDefinition>,
    /// A Select drag in progress.
    pub drag: Option<MoveDrag>,
    /// Delete was pressed; the canvas resolves it against its scene.
    pub delete_requested: bool,
    /// Gesture refusals waiting to be shown as error toasts (they arise
    /// where no egui context is at hand; the next frame posts them).
    pub refusals: Vec<String>,
    /// The sketch view of the last sketch drawn, kept while the sketch is
    /// unchanged so idle frames parse nothing.
    view_cache: Option<(SketchBlob, std::sync::Arc<SketchView>)>,
}

/// A Select drag: where it started, the point it carries (a pad centre or
/// sketch point, snapped as it moves) and the snapped offset so far.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveDrag {
    pub start: Point,
    pub reference: Point,
    pub delta: Point,
}

/// Why a gesture finished while another waits to be sent is refused.
pub const OUTBOX_BUSY: &str = "The previous change is still being applied; repeat this one";

impl ToolState {
    /// Record a gesture refusal for an error toast, in shown wording: the
    /// engine text's first sentence when it follows the message style,
    /// else `fallback`.
    pub fn refuse(&mut self, text: &str, fallback: &str) {
        let shown = crate::refusal::presentable(text).unwrap_or_else(|| fallback.to_owned());
        if self.refusals.len() < 4 && !self.refusals.contains(&shown) {
            self.refusals.push(shown);
        }
    }

    /// Queue one finished gesture; refused (false) while another waits,
    /// so no gesture is ever overwritten.
    pub fn queue(&mut self, edits: Vec<FootprintEdit>) -> bool {
        if edits.is_empty() {
            return true;
        }
        if self.outbox.is_some() {
            return false;
        }
        self.outbox = Some(edits);
        true
    }
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            tool: None,
            layer: ActiveLayer::default(),
            pending: Vec::new(),
            quarter_turns: 0,
            options: ToolOptions::default(),
            selection: Vec::new(),
            last: None,
            shown: [
                FootprintTool::Pad(PadTemplate::Rect),
                FootprintTool::Array(ArrayTool::Linear),
                FootprintTool::Drill,
            ],
            arc_turn: 0.0,
            outbox: None,
            held_for: None,
            drag: None,
            delete_requested: false,
            refusals: Vec::new(),
            view_cache: None,
        }
    }
}

fn angle(center: Point, p: Point) -> f64 {
    (p[1] - center[1]).atan2(p[0] - center[0]).to_degrees()
}

fn same(a: Point, b: Point) -> bool {
    distance(a, b) <= 1e-9
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale(a: Point, k: f64) -> Point {
    [a[0] * k, a[1] * k]
}

/// `v` turned a quarter counter-clockwise.
fn left(v: Point) -> Point {
    [-v[1], v[0]]
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

/// A length expression: `1.27 mm`.
fn mm(v: f64) -> String {
    format!("{} mm", crate::kit::number_text(v))
}

impl ToolState {
    /// The sketch view of `source`, parsed once per sketch change; empty
    /// without a source or for a sketch the evaluator refuses.
    pub fn view(&mut self, source: Option<&FootprintSource>) -> std::sync::Arc<SketchView> {
        let Some(source) = source else {
            return std::sync::Arc::default();
        };
        if let Some((blob, view)) = &self.view_cache
            && *blob == source.sketch
        {
            return std::sync::Arc::clone(view);
        }
        let view = std::sync::Arc::new(
            pcbcraft_solidcraft::footprint::view::sketch_view(source).unwrap_or_default(),
        );
        self.view_cache = Some((source.sketch.clone(), std::sync::Arc::clone(&view)));
        view
    }

    /// Take tool options from parameters the sketch already has (plain
    /// `<number> mm` values), so a new pad reuses `pad_l` and not
    /// `pad_l_2`.
    pub fn adopt(&mut self, source: Option<&FootprintSource>) {
        let Some(source) = source else {
            return;
        };
        let value = |name: &str| {
            source
                .parameters
                .iter()
                .find(|p| p.name.0 == name)
                .and_then(|p| p.expr.0.trim().strip_suffix("mm"))
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
        };
        let set = |slot: &mut f64, name: &str| {
            if let Some(v) = value(name) {
                *slot = v;
            }
        };
        let o = &mut self.options;
        set(&mut o.pad.size[0], "pad_l");
        set(&mut o.pad.size[1], "pad_w");
        set(&mut o.pad.diameter, "pad_d");
        set(&mut o.array.pitch, "pitch");
        set(&mut o.array.row_span, "row_span");
        set(&mut o.array.span[0], "span_x");
        set(&mut o.array.span[1], "span_y");
        set(&mut o.array.grid_pitch[0], "pitch_x");
        set(&mut o.array.grid_pitch[1], "pitch_y");
        set(&mut o.drill, "drill");
    }

    /// The armed tool; `None` behaves as Select.
    pub fn armed(&self) -> FootprintTool {
        self.tool.unwrap_or(FootprintTool::Select)
    }

    /// Arm `tool`; any gesture in progress is dropped.
    pub fn arm(&mut self, tool: FootprintTool) {
        if tool != FootprintTool::Select {
            self.last = Some(tool);
            self.selection.clear();
        }
        for (shown, family) in self.shown.iter_mut().zip(FootprintTool::FAMILIES) {
            if family.contains(&tool) {
                *shown = tool;
            }
        }
        self.tool = Some(tool);
        self.pending.clear();
        self.quarter_turns = 0;
        self.arc_turn = 0.0;
    }

    /// Arm the last tool again (Shift+R, Solidcraft's repeat).
    pub fn repeat(&mut self) -> bool {
        match self.last {
            Some(tool) => {
                self.arm(tool);
                true
            }
            None => false,
        }
    }

    /// Esc: end the gesture, then the tool.
    pub fn cancel(&mut self) {
        if self.pending.is_empty() {
            if self.tool.is_none_or(|t| t == FootprintTool::Select) {
                self.selection.clear();
            }
            self.tool = None;
        }
        self.pending.clear();
        self.arc_turn = 0.0;
    }

    /// Space: turn the next pad a quarter.
    pub fn turn(&mut self) -> bool {
        if self.tool.is_some_and(FootprintTool::turns) {
            self.quarter_turns = (self.quarter_turns + 1) % 4;
            true
        } else {
            false
        }
    }

    /// Follow the pointer round an arc's centre, so the arc runs the way
    /// the hand went (as the Symbol tab's arc).
    pub fn hover(&mut self, p: Point) {
        if self.tool != Some(FootprintTool::Arc) || self.pending.len() != 2 {
            self.arc_turn = 0.0;
            return;
        }
        let (c, a) = (self.pending[0], self.pending[1]);
        if distance(c, p) <= 1e-9 || distance(c, a) <= 1e-9 {
            return;
        }
        let at = angle(c, a) + self.arc_turn;
        let step = (angle(c, p) - at + 180.0).rem_euclid(360.0) - 180.0;
        if step.is_finite() {
            self.arc_turn = (self.arc_turn + step) % 360.0;
        }
    }

    /// Whether the arc in progress runs clockwise.
    pub fn arc_clockwise(&self) -> bool {
        self.arc_turn < 0.0
    }

    /// A primary click at `at`. Returns the commit of a finished gesture;
    /// an error leaves the state as it was before the click (the gesture's
    /// points are dropped, the tool stays armed).
    pub fn press(&mut self, at: Point, scene: &Scene<'_>) -> Result<Gesture> {
        let tool = self.armed();
        match tool {
            FootprintTool::Select | FootprintTool::Dimension | FootprintTool::Constrain(_) => {
                return Ok(Gesture::None);
            }
            _ => {}
        }
        if self.pending.last().is_some_and(|p| same(*p, at)) {
            return Ok(Gesture::None);
        }
        // Click-on-pad tools resolve with their first click.
        let first_on_pad = matches!(tool, FootprintTool::Drill | FootprintTool::CustomPad);
        self.pending.push(at);
        if self.pending.len() < tool.clicks() && !first_on_pad {
            return Ok(Gesture::None);
        }
        let points = std::mem::take(&mut self.pending);
        let built = self.gesture(tool, &points, scene);
        // A line chains from its end, as in the Symbol tab.
        if tool == FootprintTool::Line
            && built.is_ok()
            && let Some(end) = points.last()
        {
            self.pending = vec![*end];
        }
        self.arc_turn = 0.0;
        built.map(Gesture::Commit)
    }

    fn gesture(
        &self,
        tool: FootprintTool,
        points: &[Point],
        scene: &Scene<'_>,
    ) -> Result<Vec<FootprintEdit>> {
        let at = |i: usize| {
            points
                .get(i)
                .copied()
                .ok_or_else(|| invalid("the gesture needs another point"))
        };
        match tool {
            FootprintTool::Pad(template) => self.place_pad(template, at(0)?, scene),
            FootprintTool::CustomPad => self.custom_pad(at(0)?, scene),
            FootprintTool::Array(kind) => self.array(kind, points, scene),
            FootprintTool::Drill => self.drill(at(0)?, scene),
            FootprintTool::Slot => self.slot(at(0)?, at(1)?, scene),
            FootprintTool::Line => self.draw(
                SketchEdit::AddLine {
                    a: at(0)?,
                    b: at(1)?,
                    construction: self.layer == ActiveLayer::Construction,
                },
                scene,
            ),
            FootprintTool::Rect => {
                let (a, b) = (at(0)?, at(1)?);
                if (a[0] - b[0]).abs() <= 1e-9 || (a[1] - b[1]).abs() <= 1e-9 {
                    return Err(invalid("a rectangle needs two opposite corners"));
                }
                self.draw(
                    SketchEdit::AddRect {
                        a,
                        b,
                        construction: self.layer == ActiveLayer::Construction,
                    },
                    scene,
                )
            }
            FootprintTool::Circle => {
                let (c, p) = (at(0)?, at(1)?);
                self.draw(
                    SketchEdit::AddCircle {
                        center: c,
                        radius: distance(c, p),
                        construction: self.layer == ActiveLayer::Construction,
                    },
                    scene,
                )
            }
            FootprintTool::Arc => {
                let (c, a, e) = (at(0)?, at(1)?, at(2)?);
                let r = distance(c, a);
                if r <= 1e-9 || distance(c, e) <= 1e-9 {
                    return Err(invalid("an arc needs a radius"));
                }
                // The end lies on the circle through the start.
                let t = angle(c, e).to_radians();
                let end = [c[0] + r * t.cos(), c[1] + r * t.sin()];
                if same(end, a) {
                    return Err(invalid("an arc needs a sweep"));
                }
                let (start, end) = if self.arc_clockwise() {
                    (end, a)
                } else {
                    (a, end)
                };
                self.draw(
                    SketchEdit::AddArc {
                        center: c,
                        start,
                        end,
                        construction: self.layer == ActiveLayer::Construction,
                    },
                    scene,
                )
            }
            FootprintTool::Select | FootprintTool::Dimension | FootprintTool::Constrain(_) => {
                Ok(Vec::new())
            }
        }
    }

    /// The template parameters of a new pad: sizes bound to named
    /// parameters (`pad_l`, `pad_w`, `pad_d`), turned by Space.
    fn pad_params(
        &self,
        build: &mut Build<'_, '_>,
        template: PadTemplate,
        turned: bool,
        kind: PadKind,
    ) -> Result<PadTemplateParams> {
        let o = &self.options.pad;
        let (width, height) = match template {
            PadTemplate::Circle => {
                let d = build.length("pad_d", o.diameter)?;
                (d.clone(), d)
            }
            _ => {
                let l = build.length("pad_l", o.size[0])?;
                let w = build.length("pad_w", o.size[1])?;
                if turned { (w, l) } else { (l, w) }
            }
        };
        let number = if kind == PadKind::NonPlatedHole || kind == PadKind::Fiducial {
            String::new()
        } else {
            build.next_number()
        };
        Ok(PadTemplateParams {
            width,
            height,
            radius_ratio: (template == PadTemplate::RoundRect)
                .then(|| Expr(crate::kit::number_text(o.radius_ratio.clamp(0.0, 0.5)))),
            kind,
            side: o.side,
            terminal: None,
            number,
        })
    }

    fn place_pad(
        &self,
        template: PadTemplate,
        at: Point,
        scene: &Scene<'_>,
    ) -> Result<Vec<FootprintEdit>> {
        let mut build = Build::new(scene)?;
        let params = self.pad_params(
            &mut build,
            template,
            self.quarter_turns % 2 == 1,
            self.options.pad.kind,
        )?;
        build.pad(template, at, params)?;
        Ok(build.edits)
    }

    /// Custom pad: free a template pad's outline (`ConvertToCustom`), or
    /// start a rectangular profile pad at `at` whose corners the author
    /// then drags.
    fn custom_pad(&self, at: Point, scene: &Scene<'_>) -> Result<Vec<FootprintEdit>> {
        if let Some(role) = role_at(scene, at) {
            return match role.geometry {
                PadGeometry::Template { .. } => {
                    Ok(vec![FootprintEdit::ConvertToCustom { pad: role.pad }])
                }
                PadGeometry::Profile => Err(invalid(format!(
                    "pad {} already has a custom outline",
                    role.number
                ))),
            };
        }
        let mut build = Build::new(scene)?;
        let params = self.pad_params(
            &mut build,
            PadTemplate::Rect,
            self.quarter_turns % 2 == 1,
            self.options.pad.kind,
        )?;
        let role = build.pad(PadTemplate::Rect, at, params)?;
        build.push(FootprintEdit::ConvertToCustom { pad: role.pad });
        Ok(build.edits)
    }

    fn array(
        &self,
        kind: ArrayTool,
        points: &[Point],
        scene: &Scene<'_>,
    ) -> Result<Vec<FootprintEdit>> {
        let o = &self.options.array;
        let first = points
            .first()
            .copied()
            .ok_or_else(|| invalid("an array needs its first pad"))?;
        let turned = self.quarter_turns % 2 == 1;
        let mut build = Build::new(scene)?;
        let count = |n: u16| -> Result<Expr> {
            if n == 0 || n > 1024 {
                return Err(invalid("an array count is 1–1024"));
            }
            Ok(Expr(n.to_string()))
        };
        match kind {
            ArrayTool::Linear | ArrayTool::DualRow => {
                let toward = points
                    .get(1)
                    .copied()
                    .ok_or_else(|| invalid("pick the row direction"))?;
                let d = sub(toward, first);
                let len = distance(toward, first);
                if len <= 1e-9 {
                    return Err(invalid("the row direction needs a second point"));
                }
                // Pads stand across a row: long side across the direction.
                let across = d[1].abs() > d[0].abs();
                let pitch = build.length("pitch", o.pitch)?;
                let params = self.pad_params(
                    &mut build,
                    o.template,
                    across == turned,
                    self.options.pad.kind,
                )?;
                let first_number = params.number.clone();
                let seed = build.pad(o.template, first, params)?;
                let direction = build.direction_line(first, add(first, scale(d, 1.0 / len)))?;
                let layout = if kind == ArrayTool::Linear {
                    ArrayLayout::Linear {
                        count: count(o.count)?,
                        pitch,
                        direction,
                    }
                } else {
                    ArrayLayout::DualRow {
                        per_row: count(o.per_row)?,
                        pitch,
                        row_span: build.length("row_span", o.row_span)?,
                        direction,
                    }
                };
                build.push(FootprintEdit::AddArray {
                    seed: seed.pad,
                    layout,
                    numbering: if kind == ArrayTool::Linear {
                        NumberingScheme::Sequential
                    } else {
                        NumberingScheme::CounterClockwise
                    },
                    skip: SkipRule::default(),
                    first_number,
                });
            }
            ArrayTool::Quad => {
                let (nx, ny) = (o.nx, o.ny);
                let [sx, sy] = o.span;
                let side = match o.pin1 {
                    Pin1Position::Left => 0,
                    Pin1Position::Bottom => 1,
                    Pin1Position::Right => 2,
                    Pin1Position::Top => 3,
                };
                let n = if side % 2 == 0 { ny } else { nx };
                let t = f64::from(n.saturating_sub(1)) * 0.5 * o.pitch;
                // Pin 1: the first pad of its side, counter-clockwise.
                let offset = match side {
                    0 => [-sx * 0.5, t],
                    1 => [-t, -sy * 0.5],
                    2 => [sx * 0.5, -t],
                    _ => [t, sy * 0.5],
                };
                let pitch = build.length("pitch", o.pitch)?;
                let span_x = build.length("span_x", sx)?;
                let span_y = build.length("span_y", sy)?;
                let params = self.pad_params(
                    &mut build,
                    o.template,
                    (side % 2 == 1) != turned,
                    self.options.pad.kind,
                )?;
                let first_number = params.number.clone();
                let seed = build.pad(o.template, add(first, offset), params)?;
                build.push(FootprintEdit::AddArray {
                    seed: seed.pad,
                    layout: ArrayLayout::Quad {
                        nx: count(nx)?,
                        ny: count(ny)?,
                        pitch,
                        span_x,
                        span_y,
                        pin1: o.pin1,
                    },
                    numbering: NumberingScheme::CounterClockwise,
                    skip: SkipRule::default(),
                    first_number,
                });
            }
            ArrayTool::Grid => {
                let pitch_x = build.length("pitch_x", o.grid_pitch[0])?;
                let pitch_y = build.length("pitch_y", o.grid_pitch[1])?;
                let params =
                    self.pad_params(&mut build, o.template, turned, self.options.pad.kind)?;
                let first_number = if o.alpha {
                    String::new()
                } else {
                    params.number.clone()
                };
                let seed = build.pad(o.template, first, params)?;
                let void = (o.void[0] > 0 && o.void[1] > 0).then_some(o.void);
                build.push(FootprintEdit::AddArray {
                    seed: seed.pad,
                    layout: ArrayLayout::Grid {
                        rows: count(o.rows)?,
                        cols: count(o.cols)?,
                        pitch_x,
                        pitch_y,
                        stagger: o.stagger,
                    },
                    numbering: if o.alpha {
                        NumberingScheme::GridAlpha {
                            skip_letters: "IOQSXZ".into(),
                        }
                    } else {
                        NumberingScheme::Sequential
                    },
                    skip: SkipRule {
                        explicit: Vec::new(),
                        centre_void: void,
                        perimeter_depth: (o.perimeter > 0).then_some(o.perimeter),
                    },
                    first_number,
                });
            }
        }
        Ok(build.edits)
    }

    /// Drill: a concentric round hole on the role pad at `at` (bound to the
    /// `drill` parameter), or an NPTH pad there when no pad is hit.
    fn drill(&self, at: Point, scene: &Scene<'_>) -> Result<Vec<FootprintEdit>> {
        let mut build = Build::new(scene)?;
        let role = match role_at(scene, at) {
            Some(role) => role,
            None => {
                let drill = build.length("drill", self.options.drill)?;
                build.pad(PadTemplate::Circle, at, bare(drill.clone(), drill))?
            }
        };
        if role.hole.is_some() {
            return Err(invalid(format!("pad {} already has a hole", role.number)));
        }
        let centre = build.point_of(&role.anchor)?;
        let drill = build.param_name("drill", self.options.drill)?;
        let created = build.sketch(SketchEdit::AddCircle {
            center: centre,
            radius: self.options.drill * 0.5,
            construction: true,
        })?;
        let circle = last_curve(&created)?;
        let hole_centre = first_point(&created)?;
        build.sketch(SketchEdit::AddConstraint {
            kind: GeomConstraint::Coincident,
            refs: vec![
                SketchRef::Point(hole_centre),
                SketchRef::Point(role.anchor.clone()),
            ],
        })?;
        build.sketch(SketchEdit::AddDimension {
            kind: DimKind::Diameter,
            refs: vec![SketchRef::Curve(circle.clone())],
            value: DimValue::Param(drill),
        })?;
        let mut next = role;
        next.hole = Some(HoleRole {
            curves: vec![circle],
            plated: next.kind != PadKind::NonPlatedHole,
        });
        if next.kind == PadKind::Smd {
            next.kind = PadKind::PlatedThrough;
        }
        build.push(FootprintEdit::SetPadRole { role: next });
        Ok(build.edits)
    }

    /// Slot: a stadium hole between two end centres on the role pad under
    /// their midpoint, or an NPTH oblong pad when no pad is hit.
    fn slot(&self, a: Point, b: Point, scene: &Scene<'_>) -> Result<Vec<FootprintEdit>> {
        let len = distance(a, b);
        if len <= 1e-9 {
            return Err(invalid("a slot needs two different ends"));
        }
        let mid = scale(add(a, b), 0.5);
        let r = self.options.drill * 0.5;
        let mut build = Build::new(scene)?;
        let role = match role_at(scene, mid) {
            Some(role) => role,
            None => {
                let along_y = (b[1] - a[1]).abs() > (b[0] - a[0]).abs();
                if (b[0] - a[0]).abs() > 1e-9 && (b[1] - a[1]).abs() > 1e-9 {
                    return Err(invalid(
                        "a slot without a pad runs horizontally or vertically",
                    ));
                }
                let long = Expr(mm(len + 2.0 * r));
                let short = Expr(mm(2.0 * r));
                let params = if along_y {
                    bare(short, long)
                } else {
                    bare(long, short)
                };
                build.pad(PadTemplate::Oblong, mid, params)?
            }
        };
        if role.hole.is_some() {
            return Err(invalid(format!("pad {} already has a hole", role.number)));
        }
        let u = scale(sub(b, a), 1.0 / len);
        let n = scale(left(u), r);
        let mut curves = Vec::new();
        // Two half arcs and two sides, counter-clockwise.
        for (c, from, to) in [(b, sub(b, n), add(b, n)), (a, add(a, n), sub(a, n))] {
            let created = build.sketch(SketchEdit::AddArc {
                center: c,
                start: from,
                end: to,
                construction: true,
            })?;
            curves.push(last_curve(&created)?);
        }
        for (p, q) in [(add(b, n), add(a, n)), (sub(a, n), sub(b, n))] {
            let created = build.sketch(SketchEdit::AddLine {
                a: p,
                b: q,
                construction: true,
            })?;
            curves.push(last_curve(&created)?);
        }
        let mut next = role;
        next.hole = Some(HoleRole {
            curves,
            plated: next.kind != PadKind::NonPlatedHole,
        });
        if next.kind == PadKind::Smd {
            next.kind = PadKind::PlatedThrough;
        }
        build.push(FootprintEdit::SetPadRole { role: next });
        Ok(build.edits)
    }

    /// A drawn curve: construction geometry, or a graphic role on the
    /// active layer owning the curves the edit creates. Axis-aligned lines
    /// are constrained horizontal or vertical, and an end placed on an
    /// existing sketch point is made coincident with it.
    fn draw(&self, edit: SketchEdit, scene: &Scene<'_>) -> Result<Vec<FootprintEdit>> {
        let mut build = Build::new(scene)?;
        let line = match &edit {
            SketchEdit::AddLine { a, b, .. } => Some((*a, *b)),
            _ => None,
        };
        let created = build.sketch(edit)?;
        let curves: Vec<SketchCurveId> = created
            .iter()
            .filter_map(|r| match r {
                SketchRef::Curve(c) => Some(c.clone()),
                _ => None,
            })
            .collect();
        if let (Some((a, b)), Some(curve)) = (line, curves.first()) {
            let kind = if (a[1] - b[1]).abs() <= 1e-9 {
                Some(GeomConstraint::Horizontal)
            } else if (a[0] - b[0]).abs() <= 1e-9 {
                Some(GeomConstraint::Vertical)
            } else {
                None
            };
            if let Some(kind) = kind {
                build.sketch(SketchEdit::AddConstraint {
                    kind,
                    refs: vec![SketchRef::Curve(curve.clone())],
                })?;
            }
        }
        for new in created.iter().filter_map(|r| match r {
            SketchRef::Point(p) => Some(p.clone()),
            _ => None,
        }) {
            let Ok(at) = build.point_of(&new) else {
                continue;
            };
            if let Some(old) = scene
                .view
                .points
                .iter()
                .find(|p| same(p.at, at) && !owned_point(scene, &p.id))
            {
                build.sketch(SketchEdit::AddConstraint {
                    kind: GeomConstraint::Coincident,
                    refs: vec![
                        SketchRef::Point(new.clone()),
                        SketchRef::Point(old.id.clone()),
                    ],
                })?;
            }
        }
        if let Some(layer) = self.layer.drawing_layer() {
            build.push(FootprintEdit::SetGraphicRole {
                role: GraphicRole {
                    id: GraphicId(0),
                    layer,
                    side: Side::Component,
                    source: GraphicSource::Curves(curves),
                    width: Expr(self.layer.width().into()),
                    fill: Fill::None,
                },
            });
        }
        Ok(build.edits)
    }

    /// The pick under `at`: a free sketch point, then a pad by its land,
    /// then a sketch curve (template outlines belong to their pad).
    pub fn pick(&self, at: Point, scene: &Scene<'_>) -> Option<Pick> {
        let tol = scene.tolerance;
        let point = scene
            .view
            .points
            .iter()
            .filter(|p| !owned_point(scene, &p.id) && p.id.0 != "origin")
            .map(|p| (distance(p.at, at), p))
            .filter(|(d, _)| *d <= tol)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, p)) = point {
            return Some(Pick::Point(p.id.clone()));
        }
        if let Some(i) =
            super::footprint_geometry::pick(&scene.footprint.pads, at, tol, scene.chord)
            && let Some(pad) = scene.footprint.pads.get(i)
        {
            return Some(Pick::Pad(pad.id));
        }
        scene
            .view
            .curves
            .iter()
            .filter(|c| !owned_curve(scene, &c.id))
            .filter_map(|c| curve_distance(&c.geometry, at).map(|d| (d, c)))
            .filter(|(d, _)| *d <= tol)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, c)| Pick::Curve(c.id.clone()))
    }

    /// A Select click: pick into the selection (Shift adds and toggles).
    pub fn select(&mut self, at: Point, scene: &Scene<'_>, additive: bool) {
        let picked = self.pick(at, scene);
        if !additive {
            self.selection.clear();
        }
        if let Some(p) = picked {
            match self.selection.iter().position(|q| *q == p) {
                Some(i) if additive => {
                    self.selection.remove(i);
                }
                Some(_) => {}
                None => self.selection.push(p),
            }
        }
    }

    /// The sketch entities moving a selection carries, and the arrays whose
    /// instances are in it (moved through their seed).
    fn moved(&self, scene: &Scene<'_>) -> Result<Vec<SketchRef>> {
        let source = scene
            .source()
            .ok_or_else(|| invalid("the footprint has no sketch"))?;
        let mut ids: Vec<SketchRef> = Vec::new();
        let push = |r: SketchRef, ids: &mut Vec<SketchRef>| {
            if !ids.contains(&r) {
                ids.push(r);
            }
        };
        for pick in &self.selection {
            match pick {
                Pick::Point(p) => push(SketchRef::Point(p.clone()), &mut ids),
                Pick::Curve(c) => push(SketchRef::Curve(c.clone()), &mut ids),
                Pick::Pad(pad) => {
                    let pad = seed_of(source, *pad);
                    let Some(role) = source.pads.iter().find(|r| r.pad == pad) else {
                        return Err(invalid("this pad has no role in the sketch"));
                    };
                    for r in role_entities(scene, role) {
                        push(r, &mut ids);
                    }
                    for a in source.arrays.iter().filter(|a| a.seed == pad) {
                        if let ArrayLayout::Linear { direction, .. }
                        | ArrayLayout::DualRow { direction, .. } = &a.layout
                        {
                            push(SketchRef::Curve(direction.clone()), &mut ids);
                        }
                    }
                }
            }
        }
        Ok(ids)
    }

    /// Release of a Select drag: one `Move` of the selection by `delta`.
    pub fn move_edits(&self, delta: Point, scene: &Scene<'_>) -> Result<Gesture> {
        if self.selection.is_empty() || distance(delta, [0.0, 0.0]) <= 1e-9 {
            return Ok(Gesture::None);
        }
        let ids = self.moved(scene)?;
        if ids.is_empty() {
            return Ok(Gesture::None);
        }
        Ok(Gesture::Commit(vec![FootprintEdit::Sketch(
            SketchEdit::Move { ids, delta },
        )]))
    }

    /// Delete: remove the selected pads and sketch entities in one commit.
    /// A generated array instance is skipped in its array; a seed takes its
    /// array with it.
    pub fn delete_edits(&self, scene: &Scene<'_>) -> Result<Gesture> {
        let Some(source) = scene.source() else {
            return Ok(Gesture::None);
        };
        let mut edits = Vec::new();
        let mut sketch: Vec<SketchRef> = Vec::new();
        let mut arrays: Vec<pcbcraft_core::library::footprint_source::PadArray> = Vec::new();
        for pick in &self.selection {
            match pick {
                Pick::Point(p) => sketch.push(SketchRef::Point(p.clone())),
                Pick::Curve(c) => sketch.push(SketchRef::Curve(c.clone())),
                Pick::Pad(pad) => {
                    if let Some(k) = source.keys.iter().find(|k| k.pad == *pad)
                        && let GenerationKey::Array { array, i, j } = k.key
                        && let Some(a) = source.arrays.iter().find(|a| a.id == array)
                    {
                        if a.seed == *pad || (i, j) == (0, 0) {
                            edits.push(FootprintEdit::RemoveArray { id: a.id });
                            edits.push(FootprintEdit::RemovePad { pad: a.seed });
                            continue;
                        }
                        let slot = match arrays.iter_mut().find(|x| x.id == a.id) {
                            Some(slot) => slot,
                            None => {
                                arrays.push(a.clone());
                                let Some(slot) = arrays.last_mut() else {
                                    continue;
                                };
                                slot
                            }
                        };
                        if !slot.skip.explicit.contains(&[i, j]) {
                            slot.skip.explicit.push([i, j]);
                        }
                        continue;
                    }
                    for a in source.arrays.iter().filter(|a| a.seed == *pad) {
                        edits.push(FootprintEdit::RemoveArray { id: a.id });
                    }
                    if source.pads.iter().any(|r| r.pad == *pad) {
                        edits.push(FootprintEdit::RemovePad { pad: *pad });
                    }
                }
            }
        }
        let removed: Vec<_> = edits
            .iter()
            .filter_map(|e| match e {
                FootprintEdit::RemoveArray { id } => Some(*id),
                _ => None,
            })
            .collect();
        edits.extend(
            arrays
                .into_iter()
                .filter(|a| !removed.contains(&a.id))
                .map(|array| FootprintEdit::EditArray { array }),
        );
        edits.dedup();
        if !sketch.is_empty() {
            edits.push(FootprintEdit::Sketch(SketchEdit::Delete { ids: sketch }));
        }
        Ok(if edits.is_empty() {
            Gesture::None
        } else {
            Gesture::Commit(edits)
        })
    }
}

/// A bare (non-plated, unnumbered) hole pad's template parameters.
fn bare(width: Expr, height: Expr) -> PadTemplateParams {
    PadTemplateParams {
        width,
        height,
        radius_ratio: None,
        kind: PadKind::NonPlatedHole,
        side: PadSide::Top,
        terminal: None,
        number: String::new(),
    }
}

/// The role pad an array instance (or a role pad itself) moves with.
fn seed_of(source: &FootprintSource, pad: PadId) -> PadId {
    source
        .keys
        .iter()
        .find(|k| k.pad == pad)
        .and_then(|k| {
            let GenerationKey::Array { array, .. } = k.key;
            source.arrays.iter().find(|a| a.id == array)
        })
        .map_or(pad, |a| a.seed)
}

/// A role pad's sketch entities: its anchor, owned outline and hole; a
/// custom (profile) pad's free outline curves inside its land.
fn role_entities(scene: &Scene<'_>, role: &PadRole) -> Vec<SketchRef> {
    let mut out = vec![SketchRef::Point(role.anchor.clone())];
    match &role.geometry {
        PadGeometry::Template { owned_curves, .. } => {
            out.extend(owned_curves.iter().cloned().map(SketchRef::Curve));
        }
        PadGeometry::Profile => {
            if let Some(pad) = scene.footprint.pads.iter().find(|p| p.id == role.pad) {
                let b = pad.bounds().inflated(1e-6);
                out.extend(
                    scene
                        .view
                        .curves
                        .iter()
                        .filter(|c| !c.construction)
                        .filter(|c| {
                            !c.points.is_empty()
                                && c.points
                                    .iter()
                                    .filter_map(|p| scene.view.point(p))
                                    .all(|p| b.contains(p.at))
                        })
                        .map(|c| SketchRef::Curve(c.id.clone())),
                );
            }
        }
    }
    if let Some(h) = &role.hole {
        out.extend(h.curves.iter().cloned().map(SketchRef::Curve));
    }
    if let Some(o) = &role.orientation {
        out.push(SketchRef::Curve(o.clone()));
    }
    out
}

/// Curves a role owns (template outlines and holes): never picked alone.
fn owned_curve(scene: &Scene<'_>, id: &SketchCurveId) -> bool {
    scene.source().is_some_and(|s| {
        s.pads.iter().any(|r| {
            matches!(&r.geometry, PadGeometry::Template { owned_curves, .. } if owned_curves.contains(id))
                || r.hole.as_ref().is_some_and(|h| h.curves.contains(id))
        })
    })
}

/// Points of owned curves and pad anchors: they move with their pad.
fn owned_point(scene: &Scene<'_>, id: &SketchPointId) -> bool {
    let Some(source) = scene.source() else {
        return false;
    };
    source.pads.iter().any(|r| r.anchor == *id)
        || scene
            .view
            .curves
            .iter()
            .any(|c| c.points.contains(id) && owned_curve(scene, &c.id))
}

/// The role pad whose land contains `at`; an array instance answers with
/// nothing (holes go on role pads).
fn role_at(scene: &Scene<'_>, at: Point) -> Option<PadRole> {
    let source = scene.source()?;
    let i = super::footprint_geometry::pick(&scene.footprint.pads, at, 0.0, scene.chord)?;
    let pad = scene.footprint.pads.get(i)?;
    source.pads.iter().find(|r| r.pad == pad.id).cloned()
}

fn curve_distance(g: &ViewGeometry, p: Point) -> Option<f64> {
    use pcbcraft_core::geometry::segment_distance;
    match g {
        ViewGeometry::Line { a, b } => Some(segment_distance(p, *a, *b)),
        ViewGeometry::Circle { center, radius } => Some((distance(*center, p) - radius).abs()),
        ViewGeometry::Arc { center, start, end } => {
            let r = distance(*center, *start);
            let (a0, a1, t) = (
                angle(*center, *start),
                angle(*center, *end),
                angle(*center, p),
            );
            let sweep = (a1 - a0).rem_euclid(360.0);
            let within = (t - a0).rem_euclid(360.0) <= sweep;
            Some(if within {
                (distance(*center, p) - r).abs()
            } else {
                distance(p, *start).min(distance(p, *end))
            })
        }
        ViewGeometry::Other => None,
    }
}

fn last_curve(created: &[SketchRef]) -> Result<SketchCurveId> {
    created
        .iter()
        .rev()
        .find_map(|r| match r {
            SketchRef::Curve(c) => Some(c.clone()),
            _ => None,
        })
        .ok_or_else(|| invalid("the sketch edit created no curve"))
}

fn first_point(created: &[SketchRef]) -> Result<SketchPointId> {
    created
        .iter()
        .find_map(|r| match r {
            SketchRef::Point(p) => Some(p.clone()),
            _ => None,
        })
        .ok_or_else(|| invalid("the sketch edit created no point"))
}

/// Builds one gesture's edits, staging what it needs to learn the ids the
/// engine will allocate for them.
struct Build<'s, 'a> {
    scene: &'s Scene<'a>,
    edits: Vec<FootprintEdit>,
    /// The sketch after `edits`.
    sketch: SketchBlob,
    /// The staged source after the last `pad` (roles and parameters).
    staged: Option<FootprintSource>,
    /// Numbers taken by pads placed in this gesture.
    numbers: Vec<String>,
}

impl<'s, 'a> Build<'s, 'a> {
    fn new(scene: &'s Scene<'a>) -> Result<Self> {
        if scene.footprint.source.is_none() && !scene.footprint.pads.is_empty() {
            return Err(Error::Conflict(
                "footprint.explicit: this footprint has explicit pads; sketch tools start on a footprint without pads"
                    .into(),
            ));
        }
        let sketch = match scene.source() {
            Some(s) => s.sketch.clone(),
            None => scene.evaluator.new_sketch()?,
        };
        Ok(Self {
            scene,
            edits: Vec::new(),
            sketch,
            staged: None,
            numbers: Vec::new(),
        })
    }

    fn push(&mut self, edit: FootprintEdit) {
        self.edits.push(edit);
    }

    fn source(&self) -> Option<&FootprintSource> {
        self.staged.as_ref().or(self.scene.source())
    }

    /// Apply a sketch edit; the entities it creates.
    fn sketch(&mut self, edit: SketchEdit) -> Result<Vec<SketchRef>> {
        let out = self.scene.evaluator.apply_sketch(&self.sketch, &edit)?;
        self.sketch = out.sketch;
        self.edits.push(FootprintEdit::Sketch(edit));
        Ok(out.created)
    }

    /// Add a template pad and stage the gesture so far; its role.
    fn pad(
        &mut self,
        template: PadTemplate,
        at: Point,
        params: PadTemplateParams,
    ) -> Result<PadRole> {
        self.edits.push(FootprintEdit::AddPad {
            template,
            at,
            params,
        });
        let preview = footprint_edit::preview(
            self.scene.footprint,
            self.scene.package,
            &self.edits,
            self.scene.evaluator,
        )?;
        // Template pads come first in the report, in edit order.
        let added = self
            .edits
            .iter()
            .filter(|e| matches!(e, FootprintEdit::AddPad { .. }))
            .count();
        let pad = added
            .checked_sub(1)
            .and_then(|i| preview.report.pads.get(i))
            .copied()
            .ok_or_else(|| invalid("the pad was not added"))?;
        let source = preview
            .definition
            .source
            .map(|s| *s)
            .ok_or_else(|| invalid("the footprint has no sketch"))?;
        let role = source
            .pads
            .iter()
            .find(|r| r.pad == pad)
            .cloned()
            .ok_or_else(|| invalid("the pad has no role"))?;
        self.sketch = source.sketch.clone();
        self.staged = Some(source);
        Ok(role)
    }

    /// A sketch point's stored position in the staged sketch.
    fn point_of(&self, id: &SketchPointId) -> Result<Point> {
        let view = pcbcraft_solidcraft::footprint::view::sketch_view(&FootprintSource {
            sketch: self.sketch.clone(),
            ..FootprintSource::default()
        })?;
        view.point(id)
            .map(|p| p.at)
            .ok_or_else(|| invalid(format!("no sketch point `{}`", id.0)))
    }

    /// A construction direction line from `a` to `b`, constrained
    /// horizontal or vertical when it is.
    fn direction_line(&mut self, a: Point, b: Point) -> Result<SketchCurveId> {
        let created = self.sketch(SketchEdit::AddLine {
            a,
            b,
            construction: true,
        })?;
        let line = last_curve(&created)?;
        let kind = if (a[1] - b[1]).abs() <= 1e-9 {
            Some(GeomConstraint::Horizontal)
        } else if (a[0] - b[0]).abs() <= 1e-9 {
            Some(GeomConstraint::Vertical)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.sketch(SketchEdit::AddConstraint {
                kind,
                refs: vec![SketchRef::Curve(line.clone())],
            })?;
        }
        Ok(line)
    }

    /// A length parameter named `base` (or `base_2`, … when `base` holds
    /// another value) set to `value`; its name.
    fn param_name(&mut self, base: &str, value: f64) -> Result<ParamName> {
        if !(value.is_finite() && value > 0.0) {
            return Err(invalid(format!("{base} must be a positive length")));
        }
        let text = mm(value);
        let existing = |name: &str, this: &Self| -> Option<String> {
            let staged = this.edits.iter().rev().find_map(|e| match e {
                FootprintEdit::SetParameter { name: n, expr, .. } if n.0 == name => {
                    Some(expr.0.clone())
                }
                _ => None,
            });
            staged.or_else(|| {
                this.source().and_then(|s| {
                    s.parameters
                        .iter()
                        .find(|p| p.name.0 == name)
                        .map(|p| p.expr.0.clone())
                })
            })
        };
        for k in 1..=64u32 {
            let name = if k == 1 {
                base.to_owned()
            } else {
                format!("{base}_{k}")
            };
            match existing(&name, self) {
                Some(expr) if expr == text => return Ok(ParamName(name)),
                Some(_) => continue,
                None => {
                    self.edits.push(FootprintEdit::SetParameter {
                        name: ParamName(name.clone()),
                        expr: Expr(text),
                        unit: ParamUnit::Mm,
                    });
                    return Ok(ParamName(name));
                }
            }
        }
        Err(Error::Limit(format!("too many `{base}` parameters")))
    }

    fn length(&mut self, base: &str, value: f64) -> Result<Expr> {
        self.param_name(base, value).map(|n| Expr(n.0))
    }

    /// The next free pad number. A sketched pad lands no package
    /// terminal: binding is the explicit `library.assign_terminals`
    /// (nothing is inferred from matching numbers).
    fn next_number(&mut self) -> String {
        let fp = self.scene.footprint;
        let taken = |n: &str, this: &Self| {
            fp.pads.iter().any(|p| p.number == n)
                || this
                    .source()
                    .is_some_and(|s| s.pads.iter().any(|r| r.number == n))
                || this.numbers.iter().any(|m| m == n)
        };
        let number = (1..=u32::MAX)
            .map(|n| n.to_string())
            .find(|n| !taken(n, self))
            .unwrap_or_default();
        self.numbers.push(number.clone());
        number
    }
}

/// The one operation a finished gesture sends.
pub fn commit(draft: DefinitionId, edits: Vec<FootprintEdit>) -> Operation {
    Operation::Library(LibraryCommand::EditFootprint { draft, edits })
}

/// Pad centres an array gesture would make, for the ghost preview: the
/// same layout rules as the evaluator's expansion, without skips.
pub fn ghost_centres(state: &ToolState, hover: Point) -> Vec<Point> {
    let o = &state.options.array;
    let Some(FootprintTool::Array(kind)) = state.tool else {
        return Vec::new();
    };
    let first = state.pending.first().copied().unwrap_or(hover);
    let cap = |n: u16| n.min(256);
    match kind {
        ArrayTool::Linear | ArrayTool::DualRow => {
            let toward = if state.pending.is_empty() {
                add(hover, [0.0, -1.0])
            } else {
                hover
            };
            let len = distance(toward, first);
            if len <= 1e-9 {
                return vec![first];
            }
            let d = scale(sub(toward, first), 1.0 / len);
            let n = if kind == ArrayTool::Linear {
                cap(o.count)
            } else {
                cap(o.per_row)
            };
            let row: Vec<Point> = (0..n)
                .map(|j| add(first, scale(d, f64::from(j) * o.pitch)))
                .collect();
            if kind == ArrayTool::Linear {
                return row;
            }
            let centre = add(
                add(first, scale(left(d), o.row_span * 0.5)),
                scale(d, f64::from(n.saturating_sub(1)) * o.pitch * 0.5),
            );
            let other = row.iter().map(|p| sub(scale(centre, 2.0), *p));
            row.iter().copied().chain(other).collect()
        }
        ArrayTool::Quad => {
            let [sx, sy] = o.span;
            let side =
                |n: u16, k: u16| (f64::from(n.saturating_sub(1)) * 0.5 - f64::from(k)) * o.pitch;
            let mut out = Vec::new();
            for k in 0..cap(o.ny) {
                out.push(add(first, [-sx * 0.5, side(o.ny, k)]));
                out.push(add(first, [sx * 0.5, -side(o.ny, k)]));
            }
            for k in 0..cap(o.nx) {
                out.push(add(first, [-side(o.nx, k), -sy * 0.5]));
                out.push(add(first, [side(o.nx, k), sy * 0.5]));
            }
            out
        }
        ArrayTool::Grid => {
            let [px, py] = o.grid_pitch;
            let mut out = Vec::new();
            for i in 0..cap(o.rows).min(64) {
                let shift = if o.stagger && i % 2 == 1 {
                    px * 0.5
                } else {
                    0.0
                };
                for j in 0..cap(o.cols).min(64) {
                    out.push(add(first, [f64::from(j) * px + shift, -f64::from(i) * py]));
                }
            }
            out
        }
    }
}
