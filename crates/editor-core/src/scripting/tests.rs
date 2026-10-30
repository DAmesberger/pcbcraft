#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    DefinitionId, Engine as DocumentEngine, LibraryCommand, Operation, Outcome,
    library::{Definition, DefinitionMeta},
};

/// The footprint draft every engine fixture creates first.
const DRAFT: DefinitionId = DefinitionId(1);

fn generated(engine: &DocumentEngine) -> FootprintDefinition {
    match &engine.drafts().get(DRAFT).unwrap().definition {
        Definition::Footprint(f) => f.clone(),
        _ => panic!("not a footprint draft"),
    }
}

fn script(source: &str) -> Script {
    Script {
        api_version: 1,
        source: source.into(),
        units: vec![
            UnitDefinition {
                symbol: "mm".into(),
                dimension: quantity::LENGTH,
                scale: 0.001,
            },
            UnitDefinition {
                symbol: "V".into(),
                dimension: [2, 1, -3, -1, 0, 0, 0],
                scale: 1.0,
            },
            UnitDefinition {
                symbol: "kΩ".into(),
                dimension: [2, 1, -3, -2, 0, 0, 0],
                scale: 1000.0,
            },
        ],
        inputs: BTreeMap::from([
            (
                "voltage".into(),
                Input::Quantity {
                    value: 5.0,
                    unit: "V".into(),
                },
            ),
            (
                "resistance".into(),
                Input::Quantity {
                    value: 10.0,
                    unit: "kΩ".into(),
                },
            ),
            (
                "pitch".into(),
                Input::Quantity {
                    value: 2.54,
                    unit: "mm".into(),
                },
            ),
        ]),
    }
}

fn draft() -> FootprintDefinition {
    use crate::{PackageRef, PackageTerminalId};
    FootprintDefinition {
        package: PackageRef::default(),
        next_local: 14,
        datum_offset: [0.0; 2],
        pads: (0..4)
            .map(|i| Pad {
                id: PadId(10 + i),
                terminal: PackageTerminalId(200 + i),
                position: [0.0, i as f64 * 2.0],
                size: [1.0, 1.0],
            })
            .collect(),
    }
}

const ARRAY: &str = r#"
    for i in 0..4 {
        footprint.pad(10 + i, i * pitch, q(0.0, "mm"), q(1.2, "mm"), q(1.8, "mm"));
    }
"#;

fn command(source: &str) -> Operation {
    Operation::Library(LibraryCommand::GeneratePads {
        draft: DRAFT,
        script: script(source),
    })
}

