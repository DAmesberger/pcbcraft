//! Excellon drill files. (Being written.)
use super::{FabFile, FabricationOptions};
use crate::diagnostics::Result;
use crate::model::Project;

/// The plated and non-plated drill files, the drill map and the table.
pub fn files(
    _project: &Project,
    _options: &FabricationOptions,
    _stem: &str,
) -> Result<Vec<FabFile>> {
    Ok(Vec::new())
}
