//! What lies under the pointer in the 3D view, and the hover highlight a
//! tool shows for it (slice S5 of docs/board-sketch-references.md). Both
//! read the cached meshes: a pick is a raycast, a highlight a small block
//! of the hovered face's own triangles (or the edge's polyline) in the
//! selection layer, made again only when what is hovered changes. Nothing
//! here rebuilds geometry.
//!
//! The look follows Solidcraft's pre-highlight (`viewport.rs`,
//! `hover_highlight` and the face hover of its feedback layer, MIT,
//! revision 4f3ac07; see ATTRIBUTION.md): a hovered face is lifted by the
//! theme's `hover_face_lift` and outlined in `hover_edge` at 1.5 points, a
//! hovered edge drawn in `hover_edge` at 2 points.
//!
//! Edges are found as Solidcraft's view pick finds them (`viewport.rs`,
//! `pick`, revision 1f6e28a): any pickable edge of a shown body within a
//! few points of the pointer, nearest first, taken where it is not hidden
//! behind a face in front ([`Viewport::edges_near`]). So an edge is as
//! easy to hit at any zoom, also from just off the body.
use super::Viewport;
use crate::model_scene::next_key;
use solidcraft_viewport_gpu::view::{BlockBuilder, PaletteIndex, SceneBlock, TriClass};
use sparksmith_solidcraft::geom::{Vec3, prelude::*};
use sparksmith_solidcraft::material::BodyKind;
use sparksmith_solidcraft::scene::Geometry;

/// The surface of a body under a screen point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceHit {
    /// The body, by its index in the shown model's geometry.
    pub geometry: usize,
    /// Its pick identity (`None` for the laminate, masks and films).
    pub item: Option<u64>,
    pub kind: BodyKind,
    /// The B-rep face the ray met.
    pub face: usize,
    /// Where it met it, in the board (mm).
    pub point: [f64; 3],
    /// The face's normal there (unit).
    pub normal: [f64; 3],
}

/// A pickable edge near the pointer ([`Viewport::edges_near`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeHit {
    /// The body, by its index in the shown model's geometry.
    pub geometry: usize,
    /// Its pick identity (`None` for the laminate, masks and films).
    pub item: Option<u64>,
    /// The edge, by its index in the body's mesh.
    pub edge: usize,
    /// The edge's point under the pointer ray, in the board (mm).
    pub point: [f64; 3],
    /// How far the edge is from the pointer on screen (points).
    pub distance: f32,
}

/// What a tool highlights under the pointer: a face of a body, or one of
/// its edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hovered {
    Face { geometry: usize, face: usize },
    Edge { geometry: usize, edge: usize },
}

