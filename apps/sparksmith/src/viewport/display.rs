//! Component 3D display hooks: a rigid display transform of the shown model
//! (the package placement and its drag preview), applied to the cached
//! meshes like Solidcraft's move preview draws moved bodies from their own
//! display triangles, and a display-only overlay scene (lands, seating
//! plane) in the renderer's preview slot. Neither builds nor saves geometry.
use super::{Viewport, model_scene};
use solidcraft_viewport_gpu::view::GpuScene;
use sparksmith_core::geometry::Transform3;
use sparksmith_solidcraft::{
    geom::{Aabb3, Vec3},
    scene::{Geometry, Place},
};
use sparksmith_ui::viewport::Proj;
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
            let mut bounds = Aabb3::EMPTY;
            for corner in corners(g.min, g.max) {
                bounds.add(vec3(t.apply(corner)));
            }
            // A placed body keeps its shared mesh: only its place moves.
            if let Some(place) = g.place {
                let moved = Place(std::array::from_fn(|r| {
                    [
                        t.rotation[r][0],
                        t.rotation[r][1],
                        t.rotation[r][2],
                        t.translation[r],
                    ]
                }));
                return Geometry {
                    place: Some(moved.then(&place)),
                    min: array(bounds.min),
                    max: array(bounds.max),
                    ..g.clone()
                };
            }
            let mut mesh = (*g.mesh).clone();
            for p in &mut mesh.positions {
                *p = vec3(t.apply(array(*p)));
            }
            for n in &mut mesh.normals {
                *n = vec3(t.apply_vector(array(*n)));
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

    /// What the renderer does at its next frame: take the model scene
    /// published since its last frame (Solidcraft's `Batches::take` empties
    /// the slot), if any.
    #[cfg(test)]
    pub(crate) fn take_published(&mut self) -> Option<solidcraft_viewport_gpu::view::GpuScene> {
        self.slot.lock().ok().and_then(|mut s| s.take())
    }

    /// The model shown.
    #[cfg(test)]
    pub(crate) fn shown_model(&self) -> Option<&crate::model_scene::ModelScene> {
        self.model.as_ref()
    }

    /// Whether the model's stream is being packed again off the UI thread.
    #[cfg(test)]
    pub(crate) fn repacking(&self) -> bool {
        self.repack.is_some()
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
        // Instanced bodies follow by their placements alone; only the
        // stream is packed again.
        let triangles: usize = model
            .geometry
            .iter()
            .filter(|g| model_scene::streamed(g))
            .map(|g| g.mesh.triangles.len())
            .sum();
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
        let decals = if identity {
            self.decals.geometry.clone()
        } else {
            transformed(&self.decals.geometry, &self.display)
        };
        let fade = self.fade.as_deref();
        if let Ok((stream, _)) =
            model_scene::pack_faded(geometry, &decals, &self.layers, &model.stack, fade)
        {
            self.repack = None;
            self.publish_stream(stream);
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
            vp: self.camera.view_proj_bounds(
                aspect,
                &sparksmith_solidcraft::render::view_bounds(&self.bounds),
            ),
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
        let model = sparksmith_solidcraft::project::build(&project)?;
        let mut viewport = Viewport::detached();
        viewport.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
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

    /// Under a display transform every instanced body follows by its
    /// placement alone (the shared meshes are published as they are), the
    /// large unplaced ones too.
    #[test]
    fn a_display_transform_moves_instanced_bodies_by_their_placements()
    -> Result<(), Box<dyn std::error::Error>> {
        // A round hole is one wall face on this kernel: enough holes for the
        // laminate to count as a large body.
        let project = crate::model_scene::tests::via_project(512)?;
        let model = sparksmith_solidcraft::project::build(&project)?;
        assert!(
            model
                .geometry
                .iter()
                .any(|g| g.place.is_none()
                    && g.mesh.triangles.len() >= crate::model_scene::LARGE_BODY),
            "a large unplaced body is drawn instanced"
        );
        let mut viewport = Viewport::detached();
        viewport.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
        let published = |v: &Viewport| -> Vec<(usize, Vec<u8>)> {
            v.slot
                .lock()
                .ok()
                .and_then(|s| {
                    s.as_ref().map(|s| {
                        s.instanced
                            .iter()
                            .map(|m| (Arc::as_ptr(&m.tris) as usize, m.instances.clone()))
                            .collect()
                    })
                })
                .unwrap_or_default()
        };
        let before = published(&viewport);
        assert!(!before.is_empty());
        let t = Transform3::translation([5.0, -2.0, 1.0]);
        viewport.set_model_transform(t);
        let after = published(&viewport);
        assert_eq!(after.len(), before.len());
        let f = |b: &[u8], k: usize| {
            b.get(4 * k..4 * k + 4)
                .and_then(|x| x.try_into().ok())
                .map_or(f32::NAN, f32::from_le_bytes)
        };
        for ((tris, was), (moved_tris, now)) in before.iter().zip(&after) {
            assert_eq!(tris, moved_tris, "the same shared triangles");
            assert_eq!(was.len(), now.len(), "every placement, large bodies too");
            for (a, b) in was.chunks_exact(52).zip(now.chunks_exact(52)) {
                for (row, by) in [(0, 5.0), (1, -2.0), (2, 1.0)] {
                    let shift = f(b, 4 * row + 3) - f(a, 4 * row + 3);
                    assert!((shift - by).abs() < 1e-4, "{shift} against {by}");
                }
            }
        }
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
