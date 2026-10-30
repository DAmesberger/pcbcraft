use super::*;
use pcbcraft_core::library::PackageBody;
use pcbcraft_core::{Engine, TerminalId, Track};
use solidcraft_engine::geom::Vec3;

/// The demo's two instances, by designator.
fn instances(project: &Project) -> Result<(Id, Id)> {
    let id = |designator| {
        project
            .instance_by_designator(designator)
            .map(|i| i.id)
            .ok_or_else(|| Error::Verification(format!("demo has no {designator}")))
    };
    Ok((id("R1")?, id("R2")?))
}

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
    project.library.packages[0].definition.model_offset = [0.25, -0.4, 0.2];
    project.library.footprints[0].definition.datum_offset = [0.3, -0.4];
    for (placement, (position, layer)) in project
        .placements
        .iter_mut()
        .zip([([10.0, 12.0], Layer::Top), ([30.0, 12.0], Layer::Bottom)])
    {
        placement.position = position;
        placement.rotation_deg = 90.0;
        placement.layer = layer;
    }
    let shared = project
        .net_of(Endpoint {
            instance: instances(&project)?.0,
            terminal: TerminalId(2),
        })
        .map(|net| net.id)
        .ok_or_else(|| Error::Verification("demo has no shared net".into()))?;
    for (y, layer) in [(25.0, Layer::Top), (28.0, Layer::Bottom)] {
        let id = project
            .allocate()
            .map_err(|error| Error::Verification(error.to_string()))?;
        project.tracks.push(Track {
            id,
            net: shared,
            start: [5.0, y],
            end: [15.0, y],
            width: 0.4,
            layer,
            locked: false,
        });
    }
    Ok(project)
}

/// The one body of `kind` that belongs to the matching source.
fn source_geometry(
    model: &NativeModel,
    kind: BodyKind,
    matches: impl Fn(&Source) -> bool,
) -> Result<&Geometry> {
    let source = model
        .sources
        .iter()
        .find(|source| matches(source))
        .ok_or_else(|| Error::Verification("missing source identity".into()))?;
    let mut found = model
        .geometry
        .iter()
        .filter(|g| g.item == Some(source.pick_id) && g.kind == kind);
    let geometry = found
        .next()
        .ok_or_else(|| Error::Verification(format!("source has no {kind:?} geometry")))?;
    if found.next().is_some() {
        return Err(Error::Verification(format!(
            "source has several {kind:?} bodies"
        )));
    }
    Ok(geometry)
}

fn native(model: &NativeModel) -> impl Iterator<Item = &Geometry> {
    model.geometry.iter().filter(|g| !g.kind.is_appearance())
}

