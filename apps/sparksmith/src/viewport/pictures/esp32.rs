//! The ESP32-S31 example board as drawn: the gloss mask's sheen, bare
//! copper, the Standard look's edge and pins, and a click on a part while
//! the net is followed; and pictures of both looks, a peeled stack, X-ray
//! and the followed net for review, written to `SPARKSMITH_PICTURES` by an
//! ignored test.
use super::*;

/// The example board with its marking decals, shown in a viewport.
pub(super) fn esp32() -> Result<
    (
        Project,
        sparksmith_solidcraft::project::NativeModel,
        Viewport,
    ),
    Box<dyn std::error::Error>,
> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith");
    let project = (*sparksmith_io::read_project(&path)?.into_document().project).clone();
    let model = sparksmith_solidcraft::project::build(&project)?;
    let mut geometry = model.geometry.clone();
    geometry.extend(
        sparksmith_solidcraft::marking::board::BoardMarkings::default().decals(
            &project,
            &model.geometry,
            &model.sources,
        ),
    );
    let mut v = Viewport::detached();
    v.set_model(crate::model_scene::build(geometry, &model.stack)?);
    v.fit_to(4.0 / 3.0);
    Ok((project, model, v))
}

/// The pick identities of every body of the net named `name`.
pub(super) fn net_bodies(
    project: &Project,
    model: &sparksmith_solidcraft::project::NativeModel,
    name: &str,
) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let net = project
        .nets
        .iter()
        .find(|n| project.net_name(n.id) == name)
        .map(|n| n.id)
        .ok_or("no such net")?;
    Ok(model
        .sources
        .iter()
        .filter(|s| s.net == Some(net))
        .map(|s| s.pick_id)
        .collect())
}

