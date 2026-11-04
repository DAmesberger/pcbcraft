//! Authors Pcbcraft's built-in part files (docs/part-templates.md §11)
//! through Solidcraft's own commands and public API, and writes them under
//! `library/parts/`. Original work (MIT OR Apache-2.0).
//!
//! The ESP32-S31-WROOM-3 module body starts from the module builder's
//! construction of the example project's form (so its geometry is the
//! generated model's, measured equal by `part_file` parity tests), moved
//! into the root component of a new design with plain parameter names
//! (`can_height`, `pcb_thickness`, …) and plain body names. Appearances are
//! named after Pcbcraft materials; the marking surfaces are construction
//! planes `marking-<name>` placed by expressions over those parameters,
//! sized by `marking_<name>_u` / `_v`; `pin1` is a construction point.
//!
//! The mechanical starter set (docs/mechanical-parts.md §M7: SMD nut,
//! round spacer, finned heatsink) is drawn directly from primitives and a
//! parametric pattern over plain parameters, with generic illustrative
//! dimensions.
//!
//! `cargo test -p pcbcraft-solidcraft --test author_parts -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pcbcraft_core::ids::PackageTerminalId;
use pcbcraft_core::library::{
    GeneratedMaterials, PackageDefinition, PackageForm, PackageModel, PackageTerminal,
};
use pcbcraft_core::materials::MaterialId;
use pcbcraft_solidcraft::models::{self, look, part_file};
use pcbcraft_solidcraft::native::Native;
use serde_json::json;
use solidcraft_engine::Session;
use solidcraft_engine::doc::clone_expression;
use std::collections::BTreeMap;
use std::path::PathBuf;

const STEM: &str = "pkgT";

