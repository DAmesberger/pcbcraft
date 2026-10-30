//! `check_sketch` (design §2.5 step 2): the only way an embedded sketch is
//! loaded. Solidcraft's `Sketch` has no `deny_unknown_fields`, defaults most
//! fields and has no public whole-sketch validator, so the spike bounds the
//! input, demands the canonical serde form (anything dropped or added on a
//! round trip is refused), enforces the footprint subset and validates every
//! curve and constraint by index.

use std::collections::BTreeSet;

use solidcraft_engine::sketch::{ConstraintKind, CurveKind, Sketch};

use super::{Outcome, finding};

pub(crate) const MAX_SKETCH_BYTES: usize = 1 << 20;
pub(crate) const MAX_POINTS: usize = 4096;
pub(crate) const MAX_CURVES: usize = 4096;
pub(crate) const MAX_CONSTRAINTS: usize = 8192;
/// Largest coordinate or dimension value, mm.
pub(crate) const MAX_COORD_MM: f64 = 1000.0;

const UNSUPPORTED: &str = "footprint.sketch_unsupported";
const TOO_LARGE: &str = "footprint.too_large";

/// The canonical JSON the product stores for a sketch.
pub(crate) fn canonical(sketch: &Sketch) -> Outcome<String> {
    serde_json::to_string(sketch).map_err(|e| finding("footprint.internal", e.to_string()))
}

pub(crate) fn check_sketch(json: &str) -> Outcome<Sketch> {
    if json.len() > MAX_SKETCH_BYTES {
        return Err(finding(
            TOO_LARGE,
            format!("sketch is {} bytes", json.len()),
        ));
    }
    let given: serde_json::Value =
        serde_json::from_str(json).map_err(|e| finding(UNSUPPORTED, format!("not JSON: {e}")))?;
    // Count before the typed parse allocates the whole structure.
    for (key, cap) in [
        ("points", MAX_POINTS),
        ("curves", MAX_CURVES),
        ("constraints", MAX_CONSTRAINTS),
    ] {
        let n = given
            .get(key)
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        if n > cap {
            return Err(finding(TOO_LARGE, format!("{n} {key} (at most {cap})")));
        }
    }
    let sketch: Sketch = serde_json::from_value(given.clone())
        .map_err(|e| finding(UNSUPPORTED, format!("not a Solidcraft sketch: {e}")))?;
    let again =
        serde_json::to_value(&sketch).map_err(|e| finding("footprint.internal", e.to_string()))?;
    if again != given {
        return Err(finding(
            UNSUPPORTED,
            format!(
                "the sketch does not round-trip unchanged ({})",
                first_difference(&given, &again, "")
            ),
        ));
    }
    subset(&sketch)?;
    ids(&sketch)?;
    geometry(&sketch)?;
    Ok(sketch)
}

/// The footprint subset: no links, offsets, wires, patterns or 3D.
fn subset(sketch: &Sketch) -> Outcome<()> {
    let refuse = |what: &str| {
        Err(finding(
            UNSUPPORTED,
            format!("footprint sketches cannot have {what}"),
        ))
    };
    if !sketch.links.is_empty()
        || sketch.points.iter().any(|p| p.link.is_some())
        || sketch.curves.iter().any(|c| c.link.is_some())
    {
        return refuse("links");
    }
    if !sketch.offsets.is_empty() || !sketch.freeform_offsets.is_empty() {
        return refuse("offsets");
    }
    if !sketch.wires.is_empty() {
        return refuse("wires");
    }
    if !sketch.patterns.is_empty() {
        return refuse("sketch patterns");
    }
    if sketch.view.three_d {
        return refuse("3D drawing");
    }
    for c in &sketch.curves {
        if !matches!(
            c.kind,
            CurveKind::Line { .. } | CurveKind::Circle { .. } | CurveKind::Arc { .. }
        ) {
            return refuse(&format!("free-form curves ({})", c.id));
        }
    }
    for k in &sketch.constraints {
        if matches!(
            k.kind,
            ConstraintKind::PatternPoint { .. }
                | ConstraintKind::PatternRadius { .. }
                | ConstraintKind::OffsetDistance { .. }
        ) {
            return refuse(&format!("pattern or offset relations ({})", k.id));
        }
    }
    Ok(())
}

fn ids(sketch: &Sketch) -> Outcome<()> {
    match sketch.points.first() {
        Some(o) if o.id == "origin" && o.fixed && o.pos.x == 0.0 && o.pos.y == 0.0 => {}
        _ => return Err(finding(UNSUPPORTED, "point 0 must be the fixed origin")),
    }
    let mut seen = BTreeSet::new();
    let all = sketch
        .points
        .iter()
        .map(|p| &p.id)
        .chain(sketch.curves.iter().map(|c| &c.id))
        .chain(sketch.constraints.iter().map(|c| &c.id));
    for id in all {
        if id.is_empty() || id.len() > 128 {
            return Err(finding(UNSUPPORTED, format!("bad id `{id}`")));
        }
        if !seen.insert(id.as_str()) {
            return Err(finding(UNSUPPORTED, format!("duplicate id `{id}`")));
        }
    }
    Ok(())
}

fn in_range(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD_MM
}

fn geometry(sketch: &Sketch) -> Outcome<()> {
    for p in &sketch.points {
        if !in_range(p.pos.x) || !in_range(p.pos.y) {
            return Err(finding(
                UNSUPPORTED,
                format!("point {} is out of range", p.id),
            ));
        }
    }
    let n = sketch.points.len();
    for c in &sketch.curves {
        let pts = c.kind.point_ids();
        if pts.iter().any(|&i| i >= n) {
            return Err(finding(
                UNSUPPORTED,
                format!("curve {} names a missing point", c.id),
            ));
        }
        let distinct: BTreeSet<_> = pts.iter().collect();
        if distinct.len() != pts.len() {
            return Err(finding(
                UNSUPPORTED,
                format!("curve {} repeats a point", c.id),
            ));
        }
        if let CurveKind::Circle { r, .. } = c.kind
            && !(r > 1e-9 && in_range(r))
        {
            return Err(finding(
                UNSUPPORTED,
                format!("curve {} has radius {r}", c.id),
            ));
        }
    }
    for k in &sketch.constraints {
        sketch
            .validate(&k.kind)
            .map_err(|e| finding(UNSUPPORTED, format!("constraint {}: {e}", k.id)))?;
        if let Some(v) = k.kind.value()
            && !in_range(v)
        {
            return Err(finding(
                UNSUPPORTED,
                format!("constraint {} has value {v}", k.id),
            ));
        }
        if k.param.is_some() && k.driven {
            return Err(finding(
                UNSUPPORTED,
                format!("constraint {} is driven but bound to a parameter", k.id),
            ));
        }
    }
    Ok(())
}

/// Path of the first difference between two JSON values (for the message).
fn first_difference(a: &serde_json::Value, b: &serde_json::Value, at: &str) -> String {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                match (x.get(k), y.get(k)) {
                    (Some(p), Some(q)) if p == q => {}
                    (Some(p), Some(q)) => return first_difference(p, q, &format!("{at}/{k}")),
                    (Some(_), None) => return format!("`{at}/{k}` would be dropped"),
                    (None, _) => return format!("`{at}/{k}` would be added"),
                }
            }
            at.to_string()
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                if p != q {
                    return first_difference(p, q, &format!("{at}/{i}"));
                }
            }
            at.to_string()
        }
        _ => format!("`{at}` would change"),
    }
}
