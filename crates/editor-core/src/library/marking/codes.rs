//! Slice D4 (contract C0): value marking codes as pure functions over SI
//! values, with hand-derived tests and cited tables (IEC 60062 marking
//! codes, IEC 60063 E-series). `None` means the scheme cannot represent
//! the value; nothing is ever rounded into a wrong code.
//!
//! | Value | 3-digit | 4-digit | EIA-96 | Capacitor 3-digit |
//! | --- | --- | --- | --- | --- |
//! | 10 kΩ | `103` | `1002` | `01C` | — |
//! | 4.7 kΩ | `472` | `4701` | — | — |
//! | 4.99 kΩ | — | `4991` | `68B` | — |
//! | 4.7 Ω | `4R7` | `4R70` | — | — |
//! | 100 nF | — | — | — | `104` |
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Schemes one `Auto` list may try.
pub const MAX_AUTO_SCHEMES: usize = 8;

/// A cited voltage-letter table (tantalum capacitors), by name. D4 ships
/// the tables as data.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct TableRef(pub String);

/// A value marking scheme.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "scheme", rename_all = "snake_case")]
pub enum CodeScheme {
    Resistor3Digit,
    Resistor4Digit,
    Eia96,
    CapacitorPf3Digit,
    VoltageLetter {
        table: TableRef,
    },
    /// The first scheme that represents the value and fits the area at the
    /// minimum legible height.
    Auto {
        schemes: Vec<CodeScheme>,
    },
}

/// The code of `value` (SI units: ohm, farad, volt) in `scheme`, or `None`
/// when the scheme cannot represent it. Contract stub until D4.
pub fn encode(_scheme: &CodeScheme, _value: f64) -> Result<Option<String>> {
    Err(Error::Unsupported(
        "marking.unsupported: marking codes are not available yet (slice D4)".into(),
    ))
}
