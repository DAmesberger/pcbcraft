//! Lean parts (docs/part-templates.md §11): a package's 3D body is an
//! ordinary parametric Solidcraft design, a **part file**, built at the
//! package's literal parameter values through Solidcraft's engine.
//!
//! Pcbcraft reads what it needs by convention over generic Solidcraft
//! features, and checks the conventions itself ([`evaluate`]'s findings):
//! the design origin is the footprint origin and z = 0 the seating plane;
//! each body's appearance is named after a Pcbcraft material
//! ([`material_of`]); a construction plane `marking-<name>` with the length
//! parameters `marking_<name>_u` and `marking_<name>_v` is a marking
//! surface; a construction point `pin1` marks pin 1.
//!
//! Package datums (slice B2, docs/architecture/board-interface.md) are
//! declared the same way ([`DATUM_CONVENTION`]): a construction plane
//! `opening` or `opening-<suffix>` is the datum `opening` or
//! `opening_<suffix>`, a rectangle in the plane sized by the lengths
//! `<datum>_u` (along the plane's x) and `<datum>_v`, its corners rounded
//! by `<datum>_r` when given, swept `<datum>_depth` along the plane's
//! normal (a volume) when given, with the clearance `<datum>_clearance`
//! when given; the normal points out of the part, along the mating plug's
//! way in. A construction axis `axis` or `axis-<suffix>` is the datum
//! `axis` or `axis_<suffix>`, its diameter `<datum>_d` when given. Every
//! declared datum is marked external: it was declared for the enclosure.
//!
//! [`insert`] adds an evaluated part's history to a board design as one
//! component definition, parameters prefixed, so the saved board stays
//! parametric. It uses only the pinned fork's public API (the spike's typed
//! insert, `tests/template_spike.rs`).
use super::{GeneratedBody, ModelDefinition, PARAMETER_PREFIX, look};
use crate::geom::prelude::*;
use crate::native::Native;
use crate::{Error, Result};
use pcbcraft_core::library::BodyRole;
use pcbcraft_core::library::bundle::{MAX_ASSET_BYTES, asset_hash};
use pcbcraft_core::library::datum::{
    DatumFrame, DatumName, DatumProfile, MAX_PACKAGE_DATUMS, PART_FILE_AXIS, PART_FILE_OPENING,
    PackageDatum, PackageDatumGeometry, PackageDatumOrigin, PackageDatumParams,
};
use pcbcraft_core::library::marking::surface::{AnchorInfo, SurfaceGeometry};
use pcbcraft_core::library::model3d::MarkingArea;
use pcbcraft_core::library::package_model::{
    MAX_PART_BODIES, MAX_PART_SURFACES, MAX_PART_VALUES, PartDerived, PartValue,
    part_parameter_name_ok,
};
use pcbcraft_core::materials::MaterialId;
use pcbcraft_core::units::Nm;
use serde::{Deserialize, Serialize};
use solidcraft_engine::Session;
use solidcraft_engine::doc::construct::ConstructGeom;
use solidcraft_engine::doc::{Document, FeatureKind, ModelState, clone_expression};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

/// Literal values by the part's own parameter names.
pub type PartValues = BTreeMap<String, PartValue>;

/// The prefix of a marking-surface construction plane.
pub const MARKING_PLANE: &str = "marking-";
/// The default marking area's plane.
pub const DEFAULT_MARKING: &str = "marking-top";
/// The pin-1 construction point.
pub const PIN1_POINT: &str = "pin1";
/// The datum convention in one line, for the check's description.
pub const DATUM_CONVENTION: &str = "package datums from opening-<name> construction planes \
    (sized by <datum>_u and <datum>_v, optional <datum>_r, <datum>_depth and \
    <datum>_clearance; the normal points out along the plug's way in) and axis-<name> \
    construction axes (optional diameter <datum>_d)";
/// The lowest geometry lies this close to z = 0, mm.
pub const SEATING_TOLERANCE_MM: f64 = 0.01;
/// Cold evaluation above this is advisory `part_file.slow`, ms (a target).
pub const SLOW_MS: f64 = 100.0;
/// Evaluated parts the cache keeps (a target bound).
pub const CACHE_ENTRIES: usize = 32;
/// Failing features a finding names.
const NAMED_FAILURES: usize = 16;
/// Longest definition name (as for generated models).
const MAX_STEM: usize = 64;

/// One finding of the convention check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartFinding {
    pub code: String,
    pub blocking: bool,
    pub message: String,
}

/// A user parameter of the part as evaluated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartParameter {
    pub name: String,
    pub expression: String,
    /// `mm`, `deg` or empty.
    pub unit: String,
    /// The value in the parameter's own unit, when it evaluated.
    pub value: Option<f64>,
    /// Given by the request (a literal override).
    pub given: bool,
}

