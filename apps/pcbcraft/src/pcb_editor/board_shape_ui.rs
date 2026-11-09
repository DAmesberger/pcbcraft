//! The Board shape mode on the Layout canvas and in the right column: the
//! ribbon of sketch and board tools, the board sketch drawn over the board
//! (curves coloured by the solver as in the Footprint tab, named edges,
//! cutouts, holes and dimensions with their parameters), the live point
//! drag, and the Board shape tool card with the armed tool's options, the
//! selection's actions, the parameters and the named edges.
//!
//! Every finished gesture is one `board.edit` (one undo step); nothing here
//! changes the document by itself. The board parameters section is shared
//! with the Board card, so editing a parameter there re-solves the board
//! and moves the parts constrained to its edges.
use super::board_shape::{
    self as shape, CutoutShape, Gesture, Scene, ShapeOptions, ShapeTool, Step,
};
use super::{BoardInk, BoardView, PcbEditor, ZOOM_RANGE, board_view, zoom_pan};
use crate::kit::{self, ToolCard};
use crate::part_editor::{footprint_dims as dims, footprint_params};
use crate::refusal::{self, Origin};
use egui::{Align2, Color32, PointerButton, Pos2, Rect, Sense, Shape, Stroke, Ui, vec2};
use pcbcraft_core::layout::outline::{BoardCommand, BoardEdit, BoardShapeEvaluator};
use pcbcraft_core::library::footprint_source::{
    ParamName, SketchConstraintId, SketchPointId, SketchRef,
};
use pcbcraft_core::library::{Outline, Segment, bulge_arc};
use pcbcraft_core::model::board::{BoardSketch, CutoutKind, SolvedBoard};
use pcbcraft_core::{BoardShape, Engine, Operation, Point, Project};
use pcbcraft_solidcraft::footprint::view::{
    self as sketch_view, LiveSketch, ParamEvaluation, SketchView, SolveReport, ViewGeometry,
};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::{fonts, ui_kit};

/// Pick and snap distance, in pixels.
const PICK_PX: f32 = 7.0;
/// Chord tolerance of drawn outlines, mm.
const CHORD_MM: f64 = 0.02;

/// The loaded board sketch of one revision.
struct Loaded {
    key: (u64, usize),
    view: SketchView,
    live: Option<LiveSketch>,
    report: Option<SolveReport>,
    values: Vec<ParamEvaluation>,
}

/// A point being dragged: the live sketch follows the pointer each frame.
struct Drag {
    point: SketchPointId,
    live: LiveSketch,
    at: Point,
}

/// The Board shape mode's state (editor state, never the document).
pub(super) struct ShapeMode {
    pub(super) active: bool,
    pub(super) tool: ShapeTool,
    pub(super) options: ShapeOptions,
    pub(super) gesture: Gesture,
    pub(super) selection: Vec<SketchRef>,
    /// The selected dimension, whose value field the card shows.
    pub(super) dimension: Option<SketchConstraintId>,
    dim_text: Option<(SketchConstraintId, String)>,
    drag: Option<Drag>,
    loaded: Option<Loaded>,
    /// The host sketch engine (`board_shape::host_evaluator`).
    pub(super) evaluator: Option<&'static dyn BoardShapeEvaluator>,
    /// `board.start_sketch` was sent at this revision: the mode opens once
    /// the board has become a sketch.
    starting: Option<u64>,
    /// The parameters rows' text, per parameter, and the add row.
    pub(super) params: ParamRows,
    /// Dimension labels drawn last frame, for picking.
    labels: Vec<(SketchConstraintId, Rect)>,
    errors: std::collections::BTreeMap<&'static str, String>,
}

impl Default for ShapeMode {
    fn default() -> Self {
        Self {
            active: false,
            tool: ShapeTool::Select,
            options: ShapeOptions::default(),
            gesture: Gesture::default(),
            selection: Vec::new(),
            dimension: None,
            dim_text: None,
            drag: None,
            loaded: None,
            evaluator: shape::host_evaluator(),
            starting: None,
            params: ParamRows::default(),
            labels: Vec::new(),
            errors: Default::default(),
        }
    }
}

/// A refused parameter, why, and the board's key when it was refused.
type Refused = (ParamName, String, Option<(u64, usize)>);

/// The parameters section's text buffers.
#[derive(Default)]
pub(super) struct ParamRows {
    key: Option<(u64, usize)>,
    exprs: std::collections::BTreeMap<ParamName, String>,
    adding: bool,
    new_name: String,
    new_expr: String,
    /// A refused row (`None`: the add row) and why.
    error: Option<(Option<ParamName>, String)>,
    /// A parameter whose committed value the engine refused: the row went
    /// back to the stored value, and the whole reason shows under it until
    /// the row is edited or the document changes (the board's key then).
    refused: Option<Refused>,
    /// The row whose commit was sent last, to claim its refusal.
    sent: Option<Option<ParamName>>,
}

fn field_id(name: Option<&ParamName>) -> egui::Id {
    egui::Id::unique(("board-param", name.map(|n| n.0.clone())))
}

const DIM_FIELD: &str = "board-dimension";

impl ShapeMode {
    pub(super) fn reset_gesture(&mut self) {
        self.gesture.clear();
        self.drag = None;
    }

    /// Esc: the gesture, then the selection, then the tool, then the mode.
    /// Returns whether the mode stays open.
    pub(super) fn escape(&mut self) -> bool {
        if !self.gesture.is_empty() || self.drag.is_some() {
            self.reset_gesture();
        } else if !self.selection.is_empty() || self.dimension.is_some() {
            self.selection.clear();
            self.dimension = None;
        } else if self.tool != ShapeTool::Select {
            self.tool = ShapeTool::Select;
        } else {
            self.active = false;
        }
        self.active
    }

    pub(super) fn arm(&mut self, tool: ShapeTool) {
        self.reset_gesture();
        self.tool = tool;
        if tool != ShapeTool::Select {
            self.selection.clear();
        }
    }

