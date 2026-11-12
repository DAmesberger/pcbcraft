//! The parametric footprint evaluator (slice F2): [`SketchEvaluator`]
//! implements the core's [`FootprintEvaluator`] on Solidcraft's public
//! sketch solver and expression language, reached only through the
//! `solidcraft_engine::{sketch, doc::expr, geom}` re-exports. It replaces
//! the test-only F0 spike; measurements are in `docs/footprint-spike.md`.
//!
//! - [`check`]: the only way a stored sketch is loaded.
//! - [`params`]: parameters with typed `pkg_*` package values.
//! - [`edit`]: sketch edits and pad templates.
//! - [`evaluate`]: solve, copper-only profile pass, classification, arrays,
//!   derived graphics, datum, quantization and the result hash.
//! - [`view`]: a read-only, id-keyed sketch view for the Footprint tab.

mod arrays;
pub(crate) mod check;
mod classify;
pub(crate) mod edit;
pub(crate) mod evaluate;
pub(crate) mod params;
#[cfg(test)]
mod tests;
pub mod view;

use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::geometry::Point;
use pcbcraft_core::library::footprint_source::{
    ArrayLayout, DerivedGraphic, Evaluation, EvaluationRequest, EvaluatorRef, Expr,
    FootprintEvaluator, FootprintSource, MaskRuleExpr, PackageValue, PadShapeExpr, PadTemplate,
    PadTemplateParams, ParamName, PasteRuleExpr, SketchBlob, SketchCurveId, SketchEdit,
    SketchOutcome, SketchPointId, TemplatePad,
};
use solidcraft_engine::sketch::Sketch;

/// The engine name stored in [`EvaluatorRef::engine`].
pub const ENGINE: &str = "solidcraft-sketch";

/// The evaluator reference this build writes: [`ENGINE`] at
/// [`SOLIDCRAFT_REVISION`](crate::SOLIDCRAFT_REVISION).
pub fn evaluator_ref() -> EvaluatorRef {
    EvaluatorRef {
        engine: ENGINE.into(),
        rev: crate::SOLIDCRAFT_REVISION.into(),
    }
}

/// A rule a source or edit breaks, with its finding code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    pub code: &'static str,
    pub message: String,
}

pub(crate) fn problem(code: &'static str, message: impl Into<String>) -> Problem {
    Problem {
        code,
        message: message.into(),
    }
}

pub(crate) type Outcome<T> = std::result::Result<T, Problem>;

/// A solved value stored through the rounding crossing (design §2.2): a
/// value it refuses (not finite, beyond ±2 m) is `footprint.too_large`.
pub(crate) fn stored<T>(value: Result<T>) -> Outcome<T> {
    value.map_err(|e| problem("footprint.too_large", e.to_string()))
}

impl From<Problem> for Error {
    fn from(p: Problem) -> Self {
        let message = format!("{}: {}", p.code, p.message);
        match p.code {
            "footprint.too_large" => Error::Limit(message),
            "footprint.evaluator_rev" | "footprint.sketch_unsupported" => {
                Error::Unsupported(message)
            }
            _ => Error::Invalid(message),
        }
    }
}

/// Solidcraft's sketch solver as Pcbcraft's footprint evaluator. Stateless;
/// every call is pure in its inputs.
#[derive(Clone, Copy, Debug, Default)]
pub struct SketchEvaluator;

impl FootprintEvaluator for SketchEvaluator {
    fn evaluator(&self) -> EvaluatorRef {
        evaluator_ref()
    }

    fn apply_sketch(&self, sketch: &SketchBlob, edit: &SketchEdit) -> Result<SketchOutcome> {
        let stored = check::check_sketch(&sketch.0)?;
        let edited = edit::apply(&stored, edit)?;
        let json = check::canonical(&edited)?;
        check::check_sketch(&json)?;
        Ok(edit::diff(&stored, &edited, json))
    }

    fn evaluate(&self, request: EvaluationRequest<'_>) -> Result<Evaluation> {
        Ok(evaluate::run(request))
    }

    fn new_sketch(&self) -> Result<SketchBlob> {
        Ok(SketchBlob(check::canonical(&Sketch::new())?))
    }

    fn add_template_pad(
        &self,
        source: &FootprintSource,
        package_values: &[PackageValue],
        template: PadTemplate,
        at: Point,
        params: &PadTemplateParams,
    ) -> Result<TemplatePad> {
        let stored = check::check_sketch(&source.sketch.0)?;
        let values = params::evaluate(&source.parameters, package_values)?;
        let drawn = edit::template_pad(&stored, &values, template, at, params)?;
        let json = check::canonical(&drawn.sketch)?;
        check::check_sketch(&json)?;
        Ok(TemplatePad {
            outcome: edit::diff(&stored, &drawn.sketch, json),
            anchor: SketchPointId(drawn.anchor),
            owned_curves: drawn.owned.into_iter().map(SketchCurveId).collect(),
        })
    }