fn wroom3() -> PackageDefinition {
    let form: PackageForm =
        serde_json::from_str(include_str!("template_spike_wroom3.json")).unwrap();
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

fn run(s: &mut Session, command: &str, params: serde_json::Value) -> serde_json::Value {
    s.execute(command, &params)
        .unwrap_or_else(|e| panic!("{command} {params}: {e}"))
}

/// A `marking-<name>` plane: origin expressions, u axis and normal.
struct Mark {
    name: &'static str,
    origin: [String; 3],
    u: [f64; 3],
    normal: [f64; 3],
    size: [String; 2],
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The WROOM-3 module body as a part file's bytes.
pub fn author_wroom3() -> Vec<u8> {
    let package = wroom3();
    let PackageForm::Module(form) = &package.form else {
        panic!("a module form");
    };
    // 1. The module builder's construction in a scratch design.
    let mut native = Native::new("scratch").unwrap();
    let def = models::build_definition(&mut native, STEM, &package).unwrap();
    let src = native.session();
    let from = src
        .doc
        .components
        .iter()
        .find(|c| c.name == STEM)
        .map(|c| c.id)
        .unwrap();
    let plain = |n: &str| n.strip_prefix(&format!("{STEM} ")).unwrap_or(n).to_owned();
    let parameters: BTreeMap<String, String> = src
        .doc
        .param_defs()
        .into_iter()
        .map(|d| {
            let p = d
                .name
                .strip_prefix(&format!("{STEM}_"))
                .unwrap_or(&d.name)
                .to_owned();
            (d.name, p)
        })
        .collect();
    let mut bodies: BTreeMap<String, String> = src
        .world_state()
        .unwrap()
        .bodies
        .iter()
        .map(|b| (b.name.clone(), plain(&b.name)))
        .collect();
    let features: Vec<_> = src
        .doc
        .features
        .iter()
        .filter(|f| f.component == from)
        .cloned()
        .collect();
    for f in &features {
        for n in f.body_names.iter().chain(&f.participants) {
            bodies.entry(n.clone()).or_insert_with(|| plain(n));
        }
    }
    // 2. A new design: the parameters by their plain names, the features
    // in its root component.
    let mut part = Session::default();
    run(
        &mut part,
        "file.new",
        json!({"name": "ESP32-S31-WROOM-3 module"}),
    );
    let rows: Vec<serde_json::Value> = src
        .doc
        .params
        .iter()
        .filter(|p| !p.model && p.name.starts_with(&format!("{STEM}_")))
        .map(|p| {
            json!({"name": parameters[&p.name], "expression": clone_expression(&p.expr, &parameters),
                "unit": p.unit, "comment": p.comment})
        })
        .collect();
    run(
        &mut part,
        "parameters.import",
        json!({"text": serde_json::to_string(&rows).unwrap()}),
    );
    {
        let doc = part.doc_mut();
        let mut next = doc.next_id.max(1);
        let mut ids = BTreeMap::new();
        let mut names = BTreeMap::new();
        for f in &features {
            ids.insert(f.id, next);
            next += 1;
            names.insert(f.name.clone(), plain(&f.name));
        }
        doc.next_id = next;
        // Feature inputs and dimensions the builder named (not user
        // parameters) travel with their features.
        for p in src.doc.params.iter().filter(|p| p.model) {
            let mut q = p.clone();
            q.name = parameters[&p.name].clone();
            q.expr = clone_expression(&p.expr, &parameters);
            doc.params.push(q);
        }
        for f in &features {
            let mut g = f.clone();
            g.remap_clone(&ids, &names, &bodies, &parameters).unwrap();
            g.id = ids[&f.id];
            g.name = names[&f.name].clone();
            g.component = 0;
            doc.features.push(g);
        }
    }
    part.refresh();
    let errors: Vec<String> = part
        .model
        .results
        .iter()
        .filter_map(|r| r.error.clone().map(|e| format!("{}: {e}", r.name)))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    // 3. Appearances named after Pcbcraft materials.
    let mut by_material: BTreeMap<_, Vec<String>> = BTreeMap::new();
    for b in &def.bodies {
        by_material
            .entry(b.material)
            .or_default()
            .push(plain(&b.name));
    }
    for (material, names) in by_material {
        let d = look::definition(material).unwrap();
        let l = look::saved(&d);
        let [r, g, b] = l.srgb8;
        for chunk in names.chunks(200) {
            run(
                &mut part,
                "appearance.assign",
                json!({"bodies": chunk, "name": part_file::appearance_name(material),
                    "color": format!("#{r:02x}{g:02x}{b:02x}"), "opacity": 1.0,
                    "pbr": {"metallic": l.metallic, "roughness": l.roughness, "dielectric_ior": l.ior}}),
            );
        }
    }
    // 4. Marking planes over the part's own parameters, matching the
    // module form's anchors (`surface::anchors`) in place and size. The
    // builder draws the can's centre and the antenna from numbers, so those
    // stay numbers here too.
    use pcbcraft_core::library::marking::surface::{BoxFace, MODULE_TRACE_MM};
    let mm = |v: f64| format!("({v} mm)");
    let [cx, cy] = form.can_centre;
    let board_top = "(pad_thickness + board_thickness)".to_owned();
    let top = format!("({board_top} + can_height)");
    let side_z = format!("({board_top} + {top}) / 2");
    let can_end = format!("({} + can_length / 2)", mm(cy));
    let silk = format!("{board_top} + {}", mm(MODULE_TRACE_MM));
    let side = |name: &'static str, face: BoxFace, origin: [String; 2], along: &str| {
        let (normal, u) = face.frame();
        Mark {
            name,
            origin: [origin[0].clone(), origin[1].clone(), side_z.clone()],
            u,
            normal,
            size: [along.into(), "can_height".into()],
        }
    };
    let (up, x) = BoxFace::Top.frame();
    let antenna_from = form.length / 2.0 - form.antenna_length.min(form.length);
    let marks = vec![
        Mark {
            name: "top",
            origin: [mm(cx), mm(cy), top.clone()],
            u: x,
            normal: up,
            size: ["can_width".into(), "can_length".into()],
        },
        Mark {
            name: "can-top",
            origin: [mm(cx), mm(cy), top.clone()],
            u: x,
            normal: up,
            size: [
                "can_width - 2 * can_round".into(),
                "can_length - 2 * can_round".into(),
            ],
        },
        side(
            "can-pos-x",
            BoxFace::PosX,
            [format!("{} + can_width / 2", mm(cx)), mm(cy)],
            "can_length",
        ),
        side(
            "can-neg-x",
            BoxFace::NegX,
            [format!("{} - can_width / 2", mm(cx)), mm(cy)],
            "can_length",
        ),
        side(
            "can-pos-y",
            BoxFace::PosY,
            [mm(cx), format!("{} + can_length / 2", mm(cy))],
            "can_width",
        ),
        side(
            "can-neg-y",
            BoxFace::NegY,
            [mm(cx), format!("{} - can_length / 2", mm(cy))],
            "can_width",
        ),
        Mark {
            name: "pcb-top",
            origin: [
                "0 mm".into(),
                format!("({can_end} + length / 2) / 2"),
                silk.clone(),
            ],
            u: x,
            normal: up,
            size: ["width".into(), format!("length / 2 - {can_end}")],
        },
        Mark {
            name: "antenna-area",
            origin: [
                "0 mm".into(),
                format!("({} + length / 2) / 2", mm(antenna_from)),
                silk.clone(),
            ],
            u: x,
            normal: up,
            size: ["width".into(), format!("length / 2 - {}", mm(antenna_from))],
        },
    ];
    let mut extra = Vec::new();
    for m in &marks {
        let stem = m.name.replace('-', "_");
        extra.push(
            json!({"name": format!("marking_{stem}_u"), "expression": m.size[0], "unit": "mm",
            "comment": format!("marking-{} size along its u axis", m.name)}),
        );
        extra.push(
            json!({"name": format!("marking_{stem}_v"), "expression": m.size[1], "unit": "mm",
            "comment": format!("marking-{} size across it", m.name)}),
        );
    }
    run(
        &mut part,
        "parameters.import",
        json!({"text": serde_json::to_string(&extra).unwrap()}),
    );
    for m in &marks {
        let y = cross(m.normal, m.u);
        run(
            &mut part,
            "construct.plane.offset",
            json!({"base": {"origin": m.origin, "x_dir": m.u, "y_dir": y}, "offset": "0 mm",
                "name": format!("marking-{}", m.name)}),
        );
    }
    // 5. Pin 1: the first lead point, on the seating plane.
    let first = package.form.lead_points().first().copied().unwrap();
    run(
        &mut part,
        "construct.point.vertex",
        json!({"point": {"point": [first[0], first[1], 0.0]}, "name": "pin1"}),
    );
    solidcraft_engine::io::write_design(&part.doc)
}

/// Give `bodies` the Solidcraft appearance named after `material`, with
/// Pcbcraft's opaque PBR projection of its look.
fn assign_look(s: &mut Session, bodies: &[&str], material: MaterialId) {
    let d = look::definition(material).unwrap();
    let l = look::saved(&d);
    let [r, g, b] = l.srgb8;
    run(
        s,
        "appearance.assign",
        json!({"bodies": bodies, "name": part_file::appearance_name(material),
            "color": format!("#{r:02x}{g:02x}{b:02x}"), "opacity": 1.0,
            "pbr": {"metallic": l.metallic, "roughness": l.roughness, "dielectric_ior": l.ior}}),
    );
}

/// A new design `title` with the user parameters `(name, expression,
/// unit, comment)` and the `marking-top` size parameters.
fn new_part(title: &str, params: &[(&str, &str, &str, &str)], mark: [&str; 2]) -> Session {
    let mut s = Session::default();
    run(&mut s, "file.new", json!({"name": title}));
    let mut rows: Vec<serde_json::Value> = params
        .iter()
        .map(|(name, expression, unit, comment)| {
            json!({"name": name, "expression": expression, "unit": unit, "comment": comment})
        })
        .collect();
    rows.push(
        json!({"name": "marking_top_u", "expression": mark[0], "unit": "mm",
        "comment": "marking-top size along its u axis"}),
    );
    rows.push(
        json!({"name": "marking_top_v", "expression": mark[1], "unit": "mm",
        "comment": "marking-top size across it"}),
    );
    run(
        &mut s,
        "parameters.import",
        json!({"text": serde_json::to_string(&rows).unwrap()}),
    );
    s
}

/// The `marking-top` plane at the top centre (`top` an expression), u
/// along +x, normal +z; then the design's bytes.
fn finish_part(mut s: Session, top: &str) -> Vec<u8> {
    run(
        &mut s,
        "construct.plane.offset",
        json!({"base": {"origin": ["0 mm", "0 mm", top], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]},
            "offset": "0 mm", "name": "marking-top"}),
    );
    s.refresh();
    let errors: Vec<String> = s
        .model
        .results
        .iter()
        .filter_map(|r| r.error.clone().map(|e| format!("{}: {e}", r.name)))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    solidcraft_engine::io::write_design(&s.doc)
}

