//! Publication: the per-kind rules, aggregated once, and the atomic publish
//! of a draft set in dependency order.
use super::bundle::{RefSite, references};
use super::drafts::{Draft, Drafts};
use super::{Definition, Library, check_bounds, printable, publish_rank};
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::fields;
use crate::ids::{DefinitionId, DefinitionRef, Revision};
use std::collections::BTreeSet;

/// Drafts one `library.publish` may publish together.
pub const MAX_PUBLISH_SET: usize = 32;

/// A finding that blocks publication.
pub(super) fn issue(code: &str, message: String, objects: Vec<ObjectRef>) -> Finding {
    Finding::blocking(code, message, objects, Stage::Publish)
}

/// A draft for a message: its kind and name, such as "symbol ‘Resistor’".
fn draft_name(draft: &Draft) -> String {
    let kind = super::names::noun(draft.definition.kind());
    if draft.meta.name.trim().is_empty() {
        format!("of a {kind}")
    } else {
        format!("{kind} ‘{}’", draft.meta.name)
    }
}

pub(super) fn object(definition: DefinitionId, local: u32) -> ObjectRef {
    ObjectRef::LibraryObject { definition, local }
}

/// Local IDs and the counter of a body, for kinds that allocate them.
fn local_scope(body: &Definition) -> Option<(Vec<u32>, u32)> {
    match body {
        Definition::Component(d) => Some((d.local_ids(), d.next_local)),
        Definition::Symbol(d) => Some((d.local_ids(), d.next_local)),
        Definition::Package(d) => Some((d.local_ids(), d.next_local)),
        Definition::Footprint(d) => Some((d.local_ids(), d.next_local)),
        Definition::Field(d) => Some((d.local_ids(), d.next_local)),
        Definition::Part(d) => Some((d.local_ids(), d.next_local)),
        Definition::Binding(_)
        | Definition::QuantityKind(_)
        | Definition::Model(_)
        | Definition::PackageRow(_) => None,
    }
}

/// Local IDs are unique and nonzero, below the revision's own counter, and
/// never a retired ID of the base revision: a deleted ID is not reused with a
/// possibly different meaning.
fn local_id_findings(
    id: DefinitionId,
    body: &Definition,
    base: Option<&Definition>,
    out: &mut Vec<Finding>,
) {
    let Some((ids, next_local)) = local_scope(body) else {
        return;
    };
    let this = ObjectRef::Definition { id };
    let mut seen = BTreeSet::new();
    for local in &ids {
        if *local == 0 || !seen.insert(*local) {
            out.push(issue(
                "definition.local_id_invalid",
                "An object identity is zero or used more than once.".into(),
                vec![object(id, *local)],
            ));
        }
        if *local >= next_local {
            out.push(issue(
                "definition.local_id_unallocated",
                "An object identity was never allocated by this revision.".into(),
                vec![object(id, *local)],
            ));
        }
    }
    let Some((base_ids, base_next)) = base.and_then(local_scope) else {
        return;
    };
    if next_local < base_next {
        out.push(issue(
            "definition.next_local_decreased",
            "The identity counter is below the base revision's.".into(),
            vec![this],
        ));
    }
    let base_ids: BTreeSet<u32> = base_ids.into_iter().collect();
    for local in seen.difference(&base_ids) {
        if *local < base_next {
            out.push(issue(
                "definition.local_id_reused",
                "An object reuses an identity retired before this revision.".into(),
                vec![object(id, *local)],
            ));
        }
    }
}

/// Every finding for one published revision. Blocking findings prevent
/// publication and make a stored snapshot invalid; advisory ones do neither.
pub fn publish_findings(library: &Library, r: DefinitionRef) -> Vec<Finding> {
    let this = ObjectRef::Definition { id: r.id };
    let Some((meta, body)) = library.definition(r) else {
        return vec![issue(
            "reference.missing",
            format!(
                "{} is missing from the project library.",
                super::names::capitalised(&library.describe(r))
            ),
            vec![this],
        )];
    };
    let mut out = Vec::new();
    if !printable(&meta.name) {
        out.push(issue(
            "definition.name",
            "A published definition needs a printable name of 1–256 bytes.".into(),
            vec![this],
        ));
    }
    let base = r
        .revision
        .0
        .checked_sub(1)
        .filter(|b| *b > 0)
        .and_then(|b| {
            library.definition(DefinitionRef {
                revision: Revision(b),
                ..r
            })
        });
    local_id_findings(r.id, &body, base.as_ref().map(|(_, b)| b), &mut out);
    match &body {
        Definition::Component(d) => d.findings(r.id, library, &mut out),
        Definition::Symbol(d) => d.findings(r.id, library, &mut out),
        Definition::Package(d) => d.findings(r.id, &mut out),
        Definition::Footprint(d) => d.findings(r.id, library, &mut out),
        Definition::Binding(d) => d.findings(r.id, library, &mut out),
        Definition::QuantityKind(d) => out.extend(fields::kind_findings(r.id, d)),
        Definition::Part(d) => d.findings(r.id, library, &mut out),
        Definition::Model(d) => d.findings(r.id, &mut out),
        Definition::PackageRow(d) => d.findings(r.id, &mut out),
        Definition::Field(d) => {
            let kind = d.value_type.kind().map(|k| (k, library.quantity_kind(k)));
            if let Some((k, None)) = kind {
                out.push(issue(
                    "reference.missing",
                    format!(
                        "{} is missing from the project library.",
                        super::names::capitalised(&library.describe(k.erased()))
                    ),
                    vec![this, ObjectRef::Definition { id: k.id }],
                ));
            }
            let kind = kind.and_then(|(_, p)| p).map(|p| &p.definition);
            out.extend(fields::definition_findings(r.id, d, kind));
        }
    }
    out
}

