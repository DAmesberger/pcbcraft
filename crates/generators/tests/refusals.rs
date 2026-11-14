//! Refusals: hand-written modules that break one rule each. Every refusal
//! is a sentence a person can act on that says nothing ran or nothing
//! changed, and the runtime keeps working for the next module.
use sparksmith_generator_abi::v1::{
    self, Footprint, Generation, Lead, Model, Opening, Package, PackageGeometry, Pad, PadShape,
    ParamValue, Request, Scope, Side,
};
use sparksmith_generator_abi::{Failure, GeneratorRuntime, codec};
use sparksmith_generators::{Limits, WasmiRuntime};

/// Where the fixture keeps its description, response and request buffer.
const DESCRIPTION_AT: u32 = 1024;
const RESPONSE_AT: u32 = 16 * 1024;
const REQUEST_AT: u32 = 48 * 1024;

fn description() -> v1::Description {
    v1::Description {
        abi: v1::ABI,
        id: "fixture".into(),
        version: 1,
        title: "Fixture".into(),
        licence: "MIT OR Apache-2.0".into(),
        deterministic: true,
        capabilities: vec![v1::Capability {
            name: v1::GENERATE_PART.into(),
            effect: v1::Effect::None,
            title: None,
            params: Vec::new(),
            rules: Vec::new(),
            presets: Vec::new(),
            part: Some(v1::PartOffer {
                families: Vec::new(),
                scopes: vec![Scope::Package],
                grammar: Vec::new(),
                instance_fields: Vec::new(),
            }),
        }],
        needs: Vec::new(),
    }
}

/// The fixture's description with an `edit` command beside its part
/// capability, as a plugin that may import host functions.
fn mixed() -> v1::Description {
    let mut d = description();
    d.capabilities.push(v1::Capability {
        name: "stitch".into(),
        effect: v1::Effect::Edit,
        title: None,
        params: Vec::new(),
        rules: Vec::new(),
        presets: Vec::new(),
        part: None,
    });
    d.needs.push(v1::Need {
        module: "layout".into(),
        major: 1,
        minor: 0,
        class: v1::Effect::Edit,
    });
    d
}

/// A minimal valid package-scope part: one pad.
fn generation() -> Generation {
    Generation {
        component: None,
        symbol: None,
        package: Package {
            terminals: vec!["1".into()],
            geometry: PackageGeometry {
                envelope_nm: [1_000_000, 1_000_000, 500_000],
                seating_nm: 0,
                leads: vec![Lead {
                    key: "1".into(),
                    contact: [0, 0, 0],
                }],
                pin1: "1".into(),
            },
        },
        footprint: Footprint {
            pads: vec![Pad {
                key: "1".into(),
                terminal: "1".into(),
                shape: PadShape::Rect {
                    size: [500_000, 500_000],
                },
                at: [0, 0],
                rot_mdeg: 0,
                side: Side::Top,
                hole: None,
                mask: Opening::Default,
                paste: Opening::Default,
            }],
            courtyard: vec![
                [-750_000, -750_000],
                [750_000, -750_000],
                [750_000, 750_000],
            ],
            silk: Vec::new(),
            assembly: Vec::new(),
        },
        binding: Vec::new(),
        model: Some(model()),
        fields: Vec::new(),
        notes: Vec::new(),
    }
}

/// The fixture's model: a part file by its SHA-256, with one length.
fn model() -> Model {
    Model::PartFile {
        asset: "ab".repeat(32),
        values: [("side".to_string(), ParamValue::Int(500_000))]
            .into_iter()
            .collect(),
    }
}

/// A fixture returning `generation` (canonical) as its part.
fn returning(generation: &Generation) -> Result<Fixture, String> {
    let bytes = codec::encode(generation).map_err(|e| e.to_string())?;
    let mut fixture = Fixture::ok()?;
    fixture.response = vec![v1::STATUS_OK];
    fixture.response.extend(bytes);
    fixture.generate = format!("i64.const {}", packed(RESPONSE_AT, fixture.response.len()));
    Ok(fixture)
}

