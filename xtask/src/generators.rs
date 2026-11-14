//! `cargo xtask generators build | check | reproduce | measure`: the
//! generator modules in `generators/*`.
//!
//! A module has one hash: the SHA-256 of its committed copy,
//! `library/generators/<name>/<name>.wasm`, which the bundle's manifest
//! records. The build is reproducible across checkout paths and hosts
//! (macOS and Linux measured; `generators/README.md`, "Reproducibility"),
//! so every host compares its build with that one file.
//!
//! - `build [--ship]` builds every generator crate to
//!   `wasm32-unknown-unknown` in the `generator` profile, each in its own
//!   cargo invocation (so no generator's features leak into another), with
//!   the workspace and Cargo home remapped out of any embedded path and
//!   rustc run through [`wrap_rustc`]. It prints each module's size and
//!   SHA-256 and notes a difference from the committed copy; `--ship`
//!   writes each generator's bundle to the shipped library
//!   (`library/generators/<name>/`: `generator.json`, the module and the
//!   crate's `parts/`, committed), the only way it changes.
//! - `check` (part of `cargo xtask ci`) builds and fails when a module
//!   differs from its committed copy even after a rebuild from an empty
//!   target directory (not compared on Windows: [`COMPARE_BYTES`]) or a
//!   shipped bundle is not its generator's module with the crate's part
//!   files, and runs the golden cases natively, under wasmi natively, and
//!   under wasmi built for wasm32 (the probe module).
//! - `reproduce` builds the modules twice from empty target directories
//!   and compares the bytes with each other and with the committed copies.
//! - `measure` prints module sizes, cold and warm times and fuel (the data
//!   the host's limits are set from).
use crate::{Result, root};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

const TARGET: &str = "wasm32-unknown-unknown";
const PROFILE: &str = "generator";
/// Where the built modules ship (`sparksmith_io::shipped::Shelf::Generators`).
const SHIPPED: &str = "library/generators";
/// The probe: the generator host itself built for wasm32 (a test module,
/// not shipped: it only has to agree with the native host).
const PROBE: &str = "sparksmith-generators-probe";
/// A test plugin (`crates/generators-fixture`): not shipped;
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
/// sources and `Cargo.lock` give the same bytes.
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
    // Every `-C metadata` is dropped (cargo's, and the cache key that
    // `rust_flags` adds) and this one value is given instead.
    let mut out: Vec<std::ffi::OsString> = Vec::with_capacity(rest.len() + 2);
    let mut k = 0;
    while k < rest.len() {
        let arg = text[k].as_str();
        if arg == "-C" && text.get(k + 1).is_some_and(|v| v.starts_with("metadata=")) {
            k += 2;
            continue;
        }
        if !arg.starts_with("-Cmetadata=") {
            out.push(rest[k].clone());
        }
        k += 1;
    }
    out.push("-C".into());
    out.push(format!("metadata={}", &digest[..16]).into());
    let status = Command::new(rustc).args(&out).status()?;
    Ok(match status.code().and_then(|c| u8::try_from(c).ok()) {
        Some(0) => std::process::ExitCode::SUCCESS,
        Some(code) => std::process::ExitCode::from(code),
        None => std::process::ExitCode::FAILURE,
    })
}

/// One generator crate under `generators/`.
struct Generator {
    /// The directory name, also the module's name in its bundle.
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
        ["build", "--ship"] => build_and_report(true),
        ["check"] => check(),
        ["reproduce"] => reproduce(),
        ["measure"] => measure(),
        _ => {
            Err("usage: cargo xtask generators build [--ship] | check | reproduce | measure".into())
        }
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

/// The Rust flags of a generator build: the remaps that keep build-machine
/// paths out of the module, and a cache key naming this program, the rustc
/// wrapper. Cargo's fingerprint ignores `RUSTC_WRAPPER`, so without the key
/// a crate compiled earlier into the same target directory without the
/// wrapper (or by another version of it) is reused, keeping cargo's
/// `-C metadata`, which names the host, and with it another function
/// order: a checkout whose `target/generators` predated the wrapper built
/// other bytes than a clean one. The flags are fingerprinted, so a new
/// wrapper recompiles everything; the wrapper drops the key before rustc
/// sees it ([`wrap_rustc`]).
fn rust_flags(root: &Path) -> Result<String> {
    let wrapper = std::fs::read(std::env::current_exe()?)?;
    let key = hex(&Sha256::digest(&wrapper));
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
        "--remap-path-prefix={root}=/src\u{1f}--remap-path-prefix={cargo_home}=/cargo\u{1f}-Cmetadata=wrapper-{}",
        &key[..16]
    ))
}