/// The exact revision publishing `draft` creates: the first revision of a
/// new lineage, or the next revision of a base that is still the latest. This
/// is the one staleness rule; `library.publish` and `component.check` use it.
pub fn pending_revision(library: &Library, draft: &Draft) -> Result<DefinitionRef> {
    let id = draft.id;
    let kind = draft.definition.kind();
    let revision = match (library.latest(id), draft.base) {
        (None, None) => Revision::FIRST,
        (Some((stored, latest)), Some(base)) if stored == kind && latest == base => {
            base.next().ok_or_else(|| {
                Error::Limit(format!(
                    "The revisions of {} are exhausted.",
                    draft_name(draft)
                ))
            })?
        }
        (Some((stored, _)), _) if stored != kind => {
            return Err(Error::Conflict(format!(
                "The draft {} is a {} but its lineage is published as a {}.",
                draft_name(draft),
                super::names::noun(kind),
                super::names::noun(stored)
            )));
        }
        (Some((_, latest)), _) => {
            return Err(Error::Conflict(format!(
                "{} has a newer revision ({latest}); load it before publishing.",
                super::names::capitalised(&library.describe_lineage(kind, id))
            )));
        }
        (None, Some(_)) => {
            return Err(Error::Invalid(format!(
                "The draft {} names a base revision that does not exist.",
                draft_name(draft)
            )));
        }
    };
    Ok(DefinitionRef { kind, id, revision })
}

/// Whether a draft can be staged as its pending revision: within bounds and
/// with a current base. A draft that cannot carries this one blocking finding
/// instead of its body's findings.
pub fn stageable(library: &Library, draft: &Draft) -> std::result::Result<DefinitionRef, Finding> {
    let this = vec![ObjectRef::Definition { id: draft.id }];
    check_bounds(&draft.meta, &draft.definition)
        .map_err(|e| issue("definition.bounds", e.to_string(), this.clone()))?;
    pending_revision(library, draft).map_err(|e| {
        let code = match e {
            Error::Conflict(_) => "definition.stale_base",
            Error::Limit(_) => "definition.revisions_exhausted",
            _ => "definition.base_invalid",
        };
        issue(code, e.to_string(), this)
    })
}

/// The library with the named drafts appended as their pending revisions, in
/// dependency order, and those revisions. No finding is checked; callers
/// examine the staged revisions with [`publish_findings`].
pub fn candidate(
    library: &Library,
    drafts: &Drafts,
    ids: &[DefinitionId],
) -> Result<(Library, Vec<DefinitionRef>)> {
    if ids.len() > super::drafts::MAX_DRAFTS {
        return Err(Error::Invalid("too many drafts to stage together".into()));
    }
    let mut unique = BTreeSet::new();
    let mut set = Vec::with_capacity(ids.len());
    for id in ids {
        if !unique.insert(*id) {
            let name = drafts.get(*id).map(draft_name).unwrap_or_default();
            return Err(Error::Invalid(format!("The draft {name} is listed twice.")));
        }
        let draft = drafts.get(*id).ok_or_else(|| {
            Error::NotFound("One of the drafts to publish no longer exists.".into())
        })?;
        check_bounds(&draft.meta, &draft.definition)?;
        set.push((pending_revision(library, draft)?, draft));
    }
    set.sort_by_key(|(r, _)| (publish_rank(r.kind), r.id));
    let mut staged = library.clone();
    for (r, draft) in &set {
        if staged.count(r.kind) >= super::MAX_REVISIONS_PER_KIND {
            return Err(Error::Limit(format!(
                "The project library holds at most 4096 {} revisions.",
                super::names::noun(r.kind)
            )));
        }
        staged.append(
            r.id,
            r.revision,
            draft.meta.clone(),
            draft.definition.clone(),
        );
    }
    Ok((staged, set.into_iter().map(|(r, _)| r).collect()))
}

