//! Lengths on the copper graph: the disagreement cases C1, C2 and C6 of
//! `docs/copper-graph.md` as pair paths and net lengths, where a path
//! enters a pad, a path through a fill, and the timings at scale.
//!
//! The board is the demo's (see `connectivity/tests.rs`): R1.2 at
//! (10.8, 10) and R2.1 at (24.2, 10) on net 2, with 0.3 mm tracks and
//! 0.6 mm vias on a 1.6 mm two-layer board, whose top-to-bottom barrel is
//! 1.635 mm (1.6 mm of dielectric plus half of each 35 µm foil).
use super::super::tests::{R1_2, R2_1, rect, top, track, via};
use crate::diagnostics::Result;
use crate::layout::length::{self, PairMeasure, PairMeasured, path};
use crate::layout::tests::{SHARED, fixture};
use crate::layout::{self, CopperGraph};
use crate::model::fill::{FillSettings, FillSpec};
use crate::model::{CopperLayer, Fill, Project, ViaSpan};
use crate::rules::signal::{DiffPair, DiffPairId};

const BARREL: f64 = 1.635;

/// A pair with both halves on net 2, as the design's cases were run.
fn pair(positive: crate::ids::Id, negative: crate::ids::Id) -> DiffPair {
    DiffPair {
        id: DiffPairId(1),
        name: "P".into(),
        positive,
        negative,
        endpoints: None,
    }
}

fn measure(project: &Project) -> PairMeasure {
    path::measure(project, &CopperGraph::of(project), &pair(SHARED, SHARED))
}

/// The path of each half: (from, to, tracks, vias, through a fill).
fn halves(m: &PairMeasure) -> Vec<(String, String, f64, f64, bool)> {
    m.halves
        .iter()
        .flatten()
        .map(|h| (h.from.clone(), h.to.clone(), h.tracks, h.vias, h.via_fill))
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Both halves run R1.2 to R2.1 with `tracks` of centre line and `vias`
/// of barrel.
fn assert_path(project: &Project, tracks: f64, vias: f64) {
    let m = measure(project);
    assert_eq!(m.measured, PairMeasured::Derived, "{m:?}");
    let found = halves(&m);
    assert_eq!(found.len(), 2, "{m:?}");
    for (from, to, t, v, fill) in found {
        assert_eq!((from.as_str(), to.as_str()), ("R1.2", "R2.1"), "{m:?}");
        assert!(close(t, tracks) && close(v, vias), "{t} {v}: {m:?}");
        assert!(!fill, "{m:?}");
    }
}

fn net_length(project: &Project) -> length::NetLength {
    CopperGraph::of(project)
        .net_lengths()
        .get(&SHARED)
        .copied()
        .unwrap_or_default()
}

/// C1: two tracks crossing in mid-span. The path runs through the
/// crossing; the overhangs are off it. Hand-derived: track a runs (7.2, 3)
/// from R1.2 (7.8 mm), track b runs (10.2, −3) to R2.1 (√113.04 mm); they
/// cross at 13.4 / 17.4 of each, so the path is that share of both.
#[test]
fn c1_a_pair_path_runs_through_a_mid_span_crossing() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, R1_2, [18.0, 13.0], top())?;
    track(&mut project, [14.0, 13.0], R2_1, top())?;
    let (a, b) = (7.8, 113.04f64.sqrt());
    assert_path(&project, (a + b) * 13.4 / 17.4, 0.0);
    let net = net_length(&project);
    assert!(close(net.total, a + b) && net.via_count == 0, "{net:?}");
    Ok(())
}

