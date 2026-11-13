//! The board interface in the desktop (docs/architecture/board-interface.md;
//! slice B6): the Board card's Interface section, the Placement card's
//! Enclosure section, the two docked previews and the datums drawn in
//! Layout and 3D.
//!
//! **Interface section.** Its status in one chip ("Not published",
//! "Published · revision 3", "3 datums changed since revision 14",
//! "Broken reference"), the enclosure link with Link… or Update…, the
//! published design with Publish…, then the datums as a `kit::list`
//! (direction A): exported datums grouped as Board, Holes and Parts, each
//! with its kind and slot, then what may be exported but is not (board
//! parameters, part datums the package does not mark for the enclosure)
//! under "Not exported". A row's action marks it for export
//! (`interface.export`) or unmarks it (`interface.unexport`; ⌫ too); an
//! automatic datum is withdrawn by switching its rule off in "Automatic"
//! (`interface.auto`). Hovering or selecting a row lights its datum on the
//! canvas and in 3D.
//!
//! **Previews.** Publish… docks the publish preview as an impact review:
//! what changed since the last publish, datum by datum, and the linked
//! enclosure's dry run, the features that would fail first; then the tier
//! and the file. Update… docks the enclosure preview: the references,
//! board edges, holes and parts that would move. Both run off the UI
//! thread through the automation session's own requests
//! ([`super::interface_host`]); Apply sends the one typed command the
//! request records, one undo step. A preview older than the document reads
//! "The project changed · Recompute".
//!
//! Every list is bounded; every message names objects, never ids or codes.
use super::PcbEditor;
use super::datum_paint::{self, Look};
use super::interface_host::{self, Done, Job, Running};
use crate::kit::{
    self,
    card::CardResponse,
    dialog::{DialogAction, DialogPanel},
    list::{self, Action, Columns, Effect, Layout, Line, ListState, ListView, Row, RowKey},
    review::{ReviewAction, ReviewGroup, ReviewLine, ReviewPanel},
    severity::Level,
    toast,
};
use egui::Ui;
use pcbcraft_core::interface::housing::{
    self as housing, FileCheck, HousingPreview, HousingStatus, LinkState, PreviewParams,
    UpdateParams,
};
use pcbcraft_core::interface::{
    DatumRow, DatumStatus, EffectKind, ExportTarget, InterfaceCommand, InterfaceList,
    PublishParams, PublishPreview, PublishPreviewParams, list as datums,
};
use pcbcraft_core::library::datum::{DatumKind, DatumName};
use pcbcraft_core::model::interface::{
    AutoRule, DatumId, DatumOwner, DatumRole, DatumSource, InterfaceDatum, Tier,
};
use pcbcraft_core::{Engine, Id, Operation, Project};
use std::path::{Path, PathBuf};

/// Something that may be exported and is not: a row under "Not exported".
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Candidate {
    pub target: ExportTarget,
    pub name: String,
    pub kind: DatumKind,
}

/// The interface as the card shows it, for one document revision.
#[derive(Clone, Debug)]
pub(super) struct Shown {
    revision: u64,
    pub list: InterfaceList,
    /// The live datums' stored identity, in the list's (slot) order.
    pub keys: Vec<InterfaceDatum>,
    pub candidates: Vec<Candidate>,
    pub housing: HousingStatus,
}

/// The card's status chip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum State {
    Unpublished,
    Published { revision: u32 },
    Stale { revision: u32, changed: usize },
    Broken,
}

impl State {
    /// What the interface's state is: broken while a reference the
    /// enclosure or the interface uses no longer resolves, stale while
    /// datums changed since the last publish.
    pub(super) fn of(list: &InterfaceList, housing: &HousingStatus) -> Self {
        let lost = list.datums.iter().any(|d| d.status == DatumStatus::Lost);
        if housing.state == LinkState::BrokenReference || (lost && list.published.is_some()) {
            return Self::Broken;
        }
        match list.published {
            None => Self::Unpublished,
            Some(revision) if list.changed > 0 => Self::Stale {
                revision,
                changed: list.changed,
            },
            Some(revision) => Self::Published { revision },
        }
    }

    pub(super) fn words(&self) -> String {
        match self {
            Self::Unpublished => "Not published".into(),
            Self::Published { revision } => format!("Published · revision {revision}"),
            Self::Stale { revision, changed } => format!(
                "{changed} datum{} changed since revision {revision}",
                if *changed == 1 { "" } else { "s" }
            ),
            Self::Broken => "Broken reference · fabrication output is blocked".into(),
        }
    }

    pub(super) fn level(&self) -> Option<Level> {
        match self {
            Self::Unpublished => None,
            Self::Published { .. } => Some(Level::Ok),
            Self::Stale { .. } => Some(Level::Warning),
            Self::Broken => Some(Level::Error),
        }
    }
}

/// Which file a picker is choosing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    Design,
    Enclosure,
}

/// A docked preview and what it is waiting for.
pub(super) struct Review {
    pub(super) kind: ReviewKind,
    running: Option<Running>,
    acked: Vec<bool>,
    /// The last request's refusal, in shown words.
    refusal: Option<String>,
    /// The body starts at the top on its first frame.
    opening: bool,
}

pub(super) enum ReviewKind {
    Publish {
        tier: Tier,
        preview: Option<PublishPreview>,
        /// Apply was pressed: the design is being built and written.
        publishing: bool,
    },
    Housing {
        preview: Option<HousingPreview>,
        updating: bool,
    },
}

/// The interface's desktop state (`PcbEditor::interface`).
#[derive(Default)]
pub(super) struct InterfaceUi {
    shown: Option<Shown>,
    /// The selected datum, lit in Layout and 3D.
    pub(super) datum: Option<DatumId>,
    /// The datum whose row is under the pointer, and the frame.
    hover: Option<(DatumId, u64)>,
    /// The published design, once chosen.
    design: Option<PathBuf>,
    pub(super) review: Option<Review>,
    picking: Option<(Pick, std::sync::mpsc::Receiver<Option<PathBuf>>)>,
    /// Linking an enclosure, which needs no preview.
    linking: Option<Running>,
    /// A design file was just chosen: preview against it next frame.
    pending_preview: bool,
    /// An enclosure file was just chosen: link it next frame.
    pending_link: Option<PathBuf>,
    /// The Board group was folded once, as the list starts.
    folded_board: bool,
}

