//! The evaluation pipeline (design §2.5): revision check → `check_sketch`
//! → parameters with typed `pkg_*` values → point expressions and bound
//! dimensions → solve → copper-only profile pass → classification, holes,
//! mask/paste and vias → arrays and overrides → derived graphics → datum
//! → quantization to 1e-6 mm and the result hash.

use crate::geom::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

use crate::geom::Vec2;
use pcbcraft_core::diagnostics::{Finding, ObjectRef, Stage};
use pcbcraft_core::geometry::{Point, Shape};
use pcbcraft_core::ids::PadId;
use pcbcraft_core::library::footprint_source::{
    DerivedGraphic, EvaluatedPad, Evaluation, EvaluationRequest, GenerationKey, GraphicRole,
    GraphicSource, MaskRuleExpr, PadGeometry, PadOverride, PadRole, PadShapeExpr, PadTemplate,
    PasteRuleExpr, SketchBlob, SolveStatus, SolveSummary,
};
use pcbcraft_core::library::graphics::{FootprintGraphic, Item};
use pcbcraft_core::library::{
    ApertureId, CornerMask, Datum, FootprintDefinition, Hole, HoleShape, MAX_PADS, MaskRule, Pad,
    PadShape, PadVia, PasteRule, PasteWindow, Segment,
};
use pcbcraft_core::library::{MAX_PAD_VIAS, MAX_PASTE_WINDOWS};
use serde::Serialize;
use solidcraft_engine::sketch::{
    Analysis, CurveKind, Sketch, find_profiles, flips_from_stored, profile_at_point,
    solve_after_edit_with_analysis,
};

use super::check::{self, MAX_COORD_MM};
use super::classify::{self, UNCLASSIFIED};
use super::params::{self, Values};
use super::{Outcome, Problem, arrays, evaluator_ref, problem};

/// Evaluate a source. Design problems become blocking findings (the engine
/// refuses to commit them); the stored source is never changed here.
pub(crate) fn run(request: EvaluationRequest<'_>) -> Evaluation {
    match pipeline(request) {
        Ok(evaluation) => evaluation,
        Err(p) => Evaluation {
            summary: SolveSummary {
                evaluator: evaluator_ref(),
                status: SolveStatus::Failed,
                ..SolveSummary::default()
            },
            sketch_out: request.source.sketch.clone(),
            findings: vec![p.blocking()],
            ..Evaluation::default()
        },
    }
}

impl Problem {
    pub(crate) fn blocking(self) -> Finding {
        Finding::blocking(self.code, self.message, Vec::new(), Stage::Publish)
    }
}

/// Point expressions, then each bound dimension's value (what Solidcraft's
/// `Document::apply_point_values` / `apply_dimension_values` do for a
/// document, on public APIs).
pub(crate) fn apply_values(sketch: &mut Sketch, values: &Values) -> Outcome<()> {
    for point in &mut sketch.points {
        let Some(exprs) = point.exprs.as_deref() else {
            continue;
        };
        let mut at = point.pos;
        if let Some(x) = &exprs.x {
            at.x = values.signed_length(x)?;
        }
        if let Some(y) = &exprs.y {
            at.y = values.signed_length(y)?;
        }
        point.pos = at;
        point.fixed = true;
    }
    for constraint in &mut sketch.constraints {
        let Some(text) = &constraint.param else {
            continue;
        };
        let v = values.dimension(text, constraint.kind.is_angle())?;
        constraint.kind.set_value(v);
    }
    Ok(())
}

/// The solved sketch, its degrees of freedom and the stored sketch.
pub(crate) fn solve(stored: &Sketch, values: &Values) -> Outcome<(Sketch, usize)> {
    let mut sketch = stored.clone();
    let old = stored.driving_values();
    apply_values(&mut sketch, values)?;
    let reference: Vec<Vec2> = stored.points.iter().map(|p| p.pos).collect();
    let report =
        solve_after_edit_with_analysis(&mut sketch, &old, Some(&reference), Analysis::Full);
    if !report.ok() {
        let why = if report.folded.is_empty() {
            report.failure(|_| format!("{} cannot be satisfied", conflict(stored, &report.failing)))
        } else {
            flipped(stored, &report.folded)
        };
        return Err(problem("footprint.solve_failed", why));
    }
    if !report.folded.is_empty() {
        return Err(problem(
            "footprint.flipped",
            flipped(stored, &report.folded),
        ));
    }
    let flips = flips_from_stored(&sketch, stored);
    if !flips.is_empty() {
        return Err(problem("footprint.flipped", flipped(stored, &flips)));
    }
    Ok((sketch, report.dof))
}

/// Why a solve that turns the sketch over (`ids`, the constraints whose
/// orientation would change) is refused, naming the dimensions among them
/// by their values, never by id.
fn flipped(sketch: &Sketch, ids: &[String]) -> String {
    let (named, _) = named_constraints(sketch, ids);
    let what = if named.is_empty() {
        "the new values".to_owned()
    } else {
        named
    };
    format!("{what} would turn the shape inside out; choose a value that keeps it as drawn")
}

