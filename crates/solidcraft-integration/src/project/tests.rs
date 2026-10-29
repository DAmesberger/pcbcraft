use super::*;
use pcbcraft_core::{Engine, Track};

fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
}

fn fixture() -> Result<Project> {
    let mut project = Engine::demo()
        .map_err(|error| Error::Verification(error.to_string()))?
        .project()
        .clone();
    // Asymmetric offsets exercise the rigid bottom flip independently of the
    // symmetric body/pad shapes. This synthetic test replaces a snapshot to
    // exercise the adapter; production changes publish a new immutable revision.
    project.parts[0].package.model_offset = [0.25, -0.4, 0.2];
    project.parts[0].footprint.datum_offset = [0.3, -0.4];
    for (placement, (position, layer)) in project
        .placements
        .iter_mut()
        .zip([([10.0, 12.0], Layer::Top), ([30.0, 12.0], Layer::Bottom)])
    {
        placement.position = position;
        placement.rotation_deg = 90.0;
        placement.layer = layer;
    }
    for (y, layer) in [(25.0, Layer::Top), (28.0, Layer::Bottom)] {
        let id = project
            .allocate()
            .map_err(|error| Error::Verification(error.to_string()))?;
        project.tracks.push(Track {
            id,
            net: "net:2:2".into(),
            start: [5.0, y],
            end: [15.0, y],
            width: 0.4,
            layer,
            locked: false,
        });
    }
    Ok(project)
}

fn source_geometry(model: &NativeModel, matches: impl Fn(&Source) -> bool) -> Result<&Geometry> {
    let source = model
        .sources
        .iter()
        .find(|source| matches(source))
        .ok_or_else(|| Error::Verification("missing source identity".into()))?;
    model
        .geometry
        .iter()
        .find(|geometry| geometry.item == Some(source.pick_id))
        .ok_or_else(|| Error::Verification("source has no geometry".into()))
}

fn bounds(geometry: &Geometry, min: [f64; 3], max: [f64; 3]) {
    for axis in 0..3 {
        near(geometry.min[axis], min[axis]);
        near(geometry.max[axis], max[axis]);
    }
}

#[test]
fn native_project_has_analytic_board_copper_and_rigid_top_bottom_placements() -> Result<()> {
    let project = fixture()?;
    let model = build(&project)?;
    assert_eq!(model.geometry.len(), 9); // substrate, 2 envelopes, 4 pads, 2 traces
    assert_eq!(model.sources.len(), 8);
    let substrate = model
        .geometry
        .iter()
        .find(|geometry| geometry.item.is_none())
        .ok_or_else(|| Error::Verification("missing substrate".into()))?;
    bounds(substrate, [0.0, 0.0, 0.0], [60.0, 40.0, 1.6]);
    near(substrate.volume, 60.0 * 40.0 * 1.6);
    let top = source_geometry(&model, |s| s.instance == Some(2) && s.pad.is_none())?;
    bounds(top, [9.8, 11.25, 1.835], [11.0, 13.25, 2.435]);
    near(top.volume, 2.0 * 1.2 * 0.6);
    let bottom = source_geometry(&model, |s| s.instance == Some(3) && s.pad.is_none())?;
    bounds(bottom, [29.0, 11.25, -0.835], [30.2, 13.25, -0.235]);
    for (instance, center, z) in [
        (2, [9.6, 10.5], [1.6, 1.635]),
        (3, [30.4, 10.5], [-0.035, 0.0]),
    ] {
        let pad = source_geometry(&model, |s| s.instance == Some(instance) && s.pad == Some(1))?;
        // A 1 × 1.4 mm land rotated 90° has a 1.4 × 1 mm world footprint.
        bounds(
            pad,
            [center[0] - 0.7, center[1] - 0.5, z[0]],
            [center[0] + 0.7, center[1] + 0.5, z[1]],
        );
        near(pad.volume, 1.0 * 1.4 * 0.035);
    }
    for track in &project.tracks {
        let geometry = source_geometry(&model, |s| s.track == Some(track.id))?;
        let z = if track.layer == Layer::Top {
            [1.6, 1.635]
        } else {
            [-0.035, 0.0]
        };
        bounds(
            geometry,
            [5.0, track.start[1] - 0.2, z[0]],
            [15.0, track.start[1] + 0.2, z[1]],
        );
        near(geometry.volume, 10.0 * 0.4 * 0.035);
        let source = model
            .sources
            .iter()
            .find(|s| s.track == Some(track.id))
            .ok_or_else(|| Error::Verification("missing trace source".into()))?;
        assert_eq!(source.net.as_deref(), Some("net:2:2"));
        assert_eq!(source.layer, Some(track.layer));
    }
    Ok(())
}