fn count(model: &NativeModel, kind: BodyKind) -> usize {
    model.geometry.iter().filter(|g| g.kind == kind).count()
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
    let (r1, r2) = instances(&project)?;
    let model = build(&project)?;
    // Native: the substrate, per placement a chip-resistor envelope, per
    // pad its copper and per trace its copper. Display: two mask slabs, per
    // chip two wrapped terminations and an overcoat, per pad its finish and
    // per free trace one mask cap.
    assert_eq!(native(&model).count(), 9);
    for (kind, n) in [
        (BodyKind::Substrate, 1),
        (BodyKind::MaskTop, 1),
        (BodyKind::MaskBottom, 1),
        (BodyKind::ChipResistor, 2),
        (BodyKind::PackageBody, 0),
        (BodyKind::Termination, 4),
        (BodyKind::Overcoat, 2),
        (BodyKind::PadCopper, 4),
        (BodyKind::PadFinish, 4),
        (BodyKind::CopperTrack, 2),
        (BodyKind::MaskCap, 2),
    ] {
        assert_eq!(count(&model, kind), n, "{kind:?}");
    }
    assert_eq!(model.sources.len(), 8);
    let substrate = model
        .geometry
        .iter()
        .find(|geometry| geometry.kind == BodyKind::Substrate)
        .ok_or_else(|| Error::Verification("missing substrate".into()))?;
    assert_eq!(substrate.item, None);
    bounds(substrate, [0.0, 0.0, 0.0], [60.0, 40.0, 1.6]);
    near(substrate.volume, 60.0 * 40.0 * 1.6);
    let top = source_geometry(&model, BodyKind::ChipResistor, |s| {
        s.instance == Some(r1) && s.pad.is_none()
    })?;
    bounds(top, [9.8, 11.25, 1.835], [11.0, 13.25, 2.435]);
    near(top.volume, 2.0 * 1.2 * 0.6);
    let bottom = source_geometry(&model, BodyKind::ChipResistor, |s| {
        s.instance == Some(r2) && s.pad.is_none()
    })?;
    bounds(bottom, [29.0, 11.25, -0.835], [30.2, 13.25, -0.235]);
    for (instance, center, z) in [
        (r1, [9.6, 10.5], [1.6, 1.635]),
        (r2, [30.4, 10.5], [-0.035, 0.0]),
    ] {
        let pad = source_geometry(&model, BodyKind::PadCopper, |s| {
            s.instance == Some(instance) && s.pad == Some(PadId(1))
        })?;
        // A 1 × 1.4 mm land rotated 90° has a 1.4 × 1 mm world footprint.
        bounds(
            pad,
            [center[0] - 0.7, center[1] - 0.5, z[0]],
            [center[0] + 0.7, center[1] + 0.5, z[1]],
        );
        near(pad.volume, 1.0 * 1.4 * 0.035);
    }
    for track in &project.tracks {
        let geometry =
            source_geometry(&model, BodyKind::CopperTrack, |s| s.track == Some(track.id))?;
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
        assert_eq!(source.net, Some(track.net));
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
        let geometry = source_geometry(&model, BodyKind::CopperTrack, |source| {
            source.track == Some(track.id)
        })?;
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
    let project = fixture()?;
    let (r1, r2) = instances(&project)?;
    let original = build(&project)?;
    let temporary = tempfile::tempdir()?;
    let path = temporary
        .path()
        .join("project.solidcraft")
        .to_string_lossy()
        .into_owned();
    original.save(&path)?;
    let reopened = Native::open(&path)?;
    let metadata = serde_json::to_value(&reopened.session().doc.metadata)?;
    let retained: Vec<Source> =
        serde_json::from_value(metadata["org.pcbcraft.electronics"]["sources"].clone())?;
    assert_eq!(
        serde_json::to_value(&retained)?,
        serde_json::to_value(&original.sources)?
    );
    let before = finish(reopened, retained)?;
    // The file holds the native bodies only; display layers are not saved.
    assert_eq!(before.geometry.len(), native(&original).count());
    assert!(before.geometry.iter().all(|g| !g.kind.is_appearance()));
    for previous in native(&original) {
        let mut same = before
            .geometry
            .iter()
            .filter(|g| g.item == previous.item && g.kind == previous.kind);
        let restored = same
            .next()
            .ok_or_else(|| Error::Verification("reopen lost source geometry".into()))?;
        assert!(same.next().is_none(), "{:?} is ambiguous", previous.kind);
        bounds(restored, previous.min, previous.max);
        near(restored.volume, previous.volume);
        assert_eq!(restored.surface, previous.surface);
    }
    looks_match_the_native_file(&before)?;
    let mut reopened = before.native;
    let mut changes = json!({"board_thickness": "2.4 mm", "copper_thickness": "0.07 mm"});
    for (name, value) in [
        (format!("instance_{r1}_x"), "15 mm"),
        (format!("instance_{r1}_y"), "18 mm"),
        (format!("instance_{r1}_angle"), "0 deg"),
        (format!("instance_{r1}_length"), "4 mm"),
        (format!("instance_{r2}_angle"), "180 deg"),
    ] {
        changes[name] = json!(value);
    }
    reopened.change_parameters(changes)?;
    let changed = finish(reopened, before.sources)?;
    let top = source_geometry(&changed, BodyKind::ChipResistor, |s| {
        s.instance == Some(r1) && s.pad.is_none()
    })?;
    bounds(top, [13.25, 17.0, 2.67], [17.25, 18.2, 3.27]);
    near(top.volume, 4.0 * 1.2 * 0.6);
    let bottom = source_geometry(&changed, BodyKind::ChipResistor, |s| {
        s.instance == Some(r2) && s.pad.is_none()
    })?;
    bounds(bottom, [28.75, 11.0, -0.87], [30.75, 12.2, -0.27]);
    let pad1 = Some(PadId(1));
    let top_pad = source_geometry(&changed, BodyKind::PadCopper, |s| {
        s.instance == Some(r1) && s.pad == pad1
    })?;
    bounds(top_pad, [13.0, 17.7, 2.4], [14.0, 19.1, 2.47]);
    let bottom_pad = source_geometry(&changed, BodyKind::PadCopper, |s| {
        s.instance == Some(r2) && s.pad == pad1
    })?;
    bounds(bottom_pad, [31.0, 11.7, -0.07], [32.0, 13.1, 0.0]);
    for source in changed.sources.iter().filter(|s| s.track.is_some()) {
        let geometry = source_geometry(&changed, BodyKind::CopperTrack, |s| {
            s.pick_id == source.pick_id
        })?;
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
    let mut package = Engine::demo()
        .map_err(|error| Error::Verification(error.to_string()))?
        .project()
        .library
        .packages[0]
        .definition
        .clone();
    package.body_size = [4.0, 3.0, 2.0];
    package.model_offset = [1.5, -2.0, 0.4];
    package.body = PackageBody::Moulded;
    let model = preview(&package)?;
    assert_eq!(model.geometry.len(), 1);
    assert_eq!(model.geometry[0].kind, BodyKind::PartBody);
    looks_match_the_native_file(&model)?;
    bounds(&model.geometry[0], [-0.5, -3.5, 0.4], [3.5, -0.5, 2.4]);
    near(model.geometry[0].volume, 24.0);
    // A package declaring a chip-resistor body previews as one, with the
    // wrapped terminations and overcoat it shows on the board.
    package.body_size = [2.0, 1.25, 0.5];
    package.model_offset = [0.0; 3];
    package.body = PackageBody::ChipResistor;
    let chip = preview(&package)?;
    assert_eq!(chip.geometry[0].kind, BodyKind::ChipResistor);
    assert_eq!(count(&chip, BodyKind::Termination), 2);
    assert_eq!(count(&chip, BodyKind::Overcoat), 1);
    looks_match_the_native_file(&chip)?;
    for end in chip
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::Termination)
    {
        // 0.4 mm bands (a fifth of 2 mm), 10 µm proud of the body.
        near(end.max[0] - end.min[0], 0.41);
        bounds(end, [end.min[0], -0.635, 0.0], [end.max[0], 0.635, 0.51]);
        assert!(end.min[0] <= -1.01 + 1e-9 || end.max[0] >= 1.01 - 1e-9);
    }
    // A chip-capacitor body: MLCC ceramic, terminations, no overcoat.
    package.body = PackageBody::ChipCapacitor;
    let ceramic = preview(&package)?;
    assert_eq!(ceramic.geometry[0].kind, BodyKind::ChipCapacitor);
    assert_eq!(count(&ceramic, BodyKind::Termination), 2);
    assert_eq!(count(&ceramic, BodyKind::Overcoat), 0);
    looks_match_the_native_file(&ceramic)?;
    package.body_size[0] = 0.0;
    assert!(matches!(preview(&package), Err(Error::Invalid(_))));
    Ok(())
}

/// The body look comes from the package's declared body, never from the
/// designator: renaming R1 to C1 keeps its chip-resistor body, and giving
/// the shared package a chip-capacitor body turns both into capacitors
/// while they keep their R designators.
#[test]
fn placed_body_follows_the_package_body_not_the_designator() -> Result<()> {
    let mut project = fixture()?;
    let (r1, _) = instances(&project)?;
    for instance in &mut project.instances {
        if instance.id == r1 {
            instance.designator = "C1".into();
        }
    }
    let model = build(&project)?;
    assert_eq!(count(&model, BodyKind::ChipResistor), 2);
    assert_eq!(count(&model, BodyKind::ChipCapacitor), 0);
    assert_eq!(count(&model, BodyKind::Overcoat), 2);
    let mut project = fixture()?;
    project.library.packages[0].definition.body = PackageBody::ChipCapacitor;
    let model = build(&project)?;
    assert!(project.instance_by_designator("R1").is_some());
    assert_eq!(count(&model, BodyKind::ChipCapacitor), 2);
    assert_eq!(count(&model, BodyKind::ChipResistor), 0);
    assert_eq!(count(&model, BodyKind::Overcoat), 0);
    assert_eq!(count(&model, BodyKind::Termination), 4);
    project.library.packages[0].definition.body = PackageBody::Moulded;
    let model = build(&project)?;
    assert_eq!(count(&model, BodyKind::PackageBody), 2);
    assert_eq!(count(&model, BodyKind::ChipCapacitor), 0);
    assert_eq!(count(&model, BodyKind::Overcoat), 0);
    Ok(())
}

/// Build timings for docs/editor-evidence.md:
/// `cargo test --profile ci -p pcbcraft-solidcraft build_timings -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn build_timings() -> Result<()> {
    let demo = Engine::demo()
        .map_err(|error| Error::Verification(error.to_string()))?
        .project()
        .clone();
    let mut tracks = demo.clone();
    let net = tracks.nets[0].id;
    for k in 0..400 {
        let id = tracks
            .allocate()
            .map_err(|error| Error::Verification(error.to_string()))?;
        let y = 12.0 + f64::from(k % 100) * 0.27;
        let x = 2.0 + f64::from(k / 100) * 14.0;
        tracks.tracks.push(Track {
            id,
            net,
            start: [x, y],
            end: [x + 12.0, y],
            width: 0.15,
            layer: if k % 2 == 0 {
                Layer::Top
            } else {
                Layer::Bottom
            },
            locked: false,
        });
    }
    for (name, project, runs) in [("demo", &demo, 15), ("400 tracks", &tracks, 3)] {
        let mut times = vec![];
        let mut bodies = (0, 0);
        for _ in 0..runs {
            let start = std::time::Instant::now();
            let model = build(project)?;
            times.push(start.elapsed().as_secs_f64() * 1e3);
            bodies = (native(&model).count(), model.geometry.len());
        }
        times.sort_by(f64::total_cmp);
        let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
        println!(
            "{name}: {} native bodies, {} drawn, build p50 {:.0} ms, p95 {:.0} ms ({runs} runs)",
            bodies.0,
            bodies.1,
            at(0.5),
            at(0.95)
        );
    }
    Ok(())
}

