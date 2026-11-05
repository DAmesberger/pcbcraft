//! Walkaround, shove, spring-back, cleanup and the transaction, on the
//! demo board (60 × 40 mm; R1 pads at x 7.2 and 8.8, R2 pads at 15.2 and
//! 16.8, all at y 8, 1.0 × 1.4 mm; nets 1, 2 and 4) with original
//! obstacle fixtures. Every proposal is applied through the engine's
//! `layout.route_apply`, whose clearance checks are the authority, and
//! the board checks must find nothing new.
use super::*;
use crate::geometry::segment_distance;
use crate::layout::{self, LayoutCommand, apply};
use crate::model::CopperLayer::Top;
use crate::{Engine, Operation, Query, QueryResult};

mod pair_shove;
mod perf;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Demo nets.
const A: Id = 1;
const B: Id = 2;
const C: Id = 4;
const WIDTH: f64 = 0.3;
const CLEARANCE: f64 = 0.2;

fn board() -> Fallible<Project> {
    Ok(Engine::demo()?.project().clone())
}

fn track(p: &mut Project, net: Id, a: Point, b: Point) -> Fallible<Id> {
    apply(
        p,
        LayoutCommand::Route {
            request: RouteRequest {
                net,
                start: a,
                end: b,
                width: WIDTH,
                layer: Top,
            },
        },
    )?;
    Ok(p.tracks.last().map(|t| t.id).ok_or("no track")?)
}

fn via(p: &mut Project, net: Id, at: Point) -> Fallible<Id> {
    apply(
        p,
        LayoutCommand::AddVia {
            via: ViaRequest::through(net, at),
        },
    )?;
    Ok(p.vias.last().map(|v| v.id).ok_or("no via")?)
}

fn request(net: Id, head: Point, to: Point, mode: RouteMode) -> RouteStepRequest {
    RouteStepRequest {
        net,
        width: WIDTH,
        layer: Top,
        head,
        to,
        posture: Posture::Diagonal,
        flipped: false,
        via: None,
        via_layer: None,
        mode,
        move_vias: true,
    }
}

fn run(p: &Project, r: &RouteStepRequest) -> RoutePlan {
    plan(p, &RouteIndex::of(p), r)
}

/// Copper findings the router must never cause.
fn violations(p: &Project) -> Vec<String> {
    layout::checks(p)
        .into_iter()
        .filter(|f| {
            matches!(
                f.code.as_str(),
                "board.clearance"
                    | "board.outside_outline"
                    | "board.edge_clearance"
                    | "keepout.track"
                    | "keepout.via"
            )
        })
        .map(|f| f.message)
        .collect()
}

/// Apply the plan as the desktop does; the board stays clean.
fn commit(p: &mut Project, plan: &RoutePlan) -> Fallible {
    let before = violations(p).len();
    apply(
        p,
        LayoutCommand::ApplyRoute {
            edits: plan.edits.clone(),
        },
    )?;
    let after = violations(p);
    if after.len() > before {
        return Err(format!("new violations: {after:?}").into());
    }
    Ok(())
}

/// The step's own polyline (its added tracks of `net`, in order).
fn head_points(plan: &RoutePlan, net: Id) -> Vec<Point> {
    let mut points = Vec::new();
    for r in plan.edits.add.iter().filter(|r| r.net == net) {
        if points.is_empty() {
            points.push(r.start);
        }
        points.push(r.end);
    }
    points
}

fn angles_ok(points: &[Point]) -> bool {
    points.windows(2).all(|w| {
        let a = (w[1][1] - w[0][1])
            .atan2(w[1][0] - w[0][0])
            .to_degrees()
            .rem_euclid(45.0);
        a < 1e-6 || 45.0 - a < 1e-6
    })
}

fn close(a: Point, b: Point) -> bool {
    distance(a, b) < 1e-6
}

