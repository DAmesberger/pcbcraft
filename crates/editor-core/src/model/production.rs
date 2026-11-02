//! Production settings saved with the design: the pre-flight findings the
//! user waived for fabrication output. Data only; `fabrication::preflight`
//! matches waivers to findings and `fabrication::apply` edits them.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Waivers on one project.
pub const MAX_WAIVERS: usize = 4096;
/// Bytes of a waiver key or reason.
pub const MAX_WAIVER_TEXT: usize = 1024;

/// One waived pre-flight finding: its stable key
/// (`fabrication::PreflightItem::key`, the finding code and the objects it
/// names) and why it is acceptable for this build.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Waiver {
    pub key: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Production {
    /// In key order, keys unique.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waivers: Vec<Waiver>,
}

impl Production {
    pub fn is_default(&self) -> bool {
        self.waivers.is_empty()
    }

    pub fn waiver(&self, key: &str) -> Option<&Waiver> {
        self.waivers.iter().find(|w| w.key == key)
    }

    /// Count, text bounds, printable text and unique sorted keys.
    pub fn within_bounds(&self) -> bool {
        self.waivers.len() <= MAX_WAIVERS
            && self.waivers.iter().all(|w| {
                !w.key.trim().is_empty()
                    && w.key.len() <= MAX_WAIVER_TEXT
                    && w.reason.len() <= MAX_WAIVER_TEXT
                    && !w.key.chars().chain(w.reason.chars()).any(char::is_control)
            })
            && self.waivers.windows(2).all(|p| p[0].key < p[1].key)
    }
}
