//! The enclosure link's stored data (docs/architecture/board-interface.md,
//! "Link updates"; docs/board-sketch-references.md §7): which enclosure
//! the board is linked to, the enclosure geometry the board follows
//! (housing references), the enclosure parameters board parameters take
//! (bindings), what the enclosure uses of the board (consumers), and a
//! snapshot of all of it as last read, so the project opens, solves and
//! checks without the enclosure file. Data only; the rules and the
//! `housing.*` commands live in `crate::interface::housing`, and reading
//! the enclosure is the host's.
use crate::ids::ContentHash;
use crate::library::footprint_source::{ParamName, SketchRef};
use crate::model::dimension::{BodyName, FaceName, FeatureGeometry, HousingRefId, ReferenceStatus};
use crate::model::interface::DatumId;
use crate::units::Nm;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Housing references one link holds.
pub const MAX_HOUSING_REFS: usize = 256;
/// Parameter bindings one link holds.
pub const MAX_HOUSING_BINDINGS: usize = 64;
/// Enclosure features one link records as using the board's datums.
pub const MAX_CONSUMERS: usize = 4096;
/// Bytes of the enclosure's path as the project stores it.
pub const MAX_HOUSING_PATH: usize = 4096;
/// Bytes of an enclosure name (a feature, body, component or parameter
/// name) the link stores.
pub const MAX_HOUSING_NAME: usize = 256;

/// The project's link to an enclosure design.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HousingLink {
    /// Where the enclosure design was, as the user gave it (absolute, or
    /// relative to the project's folder). A hint for the desktop and for
    /// messages: every request that reads the enclosure names its file, so
    /// the permission gate sees it, and is refused when it is not this one
    /// (`housing.relink` changes it).
    pub enclosure_hint: String,
    /// Where the board's published design was, which the enclosure links
    /// and whose sidecar names the datums; a hint, like
    /// [`Self::enclosure_hint`].
    pub design_hint: String,
    /// The enclosure component that links this board's published design
    /// (matched by its link resolving to that file, never by name); its
    /// name in the enclosure, for messages.
    pub component: String,
    /// SHA-256 of the enclosure file's bytes when the board last read it:
    /// a different hash is "the enclosure changed".
    pub file_sha256: ContentHash,
    /// The interface revision the enclosure linked when it was read, if
    /// the published sidecar beside the linked design said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_revision: Option<u32>,
    /// Enclosure geometry the board follows, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<HousingRef>,
    /// Board parameters set from enclosure parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<HousingBinding>,
    /// Enclosure features that use the board's datums, as last read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<Consumer>,
    /// Next [`HousingRefId`]; ids are never reused.
    pub next_ref: u32,
}

/// One piece of enclosure geometry the board follows (§7.2): a face of an
/// enclosure body, by its persistent Solidcraft name and a pick point on
/// it, traced along the board's normal into the board plane. Its trace is
/// fixed geometry of the board sketch, owned by the reference
/// ([`Self::sketch`]), so board curves are dimensioned to it; only a
/// housing update moves it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HousingRef {
    pub id: HousingRefId,
    /// The enclosure body, by its name there.
    pub body: BodyName,
    /// The face's persistent name in the enclosure design.
    pub face: FaceName,
    /// A point on the face, enclosure frame, in nanometres; it moves with
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
}

/// A board parameter set from an enclosure user parameter (§7.2). Units
/// are checked; names are never matched automatically.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HousingBinding {
    pub board_param: ParamName,
    /// The enclosure's parameter, by its name there.
    pub housing_param: String,
    /// The snapshot: its value when last read.
    pub value: Nm,
}

/// An enclosure feature that uses one of the board's published datums
/// (a sketch projecting a datum's face, a joint on it), as the board last
/// read the enclosure. A consumer whose datum no longer resolves is a
/// broken reference (`housing.reference_lost`), which blocks fabrication
/// output.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Consumer {
    /// The feature's name in the enclosure ("USB opening").
    pub feature: String,
    /// The datum it uses.
    pub datum: DatumId,
    /// The datum's token as published (`i80.opening`).
    pub token: String,
    /// The datum's display name as published ("J1 opening").
    pub name: String,
}