fn engine() -> DocumentEngine {
    let mut engine = DocumentEngine::default();
    let outcome = engine
        .apply(
            0,
            Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta::default(),
                definition: Definition::Footprint(draft()),
            }),
        )
        .unwrap();
    assert_eq!(outcome, Outcome::DraftCreated { id: DRAFT });
    engine
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn electrical_arithmetic_returns_power_in_si_units() {
    // Independently: 5 V * 5 V / 10,000 Ω = 0.0025 W.
    let power = evaluate_quantity(&script("voltage * voltage / resistance")).unwrap();
    assert!((power.value() - 0.0025).abs() < 1e-15);
    assert_eq!(power.dimension(), [2, 1, -3, 0, 0, 0, 0]);
    let r = evaluate_quantity(&script(r#"resistance + q(2.0, "kΩ")"#)).unwrap();
    assert_eq!(r.value(), 12000.0);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn units_are_data_and_versions_are_explicit() {
    let mut request = script(r#"q(3.0, "custom")"#);
    request.units.push(UnitDefinition {
        symbol: "custom".into(),
        dimension: [0, 0, 1, 0, 0, 0, 0],
        scale: 60.0,
    });
    let value = evaluate_quantity(&request).unwrap();
    assert_eq!(value.value(), 180.0);
    assert_eq!(value.dimension(), [0, 0, 1, 0, 0, 0, 0]);
    request.api_version = 2;
    assert_eq!(
        evaluate_quantity(&request).unwrap_err().code(),
        "unsupported"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn dimensional_errors_and_nonfinite_results_are_rejected() {
    for (source, message) in [
        ("voltage + resistance", "different dimensions"),
        (r#"voltage / q(0.0, "V")"#, "division by zero"),
        (r#"q(1.0e100, "V") * q(1.0e100, "V")"#, "finite"),
        (r#"q(1.0, "missing")"#, "unknown unit"),
        ("voltage * 9007199254740993", "exact floating-point range"),
        (r#"q(1.0e-200, "V") * q(1.0e-200, "V")"#, "underflow"),
    ] {
        let error = evaluate_quantity(&script(source)).unwrap_err();
        assert_eq!(error.code(), "invalid_input");
        assert!(error.to_string().contains(message), "{error}");
    }
    let mut request = script("voltage");
    request.inputs.insert(
        "voltage".into(),
        Input::Quantity {
            value: f64::NAN,
            unit: "V".into(),
        },
    );
    assert!(evaluate_quantity(&request).is_err());
    request = script(r#"q(1.0, "high") * q(1.0, "high")"#);
    request.units.push(UnitDefinition {
        symbol: "high".into(),
        dimension: [16, 0, 0, 0, 0, 0, 0],
        scale: 1.0,
    });
    assert!(
        evaluate_quantity(&request)
            .unwrap_err()
            .to_string()
            .contains("exponent")
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn formulas_cannot_execute_statements_or_ambient_effects() {
    for source in [
        "let x = voltage; x",
        "voltage = resistance",
        "loop {}",
        "eval(\"voltage\")",
        "timestamp()",
        "read_file(\"secret\")",
        "print(\"hello\")",
        "import \"other\" as other;",
    ] {
        assert!(
            evaluate_quantity(&script(source)).is_err(),
            "accepted {source}"
        );
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn generated_array_preserves_mapping_and_commits_as_one_undo_step() {
    let mut engine = engine();
    let original = engine.document().clone();
    // Exercise the same serialized operation consumed by CLI/MCP.
    let json = serde_json::to_string(&command(ARRAY)).unwrap();
    engine
        .apply(engine.revision(), serde_json::from_str(&json).unwrap())
        .unwrap();
    let generated = generated(&engine);
    assert_eq!(generated.package, draft().package);
    for (pad, (id, terminal, x)) in generated.pads.iter().zip([
        (10, 200, 0.0),
        (11, 201, 2.54),
        (12, 202, 5.08),
        (13, 203, 7.62),
    ]) {
        assert_eq!((pad.id.0, pad.terminal.0), (id, terminal));
        assert!((pad.position[0] - x).abs() < 1e-12);
        assert_eq!(pad.position[1], 0.0);
        assert!((pad.size[0] - 1.2).abs() < 1e-12);
        assert!((pad.size[1] - 1.8).abs() < 1e-12);
    }
    let after = engine.document().clone();
    let saved = serde_json::to_string(&engine.to_file()).unwrap();
    let reopened = DocumentEngine::from_file(serde_json::from_str(&saved).unwrap()).unwrap();
    assert_eq!(reopened.document(), &after);
    engine.undo().unwrap();
    assert_eq!(engine.document(), &original);
    engine.redo().unwrap();
    assert_eq!(engine.document(), &after);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn partial_generation_failures_preserve_document_revision_and_redo() {
    let mut engine = engine();
    engine.apply(engine.revision(), command(ARRAY)).unwrap();
    engine.undo().unwrap();
    let before = engine.document().clone();
    let revision = engine.revision();
    // A successful callback precedes every failure, proving output is staged.
    let prefix = r#"footprint.pad(10, pitch, pitch, pitch, pitch);"#;
    for suffix in [
        "loop {}",                                          // operation budget
        "let a = []; loop { a.push(1); }",                  // data bound
        "footprint.pad(11, voltage, pitch, pitch, pitch);", // wrong dimension
        "footprint.pad(11, pitch, pitch, q(-1.0, \"mm\"), pitch);",
        "footprint.pad(10, pitch, pitch, pitch, pitch);", // duplicate ID
        "footprint.pad(99, pitch, pitch, pitch, pitch);", // unknown ID
        "",                                               // incomplete output
        "throw \"deliberate failure\";",
        "footprint = 42;", // overwritten result
    ] {
        assert!(
            engine
                .apply(revision, command(&format!("{prefix}{suffix}")))
                .is_err()
        );
        assert_eq!(engine.document(), &before);
        assert_eq!(engine.revision(), revision);
        assert!(engine.can_redo());
    }
    engine.redo().unwrap();
    assert!((generated(&engine).pads[3].position[0] - 7.62).abs() < 1e-12);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn limits_malformed_input_and_stale_revision_have_structured_errors() {
    let mut engine = engine();
    let revision = engine.revision();
    assert_eq!(
        engine
            .apply(revision, command("loop {}"))
            .unwrap_err()
            .code(),
        "limit_exceeded"
    );
    assert_eq!(
        engine
            .apply(revision, command("let = ;"))
            .unwrap_err()
            .code(),
        "invalid_input"
    );
    // Stale check precedes even a script which would run forever.
    assert_eq!(
        engine
            .apply(revision - 1, command("loop {}"))
            .unwrap_err()
            .code(),
        "stale_revision"
    );
    let mut request = script(ARRAY);
    request.source = " ".repeat(MAX_SOURCE + 1);
    assert_eq!(
        generate_pads(&request, &draft()).unwrap_err().code(),
        "limit_exceeded"
    );
    request = script(ARRAY);
    request.units.push(request.units[0].clone());
    assert_eq!(
        generate_pads(&request, &draft()).unwrap_err().code(),
        "invalid_input"
    );
    request = script(ARRAY);
    request.units[0].scale = 0.0;
    assert!(generate_pads(&request, &draft()).is_err());
    assert!(
        serde_json::from_str::<Script>(
            r#"{"api_version":1,"source":"","units":[],"inputs":{},"extra":1}"#
        )
        .is_err()
    );
}
