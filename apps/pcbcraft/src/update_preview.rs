//! Revision-bound impact review shared by all editors (Update PCB, update
//! instances, connect, disconnect, remove), docked in the subject slot on
//! the kit's [`ReviewPanel`] (slice R1 of docs/sidebar-ia.md, "Dialogs and
//! reviews").
//!
//! The panel reads slice C2's structured plan: its summary line first, then
//! the blockers (conflicting tracks with their acknowledgement), then
//! collapsed change groups by area and what the update does to the
//! document's findings. Apply is gated until the preview is current and
//! every blocker is cleared. The canvas keeps showing the document with the
//! objects the update touches outlined ([`Marks`], [`paint_board`]); while
//! the review is open the editors ignore canvas edits. Like its library
//! consumers, a delayed action belongs to its original document.
use crate::kit::review::{ReviewAction, ReviewGroup, ReviewLine, ReviewPanel};
use crate::kit::severity::Level;
use crate::kit::toast::{ToastAction, post};
use crate::refusal::{self, Origin};
use egui::{Pos2, Stroke};
use pcbcraft_core::{
    Engine, Finding, Id, ObjectRef, Operation, Project,
    geometry::Point,
    layout,
    updates::{
        ApplyUpdate, Change, ChangeArea, ChangeKind, ChangeObject, InstanceChange, UpdatePlan,
        summarize,
    },
};
use solidcraft_ui_egui::theme::Tokens;
use std::collections::BTreeSet;

/// One shown title, at most this many characters.
const MAX_LINE: usize = pcbcraft_core::issues::MAX_TITLE;

#[derive(Default)]
pub struct UpdatePreview {
    plan: Option<UpdatePlan>,
    /// The acknowledgement of each blocker line, by index.
    acked: Vec<bool>,
    /// A component-revision review that lets the user pick instances: each
    /// candidate with its designator and whether it moves.
    choices: Vec<(Id, String, bool)>,
    /// Said above the changes (what already happened before the review).
    note: Option<String>,
    /// How the last review closed, for the host to act on once.
    closed: Option<Closed>,
    /// The objects the last "Show" named, for the canvas to emphasise.
    focus: Vec<ObjectRef>,
    /// Apply was sent at this revision on this frame; the next frame tells
    /// whether the engine took it.
    applying: Option<(u64, u64)>,
    /// The engine's refusal of the last Apply, in shown wording.
    refusal: Option<String>,
}

/// How a review closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    Applied,
    Cancelled,
}

/// What the panel shows for a plan, built from the plan alone so CLI-equal
/// data drives it: the summary, the blocker lines with the objects each
/// names, the change groups and the full report for Copy report.
#[derive(Clone, Debug, PartialEq)]
pub struct Content {
    pub title: String,
    pub summary: String,
    pub lines: Vec<ReviewLine>,
    /// The objects of `lines[n]`, for Show.
    pub refs: Vec<Vec<ObjectRef>>,
    /// Which of `lines` are conflicting tracks and vias: their
    /// acknowledgements together confirm removing them.
    pub conflicts: Vec<usize>,
    /// The errors applying leaves behind, with their objects in
    /// `after_refs`.
    pub after: Vec<ReviewLine>,
    pub after_refs: Vec<Vec<ObjectRef>>,
    pub groups: Vec<ReviewGroup>,
    pub report: String,
    /// The primary's label ("Apply update", "Remove R1").
    pub apply: String,
    /// The primary removes work.
    pub destructive: bool,
}

/// The objects the canvas outlines while a review is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    /// Instances whose placement, pads or definition the update changes.
    pub changed: BTreeSet<Id>,
    /// Instances (and so their placements) the update removes.
    pub removed: BTreeSet<Id>,
    /// Tracks the update removes once confirmed.
    pub tracks: BTreeSet<Id>,
    /// Placed instances that new findings of the update name (an unrouted
    /// pad, a clearance), outlined as a warning.
    pub issues: BTreeSet<Id>,
    /// What the last "Show" named.
    pub focus: Vec<ObjectRef>,
    /// The placements the update adds or moves, as they will be: drawn as
    /// ghosts so the canvas previews the result.
    pub ghosts: Vec<pcbcraft_core::Placement>,
}

impl UpdatePreview {
    /// The panel's identity: its refusal origin and widget state.
    fn id() -> egui::Id {
        egui::Id::new("update-review")
    }

    pub fn begin(&mut self, engine: &Engine, change: Change) {
        self.set_plan(engine.preview_update(change));
        self.choices.clear();
        self.note = None;
        self.applying = None;
        self.refusal = None;
    }

    /// [`Self::begin`] for a component-revision change, listing each of its
    /// instances with a checkbox (all on), and `note` above the changes.
    pub fn begin_choosing(&mut self, engine: &Engine, change: Change, note: Option<String>) {
        let choices = match &change {
            Change::ComponentRevision { instances, .. } => instances
                .iter()
                .map(|id| {
                    let designator = engine
                        .project()
                        .instance(*id)
                        .map_or_else(|| "Removed instance".to_owned(), |i| i.designator.clone());
                    (*id, designator, true)
                })
                .collect(),
            _ => Vec::new(),
        };
        self.begin(engine, change);
        self.choices = choices;
        self.note = note;
    }

    /// A new plan: acknowledgements and the located objects start over.
    fn set_plan(&mut self, plan: UpdatePlan) {
        self.plan = Some(plan);
        self.acked.clear();
        self.focus.clear();
    }

    /// How the last review closed, once.
    pub fn take_closed(&mut self) -> Option<Closed> {
        self.closed.take()
    }

    /// The plan again for the checked instances.
    fn rechoose(&mut self, engine: &Engine) {
        let Some(plan) = &self.plan else {
            return;
        };
        let Change::ComponentRevision {
            target,
            resolutions,
            ..
        } = &plan.change
        else {
            return;
        };
        let change = Change::ComponentRevision {
            instances: self
                .choices
                .iter()
                .filter(|(_, _, on)| *on)
                .map(|(id, _, _)| *id)
                .collect(),
            target: *target,
            resolutions: resolutions.clone(),
        };
        self.set_plan(engine.preview_update(change));
    }

