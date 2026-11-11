//! The Issues surface's app side: it only presents what `project.issues`
//! returns, adding Fix launchers and locate (docs/sidebar-ia.md, "Issues").
//!
//! The app never computes, merges or rewords findings itself: severity,
//! groups, titles, details and object names come from
//! `pcbcraft_core::issues`. This module holds the Issues card's state (open,
//! scope, filter, folds, the shown issue), lays out the card body, the
//! canvas count badge and the "Showing" pill, gives the ribbon toggle and
//! the Browser their counts, and routes locate to the workspace that shows
//! an issue's objects through [`Locate`].
//!
//! Every query is cached per document revision, so a frame that changes
//! nothing asks core nothing.
use crate::kit::{
    self,
    issues::{IssueGroupView, IssueListEvent, IssueRowView},
    severity::Level,
};
use crate::shell::Workspace;
use egui::{Align, Layout, Rect, Ui, UiBuilder, pos2, vec2};
use pcbcraft_core::ids::TerminalId;
use pcbcraft_core::issues::{IssueArea, IssueCounts, IssueReport, IssueRow, IssueScope, Severity};
use pcbcraft_core::library::Definition;
use pcbcraft_core::{DefinitionId, Engine, Id, ObjectRef, Operation, Query, QueryResult};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::ui_kit;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Selecting and framing the objects an issue refers to. Each editor
/// implements it for the object kinds its workspace shows, switching tabs
/// as needed; the app routes across workspaces ([`route`]). Locate changes
/// selection and camera only, never the document or its geometry.
pub trait Locate {
    /// Select and frame every object of `refs` this editor shows. Returns
    /// whether it showed at least one.
    fn locate(&mut self, engine: &Engine, refs: &[ObjectRef]) -> bool;
}

/// A dialog an issue's Fix opens. Each issues its own typed operation.
#[allow(dead_code, reason = "the dialogs exist; no FIXES entry opens them yet")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FixDialog {
    AssignPads,
    ChoosePart,
    ResolveParts,
    UpdatePcb,
    UpdateInstances,
    Mapping,
    /// The Design rules panel at the rule a board finding rests on.
    DesignRules(crate::pcb_editor::RuleTopic),
}

/// What an issue's Fix button does. Only issues that a typed command, or a
/// dialog that issues one, resolves get a Fix.
#[derive(Clone, Debug)]
pub enum Fix {
    /// Apply this operation through the one command engine; `done` is the
    /// toast that offers Undo once it applied.
    Apply {
        label: &'static str,
        done: &'static str,
        operation: Box<Operation>,
    },
    /// Open a dialog whose primary issues the operation.
    Open {
        label: &'static str,
        dialog: FixDialog,
    },
}

impl Fix {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Apply { label, .. } | Self::Open { label, .. } => label,
        }
    }

    /// A dialog that acts on every issue of its group at once (Update PCB
    /// places every unplaced instance); its Fix sits on the group header.
    fn group_wide(&self) -> bool {
        matches!(
            self,
            Self::Open {
                dialog: FixDialog::UpdatePcb | FixDialog::ResolveParts,
                ..
            }
        )
    }
}

/// The rule codes that have a Fix, and the Fix. Only dialogs the app can
/// open today are listed; the W slices add Assign pads and Choose part when
/// their dialogs exist.
const FIXES: &[(&str, &str, FixDialog)] = &[
    ("board.unplaced", "Update PCB…", FixDialog::UpdatePcb),
    (
        "purchasing.no_part",
        "Resolve parts…",
        FixDialog::ResolveParts,
    ),
];

/// The Fix for `row`, if it has one.
pub fn fix(row: &IssueRow) -> Option<Fix> {
    no_connect_fix(row).or_else(|| dialog_fix(row))
}

/// An unconnected terminal's Fix marks it no-connect; a connected
/// no-connect terminal's clears the mark. One `instance.set_no_connect`
/// over the row's terminals, when they are of one instance.
fn no_connect_fix(row: &IssueRow) -> Option<Fix> {
    let (label, done, no_connect) = match row.group.code.as_str() {
        "instance.terminal_unaccounted" => ("Mark no-connect", "Marked no-connect", true),
        "instance.no_connect_connected" => ("Clear no-connect", "Cleared no-connect", false),
        _ => return None,
    };
    let mut instance = None;
    let mut terminals = BTreeSet::new();
    for object in row.objects() {
        if let ObjectRef::Terminal { endpoint } = object {
            if *instance.get_or_insert(endpoint.instance) != endpoint.instance {
                return None;
            }
            terminals.insert(endpoint.terminal);
        }
    }
    let operation = Operation::Instance(pcbcraft_core::InstanceCommand::SetNoConnect {
        instance: instance?,
        terminals: terminals.into_iter().collect(),
        no_connect,
    });
    Some(Fix::Apply {
        label,
        done,
        operation: Box::new(operation),
    })
}

