//! `project.issues`: the one list of document findings that the Issues
//! surface, the CLI and MCP all show (docs/sidebar-ia.md).
//!
//! Core decides each finding's severity, its group and its wording, so every
//! transport shows the same titles. Presentation (folding, filters, Fix
//! launchers, locate) belongs to the host; it never changes a count, drops
//! an [`ObjectRef`] or loses the raw finding text.
//!
//! [`report`] collects from the existing rule owners (`completeness` for
//! drafts and instances, `erc`, `layout`), merges same-cause findings into
//! one row ([`IssueRow::also`]) and gives each row its severity, group and
//! title ([`rules`]).
mod merge;
mod names;
mod rules;
pub mod style;
#[cfg(test)]
mod tests;

pub use rules::{KNOWN_CODES, group_title};

use crate::completeness;
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::erc;
use crate::layout;
use crate::library::Drafts;
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The longest issue title, in characters. Titles are designed for about 56
/// characters at the smallest supported column width and clipped there.
pub const MAX_TITLE: usize = crate::diagnostics::MAX_LINE;

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
    /// Findings of the same cause merged into this row, unchanged: the
    /// other side of a pair (a package terminal without a pad beside the
    /// pad without a terminal), the other pads of one clearance pair or one
    /// net, or the other instances of one shared Part. Nothing is dropped:
    /// Copy report lists these too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<Finding>,
}

