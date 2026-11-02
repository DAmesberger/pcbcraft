//! Marking decals placed on built models: each marked instance's top face
//! found in the built display geometry (so it follows every model family,
//! generated or envelope, and every placement rule of the board build),
//! its decal taken from the [`DecalCache`] and placed there as display
//! geometry with the instance's pick identity, so it hides with the
//! component bodies and highlights with its part.
//!
//! The marked face is the outward flat face of the instance's outermost
//! package body (the moulding, ceramic or overcoat; never a lead, a
//! termination or solder): the top for a part on the top side, the
//! underside's outer face for one on the bottom. Its u axis is the
//! package +x as placed, turned by the placement angle.
use super::{DecalCache, DecalMesh, band_surface, finish_detail, finish_surface, place};
use crate::material::{BodyKind, Detail};
use crate::project::Source;
use crate::scene::Geometry;
use crate::{Error, Result};
use pcbcraft_core::geometry::cos_sin;
use pcbcraft_core::ids::{ContentHash, Id};
use pcbcraft_core::library::marking::MarkingLayout;
use pcbcraft_core::library::model3d::MarkingArea;
use pcbcraft_core::{Layer, Project};
use solidcraft_engine::geom::{Mesh, Vec3};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The body kind decals are drawn as: a display-only appearance layer
/// with its own view toggle (Markings), hidden with the bodies it sits on.
pub const DECAL_KIND: BodyKind = BodyKind::Marking;
/// A face is flat when its triangles' normals are this close to the
/// face normal (cosine).
const FLAT_COS: f64 = 0.999;
/// Vertices this close to the outermost height lie on the face, mm.
const FACE_EPS_MM: f64 = 1e-4;
/// Media type of SVG artwork assets.
const SVG_MEDIA_TYPE: &str = pcbcraft_core::library::marking::ARTWORK_MEDIA_TYPE;

/// Bodies a marking is printed on.
fn printable(kind: BodyKind) -> bool {
    matches!(
        kind,
        BodyKind::PackageBody
            | BodyKind::ChipResistor
            | BodyKind::ChipCapacitor
            | BodyKind::PartBody
            | BodyKind::ModelBody
            | BodyKind::Overcoat
    )
}

/// The marked face of `bodies` facing `up` (+z or −z), with `u` (in plan)
/// as its u axis: the flat face of the outermost body at its outermost
/// height, sized by its extent along u and `normal × u`.
pub fn face_area(bodies: &[&Geometry], up: bool, u: [f64; 2]) -> Option<MarkingArea> {
    let s = if up { 1.0 } else { -1.0 };
    // The outermost reach along the face normal.
    let outer = bodies
        .iter()
        .max_by(|a, b| (s * reach(a, up)).total_cmp(&(s * reach(b, up))))?;
    let height = reach(outer, up);
    let n = Vec3::new(0.0, 0.0, s);
    let mesh = outer.world_mesh();
    let mut on: Vec<Vec3> = Vec::new();
    for t in &mesh.triangles {
        let Some(p) = mesh.tri(t) else {
            continue;
        };
        if p.iter().any(|q| (q.z - height).abs() > FACE_EPS_MM) {
            continue;
        }
        let face = (p[1] - p[0]).cross(p[2] - p[0]);
        let len = face.dot(face).sqrt();
        if len > 0.0 && face.dot(n) / len >= FLAT_COS {
            on.extend(p);
        }
    }
    let ul = u[0].hypot(u[1]);
    if on.is_empty() || !(ul.is_finite() && ul > 0.0) {
        return None;
    }
    let uu = Vec3::new(u[0] / ul, u[1] / ul, 0.0);
    let vv = n.cross(uu);
    let span = |axis: Vec3| {
        on.iter()
            .map(|p| p.dot(axis))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), d| {
                (lo.min(d), hi.max(d))
            })
    };
    let (u0, u1) = span(uu);
    let (v0, v1) = span(vv);
    let size = [u1 - u0, v1 - v0];
    if !(size[0] > 1e-6 && size[1] > 1e-6) {
        return None;
    }
    let centre = uu * ((u0 + u1) / 2.0) + vv * ((v0 + v1) / 2.0);
    Some(MarkingArea {
        origin: [centre.x, centre.y, height],
        normal: [n.x, n.y, n.z],
        u_axis: [uu.x, uu.y, uu.z],
        size_mm: size,
    })
}

/// A body's outermost height toward its face: its top, or its bottom.
fn reach(g: &Geometry, up: bool) -> f64 {
    if up { g.max[2] } else { g.min[2] }
}

