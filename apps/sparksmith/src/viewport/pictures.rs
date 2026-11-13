//! Headless pictures of the PCB 3D view: the viewport's own frame drawn by
//! Solidcraft's renderer on an offscreen device, as the window draws it
//! (scene target with depth, blitted to a target without). The prepare,
//! paint and read-back steps follow Solidcraft's `ui-egui` offscreen test
//! helper (MIT, fork revision d53b131; see ATTRIBUTION.md).
//!
//! Without a GPU adapter the tests return early: they measure what the
//! renderer draws and cannot run without one. With `SPARKSMITH_REQUIRE_GPU`
//! set, a missing adapter is a failure instead, so a green run there is
//! evidence of the look.
use super::*;
use crate::layer_view::ViewLayer;
use egui_wgpu::wgpu;
use solidcraft_viewport_gpu::view::StudioPreset;
use sparksmith_core::library::package_form::{typical_nm as nm, typical_point as pt};
use sparksmith_core::{Fabrication, Layer, MaskColour, PadFinish, Project};
use sparksmith_solidcraft::material::BodyKind;

/// What the window clears to behind the model (the light theme's sweep,
/// mid-way).
const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.72,
    g: 0.73,
    b: 0.75,
    a: 1.0,
};
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One offscreen device at a time. Windows' software adapter (WARP, what a
/// hosted Windows runner has) crashes the test process with an access
/// violation when several tests drive devices at once; one at a time, every
/// picture test draws and passes there.
static ONE_DEVICE: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    resources: egui_wgpu::CallbackResources,
    /// Declared last, so it is released after the device is dropped.
    _one: std::sync::MutexGuard<'static, ()>,
}

impl Gpu {
    fn new() -> Option<Gpu> {
        let one = ONE_DEVICE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        desc.backends = wgpu::Backends::PRIMARY;
        let instance = wgpu::Instance::new(desc);
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .ok()?;
        let window = GpuTarget {
            format: FORMAT,
            depth: None,
            samples: 1,
            tess: false,
            oit: false,
        };
        let scene = GpuTarget {
            depth: Some(wgpu::TextureFormat::Depth32Float),
            ..window
        };
        let mut renderer = Renderer::with_window_target(&device, scene, window, None);
        for preset in StudioPreset::ALL {
            renderer.set_studio_environment(&device, &queue, &StudioEnvironment::bake(preset));
        }
        let mut resources = egui_wgpu::CallbackResources::default();
        resources.insert(renderer);
        Some(Gpu {
            device,
            queue,
            resources,
            _one: one,
        })
    }

