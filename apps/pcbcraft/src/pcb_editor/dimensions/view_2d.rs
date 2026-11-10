//! Layout's adapter: what a part dimension can reference, picked in the
//! board plane, and the frame that runs the shared tool, drawing and
//! in-place editing over the Layout canvas.
//!
//! Every part dimension is a measurement in the board plane, so every
//! reference has a trace there (docs/board-sketch-references.md §20). The
//! parts' 3D bodies are drawn as their **body traces**: each face square
//! to the board as its line, each vertical hole or boss as its centre,
//! listed once per package revision by the host off the UI thread
//! (`model::body_faces`) and moved with the placement as it is drawn, so
//! selection and the view never list them again. They show while the
//! Dimension tool is on, and for the hovered or selected part.
//!
//! With the tool, the pointer finds, nearest first within Solidcraft's 6
//! points: a part's origin, a body trace, a courtyard side, a pad (inside
//! it: its centre), a board edge. Hovering names it as the 3D view and
//! the card do ("J1 · front face"). A body trace is picked as the 3D view
//! picks that face: by the host, at the face's own point, on its body
//! (`model::pick_on`), so it makes the same reference. When several
//! parallel traces lie under the click (a shell's back face and the
//! insulator's in the same plane), a small chooser lists them by name and
//! body; the value is measured from the one chosen.
use super::draw::{Surface, trace_reference};
use super::state::{Picked, capital, seg_dist, words, words_with_body};
use super::view_3d::EDGE_REACH_PT;
use crate::kit;
use crate::pcb_editor::{BoardView, PcbEditor, Tool};
use crate::refusal;
use egui::{Color32, FontId, Pos2, Rect, Stroke, Ui, vec2};
use pcbcraft_core::ids::PackageRef;
use pcbcraft_core::layout::dimensions::model::{self, BodyFace};
use pcbcraft_core::layout::dimensions::{FeatureGeometry, features, view};
use pcbcraft_core::layout::outline::BoardShapeEvaluator;
use pcbcraft_core::library::footprint_source::SketchCurveId;
use pcbcraft_core::model::board::ConstraintTarget;
use pcbcraft_core::model::dimension::{BoardRef, BoxSide, FeatureRef, PadPoint};
use pcbcraft_core::{Engine, Id, Operation, Project};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::ui_kit::menu::menu_frame;
use solidcraft_ui_egui::ui_kit::{Container, Surface as Panel};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

/// Two traces count as one line on screen when they run within this
/// angle (its sine) of each other.
const PARALLEL: f64 = 0.017;

impl Surface for BoardView {
    fn to_screen(&self, p: [f64; 3]) -> Option<Pos2> {
        Some(self.screen([p[0], p[1]]))
    }
}

/// A package revision's body faces, or why it has none.
type Listed = Result<Arc<Vec<BodyFace>>, String>;

/// Lists body faces off the UI thread, one placement at a time.
struct Worker {
    jobs: Sender<(Arc<Project>, Id)>,
    done: Receiver<(Option<PackageRef>, Id, Listed)>,
}