/// The violated constraints `failing` (sketch ids) as a person names
/// them: the dimensions by the value that drives them ("the dimension
/// ‘width’"), the rest counted, never by id.
fn conflict(sketch: &Sketch, failing: &[String]) -> String {
    let (list, others) = named_constraints(sketch, failing);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    match (list.is_empty(), others) {
        (true, n) => format!("{n} sketch constraint{} together", plural(n)),
        (false, 0) => list,
        (false, n) => format!("{list} with {n} other constraint{}", plural(n)),
    }
}

/// The dimensions among `ids` (sketch constraint ids) by the values that
/// drive them ("the dimension ‘width’"; empty when none is), and how many
/// other constraints there are.
fn named_constraints(sketch: &Sketch, ids: &[String]) -> (String, usize) {
    let mut named: Vec<&str> = Vec::new();
    let mut others = 0usize;
    for id in ids {
        match sketch
            .constraints
            .iter()
            .find(|c| c.id == *id)
            .and_then(|c| c.param.as_deref())
        {
            Some(param) if !named.contains(&param) => named.push(param),
            Some(_) => {}
            None => others += 1,
        }
    }
    let quoted: Vec<String> = named.iter().take(3).map(|n| format!("‘{n}’")).collect();
    let rest = named.len().saturating_sub(quoted.len());
    let list = match (quoted.as_slice(), rest) {
        ([], _) => String::new(),
        ([one], _) => format!("the dimension {one}"),
        (many, 0) => format!("the dimensions {}", many.join(", ")),
        (many, more) => format!("the dimensions {} and {more} more", many.join(", ")),
    };
    (list, others)
}

fn point_at(sk: &Sketch, id: &str, code: &'static str) -> Outcome<Vec2> {
    sk.point_index(id)
        .and_then(|i| sk.point(i))
        .ok_or_else(|| problem(code, format!("no sketch point `{id}`")))
}

fn curve_at(sk: &Sketch, id: &str, code: &'static str) -> Outcome<usize> {
    sk.curve_index(id)
        .ok_or_else(|| problem(code, format!("no sketch curve `{id}`")))
}

/// A line's start-to-end unit direction.
fn direction(sk: &Sketch, id: &str) -> Outcome<Vec2> {
    let c = curve_at(sk, id, "footprint.orientation")?;
    let Some(CurveKind::Line { a, b }) = sk.curves.get(c).map(|c| &c.kind) else {
        return Err(problem(
            "footprint.orientation",
            format!("`{id}` is not a line"),
        ));
    };
    let (Some(a), Some(b)) = (sk.point(*a), sk.point(*b)) else {
        return Err(problem(
            "footprint.orientation",
            "a line names a missing point",
        ));
    };
    (b - a)
        .normalized()
        .ok_or_else(|| problem("footprint.orientation", format!("`{id}` has no length")))
}

fn angle_of(sk: &Sketch, id: &str) -> Outcome<f64> {
    let d = direction(sk, id)?;
    Ok(d.y.atan2(d.x).to_degrees())
}

/// Curves that are never copper: graphics, holes, paste windows and the
/// direction and orientation lines roles use.
fn non_copper(request: &EvaluationRequest<'_>) -> BTreeSet<String> {
    let s = request.source;
    let mut out = BTreeSet::new();
    for g in &s.graphics {
        if let GraphicSource::Curves(ids) = &g.source {
            out.extend(ids.iter().map(|c| c.0.clone()));
        }
    }
    for role in &s.pads {
        if let Some(h) = &role.hole {
            out.extend(h.curves.iter().map(|c| c.0.clone()));
        }
        if let PasteRuleExpr::Windows(w) = &role.paste {
            out.extend(w.iter().map(|c| c.0.clone()));
        }
        out.extend(role.orientation.iter().map(|c| c.0.clone()));
    }
    for o in &s.overrides {
        if let Some(PasteRuleExpr::Windows(w)) = &o.value.paste {
            out.extend(w.iter().map(|c| c.0.clone()));
        }
    }
    for a in &s.arrays {
        match &a.layout {
            pcbcraft_core::library::footprint_source::ArrayLayout::Linear { direction, .. }
            | pcbcraft_core::library::footprint_source::ArrayLayout::DualRow {
                direction, ..
            } => {
                out.insert(direction.0.clone());
            }
            _ => {}
        }
    }
    if let Some(d) = &s.datum {
        out.extend(d.orientation.iter().map(|c| c.0.clone()));
    }
    out
}

/// The copper-only copy (design §2.5 step 6): every non-copper curve is
/// construction, so silk, courtyard and drill curves can neither split a
/// pad nor capture its anchor.
pub(crate) fn copper_only(sketch: &Sketch, non_copper: &BTreeSet<String>) -> Sketch {
    let mut filtered = sketch.clone();
    for curve in &mut filtered.curves {
        if curve.centerline || curve.infinite || non_copper.contains(&curve.id) {
            curve.construction = true;
        }
    }
    filtered
}

/// A classified role pad before arrays (unquantized).
struct Seed {
    centre: Vec2,
    pad: Pad,
}