/// One body of the part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartBody {
    /// The body's name in the part file.
    pub name: String,
    /// The appearance name the file gives it.
    pub appearance: Option<String>,
    /// The Pcbcraft material that name maps to.
    pub material: Option<MaterialId>,
    pub bounds_mm: [[f64; 3]; 2],
}

/// A part file evaluated at some values: everything a package, the check
/// and a board build need, and the evaluated source the insert clones.
#[derive(Clone)]
pub struct EvaluatedPart {
    /// `PartKey`: SHA-256 of the asset hash and the canonical values.
    pub key: [u8; 32],
    pub parameters: Vec<PartParameter>,
    pub bodies: Vec<PartBody>,
    /// Marking surfaces, the default (`marking-top`) first.
    pub surfaces: Vec<AnchorInfo>,
    pub pin1: Option<[f64; 3]>,
    /// Other named construction points.
    pub points: Vec<(String, [f64; 3])>,
    /// Package datums the part declares, model frame.
    pub datums: Vec<PackageDatum>,
    pub bounds_mm: [[f64; 3]; 2],
    pub findings: Vec<PartFinding>,
    /// Cold evaluation time of this parameter set, ms.
    pub evaluate_ms: f64,
    /// The source design with the values applied.
    source: Arc<Document>,
    /// Its evaluated bodies, with their persistent face names: what
    /// dimension references to the part's faces are traced on
    /// (`crate::board::model_refs`).
    state: Arc<ModelState>,
}

impl std::fmt::Debug for EvaluatedPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvaluatedPart")
            .field("key", &hex(&self.key))
            .field("bodies", &self.bodies.len())
            .field("surfaces", &self.surfaces.len())
            .field("findings", &self.findings)
            .finish_non_exhaustive()
    }
}

/// The check report agents, the CLI and the UI read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartReport {
    pub key: String,
    pub parameters: Vec<PartParameter>,
    pub bodies: Vec<PartBody>,
    pub surfaces: Vec<AnchorInfo>,
    pub pin1: Option<[f64; 3]>,
    pub points: Vec<(String, [f64; 3])>,
    /// Package datums the part declares, model frame, millimetres.
    #[serde(default)]
    pub datums: Vec<PackageDatumParams>,
    pub bounds_mm: [[f64; 3]; 2],
    pub findings: Vec<PartFinding>,
    pub evaluate_ms: f64,
    /// The check passes: no blocking finding.
    pub ok: bool,
}

impl EvaluatedPart {
    /// Whether a finding blocks assignment and build.
    pub fn blocked(&self) -> bool {
        self.findings.iter().any(|f| f.blocking)
    }

    /// The report of this evaluation.
    pub fn report(&self) -> PartReport {
        PartReport {
            key: hex(&self.key),
            parameters: self.parameters.clone(),
            bodies: self.bodies.clone(),
            surfaces: self.surfaces.clone(),
            pin1: self.pin1,
            points: self.points.clone(),
            datums: self.datums.iter().cloned().map(Into::into).collect(),
            bounds_mm: self.bounds_mm,
            findings: self.findings.clone(),
            evaluate_ms: self.evaluate_ms,
            ok: !self.blocked(),
        }
    }

    /// The board definition name of this parameter set:
    /// `pkgp<first 12 hex of the key>`.
    pub fn stem(&self) -> String {
        format!("{PARAMETER_PREFIX}p{}", &hex(&self.key)[..12])
    }

    /// The evaluated bodies of the part, in the model frame.
    pub fn state(&self) -> &ModelState {
        &self.state
    }

    /// The first blocking finding as a refusal.
    pub fn refusal(&self) -> Option<Error> {
        self.findings
            .iter()
            .find(|f| f.blocking)
            .map(|f| Error::Invalid(format!("{}: {}", f.code, f.message)))
    }
}

/// What a package stores of an evaluation (rechecked by every build): the
/// bounds through the one crossing into stored nanometres.
pub fn derived(part: &EvaluatedPart) -> Result<PartDerived> {
    Ok(PartDerived {
        bounds_nm: pcbcraft_core::library::model3d::bounds_nm(part.bounds_mm)
            .map_err(|e| Error::Invalid(format!("part_file.bounds: {e}")))?,
        bodies: u32::try_from(part.bodies.len()).unwrap_or(u32::MAX),
        surfaces: part.surfaces.clone(),
        datums: part.datums.clone(),
    })
}

