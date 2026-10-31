//! "Fit leads to pads": a pure rigid fit of tagged lead-contact points onto
//! pad centres, the one-click placement actions, and the advisory placement
//! findings. The core never reads geometry files: lead contacts come from
//! the typed [`PackageForm`] ([`PackageForm::lead_points`]) or from the host
//! (the integration crate, for imported models) as [`LeadPair`]s.
//!
//! Lead `k` of [`PackageForm::lead_points`] belongs to
//! `package.terminals[k]`; the lead order (shared with the generated
//! models) is documented in [`super::package_form`].
use super::footprint::{FootprintDefinition, PAD_ARC_TOLERANCE_MM, PadKind};
use super::package_form::{MAX_FORM_LEADS, PackageForm};
use super::package_model::{self, Axis, ModelPlacement, PackageEdit, QuickAction, model_frame};
use super::publish::object;
use super::{Datum, PackageDefinition};
use crate::diagnostics::{Error, Finding, Result};
use crate::geometry::{Point, Point3, Transform3, cos_sin};
use crate::ids::{DefinitionId, PackageTerminalId, PadId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Lead contact off its land by more than this fraction of the land width
/// (estimate, advisory).
pub const LEAD_OFF_LAND_FRACTION: f64 = 0.25;

/// Most lead/pad pairs one fit takes.
pub const MAX_LEAD_PAIRS: usize = 4096;

/// A lead contact further than this from the seating plane, or a body below
/// it by more, gives a finding (mm, estimate).
pub const SEAT_TOLERANCE_MM: f64 = 0.01;

/// A mirrored assignment is reported when its error is below this share of
/// the fitted error (and the fit itself is off by more than
/// [`MIRROR_MIN_RMS_MM`]).
const MIRROR_SHARE: f64 = 0.5;
const MIRROR_MIN_RMS_MM: f64 = 0.05;

/// Which rotations the fit may use.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FitRotation {
    Free,
    Quarter,
}

/// One terminal's lead-contact centroid in the model frame and the centre
/// of the pad it lands on, in the datum frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeadPair {
    pub terminal: PackageTerminalId,
    pub pad: PadId,
    pub lead: Point3,
    pub pad_centre: Point,
}

/// Residual of one terminal after the fit, in mm.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeadResidual {
    pub terminal: PackageTerminalId,
    pub error_mm: f64,
}

/// The fit result: the placement that seats the leads, and its errors.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeadFit {
    pub placement: ModelPlacement,
    pub rms_mm: f64,
    pub residuals: Vec<LeadResidual>,
    /// A mirrored assignment would fit better (pin order reversed).
    pub mirrored_better: bool,
}

/// 2D rigid least-squares fit of `pairs`, then seat on z = 0, starting from
/// `current` (its x/y rotations are kept).
///
/// The leads are first turned by `current`'s x and y rotations; the fit then finds
/// the turn about the datum z axis and the x/y shift that best map their
/// x/y onto the pad centres (2D Kabsch), and lifts the model so its lowest
/// lead contact lies on the seating plane. With [`FitRotation::Quarter`]
/// only the four quarter turns are tried. A single pair only translates.
pub fn fit_leads(
    current: &ModelPlacement,
    pairs: &[LeadPair],
    rotation: FitRotation,
) -> Result<LeadFit> {
    validate_pairs(current, pairs)?;
    // The z turn is solved from scratch; only the tilt is kept, so a
    // quarter-turn fit of an untilted model is exact.
    let [rx, ry, _] = current.rotation_deg;
    let base = Transform3::intrinsic_xyz([rx, ry, 0.0]);
    let turned: Vec<Point3> = pairs.iter().map(|p| base.apply(p.lead)).collect();
    let targets: Vec<Point> = pairs.iter().map(|p| p.pad_centre).collect();
    let flat: Vec<Point> = turned.iter().map(|q| [q[0], q[1]]).collect();
    let (angle, shift) = solve_2d(&flat, &targets, rotation);
    let lowest = turned.iter().map(|q| q[2]).fold(f64::INFINITY, f64::min);
    let outer = Transform3::translation([shift[0], shift[1], -lowest])
        .then(&Transform3::rot_z(angle))
        .then(&base);
    let placement = placement_of(&outer)?;
    let frame = placement.transform();
    let residuals: Vec<LeadResidual> = pairs
        .iter()
        .map(|p| {
            let q = frame.apply(p.lead);
            LeadResidual {
                terminal: p.terminal,
                error_mm: (q[0] - p.pad_centre[0]).hypot(q[1] - p.pad_centre[1]),
            }
        })
        .collect();
    let rms_mm = rms(residuals.iter().map(|r| r.error_mm));
    // The same leads mirrored across the x axis: what a reversed (clockwise)
    // pin order would land.
    let mirrored: Vec<Point> = flat.iter().map(|q| [q[0], -q[1]]).collect();
    let (m_angle, m_shift) = solve_2d(&mirrored, &targets, rotation);
    let m_rms = rms(mirrored.iter().zip(&targets).map(|(q, p)| {
        let r = rotate(*q, m_angle);
        (r[0] + m_shift[0] - p[0]).hypot(r[1] + m_shift[1] - p[1])
    }));
    Ok(LeadFit {
        placement,
        rms_mm,
        residuals,
        mirrored_better: rms_mm > MIRROR_MIN_RMS_MM && m_rms < MIRROR_SHARE * rms_mm,
    })
}

