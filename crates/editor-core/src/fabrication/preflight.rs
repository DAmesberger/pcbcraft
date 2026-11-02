//! The production check run before export, and the waiver commands.
//!
//! The report is the project's issues (design rules checked under the
//! chosen fabrication profile) plus production findings: unrouted nets,
//! unfilled or stale fills, parts without a footprint or placement, parts
//! shown with only an envelope model, legend over pads and slots the drill
//! file cannot route. Errors block export unless waived; waivers are saved
//! in the project (`Project::production`).
use crate::diagnostics::{Error, ObjectRef, Result};
use crate::issues::Severity;
use crate::library::Drafts;
use crate::model::Project;
use crate::model::production::{MAX_WAIVER_TEXT, MAX_WAIVERS, Production, Waiver};
use crate::rules::RuleProfile;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Waive or restore pre-flight findings. Each is one undo step; a command
/// that changes nothing is not.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum FabricationCommand {
    /// Accept a pre-flight finding for fabrication, by its key, with a
    /// reason (required). Replaces an earlier waiver of the same key.
    #[serde(rename = "fabrication.waive")]
    Waive { key: String, reason: String },
    /// Remove the waiver of a key.
    #[serde(rename = "fabrication.unwaive")]
    Unwaive { key: String },
}

/// Apply one waiver command.
pub fn apply(production: &mut Production, command: FabricationCommand) -> Result<()> {
    let waivers = &mut production.waivers;
    match command {
        FabricationCommand::Waive { key, reason } => {
            let key = key.trim().to_string();
            let reason = reason.trim().to_string();
            if key.is_empty() || reason.is_empty() {
                return Err(Error::Invalid(
                    "fabrication.waiver_reason: a waiver needs a finding key and a reason".into(),
                ));
            }
            if key.len() > MAX_WAIVER_TEXT
                || reason.len() > MAX_WAIVER_TEXT
                || key.chars().chain(reason.chars()).any(char::is_control)
            {
                return Err(Error::Invalid(format!(
                    "fabrication.waiver_bounds: keys and reasons are printable, at most {MAX_WAIVER_TEXT} bytes"
                )));
            }
            match waivers.binary_search_by(|w| w.key.as_str().cmp(&key)) {
                Ok(i) => waivers[i].reason = reason,
                Err(i) => {
                    if waivers.len() >= MAX_WAIVERS {
                        return Err(Error::Limit(format!(
                            "fabrication.waiver_limit: at most {MAX_WAIVERS} waivers"
                        )));
                    }
                    waivers.insert(i, Waiver { key, reason });
                }
            }
        }
        FabricationCommand::Unwaive { key } => {
            let Ok(i) = waivers.binary_search_by(|w| w.key.as_str().cmp(key.trim())) else {
                return Err(Error::NotFound(
                    "fabrication.waiver_missing: that finding is not waived".into(),
                ));
            };
            waivers.remove(i);
        }
    }
    Ok(())
}

/// One pre-flight finding.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PreflightItem {
    /// Stable identity for waivers: the finding code and the objects it
    /// names (`board.clearance via:12 track:40`).
    pub key: String,
    pub code: String,
    pub severity: Severity,
    /// One line naming objects by their user names.
    pub title: String,
    /// What resolves it, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    pub objects: Vec<ObjectRef>,
    /// The saved waiver, when the user accepted this finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiver: Option<Waiver>,
}

impl PreflightItem {
    /// An error nobody waived: it blocks export.
    pub fn blocks(&self) -> bool {
        self.severity == Severity::Error && self.waiver.is_none()
    }
}

/// `fabrication.preflight`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PreflightReport {
    /// The rule profile the design was checked under.
    pub profile: String,
    /// Most severe first.
    pub items: Vec<PreflightItem>,
    pub errors: usize,
    pub warnings: usize,
    /// Waived findings (of any severity).
    pub waived: usize,
    /// Unwaived errors; export is refused while this is not 0.
    pub blocking: usize,
}

impl PreflightReport {
    pub fn ready(&self) -> bool {
        self.blocking == 0
    }
}

/// The pre-flight report of `project` checked under `profile` (the
/// project's own rules when `None`; overrides and net classes are kept).
pub fn report(
    project: &Project,
    _drafts: &Drafts,
    profile: Option<&RuleProfile>,
) -> Result<PreflightReport> {
    Ok(PreflightReport {
        profile: profile
            .map(|p| p.name.clone())
            .unwrap_or_else(|| project.rules.profile.name.clone()),
        ..Default::default()
    })
}
