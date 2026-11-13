//! The board interface's stored data: exported datums, their slot table,
//! the automatic export rules and the last published revision
//! (docs/architecture/board-interface.md). Data only, with no rules and no
//! geometry: a datum's geometry is derived from the board and its parts
//! each time it is needed, never stored. The rules (slot allocation, the
//! table's invariants, tokens) live in `crate::interface`.
//!
//! The table is `Project::interface`; slice B1 keeps it through the
//! `interface.*` commands and the automatic rules (`crate::interface::auto`).
use crate::ids::{ContentHash, Id, InterfaceSourceId};
use crate::library::footprint_source::SketchCurveId;
use crate::model::board::BoardFeatureId;
use crate::model::dimension::BoardRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Live (not retired) datums of one board.
pub const MAX_DATUMS: usize = 4096;
/// Slots ever allocated on one board, retired ones included. Slots are
/// never reused, so this bounds the board's whole history; reaching it
/// needs a deliberate compaction (a new [`InterfaceSourceId`] epoch).
pub const MAX_SLOTS: u32 = 65_536;
/// Feature ids one slot uses in the exported design, padded, so every
/// slot's ids follow from its position alone (the design note's "small
/// fixed budget").
pub const SLOT_FEATURE_BUDGET: u32 = 4;

/// A datum's document identity, allocated from the table's own counter
/// ([`InterfaceTable::next_datum`]), like nets from theirs, so exporting a
/// datum never shifts the ids of other objects. Never reused: the counter,
/// like the slot counter, only grows, through undo and redo too.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct DatumId(pub Id);

impl fmt::Display for DatumId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "d{}", self.0)
    }
}

/// A datum's position in the board's append-only slot table, from 0. The
/// exported design builds slots first and in slot order, so a slot's
/// feature ids, and with them its face names, follow from this number
/// alone. A slot is never reused: a removed datum's slot is retired.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct Slot(pub u32);

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "slot {}", self.0)
    }
}

/// The object a datum belongs to. Identity only: never a designator, a
/// name or a build order.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum DatumOwner {
    /// The board itself: its frame, faces, outline, thickness, parameters
    /// and the tallest part per side.
    Board,
    /// A component placement, by instance id.
    Instance { id: Id },
    /// A mounting hole, by board hole id.
    Hole { id: BoardFeatureId },
    /// A cutout or slot of the board sketch, by the sketch curve that
    /// starts its loop (the curve id of the lowest-ordered curve of the
    /// loop; sketch curve ids are never reused).
    Curve { id: SketchCurveId },
}

/// What a datum is to its owner (docs/architecture/board-interface.md,
/// "Roles"). The role and the owner decide the datum's kind, except for
/// [`Self::PartDatum`] and [`Self::Feature`], whose kind comes from their
/// source.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum DatumRole {
    #[serde(rename = "board.frame")]
    BoardFrame,
    #[serde(rename = "board.top")]
    BoardTop,
    #[serde(rename = "board.bottom")]
    BoardBottom,
    #[serde(rename = "board.outline")]
    BoardOutline,
    #[serde(rename = "board.cutout")]
    BoardCutout,
    #[serde(rename = "board.thickness")]
    BoardThickness,
    /// An exported board parameter, named by the key's `name`.
    #[serde(rename = "board.parameter")]
    BoardParameter,
    /// A mounting hole's axis, with its diameter.
    #[serde(rename = "hole")]
    Hole,
    #[serde(rename = "part.frame")]
    PartFrame,
    /// Courtyard × body height.
    #[serde(rename = "part.envelope")]
    PartEnvelope,
    /// A datum the part's package revision declares, named by the key's
    /// `name` (`part.opening` in the sidecar).
    #[serde(rename = "part.datum")]
    PartDatum,
    /// The tallest part on one side, the key's `name` being `top` or
    /// `bottom`.
    #[serde(rename = "side.height")]
    SideHeight,
    /// A feature the user exported: a part face, edge or vertex, a pad
    /// centre, a board curve.
    #[serde(rename = "feature")]
    Feature,
}

/// What identifies a datum to the rules that make it: one key gives at
/// most one live datum. A feature datum's key has no name; its exported
/// feature tells it apart.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct DatumKey {
    pub owner: DatumOwner,
    pub role: DatumRole,
    /// The package datum's name (`DatumName` syntax), the board
    /// parameter's name (`ParamName` syntax), or the side (`top`, `bottom`)
    /// of a side height; `None` for every other role. Identity, unlike the
    /// display name; `interface::check_key` holds the syntax per role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// An automatic export rule. Every rule is on by default; each can be
