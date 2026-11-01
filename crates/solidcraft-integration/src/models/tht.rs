//! Slice B2d (contract C0): the generated through-hole model builder.
//! Owned by B2d; `models/mod.rs` dispatches here.
use super::Build;
use crate::{Error, Result};
use pcbcraft_core::library::package_form::ThtForm;

/// The builder revision (`PackageModel::Generated { family_rev }`);
/// `None` until B2d builds the family.
pub(super) const FAMILY_REV: Option<u16> = None;

/// The builder's own construction guards. Contract stub until B2d.
pub(super) fn check(_form: &ThtForm) -> Result<()> {
    Err(unsupported())
}

/// Build the family's bodies. Contract stub until B2d.
pub(super) fn build(_b: &mut Build<'_>, _form: &ThtForm) -> Result<()> {
    Err(unsupported())
}

fn unsupported() -> Error {
    Error::Invalid("model.unsupported: the through-hole model is not built yet (slice B2d)".into())
}
