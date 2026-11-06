//! The Route tool's interactive route: continuous steps from a head that
//! follows each accepted step, postures, widths and via sizes from the
//! rules, layer changes through a via, and Backspace unwinding.
//!
//! Each click is one gesture and one undo step: the router's plan of the
//! step ([`router::plan`] in the mode M chooses: shove, walkaround or
//! ignore) goes to the engine as one `layout.route_apply`, with every
//! track and via it shoves. The preview draws the same plan, computed
//! once per revision and pointer position from the committed board, so
//! shoved copper springs back as the pointer moves away. The route
//! continues from where the plan ends (short of the pointer when no way
//! exists) once the engine has taken it (the next revision is exactly
//! that step); a refusal leaves the route as it was. Backspace sends one
//! `layout.route_apply` removing exactly the last step's tracks and via
//! and putting back what it shoved, and moves the head back. Any other
//! document change ends the route.
use super::{BoardInk, BoardView, PcbEditor, Tool, capsule, pad_on};
use crate::commands::KeyDecl;
use crate::keys;
use egui::{Shape, Stroke};
use pcbcraft_core::{
    CopperLayer, Id, Operation, Point, Project,
    layout::{
        self, LayoutCommand, RouteEditSet, ViaRequest,
        pair::PairStepRequest,
        posture::{Posture, StepRequest},
        router::{self, PairPlan, RouteIndex, RouteMode, RoutePlan, RouteStepRequest},
    },
};
use std::cmp::Ordering;
use std::rc::Rc;

/// The Route tool's keys while the canvas is hovered. Each is a registry
/// command (`crate::commands::KeyDecl`), read through `crate::keys`, so a
/// rebinding changes the key itself. The Route card lists them, with the
/// Layout's layer keys (`LAYER_KEYS`: 1 to 9 and 0 change layer through a
/// via while routing).
pub const ROUTE_KEYS: &[KeyDecl] = &[
    KeyDecl::new(
        "route.click",
        "Click",
        "Corner; a pad or track of the net finishes",
    ),
    KeyDecl::new(
        "route.finish",
        "Enter",
        "Finish the route (or double-click)",
    ),
    KeyDecl::new(
        "route.end",
        "Esc",
        "End the route (or right-click); again: Select",
    ),
    KeyDecl::new("route.back", "Backspace", "Remove the last segment"),
    KeyDecl::new("route.via-next", "V", "Via to the next layer"),
    KeyDecl::new("route.via-previous", "Shift+V", "Via to the previous layer"),
    KeyDecl::new("route.posture", "/", "Next mode: 45°, 90°, free"),
    // F with any Shift: ⇧F must not also flip the board view.
    KeyDecl::new("route.flip", "F, Shift+F", "Flip the elbow corner"),
    KeyDecl::new("route.width-next", "W", "Next track width"),
    KeyDecl::new("route.width-previous", "Shift+W", "Previous track width"),
    KeyDecl::new("route.via-size-next", "D", "Next via size"),
    KeyDecl::new("route.via-size-previous", "Shift+D", "Previous via size"),
    KeyDecl::new(
        "route.pair",
        "P",
        "Pair routing on or off (a pad of a pair starts both halves)",
    ),
    KeyDecl::new(
        "route.router-mode-next",
        "M",
        "Next router: shove, walkaround, ignore",
    ),
    KeyDecl::new(
        "route.router-mode-previous",
        "Shift+M",
        "Previous router: ignore, walkaround, shove",
    ),
];

/// The keys the Route card lists: the Route tool's, with the Layout's
/// layer keys after the via keys.
pub(crate) fn route_key_table() -> Vec<KeyDecl> {
    let mut table = Vec::with_capacity(ROUTE_KEYS.len() + super::LAYER_KEYS.len());
    for decl in ROUTE_KEYS {
        table.push(*decl);
        if decl.id == "route.via-previous" {
            table.extend(super::LAYER_KEYS);
        }
    }
    table
}

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
    /// What a step does about other nets' copper (M).
    pub(super) mode: RouteMode,
    /// Shoving leaves vias where they are.
    pub(super) vias_stay: bool,
    /// The router's index of the board, for one revision.
    pub(super) index: Option<(u64, Rc<RouteIndex>)>,
    /// The last step planned (the preview), with its inputs.
    pub(super) planned: Option<(PlanKey, Rc<Planned>)>,
    /// Revisions the fill worker made by storing fill copper (from, to):
    /// no track or via changed, so a route continues through them.
    pub(super) fill_stores: Vec<(u64, u64)>,
    /// A route just finished without Esc (Enter, double-click,
    /// right-click or a step onto the net's copper): the next Esc ends
    /// nothing and keeps Route; the one after leaves it.
    pub(super) ended: bool,
    /// Why the last click was refused or stopped short, for a toast.
    pub(super) refused: Option<String>,
}

