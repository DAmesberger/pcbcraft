//! GPU-ready form of a built native model, prepared on the model worker so
//! the UI thread only publishes it. Every body's faces are drawn twice:
//! once as plain opaque triangles (depth, occlusion, edges, its look's
//! colour), grouped by [`ViewLayer`] in one buffer so a layer view keeps or
//! drops whole runs of it, and once through Solidcraft's material-graph
//! stream ([`GpuScene::graphs`]), which shades them with the body's physical
//! look, clear coat and procedural pattern under the product look. The
//! buffers move into the viewport once and are never repacked on the UI
//! thread. Shadows are theme-free geometry with alpha weights; highlight
//! blocks are flat copies keyed by pick identity and view layer, so
//! selection only recolours and hidden layers drop their blocks.
//!
//!
//! Placements of a shared definition (every via of a size, each component
//! body) are not copied into the opaque buffer: each definition mesh is one
//! [`Instanced`] mesh drawn at every placement, with coarser levels for
//! placements small on screen, and highlighted by drawing the selected
//! placements again ([`Instanced::highlight`]). The material stream holds
//! world vertices, so its copy of a placed body is one per placement:
//! [`MAX_GRAPH_BYTES`] bounds that, and placements past it keep the plain
//! colour of their instanced mesh.
//!
//! The ground shadow follows Solidcraft's viewport `ground_shadow` (MIT,
//! upstream revision 4f3ac07, unchanged in the pinned fork; see
//! ATTRIBUTION.md), adapted to a Z-up board.
use crate::layer_view::{LayerView, ViewLayer};
use pcbcraft_solidcraft::{
    geom::{Aabb3, Vec3, prelude::*, srgb},
    look_graph::{self, LookKey, LoweredLook},
    material::BodyKind,
    render::surface::{OpaqueSurface, unit_direction},
    scene::{Geometry, Place},
};
use solidcraft_ui_egui::gpu::{
    BlockBuilder, GpuScene, InstanceLook, InstancedMesh, LOD_PX, PaletteIndex, SceneBlock,
    SharedTris, TRI_SIZE, TriClass,
};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Upper bound on opaque vertex bytes per model: about 1 000 000 triangles,
/// so no board that loaded before is refused.
pub const MAX_PBR_BYTES: usize = 88 << 20;

/// Upper bound on the material stream per model. The unplaced bodies always
/// fit (the same triangle bound as [`MAX_PBR_BYTES`], at the largest vertex);
/// past it placed bodies are drawn in their plain colour, largest count last.
pub const MAX_GRAPH_BYTES: usize = 256 << 20;

/// The plain colour of a look (sRGB8), which its faces show where the material
/// stream does not draw them.
pub fn plain_colour(surface: OpaqueSurface) -> [u8; 4] {
    let b = surface.base_color();
    [
        srgb::encode8(b.x),
        srgb::encode8(b.y),
        srgb::encode8(b.z),
        255,
    ]
}

/// An empty scene whose palette holds every colour of `geometry` in order of
/// first use: the same for any layer view of the same bodies, so instanced
/// placements keep their index.
pub fn palette_of(geometry: &[Geometry]) -> GpuScene {
    let mut scene = GpuScene::default();
    for g in geometry {
        scene.color(plain_colour(g.surface));
    }
    scene
}

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// A fresh scene or block key, shared by every slot the viewport publishes.
pub fn next_key() -> u64 {
    NEXT_KEY.fetch_add(1, Ordering::Relaxed)
}

/// A shadow vertex: world position and opacity weight (0..1).
pub type ShadowVertex = ([f32; 3], f32);

/// Shadow triangles (three vertices each), independent of the theme.
#[derive(Default)]
pub struct Shadows {
    /// Soft ellipse just below the board, seen from above.
    pub ground: Vec<ShadowVertex>,
    /// Contact shadows under top-side packages, on the top soldermask.
    pub top: Vec<ShadowVertex>,
    /// Contact shadows under bottom-side packages.
    pub bottom: Vec<ShadowVertex>,
}

pub struct ModelScene {
    pub geometry: Vec<pcbcraft_solidcraft::scene::Geometry>,
    /// Every body's opaque vertices (`tris`, grouped by view layer in
    /// [`ViewLayer`] order), its palette and its material stream (`graphs`).
    /// The viewport takes it.
    pub stream: GpuScene,
    /// Each view layer's run of `stream.tris` bytes, in buffer order.
    pub runs: Runs,
    pub shadows: Shadows,
    /// One flat block per pick identity and view layer, drawn over the model
    /// in the selection colour.
    pub highlight: Vec<(u64, ViewLayer, SceneBlock)>,
    /// The placed bodies, one instanced mesh per definition mesh, look and view layer.
    pub instanced: Vec<Instanced>,
    pub bounds: Aabb3,
}

/// One definition mesh drawn at each of its placements: its shared
/// triangles (and coarser levels) and every placement of it in one view
/// layer and material.
pub struct Instanced {
    pub layer: ViewLayer,
    /// The look's plain colour.
    pub colour: [u8; 4],
    /// A placement each, in `bodies` order.
    pub mesh: InstancedMesh,
    /// The index in [`ModelScene::geometry`] of each placement.
    pub bodies: Vec<usize>,
}

/// Fewest triangles a definition mesh is simplified from for coarser
/// levels, and fewest a level keeps.
const LOD_FROM: usize = 96;
const LOD_MIN: usize = 16;

impl Instanced {
    /// The placements `keep` keeps, moved by `display`, as a mesh of the
    /// same triangles (their GPU buffers are kept).
    pub fn placed(
        &self,
        geometry: &[Geometry],
        display: Option<&Place>,
        look: InstanceLook,
        color: PaletteIndex,
        keep: impl Fn(&Geometry) -> bool,
    ) -> InstancedMesh {
        let mut out = InstancedMesh {
            key: self.mesh.key,
            tris: self.mesh.tris.clone(),
            instances: Vec::new(),
            look,
            levels: if look == self.mesh.look {
                self.mesh.levels.clone()
            } else {
                Vec::new()
            },
            closed: false,
        };
        for g in self.bodies.iter().filter_map(|i| geometry.get(*i)) {
            if !keep(g) {
                continue;
            }
            // A large unplaced body is drawn where it is.
            let place = g.place.unwrap_or(Place::IDENTITY);
            let place = display.map_or(place, |d| d.then(&place));
            out.place(place.rows_f32(), color);
        }
        out
    }

    /// The highlighted placements, drawn again flat: the selected object's
    /// in the strong selection colour (palette entry 1 of the highlight
    /// scene), the rest of its net's in the lighter net colour (entry 0),
    /// so the picked via stands out among thousands on its net. A via's
    /// display-only bore shadow and tent dome are left out: drawn flat
    /// they would cover its open drill. A large unplaced body highlights
    /// from its own block instead ([`ModelScene::highlight`]).
    pub fn highlight(
        &self,
        geometry: &[Geometry],
        display: Option<&Place>,
        highlighted: &std::collections::HashSet<u64>,
        selected: &std::collections::HashSet<u64>,
    ) -> Option<InstancedMesh> {
        let mut out = InstancedMesh {
            key: self.mesh.key,
            tris: self.mesh.tris.clone(),
            instances: Vec::new(),
            look: InstanceLook::Opaque,
            levels: Vec::new(),
            closed: false,
        };
        for g in self.bodies.iter().filter_map(|i| geometry.get(*i)) {
            let (Some(place), Some(item)) = (g.place, g.item) else {
                continue;
            };
            if matches!(g.kind, BodyKind::ViaBore | BodyKind::MaskCap) {
                continue;
            }
            let color = if selected.contains(&item) {
                PaletteIndex(1)
            } else if highlighted.contains(&item) {
                PaletteIndex(0)
            } else {
                continue;
            };
            let place = display.map_or(place, |d| d.then(&place));
            out.place(place.rows_f32(), color);
        }
        (out.count() > 0).then_some(out)
    }
}

