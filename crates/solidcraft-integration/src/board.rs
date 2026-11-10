//! The board shape evaluator: [`SketchBoardEvaluator`] implements the
//! core's [`BoardShapeEvaluator`] on the same Solidcraft sketch machinery
//! as the footprint evaluator ([`crate::footprint`]): the canonical sketch
//! check, parameters in Solidcraft's expression language, bound
//! dimensions and the solver. It turns the solved sketch into the board's
//! outer outline, cutouts and slots, mounting holes and named edges, and
//! solves dimensioned parts against the solved board ([`parts`]).
//!
//! The outline is whatever closed region the drawn curves make
//! (Solidcraft's `find_drawn_profiles`): its outer loop is the outline and
//! each hole a cutout, unless a slot role names the hole's curves. Splines,
//! ellipses and conics reach the solved board fitted with lines and arcs
//! within `board.outline_fit` (Solidcraft's arc fitting), which is all that
//! fills, checks, fabrication output and the laminate read. Curves that do
//! not make one board (open ends from `loose_ends`, crossings, more than
//! one region) leave the last valid shape in place with a [`ShapeFault`].
//!
//! Corners are rounded by real sketch fillets ([`fillet`](SketchBoardEvaluator)):
//! the two lines are shortened to a tangent arc and the corner stays as
//! their virtual intersection (a point on both lines), so dimensions to it
//! keep holding and a named edge still runs corner to corner.

use crate::geom::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use crate::geom::Vec2;
use pcbcraft_core::diagnostics::{Error, Finding, ObjectRef, Result};
use pcbcraft_core::geometry::Point;
use pcbcraft_core::layout::dimensions::{PartsRequest, PartsSolution};
use pcbcraft_core::layout::outline::{
    BoardEvaluation, BoardShapeEvaluator, DrawnSlot, FATAL_PREFIX, Fillets, FreeFormCurve,
    SHAPE_UNSOLVED, refuses,
};
use pcbcraft_core::library::footprint_source::{
    DimValue, EvaluatorRef, Expr, FootprintParam, ParamName, ParamUnit, SketchBlob, SketchCurveId,
    SketchEdit, SketchOutcome, SketchPointId, SolveStatus, SolveSummary,
};
use pcbcraft_core::library::{MAX_OUTLINE_SEGMENTS, Outline, Segment};
use pcbcraft_core::model::board::{
    BoardSketch, CutoutKind, EdgeName, NamedEdge, RECT_EDGES, ShapeFault, ShapeFaultKind,
    SolvedBoard, SolvedCutout, SolvedEdge, SolvedMountingHole,
};
use pcbcraft_core::rules::{DEFAULT_OUTLINE_FIT_MM, OUTLINE_FIT_RANGE_MM};
use solidcraft_engine::sketch::{
    ConstraintKind, CurveKind, ENDPOINT_JOIN_TOL, MAX_SPLINE_POINTS, Sketch, find_drawn_profiles,
    loose_ends,
};

use crate::geom::Seg2;

use crate::footprint::check::{self, MAX_COORD_MM};
use crate::footprint::evaluate::{fnv, q, solve};
use crate::footprint::params::{self, Values};
use crate::footprint::{ENGINE, Outcome, Problem, evaluator_ref, problem};

/// Two loop ends closer than this join, mm: where Solidcraft's profile
/// search joins them.
const JOIN_MM: f64 = ENDPOINT_JOIN_TOL;
/// Crossings or open ends one fault names at most.
const FAULT_POINTS: usize = 8;

const OUTLINE: &str = "board.shape_outline";
const CURVE: &str = "board.shape_curve";
const HOLE: &str = "board.shape_hole";
const EDGE: &str = "board.shape_edge";
const RANGE: &str = "board.shape_range";
const EDIT: &str = "board.sketch_edit";

/// Solidcraft's sketch solver as Pcbcraft's board shape evaluator.
/// Stateless; every call is pure in its inputs.
#[derive(Clone, Copy, Debug, Default)]
pub struct SketchBoardEvaluator;

fn sketch_error(e: impl std::fmt::Display) -> Problem {
    problem(EDIT, e.to_string())
}

fn mm(v: f64) -> Expr {
    Expr(crate::native::mm(v))
}

impl BoardShapeEvaluator for SketchBoardEvaluator {
    fn evaluator(&self) -> EvaluatorRef {
        evaluator_ref()
    }

