//! Slice C3: `library.pinout_schema`, the JSON schema of the
//! typed pinout document agents fill from a datasheet. Preview and apply
//! are core operations (`project.query` `library.pinout_preview`,
//! `project.apply` `library.pinout_apply`).
use super::Session;
use crate::Result;
use serde_json::Value;

impl Session {
    /// The pinout document schema, with its format number.
    pub(super) fn pinout_schema(&self) -> Result<Value> {
        Ok(serde_json::json!({
            "format": pcbcraft_core::library::pinout::PINOUT_FORMAT,
            "schema": pcbcraft_core::library::pinout::schema(),
        }))
    }
}