/// The least edge gap from a track polyline of `width` to a circle.
fn gap_to_circle(points: &[Point], width: f64, c: Point, r: f64) -> f64 {
    points
        .windows(2)
        .map(|w| segment_distance(c, w[0], w[1]) - width / 2.0 - r)
        .fold(f64::INFINITY, f64::min)
}

/// A shoved chain's new points, in chain order, from the plan's edits.
fn chain(p: &Project, plan: &RoutePlan, net: Id) -> Vec<Point> {
    let mut segs: Vec<(Point, Point)> = plan
        .edits
        .replace
        .iter()
        .filter(|t| t.net == net)
        .map(|t| (t.start, t.end))
        .chain(
            plan.edits
                .add
                .iter()
                .filter(|r| r.net == net)
                .map(|r| (r.start, r.end)),
        )
        .collect();
    // Untouched tracks of the net join the chain too.
    for t in p.tracks.iter().filter(|t| t.net == net) {
        if !plan.shoved.contains(&t.id) {
            segs.push((t.start, t.end));
        }
    }
    let Some(first) = segs.first().copied() else {
        return Vec::new();
    };
    segs.remove(0);
    let mut points = vec![first.0, first.1];
    while !segs.is_empty() {
        let head = points.first().copied().unwrap_or_default();
        let tail = points.last().copied().unwrap_or_default();
        let Some(k) = segs.iter().position(|(a, b)| {
            close(*a, tail) || close(*b, tail) || close(*a, head) || close(*b, head)
        }) else {
            break;
        };
        let (a, b) = segs.remove(k);
        if close(a, tail) {
            points.push(b);
        } else if close(b, tail) {
            points.push(a);
        } else if close(b, head) {
            points.insert(0, a);
        } else {
            points.insert(0, b);
        }
    }
    points
}

/// The head goes round a via of another net, hugging it at the clearance
/// in 45° posture, and reaches the target; the engine takes it.
#[test]
fn walkaround_goes_round_a_via() -> Fallible {
    let mut p = board()?;
    via(&mut p, A, [30.0, 20.0])?;
    let plan = run(
        &p,
        &request(B, [25.0, 20.0], [35.0, 20.0], RouteMode::Walkaround),
    );
    assert!(plan.reached, "{:?}", plan.blocked);
    assert_eq!(plan.used, RouteMode::Walkaround);
    let points = head_points(&plan, B);
    assert!(close(*points.last().ok_or("empty")?, [35.0, 20.0]));
    assert!(angles_ok(&points), "{points:?}");
    // Hugging: the nearest edge gap is the clearance, within the margin.
    let gap = gap_to_circle(&points, WIDTH, [30.0, 20.0], 0.3);
    assert!((CLEARANCE..CLEARANCE + 0.05).contains(&gap), "gap {gap}");
    assert!(clean::length(&points) < 11.0, "no long detour: {points:?}");
    commit(&mut p, &plan)
}

/// A track across the way: the head goes round its nearer end.
#[test]
fn walkaround_goes_round_a_track() -> Fallible {
    let mut p = board()?;
    track(&mut p, A, [30.0, 17.0], [30.0, 25.0])?;
    let plan = run(
        &p,
        &request(B, [25.0, 20.0], [35.0, 20.0], RouteMode::Walkaround),
    );
    assert!(plan.reached, "{:?}", plan.blocked);
    let points = head_points(&plan, B);
    assert!(angles_ok(&points));
    // Round the lower end (3 mm away) rather than the upper (5 mm).
    let low = points.iter().map(|q| q[1]).fold(f64::INFINITY, f64::min);
    assert!(low < 17.0 - 0.35 + 1e-6 && low > 16.0, "{points:?}");
    commit(&mut p, &plan)
}

