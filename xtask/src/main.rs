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
        "layers" => layers::check(),
        "ui-containers" => ui_containers::check(),
        _ => Err("usage: cargo xtask ci | ci-native | wasm | layers | ui-containers".into()),
    }
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
