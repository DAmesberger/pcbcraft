//! Controlled impedance from the stackup: published closed-form
//! approximations for microstrip, embedded microstrip and stripline and
//! their edge-coupled differential forms ([`formulas`]), the line
//! structure each copper layer forms with its reference planes
//! ([`structure`]), and the widths and gaps a net class's impedance
//! targets need on each layer ([`derive`]).
//!
//! The formulas are this module's own implementation of public equations
//! (sources and accuracy in docs/high-speed-routing.md). They are
//! quasi-static: no frequency dispersion, no solder mask, no etch
//! trapezoid and no copper roughness. Treat results as a starting point
//! and confirm critical stackups with the fabricator's field solver.
use crate::diagnostics::{Error, Result};
use crate::ids::Id;
use crate::model::{CopperLayer, Project, Stackup};
use crate::rules::{CopperObject, NetClassId, RuleKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Narrowest and widest track the solver considers, mm.
const SOLVE_RANGE: (f64, f64) = (0.005, 20.0);
/// Widest gap the solver considers, mm.
const MAX_GAP: f64 = 10.0;
/// The widest track routing accepts, mm (`layout.route`).
const MAX_ROUTE_WIDTH: f64 = 5.0;

/// The published closed-form equations, in mm and Ω. Every function
/// takes finite positive dimensions; callers check them.
pub mod formulas {
    use std::f64::consts::{E, PI};

    /// Impedance of free space, Ω.
    pub const ETA0: f64 = 376.730_313_668;

    /// Hammerstad and Jensen's zero-thickness microstrip impedance in air
    /// for width over height `u` (1980).
    fn z01(u: f64) -> f64 {
        let f = 6.0 + (2.0 * PI - 6.0) * (-(30.666 / u).powf(0.7528)).exp();
        ETA0 / (2.0 * PI) * (f / u + (1.0 + 4.0 / (u * u)).sqrt()).ln()
    }

    /// Hammerstad and Jensen's zero-thickness effective permittivity.
    fn eps_e(u: f64, er: f64) -> f64 {
        let a = 1.0
            + ((u.powi(4) + (u / 52.0).powi(2)) / (u.powi(4) + 0.432)).ln() / 49.0
            + (1.0 + (u / 18.1).powi(3)).ln() / 18.7;
        let b = 0.564 * ((er - 0.9) / (er + 3.0)).powf(0.053);
        (er + 1.0) / 2.0 + (er - 1.0) / 2.0 * (1.0 + 10.0 / u).powf(-a * b)
    }

    /// The width corrections for a strip of thickness `t` (Hammerstad
    /// and Jensen): `(u1, ur)`, the corrected width over height in air
    /// and in the dielectric.
    fn thick(u: f64, h: f64, t: f64, er: f64) -> (f64, f64) {
        if t <= 0.0 {
            return (u, u);
        }
        let th = t / h;
        let coth = 1.0 / (6.517 * u).sqrt().tanh();
        let du1 = th / PI * (1.0 + 4.0 * E / (th * coth * coth)).ln();
        let dur = 0.5 * (1.0 + 1.0 / (er - 1.0).max(0.0).sqrt().cosh()) * du1;
        (u + du1, u + dur)
    }

    /// Surface microstrip of width `w` and thickness `t` at height `h`
    /// over its plane in a dielectric of `er` (Hammerstad and Jensen,
    /// "Accurate Models for Microstrip Computer-Aided Design", IEEE
    /// MTT-S 1980): `(Z0, effective permittivity)`.
    pub fn microstrip(w: f64, h: f64, t: f64, er: f64) -> (f64, f64) {
        let u = w / h;
        let (u1, ur) = thick(u, h, t, er);
        let e = eps_e(ur, er);
        let z = z01(ur) / e.sqrt();
        let eff = e * (z01(u1) / z01(ur)).powi(2);
        (z, eff)
    }

    /// Embedded (buried) microstrip: a microstrip covered by `cover` mm
    /// of the same dielectric. The air impedance is Hammerstad and
    /// Jensen's; the permittivity is the IPC-2141-style approximation
    /// εr' = εr (1 − e^(−1.55 H/h)), H the dielectric's height over the
    /// plane, kept between the uncovered microstrip's value and εr.
    pub fn embedded_microstrip(w: f64, h: f64, t: f64, cover: f64, er: f64) -> (f64, f64) {
        let (air, _) = microstrip(w, h, t, 1.0);
        let (_, surface) = microstrip(w, h, t, er);
        let height = h + t + cover.max(0.0);
        let buried = er * (1.0 - (-1.55 * height / h).exp());
        let eff = buried.max(surface).min(er);
        (air / eff.sqrt(), eff)
    }

    /// Centred stripline of width `w` and thickness `t` between planes
    /// `b` apart (Wheeler, "Transmission-Line Properties of a Strip Line
    /// Between Parallel Planes", IEEE MTT-26, 1978, with his thickness
    /// correction).
    pub fn stripline_centred(w: f64, b: f64, t: f64, er: f64) -> f64 {
        let x = (t / b).clamp(0.0, 0.99);
        let dw = if x > 0.0 {
            let m = 2.0 / (1.0 + 2.0 / 3.0 * x / (1.0 - x));
            let inner = (x / (2.0 - x)).powi(2) + (0.0796 * x / (w / b + 1.1 * x)).powf(m);
            x / (PI * (1.0 - x)) * (1.0 - 0.5 * inner.ln()) * (b - t)
        } else {
            0.0
        };
        let ratio = (b - t) / (w + dw);
        let k = 8.0 / PI * ratio;
        30.0 / er.sqrt() * (1.0 + 4.0 / PI * ratio * (k + (k * k + 6.27).sqrt())).ln()
    }

    /// Stripline `h1` below the upper plane and `h2` above the lower one
    /// (both from the strip's faces): the centred stripline when they are
    /// equal, else the common parallel combination of the two centred
    /// lines of plane spacing 2·h1 + t and 2·h2 + t.
    pub fn stripline(w: f64, h1: f64, h2: f64, t: f64, er: f64) -> f64 {
        let a = stripline_centred(w, 2.0 * h1 + t, t, er);
        let b = stripline_centred(w, 2.0 * h2 + t, t, er);
        2.0 * a * b / (a + b)
    }

    /// Edge-coupled microstrip (surface or embedded) of single-ended
    /// `z0`, edge gap `s` and height `h`: IPC-2141's
    /// Zdiff = 2·Z0·(1 − 0.48·e^(−0.96·s/h)).
    pub fn diff_microstrip(z0: f64, s: f64, h: f64) -> f64 {
        2.0 * z0 * (1.0 - 0.48 * (-0.96 * s / h).exp())
    }

    /// Edge-coupled stripline of single-ended `z0`, edge gap `s` and
    /// plane spacing `b`: IPC-2141's Zdiff = 2·Z0·(1 − 0.347·e^(−2.9·s/b)).
    pub fn diff_stripline(z0: f64, s: f64, b: f64) -> f64 {
        2.0 * z0 * (1.0 - 0.347 * (-2.9 * s / b).exp())
    }

    /// The complete elliptic integral of the first kind, K(k), by the
    /// arithmetic-geometric mean.
    pub fn ellip_k(k: f64) -> f64 {
        let (mut a, mut b) = (1.0_f64, (1.0 - k * k).max(0.0).sqrt());
        for _ in 0..64 {
            if (a - b).abs() <= 1e-15 * a {
                break;
            }
            let next = ((a + b) / 2.0, (a * b).sqrt());
            (a, b) = next;
        }
        PI / (2.0 * a)
    }

    /// Cohn's exact impedance of a zero-thickness centred stripline
    /// (conformal mapping, 1954): (30π/√εr)·K(k)/K(k'), k = sech(πw/2b).
    /// The reference the tests hold [`stripline_centred`] to.
    pub fn cohn_stripline(w: f64, b: f64, er: f64) -> f64 {
        let x = PI * w / (2.0 * b);
        let k = 1.0 / x.cosh();
        let kp = x.tanh();
        30.0 * PI / er.sqrt() * ellip_k(k) / ellip_k(kp)
    }
}

/// How a trace on one copper layer sees its reference planes. Heights
/// are dielectric thicknesses from the trace's faces to the planes, mm.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Structure {
    /// An outer trace over one plane.
    Microstrip { h: f64, er: f64 },
    /// An inner trace with one plane, under `cover` mm of dielectric.
    EmbeddedMicrostrip { h: f64, cover: f64, er: f64 },
    /// An inner trace between two planes: `h1` to the upper, `h2` to the
    /// lower.
    Stripline { h1: f64, h2: f64, er: f64 },
}

