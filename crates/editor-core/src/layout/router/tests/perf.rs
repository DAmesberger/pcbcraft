//! Interactive budget: what one pointer move costs (a whole plan from the
//! committed board), on the showcase and on a dense board of 10,000 vias
//! and 1,500 tracks. The target is 16 ms per move (one frame at 60 Hz)
//! in a release build. `route_plan_timings` prints the distributions
//! (`cargo test -p pcbcraft-core --release route_plan_timings --
//! --ignored --nocapture`); the other test runs in every build and
//! guards against runaway work and illegal proposals on the dense board.
use super::super::*;
use crate::layout::LayoutCommand;
use crate::model::CopperLayer::Top;
use crate::model::ViaSpan;
use crate::{Engine, layout};
use std::time::{Duration, Instant};

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const PITCH: f64 = 1.27;
const ORIGIN: f64 = 5.0;
const GRID: usize = 100;
const WIDTH: f64 = 0.2;

/// The demo's nets 1 and 4 carry the obstacles; net 2 routes.
const OBSTACLE_NETS: [Id; 2] = [1, 4];
const ROUTED: Id = 2;

/// A 140 × 160 mm board with a 100 × 100 grid of vias at 1.27 mm (10,000,
/// clear of the demo's parts) and 1,500 short tracks in the channels
/// between via rows (all but one). Built directly: the fixture is legal by
/// construction (via gap 0.67 mm, track to via 0.235 mm).
fn dense() -> Fallible<Project> {
    let mut p = Engine::demo()?.project().clone();
    layout::apply(
        &mut p,
        LayoutCommand::SetBoardSize {
            size: [140.0, 160.0],
        },
    )?;
    let pads = layout::pads(&p);
    let near_part = |q: Point| pads.iter().any(|pad| distance(pad.position, q) < 3.0);
    let mut vias = 0usize;
    for i in 0..GRID {
        for j in 0..GRID {
            let at = [ORIGIN + i as f64 * PITCH, ORIGIN + 20.0 + j as f64 * PITCH];
            if near_part(at) {
                continue;
            }
            let id = p.allocate()?;
            p.vias.push(Via {
                id,
                net: OBSTACLE_NETS[(i + j) % 2],
                position: at,
                drill: 0.3,
                diameter: 0.6,
                span: ViaSpan::Through,
                tented: true,
                locked: false,
            });
            vias += 1;
        }
    }
    // Tracks in every channel but the routed one, five pitches long with
    // a pitch between them.
    let mut tracks = 0usize;
    'rows: for row in (0..GRID - 1).filter(|r| *r != 1) {
        let y = ORIGIN + 20.0 + (row as f64 + 0.5) * PITCH;
        for k in 0..16 {
            if tracks >= 1500 {
                break 'rows;
            }
            let x = ORIGIN + (k * 6) as f64 * PITCH + 0.5 * PITCH;
            let id = p.allocate()?;
            p.tracks.push(Track {
                id,
                net: OBSTACLE_NETS[(row + k) % 2],
                start: [x, y],
                end: [x + 5.0 * PITCH, y],
                width: WIDTH,
                layer: Top,
                locked: false,
            });
            tracks += 1;
        }
    }
    assert!(
        vias >= 9_900 && tracks == 1_500,
        "{vias} vias, {tracks} tracks"
    );
    layout::validate(&p)?;
    Ok(p)
}

/// Pointer positions sweeping a dense board: along a free channel, then
/// across channels (shoving tracks, walking round vias).
fn sweep(moves: usize) -> (Point, Vec<Point>) {
    let y = ORIGIN + 20.0 + 1.5 * PITCH;
    let head = [ORIGIN + 10.0 * PITCH, y];
    let targets = (0..moves)
        .map(|k| {
            let t = k as f64 / moves.max(1) as f64;
            [
                head[0] + 40.0 * t,
                y + 8.0 * (t * std::f64::consts::TAU).sin(),
            ]
        })
        .collect();
    (head, targets)
}