pub(super) fn settled(v: &mut Viewport) {
    for _ in 0..1000 {
        if !v.receive_repack() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
#[ignore = "writes tuning pictures"]
fn write_esp32_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let (project, model, mut v) = esp32()?;
    let only = std::env::var("SPARKSMITH_PICTURES_ONLY").unwrap_or_default();
    let mut save = |v: &mut Viewport, name: &str| -> Result<(), Box<dyn std::error::Error>> {
        if !name.starts_with(&only) {
            return Ok(());
        }
        settled(v);
        let (w, h) = (1200, 800);
        let rgba = gpu.draw(v, w, h, 2.0).ok_or("no picture")?;
        let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
            .ok_or("png encoding failed")?;
        std::fs::write(dir.join(format!("{name}.png")), png)?;
        Ok(())
    };
    let b = v.bounds;
    let mid = Vec3::new(20.0, 32.0, 1.6);
    for realistic in [true, false] {
        v.set_realistic(realistic);
        let look = if realistic { "realistic" } else { "standard" };
        aim(&mut v, b.center(), 1.2 * b.diagonal(), 30.0, 35.0);
        save(&mut v, &format!("{look}-overview"))?;
        // Low over the module end, where the studio's strip lay on the mask.
        aim(&mut v, Vec3::new(22.0, 40.0, 1.6), 22.0, 200.0, 14.0);
        save(&mut v, &format!("{look}-low-closeup"))?;
        aim(&mut v, Vec3::new(4.0, 30.0, 0.8), 14.0, 60.0, 12.0);
        save(&mut v, &format!("{look}-edge-closeup"))?;
        aim(&mut v, Vec3::new(6.0, 30.0, 4.0), 18.0, 120.0, 15.0);
        save(&mut v, &format!("{look}-header-closeup"))?;
    }
    v.set_realistic(true);
    let stack = model.stack.clone();
    // Peeled: the top films and the parts off, the top copper bare.
    let mut peel = crate::layer_view::LayerView::default();
    for layer in [
        ViewLayer::Bodies,
        ViewLayer::Markings,
        ViewLayer::Silkscreen(Layer::Top),
        ViewLayer::Mask(Layer::Top),
    ] {
        peel.set(layer, false);
    }
    v.set_layers(peel.clone());
    aim(&mut v, mid, 70.0, 30.0, 35.0);
    save(&mut v, "realistic-peel-top-copper")?;
    aim(&mut v, Vec3::new(20.0, 12.0, 1.6), 20.0, 30.0, 40.0);
    save(&mut v, "realistic-peel-top-copper-closeup")?;
    // Peeled to the bottom copper.
    let mut deep = peel;
    for layer in ViewLayer::stack(&stack) {
        if !matches!(
            layer,
            ViewLayer::Copper(sparksmith_core::CopperLayer::Bottom)
                | ViewLayer::Mask(Layer::Bottom)
                | ViewLayer::Silkscreen(Layer::Bottom)
                | ViewLayer::Vias
        ) {
            deep.set(layer, false);
        }
    }
    v.set_layers(deep);
    aim(&mut v, Vec3::new(20.0, 20.0, 0.2), 30.0, 30.0, 40.0);
    save(&mut v, "realistic-peel-bottom-copper")?;
    v.set_layers(crate::layer_view::LayerView::x_ray(&stack));
    aim(&mut v, mid, 70.0, 30.0, 35.0);
    save(&mut v, "realistic-x-ray")?;
    aim(&mut v, Vec3::new(20.0, 12.0, 0.8), 25.0, 30.0, 40.0);
    save(&mut v, "realistic-x-ray-closeup")?;
    aim(&mut v, mid, 70.0, 30.0, 35.0);
    v.set_realistic(false);
    save(&mut v, "standard-x-ray")?;
    v.set_layers(crate::layer_view::LayerView::default());
    let net = net_bodies(&project, &model, "USB_DN")?;
    v.set_follow_net(true);
    v.highlight_selected(net.clone(), Vec::new());
    v.set_focus(net);
    for realistic in [true, false] {
        v.set_realistic(realistic);
        let look = if realistic { "realistic" } else { "standard" };
        aim(&mut v, mid, 70.0, 30.0, 35.0);
        save(&mut v, &format!("{look}-follow-overview"))?;
        aim(&mut v, Vec3::new(18.0, 10.0, 1.6), 22.0, 20.0, 30.0);
        save(&mut v, &format!("{look}-follow-usb"))?;
    }
    Ok(())
}

/// Close-ups of the example board's solder joints and contact shadows, in
/// the realistic look: chips, the SOT-23 regulators, the module's edge and
/// the headers' pins under the board.
#[test]
#[ignore = "writes tuning pictures"]
fn write_esp32_joint_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let (_, _, mut v) = esp32()?;
    v.set_realistic(true);
    let shots: [(&str, [f64; 3], f64, f64, f64); 9] = [
        ("joints-regulator", [20.0, 12.0, 1.8], 9.0, 210.0, 30.0),
        ("joints-regulator-low", [20.0, 12.0, 1.8], 7.0, 200.0, 12.0),
        ("joints-ldo-caps", [8.5, 13.5, 1.8], 8.0, 30.0, 28.0),
        ("joints-led", [33.0, 11.2, 1.8], 5.0, 20.0, 25.0),
        ("joints-module-edge", [9.0, 42.0, 1.8], 10.0, 120.0, 20.0),
        ("joints-module-edge-2", [9.0, 42.0, 1.8], 10.0, 300.0, 20.0),
        ("joints-overview", [20.0, 32.0, 1.6], 70.0, 30.0, 35.0),
        ("joints-header-top", [2.54, 21.0, 1.8], 14.0, 20.0, 30.0),
        ("joints-header-bottom", [2.54, 21.0, 0.0], 14.0, 20.0, -30.0),
    ];
    for (name, at, distance, yaw, pitch) in shots {
        aim(&mut v, Vec3::new(at[0], at[1], at[2]), distance, yaw, pitch);
        settled(&mut v);
        let (w, h) = (1200, 800);
        let rgba = gpu.draw(&mut v, w, h, 2.0).ok_or("no picture")?;
        let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
            .ok_or("png encoding failed")?;
        std::fs::write(dir.join(format!("{name}.png")), png)?;
    }
    Ok(())
}