fn template_matches(template: PadTemplate, shape: &PadShape) -> bool {
    matches!(
        (template, shape),
        (PadTemplate::Rect, PadShape::Rect { .. })
            | (PadTemplate::RoundRect, PadShape::RoundRect { .. })
            | (PadTemplate::Circle, PadShape::Circle { .. })
            | (PadTemplate::Oblong, PadShape::Oblong { .. })
    )
}

fn mask(rule: &MaskRuleExpr, values: &Values) -> Outcome<MaskRule> {
    Ok(match rule {
        MaskRuleExpr::Inherit => MaskRule::Inherit,
        MaskRuleExpr::Tented => MaskRule::Tented,
        MaskRuleExpr::Expansion(e) => MaskRule::Expansion(values.signed_length(&e.0)?),
    })
}

fn paste(
    rule: &PasteRuleExpr,
    values: &Values,
    sk: &Sketch,
    anchor: Vec2,
    rotation_deg: f64,
) -> Outcome<PasteRule> {
    Ok(match rule {
        PasteRuleExpr::Inherit => PasteRule::Inherit,
        PasteRuleExpr::None => PasteRule::None,
        PasteRuleExpr::Reduction(e) => PasteRule::Reduction(values.length(&e.0)?),
        PasteRuleExpr::Windows(curves) => {
            PasteRule::Windows(paste_windows(sk, curves, anchor, rotation_deg)?)
        }
    })
}

/// Paste windows: their own profile pass over the window curves only.
/// Each window's id is the position of its first listed curve.
fn paste_windows(
    sk: &Sketch,
    curves: &[pcbcraft_core::library::footprint_source::SketchCurveId],
    anchor: Vec2,
    rotation_deg: f64,
) -> Outcome<Vec<PasteWindow>> {
    const BAD: &str = "footprint.paste_window";
    let wanted: BTreeMap<&str, usize> = curves
        .iter()
        .enumerate()
        .map(|(k, c)| (c.0.as_str(), k))
        .collect();
    for c in curves {
        curve_at(sk, &c.0, BAD)?;
    }
    let mut only = sk.clone();
    for curve in &mut only.curves {
        curve.construction = !wanted.contains_key(curve.id.as_str());
    }
    let mut windows: Vec<(usize, PasteWindow)> = Vec::new();
    for profile in find_profiles(&only) {
        if !profile.hole_curves.is_empty() {
            return Err(problem(BAD, "a paste window cannot have holes"));
        }
        let first = profile
            .outer_curves
            .iter()
            .filter_map(|c| wanted.get(c.as_str()).copied())
            .min()
            .ok_or_else(|| problem(BAD, "a window without its curves"))?;
        let pieces = classify::pieces(&profile.region.outer)?;
        let c = classify::classify(&pieces, anchor, Some(rotation_deg))?;
        let shape = match c.shape {
            PadShape::Circle { diameter } => Shape::Circle {
                center: [0.0, 0.0],
                radius: diameter * 0.5,
            },
            PadShape::Rect { size } => Shape::Rect {
                a: [-size[0] * 0.5, -size[1] * 0.5],
                b: [size[0] * 0.5, size[1] * 0.5],
                radius: 0.0,
            },
            PadShape::Custom { outline } => {
                let mut points = vec![outline.start];
                for s in &outline.segments {
                    match s {
                        Segment::Line { to } => points.push(*to),
                        Segment::Arc { .. } => {
                            return Err(problem(BAD, "a paste window is a circle or a polygon"));
                        }
                    }
                }
                points.pop();
                Shape::Polygon {
                    points,
                    radii: Vec::new(),
                }
            }
            _ => return Err(problem(BAD, "a paste window is a circle or a polygon")),
        };
        let id = u32::try_from(first + 1).map_err(|_| problem(BAD, "too many windows"))?;
        windows.push((
            first,
            PasteWindow {
                id: ApertureId(id),
                shape,
            },
        ));
    }
    if windows.len() > MAX_PASTE_WINDOWS {
        return Err(problem(BAD, "too many paste windows"));
    }
    windows.sort_by_key(|(k, _)| *k);
    Ok(windows.into_iter().map(|(_, w)| w).collect())
}

fn vias(role: &PadRole, values: &Values) -> Outcome<Vec<PadVia>> {
    let Some(grid) = &role.vias else {
        return Ok(Vec::new());
    };
    let (rows, cols) = (values.count(&grid.rows.0)?, values.count(&grid.cols.0)?);
    let (pitch, drill) = (
        values.length(&grid.pitch.0)?,
        values.positive(&grid.drill.0)?,
    );
    if usize::from(rows) * usize::from(cols) > MAX_PAD_VIAS {
        return Err(problem("footprint.too_large", "too many pad vias"));
    }
    let mut out = Vec::new();
    for i in 0..rows {
        for j in 0..cols {
            let x = (f64::from(j) - f64::from(cols.saturating_sub(1)) * 0.5) * pitch;
            let y = (f64::from(rows.saturating_sub(1)) * 0.5 - f64::from(i)) * pitch;
            let id = u32::from(i) * u32::from(cols) + u32::from(j) + 1;
            out.push(PadVia {
                id: ApertureId(id),
                at: [x, y],
                drill,
            });
        }
    }
    Ok(out)
}

