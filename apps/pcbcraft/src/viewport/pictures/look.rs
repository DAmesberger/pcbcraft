//! Measurements of the PCB 3D look, split by who owns the result.
//!
//! - The ENIG **material** test compares the pads with a neutral reference
//!   metal under the same studio, so it holds whatever the studio does.
//! - The ENIG **look** test is an art-direction target owned by the studio
//!   (Solidcraft's default) and Pcbcraft's look tuning.
//! - The finish test checks that pad copper never shows through its plated
//!   finish (the rose cast at oblique views).
//! - `look_diagnosis` prints per-material luminance and chromaticity of the
//!   showcase board from the overview to grazing views: the acceptance
//!   measure for retuning the studio and the board's look (see
//!   `docs/materials.md`, "Look diagnosis").
//!
//! Scene-referred values undo the view's tone map ([`untone`]): the
//! inverse of Khronos PBR Neutral, written from its published
//! specification, then divided by the exposure.
use super::*;
use crate::lighting;
use pcbcraft_solidcraft::material::Detail;
use pcbcraft_solidcraft::stack::Stack;
use pcbcraft_solidcraft::viewport::surface::OpaqueSurface;

/// Gold's normal-incidence reflectance, chromaticity and luminance band.
///
/// Johnson & Christy's optical constants for gold (Phys. Rev. B 6, 4370,
/// 1972), integrated to linear sRGB in Hoffman's "Physics and Math of
/// Shading" table, give F0 (1.000, 0.782, 0.344): G/R 0.78, B/R 0.34,
/// Rec. 709 luminance 0.80. The ENIG pad (`material.rs`) is that gold with
/// a little nickel showing, kept saturated: F0 (1.0, 0.73, 0.30), G/R
/// 0.73, B/R 0.30, luminance 0.76. The bands are centred on the pad's F0
/// and are wide enough to hold pure gold, and narrow enough to reject
/// brass (G/R 0.85, B/R 0.46, Hoffman), copper (G/R 0.67, B/R 0.56) and
/// a grey or rose cast.
const GOLD_G_OVER_R: (f64, f64) = (0.73, 0.08);
const GOLD_B_OVER_R: (f64, f64) = (0.30, 0.10);
/// The neutral reference metal's F0 (grey).
const REFERENCE_F0: f64 = 0.95;
/// Gold reflects about 0.76–0.80 of white; against the 0.95 reference that
/// is 0.80–0.84, so the ratio is 0.8 ± 0.1.
const GOLD_RATIO: (f64, f64) = (0.80, 0.10);

/// Inverse of Khronos PBR Neutral (the view's `look_tone`): display-linear
/// Rec. 709 in [0, 1] back to the radiance the tone map was given. Exact
/// below the curve's shoulder; above it, peaks are held below 0.995 (a
/// clipped channel has no inverse).
fn untone(c: [f64; 3]) -> [f64; 3] {
    const START: f64 = 0.8 - 0.04;
    const DESATURATION: f64 = 0.15;
    let d = 1.0 - START;
    let peak = c[0].max(c[1]).max(c[2]);
    let c = if peak < START {
        c
    } else {
        // Forward: c₂ = c₁ · n/p, then c = c₂ + (n − c₂) · g with n the new peak.
        let n = peak.min(0.995);
        let p = d * d / (1.0 - n) - d + START;
        let g = 1.0 - 1.0 / (DESATURATION * (p - n) + 1.0);
        c.map(|v| (v - n * g) / (1.0 - g) * p / n)
    };
    // Forward: the darkest channel x loses x − 6.25x² below 0.08, else 0.04.
    let lowest = c[0].min(c[1]).min(c[2]).max(0.0);
    let x = if lowest < 0.04 {
        (lowest / 6.25).sqrt()
    } else {
        lowest + 0.04
    };
    c.map(|v| v + (x - lowest))
}

