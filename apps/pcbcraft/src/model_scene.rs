//! GPU-ready form of a built native model, prepared on the model worker so
//! the UI thread only publishes it. PBR vertices carry each body's physical
//! look, clear coat and procedural pattern (Solidcraft's detailed PBR
//! stream, drawn with the product look) in one buffer, parts first and
//! board-level bodies after, so the
//! buffer moves into the viewport once and is never copied on the UI thread;
//! shadows are theme-free geometry with alpha weights; highlight blocks are
//! flat copies keyed by pick identity, so selection only recolours.
//!
//! The ground shadow follows Solidcraft's viewport `ground_shadow` (MIT,
//! upstream revision 4f3ac07, unchanged in the pinned fork; see
//! ATTRIBUTION.md), adapted to a Z-up board.
use pcbcraft_solidcraft::{
    geom::{Aabb3, Vec3},
    material::{BodyKind, Pattern},
    render::surface::unit_direction,
    scene::Geometry,
};
use solidcraft_ui_egui::gpu::{
    BlockBuilder, GpuScene, PBR_DETAILED_SIZE, PaletteIndex, SceneBlock, SurfacePattern, TriClass,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Upper bound on PBR vertex bytes per model: about 480 000 triangles of
/// detailed (64-byte) vertices, a little above what the earlier 48-byte
/// stream accepted under 64 MiB, so no board that loaded before is refused.
pub const MAX_PBR_BYTES: usize = 88 << 20;

/// Solidcraft's pattern for a body's procedural detail.
pub fn surface_pattern(pattern: Pattern) -> SurfacePattern {
    match pattern {
        Pattern::None => SurfacePattern::None,
        Pattern::Weave => SurfacePattern::Fr4Weave,
        Pattern::MaskPeel => SurfacePattern::SolderMask,
        Pattern::MaskPeelOverWeave => SurfacePattern::SolderMaskOverWeave,
        Pattern::Silkscreen => SurfacePattern::Silkscreen,
    }
}

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// A fresh scene or block key, shared by every slot the viewport publishes.
pub fn next_key() -> u64 {
    NEXT_KEY.fetch_add(1, Ordering::Relaxed)
}

/// Board-level bodies (hidden with the substrate) or everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Segment {
    Board,
    Parts,
}

impl Segment {
    fn of(kind: BodyKind) -> Self {
        if kind.is_board() {
            Self::Board
        } else {
            Self::Parts
        }
    }
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
    /// PBR vertices: parts, then board-level bodies. The viewport takes it.
    pub pbr: Vec<u8>,
    /// Bytes of `pbr` that are parts; the prefix shown without the board.
    pub parts_bytes: usize,
    pub shadows: Shadows,
    /// One flat block per pick identity and segment, drawn over the model in
    /// the selection colour.
    pub highlight: Vec<(u64, Segment, SceneBlock)>,
    pub bounds: Aabb3,
}

/// Detailed PBR vertex bytes of `geometry`: parts first, then board-level bodies
/// when `board` is set. Returns the bytes and the parts' share.
pub fn pack(geometry: &[Geometry], board: bool) -> Result<(Vec<u8>, usize), String> {
    let bytes_of = |board: bool| {
        geometry
            .iter()
            .filter(|g| g.kind.is_board() == board)
            .try_fold(0usize, |n, g| {
                g.mesh
                    .triangles
                    .len()
                    .checked_mul(3 * PBR_DETAILED_SIZE)
                    .and_then(|b| n.checked_add(b))
            })
    };
    let too_large = || "model too large for the 3D view".to_string();
    let parts = bytes_of(false).ok_or_else(too_large)?;
    let total = if board {
        parts
            .checked_add(bytes_of(true).ok_or_else(too_large)?)
            .ok_or_else(too_large)?
    } else {
        parts
    };
    if total > MAX_PBR_BYTES {
        return Err(too_large());
    }
    let mut scene = GpuScene::default();
    scene
        .pbr_detailed
        .try_reserve_exact(total)
        .map_err(|e| e.to_string())?;
    for segment in [false, true] {
        if segment && !board {
            break;
        }
        for g in geometry.iter().filter(|g| g.kind.is_board() == segment) {
            let pattern = surface_pattern(g.detail.pattern);
            let strength = g.detail.strength as f32;
            for indices in &g.mesh.triangles {
                let Some(p) = g.mesh.tri(indices) else {
                    continue;
                };
                let face = (p[1] - p[0]).cross(p[2] - p[0]);
                let face = unit_direction(face).unwrap_or(Vec3::Z);
                for (corner, index) in p.iter().zip(indices) {
                    let normal = usize::try_from(*index)
                        .ok()
                        .and_then(|i| g.mesh.normals.get(i))
                        .and_then(|n| unit_direction(*n).ok())
                        .unwrap_or(face);
                    scene.pbr_tri_with(
                        corner.to_f32(),
                        normal.to_f32(),
                        g.surface,
                        pattern,
                        strength,
                    );
                }
            }
        }
    }
    Ok((scene.pbr_detailed, parts))
}