/// C2: track ends overlapping two vias' rings 0.35 mm off their centres.
/// Both vias carry the signal: the path is 3.85 + 4.3 + 3.85 mm of track
/// and two barrels, and the net length counts both barrels, which the
/// centre-line rule (0.35 mm from the centre, past the 0.3 mm radius)
/// missed.
#[test]
fn c2_off_centre_vias_carry_the_path_and_their_barrels() -> Result<()> {
    let mut project = fixture()?;
    via(&mut project, [15.0, 10.0], ViaSpan::Through)?;
    via(&mut project, [20.0, 10.0], ViaSpan::Through)?;
    track(&mut project, R1_2, [14.65, 10.0], top())?;
    track(
        &mut project,
        [15.35, 10.0],
        [19.65, 10.0],
        CopperLayer::Bottom,
    )?;
    track(&mut project, [20.35, 10.0], R2_1, top())?;
    assert_path(&project, 12.0, 2.0 * BARREL);
    let net = net_length(&project);
    assert!(close(net.tracks, 12.0), "{net:?}");
    assert!(close(net.vias, 2.0 * BARREL), "{net:?}");
    assert_eq!(net.via_count, 2);
    Ok(())
}

/// C6: two tracks overlapping side by side, centre lines 0.2 mm apart.
/// The path crosses between them where the copper overlaps. Hand-derived:
/// 5.2 mm along the first track from R1.2 to the second's start, then its
/// 8.2 mm to R2.1 (or, equally, 7.2 mm and the last 6.2 mm).
#[test]
fn c6_a_pair_path_crosses_side_by_side_overlap() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, R1_2, [18.0, 10.0], top())?;
    track(&mut project, [16.0, 10.2], [24.2, 10.2], top())?;
    assert_path(&project, 13.4, 0.0);
    assert!(close(net_length(&project).total, 7.2 + 8.2));
    Ok(())
}

/// A route's last leg into a pad is path, though the corner before it
/// already lies on the pad's copper; a track overshooting the pad's
/// centre counts only up to it. Both are measured to R1.2's centre,
/// 13.4 mm from R2.1's.
#[test]
fn a_path_enters_a_pad_at_its_centre() -> Result<()> {
    let mut project = fixture()?;
    // The corner at x = 11.2 lies on R1.2's land (10.3 to 11.3 mm).
    track(&mut project, R2_1, [11.2, 10.0], top())?;
    track(&mut project, [11.2, 10.0], R1_2, top())?;
    assert_path(&project, 13.4, 0.0);
    let mut over = fixture()?;
    track(&mut over, R2_1, [10.5, 10.0], top())?;
    assert_path(&over, 13.4, 0.0);
    Ok(())
}

/// A leg shorter than the track width is path: its far end lies on the
/// copper of the track before it, but it is joined exactly to the track
/// after it, so it does not cut itself short. Hand-derived: 6.2 + 0.1 +
/// 7.2 mm, the net's whole copper (a meander's short legs were cut 0.11 mm
/// each before).
#[test]
fn a_short_leg_is_not_cut_short_through_the_track_before_it() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, R1_2, [17.0, 10.0], top())?;
    track(&mut project, [17.0, 10.0], [17.0, 10.1], top())?;
    track(&mut project, [17.0, 10.1], [24.2, 10.1], top())?;
    assert_path(&project, 13.5, 0.0);
    assert!(close(net_length(&project).total, 13.5));
    Ok(())
}

/// A path through a fill of its net is measured, the fill hop adding no
/// length, and marked as running via the fill: a track from R1.2 into a
/// pour around R2.1, 10.2 mm.
#[test]
fn a_path_through_a_fill_is_measured_and_marked() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, R1_2, [21.0, 10.0], top())?;
    let id = project.allocate()?;
    project.fills.push(Fill::from_spec(
        id,
        FillSpec {
            name: String::new(),
            net: Some(SHARED),
            layer: top(),
            outline: rect(20.0, 8.0, 26.0, 12.0),
            priority: 0,
            settings: FillSettings::default(),
        },
    ));
    let computed = layout::fill::compute(&project, id)?;
    if let Some(fill) = project.fills.iter_mut().find(|f| f.id == id) {
        fill.computed = Some(computed);
    }
    let m = measure(&project);
    assert_eq!(m.measured, PairMeasured::Derived, "{m:?}");
    for (from, to, tracks, vias, fill) in halves(&m) {
        assert_eq!((from.as_str(), to.as_str()), ("R1.2", "R2.1"));
        assert!(close(tracks, 10.2) && close(vias, 0.0), "{m:?}");
        assert!(fill, "{m:?}");
    }
    Ok(())
}

