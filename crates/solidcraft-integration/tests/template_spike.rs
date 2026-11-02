//! Spike for docs/part-templates.md: a part template is a `.solidcraft`
//! design; inserting it into a board design keeps its feature history and
//! parameters (prefixed per definition). Measures cold and cached cost and
//! compares the transplanted bodies with the generated builder's.
//!
//! The "template" here is the generated module / QFN model saved as its
//! own design with its parameters renamed to plain template names, which
//! is what an authored template would look like to the insert path.
//!
//! `cargo test --release -p pcbcraft-solidcraft --test template_spike -- --ignored --nocapture`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pcbcraft_core::ids::PackageTerminalId;
use pcbcraft_core::library::{
    GeneratedMaterials, LeadSides, NoLeadForm, PackageDefinition, PackageForm, PackageModel,
    PackageTerminal,
};
use pcbcraft_solidcraft::models;
use pcbcraft_solidcraft::native::Native;
use serde_json::json;
use solidcraft_engine::Session;
use solidcraft_engine::doc::Document;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const STEM: &str = "pkgT";

fn package(form: PackageForm, terminals: u32) -> PackageDefinition {
    PackageDefinition {
        next_local: terminals + 1,
        terminals: (1..=terminals)
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

fn wroom3() -> PackageDefinition {
    let form: PackageForm =
        serde_json::from_str(include_str!("template_spike_wroom3.json")).unwrap();
    package(form, 99)
}

fn qfn48() -> PackageDefinition {
    package(
        PackageForm::NoLead(Box::new(NoLeadForm {
            sides: LeadSides::Four,
            terminals_per_side: [12, 12],
            pitch: 0.5,
            body_length: 7.0,
            body_width: 7.0,
            body_height: 0.85,
            terminal_length: 0.4,
            terminal_width: 0.25,
            exposed_pad: Some([5.6, 5.6]),
        })),
        49,
    )
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// The generated build (today's path) and its design as template bytes.
struct Generated {
    build: Duration,
    native: Native,
    bodies: Vec<String>,
    template: Vec<u8>,
}

fn generate(pkg: &PackageDefinition) -> Generated {
    let t = Instant::now();
    let mut native = Native::new("template").unwrap();
    let def = models::build_definition(&mut native, STEM, pkg).unwrap();
    let build = t.elapsed();
    let mut doc: Document = (*native.session().doc).clone();
    // Template parameter names: the definition stem removed.
    let names: Vec<String> = doc
        .params
        .iter()
        .filter(|p| p.name.starts_with(&format!("{STEM}_")))
        .map(|p| p.name.clone())
        .collect();
    for n in names {
        let plain = n[STEM.len() + 1..].to_owned();
        doc.rename_param(&n, &plain).unwrap();
    }
    let template = solidcraft_engine::io::write_design(&doc);
    Generated {
        build,
        native,
        bodies: def.bodies.iter().map(|b| b.name.clone()).collect(),
        template,
    }
}

/// Insert the template's history as a new component `prefix` of `board`:
/// its parameters prefixed (`<prefix>_<name>`, overrides applied), its
/// features pasted into the component. Returns the time taken.
fn transplant(
    board: &mut Session,
    template: &Document,
    prefix: &str,
    overrides: &BTreeMap<&str, &str>,
    namespace_bodies: bool,
) -> Duration {
    let t = Instant::now();
    let mut src = template.clone();
    let names: Vec<String> = src.params.iter().map(|p| p.name.clone()).collect();
    for n in &names {
        src.rename_param(n, &format!("{prefix}_{n}")).unwrap();
    }
    let rows: Vec<serde_json::Value> = src
        .params
        .iter()
        .filter(|p| !p.model)
        .map(|p| {
            let plain = &p.name[prefix.len() + 1..];
            let expr = overrides
                .get(plain)
                .map_or(p.expr.clone(), |e| (*e).to_owned());
            json!({"name": p.name, "expression": expr, "unit": p.unit})
        })
        .collect();
    if !rows.is_empty() {
        board
            .execute(
                "parameters.import",
                &json!({"text": serde_json::to_string(&rows).unwrap()}),
            )
            .unwrap();
    }
    board
        .execute("component.activate", &json!({"component": "root"}))
        .unwrap();
    let made = board
        .execute(
            "component.create",
            &json!({"name": prefix, "grounded": false}),
        )
        .unwrap();
    let comp = made["component"].as_u64().unwrap();
    // The template's definition component's features (the generated
    // design keeps them in its `pkgT` component, an authored template in
    // its root).
    let from = src
        .components
        .iter()
        .find(|c| c.name == STEM)
        .map_or(0, |c| c.id);
    let mut features: Vec<solidcraft_engine::doc::Feature> = src
        .features
        .iter()
        .filter(|f| f.component == from)
        .cloned()
        .collect();
    if namespace_bodies {
        // Stand-in for a fork API: every string that is exactly a body
        // name of the template becomes `<prefix> <name>`.
        let mut json = serde_json::to_value(&features).unwrap();
        rename_strings(&mut json, &|s: &str| {
            s.strip_prefix(&format!("{STEM} "))
                .map(|rest| format!("{prefix} {rest}"))
        });
        features = serde_json::from_value(json).unwrap();
    }
    board.clipboard = features;
    let reply = board
        .execute("timeline.paste", &json!({"component": comp}))
        .unwrap();
    let elapsed = t.elapsed();
    if let Some(n) = reply.get("notices") {
        eprintln!("  paste notices: {n}");
    }
    board
        .execute("component.activate", &json!({"component": "root"}))
        .unwrap();
    elapsed
}

/// The insert the note specifies (`component.insert_history`), done on
/// the document directly: every template feature cloned with a typed remap
/// (`Feature::remap_clone`) of feature ids, feature names, body names (the
/// template's evaluated names, `<prefix> <name>`) and parameter names
/// (`<prefix>_<name>`), parameters copied with their expressions renamed,
/// one evaluation at the end.
fn insert_history(
    board: &mut Session,
    template: &Session,
    prefix: &str,
    overrides: &BTreeMap<&str, &str>,
) -> Duration {
    use solidcraft_engine::doc::clone_expression;
    let t = Instant::now();
    let src = &*template.doc;
    let from = src
        .components
        .iter()
        .find(|c| c.name == STEM)
        .map_or(0, |c| c.id);
    let rename_body = |n: &str| match n.strip_prefix(&format!("{STEM} ")) {
        Some(rest) => format!("{prefix} {rest}"),
        None => format!("{prefix} {n}"),
    };
    let parameters: BTreeMap<String, String> = src
        .param_defs()
        .into_iter()
        .map(|d| (d.name.clone(), format!("{prefix}_{}", d.name)))
        .collect();
    let mut bodies: BTreeMap<String, String> = BTreeMap::new();
    for b in &template.world_state().bodies {
        bodies.insert(b.name.clone(), rename_body(&b.name));
    }
    let feats: Vec<&solidcraft_engine::doc::Feature> = src
        .features
        .iter()
        .filter(|f| f.component == from)
        .collect();
    for f in &feats {
        for n in f.body_names.iter().chain(&f.participants) {
            bodies.entry(n.clone()).or_insert_with(|| rename_body(n));
        }
    }
    board
        .execute("component.activate", &json!({"component": "root"}))
        .unwrap();
    let made = board
        .execute(
            "component.create",
            &json!({"name": prefix, "grounded": false}),
        )
        .unwrap();
    let comp = made["component"].as_u64().unwrap();
    let doc = board.doc_mut();
    let mut next = doc.next_id.max(1);
    let mut ids = BTreeMap::new();
    let mut names = BTreeMap::new();
    for f in &feats {
        ids.insert(f.id, next);
        next += 1;
        names.insert(f.name.clone(), format!("{prefix} {}", f.name));
    }
    doc.next_id = next;
    for p in &src.params {
        let mut q = p.clone();
        q.name = parameters[&p.name].clone();
        q.expr = match overrides.get(p.name.as_str()) {
            Some(e) => (*e).to_owned(),
            None => clone_expression(&p.expr, &parameters),
        };
        doc.params.push(q);
    }
    for f in feats {
        let mut g = f.clone();
        g.remap_clone(&ids, &names, &bodies, &parameters).unwrap();
        g.id = ids[&f.id];
        g.name = names[&f.name].clone();
        g.component = comp;
        doc.features.push(g);
    }
    board.refresh();
    let elapsed = t.elapsed();
    board
        .execute("component.activate", &json!({"component": "root"}))
        .unwrap();
    elapsed
}

fn ov_of<'a>(change: (&'a str, &'a str)) -> BTreeMap<&'a str, &'a str> {
    let mut m = BTreeMap::new();
    m.insert(change.0, change.1);
    m
}

fn rename_strings(v: &mut serde_json::Value, f: &dyn Fn(&str) -> Option<String>) {
    match v {
        serde_json::Value::String(s) => {
            if let Some(n) = f(s) {
                *s = n;
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(|x| rename_strings(x, f)),
        serde_json::Value::Object(o) => o.values_mut().for_each(|x| rename_strings(x, f)),
        _ => {}
    }
}

fn errors(s: &Session) -> Vec<String> {
    s.model
        .results
        .iter()
        .filter_map(|r| r.error.clone().map(|e| format!("{}: {e}", r.name)))
        .collect()
}

/// Per-body bounding-box and volume difference, by body name.
fn compare(a: &Session, b: &Session, bodies: &[String], rename: &dyn Fn(&str) -> String) {
    let (wa, wb) = (a.world_state(), b.world_state());
    let mut worst_box = 0.0f64;
    let mut worst_vol = 0.0f64;
    let mut missing = Vec::new();
    for name in bodies {
        let Some(x) = wa.bodies.iter().find(|m| &m.name == name) else {
            missing.push(format!("generated {name}"));
            continue;
        };
        let other = rename(name);
        let Some(y) = wb.bodies.iter().find(|m| m.name == other) else {
            missing.push(format!("transplanted {other}"));
            continue;
        };
        let (mx, my) = (x.measure().unwrap(), y.measure().unwrap());
        for (p, q) in [(mx.bbox.min, my.bbox.min), (mx.bbox.max, my.bbox.max)] {
            worst_box = worst_box
                .max((p.x - q.x).abs())
                .max((p.y - q.y).abs())
                .max((p.z - q.z).abs());
        }
        worst_vol = worst_vol.max((mx.volume - my.volume).abs() / mx.volume.abs().max(1e-12));
    }
    eprintln!(
        "  parity: {} bodies (generated {}, transplanted {}), worst bbox delta {:.2e} mm, \
         worst relative volume delta {:.2e}, missing {:?}",
        bodies.len(),
        wa.bodies.len(),
        wb.bodies.len(),
        worst_box,
        worst_vol,
        &missing[..missing.len().min(3)]
    );
    if !missing.is_empty() {
        let names: Vec<&str> = wb.bodies.iter().take(6).map(|m| m.name.as_str()).collect();
        eprintln!("  transplanted names: {names:?}");
    }
}

fn run(label: &str, pkg: &PackageDefinition, change: (&str, &str), namespace: bool) {
    eprintln!("== {label} (body names namespaced: {namespace})");
    let g = generate(pkg);
    eprintln!(
        "  generated build (Native::new + build_definition, evaluated per command): {:.1} ms, {} bodies, template {} KiB",
        ms(g.build),
        g.bodies.len(),
        g.template.len() / 1024
    );
    // Cold: the template from bytes, evaluated on its own.
    let t = Instant::now();
    let doc = solidcraft_engine::io::read_design(&g.template).unwrap();
    let read = t.elapsed();
    let t = Instant::now();
    let mut alone = Session::new(doc.clone());
    let eval = t.elapsed();
    assert!(errors(&alone).is_empty(), "{:?}", errors(&alone));
    eprintln!(
        "  cold: read {:.1} ms + evaluate {:.1} ms ({} features)",
        ms(read),
        ms(eval),
        doc.features.len()
    );
    let mut slow: Vec<(String, f64)> = alone
        .model
        .results
        .iter()
        .map(|r| (r.name.clone(), r.ms))
        .collect();
    slow.sort_by(|a, b| b.1.total_cmp(&a.1));
    eprintln!("  slowest features (ms): {:?}", &slow[..slow.len().min(3)]);
    // Cold from a container that carries its evaluated model (the
    // template's default parameter set): Solidcraft adopts it.
    let path = std::env::temp_dir().join(format!(
        "template-spike-{}-{}.sc",
        std::process::id(),
        doc.features.len()
    ));
    let _ = std::fs::remove_file(&path);
    let mut saver = Session::new(doc.clone());
    saver
        .execute(
            "file.save_as",
            &json!({"path": path.to_string_lossy(), "format": "container", "cache": true, "preview": false}),
        )
        .unwrap();
    let container = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let t = Instant::now();
    let mut reader =
        solidcraft_engine::io::DesignContainerReader::new(std::io::Cursor::new(&container))
            .unwrap();
    let saved = reader.geometry();
    let cached_doc = reader.read_document().unwrap();
    let adopted = Session::opened(cached_doc, saved.as_ref());
    eprintln!(
        "  cold from a container with its saved model: {:.1} ms (model cache {}, {} KiB, errors {})",
        ms(t.elapsed()),
        if saved.is_some() { "adopted" } else { "absent" },
        container.len() / 1024,
        errors(&adopted).len()
    );
    // Tessellation of every body (display meshes), cold.
    let t = Instant::now();
    let tris: usize = alone
        .world_state()
        .bodies
        .iter()
        .map(|b| b.mesh().triangles.len())
        .sum();
    eprintln!("  tessellate: {:.1} ms, {tris} triangles", ms(t.elapsed()));
    // A parameter override on the template alone (incremental).
    let t = Instant::now();
    alone
        .execute(
            "parameters.change",
            &json!({"changes": {change.0: change.1}}),
        )
        .unwrap();
    let changed = t.elapsed();
    assert!(errors(&alone).is_empty(), "{:?}", errors(&alone));
    let original = doc
        .params
        .iter()
        .find(|p| p.name == change.0)
        .map(|p| p.expr.clone())
        .unwrap();
    let t = Instant::now();
    alone
        .execute(
            "parameters.change",
            &json!({"changes": {change.0: original}}),
        )
        .unwrap();
    let back = t.elapsed();
    eprintln!(
        "  override {}={}: {:.1} ms; set back (evaluation cache): {:.1} ms",
        change.0,
        change.1,
        ms(changed),
        ms(back)
    );
    // The typed insert (no paste): two parameter sets side by side.
    let mut typed = Session::default();
    typed
        .execute("file.new", &json!({"name": "board"}))
        .unwrap();
    let a = insert_history(&mut typed, &alone, "pkg7r1", &BTreeMap::new());
    let errs = errors(&typed);
    eprintln!(
        "  typed insert 1: {:.1} ms, errors {:?}, bodies {}",
        ms(a),
        &errs[..errs.len().min(3)],
        typed.world_state().bodies.len()
    );
    compare(
        &g.native.session().clone_for_spike(),
        &typed,
        &g.bodies,
        &|n: &str| n.replacen(STEM, "pkg7r1", 1),
    );
    let b2 = insert_history(&mut typed, &alone, "pkg8r1", &ov_of(change));
    let errs = errors(&typed);
    eprintln!(
        "  typed insert 2 ({}={}): {:.1} ms, errors {:?}, bodies {}",
        change.0,
        change.1,
        ms(b2),
        &errs[..errs.len().min(3)],
        typed.world_state().bodies.len()
    );
    let t = Instant::now();
    typed
        .execute(
            "parameters.change",
            &json!({"changes": {format!("pkg8r1_{}", change.0): original}}),
        )
        .unwrap();
    eprintln!(
        "  typed board: definition 2 set to the default (no feature result reused): {:.1} ms, errors {}",
        ms(t.elapsed()),
        errors(&typed).len()
    );
    let t = Instant::now();
    typed
        .execute(
            "parameters.change",
            &json!({"changes": {format!("pkg8r1_{}", change.0): change.1}}),
        )
        .unwrap();
    eprintln!(
        "  typed board: definition 2 back to the override (evaluation cache): {:.1} ms",
        ms(t.elapsed()),
    );
    let bytes = solidcraft_engine::io::write_design(&typed.doc);
    let t = Instant::now();
    let reopened = Session::new(solidcraft_engine::io::read_design(&bytes).unwrap());
    eprintln!(
        "  typed board reopen + evaluate: {:.1} ms, errors {}, {} KiB",
        ms(t.elapsed()),
        errors(&reopened).len(),
        bytes.len() / 1024
    );
    if namespace {
        return;
    }
    // Transplant into a board design: history and parameters kept.
    let mut board = Session::default();
    board
        .execute("file.new", &json!({"name": "board"}))
        .unwrap();
    let first = transplant(&mut board, &doc, "pkg7r1", &BTreeMap::new(), namespace);
    assert!(errors(&board).is_empty(), "{:?}", errors(&board));
    eprintln!(
        "  transplant 1 (params import + component + paste + evaluate): {:.1} ms, board {} features, {} params",
        ms(first),
        board.doc.features.len(),
        board.doc.params.len()
    );
    let rename = |n: &str| {
        if namespace {
            n.replacen(STEM, "pkg7r1", 1)
        } else {
            n.to_owned()
        }
    };
    compare(
        &g.native.session().clone_for_spike(),
        &board,
        &g.bodies,
        &rename,
    );
    // The inserted definition stays parametric in the board.
    let t = Instant::now();
    board
        .execute(
            "parameters.change",
            &json!({"changes": {format!("pkg7r1_{}", change.0): change.1}}),
        )
        .unwrap();
    assert!(errors(&board).is_empty(), "{:?}", errors(&board));
    eprintln!(
        "  board parameter change pkg7r1_{}: {:.1} ms",
        change.0,
        ms(t.elapsed())
    );
    // A second parameter set: another definition.
    let mut ov = BTreeMap::new();
    ov.insert(change.0, change.1);
    let second = transplant(&mut board, &doc, "pkg8r1", &ov, namespace);
    let errs = errors(&board);
    eprintln!(
        "  transplant 2 (other parameter set): {:.1} ms, errors {:?}, bodies {}",
        ms(second),
        &errs[..errs.len().min(3)],
        board.world_state().bodies.len()
    );
    // Placements of a definition: shared geometry.
    let comp = board
        .doc
        .components
        .iter()
        .find(|c| c.name == "pkg7r1")
        .map(|c| c.id)
        .unwrap();
    let offsets: Vec<[f64; 3]> = (1..=100).map(|k| [f64::from(k) * 40.0, 0.0, 0.0]).collect();
    let t = Instant::now();
    board
        .execute(
            "occurrence.copy",
            &json!({"component": comp, "translations": offsets}),
        )
        .unwrap();
    eprintln!(
        "  100 more placements of definition 1: {:.1} ms",
        ms(t.elapsed())
    );
    // Save/reopen the board: still parametric.
    let bytes = solidcraft_engine::io::write_design(&board.doc);
    let t = Instant::now();
    let reopened = Session::new(solidcraft_engine::io::read_design(&bytes).unwrap());
    eprintln!(
        "  board reopen + evaluate: {:.1} ms, errors {}, params with prefix pkg7r1_: {}",
        ms(t.elapsed()),
        errors(&reopened).len(),
        reopened
            .doc
            .params
            .iter()
            .filter(|p| p.name.starts_with("pkg7r1_"))
            .count()
    );
}

trait CloneForSpike {
    fn clone_for_spike(&self) -> Session;
}

impl CloneForSpike for Session {
    fn clone_for_spike(&self) -> Session {
        Session::new((*self.doc).clone())
    }
}

#[test]
#[ignore = "spike: measures template insertion"]
fn template_insert_spike() {
    for namespace in [false, true] {
        run(
            "ESP32-S31-WROOM-3 (module)",
            &wroom3(),
            ("can_height", "3 mm"),
            namespace,
        );
        run(
            "QFN-48 7x7 (no-lead)",
            &qfn48(),
            ("body_height", "1 mm"),
            namespace,
        );
    }
}