/// Every body's surface is exactly the look the native file stores, as
/// Solidcraft's own product-scene extraction reads it.
fn looks_match_the_native_file(model: &NativeModel) -> Result<()> {
    let scene = solidcraft_engine::product_scene::extract(model.native.session())
        .map_err(|e| Error::Verification(e.to_string()))?;
    assert_eq!(scene.meshes.len(), native(model).count());
    for (mesh, geometry) in scene.meshes.iter().zip(native(model)) {
        assert_eq!(mesh.surface, Some(geometry.surface), "{}", mesh.name);
        let expected = material::material(geometry.kind, DEFAULT_FINISH);
        assert_eq!(mesh.color, expected.srgb8(), "{}", mesh.name);
    }
    Ok(())
}

fn top_z(model: &NativeModel, kind: BodyKind) -> Result<f64> {
    model
        .geometry
        .iter()
        .find(|g| g.kind == kind)
        .map(|g| g.max[2])
        .ok_or_else(|| Error::Verification(format!("no {kind:?}")))
}

/// The first board-level or copper body a vertical ray meets (parts and
/// their solder are left out: they may cover a land).
fn first_hit(model: &NativeModel, at: [f64; 2], down: bool) -> Option<&Geometry> {
    let (z, dz) = if down { (10.0, -1.0) } else { (-10.0, 1.0) };
    model
        .geometry
        .iter()
        .filter(|g| {
            g.kind.is_board()
                || matches!(
                    g.kind,
                    BodyKind::CopperTrack | BodyKind::PadCopper | BodyKind::PadFinish
                )
        })
        .filter_map(|g| {
            g.mesh
                .raycast(Vec3::new(at[0], at[1], z), Vec3::new(0.0, 0.0, dz))
                .map(|(t, _)| (t, g))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, g)| g)
}