fn classify_roles(
    request: &EvaluationRequest<'_>,
    sketch: &Sketch,
    values: &Values,
    non_copper: &BTreeSet<String>,
) -> Outcome<BTreeMap<PadId, Seed>> {
    let roles = &request.source.pads;
    if roles.len() > MAX_PADS {
        return Err(problem("footprint.too_large", "too many pad roles"));
    }
    let copper = copper_only(sketch, non_copper);
    let profiles = find_profiles(&copper);
    let mut seeds = BTreeMap::new();
    let mut taken: BTreeMap<usize, PadId> = BTreeMap::new();
    for role in roles {
        let anchor = point_at(sketch, &role.anchor.0, "footprint.anchor_missing")?;
        let unclassified = |why: String| problem(UNCLASSIFIED, format!("pad {}: {why}", role.pad));
        let index = profile_at_point(&profiles, anchor)
            .ok_or_else(|| unclassified("no closed copper profile around the anchor".into()))?;
        let profile = profiles
            .get(index)
            .ok_or_else(|| unclassified("no closed copper profile around the anchor".into()))?;
        if let Some(other) = taken.insert(index, role.pad) {
            return Err(unclassified(format!(
                "it shares one copper profile with pad {other}"
            )));
        }
        if !profile.hole_curves.is_empty() {
            return Err(unclassified("its copper profile has holes".into()));
        }
        if let PadGeometry::Template { owned_curves, .. } = &role.geometry {
            let mut owned = Vec::new();
            for id in owned_curves {
                let c = curve_at(sketch, &id.0, "footprint.owned_curve")?;
                if copper.curves.get(c).is_some_and(|c| !c.construction) {
                    owned.push(id.0.clone());
                }
            }
            owned.sort();
            let mut bounding = profile.curve_ids();
            bounding.sort();
            bounding.dedup();
            if bounding != owned {
                return Err(unclassified(format!(
                    "its copper profile is bounded by {bounding:?}, not by the pad's own curves {owned:?}"
                )));
            }
        }
        let orientation = role
            .orientation
            .as_ref()
            .map(|o| angle_of(sketch, &o.0))
            .transpose()?;
        let pieces =
            classify::pieces(&profile.region.outer).map_err(|p| unclassified(p.message))?;
        let c = classify::classify(&pieces, anchor, orientation)?;
        if let PadGeometry::Template { template, .. } = &role.geometry
            && !template_matches(*template, &c.shape)
        {
            return Err(unclassified(format!(
                "the template's curves are no longer a {template:?}"
            )));
        }
        let hole = match &role.hole {
            Some(h) => {
                let curves = h
                    .curves
                    .iter()
                    .map(|c| curve_at(sketch, &c.0, "footprint.hole_unclassified"))
                    .collect::<Outcome<Vec<_>>>()?;
                Some(classify::hole(
                    sketch,
                    &curves,
                    h.plated,
                    anchor,
                    c.rotation_deg,
                )?)
            }
            None => None,
        };
        let pad = Pad {
            id: role.pad,
            kind: role.kind,
            terminal: role.terminal,
            number: role.number.clone(),
            position: [anchor.x, anchor.y],
            rotation_deg: c.rotation_deg,
            side: role.side,
            shape: c.shape,
            hole,
            mask: mask(&role.mask, values)?,
            paste: paste(&role.paste, values, sketch, anchor, c.rotation_deg)?,
            vias: vias(role, values)?,
        };
        if seeds
            .insert(
                role.pad,
                Seed {
                    centre: anchor,
                    pad,
                },
            )
            .is_some()
        {
            return Err(problem(
                "footprint.anchor_missing",
                format!("pad {} has two roles", role.pad),
            ));
        }
    }
    Ok(seeds)
}

fn override_shape(shape: &PadShapeExpr, values: &Values) -> Outcome<PadShape> {
    Ok(match shape {
        PadShapeExpr::Rect { width, height } => PadShape::Rect {
            size: [values.positive(&width.0)?, values.positive(&height.0)?],
        },
        PadShapeExpr::RoundRect {
            width,
            height,
            radius_ratio,
        } => PadShape::RoundRect {
            size: [values.positive(&width.0)?, values.positive(&height.0)?],
            radius_ratio: values.number(&radius_ratio.0)?,
            max_radius: None,
            corners: CornerMask::ALL,
        },
        PadShapeExpr::Circle { diameter } => PadShape::Circle {
            diameter: values.positive(&diameter.0)?,
        },
        PadShapeExpr::Oblong { width, height } => PadShape::Oblong {
            size: [values.positive(&width.0)?, values.positive(&height.0)?],
        },
    })
}

