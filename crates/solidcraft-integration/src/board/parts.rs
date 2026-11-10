//! The parts solve on Solidcraft's sketch solver
//! (docs/board-sketch-references.md §5.2): one scratch sketch per connected
//! component of dimensioned parts, with the solved board geometry held
//! fixed, per part an origin and a construction axis, its features bound to
//! that frame with `PatternPoint` (y mirrored on the bottom side), and the
//! dimensions, whose expressions evaluate over the board parameters.
//!
//! A part's rotation is solved only when a relation that sets a direction
//! names it (`PartRelation::orients`); otherwise it is pinned at its stored
//! (or moved-to) value before the first solve, so a distance never turns a
//! part, and two distances only a turn would satisfy are a conflict.
//!
//! The solve keeps every degree of freedom the dimensions leave free at its
//! stored value (or, for a part the operation moved, at the value it was
//! moved to): it solves the dimensions once with a freedom analysis, reads
//! which of each part's x, y and rotation they determine (through helper
//! points that copy one coordinate each), then pins the free ones and
//! solves again. Pins that turn out to be coupled (two parts held only to
//! each other) are placed in halves until each lands or proves determined.
use crate::geom::Vec2;
use crate::geom::prelude::*;
use std::collections::BTreeMap;

use crate::footprint::check::{self, MAX_COORD_MM};
use crate::footprint::edit::{self, Ref};
use crate::footprint::params::{self, Values};
use crate::footprint::{Outcome, problem};
use pcbcraft_core::diagnostics::{Finding, ObjectRef, Result};
use pcbcraft_core::ids::Id;
use pcbcraft_core::layout::dimensions::{
    FeatureGeometry, PartFreedom, PartPose, PartsRequest, PartsSolution, REFERENCE_LOST,
};
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, GeomConstraint, SketchRef, SolveStatus,
};
use pcbcraft_core::model::board::{ConstraintTarget, arc_geometry};
use pcbcraft_core::model::dimension::{
    BoardRef, FeatureRef, MAX_PART_DIMENSIONS, PartAxis, PartDimension, PartRelation,
};
use solidcraft_engine::sketch::{
    Analysis, ConstraintKind, CurveKind, Hold, Sketch, measure_dimension, solve_with,
};

/// Targets one parts solve takes.
pub const MAX_SOLVE_PARTS: usize = 256;
/// Features of one target in one solve.
pub const MAX_PART_FEATURES: usize = 64;
/// Solves one placement of pins may run (the halving fallback included).
const MAX_PIN_SOLVES: usize = 1024;
/// Length of each frame's construction axis, mm.
const AXIS_MM: f64 = 10.0;
/// How far a solved coordinate may sit from its stored value and still be
/// read as unchanged, mm or degrees.
const SAME: f64 = 1e-9;

const CODE: &str = "layout.dimension";
/// Finding code of a solve the dimensions cannot satisfy.
const CONFLICT: &str = "layout.dimension_conflict";

/// One target's frame in the scratch sketch.
struct Frame {
    target: ConstraintTarget,
    /// The pose it starts from and keeps where nothing moves it.
    stored: PartPose,
    /// Where the operation put it, for a moved target.
    dragged: Option<PartPose>,
    mirrored: bool,
    /// Fills and keepouts only translate.
    turns: bool,
    /// The rotation a part keeps because no relation that sets a direction
    /// names it ([`PartRelation::orients`]): pinned before the first solve,
    /// so distances never turn the part.
    turn_held: Option<f64>,
    origin: usize,
    axis_end: usize,
    axis: usize,
    axis_y: Option<usize>,
    /// Helper points: `x_of` copies the origin's x, `y_of` its y, `turn_of`
    /// the axis direction about the sketch origin; each is determined
    /// exactly when what it copies is.
    x_of: usize,
    y_of: usize,
    turn_of: usize,
    features: Vec<(FeatureRef, Ref)>,
}

impl Frame {
    fn rotation(&self) -> f64 {
        self.stored.rotation_deg.to_radians()
    }

    fn sign(&self) -> f64 {
        if self.mirrored { -1.0 } else { 1.0 }
    }

