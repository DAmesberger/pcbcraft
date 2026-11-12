//! Dragging one half of a differential pair moves both halves (night
//! finding F7), on the ESP32-S31 dev board's USB pair: `layout.track_drag`
//! moves the partner's coupled segment by the same shift, its neighbours
//! stretching and its held ends jogging with mitred corners, so the pair
//! keeps its gap. When the partner cannot follow, the half moves alone only
//! if the pair stays within its class limits; otherwise the drag is refused
//! with a reason naming the pair. After each edit the board has no rule
//! error and no pair finding, and one undo restores it.
use pcbcraft_automation::session::{ApplyParams, Request, Session};
use pcbcraft_core::diagnostics::Error;
use pcbcraft_core::format::EditorFile;
use pcbcraft_core::ids::Id;
use pcbcraft_core::layout::fill::{FillCache, compute_stale};
use pcbcraft_core::layout::posture::Posture;
use pcbcraft_core::layout::router::drag::{PairDrag, TrackDragRequest, drag_plan};
use pcbcraft_core::layout::router::{RouteIndex, RouteMode};
use pcbcraft_core::layout::{self, LayoutCommand, length};
use pcbcraft_core::model::Track;
use pcbcraft_core::model::production::waiver_key;
use pcbcraft_core::{Engine, Operation};
use std::path::Path;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn example() -> Fallible<Engine> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    Ok(Engine::from_file(pcbcraft_io::read_project(&path)?)?)
}

/// The tracks of the net named `name`.
fn net_tracks(engine: &Engine, name: &str) -> Fallible<Vec<Track>> {
    let p = engine.project();
    let net = p
        .nets
        .iter()
        .find(|n| n.name == name)
        .ok_or("no such net")?;
    Ok(p.tracks
        .iter()
        .filter(|t| t.net == net.id)
        .cloned()
        .collect())
}

/// A net's long coupled run below U3: its longest track, a +y run.
fn long_run(engine: &Engine, name: &str) -> Fallible<Track> {
    let length = |t: &Track| (t.end[0] - t.start[0]).hypot(t.end[1] - t.start[1]);
    let run = net_tracks(engine, name)?
        .into_iter()
        .max_by(|a, b| length(a).total_cmp(&length(b)))
        .ok_or("no such track")?;
    plus_y(name, run)
}

/// A net's short coupled run above U1: two tracks on from its long run,
/// past the diagonal between them, a +y run.
fn short_run(engine: &Engine, name: &str) -> Fallible<Track> {
    let tracks = net_tracks(engine, name)?;
    let next = |t: &Track| -> Fallible<Track> {
        Ok(tracks
            .iter()
            .find(|n| n.id != t.id && n.start == t.end)
            .cloned()
            .ok_or("no such track")?)
    };
    plus_y(name, next(&next(&long_run(engine, name)?)?)?)
}

/// `run`, checked to be a run along +y (the drags below move it in x).
fn plus_y(name: &str, run: Track) -> Fallible<Track> {
    if (run.start[0] - run.end[0]).abs() > 1e-9 || run.end[1] <= run.start[1] {
        return Err(format!("{name}'s run is not along +y: {run:?}").into());
    }
    Ok(run)
}

/// The USB pair's long coupled runs below U3, USB_DN's and USB_DP's.
fn long_runs(engine: &Engine) -> Fallible<(Track, Track)> {
    Ok((long_run(engine, "USB_DN")?, long_run(engine, "USB_DP")?))
}

/// The USB pair's short coupled runs above U1, USB_DN's and USB_DP's.
fn short_runs(engine: &Engine) -> Fallible<(Track, Track)> {
    Ok((short_run(engine, "USB_DN")?, short_run(engine, "USB_DP")?))
}

fn drag(track: Id, offset: f64) -> TrackDragRequest {
    TrackDragRequest {
        track,
        offset,
        posture: Posture::Diagonal,
        mode: RouteMode::Shove,
        move_vias: true,
        pair: PairDrag::Together,
    }
}

fn track_drag(track: Id, offset: f64) -> Operation {
    Operation::Layout(LayoutCommand::TrackDrag {
        request: drag(track, offset),
    })
}