/// The rule's words in the Automatic section.
fn rule_words(rule: AutoRule) -> &'static str {
    match rule {
        AutoRule::Board => "Board frame, faces and thickness",
        AutoRule::Outline => "Board outline",
        AutoRule::Cutouts => "Cutouts and slots",
        AutoRule::Holes => "Mounting holes",
        AutoRule::ExternalDatums => "Part openings and axes for the enclosure",
        AutoRule::SideHeights => "Tallest part per side",
    }
}

pub(super) fn kind_words(kind: DatumKind) -> &'static str {
    match kind {
        DatumKind::Frame => "frame",
        DatumKind::Plane => "plane",
        DatumKind::Axis => "axis",
        DatumKind::Profile => "profile",
        DatumKind::Volume => "volume",
        DatumKind::Parameter => "parameter",
    }
}

fn kind_icon(kind: DatumKind) -> &'static str {
    match kind {
        DatumKind::Frame => "origin",
        DatumKind::Plane => "plane",
        DatumKind::Axis => "hole",
        DatumKind::Profile => "polygon",
        DatumKind::Volume => "bounding_solid",
        DatumKind::Parameter => "params",
    }
}

/// The group a live datum is listed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Group {
    Board,
    Holes,
    Parts,
    NotExported,
}

impl Group {
    fn words(self) -> &'static str {
        match self {
            Self::Board => "Board",
            Self::Holes => "Holes",
            Self::Parts => "Parts",
            Self::NotExported => "Not exported",
        }
    }

    fn of(key: &InterfaceDatum) -> Self {
        match (&key.key.owner, key.key.role) {
            (_, DatumRole::Hole) => Self::Holes,
            (DatumOwner::Instance { .. }, _) => Self::Parts,
            _ => Self::Board,
        }
    }
}

/// What may be exported and is not, for the Board card: board parameters
/// and part datums the package does not mark for the enclosure. A part's
/// frame and envelope are offered on its own Placement card.
pub(super) fn candidates(project: &Project, keys: &[InterfaceDatum]) -> Vec<Candidate> {
    let mut out = Vec::new();
    let exported_param = |name: &str| {
        keys.iter()
            .any(|k| k.key.role == DatumRole::BoardParameter && k.key.name.as_deref() == Some(name))
    };
    if let Some(sketch) = super::board_shape_ui::sketch_of(project) {
        for p in &sketch.parameters {
            if p.name.valid() && !exported_param(&p.name.0) {
                out.push(Candidate {
                    target: ExportTarget::BoardParameter {
                        name: p.name.clone(),
                    },
                    name: format!("Board parameter {}", p.name.0),
                    kind: DatumKind::Parameter,
                });
            }
        }
    }
    for placement in &project.placements {
        let instance = placement.instance;
        for d in pcbcraft_core::interface::package::placed(project, instance) {
            if d.external || exported_datum(keys, instance, &d.name).is_some() {
                continue;
            }
            out.push(Candidate {
                name: part_datum_words(project, instance, &d.name),
                kind: d.geometry.kind(),
                target: ExportTarget::PackageDatum {
                    instance,
                    name: d.name,
                },
            });
        }
    }
    out
}

fn designator(project: &Project, instance: Id) -> String {
    project
        .instance(instance)
        .map_or_else(|| "A part".into(), |i| i.designator.clone())
}

fn part_datum_words(project: &Project, instance: Id, name: &DatumName) -> String {
    format!(
        "{} {}",
        designator(project, instance),
        name.0.replace('_', " ")
    )
}

/// The live datum exporting `instance`'s package datum `name`.
fn exported_datum<'a>(
    keys: &'a [InterfaceDatum],
    instance: Id,
    name: &DatumName,
) -> Option<&'a InterfaceDatum> {
    keys.iter().find(|k| {
        k.key.role == DatumRole::PartDatum
            && k.key.owner == DatumOwner::Instance { id: instance }
            && k.key.name.as_deref() == Some(name.0.as_str())
    })
}

/// The interface of `engine`'s document, for the card.
pub(super) fn shown(engine: &Engine) -> Result<Shown, String> {
    let project = engine.project();
    let list = datums::list(project).map_err(|e| crate::refusal::shown_error(&e))?;
    let keys = datums::table(project)
        .map_err(|e| crate::refusal::shown_error(&e))?
        .datums
        .clone();
    let candidates = candidates(project, &keys);
    Ok(Shown {
        revision: engine.revision(),
        housing: housing::status(project, FileCheck::NotChecked),
        list,
        keys,
        candidates,
    })
}

/// The file name of `path`, for a summary.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// A path as the link stores it, read against the project's folder when
/// relative.
fn resolved(project: Option<&Path>, given: &str) -> PathBuf {
    let path = Path::new(given);
    match project.and_then(Path::parent) {
        Some(folder) if path.is_relative() && !folder.as_os_str().is_empty() => folder.join(path),
        _ => path.to_path_buf(),
    }
}

/// One shown line of the datum list.
#[derive(Clone, Debug)]
enum Item {
    Header(Group, usize),
    Datum(usize),
    Candidate(usize),
}

/// The list's lines: each group's header and, open, its rows.
fn items(shown: &Shown, folded: &std::collections::BTreeSet<RowKey>) -> Vec<(RowKey, Item)> {
    let mut out = Vec::new();
    for group in [Group::Board, Group::Holes, Group::Parts] {
        let rows: Vec<usize> = shown
            .keys
            .iter()
            .enumerate()
            .filter(|(_, k)| Group::of(k) == group)
            .map(|(i, _)| i)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let key = RowKey::of(("interface-group", group));
        out.push((key, Item::Header(group, rows.len())));
        if folded.contains(&key) {
            continue;
        }
        for i in rows {
            if let Some(row) = shown.list.datums.get(i) {
                out.push((datum_key(row.id), Item::Datum(i)));
            }
        }
    }
    if !shown.candidates.is_empty() {
        let key = RowKey::of(("interface-group", Group::NotExported));
        out.push((
            key,
            Item::Header(Group::NotExported, shown.candidates.len()),
        ));
        if !folded.contains(&key) {
            for (i, c) in shown.candidates.iter().enumerate() {
                out.push((candidate_key(&c.target), Item::Candidate(i)));
            }
        }
    }
    out
}

pub(super) fn datum_key(id: DatumId) -> RowKey {
    RowKey::of(("datum", id))
}

