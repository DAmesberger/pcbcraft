//! The Route tool's interactive route: continuous steps from a head that
//! follows each accepted step, postures, widths and via sizes from the
//! rules, layer changes through a via, and Backspace unwinding.
//!
//! Each click is one gesture and one undo step: the step's elbow (and via,
//! after a layer change) goes to the engine as one `layout.route_apply`.
//! The route continues from the step's end once the engine has taken it
//! (the next revision is exactly that step); a refusal leaves the route as
//! it was. Backspace sends one `layout.route_apply` removing exactly the
//! last step's tracks and via, and moves the head back. Any other document
//! change ends the route. The elbow geometry comes from
//! [`layout::posture`], which the preview and the commit share.
use super::{BoardInk, BoardView, PcbEditor, Tool, capsule, pad_on};
use egui::{Shape, Stroke};
use pcbcraft_core::{
    CopperLayer, Id, Operation, Point, Project,
    layout::{
        self, LayoutCommand, RouteEditSet, ViaRequest,
        posture::{self, Posture, StepRequest},
    },
};
use std::cmp::Ordering;

/// The Route tool's keys while the canvas is hovered: (key, what it does).
/// The Route card lists them; a command palette can list them too.
pub const ROUTE_KEYS: &[(&str, &str)] = &[
    ("Click", "Corner; a pad or track of the net finishes"),
    ("Enter", "Finish the route (or double-click)"),
    ("Esc", "End the route (or right-click); again: Select"),
    ("Backspace", "Remove the last segment"),
    ("V, Shift+V", "Via to the next or previous layer"),
    ("1 to 9", "Via to that layer of the stackup"),
    ("/", "Next mode: 45°, 90°, free"),
    ("F", "Flip the elbow corner"),
    ("W, Shift+W", "Next or previous track width"),
    ("D, Shift+D", "Next or previous via size"),
    (
        "P",
        "Pair routing on or off (a pad of a pair starts both halves)",
    ),
];

/// The route in progress besides its head (`PcbEditor::route_start`) and
/// pending via, and the routing settings that outlive one route.
#[derive(Default)]
pub(super) struct RouteState {
    pub(super) posture: Posture,
    /// The elbow's corner flipped (`F`).
    pub(super) flipped: bool,
    /// The accepted steps of the route in progress, oldest first.
    pub(super) steps: Vec<Step>,
    /// A step or unwind sent to the engine, settled by the next sync.
    pub(super) commit: Option<Commit>,
    /// The via size chosen for a net while routing; the net class's
    /// otherwise.
    pub(super) via_size: Option<(Id, (f64, f64))>,
    /// A via size typed in the card that misses the rules: shown and
    /// marked, never used.
    pub(super) via_draft: Option<(f64, f64)>,
    /// Every key row in the card instead of the first eight.
    pub(super) keys_all: bool,
    /// Pair routing switched off (P): a pad of a pair routes one net.
    pub(super) single: bool,
    /// The differential pair being routed (`pair_tool`).
    pub(super) pair: Option<super::pair_tool::PairRoute>,
}

/// One accepted step: where it started and what it added.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Step {
    pub(super) from: Point,
    pub(super) layer: CopperLayer,
    pub(super) tracks: Vec<Id>,
    pub(super) vias: Vec<Id>,
    /// A pair step: the pair head before it and the tracks it extended.
    pub(super) pair: Option<super::pair_tool::PairUndo>,
}

pub(super) enum Commit {
    Step {
        revision: u64,
        /// The first identity the step can allocate.
        next_id: Id,
        from: Point,
        layer: CopperLayer,
        to: Point,
        finish: bool,
        /// A pair step: the head before and after it.
        pair: Option<Box<(super::pair_tool::PairUndo, layout::pair::PairHead)>>,
    },
    Unwind {
        revision: u64,
    },
}