fn track(engine: &Engine, id: Id) -> Fallible<Track> {
    Ok(engine
        .project()
        .tracks
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .ok_or("track gone")?)
}

/// Rule errors on the board (clearance, hole and keepout findings, and any
/// blocking one) and the USB pair's findings, once the stale fills are
/// refilled without an undo step.
fn issues(engine: &mut Engine) -> Fallible<Vec<String>> {
    let fresh = compute_stale(engine.project(), &mut FillCache::default())?;
    engine.store_fills(engine.revision(), fresh)?;
    // Findings the board waives for production (the J1 locating pegs)
    // are accepted, not issues.
    let production = &engine.project().production;
    Ok(layout::checks(engine.project())
        .into_iter()
        .filter(|f| {
            production
                .waiver(&waiver_key(&f.code, &f.objects))
                .is_none()
        })
        .filter(|f| {
            f.blocks.is_some()
                || f.code.starts_with("pair.")
                || f.code.starts_with("keepout.")
                || f.code.contains("clearance")
        })
        .map(|f| format!("{}: {}", f.code, f.message))
        .collect())
}

fn undo_to(engine: &mut Engine, before: &EditorFile) -> Fallible {
    engine.undo()?;
    assert!(&engine.to_file() == before, "one undo restores the board");
    Ok(())
}

/// The founder's drag: USB_DN's long run 1 mm sideways (to −x). USB_DP's
/// run beside it moves by the same shift, both clean and coupled, as one
/// undo step.
#[test]
fn dragging_one_usb_half_moves_both() -> Fallible {
    let mut engine = example()?;
    assert_eq!(issues(&mut engine)?, Vec::<String>::new());
    let (dn, dp) = long_runs(&engine)?;
    let before = engine.to_file();
    let revision = engine.revision();
    engine.apply(engine.revision(), track_drag(dn.id, 1.0))?;
    assert_eq!(engine.revision(), revision + 1, "one transaction");
    let (dn_now, dp_now) = (track(&engine, dn.id)?, track(&engine, dp.id)?);
    assert!(
        (dn_now.start[0] - (dn.start[0] - 1.0)).abs() < 1e-6,
        "{dn_now:?}"
    );
    assert!(
        (dp_now.start[0] - (dp.start[0] - 1.0)).abs() < 1e-6,
        "{dp_now:?}"
    );
    assert_eq!(issues(&mut engine)?, Vec::<String>::new());
    let report = length::report(engine.project());
    let usb = report.pairs.first().ok_or("no pair row")?;
    assert!(
        usb.uncoupled <= usb.max_uncoupled.ok_or("no limit")?,
        "{usb:?}"
    );
    assert!(usb.skew <= usb.max_skew.ok_or("no limit")?, "{usb:?}");
    undo_to(&mut engine, &before)
}

/// The preview is the same plan: it lists both halves' moved tracks.
#[test]
fn the_preview_moves_both_halves() -> Fallible {
    let engine = example()?;
    let (dn, dp) = long_runs(&engine)?;
    let p = engine.project();
    let plan = drag_plan(p, &RouteIndex::of(p), &drag(dn.id, 1.0));
    assert!(plan.reached, "{:?}", plan.blocked);
    assert!(plan.shoved.contains(&dn.id), "{:?}", plan.shoved);
    assert!(plan.shoved.contains(&dp.id), "{:?}", plan.shoved);
    let moved = |id: Id| plan.edits.replace.iter().any(|t| t.id == id);
    assert!(moved(dn.id) && moved(dp.id), "{:?}", plan.edits);
    Ok(())
}

