//! Slice D1 (contract C0): handler of `library.import_model`: the inline,
//! bounded asset bytes are packed and a Model draft created, one undo step.
//! Assigning it to a package is `library.edit_package { SetModel }`.
use super::Outcome;
use crate::diagnostics::Result;
use crate::library::model3d::{self, ImportModel};
use crate::model::Document;

/// `library.import_model` (`Outcome::DraftCreated`).
pub(crate) fn import_model(_document: &mut Document, request: ImportModel) -> Result<Outcome> {
    model3d::import_model(&request).map(|_| Outcome::None)
}
