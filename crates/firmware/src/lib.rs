//! Slice C4 (contract C0): firmware pin-configuration emitters over the
//! core IR ([`PinConfigReport`], `project.pin_config`). Pure text
//! generation: depends on `pcbcraft-core` only, sits beside `pcbcraft-io`
//! (layer 1), and is used by automation and the apps. Pin configuration
//! only (founder decision F1): no clock trees, register init or HAL
//! projects; output is labelled a starting point. Generated text is never
//! imported back.
use pcbcraft_core::firmware::PinConfigReport;
use pcbcraft_core::ids::ContentHash;
use pcbcraft_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    /// A user text template stored as a project or library-folder asset by
    /// hash (MiniJinja after a licence review, or Rhai string building).
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

/// Emit `report` with `emitter`. Contract stub until C4. `template` is the
/// user template's text for [`Emitter::User`].
pub fn emit(
    _report: &PinConfigReport,
    _emitter: &Emitter,
    _template: Option<&str>,
) -> Result<Emitted> {
    Err(Error::Unsupported(
        "firmware.unsupported: firmware emitters are not available yet (slice C4)".into(),
    ))
}