    /// A part-frame point at the stored pose.
    fn world(&self, at: [f64; 2]) -> Vec2 {
        let (sin, cos) = self.rotation().sin_cos();
        let u = Vec2::new(cos, sin);
        let o = Vec2::new(self.stored.position[0], self.stored.position[1]);
        o + u * at[0] + u.perp() * (self.sign() * at[1])
    }
}

/// A coordinate pin on a free degree of freedom.
#[derive(Clone, Copy, Debug)]
enum Pin {
    Rotation(usize, f64),
    X(usize, f64),
    Y(usize, f64),
}

impl Pin {
    fn frame(self) -> usize {
        match self {
            Self::Rotation(f, _) | Self::X(f, _) | Self::Y(f, _) => f,
        }
    }
}

fn err(message: impl Into<String>) -> crate::footprint::Problem {
    problem(CODE, message)
}

fn sketch_err(e: impl std::fmt::Display) -> crate::footprint::Problem {
    err(e.to_string())
}

fn vec2(p: [f64; 2]) -> Outcome<Vec2> {
    if p.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD_MM) {
        Ok(Vec2::new(p[0], p[1]))
    } else {
        Err(err(format!(
            "a dimensioned part at {p:?} lies outside the board's coordinate range"
        )))
    }
}

fn add_point(sk: &mut Sketch, at: Vec2) -> Outcome<usize> {
    sk.add_point(at, None).map_err(sketch_err)
}

fn add_fixed(sk: &mut Sketch, at: Vec2) -> Outcome<usize> {
    let p = add_point(sk, at)?;
    if let Some(point) = sk.points.get_mut(p) {
        point.fixed = true;
    }
    Ok(p)
}

fn add_line(sk: &mut Sketch, a: usize, b: usize) -> Outcome<usize> {
    let l = sk.add_line_pts(a, b, None).map_err(sketch_err)?;
    if let Some(c) = sk.curves.get_mut(l) {
        c.construction = true;
    }
    Ok(l)
}

fn constrain(sk: &mut Sketch, kind: ConstraintKind) -> Outcome<String> {
    sk.add_constraint(kind, None).map_err(sketch_err)
}

/// Bind a part-frame point to `frame` rigidly.
fn bind(sk: &mut Sketch, frame: &Frame, at: [f64; 2]) -> Outcome<usize> {
    let q = add_point(sk, frame.world(at))?;
    constrain(
        sk,
        ConstraintKind::PatternPoint {
            p: frame.origin,
            q,
            center: None,
            angle: 0.0,
            offset: Vec2::ZERO,
            directions: [
                Some((frame.axis, Vec2::new(at[0], frame.sign() * at[1]))),
                None,
            ],
        },
    )?;
    Ok(q)
}

/// The sketch entity of one feature, drawn and bound on first use.
fn feature(
    sk: &mut Sketch,
    frame: &mut Frame,
    f: &FeatureRef,
    geometry: &FeatureGeometry,
) -> Outcome<Ref> {
    match f {
        FeatureRef::Origin { .. } => return Ok(Ref::P(frame.origin)),
        FeatureRef::Axis {
            axis: PartAxis::X, ..
        } => return Ok(Ref::C(frame.axis)),
        FeatureRef::Axis {
            axis: PartAxis::Y, ..
        } => {
            if let Some(l) = frame.axis_y {
                return Ok(Ref::C(l));
            }
            let end = bind(sk, frame, [0.0, AXIS_MM])?;
            let l = add_line(sk, frame.origin, end)?;
            frame.axis_y = Some(l);
            return Ok(Ref::C(l));
        }
        _ => {}
    }
    Ok(match *geometry {
        FeatureGeometry::Point { at } => Ref::P(bind(sk, frame, at)?),
        FeatureGeometry::Line { a, b } => {
            let (a, b) = (bind(sk, frame, a)?, bind(sk, frame, b)?);
            Ref::C(add_line(sk, a, b)?)
        }
        FeatureGeometry::Arc { a, b, bulge } => {
            let arc = arc_geometry(a, b, bulge)
                .ok_or_else(|| err("a dimensioned feature is a degenerate arc"))?;
            // Solidcraft's arcs run counter-clockwise; a mirrored part turns
            // its arcs over.
            let (start, end) = if bulge * frame.sign() > 0.0 {
                (a, b)
            } else {
                (b, a)
            };
            let c = bind(sk, frame, arc.center)?;
            let (s, e) = (bind(sk, frame, start)?, bind(sk, frame, end)?);
            let (cp, sp, ep) = (
                frame.world(arc.center),
                frame.world(start),
                frame.world(end),
            );
            let l = sk
                .add_arc(cp, sp, ep, [Some(c), Some(s), Some(e)], None)
                .map_err(sketch_err)?;
            if let Some(curve) = sk.curves.get_mut(l) {
                curve.construction = true;
            }
            Ref::C(l)
        }
        FeatureGeometry::Circle { center, radius } => {
            let c = bind(sk, frame, center)?;
            let l = sk
                .add_circle(frame.world(center), radius, Some(c), None)
                .map_err(sketch_err)?;
            if let Some(curve) = sk.curves.get_mut(l) {
                curve.construction = true;
            }
            constrain(
                sk,
                ConstraintKind::Radius {
                    c: l,
                    value: radius,
                },
            )?;
            Ref::C(l)
        }
    })
}

