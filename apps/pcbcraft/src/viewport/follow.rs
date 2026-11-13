//! Follow the net: a see-through look of the PCB 3D view in which the
//! selected net's tracks, vias, pads and fills stay solid and every other
//! body (the laminate, the mask, other copper, the parts) is faded, so the
//! net reads as it travels through the layers.
//!
//! The faded bodies are drawn in Solidcraft's faded-context class
//! ([`gpu::TriClass::Faded`], [`gpu::InstanceLook::Faded`]), which its
//! renderer blends with weighted blended order-independent transparency
//! (`viewport-gpu` `view/oit.rs`; unsorted alpha blending on a device
//! without float blending). Nothing of the renderer is changed. The look is
//! display state over the cached meshes: turning it on, or changing the
//! net while it is on, packs the stream again off the UI thread as a layer
//! change does ([`Viewport::start_repack`]) and splits the instanced
//! placements by look, keeping their shared triangles; no geometry is
//! rebuilt. Selection and its net tint stay the highlight layer's.
use super::{Viewport, model_scene};
use std::sync::Arc;

#[cfg(doc)]
use solidcraft_viewport_gpu::view as gpu;

impl Viewport {
    /// Whether the follow-the-net look is on (a view preference).
    pub fn follow_net(&self) -> bool {
        self.follow
    }

    /// Turn the follow-the-net look on or off. It draws while a focus is
    /// set ([`Self::set_focus`]); without one the view is as built.
    pub fn set_follow_net(&mut self, on: bool) {
        self.follow = on;
        self.refresh_fade();
    }

    /// The pick identities the look keeps solid: the selected net's bodies,
    /// or none. A change while the look draws repacks the stream from the
    /// cached meshes; while it is off it only records the focus.
    pub fn set_focus(&mut self, mut focus: Vec<u64>) {
        focus.sort_unstable();
        focus.dedup();
        if focus != self.focus {
            self.focus = focus;
            self.refresh_fade();
        }
    }

    /// Whether the look is drawn now: on, with a focus.
    pub fn following(&self) -> bool {
        self.fade.is_some()
    }