/// switched off (`interface.auto`).
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum AutoRule {
    /// The board frame, top, bottom and thickness.
    Board,
    /// The board outline.
    Outline,
    /// Every cutout and slot.
    Cutouts,
    /// Every mounting hole.
    Holes,
    /// Every package datum marked `external`, for each placed instance.
    ExternalDatums,
    /// The tallest part per side.
    SideHeights,
}

impl AutoRule {
    pub const ALL: [Self; 6] = [
        Self::Board,
        Self::Outline,
        Self::Cutouts,
        Self::Holes,
        Self::ExternalDatums,
        Self::SideHeights,
    ];
}

/// Why a datum is exported.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "by", deny_unknown_fields)]
pub enum DatumSource {
    /// An automatic rule made it; switching the rule off retires it.
    Auto { rule: AutoRule },
    /// The user exported it (`interface.export`).
    User,
}

/// One live exported datum. Its geometry is derived, never stored.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDatum {
    pub id: DatumId,
    pub slot: Slot,
    pub key: DatumKey,
    pub source: DatumSource,
    /// The exported feature of a [`DatumRole::Feature`] datum (never a
    /// followed face); `None` for every other role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<BoardRef>,
    /// The display name the user gave (`interface.rename`); `None` shows
    /// the derived one ("J1 opening"). Display text, never identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// How much of the board the exported design carries after the slots.
#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// The slots only.
    Interface,
    /// The slots, the laminate, pads and parts (the default).
    #[default]
    Mechanical,
    /// The slots and every copper body as well.
    Full,
}

/// One datum as it was last published: its slot and content hash, so a
/// preview tells changed datums from unchanged ones without the file.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublishedDatum {
    pub id: DatumId,
    pub slot: Slot,
    pub hash: ContentHash,
}

/// The last publish the project recorded (`interface.record_publish`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublishedRecord {
    /// Counts publishes whose content hash differed, from 1.
    pub revision: u32,
    /// The canonical datum table's SHA-256.
    pub hash: ContentHash,
    pub tier: Tier,
    /// Every datum published, in slot order.
    pub datums: Vec<PublishedDatum>,
    /// SHA-256 of the design file this publish wrote, as SolidCraft keeps
    /// it on a linked design's link to the file (slice B5): a linked design whose
    /// link holds another hash holds an older board.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_sha256: Option<ContentHash>,
    /// The files earlier publishes wrote, oldest first, at most
    /// [`MAX_PUBLISHED_FILES`]: they name the revision a linked design still
    /// holds. Kept by `interface.record_publish`, never by the host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier: Vec<PublishedFile>,
}

/// Earlier published design files a record remembers.
pub const MAX_PUBLISHED_FILES: usize = 64;

/// A design file an earlier publish wrote: its revision and SHA-256.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublishedFile {
    pub revision: u32,
    pub sha256: ContentHash,
}

/// The board's interface: its slot table, rules and last publish. Slot
/// order is append-only; `datums` are kept in slot order.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InterfaceTable {
    /// The board's permanent interface identity, drawn by the host on the
    /// first publish (the core never generates randomness).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<InterfaceSourceId>,
    /// The next slot to allocate; every slot below it is live or retired.
    #[serde(default)]
    pub next_slot: u32,
    /// The next [`DatumId`] to allocate, from 1 (0 until the first). Like
    /// `next_slot`, it only grows.
    #[serde(default)]
    pub next_datum: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub datums: Vec<InterfaceDatum>,
    /// Slots whose datum was removed, ascending. Never allocated again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<Slot>,
    /// Automatic rules switched off, ascending (all are on by default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules_off: Vec<AutoRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<PublishedRecord>,
}

impl InterfaceTable {
    /// Nothing exported, allocated or published: what a project that never
    /// had an interface holds, and what its file leaves out.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The live datum with this id.
    pub fn datum(&self, id: DatumId) -> Option<&InterfaceDatum> {
        self.datums.iter().find(|d| d.id == id)
    }

    /// Whether the automatic rule is on (every rule is, by default).
    pub fn rule_on(&self, rule: AutoRule) -> bool {
        self.rules_off.binary_search(&rule).is_err()
    }
}