    /// The plan again at the current revision, keeping the instance choice
    /// and the note.
    fn recompute(&mut self, engine: &Engine) {
        if !self.choices.is_empty() {
            self.rechoose(engine);
        } else if let Some(plan) = &self.plan {
            let change = plan.change.clone();
            self.set_plan(engine.preview_update(change));
        }
    }

    pub fn is_open(&self) -> bool {
        self.plan.is_some()
    }

    /// The change under review.
    #[cfg(test)]
    pub fn change(&self) -> Option<&Change> {
        self.plan.as_ref().map(|p| &p.change)
    }

    /// What the canvas outlines, while a review is open.
    pub fn marks(&self) -> Option<Marks> {
        let plan = self.plan.as_ref()?;
        let mut marks = marks(plan);
        marks.focus.clone_from(&self.focus);
        Some(marks)
    }

    fn close(&mut self, closed: Closed) {
        self.plan = None;
        self.acked.clear();
        self.choices.clear();
        self.note = None;
        self.focus.clear();
        self.applying = None;
        self.refusal = None;
        self.closed = Some(closed);
    }

    /// The frame after Apply: the engine either took the update (the
    /// revision moved), or handed its refusal back to this review.
    fn settle(&mut self, ctx: &egui::Context, engine: &Engine) {
        let Some((revision, frame)) = self.applying else {
            return;
        };
        if ctx.cumulative_frame_nr() <= frame {
            return;
        }
        self.applying = None;
        if let Some(refused) = refusal::claim(ctx, Origin::Dialog(Self::id())) {
            self.refusal = Some(refused);
            return;
        }
        if engine.revision() == revision {
            // Not sent (a file operation was running): stay open.
            return;
        }
        if let Some(plan) = &self.plan {
            post(ctx, Level::Ok, applied_line(plan), vec![ToastAction::Undo]);
            let new = plan.findings.new.len();
            if new > 0 {
                post(
                    ctx,
                    Level::Warning,
                    new_issues_line(new),
                    vec![ToastAction::Show],
                );
            }
        }
        self.close(Closed::Applied);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        self.settle(&ui.ctx().clone(), engine);
        let plan = self.plan.as_ref()?;
        let content = content(plan, engine.project(), self.note.as_deref(), &self.choices);
        self.acked.resize(content.lines.len(), false);
        let stale = plan.revision != engine.revision();
        let panel = ReviewPanel {
            id: Self::id(),
            icon: icon(&plan.change),
            title: &content.title,
            summary: &content.summary,
            stale,
            blockers: &content.lines,
            after: &content.after,
            groups: &content.groups,
            apply: &content.apply,
            destructive: content.destructive,
        };
        let mut chose = false;
        let choices = &mut self.choices;
        let action = panel.show_with(ui, &mut self.acked, self.refusal.as_deref(), |ui| {
            if choices.is_empty() {
                return;
            }
            let count = choices.len();
            crate::kit::section(
                ui,
                "update-instances",
                "Instances",
                true,
                Some(count),
                |ui| {
                    for (_, designator, on) in choices.iter_mut() {
                        chose |= crate::kit::check_row(ui, designator, on);
                    }
                    crate::kit::hint(
                        ui,
                        "Only the checked instances move",
                        "The others keep their revision.",
                    );
                },
            );
        });
        if chose {
            self.refusal = None;
            self.rechoose(engine);
            return None;
        }
        match action? {
            ReviewAction::Cancel => {
                self.close(Closed::Cancelled);
                None
            }
            ReviewAction::Recompute => {
                self.refusal = None;
                self.recompute(engine);
                None
            }
            ReviewAction::CopyReport => {
                ui.ctx().copy_text(content.report);
                None
            }
            ReviewAction::Acknowledge(_) => {
                self.refusal = None;
                None
            }
            ReviewAction::Locate(n) => {
                self.focus = content.refs.get(n).cloned().unwrap_or_default();
                None
            }
            ReviewAction::LocateAfter(n) => {
                self.focus = content.after_refs.get(n).cloned().unwrap_or_default();
                None
            }
            ReviewAction::Apply => {
                // The panel gates Apply; check again, it costs nothing.
                if stale
                    || crate::kit::review::apply_gate(stale, &content.lines, &self.acked).is_some()
                {
                    return None;
                }
                let remove = !content.conflicts.is_empty()
                    && content
                        .conflicts
                        .iter()
                        .all(|n| self.acked.get(*n).copied().unwrap_or(false));
                let ctx = ui.ctx();
                refusal::mark(ctx, Origin::Dialog(Self::id()));
                self.refusal = None;
                self.applying = Some((engine.revision(), ctx.cumulative_frame_nr()));
                let plan = self.plan.as_ref()?;
                Some(Operation::Update(Box::new(ApplyUpdate {
                    expected_revision: plan.revision,
                    change: plan.change.clone(),
                    remove_conflicting_tracks: remove,
                })))
            }
        }
    }
}

/// The one acknowledged line of a plan's conflicting vias: "2 vias on GND
/// conflict with this update", acknowledged by "Remove those vias".
fn via_conflict_line(plan: &UpdatePlan, project: &Project) -> ReviewLine {
    let count = plan.conflicting_vias.len();
    let nets: BTreeSet<&str> = project
        .vias
        .iter()
        .filter(|v| plan.conflicting_vias.contains(&v.id))
        .map(|v| project.net_name(v.net))
        .collect();
    let nets = nets.into_iter().collect::<Vec<_>>().join(", ");
    let (title, ack) = if count == 1 {
        (
            format!("A via on {nets} conflicts with this update"),
            "Remove that via".to_owned(),
        )
    } else {
        (
            format!("{count} vias on {nets} conflict with this update"),
            "Remove those vias".to_owned(),
        )
    };
    ReviewLine {
        level: Level::Error,
        title: refusal::clip(&title, MAX_LINE),
        ack: Some(ack),
        locate: true,
    }
}

