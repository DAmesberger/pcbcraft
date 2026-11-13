//! Differential pair routing in the Route tool. A click on a pad of a
//! pair (either half) starts both halves from that pad and its partner's
//! nearest pad; each further click is one pair step from the core's pure
//! [`layout::pair::step`] (the preview draws the same step): both tracks
//! coupled at the pair gap in the chosen posture, a 45° fan-out from the
//! pads (necked down where they are close), a via pair on a layer change,
//! and a fan-in when the click lands on a pad of the pair. Width and gap
//! follow the class on each layer (derived from its impedance target).
//!
//! A step is one `layout.route_apply` of the router's pair plan
//! (`router::pair_plan`: in Shove and Walkaround the halves push other
//! copper and walk round it as one unit); a step that continues a coupled
//! run extends the previous step's last tracks to the new mitre, and
//! Backspace gives them back with anything the step shoved. P switches
//! pair routing off for single nets.
use super::route_tool::{Commit, Planned};
use super::{PcbEditor, PlacedPad};
use sparksmith_core::layout::pair::{self, PairHead, PairStepRequest, PairVias};
use sparksmith_core::layout::{LayoutCommand, impedance};
use sparksmith_core::rules::signal::DiffPairId;
use sparksmith_core::{Id, Operation, Point, Project, Track, geometry};

/// The pair being routed.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PairRoute {
    pub(super) pair: DiffPairId,
    /// `[positive, negative]`.
    pub(super) nets: [Id; 2],
    pub(super) head: PairHead,
    /// The last `[positive, negative]` tracks while the halves run
    /// coupled; a continuation extends them.
    pub(super) last: Option<[Id; 2]>,
}

/// What Backspace restores after a pair step.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PairUndo {
    pub(super) head: PairHead,
    pub(super) last: Option<[Id; 2]>,
    /// The previous tracks as they were before the step extended them.
    pub(super) restore: Vec<Track>,
}

impl PcbEditor {
    /// The pad of `net` nearest `near`, preferring pads of `instance`.
    fn partner_pad(&self, net: Id, near: Point, instance: Id) -> Option<&PlacedPad> {
        self.board_pads
            .iter()
            .map(|b| &b.placed)
            .filter(|p| p.net == Some(net) && p.on_copper(self.layer))
            .min_by(|a, b| {
                let key =
                    |p: &PlacedPad| (p.instance != instance, geometry::distance(p.center(), near));
                let (ka, kb) = (key(a), key(b));
                ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
            })
    }

    /// The `[positive, negative]` pad centres for a click on a pad of the
    /// pair `nets` at `pad`.
    fn pair_pads(&self, nets: [Id; 2], pad: &PlacedPad) -> Option<[Point; 2]> {
        let net = pad.net?;
        let partner = if net == nets[0] { nets[1] } else { nets[0] };
        let other = self.partner_pad(partner, pad.center(), pad.instance)?;
        Some(if net == nets[0] {
            [pad.center(), other.center()]
        } else {
            [other.center(), pad.center()]
        })
    }

    /// Start a pair route when `at` is on a pad of a pair (and pair
    /// routing is on). Returns whether it started.
    pub(super) fn pair_start(&mut self, project: &Project, at: Point, scale: f32) -> bool {
        if self.route.single {
            return false;
        }
        let Some(pad) = self.route_snap(at, scale).cloned() else {
            return false;
        };
        let Some(pair) = pad.net.and_then(|n| project.rules.pair_of(n)).cloned() else {
            return false;
        };
        let nets = [pair.positive, pair.negative];
        let Some([p, n]) = self.pair_pads(nets, &pad) else {
            return false;
        };
        let head = PairHead::at(p, n);
        self.net = Some(pair.positive);
        self.route_highlight(self.net);
        self.route_start = Some(head.center);
        self.route.steps.clear();
        self.route.pair = Some(PairRoute {
            pair: pair.id,
            nets,
            head,
            last: None,
        });
        true
    }

    /// The pair step to `to` (a centre-line target), finishing on the
    /// pads in `finish`.
    fn pair_request(
        &self,
        project: &Project,
        to: Point,
        finish: Option<[Point; 2]>,
    ) -> Option<PairStepRequest> {
        let route = self.route.pair.as_ref()?;
        let positive = route.nets[0];
        let from_layer = self.pending_via.map_or(self.layer, |(_, from)| from);
        let mut style = pair::style(project, positive, from_layer);
        if route.head.coupled.is_none() {
            style.fan_width = pair::fan_width(project, positive, route.head.ends, style.width);
        }
        let last = route.last.and_then(|[p, n]| {
            let find = |id| project.tracks.iter().find(|t| t.id == id).cloned();
            Some([find(p)?, find(n)?])
        });
        let via = self.pending_via.map(|(at, _)| {
            let (drill, diameter) = self.route_via_size(project);
            let next = pair::style(project, positive, self.layer);
            PairVias {
                at,
                drill,
                diameter,
                tented: self.via_tented(project),
                pitch: pair::via_pitch(project, route.nets, diameter, next),
                layer: self.layer,
                style: next,
            }
        });
        Some(PairStepRequest {
            nets: route.nets,
            layer: from_layer,
            style,
            posture: self.route.posture,
            flipped: self.route.flipped,
            head: route.head,
            last,
            via,
            to,
            finish,
        })
    }

