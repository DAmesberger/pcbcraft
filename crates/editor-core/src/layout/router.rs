//! Interactive routing modes: what one route step does about other nets'
//! copper in its way.
//!
//! - **Ignore** proposes the plain posture step ([`posture::step`]); the
//!   engine refuses it on a clearance violation, as before.
//! - **Walkaround** routes the head round tracks, vias, pads, keepouts and
//!   cutouts at the rule clearance, hugging them at 45° (or 90°), and
//!   stops at the last legal point when no way round exists
//!   (`walk`).
//! - **Shove** pushes other nets' tracks and vias aside, recursively and
//!   within a budget (`shove`). When that cannot make room it walks round
//!   the immovable copper and shoves along that path, else walks round
//!   everything, else shoves as far along the straight step as it can.
//!   It never proposes a violation.
//!
//! In every mode, a step that would run back against the track ending at
//! its head moves that corner back along the track instead (`corner`).
//!
//! [`plan`] is pure: the desktop's preview and the click that commits it
//! (one `layout.route_apply`, one undo step, every shoved item included)
//! call it on the same revision, and `layout.route_step` calls it inside
//! the engine for CLI and MCP callers. Because the preview is recomputed
//! from the committed board on every pointer move, shoved copper springs
//! back as soon as the head moves away. Differential pairs route through
//! [`pair_plan`], which treats both halves as one unit. Dragging an
//! existing track shoves through the same code ([`drag::drag_plan`]).
mod clean;
mod corner;
pub mod drag;
mod hull;
pub mod reroute;
mod shove;
mod walk;
mod world;

#[cfg(test)]
mod tests;

use self::walk::Walker;
use self::world::{Ctx, Hit, Piece, State};
use super::pair::{self, PairStep, PairStepRequest};
use super::posture::{self, Posture, StepRequest};
use super::{RouteEditSet, RouteRequest, ViaMove, ViaRequest};
use crate::diagnostics::{Error, ObjectRef, Result};
use crate::geometry::{Point, distance};
use crate::ids::Id;
use crate::model::{CopperLayer, Project, Track, Via};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub use world::RouteIndex;

/// What a route step does about other nets' copper in its way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteMode {
    /// Push other nets' tracks and vias aside; fall back to walking round.
    #[default]
    Shove,
    /// Route round other nets' copper at the clearance.
    Walkaround,
    /// Route straight; the engine refuses a clearance violation.
    Ignore,
}

impl RouteMode {
    pub const ALL: [RouteMode; 3] = [RouteMode::Shove, RouteMode::Walkaround, RouteMode::Ignore];

    /// The mode after this one, wrapping.
    pub fn next(self) -> Self {
        match self {
            RouteMode::Shove => RouteMode::Walkaround,
            RouteMode::Walkaround => RouteMode::Ignore,
            RouteMode::Ignore => RouteMode::Shove,
        }
    }

    /// The mode before this one, wrapping.
    pub fn previous(self) -> Self {
        match self {
            RouteMode::Shove => RouteMode::Ignore,
            RouteMode::Walkaround => RouteMode::Shove,
            RouteMode::Ignore => RouteMode::Walkaround,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RouteMode::Shove => "Shove",
            RouteMode::Walkaround => "Walkaround",
            RouteMode::Ignore => "Ignore",
        }
    }
}

fn yes() -> bool {
    true
}

/// `layout.route_step`: one interactive route step from `head` toward
/// `to` on `layer`, optionally through `via` onto `via_layer`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteStepRequest {
    pub net: Id,
    pub width: f64,
    pub layer: CopperLayer,
    pub head: Point,
    pub to: Point,
    #[serde(default)]
    pub posture: Posture,
    #[serde(default)]
    pub flipped: bool,
    /// A layer change on the way: the via, then routing on `via_layer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<ViaRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_layer: Option<CopperLayer>,
    #[serde(default)]
    pub mode: RouteMode,
    /// Whether a shove may move vias (locked vias never move).
    #[serde(default = "yes")]
    pub move_vias: bool,
    /// The largest `RoutePlan::detour_factor` to accept, at least 1. A
    /// longer path proposes nothing, and `layout.route_step` is refused.
    /// A posture's own corners count: an unobstructed 45° step can reach
    /// 1.083 and a 90° step 1.415.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_detour: Option<f64>,
}