fn request(head: Point, to: Point, mode: RouteMode) -> RouteStepRequest {
    RouteStepRequest {
        net: ROUTED,
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

struct Stats {
    median: Duration,
    p95: Duration,
    max: Duration,
    reached: usize,
    shoved: usize,
}

fn measure(
    p: &Project,
    index: &RouteIndex,
    head: Point,
    targets: &[Point],
    mode: RouteMode,
) -> Stats {
    let mut times = Vec::with_capacity(targets.len());
    let (mut reached, mut shoved) = (0, 0);
    for to in targets {
        let start = Instant::now();
        let plan = plan(p, index, &request(head, *to, mode));
        times.push(start.elapsed());
        reached += usize::from(plan.reached);
        shoved += usize::from(!plan.shoved.is_empty() || !plan.moved_vias.is_empty());
    }
    times.sort();
    let at = |q: f64| {
        times
            .get(((times.len() as f64 - 1.0) * q).round() as usize)
            .copied()
            .unwrap_or_default()
    };
    Stats {
        median: at(0.5),
        p95: at(0.95),
        max: times.last().copied().unwrap_or_default(),
        reached,
        shoved,
    }
}

/// The showcase: from each pad of a connection, sweeping toward its
/// partner.
fn showcase_sweep(p: &Project) -> Vec<(Id, Point, Vec<Point>)> {
    let pads = layout::pads(p);
    let mut out = Vec::new();
    for (k, a) in pads.iter().enumerate() {
        let Some(b) = pads
            .iter()
            .skip(k + 1)
            .find(|b| b.net == a.net && b.instance != a.instance)
        else {
            continue;
        };
        let targets = (1..=20)
            .map(|s| {
                let t = f64::from(s) / 20.0;
                [
                    a.position[0] + (b.position[0] - a.position[0]) * t,
                    a.position[1] + (b.position[1] - a.position[1]) * t + 1.5 * (t * 9.0).sin(),
                ]
            })
            .collect();
        out.push((a.net, a.position, targets));
        if out.len() >= 10 {
            break;
        }
    }
    out
}

#[test]
#[ignore = "timing report; run in release with --ignored --nocapture"]
fn route_plan_timings() -> Fallible {
    let start = Instant::now();
    let p = dense()?;
    let built = start.elapsed();
    let start = Instant::now();
    let index = RouteIndex::of(&p);
    let indexed = start.elapsed();
    eprintln!(
        "dense board: {} vias, {} tracks; fixture {built:?}, route index {indexed:?}",
        p.vias.len(),
        p.tracks.len()
    );
    let (head, targets) = sweep(400);
    for mode in [RouteMode::Walkaround, RouteMode::Shove] {
        let s = measure(&p, &index, head, &targets, mode);
        eprintln!(
            "dense {mode:?}: median {:?}, p95 {:?}, max {:?} over {} moves ({} reached, {} shoved)",
            s.median,
            s.p95,
            s.max,
            targets.len(),
            s.reached,
            s.shoved
        );
    }
    let showcase = Engine::showcase()?.project().clone();
    let start = Instant::now();
    let index = RouteIndex::of(&showcase);
    eprintln!("showcase route index {:?}", start.elapsed());
    for mode in [RouteMode::Walkaround, RouteMode::Shove] {
        let mut all = Vec::new();
        for (net, head, targets) in showcase_sweep(&showcase) {
            for to in targets {
                let start = Instant::now();
                let _ = plan(
                    &showcase,
                    &index,
                    &RouteStepRequest {
                        net,
                        width: 0.25,
                        ..request(head, to, mode)
                    },
                );
                all.push(start.elapsed());
            }
        }
        all.sort();
        let at = |q: f64| {
            all.get(((all.len() as f64 - 1.0) * q).round() as usize)
                .copied()
                .unwrap_or_default()
        };
        eprintln!(
            "showcase {mode:?}: median {:?}, p95 {:?}, max {:?} over {} moves",
            at(0.5),
            at(0.95),
            all.last().copied().unwrap_or_default(),
            all.len()
        );
    }
    Ok(())
}

/// On the dense board every proposal is accepted by the engine, the
/// sweep both shoves and walks, and no move runs away (a loose bound
/// that holds in debug builds; the 16 ms target is for release).
#[test]
fn a_dense_board_routes_legally_within_bounds() -> Fallible {
    let p = dense()?;
    let index = RouteIndex::of(&p);
    let (head, targets) = sweep(24);
    let mut shoved = 0;
    for mode in [RouteMode::Walkaround, RouteMode::Shove] {
        for to in &targets {
            let start = Instant::now();
            let plan = plan(&p, &index, &request(head, *to, mode));
            let took = start.elapsed();
            assert!(
                took < Duration::from_secs(2),
                "{mode:?} to {to:?} took {took:?}"
            );
            shoved += usize::from(!plan.shoved.is_empty());
            if plan.edits.add.is_empty() {
                continue;
            }
            let mut q = p.clone();
            layout::apply(
                &mut q,
                LayoutCommand::ApplyRoute {
                    edits: plan.edits.clone(),
                },
            )
            .map_err(|e| format!("{mode:?} to {to:?}: {e}"))?;
        }
    }
    assert!(shoved > 0, "the sweep shoves tracks");
    Ok(())
}
