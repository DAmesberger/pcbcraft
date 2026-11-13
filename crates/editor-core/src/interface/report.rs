//! What `interface.list` and `interface.publish_preview` answer
//! (docs/architecture/board-interface.md, "Commands" and "Link updates").
//! The preview's diff is slice B3's; its dry run is B4's.
use super::{DatumGeometry, DatumStatus, Vec3};
use crate::ids::{ContentHash, InterfaceSourceId};
use crate::library::datum::DatumKind;
use crate::model::interface::{AutoRule, DatumId, DatumSource, Slot, Tier};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One automatic rule and whether it is on.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct RuleState {
    pub rule: AutoRule,
    pub on: bool,
}

/// One exported datum as the Interface card and `interface.list` show it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DatumRow {
    pub id: DatumId,
    pub slot: Slot,
    /// Built from ids only (`i80.opening`).
    pub token: String,
    /// For people ("J1 opening"); never identity.
    pub name: String,
    /// The role as the sidecar writes it (`part.opening`, `hole`).
    pub role: String,
    pub kind: DatumKind,
    pub source: DatumSource,
    /// `None` when the datum is lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<DatumGeometry>,
    pub status: DatumStatus,
    /// Changed since the last published revision (or never published).
    pub changed: bool,
}

/// `interface.list`: the board's interface as it stands.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct InterfaceList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<InterfaceSourceId>,
    pub rules: Vec<RuleState>,
    /// In slot order.
    pub datums: Vec<DatumRow>,
    /// Retired slots, ascending.
    pub retired: Vec<Slot>,
    /// The last published revision, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<u32>,
    /// Datums added, changed or retired since then ("3 datums changed
    /// since revision 14").
    pub changed: usize,
}

/// How one datum changes from the last published revision to the
/// candidate.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum ChangeKind {
    Unchanged,
    /// Moved rigidly: by `offset` (mm, board frame) and turned by
    /// `turn_deg` about its own normal or axis.
    Moved {
        offset: Vec3,
        turn_deg: f64,
    },
    /// Its shape changed (a profile, diameter, height or value).
    Reshaped,
    /// Its content changed, but how is not known: the published sidecar
    /// was not there to compare with.
    Changed,
    Added,
    /// Its slot was retired.
    Retired,
    StatusChanged {
        from: DatumStatus,
        to: DatumStatus,
    },
}

/// One datum's row in the publish preview.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DatumChange {
    pub datum: DatumId,
    pub slot: Slot,
    /// The display name the preview reads ("J1 opening").
    pub name: String,
    #[serde(flatten)]
    pub change: ChangeKind,
    /// The sentence the user reads: "J1 opening, 2 mm along X".
    pub summary: String,
}

/// What a candidate publish would do to the linked enclosure's feature.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// The feature follows a moved datum.
    Moves,
    /// The feature would fail to evaluate.
    Fails,
    /// A joint would no longer solve.
    JointUnsolved,
    /// The feature references a face that is not exported (section 2 of
    /// the design), so it may move to another body when the board changes.
    UnexportedReference,
}

/// One enclosure feature the dry run reports, in the enclosure's own
/// names.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EnclosureEffect {
    /// The feature's name in SolidCraft ("USB opening").
    pub feature: String,
    pub effect: EffectKind,
    /// The datum it references, when it is one Sparksmith published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datum: Option<DatumId>,
    /// The sentence the user reads.
    pub message: String,
}

/// The dry run on a scratch copy of the linked enclosure (slice B4). The
/// user's enclosure file is never touched.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DryRun {
    /// The enclosure file the copy was made from, as the link names it.
    pub enclosure: String,
    pub effects: Vec<EnclosureEffect>,
}

/// `interface.publish_preview`: what publishing would change.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PublishPreview {
    /// The document revision the preview was made at; `interface.publish`
    /// takes it as `expected_revision`.
    pub document_revision: u64,
    /// The last published interface revision, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,
    /// The revision publishing would record: `from` + 1, or `from` itself
    /// when the hash is unchanged.
    pub to: u32,
    pub hash: ContentHash,
    pub tier: Tier,
    /// Every live or newly retired datum, in slot order.
    pub changes: Vec<DatumChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<DryRun>,
}
