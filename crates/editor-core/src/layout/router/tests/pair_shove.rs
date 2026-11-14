//! A shoved differential pair moves whole or stays put. Nets A and B run
//! coupled at the rules' gap along y = 20 from x = 20 to x = 40; C and D
//! are the nets that push.
use super::*;
use crate::layout::impedance;
use crate::layout::length;
use crate::rules::RulesCommand;
use crate::rules::signal::{DiffPair, DiffPairSpec};

/// A fourth net, which the demo lacks, for a track between pusher and pair.
fn another_net(p: &mut Project) -> Fallible<Id> {
    let mut net = p.nets.first().cloned().ok_or("no net")?;
    net.id = p.next_id;
    net.name = "D".into();
    net.terminals.clear();
    p.next_id += 1;
    let id = net.id;
    p.nets.push(net);
    Ok(id)
}

fn paired() -> Fallible<(Project, DiffPair, f64)> {
    let mut engine = Engine::demo()?;
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AddDiffPair {
            pair: DiffPairSpec {
                name: "USB".into(),
                positive: A,
                negative: B,
            },
        }),
    )?;
    let mut p = engine.project().clone();
    let pair = p.rules.diff_pairs.first().cloned().ok_or("no pair")?;
    let (_, gap) = impedance::pair_width_gap(&p, A, Top);
    let half = (WIDTH + gap) / 2.0;
    track(&mut p, A, [20.0, 20.0 + half], [40.0, 20.0 + half])?;
    track(&mut p, B, [20.0, 20.0 - half], [40.0, 20.0 - half])?;
    Ok((p, pair, gap))
}

/// The pair's coupling and length difference survive `plan`, which also
/// passes the engine; both halves keep their ends.
fn whole_after(p: &Project, pair: &DiffPair, plan: &RoutePlan) -> Fallible<Project> {
    let before = length::coupling(p, pair);
    let mut after = p.clone();
    commit(&mut after, plan)?;
    let c = length::coupling(&after, pair);
    // A mitred 45° corner leaves the outer half 2·pitch·tan(22.5°) outside
    // the parallel overlap counted as coupled (a mitre, not a tear), and a
    // bend round an octagon has at most eight.
    let (_, gap) = impedance::pair_width_gap(p, A, Top);
    let bend = 8.0 * 2.0 * (WIDTH + gap) * std::f64::consts::FRAC_PI_8.tan();
    assert!(
        c.coupled[0] >= before.coupled[0] - bend - 1e-6
            && c.coupled[1] >= before.coupled[1] - bend - 1e-6,
        "coupled {:?} from {:?}",
        c.coupled,
        before.coupled
    );
    assert!(c.too_close.is_empty(), "{:?}", c.too_close);
    let lengths = length::net_lengths(&after);
    let of = |n: Id| lengths.get(&n).map_or(0.0, |l| l.total);
    assert!((of(A) - of(B)).abs() < 1e-6, "{} {}", of(A), of(B));
    assert!(
        !layout::checks(&after, &Default::default())
            .iter()
            .any(|f| f.code.starts_with("pair.")),
        "no pair finding"
    );
    Ok(after)
}

/// Without a pair the same push moves the nearer track on its own: this is
/// the tearing the pair rule prevents, and single-net shove keeps it.
#[test]
fn unpaired_tracks_still_move_on_their_own() -> Fallible {
    let (mut p, _, _) = paired()?;
    p.rules.diff_pairs.clear();
    let plan = run(
        &p,
        &request(C, [30.0, 26.0], [30.0, 20.0], RouteMode::Shove),
    );
    assert!(plan.reached, "{:?}", plan.blocked);
    assert_eq!(plan.note, None);
    assert!(!plan.shoved.is_empty());
    let mut after = p.clone();
    commit(&mut after, &plan)?;
    Ok(())
}

