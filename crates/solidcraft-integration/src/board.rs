//! The board shape evaluator: [`SketchBoardEvaluator`] implements the
//! core's [`BoardShapeEvaluator`] on the same Solidcraft sketch machinery
//! as the footprint evaluator ([`crate::footprint`]): the canonical sketch
//! check, parameters in Solidcraft's expression language, bound
//! dimensions and the solver. It turns the solved sketch into the board's
//! outer outline, cutouts and slots (closed loops of lines and arcs, or one
//! circle), mounting holes and named edges, and evaluates the edge
//! constraints' expressions against the board parameters.
//!
//! Corners are rounded by real sketch fillets ([`fillet`](SketchBoardEvaluator)):
//! the two lines are shortened to a tangent arc and the corner stays as
//! their virtual intersection (a point on both lines), so dimensions to it
//! keep holding and a named edge still runs corner to corner.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use pcbcraft_core::diagnostics::{Error, Finding, ObjectRef, Result};
use pcbcraft_core::geometry::{Point, polygon_contains};
use pcbcraft_core::layout::outline::{
    BoardEvaluation, BoardShapeEvaluator, CONSTRAINT_UNRESOLVED, ConstraintValues, DrawnSlot,
    FATAL_PREFIX, Fillets, SHAPE_UNSOLVED,
};
use pcbcraft_core::library::footprint_source::{
    DimValue, EvaluatorRef, Expr, FootprintParam, ParamName, ParamUnit, SketchBlob, SketchCurveId,
    SketchEdit, SketchOutcome, SketchPointId, SolveStatus, SolveSummary,
};
use pcbcraft_core::library::{Outline, Segment};
use pcbcraft_core::model::EdgeConstraint;
use pcbcraft_core::model::board::{
    BoardSketch, EdgeName, NamedEdge, RECT_EDGES, SolvedBoard, SolvedCutout, SolvedEdge,
    SolvedMountingHole, flatten,
};
use solidcraft_engine::geom::Vec2;
use solidcraft_engine::sketch::{ConstraintKind, CurveKind, Sketch};

use crate::footprint::check::{self, MAX_COORD_MM};
use crate::footprint::evaluate::{fnv, q, solve};
use crate::footprint::params::{self, Values};
use crate::footprint::{ENGINE, Outcome, Problem, SketchEvaluator, evaluator_ref, problem};

/// Two loop ends closer than this join, mm.
const JOIN_MM: f64 = 1e-6;
/// Chord tolerance for inside tests, mm.
const CHORD_MM: f64 = 0.01;

const OUTLINE: &str = "board.shape_outline";
const CURVE: &str = "board.shape_curve";
const CUTOUT: &str = "board.shape_cutout";
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
    Expr(format!("{v} mm"))
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
            outline: ids.clone(),
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
        };
        let evaluation = self.evaluate(&board, &[])?;
        if let Some(f) = evaluation
            .findings
            .iter()
            .find(|f| f.code.starts_with(FATAL_PREFIX))
        {
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
        pcbcraft_core::library::FootprintEvaluator::apply_sketch(&SketchEvaluator, sketch, edit)
    }

    fn evaluate(
        &self,
        board: &BoardSketch,
        constraints: &[EdgeConstraint],
    ) -> Result<BoardEvaluation> {
        if board.evaluator.engine != ENGINE {
            return Err(Error::Unsupported(format!(
                "board.evaluator: the board sketch was made with {}, this build has {ENGINE}",
                board.evaluator.engine
            )));
        }
        let stored = check::check_sketch(&board.sketch.0)?;
        Ok(match run(board, &stored, constraints) {
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
        constraints: Vec::new(),
        findings: vec![Finding::new(code, p.message, vec![ObjectRef::Board])],
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

/// A curve as loop pieces: a line, an arc, or a circle as two half arcs.
fn pieces_of(sk: &Sketch, id: &str) -> Outcome<Vec<Piece>> {
    let c = sk
        .curve_index(id)
        .and_then(|c| sk.curves.get(c))
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
            return Err(problem(
                CURVE,
                format!("`{id}` is not a line, arc or circle"),
            ));
        }
    })
}

