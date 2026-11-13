//! Slice C4: `firmware.export_preview` (the emitted text) and
//! `firmware.export` (written atomically through io; refuses existing
//! files). Both read `project.pin_config` and run a `sparksmith-firmware`
//! emitter. Not document changes: the document and its revision never
//! change, and generated text is never imported back.
use super::Session;
use crate::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sparksmith_core::Id;
use sparksmith_core::firmware::PinConfigReport;
use sparksmith_firmware::{Emitter, MAX_TEMPLATE_BYTES};
use std::path::Path;

/// Bytes of an output or template path.
const MAX_PATH: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirmwareParams {
    pub instance: Id,
    pub emitter: Emitter,
    /// The output file for `firmware.export`; ignored by the preview. It
    /// must not exist yet.
    #[serde(default)]
    pub path: Option<String>,
    /// C4: the user template's file (a Rhai script, at most 256 KiB) for
    /// the `user` emitter. Its SHA-256 must equal the emitter's
    /// `template` hash.
    #[serde(default)]
    pub template_file: Option<String>,
}

impl Session {
    pub(super) fn firmware(&mut self, params: FirmwareParams, write: bool) -> Result<Value> {
        let report = sparksmith_core::firmware::pin_config(self.engine.project(), params.instance)?;
        export(&report, &params, write)
    }
}

fn invalid(message: &str) -> crate::Error {
    sparksmith_io::Error::Invalid(message.into()).into()
}

fn checked_path(path: &str) -> Result<&Path> {
    if path.is_empty() || path.len() > MAX_PATH {
        return Err(invalid("the path must be 1–4096 bytes"));
    }
    Ok(Path::new(path))
}

/// Read the user template named by `params`, when its emitter needs one.
fn template(params: &FirmwareParams) -> Result<Option<String>> {
    match (&params.emitter, &params.template_file) {
        (Emitter::User { .. }, Some(file)) => {
            let bytes = sparksmith_io::persistence::read_bytes(checked_path(file)?)?;
            if bytes.len() > MAX_TEMPLATE_BYTES {
                return Err(sparksmith_io::Error::TooLarge(format!(
                    "the template exceeds {} KiB",
                    MAX_TEMPLATE_BYTES >> 10
                ))
                .into());
            }
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| invalid("the template is not UTF-8 text"))
        }
        (Emitter::User { .. }, None) => Err(invalid(
            "the user emitter needs template_file, the template's path",
        )),
        (_, Some(_)) => Err(invalid("template_file applies only to the user emitter")),
        (_, None) => Ok(None),
    }
}

/// Emit `report` as `params` asks; with `write`, create `params.path`.
pub(super) fn export(
    report: &PinConfigReport,
    params: &FirmwareParams,
    write: bool,
) -> Result<Value> {
    let source = template(params)?;
    let emitted = sparksmith_firmware::emit(report, &params.emitter, source.as_deref())?;
    if !write {
        return Ok(serde_json::to_value(emitted)?);
    }
    let path = checked_path(
        params
            .path
            .as_deref()
            .ok_or_else(|| invalid("firmware.export needs path, a new file"))?,
    )?;
    // Created without replacing: a file that appears at `path` before the
    // write lands is refused, never overwritten.
    match sparksmith_io::persistence::atomic_create(path, emitted.text.as_bytes()) {
        Err(sparksmith_io::Error::Io(error))
            if error.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            return Err(invalid("output exists; choose a new file"));
        }
        other => other?,
    }
    Ok(json!({
        "path": path.display().to_string(),
        "file_name": emitted.file_name,
        "bytes": emitted.text.len(),
    }))
}
