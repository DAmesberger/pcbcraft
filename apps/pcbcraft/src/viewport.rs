//! Solidcraft's GPU renderer and camera over cached geometry. Solidcraft's full
//! navigation handler is private to its app, so this applies the same camera
//! operations (middle-drag pan, right-drag orbit about the surface under the
//! pointer, scroll zoom toward the pointer) directly.
//!
//! The model is drawn through Solidcraft's PBR path with its opt-in product
//! look (a studio environment reflected and lighting from all around, clear
//! coats, procedural patterns, ambient occlusion, PBR Neutral tone mapping
//! and dither) and the direct lights in [`crate::lighting`]. The studio is
//! baked once on a worker thread and installed into the renderer, so the
//! window never waits for it. Three slots are published separately so each change
//! uploads only what it touches: the model's PBR vertices (`slot`: a new
//! model, moved in without a copy, or the Hide-substrate toggle, repacked off
//! the UI thread), shadows (`sketch_slot`: theme, side or toggle) and the
//! selection (`hl_slot`: cached flat blocks recoloured). Adaptive
//! supersampling follows Solidcraft's viewport `supersampling` and
//! `view_settled` (MIT, revision 4f3ac07; see ATTRIBUTION.md).
use crate::lighting;
use crate::model_scene::{self, ModelScene, Segment, ShadowVertex, next_key};
use pcbcraft_solidcraft::{
    geom::{Aabb3, Vec3},
    render::Camera,
    scene::Geometry,
};
use solidcraft_ui_egui::gpu::{
    self, Frame, GpuScene, GpuTarget, Palette, Renderer, SceneSlot, StudioEnvironment,
    ViewportCallback,
};
use solidcraft_ui_egui::theme;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

/// Vertical field of view: a gentle perspective, as a product photograph.
const FIELD_OF_VIEW: f64 = 0.5;
/// Share of the view a fitted model leaves free at its widest edge.
const FIT_MARGIN: f64 = 0.08;
/// How long the view holds still before it is drawn supersampled (s).
const SETTLE: f64 = 0.25;
const SUPERSAMPLE: f32 = 2.0;
/// Selection fill opacity over the lit model.
const HIGHLIGHT_ALPHA: u8 = 110;
/// Peak shadow opacity on light and dark backgrounds (Solidcraft's values).
const SHADOW_ALPHA: [f32; 2] = [80.0, 150.0];
/// Ambient occlusion samples per pixel for a still and a moving view
/// (Solidcraft's `AO_SAMPLES` and `AO_SAMPLES_MOVING`).
const AO_SAMPLES: [u32; 2] = [32, 8];

/// What the shadow slot was last built for: dark theme, seen from below,
/// substrate hidden.
type ShadowView = (bool, bool, bool);

/// A PBR repack running off the UI thread: the model generation and
/// substrate state it is for.
struct Repack {
    generation: u64,
    hidden: bool,
    result: Receiver<Result<Vec<u8>, String>>,
}

pub struct Viewport {
    model: Option<ModelScene>,
    /// Counts models shown, so a late repack never lands on a newer one.
    generation: u64,
    repack: Option<Repack>,
    bounds: Aabb3,
    highlighted: Vec<u64>,
    slot: SceneSlot,
    key: u64,
    sketch_key: u64,
    sketch_slot: SceneSlot,
    hl_slot: SceneSlot,
    hl_key: u64,
    empty: SceneSlot,
    shadow_view: Option<ShadowView>,
    /// The view's hash and when it last changed.
    settle: (u64, f64),
    camera: Camera,
    /// Width over height of the last drawn view.
    aspect: f64,
    /// Fit again once the view's shape is known (the next draw).
    fit_pending: bool,
    /// The point a right-drag orbits about.
    pivot: Option<Vec3>,
    hide_substrate: bool,
    /// Rigid display transform of the shown model (the Component 3D
    /// placement and its drag preview), applied to cached meshes
    /// ([`display`]); never a rebuild.
    display: pcbcraft_core::geometry::Transform3,
    /// The shown model's own bounds, before the display transform.
    model_bounds: Aabb3,
    /// The display-only overlay (lands, seating plane) in the preview slot.
    overlay_bounds: Aabb3,
    /// The display transform was not applied: too many triangles to repack.
    display_limited: bool,
    pv_key: u64,
    pv_slot: SceneSlot,
    /// Where the view was last drawn.
    rect: Option<egui::Rect>,
}