fn apply_override(pad: &mut Pad, o: &PadOverride, values: &Values, sk: &Sketch) -> Outcome<()> {
    if let Some(shape) = &o.shape {
        pad.shape = override_shape(shape, values)?;
    }
    if let Some(kind) = o.kind {
        pad.kind = kind;
    }
    if let Some(m) = &o.mask {
        pad.mask = mask(m, values)?;
    }
    if let Some(p) = &o.paste {
        let at = Vec2::new(pad.position[0], pad.position[1]);
        pad.paste = paste(p, values, sk, at, pad.rotation_deg)?;
    }
    Ok(())
}

/// Arrays, then the remaining role pads, in source order.
fn expand(
    request: &EvaluationRequest<'_>,
    sketch: &Sketch,
    values: &Values,
    seeds: &BTreeMap<PadId, Seed>,
) -> Outcome<(Vec<EvaluatedPad>, Vec<GenerationKey>)> {
    let source = request.source;
    let keys: BTreeMap<GenerationKey, PadId> = source.keys.iter().map(|k| (k.key, k.pad)).collect();
    let overrides: BTreeMap<GenerationKey, &PadOverride> =
        source.overrides.iter().map(|o| (o.key, &o.value)).collect();
    let mut used_overrides = BTreeSet::new();
    let mut out = Vec::new();
    let mut new_keys = Vec::new();
    let mut seeded = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for array in &source.arrays {
        let seed = seeds.get(&array.seed).ok_or_else(|| {
            problem(
                "footprint.anchor_missing",
                format!("array {} has no seed pad {}", array.id.0, array.seed),
            )
        })?;
        if !seeded.insert(array.seed) {
            return Err(problem(
                "footprint.array",
                format!("pad {} seeds two arrays", array.seed),
            ));
        }
        if !ids.insert(array.id) {
            return Err(problem(
                "footprint.array",
                format!("array {} is defined twice", array.id.0),
            ));
        }
        let direction = match &array.layout {
            pcbcraft_core::library::footprint_source::ArrayLayout::Linear { direction, .. }
            | pcbcraft_core::library::footprint_source::ArrayLayout::DualRow {
                direction, ..
            } => Some(super::evaluate::direction(sketch, &direction.0)?),
            _ => None,
        };
        let room = MAX_PADS.saturating_sub(out.len());
        for inst in arrays::expand(array, seed.centre, direction, values, room)? {
            let key = GenerationKey::Array {
                array: array.id,
                i: inst.i,
                j: inst.j,
            };
            let mut pad = seed.pad.clone();
            pad.position = [inst.centre.x, inst.centre.y];
            // The full instance turn first: overrides are authored against
            // it, and folding then carries the removed turn into the hole,
            // vias and paste windows so nothing lands on the wrong side.
            pad.rotation_deg += inst.turn_deg;
            pad.number = inst.number;
            if let Some(o) = overrides.get(&key) {
                apply_override(&mut pad, o, values, sketch)?;
                used_overrides.insert(key);
            }
            pad.fold_rotation();
            let keyed = if inst.i == 0 && inst.j == 0 {
                None
            } else {
                match keys.get(&key) {
                    Some(id) => {
                        pad.id = *id;
                        pad.terminal = request.previous.pad(*id).and_then(|p| p.terminal);
                    }
                    None => {
                        pad.id = PadId(0);
                        pad.terminal = None;
                        new_keys.push(key);
                    }
                }
                Some(key)
            };
            out.push(EvaluatedPad { key: keyed, pad });
        }
    }
    for role in &source.pads {
        if seeded.contains(&role.pad) {
            continue;
        }
        if let Some(seed) = seeds.get(&role.pad) {
            out.push(EvaluatedPad {
                key: None,
                pad: seed.pad.clone(),
            });
        }
    }
    if let Some(orphan) = overrides.keys().find(|k| !used_overrides.contains(*k)) {
        return Err(problem(
            "footprint.override_orphaned",
            format!("an override targets {orphan:?}, which the arrays no longer generate"),
        ));
    }
    if out.len() > MAX_PADS {
        return Err(problem(
            "footprint.too_large",
            format!("{} pads", out.len()),
        ));
    }
    Ok((out, new_keys))
}

fn arc_of(sk: &Sketch, c: usize, a: usize, b: usize) -> Outcome<Shape> {
    let (Some(pc), Some(pa), Some(pb)) = (sk.point(c), sk.point(a), sk.point(b)) else {
        return Err(problem("footprint.graphic", "an arc names a missing point"));
    };
    let start = (pa.y - pc.y).atan2(pa.x - pc.x).to_degrees();
    let end = (pb.y - pc.y).atan2(pb.x - pc.x).to_degrees();
    let mut sweep = (end - start).rem_euclid(360.0);
    if sweep <= 1e-9 {
        sweep = 360.0;
    }
    Ok(Shape::Arc {
        center: [pc.x, pc.y],
        radius: pc.dist(pa),
        start_deg: start.rem_euclid(360.0),
        sweep_deg: sweep,
    })
}