/// A target's frame: origin, axis, helpers and features.
fn frame(
    sk: &mut Sketch,
    zero: usize,
    part: &pcbcraft_core::layout::dimensions::SolvePart,
    dragged: Option<PartPose>,
) -> Outcome<Frame> {
    if part.features.len() > MAX_PART_FEATURES {
        return Err(err(format!(
            "a part takes at most {MAX_PART_FEATURES} dimensioned features"
        )));
    }
    if !part.pose.valid() {
        return Err(err("a dimensioned part's pose is out of range"));
    }
    let o = vec2(part.pose.position)?;
    let (sin, cos) = part.pose.rotation_deg.to_radians().sin_cos();
    let u = Vec2::new(cos, sin);
    let origin = add_point(sk, o)?;
    let axis_end = add_point(sk, o + u * AXIS_MM)?;
    let axis = add_line(sk, origin, axis_end)?;
    constrain(
        sk,
        ConstraintKind::Distance {
            p: origin,
            q: axis_end,
            value: AXIS_MM,
        },
    )?;
    let x_of = add_point(sk, Vec2::new(o.x, 0.0))?;
    constrain(sk, ConstraintKind::VerticalPoints { p: origin, q: x_of })?;
    constrain(sk, ConstraintKind::HorizontalPoints { p: zero, q: x_of })?;
    let y_of = add_point(sk, Vec2::new(0.0, o.y))?;
    constrain(sk, ConstraintKind::HorizontalPoints { p: origin, q: y_of })?;
    constrain(sk, ConstraintKind::VerticalPoints { p: zero, q: y_of })?;
    let zero_at = sk.point(zero).unwrap_or(Vec2::ZERO);
    let turn_of = add_point(sk, zero_at + u * AXIS_MM)?;
    let turn_line = add_line(sk, zero, turn_of)?;
    constrain(
        sk,
        ConstraintKind::Distance {
            p: zero,
            q: turn_of,
            value: AXIS_MM,
        },
    )?;
    constrain(
        sk,
        ConstraintKind::Parallel {
            a: axis,
            b: turn_line,
        },
    )?;
    let turns = matches!(part.target, ConstraintTarget::Placement(_));
    if !turns {
        constrain(
            sk,
            ConstraintKind::LineAngle {
                l: axis,
                value: 0.0,
            },
        )?;
    }
    let mut f = Frame {
        target: part.target,
        stored: part.pose,
        dragged,
        mirrored: part.mirrored,
        turns,
        turn_held: None,
        origin,
        axis_end,
        axis,
        axis_y: None,
        x_of,
        y_of,
        turn_of,
        features: Vec::new(),
    };
    for (feature_ref, geometry) in &part.features {
        let r = feature(sk, &mut f, feature_ref, geometry)?;
        f.features.push((feature_ref.clone(), r));
    }
    Ok(f)
}

fn is_line(sk: &Sketch, c: usize) -> bool {
    matches!(
        sk.curves.get(c).map(|c| &c.kind),
        Some(CurveKind::Line { .. })
    )
}