/// From left of R1 to R2's pad of net 4, the head passes R1's pads and
/// R2's other pad at the clearance.
#[test]
fn walkaround_goes_round_pads() -> Fallible {
    let mut p = board()?;
    let plan = run(
        &p,
        &request(C, [3.0, 8.0], [16.8, 8.0], RouteMode::Walkaround),
    );
    assert!(plan.reached, "{:?}", plan.blocked);
    let points = head_points(&plan, C);
    assert!(angles_ok(&points));
    // Past R1 (x 8) at the pads' 0.7 mm half-height plus the clearance
    // and half the width: hugging, not wandering.
    let past = points
        .windows(2)
        .find(|w| w[0][0].min(w[1][0]) <= 8.0 && w[0][0].max(w[1][0]) >= 8.0)
        .ok_or("no leg past R1")?;
    let t = (8.0 - past[0][0]) / (past[1][0] - past[0][0]);
    let y = past[0][1] + (past[1][1] - past[0][1]) * t;
    let need = 0.7 + CLEARANCE + WIDTH / 2.0;
    assert!(((y - 8.0).abs() - need).abs() < 0.01, "{points:?}");
    commit(&mut p, &plan)
}

/// A keepout and a cutout-free board edge: the head goes round the
/// keepout; a target beyond the board edge stops at the last legal point.
#[test]
fn walkaround_respects_keepouts_and_the_board_edge() -> Fallible {
    use crate::library::{Outline, Segment};
    use crate::model::fill::{KeepoutLayers, KeepoutRules, KeepoutSpec};
    let mut p = board()?;
    let square = Outline {
        start: [29.0, 19.0],
        segments: vec![
            Segment::Line { to: [31.0, 19.0] },
            Segment::Line { to: [31.0, 21.0] },
            Segment::Line { to: [29.0, 21.0] },
            Segment::Line { to: [29.0, 19.0] },
        ],
    };
    layout::fill::apply(
        &mut p,
        layout::fill::FillCommand::AddKeepout {
            keepout: Box::new(KeepoutSpec {
                name: "K".into(),
                outline: square,
                layers: KeepoutLayers::All,
                forbid: KeepoutRules {
                    fills: true,
                    tracks: true,
                    vias: true,
                    pads: false,
                },
            }),
        },
    )?;
    let plan = run(
        &p,
        &request(B, [25.0, 20.0], [35.0, 20.0], RouteMode::Walkaround),
    );
    assert!(plan.reached, "{:?}", plan.blocked);
    commit(&mut p, &plan)?;
    // Off the board: stop inside, never outside.
    let plan = run(
        &p,
        &request(B, [50.0, 30.0], [70.0, 30.0], RouteMode::Walkaround),
    );
    assert!(!plan.reached);
    assert!(plan.end[0] <= 60.0 - WIDTH / 2.0 + 1e-6 && plan.end[0] > 59.0);
    assert!(plan.blocked.is_some());
    commit(&mut p, &plan)
}

/// A head parallel to a track and too close pushes it aside: the track
/// bends round the head at the clearance with 45° jogs and keeps its
/// identity and ends.
#[test]
fn shove_pushes_one_track() -> Fallible {
    let mut p = board()?;
    let id = track(&mut p, A, [20.0, 20.0], [40.0, 20.0])?;
    let plan = run(
        &p,
        &request(B, [25.0, 19.6], [35.0, 19.6], RouteMode::Shove),
    );
    assert!(plan.reached && plan.used == RouteMode::Shove, "{plan:?}");
    assert_eq!(plan.shoved, vec![id]);
    assert_eq!(head_points(&plan, B), vec![[25.0, 19.6], [35.0, 19.6]]);
    assert!(plan.edits.replace.iter().any(|t| t.id == id));
    let moved = chain(&p, &plan, A);
    assert!(close(moved[0], [20.0, 20.0]) && close(*moved.last().ok_or("")?, [40.0, 20.0]));
    assert!(angles_ok(&moved) && !clean::has_acute(&moved), "{moved:?}");
    // Pushed to the clearance: centre lines 0.5 mm apart over the head.
    let top = moved.iter().map(|q| q[1]).fold(0.0, f64::max);
    assert!((top - (19.6 + WIDTH + CLEARANCE)).abs() < 0.01, "{moved:?}");
    commit(&mut p, &plan)
}

