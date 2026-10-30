//! The document aggregate: data and lookups only. Rules over this data live
//! in `instances`, `nets`, `schematic` and `layout`.
use crate::fields::InstanceFields;
use crate::functions::FunctionSelections;
pub use crate::geometry::Point;
use crate::ids::{ComponentRef, DefinitionId, Endpoint, Id, TerminalId, UnitId};
use crate::library::{Drafts, Library};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The physical implementation an instance uses, as lineages resolved
/// through its pinned component revision.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PhysicalChoice {
    pub binding: DefinitionId,
    pub footprint: DefinitionId,
}

/// One use of a component in the project. It pins one exact component
/// revision; every other choice is a lineage that revision offers.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ComponentInstance {
    pub id: Id,
    pub component: ComponentRef,
    pub designator: String,
    /// Field overrides by field lineage; a missing field is inherited.
    pub properties: InstanceFields,
    /// A symbol lineage listed by the pinned component.
    pub representation: DefinitionId,
    pub physical: Option<PhysicalChoice>,
    /// Units deliberately left off the sheet.
    pub unused_units: BTreeSet<UnitId>,
    /// Project decisions that these terminals stay unconnected.
    pub no_connect: BTreeSet<TerminalId>,
    /// Explicit function selections by terminal. A missing terminal uses its
    /// fixed function or design default.
    pub functions: FunctionSelections,
}

/// One unit of one instance drawn on the sheet. Each (instance, unit) has at
/// most one occurrence.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SymbolOccurrence {
    pub id: Id,
    pub instance: Id,
    pub unit: UnitId,
    pub position: Point,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    pub id: Id,
    pub from: Endpoint,
    pub to: Endpoint,
}

/// A persistent electrical net. Its ID survives renaming, unrelated edits and
/// saving; the name is a display label. Every instance terminal belongs to
/// exactly one net, and a net's terminals are joined by its wires.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Net {
    pub id: Id,
    pub name: String,
    pub terminals: Vec<Endpoint>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum Layer {
    Top,
    Bottom,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub instance: Id,
    pub position: Point,
    pub rotation_deg: f64,
    pub layer: Layer,
    pub locked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: Id,
    pub net: Id,
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub layer: Layer,
    pub locked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    /// Object and definition-lineage identities share this allocator.
    pub next_id: Id,
    /// Every published revision this project can reference.
    pub library: Library,
    pub instances: Vec<ComponentInstance>,
    pub occurrences: Vec<SymbolOccurrence>,
    pub wires: Vec<Wire>,
    /// Net IDs have their own allocation space, independent of object IDs.
    pub next_net: Id,
    pub nets: Vec<Net>,
    pub placements: Vec<Placement>,
    pub tracks: Vec<Track>,
    pub board_size: Point,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            name: "Untitled".into(),
            next_id: 1,
            library: Library::default(),
            instances: vec![],
            occurrences: vec![],
            wires: vec![],
            next_net: 1,
            nets: vec![],
            placements: vec![],
            tracks: vec![],
            board_size: [60.0, 40.0],
        }
    }
}

impl Project {
    pub fn allocate(&mut self) -> crate::Result<Id> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| crate::Error::Limit("object identity space exhausted".into()))?;
        Ok(id)
    }
    pub fn allocate_net(&mut self) -> crate::Result<Id> {
        let id = self.next_net;
        self.next_net = id
            .checked_add(1)
            .ok_or_else(|| crate::Error::Limit("net identity space exhausted".into()))?;
        Ok(id)
    }
    pub fn instance(&self, id: Id) -> Option<&ComponentInstance> {
        self.instances.iter().find(|i| i.id == id)
    }
    /// The instance with this designator, compared case-insensitively.
    pub fn instance_by_designator(&self, designator: &str) -> Option<&ComponentInstance> {
        self.instances
            .iter()
            .find(|i| i.designator.eq_ignore_ascii_case(designator))
    }
    pub fn occurrence(&self, id: Id) -> Option<&SymbolOccurrence> {
        self.occurrences.iter().find(|o| o.id == id)
    }
    pub fn occurrences_of(&self, instance: Id) -> impl Iterator<Item = &SymbolOccurrence> {
        self.occurrences
            .iter()
            .filter(move |o| o.instance == instance)
    }
    /// Wires with an endpoint on `terminal`.
    pub fn wires_at(&self, terminal: Endpoint) -> impl Iterator<Item = &Wire> {
        self.wires
            .iter()
            .filter(move |w| w.from == terminal || w.to == terminal)
    }
    pub fn placement_of(&self, instance: Id) -> Option<&Placement> {
        self.placements.iter().find(|p| p.instance == instance)
    }
    pub fn net(&self, id: Id) -> Option<&Net> {
        self.nets.iter().find(|n| n.id == id)
    }
    /// The net containing a terminal; every valid terminal has one.
    pub fn net_of(&self, terminal: Endpoint) -> Option<&Net> {
        self.nets
            .iter()
            .find(|n| n.terminals.binary_search(&terminal).is_ok())
    }
    pub fn net_name(&self, id: Id) -> &str {
        self.net(id).map_or("unknown net", |n| n.name.as_str())
    }
}

/// Whether an edit may make consequential changes (remove wires, placements)
/// that a reviewed update plan lists, or must refuse them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditPolicy {
    Direct,
    Consequential,
}

/// Everything one undo step restores: the project and the drafts being
/// authored. Unchanged halves are shared between history entries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub project: Arc<Project>,
    pub drafts: Arc<Drafts>,
}