/// The preset after (or, `back`, before) `current` in ascending `presets`,
/// wrapping; `None` without presets.
pub(super) fn cycle<T: Copy>(
    presets: &[T],
    current: T,
    back: bool,
    order: impl Fn(&T, &T) -> Ordering,
) -> Option<T> {
    if back {
        presets
            .iter()
            .rev()
            .find(|p| order(p, &current) == Ordering::Less)
            .or(presets.last())
            .copied()
    } else {
        presets
            .iter()
            .find(|p| order(p, &current) == Ordering::Greater)
            .or(presets.first())
            .copied()
    }
}

fn by_pad(a: &(f64, f64), b: &(f64, f64)) -> Ordering {
    let close = |x: f64, y: f64| (x - y).abs() < 1e-9;
    if close(a.1, b.1) && close(a.0, b.0) {
        Ordering::Equal
    } else {
        a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0))
    }
}

fn by_value(a: &f64, b: &f64) -> Ordering {
    if (a - b).abs() < 1e-9 {
        Ordering::Equal
    } else {
        a.total_cmp(b)
    }
}

impl PcbEditor {
    /// Drop the route in progress: its head, pending via and steps. The
    /// tool, net and settings stay.
    pub(super) fn reset_route(&mut self) {
        self.route_start = None;
        self.pending_via = None;
        self.route.steps.clear();
        self.route.commit = None;
        self.route.pair = None;
    }

    /// Finish the route where it is (Enter, double-click, Esc, right-click,
    /// or a step onto the net's copper): its accepted steps stay, a via
    /// still pending is dropped, and Route stays armed.
    pub(super) fn finish_route(&mut self) {
        if let Some((_, from)) = self.pending_via {
            self.layer = from;
        }
        self.reset_route();
    }

    /// Settle a step or unwind sent last frame. Returns whether the
    /// project's current revision is exactly that change, so the route
    /// continues through it; a refused change (the same revision) leaves
    /// the route as it was.
    pub(super) fn settle_route(&mut self, project: &Project, revision: u64) -> bool {
        let Some(commit) = self.route.commit.take() else {
            return false;
        };
        let sent = match commit {
            Commit::Step { revision, .. } | Commit::Unwind { revision } => revision,
        };
        if revision == sent || sent.checked_add(1) != Some(revision) {
            return false;
        }
        match commit {
            Commit::Step {
                next_id,
                from,
                layer,
                to,
                finish,
                pair,
                ..
            } => {
                let tracks: Vec<Id> = project
                    .tracks
                    .iter()
                    .filter(|t| t.id >= next_id)
                    .map(|t| t.id)
                    .collect();
                let vias = project
                    .vias
                    .iter()
                    .filter(|v| v.id >= next_id)
                    .map(|v| v.id)
                    .collect();
                let undo = pair.map(|boxed| {
                    let (undo, after) = *boxed;
                    self.settle_pair(project, &tracks, after);
                    undo
                });
                self.route.steps.push(Step {
                    from,
                    layer,
                    tracks,
                    vias,
                    pair: undo,
                });
                self.pending_via = None;
                self.route_start = Some(to);
                if finish {
                    self.reset_route();
                }
            }
            Commit::Unwind { .. } => {
                if let Some(step) = self.route.steps.pop() {
                    self.route_start = Some(step.from);
                    self.layer = step.layer;
                    self.pending_via = None;
                    if let (Some(undo), Some(route)) = (step.pair, &mut self.route.pair) {
                        route.head = undo.head;
                        route.last = undo.last;
                    }
                }
            }
        }
        true
    }

