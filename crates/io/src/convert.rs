//! Converting files of an older format version to the current one
//! (docs/design/file-format-plan.md §4.7). Until the first release this
//! runs only as `sparksmith convert`; the app reads
//! [`FILE_VERSION`] alone and names that command when it refuses a file.
//!
//! A project's document goes through [`STEPS`], a chain of pure steps,
//! each taking a document of version N to N + 1 as JSON. After the last
//! step the document is read as the app reads it (typed and validated, so
//! a step that missed a field fails there and nothing is written), the
//! hashes a project remembers are recomputed from it ([`v1::identity`]:
//! they hash serialized records, so a format change alone moves them), and
//! it is written as the app writes it, after the original is copied beside
//! it as `FILE.v<N>`.
//!
//! Every incompatible change to a stored type raises [`FILE_VERSION`] and
//! adds a step here (`cargo xtask format-lock` fails until it does). Old
//! golden files for each version live in `crates/io/tests/golden/`; each
//! converts to the current golden byte for byte.
use serde_json::Value;
use sparksmith_core::format::FILE_VERSION;
use std::path::Path;

pub mod v1;
pub mod v2;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Choices a step may need from the person converting.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The current fab profile (`--profile FILE.json`) that an old
    /// project's rule snapshot becomes (step 1 to 2).
    pub profile: Option<Value>,
}

/// One step of the chain: a document of version `from` to `from + 1`.
pub struct Step {
    pub from: u32,
    /// What the step changes, for people.
    pub summary: &'static str,
    /// Rewrites the document's JSON; the number of stored values changed.
    /// The chain sets the new version.
    pub apply: fn(&mut Value, &Options) -> Result<usize>,
}

/// The chain, oldest first: one step from every version before
/// [`FILE_VERSION`].
pub const STEPS: &[Step] = &[
    Step {
        from: 1,
        summary: "lengths in integer nanometres and turns in millidegrees (schematic and library), fab profiles as files",
        apply: v1::container::step,
    },
    Step {
        from: 2,
        summary: "part-file values in integers; generated parts of the old generator ABI detached into ordinary parts",
        apply: v2::step,
    },
];

/// What [`upgrade`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Upgraded {
    /// The document's version before.
    pub from: u32,
    /// Stored values the steps rewrote.
    pub values: usize,
}

/// Runs the chain on a document, from its version to [`FILE_VERSION`].
/// A current document is left alone; one newer than this build is
/// refused.
pub fn upgrade(document: &mut Value, options: &Options) -> Result<Upgraded> {
    let from = document
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v >= 1)
        .ok_or("the document has no valid format version")?;
    if from > FILE_VERSION {
        return Err(format!(
            "the document is format version {from}, newer than this build's {FILE_VERSION}; open it with a newer {}",
            sparksmith_core::product::NAME
        )
        .into());
    }
    let mut values = 0;
    for version in from..FILE_VERSION {
        let step = STEPS
            .iter()
            .find(|s| s.from == version)
            .ok_or_else(|| format!("no converter from format version {version}"))?;
        values += (step.apply)(document, options)
            .map_err(|e| format!("converting version {version} to {}: {e}", version + 1))?;
        let next = version
            .checked_add(1)
            .ok_or("format version out of range")?;
        let object = document
            .as_object_mut()
            .ok_or("the document is not a JSON object")?;
        object.insert("version".into(), Value::from(next));
    }
    Ok(Upgraded { from, values })
}

/// Converts one file or folder in place; what was done, one line each.
/// A project container (any other path) goes through the version chain; a
/// library folder, a replay (`*.jsonl`) or an old profile library file
/// (`*.json`) takes the units and profile conversion ([`v1`]).
pub fn convert_path(path: &Path, options: &Options) -> Result<Vec<String>> {
    let result = if path.is_dir()
        || path
            .extension()
            .is_some_and(|e| e == "json" || e == "jsonl")
    {
        v1::convert_other(path)
    } else {
        v1::convert_project(path, options)
    };
    result.map_err(|e| format!("{}: {e}", path.display()).into())
}

#[cfg(test)]
mod tests;
