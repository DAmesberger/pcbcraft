//! A solved part carries the copper attached to its pads
//! (`sparksmith_core::layout::dimensions::carry`): a board-parameter edit
//! that moves a dimensioned part keeps its tracks connected, in one undo
//! step.
#![allow(clippy::unwrap_used)]

use super::super::tests::{Fallible, apply, close, edit, set, sketched};
use sparksmith_core::geometry::Point;
use sparksmith_core::ids::Id;
use sparksmith_core::layout::connectivity::CopperGraph;
use sparksmith_core::layout::dimensions::{DimensionCommand, carry};
use sparksmith_core::layout::{self, LayoutCommand, RouteEditSet, RouteRequest, WorldPad};
use sparksmith_core::library::footprint_source::{DimKind, DimValue, Expr, SketchRef};
use sparksmith_core::model::board::{BoardShape, ConstraintTarget};
use sparksmith_core::model::dimension::{BoardRef, FeatureRef, PartDimensionSpec, PartRelation};
use sparksmith_core::{CopperLayer, Engine, Operation, Project, Via, ViaSpan};

fn pad(project: &Project, instance: Id, n: usize) -> Fallible<WorldPad> {
    layout::pads(project)
        .into_iter()
        .filter(|p| p.instance == instance)
        .nth(n)
        .ok_or_else(|| "pad".into())
}

fn board_codes(project: &Project) -> Vec<String> {
    let mut codes: Vec<String> = layout::checks(project)
        .into_iter()
        .map(|f| f.code)
        .collect();
    codes.sort();
    codes
}

fn unrouted(project: &Project) -> usize {
    layout::checks(project)
        .iter()
        .filter(|f| f.code == "board.unrouted")
        .count()
}

/// The demo's two resistors, the second held at half the board's width
/// from its left edge, joined by two tracks with a corner between them.
fn held_and_routed() -> Fallible<(Engine, Id)> {
    let mut engine = sketched()?;
    let p = &engine.project().placements;
    let (r1, r2) = (
        p.first().ok_or("no part")?.instance,
        p.get(1).ok_or("no second part")?.instance,
    );
    let BoardShape::Sketch(sketch) = &engine.project().board_shape else {
        return Err("not a sketch".into());
    };
    let left = sketch
        .edges
        .iter()
        .find(|e| e.name.0 == "left")
        .map(|e| e.curve.clone())
        .ok_or("no left edge")?;
    apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Add {
            dimension: PartDimensionSpec {
                kind: PartRelation::Dim(DimKind::Distance),
                refs: vec![
                    BoardRef::Part(FeatureRef::Origin {
                        target: ConstraintTarget::Placement(r2),
                    }),
                    BoardRef::Board(SketchRef::Curve(left)),
                ],
                value: Some(DimValue::Expr(Expr("width / 2".into()))),
                reference: false,
            },
        }),
    )?;
    let (a, b) = (pad(engine.project(), r1, 1)?, pad(engine.project(), r2, 0)?);
    assert_eq!(a.net, b.net);
    let corner = [20.0, 14.0];
    let track = |start: Point, end: Point| RouteRequest {
        net: a.net,
        start,
        end,
        width: 0.25,
        layer: CopperLayer::Top,
    };
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::ApplyRoute {
            edits: RouteEditSet {
                add: vec![track(a.position, corner), track(corner, b.position)],
                ..Default::default()
            },
        }),
    )?;
    assert_eq!(unrouted(engine.project()), 0);
    Ok((engine, r2))
}