/// The centre and radius of a circle or arc.
fn round(sk: &Sketch, c: usize) -> Option<(Vec2, f64)> {
    match sk.curves.get(c).map(|c| &c.kind) {
        Some(CurveKind::Circle { c, r }) => Some((sk.point(*c)?, *r)),
        Some(CurveKind::Arc { c, a, .. }) => {
            let centre = sk.point(*c)?;
            Some((centre, centre.dist(sk.point(*a)?)))
        }
        _ => None,
    }
}

fn copy(r: &Ref) -> Ref {
    match r {
        Ref::P(p) => Ref::P(*p),
        Ref::C(c) => Ref::C(*c),
    }
}

/// The Solidcraft constraint a part relation asks for, between resolved
/// references (in either order where the order does not matter).
fn relation(sk: &Sketch, kind: PartRelation, refs: &[Ref]) -> Outcome<ConstraintKind> {
    match kind {
        PartRelation::Dim(DimKind::Distance) => match refs {
            [Ref::P(p), Ref::C(c)] | [Ref::C(c), Ref::P(p)] if round(sk, *c).is_some() => {
                let (centre, r) = round(sk, *c).unwrap_or((Vec2::ZERO, 0.0));
                let inside = sk.point(*p).is_some_and(|at| at.dist(centre) < r);
                let k = ConstraintKind::PointCircleDistance {
                    p: *p,
                    c: *c,
                    value: 0.0,
                    inside,
                };
                sk.validate(&k).map_err(sketch_err)?;
                Ok(k)
            }
            [Ref::C(l), Ref::P(p)] => {
                edit::dimension(sk, DimKind::Distance, &[Ref::P(*p), Ref::C(*l)])
            }
            _ => edit::dimension(sk, DimKind::Distance, refs),
        },
        PartRelation::Dim(k) => edit::dimension(sk, k, refs),
        PartRelation::Geom(g) => match (g, refs) {
            (
                GeomConstraint::Coincident
                | GeomConstraint::PointOnCurve
                | GeomConstraint::Midpoint,
                [Ref::C(c), Ref::P(p)],
            ) => edit::geom_constraint(sk, g, &[Ref::P(*p), Ref::C(*c)]),
            _ => edit::geom_constraint(sk, g, refs),
        },
        PartRelation::PointAlong { at } => match refs {
            [Ref::P(p), Ref::C(c)] | [Ref::C(c), Ref::P(p)] => {
                let k = ConstraintKind::PointAlong { p: *p, c: *c, at };
                sk.validate(&k).map_err(sketch_err)?;
                Ok(k)
            }
            _ => Err(err("a point along a curve names a point and a curve")),
        },
    }
}

/// A distance between two lines (a pad side or model face to an edge)
/// measures from the first line's middle, square to the second, as the
/// distance the views show (`dimensions::view::shown`): the first line
/// becomes a point held at its middle.
fn from_middle(sk: &mut Sketch, kind: PartRelation, refs: Vec<Ref>) -> Outcome<Vec<Ref>> {
    let [Ref::C(a), Ref::C(b)] = refs.as_slice() else {
        return Ok(refs);
    };
    if kind != PartRelation::Dim(DimKind::Distance) || !is_line(sk, *a) || !is_line(sk, *b) {
        return Ok(refs);
    }
    let Some(CurveKind::Line { a: start, b: end }) = sk.curves.get(*a).map(|c| &c.kind) else {
        return Err(err("the dimension needs a line"));
    };
    let (Some(s), Some(e)) = (sk.point(*start), sk.point(*end)) else {
        return Err(err("the dimension needs a line"));
    };
    let (line, other) = (*a, *b);
    let middle = add_point(sk, (s + e) * 0.5)?;
    constrain(sk, ConstraintKind::Midpoint { p: middle, l: line })?;
    Ok(vec![Ref::P(middle), Ref::C(other)])
}

/// A driving dimension's value, mm or radians.
fn value(values: &Values, d: &PartDimension, angle: bool) -> Outcome<f64> {
    let v = match &d.value {
        Some(DimValue::Fixed(v)) if angle => v.to_radians(),
        Some(DimValue::Fixed(v)) => *v,
        Some(DimValue::Param(name)) => values.dimension(&name.0, angle)?,
        Some(DimValue::Expr(e)) => values.dimension(&e.0, angle)?,
        None => return Err(err("a driving dimension needs a value")),
    };
    if !v.is_finite() || v.abs() > MAX_COORD_MM || (!angle && v < 0.0) {
        return Err(err(format!("a dimension of {v} is out of range")));
    }
    Ok(v)
}