/// Opening the example board in 3D: the model made (as the worker makes
/// it while the board is edited in 2D), the scene prepared, and the first
/// frame drawn; best of five. `cargo test --profile ci -p sparksmith
/// opening_esp32_timings -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn opening_esp32_timings() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith");
    let project = (*sparksmith_io::read_project(&path)?.into_document().project).clone();
    let (mut built, mut scened, mut framed) = (f64::MAX, f64::MAX, f64::MAX);
    let mut triangles = 0;
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let model = sparksmith_solidcraft::project::build(&project)?;
        built = built.min(start.elapsed().as_secs_f64() * 1e3);
        triangles = model.geometry.iter().map(|g| g.mesh.triangles.len()).sum();
        let start = std::time::Instant::now();
        let scene = crate::model_scene::build(model.geometry, &model.stack)?;
        scened = scened.min(start.elapsed().as_secs_f64() * 1e3);
        let start = std::time::Instant::now();
        let mut viewport = Viewport::detached();
        viewport.set_model(scene);
        viewport.fit_to(16.0 / 10.0);
        gpu.draw(&mut viewport, 1600, 1000, 1.0).ok_or("no frame")?;
        framed = framed.min(start.elapsed().as_secs_f64() * 1e3);
    }
    println!(
        "esp32: model {built:.0} ms, scene {scened:.0} ms, first frame {framed:.0} ms; {triangles} triangles"
    );
    Ok(())
}

pub(super) fn gpu_or_skip() -> Result<Option<Gpu>, Box<dyn std::error::Error>> {
    match Gpu::new() {
        Some(gpu) => Ok(Some(gpu)),
        None if std::env::var_os("SPARKSMITH_REQUIRE_GPU").is_some() => {
            Err("SPARKSMITH_REQUIRE_GPU is set but no GPU adapter was found".into())
        }
        None => {
            eprintln!("no GPU adapter: skipped");
            Ok(None)
        }
    }
}

/// How softly the brightest band across the rows `rows` and columns
/// `columns` of a `w`-wide picture falls off: per row, the pixels its
/// brightness takes to fall from four fifths to one fifth of its rise over
/// the row's median, on its steeper side; the median of the rows. Rows
/// without a band (a rise under `min_rise`) are skipped.
fn band_edge(
    rgba: &[u8],
    w: usize,
    rows: std::ops::Range<usize>,
    columns: std::ops::Range<usize>,
    min_rise: f64,
) -> Option<usize> {
    let mut edges = Vec::new();
    for y in rows {
        let row: Vec<f64> = columns
            .clone()
            .filter_map(|x| rgba.get(4 * (y * w + x)..4 * (y * w + x) + 4))
            .map(luminance)
            .collect();
        let mut sorted = row.clone();
        sorted.sort_by(f64::total_cmp);
        let median = *sorted.get(sorted.len() / 2)?;
        let (peak_at, peak) = row
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        if peak - median < min_rise {
            continue;
        }
        let level = |share: f64| median + share * (peak - median);
        let fall = |side: Vec<f64>| {
            let high = side.iter().position(|l| *l <= level(0.8))?;
            let low = side.iter().position(|l| *l <= level(0.2))?;
            Some(low.saturating_sub(high))
        };
        let left = fall(row.get(..peak_at)?.iter().rev().copied().collect());
        let right = fall(row.get(peak_at..)?.to_vec());
        if let Some(edge) = left.into_iter().chain(right).min() {
            edges.push(edge);
        }
    }
    edges.sort_unstable();
    edges.get(edges.len() / 2).copied()
}