impl RouteStepRequest {
    /// The request for a posture step.
    pub fn of(step: &StepRequest, mode: RouteMode, move_vias: bool) -> Self {
        Self {
            net: step.net,
            width: step.width,
            layer: step.layer,
            head: step.head,
            to: step.to,
            posture: step.posture,
            flipped: step.flipped,
            via: step.via.map(|(v, _)| v),
            via_layer: step.via.map(|(_, l)| l),
            mode,
            move_vias,
            max_detour: None,
        }
    }

    /// Refuses a `max_detour` no path can meet.
    pub fn check(&self) -> Result<()> {
        match self.max_detour {
            Some(max) if !(max.is_finite() && max >= 1.0) => Err(Error::Invalid(format!(
                "route.max_detour: the limit is a factor of at least 1, not {max}"
            ))),
            _ => Ok(()),
        }
    }

    fn step(&self) -> StepRequest {
        StepRequest {
            net: self.net,
            width: self.width,
            posture: self.posture,
            flipped: self.flipped,
            head: self.head,
            layer: self.layer,
            via: self.via.zip(self.via_layer),
            to: self.to,
        }
    }
}

/// What one step proposes (`layout.route_preview`): the edit set to
/// apply, where the head ends, and what was shoved and how to put it
/// back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutePlan {
    /// The step's copper first, then the shoved copper's replacements.
    pub edits: RouteEditSet,
    pub end: Point,
    pub end_layer: CopperLayer,
    /// Whether the head reached the target (else it stopped at the last
    /// legal point).
    pub reached: bool,
    /// Whether the step places its via.
    pub via_placed: bool,
    /// The mode that produced the proposal (a shove may fall back to
    /// walking round).
    pub used: RouteMode,
    /// Tracks the step moves (their identities are kept where they can
    /// be) and vias it moves.
    pub shoved: Vec<Id>,
    pub moved_vias: Vec<Id>,
    /// Puts the shoved copper back: the originals of replaced and removed
    /// tracks and the vias' old positions. Removing the step's own new
    /// copper is the caller's.
    pub undo: RouteEditSet,
    /// Why the head stopped short, when it did.
    pub blocked: Option<String>,
    /// The track, via or pad `blocked` names, for the preview to point
    /// at. Not part of the wire format.
    #[serde(skip)]
    pub blocker: Option<ObjectRef>,
    /// The head's path length over the straight line to where it ends
    /// (through the via, when the step places one): 1 for a straight
    /// step. Shoved copper does not count.
    pub detour_factor: f64,
    /// A differential pair in the way could not be shoved as one unit, so
    /// it stayed where it is and the step went round or stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A step's head copper: a polyline on the step's layer, and after a
/// via, one on the via's layer.
#[derive(Clone, Debug, Default)]
struct Head {
    first: Vec<Point>,
    via: Option<ViaRequest>,
    second: Vec<Point>,
}

fn legs(net: Id, width: f64, layer: CopperLayer, points: &[Point]) -> Vec<RouteRequest> {
    points
        .windows(2)
        .map(|w| RouteRequest {
            net,
            start: w[0],
            end: w[1],
            width,
            layer,
        })
        .collect()
}

fn via_of(request: &ViaRequest) -> Via {
    Via {
        id: 0,
        net: request.net,
        position: request.position,
        drill: request.drill,
        diameter: request.diameter,
        span: request.span,
        tented: request.tented,
        locked: false,
    }
}

impl Head {
    fn straight(request: &RouteStepRequest) -> Self {
        match request.via.zip(request.via_layer) {
            Some((via, _)) => Self {
                first: posture::path(request.head, via.position, request.posture, request.flipped),
                via: Some(via),
                second: posture::path(via.position, request.to, request.posture, request.flipped),
            },
            None => Self {
                first: posture::path(request.head, request.to, request.posture, request.flipped),
                ..Default::default()
            },
        }
    }

    fn second_layer(&self, request: &RouteStepRequest) -> CopperLayer {
        request.via_layer.unwrap_or(request.layer)
    }

    fn edits(&self, request: &RouteStepRequest) -> RouteEditSet {
        let mut add = legs(request.net, request.width, request.layer, &self.first);
        add.extend(legs(
            request.net,
            request.width,
            self.second_layer(request),
            &self.second,
        ));
        RouteEditSet {
            add,
            vias: self.via.into_iter().collect(),
            ..Default::default()
        }
    }