fn dialog_fix(row: &IssueRow) -> Option<Fix> {
    FIXES
        .iter()
        .find(|(code, _, _)| *code == row.group.code)
        .map(|(_, label, dialog)| Fix::Open {
            label,
            dialog: *dialog,
        })
        .or_else(|| {
            crate::pcb_editor::rule_topic_for(&row.group.code).map(|topic| Fix::Open {
                label: "Show rule",
                dialog: FixDialog::DesignRules(topic),
            })
        })
}

/// The issue area a workspace shows by default.
pub fn area(workspace: Workspace) -> IssueArea {
    match workspace {
        Workspace::Part => IssueArea::Component,
        Workspace::Schematic => IssueArea::Schematic,
        Workspace::Pcb => IssueArea::Board,
    }
}

/// The workspace that shows an area's issues.
pub fn workspace(area: IssueArea) -> Workspace {
    match area {
        IssueArea::Component => Workspace::Part,
        IssueArea::Schematic => Workspace::Schematic,
        IssueArea::Board => Workspace::Pcb,
    }
}

/// An area by its workspace's name.
fn area_label(area: IssueArea) -> &'static str {
    workspace(area).label()
}

/// One `project.issues` answer. A refusal is kept as such, so no surface
/// mistakes it for "no issues".
#[derive(Clone, Debug)]
pub enum Answer {
    Report(Arc<IssueReport>),
    /// The query refused; the text says why, in plain words.
    Refused(String),
}

impl Answer {
    fn of(result: pcbcraft_core::Result<QueryResult>) -> Self {
        match result {
            Ok(QueryResult::Issues(report)) => Self::Report(Arc::new(report)),
            Ok(_) => Self::Refused("The checks gave an unexpected answer".into()),
            Err(error) => Self::Refused(crate::refusal::shown_error(&error)),
        }
    }

    pub fn report(&self) -> Option<&Arc<IssueReport>> {
        match self {
            Self::Report(report) => Some(report),
            Self::Refused(_) => None,
        }
    }
}

/// What a cached answer was computed from.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    revision: u64,
    project: usize,
    scope: IssueScope,
}

fn document(engine: &Engine) -> (u64, usize) {
    (
        engine.revision(),
        Arc::as_ptr(engine.project_arc()) as usize,
    )
}

/// One query's answer, recomputed only when the document or scope changes.
#[derive(Default)]
struct Slot {
    cached: Option<(Key, Answer)>,
}

impl Slot {
    fn get(&mut self, engine: &Engine, scope: IssueScope) -> Answer {
        let (revision, project) = document(engine);
        let key = Key {
            revision,
            project,
            scope,
        };
        if let Some((cached, answer)) = &self.cached
            && *cached == key
        {
            return answer.clone();
        }
        let answer = Answer::of(engine.query(Query::Issues {
            scope: key.scope.clone(),
        }));
        self.cached = Some((key, answer.clone()));
        answer
    }
}

/// What the Issues bar shows: the active workspace's counts and the
/// blockers elsewhere, or that the issues could not be counted.
#[derive(Clone, Debug, PartialEq)]
pub enum Summary {
    Counted {
        counts: IssueCounts,
        /// Areas outside the workspace that hold blockers ("2 blocking in
        /// Component"), count first so a clipped bar still shows it.
        elsewhere: Option<String>,
    },
    /// The query refused: the bar says so rather than show no counts.
    NotCounted(String),
}

impl Summary {
    fn of(answer: &Answer) -> Self {
        match answer {
            Answer::Report(report) => Self::from_report(report),
            Answer::Refused(why) => Self::NotCounted(why.clone()),
        }
    }

    fn from_report(report: &IssueReport) -> Self {
        let elsewhere: Vec<String> = report
            .elsewhere
            .iter()
            .filter(|a| a.blocking > 0)
            .map(|a| format!("{} blocking in {}", a.blocking, area_label(a.area)))
            .collect();
        Self::Counted {
            counts: report.counts,
            elsewhere: (!elsewhere.is_empty()).then(|| elsewhere.join(" · ")),
        }
    }

    /// The counts, or `None` while not counted.
    pub fn counts(&self) -> Option<IssueCounts> {
        match self {
            Self::Counted { counts, .. } => Some(*counts),
            Self::NotCounted(_) => None,
        }
    }
}

/// The Selection scope's empty list when the selection names nothing it
/// lists (nothing selected, or a sheet symbol or drawing, which it does not
/// cover): which scope lists the issues the bar counts here.
fn nothing_selected(summary: &Summary, workspace: Workspace) -> String {
    let here = match summary {
        Summary::Counted { counts, .. } => {
            counts.error + counts.warning + counts.to_do + counts.info
        }
        Summary::NotCounted(_) => 0,
    };
    let only = format!("{} only", workspace.label());
    match here {
        0 => "Select an object to list its issues".to_owned(),
        1 => format!("None on the selection; {only} lists the 1 issue here"),
        n => format!("None on the selection; {only} lists the {n} issues here"),
    }
}