    fn rectangle(&self, size: Point) -> Result<BoardSketch> {
        let [w, h] = size;
        if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0)
            || w > MAX_COORD_MM
            || h > MAX_COORD_MM
        {
            return Err(Error::Invalid(format!(
                "board.size: {w} × {h} mm is not a board size"
            )));
        }
        let mut sk = Sketch::new();
        let corners = [Vec2::new(w, 0.0), Vec2::new(w, h), Vec2::new(0.0, h)];
        let mut pts = vec![0];
        for c in corners {
            pts.push(sk.add_point(c, None).map_err(sketch_error)?);
        }
        let mut lines = Vec::new();
        for k in 0..4 {
            let (a, b) = (pts[k], pts[(k + 1) % 4]);
            lines.push(sk.add_line_pts(a, b, None).map_err(sketch_error)?);
        }
        let kinds = [
            (ConstraintKind::Horizontal { l: lines[0] }, None),
            (ConstraintKind::Vertical { l: lines[1] }, None),
            (ConstraintKind::Horizontal { l: lines[2] }, None),
            (ConstraintKind::Vertical { l: lines[3] }, None),
            (
                ConstraintKind::DistanceX {
                    p: 0,
                    q: pts[1],
                    value: w,
                },
                Some("width".to_owned()),
            ),
            (
                ConstraintKind::DistanceY {
                    p: pts[1],
                    q: pts[2],
                    value: h,
                },
                Some("height".to_owned()),
            ),
        ];
        for (kind, param) in kinds {
            sk.add_constraint(kind, param).map_err(sketch_error)?;
        }
        let ids: Vec<SketchCurveId> = lines
            .iter()
            .filter_map(|l| sk.curves.get(*l).map(|c| SketchCurveId(c.id.clone())))
            .collect();
        let param = |name: &str, v: f64, comment: &str| FootprintParam {
            name: ParamName(name.into()),
            expr: mm(v),
            unit: ParamUnit::Mm,
            comment: comment.into(),
        };
        let mut board = BoardSketch {
            evaluator: evaluator_ref(),
            sketch: SketchBlob(check::canonical(&sk)?),
            parameters: vec![
                param("width", w, "Board width"),
                param("height", h, "Board height"),
            ],
            cutouts: Vec::new(),
            holes: Vec::new(),
            edges: RECT_EDGES
                .iter()
                .zip(&ids)
                .map(|(name, curve)| NamedEdge {
                    name: EdgeName((*name).into()),
                    curve: curve.clone(),
                })
                .collect(),
            next_feature: 1,
            solve: SolveSummary::default(),
            solved: SolvedBoard::rectangle(size),
            outline_fit: DEFAULT_OUTLINE_FIT_MM,
            fault: None,
            idle: Vec::new(),
        };
        let evaluation = self.evaluate(&board)?;
        if let Some(f) = evaluation.findings.iter().find(|f| refuses(&f.code)) {
            return Err(Error::Invalid(format!("{}: {}", f.code, f.message)));
        }
        board.solve = evaluation.solve;
        board.solved = evaluation.solved;
        if let Some(s) = evaluation.sketch_out {
            board.sketch = s;
        }
        Ok(board)
    }

    fn apply_sketch(&self, sketch: &SketchBlob, edit: &SketchEdit) -> Result<SketchOutcome> {
        let stored = check::check_board_sketch(&sketch.0)?;
        let edited = crate::footprint::edit::apply(&stored, edit)?;
        Ok(finish(&stored, &edited)?)
    }

    fn evaluate(&self, board: &BoardSketch) -> Result<BoardEvaluation> {
        if board.evaluator.engine != ENGINE {
            return Err(Error::Unsupported(format!(
                "board.evaluator: the board sketch was made with {}, this build has {ENGINE}",
                board.evaluator.engine
            )));
        }
        let stored = check::check_board_sketch(&board.sketch.0)?;
        Ok(match run(board, &stored) {
            Ok(evaluation) => evaluation,
            Err(p) => unsolved(board, p),
        })
    }

    fn fillet(
        &self,
        board: &BoardSketch,
        corners: &[SketchPointId],
        radius: &DimValue,
    ) -> Result<Fillets> {
        Ok(fillet(board, corners, radius)?)
    }

    fn slot(&self, board: &BoardSketch, a: Point, b: Point, width: &DimValue) -> Result<DrawnSlot> {
        Ok(slot(board, a, b, width)?)
    }

    fn add_curve(&self, board: &BoardSketch, curve: &FreeFormCurve) -> Result<SketchOutcome> {
        Ok(free_form(board, curve)?)
    }

    fn solve_parts(&self, request: &PartsRequest<'_>) -> Result<PartsSolution> {
        parts::solve(request)
    }

    fn trace_model(
        &self,
        request: &pcbcraft_core::layout::dimensions::model::TraceRequest<'_>,
    ) -> Result<pcbcraft_core::layout::dimensions::model::Traced> {
        model_refs::trace(request)
    }

    fn pick_model(
        &self,
        request: &pcbcraft_core::layout::dimensions::model::PickRequest<'_>,
    ) -> Result<pcbcraft_core::layout::dimensions::model::ModelPicked> {
        model_refs::pick(request)
    }
}

/// A failed solve: the last solved shape kept, a fatal finding.
fn unsolved(board: &BoardSketch, p: Problem) -> BoardEvaluation {
    let status = if p.code == "footprint.solve_failed" {
        SolveStatus::OverConstrained
    } else {
        SolveStatus::Failed
    };
    let code = if p.code.starts_with(FATAL_PREFIX) {
        p.code
    } else {
        SHAPE_UNSOLVED
    };
    BoardEvaluation {
        solved: board.solved.clone(),
        solve: SolveSummary {
            evaluator: evaluator_ref(),
            status,
            ..SolveSummary::default()
        },
        findings: vec![Finding::new(code, p.message, vec![ObjectRef::Board])],
        fault: None,
        idle: Vec::new(),
        sketch_out: None,
    }
}

/// One span of a loop, in loop order: a line or a counter-clockwise
/// (positive bulge) or clockwise arc.
#[derive(Clone, Debug)]
struct Piece {
    curve: String,
    a: Vec2,
    b: Vec2,
    bulge: f64,
}

impl Piece {
    fn reversed(&self) -> Self {
        Self {
            curve: self.curve.clone(),
            a: self.b,
            b: self.a,
            bulge: -self.bulge,
        }
    }
}

fn pos(sk: &Sketch, i: usize) -> Outcome<Vec2> {
    sk.point(i)
        .ok_or_else(|| problem(CURVE, "a curve names a missing point"))
}

