//! Original synthetic fixtures for the spike: SOIC-8, QFN-48 with an exposed
//! pad and BGA-256 with a centre void. Their numbers are round values chosen
//! for the test, not taken from any library, datasheet or EDA tool. Each is
//! built with Pcbcraft's own pad builders on Solidcraft's public `Sketch` API
//! (Solidcraft's own builders are private engine code).

use solidcraft_engine::geom::Vec2;
use solidcraft_engine::sketch::{ConstraintKind, PointExprs, Sketch};

use super::{
    Layout, Numbering, Outcome, PadArray, PadRole, Param, ParamUnit, Skip, Source, Template, check,
    finding,
};

fn sketch_error(e: solidcraft_engine::sketch::SketchError) -> super::Finding {
    finding("footprint.internal", e.to_string())
}

pub(crate) struct Builder {
    pub sketch: Sketch,
    pub pads: Vec<PadRole>,
    pub params: Vec<Param>,
}

impl Builder {
    pub(crate) fn new() -> Self {
        Self {
            sketch: Sketch::new(),
            pads: Vec::new(),
            params: Vec::new(),
        }
    }

    pub(crate) fn param(&mut self, name: &str, expr: &str, unit: ParamUnit) {
        self.params.push(Param {
            name: name.into(),
            expr: expr.into(),
            unit,
        });
    }

    fn point(&mut self, id: &str, at: Vec2) -> Outcome<usize> {
        self.sketch.add_point(at, Some(id)).map_err(sketch_error)
    }

    fn constrain(&mut self, kind: ConstraintKind, param: Option<&str>) -> Outcome<()> {
        self.sketch
            .add_constraint(kind, param.map(str::to_string))
            .map(|_| ())
            .map_err(sketch_error)
    }

    /// Place `anchor` from the origin: by two bound distances, by two plain
    /// distances when not bound, or coincident when it is the origin.
    fn locate(&mut self, anchor: usize, at: Vec2, by: Option<(&str, &str)>) -> Outcome<()> {
        if by.is_none() && at.x == 0.0 && at.y == 0.0 {
            return self.constrain(ConstraintKind::Coincident { p: 0, q: anchor }, None);
        }
        let (x, y) = by.map_or((None, None), |(x, y)| (Some(x), Some(y)));
        self.constrain(
            ConstraintKind::DistanceX {
                p: 0,
                q: anchor,
                value: at.x.abs(),
            },
            x,
        )?;
        self.constrain(
            ConstraintKind::DistanceY {
                p: 0,
                q: anchor,
                value: at.y.abs(),
            },
            y,
        )
    }