/// Shown on the bar while the issues cannot be counted, in place of counts.
const NOT_COUNTED: &str = "Not counted";

/// Which issues the card lists. The bar always counts the workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// The active workspace's area, the default.
    #[default]
    Workspace,
    All,
    /// Issues on the selected objects.
    Selection,
}

/// The issue the canvas pill names, located last.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    key: String,
    title: String,
    level: Level,
}

/// The width of the "Showing" row besides its text: the ‹ and › buttons,
/// the pill's glyph, × and padding.
const PILL_CHROME: f32 = 132.0;

/// The canvas's top-right corner kept for the count badge ("× 38 ○ 15"),
/// so the pill never covers it.
const BADGE_RESERVE: f32 = 120.0;

/// What the app should do for the Issues surface this frame.
#[derive(Clone, Debug)]
pub enum Request {
    /// Select and frame these objects, preferring the area's workspace.
    /// A pin-configuration (`config.*`) issue also opens the pin-mux panel
    /// of its instance at its pin ([`PinMuxTarget`]).
    Locate {
        refs: Vec<ObjectRef>,
        area: IssueArea,
        pin_mux: Option<PinMuxTarget>,
    },
    Fix(Fix),
}

/// The instance whose pin-mux panel a `config.*` issue opens, and the pin
/// it focuses: the first instance `refs` name, with its first named pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinMuxTarget {
    pub instance: Id,
    pub terminal: Option<TerminalId>,
}

/// The pin-mux panel a located issue opens: only `config.*` codes (pin
/// functions, requirements, rules and pin options) do.
pub fn pin_mux_target(code: &str, refs: &[ObjectRef]) -> Option<PinMuxTarget> {
    if !code.starts_with("config.") {
        return None;
    }
    let instance = refs.iter().find_map(|r| match *r {
        ObjectRef::Instance { id } => Some(id),
        ObjectRef::Terminal { endpoint } => Some(endpoint.instance),
        _ => None,
    })?;
    let terminal = refs.iter().find_map(|r| match *r {
        ObjectRef::Terminal { endpoint } if endpoint.instance == instance => {
            Some(endpoint.terminal)
        }
        _ => None,
    });
    Some(PinMuxTarget { instance, terminal })
}

/// The fold key of a row: core's group code and key.
fn group_key(row: &IssueRow) -> String {
    format!("{}\u{1f}{}", row.group.code, row.group.key)
}

/// A row's identity across frames and revisions, for its disclosure and the
/// pill: its group, title and the objects of its primary finding.
fn row_key(row: &IssueRow) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{:?}",
        group_key(row),
        row.title,
        row.finding.objects
    )
}

/// One group of the list as laid out: rows in core's order.
#[derive(Clone, Debug)]
struct Group {
    key: String,
    level: Level,
    title: String,
    /// Indices into the report's rows.
    rows: Vec<usize>,
    /// A group-wide Fix; its rows then carry none.
    fix: Option<Fix>,
}

/// Groups error and warning groups start open; to do and info start folded.
fn opens_by_default(level: Level) -> bool {
    matches!(level, Level::Error | Level::Warning)
}

/// The report's rows grouped by core's fold key, in core's order (most
/// severe first), keeping only `filter`'s severity when given. Nothing is
/// capped and no row is dropped other than by the filter.
fn groups(report: &IssueReport, filter: Option<Severity>) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    let mut at: BTreeMap<String, usize> = BTreeMap::new();
    for (index, row) in report.rows.iter().enumerate() {
        if filter.is_some_and(|s| s != row.severity) {
            continue;
        }
        let key = group_key(row);
        let level = Level::from(row.severity);
        match at.get(&key) {
            Some(&g) => {
                let group = &mut out[g];
                group.rows.push(index);
                group.level = group.level.min(level);
            }
            None => {
                at.insert(key.clone(), out.len());
                out.push(Group {
                    key,
                    level,
                    title: row.group.title.clone(),
                    rows: vec![index],
                    fix: None,
                });
            }
        }
    }
    for group in &mut out {
        let fixes: Vec<Option<Fix>> = group.rows.iter().map(|&i| fix(&report.rows[i])).collect();
        if let Some(Some(first)) = fixes.first()
            && first.group_wide()
            && fixes
                .iter()
                .all(|f| f.as_ref().map(Fix::label) == Some(first.label()))
        {
            group.fix = Some(first.clone());
        }
    }
    out
}

/// The rows of `groups` in list order, as the pill's ‹ › walk them.
fn flat(groups: &[Group]) -> Vec<usize> {
    groups.iter().flat_map(|g| g.rows.iter().copied()).collect()
}

