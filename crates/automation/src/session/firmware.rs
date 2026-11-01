//! Slice C4 (contract C0): `firmware.export_preview` (the emitted text) and
//! `firmware.export` (written atomically through io; refuses existing
//! files). Both read `project.pin_config` and run a `pcbcraft-firmware`
//! emitter. Not document changes.
use super::Session;
use crate::Result;
use pcbcraft_core::Id;
use pcbcraft_firmware::Emitter;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirmwareParams {
    pub instance: Id,
    pub emitter: Emitter,
    /// The output file for `firmware.export`; ignored by the preview.
    #[serde(default)]
    pub path: Option<String>,
}

impl Session {
    /// Contract stub until C4.
    pub(super) fn firmware(&mut self, params: FirmwareParams, _write: bool) -> Result<Value> {
        let report = pcbcraft_core::firmware::pin_config(self.engine.project(), params.instance)?;
        let emitted = pcbcraft_firmware::emit(&report, &params.emitter, None)?;
        Ok(serde_json::to_value(emitted)?)
    }
}
