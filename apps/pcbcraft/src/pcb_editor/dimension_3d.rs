//! Part dimensions in PCB 3D (slice S5 of docs/board-sketch-references.md):
//! the Dimension tool, and the dimensions of the selected part drawn over
//! the model as Solidcraft draws sketch dimensions.
//!
//! **The tool.** Pick two references: a face, edge or hole of a part's 3D
//! model, a pad, a board edge, or another part's feature. What a click
//! picks is what the pointer highlights, as Solidcraft pre-highlights
//! ([`crate::viewport::surface`]): an edge of a part or of the board
//! within Solidcraft's 6 points of the pointer, in front, nearest first;
//! else the face under it (the board's top or side near its outline is
//! that board edge). A pick is named in the card in a designer's words
//! ("J1 · front face", "Board · bottom edge"); the persistent face name
//! stays in the stored reference. A model pick is named by the host
//! (`layout::dimensions::model::pick`) and traced once, so a sloped face or
//! one parallel to the board is refused in the tool card, in the trace's
//! words, before anything is committed. Two picks open the value at the
//! dimension's text, holding the distance measured now: a number, a board
//! parameter or an expression (`width / 2`). Enter commits one
//! `layout.dimension_add`, one undo step, and selects the new dimension;
//! Esc drops the picks, and Esc again (or Done) ends the tool.
//!
//! **Display.** The dimensions of the selected part (and the selected
//! dimension's) are laid out by Solidcraft's own sketch dimension layout
//! (`dim_layout_reaching`) in the plane the part sits on, with its label,
//! colours and arrowheads (`dim_view::label`, `dimension`, `accent`; the
//! arrowhead is Solidcraft's `dimension_arrow` artwork, crate-private
//! upstream, reproduced; MIT, revision 4f3ac07, see ATTRIBUTION.md). A
//! lost reference is drawn in the error colour and an inexact one in the
//! warning colour, both dashed and named so in the label, with their
//! finding on hover. While the Dimension tool is on, every part dimension
//! is drawn. Extension lines start on the two references (a face's or
//! edge's trace, a part's origin, the board edge), and the references of
//! the dimension under the pointer or selected are traced in its colour.
//! Labels stand level on screen and step clear of each other, as
//! Solidcraft stacks sketch dimension texts.
//!
//! **Selecting and editing.** A click on a label or a line selects the
//! dimension: its part is selected with it, in 2D and in the Properties
//! card too. Delete (`pcb.dimension-delete`) removes it, one undo step. A
//! double-click on it, or one click on its label with the Dimension tool,
//! opens its value in place at the label, as Solidcraft edits a sketch
//! dimension: the whole value selected, so typing replaces it; a number,
//! a board parameter or an expression. Enter commits one
//! `layout.dimension_edit`, one undo step; a refusal stays at the label in
//! the engine's words with the text kept, and Esc or a click elsewhere
//! closes it unchanged. The part then moves through the board model's
//! in-place update; the dimension is drawn from the document, so while
//! the model updates it already shows the new value at its new place.
//!
//! Hover, selection and the camera read the document and the cached meshes
//! only; nothing here rebuilds geometry or traces a model but a pick.
use super::{PcbEditor, Tool};
use crate::kit;
use crate::refusal::{self, Origin};
use crate::scene::SceneController;
use crate::viewport::Viewport;
use crate::viewport::surface::{Hovered, SurfaceHit};
use egui::{FontId, Pos2, Rect, Shape, Stroke, Ui, vec2};
use pcbcraft_core::layout::dimensions::view::{self, Shown};
use pcbcraft_core::layout::dimensions::{self, DimensionCommand, FeatureGeometry, features, model};
use pcbcraft_core::layout::outline::BoardShapeEvaluator;
use pcbcraft_core::library::footprint_source::{DimKind, DimValue, SketchCurveId, SketchRef};
use pcbcraft_core::model::board::ConstraintTarget;
use pcbcraft_core::model::dimension::{
    BoardRef, FeatureRef, PadPoint, PartDimension, PartDimensionSpec, PartRelation, ReferenceStatus,
};
use pcbcraft_core::{CopperLayer, Engine, Id, Layer, Operation, PadId, Project};
use pcbcraft_solidcraft::dims::{
    ConstraintKind, DimFrame, default_text, default_text_offset, dim_layout_reaching,
};
use pcbcraft_solidcraft::geom::Vec2;
use solidcraft_theme::Tokens;

/// How near the pointer an edge is taken instead of the face under it
/// (screen points): Solidcraft's edge pick reach (`viewport.rs`, `pick`).
pub(super) const EDGE_REACH_PT: f32 = 6.0;
/// How far from the highlighted edge's point the host may find it
/// (screen points): the display mesh and the model's design may chord a
/// curved edge differently.
const EDGE_SNAP_PT: f64 = 2.0;
/// How near a board edge a pick on the laminate's side must land (screen
/// points), and at least (mm).
const BOARD_REACH_PT: f64 = 8.0;
const BOARD_REACH_MM: f64 = 0.05;
/// Decimals of a drawn length (Solidcraft's default).
const DECIMALS: u8 = 2;
/// The value field over the canvas, points wide.
const FIELD_W: f32 = 120.0;
/// Points from a dimension line or label that still count as on it
/// (Solidcraft's `dim_view` hit reach).
const HIT_PT: f32 = 4.0;

fn field_id() -> egui::Id {
    egui::Id::unique("pcb-3d-dimension-value")
}

/// The value of a drawn dimension, edited in place.
fn edit_id() -> egui::Id {
    egui::Id::unique("pcb-3d-dimension-edit")
}

/// One reference picked for the dimension being made.
#[derive(Clone, Debug, PartialEq)]
struct Picked {
    reference: BoardRef,
    /// Its geometry in the board.
    geometry: FeatureGeometry,
    /// The height it is drawn at (mm): the part's seat, or the board top.
    z: f64,
    /// The card's words for it ("J1 · front face").
    words: String,
}

/// What a click would pick under the pointer.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// A face or edge of a part's 3D model at a board point; `reach` (mm)
    /// takes the edge.
    Model {
        instance: Id,
        at: [f64; 3],
        reach: f64,
    },
    Pad {
        instance: Id,
        pad: PadId,
    },
    Board {
        curve: SketchCurveId,
        geometry: FeatureGeometry,
    },
}

