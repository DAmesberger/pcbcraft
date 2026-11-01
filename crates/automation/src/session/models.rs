//! Slice D2 (contract C0): `library.inspect_model_file` reads a STEP, GLB
//! or `.solidcraft` file through io (bounded) and returns the integration
//! crate's inspection; the import itself is `project.apply`
//! `library.import_model` with the wizard's mapping. Not a document change.
use super::Session;
use crate::Result;
use pcbcraft_core::library::model3d::ModelFormat;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelFileParams {
    pub path: String,
    pub format: ModelFormat,
}

impl Session {
    /// Contract stub until D2.
    pub(super) fn inspect_model_file(&mut self, params: ModelFileParams) -> Result<Value> {
        let inspection = pcbcraft_solidcraft::models::inspect::inspect(&[], params.format)?;
        Ok(serde_json::to_value(inspection)?)
    }
}