    /// Tuning overrides from `SPARKSMITH_TWEAK` (`ao=…,exposure=…,intensity=…,
    /// lights=…`), for comparing pictures; nothing is overridden without it.
    fn tweak(frame: &mut Frame) {
        let Ok(spec) = std::env::var("SPARKSMITH_TWEAK") else {
            return;
        };
        for item in spec.split(',') {
            let Some((key, value)) = item.split_once('=') else {
                continue;
            };
            let Ok(value) = value.parse::<f32>() else {
                continue;
            };
            match (key, frame.product_look.as_mut()) {
                ("ao", _) => frame.ao_radius = value,
                ("exposure", Some(look)) => look.exposure = value,
                ("intensity", Some(look)) => look.intensity = value,
                ("rotation", Some(look)) => look.rotation = value.to_radians(),
                ("studio", Some(look)) => {
                    if let Some(preset) = StudioPreset::from_id(value as u32) {
                        look.studio = preset;
                    }
                }
                ("lights", _) => {
                    for light in frame.product_lights.iter_mut().flatten() {
                        for c in &mut light[4..7] {
                            *c *= value;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The viewport's frame at `w` × `h` pixels (supersampled `ssaa` times),
    /// as straight RGBA.
    fn draw(&mut self, viewport: &mut Viewport, w: u32, h: u32, ssaa: f32) -> Option<Vec<u8>> {
        // A look or layer change packs the stream again off the thread: as
        // the window does, draw once it has landed.
        for _ in 0..2000 {
            if !viewport.receive_repack() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Material shapes compile over a few frames (their base response until then).
        let mut picture = self.draw_once(viewport, w, h, ssaa);
        for _ in 0..256 {
            let pending = self
                .resources
                .get::<Renderer>()
                .is_some_and(|r| r.material_stats().pending > 0);
            if !pending {
                break;
            }
            picture = self.draw_once(viewport, w, h, ssaa);
        }
        picture
    }

    /// One frame, as the window draws it.
    fn draw_once(&mut self, viewport: &mut Viewport, w: u32, h: u32, ssaa: f32) -> Option<Vec<u8>> {
        // As `show` does: shadows follow the side seen.
        viewport.publish_shadows();
        let view_proj = viewport
            .camera
            .view_proj_bounds(
                f64::from(w) / f64::from(h),
                &sparksmith_solidcraft::render::view_bounds(&viewport.bounds),
            )
            .to_f32();
        let mut frame = viewport.frame(view_proj, [w as f32, h as f32], ssaa);
        Self::tweak(&mut frame);
        let cb = ViewportCallback(frame);
        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sparksmith picture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = color.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: 1.0,
        };
        let extra = egui_wgpu::CallbackTrait::prepare(
            &cb,
            &self.device,
            &self.queue,
            &screen,
            &mut encoder,
            &mut self.resources,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("sparksmith picture"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(BACKGROUND),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32));
            let info = egui::PaintCallbackInfo {
                viewport: rect,
                clip_rect: rect,
                pixels_per_point: 1.0,
                screen_size_px: [w, h],
            };
            egui_wgpu::CallbackTrait::paint(&cb, info, &mut pass, &self.resources);
        }
        let row = (w * 4).div_ceil(256) * 256;
        let read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sparksmith picture read"),
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &read,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue
            .submit(extra.into_iter().chain([encoder.finish()]));
        let slice = read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let bytes = slice.get_mapped_range().ok()?.to_vec();
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let at = (y * row) as usize;
            out.extend_from_slice(bytes.get(at..at + (w * 4) as usize)?);
        }
        Some(out)
    }
}

fn project(fabrication: Fabrication) -> Result<Project, Box<dyn std::error::Error>> {
    let mut project = crate::model_scene::tests::demo_project()?;
    project.fabrication = fabrication;
    Ok(project)
}

fn viewport(project: &Project) -> Result<Viewport, Box<dyn std::error::Error>> {
    let model = sparksmith_solidcraft::project::build(project)?;
    let mut viewport = Viewport::detached();
    viewport.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
    viewport.fit_to(4.0 / 3.0);
    Ok(viewport)
}

/// Point the camera at `target` from `distance` (mm), `yaw` and `pitch` in
/// degrees.
fn aim(viewport: &mut Viewport, target: Vec3, distance: f64, yaw: f64, pitch: f64) {
    viewport.camera.target = target;
    viewport.camera.distance = distance;
    viewport.camera.yaw = yaw.to_radians();
    viewport.camera.pitch = pitch.to_radians();
}

fn centre(viewport: &Viewport, kind: BodyKind) -> Option<Vec3> {
    let model = viewport.model.as_ref()?;
    let g = model.geometry.iter().find(|g| g.kind == kind)?;
    Some(Vec3::new(
        (g.min[0] + g.max[0]) / 2.0,
        (g.min[1] + g.max[1]) / 2.0,
        g.max[2],
    ))
}

fn luminance(p: &[u8]) -> f64 {
    let lin = |c: u8| sparksmith_solidcraft::material::srgb8_to_linear(c);
    0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2])
}

/// The ENIG material and look tests and the look diagnosis.
mod look;

/// The follow-the-net look as drawn, and its pictures.
mod follow;

/// The ESP32-S31 example board's looks.
mod esp32;

/// The default overview of the showcase board is not flat: the studio's
/// strip light lies on part of the gloss mask as a lacquer sheen, so the
/// brightest board pixels stand well above the typical ones, while almost
/// nothing clips to white.
#[test]
fn the_overview_shows_a_sheen_on_the_mask_without_clipping()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = Gpu::new() else {
        if std::env::var_os("SPARKSMITH_REQUIRE_GPU").is_some() {
            return Err("SPARKSMITH_REQUIRE_GPU is set but no GPU adapter was found".into());
        }
        eprintln!("no GPU adapter: skipped");
        return Ok(());
    };
    let mut v = viewport(&showcase_project()?)?;
    let (w, h) = (320, 240);
    let rgba = gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?;
    let bg = |c: u8, b: f64| (f64::from(c) / 255.0 - b).abs() < 0.06;
    let mut board: Vec<f64> = rgba
        .chunks_exact(4)
        .filter(|p| !(bg(p[0], BACKGROUND.r) && bg(p[1], BACKGROUND.g) && bg(p[2], BACKGROUND.b)))
        .filter(|p| p[1] > p[0] && p[1] > p[2])
        .map(luminance)
        .collect();
    board.sort_by(f64::total_cmp);
    assert!(
        board.len() > 5_000,
        "the mask covers the frame: {}",
        board.len()
    );
    let at = |q: f64| board[((board.len() - 1) as f64 * q) as usize];
    let (median, bright) = (at(0.5), at(0.95));
    let clipped = rgba
        .chunks_exact(4)
        .filter(|p| p[..3].iter().all(|c| *c >= 254))
        .count();
    eprintln!("overview mask luminance: median {median:.4}, 95th {bright:.4}, clipped {clipped}");
    assert!(bright >= 2.0 * median, "a flat board: {median} .. {bright}");
    assert!(
        clipped as f64 <= 0.002 * f64::from(w * h),
        "{clipped} pixels clip"
    );
    Ok(())
}

/// Pictures for tuning and evidence, written to `SPARKSMITH_PICTURES`.
#[test]
#[ignore = "writes tuning pictures"]
fn write_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let mut save =
        |viewport: &mut Viewport, name: &str| -> Result<(), Box<dyn std::error::Error>> {
            let (w, h) = (1024, 768);
            let rgba = gpu.draw(viewport, w, h, 2.0).ok_or("no picture")?;
            let clipped = rgba
                .chunks_exact(4)
                .filter(|p| p[..3].iter().all(|c| *c >= 254))
                .count();
            eprintln!(
                "{name}: {:.3} % of pixels at white",
                100.0 * clipped as f64 / f64::from(w * h)
            );
            let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("{name}.png")), png)?;
            Ok(())
        };
    let only = std::env::var("SPARKSMITH_PICTURES_ONLY").unwrap_or_default();
    if only.is_empty() || only.contains("generated") {
        // The demo board's generated chips up close, and each package
        // preset's generated model as the Component 3D tab shows it.
        let mut v = viewport(&project(Fabrication::default())?)?;
        if let Some(chip) = centre(&v, BodyKind::ModelBody) {
            aim(&mut v, chip, 7.0, 30.0, 35.0);
            save(&mut v, "generated-chip-closeup")?;
            aim(&mut v, chip, 5.0, 120.0, 20.0);
            save(&mut v, "generated-chip-side")?;
        }
        for preset in sparksmith_core::library::samples::Preset::ALL {
            let mut package = sparksmith_core::library::PackageDefinition {
                form: preset.form(),
                model: sparksmith_core::library::PackageModel::Generated {
                    family_rev: 1,
                    materials: Default::default(),
                },
                ..Default::default()
            };
            package.envelope_nm = (package.form.envelope().unwrap_or([1.0; 3])).map(nm);
            let model = sparksmith_solidcraft::project::preview(&package)?;
            let mut v = Viewport::detached();
            v.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
            v.fit_to(4.0 / 3.0);
            let name = preset.label().to_lowercase().replace([' ', '×'], "-");
            save(&mut v, &format!("generated-{name}"))?;
        }
    }
    let colours = [
        (MaskColour::Green, "green"),
        (MaskColour::Blue, "blue"),
        (MaskColour::Red, "red"),
        (MaskColour::Purple, "purple"),
        (MaskColour::Yellow, "yellow"),
        (MaskColour::Black, "black"),
        (MaskColour::White, "white"),
        (MaskColour::Clear, "clear"),
    ];
    for (mask_colour, name) in colours {
        if !only.is_empty() && !only.contains(name) {
            continue;
        }
        let mut v = viewport(&project(Fabrication {
            mask_colour,
            ..Fabrication::default()
        })?)?;
        save(&mut v, &format!("mask-{name}-gloss-enig-board"))?;
        if let Some(chip) = centre(&v, BodyKind::ChipResistor) {
            aim(&mut v, chip, 9.0, 30.0, 35.0);
            save(&mut v, &format!("mask-{name}-gloss-enig-chip-closeup"))?;
        }
    }
    if !only.is_empty() && !only.contains("detail") {
        return Ok(());
    }
    // The component 3D preview: each package as the Part editor shows it.
    for (k, package) in project(Fabrication::default())?
        .library
        .packages
        .iter()
        .enumerate()
    {
        let model = sparksmith_solidcraft::project::preview(&package.definition)?;
        let mut v = Viewport::detached();
        v.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
        v.fit_to(4.0 / 3.0);
        save(&mut v, &format!("component-preview-{k}"))?;
    }
    for (pad_finish, name) in [
        (PadFinish::Enig, "enig"),
        (PadFinish::Hasl, "hasl"),
        (PadFinish::LeadFreeHasl, "lead-free-hasl"),
        (PadFinish::Osp, "osp"),
        (PadFinish::ImmersionSilver, "silver"),
    ] {
        let mut v = viewport(&project(Fabrication {
            pad_finish,
            ..Fabrication::default()
        })?)?;
        if let Some(pad) = centre(&v, BodyKind::PadFinish) {
            aim(&mut v, pad, 6.0, 200.0, 50.0);
            save(&mut v, &format!("pads-{name}-closeup"))?;
        }
    }
    let mut v = viewport(&project(Fabrication::default())?)?;
    let b = v.bounds;
    aim(
        &mut v,
        Vec3::new(b.min.x + 4.0, b.min.y, b.center().z),
        6.0,
        0.0,
        12.0,
    );
    save(&mut v, "edge-weave-closeup")?;
    let mut v = viewport(&project(Fabrication {
        mask_colour: MaskColour::Clear,
        ..Fabrication::default()
    })?)?;
    aim(&mut v, b.center(), 0.5 * b.diagonal(), 40.0, 30.0);
    save(&mut v, "mask-clear-weave-midrange")?;
    let mut v = viewport(&project(Fabrication::default())?)?;
    v.camera.pitch = 8f64.to_radians();
    v.fit_to(4.0 / 3.0);
    save(&mut v, "grazing-8deg")?;
    v.camera.pitch = (-40f64).to_radians();
    v.fit_to(4.0 / 3.0);
    save(&mut v, "bottom-side")?;
    for (finish, name) in [
        (sparksmith_core::MaskFinish::Satin, "satin"),
        (sparksmith_core::MaskFinish::Matte, "matte"),
    ] {
        let mut v = viewport(&project(Fabrication {
            mask_finish: finish,
            ..Fabrication::default()
        })?)?;
        save(&mut v, &format!("mask-green-{name}-board"))?;
    }
    Ok(())
}

