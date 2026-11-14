//! Dragging one half of a differential pair checks the pair on the copper
//! graph of the pair's own nets, not on a graph of the whole board rebuilt
//! per pointer move: the findings are the same, and a board of 10,000 vias
//! costs no more per move than a small one. The pair runs coupled along
//! y = 20 from x = 20 to x = 40 (`paired`); the large board adds a grid of
//! vias and tracks of other nets beyond y = 45.
use super::*;
use crate::layout::CopperGraph;
use crate::layout::impedance;
use crate::layout::router::drag::{PairDrag, TrackDragRequest, drag_plan, pair_findings_after};
use crate::model::{Via, ViaSpan};
use crate::rules::RulesCommand;
use crate::rules::signal::{DiffPair, DiffPairSpec};
use std::time::{Duration, Instant};

/// The demo board with nets A and B a pair coupled along y = 20, its
/// class limiting skew to `skew` mm and uncoupled length to 1 mm.
fn paired(skew: f64) -> Fallible<(Project, DiffPair)> {
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
    let class = crate::rules::apply(
        &mut p.rules,
        RulesCommand::AddNetClass {
            class: crate::rules::NetClassSpec {
                name: "USB".into(),
                signal: crate::rules::signal::SignalRules {
                    pair_max_skew: Some(skew),
                    pair_max_uncoupled: Some(1.0),
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
    let (_, gap) = impedance::pair_width_gap(&p, A, Top);
    let half = (WIDTH + gap) / 2.0;
    track(&mut p, A, [20.0, 20.0 + half], [30.0, 20.0 + half])?;
    track(&mut p, A, [30.0, 20.0 + half], [40.0, 20.0 + half])?;
    track(&mut p, B, [20.0, 20.0 - half], [30.0, 20.0 - half])?;
    track(&mut p, B, [30.0, 20.0 - half], [40.0, 20.0 - half])?;
    Ok((p, pair))
}

/// `paired` on a board grown to hold `vias` through vias of nets C and
/// a new net, every other one joined to the next by a track (1,900 at
/// most: the document holds 2,000 tracks).
fn large(vias: usize) -> Fallible<(Project, DiffPair)> {
    let (mut p, pair) = paired(0.2)?;
    let other = p.allocate_net()?;
    let mut net = p.nets.first().cloned().ok_or("no net")?;
    net.id = other;
    net.name = "OTHER".into();
    net.terminals.clear();
    p.nets.push(net);
    let columns = 100usize;
    let rows = vias.div_ceil(columns);
    p.board_size = [
        p.board_size[0].max(columns as f64 * 1.27 + 6.0),
        45.0 + rows as f64 * 1.27 + 4.0,
    ];
    for k in 0..vias {
        let position = [
            3.0 + (k % columns) as f64 * 1.27,
            45.0 + (k / columns) as f64 * 1.27,
        ];
        let net = [C, other][(k / 2) % 2];
        let id = p.allocate()?;
        p.vias.push(Via {
            id,
            net,
            position,
            drill: 0.3,
            diameter: 0.6,
            span: ViaSpan::Through,
            tented: true,
            locked: false,
        });
        if k % 2 == 0 && k % columns != columns - 1 && p.tracks.len() < 1_900 {
            let id = p.allocate()?;
            p.tracks.push(Track {
                id,
                net,
                start: position,
                end: [position[0] + 1.27, position[1]],
                width: 0.15,
                layer: Top,
                locked: false,
            });
        }
    }
    Ok((p, pair))
}

fn drag(track: Id, offset: f64, mode: RouteMode) -> TrackDragRequest {
    TrackDragRequest {
        track,
        offset,
        posture: Posture::Diagonal,
        mode,
        move_vias: true,
        pair: PairDrag::Together,
    }
}

/// The first track of `net` that starts at x = `x`.
fn segment(p: &Project, net: Id, x: f64) -> Fallible<Id> {
    Ok(p.tracks
        .iter()
        .find(|t| t.net == net && (t.start[0] - x).abs() < 1e-6)
        .map(|t| t.id)
        .ok_or("no such segment")?)
}

/// The pair findings after `edit`, by the pair-only check and by the
/// whole board's graph, which must be the same; their codes.
fn same_findings(
    p: &Project,
    pair: &DiffPair,
    edit: impl Fn(&mut Project) -> Fallible,
) -> Fallible<Vec<String>> {
    let mut next = p.clone();
    edit(&mut next)?;
    let full = CopperGraph::of(&next);
    assert!(full.complete());
    let whole: Vec<_> = length_findings(&next, &full, pair);
    let quick = pair_findings_after(&CopperGraph::of(p), &next, pair);
    assert_eq!(quick, whole);
    Ok(whole.into_iter().map(|(code, _)| code).collect())
}

/// The pair's findings on `graph`, as the board checks report them.
fn length_findings(p: &Project, graph: &CopperGraph, pair: &DiffPair) -> Vec<(String, String)> {
    let net = crate::ObjectRef::Net { id: pair.positive };
    crate::layout::length::checks(p, graph, &Default::default())
        .into_iter()
        .filter(|f| {
            ["pair.skew", "pair.uncoupled", "pair.gap"].contains(&f.code.as_str())
                && f.objects.contains(&net)
        })
        .map(|f| (f.code, f.message))
        .collect()
}

/// The pair-only check finds what the whole board's graph finds: on the
/// pair as it is, with a half lengthened (skew), with a half's copper
/// moved apart (uncoupled), squeezed together (gap), touching another
/// net's copper, joined by a stub and a via of its own net, and on the
/// large board.
#[test]
fn the_pair_only_check_finds_what_the_whole_graph_finds() -> Fallible {
    let (p, pair) = paired(0.2)?;
    let (_, gap) = impedance::pair_width_gap(&p, A, Top);
    let half = (WIDTH + gap) / 2.0;
    let a1 = segment(&p, A, 30.0)?;
    let b1 = segment(&p, B, 30.0)?;
    let mut codes = Vec::new();
    codes.extend(same_findings(&p, &pair, |_| Ok(()))?);
    // A half 1.5 mm longer: skew.
    codes.extend(same_findings(&p, &pair, |q| {
        if let Some(t) = q.tracks.iter_mut().find(|t| t.id == a1) {
            t.end[0] += 1.5;
        }
        Ok(())
    })?);
    // A half moved 2 mm away: uncoupled, and the stretch to it.
    codes.extend(same_findings(&p, &pair, |q| {
        if let Some(t) = q.tracks.iter_mut().find(|t| t.id == a1) {
            t.start[1] += 2.0;
            t.end[1] += 2.0;
        }
        Ok(())
    })?);
    // The halves squeezed together: gap.
    codes.extend(same_findings(&p, &pair, |q| {
        if let Some(t) = q.tracks.iter_mut().find(|t| t.id == b1) {
            t.start[1] += half * 0.6;
            t.end[1] += half * 0.6;
        }
        Ok(())
    })?);
    // A half laid over another net's track (a short, not a join).
    let mut with_c = p.clone();
    track(&mut with_c, C, [32.0, 22.0], [32.0, 26.0])?;
    codes.extend(same_findings(&with_c, &pair, |q| {
        if let Some(t) = q.tracks.iter_mut().find(|t| t.id == a1) {
            t.start[1] = 24.0;
            t.end[1] = 24.0;
        }
        Ok(())
    })?);
    // A stub and a via of the pair's own net joined to a half.
    codes.extend(same_findings(&p, &pair, |q| {
        let at = [35.0, 20.0 + half];
        track(q, A, at, [35.0, 26.0])?;
        via(q, A, [35.0, 26.0])?;
        Ok(())
    })?);
    // The large board, a half shortened.
    let (big, pair) = large(2_000)?;
    let a1 = segment(&big, A, 30.0)?;
    codes.extend(same_findings(&big, &pair, |q| {
        if let Some(t) = q.tracks.iter_mut().find(|t| t.id == a1) {
            t.end[0] -= 2.0;
        }
        Ok(())
    })?);
    for code in ["pair.skew", "pair.uncoupled", "pair.gap"] {
        assert!(codes.iter().any(|c| c == code), "no {code} case: {codes:?}");
    }
    Ok(())
}

/// A pair drag on a board of 10,000 vias builds a graph of the pair's
/// nets per move, not of the board: the graph it checks holds the pair's
/// copper alone, whatever the rest of the board holds.
#[test]
fn a_pair_drag_checks_only_the_pairs_copper() -> Fallible {
    let (p, pair) = large(10_000)?;
    let graph = crate::layout::router::drag::pair_graph(&CopperGraph::of(&p), &p, &pair)
        .ok_or("the pair-only graph was not used")?;
    let pair_copper = p.tracks.iter().filter(|t| pair.has(t.net)).count()
        + p.vias.iter().filter(|v| pair.has(v.net)).count();
    let pads = crate::layout::pads(&p)
        .iter()
        .filter(|w| pair.has(w.net))
        .count();
    assert!(
        graph.elements().len() <= 4 * (pair_copper + pads),
        "{} elements for {pair_copper} tracks and vias and {pads} pads",
        graph.elements().len()
    );
    Ok(())
}

/// Per-move cost of dragging a pair half on the large board, the whole
/// board's graph build each move used to repeat, and the pair check that
/// replaced it (`cargo test -p sparksmith-core --release pair_drag_timings
/// -- --ignored --nocapture`; `SPARKSMITH_PAIR_VIAS` sets the vias, 10,000
/// by default, and `SPARKSMITH_PAIR_MOVES` the moves, 21).
#[test]
#[ignore = "timing report; run in release with --ignored --nocapture"]
fn pair_drag_timings() -> Fallible {
    let setting = |name: &str, default: u32| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let vias = setting("SPARKSMITH_PAIR_VIAS", 10_000);
    let (p, pair) = large(vias as usize)?;
    let index = RouteIndex::of(&p);
    let a1 = segment(&p, A, 30.0)?;
    let start = Instant::now();
    let whole = CopperGraph::of(&p);
    let build = start.elapsed();
    let start = Instant::now();
    let found = pair_findings_after(&whole, &p, &pair);
    let checked = start.elapsed();
    let mut times: Vec<Duration> = Vec::new();
    let mut moved = 0usize;
    for i in 0..setting("SPARKSMITH_PAIR_MOVES", 21) {
        let offset = -1.0 + 2.0 * f64::from(i % 21) / 20.0;
        let start = Instant::now();
        let plan = drag_plan(&p, &index, &drag(a1, offset, RouteMode::Shove));
        times.push(start.elapsed());
        if !plan.edits.replace.is_empty() {
            moved += 1;
        }
    }
    times.sort();
    let at = |q: f64| {
        let i = ((times.len() as f64 - 1.0) * q).round() as usize;
        times.get(i).copied().unwrap_or_default()
    };
    println!(
        "pair drag, {vias} vias, {} tracks: whole-board graph {build:?}, pair check {checked:?} ({} findings); {} moves ({moved} moved), median {:?}, max {:?}",
        p.tracks.len(),
        found.len(),
        times.len(),
        at(0.5),
        at(1.0)
    );
    Ok(())
}