#[test]
fn bodies_carry_kinds_and_the_looks_stored_in_the_native_file() -> Result<()> {
    let model = build(&fixture()?)?;
    looks_match_the_native_file(&model)?;
    for geometry in &model.geometry {
        let board = geometry.kind.is_board() && geometry.kind != BodyKind::MaskCap;
        assert_eq!(geometry.item.is_none(), board, "{:?}", geometry.kind);
    }
    let copper = material::material(BodyKind::PadCopper, DEFAULT_FINISH).surface()?;
    for source in model.sources.iter().filter(|s| s.pad.is_some()) {
        let pad = source_geometry(&model, BodyKind::PadCopper, |s| s.pick_id == source.pick_id)?;
        let finish = source_geometry(&model, BodyKind::PadFinish, |s| s.pick_id == source.pick_id)?;
        assert_eq!(pad.surface.metallic(), copper.metallic());
        near(pad.volume, 1.0 * 1.4 * 0.035);
        for axis in 0..2 {
            near(finish.min[axis], pad.min[axis] - 0.005);
            near(finish.max[axis], pad.max[axis] + 0.005);
        }
        if source.layer == Some(Layer::Top) {
            near(finish.min[2], pad.min[2]);
            near(finish.max[2], pad.max[2] + 0.005);
        } else {
            near(finish.min[2], pad.min[2] - 0.005);
            near(finish.max[2], pad.max[2]);
        }
    }
    for source in model.sources.iter().filter(|s| s.track.is_some()) {
        let cap = source_geometry(&model, BodyKind::MaskCap, |s| s.pick_id == source.pick_id)?;
        let track = source_geometry(&model, BodyKind::CopperTrack, |s| {
            s.pick_id == source.pick_id
        })?;
        // Free ends are round: the cap overhangs the copper by its margin
        // along the sides and by half its width at the ends.
        near(cap.min[0], track.min[0] - 0.21);
        near(cap.max[0], track.max[0] + 0.21);
        near(cap.min[1], track.min[1] - 0.01);
        near(cap.max[1], track.max[1] + 0.01);
    }
    Ok(())
}

