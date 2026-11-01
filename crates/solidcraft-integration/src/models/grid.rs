//! Slice B2c (contract C0): the generated grid array model builder.
//! Owned by B2c; `models/mod.rs` dispatches here.
use super::Build;
use crate::{Error, Result};
use pcbcraft_core::library::package_form::GridForm;

/// The builder revision (`PackageModel::Generated { family_rev }`);
/// `None` until B2c builds the family.
pub(super) const FAMILY_REV: Option<u16> = None;

/// The builder's own construction guards. Contract stub until B2c.
pub(super) fn check(_form: &GridForm) -> Result<()> {
    Err(unsupported())
}

/// Build the family's bodies. Contract stub until B2c.
pub(super) fn build(_b: &mut Build<'_>, _form: &GridForm) -> Result<()> {
    Err(unsupported())
}

fn unsupported() -> Error {
    Error::Invalid("model.unsupported: the grid array model is not built yet (slice B2c)".into())
}