/// The shape of role-owned sketch curves: one line, circle or arc, or a
/// chain of lines (open polyline or closed polygon).
fn curves_shape(
    sk: &Sketch,
    ids: &[pcbcraft_core::library::footprint_source::SketchCurveId],
) -> Outcome<Shape> {
    const BAD: &str = "footprint.graphic";
    let pos = |i: usize| {
        sk.point(i)
            .map(|v| [v.x, v.y])
            .ok_or_else(|| problem(BAD, "a curve names a missing point"))
    };
    let kinds: Vec<&CurveKind> = ids
        .iter()
        .map(|id| {
            curve_at(sk, &id.0, BAD).and_then(|c| {
                sk.curves
                    .get(c)
                    .map(|c| &c.kind)
                    .ok_or_else(|| problem(BAD, "missing curve"))
            })
        })
        .collect::<Outcome<_>>()?;
    match kinds.as_slice() {
        [] => Err(problem(BAD, "a graphic role needs curves")),
        [CurveKind::Line { a, b }] => Ok(Shape::Line {
            a: pos(*a)?,
            b: pos(*b)?,
        }),
        [CurveKind::Circle { c, r }] => Ok(Shape::Circle {
            center: pos(*c)?,
            radius: *r,
        }),
        [CurveKind::Arc { c, a, b }] => arc_of(sk, *c, *a, *b),
        _ => {
            let mut edges = Vec::new();
            for k in &kinds {
                match k {
                    CurveKind::Line { a, b } => edges.push((*a, *b)),
                    _ => {
                        return Err(problem(
                            BAD,
                            "a graphic of several curves must be a chain of lines",
                        ));
                    }
                }
            }
            let chain =
                chain(&edges).ok_or_else(|| problem(BAD, "the lines do not form one chain"))?;
            let points = chain
                .0
                .iter()
                .map(|&p| pos(p))
                .collect::<Outcome<Vec<_>>>()?;
            Ok(if chain.1 {
                Shape::Polygon {
                    points,
                    radii: Vec::new(),
                }
            } else {
                Shape::Polyline {
                    points,
                    radii: Vec::new(),
                }
            })
        }
    }
}

/// Order edges into one path: its points and whether it closes.
fn chain(edges: &[(usize, usize)]) -> Option<(Vec<usize>, bool)> {
    let mut degree: BTreeMap<usize, usize> = BTreeMap::new();
    for &(a, b) in edges {
        *degree.entry(a).or_default() += 1;
        *degree.entry(b).or_default() += 1;
    }
    if degree.values().any(|&d| d > 2) {
        return None;
    }
    let ends: Vec<usize> = degree
        .iter()
        .filter(|(_, d)| **d == 1)
        .map(|(p, _)| *p)
        .collect();
    let closed = ends.is_empty();
    let start = if closed {
        edges.first()?.0
    } else if ends.len() == 2 {
        *ends.first()?
    } else {
        return None;
    };
    let mut used = vec![false; edges.len()];
    let mut path = vec![start];
    let mut at = start;
    for _ in 0..edges.len() {
        let (k, next) = edges.iter().enumerate().find_map(|(k, &(a, b))| {
            if used.get(k).copied().unwrap_or(true) {
                None
            } else if a == at {
                Some((k, b))
            } else if b == at {
                Some((k, a))
            } else {
                None
            }
        })?;
        if let Some(u) = used.get_mut(k) {
            *u = true;
        }
        path.push(next);
        at = next;
    }
    if closed {
        if path.last() != Some(&start) {
            return None;
        }
        path.pop();
    }
    Some((path, closed))
}

fn graphics(
    request: &EvaluationRequest<'_>,
    sketch: &Sketch,
    values: &Values,
    pads: &[EvaluatedPad],
    datum: Datum,
) -> Outcome<Vec<FootprintGraphic>> {
    let envelope = FootprintDefinition {
        datum,
        ..FootprintDefinition::default()
    }
    .envelope_outline(request.package);
    let mut out = Vec::new();
    for GraphicRole {
        id,
        layer,
        side,
        source,
        width,
        fill,
    } in &request.source.graphics
    {
        let shape = match source {
            GraphicSource::Curves(ids) => curves_shape(sketch, ids)?,
            GraphicSource::Derived(DerivedGraphic::AssemblyFromEnvelope) => Shape::Rect {
                a: envelope.min,
                b: envelope.max,
                radius: 0.0,
            },
            GraphicSource::Derived(DerivedGraphic::CourtyardFromExtents { excess, round }) => {
                let (excess, round) = (values.length(&excess.0)?, values.length(&round.0)?);
                let mut b = envelope;
                for p in pads {
                    let pb = p.pad.bounds();
                    b = b.including(pb.min).including(pb.max);
                }
                let grow = |v: f64, up: bool| {
                    let v = if up { v + excess } else { v - excess };
                    if round > 0.0 {
                        let k = v / round;
                        (if up { k.ceil() } else { k.floor() }) * round
                    } else {
                        v
                    }
                };
                Shape::Rect {
                    a: [grow(b.min[0], false), grow(b.min[1], false)],
                    b: [grow(b.max[0], true), grow(b.max[1], true)],
                    radius: 0.0,
                }
            }
        };
        out.push(FootprintGraphic {
            id: *id,
            layer: *layer,
            side: *side,
            item: Item::Shape {
                shape,
                width: values.length(&width.0)?,
                fill: *fill,
            },
        });
    }
    Ok(out)
}