#[test]
fn exposed_pads_stand_above_the_mask_and_tracks_are_capped() -> Result<()> {
    let project = fixture()?;
    let model = build(&project)?;
    let mask_top = top_z(&model, BodyKind::MaskTop)?;
    let mask_bottom = model
        .geometry
        .iter()
        .find(|g| g.kind == BodyKind::MaskBottom)
        .map(|g| g.min[2])
        .ok_or_else(|| Error::Verification("no bottom mask".into()))?;
    near(mask_top, 1.62);
    near(mask_bottom, -0.02);
    for source in model.sources.iter().filter(|s| s.pad.is_some()) {
        let pad = source_geometry(&model, BodyKind::PadCopper, |s| s.pick_id == source.pick_id)?;
        let center = [
            (pad.min[0] + pad.max[0]) / 2.0,
            (pad.min[1] + pad.max[1]) / 2.0,
        ];
        let top = source.layer == Some(Layer::Top);
        let hit = first_hit(&model, center, top)
            .ok_or_else(|| Error::Verification("ray missed the pad".into()))?;
        assert_eq!(
            (hit.kind, hit.item),
            (BodyKind::PadFinish, Some(source.pick_id))
        );
        if top {
            assert!(hit.max[2] - mask_top >= 0.010 - 1e-9, "{}", hit.max[2]);
        } else {
            assert!(mask_bottom - hit.min[2] >= 0.010 - 1e-9, "{}", hit.min[2]);
        }
    }
    for track in &project.tracks {
        let source = model
            .sources
            .iter()
            .find(|s| s.track == Some(track.id))
            .ok_or_else(|| Error::Verification("missing trace source".into()))?;
        let middle = [
            (track.start[0] + track.end[0]) / 2.0,
            (track.start[1] + track.end[1]) / 2.0,
        ];
        let hit = first_hit(&model, middle, track.layer == Layer::Top)
            .ok_or_else(|| Error::Verification("ray missed the track".into()))?;
        assert_eq!(
            (hit.kind, hit.item),
            (BodyKind::MaskCap, Some(source.pick_id))
        );
    }
    Ok(())
}