/// A curve as loop pieces in its direction of travel: a line, an arc, a
/// circle as two half arcs, or a free-form curve fitted with lines and arcs
/// within `fit` mm.
fn pieces_of(sk: &Sketch, id: &str, fit: f64) -> Outcome<Vec<Piece>> {
    let index = sk
        .curve_index(id)
        .ok_or_else(|| problem(CURVE, format!("no sketch curve `{id}`")))?;
    let c = sk
        .curves
        .get(index)
        .ok_or_else(|| problem(CURVE, format!("no sketch curve `{id}`")))?;
    let piece = |a, b, bulge| Piece {
        curve: id.to_owned(),
        a,
        b,
        bulge,
    };
    Ok(match c.kind {
        CurveKind::Line { a, b } => vec![piece(pos(sk, a)?, pos(sk, b)?, 0.0)],
        CurveKind::Arc { c, a, b } => {
            let (pc, pa, pb) = (pos(sk, c)?, pos(sk, a)?, pos(sk, b)?);
            let sweep = ((pb - pc).angle() - (pa - pc).angle()).rem_euclid(TAU);
            if sweep < 1e-9 {
                return Err(problem(CURVE, format!("arc `{id}` has no sweep")));
            }
            vec![piece(pa, pb, (sweep / 4.0).tan())]
        }
        CurveKind::Circle { c, r } => {
            let pc = pos(sk, c)?;
            let (e, w) = (pc + Vec2::new(r, 0.0), pc - Vec2::new(r, 0.0));
            vec![piece(e, w, 1.0), piece(w, e, 1.0)]
        }
        _ => {
            let mut out = Vec::new();
            for seg in sk.exact_pieces(index) {
                let fitted = seg.fit_arcs(fit).ok_or_else(|| {
                    problem(
                        OUTLINE,
                        format!("curve `{id}` cannot be fitted with arcs within {fit} mm"),
                    )
                })?;
                for s in fitted {
                    out.push(fitted_piece(id, s)?);
                }
                if out.len() > MAX_OUTLINE_SEGMENTS {
                    return Err(problem(
                        OUTLINE,
                        format!(
                            "curve `{id}` needs more than {MAX_OUTLINE_SEGMENTS} arcs at a {fit} mm \
                             outline fit; loosen the board.outline_fit rule"
                        ),
                    ));
                }
            }
            if out.is_empty() {
                return Err(problem(CURVE, format!("curve `{id}` has no length")));
            }
            out
        }
    })
}

/// A fitted span as a loop piece.
fn fitted_piece(id: &str, s: Seg2) -> Outcome<Piece> {
    let bulge = match s {
        Seg2::Line { .. } => 0.0,
        Seg2::Arc { sweep, .. } => (sweep / 4.0).tan(),
        _ => return Err(problem(CURVE, format!("curve `{id}` was not fitted"))),
    };
    Ok(Piece {
        curve: id.to_owned(),
        a: s.start(),
        b: s.end(),
        bulge,
    })
}

/// The closed loop the curves form, joined end to end in any order and
/// direction. A closed curve (circle, ellipse, periodic spline) is a loop
/// by itself.
fn loop_of(sk: &Sketch, ids: &[String], what: &str, fit: f64) -> Outcome<Vec<Piece>> {
    let mut runs: Vec<Option<Vec<Piece>>> = Vec::new();
    for id in ids {
        let closed = sk
            .curve_index(id)
            .and_then(|c| sk.curves.get(c))
            .is_some_and(|c| c.kind.ends().is_none());
        let pieces = pieces_of(sk, id, fit)?;
        if closed {
            if ids.len() > 1 {
                return Err(problem(
                    OUTLINE,
                    format!("{what}: a closed curve is a loop by itself"),
                ));
            }
            return Ok(pieces);
        }
        runs.push(Some(pieces));
    }
    let mut rest = runs.into_iter();
    let mut out = rest
        .next()
        .flatten()
        .ok_or_else(|| problem(OUTLINE, format!("{what} has no curves")))?;
    let mut rest: Vec<Option<Vec<Piece>>> = rest.collect();
    let (Some(start), Some(mut at)) = (out.first().map(|p| p.a), out.last().map(|p| p.b)) else {
        return Err(problem(OUTLINE, format!("{what} has no curves")));
    };
    for _ in 0..rest.len() {
        let next = rest.iter_mut().find_map(|slot| {
            let run = slot.as_ref()?;
            let (a, b) = (run.first()?.a, run.last()?.b);
            let joined: Vec<Piece> = if a.dist(at) <= JOIN_MM {
                run.clone()
            } else if b.dist(at) <= JOIN_MM {
                run.iter().rev().map(Piece::reversed).collect()
            } else {
                return None;
            };
            *slot = None;
            Some(joined)
        });
        let next = next.ok_or_else(|| {
            problem(
                OUTLINE,
                format!(
                    "{what} is not one closed loop (it breaks at {:.4}, {:.4})",
                    at.x, at.y
                ),
            )
        })?;
        if let Some(last) = next.last() {
            at = last.b;
        }
        out.extend(next);
    }
    if at.dist(start) > JOIN_MM {
        return Err(problem(OUTLINE, format!("{what} does not close")));
    }
    Ok(out)
}

/// The curves of the one board region the drawn curves make: its outer
/// loop's and each hole's (sorted ids).
struct Region {
    outer: Vec<String>,
    holes: Vec<Vec<String>>,
}

/// A point as a message names it: `(12.5, 0)`, to the micrometre.
fn at_text(p: Vec2) -> String {
    let r = |v: f64| (v * 1000.0).round() / 1000.0 + 0.0;
    format!("({}, {})", r(p.x), r(p.y))
}

fn fault(kind: ShapeFaultKind, message: String, at: &[Vec2]) -> ShapeFault {
    ShapeFault {
        kind,
        message,
        at: at
            .iter()
            .take(FAULT_POINTS)
            .map(|p| [p.x + 0.0, p.y + 0.0])
            .collect(),
    }
}

/// The open ends of the drawn curves (Solidcraft's `loose_ends`): ends
/// that touch no other curve, so no loop runs through them.
fn open_ends(sk: &Sketch) -> Option<ShapeFault> {
    let mut ends: Vec<Vec2> = Vec::new();
    for p in loose_ends(sk).open {
        if let Some(at) = sk.point(p)
            && !ends.iter().any(|e| e.dist(at) <= JOIN_MM)
        {
            ends.push(at);
        }
    }
    if ends.is_empty() {
        return None;
    }
    ends.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let named: Vec<String> = ends
        .iter()
        .take(FAULT_POINTS)
        .map(|p| at_text(*p))
        .collect();
    Some(fault(
        ShapeFaultKind::Open,
        format!(
            "The board outline is open: curve ends at {} touch nothing. Join them to close it.",
            named.join(", ")
        ),
        &ends,
    ))
}

