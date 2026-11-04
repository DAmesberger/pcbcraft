//! A read-only, id-keyed view of a footprint sketch for the Footprint tab
//! (slices F4/F5): points, curves and constraints with their solved
//! positions, so the app draws, picks and snaps sketch geometry and shows
//! dimensions and constraint glyphs without depending on Solidcraft's
//! sketch types. Nothing here mutates; every change is one
//! `library.edit_footprint` commit.
//!
//! The solver
//! report ([`solve_report`]), the live drag ([`drag_preview`]), "used by"
//! ([`dimensions_using`]), parameter values ([`parameter_values`]) and the
//! dimension layout share one loaded sketch, [`LiveSketch`], which the
//! Footprint tab keeps per source change instead of reloading every frame.
use crate::geom::prelude::*;
use std::collections::BTreeSet;

use crate::geom::Vec2;
use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::geometry::Point;
use pcbcraft_core::library::footprint_source::{
    FootprintSource, PackageValue, ParamName, ParamUnit, ParamValue, SketchConstraintId,
    SketchCurveId, SketchPointId, SolveStatus,
};
use solidcraft_engine::sketch::{
    Analysis, Budget, CurveKind, Hold, Sketch, chain_centres, default_text, dim_frame,
    dim_layout_reaching, dim_reach, move_point_towards, solve_after_edit_with_analysis, solve_with,
};

use super::check::{self, MAX_COORD_MM};
use super::{evaluate, params};

/// One sketch point at its stored solved position.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewPoint {
    pub id: SketchPointId,
    pub at: Point,
    /// Fixed by the sketch (the origin, expression-placed package points).
    pub fixed: bool,
}

/// A sketch curve's drawable geometry.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewGeometry {
    Line {
        a: Point,
        b: Point,
    },
    Circle {
        center: Point,
        radius: f64,
    },
    /// Counter-clockwise from `start` to `end` about `center`.
    Arc {
        center: Point,
        start: Point,
        end: Point,
    },
    /// A curve kind the footprint subset does not draw (free-form curves).
    Other,
}

/// One sketch curve.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewCurve {
    pub id: SketchCurveId,
    pub geometry: ViewGeometry,
    pub construction: bool,
    /// The curve's defining points (ends, centre), by id.
    pub points: Vec<SketchPointId>,
}

/// One constraint or dimension.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewConstraint {
    pub id: SketchConstraintId,
    /// Solidcraft's short type name (`"Horizontal"`, `"LinearDimension"`).
    pub kind: &'static str,
    /// A dimension's current value (mm, or degrees for angles); `None` for
    /// geometric constraints.
    pub value: Option<f64>,
    /// The parameter expression driving the dimension
    /// (Solidcraft's `Constraint.param`), when bound.
    pub param: Option<String>,
    /// A reference (driven) dimension: it measures, it does not constrain.
    pub driven: bool,
    pub points: Vec<SketchPointId>,
    pub curves: Vec<SketchCurveId>,
    /// Where the label was placed, in the dimension's frame; `None` is
    /// the default place.
    pub text: Option<Point>,
}

/// The whole sketch, in sketch order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SketchView {
    pub points: Vec<ViewPoint>,
    pub curves: Vec<ViewCurve>,
    pub constraints: Vec<ViewConstraint>,
}

impl SketchView {
    pub fn point(&self, id: &SketchPointId) -> Option<&ViewPoint> {
        self.points.iter().find(|p| p.id == *id)
    }

    pub fn curve(&self, id: &SketchCurveId) -> Option<&ViewCurve> {
        self.curves.iter().find(|c| c.id == *id)
    }
}

/// The solver's verdict on the current sketch, for the status bar and the
/// under- and over-constrained colouring.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SolveReport {
    pub status: SolveStatus,
    pub dof: u32,
    /// Points that can still move (drawn in Solidcraft's "free" colour).
    pub free_points: Vec<SketchPointId>,
    /// Constraints in conflict when over-constrained (or turned over when
    /// the status is `Failed`).
    pub conflicting: Vec<SketchConstraintId>,
    /// Curves whose defining values the constraints do not all determine.
    pub free_curves: Vec<SketchCurveId>,
    /// Constraints that hold but remove no freedom (implied by others):
    /// shown dimmed, not as a conflict.
    pub redundant: Vec<SketchConstraintId>,
}