/// A dimension as drawn last frame, for hover, clicks and its value
/// edited in place.
#[derive(Clone, Debug)]
struct Drawn {
    id: Id,
    /// The label's text box (screen).
    text: Rect,
    /// Its extension and dimension lines (screen).
    segs: Vec<[Pos2; 2]>,
    /// Where its extension lines start, on its two references (board mm);
    /// what the tests check the drawing against.
    #[cfg_attr(not(test), allow(dead_code))]
    ends: [[f64; 3]; 2],
    /// What the label reads.
    #[cfg_attr(not(test), allow(dead_code))]
    label: String,
}

impl Drawn {
    fn on_text(&self, p: Pos2) -> bool {
        self.text.expand(2.0).contains(p)
    }
    fn on_line(&self, p: Pos2) -> bool {
        self.segs.iter().any(|s| seg_dist(p, s[0], s[1]) <= HIT_PT)
    }
}

/// How far `p` is from the segment `a`–`b` (points).
fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
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
struct InPlace {
    id: Id,
    text: String,
    /// Frames the field takes the keyboard back in.
    fresh: u8,
    /// Something was typed: until then the whole value is selected, so
    /// the first key replaces it.
    owned: bool,
    /// Why the value was refused, by the field or by the engine.
    error: Option<String>,
    /// The revision the edit was sent at, until the engine has answered.
    sent: Option<u64>,
}

/// The Dimension tool and the drawn dimensions' state.
#[derive(Default)]
pub(super) struct Dimension3d {
    picks: Vec<Picked>,
    /// The value being typed, once two picks measure.
    value: Option<String>,
    /// Frames the value field takes the keyboard back in.
    fresh: u8,
    /// Why the last pick or value was refused.
    error: Option<String>,
    /// The dimension sent, to select once the engine added it.
    sent: Option<PartDimensionSpec>,
    /// Dimensions drawn last frame, for clicks and hover.
    drawn: Vec<Drawn>,
    /// Where the draft's text was drawn last frame.
    draft_at: Option<Pos2>,
    /// A drawn dimension's value being edited in place.
    edit: Option<InPlace>,
}

impl Dimension3d {
    /// The drawn dimension whose label is under `p`.
    fn hit_text(&self, p: Pos2) -> Option<Id> {
        self.drawn.iter().rev().find(|d| d.on_text(p)).map(|d| d.id)
    }

    /// The drawn dimension under `p`: labels first (they lie over the
    /// lines), then lines.
    fn hit(&self, p: Pos2) -> Option<Id> {
        self.hit_text(p)
            .or_else(|| self.drawn.iter().rev().find(|d| d.on_line(p)).map(|d| d.id))
    }

    fn clear(&mut self) {
        self.picks.clear();
        self.value = None;
        self.fresh = 0;
        self.error = None;
        self.draft_at = None;
    }
}

/// The height a part on `side` sits at (mm): over its side's copper.
fn seat(project: &Project, side: Layer) -> f64 {
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

/// The height a reference is drawn at: its part's seat, or the board top.
fn height(project: &Project, r: &BoardRef) -> f64 {
    let side = match r {
        BoardRef::Part(f) => match f.target() {
            ConstraintTarget::Placement(id) => {
                project.placement_of(id).map_or(Layer::Top, |p| p.layer)
            }
            _ => Layer::Top,
        },
        _ => Layer::Top,
    };
    seat(project, side)
}

/// "J1 · pad A6", "J1 · front face", "Board · bottom edge": a reference in
/// the card's words; `local` is a new model pick's trace in the part frame.
fn words(
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
        BoardRef::Housing(_) => "the housing".into(),
        BoardRef::Part(f) if f.is_model() => {
            dimensions::model_feature_words_with(project, f, local)
        }
        BoardRef::Part(FeatureRef::Pad { placement, pad, .. }) => {
            let part = project
                .instance(*placement)
                .map_or_else(|| "A part".into(), |i| i.designator.clone());
            format!("{part} · pad {}", pad.0)
        }
        BoardRef::Part(f) => match f.target() {
            ConstraintTarget::Placement(id) => project
                .instance(id)
                .map_or_else(|| "a part".into(), |i| format!("{} · origin", i.designator)),
            _ => "a fill".into(),
        },
    }
}

fn to_vec(p: [f64; 2]) -> Vec2 {
    Vec2::new(p[0], p[1])
}

/// Solidcraft's dimension arrowhead (its `markers::dimension_arrow`).
fn arrow(painter: &egui::Painter, tip: Pos2, back: Pos2, colour: egui::Color32) {
    let d = (tip - back).normalized();
    let n = vec2(-d.y, d.x);
    painter.add(Shape::convex_polygon(
        vec![tip, tip - d * 9.0 + n * 3.0, tip - d * 9.0 - n * 3.0],
        colour,
        Stroke::NONE,
    ));
}

/// How one dimension is drawn: its label, colour and line style.
struct Look<'a> {
    label: &'a str,
    colour: egui::Color32,
    dashed: bool,
}

