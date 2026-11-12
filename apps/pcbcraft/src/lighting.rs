//! Lighting for the PCB 3D view. Solidcraft's opt-in product look supplies
//! the studio: an environment (softboxes over a cyclorama) fixed in the
//! world, which every surface reflects and is lit by, so highlights slide
//! across the board as it is orbited and metal reads as metal from any side.
//! On top of it a key/fill/rim rig turns with the camera, so the model's
//! shape always reads and the side of the board the camera sees is lit. The
//! high and wall reflection lights and the ambient light the view used
//! before the environment existed are gone; the floor bounce stays (the
//! studio's lower walls are dark, and vertical metal would read black), and an ambient light
//! and stronger direct lights return when the board is seen from below,
//! where it faces the studio's dark floor. Materials and lights do not
//! follow the UI theme.
//!
//! Records use Solidcraft's product-light layout, packed as its render
//! workspace `preview()` does (MIT, revision 4f3ac07; see ATTRIBUTION.md):
//! `[x, y, z, kind, r, g, b, outer, axis xyz, inner, ref_dist², min_dist, …]`
//! with kind 0 directional (xyz toward the light), 1 point, 4 ambient.
use pcbcraft_solidcraft::{
    geom::{Aabb3, Vec3},
    viewport::Camera,
};
use solidcraft_viewport_gpu::view::{ProductLook, StudioPreset};

/// Most lights the viewport shader reads.
pub const MAX_LIGHTS: usize = 21;
/// Scene-referred scale of the radiance before PBR Neutral, tuned by
/// screenshot so the board's mask sits in the curve's linear part and
/// metal highlights roll off instead of clipping; a tenth over unity lifts
/// the overview's dark mask out of dullness with no pixel clipped (v3).
pub const EXPOSURE: f32 = 1.1;
/// The studio's share of the light against the rig below.
pub const STUDIO_INTENSITY: f32 = 1.0;
/// The studio the look reflects: Solidcraft's moody strip-light studio
/// (`STU-005`), so metal shows dark faces with bright edge streaks, gloss
/// mask a crisp streak, and its soft overhead scrim makes flat gold read
/// gold from above (chosen in the founder's look reviews, 1 and 9 Nov
/// 2026). Its turn about +Z puts one strip light's mirror image on the
/// default overview's near corner as a crisp-edged lacquer sheen: at 145°
/// (tuned against the earlier `Contrast`) the band crossed the middle of a
/// bare board as a pale haze; at 240° it softened across the far end; at
/// 285° it is a crisp sheen off the board's middle (`write_look_pictures`).
pub const STUDIO: StudioPreset = StudioPreset::Contrast;
const STUDIO_TURN_DEG: f32 = 285.0;
/// Scale of the direct lights' records.
const DIRECT: f64 = 1.0;
/// Seen from below, the board's underside faces the studio's dark floor:
/// the direct lights are scaled by this and an ambient light is added, so
/// the bottom side reads (the role the ambient light had before).
const BELOW_DIRECT: f64 = 2.5;
const BELOW_AMBIENT: f64 = 0.6;

/// Camera-relative (azimuth, elevation in degrees, intensity, colour).
const KEY: (f64, f64, f64, [f64; 3]) = (-35.0, 50.0, 0.6, [1.0, 0.97, 0.92]);
const FILL: (f64, f64, f64, [f64; 3]) = (60.0, 25.0, 0.30, [1.0, 1.0, 1.0]);
/// The rim grazes in low from behind: it outlines parts' far edges, and
/// its mirror image on the gloss mask lies past the board's far edge
/// instead of glowing across a close-up.
const RIM: (f64, f64, f64, [f64; 3]) = (165.0, 12.0, 0.12, [0.92, 0.96, 1.0]);
/// Light bounced off a pale studio floor, fixed in the world: (azimuth from
/// +X, elevation, intensity). Below the horizon, so only vertical faces
/// (termination ends, solder fillets, package sides) see it; the studio's
/// lower walls are dark, so without it vertical metal reads black.
pub const FLOOR: [(f64, f64, f64); 4] = [
    (45.0, -25.0, 0.25),
    (135.0, -25.0, 0.25),
    (225.0, -25.0, 0.25),
    (315.0, -25.0, 0.25),
];
/// Contact darkening reaches this share of the model's diagonal, within
/// [`AO_RADIUS_MM`]: about a chip's height on a board, less on a lone part.
const AO_SHARE: f64 = 0.02;
const AO_RADIUS_MM: [f64; 2] = [0.15, 1.2];

fn record(position: Vec3, kind: f32, rgb: [f64; 3], scale: f64, ref2: f64) -> [f32; 16] {
    let mut value = [0.0_f32; 16];
    value[0] = position.x as f32;
    value[1] = position.y as f32;
    value[2] = position.z as f32;
    value[3] = kind;
    for (slot, c) in value[4..7].iter_mut().zip(rgb) {
        *slot = (c * scale) as f32;
    }
    value[12] = ref2 as f32;
    value[13] = 0.001;
    value
}