    /// A fully constrained rectangular pad template centred on a new anchor:
    /// four role-owned sides, H/V, width and height bound to parameters, and
    /// the anchor at the midpoint of a construction diagonal.
    pub(crate) fn rect_pad(
        &mut self,
        name: &str,
        at: Vec2,
        size: (f64, f64),
        size_params: (&str, &str),
        locate: Option<(&str, &str)>,
    ) -> Outcome<()> {
        let anchor = self.point(name, at)?;
        let (hw, hh) = (size.0 * 0.5, size.1 * 0.5);
        let offsets = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)];
        let mut corners = Vec::new();
        for (k, (dx, dy)) in offsets.into_iter().enumerate() {
            corners.push(self.point(&format!("{name}.c{k}"), Vec2::new(at.x + dx, at.y + dy))?);
        }
        let corner = |k: usize| {
            corners
                .get(k)
                .copied()
                .ok_or_else(|| finding("footprint.internal", "corner"))
        };
        let mut sides = Vec::new();
        for k in 0..4 {
            let line = self
                .sketch
                .add_line_pts(
                    corner(k)?,
                    corner((k + 1) % 4)?,
                    Some(&format!("{name}.s{k}")),
                )
                .map_err(sketch_error)?;
            sides.push(line);
        }
        let side = |k: usize| {
            sides
                .get(k)
                .copied()
                .ok_or_else(|| finding("footprint.internal", "side"))
        };
        let diagonal = self
            .sketch
            .add_line_pts(corner(0)?, corner(2)?, Some(&format!("{name}.diag")))
            .map_err(sketch_error)?;
        if let Some(c) = self.sketch.curves.get_mut(diagonal) {
            c.construction = true;
        }
        self.constrain(ConstraintKind::Horizontal { l: side(0)? }, None)?;
        self.constrain(ConstraintKind::Vertical { l: side(1)? }, None)?;
        self.constrain(ConstraintKind::Horizontal { l: side(2)? }, None)?;
        self.constrain(ConstraintKind::Vertical { l: side(3)? }, None)?;
        self.constrain(
            ConstraintKind::DistanceX {
                p: corner(0)?,
                q: corner(1)?,
                value: size.0,
            },
            Some(size_params.0),
        )?;
        self.constrain(
            ConstraintKind::DistanceY {
                p: corner(1)?,
                q: corner(2)?,
                value: size.1,
            },
            Some(size_params.1),
        )?;
        self.constrain(
            ConstraintKind::Midpoint {
                p: anchor,
                l: diagonal,
            },
            None,
        )?;
        self.locate(anchor, at, locate)?;
        let ids = |v: &[usize], f: &dyn Fn(usize) -> Option<String>| -> Outcome<[String; 4]> {
            let out: Vec<String> = v.iter().filter_map(|&i| f(i)).collect();
            out.try_into()
                .map_err(|_| finding("footprint.internal", "template ids"))
        };
        let sketch = &self.sketch;
        let corner_ids = ids(&corners, &|i| sketch.points.get(i).map(|p| p.id.clone()))?;
        let side_ids = ids(&sides, &|i| sketch.curves.get(i).map(|c| c.id.clone()))?;
        self.pads.push(PadRole {
            anchor: name.into(),
            template: Template::Rect {
                corners: corner_ids,
                sides: side_ids,
            },
            number: name.into(),
        });
        Ok(())
    }

    /// A circular land: a circle whose centre is the anchor, diameter bound.
    pub(crate) fn circle_pad(
        &mut self,
        name: &str,
        at: Vec2,
        d: f64,
        d_param: &str,
        locate: Option<(&str, &str)>,
    ) -> Outcome<()> {
        let anchor = self.point(name, at)?;
        let id = format!("{name}.land");
        let circle = self
            .sketch
            .add_circle(at, d * 0.5, Some(anchor), Some(&id))
            .map_err(sketch_error)?;
        self.constrain(
            ConstraintKind::Diameter {
                c: circle,
                value: d,
            },
            Some(d_param),
        )?;
        self.locate(anchor, at, locate)?;
        self.pads.push(PadRole {
            anchor: name.into(),
            template: Template::Circle { curve: id },
            number: name.into(),
        });
        Ok(())
    }

    /// A closed non-copper outline through points driven by expressions
    /// (`PointExprs`, as the package envelope is drawn).
    pub(crate) fn expr_outline(
        &mut self,
        name: &str,
        corners: [(&str, &str, Vec2); 4],
        construction: bool,
    ) -> Outcome<()> {
        let mut pts = Vec::new();
        for (k, (x, y, at)) in corners.into_iter().enumerate() {
            let i = self.point(&format!("{name}.p{k}"), at)?;
            if let Some(p) = self.sketch.points.get_mut(i) {
                p.fixed = true;
                p.exprs = Some(Box::new(PointExprs {
                    x: Some(x.into()),
                    y: Some(y.into()),
                }));
            }
            pts.push(i);
        }
        for k in 0..4 {
            let (Some(&a), Some(&b)) = (pts.get(k), pts.get((k + 1) % 4)) else {
                return Err(finding("footprint.internal", "outline"));
            };
            let line = self
                .sketch
                .add_line_pts(a, b, Some(&format!("{name}.s{k}")))
                .map_err(sketch_error)?;
            if let Some(c) = self.sketch.curves.get_mut(line) {
                c.construction = construction;
            }
        }
        Ok(())
    }

    pub(crate) fn finish(self, arrays: Vec<PadArray>) -> Outcome<Source> {
        Ok(Source {
            evaluator_rev: crate::SOLIDCRAFT_REVISION.into(),
            sketch_json: check::canonical(&self.sketch)?,
            params: self.params,
            pads: self.pads,
            arrays,
        })
    }
}

