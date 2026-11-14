//! Production settings saved with the design: the pre-flight findings the
//! user waived for fabrication output. Data only; `fabrication::preflight`
//! matches waivers to findings and `fabrication::apply` edits them.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Keys one `fabrication.waive` request names: a bound on the request, not
/// on how many waivers a design keeps (it keeps any number).
pub const MAX_WAIVE_KEYS: usize = 4096;
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

/// The waiver key of a finding: its code and the objects it names
/// (`fabrication::preflight::key`).
pub fn waiver_key(code: &str, objects: &[crate::diagnostics::ObjectRef]) -> String {
    let objects = serde_json::to_string(objects).unwrap_or_default();
    format!("{code} {objects}")
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

    /// The waiver of `key`, by binary search: keys are sorted on a valid
    /// document ([`Self::within_bounds`]).
    pub fn waiver(&self, key: &str) -> Option<&Waiver> {
        self.waivers
            .binary_search_by(|w| w.key.as_str().cmp(key))
            .ok()
            .and_then(|k| self.waivers.get(k))
    }

    /// Text bounds, printable text and unique sorted keys.
    pub fn within_bounds(&self) -> bool {
        self.waivers.iter().all(|w| {
            !w.key.trim().is_empty()
                && w.key.len() <= MAX_WAIVER_TEXT
                && w.reason.len() <= MAX_WAIVER_TEXT
                && !w.key.chars().chain(w.reason.chars()).any(char::is_control)
        }) && self.waivers.windows(2).all(|p| p[0].key < p[1].key)
    }
}
