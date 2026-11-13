//! Slice C4: `firmware FILE.sparksmith DESIGNATOR EMITTER NEW_FILE [TEMPLATE.rhai]`:
//! export one instance's firmware pin configuration through the
//! `firmware.export` request. Owned by C4; `main.rs` dispatches here and
//! lists [`USAGE`].
use super::{Result, execute, open, print};
use serde_json::json;
use sparksmith_core::library::bundle::asset_hash;
use sparksmith_core::product::SHORT_NAME as CLI;
use std::io::Read;

/// `sparksmith_firmware::MAX_TEMPLATE_BYTES`; the session checks it again.
const MAX_TEMPLATE_BYTES: usize = 256 * 1024;

/// The usage line(s).
pub const USAGE: &str = "firmware FILE.sparksmith DESIGNATOR EMITTER NEW_FILE [TEMPLATE.rhai]
                                   export an instance's pin configuration; EMITTER is
                                   json, c-header, zephyr-overlay, rust-consts or user
                                   (with TEMPLATE); a starting point, no clocks or registers";

/// The wire name of a CLI emitter name, for every emitter but `user`.
fn builtin(name: &str) -> Option<&'static str> {
    match name {
        "json" => Some("json"),
        "c-header" => Some("c_header"),
        "zephyr-overlay" => Some("zephyr_overlay"),
        "rust-consts" => Some("rust_consts"),
        _ => None,
    }
}

/// The hash that names the template file's text.
fn template_emitter(path: &str) -> Result<serde_json::Value> {
    let file = std::fs::File::open(path)?;
    let mut text = String::new();
    file.take(MAX_TEMPLATE_BYTES as u64 + 1)
        .read_to_string(&mut text)?;
    if text.len() > MAX_TEMPLATE_BYTES {
        return Err(format!("the template exceeds {} KiB", MAX_TEMPLATE_BYTES >> 10).into());
    }
    Ok(json!({"emitter": "user", "template": asset_hash(text.as_bytes())}))
}

/// Run the subcommand with the arguments after its name.
pub fn run(args: &[String]) -> Result<()> {
    let (project, designator, emitter, output, template) = match args {
        [p, d, e, o] => (p, d, e, o, None),
        [p, d, e, o, t] => (p, d, e, o, Some(t)),
        _ => return Err(format!("usage: {CLI} {USAGE}").into()),
    };
    let emitter = match (emitter.as_str(), template) {
        ("user", Some(t)) => template_emitter(t)?,
        ("user", None) => return Err("the user emitter needs a TEMPLATE.rhai file".into()),
        (name, None) => json!({"emitter": builtin(name).ok_or_else(|| format!(
            "unknown emitter {name}; use json, c-header, zephyr-overlay, rust-consts or user"
        ))?}),
        (_, Some(_)) => return Err("a TEMPLATE applies only to the user emitter".into()),
    };
    let mut session = open(Some(project))?;
    let instance = session
        .engine()
        .project()
        .instance_by_designator(designator)
        .map(|i| i.id)
        .ok_or_else(|| format!("no instance {designator} in {project}"))?;
    print(&execute(
        &mut session,
        json!({"command": "firmware.export", "params": {
            "instance": instance,
            "emitter": emitter,
            "path": output,
            "template_file": template,
        }}),
    )?)
}