/// Whether a build's evaluation still matches what a package stored:
/// body count equal, bounds and surfaces within the recheck tolerance.
pub fn matches(stored: &PartDerived, now: &PartDerived) -> bool {
    let tol = super::imported::RECHECK_TOLERANCE_MM;
    let close = |a: &[f64], b: &[f64]| a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol);
    let area = |s: &AnchorInfo| match s.geometry {
        SurfaceGeometry::Plane(a) => Some(a),
        SurfaceGeometry::Cylinder(_) => None,
    };
    // Datums compare in millimetres (directions unitless), every number
    // within the recheck tolerance.
    let datums = |d: &PartDerived| {
        d.datums
            .iter()
            .map(|d| serde_json::to_value(PackageDatumParams::from(d.clone())).ok())
            .collect::<Vec<_>>()
    };
    let (was, is) = (datums(stored), datums(now));
    stored.bodies == now.bodies
        && was.len() == is.len()
        && was
            .iter()
            .zip(&is)
            .all(|(a, b)| matches!((a, b), (Some(a), Some(b)) if same_json(a, b, tol)))
        && close(&stored.bounds().concat(), &now.bounds().concat())
        && stored.surfaces.len() == now.surfaces.len()
        && stored.surfaces.iter().zip(&now.surfaces).all(|(a, b)| {
            a.name == b.name
                && match (area(a), area(b)) {
                    (Some(x), Some(y)) => {
                        close(&x.origin(), &y.origin())
                            && close(&x.normal, &y.normal)
                            && close(&x.u_axis, &y.u_axis)
                            && close(&x.size(), &y.size())
                    }
                    _ => false,
                }
        })
}

/// Two JSON values equal but for numbers, which agree within `tol`.
fn same_json(a: &serde_json::Value, b: &serde_json::Value, tol: f64) -> bool {
    use serde_json::Value as V;
    match (a, b) {
        (V::Number(x), V::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= tol,
            _ => false,
        },
        (V::Array(x), V::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_json(x, y, tol))
        }
        (V::Object(x), V::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w, tol)))
        }
        _ => a == b,
    }
}

/// The Pcbcraft material an appearance name means: a [`MaterialId`] in
/// kebab case (`nickel-silver`), case and `-`, `_` or space not told
/// apart. This is the one place a part's looks map, so a future material
/// graph named the same way maps the same way.
pub fn material_of(appearance: &str) -> Option<MaterialId> {
    let trimmed = appearance.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return None;
    }
    let key: String = trimmed
        .chars()
        .map(|c| match c {
            '-' | ' ' => '_',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    serde_json::from_value(serde_json::Value::String(key)).ok()
}

/// A part-file body's role, from its material: a body plated or made in a
/// lead finish (gold, tin, solder, nickel, copper alloys, brass) is a
/// termination, drawn and listed as a lead; every other body (mould,
/// ceramic, board, a nickel-silver or aluminium can) is the body. Display
/// only: terminals and pads stay the package's.
pub fn role_of(material: Option<MaterialId>) -> BodyRole {
    use MaterialId as M;
    match material {
        Some(
            M::Gold
            | M::TinMatte
            | M::TinBright
            | M::SolderSac
            | M::Nickel
            | M::CopperAlloy
            | M::PhosphorBronze
            | M::Brass,
        ) => BodyRole::Termination,
        _ => BodyRole::Body,
    }
}

/// The appearance name a part file gives a material's bodies.
pub fn appearance_name(material: MaterialId) -> String {
    serde_json::to_value(material)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.replace('_', "-")))
        .unwrap_or_default()
}

/// The material table a part file's appearances name, for the check.
pub fn materials() -> Vec<(String, MaterialId)> {
    MaterialId::ALL
        .into_iter()
        .map(|m| (appearance_name(m), m))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `PartKey` of `bytes` at `values`.
pub fn key(bytes: &[u8], values: &PartValues) -> Result<[u8; 32]> {
    let mut input = asset_hash(bytes).0.to_vec();
    input.extend(serde_json::to_vec(values)?);
    Ok(asset_hash(&input).0)
}

/// The process-wide cache of evaluated parts, least recently used last.
struct Cache {
    entries: VecDeque<Arc<EvaluatedPart>>,
    hits: u64,
    evaluations: u64,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            entries: VecDeque::new(),
            hits: 0,
            evaluations: 0,
        })
    })
}

/// (cache hits, evaluations) so far, process-wide.
pub fn cache_counts() -> (u64, u64) {
    cache().lock().map_or((0, 0), |c| (c.hits, c.evaluations))
}