/// Seen low over the gloss mask, the studio's strip light lies on it as a
/// soft sheen: the band's brightness falls off over many pixels on either
/// side, not in a hard-edged white shaft a few pixels wide.
#[test]
fn the_gloss_mask_sheen_has_soft_edges() -> Result<(), Box<dyn std::error::Error>> {
    let _context = sparksmith_core::library::standards::Context::fixture().map(|c| c.enter());
    let Some(mut gpu) = gpu_or_skip()? else {
        return Ok(());
    };
    let (_, _, mut v) = esp32()?;
    let (w, h) = (480, 320);
    // The low view where the strip light's reflection crosses the board.
    aim(&mut v, Vec3::new(20.0, 32.0, 1.6), 45.0, 330.0, 12.0);
    let rgba = gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?;
    if let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") {
        let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba.clone())
            .ok_or("png encoding failed")?;
        std::fs::write(std::path::Path::new(&dir).join("sheen-test.png"), png)?;
    }
    // The open mask under the module, clear of the headers and parts.
    let rows = (h as usize * 60 / 100)..(h as usize * 75 / 100);
    let columns = (w as usize * 25 / 100)..(w as usize * 68 / 100);
    let edge = band_edge(&rgba, w as usize, rows, columns, 0.05).ok_or("no sheen on the mask")?;
    eprintln!("sheen edge: {edge} px");
    assert!(
        edge >= 10,
        "a hard-edged streak: its edge falls off in {edge} px"
    );
    Ok(())
}

/// The mean straight colour of the middle `share` of a `w` × `h` picture.
fn middle_colour(rgba: &[u8], w: u32, h: u32, share: f64) -> [f64; 3] {
    let (w, h) = (w as usize, h as usize);
    let (dx, dy) = (
        (w as f64 * share / 2.0) as usize,
        (h as f64 * share / 2.0) as usize,
    );
    let mut sum = [0.0; 3];
    let mut n = 0.0;
    for y in h / 2 - dy..h / 2 + dy {
        for x in w / 2 - dx..w / 2 + dx {
            if let Some(p) = rgba.get(4 * (y * w + x)..4 * (y * w + x) + 3) {
                for (s, c) in sum.iter_mut().zip(p) {
                    *s += f64::from(*c);
                }
                n += 1.0;
            }
        }
    }
    sum.map(|s| s / f64::max(n, 1.0))
}

/// A top-side pour bared by peeling the mask reads as copper: a warm
/// orange-pink metal, the green well above the blue, not the cool pale
/// salmon of freshly polished copper under the studio, and not white.
#[test]
fn bare_copper_reads_as_copper() -> Result<(), Box<dyn std::error::Error>> {
    let _context = sparksmith_core::library::standards::Context::fixture().map(|c| c.enter());
    let Some(mut gpu) = gpu_or_skip()? else {
        return Ok(());
    };
    let (_, _, mut v) = esp32()?;
    let mut peel = crate::layer_view::LayerView::default();
    for layer in [
        ViewLayer::Bodies,
        ViewLayer::Markings,
        ViewLayer::Silkscreen(Layer::Top),
        ViewLayer::Mask(Layer::Top),
    ] {
        peel.set(layer, false);
    }
    v.set_layers(peel);
    // Over the pour beside the module, clear of pads and openings.
    aim(&mut v, Vec3::new(30.0, 30.0, 1.6), 12.0, 30.0, 50.0);
    let (w, h) = (320, 240);
    let rgba = gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?;
    if let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") {
        let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba.clone())
            .ok_or("png encoding failed")?;
        std::fs::write(std::path::Path::new(&dir).join("copper-test.png"), png)?;
    }
    let [r, g, b] = middle_colour(&rgba, w, h, 0.3);
    eprintln!("bare copper: {r:.0} {g:.0} {b:.0}");
    assert!(r > g && g > b, "a warm metal: {r:.0} {g:.0} {b:.0}");
    // Hue: freshly polished copper's pale salmon has the green barely
    // above the blue; copper's orange has it well up toward the red.
    let hue = (g - b) / (r - g);
    assert!(hue >= 0.5, "pale salmon, not copper: {hue:.2}");
    assert!(hue <= 1.4, "gold, not copper: {hue:.2}");
    assert!((r - b) / r >= 0.25, "washed out: {r:.0} {g:.0} {b:.0}");
    Ok(())
}

