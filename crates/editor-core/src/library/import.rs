//! Import, export (promote) and duplicate over a project library (S3, D1,
//! D6). Pure functions over `Library`; the engine stages them on a
//! candidate document through `library.import`, `library.place_from`,
//! `library.promote` and `library.duplicate`. Identity allocation is passed
//! in so this module needs no `model`.
//!
//! **Where a source lineage lives.** Per library, project and source
//! lineages correspond one to one through `Library::origins`
//! (`bundle::validate_snapshot` enforces it). Importing a new revision of a
//! source lineage the project already holds appends the next revision of
//! the same project lineage, so the existing reviewed update path moves
//! instances to it. A source revision older than one already copied is
//! refused (`library.older_revision`), so the project's latest revision is
//! always its newest source revision. Promoting a new project revision of a lineage the
//! library already holds writes the next source revision of the same
//! source lineage.
use super::bundle::{
    self, Bundle, Origin, OriginRecord, RevisionRecord, SourceRevision, content_hash,
};
use super::{Definition, DefinitionMeta, Library, MAX_NOTE, drafts};
use crate::diagnostics::{Error, Result};
use crate::ids::{
    BindingRef, ComponentRef, ContentHash, DefinitionId, DefinitionKind, DefinitionRef, LibraryId,
    Revision, SourceLineageId, SymbolRef, printable,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Fresh source lineage ids one promote may consume.
pub const MAX_FRESH_LINEAGES: usize = 256;

/// The message prefix of the `conflict` `library.place_from` returns when
/// the project holds another revision of the bundle root's lineage (see
/// [`OtherRevision::Refuse`]). Hosts test it with [`is_other_revision`].
pub const OTHER_REVISION: &str = "library.other_revision";

/// The message prefix of the `conflict` an import returns when a source
/// revision is older than one the project already copied of the same
/// source lineage. Project revisions are numbered in import order, so the
/// older content would become the project's latest and an update would
/// move instances backwards.
pub const OLDER_REVISION: &str = "library.older_revision";

/// The message prefix of the `conflict` an import returns when the project
/// recorded another content hash for the same source revision: the folder's
/// rN changed after it was copied.
pub const SOURCE_MODIFIED: &str = "library.source_modified";

/// Whether `error` is the `library.other_revision` refusal.
pub fn is_other_revision(error: &Error) -> bool {
    matches!(error, Error::Conflict(message) if message.starts_with(OTHER_REVISION))
}

/// One record of an import and the project revision it maps to.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImportedRevision {
    pub record: OriginRecord,
    /// An equal origin was already in the project; nothing was appended.
    pub reused: bool,
}

/// What an import did, in bundle order. `root` is the project copy of the
/// bundle's root.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImportReport {
    pub root: DefinitionRef,
    pub revisions: Vec<ImportedRevision>,
}

/// Source lineage → project lineage, and source revision → project
/// revision with the hash recorded, for one library.
struct Mapping {
    lineages: BTreeMap<SourceLineageId, DefinitionId>,
    revisions: BTreeMap<SourceRevision, OriginRecord>,
}

fn mapping(library: &Library, source: LibraryId) -> Mapping {
    let mut lineages = BTreeMap::new();
    let mut revisions = BTreeMap::new();
    for record in library
        .origins
        .iter()
        .filter(|o| o.origin.library == source)
    {
        lineages.insert(record.origin.lineage, record.local.id);
        revisions.insert(
            SourceRevision {
                lineage: record.origin.lineage,
                revision: record.origin.revision,
            },
            *record,
        );
    }
    Mapping {
        lineages,
        revisions,
    }
}

/// The project lineage that holds `lineage` of library `source`, if any.
pub fn project_lineage(
    library: &Library,
    source: LibraryId,
    lineage: SourceLineageId,
) -> Option<DefinitionId> {
    library
        .origins
        .iter()
        .find(|o| o.origin.library == source && o.origin.lineage == lineage)
        .map(|o| o.local.id)
}

fn root_record(bundle: &Bundle) -> Result<&RevisionRecord> {
    bundle
        .records
        .iter()
        .find(|r| r.lineage == bundle.root.lineage && r.revision == bundle.root.revision)
        .ok_or_else(|| Error::Invalid("the bundle does not contain its root revision".into()))
}