/// PBR Neutral forward, from the Khronos specification, to check [`untone`].
fn tone(c: [f64; 3]) -> [f64; 3] {
    const START: f64 = 0.8 - 0.04;
    const DESATURATION: f64 = 0.15;
    let x = c[0].min(c[1]).min(c[2]);
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    let c = c.map(|v| v - offset);
    let peak = c[0].max(c[1]).max(c[2]);
    if peak < START {
        return c;
    }
    let d = 1.0 - START;
    let n = 1.0 - d * d / (peak + d - START);
    let c = c.map(|v| v * n / peak);
    let g = 1.0 - 1.0 / (DESATURATION * (peak - n) + 1.0);
    c.map(|v| v + (n - v) * g)
}

#[test]
fn untone_inverts_the_tone_map() {
    for c in [
        [0.0, 0.0, 0.0],
        [0.01, 0.02, 0.005],
        [0.3, 0.22, 0.09],
        [0.9, 0.66, 0.27],
        [2.0, 1.46, 0.6],
        [0.5, 0.5, 0.5],
    ] {
        let back = untone(tone(c));
        for (a, b) in c.iter().zip(back) {
            assert!((a - b).abs() < 1e-9 * (1.0 + a), "{c:?} -> {back:?}");
        }
    }
}

fn lin(c: u8) -> f64 {
    pcbcraft_solidcraft::material::srgb8_to_linear(c)
}

fn rec709(c: [f64; 3]) -> f64 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Statistics of a set of pixels of one picture.
#[derive(Clone, Copy, Debug, Default)]
struct Patch {
    pixels: usize,
    /// Display luminance (linear, after tone map): mean and percentiles.
    mean: f64,
    p05: f64,
    p50: f64,
    p95: f64,
    /// Share of the pixels with every channel at 254 or more.
    clipped: f64,
    /// Mean display-linear colour.
    display: [f64; 3],
    /// Mean scene-referred colour (tone map undone, exposure divided out).
    scene: [f64; 3],
}

impl Patch {
    fn of(rgba: &[u8], pixels: &[usize]) -> Option<Patch> {
        let mut lums = Vec::with_capacity(pixels.len());
        let mut display = [0.0; 3];
        let mut scene = [0.0; 3];
        let mut clipped = 0usize;
        for &i in pixels {
            let p = rgba.get(4 * i..4 * i + 3)?;
            let c = [lin(p[0]), lin(p[1]), lin(p[2])];
            lums.push(rec709(c));
            let s = untone(c);
            for k in 0..3 {
                display[k] += c[k];
                scene[k] += s[k] / f64::from(lighting::EXPOSURE);
            }
            if p.iter().all(|c| *c >= 254) {
                clipped += 1;
            }
        }
        if lums.is_empty() {
            return None;
        }
        let n = lums.len() as f64;
        lums.sort_by(f64::total_cmp);
        let at = |q: f64| lums[((lums.len() - 1) as f64 * q).round() as usize];
        Some(Patch {
            pixels: lums.len(),
            mean: lums.iter().sum::<f64>() / n,
            p05: at(0.05),
            p50: at(0.5),
            p95: at(0.95),
            clipped: clipped as f64 / n,
            display: display.map(|v| v / n),
            scene: scene.map(|v| v / n),
        })
    }
}

/// (G/R, B/R) of a linear colour.
fn chroma(c: [f64; 3]) -> (f64, f64) {
    (c[1] / c[0], c[2] / c[0])
}

fn gold_hue(c: (f64, f64)) -> bool {
    (c.0 - GOLD_G_OVER_R.0).abs() <= GOLD_G_OVER_R.1
        && (c.1 - GOLD_B_OVER_R.0).abs() <= GOLD_B_OVER_R.1
}

/// The display colour as sRGB hex, to compare with screenshots.
fn hex(c: [f64; 3]) -> String {
    let e = |v: f64| pcbcraft_solidcraft::material::linear_to_srgb8(v);
    format!("#{:02X}{:02X}{:02X}", e(c[0]), e(c[1]), e(c[2]))
}

/// `geometry` with every body of `kinds` given `surface` and no detail.
fn restyled(geometry: &[Geometry], kinds: &[BodyKind], surface: OpaqueSurface) -> Vec<Geometry> {
    geometry
        .iter()
        .cloned()
        .map(|mut g| {
            if kinds.contains(&g.kind) {
                g.surface = surface;
                g.detail = Detail::NONE;
            }
            g
        })
        .collect()
}