/// In the Standard look every side of the board's routed edge reads as
/// laminate, the far side too (which no light of the material preview's
/// fixed rig reaches), and a header's gold pins read gold, not dark brown.
#[test]
fn the_standard_look_shows_the_edge_as_laminate_and_pins_as_gold()
-> Result<(), Box<dyn std::error::Error>> {
    let _context = sparksmith_core::library::standards::Context::fixture().map(|c| c.enter());
    let Some(mut gpu) = gpu_or_skip()? else {
        return Ok(());
    };
    let (_, _, mut v) = esp32()?;
    v.set_realistic(false);
    let (w, h) = (240, 180);
    let mut save = |v: &mut Viewport, name: &str| -> Result<[f64; 3], Box<dyn std::error::Error>> {
        let rgba = gpu.draw(v, w, h, 1.0).ok_or("no picture")?;
        if let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") {
            let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba.clone())
                .ok_or("png encoding failed")?;
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png)?;
        }
        Ok(middle_colour(&rgba, w, h, 0.04))
    };
    // Each edge's middle, seen square on from just above the board.
    for (name, at, yaw) in [
        ("far", Vec3::new(20.0, 64.0, 0.8), 180.0),
        ("near", Vec3::new(20.0, 0.0, 0.8), 0.0),
        ("left", Vec3::new(0.0, 40.0, 0.8), 90.0),
        ("right", Vec3::new(40.0, 40.0, 0.8), 270.0),
    ] {
        aim(&mut v, at, 8.0, yaw, 4.0);
        let [r, g, b] = save(&mut v, &format!("standard-edge-{name}"))?;
        eprintln!("{name} edge: {r:.0} {g:.0} {b:.0}");
        assert!(
            r + g + b > 3.0 * 90.0,
            "the {name} edge is dark: {r:.0} {g:.0} {b:.0}"
        );
        assert!(
            r > b + 20.0 && g > b + 15.0,
            "not laminate: {r:.0} {g:.0} {b:.0}"
        );
    }
    // A header pin's upper half, seen from either side.
    let pin = v
        .model
        .as_ref()
        .and_then(|m| {
            m.geometry
                .iter()
                .find(|g| g.kind == BodyKind::ModelLead && g.max[2] - g.min[2] > 5.0)
        })
        .map(|g| {
            Vec3::new(
                (g.min[0] + g.max[0]) / 2.0,
                (g.min[1] + g.max[1]) / 2.0,
                g.max[2] - 2.0,
            )
        })
        .ok_or("no header pin")?;
    for yaw in [0.0, 180.0] {
        aim(&mut v, pin, 5.0, yaw, 0.0);
        let [r, g, b] = save(&mut v, &format!("standard-pin-{yaw}"))?;
        eprintln!("pin from {yaw}°: {r:.0} {g:.0} {b:.0}");
        assert!(r > 150.0, "a dark pin: {r:.0} {g:.0} {b:.0}");
        assert!(
            r > b + 40.0 && g > b + 25.0,
            "not gold: {r:.0} {g:.0} {b:.0}"
        );
    }
    Ok(())
}

