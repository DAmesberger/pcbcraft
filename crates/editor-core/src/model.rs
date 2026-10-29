use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type Id = u64;
pub type Point = [f64; 2];

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub id: Id,
    pub name: String,
    pub kind: PinKind,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum PinKind {
    Passive,
    Input,
    Output,
    PowerInput,
    PowerOutput,
    NotConnected,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub terminals: Vec<Terminal>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SymbolPin {
    pub terminal: Id,
    pub position: Point,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Symbol {
    pub pins: Vec<SymbolPin>,
    pub body_size: Point,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageTerminal {
    pub id: Id,
    pub number: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub terminals: Vec<PackageTerminal>,
    pub body_size: [f64; 3],
    pub model_offset: [f64; 3],
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    pub id: Id,
    pub terminal: Id,
    pub position: Point,
    pub size: Point,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Footprint {
    pub pads: Vec<Pad>,
    pub datum_offset: Point,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalMap {
    pub electrical: Id,
    pub physical: Id,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub mappings: Vec<TerminalMap>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub id: Id,
    pub revision: u32,
    pub name: String,
    pub component: Component,
    pub symbol: Symbol,
    pub package: Package,
    pub footprint: Footprint,
    pub binding: Binding,
    pub provenance: String,
}
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(deny_unknown_fields)]
pub struct PartRef {
    pub id: Id,
    pub revision: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub id: Id,
    pub part: PartRef,
    pub reference: String,
    pub value: String,
    pub position: Point,
}
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub instance: Id,
    pub terminal: Id,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    pub id: Id,
    pub from: Endpoint,
    pub to: Endpoint,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Net {
    pub id: String,
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
    pub net: String,
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub layer: Layer,
    pub locked: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub next_id: Id,
    pub parts: Vec<Part>,
    pub instances: Vec<Instance>,
    pub wires: Vec<Wire>,
    pub placements: Vec<Placement>,
    pub tracks: Vec<Track>,
    pub board_size: Point,
}
impl Default for Project {
    fn default() -> Self {
        Self {
            version: 1,
            name: "Untitled".into(),
            next_id: 1,
            parts: vec![],
            instances: vec![],
            wires: vec![],
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
            .ok_or_else(|| crate::Error::Invalid("identity exhausted".into()))?;
        Ok(id)
    }
    pub fn part(&self, reference: PartRef) -> Option<&Part> {
        self.parts
            .iter()
            .find(|p| p.id == reference.id && p.revision == reference.revision)
    }
    pub fn instance(&self, id: Id) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }
}