impl Structure {
    pub fn label(&self) -> &'static str {
        match self {
            Structure::Microstrip { .. } => "Microstrip",
            Structure::EmbeddedMicrostrip { .. } => "Embedded microstrip",
            Structure::Stripline { h1, h2, .. } if (h1 - h2).abs() < 1e-6 => "Stripline",
            Structure::Stripline { .. } => "Offset stripline",
        }
    }

    fn er(&self) -> f64 {
        match *self {
            Structure::Microstrip { er, .. }
            | Structure::EmbeddedMicrostrip { er, .. }
            | Structure::Stripline { er, .. } => er,
        }
    }

    /// Single-ended Z0 of a trace of width `w` and thickness `t`, Ω.
    pub fn z0(&self, w: f64, t: f64) -> f64 {
        match *self {
            Structure::Microstrip { h, er } => formulas::microstrip(w, h, t, er).0,
            Structure::EmbeddedMicrostrip { h, cover, er } => {
                formulas::embedded_microstrip(w, h, t, cover, er).0
            }
            Structure::Stripline { h1, h2, er } => formulas::stripline(w, h1, h2, t, er),
        }
    }

    /// Edge-coupled differential impedance at edge gap `s`, Ω.
    pub fn zdiff(&self, w: f64, s: f64, t: f64) -> f64 {
        let z0 = self.z0(w, t);
        match *self {
            Structure::Microstrip { h, .. } | Structure::EmbeddedMicrostrip { h, .. } => {
                formulas::diff_microstrip(z0, s, h)
            }
            Structure::Stripline { h1, h2, .. } => formulas::diff_stripline(z0, s, h1 + h2 + t),
        }
    }

    /// The width giving single-ended `target` Ω, if any width in the
    /// solver's range does.
    pub fn width_for(&self, target: f64, t: f64) -> Option<f64> {
        solve_decreasing(|w| self.z0(w, t), target, SOLVE_RANGE.0, SOLVE_RANGE.1)
    }

    /// The width giving differential `target` Ω at edge gap `s`.
    pub fn width_for_diff(&self, target: f64, s: f64, t: f64) -> Option<f64> {
        solve_decreasing(
            |w| self.zdiff(w, s, t),
            target,
            SOLVE_RANGE.0,
            SOLVE_RANGE.1,
        )
    }

    /// The edge gap giving differential `target` Ω at width `w`.
    pub fn gap_for_diff(&self, target: f64, w: f64, t: f64) -> Option<f64> {
        // Zdiff grows with the gap: solve on its negation.
        solve_decreasing(|s| -self.zdiff(w, s, t), -target, 0.0, MAX_GAP)
    }
}

