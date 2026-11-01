//! Slice D2 (contract C0): build a library model (`PackageModel::Library`)
//! into a native definition: the packed asset read through Solidcraft,
//! the source frame applied, every body given its mapped role and
//! material, and lead contacts derived from the tagged geometry. Bounds are
//! rechecked against `ModelDefinition::bounds_mm`; a mismatch refuses and
//! names the asset. One shared definition per package revision, like
//! generated models. The board hook lives in `project.rs` and is wired at
//! integration (that file is owned elsewhere during the fan-out).
use super::ModelDefinition as BuiltDefinition;
use crate::native::Native;
use crate::{Error, Result};
use pcbcraft_core::library::model3d::{LibraryModel, ModelDefinition};

/// Build `model` (with its asset `bytes`) as component `component` in
/// `native`. Contract stub until D2.
pub fn build_library_model(
    _native: &mut Native,
    _component: &str,
    _model: &ModelDefinition,
    _use: &LibraryModel,
    _bytes: &[u8],
) -> Result<BuiltDefinition> {
    Err(Error::Invalid(
        "model.unsupported: library models are not built yet (slice D2)".into(),
    ))
}

/// The lead contacts of the tagged geometry, model frame, in tag order.
/// Contract stub until D2.
pub fn lead_contacts(_model: &ModelDefinition, _bytes: &[u8]) -> Result<Vec<[f64; 3]>> {
    Err(Error::Invalid(
        "model.unsupported: lead contacts are not derived yet (slice D2)".into(),
    ))
}
