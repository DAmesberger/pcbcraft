//! The Footprint tab's parameters panel (slice F4): name, expression with
//! units, typed value and "used by" per parameter, with the package's
//! `pkg_*` values shown read-only. Edits commit as one
//! `library.edit_footprint` each (`SetParameter`, `RenameParameter`,
//! `RemoveParameter`).
//!
//! Contract (docs/f45-contract.md): [`ParamRow`] and [`rows`] are final;
//! the dims task fills in values, "used by" and the panel UI.
#![expect(
    dead_code,
    reason = "F4/F5 contract: the dims task wires the parameters panel"
)]
use pcbcraft_core::library::footprint_source::{
    Expr, FootprintSource, PackageValue, ParamName, ParamUnit, ParamValue, SketchConstraintId,
};

/// One row of the parameters table.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamRow {
    pub name: ParamName,
    /// The stored expression; for a package value, its value as text.
    pub expr: Expr,
    pub unit: ParamUnit,
    /// The evaluated value; `None` until evaluated or when it fails.
    pub value: Option<ParamValue>,
    pub comment: String,
    /// Dimensions that read the parameter.
    pub used_by: Vec<SketchConstraintId>,
    /// A `pkg_*` package value: shown, never edited here.
    pub read_only: bool,
}

/// The table rows: package values first (read-only), then the source's
/// parameters in stored order.
pub fn rows(source: Option<&FootprintSource>, package: &[PackageValue]) -> Vec<ParamRow> {
    let package_rows = package.iter().map(|p| {
        let (text, unit) = match p.value {
            ParamValue::Length(v) => (format!("{v}"), ParamUnit::Mm),
            ParamValue::Angle(v) => (format!("{v}"), ParamUnit::Deg),
            ParamValue::Count(n) => (n.to_string(), ParamUnit::Count),
        };
        ParamRow {
            name: p.name.clone(),
            expr: Expr(text),
            unit,
            value: Some(p.value),
            comment: String::new(),
            used_by: Vec::new(),
            read_only: true,
        }
    });
    let own = source
        .into_iter()
        .flat_map(|s| &s.parameters)
        .map(|p| ParamRow {
            name: p.name.clone(),
            expr: p.expr.clone(),
            unit: p.unit,
            value: None,
            comment: p.comment.clone(),
            used_by: Vec::new(),
            read_only: false,
        });
    package_rows.chain(own).collect()
}