/// The header icon of a review.
fn icon(change: &Change) -> &'static str {
    match change {
        Change::SyncPcb {} => "component",
        Change::Schematic { .. } => "line",
        _ => "component",
    }
}

/// The review's title, naming what it updates.
fn title(change: &Change, project: &Project) -> String {
    use pcbcraft_core::schematic::SchematicCommand;
    let name = |id: &Id| {
        project
            .instance(*id)
            .map_or_else(|| "instance".to_owned(), |i| i.designator.clone())
    };
    match change {
        Change::SyncPcb {} => "Update PCB".into(),
        Change::ComponentRevision { .. } => "Update instances".into(),
        Change::Instance {
            edit: InstanceChange::Remove { instance },
        } => format!("Remove {}", name(instance)),
        Change::Instance {
            edit: InstanceChange::SetPhysical { instance, .. },
        } => format!("Change footprint of {}", name(instance)),
        Change::Schematic {
            command: SchematicCommand::Connect { .. },
        } => "Connect pins".into(),
        Change::Schematic {
            command: SchematicCommand::Disconnect { .. },
        } => "Disconnect wire".into(),
        Change::Schematic { .. } => "Change schematic".into(),
    }
}

/// The success toast: "Updated PCB: adds 2 placements".
pub(crate) fn applied_line(plan: &UpdatePlan) -> String {
    use pcbcraft_core::schematic::SchematicCommand;
    let done = match &plan.change {
        Change::SyncPcb {} => "Updated PCB",
        Change::ComponentRevision { .. } => "Updated instances",
        Change::Instance {
            edit: InstanceChange::Remove { .. },
        } => "Removed the instance",
        Change::Instance { .. } => "Changed the footprint",
        Change::Schematic {
            command: SchematicCommand::Connect { .. },
        } => "Connected pins",
        Change::Schematic {
            command: SchematicCommand::Disconnect { .. },
        } => "Disconnected the wire",
        Change::Schematic { .. } => "Changed the schematic",
    };
    let summary = summary_of(plan);
    let mut chars = summary.chars();
    let summary: String = chars
        .next()
        .map(|c| c.to_lowercase().chain(chars).collect())
        .unwrap_or_default();
    refusal::clip(&format!("{done}: {summary}"), MAX_LINE)
}

/// "12 parts staged beside the board — place them on the board".
fn staged_line(count: usize) -> String {
    if count == 1 {
        "1 part staged beside the board — place it on the board".into()
    } else {
        format!("{count} parts staged beside the board — place them on the board")
    }
}

fn new_issues_line(new: usize) -> String {
    if new == 1 {
        "1 new issue after this update".into()
    } else {
        format!("{new} new issues after this update")
    }
}

/// The plan's one-line summary (C2's, or rebuilt from its items).
fn summary_of(plan: &UpdatePlan) -> String {
    if plan.summary.is_empty() {
        summarize(&plan.items)
    } else {
        plan.summary.clone()
    }
}

/// A finding's one line: the core message when it reads as a title, else
/// a plain stand-in (the raw text stays in Copy report).
fn finding_line(finding: &Finding) -> String {
    refusal::presentable(&finding.message).unwrap_or_else(|| "A check finding".to_owned())
}