    fn pieces(&self, ctx: &Ctx<'_>, request: &RouteStepRequest) -> Vec<Piece> {
        let track = |layer, w: &[Point]| match w {
            [a, b] => Some(Piece::track(request.net, layer, *a, *b, request.width)),
            _ => None,
        };
        let mut pieces: Vec<Piece> = self
            .first
            .windows(2)
            .filter_map(|w| track(request.layer, w))
            .collect();
        if let Some(via) = &self.via {
            pieces.push(Piece::via(&via_of(via), &ctx.stack));
        }
        let second = self.second_layer(request);
        pieces.extend(self.second.windows(2).filter_map(|w| track(second, w)));
        pieces
    }

    /// [`RoutePlan::detour_factor`] of this head. An end too near its start
    /// to measure counts as one shortest segment, so a loop back to the
    /// start still shows.
    fn detour_factor(&self, request: &RouteStepRequest) -> f64 {
        let (end, _) = self.end(request);
        let (length, straight) = match &self.via {
            Some(via) => (
                clean::length(&self.first) + clean::length(&self.second),
                distance(request.head, via.position) + distance(via.position, end),
            ),
            None => (clean::length(&self.first), distance(request.head, end)),
        };
        (length / straight.max(posture::MIN_SEGMENT_MM)).max(1.0)
    }

    fn end(&self, request: &RouteStepRequest) -> (Point, CopperLayer) {
        match (&self.via, self.second.last()) {
            (Some(_), Some(p)) => (*p, self.second_layer(request)),
            (Some(v), None) => (v.position, self.second_layer(request)),
            (None, _) => (
                self.first.last().copied().unwrap_or(request.head),
                request.layer,
            ),
        }
    }
}

/// The step `request` proposes on `project` (whose obstacle index is
/// `index`, built from the same revision).
pub fn plan(project: &Project, index: &RouteIndex, request: &RouteStepRequest) -> RoutePlan {
    let ctx = Ctx::new(project, index);
    let plan = corner::recompute(&ctx, request, proposal(&ctx, request));
    match request.max_detour {
        Some(max) if plan.detour_factor > max => too_long(request, plan.detour_factor, max),
        _ => plan,
    }
}

/// The plan for a step whose path exceeds `max_detour`: nothing moves.
fn too_long(request: &RouteStepRequest, factor: f64, max: f64) -> RoutePlan {
    RoutePlan {
        edits: RouteEditSet::default(),
        end: request.head,
        end_layer: request.layer,
        reached: false,
        via_placed: false,
        used: request.mode,
        shoved: Vec::new(),
        moved_vias: Vec::new(),
        undo: RouteEditSet::default(),
        blocked: Some(format!(
            "route.detour: the path is {factor:.2} times the straight line; \
             the limit is {max:.2}"
        )),
        blocker: None,
        detour_factor: factor,
        note: None,
    }
}

fn proposal(ctx: &Ctx<'_>, request: &RouteStepRequest) -> RoutePlan {
    match request.mode {
        RouteMode::Ignore => {
            let edits = posture::step(&request.step());
            let end_layer = request.step().end_layer();
            RoutePlan {
                edits,
                end: request.to,
                end_layer,
                reached: true,
                via_placed: request.via.is_some() && request.via_layer.is_some(),
                used: RouteMode::Ignore,
                shoved: Vec::new(),
                moved_vias: Vec::new(),
                undo: RouteEditSet::default(),
                blocked: None,
                blocker: None,
                detour_factor: Head::straight(request).detour_factor(request),
                note: None,
            }
        }
        RouteMode::Walkaround => {
            let state = State::default();
            let (head, reached, blocker) = walk_head(ctx, &state, request, &|_| true);
            finish(
                ctx,
                request,
                &head,
                &state,
                RouteMode::Walkaround,
                reached,
                blocker,
            )
        }
        RouteMode::Shove => shove_plan(ctx, request),
    }
}

