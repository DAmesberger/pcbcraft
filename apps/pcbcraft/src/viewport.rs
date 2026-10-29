//! Solidcraft's GPU renderer and camera over cached geometry. Solidcraft's full
//! navigation handler is private to its app, so this applies the same camera
//! operations (middle-drag pan, right-drag orbit, scroll zoom) directly.
use pcbcraft_solidcraft::{
    geom::{Aabb3, Vec3},
    render::Camera,
    scene::Geometry,
};
use solidcraft_ui_egui::gpu::{
    self, BlockBuilder, Frame, GpuScene, GpuTarget, Palette, PaletteIndex, SceneBlock, SceneSlot,
    TriClass, ViewportCallback,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);
fn next_key() -> u64 {
    NEXT_KEY.fetch_add(1, Ordering::Relaxed)
}

pub struct Viewport {
    geometry: Vec<Geometry>,
    bounds: Aabb3,
    colors: Vec<[u8; 4]>,
    blocks: Vec<SceneBlock>,
    highlighted: Vec<u64>,
    slot: SceneSlot,
    empty: SceneSlot,
    key: u64,
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
        let camera = Camera::default();
        let mut result = Self {
            geometry: vec![],
            bounds: Self::initial_bounds(),
            colors: vec![],
            blocks: vec![],
            highlighted: vec![],
            slot: Arc::new(Mutex::new(None)),
            empty: Arc::new(Mutex::new(None)),
            key: next_key(),
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
    /// Replace the meshes and their base colours (one per geometry).
    pub fn set_geometry(&mut self, geometry: Vec<Geometry>, colors: Vec<[u8; 4]>) {
        if !geometry.is_empty() {
            let min = std::array::from_fn::<_, 3, _>(|axis| {
                geometry
                    .iter()
                    .map(|g| g.min[axis])
                    .fold(f64::INFINITY, f64::min)
            });
            let max = std::array::from_fn::<_, 3, _>(|axis| {
                geometry
                    .iter()
                    .map(|g| g.max[axis])
                    .fold(f64::NEG_INFINITY, f64::max)
            });
            self.bounds = Aabb3 {
                min: Vec3::new(min[0], min[1], min[2]),
                max: Vec3::new(max[0], max[1], max[2]),
            };
        }
        self.geometry = geometry;
        self.colors = colors;
        self.blocks = self
            .geometry
            .iter()
            .enumerate()
            .map(|(index, g)| {
                let mut builder = BlockBuilder::default();
                builder.begin_body();
                for indices in &g.mesh.triangles {
                    if let Some(p) = g.mesh.tri(indices) {
                        let normal = (p[1] - p[0])
                            .cross(p[2] - p[0])
                            .normalized()
                            .unwrap_or(Vec3::Z)
                            .to_f32();
                        builder.triangle(
                            p.map(|p| p.to_f32()),
                            [normal; 3],
                            PaletteIndex(index as u32),
                            TriClass::Opaque,
                        );
                    }
                }
                builder.end_body();
                SceneBlock {
                    key: next_key(),
                    bytes: builder.freeze(),
                }
            })
            .collect();
        self.refresh();
    }
    pub fn highlight(&mut self, ids: Vec<u64>) {
        if self.highlighted != ids {
            self.highlighted = ids;
            self.refresh();
        }
    }
    pub fn refresh(&mut self) {
        // Highlighting rebuilds only the palette; mesh blocks are reused.
        let board = crate::palette::board();
        let colors = self
            .geometry
            .iter()
            .enumerate()
            .map(|(index, g)| {
                if g.item.is_some_and(|i| self.highlighted.contains(&i)) {
                    board.selected.to_array()
                } else {
                    self.colors
                        .get(index)
                        .copied()
                        .unwrap_or(board.package.to_array())
                }
            })
            .collect();
        let mut scene = GpuScene::with_palette(Palette::with_stable(colors));
        scene.blocks = self
            .blocks
            .iter()
            .zip(&self.geometry)
            .filter(|(_, g)| !(self.hide_substrate && g.item.is_none()))
            .map(|(b, _)| b.clone())
            .collect();
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(scene);
            self.key = next_key();
        }
    }
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
        let pp = ui.ctx().pixels_per_point();
        let eye = self.camera.eye().to_f32();
        let cb = ViewportCallback(Frame {
            product_lights: None,
            product: false,
            key: self.key,
            slot: self.slot.clone(),
            sketch_key: 0,
            sketch_slot: self.empty.clone(),
            hl_key: 0,
            hl_slot: self.empty.clone(),
            pv_key: 0,
            pv_slot: self.empty.clone(),
            pv_hide: true,
            view_proj: self
                .camera
                .view_proj_bounds(f64::from(rect.width() / rect.height()), &self.bounds)
                .to_f32(),
            back: self.camera.back().to_f32(),
            size_px: [rect.width() * pp, rect.height() * pp],
            clip: [0.0; 4],
            cap: [0.0; 4],
            images: vec![],
            points: vec![],
            analysis: [0.0; 8],
            analysis_colors: [0.0; 36],
            access: None,
            outline: false,
            eye: [eye[0], eye[1], eye[2], 0.0],
            ao_radius: 0.0,
            ao_samples: 0,
            ssaa: 1.0,
            hover_tint: [0.1; 3],
            selection_tint: [0.8, 0.7, 0.1],
        });
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, cb));
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (origin, direction) = self.camera.ray(
                f64::from(p.x - rect.left()),
                f64::from(p.y - rect.top()),
                f64::from(rect.width()),
                f64::from(rect.height()),
            );
            let pick = self
                .geometry
                .iter()
                .filter(|g| !(self.hide_substrate && g.item.is_none()))
                .filter_map(|g| g.mesh.raycast(origin, direction).map(|(d, _)| (d, g.item)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .and_then(|(_, id)| id);
            return Some(pick);
        }
        None
    }
}
