//! The assembly link's stored data (docs/architecture/board-interface.md,
//! "Link updates"; docs/board-sketch-references.md §7): which linked design
//! the board is linked to, the linked design geometry the board follows
//! (followed faces), the linked design parameters board parameters take
//! (bindings), what the linked design uses of the board (consumers), and a
//! snapshot of all of it as last read, so the project opens, solves and
//! checks without the linked design file. Data only; the rules and the
//! `assembly.*` commands live in `crate::interface::assembly_link`, and reading
//! the linked design is the host's.
use crate::ids::ContentHash;
use crate::library::footprint_source::{ParamName, SketchRef};
use crate::model::dimension::{
    BodyName, FaceName, FeatureGeometry, FollowedFaceId, ReferenceStatus,
};
use crate::model::interface::{DatumId, PublishedDatum, Slot};
use crate::units::Nm;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Followed faces one link holds.
pub const MAX_FOLLOWED_FACES: usize = 256;
/// Parameter bindings one link holds.
pub const MAX_ASSEMBLY_BINDINGS: usize = 64;
/// Linked design features one link records as using the board's datums.
pub const MAX_CONSUMERS: usize = 4096;
/// Datums one followed face's linked design geometry is recorded as built
/// on, and loops the link records.
pub const MAX_BUILT_ON: usize = 256;
/// Bytes of the linked design's path as the project stores it.
pub const MAX_ASSEMBLY_PATH: usize = 4096;
/// Bytes of a linked design name (a feature, body, component or parameter
/// name) the link stores.
pub const MAX_ASSEMBLY_NAME: usize = 256;

/// The project's link to a linked design.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssemblyLink {
    /// Where the linked design was, as the user gave it (absolute, or
    /// relative to the project's folder). A hint for the desktop and for
    /// messages: every request that reads the linked design names its file, so
    /// the permission gate sees it, and is refused when it is not this one
    /// (`assembly.relink` changes it).
    pub assembly_hint: String,
    /// Where the board's published design was, which the assembly links
    /// and whose sidecar names the datums; a hint, like
    /// [`Self::assembly_hint`].
    pub design_hint: String,
    /// The linked design component that links this board's published design
    /// (matched by its link resolving to that file, never by name); its
    /// name in the linked design, for messages.
    pub component: String,
    /// SHA-256 of the linked design file's bytes when the board last read it:
    /// a different hash is "the linked design changed".
    pub file_sha256: ContentHash,
    /// The interface revision the assembly linked when it was read, if
    /// the published sidecar beside the linked design said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_revision: Option<u32>,
    /// Linked design geometry the board follows, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<FollowedFace>,
    /// Board parameters set from linked design parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<AssemblyBinding>,
    /// Linked design features that use the board's datums, as last read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<Consumer>,
    /// SHA-256 of the board's published design file as the linked design's
    /// link to it last read it (SolidCraft keeps it on the link), when
    /// the linked design keeps one: another hash than the last publish's is
    /// "the linked design holds an older board" (slice B5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_sha256: Option<ContentHash>,
    /// Linked design features attached to a face of the board's design that no
    /// slot carries (section 2: parts, laminate, pads), as last read. Such a
    /// face may move to another body when the board changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unexported: Vec<UnexportedUse>,
    /// Loops between the two files: a followed face whose linked design
    /// geometry is built on a datum that itself follows the reference on
    /// the board. Derived after every edit that can change them
    /// (`interface::assembly_link::follow_cycles`); a loop through
    /// positions blocks fabrication output, one through extents only is
    /// advisory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<AssemblyCycle>,
    /// The datums the link uses (its consumers' and what its followed
    /// faces are built on), with the content hash each had in the publish
    /// the linked design held when the two last agreed. A datum whose hash
    /// differs now is one the board changed since: where the linked
    /// design changed what depends on it too, the two conflict and an
    /// update asks which side wins (`assembly.record_update`'s `resolve`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agreed: Vec<PublishedDatum>,
    /// Next [`FollowedFaceId`]; ids are never reused.
    pub next_ref: u32,
}