#[test]
fn diagonal_traces_have_analytic_butt_caps_on_both_copper_layers() -> Result<()> {
    let mut project = fixture()?;
    for track in &mut project.tracks {
        track.start = [5.0, 25.0];
        track.end = [9.0, 28.0];
    }
    let model = build(&project)?;
    for track in &project.tracks {
        let geometry = source_geometry(&model, |source| source.track == Some(track.id))?;
        // The 4:3 direction has length 5 and unit normal (-3/5, 4/5).
        // At half-width 0.2, butt-cap corners extend 0.12 in X and 0.16 in Y.
        let z = if track.layer == Layer::Top {
            [1.6, 1.635]
        } else {
            [-0.035, 0.0]
        };
        bounds(geometry, [4.88, 24.84, z[0]], [9.12, 28.16, z[1]]);
        near(geometry.volume, 5.0 * 0.4 * 0.035);
    }
    Ok(())
}

#[test]
fn native_reopen_retains_sources_and_parameter_driven_board_parts_and_copper() -> Result<()> {
    let original = build(&fixture()?)?;
    let temporary = tempfile::tempdir()?;
    let path = temporary
        .path()
        .join("project.solidcraft")
        .to_string_lossy()
        .into_owned();
    original.save(&path)?;
    let mut reopened = Session::default();
    call(
        &mut reopened,
        "doc.open",
        json!({"path":path,"read_only":true}),
    )?;
    let metadata = serde_json::to_value(&reopened.doc.metadata)?;
    let retained: Vec<Source> =
        serde_json::from_value(metadata["org.pcbcraft.electronics"]["sources"].clone())?;
    assert_eq!(
        serde_json::to_value(&retained)?,
        serde_json::to_value(&original.sources)?
    );
    let before = finish(reopened, retained)?;
    for previous in &original.geometry {
        let restored = before
            .geometry
            .iter()
            .find(|g| g.item == previous.item)
            .ok_or_else(|| Error::Verification("reopen lost source geometry".into()))?;
        bounds(restored, previous.min, previous.max);
        near(restored.volume, previous.volume);
    }
    let mut reopened = before.session;
    call(
        &mut reopened,
        "parameters.change",
        json!({"changes":{
            "board_thickness":"2.4 mm", "copper_thickness":"0.07 mm",
            "instance_2_x":"15 mm", "instance_2_y":"18 mm", "instance_2_angle":"0 deg",
            "instance_2_length":"4 mm", "instance_3_angle":"180 deg"
        }}),
    )?;
    let changed = finish(reopened, before.sources)?;
    let top = source_geometry(&changed, |s| s.instance == Some(2) && s.pad.is_none())?;
    bounds(top, [13.25, 17.0, 2.67], [17.25, 18.2, 3.27]);
    near(top.volume, 4.0 * 1.2 * 0.6);
    let bottom = source_geometry(&changed, |s| s.instance == Some(3) && s.pad.is_none())?;
    bounds(bottom, [28.75, 11.0, -0.87], [30.75, 12.2, -0.27]);
    let top_pad = source_geometry(&changed, |s| s.instance == Some(2) && s.pad == Some(1))?;
    bounds(top_pad, [13.0, 17.7, 2.4], [14.0, 19.1, 2.47]);
    let bottom_pad = source_geometry(&changed, |s| s.instance == Some(3) && s.pad == Some(1))?;
    bounds(bottom_pad, [31.0, 11.7, -0.07], [32.0, 13.1, 0.0]);
    for source in changed.sources.iter().filter(|s| s.track.is_some()) {
        let geometry = source_geometry(&changed, |s| s.pick_id == source.pick_id)?;
        near(geometry.volume, 10.0 * 0.4 * 0.07);
        near(
            geometry.min[2],
            if source.layer == Some(Layer::Top) {
                2.4
            } else {
                -0.07
            },
        );
    }
    Ok(())
}

#[test]
fn authoring_preview_uses_package_dimensions_and_model_offsets() -> Result<()> {
    let mut part = pcbcraft_core::parts::passive_template("Offset body");
    part.package.body_size = [4.0, 3.0, 2.0];
    part.package.model_offset = [1.5, -2.0, 0.4];
    let model = preview(&part)?;
    assert_eq!(model.geometry.len(), 1);
    bounds(&model.geometry[0], [-0.5, -3.5, 0.4], [3.5, -0.5, 2.4]);
    near(model.geometry[0].volume, 24.0);
    Ok(())
}