/// Where the profile search cut curves apart: the corners of `profile`'s
/// loops that are no curve's end (a crossing, or a hole touching).
fn cuts(sk: &Sketch, profile: &solidcraft_engine::sketch::Profile) -> Vec<Vec2> {
    let ends: Vec<Vec2> = sk
        .curves
        .iter()
        .filter(|c| !c.construction)
        .filter_map(|c| c.kind.ends())
        .flat_map(|(a, b)| [sk.point(a), sk.point(b)])
        .flatten()
        .collect();
    let mut out: Vec<Vec2> = Vec::new();
    let loops = std::iter::once(&profile.region.outer).chain(profile.region.holes.iter());
    for corner in loops.flat_map(|l| l.segs.iter().map(|s| s.start())) {
        if !ends.iter().any(|e| e.dist(corner) <= JOIN_MM)
            && !out.iter().any(|o| o.dist(corner) <= JOIN_MM)
        {
            out.push(corner);
        }
        if out.len() >= FAULT_POINTS {
            break;
        }
    }
    out
}

/// The one region the drawn curves make: exactly one region (holes filled
/// by plain discs aside), its curves whole (nothing crosses or touches),
/// every hole inside it and apart from the others.
fn region(sk: &Sketch) -> std::result::Result<Region, ShapeFault> {
    let profiles = find_drawn_profiles(sk);
    if profiles.is_empty() {
        return Err(fault(
            ShapeFaultKind::Invalid,
            "The board's curves make no closed region; draw a closed outline.".into(),
            &[],
        ));
    }
    let mut whole = Vec::with_capacity(profiles.len());
    for p in &profiles {
        let Some(boundaries) = p.whole_curve_boundaries() else {
            let at = cuts(sk, p);
            let named: Vec<String> = at.iter().map(|p| at_text(*p)).collect();
            let place = if named.is_empty() {
                String::new()
            } else {
                format!(" at {}", named.join(", "))
            };
            return Err(fault(
                ShapeFaultKind::Invalid,
                format!(
                    "The board's curves cross or touch{place}: the outline may not cross \
                     itself, and a cutout lies inside it apart from the others."
                ),
                &at,
            ));
        };
        whole.push(boundaries);
    }
    let set = |ids: &[String]| ids.iter().cloned().collect::<BTreeSet<String>>();
    let holes: Vec<BTreeSet<String>> = whole
        .iter()
        .flat_map(|(_, h)| h.iter().map(|h| set(h)))
        .collect();
    let main: Vec<usize> = (0..whole.len())
        .filter(|i| whole.get(*i).is_some_and(|(o, _)| !holes.contains(&set(o))))
        .collect();
    let [m] = main.as_slice() else {
        return Err(fault(
            ShapeFaultKind::Invalid,
            format!(
                "The board's curves make {} separate regions; a board is one.",
                main.len()
            ),
            &[],
        ));
    };
    let (outer, outer_holes) = whole
        .get(*m)
        .cloned()
        .ok_or_else(|| fault(ShapeFaultKind::Invalid, "No board region.".into(), &[]))?;
    let mine: Vec<BTreeSet<String>> = outer_holes.iter().map(|h| set(h)).collect();
    for (i, (o, h)) in whole.iter().enumerate() {
        if i != *m && (!h.is_empty() || !mine.contains(&set(o))) {
            return Err(fault(
                ShapeFaultKind::Invalid,
                "A region lies inside a cutout; a board is one piece.".into(),
                &[],
            ));
        }
    }
    Ok(Region {
        outer,
        holes: outer_holes,
    })
}

/// Twice the signed area of a loop (positive counter-clockwise), arcs
/// included.
fn area2(pieces: &[Piece]) -> f64 {
    pieces
        .iter()
        .map(|p| {
            let chord = p.a.cross(p.b);
            let segment = if p.bulge == 0.0 {
                0.0
            } else {
                let theta = 4.0 * p.bulge.atan();
                let c = p.a.dist(p.b);
                let r = c / (2.0 * (theta / 2.0).sin().abs());
                r * r * (theta.abs() - theta.abs().sin()) * theta.signum()
            };
            chord + segment
        })
        .sum()
}

/// The loop counter-clockwise (`ccw`) or clockwise.
fn oriented(pieces: Vec<Piece>, ccw: bool) -> Vec<Piece> {
    if (area2(&pieces) > 0.0) == ccw {
        pieces
    } else {
        pieces.iter().rev().map(Piece::reversed).collect()
    }
}

fn qv(v: Vec2) -> Outcome<Point> {
    Ok([q(v.x)?, q(v.y)?])
}

fn qbulge(b: f64) -> f64 {
    (b * 1e9).round() / 1e9 + 0.0
}

fn outline_of(pieces: &[Piece]) -> Outcome<Outline> {
    let first = pieces
        .first()
        .ok_or_else(|| problem(OUTLINE, "an empty loop"))?;
    let start = qv(first.a)?;
    let mut segments = Vec::with_capacity(pieces.len());
    for (k, p) in pieces.iter().enumerate() {
        // The last span ends exactly where the loop started.
        let to = if k + 1 == pieces.len() {
            start
        } else {
            qv(p.b)?
        };
        segments.push(if p.bulge == 0.0 {
            Segment::Line { to }
        } else {
            Segment::Arc {
                to,
                bulge: qbulge(p.bulge),
            }
        });
    }
    if segments.len() > pcbcraft_core::library::MAX_OUTLINE_SEGMENTS {
        return Err(problem(OUTLINE, "a loop has too many curves"));
    }
    Ok(Outline { start, segments })
}

/// Lines each point is held on (`PointOnCurve`), for virtual corners.
fn held_on(sk: &Sketch) -> BTreeMap<usize, BTreeSet<usize>> {
    let mut out: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for k in &sk.constraints {
        if let ConstraintKind::PointOnCurve { p, c } = k.kind
            && matches!(
                sk.curves.get(c).map(|c| &c.kind),
                Some(CurveKind::Line { .. })
            )
        {
            out.entry(p).or_default().insert(c);
        }
    }
    out
}

