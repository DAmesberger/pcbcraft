//! Slice C2 (contract C0): the pin queries that need nets.
//!
//! - `project.pin_report { instance }` ([`pin_report`]): per terminal, the
//!   pads, GPIO, active function with peripheral, role and mux code, pin
//!   config, net, and every other endpoint on the net with its active
//!   function, component and Part. It reports facts and never claims
//!   compatibility; matching labels never count.
//! - `functions.suggest { instance, request }` ([`suggest`]): builds the
//!   [`SuggestInput`](crate::functions::suggest::SuggestInput) (with net
//!   hints) and runs the pure search.
use crate::diagnostics::{Error, Result};
use crate::functions::suggest::{SuggestReport, SuggestRequest};
use crate::functions::{PinConfig, Pull};
use crate::ids::{Id, ResourceId, RoleId, TerminalId};
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The active function of a terminal, as reports show it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReportFunction {
    pub label: String,
    #[serde(default)]
    pub peripheral: Option<ResourceId>,
    #[serde(default)]
    pub peripheral_label: Option<String>,
    #[serde(default)]
    pub peripheral_kind: Option<String>,
    #[serde(default)]
    pub role: Option<RoleId>,
    #[serde(default)]
    pub role_label: Option<String>,
    #[serde(default)]
    pub signal: Option<String>,
    #[serde(default)]
    pub mux: Option<String>,
}

/// Another endpoint on a terminal's net.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReportEndpoint {
    pub instance: Id,
    pub designator: String,
    pub terminal: TerminalId,
    pub terminal_name: String,
    pub component: String,
    #[serde(default)]
    pub mpn: Option<String>,
    #[serde(default)]
    pub function: Option<ReportFunction>,
}

/// One terminal of the reported instance.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReportPin {
    pub terminal: TerminalId,
    pub name: String,
    pub pads: Vec<String>,
    #[serde(default)]
    pub gpio: Option<crate::functions::GpioRef>,
    #[serde(default)]
    pub function: Option<ReportFunction>,
    #[serde(default)]
    pub config: Option<PinConfig>,
    #[serde(default)]
    pub net: Option<Id>,
    #[serde(default)]
    pub net_name: Option<String>,
    pub endpoints: Vec<ReportEndpoint>,
}

/// `project.pin_report`'s result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinReport {
    pub instance: Id,
    pub designator: String,
    pub pins: Vec<ReportPin>,
}

/// Contract stub until C2.
pub fn pin_report(_project: &Project, _instance: Id) -> Result<PinReport> {
    Err(unsupported())
}

/// Contract stub until C2.
pub fn suggest(
    _project: &Project,
    _instance: Id,
    _request: &SuggestRequest,
) -> Result<SuggestReport> {
    Err(unsupported())
}

/// The pull a report shows for a terminal: its config, else none.
pub fn pull_of(config: Option<&PinConfig>) -> Option<Pull> {
    config.and_then(|c| c.pull)
}

fn unsupported() -> Error {
    Error::Unsupported(
        "pins.unsupported: pin reports and suggestions are not available yet (slice C2)".into(),
    )
}
