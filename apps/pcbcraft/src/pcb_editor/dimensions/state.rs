//! The data both views share: a pick, a drawn dimension, the value open at
//! a label, the tool's state, the words a reference reads in, and the one
//! dimension selection (with the hover the Properties card shares).
use super::view_2d::Layout2d;
use crate::pcb_editor::PcbEditor;
use egui::{Pos2, Rect};
use pcbcraft_core::layout::dimensions::{self, FeatureGeometry};
use pcbcraft_core::layout::outline::BoardShapeEvaluator;
use pcbcraft_core::library::footprint_source::{DimValue, SketchRef};
use pcbcraft_core::model::board::ConstraintTarget;
use pcbcraft_core::model::dimension::{
    BoardRef, BoxSide, FeatureRef, PartDimension, PartDimensionSpec, ReferenceStatus,
};
use pcbcraft_core::{CopperLayer, Id, Layer, Project};
use pcbcraft_solidcraft::geom::Vec2;

/// Decimals of a drawn length (Solidcraft's default).
pub(super) const DECIMALS: u8 = 2;
/// The value field over the canvas, points wide.
pub(super) const FIELD_W: f32 = 120.0;
/// Points from a dimension line or label that still count as on it
/// (Solidcraft's `dim_view` hit reach).
pub(super) const HIT_PT: f32 = 4.0;

pub(super) fn field_id() -> egui::Id {
    egui::Id::unique("pcb-3d-dimension-value")
}

/// The value of a drawn dimension, edited in place.
pub(super) fn edit_id() -> egui::Id {
    egui::Id::unique("pcb-3d-dimension-edit")
}

/// One reference picked for the dimension being made, in either view.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Picked {
    pub(super) reference: BoardRef,
    /// Its geometry in the board.
    pub(super) geometry: FeatureGeometry,
    /// The height it is drawn at (mm): the part's seat, or the board top.
    pub(super) z: f64,
    /// The card's words for it ("J1 · front face").
    pub(super) words: String,
}

/// A dimension as drawn last frame, for hover, clicks and its value
/// edited in place.
#[derive(Clone, Debug)]
pub(super) struct Drawn {
    pub(super) id: Id,
    /// The label's text box (screen).
    pub(super) text: Rect,
    /// Its extension and dimension lines (screen).
    pub(super) segs: Vec<[Pos2; 2]>,
    /// Where its extension lines start, on its two references (board mm);
    /// what the tests check the drawing against.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) ends: [[f64; 3]; 2],
    /// What the label reads.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) label: String,
    /// Drawn lit: selected, edited, or hovered here or in its row.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) hot: bool,
}

impl Drawn {
    pub(super) fn on_text(&self, p: Pos2) -> bool {
        self.text.expand(2.0).contains(p)
    }
    pub(super) fn on_line(&self, p: Pos2) -> bool {
        self.segs.iter().any(|s| seg_dist(p, s[0], s[1]) <= HIT_PT)
    }
}