/// One generated package model of each family, as the Component 3D tab
/// shows it (`project::preview`).
fn generated_packages() -> Vec<(&'static str, sparksmith_core::library::PackageDefinition)> {
    use sparksmith_core::library::{
        ChipForm, ChipKind, GeneratedMaterials, GullWingForm, HeaderForm, LeadSides, NoLeadForm,
        PackageDefinition, PackageForm, PackageModel, PackageTerminal,
    };
    let package = |form: PackageForm, terminals: u32| PackageDefinition {
        next_local: terminals + 1,
        terminals: (1..=terminals)
            .map(|i| PackageTerminal {
                id: sparksmith_core::PackageTerminalId(i),
                designation: i.to_string(),
            })
            .collect(),
        model: PackageModel::Generated {
            family_rev: 1,
            materials: GeneratedMaterials::default(),
        },
        form,
        ..PackageDefinition::default()
    };
    let chip = |kind| ChipForm {
        kind,
        length_nm: nm(1.6),
        width_nm: nm(0.8),
        height_nm: nm(0.45),
        termination_nm: nm(0.3),
    };
    vec![
        (
            "chip-resistor-0603",
            package(PackageForm::Chip(Box::new(chip(ChipKind::Resistor))), 2),
        ),
        (
            "chip-capacitor-0603",
            package(
                PackageForm::Chip(Box::new(ChipForm {
                    height_nm: nm(0.8),
                    ..chip(ChipKind::Capacitor)
                })),
                2,
            ),
        ),
        (
            "soic-8",
            package(
                PackageForm::GullWing(Box::new(GullWingForm {
                    sides: LeadSides::Two,
                    leads_per_side: [4, 0],
                    pitch_nm: nm(1.27),
                    body_length_nm: nm(4.9),
                    body_width_nm: nm(3.9),
                    body_height_nm: nm(1.45),
                    standoff_nm: nm(0.1),
                    span_nm: [nm(6.0), nm(0.0)],
                    lead_width_nm: nm(0.41),
                    lead_thickness_nm: nm(0.2),
                    foot_length_nm: nm(0.6),
                })),
                8,
            ),
        ),
        (
            "tqfp-32",
            package(
                PackageForm::GullWing(Box::new(GullWingForm {
                    sides: LeadSides::Four,
                    leads_per_side: [8, 8],
                    pitch_nm: nm(0.8),
                    body_length_nm: nm(7.0),
                    body_width_nm: nm(7.0),
                    body_height_nm: nm(1.0),
                    standoff_nm: nm(0.1),
                    span_nm: [nm(9.0), nm(9.0)],
                    lead_width_nm: nm(0.37),
                    lead_thickness_nm: nm(0.15),
                    foot_length_nm: nm(0.6),
                })),
                32,
            ),
        ),
        (
            "qfn-16",
            package(
                PackageForm::NoLead(Box::new(NoLeadForm {
                    sides: LeadSides::Four,
                    terminals_per_side: [4, 4],
                    pitch_nm: nm(0.5),
                    body_length_nm: nm(3.0),
                    body_width_nm: nm(3.0),
                    body_height_nm: nm(0.85),
                    terminal_length_nm: nm(0.4),
                    terminal_width_nm: nm(0.25),
                    exposed_pad: Some(pt([1.7, 1.7])),
                })),
                17,
            ),
        ),
        (
            "header-2x5",
            package(
                PackageForm::Header(Box::new(HeaderForm {
                    rows: 2,
                    pins_per_row: 5,
                    pitch_nm: nm(2.54),
                    row_pitch_nm: nm(2.54),
                    body_height_nm: nm(2.5),
                    pin_side_nm: nm(0.64),
                    pin_above_nm: nm(6.0),
                    pin_below_nm: nm(3.0),
                })),
                10,
            ),
        ),
    ]
}