fn view_of(sk: &Sketch) -> SketchView {
    let pos = |i: usize| -> Point { sk.points.get(i).map_or([0.0, 0.0], |p| [p.pos.x, p.pos.y]) };
    let pid = |i: usize| -> Option<SketchPointId> {
        sk.points.get(i).map(|p| SketchPointId(p.id.clone()))
    };
    let cid = |i: usize| -> Option<SketchCurveId> {
        sk.curves.get(i).map(|c| SketchCurveId(c.id.clone()))
    };
    let points = sk
        .points
        .iter()
        .map(|p| ViewPoint {
            id: SketchPointId(p.id.clone()),
            at: [p.pos.x, p.pos.y],
            fixed: p.fixed,
        })
        .collect();
    let curves = sk
        .curves
        .iter()
        .map(|c| {
            let (geometry, defining) = match c.kind {
                CurveKind::Line { a, b } => (
                    ViewGeometry::Line {
                        a: pos(a),
                        b: pos(b),
                    },
                    vec![a, b],
                ),
                CurveKind::Circle { c: centre, r } => (
                    ViewGeometry::Circle {
                        center: pos(centre),
                        radius: r,
                    },
                    vec![centre],
                ),
                CurveKind::Arc { c: centre, a, b } => (
                    ViewGeometry::Arc {
                        center: pos(centre),
                        start: pos(a),
                        end: pos(b),
                    },
                    vec![centre, a, b],
                ),
                _ => (ViewGeometry::Other, Vec::new()),
            };
            ViewCurve {
                id: SketchCurveId(c.id.clone()),
                geometry,
                construction: c.construction,
                points: defining.into_iter().filter_map(pid).collect(),
            }
        })
        .collect();
    let constraints = sk
        .constraints
        .iter()
        .map(|k| {
            let (ps, cs) = k.kind.indices();
            ViewConstraint {
                id: SketchConstraintId(k.id.clone()),
                kind: k.kind.name(),
                // Solidcraft keeps angles in radians.
                value: k
                    .kind
                    .value()
                    .map(|v| if k.kind.is_angle() { v.to_degrees() } else { v }),
                param: k.param.clone(),
                driven: k.driven,
                points: ps.into_iter().filter_map(pid).collect(),
                curves: cs.into_iter().filter_map(cid).collect(),
                text: k.text.map(|t| [t.x, t.y]),
            }
        })
        .collect();
    SketchView {
        points,
        curves,
        constraints,
    }
}

/// The source's sketch at its stored solved positions. Pure; refuses a
/// sketch `check_sketch` refuses.
pub fn sketch_view(source: &FootprintSource) -> Result<SketchView> {
    let sk = check::check_sketch(&source.sketch.0)?;
    Ok(view_of(&sk))
}

/// Degrees of freedom, free points and conflicts of the source's sketch,
/// from a full solve of a copy (the stored sketch is never changed).
pub fn solve_report(
    source: &FootprintSource,
    package_values: &[PackageValue],
) -> Result<SolveReport> {
    Ok(LiveSketch::load(source, package_values)?.report())
}

/// The sketch with `point` dragged towards `to`, solved within one frame's
/// budget (Solidcraft's `Budget::frame`). Never touches the document; the
/// release commits one `SketchEdit::Drag`.
pub fn drag_preview(
    source: &FootprintSource,
    package_values: &[PackageValue],
    point: &SketchPointId,
    to: Point,
) -> Result<SketchView> {
    let live = LiveSketch::load(source, package_values)?;
    Ok(live.drag(point, to)?.sketch.view())
}

/// Which dimensions read parameter `name` (directly or through other
/// parameters), for the parameters panel's "used by" column.
pub fn dimensions_using(
    source: &FootprintSource,
    name: &ParamName,
) -> Result<Vec<SketchConstraintId>> {
    let sketch = check::check_sketch(&source.sketch.0)?;
    let readers = readers_of(source, name);
    Ok(sketch
        .constraints
        .iter()
        .filter(|k| {
            k.param
                .as_deref()
                .is_some_and(|text| params::references(text).iter().any(|r| readers.contains(r)))
        })
        .map(|k| SketchConstraintId(k.id.clone()))
        .collect())
}

/// Everything that reads a parameter, directly or through the parameters
/// reading it: the "used by" of the parameters panel and the reason a
/// removal is refused.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParamReaders {
    /// Dimensions whose value reads it.
    pub dimensions: Vec<SketchConstraintId>,
    /// Other parameters whose expressions read it.
    pub parameters: Vec<ParamName>,
    /// Other kinds of reader, each once, in a fixed order: `array`,
    /// `courtyard`, `pad rule`, `override`, `graphic`, `placement`.
    pub roles: Vec<&'static str>,
}

impl ParamReaders {
    /// Nothing reads the parameter: removing it changes nothing.
    pub fn is_empty(&self) -> bool {
        self.dimensions.is_empty() && self.parameters.is_empty() && self.roles.is_empty()
    }
}