fn surface(
    rgb: [f64; 3],
    metallic: f64,
    roughness: f64,
) -> Result<OpaqueSurface, Box<dyn std::error::Error>> {
    Ok(OpaqueSurface::new(
        Vec3::new(rgb[0], rgb[1], rgb[2]),
        metallic,
        roughness,
        1.5,
    )?)
}

/// Pictures of `geometry` from each of `cameras`, on the board of the model
/// `v` shows (the same board, restyled).
fn shoot(
    gpu: &mut Gpu,
    v: &mut Viewport,
    geometry: Vec<Geometry>,
    cameras: &[Camera],
    (w, h): (u32, u32),
) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let stack = v
        .model
        .as_ref()
        .map_or_else(Stack::none, |m| m.stack.clone());
    v.set_model(crate::model_scene::build(geometry, &stack)?);
    let mut out = vec![];
    for camera in cameras {
        v.camera = *camera;
        out.push(gpu.draw(v, w, h, 1.0).ok_or("no picture")?);
    }
    Ok(out)
}

/// For each of `cameras`, the pixels the bodies of `kinds` cover: those
/// that change when those bodies alone are redrawn matte red and then
/// matte blue. Their specular part is the same in both, so only their own
/// diffuse light differs; the viewport's backdrop (drawn behind the model,
/// not the clear colour) and every other body stay put.
fn owned(
    gpu: &mut Gpu,
    v: &mut Viewport,
    geometry: &[Geometry],
    kinds: &[BodyKind],
    cameras: &[Camera],
    size: (u32, u32),
) -> Result<Vec<Vec<usize>>, Box<dyn std::error::Error>> {
    let red = surface([1.0, 0.0, 0.0], 0.0, 1.0)?;
    let blue = surface([0.0, 0.0, 1.0], 0.0, 1.0)?;
    let reds = shoot(gpu, v, restyled(geometry, kinds, red), cameras, size)?;
    let blues = shoot(gpu, v, restyled(geometry, kinds, blue), cameras, size)?;
    Ok(reds
        .iter()
        .zip(&blues)
        .map(|(r, b)| {
            r.chunks_exact(4)
                .zip(b.chunks_exact(4))
                .enumerate()
                .filter(|(_, (a, z))| lin(a[0]) - lin(z[0]) > 0.02 && lin(z[2]) - lin(a[2]) > 0.02)
                .map(|(i, _)| i)
                .collect()
        })
        .collect())
}

fn gpu_or_skip() -> Result<Option<Gpu>, Box<dyn std::error::Error>> {
    match Gpu::new() {
        Some(gpu) => Ok(Some(gpu)),
        None if std::env::var_os("PCBCRAFT_REQUIRE_GPU").is_some() => {
            Err("PCBCRAFT_REQUIRE_GPU is set but no GPU adapter was found".into())
        }
        None => {
            eprintln!("no GPU adapter: skipped");
            Ok(None)
        }
    }
}

/// One ENIG view of the pads alone: the gold, and the same pixels with the
/// reference metal.
struct PadView {
    yaw: f64,
    pitch: f64,
    gold: Patch,
    reference: Patch,
}

impl PadView {
    /// Scene-referred gold luminance over the reference's.
    fn ratio(&self) -> f64 {
        rec709(self.gold.scene) / rec709(self.reference.scene)
    }

    /// Gold's chromaticity with the studio's tint divided out: each channel
    /// over the reference's, scaled back to an F0 (the reference is grey).
    fn relative_chroma(&self) -> (f64, f64) {
        let rel = [0, 1, 2].map(|k| self.gold.scene[k] / self.reference.scene[k]);
        chroma(rel)
    }
}