/// What a planned step was planned from.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PlanKey {
    Single(u64, RouteStepRequest),
    Pair(u64, Box<PairStepRequest>, RouteMode, bool),
}

/// A planned step: one net's, or a pair's.
#[derive(Clone, Debug)]
pub(super) enum Planned {
    Single(Box<RoutePlan>),
    Pair(Box<PairPlan>),
}

impl Planned {
    pub(super) fn edits(&self) -> &RouteEditSet {
        match self {
            Planned::Single(plan) => &plan.edits,
            Planned::Pair(plan) => &plan.step.edits,
        }
    }

    /// Tracks and vias the step moves (drawn at their new places only).
    pub(super) fn moved(&self) -> (&[Id], &[Id]) {
        match self {
            Planned::Single(plan) => (&plan.shoved, &plan.moved_vias),
            Planned::Pair(plan) => (&plan.shoved, &plan.moved_vias),
        }
    }

    pub(super) fn reached(&self) -> bool {
        match self {
            Planned::Single(plan) => plan.reached,
            Planned::Pair(plan) => plan.reached,
        }
    }

    /// A pair in the way that could not be shoved as one unit.
    pub(super) fn note(&self) -> Option<&str> {
        match self {
            Planned::Single(plan) => plan.note.as_deref(),
            Planned::Pair(plan) => plan.note.as_deref(),
        }
    }

    /// Where the head stops.
    pub(super) fn end(&self) -> Point {
        match self {
            Planned::Single(plan) => plan.end,
            Planned::Pair(plan) => plan.step.head.center,
        }
    }
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
    /// Puts back what the step shoved (`RoutePlan::undo`).
    pub(super) unshove: RouteEditSet,
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
        /// Whether the step placed the pending via (a step stopped short
        /// of it keeps it pending).
        via_placed: bool,
        /// Puts back what the step shoved.
        unshove: Box<RouteEditSet>,
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

/// What a click's plan owes the user: why nothing was added, or why the
/// step stopped short. `None` when it reached the click.
pub(super) fn refusal(
    reached: bool,
    blocked: Option<&str>,
    edits: &RouteEditSet,
) -> Option<String> {
    if reached {
        return None;
    }
    let why = blocked.unwrap_or("no legal way to that point on this layer");
    Some(if edits.add.is_empty() && edits.vias.is_empty() {
        format!("Can't route there: {why}")
    } else {
        format!("Route stopped short: {why}")
    })
}

/// The point of the segment `a`–`b` nearest `p`.
fn nearest_on_segment(p: Point, a: Point, b: Point) -> Point {
    let d = [b[0] - a[0], b[1] - a[1]];
    let length = d[0] * d[0] + d[1] * d[1];
    if length <= 0.0 {
        return a;
    }
    let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length).clamp(0.0, 1.0);
    [a[0] + d[0] * t, a[1] + d[1] * t]
}

impl PcbEditor {
    /// During a route, the nearest point within 8 pixels of `at` on a track
    /// of the routed net on the routing layer that this route did not add:
    /// a click just beside the track ends on it, not on the grid next to it.
    pub(super) fn net_track_point(
        &self,
        project: &Project,
        at: Point,
        scale: f32,
    ) -> Option<Point> {
        self.route_start?;
        let net = self.net?;
        let reach = f64::from(8.0 / scale);
        let own = |id: Id| self.route.steps.iter().any(|s| s.tracks.contains(&id));
        project
            .tracks
            .iter()
            .filter(|t| t.net == net && t.layer == self.layer && !own(t.id))
            .map(|t| nearest_on_segment(at, t.start, t.end))
            .map(|p| (p, pcbcraft_core::geometry::distance(p, at)))
            .filter(|(_, d)| *d < reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(p, _)| p)
    }

