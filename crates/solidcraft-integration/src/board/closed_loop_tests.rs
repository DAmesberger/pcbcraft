//! Slice S1 (docs/board-sketch-references.md §5.1): the outline is the
//! board sketch's one closed region, free-form curves reach the solved
//! outline as fitted arcs within the `board.outline_fit` rule, and a sketch
//! whose curves do not make one board keeps its last valid shape and
//! reports why. Ignored until S1 implements them; S1 removes each
//! `#[ignore]` without weakening the expected values.
#![allow(clippy::unwrap_used)]

use super::tests::{Fallible, apply, edit, sketch, sketched};
use super::*;
use pcbcraft_core::fabrication::PreflightReport;
use pcbcraft_core::issues::Severity;
use pcbcraft_core::layout::outline::{BoardEdit, SHAPE_INVALID, SHAPE_OPEN};
use pcbcraft_core::library::footprint_source::SketchRef;
use pcbcraft_core::model::board::{CutoutKind, flatten};
use pcbcraft_core::{Engine, Operation, Outcome, Query, QueryResult};

/// The curve of the rectangle sketch's named edge.
fn edge_curve(engine: &Engine, name: &str) -> Fallible<SketchCurveId> {
    sketch(engine)?
        .edges
        .iter()
        .find(|e| e.name.0 == name)
        .map(|e| e.curve.clone())
        .ok_or_else(|| format!("no edge {name}").into())
}

/// Delete all four rectangle lines.
fn delete_rectangle(engine: &Engine) -> Fallible<BoardEdit> {
    let mut ids = Vec::new();
    for name in RECT_EDGES {
        ids.push(SketchRef::Curve(edge_curve(engine, name)?));
    }
    Ok(BoardEdit::Sketch {
        edit: SketchEdit::Delete { ids },
    })
}

fn codes(engine: &Engine, code: &str) -> Vec<Finding> {
    pcbcraft_core::layout::outline::checks(engine.project())
        .into_iter()
        .filter(|f| f.code == code)
        .collect()
}

fn preflight(engine: &Engine) -> Fallible<PreflightReport> {
    match engine.query(Query::Preflight { profile: None })? {
        QueryResult::Preflight(report) => Ok(*report),
        _ => Err("preflight".into()),
    }
}