/// Evaluate `bytes` at `values`, from the cache when this key was
/// evaluated before. Refuses an unreadable or oversized file, a malformed
/// value and a value that names no user parameter or has the wrong unit;
/// every other problem is a finding of the result.
pub fn evaluate(bytes: &[u8], values: &PartValues) -> Result<Arc<EvaluatedPart>> {
    check_request(bytes, values)?;
    let key = key(bytes, values)?;
    if let Ok(mut c) = cache().lock()
        && let Some(at) = c.entries.iter().position(|e| e.key == key)
        && let Some(hit) = c.entries.remove(at)
    {
        c.hits += 1;
        c.entries.push_front(Arc::clone(&hit));
        return Ok(hit);
    }
    let part = Arc::new(evaluate_now(bytes, values, key)?);
    if let Ok(mut c) = cache().lock() {
        c.evaluations += 1;
        c.entries.push_front(Arc::clone(&part));
        c.entries.truncate(CACHE_ENTRIES);
    }
    Ok(part)
}

fn check_request(bytes: &[u8], values: &PartValues) -> Result<()> {
    if bytes.is_empty() {
        return Err(Error::Invalid(
            "part_file.read: the part file is empty".into(),
        ));
    }
    if bytes.len() > MAX_ASSET_BYTES {
        return Err(Error::Invalid(format!(
            "part_file.read: the part file has {} bytes; a part file has at most {MAX_ASSET_BYTES}",
            bytes.len()
        )));
    }
    if values.len() > MAX_PART_VALUES {
        return Err(Error::Invalid(format!(
            "part_file.value: at most {MAX_PART_VALUES} values"
        )));
    }
    for (name, value) in values {
        if !part_parameter_name_ok(name) {
            return Err(Error::Invalid(format!(
                "part_file.value: ‘{}’ is not a parameter name",
                name.chars().take(80).collect::<String>()
            )));
        }
        if !value.finite() {
            return Err(Error::Invalid(format!(
                "part_file.value: the value of {name} must be finite and within ±10000"
            )));
        }
    }
    Ok(())
}

fn finding(code: &str, blocking: bool, message: String) -> PartFinding {
    PartFinding {
        code: code.into(),
        blocking,
        message,
    }
}