    /// The live preview of the step to `end`: the elbow's copper at the
    /// step's width on each layer, the pending via and the target ring.
    /// The same [`posture::step`] the click sends.
    pub(super) fn paint_route_preview(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        project: &Project,
        end: Point,
    ) {
        let edits = if self.route.pair.is_some() {
            match self.pair_step(project, end) {
                Some(request) => layout::pair::step(&request).edits,
                None => return,
            }
        } else {
            match self.route_step(project, end) {
                Some(request) => posture::step(&request),
                None => return,
            }
        };
        let t = solidcraft_ui_egui::theme::Tokens::get();
        let scale = view.view.scale;
        let screen = |p: Point| view.screen(p);
        for leg in &edits.add {
            // The copper the step will add: round caps, opaque.
            let width = (leg.width as f32 * scale).max(1.0);
            let color =
                BoardInk::get().on_board(crate::palette::board().copper_layer(leg.layer), 1.0);
            let (a, b) = (screen(leg.start), screen(leg.end));
            painter.add(Shape::convex_polygon(
                capsule(a, b, width / 2.0),
                color,
                Stroke::NONE,
            ));
            painter.line_segment([a, b], Stroke::new(1.0, t.body_selected));
        }
        for track in &edits.replace {
            let (a, b) = (screen(track.start), screen(track.end));
            painter.line_segment([a, b], Stroke::new(1.0, t.body_selected));
        }
        for via in &edits.vias {
            let at = screen(via.position);
            painter.circle(
                at,
                (via.diameter as f32 * scale / 2.0).max(4.0),
                t.body_selected,
                Stroke::new(1.0, t.on_accent),
            );
            painter.circle_filled(
                at,
                (via.drill as f32 * scale / 2.0).max(2.0),
                BoardInk::get().drill,
            );
        }
        painter.circle_stroke(screen(end), 5.0, Stroke::new(1.5, t.body_selected));
    }

    /// The via size a layer change places on the routed net: the size
    /// chosen while routing it, else its class's.
    pub(super) fn route_via_size(&self, project: &Project) -> (f64, f64) {
        self.route
            .via_size
            .filter(|(net, _)| Some(*net) == self.net)
            .map(|(_, size)| size)
            .unwrap_or_else(|| self.class_via_size(project))
    }

    /// The step from the route head to `to` with the current settings and
    /// pending via; `None` without a route or net.
    pub(super) fn route_step(&self, project: &Project, to: Point) -> Option<StepRequest> {
        let (head, net) = (self.route_start?, self.net?);
        let (drill, diameter) = self.route_via_size(project);
        Some(StepRequest {
            net,
            width: self.width,
            posture: self.route.posture,
            flipped: self.route.flipped,
            head,
            layer: self.pending_via.map_or(self.layer, |(_, from)| from),
            via: self.pending_via.map(|(at, _)| {
                (
                    ViaRequest {
                        drill,
                        diameter,
                        tented: self.via_tented,
                        ..ViaRequest::through(net, at)
                    },
                    self.layer,
                )
            }),
            to,
        })
    }

    /// A Route click at `at`: the first starts the route on the pad's net;
    /// later ones send the step there. A step onto a pad, track or via of
    /// the routed net (other than this route's own) finishes the route.
    pub(super) fn route_click(
        &mut self,
        project: &Project,
        revision: u64,
        at: Point,
        scale: f32,
    ) -> Option<Operation> {
        if self.route.commit.is_some() {
            return None;
        }
        // A pad of a differential pair starts both halves (`pair_tool`).
        if self.route_start.is_none() && self.pair_start(project, at, scale) {
            return None;
        }
        if self.route.pair.is_some() {
            return self.pair_click(project, revision, at, scale);
        }
        let snapped = self.route_snap(at, scale).map(|p| (p.center(), p.net));
        let destination = snapped
            .map(|(c, _)| c)
            .unwrap_or_else(|| self.route_point(at, scale));
        if self.route_start.is_none() {
            if let Some((_, Some(net))) = snapped {
                self.net = Some(net);
                self.route_highlight(self.net);
            }
            if self.net.is_some() {
                self.follow_class_width(project);
                self.route_start = Some(destination);
                self.route.steps.clear();
            }
            return None;
        }
        let net = self.net?;
        self.follow_class_width(project);
        let finish = snapped.is_some_and(|(_, n)| n == Some(net))
            || self.joins_net_copper(project, net, destination);
        let request = self.route_step(project, destination)?;
        let edits = posture::step(&request);
        if edits.add.is_empty() && edits.vias.is_empty() {
            if finish {
                self.finish_route();
            }
            return None;
        }
        self.route.commit = Some(Commit::Step {
            revision,
            next_id: project.next_id,
            from: request.head,
            layer: request.layer,
            to: destination,
            finish,
            pair: None,
        });
        Some(Operation::Layout(LayoutCommand::ApplyRoute { edits }))
    }

