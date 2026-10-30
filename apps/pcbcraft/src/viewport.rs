//! Solidcraft's GPU renderer and camera over cached geometry. Solidcraft's full
//! navigation handler is private to its app, so this applies the same camera
//! operations (middle-drag pan, right-drag orbit, scroll zoom) directly.
//!
//! The model is drawn through Solidcraft's PBR path with the studio rig in
//! [`crate::lighting`]. Three slots are published separately so each change
//! uploads only what it touches: the model's PBR vertices (`slot`: a new
//! model or the Hide-substrate toggle), shadows (`sketch_slot`: theme, side
//! or toggle) and the selection (`hl_slot`: cached flat blocks recoloured).
//! Adaptive supersampling follows Solidcraft's viewport `supersampling` and
//! `view_settled` (MIT, revision 4f3ac07; see ATTRIBUTION.md).
use crate::lighting;
use crate::model_scene::{ModelScene, Segment, ShadowVertex, next_key};
use pcbcraft_solidcraft::{
    geom::{Aabb3, Vec3},
    render::Camera,
};
use solidcraft_ui_egui::gpu::{
    self, Frame, GpuScene, GpuTarget, Palette, SceneSlot, ViewportCallback,
};
use solidcraft_ui_egui::theme;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

/// Vertical field of view: a gentle perspective, as a product photograph.
const FIELD_OF_VIEW: f64 = 0.5;
/// How long the view holds still before it is drawn supersampled (s).
const SETTLE: f64 = 0.25;
const SUPERSAMPLE: f32 = 2.0;
/// Selection fill opacity over the lit model.
const HIGHLIGHT_ALPHA: u8 = 110;
/// Peak shadow opacity on light and dark backgrounds (Solidcraft's values).
const SHADOW_ALPHA: [f32; 2] = [80.0, 150.0];

/// What the shadow slot was last built for: dark theme, seen from below,
/// substrate hidden.
type ShadowView = (bool, bool, bool);

pub struct Viewport {
    model: Option<ModelScene>,
    bounds: Aabb3,
    highlighted: Vec<u64>,
    slot: SceneSlot,
    key: u64,
    sketch_slot: SceneSlot,
    sketch_key: u64,
    hl_slot: SceneSlot,
    hl_key: u64,
    empty: SceneSlot,
    shadow_view: Option<ShadowView>,
    /// The view's hash and when it last changed.
    settle: (u64, f64),
    camera: Camera,
    pub hide_substrate: bool,
}

impl Viewport {
    pub fn new(rs: &egui_wgpu::RenderState) -> Self {
        let window = GpuTarget {
            format: rs.target_format,
            depth: None,
            samples: 1,
            tess: false,
            oit: false,
        };
        let scene = GpuTarget {
            depth: Some(egui_wgpu::wgpu::TextureFormat::Depth32Float),
            ..window
        };
        gpu::install(rs, scene, window, false);
        Self::detached()
    }

    /// The viewport's state without a GPU (the renderer is installed by
    /// [`Viewport::new`]).
    pub(crate) fn detached() -> Self {
        let camera = Camera {
            fov: FIELD_OF_VIEW,
            ..Camera::default()
        };
        let mut result = Self {
            model: None,
            bounds: Self::initial_bounds(),
            highlighted: vec![],
            slot: Arc::new(Mutex::new(None)),
            key: next_key(),
            sketch_slot: Arc::new(Mutex::new(None)),
            sketch_key: next_key(),
            hl_slot: Arc::new(Mutex::new(None)),
            hl_key: next_key(),
            empty: Arc::new(Mutex::new(None)),
            shadow_view: None,
            settle: (0, 0.0),
            camera,
            hide_substrate: false,
        };
        result.fit();
        result
    }

    fn initial_bounds() -> Aabb3 {
        Aabb3 {
            min: Vec3::new(0.0, 0.0, -0.1),
            max: Vec3::new(40.0, 30.0, 1.7),
        }
    }

    pub fn fit(&mut self) {
        self.camera.fit(&self.bounds);
    }

    /// Show a model built on the worker. Only its prepared bytes are moved
    /// into the slots; nothing is packed here.
    pub fn set_model(&mut self, model: ModelScene) {
        if !model.bounds.is_empty() {
            self.bounds = model.bounds;
        }
        self.model = Some(model);
        self.refresh();
    }