impl IssueRow {
    /// The stage this issue blocks, if any. An issue that blocks a stage is
    /// always [`Severity::Error`].
    pub fn blocks(&self) -> Option<Stage> {
        self.finding
            .blocks
            .or_else(|| self.also.iter().find_map(|f| f.blocks))
    }
    /// Every object the row refers to: the primary finding's, then the
    /// merged findings'.
    pub fn objects(&self) -> impl Iterator<Item = &ObjectRef> {
        std::iter::once(&self.finding)
            .chain(&self.also)
            .flat_map(|f| f.objects.iter())
    }
    /// The number of findings in this row.
    pub fn findings(&self) -> usize {
        1 + self.also.len()
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

/// Every issue of the project and its drafts in `scope`: the publish
/// findings and drawing lint of every draft (Component), the completeness,
/// purchasing and electrical findings of every instance (Schematic) and the
/// board checks (Board). Rows are uncapped and sorted most severe first,
/// grouped by [`IssueGroup`].
/// `settings` are the host's values for limits the project leaves open.
pub fn report(
    project: &Project,
    drafts: &Drafts,
    scope: &IssueScope,
    settings: &layout::CheckSettings,
) -> Result<IssueReport> {
    report_on(
        project,
        drafts,
        scope,
        &layout::CopperGraph::of(project),
        settings,
    )
}

/// [`report`] with `graph`, the copper graph of `project` (the engine's
/// per-revision one).
pub fn report_on(
    project: &Project,
    drafts: &Drafts,
    scope: &IssueScope,
    graph: &layout::CopperGraph,
    settings: &layout::CheckSettings,
) -> Result<IssueReport> {
    report_in(project, drafts, scope, false, graph, settings)
}

/// [`report`], keeping the findings waived for production when
/// `waived` (the production check marks them; Issues leaves them out,
/// as a waived finding is accepted and blocks nothing). A finding that
/// blocks a component stage stays: a waiver is for production only.
pub(crate) fn report_with(
    project: &Project,
    drafts: &Drafts,
    scope: &IssueScope,
    waived: bool,
    settings: &layout::CheckSettings,
) -> Result<IssueReport> {
    report_in(
        project,
        drafts,
        scope,
        waived,
        &layout::CopperGraph::of(project),
        settings,
    )
}

fn report_in(
    project: &Project,
    drafts: &Drafts,
    scope: &IssueScope,
    waived: bool,
    graph: &layout::CopperGraph,
    settings: &layout::CheckSettings,
) -> Result<IssueReport> {
    if let IssueScope::Selection { objects } = scope
        && objects.len() > MAX_SCOPE_OBJECTS
    {
        return Err(Error::Limit(format!(
            "An issue selection names at most {MAX_SCOPE_OBJECTS} objects."
        )));
    }
    let names = names::Names::new(project, drafts);
    let mut all = Vec::new();
    for (area, merged) in collect(project, drafts, graph, settings)? {
        all.push(row(&names, area, merged));
    }
    if !waived && !project.production.waivers.is_empty() {
        all.retain(|r| {
            r.blocks().is_some()
                || project
                    .production
                    .waiver(&crate::model::production::waiver_key(
                        &r.finding.code,
                        &r.finding.objects,
                    ))
                    .is_none()
        });
    }
    all.sort_by(|a, b| {
        (a.severity, a.area, &a.group.code, &a.group.key).cmp(&(
            b.severity,
            b.area,
            &b.group.code,
            &b.group.key,
        ))
    });
    let (rows, rest): (Vec<IssueRow>, Vec<IssueRow>) =
        all.into_iter().partition(|r| in_scope(r, scope));
    let mut elsewhere = Vec::new();
    if matches!(scope, IssueScope::Area { .. }) {
        for area in [IssueArea::Component, IssueArea::Schematic, IssueArea::Board] {
            let outside: Vec<&IssueRow> = rest.iter().filter(|r| r.area == area).collect();
            if !outside.is_empty() {
                elsewhere.push(AreaCount {
                    area,
                    counts: counts(outside.iter().copied()),
                    blocking: outside.iter().filter(|r| r.blocks().is_some()).count(),
                });
            }
        }
    }
    Ok(IssueReport {
        counts: counts(rows.iter()),
        rows,
        elsewhere,
        coverage: coverage(scope).into(),
    })
}

/// The user's name of each of `objects`, in order, as issue titles name
/// them ("R2 pad 1", "Track on N22", "Terminal ‘VOUT’"), capitalised. Hosts
/// show these as a row's object chips, so every transport names objects
/// with core's words and never with IDs.
pub fn object_names(project: &Project, drafts: &Drafts, objects: &[ObjectRef]) -> Vec<String> {
    let names = names::Names::new(project, drafts);
    objects
        .iter()
        .map(|object| crate::library::names::capitalised(&names.object(object)))
        .collect()
}

/// What the checks of a scope cover, for the footer.
fn coverage(scope: &IssueScope) -> &'static str {
    match scope {
        IssueScope::Area {
            area: IssueArea::Component,
        } => "Publish checks and drawing lint of every draft",
        IssueScope::Area {
            area: IssueArea::Schematic,
        } => "Completeness, purchasing and electrical rules",
        IssueScope::Area {
            area: IssueArea::Board,
        } => "Connectivity, outline, 0.2 mm clearance and courtyards",
        IssueScope::All | IssueScope::Selection { .. } => {
            "Publish checks, completeness, electrical rules and board checks"
        }
    }
}

fn counts<'a>(rows: impl Iterator<Item = &'a IssueRow>) -> IssueCounts {
    let mut counts = IssueCounts::default();
    for row in rows {
        let slot = match row.severity {
            Severity::Error => &mut counts.error,
            Severity::Warning => &mut counts.warning,
            Severity::ToDo => &mut counts.to_do,
            Severity::Info => &mut counts.info,
        };
        *slot = slot.saturating_add(1);
    }
    counts
}

/// One finding as an Issues row titles it: its severity and one line that
/// names objects (`project.issues`' wording, never the raw message).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FindingRow {
    pub severity: Severity,
    pub title: String,
    pub refs: Vec<ObjectRef>,
}

/// Each finding of `project` as an Issues row would title it, one row per
/// finding: the update review lists what an update adds and resolves in
/// the same words as Issues.
pub(crate) fn finding_rows(project: &Project, findings: &[Finding]) -> Vec<FindingRow> {
    let drafts = Drafts::default();
    let names = names::Names::new(project, &drafts);
    findings
        .iter()
        .map(|finding| FindingRow {
            severity: rules::severity(finding),
            title: rules::title(&names, &[finding]),
            refs: finding.objects.clone(),
        })
        .collect()
}

/// Whether a row is in scope. A selected instance also selects its pads,
/// terminals, units, properties and symbols; a selected definition its
/// local objects.
fn in_scope(row: &IssueRow, scope: &IssueScope) -> bool {
    match scope {
        IssueScope::All => true,
        IssueScope::Area { area } => row.area == *area,
        IssueScope::Selection { objects } => row
            .objects()
            .any(|object| objects.iter().any(|selected| covers(selected, object))),
    }
}

