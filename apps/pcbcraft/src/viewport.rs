//! Solidcraft's GPU renderer and camera over cached geometry. Solidcraft's full
//! navigation handler is private to its app, so [`Viewport::navigate`]
//! reproduces its default bindings (Fusion, two-finger panning on): two
//! fingers pan, Shift+two fingers orbit, a pinch or the wheel zooms toward
//! the point under the pointer, middle-drag pans, right-drag and Shift+middle
//! orbit about the surface under the pointer, Space+drag pans, Cmd/Ctrl+drag
//! on empty space orbits, the arrows turn the view and a middle double-click
//! fits (see [`crate::navigation`]).
//!
//! The model is drawn through Solidcraft's PBR path with its opt-in product
//! look (a studio environment reflected and lighting from all around, clear
//! coats, procedural patterns, ambient occlusion, PBR Neutral tone mapping
//! and dither) and the direct lights in [`crate::lighting`]. The studio is
//! baked once on a worker thread and installed into the renderer, so the
//! window never waits for it. Three slots are published separately so each change
//! uploads only what it touches: the model's PBR vertices (`slot`: a new
//! model, moved in without a copy, or a change of the [`LayerView`], whose
//! shown layers are packed again off the UI thread), shadows (`sketch_slot`:
//! theme, side or layers) and the selection (`hl_slot`: cached flat blocks
//! recoloured, hidden layers' blocks left out). Layer visibility is display
//! state: it never rebuilds the model's geometry, and picking skips hidden
//! layers. Adaptive
//! supersampling follows Solidcraft's viewport `supersampling` and
//! `view_settled` (MIT, revision 4f3ac07; see ATTRIBUTION.md).
use crate::layer_view::{LayerView, ViewLayer};
use crate::lighting;
use crate::model_scene::{self, ModelScene, ShadowVertex, next_key};
use pcbcraft_solidcraft::{
    geom::{Aabb3, Vec3, prelude::*},
    scene::{Geometry, Place},
    viewport::Camera,
};
use pcbcraft_ui::theme;
use solidcraft_viewport_egui::ViewportCallback;
use solidcraft_viewport_gpu::view::{
    self as gpu, Frame, GpuScene, GpuTarget, Palette, Renderer, SceneSlot, StudioEnvironment,
};
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
/// The selected object's tint over the model, stronger than its net's
/// ([`HIGHLIGHT_ALPHA`]) as Solidcraft's selection is than its pre-highlight.
const SELECTED_ALPHA: u8 = 215;
/// A selected object smaller than this on screen (points) also gets a
/// ring: a via or a small pad reads as selected at any zoom, also where
/// the mask covers its tint.
const SMALL_SELECTION_PT: f64 = 24.0;
/// The ring's least radius and its stroke (points).
const RING_RADIUS_PT: f64 = 9.0;
const RING_STROKE_PT: f32 = 2.0;
/// Peak shadow opacity on light and dark backgrounds (Solidcraft's values).
const SHADOW_ALPHA: [f32; 2] = [80.0, 150.0];
/// Ambient occlusion samples per pixel for a still and a moving view
/// (Solidcraft's `AO_SAMPLES` and `AO_SAMPLES_MOVING`).
const AO_SAMPLES: [u32; 2] = [32, 8];

/// What the shadow slot was last built for: dark theme, seen from below,
/// and which shadows the layers allow ([`ShadowLayers`]).
type ShadowView = (bool, bool, ShadowLayers);

/// The shadows the shown layers leave: the ground shadow with the laminate,
/// and the contact shadows of each side's packages with the bodies and a
/// surface under them on that side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShadowLayers {
    ground: bool,
    top: bool,
    bottom: bool,
}

impl ShadowLayers {
    fn of(view: &LayerView) -> Self {
        use pcbcraft_core::Layer;
        let board = view.visible(ViewLayer::Substrate);
        let bodies = view.visible(ViewLayer::Bodies);
        let side = |side| bodies && (board || view.visible(ViewLayer::Mask(side)));
        Self {
            ground: board,
            top: side(Layer::Top),
            bottom: side(Layer::Bottom),
        }
    }
}

/// A PBR repack running off the UI thread: the model generation and layer
/// view it is for.
struct Repack {
    generation: u64,
    layers: LayerView,
    result: Receiver<Result<GpuScene, String>>,
}

pub struct Viewport {
    model: Option<ModelScene>,
    /// Counts models shown, so a late repack never lands on a newer one.
    generation: u64,
    repack: Option<Repack>,
    bounds: Aabb3,
    highlighted: Vec<u64>,
    /// The selected object's pick identities among `highlighted`, drawn in
    /// the strong selection tint (the rest are its net's).
    selected: Vec<u64>,
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
    /// While an orbit runs: the point it turns about (none: the target).
    pivot: Option<Option<Vec3>>,
    /// Two-finger scrolling across frames.
    trackpad: crate::navigation::Trackpad,
    /// What the left button's drag does, chosen when it was pressed.
    primary_nav: Option<NavMode>,
    /// The right button is past being a click and orbits.
    right_drag: bool,
    /// The left button's press is past being a click and pans or orbits.
    primary_drag: bool,
    /// Space is held for panning: it went down while no field had the
    /// keyboard and is still down. A space typed into a field never pans,
    /// even when the field's text arrives without the key's release (as
    /// synthetic typing sends it), which would leave egui's key down.
    space_held: bool,
    /// The last click was the middle button's (a middle double-click fits).
    middle_clicked: bool,
    /// The layers drawn and picked.
    layers: LayerView,
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
    /// Component 3D marking decals ([`decals`]).
    decals: decals::Decals,
    /// Where the view was last drawn.
    rect: Option<egui::Rect>,
    /// The left button went down on the navigation cube ([`nav_cube`]):
    /// where, and whether it has dragged (an orbit) since. The press point
    /// is kept, as egui forgets it on the release frame.
    cube_press: Option<(egui::Pos2, bool)>,
    /// Whether this view shows the navigation cube (PCB 3D).
    cube: bool,
    /// The Realistic look (the product look: studio, reflections and the
    /// materials' graphs), else Solidcraft's Standard modelling look. A view
    /// preference, on by default.
    realistic: bool,
    /// What a tool highlights under the pointer, with its block of the
    /// selection layer ([`surface`]).
    hovered: Option<(surface::Hovered, gpu::SceneBlock)>,
    /// Where the last click that picked landed (screen points).
    clicked_at: Option<egui::Pos2>,
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
        // Bodies come meshed (no GPU plan evaluation); GPU errors no scope
        // catches are logged by the viewport.
        solidcraft_viewport_egui::install(rs, scene, window, false, false, |_| {});
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

    /// Whether the view draws the Realistic look.
    pub fn realistic(&self) -> bool {
        self.realistic
    }

