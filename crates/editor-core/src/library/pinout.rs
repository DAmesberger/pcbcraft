//! Slice C3: typed pinout import. The agent (or the Pins
//! tab's Paste table) reads the datasheet and states every field
//! explicitly in a [`PinoutDocument`]; Pcbcraft never parses free text and
//! never splits "USART1_TX" into peripheral and role. `library.pinout_preview`
//! returns a [`PinoutPlan`] (changes, findings, review items);
//! `library.pinout_apply` applies it atomically as one undo step, refused
//! when the draft changed since the plan's `base`. The result stays a draft,
//! every imported row unreviewed (founder decision F8: publishing is
//! blocked while rows flagged `uncertain` remain unresolved).
//!
//! Rules arrive as Rhai pin-rule scripts (F2, slice C1), shown in the
//! review like any other row.
mod stage;
pub use stage::{StagedDraft, StagedImport, check_document, stage};

use super::{DefinitionMeta, Drafts, Library};
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::functions::{Direction, Drive, GpioRef, Pull, SignalKind, SourceSpan};
use crate::ids::{DefinitionId, PackageRowRef, TerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The only pinout document format this build reads.
pub const PINOUT_FORMAT: u32 = 1;
/// Pins one document lists.
pub const MAX_PINOUT_PINS: usize = 2048;
/// Peripherals one document lists.
pub const MAX_PINOUT_PERIPHERALS: usize = 256;
/// Package variants one document lists.
pub const MAX_PINOUT_VARIANTS: usize = 16;
/// Rules one document lists.
pub const MAX_PINOUT_RULES: usize = 1024;
/// Bytes of a free-text note.
pub const MAX_PINOUT_NOTE: usize = 1024;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutReference {
    pub title: String,
    #[serde(default)]
    pub document_revision: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutDevice {
    pub name: String,
    pub reference: PinoutReference,
}

/// One package variant and its terminal count (exposed pads included).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutVariant {
    pub name: String,
    #[serde(default)]
    pub package_row: Option<PackageRowRef>,
    pub terminal_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutRole {
    pub label: String,
    #[serde(default)]
    pub signal: Option<String>,
    #[serde(default = "one")]
    pub capacity: u16,
    #[serde(default)]
    pub required_when_used: bool,
}

fn one() -> u16 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutPeripheral {
    pub label: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub roles: Vec<PinoutRole>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutFunction {
    pub label: String,
    pub direction: Direction,
    pub signal: SignalKind,
    #[serde(default = "no_drive")]
    pub drive: Drive,
    /// Peripheral and role labels, stated explicitly.
    #[serde(default)]
    pub peripheral: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub mux: Option<String>,
}

fn no_drive() -> Drive {
    Drive::None
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PinoutClass {
    Electrical,
    DeviceNc,
    Reserved,
}

/// Declared options, by option name.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutOptions {
    #[serde(default)]
    pub pull: Option<Vec<Pull>>,
    #[serde(default)]
    pub speed: Option<Vec<String>>,
    #[serde(default)]
    pub initial_level: bool,
    #[serde(default)]
    pub schmitt: bool,
}

/// One pin row.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutPin {
    /// The document's own row key, stable across re-imports ("r12").
    pub row: String,
    /// Package designation per variant name ("QFN-48" → "12").
    pub number: BTreeMap<String, String>,
    pub name: String,
    pub class: PinoutClass,
    #[serde(default)]
    pub gpio: Option<GpioRef>,
    #[serde(default)]
    pub options: PinoutOptions,
    #[serde(default)]
    pub functions: Vec<PinoutFunction>,
    /// Label of the design-default function.
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub source: Option<SourceSpan>,
    /// The author is unsure; blocks publishing until resolved (F8).
    #[serde(default)]
    pub uncertain: bool,
    #[serde(default)]
    pub note: Option<String>,
}

/// One rule as a Rhai pin-rule script (C1).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutRule {
    pub label: String,
    pub script: String,
    #[serde(default)]
    pub source: Option<SourceSpan>,
}

/// The typed import document (`pinout.schema`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutDocument {
    /// Must be [`PINOUT_FORMAT`].
    pub format: u32,
    pub device: PinoutDevice,
    pub variants: Vec<PinoutVariant>,
    #[serde(default)]
    pub peripherals: Vec<PinoutPeripheral>,
    pub pins: Vec<PinoutPin>,
    #[serde(default)]
    pub rules: Vec<PinoutRule>,
}

/// What the import targets. Rows match the terminals of an existing
/// draft by package number, through the bindings of that component (the
/// first variant in document order that numbers the row and resolves);
/// a component without any binding matches by exact terminal name. Every
/// other row is a new terminal, and draft terminals no row matches are
/// removed; the plan lists each of these changes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
pub enum PinoutTarget {
    /// An existing component draft; terminals are kept by number.
    Draft { id: DefinitionId },
    /// New drafts: the component (designator prefix `U`), a block symbol
    /// with a pin per terminal, and per variant a package (terminals by
    /// the variant's numbers; the form of the variant's row, else of
    /// `row` for the first variant, else generic) and a binding mapping
    /// them by number. No footprint: lands come from a template
    /// (`library.footprint_from_template`) and are mapped through
    /// `library.assign_terminals`.
    New {
        name: String,
        #[serde(default)]
        row: Option<PackageRowRef>,
    },
}

/// The kind of one planned change.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Changed,
    Removed,
}

