use super::super::{NativeModel, build};
use super::{MODEL_CHORD_MM, simplify_loop};
use crate::material::BodyKind;
use crate::scene::Geometry;
use crate::{Error, Result};
use pcbcraft_core::model::fill::{FilledCopper, FilledRegion};
use pcbcraft_core::{CopperLayer, Engine, Id, Point, Project};

/// The showcase with its pours unfilled: the showcase refills them, and
/// these tests give the pours known copper of their own.
fn showcase() -> Result<Project> {
    let mut project = Engine::showcase()
        .map_err(|e| Error::Verification(e.to_string()))?
        .project()
        .clone();
    for fill in &mut project.fills {
        fill.computed = None;
    }
    Ok(project)
}

/// A counter-clockwise rectangle.
fn rect(lo: Point, hi: Point) -> Vec<Point> {
    vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
}

/// A clockwise circle of `n` points.
fn hole(center: Point, radius: f64, n: usize) -> Vec<Point> {
    (0..n)
        .rev()
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
        })
        .collect()
}

fn copper(regions: Vec<FilledRegion>) -> FilledCopper {
    FilledCopper {
        key: "test".into(),
        regions,
        joins: vec![],
        removed_islands: 0,
    }
}

/// The showcase pour on `layer`.
fn pour(project: &mut Project, layer: CopperLayer) -> Result<&mut pcbcraft_core::Fill> {
    project
        .fills
        .iter_mut()
        .find(|f| f.layer == layer)
        .ok_or_else(|| Error::Verification(format!("no pour on {}", layer.name())))
}

/// The copper bodies of fill `fill`: one per region, all of its source.
fn fill_bodies(model: &NativeModel, fill: Id) -> Result<Vec<&Geometry>> {
    let source = model
        .sources
        .iter()
        .find(|s| s.fill == Some(fill))
        .ok_or_else(|| Error::Verification("no fill source".into()))?;
    Ok(model
        .geometry
        .iter()
        .filter(|g| g.item == Some(source.pick_id) && g.kind == BodyKind::FillCopper)
        .collect())
}

/// The bounds and total volume of `bodies`.
fn extent(bodies: &[&Geometry]) -> ([f64; 3], [f64; 3], f64) {
    bodies.iter().fold(
        ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3], 0.0),
        |(lo, hi, v), g| {
            (
                std::array::from_fn(|k| lo[k].min(g.min[k])),
                std::array::from_fn(|k| hi[k].max(g.max[k])),
                v + g.volume(),
            )
        },
    )
}

/// The unsigned area of a loop (shoelace).
fn area(points: &[Point]) -> f64 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f64>()
        .abs()
        / 2.0
}

fn near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected}"
    );
}