/// The closed loop the curves form, joined end to end in any order and
/// direction. One circle is a loop by itself.
fn loop_of(sk: &Sketch, ids: &[SketchCurveId], what: &str) -> Outcome<Vec<Piece>> {
    let mut pieces = Vec::new();
    for id in ids {
        let p = pieces_of(sk, &id.0)?;
        if p.len() == 2 && ids.len() > 1 {
            return Err(problem(
                OUTLINE,
                format!("{what}: a circle is a loop by itself"),
            ));
        }
        pieces.extend(p);
    }
    if pieces.len() == 2 && ids.len() == 1 {
        return Ok(pieces);
    }
    let mut rest = pieces.into_iter();
    let first = rest
        .next()
        .ok_or_else(|| problem(OUTLINE, format!("{what} has no curves")))?;
    let mut rest: Vec<Option<Piece>> = rest.map(Some).collect();
    let start = first.a;
    let mut at = first.b;
    let mut out = vec![first];
    for _ in 0..rest.len() {
        let next = rest.iter_mut().find_map(|slot| {
            let p = slot.as_ref()?;
            let joined = if p.a.dist(at) <= JOIN_MM {
                p.clone()
            } else if p.b.dist(at) <= JOIN_MM {
                p.reversed()
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
        at = next.b;
        out.push(next);
    }
    if at.dist(start) > JOIN_MM {
        return Err(problem(OUTLINE, format!("{what} does not close")));
    }
    Ok(out)
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

fn run(
    board: &BoardSketch,
    stored: &Sketch,
    constraints: &[EdgeConstraint],
) -> Outcome<BoardEvaluation> {
    let values = params::evaluate(&board.parameters, &[])
        .map_err(|p| problem(SHAPE_UNSOLVED, format!("board parameters: {}", p.message)))?;
    let (sk, dof) = solve(stored, &values)?;
    let outer = oriented(loop_of(&sk, &board.outline, "the board outline")?, true);
    let outer_outline = outline_of(&outer)?;
    let outer_polygon = flatten(&outer_outline, CHORD_MM);
    // Edge orientation: material on the left. The outer loop runs
    // counter-clockwise, cutouts clockwise.
    let mut loops: BTreeMap<String, Piece> = BTreeMap::new();
    for p in &outer {
        loops.insert(p.curve.clone(), p.clone());
    }
    let mut cutouts = Vec::new();
    for role in &board.cutouts {
        let what = format!("cutout {}", role.id.0);
        let pieces = oriented(loop_of(&sk, &role.curves, &what)?, true);
        let outline = outline_of(&pieces)?;
        if !flatten(&outline, CHORD_MM)
            .iter()
            .all(|p| polygon_contains(&outer_polygon, *p))
        {
            return Err(problem(
                CUTOUT,
                format!("{what} must lie inside the board outline"),
            ));
        }
        for p in pieces.iter().rev() {
            loops.insert(p.curve.clone(), p.reversed());
        }
        cutouts.push(SolvedCutout {
            id: role.id,
            kind: role.kind,
            outline,
        });
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
    let held = held_on(&sk);
    let mut edges = Vec::new();
    for e in &board.edges {
        let what = format!("edge `{}`", e.name.0);
        let index = sk
            .curve_index(&e.curve.0)
            .ok_or_else(|| problem(EDGE, format!("{what}: no sketch curve `{}`", e.curve.0)))?;
        let mut drawn = pieces_of(&sk, &e.curve.0)?;
        if drawn.len() != 1 {
            return Err(problem(
                EDGE,
                format!("{what}: a named edge is a line or an arc, not a circle"),
            ));
        }
        let piece = match loops.get(&e.curve.0) {
            Some(p) => p.clone(),
            None => drawn.remove(0),
        };
        let (a, b) = if piece.bulge == 0.0 {
            extended(&sk, index, &piece, &held)
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
    let solved = SolvedBoard {
        outer: outer_outline,
        cutouts,
        holes,
        edges,
    };
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
    let bytes =
        serde_json::to_vec(&solved).map_err(|e| problem("board.internal", e.to_string()))?;
    let mut findings = Vec::new();
    let mut solved_constraints = Vec::new();
    for c in constraints {
        let along = values.signed_length(&c.along.0);
        let offset = values.signed_length(&c.offset.0);
        match (along, offset) {
            (Ok(along), Ok(offset)) => solved_constraints.push(ConstraintValues {
                constraint: c.id,
                along,
                offset,
            }),
            (Err(p), _) | (_, Err(p)) => findings.push(Finding::new(
                CONSTRAINT_UNRESOLVED,
                format!("edge constraint {}: {}", c.id, p.message),
                vec![ObjectRef::Board],
            )),
        }
    }
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
        constraints: solved_constraints,
        findings,
        sketch_out: Some(SketchBlob(check::canonical(&sk)?)),
    })
}

/// A dimension value: the number (mm) and the parameter it binds.
fn dim(values: &Values, value: &DimValue) -> Outcome<(f64, Option<String>)> {
    let (v, param) = match value {
        DimValue::Fixed(v) => (*v, None),
        DimValue::Param(name) => (values.positive(&name.0)?, Some(name.0.clone())),
    };
    if !(v.is_finite() && v > 0.0 && v <= MAX_COORD_MM) {
        return Err(problem(EDIT, format!("{v} mm is not a positive size")));
    }
    Ok((v, param))
}

fn finish(stored: &Sketch, sk: &Sketch) -> Outcome<SketchOutcome> {
    let json = check::canonical(sk)?;
    check::check_sketch(&json)?;
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
    let stored = check::check_sketch(&board.sketch.0)?;
    let values = params::evaluate(&board.parameters, &[])?;
    let (r, param) = dim(&values, radius)?;
    let mut sk = stored.clone();
    let mut at: Vec<usize> = Vec::new();
    if corners.is_empty() {
        // Every point where two outline lines meet.
        let outline: BTreeSet<usize> = board
            .outline
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
    let stored = check::check_sketch(&board.sketch.0)?;
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
mod tests;
