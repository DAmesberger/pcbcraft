//! PCB 3D's adapter: picking on the model, and the frame that runs the
//! shared tool, drawing and in-place editing over the 3D view.
//!
//! What a click picks is what the pointer highlights, as Solidcraft
//! pre-highlights ([`crate::viewport::surface`]): an edge of a part or of
//! the board within Solidcraft's 6 points of the pointer, in front,
//! nearest first; else the face under it (the board's top or side near
//! its outline is that board edge). A model pick is named by the host
//! (`layout::dimensions::model::pick`) and traced once, so a sloped face or
//! one parallel to the board is refused in the tool card, in the trace's
//! words, before anything is committed. Hover reads the cached meshes
//! only; nothing here rebuilds geometry or traces a model but a pick.
use super::draw::Surface;
use super::state::{Picked, seat, words};
use crate::pcb_editor::{PcbEditor, Tool};
use crate::refusal;
use crate::scene::SceneController;
use crate::viewport::Viewport;
use crate::viewport::surface::{Hovered, SurfaceHit};
use egui::{Pos2, Ui};
use sparksmith_core::layout::dimensions::view;
use sparksmith_core::layout::dimensions::{FeatureGeometry, features, model};
#[cfg(test)]
use sparksmith_core::layout::outline::BoardShapeEvaluator;
use sparksmith_core::library::footprint_source::{SketchCurveId, SketchRef};
use sparksmith_core::model::board::ConstraintTarget;
use sparksmith_core::model::dimension::{BoardRef, FeatureRef, PadPoint};
use sparksmith_core::{Engine, Id, Layer, Operation, PadId, Project};

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

impl Surface for Viewport {
    fn to_screen(&self, p: [f64; 3]) -> Option<Pos2> {
        Viewport::to_screen(self, p)
    }
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

impl PcbEditor {
    /// Use `evaluator` as the board sketch engine (a counting one).
    #[cfg(test)]
    pub fn set_board_evaluator(&mut self, evaluator: &'static dyn BoardShapeEvaluator) {
        self.shape.evaluator = Some(evaluator);
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
        match target {
            Target::Model {
                instance,
                at,
                reach,
            } => self.pick_model_at(project, *instance, *at, *reach, None),
            Target::Pad { instance, pad } => self.pick_feature(
                project,
                FeatureRef::Pad {
                    placement: *instance,
                    pad: *pad,
                    at: PadPoint::Centre,
                },
            ),
            Target::Board { curve, geometry } => Ok(self.pick_board(project, curve, *geometry)),
        }
    }

    /// A model pick at `at`, a board point on `instance`'s 3D model (on
    /// `body` only, when given), named and traced by the host
    /// (`model::pick_on`): the one way both views make a face or edge
    /// reference.
    pub(super) fn pick_model_at(
        &self,
        project: &Project,
        instance: Id,
        at: [f64; 3],
        reach: f64,
        body: Option<&str>,
    ) -> Result<Picked, String> {
        let evaluator = self
            .shape
            .evaluator
            .ok_or("Dimensions need the sketch evaluator")?;
        let (feature, local) = model::pick_on(project, evaluator, instance, body, at, reach)
            .map_err(|e| refusal::shown_error(&e))?;
        let placement = project
            .placement_of(instance)
            .ok_or("The part is not on the board")?;
        let geometry = view::transformed(local, &view::part_transform(placement));
        let reference = BoardRef::Part(feature);
        Ok(Picked {
            z: seat(project, placement.layer),
            words: words(project, Some(evaluator), &reference, Some(&local)),
            reference,
            geometry,
        })
    }

    /// A footprint feature (a pad, the origin) as a pick.
    pub(super) fn pick_feature(
        &self,
        project: &Project,
        feature: FeatureRef,
    ) -> Result<Picked, String> {
        let local = features::geometry(project, &feature).map_err(|e| refusal::shown_error(&e))?;
        let ConstraintTarget::Placement(id) = feature.target() else {
            return Err("Pick a part's face, edge, hole or pad".into());
        };
        let placement = project
            .placement_of(id)
            .ok_or("The part is not on the board")?;
        let reference = BoardRef::Part(feature);
        Ok(Picked {
            geometry: view::transformed(local, &view::part_transform(placement)),
            z: seat(project, placement.layer),
            words: words(project, self.shape.evaluator, &reference, None),
            reference,
        })
    }

