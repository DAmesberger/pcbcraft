//! On-canvas dimensions and constraints of the Footprint sketch (slice F4):
//! driving dimensions bound to named parameters through Solidcraft's
//! `Constraint.param`, inline value editing, constraint glyphs, under- and
//! over-constrained feedback and the drag preview. Reads the sketch through
//! `pcbcraft_solidcraft::footprint::view`; every change is one
//! `library.edit_footprint` commit.
//!
//! Contract (docs/f45-contract.md): [`DimInput`] and its parser are final;
//! the dims task fills in drawing, picking and the inline editor.
#![expect(
    dead_code,
    reason = "F4/F5 contract: the dims task wires dimensions into the canvas"
)]
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, Expr, FootprintEdit, ParamName, ParamUnit, SketchConstraintId, SketchEdit,
};

/// What the author typed into a dimension's value field.
#[derive(Clone, Debug, PartialEq)]
pub enum DimInput {
    /// A plain number in the dimension's unit (mm or degrees).
    Value(f64),
    /// An existing parameter by name.
    Param(ParamName),
    /// An expression (`2*pitch`); it becomes a new parameter named after
    /// the dimension, since a bound dimension names one parameter.
    Expr(Expr),
    /// `name = expr`: define (or redefine) a parameter and bind to it.
    Define { name: ParamName, expr: Expr },
}

/// Parse the value field: a number, a bare parameter name, `name = expr`,
/// or any other expression. The grammar itself is the evaluator's; this
/// only splits the forms.
pub fn parse_input(text: &str) -> Result<DimInput, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Type a value, a parameter or name = expression".into());
    }
    if let Some((name, expr)) = text.split_once('=') {
        let name = ParamName(name.trim().into());
        let expr = Expr(expr.trim().into());
        if !name.valid() {
            return Err(format!("`{}` is not a parameter name", name.0));
        }
        if name.is_package() {
            return Err("Package values (pkg_…) are read-only".into());
        }
        if !expr.valid() {
            return Err("The expression is empty or too long".into());
        }
        return Ok(DimInput::Define { name, expr });
    }
    if let Ok(v) = text.parse::<f64>() {
        return if v.is_finite() {
            Ok(DimInput::Value(v))
        } else {
            Err("The value must be finite".into())
        };
    }
    let name = ParamName(text.into());
    if name.valid() {
        return Ok(DimInput::Param(name));
    }
    let expr = Expr(text.into());
    if expr.valid() {
        Ok(DimInput::Expr(expr))
    } else {
        Err("The expression is empty or too long".into())
    }
}

/// The edits that set dimension `constraint` from `input`; `fallback` names
/// the parameter an anonymous expression creates.
pub fn input_edits(
    constraint: &SketchConstraintId,
    kind: DimKind,
    input: DimInput,
    fallback: ParamName,
) -> Vec<FootprintEdit> {
    let unit = if kind == DimKind::Angle {
        ParamUnit::Deg
    } else {
        ParamUnit::Mm
    };
    let set = |value| {
        FootprintEdit::Sketch(SketchEdit::SetDimension {
            constraint: constraint.clone(),
            value,
        })
    };
    match input {
        DimInput::Value(v) => vec![set(DimValue::Fixed(v))],
        DimInput::Param(name) => vec![set(DimValue::Param(name))],
        DimInput::Expr(expr) => vec![
            FootprintEdit::SetParameter {
                name: fallback.clone(),
                expr,
                unit,
            },
            set(DimValue::Param(fallback)),
        ],
        DimInput::Define { name, expr } => vec![
            FootprintEdit::SetParameter {
                name: name.clone(),
                expr,
                unit,
            },
            set(DimValue::Param(name)),
        ],
    }
}

/// A dimension label being edited inline (double-click on its label).
#[derive(Clone, Debug, PartialEq)]
pub struct InlineEdit {
    pub constraint: SketchConstraintId,
    pub kind: DimKind,
    pub text: String,
    /// The parse error shown under the field.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_value_field_splits_numbers_names_definitions_and_expressions() {
        assert_eq!(parse_input(" 1.27 "), Ok(DimInput::Value(1.27)));
        assert_eq!(
            parse_input("pitch"),
            Ok(DimInput::Param(ParamName("pitch".into())))
        );
        assert_eq!(
            parse_input("span = pkg_E1 + pad_l"),
            Ok(DimInput::Define {
                name: ParamName("span".into()),
                expr: Expr("pkg_E1 + pad_l".into()),
            })
        );
        assert_eq!(
            parse_input("2*pitch"),
            Ok(DimInput::Expr(Expr("2*pitch".into())))
        );
        assert!(parse_input("").is_err());
        assert!(parse_input("pkg_e = 1").is_err());
        assert!(parse_input("1x = 2").is_err());
    }
}