/// The top-side pad finishes (not the finish over track copper).
fn pad_finishes(model: &NativeModel) -> Vec<&Geometry> {
    model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::PadFinish && g.max[2] > 1.0)
        .filter(|g| {
            model
                .sources
                .iter()
                .any(|s| Some(s.pick_id) == g.item && s.pad.is_some())
        })
        .collect()
}

/// A track's display pieces of `kind`.
fn pieces(model: &NativeModel, track: Id, kind: BodyKind) -> Vec<&Geometry> {
    let Some(source) = model.sources.iter().find(|s| s.track == Some(track)) else {
        return vec![];
    };
    model
        .geometry
        .iter()
        .filter(|g| g.item == Some(source.pick_id) && g.kind == kind)
        .collect()
}

fn no_cap_over_a_pad(model: &NativeModel, caps: &[&Geometry]) {
    for cap in caps {
        for p in &cap.mesh.positions {
            for f in pad_finishes(model) {
                let over = p.x > f.min[0] + 1e-6
                    && p.x < f.max[0] - 1e-6
                    && p.y > f.min[1] + 1e-6
                    && p.y < f.max[1] - 1e-6;
                assert!(!over, "cap vertex {p:?} over a pad finish");
            }
        }
    }
}

#[test]
fn mask_caps_stop_at_mask_openings_and_exposed_track_copper_is_plated() -> Result<()> {
    let mut project = fixture()?;
    let (r1, _) = instances(&project)?;
    let net = project.tracks[0].net;
    let pad2 = layout::pads(&project)
        .into_iter()
        .find(|p| p.instance == r1 && p.pad == PadId(2))
        .ok_or_else(|| Error::Verification("missing R1 pad 2".into()))?;
    let c = pad2.position;
    let mut add = |start: Point, end: Point| -> Result<Id> {
        let id = project
            .allocate()
            .map_err(|error| Error::Verification(error.to_string()))?;
        project.tracks.push(Track {
            id,
            net,
            start,
            end,
            width: 0.3,
            layer: Layer::Top,
            locked: false,
        });
        Ok(id)
    };
    let straight = add(c, [c[0], c[1] + 5.0])?;
    let diagonal = add(c, [c[0] - 4.0, c[1] + 3.0])?;
    let inside = add([c[0] - 0.2, c[1]], [c[0] + 0.2, c[1]])?;
    // Across the land's 1.4 mm world width, clear of the part's other land.
    let through = add([c[0] - 4.0, c[1] + 0.1], [c[0] + 4.0, c[1] + 0.1])?;
    let model = build(&project)?;
    assert!(
        pieces(&model, inside, BodyKind::MaskCap).is_empty(),
        "a track inside its pad has no cap"
    );
    for track in [straight, diagonal] {
        let caps = pieces(&model, track, BodyKind::MaskCap);
        assert_eq!(caps.len(), 1);
        no_cap_over_a_pad(&model, &caps);
    }
    // Rotated 90°, the 1 × 1.4 mm land is 1 mm deep along +Y. The straight
    // cap starts at the mask opening, 0.5 + 0.05 mm from the centre, and the
    // copper between the land and the opening is plated like the land.
    let straight_cap = pieces(&model, straight, BodyKind::MaskCap);
    near(straight_cap[0].min[1], c[1] + 0.55);
    let strip = pieces(&model, straight, BodyKind::PadFinish);
    assert_eq!(strip.len(), 1);
    near(strip[0].min[1], c[1]);
    near(strip[0].max[1], c[1] + 0.55);
    near(strip[0].max[2], 1.6 + 0.035 + 0.004);
    // A track passing through the land: capped on both sides, plated across.
    let caps = pieces(&model, through, BodyKind::MaskCap);
    assert_eq!(caps.len(), 2, "pass-through cap is split at the opening");
    no_cap_over_a_pad(&model, &caps);
    let mut ends: Vec<f64> = caps.iter().flat_map(|g| [g.min[0], g.max[0]]).collect();
    ends.sort_by(f64::total_cmp);
    near(ends[1], c[0] - 0.75);
    near(ends[2], c[0] + 0.75);
    let strip = pieces(&model, through, BodyKind::PadFinish);
    assert_eq!(strip.len(), 1);
    near(strip[0].min[0], c[0] - 0.75);
    near(strip[0].max[0], c[0] + 0.75);
    // Every finish stands above the mask and below any cap top.
    for f in pad_finishes(&model) {
        near(f.max[2], 1.6 + 0.035 + 0.005);
    }
    Ok(())
}

