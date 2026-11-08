//! The copper graph against the disagreement cases of
//! `docs/copper-graph.md` (C0–C6), each with the right answer, and the
//! graph's own rules: stubs, vias, airwires, limits and the engine cache.
//!
//! The board is the demo's: R1 at (10, 10) and R2 at (25, 10), net 2
//! joining R1.2 (land 10.3–11.3 mm in x) and R2.1 (land 23.7–24.7 mm).
use super::queries::spanning;
use super::{CopperGraph, ElementKind};
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::geometry::Point;
use crate::ids::{Endpoint, Id, PadId, TerminalId};
use crate::layout::tests::{SHARED, fixture};
use crate::layout::{self, WorldPad};
use crate::library::{Outline, PadShape, Segment};
use crate::model::fill::{FillSettings, FillSpec};
use crate::model::{CopperLayer, Fill, Layer, Project, Stackup, Track, Via, ViaSpan};

pub(super) const R1_2: Point = [10.8, 10.0];
pub(super) const R2_1: Point = [24.2, 10.0];

pub(super) fn track(
    project: &mut Project,
    start: Point,
    end: Point,
    layer: CopperLayer,
) -> Result<Id> {
    let id = project.allocate()?;
    project.tracks.push(Track {
        id,
        net: SHARED,
        start,
        end,
        width: 0.3,
        layer,
        locked: false,
    });
    Ok(id)
}

pub(super) fn via(project: &mut Project, position: Point, span: ViaSpan) -> Result<Id> {
    let id = project.allocate()?;
    project.vias.push(Via {
        id,
        net: SHARED,
        position,
        drill: 0.3,
        diameter: 0.6,
        span,
        tented: true,
        locked: false,
    });
    Ok(id)
}

fn codes(findings: &[Finding], code: &str) -> Vec<Finding> {
    findings
        .iter()
        .filter(|f| f.code == code)
        .cloned()
        .collect()
}

/// The connectivity findings of `project`, from its graph.
fn connectivity(project: &Project) -> Vec<Finding> {
    CopperGraph::of(project).findings(project)
}

pub(super) fn top() -> CopperLayer {
    Layer::Top.into()
}

/// C0: one straight track from pad to pad is routed, with nothing open.
#[test]
fn c0_a_straight_track_joins_the_pads() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, [11.2, 10.0], [23.8, 10.0], top())?;
    assert_eq!(connectivity(&project), Vec::new());
    assert!(layout::airwires(&project).is_empty());
    assert!(
        layout::checks(&project).is_empty(),
        "{:?}",
        layout::checks(&project)
    );
    Ok(())
}

/// C1: two tracks crossing in mid-span join at the crossing. The board is
/// routed and each overhang past the crossing is a stub of its own.
#[test]
fn c1_tracks_crossing_mid_span_join_and_their_overhangs_are_stubs() -> Result<()> {
    let mut project = fixture()?;
    let a = track(&mut project, R1_2, [18.0, 13.0], top())?;
    let b = track(&mut project, [14.0, 13.0], R2_1, top())?;
    let found = connectivity(&project);
    assert_eq!(codes(&found, "board.unrouted"), Vec::new());
    assert!(layout::airwires(&project).is_empty());
    let stubs = codes(&found, "board.track_dangling");
    assert_eq!(stubs.len(), 2, "{stubs:?}");
    for (id, end) in [(a, "(18, 13) mm"), (b, "(14, 13) mm")] {
        assert!(
            stubs
                .iter()
                .any(|f| f.objects.first() == Some(&ObjectRef::Track { id })
                    && f.message.contains(end)),
            "{stubs:?}"
        );
    }
    // Each track is cut at the crossing: three stations, the middle one
    // shared with the other track.
    let graph = CopperGraph::of(&project);
    for index in [0, 1] {
        let element = graph
            .elements()
            .iter()
            .position(|e| e.kind == ElementKind::Track { index })
            .ok_or(Error::NotFound("track element".into()))?;
        assert_eq!(graph.vertices_of(element).len(), 3);
    }
    Ok(())
}

