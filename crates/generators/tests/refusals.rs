//! Refusals: hand-written modules that break one rule each. Every refusal
//! is a sentence a person can act on that says nothing ran or nothing
//! changed, and the runtime keeps working for the next module.
use sparksmith_generator_abi::v1::{
    self, Footprint, Generation, Lead, Opening, Package, PackageGeometry, Pad, PadShape, Request,
    Scope, Side,
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
        id_hint: "fixture".into(),
        name: "Fixture".into(),
        families: Vec::new(),
        scopes: vec![Scope::Package],
        params: Vec::new(),
        rules: Vec::new(),
        presets: Vec::new(),
        grammar: Vec::new(),
        instance_fields: Vec::new(),
    }
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
        model: None,
        fields: Vec::new(),
        notes: Vec::new(),
    }
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
    /// The body of `sparksmith_gen_generate`, which must leave an i64.
    generate: String,
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
        })
    }

    fn wasm(&self) -> Result<Vec<u8>, String> {
        let description = codec::encode(&description()).map_err(|e| e.to_string())?;
        let text = format!(
            r#"(module
              {extra}
              (memory (export "memory") {pages})
              (global $calls (mut i32) (i32.const 0))
              (data (i32.const {DESCRIPTION_AT}) "{description_data}")
              (data (i32.const {RESPONSE_AT}) "{response_data}")
              (func (export "sparksmith_gen_abi") (result i32) i32.const 1)
              (func (export "sparksmith_gen_alloc") (param i32) (result i32) i32.const {REQUEST_AT})
              (func (export "sparksmith_gen_describe") (result i64) i64.const {describe})
              (func (export "sparksmith_gen_generate") (param i32 i32) (result i64)
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
        "This generator asks for host functions (env.clock), which generators may not use. It was not run."
    );
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
        "The generator does not export \"sparksmith_gen_abi\", which ABI 1 requires. It was not run."
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