fn candidate_key(target: &ExportTarget) -> RowKey {
    RowKey::of((
        "candidate",
        serde_json::to_string(target).unwrap_or_default(),
    ))
}

fn list_id() -> egui::Id {
    egui::Id::unique("pcb-interface-list")
}

pub(super) fn review_id() -> egui::Id {
    egui::Id::unique("pcb-interface-review")
}

/// A datum row's secondary text: its kind, slot and, for an automatic
/// datum, that a rule exports it.
fn secondary(row: &DatumRow) -> String {
    let auto = match row.source {
        DatumSource::Auto { .. } => " · automatic",
        DatumSource::User => "",
    };
    format!("{} · slot {}{auto}", kind_words(row.kind), row.slot.0)
}

fn badge(row: &DatumRow, published: bool) -> Option<(Level, &'static str)> {
    match row.status {
        DatumStatus::Lost => Some((Level::Error, "Lost")),
        DatumStatus::Inexact => Some((Level::Warning, "Inexact")),
        DatumStatus::Resolved if published && row.changed => Some((Level::Info, "Changed")),
        DatumStatus::Resolved => None,
    }
}

/// The publish preview's summary line.
pub(super) fn publish_summary(preview: &PublishPreview) -> String {
    let changed = preview
        .changes
        .iter()
        .filter(|c| c.change != pcbcraft_core::interface::ChangeKind::Unchanged)
        .count();
    let datums = |n: usize| format!("{n} datum{}", if n == 1 { "" } else { "s" });
    match preview.from {
        None => format!("First publish · {}", datums(preview.changes.len())),
        Some(from) if from == preview.to => format!("Revision {from} · nothing changed"),
        Some(from) => format!(
            "Revision {from} → {} · {} changed",
            preview.to,
            datums(changed)
        ),
    }
}

/// The publish preview's blockers: the enclosure features the dry run says
/// would fail or lose a joint, each confirmed to publish anyway, and those
/// on a face that is not exported, as warnings.
pub(super) fn publish_blockers(preview: &PublishPreview) -> Vec<ReviewLine> {
    let Some(dry) = &preview.dry_run else {
        return Vec::new();
    };
    dry.effects
        .iter()
        .filter_map(|e| match e.effect {
            EffectKind::Fails | EffectKind::JointUnsolved => Some(ReviewLine {
                level: Level::Error,
                title: e.message.clone(),
                ack: Some(format!(
                    "Publish anyway: ‘{}’ will fail in SolidCraft",
                    e.feature
                )),
                locate: false,
            }),
            EffectKind::UnexportedReference => Some(ReviewLine {
                level: Level::Warning,
                title: e.message.clone(),
                ack: None,
                locate: false,
            }),
            EffectKind::Moves => None,
        })
        .collect()
}

/// The publish preview's change groups: the datums that change, and the
/// enclosure features that follow them.
pub(super) fn publish_groups(preview: &PublishPreview) -> Vec<ReviewGroup> {
    let lines: Vec<String> = preview
        .changes
        .iter()
        .filter(|c| c.change != pcbcraft_core::interface::ChangeKind::Unchanged)
        .map(|c| c.summary.clone())
        .collect();
    let mut groups = vec![ReviewGroup {
        title: "Datums".into(),
        summary: String::new(),
        lines,
    }];
    if let Some(dry) = &preview.dry_run {
        groups.push(ReviewGroup {
            title: format!("Enclosure · {}", file_name(&dry.enclosure)),
            summary: String::new(),
            lines: dry
                .effects
                .iter()
                .filter(|e| e.effect == EffectKind::Moves)
                .map(|e| e.message.clone())
                .collect(),
        });
    }
    groups
}

/// The enclosure preview's summary, blockers and groups.
pub(super) fn housing_content(
    preview: &HousingPreview,
) -> (String, Vec<ReviewLine>, Vec<ReviewGroup>) {
    if let Some(why) = &preview.refused {
        return (
            "The update would be refused".into(),
            vec![ReviewLine {
                level: Level::Error,
                title: why.clone(),
                ack: None,
                locate: false,
            }],
            Vec::new(),
        );
    }
    let moved = preview.objects.len();
    let summary = if !preview.changed && moved == 0 {
        "The enclosure is as the board last read it".into()
    } else if moved == 0 {
        "The enclosure changed · nothing on the board moves".into()
    } else {
        format!(
            "The enclosure changed · {moved} board object{} move",
            if moved == 1 { "" } else { "s" }
        )
    };
    let blockers = preview
        .references
        .iter()
        .filter(|r| r.to != pcbcraft_core::model::dimension::ReferenceStatus::Resolved)
        .map(|r| ReviewLine {
            level: Level::Warning,
            title: r.summary.clone(),
            ack: None,
            locate: false,
        })
        .collect();
    let mut groups = vec![
        ReviewGroup {
            title: "Board".into(),
            summary: String::new(),
            lines: preview.objects.iter().map(|o| o.summary.clone()).collect(),
        },
        ReviewGroup {
            title: "References".into(),
            summary: String::new(),
            lines: preview
                .references
                .iter()
                .map(|r| r.summary.clone())
                .collect(),
        },
    ];
    if !preview.consumers.is_empty() {
        groups.push(ReviewGroup {
            title: "Enclosure features".into(),
            summary: String::new(),
            lines: preview.consumers.clone(),
        });
    }
    (summary, blockers, groups)
}

impl PcbEditor {
    /// The interface of the document now, computed once per revision.
    pub(super) fn interface_shown(&mut self, engine: &Engine) -> Option<&Shown> {
        if self
            .interface
            .shown
            .as_ref()
            .is_none_or(|s| s.revision != engine.revision())
        {
            self.interface.shown = shown(engine).ok();
        }
        self.interface.shown.as_ref()
    }

    /// The published design: the one chosen, the linked enclosure's, or
    /// `<project>-board.solidcraft` beside a saved project.
    fn design_path(&self, project: &Project) -> Option<PathBuf> {
        if let Some(path) = &self.interface.design {
            return Some(path.clone());
        }
        let file = self.project_path.as_deref();
        if let Some(link) = project.housing.as_deref() {
            return Some(resolved(file, &link.design_hint));
        }
        let file = file?;
        let stem = file.file_stem()?.to_string_lossy().into_owned();
        Some(file.with_file_name(format!("{stem}-board.solidcraft")))
    }

