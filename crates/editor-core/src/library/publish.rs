//! Publication: the per-kind rules, aggregated once, and the atomic publish
//! of a draft set in dependency order.
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
        Definition::Binding(_) | Definition::QuantityKind(_) => None,
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
                format!("Local ID {local} is zero or used more than once."),
                vec![object(id, *local)],
            ));
        }
        if *local >= next_local {
            out.push(issue(
                "definition.local_id_unallocated",
                format!("Local ID {local} is not below this revision's next_local {next_local}."),
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
            format!("next_local {next_local} is below the base revision's {base_next}."),
            vec![this],
        ));
    }
    let base_ids: BTreeSet<u32> = base_ids.into_iter().collect();
    for local in seen.difference(&base_ids) {
        if *local < base_next {
            out.push(issue(
                "definition.local_id_reused",
                format!("Local ID {local} was retired before this revision and cannot be reused."),
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
                "{:?} {} {} is not in the project library.",
                r.kind, r.id, r.revision
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
        Definition::Field(d) => {
            let kind = d.value_type.kind().map(|k| (k, library.quantity_kind(k)));
            if let Some((k, None)) = kind {
                out.push(issue(
                    "reference.missing",
                    format!("Quantity kind {k} is not in the project library."),
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
        (Some((stored, latest)), Some(base)) if stored == kind && latest == base => base
            .next()
            .ok_or_else(|| Error::Limit(format!("revisions of {id} are exhausted")))?,
        (Some((stored, _)), _) if stored != kind => {
            return Err(Error::Conflict(format!(
                "draft {id} is a {kind:?} but the lineage is published as {stored:?}"
            )));
        }
        (Some((_, latest)), _) => {
            return Err(Error::Conflict(format!(
                "{id} has a newer revision ({latest}); load it before publishing"
            )));
        }
        (None, Some(_)) => {
            return Err(Error::Invalid(format!(
                "draft {id} names a base revision that does not exist"
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
            return Err(Error::Invalid(format!("draft {id} is listed twice")));
        }
        let draft = drafts
            .get(*id)
            .ok_or_else(|| Error::NotFound(format!("there is no draft of {id}")))?;
        check_bounds(&draft.meta, &draft.definition)?;
        set.push((pending_revision(library, draft)?, draft));
    }
    set.sort_by_key(|(r, _)| (publish_rank(r.kind), r.id));
    let mut staged = library.clone();
    for (r, draft) in &set {
        if staged.count(r.kind) >= super::MAX_REVISIONS_PER_KIND {
            return Err(Error::Limit(format!(
                "the project library holds at most 4096 {:?} revisions",
                r.kind
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

/// Publish a set of drafts as one transaction, in dependency order. References
/// may name stored revisions or other drafts' pending revisions in the set.
/// Any blocking finding, stale base or unsatisfied reference publishes
/// nothing. Each published draft then becomes a draft of its new revision.
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
                blocking.push(format!("{} {}: {}", r.id, r.revision, finding.message));
            }
        }
    }
    if !blocking.is_empty() {
        let shown = blocking.len().min(16);
        let mut message = format!("Nothing was published. {}", blocking[..shown].join(" "));
        if blocking.len() > shown {
            message.push_str(&format!(" ({} more findings)", blocking.len() - shown));
        }
        return Err(Error::Conflict(message));
    }
    *library = staged;
    for r in &set {
        drafts.set_base(r.id, r.revision);
    }
    Ok(set)
}