/// The object names of every row of a report, from core, computed once per
/// report.
struct Names {
    report: Arc<IssueReport>,
    rows: Vec<Vec<String>>,
}

fn names_of(engine: &Engine, report: &Arc<IssueReport>) -> Names {
    let objects: Vec<ObjectRef> = report
        .rows
        .iter()
        .flat_map(|r| r.objects().copied())
        .collect();
    let mut all = pcbcraft_core::issues::object_names(engine.project(), engine.drafts(), &objects)
        .into_iter();
    let rows = report
        .rows
        .iter()
        .map(|r| all.by_ref().take(r.objects().count()).collect())
        .collect();
    Names {
        report: report.clone(),
        rows,
    }
}

/// The Issues surface's state: the card's expansion, scope, filter, folds
/// and disclosures, the issue shown on the canvas, and the cached answers.
#[derive(Default)]
pub struct Surface {
    expanded: bool,
    scope: Scope,
    filter: Option<Severity>,
    /// Groups whose fold differs from their default.
    toggled: BTreeSet<String>,
    /// Rows whose details are disclosed.
    open: BTreeSet<String>,
    shown: Option<Shown>,
    area: Slot,
    all: Slot,
    selection: Slot,
    names: Option<Names>,
    badges: Option<((u64, usize), Arc<Badges>)>,
    /// A row's right-click menu, by the row's key.
    menu: crate::context_menu::State<String>,
}

/// An issue row menu's actions: the row's own Locate and Fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAct {
    Locate,
    Fix,
}

/// The rows of an issue row's menu: Locate, and the row's Fix when the
/// row offers one (a Fix for its whole group sits on the group header).
pub fn row_menu(row: &IssueRow) -> crate::context_menu::Rows<RowAct> {
    use crate::context_menu::Entry;
    let mut rows = vec![Some(Entry::act("Locate", "zoom", RowAct::Locate, true))];
    if let Some(fix) = fix(row).filter(|f| !f.group_wide()) {
        rows.push(Some(Entry::act(fix.label(), "update", RowAct::Fix, true)));
    }
    rows
}

