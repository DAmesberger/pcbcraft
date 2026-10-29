use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> Result<PathBuf> {
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

fn layers() -> Result<()> {
    let result = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--locked", "--format-version", "1", "--no-deps"])
        .current_dir(root()?)
        .output()?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned().into());
    }
    let data: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    let packages = data
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("metadata has no packages")?;
    for package in packages {
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or("package has no name")?;
        let dependencies = package
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .ok_or("package has no dependencies")?;
        if !matches!(
            name,
            "pcbcraft-core" | "pcbcraft-solidcraft" | "pcbcraft-cli" | "pcbcraft" | "xtask"
        ) {
            return Err(format!("classify new workspace package {name} in xtask layers").into());
        }
        for dependency in dependencies {
            let dep = dependency
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or("dependency has no name")?;
            if matches!(name, "pcbcraft-solidcraft" | "pcbcraft-core")
                && matches!(
                    dep,
                    "egui" | "eframe" | "wgpu" | "winit" | "rfd" | "pcbcraft-cli"
                )
            {
                return Err(format!("headless adapter {name} depends on {dep}").into());
            }
            if name != "pcbcraft-solidcraft"
                && (dep.starts_with("solidcraft-") || dep.starts_with("truck-"))
                && !(name == "pcbcraft" && dep == "solidcraft-ui-egui")
            {
                return Err(format!(
                    "{name} bypasses the Solidcraft integration boundary via {dep}"
                )
                .into());
            }
            if name == "pcbcraft-solidcraft"
                && (dep.starts_with("truck-")
                    || (dep.starts_with("solidcraft-") && dep != "solidcraft-engine"))
            {
                return Err(
                    "the adapter accesses Solidcraft through solidcraft-engine only".into(),
                );
            }
        }
    }
    eprintln!("workspace dependency boundaries passed");
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
            layers()?;
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
        "layers" => layers(),
        _ => Err("usage: cargo xtask ci | ci-native | wasm | layers".into()),
    }
}

fn wasm() -> Result<()> {
    cargo(&[
        "check",
        "--locked",
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