impl Worker {
    fn start(ctx: egui::Context, evaluator: &'static dyn BoardShapeEvaluator) -> Option<Self> {
        let (jobs, inbox) = mpsc::channel::<(Arc<Project>, Id)>();
        let (outbox, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("pcbcraft-body-traces".into())
            .spawn(move || {
                while let Ok((project, placement)) = inbox.recv() {
                    let package = model::package_of(&project, placement);
                    let listed = model::body_faces(&project, evaluator, placement)
                        .map(|b| Arc::new(b.faces))
                        .map_err(|e| refusal::shown_error(&e));
                    if outbox.send((package, placement, listed)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .ok()?;
        Some(Self { jobs, done })
    }
}

/// One option of the chooser: a face under the click.
#[derive(Clone, Debug)]
pub(super) struct Choice {
    instance: Id,
    face: BodyFace,
    /// "J1 · back face (Insulator)".
    words: String,
}

/// Faces stacked under one click, offered by name.
#[derive(Clone, Debug)]
pub(super) struct Chooser {
    at: Pos2,
    choices: Vec<Choice>,
    /// Each choice's row on screen last frame.
    rows: Vec<Rect>,
}

/// Layout's own state for part dimensions.
#[derive(Default)]
pub(in crate::pcb_editor) struct Layout2d {
    /// Body faces by package revision: listed, or being listed (`None`).
    bodies: Vec<(PackageRef, Option<Listed>)>,
    worker: Option<Worker>,
    /// The chooser open over stacked traces.
    pub(super) choosing: Option<Chooser>,
    /// The canvas mapping drawn last frame (for tests to aim at the board).
    #[cfg_attr(not(test), allow(dead_code))]
    view: Option<BoardView>,
}

impl Layout2d {
    fn listed(&self, package: PackageRef) -> Option<&Option<Listed>> {
        self.bodies
            .iter()
            .find(|(p, _)| *p == package)
            .map(|(_, l)| l)
    }

    /// Take what the worker has listed.
    fn poll(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        while let Ok((package, _, listed)) = worker.done.try_recv() {
            let Some(package) = package else {
                continue;
            };
            match self.bodies.iter_mut().find(|(p, _)| *p == package) {
                Some(slot) => slot.1 = Some(listed),
                None => self.bodies.push((package, Some(listed))),
            }
        }
    }
}

/// A body face under the pointer: its part, the face and its trace in
/// the board, and how far it is (points).
#[derive(Clone, Debug)]
struct FaceHit {
    instance: Id,
    face: BodyFace,
    board: FeatureGeometry,
    distance: f32,
}

/// What a click with the tool would pick in Layout.
#[derive(Clone, Debug)]
enum Found {
    /// Body faces, nearest first; more than one when they lie on one line
    /// at this zoom.
    Faces(Vec<FaceHit>),
    /// An origin, pad centre or courtyard side.
    Feature(FeatureRef, FeatureGeometry),
    Board(SketchCurveId, FeatureGeometry),
}

/// How far `p` is from `g` on screen (points): a line or arc along it, a
/// point or circle at its centre.
fn screen_distance(view: &BoardView, g: &FeatureGeometry, p: Pos2) -> f32 {
    match *g {
        FeatureGeometry::Point { at } | FeatureGeometry::Circle { center: at, .. } => {
            view.screen(at).distance(p)
        }
        FeatureGeometry::Line { a, b } => seg_dist(p, view.screen(a), view.screen(b)),
        FeatureGeometry::Arc { a, b, bulge } => super::draw::arc_points(a, b, bulge)
            .windows(2)
            .map(|w| seg_dist(p, view.screen(w[0]), view.screen(w[1])))
            .fold(f32::INFINITY, f32::min),
    }
}

/// The unit direction of a line, or `None` for anything else.
fn direction(g: &FeatureGeometry) -> Option<[f64; 2]> {
    let FeatureGeometry::Line { a, b } = *g else {
        return None;
    };
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    (len > 1e-9).then(|| [d[0] / len, d[1] / len])
}

/// Whether two traces read as one under the pointer: two lines running
/// the same way, or two points.
fn stacked(a: &FeatureGeometry, b: &FeatureGeometry) -> bool {
    match (direction(a), direction(b)) {
        (Some(u), Some(v)) => (u[0] * v[1] - u[1] * v[0]).abs() <= PARALLEL,
        (None, None) => matches!(
            (a, b),
            (FeatureGeometry::Point { .. }, FeatureGeometry::Point { .. })
        ),
        _ => false,
    }
}

/// The faint colour of body traces: apart from pads, silkscreen and the
/// courtyard.
fn body_ink() -> Color32 {
    Tokens::get().surface_accent.gamma_multiply(0.6)
}

impl PcbEditor {
    /// The body faces of `instance`'s package revision, once listed.
    fn faces_of(&self, project: &Project, instance: Id) -> Option<Arc<Vec<BodyFace>>> {
        let package = model::package_of(project, instance)?;
        match self.dims.layout.listed(package)? {
            Some(Ok(faces)) => Some(Arc::clone(faces)),
            _ => None,
        }
    }

    /// Ask the worker for the body faces of `parts` not yet listed.
    fn request_bodies(&mut self, ctx: &egui::Context, engine: &Engine, parts: &[Id]) {
        let Some(evaluator) = self.shape.evaluator else {
            return;
        };
        let project = engine.project();
        for &part in parts {
            let Some(package) = model::package_of(project, part) else {
                continue;
            };
            if self.dims.layout.listed(package).is_some() {
                continue;
            }
            if self.dims.layout.worker.is_none() {
                self.dims.layout.worker = Worker::start(ctx.clone(), evaluator);
            }
            let Some(worker) = &self.dims.layout.worker else {
                return;
            };
            if worker
                .jobs
                .send((Arc::clone(engine.project_arc()), part))
                .is_ok()
            {
                self.dims.layout.bodies.push((package, None));
            }
        }
    }

    /// Body faces still being listed (for tests: frames run until none).
    #[cfg(test)]
    pub fn body_traces_pending(&self) -> usize {
        self.dims
            .layout
            .bodies
            .iter()
            .filter(|(_, l)| l.is_none())
            .count()
    }

    /// Where a board point was on the Layout canvas last frame.
    #[cfg(test)]
    pub fn layout_to_screen(&self, p: [f64; 2]) -> Option<Pos2> {
        self.dims.layout.view.map(|v| v.screen(p))
    }

    /// The chooser's rows: each face's words and its row on screen.
    #[cfg(test)]
    pub fn dimension_chooser(&self) -> Option<Vec<(String, Rect)>> {
        self.dims.layout.choosing.as_ref().map(|c| {
            c.choices
                .iter()
                .map(|o| o.words.clone())
                .zip(c.rows.iter().copied())
                .collect()
        })
    }

    /// Body faces of `instance` in the board, with the face.
    fn board_faces(&self, project: &Project, instance: Id) -> Vec<(BodyFace, FeatureGeometry)> {
        let (Some(faces), Some(placement)) = (
            self.faces_of(project, instance),
            project.placement_of(instance),
        ) else {
            return Vec::new();
        };
        let mut placed = placement.clone();
        let delta = self.drag_delta(instance);
        placed.position = [placed.position[0] + delta[0], placed.position[1] + delta[1]];
        let to_board = view::part_transform(&placed);
        faces
            .iter()
            .map(|f| (f.clone(), view::transformed(f.trace, &to_board)))
            .collect()
    }

    /// What a click at `p` (screen) would pick with the tool.
    fn found_at(&self, project: &Project, view: &BoardView, p: Pos2) -> Option<Found> {
        let reach = EDGE_REACH_PT;
        let nearest = |items: Vec<(f32, Found)>| {
            items
                .into_iter()
                .filter(|(d, _)| *d <= reach)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, f)| f)
        };
        // An origin: a point, the smallest target, first.
        let origins = project
            .placements
            .iter()
            .map(|pl| {
                let at = pl.position;
                let feature = FeatureRef::Origin {
                    target: ConstraintTarget::Placement(pl.instance),
                };
                (
                    view.screen(at).distance(p),
                    Found::Feature(feature, FeatureGeometry::Point { at }),
                )
            })
            .collect();
        if let Some(found) = nearest(origins) {
            return Some(found);
        }
        // Body traces, and those stacked with the nearest.
        let mut hits: Vec<FaceHit> = project
            .placements
            .iter()
            .flat_map(|pl| {
                self.board_faces(project, pl.instance)
                    .into_iter()
                    .map(move |(face, board)| (pl.instance, face, board))
            })
            .filter_map(|(instance, face, board)| {
                let distance = screen_distance(view, &board, p);
                (distance <= reach).then_some(FaceHit {
                    instance,
                    face,
                    board,
                    distance,
                })
            })
            .collect();
        hits.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        if let Some(first) = hits.first().cloned() {
            hits.retain(|h| stacked(&first.board, &h.board));
            return Some(Found::Faces(hits));
        }
        // Courtyard sides.
        let sides = project
            .placements
            .iter()
            .flat_map(|pl| {
                [BoxSide::Left, BoxSide::Right, BoxSide::Bottom, BoxSide::Top]
                    .into_iter()
                    .filter_map(move |side| {
                        let feature = FeatureRef::Courtyard {
                            placement: pl.instance,
                            side: Some(side),
                        };
                        let local = features::geometry(project, &feature).ok()?;
                        let board = view::transformed(local, &view::part_transform(pl));
                        Some((
                            screen_distance(view, &board, p),
                            Found::Feature(feature, board),
                        ))
                    })
            })
            .collect();
        if let Some(found) = nearest(sides) {
            return Some(found);
        }
        // A pad, from inside it: its centre.
        let at = view.world(p);
        let layers = self.seen();
        if let Some(pad) = self.pick_pad(at, |pad| layers.pad(pad)) {
            let feature = FeatureRef::Pad {
                placement: pad.instance,
                pad: pad.pad.id,
                at: PadPoint::Centre,
            };
            let centre = FeatureGeometry::Point { at: pad.center() };
            return Some(Found::Feature(feature, centre));
        }
        // A board edge.
        let mm = f64::from(reach / view.view.scale.max(1e-6));
        view::board_curve_at(project, self.shape.evaluator, at, mm)
            .map(|(curve, geometry)| Found::Board(curve, geometry))
    }

    /// A body face picked as the 3D view picks it: by the host, at the
    /// face's own point, on its body.
    fn pick_face(
        &self,
        project: &Project,
        instance: Id,
        face: &BodyFace,
        named_by_body: bool,
    ) -> Result<Picked, String> {
        let placement = project
            .placement_of(instance)
            .ok_or("The part is not on the board")?;
        let at = model::board_point(project, placement, face.at);
        let mut picked = self.pick_model_at(project, instance, at, 0.0, Some(&face.face.body.0))?;
        if named_by_body {
            picked.words = words_with_body(
                project,
                self.shape.evaluator,
                &picked.reference,
                Some(&face.trace),
            );
        }
        Ok(picked)
    }

    /// The words a found thing reads in, for its hover tag.
    fn found_words(&self, project: &Project, found: &Found) -> String {
        let evaluator = self.shape.evaluator;
        match found {
            Found::Faces(hits) => {
                let Some(first) = hits.first() else {
                    return String::new();
                };
                let r = BoardRef::Part(FeatureRef::Face {
                    placement: first.instance,
                    face: first.face.face.clone(),
                });
                let one = words(project, evaluator, &r, Some(&first.face.trace));
                match hits.len() {
                    1 => one,
                    n => format!("{one} and {} more: click to choose", n - 1),
                }
            }
            Found::Feature(f, _) => words(project, evaluator, &BoardRef::Part(f.clone()), None),
            Found::Board(curve, _) => words(
                project,
                evaluator,
                &BoardRef::Board(pcbcraft_core::library::footprint_source::SketchRef::Curve(
                    curve.clone(),
                )),
                None,
            ),
        }
    }

    /// The start of a Layout frame: refusals of the values sent last frame.
    pub(in crate::pcb_editor) fn layout_dimension_start(&mut self, ui: &Ui, engine: &Engine) {
        self.dimension_frame_start(ui, engine);
    }

    /// Esc in Layout with the Dimension tool on (`pcb.cancel`): the chooser
    /// closes, else the picks drop, else the tool ends.
    pub(in crate::pcb_editor) fn layout_dimension_cancel(&mut self) {
        self.cancel_dimension();
    }

    /// Delete in Layout (`pcb.delete`) with a dimension selected: its
    /// `layout.dimension_delete`, one undo step.
    pub(in crate::pcb_editor) fn layout_dimension_delete(&mut self) -> Option<Operation> {
        self.delete_dimension()
    }

    /// A primary click on the Layout canvas at `at` (screen), with the
    /// Dimension tool on or on a drawn dimension. `true` when it was taken.
    pub(in crate::pcb_editor) fn layout_dimension_click(
        &mut self,
        ui: &Ui,
        project: &Project,
        view: &BoardView,
        at: Pos2,
    ) -> bool {
        let armed = self.tool == Tool::Dimension;
        if self.dims.layout.choosing.take().is_some() {
            // A click away from the chooser closes it.
            return true;
        }
        let double = ui.input(|i| {
            let primary = egui::PointerButton::Primary;
            i.pointer.button_double_clicked(primary) || i.pointer.button_triple_clicked(primary)
        });
        if self.click_drawn_dimension(project, at, double) {
            return true;
        }
        if !armed {
            return false;
        }
        self.begin_pick();
        let found = self.found_at(project, view, at);
        let picked = match found {
            None => None,
            Some(Found::Faces(hits)) if hits.len() > 1 => {
                let evaluator = self.shape.evaluator;
                let choices = hits
                    .into_iter()
                    .map(|h| {
                        let r = BoardRef::Part(FeatureRef::Face {
                            placement: h.instance,
                            face: h.face.face.clone(),
                        });
                        Choice {
                            words: words_with_body(project, evaluator, &r, Some(&h.face.trace)),
                            instance: h.instance,
                            face: h.face,
                        }
                    })
                    .collect();
                self.dims.error = None;
                self.dims.layout.choosing = Some(Chooser {
                    at,
                    choices,
                    rows: Vec::new(),
                });
                return true;
            }
            Some(Found::Faces(hits)) => hits
                .first()
                .map(|h| self.pick_face(project, h.instance, &h.face, false)),
            Some(Found::Feature(f, _)) => Some(self.pick_feature(project, f)),
            Some(Found::Board(curve, geometry)) => {
                Some(Ok(self.pick_board(project, &curve, geometry)))
            }
        };
        self.take_pick(picked);
        true
    }

    /// Part dimensions on the Layout canvas, after the board is drawn: the
    /// body traces, the tool's hover and its name, the dimensions, the
    /// chooser and the value fields. `hovered_part` is the part under the
    /// pointer; `pointer` is where it is, when it is over the canvas.
    pub(in crate::pcb_editor) fn layout_dimensions(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        view: &BoardView,
        engine: &Engine,
        pointer: Option<Pos2>,
        hovered_part: Option<Id>,
    ) -> Option<Operation> {
        let project = engine.project();
        let t = Tokens::get();
        let armed = self.tool == Tool::Dimension;
        self.dims.layout.poll();
        self.dims.layout.view = Some(*view);
        // The parts whose body traces show.
        let parts: Vec<Id> = if armed {
            project.placements.iter().map(|p| p.instance).collect()
        } else {
            hovered_part
                .into_iter()
                .chain(self.instance)
                .chain(self.also.iter().copied())
                .collect()
        };
        self.request_bodies(ui.ctx(), engine, &parts);
        let ink = Stroke::new(1.0, body_ink());
        for &part in &parts {
            for (_, board) in self.board_faces(project, part) {
                match board {
                    FeatureGeometry::Point { at } => {
                        painter.circle_stroke(view.screen(at), 2.5, ink);
                    }
                    ref g => trace_reference(painter, view, g, 0.0, ink),
                }
            }
        }
        // The tool's hover: the trace or feature a click picks, named.
        let choosing = self.dims.layout.choosing.is_some();
        let free = pointer.filter(|p| armed && !choosing && !self.over_dimension_label(*p));
        if let Some(p) = free
            && let Some(found) = self.found_at(project, view, p)
        {
            let hover = Stroke::new(2.0, t.hover_edge);
            match &found {
                Found::Faces(hits) => {
                    for h in hits {
                        trace_reference(painter, view, &h.board, 0.0, hover);
                    }
                }
                Found::Feature(_, g) | Found::Board(_, g) => {
                    trace_reference(painter, view, g, 0.0, hover);
                }
            }
            let text = capital(&self.found_words(project, &found));
            let galley =
                painter.layout_no_wrap(text, FontId::proportional(t.font_small), t.hover_edge);
            let at = p + vec2(14.0, 14.0);
            let back = Rect::from_min_size(at, galley.size() + vec2(t.space_8, t.space_4));
            painter.rect_filled(back, t.radius_sm, t.panel);
            painter.galley(at + vec2(t.space_4, t.space_2), galley, t.hover_edge);
        }
        self.paint_dimensions(ui, painter, view, project, true);
        self.chooser(ui, project);
        self.dimension_fields(ui, engine)
    }

    /// The chooser over stacked traces: one row per face, by name and
    /// body, with how high it reaches; a row picks it.
    fn chooser(&mut self, ui: &mut Ui, project: &Project) {
        let Some(chooser) = self.dims.layout.choosing.clone() else {
            return;
        };
        let t = Tokens::get();
        let mut chosen = None;
        let mut rows = Vec::new();
        let ctx = ui.ctx().clone();
        Container::new(&ctx, Panel::Popup)
            .area(egui::Id::unique("pcb-dimension-chooser"))
            .fixed_pos(chooser.at + vec2(8.0, 8.0))
            .constrain(true)
            .order(egui::Order::Foreground)
            .show(&ctx, |ui| {
                menu_frame(ui, |ui| {
                    kit::note(ui, "Faces here: pick one");
                    for (i, choice) in chooser.choices.iter().enumerate() {
                        let detail = format!(
                            "{}–{} mm high",
                            kit::number_text(super::state::round_um(choice.face.z[0])),
                            kit::number_text(super::state::round_um(choice.face.z[1]))
                        );
                        // The face on its line and how high it reaches
                        // under it, dim: in the menu's width a row on
                        // one line wraps mid-phrase.
                        let mut job = egui::text::LayoutJob::default();
                        job.append(
                            &capital(&choice.words),
                            0.0,
                            egui::TextFormat::simple(
                                FontId::proportional(t.font_label),
                                t.form_ink,
                            ),
                        );
                        job.append(
                            &format!("\n{detail}"),
                            0.0,
                            egui::TextFormat::simple(
                                FontId::proportional(t.font_small),
                                t.form_dim,
                            ),
                        );
                        let response = ui.add(
                            egui::Button::new(job)
                                .frame(false)
                                .min_size(vec2(0.0, t.control_height)),
                        );
                        rows.push(response.rect);
                        if response.clicked() {
                            chosen = Some(i);
                        }
                    }
                });
            });
        if let Some(c) = &mut self.dims.layout.choosing {
            c.rows = rows;
        }
        if let Some(i) = chosen
            && let Some(choice) = chooser.choices.get(i)
        {
            self.dims.layout.choosing = None;
            let picked = self.pick_face(project, choice.instance, &choice.face, true);
            self.take_pick(Some(picked));
        }
    }
}