/// `n` collinear tracks from R1.2, each [`step`] long, with R2 moved so
/// R2.1 ends them.
fn collinear(n: usize) -> Result<Project> {
    let mut project = fixture()?;
    let end = R1_2[0] + n as f64 * step(n);
    let r2 = project
        .instances
        .iter()
        .find(|i| i.designator == "R2")
        .map(|i| i.id)
        .ok_or_else(|| crate::Error::NotFound("R2".into()))?;
    layout::apply(
        &mut project,
        layout::LayoutCommand::Move {
            instance: r2,
            position: [end + 0.8, 10.0],
            rotation_deg: 0.0,
            layer: crate::model::Layer::Top,
        },
    )?;
    for k in 0..n {
        let x = R1_2[0] + k as f64 * step(n);
        track(&mut project, [x, 10.0], [x + step(n), 10.0], top())?;
    }
    Ok(project)
}

/// 1 mm, or shorter (never so short that a track's copper reaches past its
/// neighbour) so that `n` tracks stay within 1 m.
fn step(n: usize) -> f64 {
    (1000.0 / n as f64).min(1.0)
}

/// Best of five, in ms.
fn best_ms(mut run: impl FnMut()) -> f64 {
    (0..5)
        .map(|_| {
            let start = std::time::Instant::now();
            run();
            start.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::INFINITY, f64::min)
}

/// Pair measurement is shortest paths on the graph, not a pairwise scan:
/// halves of 256 to 2,048 collinear tracks (the old pair graph took 2.0,
/// 18.2 and 61.2 ms, and refused more).
#[test]
fn pair_measurement_scales_with_the_half() -> Result<()> {
    for n in [256, 1024, 2048] {
        let project = collinear(n)?;
        let graph = CopperGraph::of(&project);
        let p = pair(SHARED, SHARED);
        let mut m = PairMeasure::default();
        let query = best_ms(|| m = path::measure(&project, &graph, &p));
        let built = best_ms(|| {
            let _ = path::measure(&project, &CopperGraph::of(&project), &p);
        });
        eprintln!(
            "pair of two {n}-track halves: {query:.2} ms on the graph, {built:.2} ms with its build"
        );
        for (_, _, tracks, ..) in halves(&m) {
            assert!((tracks - n as f64 * step(n)).abs() < 1e-6, "{n}: {m:?}");
        }
        assert!(query < 1000.0, "{n}: {query} ms");
    }
    Ok(())
}

/// Net lengths and a pair's measurement on the 10,000-via board: one pass
/// over the graph's edges, where the old via scan took 26.7 ms.
#[test]
fn ten_thousand_vias_are_measured_within_a_budget() -> Result<()> {
    let project = crate::layout::via::tests::via_board(10_000, 2_000)?;
    let graph = CopperGraph::of(&project);
    assert!(graph.complete());
    let mut lengths = Default::default();
    let net = best_ms(|| lengths = graph.net_lengths());
    let built = best_ms(|| {
        let _ = length::net_lengths(&project);
    });
    let nets = [project.nets[0].id, project.nets[1].id];
    let p = pair(nets[0], nets[1]);
    let measured = best_ms(|| {
        let _ = path::measure(&project, &graph, &p);
    });
    eprintln!(
        "10,000 vias, 2,000 tracks: net lengths in {net:.2} ms on the graph \
         ({built:.2} ms with its build); a pair of its via nets in {measured:.2} ms"
    );
    // Every via of the two nets is counted.
    let total: usize = nets
        .iter()
        .filter_map(|n| lengths.get(n))
        .map(|l| l.via_count)
        .sum();
    assert!(total >= 10_000, "{total}");
    assert!(net < 1000.0 && measured < 1000.0);
    Ok(())
}