/// The pads alone (ENIG, the demo board), flat against the background, from
/// eight sides at 40° and from straight above, drawn as gold and as the
/// reference metal of the same roughness.
fn pad_views(gpu: &mut Gpu) -> Result<Vec<PadView>, Box<dyn std::error::Error>> {
    let model = pcbcraft_solidcraft::project::build(&project(Fabrication::default())?)?;
    let pads: Vec<Geometry> = model
        .geometry
        .into_iter()
        .filter(|g| g.kind == BodyKind::PadFinish)
        .collect();
    let gold = pads.first().ok_or("no pads")?.surface;
    let reference = surface([REFERENCE_F0; 3], 1.0, gold.roughness())?;
    let mut viewport = Viewport::detached();
    viewport.set_model(crate::model_scene::build(pads.clone(), &Stack::none())?);
    let target = viewport.bounds.center();
    let distance = 1.5 * viewport.bounds.diagonal();
    let angles: Vec<(f64, f64)> = (0..8)
        .map(|k| (45.0 * f64::from(k), 40.0))
        .chain([(0.0, 85.0)])
        .collect();
    let cameras: Vec<Camera> = angles
        .iter()
        .map(|&(yaw, pitch)| {
            aim(&mut viewport, target, distance, yaw, pitch);
            viewport.camera
        })
        .collect();
    let size = (256, 256);
    let kinds = [BodyKind::PadFinish];
    let golds = shoot(gpu, &mut viewport, pads.clone(), &cameras, size)?;
    let references = shoot(
        gpu,
        &mut viewport,
        restyled(&pads, &kinds, reference),
        &cameras,
        size,
    )?;
    let owned = owned(gpu, &mut viewport, &pads, &kinds, &cameras, size)?;
    let mut views = vec![];
    for (((&(yaw, pitch), gold), reference), pixels) in
        angles.iter().zip(&golds).zip(&references).zip(&owned)
    {
        assert!(pixels.len() > 50, "the pads cover pixels: {}", pixels.len());
        views.push(PadView {
            yaw,
            pitch,
            gold: Patch::of(gold, pixels).ok_or("no pad pixels")?,
            reference: Patch::of(reference, pixels).ok_or("no pad pixels")?,
        });
    }
    for v in &views {
        let (rg, rb) = v.relative_chroma();
        let (sg, sb) = chroma(v.gold.scene);
        let (dg, db) = chroma(v.gold.display);
        let (eg, eb) = chroma(v.reference.scene);
        eprintln!(
            "yaw {:>3} pitch {:>2}: gold {} lum {:.3} p95 {:.3} clip {:.3} | ratio {:.3} | \
             G/R B/R relative {rg:.3} {rb:.3}, scene {sg:.3} {sb:.3}, display {dg:.3} {db:.3} | \
             reference {} scene {eg:.3} {eb:.3} clip {:.3}",
            v.yaw,
            v.pitch,
            hex(v.gold.display),
            v.gold.mean,
            v.gold.p95,
            v.gold.clipped,
            v.ratio(),
            hex(v.reference.display),
            v.reference.clipped,
        );
    }
    Ok(views)
}

/// ENIG is gold whatever the studio: against a neutral reference metal of
/// the same roughness, seen from eight sides at 40°, the pads reflect
/// 0.8 ± 0.1 of the reference's light in every view, and with the
/// studio's tint divided out their chromaticity is gold's F0 (G/R 0.73 ±
/// 0.08, B/R 0.30 ± 0.10; sources at [`GOLD_G_OVER_R`]) in at least six
/// of the eight. Both are measured on scene-referred radiance (the tone
/// map undone), so they test the shading, not the studio or the curve.
#[test]
fn exposed_gold_is_gold_against_a_reference_metal() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = gpu_or_skip()? else {
        return Ok(());
    };
    let views = pad_views(&mut gpu)?;
    let sides = views.get(..8).ok_or("eight views")?;
    for v in sides {
        let ratio = v.ratio();
        assert!(
            (ratio - GOLD_RATIO.0).abs() <= GOLD_RATIO.1,
            "yaw {}: gold reflects {ratio:.3} of the reference",
            v.yaw
        );
    }
    let gold = sides
        .iter()
        .filter(|v| gold_hue(v.relative_chroma()))
        .count();
    assert!(
        gold >= 6,
        "gold's hue in {gold} of 8 views: {:?}",
        sides
            .iter()
            .map(PadView::relative_chroma)
            .collect::<Vec<_>>()
    );
    Ok(())
}

