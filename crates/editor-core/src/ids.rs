//! Identity vocabulary. Every identity is a typed value; labels are display
//! text and never identity.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Project object identity (instances, symbol occurrences, wires, tracks and
/// definition lineages share one allocator) and net identity.
pub type Id = u64;

/// A schematic sheet, in its own allocation space (`Project.sheets.next`).
/// Every project has at least one sheet; a new project's is
/// [`SheetId::FIRST`].
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct SheetId(pub u32);

impl SheetId {
    /// The sheet a new project starts with. An object on it omits its
    /// `sheet` field in the file.
    pub const FIRST: Self = Self(1);
    pub fn is_first(&self) -> bool {
        *self == Self::FIRST
    }
}

impl Default for SheetId {
    fn default() -> Self {
        Self::FIRST
    }
}

impl fmt::Display for SheetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A definition lineage. Opaque so the representation can change when shared
/// library folders need collision-free identities.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
)]
#[serde(transparent)]
pub struct DefinitionId(pub Id);

impl fmt::Display for DefinitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "D{}", self.0)
    }
}

/// An immutable published revision of one lineage, starting at 1.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
)]
#[serde(transparent)]
pub struct Revision(pub u32);

impl Revision {
    pub const FIRST: Self = Self(1);
    /// The following revision, or `None` when the space is exhausted.
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

/// What a lineage defines. One lineage has exactly one kind.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionKind {
    Component,
    Symbol,
    Package,
    Footprint,
    Binding,
    /// A quantity kind: a unit set with its own identity.
    QuantityKind,
    /// A reusable typed field.
    Field,
    /// A manufacturer part: one exact orderable item pinning a component and
    /// a binding revision (S2).
    Part,
    /// A 3D model: an imported or parametric body shared by packages
    /// (slice D1). Contract only until D1: publishing one is refused.
    Model,
    /// A named dimension-table row of one package family (slice B1).
    /// Contract only until B1: publishing one is refused.
    PackageRow,
}

/// Any exact revision, tagged with its kind.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(deny_unknown_fields)]
pub struct DefinitionRef {
    pub kind: DefinitionKind,
    pub id: DefinitionId,
    pub revision: Revision,
}

macro_rules! exact_ref {
    ($(#[$doc:meta])* $name:ident, $kind:ident) => {
        $(#[$doc])*
        #[derive(
            Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq,
            PartialOrd, Ord, Hash,
        )]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            pub id: DefinitionId,
            pub revision: Revision,
        }
        impl $name {
            pub fn new(id: DefinitionId, revision: Revision) -> Self {
                Self { id, revision }
            }
            pub fn erased(self) -> DefinitionRef {
                DefinitionRef {
                    kind: DefinitionKind::$kind,
                    id: self.id,
                    revision: self.revision,
                }
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} {}", self.id, self.revision)
            }
        }
    };
}

exact_ref!(
    /// An exact component revision.
    ComponentRef,
    Component
);
exact_ref!(
    /// An exact symbol revision.
    SymbolRef,
    Symbol
);
exact_ref!(
    /// An exact package revision.
    PackageRef,
    Package
);
exact_ref!(
    /// An exact footprint revision.
    FootprintRef,
    Footprint
);
exact_ref!(
    /// An exact binding (implementation) revision.
    BindingRef,
    Binding
);
exact_ref!(
    /// An exact quantity-kind revision.
    QuantityKindRef,
    QuantityKind
);
exact_ref!(
    /// An exact field revision.
    FieldRef,
    Field
);
exact_ref!(
    /// An exact manufacturer-part revision.
    PartRef,
    Part
);
exact_ref!(
    /// An exact 3D-model revision (slice D1).
    ModelRef,
    Model
);
exact_ref!(
    /// An exact package-row revision (slice B1).
    PackageRowRef,
    PackageRow
);

macro_rules! local_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq,
            PartialOrd, Ord, Hash,
        )]
        #[serde(transparent)]
        pub struct $name(pub u32);
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

