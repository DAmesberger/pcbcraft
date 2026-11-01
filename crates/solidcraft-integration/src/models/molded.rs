//! Slice B2b (contract C0): the generated molded two-terminal model builder.
//! Owned by B2b; `models/mod.rs` dispatches here.
use super::Build;
use crate::{Error, Result};
use pcbcraft_core::library::package_form::MoldedForm;

/// The builder revision (`PackageModel::Generated { family_rev }`);
/// `None` until B2b builds the family.
pub(super) const FAMILY_REV: Option<u16> = None;

/// The builder's own construction guards. Contract stub until B2b.
pub(super) fn check(_form: &MoldedForm) -> Result<()> {
    Err(unsupported())
}

/// Build the family's bodies. Contract stub until B2b.
pub(super) fn build(_b: &mut Build<'_>, _form: &MoldedForm) -> Result<()> {
    Err(unsupported())
}

fn unsupported() -> Error {
    Error::Invalid(
        "model.unsupported: the molded two-terminal model is not built yet (slice B2b)".into(),
    )
}
