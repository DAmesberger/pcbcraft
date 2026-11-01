use super::*;
use pcbcraft_core::geometry::cos_sin;
use pcbcraft_core::library::{ChipForm, ChipKind, PackageDefinition, PackageForm, PackageModel};

/// Give `package` a chip form of `kind` over its envelope, or a generic
/// (moulded) one, with an envelope model: these tests exercise the
/// envelope path (generated models are `models::tests`).
fn set_body(package: &mut PackageDefinition, kind: Option<ChipKind>) {
    package.form = kind.map_or(PackageForm::Generic, |kind| {
        PackageForm::Chip(Box::new(ChipForm::new(kind, package.envelope)))
    });
    package.model = PackageModel::default();
}

/// The demo with envelope models instead of its generated chip models,
/// lifted 0.05 mm off the lands as the samples lift an envelope.
fn envelope_demo() -> Result<Project> {
    let mut project = Engine::demo()
        .map_err(|error| Error::Verification(error.to_string()))?
        .project()
        .clone();
    for package in &mut project.library.packages {
        package.definition.model = PackageModel::default();
        package.definition.placement.translation = [0.0, 0.0, 0.05];
    }
    Ok(project)
}
use pcbcraft_core::library::PadShape;
use pcbcraft_core::{Endpoint, Engine, TerminalId, Track};
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
    let mut project = envelope_demo()?;
    // Asymmetric offsets exercise the rigid bottom flip independently of the
    // symmetric body/pad shapes. This synthetic test replaces a snapshot to
    // exercise the adapter; production changes publish a new immutable revision.
    project.library.packages[0].definition.placement.translation = [0.25, -0.4, 0.2];
    project.library.footprints[0].definition.datum.offset = [0.3, -0.4];
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
            layer: layer.into(),
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