/// Import a validated bundle: dedupe by origin, map each new source lineage
/// to a lineage from `allocate` and each source revision to the next project
/// revision of that lineage, rewrite every reference, append the revisions,
/// pack the assets and record the origins. A failure leaves `library`
/// unchanged only because the caller stages on a candidate.
///
/// A source revision the project already copied with another content hash
/// is a `conflict` (`library.source_modified`): the folder's record changed
/// and is never trusted over the project's copy.
pub fn import(
    library: &mut Library,
    bundle: &Bundle,
    allocate: &mut dyn FnMut() -> Result<DefinitionId>,
) -> Result<ImportReport> {
    bundle::validate_bundle(bundle)?;
    let mut map = mapping(library, bundle.library);
    let mut hashes = Vec::with_capacity(bundle.records.len());
    for record in &bundle.records {
        let hash = content_hash(record)?;
        let at = SourceRevision {
            lineage: record.lineage,
            revision: record.revision,
        };
        if let Some(known) = map.revisions.get(&at)
            && known.origin.content_hash != hash
        {
            return Err(Error::Conflict(format!(
                "{SOURCE_MODIFIED}: {} {} of ‘{}’ no longer matches the copy the project holds ({} {}); its file was changed after it was imported",
                record.lineage, record.revision, bundle.name, known.local.id, known.local.revision
            )));
        }
        hashes.push(hash);
    }
    // Lineages first, so a lineage reference to a record later in the
    // bundle (a symbol's owning component) resolves.
    for record in &bundle.records {
        if let std::collections::btree_map::Entry::Vacant(slot) = map.lineages.entry(record.lineage)
        {
            slot.insert(allocate()?);
        }
    }
    // Dependencies first by publication rank, whatever the bundle order.
    let mut order: Vec<usize> = (0..bundle.records.len()).collect();
    order.sort_by_key(|i| {
        let r = &bundle.records[*i];
        (super::publish_rank(r.kind), r.lineage, r.revision)
    });
    let mut done: BTreeMap<usize, ImportedRevision> = BTreeMap::new();
    let mut needed: BTreeSet<ContentHash> = BTreeSet::new();
    for i in order {
        let record = &bundle.records[i];
        let at = SourceRevision {
            lineage: record.lineage,
            revision: record.revision,
        };
        if let Some(known) = map.revisions.get(&at) {
            done.insert(
                i,
                ImportedRevision {
                    record: *known,
                    reused: true,
                },
            );
            continue;
        }
        let id =
            map.lineages.get(&record.lineage).copied().ok_or_else(|| {
                Error::Invalid(format!("lineage {} was not mapped", record.lineage))
            })?;
        if let Some(newer) = map
            .revisions
            .keys()
            .filter(|k| k.lineage == record.lineage && k.revision > record.revision)
            .map(|k| k.revision)
            .max()
        {
            return Err(Error::Conflict(format!(
                "{OLDER_REVISION}: ‘{}’ in ‘{}’ is {}, older than the {newer} the project already copied as {id}; the project numbers revisions in import order, so it cannot take an older one. Use the newer revision, or update what pins {} in the library.",
                record.meta.name, bundle.name, record.revision, record.revision
            )));
        }
        let revision = match library.latest(id) {
            None => Revision::FIRST,
            Some((kind, _)) if kind != record.kind => {
                return Err(Error::Invalid(format!(
                    "‘{}’ is a {} in the library, but the project holds it as a {}",
                    record.meta.name,
                    super::names::noun(record.kind),
                    super::names::noun(kind)
                )));
            }
            Some((_, latest)) => latest
                .next()
                .ok_or_else(|| Error::Limit(format!("{id} has no revision numbers left")))?,
        };
        let revisions = &map.revisions;
        let lineages = &map.lineages;
        let body = bundle::from_record(
            record,
            &mut |kind, source| {
                revisions
                    .get(&source)
                    .map(|o| o.local)
                    .filter(|local| local.kind == kind)
                    .ok_or_else(|| {
                        Error::Invalid(format!(
                            "{} {} names {} {} before it was imported",
                            record.lineage, record.revision, source.lineage, source.revision
                        ))
                    })
            },
            &mut |_, lineage| {
                lineages.get(&lineage).copied().ok_or_else(|| {
                    Error::Invalid(format!(
                        "{} {} names lineage {lineage}, which the bundle does not carry",
                        record.lineage, record.revision
                    ))
                })
            },
        )?;
        needed.extend(record.assets.iter().copied());
        library.append(id, revision, record.meta.clone(), body);
        let local = DefinitionRef {
            kind: record.kind,
            id,
            revision,
        };
        let origin = OriginRecord {
            local,
            origin: Origin {
                library: bundle.library,
                lineage: record.lineage,
                revision: record.revision,
                content_hash: hashes[i],
            },
            direction: bundle::Direction::Imported,
        };
        library.origins.push(origin);
        map.revisions.insert(at, origin);
        done.insert(
            i,
            ImportedRevision {
                record: origin,
                reused: false,
            },
        );
    }
    let held: BTreeSet<ContentHash> = library.assets.iter().map(|a| a.hash).collect();
    for asset in &bundle.assets {
        if needed.contains(&asset.hash) && !held.contains(&asset.hash) {
            library.assets.push(asset.clone());
        }
    }
    let root = map
        .revisions
        .get(&bundle.root)
        .map(|o| o.local)
        .ok_or_else(|| Error::Invalid("the bundle root was not imported".into()))?;
    Ok(ImportReport {
        root,
        revisions: done.into_values().collect(),
    })
}