/// SOIC-8: package values `pkg_e` (pitch), `pkg_E` (body width) and `pkg_D`
/// (body length).
pub(crate) const SOIC8_PKG: &[(&str, f64)] = &[("e", 1.27), ("E", 3.9), ("D", 4.9)];

/// SOIC-8: a rectangular seed pad and a dual-row array (2 × 4), pitch from
/// the package, row spacing and pad size as footprint parameters. A non-copper
/// assembly outline of the body overlaps the pads' inner ends, as a real
/// gull-wing body can: the copper-only pass must ignore it.
pub(crate) fn soic8() -> Outcome<Source> {
    let mut b = Builder::new();
    b.param("pitch", "pkg_e", ParamUnit::Mm);
    b.param("per_row", "4", ParamUnit::Count);
    b.param("row_span", "5.4 mm", ParamUnit::Mm);
    b.param("pad_l", "1.55 mm", ParamUnit::Mm);
    b.param("pad_w", "0.6 mm", ParamUnit::Mm);
    b.param("seed_x", "row_span / 2", ParamUnit::Mm);
    b.param("seed_y", "(per_row - 1) / 2 * pitch", ParamUnit::Mm);
    b.rect_pad(
        "pad1",
        Vec2::new(-2.7, 1.905),
        (1.55, 0.6),
        ("pad_l", "pad_w"),
        Some(("seed_x", "seed_y")),
    )?;
    b.expr_outline(
        "assembly",
        [
            ("-pkg_E / 2", "-pkg_D / 2", Vec2::new(-1.95, -2.45)),
            ("pkg_E / 2", "-pkg_D / 2", Vec2::new(1.95, -2.45)),
            ("pkg_E / 2", "pkg_D / 2", Vec2::new(1.95, 2.45)),
            ("-pkg_E / 2", "pkg_D / 2", Vec2::new(-1.95, 2.45)),
        ],
        false,
    )?;
    b.finish(vec![PadArray {
        id: "rows".into(),
        seed: "pad1".into(),
        layout: Layout::DualRow {
            per_row: "per_row".into(),
            pitch: "pitch".into(),
            row_span: "row_span".into(),
        },
        skip: Skip::default(),
        numbering: Numbering::Sequential,
    }])
}

pub(crate) const QFN48_PKG: &[(&str, f64)] = &[("e", 0.5), ("D2", 5.2)];

