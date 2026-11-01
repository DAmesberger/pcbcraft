//! Slice B1 (contract C0): handlers of `library.new_from_template`,
//! `library.package_from_template` and `library.save_package_row`, and the
//! `library.package_plan` preview. The dispatch arms in `library.rs` and
//! the crate root call these; B1 changes only this file and the domain
//! files it owns.
use super::Outcome;
use crate::diagnostics::Result;
use crate::library::FootprintEvaluator;
use crate::library::templates::catalog::{
    self, NewFromTemplate, PackageFromTemplate, PackagePlan, SavePackageRow,
};
use crate::model::Document;

/// `library.new_from_template`: five drafts in one undo step
/// (`Outcome::DraftsCreated`).
pub(crate) fn new_from_template(
    _document: &mut Document,
    _request: NewFromTemplate,
    _evaluator: Option<&dyn FootprintEvaluator>,
) -> Result<Outcome> {
    Err(catalog::unsupported())
}

/// `library.package_from_template` (`Outcome::PackageReplaced`).
pub(crate) fn package_from_template(
    _document: &mut Document,
    _request: PackageFromTemplate,
    _evaluator: Option<&dyn FootprintEvaluator>,
) -> Result<Outcome> {
    Err(catalog::unsupported())
}

/// `library.save_package_row` (`Outcome::DraftCreated`).
pub(crate) fn save_package_row(
    _document: &mut Document,
    _request: SavePackageRow,
) -> Result<Outcome> {
    Err(catalog::unsupported())
}

/// `library.package_plan`: what `package_from_template` would change.
pub(crate) fn package_plan(
    _document: &Document,
    _request: &PackageFromTemplate,
    _evaluator: Option<&dyn FootprintEvaluator>,
) -> Result<PackagePlan> {
    Err(catalog::unsupported())
}