/// Walk each layer of the step; the head, whether it reached the target
/// and what stopped it.
fn walk_head(
    ctx: &Ctx<'_>,
    state: &State,
    request: &RouteStepRequest,
    counts: &dyn Fn(&Hit) -> bool,
) -> (Head, bool, Option<Hit>) {
    let own = [request.net];
    let walker = |layer| Walker {
        net: request.net,
        own: &own,
        layer,
        width: request.width,
        posture: request.posture,
        counts,
    };
    match request.via.zip(request.via_layer) {
        None => {
            let w =
                walker(request.layer).walk(ctx, state, request.head, request.to, request.flipped);
            (
                Head {
                    first: w.points,
                    ..Default::default()
                },
                w.reached,
                w.blocker,
            )
        }
        Some((via, layer)) => {
            let w1 =
                walker(request.layer).walk(ctx, state, request.head, via.position, request.flipped);
            let mut head = Head {
                first: w1.points,
                ..Default::default()
            };
            if !w1.reached {
                return (head, false, w1.blocker);
            }
            let probe = Piece::via(&via_of(&via), &ctx.stack);
            if let Some(hit) = ctx
                .hits(state, &probe, &own)
                .into_iter()
                .find(|h| counts(h))
            {
                return (head, false, Some(hit));
            }
            let w2 = walker(layer).walk(ctx, state, via.position, request.to, request.flipped);
            head.via = Some(via);
            head.second = w2.points;
            (head, w2.reached, w2.blocker)
        }
    }
}

/// Shove for `head`: the overlay with everything pushed, or what could
/// not move.
fn attempt(
    ctx: &Ctx<'_>,
    request: &RouteStepRequest,
    head: &Head,
) -> std::result::Result<State, Hit> {
    let mut state = State {
        frozen: vec![request.net],
        move_vias: request.move_vias,
        fixed: head.pieces(ctx, request),
        ..Default::default()
    };
    shove::shove(ctx, &mut state)?;
    Ok(state)
}

/// Halvings when searching for the last legal point (1/256 of the step).
const BISECTIONS: usize = 8;

/// The longest prefix (by length, of `total`) that `ok` accepts, found
/// by bisection; `None` when even a sliver fails.
fn bisect<T>(total: f64, ok: impl Fn(f64) -> Option<T>) -> Option<(f64, T)> {
    let (mut lo, mut hi) = (0.0f64, total);
    let mut best = None;
    for _ in 0..BISECTIONS {
        let mid = 0.5 * (lo + hi);
        if mid < posture::MIN_SEGMENT_MM {
            break;
        }
        match ok(mid) {
            Some(t) => {
                lo = mid;
                best = Some((mid, t));
            }
            None => hi = mid,
        }
    }
    best
}

fn shove_plan(ctx: &Ctx<'_>, request: &RouteStepRequest) -> RoutePlan {
    let straight = Head::straight(request);
    let first_try = match attempt(ctx, request, &straight) {
        Ok(state) => {
            return finish(
                ctx,
                request,
                &straight,
                &state,
                RouteMode::Shove,
                true,
                None,
            );
        }
        Err(hit) => hit,
    };
    let base = State {
        frozen: vec![request.net],
        move_vias: request.move_vias,
        ..Default::default()
    };
    // Walk round what cannot move, shoving the rest along that path.
    let immovable = |h: &Hit| !ctx.movable(&base, h.target);
    let (around, reached, _) = walk_head(ctx, &base, request, &immovable);
    if reached && let Ok(state) = attempt(ctx, request, &around) {
        return finish(ctx, request, &around, &state, RouteMode::Shove, true, None);
    }
    // Walk round everything.
    let empty = State::default();
    let (walked, reached, blocker) = walk_head(ctx, &empty, request, &|_| true);
    if reached {
        return finish(
            ctx,
            request,
            &walked,
            &empty,
            RouteMode::Walkaround,
            true,
            None,
        );
    }
    // Shove as far along the straight step as room allows.
    let total = clean::length(&straight.first);
    let pushed = bisect(total, |along| {
        let head = Head {
            first: clean::prefix(&straight.first, along),
            ..Default::default()
        };
        if head.first.len() < 2 {
            return None;
        }
        attempt(ctx, request, &head).ok().map(|state| (head, state))
    });
    let walked_end = walked.end(request).0;
    match pushed {
        Some((_, (head, state)))
            if distance(head.end(request).0, request.to)
                <= distance(walked_end, request.to) + 1e-9 =>
        {
            finish(
                ctx,
                request,
                &head,
                &state,
                RouteMode::Shove,
                false,
                Some(first_try),
            )
        }
        _ => finish(
            ctx,
            request,
            &walked,
            &empty,
            RouteMode::Walkaround,
            false,
            blocker.or(Some(first_try)),
        ),
    }
}