/// What the panel shows for `plan`.
pub fn content(
    plan: &UpdatePlan,
    project: &Project,
    note: Option<&str>,
    choices: &[(Id, String, bool)],
) -> Content {
    let title = title(&plan.change, project);
    let summary = summary_of(plan);
    let mut lines = Vec::new();
    let mut refs = Vec::new();
    let mut conflicts = Vec::new();
    let mut push = |line: ReviewLine, objects: Vec<ObjectRef>| {
        lines.push(line);
        refs.push(objects);
        lines.len() - 1
    };
    if let Some(note) = note {
        push(
            ReviewLine {
                level: Level::Info,
                title: refusal::clip(note, MAX_LINE),
                ack: None,
                locate: false,
            },
            Vec::new(),
        );
    }
    if !choices.is_empty() && !choices.iter().any(|(_, _, on)| *on) {
        push(
            ReviewLine {
                level: Level::Error,
                title: "Check at least one instance to update".into(),
                ack: None,
                locate: false,
            },
            Vec::new(),
        );
    }
    for (n, blocker) in plan.blockers.iter().enumerate() {
        let (line, objects) = match plan.blocker_lines.get(n) {
            Some(line) => (line.line.clone(), line.refs.clone()),
            None => (
                refusal::presentable(blocker).unwrap_or_else(|| "The update is blocked".into()),
                Vec::new(),
            ),
        };
        push(
            ReviewLine {
                level: Level::Error,
                title: line,
                ack: None,
                locate: !objects.is_empty(),
            },
            objects,
        );
    }
    for conflict in &plan.conflicts {
        let mut objects = vec![
            ObjectRef::Track { id: conflict.track },
            ObjectRef::Net { id: conflict.net },
        ];
        objects.extend(conflict.pad);
        let n = push(
            ReviewLine {
                level: Level::Error,
                title: conflict.line.clone(),
                ack: Some(conflict.ack.clone()),
                locate: true,
            },
            objects,
        );
        conflicts.push(n);
    }
    // The conflicting vias, grouped in one acknowledged line: applying
    // removes them with the conflicting tracks.
    if !plan.conflicting_vias.is_empty() {
        let n = push(
            via_conflict_line(plan, project),
            plan.conflicting_vias
                .iter()
                .map(|id| ObjectRef::Via { id: *id })
                .collect(),
        );
        conflicts.push(n);
    }
    let vias: BTreeSet<Id> = plan.conflicting_vias.iter().copied().collect();
    let conflicting: BTreeSet<Id> = plan.conflicts.iter().map(|c| c.track).collect();
    for (n, warning) in plan.warnings.iter().enumerate() {
        let (line, objects) = match plan.warning_lines.get(n) {
            // The warning that restates the conflicting tracks: their own
            // lines and acknowledgements already say it (Copy report keeps
            // its words), so it is not shown twice.
            Some(line)
                if !conflicting.is_empty()
                    && !line.refs.is_empty()
                    && line.refs.iter().all(
                        |r| matches!(r, ObjectRef::Track { id } if conflicting.contains(id)),
                    ) =>
            {
                continue;
            }
            // Likewise the warning that restates the conflicting vias.
            Some(line)
                if !vias.is_empty()
                    && !line.refs.is_empty()
                    && line
                        .refs
                        .iter()
                        .all(|r| matches!(r, ObjectRef::Via { id } if vias.contains(id))) =>
            {
                continue;
            }
            Some(line) => (line.line.clone(), line.refs.clone()),
            None => (
                refusal::presentable(warning).unwrap_or_else(|| "Check this update".into()),
                Vec::new(),
            ),
        };
        push(
            ReviewLine {
                level: Level::Warning,
                title: line,
                ack: None,
                locate: !objects.is_empty(),
            },
            objects,
        );
    }
    // Parts new to the board are staged beside it, which no check flags
    // here: one line says so, with the parts to Show.
    let staged: Vec<ObjectRef> = plan
        .planned_placements
        .iter()
        .filter(|p| project.placement_of(p.instance).is_none())
        .map(|p| ObjectRef::Instance { id: p.instance })
        .collect();
    if !staged.is_empty() {
        push(
            ReviewLine {
                level: Level::Info,
                title: staged_line(staged.len()),
                ack: None,
                locate: true,
            },
            staged,
        );
    }
    let mut groups = Vec::new();
    for area in [
        ChangeArea::Component,
        ChangeArea::Schematic,
        ChangeArea::Nets,
        ChangeArea::Board,
    ] {
        // A new placement's pads fold into its one line.
        let items: Vec<_> = pcbcraft_core::updates::folded_items(plan, project)
            .filter(|i| i.area == area)
            .collect();
        if items.is_empty() {
            continue;
        }
        let group_summary = summarize(items.iter().copied());
        groups.push(ReviewGroup {
            title: area.title().to_owned(),
            summary: group_summary,
            lines: items.iter().map(|i| i.line.clone()).collect(),
        });
    }
    // A lone group's summary only repeats the review's summary.
    if let [group] = groups.as_mut_slice() {
        group.summary.clear();
    }
    let mut after = Vec::new();
    let mut after_refs = Vec::new();
    for row in &plan.findings.new_rows {
        if row.severity == pcbcraft_core::issues::Severity::Error {
            after.push(ReviewLine {
                level: Level::Error,
                // The row leads with its objects; the footer counts them
                // as new errors ("Leaves 3 new errors").
                title: refusal::clip(&row.title, MAX_LINE),
                ack: None,
                locate: !row.refs.is_empty(),
            });
            after_refs.push(row.refs.clone());
        }
    }
    let findings = &plan.findings;
    if !findings.line.is_empty() || !findings.new.is_empty() || !findings.resolved.is_empty() {
        // Issues' own titles, as core words them; the raw message only
        // for a plan made without them.
        let titled = |rows: &[pcbcraft_core::updates::FindingRow], raw: &[Finding]| {
            if rows.len() == raw.len() {
                rows.iter().map(|r| r.title.clone()).collect::<Vec<_>>()
            } else {
                raw.iter().map(finding_line).collect()
            }
        };
        let mut lines: Vec<String> = titled(&findings.new_rows, &findings.new)
            .into_iter()
            .map(|t| refusal::clip(&format!("{t} (new)"), MAX_LINE))
            .collect();
        lines.extend(
            titled(&findings.resolved_rows, &findings.resolved)
                .into_iter()
                .map(|t| refusal::clip(&format!("{t} (resolved)"), MAX_LINE)),
        );
        groups.push(ReviewGroup {
            title: "After this update".into(),
            summary: findings.line.clone(),
            lines,
        });
    }
    let report = report(plan, &title, &summary);
    let destructive = matches!(
        plan.change,
        Change::Instance {
            edit: InstanceChange::Remove { .. }
        }
    );
    let apply = if destructive {
        title.clone()
    } else {
        "Apply update".to_owned()
    };
    Content {
        title,
        summary,
        lines,
        refs,
        conflicts,
        after,
        after_refs,
        groups,
        report,
        apply,
        destructive,
    }
}

/// Every raw line of the plan, for Copy report.
fn report(plan: &UpdatePlan, title: &str, summary: &str) -> String {
    let mut out = vec![format!("{title}: {summary}")];
    let mut section = |name: &str, lines: &mut dyn Iterator<Item = String>| {
        let lines: Vec<String> = lines.collect();
        if !lines.is_empty() {
            out.push(String::new());
            out.push(format!("{name}:"));
            out.extend(lines.into_iter().map(|l| format!("- {l}")));
        }
    };
    section("Blockers", &mut plan.blockers.iter().cloned());
    section(
        "Conflicting tracks",
        &mut plan
            .conflicts
            .iter()
            .map(|c| format!("{} ({})", c.line, c.ack)),
    );
    section("Warnings", &mut plan.warnings.iter().cloned());
    section("Changes", &mut plan.changes.iter().cloned());
    section(
        "New issues",
        &mut plan.findings.new.iter().map(|f| f.message.clone()),
    );
    section(
        "Resolved issues",
        &mut plan.findings.resolved.iter().map(|f| f.message.clone()),
    );
    out.join("\n")
}

/// The objects `plan` touches, by how the canvas outlines them.
pub fn marks(plan: &UpdatePlan) -> Marks {
    let mut marks = Marks::default();
    for item in &plan.items {
        let removes = item.kind == ChangeKind::Remove
            && matches!(
                item.object,
                ChangeObject::Instance | ChangeObject::Placement
            );
        for object in &item.refs {
            let instance = match object {
                ObjectRef::Instance { id } => Some(*id),
                ObjectRef::Pad { instance, .. } => Some(*instance),
                ObjectRef::Terminal { endpoint } => Some(endpoint.instance),
                _ => None,
            };
            if let Some(id) = instance {
                if removes && matches!(object, ObjectRef::Instance { .. }) {
                    marks.removed.insert(id);
                } else {
                    marks.changed.insert(id);
                }
            }
        }
    }
    for id in &marks.removed {
        marks.changed.remove(id);
    }
    for finding in &plan.findings.new {
        for object in &finding.objects {
            let id = match object {
                ObjectRef::Instance { id } => *id,
                ObjectRef::Pad { instance, .. } => *instance,
                ObjectRef::Terminal { endpoint } => endpoint.instance,
                _ => continue,
            };
            if !marks.changed.contains(&id) && !marks.removed.contains(&id) {
                marks.issues.insert(id);
            }
        }
    }
    marks.tracks.extend(plan.conflicts.iter().map(|c| c.track));
    marks.tracks.extend(plan.conflicting_tracks.iter().copied());
    marks.ghosts.clone_from(&plan.planned_placements);
    marks
}

