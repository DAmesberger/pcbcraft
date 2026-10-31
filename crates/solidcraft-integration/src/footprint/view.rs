//! A read-only, id-keyed view of a footprint sketch for the Footprint tab
//! (slices F4/F5): points, curves and constraints with their solved
//! positions, so the app draws, picks and snaps sketch geometry and shows
//! dimensions and constraint glyphs without depending on Solidcraft's
//! sketch types. Nothing here mutates; every change is one
//! `library.edit_footprint` commit.
//!
//! Contract (docs/f45-contract.md): [`sketch_view`] is final; the solver
//! report ([`solve_report`]) and the live drag ([`drag_preview`]) are
//! filled in by the dims task and refuse until then.
use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::geometry::Point;
use pcbcraft_core::library::footprint_source::{
    FootprintSource, PackageValue, ParamName, SketchConstraintId, SketchCurveId, SketchPointId,
    SolveStatus,
};
use solidcraft_engine::sketch::{CurveKind, Sketch};

use super::check;

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
    /// A dimension's current value (mm or degrees); `None` for geometric
    /// constraints.
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
    /// Constraints in conflict when over-constrained.
    pub conflicting: Vec<SketchConstraintId>,
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
                value: k.kind.value(),
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

/// Degrees of freedom, free points and conflicts of the source's sketch.
pub fn solve_report(
    _source: &FootprintSource,
    _package_values: &[PackageValue],
) -> Result<SolveReport> {
    Err(Error::Unsupported(
        "footprint.solve_report: not available yet".into(),
    ))
}

/// The sketch with `point` dragged towards `to`, solved within one frame's
/// budget (Solidcraft's `Budget::frame`). Never touches the document; the
/// release commits one `SketchEdit::Drag`.
pub fn drag_preview(
    _source: &FootprintSource,
    _package_values: &[PackageValue],
    _point: &SketchPointId,
    _to: Point,
) -> Result<SketchView> {
    Err(Error::Unsupported(
        "footprint.drag_preview: not available yet".into(),
    ))
}

/// Which dimensions read parameter `name` (directly or through other
/// parameters), for the parameters panel's "used by" column.
pub fn dimensions_using(
    _source: &FootprintSource,
    _name: &ParamName,
) -> Result<Vec<SketchConstraintId>> {
    Err(Error::Unsupported(
        "footprint.dimensions_using: not available yet".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::library::footprint_source::{FootprintEvaluator, SketchEdit};

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
}
