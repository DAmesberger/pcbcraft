//! A shove never detaches copper joined to what it moves in mid-span: a
//! T on a pushed track pins the chain there, and a via joined by a track
//! running through it stays put. The joints come from the copper graph.
use super::*;
use crate::layout::CopperGraph;
use crate::layout::connectivity::keeps_joint;

/// A head of net B twice as wide as a track pushes harder than one.
fn wide(head: Point, to: Point) -> RouteStepRequest {
    RouteStepRequest {
        width: 1.5,
        ..request(B, head, to, RouteMode::Shove)
    }
}

fn track_on(p: &mut Project, net: Id, a: Point, b: Point, layer: CopperLayer) -> Fallible<Id> {
    apply(
        p,
        LayoutCommand::Route {
            request: RouteRequest {
                net,
                start: a,
                end: b,
                width: WIDTH,
                layer,
            },
        },
    )?;
    Ok(p.tracks.last().map(|t| t.id).ok_or("no track")?)
}

/// Whether the tracks and vias `ids` lie in one island of `p`'s copper.
fn one_island(p: &Project, tracks: &[Id], vias: &[Id]) -> bool {
    let islands = CopperGraph::of(p).islands();
    islands.iter().any(|i| {
        tracks.iter().all(|t| i.tracks.contains(t)) && vias.iter().all(|v| i.vias.contains(v))
    })
}

/// The board with a run of net A along y = 20 and a branch of A from
/// (`x`, 20) down to y = 18: a T on the run's centre line.
fn tee(x: f64) -> Fallible<(Project, Id)> {
    let mut p = board()?;
    let run_id = track(&mut p, A, [20.0, 20.0], [40.0, 20.0])?;
    track(&mut p, A, [x, 20.0], [x, 18.0])?;
    Ok((p, run_id))
}

/// Apply the plan; every track of net A is still in one island.
fn still_joined(p: &mut Project, plan: &RoutePlan) -> Fallible {
    commit(p, plan)?;
    let tracks: Vec<Id> = p
        .tracks
        .iter()
        .filter(|t| t.net == A)
        .map(|t| t.id)
        .collect();
    assert!(one_island(p, &tracks, &[]), "the T stays joined");
    Ok(())
}

/// A T on a pushed track. Pushed without its branch, the run's ramp back
/// down crosses x = 32.3 above y = 20, which would leave the branch's end
/// in open board. With the branch, the run is pinned at (32.3, 20): it
/// still bends round the head, but comes back down before the T.
#[test]
fn a_shove_keeps_a_t_on_the_pushed_track_joined() -> Fallible {
    let at = [32.3, 20.0];
    let push = wide([25.0, 19.6], [31.0, 19.6]);
    let mut bare = board()?;
    track(&mut bare, A, [20.0, 20.0], [40.0, 20.0])?;
    let plan = run(&bare, &push);
    assert!(plan.reached && !plan.shoved.is_empty(), "{plan:?}");
    let moved = chain(&bare, &plan, A);
    assert!(
        !keeps_joint(&moved, at),
        "the push moves the point: {moved:?}"
    );
    let (mut p, run_id) = tee(at[0])?;
    let plan = run(&p, &push);
    assert!(plan.reached && plan.shoved.contains(&run_id), "{plan:?}");
    let moved = chain(&p, &plan, A);
    assert!(
        keeps_joint(&moved, at),
        "the run passes through the T: {moved:?}"
    );
    still_joined(&mut p, &plan)
}

/// A pin away from the push holds only its own point: with the T at
/// (38, 20) the run still bends round the head, and through the T.
#[test]
fn a_pinned_chain_still_bends_elsewhere() -> Fallible {
    let at = [38.0, 20.0];
    let (mut p, run_id) = tee(at[0])?;
    let plan = run(&p, &wide([25.0, 19.6], [31.0, 19.6]));
    assert!(plan.reached && plan.shoved.contains(&run_id), "{plan:?}");
    let moved = chain(&p, &plan, A);
    assert!(
        keeps_joint(&moved, at),
        "the run passes through the T: {moved:?}"
    );
    still_joined(&mut p, &plan)
}

/// A via of net A with a track ending on it, pushed hard by a head on
/// Top; the tracks are on Bottom, out of the head's way. Without a second
/// track the via moves clear of where that track would run, and the first
/// track follows. With a track running through the via, the via may not
/// move (that would detach it), so the head goes round, and all three
/// stay joined.
#[test]
fn a_shove_does_not_move_a_via_a_track_runs_through() -> Fallible {
    let at = [30.0, 20.2];
    let bottom = CopperLayer::Bottom;
    let push = wide([25.0, 19.6], [35.0, 19.6]);
    let mut bare = board()?;
    let v = via(&mut bare, A, at)?;
    track_on(&mut bare, A, at, [30.0, 26.0], bottom)?;
    let plan = run(&bare, &push);
    assert_eq!(plan.moved_vias, vec![v], "{plan:?}");
    let moved = plan.edits.move_vias.first().ok_or("no move")?.position;
    // Farther from the through track's centre line (y = 20.2) than the
    // via's radius and the track's half-width: it would no longer touch it.
    assert!((moved[1] - at[1]).abs() > 0.3 + WIDTH / 2.0, "{moved:?}");
    let mut p = board()?;
    let v = via(&mut p, A, at)?;
    let ending = track_on(&mut p, A, at, [30.0, 26.0], bottom)?;
    let through = track_on(&mut p, A, [24.0, 20.2], [36.0, 20.2], bottom)?;
    assert!(one_island(&p, &[ending, through], &[v]));
    let plan = run(&p, &push);
    assert!(plan.moved_vias.is_empty(), "{plan:?}");
    commit(&mut p, &plan)?;
    assert_eq!(p.via(v).map(|v| v.position), Some(at));
    assert!(one_island(&p, &[ending, through], &[v]), "nothing detached");
    Ok(())
}