pub fn build(geometry: Vec<Geometry>) -> Result<ModelScene, String> {
    let (pbr, parts_bytes) = pack(&geometry, true)?;
    let mut bounds = Aabb3::EMPTY;
    let mut flat: Vec<(u64, Segment, BlockBuilder)> = Vec::new();
    let mut index: HashMap<(u64, Segment), usize> = HashMap::new();
    for g in &geometry {
        bounds.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
        bounds.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
        let Some(item) = g.item else {
            continue;
        };
        let segment = Segment::of(g.kind);
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
    Ok(ModelScene {
        geometry,
        pbr,
        parts_bytes,
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
    for p in &body.mesh.positions {
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
            layer: pcbcraft_core::Layer::Top,
            locked: false,
        });
        Ok(project)
    }

    fn demo_scene() -> Result<ModelScene, Box<dyn std::error::Error>> {
        let model = pcbcraft_solidcraft::project::build(&demo_project()?)?;
        Ok(build(model.geometry)?)
    }

    #[test]
    fn pbr_bytes_hold_three_vertices_per_triangle_split_by_segment()
    -> Result<(), Box<dyn std::error::Error>> {
        let scene = demo_scene()?;
        let triangles = |board: bool| {
            scene
                .geometry
                .iter()
                .filter(|g| g.kind.is_board() == board)
                .map(|g| g.mesh.triangles.len())
                .sum::<usize>()
        };
        assert_eq!(scene.parts_bytes, 3 * PBR_DETAILED_SIZE * triangles(false));
        assert_eq!(
            scene.pbr.len(),
            3 * PBR_DETAILED_SIZE * (triangles(true) + triangles(false))
        );
        assert!(triangles(true) > 0 && triangles(false) > 0);
        let (parts, share) = pack(&scene.geometry, false)?;
        assert_eq!((parts.len(), share), (scene.parts_bytes, scene.parts_bytes));
        assert_eq!(parts.as_slice(), &scene.pbr[..scene.parts_bytes]);
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
        for vertex in scene.pbr.chunks_exact(PBR_DETAILED_SIZE) {
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

    /// Each vertex is Solidcraft's detailed PBR vertex: position, normal,
    /// base colour, (metallic, roughness, IOR) and the detail (pattern id
    /// plus the coat IOR in its fraction, strength, coat weight, coat
    /// roughness). Masks carry their lacquer coat and orange peel, the
    /// laminate its weave, everything else neither.
    #[test]
    fn vertices_carry_each_body_s_coat_and_pattern() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(PBR_DETAILED_SIZE, 64);
        let scene = demo_scene()?;
        let floats = |vertex: &[u8]| -> Vec<f32> {
            vertex
                .chunks_exact(4)
                .filter_map(|b| b.try_into().ok().map(f32::from_le_bytes))
                .collect()
        };
        let mut at = 0;
        let mut seen = std::collections::HashSet::new();
        for segment in [false, true] {
            for g in scene
                .geometry
                .iter()
                .filter(|g| g.kind.is_board() == segment)
            {
                let bytes = 3 * PBR_DETAILED_SIZE * g.mesh.triangles.len();
                let body = scene.pbr.get(at..at + bytes).ok_or("short buffer")?;
                at += bytes;
                let Some(vertex) = body.chunks_exact(PBR_DETAILED_SIZE).next() else {
                    continue;
                };
                let v = floats(vertex);
                assert_eq!(v.len(), 16);
                let [id, strength, weight, coat_roughness] = [v[12], v[13], v[14], v[15]];
                let pattern = surface_pattern(g.detail.pattern).id() as f32;
                assert_eq!(id.floor(), pattern, "{:?}", g.kind);
                assert!((strength - g.detail.strength as f32).abs() < 1e-6);
                match g.surface.clear_coat() {
                    Some(coat) => {
                        assert_eq!(weight, 1.0, "{:?}", g.kind);
                        assert!((coat_roughness - coat.roughness() as f32).abs() < 1e-6);
                        let ior = 1.0 + (id.fract() - 0.25) * 4.0;
                        assert!((ior - 1.52).abs() < 1e-3, "{:?}: {ior}", g.kind);
                        assert_eq!(v[11], 1.0, "the body under the coat has IOR 1");
                    }
                    None => assert_eq!(weight, 0.0, "{:?}", g.kind),
                }
                seen.insert((g.kind, g.detail.pattern));
            }
        }
        assert_eq!(at, scene.pbr.len());
        assert!(seen.contains(&(BodyKind::Substrate, Pattern::Weave)));
        assert!(seen.contains(&(BodyKind::MaskTop, Pattern::MaskPeelOverWeave)));
        assert!(seen.contains(&(BodyKind::MaskCap, Pattern::MaskPeel)));
        assert!(seen.contains(&(BodyKind::PadFinish, Pattern::None)));
        Ok(())
    }

    #[test]
    fn oversized_models_are_refused() -> Result<(), Box<dyn std::error::Error>> {
        let model = pcbcraft_solidcraft::project::build(Engine::demo()?.project())?;
        let mut geometry = model.geometry;
        let one = geometry.first().cloned().ok_or("no geometry")?;
        let per_copy = one.mesh.triangles.len() * 3 * PBR_DETAILED_SIZE;
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
                    .any(|g| g.item == Some(*item) && Segment::of(g.kind) == *segment)
            );
        }
        // Tracks' mask caps are board-level but still highlight with their track.
        assert!(scene.highlight.iter().any(|(_, s, _)| *s == Segment::Board));
        Ok(())
    }
}