/// Three parallel tracks: the head pushes the first, which pushes the
/// second, which pushes the third.
#[test]
fn shove_pushes_a_chain_of_three_tracks() -> Fallible {
    let mut p = board()?;
    let ids = [
        track(&mut p, A, [20.0, 20.0], [40.0, 20.0])?,
        track(&mut p, C, [20.0, 20.5], [40.0, 20.5])?,
        track(&mut p, A, [20.0, 21.0], [40.0, 21.0])?,
    ];
    let plan = run(
        &p,
        &request(B, [25.0, 19.6], [35.0, 19.6], RouteMode::Shove),
    );
    assert!(plan.reached && plan.used == RouteMode::Shove, "{plan:?}");
    for id in ids {
        assert!(
            plan.shoved.contains(&id),
            "{id} not shoved: {:?}",
            plan.shoved
        );
    }
    commit(&mut p, &plan)?;
    for t in &p.tracks {
        assert!(angles_ok(&[t.start, t.end]), "{t:?}");
    }
    Ok(())
}

/// A via in the way moves the least distance at 45° multiples; a track
/// ending on it follows with an elbow. With via shoving off it stays.
#[test]
fn shove_moves_a_via_and_its_track_follows() -> Fallible {
    let mut p = board()?;
    let v = via(&mut p, A, [30.0, 20.2])?;
    let t = track(&mut p, A, [30.0, 20.2], [30.0, 26.0])?;
    let plan = run(
        &p,
        &request(B, [25.0, 19.6], [35.0, 19.6], RouteMode::Shove),
    );
    assert!(plan.reached && plan.used == RouteMode::Shove, "{plan:?}");
    assert_eq!(plan.moved_vias, vec![v]);
    assert!(plan.shoved.contains(&t));
    let moved = plan.edits.move_vias.first().ok_or("no move")?;
    assert!(
        (moved.position[0] - 30.0).abs() < 1e-9,
        "straight up: {moved:?}"
    );
    assert!(moved.position[1] >= 19.6 + 0.15 + CLEARANCE + 0.3 - 1e-6);
    commit(&mut p, &plan)?;
    let at = p.via(v).ok_or("via")?.position;
    assert!(
        p.tracks
            .iter()
            .any(|x| x.net == A && (close(x.start, at) || close(x.end, at)))
    );
    // Not allowed to move: the via stays and the head walks round.
    let mut q = board()?;
    let v = via(&mut q, A, [30.0, 20.2])?;
    let r = RouteStepRequest {
        move_vias: false,
        ..request(B, [25.0, 19.6], [35.0, 19.6], RouteMode::Shove)
    };
    let plan = run(&q, &r);
    assert!(plan.moved_vias.is_empty() && plan.edits.move_vias.is_empty());
    assert!(plan.reached && plan.shoved.is_empty(), "{plan:?}");
    commit(&mut q, &plan)?;
    assert_eq!(q.via(v).map(|v| v.position), Some([30.0, 20.2]));
    Ok(())
}

/// Locked tracks and pads never move: the shove falls back to walking
/// round; a locked wall with no way round stops the head at the last
/// legal point before it.
#[test]
fn immovable_copper_makes_the_shove_fall_back() -> Fallible {
    let mut p = board()?;
    let id = track(&mut p, A, [30.0, 17.0], [30.0, 23.0])?;
    apply(&mut p, LayoutCommand::SetTrackLock { id, locked: true })?;
    let plan = run(
        &p,
        &request(B, [25.0, 20.0], [35.0, 20.0], RouteMode::Shove),
    );
    assert!(plan.shoved.is_empty() && plan.reached, "{plan:?}");
    // Round the locked track's lower end.
    let low = head_points(&plan, B)
        .iter()
        .map(|q| q[1])
        .fold(f64::INFINITY, f64::min);
    assert!(low < 17.0 - 0.35 + 1e-6, "{plan:?}");
    commit(&mut p, &plan)?;
    // A locked wall across the whole board.
    let mut q = board()?;
    let wall = track(&mut q, A, [30.0, 0.2], [30.0, 39.8])?;
    apply(
        &mut q,
        LayoutCommand::SetTrackLock {
            id: wall,
            locked: true,
        },
    )?;
    let plan = run(
        &q,
        &request(B, [25.0, 20.0], [35.0, 20.0], RouteMode::Shove),
    );
    assert!(!plan.reached && plan.shoved.is_empty());
    let stop = 30.0 - WIDTH - CLEARANCE;
    assert!(
        plan.end[0] <= stop + 1e-6 && plan.end[0] > stop - 0.05,
        "{plan:?}"
    );
    assert!(plan.blocked.is_some());
    commit(&mut q, &plan)?;
    // A pad: the head going through R2 is walked round it.
    let mut r = board()?;
    let plan = run(&r, &request(A, [12.0, 8.0], [20.0, 8.0], RouteMode::Shove));
    assert!(plan.shoved.is_empty() && plan.reached, "{plan:?}");
    commit(&mut r, &plan)
}

