//! Dependency boundaries between workspace crates, checked from `cargo metadata`.
//!
//! Layers (a crate may depend only on strictly lower workspace layers):
//! - 0 `pcbcraft-units-derive`: the core's compile-time derive of the
//!   millimetre wire form of its nanometre records.
//! - 1 `pcbcraft-core`: document, commands, engine. Pure: no files, UI or Solidcraft.
//! - 2 `pcbcraft-io`: file effects. `pcbcraft-kicad`: KiCad board import over the
//!   command engine (it reads the bytes it is given). `pcbcraft-solidcraft`: native 3D via `solidcraft-engine` only.
//!   `pcbcraft-firmware`: firmware pin-configuration emitters over the core IR (slice C4).
//! - 3 `pcbcraft-automation`: headless session, JSON-lines and MCP transports, and
//!   the live channel into the open window over `solidcraft-control`.
//! - 4 `pcbcraft-assist`: AI-assisted part design over the automation session
//!   (the only crate with an HTTP client).
//! - 5 apps: `pcbcraft` (desktop UI) and `pcbcraft-cli`.
//!
//! `pcbcraft-fabcheck` (Gerber and Excellon files read back and compared) is
//! a leaf crates may use as a dev-dependency only; `xtask` uses it directly.
//!
//! Generator plugins sit beside the layers:
//! - `pcbcraft-generator-abi` is a leaf: wire types any crate may use; it
//!   depends on no workspace crate.
//! - `pcbcraft-generators` (layer 2) runs modules; `pcbcraft-generators-probe`
//!   (layer 3) is a test module nothing may depend on.
//! - Generator crates (`pcbcraft-gen-*`, built to wasm) depend on
//!   `pcbcraft-generator-sdk` and the ABI only, and nothing depends on them;
//!   only they may depend on the SDK.
use crate::{Result, root};
use std::process::Command;

const LAYERS: &[(&str, u8)] = &[
    ("pcbcraft-units-derive", 0),
    ("pcbcraft-core", 1),
    ("pcbcraft-io", 2),
    ("pcbcraft-kicad", 2),
    ("pcbcraft-solidcraft", 2),
    ("pcbcraft-firmware", 2),
    ("pcbcraft-generators", 2),
    ("pcbcraft-generators-probe", 3),
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

/// Leaf crates: below every layer, depending on no workspace crate.
const LEAVES: &[&str] = &["pcbcraft-generator-abi", DEV_ONLY];
/// The verification readers (Gerber, Excellon, comparison): a leaf that
/// workspace crates may use only as a dev-dependency, so nothing ships it.
const DEV_ONLY: &str = "pcbcraft-fabcheck";
/// The toolkit generator modules are written with.
const SDK: &str = "pcbcraft-generator-sdk";
/// Test modules nothing may depend on.
const TEST_MODULES: &[&str] = &["pcbcraft-generators-probe"];

fn is_generator(name: &str) -> bool {
    name.starts_with("pcbcraft-gen-")
}

/// The generator-plugin rules for `name` depending on `dep`.
fn generator_rule(name: &str, dep: &str) -> Option<String> {
    if is_generator(name) && dep != SDK && !LEAVES.contains(&dep) {
        return Some(format!(
            "{name} depends on {dep}; a generator depends on {SDK} and the ABI only"
        ));
    }
    if LEAVES.contains(&name) && is_workspace(dep) {
        return Some(format!(
            "{name} depends on {dep}; a leaf crate depends on no workspace crate"
        ));
    }
    if name == SDK && is_workspace(dep) && !LEAVES.contains(&dep) {
        return Some(format!("{SDK} depends on {dep}; it may use the ABI only"));
    }
    if dep == SDK && !is_generator(name) {
        return Some(format!(
            "{name} depends on {SDK}; only generator crates may"
        ));
    }
    if is_generator(dep) {
        return Some(format!(
            "{name} depends on generator {dep}; modules are loaded as data, never linked"
        ));
    }
    if TEST_MODULES.contains(&dep) {
        return Some(format!("{name} depends on test module {dep}"));
    }
    None
}

fn is_workspace(name: &str) -> bool {
    layer(name).is_some() || LEAVES.contains(&name) || name == SDK || is_generator(name)
}

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
        let outside_layers = LEAVES.contains(&name) || name == SDK || is_generator(name);
        let own = layer(name);
        if own.is_none() && !outside_layers {
            return Err(format!("classify new workspace package {name} in xtask layers").into());
        }
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
            if let Some(problem) = generator_rule(name, dep) {
                violations.push(problem);
            }
            if dep == DEV_ONLY && !dev {
                violations.push(format!(
                    "{name} depends on {DEV_ONLY}; use it as a dev-dependency only"
                ));
            }
            if let (Some(own), Some(other)) = (own, layer(dep))
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
