//! Dragging a track with shove (`drag`): the segment moves along its
//! normal, end-to-end neighbours stretch and other ends jog, other nets'
//! copper is pushed and springs back, a blocked drag stops at the last
//! legal offset, and `layout.track_drag` is one undo step that is refused
//! without change when nothing can move. On the demo board's open area
//! (x 18–42, y 18–32), with every plan applied through the engine's
//! `layout.route_apply`.
use super::*;
use crate::layout::router::drag::{TrackDragRequest, drag_plan, dragged_by};
use std::time::{Duration, Instant};

fn drag(track: Id, offset: f64, mode: RouteMode) -> TrackDragRequest {
    TrackDragRequest {
        track,
        offset,
        posture: Posture::Diagonal,
        mode,
        move_vias: true,
        pair: Default::default(),
    }
}

fn run_drag(p: &Project, r: &TrackDragRequest) -> RoutePlan {
    drag_plan(p, &RouteIndex::of(p), r)
}

fn track_of(p: &Project, id: Id) -> Fallible<Track> {
    Ok(p.tracks
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .ok_or("no track")?)
}

/// A 45° approach, a straight run and a 45° exit on net A.
fn trapezoid(p: &mut Project) -> Fallible<[Id; 3]> {
    Ok([
        track(p, A, [20.0, 20.0], [25.0, 25.0])?,
        track(p, A, [25.0, 25.0], [35.0, 25.0])?,
        track(p, A, [35.0, 25.0], [40.0, 20.0])?,
    ])
}

/// The straight run moves up by 1 mm; both diagonal neighbours stretch
/// along their own directions to meet it, keeping their far ends and
/// their identities, and nothing is added.
#[test]
fn a_dragged_segment_moves_and_its_neighbours_stretch() -> Fallible {
    let mut p = board()?;
    let [left, middle, right] = trapezoid(&mut p)?;
    let plan = run_drag(&p, &drag(middle, 1.0, RouteMode::Shove));
    assert!(plan.reached, "{:?}", plan.blocked);
    assert!(plan.edits.add.is_empty(), "{:?}", plan.edits.add);
    let moved = |id| plan.edits.replace.iter().find(|t| t.id == id);
    let m = moved(middle).ok_or("middle moved")?;
    assert!(
        close(m.start, [26.0, 26.0]) && close(m.end, [34.0, 26.0]),
        "{m:?}"
    );
    let l = moved(left).ok_or("left stretched")?;
    assert!(
        close(l.start, [20.0, 20.0]) && close(l.end, [26.0, 26.0]),
        "{l:?}"
    );
    let r = moved(right).ok_or("right stretched")?;
    assert!(
        close(r.start, [34.0, 26.0]) && close(r.end, [40.0, 20.0]),
        "{r:?}"
    );
    assert!((dragged_by(&track_of(&p, middle)?, &plan) - 1.0).abs() < 1e-9);
    // The undo edits put every track back.
    assert_eq!(plan.undo.replace.len(), 3);
    commit(&mut p, &plan)?;
    assert_eq!(track_of(&p, left)?.start, [20.0, 20.0]);
    Ok(())
}

/// An end held by a via (or a pad) stays where it is and joins the moved
/// segment with a 45° jog; an end nothing meets moves with the segment.
#[test]
fn a_held_end_jogs_at_45_degrees_and_a_free_end_follows() -> Fallible {
    let mut p = board()?;
    let id = track(&mut p, A, [20.0, 25.0], [30.0, 25.0])?;
    via(&mut p, A, [20.0, 25.0])?;
    let plan = run_drag(&p, &drag(id, -1.0, RouteMode::Shove));
    assert!(plan.reached, "{:?}", plan.blocked);
    let m = plan.edits.replace.first().ok_or("moved")?;
    assert!(
        close(m.start, [21.0, 24.0]) && close(m.end, [30.0, 24.0]),
        "{m:?}"
    );
    let jogs: Vec<[Point; 2]> = plan.edits.add.iter().map(|r| [r.start, r.end]).collect();
    assert_eq!(jogs.len(), 1, "{jogs:?}");
    assert!(close(jogs[0][0], [20.0, 25.0]) && close(jogs[0][1], [21.0, 24.0]));
    commit(&mut p, &plan)
}

/// Another net's parallel track in the way is pushed during the drag and
/// springs back when the drag returns; previews never touch the board.
#[test]
fn a_drag_shoves_other_nets_and_springs_back() -> Fallible {
    let mut p = board()?;
    let [_, middle, _] = trapezoid(&mut p)?;
    let other = track(&mut p, C, [18.0, 27.0], [42.0, 27.0])?;
    let before = p.tracks.clone();
    let near = run_drag(&p, &drag(middle, 2.0, RouteMode::Shove));
    assert!(near.reached, "{:?}", near.blocked);
    assert!(near.shoved.contains(&other), "{:?}", near.shoved);
    let away = run_drag(&p, &drag(middle, 0.5, RouteMode::Shove));
    assert!(
        !away.shoved.contains(&other),
        "springs back: {:?}",
        away.shoved
    );
    assert_eq!(p.tracks, before, "previews never touch the board");
    let again = run_drag(&p, &drag(middle, 2.0, RouteMode::Shove));
    assert_eq!(again.edits, near.edits, "the same offset, the same push");
    commit(&mut p, &near)
}

