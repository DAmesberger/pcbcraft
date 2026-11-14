//! `cargo xtask generators build | check | reproduce | measure`: the
//! generator modules in `generators/*`.
//!
//! - `build [--update-lock]` builds every generator crate to
//!   `wasm32-unknown-unknown` in the `generator` profile, each in its own
//!   cargo invocation (so no generator's features leak into another), with
//!   the workspace and Cargo home remapped out of any embedded path. It
//!   prints each module's size and SHA-256; `--update-lock` records the
//!   hashes for this build host in `generators/modules.lock`, the only way
//!   that file changes, and writes each generator's bundle to the shipped
//!   library (`library/generators/<name>/`: `generator.json`, the module and
//!   the crate's `parts/`, committed). Bytes are reproducible
//!   per build host (see [`host_triple`]), so the lock keeps one hash per
//!   host, and every host ships the one committed copy, so a part's key is
//!   the same wherever it is made (docs/generator-plugins.md).
//! - `check` (part of `cargo xtask ci`) builds, fails when a module's hash
//!   differs from the hash `modules.lock` records for this host (a host
//!   with none recorded gets a note) or a shipped bundle is not a locked
//!   build of its generator with the crate's part files, and runs the golden cases natively, under
//!   wasmi natively, and under wasmi built for wasm32 (the probe module).
//! - `reproduce` builds everything twice from empty target directories and
//!   compares the bytes with each other and with the lock.
//! - `measure` prints module sizes, cold and warm times and fuel (the data
//!   the host's limits are set from).
use crate::{Result, root};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

const TARGET: &str = "wasm32-unknown-unknown";
const PROFILE: &str = "generator";
const LOCK: &str = "generators/modules.lock";
/// Where the built modules ship (`sparksmith_io::shipped::Shelf::Generators`).
const SHIPPED: &str = "library/generators";
/// The probe: the generator host itself built for wasm32 (a test module,
/// not locked: it only has to agree with the native host).
const PROBE: &str = "sparksmith-generators-probe";
/// A test plugin (`crates/generators-fixture`): not locked or shipped;
/// its golden case runs natively, under wasmi and under the probe.
const FIXTURE: &str = "sparksmith-gen-fixture";
/// Where `check` points the host's module tests.
const MODULES_ENV: &str = "SPARKSMITH_GENERATOR_MODULES";
/// Set on the generator builds, so this program runs as their rustc
/// wrapper ([`wrap_rustc`]).
pub const WRAP_ENV: &str = "SPARKSMITH_GENERATOR_RUSTC";

/// Run as cargo's rustc wrapper for a generator build: `args` are the
/// rustc program and its arguments. Cargo's `-C metadata` value hashes,
/// among other things, rustc's verbose version, which names the build
/// host; the value seeds each crate's stable id, and with it the order of
/// the module's functions. It is replaced by a hash of what identifies the
/// crate within the build (package name and version, crate name and types,
/// the `--cfg` set and the target), so two hosts with the same toolchain,
/// sources and lock give the same bytes.
pub fn wrap_rustc(args: &[std::ffi::OsString]) -> Result<std::process::ExitCode> {
    let (rustc, rest) = args
        .split_first()
        .ok_or("the rustc wrapper was called without a program")?;
    let text: Vec<String> = rest
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut identity = Vec::new();
    for key in ["CARGO_PKG_NAME", "CARGO_PKG_VERSION"] {
        identity.push(std::env::var(key).unwrap_or_default());
    }
    let mut k = 0;
    while k < text.len() {
        let arg = text[k].as_str();
        if matches!(arg, "--crate-name" | "--crate-type" | "--cfg" | "--target") {
            let value = text.get(k + 1).map(String::as_str).unwrap_or_default();
            identity.push(format!("{arg}={value}"));
            k += 1;
        }
        k += 1;
    }
    let digest = hex(&Sha256::digest(identity.join("\u{1f}").as_bytes()));
    let metadata = format!("metadata={}", &digest[..16]);
    let mut out: Vec<std::ffi::OsString> = Vec::with_capacity(rest.len());
    let mut k = 0;
    while k < rest.len() {
        let arg = text[k].as_str();
        if arg == "-C" && text.get(k + 1).is_some_and(|v| v.starts_with("metadata=")) {
            out.push("-C".into());
            out.push(metadata.clone().into());
            k += 2;
            continue;
        }
        if arg.starts_with("-Cmetadata=") {
            out.push(format!("-C{metadata}").into());
        } else {
            out.push(rest[k].clone());
        }
        k += 1;
    }
    let status = Command::new(rustc).args(&out).status()?;
    Ok(match status.code().and_then(|c| u8::try_from(c).ok()) {
        Some(0) => std::process::ExitCode::SUCCESS,
        Some(code) => std::process::ExitCode::from(code),
        None => std::process::ExitCode::FAILURE,
    })
}