    /// What a route's first click starts on, whatever net the card still
    /// shows: a pad on this layer, a via, a visible track (this layer's
    /// first) within 8 pixels, or a visible pad on another layer, which
    /// the route moves to. Its net, the head and the layer.
    fn route_start_on(
        &self,
        project: &Project,
        at: Point,
        scale: f32,
    ) -> Option<(Id, Point, CopperLayer)> {
        if let Some(pad) = self.route_snap(at, scale)
            && let Some(net) = pad.net
        {
            return Some((net, pad.center(), self.layer));
        }
        if let Some(via) = self.pick_via(project, at) {
            return Some((via.net, via.position, self.layer));
        }
        let reach = f64::from(8.0 / scale);
        let seen = self.seen();
        let track = project
            .tracks
            .iter()
            .filter(|t| seen.copper_layer(t.layer))
            .map(|t| {
                let end = [t.start, t.end]
                    .into_iter()
                    .find(|e| pcbcraft_core::geometry::distance(*e, at) < reach);
                let on = end.unwrap_or_else(|| nearest_on_segment(at, t.start, t.end));
                (t, on, pcbcraft_core::geometry::distance(on, at))
            })
            .filter(|(_, _, d)| *d < reach)
            .min_by(|a, b| {
                (a.0.layer != self.layer)
                    .cmp(&(b.0.layer != self.layer))
                    .then(a.2.total_cmp(&b.2))
            });
        if let Some((t, on, _)) = track {
            return Some((t.net, on, t.layer));
        }
        let pad = self.pick_pad(at, |p| seen.pad(p))?;
        let layer = self.stack.iter().copied().find(|l| pad_on(pad, *l))?;
        Some((pad.net?, pad.center(), layer))
    }

    /// Drop the route in progress: its head, pending via and steps. The
    /// tool, net and settings stay.
    pub(super) fn reset_route(&mut self) {
        self.route_start = None;
        self.pending_via = None;
        self.route.steps.clear();
        self.route.commit = None;
        self.route.pair = None;
        self.route.planned = None;
    }