impl Surface {
    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// Expand or collapse the card (the bar, the ribbon toggle, the badge).
    pub fn toggle(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Expand the card (a toast's Show).
    pub fn open(&mut self) {
        self.expanded = true;
    }

    /// Expand the card on what a card's issue slot counted: the selected
    /// object, or for the Board or sheet card (`whole`) the workspace.
    pub fn show_slot(&mut self, whole: bool) {
        self.expanded = true;
        self.scope = if whole {
            Scope::Workspace
        } else {
            Scope::Selection
        };
    }

    /// The bar's counts: always the active workspace, whatever the card
    /// lists, so a blocker is never out of the count.
    pub fn summary(&mut self, engine: &Engine, workspace: Workspace) -> Summary {
        Summary::of(&self.area.get(
            engine,
            IssueScope::Area {
                area: area(workspace),
            },
        ))
    }

    /// The answer the card lists for its scope.
    fn listed(&mut self, engine: &Engine, workspace: Workspace, selection: &[ObjectRef]) -> Answer {
        match self.scope {
            Scope::Workspace => self.area.get(
                engine,
                IssueScope::Area {
                    area: area(workspace),
                },
            ),
            Scope::All => self.all.get(engine, IssueScope::All),
            Scope::Selection => self.selection.get(
                engine,
                IssueScope::Selection {
                    objects: selection.to_vec(),
                },
            ),
        }
    }

    fn names(&mut self, engine: &Engine, report: &Arc<IssueReport>) -> &[Vec<String>] {
        if !self
            .names
            .as_ref()
            .is_some_and(|n| Arc::ptr_eq(&n.report, report))
        {
            self.names = Some(names_of(engine, report));
        }
        self.names.as_ref().map_or(&[], |n| n.rows.as_slice())
    }

    fn is_open(&self, group: &Group) -> bool {
        opens_by_default(group.level) != self.toggled.contains(&group.key)
    }

    /// The Issues card: the always-visible bar and, expanded, the scope and
    /// filter, the grouped list (the card's one scroll) and a pinned footer
    /// with the coverage hint and Copy report.
    pub fn card(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        workspace: Workspace,
        selection: &[ObjectRef],
    ) -> Option<Request> {
        let t = Tokens::get();
        let top = ui.max_rect().top();
        let summary = self.summary(engine, workspace);
        let zero = IssueCounts::default();
        let (counts, elsewhere, tip) = match &summary {
            Summary::Counted { counts, elsewhere } => (counts, elsewhere.as_deref(), None),
            Summary::NotCounted(why) => (&zero, Some(NOT_COUNTED), Some(why.as_str())),
        };
        let mut bar = kit::issues_bar(ui, counts, self.expanded, elsewhere);
        if let Some(tip) = tip {
            bar = bar.on_hover_text(tip);
        }
        if bar.clicked() {
            self.toggle();
        }
        if !self.expanded {
            return None;
        }
        kit::rule(ui);
        let pad = egui::Margin::symmetric(t.space_8 as i8, t.space_4 as i8);
        egui::Frame::NONE.inner_margin(pad).show(ui, |ui| {
            self.toolbar(ui, workspace);
        });
        let answer = self.listed(engine, workspace, selection);
        let idle = nothing_selected(&summary, workspace);
        let full = ui.available_rect_before_wrap();
        let footer_height = t.control_height + 2.0 * t.space_4;
        let split = (full.bottom() - footer_height).max(full.top());
        let body = Rect::from_min_max(full.min, pos2(full.right(), split));
        let footer = Rect::from_min_max(pos2(full.left(), split), full.max);
        let mut request = None;
        let report = answer.report().cloned();
        let listed = ui.scope_builder(UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("issues-body")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(t.space_4 as i8, 0))
                        .show(ui, |ui| match &answer {
                            Answer::Refused(why) => {
                                kit::hint(ui, "The issues could not be counted", why);
                            }
                            Answer::Report(report) => {
                                request = self.list(ui, engine, report, selection, &idle);
                            }
                        });
                })
                .content_size
                .y
        });
        kit::card::remember_issues_wants(ui.ctx(), body.top() - top + listed.inner + footer_height);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(footer.shrink2(vec2(t.space_8, 0.0)))
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                t.secondary_hline(ui.painter(), footer.x_range(), footer.top(), t.dialog_rule);
                if let Some(report) = &report {
                    let copy = ui_kit::button(ui, "Copy report", ui_kit::Button::Small)
                        .on_hover_text("Copy every finding with its full text");
                    if copy.clicked() {
                        ui.ctx().copy_text(report_text(report));
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        kit::hint(ui, &report.coverage, "What these checks cover");
                    });
                }
            },
        );
        request
    }

    /// The scope and the severity filter: two dropdowns that share the
    /// column's width, so the card never grows wider than the column.
    fn toolbar(&mut self, ui: &mut Ui, workspace: Workspace) {
        let t = Tokens::get();
        let scopes = [
            (Scope::Workspace, format!("{} only", workspace.label())),
            (Scope::All, "Whole project".to_owned()),
            (Scope::Selection, "Selection".to_owned()),
        ];
        let filters = [
            (None, "Every kind".to_owned()),
            (Some(Severity::Error), "Errors".to_owned()),
            (Some(Severity::Warning), "Warnings".to_owned()),
            (Some(Severity::ToDo), "To do".to_owned()),
            (Some(Severity::Info), "Info".to_owned()),
        ];
        let half = ((ui.available_width() - t.space_4) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = t.space_4;
            ui.allocate_ui(vec2(half, t.control_height), |ui| {
                ui.set_max_width(half);
                kit::dropdown(ui, "issues-scope", &mut self.scope, &scopes);
            });
            ui.allocate_ui(vec2(half, t.control_height), |ui| {
                ui.set_max_width(half);
                kit::dropdown(ui, "issues-filter", &mut self.filter, &filters);
            });
        });
    }

    /// The grouped list, or why it is empty.
    fn list(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        report: &Arc<IssueReport>,
        selection: &[ObjectRef],
        idle: &str,
    ) -> Option<Request> {
        let groups = groups(report, self.filter);
        if groups.is_empty() {
            let empty = match (self.scope, self.filter, report.rows.is_empty()) {
                (Scope::Selection, _, _) if selection.is_empty() => idle.to_owned(),
                (_, Some(_), false) => {
                    "None of this kind; choose Every kind to see the rest".to_owned()
                }
                _ => "No issues".to_owned(),
            };
            kit::hint(ui, &empty, &report.coverage);
            return None;
        }
        let views: Vec<IssueGroupView> = groups
            .iter()
            .map(|g| IssueGroupView {
                level: g.level,
                title: g.title.clone(),
                rows: g.rows.len(),
                open: self.is_open(g),
                fix: g.fix.as_ref().map(|f| f.label().to_owned()),
            })
            .collect();
        self.names(engine, report);
        let names = self.names.as_ref().map_or(&[][..], |n| n.rows.as_slice());
        let open = &self.open;
        let event = kit::issues_list(ui, egui::Id::unique("issues-list"), &views, |g, r| {
            let index = groups[g].rows[r];
            let row = &report.rows[index];
            IssueRowView {
                level: row.severity.into(),
                title: row.title.clone(),
                count: row.findings(),
                fix: if groups[g].fix.is_some() {
                    None
                } else {
                    fix(row).map(|f| f.label().to_owned())
                },
                open: open.contains(&row_key(row)),
                detail: row.detail.clone(),
                objects: names.get(index).cloned().unwrap_or_default(),
            }
        });
        // The open row menu, about the row it opened on; a row gone from
        // the list closes it.
        let menu_row = self
            .menu
            .target()
            .and_then(|key| report.rows.iter().find(|r| row_key(r) == *key));
        let rows = menu_row.map(row_menu).unwrap_or_default();
        if let Some(crate::context_menu::Run::Act(act)) = self.menu.show(ui.ctx(), &rows)
            && let Some(row) = menu_row
        {
            return match act {
                RowAct::Locate => Some(self.show(row)),
                RowAct::Fix => fix(row).map(Request::Fix),
            };
        }
        match event? {
            IssueListEvent::Group(g) => {
                let key = &groups[g].key;
                if !self.toggled.remove(key) {
                    self.toggled.insert(key.clone());
                }
                None
            }
            IssueListEvent::GroupFix(g) => groups[g].fix.clone().map(Request::Fix),
            IssueListEvent::Row { group, row, action } => {
                let issue = &report.rows[groups[group].rows[row]];
                if let Some(at) = action.menu {
                    self.menu.open(at, row_key(issue));
                    // It draws from the next frame.
                    ui.ctx().request_repaint();
                    return None;
                }
                if action.toggle {
                    let key = row_key(issue);
                    if !self.open.remove(&key) {
                        self.open.insert(key);
                    }
                    return None;
                }
                if action.fix {
                    return fix(issue).map(Request::Fix);
                }
                if let Some(object) = action.object {
                    let refs: Vec<ObjectRef> =
                        issue.objects().copied().skip(object).take(1).collect();
                    return Some(Request::Locate {
                        pin_mux: pin_mux_target(&issue.finding.code, &refs),
                        refs,
                        area: issue.area,
                    });
                }
                action.locate.then(|| self.show(issue))
            }
        }
    }

    /// Show `row` on the canvas: remember it for the pill and locate it.
    fn show(&mut self, row: &IssueRow) -> Request {
        self.shown = Some(Shown {
            key: row_key(row),
            title: row.title.clone(),
            level: row.severity.into(),
        });
        let refs: Vec<ObjectRef> = row.objects().copied().collect();
        Request::Locate {
            pin_mux: pin_mux_target(&row.finding.code, &refs),
            refs,
            area: row.area,
        }
    }

    /// The "Showing: …" pill at the top of the canvas for the issue located
    /// last, with ‹ › to step through the listed issues and × to end it.
    /// It ends by itself once the issue is gone.
    pub fn pill(
        &mut self,
        ui: &mut Ui,
        canvas: Rect,
        engine: &Engine,
        workspace: Workspace,
        selection: &[ObjectRef],
    ) -> Option<Request> {
        let shown = self.shown.clone()?;
        let answer = self.listed(engine, workspace, selection);
        let Some(report) = answer.report() else {
            self.shown = None;
            return None;
        };
        let order = flat(&groups(report, self.filter));
        let Some(at) = order
            .iter()
            .position(|&i| row_key(&report.rows[i]) == shown.key)
        else {
            self.shown = None;
            return None;
        };
        let t = Tokens::get();
        let text = crate::refusal::clip(
            &format!("Showing: {}", shown.title),
            pcbcraft_core::issues::MAX_TITLE,
        );
        // The pill takes the width it is given, so the row is sized from
        // the text: ‹, the glyph, the text, ×, › and their padding.
        let text_width = ui
            .painter()
            .layout_no_wrap(
                text.clone(),
                egui::FontId::proportional(t.font_label),
                t.text,
            )
            .size()
            .x;
        // Centred on the canvas, but kept left of the badge's corner.
        let left = canvas.left() + t.space_8;
        let right = (canvas.right() - t.space_8 - BADGE_RESERVE).max(left);
        let width = (text_width + PILL_CHROME).min(right - left).max(0.0);
        let x = (canvas.center().x - width / 2.0).clamp(left, (right - width).max(left));
        let rect = Rect::from_min_size(
            pos2(x, canvas.top() + t.space_8),
            vec2(width, t.control_height + t.space_8),
        );
        // ‹ and › keep fixed slots at the ends; the pill takes the middle.
        let arrow = t.control_height;
        let slot = |left: f32, right: f32, align: Align| {
            UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    pos2(left, rect.top()),
                    pos2(right.max(left), rect.bottom()),
                ))
                .layout(Layout::left_to_right(Align::Center).with_main_align(align))
        };
        let mut step = None;
        let mut previous = ui.new_child(slot(rect.left(), rect.left() + arrow, Align::Min));
        if ui_kit::button(&mut previous, "‹", ui_kit::Button::Enabled(at > 0))
            .on_hover_text("Previous issue")
            .clicked()
        {
            step = at.checked_sub(1);
        }
        let mut middle = ui.new_child(slot(
            rect.left() + arrow + t.space_4,
            rect.right() - arrow - t.space_4,
            Align::Min,
        ));
        let pill = kit::pill(&mut middle, Some(shown.level), &text);
        let mut next = ui.new_child(slot(rect.right() - arrow, rect.right(), Align::Max));
        if ui_kit::button(
            &mut next,
            "›",
            ui_kit::Button::Enabled(at + 1 < order.len()),
        )
        .on_hover_text("Next issue")
        .clicked()
        {
            step = Some(at + 1).filter(|n| *n < order.len());
        }
        if pill.closed {
            self.shown = None;
            return None;
        }
        let report = report.clone();
        if let Some(next) = step.and_then(|n| order.get(n)) {
            return Some(self.show(&report.rows[*next]));
        }
        pill.clicked.then(|| self.show(&report.rows[order[at]]))
    }

    /// The canvas count badge, top right: the workspace's counts, To do
    /// hidden while errors exist. A click expands or collapses Issues. It
    /// is left out while nothing is counted or nothing is open. Returns
    /// where it is drawn.
    pub fn badge(
        &mut self,
        ui: &mut Ui,
        canvas: Rect,
        engine: &Engine,
        workspace: Workspace,
    ) -> Option<Rect> {
        let counts = self.summary(engine, workspace).counts()?;
        if counts.error + counts.warning + counts.to_do == 0 {
            return None;
        }
        let t = Tokens::get();
        let rect = Rect::from_min_max(
            pos2(canvas.left(), canvas.top() + t.space_8),
            pos2(
                canvas.right() - t.space_8,
                canvas.top() + t.space_8 + t.control_height,
            ),
        );
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::right_to_left(Align::Min)),
        );
        let badge = kit::count_badge(&mut child, &counts, true);
        if badge.clicked() {
            self.toggle();
        }
        Some(badge.rect)
    }

    /// The ribbon's Issues toggle: the workspace's counts, lit while the
    /// card is expanded.
    pub fn ribbon_toggle(
        &mut self,
        engine: &Engine,
        workspace: Workspace,
    ) -> crate::shell::IssuesToggle {
        crate::shell::IssuesToggle {
            counts: self.summary(engine, workspace).counts(),
            expanded: self.expanded,
        }
    }

    /// The Browser's severity dots, from every issue of the project.
    pub fn badges(&mut self, engine: &Engine) -> Arc<Badges> {
        let at = document(engine);
        if let Some((key, badges)) = &self.badges
            && *key == at
        {
            return badges.clone();
        }
        let badges = Arc::new(match self.all.get(engine, IssueScope::All) {
            Answer::Report(report) => Badges::of(engine, &report),
            Answer::Refused(_) => Badges::default(),
        });
        self.badges = Some((at, badges.clone()));
        badges
    }
}