/// What `library.place_from` places.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "from", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlaceSource {
    /// A component revision the project already holds.
    Project { component: crate::ids::ComponentRef },
    /// The root component (or Part) of a bundle, imported first (or reused
    /// when an equal origin is already in the project).
    Bundle { bundle: Box<Bundle> },
}

/// What `library.place_from` does when the project holds a different
/// revision of the bundle root's lineage. It never upgrades silently.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OtherRevision {
    /// Refuse with `conflict` code `library.other_revision`, naming both
    /// revisions, so the host can offer Keep both / Update project copy /
    /// Cancel.
    #[default]
    Refuse,
    /// Import the bundle's revision beside the existing one and place it.
    KeepBoth,
}

/// The `library.other_revision` refusal, when the project holds another
/// revision of the bundle root's lineage and not the root itself.
pub fn check_other_revision(library: &Library, bundle: &Bundle) -> Result<()> {
    let root = root_record(bundle)?;
    let map = mapping(library, bundle.library);
    // The root itself is held: an equal hash is reused, another hash is
    // the import's `library.source_modified`.
    if map.revisions.contains_key(&bundle.root) {
        return Ok(());
    }
    let Some(id) = map.lineages.get(&bundle.root.lineage) else {
        return Ok(());
    };
    // The newest source revision the project copied of this lineage.
    let held = library
        .origins
        .iter()
        .filter(|o| o.local.id == *id && o.origin.library == bundle.library)
        .map(|o| o.origin.revision)
        .max();
    let held = held.map_or_else(|| "another revision".to_owned(), |r| r.to_string());
    Err(Error::Conflict(format!(
        "{OTHER_REVISION}: This project uses {} {held} from {}; the library now has {}.",
        root.meta.name, bundle.name, bundle.root.revision
    )))
}

/// What promoting `root` to a library would write and record.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PromotePlan {
    /// Every record of the closure. Members whose origin is already in the
    /// target map back to their source lineage; io skips a record the
    /// folder holds with an equal hash and refuses one with another hash.
    pub bundle: Bundle,
    /// The origins `library.promote` records for the project revisions.
    pub origins: Vec<OriginRecord>,
    /// Fresh lineage ids the plan used, in `fresh` order.
    pub used: Vec<SourceLineageId>,
}

