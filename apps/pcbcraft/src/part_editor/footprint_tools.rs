//! The Footprint tab's sketch tools (slice F4) as a small state machine,
//! following the Symbol drawing tools (`drawing.rs`, adapted from
//! Solidcraft's sketch tools): clicked points collect until a gesture is
//! complete, Esc or a right click ends the gesture and then the tool, the
//! tool stays armed after a placement, and snapping happens before a point
//! reaches the tool. Nothing here touches the document: a finished gesture
//! returns the [`FootprintEdit`]s of one `library.edit_footprint` commit.
//!
//! Contract (docs/f45-contract.md): the tool vocabulary, ids, the active
//! layer and [`Gesture`] are fixed; the tools task fills in the gestures
//! and the ribbon. Dimension and constraint gestures belong to
//! `footprint_dims`.
#![expect(
    dead_code,
    reason = "F4/F5 contract: the tools task wires the gestures and the ribbon"
)]
use pcbcraft_core::library::footprint_source::{FootprintEdit, GeomConstraint, PadTemplate};
use pcbcraft_core::library::graphics::DrawingLayer;
use pcbcraft_core::{DefinitionId, LibraryCommand, Operation, Point};

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

/// The armed tool and its pending points. Points are millimetres in the
/// footprint frame, already snapped.
#[derive(Clone, Debug, Default)]
pub struct ToolState {
    pub tool: Option<FootprintTool>,
    pub layer: ActiveLayer,
    /// Points collected for the gesture in progress.
    pub pending: Vec<Point>,
    /// Quarter turns (Space) applied to the next placed pad.
    pub quarter_turns: u8,
}

impl ToolState {
    /// Arm `tool`; any gesture in progress is dropped.
    pub fn arm(&mut self, tool: FootprintTool) {
        self.tool = Some(tool);
        self.pending.clear();
        self.quarter_turns = 0;
    }

    /// Esc: end the gesture, then the tool.
    pub fn cancel(&mut self) {
        if self.pending.is_empty() {
            self.tool = None;
        }
        self.pending.clear();
    }

    /// A primary click at `at`.
    pub fn press(&mut self, _at: Point) -> Gesture {
        Gesture::None
    }
}

/// The one operation a finished gesture sends.
pub fn commit(draft: DefinitionId, edits: Vec<FootprintEdit>) -> Operation {
    Operation::Library(LibraryCommand::EditFootprint { draft, edits })
}
