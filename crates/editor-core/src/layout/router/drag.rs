//! Dragging an existing track segment with push-and-shove.
//!
//! The segment moves along its normal by the request's offset and keeps
//! its direction. At each end, a neighbour joined end to end (on the
//! copper graph, the only joint there) stretches along its own direction
//! to meet the moved segment, as long as it keeps its own joints; an end
//! nothing meets moves with the segment; any other end stays where it is
//! and joins the moved segment with a jog, at 45° to the segment (at 90°
//! in the orthogonal posture). The new copper
//! is the fixed copper of the same [`shove::shove`] a route step runs,
//! with the dragged net frozen and the copper it replaces hidden. When it
//! cannot make room, the segment stops at the last legal offset
//! (bisected, as a route step stops short). A differential pair's track
//! moves with its partner's coupled segment beside it, as one gesture
//! (see [`drag_plan`] for when the partner cannot follow), unless the
//! request moves that half alone ([`PairDrag::Alone`]). [`drag_plan`]
//! is pure: the
//! desktop's preview and release, and `layout.track_drag` and
//! `layout.track_drag_preview` for CLI and MCP, all call it.
use super::world::{Ctx, Hit, Piece, State};
use super::{RouteIndex, RouteMode, RoutePlan, bisect, shove, shoved, yes};
use crate::diagnostics::ObjectRef;
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, distance};
use crate::ids::Id;
use crate::layout::connectivity::{CopperGraph, Meets, TrackEnd, keeps_joint};
use crate::layout::length;
use crate::layout::pair::{left, mitre};
use crate::layout::posture::{MIN_SEGMENT_MM, Posture};
use crate::layout::{RouteEditSet, RouteRequest, apply_routes};
use crate::model::{CopperLayer, Project, Track};
use crate::rules::signal::DiffPair;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The farthest one drag moves a segment, mm.
pub const MAX_DRAG_MM: f64 = 1000.0;

/// `layout.track_drag`: move board track `track` along its normal by
/// `offset` mm (positive to the left of start → end), its neighbours on
/// the net following.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackDragRequest {
    pub track: Id,
    pub offset: f64,
    /// The jog an end that cannot stretch a neighbour takes: 45° to the
    /// segment, or 90° when orthogonal.
    #[serde(default)]
    pub posture: Posture,
    #[serde(default)]
    pub mode: RouteMode,
    /// Whether a shove may move vias (locked vias never move).
    #[serde(default = "yes")]
    pub move_vias: bool,
    /// For a differential pair's track: move its partner with it (the
    /// default), or move this half alone (the desktop's ⌥-drag).
    #[serde(default, skip_serializing_if = "PairDrag::is_together")]
    pub pair: PairDrag,
}

/// What a drag of one half of a differential pair does with the other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PairDrag {
    /// The partner's coupled segment moves beside it, when it can.
    #[default]
    Together,
    /// This half moves alone, with the same shove or walkaround, and the
    /// partner stays where it is (it is not shoved). The pair findings
    /// this may cause are the caller's explicit choice.
    Alone,
}

impl PairDrag {
    fn is_together(&self) -> bool {
        *self == Self::Together
    }
}

impl TrackDragRequest {
    /// Refuses an offset that is not a finite distance within
    /// [`MAX_DRAG_MM`].
    pub fn check(&self) -> Result<()> {
        if self.offset.is_finite() && self.offset.abs() <= MAX_DRAG_MM {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "track_drag.offset: a distance within ±{MAX_DRAG_MM} mm, not {}",
                self.offset
            )))
        }
    }
}

/// The dragged track and what each of its ends does.
#[derive(Clone)]
struct Grip<'a> {
    k: usize,
    track: &'a Track,
    /// Unit direction start → end and the left normal.
    dir: Point,
    normal: Point,
    ends: [EndHold<'a>; 2],
}