    fn load(&mut self, engine: &Engine, board: &BoardSketch) {
        let key = (
            engine.revision(),
            std::sync::Arc::as_ptr(engine.project_arc()) as usize,
        );
        if self.loaded.as_ref().is_some_and(|l| l.key == key) {
            return;
        }
        let source = shape::source_of(board);
        let live = LiveSketch::load(&source, &[]).ok();
        let view = live
            .as_ref()
            .map(LiveSketch::view)
            .or_else(|| sketch_view::sketch_view(&source).ok())
            .unwrap_or_default();
        // Drop what the new revision no longer has.
        self.selection.retain(|r| match r {
            SketchRef::Point(p) => view.point(p).is_some(),
            SketchRef::Curve(c) => view.curve(c).is_some(),
            SketchRef::Constraint(k) => view.constraints.iter().any(|v| &v.id == k),
        });
        if self
            .dimension
            .as_ref()
            .is_some_and(|d| !view.constraints.iter().any(|k| &k.id == d))
        {
            self.dimension = None;
        }
        self.dim_text = None;
        self.drag = None;
        self.loaded = Some(Loaded {
            key,
            report: live.as_ref().map(LiveSketch::report),
            values: sketch_view::parameter_values(&source, &[]).unwrap_or_default(),
            view,
            live,
        });
    }
}

/// The board sketch, when the board is one.
pub(super) fn sketch_of(project: &Project) -> Option<&BoardSketch> {
    match &project.board_shape {
        BoardShape::Sketch(sketch) => Some(sketch),
        BoardShape::Rectangle => None,
    }
}

impl PcbEditor {
    /// Keep the mode with the document: it opens once a requested sketch
    /// exists and closes when the board is a rectangle again.
    pub(super) fn sync_shape(&mut self, engine: &Engine) {
        let project = engine.project();
        let is_sketch = sketch_of(project).is_some();
        if let Some(sent) = self.shape.starting
            && sent != engine.revision()
        {
            self.shape.starting = None;
            self.shape.active = is_sketch;
        }
        if !is_sketch {
            self.shape.active = false;
            self.shape.loaded = None;
            return;
        }
        if let Some(board) = sketch_of(project) {
            self.shape.load(engine, board);
        }
    }

    /// Open the Board shape mode; a rectangular board is first turned
    /// into its sketch (`board.start_sketch`, one undo step).
    pub(super) fn enter_shape(&mut self, engine: &Engine) -> Option<Operation> {
        self.end_route();
        self.cancel_drag();
        self.shape.arm(ShapeTool::Select);
        if sketch_of(engine.project()).is_some() {
            self.shape.active = true;
            None
        } else {
            self.shape.starting = Some(engine.revision());
            Some(Operation::Board(BoardCommand::StartSketch))
        }
    }