/// Following a net, a click on a part's package follows the net of the
/// part's pad under the pointer (seen through the faded package); over
/// the package where none of its pads lies, the part is selected and the
/// followed net kept, never switched to another of its nets.
#[test]
fn a_click_on_a_part_follows_the_pad_under_it_or_keeps_the_net()
-> Result<(), Box<dyn std::error::Error>> {
    let _context = sparksmith_core::library::standards::Context::fixture().map(|c| c.enter());
    let (project, model, mut v) = esp32()?;
    let net_id = |name: &str| {
        project
            .nets
            .iter()
            .find(|n| project.net_name(n.id) == name)
            .map(|n| n.id)
    };
    let (usb_dn, usb_dp) = (
        net_id("USB_DN").ok_or("no USB_DN")?,
        net_id("USB_DP").ok_or("no USB_DP")?,
    );
    let source = |id: u64| model.sources.iter().find(|s| s.pick_id == id);
    // U3, the USB protection part: its package and its USB_DP pad.
    let package = model
        .sources
        .iter()
        .find(|s| s.label == "U3 model")
        .ok_or("no U3 package")?;
    let instance = package.instance.ok_or("U3 has no instance")?;
    let pad = model
        .sources
        .iter()
        .find(|s| s.instance == Some(instance) && s.net == Some(usb_dp))
        .ok_or("no U3 pad on USB_DP")?;
    let bounds = |item: u64| {
        let mut b = sparksmith_solidcraft::geom::Aabb3::EMPTY;
        for g in v
            .model
            .iter()
            .flat_map(|m| &m.geometry)
            .filter(|g| g.item == Some(item))
        {
            b.add(Vec3::new(g.min[0], g.min[1], g.min[2]));
            b.add(Vec3::new(g.max[0], g.max[1], g.max[2]));
        }
        b
    };
    let body = bounds(package.pick_id);
    let land = bounds(pad.pick_id);
    let follow =
        |v: &mut Viewport| -> Result<crate::pcb_editor::PcbEditor, Box<dyn std::error::Error>> {
            let net = net_bodies(&project, &model, "USB_DN")?;
            v.set_follow_net(true);
            v.set_focus(net);
            let mut editor = crate::pcb_editor::PcbEditor::default();
            editor.set_selection(None, None, Some(usb_dn));
            Ok(editor)
        };
    let click = |v: &mut Viewport, editor: &mut crate::pcb_editor::PcbEditor, at: Vec3| {
        aim(v, at, 10.0, 0.0, 89.0);
        let (w, h) = (800.0, 600.0);
        let picked = v.pick(w / 2.0, h / 2.0, w, h).and_then(source);
        let along: Vec<_> = v
            .picks_along(w / 2.0, h / 2.0, w, h)
            .into_iter()
            .filter_map(source)
            .collect();
        assert_eq!(
            along.first().map(|s| s.pick_id),
            Some(package.pick_id),
            "over the package"
        );
        editor.select_picked_following(picked, &along);
    };
    // Over the package, clear of its pads: the net is kept.
    let clear = (0..9)
        .flat_map(|i| (0..5).map(move |j| (i, j)))
        .map(|(i, j)| {
            Vec3::new(
                body.min.x + (body.max.x - body.min.x) * (0.1 + 0.1 * f64::from(i)),
                body.min.y + (body.max.y - body.min.y) * (0.1 + 0.2 * f64::from(j)),
                body.max.z,
            )
        })
        .find(|at| {
            aim(&mut v, *at, 10.0, 0.0, 89.0);
            let along = v.picks_along(400.0, 300.0, 800.0, 600.0);
            along.first() == Some(&package.pick_id)
                && !along
                    .iter()
                    .filter_map(|id| source(*id))
                    .any(|s| s.instance == Some(instance) && s.pad.is_some())
        })
        .ok_or("no point of the package clear of its pads")?;
    let mut editor = follow(&mut v)?;
    click(&mut v, &mut editor, clear);
    assert_eq!(
        editor.selected_instance(),
        Some(instance),
        "the part is selected"
    );
    assert_eq!(
        editor.highlighted_net(),
        Some(usb_dn),
        "the followed net is kept"
    );
    // Over the USB_DP pad's lead: its net is followed.
    let mut editor = follow(&mut v)?;
    let lead = Vec3::new(land.center().x, land.center().y, body.max.z);
    click(&mut v, &mut editor, lead);
    assert_eq!(editor.selected_instance(), Some(instance));
    assert_eq!(editor.highlighted_net(), Some(usb_dp), "the pad's net");
    Ok(())
}