/// What holds an end of the dragged track.
#[derive(Clone)]
enum EndHold<'a> {
    /// A neighbour joined end to end that may stretch: its obstacle
    /// index, the track, which of its ends meets the dragged one, and its
    /// mid-span joints.
    Neighbour {
        k: usize,
        track: &'a Track,
        end: TrackEnd,
        pins: Vec<Point>,
    },
    /// Anything else (a pad, a via, several tracks, a locked track):
    /// the end stays and a jog joins it.
    Anchored,
    /// Nothing meets the end: it moves with the segment.
    Free,
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: Point, b: Point, s: f64) -> Point {
    [a[0] + b[0] * s, a[1] + b[1] * s]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn end_of(track: &Track, end: TrackEnd) -> (Point, Point) {
    match end {
        TrackEnd::Start => (track.start, track.end),
        TrackEnd::End => (track.end, track.start),
    }
}

fn grip<'a>(ctx: &Ctx<'a>, track: Id) -> std::result::Result<Grip<'a>, String> {
    let k = ctx
        .project
        .tracks
        .iter()
        .position(|t| t.id == track)
        .ok_or_else(|| format!("No track {track}"))?;
    let t = ctx
        .base_track(k)
        .ok_or_else(|| format!("No track {track}"))?;
    if t.locked {
        return Err("The track is locked".into());
    }
    let length = distance(t.start, t.end);
    if length < MIN_SEGMENT_MM {
        return Err("The track is too short to drag".into());
    }
    let dir = [
        (t.end[0] - t.start[0]) / length,
        (t.end[1] - t.start[1]) / length,
    ];
    let graph = ctx.graph();
    let joints = graph.joints_on(k);
    if joints
        .iter()
        .any(|j| j.end.is_none() && j.meets != Meets::Fill)
    {
        return Err("Copper joins the track in mid-span, which holds it in place".into());
    }
    let hold = |end: TrackEnd| -> EndHold<'a> {
        let mut at = joints
            .iter()
            .filter(|j| j.end == Some(end) && j.meets != Meets::Fill);
        let joint = match (at.next(), at.next()) {
            (None, _) => return EndHold::Free,
            (Some(joint), None) => joint,
            _ => return EndHold::Anchored,
        };
        let Meets::Track {
            index: other,
            end: Some(back),
            ..
        } = joint.meets
        else {
            return EndHold::Anchored;
        };
        let back_only = {
            let mut there = graph
                .joints_on(other)
                .into_iter()
                .filter(|j| j.end == Some(back) && j.meets != Meets::Fill);
            matches!(
                (there.next(), there.next()),
                (Some(j), None) if matches!(j.meets, Meets::Track { index, .. } if index == k)
            )
        };
        match ctx.base_track(other) {
            Some(n) if joint.end_to_end && back_only && !n.locked && n.layer == t.layer => {
                let pins = graph
                    .joints_on(other)
                    .into_iter()
                    .filter(|j| j.end.is_none() && j.meets != Meets::Fill)
                    .map(|j| j.at)
                    .collect();
                EndHold::Neighbour {
                    k: other,
                    track: n,
                    end: back,
                    pins,
                }
            }
            _ => EndHold::Anchored,
        }
    };
    Ok(Grip {
        k,
        track: t,
        dir,
        normal: [-dir[1], dir[0]],
        ends: [hold(TrackEnd::Start), hold(TrackEnd::End)],
    })
}

/// The dragged net's copper at one offset: the edits (the dragged track
/// and stretched neighbours replaced, jogs added), the copper they hide
/// and the new copper as pieces.
#[derive(Default)]
struct Moved {
    edits: RouteEditSet,
    undo: RouteEditSet,
    hidden: Vec<usize>,
    pieces: Vec<Piece>,
    tracks: Vec<Id>,
}

impl Moved {
    fn extend(&mut self, other: Moved) {
        self.edits.replace.extend(other.edits.replace);
        self.edits.add.extend(other.edits.add);
        self.undo.replace.extend(other.undo.replace);
        self.hidden.extend(other.hidden);
        self.pieces.extend(other.pieces);
        self.tracks.extend(other.tracks);
    }
}

/// What one end of the moved segment does.
enum Shape<'a> {
    /// Nothing meets it: it moves to this point.
    Free(Point),
    /// The neighbour `track` (obstacle index `k`) stretches its `end` to
    /// `meet`.
    Stretch {
        k: usize,
        track: &'a Track,
        end: TrackEnd,
        meet: Point,
    },
    /// The end stays at `from`, runs straight on to `corner` (a pair's
    /// mitre; `from` itself for one track) and jogs to `to`.
    Jog {
        from: Point,
        corner: Point,
        to: Point,
    },
}