/// One generator crate under `generators/`.
struct Generator {
    /// The directory name, also the module's name in the lock.
    name: String,
    package: String,
}

impl Generator {
    fn artifact(&self, target_dir: &Path) -> PathBuf {
        artifact(target_dir, &self.package)
    }
}

fn artifact(target_dir: &Path, package: &str) -> PathBuf {
    target_dir
        .join(TARGET)
        .join(PROFILE)
        .join(format!("{}.wasm", package.replace('-', "_")))
}

pub fn run(args: &[String]) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["build"] => build_and_report(false),
        ["build", "--update-lock"] => build_and_report(true),
        ["check"] => check(),
        ["reproduce"] => reproduce(),
        ["measure"] => measure(),
        _ => Err(
            "usage: cargo xtask generators build [--update-lock] | check | reproduce | measure"
                .into(),
        ),
    }
}

fn generators() -> Result<Vec<Generator>> {
    let root = root()?;
    let output = Command::new(cargo_bin())
        .args(["metadata", "--locked", "--format-version", "1", "--no-deps"])
        .current_dir(&root)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let data: Value = serde_json::from_slice(&output.stdout)?;
    let packages = data
        .get("packages")
        .and_then(Value::as_array)
        .ok_or("metadata has no packages")?;
    let base = root.join("generators");
    let mut found = Vec::new();
    for package in packages {
        let (Some(name), Some(manifest)) = (
            package.get("name").and_then(Value::as_str),
            package.get("manifest_path").and_then(Value::as_str),
        ) else {
            continue;
        };
        let manifest = Path::new(manifest);
        let Some(dir) = manifest.parent() else {
            continue;
        };
        if dir.parent() == Some(base.as_path()) {
            let dir_name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("generator directory name is not UTF-8")?;
            found.push(Generator {
                name: dir_name.to_string(),
                package: name.to_string(),
            });
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    if found.is_empty() {
        return Err("no generator crates under generators/".into());
    }
    Ok(found)
}

fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// Rust flags that keep build-machine paths out of the module.
fn remap_flags(root: &Path) -> Result<String> {
    let cargo_home = match std::env::var_os("CARGO_HOME") {
        Some(home) => PathBuf::from(home),
        None => std::env::home_dir()
            .ok_or("cannot find the home directory for CARGO_HOME")?
            .join(".cargo"),
    };
    let root = root.to_str().ok_or("workspace path is not UTF-8")?;
    let cargo_home = cargo_home.to_str().ok_or("CARGO_HOME is not UTF-8")?;
    // Separated by 0x1f, as CARGO_ENCODED_RUSTFLAGS requires.
    Ok(format!(
        "--remap-path-prefix={root}=/src\u{1f}--remap-path-prefix={cargo_home}=/cargo"
    ))
}

fn build_package(package: &str, target_dir: &Path) -> Result<()> {
    let root = root()?;
    eprintln!("building {package} for {TARGET} ({PROFILE} profile)");
    let status = Command::new(cargo_bin())
        .args([
            "build",
            "--locked",
            "--package",
            package,
            "--lib",
            "--target",
            TARGET,
            "--profile",
            PROFILE,
            "--target-dir",
        ])
        .arg(target_dir)
        // The flags replace any RUSTFLAGS of the environment, so the bytes do
        // not depend on the caller's settings.
        .env("CARGO_ENCODED_RUSTFLAGS", remap_flags(&root)?)
        .env_remove("RUSTFLAGS")
        // Every rustc call goes through this program (`wrap_rustc`), which
        // gives each crate a `-C metadata` that does not name the build
        // host; any caching wrapper of the environment is left out.
        .env("RUSTC_WRAPPER", std::env::current_exe()?)
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env(WRAP_ENV, "1")
        .current_dir(&root)
        .status()?;
    if !status.success() {
        return Err(format!("building {package} failed: {status}").into());
    }
    Ok(())
}

/// A built module's name, size and hash.
struct Built {
    name: String,
    package: String,
    bytes: usize,
    sha256: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn build_all(target_dir: &Path, with_probe: bool) -> Result<Vec<Built>> {
    let mut built = Vec::new();
    for generator in generators()? {
        build_package(&generator.package, target_dir)?;
        let path = generator.artifact(target_dir);
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        built.push(Built {
            name: generator.name,
            package: generator.package,
            bytes: bytes.len(),
            sha256: hex(&Sha256::digest(&bytes)),
        });
    }
    if with_probe {
        build_package(PROBE, target_dir)?;
        build_package(FIXTURE, target_dir)?;
    }
    Ok(built)
}

fn target_dir() -> Result<PathBuf> {
    Ok(root()?.join("target").join("generators"))
}

fn toolchain() -> Result<String> {
    let text = std::fs::read_to_string(root()?.join("rust-toolchain.toml"))?;
    text.lines()
        .find_map(|line| {
            let rest = line
                .trim()
                .strip_prefix("channel")?
                .trim()
                .strip_prefix('=')?;
            Some(rest.trim().trim_matches('"').to_string())
        })
        .ok_or_else(|| "rust-toolchain.toml has no channel".into())
}

/// The build host's target triple, from `rustc -vV`. Module bytes are
/// reproducible per build host: cargo mixes rustc's verbose version, which
/// names the host, into every crate's `-C metadata`, and that reorders the
/// module's functions.
fn host_triple() -> Result<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .arg("-vV")
        .current_dir(root()?)
        .output()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        .ok_or_else(|| "rustc -vV names no host".into())
}

fn read_lock() -> Result<Value> {
    let path = root()?.join(LOCK);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(
            serde_json::from_str(&text).map_err(|e| format!("{LOCK} is not valid JSON: {e}"))?
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(e.into()),
    }
}

/// The recorded hashes of module `name`, by build host.
fn recorded<'a>(lock: &'a Value, name: &str) -> Option<&'a serde_json::Map<String, Value>> {
    lock.get("modules")?
        .as_array()?
        .iter()
        .find(|e| e.get("name").and_then(Value::as_str) == Some(name))?
        .get("sha256")?
        .as_object()
}

