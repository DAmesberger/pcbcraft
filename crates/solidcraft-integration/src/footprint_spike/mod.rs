//! **Spike F0 — not a product API.** A headless prototype of the parametric
//! footprint evaluator described in the footprint design (slice F0). It proves
//! that Solidcraft's public sketch solver and expression language, reached only
//! through the `solidcraft_engine::{sketch, doc::expr}` re-exports, can drive
//! footprints with stable pad identity. It is compiled for tests only; the real
//! evaluator (F2) replaces it and owns its own types in core. Findings and
//! measurements are written up in `docs/footprint-spike.md`.
//!
//! Pipeline (design §2.5): revision check → [`check::check_sketch`] →
//! parameters (`doc::expr::eval_params`, package values as `pkg_<name>`) →
//! point expressions and bound dimensions → solve → copper-only profile pass →
//! classification of seed pads → rigid array copies outside the solver →
//! quantization to 1e-6 mm and a result hash.

pub(crate) mod bridge;
pub(crate) mod check;
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use serde::Serialize;
use solidcraft_engine::doc::expr::{self, Kind, ParamDef, Value};
use solidcraft_engine::geom::Vec2;
use solidcraft_engine::sketch::{
    Analysis, CurveKind, Sketch, find_profiles, flips_from_stored, profile_at_point,
    solve_after_edit_with_analysis,
};

/// Spike caps (design §2.6); the product moves them to core.
pub(crate) const MAX_PARAMS: usize = 256;
pub(crate) const MAX_ARRAYS: usize = 32;
pub(crate) const MAX_PADS: usize = 1024;
/// Quantum of every stored length: 1e-6 mm (one nanometre).
pub(crate) const QUANTUM_MM: f64 = 1e-6;
/// Reserved prefix of injected package values.
pub(crate) const PKG_PREFIX: &str = "pkg_";

/// A rule broken by a source, with the design's finding id.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{rule}: {message}")]
pub(crate) struct Finding {
    pub rule: &'static str,
    pub message: String,
}

pub(crate) fn finding(rule: &'static str, message: impl Into<String>) -> Finding {
    Finding {
        rule,
        message: message.into(),
    }
}

pub(crate) type Outcome<T> = std::result::Result<T, Finding>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParamUnit {
    Mm,
    Deg,
    Count,
}