/// What reads `name`: dimensions, parameters, arrays, derived graphics,
/// pad rules, overrides and expression-placed points (the same readers
/// the evaluator's removal check counts).
pub fn readers(source: &FootprintSource, name: &ParamName) -> Result<ParamReaders> {
    use pcbcraft_core::library::footprint_source::{
        ArrayLayout, DerivedGraphic, Expr, GraphicSource, MaskRuleExpr, PadShapeExpr, PasteRuleExpr,
    };
    let readers = readers_of(source, name);
    let reads = |e: &Expr| params::references(&e.0).iter().any(|r| readers.contains(r));
    let any = |es: &[&Expr]| es.iter().any(|e| reads(e));
    let mask = |m: &MaskRuleExpr| matches!(m, MaskRuleExpr::Expansion(e) if reads(e));
    let paste = |p: &PasteRuleExpr| matches!(p, PasteRuleExpr::Reduction(e) if reads(e));
    let mut out = ParamReaders {
        dimensions: dimensions_using(source, name)?,
        parameters: source
            .parameters
            .iter()
            .filter(|p| p.name != *name && readers.contains(&p.name.0))
            .map(|p| p.name.clone())
            .collect(),
        roles: Vec::new(),
    };
    let array = source.arrays.iter().any(|a| match &a.layout {
        ArrayLayout::Linear { count, pitch, .. } => any(&[count, pitch]),
        ArrayLayout::DualRow {
            per_row,
            pitch,
            row_span,
            ..
        } => any(&[per_row, pitch, row_span]),
        ArrayLayout::Quad {
            nx,
            ny,
            pitch,
            span_x,
            span_y,
            ..
        } => any(&[nx, ny, pitch, span_x, span_y]),
        ArrayLayout::Grid {
            rows,
            cols,
            pitch_x,
            pitch_y,
            ..
        } => any(&[rows, cols, pitch_x, pitch_y]),
    });
    let courtyard = source.graphics.iter().any(|g| {
        matches!(
            &g.source,
            GraphicSource::Derived(DerivedGraphic::CourtyardFromExtents { excess, round })
                if any(&[excess, round])
        )
    });
    let pad_rule = source.pads.iter().any(|r| {
        mask(&r.mask)
            || paste(&r.paste)
            || r.vias
                .as_ref()
                .is_some_and(|v| any(&[&v.rows, &v.cols, &v.pitch, &v.drill]))
    });
    let shape = |s: &PadShapeExpr| match s {
        PadShapeExpr::Rect { width, height } | PadShapeExpr::Oblong { width, height } => {
            any(&[width, height])
        }
        PadShapeExpr::RoundRect {
            width,
            height,
            radius_ratio,
        } => any(&[width, height, radius_ratio]),
        PadShapeExpr::Circle { diameter } => reads(diameter),
    };
    let overrides = source.overrides.iter().any(|o| {
        o.value.shape.as_ref().is_some_and(shape)
            || o.value.mask.as_ref().is_some_and(mask)
            || o.value.paste.as_ref().is_some_and(paste)
    });
    let graphic = source.graphics.iter().any(|g| reads(&g.width));
    let sketch = check::check_sketch(&source.sketch.0)?;
    let placement = sketch
        .points
        .iter()
        .filter_map(|p| p.exprs.as_deref())
        .flat_map(|e| e.list().into_iter().map(|(_, s)| s.as_str()))
        .any(|text| params::references(text).iter().any(|r| readers.contains(r)));
    for (hit, label) in [
        (array, "array"),
        (courtyard, "courtyard"),
        (pad_rule, "pad rule"),
        (overrides, "override"),
        (graphic, "graphic"),
        (placement, "placement"),
    ] {
        if hit {
            out.roles.push(label);
        }
    }
    Ok(out)
}

/// `name` and every parameter that reads it, directly or through others.
fn readers_of(source: &FootprintSource, name: &ParamName) -> BTreeSet<String> {
    let mut readers = BTreeSet::from([name.0.clone()]);
    // Each pass adds the parameters reading one more level; a cycle (which
    // evaluation refuses) cannot loop because names are only ever added.
    for _ in 0..=source.parameters.len() {
        let before = readers.len();
        for p in &source.parameters {
            if !readers.contains(&p.name.0)
                && params::references(&p.expr.0)
                    .iter()
                    .any(|r| readers.contains(r))
            {
                readers.insert(p.name.0.clone());
            }
        }
        if readers.len() == before {
            break;
        }
    }
    readers
}

/// Why `expr` cannot define parameter `name` by itself: its syntax, an
/// unknown name or its units, read against the other parameters' stored
/// values. `None` when it reads by itself or when only the whole list's
/// evaluation (a cycle through it) can refuse it.
pub fn expression_error(
    source: &FootprintSource,
    package_values: &[PackageValue],
    name: &ParamName,
    expr: &str,
    unit: ParamUnit,
) -> Option<String> {
    params::expression_error(&source.parameters, package_values, &name.0, expr, unit)
}

/// One footprint parameter's value (in its own unit: mm, degrees or a
/// count), or why it has none.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamEvaluation {
    pub name: ParamName,
    pub value: std::result::Result<ParamValue, String>,
}

/// Every footprint parameter's value with the package values injected,
/// each on its own (one failing expression does not hide the others).
/// Refuses only a parameter list evaluation refuses outright (bad or
/// duplicate names, too many parameters).
pub fn parameter_values(
    source: &FootprintSource,
    package_values: &[PackageValue],
) -> Result<Vec<ParamEvaluation>> {
    let values = params::evaluate_each(&source.parameters, package_values)?;
    Ok(source
        .parameters
        .iter()
        .zip(values)
        .map(|(p, value)| ParamEvaluation {
            name: p.name.clone(),
            value,
        })
        .collect())
}

/// A dimension laid out for drawing (Solidcraft's `dim_layout`), in
/// footprint millimetres: extension and dimension lines (polylines),
/// arrowheads (tip and unit direction) and the centre of the value text.
#[derive(Clone, Debug, PartialEq)]
pub struct DimensionLayout {
    pub id: SketchConstraintId,
    pub lines: Vec<Vec<Point>>,
    pub arrows: Vec<(Point, Point)>,
    pub text: Point,
}