/// Shoved copper returns as the head moves away within one gesture: the
/// plan is a pure function of the committed board and the pointer.
#[test]
fn shoved_copper_springs_back() -> Fallible {
    let mut p = board()?;
    let id = track(&mut p, A, [20.0, 20.0], [40.0, 20.0])?;
    let before = p.tracks.clone();
    let near = run(
        &p,
        &request(B, [25.0, 17.0], [30.0, 19.7], RouteMode::Shove),
    );
    assert_eq!(near.shoved, vec![id]);
    let away = run(
        &p,
        &request(B, [25.0, 17.0], [30.0, 18.0], RouteMode::Shove),
    );
    assert!(away.shoved.is_empty() && away.edits.replace.is_empty());
    assert_eq!(p.tracks, before, "previews never touch the board");
    let back = run(
        &p,
        &request(B, [25.0, 17.0], [30.0, 19.7], RouteMode::Shove),
    );
    assert_eq!(back.edits, near.edits, "the same pointer, the same push");
    commit(&mut p, &away)
}

/// Shoved chains keep their posture: a 90° chain stays on the axes, a
/// 45° chain at 45° multiples, with no acute corner, no collinear
/// neighbours and no loop.
#[test]
fn shoved_chains_are_cleaned_up_in_their_posture() -> Fallible {
    let mut p = board()?;
    // A 90° chain: along, up, along.
    track(&mut p, A, [20.0, 20.0], [28.0, 20.0])?;
    track(&mut p, A, [28.0, 20.0], [28.0, 24.0])?;
    track(&mut p, A, [28.0, 24.0], [40.0, 24.0])?;
    let plan = run(
        &p,
        &request(B, [22.0, 19.6], [26.0, 19.6], RouteMode::Shove),
    );
    assert!(plan.reached && !plan.shoved.is_empty(), "{plan:?}");
    let moved = chain(&p, &plan, A);
    assert_eq!(clean::posture_of(&moved), Posture::Orthogonal, "{moved:?}");
    for w in moved.windows(3) {
        let d1 = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
        let d2 = [w[2][0] - w[1][0], w[2][1] - w[1][1]];
        assert!(d1[0] * d2[1] - d1[1] * d2[0] != 0.0, "collinear: {moved:?}");
    }
    assert!(!clean::has_acute(&moved));
    commit(&mut p, &plan)?;
    // A 45° chain crossed by a head coming up to it.
    let mut q = board()?;
    track(&mut q, A, [20.0, 20.0], [25.0, 25.0])?;
    track(&mut q, A, [25.0, 25.0], [35.0, 25.0])?;
    let plan = run(
        &q,
        &request(B, [30.0, 20.0], [30.0, 24.8], RouteMode::Shove),
    );
    assert!(plan.reached && !plan.shoved.is_empty(), "{plan:?}");
    let moved = chain(&q, &plan, A);
    assert!(angles_ok(&moved) && !clean::has_acute(&moved), "{moved:?}");
    assert_eq!(clean::simplify(&moved).len(), moved.len(), "{moved:?}");
    commit(&mut q, &plan)
}

