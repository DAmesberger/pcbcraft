//! Slice C4: firmware pin-configuration emitters over the
//! core IR ([`PinConfigReport`], `project.pin_config`). Pure text
//! generation: depends on `sparksmith-core` only, sits beside `sparksmith-io`
//! (layer 1), and is used by automation and the apps. Pin configuration
//! only (founder decision F1): no clock trees, register init or HAL
//! projects; output is labelled a starting point. Generated text is never
//! imported back.
//!
//! User templates are Rhai scripts that build the text from the IR (see
//! [`template`]). MiniJinja (Apache-2.0) passed the licence review but not
//! the bounds review: its string repetition allocates up to 100 MB in one
//! step and `~` concatenation is unbounded, neither of which its fuel
//! counter limits. Rhai, already a dependency, caps every string, array
//! and map and counts operations.
mod c_header;
mod rust_consts;
pub mod template;
mod text;
mod zephyr;

#[cfg(test)]
mod tests;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sparksmith_core::firmware::{PIN_CONFIG_FORMAT, PinConfigReport};
use sparksmith_core::ids::ContentHash;
use sparksmith_core::library::bundle::asset_hash;
use sparksmith_core::{Error, Result};

/// Bytes one emitted file may have.
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
/// Bytes of a user template.
pub const MAX_TEMPLATE_BYTES: usize = 256 * 1024;

/// Which emitter writes the text.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "emitter", rename_all = "snake_case", deny_unknown_fields)]
pub enum Emitter {
    /// The IR itself, pretty JSON.
    Json,
    /// `#define <NAME>_PORT/_PIN/_AF` plus a pin table array.
    CHeader,
    /// A devicetree overlay skeleton ("starting point; check node names
    /// against your board's HAL").
    ZephyrOverlay,
    /// `pub const` items.
    RustConsts,
    /// A user text template: a Rhai script over the IR ([`template`]),
    /// named by the SHA-256 of its text. The caller reads the text (the
    /// `firmware.export` command reads it from its `template_file` path)
    /// and [`emit`] checks it against this hash.
    User { template: ContentHash },
}

/// One emitted file.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Emitted {
    /// Suggested file name ("pins.h", "board.overlay").
    pub file_name: String,
    pub text: String,
}

/// Emit `report` with `emitter`. `template` is the user template's text
/// for [`Emitter::User`]; its SHA-256 must equal the emitter's hash, so the
/// hash names exactly the text that ran.
///
/// The code emitters (C header, Zephyr overlay, Rust consts) refuse a
/// report with name conflicts: they never rename a pin, and duplicate
/// identifiers would not compile. JSON and user templates carry the
/// conflicts as data.
pub fn emit(
    report: &PinConfigReport,
    emitter: &Emitter,
    template: Option<&str>,
) -> Result<Emitted> {
    if report.format != PIN_CONFIG_FORMAT {
        return Err(Error::Unsupported(format!(
            "pin configuration format {} is not supported; this build reads format {PIN_CONFIG_FORMAT}",
            report.format
        )));
    }
    let code = matches!(
        emitter,
        Emitter::CHeader | Emitter::ZephyrOverlay | Emitter::RustConsts
    );
    if code && !report.name_conflicts.is_empty() {
        let names: Vec<String> = report
            .name_conflicts
            .iter()
            .map(|c| {
                let terminals: Vec<String> = c.terminals.iter().map(|t| t.to_string()).collect();
                format!("{} (terminals {})", c.name, terminals.join(", "))
            })
            .collect();
        return Err(Error::Conflict(format!(
            "firmware names collide: {}; set a distinct pin-config name on each pin",
            names.join("; ")
        )));
    }
    let emitted = match emitter {
        Emitter::Json => Emitted {
            file_name: "pins.json".into(),
            text: format!("{}\n", serde_json::to_string_pretty(report)?),
        },
        Emitter::CHeader => Emitted {
            file_name: "pins.h".into(),
            text: c_header::emit(report),
        },
        Emitter::ZephyrOverlay => Emitted {
            file_name: "pins.overlay".into(),
            text: zephyr::emit(report),
        },
        Emitter::RustConsts => Emitted {
            file_name: "pins.rs".into(),
            text: rust_consts::emit(report)?,
        },
        Emitter::User { template: hash } => {
            let source = template.ok_or_else(|| {
                Error::Invalid(format!("the user template {hash} was not supplied"))
            })?;
            if source.len() > MAX_TEMPLATE_BYTES {
                return Err(Error::Limit(format!(
                    "the user template exceeds {} KiB",
                    MAX_TEMPLATE_BYTES >> 10
                )));
            }
            let actual = asset_hash(source.as_bytes());
            if actual != *hash {
                return Err(Error::Invalid(format!(
                    "the supplied template has hash {actual}, not {hash}"
                )));
            }
            template::emit(report, source)?
        }
    };
    if emitted.text.len() > MAX_OUTPUT_BYTES {
        return Err(Error::Limit(format!(
            "the emitted text exceeds {} MiB",
            MAX_OUTPUT_BYTES >> 20
        )));
    }
    Ok(emitted)
}

/// The SHA-256 a user template is named by.
pub fn template_hash(source: &str) -> ContentHash {
    asset_hash(source.as_bytes())
}