/// `x` in `[lo, hi]` where the decreasing `f(x)` meets `target`, by
/// bisection; `None` when the target lies outside `f`'s range there.
fn solve_decreasing(f: impl Fn(f64) -> f64, target: f64, lo: f64, hi: f64) -> Option<f64> {
    let (mut a, mut b) = (lo, hi);
    let (fa, fb) = (f(a), f(b));
    if !(fa.is_finite() && fb.is_finite()) || target > fa || target < fb {
        return None;
    }
    for _ in 0..200 {
        let mid = 0.5 * (a + b);
        if f(mid) > target {
            a = mid;
        } else {
            b = mid;
        }
        if b - a < 1e-9 {
            break;
        }
    }
    Some(0.5 * (a + b))
}

/// A layer's line structure and how it was found.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct LayerLine {
    pub layer: CopperLayer,
    pub structure: Structure,
    /// Copper thickness of the layer, mm.
    pub t: f64,
    /// The reference layers (one or two).
    pub references: Vec<CopperLayer>,
    /// The references are the neighbouring copper layers because no foil
    /// of the stackup is marked as a plane.
    pub assumed_references: bool,
    /// Some dielectric's Dk is the glass-style estimate.
    pub dk_estimated: bool,
}

/// The line structure of `layer` in `stackup`: its references are the
/// nearest foils marked as planes above and below it, or, when no foil is
/// marked, its neighbouring copper layers. An outer layer over a plane is
/// microstrip; an inner layer between two planes stripline; an inner
/// layer with one plane embedded microstrip. The dielectric between the
/// trace and its planes is averaged by thickness.
pub fn structure(stackup: &Stackup, layer: CopperLayer) -> Result<LayerLine> {
    let layers = stackup.layers();
    let index = stackup
        .index(layer)
        .ok_or_else(|| Error::Invalid(format!("the stackup has no layer {}", layer.name())))?;
    let marked = stackup.copper.iter().any(|c| c.plane);
    let is_ref =
        |j: usize| j != index && (!marked || stackup.copper.get(j).is_some_and(|c| c.plane));
    let above = (0..index)
        .rev()
        .find(|j| is_ref(*j) && (marked || *j + 1 == index));
    let below = (index + 1..layers.len()).find(|j| is_ref(*j) && (marked || *j == index + 1));
    let span = |j: usize| layers.get(j).and_then(|l| stackup.copper_span(*l));
    let (bottom, top) = stackup
        .copper_span(layer)
        .ok_or_else(|| Error::Invalid("layer outside the stackup".into()))?;
    let t = top - bottom;
    if let Some(d) = stackup
        .dielectrics
        .iter()
        .find(|d| stackup.permittivity(d).is_none())
    {
        return Err(Error::Invalid(format!(
            "the stackup has no Dk for its {} mm dielectric and no estimate for its glass: \
             enter the fabricator's Dk",
            d.thickness_mm
        )));
    }
    // Dielectrics between foil `a` and foil `b` (a < b): thickness-weighted Dk.
    let er_between = |a: usize, b: usize| {
        let parts: Vec<_> = stackup.dielectrics.get(a..b).unwrap_or_default().to_vec();
        let thick: f64 = parts.iter().map(|d| d.thickness_mm).sum();
        let weighted: f64 = parts
            .iter()
            .map(|d| d.thickness_mm * stackup.permittivity(d).unwrap_or(f64::NAN))
            .sum();
        let estimated = parts.iter().any(|d| d.permittivity_estimated());
        ((weighted / thick.max(1e-9)), estimated)
    };
    let h_above = above.and_then(span).map(|(ref_bottom, _)| ref_bottom - top);
    let h_below = below.and_then(span).map(|(_, ref_top)| bottom - ref_top);
    let outer = index == 0 || index + 1 == layers.len();
    let named = |j: Option<usize>| j.and_then(|j| layers.get(j).copied());
    let (structure, er_range, refs) = match (h_above, h_below) {
        (Some(h1), Some(h2)) => {
            let (Some(a), Some(b)) = (above, below) else {
                return Err(Error::Invalid("reference planes".into()));
            };
            let (er, est) = er_between(a, b);
            (
                Structure::Stripline { h1, h2, er },
                est,
                vec![named(above), named(below)],
            )
        }
        (Some(h), None) | (None, Some(h)) => {
            let reference = above.or(below).unwrap_or(index);
            let (lo, hi) = if reference < index {
                (reference, index)
            } else {
                (index, reference)
            };
            let (er, est) = er_between(lo, hi);
            if outer {
                (
                    Structure::Microstrip { h, er },
                    est,
                    vec![named(Some(reference))],
                )
            } else {
                // The dielectric over the trace on its open side, out to
                // the board surface.
                let surface = if reference > index {
                    stackup.copper_span(CopperLayer::Top).map(|(_, z)| z)
                } else {
                    stackup.copper_span(CopperLayer::Bottom).map(|(z, _)| z)
                };
                let cover = surface.map_or(0.0, |z| {
                    if reference > index {
                        (z - top).max(0.0)
                    } else {
                        (bottom - z).max(0.0)
                    }
                });
                (
                    Structure::EmbeddedMicrostrip { h, cover, er },
                    est,
                    vec![named(Some(reference))],
                )
            }
        }
        (None, None) => {
            return Err(Error::Invalid(format!(
                "{} has no reference plane",
                layer.name()
            )));
        }
    };
    let er = structure.er();
    if !(er.is_finite() && er >= 1.0 && t.is_finite() && t > 0.0) {
        return Err(Error::Invalid("stackup values out of range".into()));
    }
    let heights = match structure {
        Structure::Microstrip { h, .. } | Structure::EmbeddedMicrostrip { h, .. } => [h, h],
        Structure::Stripline { h1, h2, .. } => [h1, h2],
    };
    if !heights.iter().all(|h| h.is_finite() && *h > 0.0) {
        return Err(Error::Invalid(format!(
            "{} lies on its reference plane",
            layer.name()
        )));
    }
    Ok(LayerLine {
        layer,
        structure,
        t,
        references: refs.into_iter().flatten().collect(),
        assumed_references: !marked,
        dk_estimated: er_range,
    })
}