/// A loaded footprint sketch with its parameters applied: the stored solved
/// state the Footprint tab draws, reports on and drags from. Loaded once per
/// source change; nothing here writes the document.
#[derive(Clone, Debug)]
pub struct LiveSketch {
    /// As stored (the driving values the stored positions were solved for).
    stored: Sketch,
    /// The stored sketch with point expressions and bound dimensions
    /// applied: what a solve starts from.
    sketch: Sketch,
}

/// What one drag frame did.
#[derive(Clone, Debug)]
pub struct DragFrame {
    /// The sketch after the frame; drag it again for the next frame.
    pub sketch: LiveSketch,
    /// Where the dragged point is now (what the release commits).
    pub at: Point,
    /// The point followed the pointer (exactly, or as close as the
    /// constraints allow); false when it could not move at all.
    pub moved: bool,
    /// The frame budget ran out before the solve finished.
    pub exhausted: bool,
}

const DRAG_HELD: &str = "footprint.drag";

impl LiveSketch {
    /// Load the source's sketch (`check_sketch`) and apply its parameters
    /// and the package values.
    pub fn load(source: &FootprintSource, package_values: &[PackageValue]) -> Result<Self> {
        let stored = check::check_sketch(&source.sketch.0)?;
        let values = params::evaluate(&source.parameters, package_values)?;
        let mut sketch = stored.clone();
        evaluate::apply_values(&mut sketch, &values)?;
        Ok(Self { stored, sketch })
    }

    /// The view at the current positions.
    pub fn view(&self) -> SketchView {
        view_of(&self.sketch)
    }

    /// A full solve of a copy: status, degrees of freedom, free geometry,
    /// conflicting and redundant constraints.
    pub fn report(&self) -> SolveReport {
        let mut sketch = self.sketch.clone();
        let old = self.stored.driving_values();
        let reference: Vec<Vec2> = self.stored.points.iter().map(|p| p.pos).collect();
        let r = solve_after_edit_with_analysis(&mut sketch, &old, Some(&reference), Analysis::Full);
        let constraint_ids = |ids: &[String]| -> Vec<SketchConstraintId> {
            ids.iter().cloned().map(SketchConstraintId).collect()
        };
        let free_points = sketch
            .points
            .iter()
            .zip(&r.point_determined)
            .filter(|(_, determined)| !**determined)
            .map(|(p, _)| SketchPointId(p.id.clone()))
            .collect();
        let free_curves = sketch
            .curves
            .iter()
            .zip(&r.curve_determined)
            .filter(|(_, determined)| !**determined)
            .map(|(c, _)| SketchCurveId(c.id.clone()))
            .collect();
        let dof = u32::try_from(r.dof).unwrap_or(u32::MAX);
        let (status, conflicting) = if !r.ok() {
            if r.failing.is_empty() {
                (SolveStatus::Failed, Vec::new())
            } else {
                (SolveStatus::OverConstrained, constraint_ids(&r.failing))
            }
        } else if !r.folded.is_empty() {
            (SolveStatus::Failed, constraint_ids(&r.folded))
        } else if dof > 0 {
            (SolveStatus::UnderConstrained, Vec::new())
        } else {
            (SolveStatus::Solved, Vec::new())
        };
        SolveReport {
            status,
            dof,
            free_points,
            conflicting,
            free_curves,
            redundant: constraint_ids(&r.redundant),
        }
    }

    /// One live drag frame: `point` towards `to` within one frame's budget
    /// (Solidcraft's `Budget::frame`). The point is held at `to` and the
    /// rest solved; where the constraints do not allow that, it moves as
    /// close as they allow (`move_point_towards`); otherwise it stays.
    /// Drag the returned sketch for the next frame, so each frame continues
    /// from the last one.
    pub fn drag(&self, point: &SketchPointId, to: Point) -> Result<DragFrame> {
        let p = self.sketch.point_index(&point.0).ok_or_else(|| {
            Error::NotFound(format!("{DRAG_HELD}: no sketch point `{}`", point.0))
        })?;
        let locked = self.sketch.locked_points();
        let fixed = self
            .sketch
            .points
            .get(p)
            .is_none_or(|q| q.fixed || q.exprs.is_some());
        if p == 0 || fixed || locked.get(p).copied().unwrap_or(true) {
            return Err(Error::Invalid(format!(
                "{DRAG_HELD}: point `{}` is fixed and cannot move",
                point.0
            )));
        }
        if !(to[0].is_finite()
            && to[1].is_finite()
            && to[0].abs() <= MAX_COORD_MM
            && to[1].abs() <= MAX_COORD_MM)
        {
            return Err(Error::Invalid(format!(
                "{DRAG_HELD}: the target is outside the coordinate range"
            )));
        }
        let target = Vec2::new(to[0], to[1]);
        let reference: Vec<Vec2> = self.sketch.points.iter().map(|q| q.pos).collect();
        let from = reference.get(p).copied().unwrap_or(target);
        let mut budget = Budget::frame();
        let in_range = |sk: &Sketch| {
            sk.points.iter().all(|q| {
                q.pos.is_finite() && q.pos.x.abs() <= MAX_COORD_MM && q.pos.y.abs() <= MAX_COORD_MM
            })
        };
        let done = |sketch: Sketch, moved: bool, budget: &Budget| {
            let at = sketch.point(p).map_or(to, |v| [v.x, v.y]);
            DragFrame {
                sketch: LiveSketch {
                    stored: self.stored.clone(),
                    sketch,
                },
                at,
                moved,
                exhausted: budget.exhausted() || budget.numeric_exhausted(),
            }
        };
        // Held exactly at the pointer, the rest solved around it.
        let mut held = self.sketch.clone();
        if let Some(q) = held.points.get_mut(p) {
            q.pos = target;
        }
        let hold = Hold {
            points: vec![p],
            radii: Vec::new(),
        };
        let r = solve_with(
            &mut held,
            &hold,
            Some(&reference),
            Analysis::StatusOnly,
            Some(&mut budget),
        );
        if r.ok() && in_range(&held) {
            return Ok(done(held, true, &budget));
        }
        // As close as the constraints allow.
        let mut follow = self.sketch.clone();
        let max_step = (target - from).len().max(1e-3);
        if move_point_towards(
            &mut follow,
            p,
            target,
            max_step,
            Some(&mut budget),
            |_, _| true,
        ) && in_range(&follow)
        {
            let moved = follow.point(p).is_some_and(|v| (v - from).len() > 1e-9);
            return Ok(done(follow, moved, &budget));
        }
        Ok(done(self.sketch.clone(), false, &budget))
    }

