#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Lean parts (docs/part-templates.md §11): the conventions, the cache,
//! refusals, and parity of the built-in ESP32-S31-WROOM-3 part file with
//! the module builder it replaces.
use super::*;
use crate::models;
use pcbcraft_core::ids::PackageTerminalId;
use pcbcraft_core::library::marking::surface;
use pcbcraft_core::library::{
    GeneratedMaterials, PackageDefinition, PackageForm, PackageModel, PackageTerminal,
};
use serde_json::json;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const WROOM3: &str = "esp32-s31-wroom-3";

fn wroom3_package() -> PackageDefinition {
    let form: PackageForm =
        serde_json::from_str(include_str!("../../tests/template_spike_wroom3.json")).unwrap();
    PackageDefinition {
        next_local: 100,
        terminals: (1..=99)
            .map(|i| PackageTerminal {
                id: PackageTerminalId(i),
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

fn bytes() -> &'static [u8] {
    builtin(WROOM3).unwrap()
}

/// A small part file made with Solidcraft commands: one box per entry
/// (corner, size, appearance), optional marking plane.
fn small_part(boxes: &[([f64; 3], [f64; 3], Option<&str>)], marking: bool) -> Vec<u8> {
    let mut s = Session::default();
    s.execute("file.new", &json!({"name": "part"})).unwrap();
    s.execute(
        "parameters.import",
        &json!({"text": serde_json::to_string(&json!([
            {"name": "body_height", "expression": "1 mm", "unit": "mm"},
            {"name": "marking_top_u", "expression": "2 mm", "unit": "mm"},
            {"name": "marking_top_v", "expression": "1 mm", "unit": "mm"},
        ])).unwrap()}),
    )
    .unwrap();
    for (k, (corner, size, look)) in boxes.iter().enumerate() {
        let made = s
            .execute(
                "solid.box",
                &json!({"corner": corner, "length": size[0], "width": size[1],
                    "height": if k == 0 { json!("body_height") } else { json!(size[2]) },
                    "name": format!("b{k}")}),
            )
            .unwrap();
        let name = made["created_bodies"][0].clone();
        if let Some(look) = look {
            s.execute(
                "appearance.assign",
                &json!({"bodies": [name], "name": look, "color": "#202020", "opacity": 1.0}),
            )
            .unwrap();
        }
    }
    if marking {
        s.execute(
            "construct.plane.offset",
            &json!({"base": {"origin": [0, 0, "body_height"], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]},
                "offset": "0 mm", "name": "marking-top"}),
        )
        .unwrap();
    }
    solidcraft_engine::io::write_design(&s.doc)
}

#[test]
fn appearance_names_map_every_material_and_nothing_else() {
    for (name, id) in materials() {
        assert_eq!(material_of(&name), Some(id), "{name}");
    }
    assert_eq!(material_of("Nickel Silver"), Some(MaterialId::NickelSilver));
    assert_eq!(material_of("nickel_silver"), Some(MaterialId::NickelSilver));
    assert_eq!(appearance_name(MaterialId::NickelSilver), "nickel-silver");
    for other in ["", "chrome", "gold!", "pcbcraft:gold", &"x".repeat(100)] {
        assert_eq!(material_of(other), None, "{other}");
    }
}

#[test]
fn the_built_in_wroom3_part_passes_its_check() -> Fallible {
    let part = evaluate(bytes(), &PartValues::new())?;
    let report = part.report();
    assert!(report.ok, "{:?}", report.findings);
    assert!(
        report.findings.iter().all(|f| f.code != "part_file.import"),
        "an original part imports nothing"
    );
    assert_eq!(report.bodies.len(), 110);
    assert!(report.bodies.iter().all(|b| b.material.is_some()));
    assert_eq!(
        report.surfaces.first().map(|s| s.name.as_str()),
        Some("top")
    );
    assert!(report.pin1.is_some());
    for name in [
        "can_height",
        "board_thickness",
        "can_width",
        "can_length",
        "width",
        "length",
    ] {
        assert!(report.parameters.iter().any(|p| p.name == name), "{name}");
    }
    Ok(())
}

/// The part file's marking surfaces are the module form's anchors.
#[test]
fn the_wroom3_marking_planes_are_the_module_anchors() -> Fallible {
    let package = wroom3_package();
    let part = evaluate(bytes(), &PartValues::new())?;
    let anchors = surface::anchors(&package.form);
    assert!(!anchors.is_empty());
    for a in &anchors {
        let found = part
            .surfaces
            .iter()
            .find(|s| s.name == a.name)
            .unwrap_or_else(|| panic!("no surface {}", a.name));
        let (SurfaceGeometry::Plane(x), SurfaceGeometry::Plane(y)) = (a.geometry, found.geometry)
        else {
            panic!("{} is a plane", a.name);
        };
        let close = |p: &[f64], q: &[f64]| p.iter().zip(q).all(|(u, v)| (u - v).abs() < 1e-9);
        assert!(
            close(&x.origin, &y.origin)
                && close(&x.normal, &y.normal)
                && close(&x.u_axis, &y.u_axis)
                && close(&x.size_mm, &y.size_mm),
            "{}: form {x:?}, part {y:?}",
            a.name
        );
    }
    // The default area is the whole can top, as `package_area` of a module.
    let PackageForm::Module(m) = &package.form else {
        panic!("a module");
    };
    let derived = derived(&part);
    let file = pcbcraft_core::library::package_model::PartFileModel {
        derived,
        ..Default::default()
    };
    assert_eq!(file.default_area(), Some(m.can_size));
    Ok(())
}

/// Per body: bounding box within 1 µm and volume within 1e-9 relative;
/// the same body count; equal union bounds; equal materials.
#[test]
fn the_wroom3_part_builds_what_the_module_builder_builds() -> Fallible {
    let package = wroom3_package();
    let mut generated = Native::new("generated")?;
    let def = models::build_definition(&mut generated, "pkgT", &package)?;
    let part = evaluate(bytes(), &PartValues::new())?;
    let mut board = Native::new("board")?;
    let stem = part.stem();
    let inserted = insert(&mut board, &stem, &part)?;
    assert!(board.evaluation_error().is_none());
    assert_eq!(def.bodies.len(), inserted.bodies.len());
    let (ga, gb) = (
        generated.session().world_state(),
        board.session().world_state(),
    );
    let (mut worst_box, mut worst_volume) = (0.0f64, 0.0f64);
    let (mut volume_a, mut volume_b) = (0.0, 0.0);
    for body in &def.bodies {
        let plain = body.name.strip_prefix("pkgT ").unwrap();
        let x = ga.bodies.iter().find(|b| b.name == body.name).unwrap();
        let y = gb
            .bodies
            .iter()
            .find(|b| b.name == format!("{stem} {plain}"))
            .unwrap_or_else(|| panic!("no {plain} in the part"));
        let (mx, my) = (x.measure()?, y.measure()?);
        for (p, q) in [(mx.bbox.min, my.bbox.min), (mx.bbox.max, my.bbox.max)] {
            worst_box = worst_box
                .max((p.x - q.x).abs())
                .max((p.y - q.y).abs())
                .max((p.z - q.z).abs());
        }
        worst_volume = worst_volume.max((mx.volume - my.volume).abs() / mx.volume.abs());
        volume_a += mx.volume;
        volume_b += my.volume;
        let made = inserted
            .bodies
            .iter()
            .find(|b| b.name == format!("{stem} {plain}"))
            .unwrap();
        assert_eq!(made.material, body.material, "{plain}");
        assert_eq!(
            made.kind(),
            body.kind(),
            "{plain} is drawn as the builder draws it"
        );
    }
    let union_a = models::imported::bounds_of(
        &generated,
        &def.bodies
            .iter()
            .map(|b| b.name.clone())
            .collect::<Vec<_>>(),
    )?;
    eprintln!(
        "WROOM-3 parity: {} bodies each, worst bbox Δ {worst_box:.2e} mm, worst volume Δ \
         {worst_volume:.2e} (relative), volume {volume_a:.4} vs {volume_b:.4} mm³, \
         bounds {union_a:?} vs {:?}",
        def.bodies.len(),
        part.bounds_mm
    );
    assert!(worst_box < 1e-6, "{worst_box}");
    assert!(worst_volume < 1e-9, "{worst_volume}");
    for (a, b) in union_a
        .iter()
        .flatten()
        .zip(part.bounds_mm.iter().flatten())
    {
        assert!((a - b).abs() < 1e-6, "{union_a:?} vs {:?}", part.bounds_mm);
    }
    Ok(())
}

/// The part stays parametric: a value changes the geometry; the inserted
/// definition keeps live, prefixed parameters.
#[test]
fn values_drive_the_part_and_stay_live_in_the_board() -> Fallible {
    let mut values = PartValues::new();
    values.insert("can_height".into(), PartValue::Mm(2.0));
    let part = evaluate(bytes(), &values)?;
    assert!(!part.blocked(), "{:?}", part.findings);
    let base = evaluate(bytes(), &PartValues::new())?;
    let dz = base.bounds_mm[1][2] - part.bounds_mm[1][2];
    assert!((dz - 0.465).abs() < 1e-6, "the can 0.465 mm lower: {dz}");
    assert_ne!(part.stem(), base.stem());
    let mut board = Native::new("board")?;
    insert(&mut board, &part.stem(), &part)?;
    let name = format!("{}_can_height", part.stem());
    let p = board
        .session()
        .doc
        .params
        .iter()
        .find(|p| p.name == name)
        .unwrap();
    assert_eq!(p.expr, "2 mm");
    // A second definition beside the first, and a clash refused.
    insert(&mut board, &base.stem(), &base)?;
    assert!(insert(&mut board, &base.stem(), &base).is_err());
    assert!(board.evaluation_error().is_none());
    Ok(())
}

#[test]
fn equal_values_evaluate_once() -> Fallible {
    let mut values = PartValues::new();
    values.insert("can_round".into(), PartValue::Mm(0.25));
    let first = evaluate(bytes(), &values)?;
    let (hits, _) = cache_counts();
    let second = evaluate(bytes(), &values)?;
    assert!(Arc::ptr_eq(&first, &second), "served from the cache");
    assert!(cache_counts().0 > hits);
    Ok(())
}

#[test]
fn bad_part_files_and_values_are_refused() -> Fallible {
    let code = |r: Result<Arc<EvaluatedPart>>| match r {
        Err(Error::Invalid(m)) => m.split(':').next().unwrap_or_default().to_owned(),
        other => format!("{other:?}"),
    };
    assert_eq!(code(evaluate(b"", &PartValues::new())), "part_file.read");
    assert_eq!(
        code(evaluate(b"{not a design", &PartValues::new())),
        "part_file.read"
    );
    let mut unknown = PartValues::new();
    unknown.insert("no_such".into(), PartValue::Mm(1.0));
    assert_eq!(code(evaluate(bytes(), &unknown)), "part_file.value");
    let mut unit = PartValues::new();
    unit.insert("can_height".into(), PartValue::Count(2));
    assert_eq!(code(evaluate(bytes(), &unit)), "part_file.value");
    let mut name = PartValues::new();
    name.insert("can height; x".into(), PartValue::Mm(1.0));
    assert_eq!(code(evaluate(bytes(), &name)), "part_file.value");
    let mut huge = PartValues::new();
    huge.insert("can_height".into(), PartValue::Mm(f64::INFINITY));
    assert_eq!(code(evaluate(bytes(), &huge)), "part_file.value");
    Ok(())
}

fn codes(part: &EvaluatedPart) -> Vec<(&str, bool)> {
    part.findings
        .iter()
        .map(|f| (f.code.as_str(), f.blocking))
        .collect()
}

#[test]
fn convention_breaks_are_findings_and_block_the_insert() -> Fallible {
    // Good: seated, mapped, marked.
    let good = small_part(
        &[([-1.0, -0.5, 0.0], [2.0, 1.0, 1.0], Some("epoxy-mould"))],
        true,
    );
    let part = evaluate(&good, &PartValues::new())?;
    assert!(!part.blocked(), "{:?}", part.findings);
    assert_eq!(part.surfaces.len(), 1);
    // No appearance, and one Pcbcraft does not know.
    let unmapped = small_part(
        &[
            ([-1.0, -0.5, 0.0], [2.0, 1.0, 1.0], None),
            ([2.0, -0.5, 0.0], [1.0, 1.0, 1.0], Some("chrome")),
        ],
        true,
    );
    let part = evaluate(&unmapped, &PartValues::new())?;
    assert!(codes(&part).contains(&("part_file.material", true)));
    let mut board = Native::new("board")?;
    assert!(insert(&mut board, &part.stem(), &part).is_err());
    assert!(
        board.find_component(&part.stem()).is_none(),
        "nothing inserted"
    );
    // Floating above the seating plane, and no marking plane (advisory).
    let floating = small_part(&[([-1.0, -0.5, 0.5], [2.0, 1.0, 1.0], Some("gold"))], false);
    let part = evaluate(&floating, &PartValues::new())?;
    assert!(codes(&part).contains(&("part_file.seating", true)));
    assert!(codes(&part).contains(&("part_file.no_marking", false)));
    // A marking plane without its size, and the origin outside the part.
    let mut s = Session::new(solidcraft_engine::io::read_design(&good)?);
    s.execute(
        "construct.plane.offset",
        &json!({"base": "XY", "offset": "1 mm", "name": "marking-side"}),
    )?;
    let far = s.execute(
        "solid.box",
        &json!({"corner": [5, 5, 0], "length": 1, "width": 1, "height": 1, "name": "far"}),
    )?;
    s.execute(
        "appearance.assign",
        &json!({"bodies": [far["created_bodies"][0]], "name": "gold", "color": "#d0a000", "opacity": 1.0}),
    )?;
    s.execute(
        "parameters.change",
        &json!({"changes": {"marking_top_u": "0 mm"}}),
    )?;
    let part = evaluate(
        &solidcraft_engine::io::write_design(&s.doc),
        &PartValues::new(),
    )?;
    let found = codes(&part);
    assert!(found.contains(&("part_file.marking", true)), "{found:?}");
    // A feature failing at the values.
    let mut v = PartValues::new();
    v.insert("body_height".into(), PartValue::Mm(0.0));
    let part = evaluate(&good, &v)?;
    assert!(
        codes(&part).contains(&("part_file.feature", true)),
        "{:?}",
        part.findings
    );
    Ok(())
}

#[test]
fn a_nested_part_is_refused() -> Fallible {
    let mut s = Session::new(solidcraft_engine::io::read_design(&small_part(
        &[([-1.0, -0.5, 0.0], [2.0, 1.0, 1.0], Some("gold"))],
        true,
    ))?);
    s.execute(
        "component.create",
        &json!({"name": "inner", "grounded": false}),
    )?;
    s.execute(
        "solid.box",
        &json!({"corner": [0, 0, 0], "length": 1, "width": 1, "height": 1, "name": "nested"}),
    )?;
    let part = evaluate(
        &solidcraft_engine::io::write_design(&s.doc),
        &PartValues::new(),
    )?;
    assert!(
        codes(&part).contains(&("part_file.structure", true)),
        "{:?}",
        part.findings
    );
    Ok(())
}

/// The evaluated part's total volume, mm³, through Solidcraft's measure.
fn volume(part: &EvaluatedPart) -> Fallible<f64> {
    let mut s = Session::new((*part.source).clone());
    let measured = s.execute("inspect.measure", &json!({}))?;
    Ok(measured["total"]["volume_mm3"].as_f64().unwrap_or(0.0))
}

/// The mechanical starter set (docs/mechanical-parts.md §M7): each built-in
/// passes its check at its defaults, names its parameters and material,
/// seats on z = 0, has `marking-top`, and follows its values.
#[test]
fn the_mechanical_built_ins_pass_their_check_and_follow_their_values() -> Fallible {
    type Expected = (
        &'static str,
        &'static [(&'static str, &'static str)],
        MaterialId,
    );
    let set: [Expected; 3] = [
        (
            "smd-nut",
            &[("thread_d", "mm"), ("outer_d", "mm"), ("height", "mm")],
            MaterialId::TinMatte,
        ),
        (
            "round-spacer",
            &[("inner_d", "mm"), ("outer_d", "mm"), ("height", "mm")],
            MaterialId::Pa9t,
        ),
        (
            "finned-heatsink",
            &[
                ("length", "mm"),
                ("width", "mm"),
                ("height", "mm"),
                ("base_height", "mm"),
                ("fin_count", ""),
                ("fin_thickness", "mm"),
            ],
            MaterialId::Aluminium,
        ),
    ];
    for (name, params, material) in set {
        let part = evaluate(builtin(name).unwrap(), &PartValues::new())?;
        let report = part.report();
        eprintln!(
            "{name}: {} bodies, cold evaluate {:.1} ms",
            report.bodies.len(),
            report.evaluate_ms
        );
        assert!(report.ok, "{name}: {:?}", report.findings);
        for (p, unit) in params {
            let found = report.parameters.iter().find(|q| q.name == *p);
            assert_eq!(found.map(|q| q.unit.as_str()), Some(*unit), "{name} {p}");
        }
        assert_eq!(
            report.surfaces.first().map(|s| s.name.as_str()),
            Some("top"),
            "{name}"
        );
        assert!(!report.bodies.is_empty(), "{name}");
        assert!(
            report.bodies.iter().all(|b| b.material == Some(material)),
            "{name}: {:?}",
            report.bodies
        );
        assert!(
            report.bounds_mm[0][2].abs() <= 0.01,
            "{name} seats on z = 0"
        );
        for axis in 0..2 {
            let mid = (report.bounds_mm[0][axis] + report.bounds_mm[1][axis]) / 2.0;
            assert!(mid.abs() < 1e-6, "{name} centred: {:?}", report.bounds_mm);
        }
    }
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    // The nut's height, and its marking area on top.
    let mut values = PartValues::new();
    values.insert("height".into(), PartValue::Mm(4.0));
    let nut = evaluate(builtin("smd-nut").unwrap(), &values)?;
    assert!(!nut.blocked(), "{:?}", nut.findings);
    assert!(close(nut.bounds_mm[1][2], 4.0), "{:?}", nut.bounds_mm);
    let Some(SurfaceGeometry::Plane(area)) = nut.surfaces.first().map(|s| s.geometry) else {
        panic!("a planar marking-top");
    };
    assert!(
        close(area.origin[2], 4.0),
        "marking-top on the top: {area:?}"
    );
    assert!(close(area.size_mm[0], 3.0), "0.6 × outer_d: {area:?}");
    // The spacer's diameter. A round body's outline bounds hold the
    // circle but are not tight (they come from its edges' outline), so the
    // check is that the box holds 8 mm and scales with the diameter.
    let mut values = PartValues::new();
    values.insert("outer_d".into(), PartValue::Mm(8.0));
    let spacer = evaluate(builtin("round-spacer").unwrap(), &values)?;
    assert!(!spacer.blocked(), "{:?}", spacer.findings);
    let six = evaluate(builtin("round-spacer").unwrap(), &PartValues::new())?;
    for axis in 0..2 {
        let w = spacer.bounds_mm[1][axis] - spacer.bounds_mm[0][axis];
        let w6 = six.bounds_mm[1][axis] - six.bounds_mm[0][axis];
        assert!(w >= 8.0 - 0.01, "8 mm across: {:?}", spacer.bounds_mm);
        assert!((w / w6 - 8.0 / 6.0).abs() < 1e-3, "{w} vs {w6}");
    }
    // The heatsink's fin count: fewer fins, less metal, one body still.
    let heatsink = builtin("finned-heatsink").unwrap();
    let six = evaluate(heatsink, &PartValues::new())?;
    let mut values = PartValues::new();
    values.insert("fin_count".into(), PartValue::Count(4));
    let four = evaluate(heatsink, &values)?;
    assert!(!four.blocked(), "{:?}", four.findings);
    assert_eq!(four.bodies.len(), six.bodies.len());
    // A 20 × 20 × 2 base and fins of 20 × 1.2 × 8 = 192 mm³ each.
    let (v6, v4) = (volume(&six)?, volume(&four)?);
    assert!((v6 - (800.0 + 6.0 * 192.0)).abs() < 0.5, "six fins: {v6}");
    assert!((v4 - (800.0 + 4.0 * 192.0)).abs() < 0.5, "four fins: {v4}");
    Ok(())
}