fn created(outcome: Outcome) -> Vec<SketchCurveId> {
    match outcome {
        Outcome::BoardEdited { created } => created
            .into_iter()
            .filter_map(|r| match r {
                SketchRef::Curve(c) => Some(c),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Distance from `p` to the closed polygon `poly`.
fn distance_to(poly: &[Point], p: Point) -> f64 {
    let mut best = f64::INFINITY;
    for (k, a) in poly.iter().enumerate() {
        let b = poly[(k + 1) % poly.len()];
        let d = [b[0] - a[0], b[1] - a[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let t = if len2 > 0.0 {
            (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = [a[0] + d[0] * t, a[1] + d[1] * t];
        best = best.min((p[0] - q[0]).hypot(p[1] - q[1]));
    }
    best
}

/// Deleting an outline line is no longer refused: the curves stop
/// closing, the board keeps its last valid shape and size, and
/// `board.shape_open` names both open ends. Outside Board shape mode it is
/// an error that blocks fabrication output. A line drawn between the ends
/// closes the loop again (ends join within the loop join tolerance) and the
/// finding goes.
#[test]
fn deleting_an_outline_line_opens_the_loop_and_keeps_the_last_shape() -> Fallible<()> {
    let mut engine = sketched()?;
    let before = engine.project().board().into_owned();
    let bottom = edge_curve(&engine, "bottom")?;
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::Delete {
                ids: vec![SketchRef::Curve(bottom)],
            },
        }],
    )?;
    assert_eq!(engine.project().board_size, [60.0, 40.0]);
    assert_eq!(engine.project().board().into_owned(), before);
    let open = codes(&engine, SHAPE_OPEN);
    assert_eq!(open.len(), 1, "{open:#?}");
    assert!(open[0].message.contains("(0, 0)"), "{}", open[0].message);
    assert!(open[0].message.contains("(60, 0)"), "{}", open[0].message);
    // A whole sentence, so the card's "The board keeps its last shape."
    // reads after it.
    assert!(
        open[0].message.ends_with("close it."),
        "{}",
        open[0].message
    );
    let report = preflight(&engine)?;
    assert!(
        report
            .items
            .iter()
            .any(|i| i.code == SHAPE_OPEN && i.severity == Severity::Error && i.blocks())
    );
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddLine {
                a: [0.0, 0.0],
                b: [60.0, 0.0],
                construction: false,
            },
        }],
    )?;
    assert!(codes(&engine, SHAPE_OPEN).is_empty());
    assert_eq!(
        engine.project().board().into_owned().bounds(),
        before.bounds()
    );
    Ok(())
}

/// The outline is whatever closed region the curves make; no curve list
/// names it. Replacing the rectangle by a closed pentagon in one edit makes
/// the pentagon the board.
#[test]
fn the_outline_is_the_closed_region_not_a_curve_list() -> Fallible<()> {
    let mut engine = sketched()?;
    let delete = delete_rectangle(&engine)?;
    edit(
        &mut engine,
        vec![
            delete,
            BoardEdit::Sketch {
                edit: SketchEdit::AddPolyline {
                    points: vec![
                        [0.0, 0.0],
                        [50.0, 0.0],
                        [50.0, 30.0],
                        [25.0, 45.0],
                        [0.0, 30.0],
                    ],
                    closed: true,
                    construction: false,
                },
            },
        ],
    )?;
    assert_eq!(engine.project().board_size, [50.0, 45.0]);
    let board = engine.project().board().into_owned();
    assert_eq!(board.outer.segments.len(), 5);
    assert!(board.contains([25.0, 40.0], 0.001));
    assert!(codes(&engine, SHAPE_OPEN).is_empty());
    assert!(codes(&engine, SHAPE_INVALID).is_empty());
    Ok(())
}

/// A self-crossing outer loop is not a board: reported, last shape kept.
#[test]
fn a_self_crossing_outline_is_invalid_and_keeps_the_last_shape() -> Fallible<()> {
    let mut engine = sketched()?;
    let before = engine.project().board().into_owned();
    let delete = delete_rectangle(&engine)?;
    edit(
        &mut engine,
        vec![
            delete,
            BoardEdit::Sketch {
                edit: SketchEdit::AddPolyline {
                    points: vec![[0.0, 0.0], [40.0, 30.0], [40.0, 0.0], [0.0, 30.0]],
                    closed: true,
                    construction: false,
                },
            },
        ],
    )?;
    assert_eq!(engine.project().board().into_owned(), before);
    assert_eq!(engine.project().board_size, [60.0, 40.0]);
    assert_eq!(codes(&engine, SHAPE_INVALID).len(), 1);
    Ok(())
}

/// A hole must lie strictly inside the outer loop: one touching the left
/// edge is reported, and the board keeps its last shape (no cutout).
#[test]
fn a_hole_touching_the_outline_is_invalid() -> Fallible<()> {
    let mut engine = sketched()?;
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddCircle {
                center: [5.0, 20.0],
                radius: 5.0,
                construction: false,
            },
        }],
    )?;
    assert_eq!(codes(&engine, SHAPE_INVALID).len(), 1);
    assert!(engine.project().board().cutouts.is_empty());
    Ok(())
}

/// Every hole is a cutout without being named one; `SetCutoutKind`, keyed
/// by the hole's curves, makes it a slot and back.
#[test]
fn inner_loops_are_cutouts_and_set_cutout_kind_makes_a_slot() -> Fallible<()> {
    let mut engine = sketched()?;
    let circle = created(edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddCircle {
                center: [30.0, 20.0],
                radius: 3.0,
                construction: false,
            },
        }],
    )?);
    let cutouts = engine.project().board().into_owned().cutouts;
    assert_eq!(cutouts.len(), 1);
    assert_eq!(cutouts[0].kind, CutoutKind::Cutout);
    assert!(!engine.project().board().contains([30.0, 20.0], 0.001));
    edit(
        &mut engine,
        vec![BoardEdit::SetCutoutKind {
            curves: circle.clone(),
            kind: CutoutKind::Slot { plated: true },
        }],
    )?;
    assert_eq!(
        engine.project().board().cutouts[0].kind,
        CutoutKind::Slot { plated: true }
    );
    edit(
        &mut engine,
        vec![BoardEdit::SetCutoutKind {
            curves: circle,
            kind: CutoutKind::Cutout,
        }],
    )?;
    assert_eq!(engine.project().board().cutouts[0].kind, CutoutKind::Cutout);
    Ok(())
}