/// C2: track ends overlapping a via's ring 0.35 mm off its centre join the
/// via: routed, and neither the tracks nor the vias are open.
#[test]
fn c2_track_ends_off_a_via_centre_join_it() -> Result<()> {
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
    assert_eq!(connectivity(&project), Vec::new());
    assert!(layout::airwires(&project).is_empty());
    Ok(())
}

/// C3 (the via gap): a track ending at a via that joins nothing else is a
/// stub, reported once with the via named in it; the via raises no
/// finding of its own.
#[test]
fn c3_a_track_to_a_lone_via_is_one_stub() -> Result<()> {
    let mut project = fixture()?;
    let t = track(&mut project, R1_2, [17.0, 10.0], top())?;
    let v = via(&mut project, [17.0, 10.0], ViaSpan::Through)?;
    let found = connectivity(&project);
    assert_eq!(codes(&found, "board.unrouted").len(), 2, "{found:?}");
    assert_eq!(codes(&found, "board.via_dangling"), Vec::new());
    let [stub] = codes(&found, "board.track_dangling")
        .try_into()
        .map_err(|f: Vec<Finding>| Error::Invalid(format!("one stub: {f:?}")))?;
    assert_eq!(
        stub.objects,
        vec![
            ObjectRef::Track { id: t },
            ObjectRef::Via { id: v },
            ObjectRef::Net { id: SHARED }
        ]
    );
    assert!(
        stub.message.contains("a via at (17, 10) mm"),
        "{}",
        stub.message
    );
    Ok(())
}

/// C4: a blind via from Top to Inner 1 does not reach a Bottom track
/// over it. The net is unrouted, and each side is copper reaching one pad
/// only, so a stub: the Top track and the blind via from R1.2; the Bottom
/// track (open over the blind via), the through via and the Top track on to
/// R2.1.
#[test]
fn c4_a_blind_via_joins_only_the_layers_it_spans() -> Result<()> {
    let mut project = fixture()?;
    project.stackup = Stackup::preset(4)?;
    let top_track = track(&mut project, R1_2, [17.0, 10.0], top())?;
    let blind = via(
        &mut project,
        [17.0, 10.0],
        ViaSpan::Between {
            from: CopperLayer::Top,
            to: CopperLayer::Inner(1),
        },
    )?;
    let bottom = track(
        &mut project,
        [17.0, 10.0],
        [20.0, 10.0],
        CopperLayer::Bottom,
    )?;
    let through = via(&mut project, [20.0, 10.0], ViaSpan::Through)?;
    let last = track(&mut project, [20.0, 10.0], R2_1, top())?;
    let found = connectivity(&project);
    assert_eq!(codes(&found, "board.unrouted").len(), 2, "{found:?}");
    assert_eq!(layout::airwires(&project).len(), 1);
    let stubs = codes(&found, "board.track_dangling");
    assert_eq!(stubs.len(), 2, "{stubs:?}");
    assert!(stubs.iter().any(|f| f.objects
        == vec![
            ObjectRef::Track { id: top_track },
            ObjectRef::Via { id: blind },
            ObjectRef::Net { id: SHARED }
        ]));
    assert!(
        stubs.iter().any(|f| f.objects
            == vec![
                ObjectRef::Track { id: bottom },
                ObjectRef::Track { id: last },
                ObjectRef::Via { id: through },
                ObjectRef::Net { id: SHARED }
            ]
            && f.message.contains("(Top, Bottom) ends at (17, 10) mm")),
        "{stubs:?}"
    );
    // Turned into a through via, it joins all three tracks.
    if let Some(v) = project.vias.iter_mut().find(|v| v.id == blind) {
        v.span = ViaSpan::Through;
    }
    assert_eq!(connectivity(&project), Vec::new());
    Ok(())
}

pub(super) fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Outline {
    Outline {
        start: [x0, y0],
        segments: vec![
            Segment::Line { to: [x1, y0] },
            Segment::Line { to: [x1, y1] },
            Segment::Line { to: [x0, y1] },
            Segment::Line { to: [x0, y0] },
        ],
    }
}