/// Art-direction target, owned by the studio (Solidcraft's default, which
/// Pcbcraft uses) and Pcbcraft's look tuning, not physics: ENIG never goes
/// black (every view's mean display luminance ≥ 0.10), one of eight views
/// at 40° shows a clear highlight (95th-percentile pad luminance ≥ 0.5),
/// and from straight above the pads read as light gold (mean ≥ 0.30, in
/// gold's hue band), by a soft top light rather than a brighter F0.
#[test]
fn exposed_gold_reads_as_gold_in_the_studio() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = gpu_or_skip()? else {
        return Ok(());
    };
    let views = pad_views(&mut gpu)?;
    let sides = views.get(..8).ok_or("eight views")?;
    let top = views.get(8).ok_or("the top view")?;
    let lowest = sides
        .iter()
        .map(|v| v.gold.mean)
        .fold(f64::INFINITY, f64::min);
    let highlight = sides.iter().map(|v| v.gold.p95).fold(0.0, f64::max);
    assert!(lowest >= 0.10, "darkest view {lowest:.3}");
    assert!(
        highlight >= 0.5,
        "no view catches a highlight: p95 {highlight:.3}"
    );
    assert!(
        top.gold.mean >= 0.30,
        "from above the pads are dim: {:.3}",
        top.gold.mean
    );
    assert!(
        gold_hue(chroma(top.gold.display)),
        "from above the pads are not gold: {:?}",
        chroma(top.gold.display)
    );
    Ok(())
}

/// A pad's plated finish covers its copper's top and sides 5 µm thick, so
/// from above the board, at any angle, the finish hides the copper: on the
/// showcase, pad copper covers at most 5 % as many pixels as the finish.
#[test]
fn the_finish_hides_the_pad_copper_under_it() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = gpu_or_skip()? else {
        return Ok(());
    };
    let model = pcbcraft_solidcraft::project::build(&showcase_project()?)?;
    let geometry = model.geometry.clone();
    let mut v = Viewport::detached();
    v.set_model(crate::model_scene::build(geometry.clone(), &model.stack)?);
    let b = v.bounds;
    let cameras: Vec<Camera> = [(25.0, 40.0), (25.0, 25.0), (0.0, 85.0)]
        .iter()
        .map(|&(yaw, pitch)| {
            aim(&mut v, b.center(), 0.75 * b.diagonal(), yaw, pitch);
            v.camera
        })
        .collect();
    let size = (640, 480);
    let finish = owned(
        &mut gpu,
        &mut v,
        &geometry,
        &[BodyKind::PadFinish],
        &cameras,
        size,
    )?;
    let copper = owned(
        &mut gpu,
        &mut v,
        &geometry,
        &[BodyKind::PadCopper],
        &cameras,
        size,
    )?;
    for ((camera, finish), copper) in cameras.iter().zip(&finish).zip(&copper) {
        let pitch = camera.pitch.to_degrees();
        eprintln!(
            "pitch {pitch:.0}: finish {} pixels, pad copper {}",
            finish.len(),
            copper.len()
        );
        assert!(finish.len() > 50, "pitch {pitch:.0}: the finish shows");
        assert!(
            copper.len() as f64 <= 0.05 * finish.len() as f64,
            "pitch {pitch:.0}: pad copper {} pixels against the finish's {}",
            copper.len(),
            finish.len()
        );
    }
    Ok(())
}