/// Up toward the side of the board the camera sees.
fn side(camera: &Camera) -> f64 {
    if camera.pitch < 0.0 { -1.0 } else { 1.0 }
}

/// A unit direction `azimuth` degrees around the camera's horizontal frame
/// (0 toward the camera, negative to its left) and `elevation` degrees above
/// the board on the side the camera looks from.
fn around(camera: &Camera, azimuth: f64, elevation: f64) -> Vec3 {
    let (sy, cy) = camera.yaw.sin_cos();
    let toward = Vec3::new(-sy, -cy, 0.0);
    let right = Vec3::new(cy, -sy, 0.0);
    let up = Vec3::new(0.0, 0.0, side(camera));
    let (sa, ca) = azimuth.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    (toward * ca + right * sa) * ce + up * se
}

/// A world direction `azimuth` degrees from +X and `elevation` degrees above
/// the board on the side the camera looks from.
fn fixed(camera: &Camera, azimuth: f64, elevation: f64) -> Vec3 {
    let (sa, ca) = azimuth.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    Vec3::new(ca * ce, sa * ce, se * side(camera))
}

fn diagonal(bounds: &Aabb3) -> f64 {
    if bounds.is_empty() {
        100.0
    } else {
        bounds.diagonal().max(1.0)
    }
}

/// The direct lights for `camera` looking at a model within `bounds`.
pub fn studio(camera: &Camera, bounds: &Aabb3) -> Vec<[f32; 16]> {
    let below = camera.pitch < 0.0;
    let direct = if below { DIRECT * BELOW_DIRECT } else { DIRECT };
    let mut rig = Vec::with_capacity(4 + FLOOR.len());
    let (az, el, i, rgb) = KEY;
    let distance = 1.6 * diagonal(bounds);
    rig.push(record(
        camera.target + around(camera, az, el) * distance,
        1.0,
        rgb,
        i * direct,
        distance * distance,
    ));
    for (az, el, i, rgb) in [FILL, RIM] {
        rig.push(record(around(camera, az, el), 0.0, rgb, i * direct, 1.0));
    }
    for (az, el, i) in FLOOR {
        rig.push(record(
            fixed(camera, az, el),
            0.0,
            [1.0; 3],
            i * direct,
            1.0,
        ));
    }
    if below {
        rig.push(record(Vec3::ZERO, 4.0, [1.0; 3], BELOW_AMBIENT, 1.0));
    }
    rig.truncate(MAX_LIGHTS);
    rig
}

/// The product look: the studio fixed in the world (it does not turn with
/// the camera). Patterns are in millimetres (the frame's `units_per_mm`) at
/// glass style 7628 (the look graphs' pitch). The same for every view and model.
pub fn look() -> ProductLook {
    ProductLook {
        exposure: EXPOSURE,
        studio: STUDIO,
        rotation: STUDIO_TURN_DEG.to_radians(),
        intensity: STUDIO_INTENSITY,
        ..ProductLook::default()
    }
}

