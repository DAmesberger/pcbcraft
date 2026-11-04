//! Vias drawn instanced on the GPU: their shared meshes go through the same
//! PBR material path as the bodies copied into the vertex stream, a layer
//! toggle uploads no via triangles again, and (ignored, a benchmark) how
//! long boards of thousands of vias take to show and to draw.
use super::*;
use crate::layer_view::ViewLayer;
use pcbcraft_solidcraft::scene::Geometry;

fn gpu() -> Result<Option<Gpu>, Box<dyn std::error::Error>> {
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

/// The via strip's middle.
fn vias_centre(geometry: &[Geometry]) -> Result<Vec3, Box<dyn std::error::Error>> {
    let mut b = Aabb3::EMPTY;
    for g in geometry.iter().filter(|g| g.kind == BodyKind::ViaCopper) {
        b.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
        b.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
    }
    if b.is_empty() {
        return Err("no vias".into());
    }
    Ok(b.center())
}

fn shown(model: ModelScene) -> Viewport {
    let mut viewport = Viewport::detached();
    viewport.set_model(model);
    viewport.fit_to(4.0 / 3.0);
    viewport
}

/// Pixels whose colour differs by more than `tolerance` in a channel.
fn differing(a: &[u8], b: &[u8], tolerance: u8) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(p, q)| {
            p.iter()
                .zip(q.iter())
                .any(|(x, y)| x.abs_diff(*y) > tolerance)
        })
        .count()
}

/// The vias drawn as placements of shared meshes look like the same bodies
/// copied into the PBR stream: the copper, finish, tents and bores keep
/// their materials (clear coat, pattern, ambient occlusion) through the
/// instanced path, and the vias cover a good share of the picture.
#[test]
fn instanced_vias_draw_as_their_copied_meshes() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = gpu()? else {
        return Ok(());
    };
    let model = pcbcraft_solidcraft::project::build(&crate::model_scene::tests::via_project(64)?)?;
    let target = vias_centre(&model.geometry)?;
    let copied: Vec<Geometry> = model
        .geometry
        .iter()
        .map(|g| Geometry {
            mesh: g.world_mesh(),
            place: None,
            ..g.clone()
        })
        .collect();
    let copied = crate::model_scene::build(copied)?;
    assert!(
        copied
            .instanced
            .iter()
            .all(|i| i.bodies.iter().all(|b| copied.geometry[*b].place.is_none())),
        "only large bodies are instanced"
    );
    let instanced = crate::model_scene::build(model.geometry)?;
    assert!(!instanced.instanced.is_empty());
    let (w, h) = (400, 300);
    let mut pictures = vec![];
    for scene in [copied, instanced] {
        let mut v = shown(scene);
        aim(&mut v, target, 4.0, 30.0, 50.0);
        pictures.push(gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?);
    }
    // Without the vias, to see that they cover the frame.
    let mut bare = shown(crate::model_scene::build(
        pcbcraft_solidcraft::project::build(&crate::model_scene::tests::via_project(64)?)?
            .geometry
            .into_iter()
            .filter(|g| g.place.is_none())
            .collect(),
    )?);
    aim(&mut bare, target, 4.0, 30.0, 50.0);
    let without = gpu.draw(&mut bare, w, h, 1.0).ok_or("no picture")?;
    let pixels = (w * h) as usize;
    let covered = differing(&pictures[0], &without, 8);
    let off = differing(&pictures[0], &pictures[1], 8);
    eprintln!("vias cover {covered} of {pixels} pixels; instanced differs at {off}");
    if let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        for (name, rgba) in [
            ("copied", &pictures[0]),
            ("instanced", &pictures[1]),
            ("bare", &without),
        ] {
            let png = pcbcraft_solidcraft::render::png(w as usize, h as usize, rgba.clone())
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("vias-{name}.png")), png)?;
        }
        let diff: Vec<u8> = pictures[0]
            .chunks_exact(4)
            .zip(pictures[1].chunks_exact(4))
            .flat_map(|(p, q)| {
                let d = p
                    .iter()
                    .zip(q)
                    .take(3)
                    .map(|(x, y)| x.abs_diff(*y))
                    .max()
                    .unwrap_or(0);
                [
                    d.saturating_mul(8),
                    d.saturating_mul(8),
                    d.saturating_mul(8),
                    255,
                ]
            })
            .collect();
        let png = pcbcraft_solidcraft::render::png(w as usize, h as usize, diff).ok_or("png")?;
        std::fs::write(dir.join("vias-diff.png"), png)?;
    }
    assert!(covered * 10 > pixels, "the vias cover the view: {covered}");
    assert!(off * 200 < covered, "{off} pixels differ of {covered}");
    Ok(())
}

