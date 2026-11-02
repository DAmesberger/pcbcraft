//! The Gerber X2 layers of a board. (Being written.)
use super::{FabFile, FabricationOptions};
use crate::diagnostics::Result;
use crate::model::Project;

/// Every Gerber layer file of `project`.
pub fn layers(
    _project: &Project,
    _options: &FabricationOptions,
    _stem: &str,
) -> Result<Vec<FabFile>> {
    Ok(Vec::new())
}