/// Draw one linear dimension in the plane `z` above the board, laid out
/// by Solidcraft's sketch dimension layout: extension lines from the
/// measured points, the dimension line with its arrows, and the label
/// level on screen. A label that would land on one already drawn this
/// frame (`placed`) steps to the next row across the dimension line, as
/// Solidcraft's `dim_view::show` stacks sketch dimension texts. Its text
/// box and its lines on screen.
fn draw(
    painter: &egui::Painter,
    viewport: &Viewport,
    shown: &Shown,
    z: f64,
    away: [f64; 2],
    look: &Look<'_>,
    placed: &mut Vec<Rect>,
) -> Option<(Rect, Vec<[Pos2; 2]>)> {
    let t = Tokens::get();
    let colour = look.colour;
    let to = |p: Vec2| viewport.to_screen([p.x, p.y, z]);
    let (p0, p1) = (to_vec(shown.from), to_vec(shown.to));
    let frame = DimFrame::Linear {
        p0,
        p1,
        dir: to_vec(shown.along),
    };
    let anchor = (p0 + p1) * 0.5;
    let (a, b) = (to(anchor)?, to(anchor + Vec2::new(1.0, 0.0))?);
    let (c, d) = (to(anchor), to(anchor + Vec2::new(0.0, 1.0)));
    // Millimetres per point at the dimension, the larger of the two axes'
    // (one may be foreshortened).
    let across = c.zip(d).map_or(0.0, |(c, d)| c.distance(d));
    let px = 1.0 / f64::from(a.distance(b).max(across)).max(1e-9);
    if !px.is_finite() {
        return None;
    }
    let font = FontId::monospace(t.font_body);
    let galley = painter.layout_no_wrap(look.label.to_owned(), font, colour);
    let size = galley.size() + vec2(6.0, 2.0);
    let half = Vec2::new(f64::from(size.x) * 0.5 * px, f64::from(size.y) * 0.5 * px);
    let mut text = default_text(&frame, to_vec(away), px);
    let mut lay = dim_layout_reaching(&frame, text, px, half, [None, None]);
    for _ in 0..6 {
        let Some(at) = to(lay.text) else { break };
        let here = Rect::from_center_size(at, size).expand(-0.5);
        if !placed.iter().any(|q| q.expand(-0.5).intersects(here)) {
            break;
        }
        let row = (f64::from(size.y) + 2.0) * px;
        let base = text.unwrap_or_else(|| default_text_offset(&frame, px));
        let side = if base.y < 0.0 { -1.0 } else { 1.0 };
        text = Some(base + Vec2::new(0.0, row * side));
        lay = dim_layout_reaching(&frame, text, px, half, [None, None]);
    }
    let stroke = Stroke::new(1.0, colour);
    // A dark rim under each line keeps it readable over the copper and
    // the parts' bodies.
    let rim = Stroke::new(3.0, t.viewport_top.gamma_multiply(0.6));
    let mut segs = Vec::new();
    for line in &lay.lines {
        let points: Vec<Pos2> = line.iter().filter_map(|q| to(*q)).collect();
        segs.extend(points.windows(2).map(|w| [w[0], w[1]]));
        painter.add(Shape::line(points.clone(), rim));
        if look.dashed {
            painter.extend(Shape::dashed_line(&points, stroke, 4.0, 3.0));
        } else {
            painter.add(Shape::line(points, stroke));
        }
    }
    for (tip, dir) in &lay.arrows {
        if let (Some(tip), Some(back)) = (to(*tip), to(*tip - *dir * px)) {
            arrow(painter, tip, back, colour);
        }
    }
    let centre = to(lay.text)?;
    let rect = Rect::from_center_size(centre, size);
    placed.push(rect);
    painter.rect_filled(
        rect,
        f32::from(t.radius_sm) / 2.0,
        t.viewport_top.gamma_multiply(0.85),
    );
    painter.galley(rect.min + vec2(3.0, 1.0), galley, colour);
    Some((rect, segs))
}

/// Trace a reference's geometry in the plane `z`: a line or arc along
/// itself, a circle round, a point as a dot.
fn trace_reference(
    painter: &egui::Painter,
    viewport: &Viewport,
    g: &FeatureGeometry,
    z: f64,
    colour: egui::Color32,
) {
    let to = |p: [f64; 2]| viewport.to_screen([p[0], p[1], z]);
    let stroke = Stroke::new(2.0, colour);
    let points: Vec<[f64; 2]> = match *g {
        FeatureGeometry::Point { at } => {
            if let Some(p) = to(at) {
                painter.circle_filled(p, 3.0, colour);
            }
            return;
        }
        FeatureGeometry::Line { a, b } => vec![a, b],
        FeatureGeometry::Arc { a, b, bulge } => arc_points(a, b, bulge),
        FeatureGeometry::Circle { center, radius } => (0..=48)
            .map(|i| {
                let t = std::f64::consts::TAU * f64::from(i) / 48.0;
                [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
            })
            .collect(),
    };
    let points: Vec<Pos2> = points.into_iter().filter_map(to).collect();
    painter.add(Shape::line(points, stroke));
}

/// Points along a bulge arc from `a` to `b` (`library::Segment::Arc`:
/// the bulge is the tangent of a quarter of the counter-clockwise sweep).
fn arc_points(a: [f64; 2], b: [f64; 2], bulge: f64) -> Vec<[f64; 2]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let chord = dx.hypot(dy);
    if !(chord > 1e-12 && bulge.is_finite() && bulge != 0.0) {
        return vec![a, b];
    }
    let sweep = 4.0 * bulge.atan();
    let radius = chord / (2.0 * (sweep / 2.0).sin());
    // The centre: from the chord's middle, square to it to the left of
    // a → b for a counter-clockwise sweep under half a turn.
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let off = radius * (sweep / 2.0).cos();
    let n = [-dy / chord, dx / chord];
    let c = [mid[0] + n[0] * off, mid[1] + n[1] * off];
    let start = (a[1] - c[1]).atan2(a[0] - c[0]);
    let r = radius.abs();
    (0..=24)
        .map(|i| {
            let t = start + sweep * f64::from(i) / 24.0;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        })
        .collect()
}

/// A dimension's label: its measured length, "fx:" when an expression
/// drives it, in parentheses when it only measures (Solidcraft's).
fn label(value: f64, d: Option<&PartDimension>) -> String {
    let expr = d.and_then(|d| match &d.value {
        Some(DimValue::Expr(e)) => Some(e.0.as_str()),
        Some(DimValue::Param(p)) => Some(p.0.as_str()),
        _ => None,
    });
    let reference = d.is_some_and(|d| d.reference);
    solidcraft_ui_egui::dim_view::label(
        &ConstraintKind::Distance { p: 0, q: 1, value },
        expr,
        reference,
        "mm",
        DECIMALS,
    )
}

/// Enter and Esc this frame, for a value field over the view: Enter
/// commits the value typed, Esc drops it.
fn field_keys(ui: &Ui) -> (bool, bool) {
    ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    })
}

