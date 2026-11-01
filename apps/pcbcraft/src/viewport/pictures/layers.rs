//! Evidence pictures of the 3D layer toggles: the demo board whole, with
//! the top solder mask hidden (the copper under it shows), with the
//! substrate hidden, and with the copper alone. Written to
//! `PCBCRAFT_PICTURES` by `write_layer_pictures`.
use super::*;
use crate::layer_view::{LayerView, ViewLayer};
use pcbcraft_core::{CopperLayer, Layer};

#[test]
#[ignore = "writes layer pictures"]
fn write_layer_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") else {
        return Err("set PCBCRAFT_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let mut v = viewport(&project(Fabrication::default())?)?;
    let target = centre(&v, BodyKind::CopperTrack).ok_or("no track")?;
    let hidden = |layers: &[ViewLayer]| {
        let mut view = LayerView::default();
        for layer in layers {
            view.set(*layer, false);
        }
        view
    };
    let views = [
        ("layers-all", LayerView::default()),
        ("layers-no-top-mask", hidden(&[ViewLayer::Mask(Layer::Top)])),
        ("layers-no-substrate", hidden(&[ViewLayer::Substrate])),
        (
            "layers-copper-only",
            hidden(&[
                ViewLayer::Bodies,
                ViewLayer::Substrate,
                ViewLayer::Mask(Layer::Top),
                ViewLayer::Mask(Layer::Bottom),
                ViewLayer::Silkscreen(Layer::Top),
                ViewLayer::Silkscreen(Layer::Bottom),
            ]),
        ),
        (
            "layers-no-top-copper-no-mask",
            hidden(&[
                ViewLayer::Mask(Layer::Top),
                ViewLayer::Copper(CopperLayer::Top),
            ]),
        ),
    ];
    for (name, view) in views {
        // A fresh pack of the shown layers: what the background repack
        // publishes in the window.
        let model = v.model.as_ref().ok_or("no model")?;
        let (pbr, _) = crate::model_scene::pack(&model.geometry, &view)?;
        v.set_layers(view);
        v.repack = None;
        v.publish_pbr(pbr);
        for (suffix, distance, yaw, pitch) in [("", 0.0, 35.0, 40.0), ("-close", 9.0, 20.0, 50.0)] {
            if distance > 0.0 {
                aim(&mut v, target, distance, yaw, pitch);
            } else {
                v.fit_to(4.0 / 3.0);
                v.camera.yaw = yaw.to_radians();
                v.camera.pitch = pitch.to_radians();
                v.fit_to(4.0 / 3.0);
            }
            let (w, h) = (1024, 768);
            let rgba = gpu.draw(&mut v, w, h, 2.0).ok_or("no picture")?;
            let png = pcbcraft_solidcraft::render::png(w as usize, h as usize, rgba)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("{name}{suffix}.png")), png)?;
        }
    }
    Ok(())
}