local_id!(
    /// A component's electrical terminal, stable across revisions.
    TerminalId
);
local_id!(
    /// A symbol unit, stable across revisions of one symbol lineage.
    UnitId
);
local_id!(
    /// A drawn primitive or text of one symbol (or footprint) lineage, stable
    /// across its revisions. Shares the lineage's local namespace.
    GraphicId
);
local_id!(
    /// A package's physical terminal.
    PackageTerminalId
);
local_id!(
    /// A footprint pad (copper land).
    PadId
);
local_id!(
    /// An enumeration choice of a field, stable across its revisions.
    ChoiceId
);
local_id!(
    /// One alternative function of a component terminal. Unique per component
    /// lineage, not per terminal.
    FunctionId
);
local_id!(
    /// A shared hardware resource of a component (a peripheral, a channel
    /// group) whose roles terminal functions take.
    ResourceId
);
local_id!(
    /// A role within one resource, with a capacity.
    RoleId
);
local_id!(
    /// A pin rule script of one component lineage (slice C1), stable
    /// across its revisions. Shares the lineage's local namespace.
    RuleId
);

/// One function of one terminal. A `FunctionId` is unique per component, so
/// the pair is redundant by design: every rule that takes a key checks that
/// the function belongs to the named terminal.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(deny_unknown_fields)]
pub struct FunctionKey {
    pub terminal: TerminalId,
    pub function: FunctionId,
}

/// Longest name, label or designation.
pub const MAX_LABEL: usize = 256;

/// Whether `text` is acceptable display text for a published name or label:
/// not blank, at most [`MAX_LABEL`] bytes and free of control characters.
/// Labels are presentation and never identity.
pub fn printable(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_LABEL && !text.chars().any(char::is_control)
}

/// One terminal of one component instance: the electrical identity wires,
/// nets and pads refer to.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub instance: Id,
    pub terminal: TerminalId,
}

/// Lower-case hex digits of `bytes`.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        text.push(char::from(DIGITS[usize::from(b >> 4)]));
        text.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    text
}

/// Exactly `N` bytes from `2 * N` lower-case hex digits.
fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    fn digit(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        }
    }
    let raw = text.as_bytes();
    if raw.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (slot, pair) in out.iter_mut().zip(raw.chunks_exact(2)) {
        let [high, low] = pair else { return None };
        *slot = (digit(*high)? << 4) | digit(*low)?;
    }
    Some(out)
}

macro_rules! random_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        ///
        /// Written as 32 lower-case hex digits. The core never generates one:
        /// `sparksmith-io` draws them from the operating system and passes them
        /// in as values.
        #[derive(
            Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq,
            PartialOrd, Ord, Hash,
        )]
        #[serde(try_from = "String", into = "String")]
        #[schemars(with = "String")]
        pub struct $name(pub u128);
        impl $name {
            pub fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(u128::from_be_bytes(bytes))
            }
            pub fn parse(text: &str) -> Option<Self> {
                unhex::<16>(text).map(Self::from_bytes)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&hex(&self.0.to_be_bytes()))
            }
        }
        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.to_string()
            }
        }
        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(text: String) -> std::result::Result<Self, String> {
                Self::parse(&text).ok_or_else(|| {
                    format!("{} must be 32 lower-case hex digits", stringify!($name))
                })
            }
        }
    };
}

random_id!(
    /// A library source's identity (S3, D1): random, generated once when the
    /// folder is created and stored in its manifest. Never a path or name.
    LibraryId
);
random_id!(
    /// A lineage's identity inside one library folder (S3, D1): random, so
    /// two git authors adding lineages never collide on a counter. Project
    /// copies get fresh project `DefinitionId`s on import.
    SourceLineageId
);
random_id!(
    /// A board's permanent identity in its published interface
    /// (docs/architecture/board-interface.md): random, drawn once and kept
    /// across every publish, so a linked design can tell an older revision of
    /// the same board from a different board. A deliberate slot
    /// compaction draws a new one (a new epoch).
    InterfaceSourceId
);

/// A SHA-256 digest, written as 64 lower-case hex digits. Names a revision
/// record's canonical content (`library::bundle::content_hash`) or an
/// asset's bytes (`library::bundle::asset_hash`).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn parse(text: &str) -> Option<Self> {
        unhex::<32>(text).map(Self)
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl From<ContentHash> for String {
    fn from(hash: ContentHash) -> String {
        hash.to_string()
    }
}

impl TryFrom<String> for ContentHash {
    type Error = String;
    fn try_from(text: String) -> std::result::Result<Self, String> {
        Self::parse(&text).ok_or_else(|| "a content hash must be 64 lower-case hex digits".into())
    }
}