/// Close-ups of each generated family in the product look, from above and
/// from below, and the demo board with generated chip resistors.
#[test]
#[ignore = "writes model pictures"]
fn write_model_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let mut save =
        |viewport: &mut Viewport, name: &str| -> Result<(), Box<dyn std::error::Error>> {
            let (w, h) = (1024, 768);
            let rgba = gpu.draw(viewport, w, h, 2.0).ok_or("no picture")?;
            let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("{name}.png")), png)?;
            Ok(())
        };
    for (name, package) in generated_packages() {
        let model = sparksmith_solidcraft::project::preview(&package)?;
        let mut v = Viewport::detached();
        v.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
        v.fit_to(4.0 / 3.0);
        let b = v.bounds;
        aim(&mut v, b.center(), 1.7 * b.diagonal(), 35.0, 32.0);
        save(&mut v, &format!("model-{name}"))?;
        aim(&mut v, b.center(), 1.7 * b.diagonal(), 215.0, -30.0);
        save(&mut v, &format!("model-{name}-below"))?;
    }
    let mut project = project(Fabrication::default())?;
    let package = &mut project.library.packages[0].definition;
    package.form =
        sparksmith_core::library::PackageForm::Chip(Box::new(sparksmith_core::library::ChipForm {
            kind: sparksmith_core::library::ChipKind::Resistor,
            length_nm: nm(2.0),
            width_nm: nm(1.2),
            height_nm: nm(0.6),
            termination_nm: nm(0.4),
        }));
    package.model = sparksmith_core::library::PackageModel::Generated {
        family_rev: 1,
        materials: Default::default(),
    };
    let mut v = viewport(&project)?;
    if let Some(chip) = centre(&v, BodyKind::ModelBody) {
        aim(&mut v, chip, 9.0, 30.0, 35.0);
        save(&mut v, "model-board-chip-closeup")?;
    }
    Ok(())
}

/// The showcase board, [`sparksmith_core::Engine::showcase`]: a 4-layer
/// board of the sample packages routed on top and bottom through vias.
fn showcase_project() -> Result<Project, Box<dyn std::error::Error>> {
    Ok(sparksmith_core::Engine::showcase()?.project().clone())
}

/// Where on the board the first part matching `wanted` stands (its
/// placement and side), by its package form.
fn part_at(
    project: &Project,
    wanted: impl Fn(&sparksmith_core::library::PackageForm) -> bool,
) -> Option<(Vec3, Layer)> {
    project.placements.iter().find_map(|p| {
        let instance = project.instance(p.instance)?;
        let physical = sparksmith_core::instances::resolve(&project.library, instance)
            .ok()?
            .physical?;
        wanted(&physical.package.definition.form)
            .then(|| (Vec3::new(p.position[0], p.position[1], 0.0), p.layer))
    })
}