    /// Draw the Realistic look or Solidcraft's Standard one. Only the frame's
    /// look changes: no geometry is rebuilt.
    pub fn set_realistic(&mut self, on: bool) {
        self.realistic = on;
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
            selected: vec![],
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
            trackpad: Default::default(),
            primary_nav: None,
            right_drag: false,
            primary_drag: false,
            space_held: false,
            middle_clicked: false,
            layers: LayerView::default(),
            display: pcbcraft_core::geometry::Transform3::IDENTITY,
            model_bounds: Self::initial_bounds(),
            overlay_bounds: Aabb3::EMPTY,
            display_limited: false,
            pv_key: 0,
            pv_slot: Arc::new(Mutex::new(None)),
            decals: Default::default(),
            rect: None,
            cube_press: None,
            cube: false,
            realistic: true,
            hovered: None,
            clicked_at: None,
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
    /// A model of another size (the board made wider) that would leave the
    /// view where the last one was framed in it is fitted again; any other
    /// new model keeps the camera, and nothing else moves it.
    pub fn set_model(&mut self, mut model: ModelScene) {
        let board =
            self.model.is_some() && self.display == pcbcraft_core::geometry::Transform3::IDENTITY;
        let framed = board && self.frames(&self.bounds);
        let before = self.model_bounds;
        if !model.bounds.is_empty() {
            self.model_bounds = model.bounds;
        }
        let mut stream = std::mem::take(&mut model.stream);
        // Built with every layer. The material stream cannot drop runs in
        // place: another view shows plain faces until its repack lands.
        let every_layer = self.layers == LayerView::default();
        if !every_layer {
            model_scene::keep_visible(&mut stream.tris, &model.runs, &self.layers);
            stream.graphs = Default::default();
        }
        self.model = Some(model);
        // Its bodies are new: a hover over the last model's is gone.
        self.hovered = None;
        self.generation = self.generation.wrapping_add(1);
        self.repack = None;
        self.publish_stream(stream);
        if !every_layer {
            self.start_repack();
        }
        self.shadow_view = None;
        // A new model is shown as built; the Part view sets its placement
        // transform again before drawing.
        self.display = pcbcraft_core::geometry::Transform3::IDENTITY;
        self.refresh_bounds();
        if framed && self.model_bounds != before && !self.frames(&self.bounds) {
            self.fit();
        }
        self.publish_shadows();
        self.publish_highlight();
        self.model_changed_decals();
    }

    /// Whether every corner of `b` is inside the view, in the view's last
    /// shape.
    fn frames(&self, b: &Aabb3) -> bool {
        if b.is_empty() || !b.min.is_finite() || !b.max.is_finite() {
            return false;
        }
        let aspect = if self.aspect.is_finite() && self.aspect > 1e-3 {
            self.aspect
        } else {
            1.0
        };
        let (w, h) = (1000.0 * aspect, 1000.0);
        let radius = b.diagonal().max(self.bounds.diagonal());
        (0..8).all(|k| {
            let p = Vec3::new(
                if k & 1 == 0 { b.min.x } else { b.max.x },
                if k & 2 == 0 { b.min.y } else { b.max.y },
                if k & 4 == 0 { b.min.z } else { b.max.z },
            );
            self.camera
                .to_screen(p, w, h, radius)
                .is_some_and(|(x, y, _)| (0.0..=w).contains(&x) && (0.0..=h).contains(&y))
        })
    }

    #[cfg(test)]
    /// The layers drawn and picked.
    pub fn layers(&self) -> &LayerView {
        &self.layers
    }

    #[cfg(test)]
    /// Show or hide one layer ([`Viewport::set_layers`]).
    pub fn set_layer(&mut self, layer: ViewLayer, visible: bool) {
        if self.layers.visible(layer) != visible {
            let mut layers = self.layers.clone();
            layers.set(layer, visible);
            self.set_layers(layers);
        }
    }

    /// Draw and pick only the layers `layers` shows. Shadows and selection
    /// change at once; the model's shown bodies are packed again on a
    /// background thread from the cached meshes (nothing is rebuilt), and
    /// the last vertices stay up until then.
    pub fn set_layers(&mut self, layers: LayerView) {
        if self.layers == layers {
            return;
        }
        self.layers = layers;
        self.publish_shadows();
        self.publish_highlight();
        self.start_repack();
    }

    /// Pack the shown layers of the model again from its cached meshes on a
    /// background thread; [`Self::receive_repack`] publishes the result with
    /// the instanced meshes. The last published vertices stay up until then.
    pub(super) fn start_repack(&mut self) {
        let Some(model) = &self.model else {
            return;
        };
        let layers = self.layers.clone();
        let (geometry, decals) = if self.display == pcbcraft_core::geometry::Transform3::IDENTITY {
            (model.geometry.clone(), self.decals.geometry.clone())
        } else {
            (
                display::transformed(&model.geometry, &self.display),
                display::transformed(&self.decals.geometry, &self.display),
            )
        };
        let (send, result) = mpsc::channel();
        let view = layers.clone();
        let stack = model.stack.clone();
        let spawned = std::thread::Builder::new()
            .name("pcb-3d-repack".into())
            .spawn(move || {
                let packed =
                    model_scene::pack(&geometry, &decals, &view, &stack).map(|(scene, _)| scene);
                let _ = send.send(packed);
            });
        self.repack = match spawned {
            Ok(_) => Some(Repack {
                generation: self.generation,
                layers,
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
            Ok(Ok(stream))
                if repack.generation == self.generation && repack.layers == self.layers =>
            {
                self.repack = None;
                self.publish_stream(stream);
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

    /// Publish the model's opaque vertices and material stream `stream` with
    /// its placed bodies drawn instanced: the shown layers' definition meshes
    /// (the same shared triangles each time, so their GPU buffers are kept)
    /// at their placements under the display transform.
    fn publish_stream(&mut self, mut stream: GpuScene) {
        stream.instanced = self.instanced(&mut stream);
        Self::publish(&self.slot, &mut self.key, stream);
    }

    /// The display transform as a placement, or `None` for none.
    fn display_place(&self) -> Option<Place> {
        let t = &self.display;
        (*t != pcbcraft_core::geometry::Transform3::IDENTITY).then(|| {
            Place(std::array::from_fn(|r| {
                [
                    t.rotation[r][0],
                    t.rotation[r][1],
                    t.rotation[r][2],
                    t.translation[r],
                ]
            }))
        })
    }

    /// The shown layers' instanced meshes, in the colours of `scene`'s palette
    /// (the model's are in it by [`model_scene::palette_of`]; decals' join it).
    fn instanced(&self, scene: &mut GpuScene) -> Vec<gpu::InstancedMesh> {
        let Some(model) = &self.model else {
            return Vec::new();
        };
        let display = self.display_place();
        let shown = |i: &&model_scene::Instanced| self.layers.draws(i.layer);
        let mut out = Vec::new();
        for i in model.instanced.iter().filter(shown) {
            let mesh = match display {
                // Unmoved: the placements as built, without a pass over them.
                None => i.mesh.clone(),
                Some(_) => {
                    let color = scene.color(i.colour);
                    i.placed(
                        &model.geometry,
                        display.as_ref(),
                        i.mesh.look,
                        color,
                        |_| true,
                    )
                }
            };
            out.push(i.shaded(scene, mesh));
        }
        for i in self.decals.meshes.iter().filter(shown) {
            let color = scene.color(i.colour);
            let mesh = i.placed(
                &self.decals.geometry,
                display.as_ref(),
                i.mesh.look,
                color,
                |_| true,
            );
            out.push(i.shaded(scene, mesh));
        }
        out
    }

    fn current_shadow_view(&self) -> ShadowView {
        (
            theme::is_dark(),
            self.camera.pitch < 0.0,
            ShadowLayers::of(&self.layers),
        )
    }

    /// Rebuild the shadow slot when the theme, the side seen or the shown
    /// layers changed. The model's PBR buffer is not touched.
    fn publish_shadows(&mut self) {
        let view = self.current_shadow_view();
        if self.shadow_view == Some(view) {
            return;
        }
        self.shadow_view = Some(view);
        let (dark, below, shown) = view;
        let peak = SHADOW_ALPHA[usize::from(dark)];
        let display = self.display;
        let mut scene = GpuScene::default();
        if let Some(model) = self.model.as_ref() {
            let mut add = |vertices: &[ShadowVertex]| {
                for (p, weight) in vertices {
                    let alpha = (peak * weight.clamp(0.0, 1.0)).round() as u8;
                    let color = scene.color([0, 0, 0, alpha]);
                    scene.trans_tri(display::point(&display, *p), [0.0; 3], color);
                }
            };
            if below {
                if shown.bottom {
                    add(&model.shadows.bottom);
                }
            } else {
                if shown.ground {
                    add(&model.shadows.ground);
                }
                if shown.top {
                    add(&model.shadows.top);
                }
            }
        }
        Self::publish(&self.sketch_slot, &mut self.sketch_key, scene);
    }

    #[cfg(test)]
    pub fn highlight(&mut self, ids: Vec<u64>) {
        self.highlight_selected(ids, Vec::new());
    }

    /// [`Self::highlight`] with `selected` (the picked object's identities,
    /// also in `ids`) drawn in a stronger tint than the rest (its net).
    pub fn highlight_selected(&mut self, ids: Vec<u64>, selected: Vec<u64>) {
        if self.highlighted != ids || self.selected != selected {
            self.highlighted = ids;
            self.selected = selected;
            self.publish_highlight();
        }
    }

    /// The selection: cached flat blocks of the highlighted items in the
    /// selection colour. Model geometry and its key stay as they are.
    fn publish_highlight(&mut self) {
        let selected = crate::palette::board().selected;
        let rgba = [selected.r(), selected.g(), selected.b(), HIGHLIGHT_ALPHA];
        let strong = [selected.r(), selected.g(), selected.b(), SELECTED_ALPHA];
        // Solidcraft's pre-highlight: the face lifted, its outline in the
        // hover edge colour ([`surface`]).
        let t = theme::Tokens::get();
        // Over the lit model, the lift is white at about three times its
        // share.
        let face = [255, 255, 255, t.hover_face_lift.saturating_mul(3)];
        let edge = t.hover_edge;
        let edge = [edge.r(), edge.g(), edge.b(), 255];
        let mut scene =
            GpuScene::with_palette(Palette::with_stable(vec![rgba, strong, face, edge]));
        if let Some(model) = &self.model {
            scene.blocks = model
                .highlight
                .iter()
                .filter(|(item, layer, _)| {
                    self.highlighted.contains(item) && self.layers.draws(*layer)
                })
                .map(|(_, _, block)| block.clone())
                .collect();
            if !self.highlighted.is_empty() {
                let highlighted: std::collections::HashSet<u64> =
                    self.highlighted.iter().copied().collect();
                let selected: std::collections::HashSet<u64> =
                    self.selected.iter().copied().collect();
                let display = self.display_place();
                scene.selected_instanced = model
                    .instanced
                    .iter()
                    .filter(|i| self.layers.draws(i.layer))
                    .filter_map(|i| {
                        i.highlight(&model.geometry, display.as_ref(), &highlighted, &selected)
                    })
                    .collect();
            }
        }
        if let Some((_, block)) = &self.hovered {
            scene.blocks.push(block.clone());
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
            .filter(|g| self.layers.shows(g))
            .filter(|g| box_hit(g, origin, direction))
            .filter_map(|g| g.raycast(origin, direction).map(|(d, _)| (d, g)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// The rings marking selected objects that are small on screen in a
    /// `width` × `height` view: each one's centre and radius in view
    /// points. Shown layers only; a net's other objects get none.
    fn selection_rings(&self, width: f64, height: f64) -> Vec<([f64; 2], f64)> {
        let Some(model) = self.model.as_ref().filter(|_| !self.selected.is_empty()) else {
            return Vec::new();
        };
        let mut boxes: Vec<(u64, Aabb3)> = Vec::new();
        for g in model.geometry.iter().filter(|g| self.layers.shows(g)) {
            let Some(item) = g.item.filter(|i| self.selected.contains(i)) else {
                continue;
            };
            let at = match boxes.iter().position(|(i, _)| *i == item) {
                Some(at) => at,
                None => {
                    boxes.push((item, Aabb3::EMPTY));
                    boxes.len() - 1
                }
            };
            if let Some((_, b)) = boxes.get_mut(at) {
                b.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
                b.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
            }
        }
        let radius = self.bounds.diagonal();
        boxes
            .into_iter()
            .filter_map(|(_, b)| {
                let (lo, hi) = ([b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z]);
                let mut screen: Option<[f64; 4]> = None;
                for k in 0..8 {
                    let corner =
                        std::array::from_fn(|i| if (k >> i) & 1 == 0 { lo[i] } else { hi[i] });
                    let [x, y, z] = self.display.apply(corner);
                    let (sx, sy, _) =
                        self.camera
                            .to_screen(Vec3::new(x, y, z), width, height, radius)?;
                    let s = screen.get_or_insert([sx, sy, sx, sy]);
                    *s = [s[0].min(sx), s[1].min(sy), s[2].max(sx), s[3].max(sy)];
                }
                let [x0, y0, x1, y1] = screen?;
                let size = (x1 - x0).max(y1 - y0);
                (size < SMALL_SELECTION_PT).then(|| {
                    let half = 0.5 * (x1 - x0).hypot(y1 - y0);
                    (
                        [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
                        (half + 3.0).max(RING_RADIUS_PT),
                    )
                })
            })
            .collect()
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
            product_lights: self
                .realistic
                .then(|| lighting::studio(&self.camera, &self.bounds)),
            product_look: self.realistic.then(lighting::look),
            units_per_mm: 1.0,
            moving: None,
            preview_bounds: None,
            product: false,
            upload_error: None,
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
            clouds: vec![],
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
            hatch: Default::default(),
            sky: [0.0; 4],
        }
    }

    /// Draw the view; `Some(pick)` when it was clicked. `caps` says what
    /// the left button and the keys may do to the camera there.
    pub fn show(&mut self, ui: &mut egui::Ui, caps: NavCaps) -> Option<Option<u64>> {
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
        // Received decals may start a repack (`publish_instanced`): look
        // again until it lands.
        if self.receive_repack() | self.receive_decals() || self.repack.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }
        let local = |p: egui::Pos2| (f64::from(p.x - rect.left()), f64::from(p.y - rect.top()));
        self.cube = caps.cube;
        let cube_owns = caps.cube && self.cube_input(ui, rect);
        self.navigate(ui, rect, &response, caps);
        if self.shadow_view != Some(self.current_shadow_view()) {
            self.publish_shadows();
        }
        Self::background(ui, rect);
        let pp = ui.ctx().pixels_per_point();
        let size_px = [rect.width() * pp, rect.height() * pp];
        let view_proj = self
            .camera
            .view_proj_bounds(
                f64::from(rect.width() / rect.height()),
                &pcbcraft_solidcraft::render::view_bounds(&self.bounds),
            )
            .to_f32();
        let ssaa = self.supersampling(ui.ctx(), &view_proj, size_px);
        let cb = ViewportCallback(self.frame(view_proj, size_px, ssaa));
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, cb));
        let ring = egui::Stroke::new(RING_STROKE_PT, crate::palette::board().selected);
        for ([x, y], r) in self.selection_rings(w, h) {
            let centre = rect.min + egui::vec2(x as f32, y as f32);
            ui.painter_at(rect).circle_stroke(centre, r as f32, ring);
        }
        if caps.cube {
            self.paint_cube(ui, rect);
        }
        // A press that ended as a click selects, Space or Cmd/Ctrl held or
        // not: navigation only begins once it is a drag.
        if response.clicked()
            && !cube_owns
            && let Some(p) = response.interact_pointer_pos()
        {
            let (x, y) = local(p);
            self.clicked_at = Some(p);
            return Some(self.pick(x, y, w, h));
        }
        None
    }

    /// Where the click [`Self::show`] last reported landed.
    pub fn clicked_at(&self) -> Option<egui::Pos2> {
        self.clicked_at
    }
}

/// What a camera drag does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavMode {
    Pan,
    Orbit,
}

/// What the left button and the keys may do to a 3D view's camera. The
/// trackpad, the wheel, the middle and right buttons always navigate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavCaps {
    /// Space+drag pans and Cmd/Ctrl+drag on empty space orbits. Off where
    /// the left button drags something of its own (the placement triad).
    pub primary: bool,
    /// The arrows turn the view and Cmd/Ctrl+arrows pan it. Off where the
    /// arrows nudge something.
    pub keys: bool,
    /// The navigation cube shows in the top-right corner ([`nav_cube`]).
    pub cube: bool,
}

impl NavCaps {
    /// PCB 3D: everything navigates.
    pub const BOARD: NavCaps = NavCaps {
        primary: true,
        keys: true,
        cube: true,
    };
    /// Component 3D: the left button and the arrows place the model.
    pub const PLACEMENT: NavCaps = NavCaps {
        primary: false,
        keys: false,
        cube: false,
    };
}

/// How far a tap of an arrow turns the view (degrees; Shift: a quarter turn).
const KEY_TURN_DEGREES: f64 = 15.0;
/// How far Cmd/Ctrl+arrow pans, as a share of the view's height.
const KEY_PAN_SHARE: f32 = 0.1;

impl Viewport {
    /// The model point under a screen position of the view at `rect`.
    fn surface_at(&self, rect: egui::Rect, p: egui::Pos2) -> Option<Vec3> {
        self.surface_point(
            f64::from(p.x - rect.left()),
            f64::from(p.y - rect.top()),
            f64::from(rect.width()),
            f64::from(rect.height()),
        )
    }

    /// Orbit by a pointer or finger motion about the surface point under
    /// `at`, chosen when the orbit begins.
    fn orbit_by(&mut self, rect: egui::Rect, d: egui::Vec2, at: Option<egui::Pos2>) {
        let pivot = match self.pivot {
            Some(pivot) => pivot,
            None => {
                let pivot = at.and_then(|p| self.surface_at(rect, p));
                self.pivot = Some(pivot);
                pivot
            }
        };
        let (dx, dy) = (f64::from(d.x), f64::from(d.y));
        match pivot {
            Some(pivot) => self.camera.orbit_about(dx, dy, pivot),
            None => self.camera.orbit(dx, dy),
        }
    }

    /// One frame of camera input over the view at `rect` (Solidcraft's
    /// `navigation::handle` with its default bindings).
    fn navigate(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        response: &egui::Response,
        caps: NavCaps,
    ) {
        let h = f64::from(rect.height());
        let wheel = self.trackpad.read(ui, true);
        let (hover, mods, middle, secondary_down) = ui.input(|i| {
            (
                i.pointer.hover_pos(),
                i.modifiers,
                i.pointer.middle_down(),
                i.pointer.secondary_down(),
            )
        });
        let (primary_down, primary_pressed, primary_released) = ui.input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
            )
        });
        let (delta, press, latest, could_click) = ui.input(|i| {
            (
                i.pointer.delta(),
                i.pointer.press_origin(),
                i.pointer.latest_pos(),
                i.pointer.could_any_button_be_click(),
            )
        });
        let inside = ui.is_enabled()
            && response.contains_pointer()
            && hover.is_some_and(|p| rect.contains(p));
        let on_cube = |p: egui::Pos2| caps.cube && nav_cube::owns(rect, p);
        let pressed_here = press.is_some_and(|p| rect.contains(p));
        let primary_here = press.is_some_and(|p| rect.contains(p) && !on_cube(p));
        self.middle_double_click(ui, inside);
        let mut orbiting = false;
        // Pinch and the wheel zoom toward the point under the pointer; two
        // fingers pan, and with Shift they orbit.
        if inside && wheel.moves() {
            let anchor = hover.and_then(|p| self.surface_at(rect, p));
            if wheel.pinch != 1.0 {
                self.camera.zoom_at(1.0 / f64::from(wheel.pinch), anchor);
            }
            if wheel.zoom != 0.0 {
                let per_point = f64::from(crate::navigation::ZOOM_PER_POINT);
                self.camera
                    .zoom_at((-f64::from(wheel.zoom) * per_point).exp(), anchor);
            }
            if wheel.pan != egui::Vec2::ZERO {
                self.camera
                    .pan(f64::from(wheel.pan.x), f64::from(wheel.pan.y), h);
            }
            if wheel.orbit != egui::Vec2::ZERO {
                self.orbit_by(rect, wheel.orbit, hover);
                orbiting = true;
            }
        }
        // A left press with Space held pans, and with Cmd/Ctrl on empty
        // space orbits, until the button is let go. Either starts only once
        // the press can no longer be a click, so a click selects and never
        // moves the camera.
        let typing = ui.ctx().egui_wants_keyboard_input();
        let (space_pressed, space_down) = ui.input(|i| {
            let pressed = i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Space,
                        pressed: true,
                        ..
                    }
                )
            });
            (pressed, i.key_down(egui::Key::Space))
        });
        self.space_held = space_down && (self.space_held || (space_pressed && !typing));
        let space = caps.primary && !typing && self.space_held;
        if primary_pressed && inside && primary_here {
            let command = (mods.command || mods.ctrl) && !mods.shift && !mods.alt;
            let empty = |viewport: &Self| {
                press.is_some_and(|p| {
                    let (x, y) = (f64::from(p.x - rect.left()), f64::from(p.y - rect.top()));
                    viewport.pick(x, y, f64::from(rect.width()), h).is_none()
                })
            };
            self.primary_nav = if space {
                Some(NavMode::Pan)
            } else if caps.primary && command && empty(self) {
                Some(NavMode::Orbit)
            } else {
                None
            };
        } else if !primary_down && !primary_released {
            self.primary_nav = None;
        }
        if space && (inside || primary_down) {
            ui.ctx().set_cursor_icon(if primary_down {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        // The right button also clicks, so it orbits only once the press can
        // no longer be one; that frame catches up with the whole way from
        // the press.
        // The left button's Space pan and Cmd/Ctrl orbit wait the same way.
        let right_drag = secondary_down && pressed_here && !could_click;
        let primary_mode = if space {
            Some(NavMode::Pan)
        } else {
            self.primary_nav
        };
        let primary_drag = primary_down && primary_here && !could_click && primary_mode.is_some();
        let delta = match (press, latest) {
            (Some(from), Some(to)) if right_drag && !self.right_drag && !middle => to - from,
            (Some(from), Some(to))
                if primary_drag && !self.primary_drag && !middle && !right_drag =>
            {
                to - from
            }
            _ => delta,
        };
        self.right_drag = right_drag;
        self.primary_drag = primary_drag;
        let mode = if middle && pressed_here {
            let shift = mods.shift && !mods.command && !mods.ctrl && !mods.alt;
            Some(if shift { NavMode::Orbit } else { NavMode::Pan })
        } else if right_drag {
            Some(NavMode::Orbit)
        } else if primary_drag {
            primary_mode
        } else {
            None
        };
        if delta.is_finite() && delta != egui::Vec2::ZERO {
            match mode {
                Some(NavMode::Pan) => self.camera.pan(f64::from(delta.x), f64::from(delta.y), h),
                Some(NavMode::Orbit) => self.orbit_by(rect, delta, press),
                None => {}
            }
        }
        orbiting |= mode == Some(NavMode::Orbit);
        if !orbiting && !self.trackpad.in_gesture() {
            self.pivot = None;
        }
        if caps.keys && inside {
            self.view_keys(ui, rect);
        }
        if !self.camera.is_valid() {
            self.camera = Camera {
                fov: FIELD_OF_VIEW,
                ..Camera::default()
            };
            self.fit();
        }
    }

    /// A middle-button double-click fits the view. Only clicks count: a
    /// press that moved (a pan) is not one, and both clicks must be the
    /// middle button's.
    fn middle_double_click(&mut self, ui: &egui::Ui, inside: bool) {
        use egui::PointerButton::{Extra1, Extra2, Middle, Primary, Secondary};
        let (double, releases, middle, other, dragging) = ui.input(|i| {
            (
                i.pointer.button_double_clicked(Middle),
                i.events
                    .iter()
                    .filter(|e| {
                        matches!(
                            e,
                            egui::Event::PointerButton {
                                button: Middle,
                                pressed: false,
                                ..
                            }
                        )
                    })
                    .count(),
                i.pointer.button_clicked(Middle),
                [Primary, Secondary, Extra1, Extra2]
                    .into_iter()
                    .any(|b| i.pointer.button_clicked(b)),
                i.pointer.is_decidedly_dragging(),
            )
        });
        let go = double && inside && !other && (self.middle_clicked || releases >= 2);
        self.middle_clicked = !(other || dragging || go) && (middle || self.middle_clicked);
        if go {
            self.fit();
        }
    }

    /// The view keys while the pointer is over the view and nothing else
    /// has the keyboard: an arrow turns the view 15° (Shift: a quarter
    /// turn), Cmd/Ctrl+arrow pans it a tenth of its height.
    fn view_keys(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        let ctx = ui.ctx();
        if ctx.egui_wants_keyboard_input()
            || ctx.memory(|m| m.focused().is_some())
            || ctx.any_popup_open()
        {
            return;
        }
        let keys: Vec<(egui::Key, egui::Modifiers)> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        for (key, mods) in keys {
            if mods.alt {
                continue;
            }
            let (dx, dy) = match key {
                egui::Key::ArrowLeft => (-1.0, 0.0),
                egui::Key::ArrowRight => (1.0, 0.0),
                egui::Key::ArrowUp => (0.0, -1.0),
                egui::Key::ArrowDown => (0.0, 1.0),
                _ => continue,
            };
            if mods.command || mods.ctrl {
                // The model moves with the arrow, as under a pan drag.
                let step = rect.height() * KEY_PAN_SHARE;
                self.camera.pan(
                    f64::from(dx * step),
                    f64::from(dy * step),
                    f64::from(rect.height()),
                );
            } else {
                let degrees = if mods.shift { 90.0 } else { KEY_TURN_DEGREES };
                let (sin, cos) = f64::to_radians(degrees).sin_cos();
                let (right, up, back) = self.camera.basis();
                let toward = right * f64::from(dx) - up * f64::from(dy);
                self.camera = self.camera.looking_from(back * cos + toward * sin);
            }
        }
    }
}

/// Standard views and the navigation cube.
impl Viewport {
    /// Look at the model from a standard view, keeping the target and
    /// distance; the isometric (home) view also fits.
    pub fn standard_view(&mut self, view: pcbcraft_solidcraft::viewport::StandardView) {
        self.camera.set_view(view);
        if view == pcbcraft_solidcraft::viewport::StandardView::Iso {
            self.fit();
        }
    }

    /// Look at the model from `direction` (toward the eye).
    pub fn view_from(&mut self, direction: Vec3) {
        if direction.is_finite() && direction.dot(direction) > 0.0 {
            self.camera = self.camera.looking_from(direction);
        }
    }

    /// Frame the bodies with pick identities `items` (the selection), so
    /// orbiting turns about them; `false` when none of them is drawn.
    pub fn fit_items(&mut self, items: &[u64]) -> bool {
        let Some(model) = &self.model else {
            return false;
        };
        let mut bounds = Aabb3::EMPTY;
        for g in model
            .geometry
            .iter()
            .filter(|g| g.item.is_some_and(|i| items.contains(&i)))
        {
            bounds.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
            bounds.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
        }
        if bounds.is_empty() {
            return false;
        }
        let whole = self.bounds;
        self.bounds = bounds;
        self.fit_to(self.aspect);
        self.bounds = whole;
        self.fit_pending = false;
        true
    }

    /// One frame of the left button on the navigation cube: a click looks
    /// along the face or corner under it (or goes home), a drag orbits.
    /// Whether the button's press belongs to the cube this frame.
    fn cube_input(&mut self, ui: &egui::Ui, rect: egui::Rect) -> bool {
        let (pressed, down, released, press, delta, could_click) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.press_origin().or(i.pointer.interact_pos()),
                i.pointer.delta(),
                i.pointer.could_any_button_be_click(),
            )
        });
        if pressed
            && ui.is_enabled()
            && let Some(at) = press.filter(|p| nav_cube::owns(rect, *p))
        {
            self.cube_press = Some((at, false));
        }
        let Some((at, dragged)) = self.cube_press else {
            return false;
        };
        if down && !could_click && delta.is_finite() && delta != egui::Vec2::ZERO {
            self.camera.orbit(f64::from(delta.x), f64::from(delta.y));
            self.cube_press = Some((at, true));
        }
        if released || !down {
            if !dragged
                && released
                && let Some(target) = nav_cube::target(&self.camera, rect, at)
            {
                match target {
                    nav_cube::Target::Home => {
                        self.standard_view(pcbcraft_solidcraft::viewport::StandardView::Iso)
                    }
                    nav_cube::Target::View(direction) => self.view_from(direction),
                }
            }
            self.cube_press = None;
        }
        true
    }

    /// Draw the navigation cube over the view at `rect`, with the hovered
    /// target's tooltip.
    fn paint_cube(&self, ui: &egui::Ui, rect: egui::Rect) {
        let hover = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| ui.is_enabled() && rect.contains(*p));
        let over = hover.filter(|p| nav_cube::owns(rect, *p));
        let dragging = self.cube_press.is_some_and(|(_, dragged)| dragged);
        nav_cube::paint(
            &ui.painter_at(rect),
            &self.camera,
            rect,
            over.filter(|_| !dragging),
            over.is_some() || self.cube_press.is_some(),
        );
        if let Some(target) = over
            .filter(|_| !dragging)
            .and_then(|p| nav_cube::target(&self.camera, rect, p))
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            let (cube, _) = nav_cube::layout(rect);
            ui.interact(
                cube,
                ui.scope_id().with("pcb-3d-cube"),
                egui::Sense::hover(),
            )
            .on_hover_text_at_pointer(nav_cube::tooltip(target));
        }
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