#[test]
fn native_bodies_are_disjoint_except_where_copper_joins_copper() -> Result<()> {
    let mut project = fixture()?;
    let (r1, _) = instances(&project)?;
    let pad = layout::pads(&project)
        .into_iter()
        .find(|p| p.instance == r1)
        .ok_or_else(|| Error::Verification("missing R1 pad".into()))?;
    let id = project
        .allocate()
        .map_err(|error| Error::Verification(error.to_string()))?;
    project.tracks.push(Track {
        id,
        net: pad.net,
        start: pad.position,
        end: [pad.position[0] + 6.0, pad.position[1]],
        width: 0.25,
        layer: Layer::Top,
        locked: false,
    });
    let model = build(&project)?;
    let bodies: Vec<&Geometry> = native(&model).collect();
    let copper = |g: &Geometry| matches!(g.kind, BodyKind::CopperTrack | BodyKind::PadCopper);
    for (i, a) in bodies.iter().enumerate() {
        for b in &bodies[i + 1..] {
            if copper(a) && copper(b) {
                continue;
            }
            // Every body here is a box lying in its own z band, so a shared
            // interior needs overlapping axis-aligned boxes.
            let overlap: f64 = (0..3)
                .map(|k| (a.max[k].min(b.max[k]) - a.min[k].max(b.min[k])).max(0.0))
                .product();
            assert!(
                overlap < 1e-9,
                "{:?} and {:?} overlap by {overlap} mm³",
                a.kind,
                b.kind
            );
        }
    }
    // The saved model's volume is the laminate, copper and envelopes only.
    let volume: f64 = bodies.iter().map(|g| g.volume).sum();
    let expected = 60.0 * 40.0 * 1.6
        + 2.0 * (2.0 * 1.2 * 0.6)
        + 4.0 * (1.0 * 1.4 * 0.035)
        + (2.0 * 10.0 * 0.4 + 6.0 * 0.25) * 0.035;
    near(volume, expected);
    Ok(())
}

#[test]
fn body_kind_chunks_measure_escaped_names() -> Result<()> {
    let mut bodies = BTreeMap::new();
    // Quotes and backslashes serialize at twice their length.
    for k in 0..3000 {
        let kind = BodyKind::ALL[k % BodyKind::ALL.len()];
        bodies.insert(format!("{k:05} \\\"{}", "\\".repeat(20)), kind);
    }
    let chunks = kind_chunks(&bodies)?;
    assert!(chunks.len() > 1);
    let mut names = 0;
    for chunk in &chunks {
        let bytes = serde_json::to_string(chunk)?.len();
        assert!(bytes <= BODIES_CHUNK_BYTES, "{bytes} bytes");
        names += chunk.values().map(Vec::len).sum::<usize>();
    }
    assert_eq!(names, bodies.len());
    bodies.insert("x".repeat(BODIES_CHUNK_BYTES), BodyKind::Substrate);
    assert!(matches!(kind_chunks(&bodies), Err(Error::Invalid(_))));
    Ok(())
}