/// One planned change, by document row.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutChange {
    pub kind: ChangeKind,
    /// "terminal", "function", "peripheral", "rule", "package_terminal", "pad".
    pub object: String,
    #[serde(default)]
    pub row: Option<String>,
    #[serde(default)]
    pub terminal: Option<TerminalId>,
    pub summary: String,
}

/// Why a row needs a person's look.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    Uncertain,
    ContradictsDraft,
    VariantsDisagree,
    /// The draft has no binding, so the row kept an existing terminal's
    /// identity only because the names are equal: a person confirms it is
    /// the same pin.
    MatchedByName,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReviewItem {
    pub row: String,
    pub reason: ReviewReason,
    pub message: String,
}

/// `library.pinout_preview`'s result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutPlan {
    /// The draft revision token the plan was made against; `pinout_apply`
    /// refuses when it no longer matches.
    pub base: u64,
    pub changes: Vec<PinoutChange>,
    /// Every publish finding the result would have.
    pub findings: Vec<Finding>,
    pub review: Vec<ReviewItem>,
}

/// Which rows `pinout_apply` takes. `Except` lists pin row keys and
/// rule keys (`rule:<label>`); an excluded row of an existing draft keeps
/// what the draft had, and on a new component it is not imported.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "accept", content = "rows", rename_all = "snake_case")]
pub enum PinoutAccept {
    #[default]
    All,
    Except(Vec<String>),
}

/// What `pinout_apply` wrote.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutApplied {
    /// The drafts created or changed, component first.
    pub drafts: Vec<DefinitionId>,
    /// Imported rows, all unreviewed.
    pub unreviewed: Vec<String>,
}

/// A component's import review state (F8), part of the revision: rows an
/// import created that nobody confirmed, and rows flagged uncertain
/// (`pinout.uncertain` blocks publishing; `pinout.unreviewed` warns).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImportReview {
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub unreviewed: std::collections::BTreeSet<TerminalId>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub uncertain: std::collections::BTreeSet<TerminalId>,
}

impl ImportReview {
    pub fn is_empty(&self) -> bool {
        self.unreviewed.is_empty() && self.uncertain.is_empty()
    }
}

/// The `library.pinout_apply` request: the previewed target and
/// document, the plan's `base` and which rows to take.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinoutApply {
    pub target: PinoutTarget,
    pub document: PinoutDocument,
    pub base: u64,
    #[serde(default)]
    pub accept: PinoutAccept,
}

/// The planned drafts of an import.
pub type PinoutDrafts = Vec<(DefinitionMeta, super::Definition)>;

/// The `accept` key of a rule: this prefix and the rule's label
/// (`rule:uart0_pins`). Row keys never start with it.
pub const RULE_ROW_PREFIX: &str = "rule:";

/// EXL-48, an original invented 48-pin controller in two packages (QFN-48
/// with an exposed pad, and LQFP-48), with alternate functions,
/// peripherals and Rhai pin rules: the core fixture of import and review.
/// It describes no real device; the automation scenario uses its own
/// single-package EXM-48.
#[cfg(test)]
pub(crate) const EXL48: &str = include_str!("pinout/exl48.json");

/// The F8 gate over a component's review state: each terminal flagged
/// uncertain blocks publishing (`pinout.uncertain`); unreviewed imported
/// terminals are one warning (`pinout.unreviewed`). A re-import whose row
/// is no longer flagged clears its uncertainty; a person clears both
/// by editing the draft's `review`.
pub fn review_findings(component: DefinitionId, review: &ImportReview) -> Vec<Finding> {
    let at = |t: &TerminalId| ObjectRef::LibraryObject {
        definition: component,
        local: t.0,
    };
    let mut out: Vec<Finding> = review
        .uncertain
        .iter()
        .map(|t| {
            Finding::blocking(
                "pinout.uncertain",
                format!(
                    "Imported terminal {t} is flagged uncertain; check it against the datasheet and clear the flag before publishing."
                ),
                vec![at(t)],
                Stage::Publish,
            )
        })
        .collect();
    if !review.unreviewed.is_empty() {
        let count = review.unreviewed.len();
        out.push(Finding::new(
            "pinout.unreviewed",
            format!(
                "{count} imported terminal{} not been reviewed yet.",
                if count == 1 { " has" } else { "s have" }
            ),
            review.unreviewed.iter().map(at).collect(),
        ));
    }
    out
}

/// The token a plan is made against (`PinoutPlan::base`): a digest of the
/// target draft and every binding draft of it (they decide how rows match
/// terminals by number), or, for a new component, of the package rows it
/// names. `pinout_apply` refuses when the token changed.
pub fn base_token(library: &Library, drafts: &Drafts, target: &PinoutTarget) -> Result<u64> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    match target {
        PinoutTarget::Draft { id } => {
            let draft = drafts
                .get(*id)
                .ok_or_else(|| Error::NotFound("That draft no longer exists".into()))?;
            hasher.update(serde_json::to_vec(draft)?);
            for other in &drafts.items {
                if let super::Definition::Binding(b) = &other.definition
                    && b.component == *id
                {
                    hasher.update(serde_json::to_vec(other)?);
                }
            }
        }
        PinoutTarget::New { name, row } => {
            hasher.update(name.as_bytes());
            if let Some(r) = row {
                hasher.update(serde_json::to_vec(&library.package_row(*r))?);
            }
        }
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    for (b, d) in bytes.iter_mut().zip(digest.iter()) {
        *b = *d;
    }
    Ok(u64::from_le_bytes(bytes))
}

/// JSON schema of [`PinoutDocument`] (`pinout.schema`).
pub fn schema() -> serde_json::Value {
    serde_json::json!(schemars::schema_for!(PinoutDocument))
}
