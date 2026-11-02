//! The Gerber job file. (Being written.)
use super::{FabFile, FabricationOptions, FileGroup};
use crate::diagnostics::Result;
use crate::model::Project;

/// The `.gbrjob` describing `files`.
pub fn file(
    _project: &Project,
    _options: &FabricationOptions,
    stem: &str,
    _files: &[FabFile],
) -> Result<FabFile> {
    Ok(FabFile {
        name: format!("{stem}-job.gbrjob"),
        group: FileGroup::Job,
        description: "Gerber job file".into(),
        bytes: Vec::new(),
    })
}