/// The shoved copper's edits (replacements, extra and dropped segments,
/// moved vias), the edits putting it back, and what moved.
struct Shoved {
    edits: RouteEditSet,
    undo: RouteEditSet,
    tracks: Vec<Id>,
    vias: Vec<Id>,
}

fn same(a: &[Point], b: &[Point]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(p, q)| distance(*p, *q) < 1e-12)
}

fn shoved(ctx: &Ctx<'_>, state: &State) -> Shoved {
    let mut out = Shoved {
        edits: RouteEditSet::default(),
        undo: RouteEditSet::default(),
        tracks: Vec::new(),
        vias: Vec::new(),
    };
    for line in &state.lines {
        if same(&line.points, &line.original) {
            continue;
        }
        let originals: Vec<Track> = line
            .base
            .iter()
            .filter_map(|k| ctx.base_track(*k).cloned())
            .collect();
        let segments = line.points.len().saturating_sub(1);
        for (i, w) in line.points.windows(2).enumerate() {
            match (line.ids.get(i), originals.get(i)) {
                (Some(id), Some(original)) => {
                    out.edits.replace.push(Track {
                        id: *id,
                        net: line.net,
                        start: w[0],
                        end: w[1],
                        width: line.width,
                        layer: line.layer,
                        locked: false,
                    });
                    out.undo.replace.push(original.clone());
                }
                _ => out.edits.add.push(RouteRequest {
                    net: line.net,
                    start: w[0],
                    end: w[1],
                    width: line.width,
                    layer: line.layer,
                }),
            }
        }
        for (id, original) in line.ids.iter().zip(&originals).skip(segments) {
            out.edits.remove.push(*id);
            out.undo.restore.push(original.clone());
        }
        out.tracks.extend(line.ids.iter().copied());
    }
    for moved in &state.vias {
        if distance(moved.via.position, moved.original) < 1e-12 {
            continue;
        }
        out.edits.move_vias.push(ViaMove {
            id: moved.via.id,
            position: moved.via.position,
        });
        out.undo.move_vias.push(ViaMove {
            id: moved.via.id,
            position: moved.original,
        });
        out.vias.push(moved.via.id);
    }
    out
}

fn finish(
    ctx: &Ctx<'_>,
    request: &RouteStepRequest,
    head: &Head,
    state: &State,
    used: RouteMode,
    reached: bool,
    blocker: Option<Hit>,
) -> RoutePlan {
    let mut edits = head.edits(request);
    let moved = shoved(ctx, state);
    edits.add.extend(moved.edits.add);
    edits.replace.extend(moved.edits.replace);
    edits.remove.extend(moved.edits.remove);
    edits.move_vias.extend(moved.edits.move_vias);
    let (end, end_layer) = head.end(request);
    RoutePlan {
        edits,
        end,
        end_layer,
        reached,
        via_placed: head.via.is_some(),
        used,
        shoved: moved.tracks,
        moved_vias: moved.vias,
        undo: moved.undo,
        blocked: if reached {
            None
        } else {
            Some(stopped_by(ctx, state, blocker))
        },
        blocker: blocking_object(ctx, state, reached, blocker),
        detour_factor: head.detour_factor(request),
        note: ctx.held_note(state),
    }
}

/// Why a head stopped short: the pair it could not shove, else what it hit.
fn stopped_by(ctx: &Ctx<'_>, state: &State, blocker: Option<Hit>) -> String {
    ctx.held_note(state).unwrap_or_else(|| {
        blocker.map_or_else(|| "No room to route".to_owned(), |h| ctx.describe(&h))
    })
}

/// The object [`stopped_by`] names, when it names a board object rather
/// than a held pair, an edge or a keepout.
fn blocking_object(
    ctx: &Ctx<'_>,
    state: &State,
    reached: bool,
    blocker: Option<Hit>,
) -> Option<ObjectRef> {
    if reached || ctx.held_note(state).is_some() {
        return None;
    }
    ctx.object(&blocker?)
}

/// A pair step with its shove: the step (head and edits, the shoved
/// copper's edits appended) and what moved.
#[derive(Clone, Debug)]
pub struct PairPlan {
    pub step: PairStep,
    pub used: RouteMode,
    pub reached: bool,
    pub shoved: Vec<Id>,
    pub moved_vias: Vec<Id>,
    pub undo: RouteEditSet,
    pub blocked: Option<String>,
    /// The track, via or pad `blocked` names.
    pub blocker: Option<ObjectRef>,
    /// Another pair in the way could not be shoved as one unit.
    pub note: Option<String>,
}