/// The lock's header, which must match the build exactly.
fn header() -> Result<Value> {
    Ok(json!({
        "format": 1,
        "rustc": toolchain()?,
        "target": TARGET,
        "profile": PROFILE,
    }))
}

/// Compare `built` with the lock for `host`: failures (a recorded hash
/// differs, a module is missing or extra, the header differs) and notes (no
/// hash recorded for this host yet).
fn review_lock(built: &[Built], host: &str) -> Result<(Vec<String>, Vec<String>)> {
    let lock = read_lock()?;
    let mut failures = Vec::new();
    let mut notes = Vec::new();
    if let Value::Object(header) = header()? {
        for (key, value) in header {
            if lock.get(&key) != Some(&value) {
                failures.push(format!(
                    "{LOCK} has {key} {:?}; the build uses {value}",
                    lock.get(&key)
                ));
            }
        }
    }
    let locked_names: Vec<&str> = lock
        .get("modules")
        .and_then(Value::as_array)
        .map(|m| {
            m.iter()
                .filter_map(|e| e.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    for name in &locked_names {
        if !built.iter().any(|b| b.name == *name) {
            failures.push(format!(
                "{LOCK} lists module {name}, which no longer exists"
            ));
        }
    }
    for b in built {
        match recorded(&lock, &b.name).map(|hosts| hosts.get(host)) {
            None => failures.push(format!("module {} is not in {LOCK}", b.name)),
            Some(None) => notes.push(format!(
                "{LOCK} has no hash of module {} built on {host}; this build is sha256 {} (record it with `cargo xtask generators build --update-lock`)",
                b.name, b.sha256
            )),
            Some(Some(hash)) if hash.as_str() == Some(b.sha256.as_str()) => {}
            Some(Some(hash)) => failures.push(format!(
                "module {} built on {host} is sha256 {}, but {LOCK} has {}",
                b.name,
                b.sha256,
                hash.as_str().unwrap_or("?")
            )),
        }
    }
    Ok((failures, notes))
}

/// Record `built` for `host`. A module whose recorded hash on this host
/// changed keeps no other host's hash: those are stale until rebuilt
/// there. A host recording its first hash adds it beside the others,
/// since bytes differ per host even from the same source.
fn update_lock(built: &[Built], host: &str) -> Result<()> {
    let lock = read_lock()?;
    let mut modules = Vec::new();
    for b in built {
        let mut hosts = recorded(&lock, &b.name).cloned().unwrap_or_default();
        let before = hosts.get(host).and_then(Value::as_str);
        if before.is_none() {
            hosts.insert(host.to_string(), Value::String(b.sha256.clone()));
        } else if before != Some(b.sha256.as_str()) {
            let stale: Vec<&String> = hosts.keys().filter(|h| *h != host).collect();
            if !stale.is_empty() {
                eprintln!(
                    "note: module {} changed; dropped the hashes recorded on {} (rebuild there with --update-lock)",
                    b.name,
                    stale
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            hosts.clear();
            hosts.insert(host.to_string(), Value::String(b.sha256.clone()));
        }
        modules.push(json!({
            "name": b.name,
            "package": b.package,
            "sha256": hosts,
        }));
    }
    let mut lock = header()?;
    if let Value::Object(map) = &mut lock {
        map.insert("modules".into(), Value::Array(modules));
    }
    std::fs::write(
        root()?.join(LOCK),
        format!("{}\n", serde_json::to_string_pretty(&lock)?),
    )?;
    eprintln!("wrote {LOCK} for {host}");
    Ok(())
}

/// The bundle of `generator` as it ships (`library/generators/<name>/`):
/// each file's path inside the bundle and bytes, the manifest
/// (`generator.json`) last. The part files and their licence and
/// provenance come from the crate's `parts/` (`<stem>.solidcraft` with its
/// `<stem>.part.json` sidecar, the shipped part files' format), the module
/// from `module`.
fn bundle(generator: &Generator, module: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let source = root()?
        .join("generators")
        .join(&generator.name)
        .join("parts");
    let mut stems = Vec::new();
    if source.is_dir() {
        for entry in std::fs::read_dir(&source)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".solidcraft") {
                stems.push(stem.to_string());
            }
        }
    }
    stems.sort();
    let module_file = format!("{}.wasm", generator.name);
    let mut files = vec![(module_file.clone(), module.to_vec())];
    let mut parts = Vec::new();
    for stem in stems {
        let bytes = std::fs::read(source.join(format!("{stem}.solidcraft")))?;
        let sidecar_path = source.join(format!("{stem}.part.json"));
        let sidecar: Value =
            serde_json::from_slice(&std::fs::read(&sidecar_path).map_err(|e| {
                format!(
                    "{}: {e} (each part file needs its sidecar)",
                    sidecar_path.display()
                )
            })?)?;
        let text = |key: &str| -> Result<String> {
            sidecar
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("{}: no {key}", sidecar_path.display()).into())
        };
        let file = format!("parts/{stem}.solidcraft");
        parts.push(json!({
            "file": file,
            "sha256": hex(&Sha256::digest(&bytes)),
            "title": text("title")?,
            "licence": text("licence")?,
            "provenance": text("provenance")?,
        }));
        files.push((file, bytes));
    }
    let manifest = json!({
        "format": 1,
        "name": generator.name,
        "module": {"file": module_file, "sha256": hex(&Sha256::digest(module))},
        "parts": parts,
    });
    let mut text = serde_json::to_string_pretty(&manifest)?;
    text.push('\n');
    files.push(("generator.json".into(), text.into_bytes()));
    Ok(files)
}

/// Write each built module's bundle from `target_dir` to the shipped
/// library, replacing what was there.
fn ship(target_dir: &Path) -> Result<()> {
    let shelf = root()?.join(SHIPPED);
    std::fs::create_dir_all(&shelf)?;
    for generator in generators()? {
        let module = std::fs::read(generator.artifact(target_dir))?;
        let dir = shelf.join(&generator.name);
        if dir.is_dir() {
            std::fs::remove_dir_all(&dir)?;
        }
        for (file, bytes) in bundle(&generator, &module)? {
            let to = dir.join(&file);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&to, bytes)?;
        }
        eprintln!("shipped {}", dir.display());
    }
    Ok(())
}

/// Each shipped bundle holds a build of its generator that the lock
/// records (on any host), the crate's part files and the manifest naming
/// them, and nothing else ships.
fn review_shipped() -> Result<Vec<String>> {
    let lock = read_lock()?;
    let shelf = root()?.join(SHIPPED);
    let generators = generators()?;
    let mut failures = Vec::new();
    for generator in &generators {
        let dir = shelf.join(&generator.name);
        let module_path = dir.join(format!("{}.wasm", generator.name));
        let Ok(module) = std::fs::read(&module_path) else {
            failures.push(format!(
                "{SHIPPED}/{}/{}.wasm is missing",
                generator.name, generator.name
            ));
            continue;
        };
        let hash = hex(&Sha256::digest(&module));
        let locked = recorded(&lock, &generator.name)
            .is_some_and(|hosts| hosts.values().any(|h| h.as_str() == Some(hash.as_str())));
        if !locked {
            failures.push(format!(
                "{SHIPPED}/{}/{}.wasm is sha256 {hash}, which {LOCK} does not record for {}",
                generator.name, generator.name, generator.name
            ));
        }
        let expected = bundle(generator, &module)?;
        for (file, bytes) in &expected {
            match std::fs::read(dir.join(file)) {
                Ok(shipped) if &shipped == bytes => {}
                Ok(_) => failures.push(format!(
                    "{SHIPPED}/{}/{file} differs from what generators/{} builds",
                    generator.name, generator.name
                )),
                Err(_) => failures.push(format!("{SHIPPED}/{}/{file} is missing", generator.name)),
            }
        }
        let mut shipped_files = Vec::new();
        list_files(&dir, &dir, &mut shipped_files)?;
        for file in shipped_files {
            if !expected.iter().any(|(f, _)| *f == file) {
                failures.push(format!(
                    "{SHIPPED}/{}/{file} is not part of the generator's bundle",
                    generator.name
                ));
            }
        }
    }
    if shelf.is_dir() {
        for entry in std::fs::read_dir(&shelf)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if !generators.iter().any(|g| g.name == name) {
                failures.push(format!(
                    "{SHIPPED}/{name} is not the bundle of a generator crate"
                ));
            }
        }
    }
    Ok(failures)
}

