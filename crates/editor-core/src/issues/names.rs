//! The user's name of every object an issue refers to, for titles: a
//! designator, a net name, "R2 pad 1", "terminal ‘VOUT’", never an
//! internal ID.
use crate::diagnostics::ObjectRef;
use crate::ids::{DefinitionId, DefinitionRef, Id, PackageTerminalId, PadId, TerminalId, UnitId};
use crate::instances;
use crate::layout;
use crate::library::{Definition, Drafts};
use crate::model::Project;
use std::collections::BTreeMap;

pub(super) struct Names<'a> {
    project: &'a Project,
    drafts: &'a Drafts,
    pads: BTreeMap<(Id, PadId), String>,
}

impl<'a> Names<'a> {
    pub(super) fn new(project: &'a Project, drafts: &'a Drafts) -> Self {
        Self {
            project,
            drafts,
            pads: layout::pad_labels(project),
        }
    }

    pub(super) fn designator(&self, id: Id) -> String {
        self.project
            .instance(id)
            .map_or_else(|| "an instance".into(), |i| i.designator.clone())
    }

    pub(super) fn net(&self, id: Id) -> String {
        self.project.net_name(id).to_owned()
    }

    /// A definition lineage by its name: the draft's, else the latest
    /// published revision's.
    pub(super) fn definition(&self, id: DefinitionId) -> String {
        self.drafts
            .get(id)
            .map(|d| d.meta.name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .or_else(|| self.project.library.lineage_name(id).map(str::to_owned))
            .map_or_else(
                || "an unnamed definition".into(),
                |name| format!("‘{name}’"),
            )
    }

    /// The body of a lineage: the draft, else the latest published revision.
    pub(super) fn body(&self, id: DefinitionId) -> Option<Definition> {
        if let Some(draft) = self.drafts.get(id) {
            return Some(draft.definition.clone());
        }
        let (kind, revision) = self.project.library.latest(id)?;
        self.project
            .library
            .definition(DefinitionRef { kind, id, revision })
            .map(|(_, body)| body)
    }

    /// "R1 terminal ‘VOUT’", or "R1 terminal" when it has no name.
    pub(super) fn terminal(&self, instance: Id, terminal: TerminalId) -> String {
        let name = self
            .project
            .instance(instance)
            .and_then(|i| self.project.library.component(i.component))
            .and_then(|c| c.definition.terminal(terminal))
            .map(|t| t.name.trim().to_owned())
            .filter(|name| !name.is_empty());
        let designator = self.designator(instance);
        name.map_or_else(
            || format!("{designator} pin"),
            |name| format!("{designator} pin {name}"),
        )
    }

    /// A local object of a definition: "terminal ‘VOUT’", "package terminal
    /// ‘2’", "pad 1", "unit ‘A’", or its kind when it has no name.
    pub(super) fn local(&self, definition: DefinitionId, local: u32) -> String {
        let Some(body) = self.body(definition) else {
            return "an object".into();
        };
        let quoted = |name: &str| {
            let name = name.trim();
            (!name.is_empty()).then(|| format!("‘{name}’"))
        };
        match &body {
            Definition::Component(c) => c
                .terminal(TerminalId(local))
                .and_then(|t| quoted(&t.name))
                .map_or_else(|| "a function".into(), |n| format!("terminal {n}")),
            Definition::Package(p) => p
                .terminal(PackageTerminalId(local))
                .and_then(|t| quoted(&t.designation))
                .map_or_else(
                    || "a package terminal".into(),
                    |n| format!("package terminal {n}"),
                ),
            Definition::Footprint(f) => match f.pad(PadId(local)) {
                Some(pad) if !pad.number.trim().is_empty() => format!("pad {}", pad.number.trim()),
                Some(_) => "a pad without a number".into(),
                None => "a graphic".into(),
            },
            Definition::Symbol(s) => s
                .unit(UnitId(local))
                .and_then(|u| quoted(&u.label))
                .map_or_else(|| "a graphic".into(), |n| format!("unit {n}")),
            Definition::Part(_) => "a reference or offer".into(),
            Definition::Field(_) => "a choice".into(),
            Definition::Model(_) => "a body or lead".into(),
            Definition::Binding(_) | Definition::QuantityKind(_) | Definition::PackageRow(_) => {
                "an object".into()
            }
        }
    }

    /// Any referenced object, for a title.
    pub(super) fn object(&self, object: &ObjectRef) -> String {
        match *object {
            ObjectRef::Instance { id } => self.designator(id),
            ObjectRef::Terminal { endpoint } => self.terminal(endpoint.instance, endpoint.terminal),
            ObjectRef::Net { id } => self.net(id),
            ObjectRef::Track { id } => self.project.tracks.iter().find(|t| t.id == id).map_or_else(
                || "a track".into(),
                |t| format!("track on {}", self.net(t.net)),
            ),
            ObjectRef::Pad { instance, pad } => match self.pads.get(&(instance, pad)) {
                Some(label) => format!("{} pad {label}", self.designator(instance)),
                None => format!("a pad of {}", self.designator(instance)),
            },
            ObjectRef::Definition { id } => self.definition(id),
            ObjectRef::LibraryObject { definition, local } => self.local(definition, local),
            ObjectRef::Occurrence { id } => self
                .project
                .occurrence(id)
                .map_or_else(|| "a symbol".into(), |o| self.unit(o.instance, o.unit)),
            ObjectRef::Unit { instance, unit } => self.unit(instance, unit),
            ObjectRef::Property { instance, field } => format!(
                "{} {}",
                self.designator(instance),
                self.project.library.lineage_name(field).unwrap_or("field")
            ),
            ObjectRef::Via { id } => self
                .project
                .via(id)
                .map_or_else(|| "a via".into(), |v| format!("via on {}", self.net(v.net))),
            ObjectRef::Fill { id } => self.project.fill(id).map_or_else(
                || "a fill".into(),
                |f| crate::layout::fill::fill_label(self.project, f),
            ),
            ObjectRef::Keepout { id } => self.project.keepout(id).map_or_else(
                || "a keepout".into(),
                |k| {
                    if k.name.is_empty() {
                        "a keepout".into()
                    } else {
                        format!("keepout {}", k.name)
                    }
                },
            ),
            ObjectRef::Board => "the board".into(),
        }
    }

    fn unit(&self, instance: Id, unit: UnitId) -> String {
        match self.project.instance(instance) {
            Some(item) => instances::display_label(self.project, item, unit),
            None => "an instance".into(),
        }
    }
}
