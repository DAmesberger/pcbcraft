//! The `interface.*` commands (docs/architecture/board-interface.md,
//! "Commands"). Document changes are [`InterfaceCommand`]s: typed, one undo
//! step each, through the one engine, never opening a dialog; a refused
//! command changes nothing. Writing the exported files is an external
//! effect, so `interface.publish` and its preview are host requests
//! ([`PublishParams`], [`PublishPreviewParams`]) that read the document and,
//! after the files are written, record the publish with
//! [`InterfaceCommand::RecordPublish`].
//!
//! The host requests are served by the automation session (slice B3).
use crate::ids::{Id, InterfaceSourceId};
use crate::library::datum::DatumName;
use crate::library::footprint_source::ParamName;
use crate::model::dimension::BoardRef;
use crate::model::interface::{AutoRule, DatumId, PublishedRecord, Tier};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What `interface.export` exports. Everything the automatic rules export
/// can only be switched with `interface.auto`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "target", deny_unknown_fields)]
pub enum ExportTarget {
    /// Any dimensionable feature: a part face, edge or vertex, a pad, a
    /// board-sketch point or curve. Never a followed face.
    Feature { feature: BoardRef },
    /// A datum the instance's package revision declares (one not marked
    /// `external`, which the automatic rule already exports).
    PackageDatum { instance: Id, name: DatumName },
    /// A placement's frame.
    PartFrame { instance: Id },
    /// A placement's envelope: courtyard × body height.
    PartEnvelope { instance: Id },
    /// A board parameter, read-only on the other side.
    BoardParameter { name: ParamName },
}

/// Document changes to the board's interface. Each is one undo step; a
/// command that changes nothing is not.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum InterfaceCommand {
    /// Export a feature, package datum, part frame or envelope, or board
    /// parameter, allocating a new slot. Refused when it is already
    /// exported, when its owner does not exist, or at the slot bound.
    #[serde(rename = "interface.export")]
    Export {
        target: ExportTarget,
        /// A display name; the derived one ("J1 opening") when omitted.
        #[serde(default)]
        name: Option<String>,
    },
    /// Stop exporting a datum and retire its slot, which is never used
    /// again. An automatic datum is withdrawn by switching its rule off.
    /// The publish preview names the linked design features that used it.
    #[serde(rename = "interface.unexport")]
    Unexport { datum: DatumId },
    /// Set a datum's display name, or with `None` go back to the derived
    /// one. Never changes its token, slot or anything the linked design sees
    /// other than the label.
    #[serde(rename = "interface.rename")]
    Rename {
        datum: DatumId,
        #[serde(default)]
        name: Option<String>,
    },
    /// Switch an automatic export rule on or off. Off retires the slots of
    /// the datums it made; on again allocates new slots.
    #[serde(rename = "interface.auto")]
    Auto { rule: AutoRule, on: bool },
    /// Record a publish the host has written. It holds the revision, its
    /// hash, the tier and every datum's hash, and the board's interface
    /// identity on the first publish. Sent by the host after `interface.publish`
    /// wrote the files; refused when `source_id` differs from a recorded
    /// one or the revision does not follow the recorded one.
    #[serde(rename = "interface.record_publish")]
    RecordPublish {
        source_id: InterfaceSourceId,
        record: PublishedRecord,
    },
}

// A new command chooses its permission class here.
declare_entries!(InterfaceCommand in model3d {
    Export "interface.export" Edit "Export to the linked design",
    Unexport "interface.unexport" Edit "Stop exporting a datum",
    Rename "interface.rename" Edit "Rename an exported datum",
    Auto "interface.auto" Edit "Switch an automatic export",
    RecordPublish "interface.record_publish" Edit "Record a published interface",
});

/// `interface.publish_preview` (a host request; reads only): the diff
/// against the last published revision and, when a linked design is linked,
/// the dry run on a scratch copy of it (slices B3 and B4).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublishPreviewParams {
    /// The tier to preview; the last published one, or
    /// [`Tier::Mechanical`], when omitted.
    #[serde(default)]
    pub tier: Option<Tier>,
    /// The published `.solidcraft` design to compare with: the sidecar
    /// beside it says where each datum was, so a moved datum reads by how
    /// much. Without it, or when it is gone, a changed datum reads only as
    /// changed.
    #[serde(default)]
    pub path: Option<crate::permission::FilePath>,
    /// The linked design's file: with it (and `path`, the design it
    /// links), the preview runs the dry run on a scratch copy of it and
    /// names the linked design features that would move or fail (slice B4).
    #[serde(default)]
    pub assembly_path: Option<crate::permission::FilePath>,
}

/// `interface.publish` (a host request; Ask, because it writes files):
/// write the exported design and its sidecar atomically at `path`, then
/// record the publish. Refused, writing nothing, when the document is not
/// at `expected_revision`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublishParams {
    /// The `.solidcraft` file to write; the sidecar is written beside it
    /// as `<name>.interface.json`.
    pub path: crate::permission::FilePath,
    #[serde(default)]
    pub tier: Tier,
    /// The document revision the preview was made at.
    pub expected_revision: u64,
}