/// What a dimension names, resolved in the scratch sketch; `None` for a
/// board point or curve the sketch no longer has (a lost reference).
fn resolve(sk: &Sketch, frames: &[Frame], r: &BoardRef) -> Outcome<Option<Ref>> {
    match r {
        BoardRef::Board(SketchRef::Point(p)) => Ok(sk.point_index(&p.0).map(Ref::P)),
        BoardRef::Board(SketchRef::Curve(c)) => Ok(sk.curve_index(&c.0).map(Ref::C)),
        BoardRef::Board(SketchRef::Constraint(_)) => {
            Err(err("a dimension names board points and curves"))
        }
        BoardRef::Housing(_) => Err(err("housing references are not yet supported")),
        BoardRef::Part(f) => frames
            .iter()
            .find(|frame| frame.target == f.target())
            .and_then(|frame| frame.features.iter().find(|(g, _)| g == f))
            .map(|(_, r)| Some(copy(r)))
            .ok_or_else(|| err("a dimension names a feature the request did not list")),
    }
}

fn add_pin(sk: &mut Sketch, frames: &[Frame], pin: Pin) -> Outcome<()> {
    let Some(f) = frames.get(pin.frame()) else {
        return Ok(());
    };
    match pin {
        Pin::Rotation(_, deg) => {
            constrain(
                sk,
                ConstraintKind::LineAngle {
                    l: f.axis,
                    value: deg.to_radians(),
                },
            )?;
        }
        Pin::X(_, x) => {
            let at = add_fixed(sk, vec2([x, 0.0])?)?;
            constrain(sk, ConstraintKind::VerticalPoints { p: at, q: f.origin })?;
        }
        Pin::Y(_, y) => {
            let at = add_fixed(sk, vec2([0.0, y])?)?;
            constrain(sk, ConstraintKind::HorizontalPoints { p: at, q: f.origin })?;
        }
    }
    Ok(())
}

/// Whether a pin's degree of freedom is still free.
fn free(determined: &[bool], frames: &[Frame], pin: Pin) -> bool {
    let Some(f) = frames.get(pin.frame()) else {
        return false;
    };
    let point = match pin {
        Pin::Rotation(..) => f.turn_of,
        Pin::X(..) => f.x_of,
        Pin::Y(..) => f.y_of,
    };
    !determined.get(point).copied().unwrap_or(true)
}

/// Pin every free degree of freedom: all at once, or, when two pins are
/// coupled and their values disagree, in halves until each lands or its
/// freedom proves taken by the pins before it. Returns the pins placed.
fn place_pins(
    sk: &mut Sketch,
    frames: &[Frame],
    pins: Vec<Pin>,
    reference: &[Vec2],
    determined: Vec<bool>,
) -> Outcome<Vec<Pin>> {
    let mark = (sk.points.len(), sk.constraints.len());
    let rollback = |sk: &mut Sketch, mark: (usize, usize)| {
        sk.points.truncate(mark.0);
        sk.constraints.truncate(mark.1);
    };
    if pins.is_empty() {
        return Ok(pins);
    }
    for pin in &pins {
        add_pin(sk, frames, *pin)?;
    }
    if solve_with(
        sk,
        &Hold::default(),
        Some(reference),
        Analysis::StatusOnly,
        None,
    )
    .ok()
    {
        return Ok(pins);
    }
    rollback(sk, mark);
    let mut determined = determined;
    let mut placed = Vec::new();
    let mut queue = std::collections::VecDeque::from([pins]);
    let mut solves = 0;
    while let Some(batch) = queue.pop_front() {
        let batch: Vec<Pin> = batch
            .into_iter()
            .filter(|p| free(&determined, frames, *p))
            .collect();
        if batch.is_empty() {
            continue;
        }
        solves += 1;
        if solves > MAX_PIN_SOLVES {
            break;
        }
        let mark = (sk.points.len(), sk.constraints.len());
        for pin in &batch {
            add_pin(sk, frames, *pin)?;
        }
        let report = solve_with(
            sk,
            &Hold::default(),
            Some(reference),
            Analysis::Freedom,
            None,
        );
        if report.ok() {
            determined = report.point_determined;
            placed.extend(batch);
            continue;
        }
        rollback(sk, mark);
        if batch.len() > 1 {
            let mut first = batch;
            let second = first.split_off(first.len() / 2);
            queue.push_front(second);
            queue.push_front(first);
        }
    }
    Ok(placed)
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= SAME * a.abs().max(b.abs()).max(1.0)
}