    /// The datum lit as if selected: its row under the pointer, this frame
    /// or the last, or the selected one.
    fn hot_datum(&self, ctx: &egui::Context) -> Option<DatumId> {
        self.interface
            .hover
            .filter(|(_, frame)| frame + 1 >= ctx.cumulative_frame_nr())
            .map(|(id, _)| id)
            .or(self.interface.datum)
    }

    /// Whether the interface shows on the canvas: the Board card or a
    /// preview is open, or a part with datums is selected.
    fn interface_on_canvas(&self, project: &Project) -> bool {
        self.interface.review.is_some()
            || matches!(self.inspected(project), super::Inspected::Board)
    }

    /// The datums on the Layout canvas or in 3D, after the view is drawn.
    pub(super) fn paint_datums(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        surface: &dyn super::dimensions::Surface,
        engine: &Engine,
    ) {
        let hot = self.hot_datum(ui.ctx());
        let board = self.interface_on_canvas(engine.project());
        let selected: Vec<Id> = self
            .instance
            .into_iter()
            .chain(self.also.iter().copied())
            .collect();
        let Some(shown) = self.interface_shown(engine) else {
            return;
        };
        let rows: Vec<(&DatumRow, Look)> = shown
            .list
            .datums
            .iter()
            .zip(&shown.keys)
            .filter_map(|(row, key)| {
                let look = if hot == Some(row.id) {
                    Look::Hot
                } else {
                    Look::Quiet
                };
                let part = match key.key.owner {
                    DatumOwner::Instance { id } => selected.contains(&id),
                    _ => false,
                };
                let shows =
                    look == Look::Hot || ((board || part) && !datum_paint::traces_the_board(row));
                shows.then_some((row, look))
            })
            .collect();
        datum_paint::paint(painter, surface, &rows);
    }

    /// Take what the interface's requests gave back: an operation to apply
    /// now (a publish's record, an enclosure's read), once per result.
    pub(super) fn interface_poll(&mut self, ctx: &egui::Context) -> Option<Operation> {
        self.poll_picker();
        if let Some(running) = &mut self.interface.linking
            && let Some(done) = running.poll()
        {
            self.interface.linking = None;
            match done {
                Done::HousingRead(Ok(operation)) => {
                    toast::post(ctx, Level::Ok, "Linked the enclosure", Vec::new());
                    return Some(operation);
                }
                Done::HousingRead(Err(why))
                | Done::PublishPreview(Err(why))
                | Done::Published(Err(why))
                | Done::HousingPreview(Err(why)) => {
                    toast::post(
                        ctx,
                        Level::Error,
                        format!("Could not link: {why}"),
                        Vec::new(),
                    );
                }
                _ => {}
            }
        }
        let review = self.interface.review.as_mut()?;
        let done = review.running.as_mut()?.poll()?;
        review.running = None;
        self.interface_done(ctx, done)
    }

    /// A finished request's result, in the open review.
    fn interface_done(&mut self, ctx: &egui::Context, done: Done) -> Option<Operation> {
        let review = self.interface.review.as_mut()?;
        match (&mut review.kind, done) {
            (ReviewKind::Publish { preview, .. }, Done::PublishPreview(result)) => match result {
                Ok(p) => {
                    review.acked = vec![false; publish_blockers(&p).len()];
                    review.refusal = None;
                    *preview = Some(p);
                }
                Err(why) => review.refusal = Some(why),
            },
            (ReviewKind::Publish { publishing, .. }, Done::Published(result)) => {
                *publishing = false;
                match result {
                    Ok((operation, revision)) => {
                        self.interface.review = None;
                        toast::post(
                            ctx,
                            Level::Ok,
                            format!("Published the board interface, revision {revision}"),
                            Vec::new(),
                        );
                        return Some(operation);
                    }
                    Err(why) => review.refusal = Some(why),
                }
            }
            (ReviewKind::Housing { preview, .. }, Done::HousingPreview(result)) => match result {
                Ok(p) => {
                    review.acked = vec![false; housing_content(&p).1.len()];
                    review.refusal = None;
                    *preview = Some(p);
                }
                Err(why) => review.refusal = Some(why),
            },
            (ReviewKind::Housing { updating, .. }, Done::HousingRead(result)) => {
                *updating = false;
                match result {
                    Ok(operation) => {
                        self.interface.review = None;
                        toast::post(ctx, Level::Ok, "Updated from the enclosure", Vec::new());
                        return Some(operation);
                    }
                    Err(why) => review.refusal = Some(why),
                }
            }
            (_, other) => {
                review.refusal = Some(match other {
                    Done::PublishPreview(Err(w))
                    | Done::Published(Err(w))
                    | Done::HousingPreview(Err(w))
                    | Done::HousingRead(Err(w)) => w,
                    _ => "The request answered something else".into(),
                });
            }
        }
        None
    }

    /// Hand a finished request to the open review (tests).
    #[cfg(test)]
    pub(super) fn interface_answer(
        &mut self,
        ctx: &egui::Context,
        done: Done,
    ) -> Option<Operation> {
        if let Some(review) = &mut self.interface.review {
            review.running = None;
        }
        self.interface_done(ctx, done)
    }

    /// Let the open review wait on `running` (tests).
    #[cfg(test)]
    pub(super) fn interface_hold(&mut self, running: Running) {
        if let Some(review) = &mut self.interface.review {
            review.running = Some(running);
        }
    }

    /// Choose the open publish preview's tier, as its Contents row does,
    /// and compute the preview again (tests).
    #[cfg(test)]
    pub(super) fn interface_set_tier(&mut self, ctx: &egui::Context, engine: &Engine, to: Tier) {
        if let Some(Review {
            kind: ReviewKind::Publish { tier, .. },
            ..
        }) = &mut self.interface.review
        {
            *tier = to;
        }
        self.preview_publish(ctx, engine);
    }

    /// Whether the open publish preview is publishing (tests).
    #[cfg(test)]
    pub(super) fn interface_publishing(&self) -> bool {
        matches!(
            self.interface.review.as_ref().map(|r| &r.kind),
            Some(ReviewKind::Publish {
                publishing: true,
                ..
            })
        )
    }

    /// Whether a request of the open review is running.
    #[cfg(test)]
    pub(super) fn interface_waiting(&self) -> bool {
        self.interface
            .review
            .as_ref()
            .is_some_and(|r| r.running.is_some())
    }

