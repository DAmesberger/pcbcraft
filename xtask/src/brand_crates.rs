//! `cargo xtask brand-crates`: no SolidCraft brand-licensed crate in Sparksmith's dependency graph.
//!
//! Since upstream SolidCraft #14578 (after `a34756830`), `crates/ui-kit`, `crates/look` and
//! `crates/theme` are under SolidCraft's Brand and Interface Licence: linking them from software
//! that is not SolidCraft needs the maintainers' written permission (§3a). `solidcraft-ui-egui`
//! links all three. Sparksmith draws with its own vendored copy of the earlier MIT/Apache code
//! (`crates/ui`) instead, so none of these may be resolved for any target, platform or
//! dependency kind. The gate reads the resolved graph from `cargo metadata --locked`.

use crate::{Result, root};
use serde_json::Value;
use std::process::Command;

/// Packages Sparksmith must not link: the brand-licensed crates and the UI crate that links them.
const FORBIDDEN: &[&str] = &[
    "solidcraft-ui-kit",
    "solidcraft-look",
    "solidcraft-theme",
    "solidcraft-ui-egui",
];

/// The forbidden packages in a `cargo metadata` document's resolved package list.
fn forbidden_in(metadata: &Value) -> Result<Vec<String>> {
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or("metadata has no packages")?;
    let mut found = Vec::new();
    for package in packages {
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .ok_or("package has no name")?;
        if FORBIDDEN.contains(&name) {
            let version = package
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("?");
            found.push(format!("{name} {version}"));
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

pub fn check() -> Result<()> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--locked", "--format-version", "1"])
        .current_dir(root()?)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let found = forbidden_in(&metadata)?;
    if found.is_empty() {
        eprintln!("no SolidCraft brand-licensed crate in the dependency graph");
        Ok(())
    } else {
        Err(format!(
            "SolidCraft brand-licensed crates in the dependency graph: {}; link only SolidCraft's MIT/Apache crates and use crates/ui for the interface",
            found.join(", ")
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_each_forbidden_package_and_nothing_else() -> Result<()> {
        let metadata = json!({"packages": [
            {"name": "solidcraft-engine", "version": "0.40.0"},
            {"name": "solidcraft-theme", "version": "0.40.0"},
            {"name": "solidcraft-ui-egui", "version": "0.40.0"},
            {"name": "solidcraft-look", "version": "0.41.0"},
            {"name": "solidcraft-ui-kit", "version": "0.41.0"},
            {"name": "solidcraft-ui-registry", "version": "0.41.0"},
        ]});
        assert_eq!(
            forbidden_in(&metadata)?,
            [
                "solidcraft-look 0.41.0",
                "solidcraft-theme 0.40.0",
                "solidcraft-ui-egui 0.40.0",
                "solidcraft-ui-kit 0.41.0"
            ]
        );
        assert!(
            forbidden_in(
                &json!({"packages": [{"name": "solidcraft-viewport-gpu", "version": "1"}]})
            )?
            .is_empty()
        );
        Ok(())
    }
}
