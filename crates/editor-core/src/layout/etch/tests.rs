//! Etch hazards on hand-built boards. Expected angles, gaps and apexes are
//! derived by hand from the track coordinates and the rules (0.2 mm copper
//! and fill clearance, 0.2 mm min fill width, 0.3 mm edge clearance).
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::Point;
use crate::ids::Id;
use crate::layout::CopperGraph;
use crate::layout::fill::{self, RefillTarget};
use crate::library::{Outline, Segment};
use crate::model::fill::{FillSettings, FillSpec, Hatch, IslandRemoval};
use crate::model::{CopperLayer, Fill, Project, Track};
use crate::rules::{RuleKey, RuleValue, RulesCommand};
use crate::{Engine, Operation};

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The showcase board with no copper of its own and no net classes, so
/// every gap is the profile's: tracks, vias, fills and keepouts cleared;
/// `parts` keeps the placed footprints.
fn bare(parts: bool) -> Fallible<Project> {
    let mut project = Engine::showcase()?.project().clone();
    project.tracks.clear();
    project.vias.clear();
    project.fills.clear();
    project.keepouts.clear();
    project.rules.net_classes.clear();
    project.rules.class_clearances.clear();
    if !parts {
        project.placements.clear();
    }
    Ok(project)
}

fn nets(project: &Project) -> Fallible<[Id; 3]> {
    let ids: Vec<Id> = project.nets.iter().map(|n| n.id).collect();
    match ids.as_slice() {
        [a, b, c, ..] => Ok([*a, *b, *c]),
        _ => Err("the showcase has fewer than three nets".into()),
    }
}

