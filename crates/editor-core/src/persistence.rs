//! Versioned editor files. Version 1 bare projects migrate without changing IDs.
//! Authoring drafts deliberately permit unfinished bindings and geometry edits.
use crate::{Error, Part, Project, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
};

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const FILE_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EditorFile {
    pub version: u32,
    pub project: Project,
    pub authoring_draft: Option<Part>,
}

impl EditorFile {
    pub fn validate(&self) -> Result<()> {
        if self.version != FILE_VERSION {
            return Err(Error::Invalid("unsupported editor file version".into()));
        }
        crate::validate(&self.project)?;
        if let Some(draft) = &self.authoring_draft {
            validate_draft(draft)?;
        }
        Ok(())
    }
}

/// Drafts are not published components: empty names, missing mappings, duplicate
/// local IDs and zero/negative dimensions may be legitimate interrupted edits.
/// Only resource bounds and finite geometry are required for safe persistence.
pub fn validate_draft(part: &Part) -> Result<()> {
    if part.name.len() > 256
        || part.provenance.len() > 4096
        || part.component.terminals.len() > 256
        || part.symbol.pins.len() > 256
        || part.package.terminals.len() > 512
        || part.footprint.pads.len() > 1024
        || part.binding.mappings.len() > 512
        || part.component.terminals.iter().any(|t| t.name.len() > 256)
        || part.package.terminals.iter().any(|t| t.number.len() > 256)
    {
        return Err(Error::Invalid(
            "authoring draft exceeds supported text or item limits".into(),
        ));
    }
    let finite = |values: &[f64]| values.iter().all(|v| v.is_finite() && v.abs() <= 10000.0);
    if !finite(&part.symbol.body_size)
        || !finite(&part.package.body_size)
        || !finite(&part.package.model_offset)
        || !finite(&part.footprint.datum_offset)
        || part.symbol.pins.iter().any(|p| !finite(&p.position))
        || part
            .footprint
            .pads
            .iter()
            .any(|p| !finite(&p.position) || !finite(&p.size))
    {
        return Err(Error::Invalid(
            "authoring draft geometry must be finite and within ±10000 mm".into(),
        ));
    }
    Ok(())
}

fn check_path(path: &Path) -> Result<()> {
    if path.extension().is_none_or(|e| e != "pcbcraft") {
        return Err(Error::Invalid("use a .pcbcraft project filename".into()));
    }
    Ok(())
}

pub fn read(path: &Path) -> Result<EditorFile> {
    check_path(path)?;
    decode(&read_bytes(path)?)
}

pub fn decode(bytes: &[u8]) -> Result<EditorFile> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::Invalid("editor file exceeds 16 MiB".into()));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::Invalid("editor file version is missing or invalid".into()))?;
    let file = match version {
        1 => EditorFile {
            version: FILE_VERSION,
            project: serde_json::from_value(value)?,
            authoring_draft: None,
        },
        2 => serde_json::from_value(value)?,
        _ => {
            return Err(Error::Invalid(format!(
                "unsupported editor file version {version}"
            )));
        }
    };
    file.validate()?;
    Ok(file)
}

pub fn write(path: &Path, file: &EditorFile) -> Result<()> {
    check_path(path)?;
    file.validate()?;
    atomic_write(path, &encode(file)?)
}

pub(crate) fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_FILE_BYTES as u64 {
        return Err(Error::Invalid("file exceeds 16 MiB".into()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::Invalid("file exceeds 16 MiB".into()));
    }
    Ok(bytes)
}

/// Bound serialization itself, rather than allocating an arbitrarily large JSON
/// buffer and checking only after it is complete.
pub(crate) fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_FILE_BYTES.saturating_sub(self.0.len()) {
                return Err(std::io::Error::other("file exceeds 16 MiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Bounded(Vec::new());
    serde_json::to_writer_pretty(&mut buffer, value)?;
    Ok(buffer.0)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::Invalid("file exceeds 16 MiB".into()));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| Error::Io(e.error))?;
    #[cfg(unix)]
    std::fs::File::open(parent).and_then(|directory|directory.sync_all()).map_err(|error| {
        Error::Invalid(format!("The file was replaced, but synchronizing its directory failed; power-loss durability is unconfirmed: {error}"))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;
