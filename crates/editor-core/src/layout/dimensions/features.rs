//! Feature geometry for the parts solve, rebuilt from the library snapshot
//! for every solve and never stored (§5.2, §5.4): a target's origin and
//! axes, pad centres and sides, courtyard sides and centre, and the
//! anchors a package names. Everything is in the target's frame: the part
//! frame (footprint moved to its datum, before the placement mirrors and
//! turns it), or a fill's or keepout's outline start.
//!
//! A feature whose source is gone (a deleted part, pad or anchor) is
//! `Error::NotFound`, which the caller reports as a lost reference. 3D
//! model faces, edges and vertices are the host's traces, kept in the
//! document (`super::model`); one that does not resolve is `NotFound` too.
use super::FeatureGeometry;
use crate::diagnostics::{Error, Result};
use crate::geometry::{Bounds, Point, Transform2};
use crate::ids::Id;
use crate::instances;
use crate::library::graphics::{DrawingLayer, Item};
use crate::library::marking::SurfaceGeometry;
use crate::library::{Datum, FootprintDefinition, PAD_ARC_TOLERANCE_MM, PackageDefinition};
use crate::model::Project;
use crate::model::board::ConstraintTarget;
use crate::model::dimension::{BoxSide, FeatureRef, PadPoint, PartAxis};

/// Length of the frame axes a feature reports, mm.
pub const AXIS_MM: f64 = 10.0;
/// Chord tolerance of courtyard and pad boxes, mm.
const CHORD_MM: f64 = 0.001;
/// How close to vertical (or horizontal) a 3D direction counts as one.
const SQUARE: f64 = 1e-6;

fn lost(what: String) -> Error {
    Error::NotFound(format!("layout.dimension_reference: {what}"))
}

/// Whether `target` is on the board.
pub fn target_exists(project: &Project, target: ConstraintTarget) -> bool {
    match target {
        ConstraintTarget::Placement(id) => project.placement_of(id).is_some(),
        ConstraintTarget::Fill(id) => project.fill(id).is_some(),
        ConstraintTarget::Keepout(id) => project.keepouts.iter().any(|k| k.id == id),
    }
}

/// `feature`'s geometry in its target's frame.
pub fn geometry(project: &Project, feature: &FeatureRef) -> Result<FeatureGeometry> {
    let target = feature.target();
    if !target_exists(project, target) {
        return Err(lost(
            "the dimensioned object is no longer on the board".into(),
        ));
    }
    match feature {
        FeatureRef::Origin { .. } => Ok(FeatureGeometry::Point { at: [0.0, 0.0] }),
        FeatureRef::Axis { axis, .. } => Ok(FeatureGeometry::Line {
            a: [0.0, 0.0],
            b: match axis {
                PartAxis::X => [AXIS_MM, 0.0],
                PartAxis::Y => [0.0, AXIS_MM],
            },
        }),
        FeatureRef::Pad { placement, pad, at } => {
            let (footprint, _) = physical(project, *placement)?;
            let pad = footprint
                .pads
                .iter()
                .find(|p| p.id == *pad)
                .ok_or_else(|| lost(format!("the part has no pad {}", pad.0)))?;
            let to_part = part_frame(&footprint.datum);
            match at {
                PadPoint::Centre => Ok(FeatureGeometry::Point {
                    at: to_part.apply(pad.position()),
                }),
                PadPoint::Side(side) => {
                    let frame = Transform2 {
                        translate: pad.position(),
                        rotation_deg: pad.rotation_deg(),
                        mirror: false,
                    }
                    .then(&to_part);
                    let ring = pad
                        .shape
                        .outline(PAD_ARC_TOLERANCE_MM)
                        .transformed(&frame)
                        .flatten(CHORD_MM);
                    let b =
                        Bounds::of(ring).ok_or_else(|| lost("the pad has no outline".into()))?;
                    Ok(box_side(b, *side))
                }
            }
        }
        FeatureRef::Courtyard { placement, side } => {
            let b = extent(project, *placement)?;
            Ok(match side {
                None => FeatureGeometry::Point {
                    at: [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0],
                },
                Some(side) => box_side(b, *side),
            })
        }
        FeatureRef::Anchor { placement, name } => {
            let (_, package) = physical(project, *placement)?;
            let anchor = crate::library::marking::surface::package_anchor(package, &name.0)
                .ok_or_else(|| lost(format!("the part has no anchor ‘{}’", name.0)))?;
            anchor_geometry(&anchor.geometry)
        }
        FeatureRef::Face { .. } | FeatureRef::Edge { .. } | FeatureRef::Vertex { .. } => {
            super::model::geometry(project, feature)
        }
    }
}

