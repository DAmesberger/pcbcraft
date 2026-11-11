//! Dragging a track with Select: the segment under the press moves along
//! its normal with the pointer, its neighbours on the net follow, and
//! other nets' copper is shoved live ([`router::drag::drag_plan`], in the
//! router mode the Route tool's M chooses); a pair half's partner beside
//! it moves too, drawn as dragged copper. The preview is drawn as the
//! route preview is, from the committed board on every move, so shoved
//! copper springs back; a drag that stops short shows the warning ring.
//! Release sends the plan's edits as one `layout.route_apply` (one undo
//! step with every shoved item); Esc cancels.
use super::{BoardView, PcbEditor};
use pcbcraft_core::{
    Id, Operation, Point, Project,
    geometry::distance,
    layout::{
        LayoutCommand,
        router::{
            RoutePlan,
            drag::{MAX_DRAG_MM, TrackDragRequest, drag_plan},
        },
    },
};
use std::rc::Rc;

/// Offsets are planned in steps of this, mm, so a pointer moving by less
/// than a step reuses the plan.
const STEP_MM: f64 = 0.01;

/// A track drag in progress.
#[derive(Clone, Debug)]
pub(super) struct TrackDrag {
    pub(super) track: Id,
    /// Where the press was, and the track's unit left normal.
    start: Point,
    normal: Point,
    /// The last plan, with the revision and request it was made for.
    pub(super) planned: Option<(u64, TrackDragRequest, Rc<RoutePlan>)>,
}

impl TrackDrag {
    /// A drag of `track` from `start`; `None` for a track too short to
    /// have a direction.
    pub(super) fn of(project: &Project, track: Id, start: Point) -> Option<Self> {
        let t = project.tracks.iter().find(|t| t.id == track)?;
        let length = distance(t.start, t.end);
        (length > 1e-9).then(|| Self {
            track,
            start,
            normal: [
                -(t.end[1] - t.start[1]) / length,
                (t.end[0] - t.start[0]) / length,
            ],
            planned: None,
        })
    }

    /// The pointer's offset along the normal, in plan steps.
    fn offset(&self, at: Point) -> f64 {
        let along =
            (at[0] - self.start[0]) * self.normal[0] + (at[1] - self.start[1]) * self.normal[1];
        ((along / STEP_MM).round() * STEP_MM).clamp(-MAX_DRAG_MM, MAX_DRAG_MM)
    }
}

impl PcbEditor {
    /// Plan the drag to the pointer `at` (once per revision and offset).
    pub(super) fn plan_track_drag(&mut self, project: &Project, revision: u64, at: Point) {
        let Some(drag) = &self.track_drag else {
            return;
        };
        let request = TrackDragRequest {
            track: drag.track,
            offset: drag.offset(at),
            posture: self.route.posture,
            mode: self.route.mode,
            move_vias: !self.route.vias_stay,
        };
        if drag
            .planned
            .as_ref()
            .is_some_and(|(r, q, _)| *r == revision && *q == request)
        {
            return;
        }
        let index = self.route_index(project, revision);
        let plan = Rc::new(drag_plan(project, &index, &request));
        if let Some(drag) = &mut self.track_drag {
            drag.planned = Some((revision, request, plan));
        }
    }

    /// The tracks and vias the planned drag moves (the board draws them at
    /// their new places only).
    pub(super) fn track_drag_moved(&self, revision: u64) -> Option<(Vec<Id>, Vec<Id>)> {
        let (at, _, plan) = self.track_drag.as_ref()?.planned.as_ref()?;
        (*at == revision).then(|| (plan.shoved.clone(), plan.moved_vias.clone()))
    }

    /// Release: the planned edits as one `layout.route_apply`, or nothing
    /// when the drag moved nothing; then a toast says why, as the Route
    /// tool's refused click does.
    pub(super) fn finish_track_drag(
        &mut self,
        ctx: &egui::Context,
        revision: u64,
    ) -> Option<Operation> {
        let drag = self.track_drag.take()?;
        let (at, _, plan) = drag.planned?;
        if at != revision {
            return None;
        }
        if plan.edits.replace.is_empty() {
            if let Some(why) = &plan.blocked {
                crate::kit::toast::post(
                    ctx,
                    crate::kit::severity::Level::Warning,
                    format!("Can't drag the track: {why}"),
                    Vec::new(),
                );
            }
            return None;
        }
        Some(Operation::Layout(LayoutCommand::ApplyRoute {
            edits: plan.edits.clone(),
        }))
    }

    /// The planned drag's copper, drawn as the route preview draws a step.
    pub(super) fn paint_track_drag(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        project: &Project,
        pointer: Point,
    ) {
        let Some((_, _, plan)) = self.track_drag.as_ref().and_then(|d| d.planned.as_ref()) else {
            return;
        };
        let own = project
            .tracks
            .iter()
            .find(|t| Some(t.id) == self.track_drag.as_ref().map(|d| d.track))
            .map(|t| t.net);
        // A pair's halves move together: both are drawn as the dragged copper.
        let partner = own.and_then(|net| project.rules.pair_of(net)?.partner(net));
        self.paint_plan(
            painter,
            view,
            project,
            super::route_tool::PlanLook {
                edits: &plan.edits,
                own: &|net| Some(net) == own || Some(net) == partner,
                own_as_new: true,
                layer: plan.end_layer,
                reached: plan.reached,
                end: plan.end,
                pointer,
                target_ring: false,
                note: plan.note.as_deref(),
            },
        );
    }
}