    /// The Board card's SHAPE section: what the outline is, Edit shape…
    /// (the Board shape mode), and the board parameters, editable here so
    /// a parameter change re-solves the board and moves the parts on its
    /// edges.
    pub(super) fn board_shape_section(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> Option<Operation> {
        let project = engine.project();
        let board = sketch_of(project);
        let summary = match board {
            None => "Rectangle".to_owned(),
            Some(b) => {
                let mut parts = vec!["Sketch".to_owned()];
                if !b.cutouts.is_empty() {
                    parts.push(format!("{} cutouts", b.cutouts.len()));
                }
                if !b.holes.is_empty() {
                    parts.push(format!("{} holes", b.holes.len()));
                }
                parts.push(format!("{} named edges", b.edges.len()));
                parts.join(" · ")
            }
        };
        let mut edit = false;
        kit::section(ui, "pcb-board-shape", "Shape", true, None, |ui| {
            kit::row(ui, "Outline", |ui| {
                edit = kit::summary_row(ui, &summary, Some("Edit shape…"));
            });
            let constrained = project.edge_constraints.len();
            if constrained > 0 {
                kit::value_row(
                    ui,
                    "On edges",
                    &if constrained == 1 {
                        "1 part".to_owned()
                    } else {
                        format!("{constrained} parts")
                    },
                );
            }
        });
        let mut operation = if edit { self.enter_shape(engine) } else { None };
        if let Some(board) = board {
            self.claim_shape_refusals(ui.ctx());
            if let Some(op) = self.parameters_section(ui, board, false) {
                operation = Some(op);
            }
        }
        operation
    }

    /// The ribbon groups of the Board shape mode.
    pub(super) fn shape_groups(&self) -> Vec<crate::shell::Group> {
        use crate::shell::{Group, Tool as Button};
        let button = |tool: ShapeTool| {
            Button::new(tool.id(), tool.icon(), tool.label())
                .selected(self.shape.tool == tool)
                .tip(tool.hint())
        };
        let group = |label, tools: &[ShapeTool]| Group {
            label,
            tools: tools.iter().map(|t| button(*t)).collect(),
        };
        vec![
            group(
                "Sketch",
                &[
                    ShapeTool::Select,
                    ShapeTool::Line,
                    ShapeTool::Arc,
                    ShapeTool::Circle,
                    ShapeTool::Rect,
                    ShapeTool::Fillet,
                ],
            ),
            group("Constrain", &[ShapeTool::Dimension]),
            group(
                "Board",
                &[
                    ShapeTool::Cutout,
                    ShapeTool::Slot,
                    ShapeTool::Hole,
                    ShapeTool::NameEdge,
                ],
            ),
            Group {
                label: "Shape",
                tools: vec![
                    Button::new("bs-done", "finish", "Done").tip("Leave the board shape (Esc)"),
                ],
            },
        ]
    }

    /// A Board shape ribbon tool; `None` when `id` is not one.
    pub(super) fn shape_tool(&mut self, id: &str, engine: &Engine) -> Option<Option<Operation>> {
        if id == "board-shape" {
            return Some(self.enter_shape(engine));
        }
        if id == "bs-done" {
            self.shape.active = false;
            self.shape.reset_gesture();
            return Some(None);
        }
        let tool = ShapeTool::from_id(id)?;
        self.shape.arm(tool);
        if tool == ShapeTool::NameEdge
            && self.shape.options.edge_name.is_empty()
            && let Some(board) = sketch_of(engine.project())
        {
            self.shape.options.edge_name = shape::next_edge_name(board);
        }
        Some(None)
    }

    /// The Board shape canvas: the board as Layout draws it, the sketch
    /// over it, and the armed tool's input.
    pub(super) fn shape_canvas(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let project = engine.project();
        let board = sketch_of(project)?;
        let t = Tokens::get();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        if response.dragged_by(PointerButton::Middle) {
            self.pan += response.drag_delta();
        }
        let wheel = self.trackpad.read(ui, false);
        if response.contains_pointer() && wheel.moves() {
            self.pan += wheel.pan;
            let zoom =
                (self.zoom * wheel.scale_factor()).clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end());
            if zoom != self.zoom
                && let Some(p) = response.hover_pos()
            {
                self.pan = zoom_pan(rect, project.board_size, self.zoom, zoom, self.pan, p);
            }
            self.zoom = zoom;
        }
        let view = board_view(
            rect,
            project.board_size,
            self.zoom,
            self.pan,
            self.from_bottom,
        );
        let tolerance = f64::from(PICK_PX / view.view.scale);
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.hover_pos()));
        let cursor = pointer.map(|p| view.world(p));
        let typing = ui.ctx().egui_wants_keyboard_input();
        let mut operation = None;
        let mut refused = None;
        if response.hovered() && !typing {
            let ctx = ui.ctx().clone();
            if crate::keys::pressed(&ctx, "shape.cancel") {
                self.shape.escape();
            }
            let delete = crate::keys::pressed(&ctx, "shape.delete");
            for tool in ShapeTool::ALL {
                if crate::keys::pressed_tool(&ctx, "shape", tool.id()) {
                    let _ = self.shape_tool(tool.id(), engine);
                }
            }
            if crate::keys::pressed(&ctx, "shape.construction") {
                self.shape.options.construction = !self.shape.options.construction;
            }
            if delete && self.shape.tool == ShapeTool::Select {
                let edits = shape::delete_edits(board, &self.shape.selection);
                if !edits.is_empty() {
                    self.shape.selection.clear();
                    operation = Some(shape::operation(edits));
                }
            }
        }
        // Taken for the frame, so the gesture can update the mode's state.
        let Some(loaded) = self.shape.loaded.take() else {
            return operation;
        };
        // Drag a free point: the live sketch follows; release commits.
        if self.shape.tool == ShapeTool::Select && response.drag_started_by(PointerButton::Primary)
        {
            let start = ui
                .input(|i| i.pointer.press_origin())
                .map(|p| view.world(p));
            if let (Some(start), Some(live)) = (start, loaded.live.clone())
                && let Some(point) =
                    dims::pick_free_point(&loaded.view, loaded.report.as_ref(), start, tolerance)
            {
                self.shape.drag = Some(Drag {
                    point,
                    live,
                    at: start,
                });
            }
        }
        if let (Some(drag), Some(at)) = (&mut self.shape.drag, cursor)
            && response.dragged_by(PointerButton::Primary)
            && let Ok(frame) = drag.live.drag(&drag.point, at)
        {
            drag.live = frame.sketch;
            drag.at = frame.at;
        }
        if response.drag_stopped_by(PointerButton::Primary)
            && let Some(drag) = self.shape.drag.take()
        {
            operation = Some(shape::operation(vec![BoardEdit::Sketch {
                edit: pcbcraft_core::library::footprint_source::SketchEdit::Drag {
                    point: drag.point,
                    to: drag.at,
                },
            }]));
        }
        if response.clicked_by(PointerButton::Secondary) {
            self.shape.reset_gesture();
        }
        if response.clicked_by(PointerButton::Primary)
            && let Some(at) = cursor
        {
            let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            let label = pointer.and_then(|p| {
                self.shape
                    .labels
                    .iter()
                    .find(|(_, r)| r.contains(p))
                    .map(|(id, _)| id.clone())
            });
            if let Some(id) = label.filter(|_| {
                matches!(self.shape.tool, ShapeTool::Select | ShapeTool::Dimension)
                    && self.shape.gesture.is_empty()
            }) {
                self.shape.dimension = Some(id);
                self.shape.dim_text = None;
            } else if self.shape.tool == ShapeTool::Select {
                let pick = dims::pick(&loaded.view, at, tolerance);
                if !additive {
                    self.shape.selection.clear();
                    self.shape.dimension = None;
                }
                if let Some(pick) = pick {
                    if let Some(k) = self.shape.selection.iter().position(|r| *r == pick) {
                        self.shape.selection.remove(k);
                    } else {
                        self.shape.selection.push(pick);
                    }
                }
            } else if let Some(evaluator) = self.shape.evaluator {
                let scene = Scene {
                    board,
                    view: &loaded.view,
                    evaluator,
                    tolerance,
                };
                match shape::press(
                    self.shape.tool,
                    &self.shape.options,
                    &mut self.shape.gesture,
                    at,
                    &scene,
                ) {
                    Ok(Step::Commit(commit)) => {
                        if self.shape.tool == ShapeTool::NameEdge {
                            self.shape.options.edge_name = bump_name(&self.shape.options.edge_name);
                        }
                        self.shape.dimension = commit.select;
                        operation = Some(shape::operation(commit.edits));
                    }
                    Ok(Step::More) => {}
                    Err(error) => refused = Some(error),
                }
            } else {
                refused = Some(pcbcraft_core::Error::Unsupported(
                    "board.evaluator: board sketches need the host's sketch engine".into(),
                ));
            }
        }
        self.shape.loaded = Some(loaded);
        if let Some(error) = refused {
            kit::toast::post(
                ui.ctx(),
                refusal::level(&error),
                refusal::shown_error(&error),
                vec![],
            );
        }
        let painter = ui.painter().with_clip_rect(rect);
        self.paint_board(&painter, view, project);
        self.paint_sketch(&painter, view, board, cursor, pointer);
        let tool = self.shape.tool;
        let status = self
            .shape
            .loaded
            .as_ref()
            .and_then(|l| l.report.as_ref())
            .map_or_else(|| "Not solved".to_owned(), dims::status_text);
        kit::canvas_hint(
            &painter,
            rect,
            t.space_16,
            &format!(
                "Board shape · {} · {status}{}",
                tool.label(),
                if self.shape.options.construction {
                    " · construction"
                } else {
                    ""
                }
            ),
        );
        operation
    }

    /// The sketch over the board: curves by solve state, named edges,
    /// points, dimensions, holes and the gesture's preview.
    fn paint_sketch(
        &mut self,
        painter: &egui::Painter,
        view: BoardView,
        board: &BoardSketch,
        cursor: Option<Point>,
        pointer: Option<Pos2>,
    ) {
        let t = Tokens::get();
        let Some(loaded) = &self.shape.loaded else {
            return;
        };
        let drawn = self
            .shape
            .drag
            .as_ref()
            .map(|d| d.live.view())
            .unwrap_or_else(|| loaded.view.clone());
        let report = loaded.report.as_ref();
        let conflicting: Vec<_> = report.map(|r| r.conflicting.clone()).unwrap_or_default();
        let conflict_curves: Vec<_> = drawn
            .constraints
            .iter()
            .filter(|k| conflicting.contains(&k.id))
            .flat_map(|k| k.curves.iter().cloned())
            .collect();
        let selected = |r: &SketchRef| self.shape.selection.contains(r);
        let picks = &self.shape.gesture.picks;
        let cut: Vec<_> = board.cutouts.iter().flat_map(|c| c.curves.iter()).collect();
        for curve in &drawn.curves {
            let r = SketchRef::Curve(curve.id.clone());
            let line: Vec<Pos2> = polyline(&curve.geometry)
                .into_iter()
                .map(|p| view.screen(p))
                .collect();
            let color = if selected(&r) || picks.contains(&r) {
                t.accent
            } else if conflict_curves.contains(&curve.id) {
                t.sketch_conflict
            } else if cut.contains(&&curve.id) {
                t.preview_cut
            } else if report.is_none_or(|r| r.free_curves.contains(&curve.id)) {
                t.sketch
            } else {
                t.sketch_fixed
            };
            let width = if color == t.accent { 2.5 } else { 1.5 };
            if curve.construction {
                dashed(painter, &line, Stroke::new(width, color));
            } else {
                painter.add(Shape::line(line, Stroke::new(width, color)));
            }
        }
        for point in drawn.points.iter().filter(|p| p.id.0 != "origin") {
            let r = SketchRef::Point(point.id.clone());
            let free = report.is_some_and(|rep| rep.free_points.contains(&point.id));
            let color = if selected(&r) || picks.contains(&r) {
                t.accent
            } else if free {
                t.sketch_point_free
            } else {
                t.sketch_point
            };
            painter.circle_filled(view.screen(point.at), if free { 2.5 } else { 2.0 }, color);
        }
        for hole in &board.holes {
            if let Some(p) = drawn.point(&hole.center) {
                let radius = (hole_size(&hole.drill.0) as f32 * view.view.scale / 2.0).max(3.0);
                painter.circle_stroke(view.screen(p.at), radius, Stroke::new(1.5, t.sketch_accent));
            }
        }
        // Dimensions.
        let px = f64::from(1.0 / view.view.scale);
        let font = fonts::medium(t.font_small);
        let labels: Vec<(SketchConstraintId, String)> = drawn
            .constraints
            .iter()
            .filter(|k| k.value.is_some())
            .map(|k| (k.id.clone(), dims::label(k)))
            .collect();
        let char_px = t.font_small * 0.56;
        let half_of = |id: &SketchConstraintId| -> Point {
            let chars = labels
                .iter()
                .find(|(k, _)| k == id)
                .map_or(4, |(_, s)| s.chars().count());
            [
                f64::from(chars as f32 * char_px / 2.0 + 3.0) * px,
                f64::from(t.font_small / 2.0 + 2.0) * px,
            ]
        };
        let live = self
            .shape
            .drag
            .as_ref()
            .map(|d| &d.live)
            .or(loaded.live.as_ref());
        let layouts = live.map(|l| l.dimensions(px, &half_of)).unwrap_or_default();
        let mut rects = Vec::new();
        for layout in &layouts {
            let lit = self.shape.dimension.as_ref() == Some(&layout.id)
                || pointer.is_some_and(|p| {
                    self.shape
                        .labels
                        .iter()
                        .any(|(id, r)| *id == layout.id && r.contains(p))
                });
            let color = if lit { t.accent } else { t.text_dim };
            for line in &layout.lines {
                let line: Vec<Pos2> = line.iter().map(|p| view.screen(*p)).collect();
                painter.add(Shape::line(line, Stroke::new(1.0, color)));
            }
            for (tip, dir) in &layout.arrows {
                // The direction through the view (the bottom view mirrors).
                let ahead = view.screen([tip[0] + dir[0], tip[1] + dir[1]]);
                let tip = view.screen(*tip);
                let d = ahead - tip;
                let d = if d.length() > 1e-6 {
                    d.normalized()
                } else {
                    vec2(1.0, 0.0)
                };
                let n = vec2(-d.y, d.x);
                let back = tip - d * 8.0;
                painter.add(Shape::convex_polygon(
                    vec![tip, back + n * 2.6, back - n * 2.6],
                    color,
                    Stroke::NONE,
                ));
            }
            let text = labels
                .iter()
                .find(|(k, _)| *k == layout.id)
                .map_or_else(String::new, |(_, s)| s.clone());
            let galley = painter.layout_no_wrap(text.clone(), font.clone(), color);
            let r =
                Rect::from_center_size(view.screen(layout.text), galley.size() + vec2(6.0, 2.0));
            // Two dimensions meeting at a corner (insets to one hole) would
            // stack their labels: slide this one along its own line clear.
            let along = layout
                .arrows
                .first()
                .map(|(tip, dir)| {
                    view.screen([tip[0] + dir[0], tip[1] + dir[1]]) - view.screen(*tip)
                })
                .filter(|d| d.length() > 1e-6)
                .map_or(vec2(1.0, 0.0), |d| d.normalized());
            let placed: Vec<Rect> = rects.iter().map(|(_, r)| *r).collect();
            let r = clear_of(r, along, &placed);
            let centre = r.center();
            painter.rect_filled(r, 2.0, t.panel);
            painter.text(centre, Align2::CENTER_CENTER, text, font.clone(), color);
            rects.push((layout.id.clone(), r));
        }
        // Named edges: each name centred on its edge just outside the
        // outline, slid along or away from the edge to stay clear of the
        // dimension labels, the other names and the parts' pads.
        let mut taken: Vec<Rect> = rects.iter().map(|(_, r)| *r).collect();
        taken.extend(self.board_pads.iter().map(|p| {
            let b = p.outline.bounds(0.0);
            Rect::from_two_pos(view.screen(b.min), view.screen(b.max))
        }));
        let edge_lines: Vec<(&str, Vec<Pos2>)> = board
            .edges
            .iter()
            .filter_map(|edge| {
                let curve = drawn.curve(&edge.curve)?;
                let line: Vec<Pos2> = polyline(&curve.geometry)
                    .into_iter()
                    .map(|p| view.screen(p))
                    .collect();
                (line.len() >= 2).then_some((edge.name.0.as_str(), line))
            })
            .collect();
        let all: Vec<Pos2> = edge_lines
            .iter()
            .flat_map(|(_, l)| l.iter().copied())
            .collect();
        let centre = all.iter().fold(Pos2::ZERO, |c, p| c + p.to_vec2()) / all.len().max(1) as f32;
        for (name, line) in &edge_lines {
            let galley = painter.layout_no_wrap((*name).to_owned(), font.clone(), t.sketch_accent);
            if let Some(r) = edge_name_rect(line, centre, galley.size(), t.space_4, &taken) {
                painter.galley(r.min, galley, t.sketch_accent);
                taken.push(r);
            }
        }
        self.shape.labels = rects;
        // The gesture's preview: clicked points and a rubber band.
        if let Some(at) = cursor {
            let pending: Vec<Point> = self.shape.gesture.pending.iter().map(|c| c.at).collect();
            let stroke = Stroke::new(1.0, t.preview_sketch);
            match (self.shape.tool, pending.as_slice()) {
                (ShapeTool::Line | ShapeTool::Slot, [a]) => {
                    painter.line_segment([view.screen(*a), view.screen(at)], stroke);
                }
                (ShapeTool::Rect, [a]) | (ShapeTool::Cutout, [a])
                    if self.shape.tool == ShapeTool::Rect
                        || self.shape.options.cutout == CutoutShape::Rect =>
                {
                    let corners = [*a, [at[0], a[1]], at, [a[0], at[1]], *a];
                    painter.add(Shape::line(
                        corners.iter().map(|p| view.screen(*p)).collect(),
                        stroke,
                    ));
                }
                (ShapeTool::Circle | ShapeTool::Cutout, [c]) => {
                    let r = ((at[0] - c[0]).hypot(at[1] - c[1]) as f32) * view.view.scale;
                    painter.circle_stroke(view.screen(*c), r, stroke);
                }
                (ShapeTool::Arc, [c]) | (ShapeTool::Arc, [c, _]) => {
                    painter.line_segment([view.screen(*c), view.screen(at)], stroke);
                }
                _ => {}
            }
            for p in &pending {
                painter.circle_stroke(view.screen(*p), 4.0, Stroke::new(1.5, t.accent));
            }
            if self.shape.tool != ShapeTool::Select
                && let Some(snapped) =
                    drawn
                        .points
                        .iter()
                        .filter(|p| p.id.0 != "origin")
                        .find(|p| {
                            (p.at[0] - at[0]).hypot(p.at[1] - at[1])
                                <= f64::from(PICK_PX / view.view.scale)
                        })
            {
                painter.circle_stroke(view.screen(snapped.at), 6.0, Stroke::new(1.5, t.accent));
            }
        }
    }

    /// The Board shape tool card: the armed tool's options, the selection
    /// and its actions, the selected dimension's value, the parameters and
    /// the named edges. Done leaves the mode.
    pub(super) fn shape_card(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let project = engine.project();
        let board = sketch_of(project)?;
        self.claim_shape_refusals(ui.ctx());
        let tool = self.shape.tool;
        let card = ToolCard {
            icon: tool.icon(),
            name: if tool == ShapeTool::Select {
                "Board shape"
            } else {
                tool.label()
            },
            key: tool.key_text(),
            hint: tool.hint(),
            tip: "The board outline is a parametric sketch: dimensions bind to the board \
                  parameters, and parts constrained to a named edge follow it when the \
                  board re-solves. Each change is one undo step.",
            finish: None,
        };
        let t = Tokens::get();
        let margin = egui::Margin::symmetric(t.space_8 as i8, 0);
        let mut operation = None;
        let response = egui::Frame::NONE
            .inner_margin(margin)
            .show(ui, |ui| {
                kit::tool_card(ui, &card, |ui| {
                    operation = self.shape_card_rows(ui, project, board);
                })
            })
            .inner;
        if response.done {
            self.shape.active = false;
            self.shape.reset_gesture();
        }
        operation
    }

    fn shape_card_rows(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        board: &BoardSketch,
    ) -> Option<Operation> {
        let mut operation = None;
        let o = &mut self.shape.options;
        match self.shape.tool {
            ShapeTool::Line | ShapeTool::Arc | ShapeTool::Circle => {
                kit::check_row(ui, "Construction", &mut o.construction);
            }
            ShapeTool::Rect => {
                kit::number_row(ui, "Corner radius", &mut o.corner_radius, "mm", 0.0..=500.0);
                kit::check_row(ui, "Construction", &mut o.construction);
            }
            ShapeTool::Fillet => {
                kit::number_row(ui, "Radius", &mut o.fillet_radius, "mm", 0.01..=500.0);
            }
            ShapeTool::Cutout => {
                kit::choice_row(
                    ui,
                    "Shape",
                    &mut o.cutout,
                    &[
                        (CutoutShape::Rect, "Rectangle".to_owned()),
                        (CutoutShape::Circle, "Circle".to_owned()),
                    ],
                );
            }
            ShapeTool::Slot => {
                kit::number_row(ui, "Width", &mut o.slot_width, "mm", 0.1..=50.0);
                kit::check_row(ui, "Plated", &mut o.slot_plated);
            }
            ShapeTool::Hole => {
                kit::number_row(ui, "Drill", &mut o.hole_drill, "mm", 0.1..=50.0);
                kit::check_row(ui, "Plated", &mut o.hole_plated);
                if o.hole_plated {
                    kit::number_row(ui, "Pad", &mut o.hole_pad, "mm", 0.1..=80.0);
                }
            }
            ShapeTool::NameEdge => {
                let r = kit::text_row(ui, "Name", &mut o.edge_name);
                if !shape::edge_name_valid(&pcbcraft_core::model::board::EdgeName(
                    o.edge_name.trim().to_owned(),
                )) {
                    kit::field_error(ui, &r, "Use letters, digits and _", true);
                }
            }
            ShapeTool::Select | ShapeTool::Dimension => {}
        }
        if let Some(op) = self.dimension_rows(ui, board) {
            operation = Some(op);
        }
        if let Some(op) = self.selection_rows(ui, board) {
            operation = Some(op);
        }
        let status = self
            .shape
            .loaded
            .as_ref()
            .and_then(|l| l.report.as_ref())
            .map_or_else(|| "Not solved".to_owned(), dims::status_text);
        kit::value_row(ui, "Sketch", &status);
        if let Some(op) = self.parameters_section(ui, board, true) {
            operation = Some(op);
        }
        if let Some(op) = self.edges_section(ui, project, board) {
            operation = Some(op);
        }
        operation
    }

    /// The selected dimension's value: a number, a parameter, an
    /// expression or `name = expr`; Enter commits it.
    fn dimension_rows(&mut self, ui: &mut Ui, board: &BoardSketch) -> Option<Operation> {
        let id = self.shape.dimension.clone()?;
        let loaded = self.shape.loaded.as_ref()?;
        let shown = loaded.view.constraints.iter().find(|k| k.id == id)?;
        if self.shape.dim_text.as_ref().is_none_or(|(k, _)| *k != id) {
            self.shape.dim_text = Some((id.clone(), dims::field_text(shown)));
        }
        let view = loaded.view.clone();
        let label = dims::label(shown);
        let original = dims::field_text(shown);
        let (_, text) = self.shape.dim_text.as_mut()?;
        let mut operation = None;
        kit::section(ui, "board-dimension", "Dimension", true, None, |ui| {
            kit::value_row(ui, "Shows", &label);
            let field = kit::text_row(ui, "Value", text);
            if field.lost_focus() && text.trim() != original {
                match shape::dimension_value_edits(board, &view, &id, text) {
                    Ok(edits) => {
                        self.shape.errors.remove(DIM_FIELD);
                        refusal::mark(ui.ctx(), Origin::Field(egui::Id::unique(DIM_FIELD)));
                        operation = Some(shape::operation(edits));
                    }
                    Err(why) => {
                        self.shape.errors.insert(DIM_FIELD, why);
                    }
                }
            }
            if let Some(why) = self.shape.errors.get(DIM_FIELD) {
                kit::field_error(ui, &field, why, true);
            }
            kit::hint(
                ui,
                "A number, a parameter, or name = expression",
                "Type width, or width = 80 mm, to bind the dimension to a board parameter",
            );
        });
        operation
    }

    /// What the selection can become: the outline, a cutout or a slot.
    fn selection_rows(&mut self, ui: &mut Ui, board: &BoardSketch) -> Option<Operation> {
        if self.shape.tool != ShapeTool::Select || self.shape.selection.is_empty() {
            return None;
        }
        let curves = self
            .shape
            .selection
            .iter()
            .filter(|r| matches!(r, SketchRef::Curve(_)))
            .count();
        let points = self.shape.selection.len() - curves;
        let mut edits = None;
        kit::section(ui, "board-selection", "Selection", true, None, |ui| {
            kit::value_row(
                ui,
                "Selected",
                &format!(
                    "{curves} {} · {points} {}",
                    if curves == 1 { "curve" } else { "curves" },
                    if points == 1 { "point" } else { "points" }
                ),
            );
            if curves > 0 {
                if kit::summary_row(ui, "Outer edge of the board", Some("Set as outline")) {
                    edits = shape::outline_edit(&self.shape.selection).map(|e| vec![e]);
                }
                if kit::summary_row(ui, "Material removed inside", Some("Make cutout")) {
                    edits = shape::cutout_edit(&self.shape.selection, CutoutKind::Cutout)
                        .map(|e| vec![e]);
                }
            }
            if kit::summary_row(ui, "", Some("Delete")) {
                edits = Some(shape::delete_edits(board, &self.shape.selection));
            }
        });
        let edits = edits.filter(|e| !e.is_empty())?;
        self.shape.selection.clear();
        Some(shape::operation(edits))
    }

    /// BOARD PARAMETERS: each parameter's expression, committed on Enter
    /// or focus loss as one `board.edit`, with its value under it. Shared
    /// by the Board card and the Board shape card (`editable` adds the
    /// remove and add controls).
    pub(super) fn parameters_section(
        &mut self,
        ui: &mut Ui,
        board: &BoardSketch,
        editable: bool,
    ) -> Option<Operation> {
        let values = self
            .shape
            .loaded
            .as_ref()
            .map(|l| l.values.clone())
            .unwrap_or_default();
        let rows = &mut self.shape.params;
        let key = self.shape.loaded.as_ref().map(|l| l.key);
        if rows.key != key {
            rows.key = key;
            rows.exprs = board
                .parameters
                .iter()
                .map(|p| (p.name.clone(), p.expr.0.clone()))
                .collect();
        }
        // A refusal is about the value that was refused: it goes when the
        // document changes or its row is edited again.
        if let Some((name, _, at)) = &rows.refused
            && (*at != key
                || board.parameters.iter().any(|p| {
                    p.name == *name && rows.exprs.get(name).is_some_and(|t| t.trim() != p.expr.0)
                }))
        {
            rows.refused = None;
        }
        let mut operation = None;
        let mut sent = None;
        kit::section(
            ui,
            "board-parameters",
            "Board parameters",
            true,
            Some(board.parameters.len()),
            |ui| {
                for param in &board.parameters {
                    let Some(text) = rows.exprs.get_mut(&param.name) else {
                        continue;
                    };
                    let failed = rows
                        .error
                        .as_ref()
                        .is_some_and(|(n, _)| n.as_ref() == Some(&param.name));
                    let field = kit::row(ui, &param.name.0, |ui| {
                        ui.horizontal(|ui| {
                            let t = Tokens::get();
                            let width = ui.available_width()
                                - if editable {
                                    t.control_height + t.space_4
                                } else {
                                    0.0
                                };
                            let r = ui
                                .scope(|ui| {
                                    ui.set_max_width(width.max(48.0));
                                    ui.spacing_mut().text_edit_width = width.max(48.0);
                                    kit::text_field(
                                        ui,
                                        &format!("board-param-{}", param.name.0),
                                        text,
                                        "expression",
                                        failed,
                                    )
                                })
                                .inner;
                            if editable
                                && ui_kit::button(ui, "×", ui_kit::Button::Small)
                                    .on_hover_text(
                                        "Remove this parameter (refused while it is read)",
                                    )
                                    .clicked()
                            {
                                operation =
                                    Some(shape::operation(vec![BoardEdit::RemoveParameter {
                                        name: param.name.clone(),
                                    }]));
                                sent = Some(Some(param.name.clone()));
                            }
                            r
                        })
                        .inner
                    });
                    if field.lost_focus() && text.trim() != param.expr.0 {
                        match shape::parameter_edit(board, &param.name, text, param.unit) {
                            Ok(edit) => {
                                operation = Some(shape::operation(vec![edit]));
                                sent = Some(Some(param.name.clone()));
                                rows.error = None;
                            }
                            Err(why) => rows.error = Some((Some(param.name.clone()), why)),
                        }
                    } else if field.lost_focus() && failed {
                        // Back at the stored value: the earlier text's error goes.
                        rows.error = None;
                    }
                    let refused = rows
                        .refused
                        .as_ref()
                        .filter(|(n, _, _)| *n == param.name)
                        .map(|(_, why, _)| why.clone());
                    match (&rows.error, values.iter().find(|v| v.name == param.name)) {
                        (Some((Some(n), why)), _) if *n == param.name => {
                            kit::field_error(ui, &field, why, true);
                        }
                        _ if refused.is_some() => {
                            kit::field_refused(ui, &field, refused.as_deref().unwrap_or_default());
                        }
                        (_, Some(ParamEvaluation { value: Ok(v), .. })) => {
                            kit::note(ui, &format!("= {}", footprint_params::value_text(*v)));
                        }
                        (
                            _,
                            Some(ParamEvaluation {
                                value: Err(why), ..
                            }),
                        ) => {
                            kit::field_error(ui, &field, why, false);
                        }
                        _ => {}
                    }
                }
                if board.parameters.is_empty() {
                    kit::hint(
                        ui,
                        "Name a dimension's value to add a parameter",
                        "Type width = 80 mm into a dimension; parts placed on the board's edges follow it",
                    );
                }
                if !editable {
                    return;
                }
                if rows.adding {
                    let name = kit::text_row(ui, "New name", &mut rows.new_name);
                    let expr = kit::text_row(ui, "Expression", &mut rows.new_expr);
                    if let Some((None, why)) = &rows.error {
                        let field = if ParamName(rows.new_name.trim().into()).valid() {
                            &expr
                        } else {
                            &name
                        };
                        kit::field_error(ui, field, why, true);
                    }
                    let mut cancel = false;
                    kit::actions(ui, |ui| {
                        let ready =
                            !rows.new_name.trim().is_empty() && !rows.new_expr.trim().is_empty();
                        if kit::primary(ui, "Add", ready).clicked() && ready {
                            let name = ParamName(rows.new_name.trim().into());
                            let unit = footprint_params::unit_of(&rows.new_expr);
                            if board.parameters.iter().any(|p| p.name == name) {
                                rows.error = Some((None, format!("{} already exists", name.0)));
                            } else {
                                match shape::parameter_edit(board, &name, &rows.new_expr, unit) {
                                    Ok(edit) => {
                                        operation = Some(shape::operation(vec![edit]));
                                        sent = Some(None);
                                        rows.error = None;
                                    }
                                    Err(why) => rows.error = Some((None, why)),
                                }
                            }
                        }
                        if ui_kit::button(ui, "Cancel", ui_kit::Button::Normal).clicked() {
                            cancel = true;
                        }
                    });
                    if cancel {
                        rows.adding = false;
                        rows.new_name.clear();
                        rows.new_expr.clear();
                        rows.error = None;
                    }
                } else if kit::summary_row(ui, "", Some("Add parameter…")) {
                    rows.adding = true;
                }
            },
        );
        if let Some(row) = sent {
            refusal::mark(ui.ctx(), Origin::Field(field_id(row.as_ref())));
            if row.is_none() {
                rows.adding = false;
                rows.new_name.clear();
                rows.new_expr.clear();
            }
            rows.sent = Some(row);
        }
        operation
    }

    /// NAMED EDGES: each name with its edge; × unnames it.
    fn edges_section(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        board: &BoardSketch,
    ) -> Option<Operation> {
        let mut operation = None;
        kit::section(
            ui,
            "board-edges",
            "Named edges",
            true,
            Some(board.edges.len()),
            |ui| {
                for edge in board.edges.iter().take(kit::LIST_MAX_ROWS) {
                    let used = project
                        .edge_constraints
                        .iter()
                        .filter(|c| c.edge == edge.name)
                        .count();
                    if kit::row(ui, &edge.name.0, |ui| {
                        kit::summary_row(
                            ui,
                            &match used {
                                0 => "No parts".to_owned(),
                                1 => "1 part".to_owned(),
                                n => format!("{n} parts"),
                            },
                            Some("Unname"),
                        )
                    }) {
                        operation = Some(shape::operation(vec![BoardEdit::UnnameEdge {
                            name: edge.name.clone(),
                        }]));
                    }
                }
                if board.edges.len() > kit::LIST_MAX_ROWS {
                    kit::note(
                        ui,
                        &format!(
                            "{} more named edges",
                            board.edges.len() - kit::LIST_MAX_ROWS
                        ),
                    );
                }
                if board.edges.is_empty() {
                    kit::hint(ui, "Name an edge (E) to place parts relative to it", "");
                }
                let cutouts = board.cutouts.len();
                let holes = board.holes.len();
                if cutouts + holes > 0 {
                    kit::value_row(
                        ui,
                        "Features",
                        &format!("{cutouts} cutouts and slots · {holes} holes"),
                    );
                }
            },
        );
        operation
    }

    fn claim_shape_refusals(&mut self, ctx: &egui::Context) {
        if let Some(text) = refusal::claim(ctx, Origin::Field(egui::Id::unique(DIM_FIELD))) {
            self.shape.errors.insert(DIM_FIELD, text);
        }
        if let Some(row) = self.shape.params.sent.clone()
            && let Some(text) = refusal::claim(ctx, Origin::Field(field_id(row.as_ref())))
        {
            let rows = &mut self.shape.params;
            match row {
                // A refused parameter keeps its stored, valid value: the
                // row reloads it, and the reason shows in full.
                Some(name) => {
                    let key = self.shape.loaded.as_ref().map(|l| l.key);
                    rows.refused = Some((name.clone(), text, key));
                    if rows
                        .error
                        .as_ref()
                        .is_some_and(|(n, _)| n.as_ref() == Some(&name))
                    {
                        rows.error = None;
                    }
                    rows.key = None;
                }
                None => rows.error = Some((None, text)),
            }
            rows.sent = None;
        }
    }
}