/// What a request to the calculator describes: a layer of the project's
/// stackup, or an explicit structure.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "on", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImpedanceRequest {
    /// A trace of `width` (and a pair at edge `gap`) on a stackup layer.
    Layer {
        layer: CopperLayer,
        width: f64,
        #[serde(default)]
        gap: Option<f64>,
    },
    /// An explicit structure with copper thickness `t`, mm.
    Structure {
        structure: Structure,
        t: f64,
        width: f64,
        #[serde(default)]
        gap: Option<f64>,
    },
}

/// `impedance.calculate`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ImpedanceResult {
    pub structure: Structure,
    pub label: String,
    pub t: f64,
    pub width: f64,
    pub z0: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zdiff: Option<f64>,
    /// The model's stated accuracy, one line.
    pub accuracy: String,
}

fn positive(v: f64, what: &str) -> Result<()> {
    if v.is_finite() && v > 0.0 && v <= 100.0 {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "{what} must be above 0 and at most 100 mm"
        )))
    }
}

/// The accuracy statement shown with each result.
pub fn accuracy(structure: &Structure, differential: bool) -> &'static str {
    match (structure, differential) {
        (Structure::Microstrip { .. }, false) => {
            "Hammerstad–Jensen, quasi-static: about 1 %; no mask, etch or roughness"
        }
        (Structure::Stripline { .. }, false) => {
            "Wheeler 1978, offset by parallel combination: about 1–3 %"
        }
        (Structure::EmbeddedMicrostrip { .. }, false) => {
            "IPC-2141-style buried permittivity: about 5 %"
        }
        (_, true) => "IPC-2141 coupling approximation: about 5–10 %",
    }
}