/// The showcase picture set for the founder's look review: the default
/// overview, close-ups of the SOIC, QFN, header and passives, tented and
/// open vias (when the board has them), the edge stackup and the bottom
/// side. Each picture's share of clipped pixels is printed.
#[test]
#[ignore = "writes showcase pictures"]
fn write_showcase_pictures() -> Result<(), Box<dyn std::error::Error>> {
    use sparksmith_core::library::{LeadSides, PackageForm};
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    // `SPARKSMITH_PICTURES_ONLY` (a name prefix) writes only those pictures,
    // for tuning.
    let only = std::env::var("SPARKSMITH_PICTURES_ONLY").unwrap_or_default();
    let mut save =
        |viewport: &mut Viewport, name: &str| -> Result<(), Box<dyn std::error::Error>> {
            if !name.starts_with(only.as_str()) {
                return Ok(());
            }
            let (w, h) = (1280, 960);
            let rgba = gpu.draw(viewport, w, h, 2.0).ok_or("no picture")?;
            let clipped = rgba
                .chunks_exact(4)
                .filter(|p| p[..3].iter().all(|c| *c >= 254))
                .count();
            eprintln!(
                "{name}: {:.3} % of pixels at white",
                100.0 * clipped as f64 / f64::from(w * h)
            );
            let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("{name}.png")), png)?;
            Ok(())
        };
    let project = showcase_project()?;
    let mut v = viewport(&project)?;
    // The board's faces: the mask's top and the substrate's underside.
    let face = |kind: BodyKind, upper: bool| {
        v.model.as_ref().and_then(|m| {
            m.geometry
                .iter()
                .filter(|g| g.kind == kind)
                .map(|g| if upper { g.max[2] } else { g.min[2] })
                .reduce(if upper { f64::max } else { f64::min })
        })
    };
    let top = face(BodyKind::MaskTop, true).unwrap_or(1.6);
    let underside = face(BodyKind::Substrate, false).unwrap_or(0.0);
    save(&mut v, "01-overview")?;
    let b = v.bounds;
    aim(&mut v, b.center(), 0.75 * b.diagonal(), 25.0, 40.0);
    save(&mut v, "02-overview-turned")?;
    // Aim at a part; from far off the target rises off the board so a tall
    // part (the header's pins) stays in frame.
    let close = |v: &mut Viewport, at: Option<(Vec3, Layer)>, distance: f64, yaw: f64| {
        let (p, layer) = at?;
        let below = layer == Layer::Bottom;
        let lift = if distance > 20.0 {
            0.12 * distance
        } else {
            0.0
        };
        let z = if below { underside - lift } else { top + lift };
        aim(
            v,
            Vec3::new(p.x, p.y, z),
            distance,
            yaw,
            if below { -35.0 } else { 35.0 },
        );
        Some(())
    };
    type Wanted = Box<dyn Fn(&PackageForm) -> bool>;
    let shots: [(&str, Wanted, f64, f64); 5] = [
        (
            "03-soic",
            Box::new(|f| matches!(f, PackageForm::GullWing(g) if g.sides == LeadSides::Two)),
            14.0,
            30.0,
        ),
        (
            "04-qfn",
            Box::new(|f| matches!(f, PackageForm::NoLead(n) if n.sides == LeadSides::Four)),
            9.0,
            35.0,
        ),
        (
            "05-header",
            Box::new(|f| matches!(f, PackageForm::Header(_))),
            34.0,
            30.0,
        ),
        (
            "06-passive-0603",
            Box::new(|f| matches!(f, PackageForm::Chip(c) if c.length() < 1.8)),
            6.0,
            30.0,
        ),
        (
            "07-passive-0805",
            Box::new(|f| matches!(f, PackageForm::Chip(c) if c.length() >= 1.8)),
            7.0,
            210.0,
        ),
    ];
    for (name, wanted, distance, yaw) in shots {
        if close(&mut v, part_at(&project, wanted), distance, yaw).is_some() {
            save(&mut v, name)?;
        } else {
            eprintln!("{name}: the board has no such part");
        }
    }
    // Vias ranked by their clearance to the nearest component body, the
    // clearest first, so nothing hides them (the showcase's vias are
    // fan-outs about 1 mm from their parts).
    let model = v.model.as_ref().ok_or("no model")?;
    let clearance = |via: &sparksmith_core::Via| {
        model
            .geometry
            .iter()
            .filter(|g| ViewLayer::of_body(g) == ViewLayer::Bodies)
            .map(|g| {
                let dx = (g.min[0] - via.position[0]).max(via.position[0] - g.max[0]);
                let dy = (g.min[1] - via.position[1]).max(via.position[1] - g.max[1]);
                dx.max(dy)
            })
            .fold(f64::INFINITY, f64::min)
    };
    let ranked = |tented: bool| {
        let mut vias: Vec<(f64, &sparksmith_core::Via)> = project
            .vias
            .iter()
            .filter(|v| v.tented == tented)
            .map(|v| (clearance(v), v))
            .filter(|(c, _)| *c > 0.75)
            .collect();
        vias.sort_by(|a, b| b.0.total_cmp(&a.0));
        vias.into_iter().map(|(_, v)| v).collect::<Vec<_>>()
    };
    let open = ranked(false);
    let tented = ranked(true);
    // A via's pick: that of the plated barrel standing at its position.
    let pick_of = |v: &Viewport, via: &sparksmith_core::Via| {
        v.model.as_ref().and_then(|m| {
            m.geometry
                .iter()
                .find(|g| {
                    g.kind == BodyKind::ViaCopper
                        && (g.min[0]..=g.max[0]).contains(&via.position[0])
                        && (g.min[1]..=g.max[1]).contains(&via.position[1])
                })
                .and_then(|g| g.item)
        })
    };
    let (w, h) = (1280.0, 960.0);
    for (via, name) in [
        (tented.first(), "08-via-tented"),
        (open.first(), "09-via-open"),
    ] {
        let Some(via) = via else {
            return Err(format!("{name}: the showcase has no unobstructed via").into());
        };
        let at = Vec3::new(via.position[0], via.position[1], top);
        aim(&mut v, at, 3.0, 30.0, 55.0);
        // The frame shows the via: it is what the centre pixel picks.
        let centre = v.pick(w / 2.0, h / 2.0, w, h);
        if centre.is_none() || centre != pick_of(&v, via) {
            return Err(format!("{name}: the frame's centre is not the via").into());
        }
        save(&mut v, name)?;
    }
    // A tented and an open via side by side: the closest such pair.
    let pair = tented
        .iter()
        .flat_map(|t| open.iter().map(move |o| (*t, *o)))
        .min_by(|a, b| {
            let d = |(t, o): (&sparksmith_core::Via, &sparksmith_core::Via)| {
                (t.position[0] - o.position[0]).hypot(t.position[1] - o.position[1])
            };
            d(*a).total_cmp(&d(*b))
        })
        .ok_or("no tented and open vias")?;
    let (t, o) = pair;
    let mid = Vec3::new(
        (t.position[0] + o.position[0]) / 2.0,
        (t.position[1] + o.position[1]) / 2.0,
        top,
    );
    let gap = (t.position[0] - o.position[0]).hypot(t.position[1] - o.position[1]);
    aim(&mut v, mid, 2.2 * gap + 3.0, 20.0, 50.0);
    save(&mut v, "08b-vias-tented-and-open")?;
    // The open via's barrel through the 4-layer stackup, in X-ray: the
    // plated barrel with its ring on every copper layer and the inner
    // tracks it joins.
    let stack = v.model.as_ref().ok_or("no model")?.stack.clone();
    show_layers(&mut v, crate::layer_view::LayerView::x_ray(&stack))?;
    let barrel = Vec3::new(o.position[0], o.position[1], (underside + top) / 2.0);
    aim(&mut v, barrel, 6.5, 30.0, 12.0);
    save(&mut v, "09b-via-barrel-x-ray")?;
    show_layers(&mut v, crate::layer_view::LayerView::default())?;
    // The board edge face on, low, so the stackup bands fill the frame.
    let mid = (underside + top) / 2.0;
    aim(
        &mut v,
        Vec3::new(b.min.x + 6.0, b.min.y, mid),
        3.5,
        20.0,
        5.0,
    );
    save(&mut v, "10-edge-stackup")?;
    aim(&mut v, Vec3::new(b.min.x, b.min.y, mid), 6.0, 45.0, 20.0);
    save(&mut v, "11-edge-corner")?;
    aim(&mut v, b.center(), 0.75 * b.diagonal(), 200.0, -35.0);
    save(&mut v, "12-bottom-side")?;
    Ok(())
}

/// Show only `view`'s layers: a fresh pack of the shown bodies, what the
/// background repack publishes in the window.
fn show_layers(
    v: &mut Viewport,
    view: crate::layer_view::LayerView,
) -> Result<(), Box<dyn std::error::Error>> {
    let model = v.model.as_ref().ok_or("no model")?;
    let (stream, _) =
        crate::model_scene::pack(&model.geometry, &v.decals.geometry, &view, &model.stack)?;
    v.set_layers(view);
    v.repack = None;
    v.publish_stream(stream);
    Ok(())
}

/// Pictures of the 3D layer toggles.
mod layers;