    /// Publish everything again (a new model or the Hide-substrate toggle).
    pub fn refresh(&mut self) {
        self.publish_model();
        self.shadow_view = None;
        self.publish_shadows();
        self.publish_highlight();
    }

    fn publish(slot: &SceneSlot, key: &mut u64, scene: GpuScene) {
        if let Ok(mut slot) = slot.lock() {
            *slot = Some(scene);
            *key = next_key();
        }
    }

    fn publish_model(&mut self) {
        let mut scene = GpuScene::default();
        if let Some(model) = &self.model {
            let board = if self.hide_substrate {
                0
            } else {
                model.board_pbr.len()
            };
            if scene
                .pbr
                .try_reserve_exact(model.parts_pbr.len().saturating_add(board))
                .is_ok()
            {
                scene.pbr.extend_from_slice(&model.parts_pbr);
                if !self.hide_substrate {
                    scene.pbr.extend_from_slice(&model.board_pbr);
                }
            }
        }
        Self::publish(&self.slot, &mut self.key, scene);
    }

    fn current_shadow_view(&self) -> ShadowView {
        (
            theme::is_dark(),
            self.camera.pitch < 0.0,
            self.hide_substrate,
        )
    }

    /// Rebuild the shadow slot when the theme, the side seen or the toggle
    /// changed. The model's PBR buffer is not touched.
    fn publish_shadows(&mut self) {
        let view = self.current_shadow_view();
        if self.shadow_view == Some(view) {
            return;
        }
        self.shadow_view = Some(view);
        let (dark, below, hidden) = view;
        let peak = SHADOW_ALPHA[usize::from(dark)];
        let mut scene = GpuScene::default();
        if let Some(model) = self.model.as_ref().filter(|_| !hidden) {
            let mut add = |vertices: &[ShadowVertex]| {
                for (p, weight) in vertices {
                    let alpha = (peak * weight.clamp(0.0, 1.0)).round() as u8;
                    let color = scene.color([0, 0, 0, alpha]);
                    scene.trans_tri(*p, [0.0; 3], color);
                }
            };
            if below {
                add(&model.shadows.bottom);
            } else {
                add(&model.shadows.ground);
                add(&model.shadows.top);
            }
        }
        Self::publish(&self.sketch_slot, &mut self.sketch_key, scene);
    }

    pub fn highlight(&mut self, ids: Vec<u64>) {
        if self.highlighted != ids {
            self.highlighted = ids;
            self.publish_highlight();
        }
    }

    /// The selection: cached flat blocks of the highlighted items in the
    /// selection colour. Model geometry and its key stay as they are.
    fn publish_highlight(&mut self) {
        let selected = crate::palette::board().selected;
        let rgba = [selected.r(), selected.g(), selected.b(), HIGHLIGHT_ALPHA];
        let mut scene = GpuScene::with_palette(Palette::with_stable(vec![rgba]));
        if let Some(model) = &self.model {
            scene.blocks = model
                .highlight
                .iter()
                .filter(|(item, segment, _)| {
                    self.highlighted.contains(item)
                        && !(self.hide_substrate && *segment == Segment::Board)
                })
                .map(|(_, _, block)| block.clone())
                .collect();
        }
        Self::publish(&self.hl_slot, &mut self.hl_key, scene);
    }