/// A bottom pour of two regions (one with two holes) and an inner plane:
/// each is one saved copper body on its layer, as thick as its foil and
/// as large as its regions, its source naming the fill, net and layer.
/// The inner plane lies in its foil level, so the laminate loses exactly
/// its copper ([`an_inner_fill_lies_in_its_own_foil_level`]); the bottom
/// pour wears a display-only mask cap opened at its holes.
#[test]
fn fill_copper_is_one_saved_body_per_fill_on_its_layer() -> Result<()> {
    let plain = showcase()?;
    let unfilled = build(&plain)?;
    assert!(
        unfilled
            .geometry
            .iter()
            .all(|g| g.kind != BodyKind::FillCopper),
        "unfilled pours build no copper"
    );
    let mut project = plain.clone();
    let bottom = pour(&mut project, CopperLayer::Bottom)?;
    let bottom_id = bottom.id;
    bottom.computed = Some(copper(vec![
        FilledRegion {
            outer: rect([30.0, 1.0], [40.0, 4.0]),
            holes: vec![hole([32.0, 2.5], 0.5, 48), hole([36.0, 2.5], 0.5, 48)],
        },
        FilledRegion {
            outer: rect([30.0, 4.5], [34.0, 6.0]),
            holes: vec![],
        },
    ]));
    let inner = pour(&mut project, CopperLayer::Inner(1))?;
    let inner_id = inner.id;
    inner.computed = Some(copper(vec![FilledRegion {
        outer: rect([30.0, 1.0], [40.0, 4.0]),
        holes: vec![],
    }]));
    let model = build(&project)?;
    let stackup = &project.stackup;
    let thickness = |layer| {
        stackup
            .index(layer)
            .and_then(|i| stackup.copper.get(i))
            .map(|c| c.thickness_um / 1000.0)
            .ok_or_else(|| Error::Verification("foil".into()))
    };
    let t_bottom = thickness(CopperLayer::Bottom)?;
    let t_inner = thickness(CopperLayer::Inner(1))?;

    let bodies = fill_bodies(&model, bottom_id)?;
    assert_eq!(bodies.len(), 2, "one body per region");
    assert!(bodies.iter().all(|g| g.layer == Some(CopperLayer::Bottom)));
    let (lo, hi, volume) = extent(&bodies);
    // The holes as the model simplifies them (MODEL_CHORD_MM).
    let disc = area(&simplify_loop(&hole([32.0, 2.5], 0.5, 48), MODEL_CHORD_MM));
    let area = 30.0 - 2.0 * disc + 6.0;
    near(volume, area * t_bottom, 1e-6);
    near(hi[2], 0.0, 1e-9);
    near(lo[2], -t_bottom, 1e-9);
    near(lo[0], 30.0, 1e-9);
    near(hi[0], 40.0, 1e-9);

    let plane = fill_bodies(&model, inner_id)?;
    assert_eq!(plane.len(), 1);
    assert!(plane.iter().all(|g| g.layer == Some(CopperLayer::Inner(1))));
    let (lo, hi, volume) = extent(&plane);
    near(volume, 30.0 * t_inner, 1e-6);
    let span = stackup
        .copper_span(CopperLayer::Inner(1))
        .ok_or_else(|| Error::Verification("inner span".into()))?;
    near(lo[2], span.0, 1e-9);
    near(hi[2], span.1, 1e-9);

    let source = model
        .sources
        .iter()
        .find(|s| s.fill == Some(bottom_id))
        .ok_or_else(|| Error::Verification("source".into()))?;
    let gnd = project.fills.first().and_then(|f| f.net);
    assert_eq!(source.net, gnd);
    assert_eq!(source.layer, Some(CopperLayer::Bottom));
    assert!(source.track.is_none() && source.via.is_none() && source.instance.is_none());

    // The inner plane takes its place in the laminate.
    near(
        substrate(&unfilled) - substrate(&model),
        30.0 * t_inner,
        1e-6,
    );

    // The bottom pour's mask cap: display-only, picked as the fill, one
    // per region, below the board.
    let caps: Vec<&Geometry> = model
        .geometry
        .iter()
        .filter(|g| g.item == Some(source.pick_id) && g.kind == BodyKind::MaskCap)
        .collect();
    assert_eq!(caps.len(), 2);
    assert!(
        caps.iter()
            .all(|g| g.kind.is_appearance() && g.max[2] <= 1e-9)
    );
    assert!(caps.iter().all(|g| g.min[2] < -t_bottom));
    Ok(())
}

fn substrate(m: &NativeModel) -> f64 {
    m.geometry
        .iter()
        .filter(|g| g.kind == BodyKind::Substrate)
        .map(|g| g.volume())
        .sum()
}

