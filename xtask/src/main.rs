mod layers;
mod ui_containers;

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
        _ => "help",
    };
    match command {
        "ci" | "ci-native" => {
            cargo(&["fmt", "--all", "--", "--check"])?;
            layers::check()?;
            ui_containers::check()?;
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
        "wasm-scripting" => wasm_scripting(),
        "layers" => layers::check(),
        "ui-containers" => ui_containers::check(),
        _ => Err(
            "usage: cargo xtask ci | ci-native | wasm | wasm-scripting | layers | ui-containers"
                .into(),
        ),
    }
}

/// Execute the scripting proof in Node, separately from the compile-only gate.
/// Requires the wasm-bindgen test runner matching Cargo.lock and Node on PATH.
fn wasm_scripting() -> Result<()> {
    let runner = std::env::var_os("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER")
        .unwrap_or_else(|| "wasm-bindgen-test-runner".into());
    let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args([
            "test",
            "--locked",
            "-p",
            "pcbcraft-core",
            "--lib",
            "--profile",
            "ci",
            "--target",
            "wasm32-unknown-unknown",
            "--target-dir",
            "target/wasm-test",
            "scripting::tests",
        ])
        .env("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER", runner)
        .current_dir(root()?)
        .status()?;
    if !status.success() {
        return Err(format!("wasm scripting tests failed: {status}").into());
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