/// A placed decal as display geometry: one body in the finish and one per
/// band colour, each with the pick identity `item`.
pub fn decal_geometry(mesh: &DecalMesh, item: Option<u64>) -> Result<Vec<Geometry>> {
    let triangle = |k: usize| -> Option<[u32; 3]> {
        Some([
            *mesh.indices.get(3 * k)?,
            *mesh.indices.get(3 * k + 1)?,
            *mesh.indices.get(3 * k + 2)?,
        ])
    };
    let count = mesh.triangle_count();
    let mut banded = vec![None::<u8>; count];
    for run in &mesh.bands {
        let end = (run.indices.end / 3).min(count);
        for slot in banded.iter_mut().take(end).skip(run.indices.start / 3) {
            *slot = Some(run.colour);
        }
    }
    let mut groups: BTreeMap<Option<u8>, Vec<[u32; 3]>> = BTreeMap::new();
    for (k, colour) in banded.iter().enumerate() {
        if let Some(t) = triangle(k) {
            groups.entry(*colour).or_default().push(t);
        }
    }
    let normal = Vec3::new(
        f64::from(mesh.normal[0]),
        f64::from(mesh.normal[1]),
        f64::from(mesh.normal[2]),
    );
    let mut out = Vec::with_capacity(groups.len());
    for (colour, triangles) in groups {
        let (surface, detail) = match colour {
            None => (finish_surface(mesh.finish)?, finish_detail(mesh.finish)?),
            Some(c) => (band_surface(c)?, Detail::NONE),
        };
        // Each body keeps only the vertices its triangles use.
        let mut remap: BTreeMap<u32, u32> = BTreeMap::new();
        let mut positions: Vec<Vec3> = Vec::new();
        let mut local = Vec::with_capacity(triangles.len());
        for t in triangles {
            let mut l = [0u32; 3];
            for (slot, i) in l.iter_mut().zip(t) {
                let next = u32::try_from(positions.len())
                    .map_err(|_| Error::Invalid("marking decal: too many vertices".into()))?;
                *slot = match remap.get(&i) {
                    Some(j) => *j,
                    None => {
                        let p = mesh.positions.get(i as usize).ok_or_else(|| {
                            Error::Invalid("marking decal: a triangle index is out of range".into())
                        })?;
                        positions.push(Vec3::new(
                            f64::from(p[0]),
                            f64::from(p[1]),
                            f64::from(p[2]),
                        ));
                        remap.insert(i, next);
                        next
                    }
                };
            }
            local.push(l);
        }
        let display = Mesh {
            normals: vec![normal; positions.len()],
            tri_face: vec![0; local.len()],
            positions,
            triangles: local,
            display_only_edges: true,
            ..Mesh::default()
        };
        let bounds = display.bounds();
        out.push(Geometry {
            item,
            kind: DECAL_KIND,
            layer: None,
            surface,
            detail,
            mesh: Arc::new(display),
            place: None,
            volume: crate::scene::Volume::known(0.0),
            min: [bounds.min.x, bounds.min.y, bounds.min.z],
            max: [bounds.max.x, bounds.max.y, bounds.max.z],
        });
    }
    Ok(out)
}

/// SVG artwork from the project's packed assets.
pub fn project_artwork(project: &Project) -> impl Fn(&ContentHash) -> Option<String> + '_ {
    move |hash| {
        project
            .library
            .assets
            .iter()
            .find(|a| a.hash == *hash && a.media_type == SVG_MEDIA_TYPE)
            .and_then(|a| {
                std::str::from_utf8(a.data.as_slice())
                    .map(str::to_owned)
                    .ok()
            })
    }
}

/// Whether any placed instance of `project` can have a marking: its
/// component or Part carries a spec, or the instance replaces it.
pub fn any_marked(project: &Project) -> bool {
    project.placements.iter().any(|placement| {
        let Some(instance) = project.instance(placement.instance) else {
            return false;
        };
        if instance.marking.hide {
            return false;
        }
        if instance.marking.replace.is_some() {
            return true;
        }
        let component = project
            .library
            .component(instance.component)
            .is_some_and(|c| c.definition.marking.is_some());
        let part = instance
            .part
            .and_then(|p| project.library.part(p))
            .is_some_and(|p| p.definition.top_mark.is_some());
        component || part
    })
}

/// The placed instances' resolved marking layouts (`instance.marking`),
/// and the instances whose marking could not be laid out, with why.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstanceLayouts {
    /// Instances with something to draw.
    pub layouts: BTreeMap<Id, MarkingLayout>,
    /// Instances whose marking the core refused to lay out; they show no
    /// marking. Never silent: the worker reports them.
    pub failures: Vec<(Id, String)>,
}