fn bump_name(name: &str) -> String {
    let digits = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let n: u32 = name[digits.len()..].parse().unwrap_or(1);
    format!("{digits}{}", n.saturating_add(1))
}

/// A dimension label box `r` moved along its dimension's screen direction
/// `along` (a unit vector), by whole steps alternating forward and back,
/// to the nearest place clear of the `placed` labels; `r` itself when it is
/// clear or no step within reach is.
pub(super) fn clear_of(r: Rect, along: egui::Vec2, placed: &[Rect]) -> Rect {
    let free = |r: &Rect| placed.iter().all(|p| !p.intersects(*r));
    if free(&r) {
        return r;
    }
    // One step clears a box of the same size beside it along `along`.
    let step = along.x.abs() * r.width() + along.y.abs() * r.height() + 2.0;
    (1..=6u8)
        .flat_map(|k| [f32::from(k), -f32::from(k)])
        .map(|k| r.translate(along * step * k))
        .find(free)
        .unwrap_or(r)
}

/// Where an edge's name goes on screen: centred on the edge `line` just
/// outside it (away from the outline's `centre`), `gap` points off; when
/// that box meets one of `taken`, the first free box slid along the edge
/// or farther out. `None` for a degenerate edge.
pub(super) fn edge_name_rect(
    line: &[Pos2],
    centre: Pos2,
    size: egui::Vec2,
    gap: f32,
    taken: &[Rect],
) -> Option<Rect> {
    let length: f32 = line.windows(2).map(|w| w[0].distance(w[1])).sum();
    if length <= 1e-3 {
        return None;
    }
    // The point and direction a fraction `f` along the line.
    let at = |f: f32| -> (Pos2, egui::Vec2) {
        let mut left = f.clamp(0.0, 1.0) * length;
        for w in line.windows(2) {
            let d = w[0].distance(w[1]);
            if left <= d && d > 0.0 {
                return (w[0] + (w[1] - w[0]) * (left / d), (w[1] - w[0]) / d);
            }
            left -= d;
        }
        let n = line.len();
        (line[n - 1], (line[n - 1] - line[n - 2]).normalized())
    };
    let boxed = |f: f32, out: f32| {
        let (p, dir) = at(f);
        let mut n = egui::vec2(-dir.y, dir.x);
        if (p - centre).dot(n) < 0.0 {
            n = -n;
        }
        let reach = n.x.abs() * size.x / 2.0 + n.y.abs() * size.y / 2.0 + gap + out;
        Rect::from_center_size(p + n * reach, size)
    };
    let free = |r: &Rect| !taken.iter().any(|o| o.intersects(*r));
    let along = [0.5, 0.35, 0.65, 0.2, 0.8];
    [0.0, size.y + gap, 2.0 * (size.y + gap)]
        .iter()
        .flat_map(|out| along.iter().map(move |f| (*f, *out)))
        .map(|(f, out)| boxed(f, out))
        .find(free)
        .or_else(|| Some(boxed(0.5, 0.0)))
}

