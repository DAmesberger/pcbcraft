//! Spike F0 evidence. The tests marked `wasm_bindgen_test` also run in Node
//! (`wasm32-unknown-unknown`) and assert the same golden hashes as natively:
//! that is the native/wasm agreement check.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use solidcraft_engine::sketch::{Sketch, find_profiles, profile_at_point};

use super::fixtures::{self, BGA256_PKG, QFN48_PKG, SOIC8_PKG};
use super::{Evaluation, Finding, PadKey, PadOut, PadShape, Source, check, evaluate, finding};

type R = Result<(), Finding>;

fn pad<'a>(e: &'a Evaluation, key: &PadKey) -> Result<&'a PadOut, Finding> {
    e.pads
        .iter()
        .find(|p| &p.key == key)
        .ok_or_else(|| finding("test", format!("no pad {key:?}")))
}

fn key(array: &str, i: u16, j: u16) -> PadKey {
    PadKey::Array {
        array: array.into(),
        i,
        j,
    }
}

fn keys(e: &Evaluation) -> BTreeSet<PadKey> {
    e.pads.iter().map(|p| p.key.clone()).collect()
}

fn nm(mm: f64) -> i64 {
    (mm * 1e6).round() as i64
}

/// Golden hashes recorded natively (macOS arm64, ci profile) and asserted on
/// every target: the quantized pads, and the raw f64 bits of every solved
/// sketch point (bit-for-bit solver agreement).
const GOLDEN: [(&str, u64, u64); 3] = [
    ("SOIC-8", 0x48206f04aada3221, 0x3db9a58e56d62319),
    ("QFN-48+EP", 0x1e1915d0397fbec5, 0xd15aaed38c07bd4e),
    ("BGA-256", 0x61a720bf1bce8965, 0x1bd4d84c6beaade5),
];

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn soic8_evaluates_to_eight_typed_pads() -> R {
    let e = evaluate(&fixtures::soic8()?, SOIC8_PKG)?;
    assert_eq!(e.pads.len(), 8);
    assert_eq!(e.dof, 0, "the fixture is fully constrained");
    let first = pad(&e, &key("rows", 0, 0))?;
    assert_eq!(first.number, "1");
    assert_eq!(first.centre_nm, [nm(-2.7), nm(1.905)]);
    assert_eq!(
        first.shape,
        PadShape::Rect {
            w_nm: nm(1.55),
            h_nm: nm(0.6)
        }
    );
    assert_eq!(first.rotation_udeg, 0);
    let fourth = pad(&e, &key("rows", 0, 3))?;
    assert_eq!(
        (fourth.number.as_str(), fourth.centre_nm),
        ("4", [nm(-2.7), nm(-1.905)])
    );
    // Row 1 is row 0 turned half a turn: pin 5 is bottom right, pin 8 top right.
    let fifth = pad(&e, &key("rows", 1, 0))?;
    assert_eq!(
        (fifth.number.as_str(), fifth.centre_nm, fifth.rotation_udeg),
        ("5", [nm(2.7), nm(-1.905)], 180_000_000)
    );
    assert_eq!(pad(&e, &key("rows", 1, 3))?.centre_nm, [nm(2.7), nm(1.905)]);
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn qfn48_evaluates_quad_and_exposed_pad() -> R {
    let e = evaluate(&fixtures::qfn48()?, QFN48_PKG)?;
    assert_eq!(e.pads.len(), 49);
    assert_eq!(e.dof, 0);
    let ep = pad(&e, &PadKey::Anchor("ep".into()))?;
    assert_eq!((ep.number.as_str(), ep.centre_nm), ("EP", [0, 0]));
    assert_eq!(
        ep.shape,
        PadShape::Rect {
            w_nm: nm(5.2),
            h_nm: nm(5.2)
        }
    );
    let first = pad(&e, &key("sides", 0, 0))?;
    assert_eq!(first.centre_nm, [nm(-3.4), nm(2.75)]);
    assert_eq!(
        first.shape,
        PadShape::Rect {
            w_nm: nm(0.8),
            h_nm: nm(0.25)
        }
    );
    // Side 1 (pins 13–24) is side 0 turned a quarter: along the bottom, left to right.
    let thirteenth = pad(&e, &key("sides", 1, 0))?;
    assert_eq!(
        (
            thirteenth.number.as_str(),
            thirteenth.centre_nm,
            thirteenth.rotation_udeg
        ),
        ("13", [nm(-2.75), nm(-3.4)], 90_000_000)
    );
    assert_eq!(pad(&e, &key("sides", 3, 11))?.number, "48");
    // No two pads share a centre.
    let centres: BTreeSet<[i64; 2]> = e.pads.iter().map(|p| p.centre_nm).collect();
    assert_eq!(centres.len(), 49);
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn bga256_evaluates_grid_with_centre_void() -> R {
    let e = evaluate(&fixtures::bga256()?, BGA256_PKG)?;
    assert_eq!(e.pads.len(), 240, "16 × 16 minus the centre 4 × 4");
    assert_eq!(e.dof, 0);
    let a1 = pad(&e, &key("grid", 0, 0))?;
    assert_eq!(
        (a1.number.as_str(), a1.centre_nm),
        ("A1", [nm(-7.5), nm(7.5)])
    );
    assert_eq!(a1.shape, PadShape::Circle { d_nm: nm(0.45) });
    let last = pad(&e, &key("grid", 15, 15))?;
    assert_eq!(
        (last.number.as_str(), last.centre_nm),
        ("T16", [nm(7.5), nm(-7.5)])
    );
    for i in 6..10 {
        for j in 6..10 {
            assert!(
                pad(&e, &key("grid", i, j)).is_err(),
                "({i}, {j}) is in the void"
            );
        }
    }
    assert!(pad(&e, &key("grid", 5, 6)).is_ok() && pad(&e, &key("grid", 10, 9)).is_ok());
    assert!(!e.pads.iter().any(|p| {
        ["I", "O", "Q", "S", "X", "Z"]
            .iter()
            .any(|l| p.number.starts_with(l))
    }));
    // Rigid copies land exactly where solving every land in the sketch does.
    let flat = evaluate(&fixtures::bga256_flat()?, &[])?;
    let geometry = |e: &Evaluation| -> BTreeSet<([i64; 2], i64)> {
        e.pads
            .iter()
            .map(|p| {
                (
                    p.centre_nm,
                    if let PadShape::Circle { d_nm } = p.shape {
                        d_nm
                    } else {
                        0
                    },
                )
            })
            .collect()
    };
    assert_eq!(geometry(&flat), geometry(&e));
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn results_match_golden_hashes_on_this_target() -> R {
    let mut got = Vec::new();
    for (name, source, pkg) in fixtures::all()? {
        let e = evaluate(&source, pkg)?;
        println!("{name}: hash {:#018x} raw {:#018x}", e.hash, e.raw_hash);
        got.push((name, e.hash, e.raw_hash));
    }
    for (name, hash, raw) in got {
        let golden = GOLDEN
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, h, r)| (*h, *r));
        assert_eq!(
            golden.map(|g| g.0),
            Some(hash),
            "{name}: quantized result changed on this target"
        );
        assert_eq!(
            golden.map(|g| g.1),
            Some(raw),
            "{name}: solved point bits changed on this target"
        );
    }
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn evaluation_is_deterministic_across_runs() -> R {
    for (name, source, pkg) in fixtures::all()? {
        let first = evaluate(&source, pkg)?;
        let first_sketch = check::canonical(&first.sketch_out)?;
        for _ in 0..20 {
            let again = evaluate(&source, pkg)?;
            assert_eq!(again.pads, first.pads, "{name}");
            assert_eq!(
                (again.hash, again.raw_hash),
                (first.hash, first.raw_hash),
                "{name}"
            );
            assert_eq!(check::canonical(&again.sketch_out)?, first_sketch, "{name}");
        }
        // The stored solved state re-evaluates to itself (what publish checks).
        let mut stored = source.clone();
        stored.sketch_json = first_sketch;
        assert_eq!(
            evaluate(&stored, pkg)?.hash,
            first.hash,
            "{name}: stored result is stale"
        );
    }
    Ok(())
}

/// Item 6: a parameter change re-solves and surviving pads keep their identity.
#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn parameter_changes_keep_surviving_identities() -> R {
    let soic = fixtures::soic8()?;
    let base = evaluate(&soic, SOIC8_PKG)?;

    // Pitch (a package value): same pads, same identities, new places.
    let pitch = evaluate(&soic, &[("e", 1.0), ("E", 3.9), ("D", 4.9)])?;
    assert_eq!(keys(&pitch), keys(&base));
    assert_eq!(
        pad(&pitch, &key("rows", 0, 0))?.centre_nm,
        [nm(-2.7), nm(1.5)]
    );
    assert_eq!(
        pad(&pitch, &key("rows", 1, 0))?.centre_nm,
        [nm(2.7), nm(-1.5)]
    );

    // Count up (SOIC-14): every old key survives; six are new.
    let mut grown = soic.clone();
    grown.set_param("per_row", "7")?;
    let grown_eval = evaluate(&grown, SOIC8_PKG)?;
    assert!(keys(&base).is_subset(&keys(&grown_eval)));
    assert_eq!(grown_eval.pads.len(), 14);
    assert_eq!(
        pad(&grown_eval, &key("rows", 0, 0))?.centre_nm,
        [nm(-2.7), nm(3.81)]
    );
    // The seed's anchor keeps its sketch point id through the re-solve.
    assert!(grown_eval.sketch_out.point_index("pad1").is_some());

    // Count down: survivors are exactly the keys still generated.
    let mut shrunk = soic.clone();
    shrunk.set_param("per_row", "2")?;
    let shrunk_eval = evaluate(&shrunk, SOIC8_PKG)?;
    let expected: BTreeSet<PadKey> = [
        key("rows", 0, 0),
        key("rows", 0, 1),
        key("rows", 1, 0),
        key("rows", 1, 1),
    ]
    .into();
    assert_eq!(keys(&shrunk_eval), expected);

    // BGA growth and QFN pitch: the same rule on the other layouts.
    let mut bga = fixtures::bga256()?;
    let before = evaluate(&bga, BGA256_PKG)?;
    bga.set_param("rows", "18")?;
    bga.set_param("cols", "18")?;
    let after = evaluate(&bga, BGA256_PKG)?;
    assert_eq!(after.pads.len(), 18 * 18 - 16);
    // The centre void re-centres (rows and columns 7–10 instead of 6–9): the
    // keys it now covers are retired, every other old key survives. An update
    // preview must list those retirements; nothing is renumbered silently.
    let retired: BTreeSet<PadKey> = keys(&before).difference(&keys(&after)).cloned().collect();
    let expected: BTreeSet<PadKey> = (7..=10u16)
        .flat_map(|i| (7..=10u16).map(move |j| (i, j)))
        .filter(|(i, j)| !((6..=9).contains(i) && (6..=9).contains(j)))
        .map(|(i, j)| key("grid", i, j))
        .collect();
    assert_eq!(retired, expected);
    assert_eq!(retired.len(), 7);
    let qfn = fixtures::qfn48()?;
    let q0 = evaluate(&qfn, QFN48_PKG)?;
    let q1 = evaluate(&qfn, &[("e", 0.4), ("D2", 4.6)])?;
    assert_eq!(keys(&q0), keys(&q1));
    assert_eq!(
        pad(&q1, &PadKey::Anchor("ep".into()))?.shape,
        PadShape::Rect {
            w_nm: nm(4.6),
            h_nm: nm(4.6)
        }
    );
    assert_eq!(
        pad(&q1, &key("sides", 0, 0))?.centre_nm,
        [nm(-3.4), nm(2.2)]
    );
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn copper_only_pass_is_needed_for_overlapping_and_concentric_curves() -> R {
    // SOIC: the body's assembly outline crosses the pads' inner ends.
    let soic = fixtures::soic8()?;
    let solved = evaluate(&soic, SOIC8_PKG)?.sketch_out;
    let anchor = solved
        .points
        .iter()
        .find(|p| p.id == "pad1")
        .map(|p| p.pos)
        .ok_or_else(|| finding("test", "pad1"))?;
    let all = find_profiles(&solved);
    let unfiltered = profile_at_point(&all, anchor)
        .and_then(|i| all.get(i))
        .ok_or_else(|| finding("test", "profile"))?;
    assert!(
        unfiltered
            .curve_ids()
            .iter()
            .any(|c| c.starts_with("assembly")),
        "unfiltered, the assembly outline splits the pad: {:?}",
        unfiltered.curve_ids()
    );
    let copper = find_profiles(&super::copper_only(&solved, &soic.pads));
    let filtered = profile_at_point(&copper, anchor)
        .and_then(|i| copper.get(i))
        .ok_or_else(|| finding("test", "profile"))?;
    assert!((filtered.area - 1.55 * 0.6).abs() < 1e-9);

    // QFN: the exposed pad's concentric drill would capture its anchor.
    let qfn = fixtures::qfn48()?;
    let solved = evaluate(&qfn, QFN48_PKG)?.sketch_out;
    let all = find_profiles(&solved);
    let inner = profile_at_point(&all, solidcraft_engine::geom::Vec2::ZERO)
        .and_then(|i| all.get(i))
        .ok_or_else(|| finding("test", "profile"))?;
    assert_eq!(
        inner.curve_ids(),
        vec!["ep.drill".to_string()],
        "unfiltered, the drill is the innermost profile"
    );
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn bad_sources_are_refused_with_findings() -> R {
    let soic = fixtures::soic8()?;
    let rule = |source: &Source, pkg: &[(&str, f64)]| evaluate(source, pkg).err().map(|f| f.rule);

    let mut s = soic.clone();
    s.evaluator_rev = "0000".into();
    assert_eq!(rule(&s, SOIC8_PKG), Some("footprint.evaluator_rev"));

    let mut s = soic.clone();
    s.set_param("pad_l", "pad_w * 2")?;
    s.set_param("pad_w", "pad_l / 2")?;
    assert_eq!(rule(&s, SOIC8_PKG), Some("footprint.param_expr"), "cycle");

    let mut s = soic.clone();
    s.set_param("pad_l", "nothing_here")?;
    assert_eq!(
        rule(&s, SOIC8_PKG),
        Some("footprint.param_expr"),
        "unknown name"
    );

    let mut s = soic.clone();
    s.set_param("pad_l", "30 deg")?;
    assert_eq!(
        rule(&s, SOIC8_PKG),
        Some("footprint.param_expr"),
        "unit mismatch"
    );
    // An angle parameter driving a length dimension.
    let mut s = soic.clone();
    s.set_param("pad_l", "30")?;
    if let Some(p) = s.params.iter_mut().find(|p| p.name == "pad_l") {
        p.unit = super::ParamUnit::Deg;
    }
    assert_eq!(
        rule(&s, SOIC8_PKG),
        Some("footprint.param_expr"),
        "angle as length"
    );

    for name in ["mm", "pi", "sqrt", "pkg_e", "1x", "a.b"] {
        let mut s = soic.clone();
        if let Some(p) = s.params.first_mut() {
            p.name = name.into();
        }
        assert_eq!(rule(&s, SOIC8_PKG), Some("footprint.param_name"), "{name}");
    }

    // Missing package value.
    assert_eq!(
        rule(&soic, &[("E", 3.9), ("D", 4.9)]),
        Some("footprint.param_expr")
    );

    // A pad narrower than nothing cannot be solved; the source is unchanged.
    let mut s = soic.clone();
    s.set_param("pad_w", "-1 mm")?;
    let before = s.clone();
    assert!(matches!(
        rule(&s, SOIC8_PKG),
        Some("footprint.solve_failed" | "footprint.flipped")
    ));
    assert_eq!(s, before);

    // Non-whole counts.
    let mut s = soic.clone();
    s.set_param("per_row", "2.5")?;
    assert_eq!(rule(&s, SOIC8_PKG), Some("footprint.param_expr"));
    Ok(())
}

fn soic_value() -> Result<Value, Finding> {
    serde_json::from_str(&fixtures::soic8()?.sketch_json)
        .map_err(|e| finding("test", e.to_string()))
}

fn refusal(v: &Value) -> Option<&'static str> {
    check::check_sketch(&v.to_string()).err().map(|f| f.rule)
}

fn refuse_with_sketch(sketch: &Sketch) -> Result<Option<&'static str>, Finding> {
    Ok(check::check_sketch(&check::canonical(sketch)?)
        .err()
        .map(|f| f.rule))
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn check_sketch_round_trip_caps_and_validation() -> R {
    let canonical = soic_value()?;
    assert_eq!(refusal(&canonical), None);
    const UNSUPPORTED: Option<&str> = Some("footprint.sketch_unsupported");

    // Unknown field: serde would drop it silently.
    let mut v = canonical.clone();
    v["points"][1]["colour"] = json!("red");
    assert_eq!(refusal(&v), UNSUPPORTED);
    let mut v = canonical.clone();
    v["future_feature"] = json!({"x": 1});
    assert_eq!(refusal(&v), UNSUPPORTED);
    // Non-canonical: a defaulted field omitted would be added back.
    let mut v = canonical.clone();
    if let Some(o) = v["points"][1].as_object_mut() {
        o.remove("fixed");
    }
    assert_eq!(refusal(&v), UNSUPPORTED);
    // Not JSON, not a sketch.
    assert_eq!(check::check_sketch("{").err().map(|f| f.rule), UNSUPPORTED);
    assert_eq!(check::check_sketch("[]").err().map(|f| f.rule), UNSUPPORTED);

    // Caps.
    let big = "x".repeat(check::MAX_SKETCH_BYTES + 1);
    assert_eq!(
        check::check_sketch(&big).err().map(|f| f.rule),
        Some("footprint.too_large")
    );
    let mut v = canonical.clone();
    if let Some(points) = v["points"].as_array_mut() {
        let p = points.get(1).cloned().unwrap_or(Value::Null);
        points.extend(std::iter::repeat_n(p, check::MAX_POINTS));
    }
    assert_eq!(refusal(&v), Some("footprint.too_large"));
    let mut v = canonical.clone();
    v["points"][1]["pos"][0] = json!(1000.5);
    assert_eq!(refusal(&v), UNSUPPORTED);

    // Per-constraint validation by index and kind.
    let mut v = canonical.clone();
    let horizontal = v["constraints"]
        .as_array()
        .and_then(|c| c.iter().position(|k| k["type"] == "horizontal"));
    if let Some(i) = horizontal {
        v["constraints"][i]["l"] = json!(9999);
    }
    assert!(horizontal.is_some());
    assert_eq!(refusal(&v), UNSUPPORTED);
    let mut v = canonical.clone();
    if let Some(i) = horizontal {
        // A diagonal-free line made into a circle reference: wrong kind.
        v["constraints"][i] = json!({"id": "kx", "type": "radius", "c": 0, "value": 1.0});
    }
    assert_eq!(refusal(&v), UNSUPPORTED);
    // Curve point indices.
    let mut v = canonical.clone();
    v["curves"][0]["a"] = json!(9999);
    assert_eq!(refusal(&v), UNSUPPORTED);
    // Duplicate ids.
    let mut v = canonical.clone();
    v["curves"][1]["id"] = v["curves"][0]["id"].clone();
    assert_eq!(refusal(&v), UNSUPPORTED);
    // Point 0 must be the origin.
    let mut v = canonical.clone();
    v["points"][0]["fixed"] = json!(false);
    assert_eq!(refusal(&v), UNSUPPORTED);

    // The footprint subset.
    let base: Sketch =
        serde_json::from_value(canonical.clone()).map_err(|e| finding("test", e.to_string()))?;
    let mut s = base.clone();
    s.view.three_d = true;
    assert_eq!(refuse_with_sketch(&s)?, UNSUPPORTED, "3D");
    let mut s = base.clone();
    if let Some(p) = s.points.get_mut(1) {
        p.link = Some("l9".into());
    }
    assert_eq!(refuse_with_sketch(&s)?, UNSUPPORTED, "linked point");
    let mut s = base.clone();
    s.add_point(solidcraft_engine::geom::Vec2::new(1.0, 1.0), Some("e0"))
        .map_err(|e| finding("test", e.to_string()))?;
    let c = s.point_index("e0").unwrap_or(0);
    s.curves.push(
        serde_json::from_value(json!({"id": "el", "type": "ellipse", "c": c, "m": 0, "r": 0.5}))
            .map_err(|e| finding("test", e.to_string()))?,
    );
    assert_eq!(refuse_with_sketch(&s)?, UNSUPPORTED, "free-form curve");
    Ok(())
}

/// The subset refusals that need Solidcraft's own commands to produce the
/// feature (patterns, associative offsets), made through the bridge.
#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn check_sketch_refuses_patterns_and_offsets_made_by_solidcraft() -> R {
    let base = pad_only()?;
    let pattern = super::bridge::run(
        &base,
        "sketch.pattern.rectangular",
        json!({"entities": ["p.s0", "p.s1", "p.s2", "p.s3"], "count": 2, "spacing": 3}),
    )?;
    assert!(!pattern.sketch.patterns.is_empty());
    assert_eq!(
        refuse_with_sketch(&pattern.sketch)?,
        Some("footprint.sketch_unsupported")
    );
    let offset = super::bridge::run(
        &base,
        "sketch.offset",
        json!({"curves": ["p.s0"], "distance": 0.1}),
    )?;
    assert!(!offset.sketch.offsets.is_empty() || !offset.sketch.freeform_offsets.is_empty());
    assert_eq!(
        refuse_with_sketch(&offset.sketch)?,
        Some("footprint.sketch_unsupported")
    );
    Ok(())
}

/// One fully constrained 2 × 1 mm pad `p` at (3, 2).
fn pad_only() -> Result<Sketch, Finding> {
    let mut b = fixtures::Builder::new();
    b.param("w", "2 mm", super::ParamUnit::Mm);
    b.param("h", "1 mm", super::ParamUnit::Mm);
    b.param("x", "3 mm", super::ParamUnit::Mm);
    b.param("y", "2 mm", super::ParamUnit::Mm);
    b.rect_pad(
        "p",
        solidcraft_engine::geom::Vec2::new(3.0, 2.0),
        (2.0, 1.0),
        ("w", "h"),
        Some(("x", "y")),
    )?;
    Ok(b.sketch)
}

/// FD2b probe: trim, fillet, offset and mirror through a scratch `Session`.
#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn fd2b_scratch_session_bridge_reaches_modify_commands() -> R {
    let base = pad_only()?;
    let bound = |s: &Sketch| s.constraints.iter().filter(|c| c.param.is_some()).count();
    assert_eq!(bound(&base), 4);

    let fillet = super::bridge::run(
        &base,
        "sketch.fillet",
        json!({"point": "p.c2", "radius": "0.2 mm"}),
    )?;
    let arcs = fillet
        .sketch
        .curves
        .iter()
        .filter(|c| matches!(c.kind, solidcraft_engine::sketch::CurveKind::Arc { .. }))
        .count();
    assert_eq!(arcs, 1, "{}", fillet.reply);
    // Pcbcraft's bindings survive by constraint id; the new radius dimension
    // arrives bound to a parameter the scratch document invented.
    for c in base.constraints.iter().filter(|c| c.param.is_some()) {
        let after = fillet
            .sketch
            .constraints
            .iter()
            .find(|k| k.id == c.id)
            .and_then(|k| k.param.clone());
        assert_eq!(after, c.param, "{}", c.id);
    }
    assert_eq!(bound(&fillet.sketch), 5);
    println!("fillet created parameters: {:?}", fillet.created);
    assert_eq!(
        fillet.created.len(),
        1,
        "the radius became a scratch-document parameter"
    );
    assert!(check::check_sketch(&check::canonical(&fillet.sketch)?).is_ok());

    // Trim a line nothing crosses: it is deleted whole.
    let trim = super::bridge::run(
        &base,
        "sketch.trim",
        json!({"curve": "p.s0", "at": [3.0, 1.5]}),
    )?;
    assert_eq!(trim.reply["result"]["deleted"], "p.s0", "{}", trim.reply);
    // Trim the SOIC body outline where it crosses pad 1 (clearing it from
    // the copper): the piece between the pad's edges goes, two pieces stay,
    // and the result is still inside the footprint subset.
    let soic = check::check_sketch(&fixtures::soic8()?.sketch_json)?;
    let cleared = super::bridge::run(
        &soic,
        "sketch.trim",
        json!({"curve": "assembly.s3", "at": [-1.95, 1.905]}),
    )?;
    println!("trim reply: {}", cleared.reply);
    assert_eq!(
        cleared.reply["result"]["trimmed"], "assembly.s3",
        "{}",
        cleared.reply
    );
    assert_eq!(
        cleared.sketch.curves.len(),
        soic.curves.len() + 1,
        "one line became two: {}",
        cleared.reply
    );
    // Expression-driven points were detached for the command and come back.
    assert_eq!(
        cleared
            .sketch
            .points
            .iter()
            .filter(|p| p.exprs.is_some())
            .count(),
        4
    );
    assert!(check::check_sketch(&check::canonical(&cleared.sketch)?).is_ok());

    // Mirroring about a sketch axis projects that axis in as a link, which
    // the footprint subset refuses; about the sketch's own line it is plain.
    let entities = json!(["p.s0", "p.s1", "p.s2", "p.s3"]);
    let by_axis = super::bridge::run(
        &base,
        "sketch.mirror",
        json!({"entities": entities, "line": "y"}),
    )?;
    assert!(!by_axis.sketch.links.is_empty(), "{}", by_axis.reply);
    assert_eq!(
        refuse_with_sketch(&by_axis.sketch)?,
        Some("footprint.sketch_unsupported")
    );
    let mut with_axis = base.clone();
    let axis = with_axis
        .add_line(
            solidcraft_engine::geom::Vec2::new(0.0, -5.0),
            solidcraft_engine::geom::Vec2::new(0.0, 5.0),
            None,
            None,
            Some("axis"),
        )
        .map_err(|e| finding("test", e.to_string()))?;
    if let Some(c) = with_axis.curves.get_mut(axis) {
        c.construction = true;
    }
    let mirror = super::bridge::run(
        &with_axis,
        "sketch.mirror",
        json!({"entities": entities, "line": "axis"}),
    )?;
    assert_eq!(
        mirror.sketch.curves.len(),
        with_axis.curves.len() + 4,
        "{}",
        mirror.reply
    );
    assert!(check::check_sketch(&check::canonical(&mirror.sketch)?).is_ok());

    // Offset makes an associative offset (refused by the subset, see the
    // test above); breaking the association in the same scratch session
    // leaves plain curves the subset accepts.
    let offset = super::bridge::run(
        &base,
        "sketch.offset",
        json!({"curves": ["p.s0"], "distance": 0.1}),
    )?;
    println!("offset reply: {}", offset.reply);
    let outputs: Vec<String> = offset.reply["curves"]
        .as_array()
        .map(|c| {
            c.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(outputs.len(), 4, "the whole closed chain is offset");
    let mut steps = vec![(
        "sketch.offset",
        json!({"curves": ["p.s0"], "distance": 0.1}),
    )];
    // Breaking one output dissolves the whole chain's association.
    steps.extend(
        outputs
            .iter()
            .take(1)
            .map(|c| ("sketch.offset.break", json!({"curve": c}))),
    );
    let broken = super::bridge::run_all(&base, steps)?;
    println!("offset.break reply: {}", broken.reply);
    assert!(broken.sketch.offsets.is_empty() && broken.sketch.freeform_offsets.is_empty());
    assert_eq!(broken.sketch.curves.len(), base.curves.len() + 4);
    assert!(check::check_sketch(&check::canonical(&broken.sketch)?).is_ok());
    Ok(())
}

/// Wasm timing in Node: `Instant` is unavailable on wasm32-unknown-unknown and
/// `Date.now()` has millisecond resolution, so only the mean over many runs is
/// meaningful (no percentiles).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen_test::wasm_bindgen_test]
fn measure_wasm_mean_timings() -> R {
    let mut cases = fixtures::all()?;
    cases.push(("BGA-256 flat", fixtures::bga256_flat()?, &[]));
    for (name, source, pkg) in cases {
        let runs = if pkg.is_empty() { 50 } else { 2000 };
        let start = js_sys::Date::now();
        for _ in 0..runs {
            evaluate(&source, pkg)?;
        }
        let mean = (js_sys::Date::now() - start) / f64::from(runs);
        wasm_bindgen_test::console_log!("wasm {name}: mean {mean:.4} ms over {runs} runs");
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted.get(i).copied().unwrap_or(f64::NAN)
}

/// Item 4: timings per phase (printed; run with `--nocapture`). No timing is
/// asserted: these are measurements, not targets.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn measure_evaluation_timings() -> R {
    use std::time::Instant;
    const RUNS: usize = 200;
    // "same": re-evaluate the stored source unchanged (what publish does;
    // Solidcraft's per-thread parameter cache hits). "edit": every run changes
    // the package pitch `pkg_e` by a different amount from the stored solved
    // state, so parameters are re-evaluated and the seed really moves.
    // The flat BGA has no package values: its edit changes the land diameter.
    let mut cases = fixtures::all()?;
    cases.push(("BGA-256 flat", fixtures::bga256_flat()?, &[]));
    for (name, source, pkg) in cases {
        for scenario in ["same", "edit"] {
            let mut phases: Vec<Vec<f64>> = vec![Vec::new(); super::PHASES.len() + 1];
            for k in 0..RUNS {
                let step = 1.0 + (k % 97 + 1) as f64 * 1e-3;
                let edited: Vec<(&str, f64)> = pkg
                    .iter()
                    .map(|&(n, v)| {
                        if scenario == "edit" && n == "e" {
                            (n, v * step)
                        } else {
                            (n, v)
                        }
                    })
                    .collect();
                let mut source = source.clone();
                if scenario == "edit" && pkg.is_empty() {
                    source.set_param("land_d", &format!("{} mm", 0.45 * step))?;
                }
                let start = Instant::now();
                let mut last = start;
                let mut at = 0usize;
                super::evaluate_with_clock(&source, &edited, &mut |_| {
                    let now = Instant::now();
                    if let Some(v) = phases.get_mut(at) {
                        v.push(now.duration_since(last).as_secs_f64() * 1e3);
                    }
                    last = now;
                    at += 1;
                })?;
                if let Some(v) = phases.last_mut() {
                    v.push(start.elapsed().as_secs_f64() * 1e3);
                }
            }
            let mut line = format!(
                "{name:10} {scenario} sketch {:5} B",
                source.sketch_json.len()
            );
            for (label, v) in super::PHASES.iter().chain(&["total"]).zip(&mut phases) {
                v.sort_by(f64::total_cmp);
                line += &format!(
                    " | {label} {:.3}/{:.3}",
                    percentile(v, 0.5),
                    percentile(v, 0.95)
                );
            }
            println!("{line}  (ms p50/p95, {RUNS} runs)");
        }
    }
    let base = pad_only()?;
    for (command, params) in [
        (
            "sketch.fillet",
            json!({"point": "p.c2", "radius": "0.2 mm"}),
        ),
        ("sketch.trim", json!({"curve": "p.s0", "at": [3.0, 1.5]})),
        (
            "sketch.mirror",
            json!({"entities": ["p.s0", "p.s1", "p.s2", "p.s3"], "line": "y"}),
        ),
        (
            "sketch.offset",
            json!({"curves": ["p.s0"], "distance": 0.1}),
        ),
    ] {
        let mut v = Vec::new();
        for _ in 0..50 {
            let start = Instant::now();
            super::bridge::run(&base, command, params.clone())?;
            v.push(start.elapsed().as_secs_f64() * 1e3);
        }
        v.sort_by(f64::total_cmp);
        println!(
            "bridge {command:14} p50 {:.3} p95 {:.3} ms",
            percentile(&v, 0.5),
            percentile(&v, 0.95)
        );
    }
    Ok(())
}
