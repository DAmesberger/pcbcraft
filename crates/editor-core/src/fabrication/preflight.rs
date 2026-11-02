//! The production check run before export, and the waiver commands.
//!
//! The report is the project's issues (design rules checked under the
//! chosen fabrication profile) plus production findings: unrouted nets,
//! unfilled or stale fills, parts without a footprint or placement, parts
//! shown with only an envelope model, legend over pads and slots the drill
//! file cannot route. Errors block export unless waived; waivers are saved
//! in the project (`Project::production`).
use crate::diagnostics::{Error, ObjectRef, Result};
use crate::geometry::{Bounds, Point, polyline_polygon_distance};
use crate::ids::{DefinitionId, Id};
use crate::issues::{IssueArea, IssueRow, IssueScope, Severity};
use crate::layout::{self, BoardDrawing, PlacedItem};
use crate::library::{Drafts, MaskRule, PackageModel};
use crate::model::board::CutoutKind;
use crate::model::production::{MAX_WAIVER_TEXT, MAX_WAIVERS, Production, Waiver};
use crate::model::{Layer, Project};
use crate::rules::{self, RuleProfile, RulesCommand};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
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

/// Silkscreen-over-pad comparisons per report (a target bound; beyond it
/// the check stops and says so).
pub const MAX_SILK_PAIRS: usize = 4_000_000;

/// The waiver key of a finding: its code and the objects it names.
pub fn key(code: &str, objects: &[ObjectRef]) -> String {
    let objects = serde_json::to_string(objects).unwrap_or_default();
    format!("{code} {objects}")
}

/// Pre-flight severity of an issue row: unrouted connections, unplaced
/// parts, stale fills and an unsolved outline block production;
/// component-area findings (drafts and library definitions) and to-dos
/// (such as parts without a purchasable Part) are warnings; information
/// is left out.
fn severity(row: &IssueRow) -> Option<Severity> {
    let code = row.finding.code.as_str();
    if matches!(
        code,
        "board.unrouted" | "board.unplaced" | "fill.stale" | "board.shape_unsolved"
    ) {
        return Some(Severity::Error);
    }
    match (row.area, row.severity) {
        (_, Severity::Info) => None,
        (IssueArea::Component, Severity::Error) | (_, Severity::ToDo) => Some(Severity::Warning),
        (_, s) => Some(s),
    }
}

/// The pre-flight report of `project` checked under `profile` (the
/// project's own rules when `None`; overrides and net classes are kept).
/// Fill copper computed under other rules is reported stale: refill under
/// the fabrication profile before export.
pub fn report(
    project: &Project,
    drafts: &Drafts,
    profile: Option<&RuleProfile>,
) -> Result<PreflightReport> {
    let mut items = Vec::new();
    let checked: Cow<'_, Project> = match profile {
        Some(p) if *p != project.rules.profile => {
            let mut candidate = project.clone();
            match rules::apply(
                &mut candidate.rules,
                RulesCommand::SetProfile {
                    profile: Box::new(p.clone()),
                    keep_overrides: true,
                },
            ) {
                Ok(_) => Cow::Owned(candidate),
                Err(e) => {
                    items.push(item(
                        "fabrication.profile_refused",
                        Severity::Error,
                        format!("The design cannot use {}: {}", p.name, e),
                        Some("Clear the overrides that conflict with the profile".into()),
                        Vec::new(),
                    ));
                    Cow::Borrowed(project)
                }
            }
        }
        _ => Cow::Borrowed(project),
    };
    let p: &Project = &checked;
    let issues = crate::issues::report(p, drafts, &IssueScope::All)?;
    for row in &issues.rows {
        let Some(severity) = severity(row) else {
            continue;
        };
        items.push(item(
            &row.finding.code,
            severity,
            row.title.clone(),
            row.detail.fix.clone(),
            row.finding.objects.clone(),
        ));
    }
    production_findings(p, drafts, &mut items);
    for i in &mut items {
        i.waiver = project.production.waiver(&i.key).cloned();
    }
    items.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.code.cmp(&b.code))
            .then_with(|| a.key.cmp(&b.key))
    });
    items.dedup_by(|a, b| a.key == b.key);
    let count = |s: Severity| items.iter().filter(|i| i.severity == s).count();
    Ok(PreflightReport {
        profile: p.rules.profile.name.clone(),
        errors: count(Severity::Error),
        warnings: count(Severity::Warning),
        waived: items.iter().filter(|i| i.waiver.is_some()).count(),
        blocking: items.iter().filter(|i| i.blocks()).count(),
        items,
    })
}

fn item(
    code: &str,
    severity: Severity,
    title: String,
    fix: Option<String>,
    objects: Vec<ObjectRef>,
) -> PreflightItem {
    PreflightItem {
        key: key(code, &objects),
        code: code.into(),
        severity,
        title,
        fix,
        objects,
        waiver: None,
    }
}