/// C5: a fresh fill joins both pads, and a track runs from inside the fill
/// to an open end outside it. Routed through the fill; the track's open
/// end is a stub (being joined to a fill no longer hides it). Stale, the
/// fill joins nothing.
#[test]
fn c5_a_track_leaving_a_fill_to_an_open_end_is_a_stub() -> Result<()> {
    let mut project = fixture()?;
    let open = track(&mut project, [17.0, 10.0], [17.0, 15.0], top())?;
    let id = project.allocate()?;
    project.fills.push(Fill::from_spec(
        id,
        FillSpec {
            name: String::new(),
            net: Some(SHARED),
            layer: top(),
            outline: rect(10.4, 8.0, 24.6, 12.0),
            priority: 0,
            settings: FillSettings::default(),
        },
    ));
    let computed = layout::fill::compute(&project, id)?;
    if let Some(fill) = project.fills.iter_mut().find(|f| f.id == id) {
        fill.computed = Some(computed);
    }
    let found = connectivity(&project);
    assert_eq!(codes(&found, "board.unrouted"), Vec::new(), "{found:?}");
    assert!(layout::airwires(&project).is_empty());
    let [stub] = codes(&found, "board.track_dangling")
        .try_into()
        .map_err(|f: Vec<Finding>| Error::Invalid(format!("one stub: {f:?}")))?;
    assert_eq!(stub.objects.first(), Some(&ObjectRef::Track { id: open }));
    assert!(stub.message.contains("(17, 15) mm"), "{}", stub.message);
    // Moved, the track makes the fill stale: its joins stop counting.
    if let Some(t) = project.tracks.iter_mut().find(|t| t.id == open) {
        t.end = [17.5, 15.0];
    }
    assert_eq!(codes(&connectivity(&project), "board.unrouted").len(), 2);
    Ok(())
}

/// C6: two tracks overlapping side by side, their centre lines 0.2 mm
/// apart and neither end on the other's copper, join: routed, no stub.
#[test]
fn c6_tracks_overlapping_side_by_side_join() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, R1_2, [18.0, 10.0], top())?;
    track(&mut project, [16.0, 10.2], [24.2, 10.2], top())?;
    assert_eq!(connectivity(&project), Vec::new());
    assert!(layout::airwires(&project).is_empty());
    Ok(())
}

/// A branch off a routed track to an open end is a stub; a branch on to a
/// pin is not.
#[test]
fn a_branch_to_no_pad_is_a_stub_and_a_tee_into_the_run_is_not() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, R1_2, R2_1, top())?;
    let branch = track(&mut project, [17.0, 10.0], [17.0, 14.0], top())?;
    let found = connectivity(&project);
    let [stub] = codes(&found, "board.track_dangling")
        .try_into()
        .map_err(|f: Vec<Finding>| Error::Invalid(format!("one stub: {f:?}")))?;
    assert_eq!(stub.objects.first(), Some(&ObjectRef::Track { id: branch }));
    let graph = CopperGraph::of(&project);
    let [s] = graph
        .stubs()
        .try_into()
        .map_err(|s: Vec<super::Stub>| Error::Invalid(format!("one stub: {s:?}")))?;
    assert!((s.length - 4.0).abs() < 1e-9, "{s:?}");
    assert_eq!(s.open_ends, vec![[17.0, 14.0]]);
    Ok(())
}

/// A via joining nothing is `board.via_dangling`; one joined only to
/// other copper reaching no pad says so; a via a stub ends at is named in
/// that stub instead.
#[test]
fn vias_dangle_alone_or_without_a_pad_and_are_otherwise_named_in_stubs() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, R1_2, R2_1, top())?;
    let lone = via(&mut project, [30.0, 20.0], ViaSpan::Through)?;
    let pair = [
        via(&mut project, [40.0, 20.0], ViaSpan::Through)?,
        via(&mut project, [40.3, 20.0], ViaSpan::Through)?,
    ];
    let found = connectivity(&project);
    let dangling = codes(&found, "board.via_dangling");
    assert_eq!(dangling.len(), 3, "{dangling:?}");
    let message = |id: Id| {
        dangling
            .iter()
            .find(|f| f.objects.first() == Some(&ObjectRef::Via { id }))
            .map(|f| f.message.clone())
            .unwrap_or_default()
    };
    assert!(message(lone).ends_with("joins no copper of its net"));
    for id in pair {
        assert!(message(id).ends_with("reaches no pad of its net"));
    }
    assert_eq!(codes(&found, "board.track_dangling"), Vec::new());
    Ok(())
}

