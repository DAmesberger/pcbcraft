//! Marking decals placed on built models: each marked instance's surfaces
//! found in the built display geometry or from its package's named
//! anchors, its decals taken from the [`DecalCache`] and placed there as
//! display geometry with the instance's pick identity, so they hide with
//! the component bodies and highlight with their part.
//!
//! Surfaces ([`Placement::on`]):
//! - **Model**: the outward flat face of the instance's outermost package
//!   body (the moulding, ceramic or overcoat; never a lead, a termination
//!   or solder): the top for a part on the top side, the underside's outer
//!   face for one on the bottom; its u axis the package +x as placed;
//! - **a box face** of the outermost body along that face's direction in
//!   the package frame, found in the built mesh, so it follows imported
//!   models too;
//! - **a named anchor** of the package form (`can_top`, `pcb_top`,
//!   `sleeve`, …), exact from the form, moved with the instance's model:
//!   by the placement of its shared definition mesh, which is the model
//!   frame as placed (the package placement, the board joint and side).
use super::map::SurfaceMap;
use super::{DecalCache, DecalMesh, Ink, Region, finish_detail, ink_surface, place_on};
use crate::geom::prelude::*;
use crate::geom::{Mesh, Vec3};
use crate::material::{BodyKind, Detail};
use crate::project::Source;
use crate::scene::{Geometry, Place};
use crate::{Error, Result};
use pcbcraft_core::geometry::cos_sin;
use pcbcraft_core::ids::{ContentHash, Id};
use pcbcraft_core::library::PackageDefinition;
use pcbcraft_core::library::marking::surface::{self, CylinderArea, SurfaceGeometry};
use pcbcraft_core::library::marking::{BoxFace, MarkingLayout, Placement, SurfaceRef};
use pcbcraft_core::library::model3d::MarkingArea;
use pcbcraft_core::{Layer, Project};
use solidcraft_viewport::surface::OpaqueSurface;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The body kind decals are drawn as: a display-only appearance layer
/// with its own view toggle (Markings), hidden with the bodies it sits on.
pub const DECAL_KIND: BodyKind = BodyKind::Marking;
/// A face is flat when its triangles' normals are this close to the
/// face normal (cosine).
const FLAT_COS: f64 = 0.999;
/// Vertices this close to the outermost reach lie on the face, mm.
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

