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
//! (bisected, as a route step stops short). [`drag_plan`] is pure: the
//! desktop's preview and release, and `layout.track_drag` and
//! `layout.track_drag_preview` for CLI and MCP, all call it.
use super::world::{Ctx, Hit, Piece, State};
use super::{RouteIndex, RouteMode, RoutePlan, bisect, shove, shoved, yes};
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, distance};
use crate::ids::Id;
use crate::layout::connectivity::{Meets, TrackEnd, keeps_joint};
use crate::layout::posture::{MIN_SEGMENT_MM, Posture};
use crate::layout::{RouteEditSet, RouteRequest};
use crate::model::{Project, Track};
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
struct Grip<'a> {
    k: usize,
    track: &'a Track,
    /// Unit direction start → end and the left normal.
    dir: Point,
    normal: Point,
    ends: [EndHold<'a>; 2],
}

/// What holds an end of the dragged track.
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
struct Moved {
    edits: RouteEditSet,
    undo: RouteEditSet,
    hidden: Vec<usize>,
    pieces: Vec<Piece>,
    tracks: Vec<Id>,
}

fn moved(grip: &Grip<'_>, offset: f64, posture: Posture) -> Option<Moved> {
    let t = grip.track;
    let shift = add([0.0, 0.0], grip.normal, offset);
    let mut out = Moved {
        edits: RouteEditSet::default(),
        undo: RouteEditSet::default(),
        hidden: vec![grip.k],
        pieces: Vec::new(),
        tracks: vec![t.id],
    };
    let mut ends = [t.start, t.end];
    for (i, (p, hold)) in [t.start, t.end].into_iter().zip(&grip.ends).enumerate() {
        // Inward along the segment: + from the start, − from the end.
        let inward = if i == 0 { 1.0 } else { -1.0 };
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
                    .map(|meet| (*k, *n, *end, meet))
            }
            EndHold::Anchored => None,
            EndHold::Free => {
                ends[i] = on_line;
                continue;
            }
        };
        match stretched {
            Some((k, n, end, meet)) => {
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
                ends[i] = meet;
            }
            None => {
                let slide = match posture {
                    Posture::Orthogonal => 0.0,
                    Posture::Diagonal | Posture::Free => offset.abs(),
                };
                let joined = add(on_line, grip.dir, inward * slide);
                out.pieces
                    .push(Piece::track(t.net, t.layer, p, joined, t.width));
                out.edits.add.push(RouteRequest {
                    net: t.net,
                    start: p,
                    end: joined,
                    width: t.width,
                    layer: t.layer,
                });
                ends[i] = joined;
            }
        }
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

/// The overlay for the dragged copper at `offset` with everything it
/// pushes, or what stopped it.
fn attempt(
    ctx: &Ctx<'_>,
    grip: &Grip<'_>,
    request: &TrackDragRequest,
    offset: f64,
) -> std::result::Result<(Moved, State), Option<Hit>> {
    let copper = moved(grip, offset, request.posture).ok_or(None)?;
    let mut state = State {
        frozen: vec![grip.track.net],
        move_vias: request.move_vias,
        fixed: copper.pieces.clone(),
        hidden: copper.hidden.iter().copied().collect(),
        ..Default::default()
    };
    match request.mode {
        RouteMode::Shove => shove::shove(ctx, &mut state).map_err(Some)?,
        RouteMode::Walkaround => {
            let own = [grip.track.net];
            if let Some(hit) = state
                .fixed
                .iter()
                .find_map(|p| ctx.hits(&state, p, &own).into_iter().next())
            {
                return Err(Some(hit));
            }
        }
        RouteMode::Ignore => {}
    }
    Ok((copper, state))
}

/// What dragging `request.track` by `request.offset` proposes on
/// `project` (whose index is `index`, of the same revision): the plan's
/// `end` is the moved segment's midpoint, and `shoved` lists the dragged
/// and stretched tracks as well as other nets' pushed tracks. A drag that
/// cannot move at all proposes no edits and says why.
pub fn drag_plan(project: &Project, index: &RouteIndex, request: &TrackDragRequest) -> RoutePlan {
    let ctx = Ctx::new(project, index);
    let nothing = |why: String, at: Point, layer| RoutePlan {
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
    };
    let grip = match grip(&ctx, request.track) {
        Ok(grip) => grip,
        Err(why) => {
            let (at, layer) = project
                .tracks
                .iter()
                .find(|t| t.id == request.track)
                .map_or(([0.0, 0.0], crate::model::CopperLayer::Top), |t| {
                    (add(t.start, sub(t.end, t.start), 0.5), t.layer)
                });
            return nothing(why, at, layer);
        }
    };
    let t = grip.track;
    let middle = add(t.start, sub(t.end, t.start), 0.5);
    let (found, reached, blocker) = match attempt(&ctx, &grip, request, request.offset) {
        Ok(found) => (Some(found), true, None),
        Err(hit) => {
            let sign = request.offset.signum();
            let part = bisect(request.offset.abs(), |by| {
                attempt(&ctx, &grip, request, sign * by).ok()
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
        return nothing(why(&State::default()), middle, t.layer);
    };
    let pushed = shoved(&ctx, &state);
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