/// The drafts one `library.publish` of `id` must also list: every draft it
/// reaches through a reference the snapshot cannot satisfy on its own (an
/// exact reference to a draft's pending revision that is not stored yet, or
/// a lineage reference to a draft whose lineage has no published revision).
/// Excludes `id`, follows drafts transitively and lists dependencies first.
/// A reference to a stored revision needs nothing, even when that lineage
/// also has a draft: exact pins never move.
pub fn publish_set(library: &Library, drafts: &Drafts, id: DefinitionId) -> Vec<DefinitionRef> {
    let mut seen = BTreeSet::from([id]);
    let mut stack = vec![id];
    let mut out = Vec::new();
    while let Some(current) = stack.pop() {
        let Some(draft) = drafts.get(current) else {
            continue;
        };
        for site in references(&draft.definition).unwrap_or_default() {
            let (kind, target, exact) = match site {
                RefSite::Exact { target } if library.definition(target).is_none() => {
                    (target.kind, target.id, Some(target.revision))
                }
                RefSite::Lineage { kind, id } if library.latest(id).is_none() => (kind, id, None),
                _ => continue,
            };
            let Some(needed) = drafts.get(target) else {
                continue;
            };
            let Some(pending) = needed.pending() else {
                continue;
            };
            if needed.definition.kind() != kind
                || exact.is_some_and(|r| r != pending)
                || !seen.insert(target)
            {
                continue;
            }
            stack.push(target);
            out.push(DefinitionRef {
                kind,
                id: target,
                revision: pending,
            });
        }
    }
    out.sort_by_key(|r| (publish_rank(r.kind), r.id));
    out
}

/// A sentence naming the drafts `ids` need but do not list, or nothing.
fn missing_from_set(library: &Library, drafts: &Drafts, ids: &[DefinitionId]) -> Option<String> {
    let mut missing: Vec<DefinitionRef> = ids
        .iter()
        .flat_map(|id| publish_set(library, drafts, *id))
        .filter(|r| !ids.contains(&r.id))
        .collect();
    missing.sort_by_key(|r| (publish_rank(r.kind), r.id));
    missing.dedup();
    if missing.is_empty() {
        return None;
    }
    let names: Vec<String> = missing
        .iter()
        .filter_map(|r| drafts.get(r.id))
        .map(|d| format!("the draft {} ({})", draft_name(d), d.id))
        .collect();
    Some(format!(
        "This set needs drafts it does not list; publish them together: {}.",
        names.join(", ")
    ))
}

/// Publish a set of drafts as one transaction, in dependency order. References
/// may name stored revisions or other drafts' pending revisions in the set.
/// Any blocking finding, stale base or unsatisfied reference publishes
/// nothing, and the refusal names any draft the set needs but does not list.
/// Each published draft then becomes a draft of its new revision;
/// `library.publish` closes it unless asked to keep it open.
pub fn publish(
    library: &mut Library,
    drafts: &mut Drafts,
    ids: &[DefinitionId],
) -> Result<Vec<DefinitionRef>> {
    if ids.is_empty() || ids.len() > MAX_PUBLISH_SET {
        return Err(Error::Invalid(
            "publish between 1 and 32 drafts at a time".into(),
        ));
    }
    let (staged, set) = candidate(library, drafts, ids)?;
    let mut blocking = Vec::new();
    for r in &set {
        for finding in publish_findings(&staged, *r) {
            if finding.blocks_stage(Stage::Publish) {
                blocking.push(format!(
                    "{}: {}",
                    super::names::capitalised(&staged.describe(*r)),
                    finding.message
                ));
            }
        }
    }
    if !blocking.is_empty() {
        let shown = blocking.len().min(16);
        let count = if blocking.len() == 1 {
            "1 issue blocks".to_owned()
        } else {
            format!("{} issues block", blocking.len())
        };
        let mut message = format!(
            "Not published: {count} publishing. {}",
            blocking[..shown].join(" ")
        );
        if blocking.len() > shown {
            message.push_str(&format!(" ({} more findings)", blocking.len() - shown));
        }
        if let Some(hint) = missing_from_set(library, drafts, ids) {
            message.push(' ');
            message.push_str(&hint);
        }
        return Err(Error::Conflict(message));
    }
    // Import review state (F8) is draft bookkeeping: it gates publishing
    // (an uncertain row blocks above) but never becomes revision content,
    // so a revision's identity does not depend on it. The published drafts
    // drop it too, staying equal to their new revisions.
    let reviewed = set.iter().any(|r| {
        drafts.get(r.id).is_some_and(
            |d| matches!(&d.definition, Definition::Component(c) if c.review.is_some()),
        )
    });
    let (staged, set) = if reviewed {
        let mut cleared = drafts.clone();
        for draft in cleared.items.iter_mut().filter(|d| ids.contains(&d.id)) {
            if let Definition::Component(component) = &mut draft.definition {
                component.review = None;
            }
        }
        let restaged = candidate(library, &cleared, ids)?;
        *drafts = cleared;
        restaged
    } else {
        (staged, set)
    };
    *library = staged;
    for r in &set {
        drafts.set_base(r.id, r.revision);
    }
    Ok(set)
}
