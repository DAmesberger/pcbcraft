//! Package datums on the board (slice B2): a datum a package declares in
//! its model frame ([`PackageDatum`]), placed with its instance into the
//! board frame as [`DatumGeometry`] in millimetres, x and y on the board,
//! z up from the bottom face of the lowest dielectric (the stackup's).
//!
//! A part sits as the 3D board holds it (`layout::placement_transform` in
//! the plane, the seating plane on the top of its side's land copper): the
//! model frame moves by the package's model placement into the package
//! datum frame, turns over on the bottom (a rigid half turn about x), turns
//! by the placement angle and moves to the placement's position and its
//! side's seating height. Selection and the camera never reach here; the
//! geometry is derived from the document each time it is needed and never
//! stored.
use super::{DatumGeometry, Frame, Vec3};
use crate::diagnostics::{Error, Result};
use crate::geometry::Transform3;
use crate::ids::Id;
use crate::instances;
use crate::library::datum::{DatumName, DatumProfile, PackageDatum, PackageDatumGeometry};
use crate::library::{Outline, PackageDefinition, Segment};

use crate::model::{CopperLayer, Layer, Placement, Project, Stackup};
use crate::units::Nm;

/// A package datum of one placed instance, in the board frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedDatum {
    pub instance: Id,
    pub name: DatumName,
    /// Marked for the enclosure (the `external_datums` rule exports it).
    pub external: bool,
    pub clearance_mm: Option<f64>,
    pub geometry: DatumGeometry,
}

/// Model frame → board frame of one placement, mm.
///
/// Equal to the footprint-frame chain the 2D drawing and the 3D board use
/// (`library::model_frame` then `layout::placement_transform`, the seating
/// plane at its side's land copper): the footprint datum that chain applies
/// and undoes cancels, leaving the model placement in the datum frame.
/// Refused when the stackup has no copper on the placement's side.
pub fn model_to_board(
    placement: &Placement,
    package: &PackageDefinition,
    stackup: &Stackup,
) -> Result<Transform3> {
    let (layer, seat) = match placement.layer {
        Layer::Top => (CopperLayer::Top, 1),
        Layer::Bottom => (CopperLayer::Bottom, 0),
    };
    let span = stackup.copper_span(layer).ok_or_else(|| {
        Error::Invalid(format!(
            "interface.datum: the stackup has no {} copper to seat a part on",
            layer.name()
        ))
    })?;
    let z = if seat == 1 { span.1 } else { span.0 };
    let side = match placement.layer {
        Layer::Top => Transform3::IDENTITY,
        Layer::Bottom => Transform3::rot_x(180.0),
    };
    let [x, y] = placement.position;
    Ok(Transform3::translation([x, y, z])
        .then(&Transform3::rot_z(placement.rotation_deg))
        .then(&side)
        .then(&package.placement.transform()))
}

/// `datum` placed by `to_board` (model → board).
pub fn placed_geometry(datum: &PackageDatum, to_board: &Transform3) -> DatumGeometry {
    let point = |p: [Nm; 3]| Vec3(to_board.apply(p.map(Nm::to_mm_f64)));
    let vector = |v: [f64; 3]| Vec3(to_board.apply_vector(v));
    let frame = |f: &crate::library::datum::DatumFrame| Frame {
        origin: point(f.origin_nm),
        x: vector(f.x),
        y: vector(f.y),
    };
    match &datum.geometry {
        PackageDatumGeometry::Frame { frame: f } => DatumGeometry::Frame { frame: frame(f) },
        PackageDatumGeometry::Plane { origin_nm, normal } => DatumGeometry::Plane {
            origin: point(*origin_nm),
            normal: vector(*normal),
        },
        PackageDatumGeometry::Axis {
            point_nm,
            direction,
            diameter_nm,
        } => DatumGeometry::Axis {
            point: point(*point_nm),
            direction: vector(*direction),
            diameter: diameter_nm.map(Nm::to_mm_f64),
        },
        PackageDatumGeometry::Profile { frame: f, profile } => DatumGeometry::Profile {
            frame: frame(f),
            loops: vec![outline(profile)],
        },
        PackageDatumGeometry::Volume {
            frame: f,
            profile,
            height_nm,
        } => DatumGeometry::Volume {
            frame: frame(f),
            loops: vec![outline(profile)],
            height: height_nm.to_mm_f64(),
        },
    }
}

/// A profile as one counter-clockwise loop in its frame's x–y plane,
/// centred on the origin: lines and quarter arcs, without zero-length
/// lines (a stadium has two straight sides).
pub fn outline(profile: &DatumProfile) -> Outline {
    let ([w, h], r) = profile.rect_mm();
    let (hx, hy) = (w / 2.0, h / 2.0);
    // tan(90° / 4): a quarter arc in bulge form.
    let quarter = (std::f64::consts::PI / 8.0).tan();
    let start = [-hx + r, -hy];
    let corners = [
        ([hx - r, -hy], [hx, -hy + r]),
        ([hx, hy - r], [hx - r, hy]),
        ([-hx + r, hy], [-hx, hy - r]),
        ([-hx, -hy + r], start),
    ];
    let mut segments = Vec::with_capacity(8);
    let mut at = start;
    for (line_to, arc_to) in corners {
        if line_to != at {
            segments.push(Segment::Line { to: line_to });
        }
        if r > 0.0 {
            segments.push(Segment::Arc {
                to: arc_to,
                bulge: quarter,
            });
            at = arc_to;
        } else {
            at = line_to;
        }
    }
    if at != start {
        segments.push(Segment::Line { to: start });
    }
    Outline { start, segments }
}

/// The datums `instance`'s package declares, in the board frame, in the
/// package's order. Refused when the instance is not on the board or has
/// no footprint and package; an instance whose package declares none has
/// none (nothing is inferred from its name or category).
pub fn instance_datums(project: &Project, instance: Id) -> Result<Vec<PlacedDatum>> {
    let lost = |what: &str| Error::NotFound(format!("interface.datum: {what}"));
    let placement = project
        .placement_of(instance)
        .ok_or_else(|| lost("the part is not on the board"))?;
    let package = package_of(project, instance).ok_or_else(|| lost("the part has no package"))?;
    let to_board = model_to_board(placement, package, &project.stackup)?;
    Ok(package
        .datums
        .iter()
        .map(|d| PlacedDatum {
            instance,
            name: d.name.clone(),
            external: d.external,
            clearance_mm: d.clearance_nm.map(Nm::to_mm_f64),
            geometry: placed_geometry(d, &to_board),
        })
        .collect())
}

/// Every placed instance's datums marked `external`, by instance in board
/// order: what the `external_datums` rule exports. Instances that do not
/// resolve to a package are skipped (their own findings report them).
pub fn external_datums(project: &Project) -> Vec<PlacedDatum> {
    project
        .placements
        .iter()
        .filter_map(|p| instance_datums(project, p.instance).ok())
        .flatten()
        .filter(|d| d.external)
        .collect()
}

fn package_of(project: &Project, instance: Id) -> Option<&PackageDefinition> {
    let instance = project.instance(instance)?;
    let physical = instances::resolve(&project.library, instance)
        .ok()?
        .physical?;
    Some(&physical.package.definition)
}

/// The datums `instance`'s package declares, in the board frame, for the
/// automatic rules, `interface.export` and `interface.list`: empty when the
/// part is not placed, has no package or cannot be seated (those report
/// themselves through their own findings).
pub fn placed(project: &Project, instance: Id) -> Vec<PlacedDatum> {
    instance_datums(project, instance).unwrap_or_default()
}