/// A closed (periodic) fit spline is the whole outline. It reaches the
/// solved outline as lines and arcs (what fills, checks and Gerber read)
/// passing through every fit point within the default `board.outline_fit`
/// of 0.01 mm.
#[test]
fn a_spline_outline_is_fitted_with_arcs_within_the_fit_tolerance() -> Fallible<()> {
    let mut engine = sketched()?;
    // The periodic spline through these swings 7.8 mm below its lowest fit
    // points (to y = 7.18), so the board stays above the origin.
    let points = vec![
        [10.0, 15.0],
        [50.0, 15.0],
        [55.0, 35.0],
        [30.0, 50.0],
        [5.0, 35.0],
    ];
    let delete = delete_rectangle(&engine)?;
    edit(
        &mut engine,
        vec![
            delete,
            BoardEdit::AddSpline {
                points: points.clone(),
                control: false,
                periodic: true,
                construction: false,
            },
        ],
    )?;
    let board = engine.project().board().into_owned();
    assert!(
        board
            .outer
            .segments
            .iter()
            .any(|s| matches!(s, Segment::Arc { .. }))
    );
    let poly = flatten(&board.outer, 1e-5);
    for p in points {
        let d = distance_to(&poly, p);
        assert!(d <= 0.01 + 1e-5, "{p:?} is {d} mm off the outline");
    }
    assert!(codes(&engine, SHAPE_OPEN).is_empty());
    Ok(())
}

/// An ellipse inside the board is a cutout whose fitted outline spans its
/// axes within the fit tolerance; overriding `board.outline_fit` to 1 µm
/// fits it closer, with more segments.
#[test]
fn an_ellipse_cutout_is_fitted_within_the_outline_fit_rule() -> Fallible<()> {
    let mut engine = sketched()?;
    edit(
        &mut engine,
        vec![BoardEdit::AddEllipse {
            center: [30.0, 20.0],
            major: [40.0, 20.0],
            minor_radius: 5.0,
            construction: false,
        }],
    )?;
    let spans = |engine: &Engine, tol: f64| -> Fallible<usize> {
        let board = engine.project().board().into_owned();
        let cutout = board.cutouts.first().ok_or("ellipse cutout")?;
        let b = pcbcraft_core::model::board::outline_bounds(&cutout.outline);
        for (got, want) in [
            (b.min[0], 20.0),
            (b.min[1], 15.0),
            (b.max[0], 40.0),
            (b.max[1], 25.0),
        ] {
            assert!((got - want).abs() <= tol, "{got} vs {want}");
        }
        Ok(cutout.outline.segments.len())
    };
    let coarse = spans(&engine, 0.01)?;
    let rule: Operation = serde_json::from_value(serde_json::json!({
        "editor": "Rules",
        "operation": {
            "command": "rules.set_override",
            "params": {
                "key": {"rule": "board_outline_fit"},
                "value": {"kind": "length", "value": 0.001}
            }
        }
    }))?;
    apply(&mut engine, rule)?;
    let fine = spans(&engine, 0.001)?;
    assert!(fine > coarse, "{fine} segments at 1 µm, {coarse} at 10 µm");
    Ok(())
}