/// A track ending between the halves pushes both the same way: coupling,
/// gap and skew stay; one transaction; the undo edits put them back.
#[test]
fn a_track_between_the_halves_pushes_the_pair_whole() -> Fallible {
    let (p, pair, _) = paired()?;
    let plan = run(
        &p,
        &request(C, [30.0, 26.0], [30.0, 20.0], RouteMode::Shove),
    );
    assert!(
        plan.reached && plan.used == RouteMode::Shove,
        "{:?}",
        plan.blocked
    );
    assert_eq!(plan.note, None);
    for net in [A, B] {
        let moved = p
            .tracks
            .iter()
            .any(|t| t.net == net && plan.shoved.contains(&t.id));
        assert!(moved, "net {net} moved");
    }
    let after = whole_after(&p, &pair, &plan)?;
    // Both halves pass on one side of the head's tip.
    for net in [A, B] {
        let lowest = after
            .tracks
            .iter()
            .filter(|t| t.net == net)
            .map(|t| t.start[1].min(t.end[1]))
            .fold(f64::INFINITY, f64::min);
        assert!(lowest < 19.9, "net {net} went round the tip: {lowest}");
    }
    Ok(())
}

/// A sweep of heads across the pair: every result is whole, moved or not.
#[test]
fn a_sweep_across_the_pair_never_tears_it() -> Fallible {
    let (p, pair, _) = paired()?;
    let index = RouteIndex::of(&p);
    let mut moved = 0;
    for k in 0..40 {
        let x = 21.0 + 0.45 * f64::from(k);
        for y in [20.6, 20.2, 20.0, 19.8, 19.4, 19.0] {
            let plan = plan(
                &p,
                &index,
                &request(C, [x, 27.0], [x + 0.3, y], RouteMode::Shove),
            );
            whole_after(&p, &pair, &plan)?;
            if p.tracks
                .iter()
                .any(|t| plan.shoved.contains(&t.id) && pair.has(t.net))
            {
                moved += 1;
            }
        }
    }
    assert!(moved > 0);
    Ok(())
}

/// A track another step pushes passes the push on to the pair, whole.
#[test]
fn a_pushed_track_pushes_the_pair_whole() -> Fallible {
    let (mut p, pair, gap) = paired()?;
    let half = (WIDTH + gap) / 2.0;
    let y = 20.0 + half + WIDTH + CLEARANCE + 0.05;
    let d = another_net(&mut p)?;
    track(&mut p, d, [24.0, y], [36.0, y])?;
    // The head's tip stops above D: D bends under it into the pair's way.
    let plan = run(
        &p,
        &request(C, [30.0, 27.0], [30.0, 21.1], RouteMode::Shove),
    );
    assert!(plan.reached, "{:?}", plan.blocked);
    assert_eq!(plan.note, None);
    for net in [A, B, d] {
        let moved = p
            .tracks
            .iter()
            .any(|t| t.net == net && plan.shoved.contains(&t.id));
        assert!(moved, "net {net} moved: {:?}", plan.shoved);
    }
    whole_after(&p, &pair, &plan)?;
    Ok(())
}

/// A head at the end of the coupled run cannot bend it (the window's end
/// stays put): the pair does not move, the plan says so and names it.
#[test]
fn a_pair_that_cannot_bend_is_held_with_a_message() -> Fallible {
    let (p, pair, _) = paired()?;
    let plan = run(
        &p,
        &request(C, [19.5, 26.0], [20.2, 20.0], RouteMode::Shove),
    );
    assert!(
        !p.tracks
            .iter()
            .any(|t| plan.shoved.contains(&t.id) && pair.has(t.net)),
        "the pair stayed"
    );
    let note = plan.note.as_deref().ok_or("no message")?;
    assert!(note.contains("USB"), "{note}");
    assert!(crate::issues::style::check(note).is_empty(), "{note}");
    if !plan.reached {
        assert_eq!(plan.blocked.as_deref(), Some(note));
    }
    whole_after(&p, &pair, &plan)?;
    Ok(())
}

/// A pair's via stays put; a step at it stops short or goes round, and says
/// why, where another net's via would have moved.
#[test]
fn a_pair_via_never_moves() -> Fallible {
    let (mut p, _, _) = paired()?;
    let v = via(&mut p, A, [30.0, 26.0])?;
    let plan = run(
        &p,
        &request(C, [26.0, 26.0], [30.0, 26.0], RouteMode::Shove),
    );
    assert!(plan.moved_vias.is_empty(), "{:?}", plan.moved_vias);
    assert!(!plan.reached, "the target is inside the via");
    assert!(plan.note.as_deref().is_some_and(|n| n.contains("USB")));
    let mut q = p.clone();
    q.rules.diff_pairs.clear();
    let plain = run(
        &q,
        &request(C, [26.0, 26.0], [30.0, 26.0], RouteMode::Shove),
    );
    assert_eq!(plain.moved_vias, vec![v], "only a pair's via holds");
    Ok(())
}

