//! Manufacturing output, pure and deterministic: Gerber X2 artwork for every
//! copper, mask, paste, legend and profile layer, Excellon drill files with a
//! tool table and routed slots, a Gerber job file, pick-and-place and BOM
//! tables, the data the fabrication and assembly drawings print, and the
//! pre-flight report that gates export. Written from Ucamco's public Gerber
//! Layer Format and Gerber Job File specifications and the public Excellon
//! format description; no other tool's reader or writer was consulted.
//! `pcbcraft-io` writes the files (a folder or one ZIP) and the PDF drawings.
//!
//! Coordinates are board millimetres (the board origin, y up), written at
//! 6 decimals (1 nm), the precision the Gerber specification recommends.
use crate::diagnostics::{Error, Result};
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod artwork;
pub mod assembly;
pub mod drawing;
pub mod drill;
pub mod excellon;
pub mod gerber;
pub mod job;
pub mod preflight;
#[cfg(test)]
mod tests;

pub use assembly::{BottomRotation, PlacementConvention, PlacementOrigin, RotationOffset};
pub use drawing::DrawingData;
pub use drill::{DrillHole, DrillTable, DrillTool, HoleFeature, HoleFunction};
pub use preflight::{FabricationCommand, PreflightItem, PreflightReport};

/// Files in one fabrication set (a target bound on the output, well above
/// a 32-layer board's).
pub const MAX_FILES: usize = 128;
/// Bytes of one generated file (a target bound; a 10,000-via board's drill
/// file is about 0.3 MB and its outer copper a few MB).
pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
/// Bytes of the whole set.
pub const MAX_SET_BYTES: usize = 256 * 1024 * 1024;

/// What to produce and how. Every field has a default, so `{}` is a
/// complete request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FabricationOptions {
    /// The file name stem (`showcase` gives `showcase-F_Cu.gbr`, …):
    /// letters, digits, `-` and `_`, at most 64. Defaults to the project
    /// name with other characters replaced by `_`.
    #[serde(default)]
    pub base_name: Option<String>,
    /// ISO 8601 creation time written to `.CreationDate` and the job file.
    /// Left out (the default) for byte-identical output from the same
    /// design; the CLI passes the current time unless `SOURCE_DATE_EPOCH`
    /// is set.
    #[serde(default)]
    pub creation_date: Option<String>,
    /// Pick-and-place origin and rotation convention.
    #[serde(default)]
    pub placement: PlacementConvention,
    #[serde(default)]
    pub outputs: Outputs,
}

impl Default for FabricationOptions {
    fn default() -> Self {
        Self {
            base_name: None,
            creation_date: None,
            placement: PlacementConvention::default(),
            outputs: Outputs::default(),
        }
    }
}

/// Which groups of files to write (all by default).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Outputs {
    /// Gerber X2 layers and the job file.
    #[serde(default = "yes")]
    pub gerber: bool,
    /// Excellon drill files, the drill map and the drill table.
    #[serde(default = "yes")]
    pub drill: bool,
    /// Pick-and-place and BOM tables.
    #[serde(default = "yes")]
    pub assembly: bool,
    /// The fabrication and assembly drawings (PDF, written by
    /// `pcbcraft-io`; the core ignores this flag).
    #[serde(default = "yes")]
    pub drawings: bool,
}

fn yes() -> bool {
    true
}

impl Default for Outputs {
    fn default() -> Self {
        Self {
            gerber: true,
            drill: true,
            assembly: true,
            drawings: true,
        }
    }
}

/// What a generated file is, for listings and the job file.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileGroup {
    Gerber,
    Drill,
    Job,
    Assembly,
    Drawing,
}

/// One generated file: its name inside the output folder (or ZIP), its
/// group, a one-line description and its bytes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FabFile {
    pub name: String,
    pub group: FileGroup,
    pub description: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

/// One fabrication set, files in a fixed order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FabricationSet {
    pub files: Vec<FabFile>,
    pub drills: DrillTable,
}

impl FabricationSet {
    /// Add a file, refusing a repeated name or one past the bounds.
    pub fn push(&mut self, file: FabFile) -> Result<()> {
        if self.files.iter().any(|f| f.name == file.name) {
            return Err(Error::Invalid(format!(
                "fabrication.file_repeated: {} is produced twice",
                file.name
            )));
        }
        let total: usize = self.files.iter().map(|f| f.bytes.len()).sum();
        if self.files.len() >= MAX_FILES
            || file.bytes.len() > MAX_FILE_BYTES
            || total.saturating_add(file.bytes.len()) > MAX_SET_BYTES
        {
            return Err(Error::Limit(format!(
                "fabrication.too_large: {} exceeds the output bounds",
                file.name
            )));
        }
        self.files.push(file);
        Ok(())
    }
}

/// The file name stem for `project` under `options`.
pub fn base_name(project: &Project, options: &FabricationOptions) -> Result<String> {
    let valid = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    match &options.base_name {
        Some(name) if valid(name) => Ok(name.clone()),
        Some(_) => Err(Error::Invalid(
            "fabrication.base_name: use letters, digits, - and _ (at most 64)".into(),
        )),
        None => {
            let mut stem: String = project
                .name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .take(64)
                .collect();
            if stem.trim_matches('_').is_empty() {
                stem = "board".into();
            }
            Ok(stem)
        }
    }
}

/// Every text file of the set for `project`: Gerber layers and the job
/// file, drill files, drill map and table, pick-and-place and BOM, in a
/// fixed order. Deterministic: the same design and options give the same
/// bytes. The PDF drawings are `pcbcraft-io`'s ([`drawing::data`]).
///
/// Generation does not check the design; run [`preflight::report`] first.
/// It fails only on input it cannot represent (a coordinate past the
/// format, an output past the bounds).
pub fn generate(project: &Project, options: &FabricationOptions) -> Result<FabricationSet> {
    let stem = base_name(project, options)?;
    let mut set = FabricationSet {
        drills: drill::table(&drill::holes(project)),
        ..Default::default()
    };
    if options.outputs.gerber {
        for file in artwork::layers(project, options, &stem)? {
            set.push(file)?;
        }
    }
    if options.outputs.drill {
        for file in excellon::files(project, options, &stem)? {
            set.push(file)?;
        }
    }
    if options.outputs.gerber {
        let job = job::file(project, options, &stem, &set.files)?;
        set.push(job)?;
    }
    if options.outputs.assembly {
        for file in assembly::files(project, options, &stem)? {
            set.push(file)?;
        }
    }
    Ok(set)
}