    /// Every dimension laid out for drawing. `px` is the footprint length
    /// of one screen pixel; `half` gives each dimension's text half size
    /// (mm), which the dimension line leaves a gap for.
    pub fn dimensions(
        &self,
        px: f64,
        half: &dyn Fn(&SketchConstraintId) -> Point,
    ) -> Vec<DimensionLayout> {
        let sk = &self.sketch;
        let centres = chain_centres(sk);
        let pt = |v: Vec2| -> Point { [v.x, v.y] };
        sk.constraints
            .iter()
            .filter_map(|k| {
                let frame = dim_frame(sk, &k.kind)?;
                let id = SketchConstraintId(k.id.clone());
                let away = k
                    .kind
                    .indices()
                    .0
                    .first()
                    .and_then(|i| centres.get(*i).copied())
                    .unwrap_or(Vec2::ZERO);
                let text = k.text.or_else(|| default_text(&frame, away, px));
                let [hx, hy] = half(&id);
                let layout = dim_layout_reaching(
                    &frame,
                    text,
                    px,
                    Vec2::new(hx, hy),
                    dim_reach(sk, &k.kind),
                );
                Some(DimensionLayout {
                    id,
                    lines: layout
                        .lines
                        .iter()
                        .map(|l| l.iter().copied().map(pt).collect())
                        .collect(),
                    arrows: layout
                        .arrows
                        .iter()
                        .map(|(tip, dir)| (pt(*tip), pt(*dir)))
                        .collect(),
                    text: pt(layout.text),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::library::footprint_source::{
        DimKind, DimValue, Expr, FootprintEvaluator, FootprintParam, GeomConstraint, ParamUnit,
        SketchEdit, SketchRef,
    };

    #[test]
    fn the_view_lists_sketch_entities_by_id_at_their_stored_positions() -> Result<()> {
        let ev = super::super::SketchEvaluator;
        let sketch = ev.new_sketch()?;
        let drawn = ev.apply_sketch(
            &sketch,
            &SketchEdit::AddLine {
                a: [0.0, 0.0],
                b: [2.0, 1.0],
                construction: true,
            },
        )?;
        let source = FootprintSource {
            sketch: drawn.sketch,
            ..Default::default()
        };
        let view = sketch_view(&source)?;
        let line = view
            .curves
            .iter()
            .find(|c| matches!(c.geometry, ViewGeometry::Line { .. }))
            .ok_or_else(|| Error::Invalid("no line".into()))?;
        assert!(line.construction);
        assert_eq!(
            line.geometry,
            ViewGeometry::Line {
                a: [0.0, 0.0],
                b: [2.0, 1.0]
            }
        );
        assert_eq!(line.points.len(), 2);
        for p in &line.points {
            assert!(view.point(p).is_some());
        }
        Ok(())
    }

    /// Draws a source one sketch edit at a time through the evaluator,
    /// keeping the ids each edit created.
    struct Draw {
        source: FootprintSource,
    }

    impl Draw {
        fn new() -> Result<Self> {
            Ok(Self {
                source: FootprintSource {
                    sketch: super::super::SketchEvaluator.new_sketch()?,
                    ..Default::default()
                },
            })
        }
        fn edit(&mut self, edit: SketchEdit) -> Result<Vec<SketchRef>> {
            let out = super::super::SketchEvaluator.apply_sketch(&self.source.sketch, &edit)?;
            self.source.sketch = out.sketch;
            Ok(out.created)
        }
        /// A line; its id and its end points' ids.
        fn line(&mut self, a: Point, b: Point) -> Result<(SketchCurveId, [SketchPointId; 2])> {
            let created = self.edit(SketchEdit::AddLine {
                a,
                b,
                construction: false,
            })?;
            let curve = created.iter().find_map(|r| match r {
                SketchRef::Curve(c) => Some(c.clone()),
                _ => None,
            });
            let points: Vec<SketchPointId> = created
                .iter()
                .filter_map(|r| match r {
                    SketchRef::Point(p) => Some(p.clone()),
                    _ => None,
                })
                .collect();
            match (curve, points.as_slice()) {
                (Some(c), [a, b]) => Ok((c, [a.clone(), b.clone()])),
                _ => Err(Error::Invalid(format!("a line made {created:?}"))),
            }
        }
        fn constrain(&mut self, kind: GeomConstraint, refs: Vec<SketchRef>) -> Result<()> {
            self.edit(SketchEdit::AddConstraint { kind, refs })?;
            Ok(())
        }
        fn dimension(
            &mut self,
            kind: DimKind,
            refs: Vec<SketchRef>,
            value: DimValue,
        ) -> Result<SketchConstraintId> {
            self.edit(SketchEdit::AddDimension { kind, refs, value })?
                .into_iter()
                .find_map(|r| match r {
                    SketchRef::Constraint(k) => Some(k),
                    _ => None,
                })
                .ok_or_else(|| Error::Invalid("no dimension created".into()))
        }
        fn param(&mut self, name: &str, expr: &str, unit: ParamUnit) {
            self.source.parameters.push(FootprintParam {
                name: ParamName(name.into()),
                expr: Expr(expr.into()),
                unit,
                comment: String::new(),
            });
        }
    }

    fn origin() -> SketchRef {
        SketchRef::Point(SketchPointId("origin".into()))
    }

    fn at(view: &SketchView, id: &SketchPointId) -> Result<Point> {
        view.point(id)
            .map(|p| p.at)
            .ok_or_else(|| Error::NotFound(id.0.clone()))
    }

    fn near(a: Point, b: Point) -> bool {
        (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6
    }

    /// A free line has four degrees of freedom and both ends free; tying
    /// one end to the origin, making it horizontal and giving it a length
    /// leaves none.
    #[test]
    fn the_report_counts_freedom_and_names_free_geometry() -> Result<()> {
        let mut d = Draw::new()?;
        let (line, [a, b]) = d.line([0.0, 0.0], [2.0, 0.0])?;
        let free = solve_report(&d.source, &[])?;
        assert_eq!(free.status, SolveStatus::UnderConstrained);
        assert_eq!(free.dof, 4);
        assert_eq!(free.free_points, vec![a.clone(), b.clone()]);
        assert_eq!(free.free_curves, vec![line.clone()]);
        d.constrain(
            GeomConstraint::Coincident,
            vec![origin(), SketchRef::Point(a)],
        )?;
        d.constrain(
            GeomConstraint::Horizontal,
            vec![SketchRef::Curve(line.clone())],
        )?;
        let sliding = solve_report(&d.source, &[])?;
        assert_eq!(sliding.dof, 1);
        assert_eq!(sliding.free_points, vec![b]);
        d.dimension(
            DimKind::Distance,
            vec![SketchRef::Curve(line)],
            DimValue::Fixed(2.0),
        )?;
        let defined = solve_report(&d.source, &[])?;
        assert_eq!(defined.status, SolveStatus::Solved);
        assert_eq!(defined.dof, 0);
        assert!(defined.free_points.is_empty() && defined.free_curves.is_empty());
        assert!(defined.conflicting.is_empty());
        Ok(())
    }

    /// A line both horizontal and vertical cannot have a length: the report
    /// says over-constrained and names the constraints in conflict.
    #[test]
    fn conflicting_constraints_are_reported_over_constrained() -> Result<()> {
        let mut d = Draw::new()?;
        let (line, _) = d.line([0.0, 0.0], [2.0, 0.0])?;
        d.constrain(
            GeomConstraint::Horizontal,
            vec![SketchRef::Curve(line.clone())],
        )?;
        d.constrain(
            GeomConstraint::Vertical,
            vec![SketchRef::Curve(line.clone())],
        )?;
        let length = d.dimension(
            DimKind::Distance,
            vec![SketchRef::Curve(line)],
            DimValue::Fixed(2.0),
        )?;
        let report = solve_report(&d.source, &[])?;
        assert_eq!(report.status, SolveStatus::OverConstrained);
        assert!(!report.conflicting.is_empty(), "{report:?}");
        let names: Vec<_> = sketch_view(&d.source)?
            .constraints
            .iter()
            .map(|k| k.id.clone())
            .collect();
        assert!(names.contains(&length));
        for k in &report.conflicting {
            assert!(names.contains(k), "{k:?} is a sketch constraint");
        }
        Ok(())
    }

    /// A drag holds the point at the pointer and moves the rest as little
    /// as the constraints allow; a point the constraints only let slide
    /// follows the pointer along its freedom; a determined point stays.
    #[test]
    fn a_drag_frame_follows_the_pointer_within_the_constraints() -> Result<()> {
        let mut d = Draw::new()?;
        let (line, [a, b]) = d.line([0.0, 0.0], [2.0, 0.0])?;
        d.constrain(
            GeomConstraint::Horizontal,
            vec![SketchRef::Curve(line.clone())],
        )?;
        // Free horizontal line: b goes exactly to the pointer and a follows
        // it to the same height without moving along x.
        let view = drag_preview(&d.source, &[], &b, [3.0, 1.0])?;
        assert!(near(at(&view, &b)?, [3.0, 1.0]));
        assert!(near(at(&view, &a)?, [0.0, 1.0]), "{:?}", at(&view, &a));
        // With a on the origin, b can only slide along x.
        d.constrain(
            GeomConstraint::Coincident,
            vec![origin(), SketchRef::Point(a.clone())],
        )?;
        let live = LiveSketch::load(&d.source, &[])?;
        let frame = live.drag(&b, [3.0, 1.0])?;
        assert!(frame.moved);
        assert!(
            (frame.at[0] - 3.0).abs() < 1e-3 && frame.at[1].abs() < 1e-9,
            "{:?}",
            frame.at
        );
        assert!(near(at(&frame.sketch.view(), &a)?, [0.0, 0.0]));
        // A second frame continues from the first.
        let next = frame.sketch.drag(&b, [3.5, -1.0])?;
        assert!((next.at[0] - 3.5).abs() < 1e-3 && next.at[1].abs() < 1e-9);
        // Given a length, b is determined: it stays where it is.
        d.dimension(
            DimKind::Distance,
            vec![SketchRef::Curve(line)],
            DimValue::Fixed(2.0),
        )?;
        let fixed = LiveSketch::load(&d.source, &[])?.drag(&b, [3.0, 1.0])?;
        assert!(!fixed.moved);
        assert!(near(fixed.at, [2.0, 0.0]), "{:?}", fixed.at);
        // The origin never drags; the stored source is untouched.
        let before = d.source.clone();
        assert!(drag_preview(&d.source, &[], &SketchPointId("origin".into()), [1.0, 1.0]).is_err());
        assert_eq!(d.source, before);
        Ok(())
    }

    /// "Used by" follows parameters through other parameters; values are
    /// reported per parameter in its own unit, and one failing expression
    /// does not hide the others.
    #[test]
    fn parameters_report_values_and_the_dimensions_reading_them() -> Result<()> {
        let mut d = Draw::new()?;
        let (l1, _) = d.line([0.0, 0.0], [1.0, 0.0])?;
        let (l2, _) = d.line([0.0, 1.0], [2.0, 1.0])?;
        let (l3, _) = d.line([0.0, 2.0], [3.0, 2.0])?;
        d.param("pitch", "1.27 mm", ParamUnit::Mm);
        d.param("span", "2 * pitch + pkg_L", ParamUnit::Mm);
        d.param("turn", "45 deg * 2", ParamUnit::Deg);
        d.param("n", "pkg_n * 2", ParamUnit::Count);
        d.param("broken", "nope + 1 mm", ParamUnit::Mm);
        let by_pitch = d.dimension(
            DimKind::Distance,
            vec![SketchRef::Curve(l1)],
            DimValue::Param(ParamName("pitch".into())),
        )?;
        let by_span = d.dimension(
            DimKind::Distance,
            vec![SketchRef::Curve(l2)],
            DimValue::Param(ParamName("span".into())),
        )?;
        d.dimension(
            DimKind::Distance,
            vec![SketchRef::Curve(l3)],
            DimValue::Fixed(3.0),
        )?;
        let name = |n: &str| ParamName(n.into());
        assert_eq!(
            dimensions_using(&d.source, &name("pitch"))?,
            vec![by_pitch.clone(), by_span.clone()]
        );
        assert_eq!(
            dimensions_using(&d.source, &name("span"))?,
            vec![by_span.clone()]
        );
        assert_eq!(dimensions_using(&d.source, &name("pkg_L"))?, vec![by_span]);
        assert!(dimensions_using(&d.source, &name("turn"))?.is_empty());
        let package = [
            PackageValue {
                name: name("pkg_L"),
                value: ParamValue::Length(4.0),
            },
            PackageValue {
                name: name("pkg_n"),
                value: ParamValue::Count(8),
            },
        ];
        let values = parameter_values(&d.source, &package)?;
        let value = |n: &str| {
            values
                .iter()
                .find(|v| v.name.0 == n)
                .map(|v| v.value.clone())
        };
        assert_eq!(value("pitch"), Some(Ok(ParamValue::Length(1.27))));
        let span = match value("span") {
            Some(Ok(ParamValue::Length(v))) => v,
            other => return Err(Error::Invalid(format!("span {other:?}"))),
        };
        assert!((span - 6.54).abs() < 1e-12);
        match value("turn") {
            Some(Ok(ParamValue::Angle(deg))) => assert!((deg - 90.0).abs() < 1e-9),
            other => return Err(Error::Invalid(format!("turn {other:?}"))),
        }
        assert_eq!(value("n"), Some(Ok(ParamValue::Count(16))));
        assert!(matches!(value("broken"), Some(Err(e)) if e.contains("nope")));
        Ok(())
    }

    /// Dimension values in the view are millimetres and degrees; a length
    /// dimension of a horizontal 2 mm line is laid out with its arrows at
    /// the line's ends and its text above it.
    #[test]
    fn dimensions_are_shown_in_millimetres_and_degrees_and_laid_out() -> Result<()> {
        let mut d = Draw::new()?;
        let (flat, _) = d.line([0.0, 0.0], [2.0, 0.0])?;
        let (tilted, _) = d.line([0.0, 1.0], [3f64.sqrt(), 2.0])?;
        let length = d.dimension(
            DimKind::Distance,
            vec![SketchRef::Curve(flat)],
            DimValue::Fixed(2.0),
        )?;
        let angle = d.dimension(
            DimKind::Angle,
            vec![SketchRef::Curve(tilted)],
            DimValue::Fixed(30.0),
        )?;
        let view = sketch_view(&d.source)?;
        let value = |id: &SketchConstraintId| {
            view.constraints
                .iter()
                .find(|k| k.id == *id)
                .and_then(|k| k.value)
        };
        assert!(value(&length).is_some_and(|v| (v - 2.0).abs() < 1e-9));
        assert!(
            value(&angle).is_some_and(|v| (v - 30.0).abs() < 1e-9),
            "{:?}",
            value(&angle)
        );
        let px = 0.01;
        let live = LiveSketch::load(&d.source, &[])?;
        let layouts = live.dimensions(px, &|_| [0.1, 0.05]);
        let layout = layouts
            .iter()
            .find(|l| l.id == length)
            .ok_or_else(|| Error::NotFound("length layout".into()))?;
        let mut tips: Vec<f64> = layout.arrows.iter().map(|(tip, _)| tip[0]).collect();
        tips.sort_by(f64::total_cmp);
        assert_eq!(tips.len(), 2);
        assert!(
            (tips[0] - 0.0).abs() < 1e-9 && (tips[1] - 2.0).abs() < 1e-9,
            "{tips:?}"
        );
        assert!((layout.text[0] - 1.0).abs() < 1e-9);
        assert!(
            layout.text[1].abs() > 1e-3,
            "text off the line: {:?}",
            layout.text
        );
        assert!(!layout.lines.is_empty());
        assert!(layouts.iter().any(|l| l.id == angle));
        Ok(())
    }

    /// Drag-preview frame time against the 16 ms target (design §2.5) on
    /// 100 dimensioned, fully constrained rectangles with one rectangle
    /// left free to drag: solve, view and dimension layout per frame. Run with
    /// `cargo test --release -p pcbcraft-solidcraft drag_frame_time -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing measurement; run explicitly in release"]
    fn drag_frame_time() -> Result<()> {
        let mut d = Draw::new()?;
        let mut corner = None;
        for i in 0..100u32 {
            let (x, y) = (f64::from(i % 10) * 3.0, f64::from(i / 10) * 3.0);
            let created = d.edit(SketchEdit::AddRect {
                a: [x, y],
                b: [x + 2.0, y + 1.0],
                construction: false,
            })?;
            let lines: Vec<SketchCurveId> = created
                .iter()
                .filter_map(|r| match r {
                    SketchRef::Curve(c) => Some(c.clone()),
                    _ => None,
                })
                .collect();
            let points: Vec<SketchPointId> = created
                .iter()
                .filter_map(|r| match r {
                    SketchRef::Point(p) => Some(p.clone()),
                    _ => None,
                })
                .collect();
            if let [bottom, right, ..] = lines.as_slice() {
                d.dimension(
                    DimKind::Distance,
                    vec![SketchRef::Curve(bottom.clone())],
                    DimValue::Fixed(2.0),
                )?;
                d.dimension(
                    DimKind::Distance,
                    vec![SketchRef::Curve(right.clone())],
                    DimValue::Fixed(1.0),
                )?;
            }
            if let Some(p) = points.first() {
                if i == 55 {
                    corner = Some(p.clone());
                } else {
                    d.dimension(
                        DimKind::HorizontalDistance,
                        vec![origin(), SketchRef::Point(p.clone())],
                        DimValue::Fixed(x),
                    )?;
                    d.dimension(
                        DimKind::VerticalDistance,
                        vec![origin(), SketchRef::Point(p.clone())],
                        DimValue::Fixed(y),
                    )?;
                }
            }
        }
        let corner = corner.ok_or_else(|| Error::Invalid("no corner".into()))?;
        let mut live = LiveSketch::load(&d.source, &[])?;
        let start = live
            .view()
            .point(&corner)
            .map(|p| p.at)
            .ok_or_else(|| Error::NotFound("corner".into()))?;
        let mut times = Vec::new();
        for frame in 0..120u32 {
            let t = f64::from(frame) / 120.0 * std::f64::consts::TAU;
            let to = [start[0] + 0.8 * t.sin(), start[1] + 0.5 * (2.0 * t).sin()];
            let clock = std::time::Instant::now();
            // What the Footprint tab does per drag frame: the solve, the
            // view it draws and the dimension layout.
            let next = live.drag(&corner, to)?;
            let drawn = next.sketch.view();
            let dims = next.sketch.dimensions(0.01, &|_| [0.2, 0.1]);
            times.push(clock.elapsed().as_secs_f64() * 1e3);
            assert_eq!(drawn.points.len(), 401);
            assert_eq!(dims.len(), 398);
            assert!(next.moved, "frame {frame} moved");
            live = next.sketch;
        }
        times.sort_by(f64::total_cmp);
        let pct = |q: f64| {
            times
                .get(((times.len() - 1) as f64 * q) as usize)
                .copied()
                .unwrap_or(f64::NAN)
        };
        println!(
            "drag frame, 100 dimensioned rectangles ({} points): p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms (target 16 ms)",
            live.view().points.len(),
            pct(0.5),
            pct(0.95),
            pct(1.0)
        );
        assert!(pct(0.95) < 16.0);
        Ok(())
    }
}
