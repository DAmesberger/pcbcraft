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
//! finding on hover. A click on a label selects the dimension: its part is
//! selected with it, in 2D and in the Properties card too.
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
use pcbcraft_solidcraft::dims::{ConstraintKind, DimFrame, default_text, dim_layout_reaching};
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

fn field_id() -> egui::Id {
    egui::Id::unique("pcb-3d-dimension-value")
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
    /// Dimension labels drawn last frame, for clicks and hover.
    drawn: Vec<(Id, Rect)>,
    /// Where the draft's text was drawn last frame.
    draft_at: Option<Pos2>,
}

impl Dimension3d {
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

/// Draw one linear dimension in the plane `z` above the board, laid out
/// by Solidcraft's sketch dimension layout; its text box on screen.
#[allow(clippy::too_many_arguments)]
fn draw(
    painter: &egui::Painter,
    viewport: &Viewport,
    shown: &Shown,
    z: f64,
    away: [f64; 2],
    label: &str,
    colour: egui::Color32,
    dashed: bool,
) -> Option<Rect> {
    let t = Tokens::get();
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
    let galley = painter.layout_no_wrap(label.to_owned(), font, colour);
    let size = galley.size() + vec2(6.0, 2.0);
    let half = Vec2::new(f64::from(size.x) * 0.5 * px, f64::from(size.y) * 0.5 * px);
    let text = default_text(&frame, to_vec(away), px);
    let lay = dim_layout_reaching(&frame, text, px, half, [None, None]);
    let stroke = Stroke::new(1.0, colour);
    for line in &lay.lines {
        let points: Vec<Pos2> = line.iter().filter_map(|q| to(*q)).collect();
        if dashed {
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
    painter.rect_filled(
        rect,
        f32::from(t.radius_sm) / 2.0,
        t.viewport_top.gamma_multiply(0.85),
    );
    painter.galley(rect.min + vec2(3.0, 1.0), galley, colour);
    Some(rect)
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
    /// picks and value, the dimensions of the selection, and clicks on
    /// them. `pick` is the view's click this frame. The operation commits a
    /// new dimension.
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
        let armed = self.tool == Tool::Dimension;
        let pointer = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p) && ui.rect_contains_pointer(rect));
        let over_label = pointer.is_some_and(|p| {
            self.dim3d.drawn.iter().any(|(_, r)| r.contains(p))
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
        // A click: on a label selects its dimension; with the tool, picks;
        // else selects what is under it, as a Layout click.
        if let Some(pick) = pick {
            let at = scene.viewport.clicked_at();
            let on_label = at.and_then(|p| {
                self.dim3d
                    .drawn
                    .iter()
                    .find(|(_, r)| r.contains(p))
                    .map(|(id, _)| *id)
            });
            if let Some(id) = on_label {
                self.select_dimension(project, id);
            } else if armed {
                if let Some(at) = at {
                    self.dimension_click(project, scene, at);
                }
            } else {
                self.select_picked(pick.and_then(|id| scene.source(id)));
            }
        }
        let painter = ui.painter_at(rect);
        self.paint_dimensions_3d(ui, &painter, &scene.viewport, project);
        self.value_field(ui, project)
    }

    /// The dimensions of the selected part (and of the selected
    /// dimension), and the tool's draft.
    fn paint_dimensions_3d(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        viewport: &Viewport,
        project: &Project,
    ) {
        let t = Tokens::get();
        let evaluator = self.shape.evaluator;
        let hover = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| painter.clip_rect().contains(*p));
        let mut drawn = Vec::new();
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
            let shows = self.dimension == Some(d.id)
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
            let away = part
                .and_then(|p| project.placement_of(p))
                .map_or(shown.from, |p| p.position);
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
            let last = self.dim3d.drawn.iter().find(|(id, _)| *id == d.id);
            let hot = self.dimension == Some(d.id)
                || last.is_some_and(|(_, r)| hover.is_some_and(|p| r.contains(p)));
            let colour = if hot && status == ReferenceStatus::Resolved {
                t.accent
            } else {
                colour
            };
            if let Some(rect) = draw(painter, viewport, &shown, z, away, &text, colour, dashed) {
                drawn.push((d.id, rect));
                // A lost or inexact reference says why on hover.
                if status != ReferenceStatus::Resolved
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
            }
        }
        self.dim3d.drawn = drawn;
        // The draft: the picks so far, and their dimension once measured.
        self.dim3d.draft_at = None;
        if self.tool == Tool::Dimension {
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
                self.dim3d.draft_at =
                    draw(painter, viewport, &shown, z, away, &text, t.accent, false)
                        .map(|r| r.center());
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
        let (enter, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
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