/// Compute the export of `root` and its closure for `target` (pure and
/// deterministic: equal inputs give equal plans). New lineages take ids
/// from `fresh` in order; too few is `invalid_input`.
///
/// A closure member with an origin in `target` maps back to that source
/// revision. Another revision of a lineage `target` already holds becomes
/// the next source revision of that source lineage (after the highest the
/// project recorded). A lineage reference that names neither a closure
/// member nor a lineage `target` holds (a symbol promoted without its
/// component) is `invalid_input`.
pub fn export(
    library: &Library,
    root: DefinitionRef,
    target: LibraryId,
    name: &str,
    fresh: &[SourceLineageId],
) -> Result<PromotePlan> {
    if fresh.len() > MAX_FRESH_LINEAGES {
        return Err(Error::Limit(format!(
            "a promote takes at most {MAX_FRESH_LINEAGES} fresh lineage ids"
        )));
    }
    if !printable(name) {
        return Err(Error::Invalid(
            "a library needs a printable name of 1–256 bytes".into(),
        ));
    }
    let map = mapping(library, target);
    let mut unique = BTreeSet::new();
    for id in fresh {
        if !unique.insert(*id) {
            return Err(Error::Invalid(format!("fresh lineage {id} is given twice")));
        }
        if map.lineages.contains_key(id) {
            return Err(Error::Invalid(format!(
                "fresh lineage {id} is already a lineage of library {target}"
            )));
        }
    }
    let members = bundle::closure(library, root)?;
    shareable(library, &members)?;
    let mut lineages: BTreeMap<DefinitionId, SourceLineageId> = map
        .lineages
        .iter()
        .map(|(source, local)| (*local, *source))
        .collect();
    let mut highest: BTreeMap<SourceLineageId, Revision> = BTreeMap::new();
    for at in map.revisions.keys() {
        let slot = highest.entry(at.lineage).or_insert(at.revision);
        *slot = (*slot).max(at.revision);
    }
    let by_local: BTreeMap<DefinitionRef, OriginRecord> =
        map.revisions.values().map(|o| (o.local, *o)).collect();
    let mut used = Vec::new();
    let mut sources: BTreeMap<DefinitionRef, SourceRevision> = BTreeMap::new();
    let mut new = BTreeSet::new();
    for member in &members {
        if let Some(known) = by_local.get(member) {
            sources.insert(
                *member,
                SourceRevision {
                    lineage: known.origin.lineage,
                    revision: known.origin.revision,
                },
            );
            continue;
        }
        let lineage = match lineages.get(&member.id) {
            Some(lineage) => *lineage,
            None => {
                let lineage = *fresh.get(used.len()).ok_or_else(|| {
                    Error::Invalid(format!(
                        "promoting needs more than {} fresh lineage ids",
                        fresh.len()
                    ))
                })?;
                used.push(lineage);
                lineages.insert(member.id, lineage);
                lineage
            }
        };
        let revision = match highest.get(&lineage) {
            None => Revision::FIRST,
            Some(r) => r
                .next()
                .ok_or_else(|| Error::Limit(format!("{lineage} has no revision numbers left")))?,
        };
        highest.insert(lineage, revision);
        sources.insert(*member, SourceRevision { lineage, revision });
        new.insert(*member);
    }
    let mut records = Vec::with_capacity(members.len());
    let mut origins = Vec::new();
    let mut assets = BTreeSet::new();
    for member in &members {
        let (meta, body) = library.definition(*member).ok_or_else(|| {
            Error::NotFound(format!(
                "{} is missing from the project",
                library.describe(*member)
            ))
        })?;
        let at = sources.get(member).copied().ok_or_else(|| {
            Error::Invalid(format!("{} was not mapped", library.describe(*member)))
        })?;
        let record = bundle::to_record(
            at.lineage,
            at.revision,
            meta,
            body,
            &mut |r| {
                sources.get(&r).copied().ok_or_else(|| {
                    Error::Invalid(format!(
                        "{} names {}, which is not in its closure",
                        library.describe(*member),
                        library.describe(r)
                    ))
                })
            },
            &mut |kind, id| {
                lineages.get(&id).copied().ok_or_else(|| {
                    Error::Invalid(format!(
                        "{} names {}, which is neither in its closure nor in the target library; promote the item that owns it",
                        library.describe(*member),
                        library.describe_lineage(kind, id)
                    ))
                })
            },
        )?;
        assets.extend(record.assets.iter().copied());
        if new.contains(member) {
            origins.push(OriginRecord {
                local: *member,
                origin: Origin {
                    library: target,
                    lineage: at.lineage,
                    revision: at.revision,
                    content_hash: content_hash(&record)?,
                },
                direction: bundle::Direction::Promoted,
            });
        }
        records.push(record);
    }
    let root_at = sources
        .get(&root)
        .copied()
        .ok_or_else(|| Error::Invalid("the root was not mapped".into()))?;
    let bundle = Bundle {
        library: target,
        name: name.to_owned(),
        root: root_at,
        records,
        assets: bundle::packed(library, &assets)?,
    };
    bundle::validate_bundle(&bundle)?;
    Ok(PromotePlan {
        bundle,
        origins,
        used,
    })
}