fn pad(index: u32, x: f64) -> WorldPad {
    WorldPad {
        instance: Id::from(index),
        pad: PadId(1),
        terminal: Endpoint {
            instance: Id::from(index),
            terminal: TerminalId(1),
        },
        net: SHARED,
        position: [x, 0.0],
        size: [1.0, 1.0],
        shape: PadShape::Rect { size: [1.0, 1.0] },
        rotation_deg: 0.0,
        layer: Layer::Top,
        through: false,
        mirrored: false,
    }
}

/// Airwires join the nearest pads between islands (a minimum spanning
/// tree), not the first pads in document order: pads at 0, 10 and 1 mm,
/// each its own island, give 0–1 and 1–10, not 0–10 and 10–1.
#[test]
fn airwires_join_the_nearest_pads_between_islands() {
    let pads = [pad(1, 0.0), pad(2, 10.0), pad(3, 1.0)];
    let wires: Vec<(f64, f64)> = spanning(&pads, &[0, 1, 2])
        .into_iter()
        .map(|(a, b)| (a.position[0], b.position[0]))
        .collect();
    assert_eq!(wires, vec![(0.0, 1.0), (1.0, 10.0)]);
    // Pads sharing an island need no airwire between them.
    assert_eq!(spanning(&pads, &[0, 1, 0]).len(), 1);
    assert!(spanning(&pads, &[4, 4, 4]).is_empty());
}

/// Past its joint limit the graph is partial: the checks say so with one
/// `board.check_limit` and report no connectivity a missing joint could
/// make false. Nothing stops silently.
#[test]
fn a_partial_graph_reports_the_check_limit_once() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, R1_2, R2_1, top())?;
    for _ in 0..100 {
        via(&mut project, [30.0, 20.0], ViaSpan::Through)?;
    }
    let graph = CopperGraph::of(&project);
    assert!(!graph.complete());
    assert_eq!(
        graph
            .findings(&project)
            .iter()
            .map(|f| f.code.as_str())
            .collect::<Vec<_>>(),
        vec!["board.check_limit"]
    );
    let all = layout::checks(&project);
    assert_eq!(codes(&all, "board.check_limit").len(), 1, "{all:?}");
    Ok(())
}

/// The engine builds the graph once per revision and drops it with the
/// revision.
#[test]
fn the_engine_caches_the_graph_per_revision() -> Result<()> {
    use crate::layout::{LayoutCommand, RouteRequest};
    use crate::{Engine, Operation};
    let mut file = Engine::demo()?.to_file();
    file.project = fixture()?;
    let mut engine = Engine::from_file(file)?;
    let first = engine.copper_graph();
    assert!(std::sync::Arc::ptr_eq(&first, &engine.copper_graph()));
    assert_eq!(first.airwires().len(), 1);
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net: SHARED,
                start: [11.2, 10.0],
                end: [23.8, 10.0],
                width: 0.3,
                layer: top(),
            },
        }),
    )?;
    let routed = engine.copper_graph();
    assert!(!std::sync::Arc::ptr_eq(&first, &routed));
    assert!(routed.airwires().is_empty());
    engine.undo()?;
    assert_eq!(engine.copper_graph().airwires().len(), 1);
    Ok(())
}

/// A fill of net 2 on Top over `outline`, refilled, with `settings`.
fn filled(project: &mut Project, outline: Outline, settings: FillSettings) -> Result<Id> {
    let id = project.allocate()?;
    project.fills.push(Fill::from_spec(
        id,
        FillSpec {
            name: String::new(),
            net: Some(SHARED),
            layer: top(),
            outline,
            priority: 0,
            settings,
        },
    ));
    let computed = layout::fill::compute(project, id)?;
    if let Some(fill) = project.fills.iter_mut().find(|f| f.id == id) {
        fill.computed = Some(computed);
    }
    Ok(id)
}

/// The fill's (kept islands, removed islands).
fn islands(project: &Project, id: Id) -> Option<(usize, u32)> {
    let c = project.fill(id)?.computed.as_ref()?;
    Some((c.regions.len(), c.removed_islands))
}