/// A tube standing on z = 0: a cylinder of diameter `outer` with a
/// through bore of diameter `inner`, `height` tall.
fn tube(s: &mut Session, body: &str, outer: &str, inner: &str) {
    run(
        s,
        "solid.cylinder",
        json!({"diameter": outer, "height": "height", "name": "Body", "body_name": body}),
    );
    run(
        s,
        "solid.cylinder",
        json!({"diameter": inner, "height": "height + 2 mm", "base": ["0 mm", "0 mm", "-1 mm"],
            "operation": "cut", "targets": [body], "name": "Bore"}),
    );
}

/// An SMD nut: a tin-plated tube soldered to the board by its base.
/// Generic illustrative dimensions.
pub fn author_smd_nut() -> Vec<u8> {
    let mut s = new_part(
        "SMD nut",
        &[
            ("thread_d", "3 mm", "mm", "thread (bore) diameter"),
            ("outer_d", "5 mm", "mm", "outer diameter"),
            ("height", "3 mm", "mm", "height above the seating plane"),
        ],
        ["outer_d * 0.6", "outer_d * 0.6"],
    );
    tube(&mut s, "Nut", "outer_d", "thread_d");
    assign_look(&mut s, &["Nut"], MaterialId::TinMatte);
    finish_part(s, "height")
}