/// Outline what a review changes on the board canvas: changed placements
/// in the accent, those new findings name in the warning colour, removed
/// ones and the tracks the update removes in the error colour, and what "Show" named with a wider halo. `outline` gives
/// a placement's body corners; `screen` maps board millimetres.
pub fn paint_board(
    painter: &egui::Painter,
    project: &Project,
    marks: &Marks,
    screen: impl Fn(Point) -> Pos2,
    outline: impl Fn(Id) -> Option<[Point; 4]>,
    ghost: impl Fn(&pcbcraft_core::Placement) -> Option<[Point; 4]>,
) {
    let t = Tokens::get();
    // The result first, under the outlines: each added or moved placement
    // as a translucent body with its designator.
    for placement in &marks.ghosts {
        let Some(corners) = ghost(placement) else {
            continue;
        };
        let points: Vec<Pos2> = corners.iter().map(|p| screen(*p)).collect();
        painter.add(egui::Shape::convex_polygon(
            points.clone(),
            t.accent.gamma_multiply(0.18),
            Stroke::new(1.5, t.accent.gamma_multiply(0.8)),
        ));
        if let Some(name) = project.instance(placement.instance).map(|i| &i.designator) {
            let centre = points
                .iter()
                .fold(egui::Vec2::ZERO, |sum, p| sum + p.to_vec2())
                / points.len().max(1) as f32;
            painter.text(
                centre.to_pos2(),
                egui::Align2::CENTER_CENTER,
                name,
                egui::FontId::proportional(t.font_small),
                t.accent,
            );
        }
    }
    let focused = |object: &ObjectRef| marks.focus.contains(object);
    // A halo a few points outside a convex outline, so it reads over copper.
    let ring = |corners: &[Point], color: egui::Color32, width: f32| {
        let points: Vec<Pos2> = corners.iter().map(|p| screen(*p)).collect();
        let halo = halo(&points, t.space_4 + width);
        painter.add(egui::Shape::closed_line(halo, Stroke::new(width, color)));
    };
    for track in &project.tracks {
        if !marks.tracks.contains(&track.id) {
            continue;
        }
        let corners = layout::track_outline(track);
        let points: Vec<Pos2> = corners.iter().map(|p| screen(*p)).collect();
        painter.add(egui::Shape::convex_polygon(
            points,
            t.error.gamma_multiply(0.35),
            Stroke::NONE,
        ));
        let width = if focused(&ObjectRef::Track { id: track.id }) {
            3.0
        } else {
            1.5
        };
        ring(&corners, t.error, width);
    }
    for (ids, color) in [
        (&marks.issues, t.warning),
        (&marks.changed, t.accent),
        (&marks.removed, t.error),
    ] {
        for id in ids {
            let Some(corners) = outline(*id) else {
                continue;
            };
            let width = if focused(&ObjectRef::Instance { id: *id }) {
                3.0
            } else {
                2.0
            };
            ring(&corners, color, width);
        }
    }
    // What Show named but the plan does not outline (a pad's part, or a
    // net's tracks): a halo in the selection colour.
    for object in &marks.focus {
        match object {
            ObjectRef::Instance { id } | ObjectRef::Pad { instance: id, .. }
                if !marks.changed.contains(id)
                    && !marks.removed.contains(id)
                    && !marks.issues.contains(id) =>
            {
                if let Some(corners) = outline(*id) {
                    ring(&corners, t.body_selected, 3.0);
                }
            }
            _ => {}
        }
    }
}