impl Viewport {
    pub fn new(rs: &egui_wgpu::RenderState, ctx: &egui::Context) -> Self {
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
        Self::bake_studio(rs, ctx);
        Self::detached()
    }

    /// Bake the product look's studio on a worker thread (tens of
    /// milliseconds) and install it into the renderer; until then the look
    /// draws with the direct lights only.
    fn bake_studio(rs: &egui_wgpu::RenderState, ctx: &egui::Context) {
        let (rs, ctx) = (rs.clone(), ctx.clone());
        let preset = lighting::STUDIO;
        let spawned = std::thread::Builder::new()
            .name("pcb-3d-studio".into())
            .spawn(move || {
                let env = StudioEnvironment::bake(preset);
                let mut renderer = rs.renderer.write();
                if let Some(r) = renderer.callback_resources.get_mut::<Renderer>() {
                    r.set_studio_environment(&rs.device, &rs.queue, &env);
                }
                drop(renderer);
                ctx.request_repaint();
            });
        if let Err(error) = spawned {
            eprintln!("pcbcraft: the 3D studio was not baked: {error}");
        }
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
            generation: 0,
            repack: None,
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
            aspect: 1.6,
            fit_pending: false,
            pivot: None,
            hide_substrate: false,
            display: pcbcraft_core::geometry::Transform3::IDENTITY,
            model_bounds: Self::initial_bounds(),
            overlay_bounds: Aabb3::EMPTY,
            display_limited: false,
            pv_key: 0,
            pv_slot: Arc::new(Mutex::new(None)),
            rect: None,
        };
        result.fit();
        result
    }

    /// A placeholder until the first model arrives, which is fitted again.
    fn initial_bounds() -> Aabb3 {
        Aabb3 {
            min: Vec3::new(0.0, 0.0, -0.1),
            max: Vec3::new(40.0, 30.0, 1.7),
        }
    }

    /// The camera the view is drawn with.
    pub fn camera(&self) -> Camera {
        self.camera
    }

    /// Show the view from `camera` (a framing kept from earlier).
    pub fn set_camera(&mut self, camera: Camera) {
        self.camera = camera;
        self.fit_pending = false;
    }

    /// Fit the model to the view now, and again on the next draw, when the
    /// view's shape is known.
    pub fn fit(&mut self) {
        self.fit_to(self.aspect);
        self.fit_pending = true;
    }

    /// Frame the bounds' eight corners inside both the vertical and the
    /// horizontal field of view, with [`FIT_MARGIN`] to spare.
    fn fit_to(&mut self, aspect: f64) {
        let b = self.bounds;
        if b.is_empty() || !b.min.is_finite() || !b.max.is_finite() {
            return;
        }
        let aspect = if aspect.is_finite() && aspect > 1e-3 {
            aspect
        } else {
            1.0
        };
        self.camera.target = b.center();
        let (right, up, back) = self.camera.basis();
        let keep = 1.0 - FIT_MARGIN;
        let corners = (0..8).map(|k| {
            Vec3::new(
                if k & 1 == 0 { b.min.x } else { b.max.x },
                if k & 2 == 0 { b.min.y } else { b.max.y },
                if k & 4 == 0 { b.min.z } else { b.max.z },
            ) - b.center()
        });
        if self.camera.fov > 0.0 {
            let vertical = (self.camera.fov * 0.5).tan() * keep;
            let horizontal = vertical * aspect;
            let distance = corners
                .map(|v| {
                    let toward = v.dot(back);
                    (toward + v.dot(right).abs() / horizontal)
                        .max(toward + v.dot(up).abs() / vertical)
                })
                .fold(1e-3, f64::max);
            self.camera.distance = distance;
        } else {
            // Solidcraft's parallel view shows 0.4 × distance above the centre.
            let half = corners
                .map(|v| (v.dot(up).abs() / keep).max(v.dot(right).abs() / (aspect * keep)))
                .fold(1e-3, f64::max);
            self.camera.distance = half / 0.4;
        }
    }

    /// Show a model built on the worker. Its PBR bytes move into the slot.
    pub fn set_model(&mut self, mut model: ModelScene) {
        if !model.bounds.is_empty() {
            self.model_bounds = model.bounds;
        }
        let mut pbr = std::mem::take(&mut model.pbr);
        if self.hide_substrate {
            pbr.truncate(model.parts_bytes);
        }
        self.model = Some(model);
        self.generation = self.generation.wrapping_add(1);
        self.repack = None;
        self.publish_pbr(pbr);
        self.shadow_view = None;
        // A new model is shown as built; the Part view sets its placement
        // transform again before drawing.
        self.display = pcbcraft_core::geometry::Transform3::IDENTITY;
        self.refresh_bounds();
        self.publish_shadows();
        self.publish_highlight();
    }

    pub fn hide_substrate(&self) -> bool {
        self.hide_substrate
    }

    /// Show or hide the board-level bodies. Shadows and selection change at
    /// once; the model's vertices are repacked on a background thread.
    pub fn set_hide_substrate(&mut self, hidden: bool) {
        if self.hide_substrate == hidden {
            return;
        }
        self.hide_substrate = hidden;
        self.shadow_view = None;
        self.publish_shadows();
        self.publish_highlight();
        let Some(model) = &self.model else {
            return;
        };
        let geometry: Vec<Geometry> = model.geometry.clone();
        let (send, result) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("pcb-3d-repack".into())
            .spawn(move || {
                let packed = model_scene::pack(&geometry, !hidden).map(|(bytes, _)| bytes);
                let _ = send.send(packed);
            });
        self.repack = match spawned {
            Ok(_) => Some(Repack {
                generation: self.generation,
                hidden,
                result,
            }),
            Err(_) => None,
        };
    }

    /// Take a finished repack if it is still current; whether one is due.
    fn receive_repack(&mut self) -> bool {
        let Some(repack) = &self.repack else {
            return false;
        };
        match repack.result.try_recv() {
            Ok(Ok(bytes))
                if repack.generation == self.generation && repack.hidden == self.hide_substrate =>
            {
                self.repack = None;
                self.publish_pbr(bytes);
                false
            }
            Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                self.repack = None;
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
        }
    }

    fn publish(slot: &SceneSlot, key: &mut u64, scene: GpuScene) {
        if let Ok(mut slot) = slot.lock() {
            *slot = Some(scene);
            *key = next_key();
        }
    }

    fn publish_pbr(&mut self, pbr: Vec<u8>) {
        let scene = GpuScene {
            pbr_detailed: pbr,
            ..GpuScene::default()
        };
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
        let display = self.display;
        let mut scene = GpuScene::default();
        if let Some(model) = self.model.as_ref().filter(|_| !hidden) {
            let mut add = |vertices: &[ShadowVertex]| {
                for (p, weight) in vertices {
                    let alpha = (peak * weight.clamp(0.0, 1.0)).round() as u8;
                    let color = scene.color([0, 0, 0, alpha]);
                    scene.trans_tri(display::point(&display, *p), [0.0; 3], color);
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

    /// The nearest shown body under a pixel of a `width` × `height` view
    /// and the distance along the pointer ray.
    fn hit(&self, x: f64, y: f64, width: f64, height: f64) -> Option<(f64, &Geometry)> {
        let model = self.model.as_ref()?;
        let (origin, direction) = self.camera.ray(x, y, width, height);
        let (origin, direction) = self.model_ray(origin, direction);
        model
            .geometry
            .iter()
            .filter(|g| !(self.hide_substrate && g.kind.is_board()))
            .filter(|g| box_hit(g, origin, direction))
            .filter_map(|g| g.mesh.raycast(origin, direction).map(|(d, _)| (d, g)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// The pick identity under a pixel of a `width` × `height` view.
    pub fn pick(&self, x: f64, y: f64, width: f64, height: f64) -> Option<u64> {
        self.hit(x, y, width, height).and_then(|(_, g)| g.item)
    }

    /// The model point under a pixel, or where the pointer ray meets the
    /// board's mid-plane: the anchor for zooming and orbiting.
    fn surface_point(&self, x: f64, y: f64, width: f64, height: f64) -> Option<Vec3> {
        let (origin, direction) = self.camera.ray(x, y, width, height);
        if let Some((d, _)) = self.hit(x, y, width, height) {
            // A rigid display transform keeps distances along the ray.
            return Some(origin + direction * d);
        }
        let plane = self.bounds.center().z;
        let t = (plane - origin.z) / direction.z;
        (t.is_finite() && t > 0.0).then(|| origin + direction * t)
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

    /// The frame drawn for a view of `size_px` pixels.
    pub(crate) fn frame(&self, view_proj: [[f32; 4]; 4], size_px: [f32; 2], ssaa: f32) -> Frame {
        let eye = self.camera.eye().to_f32();
        Frame {
            product_lights: Some(lighting::studio(&self.camera, &self.bounds)),
            product_look: Some(lighting::look()),
            product: false,
            key: self.key,
            slot: self.slot.clone(),
            sketch_key: self.sketch_key,
            sketch_slot: self.sketch_slot.clone(),
            hl_key: self.hl_key,
            hl_slot: self.hl_slot.clone(),
            pv_key: self.pv_key,
            pv_slot: if self.pv_key == 0 {
                self.empty.clone()
            } else {
                self.pv_slot.clone()
            },
            pv_hide: self.pv_key == 0,
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
            ao_radius: lighting::ao_radius(&self.bounds),
            ao_samples: AO_SAMPLES[usize::from(ssaa <= 1.0)],
            ssaa,
            hover_tint: [0.1; 3],
            selection_tint: [0.8, 0.7, 0.1],
        }
    }

    /// Draw the view; `Some(pick)` when it was clicked.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Option<u64>> {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return None;
        }
        let (w, h) = (f64::from(rect.width()), f64::from(rect.height()));
        self.aspect = w / h;
        self.rect = Some(rect);
        if self.fit_pending {
            self.fit_pending = false;
            self.fit_to(self.aspect);
        }
        if self.receive_repack() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }
        let local = |p: egui::Pos2| (f64::from(p.x - rect.left()), f64::from(p.y - rect.top()));
        if response.dragged_by(egui::PointerButton::Middle) {
            let d = ui.input(|i| i.pointer.delta());
            self.camera.pan(f64::from(d.x), f64::from(d.y), h);
        }
        if response.drag_started_by(egui::PointerButton::Secondary) {
            self.pivot = ui.input(|i| i.pointer.press_origin()).and_then(|p| {
                let (x, y) = local(p);
                self.surface_point(x, y, w, h)
            });
        }
        if response.dragged_by(egui::PointerButton::Secondary) {
            let d = ui.input(|i| i.pointer.delta());
            let (dx, dy) = (f64::from(d.x), f64::from(d.y));
            match self.pivot {
                Some(pivot) => self.camera.orbit_about(dx, dy, pivot),
                None => self.camera.orbit(dx, dy),
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let anchor = response.hover_pos().and_then(|p| {
                    let (x, y) = local(p);
                    self.surface_point(x, y, w, h)
                });
                // Scrolling up zooms in, as in Solidcraft and the 2D
                // canvases: the camera distance shrinks.
                self.camera
                    .zoom_at((-f64::from(scroll) * 0.002).exp(), anchor);
            }
        }
        if self.shadow_view != Some(self.current_shadow_view()) {
            self.publish_shadows();
        }
        Self::background(ui, rect);
        let pp = ui.ctx().pixels_per_point();
        let size_px = [rect.width() * pp, rect.height() * pp];
        let view_proj = self
            .camera
            .view_proj_bounds(f64::from(rect.width() / rect.height()), &self.bounds)
            .to_f32();
        let ssaa = self.supersampling(ui.ctx(), &view_proj, size_px);
        let cb = ViewportCallback(self.frame(view_proj, size_px, ssaa));
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, cb));
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (x, y) = local(p);
            return Some(self.pick(x, y, w, h));
        }
        None
    }
}

/// Whether the ray meets a body's bounding box ahead of its origin: a cheap
/// test before the triangles.
fn box_hit(g: &Geometry, origin: Vec3, direction: Vec3) -> bool {
    let (o, d) = (
        [origin.x, origin.y, origin.z],
        [direction.x, direction.y, direction.z],
    );
    let (mut enter, mut leave) = (0.0_f64, f64::INFINITY);
    for k in 0..3 {
        if d[k].abs() < 1e-15 {
            if o[k] < g.min[k] || o[k] > g.max[k] {
                return false;
            }
            continue;
        }
        let a = (g.min[k] - o[k]) / d[k];
        let b = (g.max[k] - o[k]) / d[k];
        enter = enter.max(a.min(b));
        leave = leave.min(a.max(b));
    }
    enter <= leave + 1e-9
}

mod display;
#[cfg(test)]
mod pictures;

#[cfg(test)]
mod tests {
    use super::*;

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
            .and_then(|s| s.as_ref().map(|s| s.pbr_detailed.len()))
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
        viewport.set_hide_substrate(true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while viewport.receive_repack() {
            if std::time::Instant::now() > deadline {
                return Err("the repack did not finish".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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

    /// The bounds' corners on screen, as shares of the view (0..1 inside).
    fn corner_shares(viewport: &Viewport, w: f64, h: f64) -> Result<Vec<(f64, f64)>, String> {
        let b = viewport.bounds;
        (0..8)
            .map(|corner| {
                let p = Vec3::new(
                    if corner & 1 == 0 { b.min.x } else { b.max.x },
                    if corner & 2 == 0 { b.min.y } else { b.max.y },
                    if corner & 4 == 0 { b.min.z } else { b.max.z },
                );
                viewport
                    .camera
                    .to_screen(p, w, h, b.diagonal())
                    .map(|(x, y, _)| (x / w, y / h))
                    .ok_or_else(|| "behind the eye".to_string())
            })
            .collect()
    }

    #[test]
    fn fit_frames_the_whole_board_in_wide_and_narrow_views()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = demo()?;
        for (w, h) in [
            (1500.0, 800.0),
            (830.0, 900.0),
            (400.0, 900.0),
            (1600.0, 300.0),
        ] {
            for (yaw, pitch) in [(0.0, 0.6), (0.785, 0.615), (2.0, 1.5), (4.0, -0.4)] {
                viewport.camera.yaw = yaw;
                viewport.camera.pitch = pitch;
                viewport.fit_to(w / h);
                let shares = corner_shares(&viewport, w, h)?;
                let inside = |v: f64| (0.0..=1.0).contains(&v);
                assert!(
                    shares.iter().all(|(x, y)| inside(*x) && inside(*y)),
                    "{w}×{h} at {yaw}/{pitch}: {shares:?}"
                );
                // Snug: the widest corner reaches the margin on one axis.
                let reach = shares
                    .iter()
                    .flat_map(|(x, y)| [(x - 0.5).abs(), (y - 0.5).abs()])
                    .fold(0.0, f64::max);
                assert!(reach > 0.40 && reach <= 0.5, "{w}×{h}: {reach}");
            }
        }
        Ok(())
    }

    #[test]
    fn scrolling_zooms_toward_the_point_under_the_pointer() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut viewport = demo()?;
        let (w, h) = (800.0, 600.0);
        let (x, y) = (610.0, 220.0);
        let anchor = viewport.surface_point(x, y, w, h).ok_or("no anchor")?;
        let radius = viewport.bounds.diagonal();
        for factor in [0.5, 0.5, 1.7] {
            viewport.camera.zoom_at(factor, Some(anchor));
            let (sx, sy, _) = viewport
                .camera
                .to_screen(anchor, w, h, radius)
                .ok_or("behind the eye")?;
            assert!((sx - x).abs() < 1e-6 && (sy - y).abs() < 1e-6, "{sx}, {sy}");
        }
        Ok(())
    }

    #[test]
    fn the_perspective_view_fits_and_picks_the_part_under_the_pointer()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = demo()?;
        assert!(viewport.camera.fov > 0.0);
        let (w, h) = (800.0, 600.0);
        viewport.fit_to(w / h);
        let radius = viewport.bounds.diagonal();
        for (x, y) in corner_shares(&viewport, w, h)? {
            assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
        }
        let model = viewport.model.as_ref().ok_or("no model")?;
        let body = model
            .geometry
            .iter()
            .find(|g| g.kind.is_envelope() && g.min[2] > 1.0)
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
        let (x, y, _) = viewport
            .camera
            .to_screen(Vec3::new(0.5, 0.5, 1.6), w, h, radius)
            .ok_or("behind the eye")?;
        assert_eq!(viewport.pick(x, y, w, h), None, "laminate has no item");
        assert!(viewport.hit(x, y, w, h).is_some(), "the board is hit");
        viewport.set_hide_substrate(true);
        assert!(
            viewport.hit(x, y, w, h).is_none(),
            "hidden board is not hit"
        );
        Ok(())
    }
}