/// The overlay holding a pair step's copper: its tracks and vias fixed,
/// the tracks it extends hidden.
fn pair_state(ctx: &Ctx<'_>, request: &PairStepRequest, step: &PairStep, move_vias: bool) -> State {
    let mut state = State {
        frozen: request.nets.to_vec(),
        move_vias,
        ..Default::default()
    };
    for r in &step.edits.add {
        state
            .fixed
            .push(Piece::track(r.net, r.layer, r.start, r.end, r.width));
    }
    for t in &step.edits.replace {
        state
            .fixed
            .push(Piece::track(t.net, t.layer, t.start, t.end, t.width));
        if let Some(k) = ctx.project.tracks.iter().position(|o| o.id == t.id) {
            state.hidden.insert(k);
        }
    }
    for v in &step.edits.vias {
        state.fixed.push(Piece::via(&via_of(v), &ctx.stack));
    }
    state
}

/// What a pair step's own copper collides with, if anything.
fn pair_conflict(ctx: &Ctx<'_>, request: &PairStepRequest, step: &PairStep) -> Option<Hit> {
    let state = pair_state(ctx, request, step, false);
    state
        .fixed
        .iter()
        .find_map(|piece| ctx.hits(&state, piece, &[piece.net]).into_iter().next())
}

/// The pair step `request` proposes in `mode`: both halves walk round
/// obstacles as one envelope (the pitch plus a width) and shove as one
/// unit.
pub fn pair_plan(
    project: &Project,
    index: &RouteIndex,
    request: &PairStepRequest,
    mode: RouteMode,
    move_vias: bool,
) -> PairPlan {
    let ctx = Ctx::new(project, index);
    let plain = pair::step(request);
    let done = |step: PairStep, state: &State, used, reached, blocker: Option<Hit>| {
        let moved = shoved(&ctx, state);
        let mut step = step;
        step.edits.add.extend(moved.edits.add);
        step.edits.replace.extend(moved.edits.replace);
        step.edits.remove.extend(moved.edits.remove);
        step.edits.move_vias.extend(moved.edits.move_vias);
        PairPlan {
            step,
            used,
            reached,
            shoved: moved.tracks,
            moved_vias: moved.vias,
            undo: moved.undo,
            blocked: if reached {
                None
            } else {
                Some(stopped_by(&ctx, state, blocker))
            },
            blocker: blocking_object(&ctx, state, reached, blocker),
            note: ctx.held_note(state),
        }
    };
    let empty = State::default();
    match mode {
        RouteMode::Ignore => done(plain, &empty, RouteMode::Ignore, true, None),
        RouteMode::Walkaround => {
            let (step, reached, blocker) = pair_walk(&ctx, request);
            done(step, &empty, RouteMode::Walkaround, reached, blocker)
        }
        RouteMode::Shove => {
            let mut state = pair_state(&ctx, request, &plain, move_vias);
            let first = match shove::shove(&ctx, &mut state) {
                Ok(()) => return done(plain, &state, RouteMode::Shove, true, None),
                Err(hit) => hit,
            };
            let (walked, reached, blocker) = pair_walk(&ctx, request);
            if reached {
                return done(walked, &empty, RouteMode::Walkaround, true, None);
            }
            // Shove as far along the centre line as room allows.
            let line = posture::path(
                request.head.center,
                request.to,
                request.posture,
                request.flipped,
            );
            let pushed = bisect(clean::length(&line), |along| {
                let to = *clean::prefix(&line, along).last()?;
                let step = pair::step(&PairStepRequest {
                    to,
                    finish: None,
                    via: None,
                    ..request.clone()
                });
                let mut state = pair_state(&ctx, request, &step, move_vias);
                shove::shove(&ctx, &mut state).ok().map(|()| (step, state))
            });
            let walked_end = walked.head.center;
            match pushed {
                Some((_, (step, state)))
                    if distance(step.head.center, request.to)
                        <= distance(walked_end, request.to) + 1e-9 =>
                {
                    done(step, &state, RouteMode::Shove, false, Some(first))
                }
                _ => done(
                    walked,
                    &empty,
                    RouteMode::Walkaround,
                    false,
                    blocker.or(Some(first)),
                ),
            }
        }
    }
}