/// `impedance.calculate`: Z0 (and Zdiff with a gap) of one trace.
pub fn calculate(project: &Project, request: &ImpedanceRequest) -> Result<ImpedanceResult> {
    let (structure, t, width, gap) = match request {
        ImpedanceRequest::Layer { layer, width, gap } => {
            let line = structure(&project.stackup, *layer)?;
            (line.structure, line.t, *width, *gap)
        }
        ImpedanceRequest::Structure {
            structure,
            t,
            width,
            gap,
        } => {
            let ok = |v: f64| v.is_finite() && (0.0..=100.0).contains(&v);
            let valid = match *structure {
                Structure::Microstrip { h, er } => ok(h) && h > 0.0 && ok(er) && er >= 1.0,
                Structure::EmbeddedMicrostrip { h, cover, er } => {
                    ok(h) && h > 0.0 && ok(cover) && ok(er) && er >= 1.0
                }
                Structure::Stripline { h1, h2, er } => {
                    ok(h1) && ok(h2) && h1 > 0.0 && h2 > 0.0 && ok(er) && er >= 1.0
                }
            };
            if !valid {
                return Err(Error::Invalid(
                    "heights must be above 0 mm and the permittivity at least 1".into(),
                ));
            }
            positive(*t, "Copper thickness")?;
            (*structure, *t, *width, *gap)
        }
    };
    positive(width, "Width")?;
    if let Some(gap) = gap {
        positive(gap, "Gap")?;
    }
    let differential = gap.is_some();
    Ok(ImpedanceResult {
        label: structure.label().into(),
        z0: structure.z0(width, t),
        zdiff: gap.map(|s| structure.zdiff(width, s, t)),
        structure,
        t,
        width,
        gap,
        accuracy: accuracy(&structure, differential).into(),
    })
}

