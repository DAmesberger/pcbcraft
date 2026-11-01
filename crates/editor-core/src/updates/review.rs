//! The structured review data of an [`UpdatePlan`](super::UpdatePlan): each
//! change by area, kind and the objects it refers to, with a one-line title
//! and the full detail; what the update does to the document's findings;
//! and the tracks the update cannot keep, with their net, layer and length.
//!
//! Titles follow the message style of `docs/sidebar-ia.md` (one line of at
//! most [`MAX_TITLE`] characters, user names, no IDs). The full detail keeps
//! the engine wording for Copy report and Messages. Hosts lay this out; they
//! never rebuild it from the text.
use crate::diagnostics::{Finding, ObjectRef};
use crate::ids::Id;
use crate::issues::MAX_TITLE;
use crate::model::Layer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a change shows. Definition changes that reach an instance belong
/// to the area that displays them: symbols to the schematic, pads to the
/// board, terminals, functions, fields and Parts to the component.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum ChangeArea {
    Component,
    Schematic,
    Nets,
    Board,
}

impl ChangeArea {
    /// The group header a review shows for this area.
    pub fn title(self) -> &'static str {
        match self {
            Self::Component => "Component",
            Self::Schematic => "Schematic",
            Self::Nets => "Nets",
            Self::Board => "PCB",
        }
    }
}

/// What a change does to its object.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Add,
    Remove,
    Change,
}

/// The kind of object a change is about, in the order a summary names
/// them.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum ChangeObject {
    Instance,
    Placement,
    Wire,
    Net,
    /// A symbol occurrence on the sheet.
    Symbol,
    Component,
    Unit,
    Pin,
    Terminal,
    Function,
    Field,
    Part,
    Package,
    Footprint,
    Pad,
}

impl ChangeObject {
    fn noun(self, count: usize) -> &'static str {
        let (one, many) = match self {
            Self::Instance => ("instance", "instances"),
            Self::Component => ("component", "components"),
            Self::Unit => ("unit", "units"),
            Self::Pin => ("pin", "pins"),
            Self::Terminal => ("terminal", "terminals"),
            Self::Function => ("function", "functions"),
            Self::Field => ("field", "fields"),
            Self::Part => ("Part", "Parts"),
            Self::Package => ("package", "packages"),
            Self::Footprint => ("footprint", "footprints"),
            Self::Pad => ("pad", "pads"),
            Self::Symbol => ("sheet symbol", "sheet symbols"),
            Self::Wire => ("wire", "wires"),
            Self::Net => ("net", "nets"),
            Self::Placement => ("placement", "placements"),
        };
        if count == 1 { one } else { many }
    }
}

/// One change an update makes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlanChange {
    pub area: ChangeArea,
    pub kind: ChangeKind,
    pub object: ChangeObject,
    /// Every object the change refers to, for locate.
    pub refs: Vec<ObjectRef>,
    /// One line naming objects by their user names, at most
    /// [`MAX_TITLE`] characters, without IDs.
    pub line: String,
    /// The full engine wording; also listed in `UpdatePlan::changes`.
    pub detail: String,
}

/// One blocker or warning of a plan.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlanLine {
    /// One line, at most [`MAX_TITLE`] characters, without IDs.
    pub line: String,
    /// The full engine wording; also listed in `UpdatePlan::blockers` or
    /// `UpdatePlan::warnings`.
    pub detail: String,
    pub refs: Vec<ObjectRef>,
}

/// What the update does to the board and electrical checks: findings the
/// candidate has that the current project has not, and the reverse. A
/// finding is the same when its rule code and objects agree, so a reworded
/// or re-measured finding is neither new nor resolved.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FindingsDiff {
    pub new: Vec<Finding>,
    pub resolved: Vec<Finding>,
    /// `new`, in the same order, as `project.issues` titles them on the
    /// updated project, with their severity.
    #[serde(default)]
    pub new_rows: Vec<FindingRow>,
    /// `resolved`, in the same order, titled on the current project.
    #[serde(default)]
    pub resolved_rows: Vec<FindingRow>,
    /// "4 new issues, 1 resolved", "No new issues", or why the findings
    /// were not checked.
    pub line: String,
}

pub use crate::issues::FindingRow;

/// Why a track must be removed for the update to apply.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictReason {
    /// The track's net loses connections, merges or disappears, so the
    /// copper would connect pads the net no longer joins.
    NetChanged,
    /// A changed pad of another net would touch or crowd the track.
    Clearance,
}

/// A track the update cannot keep. It is removed only when the apply
/// request confirms it; retained tracks are never moved to another net.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TrackConflict {
    pub track: Id,
    pub net: Id,
    pub net_name: String,
    pub layer: crate::model::CopperLayer,
    /// Centre-line length in millimetres.
    pub length_mm: f64,
    pub reason: ConflictReason,
    /// The pad of another net, for a clearance conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad: Option<ObjectRef>,
    /// "Track on N22 would touch R3 pad ‘1’ of another net".
    pub line: String,
    /// The acknowledgement that removes it: "Remove that track (Top, 4.2 mm)".
    pub ack: String,
}