/// A mesh's coarser levels, finest first, each with its error (mm).
type Levels = Vec<(Arc<SharedTris>, f32)>;

/// Fewest triangles of an unplaced body drawn as an indexed mesh (at one
/// placement) instead of in the vertex stream: a board's laminate and mask
/// with thousands of drill holes and openings. Indexed, each vertex is
/// stored once and the stream stays small whatever the board.
pub const LARGE_BODY: usize = 32_768;

/// Most bytes the instanced meshes may hold on the GPU (their vertices,
/// indices, levels and placements).
pub const MAX_INSTANCED_BYTES: usize = 768 << 20;

/// Whether a body goes into the vertex stream ([`pack`]): unplaced and not
/// large. Everything else is drawn as an instanced mesh ([`Instanced`]).
pub fn streamed(g: &Geometry) -> bool {
    g.place.is_none() && g.mesh.triangles.len() < LARGE_BODY
}

/// The placed and the large bodies of `geometry` grouped by definition
/// mesh, material and view layer, with coarser levels of each mesh placed
/// more than once.
/// [`instance`] for bodies drawn beside a model (Component 3D decals).
pub fn instance_bodies(geometry: &[Geometry]) -> Result<Vec<Instanced>, String> {
    instance(geometry)
}

fn instance(geometry: &[Geometry]) -> Result<Vec<Instanced>, String> {
    let mut out: Vec<Instanced> = Vec::new();
    let mut palette = palette_of(geometry);
    let mut by: HashMap<(usize, [u8; 4], ViewLayer), usize> = HashMap::new();
    let mut tris: HashMap<usize, (Arc<SharedTris>, Levels)> = HashMap::new();
    let uses = {
        let mut uses: HashMap<usize, usize> = HashMap::new();
        for g in geometry.iter().filter(|g| g.place.is_some()) {
            *uses.entry(Arc::as_ptr(&g.mesh) as usize).or_default() += 1;
        }
        uses
    };
    let mut bytes = 0usize;
    for (index, g) in geometry.iter().enumerate() {
        if streamed(g) {
            continue;
        }
        let place = g.place.unwrap_or(Place::IDENTITY);
        let layer = ViewLayer::of(g.kind, g.layer);
        let address = Arc::as_ptr(&g.mesh) as usize;
        let colour = plain_colour(g.surface);
        let at = *by.entry((address, colour, layer)).or_insert_with(|| {
            let (shared, levels) = tris
                .entry(address)
                .or_insert_with(|| {
                    let several = uses.get(&address).is_some_and(|n| *n > 1);
                    let shared = Arc::new(indexed(&g.mesh));
                    (shared, if several { levels(&g.mesh) } else { Vec::new() })
                })
                .clone();
            bytes = bytes.saturating_add(
                std::iter::once(&shared)
                    .chain(levels.iter().map(|(t, _)| t))
                    .map(|t| gpu_bytes(t))
                    .sum(),
            );
            out.push(Instanced {
                layer,
                colour,
                mesh: InstancedMesh {
                    key: address as u64,
                    tris: shared,
                    instances: Vec::new(),
                    look: InstanceLook::Opaque,
                    levels,
                    closed: false,
                },
                bodies: Vec::new(),
            });
            out.len() - 1
        });
        if let Some(group) = out.get_mut(at) {
            group.mesh.place(place.rows_f32(), palette.color(colour));
            group.bodies.push(index);
            bytes = bytes.saturating_add(solidcraft_ui_egui::gpu::INSTANCE_SIZE);
        }
        if bytes > MAX_INSTANCED_BYTES {
            return Err("model too large for the 3D view".into());
        }
    }
    Ok(out)
}

/// Bytes a mesh's triangles take on the GPU: its vertices and indices.
pub fn gpu_bytes(tris: &SharedTris) -> usize {
    tris.vertices.len() + tris.indices.len() * 4
}

/// `mesh`'s triangles with each vertex stored once per position and
/// normal, lit by those normals (also where every triangle is flat, which
/// Solidcraft's own sharing would light by its facets alone).
fn indexed(mesh: &pcbcraft_solidcraft::geom::Mesh) -> SharedTris {
    let mut out = SharedTris::default();
    out.indices.reserve_exact(mesh.triangles.len() * 3);
    let mut seen: HashMap<[u32; 6], u32> = HashMap::with_capacity(mesh.positions.len());
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for t in &mesh.triangles {
        let corners = t.map(|i| {
            let i = usize::try_from(i).ok()?;
            Some((
                mesh.positions.get(i)?.to_f32(),
                mesh.normals.get(i)?.to_f32(),
            ))
        });
        let [Some(a), Some(b), Some(c)] = corners else {
            continue;
        };
        for (p, n) in [a, b, c] {
            let bits = [p[0], p[1], p[2], n[0], n[1], n[2]].map(|x| (x + 0.0).to_bits());
            let next = u32::try_from(out.vertices.len() / solidcraft_ui_egui::gpu::SHARED_SIZE)
                .unwrap_or(u32::MAX);
            let index = *seen.entry(bits).or_insert_with(|| {
                for x in p.iter().chain(&n) {
                    out.vertices.extend_from_slice(&x.to_le_bytes());
                }
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
                next
            });
            out.indices.push(index);
        }
    }
    out.vertices.shrink_to_fit();
    out.bounds = [lo, hi];
    out
}

/// Coarser copies of a definition mesh, finest first, each with the error
/// (mm) the renderer weighs against [`LOD_PX`]: display only, never picked
/// or measured.
///
/// A level's outline may stray by less than a pixel long before its
/// shading does (a shallow tent dome's glossy highlight follows its
/// normals, not its outline), so a level is also held back until the whole
/// body is small on screen: [`LOD_SPAN_PX`] pixels across for a quarter of
/// the triangles, fewer the coarser it is.
fn levels(mesh: &pcbcraft_solidcraft::geom::Mesh) -> Vec<(Arc<SharedTris>, f32)> {
    let n = mesh.triangles.len();
    if n < LOD_FROM {
        return Vec::new();
    }
    let targets: Vec<usize> = [4, 16, 64]
        .into_iter()
        .map(|d| n / d)
        .filter(|t| *t >= LOD_MIN)
        .collect();
    let span = mesh.bounds().diagonal();
    pcbcraft_solidcraft::geom::simplify::levels(&mesh.positions, &mesh.triangles, &targets)
        .iter()
        .filter(|l| !l.triangles.is_empty())
        .map(|l| {
            let kept = l.triangles.len() as f64 / n as f64;
            // 4× fewer triangles: LOD_SPAN_PX; each further 4× halves it.
            let pixels = LOD_SPAN_PX * (4.0 * kept).sqrt();
            let error = l.error.max(span * f64::from(LOD_PX) / pixels);
            (Arc::new(smooth(l)), error as f32)
        })
        .collect()
}

/// How many pixels across a placed body is drawn from a quarter of its
/// triangles at most.
const LOD_SPAN_PX: f64 = 48.0;

/// Corners whose faces turn by more than this (cosine, about 40°) are lit
/// apart, as a crease.
const CREASE_COS: f64 = 0.766;