fn datum(request: &EvaluationRequest<'_>, sketch: &Sketch) -> Outcome<Datum> {
    let Some(role) = &request.source.datum else {
        return Ok(Datum::default());
    };
    let at = point_at(sketch, &role.point.0, "footprint.datum")?;
    let rotation_deg = match &role.orientation {
        Some(o) => angle_of(sketch, &o.0)?.rem_euclid(360.0),
        None => 0.0,
    };
    Ok(Datum {
        offset: [at.x, at.y],
        rotation_deg,
    })
}

/// Pcbcraft's own quantizer (Solidcraft's is private): 1e-6 mm, and
/// micro-degrees for angles.
pub(crate) fn q(v: f64) -> Outcome<f64> {
    let k = (v * 1e6).round();
    if !k.is_finite() || k.abs() > 9.0e15 {
        return Err(problem(
            "footprint.too_large",
            format!("{v} cannot be quantized"),
        ));
    }
    // `+ 0.0` turns −0 into 0 so equal results hash equally.
    Ok(k / 1e6 + 0.0)
}

fn qp(p: Point) -> Outcome<Point> {
    Ok([q(p[0])?, q(p[1])?])
}

fn q_shape(shape: &mut Shape) -> Outcome<()> {
    match shape {
        Shape::Line { a, b } => {
            *a = qp(*a)?;
            *b = qp(*b)?;
        }
        Shape::Polyline { points, radii } | Shape::Polygon { points, radii } => {
            for p in points.iter_mut() {
                *p = qp(*p)?;
            }
            for r in radii.iter_mut() {
                *r = q(*r)?;
            }
        }
        Shape::Rect { a, b, radius } => {
            *a = qp(*a)?;
            *b = qp(*b)?;
            *radius = q(*radius)?;
        }
        Shape::Circle { center, radius } => {
            *center = qp(*center)?;
            *radius = q(*radius)?;
        }
        Shape::Arc {
            center,
            radius,
            start_deg,
            sweep_deg,
        } => {
            *center = qp(*center)?;
            *radius = q(*radius)?;
            *start_deg = q(*start_deg)?;
            *sweep_deg = q(*sweep_deg)?;
        }
    }
    Ok(())
}

fn q_pad(pad: &mut Pad) -> Outcome<()> {
    pad.position = qp(pad.position)?;
    // Rounding can land exactly on the period (180° or 360°); fold it back.
    // Fold before quantising the pad-frame features the fold may turn.
    pad.fold_rotation();
    pad.rotation_deg = q(pad.rotation_deg)?;
    pad.fold_rotation();
    pad.rotation_deg = q(pad.rotation_deg)?;
    match &mut pad.shape {
        PadShape::Rect { size } | PadShape::Oblong { size } => *size = qp(*size)?,
        PadShape::RoundRect {
            size,
            radius_ratio,
            max_radius,
            ..
        } => {
            *size = qp(*size)?;
            *radius_ratio = q(*radius_ratio)?;
            if let Some(m) = max_radius {
                *m = q(*m)?;
            }
        }
        PadShape::Circle { diameter } => *diameter = q(*diameter)?,
        PadShape::Custom { outline } => {
            outline.start = qp(outline.start)?;
            for s in &mut outline.segments {
                match s {
                    Segment::Line { to } => *to = qp(*to)?,
                    Segment::Arc { to, bulge } => {
                        *to = qp(*to)?;
                        *bulge = (*bulge * 1e9).round() / 1e9 + 0.0;
                    }
                }
            }
        }
    }
    if let Some(Hole { shape, offset, .. }) = &mut pad.hole {
        *offset = qp(*offset)?;
        match shape {
            HoleShape::Round { diameter } => *diameter = q(*diameter)?,
            HoleShape::Slot {
                width,
                length,
                angle_deg,
            } => {
                *width = q(*width)?;
                *length = q(*length)?;
                *angle_deg = q(*angle_deg)?;
            }
        }
    }
    if let MaskRule::Expansion(v) = &mut pad.mask {
        *v = q(*v)?;
    }
    match &mut pad.paste {
        PasteRule::Reduction(v) => *v = q(*v)?,
        PasteRule::Windows(w) => {
            for w in w.iter_mut() {
                q_shape(&mut w.shape)?;
            }
        }
        _ => {}
    }
    for v in &mut pad.vias {
        v.at = qp(v.at)?;
        v.drill = q(v.drill)?;
    }
    Ok(())
}

/// 64-bit FNV-1a (enough to compare results; no new dependency).
pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// What the result hash covers: geometry and numbering, keyed by stable
/// identity, never terminals (assigning terminals is not a re-solve).
#[derive(Serialize)]
struct Hashed<'a> {
    pads: Vec<(Option<GenerationKey>, Option<PadId>, Pad)>,
    graphics: &'a [FootprintGraphic],
    datum: &'a Datum,
}