    /// Finish the route where it is (Enter, double-click, Esc, right-click,
    /// or a step onto the net's copper): its accepted steps stay, a via
    /// still pending is dropped, and Route stays armed.
    pub(super) fn finish_route(&mut self) {
        if let Some((_, from)) = self.pending_via {
            self.layer = from;
        }
        self.reset_route();
        self.route.ended = true;
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
        let revision = self.route_revision(sent, revision);
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
                via_placed,
                unshove,
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
                    unshove: *unshove,
                });
                if via_placed {
                    self.pending_via = None;
                }
                self.route_start = Some(to);
                if finish {
                    self.reset_route();
                    self.route.ended = true;
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

    /// The fill worker stored fill copper, moving the document from
    /// `before` to `after`; a route in progress continues through it.
    pub fn fills_stored(&mut self, before: u64, after: u64) {
        if after > before {
            self.route.fill_stores.push((before, after));
        }
    }

    /// `revision` less the fill-store revisions made since `since`: the
    /// revision as far as tracks and vias are concerned.
    pub(super) fn route_revision(&self, since: u64, revision: u64) -> u64 {
        let stored: u64 = self
            .route
            .fill_stores
            .iter()
            .filter(|(before, after)| *before >= since && *after <= revision)
            .map(|(before, after)| after - before)
            .sum();
        revision.saturating_sub(stored)
    }

    /// The router's index of the board at `revision`, built once.
    fn route_index(&mut self, project: &Project, revision: u64) -> Rc<RouteIndex> {
        match &self.route.index {
            Some((at, index)) if *at == revision => Rc::clone(index),
            _ => {
                let index = Rc::new(RouteIndex::of(project));
                self.route.index = Some((revision, Rc::clone(&index)));
                index
            }
        }
    }

    /// The step to `end` as the router plans it in the current mode: the
    /// preview draws it and a click commits exactly it. Planned once per
    /// revision and input, so a still pointer costs nothing.
    pub(super) fn route_plan(
        &mut self,
        project: &Project,
        revision: u64,
        end: Point,
    ) -> Option<Rc<Planned>> {
        let mode = self.route.mode;
        let move_vias = !self.route.vias_stay;
        let key = if self.route.pair.is_some() {
            let request = self.pair_step(project, end)?;
            PlanKey::Pair(revision, Box::new(request), mode, move_vias)
        } else {
            let step = self.route_step(project, end)?;
            PlanKey::Single(revision, RouteStepRequest::of(&step, mode, move_vias))
        };
        if let Some((planned, plan)) = &self.route.planned
            && *planned == key
        {
            return Some(Rc::clone(plan));
        }
        let index = self.route_index(project, revision);
        let plan = Rc::new(match &key {
            PlanKey::Single(_, request) => {
                Planned::Single(Box::new(router::plan(project, &index, request)))
            }
            PlanKey::Pair(_, request, mode, move_vias) => Planned::Pair(Box::new(
                router::pair_plan(project, &index, request, *mode, *move_vias),
            )),
        });
        self.route.planned = Some((key, Rc::clone(&plan)));
        Some(plan)
    }

    /// The tracks and vias the previewed step moves: the board draws them
    /// at their new places only.
    pub(super) fn route_moved(&self, revision: u64) -> (Vec<Id>, Vec<Id>) {
        match &self.route.planned {
            Some((PlanKey::Single(at, _) | PlanKey::Pair(at, ..), plan))
                if *at == revision && self.route_start.is_some() =>
            {
                let (tracks, vias) = plan.moved();
                (tracks.to_vec(), vias.to_vec())
            }
            _ => (Vec::new(), Vec::new()),
        }
    }

    /// The live preview of the planned step to `end`: the copper it adds
    /// at its width on each layer, the copper it shoves where it goes
    /// (outlined in the accent), its vias, and where the head will be: the
    /// target ring when it arrives, a warning ring and line where it stops
    /// short. The same plan the click sends.
    pub(super) fn paint_route_preview(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        project: &Project,
        end: Point,
    ) {
        let Some((_, plan)) = &self.route.planned else {
            return;
        };
        let edits = plan.edits();
        let t = solidcraft_ui_egui::theme::Tokens::get();
        let scale = view.view.scale;
        let screen = |p: Point| view.screen(p);
        let ink = BoardInk::get();
        let colors = crate::palette::board();
        let own = |n: Id| {
            Some(n) == self.net
                || self
                    .route
                    .pair
                    .as_ref()
                    .is_some_and(|p| p.nets.contains(&n))
        };
        let leg = |start: Point, end: Point, width: f64, layer: CopperLayer, net: Id| {
            let width = (width as f32 * scale).max(1.0);
            let color = ink.on_board(colors.copper_layer(layer), 1.0);
            let (a, b) = (screen(start), screen(end));
            painter.add(Shape::convex_polygon(
                capsule(a, b, width / 2.0),
                color,
                Stroke::NONE,
            ));
            let outline = if own(net) { t.body_selected } else { t.accent };
            painter.line_segment([a, b], Stroke::new(1.0, outline));
        };
        for r in &edits.add {
            leg(r.start, r.end, r.width, r.layer, r.net);
        }
        for track in &edits.replace {
            leg(track.start, track.end, track.width, track.layer, track.net);
        }
        let via_ring = |at: Point, diameter: f64, drill: f64, fill, outline| {
            let at = screen(at);
            painter.circle(
                at,
                (diameter as f32 * scale / 2.0).max(4.0),
                fill,
                Stroke::new(1.0, outline),
            );
            painter.circle_filled(at, (drill as f32 * scale / 2.0).max(2.0), ink.drill);
        };
        for via in &edits.vias {
            via_ring(
                via.position,
                via.diameter,
                via.drill,
                t.body_selected,
                t.on_accent,
            );
        }
        for moved in &edits.move_vias {
            if let Some(via) = project.via(moved.id) {
                let fill = ink.on_board(colors.copper_layer(self.layer), 1.0);
                via_ring(moved.position, via.diameter, via.drill, fill, t.accent);
            }
        }
        if plan.reached() {
            painter.circle_stroke(screen(end), 5.0, Stroke::new(1.5, t.body_selected));
        } else {
            let stop = screen(plan.end());
            painter.circle_stroke(stop, 5.0, Stroke::new(1.5, t.warning));
            painter.line_segment([stop, screen(end)], Stroke::new(1.0, t.warning));
        }
        if let Some(note) = plan.note() {
            painter.text(
                screen(end) + egui::vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                note,
                egui::FontId::proportional(t.font_label),
                t.warning,
            );
        }
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
        if self.route_start.is_none() {
            let start = self.route_start_on(project, at, scale);
            let head = match start {
                Some((net, head, layer)) => {
                    self.net = Some(net);
                    self.layer = layer;
                    self.route_highlight(self.net);
                    Some(head)
                }
                None => self.net.map(|_| self.route_point(project, at, scale)),
            };
            match head {
                Some(head) => {
                    self.follow_class_width(project);
                    self.route_start = Some(head);
                    self.route.steps.clear();
                }
                None => {
                    self.route.refused = Some(
                        "Can't start a route here: click a pad, track or via, or choose a Net"
                            .to_owned(),
                    );
                }
            }
            return None;
        }
        let snapped = self
            .route_pad(project, at, scale)
            .map(|p| (p.center(), p.net));
        let destination = snapped
            .map(|(c, _)| c)
            .unwrap_or_else(|| self.route_point(project, at, scale));
        let net = self.net?;
        self.follow_class_width(project);
        let finish = snapped.is_some_and(|(_, n)| n == Some(net))
            || self.joins_net_copper(project, net, destination);
        let request = self.route_step(project, destination)?;
        let planned = self.route_plan(project, revision, destination)?;
        let Planned::Single(plan) = planned.as_ref() else {
            return None;
        };
        self.route.refused = refusal(plan.reached, plan.blocked.as_deref(), &plan.edits);
        if plan.edits.add.is_empty() && plan.edits.vias.is_empty() {
            if finish && plan.reached {
                self.finish_route();
            }
            return None;
        }
        self.route.commit = Some(Commit::Step {
            revision,
            next_id: project.next_id,
            from: request.head,
            layer: request.layer,
            to: plan.end,
            finish: finish && plan.reached,
            via_placed: plan.via_placed,
            unshove: Box::new(plan.undo.clone()),
            pair: None,
        });
        Some(Operation::Layout(LayoutCommand::ApplyRoute {
            edits: plan.edits.clone(),
        }))
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
        // A pair step also gives back the previous tracks it extended;
        // a shoving step puts back what it shoved.
        let mut replace: Vec<_> = step
            .pair
            .as_ref()
            .map(|undo| undo.restore.clone())
            .unwrap_or_default();
        replace.extend(step.unshove.replace.iter().cloned());
        let edits = RouteEditSet {
            remove: step.tracks.clone(),
            remove_vias: step.vias.clone(),
            replace,
            restore: step.unshove.restore.clone(),
            move_vias: step.unshove.move_vias.clone(),
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
        if self.tool != Tool::Route || ui.ctx().egui_wants_keyboard_input() {
            return None;
        }
        let pressed = |id| keys::pressed(ui.ctx(), id);
        if pressed("route.finish") {
            self.finish_route();
        }
        if pressed("route.posture") {
            self.route.posture = self.route.posture.next();
        }
        if pressed("route.flip") {
            self.route.flipped = !self.route.flipped;
        }
        for (id, reverse) in [("route.width-next", false), ("route.width-previous", true)] {
            if pressed(id) {
                self.cycle_width(project, reverse);
            }
        }
        for (id, reverse) in [
            ("route.via-size-next", false),
            ("route.via-size-previous", true),
        ] {
            if pressed(id) {
                self.cycle_via_size(project, reverse);
            }
        }
        if pressed("route.pair") {
            self.route.single = !self.route.single;
        }
        if pressed("route.router-mode-next") {
            self.route.mode = self.route.mode.next();
        }
        if pressed("route.router-mode-previous") {
            self.route.mode = self.route.mode.previous();
        }
        for (id, reverse) in [("route.via-next", false), ("route.via-previous", true)] {
            if pressed(id) {
                self.next_layer(project, pointer, reverse);
            }
        }
        if pressed("route.back") {
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
        assert_eq!(ROUTE_KEYS.len(), 15);
    }
}