/// A simplified level lit smooth, as the full mesh is (a level lit flat
/// shows its facets in a glossy reflection long before its outline
/// strays): each corner's normal is the area-weighted mean of the faces
/// meeting at its vertex that turn from its own by less than a crease.
fn smooth(level: &pcbcraft_solidcraft::geom::simplify::Level) -> SharedTris {
    let p = &level.positions;
    let corner = |t: &[u32; 3]| -> Option<[Vec3; 3]> {
        Some([
            *p.get(usize::try_from(t[0]).ok()?)?,
            *p.get(usize::try_from(t[1]).ok()?)?,
            *p.get(usize::try_from(t[2]).ok()?)?,
        ])
    };
    let triangles: Vec<([u32; 3], Vec3)> = level
        .triangles
        .iter()
        .filter_map(|t| {
            let [a, b, c] = corner(t)?;
            Some((*t, (b - a).cross(c - a)))
        })
        .collect();
    let mut around: Vec<Vec<usize>> = vec![Vec::new(); p.len()];
    for (k, (t, _)) in triangles.iter().enumerate() {
        for i in t {
            if let Some(list) = around.get_mut(*i as usize) {
                list.push(k);
            }
        }
    }
    let mut out = SharedTris::default();
    let mut seen: HashMap<(u32, [u32; 3]), u32> = HashMap::new();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for (t, area) in &triangles {
        let own = unit_direction(*area).unwrap_or(Vec3::Z);
        for i in t {
            let mut sum = Vec3::new(0.0, 0.0, 0.0);
            for k in around.get(*i as usize).into_iter().flatten() {
                let Some((_, other)) = triangles.get(*k) else {
                    continue;
                };
                if unit_direction(*other).is_ok_and(|n| n.dot(own) >= CREASE_COS) {
                    sum += *other;
                }
            }
            let n = unit_direction(sum).unwrap_or(own).to_f32();
            let position = p.get(*i as usize).map_or([0.0; 3], |v| v.to_f32());
            let next = u32::try_from(out.vertices.len() / solidcraft_ui_egui::gpu::SHARED_SIZE)
                .unwrap_or(u32::MAX);
            let index = *seen.entry((*i, n.map(f32::to_bits))).or_insert_with(|| {
                for x in position.iter().chain(&n) {
                    out.vertices.extend_from_slice(&x.to_le_bytes());
                }
                for k in 0..3 {
                    lo[k] = lo[k].min(position[k]);
                    hi[k] = hi[k].max(position[k]);
                }
                next
            });
            out.indices.push(index);
        }
    }
    out.bounds = [lo, hi];
    out
}

/// Each view layer's run of bytes in a PBR buffer, in buffer order.
pub type Runs = Vec<(ViewLayer, Range<usize>)>;

/// The bodies of `geometry` in buffer order: grouped by view layer, in
/// model order within a layer. Placed and large bodies are drawn instanced
/// instead ([`streamed`]).
fn ordered(geometry: &[Geometry]) -> Vec<(ViewLayer, &Geometry)> {
    let mut out: Vec<(ViewLayer, &Geometry)> = geometry
        .iter()
        .filter(|g| streamed(g))
        .map(|g| (ViewLayer::of(g.kind, g.layer), g))
        .collect();
    // A stable sort keeps model order inside each layer.
    out.sort_by_key(|(layer, _)| *layer);
    out
}

/// The looks of a scene, lowered once each (a board has a few dozen).
#[derive(Default)]
struct Looks(HashMap<LookKey, Arc<LoweredLook>>);

impl Looks {
    fn get(&mut self, g: &Geometry) -> Result<Arc<LoweredLook>, String> {
        let key = look_graph::look_key(g.surface, g.detail);
        if let Some(look) = self.0.get(&key) {
            return Ok(Arc::clone(look));
        }
        let look =
            Arc::new(look_graph::lower_look(g.surface, g.detail).map_err(|e| e.to_string())?);
        self.0.insert(key, Arc::clone(&look));
        Ok(look)
    }
}

/// Whether a face of `g` with unit normal `n` lies under a layer `view` shows,
/// so the material stream leaves it out. The stream tests against the opaque
/// faces' depth with a bias of some pixels' depth slope (Solidcraft's overlay
/// depth), wider than a 20 µm mask when zoomed out: a covered face would shade
/// over the layer above it. Its opaque copy stays, for depth.
fn covered(g: &Geometry, n: Vec3, view: &LayerView) -> bool {
    use pcbcraft_core::{CopperLayer, Layer};
    if n.z.abs() < 0.9 {
        return false;
    }
    let side = if n.z > 0.0 { Layer::Top } else { Layer::Bottom };
    let mask = view.draws(ViewLayer::Mask(side));
    match g.kind {
        BodyKind::Substrate => mask,
        BodyKind::CopperTrack | BodyKind::FillCopper => match g.layer {
            Some(CopperLayer::Top) => side == Layer::Top && mask,
            Some(CopperLayer::Bottom) => side == Layer::Bottom && mask,
            _ => view.draws(ViewLayer::Substrate),
        },
        _ => false,
    }
}

/// The opaque vertices of the streamed bodies of `geometry` that `view` shows
/// ([`streamed`]), grouped by view layer, with each layer's run of bytes, and
/// the material stream of those and of the placed bodies, `decals` too. See
/// [`MAX_GRAPH_BYTES`] for placed bodies past the bound.
pub fn pack(
    geometry: &[Geometry],
    decals: &[Geometry],
    view: &LayerView,
) -> Result<(GpuScene, Runs), String> {
    let bodies: Vec<(ViewLayer, &Geometry)> = ordered(geometry)
        .into_iter()
        .filter(|(layer, _)| view.draws(*layer))
        .collect();
    let too_large = || "model too large for the 3D view".to_string();
    let total = bodies
        .iter()
        .try_fold(0usize, |n, (_, g)| {
            g.mesh
                .triangles
                .len()
                .checked_mul(3 * TRI_SIZE)
                .and_then(|b| n.checked_add(b))
        })
        .ok_or_else(too_large)?;
    if total > MAX_PBR_BYTES {
        return Err(too_large());
    }
    let mut scene = palette_of(geometry);
    scene
        .tris
        .try_reserve_exact(total)
        .map_err(|e| e.to_string())?;
    let mut looks = Looks::default();
    let mut runs: Runs = Vec::new();
    for (layer, g) in bodies {
        let start = scene.tris.len();
        let colour = scene.color(plain_colour(g.surface));
        let look = looks.get(g)?;
        let material = scene
            .graphs
            .material(&look.ir, &look.words, &look.base)
            .map_err(|e| e.to_string())?;
        for indices in &g.mesh.triangles {
            let Some(p) = g.mesh.tri(indices) else {
                continue;
            };
            let face = (p[1] - p[0]).cross(p[2] - p[0]);
            let face = unit_direction(face).unwrap_or(Vec3::Z);
            let shaded = !covered(g, face, view);
            for (corner, index) in p.iter().zip(indices) {
                let normal = usize::try_from(*index)
                    .ok()
                    .and_then(|i| g.mesh.normals.get(i))
                    .and_then(|n| unit_direction(*n).ok())
                    .unwrap_or(face);
                let (p, n) = (corner.to_f32(), normal.to_f32());
                scene.tri(p, n, colour);
                if shaded {
                    scene.graphs.vertex(material, p, n, p);
                }
            }
        }
        let end = scene.tris.len();
        match runs.last_mut() {
            Some((last, run)) if *last == layer => run.end = end,
            _ => runs.push((layer, start..end)),
        }
    }
    pack_placed(&mut scene, &mut looks, geometry, decals, view)?;
    Ok((scene, runs))
}

/// Bytes the material stream may hold before a placed body's vertices go in.
fn stream_bytes(scene: &GpuScene) -> usize {
    scene.graphs.draws().iter().map(|d| d.vertices.len()).sum()
}

