//! Packed-asset housekeeping shared by the library handlers: assets that
//! no published revision, draft, instance marking or generated part names
//! any more are
//! dropped by the command that stopped naming them, so they leave in the
//! same undo step and are never saved into every later container.
use crate::ids::{ContentHash, DefinitionId};
use crate::library::bundle::named_assets;
use crate::library::{Definition, Drafts};
use crate::model::Document;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The assets the drafts `ids` name now (for a later [`drop_unnamed`]). A
/// draft whose names cannot be read contributes nothing.
pub(crate) fn named_by_drafts(
    drafts: &Drafts,
    ids: impl IntoIterator<Item = DefinitionId>,
) -> BTreeSet<ContentHash> {
    ids.into_iter()
        .filter_map(|id| drafts.get(id))
        .filter_map(|d| named_assets(&d.definition).ok())
        .flatten()
        .collect()
}

/// Every asset some published revision, draft or instance marking names;
/// `None` when one cannot be read (then nothing is dropped).
fn in_use(document: &Document) -> Option<BTreeSet<ContentHash>> {
    let library = &document.project.library;
    let mut out = crate::library::generated::named_modules(library);
    for model in &library.models {
        out.extend(model.definition.assets().map(|a| a.hash));
    }
    for component in &library.components {
        out.extend(
            component
                .definition
                .marking
                .iter()
                .flat_map(|m| m.artwork()),
        );
    }
    for part in &library.parts {
        let body = Definition::Part(Box::new(part.definition.clone()));
        out.extend(named_assets(&body).ok()?);
    }
    for draft in &document.drafts.items {
        out.extend(named_assets(&draft.definition).ok()?);
    }
    for instance in &document.project.instances {
        out.extend(instance.marking.replace.iter().flat_map(|m| m.artwork()));
    }
    Some(out)
}

/// Drop the packed assets among `candidates` that nothing names any more.
pub(crate) fn drop_unnamed(document: &mut Document, candidates: BTreeSet<ContentHash>) {
    let packed: BTreeSet<ContentHash> = document
        .project
        .library
        .assets
        .iter()
        .map(|a| a.hash)
        .filter(|h| candidates.contains(h))
        .collect();
    if packed.is_empty() {
        return;
    }
    let Some(used) = in_use(document) else {
        return;
    };
    let gone: BTreeSet<ContentHash> = packed.difference(&used).copied().collect();
    if gone.is_empty() {
        return;
    }
    Arc::make_mut(&mut document.project)
        .library
        .assets
        .retain(|a| !gone.contains(&a.hash));
}