/// A straight named edge reaches out to the virtual corners on its line
/// (a filleted corner's intersection), so it still runs corner to corner.
fn extended(
    sk: &Sketch,
    curve: usize,
    piece: &Piece,
    held: &BTreeMap<usize, BTreeSet<usize>>,
) -> (Vec2, Vec2) {
    let (a, b) = (piece.a, piece.b);
    let Some(u) = (b - a).normalized() else {
        return (a, b);
    };
    let len = a.dist(b);
    let used: BTreeSet<usize> = sk.curves.iter().flat_map(|c| c.kind.point_ids()).collect();
    let (mut lo, mut hi) = (0.0_f64, len);
    for (p, lines) in held {
        if !lines.contains(&curve) || lines.len() < 2 || used.contains(p) {
            continue;
        }
        let Some(at) = sk.point(*p) else {
            continue;
        };
        let t = (at - a).dot(u);
        if t < 0.0 && (lo == 0.0 || t > lo) {
            lo = t;
        } else if t > len && (hi == len || t < hi) {
            hi = t;
        }
    }
    (a + u * lo, a + u * hi)
}

fn run(board: &BoardSketch, stored: &Sketch) -> Outcome<BoardEvaluation> {
    let values = params::evaluate(&board.parameters, &[])
        .map_err(|p| problem(SHAPE_UNSOLVED, format!("board parameters: {}", p.message)))?;
    if !OUTLINE_FIT_RANGE_MM.contains(&board.outline_fit) {
        return Err(problem(
            RANGE,
            format!("an outline fit of {} mm is out of range", board.outline_fit),
        ));
    }
    let (sk, dof) = solve(stored, &values)?;
    if sk
        .points
        .iter()
        .any(|p| p.pos.x.abs() > MAX_COORD_MM || p.pos.y.abs() > MAX_COORD_MM)
    {
        return Err(problem(
            RANGE,
            "the solved sketch leaves the coordinate range",
        ));
    }
    // Curves that are not one board keep the last valid shape; open ends
    // are named even when the closed curves make a board.
    let shaped = match region(&sk) {
        Ok(region) => shape(board, &sk, &values, &region)?,
        Err(fault) => Err(fault),
    };
    let (solved, fault) = match shaped {
        Ok(solved) => (solved, open_ends(&sk)),
        Err(fault) => (board.solved.clone(), Some(open_ends(&sk).unwrap_or(fault))),
    };
    let idle = if fault.is_none() {
        idle(board, &sk)
    } else {
        Vec::new()
    };
    let bytes =
        serde_json::to_vec(&solved).map_err(|e| problem("board.internal", e.to_string()))?;
    let dof = u32::try_from(dof).unwrap_or(u32::MAX);
    Ok(BoardEvaluation {
        solved,
        solve: SolveSummary {
            evaluator: evaluator_ref(),
            status: if dof > 0 {
                SolveStatus::UnderConstrained
            } else {
                SolveStatus::Solved
            },
            dof,
            result_hash: format!("{:016x}", fnv(&bytes)),
        },
        findings: Vec::new(),
        fault,
        sketch_out: Some(SketchBlob(check::canonical(&sk)?)),
        idle,
    })
}

/// The board parameters that drive a sketch dimension but move no outline
/// curve or mounting hole: each, nudged by 1 %, re-solved from `sk` (the
/// solved sketch) by the same solve; one whose nudge moves no point or
/// radius of a drawn curve and no hole centre drives nothing of the board.
/// A nudge that does not evaluate or solve is not reported.
fn idle(board: &BoardSketch, sk: &Sketch) -> Vec<ParamName> {
    /// Moves smaller than this are the solver's, not the nudge's, mm.
    const STILL_MM: f64 = 1e-6;
    let mut used: BTreeSet<usize> = board
        .holes
        .iter()
        .filter_map(|h| sk.point_index(&h.center.0))
        .collect();
    let radius = |k: &CurveKind| match k {
        CurveKind::Circle { r, .. } | CurveKind::Ellipse { r, .. } => *r,
        _ => 0.0,
    };
    for c in sk.curves.iter().filter(|c| !c.construction) {
        used.extend(c.kind.point_ids());
    }
    let moves = |b: &Sketch| -> bool {
        used.iter().any(|&p| {
            sk.point(p)
                .zip(b.point(p))
                .is_none_or(|(pa, pb)| pa.dist(pb) > STILL_MM)
        }) || sk
            .curves
            .iter()
            .zip(&b.curves)
            .any(|(ca, cb)| (radius(&ca.kind) - radius(&cb.kind)).abs() > STILL_MM)
    };
    let driving = sk.driving_values();
    let mut out = Vec::new();
    for (k, p) in board.parameters.iter().enumerate() {
        if p.unit == ParamUnit::Count {
            continue;
        }
        let mut nudged = board.parameters.clone();
        if let Some(n) = nudged.get_mut(k) {
            n.expr = Expr(format!("({}) * 1.01", p.expr.0));
        }
        let Ok(values) = params::evaluate(&nudged, &[]) else {
            continue;
        };
        let mut applied = sk.clone();
        if crate::footprint::evaluate::apply_values(&mut applied, &values).is_err()
            || applied.driving_values() == driving
        {
            continue;
        }
        if let Ok((moved, _)) = solve(sk, &values)
            && !moves(&moved)
        {
            out.push(p.name.clone());
        }
    }
    out
}