/// The objects of the active editor's selection, for the Selection scope.
pub fn selection(
    workspace: Workspace,
    part: Option<DefinitionId>,
    instance: Option<Id>,
    track: Option<Id>,
    via: Option<Id>,
    net: Option<Id>,
) -> Vec<ObjectRef> {
    let mut out = Vec::new();
    match workspace {
        Workspace::Part => out.extend(part.map(|id| ObjectRef::Definition { id })),
        Workspace::Schematic | Workspace::Pcb => {
            out.extend(instance.map(|id| ObjectRef::Instance { id }));
            out.extend(track.map(|id| ObjectRef::Track { id }));
            out.extend(via.map(|id| ObjectRef::Via { id }));
            out.extend(net.map(|id| ObjectRef::Net { id }));
        }
    }
    out
}

/// Every finding of a report with its full text, for Copy report: each row
/// as its marker and title, then every raw finding it holds.
pub fn report_text(report: &IssueReport) -> String {
    let mut out = String::new();
    for row in &report.rows {
        let level = Level::from(row.severity);
        out.push_str(&format!("{} {}\n", level.marker(), row.title));
        for finding in std::iter::once(&row.finding).chain(&row.also) {
            out.push_str(&format!("    {}: {}\n", finding.code, finding.message));
        }
    }
    out.push_str(&format!(
        "{}. {}\n",
        kit::issues::counts_words(&report.counts, true, true),
        report.coverage
    ));
    out
}

