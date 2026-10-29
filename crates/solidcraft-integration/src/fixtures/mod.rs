//! Native foundation fixtures: a parametric probe (repeated definitions,
//! top/bottom placement, independent regeneration) and a copper sample (via,
//! zone with hole, same-net island). They prove Solidcraft capabilities the
//! product relies on and run only as tests; they have their own small
//! harnesses and are not part of Pcbcraft's command engine.
mod copper;
mod measure;
mod probe;
mod probe_model;

use crate::{Error, Result};
use serde_json::Value;
use solidcraft_engine::Session;

pub(crate) use crate::native::id;
pub(crate) use measure::Snapshot;
pub(crate) use probe_model::Parameters;

/// Raw command call for fixture construction.
pub(crate) fn call(session: &mut Session, command: &str, params: Value) -> Result<Value> {
    session
        .execute(command, &params)
        .map_err(|error| Error::Engine {
            command: command.into(),
            message: error.to_string(),
        })
}