/// The widths (and pair gap) a class's targets need on one layer.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Derived {
    pub layer: CopperLayer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<Structure>,
    /// Single-ended: the track width and the impedance it gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z0: Option<f64>,
    /// Differential: the pair width and gap and the impedance they give.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_gap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zdiff: Option<f64>,
    /// Why the targets cannot be met here within the rules, one line each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

/// One class's derived values on every layer (`impedance.table`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ClassImpedance {
    pub class: NetClassId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_impedance: Option<f64>,
    /// The stated tolerance; without one routed impedance is not checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance_pct: Option<f64>,
    pub layers: Vec<Derived>,
}

/// `impedance.table`: every class with an impedance target and its
/// derived widths and gaps on every layer, and each layer's structure.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ImpedanceTable {
    pub layers: Vec<LayerReport>,
    pub classes: Vec<ClassImpedance>,
}

/// One layer's line structure, or why it has none.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct LayerReport {
    pub layer: CopperLayer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<LayerLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

fn mm(v: f64) -> String {
    super::limits::mm(v)
}

fn ohms(v: f64) -> String {
    format!("{v:.1} Ω")
}

/// The narrowest track routing accepts on `class`'s nets.
fn min_width(project: &Project, class: Option<NetClassId>) -> f64 {
    project.rules.track_width_min(class).value
}

/// The least edge gap between the two halves of a pair: the profile's
/// track-to-track clearance (class clearances do not apply inside a
/// pair; see `layout::limits`).
pub fn pair_gap_min(project: &Project) -> f64 {
    let key = RuleKey::Clearance {
        a: CopperObject::Track,
        b: CopperObject::Track,
    };
    match key.get(&project.rules.effective()) {
        crate::rules::RuleValue::Length(mm) => mm,
        _ => 0.0,
    }
}

/// The widths and gap `class`'s impedance targets need on `layer`,
/// within the rules: no narrower than the class's minimum width, no
/// wider than routing accepts, and a pair gap no smaller than the
/// profile's track clearance. When a target cannot be met, the nearest
/// value within the rules is given with a problem line.
pub fn derive(project: &Project, class: Option<NetClassId>, layer: CopperLayer) -> Derived {
    let rules = &project.rules;
    let signal = rules.signal(class);
    let class_def = class.and_then(|c| rules.class(c));
    let mut out = Derived {
        layer,
        structure: None,
        width: None,
        z0: None,
        pair_width: None,
        pair_gap: None,
        zdiff: None,
        problems: Vec::new(),
    };
    if signal.impedance.is_none() && signal.diff_impedance.is_none() {
        return out;
    }
    let line = match structure(&project.stackup, layer) {
        Ok(line) => line,
        Err(e) => {
            out.problems.push(crate::diagnostics::Error::to_string(&e));
            return out;
        }
    };
    let (s, t) = (line.structure, line.t);
    out.structure = Some(s);
    let min = min_width(project, class);
    let name = layer.name();
    let clamp_width = |w: f64, problems: &mut Vec<String>, target: f64| -> f64 {
        if w + 1e-9 < min {
            problems.push(format!(
                "{name} needs {} mm for {}, below the {} mm minimum width",
                mm(w),
                ohms(target),
                mm(min)
            ));
            min
        } else if w > MAX_ROUTE_WIDTH {
            problems.push(format!(
                "{name} needs {} mm for {}, wider than routing allows",
                mm(w),
                ohms(target)
            ));
            MAX_ROUTE_WIDTH
        } else {
            w
        }
    };
    if let Some(target) = signal.impedance {
        let w = match s.width_for(target, t) {
            Some(w) => clamp_width(w, &mut out.problems, target),
            None => {
                out.problems
                    .push(format!("{name} cannot reach {} at any width", ohms(target)));
                min.max(rules.track_width(class).value)
            }
        };
        out.width = Some(w);
        out.z0 = Some(s.z0(w, t));
    }
    if let Some(target) = signal.diff_impedance {
        let gap_min = pair_gap_min(project);
        let set_width = class_def.and_then(|c| c.diff_pair_width);
        let set_gap = class_def.and_then(|c| c.diff_pair_gap);
        let (w, g) = match (set_width, set_gap) {
            (Some(w), Some(g)) => (w, g),
            (None, Some(g)) => {
                let w = match s.width_for_diff(target, g, t) {
                    Some(w) => clamp_width(w, &mut out.problems, target),
                    None => {
                        out.problems.push(format!(
                            "{name} cannot reach {} at a {} mm gap",
                            ohms(target),
                            mm(g)
                        ));
                        min
                    }
                };
                (w, g)
            }
            (width, None) => {
                // A given width (else the single-ended one) sets the gap;
                // without either, the tightest gap the rules allow sets
                // the width.
                let base = width.or(out.width.filter(|_| out.problems.is_empty()));
                match base {
                    Some(w) => match s.gap_for_diff(target, w, t) {
                        Some(g) => (w, g),
                        None => {
                            out.problems.push(format!(
                                "{name} cannot reach {} at {} mm width",
                                ohms(target),
                                mm(w)
                            ));
                            (w, gap_min)
                        }
                    },
                    None => {
                        let g = gap_min;
                        let w = match s.width_for_diff(target, g, t) {
                            Some(w) => clamp_width(w, &mut out.problems, target),
                            None => min,
                        };
                        (w, g)
                    }
                }
            }
        };
        if g + 1e-9 < gap_min {
            out.problems.push(format!(
                "{name} needs a {} mm gap, below the {} mm track clearance",
                mm(g),
                mm(gap_min)
            ));
        }
        out.pair_width = Some(w);
        out.pair_gap = Some(g.max(gap_min));
        out.zdiff = Some(s.zdiff(w, g.max(gap_min), t));
    }
    out
}