/// Inward along the segment from end `i`: + from the start, − from the
/// end.
fn inward(i: usize) -> f64 {
    if i == 0 { 1.0 } else { -1.0 }
}

/// How far a jog slides along the segment for an offset.
fn slide(offset: f64, posture: Posture) -> f64 {
    match posture {
        Posture::Orthogonal => 0.0,
        Posture::Diagonal | Posture::Free => offset.abs(),
    }
}

/// What each end of `grip`'s segment does when it moves by `shift`.
fn shapes<'a>(grip: &Grip<'a>, shift: Point, posture: Posture) -> [Shape<'a>; 2] {
    let t = grip.track;
    let offset = dot(shift, grip.normal);
    [0, 1].map(|i| {
        let (p, hold) = if i == 0 {
            (t.start, &grip.ends[0])
        } else {
            (t.end, &grip.ends[1])
        };
        let on_line = add(p, shift, 1.0);
        let stretched = match hold {
            EndHold::Neighbour {
                k,
                track: n,
                end,
                pins,
            } => {
                let (near, far) = end_of(n, *end);
                let along = sub(near, far);
                let denom = cross(along, grip.dir);
                // The neighbour's line meets the moved segment's line at
                // far + s·along, with s = cross(on_line − far, dir) / denom.
                (denom.abs() > 1e-9 * distance(near, far).max(1.0))
                    .then(|| cross(sub(on_line, far), grip.dir) / denom)
                    .map(|s| add(far, along, s))
                    .filter(|meet| {
                        dot(sub(*meet, far), along) > 0.0
                            && distance(*meet, far) >= MIN_SEGMENT_MM
                            && pins.iter().all(|pin| keeps_joint(&[far, *meet], *pin))
                    })
                    .map(|meet| Shape::Stretch {
                        k: *k,
                        track: n,
                        end: *end,
                        meet,
                    })
            }
            EndHold::Anchored => None,
            EndHold::Free => return Shape::Free(on_line),
        };
        stretched.unwrap_or_else(|| Shape::Jog {
            from: p,
            corner: p,
            to: add(on_line, grip.dir, inward(i) * slide(offset, posture)),
        })
    })
}

