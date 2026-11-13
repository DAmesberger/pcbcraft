mod brand_crates;
mod clean_room;
mod connectivity;
mod convert_rules;
mod convert_v1;
mod core_modules;
mod f64_lengths;
mod generators;
mod generic_terms;
mod kicad_compare;
mod layers;
mod licences;
mod no_builtin_data;
mod raw_keys;
mod ui_containers;
mod ui_forms;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "cannot find workspace root".into())
}

fn cargo(args: &[&str]) -> Result<()> {
    eprintln!("cargo {}", args.join(" "));
    let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(args)
        .current_dir(root()?)
        .status()?;
    if !status.success() {
        return Err(format!("cargo {} failed: {status}", args.join(" ")).into());
    }
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let Some((command, files)) = args.split_first()
        && command == "convert-v1"
    {
        return convert_v1::run(files);
    }
    if let Some((command, rest)) = args.split_first()
        && command == "generators"
    {
        return generators::run(rest);
    }
    if let Some((command, rest)) = args.split_first()
        && command == "kicad-compare"
    {
        return kicad_compare::run(rest);
    }
    let command = match args.as_slice() {
        [command] => command.as_str(),
        [command, flag] if command == "licences" && flag == "--check" => "licences --check",
        _ => "help",
    };
    match command {
        "ci" | "ci-native" => {
            cargo(&["fmt", "--all", "--", "--check"])?;
            layers::check()?;
            core_modules::check()?;
            connectivity::check()?;
            f64_lengths::check()?;
            licences::check()?;
            brand_crates::check()?;
            clean_room::check()?;
            no_builtin_data::check()?;
            generic_terms::check()?;
            ui_containers::check()?;
            ui_forms::check()?;
            raw_keys::check()?;
            cargo(&[
                "clippy",
                "--locked",
                "--workspace",
                "--all-targets",
                "--profile",
                "ci",
                "--",
                "-D",
                "warnings",
            ])?;
            // The shipped binaries alone, without dev-dependencies: test
            // fixtures (`sparksmith-core/test-fixtures`) never build into them.
            cargo(&[
                "check", "--locked", "-p", "sparksmith", "-p", "sparksmith-cli", "--bins", "--profile",
                "ci",
            ])?;
            // Every test binary runs, so one CI run reports all failures.
            cargo(&[
                "test",
                "--locked",
                "--workspace",
                "--profile",
                "ci",
                "--no-fail-fast",
            ])?;
            board_parity()?;
            if command == "ci" {
                wasm()?;
                generators::run(&["check".into()])?;
            }
            Ok(())
        }
        "board-parity" => board_parity(),
        "wasm" => wasm(),
        "wasm-scripting" => wasm_tests("sparksmith-core", "scripting::tests", "scripting"),
        "wasm-footprint" => wasm_tests("sparksmith-solidcraft", "footprint::tests", "footprint"),
        "layers" => layers::check(),
        "core-modules" => core_modules::check(),
        "connectivity" => connectivity::check(),
        "f64-lengths" => f64_lengths::check(),
        "clean-room" => clean_room::check(),
        "no-builtin-data" | "no-builtin-parts" => no_builtin_data::check(),
        "generic-terms" => generic_terms::check(),
        "licences --check" => licences::check(),
        "brand-crates" => brand_crates::check(),
        "ui-containers" => ui_containers::check(),
        "ui-forms" => ui_forms::check(),
        "raw-keys" => raw_keys::check(),
        _ => Err(
            "usage: cargo xtask ci | ci-native | board-parity | wasm | wasm-scripting | wasm-footprint | layers | core-modules | connectivity | f64-lengths \
             | licences --check | brand-crates | clean-room | no-builtin-data | generic-terms | ui-containers | ui-forms | raw-keys | generators build [--update-lock] | check | reproduce | measure \
             | convert-v1 [--profile PROFILE.json] PATH… | kicad-compare [--design PATH]"
                .into(),
        ),
    }
}

/// Execute a crate's wasm proof tests in Node, separately from the
/// compile-only gate: the scripting proof (`wasm-scripting`) and the
/// footprint evaluator's golden hashes (`wasm-footprint`, native/wasm
/// agreement). Requires the wasm-bindgen test runner matching Cargo.lock
/// and Node on PATH.
fn wasm_tests(package: &str, filter: &str, what: &str) -> Result<()> {
    let runner = std::env::var_os("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER")
        .unwrap_or_else(|| "wasm-bindgen-test-runner".into());
    let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args([
            "test",
            "--locked",
            "-p",
            package,
            "--lib",
            "--profile",
            "ci",
            "--target",
            "wasm32-unknown-unknown",
            "--target-dir",
            "target/wasm-test",
            filter,
        ])
        .env("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER", runner)
        .current_dir(root()?)
        .status()?;
    if !status.success() {
        return Err(format!("wasm {what} tests failed: {status}").into());
    }
    Ok(())
}

/// The board's exact native build against the board plan it is made
/// from, on every example board (`crates/automation/tests/board_parity.rs`):
/// the same flat bodies by kind, source, layer, box and volume, and every
/// native body as recorded in its `tests/board_parity/` record; and the
/// live view's meshes of each plan (`mesh::parity`). The exact build is
/// slow, so the workspace tests leave it out and CI runs it once, here.
fn board_parity() -> Result<()> {
    cargo(&[
        "test",
        "--locked",
        "--profile",
        "ci",
        "-p",
        "sparksmith-automation",
        "--test",
        "board_parity",
        "--",
        "--include-ignored",
        "--nocapture",
    ])
}

/// The pure core and the native adapter must build for the browser.
fn wasm() -> Result<()> {
    cargo(&[
        "check",
        "--locked",
        "-p",
        "sparksmith-core",
        "-p",
        "sparksmith-solidcraft",
        "-p",
        "sparksmith-generators",
        "--lib",
        "--target",
        "wasm32-unknown-unknown",
        "--target-dir",
        "target/wasm-check",
        "--profile",
        "ci",
    ])
}