    /// Whether `at` is on copper of `net` on the routing layer that this
    /// route did not add: a track or a via.
    fn joins_net_copper(&self, project: &Project, net: Id, at: Point) -> bool {
        let own = |id: Id| {
            self.route
                .steps
                .iter()
                .any(|s| s.tracks.contains(&id) || s.vias.contains(&id))
        };
        project.tracks.iter().any(|t| {
            t.net == net
                && t.layer == self.layer
                && !own(t.id)
                && (t.start == at || t.end == at || layout::track_contains(t, at, 0.0))
        }) || project.vias.iter().any(|v| {
            v.net == net
                && !own(v.id)
                && pcbcraft_core::geometry::distance(v.position, at) <= v.diameter / 2.0
        })
    }

    /// Backspace: drop a pending via, else unwind the last step (one
    /// `layout.route_apply` removing exactly its tracks and via), else end
    /// the route at its start.
    pub(super) fn route_back(&mut self, revision: u64) -> Option<Operation> {
        if self.route.commit.is_some() || self.route_start.is_none() {
            return None;
        }
        if let Some((_, from)) = self.pending_via.take() {
            self.layer = from;
            return None;
        }
        let Some(step) = self.route.steps.last() else {
            self.reset_route();
            return None;
        };
        // A pair step also gives back the previous tracks it extended.
        let edits = RouteEditSet {
            remove: step.tracks.clone(),
            remove_vias: step.vias.clone(),
            replace: step
                .pair
                .as_ref()
                .map(|undo| undo.restore.clone())
                .unwrap_or_default(),
            ..Default::default()
        };
        self.route.commit = Some(Commit::Unwind { revision });
        Some(Operation::Layout(LayoutCommand::ApplyRoute { edits }))
    }

    /// Route on `layer`. During a route a via goes at `at` (the head when
    /// `None`), unless one is already pending or the head already joins
    /// both layers (a via or through-hole pad of the net there); back on
    /// the layer the pending via was reached on, no via.
    pub(super) fn change_layer_to(
        &mut self,
        project: &Project,
        layer: CopperLayer,
        at: Option<Point>,
    ) {
        if layer == self.layer || !self.stack.contains(&layer) {
            return;
        }
        if let (Some(head), None) = (self.route_start, self.pending_via) {
            let at = at.unwrap_or(head);
            let joined = at == head && self.head_joins_layers(project, head, layer);
            if !joined {
                self.pending_via = Some((at, self.layer));
            }
        }
        if self.pending_via.is_some_and(|(_, from)| from == layer) {
            self.pending_via = None;
        }
        self.layer = layer;
    }

    /// Whether copper of the routed net at `head` already joins the
    /// current layer and `layer`.
    fn head_joins_layers(&self, project: &Project, head: Point, layer: CopperLayer) -> bool {
        let Some(net) = self.net else {
            return false;
        };
        let via = project.vias.iter().any(|v| {
            v.net == net
                && pcbcraft_core::geometry::distance(v.position, head) <= v.diameter / 2.0
                && v.on(self.layer, &self.stack)
                && v.on(layer, &self.stack)
        });
        via || self.board_pads.iter().any(|b| {
            b.placed.net == Some(net)
                && b.contains(head)
                && pad_on(&b.placed, self.layer)
                && pad_on(&b.placed, layer)
        })
    }