/// The laser-etched marking on the WROOM-3 module's can shows at every
/// tilt, not only from straight down: seen from above and at 40°, 25° and
/// 10°, the marking's own pixels (those that change when only the decals
/// are redrawn red and then blue) number at least half as many as from
/// above, foreshortening aside (each count is divided by the sine of its
/// pitch, the share of the can's top the view sees).
#[test]
fn the_module_marking_shows_at_every_tilt() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = gpu_or_skip()? else {
        return Ok(());
    };
    let engine = crate::scene::fixtures::wroom_board()?;
    let project = engine.project().clone();
    let model = pcbcraft_solidcraft::project::build(&project)?;
    let mut geometry = model.geometry.clone();
    let decals = pcbcraft_solidcraft::marking::board::BoardMarkings::default().decals(
        &project,
        &model.geometry,
        &model.sources,
    );
    assert!(!decals.is_empty(), "the module is marked");
    geometry.extend(decals);
    let mut v = Viewport::detached();
    v.set_model(crate::model_scene::build(geometry.clone(), &model.stack)?);
    let can = geometry
        .iter()
        .filter(|g| g.kind == BodyKind::Marking)
        .fold(pcbcraft_solidcraft::geom::Aabb3::EMPTY, |mut b, g| {
            b.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
            b.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
            b
        });
    let pitches = [90.0, 40.0, 25.0, 10.0];
    let cameras: Vec<Camera> = pitches
        .iter()
        .map(|&pitch| {
            aim(&mut v, can.center(), 2.0 * can.diagonal(), 30.0, pitch);
            v.camera
        })
        .collect();
    let owned = owned(
        &mut gpu,
        &mut v,
        &geometry,
        &[BodyKind::Marking],
        &cameras,
        (640, 480),
    )?;
    let seen: Vec<f64> = pitches
        .iter()
        .zip(&owned)
        .map(|(pitch, pixels)| pixels.len() as f64 / f64::to_radians(*pitch).sin())
        .collect();
    eprintln!("marking pixels per unit of seen top: {seen:?}");
    let above = seen.first().copied().unwrap_or(0.0);
    assert!(above > 200.0, "the marking shows from above: {above}");
    for (pitch, n) in pitches.iter().zip(&seen) {
        assert!(
            *n >= 0.5 * above,
            "pitch {pitch}: {n} against {above} from above"
        );
    }
    Ok(())
}

/// A diagnosis view's camera: target, distance, yaw and pitch; `None`:
/// the default overview.
type Aim = Option<(Vec3, f64, f64, f64)>;

/// What the diagnosis splits the board into.
const MATERIALS: [(&str, &[BodyKind]); 6] = [
    (
        "mask",
        &[BodyKind::MaskTop, BodyKind::MaskBottom, BodyKind::MaskCap],
    ),
    ("gold", &[BodyKind::PadFinish]),
    ("silk", &[BodyKind::Silkscreen]),
    // The laminate a mask opening leaves bare is drawn by its own film.
    (
        "laminate",
        &[
            BodyKind::Substrate,
            BodyKind::OpenLaminate,
            BodyKind::EdgeDielectric,
        ],
    ),
    // Pad copper lies under its finish (5 µm), so it should not show.
    ("pad-copper", &[BodyKind::PadCopper]),
    // The other copper: edge stackup lines, tracks and fills under the mask.
    (
        "copper",
        &[
            BodyKind::CopperTrack,
            BodyKind::ViaCopper,
            BodyKind::FillCopper,
            BodyKind::EdgeCopper,
        ],
    ),
];