/// An inner plane is copper in its own foil level, not copper overlapping
/// the laminate: the laminate loses exactly the plane's copper, less a
/// same-net via ring inside it (cut already), and keeps the laminate in
/// the plane's clearance round another net's via, whose ring stays its
/// exact circle. So the saved bodies are disjoint and the laminate's
/// mass is physical.
#[test]
fn an_inner_fill_lies_in_its_own_foil_level() -> Result<()> {
    let mut plain = showcase()?;
    let layer = CopperLayer::Inner(1);
    let (lo, hi) = ([30.0, 1.0], [40.0, 4.0]);
    let clear = |a: Point, b: Point, r: f64| {
        a[0].max(b[0]) + r < lo[0] - 1.0
            || a[0].min(b[0]) - r > hi[0] + 1.0
            || a[1].max(b[1]) + r < lo[1] - 1.0
            || a[1].min(b[1]) - r > hi[1] + 1.0
    };
    let layers = plain.stackup.layers();
    assert!(
        plain
            .tracks
            .iter()
            .filter(|t| t.layer == layer)
            .all(|t| clear(t.start, t.end, t.width / 2.0))
            && plain
                .vias
                .iter()
                .filter(|v| v.on(layer, &layers))
                .all(|v| clear(v.position, v.position, v.diameter / 2.0)),
        "the plane's area holds no showcase copper of its layer"
    );
    let net = pour(&mut plain, layer)?
        .net
        .ok_or_else(|| Error::Verification("plane net".into()))?;
    let other = plain
        .nets
        .iter()
        .map(|n| n.id)
        .find(|n| *n != net)
        .ok_or_else(|| Error::Verification("second net".into()))?;
    for (net, position) in [(net, [33.0, 2.5]), (other, [37.0, 2.5])] {
        let id = plain
            .allocate()
            .map_err(|e| Error::Verification(e.to_string()))?;
        plain.vias.push(pcbcraft_core::Via {
            id,
            net,
            position,
            drill: 0.3,
            diameter: 0.6,
            span: pcbcraft_core::ViaSpan::Through,
            tented: true,
            locked: false,
        });
    }
    let clearance = hole([37.0, 2.5], 0.6, 96);
    let mut project = plain.clone();
    pour(&mut project, layer)?.computed = Some(copper(vec![FilledRegion {
        outer: rect(lo, hi),
        holes: vec![clearance.clone()],
    }]));
    let (before, after) = (build(&plain)?, build(&project)?);
    let foil = project
        .stackup
        .index(layer)
        .and_then(|i| project.stackup.copper.get(i))
        .map(|c| c.thickness_um / 1000.0)
        .ok_or_else(|| Error::Verification("foil".into()))?;
    let plane = 30.0 - area(&simplify_loop(&clearance, MODEL_CHORD_MM));
    let ring = std::f64::consts::PI * 0.3 * 0.3;
    near(
        substrate(&before) - substrate(&after),
        (plane - ring) * foil,
        1e-5,
    );
    // The plane's copper is the rest: its body is the region.
    let id = pour(&mut project, layer)?.id;
    let (_, _, copper) = extent(&fill_bodies(&after, id)?);
    near(copper, plane * foil, 1e-6);
    Ok(())
}

/// A pour of many pad clearances builds as one body: one sketch command
/// and one extrusion, not a command or a Boolean per hole.
/// Prints the build time with and without it.
#[test]
fn a_pour_with_many_clearances_is_one_body() -> Result<()> {
    let plain = showcase()?;
    let start = std::time::Instant::now();
    build(&plain)?;
    let base = start.elapsed();
    let mut project = plain.clone();
    let holes: Vec<Vec<Point>> = (0..20)
        .flat_map(|i| (0..10).map(move |j| (i, j)))
        .map(|(i, j)| {
            hole(
                [2.0 + 2.0 * f64::from(i), 2.0 + 2.4 * f64::from(j)],
                0.45,
                32,
            )
        })
        .collect();
    let count = holes.len();
    let bottom = pour(&mut project, CopperLayer::Bottom)?;
    let id = bottom.id;
    bottom.computed = Some(copper(vec![FilledRegion {
        outer: rect([1.0, 1.0], [43.0, 27.0]),
        holes,
    }]));
    let start = std::time::Instant::now();
    let model = build(&project)?;
    let filled = start.elapsed();
    eprintln!("showcase build {base:?}; with a {count}-hole pour {filled:?}");
    let bodies = fill_bodies(&model, id)?;
    assert_eq!(bodies.len(), 1);
    let (_, _, volume) = extent(&bodies);
    let disc = area(&simplify_loop(&hole([2.0, 2.0], 0.45, 32), MODEL_CHORD_MM));
    let area = 42.0 * 26.0 - count as f64 * disc;
    let (z0, z1) = project
        .stackup
        .copper_span(CopperLayer::Bottom)
        .ok_or_else(|| Error::Verification("bottom span".into()))?;
    near(volume, area * (z1 - z0), 1e-4);
    Ok(())
}