    /// The look for the current preference and focus, published when it
    /// changed: the stream packed again and the instanced meshes split.
    fn refresh_fade(&mut self) {
        let fade = if self.follow {
            model_scene::Fade::new(self.focus.iter().copied()).map(Arc::new)
        } else {
            None
        };
        if fade != self.fade {
            self.fade = fade;
            self.publish_shadows();
            if self.model.is_some() {
                self.start_repack();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{demo, settle, vias};
    use super::*;
    use crate::layer_view::ViewLayer;
    use pcbcraft_solidcraft::material::BodyKind;
    use pcbcraft_solidcraft::scene::Geometry;
    use solidcraft_viewport_gpu::view::{GpuScene, InstanceLook, PaletteIndex, TRI_SIZE};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The published model scene, cloned.
    fn published(viewport: &Viewport) -> Result<GpuScene, Box<dyn std::error::Error>> {
        viewport
            .slot
            .lock()
            .map_err(|_| "slot poisoned")?
            .clone()
            .ok_or_else(|| "nothing published".into())
    }

    /// Every body's mesh address: unchanged while no geometry is rebuilt.
    fn meshes(viewport: &Viewport) -> Vec<usize> {
        viewport
            .model
            .as_ref()
            .map(|m| {
                m.geometry
                    .iter()
                    .map(|g| Arc::as_ptr(&g.mesh) as usize)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The pick identities of the bodies of one net: the demo track's.
    fn track_net(viewport: &Viewport) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
        let model = viewport.model.as_ref().ok_or("no model")?;
        let track = model
            .geometry
            .iter()
            .find(|g| g.kind == BodyKind::CopperTrack)
            .and_then(|g| g.item)
            .ok_or("no track")?;
        Ok(vec![track])
    }

    /// The alpha of the palette entry of every vertex in `bytes`.
    fn alphas(scene: &GpuScene, bytes: &[u8]) -> Vec<u8> {
        bytes
            .chunks_exact(TRI_SIZE)
            .filter_map(|v| v.get(24..28).and_then(|b| b.try_into().ok()))
            .map(u32::from_le_bytes)
            .filter_map(|i| scene.palette.get(PaletteIndex(i)))
            .map(|rgba| rgba[3])
            .collect()
    }

    fn streamed_bytes(viewport: &Viewport, keep: impl Fn(&Geometry) -> bool) -> usize {
        viewport
            .model
            .as_ref()
            .map(|m| {
                m.geometry
                    .iter()
                    .filter(|g| model_scene::streamed(g) && keep(g))
                    .map(|g| g.mesh.triangles.len() * 3 * TRI_SIZE)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// The look is a toggle: off by default, drawn only while on with a
    /// focus, and off again restores the model as built.
    #[test]
    fn follow_net_toggles_and_draws_only_with_a_focus() -> TestResult {
        let mut viewport = demo()?;
        assert!(!viewport.follow_net(), "off by default");
        let built = published(&viewport)?;
        let net = track_net(&viewport)?;
        viewport.set_focus(net.clone());
        assert!(!viewport.following(), "a focus alone draws nothing");
        assert!(viewport.repack.is_none(), "nor packs anything");
        let shadows = |v: &Viewport| {
            v.sketch_slot
                .lock()
                .ok()
                .and_then(|s| s.as_ref().map(|s| s.trans.len()))
                .unwrap_or(0)
        };
        let all_shadows = shadows(&viewport);
        viewport.set_follow_net(true);
        assert!(viewport.follow_net() && viewport.following());
        let ground = shadows(&viewport);
        assert!(
            0 < ground && ground < all_shadows,
            "the see-through parts cast no contact shadow"
        );
        settle(&mut viewport)?;
        assert!(!published(&viewport)?.trans.is_empty(), "the rest fades");
        viewport.set_focus(Vec::new());
        assert!(
            viewport.follow_net() && !viewport.following(),
            "nothing to follow"
        );
        settle(&mut viewport)?;
        let plain = published(&viewport)?;
        assert!(plain.trans.is_empty());
        assert_eq!(plain.tris, built.tris, "as built without a focus");
        viewport.set_focus(net);
        viewport.set_follow_net(false);
        assert!(!viewport.following());
        settle(&mut viewport)?;
        let off = published(&viewport)?;
        assert!(off.trans.is_empty());
        assert_eq!(off.tris, built.tris, "off restores the model as built");
        assert_eq!(shadows(&viewport), all_shadows, "and its shadows");
        Ok(())
    }

    /// The net's bodies stay opaque in their own colours and material;
    /// every other body moves to the see-through triangles, faded.
    #[test]
    fn the_nets_bodies_stay_opaque_and_the_rest_fade() -> TestResult {
        let mut viewport = demo()?;
        // The track and a body of the stream (a pad's copper, say).
        let mut net = track_net(&viewport)?;
        let streamed = viewport
            .model
            .as_ref()
            .and_then(|m| {
                m.geometry.iter().find(|g| {
                    model_scene::streamed(g) && g.kind != BodyKind::MaskCap && g.item.is_some()
                })
            })
            .and_then(|g| g.item)
            .ok_or("no streamed body with a pick identity")?;
        net.push(streamed);
        viewport.set_follow_net(true);
        viewport.set_focus(net.clone());
        settle(&mut viewport)?;
        let scene = published(&viewport)?;
        let kept =
            |g: &Geometry| g.kind != BodyKind::MaskCap && g.item.is_some_and(|i| net.contains(&i));
        let solid = streamed_bytes(&viewport, kept);
        let faded = streamed_bytes(&viewport, |g| !kept(g));
        assert!(solid > 0 && faded > 0, "{solid} solid, {faded} faded bytes");
        assert_eq!(scene.tris.len(), solid, "only the net is opaque");
        assert_eq!(scene.trans.len(), faded, "everything else is see-through");
        assert!(alphas(&scene, &scene.tris).iter().all(|a| *a == 255));
        assert!(
            alphas(&scene, &scene.trans)
                .iter()
                .all(|a| *a == model_scene::FADE_ALPHA)
        );
        // The material stream shades the net only.
        let shaded: usize = scene.graphs.draws().iter().map(|d| d.vertices.len()).sum();
        assert!(shaded > 0, "the net keeps its material");
        // Placed and large bodies (the laminate, the mask) fade as placements.
        let model = viewport.model.as_ref().ok_or("no model")?;
        let large = model
            .geometry
            .iter()
            .filter(|g| !model_scene::streamed(g))
            .count();
        let drawn: usize = scene.instanced.iter().map(|m| m.count()).sum();
        assert_eq!(drawn, large, "every placement is drawn once");
        assert!(
            scene
                .instanced
                .iter()
                .any(|m| m.look == InstanceLook::Faded),
            "the board fades"
        );
        for m in scene
            .instanced
            .iter()
            .filter(|m| m.look == InstanceLook::Faded)
        {
            assert!(m.graph.is_none(), "a faded body has no material overlay");
        }
        Ok(())
    }

    /// A via of the followed net stays an opaque placement while the other
    /// vias fade, from the same shared triangles; a click over it picks it
    /// through the faded mask above.
    #[test]
    fn a_followed_via_stays_solid_among_faded_vias() -> TestResult {
        let mut viewport = vias(40)?;
        let (item, x, y) = super::super::tests::above_via(&mut viewport, 17)?;
        let shared: Vec<usize> = viewport
            .model
            .as_ref()
            .map(|m| {
                m.instanced
                    .iter()
                    .map(|i| Arc::as_ptr(&i.mesh.tris) as usize)
                    .collect()
            })
            .unwrap_or_default();
        viewport.set_follow_net(true);
        viewport.set_focus(vec![item]);
        settle(&mut viewport)?;
        let scene = published(&viewport)?;
        let model = viewport.model.as_ref().ok_or("no model")?;
        let of_via = model
            .geometry
            .iter()
            .filter(|g| g.item == Some(item) && g.kind != BodyKind::MaskCap)
            .count();
        let opaque_vias: usize = scene
            .instanced
            .iter()
            .filter(|m| m.look == InstanceLook::Opaque)
            .map(|m| m.count())
            .sum();
        assert_eq!(opaque_vias, of_via, "only the followed via is opaque");
        assert!(
            scene
                .instanced
                .iter()
                .all(|m| shared.contains(&(Arc::as_ptr(&m.tris) as usize))),
            "every split mesh draws the shared triangles"
        );
        assert!(model.instanced.iter().any(|i| i.layer == ViewLayer::Vias));
        assert_eq!(viewport.pick(x, y, 800.0, 600.0), Some(item));
        Ok(())
    }

    /// Changing the selection while following never rebuilds geometry: the
    /// cached meshes and the model's generation stay, only the look's
    /// stream is packed again and the highlight recoloured.
    #[test]
    fn selection_changes_while_following_rebuild_no_geometry() -> TestResult {
        let mut viewport = vias(8)?;
        let before = meshes(&viewport);
        let generation = viewport.generation;
        let instanced: Vec<usize> = viewport
            .model
            .as_ref()
            .map(|m| {
                m.instanced
                    .iter()
                    .map(|i| Arc::as_ptr(&i.mesh.tris) as usize)
                    .collect()
            })
            .unwrap_or_default();
        viewport.set_follow_net(true);
        let (first, _, _) = super::super::tests::above_via(&mut viewport, 1)?;
        let (second, _, _) = super::super::tests::above_via(&mut viewport, 5)?;
        for item in [first, second, first] {
            viewport.highlight_selected(vec![item], vec![item]);
            viewport.set_focus(vec![item]);
            settle(&mut viewport)?;
            assert!(viewport.following());
        }
        // The same focus again: nothing to pack.
        viewport.set_focus(vec![first]);
        assert!(viewport.repack.is_none());
        assert_eq!(
            viewport.generation, generation,
            "the model was not replaced"
        );
        assert_eq!(meshes(&viewport), before, "the cached meshes are kept");
        let after: Vec<usize> = viewport
            .model
            .as_ref()
            .map(|m| {
                m.instanced
                    .iter()
                    .map(|i| Arc::as_ptr(&i.mesh.tris) as usize)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(after, instanced, "the instanced triangles are kept");
        Ok(())
    }
}