fn covers(selected: &ObjectRef, object: &ObjectRef) -> bool {
    if selected == object {
        return true;
    }
    match (*selected, *object) {
        (ObjectRef::Instance { id }, ObjectRef::Pad { instance, .. })
        | (ObjectRef::Instance { id }, ObjectRef::Unit { instance, .. })
        | (ObjectRef::Instance { id }, ObjectRef::Property { instance, .. }) => id == instance,
        (ObjectRef::Instance { id }, ObjectRef::Terminal { endpoint }) => id == endpoint.instance,
        (ObjectRef::Definition { id }, ObjectRef::LibraryObject { definition, .. }) => {
            id == definition
        }
        _ => false,
    }
}

/// One row from its findings, the first being the primary one.
fn row(names: &names::Names<'_>, area: IssueArea, merged: Vec<Finding>) -> IssueRow {
    let refs: Vec<&Finding> = merged.iter().collect();
    let title = rules::title(names, &refs);
    let mut findings = merged.into_iter();
    let finding = findings.next().unwrap_or_else(|| {
        Finding::new(
            rules::CHECK_FAILED,
            "An issue lost its finding".into(),
            Vec::new(),
        )
    });
    let also: Vec<Finding> = findings.collect();
    let severity = std::iter::once(&finding)
        .chain(&also)
        .map(rules::severity)
        .min()
        .unwrap_or(Severity::Info);
    let cause = rules::line_full(&finding.message);
    IssueRow {
        severity,
        area,
        group: IssueGroup {
            code: finding.code.clone(),
            key: String::new(),
            title: rules::group_title(&finding.code),
        },
        detail: IssueDetail {
            // A merged row's first message speaks for one finding only.
            cause: (also.is_empty() && cause != title).then_some(cause),
            fix: rules::fix(&finding.code).map(str::to_owned),
        },
        title,
        finding,
        also,
    }
}

/// Every finding of the project, by area, with same-cause findings merged
/// into one entry (primary first).
fn collect(
    project: &Project,
    drafts: &Drafts,
    graph: &layout::CopperGraph,
    settings: &layout::CheckSettings,
) -> Result<Vec<(IssueArea, Vec<Finding>)>> {
    let mut out = Vec::new();
    let mut component = Vec::new();
    for report in completeness::drafts(project, drafts)? {
        component.extend(report.findings);
    }
    out.extend(
        merge::component(&names::Names::new(project, drafts), component)
            .into_iter()
            .map(|m| (IssueArea::Component, m)),
    );
    let mut schematic = Vec::new();
    for instance in &project.instances {
        match completeness::instance(project, instance.id) {
            Ok(report) => {
                schematic.extend(report.findings);
                // A mechanical part kept out of the BOM is not purchased
                // through this design (docs/mechanical-parts.md M6).
                if !crate::library::mechanical::in_bom(&project.library, instance.component) {
                    continue;
                }
                if report.purchasing.state == completeness::PurchasingState::NoPart {
                    schematic.push(Finding::new(
                        rules::NO_PART,
                        format!("{} has no Part chosen.", instance.designator),
                        vec![ObjectRef::Instance { id: instance.id }],
                    ));
                }
                schematic.extend(report.purchasing.findings);
            }
            Err(error) => schematic.push(Finding::new(
                rules::CHECK_FAILED,
                format!("{} cannot be checked: {error}", instance.designator),
                vec![ObjectRef::Instance { id: instance.id }],
            )),
        }
    }
    schematic.extend(erc::checks(project));
    out.extend(
        merge::schematic(schematic)
            .into_iter()
            .map(|m| (IssueArea::Schematic, m)),
    );
    let mut board = layout::checks_with(project, graph, settings);
    board.extend(crate::interface::list::checks(project));
    board.extend(crate::interface::assembly_link::checks(project));
    out.extend(
        merge::board(board)
            .into_iter()
            .map(|m| (IssueArea::Board, m)),
    );
    Ok(out)
}
