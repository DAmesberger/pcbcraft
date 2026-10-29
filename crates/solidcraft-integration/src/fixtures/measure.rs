use super::Parameters;
use crate::{Error, Result, SOLIDCRAFT_REVISION};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
use solidcraft_engine::{Session, doc::expr::Kind};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, JsonSchema)]
pub struct BodyMeasurement {
    pub name: String,
    pub definition: String,
    pub volume_mm3: f64,
    pub min_mm: [f64; 3],
    pub max_mm: [f64; 3],
    pub centroid_mm: [f64; 3],
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Placement {
    pub role: String,
    pub occurrence_id: u64,
    pub component_id: u64,
    pub origin_mm: [f64; 3],
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Snapshot {
    pub solidcraft_revision: String,
    pub parameters: Parameters,
    pub bodies: Vec<BodyMeasurement>,
    pub placements: Vec<Placement>,
    pub feature_count: usize,
    pub shared_package_occurrences: usize,
}

pub(crate) fn parameters(s: &Session) -> Result<Parameters> {
    let mut values = BTreeMap::new();
    for name in Parameters::default().values().keys() {
        let angle = name.ends_with("angle");
        let value = s
            .doc
            .eval(name, if angle { Kind::Angle } else { Kind::Length })
            .map_err(|e| Error::Verification(format!("parameter {name}: {e}")))?;
        values.insert(*name, if angle { value.to_degrees() } else { value });
    }
    let p: Parameters = serde_json::from_value(json!(values))?;
    p.validate()?;
    Ok(p)
}

pub(crate) fn snapshot(s: &Session) -> Result<Snapshot> {
    for feature in &s.model.results {
        if let Some(error) = &feature.error {
            return Err(Error::Verification(format!(
                "feature {}: {error}",
                feature.name
            )));
        }
    }
    let p = parameters(s)?;
    let world = s.world_state();
    let mut bodies = Vec::new();
    for body in &world.bodies {
        let m = body
            .measure()
            .map_err(|e| Error::Verification(e.to_string()))?;
        bodies.push(BodyMeasurement {
            name: body.name.clone(),
            definition: body.definition_name().into(),
            volume_mm3: m.volume,
            min_mm: [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
            max_mm: [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
            centroid_mm: [m.centroid.x, m.centroid.y, m.centroid.z],
        });
    }
    let mut placements = Vec::new();
    for role in ["top_placement", "bottom_placement"] {
        let joint = s
            .doc
            .assembly
            .joints
            .iter()
            .find(|joint| joint.name == role)
            .ok_or_else(|| Error::Verification(format!("missing joint {role}")))?;
        let occurrence = s
            .doc
            .occurrences
            .iter()
            .find(|o| o.id == joint.b.occurrence)
            .ok_or_else(|| Error::Verification(format!("missing occurrence for {role}")))?;
        let origin = solidcraft_engine::doc::apply_point(
            &occurrence.transform,
            solidcraft_engine::geom::Vec3::ZERO,
        );
        placements.push(Placement {
            role: role.into(),
            occurrence_id: occurrence.id,
            component_id: occurrence.component,
            origin_mm: [origin.x, origin.y, origin.z],
        });
    }
    let device = s
        .doc
        .components
        .iter()
        .find(|c| c.name == "Device")
        .ok_or_else(|| Error::Verification("missing Device definition".into()))?;
    Ok(Snapshot {
        solidcraft_revision: SOLIDCRAFT_REVISION.into(),
        parameters: p,
        bodies,
        placements,
        feature_count: s.doc.features.len(),
        shared_package_occurrences: s
            .doc
            .occurrences
            .iter()
            .filter(|o| o.component == device.id)
            .count(),
    })
}

pub(crate) fn verify(s: &Session, expected: &Parameters) -> Result<()> {
    let got = snapshot(s)?;
    if got.bodies.len() != 3 || got.shared_package_occurrences != 2 {
        return Err(Error::Verification(
            "expected board and two occurrences of one package definition".into(),
        ));
    }
    for (name, value) in expected.values() {
        let actual = got
            .parameters
            .values()
            .get(name)
            .copied()
            .ok_or_else(|| Error::Verification(format!("missing {name}")))?;
        near(name, actual, value)?;
    }
    let board = got
        .bodies
        .iter()
        .find(|b| b.definition == "Substrate")
        .ok_or_else(|| Error::Verification("missing substrate".into()))?;
    let area = expected.board_width * expected.board_depth;
    let hole = std::f64::consts::PI * expected.hole_radius.powi(2);
    near(
        "substrate volume",
        board.volume_mm3,
        (area - hole) * expected.board_thickness,
    )?;
    vector("substrate min", board.min_mm, [0.0, 0.0, 0.0])?;
    vector(
        "substrate max",
        board.max_mm,
        [
            expected.board_width,
            expected.board_depth,
            expected.board_thickness,
        ],
    )?;
    vector(
        "substrate centroid",
        board.centroid_mm,
        [
            (area * expected.board_width / 2.0 - hole * expected.hole_x) / (area - hole),
            (area * expected.board_depth / 2.0 - hole * expected.hole_y) / (area - hole),
            expected.board_thickness / 2.0,
        ],
    )?;
    for top in [true, false] {
        let body = got
            .bodies
            .iter()
            .find(|b| b.definition == "Package" && (b.centroid_mm[2] > 0.0) == top)
            .ok_or_else(|| {
                Error::Verification(format!(
                    "missing {} package",
                    if top { "top" } else { "bottom" }
                ))
            })?;
        near(
            "package volume",
            body.volume_mm3,
            expected.package_length * expected.package_width * expected.package_height,
        )?;
        let (lo, hi) = package_bounds(expected, top);
        vector("package min", body.min_mm, lo)?;
        vector("package max", body.max_mm, hi)?;
    }
    let top = got
        .placements
        .iter()
        .find(|p| p.role == "top_placement")
        .ok_or_else(|| Error::Verification("missing top placement".into()))?;
    let bottom = got
        .placements
        .iter()
        .find(|p| p.role == "bottom_placement")
        .ok_or_else(|| Error::Verification("missing bottom placement".into()))?;
    if top.component_id != bottom.component_id || top.occurrence_id == bottom.occurrence_id {
        return Err(Error::Verification(
            "package definition/occurrence identity is incorrect".into(),
        ));
    }
    vector(
        "top origin",
        top.origin_mm,
        [expected.top_x, expected.top_y, expected.board_thickness],
    )?;
    vector(
        "bottom origin",
        bottom.origin_mm,
        [expected.bottom_x, expected.bottom_y, 0.0],
    )?;
    Ok(())
}

fn near(label: &str, actual: f64, expected: f64) -> Result<()> {
    let tolerance = 1e-5 + expected.abs() * 1e-6;
    if !actual.is_finite() || (actual - expected).abs() > tolerance {
        return Err(Error::Verification(format!(
            "{label}: expected {expected}, got {actual} (tolerance {tolerance})"
        )));
    }
    Ok(())
}

fn vector(label: &str, actual: [f64; 3], expected: [f64; 3]) -> Result<()> {
    for (a, e) in actual.into_iter().zip(expected) {
        near(label, a, e)?;
    }
    Ok(())
}

// Analytic rotated-box bounds, independent of the CAD kernel and joint solver.
fn package_bounds(p: &Parameters, top: bool) -> ([f64; 3], [f64; 3]) {
    let (tx, ty, angle, tz, sign) = if top {
        (p.top_x, p.top_y, p.top_angle, p.board_thickness, 1.0)
    } else {
        (p.bottom_x, p.bottom_y, p.bottom_angle, 0.0, -1.0)
    };
    let (sin, cos) = angle.to_radians().sin_cos();
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for x in [0.25, 0.25 + p.package_length] {
        for y in [-0.5, -0.5 + p.package_width] {
            for z in [0.3, 0.3 + p.package_height] {
                let world = [
                    tx + cos * x - sin * y * sign,
                    ty + sin * x + cos * y * sign,
                    tz + z * sign,
                ];
                for ((min, max), v) in lo.iter_mut().zip(hi.iter_mut()).zip(world) {
                    *min = min.min(v);
                    *max = max.max(v);
                }
            }
        }
    }
    (lo, hi)
}
