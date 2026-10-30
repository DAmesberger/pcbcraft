//! Library definitions, the project's immutable snapshot store, drafts and
//! publication. Definition bodies are plain data; the publish rules for each
//! kind live beside its type and are aggregated by [`publish_findings`], the
//! one function `library.publish`, document decode and `component.check` use.
mod binding;
mod component;
pub mod drafts;
mod footprint;
mod package;
mod publish;
pub mod samples;
mod symbol;

pub use binding::{BindingDefinition, TerminalMap};
pub use component::{ComponentDefinition, PinKind, Terminal};
pub use drafts::{Draft, DraftEdit, Drafts};
pub use footprint::{FootprintDefinition, Pad};
pub use package::{PackageDefinition, PackageTerminal};
pub use publish::{candidate, pending_revision, publish, publish_findings, stageable};
pub use symbol::{Anchor, SymbolDefinition, Unit};

use crate::diagnostics::{Error, Result};
use crate::ids::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, DefinitionRef, FootprintRef,
    PackageRef, Revision, SymbolRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Published revisions per kind.
pub const MAX_REVISIONS_PER_KIND: usize = 4096;
/// Names, labels and designations.
pub const MAX_LABEL: usize = 256;
/// Descriptions and provenance notes.
pub const MAX_NOTE: usize = 4096;
pub const MAX_TERMINALS: usize = 256;
pub const MAX_UNITS: usize = 64;
pub const MAX_PACKAGE_TERMINALS: usize = 512;
pub const MAX_PADS: usize = 1024;
pub const MAX_FOOTPRINTS_PER_BINDING: usize = 16;
/// Representations or implementations listed by one component revision.
pub const MAX_COMPONENT_LISTS: usize = 64;

/// The envelope every definition kind shares. Bodies carry no metadata.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DefinitionMeta {
    pub name: String,
    pub description: String,
    /// Free text. Never a verification stamp.
    pub provenance: String,
}

/// A definition body of any kind. Drafts and published revisions use the same
/// types; drafts may be incomplete.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum Definition {
    Component(ComponentDefinition),
    Symbol(SymbolDefinition),
    Package(PackageDefinition),
    Footprint(FootprintDefinition),
    Binding(BindingDefinition),
}

impl Definition {
    pub fn kind(&self) -> DefinitionKind {
        match self {
            Self::Component(_) => DefinitionKind::Component,
            Self::Symbol(_) => DefinitionKind::Symbol,
            Self::Package(_) => DefinitionKind::Package,
            Self::Footprint(_) => DefinitionKind::Footprint,
            Self::Binding(_) => DefinitionKind::Binding,
        }
    }
    /// An empty body of `kind`, for a new lineage authored step by step.
    pub fn empty(kind: DefinitionKind) -> Self {
        match kind {
            DefinitionKind::Component => Self::Component(Default::default()),
            DefinitionKind::Symbol => Self::Symbol(Default::default()),
            DefinitionKind::Package => Self::Package(Default::default()),
            DefinitionKind::Footprint => Self::Footprint(Default::default()),
            DefinitionKind::Binding => Self::Binding(Default::default()),
        }
    }
}

/// One immutable published revision.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Published<T> {
    pub id: DefinitionId,
    pub revision: Revision,
    pub meta: DefinitionMeta,
    pub definition: T,
}

/// Every published revision of every kind. Revisions are appended, never
/// rewritten, and every exact reference resolves inside this store, so a
/// project reopens offline with no other source.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Library {
    pub components: Vec<Published<ComponentDefinition>>,
    pub symbols: Vec<Published<SymbolDefinition>>,
    pub packages: Vec<Published<PackageDefinition>>,
    pub footprints: Vec<Published<FootprintDefinition>>,
    pub bindings: Vec<Published<BindingDefinition>>,
}

fn find<T>(items: &[Published<T>], id: DefinitionId, revision: Revision) -> Option<&Published<T>> {
    items.iter().find(|p| p.id == id && p.revision == revision)
}

fn latest_of<T>(items: &[Published<T>], id: DefinitionId) -> Option<&Published<T>> {
    items
        .iter()
        .filter(|p| p.id == id)
        .max_by_key(|p| p.revision)
}