fn arr(v: crate::geom::Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

fn evaluate_now(bytes: &[u8], values: &PartValues, key: [u8; 32]) -> Result<EvaluatedPart> {
    let mut doc = solidcraft_engine::io::read_design(bytes)
        .map_err(|e| Error::Invalid(format!("part_file.read: {e}")))?;
    // The values, as literal expressions of user parameters.
    for (name, value) in values {
        let Some(param) = doc.params.iter_mut().find(|p| &p.name == name && !p.model) else {
            return Err(Error::Invalid(format!(
                "part_file.value: the part file has no user parameter {name}"
            )));
        };
        if param.unit != value.unit() {
            return Err(Error::Invalid(format!(
                "part_file.value: {name} is in ‘{}’; the value is in ‘{}’",
                param.unit,
                value.unit()
            )));
        }
        param.expr = value.expression();
    }
    let clock = std::time::Instant::now();
    let session = Session::opened_validating(doc, None, crate::native::build_validation());
    let evaluate_ms = clock.elapsed().as_secs_f64() * 1e3;
    let doc = Arc::clone(&session.doc);
    let mut findings = Vec::new();
    // Structure.
    let nested = doc.features.iter().filter(|f| f.component != 0).count();
    if nested > 0 || !doc.occurrences.is_empty() || !doc.assembly.joints.is_empty() {
        findings.push(finding(
            "part_file.structure",
            true,
            format!(
                "a part file keeps its features in the root component, with no occurrences or joints \
                 ({nested} feature(s) in subcomponents, {} occurrence(s), {} joint(s))",
                doc.occurrences.len(),
                doc.assembly.joints.len()
            ),
        ));
    }
    if doc.features.iter().any(|f| {
        matches!(
            f.kind,
            FeatureKind::Import { .. } | FeatureKind::MeshImport { .. }
        )
    }) {
        findings.push(finding(
            "part_file.import",
            false,
            "the part file imports geometry; record its source licence on the model".into(),
        ));
    }
    let failed: Vec<String> = session
        .model
        .results
        .iter()
        .filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.name)))
        .collect();
    if !failed.is_empty() {
        findings.push(finding(
            "part_file.feature",
            true,
            format!(
                "{} feature(s) fail at these values: {}",
                failed.len(),
                failed
                    .iter()
                    .take(NAMED_FAILURES)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        ));
    }
    // Parameters.
    let (vals, _) = doc.param_values();
    let parameters: Vec<PartParameter> = doc
        .params
        .iter()
        .filter(|p| !p.model)
        .map(|p| PartParameter {
            name: p.name.clone(),
            expression: p.expr.clone(),
            unit: p.unit.clone(),
            value: doc.param_display_value(&vals, &p.name),
            given: values.contains_key(&p.name),
        })
        .collect();
    let length = |name: &str| {
        vals.get(name)
            .filter(|v| v.len == 1 && v.ang == 0)
            .map(|v| v.v)
    };
    // Bodies and materials.
    let state = crate::native::world_state(&session)?;
    let mut bodies = Vec::with_capacity(state.bodies.len());
    let mut union: Option<[[f64; 3]; 2]> = None;
    for body in &state.bodies {
        let b = body.outline_bounds();
        let bounds = [arr(b.min), arr(b.max)];
        union = Some(match union {
            None => bounds,
            Some([l, h]) => [
                [0, 1, 2].map(|i| l[i].min(bounds[0][i])),
                [0, 1, 2].map(|i| h[i].max(bounds[1][i])),
            ],
        });
        let appearance = doc.model_body_look(body).map(|l| l.name);
        bodies.push(PartBody {
            name: body.name.clone(),
            material: appearance.as_deref().and_then(material_of),
            appearance,
            bounds_mm: bounds,
        });
    }
    let bounds_mm = union.unwrap_or_default();
    let count = u32::try_from(bodies.len()).unwrap_or(u32::MAX);
    if bodies.is_empty() {
        findings.push(finding(
            "part_file.empty",
            true,
            "the part file has no bodies".into(),
        ));
    } else if count > MAX_PART_BODIES {
        findings.push(finding(
            "part_file.bodies",
            true,
            format!("the part file has {count} bodies; at most {MAX_PART_BODIES}"),
        ));
    } else {
        let low = bounds_mm[0][2];
        if low.abs() > SEATING_TOLERANCE_MM {
            findings.push(finding(
                "part_file.seating",
                true,
                format!(
                    "the lowest geometry is at z = {low:.3} mm; the seating plane is z = 0 (±{SEATING_TOLERANCE_MM} mm)"
                ),
            ));
        }
        let [lo, hi] = bounds_mm;
        if !(lo[0] <= 0.0 && hi[0] >= 0.0 && lo[1] <= 0.0 && hi[1] >= 0.0) {
            findings.push(finding(
                "part_file.origin",
                false,
                "the design origin (the footprint origin) lies outside the part's xy bounds".into(),
            ));
        }
    }
    let unmapped: Vec<String> = bodies
        .iter()
        .filter(|b| b.material.is_none())
        .map(|b| match &b.appearance {
            Some(a) => format!("{} (‘{a}’)", b.name),
            None => format!("{} (no appearance)", b.name),
        })
        .collect();
    if !unmapped.is_empty() {
        findings.push(finding(
            "part_file.material",
            true,
            format!(
                "{} body(ies) have no appearance named after a {} material: {}",
                unmapped.len(),
                pcbcraft_core::product::NAME,
                unmapped
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    // Construction geometry: marking planes, declared datums and named
    // points.
    let mut surfaces: Vec<AnchorInfo> = Vec::new();
    let mut pin1 = None;
    let mut points = Vec::new();
    let mut marking_problems = Vec::new();
    let mut datums: Vec<PackageDatum> = Vec::new();
    let mut datum_problems = Vec::new();
    let mut declare = |datum: std::result::Result<PackageDatum, String>| match datum {
        Ok(d) if datums.iter().any(|e| e.name == d.name) => {
            datum_problems.push(format!("two datums are named {}", d.name.0));
        }
        Ok(d) => datums.push(d),
        Err(problem) => datum_problems.push(problem),
    };
    for c in &state.construct {
        match &c.geom {
            ConstructGeom::Plane(plane) if declares(&c.name, PART_FILE_OPENING) => {
                declare(plane_datum(&c.name, plane, &length));
            }
            ConstructGeom::Axis { origin, dir } if declares(&c.name, PART_FILE_AXIS) => {
                declare(axis_datum(&c.name, *origin, *dir, &length));
            }
            ConstructGeom::Plane(plane) => {
                let Some(rest) = c.name.strip_prefix(MARKING_PLANE) else {
                    continue;
                };
                let stem = rest.replace('-', "_");
                if !part_parameter_name_ok(&stem) {
                    marking_problems.push(format!("‘{}’ is not a marking name", c.name));
                    continue;
                }
                let (u, v) = (format!("marking_{stem}_u"), format!("marking_{stem}_v"));
                let (Some(su), Some(sv)) = (length(&u), length(&v)) else {
                    marking_problems.push(format!(
                        "{} needs the length parameters {u} and {v}",
                        c.name
                    ));
                    continue;
                };
                if !(su.is_finite() && sv.is_finite() && su > 0.0 && sv > 0.0) {
                    marking_problems.push(format!("{} needs a positive size", c.name));
                    continue;
                }
                if surfaces.iter().any(|s| s.name == stem) {
                    marking_problems.push(format!("two marking planes are named {stem}"));
                    continue;
                }
                let (Some(ux), Some(n)) = (plane.x.normalized(), plane.normal().normalized())
                else {
                    marking_problems.push(format!("{} has no direction", c.name));
                    continue;
                };
                let Ok(area) = MarkingArea::from_mm(arr(plane.origin), arr(n), arr(ux), [su, sv])
                else {
                    marking_problems.push(format!("{} lies beyond ±2 m", c.name));
                    continue;
                };
                surfaces.push(AnchorInfo {
                    name: stem.clone(),
                    description: format!("the part file's marking plane {}", c.name),
                    geometry: SurfaceGeometry::Plane(area),
                });
            }
            ConstructGeom::Point(p) if c.name == PIN1_POINT => pin1 = Some(arr(*p)),
            ConstructGeom::Point(p) => points.push((c.name.clone(), arr(*p))),
            ConstructGeom::Axis { .. } => {}
        }
    }
    if surfaces.len() > MAX_PART_SURFACES {
        marking_problems.push(format!(
            "{} marking planes; at most {MAX_PART_SURFACES}",
            surfaces.len()
        ));
    }
    if datums.len() > MAX_PACKAGE_DATUMS {
        datum_problems.push(format!(
            "{} datums; a package declares at most {MAX_PACKAGE_DATUMS}",
            datums.len()
        ));
    }
    if !datum_problems.is_empty() {
        findings.push(finding("part_file.datum", true, datum_problems.join("; ")));
    }
    if !marking_problems.is_empty() {
        findings.push(finding(
            "part_file.marking",
            true,
            marking_problems.join("; "),
        ));
    } else if surfaces.is_empty() {
        findings.push(finding(
            "part_file.no_marking",
            false,
            "no marking plane (`marking-<name>`): markings use the envelope's top".into(),
        ));
    }
    // The default area first.
    let top = DEFAULT_MARKING[MARKING_PLANE.len()..].replace('-', "_");
    if let Some(at) = surfaces.iter().position(|s| s.name == top) {
        let s = surfaces.remove(at);
        surfaces.insert(0, s);
    }
    if evaluate_ms > SLOW_MS {
        findings.push(finding(
            "part_file.slow",
            false,
            format!("the cold evaluation took {evaluate_ms:.0} ms (target {SLOW_MS:.0} ms)"),
        ));
    }
    Ok(EvaluatedPart {
        key,
        parameters,
        bodies,
        surfaces,
        pin1,
        points,
        datums,
        bounds_mm,
        findings,
        evaluate_ms,
        source: doc,
        state,
    })
}

/// Whether a construction feature's name declares a datum of `prefix`:
/// the prefix alone or followed by `-`.
fn declares(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
}

/// The datum name a declaring feature gives, or why it gives none.
fn datum_name(feature: &str, prefix: &str) -> std::result::Result<DatumName, String> {
    DatumName::from_part_file(feature, prefix).ok_or_else(|| {
        format!(
            "‘{}’ does not make a datum name ({prefix} or {prefix}-<suffix> in lower case \
             letters, digits and -, at most {} bytes)",
            feature.chars().take(40).collect::<String>(),
            pcbcraft_core::library::datum::MAX_DATUM_NAME
        )
    })
}

fn stored(mm: f64, what: &str) -> std::result::Result<Nm, String> {
    Nm::from_mm_f64_rounded(mm).map_err(|e| format!("{what}: {e}"))
}

fn triple(mm: [f64; 3], what: &str) -> std::result::Result<[Nm; 3], String> {
    Ok([
        stored(mm[0], what)?,
        stored(mm[1], what)?,
        stored(mm[2], what)?,
    ])
}

/// An `opening-*` plane as a package datum: a rectangle `<datum>_u` ×
/// `<datum>_v` in the plane, rounded by `<datum>_r`, swept `<datum>_depth`
/// along the normal when given.
fn plane_datum(
    feature: &str,
    plane: &solidcore_record::Plane,
    length: &impl Fn(&str) -> Option<f64>,
) -> std::result::Result<PackageDatum, String> {
    let name = datum_name(feature, PART_FILE_OPENING)?;
    let n = &name.0;
    let (Some(u), Some(v)) = (length(&format!("{n}_u")), length(&format!("{n}_v"))) else {
        return Err(format!(
            "{feature} needs the length parameters {n}_u and {n}_v"
        ));
    };
    let (Some(x), Some(y)) = (plane.x.normalized(), plane.y.normalized()) else {
        return Err(format!("{feature} has no direction"));
    };
    let profile = DatumProfile::Rect {
        size_nm: [stored(u, feature)?, stored(v, feature)?],
        corner_radius_nm: stored(length(&format!("{n}_r")).unwrap_or(0.0), feature)?,
    };
    let frame = DatumFrame {
        origin_nm: triple(arr(plane.origin), feature)?,
        x: arr(x),
        y: arr(y),
    };
    let geometry = match length(&format!("{n}_depth")) {
        Some(depth) => PackageDatumGeometry::Volume {
            frame,
            profile,
            height_nm: stored(depth, feature)?,
        },
        None => PackageDatumGeometry::Profile { frame, profile },
    };
    declared(name, geometry, length)
}

/// An `axis-*` construction axis as a package datum, its diameter
/// `<datum>_d` when given.
fn axis_datum(
    feature: &str,
    origin: crate::geom::Vec3,
    dir: crate::geom::Vec3,
    length: &impl Fn(&str) -> Option<f64>,
) -> std::result::Result<PackageDatum, String> {
    let name = datum_name(feature, PART_FILE_AXIS)?;
    let Some(direction) = dir.normalized() else {
        return Err(format!("{feature} has no direction"));
    };
    let diameter_nm = match length(&format!("{}_d", name.0)) {
        Some(d) => Some(stored(d, feature)?),
        None => None,
    };
    let geometry = PackageDatumGeometry::Axis {
        point_nm: triple(arr(origin), feature)?,
        direction: arr(direction),
        diameter_nm,
    };
    declared(name, geometry, length)
}

/// A declared datum, external, with its `<datum>_clearance` when given,
/// checked on its own.
fn declared(
    name: DatumName,
    geometry: PackageDatumGeometry,
    length: &impl Fn(&str) -> Option<f64>,
) -> std::result::Result<PackageDatum, String> {
    let clearance_nm = match length(&format!("{}_clearance", name.0)) {
        Some(c) => Some(stored(c, &name.0)?),
        None => None,
    };
    let datum = PackageDatum {
        name,
        geometry,
        clearance_nm,
        external: true,
        origin: PackageDatumOrigin::PartFile,
    };
    match datum.problem() {
        Some(problem) => Err(problem),
        None => Ok(datum),
    }
}

fn valid_stem(stem: &str) -> bool {
    stem.len() <= MAX_STEM
        && stem.starts_with(PARAMETER_PREFIX)
        && stem.len() > PARAMETER_PREFIX.len()
        && stem.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Add `part`'s history to `native` as the component definition `stem`
/// ([`insert_design`]), the bodies' roles and materials from their looks.
/// Refuses a blocked part. Leaves the root component active.
pub fn insert(native: &mut Native, stem: &str, part: &EvaluatedPart) -> Result<ModelDefinition> {
    if let Some(refusal) = part.refusal() {
        return Err(refusal);
    }
    let bodies: Vec<GeneratedBody> = part
        .bodies
        .iter()
        .map(|b| GeneratedBody {
            name: b.name.clone(),
            role: role_of(b.material),
            material: b.material.unwrap_or(MaterialId::EpoxyMould),
            terminal: None,
        })
        .collect();
    insert_design(native, stem, &part.source, &bodies)
}

/// Add a standalone design's history to `native` as the component
/// definition `stem`: every root feature of `src` cloned with complete
/// name maps (feature ids, feature names and body names `<stem> <name>`,
/// every parameter, feature input and dimension `<stem>_<name>`), user
/// parameters copied with their (literal or renamed) expressions, one
/// evaluation, the looks of `bodies` (by their names in `src`) assigned
/// from their materials. A part file and a generated model's standalone
/// design (`super::named`) are inserted this way. Refuses a name the
/// design already uses and any inserted feature failing; it runs inside a
/// board build, which is discarded whole on a refusal. Leaves the root
/// component active.
pub fn insert_design(
    native: &mut Native,
    stem: &str,
    src: &Document,
    bodies: &[GeneratedBody],
) -> Result<ModelDefinition> {
    if !valid_stem(stem) {
        return Err(Error::Invalid(format!(
            "model.name: `{stem}` must start with `{PARAMETER_PREFIX}` and use at most \
             {MAX_STEM} ASCII letters, digits and underscores"
        )));
    }
    if native.find_component(stem).is_some() {
        return Err(Error::Invalid(format!(
            "model.name: the design already has a definition named {stem}"
        )));
    }
    let parameters: BTreeMap<String, String> = src
        .param_defs()
        .into_iter()
        .map(|d| {
            let renamed = format!("{stem}_{}", d.name);
            (d.name, renamed)
        })
        .collect();
    let body_name = |n: &str| format!("{stem} {n}");
    let mut renamed: BTreeMap<String, String> = bodies
        .iter()
        .map(|b| (b.name.clone(), body_name(&b.name)))
        .collect();
    for f in &src.features {
        for n in f.body_names.iter().chain(&f.participants) {
            renamed.entry(n.clone()).or_insert_with(|| body_name(n));
        }
    }
    let created = native.component(stem, false)?;
    let session = native.session_mut();
    let engine = |message: String| Error::Engine {
        command: "part_file.insert".into(),
        message,
    };
    {
        let doc = session.doc_mut();
        let taken: BTreeSet<String> = doc.param_defs().into_iter().map(|d| d.name).collect();
        if let Some(clash) = parameters.values().find(|n| taken.contains(*n)) {
            return Err(Error::Invalid(format!(
                "part_file.insert: the design already has a parameter {clash}"
            )));
        }
        let mut next = doc.next_id.max(1);
        let mut ids = BTreeMap::new();
        let mut names = BTreeMap::new();
        for f in &src.features {
            ids.insert(f.id, next);
            next = next
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("part_file.insert: feature ids ran out".into()))?;
            names.insert(f.name.clone(), format!("{stem} {}", f.name));
        }
        if let Some(clash) = names.values().find(|n| doc.find_feature(n).is_some()) {
            return Err(Error::Invalid(format!(
                "part_file.insert: the design already has a feature {clash}"
            )));
        }
        doc.next_id = next;
        for p in &src.params {
            let mut q = p.clone();
            q.name = parameters
                .get(&p.name)
                .cloned()
                .unwrap_or_else(|| format!("{stem}_{}", p.name));
            q.expr = clone_expression(&p.expr, &parameters);
            doc.params.push(q);
        }
        for f in &src.features {
            let mut g = f.clone();
            g.remap_clone(&ids, &names, &renamed, &parameters)
                .map_err(|e| engine(format!("{}: {e}", f.name)))?;
            g.id = ids.get(&f.id).copied().unwrap_or_default();
            g.name = names.get(&f.name).cloned().unwrap_or_default();
            g.component = created.component;
            doc.features.push(g);
        }
    }
    session.refresh();
    if let Some(error) = session
        .model
        .results
        .iter()
        .filter(|r| r.name.starts_with(&format!("{stem} ")))
        .find_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.name)))
    {
        return Err(Error::Verification(format!(
            "model.build: {stem} did not build in the board: {error}"
        )));
    }
    let state = session.model.state();
    let built: BTreeSet<String> = state
        .bodies
        .iter()
        .filter(|b| b.name.starts_with(&format!("{stem} ")))
        .map(|b| b.name.clone())
        .collect();
    let expected: BTreeSet<String> = bodies.iter().map(|b| body_name(&b.name)).collect();
    if built != expected {
        return Err(Error::Verification(format!(
            "model.build: {stem} built {} bodies in the board, expected {}",
            built.len(),
            expected.len()
        )));
    }
    let generated: Vec<GeneratedBody> = bodies
        .iter()
        .map(|b| GeneratedBody {
            name: body_name(&b.name),
            ..b.clone()
        })
        .collect();
    let mut by_material: BTreeMap<MaterialId, Vec<&str>> = BTreeMap::new();
    for b in &generated {
        by_material.entry(b.material).or_default().push(&b.name);
    }
    for (id, names) in by_material {
        let def = look::definition(id)?;
        native.look(&names, &look::saved(&def))?;
    }
    native.activate_root()?;
    Ok(ModelDefinition {
        component: stem.to_owned(),
        bodies: generated,
    })
}

/// Pcbcraft's built-in part files (`library/parts/`, original work,
/// MIT OR Apache-2.0), by name.
pub const BUILTIN: &[(&str, &[u8])] = &[
    (
        "esp32-s31-wroom-3",
        include_bytes!("../../../../library/parts/esp32-s31-wroom-3.solidcraft"),
    ),
    (
        "smd-nut",
        include_bytes!("../../../../library/parts/smd-nut.solidcraft"),
    ),
    (
        "round-spacer",
        include_bytes!("../../../../library/parts/round-spacer.solidcraft"),
    ),
    (
        "finned-heatsink",
        include_bytes!("../../../../library/parts/finned-heatsink.solidcraft"),
    ),
];

/// The bytes of the built-in part file `name`.
pub fn builtin(name: &str) -> Option<&'static [u8]> {
    BUILTIN.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

#[cfg(test)]
#[path = "part_file_tests.rs"]
mod tests;