fn arr(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// A body-frame point of `g` in the model.
fn placed(g: &Geometry, p: Vec3) -> Vec3 {
    g.place.as_ref().map_or(p, |place| place.point(p))
}

impl Viewport {
    /// The view's size and where it is, from its last draw.
    fn area(&self) -> Option<egui::Rect> {
        self.rect.filter(|r| r.width() >= 1.0 && r.height() >= 1.0)
    }

    /// The pointer ray through `pos` (screen points), in the board: its
    /// origin and unit direction.
    fn ray_at(&self, pos: egui::Pos2) -> Option<(Vec3, Vec3)> {
        let rect = self.area()?;
        let (w, h) = (f64::from(rect.width()), f64::from(rect.height()));
        let (x, y) = (
            f64::from(pos.x - rect.left()),
            f64::from(pos.y - rect.top()),
        );
        let (origin, direction) = self.camera.ray(x, y, w, h);
        Some(self.model_ray(origin, direction))
    }

    /// The shown body surface under `pos` (screen points), on shown layers.
    pub fn surface_hit(&self, pos: egui::Pos2) -> Option<SurfaceHit> {
        let model = self.model.as_ref()?;
        let (origin, direction) = self.ray_at(pos)?;
        let (d, index, triangle) = model
            .geometry
            .iter()
            .enumerate()
            .filter(|(_, g)| self.layers.shows(g))
            .filter(|(_, g)| super::box_hit(g, origin, direction))
            .filter_map(|(i, g)| g.raycast(origin, direction).map(|(d, t)| (d, i, t)))
            .min_by(|a, b| a.0.total_cmp(&b.0))?;
        let g = model.geometry.get(index)?;
        let face = *g.mesh.tri_face.get(triangle)? as usize;
        let corners = g.mesh.tri(g.mesh.triangles.get(triangle)?)?;
        let n = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
        let n = g.place.as_ref().map_or(n, |p| p.vector(n));
        let n = n.normalized()?;
        let point = origin + direction * d;
        Some(SurfaceHit {
            geometry: index,
            item: g.item,
            kind: g.kind,
            face,
            point: arr(point),
            normal: arr(n),
        })
    }

    /// A board point on screen, from the last draw.
    pub fn to_screen(&self, p: [f64; 3]) -> Option<egui::Pos2> {
        let rect = self.area()?;
        let [x, y, z] = self.display.apply(p);
        let (sx, sy, _) = self.camera.to_screen(
            Vec3::new(x, y, z),
            f64::from(rect.width()),
            f64::from(rect.height()),
            self.bounds.diagonal(),
        )?;
        Some(rect.min + egui::vec2(sx as f32, sy as f32))
    }

    /// Millimetres one screen point spans at board point `p`.
    pub fn mm_per_point(&self, p: [f64; 3]) -> Option<f64> {
        let (right, _, _) = self.camera.basis();
        let a = self.to_screen(p)?;
        let q = [p[0] + right.x, p[1] + right.y, p[2] + right.z];
        let b = self.to_screen(q)?;
        let px = f64::from(a.distance(b));
        (px > 1e-9).then(|| 1.0 / px)
    }

    /// The pickable edges of shown bodies that `keep` accepts within
    /// `reach` screen points of `pos`, nearest first, each at its point
    /// under the pointer ray; an edge hidden there behind a face in front
    /// is left out. At most [`MAX_EDGE_HITS`]. Reads the cached meshes only.
    pub fn edges_near(
        &self,
        pos: egui::Pos2,
        reach: f32,
        keep: impl Fn(&Geometry) -> bool,
    ) -> Vec<EdgeHit> {
        let (Some(model), Some((origin, direction))) = (self.model.as_ref(), self.ray_at(pos))
        else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for (index, g) in model.geometry.iter().enumerate() {
            if !self.layers.shows(g) || !keep(g) || !self.box_near(g, pos, reach) {
                continue;
            }
            let mesh = &g.mesh;
            for (i, polyline) in mesh.edges.iter().enumerate() {
                if !mesh.edge_pickable(i) {
                    continue;
                }
                let mut best: Option<(f32, Vec3, Vec3)> = None;
                for w in polyline.windows(2) {
                    let (p, q) = (placed(g, w[0]), placed(g, w[1]));
                    let (Some(a), Some(b)) = (self.to_screen(arr(p)), self.to_screen(arr(q)))
                    else {
                        continue;
                    };
                    let d = segment_distance(pos, a, b);
                    if d <= reach && best.is_none_or(|(n, _, _)| d < n) {
                        best = Some((d, p, q));
                    }
                }
                if let Some((distance, p, q)) = best {
                    found.push(EdgeHit {
                        geometry: index,
                        item: g.item,
                        edge: i,
                        point: arr(ray_segment_closest(origin, direction, p, q)),
                        distance,
                    });
                }
            }
        }
        found.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        found.truncate(MAX_EDGE_CANDIDATES);
        found.retain(|e| self.in_view(e.point));
        found.truncate(MAX_EDGE_HITS);
        found
    }

    /// Whether body `g`'s box comes within `reach` points of `pos` on
    /// screen (a corner off screen behind the eye keeps it).
    fn box_near(&self, g: &Geometry, pos: egui::Pos2, reach: f32) -> bool {
        let mut area: Option<egui::Rect> = None;
        for k in 0..8 {
            let corner = [
                if k & 1 == 0 { g.min[0] } else { g.max[0] },
                if k & 2 == 0 { g.min[1] } else { g.max[1] },
                if k & 4 == 0 { g.min[2] } else { g.max[2] },
            ];
            let Some(s) = self.to_screen(corner) else {
                return true;
            };
            area = Some(area.map_or(egui::Rect::from_min_max(s, s), |r| {
                r.union(egui::Rect::from_min_max(s, s))
            }));
        }
        area.is_some_and(|r| r.expand(reach).contains(pos))
    }

    /// Whether board point `p` is seen: no shown face lies in front of it
    /// on its own ray, within a point's depth.
    fn in_view(&self, p: [f64; 3]) -> bool {
        let Some(model) = self.model.as_ref() else {
            return false;
        };
        let Some((origin, direction)) = self.to_screen(p).and_then(|s| self.ray_at(s)) else {
            return false;
        };
        let depth = (Vec3::new(p[0], p[1], p[2]) - origin).dot(direction);
        let slack = self.mm_per_point(p).unwrap_or(0.0).max(1e-3);
        !model
            .geometry
            .iter()
            .filter(|g| self.layers.shows(g))
            .filter(|g| super::box_hit(g, origin, direction))
            .filter_map(|g| g.raycast(origin, direction).map(|(d, _)| d))
            .any(|d| d < depth - slack)
    }

    /// The edge of the hit face nearest `pos` within `reach` screen
    /// points: its index in the body's mesh, and its nearest point.
    pub fn edge_near(&self, hit: &SurfaceHit, pos: egui::Pos2, reach: f32) -> Option<usize> {
        let g = self.model.as_ref()?.geometry.get(hit.geometry)?;
        let mesh = &g.mesh;
        let face = u32::try_from(hit.face).ok()?;
        let mut best: Option<(f32, usize)> = None;
        for (i, polyline) in mesh.edges.iter().enumerate() {
            let bounds = mesh.edge_faces.get(i).is_some_and(|fs| fs.contains(&face));
            if !bounds || !mesh.edge_pickable(i) {
                continue;
            }
            let screen: Vec<egui::Pos2> = polyline
                .iter()
                .filter_map(|p| self.to_screen(arr(placed(g, *p))))
                .collect();
            for w in screen.windows(2) {
                let d = segment_distance(pos, w[0], w[1]);
                if d <= reach && best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, i));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Highlight `hovered` (or nothing) in the selection layer. A change
    /// of what is hovered publishes the layer again; the same hover keeps
    /// it as it is.
    pub fn set_hover(&mut self, hovered: Option<Hovered>) {
        if self.hovered.as_ref().map(|(h, _)| *h) == hovered {
            return;
        }
        self.hovered = hovered.and_then(|h| Some((h, self.hover_block(h)?)));
        self.publish_highlight();
    }

    /// What is highlighted as hovered.
    #[cfg(test)]
    pub fn hovered(&self) -> Option<Hovered> {
        self.hovered.as_ref().map(|(h, _)| *h)
    }

    /// The hovered face's triangles and outline, or the hovered edge, as
    /// one block of the selection layer (palette entries 2 and 3).
    fn hover_block(&self, hovered: Hovered) -> Option<SceneBlock> {
        let model = self.model.as_ref()?;
        let mut builder = BlockBuilder::default();
        builder.begin_body();
        let point = |g: &Geometry, p: Vec3| -> [f32; 3] {
            let [x, y, z] = self.display.apply(arr(placed(g, p)));
            [x as f32, y as f32, z as f32]
        };
        match hovered {
            Hovered::Face { geometry, face } => {
                let g = model.geometry.get(geometry)?;
                let mesh = &g.mesh;
                let face = u32::try_from(face).ok()?;
                for (t, f) in mesh.triangles.iter().zip(&mesh.tri_face) {
                    if *f != face {
                        continue;
                    }
                    if let Some(c) = mesh.tri(t) {
                        builder.triangle(
                            c.map(|p| point(g, p)),
                            [[0.0; 3]; 3],
                            PaletteIndex(2),
                            TriClass::Opaque,
                        );
                    }
                }
                for (i, polyline) in mesh.edges.iter().enumerate() {
                    let bounds = mesh.edge_faces.get(i).is_some_and(|fs| fs.contains(&face));
                    if bounds && mesh.edge_pickable(i) {
                        for w in polyline.windows(2) {
                            builder.line(point(g, w[0]), point(g, w[1]), PaletteIndex(3), 1.5);
                        }
                    }
                }
            }
            Hovered::Edge { geometry, edge } => {
                let g = model.geometry.get(geometry)?;
                for w in g.mesh.edges.get(edge)?.windows(2) {
                    builder.line(point(g, w[0]), point(g, w[1]), PaletteIndex(3), 2.0);
                }
            }
        }
        builder.end_body();
        Some(SceneBlock {
            key: next_key(),
            bytes: builder.freeze(),
        })
    }
}

/// Edges near the pointer checked for being in view, and kept.
const MAX_EDGE_CANDIDATES: usize = 16;
pub const MAX_EDGE_HITS: usize = 8;

/// The point of segment `a`–`b` nearest the ray from `o` along `d`: the
/// edge's point under the pointer.
fn ray_segment_closest(o: Vec3, d: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let u = b - a;
    let w = a - o;
    let (dd, du, uu) = (d.dot(d), d.dot(u), u.dot(u));
    let den = dd * uu - du * du;
    // Parallel or degenerate: the start stands for the segment.
    if !(den.is_finite() && den > 1e-12 * dd * uu) {
        return a;
    }
    let s = (du * d.dot(w) - dd * u.dot(w)) / den;
    if !s.is_finite() {
        return a;
    }
    a + u * s.clamp(0.0, 1.0)
}

/// Distance from `p` to the segment `a`–`b` (screen points).
fn segment_distance(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let d = b - a;
    let len2 = d.length_sq();
    let t = if len2 > 0.0 {
        ((p - a).dot(d) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + d * t)
}
