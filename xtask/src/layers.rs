//! Dependency boundaries between workspace crates, checked from `cargo metadata`.
//!
//! Layers (a crate may depend only on strictly lower workspace layers):
//! - 0 `pcbcraft-units-derive`: the core's compile-time derive of the
//!   millimetre wire form of its nanometre records.
//! - 1 `pcbcraft-core`: document, commands, engine. Pure: no files, UI or Solidcraft.
//! - 2 `pcbcraft-io`: file effects. `pcbcraft-solidcraft`: native 3D via `solidcraft-engine` only.
//!   `pcbcraft-firmware`: firmware pin-configuration emitters over the core IR (slice C4).
//! - 3 `pcbcraft-automation`: headless session, JSON-lines and MCP transports, and
//!   the live channel into the open window over `solidcraft-control`.
//! - 4 `pcbcraft-assist`: AI-assisted part design over the automation session
//!   (the only crate with an HTTP client).
//! - 5 apps: `pcbcraft` (desktop UI) and `pcbcraft-cli`.
use crate::{Result, root};
use std::process::Command;

const LAYERS: &[(&str, u8)] = &[
    ("pcbcraft-units-derive", 0),
    ("pcbcraft-core", 1),
    ("pcbcraft-io", 2),
    ("pcbcraft-solidcraft", 2),
    ("pcbcraft-firmware", 2),
    ("pcbcraft-automation", 3),
    ("pcbcraft-assist", 4),
    ("pcbcraft-cli", 5),
    ("pcbcraft", 5),
];

/// Window, GPU, dialog and executor crates belong to the desktop app only.
const UI_CRATES: &[&str] = &[
    "egui",
    "eframe",
    "egui-wgpu",
    "wgpu",
    "winit",
    "rfd",
    "pollster",
    "solidcraft-ui-egui",
    "solidcraft-theme",
];

/// The core stays pure so it compiles and runs unchanged on wasm.
const CORE_FORBIDDEN: &[&str] = &["tempfile"];

fn layer(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == name).map(|(_, l)| *l)
}

pub fn check() -> Result<()> {
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
    let mut violations = Vec::new();
    for package in packages {
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or("package has no name")?;
        if name == "xtask" {
            continue;
        }
        let Some(own) = layer(name) else {
            return Err(format!("classify new workspace package {name} in xtask layers").into());
        };
        let dependencies = package
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .ok_or("package has no dependencies")?;
        for dependency in dependencies {
            let dep = dependency
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or("dependency has no name")?;
            let dev = dependency.get("kind").and_then(serde_json::Value::as_str) == Some("dev");
            if let Some(other) = layer(dep)
                && other >= own
            {
                violations.push(format!(
                    "{name} (layer {own}) depends on {dep} (layer {other}); depend only on lower layers"
                ));
            }
            if UI_CRATES.contains(&dep) && name != "pcbcraft" {
                violations.push(format!(
                    "{name} depends on UI crate {dep}; only the desktop app may"
                ));
            }
            // The firmware emitters are pure text over the core IR, like
            // the core itself (slice C4).
            let pure = matches!(name, "pcbcraft-core" | "pcbcraft-firmware");
            if pure && !dev && CORE_FORBIDDEN.contains(&dep) {
                violations.push(format!(
                    "{name} depends on {dep}; file effects belong to pcbcraft-io"
                ));
            }
            let solidcraft = dep.starts_with("solidcraft-") || dep.starts_with("truck-");
            let allowed = matches!(
                (name, dep),
                (
                    "pcbcraft-solidcraft",
                    "solidcraft-engine" | "solidcraft-color" | "solidcraft-viewport"
                ) | (
                    "pcbcraft",
                    "solidcraft-ui-egui" | "solidcraft-theme" | "solidcraft-viewport-egui"
                ) | ("pcbcraft-automation", "solidcraft-control")
            );
            if solidcraft && !allowed {
                violations.push(format!(
                    "{name} depends on {dep}; Solidcraft is reached through pcbcraft-solidcraft (engine, colour, viewport shading models), pcbcraft-automation (control channel) or the app (UI kit, tokens, viewport paint callbacks) only"
                ));
            }
        }
    }
    if violations.is_empty() {
        eprintln!("workspace dependency boundaries passed");
        Ok(())
    } else {
        Err(violations.join("\n").into())
    }
}