/// `impedance.table`.
pub fn table(project: &Project) -> ImpedanceTable {
    let layers = project.stackup.layers();
    let reports = layers
        .iter()
        .map(|l| match structure(&project.stackup, *l) {
            Ok(line) => LayerReport {
                layer: *l,
                line: Some(line),
                problem: None,
            },
            Err(e) => LayerReport {
                layer: *l,
                line: None,
                problem: Some(e.to_string()),
            },
        })
        .collect();
    let classes = project
        .rules
        .net_classes
        .iter()
        .filter(|c| c.signal.impedance.is_some() || c.signal.diff_impedance.is_some())
        .map(|c| ClassImpedance {
            class: c.id,
            name: c.name.clone(),
            impedance: c.signal.impedance,
            diff_impedance: c.signal.diff_impedance,
            tolerance_pct: c.signal.tolerance_pct(),
            layers: layers
                .iter()
                .map(|l| derive(project, Some(c.id), *l))
                .collect(),
        })
        .collect();
    ImpedanceTable {
        layers: reports,
        classes,
    }
}

/// The track width the Route tool uses for `net` on `layer`: the width
/// its class's single-ended impedance target needs there, else the
/// class's (or default) width.
pub fn route_width_on(project: &Project, net: Id, layer: CopperLayer) -> f64 {
    let class = super::net_class(project, net);
    derive(project, class, layer)
        .width
        .unwrap_or_else(|| super::route_width(project, net).value)
}

/// The width and edge gap a pair routes with on `layer`: the derived ones
/// when its class (the positive net's) has a differential target, else
/// the class's pair width and gap, else its track width and the
/// profile's track clearance.
pub fn pair_width_gap(project: &Project, positive: Id, layer: CopperLayer) -> (f64, f64) {
    let class = super::net_class(project, positive);
    let derived = derive(project, class, layer);
    if let (Some(w), Some(g)) = (derived.pair_width, derived.pair_gap) {
        return (w, g);
    }
    let def = class.and_then(|c| project.rules.class(c));
    let width = def
        .and_then(|c| c.diff_pair_width)
        .unwrap_or_else(|| super::route_width(project, positive).value);
    let gap = def
        .and_then(|c| c.diff_pair_gap)
        .unwrap_or_else(|| pair_gap_min(project));
    (width, gap.max(pair_gap_min(project)))
}

#[cfg(test)]
mod tests;
