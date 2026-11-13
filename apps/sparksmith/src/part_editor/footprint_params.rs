//! The Footprint tab's parameters panel (slice F4): name, expression with
//! units, typed value and "used by" per parameter, with the package's
//! `pkg_*` values shown read-only. Edits commit as one
//! `library.edit_footprint` each (`SetParameter`, `RenameParameter`,
//! `RemoveParameter`).
//!
//! See docs/component-model.md (F4/F5 as built). Values and "used by" come from the evaluator
//! (`sparksmith_solidcraft::footprint::view`); the panel itself is
//! `footprint_params_ui`.
use std::collections::BTreeMap;

use sparksmith_core::library::footprint_source::{
    Expr, FootprintSource, PackageValue, ParamName, ParamUnit, ParamValue, SketchConstraintId,
};
use sparksmith_solidcraft::footprint::view as sketch;

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
    /// Every reader: dimensions, parameters, arrays, derived roles…
    pub readers: sketch::ParamReaders,
    /// A `pkg_*` package value: shown, never edited here.
    pub read_only: bool,
}

/// The table rows: package values first (read-only), then the source's
/// parameters in stored order, each with its evaluated value and the
/// dimensions reading it (directly or through other parameters).
pub fn rows(source: Option<&FootprintSource>, package: &[PackageValue]) -> Vec<ParamRow> {
    let mut rows = bare_rows(source, package);
    let Some(source) = source else {
        return rows;
    };
    let values: BTreeMap<ParamName, ParamValue> = sketch::parameter_values(source, package)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| Some((e.name, e.value.ok()?)))
        .collect();
    for row in &mut rows {
        if !row.read_only {
            row.value = values.get(&row.name).copied();
        }
        row.readers = sketch::readers(source, &row.name).unwrap_or_default();
        row.used_by = row.readers.dimensions.clone();
    }
    rows
}

/// Why each of the source's parameters has no value, by name.
pub fn errors(source: &FootprintSource, package: &[PackageValue]) -> BTreeMap<ParamName, String> {
    match sketch::parameter_values(source, package) {
        Ok(values) => values
            .into_iter()
            .filter_map(|e| Some((e.name, e.value.err()?)))
            .collect(),
        Err(e) => source
            .parameters
            .iter()
            .map(|p| (p.name.clone(), e.to_string()))
            .collect(),
    }
}

/// The readers as a phrase: "2 dimensions, array, courtyard"; `None`
/// when nothing reads the parameter.
pub fn readers_phrase(readers: &sketch::ParamReaders) -> Option<String> {
    let mut parts = Vec::new();
    match readers.dimensions.len() {
        0 => {}
        1 => parts.push("1 dimension".to_string()),
        n => parts.push(format!("{n} dimensions")),
    }
    for p in readers.parameters.iter().take(3) {
        parts.push(p.0.clone());
    }
    if readers.parameters.len() > 3 {
        parts.push(format!("{} more parameters", readers.parameters.len() - 3));
    }
    parts.extend(readers.roles.iter().map(|r| r.to_string()));
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// A value with its unit, as the table shows it.
pub fn value_text(value: ParamValue) -> String {
    match value {
        ParamValue::Length(v) => format!("{} mm", crate::kit::number_text(v)),
        ParamValue::Angle(v) => format!("{}°", crate::kit::number_text(v)),
        ParamValue::Count(n) => n.to_string(),
    }
}

/// The unit a new parameter gets from its expression: degrees when it
/// names an angle unit, else millimetres.
pub fn unit_of(expr: &str) -> ParamUnit {
    let words = expr
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .collect::<Vec<_>>();
    if words.iter().any(|w| matches!(*w, "deg" | "rad")) {
        ParamUnit::Deg
    } else {
        ParamUnit::Mm
    }
}

/// The rows without values or readers.
fn bare_rows(source: Option<&FootprintSource>, package: &[PackageValue]) -> Vec<ParamRow> {
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
            readers: sketch::ParamReaders::default(),
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
            readers: sketch::ParamReaders::default(),
            read_only: false,
        });
    package_rows.chain(own).collect()
}
