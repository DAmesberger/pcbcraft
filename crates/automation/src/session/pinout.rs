//! Slice C3: `library.pinout_schema`, the JSON schema of the
//! typed pinout document agents fill from a datasheet. Preview and apply
//! are core operations (`project.query` `library.pinout_preview`,
//! `project.apply` `library.pinout_apply`).
use super::Session;
use crate::Result;
use serde_json::Value;

impl Session {
    /// The pinout document schema, with its format number and the pin
    /// rule script contract (helpers, verdicts, examples).
    pub(super) fn pinout_schema(&self) -> Result<Value> {
        Ok(serde_json::json!({
            "format": sparksmith_core::library::pinout::PINOUT_FORMAT,
            "schema": sparksmith_core::library::pinout::schema(),
            "rule_api": sparksmith_core::functions::rules::RULE_API,
        }))
    }
}