/// QFN-48 (7 × 7 synthetic): a seed pad repeated on four sides (4 × 12) and
/// an exposed pad at the origin with a concentric non-copper thermal drill
/// circle. Unfiltered, that drill would be the innermost profile at the
/// exposed pad's anchor.
pub(crate) fn qfn48() -> Outcome<Source> {
    let mut b = Builder::new();
    b.param("pitch", "pkg_e", ParamUnit::Mm);
    b.param("per_side", "12", ParamUnit::Count);
    b.param("span", "6.8 mm", ParamUnit::Mm);
    b.param("pad_l", "0.8 mm", ParamUnit::Mm);
    b.param("pad_w", "0.25 mm", ParamUnit::Mm);
    b.param("ep", "pkg_D2", ParamUnit::Mm);
    b.param("drill", "0.3 mm", ParamUnit::Mm);
    b.param("seed_x", "span / 2", ParamUnit::Mm);
    b.param("seed_y", "(per_side - 1) / 2 * pitch", ParamUnit::Mm);
    b.rect_pad(
        "pad1",
        Vec2::new(-3.4, 2.75),
        (0.8, 0.25),
        ("pad_l", "pad_w"),
        Some(("seed_x", "seed_y")),
    )?;
    b.rect_pad("ep", Vec2::new(0.0, 0.0), (5.2, 5.2), ("ep", "ep"), None)?;
    if let Some(ep) = b.pads.last_mut() {
        ep.number = "EP".into();
    }
    let centre = b
        .sketch
        .point_index("ep")
        .ok_or_else(|| finding("footprint.internal", "ep"))?;
    let drill = b
        .sketch
        .add_circle(Vec2::new(0.0, 0.0), 0.15, Some(centre), Some("ep.drill"))
        .map_err(sketch_error)?;
    b.constrain(
        ConstraintKind::Diameter {
            c: drill,
            value: 0.3,
        },
        Some("drill"),
    )?;
    b.finish(vec![PadArray {
        id: "sides".into(),
        seed: "pad1".into(),
        layout: Layout::Quad {
            per_side: "per_side".into(),
            pitch: "pitch".into(),
            span: "span".into(),
        },
        skip: Skip::default(),
        numbering: Numbering::Sequential,
    }])
}

pub(crate) const BGA256_PKG: &[(&str, f64)] = &[("e", 1.0)];

/// BGA-256: one circular seed land and a 16 × 16 grid at 1.0 mm with the
/// centre 4 × 4 removed (240 lands), row letters without I, O, Q, S, X, Z.
pub(crate) fn bga256() -> Outcome<Source> {
    let mut b = Builder::new();
    b.param("pitch", "pkg_e", ParamUnit::Mm);
    b.param("rows", "16", ParamUnit::Count);
    b.param("cols", "16", ParamUnit::Count);
    b.param("land_d", "0.45 mm", ParamUnit::Mm);
    b.param("seed_x", "(cols - 1) / 2 * pitch", ParamUnit::Mm);
    b.param("seed_y", "(rows - 1) / 2 * pitch", ParamUnit::Mm);
    b.circle_pad(
        "a1",
        Vec2::new(-7.5, 7.5),
        0.45,
        "land_d",
        Some(("seed_x", "seed_y")),
    )?;
    b.finish(vec![PadArray {
        id: "grid".into(),
        seed: "a1".into(),
        layout: Layout::Grid {
            rows: "rows".into(),
            cols: "cols".into(),
            pitch: "pitch".into(),
        },
        skip: Skip {
            explicit: Vec::new(),
            centre_void: Some([4, 4]),
        },
        numbering: Numbering::GridAlpha,
    }])
}

/// For comparison only: BGA-256 with every land drawn and dimensioned in the
/// sketch (240 circles, 720 dimensions), no array. Shows what the solver and
/// profile pass cost when repetition is not kept outside them.
pub(crate) fn bga256_flat() -> Outcome<Source> {
    let mut b = Builder::new();
    b.param("land_d", "0.45 mm", ParamUnit::Mm);
    for i in 0..16u16 {
        for j in 0..16u16 {
            if (6..10).contains(&i) && (6..10).contains(&j) {
                continue;
            }
            let at = Vec2::new(f64::from(j) - 7.5, 7.5 - f64::from(i));
            b.circle_pad(&format!("l{i}_{j}"), at, 0.45, "land_d", None)?;
        }
    }
    b.finish(Vec::new())
}

/// A named fixture with its package values.
pub(crate) type Case = (&'static str, Source, &'static [(&'static str, f64)]);

/// The three fixtures with their package values.
pub(crate) fn all() -> Outcome<Vec<Case>> {
    Ok(vec![
        ("SOIC-8", soic8()?, SOIC8_PKG),
        ("QFN-48+EP", qfn48()?, QFN48_PKG),
        ("BGA-256", bga256()?, BGA256_PKG),
    ])
}
