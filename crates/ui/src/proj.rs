//! The viewport's world ↔ screen mapping, from SolidCraft's `crates/ui-egui/src/viewport.rs`
//! (only [`Proj`] and the face drawing the move triad uses).

use egui::{Color32, Pos2, Rect, Shape, Stroke, pos2};
use solidcore_record::Vec3;
use solidcraft_viewport::{Camera, Mat4};

/// World ↔ screen mapping for the current frame.
#[derive(Clone, Copy)]
pub struct Proj {
    pub cam: Camera,
    pub vp: Mat4,
    pub rect: Rect,
}

impl Proj {
    pub fn to_screen(&self, p: Vec3) -> Option<Pos2> {
        let c = self.vp.apply(p);
        if c[3] <= 1e-9 {
            return None;
        }
        let (x, y) = (c[0] / c[3], c[1] / c[3]);
        Some(pos2(
            self.rect.left() + ((x + 1.0) * 0.5 * self.rect.width() as f64) as f32,
            self.rect.top() + ((1.0 - y) * 0.5 * self.rect.height() as f64) as f32,
        ))
    }
    pub fn ray(&self, p: Pos2) -> (Vec3, Vec3) {
        self.cam.ray((p.x - self.rect.left()) as f64, (p.y - self.rect.top()) as f64, self.rect.width() as f64, self.rect.height() as f64)
    }
}

/// A projected 3D face (convex): an unfeathered triangle fan, outlined one edge at a time (no
/// joins between the edges). Unlike `Shape::convex_polygon`, it stays inside its outline when
/// the face turns edge-on.
pub(crate) fn projected_polygon(pts: &[Pos2], fill: Color32, stroke: Stroke) -> Vec<Shape> {
    let mut mesh = egui::Mesh::default();
    for (i, p) in (0u32..).zip(pts) {
        mesh.colored_vertex(*p, fill);
        if i >= 2 {
            mesh.add_triangle(0, i - 1, i);
        }
    }
    let mut out = vec![Shape::mesh(mesh)];
    out.extend(pts.iter().zip(pts.iter().skip(1).chain(pts.first())).map(|(a, b)| Shape::line_segment([*a, *b], stroke)));
    out
}

/// View directions all round, a few degrees apart, none exactly along an axis.
#[cfg(test)]
pub(crate) fn orbit_directions() -> impl Iterator<Item = Vec3> {
    (0..90).flat_map(|i| {
        (0..45).map(move |j| {
            let (yaw, pitch) = (f64::from(i) * 4.0f64.to_radians() + 0.003, f64::from(j) * 4.0f64.to_radians() - 1.5);
            Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin())
        })
    })
}

/// Every vertex egui draws for `shapes`.
#[cfg(test)]
pub(crate) fn drawn_points(shapes: Vec<Shape>) -> Vec<Pos2> {
    let mut tess = egui::epaint::Tessellator::new(1.0, egui::epaint::TessellationOptions::default(), [1, 1], vec![]);
    let mut mesh = egui::Mesh::default();
    for shape in shapes {
        tess.tessellate_shape(shape, &mut mesh);
    }
    mesh.vertices.iter().map(|v| v.pos).collect()
}