/// Look diagnosis of the showcase board: for each view and material, the
/// pixel count, display luminance (mean, 5th, 50th and 95th percentile),
/// clipped share, the mean display colour (sRGB hex) and its G/R and B/R,
/// both display-linear and scene-referred (tone map undone).
///
/// A material's pixels are those that change when its bodies alone are
/// redrawn as matte red and then matte blue: their specular part is the
/// same in both, so only that material's diffuse light differs.
///
/// `cargo test --profile ci -p pcbcraft look_diagnosis -- --ignored
/// --nocapture`. `PCBCRAFT_LOOK_OUT=file.tsv` writes the table;
/// `PCBCRAFT_LOOK_BASELINE=file.tsv` prints each row's change from an
/// earlier one; `PCBCRAFT_LOOK_BOARD=file.pcbcraft` measures that design
/// instead of the showcase. Knobs: `PCBCRAFT_LOOK_OFF=coat,detail` draws the board's
/// looks without clear coats or without surface detail (mask peel, silk
/// grain, weave); `PCBCRAFT_TWEAK` (`lights=0`, `intensity=0`,
/// `exposure=…`, `rotation=…`, `studio=…`) as for the pictures.
#[test]
#[ignore = "look diagnosis"]
fn look_diagnosis() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let off = std::env::var("PCBCRAFT_LOOK_OFF").unwrap_or_default();
    // `PCBCRAFT_LOOK_BOARD=file.pcbcraft` diagnoses that design instead.
    let project = match std::env::var_os("PCBCRAFT_LOOK_BOARD") {
        Some(path) => {
            let file = pcbcraft_io::read_project(std::path::Path::new(&path))?;
            let mut engine = pcbcraft_core::Engine::default();
            engine.replace_document(file)?;
            engine.project().clone()
        }
        None => showcase_project()?,
    };
    let model = pcbcraft_solidcraft::project::build(&project)?;
    let mut geometry = model.geometry.clone();
    for g in &mut geometry {
        if off.contains("detail") {
            g.detail = Detail::NONE;
        }
        if off.contains("coat") && g.surface.clear_coat().is_some() {
            let s = g.surface;
            let b = s.base_color();
            g.surface = OpaqueSurface::new(b, s.metallic(), s.roughness(), s.dielectric_ior())?;
        }
    }
    let mut v = Viewport::detached();
    v.set_model(crate::model_scene::build(geometry.clone(), &model.stack)?);
    v.fit_to(4.0 / 3.0);
    let overview = v.camera;
    let b = v.bounds;
    let near = 0.75 * b.diagonal();
    // Close-ups look at the board's middle, on the mask's top.
    let top = geometry
        .iter()
        .filter(|g| g.kind == BodyKind::MaskTop)
        .map(|g| g.max[2])
        .fold(f64::NEG_INFINITY, f64::max);
    if !top.is_finite() {
        return Err("the board has no top mask".into());
    }
    let middle = Vec3::new(b.center().x, b.center().y, top);
    // (name, target, distance, yaw, pitch); `None`: the default overview.
    let views: [(&str, Aim); 9] = [
        ("overview", None),
        ("turned-40", Some((b.center(), near, 25.0, 40.0))),
        ("top-85", Some((b.center(), near, 0.0, 85.0))),
        ("oblique-25", Some((b.center(), near, 25.0, 25.0))),
        ("oblique-25-far", Some((b.center(), near, 205.0, 25.0))),
        ("grazing-10", Some((b.center(), near, 25.0, 10.0))),
        ("grazing-5", Some((b.center(), near, 205.0, 5.0))),
        ("close-oblique-30", Some((middle, 12.0, 30.0, 30.0))),
        ("close-grazing-12", Some((middle, 12.0, 30.0, 12.0))),
    ];
    let cameras: Vec<Camera> = views
        .iter()
        .map(|(_, aimed)| {
            v.camera = overview;
            if let Some((target, distance, yaw, pitch)) = aimed {
                aim(&mut v, *target, *distance, *yaw, *pitch);
            }
            v.camera
        })
        .collect();
    let size = (640, 480);
    let board = shoot(&mut gpu, &mut v, geometry.clone(), &cameras, size)?;
    let mut rows = vec![];
    for (material, kinds) in MATERIALS {
        let owned = owned(&mut gpu, &mut v, &geometry, kinds, &cameras, size)?;
        for (((view, _), picture), pixels) in views.iter().zip(&board).zip(&owned) {
            rows.push((format!("{view}\t{material}"), Patch::of(picture, pixels)));
        }
    }
    let header = "view\tmaterial\tpixels\tmean\tp05\tp50\tp95\tclipped\thex\tdisp_g/r\tdisp_b/r\tscene_g/r\tscene_b/r";
    let mut table = vec![header.to_string()];
    for (key, patch) in &rows {
        let p = patch.unwrap_or_default();
        let (dg, db) = chroma(p.display);
        let (sg, sb) = chroma(p.scene);
        table.push(format!(
            "{key}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.4}\t{}\t{dg:.3}\t{db:.3}\t{sg:.3}\t{sb:.3}",
            p.pixels,
            p.mean,
            p.p05,
            p.p50,
            p.p95,
            p.clipped,
            hex(p.display),
        ));
    }
    let text = table.join("\n");
    eprintln!("{text}");
    if let Some(path) = std::env::var_os("PCBCRAFT_LOOK_OUT") {
        std::fs::write(path, format!("{text}\n"))?;
    }
    if let Some(path) = std::env::var_os("PCBCRAFT_LOOK_BASELINE") {
        let before = std::fs::read_to_string(path)?;
        eprintln!("change from the baseline (mean, p95, scene G/R, scene B/R):");
        for row in table.iter().skip(1) {
            let now: Vec<&str> = row.split('\t').collect();
            let Some(was) = before
                .lines()
                .find(|l| l.split('\t').take(2).eq(now.iter().copied().take(2)))
            else {
                continue;
            };
            let was: Vec<&str> = was.split('\t').collect();
            let delta = |i: usize| -> f64 {
                let parse = |s: &[&str]| {
                    s.get(i)
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(f64::NAN)
                };
                parse(&now) - parse(&was)
            };
            eprintln!(
                "{}\t{}\t{:+.3}\t{:+.3}\t{:+.3}\t{:+.3}",
                now.first().unwrap_or(&""),
                now.get(1).unwrap_or(&""),
                delta(3),
                delta(6),
                delta(11),
                delta(12)
            );
        }
    }
    Ok(())
}