mod decals;
mod display;
pub mod nav_cube;
pub mod surface;
pub use decals::DecalRequest;

#[cfg(test)]
mod navigation_tests;
#[cfg(test)]
mod pictures;

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::{CopperLayer, Layer};
    use pcbcraft_solidcraft::material::BodyKind;

    fn demo() -> Result<Viewport, Box<dyn std::error::Error>> {
        let project = crate::model_scene::tests::demo_project()?;
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let mut viewport = Viewport::detached();
        viewport.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
        viewport.fit();
        Ok(viewport)
    }

    fn vias(count: usize) -> Result<Viewport, Box<dyn std::error::Error>> {
        let project = crate::model_scene::tests::via_project(count)?;
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let mut viewport = Viewport::detached();
        viewport.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
        viewport.fit();
        Ok(viewport)
    }

    /// The published instanced meshes: key, triangles' address and
    /// placements.
    fn instanced(slot: &SceneSlot) -> Vec<(u64, usize, usize)> {
        slot.lock()
            .ok()
            .and_then(|s| {
                s.as_ref().map(|s| {
                    s.instanced
                        .iter()
                        .map(|m| (m.key, Arc::as_ptr(&m.tris) as usize, m.count()))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    /// The via bodies' plan centre and top, seen from straight above.
    fn above_via(
        viewport: &mut Viewport,
        index: usize,
    ) -> Result<(u64, f64, f64), Box<dyn std::error::Error>> {
        let (w, h) = (800.0, 600.0);
        let model = viewport.model.as_ref().ok_or("no model")?;
        let mut items: Vec<u64> = model
            .geometry
            .iter()
            .filter(|g| g.kind == BodyKind::ViaCopper)
            .filter_map(|g| g.item)
            .collect();
        items.dedup();
        let item = *items.get(index).ok_or("no such via")?;
        let mut b = Aabb3::EMPTY;
        for g in model.geometry.iter().filter(|g| g.item == Some(item)) {
            b.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
            b.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
        }
        // On the ring, beside the drill.
        let at = Vec3::new(b.center().x + 0.22, b.center().y, b.max.z);
        viewport.camera.target = at;
        viewport.camera.yaw = 0.0;
        viewport.camera.pitch = 1.55;
        viewport.camera.distance = 6.0;
        let (x, y, _) = viewport
            .camera
            .to_screen(at, w, h, viewport.bounds.diagonal())
            .ok_or("behind the eye")?;
        Ok((item, x, y))
    }

    /// A click over a via picks that via, though its bodies are placements
    /// of meshes every via shares; with the vias' layer hidden it passes
    /// through to what lies below.
    #[test]
    fn a_click_picks_the_via_instance_under_the_pointer() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut viewport = vias(40)?;
        let (w, h) = (800.0, 600.0);
        for index in [0, 17, 39] {
            let (item, x, y) = above_via(&mut viewport, index)?;
            let hit = viewport.hit(x, y, w, h).ok_or("nothing hit")?;
            assert!(hit.1.place.is_some(), "{:?} is drawn instanced", hit.1.kind);
            assert_eq!(viewport.pick(x, y, w, h), Some(item), "via {index}");
        }
        let (item, x, y) = above_via(&mut viewport, 17)?;
        viewport.set_layer(ViewLayer::Vias, false);
        viewport.set_layer(ViewLayer::Mask(Layer::Top), false);
        settle(&mut viewport)?;
        assert_ne!(viewport.pick(x, y, w, h), Some(item));
        Ok(())
    }

    /// Hiding the vias' layer leaves their instanced meshes out of the
    /// published scene; showing it again publishes the same shared
    /// triangles (their GPU buffers are kept), with every placement, and
    /// no mesh is rebuilt.
    #[test]
    fn a_layer_toggle_hides_instanced_vias_without_a_rebuild()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = vias(40)?;
        let model = viewport.model.as_ref().ok_or("no model")?;
        let meshes: Vec<usize> = model
            .geometry
            .iter()
            .map(|g| Arc::as_ptr(&g.mesh) as usize)
            .collect();
        let of_vias: Vec<u64> = model
            .instanced
            .iter()
            .filter(|i| i.layer == ViewLayer::Vias)
            .map(|i| i.mesh.key)
            .collect();
        assert!(!of_vias.is_empty());
        let shown = instanced(&viewport.slot);
        let generation = viewport.generation;
        viewport.set_layer(ViewLayer::Vias, false);
        settle(&mut viewport)?;
        let hidden = instanced(&viewport.slot);
        assert!(hidden.iter().all(|(key, _, _)| !of_vias.contains(key)));
        assert_eq!(
            hidden,
            shown
                .iter()
                .filter(|(key, _, _)| !of_vias.contains(key))
                .copied()
                .collect::<Vec<_>>()
        );
        viewport.set_layer(ViewLayer::Vias, true);
        settle(&mut viewport)?;
        assert_eq!(instanced(&viewport.slot), shown);
        assert_eq!(viewport.generation, generation, "the model was not rebuilt");
        let after: Vec<usize> = viewport
            .model
            .as_ref()
            .map(|m| {
                m.geometry
                    .iter()
                    .map(|g| Arc::as_ptr(&g.mesh) as usize)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(after, meshes);
        Ok(())
    }

    /// VM check #7: a selected via, a few points across in the fitted
    /// board, gets a ring around it in the selection colour; zoomed in
    /// until it is large, the tint alone shows it. Its net's other vias
    /// (highlighted, not selected) get none.
    #[test]
    fn a_small_selected_via_gets_a_ring() -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = vias(40)?;
        let (w, h) = (800.0, 600.0);
        let (item, _, _) = above_via(&mut viewport, 5)?;
        let (other, _, _) = above_via(&mut viewport, 6)?;
        viewport.fit();
        viewport.highlight_selected(vec![item, other], vec![item]);
        let rings = viewport.selection_rings(w, h);
        let [([cx, cy], r)] = rings[..] else {
            return Err(format!("one ring, got {rings:?}").into());
        };
        assert!(r >= RING_RADIUS_PT, "{r}");
        let model = viewport.model.as_ref().ok_or("no model")?;
        let mut b = Aabb3::EMPTY;
        for g in model.geometry.iter().filter(|g| g.item == Some(item)) {
            b.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
            b.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
        }
        let (vx, vy, _) = viewport
            .camera
            .to_screen(b.center(), w, h, viewport.bounds.diagonal())
            .ok_or("behind the eye")?;
        assert!((cx - vx).hypot(cy - vy) < r, "round the via");
        // Close up (as `above_via` frames it) the via is large: no ring.
        above_via(&mut viewport, 5)?;
        viewport.camera.distance = 1.0;
        assert!(viewport.selection_rings(w, h).is_empty());
        viewport.highlight_selected(vec![], vec![]);
        viewport.fit();
        assert!(viewport.selection_rings(w, h).is_empty());
        Ok(())
    }

    /// Selecting a via draws exactly its copper placements again in the
    /// selection colour, from the shared triangles (its display-only bore
    /// shadow and tent dome, which would cover the open drill, are left
    /// out); a hidden layer's are left out.
    #[test]
    fn a_selected_via_highlights_its_own_placements() -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = vias(40)?;
        let (item, _, _) = above_via(&mut viewport, 5)?;
        let model = viewport.model.as_ref().ok_or("no model")?;
        let bodies = model
            .geometry
            .iter()
            .filter(|g| g.item == Some(item) && g.place.is_some())
            .filter(|g| !matches!(g.kind, BodyKind::ViaBore | BodyKind::MaskCap))
            .count();
        assert!(bodies > 0);
        let model_tris: Vec<usize> = model
            .instanced
            .iter()
            .map(|i| Arc::as_ptr(&i.mesh.tris) as usize)
            .collect();
        let key = viewport.key;
        viewport.highlight(vec![item]);
        let selected = |v: &Viewport| -> Vec<(usize, usize)> {
            v.hl_slot
                .lock()
                .ok()
                .and_then(|s| {
                    s.as_ref().map(|s| {
                        s.selected_instanced
                            .iter()
                            .map(|m| (Arc::as_ptr(&m.tris) as usize, m.count()))
                            .collect()
                    })
                })
                .unwrap_or_default()
        };
        let drawn = selected(&viewport);
        assert_eq!(drawn.iter().map(|(_, n)| n).sum::<usize>(), bodies);
        assert!(drawn.iter().all(|(t, _)| model_tris.contains(t)));
        assert_eq!(viewport.key, key, "the model is not published again");
        viewport.set_layer(ViewLayer::Vias, false);
        assert!(
            selected(&viewport).iter().map(|(_, n)| n).sum::<usize>() < bodies,
            "the hidden barrel is not highlighted"
        );
        Ok(())
    }

    /// The picked via is drawn in the strong selection tint (palette
    /// entry 1) and the other vias of its net in the lighter net tint
    /// (entry 0), so one via stands out among many on the same net.
    #[test]
    fn the_picked_via_stands_out_from_its_net() -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = vias(40)?;
        let (item, _, _) = above_via(&mut viewport, 5)?;
        let model = viewport.model.as_ref().ok_or("no model")?;
        let copper = |g: &&Geometry| {
            g.place.is_some() && !matches!(g.kind, BodyKind::ViaBore | BodyKind::MaskCap)
        };
        let net: Vec<u64> = model
            .geometry
            .iter()
            .filter(copper)
            .filter(|g| g.kind == BodyKind::ViaCopper)
            .filter_map(|g| g.item)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert!(net.len() > 1 && net.contains(&item));
        let own = model
            .geometry
            .iter()
            .filter(copper)
            .filter(|g| g.item == Some(item))
            .count();
        let all = model
            .geometry
            .iter()
            .filter(copper)
            .filter(|g| g.item.is_some_and(|i| net.contains(&i)))
            .count();
        viewport.highlight_selected(net, vec![item]);
        let colours: Vec<u32> = viewport
            .hl_slot
            .lock()
            .ok()
            .and_then(|s| {
                s.as_ref().map(|s| {
                    s.selected_instanced
                        .iter()
                        .flat_map(|m| {
                            m.instances
                                .chunks_exact(solidcraft_viewport_gpu::view::INSTANCE_SIZE)
                                .filter_map(|c| c.get(48..52)?.try_into().ok())
                                .map(u32::from_le_bytes)
                                .collect::<Vec<_>>()
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        assert_eq!(colours.iter().filter(|c| **c == 1).count(), own);
        assert_eq!(colours.iter().filter(|c| **c == 0).count(), all - own);
        Ok(())
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

    /// Wait for a layer repack to land.
    fn settle(viewport: &mut Viewport) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while viewport.receive_repack() {
            if std::time::Instant::now() > deadline {
                return Err("the repack did not finish".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(())
    }

    fn pbr_bytes(slot: &SceneSlot) -> Vec<u8> {
        slot.lock()
            .ok()
            .and_then(|s| s.as_ref().map(|s| s.tris.clone()))
            .unwrap_or_default()
    }

    fn pbr_len(slot: &SceneSlot) -> usize {
        slot.lock()
            .ok()
            .and_then(|s| s.as_ref().map(|s| s.tris.len()))
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
    fn hiding_the_substrate_republishes_the_model_but_no_new_blocks()
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
        viewport.set_layers(LayerView::substrate_hidden(true));
        settle(&mut viewport)?;
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
        viewport.shadow_view = Some((!dark, false, ShadowLayers::of(&LayerView::default())));
        viewport.publish_shadows();
        let after = (viewport.key, viewport.hl_key, viewport.sketch_key);
        assert_eq!((before.0, before.1), (after.0, after.1));
        assert_ne!(before.2, after.2);
        let unchanged = viewport.sketch_key;
        viewport.publish_shadows();
        assert_eq!(viewport.sketch_key, unchanged, "nothing changed: no upload");
        Ok(())
    }

    /// The middle of the demo track, seen from straight above.
    fn above_track(viewport: &mut Viewport) -> Result<(f64, f64), Box<dyn std::error::Error>> {
        let (w, h) = (800.0, 600.0);
        let track = viewport
            .model
            .as_ref()
            .and_then(|m| m.geometry.iter().find(|g| g.kind == BodyKind::CopperTrack))
            .ok_or("no track")?;
        let middle = Vec3::new(
            (track.min[0] + track.max[0]) / 2.0,
            (track.min[1] + track.max[1]) / 2.0,
            track.max[2],
        );
        viewport.camera.target = middle;
        viewport.camera.yaw = 0.0;
        viewport.camera.pitch = 1.55;
        viewport.camera.distance = 20.0;
        let (x, y, _) = viewport
            .camera
            .to_screen(middle, w, h, viewport.bounds.diagonal())
            .ok_or("behind the eye")?;
        Ok((x, y))
    }

    /// Hiding the top mask shows the copper under it: the mask is neither
    /// drawn nor picked, the track under it is, and no mesh is rebuilt.
    #[test]
    fn hiding_the_top_mask_reveals_and_picks_the_copper_under_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = demo()?;
        let meshes: Vec<usize> = viewport
            .model
            .as_ref()
            .map(|m| {
                m.geometry
                    .iter()
                    .map(|g| Arc::as_ptr(&g.mesh) as usize)
                    .collect()
            })
            .unwrap_or_default();
        let (x, y) = above_track(&mut viewport)?;
        let (w, h) = (800.0, 600.0);
        let kind = |v: &Viewport| v.hit(x, y, w, h).map(|(_, g)| g.kind);
        assert_eq!(
            kind(&viewport),
            Some(BodyKind::MaskCap),
            "the mask covers it"
        );
        let full = pbr_len(&viewport.slot);
        viewport.set_layer(ViewLayer::Mask(Layer::Top), false);
        assert_eq!(kind(&viewport), Some(BodyKind::CopperTrack));
        settle(&mut viewport)?;
        let model = viewport.model.as_ref().ok_or("no model")?;
        let (expected, _) =
            model_scene::pack(&model.geometry, &[], viewport.layers(), &model.stack)?;
        assert_eq!(pbr_bytes(&viewport.slot), expected.tris);
        assert!(expected.tris.len() < full);
        let after: Vec<usize> = model
            .geometry
            .iter()
            .map(|g| Arc::as_ptr(&g.mesh) as usize)
            .collect();
        assert_eq!(meshes, after, "the cached meshes are kept");
        // Copper hidden as well: the pick reaches the laminate.
        viewport.set_layer(ViewLayer::Copper(CopperLayer::Top), false);
        assert_eq!(kind(&viewport), Some(BodyKind::Substrate));
        viewport.set_layers(LayerView::default());
        assert_eq!(kind(&viewport), Some(BodyKind::MaskCap));
        Ok(())
    }

    /// The selection and shadows follow the layers: a hidden layer's
    /// highlight blocks and a hidden package's contact shadow are left out.
    #[test]
    fn hidden_layers_drop_their_highlight_and_shadows() -> Result<(), Box<dyn std::error::Error>> {
        let mut viewport = demo()?;
        let all: Vec<u64> = viewport
            .model
            .as_ref()
            .map(|m| m.highlight.iter().map(|(i, _, _)| *i).collect())
            .unwrap_or_default();
        viewport.highlight(all);
        let count = |v: &Viewport| blocks(&v.hl_slot).len();
        let shadow = |v: &Viewport| {
            v.sketch_slot
                .lock()
                .ok()
                .and_then(|s| s.as_ref().map(|s| s.trans.len()))
                .unwrap_or(0)
        };
        let (blocks_all, shadows_all) = (count(&viewport), shadow(&viewport));
        let key = viewport.key;
        viewport.set_layer(ViewLayer::Bodies, false);
        assert!(count(&viewport) < blocks_all);
        assert!(
            shadow(&viewport) < shadows_all,
            "contact shadows go with the bodies"
        );
        assert!(shadow(&viewport) > 0, "the ground shadow stays");
        assert_eq!(viewport.key, key, "the model waits for its repack");
        viewport.set_layers(LayerView::default());
        assert_eq!(
            (count(&viewport), shadow(&viewport)),
            (blocks_all, shadows_all)
        );
        Ok(())
    }

    /// A model that arrives while layers are hidden is shown with them
    /// hidden at once, its vertices moved in place, not packed again.
    #[test]
    fn a_new_model_keeps_the_hidden_layers() -> Result<(), Box<dyn std::error::Error>> {
        let project = crate::model_scene::tests::demo_project()?;
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let scene = crate::model_scene::build(model.geometry, &model.stack)?;
        let mut view = LayerView::default();
        view.set(ViewLayer::Substrate, false);
        view.set(ViewLayer::Silkscreen(Layer::Top), false);
        let (expected, _) = model_scene::pack(&scene.geometry, &[], &view, &scene.stack)?;
        let mut viewport = Viewport::detached();
        viewport.set_layers(view);
        viewport.set_model(scene);
        // The opaque bytes are the view's at once, kept in place; only the
        // material stream, which cannot drop runs, follows from a repack.
        assert_eq!(pbr_bytes(&viewport.slot), expected.tris);
        assert!(viewport.repack.is_some());
        settle(&mut viewport)?;
        assert_eq!(pbr_bytes(&viewport.slot), expected.tris);
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

    /// Demo run 3: a board made wider than the view is fitted again when
    /// its model arrives; a model of the same size (an edit elsewhere, or
    /// the view zoomed in on purpose) keeps the camera.
    #[test]
    fn a_board_grown_out_of_view_is_fitted_again() -> Result<(), Box<dyn std::error::Error>> {
        let (w, h) = (1500.0, 800.0);
        let built = |grow: f64| -> Result<ModelScene, Box<dyn std::error::Error>> {
            let mut project = crate::model_scene::tests::demo_project()?;
            project.board_size[0] *= grow;
            let model = pcbcraft_solidcraft::project::build(&project)?;
            Ok(crate::model_scene::build(model.geometry, &model.stack)?)
        };
        let mut viewport = demo()?;
        viewport.aspect = w / h;
        viewport.fit_to(w / h);
        viewport.fit_pending = false;
        let inside = |viewport: &Viewport| -> Result<bool, String> {
            Ok(corner_shares(viewport, w, h)?
                .iter()
                .all(|(x, y)| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y)))
        };
        assert!(inside(&viewport)?);
        // The same board again: the camera stays.
        let camera = viewport.camera;
        viewport.set_model(built(1.0)?);
        assert_eq!(viewport.camera, camera);
        // Twice as wide: it would leave the view, so it is fitted.
        let before = viewport.model_bounds;
        viewport.set_model(built(2.0)?);
        assert!(viewport.model_bounds.max.x > before.max.x + 1.0, "grew");
        viewport.fit_to(w / h);
        assert!(inside(&viewport)?);
        // Zoomed in on purpose: a new model of that size keeps the camera.
        viewport.camera.distance *= 0.3;
        let zoomed = viewport.camera;
        viewport.fit_pending = false;
        viewport.set_model(built(2.0)?);
        assert_eq!(viewport.camera, zoomed);
        assert!(!viewport.fit_pending);
        Ok(())
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
        viewport.set_layers(LayerView::substrate_hidden(true));
        assert!(
            viewport.hit(x, y, w, h).is_none(),
            "hidden board is not hit"
        );
        Ok(())
    }
}