/// A conic closed by a line is a cutout; with `rho` 0.5 (a parabola) its
/// shoulder lies halfway from the chord's midpoint to the apex.
#[test]
fn a_conic_closed_by_a_line_is_a_cutout() -> Fallible<()> {
    let mut engine = sketched()?;
    edit(
        &mut engine,
        vec![
            BoardEdit::AddConic {
                a: [20.0, 10.0],
                b: [40.0, 10.0],
                apex: [30.0, 25.0],
                rho: 0.5,
                construction: false,
            },
            BoardEdit::Sketch {
                edit: SketchEdit::AddLine {
                    a: [40.0, 10.0],
                    b: [20.0, 10.0],
                    construction: false,
                },
            },
        ],
    )?;
    let board = engine.project().board().into_owned();
    let cutout = board.cutouts.first().ok_or("conic cutout")?;
    let b = pcbcraft_core::model::board::outline_bounds(&cutout.outline);
    assert!((b.min[1] - 10.0).abs() <= 0.01);
    assert!((b.max[1] - 17.5).abs() <= 0.01, "{}", b.max[1]);
    assert!((b.min[0] - 20.0).abs() <= 0.01 && (b.max[0] - 40.0).abs() <= 0.01);
    Ok(())
}

/// Distance from `p` to the ellipse centred at `c` with semi-axes `a`
/// (along x) and `b` (along y): Newton on the foot point's parameter.
fn ellipse_distance(c: Point, a: f64, b: f64, p: Point) -> f64 {
    let (x, y) = (p[0] - c[0], p[1] - c[1]);
    let mut t = (y / b).atan2(x / a);
    for _ in 0..50 {
        let (s, k) = t.sin_cos();
        // f(t) = (E(t) - p) · E'(t) with E(t) = (a cos t, b sin t).
        let f = (a * k - x) * (-a * s) + (b * s - y) * (b * k);
        let df = (a * a - b * b) * (s * s - k * k) + x * a * k + y * b * s;
        if df.abs() < 1e-300 {
            break;
        }
        let step = f / df;
        t -= step;
        if step.abs() < 1e-15 {
            break;
        }
    }
    (a * t.cos() - x).hypot(b * t.sin() - y)
}

/// The fitted arcs stand for the analytic curve within `board.outline_fit`
/// both ways: every point of the fitted cutout lies within the fit of the
/// true ellipse, and every point of the ellipse within the fit of the
/// fitted cutout, at the default 10 µm and at an overridden 1 µm.
#[test]
fn fitted_arcs_follow_the_analytic_ellipse_within_the_fit() -> Fallible<()> {
    let (c, a, b) = ([30.0, 20.0], 10.0, 5.0);
    let mut engine = sketched()?;
    edit(
        &mut engine,
        vec![BoardEdit::AddEllipse {
            center: c,
            major: [c[0] + a, c[1]],
            minor_radius: b,
            construction: false,
        }],
    )?;
    let worst = |engine: &Engine| -> Fallible<(f64, f64)> {
        let board = engine.project().board().into_owned();
        let cutout = board.cutouts.first().ok_or("ellipse cutout")?;
        // Flattened a thousand times finer than the fit, so the chords add
        // nothing measurable.
        let fitted = flatten(&cutout.outline, 1e-6);
        let off_curve = fitted
            .iter()
            .map(|p| ellipse_distance(c, a, b, *p))
            .fold(0.0, f64::max);
        let off_fit = (0..2000)
            .map(|k| {
                let t = std::f64::consts::TAU * f64::from(k) / 2000.0;
                distance_to(&fitted, [c[0] + a * t.cos(), c[1] + b * t.sin()])
            })
            .fold(0.0, f64::max);
        Ok((off_curve, off_fit))
    };
    let (curve, fit) = worst(&engine)?;
    assert!(
        curve <= 0.01 + 1e-6,
        "fitted points {curve} mm off the ellipse"
    );
    assert!(
        fit <= 0.01 + 1e-6,
        "the ellipse {fit} mm off the fitted arcs"
    );
    let rule: Operation = serde_json::from_value(serde_json::json!({
        "editor": "Rules",
        "operation": {
            "command": "rules.set_override",
            "params": {
                "key": {"rule": "board_outline_fit"},
                "value": {"kind": "length", "value": 0.001}
            }
        }
    }))?;
    apply(&mut engine, rule)?;
    let (curve, fit) = worst(&engine)?;
    assert!(
        curve <= 0.001 + 1e-6,
        "fitted points {curve} mm off the ellipse"
    );
    assert!(
        fit <= 0.001 + 1e-6,
        "the ellipse {fit} mm off the fitted arcs"
    );
    Ok(())
}