pub(crate) fn result_hash(
    pads: &[EvaluatedPad],
    graphics: &[FootprintGraphic],
    datum: &Datum,
) -> Outcome<String> {
    let pads = pads
        .iter()
        .map(|p| {
            let mut pad = p.pad.clone();
            pad.terminal = None;
            let id = if p.key.is_none() { Some(pad.id) } else { None };
            pad.id = PadId(0);
            (p.key, id, pad)
        })
        .collect();
    let bytes = serde_json::to_vec(&Hashed {
        pads,
        graphics,
        datum,
    })
    .map_err(|e| problem("footprint.internal", e.to_string()))?;
    Ok(format!("{:016x}", fnv(&bytes)))
}

fn pipeline(request: EvaluationRequest<'_>) -> Outcome<Evaluation> {
    let source = request.source;
    if source.evaluator != evaluator_ref() {
        return Err(problem(
            "footprint.evaluator_rev",
            format!(
                "the source was made with {} {}, this build has {} {}",
                source.evaluator.engine,
                source.evaluator.rev,
                super::ENGINE,
                crate::SOLIDCRAFT_REVISION
            ),
        ));
    }
    let stored = check::check_sketch(&source.sketch.0)?;
    let values = params::evaluate(&source.parameters, request.package_values)?;
    let (sketch, dof) = solve(&stored, &values)?;
    let excluded = non_copper(&request);
    let seeds = classify_roles(&request, &sketch, &values, &excluded)?;
    let (mut pads, new_keys) = expand(&request, &sketch, &values, &seeds)?;
    for p in &mut pads {
        q_pad(&mut p.pad)?;
    }
    let mut datum = datum(&request, &sketch)?;
    datum.offset = qp(datum.offset)?;
    datum.rotation_deg = q(datum.rotation_deg)?;
    let mut graphics = graphics(&request, &sketch, &values, &pads, datum)?;
    for g in &mut graphics {
        if let Item::Shape { shape, width, .. } = &mut g.item {
            q_shape(shape)?;
            *width = q(*width)?;
        }
    }
    let result_hash = result_hash(&pads, &graphics, &datum)?;
    let dof = u32::try_from(dof).unwrap_or(u32::MAX);
    let mut findings = Vec::new();
    if dof > 0 {
        findings.push(Finding::new(
            "footprint.under_constrained",
            format!("The footprint sketch has {dof} remaining degrees of freedom."),
            Vec::new(),
        ));
    }
    if !new_keys.is_empty() {
        findings.push(Finding::new(
            "footprint.unresolved_key",
            format!(
                "{} new array pads have no terminal decision yet (assign terminals).",
                new_keys.len()
            ),
            Vec::<ObjectRef>::new(),
        ));
    }
    let sketch_out = check::canonical(&sketch)?;
    // Positions beyond the range would fail the next load; refuse now.
    if sketch
        .points
        .iter()
        .any(|p| p.pos.x.abs() > MAX_COORD_MM || p.pos.y.abs() > MAX_COORD_MM)
    {
        return Err(problem(
            "footprint.too_large",
            "the solved sketch leaves the coordinate range",
        ));
    }
    Ok(Evaluation {
        pads,
        graphics,
        datum,
        summary: SolveSummary {
            evaluator: evaluator_ref(),
            status: if dof > 0 {
                SolveStatus::UnderConstrained
            } else {
                SolveStatus::Solved
            },
            dof,
            result_hash,
        },
        sketch_out: SketchBlob(sketch_out),
        new_keys,
        findings,
    })
}

#[cfg(test)]
mod flip_words_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use solidcraft_engine::sketch::ConstraintKind;

    /// Second VM test drive, issue 6: Solidcraft reports a flip by
    /// constraint ids ("the new values would flip the sketch over (k15, k16
    /// would change orientation)"), which reached the Shape card's
    /// `corner_r` field as they were. The refusal names the dimension by
    /// the value that drives it and never an id.
    #[test]
    fn a_flip_names_the_dimension_never_the_constraint_ids() {
        let mut sk = Sketch::new();
        let a = sk.add_point(Vec2::new(0.0, 0.0), None).unwrap();
        let b = sk.add_point(Vec2::new(10.0, 0.0), None).unwrap();
        let c = sk.add_point(Vec2::new(5.0, 4.0), None).unwrap();
        let driven = sk
            .add_constraint(
                ConstraintKind::Distance {
                    p: a,
                    q: c,
                    value: 41f64.sqrt(),
                },
                Some("corner_r".into()),
            )
            .unwrap();
        let plain = sk
            .add_constraint(
                ConstraintKind::Distance {
                    p: b,
                    q: c,
                    value: 41f64.sqrt(),
                },
                None,
            )
            .unwrap();
        let words = flipped(&sk, &[driven.clone(), plain.clone()]);
        assert_eq!(
            words,
            "the dimension ‘corner_r’ would turn the shape inside out; choose a value that \
             keeps it as drawn"
        );
        assert!(
            !words.contains(&driven) && !words.contains(&plain),
            "{words}"
        );
        // With no dimension among them, the values are named as a whole.
        let words = flipped(&sk, std::slice::from_ref(&plain));
        assert!(words.starts_with("the new values would turn"), "{words}");
        assert!(!words.contains(&plain), "{words}");
    }
}