/// The time from opening the PCB 3D view, its model prepared in the
/// background, to its first frame: the prepared scene published to the
/// viewport, framed, uploaded and drawn on the GPU (and read back, which
/// the window does not do). Best of a few openings, each with a new
/// viewport. `cargo test --profile ci -p sparksmith opening_3d_timings --
/// --ignored --nocapture`; `SPARKSMITH_BENCH_VIAS=0,40` sets the boards (the
/// showcase on 4 layers with that many vias; 0, the showcase).
#[test]
#[ignore = "benchmark"]
fn opening_3d_timings() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let boards: Vec<usize> = std::env::var("SPARKSMITH_BENCH_VIAS")
        .ok()
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 40, 1000, 10_000]);
    for count in boards {
        let mut project = showcase_project()?;
        if count > 0 {
            project.stackup = sparksmith_core::Stackup::preset(4)?;
            let net = project.nets[0].id;
            let [width, depth] = project.board_size;
            let rows = count.div_ceil(64);
            project.board_size = [
                width.max(64.0 * 1.27 + 6.0),
                depth + rows as f64 * 1.27 + 4.0,
            ];
            for k in 0..count {
                let id = project.allocate()?;
                project.vias.push(sparksmith_core::Via {
                    id,
                    net,
                    position: [
                        3.0 + (k % 64) as f64 * 1.27,
                        depth + 2.0 + (k / 64) as f64 * 1.27,
                    ],
                    drill: 0.3,
                    diameter: 0.6,
                    span: sparksmith_core::ViaSpan::Through,
                    tented: k % 2 == 0,
                    locked: false,
                });
            }
        }
        // Prepared in the background while the board is edited in 2D.
        let start = std::time::Instant::now();
        let model = sparksmith_solidcraft::project::build(&project)?;
        let built = start.elapsed();
        let name = if count == 0 {
            "showcase".to_owned()
        } else {
            format!("showcase, 4 layers, {count} vias")
        };
        let mut best = f64::MAX;
        let mut prepared = None;
        for _ in 0..5 {
            let scene = match crate::model_scene::build(model.geometry.clone(), &model.stack) {
                Ok(scene) => scene,
                Err(error) => {
                    prepared = Some(error);
                    break;
                }
            };
            let start = std::time::Instant::now();
            let mut viewport = Viewport::detached();
            viewport.set_model(scene);
            viewport.fit_to(16.0 / 10.0);
            gpu.draw(&mut viewport, 1600, 1000, 1.0).ok_or("no frame")?;
            best = best.min(start.elapsed().as_secs_f64() * 1e3);
        }
        match prepared {
            Some(error) => println!(
                "{name}: prepared in {:.0} ms; the view cannot show it: {error}",
                built.as_secs_f64() * 1e3
            ),
            None => println!(
                "{name}: prepared in {:.0} ms; first frame {best:.0} ms after opening",
                built.as_secs_f64() * 1e3
            ),
        }
    }
    Ok(())
}

/// Vias drawn instanced: the look, uploads and timings.
mod instanced;

/// A generated package with `terminals` numbered terminals, for pictures.
fn picture_package(
    form: sparksmith_core::library::PackageForm,
    terminals: u32,
) -> sparksmith_core::library::PackageDefinition {
    use sparksmith_core::library::{
        GeneratedMaterials, PackageDefinition, PackageModel, PackageTerminal,
    };
    PackageDefinition {
        next_local: terminals + 1,
        terminals: (1..=terminals)
            .map(|i| PackageTerminal {
                id: sparksmith_core::PackageTerminalId(i),
                designation: i.to_string(),
            })
            .collect(),
        model: PackageModel::Generated {
            family_rev: 1,
            materials: GeneratedMaterials::default(),
        },
        form,
        ..PackageDefinition::default()
    }
}

/// A package preview with `spec` laid out for `inputs` and drawn as its
/// decals, in a detached viewport.
fn marked_preview(
    package: &sparksmith_core::library::PackageDefinition,
    library: &sparksmith_core::library::Library,
    spec: &sparksmith_core::library::marking::MarkingSpec,
    inputs: &sparksmith_core::library::marking::MarkingInputs,
) -> Result<Viewport, Box<dyn std::error::Error>> {
    use sparksmith_core::library::marking;
    let model = sparksmith_solidcraft::project::preview(package)?;
    let region = |s: &marking::MarkingSpec| {
        marking::surface_region(library, Some(package), s.area, &s.placement)
    };
    let layout = marking::layout_with(spec, inputs, &region)?;
    for f in &layout.findings {
        eprintln!("marking finding: {}", f.message);
    }
    let artwork = sparksmith_solidcraft::marking::board::project_artwork_of(library);
    let mut cache = sparksmith_solidcraft::marking::DecalCache::default();
    let (decals, problems) = sparksmith_solidcraft::marking::board::preview_decals(
        &layout,
        &model.geometry,
        Some(package),
        &artwork,
        &mut cache,
    );
    if !problems.is_empty() {
        return Err(problems.join("; ").into());
    }
    let mut geometry = model.geometry;
    geometry.extend(decals);
    let mut v = Viewport::detached();
    v.set_model(crate::model_scene::build(geometry, &model.stack)?);
    v.fit_to(4.0 / 3.0);
    Ok(v)
}

