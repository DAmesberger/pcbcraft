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

/// A test length in mm, as stored.
#[allow(dead_code)]
fn nm(mm: f64) -> pcbcraft_core::units::Nm {
    pcbcraft_core::units::Nm::from_mm_f64_rounded(mm).unwrap_or_default()
}

/// A test point or size in mm, as stored.
#[allow(dead_code)]
fn pt(p: [f64; 2]) -> pcbcraft_core::units::PointNm {
    pcbcraft_core::units::PointNm::from_mm_f64_rounded(p).unwrap_or_default()
}

const STEM: &str = "pkgT";

fn wroom3() -> PackageDefinition {
    // The fixture is the module datasheet's millimetres: the wire form.
    let form: PackageForm = serde_json::from_str::<
        pcbcraft_core::library::package_form::PackageFormParams,
    >(include_str!("template_spike_wroom3.json"))
    .unwrap()
    .into();
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
    let [cx, cy] = form.can_centre();
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
    let antenna_from = form.length() / 2.0 - form.antenna_length().min(form.length());
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

/// The Omron B3U-1000P top-actuated tactile switch of the ESP32-S31 dev
/// board example: a black housing, a metal cover, the round black plunger
/// and two tin-plated terminals, one each side along x. Dimensions from
/// Omron's public B3U catalog (en-b3u.pdf, p.2, Dimensions, B3U-1000P(M)):
/// housing 3 x 2.5 mm, 1.2 mm to the cover, 1.6 mm over the 1.5 mm
/// plunger, 4 mm over the terminals, terminals 1.4 mm wide. The drawing
/// gives no cover or terminal thickness; those two are illustrative. No
/// CAD or EDA library data was copied.
pub fn author_b3u_1000p() -> Vec<u8> {
    let mut s = new_part(
        "Omron B3U-1000P tactile switch",
        &[
            ("length", "3 mm", "mm", "housing size along x"),
            ("width", "2.5 mm", "mm", "housing size along y"),
            (
                "body_height",
                "1.2 mm",
                "mm",
                "seating plane to the cover top",
            ),
            ("height", "1.6 mm", "mm", "seating plane to the plunger top"),
            ("plunger_d", "1.5 mm", "mm", "plunger diameter"),
            (
                "terminal_span",
                "4 mm",
                "mm",
                "size along x over the terminals",
            ),
            ("terminal_width", "1.4 mm", "mm", "terminal size along y"),
            (
                "cover_thickness",
                "0.1 mm",
                "mm",
                "metal cover thickness (illustrative)",
            ),
            (
                "terminal_thickness",
                "0.15 mm",
                "mm",
                "terminal thickness (illustrative)",
            ),
        ],
        ["length * 0.6", "width * 0.6"],
    );
    run(
        &mut s,
        "solid.box",
        json!({"corner": ["-length / 2", "-width / 2", "0 mm"], "length": "length",
            "width": "width", "height": "body_height - cover_thickness", "name": "Housing",
            "body_name": "Housing"}),
    );
    run(
        &mut s,
        "solid.box",
        json!({"corner": ["-length / 2", "-width / 2", "body_height - cover_thickness"],
            "length": "length", "width": "width", "height": "cover_thickness", "name": "Cover",
            "body_name": "Cover"}),
    );
    run(
        &mut s,
        "solid.cylinder",
        json!({"diameter": "plunger_d", "height": "height - body_height",
            "base": ["0 mm", "0 mm", "body_height"], "name": "Plunger", "body_name": "Plunger"}),
    );
    for (name, x) in [
        ("Terminal 1", "length / 2"),
        ("Terminal 2", "-terminal_span / 2"),
    ] {
        run(
            &mut s,
            "solid.box",
            json!({"corner": [x, "-terminal_width / 2", "0 mm"],
                "length": "(terminal_span - length) / 2", "width": "terminal_width",
                "height": "terminal_thickness", "name": name, "body_name": name}),
        );
    }
    assign_look(&mut s, &["Housing"], MaterialId::Lcp);
    assign_look(&mut s, &["Cover"], MaterialId::NickelSilver);
    assign_look(&mut s, &["Plunger"], MaterialId::Pa9t);
    assign_look(&mut s, &["Terminal 1", "Terminal 2"], MaterialId::TinMatte);
    finish_part(s, "body_height")
}

/// In the active sketch: a stadium (a slot with fully rounded ends), its
/// straight sides along the sketch's u axis, centred on its v axis at `v`,
/// `length` overall and `high` across (expressions over the design's
/// parameters; `at` their values now). Every point is placed by
/// expressions, so the profile follows the parameters. `tag` keeps the
/// point ids of two stadiums in one sketch apart.
fn stadium(s: &mut Session, tag: &str, [length, high, v]: [&str; 3], at: [f64; 3]) {
    let [l, h, v0] = at;
    let r = format!("(({high}) / 2)");
    let flat = format!("(({length}) / 2 - {r})");
    let (f, rr) = (l / 2.0 - h / 2.0, h / 2.0);
    let points = [
        (
            "bl",
            format!("-{flat}"),
            format!("({v}) - {r}"),
            [-f, v0 - rr],
        ),
        ("br", flat.clone(), format!("({v}) - {r}"), [f, v0 - rr]),
        ("tr", flat.clone(), format!("({v}) + {r}"), [f, v0 + rr]),
        (
            "tl",
            format!("-{flat}"),
            format!("({v}) + {r}"),
            [-f, v0 + rr],
        ),
        ("cr", flat.clone(), format!("({v})"), [f, v0]),
        ("cl", format!("-{flat}"), format!("({v})"), [-f, v0]),
    ];
    for (id, x, y, _) in &points {
        run(
            s,
            "sketch.point",
            json!({"point": [x, y], "id": format!("{tag}{id}")}),
        );
    }
    let p = |id: &str| format!("{tag}{id}");
    run(
        s,
        "sketch.line",
        json!({"points": [p("bl"), p("br")], "infer": false}),
    );
    run(
        s,
        "sketch.arc.center_point",
        json!({"center": p("cr"), "start": p("br"), "end": p("tr")}),
    );
    run(
        s,
        "sketch.line",
        json!({"points": [p("tr"), p("tl")], "infer": false}),
    );
    run(
        s,
        "sketch.arc.center_point",
        json!({"center": p("cl"), "start": p("tl"), "end": p("bl")}),
    );
}

/// The GCT USB4105-GF-A USB-C receptacle of the ESP32-S31 dev board
/// example, from GCT's public USB4105 drawing (rev B4, sheet 1):
///
/// - **Shell**: a nickel stadium tube, open at the mating face (−y), from a
///   sketch of its outer and inner outline extruded over the depth. The
///   outline is 8.94 × 7.35 × 3.31 mm and the 8.34 × 2.56 mm opening is
///   centred 1.73 mm up; the 0.30 mm sheet and the outer stadium (3.16 mm
///   high, from 0.15 mm up) follow from those. Its four stakes stand in the
///   plane of the side walls, joined to it: the front pair centred 2.60 mm
///   from the mating face and 1.10 mm long, the rear pair 4.18 mm behind.
///   Only the part above the seating plane is drawn (a part file's lowest
///   geometry is the seating plane), from z = 0 up to the opening's centre,
///   where they meet the rounded walls.
/// - **Insulator** (LCP) closing the shell's rear behind the cavity, and the
///   **Tongue** (LCP), a plate with rounded corners centred in the opening.
/// - Twelve gold solder **tails** in the rear row, at the drawing's x
///   positions and widths (0.20 mm signal, 0.45 mm shared VBUS and GND).
/// - The **`opening` datum** (slice B2): a construction plane on the
///   mating face at the opening's centre, its normal out along the plug's
///   way in, sized by `opening_u` × `opening_v` with round ends
///   (`opening_r`) and `opening_depth` deep: the plug's shell, which fills
///   the 8.34 × 2.56 mm opening, out to the drawing's mating view's
///   "1.85 Min" where the plug's overmold begins.
///
/// Origin at the outline centre, as the footprint's. The shell's front
/// face (the extrusion's start face, `F2:start` of the Shell body) is what
/// the board holds flush to its edge. Illustrative, as their parameters
/// say: the cavity depth, the tongue, the rear stakes' length and the
/// tails' thickness, reach and run under the shell. No CAD or EDA library
/// data was copied.
pub fn author_usb4105() -> Vec<u8> {
    let mut s = new_part(
        "GCT USB4105-GF-A USB-C receptacle",
        &[
            ("width", "8.94 mm", "mm", "shell across (x)"),
            (
                "depth",
                "7.35 mm",
                "mm",
                "shell deep (y), mating face at -y",
            ),
            ("height", "3.31 mm", "mm", "seated height"),
            ("opening_w", "8.34 mm", "mm", "mating opening across"),
            ("opening_h", "2.56 mm", "mm", "mating opening high"),
            ("opening_z", "1.73 mm", "mm", "mating opening centre height"),
            (
                "shell_t",
                "(width - opening_w) / 2",
                "mm",
                "shell sheet thickness",
            ),
            (
                "cavity_depth",
                "6.2 mm",
                "mm",
                "how deep the opening reaches (illustrative)",
            ),
            ("tongue_w", "6.7 mm", "mm", "tongue across (illustrative)"),
            (
                "tongue_t",
                "0.7 mm",
                "mm",
                "tongue thickness (illustrative)",
            ),
            (
                "tongue_recess",
                "0.6 mm",
                "mm",
                "tongue tip behind the mating face (illustrative)",
            ),
            (
                "tongue_r",
                "0.3 mm",
                "mm",
                "tongue corner radius (illustrative)",
            ),
            (
                "stake_front_y",
                "2.6 mm",
                "mm",
                "front stake centre from the mating face",
            ),
            (
                "stake_pitch",
                "4.18 mm",
                "mm",
                "front to rear stake centres",
            ),
            ("stake_len", "1.1 mm", "mm", "front stake length along y"),
            (
                "rear_stake_len",
                "stake_len",
                "mm",
                "rear stake length along y (illustrative)",
            ),
            ("pitch", "0.5 mm", "mm", "signal tail pitch"),
            ("signal_tail_w", "0.2 mm", "mm", "signal tail width"),
            (
                "power_tail_w",
                "0.45 mm",
                "mm",
                "shared VBUS and GND tail width",
            ),
            ("vbus_x", "2.4 mm", "mm", "VBUS tail centres from x = 0"),
            ("gnd_x", "3.2 mm", "mm", "GND tail centres from x = 0"),
            ("tail_t", "0.15 mm", "mm", "tail thickness (illustrative)"),
            (
                "tail_reach",
                "0.38 mm",
                "mm",
                "how far the tails reach past the rear (illustrative)",
            ),
            (
                "tail_under",
                "0.6 mm",
                "mm",
                "how far the tails run under the shell (illustrative)",
            ),
            // The `opening` datum (slice B2): the aperture the mating
            // plug needs is the plug's shell, which fills the receptacle's
            // opening, out to where the plug's overmold begins.
            (
                "opening_u",
                "opening_w",
                "mm",
                "opening datum across: the plug's shell",
            ),
            (
                "opening_v",
                "opening_h",
                "mm",
                "opening datum high: the plug's shell",
            ),
            (
                "opening_r",
                "opening_h / 2",
                "mm",
                "opening datum corner radius: round ends",
            ),
            (
                "opening_depth",
                "1.85 mm",
                "mm",
                "opening datum depth: the plug's shell before its overmold (mating view, 1.85 min)",
            ),
        ],
        ["width * 0.6", "depth * 0.6"],
    );
    // The opening plane on the mating face at the opening's centre: x
    // across, y up, so its normal (x × y) is −y, out along the plug's way
    // in.
    run(
        &mut s,
        "construct.plane.offset",
        json!({"base": {"origin": ["0 mm", "-depth / 2", "opening_z"], "x_dir": [1, 0, 0],
            "y_dir": [0, 0, 1]}, "offset": "0 mm", "name": "opening"}),
    );
    let (w, h, ow, oh, oz) = (8.94, 3.31, 8.34, 2.56, 1.73);
    let opening = ["opening_w", "opening_h", "-opening_z"];
    // The XZ plane maps sketch (u, v) to (u, offset, −v), normal +y: the
    // shell's sketch lies in the mating face and is extruded back.
    run(
        &mut s,
        "sketch.create",
        json!({"plane": "XZ", "offset": "-depth / 2", "name": "Shell profile"}),
    );
    stadium(
        &mut s,
        "o",
        ["width", "2 * (height - opening_z)", "-opening_z"],
        [w, 2.0 * (h - oz), -oz],
    );
    stadium(&mut s, "i", opening, [ow, oh, -oz]);
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "solid.extrude",
        json!({"sketch": "Shell profile", "distance": "depth", "operation": "new",
            "name": "Shell", "body_name": "Shell"}),
    );
    // The stakes, in the side walls' plane, joined to the shell.
    let rear = "stake_front_y + stake_pitch";
    for (name, x, y, len) in [
        ("Stake 1", "-width / 2", "stake_front_y", "stake_len"),
        (
            "Stake 2",
            "width / 2 - shell_t",
            "stake_front_y",
            "stake_len",
        ),
        ("Stake 3", "-width / 2", rear, "rear_stake_len"),
        ("Stake 4", "width / 2 - shell_t", rear, "rear_stake_len"),
    ] {
        run(
            &mut s,
            "solid.box",
            json!({"corner": [x, format!("-depth / 2 + {y} - {len} / 2"), "0 mm"],
                "length": "shell_t", "width": len, "height": "opening_z",
                "operation": "join", "targets": ["Shell"], "name": name}),
        );
    }
    // The insulator closing the rear: the opening's stadium, behind the
    // cavity.
    run(
        &mut s,
        "sketch.create",
        json!({"plane": "XZ", "offset": "-depth / 2 + cavity_depth", "name": "Insulator profile"}),
    );
    stadium(&mut s, "i", opening, [ow, oh, -oz]);
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "solid.extrude",
        json!({"sketch": "Insulator profile", "distance": "depth - cavity_depth",
            "operation": "new", "name": "Insulator", "body_name": "Insulator"}),
    );
    // The tongue: a rounded rectangle at its lower face, from its tip to
    // the insulator, extruded up its thickness.
    run(
        &mut s,
        "sketch.create",
        json!({"plane": "XY", "offset": "opening_z - tongue_t / 2", "name": "Tongue profile"}),
    );
    run(
        &mut s,
        "sketch.rectangle.rounded",
        json!({"p0": ["-tongue_w / 2", "-depth / 2 + tongue_recess"],
            "p1": ["tongue_w / 2", "-depth / 2 + cavity_depth"], "radius": "tongue_r"}),
    );
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "solid.extrude",
        json!({"sketch": "Tongue profile", "distance": "tongue_t", "operation": "new",
            "name": "Tongue", "body_name": "Tongue"}),
    );
    // The solder tails in the drawing's x order (the footprint's lands).
    let mut centres: Vec<(String, &str)> = vec![
        ("-gnd_x".into(), "power_tail_w"),
        ("-vbus_x".into(), "power_tail_w"),
    ];
    for k in 0..8 {
        centres.push((format!("({k} - 3.5) * pitch"), "signal_tail_w"));
    }
    centres.push(("vbus_x".into(), "power_tail_w"));
    centres.push(("gnd_x".into(), "power_tail_w"));
    let mut tails = Vec::new();
    for ((x, tw), designation) in centres.iter().zip([
        "A1B12", "A4B9", "B8", "A5", "B7", "A6", "A7", "B6", "A8", "B5", "B4A9", "B1A12",
    ]) {
        let name = format!("Tail {designation}");
        run(
            &mut s,
            "solid.box",
            json!({"corner": [format!("{x} - {tw} / 2"), "depth / 2 - tail_under", "0 mm"],
                "length": tw, "width": "tail_under + tail_reach", "height": "tail_t",
                "name": name, "body_name": name}),
        );
        tails.push(name);
    }
    let tails: Vec<&str> = tails.iter().map(String::as_str).collect();
    assign_look(&mut s, &["Shell"], MaterialId::Nickel);
    assign_look(&mut s, &["Insulator", "Tongue"], MaterialId::Lcp);
    assign_look(&mut s, &tails, MaterialId::Gold);
    finish_part(s, "height")
}

#[test]
#[ignore = "writes library/parts/*.solidcraft and the examples' part files"]
fn author_parts() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    type Author = fn() -> Vec<u8>;
    let parts: [(&str, &str, Author); 6] = [
        ("library/parts", "esp32-s31-wroom-3", author_wroom3),
        ("library/parts", "smd-nut", author_smd_nut),
        ("library/parts", "round-spacer", author_round_spacer),
        ("library/parts", "finned-heatsink", author_finned_heatsink),
        // The ESP32 dev board example's switches: the example's own part
        // file, not a built-in.
        (
            "examples/esp32-s31-devboard/parts",
            "b3u-1000p",
            author_b3u_1000p,
        ),
        // Its USB-C receptacle J1, held flush to the board edge by the
        // shell's front face.
        (
            "examples/esp32-s31-devboard/parts",
            "usb4105",
            author_usb4105,
        ),
    ];
    for (folder, name, author) in parts {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
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
