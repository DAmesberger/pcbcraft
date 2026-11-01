//! Slice C4 (contract C0): the firmware pin-configuration IR (founder
//! decision F1: pin configuration only; no clock trees, register init or
//! HAL projects). `project.pin_config { instance }` produces a versioned,
//! JSON-schema'd [`PinConfigReport`]; emitters (C header, Zephyr overlay,
//! JSON, Rust consts, user templates) live in the `pcbcraft-firmware`
//! crate. Generated text is never imported back.
//!
//! Firmware names are the `PinConfig::name` or the net name, sanitised to
//! `[A-Z0-9_]`; a collision is reported in `name_conflicts`, never renamed
//! silently.
use crate::diagnostics::{Error, Result};
use crate::functions::{Level, Pull};
use crate::ids::{ComponentRef, Id, PartRef, ResourceId, TerminalId};
use crate::model::Project;
use crate::pins::ReportEndpoint;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only IR format this build writes.
pub const PIN_CONFIG_FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirmwareDevice {
    pub instance: Id,
    pub designator: String,
    pub component: ComponentRef,
    pub component_name: String,
    #[serde(default)]
    pub part: Option<PartRef>,
    #[serde(default)]
    pub mpn: Option<String>,
}

/// One enabled peripheral and its active roles.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirmwarePeripheral {
    pub peripheral: ResourceId,
    pub label: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub roles: Vec<String>,
}

/// One assigned pin.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirmwarePin {
    pub terminal: TerminalId,
    pub name: String,
    #[serde(default)]
    pub gpio: Option<crate::functions::GpioRef>,
    pub function: String,
    #[serde(default)]
    pub peripheral: Option<String>,
    #[serde(default)]
    pub peripheral_kind: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub signal: Option<String>,
    #[serde(default)]
    pub mux: Option<String>,
    #[serde(default)]
    pub pull: Option<Pull>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub initial: Option<Level>,
    /// Sanitised `[A-Z0-9_]` identifier.
    pub firmware_name: String,
    pub endpoints: Vec<ReportEndpoint>,
}

/// Two or more pins that sanitise to one firmware name.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NameConflict {
    pub name: String,
    pub terminals: Vec<TerminalId>,
}

/// The IR every emitter reads.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinConfigReport {
    /// [`PIN_CONFIG_FORMAT`].
    pub format: u32,
    pub device: FirmwareDevice,
    pub peripherals: Vec<FirmwarePeripheral>,
    pub pins: Vec<FirmwarePin>,
    pub name_conflicts: Vec<NameConflict>,
}

/// `project.pin_config`. Contract stub until C4.
pub fn pin_config(_project: &Project, _instance: Id) -> Result<PinConfigReport> {
    Err(Error::Unsupported(
        "firmware.unsupported: firmware pin-configuration export is not available yet (slice C4)"
            .into(),
    ))
}

/// JSON schema of the IR.
pub fn schema() -> serde_json::Value {
    serde_json::json!(schemars::schema_for!(PinConfigReport))
}