/// D7 evidence: the ESP32-S31-WROOM-3 example's can and board markings,
/// an aluminium electrolytic's wrapped sleeve with its stripe and vent
/// score, and an axial part's wrapped colour bands, in the product look.
/// `SPARKSMITH_PICTURES=dir cargo test -p sparksmith write_marking_pictures
/// -- --ignored`.
#[test]
#[ignore = "writes marking pictures"]
fn write_marking_pictures() -> Result<(), Box<dyn std::error::Error>> {
    use sparksmith_core::FieldRef;
    use sparksmith_core::ids::{DefinitionId, Revision};
    use sparksmith_core::library::Definition;
    use sparksmith_core::library::marking::{
        Finish, MarkingInputs, MarkingItem, MarkingSpec, Placement, SurfaceRef, Wrap,
    };
    use sparksmith_core::library::package_form::PackageForm;
    use sparksmith_core::library::package_form::tht::{ThtForm, ThtKind};
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let mut save =
        |viewport: &mut Viewport, name: &str| -> Result<(), Box<dyn std::error::Error>> {
            let (w, h) = (1280, 960);
            let rgba = gpu.draw(viewport, w, h, 2.0).ok_or("no picture")?;
            let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("{name}.png")), png)?;
            Ok(())
        };
    // The ESP32-S31-WROOM-3 example as a Component 3D preview.
    let file = sparksmith_io::persistence::read_project(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-wroom-3/esp32-s31-wroom-3.sparksmith"
    )))?;
    let find = |pick: fn(&Definition) -> bool| {
        file.drafts
            .items
            .iter()
            .find(|d| pick(&d.definition))
            .map(|d| d.definition.clone())
    };
    let Some(Definition::Package(package)) = find(|d| matches!(d, Definition::Package(_))) else {
        return Err("no package draft".into());
    };
    let Some(Definition::Component(component)) = find(|d| matches!(d, Definition::Component(_)))
    else {
        return Err("no component draft".into());
    };
    let spec = component
        .marking
        .as_deref()
        .ok_or("the example is not marked")?;
    let inputs = MarkingInputs {
        designator: "U1".into(),
        ..Default::default()
    };
    let mut v = marked_preview(&package, &file.project.library, spec, &inputs)?;
    let b = v.bounds;
    aim(&mut v, b.center(), 1.6 * b.diagonal(), 30.0, 38.0);
    save(&mut v, "marking-wroom3")?;
    aim(&mut v, b.center(), 1.25 * b.diagonal(), 0.0, 89.0);
    save(&mut v, "marking-wroom3-top")?;
    aim(&mut v, Vec3::new(5.0, -8.0, 3.5), 14.0, 20.0, 50.0);
    save(&mut v, "marking-wroom3-qr-closeup")?;
    aim(&mut v, Vec3::new(0.0, 12.0, 1.0), 16.0, 200.0, 40.0);
    save(&mut v, "marking-wroom3-antenna-silkscreen")?;
    // From below with the footprint's lands, as the Component 3D tab draws
    // them: the module's pads stand a solder bond line above the lands,
    // never on their plane.
    if let Some(Definition::Footprint(footprint)) = find(|d| matches!(d, Definition::Footprint(_)))
    {
        v.set_overlay(Some(crate::part_editor::land_overlay(&footprint, &package)));
        aim(&mut v, Vec3::new(0.0, -3.0, 0.0), 20.0, 20.0, -40.0);
        save(&mut v, "marking-wroom3-below-lands")?;
        v.set_overlay(None);
    }

    // An aluminium electrolytic: µF/V and stripe wrapped on its sleeve,
    // a vent cross engraved in its can top.
    let (cap, volt) = (DefinitionId(1), DefinitionId(2));
    let field = |id| FieldRef::new(id, Revision::FIRST);
    let radial = picture_package(
        PackageForm::Tht(Box::new(ThtForm::new(
            ThtKind::Radial {
                body_diameter_nm: nm(6.3),
                body_height_nm: nm(11.0),
            },
            nm(2.5),
            nm(0.5),
            nm(3.0),
        ))),
        2,
    );
    let on = |name: &str| Placement {
        on: SurfaceRef::Anchor { name: name.into() },
        ..Placement::default()
    };
    let vent_svg = "<svg xmlns='http://www.w3.org/2000/svg' width='10mm' height='10mm' viewBox='0 0 10 10'><path d='M1 1 L9 9 M9 1 L1 9' stroke='black' stroke-width='0.7' fill='none'/></svg>";
    let mut library = file.project.library.clone();
    let vent = sparksmith_core::library::bundle::AssetBytes::new(vent_svg.as_bytes().to_vec());
    let vent_hash = vent.hash();
    library.assets.push(sparksmith_core::library::PackedAsset {
        hash: vent_hash,
        media_type: sparksmith_core::library::marking::ARTWORK_MEDIA_TYPE.into(),
        data: vent,
    });
    let electrolytic = MarkingSpec {
        items: vec![
            MarkingItem::Electrolytic {
                capacitance: field(cap),
                voltage: Some(field(volt)),
                stripe: true,
            },
            MarkingItem::Text {
                template: sparksmith_core::library::marking::Template("LOW ESR".into()),
                height_nm: sparksmith_core::units::Nm::from_mm_f64_rounded(0.9).ok(),
                align: Default::default(),
            },
        ],
        finish: Finish::Ink {
            colour: [228, 230, 232],
        },
        placement: on("sleeve"),
        decals: vec![MarkingSpec {
            items: vec![MarkingItem::Artwork {
                svg: vent_hash,
                size_nm: [sparksmith_core::units::Nm::from_mm_f64_rounded(2.6).unwrap_or_default();
                    2],
                anchor: Default::default(),
                licence: Some("CC0-1.0".into()),
            }],
            finish: Finish::Engraved,
            placement: on("top"),
            ..MarkingSpec::default()
        }],
        ..MarkingSpec::default()
    };
    let mut inputs = MarkingInputs {
        designator: "C1".into(),
        ..Default::default()
    };
    inputs.fields.insert(cap, ("220u".into(), Some(220e-6)));
    inputs.fields.insert(volt, ("25".into(), Some(25.0)));
    let mut v = marked_preview(&radial, &library, &electrolytic, &inputs)?;
    let b = v.bounds;
    aim(&mut v, b.center(), 2.4 * b.diagonal(), 90.0, 20.0);
    save(&mut v, "marking-electrolytic-text-side")?;
    aim(&mut v, b.center(), 2.4 * b.diagonal(), 270.0, 20.0);
    save(&mut v, "marking-electrolytic-stripe-side")?;
    aim(&mut v, b.center(), 2.2 * b.diagonal(), 300.0, 60.0);
    save(&mut v, "marking-electrolytic-top")?;

    // An axial part: its value's colour bands wrapped round the body.
    let axial = picture_package(
        PackageForm::Tht(Box::new(ThtForm::new(
            ThtKind::Axial {
                body_diameter_nm: nm(2.5),
                body_length_nm: nm(6.5),
            },
            nm(10.16),
            nm(0.6),
            nm(3.0),
        ))),
        2,
    );
    let value = DefinitionId(3);
    let bands = MarkingSpec {
        items: vec![MarkingItem::Bands {
            value: field(value),
            tolerance: None,
        }],
        finish: Finish::LaserEtch,
        placement: Placement {
            wrap: Some(Wrap::default()),
            ..on("body")
        },
        ..MarkingSpec::default()
    };
    let mut inputs = MarkingInputs {
        designator: "R1".into(),
        ..Default::default()
    };
    inputs.fields.insert(value, ("4k7".into(), Some(4700.0)));
    let mut v = marked_preview(&axial, &library, &bands, &inputs)?;
    let b = v.bounds;
    aim(&mut v, b.center(), 1.8 * b.diagonal(), 25.0, 30.0);
    save(&mut v, "marking-axial-bands")?;
    Ok(())
}