/// Walk the pair's centre line round obstacles as one envelope, then lay
/// both halves along it; the longest clear prefix when the whole is not.
fn pair_walk(ctx: &Ctx<'_>, request: &PairStepRequest) -> (PairStep, bool, Option<Hit>) {
    let plain = pair::step(request);
    if request.via.is_some() {
        // A layer change keeps its straight legs; it is checked, and cut
        // back short of the vias when it collides.
        let Some(hit) = pair_conflict(ctx, request, &plain) else {
            return (plain, true, None);
        };
        let line = posture::path(
            request.head.center,
            request.to,
            request.posture,
            request.flipped,
        );
        return cut_back(ctx, request, &line, hit);
    }
    let envelope = 2.0 * request.style.width + request.style.gap;
    let counts = |_: &Hit| true;
    let walker = Walker {
        net: request.nets[0],
        own: &request.nets,
        layer: request.layer,
        width: envelope,
        posture: request.posture,
        counts: &counts,
    };
    // Candidates: the walk in either elbow sense, and one that first
    // leads straight out of the head for two pitches, so halves leaving
    // pads or vias fan out before the first bend.
    let center = request.head.center;
    let pitch = request.style.width + request.style.gap;
    let lead = lead_direction(request).map(|d| {
        [
            center[0] + d[0] * 2.0 * pitch,
            center[1] + d[1] * 2.0 * pitch,
        ]
    });
    let empty = State::default();
    let mut walks = Vec::new();
    for flip in [request.flipped, !request.flipped] {
        walks.push(walker.walk(ctx, &empty, center, request.to, flip));
    }
    if let Some(lead) = lead
        && walker.clear(ctx, &empty, &[center, lead])
    {
        let mut walk = walker.walk(ctx, &empty, lead, request.to, request.flipped);
        walk.points.insert(0, center);
        walk.points = clean::simplify(&walk.points);
        walks.push(walk);
    }
    let mut first = None;
    for walk in walks {
        let step = pair::step_along(
            &PairStepRequest {
                finish: if walk.reached { request.finish } else { None },
                ..request.clone()
            },
            &walk.points,
        );
        match pair_conflict(ctx, request, &step) {
            None if walk.reached => return (step, true, None),
            None => {
                if first.is_none() {
                    first = Some((walk, None, Some(step)));
                }
            }
            Some(hit) => {
                if first.is_none() {
                    first = Some((walk, Some(hit), None));
                }
            }
        }
    }
    match first {
        Some((walk, None, Some(step))) => (step, false, walk.blocker),
        Some((walk, Some(hit), _)) => cut_back(ctx, request, &walk.points, hit),
        _ => (
            PairStep {
                edits: RouteEditSet::default(),
                head: request.head,
                layer: request.layer,
            },
            false,
            None,
        ),
    }
}

/// The direction a pair leaves its head in: its coupled direction, else
/// square to its two ends, toward the target.
fn lead_direction(request: &PairStepRequest) -> Option<Point> {
    if let Some((d, _)) = request.head.coupled {
        return Some(d);
    }
    let [p, n] = request.head.ends;
    let across = [p[0] - n[0], p[1] - n[1]];
    let l = across[0].hypot(across[1]);
    if l < 1e-9 {
        return None;
    }
    let d = [-across[1] / l, across[0] / l];
    let c = request.head.center;
    let toward = (request.to[0] - c[0]) * d[0] + (request.to[1] - c[1]) * d[1];
    Some(if toward < 0.0 { [-d[0], -d[1]] } else { d })
}

/// The longest clear prefix of the centre line `line`, laid as a pair
/// step; the head stays when none is clear.
fn cut_back(
    ctx: &Ctx<'_>,
    request: &PairStepRequest,
    line: &[Point],
    hit: Hit,
) -> (PairStep, bool, Option<Hit>) {
    let build = |along: f64| {
        let prefix = clean::prefix(line, along);
        let step = pair::step_along(
            &PairStepRequest {
                finish: None,
                via: None,
                ..request.clone()
            },
            &prefix,
        );
        pair_conflict(ctx, request, &step).is_none().then_some(step)
    };
    match bisect(clean::length(line), build) {
        Some((_, step)) => (step, false, Some(hit)),
        // Nothing clear: no copper, the head where it was.
        None => (
            PairStep {
                edits: RouteEditSet::default(),
                head: request.head,
                layer: request.layer,
            },
            false,
            Some(hit),
        ),
    }
}