/// The showcase as shipped (sketched board, three pours, all filled)
/// builds within 1.25× of the same board with its pours unfilled (plus a
/// second for timer noise): the fills must not dominate the 3D view's
/// build, as the inner plane's laminate cut once did (about 6×).
#[test]
#[ignore = "speed bar not met on SMLib: a pour's extrude is slow until upstream's solidcore fixes land (founder hold on speed work, 2026-11-05)"]
fn the_showcase_fills_cost_little_of_the_build() -> Result<()> {
    let filled = Engine::showcase_hosted(pcbcraft_core::Evaluators {
        footprint: Some(&crate::footprint::SketchEvaluator),
        board: Some(&crate::board::SketchBoardEvaluator),
    })
    .map_err(|e| Error::Verification(e.to_string()))?
    .project()
    .clone();
    assert_eq!(filled.fills.len(), 3);
    assert!(
        filled.fills.iter().all(|f| f.computed.is_some()),
        "the showcase ships its pours filled"
    );
    let mut plain = filled.clone();
    for fill in &mut plain.fills {
        fill.computed = None;
    }
    let time = |p: &Project| -> Result<std::time::Duration> {
        let start = std::time::Instant::now();
        build(p)?;
        Ok(start.elapsed())
    };
    // Warm both once; take the faster of two runs each.
    let base = time(&plain)?.min(time(&plain)?);
    let with = time(&filled)?.min(time(&filled)?);
    eprintln!("showcase build unfilled {base:?}, filled {with:?}");
    assert!(
        with.as_secs_f64() <= base.as_secs_f64() * 1.25 + 1.0,
        "filled {with:?} against unfilled {base:?}"
    );
    Ok(())
}

/// A loop's simplification drops collinear points and arc points within
/// the tolerance, keeps corners and keeps every point within the
/// tolerance of the original boundary.
#[test]
fn model_loops_drop_points_within_the_chord_tolerance() {
    // A 4 × 2 rectangle sampled every 0.01 mm along its edges.
    let mut rect = Vec::new();
    let steps = |n: usize| (0..n).map(move |k| k as f64 / n as f64);
    rect.extend(steps(400).map(|t| [4.0 * t, 0.0]));
    rect.extend(steps(200).map(|t| [4.0, 2.0 * t]));
    rect.extend(steps(400).map(|t| [4.0 - 4.0 * t, 2.0]));
    rect.extend(steps(200).map(|t| [0.0, 2.0 - 2.0 * t]));
    let simple = simplify_loop(&rect, MODEL_CHORD_MM);
    assert_eq!(simple.len(), 4, "{simple:?}");
    for corner in [[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [0.0, 2.0]] {
        assert!(simple.contains(&corner), "{corner:?} kept");
    }
    // A circle of radius 0.5 at 5 µm chords keeps far fewer points, each
    // on the circle, and every dropped point lies within the tolerance
    // of the simplified polygon's chords.
    let circle = hole([0.0, 0.0], 0.5, 400);
    let simple = simplify_loop(&circle, MODEL_CHORD_MM);
    assert!(simple.len() < 40 && simple.len() >= 8, "{}", simple.len());
    let max_sagitta = simple
        .iter()
        .zip(simple.iter().cycle().skip(1))
        .map(|(a, b)| {
            let half = (b[0] - a[0]).hypot(b[1] - a[1]) / 2.0;
            0.5 - (0.25 - half * half).sqrt()
        })
        .fold(0.0_f64, f64::max);
    assert!(max_sagitta <= MODEL_CHORD_MM + 1e-9, "{max_sagitta}");
}