    /// The step the preview draws for the pointer at `end`.
    pub(super) fn pair_step(&self, project: &Project, end: Point) -> Option<PairStepRequest> {
        let (to, finish) = self.pair_target(end);
        self.pair_request(project, to, finish)
    }

    /// Where a click at `end` takes the centre line: onto a pad of the
    /// pair, the pads' midpoint (finishing there), else `end`.
    fn pair_target(&self, end: Point) -> (Point, Option<[Point; 2]>) {
        let Some(route) = &self.route.pair else {
            return (end, None);
        };
        let pad = self.board_pads.iter().map(|b| &b.placed).find(|p| {
            p.net.is_some_and(|n| route.nets.contains(&n))
                && geometry::distance(p.center(), end) < 1e-9
        });
        match pad.and_then(|p| self.pair_pads(route.nets, p)) {
            Some(pads) if pads != route.head.ends => {
                let mid = [
                    0.5 * (pads[0][0] + pads[1][0]),
                    0.5 * (pads[0][1] + pads[1][1]),
                ];
                (mid, Some(pads))
            }
            _ => (end, None),
        }
    }

    /// A Route click during a pair route: send the pair step there.
    pub(super) fn pair_click(
        &mut self,
        project: &Project,
        revision: u64,
        at: Point,
        scale: f32,
    ) -> Option<Operation> {
        let end = self
            .route_pad(project, at, scale)
            .map(PlacedPad::center)
            .unwrap_or_else(|| self.route_destination(project, at, scale));
        let (to, finish) = self.pair_target(end);
        let request = self.pair_request(project, to, finish)?;
        let planned = self.route_plan(project, revision, end)?;
        let Planned::Pair(plan) = planned.as_ref() else {
            return None;
        };
        let step = &plan.step;
        self.route.refused =
            super::route_tool::refusal(plan.reached, plan.blocked.as_deref(), &step.edits);
        if step.edits.add.is_empty() && step.edits.vias.is_empty() {
            return None;
        }
        let route = self.route.pair.as_ref()?;
        // The pair's own tracks it extends; shoved tracks go back through
        // the plan's undo.
        let restore = step
            .edits
            .replace
            .iter()
            .filter(|r| route.nets.contains(&r.net))
            .filter_map(|r| project.tracks.iter().find(|t| t.id == r.id).cloned())
            .collect();
        self.route.commit = Some(Commit::Step {
            revision,
            next_id: project.next_id,
            from: route.head.center,
            layer: request.layer,
            to: step.head.center,
            finish: finish.is_some() && plan.reached,
            via_placed: !step.edits.vias.is_empty(),
            unshove: Box::new(plan.undo.clone()),
            pair: Some(Box::new((
                PairUndo {
                    head: route.head,
                    last: route.last,
                    restore,
                },
                step.head,
            ))),
        });
        Some(Operation::Layout(LayoutCommand::ApplyRoute {
            edits: step.edits.clone(),
        }))
    }

    /// The engine took a pair step: move the head and remember the new
    /// last tracks (those of `tracks` ending at the head).
    pub(super) fn settle_pair(&mut self, project: &Project, tracks: &[Id], after: PairHead) {
        let Some(route) = &mut self.route.pair else {
            return;
        };
        let last_of = |net: Id, end: Point| {
            project
                .tracks
                .iter()
                .filter(|t| tracks.contains(&t.id))
                .find(|t| t.net == net && geometry::distance(t.end, end) < 1e-9)
                .map(|t| t.id)
        };
        route.last = after.coupled.and_then(|_| {
            Some([
                last_of(route.nets[0], after.ends[0])?,
                last_of(route.nets[1], after.ends[1])?,
            ])
        });
        route.head = after;
    }

    /// The pair's width and gap on the current layer, for the Route card.
    pub(super) fn pair_width_gap(&self, project: &Project) -> Option<(f64, f64)> {
        let route = self.route.pair.as_ref()?;
        Some(impedance::pair_width_gap(
            project,
            route.nets[0],
            self.layer,
        ))
    }
}