fn request() -> Request {
    Request {
        abi: v1::ABI,
        scope: Scope::Package,
        params: v1::Params::new(),
    }
}

/// Bytes as a WAT data string.
fn data(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

fn packed(ptr: u32, len: usize) -> i64 {
    ((u64::from(ptr) << 32) | (len as u64)) as i64
}

/// The parts of a fixture module that a test changes.
struct Fixture {
    /// Extra module fields (imports, functions).
    extra: &'static str,
    /// Memory pages.
    pages: u32,
    /// The body of `sparksmith_invoke`, which must leave an i64.
    generate: String,
    /// The description it gives.
    description: v1::Description,
    /// The response bytes stored at `RESPONSE_AT`.
    response: Vec<u8>,
}

impl Fixture {
    fn ok() -> Result<Self, String> {
        let mut response = vec![v1::STATUS_OK];
        response.extend(codec::encode(&generation()).map_err(|e| e.to_string())?);
        Ok(Self {
            extra: "",
            pages: 2,
            generate: format!("i64.const {}", packed(RESPONSE_AT, response.len())),
            response,
            description: description(),
        })
    }

    fn wasm(&self) -> Result<Vec<u8>, String> {
        let description = codec::encode(&self.description).map_err(|e| e.to_string())?;
        let text = format!(
            r#"(module
              {extra}
              (memory (export "memory") {pages})
              (global $calls (mut i32) (i32.const 0))
              (data (i32.const {DESCRIPTION_AT}) "{description_data}")
              (data (i32.const {RESPONSE_AT}) "{response_data}")
              (func (export "sparksmith_abi") (result i32) i32.const 1)
              (func (export "sparksmith_alloc") (param i32) (result i32) i32.const {REQUEST_AT})
              (func (export "sparksmith_describe") (result i64) i64.const {describe})
              (func (export "sparksmith_invoke") (param i32 i32) (result i64)
                ;; A second call on the same instance traps: calls must not share one.
                (global.set $calls (i32.add (global.get $calls) (i32.const 1)))
                (if (i32.gt_u (global.get $calls) (i32.const 1)) (then unreachable))
                {generate}))"#,
            extra = self.extra,
            pages = self.pages,
            description_data = data(&description),
            response_data = data(&self.response),
            describe = packed(DESCRIPTION_AT, description.len()),
            generate = self.generate,
        );
        wat::parse_str(&text).map_err(|e| e.to_string())
    }
}

/// The refusal of running `fixture`, with its checks: plain words, says
/// nothing ran or changed, and the same runtime still runs a good module.
fn refused(runtime: &WasmiRuntime, fixture: &Fixture) -> Result<Failure, String> {
    let wasm = fixture.wasm()?;
    let failure = match runtime.generate(&wasm, &request()) {
        Ok(_) => return Err("the module was not refused".into()),
        Err(failure) => failure,
    };
    for problem in &failure.problems {
        let m = &problem.message;
        assert!(
            m.ends_with("Nothing was changed.") || m.ends_with("It was not run."),
            "{m}"
        );
        assert!(m.starts_with(char::is_uppercase), "{m}");
    }
    let good = Fixture::ok()?.wasm()?;
    assert!(runtime.generate(&good, &request()).is_ok());
    Ok(failure)
}

fn code(failure: &Failure) -> &str {
    failure.code().unwrap_or("")
}

#[test]
fn a_valid_fixture_runs_and_each_call_gets_a_fresh_instance() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let wasm = Fixture::ok()?.wasm()?;
    let loaded = runtime.load(&wasm).map_err(|f| f.to_string())?;
    assert_eq!(loaded.describe().map_err(|f| f.to_string())?, description());
    // The fixture traps on its second call within one instance.
    let first = loaded.generate(&request()).map_err(|f| f.to_string())?;
    let second = loaded.generate(&request()).map_err(|f| f.to_string())?;
    assert_eq!(first, second);
    assert_eq!(first.generation, generation());
    Ok(())
}