/// Partner locked: the long run cannot move alone (it would leave the
/// pair past its skew and uncoupled limits), so the drag is refused naming the pair
/// and the board is unchanged.
#[test]
fn a_locked_partner_refuses_a_drag_that_breaks_the_pair() -> Fallible {
    let mut engine = example()?;
    let (dn, dp) = long_runs(&engine)?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetTrackLock {
            id: dp.id,
            locked: true,
        }),
    )?;
    let before = engine.to_file();
    let refused = engine.apply(engine.revision(), track_drag(dn.id, 1.0));
    let Err(Error::Conflict(why)) = refused else {
        return Err(format!("not refused: {refused:?}").into());
    };
    assert!(
        why.starts_with(
            "USB_DP can't follow (the track is locked), and moving USB_DN alone breaks pair USB: "
        ),
        "{why}"
    );
    assert!(engine.to_file() == before, "unchanged");
    Ok(())
}

/// Partner locked: a short run moved a little away from its partner
/// keeps the pair within its limits, so it moves alone, cleanly.
#[test]
fn a_locked_partner_lets_a_half_move_alone_within_the_pair_limits() -> Fallible {
    let mut engine = example()?;
    let (dn, dp) = short_runs(&engine)?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetTrackLock {
            id: dp.id,
            locked: true,
        }),
    )?;
    let dp = track(&engine, dp.id)?;
    let before = engine.to_file();
    // +0.3 mm along the left normal of a +y run is −x, away from USB_DP.
    engine.apply(engine.revision(), track_drag(dn.id, 0.3))?;
    let dn_now = track(&engine, dn.id)?;
    assert!(
        (dn_now.end[0] - (dn.end[0] - 0.3)).abs() < 1e-6,
        "{dn_now:?}"
    );
    assert_eq!(track(&engine, dp.id)?, dp, "the locked half stays");
    assert_eq!(issues(&mut engine)?, Vec::<String>::new());
    undo_to(&mut engine, &before)
}

/// The same drag gives the same board.
#[test]
fn a_pair_drag_is_deterministic() -> Fallible {
    let mut boards = Vec::new();
    for _ in 0..2 {
        let mut engine = example()?;
        let (dn, _) = long_runs(&engine)?;
        engine.apply(engine.revision(), track_drag(dn.id, -0.6))?;
        boards.push(engine.to_file());
    }
    assert!(boards.first() == boards.get(1));
    Ok(())
}

/// CLI and MCP callers send `layout.track_drag` as JSON: the same board as
/// the engine's drag, both halves moved.
#[test]
fn layout_track_drag_over_the_session_moves_both_halves() -> Fallible {
    let mut direct = example()?;
    let (dn, dp) = long_runs(&direct)?;
    direct.apply(direct.revision(), track_drag(dn.id, 1.0))?;
    let mut session = Session::new(example()?);
    let operation: Operation = serde_json::from_value(serde_json::json!({
        "editor": "Layout",
        "operation": {
            "command": "layout.track_drag",
            "params": { "request": { "track": dn.id, "offset": 1.0 } }
        }
    }))?;
    session.execute(Request::Apply(ApplyParams {
        expected_revision: session.engine().revision(),
        operation,
    }))?;
    let moved = track(session.engine(), dp.id)?;
    assert!(
        (moved.start[0] - (dp.start[0] - 1.0)).abs() < 1e-6,
        "{moved:?}"
    );
    assert!(session.engine().project().tracks == direct.project().tracks);
    Ok(())
}

/// `pair: "alone"` (the desktop's ⌥-drag) moves the dragged half by itself,
/// over the session as CLI and MCP send it: USB_DP stays put, and the
/// board gains no clearance error, only the pair findings asked for.
/// Toward the partner, the half is not allowed to shove it: it stops short
/// and USB_DP still stays.
#[test]
fn a_half_dragged_alone_leaves_its_partner() -> Fallible {
    let mut session = Session::new(example()?);
    let (dn, dp) = long_runs(session.engine())?;
    let apply = |session: &mut Session, offset: f64| -> Fallible {
        let operation: Operation = serde_json::from_value(serde_json::json!({
            "editor": "Layout",
            "operation": {
                "command": "layout.track_drag",
                "params": { "request": { "track": dn.id, "offset": offset, "pair": "alone" } }
            }
        }))?;
        session.execute(Request::Apply(ApplyParams {
            expected_revision: session.engine().revision(),
            operation,
        }))?;
        Ok(())
    };
    let errors = |engine: &mut Engine| -> Fallible<Vec<String>> {
        Ok(issues(engine)?
            .into_iter()
            .filter(|i| !i.starts_with("pair."))
            .collect())
    };
    apply(&mut session, 1.0)?;
    let mut engine = Engine::from_file(session.engine().to_file())?;
    let dn_now = track(&engine, dn.id)?;
    assert!(
        (dn_now.start[0] - (dn.start[0] - 1.0)).abs() < 1e-6,
        "{dn_now:?}"
    );
    assert_eq!(track(&engine, dp.id)?, dp, "the partner stays");
    assert_eq!(errors(&mut engine)?, Vec::<String>::new());
    // Back past where it was, toward USB_DP: it stops before it.
    let _ = apply(&mut session, -3.0);
    let mut engine = Engine::from_file(session.engine().to_file())?;
    assert_eq!(track(&engine, dp.id)?, dp, "the partner is not shoved");
    assert_eq!(errors(&mut engine)?, Vec::<String>::new());
    Ok(())
}