/// Each placed instance's resolved marking layout, read straight from the
/// valid `project` through the core's pure `instance.marking`
/// ([`pcbcraft_core::commands::instance_marking`]): no project copy, no
/// engine and no revalidation of the library or its packed assets. Only
/// when [`any_marked`] says there is something to read.
pub fn instance_layouts(project: &Project) -> InstanceLayouts {
    let mut out = InstanceLayouts::default();
    if !any_marked(project) {
        return out;
    }
    for placement in &project.placements {
        let instance = placement.instance;
        match pcbcraft_core::commands::instance_marking(project, instance) {
            Ok(Some(layout)) if !layout.elements.is_empty() => {
                out.layouts.insert(instance, layout);
            }
            Ok(_) => {}
            Err(error) => out.failures.push((instance, error.to_string())),
        }
    }
    out
}

/// The decals of a built board: one per placed instance with a layout in
/// `layouts`, on its marked face, as display geometry carrying the
/// instance's pick identity. An instance whose face or decal cannot be
/// made shows no marking. Unused cache entries are swept.
pub fn board_decals(
    project: &Project,
    geometry: &[Geometry],
    sources: &[Source],
    layouts: &BTreeMap<Id, MarkingLayout>,
    cache: &mut DecalCache,
) -> Vec<Geometry> {
    let artwork = project_artwork(project);
    let mut out = Vec::new();
    for placement in &project.placements {
        let Some(layout) = layouts.get(&placement.instance) else {
            continue;
        };
        // The instance's body picks: its model or envelope, not its pads.
        let picks: BTreeSet<u64> = sources
            .iter()
            .filter(|s| {
                s.instance == Some(placement.instance)
                    && s.pad.is_none()
                    && s.track.is_none()
                    && s.via.is_none()
            })
            .map(|s| s.pick_id)
            .collect();
        let bodies: Vec<&Geometry> = geometry
            .iter()
            .filter(|g| printable(g.kind) && g.item.is_some_and(|i| picks.contains(&i)))
            .collect();
        let (c, s) = cos_sin(placement.rotation_deg);
        let Some(area) = face_area(&bodies, placement.layer == Layer::Top, [c, s]) else {
            continue;
        };
        let item = bodies.iter().find_map(|g| g.item);
        let placed = cache
            .flat(placement.instance, layout, area.size_mm, &artwork)
            .and_then(|flat| place(&flat, &area))
            .and_then(|mesh| decal_geometry(&mesh, item));
        if let Ok(bodies) = placed {
            out.extend(bodies);
        }
    }
    cache.sweep();
    out
}

/// The decal of `layout` on a package preview (Component 3D): on the top
/// face of the preview's outermost package body, +x along the package.
pub fn preview_decal(
    layout: &MarkingLayout,
    geometry: &[Geometry],
    artwork: super::ArtworkSource<'_>,
) -> Result<Vec<Geometry>> {
    let bodies: Vec<&Geometry> = geometry.iter().filter(|g| printable(g.kind)).collect();
    let area = face_area(&bodies, true, [1.0, 0.0]).ok_or_else(|| {
        Error::Invalid("marking decal: the model has no flat top face to mark".into())
    })?;
    let item = bodies.iter().find_map(|g| g.item);
    let mesh = super::decal_with(layout, &area, artwork)?;
    decal_geometry(&mesh, item)
}

/// The marking decals of successive builds of one board, for the model
/// worker: it keeps the per-instance [`DecalCache`] between builds, so a
/// rebuild after a value change refills only the changed instance's decal.
/// The decals are appended to the built display geometry; they never enter
/// the native model or its saved file.
#[derive(Default)]
pub struct BoardMarkings {
    cache: DecalCache,
    failures: Vec<(Id, String)>,
}

impl BoardMarkings {
    /// The decals of `project` on its built `geometry` (see
    /// [`board_decals`]); nothing, quickly, when nothing is marked.
    pub fn decals(
        &mut self,
        project: &Project,
        geometry: &[Geometry],
        sources: &[Source],
    ) -> Vec<Geometry> {
        let InstanceLayouts { layouts, failures } = instance_layouts(project);
        self.failures = failures;
        if layouts.is_empty() {
            self.cache.sweep();
            return Vec::new();
        }
        board_decals(project, geometry, sources, &layouts, &mut self.cache)
    }

    /// The instances of the last build whose marking could not be laid
    /// out, with why.
    pub fn failures(&self) -> &[(Id, String)] {
        &self.failures
    }

    /// The cache, for evidence of what was rebuilt.
    pub fn cache(&self) -> &DecalCache {
        &self.cache
    }
}