/// The solved board of a valid region: the outline counter-clockwise, its
/// holes as cutouts (or the slots their roles make them), the mounting
/// holes and the named edges. A slot role whose curves no longer bound one
/// hole is a fault.
fn shape(
    board: &BoardSketch,
    sk: &Sketch,
    values: &Values,
    region: &Region,
) -> Outcome<std::result::Result<SolvedBoard, ShapeFault>> {
    let fit = board.outline_fit;
    let outer = oriented(loop_of(sk, &region.outer, "the board outline", fit)?, true);
    let outer_outline = outline_of(&outer)?;
    // Edge orientation: material on the left. The outer loop runs
    // counter-clockwise, cutouts clockwise.
    let mut loops: BTreeMap<String, Piece> = BTreeMap::new();
    for p in &outer {
        loops.insert(p.curve.clone(), p.clone());
    }
    let mut cutouts = Vec::new();
    let mut matched = BTreeSet::new();
    for (k, hole) in region.holes.iter().enumerate() {
        let curves: BTreeSet<&String> = hole.iter().collect();
        let role = board
            .cutouts
            .iter()
            .find(|r| r.curves.iter().map(|c| &c.0).collect::<BTreeSet<_>>() == curves);
        if let Some(r) = role {
            matched.insert(r.id);
        }
        let what = format!("cutout {}", k + 1);
        let pieces = oriented(loop_of(sk, hole, &what, fit)?, true);
        let outline = outline_of(&pieces)?;
        for p in pieces.iter().rev() {
            loops.insert(p.curve.clone(), p.reversed());
        }
        cutouts.push(SolvedCutout {
            id: role.map(|r| r.id),
            kind: role.map_or(CutoutKind::Cutout, |r| r.kind),
            outline,
            curves: pieces
                .iter()
                .map(|p| SketchCurveId(p.curve.clone()))
                .collect(),
        });
    }
    if let Some(lost) = board.cutouts.iter().find(|r| !matched.contains(&r.id)) {
        return Ok(Err(fault(
            ShapeFaultKind::Invalid,
            format!(
                "Slot {}'s curves no longer bound one hole of the board; redraw it or make it a \
                 cutout.",
                lost.id.0
            ),
            &[],
        )));
    }
    let mut holes = Vec::new();
    for h in &board.holes {
        let what = format!("mounting hole {}", h.id.0);
        let at = sk
            .point_index(&h.center.0)
            .and_then(|i| sk.point(i))
            .ok_or_else(|| problem(HOLE, format!("{what}: no sketch point `{}`", h.center.0)))?;
        let drill = values
            .positive(&h.drill.0)
            .map_err(|p| problem(HOLE, format!("{what}: {}", p.message)))?;
        let pad = h
            .pad_diameter
            .as_ref()
            .map(|e| values.positive(&e.0))
            .transpose()
            .map_err(|p| problem(HOLE, format!("{what}: {}", p.message)))?;
        if pad.is_some_and(|d| d <= drill) {
            return Err(problem(
                HOLE,
                format!("{what}: the copper ring must be wider than the drill"),
            ));
        }
        if !h.plated && pad.is_some() {
            return Err(problem(
                HOLE,
                format!("{what}: only a plated hole has a copper ring"),
            ));
        }
        holes.push(SolvedMountingHole {
            id: h.id,
            at: qv(at)?,
            drill: q(drill)?,
            plated: h.plated,
            pad_diameter: pad.map(q).transpose()?,
        });
    }
    let held = held_on(sk);
    let mut edges = Vec::new();
    for e in &board.edges {
        let what = format!("edge `{}`", e.name.0);
        let index = sk
            .curve_index(&e.curve.0)
            .ok_or_else(|| problem(EDGE, format!("{what}: no sketch curve `{}`", e.curve.0)))?;
        let mut drawn = pieces_of(sk, &e.curve.0, fit)?;
        let straight_or_arc = matches!(
            sk.curves.get(index).map(|c| &c.kind),
            Some(CurveKind::Line { .. } | CurveKind::Arc { .. })
        );
        if drawn.len() != 1 || !straight_or_arc {
            return Err(problem(
                EDGE,
                format!("{what}: a named edge is a line or an arc"),
            ));
        }
        let piece = match loops.get(&e.curve.0) {
            Some(p) => p.clone(),
            None => drawn.remove(0),
        };
        let (a, b) = if piece.bulge == 0.0 {
            extended(sk, index, &piece, &held)
        } else {
            (piece.a, piece.b)
        };
        edges.push(SolvedEdge {
            name: e.name.clone(),
            a: qv(a)?,
            b: qv(b)?,
            bulge: qbulge(piece.bulge),
        });
    }
    Ok(Ok(SolvedBoard {
        outer: outer_outline,
        outer_curves: outer
            .iter()
            .map(|p| SketchCurveId(p.curve.clone()))
            .collect(),
        cutouts,
        holes,
        edges,
    }))
}

/// A dimension value: the number (mm) and the parameter it binds.
fn dim(values: &Values, value: &DimValue) -> Outcome<(f64, Option<String>)> {
    let (v, param) = match value {
        DimValue::Fixed(v) => (*v, None),
        DimValue::Param(name) => (values.positive(&name.0)?, Some(name.0.clone())),
        DimValue::Expr(_) => {
            return Err(problem(
                EDIT,
                "a board sketch dimension takes a number or a board parameter",
            ));
        }
    };
    if !(v.is_finite() && v > 0.0 && v <= MAX_COORD_MM) {
        return Err(problem(EDIT, format!("{v} mm is not a positive size")));
    }
    Ok((v, param))
}

fn finish(stored: &Sketch, sk: &Sketch) -> Outcome<SketchOutcome> {
    let json = check::canonical(sk)?;
    check::check_board_sketch(&json)?;
    Ok(crate::footprint::edit::diff(stored, sk, json))
}

/// The two drawn lines that end at point `p`.
fn corner_lines(sk: &Sketch, p: usize) -> Vec<usize> {
    sk.curves
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            !c.construction && matches!(c.kind, CurveKind::Line { a, b } if a == p || b == p)
        })
        .map(|(i, _)| i)
        .collect()
}

fn other_end(sk: &Sketch, line: usize, p: usize) -> Outcome<usize> {
    match sk.curves.get(line).map(|c| &c.kind) {
        Some(CurveKind::Line { a, b }) => Ok(if *a == p { *b } else { *a }),
        _ => Err(problem(EDIT, "a corner line is gone")),
    }
}