/// The placed bodies that `view` shows into the material stream, fewest
/// placements of a definition first: past [`MAX_GRAPH_BYTES`] the vias of a
/// large board are the ones left in plain colour, not its packages.
fn pack_placed(
    scene: &mut GpuScene,
    looks: &mut Looks,
    geometry: &[Geometry],
    decals: &[Geometry],
    view: &LayerView,
) -> Result<(), String> {
    let mut uses: HashMap<usize, usize> = HashMap::new();
    let placed: Vec<&Geometry> = geometry
        .iter()
        .chain(decals)
        .filter(|g| !streamed(g) && view.draws(ViewLayer::of(g.kind, g.layer)))
        .inspect(|g| *uses.entry(Arc::as_ptr(&g.mesh) as usize).or_default() += 1)
        .collect();
    let mut order: Vec<&Geometry> = placed;
    order.sort_by_key(|g| {
        uses.get(&(Arc::as_ptr(&g.mesh) as usize))
            .copied()
            .unwrap_or(0)
    });
    for g in order {
        if stream_bytes(scene) > MAX_GRAPH_BYTES {
            break;
        }
        let look = looks.get(g)?;
        let material = scene
            .graphs
            .material(&look.ir, &look.words, &look.base)
            .map_err(|e| e.to_string())?;
        let place = g.place.unwrap_or(Place::IDENTITY);
        for indices in &g.mesh.triangles {
            let Some(p) = g.mesh.tri(indices) else {
                continue;
            };
            let face = (p[1] - p[0]).cross(p[2] - p[0]);
            let face = unit_direction(face).unwrap_or(Vec3::Z);
            if covered(g, unit_direction(place.vector(face)).unwrap_or(face), view) {
                continue;
            }
            for (corner, index) in p.iter().zip(indices) {
                let normal = usize::try_from(*index)
                    .ok()
                    .and_then(|i| g.mesh.normals.get(i))
                    .and_then(|n| unit_direction(*n).ok())
                    .unwrap_or(face);
                let at = place.point(*corner).to_f32();
                let n = unit_direction(place.vector(normal))
                    .unwrap_or(normal)
                    .to_f32();
                scene.graphs.vertex(material, at, n, at);
            }
        }
    }
    Ok(())
}

/// Keep only the runs of `bytes` that `view` shows, in place: a move of
/// the kept bytes toward the front, never a repack. `runs` describe
/// `bytes` as [`pack`] laid it out.
pub fn keep_visible(bytes: &mut Vec<u8>, runs: &[(ViewLayer, Range<usize>)], view: &LayerView) {
    let mut end = 0;
    for (layer, run) in runs {
        if !view.draws(*layer) || run.end > bytes.len() || run.start > run.end {
            continue;
        }
        let len = run.end - run.start;
        if run.start != end {
            bytes.copy_within(run.clone(), end);
        }
        end += len;
    }
    bytes.truncate(end);
}

pub fn build(geometry: Vec<Geometry>) -> Result<ModelScene, String> {
    let (stream, runs) = pack(&geometry, &[], &LayerView::default())?;
    let mut bounds = Aabb3::EMPTY;
    let mut flat: Vec<(u64, ViewLayer, BlockBuilder)> = Vec::new();
    let mut index: HashMap<(u64, ViewLayer), usize> = HashMap::new();
    for g in &geometry {
        bounds.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
        bounds.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
        // A placed body highlights from its instanced mesh.
        let Some(item) = g.item.filter(|_| g.place.is_none()) else {
            continue;
        };
        let segment = ViewLayer::of(g.kind, g.layer);
        let at = *index.entry((item, segment)).or_insert_with(|| {
            flat.push((item, segment, BlockBuilder::default()));
            flat.len() - 1
        });
        let Some((_, _, builder)) = flat.get_mut(at) else {
            continue;
        };
        builder.begin_body();
        for indices in &g.mesh.triangles {
            if let Some(p) = g.mesh.tri(indices) {
                builder.triangle(
                    p.map(|p| p.to_f32()),
                    [[0.0; 3]; 3],
                    PaletteIndex(0),
                    TriClass::Opaque,
                );
            }
        }
        builder.end_body();
    }
    let shadows = shadows(&geometry, &bounds);
    let instanced = instance(&geometry)?;
    Ok(ModelScene {
        instanced,
        geometry,
        stream,
        runs,
        shadows,
        highlight: flat
            .into_iter()
            .map(|(item, segment, builder)| {
                (
                    item,
                    segment,
                    SceneBlock {
                        key: next_key(),
                        bytes: builder.freeze(),
                    },
                )
            })
            .collect(),
        bounds,
    })
}

/// Segments per ellipse ring and rings (radius share, weight), from the
/// middle out, fading to nothing.
const GROUND_SEGMENTS: usize = 48;
const GROUND_RINGS: [(f64, f32); 4] = [(0.0, 1.0), (0.55, 0.75), (0.85, 1.0 / 3.0), (1.0, 0.0)];
/// Contact rings: offset from the body footprint (share of its mean half
/// size) and weight.
const CONTACT_RINGS: [(f64, f32); 3] = [(0.0, 1.0), (0.15, 0.5), (0.4, 0.0)];
/// Arc steps per rounded corner of a contact ring.
const CORNER_STEPS: usize = 4;
/// Height of a contact shadow above the soldermask (mm).
const CONTACT_LIFT: f64 = 0.002;

fn shadows(geometry: &[Geometry], bounds: &Aabb3) -> Shadows {
    let mut out = Shadows::default();
    ground(&mut out.ground, bounds);
    let surface = |kind, top: bool| {
        geometry
            .iter()
            .find(|g| g.kind == kind)
            .map(|g| if top { g.max[2] } else { g.min[2] })
    };
    let (Some(top), Some(bottom)) = (
        surface(BodyKind::MaskTop, true),
        surface(BodyKind::MaskBottom, false),
    ) else {
        return out;
    };
    for body in package_bodies(geometry) {
        let corners = footprint(body);
        if body.min[2] >= top - 1e-6 {
            contact(&mut out.top, &corners, top + CONTACT_LIFT);
        } else if body.max[2] <= bottom + 1e-6 {
            contact(&mut out.bottom, &corners, bottom - CONTACT_LIFT);
        }
    }
    out
}

/// One body per package for its contact shadow: an envelope, or the
/// lowest body of a generated model (its moulding or ceramic, not an
/// overcoat or marking that sits on it). Bodies without a pick identity
/// stand alone.
fn package_bodies(geometry: &[Geometry]) -> Vec<&Geometry> {
    let mut out: Vec<&Geometry> = Vec::new();
    for body in geometry.iter().filter(|g| g.kind.is_envelope()) {
        let same = body
            .item
            .and_then(|item| out.iter().position(|b| b.item == Some(item)));
        match same {
            Some(k) if body.min[2] < out[k].min[2] => out[k] = body,
            Some(_) => {}
            None => out.push(body),
        }
    }
    out
}