/// Seen from above, a picked open via keeps its drill dark under the
/// selection (the bore shadow is not drawn over), its copper turns the
/// strong selection tint, and the other vias of its net turn the lighter
/// net tint, so the picked via is told from the rest.
#[test]
fn a_picked_via_keeps_its_drill_open_and_stands_out() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = gpu()? else {
        return Ok(());
    };
    let model = pcbcraft_solidcraft::project::build(&crate::model_scene::tests::via_project(64)?)?;
    let scene = crate::model_scene::build(model.geometry)?;
    // An open (untented) via: it has a bore shadow and no dome.
    let picked = scene
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::ViaBore && g.place.is_some())
        .filter_map(|g| g.item)
        .find(|item| {
            !scene
                .geometry
                .iter()
                .any(|g| g.item == Some(*item) && g.kind == BodyKind::MaskCap)
        })
        .ok_or("no open via")?;
    let net: Vec<u64> = scene
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::ViaCopper)
        .filter_map(|g| g.item)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut b = Aabb3::EMPTY;
    for g in scene.geometry.iter().filter(|g| g.item == Some(picked)) {
        b.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
        b.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
    }
    let target = Vec3::new(b.center().x, b.center().y, b.max.z);
    let mut v = shown(scene);
    aim(&mut v, target, 3.0, 0.0, 90.0);
    let (w, h) = (300, 300);
    let plain = gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?;
    v.highlight_selected(net.clone(), vec![picked]);
    let picked_shown = gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?;
    v.highlight_selected(net, Vec::new());
    let net_shown = gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?;
    if let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        for (name, rgba) in [
            ("plain", &plain),
            ("picked", &picked_shown),
            ("net", &net_shown),
        ] {
            let png = pcbcraft_solidcraft::render::png(w as usize, h as usize, rgba.clone())
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("via-select-{name}.png")), png)?;
        }
    }
    let at = |rgba: &[u8], x: u32, y: u32| -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        rgba.get(i..i + 4)
            .and_then(|p| p.try_into().ok())
            .unwrap_or([0; 4])
    };
    // The drill at the centre stays as it was.
    let (cx, cy) = (w / 2, h / 2);
    let (before, after) = (at(&plain, cx, cy), at(&picked_shown, cx, cy));
    assert!(
        before
            .iter()
            .zip(after)
            .take(3)
            .all(|(a, b)| a.abs_diff(b) <= 12),
        "{before:?} {after:?}"
    );
    // The picked via differs between its own tint and the net's.
    let changed = differing(&picked_shown, &net_shown, 8);
    assert!(changed > 50, "{changed}");
    Ok(())
}