fn fillet(board: &BoardSketch, corners: &[SketchPointId], radius: &DimValue) -> Outcome<Fillets> {
    let stored = check::check_board_sketch(&board.sketch.0)?;
    let values = params::evaluate(&board.parameters, &[])?;
    let (r, param) = dim(&values, radius)?;
    let mut sk = stored.clone();
    let mut at: Vec<usize> = Vec::new();
    if corners.is_empty() {
        // Every point where two lines of the outline meet.
        let outline: BTreeSet<usize> = board
            .solved
            .outer_curves
            .iter()
            .filter_map(|c| sk.curve_index(&c.0))
            .collect();
        let mut ends: BTreeMap<usize, usize> = BTreeMap::new();
        for c in &outline {
            if let Some(CurveKind::Line { a, b }) = sk.curves.get(*c).map(|c| &c.kind) {
                *ends.entry(*a).or_default() += 1;
                *ends.entry(*b).or_default() += 1;
            }
        }
        at.extend(ends.into_iter().filter(|(_, n)| *n == 2).map(|(p, _)| p));
        if at.is_empty() {
            return Err(problem(
                EDIT,
                "the outline has no corner where two lines meet",
            ));
        }
    } else {
        for c in corners {
            at.push(
                sk.point_index(&c.0)
                    .ok_or_else(|| problem(EDIT, format!("no sketch point `{}`", c.0)))?,
            );
        }
    }
    let mut arcs = Vec::new();
    for p in at {
        let lines = corner_lines(&sk, p);
        let [l1, l2] = lines.as_slice() else {
            return Err(problem(
                EDIT,
                format!(
                    "point `{}` is not a corner where exactly two lines meet",
                    sk.points.get(p).map_or("?", |q| q.id.as_str())
                ),
            ));
        };
        let (l1, l2) = (*l1, *l2);
        let corner = pos(&sk, p)?;
        let (e1, e2) = (other_end(&sk, l1, p)?, other_end(&sk, l2, p)?);
        let (a, b) = (pos(&sk, e1)?, pos(&sk, e2)?);
        let (Some(u1), Some(u2)) = ((a - corner).normalized(), (b - corner).normalized()) else {
            return Err(problem(EDIT, "a corner line has no length"));
        };
        let phi = u1.dot(u2).clamp(-1.0, 1.0).acos();
        if !(1e-6..std::f64::consts::PI - 1e-6).contains(&phi) {
            return Err(problem(EDIT, "the lines at the corner are parallel"));
        }
        let d = r / (phi / 2.0).tan();
        if d >= corner.dist(a) - 1e-6 || d >= corner.dist(b) - 1e-6 {
            return Err(problem(
                EDIT,
                format!("a {r} mm fillet does not fit the lines at the corner"),
            ));
        }
        let (t1, t2) = (corner + u1 * d, corner + u2 * d);
        let bis = (u1 + u2)
            .normalized()
            .ok_or_else(|| problem(EDIT, "the corner has no bisector"))?;
        let centre = corner + bis * (r / (phi / 2.0).sin());
        let pt1 = sk.add_point(t1, None).map_err(sketch_error)?;
        let pt2 = sk.add_point(t2, None).map_err(sketch_error)?;
        let pc = sk.add_point(centre, None).map_err(sketch_error)?;
        for (line, t) in [(l1, pt1), (l2, pt2)] {
            if let Some(CurveKind::Line { a, b }) = sk.curves.get_mut(line).map(|c| &mut c.kind) {
                if *a == p {
                    *a = t;
                } else {
                    *b = t;
                }
            }
        }
        // Counter-clockwise about the centre, the short way round.
        let (s, e) = if (t1 - centre).cross(t2 - centre) > 0.0 {
            (pt1, pt2)
        } else {
            (pt2, pt1)
        };
        let (ps, pe) = (pos(&sk, s)?, pos(&sk, e)?);
        let arc = sk
            .add_arc(centre, ps, pe, [Some(pc), Some(s), Some(e)], None)
            .map_err(sketch_error)?;
        let kinds = [
            (ConstraintKind::PointOnCurve { p, c: l1 }, None),
            (ConstraintKind::PointOnCurve { p, c: l2 }, None),
            (ConstraintKind::Tangent { a: l1, b: arc }, None),
            (ConstraintKind::Tangent { a: l2, b: arc }, None),
            (ConstraintKind::Radius { c: arc, value: r }, param.clone()),
        ];
        for (kind, param) in kinds {
            sk.add_constraint(kind, param).map_err(sketch_error)?;
        }
        let id = |c: usize| {
            sk.curves
                .get(c)
                .map(|c| SketchCurveId(c.id.clone()))
                .ok_or_else(|| problem(EDIT, "a fillet curve is gone"))
        };
        arcs.push((id(arc)?, id(l1)?));
    }
    Ok(Fillets {
        outcome: finish(&stored, &sk)?,
        arcs,
    })
}

fn slot(board: &BoardSketch, a: Point, b: Point, width: &DimValue) -> Outcome<DrawnSlot> {
    let stored = check::check_board_sketch(&board.sketch.0)?;
    let values = params::evaluate(&board.parameters, &[])?;
    let (w, param) = dim(&values, width)?;
    let (a, b) = (
        crate::footprint::edit::vec2(a)?,
        crate::footprint::edit::vec2(b)?,
    );
    let u = (b - a)
        .normalized()
        .ok_or_else(|| problem(EDIT, "a slot needs two different cap centres"))?;
    let n = u.perp() * (w / 2.0);
    let mut sk = stored.clone();
    let add = |sk: &mut Sketch, v: Vec2| sk.add_point(v, None).map_err(sketch_error);
    let (ca, cb) = (add(&mut sk, a)?, add(&mut sk, b)?);
    let (t1, t2, t3, t4) = (
        add(&mut sk, a - n)?,
        add(&mut sk, b - n)?,
        add(&mut sk, b + n)?,
        add(&mut sk, a + n)?,
    );
    let lower = sk.add_line_pts(t1, t2, None).map_err(sketch_error)?;
    let arc_b = sk
        .add_arc(b, b - n, b + n, [Some(cb), Some(t2), Some(t3)], None)
        .map_err(sketch_error)?;
    let upper = sk.add_line_pts(t3, t4, None).map_err(sketch_error)?;
    let arc_a = sk
        .add_arc(a, a + n, a - n, [Some(ca), Some(t4), Some(t1)], None)
        .map_err(sketch_error)?;
    let mut kinds = Vec::new();
    for (l, arc) in [
        (lower, arc_a),
        (lower, arc_b),
        (upper, arc_a),
        (upper, arc_b),
    ] {
        kinds.push((ConstraintKind::Tangent { a: l, b: arc }, None));
    }
    kinds.push((ConstraintKind::Equal { a: arc_a, b: arc_b }, None));
    kinds.push((ConstraintKind::Diameter { c: arc_a, value: w }, param));
    for (kind, param) in kinds {
        sk.add_constraint(kind, param).map_err(sketch_error)?;
    }
    let curves = [lower, arc_b, upper, arc_a]
        .iter()
        .map(|c| {
            sk.curves
                .get(*c)
                .map(|c| SketchCurveId(c.id.clone()))
                .ok_or_else(|| problem(EDIT, "a slot curve is gone"))
        })
        .collect::<Outcome<Vec<_>>>()?;
    Ok(DrawnSlot {
        outcome: finish(&stored, &sk)?,
        curves,
    })
}

