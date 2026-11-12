//! The five drafts the component editor authors together: a component, one
//! symbol, its package, one footprint and the binding between them. Every
//! change reaches the engine as public library commands.
#[cfg(test)]
use pcbcraft_core::library::samples::{self, SampleRefs};
use pcbcraft_core::library::{
    BindingDefinition, ComponentDefinition, Definition, DefinitionMeta, DraftEdit, Drafts,
    FootprintDefinition, PackageDefinition, SymbolDefinition,
};
use pcbcraft_core::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, DefinitionRef, Engine, Error,
    FootprintRef, LibraryCommand, Operation, PackageRef, Result, Revision, SymbolRef,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ids {
    pub component: DefinitionId,
    pub symbol: DefinitionId,
    pub package: DefinitionId,
    pub footprint: DefinitionId,
    pub binding: DefinitionId,
}

impl Ids {
    pub fn all(&self) -> Vec<DefinitionId> {
        vec![
            self.component,
            self.symbol,
            self.package,
            self.footprint,
            self.binding,
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metas {
    pub component: DefinitionMeta,
    pub symbol: DefinitionMeta,
    pub package: DefinitionMeta,
    pub footprint: DefinitionMeta,
    pub binding: DefinitionMeta,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkingSet {
    pub ids: Ids,
    pub metas: Metas,
    pub component: ComponentDefinition,
    pub symbol: SymbolDefinition,
    pub package: PackageDefinition,
    pub footprint: FootprintDefinition,
    pub binding: BindingDefinition,
}

/// The revisions sibling references pin.
#[derive(Clone, Copy)]
struct Revisions {
    symbol: Revision,
    package: Revision,
    footprint: Revision,
    binding: Revision,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    TwoTerminal,
    Header,
    /// A component on a typed package preset with a generated model.
    Package(samples::Preset),
}

#[cfg(test)]
impl Template {
    pub fn name(self) -> &'static str {
        match self {
            Self::TwoTerminal => "Resistor",
            Self::Header => "Connector",
            Self::Package(preset) => preset.label(),
        }
    }
    fn edits(self, refs: SampleRefs) -> pcbcraft_core::Result<Vec<DraftEdit>> {
        match self {
            Self::TwoTerminal => samples::two_terminal(self.name(), samples::Kind::RESISTOR, refs),
            Self::Header => {
                samples::two_terminal_header(self.name(), samples::Kind::CONNECTOR, refs)
            }
            Self::Package(preset) => samples::packaged(self.name(), preset, refs),
        }
    }
}

impl WorkingSet {
    /// The complete set of drafts around exactly the component lineage
    /// `lineage`; never another one. `None` takes the first complete set,
    /// which only listings and tests ask for: the Component workspace
    /// always opens one lineage explicitly ([`Self::of`]).
    pub fn find(drafts: &Drafts, lineage: Option<DefinitionId>) -> Option<Self> {
        match lineage {
            Some(lineage) => Self::of(drafts, lineage),
            None => drafts
                .items
                .iter()
                .filter(|d| d.definition.kind() == DefinitionKind::Component)
                .find_map(|component| Self::around(drafts, component.id)),
        }
    }

    /// The complete set of drafts around the component lineage `lineage`.
    pub fn of(drafts: &Drafts, lineage: DefinitionId) -> Option<Self> {
        Self::around(drafts, lineage)
    }

    fn around(drafts: &Drafts, component: DefinitionId) -> Option<Self> {
        let draft = |id| drafts.get(id);
        let component_draft = draft(component)?;
        let Definition::Component(component_body) = &component_draft.definition else {
            return None;
        };
        let (symbol_id, symbol_meta, symbol) =
            drafts.items.iter().find_map(|d| match &d.definition {
                Definition::Symbol(s) if s.component == component => {
                    Some((d.id, d.meta.clone(), s.clone()))
                }
                _ => None,
            })?;
        let (binding_id, binding_meta, binding) =
            drafts.items.iter().find_map(|d| match &d.definition {
                Definition::Binding(b) if b.component == component => {
                    Some((d.id, d.meta.clone(), b.clone()))
                }
                _ => None,
            })?;
        let package_draft = draft(binding.package.id)?;
        let Definition::Package(package) = &package_draft.definition else {
            return None;
        };
        let (footprint_id, footprint_meta, footprint) =
            binding.footprints.iter().find_map(|f| {
                let d = draft(f.id)?;
                match &d.definition {
                    Definition::Footprint(body) => Some((d.id, d.meta.clone(), body.clone())),
                    _ => None,
                }
            })?;
        Some(Self {
            ids: Ids {
                component,
                symbol: symbol_id,
                package: package_draft.id,
                footprint: footprint_id,
                binding: binding_id,
            },
            metas: Metas {
                component: component_draft.meta.clone(),
                symbol: symbol_meta,
                package: package_draft.meta.clone(),
                footprint: footprint_meta,
                binding: binding_meta,
            },
            component: component_body.clone(),
            symbol,
            package: package.clone(),
            footprint,
            binding,
        })
    }

    /// The working set the given edits describe, if they are one complete set.
    #[cfg(test)]
    pub fn from_edits(edits: Vec<DraftEdit>) -> Option<Self> {
        let mut drafts = Drafts::default();
        for edit in edits {
            drafts.items.push(pcbcraft_core::library::Draft {
                id: edit.id,
                base: None,
                meta: edit.meta,
                definition: edit.definition,
            });
        }
        let component = drafts
            .items
            .iter()
            .find(|d| d.definition.kind() == DefinitionKind::Component)?
            .id;
        Self::around(&drafts, component)
    }

    pub fn edits(&self) -> Vec<DraftEdit> {
        vec![
            DraftEdit {
                id: self.ids.component,
                meta: self.metas.component.clone(),
                definition: Definition::Component(self.component.clone()),
            },
            DraftEdit {
                id: self.ids.symbol,
                meta: self.metas.symbol.clone(),
                definition: Definition::Symbol(self.symbol.clone()),
            },
            DraftEdit {
                id: self.ids.package,
                meta: self.metas.package.clone(),
                definition: Definition::Package(self.package.clone()),
            },
            DraftEdit {
                id: self.ids.footprint,
                meta: self.metas.footprint.clone(),
                definition: Definition::Footprint(self.footprint.clone()),
            },
            DraftEdit {
                id: self.ids.binding,
                meta: self.metas.binding.clone(),
                definition: Definition::Binding(self.binding.clone()),
            },
        ]
    }

    fn with_revisions(&self, revisions: Revisions) -> Self {
        let mut set = self.clone();
        let ids = self.ids;
        for r in &mut set.component.representations {
            if r.id == ids.symbol {
                *r = SymbolRef::new(ids.symbol, revisions.symbol);
            }
        }
        for r in &mut set.component.implementations {
            if r.id == ids.binding {
                *r = BindingRef::new(ids.binding, revisions.binding);
            }
        }
        let package = PackageRef::new(ids.package, revisions.package);
        if set.footprint.package.id == ids.package {
            set.footprint.package = package;
        }
        if set.binding.package.id == ids.package {
            set.binding.package = package;
        }
        for r in &mut set.binding.footprints {
            if r.id == ids.footprint {
                *r = FootprintRef::new(ids.footprint, revisions.footprint);
            }
        }
        set
    }

    fn revisions(
        &self,
        drafts: &Drafts,
        pick: fn(&pcbcraft_core::library::Draft) -> Option<Revision>,
    ) -> Option<Revisions> {
        let revision = |id| drafts.get(id).and_then(pick);
        Some(Revisions {
            symbol: revision(self.ids.symbol)?,
            package: revision(self.ids.package)?,
            footprint: revision(self.ids.footprint)?,
            binding: revision(self.ids.binding)?,
        })
    }

    /// The set with sibling references pinning the siblings' pending
    /// revisions, so the five publish together as one cascade.
    pub fn retargeted(&self, drafts: &Drafts) -> Self {
        match self.revisions(drafts, |d| d.pending()) {
            Some(revisions) => self.with_revisions(revisions),
            None => self.clone(),
        }
    }

    /// Whether any of the five differs from its published base revision once
    /// sibling references are read against the bases.
    pub fn unpublished(&self, engine: &Engine) -> bool {
        let drafts = engine.drafts();
        let Some(bases) = self.revisions(drafts, |d| d.base) else {
            return true;
        };
        let normalized = self.with_revisions(bases);
        normalized.edits().into_iter().any(|edit| {
            let Some(base) = drafts.get(edit.id).and_then(|d| d.base) else {
                return true;
            };
            let published = engine.project().library.definition(DefinitionRef {
                kind: edit.definition.kind(),
                id: edit.id,
                revision: base,
            });
            published != Some((edit.meta, edit.definition))
        })
    }

    /// The latest published component revision of this lineage.
    pub fn published(&self, engine: &Engine) -> Option<ComponentRef> {
        engine
            .project()
            .library
            .latest(self.ids.component)
            .map(|(_, revision)| ComponentRef::new(self.ids.component, revision))
    }

    /// Replace the bodies with a template, keeping lineages and never lowering
    /// a local-ID counter.
    #[cfg(test)]
    pub fn load_template(&mut self, template: Template) {
        let first = Revision::FIRST;
        let refs = SampleRefs {
            component: self.ids.component,
            symbol: SymbolRef::new(self.ids.symbol, first),
            package: PackageRef::new(self.ids.package, first),
            footprint: FootprintRef::new(self.ids.footprint, first),
            binding: BindingRef::new(self.ids.binding, first),
        };
        let Some(mut sample) = template.edits(refs).ok().and_then(Self::from_edits) else {
            return;
        };
        sample.component.next_local = sample.component.next_local.max(self.component.next_local);
        sample.symbol.next_local = sample.symbol.next_local.max(self.symbol.next_local);
        sample.package.next_local = sample.package.next_local.max(self.package.next_local);
        sample.footprint.next_local = sample.footprint.next_local.max(self.footprint.next_local);
        *self = sample;
    }

    /// Publish the five drafts together.
    pub fn publish(&self) -> Operation {
        Operation::Library(LibraryCommand::Publish {
            drafts: self.ids.all(),
            close: false,
        })
    }
}

/// Tests: start a new component from a sample template in a project
/// without a working set: five `library.new_draft` reservations, then one
/// `library.edit`.
#[cfg(test)]
pub fn create(engine: &mut Engine, template: Template) -> Result<DefinitionId> {
    let mut ids = Vec::new();
    for kind in [
        DefinitionKind::Component,
        DefinitionKind::Symbol,
        DefinitionKind::Package,
        DefinitionKind::Footprint,
        DefinitionKind::Binding,
    ] {
        if let pcbcraft_core::Outcome::DraftCreated { id } = engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta::default(),
                definition: Definition::empty(kind),
            }),
        )? {
            ids.push(id);
        }
    }
    let [component, symbol, package, footprint, binding] = ids[..] else {
        return Err(Error::Invalid("draft reservation failed".into()));
    };
    let first = Revision::FIRST;
    let edits = template.edits(SampleRefs {
        component,
        symbol: SymbolRef::new(symbol, first),
        package: PackageRef::new(package, first),
        footprint: FootprintRef::new(footprint, first),
        binding: BindingRef::new(binding, first),
    })?;
    engine
        .apply(
            engine.revision(),
            Operation::Library(LibraryCommand::Edit { edits }),
        )
        .map(|_| component)
}

/// Draft every lineage of a published component revision's first symbol and
/// implementation that has no draft yet: one `library.draft_closure`, so
/// one undo step (none when everything is drafted already).
/// Returns how many drafts it created: drafts are project state (saved
/// with the project and undoable), so creating them is a real change.
pub fn draft_closure(engine: &mut Engine, component: ComponentRef) -> Result<usize> {
    match engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::DraftClosure { component }),
    )? {
        pcbcraft_core::Outcome::DraftsCreated { ids } => Ok(ids.len()),
        _ => Err(Error::Invalid(
            "library.draft_closure returned an unexpected outcome".into(),
        )),
    }
}