/// A layer toggle publishes the same shared via triangles again: the
/// renderer keeps their buffers and uploads only the stream and the
/// placements.
#[test]
fn a_layer_toggle_uploads_no_instanced_via_mesh() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = gpu()? else {
        return Ok(());
    };
    let model = pcbcraft_solidcraft::project::build(&crate::model_scene::tests::via_project(64)?)?;
    let mut v = shown(crate::model_scene::build(model.geometry)?);
    gpu.draw(&mut v, 200, 150, 1.0).ok_or("no picture")?;
    let uploads = |gpu: &Gpu| gpu.resources.get::<Renderer>().map(|r| r.uploads());
    let first = uploads(&gpu).ok_or("no renderer")?;
    let stream = |v: &Viewport| {
        v.slot
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|s| s.tris.len() as u64))
            .unwrap_or(0)
    };
    let placements = |v: &Viewport| {
        v.slot
            .lock()
            .ok()
            .and_then(|s| {
                // Each level of detail gets its own buffer of the placements.
                s.as_ref().map(|s| {
                    s.instanced
                        .iter()
                        .map(|m| (m.instances.len() * (1 + m.levels.len())) as u64)
                        .sum()
                })
            })
            .unwrap_or(0)
    };
    assert!(first.geometry > stream(&v), "the via meshes went up once");
    assert!(first.placements > 0);
    v.set_layer(ViewLayer::Substrate, false);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while v.receive_repack() {
        if std::time::Instant::now() > deadline {
            return Err("the repack did not finish".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let (shown, placed) = (stream(&v), placements(&v));
    gpu.draw(&mut v, 200, 150, 1.0).ok_or("no picture")?;
    let again = uploads(&gpu).ok_or("no renderer")?;
    assert_eq!(again.geometry, shown, "only the stream is uploaded again");
    assert_eq!(again.placements, placed);
    assert!(placed > 0);
    Ok(())
}

/// The showcase on 4 layers with `count` through vias on a 1.27 mm grid
/// below its parts (an original benchmark board).
fn via_showcase(count: usize) -> Result<Project, Box<dyn std::error::Error>> {
    let mut project = showcase_project()?;
    let net = project.nets.first().map(|n| n.id).ok_or("no net")?;
    let columns = 100usize;
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

/// Building and showing boards of 1,000, 4,096 and 10,000 vias: the native
/// build, the GPU-ready scene, the first frame of the 3D view once a model
/// is built (scene upload, culling and the frame read back; the best of
/// three) and a frame while orbiting. Run with
/// `cargo test --profile ci -p pcbcraft via_display_timings -- --ignored --nocapture`;
/// `PCBCRAFT_VIA_COUNTS=1000,4096` picks the boards.
#[test]
#[ignore = "benchmark"]
fn via_display_timings() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let counts: Vec<usize> = std::env::var("PCBCRAFT_VIA_COUNTS")
        .ok()
        .map(|s| s.split(',').filter_map(|c| c.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1000, 4096, 10_000]);
    let (w, h) = (1280, 800);
    eprintln!(
        "| Vias | Native build | Scene | First frame | Orbit frame | Stream | Instanced meshes | Placements | Triangles drawn |"
    );
    eprintln!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for count in counts {
        let project = via_showcase(count)?;
        let started = std::time::Instant::now();
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let native = started.elapsed();
        let started = std::time::Instant::now();
        let scene = crate::model_scene::build(model.geometry.clone())?;
        let built = started.elapsed();
        let stream = scene.stream.tris.len()
            + scene
                .stream
                .graphs
                .draws()
                .iter()
                .map(|d| d.vertices.len())
                .sum::<usize>();
        let meshes: usize = scene
            .instanced
            .iter()
            .map(|i| {
                std::iter::once(&i.mesh.tris)
                    .chain(i.mesh.levels.iter().map(|(t, _)| t))
                    .map(|t| t.vertices.len() + t.indices.len() * 4)
                    .sum::<usize>()
            })
            .sum();
        let placements: usize = scene.instanced.iter().map(|i| i.mesh.instances.len()).sum();
        drop(scene);
        // Opening the view on a finished build: the scene is handed over
        // and the first frame drawn.
        let mut first = std::time::Duration::MAX;
        for _ in 0..3 {
            let scene = crate::model_scene::build(model.geometry.clone())?;
            let started = std::time::Instant::now();
            let mut v = Viewport::detached();
            v.set_model(scene);
            v.fit_to(f64::from(w) / f64::from(h));
            gpu.draw_once(&mut v, w, h, 1.0).ok_or("no picture")?;
            first = first.min(started.elapsed());
        }
        let mut v = Viewport::detached();
        v.set_model(crate::model_scene::build(model.geometry.clone())?);
        v.fit_to(f64::from(w) / f64::from(h));
        let overview = gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?;
        if let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir)?;
            let png = pcbcraft_solidcraft::render::png(w as usize, h as usize, overview)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("vias-{count}.png")), png)?;
        }
        let mut orbit = std::time::Duration::MAX;
        for k in 0..5 {
            v.camera.yaw += 0.05 * f64::from(k + 1);
            let started = std::time::Instant::now();
            gpu.draw_once(&mut v, w, h, 1.0).ok_or("no picture")?;
            orbit = orbit.min(started.elapsed());
        }
        let triangles = gpu
            .resources
            .get::<Renderer>()
            .and_then(|r| r.stats().lock().ok().map(|s| s.triangles))
            .unwrap_or(0);
        eprintln!(
            "| {count} | {native:.2?} | {built:.2?} | {first:.2?} | {orbit:.2?} | {:.1} MiB | {:.2} MiB | {:.2} MiB | {triangles} |",
            stream as f64 / f64::from(1 << 20),
            meshes as f64 / f64::from(1 << 20),
            placements as f64 / f64::from(1 << 20),
        );
    }
    Ok(())
}

/// The ESP32-S31 example's 3D view: its native build, the GPU-ready scene and
/// the first frame on the finished build. Run with
/// `cargo test --profile ci -p pcbcraft esp32_display_timings -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn esp32_display_timings() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    let document = pcbcraft_io::read_project(&path)?.into_document();
    let started = std::time::Instant::now();
    let model = pcbcraft_solidcraft::project::build(&document.project)?;
    let native = started.elapsed();
    let started = std::time::Instant::now();
    let scene = crate::model_scene::build(model.geometry.clone())?;
    let built = started.elapsed();
    let triangles: usize = model.geometry.iter().map(|g| g.mesh.triangles.len()).sum();
    let started = std::time::Instant::now();
    let mut v = Viewport::detached();
    v.set_model(scene);
    v.fit_to(1280.0 / 800.0);
    gpu.draw_once(&mut v, 1280, 800, 1.0).ok_or("no picture")?;
    let first = started.elapsed();
    let picture = gpu.draw(&mut v, 1280, 800, 1.0).ok_or("no picture")?;
    if let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        let png =
            pcbcraft_solidcraft::render::png(1280, 800, picture).ok_or("png encoding failed")?;
        std::fs::write(dir.join("esp32-overview.png"), png)?;
    }
    eprintln!(
        "esp32: native build {native:.2?}, scene {built:.2?}, first frame {first:.2?}, {} bodies, {triangles} triangles",
        model.geometry.len()
    );
    Ok(())
}