/// The VM's J1 case: a part held at `width / 2` with two routed tracks.
/// Widening the board moves the part; the track on its pad slides with it
/// and the first track stretches along its own line to meet it, so both
/// keep their directions: no unconnected item, no new finding, and undo
/// restores the board exactly.
#[test]
fn a_part_held_at_half_the_width_keeps_its_tracks_when_the_width_changes() -> Fallible<()> {
    let (mut engine, r2) = held_and_routed()?;
    let codes = board_codes(engine.project());
    let saved = engine.to_file();
    let tracks = engine.project().tracks.clone();
    edit(&mut engine, vec![set("width", "80 mm")])?;
    let project = engine.project();
    let moved = pad(project, r2, 0)?;
    assert!((project.placement_of(r2).ok_or("placement")?.position[0] - 40.0).abs() < 1e-6);
    let first = project.tracks.first().ok_or("track")?;
    let second = project.tracks.get(1).ok_or("track")?;
    let (was_first, was_second) = (
        tracks.first().ok_or("track")?,
        tracks.get(1).ok_or("track")?,
    );
    let way = |t: &sparksmith_core::Track| {
        let (dx, dy) = (t.end[0] - t.start[0], t.end[1] - t.start[1]);
        let length = dx.hypot(dy);
        [dx / length, dy / length]
    };
    let same_way = |a: [f64; 2], b: [f64; 2]| close([a[0] - b[0], a[1] - b[1]], [0.0, 0.0]);
    // The first track keeps its start and direction; the corner moves
    // along it, and the second track meets it there, slid with the pad.
    assert!(close(first.start, was_first.start), "{first:?}");
    assert!(same_way(way(first), way(was_first)), "{first:?}");
    assert!(close(second.start, first.end), "{second:?}");
    assert!(same_way(way(second), way(was_second)), "{second:?}");
    assert!(close(second.end, moved.position), "{:?}", second.end);
    assert_eq!(unrouted(project), 0);
    assert_eq!(board_codes(project), codes);
    engine.undo()?;
    assert_eq!(engine.to_file(), saved);
    Ok(())
}

/// A turned and moved part carries a via on its pad and the track ends on
/// both: each lands where the part's frame takes it. A locked track stays.
#[test]
fn a_turned_part_carries_its_via_and_track_ends_rigidly() -> Fallible<()> {
    let (engine, r2) = held_and_routed()?;
    let before = engine.project().clone();
    let pad_at = pad(&before, r2, 0)?.position;
    let net = pad(&before, r2, 0)?.net;
    let mut with_via = before.clone();
    let id = with_via.allocate()?;
    with_via.vias.push(Via {
        id,
        net,
        position: pad_at,
        drill: 0.3,
        diameter: 0.6,
        span: ViaSpan::Through,
        tented: true,
        locked: false,
    });
    let mut after = with_via.clone();
    let p = after
        .placements
        .iter_mut()
        .find(|p| p.instance == r2)
        .ok_or("placement")?;
    let origin = p.position;
    p.position = [origin[0] + 3.0, origin[1] + 1.0];
    p.rotation_deg += 90.0;
    carry(
        &with_via,
        &mut after,
        &CopperGraph::of(&with_via),
        &[r2],
        None,
    )?;
    let landed = pad(&after, r2, 0)?.position;
    let via = after.via(id).ok_or("via")?;
    assert!(close(via.position, landed), "{:?} {landed:?}", via.position);
    let second = after.tracks.get(1).ok_or("track")?;
    assert!(close(second.end, landed), "{:?}", second.end);
    assert_eq!(after.tracks.first(), with_via.tracks.first());

    // Locked, the track stays where it was; the via still goes.
    let mut locked = with_via.clone();
    if let Some(t) = locked.tracks.get_mut(1) {
        t.locked = true;
    }
    let mut after = locked.clone();
    if let Some(p) = after.placements.iter_mut().find(|p| p.instance == r2) {
        p.position = [origin[0] + 3.0, origin[1] + 1.0];
    }
    carry(&locked, &mut after, &CopperGraph::of(&locked), &[r2], None)?;
    assert_eq!(after.tracks.get(1), locked.tracks.get(1));
    assert!(close(
        after.via(id).ok_or("via")?.position,
        [pad_at[0] + 3.0, pad_at[1] + 1.0]
    ));
    Ok(())
}