    /// A board curve as a pick.
    pub(super) fn pick_board(
        &self,
        project: &Project,
        curve: &SketchCurveId,
        geometry: FeatureGeometry,
    ) -> Picked {
        let reference = BoardRef::Board(SketchRef::Curve(curve.clone()));
        Picked {
            words: words(project, self.shape.evaluator, &reference, None),
            reference,
            geometry,
            z: seat(project, Layer::Top),
        }
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
        self.dimension_frame_start(ui, engine);
        let armed = self.tool == Tool::Dimension;
        let pointer = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p) && ui.rect_contains_pointer(rect));
        let over_label = pointer.is_some_and(|p| self.over_dimension_label(p));
        // The tool's hover: highlighted while nothing but the pointer moves.
        let moving = ui.input(|i| i.pointer.any_down());
        let hovered = if armed && !moving && !over_label {
            pointer.and_then(|p| self.target_at(project, scene, p).map(|(_, h)| h))
        } else {
            None
        };
        scene.viewport.set_hover(hovered);
        // Without the tool, the part or via under the pointer lights its
        // table row (one ray per pointer move).
        self.hover_3d(scene, pointer.filter(|_| !armed && !moving && !over_label));
        if armed && crate::keys::pressed(ui.ctx(), "pcb.dimension-cancel") {
            self.cancel_dimension();
        }
        let mut operation = None;
        if self.dimension.is_some() && crate::keys::pressed(ui.ctx(), "pcb.dimension-delete") {
            operation = self.delete_dimension();
        }
        // A click on a dimension selects it, and a double-click opens its
        // value (with the tool, a click on its label). Otherwise the tool
        // picks, or the click selects what is under it, as a Layout click.
        if let Some(pick) = pick {
            let at = scene.viewport.clicked_at();
            // A quick third click (after the part's) counts as well.
            let double = ui.input(|i| {
                let primary = egui::PointerButton::Primary;
                i.pointer.button_double_clicked(primary) || i.pointer.button_triple_clicked(primary)
            });
            let on = at.is_some_and(|p| self.click_drawn_dimension(project, p, double));
            if !on && armed {
                if let Some(at) = at {
                    self.begin_pick();
                    let found = self
                        .target_at(project, scene, at)
                        .map(|(target, _)| self.pick_target(project, &target));
                    self.take_pick(found);
                }
            } else if !on && ui.input(|i| i.modifiers.shift || i.modifiers.command) {
                // ⇧ or ⌘ adds the object, or takes it out, as in Layout.
                self.toggle_picked(pick.and_then(|id| scene.source(id)), project);
            } else if !on && scene.viewport.following() {
                let along = at.map_or_else(Vec::new, |p| scene.viewport.picks_at(p));
                let along: Vec<_> = along.iter().filter_map(|id| scene.source(*id)).collect();
                self.select_picked_following(pick.and_then(|id| scene.source(id)), &along);
            } else if !on {
                self.select_picked(pick.and_then(|id| scene.source(id)));
            }
        }
        let painter = ui.painter_at(rect);
        self.paint_datums(ui, &painter, &scene.viewport, engine, self.interface_shown);
        self.paint_dimensions(ui, &painter, &scene.viewport, project, true);
        let fields = self.dimension_fields(ui, engine);
        operation.or(fields)
    }

    /// PCB 3D while a newer board model is made: the last model stays up,
    /// and the dimensions are drawn from the document, so an edited value
    /// shows at once, at the part's new place. Nothing is picked.
    pub fn board_3d_updating(&mut self, ui: &mut Ui, scene: &SceneController, engine: &Engine) {
        self.sync(engine);
        self.answered(ui.ctx(), engine);
        if let Some(rect) = scene.viewport.rect() {
            let painter = ui.painter_at(rect);
            self.paint_datums(ui, &painter, &scene.viewport, engine, self.interface_shown);
            self.paint_dimensions(ui, &painter, &scene.viewport, engine.project(), false);
        }
    }
}
