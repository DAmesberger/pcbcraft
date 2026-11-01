//! Slice D1: handler of `library.import_model`: the inline, bounded asset
//! bytes are packed by content hash and a Model draft of a new lineage is
//! created, one undo step. Assigning it to a package is
//! `library.edit_package { SetModel }`.
use super::Outcome;
use crate::diagnostics::Result;
use crate::ids::DefinitionId;
use crate::library::drafts;
use crate::library::model3d::{self, ImportModel};
use crate::model::Document;
use std::sync::Arc;

/// `library.import_model` (`Outcome::DraftCreated`). Any refusal leaves
/// the document unchanged (the engine stages on a candidate).
pub(crate) fn import_model(document: &mut Document, request: ImportModel) -> Result<Outcome> {
    let (meta, definition) = model3d::import_model(&request)?;
    let fresh = model3d::assets_to_pack(&document.project.library, &request)?;
    let project = Arc::make_mut(&mut document.project);
    let id = DefinitionId(project.allocate()?);
    project.library.assets.extend(fresh);
    let drafts = Arc::make_mut(&mut document.drafts);
    drafts::create(drafts, &project.library, id, None, meta, definition)?;
    Ok(Outcome::DraftCreated { id })
}