/// FD11: a 3D model is promoted or shared only when its licence permits
/// redistribution; `Forbidden` and `Unknown` refuse and name the model.
fn shareable(library: &Library, members: &[DefinitionRef]) -> Result<()> {
    for member in members {
        if member.kind != DefinitionKind::Model {
            continue;
        }
        let Some(model) = library.model(crate::ids::ModelRef::new(member.id, member.revision))
        else {
            continue;
        };
        let redistribution = model.definition.provenance.redistribution;
        if redistribution != super::model3d::Redistribution::Permitted {
            return Err(Error::Invalid(format!(
                "model.licence: {} may not be redistributed (redistribution {}); it stays in this project",
                library.describe(*member),
                redistribution.label()
            )));
        }
    }
    Ok(())
}

/// `note` then `line`, or `line` alone when both would not fit.
fn provenance(note: &str, line: &str) -> String {
    if note.is_empty() {
        line.to_owned()
    } else if note.len() + 1 + line.len() <= MAX_NOTE {
        format!("{note}\n{line}")
    } else {
        line.to_owned()
    }
}

/// D6: copy a component revision with its symbols and bindings into new
/// draft lineages from `allocate` (component first), references rewritten
/// to the copies; footprints, packages and fields stay shared. The copies
/// record their source in `DefinitionMeta::provenance` text only; a typed
/// duplicate-of reference is not recorded in S3 (see docs/parts-library.md,
/// D6). Returns the new lineages, component first.
pub fn duplicate(
    library: &Library,
    drafts: &mut super::Drafts,
    component: ComponentRef,
    name: &str,
    allocate: &mut dyn FnMut() -> Result<DefinitionId>,
) -> Result<Vec<DefinitionId>> {
    if !printable(name) {
        return Err(Error::Invalid(
            "a duplicate needs a printable name of 1–256 bytes".into(),
        ));
    }
    let source = library.component(component).ok_or_else(|| {
        Error::NotFound(format!(
            "component {component} is not in the project library"
        ))
    })?;
    let line = format!("Duplicated from ‘{}’ ({component}).", source.meta.name);
    let copy = allocate()?;
    let mut created: Vec<(DefinitionId, DefinitionMeta, Definition)> = Vec::new();
    let mut body = source.definition.clone();
    for r in &mut body.representations {
        let symbol = library.symbol(*r).ok_or_else(|| {
            Error::NotFound(format!(
                "{} is missing from the project library",
                super::names::capitalised(&library.describe(r.erased()))
            ))
        })?;
        let id = allocate()?;
        let mut definition = symbol.definition.clone();
        definition.component = copy;
        let meta = DefinitionMeta {
            provenance: provenance(&symbol.meta.provenance, &line),
            ..symbol.meta.clone()
        };
        created.push((id, meta, Definition::Symbol(definition)));
        *r = SymbolRef::new(id, Revision::FIRST);
    }
    for r in &mut body.implementations {
        let binding = library.binding(*r).ok_or_else(|| {
            Error::NotFound(format!(
                "{} is missing from the project library",
                super::names::capitalised(&library.describe(r.erased()))
            ))
        })?;
        let id = allocate()?;
        let mut definition = binding.definition.clone();
        definition.component = copy;
        let meta = DefinitionMeta {
            provenance: provenance(&binding.meta.provenance, &line),
            ..binding.meta.clone()
        };
        created.push((id, meta, Definition::Binding(definition)));
        *r = BindingRef::new(id, Revision::FIRST);
    }
    let meta = DefinitionMeta {
        name: name.to_owned(),
        description: source.meta.description.clone(),
        provenance: provenance(&source.meta.provenance, &line),
    };
    created.insert(0, (copy, meta, Definition::Component(body)));
    let mut lineages = Vec::with_capacity(created.len());
    for (id, meta, definition) in created {
        drafts::create(drafts, library, id, None, meta, definition)?;
        lineages.push(id);
    }
    Ok(lineages)
}