#[test]
fn a_module_with_imports_is_refused_before_it_runs() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let mut fixture = Fixture::ok()?;
    fixture.extra = r#"(import "env" "clock" (func $clock (result i64)))"#;
    let failure = refused(&runtime, &fixture)?;
    assert_eq!(code(&failure), "generator.has_imports");
    assert_eq!(
        failure.to_string(),
        "This plugin asks for the host function env.clock, which the plugin ABI does not offer with that signature (only sparksmith.call, sparksmith.log and sparksmith.cancelled). It was not run."
    );
    // The plugin ABI's own function with another signature.
    fixture.extra = r#"(import "sparksmith" "call" (func $call (param i32) (result i64)))"#;
    assert_eq!(code(&refused(&runtime, &fixture)?), "generator.has_imports");
    Ok(())
}

/// A plugin whose capabilities are all of effect none may import nothing,
/// not even the plugin ABI's own functions.
#[test]
fn a_pure_plugin_that_imports_anything_is_refused() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let mut fixture = Fixture::ok()?;
    fixture.extra = r#"(import "sparksmith" "cancelled" (func $cancelled (result i32)))"#;
    let failure = refused(&runtime, &fixture)?;
    assert_eq!(code(&failure), "generator.has_imports");
    assert_eq!(
        failure.to_string(),
        "This plugin imports host functions, but every capability it offers is of effect none, which may import nothing. It was not run."
    );
    Ok(())
}

/// A plugin with an edit command beside `generate-part` may import `call`;
/// during `generate-part` (effect none) the host answers it with the fixed
/// refusal, so the result is the same as without it.
#[test]
fn call_during_generate_part_is_refused_as_pure_and_changes_nothing() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let plain = Fixture::ok()?;
    let mut calling = Fixture::ok()?;
    calling.description = mixed();
    calling.extra = PURE_CALLER;
    calling.generate = format!(
        "(call $check) i64.const {}",
        packed(RESPONSE_AT, calling.response.len())
    );
    let wasm = calling.wasm()?;
    let loaded = runtime.load(&wasm).map_err(|f| f.to_string())?;
    assert_eq!(loaded.describe().map_err(|f| f.to_string())?, mixed());
    let with_call = runtime
        .generate(&wasm, &request())
        .map_err(|f| f.to_string())?;
    let without = runtime
        .generate(&plain.wasm()?, &request())
        .map_err(|f| f.to_string())?;
    assert_eq!(with_call.result_hash, without.result_hash);
    assert_eq!(with_call.generation, generation());
    Ok(())
}

/// Imports the three host functions and, in `$check`, calls each: `call`
/// must answer a reply of at least 20 bytes whose status byte is 1 (an
/// error) and whose body names `plugin.pure` at a fixed place, `cancelled`
/// must answer 0; otherwise the module traps.
const PURE_CALLER: &str = r#"
    (import "sparksmith" "call" (func $call (param i32 i32) (result i64)))
    (import "sparksmith" "log" (func $log (param i32 i32 i32)))
    (import "sparksmith" "cancelled" (func $cancelled (result i32)))
    (func $check (local $packed i64) (local $ptr i32)
      (call $log (i32.const 1) (i32.const 0) (i32.const 0))
      (if (i32.ne (call $cancelled) (i32.const 0)) (then unreachable))
      (local.set $packed (call $call (i32.const 0) (i32.const 0)))
      (if (i64.lt_u (i64.and (local.get $packed) (i64.const 0xffffffff)) (i64.const 20))
        (then unreachable))
      (local.set $ptr (i32.wrap_i64 (i64.shr_u (local.get $packed) (i64.const 32))))
      (if (i32.ne (i32.load8_u (local.get $ptr)) (i32.const 1)) (then unreachable))
      ;; {"errors":[{"code":"plugin.pure",... : the code starts at byte 21.
      (if (i32.ne (i32.load8_u (i32.add (local.get $ptr) (i32.const 21))) (i32.const 112))
        (then unreachable)))
"#;

#[test]
fn a_module_using_floats_is_refused_before_it_runs() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let mut fixture = Fixture::ok()?;
    fixture.extra = r#"(func $half (param i32) (result i32)
        (i32.trunc_f64_s (f64.mul (f64.convert_i32_s (local.get 0)) (f64.const 0.5))))"#;
    let failure = refused(&runtime, &fixture)?;
    assert_eq!(code(&failure), "generator.float");
    assert!(
        failure
            .to_string()
            .starts_with("The plugin uses floating-point arithmetic (floating-point")
    );
    assert!(failure.to_string().ends_with(
        "which a plugin whose results are hashed may not: its results must be the same bits on every computer. It was not run."
    ));
    Ok(())
}

#[test]
fn a_module_using_simd_is_refused_before_it_runs() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let mut fixture = Fixture::ok()?;
    fixture.extra =
        r#"(func $simd (result i32) (i32x4.extract_lane 0 (v128.const i32x4 1 2 3 4)))"#;
    let failure = refused(&runtime, &fixture)?;
    assert_eq!(code(&failure), "generator.unsupported_feature");
    assert!(
        failure
            .to_string()
            .starts_with("The generator uses a WebAssembly feature generators may not use (")
    );
    Ok(())
}