fn ground(out: &mut Vec<ShadowVertex>, b: &Aabb3) {
    if b.is_empty() {
        return;
    }
    let size = b.size();
    let c = b.center();
    let z = b.min.z - 1e-3 * (1.0 + b.diagonal());
    let (rx, ry) = (size.x * 0.62 + size.z * 0.15, size.y * 0.62 + size.z * 0.15);
    if !(rx > 0.0 && ry > 0.0) {
        return;
    }
    let at = |r: f64, k: usize| {
        let a = k as f64 / GROUND_SEGMENTS as f64 * std::f64::consts::TAU;
        Vec3::new(c.x + rx * r * a.cos(), c.y + ry * r * a.sin(), z).to_f32()
    };
    for w in GROUND_RINGS.windows(2) {
        let [(r0, a0), (r1, a1)] = [w[0], w[1]];
        for k in 0..GROUND_SEGMENTS {
            let (p00, p01, p10, p11) = (at(r0, k), at(r0, k + 1), at(r1, k), at(r1, k + 1));
            out.extend([(p00, a0), (p10, a1), (p11, a1)]);
            if r0 > 0.0 {
                out.extend([(p00, a0), (p11, a1), (p01, a0)]);
            }
        }
    }
}

/// A body's footprint corners (counter-clockwise, in plan). Box bodies give
/// their own four corners; anything else falls back to its plan bounds.
fn footprint(body: &Geometry) -> Vec<[f64; 2]> {
    let mut corners: Vec<[f64; 2]> = Vec::new();
    let at = |p: &Vec3| body.place.map_or(*p, |place| place.point(*p));
    for p in body.mesh.positions.iter().map(at) {
        if !corners
            .iter()
            .any(|c| (c[0] - p.x).abs() < 1e-6 && (c[1] - p.y).abs() < 1e-6)
        {
            corners.push([p.x, p.y]);
            if corners.len() > 4 {
                break;
            }
        }
    }
    if corners.len() != 4 {
        corners = vec![
            [body.min[0], body.min[1]],
            [body.max[0], body.min[1]],
            [body.max[0], body.max[1]],
            [body.min[0], body.max[1]],
        ];
    }
    let n = corners.len() as f64;
    let center = corners
        .iter()
        .fold([0.0, 0.0], |a, c| [a[0] + c[0] / n, a[1] + c[1] / n]);
    corners.sort_by(|a, b| {
        let angle = |p: &[f64; 2]| (p[1] - center[1]).atan2(p[0] - center[0]);
        angle(a).total_cmp(&angle(b))
    });
    corners
}