/// The drill of a hole expression in mm (`"3.2 mm"`), for drawing.
fn hole_size(expr: &str) -> f64 {
    kit::parse_number(expr, "mm").unwrap_or(3.0)
}

/// A curve's drawn polyline in board mm.
pub(super) fn polyline(geometry: &ViewGeometry) -> Vec<Point> {
    let steps = 48;
    match *geometry {
        ViewGeometry::Line { a, b } => vec![a, b],
        ViewGeometry::Circle { center, radius } => (0..=steps)
            .map(|k| {
                let a = std::f64::consts::TAU * f64::from(k) / f64::from(steps);
                [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
            })
            .collect(),
        ViewGeometry::Arc { center, start, end } => {
            let r = (start[0] - center[0]).hypot(start[1] - center[1]);
            let a0 = (start[1] - center[1]).atan2(start[0] - center[0]);
            let a1 = (end[1] - center[1]).atan2(end[0] - center[0]);
            let mut sweep = (a1 - a0).rem_euclid(std::f64::consts::TAU);
            if sweep <= 1e-12 {
                sweep = std::f64::consts::TAU;
            }
            (0..=steps)
                .map(|k| {
                    let a = a0 + sweep * f64::from(k) / f64::from(steps);
                    [center[0] + r * a.cos(), center[1] + r * a.sin()]
                })
                .collect()
        }
        ViewGeometry::Other => Vec::new(),
    }
}

fn dashed(painter: &egui::Painter, points: &[Pos2], stroke: Stroke) {
    for pair in points.windows(2) {
        if let [a, b] = pair {
            painter.add(Shape::dashed_line(&[*a, *b], stroke, 5.0, 3.0));
        }
    }
}

/// An outline flattened to a ring (no repeated first point).
pub(super) fn ring(outline: &Outline) -> Vec<Point> {
    let mut out = vec![outline.start];
    let mut at = outline.start;
    for segment in &outline.segments {
        match *segment {
            Segment::Line { to } => {
                out.push(to);
                at = to;
            }
            Segment::Arc { to, bulge } => {
                match bulge_arc(at, to, bulge) {
                    Some(arc) => out.extend(arc.flatten(CHORD_MM).into_iter().skip(1)),
                    None => out.push(to),
                }
                at = to;
            }
        }
    }
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// Triangles of a simple polygon (ear clipping), as index triples.
pub(super) fn triangulate(ring: &[Pos2]) -> Vec<[u32; 3]> {
    let n = ring.len();
    if !(3..=20_000).contains(&n) {
        return Vec::new();
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    // Screen y points down: a board counter-clockwise ring is clockwise
    // here. Work on a counter-clockwise (in screen terms) index order.
    let mut idx: Vec<usize> = if area > 0.0 {
        (0..n).collect()
    } else {
        (0..n).rev().collect()
    };
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut out = Vec::with_capacity(n - 2);
    let mut guard = 0usize;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let (i0, i1, i2) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (ring[i0], ring[i1], ring[i2]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                j != i0
                    && j != i1
                    && j != i2
                    // On an edge counts as inside: a reflex corner on
                    // the diagonal must not be cut off.
                    && cross(a, b, ring[j]) >= 0.0
                    && cross(b, c, ring[j]) >= 0.0
                    && cross(c, a, ring[j]) >= 0.0
            });
            if inside {
                continue;
            }
            out.push([i0 as u32, i1 as u32, i2 as u32]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if let [a, b, c] = idx.as_slice() {
        out.push([*a as u32, *b as u32, *c as u32]);
    }
    out
}

/// Fill a simple ring with `color`.
fn fill_ring(painter: &egui::Painter, points: &[Pos2], color: Color32) {
    let mut mesh = egui::Mesh::default();
    for p in points {
        mesh.colored_vertex(*p, color);
    }
    for [a, b, c] in triangulate(points) {
        mesh.add_triangle(a, b, c);
    }
    painter.add(Shape::mesh(mesh));
}

/// The solved board: the outer outline filled with the board colour and
/// edged, its cutouts and slots open (the canvas shows through) and its
/// mounting holes drilled.
pub(super) fn paint_solved(
    painter: &egui::Painter,
    view: &BoardView,
    solved: &SolvedBoard,
    ink: &BoardInk,
    edge: Stroke,
) {
    let outer: Vec<Pos2> = ring(&solved.outer)
        .iter()
        .map(|p| view.screen(*p))
        .collect();
    fill_ring(painter, &outer, ink.board);
    let mut closed = outer.clone();
    if let Some(first) = outer.first() {
        closed.push(*first);
    }
    painter.add(Shape::line(closed, edge));
    for cutout in &solved.cutouts {
        let inner: Vec<Pos2> = ring(&cutout.outline)
            .iter()
            .map(|p| view.screen(*p))
            .collect();
        fill_ring(painter, &inner, ink.drill);
        let mut closed = inner.clone();
        if let Some(first) = inner.first() {
            closed.push(*first);
        }
        let stroke = match cutout.kind {
            CutoutKind::Slot { plated: true } => Stroke::new(edge.width, ink.plated),
            _ => edge,
        };
        painter.add(Shape::line(closed, stroke));
    }
    for hole in &solved.holes {
        let at = view.screen(hole.at);
        if let Some(pad) = hole.pad_diameter {
            painter.circle_filled(at, pad as f32 * view.view.scale / 2.0, ink.plated);
        }
        painter.circle_filled(at, hole.drill as f32 * view.view.scale / 2.0, ink.drill);
    }
}