/// Routing one half alone never drags the other half with it.
#[test]
fn a_half_being_routed_does_not_push_its_partner() -> Fallible {
    let (p, _, _) = paired()?;
    let plan = run(
        &p,
        &request(A, [30.0, 26.0], [30.0, 19.6], RouteMode::Shove),
    );
    assert!(
        !p.tracks
            .iter()
            .any(|t| t.net == B && plan.shoved.contains(&t.id)),
        "{:?}",
        plan.shoved
    );
    Ok(())
}

/// A pair at `deg` from the x-axis through (30, 20), 20 mm long, its class
/// setting `skew` as the max skew when given.
fn sloped(deg: f64, skew: Option<f64>) -> Fallible<(Project, DiffPair)> {
    let (mut p, pair, _) = paired()?;
    p.tracks.retain(|t| !pair.has(t.net));
    if let Some(mm) = skew {
        let class = crate::rules::apply(
            &mut p.rules,
            RulesCommand::AddNetClass {
                class: crate::rules::NetClassSpec {
                    name: "USB".into(),
                    signal: crate::rules::signal::SignalRules {
                        pair_max_skew: Some(mm),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
        )?;
        crate::rules::apply(
            &mut p.rules,
            RulesCommand::AssignNets {
                nets: vec![A, B],
                class,
            },
        )?;
    }
    let (_, gap) = impedance::pair_width_gap(&p, A, Top);
    let half = (WIDTH + gap) / 2.0;
    let (s, c) = deg.to_radians().sin_cos();
    for (net, side) in [(A, 1.0), (B, -1.0)] {
        let off = [-s * half * side, c * half * side];
        track(
            &mut p,
            net,
            [30.0 - 10.0 * c + off[0], 20.0 - 10.0 * s + off[1]],
            [30.0 + 10.0 * c + off[0], 20.0 + 10.0 * s + off[1]],
        )?;
    }
    Ok((p, pair))
}

fn half_lengths(p: &Project) -> (f64, f64) {
    let of = |n: Id| {
        p.tracks
            .iter()
            .filter(|t| t.net == n)
            .map(|t| distance(t.start, t.end))
            .sum::<f64>()
    };
    (of(A), of(B))
}

/// The most skew any push across the pair at `deg` adds, over the pushes
/// that moved the pair, and how many moved it.
fn added_skew(deg: f64, limit: Option<f64>) -> Fallible<(usize, f64)> {
    let (p, pair) = sloped(deg, limit)?;
    let index = RouteIndex::of(&p);
    let (a0, b0) = half_lengths(&p);
    let (mut moved, mut worst) = (0, 0.0f64);
    for k in 0..15 {
        let x = 22.0 + 1.1 * f64::from(k);
        let y = 20.0 + (x - 30.0) * deg.to_radians().tan();
        for dy in [0.6, 0.0] {
            let plan = plan(
                &p,
                &index,
                &request(C, [x, y + 7.0], [x + 0.3, y + dy], RouteMode::Shove),
            );
            if !p
                .tracks
                .iter()
                .any(|t| plan.shoved.contains(&t.id) && pair.has(t.net))
            {
                continue;
            }
            let mut q = p.clone();
            commit(&mut q, &plan)?;
            let (a, b) = half_lengths(&q);
            moved += 1;
            worst = worst.max((a - b).abs() - (a0 - b0).abs());
        }
    }
    Ok((moved, worst))
}

/// Bending a pair that runs off the 45° grid adds skew (its corners do
/// not cancel exactly). Its class's max skew decides how much: a push may
/// add half of it. A class that sets none allows no added skew at all,
/// not an assumed allowance, so the pair does not move.
#[test]
fn a_push_adds_skew_only_within_the_class_limit() -> Fallible {
    let deg = 20.0;
    let (moved, worst) = added_skew(deg, Some(1.0))?;
    assert!(moved > 0, "the pushes move the pair");
    assert!(worst > 1e-3 && worst <= 0.5, "{worst}");
    let (moved, worst) = added_skew(deg, Some(0.1))?;
    assert!(
        worst <= 0.05,
        "half of 0.1 mm at most: {worst} over {moved}"
    );
    let (moved, worst) = added_skew(deg, None)?;
    assert_eq!(moved, 0, "no class limit: no push adds skew ({worst})");
    Ok(())
}
