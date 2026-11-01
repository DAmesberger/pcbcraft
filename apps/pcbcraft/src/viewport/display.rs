//! Component 3D display hooks: a rigid display transform of the shown model
//! (the package placement and its drag preview), applied to the cached
//! meshes like Solidcraft's move preview draws moved bodies from their own
//! display triangles, and a display-only overlay scene (lands, seating
//! plane) in the renderer's preview slot. Neither builds nor saves geometry.
use super::{Viewport, model_scene};
use pcbcraft_core::geometry::Transform3;
use pcbcraft_solidcraft::{
    geom::{Aabb3, Vec3},
    scene::Geometry,
};
use solidcraft_ui_egui::gpu::GpuScene;
use solidcraft_ui_egui::viewport::Proj;
use std::sync::Arc;

/// Most model triangles the display transform repacks on the UI thread in
/// one frame (a package is a few thousand); a larger model keeps its last
/// packing while only the overlay follows.
pub const MAX_LIVE_TRIANGLES: usize = 200_000;

pub(super) fn point(t: &Transform3, p: [f32; 3]) -> [f32; 3] {
    if *t == Transform3::IDENTITY {
        return p;
    }
    t.apply(p.map(f64::from)).map(|v| v as f32)
}

fn vec3(p: [f64; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

fn array(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// `geometry` moved by `t`: positions, normals and boxes.
pub fn transformed(geometry: &[Geometry], t: &Transform3) -> Vec<Geometry> {
    geometry
        .iter()
        .map(|g| {
            let mut mesh = (*g.mesh).clone();
            for p in &mut mesh.positions {
                *p = vec3(t.apply(array(*p)));
            }
            for n in &mut mesh.normals {
                *n = vec3(t.apply_vector(array(*n)));
            }
            let mut bounds = Aabb3::EMPTY;
            for corner in corners(g.min, g.max) {
                bounds.add(vec3(t.apply(corner)));
            }
            Geometry {
                mesh: Arc::new(mesh),
                min: array(bounds.min),
                max: array(bounds.max),
                ..g.clone()
            }
        })
        .collect()
}

fn corners(min: [f64; 3], max: [f64; 3]) -> [[f64; 3]; 8] {
    std::array::from_fn(|k| {
        [
            if k & 1 == 0 { min[0] } else { max[0] },
            if k & 2 == 0 { min[1] } else { max[1] },
            if k & 4 == 0 { min[2] } else { max[2] },
        ]
    })
}

/// The inverse of a rigid transform.
fn inverse(t: &Transform3) -> Transform3 {
    let r = t.rotation;
    let rotation = std::array::from_fn(|i| std::array::from_fn(|j| r[j][i]));
    let back = Transform3 {
        rotation,
        translation: [0.0; 3],
    };
    let shift = back.apply_vector(t.translation).map(|v| -v);
    Transform3 {
        rotation,
        translation: shift,
    }
}

impl Viewport {
    /// Show the model moved by `t` (model → view frame). Only the cached
    /// meshes are repacked; nothing is evaluated or rebuilt.
    pub fn set_model_transform(&mut self, t: Transform3) {
        if self.display == t {
            return;
        }
        self.display = t;
        self.apply_display();
    }

    /// The display transform the model is shown with.
    #[cfg(test)]
    pub(crate) fn model_transform(&self) -> Transform3 {
        self.display
    }

    /// Models shown so far: a rebuild shows a new one.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Publish the model's PBR vertices and shadows under the display
    /// transform.
    pub(super) fn apply_display(&mut self) {
        self.refresh_bounds();
        let Some(model) = &self.model else {
            return;
        };
        let triangles: usize = model.geometry.iter().map(|g| g.mesh.triangles.len()).sum();
        let identity = self.display == Transform3::IDENTITY;
        self.display_limited = !identity && triangles > MAX_LIVE_TRIANGLES;
        if self.display_limited {
            return;
        }
        let moved;
        let geometry = if identity {
            &model.geometry
        } else {
            moved = transformed(&model.geometry, &self.display);
            &moved
        };
        if let Ok((pbr, _)) = model_scene::pack(geometry, &self.layers) {
            self.repack = None;
            self.publish_pbr(pbr);
        }
        self.shadow_view = None;
        self.publish_shadows();
    }

    /// The view bounds: the model's under the display transform and the
    /// overlay's.
    pub(super) fn refresh_bounds(&mut self) {
        let mut bounds = Aabb3::EMPTY;
        if !self.model_bounds.is_empty() {
            for corner in corners(array(self.model_bounds.min), array(self.model_bounds.max)) {
                bounds.add(vec3(self.display.apply(corner)));
            }
        }
        if !self.overlay_bounds.is_empty() {
            bounds.add(self.overlay_bounds.min);
            bounds.add(self.overlay_bounds.max);
        }
        if !bounds.is_empty() {
            self.bounds = bounds;
        }
    }

    /// A pointer ray in the model's own frame.
    pub(super) fn model_ray(&self, origin: Vec3, direction: Vec3) -> (Vec3, Vec3) {
        if self.display == Transform3::IDENTITY {
            return (origin, direction);
        }
        let back = inverse(&self.display);
        (
            vec3(back.apply(array(origin))),
            vec3(back.apply_vector(array(direction))),
        )
    }

    /// Show a display-only overlay scene drawn with the model (depth
    /// tested), or none. `bounds` joins the view bounds.
    pub fn set_overlay(&mut self, overlay: Option<(GpuScene, Aabb3)>) {
        match overlay {
            Some((scene, bounds)) => {
                Self::publish(&self.pv_slot, &mut self.pv_key, scene);
                self.overlay_bounds = bounds;
            }
            None => {
                self.pv_key = 0;
                self.overlay_bounds = Aabb3::EMPTY;
            }
        }
        self.refresh_bounds();
    }

    /// The model is too large to follow the display transform live: it
    /// shows its last placement until the command is applied and rebuilt.
    pub fn display_limited(&self) -> bool {
        self.display_limited
    }

    pub fn has_overlay(&self) -> bool {
        self.pv_key != 0
    }

    /// Where the view was last drawn.
    pub fn rect(&self) -> Option<egui::Rect> {
        self.rect
    }

    /// World ↔ screen mapping of the view drawn in `rect`, in Solidcraft's
    /// form (what its triad takes).
    pub fn proj(&self, rect: egui::Rect) -> Proj {
        let aspect = f64::from(rect.width() / rect.height().max(1.0));
        Proj {
            cam: self.camera,
            vp: self.camera.view_proj_bounds(aspect, &self.bounds),
            rect,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_display_transform_moves_cached_meshes_without_a_rebuild()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = crate::model_scene::tests::demo_project()?;
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let mut viewport = Viewport::detached();
        viewport.set_model(crate::model_scene::build(model.geometry)?);
        let generation = viewport.generation;
        let before = viewport.bounds;
        let key = viewport.key;
        viewport.set_model_transform(Transform3::translation([5.0, 0.0, 1.0]));
        // Same model (no new generation), new vertices, moved bounds.
        assert_eq!(viewport.generation, generation);
        assert_ne!(viewport.key, key);
        assert!((viewport.bounds.min.x - before.min.x - 5.0).abs() < 1e-6);
        assert!((viewport.bounds.max.z - before.max.z - 1.0).abs() < 1e-6);
        // Setting the same transform again publishes nothing.
        let key = viewport.key;
        viewport.set_model_transform(Transform3::translation([5.0, 0.0, 1.0]));
        assert_eq!(viewport.key, key);
        Ok(())
    }

    #[test]
    fn the_inverse_undoes_a_turned_shift() {
        let t = Transform3::translation([1.0, 2.0, 3.0])
            .then(&Transform3::intrinsic_xyz([30.0, 0.0, 90.0]));
        let p = [0.3, -0.7, 2.0];
        let back = inverse(&t).apply(t.apply(p));
        assert!(back.iter().zip(p).all(|(a, b)| (a - b).abs() < 1e-12));
    }
}
