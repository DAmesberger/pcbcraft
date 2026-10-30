//! Studio lighting for the PCB 3D view: a key/fill/rim product-shot rig
//! that turns with the camera, a sheen light whose mirror image through the
//! board lies behind the view centre, and a ring of dim lights around the
//! mirror direction that stands in for a softbox reflected by horizontal
//! metal and soldermask. Solidcraft's PBR viewport has no environment map,
//! so these lights are the only reflections. Materials and lights do not
//! follow the UI theme.
//!
//! Records use Solidcraft's product-light layout, packed as its render
//! workspace `preview()` does (MIT, revision 4f3ac07; see ATTRIBUTION.md):
//! `[x, y, z, kind, r, g, b, outer, axis xyz, inner, ref_dist², min_dist, …]`
//! with kind 0 directional (xyz toward the light), 1 point, 4 ambient.
use pcbcraft_solidcraft::{
    geom::{Aabb3, Vec3},
    render::Camera,
};

/// Most lights the viewport shader reads.
pub const MAX_LIGHTS: usize = 21;
/// Scene-referred scale of every light, tuned by screenshot so mid-grey
/// diffuse reads about 0.3–0.5 after the shader's fixed Reinhard curve.
pub const EXPOSURE: f64 = 3.0;

const KEY: (f64, f64, f64, [f64; 3]) = (-35.0, 50.0, 1.0, [1.0, 0.97, 0.92]);
const FILL: (f64, f64, f64, [f64; 3]) = (60.0, 25.0, 0.30, [1.0, 1.0, 1.0]);
const RIM: (f64, f64, f64, [f64; 3]) = (165.0, 35.0, 0.12, [0.92, 0.96, 1.0]);
/// The sheen light's turn about the vertical, past the mirror direction.
const SHEEN_TURN: f64 = 195.0;
const SHEEN: f64 = 0.10;
/// Reflection softbox around the mirror direction: rings of (half-angle in
/// degrees, lights). Twelve lights 6° out are close enough that their
/// lobes merge into one disc on gloss mask and metal instead of dots, even
/// where a grazing view stretches them.
const RING: [(f64, usize); 2] = [(0.0, 1), (6.0, 12)];
const RING_LIGHTS: usize = 13;
const RING_SHARE: f64 = 0.0035;
const AMBIENT: f64 = 0.15;

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

/// A unit direction `azimuth` degrees around the camera's horizontal frame
/// (0 toward the camera, negative to its left) and `elevation` degrees above
/// the board on the side the camera looks from.
fn around(camera: &Camera, azimuth: f64, elevation: f64) -> Vec3 {
    let (sy, cy) = camera.yaw.sin_cos();
    let toward = Vec3::new(-sy, -cy, 0.0);
    let right = Vec3::new(cy, -sy, 0.0);
    let up = Vec3::new(0.0, 0.0, if camera.pitch < 0.0 { -1.0 } else { 1.0 });
    let (sa, ca) = azimuth.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    (toward * ca + right * sa) * ce + up * se
}