impl Library {
    pub fn component(&self, r: ComponentRef) -> Option<&Published<ComponentDefinition>> {
        find(&self.components, r.id, r.revision)
    }
    pub fn symbol(&self, r: SymbolRef) -> Option<&Published<SymbolDefinition>> {
        find(&self.symbols, r.id, r.revision)
    }
    pub fn package(&self, r: PackageRef) -> Option<&Published<PackageDefinition>> {
        find(&self.packages, r.id, r.revision)
    }
    pub fn footprint(&self, r: FootprintRef) -> Option<&Published<FootprintDefinition>> {
        find(&self.footprints, r.id, r.revision)
    }
    pub fn binding(&self, r: BindingRef) -> Option<&Published<BindingDefinition>> {
        find(&self.bindings, r.id, r.revision)
    }

    /// The kind and latest revision of a lineage, if any revision is published.
    pub fn latest(&self, id: DefinitionId) -> Option<(DefinitionKind, Revision)> {
        [
            (
                DefinitionKind::Component,
                latest_of(&self.components, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Symbol,
                latest_of(&self.symbols, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Package,
                latest_of(&self.packages, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Footprint,
                latest_of(&self.footprints, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Binding,
                latest_of(&self.bindings, id).map(|p| p.revision),
            ),
        ]
        .into_iter()
        .find_map(|(kind, revision)| revision.map(|r| (kind, r)))
    }

    /// The latest published revision of each component lineage, oldest
    /// lineage first.
    pub fn latest_components(&self) -> Vec<&Published<ComponentDefinition>> {
        let ids: BTreeSet<DefinitionId> = self.components.iter().map(|p| p.id).collect();
        ids.into_iter()
            .filter_map(|id| latest_of(&self.components, id))
            .collect()
    }

    /// A published revision's envelope and body, cloned into the kind-tagged
    /// form drafts use.
    pub fn definition(&self, r: DefinitionRef) -> Option<(DefinitionMeta, Definition)> {
        fn take<T: Clone>(
            item: Option<&Published<T>>,
            wrap: fn(T) -> Definition,
        ) -> Option<(DefinitionMeta, Definition)> {
            item.map(|p| (p.meta.clone(), wrap(p.definition.clone())))
        }
        match r.kind {
            DefinitionKind::Component => take(
                find(&self.components, r.id, r.revision),
                Definition::Component,
            ),
            DefinitionKind::Symbol => {
                take(find(&self.symbols, r.id, r.revision), Definition::Symbol)
            }
            DefinitionKind::Package => {
                take(find(&self.packages, r.id, r.revision), Definition::Package)
            }
            DefinitionKind::Footprint => take(
                find(&self.footprints, r.id, r.revision),
                Definition::Footprint,
            ),
            DefinitionKind::Binding => {
                take(find(&self.bindings, r.id, r.revision), Definition::Binding)
            }
        }
    }

    /// Every published revision, as kind-tagged references.
    pub fn revisions(&self) -> Vec<DefinitionRef> {
        fn refs<T>(items: &[Published<T>], kind: DefinitionKind) -> Vec<DefinitionRef> {
            items
                .iter()
                .map(|p| DefinitionRef {
                    kind,
                    id: p.id,
                    revision: p.revision,
                })
                .collect()
        }
        let mut all = refs(&self.components, DefinitionKind::Component);
        all.extend(refs(&self.symbols, DefinitionKind::Symbol));
        all.extend(refs(&self.packages, DefinitionKind::Package));
        all.extend(refs(&self.footprints, DefinitionKind::Footprint));
        all.extend(refs(&self.bindings, DefinitionKind::Binding));
        all
    }

    /// Append one revision. Callers check revision order and findings.
    fn append(
        &mut self,
        id: DefinitionId,
        revision: Revision,
        meta: DefinitionMeta,
        body: Definition,
    ) {
        match body {
            Definition::Component(definition) => self.components.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Symbol(definition) => self.symbols.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Package(definition) => self.packages.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Footprint(definition) => self.footprints.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Binding(definition) => self.bindings.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
        }
    }

    fn count(&self, kind: DefinitionKind) -> usize {
        match kind {
            DefinitionKind::Component => self.components.len(),
            DefinitionKind::Symbol => self.symbols.len(),
            DefinitionKind::Package => self.packages.len(),
            DefinitionKind::Footprint => self.footprints.len(),
            DefinitionKind::Binding => self.bindings.len(),
        }
    }
}

fn label_bounded(text: &str) -> bool {
    text.len() <= MAX_LABEL
}

/// Persistence bounds and finite geometry: the only checks a draft must pass.
/// Structurally contradictory but finite content stays visible draft input.
pub fn check_bounds(meta: &DefinitionMeta, definition: &Definition) -> Result<()> {
    if !label_bounded(&meta.name)
        || meta.description.len() > MAX_NOTE
        || meta.provenance.len() > MAX_NOTE
    {
        return Err(Error::Limit(
            "definition name, description or provenance is too long".into(),
        ));
    }
    let within = match definition {
        Definition::Component(d) => d.within_bounds(),
        Definition::Symbol(d) => d.within_bounds(),
        Definition::Package(d) => d.within_bounds(),
        Definition::Footprint(d) => d.within_bounds(),
        Definition::Binding(d) => d.within_bounds(),
    };
    if !within {
        return Err(Error::Limit(
            "definition exceeds supported text or item limits".into(),
        ));
    }
    let finite = match definition {
        Definition::Component(_) | Definition::Binding(_) => true,
        Definition::Symbol(d) => d.geometry_finite(),
        Definition::Package(d) => d.geometry_finite(),
        Definition::Footprint(d) => d.geometry_finite(),
    };
    if !finite {
        return Err(Error::Invalid(
            "definition geometry must be finite and within ±10000 mm".into(),
        ));
    }
    Ok(())
}

/// The publication order: a revision may only depend on kinds before it.
pub(crate) fn publish_rank(kind: DefinitionKind) -> u8 {
    match kind {
        DefinitionKind::Package => 0,
        DefinitionKind::Footprint => 1,
        DefinitionKind::Binding => 2,
        DefinitionKind::Symbol => 3,
        DefinitionKind::Component => 4,
    }
}

/// Validate a stored snapshot: identities, revision sequences, bounds and,
/// through [`publish_findings`], exactly the rules `library.publish` applies.
/// Advisory findings never fail a document.
pub fn validate(library: &Library) -> Result<()> {
    let mut kinds: BTreeMap<DefinitionId, (DefinitionKind, BTreeSet<Revision>)> = BTreeMap::new();
    let revisions = library.revisions();
    for kind in [
        DefinitionKind::Component,
        DefinitionKind::Symbol,
        DefinitionKind::Package,
        DefinitionKind::Footprint,
        DefinitionKind::Binding,
    ] {
        if library.count(kind) > MAX_REVISIONS_PER_KIND {
            return Err(Error::Invalid(format!(
                "too many published {kind:?} revisions"
            )));
        }
    }
    for r in &revisions {
        if r.id.0 == 0 || r.revision.0 == 0 {
            return Err(Error::Invalid(
                "published definition identities and revisions must be nonzero".into(),
            ));
        }
        let entry = kinds.entry(r.id).or_insert((r.kind, BTreeSet::new()));
        if entry.0 != r.kind {
            return Err(Error::Invalid(format!(
                "lineage {} is published as more than one kind",
                r.id
            )));
        }
        if !entry.1.insert(r.revision) {
            return Err(Error::Invalid(format!(
                "{} {} is published twice",
                r.id, r.revision
            )));
        }
    }
    for (id, (_, set)) in &kinds {
        let contiguous = set
            .iter()
            .enumerate()
            .all(|(i, r)| u32::try_from(i + 1).is_ok_and(|n| n == r.0));
        if !contiguous {
            return Err(Error::Invalid(format!(
                "revisions of {id} must run contiguously from r1"
            )));
        }
    }
    for r in revisions {
        let Some((meta, body)) = library.definition(r) else {
            return Err(Error::Invalid("missing published revision".into()));
        };
        check_bounds(&meta, &body)?;
        let blocking: Vec<String> = publish_findings(library, r)
            .into_iter()
            .filter(|f| f.blocks_stage(crate::diagnostics::Stage::Publish))
            .map(|f| f.message)
            .collect();
        if !blocking.is_empty() {
            return Err(Error::Invalid(format!(
                "published {} {} is invalid: {}",
                r.id,
                r.revision,
                blocking.join("; ")
            )));
        }
    }
    Ok(())
}

/// Text that publication accepts as a name or label.
pub(crate) fn printable(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_LABEL && !text.chars().any(char::is_control)
}