/// How far `p` is from the segment `a`–`b` (points).
pub(super) fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 {
        ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// A drawn dimension's value, open for editing at its label.
#[derive(Clone, Debug)]
pub(super) struct InPlace {
    pub(super) id: Id,
    pub(super) text: String,
    /// Frames the field takes the keyboard back in.
    pub(super) fresh: u8,
    /// Something was typed: until then the whole value is selected, so
    /// the first key replaces it.
    pub(super) owned: bool,
    /// Why the value was refused, by the field or by the engine.
    pub(super) error: Option<String>,
    /// The revision the edit was sent at, until the engine has answered.
    pub(super) sent: Option<u64>,
}

/// The Dimension tool and the drawn dimensions' state, one for both views
/// (only one is drawn at a time).
#[derive(Default)]
pub(in crate::pcb_editor) struct Dimensions {
    pub(super) picks: Vec<Picked>,
    /// The value being typed, once two picks measure.
    pub(super) value: Option<String>,
    /// Frames the value field takes the keyboard back in.
    pub(super) fresh: u8,
    /// Why the last pick or value was refused.
    pub(super) error: Option<String>,
    /// The dimension sent, to select once the engine added it.
    pub(super) sent: Option<PartDimensionSpec>,
    /// Dimensions drawn last frame, for clicks and hover.
    pub(super) drawn: Vec<Drawn>,
    /// Where the draft's text was drawn last frame.
    pub(super) draft_at: Option<Pos2>,
    /// A drawn dimension's value being edited in place.
    pub(super) edit: Option<InPlace>,
    /// The dimension under the pointer in the view last frame.
    pub(super) hovered: Option<Id>,
    /// Layout's own picking: the parts' body traces and the chooser.
    pub(super) layout: Layout2d,
}

impl Dimensions {
    /// The view switched (Layout ⇄ 3D): the picks stay, Layout's chooser
    /// and the hover go.
    pub(in crate::pcb_editor) fn view_switched(&mut self) {
        self.layout.choosing = None;
        self.hovered = None;
    }

    /// The drawn dimension whose label is under `p`.
    pub(super) fn hit_text(&self, p: Pos2) -> Option<Id> {
        self.drawn.iter().rev().find(|d| d.on_text(p)).map(|d| d.id)
    }

    /// The drawn dimension under `p`: labels first (they lie over the
    /// lines), then lines.
    pub(super) fn hit(&self, p: Pos2) -> Option<Id> {
        self.hit_text(p)
            .or_else(|| self.drawn.iter().rev().find(|d| d.on_line(p)).map(|d| d.id))
    }

    /// Drop the tool's picks, value and refusal.
    pub(super) fn clear(&mut self) {
        self.picks.clear();
        self.value = None;
        self.fresh = 0;
        self.error = None;
        self.draft_at = None;
        self.layout.choosing = None;
    }
}

/// The height a part on `side` sits at (mm): over its side's copper.
pub(super) fn seat(project: &Project, side: Layer) -> f64 {
    match side {
        Layer::Top => project
            .stackup
            .copper_span(CopperLayer::Top)
            .map_or(0.0, |s| s.1),
        Layer::Bottom => project
            .stackup
            .copper_span(CopperLayer::Bottom)
            .map_or(0.0, |s| s.0),
    }
}

/// The part a reference belongs to.
pub(super) fn part_of(r: &BoardRef) -> Option<Id> {
    match r {
        BoardRef::Part(f) => match f.target() {
            ConstraintTarget::Placement(id) => Some(id),
            _ => None,
        },
        _ => None,
    }
}

/// The part a dimension holds: its first part reference's.
pub(super) fn part_held(d: &PartDimension) -> Option<Id> {
    d.refs.iter().find_map(part_of)
}

/// The height a reference is drawn at: its part's seat, or the board top.
pub(super) fn height(project: &Project, r: &BoardRef) -> f64 {
    let side = part_of(r)
        .and_then(|id| project.placement_of(id))
        .map_or(Layer::Top, |p| p.layer);
    seat(project, side)
}

/// "J1 · pad A6", "J1 · front face", "Board · bottom edge": a reference in
/// the card's words; `local` is a new model pick's trace in the part frame.
pub(super) fn words(
    project: &Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
    r: &BoardRef,
    local: Option<&FeatureGeometry>,
) -> String {
    match r {
        BoardRef::Board(SketchRef::Curve(c)) => evaluator
            .and_then(|ev| dimensions::board_sketch(project, ev).ok())
            .and_then(|b| {
                dimensions::edge_name(&b, c).map(|n| {
                    let edge = dimensions::edge_words(n);
                    format!("Board · {}", edge.trim_start_matches("the "))
                })
            })
            .unwrap_or_else(|| "Board · edge".into()),
        BoardRef::Board(_) => "Board · point".into(),
        BoardRef::FollowedFace(_) => "the linked design".into(),
        BoardRef::Part(f) if f.is_model() => {
            dimensions::model_feature_words_with(project, f, local)
        }
        BoardRef::Part(FeatureRef::Pad { placement, pad, .. }) => {
            let part = project
                .instance(*placement)
                .map_or_else(|| "A part".into(), |i| i.designator.clone());
            format!("{part} · pad {}", pad.0)
        }
        BoardRef::Part(FeatureRef::Courtyard {
            placement,
            side: Some(side),
        }) => {
            let part = project
                .instance(*placement)
                .map_or_else(|| "A part".into(), |i| i.designator.clone());
            let side = match side {
                BoxSide::Left => "left",
                BoxSide::Right => "right",
                BoxSide::Bottom => "front",
                BoxSide::Top => "back",
            };
            format!("{part} · courtyard {side} side")
        }
        BoardRef::Part(f) => match f.target() {
            ConstraintTarget::Placement(id) => project
                .instance(id)
                .map_or_else(|| "a part".into(), |i| format!("{} · origin", i.designator)),
            _ => "a fill".into(),
        },
    }
}

/// A reference in words with the body its face or edge is on ("J1 · back
/// face (Insulator)"): what a stacked pick is told apart by, in the
/// chooser and in a drawn dimension's hover.
pub(super) fn words_with_body(
    project: &Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
    r: &BoardRef,
    local: Option<&FeatureGeometry>,
) -> String {
    let plain = words(project, evaluator, r, local);
    let body = match r {
        BoardRef::Part(FeatureRef::Face { face, .. }) => Some(&face.body.0),
        BoardRef::Part(FeatureRef::Edge { edge, .. } | FeatureRef::Vertex { edge, .. }) => {
            Some(&edge.body.0)
        }
        _ => None,
    };
    match body {
        Some(body) if !body.is_empty() => format!("{plain} ({body})"),
        _ => plain,
    }
}

pub(super) fn to_vec(p: [f64; 2]) -> Vec2 {
    Vec2::new(p[0], p[1])
}

/// A dimension's label: its measured length, "fx:" when an expression
/// drives it, in parentheses when it only measures (Solidcraft's).
pub(super) fn label(value: f64, d: Option<&PartDimension>) -> String {
    let expr = d.and_then(|d| match &d.value {
        Some(DimValue::Expr(e)) => Some(e.0.as_str()),
        Some(DimValue::Param(p)) => Some(p.0.as_str()),
        _ => None,
    });
    let reference = d.is_some_and(|d| d.reference);
    pcbcraft_ui::dim_view::length_label(value, expr, reference, "mm", DECIMALS)
}

/// The middle of the board outline's bounds (board mm).
pub(super) fn board_centre(project: &Project) -> [f64; 2] {
    let board = project.board();
    let points =
        std::iter::once(board.outer.start).chain(board.outer.segments.iter().map(|s| s.to()));
    let (lo, hi) = points.fold(
        ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]),
        |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        },
    );
    if lo[0].is_finite() && hi[0].is_finite() {
        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
    } else {
        [0.0, 0.0]
    }
}