#[test]
fn every_footprint_pad_is_enumerated_and_unconnected_ones_have_no_net() -> Result<()> {
    let mut project = fixture()?;
    let connected_before = layout::pads(&project).len();
    // Synthetic: a mechanical land whose package terminal carries no
    // electrical terminal, so it has no net on either placement.
    let unmapped = pcbcraft_core::PackageTerminalId(90);
    let package = &mut project.library.packages[0].definition;
    package
        .terminals
        .push(pcbcraft_core::library::PackageTerminal {
            id: unmapped,
            designation: "MP".into(),
        });
    package.next_local = 91;
    let footprint = &mut project.library.footprints[0].definition;
    footprint.pads.push(pcbcraft_core::library::Pad {
        id: PadId(90),
        terminal: unmapped,
        position: [0.0, 3.0],
        size: [0.8, 0.8],
    });
    footprint.next_local = 91;
    // Library validation refuses such a definition today, so the
    // enumeration is checked directly; the core's pad list is unchanged.
    let connected = layout::pads(&project);
    assert_eq!(connected.len(), connected_before);
    let pads = placed_pads(&project)?;
    assert_eq!(pads.len(), 6);
    for pad in &pads {
        let expected = connected
            .iter()
            .find(|p| p.instance == pad.instance && p.pad == pad.pad.id)
            .map(|p| (p.net, p.position));
        assert_eq!(pad.net.is_none(), pad.pad.terminal == unmapped);
        if let Some((net, position)) = expected {
            assert_eq!(pad.net, Some(net));
            assert_eq!(pad.center, position);
        }
    }
    Ok(())
}

#[test]
fn chip_terminations_wrap_the_ends_and_solder_wets_the_lands_on_both_sides() -> Result<()> {
    // The demo's centred chips get a fillet at every land; the fixture's
    // offset bodies leave one end hanging off its land, which gets none.
    let demo = build(
        Engine::demo()
            .map_err(|error| Error::Verification(error.to_string()))?
            .project(),
    )?;
    assert_eq!(count(&demo, BodyKind::Solder), 4);
    assert!(count(&build(&fixture()?)?, BodyKind::Solder) >= 1);
    check_chip_parts(&demo)?;
    check_chip_parts(&build(&fixture()?)?)?;
    Ok(())
}

fn check_chip_parts(model: &NativeModel) -> Result<()> {
    let envelopes = model
        .sources
        .iter()
        .filter(|s| s.pad.is_none() && s.instance.is_some());
    for source in envelopes {
        let body = source_geometry(model, BodyKind::ChipResistor, |s| {
            s.pick_id == source.pick_id
        })?;
        let of = |kind| {
            model
                .geometry
                .iter()
                .filter(move |g| g.item == Some(source.pick_id) && g.kind == kind)
                .collect::<Vec<_>>()
        };
        let ends = of(BodyKind::Termination);
        assert_eq!(ends.len(), 2, "{}", source.label);
        let top = source.layer == Some(Layer::Top);
        for end in &ends {
            // Each cap wraps 10 µm past the body's top and underside (the
            // fixture lifts the body 0.2 mm), its sides and one end face.
            near(end.max[2], body.max[2] + 0.01);
            near(end.min[2], body.min[2] - 0.01);
            let outside = (0..2).any(|a| {
                (end.min[a] - (body.min[a] - 0.01)).abs() < 1e-9
                    || (end.max[a] - (body.max[a] + 0.01)).abs() < 1e-9
            });
            assert!(outside, "{} termination misses an end face", source.label);
        }
        let overcoat = source_geometry(model, BodyKind::Overcoat, |s| s.pick_id == source.pick_id)?;
        assert!(overcoat.max[2] > body.max[2] || overcoat.min[2] < body.min[2]);
        let lands: Vec<&Geometry> = model
            .sources
            .iter()
            .filter(|s| s.instance == source.instance && s.pad.is_some())
            .filter_map(|s| {
                source_geometry(model, BodyKind::PadCopper, |x| x.pick_id == s.pick_id).ok()
            })
            .collect();
        for solder in of(BodyKind::Solder) {
            let on_land = lands.iter().any(|pad| {
                (0..2).all(|a| {
                    solder.min[a] >= pad.min[a] - 1e-6 && solder.max[a] <= pad.max[a] + 1e-6
                })
            });
            assert!(on_land, "{} fillet off its land", source.label);
            let touches = ends.iter().any(|end| {
                (0..2).any(|a| {
                    (solder.min[a] - end.max[a]).abs() < 1e-6
                        || (solder.max[a] - end.min[a]).abs() < 1e-6
                })
            });
            assert!(
                touches,
                "{} fillet does not meet its termination",
                source.label
            );
            if top {
                near(solder.min[2], 1.6 + 0.035 + 0.005);
            } else {
                near(solder.max[2], -0.035 - 0.005);
            }
            assert!(solder.volume > 0.0);
        }
    }
    Ok(())
}