/// A track's copper outline in plan: its box (min, max) and area. Round
/// end caps of `TRACK_CAP_SEGMENTS` chords each.
fn track_plan(start: Point, end: Point, width: f64) -> ([f64; 2], [f64; 2], f64) {
    let outline = track_outline(start, end, width);
    let n = outline.len();
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for p in &outline {
        for k in 0..2 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let area = (0..n)
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0;
    (min, max, area)
}

#[test]
fn track_outlines_are_stadiums_with_round_caps() {
    // Axis-aligned: the caps reach half the width past each end.
    let (min, max, area) = track_plan([5.0, 10.0], [15.0, 10.0], 0.4);
    for (a, b) in min.iter().zip([4.8, 9.8]) {
        near(*a, b);
    }
    for (a, b) in max.iter().zip([15.2, 10.2]) {
        near(*a, b);
    }
    // The rectangle and a 16-gon inscribed in the cap circle.
    let disc = 8.0 * 0.2 * 0.2 * (std::f64::consts::PI / 8.0).sin();
    near(area, 10.0 * 0.4 + disc);
}

#[test]
fn native_project_has_analytic_board_copper_and_rigid_top_bottom_placements() -> Result<()> {
    let project = fixture()?;
    let (r1, r2) = instances(&project)?;
    let model = build(&project)?;
    // Native: the substrate, per placement a chip-resistor envelope, per
    // pad its copper and per trace its copper. Display: two mask slabs, per
    // chip two wrapped terminations and an overcoat, per pad its finish and
    // per free trace one mask cap, and at the routed edge a mask lip per
    // side over a line per copper foil and the core's band.
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
        (BodyKind::MaskCap, 2 + 2),
        (BodyKind::EdgeCopper, 2),
        (BodyKind::EdgeDielectric, 1),
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
        (r1, [9.6, 10.9], [1.6, 1.635]),
        (r2, [30.4, 10.9], [-0.035, 0.0]),
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
        // Round caps reach half the width past both ends.
        bounds(
            geometry,
            [4.8, track.start[1] - 0.2, z[0]],
            [15.2, track.start[1] + 0.2, z[1]],
        );
        let (_, _, area) = track_plan(track.start, track.end, track.width);
        near(geometry.volume, area * 0.035);
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
fn diagonal_traces_have_round_caps_on_both_copper_layers() -> Result<()> {
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
        // The 4:3 direction has length 5; the round caps' chords reach
        // within 2 % of half the width past each end, in every direction.
        let z = if track.layer == Layer::Top {
            [1.6, 1.635]
        } else {
            [-0.035, 0.0]
        };
        let (min, max, area) = track_plan(track.start, track.end, track.width);
        for k in 0..2 {
            assert!(min[k] < [5.0, 25.0][k] - 0.19 && max[k] > [9.0, 28.0][k] + 0.19);
        }
        bounds(geometry, [min[0], min[1], z[0]], [max[0], max[1], z[1]]);
        near(geometry.volume, area * 0.035);
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
    bounds(top_pad, [13.4, 17.7, 2.4], [14.4, 19.1, 2.47]);
    let bottom_pad = source_geometry(&changed, BodyKind::PadCopper, |s| {
        s.instance == Some(r2) && s.pad == pad1
    })?;
    bounds(bottom_pad, [30.6, 11.7, -0.07], [31.6, 13.1, 0.0]);
    for source in changed.sources.iter().filter(|s| s.track.is_some()) {
        let geometry = source_geometry(&changed, BodyKind::CopperTrack, |s| {
            s.pick_id == source.pick_id
        })?;
        let (_, _, area) = track_plan([5.0, 0.0], [15.0, 0.0], 0.4);
        near(geometry.volume, area * 0.07);
        near(
            geometry.min[2],
            if source.layer == Some(CopperLayer::Top) {
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
    package.envelope = [4.0, 3.0, 2.0];
    package.placement.translation = [1.5, -2.0, 0.4];
    set_body(&mut package, None);
    let model = preview(&package)?;
    assert_eq!(model.geometry.len(), 1);
    assert_eq!(model.geometry[0].kind, BodyKind::PartBody);
    looks_match_the_native_file(&model)?;
    bounds(&model.geometry[0], [-0.5, -3.5, 0.4], [3.5, -0.5, 2.4]);
    near(model.geometry[0].volume, 24.0);
    // A package declaring a chip-resistor body previews as one, with the
    // wrapped terminations and overcoat it shows on the board.
    package.envelope = [2.0, 1.25, 0.5];
    package.placement.translation = [0.0; 3];
    set_body(&mut package, Some(ChipKind::Resistor));
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
    set_body(&mut package, Some(ChipKind::Capacitor));
    let ceramic = preview(&package)?;
    assert_eq!(ceramic.geometry[0].kind, BodyKind::ChipCapacitor);
    assert_eq!(count(&ceramic, BodyKind::Termination), 2);
    assert_eq!(count(&ceramic, BodyKind::Overcoat), 0);
    looks_match_the_native_file(&ceramic)?;
    package.envelope[0] = 0.0;
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
    set_body(
        &mut project.library.packages[0].definition,
        Some(ChipKind::Capacitor),
    );
    let model = build(&project)?;
    assert!(project.instance_by_designator("R1").is_some());
    assert_eq!(count(&model, BodyKind::ChipCapacitor), 2);
    assert_eq!(count(&model, BodyKind::ChipResistor), 0);
    assert_eq!(count(&model, BodyKind::Overcoat), 0);
    assert_eq!(count(&model, BodyKind::Termination), 4);
    set_body(&mut project.library.packages[0].definition, None);
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
                CopperLayer::Top
            } else {
                CopperLayer::Bottom
            },
            locked: false,
        });
    }
    let showcase = Engine::showcase()
        .map_err(|error| Error::Verification(error.to_string()))?
        .project()
        .clone();
    // The demo on 4 layers with 50 through vias and 25 inner tracks. (200
    // vias exceed Solidcraft's planar comparison budget cutting the
    // laminate: see docs/pcb-realism.md, "Open".)
    let mut vias = demo.clone();
    vias.stackup = Stackup::preset(4).map_err(|error| Error::Verification(error.to_string()))?;
    for k in 0..50 {
        let id = vias
            .allocate()
            .map_err(|error| Error::Verification(error.to_string()))?;
        let position = [
            4.0 + f64::from(k % 20) * 2.5,
            22.0 + f64::from(k / 20) * 1.5,
        ];
        vias.vias.push(pcbcraft_core::Via {
            id,
            net,
            position,
            drill: 0.3,
            diameter: 0.6,
            span: pcbcraft_core::ViaSpan::Through,
            tented: k % 2 == 0,
            locked: false,
        });
        if k % 2 == 0 {
            let id = vias
                .allocate()
                .map_err(|error| Error::Verification(error.to_string()))?;
            vias.tracks.push(Track {
                id,
                net,
                start: position,
                end: [position[0] + 2.5, position[1]],
                width: 0.15,
                layer: CopperLayer::Inner(1 + u8::from(k % 4 == 0)),
                locked: false,
            });
        }
    }
    for (name, project, runs) in [
        ("demo", &demo, 15),
        ("400 tracks", &tracks, 3),
        ("showcase", &showcase, 5),
        ("4 layers, 50 vias, 25 inner tracks", &vias, 3),
    ] {
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
        let expected = material::material(geometry.kind, &Fabrication::default());
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
        // Board-level bodies and the edge's stackup bands have no pick
        // identity; neither do the mask lips at the edge (mask caps
        // standing proud of the routed outline). Everything else does.
        let lip = geometry.kind == BodyKind::MaskCap && geometry.min[0] < 0.0;
        let board = (geometry.kind.is_board() && geometry.kind != BodyKind::MaskCap)
            || matches!(
                geometry.kind,
                BodyKind::EdgeDielectric | BodyKind::EdgeCopper
            )
            || lip;
        assert_eq!(geometry.item.is_none(), board, "{:?}", geometry.kind);
    }
    let copper = material::material(BodyKind::PadCopper, &Fabrication::default()).surface()?;
    for source in model.sources.iter().filter(|s| s.pad.is_some()) {
        let pad = source_geometry(&model, BodyKind::PadCopper, |s| s.pick_id == source.pick_id)?;
        let finish = source_geometry(&model, BodyKind::PadFinish, |s| s.pick_id == source.pick_id)?;
        assert_eq!(pad.surface.metallic(), copper.metallic());
        near(pad.volume, 1.0 * 1.4 * 0.035);
        for axis in 0..2 {
            near(finish.min[axis], pad.min[axis] - 0.005);
            near(finish.max[axis], pad.max[axis] + 0.005);
        }
        if source.layer == Some(CopperLayer::Top) {
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
        // Free ends are round, like the copper's: the cap overhangs the
        // copper by its margin all round.
        near(cap.min[0], track.min[0] - 0.01);
        near(cap.max[0], track.max[0] + 0.01);
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
        let top = source.layer == Some(CopperLayer::Top);
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
            layer: CopperLayer::Top,
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
        layer: CopperLayer::Top,
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
        + (2.0 * track_plan([0.0, 0.0], [10.0, 0.0], 0.4).2
            + track_plan([0.0, 0.0], [6.0, 0.0], 0.25).2)
            * 0.035;
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
    footprint.pads.push(pcbcraft_core::library::Pad::smd_rect(
        PadId(90),
        unmapped,
        "MP",
        [0.0, 3.0],
        [0.8, 0.8],
    ));
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
            .find(|p| p.instance == pad.placed.instance && p.pad == pad.pad().id)
            .map(|p| (p.net, p.position));
        assert_eq!(
            pad.placed.net.is_none(),
            pad.pad().terminal == Some(unmapped)
        );
        if let Some((net, position)) = expected {
            assert_eq!(pad.placed.net, Some(net));
            near(pad.origin[0], position[0]);
            near(pad.origin[1], position[1]);
        }
    }
    Ok(())
}

#[test]
fn chip_terminations_wrap_the_ends_and_solder_wets_the_lands_on_both_sides() -> Result<()> {
    // The demo's centred chips get a fillet at every land; the fixture's
    // offset bodies leave one end hanging off its land, which gets none.
    let demo = build(&envelope_demo()?)?;
    assert_eq!(count(&demo, BodyKind::Solder), 4);
    assert!(count(&build(&fixture()?)?, BodyKind::Solder) >= 1);
    check_chip_parts(&demo)?;
    check_chip_parts(&build(&fixture()?)?)?;
    Ok(())
}

/// The demo's generated chips: a display-only fillet at every land, each
/// on its land and against the end face of a real termination body.
#[test]
fn generated_chips_get_solder_fillets_against_their_terminations() -> Result<()> {
    let demo = build(
        Engine::demo()
            .map_err(|error| Error::Verification(error.to_string()))?
            .project(),
    )?;
    assert!(!demo.models.is_empty(), "the demo chips are generated");
    let fillets = kind(&demo, BodyKind::Solder);
    assert_eq!(fillets.len(), 4);
    for solder in fillets {
        let land = demo.sources.iter().filter(|s| s.pad.is_some()).any(|s| {
            source_geometry(&demo, BodyKind::PadCopper, |x| x.pick_id == s.pick_id).is_ok_and(
                |pad| {
                    (0..2).all(|a| {
                        solder.min[a] >= pad.min[a] - 1e-6 && solder.max[a] <= pad.max[a] + 1e-6
                    })
                },
            )
        });
        assert!(land, "fillet off its land");
        let ends: Vec<&Geometry> = demo
            .geometry
            .iter()
            .filter(|g| g.item == solder.item && g.kind == BodyKind::ModelLead)
            .collect();
        assert_eq!(ends.len(), 2, "the model's two terminations");
        let touches = ends.iter().any(|end| {
            (0..2).any(|a| {
                (solder.min[a] - end.max[a]).abs() < 1e-6
                    || (solder.max[a] - end.min[a]).abs() < 1e-6
            })
        });
        assert!(touches, "fillet does not meet its termination");
        assert!(solder.volume > 0.0);
    }
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
        let top = source.layer == Some(CopperLayer::Top);
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

/// The project's fabrication attributes set the drawn mask and finish looks,
/// while the saved native bodies, their stored looks and every shape stay as
/// they are for the default fabrication.
#[test]
fn fabrication_restyles_display_layers_and_leaves_the_native_model_alone() -> Result<()> {
    use pcbcraft_core::{MaskColour, MaskFinish, PadFinish};
    let plain = build(&fixture()?)?;
    let mut project = fixture()?;
    project.fabrication = Fabrication {
        mask_colour: MaskColour::Red,
        mask_finish: MaskFinish::Matte,
        pad_finish: PadFinish::ImmersionSilver,
        ..Fabrication::default()
    };
    let styled = build(&project)?;
    looks_match_the_native_file(&styled)?;
    assert_eq!(plain.geometry.len(), styled.geometry.len());
    let mut restyled = 0;
    for (a, b) in plain.geometry.iter().zip(&styled.geometry) {
        assert_eq!((a.kind, a.item), (b.kind, b.item));
        assert_eq!((a.min, a.max), (b.min, b.max), "{:?}", a.kind);
        if b.kind.is_appearance() {
            // Drawn directly from the table; native bodies go through the
            // file's 8-bit colour, which looks_match_the_native_file checks.
            let expected = material::material(b.kind, &project.fabrication).surface()?;
            assert_eq!(b.surface, expected, "{:?}", b.kind);
        }
        if matches!(
            b.kind,
            BodyKind::MaskTop | BodyKind::MaskBottom | BodyKind::MaskCap | BodyKind::PadFinish
        ) {
            assert_ne!(a.surface, b.surface, "{:?}", b.kind);
            restyled += 1;
        } else {
            assert_eq!(a.surface, b.surface, "{:?}", b.kind);
        }
    }
    assert!(restyled >= 4, "{restyled}");
    Ok(())
}

/// The fixture with every kind of land on its one footprint (synthetic,
/// original): an L-shaped custom pad turned 30°, a plated-through stadium
/// turned 45° with an offset drill, a bare hole and a tented mechanical
/// land on the far side; the datum is turned a quarter turn.
fn shaped() -> Result<Project> {
    use pcbcraft_core::library::{Hole, HoleShape, MaskRule, Outline, PadKind, PadSide, Segment};
    let mut project = fixture()?;
    let footprint = &mut project.library.footprints[0].definition;
    footprint.datum.rotation_deg = 90.0;
    let [first, second] = &mut footprint.pads[..] else {
        return Err(Error::Verification("demo footprint has two pads".into()));
    };
    let l = |x: f64, y: f64| Segment::Line { to: [x, y] };
    first.shape = PadShape::Custom {
        outline: Outline {
            start: [-0.5, -0.4],
            segments: vec![
                l(0.5, -0.4),
                l(0.5, 0.0),
                l(0.0, 0.0),
                l(0.0, 0.4),
                l(-0.5, 0.4),
                l(-0.5, -0.4),
            ],
        },
    };
    first.rotation_deg = 30.0;
    second.kind = PadKind::PlatedThrough;
    second.shape = PadShape::Oblong { size: [1.6, 1.0] };
    second.rotation_deg = 45.0;
    second.hole = Some(Hole {
        shape: HoleShape::Round { diameter: 0.6 },
        plated: true,
        offset: [0.2, 0.0],
    });
    let mut bare = first.clone();
    bare.id = PadId(90);
    bare.kind = PadKind::NonPlatedHole;
    bare.terminal = None;
    bare.number = "MH".into();
    bare.position = [0.0, 2.5];
    bare.rotation_deg = 0.0;
    bare.shape = PadShape::Circle { diameter: 1.2 };
    bare.hole = Some(Hole {
        shape: HoleShape::Round { diameter: 1.0 },
        plated: false,
        offset: [0.0, 0.0],
    });
    let mut tab = bare.clone();
    tab.id = PadId(91);
    tab.kind = PadKind::Mechanical;
    tab.number = "MP".into();
    tab.position = [0.0, -2.5];
    tab.side = PadSide::Bottom;
    tab.shape = PadShape::RoundRect {
        size: [0.8, 0.5],
        radius_ratio: 0.25,
        max_radius: None,
        corners: pcbcraft_core::library::CornerMask::ALL,
    };
    tab.hole = None;
    tab.mask = MaskRule::Tented;
    footprint.pads.extend([bare, tab]);
    footprint.next_local = 92;
    Ok(project)
}

/// A pad-frame point of a placed pad on the board.
fn on_board(pad: &layout::PlacedPad, q: Point) -> Point {
    let (c, s) = cos_sin(pad.pad.rotation_deg);
    let p = pad.pad.position;
    pad.transform
        .apply([p[0] + c * q[0] - s * q[1], p[1] + s * q[0] + c * q[1]])
}

/// Mid-height of the copper on a layer's face.
fn copper_z(layer: Layer) -> f64 {
    match layer {
        Layer::Top => 1.6 + 0.0175,
        Layer::Bottom => -0.0175,
    }
}

fn inside(geometry: &[&Geometry], p: Point, z: f64) -> bool {
    geometry
        .iter()
        .any(|g| g.mesh.contains(Vec3::new(p[0], p[1], z)))
}

fn pad_bodies(model: &NativeModel, instance: Id, pad: PadId) -> Vec<&Geometry> {
    let Some(source) = model
        .sources
        .iter()
        .find(|s| s.instance == Some(instance) && s.pad == Some(pad))
    else {
        return vec![];
    };
    native(model)
        .filter(|g| g.item == Some(source.pick_id) && g.kind == BodyKind::PadCopper)
        .collect()
}

fn kind(model: &NativeModel, kind: BodyKind) -> Vec<&Geometry> {
    model.geometry.iter().filter(|g| g.kind == kind).collect()
}

#[test]
fn every_pad_shape_is_exact_native_copper_in_its_placed_frame() -> Result<()> {
    let project = shaped()?;
    let model = build(&project)?;
    let substrate = kind(&model, BodyKind::Substrate);
    let placed = layout::placed_pads(&project);
    assert_eq!(placed.len(), 8);
    for pad in &placed {
        let bodies = pad_bodies(&model, pad.instance, pad.pad.id);
        let other = match pad.layer {
            Layer::Top => Layer::Bottom,
            Layer::Bottom => Layer::Top,
        };
        match pad.pad.id.0 {
            // The L: both arms in, the notch out. A mirror or turn error
            // swaps them.
            1 => {
                assert_eq!(bodies.len(), 1);
                let z = copper_z(pad.layer);
                for (q, expected) in [
                    ([0.3, -0.2], true),
                    ([-0.3, 0.2], true),
                    ([0.3, 0.2], false),
                    ([-0.3, -0.35], true),
                    ([0.45, -0.35], true),
                    ([0.6, -0.2], false),
                ] {
                    let p = on_board(pad, q);
                    assert_eq!(
                        inside(&bodies, p, z),
                        expected,
                        "pad 1 {q:?} on {:?}",
                        pad.layer
                    );
                }
            }
            // Plated through: a land on each face, a barrel between them,
            // the drill through the laminate.
            2 => {
                assert_eq!(bodies.len(), 3);
                for layer in [pad.layer, other] {
                    let z = copper_z(layer);
                    assert!(inside(&bodies, on_board(pad, [0.7, 0.0]), z));
                    assert!(inside(&bodies, on_board(pad, [-0.75, 0.0]), z));
                    assert!(
                        !inside(&bodies, on_board(pad, [0.75, 0.45]), z),
                        "stadium end"
                    );
                    assert!(!inside(&bodies, on_board(pad, [0.2, 0.0]), z), "bore");
                }
                let wall = on_board(pad, [0.2 + 0.3 - PLATING_MM / 2.0, 0.0]);
                assert!(inside(&bodies, wall, 0.8), "barrel");
                assert!(!inside(&substrate, wall, 0.8), "barrel inside the laminate");
                assert!(!inside(&bodies, on_board(pad, [0.2, 0.0]), 0.8));
                assert!(!inside(&substrate, on_board(pad, [0.2, 0.0]), 0.8));
                assert!(inside(&substrate, on_board(pad, [0.2 + 0.35, 0.0]), 0.8));
            }
            // A bare hole: no copper and no pick, the laminate drilled.
            90 => {
                assert!(bodies.is_empty());
                assert!(
                    !model
                        .sources
                        .iter()
                        .any(|s| s.instance == Some(pad.instance) && s.pad == Some(pad.pad.id))
                );
                assert!(!inside(&substrate, on_board(pad, [0.45, 0.0]), 0.8));
                assert!(inside(&substrate, on_board(pad, [0.55, 0.0]), 0.8));
            }
            // A far-side land: copper on the opposite face only.
            91 => {
                assert_eq!(bodies.len(), 1);
                assert_ne!(
                    Some(pad.layer),
                    project
                        .placements
                        .iter()
                        .find(|p| p.instance == pad.instance)
                        .map(|p| p.layer)
                );
                assert!(inside(
                    &bodies,
                    on_board(pad, [0.35, 0.2]),
                    copper_z(pad.layer)
                ));
                assert!(
                    !inside(&bodies, on_board(pad, [0.39, 0.24]), copper_z(pad.layer)),
                    "rounded corner"
                );
                assert!(!inside(&bodies, on_board(pad, [0.0, 0.0]), copper_z(other)));
            }
            id => return Err(Error::Verification(format!("unexpected pad {id}"))),
        }
    }
    // The laminate lost two drills per placement.
    let full = 60.0 * 40.0 * 1.6;
    let drilled = 2.0 * std::f64::consts::PI * (0.3f64.powi(2) + 0.5f64.powi(2)) * 1.6;
    let volume: f64 = substrate.iter().map(|g| g.volume).sum();
    assert!((volume - (full - drilled)).abs() < 2e-3, "{volume}");
    Ok(())
}

#[test]
fn mask_openings_and_finish_follow_the_shapes_and_tented_lands_stay_covered() -> Result<()> {
    let project = shaped()?;
    let model = build(&project)?;
    let mask = |layer: Layer| {
        kind(
            &model,
            match layer {
                Layer::Top => BodyKind::MaskTop,
                Layer::Bottom => BodyKind::MaskBottom,
            },
        )
    };
    let mask_z = |layer: Layer| match layer {
        Layer::Top => 1.61,
        Layer::Bottom => -0.01,
    };
    let finish = kind(&model, BodyKind::PadFinish);
    for pad in layout::placed_pads(&project) {
        let layer = pad.layer;
        let other = match layer {
            Layer::Top => Layer::Bottom,
            Layer::Bottom => Layer::Top,
        };
        match pad.pad.id.0 {
            2 => {
                for side in [layer, other] {
                    // The 50 µm default opening around the stadium's tip.
                    let ring = on_board(&pad, [0.8 + 0.025, 0.0]);
                    let beyond = on_board(&pad, [0.8 + 0.08, 0.0]);
                    assert!(!inside(&mask(side), ring, mask_z(side)), "ring on {side:?}");
                    assert!(
                        inside(&mask(side), beyond, mask_z(side)),
                        "beyond on {side:?}"
                    );
                    let z = copper_z(side) + if side == Layer::Top { 0.02 } else { -0.02 };
                    assert!(inside(&finish, on_board(&pad, [0.7, 0.0]), z));
                    assert!(
                        !inside(&finish, on_board(&pad, [0.2, 0.0]), z),
                        "finish over the bore"
                    );
                }
            }
            90 => {
                for side in [Layer::Top, Layer::Bottom] {
                    assert!(!inside(
                        &mask(side),
                        on_board(&pad, [0.0, 0.0]),
                        mask_z(side)
                    ));
                }
            }
            91 => {
                assert!(inside(
                    &mask(layer),
                    on_board(&pad, [0.0, 0.0]),
                    mask_z(layer)
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

#[test]
fn shaped_copper_survives_reopening_and_follows_the_board_thickness() -> Result<()> {
    let project = shaped()?;
    let (r1, _) = instances(&project)?;
    let original = build(&project)?;
    let temporary = tempfile::tempdir()?;
    let path = temporary
        .path()
        .join("shaped.solidcraft")
        .to_string_lossy()
        .into_owned();
    original.save(&path)?;
    let reopened = Native::open(&path)?;
    let before = finish(reopened, original.sources.clone())?;
    assert_eq!(before.geometry.len(), native(&original).count());
    let total = |m: &NativeModel| -> f64 { native(m).map(|g| g.volume).sum() };
    near(total(&before), total(&original));
    let mut native_model = before.native;
    native_model.change_parameters(json!({"board_thickness": "2.4 mm"}))?;
    let changed = finish(native_model, before.sources)?;
    let through = pad_bodies(&changed, r1, PadId(2));
    assert_eq!(through.len(), 3);
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for g in &through {
        lo = lo.min(g.min[2]);
        hi = hi.max(g.max[2]);
    }
    near(lo, -0.035);
    near(hi, 2.4 + 0.035);
    Ok(())
}

#[test]
fn an_envelope_follows_its_turned_placement_and_datum_like_its_2d_outline() -> Result<()> {
    let mut project = envelope_demo()?;
    let package = &mut project.library.packages[0].definition;
    set_body(package, None);
    package.placement = pcbcraft_core::library::ModelPlacement {
        translation: [0.25, -0.4, 0.0],
        rotation_deg: [90.0, 0.0, 90.0],
    };
    let package = package.clone();
    let datum = pcbcraft_core::library::Datum {
        offset: [0.3, -0.4],
        rotation_deg: 90.0,
    };
    project.library.footprints[0].definition.datum = datum;
    for placement in &mut project.placements {
        placement.rotation_deg = 30.0;
    }
    let model = build(&project)?;
    for placement in &project.placements {
        let body = source_geometry(&model, BodyKind::PackageBody, |s| {
            s.instance == Some(placement.instance) && s.pad.is_none()
        })?;
        // Expected: each envelope corner through the model frame into the
        // footprint frame, then onto the board as the pads are placed.
        let frame = pcbcraft_core::library::model_frame(&datum, &package.placement);
        let board = pcbcraft_core::layout::placement_transform(placement, &datum);
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for corner in package.envelope_corners() {
            let f = frame.apply(corner);
            let [x, y] = board.apply([f[0], f[1]]);
            let z = match placement.layer {
                Layer::Top => 1.635 + f[2],
                Layer::Bottom => -0.035 - f[2],
            };
            for (axis, v) in [x, y, z].into_iter().enumerate() {
                lo[axis] = lo[axis].min(v);
                hi[axis] = hi[axis].max(v);
            }
        }
        bounds(body, lo, hi);
    }
    // The preview turns the box the same way (here in the datum frame).
    let preview = preview(&package)?;
    let body = preview
        .geometry
        .iter()
        .find(|g| g.kind == BodyKind::PartBody)
        .ok_or_else(|| Error::Verification("no preview body".into()))?;
    let t = package.placement.transform();
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for corner in package.envelope_corners() {
        let p = t.apply(corner);
        for axis in 0..3 {
            lo[axis] = lo[axis].min(p[axis]);
            hi[axis] = hi[axis].max(p[axis]);
        }
    }
    bounds(body, lo, hi);
    Ok(())
}

#[test]
fn a_saved_board_reopens_with_every_instance_on_its_one_shared_generated_definition() -> Result<()>
{
    use pcbcraft_core::library::GeneratedMaterials;
    // The demo's chip package with a generated model: two instances, one
    // on each side, both occurrences of one definition.
    let mut project = Engine::demo()
        .map_err(|error| Error::Verification(error.to_string()))?
        .project()
        .clone();
    let package = &mut project.library.packages[0];
    package.definition.model = PackageModel::Generated {
        family_rev: 1,
        materials: GeneratedMaterials::default(),
    };
    let stem = models::definition_name(package.id, package.revision.0);
    if let Some(bottom) = project.placements.get_mut(1) {
        bottom.layer = Layer::Bottom;
    }
    let (r1, r2) = instances(&project)?;
    let original = build(&project)?;
    assert_eq!(original.models.len(), 1);
    let temporary = tempfile::tempdir()?;
    let path = temporary
        .path()
        .join("shared.solidcraft")
        .to_string_lossy()
        .into_owned();
    original.save(&path)?;
    let reopened = Native::open(&path)?;
    let metadata = serde_json::to_value(&reopened.session().doc.metadata)?;
    let retained: Vec<Source> =
        serde_json::from_value(metadata["org.pcbcraft.electronics"]["sources"].clone())?;
    let before = finish(reopened, retained)?;
    // Every generated body keeps its instance's pick, bounds and look.
    let generated = |model: &NativeModel| -> Vec<Geometry> {
        model
            .geometry
            .iter()
            .filter(|g| g.kind.is_generated())
            .cloned()
            .collect()
    };
    let (was, now) = (generated(&original), generated(&before));
    assert!(!was.is_empty());
    assert_eq!(was.len(), now.len());
    for previous in &was {
        let restored = now
            .iter()
            .find(|g| {
                g.item == previous.item
                    && g.kind == previous.kind
                    && (0..3).all(|a| (g.min[a] - previous.min[a]).abs() < 1e-6)
            })
            .ok_or_else(|| {
                Error::Verification(format!(
                    "reopen lost {:?} of pick {:?}",
                    previous.kind, previous.item
                ))
            })?;
        bounds(restored, previous.min, previous.max);
        assert_eq!(restored.surface, previous.surface);
    }
    let pick_of = |model: &NativeModel, instance: Id| {
        model
            .sources
            .iter()
            .find(|s| s.instance == Some(instance) && s.occurrence.is_some())
            .map(|s| s.pick_id)
    };
    // One definition parameter moves both occurrences; an instance's own
    // position moves only its occurrence.
    let span = |model: &NativeModel, pick: Option<u64>| {
        let bodies: Vec<&Geometry> = model
            .geometry
            .iter()
            .filter(|g| g.kind.is_generated() && g.item == pick)
            .collect();
        let lo = bodies.iter().map(|g| g.min[0]).fold(f64::MAX, f64::min);
        let hi = bodies.iter().map(|g| g.max[0]).fold(f64::MIN, f64::max);
        (lo, hi)
    };
    let (p1, p2) = (pick_of(&before, r1), pick_of(&before, r2));
    let (a1, a2) = (span(&before, p1), span(&before, p2));
    let mut native = before.native;
    let mut changes = json!({ format!("{stem}_length"): "3 mm" });
    changes[format!("instance_{r1}_x")] = json!("5 mm");
    native.change_parameters(changes)?;
    let changed = finish(native, before.sources)?;
    let (b1, b2) = (span(&changed, p1), span(&changed, p2));
    near(b1.1 - b1.0, 3.0);
    near(b2.1 - b2.0, 3.0);
    near((b1.0 + b1.1) / 2.0, 5.0);
    near((b2.0 + b2.1) / 2.0, (a2.0 + a2.1) / 2.0);
    assert!(((a1.0 + a1.1) / 2.0 - 5.0).abs() > 1.0, "R1 moved");
    Ok(())
}

/// D2's board hook: a package with a library model (an original GLB with
/// its own clear-coated material) builds one shared definition its
/// instances occur in, its bodies carrying their instance's pick, the
/// lead bound to its terminal, and the model material's full look drawn.
#[test]
fn a_library_model_builds_on_the_board_as_one_shared_definition() -> Result<()> {
    use crate::models::imported::tests::{fixture_glb, lead_key, model, use_lead};
    use pcbcraft_core::library::bundle::{AssetBytes, PackedAsset};
    use pcbcraft_core::library::model3d::{LeadTag, ModelFormat};
    use pcbcraft_core::library::{DefinitionMeta, Published};
    use pcbcraft_core::{DefinitionId, ModelRef, Revision};
    let bytes = fixture_glb();
    let seen = crate::models::inspect::inspect(&bytes, ModelFormat::Gltf)?;
    let mut definition = model(ModelFormat::Gltf, &bytes, &seen);
    definition.leads = vec![LeadTag {
        key: lead_key(&seen)?,
        designation: "1".into(),
        contact: [0.0; 3],
    }];
    definition.leads[0].contact = models::imported::lead_contacts(&definition, &bytes)?[0];
    definition.provenance.record.licence = Some("CC0-1.0".into());
    let mut project = Engine::demo()
        .map_err(|error| Error::Verification(error.to_string()))?
        .project()
        .clone();
    let id = DefinitionId(
        project
            .allocate()
            .map_err(|error| Error::Verification(error.to_string()))?,
    );
    let reference = ModelRef::new(id, Revision(1));
    project.library.assets.push(PackedAsset {
        hash: definition.asset.hash,
        media_type: definition.asset.media_type.clone(),
        data: AssetBytes::new(bytes.clone()),
    });
    project.library.models.push(Published {
        id,
        revision: Revision(1),
        meta: DefinitionMeta {
            name: "Original fixture".into(),
            ..DefinitionMeta::default()
        },
        definition,
    });
    let package = &mut project.library.packages[0];
    let terminal = package
        .definition
        .terminals
        .first()
        .map(|t| t.id.0)
        .ok_or_else(|| Error::Verification("demo package has no terminal".into()))?;
    package.definition.model = PackageModel::Library(pcbcraft_core::library::LibraryModel {
        model: reference,
        ..use_lead(terminal)
    });
    let (r1, r2) = instances(&project)?;
    let built = build(&project)?;
    assert_eq!(built.models.len(), 1, "one definition per package revision");
    for instance in [r1, r2] {
        let pick = built
            .sources
            .iter()
            .find(|s| s.instance == Some(instance) && s.occurrence.is_some())
            .map(|s| s.pick_id)
            .ok_or_else(|| Error::Verification(format!("{instance} has no model")))?;
        let bodies: Vec<&Geometry> = built
            .geometry
            .iter()
            .filter(|g| g.item == Some(pick))
            .collect();
        assert!(bodies.iter().any(|g| g.kind == BodyKind::ModelLead));
        let moulding = bodies
            .iter()
            .find(|g| g.kind == BodyKind::ModelBody)
            .ok_or_else(|| Error::Verification("no model body".into()))?;
        assert!(
            moulding.surface.clear_coat().is_some(),
            "the model material's coat is drawn"
        );
    }
    // The model worker's source cache: a rebuild neither re-hashes nor
    // re-reads the model file, and draws the same; a build that no longer
    // uses it drops it.
    let sources = models::imported::SourceCache::default();
    let start = std::time::Instant::now();
    let cold = build_with(&project, &sources)?;
    let cold_time = start.elapsed();
    assert_eq!(sources.counts(), (0, 1));
    let start = std::time::Instant::now();
    let warm = build_with(&project, &sources)?;
    let warm_time = start.elapsed();
    assert_eq!(sources.counts(), (1, 1), "the second build reads nothing");
    assert_eq!(sources.len(), 1);
    assert_eq!(warm.geometry.len(), cold.geometry.len());
    for (a, b) in cold.geometry.iter().zip(&warm.geometry) {
        assert_eq!(
            (a.kind, a.item, a.min, a.max),
            (b.kind, b.item, b.min, b.max)
        );
    }
    eprintln!("library-model board build: cold {cold_time:?}, cached {warm_time:?}");
    // A model or asset the project does not hold refuses the build,
    // naming it; nothing falls back to an envelope.
    let mut no_model = project.clone();
    no_model.library.models.clear();
    let refused = build_with(&no_model, &sources).err().map(|e| e.to_string());
    assert!(sources.is_empty(), "an unused model file is dropped");
    assert!(
        refused
            .as_deref()
            .is_some_and(|e| e.contains("3D model") && e.contains("missing")),
        "{refused:?}"
    );
    project.library.assets.clear();
    let refused = build(&project).err().map(|e| e.to_string());
    assert!(
        refused
            .as_deref()
            .is_some_and(|e| e.contains("fixture.glb") && e.contains("not packed")),
        "{refused:?}"
    );
    Ok(())
}