/// A tiny deterministic generator for the obstacle fields.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }

    fn point(&mut self) -> Point {
        [
            (self.range(22.0, 58.0) * 10.0).round() / 10.0,
            (self.range(2.0, 38.0) * 10.0).round() / 10.0,
        ]
    }
}

/// Over random fields of vias and 45° tracks of other nets, neither mode
/// ever proposes copper the engine refuses or the checks flag, and most
/// steps arrive.
#[test]
fn clearance_is_always_honoured_over_random_obstacle_fields() -> Fallible {
    let mut reached = 0usize;
    let mut steps = 0usize;
    for seed in 1..=30u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut p = board()?;
        for k in 0..12 {
            let net = if k % 2 == 0 { A } else { C };
            let at = rng.point();
            let _ = via(&mut p, net, at);
        }
        for k in 0..10 {
            let net = if k % 2 == 0 { C } else { A };
            let a = rng.point();
            let len = rng.range(2.0, 8.0);
            let dir = (rng.next() * 8.0).floor() * std::f64::consts::FRAC_PI_4;
            let b = [a[0] + len * dir.cos(), a[1] + len * dir.sin()];
            let _ = track(&mut p, net, a, b);
        }
        for mode in [RouteMode::Walkaround, RouteMode::Shove] {
            for _ in 0..4 {
                let (head, to) = (rng.point(), rng.point());
                let r = request(B, head, to, mode);
                if !run(
                    &p,
                    &RouteStepRequest {
                        to: head,
                        ..r.clone()
                    },
                )
                .edits
                .add
                .is_empty()
                {
                    continue;
                }
                let plan = run(&p, &r);
                if plan.edits.add.is_empty() {
                    continue;
                }
                steps += 1;
                reached += usize::from(plan.reached);
                let mut q = p.clone();
                commit(&mut q, &plan)
                    .map_err(|e| format!("seed {seed} {mode:?} {head:?}→{to:?}: {e}"))?;
            }
        }
    }
    assert!(steps >= 100, "{steps} steps");
    assert!(reached * 2 >= steps, "{reached} of {steps} arrived");
    Ok(())
}

fn pair_request(to: Point) -> PairStepRequest {
    let head = pair::PairHead::at([25.0, 20.2], [25.0, 19.8]);
    PairStepRequest {
        nets: [A, B],
        layer: Top,
        style: pair::PairStyle {
            width: 0.2,
            gap: 0.2,
            fan_width: 0.2,
        },
        posture: Posture::Diagonal,
        flipped: false,
        head,
        last: None,
        via: None,
        to,
        finish: None,
    }
}

/// A pair shoves a track as one unit; it walks round a via as one
/// envelope. Both commit cleanly.
#[test]
fn a_pair_shoves_and_walks_as_one_unit() -> Fallible {
    let mut p = board()?;
    let id = track(&mut p, C, [20.0, 20.6], [40.0, 20.6])?;
    let index = RouteIndex::of(&p);
    let plan = pair_plan(
        &p,
        &index,
        &pair_request([35.0, 20.0]),
        RouteMode::Shove,
        true,
    );
    assert!(
        plan.reached && plan.used == RouteMode::Shove,
        "{:?}",
        plan.blocked
    );
    assert_eq!(plan.shoved, vec![id]);
    assert!(plan.step.edits.add.iter().any(|r| r.net == A));
    assert!(plan.step.edits.add.iter().any(|r| r.net == B));
    apply(
        &mut p,
        LayoutCommand::ApplyRoute {
            edits: plan.step.edits.clone(),
        },
    )?;
    assert!(violations(&p).is_empty(), "{:?}", violations(&p));
    // Round a via on the centre line.
    let mut q = board()?;
    via(&mut q, C, [30.0, 20.0])?;
    let index = RouteIndex::of(&q);
    let plan = pair_plan(
        &q,
        &index,
        &pair_request([35.0, 20.0]),
        RouteMode::Walkaround,
        true,
    );
    assert!(plan.reached, "{:?}", plan.blocked);
    apply(
        &mut q,
        LayoutCommand::ApplyRoute {
            edits: plan.step.edits.clone(),
        },
    )?;
    assert!(violations(&q).is_empty(), "{:?}", violations(&q));
    Ok(())
}