/// The placed part's footprint and package.
fn physical(
    project: &Project,
    placement: Id,
) -> Result<(&FootprintDefinition, &PackageDefinition)> {
    let instance = project
        .instance(placement)
        .ok_or_else(|| lost("the part is gone".into()))?;
    let physical = instances::resolve(&project.library, instance)
        .ok()
        .and_then(|r| r.physical)
        .ok_or_else(|| lost("the part has no footprint".into()))?;
    Ok((&physical.footprint.definition, &physical.package.definition))
}

/// Footprint-local to part frame: the datum to the origin, its rotation
/// undone (as `layout::placed::placement_transform` before the placement).
fn part_frame(datum: &Datum) -> Transform2 {
    Transform2::translation([-datum.offset()[0], -datum.offset()[1]]).then(&Transform2 {
        translate: [0.0, 0.0],
        rotation_deg: -datum.rotation_deg(),
        mirror: false,
    })
}

/// The part's extent in its frame: its closed courtyard shapes, or its
/// pads' copper when it has none.
fn extent(project: &Project, placement: Id) -> Result<Bounds> {
    let (footprint, _) = physical(project, placement)?;
    let to_part = part_frame(&footprint.datum);
    let courtyard: Vec<Point> = footprint
        .graphics
        .iter()
        .filter(|g| g.layer == DrawingLayer::Courtyard)
        .filter_map(|g| match &g.item {
            Item::Shape { shape, .. } if shape.is_closed() => {
                Some(shape.to_mm().transformed(&to_part).flatten(CHORD_MM))
            }
            _ => None,
        })
        .flatten()
        .collect();
    if let Some(b) = Bounds::of(courtyard) {
        return Ok(b);
    }
    let pads = footprint.pads.iter().flat_map(|pad| {
        let frame = Transform2 {
            translate: pad.position(),
            rotation_deg: pad.rotation_deg(),
            mirror: false,
        }
        .then(&to_part);
        pad.shape
            .outline(PAD_ARC_TOLERANCE_MM)
            .transformed(&frame)
            .flatten(CHORD_MM)
    });
    Bounds::of(pads).ok_or_else(|| lost("the part has no courtyard and no pads".into()))
}

/// A side of a box as a line, counter-clockwise round the box (bottom left
/// to right, right upwards, top right to left, left downwards).
fn box_side(b: Bounds, side: BoxSide) -> FeatureGeometry {
    let [x0, y0] = b.min;
    let [x1, y1] = b.max;
    let (a, b) = match side {
        BoxSide::Bottom => ([x0, y0], [x1, y0]),
        BoxSide::Right => ([x1, y0], [x1, y1]),
        BoxSide::Top => ([x1, y1], [x0, y1]),
        BoxSide::Left => ([x0, y1], [x0, y0]),
    };
    FeatureGeometry::Line { a, b }
}

/// An anchor's trace in the board plane (§5.4): a face parallel to the
/// board is its centre, a face square to it its line, a cylinder standing
/// on it its circle. Anything sloped is refused.
fn anchor_geometry(geometry: &SurfaceGeometry) -> Result<FeatureGeometry> {
    let sloped = || {
        Err(Error::Unsupported(
            "layout.dimension: this anchor is sloped; pick an edge of its face".into(),
        ))
    };
    match geometry {
        SurfaceGeometry::Plane(area) => {
            let [nx, ny, nz] = area.normal;
            let [ox, oy, _] = area.origin();
            if (nz.abs() - 1.0).abs() <= SQUARE {
                return Ok(FeatureGeometry::Point { at: [ox, oy] });
            }
            let flat = nx.hypot(ny);
            if nz.abs() > SQUARE || flat <= SQUARE {
                return sloped();
            }
            // Its horizontal extent: the chart axis that lies flat.
            let width = if area.u_axis[2].abs() <= SQUARE {
                area.size()[0]
            } else {
                area.size()[1]
            };
            let h = [-ny / flat * width / 2.0, nx / flat * width / 2.0];
            Ok(FeatureGeometry::Line {
                a: [ox - h[0], oy - h[1]],
                b: [ox + h[0], oy + h[1]],
            })
        }
        SurfaceGeometry::Cylinder(c) => {
            let [ax, ay, az] = c.axis;
            let len = (ax * ax + ay * ay + az * az).sqrt();
            if !(len > SQUARE && (az.abs() / len - 1.0).abs() <= SQUARE) {
                return sloped();
            }
            let [x, y, _] = c.base();
            Ok(FeatureGeometry::Circle {
                center: [x, y],
                radius: c.radius(),
            })
        }
    }
}