    /// The pick identity under a pixel of a `width` × `height` view.
    pub fn pick(&self, x: f64, y: f64, width: f64, height: f64) -> Option<u64> {
        let model = self.model.as_ref()?;
        let (origin, direction) = self.camera.ray(x, y, width, height);
        model
            .geometry
            .iter()
            .filter(|g| !(self.hide_substrate && g.kind.is_board()))
            .filter_map(|g| g.mesh.raycast(origin, direction).map(|(d, _)| (d, g.item)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .and_then(|(_, id)| id)
    }

    /// Supersampling once the view has held still (Solidcraft's adaptive
    /// pattern), asking for the repaint that draws it.
    fn supersampling(
        &mut self,
        ctx: &egui::Context,
        view_proj: &[[f32; 4]; 4],
        size: [f32; 2],
    ) -> f32 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        view_proj
            .iter()
            .flatten()
            .chain(&size)
            .for_each(|x| x.to_bits().hash(&mut h));
        let view = h.finish();
        let now = ctx.input(|i| i.time);
        if view != self.settle.0 {
            self.settle = (view, now);
        }
        let still = now - self.settle.1;
        if still < SETTLE {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE - still + 0.01));
            1.0
        } else {
            SUPERSAMPLE
        }
    }

    /// A neutral studio sweep behind the model, following the theme.
    fn background(ui: &egui::Ui, rect: egui::Rect) {
        let (top, bottom) = if theme::is_dark() {
            (
                egui::Color32::from_rgb(0x2c, 0x2f, 0x34),
                egui::Color32::from_rgb(0x17, 0x19, 0x1c),
            )
        } else {
            (
                egui::Color32::from_rgb(0xe9, 0xeb, 0xee),
                egui::Color32::from_rgb(0xc8, 0xcc, 0xd1),
            )
        };
        let mut mesh = egui::Mesh::default();
        for (pos, color) in [
            (rect.left_top(), top),
            (rect.right_top(), top),
            (rect.right_bottom(), bottom),
            (rect.left_bottom(), bottom),
        ] {
            mesh.colored_vertex(pos, color);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));
    }

    /// Draw the view; `Some(pick)` when it was clicked.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Option<u64>> {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return None;
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            let d = ui.input(|i| i.pointer.delta());
            self.camera
                .pan(f64::from(d.x), f64::from(d.y), f64::from(rect.height()));
        }
        if response.dragged_by(egui::PointerButton::Secondary) {
            let d = ui.input(|i| i.pointer.delta());
            self.camera.orbit(f64::from(d.x), f64::from(d.y));
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom_at((f64::from(scroll) * 0.002).exp(), None);
            }
        }
        if self.shadow_view != Some(self.current_shadow_view()) {
            self.publish_shadows();
        }
        Self::background(ui, rect);
        let pp = ui.ctx().pixels_per_point();
        let eye = self.camera.eye().to_f32();
        let size_px = [rect.width() * pp, rect.height() * pp];
        let view_proj = self
            .camera
            .view_proj_bounds(f64::from(rect.width() / rect.height()), &self.bounds)
            .to_f32();
        let ssaa = self.supersampling(ui.ctx(), &view_proj, size_px);
        let cb = ViewportCallback(Frame {
            product_lights: Some(lighting::studio(
                &self.camera,
                &self.bounds,
                lighting::EXPOSURE,
            )),
            product: false,
            key: self.key,
            slot: self.slot.clone(),
            sketch_key: self.sketch_key,
            sketch_slot: self.sketch_slot.clone(),
            hl_key: self.hl_key,
            hl_slot: self.hl_slot.clone(),
            pv_key: 0,
            pv_slot: self.empty.clone(),
            pv_hide: true,
            view_proj,
            back: self.camera.back().to_f32(),
            size_px,
            clip: [0.0; 4],
            cap: [0.0; 4],
            images: vec![],
            points: vec![],
            analysis: [0.0; 8],
            analysis_colors: [0.0; 36],
            access: None,
            outline: false,
            eye: [
                eye[0],
                eye[1],
                eye[2],
                if self.camera.fov > 0.0 { 1.0 } else { 0.0 },
            ],
            ao_radius: 0.0,
            ao_samples: 0,
            ssaa,
            hover_tint: [0.1; 3],
            selection_tint: [0.8, 0.7, 0.1],
        });
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, cb));
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            return Some(self.pick(
                f64::from(p.x - rect.left()),
                f64::from(p.y - rect.top()),
                f64::from(rect.width()),
                f64::from(rect.height()),
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_solidcraft::material::BodyKind;

    fn demo() -> Result<Viewport, Box<dyn std::error::Error>> {
        let project = crate::model_scene::tests::demo_project()?;
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let mut viewport = Viewport::detached();
        viewport.set_model(crate::model_scene::build(model.geometry)?);
        viewport.fit();
        Ok(viewport)
    }

    fn blocks(slot: &SceneSlot) -> Vec<(u64, usize)> {
        slot.lock()
            .ok()
            .and_then(|s| {
                s.as_ref().map(|s| {
                    s.blocks
                        .iter()
                        .map(|b| (b.key, Arc::as_ptr(&b.bytes) as usize))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    fn pbr_len(slot: &SceneSlot) -> usize {
        slot.lock()
            .ok()
            .and_then(|s| s.as_ref().map(|s| s.pbr.len()))
            .unwrap_or(0)
    }

    #[test]
    fn highlighting_recolours_cached_blocks_without_touching_the_model()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = demo()?;
        let item = viewport
            .model
            .as_ref()
            .and_then(|m| m.geometry.iter().find_map(|g| g.item))
            .ok_or("no pickable body")?;
        let (key, sketch) = (viewport.key, viewport.sketch_key);
        viewport.highlight(vec![item]);
        let first = (viewport.hl_key, blocks(&viewport.hl_slot));
        viewport.highlight(vec![]);
        viewport.highlight(vec![item]);
        let second = (viewport.hl_key, blocks(&viewport.hl_slot));
        assert_eq!((viewport.key, viewport.sketch_key), (key, sketch));
        assert_ne!(first.0, second.0);
        assert!(!first.1.is_empty());
        assert_eq!(first.1, second.1, "the same blocks and bytes are reused");
        Ok(())
    }

    #[test]
    fn hiding_the_substrate_republishes_the_model_but_no_blocks()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = demo()?;
        let all: Vec<u64> = viewport
            .model
            .as_ref()
            .map(|m| m.highlight.iter().map(|(i, _, _)| *i).collect())
            .unwrap_or_default();
        viewport.highlight(all);
        let before = (
            viewport.key,
            blocks(&viewport.hl_slot),
            pbr_len(&viewport.slot),
        );
        viewport.hide_substrate = true;
        viewport.refresh();
        let after = (
            viewport.key,
            blocks(&viewport.hl_slot),
            pbr_len(&viewport.slot),
        );
        assert_ne!(before.0, after.0);
        assert!(after.2 < before.2);
        assert!(after.1.iter().all(|b| before.1.contains(b)));
        assert!(
            after.1.len() < before.1.len(),
            "mask caps are hidden with the board"
        );
        Ok(())
    }

    #[test]
    fn a_theme_flip_rebuilds_only_the_shadows() -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = demo()?;
        let dark = theme::is_dark();
        let before = (viewport.key, viewport.hl_key, viewport.sketch_key);
        viewport.shadow_view = Some((!dark, false, false));
        viewport.publish_shadows();
        let after = (viewport.key, viewport.hl_key, viewport.sketch_key);
        assert_eq!((before.0, before.1), (after.0, after.1));
        assert_ne!(before.2, after.2);
        let unchanged = viewport.sketch_key;
        viewport.publish_shadows();
        assert_eq!(viewport.sketch_key, unchanged, "nothing changed: no upload");
        Ok(())
    }

    #[test]
    fn the_perspective_view_fits_and_picks_the_part_under_the_pointer()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = demo()?;
        assert!(viewport.camera.fov > 0.0);
        let (w, h) = (800.0, 600.0);
        let radius = viewport.bounds.diagonal();
        for corner in 0..8 {
            let b = viewport.bounds;
            let p = Vec3::new(
                if corner & 1 == 0 { b.min.x } else { b.max.x },
                if corner & 2 == 0 { b.min.y } else { b.max.y },
                if corner & 4 == 0 { b.min.z } else { b.max.z },
            );
            let (x, y, _) = viewport
                .camera
                .to_screen(p, w, h, radius)
                .ok_or("behind the eye")?;
            assert!(
                (0.0..=w).contains(&x) && (0.0..=h).contains(&y),
                "{p:?} at {x}, {y}"
            );
        }
        let model = viewport.model.as_ref().ok_or("no model")?;
        let body = model
            .geometry
            .iter()
            .find(|g| g.kind == BodyKind::PackageBody && g.min[2] > 1.0)
            .ok_or("no top package")?;
        let item = body.item;
        let top = Vec3::new(
            (body.min[0] + body.max[0]) / 2.0,
            (body.min[1] + body.max[1]) / 2.0,
            body.max[2],
        );
        viewport.camera.target = top;
        let (x, y, _) = viewport
            .camera
            .to_screen(top, w, h, radius)
            .ok_or("behind the eye")?;
        assert_eq!(viewport.pick(x, y, w, h), item);
        viewport.hide_substrate = true;
        let edge = viewport
            .camera
            .to_screen(Vec3::new(0.5, 0.5, 1.6), w, h, radius);
        if let Some((x, y, _)) = edge {
            assert_eq!(
                viewport.pick(x, y, w, h),
                None,
                "hidden board is not picked"
            );
        }
        Ok(())
    }
}