/// The editors locate targets, one per workspace.
pub struct Editors<'a> {
    pub part: &'a mut dyn Locate,
    pub schematic: &'a mut dyn Locate,
    pub pcb: &'a mut dyn Locate,
}

/// Locate `refs` in the workspace that shows them: `preferred` (the
/// issue's area) first, then the others. Returns the workspace that showed
/// them, which the app makes active; `None` when none could.
pub fn route(
    editors: &mut Editors<'_>,
    engine: &Engine,
    refs: &[ObjectRef],
    preferred: Workspace,
) -> Option<Workspace> {
    if refs.is_empty() {
        return None;
    }
    let order =
        std::iter::once(preferred).chain(Workspace::ALL.into_iter().filter(|w| *w != preferred));
    for workspace in order {
        let target: &mut dyn Locate = match workspace {
            Workspace::Part => &mut *editors.part,
            Workspace::Schematic => &mut *editors.schematic,
            Workspace::Pcb => &mut *editors.pcb,
        };
        if target.locate(engine, refs) {
            return Some(workspace);
        }
    }
    None
}

/// The component lineage that owns each draft definition (its symbol,
/// package, footprint, binding and Parts), and every published component
/// and Part.
fn owners(engine: &Engine) -> BTreeMap<DefinitionId, DefinitionId> {
    let drafts = engine.drafts();
    let library = &engine.project().library;
    let mut out = BTreeMap::new();
    for component in library.latest_components() {
        out.insert(component.id, component.id);
    }
    for part in library.latest_parts() {
        out.insert(part.id, part.definition.component.id);
    }
    for draft in &drafts.items {
        match &draft.definition {
            Definition::Component(_) => {
                out.insert(draft.id, draft.id);
                if let Some(set) = crate::part_editor::WorkingSet::find(drafts, Some(draft.id))
                    .filter(|set| set.ids.component == draft.id)
                {
                    for id in set.ids.all() {
                        out.insert(id, draft.id);
                    }
                }
            }
            Definition::Part(part) => {
                out.insert(draft.id, part.component.id);
            }
            _ => {}
        }
    }
    out
}

