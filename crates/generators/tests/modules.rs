//! The built generator modules under the host: golden hashes under wasmi
//! natively and under the wasm32 build of the host (the probe), plus the
//! measurements the limits are set from.
//!
//! These need the modules built, so they are ignored in a plain
//! `cargo test`; `cargo xtask generators check` builds the modules and runs
//! them with `PCBCRAFT_GENERATOR_MODULES` pointing at the build.
use pcbcraft_generator_abi::v1::{self, ErrorBody, Request, Scope};
use pcbcraft_generator_abi::{Failure, codec, hex, result_hash};
use pcbcraft_generators::{Limits, WasmiRuntime, config};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

type TestResult = Result<(), String>;

fn modules_dir() -> Result<PathBuf, String> {
    std::env::var_os("PCBCRAFT_GENERATOR_MODULES")
        .map(PathBuf::from)
        .ok_or_else(|| "set PCBCRAFT_GENERATOR_MODULES (run `cargo xtask generators check`)".into())
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// A generator crate's module and golden cases.
struct Golden {
    name: String,
    module: Vec<u8>,
    cases: Vec<(String, Request, String)>,
}

fn goldens() -> Result<Vec<Golden>, String> {
    let dir = modules_dir()?;
    let generators = workspace().join("generators");
    let mut found = Vec::new();
    let entries = std::fs::read_dir(&generators).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("golden.json").is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    names.sort();
    for name in names {
        let golden: Value = serde_json::from_str(
            &std::fs::read_to_string(generators.join(&name).join("golden.json"))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let module_path = dir.join(format!("pcbcraft_gen_{}.wasm", name.replace('-', "_")));
        let module =
            std::fs::read(&module_path).map_err(|e| format!("{}: {e}", module_path.display()))?;
        let mut cases = Vec::new();
        for case in golden
            .get("cases")
            .and_then(Value::as_array)
            .ok_or("golden.json has no cases")?
        {
            let name = case
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let request: Request =
                serde_json::from_value(case.get("request").cloned().unwrap_or_default())
                    .map_err(|e| format!("{name}: {e}"))?;
            let result = case
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            cases.push((name, request, result));
        }
        found.push(Golden {
            name,
            module,
            cases,
        });
    }
    if found.is_empty() {
        return Err("no generators with golden.json".into());
    }
    Ok(found)
}

#[test]
#[ignore = "needs built modules: cargo xtask generators check"]
fn golden_hashes_match_under_wasmi() -> TestResult {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    for golden in goldens()? {
        let loaded = runtime.load(&golden.module).map_err(|f| f.to_string())?;
        let description = loaded.describe().map_err(|f| f.to_string())?;
        assert_eq!(description.abi, v1::ABI);
        for (name, request, expected) in &golden.cases {
            let generated = loaded
                .generate(request)
                .map_err(|f| format!("{name}: {f}"))?;
            assert_eq!(
                &hex(&generated.result_hash),
                expected,
                "{}: {name}",
                golden.name
            );
            // The hash covers exactly the canonical bytes returned.
            assert_eq!(generated.result_hash, result_hash(&generated.canonical));
            // A second run on a fresh instance gives the same bytes and work.
            let again = loaded.generate(request).map_err(|f| f.to_string())?;
            assert_eq!(again.canonical, generated.canonical);
            assert_eq!(again.fuel_used, generated.fuel_used);
        }
    }
    Ok(())
}

/// The probe's limits: room for the wasmi interpreter itself running the
/// generator under the default limits.
const PROBE_LIMITS: Limits = Limits {
    module_bytes: 16 * 1024 * 1024,
    fuel: 200_000_000_000,
    memory_bytes: 512 * 1024 * 1024,
    table_elements: 100_000,
    request_bytes: 8 * 1024 * 1024,
    description_bytes: 1024 * 1024,
    response_bytes: 8 * 1024 * 1024,
};

fn probe_input(module: &[u8], request: &Request) -> Result<Vec<u8>, String> {
    let len = u32::try_from(module.len()).map_err(|e| e.to_string())?;
    let mut input = len.to_le_bytes().to_vec();
    input.extend_from_slice(module);
    input.extend_from_slice(&codec::encode(request).map_err(|e| e.to_string())?);
    Ok(input)
}

#[test]
#[ignore = "needs built modules: cargo xtask generators check"]
fn golden_hashes_match_under_wasmi_built_for_wasm32() -> TestResult {
    let probe = std::fs::read(modules_dir()?.join("pcbcraft_generators_probe.wasm"))
        .map_err(|e| format!("probe module: {e}"))?;
    let outer = WasmiRuntime::new(PROBE_LIMITS);
    // The wasm32 build of the host imports nothing either.
    let probe = outer.load(&probe).map_err(|f| f.to_string())?;
    let native = WasmiRuntime::new(Limits::DEFAULT);
    for golden in goldens()? {
        let loaded = native.load(&golden.module).map_err(|f| f.to_string())?;
        for (name, request, expected) in &golden.cases {
            let raw = probe
                .generate_raw(&probe_input(&golden.module, request)?)
                .map_err(|f| format!("{name}: {f}"))?;
            if raw.status != v1::STATUS_OK {
                let body: ErrorBody =
                    codec::decode(&raw.body, usize::MAX).map_err(|e| e.to_string())?;
                return Err(format!("{name}: refused in wasm32: {:?}", body.errors));
            }
            let (hash, rest) = raw.body.split_at_checked(32).ok_or("short probe reply")?;
            let (fuel, canonical) = rest.split_at_checked(8).ok_or("short probe reply")?;
            let fuel = u64::from_le_bytes(fuel.try_into().map_err(|_| "fuel")?);
            assert_eq!(
                &hex(hash),
                expected,
                "{}: {name} (wasm32 host)",
                golden.name
            );
            assert_eq!(hash, result_hash(canonical));
            // Same work on both builds of the interpreter.
            let here = loaded.generate(request).map_err(|f| f.to_string())?;
            assert_eq!(
                Some(fuel),
                here.fuel_used,
                "{name}: fuel differs between builds"
            );
        }
    }
    Ok(())
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times.get(times.len() / 2).copied().unwrap_or_default()
}

/// Which WebAssembly features the modules use: each feature of the allowed
/// set is switched off in turn and the module validated without it.
fn features_used(module: &[u8]) -> Vec<&'static str> {
    type Toggle = fn(&mut wasmi::Config) -> &mut wasmi::Config;
    let toggles: [(&str, Toggle); 7] = [
        ("mutable-global", |c| c.wasm_mutable_global(false)),
        ("sign-extension", |c| c.wasm_sign_extension(false)),
        ("saturating-float-to-int", |c| {
            c.wasm_saturating_float_to_int(false)
        }),
        ("multi-value", |c| c.wasm_multi_value(false)),
        ("bulk-memory", |c| c.wasm_bulk_memory(false)),
        ("reference-types", |c| c.wasm_reference_types(false)),
        ("floats", |c| c.floats(false)),
    ];
    toggles
        .into_iter()
        .filter(|(_, toggle)| {
            let mut c = config();
            toggle(&mut c);
            WasmiRuntime::with_config(c, Limits::DEFAULT)
                .load(module)
                .is_err()
        })
        .map(|(name, _)| name)
        .collect()
}

#[test]
#[ignore = "measurement: cargo xtask generators measure"]
fn measure() -> TestResult {
    const RUNS: usize = 21;
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    println!("| generator | module bytes | features used |");
    println!("| --- | --- | --- |");
    let goldens = goldens()?;
    for g in &goldens {
        println!(
            "| {} | {} | {} |",
            g.name,
            g.module.len(),
            features_used(&g.module).join(", ")
        );
    }
    println!();
    println!(
        "| generator | describe fuel | description bytes | load (check + compile) | cold load+describe |"
    );
    println!("| --- | --- | --- | --- | --- |");
    for g in &goldens {
        let mut load = Vec::new();
        let mut cold_describe = Vec::new();
        let mut size = 0;
        for _ in 0..RUNS {
            let t = Instant::now();
            let loaded = runtime.load(&g.module).map_err(|f| f.to_string())?;
            load.push(t.elapsed());
            let t = Instant::now();
            let loaded_again = runtime.load(&g.module).map_err(|f| f.to_string())?;
            let description = loaded_again.describe().map_err(|f| f.to_string())?;
            cold_describe.push(t.elapsed());
            size = codec::encode(&description)
                .map_err(|e| e.to_string())?
                .len();
            drop(loaded);
        }
        println!(
            "| {} | {} | {size} | {:.2?} | {:.2?} |",
            g.name,
            describe_fuel(&g.module)?,
            median(load),
            median(cold_describe)
        );
    }
    println!();
    println!(
        "| case | generate fuel | memory after | request bytes | response bytes | cold load+generate | warm generate |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    for g in &goldens {
        for (name, request, _) in &g.cases {
            let request_bytes = codec::encode(request).map_err(|e| e.to_string())?;
            let mut cold_generate = Vec::new();
            let mut warm = Vec::new();
            let mut last = None;
            for _ in 0..RUNS {
                let t = Instant::now();
                let loaded = runtime.load(&g.module).map_err(|f| f.to_string())?;
                loaded.generate(request).map_err(|f| f.to_string())?;
                cold_generate.push(t.elapsed());
                let t = Instant::now();
                let raw = loaded
                    .generate_raw(&request_bytes)
                    .map_err(|f| f.to_string())?;
                warm.push(t.elapsed());
                last = Some(raw);
            }
            let raw = last.ok_or("no runs")?;
            println!(
                "| {}: {name} | {} | {} KiB | {} | {} | {:.2?} | {:.2?} |",
                g.name,
                raw.fuel_used,
                raw.memory_bytes / 1024,
                request_bytes.len(),
                raw.body.len() + 1,
                median(cold_generate),
                median(warm)
            );
        }
    }
    // The probe: the wasm32 host's own size.
    if let Ok(probe) = std::fs::read(modules_dir()?.join("pcbcraft_generators_probe.wasm")) {
        println!("\nwasm32 build of the host (probe): {} bytes", probe.len());
    }
    Ok(())
}

/// Fuel of a describe call: the least budget under which it succeeds.
fn describe_fuel(module: &[u8]) -> Result<u64, String> {
    let ok = |fuel: u64| {
        WasmiRuntime::new(Limits {
            fuel,
            ..Limits::DEFAULT
        })
        .load(module)
        .and_then(|m| m.describe())
        .is_ok()
    };
    let (mut lo, mut hi) = (0u64, Limits::DEFAULT.fuel);
    if !ok(hi) {
        return Err("describe does not finish within the default fuel".into());
    }
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if ok(mid) { hi = mid } else { lo = mid }
    }
    Ok(hi)
}

#[test]
#[ignore = "needs built modules: cargo xtask generators check"]
fn host_completes_defaults_and_refuses_bad_params_before_running() -> TestResult {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    for g in goldens()? {
        let loaded = runtime.load(&g.module).map_err(|f| f.to_string())?;
        let description = loaded.describe().map_err(|f| f.to_string())?;
        // An empty request completes to the defaults and generates.
        let request =
            pcbcraft_generators::params::complete(&description, Scope::Package, &v1::Params::new())
                .map_err(|f| f.to_string())?;
        loaded.generate(&request).map_err(|f| f.to_string())?;
        // An unknown parameter is refused by the host, naming it.
        let mut given = v1::Params::new();
        given.insert("no_such_param".into(), v1::ParamValue::Int(1));
        let refused: Result<Request, Failure> =
            pcbcraft_generators::params::complete(&description, Scope::Package, &given);
        assert_eq!(
            refused.err().and_then(|f| f.code().map(str::to_string)),
            Some("param.unknown".into())
        );
    }
    Ok(())
}
