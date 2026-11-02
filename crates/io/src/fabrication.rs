//! Fabrication export: the pre-flight gate, the core's file set, the PDF
//! drawings, and an atomic write as a folder or one ZIP. The desktop
//! dialog, the CLI (`pcbcraft-cli fab`) and the automation request
//! `project.export_fabrication` all call [`export`].
use crate::{Error, Result};
use pcbcraft_core::fabrication::{
    self, FabFile, FabricationOptions, FileGroup, PreflightReport, drawing,
};
use pcbcraft_core::library::Drafts;
use pcbcraft_core::rules::RuleProfile;
use pcbcraft_core::{Project, fabrication::preflight};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// One export.
#[derive(Clone, Debug)]
pub struct ExportRequest {
    /// A new or empty folder, or with `zip` the `.zip` file to create.
    pub output: PathBuf,
    pub zip: bool,
    /// The fabrication rule profile to check against (the project's own
    /// rules when `None`).
    pub profile: Option<RuleProfile>,
    pub options: FabricationOptions,
}

/// One timed step of an export.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Timing {
    pub step: String,
    pub ms: f64,
}

/// What an export wrote.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ExportReport {
    pub preflight: PreflightReport,
    /// The folder or ZIP written.
    pub output: String,
    /// Every file, in order, with its size.
    pub files: Vec<WrittenFile>,
    pub timings: Vec<Timing>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WrittenFile {
    pub name: String,
    pub group: FileGroup,
    pub description: String,
    pub bytes: usize,
}

/// Export progress: `step` of `total` named steps.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub step: usize,
    pub total: usize,
    pub label: &'static str,
}

const STEPS: usize = 4;

/// Check, generate and write one fabrication set. Refused (nothing
/// written) while the pre-flight report has unwaived errors, when the
/// output exists and is not an empty folder, or on any generation error.
/// `progress` is called before each step; it may be a no-op.
pub fn export(
    project: &Project,
    drafts: &Drafts,
    request: &ExportRequest,
    progress: &mut dyn FnMut(Progress),
) -> Result<ExportReport> {
    let mut timings = Vec::new();
    let mut timed = |step: &str, start: Instant| {
        timings.push(Timing {
            step: step.into(),
            ms: start.elapsed().as_secs_f64() * 1000.0,
        });
    };
    check_output(&request.output, request.zip)?;
    progress(Progress {
        step: 1,
        total: STEPS,
        label: "Checking the design",
    });
    let start = Instant::now();
    let report = preflight::report(project, drafts, request.profile.as_ref())?;
    timed("preflight", start);
    if !report.ready() {
        return Err(Error::Document(pcbcraft_core::Error::Conflict(format!(
            "fabrication.blocked: {} unwaived error{} in the production check; fix or waive {} before export",
            report.blocking,
            if report.blocking == 1 { "" } else { "s" },
            if report.blocking == 1 { "it" } else { "them" },
        ))));
    }
    progress(Progress {
        step: 2,
        total: STEPS,
        label: "Writing Gerber, drill and assembly files",
    });
    let start = Instant::now();
    let mut set = fabrication::generate(project, &request.options)?;
    timed("generate", start);
    if request.options.outputs.drawings {
        progress(Progress {
            step: 3,
            total: STEPS,
            label: "Drawing the fabrication and assembly PDFs",
        });
        let start = Instant::now();
        let stem = fabrication::base_name(project, &request.options)?;
        let data = drawing::data(project, &request.options)?;
        for (suffix, description, bytes) in [
            (
                "fabrication.pdf",
                "Fabrication drawing",
                crate::fab_drawings::fabrication_pdf(&data)?,
            ),
            (
                "assembly.pdf",
                "Assembly drawing",
                crate::fab_drawings::assembly_pdf(&data)?,
            ),
        ] {
            if bytes.is_empty() {
                continue;
            }
            set.push(FabFile {
                name: format!("{stem}-{suffix}"),
                group: FileGroup::Drawing,
                description: description.into(),
                bytes,
            })?;
        }
        timed("drawings", start);
    }
    progress(Progress {
        step: STEPS,
        total: STEPS,
        label: "Saving",
    });
    let start = Instant::now();
    if request.zip {
        write_zip(&request.output, &set.files)?;
    } else {
        write_folder(&request.output, &set.files)?;
    }
    timed("write", start);
    Ok(ExportReport {
        preflight: report,
        output: request.output.display().to_string(),
        files: set
            .files
            .iter()
            .map(|f| WrittenFile {
                name: f.name.clone(),
                group: f.group,
                description: f.description.clone(),
                bytes: f.bytes.len(),
            })
            .collect(),
        timings,
    })
}

/// A ZIP must be a new `.zip` file; a folder must be new or empty.
fn check_output(output: &Path, zip: bool) -> Result<()> {
    if zip {
        if output.extension().and_then(|e| e.to_str()) != Some("zip") {
            return Err(Error::Invalid(
                "fabrication.output: a ZIP output needs a .zip file name".into(),
            ));
        }
        if output.try_exists()? {
            return Err(Error::Invalid(format!(
                "fabrication.output: {} exists; choose a new file",
                output.display()
            )));
        }
        return Ok(());
    }
    if output.try_exists()? && (!output.is_dir() || std::fs::read_dir(output)?.next().is_some()) {
        return Err(Error::Invalid(format!(
            "fabrication.output: {} exists and is not an empty folder; choose a new folder",
            output.display()
        )));
    }
    Ok(())
}

/// Write every file into a sibling temporary folder, then move it into
/// place: the folder appears complete or not at all.
fn write_folder(output: &Path, files: &[FabFile]) -> Result<()> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".pcbcraft-fab-")
        .tempdir_in(parent)?;
    for file in files {
        let path = staging.path().join(&file.name);
        let mut f = std::fs::File::create(&path)?;
        f.write_all(&file.bytes)?;
        f.sync_all()?;
    }
    // An empty folder the user chose is replaced by the finished one.
    if output.is_dir() {
        std::fs::remove_dir(output)?;
    }
    let staged = staging.keep();
    if let Err(error) = std::fs::rename(&staged, output) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error.into());
    }
    Ok(())
}

/// Write one ZIP atomically, entries in set order with a fixed timestamp,
/// so the same set gives the same archive.
fn write_zip(output: &Path, files: &[FabFile]) -> Result<()> {
    crate::persistence::atomic_write_with(output, |target| {
        let mut zip = zip::ZipWriter::new(target);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644);
        for file in files {
            zip.start_file(file.name.as_str(), options)
                .map_err(|e| Error::Invalid(format!("fabrication.zip: {e}")))?;
            zip.write_all(&file.bytes)?;
        }
        zip.finish()
            .map_err(|e| Error::Invalid(format!("fabrication.zip: {e}")))?;
        Ok(())
    })
}

/// A fabrication profile by id: a built-in profile (`std-6-6`, `fine-4-4`,
/// `hdi-3-3`, `pcbcraft-default`) or one of the user's library
/// (`UserDirs::config/rule-profiles/`).
pub fn resolve_profile(id: &str) -> Result<RuleProfile> {
    if let Some(profile) = pcbcraft_core::rules::builtin::by_id(id) {
        return Ok(profile);
    }
    let dirs = crate::settings::UserDirs::from_env()?;
    crate::rule_profiles::ProfileLibrary::from_dirs(&dirs).get(id)
}