#[test]
#[ignore = "temp"]
fn temp_finish_top() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = Gpu::new() else {
        return Err("no gpu".into());
    };
    let model = pcbcraft_solidcraft::project::build(&showcase_project()?)?;
    for drop in [false, true] {
        let geometry: Vec<Geometry> = model
            .geometry
            .iter()
            .filter(|g| !drop || !matches!(g.kind, BodyKind::Solder | BodyKind::Termination))
            .cloned()
            .collect();
        let mut v = Viewport::detached();
        v.set_model(crate::model_scene::build(geometry.clone(), &model.stack)?);
        let b = v.bounds;
        aim(&mut v, b.center(), 0.75 * b.diagonal(), 0.0, 85.0);
        let cam = v.camera;
        let px = owned(
            &mut gpu,
            &mut v,
            &geometry,
            &[BodyKind::PadFinish],
            &[cam],
            (640, 480),
        )?;
        eprintln!("drop films {drop}: finish {}", px[0].len());
    }
    Ok(())
}

/// The PCB 3D view's overview, drawn as the window opens it, in the
/// Realistic and the Standard look, written to `PCBCRAFT_PICTURES`: the
/// showcase, a new project's bare board and each design named in
/// `PCBCRAFT_LOOK_BOARDS` (`:`-separated paths). Evidence for look tuning.
#[test]
#[ignore = "writes look pictures"]
fn write_look_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") else {
        return Err("set PCBCRAFT_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let mut boards = vec![
        ("showcase".to_owned(), showcase_project()?),
        (
            "bare".to_owned(),
            pcbcraft_core::Engine::default().project().clone(),
        ),
    ];
    for path in std::env::var("PCBCRAFT_LOOK_BOARDS")
        .unwrap_or_default()
        .split(':')
        .filter(|p| !p.is_empty())
    {
        let file = pcbcraft_io::read_project(std::path::Path::new(path))?;
        let mut engine = pcbcraft_core::Engine::default();
        engine.replace_document(file)?;
        let name = std::path::Path::new(path)
            .file_stem()
            .map_or("board".into(), |s| s.to_string_lossy().into_owned());
        boards.push((name, engine.project().clone()));
    }
    for (name, project) in boards {
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let mut v = Viewport::detached();
        v.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
        v.fit_to(4.0 / 3.0);
        for (look, realistic) in [("realistic", true), ("standard", false)] {
            v.set_realistic(realistic);
            let (w, h) = (1280, 960);
            let rgba = gpu.draw(&mut v, w, h, 2.0).ok_or("no picture")?;
            let png = pcbcraft_solidcraft::viewport::png(w as usize, h as usize, rgba)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("{name}-{look}.png")), png)?;
        }
    }
    Ok(())
}