#[test]
fn a_module_that_never_finishes_runs_out_of_fuel() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let mut fixture = Fixture::ok()?;
    fixture.generate = "(loop $forever (br $forever)) i64.const 0".into();
    let failure = refused(&runtime, &fixture)?;
    assert_eq!(code(&failure), "generator.out_of_fuel");
    assert_eq!(
        failure.to_string(),
        "The generator did not finish within its work limit. Nothing was changed."
    );
    Ok(())
}

#[test]
fn memory_over_the_limit_is_refused_at_load_and_at_growth() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let message = "The generator needed more than its 32 MiB of memory. Nothing was changed.";
    // Declared up front: 513 pages of 64 KiB is just over 32 MiB.
    let mut fixture = Fixture::ok()?;
    fixture.pages = 513;
    let failure = refused(&runtime, &fixture)?;
    assert_eq!(code(&failure), "generator.memory_limit");
    assert_eq!(failure.to_string(), message);
    // Grown while running.
    let mut fixture = Fixture::ok()?;
    fixture.generate = "(drop (memory.grow (i32.const 511))) i64.const 0".into();
    let failure = refused(&runtime, &fixture)?;
    assert_eq!(code(&failure), "generator.memory_limit");
    assert_eq!(failure.to_string(), message);
    // Exactly at the limit is allowed.
    let mut fixture = Fixture::ok()?;
    fixture.generate = format!(
        "(drop (memory.grow (i32.const 510))) i64.const {}",
        packed(RESPONSE_AT, fixture.response.len())
    );
    assert!(runtime.generate(&fixture.wasm()?, &request()).is_ok());
    Ok(())
}

#[test]
fn output_over_the_limit_is_refused_unread() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let mut fixture = Fixture::ok()?;
    // Claims 5 MiB, past the 1 MiB response bound (and past its memory).
    fixture.generate = format!("i64.const {}", packed(0, 5 * 1024 * 1024));
    let failure = refused(&runtime, &fixture)?;
    assert_eq!(code(&failure), "generator.output_too_large");
    assert_eq!(
        failure.to_string(),
        "The generator returned a response of 5242880 bytes, more than the 1048576 allowed. Nothing was changed."
    );
    Ok(())
}