/// How a linked design feature uses a datum.
#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum UseKind {
    /// A sketch projects or intersects the datum's geometry.
    #[default]
    Sketch,
    /// A joint places the board by it: the whole board frame in the
    /// linked design, and every followed face with it, depends on it.
    Joint,
    /// A sketch projects the datum only to shape a profile in its own
    /// plane, and the followed face is that sketch's plane: the datum sets
    /// the face's extent, never its position. Only a followed face's
    /// [`FollowedFace::built_on`] says this.
    Extent,
}

impl UseKind {
    fn is_sketch(&self) -> bool {
        *self == Self::Sketch
    }
}

/// A linked design feature attached to a face of the board's design that is
/// not exported (no slot carries it).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct UnexportedUse {
    /// The feature's name in the linked design ("Lid rib").
    pub feature: String,
    /// The board design's body it is attached to, by its name there
    /// ("pkg15r1 body (pkg15r1:2)").
    pub body: String,
    /// The part that body belongs to, from the published sidecar's
    /// [`PartBody`](crate::interface::sidecar::PartBody) list: SolidCraft
    /// names a part's body by its package, so this, not the body's name,
    /// says which part it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<crate::ids::Id>,
}

/// A loop between the two files: the board follows followed face
/// `reference`, whose linked design geometry is built on `datum` through the
/// linked design's `feature`, and `datum` moves when the reference does.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct AssemblyCycle {
    pub reference: FollowedFaceId,
    pub datum: DatumId,
    /// The linked design feature (or joint) that uses the datum, by its name
    /// there.
    pub feature: String,
    #[serde(default, skip_serializing_if = "UseKind::is_sketch")]
    pub via: UseKind,
}

/// One piece of linked design geometry the board follows (§7.2): a face of an
/// linked design body, by its persistent Solidcraft name and a pick point on
/// it, traced along the board's normal into the board plane. Its trace is
/// fixed geometry of the board sketch, owned by the reference
/// ([`Self::sketch`]), so board curves are dimensioned to it; only a
/// assembly update moves it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FollowedFace {
    pub id: FollowedFaceId,
    /// The linked design body, by its name there.
    pub body: BodyName,
    /// The face's persistent name in the linked design.
    pub face: FaceName,
    /// A point on the face, linked design frame, in nanometres; it moves with
    /// the face when an update finds the face elsewhere.
    pub pick: [Nm; 3],
    /// For people ("Far wall · face"); never identity.
    pub name: String,
    /// The snapshot: its trace in the board plane, board mm, as last read.
    /// A lost or inexact reference keeps the last geometry.
    pub geometry: FeatureGeometry,
    pub status: ReferenceStatus,
    /// Why it is inexact or lost, in the user's words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The board-sketch point or construction line that holds the trace,
    /// fixed and owned by this reference.
    pub sketch: SketchRef,
    /// The board's datums the linked design geometry under this face is built
    /// on, as last read: through the sketches its feature is made from,
    /// and every joint that places the board. A datum here that follows
    /// this reference on the board is a loop (`assembly.cycle`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub built_on: Vec<Consumer>,
}

/// A board parameter set from a linked design user parameter (§7.2). Units
/// are checked; names are never matched automatically.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssemblyBinding {
    pub board_param: ParamName,
    /// The linked design's parameter, by its name there.
    pub assembly_param: String,
    /// The snapshot: its value when last read.
    pub value: Nm,
}

/// A linked design feature that uses one of the board's published datums
/// (a sketch projecting a datum's face, a joint on it), as the board last
/// read the linked design. A consumer whose datum no longer resolves is a
/// broken reference (`assembly.reference_lost`), a warning that blocks
/// publishing to the linked design until it is resolved.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Consumer {
    /// The feature's name in the linked design ("USB opening").
    pub feature: String,
    /// The datum it uses.
    pub datum: DatumId,
    /// The slot that carries it in the published design: the linked design's
    /// link names a face or body of that slot. The board checks the
    /// consumer by its slot, which is never reused.
    pub slot: Slot,
    /// A sketch, or a joint that places the board.
    #[serde(default, skip_serializing_if = "UseKind::is_sketch")]
    pub via: UseKind,
    /// The datum's token as published (`i80.opening`).
    pub token: String,
    /// The datum's display name as published ("J1 opening").
    pub name: String,
}