/// The light rig for `camera` looking at a model within `bounds`.
pub fn studio(camera: &Camera, bounds: &Aabb3, exposure: f64) -> Vec<[f32; 16]> {
    let diagonal = if bounds.is_empty() {
        100.0
    } else {
        bounds.diagonal().max(1.0)
    };
    let target = camera.target;
    let mut rig = Vec::with_capacity(5 + RING_LIGHTS);
    let (az, el, i, rgb) = KEY;
    let distance = 1.6 * diagonal;
    rig.push(record(
        target + around(camera, az, el) * distance,
        1.0,
        rgb,
        i * exposure,
        distance * distance,
    ));
    for (az, el, i, rgb) in [FILL, RIM] {
        rig.push(record(around(camera, az, el), 0.0, rgb, i * exposure, 1.0));
    }
    let eye = camera.eye() - target;
    let (s, c) = SHEEN_TURN.to_radians().sin_cos();
    let sheen = Vec3::new(eye.x * c - eye.y * s, eye.x * s + eye.y * c, eye.z);
    let reach = sheen.len().max(1e-3);
    rig.push(record(
        target + sheen,
        1.0,
        [1.0; 3],
        SHEEN * exposure,
        reach * reach,
    ));
    let b = camera.back();
    let mirror = Vec3::new(-b.x, -b.y, b.z).normalized().unwrap_or(Vec3::Z);
    let seed = if mirror.z.abs() > 0.9 {
        Vec3::X
    } else {
        Vec3::Z
    };
    let e1 = mirror.cross(seed).normalized().unwrap_or(Vec3::X);
    let e2 = mirror.cross(e1).normalized().unwrap_or(Vec3::Y);
    for (half, lights) in RING {
        let (st, ct) = half.to_radians().sin_cos();
        for k in 0..lights {
            let phi = k as f64 / lights as f64 * std::f64::consts::TAU;
            let d = mirror * ct + (e1 * phi.cos() + e2 * phi.sin()) * st;
            rig.push(record(d, 0.0, [1.0; 3], RING_SHARE * exposure, 1.0));
        }
    }
    rig.push(record(Vec3::ZERO, 4.0, [1.0; 3], AMBIENT * exposure, 1.0));
    rig.truncate(MAX_LIGHTS);
    rig
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use pcbcraft_solidcraft::material::{BodyKind, DEFAULT_FINISH, material};
    use pcbcraft_solidcraft::render::surface::OpaqueSurface;

    /// What the viewport's `fs_pbr` computes for one pixel: the rig's
    /// unshadowed radiance through the surface BRDF, then Reinhard.
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
            if light[3] == 4.0 {
                let base =
                    surface.base_color() * ((1.0 - surface.metallic()) / std::f64::consts::PI);
                radiance += Vec3::new(base.x * rgb.x, base.y * rgb.y, base.z * rgb.z);
                continue;
            }
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
        Ok([radiance.x, radiance.y, radiance.z].map(|c| c / (1.0 + c)))
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
        camera.fit(&board());
        camera
    }

    fn views() -> impl Iterator<Item = Camera> {
        [20.0, 45.0, 75.0, -45.0]
            .into_iter()
            .flat_map(|pitch| (0..12).map(move |k| camera(f64::from(k) * 30.0, pitch)))
    }

    #[test]
    fn exposed_gold_is_never_black() -> Result<(), Box<dyn std::error::Error>> {
        let enig = material(BodyKind::PadFinish, DEFAULT_FINISH).surface()?;
        for camera in views() {
            let rig = studio(&camera, &board(), EXPOSURE);
            let n = if camera.pitch < 0.0 {
                -Vec3::Z
            } else {
                Vec3::Z
            };
            let lum = luminance(shade(&rig, enig, n, camera.eye(), camera.target)?);
            assert!(
                lum >= 0.15,
                "yaw {} pitch {}: {lum}",
                camera.yaw,
                camera.pitch
            );
        }
        Ok(())
    }

    #[test]
    fn soldermask_shading_varies_across_the_board() -> Result<(), Box<dyn std::error::Error>> {
        let mask = material(BodyKind::MaskTop, DEFAULT_FINISH).surface()?;
        let b = board();
        for camera in views() {
            let rig = studio(&camera, &b, EXPOSURE);
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

    #[test]
    #[ignore = "tuning printout"]
    fn print_default_view() -> Result<(), Box<dyn std::error::Error>> {
        let b = board();
        let mut camera = Camera {
            fov: 0.5,
            ..Camera::default()
        };
        camera.fit(&b);
        let rig = studio(&camera, &b, EXPOSURE);
        let srgb = |c: f64| {
            let c = c.clamp(0.0, 1.0);
            let e = if c <= 0.0031308 {
                12.92 * c
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (e * 255.0).round() as u8
        };
        let body = material(BodyKind::PackageBody, DEFAULT_FINISH).surface()?;
        for (k, light) in rig.iter().enumerate() {
            let c = shade(&[*light], body, Vec3::Z, camera.eye(), camera.target)?;
            println!(
                "light {k} kind {} -> {:?}",
                light[3],
                c.map(|x| x / (1.0 - x))
            );
        }
        for kind in [
            BodyKind::MaskTop,
            BodyKind::MaskCap,
            BodyKind::PadFinish,
            BodyKind::PackageBody,
        ] {
            let s = material(kind, DEFAULT_FINISH).surface()?;
            println!("{kind:?}");
            for j in (0..5).rev() {
                let mut row = String::new();
                for i in 0..5 {
                    let p = Vec3::new(60.0 * f64::from(i) / 4.0, 40.0 * f64::from(j) / 4.0, 1.62);
                    let c = shade(&rig, s, Vec3::Z, camera.eye(), p)?;
                    row += &format!("{:?} ", c.map(srgb));
                }
                println!("{row}");
            }
        }
        Ok(())
    }

    #[test]
    fn rig_is_bounded_finite_and_turns_with_the_camera() {
        for camera in views() {
            let rig = studio(&camera, &board(), EXPOSURE);
            assert!(rig.len() <= MAX_LIGHTS);
            assert_eq!(RING.iter().map(|r| r.1).sum::<usize>(), RING_LIGHTS);
            assert_eq!(rig.len(), 3 + 1 + RING_LIGHTS + 1);
            for light in &rig {
                assert!(light.iter().all(|x| x.is_finite()));
                if light[3] == 0.0 {
                    let d = Vec3::new(light[0].into(), light[1].into(), light[2].into());
                    assert!((d.len() - 1.0).abs() < 1e-5);
                }
            }
        }
        let key = |yaw: f64| {
            let camera = camera(yaw, 45.0);
            let rig = studio(&camera, &board(), EXPOSURE);
            Vec3::new(rig[0][0].into(), rig[0][1].into(), rig[0][2].into()) - camera.target
        };
        let (a, b) = (key(0.0), key(90.0));
        assert!((a.len() - b.len()).abs() < 1e-3);
        // Turning the camera 90° about the vertical turns the key with it.
        let turned = Vec3::new(a.y, -a.x, a.z);
        assert!((turned - b).len() < 1e-3 * a.len(), "{a:?} {b:?}");
    }
}