/// A convex screen outline moved `by` points outward, each vertex along
/// the mitre of its two edges' outward normals.
fn halo(points: &[Pos2], by: f32) -> Vec<Pos2> {
    let n = points.len();
    let centre = points
        .iter()
        .fold(egui::Vec2::ZERO, |sum, p| sum + p.to_vec2())
        / n.max(1) as f32;
    let outward = |a: Pos2, b: Pos2| {
        let d = (b - a).normalized();
        let normal = egui::vec2(d.y, -d.x);
        // Outward is away from the centre of a convex outline.
        if normal.dot(a.to_vec2() - centre) < 0.0 {
            -normal
        } else {
            normal
        }
    };
    (0..n)
        .map(|i| {
            let p = points[i];
            let n0 = outward(points[(i + n - 1) % n], p);
            let n1 = outward(p, points[(i + 1) % n]);
            let k = (1.0 + n0.dot(n1)).max(0.125);
            p + (n0 + n1) * (by / k)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::issues::style;

    /// A review that lets the user pick instances lists each one checked;
    /// unchecking one recomputes the plan for the others only.
    #[test]
    fn unchecked_instances_leave_the_reviewed_change() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let instances: Vec<_> = engine.project().instances.iter().map(|i| i.id).collect();
        let first = engine.project().instances.first().ok_or("an instance")?;
        assert!(instances.len() >= 2, "the demo places several instances");
        let change = Change::ComponentRevision {
            instances: instances.clone(),
            target: first.component,
            resolutions: Default::default(),
        };
        let mut review = UpdatePreview::default();
        review.begin_choosing(&engine, change, Some("copied".into()));
        assert_eq!(review.choices.len(), instances.len());
        assert!(review.choices.iter().all(|(_, _, on)| *on));
        assert_eq!(review.note.as_deref(), Some("copied"));
        review.choices[0].2 = false;
        review.rechoose(&engine);
        match review.plan.as_ref().map(|p| &p.change) {
            Some(Change::ComponentRevision {
                instances: kept, ..
            }) => {
                assert_eq!(kept.as_slice(), &instances[1..]);
            }
            other => return Err(format!("unexpected {other:?}").into()),
        }
        assert_eq!(review.take_closed(), None);
        Ok(())
    }

    /// The demo plus R3 on the schematic but not yet on the board, and a
    /// track that joins R1 pad 1 to R2 pad 1 under N-names the update
    /// keeps, so Update PCB has something to add.
    fn with_unplaced() -> Result<Engine, Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let first = engine.project().instances[0].clone();
        engine.apply(
            engine.revision(),
            Operation::Instance(pcbcraft_core::InstanceCommand::Place(
                pcbcraft_core::instances::NewInstance {
                    sheet: None,
                    component: first.component,
                    designator: Some("R3".into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: pcbcraft_core::UnitId(1),
                    position: [30.0, 0.0],
                    physical: first.physical,
                },
            )),
        )?;
        Ok(engine)
    }

    fn check_style(content: &Content) {
        let mut texts = vec![content.title.clone(), content.summary.clone()];
        texts.extend(content.lines.iter().map(|l| l.title.clone()));
        texts.extend(content.lines.iter().filter_map(|l| l.ack.clone()));
        for group in &content.groups {
            texts.push(group.title.clone());
            // An empty group summary is not shown.
            if !group.summary.is_empty() {
                texts.push(group.summary.clone());
            }
            texts.extend(group.lines.iter().cloned());
        }
        for text in texts {
            assert_eq!(style::check(&text), vec![], "{text:?}");
        }
    }

    /// Update PCB reads its C2 summary first, groups the changes by area,
    /// says what happens to the findings, and every shown line follows the
    /// message style (no IDs, at most 80 characters).
    #[test]
    fn update_pcb_reads_c2_summary_groups_and_findings() -> Result<(), Box<dyn std::error::Error>> {
        let engine = with_unplaced()?;
        let plan = engine.preview_update(Change::SyncPcb {});
        let content = content(&plan, engine.project(), None, &[]);
        assert_eq!(content.title, "Update PCB");
        assert!(
            content.summary.starts_with("Adds 1 placement: R3"),
            "{}",
            content.summary
        );
        assert_eq!(content.apply, "Apply update");
        assert!(!content.destructive);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        let pcb = content
            .groups
            .iter()
            .find(|g| g.title == "PCB")
            .ok_or("a PCB group")?;
        assert!(pcb.lines.iter().any(|l| l.contains("R3")), "{pcb:?}");
        let after = content.groups.last().ok_or("an After group")?;
        assert_eq!(after.title, "After this update");
        assert_eq!(after.summary, plan.findings.line);
        assert_eq!(
            after.lines.len(),
            plan.findings.new.len() + plan.findings.resolved.len(),
            "every new and resolved finding is listed"
        );
        check_style(&content);
        // Copy report keeps the full engine wording of every change.
        for change in &plan.changes {
            assert!(content.report.contains(change.as_str()), "{change}");
        }
        let marks = marks(&plan);
        let r3 = engine
            .project()
            .instance_by_designator("R3")
            .ok_or("R3")?
            .id;
        assert!(marks.changed.contains(&r3), "{marks:?}");
        // The canvas previews the result: R3 drawn where it will land.
        assert!(marks.ghosts.iter().any(|p| p.instance == r3), "{marks:?}");
        // New errors are error rows above the changes, with their objects.
        let errors = plan
            .findings
            .new_rows
            .iter()
            .filter(|r| r.severity == pcbcraft_core::issues::Severity::Error)
            .count();
        assert_eq!(content.after.len(), errors);
        // Each leads with its own objects, not a shared prefix.
        assert!(content.after.iter().all(|l| l.level == Level::Error
            && !l.title.starts_with("New")
            && plan.findings.new_rows.iter().any(|r| r.title == l.title)));
        // R3 is staged beside the board: its copper past the edge is no
        // new finding of the update; one line says it is staged.
        assert!(
            !plan
                .findings
                .new
                .iter()
                .any(|f| f.code == "board.outside_outline"),
            "{:?}",
            plan.findings.new
        );
        let staged = content
            .lines
            .iter()
            .position(|l| l.title == "1 part staged beside the board — place it on the board")
            .ok_or("a staged line")?;
        assert_eq!(content.lines[staged].level, Level::Info);
        assert_eq!(content.refs[staged], vec![ObjectRef::Instance { id: r3 }]);
        // The board checks still report it once applied.
        let mut applied = with_unplaced()?;
        applied.apply(
            applied.revision(),
            Operation::Update(Box::new(ApplyUpdate {
                expected_revision: applied.revision(),
                change: plan.change.clone(),
                remove_conflicting_tracks: false,
            })),
        )?;
        assert!(
            layout::checks(applied.project()).iter().any(|f| {
                f.code == "board.outside_outline"
                    && f.objects
                        .iter()
                        .all(|o| matches!(o, ObjectRef::Pad { instance, .. } if *instance == r3))
            }),
            "the board checks flag the staged part"
        );
        // Pads of a new placement fold into its line.
        assert!(
            !pcb.lines
                .iter()
                .any(|l| l.contains("is added on the board")),
            "{pcb:?}"
        );
        // Placed parts that a new finding names are outlined too.
        for finding in &plan.findings.new {
            for object in &finding.objects {
                if let ObjectRef::Pad { instance, .. } = object
                    && *instance != r3
                {
                    assert!(marks.issues.contains(instance), "{marks:?}");
                }
            }
        }
        Ok(())
    }

    /// A removal's review says what it removes in a user's words, and its
    /// primary is the destructive "Remove R1"; the removed part is marked.
    #[test]
    fn a_removal_review_is_destructive_and_names_what_goes()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let r1 = engine
            .project()
            .instance_by_designator("R1")
            .ok_or("R1")?
            .id;
        let plan = engine.preview_update(Change::Instance {
            edit: InstanceChange::Remove { instance: r1 },
        });
        let content = content(&plan, engine.project(), None, &[]);
        assert_eq!(content.apply, "Remove R1");
        assert!(content.destructive);
        assert!(
            content.summary.starts_with("Removes R1"),
            "{}",
            content.summary
        );
        assert!(
            !content.summary.contains("sheet symbol"),
            "{}",
            content.summary
        );
        check_style(&content);
        assert!(marks(&plan).removed.contains(&r1));
        Ok(())
    }

    /// Nothing that blocks is hidden: every blocker, conflicting track and
    /// warning of the plan has its own line, and conflicts carry their
    /// acknowledgement and objects for Show.
    #[test]
    fn every_blocker_conflict_and_warning_has_a_line() -> Result<(), Box<dyn std::error::Error>> {
        use pcbcraft_core::Layer;
        use pcbcraft_core::updates::{ConflictReason, PlanLine, TrackConflict};
        let engine = Engine::demo()?;
        let mut plan = engine.preview_update(Change::SyncPcb {});
        plan.blockers = vec!["R9 has no footprint.".into()];
        plan.blocker_lines = vec![PlanLine {
            line: "R9 has no footprint".into(),
            detail: "R9 has no footprint.".into(),
            refs: vec![ObjectRef::Instance { id: 9 }],
        }];
        plan.warnings = vec![
            "Remove 1 conflicting track to apply this update.".into(),
            "Unnamed warning".into(),
        ];
        plan.warning_lines = vec![PlanLine {
            line: "Remove 1 conflicting track to apply; other copper stays".into(),
            detail: "Remove 1 conflicting track to apply this update.".into(),
            refs: vec![ObjectRef::Track { id: 42 }],
        }];
        plan.conflicting_tracks = vec![42];
        plan.conflicts = vec![TrackConflict {
            track: 42,
            net: 7,
            net_name: "N22".into(),
            layer: Layer::Top.into(),
            length_mm: 4.2,
            reason: ConflictReason::NetChanged,
            pad: None,
            line: "1 track on N22 would join two nets".into(),
            ack: "Remove that track (Top, 4.2 mm)".into(),
        }];
        let c = content(
            &plan,
            engine.project(),
            Some("Copied r2 into the project"),
            &[],
        );
        let titles: Vec<_> = c
            .lines
            .iter()
            .map(|l| (l.level, l.title.as_str()))
            .collect();
        assert_eq!(
            titles,
            vec![
                (Level::Info, "Copied r2 into the project"),
                (Level::Error, "R9 has no footprint"),
                (Level::Error, "1 track on N22 would join two nets"),
                (Level::Warning, "Unnamed warning"),
            ]
        );
        assert_eq!(c.conflicts, vec![2]);
        assert_eq!(
            c.lines[2].ack.as_deref(),
            Some("Remove that track (Top, 4.2 mm)")
        );
        assert!(c.refs[2].contains(&ObjectRef::Track { id: 42 }));
        assert!(c.lines[1].locate && c.lines[2].locate);
        // The unackable blocker gates; the conflict alone gates until acked.
        assert!(
            crate::kit::review::apply_gate(false, &c.lines, &[true, true, true, true]).is_some()
        );
        let only_conflict: Vec<_> = c
            .lines
            .iter()
            .filter(|l| l.level != Level::Error || l.ack.is_some())
            .cloned()
            .collect();
        assert!(
            crate::kit::review::apply_gate(false, &only_conflict, &[false, false, false]).is_some()
        );
        assert_eq!(
            crate::kit::review::apply_gate(false, &only_conflict, &[false, true, false]),
            None
        );
        assert!(c.report.contains("R9 has no footprint."));
        assert!(c.report.contains("Remove that track (Top, 4.2 mm)"));
        // The warning that restates the conflict shows once, as the
        // conflict, but Copy report keeps it.
        assert!(
            !c.lines
                .iter()
                .any(|l| l.title.starts_with("Remove 1 conflicting"))
        );
        assert!(
            c.report
                .contains("Remove 1 conflicting track to apply this update.")
        );
        assert!(marks(&plan).tracks.contains(&42));
        // Unchecking every instance is itself a blocker.
        let none = content(&plan, engine.project(), None, &[(1, "R1".into(), false)]);
        assert_eq!(none.lines[0].title, "Check at least one instance to update");
        assert_eq!(none.lines[0].level, Level::Error);
        Ok(())
    }

    fn frame(
        review: &mut UpdatePreview,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Operation>) {
        frame_with(review, engine, ctx, events, |_, _| {})
    }

    /// One frame; `after` runs inside it, after the review, as the app's
    /// `apply` does with the returned operation.
    fn frame_with(
        review: &mut UpdatePreview,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut after: impl FnMut(&egui::Context, Option<&Operation>),
    ) -> (egui::FullOutput, Option<Operation>) {
        let mut out = None;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(380.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            out = review.show(ui, engine);
            after(ui.ctx(), out.as_ref());
        });
        output.textures_delta.clear();
        (output, out)
    }

    fn text_at(output: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
        text_where(output, |shown| shown == text)
    }

    fn text_where(output: &egui::FullOutput, pick: impl Fn(&str) -> bool) -> Option<egui::Pos2> {
        output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if pick(&t.galley.job.text) => Some(t.pos + egui::vec2(4.0, 4.0)),
            _ => None,
        })
    }

    fn click(
        review: &mut UpdatePreview,
        engine: &Engine,
        ctx: &egui::Context,
        text: &str,
    ) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
        click_with(review, engine, ctx, text, |_, _| {})
    }

    fn click_with(
        review: &mut UpdatePreview,
        engine: &Engine,
        ctx: &egui::Context,
        text: &str,
        after: impl FnMut(&egui::Context, Option<&Operation>),
    ) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
        let (output, _) = frame(review, engine, ctx, vec![]);
        let at = text_at(&output, text).ok_or_else(|| format!("no {text:?}"))?;
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(review, engine, ctx, vec![egui::Event::PointerMoved(at)]);
        let (_, op) = frame_with(review, engine, ctx, vec![press(true), press(false)], after);
        Ok(op)
    }

    /// The docked review: summary, then the gated Apply that sends the
    /// reviewed plan once; the next frame closes it as applied and posts
    /// the success toast with Undo.
    #[test]
    fn apply_sends_the_reviewed_plan_and_closes_on_the_next_frame()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = with_unplaced()?;
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let mut review = UpdatePreview::default();
        review.begin(&engine, Change::SyncPcb {});
        let (output, _) = frame(&mut review, &engine, &ctx, vec![]);
        let summary =
            text_where(&output, |t| t.starts_with("Adds 1 placement")).ok_or("summary")?;
        let groups = text_at(&output, "PCB").ok_or("PCB group")?;
        assert!(summary.y < groups.y, "the summary comes first");
        let op = click(&mut review, &engine, &ctx, "Apply update")?.ok_or("apply")?;
        let Operation::Update(request) = &op else {
            return Err("not an update".into());
        };
        assert_eq!(request.expected_revision, engine.revision());
        assert!(review.is_open(), "open until the engine answers");
        engine.apply(engine.revision(), op)?;
        frame(&mut review, &engine, &ctx, vec![]);
        assert!(!review.is_open());
        assert_eq!(review.take_closed(), Some(Closed::Applied));
        let posted = crate::kit::toast::take_posted(&ctx);
        let first = posted.first().ok_or("a toast")?;
        assert_eq!(first.level, Level::Ok);
        assert!(
            first.text.starts_with("Updated PCB: adds 1 placement"),
            "{}",
            first.text
        );
        assert_eq!(style::check(&first.text), vec![]);
        assert_eq!(first.actions, vec![ToastAction::Undo]);
        Ok(())
    }

    /// An update whose only conflicts are vias shows them as one
    /// acknowledged line; acknowledging it confirms their removal, and the
    /// engine applies the update and removes the via.
    #[test]
    fn an_update_with_only_via_conflicts_applies_once_acknowledged()
    -> Result<(), Box<dyn std::error::Error>> {
        use pcbcraft_core::layout::{LayoutCommand, ViaRequest};
        let mut engine = Engine::demo()?;
        let wire = engine.project().wires.first().ok_or("a wire")?.id;
        let r1 = engine
            .project()
            .instances
            .iter()
            .find(|i| i.designator == "R1")
            .map(|i| i.id)
            .ok_or("R1")?;
        let net = engine
            .project()
            .net_of(pcbcraft_core::Endpoint {
                instance: r1,
                terminal: pcbcraft_core::TerminalId(2),
            })
            .map(|n| n.id)
            .ok_or("the shared net")?;
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::AddVia {
                via: ViaRequest::through(net, [12.0, 20.0]),
            }),
        )?;
        let change = Change::Schematic {
            command: pcbcraft_core::schematic::SchematicCommand::Disconnect { wire },
        };
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let mut review = UpdatePreview::default();
        review.begin(&engine, change);
        let plan = review.plan.as_ref().ok_or("plan")?;
        assert_eq!(plan.conflicting_vias.len(), 1);
        assert!(plan.conflicts.is_empty(), "no track conflicts");
        let content = content(plan, engine.project(), None, &[]);
        let [line] = content.conflicts.as_slice() else {
            return Err("one conflict line".into());
        };
        assert_eq!(content.lines[*line].ack.as_deref(), Some("Remove that via"));
        assert!(matches!(
            content.refs[*line].as_slice(),
            [ObjectRef::Via { .. }]
        ));
        // No warning restates it.
        assert!(
            !content
                .lines
                .iter()
                .any(|l| l.title.contains("conflicting via"))
        );
        assert!(click(&mut review, &engine, &ctx, "Remove that via")?.is_none());
        let op = click(&mut review, &engine, &ctx, "Apply update")?.ok_or("apply")?;
        let Operation::Update(request) = &op else {
            return Err("not an update".into());
        };
        assert!(
            request.remove_conflicting_tracks,
            "the acknowledgement confirms"
        );
        engine.apply(engine.revision(), op)?;
        assert!(engine.project().vias.is_empty());
        Ok(())
    }

    /// A refusal handed back to the review shows as its own last row and
    /// keeps the review open for a retry.
    #[test]
    fn a_refused_apply_stays_open_with_its_refusal() -> Result<(), Box<dyn std::error::Error>> {
        let engine = with_unplaced()?;
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let mut review = UpdatePreview::default();
        review.begin(&engine, Change::SyncPcb {});
        // The app refuses it in the same frame (as App::apply would, to the
        // origin the review marked) and hands it back.
        let op = click_with(&mut review, &engine, &ctx, "Apply update", |ctx, op| {
            if op.is_some() {
                let origin = refusal::take_mark(ctx).unwrap_or(Origin::Command);
                refusal::hand_back(
                    ctx,
                    refusal::Refused {
                        origin,
                        error: pcbcraft_core::Error::Conflict("Board update refused.".into()),
                    },
                    false,
                );
            }
        })?
        .ok_or("apply")?;
        assert!(matches!(op, Operation::Update(_)));
        frame(&mut review, &engine, &ctx, vec![]);
        let (output, _) = frame(&mut review, &engine, &ctx, vec![]);
        assert!(review.is_open());
        assert_eq!(review.take_closed(), None);
        assert!(review.refusal.is_some());
        assert!(
            output.shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Text(t) if t.galley.job.text.contains("Board update refused"))),
            "the refusal row shows"
        );
        Ok(())
    }

    /// Show on a conflicting track names it for the canvas, which outlines
    /// it in the error colour.
    #[test]
    fn marks_carry_the_located_objects() -> Result<(), Box<dyn std::error::Error>> {
        let engine = with_unplaced()?;
        let mut review = UpdatePreview::default();
        assert!(review.marks().is_none(), "no marks without a review");
        review.begin(&engine, Change::SyncPcb {});
        review.focus = vec![ObjectRef::Track { id: 5 }];
        let marks = review.marks().ok_or("marks")?;
        assert_eq!(marks.focus, vec![ObjectRef::Track { id: 5 }]);
        review.recompute(&engine);
        assert!(
            review.marks().ok_or("marks")?.focus.is_empty(),
            "a new plan starts unlocated"
        );
        Ok(())
    }
}