/// Draw a free-form curve over new points: a cubic spline, an ellipse or a
/// conic, as Solidcraft's own sketch does. An end drawn on an existing
/// point (a corner of the edge it replaces) is tied to it with a
/// Coincident, so the outline stays closed when the corner moves.
fn free_form(board: &BoardSketch, curve: &FreeFormCurve) -> Outcome<SketchOutcome> {
    let stored = check::check_board_sketch(&board.sketch.0)?;
    let mut sk = stored.clone();
    let index = add_free_form(&mut sk, curve)?;
    let ends = sk.curves.get(index).and_then(|c| c.kind.ends());
    for p in ends.into_iter().flat_map(|(a, b)| [a, b]) {
        let Some(at) = sk.point(p) else { continue };
        let corner = (0..stored.points.len())
            .filter_map(|q| Some((q, stored.point(q)?.dist(at))))
            .filter(|(_, d)| *d <= JOIN_MM)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((q, _)) = corner {
            sk.add_constraint(ConstraintKind::Coincident { p, q }, None)
                .map_err(sketch_error)?;
        }
    }
    finish(&stored, &sk)
}

/// The polyline a free-form curve would draw, for a drawing tool's live
/// preview: the curve alone in a scratch sketch, built as `board.edit`
/// builds it ([`add_free_form`]), so the preview is the curve the gesture
/// commits. Touches no board; `None` while the input is not a curve yet.
pub fn free_form_preview(curve: &FreeFormCurve) -> Option<Vec<Point>> {
    let mut sk = Sketch::new();
    let index = add_free_form(&mut sk, curve).ok()?;
    let line: Vec<Point> = sk.polyline(index).iter().map(|p| [p.x, p.y]).collect();
    (line.len() >= 2 && line.iter().all(|p| p[0].is_finite() && p[1].is_finite())).then_some(line)
}

/// Add a free-form curve over new points to `sk`; returns its index.
fn add_free_form(sk: &mut Sketch, curve: &FreeFormCurve) -> Outcome<usize> {
    let add = |sk: &mut Sketch, p: Point| -> Outcome<usize> {
        let v = crate::footprint::edit::vec2(p)?;
        sk.add_point(v, None).map_err(sketch_error)
    };
    let (kind, construction) = match curve {
        FreeFormCurve::Spline {
            points,
            control,
            periodic,
            construction,
        } => {
            if !(2..=MAX_SPLINE_POINTS).contains(&(points.len() + usize::from(*periodic))) {
                return Err(problem(
                    EDIT,
                    format!("a spline has 2–{MAX_SPLINE_POINTS} points"),
                ));
            }
            let mut pts = Vec::with_capacity(points.len() + 1);
            for p in points {
                pts.push(add(sk, *p)?);
            }
            // A periodic spline closes on its first point.
            if *periodic && let Some(first) = pts.first().copied() {
                pts.push(first);
            }
            (
                CurveKind::Spline {
                    pts,
                    control: *control,
                    degree: 3,
                    periodic: *periodic,
                    nurbs: None,
                },
                *construction,
            )
        }
        FreeFormCurve::Ellipse {
            center,
            major,
            minor_radius,
            construction,
        } => {
            let r = *minor_radius;
            if !(r.is_finite() && r > 0.0 && r <= MAX_COORD_MM) {
                return Err(problem(
                    EDIT,
                    format!("{r} mm is not an ellipse's minor radius"),
                ));
            }
            let c = add(sk, *center)?;
            let m = add(sk, *major)?;
            (CurveKind::Ellipse { c, m, r }, *construction)
        }
        FreeFormCurve::Conic {
            a,
            b,
            apex,
            rho,
            construction,
        } => {
            let (a, b, apex) = (add(sk, *a)?, add(sk, *b)?, add(sk, *apex)?);
            (
                CurveKind::Conic {
                    a,
                    b,
                    apex,
                    rho: *rho,
                },
                *construction,
            )
        }
    };
    let index = sk.add_curve(kind, None).map_err(sketch_error)?;
    if let Some(c) = sk.curves.get_mut(index) {
        c.construction = construction;
    }
    Ok(index)
}

/// A sketched board's laminate for the native model: the outer outline
/// to extrude and the contours cut through it (cutouts, slots and
/// mounting holes). `None` for a rectangular board, which stays a box.
pub struct Laminate {
    pub outline: crate::pad_outline::Contour,
    pub through: Vec<crate::pad_outline::Contour>,
}

pub fn laminate(project: &pcbcraft_core::Project) -> Option<Laminate> {
    use crate::pad_outline::Contour;
    if project.board_shape.is_rectangle() {
        return None;
    }
    let board = project.board();
    let mut through: Vec<Contour> = board
        .cutouts
        .iter()
        .map(|c| Contour::Outline(c.outline.clone()))
        .collect();
    through.extend(board.holes.iter().map(|h| Contour::Circle {
        center: h.at,
        radius: h.drill / 2.0,
    }));
    Some(Laminate {
        outline: Contour::Outline(board.outer.clone()),
        through,
    })
}

#[cfg(test)]
mod closed_loop_tests;
pub mod model_refs;
mod parts;
#[cfg(test)]
mod tests;