/// The middle of the board outline's bounds (board mm).
fn board_centre(project: &Project) -> [f64; 2] {
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

/// The point a dimension's label is put away from: its middle mirrored
/// through the board's centre, so the label sits on the board's inner
/// side, readable over the board and clear of the edges the tool picks.
fn inward(centre: [f64; 2], shown: &Shown) -> [f64; 2] {
    let mid = [
        (shown.from[0] + shown.to[0]) / 2.0,
        (shown.from[1] + shown.to[1]) / 2.0,
    ];
    [2.0 * mid[0] - centre[0], 2.0 * mid[1] - centre[1]]
}

/// The worse of two statuses.
fn worse(a: ReferenceStatus, b: ReferenceStatus) -> ReferenceStatus {
    use ReferenceStatus::*;
    match (a, b) {
        (Lost, _) | (_, Lost) => Lost,
        (Inexact, _) | (_, Inexact) => Inexact,
        _ => Resolved,
    }
}

impl PcbEditor {
    /// Arm or end the Dimension tool (PCB 3D only).
    pub(super) fn arm_dimension(&mut self, armed: bool) {
        self.dim3d.clear();
        self.tool = if armed && self.view_3d {
            Tool::Dimension
        } else {
            Tool::Select
        };
    }

    /// Use `evaluator` as the board sketch engine (a counting one).
    #[cfg(test)]
    pub fn set_board_evaluator(&mut self, evaluator: &'static dyn BoardShapeEvaluator) {
        self.shape.evaluator = Some(evaluator);
    }

    /// The Dimension tool's picks, each with what its card calls it.
    #[cfg(test)]
    pub fn dimension_picks(&self) -> Vec<(BoardRef, String)> {
        self.dim3d
            .picks
            .iter()
            .map(|p| (p.reference.clone(), p.words.clone()))
            .collect()
    }

    /// How many references the Dimension tool has picked, the value
    /// being typed and why the last pick was refused.
    #[cfg(test)]
    pub fn dimension_draft(&self) -> (usize, Option<String>, Option<String>) {
        (
            self.dim3d.picks.len(),
            self.dim3d.value.clone(),
            self.dim3d.error.clone(),
        )
    }

    /// The dimension selected in 2D, 3D and the Properties card.
    #[cfg(test)]
    pub fn selected_dimension(&self) -> Option<Id> {
        self.dimension
    }

    /// The dimensions drawn in 3D last frame: id, label, the label's text
    /// box on screen, and where the extension lines start on the two
    /// references (board mm).
    #[cfg(test)]
    pub fn dimensions_drawn(&self) -> Vec<(Id, String, Rect, [[f64; 3]; 2])> {
        self.dim3d
            .drawn
            .iter()
            .map(|d| (d.id, d.label.clone(), d.text, d.ends))
            .collect()
    }

    /// The value open at a 3D dimension's label: the dimension, the text
    /// and why it was refused.
    #[cfg(test)]
    pub fn dimension_in_place(&self) -> Option<(Id, String, Option<String>)> {
        self.dim3d
            .edit
            .as_ref()
            .map(|e| (e.id, e.text.clone(), e.error.clone()))
    }

    /// Select a part dimension: its part is selected with it.
    pub(super) fn select_dimension(&mut self, project: &Project, id: Id) {
        let part = project
            .part_dimensions
            .iter()
            .find(|d| d.id == id)
            .and_then(|d| {
                d.refs.iter().find_map(|r| match r {
                    BoardRef::Part(f) => match f.target() {
                        ConstraintTarget::Placement(p) => Some(p),
                        _ => None,
                    },
                    _ => None,
                })
            });
        if let Some(part) = part
            && self.instance != Some(part)
        {
            self.set_selection(Some(part), None, None);
        }
        self.dimension = Some(id);
    }

    /// The dimension the tool just added is selected once the engine has
    /// it (called from every sync, whichever view is drawn).
    pub(super) fn select_added_dimension(&mut self, project: &Project) {
        if let Some(id) = self.dimension
            && !project.part_dimensions.iter().any(|d| d.id == id)
        {
            self.dimension = None;
        }
        if let Some(spec) = &self.dim3d.sent
            && let Some(d) = project
                .part_dimensions
                .iter()
                .rev()
                .find(|d| d.refs == spec.refs && d.value == spec.value)
        {
            let id = d.id;
            self.dim3d.sent = None;
            self.select_dimension(project, id);
        }
    }

    /// What a click would pick under `pos`, and its highlight: hover and
    /// click both ask here, so the highlight is the pick. An edge of a
    /// part body or of the board within [`EDGE_REACH_PT`], in front,
    /// nearest first; else the face under the pointer.
    fn target_at(
        &self,
        project: &Project,
        scene: &SceneController,
        pos: Pos2,
    ) -> Option<(Target, Hovered)> {
        let viewport = &scene.viewport;
        // The part a body is the 3D model of; pads, copper and fills are
        // not model bodies.
        let part_of = |item: u64| {
            let source = scene.source(item)?;
            let plain = source.pad.is_none()
                && source.track.is_none()
                && source.via.is_none()
                && source.fill.is_none();
            source.instance.filter(|_| plain)
        };
        let board_reach = |at: [f64; 3]| {
            let mm = viewport.mm_per_point(at).unwrap_or(0.0);
            (BOARD_REACH_PT * mm).max(BOARD_REACH_MM)
        };
        let board_at = |at: [f64; 3]| {
            view::board_curve_at(
                project,
                self.shape.evaluator,
                [at[0], at[1]],
                board_reach(at),
            )
        };
        // What the pointer is over. On a part's body, a board edge flush
        // with it (J1's front on the bottom edge) is not taken: the part's
        // face or edges are. A board edge is taken off the parts: over the
        // board's own bodies, or just off the board.
        let under = viewport.surface_hit(pos);
        let over_part = under
            .and_then(|hit| hit.item)
            .is_some_and(|item| part_of(item).is_some());
        let edges = viewport.edges_near(pos, EDGE_REACH_PT, |g| match g.item {
            Some(item) => part_of(item).is_some(),
            None => !over_part,
        });
        for e in edges {
            let hovered = Hovered::Edge {
                geometry: e.geometry,
                edge: e.edge,
            };
            match e.item {
                Some(item) => {
                    let Some(instance) = part_of(item) else {
                        continue;
                    };
                    let mm = viewport.mm_per_point(e.point).unwrap_or(0.0);
                    let target = Target::Model {
                        instance,
                        at: e.point,
                        reach: EDGE_SNAP_PT * mm + 1e-3,
                    };
                    return Some((target, hovered));
                }
                // A board-level body's edge (laminate, mask or film) is a
                // board edge where it runs along the outline; an opening's
                // edge is not.
                None => {
                    if let Some((curve, geometry)) = board_at(e.point) {
                        return Some((Target::Board { curve, geometry }, hovered));
                    }
                }
            }
        }
        let hit: SurfaceHit = under?;
        let face = Hovered::Face {
            geometry: hit.geometry,
            face: hit.face,
        };
        if let Some(item) = hit.item {
            let source = scene.source(item)?;
            let instance = source.instance?;
            if let Some(pad) = source.pad {
                return Some((Target::Pad { instance, pad }, face));
            }
            part_of(item)?;
            let target = Target::Model {
                instance,
                at: hit.point,
                reach: 0.0,
            };
            return Some((target, face));
        }
        // The laminate's side, or a board-level top or bottom near the
        // outline: the board curve there, shown by the face's edge along it
        // when one is within reach.
        let (curve, geometry) = board_at(hit.point)?;
        let shown = viewport
            .edge_near(&hit, pos, BOARD_REACH_PT as f32)
            .map_or(face, |edge| Hovered::Edge {
                geometry: hit.geometry,
                edge,
            });
        Some((Target::Board { curve, geometry }, shown))
    }

    /// The reference a click on `target` picks, with its geometry in the
    /// board; `Err` says why it cannot be dimensioned.
    fn pick_target(&self, project: &Project, target: &Target) -> Result<Picked, String> {
        let reference = match target {
            Target::Model {
                instance,
                at,
                reach,
            } => {
                let evaluator = self
                    .shape
                    .evaluator
                    .ok_or("Dimensions need the sketch evaluator")?;
                let (feature, local) = model::pick(project, evaluator, *instance, *at, *reach)
                    .map_err(|e| refusal::shown_error(&e))?;
                let placement = project
                    .placement_of(*instance)
                    .ok_or("The part is not on the board")?;
                let geometry = view::transformed(local, &view::part_transform(placement));
                let reference = BoardRef::Part(feature);
                return Ok(Picked {
                    z: seat(project, placement.layer),
                    words: words(project, Some(evaluator), &reference, Some(&local)),
                    reference,
                    geometry,
                });
            }
            Target::Pad { instance, pad } => BoardRef::Part(FeatureRef::Pad {
                placement: *instance,
                pad: *pad,
                at: PadPoint::Centre,
            }),
            Target::Board { curve, geometry } => {
                let reference = BoardRef::Board(SketchRef::Curve(curve.clone()));
                return Ok(Picked {
                    words: words(project, self.shape.evaluator, &reference, None),
                    reference,
                    geometry: *geometry,
                    z: seat(project, Layer::Top),
                });
            }
        };
        let BoardRef::Part(feature) = &reference else {
            return Err("Pick a part's face, edge, hole or pad".into());
        };
        let local = features::geometry(project, feature).map_err(|e| refusal::shown_error(&e))?;
        let ConstraintTarget::Placement(id) = feature.target() else {
            return Err("Pick a part's face, edge, hole or pad".into());
        };
        let placement = project
            .placement_of(id)
            .ok_or("The part is not on the board")?;
        Ok(Picked {
            geometry: view::transformed(local, &view::part_transform(placement)),
            z: seat(project, placement.layer),
            words: words(project, self.shape.evaluator, &reference, None),
            reference,
        })
    }

    /// The draft's measurement once two picks are made.
    fn draft_shown(&self) -> Option<Shown> {
        match self.dim3d.picks.as_slice() {
            [a, b] => view::shown(&a.geometry, &b.geometry),
            _ => None,
        }
    }

    /// A click with the tool: pick what is under `pos`; the second pick
    /// opens the value.
    fn dimension_click(&mut self, project: &Project, scene: &SceneController, pos: Pos2) {
        if self.dim3d.picks.len() >= 2 {
            // A click away from the value drops the picks and starts again.
            self.dim3d.clear();
        }
        let Some((target, _)) = self.target_at(project, scene, pos) else {
            // Say so: a click that picks nothing is not lost silently.
            self.dim3d.error = Some(
                "Nothing to dimension there: pick a part's face, edge or pad, or a board edge"
                    .into(),
            );
            return;
        };
        let picked = match self.pick_target(project, &target) {
            Ok(p) => p,
            Err(why) => {
                // The host's words follow a code; the card starts a sentence.
                self.dim3d.error = Some(capital(&why));
                return;
            }
        };
        if self
            .dim3d
            .picks
            .first()
            .is_some_and(|p| p.reference == picked.reference)
        {
            self.dim3d.error = Some("Pick another face, edge, hole or board edge".into());
            return;
        }
        self.dim3d.error = None;
        self.dim3d.picks.push(picked);
        if self.dim3d.picks.len() < 2 {
            return;
        }
        let any_part = self
            .dim3d
            .picks
            .iter()
            .any(|p| matches!(p.reference, BoardRef::Part(_)));
        if !any_part {
            self.dim3d.picks.pop();
            self.dim3d.error =
                Some("A dimension holds a part: pick a part's face, edge, hole or pad".into());
            return;
        }
        match self.draft_shown() {
            Some(shown) => {
                self.dim3d.value = Some(format!("{} mm", kit::number_text(round_um(shown.value))));
                self.dim3d.fresh = 2;
            }
            None => {
                self.dim3d.picks.pop();
                self.dim3d.error = Some("These two cannot be measured; pick another".into());
            }
        }
    }

    /// The dimension the typed value makes, or why it makes none.
    fn draft_spec(&self, project: &Project) -> Result<PartDimensionSpec, String> {
        let text = self.dim3d.value.as_deref().unwrap_or_default();
        let value = super::edge_card::length_value(project, text)?;
        Ok(PartDimensionSpec {
            kind: PartRelation::Dim(DimKind::Distance),
            refs: self
                .dim3d
                .picks
                .iter()
                .map(|p| p.reference.clone())
                .collect(),
            value: Some(value),
            reference: false,
        })
    }

    /// PCB 3D's part dimensions, after the view is drawn: the tool's hover,
    /// picks and value, the dimensions drawn, clicks on them and the value
    /// edited in place. `pick` is the view's click this frame. The
    /// operation commits a new dimension, an edited value or a deletion.
    pub fn board_3d(
        &mut self,
        ui: &mut Ui,
        scene: &mut SceneController,
        engine: &Engine,
        pick: Option<Option<u64>>,
    ) -> Option<Operation> {
        self.sync(engine);
        let project = engine.project();
        let rect = scene.viewport.rect()?;
        if let Some(text) = refusal::claim(ui.ctx(), Origin::Field(field_id())) {
            self.dim3d.error = Some(text);
            self.dim3d.sent = None;
            self.dim3d.fresh = 2;
        }
        self.answered(ui.ctx(), engine);
        let armed = self.tool == Tool::Dimension;
        let pointer = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p) && ui.rect_contains_pointer(rect));
        let over_label = pointer.is_some_and(|p| {
            self.dim3d.hit_text(p).is_some()
                || self
                    .dim3d
                    .draft_at
                    .is_some_and(|c| Rect::from_center_size(c, vec2(FIELD_W, 24.0)).contains(p))
        });
        // The tool's hover: highlighted while nothing but the pointer moves.
        let moving = ui.input(|i| i.pointer.any_down());
        let hovered = if armed && !moving && !over_label {
            pointer.and_then(|p| self.target_at(project, scene, p).map(|(_, h)| h))
        } else {
            None
        };
        scene.viewport.set_hover(hovered);
        if armed && crate::keys::pressed(ui.ctx(), "pcb.dimension-cancel") {
            if self.dim3d.picks.is_empty() {
                self.arm_dimension(false);
            } else {
                self.dim3d.clear();
            }
        }
        // Delete removes the selected dimension (never while its value or
        // a new one's is typed: the field keeps the key).
        let mut operation = None;
        if let Some(id) = self.dimension
            && crate::keys::pressed(ui.ctx(), "pcb.dimension-delete")
        {
            self.dim3d.edit = None;
            operation = Some(Operation::Dimension(DimensionCommand::Delete { id }));
        }
        // A click on a dimension selects it, and a double-click opens its
        // value; with the tool, a click on a label opens it (its lines are
        // left to the picks). Otherwise the tool picks, or the click
        // selects what is under it, as a Layout click.
        if let Some(pick) = pick {
            let at = scene.viewport.clicked_at();
            // A quick third click (after the part's) counts as well.
            let double = ui.input(|i| {
                let primary = egui::PointerButton::Primary;
                i.pointer.button_double_clicked(primary) || i.pointer.button_triple_clicked(primary)
            });
            let on = at.and_then(|p| {
                if armed {
                    self.dim3d.hit_text(p)
                } else {
                    self.dim3d.hit(p)
                }
            });
            if let Some(id) = on {
                self.select_dimension(project, id);
                if armed || double {
                    self.open_in_place(project, id);
                }
            } else if armed {
                if let Some(at) = at {
                    self.dimension_click(project, scene, at);
                }
            } else {
                self.select_picked(pick.and_then(|id| scene.source(id)));
            }
        }
        let painter = ui.painter_at(rect);
        self.paint_dimensions_3d(ui, &painter, &scene.viewport, project, true);
        let edited = self.in_place_field(ui, engine);
        let added = self.value_field(ui, project);
        operation.or(edited).or(added)
    }

    /// PCB 3D while a newer board model is made: the last model stays up,
    /// and the dimensions are drawn from the document, so an edited value
    /// shows at once, at the part's new place. Nothing is picked.
    pub fn board_3d_updating(&mut self, ui: &mut Ui, scene: &SceneController, engine: &Engine) {
        self.sync(engine);
        self.answered(ui.ctx(), engine);
        if let Some(rect) = scene.viewport.rect() {
            let painter = ui.painter_at(rect);
            self.paint_dimensions_3d(ui, &painter, &scene.viewport, engine.project(), false);
        }
    }

    /// Open a drawn dimension's value at its label: its expression or
    /// value, all selected. A dimension that only measures, or holds
    /// without a value, has none to edit.
    fn open_in_place(&mut self, project: &Project, id: Id) {
        let Some(d) = project.part_dimensions.iter().find(|d| d.id == id) else {
            return;
        };
        if d.reference || d.value.is_none() {
            return;
        }
        // The tool's picks give way to the value opened.
        self.dim3d.clear();
        self.dim3d.edit = Some(InPlace {
            id,
            text: super::edge_card::value_text(d),
            fresh: 3,
            owned: false,
            error: None,
            sent: None,
        });
    }

    /// The engine's answer to the value sent last frame: a refusal stays
    /// at the label with the text kept and selected; an applied edit (or a
    /// dimension gone) closes the field.
    fn answered(&mut self, ctx: &egui::Context, engine: &Engine) {
        let Some(edit) = self.dim3d.edit.as_mut() else {
            return;
        };
        if !engine
            .project()
            .part_dimensions
            .iter()
            .any(|d| d.id == edit.id)
        {
            self.dim3d.edit = None;
            return;
        }
        let Some(revision) = edit.sent else {
            return;
        };
        if let Some(text) = refusal::claim(ctx, Origin::Field(edit_id())) {
            edit.error = Some(text);
            edit.sent = None;
            edit.fresh = 2;
            edit.owned = false;
        } else if engine.revision() != revision {
            self.dim3d.edit = None;
        }
    }

    /// The value of a drawn dimension, open at its label: Enter commits one
    /// `layout.dimension_edit`, Esc or a click elsewhere closes it.
    fn in_place_field(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let project = engine.project();
        let edit = self.dim3d.edit.as_mut()?;
        if edit.sent.is_some() {
            // Waiting for the engine: the label shows what is stored.
            return None;
        }
        let Some(label) = self
            .dim3d
            .drawn
            .iter()
            .find(|d| d.id == edit.id)
            .map(|d| d.text)
        else {
            // Not drawn (off the view, or no longer shown): nothing to type
            // into.
            self.dim3d.edit = None;
            return None;
        };
        let t = Tokens::get();
        let r = Rect::from_center_size(
            label.center(),
            vec2(FIELD_W.max(label.width() + t.space_8), t.control_height),
        );
        let mut text = edit.text.clone();
        let invalid = edit.error.is_some();
        let response = ui
            .scope_builder(
                egui::UiBuilder::new().max_rect(r).id_salt(edit_id()),
                |ui| {
                    kit::text_field(
                        ui,
                        "pcb-3d-dimension-edit",
                        &mut text,
                        "distance, like 5 mm or width / 2",
                        invalid,
                    )
                },
            )
            .inner;
        if text != edit.text {
            edit.error = None;
        }
        edit.text = text;
        // Read before any focus request below, which would take the focus
        // straight back and hide Enter's surrender.
        let lost = response.lost_focus();
        if edit.fresh > 0 {
            edit.fresh -= 1;
            if !lost {
                response.request_focus();
            }
        }
        let typed = ui.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Key { pressed: true, .. }
                )
            })
        });
        edit.owned |= response.changed() || (response.has_focus() && typed);
        if !edit.owned {
            // The whole value stays selected until something is typed, so
            // the first key replaces it (Solidcraft's inline value).
            let n = edit.text.chars().count();
            let mut state = egui::TextEdit::load_state(ui.ctx(), response.id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(n),
                )));
            state.store(ui.ctx(), response.id);
        }
        if let Some(why) = &edit.error {
            // The refusal at the label, whole, wrapped under the field.
            let painter = ui.painter();
            let galley = painter.layout(
                why.clone(),
                FontId::proportional(t.font_small),
                t.error,
                FIELD_W * 2.5,
            );
            let at = Pos2::new(r.left(), r.bottom() + t.space_2);
            let back = Rect::from_min_size(at, galley.size() + vec2(t.space_8, t.space_4));
            painter.rect_filled(back, t.radius_sm, t.panel);
            painter.rect_stroke(
                back,
                t.radius_sm,
                Stroke::new(1.0, t.error),
                egui::StrokeKind::Inside,
            );
            painter.galley(at + vec2(t.space_4, t.space_2), galley, t.error);
        }
        let (enter, escape) = field_keys(ui);
        if escape || (lost && !enter && edit.fresh == 0) {
            self.dim3d.edit = None;
            return None;
        }
        if !(lost && enter) {
            return None;
        }
        let d = project.part_dimensions.iter().find(|d| d.id == edit.id)?;
        match super::edge_card::typed_value(project, d, &edit.text) {
            Ok(value) if Some(&value) == d.value.as_ref() => {
                self.dim3d.edit = None;
                None
            }
            Ok(value) => {
                let mut next = d.clone();
                next.value = Some(value);
                edit.sent = Some(engine.revision());
                edit.error = None;
                refusal::mark(ui.ctx(), Origin::Field(edit_id()));
                Some(Operation::Dimension(DimensionCommand::Edit {
                    dimension: next,
                }))
            }
            Err(why) => {
                edit.error = Some(why);
                edit.fresh = 2;
                edit.owned = false;
                None
            }
        }
    }

    /// The dimensions drawn: the selected part's and the selected one,
    /// every one while the Dimension tool is on, and the one being edited;
    /// then the tool's draft. `live` is false while a newer board model is
    /// made: no hover then.
    fn paint_dimensions_3d(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        viewport: &Viewport,
        project: &Project,
        live: bool,
    ) {
        let t = Tokens::get();
        let evaluator = self.shape.evaluator;
        let hover = ui.input(|i| i.pointer.hover_pos()).filter(|p| {
            live && painter.clip_rect().contains(*p)
                && ui.rect_contains_pointer(painter.clip_rect())
        });
        let armed = self.tool == Tool::Dimension;
        let hovered = hover.and_then(|p| {
            if armed {
                self.dim3d.hit_text(p)
            } else {
                self.dim3d.hit(p)
            }
        });
        let editing = self.dim3d.edit.as_ref().map(|e| e.id);
        let mut drawn = Vec::new();
        let mut placed = Vec::new();
        let centre = board_centre(project);
        let findings = if project.part_dimensions.is_empty() {
            Vec::new()
        } else {
            dimensions::checks(project)
        };
        for d in project.part_dimensions.iter() {
            if !matches!(d.kind, PartRelation::Dim(DimKind::Distance)) {
                continue;
            }
            let part = d.refs.iter().find_map(|r| match r {
                BoardRef::Part(f) => match f.target() {
                    ConstraintTarget::Placement(id) => Some(id),
                    _ => None,
                },
                _ => None,
            });
            let shows = armed
                || self.dimension == Some(d.id)
                || editing == Some(d.id)
                || part.is_some_and(|p| self.instance == Some(p) || self.also.contains(&p));
            if !shows {
                continue;
            }
            let (Some(a), Some(b)) = (d.refs.first(), d.refs.get(1)) else {
                continue;
            };
            let (Some((ga, sa)), Some((gb, sb))) = (
                view::reference_geometry(project, evaluator, a),
                view::reference_geometry(project, evaluator, b),
            ) else {
                continue;
            };
            let Some(shown) = view::shown(&ga, &gb) else {
                continue;
            };
            let status = worse(sa, sb);
            let z = height(project, a).max(height(project, b));
            let away = inward(centre, &shown);
            let mut text = label(shown.value, Some(d));
            let (colour, dashed) = match status {
                ReferenceStatus::Lost => {
                    text.push_str(" · lost");
                    (t.error, true)
                }
                ReferenceStatus::Inexact => {
                    text.push_str(" · inexact");
                    (t.warning, true)
                }
                ReferenceStatus::Resolved => (t.dimension, false),
            };
            let hot =
                self.dimension == Some(d.id) || hovered == Some(d.id) || editing == Some(d.id);
            let colour = if hot && status == ReferenceStatus::Resolved {
                t.accent
            } else {
                colour
            };
            if hot {
                // What it measures between, traced in its colour.
                for g in [&ga, &gb] {
                    trace_reference(painter, viewport, g, z, colour);
                }
            }
            let look = Look {
                label: &text,
                colour,
                dashed,
            };
            let Some((rect, segs)) = draw(painter, viewport, &shown, z, away, &look, &mut placed)
            else {
                continue;
            };
            // A lost or inexact reference says why on hover.
            if live
                && status != ReferenceStatus::Resolved
                && let Some(part) = part
            {
                let why: Vec<&str> = findings
                    .iter()
                    .filter(|f| {
                        f.code == dimensions::REFERENCE_LOST
                            || f.code == dimensions::REFERENCE_INEXACT
                    })
                    .filter(|f| {
                        f.objects.iter().any(|o| {
                            matches!(o, pcbcraft_core::ObjectRef::Instance { id } if *id == part)
                        })
                    })
                    .map(|f| f.message.as_str())
                    .collect();
                if !why.is_empty() {
                    ui.interact(
                        rect,
                        egui::Id::unique(("pcb-3d-dimension-why", d.id)),
                        egui::Sense::hover(),
                    )
                    .on_hover_text(why.join("\n"));
                }
            }
            drawn.push(Drawn {
                id: d.id,
                text: rect,
                segs,
                ends: [
                    [shown.from[0], shown.from[1], z],
                    [shown.to[0], shown.to[1], z],
                ],
                label: text,
            });
        }
        self.dim3d.drawn = drawn;
        // The draft: the picks so far, and their dimension once measured.
        self.dim3d.draft_at = None;
        if armed && live {
            for p in &self.dim3d.picks {
                if let FeatureGeometry::Point { at } | FeatureGeometry::Circle { center: at, .. } =
                    p.geometry
                    && let Some(s) = viewport.to_screen([at[0], at[1], p.z])
                {
                    painter.circle_stroke(s, 5.0, Stroke::new(1.5, t.accent));
                }
            }
            if let (Some(shown), [a, b]) = (self.draft_shown(), self.dim3d.picks.as_slice()) {
                let typed = self
                    .dim3d
                    .value
                    .as_deref()
                    .and_then(|v| super::edge_card::length_value(project, v).ok())
                    .and_then(|v| super::edge_card::length(project, &v));
                let text = label(typed.unwrap_or(shown.value), None);
                let z = a.z.max(b.z);
                let away = match (&a.reference, &b.reference) {
                    (BoardRef::Part(f), _) | (_, BoardRef::Part(f)) => match f.target() {
                        ConstraintTarget::Placement(id) => {
                            project.placement_of(id).map(|p| p.position)
                        }
                        _ => None,
                    },
                    _ => None,
                }
                .unwrap_or(shown.from);
                let look = Look {
                    label: &text,
                    colour: t.accent,
                    dashed: false,
                };
                self.dim3d.draft_at = draw(painter, viewport, &shown, z, away, &look, &mut placed)
                    .map(|(r, _)| r.center());
            }
        }
    }

    /// The value field at the draft's text: Enter commits the dimension
    /// (one `layout.dimension_add`), Esc drops the picks.
    fn value_field(&mut self, ui: &mut Ui, project: &Project) -> Option<Operation> {
        let mut text = self.dim3d.value.clone()?;
        let centre = self.dim3d.draft_at?;
        let t = Tokens::get();
        let r = Rect::from_center_size(
            centre + vec2(0.0, t.control_height),
            vec2(FIELD_W, t.control_height),
        );
        let invalid = self.dim3d.error.is_some();
        let response = ui
            .scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
                kit::text_field(
                    ui,
                    "pcb-3d-dimension-value",
                    &mut text,
                    "distance, like 5 mm or width / 2",
                    invalid,
                )
            })
            .inner;
        if text != self.dim3d.value.clone().unwrap_or_default() {
            self.dim3d.error = None;
        }
        self.dim3d.value = Some(text);
        let lost = response.lost_focus();
        if self.dim3d.fresh > 0 {
            self.dim3d.fresh -= 1;
            if !lost {
                response.request_focus();
            }
        }
        let (enter, escape) = field_keys(ui);
        if escape {
            self.dim3d.clear();
            return None;
        }
        if !(lost && enter) {
            return None;
        }
        match self.draft_spec(project) {
            Ok(spec) => {
                self.dim3d.sent = Some(spec.clone());
                self.dim3d.clear();
                refusal::mark(ui.ctx(), Origin::Field(field_id()));
                Some(Operation::Dimension(DimensionCommand::Add {
                    dimension: spec,
                }))
            }
            Err(why) => {
                self.dim3d.error = Some(why);
                self.dim3d.fresh = 2;
                None
            }
        }
    }

    /// The Dimension tool's card: what is picked, the next step, and why
    /// the last pick or value was refused.
    pub(super) fn dimension_card(&mut self, ui: &mut Ui) -> Option<Operation> {
        let hint = match self.dim3d.picks.len() {
            0 => "Pick a part's face, edge, hole or pad, or a board edge",
            1 => "Pick what to dimension it to",
            _ => "Type the distance and press Enter; Esc starts again",
        };
        let card = kit::ToolCard {
            icon: "dimension",
            name: "Dimension",
            key: "",
            hint,
            tip: "Holds a part at a distance from a board edge or another part's feature. \
                  An edge is taken within a few points of it. Sloped faces and \
                  faces parallel to the board have no single trace on the board: pick one \
                  of their edges instead.",
            finish: None,
        };
        let picks: Vec<String> = self.dim3d.picks.iter().map(|p| p.words.clone()).collect();
        let error = self.dim3d.error.clone();
        let shown = self.draft_shown();
        let t = Tokens::get();
        let margin = egui::Margin::symmetric(t.space_8 as i8, 0);
        let response = egui::Frame::NONE
            .inner_margin(margin)
            .show(ui, |ui| {
                kit::tool_card(ui, &card, |ui| {
                    for (i, p) in picks.iter().enumerate() {
                        kit::value_row(ui, if i == 0 { "From" } else { "To" }, &capital(p));
                    }
                    if let Some(shown) = shown {
                        kit::value_row(
                            ui,
                            "Now",
                            &format!("{} mm", kit::number_text(round_um(shown.value))),
                        );
                    }
                    if let Some(why) = &error {
                        kit::refusal_wrapped(ui, why);
                    }
                })
            })
            .inner;
        if response.done {
            self.arm_dimension(false);
        }
        None
    }
}

/// A length to the micrometre.
fn round_um(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().chain(chars).collect::<String>()
    })
}