/// The instance an object belongs to, if any.
fn instance_of(engine: &Engine, object: &ObjectRef) -> Option<Id> {
    match *object {
        ObjectRef::Instance { id }
        | ObjectRef::Pad { instance: id, .. }
        | ObjectRef::Unit { instance: id, .. }
        | ObjectRef::Property { instance: id, .. } => Some(id),
        ObjectRef::Terminal { endpoint } => Some(endpoint.instance),
        ObjectRef::Occurrence { id } => engine.project().occurrence(id).map(|o| o.instance),
        _ => None,
    }
}

/// One Browser row's issue mark: the most severe issue on the object, its
/// title and how many issues it has.
#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    pub level: Level,
    pub title: String,
    pub count: usize,
}

impl Badge {
    /// The row's hover text.
    pub fn tip(&self) -> String {
        match self.count {
            0 | 1 => self.title.clone(),
            n => format!("{} · and {} more", self.title, n - 1),
        }
    }
}

/// The Browser's issue marks by object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Badges {
    pub components: BTreeMap<DefinitionId, Badge>,
    pub instances: BTreeMap<Id, Badge>,
    pub placements: BTreeMap<Id, Badge>,
    pub nets: BTreeMap<Id, Badge>,
    pub tracks: BTreeMap<Id, Badge>,
    pub board: Option<Badge>,
}

fn mark<K: Ord>(map: &mut BTreeMap<K, Badge>, key: K, row: &IssueRow) {
    note(
        map.entry(key).or_insert_with(|| Badge {
            level: Level::Ok,
            title: String::new(),
            count: 0,
        }),
        row,
    );
}

fn note(badge: &mut Badge, row: &IssueRow) {
    let level = Level::from(row.severity);
    if badge.count == 0 || level < badge.level {
        badge.level = level;
        badge.title = row.title.clone();
    }
    badge.count += 1;
}

impl Badges {
    /// Every row marks each object it refers to once: Component rows their
    /// component, Schematic rows their instances, Board rows their
    /// placements and the board; nets and tracks from any row.
    pub fn of(engine: &Engine, report: &IssueReport) -> Self {
        let owners = owners(engine);
        let mut out = Self::default();
        for row in &report.rows {
            let mut components = BTreeSet::new();
            let mut instances = BTreeSet::new();
            let mut nets = BTreeSet::new();
            let mut tracks = BTreeSet::new();
            for object in row.objects() {
                match *object {
                    ObjectRef::Definition { id }
                    | ObjectRef::LibraryObject { definition: id, .. } => {
                        components.extend(owners.get(&id).copied());
                    }
                    ObjectRef::Net { id } => {
                        nets.insert(id);
                    }
                    ObjectRef::Track { id } => {
                        tracks.insert(id);
                    }
                    _ => {
                        instances.extend(instance_of(engine, object));
                    }
                }
            }
            match row.area {
                IssueArea::Component => {
                    for id in components {
                        mark(&mut out.components, id, row);
                    }
                }
                IssueArea::Schematic => {
                    for id in instances {
                        mark(&mut out.instances, id, row);
                    }
                }
                IssueArea::Board => {
                    for id in instances {
                        mark(&mut out.placements, id, row);
                    }
                    note(
                        out.board.get_or_insert_with(|| Badge {
                            level: Level::Ok,
                            title: String::new(),
                            count: 0,
                        }),
                        row,
                    );
                }
            }
            for id in nets {
                mark(&mut out.nets, id, row);
            }
            for id in tracks {
                mark(&mut out.tracks, id, row);
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "issues_tests.rs"]
mod tests;