fn vec3(a: [f64; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

fn arr(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// The marked face of `bodies` facing `up` (+z or −z), with `u` (in plan)
/// as its u axis: the flat face of the outermost body at its outermost
/// height, sized by its extent along u and `normal × u`.
pub fn face_area(bodies: &[&Geometry], up: bool, u: [f64; 2]) -> Option<MarkingArea> {
    let s = if up { 1.0 } else { -1.0 };
    face_towards(bodies, [0.0, 0.0, s], [u[0], u[1], 0.0]).map(|(area, _)| area)
}

/// The flat face of `bodies` facing `normal` (a unit vector), with `u` as
/// its u axis (made square to the normal): the face of the outermost body
/// along `normal` at its outermost reach, sized by its extent along u and
/// `normal × u`, with that body's look.
pub fn face_towards(
    bodies: &[&Geometry],
    normal: [f64; 3],
    u: [f64; 3],
) -> Option<(MarkingArea, OpaqueSurface)> {
    let n = vec3(normal);
    let nl = n.dot(n).sqrt();
    if !(nl.is_finite() && nl > 1e-9) {
        return None;
    }
    let n = n * (1.0 / nl);
    let reach = |g: &Geometry| {
        (0..8)
            .map(|k| {
                let c = Vec3::new(
                    if k & 1 == 0 { g.min[0] } else { g.max[0] },
                    if k & 2 == 0 { g.min[1] } else { g.max[1] },
                    if k & 4 == 0 { g.min[2] } else { g.max[2] },
                );
                c.dot(n)
            })
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let outer = bodies.iter().max_by(|a, b| reach(a).total_cmp(&reach(b)))?;
    let height = reach(outer);
    let mesh = outer.world_mesh();
    let mut on: Vec<Vec3> = Vec::new();
    for t in &mesh.triangles {
        let Some(p) = mesh.tri(t) else {
            continue;
        };
        if p.iter().any(|q| (q.dot(n) - height).abs() > FACE_EPS_MM) {
            continue;
        }
        let face = (p[1] - p[0]).cross(p[2] - p[0]);
        let len = face.dot(face).sqrt();
        if len > 0.0 && face.dot(n) / len >= FLAT_COS {
            on.extend(p);
        }
    }
    let u = vec3(u);
    let u = u - n * u.dot(n);
    let ul = u.dot(u).sqrt();
    if on.is_empty() || !(ul.is_finite() && ul > 1e-9) {
        return None;
    }
    let uu = u * (1.0 / ul);
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
    let centre = uu * ((u0 + u1) / 2.0) + vv * ((v0 + v1) / 2.0) + n * height;
    Some((
        MarkingArea::from_mm(arr(centre), arr(n), arr(uu), size).ok()?,
        outer.surface,
    ))
}

/// One instance's model as built: its printable bodies, the frame its
/// model is drawn in (model → world, from a placed definition body), its
/// side and plan u axis, and its package.
pub struct Marked<'a> {
    pub bodies: Vec<&'a Geometry>,
    /// The model frame as placed; `None` falls back to the plan pose.
    pub frame: Option<Place>,
    pub top: bool,
    /// The package +x as placed, in plan.
    pub u: [f64; 2],
    pub package: Option<&'a PackageDefinition>,
}

impl Marked<'_> {
    /// Package frame → world: the definition's placement, else the plan
    /// pose (turned by the placement angle; a bottom part turned over
    /// about x first, as the board mirrors it across x).
    fn to_world(&self, v: [f64; 3]) -> Vec3 {
        match &self.frame {
            Some(place) => place.vector(vec3(v)),
            None => {
                let [c, s] = self.u;
                let (x, y, z) = if self.top {
                    (v[0], v[1], v[2])
                } else {
                    (v[0], -v[1], -v[2])
                };
                Vec3::new(c * x - s * y, s * x + c * y, z)
            }
        }
    }

    fn point(&self, p: [f64; 3]) -> Option<Vec3> {
        self.frame.as_ref().map(|place| place.point(vec3(p)))
    }

    /// The look of the body nearest `p` (for an engraving).
    fn base_near(&self, p: Vec3) -> Option<OpaqueSurface> {
        let gap = |g: &Geometry| {
            let d: f64 = (0..3)
                .map(|k| {
                    let v = [p.x, p.y, p.z][k];
                    (g.min[k] - v).max(v - g.max[k]).max(0.0).powi(2)
                })
                .sum();
            d
        };
        self.bodies
            .iter()
            .min_by(|a, b| gap(a).total_cmp(&gap(b)))
            .map(|g| g.surface)
    }

    /// The surface `placement` names, its map and region in world, and
    /// the face's own look; `None` when the model has no such surface.
    pub fn resolve(
        &self,
        placement: &Placement,
    ) -> Option<(SurfaceMap, Region, Option<OpaqueSurface>)> {
        match &placement.on {
            SurfaceRef::Model => {
                let s = if self.top { 1.0 } else { -1.0 };
                let (area, look) =
                    face_towards(&self.bodies, [0.0, 0.0, s], [self.u[0], self.u[1], 0.0])?;
                Some((
                    SurfaceMap::plane(&area).ok()?,
                    Region::plane(area.size()),
                    Some(look),
                ))
            }
            SurfaceRef::Face { face } => {
                let (n, u) = face.frame();
                let (area, look) =
                    face_towards(&self.bodies, arr(self.to_world(n)), arr(self.to_world(u)))?;
                Some((
                    SurfaceMap::plane(&area).ok()?,
                    Region::plane(area.size()),
                    Some(look),
                ))
            }
            SurfaceRef::Anchor { name } => {
                let found = surface::package_anchor(self.package?, name)?;
                let world = |p: [f64; 3]| match self.point(p) {
                    Some(q) => Some(q),
                    // Unplaced (a preview drawn in its model frame).
                    None if self.frame.is_none() => Some(vec3(p)),
                    None => None,
                };
                let dir = |v: [f64; 3]| match &self.frame {
                    Some(place) => place.vector(vec3(v)),
                    None => vec3(v),
                };
                match found.geometry {
                    SurfaceGeometry::Plane(a) => {
                        let origin = world(a.origin())?;
                        let area = MarkingArea::from_mm(
                            arr(origin),
                            arr(dir(a.normal)),
                            arr(dir(a.u_axis)),
                            a.size(),
                        )
                        .ok()?;
                        let look = self.base_near(origin);
                        Some((
                            SurfaceMap::plane(&area).ok()?,
                            Region::plane(a.size()),
                            look,
                        ))
                    }
                    SurfaceGeometry::Cylinder(c) => {
                        let cylinder = CylinderArea {
                            base_nm: pcbcraft_core::library::model3d::triple_nm(arr(world(
                                c.base(),
                            )?))
                            .ok()?,
                            axis: arr(dir(c.axis)),
                            zero: arr(dir(c.zero)),
                            ..c
                        };
                        let wrap = placement.wrap.unwrap_or_default();
                        let chart = cylinder.chart(&wrap)?;
                        let region = Region::wrap(&cylinder, &chart);
                        let mid = vec3(cylinder.base()) + vec3(cylinder.axis) * chart.mid;
                        let look = self.base_near(mid);
                        Some((SurfaceMap::cylinder(cylinder, chart).ok()?, region, look))
                    }
                }
            }
        }
    }

    /// The decals of `layout` and its further decals on this model,
    /// from `cache`; a decal whose surface is missing or that cannot be
    /// drawn is left out, with why.
    pub fn decals(
        &self,
        layout: &MarkingLayout,
        item: Option<u64>,
        cache: &mut DecalCache,
        artwork: super::ArtworkSource<'_>,
    ) -> (Vec<Geometry>, Vec<String>) {
        let mut out = Vec::new();
        let mut problems = Vec::new();
        for (k, laid) in layout.all().enumerate() {
            if laid.elements.is_empty() {
                continue;
            }
            let Some((map, region, look)) = self.resolve(&laid.placement) else {
                problems.push(format!("decal {k}: the model has no such surface to mark"));
                continue;
            };
            let placed = cache
                .flat_in(laid, &region, artwork)
                .and_then(|flat| place_on(&flat, &map))
                .and_then(|mesh| decal_geometry(&mesh, item, look));
            match placed {
                Ok(bodies) => out.extend(bodies),
                Err(error) => problems.push(format!("decal {k}: {error}")),
            }
        }
        (out, problems)
    }
}

/// A placed decal as display geometry: one body per ink (the finish,
/// band colours, artwork colours, an engraving's walls), each with the
/// pick identity `item`. `base` is the look of the face it sits on.
pub fn decal_geometry(
    mesh: &DecalMesh,
    item: Option<u64>,
    base: Option<OpaqueSurface>,
) -> Result<Vec<Geometry>> {
    let triangle = |k: usize| -> Option<[u32; 3]> {
        Some([
            *mesh.indices.get(3 * k)?,
            *mesh.indices.get(3 * k + 1)?,
            *mesh.indices.get(3 * k + 2)?,
        ])
    };
    let count = mesh.triangle_count();
    let mut inks = vec![Ink::Finish; count];
    for run in &mesh.runs {
        let end = (run.indices.end / 3).min(count);
        for slot in inks.iter_mut().take(end).skip(run.indices.start / 3) {
            *slot = run.ink;
        }
    }
    let mut groups: BTreeMap<Ink, Vec<[u32; 3]>> = BTreeMap::new();
    for (k, ink) in inks.iter().enumerate() {
        if let Some(t) = triangle(k) {
            groups.entry(*ink).or_default().push(t);
        }
    }
    let mut out = Vec::with_capacity(groups.len());
    for (ink, triangles) in groups {
        let surface = ink_surface(ink, mesh.finish, base)?;
        let detail = match ink {
            Ink::Finish | Ink::Srgb(_) => finish_detail(mesh.finish)?,
            Ink::Band(_) | Ink::Wall => Detail::NONE,
        };
        // Each body keeps only the vertices its triangles use.
        let mut remap: BTreeMap<u32, u32> = BTreeMap::new();
        let mut positions: Vec<Vec3> = Vec::new();
        let mut normals: Vec<Vec3> = Vec::new();
        let mut local = Vec::with_capacity(triangles.len());
        for t in triangles {
            let mut l = [0u32; 3];
            for (slot, i) in l.iter_mut().zip(t) {
                let next = u32::try_from(positions.len())
                    .map_err(|_| Error::Invalid("marking decal: too many vertices".into()))?;
                *slot = match remap.get(&i) {
                    Some(j) => *j,
                    None => {
                        let bad = || {
                            Error::Invalid("marking decal: a triangle index is out of range".into())
                        };
                        let p = mesh.positions.get(i as usize).ok_or_else(bad)?;
                        let n = mesh.normals.get(i as usize).ok_or_else(bad)?;
                        positions.push(Vec3::new(
                            f64::from(p[0]),
                            f64::from(p[1]),
                            f64::from(p[2]),
                        ));
                        normals.push(Vec3::new(f64::from(n[0]), f64::from(n[1]), f64::from(n[2])));
                        remap.insert(i, next);
                        next
                    }
                };
            }
            local.push(l);
        }
        let display = Mesh {
            normals,
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
    project_artwork_of(&project.library)
}

/// SVG artwork from a library's packed assets.
pub fn project_artwork_of(
    library: &pcbcraft_core::library::Library,
) -> impl Fn(&ContentHash) -> Option<String> + '_ {
    move |hash| {
        library
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
            Ok(Some(layout)) if !layout.is_blank() => {
                out.layouts.insert(instance, layout);
            }
            Ok(_) => {}
            Err(error) => out.failures.push((instance, error.to_string())),
        }
    }
    out
}

/// The package an instance is built with, if it resolves.
fn package_of(project: &Project, instance: Id) -> Option<&PackageDefinition> {
    let target = project.instance(instance)?;
    let component = project.library.component(target.component)?;
    let choice = target.physical?;
    pcbcraft_core::instances::resolve_physical(&project.library, &component.definition, choice)
        .ok()
        .map(|view| &view.package.definition)
}

/// The decals of a built board: each placed instance with a layout in
/// `layouts`, on its surfaces, as display geometry carrying the
/// instance's pick identity. A decal whose surface or mesh cannot be made
/// is left out and reported. Unused cache entries are swept.
pub fn board_decals(
    project: &Project,
    geometry: &[Geometry],
    sources: &[Source],
    layouts: &BTreeMap<Id, MarkingLayout>,
    cache: &mut DecalCache,
) -> Vec<Geometry> {
    board_decals_reporting(project, geometry, sources, layouts, cache).0
}

/// [`board_decals`] with the instances whose decals were left out, why.
pub fn board_decals_reporting(
    project: &Project,
    geometry: &[Geometry],
    sources: &[Source],
    layouts: &BTreeMap<Id, MarkingLayout>,
    cache: &mut DecalCache,
) -> (Vec<Geometry>, Vec<(Id, String)>) {
    let artwork = project_artwork(project);
    let mut out = Vec::new();
    let mut problems = Vec::new();
    // Bodies by pick, so each instance finds its own without a scan.
    let mut by_pick: BTreeMap<u64, Vec<&Geometry>> = BTreeMap::new();
    for g in geometry.iter().filter(|g| printable(g.kind)) {
        if let Some(item) = g.item {
            by_pick.entry(item).or_default().push(g);
        }
    }
    let mut picks: BTreeMap<Id, BTreeSet<u64>> = BTreeMap::new();
    for s in sources {
        if let Some(instance) = s.instance
            && s.pad.is_none()
            && s.track.is_none()
            && s.via.is_none()
        {
            picks.entry(instance).or_default().insert(s.pick_id);
        }
    }
    for placement in &project.placements {
        let Some(layout) = layouts.get(&placement.instance) else {
            continue;
        };
        // The instance's body picks: its model or envelope, not its pads.
        let bodies: Vec<&Geometry> = picks
            .get(&placement.instance)
            .into_iter()
            .flatten()
            .filter_map(|p| by_pick.get(p))
            .flatten()
            .copied()
            .collect();
        if bodies.is_empty() {
            continue;
        }
        let (c, s) = cos_sin(placement.rotation_deg);
        let marked = Marked {
            frame: bodies.iter().find_map(|g| g.place),
            bodies,
            top: placement.layer == Layer::Top,
            u: [c, s],
            package: package_of(project, placement.instance),
        };
        let item = marked.bodies.iter().find_map(|g| g.item);
        let (decals, why) = marked.decals(layout, item, cache, &artwork);
        out.extend(decals);
        problems.extend(why.into_iter().map(|w| (placement.instance, w)));
    }
    cache.sweep();
    (out, problems)
}

/// The decal of `layout` on a package preview (Component 3D): its
/// surfaces on the preview's bodies in their model frame, +x along the
/// package. Errors when nothing at all could be drawn.
pub fn preview_decal(
    layout: &MarkingLayout,
    geometry: &[Geometry],
    artwork: super::ArtworkSource<'_>,
) -> Result<Vec<Geometry>> {
    let mut cache = DecalCache::default();
    let (out, problems) = preview_decals(layout, geometry, None, artwork, &mut cache);
    if out.is_empty()
        && let Some(problem) = problems.into_iter().next()
    {
        return Err(Error::Invalid(format!("marking decal: {problem}")));
    }
    Ok(out)
}

/// The decals of `layout` and its further decals on a package preview of
/// `package` (Component 3D), from `cache`, and why any were left out. The
/// preview is drawn in its model frame, so anchors need no placement.
pub fn preview_decals(
    layout: &MarkingLayout,
    geometry: &[Geometry],
    package: Option<&PackageDefinition>,
    artwork: super::ArtworkSource<'_>,
    cache: &mut DecalCache,
) -> (Vec<Geometry>, Vec<String>) {
    let bodies: Vec<&Geometry> = geometry.iter().filter(|g| printable(g.kind)).collect();
    let item = bodies.iter().find_map(|g| g.item);
    let marked = Marked {
        frame: bodies.iter().find_map(|g| g.place),
        bodies,
        top: true,
        u: [1.0, 0.0],
        package,
    };
    let out = marked.decals(layout, item, cache, artwork);
    cache.sweep();
    out
}

/// The box face named by `face` exists on every model with bodies; this
/// lists the faces and named anchors a package offers, for the UI and
/// agents.
pub fn surfaces(package: Option<&PackageDefinition>) -> Vec<SurfaceRef> {
    let mut out = vec![SurfaceRef::Model];
    out.extend(BoxFace::ALL.map(|face| SurfaceRef::Face { face }));
    if let Some(package) = package {
        out.extend(
            surface::package_anchors(package)
                .into_iter()
                .map(|a| SurfaceRef::Anchor { name: a.name }),
        );
    }
    out
}

/// The marking decals of successive builds of one board, for the model
/// worker: it keeps the [`DecalCache`] between builds, so a rebuild after
/// a value change refills only the changed decal. The decals are appended
/// to the built display geometry; they never enter the native model or
/// its saved file.
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
        let (out, problems) =
            board_decals_reporting(project, geometry, sources, &layouts, &mut self.cache);
        self.failures.extend(problems);
        out
    }

    /// The instances of the last build whose marking (or one of its
    /// decals) could not be drawn, with why.
    pub fn failures(&self) -> &[(Id, String)] {
        &self.failures
    }

    /// The cache, for evidence of what was rebuilt.
    pub fn cache(&self) -> &DecalCache {
        &self.cache
    }
}