/// Ambient-occlusion radius (mm) for a model within `bounds`.
pub fn ao_radius(bounds: &Aabb3) -> f32 {
    (AO_SHARE * diagonal(bounds)).clamp(AO_RADIUS_MM[0], AO_RADIUS_MM[1]) as f32
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use pcbcraft_core::Fabrication;
    use pcbcraft_solidcraft::geom::prelude::*;
    use pcbcraft_solidcraft::material::{BodyKind, material};
    use pcbcraft_solidcraft::viewport::surface::OpaqueSurface;

    /// The rig's unshadowed direct radiance at one point through the
    /// surface's base BRDF (before the studio, exposure and tone mapping).
    pub(crate) fn shade(
        rig: &[[f32; 16]],
        surface: OpaqueSurface,
        normal: Vec3,
        eye: Vec3,
        point: Vec3,
    ) -> Result<[f64; 3], Box<dyn std::error::Error>> {
        let mut radiance = Vec3::ZERO;
        let v = eye - point;
        for light in rig {
            let rgb = Vec3::new(light[4].into(), light[5].into(), light[6].into());
            let source = Vec3::new(light[0].into(), light[1].into(), light[2].into());
            let (direction, weight) = if light[3] > 0.5 {
                let delta = source - point;
                let d = delta.len().max(f64::from(light[13]));
                (delta, f64::from(light[12]) / (d * d))
            } else {
                (source, 1.0)
            };
            let l = direction.normalized().ok_or("zero light direction")?;
            let cosine = normal.dot(l).max(0.0);
            if cosine == 0.0 {
                continue;
            }
            let f = surface.brdf(normal, v, l)?;
            let e = rgb * (weight * cosine);
            radiance += Vec3::new(f.x * e.x, f.y * e.y, f.z * e.z);
        }
        Ok([radiance.x, radiance.y, radiance.z])
    }

    pub(crate) fn luminance(c: [f64; 3]) -> f64 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    fn board() -> Aabb3 {
        Aabb3 {
            min: Vec3::new(0.0, 0.0, -0.05),
            max: Vec3::new(60.0, 40.0, 2.4),
        }
    }

    fn camera(yaw: f64, pitch: f64) -> Camera {
        let mut camera = Camera {
            fov: 0.5,
            yaw: yaw.to_radians(),
            pitch: pitch.to_radians(),
            ..Camera::default()
        };
        camera.fit(&pcbcraft_solidcraft::render::view_bounds(&board()));
        camera
    }

    fn views() -> impl Iterator<Item = Camera> {
        [20.0, 45.0, 75.0, -45.0]
            .into_iter()
            .flat_map(|pitch| (0..12).map(move |k| camera(f64::from(k) * 30.0, pitch)))
    }

    #[test]
    fn soldermask_shading_varies_across_the_board() -> Result<(), Box<dyn std::error::Error>> {
        let mask = material(
            BodyKind::MaskTop,
            &Fabrication::default(),
            &pcbcraft_solidcraft::stack::Stack::default(),
        )
        .surface()?;
        let b = board();
        for camera in views() {
            let rig = studio(&camera, &b);
            let (n, z) = if camera.pitch < 0.0 {
                (-Vec3::Z, b.min.z)
            } else {
                (Vec3::Z, b.max.z)
            };
            let mut values = vec![];
            for i in 0..5 {
                for j in 0..5 {
                    let p = Vec3::new(
                        b.min.x + (b.max.x - b.min.x) * f64::from(i) / 4.0,
                        b.min.y + (b.max.y - b.min.y) * f64::from(j) / 4.0,
                        z,
                    );
                    values.push(luminance(shade(&rig, mask, n, camera.eye(), p)?));
                }
            }
            let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = values.iter().copied().fold(0.0, f64::max);
            assert!(
                lo > 0.0 && hi >= 1.15 * lo,
                "yaw {} pitch {}: {lo}..{hi}",
                camera.yaw,
                camera.pitch
            );
        }
        Ok(())
    }

    /// The rig turns with the camera and lights the side it sees; the
    /// environment, not lights, supplies reflections and ambient light.
    #[test]
    fn rig_is_bounded_finite_and_turns_with_the_camera() {
        for camera in views() {
            let rig = studio(&camera, &board());
            let below = camera.pitch < 0.0;
            assert_eq!(rig.len(), 3 + FLOOR.len() + usize::from(below));
            for light in &rig {
                assert!(light.iter().all(|x| x.is_finite()));
                assert!(
                    light[3] == 0.0 || light[3] == 1.0 || (below && light[3] == 4.0),
                    "an ambient light only from below"
                );
                if light[3] == 0.0 {
                    let d = Vec3::new(light[0].into(), light[1].into(), light[2].into());
                    assert!((d.len() - 1.0).abs() < 1e-5);
                    // Rig lights shine on the side seen; floor bounce from
                    // below its horizon.
                    assert!(d.z.abs() > 0.1);
                }
            }
            let key = Vec3::new(rig[0][0].into(), rig[0][1].into(), rig[0][2].into());
            assert!((key - camera.target).z * side(&camera) > 0.0);
        }
        let at = |yaw: f64| {
            let camera = camera(yaw, 45.0);
            let rig = studio(&camera, &board());
            Vec3::new(rig[0][0].into(), rig[0][1].into(), rig[0][2].into()) - camera.target
        };
        let (a, b) = (at(0.0), at(90.0));
        assert!((a.len() - b.len()).abs() < 1e-3);
        // Turning the camera 90° about the vertical turns the key with it…
        let turned = Vec3::new(a.y, -a.x, a.z);
        assert!((turned - b).len() < 1e-3 * a.len(), "{a:?} {b:?}");
        // …but not the floor bounce.
        let floor = |yaw: f64| studio(&camera(yaw, 45.0), &board())[3];
        assert_eq!(floor(0.0), floor(90.0));
    }

    /// The studio is fixed in the world (the look takes no view), so
    /// highlights slide across the board as it is orbited; patterns keep
    /// their physical size.
    #[test]
    fn the_look_is_fixed_in_the_world() {
        let l = look();
        assert_eq!(l.sanitized(), l, "every value is within range");
        assert_eq!(l.studio, STUDIO);
        assert_eq!(l.room, None);
    }

    #[test]
    fn occlusion_reaches_about_a_part_s_height() {
        assert_eq!(ao_radius(&board()), 1.2);
        let part = Aabb3 {
            min: Vec3::new(-1.0, -0.5, 0.0),
            max: Vec3::new(1.0, 0.5, 0.6),
        };
        assert!((ao_radius(&part) - 0.15).abs() < 1e-6);
        assert!(ao_radius(&Aabb3::EMPTY).is_finite());
    }
}