/// Whether the graph puts `object`'s copper and every island of fill `id`
/// in one island.
fn joined_to_fill(project: &Project, object: ElementKind, id: Id) -> bool {
    let graph = CopperGraph::of(project);
    let island = |pick: &dyn Fn(ElementKind) -> bool| -> Vec<Option<usize>> {
        graph
            .elements()
            .iter()
            .enumerate()
            .filter(|(_, e)| pick(e.kind))
            .map(|(i, _)| graph.island_of(graph.vertices_of(i).start))
            .collect()
    };
    let theirs = island(&|k| k == object);
    let fill = island(&|k| matches!(k, ElementKind::Fill { fill, .. } if fill == id));
    !fill.is_empty()
        && fill
            .iter()
            .all(|f| f.is_some() && theirs.iter().any(|t| t == f))
}

/// The one joining rule against the fill's old centre-line sampling: a
/// track whose copper (not its centre line) overlaps an island joins it.
/// The island starts 0.1 mm above the track's centre line, inside its
/// 0.15 mm half-width. Sampling the centre line found no join and removed
/// the island as unconnected; the copper overlaps, so it is kept, joined
/// and not reported.
#[test]
fn a_track_overlapping_an_island_only_at_its_edge_joins_it() -> Result<()> {
    let mut project = fixture()?;
    track(&mut project, R1_2, R2_1, top())?;
    let id = filled(
        &mut project,
        rect(14.0, 10.1, 20.0, 14.0),
        FillSettings::default(),
    )?;
    assert_eq!(islands(&project, id), Some((1, 0)));
    assert!(joined_to_fill(
        &project,
        ElementKind::Track { index: 0 },
        id
    ));
    assert_eq!(layout::checks(&project), Vec::new());
    // 0.06 mm further up, the island is clear of the copper: removed.
    let mut apart = fixture()?;
    track(&mut apart, R1_2, R2_1, top())?;
    let id = filled(
        &mut apart,
        rect(14.0, 10.16, 20.0, 14.0),
        FillSettings::default(),
    )?;
    assert_eq!(islands(&apart, id), Some((0, 1)));
    Ok(())
}

/// The one joining rule against the fill's old pad and via probes: a
/// solidly joined via whose land overlaps an island by 0.01 mm, its centre
/// outside the island, joins it. The probe looked at the centre and at
/// the land's outline pulled 5 % (0.015 mm) inward, missed the overlap and
/// removed the island; the copper overlaps, so it is kept and joined. Kept
/// on request, an island that nothing touches is reported by the graph.
#[test]
fn a_via_land_overlapping_an_island_joins_it() -> Result<()> {
    use crate::model::fill::{IslandRemoval, PadConnections};
    use crate::rules::PadConnection;
    let mut project = fixture()?;
    track(&mut project, R1_2, R2_1, top())?;
    via(&mut project, [17.0, 16.29], ViaSpan::Through)?;
    let solid = FillSettings {
        connections: PadConnections {
            via: Some(PadConnection::Solid),
            ..Default::default()
        },
        ..FillSettings::default()
    };
    let outline = rect(14.0, 13.0, 20.0, 16.0);
    let id = filled(&mut project, outline.clone(), solid)?;
    assert_eq!(islands(&project, id), Some((1, 0)));
    assert!(joined_to_fill(&project, ElementKind::Via { index: 0 }, id));
    let found = layout::checks(&project);
    assert_eq!(
        codes(&found, "fill.island_unconnected"),
        Vec::new(),
        "{found:?}"
    );
    // Without the via the island joins nothing: removed, or kept and
    // reported when the fill keeps every island.
    project.vias.clear();
    project.fills.clear();
    let id = filled(&mut project, outline.clone(), solid)?;
    assert_eq!(islands(&project, id), Some((0, 1)));
    project.fills.clear();
    let keep = FillSettings {
        islands: IslandRemoval::Never,
        ..solid
    };
    filled(&mut project, outline, keep)?;
    let found = layout::checks(&project);
    let [dead] = codes(&found, "fill.island_unconnected")
        .try_into()
        .map_err(|f: Vec<Finding>| Error::Invalid(format!("one finding: {f:?}")))?;
    assert!(
        dead.message.contains("has 1 unconnected copper islands"),
        "{}",
        dead.message
    );
    Ok(())
}