/// A rounded-rectangle falloff around a footprint, at height `z`.
fn contact(out: &mut Vec<ShadowVertex>, corners: &[[f64; 2]], z: f64) {
    let n = corners.len();
    if n < 3 {
        return;
    }
    let center = corners.iter().fold([0.0, 0.0], |a, c| {
        [a[0] + c[0] / n as f64, a[1] + c[1] / n as f64]
    });
    let edge = |i: usize| {
        let (a, b) = (corners[i % n], corners[(i + 1) % n]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy).max(1e-12);
        // Outward normal of a counter-clockwise edge.
        ([dy / len, -dx / len], len)
    };
    let half = (0..n).map(|i| edge(i).1).fold(0.0, f64::max) * 0.25
        + (0..n).map(|i| edge(i).1).fold(f64::INFINITY, f64::min) * 0.25;
    let ring = |offset: f64| -> Vec<[f32; 3]> {
        let mut points = Vec::with_capacity(n * (CORNER_STEPS + 1));
        for (i, &corner) in corners.iter().enumerate() {
            let (from, _) = edge(i + n - 1);
            let (to, _) = edge(i);
            let a0 = from[1].atan2(from[0]);
            let mut a1 = to[1].atan2(to[0]);
            while a1 < a0 {
                a1 += std::f64::consts::TAU;
            }
            for s in 0..=CORNER_STEPS {
                let a = a0 + (a1 - a0) * s as f64 / CORNER_STEPS as f64;
                points.push(
                    Vec3::new(
                        corner[0] + offset * a.cos(),
                        corner[1] + offset * a.sin(),
                        z,
                    )
                    .to_f32(),
                );
            }
        }
        points
    };
    let rings: Vec<(Vec<[f32; 3]>, f32)> = CONTACT_RINGS
        .iter()
        .map(|(share, weight)| (ring(share * half), *weight))
        .collect();
    let mid = Vec3::new(center[0], center[1], z).to_f32();
    if let Some((inner, weight)) = rings.first() {
        for k in 0..inner.len() {
            let next = (k + 1) % inner.len();
            out.extend([(mid, *weight), (inner[k], *weight), (inner[next], *weight)]);
        }
    }
    for w in rings.windows(2) {
        let [(a, wa), (b, wb)] = [&w[0], &w[1]];
        for k in 0..a.len().min(b.len()) {
            let next = (k + 1) % a.len();
            out.extend([(a[k], *wa), (b[k], *wb), (b[next], *wb)]);
            out.extend([(a[k], *wa), (b[next], *wb), (a[next], *wa)]);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use pcbcraft_core::Engine;

    /// The demo board with one routed track, so every body kind is present.
    pub(crate) fn demo_project() -> Result<pcbcraft_core::Project, Box<dyn std::error::Error>> {
        let mut project = Engine::demo()?.project().clone();
        let net = project
            .nets
            .first()
            .map(|n| n.id)
            .ok_or("demo has no net")?;
        let id = project.allocate()?;
        project.tracks.push(pcbcraft_core::Track {
            id,
            net,
            start: [5.0, 5.0],
            end: [25.0, 5.0],
            width: 0.3,
            layer: pcbcraft_core::CopperLayer::Top,
            locked: false,
        });
        Ok(project)
    }

    /// The demo board with `count` through vias on a 1.27 mm grid in a strip
    /// added below it, every other one tented.
    pub(crate) fn via_project(
        count: usize,
    ) -> Result<pcbcraft_core::Project, Box<dyn std::error::Error>> {
        let mut project = demo_project()?;
        let net = project
            .nets
            .first()
            .map(|n| n.id)
            .ok_or("demo has no net")?;
        let columns = 32usize;
        let rows = count.div_ceil(columns);
        let [width, depth] = project.board_size;
        project.board_size = [
            width.max(columns as f64 * 1.27 + 6.0),
            depth + rows as f64 * 1.27 + 4.0,
        ];
        for k in 0..count {
            let id = project.allocate()?;
            project.vias.push(pcbcraft_core::Via {
                id,
                net,
                position: [
                    3.0 + (k % columns) as f64 * 1.27,
                    depth + 2.0 + (k / columns) as f64 * 1.27,
                ],
                drill: 0.3,
                diameter: 0.6,
                span: pcbcraft_core::ViaSpan::Through,
                tented: k % 2 == 0,
                locked: false,
            });
        }
        Ok(project)
    }

    /// Bytes the GPU holds for an instanced mesh: its triangles, their
    /// levels and its placements.
    fn resident(mesh: &InstancedMesh) -> usize {
        std::iter::once(&mesh.tris)
            .chain(mesh.levels.iter().map(|(t, _)| t))
            .map(|t| gpu_bytes(t))
            .sum::<usize>()
            + mesh.instances.len()
    }

    fn is_via(kind: BodyKind) -> bool {
        matches!(kind, BodyKind::ViaCopper | BodyKind::ViaBore)
    }

    /// Every via body is a placement of a shared definition mesh: none is
    /// copied into the PBR stream or the highlight blocks, each definition
    /// is one instanced mesh in the copper material path drawn at every
    /// placement, and highlighting a via draws exactly its placements.
    #[test]
    fn vias_draw_as_placements_of_one_mesh_per_definition() -> Result<(), Box<dyn std::error::Error>>
    {
        let count = 40;
        let project = via_project(count)?;
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let scene = build(model.geometry)?;
        let vias: Vec<&Geometry> = scene.geometry.iter().filter(|g| is_via(g.kind)).collect();
        assert!(vias.len() >= count, "{} via bodies", vias.len());
        assert!(
            vias.iter().all(|g| g.place.is_some()),
            "every via body is placed"
        );
        let meshes: std::collections::HashSet<usize> =
            vias.iter().map(|g| Arc::as_ptr(&g.mesh) as usize).collect();
        assert!(
            meshes.len() * 8 <= vias.len(),
            "{} meshes for {} via bodies",
            meshes.len(),
            vias.len()
        );
        // Every placed body is drawn exactly once, by its definition's mesh.
        let placed = scene.geometry.iter().filter(|g| !streamed(g)).count();
        let drawn: usize = scene.instanced.iter().map(|i| i.mesh.count()).sum();
        assert_eq!(drawn, placed);
        for group in &scene.instanced {
            assert_eq!(group.mesh.count(), group.bodies.len());
            assert_eq!(group.mesh.look, InstanceLook::Opaque);
            for (k, at) in group.bodies.iter().enumerate() {
                let g = &scene.geometry[*at];
                assert_eq!(Arc::as_ptr(&g.mesh) as usize as u64, group.mesh.key);
                assert_eq!(ViewLayer::of(g.kind, g.layer), group.layer);
                let rows = g.place.unwrap_or(Place::IDENTITY).rows_f32();
                let bytes: Vec<u8> = rows
                    .iter()
                    .flatten()
                    .flat_map(|x| x.to_le_bytes())
                    .collect();
                let at = k * solidcraft_ui_egui::gpu::INSTANCE_SIZE;
                assert_eq!(group.mesh.instances.get(at..at + 48), Some(&bytes[..]));
            }
        }
        // The vias' copper draws with the copper look the stream would carry.
        let copper = vias
            .iter()
            .find(|g| g.kind == BodyKind::ViaCopper)
            .ok_or("no via copper")?;
        assert!(
            scene
                .instanced
                .iter()
                .any(|i| i.colour == plain_colour(copper.surface) && i.layer == ViewLayer::Vias)
        );
        let unplaced: usize = scene
            .geometry
            .iter()
            .filter(|g| streamed(g))
            .map(|g| g.mesh.triangles.len())
            .sum();
        assert_eq!(scene.stream.tris.len(), unplaced * 3 * TRI_SIZE);
        let via = vias[7].item.ok_or("via has no pick id")?;
        assert!(scene.highlight.iter().all(|(item, _, _)| *item != via));
        let highlighted = std::collections::HashSet::from([via]);
        let shown: usize = scene
            .instanced
            .iter()
            .filter_map(|i| i.highlight(&scene.geometry, None, &highlighted, &highlighted))
            .map(|m| m.count())
            .sum();
        let placed = |g: &&Geometry| g.item == Some(via) && g.place.is_some();
        let bodies = scene.geometry.iter().filter(placed).count();
        assert!(bodies >= 2, "a via has its barrel and bore at least");
        // Its display-only bore shadow and tent dome are not drawn over.
        let copper = scene
            .geometry
            .iter()
            .filter(placed)
            .filter(|g| !matches!(g.kind, BodyKind::ViaBore | BodyKind::MaskCap))
            .count();
        assert!(copper >= 1 && copper < bodies);
        assert_eq!(shown, copper);
        Ok(())
    }

    /// A 10,000-via board in the 3D view: its vias are placements of the few
    /// definition meshes of the real via bodies, so the GPU holds a few
    /// hundred kilobytes of via triangles and one 52-byte placement per via
    /// body, where copying each into the PBR stream would exceed the view's
    /// cap many times over.
    #[test]
    fn ten_thousand_vias_hold_one_mesh_per_definition() -> Result<(), Box<dyn std::error::Error>> {
        let model = pcbcraft_solidcraft::project::build(&via_project(2)?)?;
        let mut geometry = model.geometry;
        // The first via's bodies (one tented, one open via are built): copied
        // to 10,000 places, as a real board's placements are.
        let via = geometry
            .iter()
            .find(|g| is_via(g.kind))
            .and_then(|g| g.item)
            .ok_or("no via")?;
        let bodies: Vec<Geometry> = geometry
            .iter()
            .filter(|g| g.item == Some(via) && g.place.is_some())
            .cloned()
            .collect();
        geometry.retain(|g| g.item != Some(via) || g.place.is_none());
        geometry.retain(|g| !is_via(g.kind));
        let base: usize = geometry
            .iter()
            .filter(|g| streamed(g))
            .map(|g| g.mesh.triangles.len())
            .sum();
        let others = geometry.iter().filter(|g| !streamed(g)).count();
        let vias = 10_000usize;
        for k in 0..vias {
            let shift = Place([
                [1.0, 0.0, 0.0, (k % 100) as f64 * 1.27],
                [0.0, 1.0, 0.0, (k / 100) as f64 * 1.27],
                [0.0, 0.0, 1.0, 0.0],
            ]);
            for b in &bodies {
                let place = b.place.ok_or("unplaced")?;
                let moved = |p: [f64; 3]| [p[0] + shift.0[0][3], p[1] + shift.0[1][3], p[2]];
                geometry.push(Geometry {
                    item: Some(1_000_000 + k as u64),
                    place: Some(shift.then(&place)),
                    min: moved(b.min),
                    max: moved(b.max),
                    ..b.clone()
                });
            }
        }
        let started = std::time::Instant::now();
        let scene = build(geometry)?;
        let built = started.elapsed();
        let placed = vias * bodies.len();
        assert_eq!(
            scene
                .instanced
                .iter()
                .map(|i| i.mesh.count())
                .sum::<usize>(),
            placed + others
        );
        // One mesh per definition body, shared by every placement.
        let tris: std::collections::HashSet<usize> = scene
            .instanced
            .iter()
            .filter(|i| i.bodies.iter().any(|b| is_via(scene.geometry[*b].kind)))
            .map(|i| Arc::as_ptr(&i.mesh.tris) as usize)
            .collect();
        assert_eq!(tris.len(), bodies.iter().filter(|b| is_via(b.kind)).count());
        assert_eq!(
            scene.stream.tris.len(),
            base * 3 * TRI_SIZE,
            "no via in the stream"
        );
        let triangles: usize = bodies.iter().map(|b| b.mesh.triangles.len()).sum();
        let copied = vias * triangles * 3 * TRI_SIZE;
        let held: usize = scene.instanced.iter().map(|i| resident(&i.mesh)).sum();
        let placements: usize = scene.instanced.iter().map(|i| i.mesh.instances.len()).sum();
        assert_eq!(
            placements,
            (placed + others) * solidcraft_ui_egui::gpu::INSTANCE_SIZE
        );
        let meshes = held - placements;
        eprintln!(
            "10,000 vias: {} bodies, {triangles} triangles a via; instanced {meshes} B of meshes + {placements} B of placements; copied {copied} B; scene built in {built:?}",
            placed
        );
        assert!(copied > 10 * MAX_PBR_BYTES, "copied: {copied}");
        assert!(meshes < 1 << 20, "via meshes: {meshes} B");
        assert!(held < 8 << 20, "on the GPU: {held} B");
        Ok(())
    }

    fn demo_scene() -> Result<ModelScene, Box<dyn std::error::Error>> {
        let model = pcbcraft_solidcraft::project::build(&demo_project()?)?;
        Ok(build(model.geometry)?)
    }

    #[test]
    fn pbr_bytes_hold_three_vertices_per_triangle_in_one_run_per_layer()
    -> Result<(), Box<dyn std::error::Error>> {
        let scene = demo_scene()?;
        let bytes = |layer: ViewLayer| {
            3 * TRI_SIZE
                * scene
                    .geometry
                    .iter()
                    .filter(|g| streamed(g) && ViewLayer::of(g.kind, g.layer) == layer)
                    .map(|g| g.mesh.triangles.len())
                    .sum::<usize>()
        };
        // The runs tile the buffer in layer order, one per layer.
        let mut at = 0;
        for window in scene.runs.windows(2) {
            assert!(window[0].0 < window[1].0, "{:?}", scene.runs);
        }
        for (layer, run) in &scene.runs {
            assert_eq!(run.start, at);
            assert_eq!(run.len(), bytes(*layer), "{layer:?}");
            at = run.end;
        }
        assert_eq!(at, scene.stream.tris.len());
        for layer in [
            ViewLayer::Bodies,
            ViewLayer::Substrate,
            ViewLayer::Mask(pcbcraft_core::Layer::Top),
            ViewLayer::Copper(pcbcraft_core::CopperLayer::Top),
        ] {
            assert!(scene.runs.iter().any(|(l, _)| *l == layer), "{layer:?}");
        }
        Ok(())
    }

    /// Hiding layers in place gives exactly the bytes a fresh pack of the
    /// shown bodies gives: the same vertices, nothing rebuilt.
    #[test]
    fn keeping_visible_runs_matches_packing_only_the_shown_bodies()
    -> Result<(), Box<dyn std::error::Error>> {
        use pcbcraft_core::{CopperLayer, Layer};
        let scene = demo_scene()?;
        let views = [
            LayerView::substrate_hidden(true),
            {
                let mut v = LayerView::default();
                v.set(ViewLayer::Mask(Layer::Top), false);
                v
            },
            {
                let mut v = LayerView::default();
                v.set(ViewLayer::Bodies, false);
                v.set(ViewLayer::Copper(CopperLayer::Top), false);
                v
            },
            LayerView::default(),
        ];
        for view in views {
            let mut kept = scene.stream.tris.clone();
            keep_visible(&mut kept, &scene.runs, &view);
            let (packed, runs) = pack(&scene.geometry, &[], &view)?;
            assert_eq!(kept, packed.tris, "{view:?}");
            assert!(runs.iter().all(|(l, _)| view.draws(*l)));
            if view != LayerView::default() {
                assert!(
                    kept.len() < scene.stream.tris.len(),
                    "{view:?} hides something"
                );
                assert!(
                    stream_bytes(&packed) < stream_bytes(&scene.stream),
                    "{view:?} hides its material stream too"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn normals_are_unit_and_outward() -> Result<(), Box<dyn std::error::Error>> {
        let scene = demo_scene()?;
        let word = |bytes: &[u8], at: usize| -> Option<f32> {
            bytes
                .get(at..at + 4)
                .and_then(|b| b.try_into().ok())
                .map(f32::from_le_bytes)
        };
        for vertex in scene.stream.tris.chunks_exact(TRI_SIZE) {
            let v: Vec<f32> = (0..6).filter_map(|i| word(vertex, 4 * i)).collect();
            assert_eq!(v.len(), 6);
            let n = &v[3..6];
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((length - 1.0).abs() < 1e-4, "{n:?}");
            assert!(v.iter().all(|x| x.is_finite()));
        }
        // Outward: every body encloses a positive volume and each
        // triangle's winding agrees with its vertex normals (solder fillets
        // are concave, so a centre test would not do).
        for g in &scene.geometry {
            assert!(g.mesh.measure().volume > 0.0, "{:?}", g.kind);
            for t in &g.mesh.triangles {
                let p = g.mesh.tri(t).ok_or("bad triangle")?;
                let face = (p[1] - p[0]).cross(p[2] - p[0]);
                for i in t {
                    let n = g.mesh.normals[*i as usize];
                    assert!(face.dot(n) > 0.0, "{:?}: {face:?} vs {n:?}", g.kind);
                }
            }
        }
        Ok(())
    }

    /// Each body's vertices in the material stream name its look's record:
    /// (base, metallic), (roughness, IOR, coat weight, coat roughness), (coat
    /// IOR, ...) and then its graph's uniforms, which hold the pattern's
    /// strength. Each pattern draws with its own shape. Masks carry their
    /// lacquer coat and orange peel, the laminate its weave, everything else
    /// neither.
    #[test]
    fn vertices_carry_each_body_s_coat_and_pattern() -> Result<(), Box<dyn std::error::Error>> {
        let scene = demo_scene()?;
        let graphs = &scene.stream.graphs;
        let mut seen = std::collections::HashSet::new();
        // Where each draw's stream has been read to.
        let mut read: HashMap<usize, usize> = HashMap::new();
        for (_, g) in ordered(&scene.geometry)
            .into_iter()
            .filter(|(_, g)| streamed(g))
        {
            let look = look_graph::lower_look(g.surface, g.detail)?;
            let (at, draw) = graphs
                .draws()
                .iter()
                .enumerate()
                .find(|(i, d)| {
                    d.shape == look.ir.shape && d.vertices.len() > read.get(i).copied().unwrap_or(0)
                })
                .ok_or("no draw of the body's shape")?;
            assert_eq!(draw.object, look.ir.object_space, "{:?}", g.kind);
            let stride = if draw.object { 40 } else { 28 };
            let start = read.entry(at).or_default();
            let shaded = g
                .mesh
                .triangles
                .iter()
                .filter_map(|t| g.mesh.tri(t))
                .filter(|p| {
                    let face = (p[1] - p[0]).cross(p[2] - p[0]);
                    !covered(
                        g,
                        unit_direction(face).unwrap_or(Vec3::Z),
                        &LayerView::default(),
                    )
                })
                .count();
            let bytes = 3 * stride * shaded;
            let body = draw
                .vertices
                .get(*start..*start + bytes)
                .ok_or("short stream")?;
            *start += bytes;
            let Some(vertex) = body.chunks_exact(stride).next() else {
                continue;
            };
            let record = u32::from_le_bytes(vertex[24..28].try_into()?) as usize;
            let page = graphs.pages().get(draw.page as usize).ok_or("no page")?;
            let word = |i: usize| f32::from_bits(page.words[record * 4 + i]);
            // The base response, then the graph's own uniforms.
            let base = g.surface.base_color();
            assert!((f64::from(word(0)) - base.x).abs() < 1e-3, "{:?}", g.kind);
            assert_eq!(word(3), g.surface.metallic() as f32, "{:?}", g.kind);
            assert_eq!(word(4), g.surface.roughness() as f32, "{:?}", g.kind);
            match g.surface.clear_coat() {
                Some(coat) => {
                    assert_eq!(word(6), 1.0, "{:?}", g.kind);
                    assert_eq!(word(7), coat.roughness() as f32, "{:?}", g.kind);
                    assert!((word(8) - 1.52).abs() < 1e-3, "{:?}", g.kind);
                    assert_eq!(word(5), 1.0, "the body under the coat has IOR 1");
                }
                None => assert_eq!(word(6), 0.0, "{:?}", g.kind),
            }
            for (i, w) in look.words.iter().enumerate() {
                assert_eq!(
                    page.words[record * 4 + 4 * BASE_VEC4 + i],
                    *w,
                    "{:?}: the graph's uniform {i}",
                    g.kind
                );
            }
            seen.insert((g.kind, g.detail.pattern));
        }
        use pcbcraft_solidcraft::material::Pattern;
        assert!(seen.contains(&(BodyKind::Substrate, Pattern::Weave)));
        assert!(seen.contains(&(BodyKind::MaskTop, Pattern::MaskPeelOverWeave)));
        assert!(seen.contains(&(BodyKind::MaskCap, Pattern::MaskPeel)));
        assert!(seen.contains(&(BodyKind::PadFinish, Pattern::None)));
        Ok(())
    }

    /// The vec4s of the base response at the head of a material record.
    const BASE_VEC4: usize = 7;

    /// Placed bodies join the material stream, one copy a placement, so their
    /// metal reflects the studio like the rest of the board.
    #[test]
    fn placed_bodies_are_shaded_by_the_material_stream() -> Result<(), Box<dyn std::error::Error>> {
        let scene = demo_scene()?;
        let vertices: usize = scene
            .stream
            .graphs
            .draws()
            .iter()
            .map(|d| d.vertices.len() / if d.object { 40 } else { 28 })
            .sum();
        let view = LayerView::default();
        let triangles: usize = scene
            .geometry
            .iter()
            .map(|g| {
                let place = g.place.unwrap_or(Place::IDENTITY);
                g.mesh
                    .triangles
                    .iter()
                    .filter_map(|t| g.mesh.tri(t))
                    .filter(|p| {
                        let face = (p[1] - p[0]).cross(p[2] - p[0]);
                        let face = unit_direction(face).unwrap_or(Vec3::Z);
                        !covered(g, unit_direction(place.vector(face)).unwrap_or(face), &view)
                    })
                    .count()
            })
            .sum();
        assert_eq!(vertices, 3 * triangles);
        // A mask's own faces are drawn; the laminate's faces under it are not.
        assert!(triangles > 0);
        Ok(())
    }

    #[test]
    fn oversized_models_are_refused() -> Result<(), Box<dyn std::error::Error>> {
        let model = pcbcraft_solidcraft::project::build(Engine::demo()?.project())?;
        let mut geometry = model.geometry;
        let one = geometry
            .iter()
            .find(|g| streamed(g))
            .cloned()
            .ok_or("no geometry")?;
        let per_copy = one.mesh.triangles.len() * 3 * TRI_SIZE;
        let copies = MAX_PBR_BYTES / per_copy.max(1) + 1;
        geometry.extend(std::iter::repeat_n(one, copies));
        assert!(build(geometry).is_err());
        Ok(())
    }

    #[test]
    fn shadows_ring_the_board_and_every_package() -> Result<(), Box<dyn std::error::Error>> {
        let scene = demo_scene()?;
        // Three ring bands of 48 segments; the centre band is a fan.
        assert_eq!(scene.shadows.ground.len(), GROUND_SEGMENTS * (3 + 6 + 6));
        let per_body = {
            let ring = 4 * (CORNER_STEPS + 1);
            3 * ring + 2 * 6 * ring
        };
        // One shadow per package, however many bodies its model has.
        let packages = package_bodies(&scene.geometry).len();
        assert_eq!(packages, 2);
        assert_eq!(
            scene.shadows.top.len() + scene.shadows.bottom.len(),
            packages * per_body
        );
        let mask = scene
            .geometry
            .iter()
            .find(|g| g.kind == BodyKind::MaskTop)
            .map(|g| g.max[2] as f32)
            .ok_or("no mask")?;
        assert!(scene.shadows.top.iter().all(|(p, w)| {
            (p[2] - mask - CONTACT_LIFT as f32).abs() < 1e-4 && (0.0..=1.0).contains(w)
        }));
        Ok(())
    }

    /// The demo board with a marking on every placed instance: its built
    /// geometry plus the decals, as the model worker appends them.
    fn marked_scene() -> Result<(ModelScene, usize), Box<dyn std::error::Error>> {
        use pcbcraft_core::library::marking::{Finish, LaidElement, LaidText, MarkingLayout};
        use pcbcraft_solidcraft::marking::{DecalCache, board};
        let project = demo_project()?;
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let layouts = project
            .placements
            .iter()
            .map(|p| {
                let layout = MarkingLayout {
                    elements: vec![LaidElement::Text(LaidText {
                        text: "103".into(),
                        height_mm: 0.3,
                        origin: [0.0, 0.0],
                    })],
                    finish: Finish::LaserEtch,
                    ..MarkingLayout::default()
                };
                (p.instance, layout)
            })
            .collect();
        let decals = board::board_decals(
            &project,
            &model.geometry,
            &model.sources,
            &layouts,
            &mut DecalCache::default(),
        );
        let count = decals.len();
        let mut geometry = model.geometry;
        geometry.extend(decals);
        Ok((build(geometry)?, count))
    }

    /// PCB 3D: decals pack in their own Markings run (the Markings toggle or
    /// hiding the bodies drops them, nothing rebuilt), take their finish's
    /// look and highlight with their instance.
    #[test]
    fn marking_decals_draw_and_hide_with_the_component_bodies()
    -> Result<(), Box<dyn std::error::Error>> {
        use pcbcraft_solidcraft::marking::{board::DECAL_KIND, finish_surface};
        let (scene, count) = marked_scene()?;
        assert!(count >= 2, "{count}");
        let plain = demo_scene()?;
        let decals: Vec<&Geometry> = scene.geometry[plain.geometry.len()..].iter().collect();
        assert_eq!(decals.len(), count);
        let triangles: usize = decals.iter().map(|g| g.mesh.triangles.len()).sum();
        // The decal vertices are exactly the Markings run; the bodies'
        // run is unchanged.
        let run = |s: &ModelScene, layer: ViewLayer| {
            s.runs
                .iter()
                .find(|(l, _)| *l == layer)
                .map(|(_, r)| r.len())
                .unwrap_or(0)
        };
        assert_eq!(
            run(&scene, ViewLayer::Bodies),
            run(&plain, ViewLayer::Bodies)
        );
        assert_eq!(run(&scene, ViewLayer::Markings), 3 * TRI_SIZE * triangles);
        assert_eq!(
            scene.stream.tris.len(),
            plain.stream.tris.len() + 3 * TRI_SIZE * triangles
        );
        let laser = finish_surface(pcbcraft_core::library::marking::Finish::LaserEtch)?;
        for g in &decals {
            assert_eq!(g.kind, DECAL_KIND);
            assert_eq!(g.surface.base_color(), laser.base_color());
            assert!(scene.highlight.iter().any(|(item, layer, _)| {
                Some(*item) == g.item && *layer == ViewLayer::Markings
            }));
        }
        // Hiding the bodies drops the decals too, and the Markings toggle
        // drops only them; shadows are unchanged.
        for hidden in [ViewLayer::Bodies, ViewLayer::Markings] {
            let mut view = LayerView::default();
            view.set(hidden, false);
            let (marked, _) = pack(&scene.geometry, &[], &view)?;
            let (unmarked, _) = pack(&plain.geometry, &[], &view)?;
            assert_eq!(marked.tris, unmarked.tris, "{hidden:?}");
            let mut kept = scene.stream.tris.clone();
            keep_visible(&mut kept, &scene.runs, &view);
            assert_eq!(kept, marked.tris, "{hidden:?}: kept without a repack");
        }
        assert_eq!(scene.shadows.top.len(), plain.shadows.top.len());
        Ok(())
    }

    #[test]
    fn highlight_blocks_are_flat_one_per_item_and_segment() -> Result<(), Box<dyn std::error::Error>>
    {
        let scene = demo_scene()?;
        for (item, segment, _) in &scene.highlight {
            let same = scene
                .highlight
                .iter()
                .filter(|(i, s, _)| i == item && s == segment)
                .count();
            assert_eq!(same, 1);
            assert!(
                scene
                    .geometry
                    .iter()
                    .any(|g| g.item == Some(*item) && ViewLayer::of(g.kind, g.layer) == *segment)
            );
        }
        // Tracks' mask caps sit on the mask layer but still highlight with
        // their track.
        assert!(
            scene
                .highlight
                .iter()
                .any(|(_, s, _)| matches!(s, ViewLayer::Mask(_)))
        );
        Ok(())
    }
}