/// A pair's matching ends that both jog, mitred so the two jogs run at
/// the pair's pitch as one bend of the centre line: the jog of the
/// centre line between the halves' ends, each half offset from it with
/// mitred corners, starting where neither half has to run back. `None`
/// when a half's jog would be too short at this offset.
fn mitre_jogs(
    grips: [&Grip<'_>; 2],
    shapes: &mut [[Shape<'_>; 2]; 2],
    shift: Point,
    posture: Posture,
) -> Option<()> {
    let [a, b] = grips;
    let points = |g: &Grip<'_>| [g.track.start, g.track.end];
    let offset = dot(shift, a.normal);
    for i in 0..2 {
        let pa = points(a)[i];
        let j = if distance(points(b)[0], pa) <= distance(points(b)[1], pa) {
            0
        } else {
            1
        };
        let pb = points(b)[j];
        let jogs =
            matches!(shapes[0][i], Shape::Jog { .. }) && matches!(shapes[1][j], Shape::Jog { .. });
        if !jogs {
            continue;
        }
        // The path runs along `t` into the jog `jd`, then along `t` again.
        let t = [a.dir[0] * inward(i), a.dir[1] * inward(i)];
        let run = slide(offset, posture);
        let to = add(shift, t, run);
        let length = distance([0.0, 0.0], to);
        if length < 1e-12 {
            return None;
        }
        let jd = [to[0] / length, to[1] / length];
        let (into, out) = (mitre(t, jd), mitre(jd, t));
        let middle = add(pa, sub(pb, pa), 0.5);
        let lateral = |p: Point| dot(sub(p, middle), left(t));
        let behind = |p: Point| dot(sub(p, middle), t) - lateral(p) * dot(into, t);
        let s = behind(pa).max(behind(pb));
        let centre = add(middle, t, s);
        for (side, p, end) in [(0, pa, i), (1, pb, j)] {
            let e = lateral(p);
            let corner = add(centre, into, e);
            let joined = add(add(add(centre, shift, 1.0), t, run), out, e);
            if dot(sub(joined, corner), jd) < MIN_SEGMENT_MM {
                return None;
            }
            if let Some(slot) = shapes.get_mut(side).and_then(|s| s.get_mut(end)) {
                *slot = Shape::Jog {
                    from: p,
                    corner,
                    to: joined,
                };
            }
        }
    }
    Some(())
}

/// `grip`'s copper with its ends shaped as `shapes`.
fn moved(grip: &Grip<'_>, shapes: [Shape<'_>; 2]) -> Option<Moved> {
    let t = grip.track;
    let mut out = Moved {
        hidden: vec![grip.k],
        tracks: vec![t.id],
        ..Default::default()
    };
    let jog = |out: &mut Moved, a: Point, b: Point| {
        out.pieces.push(Piece::track(t.net, t.layer, a, b, t.width));
        out.edits.add.push(RouteRequest {
            net: t.net,
            start: a,
            end: b,
            width: t.width,
            layer: t.layer,
        });
    };
    let mut ends = [t.start, t.end];
    for (slot, shape) in ends.iter_mut().zip(shapes) {
        *slot = match shape {
            Shape::Free(at) => at,
            Shape::Stretch {
                k,
                track: n,
                end,
                meet,
            } => {
                let mut replaced = n.clone();
                match end {
                    TrackEnd::Start => replaced.start = meet,
                    TrackEnd::End => replaced.end = meet,
                }
                out.pieces.push(Piece::track(
                    n.net,
                    n.layer,
                    replaced.start,
                    replaced.end,
                    n.width,
                ));
                out.edits.replace.push(replaced);
                out.undo.replace.push(n.clone());
                out.hidden.push(k);
                out.tracks.push(n.id);
                meet
            }
            Shape::Jog { from, corner, to } => {
                if distance(from, corner) > 1e-9 {
                    jog(&mut out, from, corner);
                }
                jog(&mut out, corner, to);
                to
            }
        };
    }
    // The moved segment must keep its direction and a length.
    if dot(sub(ends[1], ends[0]), grip.dir) < MIN_SEGMENT_MM {
        return None;
    }
    out.pieces
        .push(Piece::track(t.net, t.layer, ends[0], ends[1], t.width));
    out.edits.replace.insert(
        0,
        Track {
            start: ends[0],
            end: ends[1],
            ..t.clone()
        },
    );
    out.undo.replace.insert(0, t.clone());
    Some(out)
}

/// The copper of `grips` (the dragged track first, then its pair partner
/// beside it) moved by `offset` along the dragged track's normal.
fn moved_all(grips: &[Grip<'_>], offset: f64, posture: Posture) -> Option<Moved> {
    let first = grips.first()?;
    let shift = add([0.0, 0.0], first.normal, offset);
    match grips {
        [one] => moved(one, shapes(one, shift, posture)),
        [a, b] => {
            let mut both = [shapes(a, shift, posture), shapes(b, shift, posture)];
            mitre_jogs([a, b], &mut both, shift, posture)?;
            let [sa, sb] = both;
            let mut out = moved(a, sa)?;
            out.extend(moved(b, sb)?);
            Some(out)
        }
        _ => None,
    }
}

/// The overlay for the dragged copper at `offset` with everything it
/// pushes, or what stopped it.
fn attempt(
    ctx: &Ctx<'_>,
    grips: &[Grip<'_>],
    request: &TrackDragRequest,
    offset: f64,
) -> std::result::Result<(Moved, State), Option<Hit>> {
    let copper = moved_all(grips, offset, request.posture).ok_or(None)?;
    let mut frozen: Vec<Id> = grips.iter().map(|g| g.track.net).collect();
    // A half moved alone leaves its partner where it is.
    if request.pair == PairDrag::Alone {
        frozen.extend(grips.iter().filter_map(|g| {
            let net = g.track.net;
            ctx.project.rules.pair_of(net)?.partner(net)
        }));
    }
    let mut state = State {
        frozen,
        move_vias: request.move_vias,
        fixed: copper.pieces.clone(),
        hidden: copper.hidden.iter().copied().collect(),
        ..Default::default()
    };
    match request.mode {
        RouteMode::Shove => shove::shove(ctx, &mut state).map_err(Some)?,
        RouteMode::Walkaround => {
            if let Some(hit) = state
                .fixed
                .iter()
                .find_map(|p| ctx.hits(&state, p, &[p.net]).into_iter().next())
            {
                return Err(Some(hit));
            }
        }
        RouteMode::Ignore => {}
    }
    Ok((copper, state))
}

/// A plan that moves nothing and says why.
fn refused(request: &TrackDragRequest, why: String, at: Point, layer: CopperLayer) -> RoutePlan {
    RoutePlan {
        edits: RouteEditSet::default(),
        end: at,
        end_layer: layer,
        reached: false,
        via_placed: false,
        used: request.mode,
        shoved: Vec::new(),
        moved_vias: Vec::new(),
        undo: RouteEditSet::default(),
        blocked: Some(why),
        blocker: None,
        detour_factor: 1.0,
        note: None,
    }
}

/// What dragging `request.track` by `request.offset` proposes on
/// `project` (whose index is `index`, of the same revision): the plan's
/// `end` is the moved segment's midpoint, and `shoved` lists the dragged
/// and stretched tracks as well as other nets' pushed tracks. A drag that
/// cannot move at all proposes no edits and says why.
///
/// A track of a differential pair whose partner runs coupled beside it
/// moves with that partner segment, by the same shift: both halves'
/// neighbours stretch, and ends that jog are mitred as one bend, so the
/// pair keeps its gap and width. When the partner cannot follow (locked,
/// held in mid-span, not coupled there, or no room for both), the track
/// moves alone only if the pair gains no skew, uncoupled-length or gap
/// finding; otherwise the drag is refused naming the pair. With
/// [`PairDrag::Alone`] the track moves alone whatever that does to the pair.
pub fn drag_plan(project: &Project, index: &RouteIndex, request: &TrackDragRequest) -> RoutePlan {
    let ctx = Ctx::new(project, index);
    let one = match grip(&ctx, request.track) {
        Ok(grip) => grip,
        Err(why) => {
            let (at, layer) = project
                .tracks
                .iter()
                .find(|t| t.id == request.track)
                .map_or(([0.0, 0.0], CopperLayer::Top), |t| {
                    (add(t.start, sub(t.end, t.start), 0.5), t.layer)
                });
            return refused(request, why, at, layer);
        }
    };
    let t = one.track;
    let Some(pair) = project
        .rules
        .pair_of(t.net)
        .filter(|_| request.pair == PairDrag::Together)
    else {
        return plan_with(&ctx, request, &[one]);
    };
    let partner = coupled_partner(project, pair, t.id)
        .ok_or_else(|| "its track here does not run coupled".to_owned())
        .and_then(|id| grip(&ctx, id));
    let reason = match partner {
        Ok(other) => {
            let plan = plan_with(&ctx, request, &[one.clone(), other]);
            if !plan.edits.replace.is_empty() {
                match new_pair_finding(&ctx, &plan.edits, pair) {
                    None => return plan,
                    Some(finding) => finding,
                }
            } else {
                plan.blocked
                    .unwrap_or_else(|| "no room to move both halves".into())
            }
        }
        Err(why) => why,
    };
    let alone = plan_with(&ctx, request, &[one]);
    if alone.edits.replace.is_empty() {
        return alone;
    }
    match new_pair_finding(&ctx, &alone.edits, pair) {
        None => alone,
        Some(finding) => {
            let partner = pair.partner(t.net).map_or("", |n| project.net_name(n));
            let reason = lower_first(&reason);
            // The finding names the pair already ("USB: …").
            let finding = finding
                .strip_prefix(&format!("{}: ", pair.name))
                .unwrap_or(&finding);
            refused(
                request,
                format!(
                    "{partner} can't follow ({reason}), and moving {} alone breaks pair {}: {finding}",
                    project.net_name(t.net),
                    pair.name
                ),
                add(t.start, sub(t.end, t.start), 0.5),
                t.layer,
            )
        }
    }
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_lowercase().chain(chars).collect::<String>()
    })
}

/// The partner half's track that runs coupled beside `track`.
fn coupled_partner(project: &Project, pair: &DiffPair, track: Id) -> Option<Id> {
    length::coupling(project, pair)
        .coupled_runs
        .iter()
        .find_map(|(p, n, ..)| {
            if *p == track {
                Some(*n)
            } else if *n == track {
                Some(*p)
            } else {
                None
            }
        })
}

/// The pair's skew, uncoupled-length and gap findings, by code.
fn pair_findings(project: &Project, graph: &CopperGraph, pair: &DiffPair) -> Vec<(String, String)> {
    let net = ObjectRef::Net { id: pair.positive };
    length::checks(project, graph)
        .into_iter()
        .filter(|f| {
            ["pair.skew", "pair.uncoupled", "pair.gap"].contains(&f.code.as_str())
                && f.objects.contains(&net)
        })
        .map(|f| (f.code, f.message))
        .collect()
}

/// The first pair finding `edits` add on the board (one it did not have),
/// or a refusal when they cannot be applied.
fn new_pair_finding(ctx: &Ctx<'_>, edits: &RouteEditSet, pair: &DiffPair) -> Option<String> {
    let mut next = ctx.project.clone();
    if let Err(e) = apply_routes(&mut next, edits.clone()) {
        return Some(e.to_string());
    }
    let after = pair_findings(&next, &CopperGraph::of(&next), pair);
    if after.is_empty() {
        return None;
    }
    let before = pair_findings(ctx.project, ctx.graph(), pair);
    after
        .into_iter()
        .find(|(code, _)| !before.iter().any(|(c, _)| c == code))
        .map(|(_, message)| message)
}

/// The plan for moving `grips` (the dragged track first) together.
fn plan_with(ctx: &Ctx<'_>, request: &TrackDragRequest, grips: &[Grip<'_>]) -> RoutePlan {
    let Some(first) = grips.first() else {
        return refused(
            request,
            "Nothing to drag".into(),
            [0.0, 0.0],
            CopperLayer::Top,
        );
    };
    let t = first.track;
    let middle = add(t.start, sub(t.end, t.start), 0.5);
    let (found, reached, blocker) = match attempt(ctx, grips, request, request.offset) {
        Ok(found) => (Some(found), true, None),
        Err(hit) => {
            let sign = request.offset.signum();
            let part = bisect(request.offset.abs(), |by| {
                attempt(ctx, grips, request, sign * by).ok()
            });
            (part.map(|(_, found)| found), false, hit)
        }
    };
    let why = |state: &State| {
        ctx.held_note(state).unwrap_or_else(|| {
            blocker.map_or_else(
                || "No room to drag the track there".to_owned(),
                |h| ctx.describe(&h),
            )
        })
    };
    let Some((copper, state)) = found else {
        return refused(request, why(&State::default()), middle, t.layer);
    };
    let pushed = shoved(ctx, &state);
    let mut edits = copper.edits;
    edits.add.extend(pushed.edits.add);
    edits.replace.extend(pushed.edits.replace);
    edits.remove.extend(pushed.edits.remove);
    edits.move_vias.extend(pushed.edits.move_vias);
    let mut undo = copper.undo;
    undo.replace.extend(pushed.undo.replace);
    undo.restore.extend(pushed.undo.restore);
    undo.move_vias.extend(pushed.undo.move_vias);
    let mut shoved_tracks = copper.tracks;
    shoved_tracks.extend(pushed.tracks);
    let end = edits
        .replace
        .first()
        .map_or(middle, |m| add(m.start, sub(m.end, m.start), 0.5));
    RoutePlan {
        edits,
        end,
        end_layer: t.layer,
        reached,
        via_placed: false,
        used: request.mode,
        shoved: shoved_tracks,
        moved_vias: pushed.vias,
        undo,
        blocked: (!reached).then(|| why(&state)),
        blocker: None,
        detour_factor: 1.0,
        note: ctx.held_note(&state),
    }
}

/// How far `plan` moved the dragged segment, `track` as it was, along its
/// normal, mm.
pub fn dragged_by(track: &Track, plan: &RoutePlan) -> f64 {
    let length = distance(track.start, track.end);
    if length <= 0.0 {
        return 0.0;
    }
    let normal = [
        -(track.end[1] - track.start[1]) / length,
        (track.end[0] - track.start[0]) / length,
    ];
    let middle = add(track.start, sub(track.end, track.start), 0.5);
    dot(sub(plan.end, middle), normal)
}