#[test]
fn a_trap_an_oversized_module_and_unreadable_output_are_refused() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let mut trap = Fixture::ok()?;
    trap.generate = "unreachable".into();
    let failure = refused(&runtime, &trap)?;
    assert_eq!(code(&failure), "generator.trapped");
    // Not canonical: a space after the status byte.
    let mut sloppy = Fixture::ok()?;
    sloppy.response.insert(1, b' ');
    sloppy.generate = format!("i64.const {}", packed(RESPONSE_AT, sloppy.response.len()));
    assert_eq!(
        code(&refused(&runtime, &sloppy)?),
        "generator.invalid_output"
    );
    // Pad landing a terminal the package lacks.
    let mut wrong = generation();
    if let Some(pad) = wrong.footprint.pads.first_mut() {
        pad.terminal = "2".into();
    }
    let mut dangling = Fixture::ok()?;
    dangling.response = vec![v1::STATUS_OK];
    dangling
        .response
        .extend(codec::encode(&wrong).map_err(|e| e.to_string())?);
    dangling.generate = format!("i64.const {}", packed(RESPONSE_AT, dangling.response.len()));
    let failure = refused(&runtime, &dangling)?;
    assert_eq!(
        failure.to_string(),
        "The generator's pad 1 lands terminal \"2\", which the package does not have. Nothing was changed."
    );
    // A module over the size bound is not even parsed.
    let small = WasmiRuntime::new(Limits {
        module_bytes: 64,
        ..Limits::DEFAULT
    });
    let failure = match small.generate(&Fixture::ok()?.wasm()?, &request()) {
        Err(f) => f,
        Ok(_) => return Err("oversized module ran".into()),
    };
    assert_eq!(code(&failure), "generator.too_large");
    Ok(())
}

#[test]
fn a_module_without_the_abi_exports_is_refused() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let wasm =
        wat::parse_str(r#"(module (memory (export "memory") 1))"#).map_err(|e| e.to_string())?;
    let failure = match runtime.generate(&wasm, &request()) {
        Err(f) => f,
        Ok(_) => return Err("module without exports ran".into()),
    };
    assert_eq!(code(&failure), "generator.missing_export");
    assert_eq!(
        failure.to_string(),
        "The generator does not export \"sparksmith_abi\", which ABI 1 requires. It was not run."
    );
    Ok(())
}

#[test]
fn a_module_using_a_disabled_proposal_is_refused_before_it_runs() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let mut fixture = Fixture::ok()?;
    // Tail calls: not among the features rustc emits for this target.
    fixture.extra = r#"(func $again (return_call $again))"#;
    let failure = refused(&runtime, &fixture)?;
    assert_eq!(code(&failure), "generator.unsupported_feature");
    Ok(())
}

/// Each change to the fixture's model and the refusal it gets, word for
/// word.
#[test]
fn missing_or_malformed_models_are_refused_in_plain_words() -> Result<(), String> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    type Edit = fn(&mut Generation);
    let cases: Vec<(Edit, &str)> = vec![
        (
            |g| g.model = None,
            "The generator made no 3D model; a part needs one.",
        ),
        (
            |g| edit(g, |asset, _| *asset = "chip.solidcraft".into()),
            "The generator names its 3D model's part file \"chip.solidcraft\", which is not a SHA-256 (64 lower-case hex digits).",
        ),
        (
            |g| {
                edit(g, |_, values| {
                    values.insert("side".into(), ParamValue::Int(2_000_000_000));
                })
            },
            "The generator sets the part file's \"side\" to 2000000000, beyond the 1 m bound.",
        ),
        (
            |g| {
                edit(g, |_, values| {
                    values.insert("side".into(), ParamValue::Text("wide".into()));
                })
            },
            "The generator sets the part file's \"side\" to something other than a whole number.",
        ),
        (
            |g| {
                edit(g, |_, values| {
                    values.insert("2nd side".into(), ParamValue::Int(1));
                })
            },
            "The generator gives a value to \"2nd side\", which cannot name a part-file parameter (1 to 64 ASCII letters, digits and underscores, not starting with a digit).",
        ),
    ];
    for (change, message) in cases {
        let mut g = generation();
        change(&mut g);
        let failure = refused(&runtime, &returning(&g)?)?;
        assert_eq!(code(&failure), "generator.invalid_model", "{message}");
        assert_eq!(
            failure.to_string(),
            format!("{message} Nothing was changed.")
        );
    }
    Ok(())
}

fn edit(
    g: &mut Generation,
    change: impl FnOnce(&mut String, &mut std::collections::BTreeMap<String, ParamValue>),
) {
    if let Some(Model::PartFile { asset, values }) = &mut g.model {
        change(asset, values);
    }
}