    fn rename_parameter(
        &self,
        source: &FootprintSource,
        from: &ParamName,
        to: &ParamName,
    ) -> Result<FootprintSource> {
        params::check_name(&to.0)?;
        if to.is_package() || from.is_package() {
            return Err(Error::Invalid(format!(
                "footprint.param_name: package values (`{}`) cannot be renamed",
                pcbcraft_core::library::footprint_source::PACKAGE_PREFIX
            )));
        }
        if !source.parameters.iter().any(|p| p.name == *from) {
            return Err(Error::NotFound(format!("no parameter `{}`", from.0)));
        }
        if source.parameters.iter().any(|p| p.name == *to) {
            return Err(Error::Conflict(format!(
                "parameter `{}` already exists",
                to.0
            )));
        }
        let mut out = source.clone();
        for p in &mut out.parameters {
            if p.name == *from {
                p.name = to.clone();
            }
            p.expr.0 = params::rename(&p.expr.0, &from.0, &to.0);
        }
        for e in exprs_mut(&mut out) {
            e.0 = params::rename(&e.0, &from.0, &to.0);
        }
        let mut sketch = check::check_sketch(&source.sketch.0)?;
        for k in &mut sketch.constraints {
            if let Some(param) = &mut k.param {
                *param = params::rename(param, &from.0, &to.0);
            }
        }
        for p in &mut sketch.points {
            if let Some(exprs) = p.exprs.as_deref_mut() {
                for e in exprs.list_mut() {
                    *e = params::rename(e, &from.0, &to.0);
                }
            }
        }
        out.sketch = SketchBlob(check::canonical(&sketch)?);
        Ok(out)
    }

    fn references(&self, source: &FootprintSource, name: &ParamName) -> Result<bool> {
        let mut copy = source.clone();
        let mentions = |text: &str| params::references(text).contains(&name.0);
        if copy
            .parameters
            .iter()
            .any(|p| p.name != *name && mentions(&p.expr.0))
        {
            return Ok(true);
        }
        if exprs_mut(&mut copy).into_iter().any(|e| mentions(&e.0)) {
            return Ok(true);
        }
        let sketch = check::check_sketch(&source.sketch.0)?;
        Ok(sketch
            .constraints
            .iter()
            .filter_map(|k| k.param.as_deref())
            .chain(
                sketch
                    .points
                    .iter()
                    .filter_map(|p| p.exprs.as_deref())
                    .flat_map(|e| e.list().into_iter().map(|(_, s)| s.as_str())),
            )
            .any(mentions))
    }
}

fn shape_exprs(shape: &mut PadShapeExpr) -> Vec<&mut Expr> {
    match shape {
        PadShapeExpr::Rect { width, height } | PadShapeExpr::Oblong { width, height } => {
            vec![width, height]
        }
        PadShapeExpr::RoundRect {
            width,
            height,
            radius_ratio,
        } => vec![width, height, radius_ratio],
        PadShapeExpr::Circle { diameter } => vec![diameter],
    }
}

fn mask_expr(m: &mut MaskRuleExpr) -> Option<&mut Expr> {
    match m {
        MaskRuleExpr::Expansion(e) => Some(e),
        _ => None,
    }
}

fn paste_expr(p: &mut PasteRuleExpr) -> Option<&mut Expr> {
    match p {
        PasteRuleExpr::Reduction(e) => Some(e),
        _ => None,
    }
}

/// Every expression of a source outside the parameter definitions and the
/// sketch: roles, arrays, overrides and graphic roles.
fn exprs_mut(source: &mut FootprintSource) -> Vec<&mut Expr> {
    let mut out: Vec<&mut Expr> = Vec::new();
    for role in &mut source.pads {
        out.extend(mask_expr(&mut role.mask));
        out.extend(paste_expr(&mut role.paste));
        if let Some(v) = &mut role.vias {
            out.extend([&mut v.rows, &mut v.cols, &mut v.pitch, &mut v.drill]);
        }
    }
    for array in &mut source.arrays {
        match &mut array.layout {
            ArrayLayout::Linear { count, pitch, .. } => out.extend([count, pitch]),
            ArrayLayout::DualRow {
                per_row,
                pitch,
                row_span,
                ..
            } => out.extend([per_row, pitch, row_span]),
            ArrayLayout::Quad {
                nx,
                ny,
                pitch,
                span_x,
                span_y,
                ..
            } => out.extend([nx, ny, pitch, span_x, span_y]),
            ArrayLayout::Grid {
                rows,
                cols,
                pitch_x,
                pitch_y,
                ..
            } => out.extend([rows, cols, pitch_x, pitch_y]),
        }
    }
    for o in &mut source.overrides {
        if let Some(s) = &mut o.value.shape {
            out.extend(shape_exprs(s));
        }
        if let Some(m) = &mut o.value.mask {
            out.extend(mask_expr(m));
        }
        if let Some(p) = &mut o.value.paste {
            out.extend(paste_expr(p));
        }
    }
    for g in &mut source.graphics {
        out.push(&mut g.width);
        if let pcbcraft_core::library::footprint_source::GraphicSource::Derived(
            DerivedGraphic::CourtyardFromExtents { excess, round },
        ) = &mut g.source
        {
            out.extend([excess, round]);
        }
    }
    out
}
