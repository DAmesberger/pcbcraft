mod clean_room;
mod connectivity;
mod core_modules;
mod f64_lengths;
mod layers;
mod licences;
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
            clean_room::check()?;
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
            cargo(&["test", "--locked", "--workspace", "--profile", "ci"])?;
            if command == "ci" {
                wasm()?;
            }
            Ok(())
        }
        "wasm" => wasm(),
        "wasm-scripting" => wasm_tests("pcbcraft-core", "scripting::tests", "scripting"),
        "wasm-footprint" => wasm_tests("pcbcraft-solidcraft", "footprint::tests", "footprint"),
        "layers" => layers::check(),
        "core-modules" => core_modules::check(),
        "connectivity" => connectivity::check(),
        "f64-lengths" => f64_lengths::check(),
        "clean-room" => clean_room::check(),
        "licences --check" => licences::check(),
        "ui-containers" => ui_containers::check(),
        "ui-forms" => ui_forms::check(),
        "raw-keys" => raw_keys::check(),
        _ => Err(
            "usage: cargo xtask ci | ci-native | wasm | wasm-scripting | wasm-footprint | layers | core-modules | connectivity | f64-lengths \
             | licences --check | clean-room | ui-containers | ui-forms | raw-keys"
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

/// The pure core and the native adapter must build for the browser.
fn wasm() -> Result<()> {
    cargo(&[
        "check",
        "--locked",
        "-p",
        "pcbcraft-core",
        "-p",
        "pcbcraft-solidcraft",
        "--lib",
        "--target",
        "wasm32-unknown-unknown",
        "--target-dir",
        "target/wasm-check",
        "--profile",
        "ci",
    ])
}
