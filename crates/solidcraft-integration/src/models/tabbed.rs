//! Slice B2a (contract C0): the generated tabbed gull-wing model builder.
//! Owned by B2a; `models/mod.rs` dispatches here.
use super::Build;
use crate::{Error, Result};
use pcbcraft_core::library::package_form::TabbedForm;

/// The builder revision (`PackageModel::Generated { family_rev }`);
/// `None` until B2a builds the family.
pub(super) const FAMILY_REV: Option<u16> = None;

/// The builder's own construction guards. Contract stub until B2a.
pub(super) fn check(_form: &TabbedForm) -> Result<()> {
    Err(unsupported())
}

/// Build the family's bodies. Contract stub until B2a.
pub(super) fn build(_b: &mut Build<'_>, _form: &TabbedForm) -> Result<()> {
    Err(unsupported())
}

fn unsupported() -> Error {
    Error::Invalid(
        "model.unsupported: the tabbed gull-wing model is not built yet (slice B2a)".into(),
    )
}