    /// The Route tool's keys while the canvas is hovered and no field has
    /// the keyboard; returns an unwind Backspace sends.
    pub(super) fn route_keys(
        &mut self,
        ui: &egui::Ui,
        project: &Project,
        revision: u64,
        pointer: Option<Point>,
    ) -> Option<Operation> {
        use egui::Key;
        if self.tool != Tool::Route || ui.ctx().egui_wants_keyboard_input() {
            return None;
        }
        let pressed = |key| ui.input(|i| i.key_pressed(key));
        let shift = ui.input(|i| i.modifiers.shift);
        if pressed(Key::Enter) {
            self.finish_route();
        }
        if pressed(Key::Slash) {
            self.route.posture = self.route.posture.next();
        }
        if pressed(Key::F) {
            self.route.flipped = !self.route.flipped;
        }
        if pressed(Key::W) {
            self.cycle_width(project, shift);
        }
        if pressed(Key::D) {
            self.cycle_via_size(project, shift);
        }
        if pressed(Key::P) && !shift {
            self.route.single = !self.route.single;
        }
        if pressed(Key::V) {
            self.next_layer(project, pointer, shift);
        }
        let digits = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        if let Some(index) = digits.iter().position(|k| pressed(*k))
            && let Some(&layer) = self.stack.get(index)
        {
            self.change_layer_to(project, layer, pointer);
        }
        if pressed(Key::Backspace) {
            return self.route_back(revision);
        }
        None
    }

    /// V: the next copper layer of the stackup (Shift: the previous one),
    /// through a via at `at` during a route.
    pub(super) fn next_layer(&mut self, project: &Project, at: Option<Point>, reverse: bool) {
        let Some(index) = self.stack.iter().position(|l| *l == self.layer) else {
            self.layer = CopperLayer::Top;
            return;
        };
        let count = self.stack.len().max(1);
        let next = if reverse {
            (index + count - 1) % count
        } else {
            (index + 1) % count
        };
        if let Some(&layer) = self.stack.get(next) {
            self.change_layer_to(project, layer, at);
        }
    }

    /// W: the next track width preset of the routed net (Shift: the
    /// previous); it applies from the next step.
    pub(super) fn cycle_width(&mut self, project: &Project, back: bool) {
        let Some(net) = self.net.filter(|n| project.net(*n).is_some()) else {
            return;
        };
        self.follow_class_width(project);
        let presets = layout::route_widths(project, net);
        if let Some(width) = cycle(&presets, self.width, back, by_value) {
            self.width = width;
        }
    }

    /// D: the next via size preset of the routed net (Shift: the
    /// previous), for the next layer change.
    pub(super) fn cycle_via_size(&mut self, project: &Project, back: bool) {
        let Some(net) = self.net.filter(|n| project.net(*n).is_some()) else {
            return;
        };
        let presets = layout::via_sizes(project, net);
        if let Some(size) = cycle(&presets, self.route_via_size(project), back, by_pad) {
            self.route.via_size = Some((net, size));
            self.route.via_draft = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_cycle_both_ways_and_wrap() {
        let widths = [0.2, 0.3, 0.8];
        assert_eq!(cycle(&widths, 0.3, false, by_value), Some(0.8));
        assert_eq!(cycle(&widths, 0.8, false, by_value), Some(0.2));
        assert_eq!(cycle(&widths, 0.3, true, by_value), Some(0.2));
        assert_eq!(cycle(&widths, 0.2, true, by_value), Some(0.8));
        // A width typed between presets steps to the neighbours.
        assert_eq!(cycle(&widths, 0.25, false, by_value), Some(0.3));
        assert_eq!(cycle(&widths, 0.25, true, by_value), Some(0.2));
        assert_eq!(cycle::<f64>(&[], 0.25, true, by_value), None);
        assert_eq!(ROUTE_KEYS.len(), 11);
    }
}
