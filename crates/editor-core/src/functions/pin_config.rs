//! Slice C2 (contract C0): per-terminal data a pin carries beyond its
//! electrical functions (mux codes, GPIO identity, settable non-electrical
//! options) and the per-instance pin configuration that selects among
//! those options.
//!
//! Nothing here changes electrical behaviour: anything that does stays a
//! [`Function`](super::Function), so ERC never reads a [`PinConfig`].
//! Every value is explicit data; nothing is parsed out of names such as
//! `PA5` or `USART1_TX`.
use crate::diagnostics::{Error, Result};
use crate::ids::TerminalId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bytes of a [`MuxCode`].
pub const MAX_MUX_CODE: usize = 16;
/// Bytes of a GPIO port name.
pub const MAX_GPIO_PORT: usize = 8;
/// Options one terminal declares.
pub const MAX_PIN_OPTIONS: usize = 8;
/// Allowed values of one enumerated option (speeds).
pub const MAX_OPTION_VALUES: usize = 16;
/// Bytes of a firmware name override.
pub const MAX_FIRMWARE_NAME: usize = 64;
/// Terminals one `instance.set_pin_config` edits.
pub const MAX_PIN_CONFIG_EDITS: usize = 256;

/// The vendor's alternate-function selector for one function, as opaque
/// data (`AF7`, `5`, `ALT2`): at most [`MAX_MUX_CODE`] bytes. Never
/// interpreted by the core.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct MuxCode(pub String);

/// Where an imported row came from in its reference document.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    /// Page of the reference document, 1-based; `None` when unknown.
    #[serde(default)]
    pub page: Option<u16>,
    /// Table or section label as printed ("Table 9").
    #[serde(default)]
    pub table: Option<String>,
}

/// A terminal's GPIO identity, stated explicitly: never parsed from a
/// terminal name.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GpioRef {
    /// Port name, 1–[`MAX_GPIO_PORT`] bytes ("A", "P0", "GPIO1").
    pub port: String,
    pub index: u16,
}

/// Internal pull resistor setting.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Pull {
    None,
    Up,
    Down,
}

/// Initial output level.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Low,
    High,
}

/// A settable non-electrical option of a terminal and its allowed values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "option", content = "allowed", rename_all = "snake_case")]
pub enum PinOption {
    Pull(Vec<Pull>),
    /// Vendor speed grades as data ("low", "high", "very_high").
    Speed(Vec<String>),
    InitialLevel,
    Schmitt,
}

/// One instance's configuration of one terminal. Validated against the
/// terminal's [`PinOption`]s by `instance.set_pin_config`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<Pull>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<Level>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schmitt: Option<bool>,
    /// Firmware identifier override; the net name is the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// An instance's pin configuration by terminal.
pub type PinConfigs = BTreeMap<TerminalId, PinConfig>;

/// One `instance.set_pin_config` edit: `None` clears the terminal's
/// configuration.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinConfigEdit {
    pub terminal: TerminalId,
    pub config: Option<PinConfig>,
}

/// Persistence bounds of a terminal's C2 data (called by
/// `TerminalFunctions::within_bounds`).
pub fn terminal_within_bounds(gpio: Option<&GpioRef>, options: &[PinOption]) -> bool {
    gpio.is_none_or(|g| g.port.len() <= MAX_GPIO_PORT)
        && options.len() <= MAX_PIN_OPTIONS
        && options.iter().all(|o| match o {
            PinOption::Pull(values) => values.len() <= MAX_OPTION_VALUES,
            PinOption::Speed(values) => {
                values.len() <= MAX_OPTION_VALUES
                    && values.iter().all(|v| v.len() <= crate::ids::MAX_LABEL)
            }
            PinOption::InitialLevel | PinOption::Schmitt => true,
        })
}

/// Persistence bounds of a function's C2/C3 data (called by
/// `TerminalFunctions::within_bounds`).
pub fn function_within_bounds(mux: Option<&MuxCode>, source: Option<&SourceSpan>) -> bool {
    mux.is_none_or(|m| m.0.len() <= MAX_MUX_CODE)
        && source.is_none_or(|s| {
            s.table
                .as_ref()
                .is_none_or(|t| t.len() <= crate::ids::MAX_LABEL)
        })
}

/// Check `config` against a terminal's declared options (C2). Contract
/// stub: refuses until C2 implements it.
pub fn check_config(_options: &[PinOption], _config: &PinConfig) -> Result<()> {
    Err(Error::Unsupported(
        "pin_config.unsupported: pin configuration is not available yet (slice C2)".into(),
    ))
}
