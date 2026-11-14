//! Follow the net as the renderer draws it: a bottom track seen from
//! above shows through the faded board, and pictures for review.
use super::*;

/// A bottom-side track of the showcase: its pick identity and the middle
/// of its top face.
fn bottom_track(viewport: &Viewport) -> Option<(u64, Vec3)> {
    let model = viewport.model.as_ref()?;
    let g = model.geometry.iter().find(|g| {
        g.kind == BodyKind::CopperTrack
            && g.layer == Some(sparksmith_core::CopperLayer::Bottom)
            && g.item.is_some()
            && (g.max[0] - g.min[0]).max(g.max[1] - g.min[1]) > 3.0
    })?;
    Some((
        g.item?,
        Vec3::new(
            (g.min[0] + g.max[0]) / 2.0,
            (g.min[1] + g.max[1]) / 2.0,
            g.max[2],
        ),
    ))
}

/// The pixels within `r` of the frame's middle.
fn middle(rgba: &[u8], w: u32, h: u32, r: u32) -> Vec<[u8; 4]> {
    let mut out = Vec::new();
    for y in h / 2 - r..=h / 2 + r {
        for x in w / 2 - r..=w / 2 + r {
            let at = ((y * w + x) * 4) as usize;
            if let Some(p) = rgba
                .get(at..at + 4)
                .and_then(|p| <[u8; 4]>::try_from(p).ok())
            {
                out.push(p);
            }
        }
    }
    out
}

fn mean(pixels: &[[u8; 4]]) -> [f64; 3] {
    let n = pixels.len().max(1) as f64;
    std::array::from_fn(|c| pixels.iter().map(|p| f64::from(p[c])).sum::<f64>() / n)
}

/// Seen from above, a bottom track lies under the laminate and the top
/// mask: hidden as built, it shows through them while followed, in its
/// copper's warm colour, and hides again once the look is off.
#[test]
fn a_followed_bottom_track_shows_through_the_faded_board() -> Result<(), Box<dyn std::error::Error>>
{
    let _context = sparksmith_core::library::standards::Context::fixture().map(|c| c.enter());
    let Some(mut gpu) = Gpu::new() else {
        if std::env::var_os("SPARKSMITH_REQUIRE_GPU").is_some() {
            return Err("SPARKSMITH_REQUIRE_GPU is set but no GPU adapter was found".into());
        }
        eprintln!("no GPU adapter: skipped");
        return Ok(());
    };
    let mut v = viewport(&showcase_project()?)?;
    let (item, at) = bottom_track(&v).ok_or("the showcase has no bottom track")?;
    aim(&mut v, at, 12.0, 0.0, 89.0);
    v.set_realistic(false);
    let (w, h) = (160, 120);
    let built = mean(&middle(
        &gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?,
        w,
        h,
        2,
    ));
    v.set_follow_net(true);
    v.set_focus(vec![item]);
    super::super::tests::settle(&mut v)?;
    let followed = mean(&middle(
        &gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?,
        w,
        h,
        2,
    ));
    v.set_follow_net(false);
    super::super::tests::settle(&mut v)?;
    let off = mean(&middle(
        &gpu.draw(&mut v, w, h, 1.0).ok_or("no picture")?,
        w,
        h,
        2,
    ));
    eprintln!("over the bottom track: built {built:?}, followed {followed:?}, off {off:?}");
    let apart = |a: [f64; 3], b: [f64; 3]| (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f64::max);
    assert!(apart(built, followed) > 30.0, "the track shows through");
    assert!(
        followed[0] > followed[2] + 20.0,
        "in its copper's warm colour: {followed:?}"
    );
    assert!(apart(built, off) < 4.0, "off draws the board as built");
    Ok(())
}

/// The faded board around a followed net reads in the Realistic look as
/// it does in the Standard one, not as a dark smoke: the overview's board
/// pixels are about as bright in both looks.
#[test]
fn the_faded_board_is_as_legible_in_the_realistic_look() -> Result<(), Box<dyn std::error::Error>> {
    let _context = sparksmith_core::library::standards::Context::fixture().map(|c| c.enter());
    let Some(mut gpu) = super::esp32::gpu_or_skip()? else {
        return Ok(());
    };
    let mut v = viewport(&showcase_project()?)?;
    let (item, _) = bottom_track(&v).ok_or("the showcase has no bottom track")?;
    v.set_follow_net(true);
    v.set_focus(vec![item]);
    let (w, h) = (240, 180);
    let mut board =
        |v: &mut Viewport, realistic: bool| -> Result<f64, Box<dyn std::error::Error>> {
            v.set_realistic(realistic);
            let rgba = gpu.draw(v, w, h, 1.0).ok_or("no picture")?;
            let bg = |c: u8, b: f64| (f64::from(c) / 255.0 - b).abs() < 0.04;
            let shown: Vec<f64> = rgba
                .chunks_exact(4)
                .filter(|p| {
                    !(bg(p[0], BACKGROUND.r) && bg(p[1], BACKGROUND.g) && bg(p[2], BACKGROUND.b))
                })
                .map(luminance)
                .collect();
            if shown.len() < 2000 {
                return Err("the board is not in the frame".into());
            }
            Ok(shown.iter().sum::<f64>() / shown.len() as f64)
        };
    let standard = board(&mut v, false)?;
    let realistic = board(&mut v, true)?;
    eprintln!("faded board luminance: standard {standard:.4}, realistic {realistic:.4}");
    assert!(
        realistic >= 0.8 * standard,
        "a dark smoke in the Realistic look: {realistic:.4} against {standard:.4}"
    );
    Ok(())
}

/// Follow-the-net pictures of the showcase for review, written to
/// `SPARKSMITH_PICTURES`: the overview and a close-up of a bottom track,
/// as built and followed, in both looks.
#[test]
#[ignore = "writes follow-net pictures"]
fn write_follow_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let project = showcase_project()?;
    let model = sparksmith_solidcraft::project::build(&project)?;
    let sources = model.sources.clone();
    let mut v = Viewport::detached();
    v.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
    v.fit_to(4.0 / 3.0);
    let (item, at) = bottom_track(&v).ok_or("the showcase has no bottom track")?;
    // The track's whole net, as the app's selection gives it.
    let net = sources
        .iter()
        .find(|s| s.pick_id == item)
        .and_then(|s| s.net);
    let focus: Vec<u64> = sources
        .iter()
        .filter(|s| s.pick_id == item || (s.net.is_some() && s.net == net))
        .map(|s| s.pick_id)
        .collect();
    let overview = v.camera;
    for (view, close) in [("overview", false), ("close", true)] {
        if close {
            aim(&mut v, at, 30.0, 30.0, 40.0);
        } else {
            v.camera = overview;
        }
        for (look, realistic) in [("realistic", true), ("standard", false)] {
            v.set_realistic(realistic);
            for (state, on) in [("built", false), ("followed", true)] {
                v.highlight_selected(focus.clone(), vec![item]);
                v.set_focus(focus.clone());
                v.set_follow_net(on);
                super::super::tests::settle(&mut v)?;
                let (w, h) = (1280, 960);
                let rgba = gpu.draw(&mut v, w, h, 2.0).ok_or("no picture")?;
                let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
                    .ok_or("png encoding failed")?;
                std::fs::write(dir.join(format!("follow-{view}-{look}-{state}.png")), png)?;
            }
        }
    }
    Ok(())
}