/// `layout.route_step` is one transaction: one revision and one undo
/// step with every shoved item; undo restores them all exactly and redo
/// reapplies them. The preview query returns the same plan; a step that
/// cannot move is refused and changes nothing.
#[test]
fn a_route_step_is_one_atomic_undoable_transaction() -> Fallible {
    let mut engine = Engine::demo()?;
    let layout = |c| Operation::Layout(c);
    for (net, a, b) in [
        (A, [20.0, 20.0], [40.0, 20.0]),
        (C, [20.0, 20.5], [40.0, 20.5]),
    ] {
        engine.apply(
            engine.revision(),
            layout(LayoutCommand::Route {
                request: RouteRequest {
                    net,
                    start: a,
                    end: b,
                    width: WIDTH,
                    layer: Top,
                },
            }),
        )?;
    }
    engine.apply(
        engine.revision(),
        layout(LayoutCommand::AddVia {
            via: ViaRequest::through(C, [33.0, 19.0]),
        }),
    )?;
    let before = engine.project().clone();
    let r = request(B, [25.0, 19.6], [35.0, 19.6], RouteMode::Shove);
    let QueryResult::RoutePreview(preview) =
        engine.query(Query::RoutePreview { request: r.clone() })?
    else {
        return Err("no preview".into());
    };
    assert!(
        preview.shoved.len() >= 2 && !preview.moved_vias.is_empty(),
        "{preview:?}"
    );
    let revision = engine.revision();
    engine.apply(revision, layout(LayoutCommand::RouteStep { request: r }))?;
    assert_eq!(engine.revision(), revision + 1);
    let after = engine.project().clone();
    assert_ne!(after.tracks, before.tracks);
    engine.undo()?;
    assert_eq!(engine.project().tracks, before.tracks);
    assert_eq!(engine.project().vias, before.vias);
    engine.redo()?;
    assert_eq!(engine.project().tracks, after.tracks);
    assert_eq!(engine.project().vias, after.vias);
    // Unwinding by the plan's undo edits puts the shoved copper back.
    let mut unwind = preview.undo.clone();
    unwind.remove.extend(
        after
            .tracks
            .iter()
            .filter(|t| t.id >= before.next_id)
            .map(|t| t.id),
    );
    engine.apply(
        engine.revision(),
        layout(LayoutCommand::ApplyRoute { edits: unwind }),
    )?;
    let mut restored = engine.project().tracks.clone();
    let mut original = before.tracks.clone();
    restored.sort_by_key(|t| t.id);
    original.sort_by_key(|t| t.id);
    assert_eq!(restored, original);
    assert_eq!(engine.project().vias, before.vias);
    // A step that cannot move is refused and changes nothing.
    let stuck = request(B, [25.0, 19.6], [25.0, 19.6], RouteMode::Shove);
    let revision = engine.revision();
    assert!(
        engine
            .apply(
                revision,
                layout(LayoutCommand::RouteStep { request: stuck })
            )
            .is_err()
    );
    assert_eq!(engine.revision(), revision);
    Ok(())
}

/// Ignore is today's behaviour: the plain step, refused by the engine on
/// a clearance violation.
#[test]
fn ignore_proposes_the_plain_step() -> Fallible {
    let mut p = board()?;
    track(&mut p, A, [20.0, 20.0], [40.0, 20.0])?;
    let r = request(B, [25.0, 19.6], [35.0, 19.6], RouteMode::Ignore);
    let plan = run(&p, &r);
    assert_eq!(plan.edits, posture::step(&r.step()));
    assert!(
        apply(
            &mut p,
            LayoutCommand::ApplyRoute {
                edits: plan.edits.clone(),
            }
        )
        .is_err()
    );
    Ok(())
}