/// Every file under `dir`, as paths relative to `base` with `/`.
fn list_files(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            list_files(base, &path, out)?;
        } else if let Ok(relative) = path.strip_prefix(base) {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn report(built: &[Built]) {
    for b in built {
        eprintln!("  {:<12} {:>9} bytes  sha256 {}", b.name, b.bytes, b.sha256);
    }
}

fn build_and_report(update: bool) -> Result<()> {
    let built = build_all(&target_dir()?, false)?;
    report(&built);
    let host = host_triple()?;
    if update {
        update_lock(&built, &host)?;
        ship(&target_dir()?)
    } else {
        let (failures, notes) = review_lock(&built, &host)?;
        for line in failures.iter().chain(&notes) {
            eprintln!("note: {line}");
        }
        Ok(())
    }
}

fn check() -> Result<()> {
    let dir = target_dir()?;
    let built = build_all(&dir, true)?;
    report(&built);
    let host = host_triple()?;
    let (mut failures, notes) = review_lock(&built, &host)?;
    failures.extend(review_shipped()?);
    for note in &notes {
        eprintln!("note: {note}");
    }
    if !failures.is_empty() {
        return Err(format!(
            "generator modules do not match {LOCK}:\n  {}\nA module's hash is its identity. If the change is intended, run `cargo xtask generators build --update-lock` and commit {LOCK} and {SHIPPED}.",
            failures.join("\n  ")
        )
        .into());
    }
    if notes.is_empty() {
        eprintln!("generator modules match {LOCK} for {host}");
    }
    golden_native()?;
    module_tests(&dir, &["--skip", "measure"])?;
    Ok(())
}

/// The golden cases natively, in each generator crate and the fixture.
fn golden_native() -> Result<()> {
    let mut packages: Vec<String> = generators()?.into_iter().map(|g| g.package).collect();
    packages.push(FIXTURE.into());
    for package in &packages {
        crate::cargo(&[
            "test",
            "--locked",
            "--package",
            package,
            "--lib",
            "--profile",
            "ci",
            "golden",
        ])?;
    }
    Ok(())
}

/// The host's module tests (`crates/generators/tests/modules.rs`), against
/// the modules in `dir`.
fn module_tests(dir: &Path, extra: &[&str]) -> Result<()> {
    let mut args = vec![
        "test",
        "--locked",
        "--package",
        "sparksmith-generators",
        "--test",
        "modules",
        "--profile",
        "ci",
        "--",
        "--ignored",
    ];
    args.extend_from_slice(extra);
    eprintln!("cargo {}", args.join(" "));
    let modules = dir.join(TARGET).join(PROFILE);
    let status = Command::new(cargo_bin())
        .args(&args)
        .env(MODULES_ENV, &modules)
        .current_dir(root()?)
        .status()?;
    if !status.success() {
        return Err(format!("generator module tests failed: {status}").into());
    }
    Ok(())
}

fn reproduce() -> Result<()> {
    let base = root()?.join("target");
    let mut runs = Vec::new();
    for run in ["generators-repro-a", "generators-repro-b"] {
        let dir = base.join(run);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        runs.push(build_all(&dir, false)?);
    }
    let (Some(a), Some(b)) = (runs.first(), runs.get(1)) else {
        return Err("reproduce made fewer than two builds".into());
    };
    report(a);
    let mut problems = Vec::new();
    for (x, y) in a.iter().zip(b) {
        if x.sha256 != y.sha256 {
            problems.push(format!(
                "module {} differs between two clean builds: {} and {}",
                x.name, x.sha256, y.sha256
            ));
        }
    }
    let (failures, notes) = review_lock(a, &host_triple()?)?;
    problems.extend(failures);
    for note in &notes {
        eprintln!("note: {note}");
    }
    if problems.is_empty() {
        eprintln!("two clean builds gave identical modules, agreeing with {LOCK}");
        Ok(())
    } else {
        Err(problems.join("\n").into())
    }
}

fn measure() -> Result<()> {
    let dir = target_dir()?;
    let built = build_all(&dir, true)?;
    report(&built);
    module_tests(&dir, &["--nocapture", "--test-threads", "1", "measure"])
}