/// Findings only production needs: parts without a footprint, fills never
/// computed, parts with only an envelope 3D model, unpublished draft
/// changes of definitions in use, plated slots the drill file cannot route
/// and legend ink over pad openings.
fn production_findings(project: &Project, drafts: &Drafts, items: &mut Vec<PreflightItem>) {
    // Drafts whose content differs from the revision they edit (an open
    // draft identical to its base is no pending change).
    let drafted: BTreeSet<DefinitionId> = drafts
        .items
        .iter()
        .filter(|d| {
            let base = d.base.and_then(|revision| {
                project.library.definition(crate::ids::DefinitionRef {
                    kind: d.definition.kind(),
                    id: d.id,
                    revision,
                })
            });
            base.is_none_or(|(meta, definition)| meta != d.meta || definition != d.definition)
        })
        .map(|d| d.id)
        .collect();
    for instance in &project.instances {
        let objects = vec![ObjectRef::Instance { id: instance.id }];
        let resolved = crate::instances::resolve(&project.library, instance).ok();
        match resolved.as_ref().and_then(|r| r.physical) {
            None => items.push(item(
                "fabrication.no_footprint",
                Severity::Error,
                format!("{} has no footprint", instance.designator),
                Some("Choose a footprint for the part".into()),
                objects,
            )),
            Some(physical) => {
                if matches!(
                    physical.package.definition.model,
                    PackageModel::Envelope { .. }
                ) {
                    items.push(item(
                        "fabrication.envelope_model",
                        Severity::Warning,
                        format!("{} has only an envelope 3D model", instance.designator),
                        Some("Generate or import a 3D model for the package".into()),
                        objects.clone(),
                    ));
                }
                let lineages = [
                    instance.component.id,
                    physical.footprint.id,
                    physical.binding.id,
                    physical.package.id,
                ];
                if lineages.iter().any(|l| drafted.contains(l)) {
                    items.push(item(
                        "fabrication.draft_pending",
                        Severity::Warning,
                        format!(
                            "{} uses a definition with unpublished draft changes",
                            instance.designator
                        ),
                        Some("Review and publish the draft, or discard it".into()),
                        objects,
                    ));
                }
            }
        }
    }
    for fill in &project.fills {
        if fill.computed.is_none() {
            items.push(item(
                "fabrication.fill_unfilled",
                Severity::Error,
                format!("Fill {} has never been filled", fill.name),
                Some("Refill the fills".into()),
                Vec::new(),
            ));
        }
    }
    for cutout in &project.board().cutouts {
        if cutout.kind == (CutoutKind::Slot { plated: true })
            && super::drill::stadium(&cutout.outline).is_none()
        {
            items.push(item(
                &format!("fabrication.slot_unroutable.{}", cutout.id.0),
                Severity::Error,
                "A plated board slot is not a straight slot the drill file can route".into(),
                Some("Draw the plated slot as a straight slot with round ends".into()),
                Vec::new(),
            ));
        }
    }
    silk_over_pads(project, items);
}

/// Legend strokes that reach into a solder-mask opening of a pad on the
/// same side: the fabricator clips that ink.
fn silk_over_pads(project: &Project, items: &mut Vec<PreflightItem>) {
    let rules = project.rules.effective();
    let tol = 0.01;
    let mut openings: Vec<(Layer, Vec<Point>, Bounds)> = Vec::new();
    for pad in layout::placed_pads(project) {
        let by = match pad.pad.mask {
            MaskRule::Inherit => rules.mask.expansion,
            MaskRule::Tented => continue,
            MaskRule::Expansion(e) => e,
        };
        let Some(shape) = pad.pad.shape.grown(by, tol) else {
            continue;
        };
        let ring = shape.transformed(&pad.pad_transform()).flatten(tol);
        let Some(bounds) = Bounds::of(ring.iter().copied()) else {
            continue;
        };
        if pad.pad.kind.through_hole() {
            openings.push((Layer::Top, ring.clone(), bounds));
            openings.push((Layer::Bottom, ring, bounds));
        } else {
            openings.push((pad.layer, ring, bounds));
        }
    }
    let mut flagged: BTreeSet<(Id, Layer)> = BTreeSet::new();
    let mut pairs = 0usize;
    for graphic in layout::placed_graphics(project) {
        let BoardDrawing::Silkscreen(side) = graphic.layer else {
            continue;
        };
        if flagged.contains(&(graphic.instance, side)) {
            continue;
        }
        let strokes: Vec<(Vec<Point>, f64)> = match &graphic.item {
            PlacedItem::Shape { shape, width, .. } => vec![(shape.flatten(tol), *width)],
            PlacedItem::Text(text) => {
                let s = layout::legend::text_strokes(text);
                s.strokes.into_iter().map(|p| (p, s.width)).collect()
            }
        };
        'strokes: for (points, width) in strokes {
            let Some(bounds) = Bounds::of(points.iter().copied()) else {
                continue;
            };
            let bounds = bounds.inflated(width / 2.0);
            for (s, ring, b) in &openings {
                if *s != side || !b.intersects(&bounds) {
                    continue;
                }
                pairs += 1;
                if pairs > MAX_SILK_PAIRS {
                    items.push(item(
                        "fabrication.silk_check_limit",
                        Severity::Warning,
                        "The legend-over-pad check stopped at its limit".into(),
                        None,
                        Vec::new(),
                    ));
                    return;
                }
                let path: Vec<Point> = if points.len() == 1 {
                    vec![points[0], points[0]]
                } else {
                    points.clone()
                };
                let hits = polyline_polygon_distance(&path, ring) < width / 2.0;
                if hits {
                    flagged.insert((graphic.instance, side));
                    break 'strokes;
                }
            }
        }
    }
    let names: BTreeMap<Id, &str> = project
        .instances
        .iter()
        .map(|i| (i.id, i.designator.as_str()))
        .collect();
    for (instance, side) in flagged {
        items.push(item(
            "fabrication.silk_over_pad",
            Severity::Warning,
            format!(
                "Legend of {} runs into a pad opening on the {} side",
                names.get(&instance).copied().unwrap_or("a part"),
                match side {
                    Layer::Top => "top",
                    Layer::Bottom => "bottom",
                }
            ),
            Some("Move the legend clear of the pads; the fabricator clips ink in openings".into()),
            vec![ObjectRef::Instance { id: instance }],
        ));
    }
}