/// Copper that cannot move (a locked track) stops the drag at the last
/// legal offset, with the reason; Walkaround never pushes.
#[test]
fn a_blocked_drag_stops_at_the_last_legal_offset() -> Fallible {
    let mut p = board()?;
    let [_, middle, _] = trapezoid(&mut p)?;
    let wall = track(&mut p, C, [18.0, 27.0], [42.0, 27.0])?;
    apply(
        &mut p,
        LayoutCommand::SetTrackLock {
            id: wall,
            locked: true,
        },
    )?;
    let request = drag(middle, 2.0, RouteMode::Shove);
    let plan = run_drag(&p, &request);
    assert!(!plan.reached && plan.blocked.is_some(), "{plan:?}");
    let by = dragged_by(&track_of(&p, middle)?, &plan);
    // Centre lines 0.5 mm apart at the most: 1.5 mm, within a bisection step.
    assert!(by > 1.4 && by <= 1.5 + 1e-6, "{by}");
    assert!(!plan.shoved.contains(&wall));
    commit(&mut p, &plan)?;
    // Walkaround does not push the free track either.
    let mut q = board()?;
    let [_, middle, _] = trapezoid(&mut q)?;
    let free = track(&mut q, C, [18.0, 27.0], [42.0, 27.0])?;
    let walk = run_drag(&q, &drag(middle, 2.0, RouteMode::Walkaround));
    assert!(!walk.reached && !walk.shoved.contains(&free), "{walk:?}");
    commit(&mut q, &walk)
}

/// `layout.track_drag` applies the plan as one undo step with every
/// shoved track; `layout.track_drag_preview` returns the same plan; a drag
/// that cannot move at all is refused and the document is unchanged.
#[test]
fn track_drag_is_one_undo_step_and_refused_when_blocked() -> Fallible {
    let mut engine = Engine::demo()?;
    let mut ids = Vec::new();
    for (net, a, b) in [
        (A, [20.0, 20.0], [25.0, 25.0]),
        (A, [25.0, 25.0], [35.0, 25.0]),
        (A, [35.0, 25.0], [40.0, 20.0]),
        (C, [18.0, 27.0], [42.0, 27.0]),
    ] {
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Route {
                request: RouteRequest {
                    net,
                    start: a,
                    end: b,
                    width: WIDTH,
                    layer: Top,
                },
            }),
        )?;
        ids.push(
            engine
                .project()
                .tracks
                .last()
                .map(|t| t.id)
                .ok_or("track")?,
        );
    }
    let request = drag(ids[1], 2.0, RouteMode::Shove);
    let preview = match engine.query(Query::TrackDragPreview {
        request: request.clone(),
    })? {
        QueryResult::RoutePreview(plan) => *plan,
        other => return Err(format!("unexpected {other:?}").into()),
    };
    assert!(preview.shoved.contains(&ids[3]));
    let before = engine.project().tracks.clone();
    let revision = engine.revision();
    let outcome = engine.apply(
        revision,
        Operation::Layout(LayoutCommand::TrackDrag {
            request: request.clone(),
        }),
    )?;
    assert!(
        matches!(outcome, crate::Outcome::TrackDragged { reached: true, offset } if (offset - 2.0).abs() < 1e-9),
        "{outcome:?}"
    );
    assert_eq!(engine.revision(), revision + 1);
    let other = track_of(engine.project(), ids[3])?;
    assert_ne!(other, before[before.len() - 1], "the other net moved");
    engine.undo()?;
    let mut now = engine.project().tracks.clone();
    let mut was = before.clone();
    now.sort_by_key(|t| t.id);
    was.sort_by_key(|t| t.id);
    assert_eq!(now, was, "one undo step restores every track");
    // Locked against the wall at the clearance: no offset is legal.
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetTrackLock {
            id: ids[3],
            locked: true,
        }),
    )?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::TrackDrag {
            request: drag(ids[1], 2.0, RouteMode::Shove),
        }),
    )?;
    let revision = engine.revision();
    let tracks = engine.project().tracks.clone();
    let refused = engine.apply(
        revision,
        Operation::Layout(LayoutCommand::TrackDrag {
            request: drag(ids[1], 1.0, RouteMode::Shove),
        }),
    );
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(engine.revision(), revision);
    assert_eq!(engine.project().tracks, tracks);
    // Bad input is refused too.
    let bad = engine.apply(
        revision,
        Operation::Layout(LayoutCommand::TrackDrag {
            request: drag(ids[1], f64::NAN, RouteMode::Shove),
        }),
    );
    assert!(bad.is_err());
    assert_eq!(engine.revision(), revision);
    Ok(())
}

/// Per-move cost of a drag on the showcase: every unlocked top-layer
/// track dragged through 20 offsets from −1 to +1 mm, a whole plan from
/// the committed board each (`cargo test -p pcbcraft-core --release
/// track_drag_timings -- --ignored --nocapture`).
#[test]
#[ignore = "timing report; run in release with --ignored --nocapture"]
fn track_drag_timings() -> Fallible {
    let engine = Engine::showcase()?;
    let p = engine.project();
    let index = RouteIndex::with_graph(p, engine.copper_graph());
    let mut times: Vec<Duration> = Vec::new();
    let mut shoving = 0usize;
    for t in p.tracks.iter().filter(|t| t.layer == Top && !t.locked) {
        for i in 0..20 {
            let offset = -1.0 + 2.0 * f64::from(i) / 19.0;
            let start = Instant::now();
            let plan = drag_plan(p, &index, &drag(t.id, offset, RouteMode::Shove));
            times.push(start.elapsed());
            if plan.shoved.len() > 1 {
                shoving += 1;
            }
        }
    }
    times.sort();
    let at = |q: f64| {
        let i = ((times.len() as f64 - 1.0) * q).round() as usize;
        times.get(i).copied().unwrap_or_default()
    };
    println!(
        "track drag on the showcase: {} moves ({shoving} shoving), median {:?}, p95 {:?}, max {:?}",
        times.len(),
        at(0.5),
        at(0.95),
        at(1.0)
    );
    Ok(())
}
