//! Identity vocabulary. Every identity is a typed value; labels are display
//! text and never identity.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Project object identity (instances, symbol occurrences, wires, tracks and
/// definition lineages share one allocator) and net identity.
pub type Id = u64;

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
