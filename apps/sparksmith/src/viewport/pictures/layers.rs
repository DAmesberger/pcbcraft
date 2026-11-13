//! Evidence pictures of the 3D layer toggles on the 4-layer showcase:
//! every layer, the top mask hidden (the copper under it shows), X-ray
//! (copper and vias only, the stackup's copper layer over layer), the
//! copper alone, each inner layer alone, and the top copper and mask
//! hidden. Written to `SPARKSMITH_PICTURES` by `write_layer_pictures`.
use super::*;
use crate::layer_view::{LayerView, ViewLayer};
use sparksmith_core::{CopperLayer, Layer};

#[test]
#[ignore = "writes layer pictures"]
fn write_layer_pictures() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("SPARKSMITH_PICTURES") else {
        return Err("set SPARKSMITH_PICTURES".into());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let Some(mut gpu) = Gpu::new() else {
        return Err("no GPU adapter".into());
    };
    let project = showcase_project()?;
    let stack =
        sparksmith_solidcraft::stack::Stack::of(&project.stackup, project.fabrication.pad_finish);
    if project.stackup.layers().len() < 4 {
        return Err("the showcase is a 4-layer board".into());
    }
    let mut v = viewport(&project)?;
    // The routed middle of the board, where every layer carries tracks.
    let b = v.bounds;
    let target = Vec3::new(
        b.min.x + 0.45 * (b.max.x - b.min.x),
        b.center().y,
        b.max.z.min(2.0),
    );
    let hidden = |layers: &[ViewLayer]| {
        let mut view = LayerView::default();
        for layer in layers {
            view.set(*layer, false);
        }
        view
    };
    let mut copper_only = LayerView::x_ray(&stack);
    copper_only.set(ViewLayer::Vias, false);
    let views = [
        ("layers-all", LayerView::default()),
        ("layers-no-top-mask", hidden(&[ViewLayer::Mask(Layer::Top)])),
        ("layers-x-ray", LayerView::x_ray(&stack)),
        ("layers-copper-only", copper_only),
        (
            "layers-inner-1-only",
            LayerView::only(CopperLayer::Inner(1), &stack),
        ),
        (
            "layers-inner-2-only",
            LayerView::only(CopperLayer::Inner(2), &stack),
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
        show_layers(&mut v, view)?;
        for (suffix, distance, yaw, pitch) in [("", 0.0, 35.0, 40.0), ("-close", 16.0, 20.0, 50.0)]
        {
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
            let png = sparksmith_solidcraft::viewport::png(w as usize, h as usize, rgba)
                .ok_or("png encoding failed")?;
            std::fs::write(dir.join(format!("{name}{suffix}.png")), png)?;
        }
    }
    Ok(())
}