/// A round spacer: a plastic tube. Generic illustrative dimensions.
pub fn author_round_spacer() -> Vec<u8> {
    let mut s = new_part(
        "Round spacer",
        &[
            ("inner_d", "3.2 mm", "mm", "bore diameter"),
            ("outer_d", "6 mm", "mm", "outer diameter"),
            ("height", "5 mm", "mm", "height above the seating plane"),
        ],
        ["outer_d * 0.6", "outer_d * 0.6"],
    );
    tube(&mut s, "Spacer", "outer_d", "inner_d");
    assign_look(&mut s, &["Spacer"], MaterialId::Pa9t);
    finish_part(s, "height")
}

/// A finned heatsink: a base plate and `fin_count` fins along x, evenly
/// spaced across the width with the outer fins flush with the sides. The
/// first fin joins the base, and a rectangular pattern whose count is the
/// `fin_count` parameter replays it, so the part stays one body at any
/// count. Generic illustrative dimensions.
pub fn author_finned_heatsink() -> Vec<u8> {
    let mut s = new_part(
        "Finned heatsink",
        &[
            (
                "length",
                "20 mm",
                "mm",
                "size along x (the fins' direction)",
            ),
            ("width", "20 mm", "mm", "size along y"),
            (
                "height",
                "10 mm",
                "mm",
                "overall height above the seating plane",
            ),
            ("base_height", "2 mm", "mm", "base plate thickness"),
            ("fin_count", "6", "", "number of fins (at least 2)"),
            ("fin_thickness", "1.2 mm", "mm", "thickness of one fin"),
        ],
        ["length", "width"],
    );
    run(
        &mut s,
        "solid.box",
        json!({"corner": ["-length / 2", "-width / 2", "0 mm"], "length": "length",
            "width": "width", "height": "base_height", "name": "Base", "body_name": "Heatsink"}),
    );
    run(
        &mut s,
        "solid.box",
        json!({"corner": ["-length / 2", "-width / 2", "base_height"], "length": "length",
            "width": "fin_thickness", "height": "height - base_height", "operation": "join",
            "targets": ["Heatsink"], "name": "Fin"}),
    );
    run(
        &mut s,
        "solid.pattern.rectangular",
        json!({"features": ["Fin"], "dir1": [0, 1, 0], "count1": "fin_count",
            "spacing1": "(width - fin_thickness) / (fin_count - 1)", "name": "Fins"}),
    );
    assign_look(&mut s, &["Heatsink"], MaterialId::Aluminium);
    finish_part(s, "height")
}

#[test]
#[ignore = "writes library/parts/*.solidcraft"]
fn author_parts() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../library/parts");
    std::fs::create_dir_all(&dir).unwrap();
    type Author = fn() -> Vec<u8>;
    let parts: [(&str, Author); 4] = [
        ("esp32-s31-wroom-3", author_wroom3),
        ("smd-nut", author_smd_nut),
        ("round-spacer", author_round_spacer),
        ("finned-heatsink", author_finned_heatsink),
    ];
    for (name, author) in parts {
        let bytes = author();
        let path = dir.join(format!("{name}.solidcraft"));
        std::fs::write(&path, &bytes).unwrap();
        let report = part_file::evaluate(&bytes, &BTreeMap::new())
            .unwrap()
            .report();
        eprintln!(
            "wrote {} ({} KiB): {} parameters, {} bodies, {} surfaces, {:.1} ms, findings {:?}",
            path.display(),
            bytes.len() / 1024,
            report.parameters.len(),
            report.bodies.len(),
            report.surfaces.len(),
            report.evaluate_ms,
            report.findings
        );
        for p in &report.parameters {
            eprintln!(
                "  {} = {} [{}] -> {:?}",
                p.name, p.expression, p.unit, p.value
            );
        }
        assert!(report.ok, "{name}: {:?}", report.findings);
    }
}