fn same_angle(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d <= SAME || 360.0 - d <= SAME
}

/// Solve one component of dimensioned parts.
pub(super) fn solve(request: &PartsRequest<'_>) -> Result<PartsSolution> {
    Ok(solve_outcome(request)?)
}

fn solve_outcome(request: &PartsRequest<'_>) -> Outcome<PartsSolution> {
    if request.parts.is_empty() || request.parts.len() > MAX_SOLVE_PARTS {
        return Err(problem(
            "layout.dimension_limit",
            format!("a parts solve takes 1–{MAX_SOLVE_PARTS} parts"),
        ));
    }
    if request.dimensions.len() > MAX_PART_DIMENSIONS {
        return Err(problem(
            "layout.dimension_limit",
            format!("a parts solve takes at most {MAX_PART_DIMENSIONS} dimensions"),
        ));
    }
    let values = params::evaluate(&request.board.parameters, &[])
        .map_err(|p| err(format!("board parameters: {}", p.message)))?;
    // The solved board, every point held where the board solve put it.
    let mut sk = check::check_board_sketch(&request.board.sketch.0)?;
    sk.constraints.clear();
    for p in &mut sk.points {
        p.fixed = true;
    }
    for c in &mut sk.curves {
        c.fixed = true;
    }
    let zero = match sk.point_index("origin") {
        Some(z) => z,
        None => add_fixed(&mut sk, Vec2::ZERO)?,
    };
    let mut frames = Vec::with_capacity(request.parts.len());
    for part in request.parts {
        let dragged = request
            .drags
            .iter()
            .find(|d| d.target == part.target)
            .map(|d| d.to);
        frames.push(frame(&mut sk, zero, part, dragged)?);
    }
    // A part no direction relation names keeps its rotation: pin it now,
    // so the dimensions solve for its position alone and two distances only
    // a turn would satisfy conflict.
    let oriented: Vec<ConstraintTarget> = request
        .dimensions
        .iter()
        .filter(|d| !d.reference && d.kind.orients())
        .flat_map(|d| {
            d.refs.iter().filter_map(|r| match r {
                BoardRef::Part(f) => Some(f.target()),
                _ => None,
            })
        })
        .collect();
    for i in 0..frames.len() {
        let Some(f) = frames.get(i) else {
            continue;
        };
        if !f.turns || oriented.contains(&f.target) {
            continue;
        }
        let deg = f.dragged.unwrap_or(f.stored).rotation_deg;
        add_pin(&mut sk, &frames, Pin::Rotation(i, deg))?;
        if let Some(f) = frames.get_mut(i) {
            f.turn_held = Some(deg);
        }
    }
    let mut owner: BTreeMap<String, Id> = BTreeMap::new();
    let mut measures: Vec<(Id, ConstraintKind)> = Vec::new();
    let mut findings = Vec::new();
    for d in request.dimensions {
        let mut refs = Vec::with_capacity(d.refs.len());
        for r in &d.refs {
            refs.push(resolve(&sk, &frames, r)?);
        }
        let Some(refs) = refs.into_iter().collect::<Option<Vec<Ref>>>() else {
            findings.push(Finding::new(
                REFERENCE_LOST,
                "A dimension names board geometry that was deleted".into(),
                vec![ObjectRef::Board],
            ));
            continue;
        };
        let refs = from_middle(&mut sk, d.kind, refs)?;
        let mut kind = relation(&sk, d.kind, &refs)?;
        if !d.reference && matches!(d.kind, PartRelation::Dim(_)) {
            kind.set_value(value(&values, d, kind.is_angle())?);
        }
        if !d.reference {
            let id = constrain(&mut sk, kind.clone())?;
            owner.insert(id, d.id);
        }
        measures.push((d.id, kind));
    }
    let reference: Vec<Vec2> = sk.points.iter().map(|p| p.pos).collect();
    let first = solve_with(
        &mut sk,
        &Hold::default(),
        Some(&reference),
        Analysis::Freedom,
        None,
    );
    if !first.ok() {
        let failing: Vec<Id> = first
            .failing
            .iter()
            .filter_map(|k| owner.get(k).copied())
            .collect();
        findings.push(Finding::new(
            CONFLICT,
            // Never the solver's constraint ids: the core names the
            // dimensions from `failing`.
            match first.oversized {
                Some(_) => first.failure(|_| String::new()),
                None => "the dimensions cannot all hold".into(),
            },
            vec![ObjectRef::Board],
        ));
        return Ok(PartsSolution {
            poses: Vec::new(),
            measured: Vec::new(),
            status: SolveStatus::OverConstrained,
            freedom: Vec::new(),
            failing,
            findings,
        });
    }
    let determined = |p: usize| first.point_determined.get(p).copied().unwrap_or(true);
    let freedom: Vec<(ConstraintTarget, PartFreedom)> = frames
        .iter()
        .map(|f| {
            (
                f.target,
                PartFreedom {
                    x: !determined(f.x_of),
                    y: !determined(f.y_of),
                    rotation: f.turns && (f.turn_held.is_some() || !determined(f.turn_of)),
                },
            )
        })
        .collect();
    // Pins: the moved targets first, so they go where they were put.
    let mut order: Vec<usize> = (0..frames.len()).collect();
    order.sort_by_key(|i| frames.get(*i).is_none_or(|f| f.dragged.is_none()));
    let mut pins = Vec::new();
    for i in order {
        let (Some(f), Some((_, free))) = (frames.get(i), freedom.get(i)) else {
            continue;
        };
        let to = f.dragged.unwrap_or(f.stored);
        if free.rotation && f.turn_held.is_none() {
            pins.push(Pin::Rotation(i, to.rotation_deg));
        }
        if free.x {
            pins.push(Pin::X(i, to.position[0]));
        }
        if free.y {
            pins.push(Pin::Y(i, to.position[1]));
        }
    }
    let placed = place_pins(
        &mut sk,
        &frames,
        pins,
        &reference,
        first.point_determined.clone(),
    )?;
    let mut poses = Vec::with_capacity(frames.len());
    for (i, f) in frames.iter().enumerate() {
        let (Some(o), Some(a)) = (sk.point(f.origin), sk.point(f.axis_end)) else {
            return Err(err("the solve lost a part's frame"));
        };
        let mut position = [o.x, o.y];
        let mut rotation_deg = if f.turns {
            (a - o).angle().to_degrees().rem_euclid(360.0)
        } else {
            f.stored.rotation_deg
        };
        // Pinned values are exact; anything left where it was keeps its
        // stored value exactly.
        if let Some(v) = f.turn_held {
            rotation_deg = v;
        }
        for pin in placed.iter().filter(|p| p.frame() == i) {
            match *pin {
                Pin::Rotation(_, v) => rotation_deg = v,
                Pin::X(_, v) => position[0] = v,
                Pin::Y(_, v) => position[1] = v,
            }
        }
        for k in 0..2 {
            if let (Some(p), Some(s)) = (position.get_mut(k), f.stored.position.get(k))
                && same(*p, *s)
            {
                *p = *s;
            }
        }
        if same_angle(rotation_deg, f.stored.rotation_deg) {
            rotation_deg = f.stored.rotation_deg;
        }
        let pose = PartPose {
            position,
            rotation_deg,
        };
        if !pose.valid() {
            return Err(err("the dimensions put a part out of range"));
        }
        poses.push((f.target, pose));
    }
    let measured = measures
        .iter()
        .filter_map(|(id, k)| {
            let v = measure_dimension(&sk, k)?;
            Some((*id, if k.is_angle() { v.to_degrees() } else { v }))
        })
        .collect();
    let under = freedom.iter().any(|(_, f)| f.x || f.y || f.rotation);
    Ok(PartsSolution {
        poses,
        measured,
        status: if under {
            SolveStatus::UnderConstrained
        } else {
            SolveStatus::Solved
        },
        freedom,
        failing: Vec::new(),
        findings,
    })
}

#[cfg(test)]
mod carry_tests;
#[cfg(test)]
mod tests;