fn build_package(package: &str, target_dir: &Path) -> Result<()> {
    let root = root()?;
    eprintln!("building {package} for {TARGET} ({PROFILE} profile)");
    let mut command = Command::new(cargo_bin());
    // The `generator` profile applies as written: incremental compilation
    // or a profile override from the environment would change the bytes.
    command.env_remove("CARGO_INCREMENTAL");
    for (key, _) in std::env::vars_os() {
        if key
            .to_str()
            .is_some_and(|k| k.starts_with("CARGO_PROFILE_GENERATOR_"))
        {
            command.env_remove(key);
        }
    }
    let status = command
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
        .env("CARGO_ENCODED_RUSTFLAGS", rust_flags(&root)?)
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

/// The committed module of generator `name`, which its bundle's manifest
/// names by SHA-256: the one hash of the module.
fn shipped_module(name: &str) -> Result<PathBuf> {
    Ok(root()?
        .join(SHIPPED)
        .join(name)
        .join(format!("{name}.wasm")))
}

/// Each built module that differs from its committed copy, in words (a
/// missing copy differs too).
fn differing(built: &[Built]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for b in built {
        let file = format!("{SHIPPED}/{}/{}.wasm", b.name, b.name);
        match std::fs::read(shipped_module(&b.name)?) {
            Ok(bytes) => {
                let hash = hex(&Sha256::digest(&bytes));
                if hash != b.sha256 {
                    out.push(format!(
                        "module {} builds to sha256 {} ({} bytes), but {file} is sha256 {hash} ({} bytes)",
                        b.name,
                        b.sha256,
                        b.bytes,
                        bytes.len()
                    ));
                }
            }
            Err(e) => out.push(format!(
                "{file} cannot be read ({e}); the build is sha256 {}",
                b.sha256
            )),
        }
    }
    Ok(out)
}

/// Whether this host compares the built modules' bytes with the committed
/// ones. Not on Windows: the remapped source paths in panic locations keep
/// Windows' backslashes there (`/src\crates\…`), so its bytes differ from
/// the macOS and Linux build, which is the one committed. Windows checks
/// the bundle and runs the golden cases only; it records no hash of its
/// own.
const COMPARE_BYTES: bool = !cfg!(windows);

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

/// Each shipped bundle holds its module, the crate's part files and the
/// manifest naming them, and nothing else ships. (That the module is what
/// the source builds is [`differing`]'s check.)
fn review_shipped() -> Result<Vec<String>> {
    let shelf = root()?.join(SHIPPED);
    let generators = generators()?;
    let mut failures = Vec::new();
    for generator in &generators {
        let dir = shelf.join(&generator.name);
        let Ok(module) = std::fs::read(shipped_module(&generator.name)?) else {
            failures.push(format!(
                "{SHIPPED}/{}/{}.wasm is missing",
                generator.name, generator.name
            ));
            continue;
        };
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

fn build_and_report(ship_it: bool) -> Result<()> {
    let built = build_all(&target_dir()?, false)?;
    report(&built);
    if ship_it {
        ship(&target_dir()?)
    } else {
        for line in differing(&built)? {
            eprintln!("note: {line}");
        }
        Ok(())
    }
}

fn check() -> Result<()> {
    let dir = target_dir()?;
    let mut built = build_all(&dir, true)?;
    report(&built);
    if COMPARE_BYTES {
        let mut differ = differing(&built)?;
        if !differ.is_empty() {
            // A target directory can hold artifacts that cargo reuses but
            // a clean build would not make (for example from an older
            // build recipe), so a mismatch counts only if a build from an
            // empty directory repeats it.
            for line in &differ {
                eprintln!("note: {line}");
            }
            eprintln!(
                "note: rebuilding from an empty {} to rule out stale build artifacts",
                dir.display()
            );
            std::fs::remove_dir_all(&dir)?;
            built = build_all(&dir, true)?;
            report(&built);
            differ = differing(&built)?;
        }
        if !differ.is_empty() {
            return Err(format!(
                "a clean build of the generator modules differs from the committed ones:\n  {}\nIf the source change is intended, run `cargo xtask generators build --ship` and commit {SHIPPED}.",
                differ.join("\n  ")
            )
            .into());
        }
        eprintln!("generator modules build to the committed {SHIPPED}/*/*.wasm");
    } else {
        eprintln!(
            "note: module bytes are not compared on Windows (see COMPARE_BYTES); checking the bundles and golden cases only"
        );
    }
    let failures = review_shipped()?;
    if !failures.is_empty() {
        return Err(format!(
            "generator bundles in {SHIPPED} are inconsistent:\n  {}\nRun `cargo xtask generators build --ship` and commit {SHIPPED}.",
            failures.join("\n  ")
        )
        .into());
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
    if COMPARE_BYTES {
        problems.extend(differing(a)?);
    }
    if problems.is_empty() {
        eprintln!("two clean builds gave identical modules, equal to the committed ones");
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