/// The WROOM-3 module placed on a board (`scene::fixtures::wroom_board`),
/// as the PCB 3D view first shows it: over the demo board the app showed
/// before the file was opened, from above, from below without the
/// laminate and close to its pads. Run with `SPARKSMITH_PICTURES=<dir>
/// cargo test -p sparksmith write_wroom_board_pictures -- --ignored`.
#[test]
#[ignore = "writes WROOM-3 board pictures"]
fn write_wroom_board_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let engine = crate::scene::fixtures::wroom_board()?;
    let project = engine.project().clone();
    let mut v = viewport(&crate::model_scene::tests::demo_project()?)?;
    gpu.draw(&mut v, 320, 240, 1.0).ok_or("no picture")?;
    let model = sparksmith_solidcraft::project::build(&project)?;
    let mut geometry = model.geometry.clone();
    geometry.extend(
        sparksmith_solidcraft::marking::board::BoardMarkings::default().decals(
            &project,
            &model.geometry,
            &model.sources,
        ),
    );
    v.set_model(crate::model_scene::build(geometry, &model.stack)?);
    v.fit_to(4.0 / 3.0);
    let mut save =
        |viewport: &mut Viewport, name: &str| -> Result<(), Box<dyn std::error::Error>> {
            let (w, h) = (1280, 960);
            let rgba = gpu.draw(viewport, w, h, 2.0).ok_or("no picture")?;
            let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("{name}.png")), png)?;
            Ok(())
        };
    let b = v.bounds;
    let c = Vec3::new(
        project.board_size[0] / 2.0,
        project.board_size[1] / 2.0,
        1.0,
    );
    aim(&mut v, b.center(), 1.3 * b.diagonal(), 30.0, 40.0);
    save(&mut v, "wroom-board")?;
    aim(&mut v, c, 30.0, 20.0, -35.0);
    save(&mut v, "wroom-board-below")?;
    v.set_laminate(false);
    v.set_layer(ViewLayer::Mask(Layer::Bottom), false);
    v.set_layer(ViewLayer::Mask(Layer::Top), false);
    for _ in 0..500 {
        if !v.receive_repack() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    aim(&mut v, c, 30.0, 20.0, -35.0);
    save(&mut v, "wroom-board-below-no-laminate")?;
    aim(&mut v, c - Vec3::new(0.0, 3.0, 0.0), 9.0, 10.0, -50.0);
    save(&mut v, "wroom-board-pads-closeup")?;
    Ok(())
}

/// The showcase board with built-in mechanical parts placed directly
/// (docs/mechanical-parts.md): SMD nuts in two corners, a spacer and a
/// finned heatsink over U2, each through the same `layout.place_part`
/// the Place 3D part command applies.
pub(crate) fn mechanical_board() -> Result<sparksmith_core::Engine, Box<dyn std::error::Error>> {
    use sparksmith_automation::session::mechanical_parts::{
        PartFileSource, PlacePartFileParams, place_part_operation,
    };
    use sparksmith_core::library::package_model::PartValue;
    let mut engine = sparksmith_core::Engine::showcase()?;
    type Placed<'a> = (&'a str, [f64; 2], Vec<(&'a str, PartValue)>);
    let parts: [Placed<'_>; 4] = [
        ("smd-nut", [3.5, 3.5], vec![]),
        (
            "smd-nut",
            [40.5, 24.5],
            vec![("height", PartValue::Mm(4.0))],
        ),
        ("round-spacer", [40.5, 3.5], vec![]),
        (
            "finned-heatsink",
            [36.0, 15.0],
            vec![
                ("length", PartValue::Mm(8.0)),
                ("width", PartValue::Mm(8.0)),
                ("height", PartValue::Mm(6.0)),
                ("fin_count", PartValue::Count(5)),
                ("fin_thickness", PartValue::Mm(0.8)),
                ("base_height", PartValue::Mm(1.2)),
            ],
        ),
    ];
    for (name, at, values) in parts {
        let mut params =
            PlacePartFileParams::new(PartFileSource::Builtin(name.into()), at, engine.revision());
        params.values = values.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        let (operation, _) = place_part_operation(&engine, &params)?;
        engine.apply(engine.revision(), operation)?;
    }
    Ok(engine)
}

/// The mechanical-parts board in 3D. Run with `SPARKSMITH_PICTURES=<dir>
/// cargo test -p sparksmith write_mechanical_board_pictures -- --ignored`.
#[test]
#[ignore = "writes mechanical-part board pictures"]
fn write_mechanical_board_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let engine = mechanical_board()?;
    let project = engine.project().clone();
    let mut v = viewport(&project)?;
    let model = sparksmith_solidcraft::project::build(&project)?;
    v.set_model(crate::model_scene::build(
        model.geometry.clone(),
        &model.stack,
    )?);
    v.fit_to(4.0 / 3.0);
    let mut save =
        |viewport: &mut Viewport, name: &str| -> Result<(), Box<dyn std::error::Error>> {
            let (w, h) = (1280, 960);
            let rgba = gpu.draw(viewport, w, h, 2.0).ok_or("no picture")?;
            let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("{name}.png")), png)?;
            Ok(())
        };
    let b = v.bounds;
    aim(&mut v, b.center(), 1.2 * b.diagonal(), 30.0, 35.0);
    save(&mut v, "mechanical-board")?;
    aim(&mut v, Vec3::new(36.0, 15.0, 3.0), 26.0, 35.0, 30.0);
    save(&mut v, "mechanical-board-heatsink")?;
    aim(&mut v, Vec3::new(3.5, 3.5, 1.5), 12.0, 40.0, 35.0);
    save(&mut v, "mechanical-board-nut")?;
    Ok(())
}