    fn poll_picker(&mut self) {
        let Some((pick, receiver)) = &self.interface.picking else {
            return;
        };
        let pick = *pick;
        match receiver.try_recv() {
            Ok(path) => {
                self.interface.picking = None;
                if let Some(path) = path {
                    match pick {
                        Pick::Design => {
                            self.interface.design = Some(path);
                            // The comparison is with the file now chosen.
                            if let Some(review) = &mut self.interface.review {
                                review.refusal = None;
                                if let ReviewKind::Publish { preview, .. } = &mut review.kind {
                                    *preview = None;
                                }
                            }
                            self.interface.pending_preview = true;
                        }
                        Pick::Enclosure => self.interface.pending_link = Some(path),
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.interface.picking = None,
        }
    }

    fn start_pick(&mut self, ctx: &egui::Context, pick: Pick, proposed: Option<&Path>) {
        if self.interface.picking.is_some() {
            return;
        }
        let mut dialog =
            rfd::AsyncFileDialog::new().add_filter("SolidCraft design", &["solidcraft"]);
        if let Some(folder) = proposed.and_then(Path::parent) {
            dialog = dialog.set_directory(folder);
        }
        let future: std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<rfd::FileHandle>> + Send>,
        > = match pick {
            Pick::Design => {
                let name = proposed.and_then(Path::file_name).map_or_else(
                    || "board.solidcraft".into(),
                    |n| n.to_string_lossy().into_owned(),
                );
                Box::pin(
                    dialog
                        .set_title("Publish the board interface as")
                        .set_file_name(name)
                        .save_file(),
                )
            }
            Pick::Enclosure => {
                Box::pin(dialog.set_title("Choose the enclosure design").pick_file())
            }
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        if std::thread::Builder::new()
            .name("pcbcraft-interface-picker".into())
            .spawn(move || {
                let picked = pollster::block_on(future).map(|f| f.path().to_path_buf());
                let _ = sender.send(picked);
                repaint.request_repaint();
            })
            .is_ok()
        {
            self.interface.picking = Some((pick, receiver));
        }
    }

    /// Open the publish preview, and compute it.
    pub(super) fn open_publish(&mut self, ctx: &egui::Context, engine: &Engine) {
        let tier = engine
            .project()
            .interface
            .published
            .as_ref()
            .map_or(Tier::Mechanical, |r| r.tier);
        self.interface.review = Some(Review {
            kind: ReviewKind::Publish {
                tier,
                preview: None,
                publishing: false,
            },
            running: None,
            acked: Vec::new(),
            refusal: None,
            opening: true,
        });
        self.preview_publish(ctx, engine);
    }

    fn preview_publish(&mut self, ctx: &egui::Context, engine: &Engine) {
        let project = engine.project();
        let design = self.design_path(project);
        let enclosure = project
            .housing
            .as_deref()
            .map(|l| resolved(self.project_path.as_deref(), &l.enclosure_hint));
        let project_path = self.project_path.clone();
        let Some(review) = &mut self.interface.review else {
            return;
        };
        let ReviewKind::Publish { tier, preview, .. } = &mut review.kind else {
            return;
        };
        *preview = None;
        let params = PublishPreviewParams {
            tier: Some(*tier),
            path: design
                .filter(|d| d.exists())
                .map(|d| d.to_string_lossy().into_owned()),
            enclosure_path: enclosure.map(|e| e.to_string_lossy().into_owned()),
        };
        let params = PublishPreviewParams {
            enclosure_path: params.enclosure_path.filter(|_| params.path.is_some()),
            ..params
        };
        review.running =
            interface_host::start(ctx, engine, project_path, Job::PublishPreview(params));
    }

    /// Open the enclosure preview, and compute it.
    pub(super) fn open_housing(&mut self, ctx: &egui::Context, engine: &Engine) {
        let project = engine.project();
        let Some(link) = project.housing.as_deref() else {
            return;
        };
        let file = self.project_path.as_deref();
        let params = PreviewParams {
            path: resolved(file, &link.enclosure_hint)
                .to_string_lossy()
                .into_owned(),
            design: resolved(file, &link.design_hint)
                .to_string_lossy()
                .into_owned(),
        };
        let running = interface_host::start(
            ctx,
            engine,
            self.project_path.clone(),
            Job::HousingPreview(params),
        );
        self.interface.review = Some(Review {
            kind: ReviewKind::Housing {
                preview: None,
                updating: false,
            },
            running,
            acked: Vec::new(),
            refusal: None,
            opening: true,
        });
    }

    /// Link the enclosure at `path` to the published design.
    fn link_enclosure(&mut self, ctx: &egui::Context, engine: &Engine, path: &Path) {
        let Some(design) = self.design_path(engine.project()) else {
            return;
        };
        let params = UpdateParams {
            path: path.to_string_lossy().into_owned(),
            design: design.to_string_lossy().into_owned(),
            expect_hash: None,
            expected_revision: engine.revision(),
        };
        self.interface.linking = interface_host::start(
            ctx,
            engine,
            self.project_path.clone(),
            Job::HousingUpdate(params),
        );
    }

    /// INTERFACE, in the Board card: status, enclosure, publish, the datum
    /// list and the automatic rules.
    pub(super) fn interface_section(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let ctx = ui.ctx().clone();
        if self.interface.pending_preview {
            self.interface.pending_preview = false;
            self.preview_publish(&ctx, engine);
        }
        if let Some(path) = self.interface.pending_link.take() {
            self.link_enclosure(&ctx, engine, &path);
        }
        let project = engine.project();
        let design = self.design_path(project);
        let shown = self.interface_shown(engine)?.clone();
        let state = State::of(&shown.list, &shown.housing);
        let mut operation = None;
        let mut publish = false;
        let mut update = false;
        let mut link = false;
        let count = shown.list.datums.len();
        let linking = self.interface.linking.is_some();
        kit::section(ui, "pcb-interface", "Interface", true, Some(count), |ui| {
            kit::row(ui, "Status", |ui| {
                kit::status_chip(ui, state.level(), &state.words());
            });
            kit::row(ui, "Published", |ui| {
                let summary = match (&design, shown.list.published) {
                    (Some(d), Some(_)) => file_name(&d.to_string_lossy()),
                    (_, None) => "Not yet".into(),
                    (None, Some(_)) => "Save the project to choose a file".into(),
                };
                publish = kit::summary_row(ui, &summary, Some("Publish…"));
            });
            kit::row(ui, "Enclosure", |ui| match project.housing.as_deref() {
                Some(l) => {
                    let summary =
                        format!("{} · {}", file_name(&l.enclosure_hint), shown.housing.words);
                    update = kit::summary_row(ui, &summary, Some("Update…"));
                }
                None if linking => {
                    kit::summary_row(ui, "Reading the enclosure…", None);
                }
                None if shown.list.published.is_none() => {
                    kit::summary_row(ui, "Publish first to link an enclosure", None);
                }
                None => link = kit::summary_row(ui, "No enclosure", Some("Link…")),
            });
            operation = self.datum_list(ui, project, &shown);
            kit::section(ui, "pcb-interface-auto", "Automatic", false, None, |ui| {
                for rule in shown.list.rules {
                    let mut on = rule.on;
                    if kit::check_row(ui, rule_words(rule.rule), &mut on) {
                        operation = Some(Operation::Interface(InterfaceCommand::Auto {
                            rule: rule.rule,
                            on,
                        }));
                    }
                }
            });
        });
        if publish {
            self.open_publish(&ctx, engine);
        }
        if update {
            self.open_housing(&ctx, engine);
        }
        if link {
            let proposed = design.clone();
            self.start_pick(&ctx, Pick::Enclosure, proposed.as_deref());
        }
        operation
    }

    /// The datums as a list: exported ones by group with Unexport, then
    /// what may be exported with Export.
    fn datum_list(&mut self, ui: &mut Ui, project: &Project, shown: &Shown) -> Option<Operation> {
        let id = list_id();
        let ctx = ui.ctx().clone();
        let mut state = ListState::load(&ctx, id);
        let wanted = self.interface.datum.map(datum_key);
        if state.selection.iter().next().copied() != wanted {
            state.selection = wanted.into_iter().collect();
        }
        // The board's own datums start folded, so the parts' show.
        if !self.interface.folded_board {
            self.interface.folded_board = true;
            state
                .folded
                .insert(RowKey::of(("interface-group", Group::Board)));
        }
        let folded = state.folded.clone();
        state.store(&ctx, id);
        let items = items(shown, &folded);
        if items.is_empty() {
            list::view::empty(
                ui,
                "Nothing exported",
                "Turn on an automatic rule below, or export a part's datum from its card",
            );
            return None;
        }
        let names: Vec<String> = items
            .iter()
            .map(|(_, item)| match item {
                Item::Header(g, _) => g.words().to_owned(),
                Item::Datum(i) => shown
                    .list
                    .datums
                    .get(*i)
                    .map_or_else(String::new, |r| r.name.clone()),
                Item::Candidate(i) => shown
                    .candidates
                    .get(*i)
                    .map_or_else(String::new, |c| c.name.clone()),
            })
            .collect();
        let secondaries: Vec<String> = items
            .iter()
            .map(|(_, item)| match item {
                Item::Datum(i) => shown.list.datums.get(*i).map(secondary).unwrap_or_default(),
                Item::Candidate(i) => shown
                    .candidates
                    .get(*i)
                    .map(|c| kind_words(c.kind).to_owned())
                    .unwrap_or_default(),
                Item::Header(..) => String::new(),
            })
            .collect();
        let labels: Vec<String> = items
            .iter()
            .zip(&names)
            .map(|((_, item), name)| match item {
                Item::Datum(_) => format!("Stop exporting {name}"),
                Item::Candidate(_) => format!("Export {name} to the enclosure"),
                Item::Header(..) => String::new(),
            })
            .collect();
        let lines: Vec<Line<'_>> = {
            let mut parent = None;
            items
                .iter()
                .zip(&names)
                .map(|((key, item), name)| match item {
                    Item::Header(..) => {
                        parent = Some(*key);
                        Line::header(*key, name, folded.contains(key))
                    }
                    _ => match parent {
                        Some(p) => Line::item(*key, name).under(p),
                        None => Line::item(*key, name),
                    },
                })
                .collect()
        };
        let published = shown.list.published.is_some();
        let view = ListView {
            multiple: false,
            delete: true,
            ..ListView::new(
                id,
                "Interface datums",
                Layout::Bounded {
                    rows: kit::LIST_MAX_ROWS,
                },
            )
        };
        let cols = Columns::default();
        let out = view.show(ui, &lines, |ui, rect, line, cx| {
            let at = items.iter().position(|(k, _)| *k == line.key);
            let item = at.and_then(|i| items.get(i)).map(|(_, item)| item.clone());
            let name = at.and_then(|i| names.get(i)).map_or("", String::as_str);
            let parts = [name];
            match item {
                Some(Item::Header(_, n)) => {
                    let response = list::row::header(
                        ui,
                        cx.id,
                        rect,
                        line.key,
                        name,
                        n,
                        line.fold == list::Fold::Folded,
                        cx.cursor,
                    );
                    list::RowResponse {
                        clicked: response.clicked(),
                        double_clicked: false,
                        hovered: response.hovered(),
                        fold: false,
                        action: None,
                        grip: None,
                        value_rect: rect,
                        response,
                    }
                }
                Some(item) => {
                    let label = at.and_then(|i| labels.get(i)).map_or("", String::as_str);
                    let (actions, kind, row_badge) = match &item {
                        Item::Datum(i) => {
                            let row = shown.list.datums.get(*i);
                            let user = row.is_some_and(|r| r.source == DatumSource::User);
                            (
                                if user {
                                    vec![Action::text("Unexport", label)]
                                } else {
                                    Vec::new()
                                },
                                row.map(|r| r.kind),
                                row.and_then(|r| badge(r, published)),
                            )
                        }
                        Item::Candidate(i) => (
                            vec![Action::text("Export", label)],
                            shown.candidates.get(*i).map(|c| c.kind),
                            None,
                        ),
                        Item::Header(..) => (Vec::new(), None, None),
                    };
                    let mut row = Row::new(line.key, &parts);
                    row.depth = 1;
                    row.fold_slot = true;
                    row.icon = kind.map(kind_icon);
                    row.secondary = at.and_then(|i| secondaries.get(i)).map(String::as_str);
                    row.badge = row_badge;
                    row.actions = &actions;
                    row.selected = cx.selected;
                    row.cursor = cx.cursor;
                    row.role = egui::Role::TreeItem;
                    list::row::paint(ui, cx.id, rect, &row, &cols)
                }
                None => list::row::paint(ui, cx.id, rect, &Row::new(line.key, &parts), &cols),
            }
        });
        let item_of = |k: &RowKey| items.iter().find(|(x, _)| x == k).map(|(_, i)| i.clone());
        let mut operation = None;
        let act = |item: Item| -> Option<Operation> {
            match item {
                Item::Datum(i) => shown
                    .list
                    .datums
                    .get(i)
                    .filter(|r| r.source == DatumSource::User)
                    .map(|r| Operation::Interface(InterfaceCommand::Unexport { datum: r.id })),
                Item::Candidate(i) => shown.candidates.get(i).map(|c| {
                    Operation::Interface(InterfaceCommand::Export {
                        target: c.target.clone(),
                        name: None,
                    })
                }),
                Item::Header(..) => None,
            }
        };
        for effect in &out.effects {
            match effect {
                Effect::Selected => {
                    self.interface.datum = out
                        .state
                        .selection
                        .iter()
                        .next()
                        .and_then(&item_of)
                        .and_then(|item| match item {
                            Item::Datum(i) => shown.list.datums.get(i).map(|r| r.id),
                            _ => None,
                        });
                }
                Effect::Activate(k) => {
                    if let Some(item @ Item::Candidate(_)) = item_of(k) {
                        operation = act(item);
                    }
                }
                Effect::Delete(gone) => {
                    if let Some(item @ Item::Datum(_)) = gone.first().and_then(&item_of) {
                        operation = act(item);
                    }
                }
                _ => {}
            }
        }
        for (k, _) in &out.actions {
            if let Some(item) = item_of(k) {
                operation = act(item);
            }
        }
        self.interface.hover = out
            .hovered
            .and_then(|k| item_of(&k))
            .and_then(|item| match item {
                Item::Datum(i) => shown.list.datums.get(i).map(|r| r.id),
                _ => None,
            })
            .map(|id| (id, ctx.cumulative_frame_nr()));
        let _ = project;
        operation
    }

    /// ENCLOSURE, in the Placement card: the part's frame, envelope and
    /// package datums, each marked for export or not. A datum the package
    /// marks for the enclosure is exported by its rule.
    pub(super) fn interface_part_section(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        instance: Id,
    ) -> Option<Operation> {
        let project = engine.project();
        let shown = self.interface_shown(engine)?.clone();
        let find = |role: DatumRole| {
            shown.keys.iter().find(|k| {
                k.key.role == role && k.key.owner == DatumOwner::Instance { id: instance }
            })
        };
        let package = pcbcraft_core::interface::package::placed(project, instance);
        let mut rows: Vec<(String, ExportTarget, Option<&InterfaceDatum>)> = vec![
            (
                "Frame".into(),
                ExportTarget::PartFrame { instance },
                find(DatumRole::PartFrame),
            ),
            (
                "Envelope".into(),
                ExportTarget::PartEnvelope { instance },
                find(DatumRole::PartEnvelope),
            ),
        ];
        for d in &package {
            let mut name = d.name.0.replace('_', " ");
            if let Some(first) = name.get(..1) {
                name = first.to_uppercase() + name.get(1..).unwrap_or_default();
            }
            rows.push((
                name,
                ExportTarget::PackageDatum {
                    instance,
                    name: d.name.clone(),
                },
                exported_datum(&shown.keys, instance, &d.name),
            ));
        }
        let exported = rows.iter().filter(|r| r.2.is_some()).count();
        let summary = match exported {
            0 => "Nothing exported".to_owned(),
            n => format!("{n} exported"),
        };
        let mut operation = None;
        kit::summary_section(
            ui,
            "pcb-part-interface",
            "Enclosure",
            false,
            &summary,
            |ui| {
                for (label, target, datum) in &rows {
                    match datum {
                        Some(d) if matches!(d.source, DatumSource::Auto { .. }) => {
                            kit::value_row(ui, label, "Exported automatically");
                        }
                        _ => {
                            let mut on = datum.is_some();
                            if kit::check_row(ui, label, &mut on) {
                                operation = Some(Operation::Interface(match datum {
                                    Some(d) => InterfaceCommand::Unexport { datum: d.id },
                                    None => InterfaceCommand::Export {
                                        target: target.clone(),
                                        name: None,
                                    },
                                }));
                            }
                        }
                    }
                }
                kit::note(ui, "Exported datums are what an enclosure may reference");
            },
        );
        operation
    }

    /// The docked preview, the whole subject slot while it is open.
    pub(super) fn interface_review_slot(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> CardResponse<Operation> {
        let ctx = ui.ctx().clone();
        if self.interface.pending_preview {
            self.interface.pending_preview = false;
            self.preview_publish(&ctx, engine);
        }
        let design = self.design_path(engine.project());
        let Some(mut review) = self.interface.review.take() else {
            return CardResponse::output(None);
        };
        let mut action = None;
        let mut recompute = false;
        let mut choose = false;
        let mut close = false;
        let mut start: Option<Job> = None;
        let opening = review.opening;
        review.opening = false;
        let waiting = review.running.is_some();
        match &mut review.kind {
            ReviewKind::Publish {
                tier,
                preview,
                publishing,
            } => {
                let title = "Publish interface";
                let mut tier_changed = false;
                let mut inputs = |ui: &mut Ui| {
                    let options = [
                        (Tier::Interface, "Interface only".to_owned()),
                        (
                            Tier::Mechanical,
                            "Interface and mechanical bodies".to_owned(),
                        ),
                        (
                            Tier::Full,
                            "Interface, mechanical bodies and copper".to_owned(),
                        ),
                    ];
                    if kit::choice_row(ui, "Contents", tier, &options) {
                        tier_changed = true;
                    }
                    kit::row(ui, "File", |ui| {
                        let summary = design.as_ref().map_or_else(
                            || "Choose where to publish".to_owned(),
                            |d| file_name(&d.to_string_lossy()),
                        );
                        choose = kit::summary_row(ui, &summary, Some("Choose…"));
                    });
                };
                match (preview.as_ref(), waiting) {
                    (Some(p), false) => {
                        let stale = p.document_revision != engine.revision();
                        let summary = publish_summary(p);
                        let blockers = publish_blockers(p);
                        let groups = publish_groups(p);
                        review.acked.resize(blockers.len(), false);
                        let no_file = design.is_none();
                        let panel = ReviewPanel {
                            id: review_id(),
                            icon: "export",
                            title,
                            summary: &summary,
                            stale,
                            blockers: &blockers,
                            after: &[],
                            groups: &groups,
                            apply: "Publish",
                            destructive: false,
                        };
                        action =
                            panel.show_with(
                                ui,
                                &mut review.acked,
                                review.refusal.as_deref().or(no_file
                                    .then_some("Save the project, or choose a file, to publish")),
                                &mut inputs,
                            );
                        if action == Some(ReviewAction::Apply) {
                            match &design {
                                Some(path) => {
                                    *publishing = true;
                                    review.refusal = None;
                                    start = Some(Job::Publish(PublishParams {
                                        path: path.to_string_lossy().into_owned(),
                                        tier: *tier,
                                        expected_revision: engine.revision(),
                                    }));
                                }
                                None => choose = true,
                            }
                        }
                    }
                    _ => {
                        let status = if *publishing {
                            "Building and writing the design…"
                        } else if waiting {
                            "Comparing with the last publish…"
                        } else {
                            "The preview could not be made"
                        };
                        let footer = waiting_panel(
                            ui,
                            title,
                            "Publish",
                            status,
                            review.refusal.as_deref().filter(|_| !waiting),
                            opening,
                            &mut recompute,
                            &mut inputs,
                        );
                        if footer == Some(DialogAction::Cancel) {
                            close = true;
                        }
                    }
                }
                if tier_changed {
                    recompute = true;
                }
            }
            ReviewKind::Housing { preview, updating } => {
                let title = "Update from enclosure";
                match (preview.as_ref(), waiting) {
                    (Some(p), false) => {
                        let stale = p.document_revision != engine.revision();
                        let (summary, blockers, groups) = housing_content(p);
                        review.acked.resize(blockers.len(), false);
                        let panel = ReviewPanel {
                            id: review_id(),
                            icon: "update",
                            title,
                            summary: &summary,
                            stale,
                            blockers: &blockers,
                            after: &[],
                            groups: &groups,
                            apply: "Update",
                            destructive: false,
                        };
                        action = panel.show_with(
                            ui,
                            &mut review.acked,
                            review.refusal.as_deref(),
                            |_| {},
                        );
                        if action == Some(ReviewAction::Apply)
                            && let Some(link) = engine.project().housing.as_deref()
                        {
                            let file = self.project_path.as_deref();
                            *updating = true;
                            review.refusal = None;
                            start = Some(Job::HousingUpdate(UpdateParams {
                                path: resolved(file, &link.enclosure_hint)
                                    .to_string_lossy()
                                    .into_owned(),
                                design: resolved(file, &link.design_hint)
                                    .to_string_lossy()
                                    .into_owned(),
                                expect_hash: Some(p.expect_hash.to_string()),
                                expected_revision: engine.revision(),
                            }));
                        }
                    }
                    _ => {
                        let status = if *updating {
                            "Reading the enclosure and updating…"
                        } else if waiting {
                            "Reading the enclosure…"
                        } else {
                            "The preview could not be made"
                        };
                        if waiting_panel(
                            ui,
                            title,
                            "Update",
                            status,
                            review.refusal.as_deref().filter(|_| !waiting),
                            opening,
                            &mut recompute,
                            |_| {},
                        ) == Some(DialogAction::Cancel)
                        {
                            close = true;
                        }
                    }
                }
            }
        }
        match action {
            Some(ReviewAction::Cancel) => close = true,
            Some(ReviewAction::Recompute) => recompute = true,
            Some(ReviewAction::CopyReport) => {
                ctx.copy_text(report(&review));
            }
            _ => {}
        }
        if let Some(job) = start {
            review.running = interface_host::start(&ctx, engine, self.project_path.clone(), job);
        }
        if !close {
            self.interface.review = Some(review);
        }
        if recompute {
            match self.interface.review.as_ref().map(|r| &r.kind) {
                Some(ReviewKind::Publish { .. }) => self.preview_publish(&ctx, engine),
                Some(ReviewKind::Housing { .. }) => self.open_housing(&ctx, engine),
                None => {}
            }
        }
        if choose {
            let proposed = design.clone();
            self.start_pick(&ctx, Pick::Design, proposed.as_deref());
        }
        CardResponse::output(None)
    }
}

/// The preview's text for Copy report.
fn report(review: &Review) -> String {
    match &review.kind {
        ReviewKind::Publish {
            preview: Some(p), ..
        } => {
            let mut lines = vec![publish_summary(p)];
            lines.extend(publish_blockers(p).into_iter().map(|b| b.title));
            for g in publish_groups(p) {
                lines.push(g.title);
                lines.extend(g.lines.into_iter().map(|l| format!("  {l}")));
            }
            lines.join("\n")
        }
        ReviewKind::Housing {
            preview: Some(p), ..
        } => {
            let (summary, blockers, groups) = housing_content(p);
            let mut lines = vec![summary];
            lines.extend(blockers.into_iter().map(|b| b.title));
            for g in groups {
                lines.push(g.title);
                lines.extend(g.lines.into_iter().map(|l| format!("  {l}")));
            }
            lines.join("\n")
        }
        _ => String::new(),
    }
}

/// The preview while its request runs, or after it failed: the review's
/// header and inputs, the status in the footer, the primary disabled.
#[allow(clippy::too_many_arguments, reason = "one docked panel's whole state")]
fn waiting_panel(
    ui: &mut Ui,
    title: &str,
    primary: &str,
    status: &str,
    refusal: Option<&str>,
    opening: bool,
    retry: &mut bool,
    inputs: impl FnOnce(&mut Ui),
) -> Option<DialogAction> {
    let failed = refusal.is_some();
    let panel = DialogPanel {
        kind: "Review",
        body_rows: f32::INFINITY,
        primary_disabled: Some(status),
        refusal,
        reset_scroll: opening,
        status: Some(status),
        ..DialogPanel::new(review_id(), "export", title, primary)
    };
    panel.show(ui, |ui| {
        if failed {
            *retry = kit::summary_row(ui, status, Some("Try again"));
        } else {
            kit::status(ui, status, kit::Tone::Dim);
        }
        inputs(ui);
    })
}