/// Times one drag plan per pointer move over a sweep of offsets
/// (`--ignored --nocapture`).
#[test]
#[ignore = "timing report"]
fn pair_drag_preview_timings() -> Fallible {
    let engine = example()?;
    let (dn, _) = long_runs(&engine)?;
    let p = engine.project();
    let index = RouteIndex::of(p);
    let mut times = Vec::new();
    for step in 1..=60 {
        let offset = f64::from(step) * 0.05 * if step % 2 == 0 { 1.0 } else { -1.0 };
        let t = std::time::Instant::now();
        let _ = drag_plan(p, &index, &drag(dn.id, offset));
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    times.sort_by(f64::total_cmp);
    let median = times.get(times.len() / 2).copied().unwrap_or(0.0);
    let max = times.last().copied().unwrap_or(0.0);
    println!("pair drag plan per move: median {median:.2} ms, max {max:.2} ms");
    Ok(())
}

/// The USB pair's skew, uncoupled and gap findings, measured on the graph
/// of the pair's own nets (what a drag checks per pointer move), are the
/// whole board's: on the board as it is and after drags of both halves and
/// of one half alone, some of which break the pair.
#[test]
fn the_usb_pair_measures_the_same_on_its_own_nets() -> Fallible {
    use pcbcraft_core::diagnostics::ObjectRef;
    use pcbcraft_core::layout::CopperGraph;
    let engine = example()?;
    let p = engine.project();
    let (dn, _) = long_runs(&engine)?;
    let pair = p
        .rules
        .pair_of(dn.net)
        .cloned()
        .ok_or("USB_DN is not a pair half")?;
    let index = RouteIndex::of(p);
    let codes = ["pair.skew", "pair.uncoupled", "pair.gap"];
    let positive = ObjectRef::Net { id: pair.positive };
    let mut found = 0;
    for (offset, half) in [
        (0.0, PairDrag::Together),
        (0.3, PairDrag::Together),
        (-0.4, PairDrag::Together),
        (0.5, PairDrag::Alone),
        (-1.0, PairDrag::Alone),
    ] {
        let mut next = p.clone();
        if offset != 0.0 {
            let mut request = drag(dn.id, offset);
            request.pair = half;
            let plan = drag_plan(p, &index, &request);
            layout::apply(&mut next, LayoutCommand::ApplyRoute { edits: plan.edits })?;
        }
        let whole: Vec<String> = length::checks(&next, &CopperGraph::of(&next))
            .into_iter()
            .filter(|f| codes.contains(&f.code.as_str()) && f.objects.contains(&positive))
            .map(|f| f.message)
            .collect();
        let own_graph = CopperGraph::of_nets(&next, &[pair.positive, pair.negative]);
        let own: Vec<String> = length::pair_checks(&next, &own_graph, &pair)
            .into_iter()
            .filter(|f| codes.contains(&f.code.as_str()))
            .map(|f| f.message)
            .collect();
        assert_eq!(own, whole, "{offset} mm, {half:?}");
        found += whole.len();
    }
    assert!(found > 0, "some drag breaks the pair");
    Ok(())
}
