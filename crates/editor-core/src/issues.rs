//! `project.issues`: the one list of document findings that the Issues
//! surface, the CLI and MCP all show (docs/sidebar-ia.md).
//!
//! Core decides each finding's severity, its group and its wording, so every
//! transport shows the same titles. Presentation (folding, filters, Fix
//! launchers, locate) belongs to the host; it never changes a count, drops
//! an [`ObjectRef`] or loses the raw finding text.
//!
//! Slice S0 fixes the types and signatures only. [`report`] is not
//! implemented yet and refuses, so no transport can mistake an empty list for
//! "no issues". Slice C1 fills it from the existing rule owners
//! (`completeness`, `layout`, `erc`, the footprint and publish checks).
pub mod style;
#[cfg(test)]
mod tests;

use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::library::Drafts;
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The longest issue title, in characters. Titles are designed for about 56
/// characters at the smallest supported column width and clipped there.
pub const MAX_TITLE: usize = 80;

/// The most objects a `selection` scope may name.
pub const MAX_SCOPE_OBJECTS: usize = 4096;

/// How much an issue matters, most severe first. Core decides it; hosts only
/// map it to a glyph and ink.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Blocks a stage (publish, board placement, electrical resolution) or
    /// breaks a manufacturing or electrical rule. The only red severity.
    /// Anything that blocks publishing is always an error.
    Error,
    /// Lint, and purchasing or rating mismatches.
    Warning,
    /// Work that is expected to remain while designing: unrouted
    /// connections, unplaced instances, unresolved purchasing.
    ToDo,
    /// Information that needs no action.
    Info,
}

/// The part of the project an issue belongs to. Each desktop workspace shows
/// one area by default.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum IssueArea {
    /// Component drafts and published definitions (Component workspace).
    Component,
    /// Instances, wires, nets and purchasing (Schematic workspace).
    Schematic,
    /// Placements, copper and board rules (PCB workspace).
    Board,
}

/// Which issues a report lists. Counts in [`IssueReport::elsewhere`] always
/// cover the areas the scope leaves out.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IssueScope {
    /// Every area.
    All,
    /// One area, such as the active workspace's.
    Area { area: IssueArea },
    /// Issues that refer to at least one of these objects, at most
    /// [`MAX_SCOPE_OBJECTS`].
    Selection { objects: Vec<ObjectRef> },
}

/// The fold key of a row: rows with equal `code` and `key` share one group
/// header. `code` is the finding's rule code; `key` separates groups of one
/// rule (for example one net or one pair of placements).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
pub struct IssueGroup {
    pub code: String,
    pub key: String,
    /// The group header, such as "Clearance" or "Unrouted connections"; at
    /// most [`MAX_TITLE`] characters and in the [`style`] rules.
    pub title: String,
}

/// Details on demand under a row.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct IssueDetail {
    /// One sentence: why this is a problem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// An imperative: what resolves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// One issue: a finding with the severity, group and wording core gives it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct IssueRow {
    pub severity: Severity,
    pub area: IssueArea,
    pub group: IssueGroup,
    /// One line naming objects by their user names, at most [`MAX_TITLE`]
    /// characters, never IDs, codes or debug text ([`style::check`]).
    pub title: String,
    #[serde(default)]
    pub detail: IssueDetail,
    /// The underlying finding, unchanged: its code, raw message, every
    /// object it refers to (both sides of a same-cause pair) and the stage
    /// it blocks. Copy report and Messages show this text.
    pub finding: Finding,
}

impl IssueRow {
    /// The stage this issue blocks, if any. An issue that blocks a stage is
    /// always [`Severity::Error`].
    pub fn blocks(&self) -> Option<Stage> {
        self.finding.blocks
    }
}

/// Issue counts by severity.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct IssueCounts {
    pub error: usize,
    pub warning: usize,
    pub to_do: usize,
    pub info: usize,
}

/// Blocking issues in an area the scope did not list, so a blocker is never
/// out of sight ("Component: 2 block publishing").
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AreaCount {
    pub area: IssueArea,
    pub counts: IssueCounts,
    /// How many of those block a stage.
    pub blocking: usize,
}

/// The answer to `project.issues`: every issue in scope, uncapped, most
/// severe first and grouped by [`IssueGroup`].
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct IssueReport {
    pub rows: Vec<IssueRow>,
    /// The counts of `rows`.
    pub counts: IssueCounts,
    /// Areas outside the scope that hold issues.
    #[serde(default)]
    pub elsewhere: Vec<AreaCount>,
    /// What the checks cover, for the footer ("Connectivity, outline,
    /// 0.2 mm").
    #[serde(default)]
    pub coverage: String,
}

/// Every issue of the project and its drafts in `scope`.
///
/// Not implemented yet (slice C1): it refuses rather than report an empty,
/// and therefore wrong, list.
pub fn report(project: &Project, drafts: &Drafts, scope: &IssueScope) -> Result<IssueReport> {
    let _ = (project, drafts, scope);
    Err(Error::Unsupported(
        "The issue list is not available in this build yet".into(),
    ))
}