/// `text` clipped to [`MAX_TITLE`] characters, ending in "…" when clipped,
/// and without a final full stop.
pub(crate) fn clip(text: &str) -> String {
    let text = text.trim().trim_end_matches('.');
    if text.chars().count() <= MAX_TITLE {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(MAX_TITLE - 1).collect();
    out.push('…');
    out
}

pub(crate) fn layer_label(layer: Layer) -> &'static str {
    match layer {
        Layer::Top => "Top",
        Layer::Bottom => "Bottom",
    }
}

/// A one-line summary of `changes` by kind and object, such as "Adds 6
/// placements, removes 1 wire"; objects named by several lines of the same
/// kind count once. "No changes" when empty. Hosts use it for a whole plan
/// and for one area's group.
pub fn summarize<'a>(changes: impl IntoIterator<Item = &'a PlanChange>) -> String {
    let mut counts: BTreeMap<(ChangeKind, ChangeObject), Vec<&[ObjectRef]>> = BTreeMap::new();
    for change in changes {
        let seen = counts.entry((change.kind, change.object)).or_default();
        if change.refs.is_empty() || !seen.contains(&change.refs.as_slice()) {
            seen.push(&change.refs);
        }
    }
    if counts.is_empty() {
        return "No changes".into();
    }
    let mut out = String::new();
    let mut last = None;
    for ((kind, object), refs) in counts {
        let verb = match (kind, last == Some(kind)) {
            (_, true) => "",
            (ChangeKind::Add, false) => "adds ",
            (ChangeKind::Remove, false) => "removes ",
            (ChangeKind::Change, false) => "changes ",
        };
        let part = format!("{verb}{} {}", refs.len(), object.noun(refs.len()));
        let separator = if out.is_empty() { "" } else { ", " };
        // Leave room for the "and more" tail.
        if out.chars().count() + separator.len() + part.chars().count() > MAX_TITLE - 9 {
            out.push_str(" and more");
            break;
        }
        out.push_str(separator);
        out.push_str(&part);
        last = Some(kind);
    }
    let mut chars = out.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Designators as a short list: "R17–R22" for a run of one prefix with
/// consecutive numbers, "R1, R4 and R9", or "R1, R4 and 5 more".
pub(crate) fn designator_list(names: &[String]) -> String {
    let split = |name: &str| {
        let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let (prefix, number) = name.split_at(name.len() - digits);
        number.parse::<u64>().ok().map(|n| (prefix.to_owned(), n))
    };
    let parsed: Option<Vec<(String, u64)>> = names.iter().map(|n| split(n)).collect();
    if let Some(parsed) = parsed
        && parsed.len() > 2
        && parsed
            .windows(2)
            .all(|w| w[0].0 == w[1].0 && w[1].1 == w[0].1 + 1)
        && let (Some(first), Some(last)) = (names.first(), names.last())
    {
        return format!("{first}–{last}");
    }
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] if names.len() <= 3 => format!("{} and {last}", init.join(", ")),
        _ => format!(
            "{} and {} more",
            names.get(..2).unwrap_or_default().join(", "),
            names.len() - 2
        ),
    }
}

/// The identity of a finding for [`findings_diff`]: its code and objects.
fn finding_key(finding: &Finding) -> (String, String) {
    (finding.code.clone(), format!("{:?}", finding.objects))
}

/// Findings of `after` that `before` lacks (`new`) and of `before` that
/// `after` lacks (`resolved`), counting repeats.
pub(crate) fn findings_diff(before: Vec<Finding>, after: Vec<Finding>) -> FindingsDiff {
    let mut remaining: BTreeMap<(String, String), usize> = BTreeMap::new();
    for finding in &before {
        *remaining.entry(finding_key(finding)).or_default() += 1;
    }
    let mut new = Vec::new();
    let mut kept: BTreeMap<(String, String), usize> = BTreeMap::new();
    for finding in after {
        let key = finding_key(&finding);
        match remaining.get_mut(&key) {
            Some(count) if *count > 0 => {
                *count -= 1;
                *kept.entry(key).or_default() += 1;
            }
            _ => new.push(finding),
        }
    }
    let mut resolved = Vec::new();
    for finding in before {
        let key = finding_key(&finding);
        match kept.get_mut(&key) {
            Some(count) if *count > 0 => *count -= 1,
            _ => resolved.push(finding),
        }
    }
    let line = match (new.len(), resolved.len()) {
        (0, 0) => "No new issues".to_owned(),
        (1, r) => format!("1 new issue, {r} resolved"),
        (n, r) => format!("{n} new issues, {r} resolved"),
    };
    FindingsDiff {
        new,
        resolved,
        new_rows: Vec::new(),
        resolved_rows: Vec::new(),
        line,
    }
}