/// The worse of two statuses.
pub(super) fn worse(a: ReferenceStatus, b: ReferenceStatus) -> ReferenceStatus {
    use ReferenceStatus::*;
    match (a, b) {
        (Lost, _) | (_, Lost) => Lost,
        (Inexact, _) | (_, Inexact) => Inexact,
        _ => Resolved,
    }
}

/// A length to the micrometre.
pub(super) fn round_um(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

pub(super) fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().chain(chars).collect::<String>()
    })
}

impl PcbEditor {
    /// Select a part dimension: its part is selected with it. The one
    /// selection Layout, 3D and the Properties card's Dimensions rows share.
    pub(in crate::pcb_editor) fn select_dimension(&mut self, project: &Project, id: Id) {
        let part = project
            .part_dimensions
            .iter()
            .find(|d| d.id == id)
            .and_then(part_held);
        if let Some(part) = part
            && self.instance != Some(part)
        {
            self.set_selection(Some(part), None, None);
        }
        self.dimension = Some(id);
    }

    /// The dimension the tool just added is selected once the engine has
    /// it (called from every sync, whichever view is drawn).
    pub(in crate::pcb_editor) fn select_added_dimension(&mut self, project: &Project) {
        if let Some(id) = self.dimension
            && !project.part_dimensions.iter().any(|d| d.id == id)
        {
            self.dimension = None;
        }
        if let Some(spec) = &self.dims.sent
            && let Some(d) = project
                .part_dimensions
                .iter()
                .rev()
                .find(|d| d.refs == spec.refs && d.value == spec.value)
        {
            let id = d.id;
            self.dims.sent = None;
            self.select_dimension(project, id);
        }
    }
}