fn track(project: &mut Project, net: Id, layer: CopperLayer, a: Point, b: Point) -> Fallible<Id> {
    let id = project.allocate()?;
    project.tracks.push(Track {
        id,
        net,
        start: a,
        end: b,
        width: 0.25,
        layer,
        locked: false,
    });
    Ok(id)
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Outline {
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

/// Add a fill and fill every fill.
fn pour(project: &mut Project, net: Id, layer: CopperLayer, outline: Outline) -> Fallible<Id> {
    // Every island kept: the tests' pours have no copper of their own net.
    let settings = FillSettings {
        islands: IslandRemoval::Never,
        ..FillSettings::default()
    };
    pour_with(project, net, layer, outline, settings)
}

fn pour_with(
    project: &mut Project,
    net: Id,
    layer: CopperLayer,
    outline: Outline,
    settings: FillSettings,
) -> Fallible<Id> {
    let id = project.allocate()?;
    project.fills.push(Fill::from_spec(
        id,
        FillSpec {
            name: String::new(),
            net: Some(net),
            layer,
            region: crate::model::fill::FillRegion::Drawn,
            outline,
            priority: 0,
            settings,
        },
    ));
    refill(project)?;
    Ok(id)
}

fn refill(project: &mut Project) -> Fallible {
    for (id, copper) in fill::refill(project, RefillTarget::All)? {
        if let Some(f) = project.fills.iter_mut().find(|f| f.id == id) {
            f.computed = Some(copper);
        }
    }
    Ok(())
}

fn set(project: &mut Project, key: RuleKey, value: RuleValue) -> Fallible {
    crate::rules::apply(&mut project.rules, RulesCommand::SetOverride { key, value })?;
    Ok(())
}

fn etch(project: &Project) -> Vec<Finding> {
    super::checks(project, &CopperGraph::of(project))
}

fn codes(findings: &[Finding], code: &str) -> usize {
    findings.iter().filter(|f| f.code == code).count()
}

/// The point the message places a finding at, "(x, y) mm".
fn location(f: &Finding) -> Fallible<Point> {
    let open = f.message.rfind(" at (").ok_or("no location")? + 5;
    let close = open + f.message[open..].find(')').ok_or("no location")?;
    let mut parts = f.message[open..close].trim_end_matches(" mm").split(", ");
    let x: f64 = parts.next().ok_or("no x")?.parse()?;
    let y: f64 = parts
        .next()
        .ok_or("no y")?
        .trim_end_matches(" mm")
        .parse()?;
    Ok([x, y])
}

fn near(p: Point, q: Point, tol: f64) -> bool {
    (p[0] - q[0]).hypot(p[1] - q[1]) <= tol
}

#[test]
fn unset_rules_check_nothing() -> Fallible {
    let mut project = bare(false)?;
    let [a, ..] = nets(&project)?;
    let t30 = (30f64).to_radians().tan();
    track(
        &mut project,
        a,
        CopperLayer::Top,
        [10.0, 10.0],
        [20.0, 10.0],
    )?;
    track(
        &mut project,
        a,
        CopperLayer::Top,
        [10.0, 10.0],
        [20.0, 10.0 + 10.0 * t30],
    )?;
    assert_eq!(
        project.rules.effective().etch,
        crate::rules::EtchRules::default()
    );
    assert!(etch(&project).is_empty());
    // Unset rules are left out of the saved rules.
    let saved = serde_json::to_string(&project.rules)?;
    assert!(!saved.contains("etch"), "{saved}");
    Ok(())
}

/// Two tracks of one net leave (10, 10) along 0° and 30°: a 30° wedge.
/// The facing sides (0.125 mm off each centre line) cross on the bisector
/// at 0.125 / sin 15° = 0.483 mm: (10.4665, 10.125).
#[test]
fn an_acute_junction_is_one_finding_at_its_apex() -> Fallible {
    let mut project = bare(false)?;
    let [a, ..] = nets(&project)?;
    let t30 = (30f64).to_radians().tan();
    let one = track(
        &mut project,
        a,
        CopperLayer::Top,
        [10.0, 10.0],
        [20.0, 10.0],
    )?;
    let two = track(
        &mut project,
        a,
        CopperLayer::Top,
        [10.0, 10.0],
        [20.0, 10.0 + 10.0 * t30],
    )?;
    set(
        &mut project,
        RuleKey::CopperAngleMin,
        RuleValue::Angle(90.0),
    )?;
    let found = etch(&project);
    assert_eq!(codes(&found, "board.acute_copper"), 1, "{found:?}");
    let f = found.first().ok_or("no finding")?;
    assert!(f.objects.contains(&ObjectRef::Track { id: one }));
    assert!(f.objects.contains(&ObjectRef::Track { id: two }));
    assert!(
        f.message.starts_with("Copper angle 30° < 90°"),
        "{}",
        f.message
    );
    let apex = [10.0 + 0.125 / (15f64).to_radians().tan(), 10.125];
    assert!(near(location(f)?, apex, 2e-3), "{} vs {apex:?}", f.message);
    // 30° is not under a 30° minimum, nor under 20°.
    set(
        &mut project,
        RuleKey::CopperAngleMin,
        RuleValue::Angle(30.0),
    )?;
    assert_eq!(codes(&etch(&project), "board.acute_copper"), 0);
    Ok(())
}

/// A right-angle corner, a straight run split in two and a T are not
/// acute under 90°; a 60° T arm is (once, its sharp side).
#[test]
fn right_angles_and_straight_joins_are_not_acute() -> Fallible {
    let mut project = bare(false)?;
    let [a, b, _] = nets(&project)?;
    set(
        &mut project,
        RuleKey::CopperAngleMin,
        RuleValue::Angle(90.0),
    )?;
    track(&mut project, a, CopperLayer::Top, [5.0, 5.0], [15.0, 5.0])?;
    track(&mut project, a, CopperLayer::Top, [15.0, 5.0], [15.0, 15.0])?;
    track(
        &mut project,
        a,
        CopperLayer::Top,
        [15.0, 15.0],
        [25.0, 15.0],
    )?;
    track(
        &mut project,
        a,
        CopperLayer::Top,
        [20.0, 15.0],
        [20.0, 25.0],
    )?;
    assert!(etch(&project).is_empty(), "{:?}", etch(&project));
    let c60 = (60f64).to_radians();
    let arm = track(
        &mut project,
        b,
        CopperLayer::Bottom,
        [30.0, 20.0],
        [30.0 + 5.0 * c60.cos(), 20.0 + 5.0 * c60.sin()],
    )?;
    track(
        &mut project,
        b,
        CopperLayer::Bottom,
        [25.0, 20.0],
        [35.0, 20.0],
    )?;
    let found = etch(&project);
    assert_eq!(codes(&found, "board.acute_copper"), 1, "{found:?}");
    assert!(
        found
            .iter()
            .all(|f| f.objects.contains(&ObjectRef::Track { id: arm }))
    );
    assert!(
        found
            .iter()
            .all(|f| f.message.starts_with("Copper angle 60°"))
    );
    Ok(())
}

/// A pad of a placed part with a straight side, axis-aligned on the board,
/// that a 30°-entering track's sides cross on that side: (pad, entry
/// point) where the entry runs from the left at 30° to the pad's centre.
fn rect_pad(project: &Project) -> Fallible<crate::layout::WorldPad> {
    use crate::library::PadShape;
    let h = 0.125 / (30f64).to_radians().cos();
    crate::layout::pads(project)
        .into_iter()
        .find(|p| {
            let quarter = crate::geometry::is_quarter_turn(p.rotation_deg);
            let (size, r) = match p.shape {
                PadShape::Rect { size } => (size.to_mm_f64(), 0.0),
                PadShape::RoundRect { size, .. } => (size.to_mm_f64(), p.shape.corner_radius()),
                _ => return false,
            };
            let turned = (p.rotation_deg.rem_euclid(180.0) - 90.0).abs() < 1e-9;
            let [sx, sy] = if turned { [size[1], size[0]] } else { size };
            let rise = sx / 2.0 * (30f64).to_radians().tan() + h;
            quarter && !p.through && rise < sy / 2.0 - r - 0.01
        })
        .ok_or_else(|| "no rectangular pad wide enough on the showcase".into())
}

/// A track runs from the left into a rectangular pad's centre at 30°
/// above the x axis: its sides cross the pad's left side at 90° − 30° =
/// 60° on one side (and 120° on the other).
#[test]
fn a_track_entering_a_pad_at_a_slant_is_acute() -> Fallible {
    let mut project = bare(true)?;
    let pad = rect_pad(&project)?;
    let (s, c) = (30f64).to_radians().sin_cos();
    let start = [pad.position[0] - 3.0 * c, pad.position[1] - 3.0 * s];
    let id = track(&mut project, pad.net, pad.layer.into(), start, pad.position)?;
    set(
        &mut project,
        RuleKey::CopperAngleMin,
        RuleValue::Angle(90.0),
    )?;
    let found: Vec<Finding> = etch(&project)
        .into_iter()
        .filter(|f| f.code == "board.acute_copper")
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    let f = found.first().ok_or("none")?;
    assert_eq!(
        f.objects,
        vec![
            ObjectRef::Track { id },
            ObjectRef::Pad {
                instance: pad.instance,
                pad: pad.pad
            }
        ]
    );
    assert!(f.message.starts_with("Copper angle 60°"), "{}", f.message);
    // Straight in, perpendicular to the side: no wedge under 90°.
    let mut square = bare(true)?;
    track(
        &mut square,
        pad.net,
        pad.layer.into(),
        [pad.position[0] - 3.0, pad.position[1]],
        pad.position,
    )?;
    set(&mut square, RuleKey::CopperAngleMin, RuleValue::Angle(90.0))?;
    assert_eq!(codes(&etch(&square), "board.acute_copper"), 0);
    Ok(())
}

/// A track of the fill's net leaves a pour through its right side at 30°:
/// 60° between the track's side and the fill's edge.
#[test]
fn a_track_leaving_a_fill_at_a_slant_is_acute() -> Fallible {
    let mut project = bare(false)?;
    let [a, ..] = nets(&project)?;
    let fill = pour(
        &mut project,
        a,
        CopperLayer::Bottom,
        rect(5.0, 5.0, 25.0, 25.0),
    )?;
    let (s, c) = (30f64).to_radians().sin_cos();
    let id = track(
        &mut project,
        a,
        CopperLayer::Bottom,
        [20.0, 15.0],
        [20.0 + 12.0 * c, 15.0 + 12.0 * s],
    )?;
    refill(&mut project)?;
    set(
        &mut project,
        RuleKey::CopperAngleMin,
        RuleValue::Angle(90.0),
    )?;
    refill(&mut project)?;
    let found = etch(&project);
    assert_eq!(codes(&found, "board.acute_copper"), 1, "{found:?}");
    let f = found.first().ok_or("none")?;
    assert_eq!(
        f.objects,
        vec![ObjectRef::Track { id }, ObjectRef::Fill { id: fill }]
    );
    assert!(f.message.starts_with("Copper angle 60°"), "{}", f.message);
    // The edge is x = 25: the sharp side is the track's lower side.
    let at = location(f)?;
    assert!((at[0] - 25.0).abs() < 1e-3, "{at:?}");
    Ok(())
}

/// Two tracks of two other nets run 0.4 mm apart (edge to edge) through a
/// pour of a third: a fill needs 0.2 + 0.2 + 0.2 = 0.6 mm to flow between
/// them, so the gap stays bare.
#[test]
fn a_narrow_gap_in_a_pour_is_a_sliver() -> Fallible {
    let mut project = bare(false)?;
    let [a, b, gnd] = nets(&project)?;
    let one = track(
        &mut project,
        a,
        CopperLayer::Bottom,
        [10.0, 15.0],
        [30.0, 15.0],
    )?;
    let two = track(
        &mut project,
        b,
        CopperLayer::Bottom,
        [10.0, 15.65],
        [30.0, 15.65],
    )?;
    let fill = pour(
        &mut project,
        gnd,
        CopperLayer::Bottom,
        rect(5.0, 5.0, 35.0, 30.0),
    )?;
    set(
        &mut project,
        RuleKey::SliverWidthMin,
        RuleValue::Length(0.5),
    )?;
    refill(&mut project)?;
    let found = etch(&project);
    assert_eq!(codes(&found, "board.copper_sliver"), 1, "{found:?}");
    let f = found.first().ok_or("none")?;
    assert_eq!(
        f.objects,
        vec![
            ObjectRef::Track { id: one },
            ObjectRef::Track { id: two },
            ObjectRef::Fill { id: fill }
        ]
    );
    assert!(
        f.message.starts_with("Unfilled gap 0.4 mm < 0.5 mm"),
        "{}",
        f.message
    );
    let at = location(f)?;
    assert!((at[1] - 15.325).abs() < 1e-3, "{at:?}");
    // 0.4 mm is not under a 0.3 mm minimum.
    set(
        &mut project,
        RuleKey::SliverWidthMin,
        RuleValue::Length(0.3),
    )?;
    refill(&mut project)?;
    assert_eq!(codes(&etch(&project), "board.copper_sliver"), 0);
    Ok(())
}

/// 1.0 mm apart, the pour flows between the tracks (1.0 − 2 × 0.2 =
/// 0.6 mm ≥ 0.2 mm): under a 1.2 mm minimum the gap is narrow but filled,
/// so it is no sliver. Without a pour, a narrow gap is no sliver either.
#[test]
fn a_filled_or_unpoured_gap_is_no_sliver() -> Fallible {
    let mut project = bare(false)?;
    let [a, b, gnd] = nets(&project)?;
    track(
        &mut project,
        a,
        CopperLayer::Bottom,
        [10.0, 15.0],
        [30.0, 15.0],
    )?;
    track(
        &mut project,
        b,
        CopperLayer::Bottom,
        [10.0, 16.25],
        [30.0, 16.25],
    )?;
    track(
        &mut project,
        a,
        CopperLayer::Top,
        [10.0, 15.0],
        [30.0, 15.0],
    )?;
    track(
        &mut project,
        b,
        CopperLayer::Top,
        [10.0, 15.65],
        [30.0, 15.65],
    )?;
    pour(
        &mut project,
        gnd,
        CopperLayer::Bottom,
        rect(5.0, 5.0, 35.0, 30.0),
    )?;
    set(
        &mut project,
        RuleKey::SliverWidthMin,
        RuleValue::Length(1.2),
    )?;
    refill(&mut project)?;
    let found = etch(&project);
    assert_eq!(codes(&found, "board.copper_sliver"), 0, "{found:?}");
    Ok(())
}

/// A track 0.45 mm (edge) from the board's bottom edge inside a pour over
/// the whole board: a fill needs 0.3 + 0.2 + 0.2 = 0.7 mm there.
#[test]
fn a_narrow_gap_to_the_board_edge_is_a_sliver() -> Fallible {
    let mut project = bare(false)?;
    let [a, _, gnd] = nets(&project)?;
    let [w, h] = project.board_size;
    let y = 0.45 + 0.125;
    let id = track(&mut project, a, CopperLayer::Bottom, [10.0, y], [30.0, y])?;
    let fill = pour(&mut project, gnd, CopperLayer::Bottom, rect(0.0, 0.0, w, h))?;
    set(
        &mut project,
        RuleKey::SliverWidthMin,
        RuleValue::Length(0.6),
    )?;
    refill(&mut project)?;
    let found = etch(&project);
    assert_eq!(codes(&found, "board.copper_sliver"), 1, "{found:?}");
    let f = found.first().ok_or("none")?;
    assert_eq!(
        f.objects,
        vec![
            ObjectRef::Track { id },
            ObjectRef::Board,
            ObjectRef::Fill { id: fill }
        ]
    );
    assert!(
        f.message.starts_with("Unfilled gap 0.45 mm"),
        "{}",
        f.message
    );
    Ok(())
}

/// A clean layout under both rules: square corners, a track leaving a
/// pour square to its edge, other-net tracks far apart in the pour.
#[test]
fn a_clean_layout_has_no_etch_findings() -> Fallible {
    let mut project = bare(false)?;
    let [a, b, gnd] = nets(&project)?;
    track(
        &mut project,
        a,
        CopperLayer::Bottom,
        [10.0, 10.0],
        [20.0, 10.0],
    )?;
    track(
        &mut project,
        a,
        CopperLayer::Bottom,
        [20.0, 10.0],
        [20.0, 20.0],
    )?;
    track(
        &mut project,
        b,
        CopperLayer::Bottom,
        [10.0, 22.0],
        [30.0, 22.0],
    )?;
    track(
        &mut project,
        gnd,
        CopperLayer::Bottom,
        [25.0, 15.0],
        [40.0, 15.0],
    )?;
    pour(
        &mut project,
        gnd,
        CopperLayer::Bottom,
        rect(5.0, 5.0, 35.0, 30.0),
    )?;
    set(
        &mut project,
        RuleKey::CopperAngleMin,
        RuleValue::Angle(90.0),
    )?;
    set(
        &mut project,
        RuleKey::SliverWidthMin,
        RuleValue::Length(0.6),
    )?;
    refill(&mut project)?;
    let found = etch(&project);
    assert!(found.is_empty(), "{found:?}");
    Ok(())
}

/// Fill compute removes solid copper narrower than the min fill width (an
/// opening), so a solid fill has none: opening its copper again by half
/// the width keeps its area. A hatch narrower than that width is drawn
/// after the opening and is reported.
#[test]
fn fill_slivers_are_removed_and_thin_hatches_are_reported() -> Fallible {
    use crate::layout::fill::geom;
    let mut project = bare(false)?;
    let [a, b, gnd] = nets(&project)?;
    track(
        &mut project,
        a,
        CopperLayer::Bottom,
        [10.0, 15.0],
        [30.0, 15.0],
    )?;
    track(
        &mut project,
        b,
        CopperLayer::Bottom,
        [10.0, 15.9],
        [30.0, 15.9],
    )?;
    let fill = pour(
        &mut project,
        gnd,
        CopperLayer::Bottom,
        rect(5.0, 5.0, 35.0, 30.0),
    )?;
    let copper = project
        .fill(fill)
        .and_then(|f| f.computed.as_ref())
        .ok_or("not filled")?;
    let regions: Vec<geom::Region> = copper
        .regions
        .iter()
        .map(|r| {
            std::iter::once(r.outer.clone())
                .chain(r.holes.iter().cloned())
                .collect()
        })
        .collect();
    let half = 0.1 - 2e-4;
    let opened = geom::offset(&geom::offset(&regions, -half, 0.001), half, 0.001);
    let area = |rs: &[geom::Region]| rs.iter().map(geom::region_area).sum::<f64>();
    let (before, after) = (area(&regions), area(&opened));
    assert!((before - after).abs() < 1e-3 * before, "{before} {after}");
    assert_eq!(codes(&etch(&project), "board.copper_sliver"), 0);
    let mut hatched = bare(false)?;
    let thin = pour_with(
        &mut hatched,
        gnd,
        CopperLayer::Bottom,
        rect(5.0, 5.0, 35.0, 30.0),
        FillSettings {
            hatch: Some(Hatch {
                width: 0.15,
                gap: 0.5,
                angle_deg: 45.0,
            }),
            islands: IslandRemoval::Never,
            ..FillSettings::default()
        },
    )?;
    let found = etch(&hatched);
    assert_eq!(codes(&found, "board.copper_sliver"), 1, "{found:?}");
    assert_eq!(
        found.first().map(|f| f.objects.clone()),
        Some(vec![ObjectRef::Fill { id: thin }])
    );
    Ok(())
}

/// The rules are edited through the engine's commands, bounded and undone.
#[test]
fn the_rules_are_commands_with_bounds() -> Fallible {
    let mut engine = Engine::showcase()?;
    let set = |engine: &mut Engine, key, value| {
        engine.apply(
            engine.revision(),
            Operation::Rules(RulesCommand::SetOverride { key, value }),
        )
    };
    set(&mut engine, RuleKey::CopperAngleMin, RuleValue::Angle(60.0))?;
    set(
        &mut engine,
        RuleKey::SliverWidthMin,
        RuleValue::Length(0.35),
    )?;
    let etch = engine.project().rules.effective().etch;
    assert_eq!((etch.copper_angle_min, etch.sliver_width_min), (60.0, 0.35));
    let before = engine.project().rules.clone();
    assert!(
        set(
            &mut engine,
            RuleKey::CopperAngleMin,
            RuleValue::Angle(181.0)
        )
        .is_err()
    );
    assert!(set(&mut engine, RuleKey::CopperAngleMin, RuleValue::Angle(-1.0)).is_err());
    assert!(
        set(
            &mut engine,
            RuleKey::SliverWidthMin,
            RuleValue::Length(-0.1)
        )
        .is_err()
    );
    assert!(set(&mut engine, RuleKey::SliverWidthMin, RuleValue::Angle(1.0)).is_err());
    assert_eq!(engine.project().rules, before);
    engine.undo()?;
    assert_eq!(
        engine.project().rules.effective().etch.sliver_width_min,
        0.0
    );
    Ok(())
}

/// The check stays cheap on the 10,000-via benchmark board with both rules
/// set and its vias in a pour. Prints the times; the budget is a target.
#[test]
fn the_check_is_cheap_on_ten_thousand_vias() -> Fallible {
    let mut project = crate::layout::via::tests::via_board(10_000, 2_000)?;
    let [w, h] = project.board_size;
    let gnd = project.nets.first().map(|n| n.id).ok_or("no net")?;
    let fill_start = std::time::Instant::now();
    pour(&mut project, gnd, CopperLayer::Bottom, rect(0.0, 0.0, w, h))?;
    let fill_time = fill_start.elapsed();
    set(
        &mut project,
        RuleKey::CopperAngleMin,
        RuleValue::Angle(90.0),
    )?;
    set(
        &mut project,
        RuleKey::SliverWidthMin,
        RuleValue::Length(0.6),
    )?;
    refill(&mut project)?;
    let graph = CopperGraph::of(&project);
    let mut best = std::time::Duration::MAX;
    let mut found = Vec::new();
    for _ in 0..3 {
        let start = std::time::Instant::now();
        found = super::checks(&project, &graph);
        best = best.min(start.elapsed());
    }
    let time = |f: &dyn Fn()| {
        let start = std::time::Instant::now();
        f();
        start.elapsed()
    };
    let angles = time(&|| super::acute(&project, &graph, 90.0, &mut Vec::new()));
    let slivers = time(&|| super::slivers(&project, &graph, 0.6, &mut Vec::new()));
    let fresh = time(&|| {
        fill::fresh(&project);
    });
    let start = std::time::Instant::now();
    let all = crate::layout::checks_with(&project, &graph, &Default::default());
    let board = start.elapsed();
    println!(
        "etch checks {best:?} (angles {angles:?}, slivers {slivers:?}, of which fresh fills {fresh:?} each; {} findings); all board checks {board:?}; fill {fill_time:?}",
        found.len()
    );
    for f in &found {
        println!("  {}: {}", f.code, f.message);
    }
    assert!(all.len() >= found.len());
    assert!(best < std::time::Duration::from_secs(1), "{best:?}");
    Ok(())
}

/// The etch rules are read by checks only: setting them leaves every fill
/// fresh (its inputs key unchanged), so no refill is needed to check.
#[test]
fn etch_rules_leave_fills_fresh() -> Fallible {
    let mut project = bare(false)?;
    let [a, ..] = nets(&project)?;
    let id = pour(
        &mut project,
        a,
        CopperLayer::Bottom,
        rect(5.0, 5.0, 25.0, 25.0),
    )?;
    let before = fill::inputs_key(&project, id)?;
    set(
        &mut project,
        RuleKey::CopperAngleMin,
        RuleValue::Angle(90.0),
    )?;
    set(
        &mut project,
        RuleKey::SliverWidthMin,
        RuleValue::Length(0.6),
    )?;
    assert_eq!(fill::inputs_key(&project, id)?, before);
    let f = project.fill(id).ok_or("no fill")?;
    assert!(!fill::is_stale(&project, f));
    // A rule fills read still moves the key.
    set(&mut project, RuleKey::FillWidthMin, RuleValue::Length(0.3))?;
    assert_ne!(fill::inputs_key(&project, id)?, before);
    Ok(())
}