impl ParamUnit {
    fn solidcraft(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Deg => "deg",
            Self::Count => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Param {
    pub name: String,
    pub expr: String,
    pub unit: ParamUnit,
}

/// How a seed pad's copper is drawn in the sketch. Template curves are
/// role-owned (design §2.1); identity is the anchor point, never a curve.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Template {
    /// Four lines through four corner points, in order; `centre` is the anchor.
    Rect {
        corners: [String; 4],
        sides: [String; 4],
    },
    /// A circle whose centre is the anchor.
    Circle { curve: String },
}

impl Template {
    fn copper_curves(&self) -> Vec<&str> {
        match self {
            Self::Rect { sides, .. } => sides.iter().map(String::as_str).collect(),
            Self::Circle { curve } => vec![curve.as_str()],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PadRole {
    /// Sketch point id of the pad centre: the identity carrier.
    pub anchor: String,
    pub template: Template,
    pub number: String,
}

/// Rigid repetition of a solved seed, outside the solver. Expressions are
/// evaluated against the footprint parameters.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Layout {
    /// Two rows along −Y from the seed; row 1 is row 0 turned 180° about the
    /// array centre (`row_span` apart). Counter-clockwise numbering.
    DualRow {
        per_row: String,
        pitch: String,
        row_span: String,
    },
    /// Four sides; side `k` is side 0 turned `k`·90° about the array centre
    /// (`span` between opposite rows). Counter-clockwise numbering.
    Quad {
        per_side: String,
        pitch: String,
        span: String,
    },
    /// Rows along −Y, columns along +X from the seed (row `i`, column `j`).
    Grid {
        rows: String,
        cols: String,
        pitch: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Skip {
    pub explicit: Vec<[u16; 2]>,
    /// A centred block of `[rows, cols]` removed from a grid.
    pub centre_void: Option<[u16; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Numbering {
    Sequential,
    /// Row letters without I, O, Q, S, X and Z (a JESD95-1 concept), column numbers.
    GridAlpha,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PadArray {
    pub id: String,
    /// Anchor of the seed pad role; instance (0, 0) is the seed itself.
    pub seed: String,
    pub layout: Layout,
    pub skip: Skip,
    pub numbering: Numbering,
}

/// What the spike evaluates: an embedded Solidcraft sketch (canonical JSON)
/// plus Pcbcraft-level roles, parameters and arrays.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Source {
    pub evaluator_rev: String,
    pub sketch_json: String,
    pub params: Vec<Param>,
    pub pads: Vec<PadRole>,
    pub arrays: Vec<PadArray>,
}

impl Source {
    /// Replace a parameter's expression (the edit a parameter table makes).
    pub(crate) fn set_param(&mut self, name: &str, expr: &str) -> Outcome<()> {
        let param = self
            .params
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| finding("footprint.param_unknown", format!("no parameter `{name}`")))?;
        param.expr = expr.to_string();
        Ok(())
    }
}

/// Stable pad identity (design §2.1): a sketch point id, or an array
/// generation key. Never geometry, profile order or pad number.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub(crate) enum PadKey {
    Anchor(String),
    Array { array: String, i: u16, j: u16 },
}

/// Quantized pad shape (lengths in nanometres, 1e-6 mm).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum PadShape {
    Rect { w_nm: i64, h_nm: i64 },
    Circle { d_nm: i64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PadOut {
    pub key: PadKey,
    pub number: String,
    pub centre_nm: [i64; 2],
    /// Counter-clockwise rotation in micro-degrees, in [0, 360°).
    pub rotation_udeg: i64,
    pub shape: PadShape,
}

#[derive(Clone, Debug)]
pub(crate) struct Evaluation {
    pub pads: Vec<PadOut>,
    pub dof: usize,
    /// The sketch with its new solved positions (what the product stores back).
    pub sketch_out: Sketch,
    /// FNV-1a of the quantized pads (what `library.publish` compares).
    pub hash: u64,
    /// FNV-1a of the raw f64 bits of every solved point: bit-level agreement.
    pub raw_hash: u64,
}

/// Evaluate a source with package values `pkg` (names without the prefix).
pub(crate) fn evaluate(source: &Source, pkg: &[(&str, f64)]) -> Outcome<Evaluation> {
    evaluate_with_clock(source, pkg, &mut |_| {})
}

/// Phases [`evaluate_with_clock`] reports, in order, as each one ends.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) const PHASES: [&str; 4] = ["check", "solve", "extract", "arrays"];

/// [`evaluate`], calling `clock` with each phase's name as it ends (for the
/// spike's measurements).
pub(crate) fn evaluate_with_clock(
    source: &Source,
    pkg: &[(&str, f64)],
    clock: &mut dyn FnMut(&'static str),
) -> Outcome<Evaluation> {
    if source.evaluator_rev != crate::SOLIDCRAFT_REVISION {
        return Err(finding(
            "footprint.evaluator_rev",
            format!(
                "source was made with Solidcraft {}, this build has {}",
                source.evaluator_rev,
                crate::SOLIDCRAFT_REVISION
            ),
        ));
    }
    let stored = check::check_sketch(&source.sketch_json)?;
    clock("check");
    let values = parameters(&source.params, pkg)?;
    let mut sketch = stored.clone();
    let old = stored.driving_values();
    apply_values(&mut sketch, &values)?;
    let reference: Vec<Vec2> = stored.points.iter().map(|p| p.pos).collect();
    let report =
        solve_after_edit_with_analysis(&mut sketch, &old, Some(&reference), Analysis::Full);
    if !report.ok() {
        return Err(finding(
            "footprint.solve_failed",
            report.failure(|c| format!("constraints {c} cannot be satisfied")),
        ));
    }
    if !report.folded.is_empty() {
        return Err(finding(
            "footprint.flipped",
            report.failure(|c| c.to_string()),
        ));
    }
    let flips = flips_from_stored(&sketch, &stored);
    if !flips.is_empty() {
        return Err(finding("footprint.flipped", flips.join(", ")));
    }
    clock("solve");
    let seeds = classify(&sketch, &source.pads)?;
    clock("extract");
    let mut pads = Vec::new();
    let mut in_array = std::collections::BTreeSet::new();
    if source.arrays.len() > MAX_ARRAYS {
        return Err(finding("footprint.too_large", "too many arrays"));
    }
    for array in &source.arrays {
        let seed = seeds.get(&array.seed).ok_or_else(|| {
            finding(
                "footprint.anchor_missing",
                format!("array {} has no seed pad `{}`", array.id, array.seed),
            )
        })?;
        if !in_array.insert(array.seed.clone()) {
            return Err(finding(
                "footprint.anchor_missing",
                format!("seed `{}` is used by two arrays", array.seed),
            ));
        }
        expand(array, seed, &values, &mut pads)?;
    }
    for role in &source.pads {
        if in_array.contains(&role.anchor) {
            continue;
        }
        if let Some(seed) = seeds.get(&role.anchor) {
            pads.push(seed.placed(
                PadKey::Anchor(role.anchor.clone()),
                role.number.clone(),
                Rigid::IDENTITY,
            )?);
        }
    }
    if pads.len() > MAX_PADS {
        return Err(finding(
            "footprint.too_large",
            format!("{} pads", pads.len()),
        ));
    }
    let hash =
        fnv(&serde_json::to_vec(&pads).map_err(|e| finding("footprint.internal", e.to_string()))?);
    let raw: Vec<u8> = sketch
        .points
        .iter()
        .flat_map(|p| {
            [
                p.pos.x.to_bits().to_le_bytes(),
                p.pos.y.to_bits().to_le_bytes(),
            ]
        })
        .flatten()
        .collect();
    clock("arrays");
    Ok(Evaluation {
        pads,
        dof: report.dof,
        raw_hash: fnv(&raw),
        hash,
        sketch_out: sketch,
    })
}

/// Footprint parameters plus injected `pkg_*` values, evaluated by
/// Solidcraft's expression language (values in mm, radians or plain numbers).
pub(crate) fn parameters(
    params: &[Param],
    pkg: &[(&str, f64)],
) -> Outcome<BTreeMap<String, Value>> {
    if params.len() > MAX_PARAMS {
        return Err(finding("footprint.too_large", "too many parameters"));
    }
    let mut defs = Vec::with_capacity(params.len() + pkg.len());
    for p in params {
        check_name(&p.name)?;
        if p.name.starts_with(PKG_PREFIX) {
            return Err(finding(
                "footprint.param_name",
                format!(
                    "`{}`: the `{PKG_PREFIX}` prefix is reserved for package values",
                    p.name
                ),
            ));
        }
        if p.expr.len() > 256 {
            return Err(finding(
                "footprint.param_expr",
                format!("`{}` is too long", p.name),
            ));
        }
        defs.push(ParamDef {
            name: p.name.clone(),
            expr: p.expr.clone(),
            unit: p.unit.solidcraft().into(),
        });
    }
    for (name, mm) in pkg {
        let name = format!("{PKG_PREFIX}{name}");
        check_name(&name)?;
        if !mm.is_finite() {
            return Err(finding(
                "footprint.param_expr",
                format!("`{name}` is not finite"),
            ));
        }
        // `{:?}` keeps every bit of the f64 (shortest round-trip form).
        defs.push(ParamDef {
            name,
            expr: format!("{mm:?} mm"),
            unit: "mm".into(),
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    for d in &defs {
        if !seen.insert(d.name.as_str()) {
            return Err(finding(
                "footprint.param_name",
                format!("`{}` is defined twice", d.name),
            ));
        }
    }
    let (values, errors) = expr::eval_params(&defs, 1.0);
    if let Some((name, error)) = errors.into_iter().next() {
        return Err(finding(
            "footprint.param_expr",
            format!("`{name}`: {error}"),
        ));
    }
    Ok(values)
}

fn check_name(name: &str) -> Outcome<()> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !first_ok || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') || name.len() > 64 {
        return Err(finding(
            "footprint.param_name",
            format!("`{name}` is not a parameter name"),
        ));
    }
    if let Some(kind) = expr::reserved_kind(name) {
        return Err(finding(
            "footprint.param_name",
            format!("`{name}` is a {kind} in expressions"),
        ));
    }
    Ok(())
}

fn value(values: &BTreeMap<String, Value>, text: &str, kind: Kind) -> Outcome<f64> {
    let lookup = |n: &str| {
        values.get(n).copied().ok_or_else(|| {
            solidcraft_engine::doc::DocError::Expr(format!("unknown parameter `{n}`"))
        })
    };
    expr::eval_with(text, &lookup)
        .and_then(|v| v.to_kind(kind))
        .map_err(|e| finding("footprint.param_expr", format!("`{text}`: {e}")))
}

/// Point expressions, then each bound dimension's value (mirrors what
/// Solidcraft's `Document::apply_point_values` / `apply_dimension_values` do
/// for a document, re-implemented on public APIs).
fn apply_values(sketch: &mut Sketch, values: &BTreeMap<String, Value>) -> Outcome<()> {
    for point in &mut sketch.points {
        let Some(exprs) = point.exprs.as_deref() else {
            continue;
        };
        let mut at = point.pos;
        if let Some(x) = &exprs.x {
            at.x = value(values, x, Kind::Length)?;
        }
        if let Some(y) = &exprs.y {
            at.y = value(values, y, Kind::Length)?;
        }
        if !(at.is_finite()
            && at.x.abs() <= check::MAX_COORD_MM
            && at.y.abs() <= check::MAX_COORD_MM)
        {
            return Err(finding(
                "footprint.too_large",
                format!("point {} is out of range", point.id),
            ));
        }
        point.pos = at;
        point.fixed = true;
    }
    for constraint in &mut sketch.constraints {
        let Some(name) = &constraint.param else {
            continue;
        };
        let kind = if constraint.kind.is_angle() {
            Kind::Angle
        } else {
            Kind::Length
        };
        let v = value(values, name, kind)?;
        if !v.is_finite() || v.abs() > check::MAX_COORD_MM {
            return Err(finding(
                "footprint.param_expr",
                format!("{} = {v} is out of range", constraint.id),
            ));
        }
        constraint.kind.set_value(v);
    }
    Ok(())
}

/// A rigid transform in the footprint plane: an exact quarter-turn rotation
/// about `centre`, then a translation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rigid {
    quarter_turns: u8,
    centre: Vec2,
    shift: Vec2,
}

impl Rigid {
    pub(crate) const IDENTITY: Rigid = Rigid {
        quarter_turns: 0,
        centre: Vec2 { x: 0.0, y: 0.0 },
        shift: Vec2 { x: 0.0, y: 0.0 },
    };

    fn apply(&self, p: Vec2) -> Vec2 {
        let d = Vec2::new(p.x - self.centre.x, p.y - self.centre.y);
        // Quarter turns by table, so copies are exact (no cos(90°) residue).
        let r = match self.quarter_turns % 4 {
            0 => d,
            1 => Vec2::new(-d.y, d.x),
            2 => Vec2::new(-d.x, -d.y),
            _ => Vec2::new(d.y, -d.x),
        };
        Vec2::new(
            r.x + self.centre.x + self.shift.x,
            r.y + self.centre.y + self.shift.y,
        )
    }
}

/// A solved, classified seed pad (unquantized).
#[derive(Clone, Debug)]
pub(crate) struct Seed {
    centre: Vec2,
    rotation_deg: f64,
    shape: SeedShape,
}

#[derive(Clone, Copy, Debug)]
enum SeedShape {
    Rect { w: f64, h: f64 },
    Circle { d: f64 },
}

impl Seed {
    fn placed(&self, key: PadKey, number: String, at: Rigid) -> Outcome<PadOut> {
        let c = at.apply(self.centre);
        let rotation = (self.rotation_deg + 90.0 * f64::from(at.quarter_turns)).rem_euclid(360.0);
        let shape = match self.shape {
            SeedShape::Rect { w, h } => PadShape::Rect {
                w_nm: quantize(w)?,
                h_nm: quantize(h)?,
            },
            SeedShape::Circle { d } => PadShape::Circle { d_nm: quantize(d)? },
        };
        let mut rotation_udeg = quantize_with(rotation, 1e-6)?;
        if rotation_udeg >= 360_000_000 {
            rotation_udeg -= 360_000_000;
        }
        Ok(PadOut {
            key,
            number,
            centre_nm: [quantize(c.x)?, quantize(c.y)?],
            rotation_udeg,
            shape,
        })
    }
}

/// Pcbcraft's own quantizer (Solidcraft's is private): round to 1e-6 mm.
pub(crate) fn quantize(mm: f64) -> Outcome<i64> {
    quantize_with(mm, QUANTUM_MM)
}

fn quantize_with(v: f64, quantum: f64) -> Outcome<i64> {
    let q = (v / quantum).round();
    if !q.is_finite() || q.abs() > 9.0e15 {
        return Err(finding(
            "footprint.too_large",
            format!("{v} cannot be quantized"),
        ));
    }
    // `as` saturates; the bound above keeps it exact (and −0 casts to 0).
    Ok(q as i64)
}

/// The copper-only profile pass (design §2.5 step 6): every curve that is not
/// a pad's role-owned copper is treated as construction, so silk, courtyard
/// and drill curves can neither split a pad nor capture its anchor.
pub(crate) fn copper_only(sketch: &Sketch, roles: &[PadRole]) -> Sketch {
    let copper: std::collections::BTreeSet<&str> = roles
        .iter()
        .flat_map(|r| r.template.copper_curves())
        .collect();
    let mut filtered = sketch.clone();
    for curve in &mut filtered.curves {
        if !copper.contains(curve.id.as_str()) {
            curve.construction = true;
        }
    }
    filtered
}

/// Solve-independent classification of every pad role on the solved sketch.
fn classify(sketch: &Sketch, roles: &[PadRole]) -> Outcome<BTreeMap<String, Seed>> {
    if roles.len() > MAX_PADS {
        return Err(finding("footprint.too_large", "too many pad roles"));
    }
    let profiles = find_profiles(&copper_only(sketch, roles));
    let mut seeds = BTreeMap::new();
    for role in roles {
        let anchor_index = sketch.point_index(&role.anchor).ok_or_else(|| {
            finding(
                "footprint.anchor_missing",
                format!("no anchor point `{}`", role.anchor),
            )
        })?;
        let anchor = sketch
            .points
            .get(anchor_index)
            .map(|p| p.pos)
            .ok_or_else(|| {
                finding(
                    "footprint.anchor_missing",
                    format!("no anchor point `{}`", role.anchor),
                )
            })?;
        let unclassified = |why: String| {
            finding(
                "footprint.pad_shape_unclassified",
                format!("{}: {why}", role.anchor),
            )
        };
        let profile = profile_at_point(&profiles, anchor)
            .and_then(|i| profiles.get(i))
            .ok_or_else(|| unclassified("no closed copper profile around the anchor".into()))?;
        let mut bounding: Vec<String> = profile.curve_ids();
        bounding.sort();
        bounding.dedup();
        let mut owned: Vec<String> = role
            .template
            .copper_curves()
            .into_iter()
            .map(str::to_string)
            .collect();
        owned.sort();
        if bounding != owned || !profile.hole_curves.is_empty() {
            return Err(unclassified(format!(
                "copper profile is bounded by {bounding:?}, not by the pad's own curves {owned:?}"
            )));
        }
        let point = |id: &str| {
            sketch
                .point_index(id)
                .and_then(|i| sketch.points.get(i))
                .map(|p| p.pos)
                .ok_or_else(|| unclassified(format!("missing point `{id}`")))
        };
        let seed = match &role.template {
            Template::Rect { corners, .. } => {
                let [a, b, c, d] = [
                    point(&corners[0])?,
                    point(&corners[1])?,
                    point(&corners[2])?,
                    point(&corners[3])?,
                ];
                let (w, h) = (a.dist(b), b.dist(c));
                let centre = Vec2::new((a.x + c.x) * 0.5, (a.y + c.y) * 0.5);
                let square = (w * w + h * h - a.dist(c).powi(2)).abs() < 1e-6
                    && (a.dist(c) - b.dist(d)).abs() < 1e-6;
                if !square
                    || centre.dist(anchor) > 1e-6
                    || (profile.area - w * h).abs() > 1e-6 * (1.0 + w * h)
                {
                    return Err(unclassified(
                        "the owned curves are not a rectangle centred on the anchor".into(),
                    ));
                }
                let dir = Vec2::new(b.x - a.x, b.y - a.y);
                Seed {
                    centre: anchor,
                    rotation_deg: dir.y.atan2(dir.x).to_degrees().rem_euclid(180.0),
                    shape: SeedShape::Rect { w, h },
                }
            }
            Template::Circle { curve } => {
                let c = sketch
                    .curve_index(curve)
                    .and_then(|i| sketch.curves.get(i))
                    .ok_or_else(|| unclassified(format!("missing curve `{curve}`")))?;
                let CurveKind::Circle { c: centre, r } = c.kind else {
                    return Err(unclassified(format!("`{curve}` is not a circle")));
                };
                let area = std::f64::consts::PI * r * r;
                if centre != anchor_index || (profile.area - area).abs() > 1e-3 * area {
                    return Err(unclassified(
                        "the circle is not centred on the anchor".into(),
                    ));
                }
                Seed {
                    centre: anchor,
                    rotation_deg: 0.0,
                    shape: SeedShape::Circle { d: 2.0 * r },
                }
            }
        };
        if seeds.insert(role.anchor.clone(), seed).is_some() {
            return Err(finding(
                "footprint.anchor_missing",
                format!("`{}` anchors two pads", role.anchor),
            ));
        }
    }
    Ok(seeds)
}

fn count(values: &BTreeMap<String, Value>, text: &str) -> Outcome<u16> {
    let n = value(values, text, Kind::Unitless)?;
    if n.fract() != 0.0 || !(1.0..=256.0).contains(&n) {
        return Err(finding(
            "footprint.param_expr",
            format!("count `{text}` = {n} is not a whole number in 1…256"),
        ));
    }
    Ok(n as u16)
}

fn length(values: &BTreeMap<String, Value>, text: &str) -> Outcome<f64> {
    let v = value(values, text, Kind::Length)?;
    if !(0.0..=check::MAX_COORD_MM).contains(&v) {
        return Err(finding(
            "footprint.param_expr",
            format!("`{text}` = {v} mm is out of range"),
        ));
    }
    Ok(v)
}

const GRID_LETTERS: &[u8] = b"ABCDEFGHJKLMNPRTUVWY";

fn row_name(mut i: usize) -> String {
    // A, B, … Y, AA, AB, … (bijective base-20 over the letters kept).
    let n = GRID_LETTERS.len();
    let mut out = Vec::new();
    loop {
        out.push(GRID_LETTERS.get(i % n).copied().unwrap_or(b'?'));
        if i < n {
            break;
        }
        i = i / n - 1;
    }
    out.reverse();
    String::from_utf8_lossy(&out).into_owned()
}

/// Rigid copies of a solved seed (design §2.5 step 8). Linear in the count.
fn expand(
    array: &PadArray,
    seed: &Seed,
    values: &BTreeMap<String, Value>,
    out: &mut Vec<PadOut>,
) -> Outcome<()> {
    // (i, j, transform, sequential index)
    let mut instances: Vec<(u16, u16, Rigid)> = Vec::new();
    match &array.layout {
        Layout::DualRow {
            per_row,
            pitch,
            row_span,
        } => {
            let (n, p, span) = (
                count(values, per_row)?,
                length(values, pitch)?,
                length(values, row_span)?,
            );
            let centre = Vec2::new(
                seed.centre.x + span * 0.5,
                seed.centre.y - f64::from(n - 1) * p * 0.5,
            );
            for row in 0..2u16 {
                for i in 0..n {
                    let down = Rigid {
                        quarter_turns: 0,
                        centre,
                        shift: Vec2::new(0.0, -f64::from(i) * p),
                    };
                    let turn = Rigid {
                        quarter_turns: (2 * row) as u8,
                        centre,
                        shift: Vec2::ZERO,
                    };
                    instances.push((row, i, compose(down, turn)));
                }
            }
        }
        Layout::Quad {
            per_side,
            pitch,
            span,
        } => {
            let (n, p, span) = (
                count(values, per_side)?,
                length(values, pitch)?,
                length(values, span)?,
            );
            let centre = Vec2::new(
                seed.centre.x + span * 0.5,
                seed.centre.y - f64::from(n - 1) * p * 0.5,
            );
            for side in 0..4u16 {
                for i in 0..n {
                    let down = Rigid {
                        quarter_turns: 0,
                        centre,
                        shift: Vec2::new(0.0, -f64::from(i) * p),
                    };
                    let turn = Rigid {
                        quarter_turns: side as u8,
                        centre,
                        shift: Vec2::ZERO,
                    };
                    instances.push((side, i, compose(down, turn)));
                }
            }
        }
        Layout::Grid { rows, cols, pitch } => {
            let (r, c, p) = (
                count(values, rows)?,
                count(values, cols)?,
                length(values, pitch)?,
            );
            for i in 0..r {
                for j in 0..c {
                    instances.push((
                        i,
                        j,
                        Rigid {
                            quarter_turns: 0,
                            centre: Vec2::ZERO,
                            shift: Vec2::new(f64::from(j) * p, -f64::from(i) * p),
                        },
                    ));
                }
            }
        }
    }
    let (rows, cols) = match &array.layout {
        Layout::Grid { rows, cols, .. } => (count(values, rows)?, count(values, cols)?),
        _ => (0, 0),
    };
    let void = array.skip.centre_void.map(|[h, w]| {
        let r0 = rows.saturating_sub(h) / 2;
        let c0 = cols.saturating_sub(w) / 2;
        (r0..r0.saturating_add(h), c0..c0.saturating_add(w))
    });
    if out.len().saturating_add(instances.len()) > MAX_PADS {
        return Err(finding(
            "footprint.too_large",
            format!("array {} makes too many pads", array.id),
        ));
    }
    for (seq, (i, j, at)) in instances.into_iter().enumerate() {
        if array.skip.explicit.contains(&[i, j])
            || void
                .as_ref()
                .is_some_and(|(r, c)| r.contains(&i) && c.contains(&j))
        {
            continue;
        }
        let number = match array.numbering {
            Numbering::Sequential => (seq + 1).to_string(),
            Numbering::GridAlpha => format!("{}{}", row_name(usize::from(i)), u32::from(j) + 1),
        };
        out.push(seed.placed(
            PadKey::Array {
                array: array.id.clone(),
                i,
                j,
            },
            number,
            at,
        )?);
    }
    Ok(())
}

/// `second ∘ first` for the two transforms the layouts use: a pure shift,
/// then a quarter-turn rotation about a centre.
fn compose(first: Rigid, second: Rigid) -> Rigid {
    // rotate(p + s - c) about c == rotate(p - c) + c + rotate(s)
    let s = first.shift;
    let rs = Rigid {
        quarter_turns: second.quarter_turns,
        centre: Vec2::ZERO,
        shift: Vec2::ZERO,
    }
    .apply(s);
    Rigid {
        quarter_turns: second.quarter_turns,
        centre: second.centre,
        shift: Vec2::new(rs.x + second.shift.x, rs.y + second.shift.y),
    }
}

/// 64-bit FNV-1a (no new dependency; enough to compare results).
pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