fn validate_pairs(current: &ModelPlacement, pairs: &[LeadPair]) -> Result<()> {
    if pairs.is_empty() || pairs.len() > MAX_LEAD_PAIRS {
        return Err(Error::Invalid(format!(
            "package.fit_pairs: fitting takes 1–{MAX_LEAD_PAIRS} lead/pad pairs"
        )));
    }
    if !current.finite() {
        return Err(Error::Invalid(
            "package.placement: placement values must be finite and within ±10000".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for pair in pairs {
        let finite = pair
            .lead
            .iter()
            .chain(&pair.pad_centre)
            .all(|v| v.is_finite() && v.abs() <= crate::geometry::COORDINATE_LIMIT_MM);
        if !finite {
            return Err(Error::Invalid(format!(
                "package.fit_pairs: terminal {} has a non-finite or out-of-range point",
                pair.terminal
            )));
        }
        if !seen.insert(pair.terminal) {
            return Err(Error::Invalid(format!(
                "package.fit_pairs: terminal {} is paired twice",
                pair.terminal
            )));
        }
    }
    Ok(())
}

fn rms(errors: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = errors.fold((0.0, 0usize), |(s, n), e| (s + e * e, n + 1));
    if n == 0 { 0.0 } else { (sum / n as f64).sqrt() }
}

fn rotate(p: Point, deg: f64) -> Point {
    let (c, s) = cos_sin(deg);
    [c * p[0] - s * p[1], s * p[0] + c * p[1]]
}

fn centroid(points: &[Point]) -> Point {
    let n = points.len().max(1) as f64;
    let sum = points
        .iter()
        .fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
    [sum[0] / n, sum[1] / n]
}

/// The turn (degrees, about the origin) and shift mapping `from` onto `to`
/// by least squares: `to ≈ R(angle)·from + shift`.
fn solve_2d(from: &[Point], to: &[Point], rotation: FitRotation) -> (f64, Point) {
    let (cf, ct) = (centroid(from), centroid(to));
    let shift_for = |angle: f64| {
        let r = rotate(cf, angle);
        [ct[0] - r[0], ct[1] - r[1]]
    };
    let angle = match rotation {
        FitRotation::Free => {
            let (mut dot, mut cross) = (0.0, 0.0);
            for (a, b) in from.iter().zip(to) {
                let (a, b) = ([a[0] - cf[0], a[1] - cf[1]], [b[0] - ct[0], b[1] - ct[1]]);
                dot += a[0] * b[0] + a[1] * b[1];
                cross += a[0] * b[1] - a[1] * b[0];
            }
            if dot.hypot(cross) < 1e-12 {
                0.0
            } else {
                snap_degrees(cross.atan2(dot).to_degrees())
            }
        }
        FitRotation::Quarter => {
            let error = |angle: f64| {
                let shift = shift_for(angle);
                from.iter()
                    .zip(to)
                    .map(|(a, b)| {
                        let r = rotate(*a, angle);
                        (r[0] + shift[0] - b[0]).powi(2) + (r[1] + shift[1] - b[1]).powi(2)
                    })
                    .sum::<f64>()
            };
            // Ties keep the smallest turn, so a symmetric part stays put.
            [0.0, 90.0, -90.0, 180.0]
                .into_iter()
                .map(|a| (a, error(a)))
                .fold((0.0, f64::INFINITY), |best, (a, e)| {
                    if e < best.1 - 1e-12 { (a, e) } else { best }
                })
                .0
        }
    };
    (angle, shift_for(angle))
}

/// An angle within 1e-9° of a quarter turn becomes exact, in (−180, 180].
fn snap_degrees(deg: f64) -> f64 {
    let quarter = (deg / 90.0).round() * 90.0;
    let deg = if (deg - quarter).abs() < 1e-9 {
        quarter
    } else {
        deg
    };
    let wrapped = deg.rem_euclid(360.0);
    let wrapped = if wrapped > 180.0 {
        wrapped - 360.0
    } else {
        wrapped
    };
    if wrapped == 0.0 { 0.0 } else { wrapped }
}

/// The intrinsic X → Y → Z angles (degrees) of a rotation matrix:
/// `R = Rx(a)·Ry(b)·Rz(c)`. Quarter turns come out exact.
pub fn intrinsic_xyz_of(r: &[[f64; 3]; 3]) -> Point3 {
    let sb = r[0][2].clamp(-1.0, 1.0);
    let (a, b, c) = if sb.abs() > 1.0 - 1e-12 {
        // Gimbal lock: only a ± c is defined; put it all in a.
        (
            r[2][1].atan2(r[1][1]),
            sb.signum() * 90.0_f64.to_radians(),
            0.0,
        )
    } else {
        (
            (-r[1][2]).atan2(r[2][2]),
            sb.asin(),
            (-r[0][1]).atan2(r[0][0]),
        )
    };
    [a, b, c].map(|v| snap_degrees(v.to_degrees()))
}

/// The placement of a rigid model → datum transform (no mirror).
fn placement_of(t: &Transform3) -> Result<ModelPlacement> {
    let placement = ModelPlacement {
        translation: t.translation.map(|v| if v == 0.0 { 0.0 } else { v }),
        rotation_deg: intrinsic_xyz_of(&t.rotation),
    };
    if placement.finite() {
        Ok(placement)
    } else {
        Err(Error::Invalid(
            "package.placement: the result is not finite or exceeds ±10000 mm".into(),
        ))
    }
}

/// `outer ∘ placement`: the placement moved by a datum-frame transform.
pub fn moved(placement: &ModelPlacement, outer: &Transform3) -> Result<ModelPlacement> {
    placement_of(&outer.then(&placement.transform()))
}

/// A turn of `deg` about datum `axis` through `pivot`.
pub fn turn_about(axis: Axis, deg: f64, pivot: Point3) -> Transform3 {
    let turn = match axis {
        Axis::X => Transform3::rot_x(deg),
        Axis::Y => Transform3::rot_y(deg),
        Axis::Z => Transform3::rot_z(deg),
    };
    Transform3::translation(pivot)
        .then(&turn)
        .then(&Transform3::translation(pivot.map(|v| -v)))
}

/// The body's box in the model frame (min, max), leads excluded.
fn body_box(package: &PackageDefinition) -> (Point3, Point3) {
    let centred =
        |l: f64, w: f64, z0: f64, z1: f64| ([-l / 2.0, -w / 2.0, z0], [l / 2.0, w / 2.0, z1]);
    match &package.form {
        PackageForm::Generic => {
            let s = package.envelope;
            centred(s[0], s[1], 0.0, s[2])
        }
        PackageForm::Chip(c) => centred(c.length, c.width, 0.0, c.height),
        PackageForm::GullWing(g) => centred(
            g.body_length,
            g.body_width,
            g.standoff,
            g.standoff + g.body_height,
        ),
        PackageForm::NoLead(n) => centred(n.body_length, n.body_width, 0.0, n.body_height),
        PackageForm::Header(h) => centred(
            h.pitch * f64::from(h.pins_per_row.min(MAX_FORM_LEADS)),
            h.row_pitch * f64::from(h.rows.min(MAX_FORM_LEADS)),
            0.0,
            h.body_height,
        ),
    }
}

fn box_corners((min, max): (Point3, Point3)) -> [Point3; 8] {
    std::array::from_fn(|k| {
        [
            if k & 1 == 0 { min[0] } else { max[0] },
            if k & 2 == 0 { min[1] } else { max[1] },
            if k & 4 == 0 { min[2] } else { max[2] },
        ]
    })
}

/// The body's corners in the datum frame under `placement`.
pub fn body_corners(package: &PackageDefinition, placement: &ModelPlacement) -> [Point3; 8] {
    let t = placement.transform();
    box_corners(body_box(package)).map(|p| t.apply(p))
}

/// The centre of the placed body in the datum frame: the pivot quick turns
/// and the triad use.
pub fn body_centre(package: &PackageDefinition, placement: &ModelPlacement) -> Point3 {
    let (min, max) = body_box(package);
    placement
        .transform()
        .apply(std::array::from_fn(|i| (min[i] + max[i]) / 2.0))
}

/// Footprint frame → datum frame.
pub fn to_datum(datum: &Datum, p: Point) -> Point {
    rotate(
        [p[0] - datum.offset[0], p[1] - datum.offset[1]],
        -datum.rotation_deg,
    )
}

fn copper(kind: PadKind) -> bool {
    matches!(kind, PadKind::Smd | PadKind::PlatedThrough)
}

/// The lead/pad pairs of a typed form on `footprint`: lead `k` of
/// [`PackageForm::lead_points`] with the centroid of the copper pads landing
/// `package.terminals[k]`, in the datum frame. Terminals without a pad are
/// left out; never inferred from numbers or names.
pub fn form_pairs(
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
) -> Result<Vec<LeadPair>> {
    let contacts = package.form.lead_points();
    if contacts.is_empty() {
        return Err(Error::Invalid(format!(
            "package.fit_no_leads: a {} package has no typed lead contacts; pass lead pairs",
            package.form.label()
        )));
    }
    if contacts.len() != package.terminals.len() {
        return Err(Error::Invalid(format!(
            "package.fit_terminal_count: the form has {} lead contacts but the package has {} terminals",
            contacts.len(),
            package.terminals.len()
        )));
    }
    let pairs: Vec<LeadPair> = package
        .terminals
        .iter()
        .zip(contacts)
        .filter_map(|(terminal, lead)| {
            let pads: Vec<_> = footprint
                .pads
                .iter()
                .filter(|p| p.terminal == Some(terminal.id) && copper(p.kind))
                .collect();
            let first = pads.first()?;
            let centres: Vec<Point> = pads
                .iter()
                .map(|p| to_datum(&footprint.datum, p.position))
                .collect();
            Some(LeadPair {
                terminal: terminal.id,
                pad: first.id,
                lead,
                pad_centre: centroid(&centres),
            })
        })
        .collect();
    if pairs.is_empty() {
        return Err(Error::Invalid(
            "package.fit_no_pads: no footprint pad lands a terminal of the package".into(),
        ));
    }
    Ok(pairs)
}

/// Host-supplied pairs name terminals of `package` and, when the landing
/// footprint is known, its pads landing that terminal; never accepted
/// unchecked.
pub fn check_pair_ids(
    package: &PackageDefinition,
    footprint: Option<&FootprintDefinition>,
    pairs: &[LeadPair],
) -> Result<()> {
    for pair in pairs {
        if package.terminal(pair.terminal).is_none() {
            return Err(Error::NotFound(format!(
                "package.fit_pairs: the package has no terminal {}",
                pair.terminal
            )));
        }
        if let Some(footprint) = footprint {
            let pad = footprint.pad(pair.pad).ok_or_else(|| {
                Error::NotFound(format!(
                    "package.fit_pairs: the footprint has no pad {}",
                    pair.pad
                ))
            })?;
            if pad.terminal != Some(pair.terminal) {
                return Err(Error::Invalid(format!(
                    "package.fit_pairs: pad {} does not land terminal {}",
                    pair.pad, pair.terminal
                )));
            }
        }
    }
    Ok(())
}

/// A fit residual that placement cannot remove: the leads' spread and the
/// pads' differ, so the body or the footprint must change. Returns `(lead
/// spacing, pad spacing)` of two pairs far apart (the pad furthest from the
/// pads' centroid and the pad furthest from it), when they differ by more
/// than `tolerance_mm`. Linear in the pairs.
pub fn spacing_mismatch(pairs: &[LeadPair], tolerance_mm: f64) -> Option<(f64, f64)> {
    let distance = |p: Point, q: Point| (p[0] - q[0]).hypot(p[1] - q[1]);
    let furthest = |from: Point| {
        pairs
            .iter()
            .max_by(|a, b| distance(a.pad_centre, from).total_cmp(&distance(b.pad_centre, from)))
    };
    let centre = centroid(&pairs.iter().map(|p| p.pad_centre).collect::<Vec<_>>());
    let a = furthest(centre)?;
    let b = furthest(a.pad_centre)?;
    let leads = distance([a.lead[0], a.lead[1]], [b.lead[0], b.lead[1]]);
    let pads = distance(a.pad_centre, b.pad_centre);
    ((leads - pads).abs() > tolerance_mm).then_some((leads, pads))
}

fn needs_footprint(action: QuickAction) -> Error {
    Error::Invalid(format!(
        "package.quick_needs_footprint: {action:?} needs the footprint the package lands on"
    ))
}

/// Move the lowest lead contact (or, without leads, the body's underside)
/// onto the seating plane.
fn seat(package: &PackageDefinition, placement: &ModelPlacement) -> Result<ModelPlacement> {
    let t = placement.transform();
    let contacts = package.form.lead_points();
    let lowest = if contacts.is_empty() {
        body_corners(package, placement)
            .iter()
            .map(|p| p[2])
            .fold(f64::INFINITY, f64::min)
    } else {
        contacts
            .iter()
            .map(|p| t.apply(*p)[2])
            .fold(f64::INFINITY, f64::min)
    };
    moved(placement, &Transform3::translation([0.0, 0.0, -lowest]))
}

/// Centre the lead contacts on their pads in x/y; without typed leads, the
/// body's centre on the centroid of all copper pads.
fn centre_on_pads(
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
    placement: &ModelPlacement,
) -> Result<ModelPlacement> {
    let t = placement.transform();
    let (from, to) = match form_pairs(package, footprint) {
        Ok(pairs) => (
            centroid(
                &pairs
                    .iter()
                    .map(|p| {
                        let q = t.apply(p.lead);
                        [q[0], q[1]]
                    })
                    .collect::<Vec<_>>(),
            ),
            centroid(&pairs.iter().map(|p| p.pad_centre).collect::<Vec<_>>()),
        ),
        Err(_) => {
            let pads: Vec<Point> = footprint
                .pads
                .iter()
                .filter(|p| copper(p.kind))
                .map(|p| to_datum(&footprint.datum, p.position))
                .collect();
            if pads.is_empty() {
                return Err(Error::Invalid(
                    "package.quick_no_pads: the footprint has no copper pads".into(),
                ));
            }
            let c = body_centre(package, placement);
            ([c[0], c[1]], centroid(&pads))
        }
    };
    moved(
        placement,
        &Transform3::translation([to[0] - from[0], to[1] - from[1], 0.0]),
    )
}

/// The quarter turn about z that brings terminal 1's lead nearest pad 1,
/// then centred on the pads.
fn pin1_to_pad1(
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
    placement: &ModelPlacement,
) -> Result<ModelPlacement> {
    let pairs = form_pairs(package, footprint)?;
    let first = package.terminals.first().map(|t| t.id);
    let pin1 = pairs
        .iter()
        .find(|p| Some(p.terminal) == first)
        .ok_or_else(|| {
            Error::Invalid("package.quick_no_pad1: terminal 1 has no footprint pad".into())
        })?;
    let pivot = body_centre(package, placement);
    let mut best: Option<(f64, ModelPlacement)> = None;
    for k in 0..4 {
        let turned = moved(placement, &turn_about(Axis::Z, 90.0 * f64::from(k), pivot))?;
        let centred = centre_on_pads(package, footprint, &turned)?;
        let q = centred.transform().apply(pin1.lead);
        let error = (q[0] - pin1.pad_centre[0]).hypot(q[1] - pin1.pad_centre[1]);
        if best.as_ref().is_none_or(|(e, _)| error < *e - 1e-9) {
            best = Some((error, centred));
        }
    }
    best.map(|(_, p)| p)
        .ok_or_else(|| Error::Invalid("package.quick_no_pad1: no candidate turn".into()))
}

/// The placement a quick action gives. `footprint` is the footprint the
/// package lands on; the pad-relative actions refuse without it.
pub fn apply_quick(
    package: &PackageDefinition,
    footprint: Option<&FootprintDefinition>,
    action: QuickAction,
) -> Result<ModelPlacement> {
    let current = &package.placement;
    if !current.finite() {
        return Err(Error::Invalid(
            "package.placement: placement values must be finite and within ±10000".into(),
        ));
    }
    let pivot = body_centre(package, current);
    match action {
        QuickAction::Rotate {
            axis,
            quarter_turns,
        } => {
            if !(-3..=3).contains(&quarter_turns) || quarter_turns == 0 {
                return Err(Error::Invalid(
                    "package.quick_turns: rotate by 1–3 quarter turns either way".into(),
                ));
            }
            moved(
                current,
                &turn_about(axis, 90.0 * f64::from(quarter_turns), pivot),
            )
        }
        QuickAction::Flip => moved(current, &turn_about(Axis::X, 180.0, pivot)),
        QuickAction::StandUp => {
            // The source's +y becomes the datum's +z: a quarter turn about
            // the model's own x axis, then seated.
            let stood = placement_of(&current.transform().then(&Transform3::rot_x(90.0)))?;
            seat(package, &stood)
        }
        QuickAction::Seat => seat(package, current),
        QuickAction::CentreOnPads => {
            centre_on_pads(package, footprint.ok_or(needs_footprint(action))?, current)
        }
        QuickAction::Pin1ToPad1 => {
            pin1_to_pad1(package, footprint.ok_or(needs_footprint(action))?, current)
        }
    }
}

/// [`package_model::edit_package`] with the footprint the package lands
/// on, so the pad-relative quick actions can run. Atomic: any failure
/// returns an error and nothing else.
pub fn edit_package_in(
    package: &PackageDefinition,
    footprint: Option<&FootprintDefinition>,
    edits: &[PackageEdit],
) -> Result<PackageDefinition> {
    if edits.is_empty() || edits.len() > 256 {
        return Err(Error::Invalid(
            "library.edit_package takes 1–256 edits".into(),
        ));
    }
    let mut out = package.clone();
    for edit in edits {
        match edit {
            PackageEdit::ApplyQuick(action) => {
                out.placement = apply_quick(&out, footprint, *action)?;
            }
            other => out = package_model::edit_package(&out, std::slice::from_ref(other))?,
        }
    }
    Ok(out)
}

/// Where one typed lead contact lands under a placement.
#[derive(Clone, Debug, PartialEq)]
pub struct LeadState {
    pub terminal: PackageTerminalId,
    /// The contact in the datum frame (z: height above the seating plane).
    pub at: Point3,
    /// How far it misses its best land: past [`LEAD_OFF_LAND_FRACTION`] of
    /// the land width across the land, or outside it (≤ 0 is on the land);
    /// `None` when no copper pad lands the terminal.
    pub miss_mm: Option<f64>,
    pub pads: Vec<PadId>,
}

/// The typed lead contacts of `package` under `placement` against the
/// footprint's lands; empty when the form has no leads or its lead count
/// differs from the terminals.
pub fn lead_states(
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
    placement: &ModelPlacement,
) -> Vec<LeadState> {
    let contacts = package.form.lead_points();
    if contacts.len() != package.terminals.len() || !placement.finite() {
        return Vec::new();
    }
    let datum = placement.transform();
    let frame = model_frame(&footprint.datum, placement);
    package
        .terminals
        .iter()
        .zip(contacts)
        .map(|(terminal, lead)| {
            let q = frame.apply(lead);
            let pads: Vec<_> = footprint
                .pads
                .iter()
                .filter(|p| p.terminal == Some(terminal.id) && copper(p.kind))
                .collect();
            let miss_mm = pads
                .iter()
                .map(|pad| {
                    let local = rotate(
                        [q[0] - pad.position[0], q[1] - pad.position[1]],
                        -pad.rotation_deg,
                    );
                    let extent = pad.shape.extent();
                    let across = if extent[0] <= extent[1] { 0 } else { 1 };
                    let lateral = local[across].abs() - LEAD_OFF_LAND_FRACTION * extent[across];
                    let outside = pad.land(PAD_ARC_TOLERANCE_MM).distance([q[0], q[1]], true);
                    lateral.max(outside)
                })
                .reduce(f64::min);
            LeadState {
                terminal: terminal.id,
                at: datum.apply(lead),
                miss_mm,
                pads: pads.iter().map(|p| p.id).collect(),
            }
        })
        .collect()
}

/// Advisory placement findings of `package` on `footprint` (whose lineage
/// is `footprint_id`): lead contacts off their lands (more than
/// [`LEAD_OFF_LAND_FRACTION`] of the land width across it, or outside it),
/// lead contacts off the seating plane, and a body below the board.
pub fn placement_findings(
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
    footprint_id: DefinitionId,
) -> Vec<Finding> {
    let mut out = Vec::new();
    if !package.placement.finite() {
        return out;
    }
    let package_id = footprint.package.id;
    let lowest = body_corners(package, &package.placement)
        .iter()
        .map(|p| p[2])
        .fold(f64::INFINITY, f64::min);
    if lowest < -SEAT_TOLERANCE_MM {
        out.push(Finding::new(
            "package.body_below_board",
            format!(
                "The package body reaches {:.3} mm below the seating plane, into the board.",
                -lowest
            ),
            vec![crate::diagnostics::ObjectRef::Definition { id: package_id }],
        ));
    }
    let (mut off, mut off_count, mut off_worst) = (Vec::new(), 0usize, 0.0_f64);
    let (mut unseated, mut unseated_worst) = (Vec::new(), 0.0_f64);
    for lead in lead_states(package, footprint, &package.placement) {
        if lead.at[2].abs() > SEAT_TOLERANCE_MM {
            unseated.push(object(package_id, lead.terminal.0));
            unseated_worst = unseated_worst.max(lead.at[2].abs());
        }
        if let Some(miss) = lead.miss_mm.filter(|m| *m > 1e-9) {
            off.extend(lead.pads.iter().map(|p| object(footprint_id, p.0)));
            off.push(object(package_id, lead.terminal.0));
            off_count += 1;
            off_worst = off_worst.max(miss);
        }
    }
    if !off.is_empty() {
        out.push(Finding::new(
            "package.lead_off_land",
            format!(
                "{} lead contact(s) lie off their lands, by up to {:.3} mm past {} % of the land width (estimate).",
                off_count,
                off_worst,
                LEAD_OFF_LAND_FRACTION * 100.0
            ),
            off,
        ));
    }
    if !unseated.is_empty() {
        out.push(Finding::new(
            "package.lead_unseated",
            format!(
                "{} lead contact(s) are off the seating plane, by up to {unseated_worst:.3} mm; use Seat.",
                unseated.len()
            ),
            unseated,
        ));
    }
    out
}

#[cfg(test)]
mod tests;
