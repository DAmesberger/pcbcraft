//! Production files…: the docked dialog that checks the board for
//! fabrication and writes the production set (docs/fabrication-output.md).
//!
//! The dialog picks the fabrication profile to check against (the board's
//! own rules, a built-in profile or one of the user's), shows the
//! pre-flight report (`fabrication.preflight`) grouped by kind, and writes
//! Gerber, drill, job, assembly and drawing files as a folder or one ZIP
//! through `pcbcraft_io::fabrication::export`, the call the CLI and
//! automation use too. Findings are ticked one by one, as a Shift-click
//! run or as a whole group, and waived with one reason: the reason form
//! opens under the row or group where the waiving started, and one
//! `fabrication.waive` with every ticked key is one undo step. Restore
//! (`fabrication.unwaive`) undoes one waiver. Writing files is an external
//! effect and is not undone.
//!
//! The pre-flight check and the export run off the UI thread on a snapshot
//! of the document (shared handles, no copy). A report computed for an
//! older revision or another profile is dropped and computed again; an
//! export keeps the snapshot it started from.
use super::PcbEditor;
use crate::kit::{
    self, PickResponse, PickRow, Tick,
    card::{CardResponse, Subject},
    dialog::{DialogAction, DialogPanel},
    severity::Level,
    toast,
};
use crate::refusal::{self, Origin};
use egui::Ui;
use pcbcraft_core::fabrication::{
    self, BottomRotation, FabricationCommand, FabricationOptions, PlacementOrigin, PreflightItem,
    PreflightReport,
};
use pcbcraft_core::issues::Severity;
use pcbcraft_core::model::Document;
use pcbcraft_core::rules::{RuleProfile, builtin};
use pcbcraft_core::{Engine, Operation};
use pcbcraft_io::fabrication::{ExportReport, ExportRequest, Progress};
use pcbcraft_io::rule_profiles::ProfileLibrary;
use solidcraft_ui_egui::theme::Tokens;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Rows a group of findings shows before "Show all".
const LIST_ROWS: usize = kit::LIST_MAX_ROWS;

pub(super) fn dialog_id() -> egui::Id {
    egui::Id::new("pcb-production")
}

/// The profile the board is checked against.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Pick {
    /// The board's own rules (its profile snapshot, overrides and classes).
    Board,
    BuiltIn(String),
    User(String),
}

/// Folder or one ZIP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    Folder,
    Zip,
}

/// A pre-flight report, for one revision and profile.
type Checked = (u64, Pick, Result<PreflightReport, String>);

enum ExportMessage {
    Progress(Progress),
    Done(Result<ExportReport, String>),
}

/// The Production files dialog's state while it is open.
pub(super) struct ProductionDialog {
    /// The user's rule profiles, read once when the dialog opens.
    user_profiles: Vec<RuleProfile>,
    pub(super) pick: Pick,
    pub(super) format: Format,
    /// The folder chosen to hold the output folder, or the ZIP file.
    pub(super) location: Option<PathBuf>,
    pub(super) options: FabricationOptions,
    /// The current report and what it was computed for.
    pub(super) report: Option<Checked>,
    checking: Option<(u64, Pick, Receiver<Checked>)>,
    /// Ticked findings and the reason being typed.
    pub(super) waiving: Waiving,
    picking: Option<Receiver<Option<PathBuf>>>,
    exporting: Option<Receiver<ExportMessage>>,
    progress: Option<Progress>,
    pub(super) written: Option<ExportReport>,
    refusal: Option<String>,
    opening: bool,
}

impl ProductionDialog {
    pub(super) fn new(library: Option<ProfileLibrary>) -> Self {
        let user_profiles = library
            .as_ref()
            .and_then(|l| l.list().ok())
            .map(|l| l.profiles)
            .unwrap_or_default();
        Self {
            user_profiles,
            pick: Pick::Board,
            format: Format::Folder,
            location: None,
            options: FabricationOptions::default(),
            report: None,
            checking: None,
            waiving: Waiving::default(),
            picking: None,
            exporting: None,
            progress: None,
            written: None,
            refusal: None,
            opening: true,
        }
    }

    /// The profile `pick` names; `None` checks the board's own rules.
    fn profile(&self) -> Option<RuleProfile> {
        match &self.pick {
            Pick::Board => None,
            Pick::BuiltIn(id) => builtin::all().into_iter().find(|p| p.id == *id),
            Pick::User(id) => self.user_profiles.iter().find(|p| p.id == *id).cloned(),
        }
    }

    fn options_list(&self, engine: &Engine) -> Vec<(Pick, String)> {
        let mut out = vec![(
            Pick::Board,
            format!("{} · this board", engine.project().rules.profile.name),
        )];
        out.extend(builtin::all().into_iter().map(|p| {
            (
                Pick::BuiltIn(p.id.clone()),
                format!("{} · built-in", p.name),
            )
        }));
        out.extend(
            self.user_profiles
                .iter()
                .map(|p| (Pick::User(p.id.clone()), p.name.clone())),
        );
        out
    }

    /// Start or take the pre-flight report for the current revision and
    /// profile. A result for an older revision or profile is dropped.
    fn check(&mut self, ctx: &egui::Context, engine: &Engine) {
        let wanted = (engine.revision(), self.pick.clone());
        if let Some((revision, pick, receiver)) = &self.checking {
            match receiver.try_recv() {
                Ok(checked) => {
                    self.checking = None;
                    if (checked.0, &checked.1) == (wanted.0, &wanted.1) {
                        self.report = Some(checked);
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.checking = None;
                    self.report = Some((
                        wanted.0,
                        wanted.1.clone(),
                        Err("The production check stopped unexpectedly".into()),
                    ));
                }
                Err(TryRecvError::Empty) if (*revision, pick) == (wanted.0, &wanted.1) => return,
                Err(TryRecvError::Empty) => self.checking = None,
            }
        }
        let current = self
            .report
            .as_ref()
            .is_some_and(|(r, p, _)| (*r, p) == (wanted.0, &wanted.1));
        if current || self.checking.is_some() {
            return;
        }
        let document = engine.document().clone();
        let profile = self.profile();
        let (sender, receiver) = std::sync::mpsc::channel();
        let (revision, pick) = wanted.clone();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("pcbcraft-preflight".into())
            .spawn(move || {
                let report = preflight(&document, profile.as_ref());
                let _ = sender.send((revision, pick, report));
                repaint.request_repaint();
            });
        match spawned {
            Ok(_) => self.checking = Some((wanted.0, wanted.1, receiver)),
            Err(error) => {
                self.report = Some((
                    wanted.0,
                    wanted.1,
                    Err(format!("The production check could not start: {error}")),
                ));
            }
        }
    }

    /// The report for the current revision and profile, if computed.
    pub(super) fn current(&self, engine: &Engine) -> Option<&Result<PreflightReport, String>> {
        self.report
            .as_ref()
            .filter(|(r, p, _)| *r == engine.revision() && *p == self.pick)
            .map(|(_, _, report)| report)
    }

    /// Take a picked location.
    fn poll_picker(&mut self) {
        let Some(receiver) = &self.picking else {
            return;
        };
        match receiver.try_recv() {
            Ok(path) => {
                self.picking = None;
                if path.is_some() {
                    self.location = path;
                    self.refusal = None;
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.picking = None,
        }
    }

    /// Take export progress and the result.
    fn poll_export(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.exporting else {
            return;
        };
        loop {
            match receiver.try_recv() {
                Ok(ExportMessage::Progress(progress)) => self.progress = Some(progress),
                Ok(ExportMessage::Done(result)) => {
                    self.exporting = None;
                    self.progress = None;
                    match result {
                        Ok(report) => {
                            toast::post(
                                ctx,
                                Level::Ok,
                                format!("Wrote {} production files", report.files.len()),
                                Vec::new(),
                            );
                            self.written = Some(report);
                            self.refusal = None;
                        }
                        Err(error) => self.refusal = Some(error),
                    }
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.exporting = None;
                    self.progress = None;
                    self.refusal = Some("The export stopped unexpectedly".into());
                    return;
                }
            }
        }
    }

    /// Ask where to write, off the UI thread: a folder that will hold the
    /// output folder, or the ZIP file.
    fn start_pick(&mut self, ctx: &egui::Context, stem: &str) {
        if self.picking.is_some() {
            return;
        }
        let dialog = rfd::AsyncFileDialog::new();
        let future: std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<rfd::FileHandle>> + Send>,
        > = match self.format {
            Format::Folder => Box::pin(
                dialog
                    .set_title("Choose the folder for the production files")
                    .pick_folder(),
            ),
            Format::Zip => Box::pin(
                dialog
                    .set_title("Save the production files as a ZIP")
                    .add_filter("ZIP archive", &["zip"])
                    .set_file_name(format!("{stem}-production.zip"))
                    .save_file(),
            ),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("pcbcraft-production-picker".into())
            .spawn(move || {
                let picked = pollster::block_on(future).map(|f| f.path().to_path_buf());
                let _ = sender.send(picked);
                repaint.request_repaint();
            });
        if spawned.is_ok() {
            self.picking = Some(receiver);
        }
    }

    /// The folder or ZIP the export writes.
    pub(super) fn output(&self, stem: &str) -> Option<PathBuf> {
        let location = self.location.as_ref()?;
        Some(match self.format {
            Format::Folder => location.join(format!("{stem}-production")),
            Format::Zip => location.clone(),
        })
    }

    /// Write the set from `document` off the UI thread.
    pub(super) fn start_export(
        &mut self,
        ctx: &egui::Context,
        document: Document,
        output: PathBuf,
    ) {
        if self.exporting.is_some() {
            return;
        }
        let request = ExportRequest {
            output,
            zip: self.format == Format::Zip,
            profile: self.profile(),
            options: self.options.clone(),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("pcbcraft-production".into())
            .spawn(move || {
                let mut progress = |p: Progress| {
                    let _ = sender.send(ExportMessage::Progress(p));
                    repaint.request_repaint();
                };
                let result = pcbcraft_io::fabrication::export(
                    &document.project,
                    &document.drafts,
                    &request,
                    &mut progress,
                )
                .map_err(|e| export_error(&e));
                let _ = sender.send(ExportMessage::Done(result));
                repaint.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.exporting = Some(receiver);
                self.written = None;
                self.refusal = None;
            }
            Err(error) => self.refusal = Some(format!("The export could not start: {error}")),
        }
    }

    /// Whether an export is running.
    pub(super) fn busy(&self) -> bool {
        self.exporting.is_some()
    }

    /// Block until the running export finishes (tests).
    #[cfg(test)]
    pub(super) fn wait(&mut self, ctx: &egui::Context) {
        while self.exporting.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll_export(ctx);
        }
    }

    /// Block until the pre-flight report for the current state arrives
    /// (tests).
    #[cfg(test)]
    pub(super) fn wait_check(&mut self, ctx: &egui::Context, engine: &Engine) {
        for _ in 0..2000 {
            self.check(ctx, engine);
            if self.current(engine).is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// The pre-flight report of a snapshot.
fn preflight(
    document: &Document,
    profile: Option<&RuleProfile>,
) -> Result<PreflightReport, String> {
    fabrication::preflight::report(&document.project, &document.drafts, profile)
        .map_err(|e| refusal::shown_error(&e))
}

/// One line for an export failure: the rule code is dropped.
fn export_error(error: &pcbcraft_io::Error) -> String {
    match error {
        pcbcraft_io::Error::Document(e) => refusal::shown_error(e),
        other => {
            let text = other.to_string();
            let text = match text.split_once(": ") {
                Some((code, rest)) if code.contains('.') && !code.contains(' ') => rest.to_owned(),
                _ => text,
            };
            refusal::clip(&text, 80)
        }
    }
}

/// "1.2 MB", "34.5 KB", "812 B".
pub fn size_text(bytes: usize) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// "2 errors · 1 warning · 1 waived", or "No issues".
pub fn report_summary(report: &PreflightReport) -> String {
    let mut parts = Vec::new();
    let unwaived = |s: Severity| {
        report
            .items
            .iter()
            .filter(|i| i.severity == s && i.waiver.is_none())
            .count()
    };
    for (severity, level) in [
        (Severity::Error, Level::Error),
        (Severity::Warning, Level::Warning),
        (Severity::ToDo, Level::ToDo),
    ] {
        let n = unwaived(severity);
        if n > 0 {
            parts.push(level.count_words(n));
        }
    }
    if report.waived > 0 {
        parts.push(format!("{} waived", report.waived));
    }
    if parts.is_empty() {
        "No issues".into()
    } else {
        parts.join(" · ")
    }
}

/// Why Export is disabled, or `None` when it can run.
pub(super) fn export_blocked(
    dialog: &ProductionDialog,
    report: Option<&Result<PreflightReport, String>>,
    output: Option<&PathBuf>,
) -> Option<String> {
    if dialog.busy() {
        return Some("Writing the production files…".into());
    }
    match report {
        None => return Some("Checking the design…".into()),
        Some(Err(error)) => return Some(error.clone()),
        Some(Ok(report)) if report.blocking == 1 => {
            return Some("Fix or waive the error first".into());
        }
        Some(Ok(report)) if report.blocking > 1 => {
            return Some(format!("Fix or waive the {} errors first", report.blocking));
        }
        Some(Ok(_)) => {}
    }
    if output.is_none() {
        return Some("Choose where to save the files".into());
    }
    None
}

impl PcbEditor {
    /// Open the Production files dialog (the ribbon's Production files…).
    /// The user profile library is the config folder's `rule-profiles/`.
    pub(super) fn open_production(&mut self) {
        let library = pcbcraft_io::settings::UserDirs::from_env()
            .ok()
            .map(|dirs| ProfileLibrary::from_dirs(&dirs));
        self.open_production_in(library);
    }

    /// [`Self::open_production`] over a given library (tests).
    pub(super) fn open_production_in(&mut self, library: Option<ProfileLibrary>) {
        self.setup = None;
        self.stackup = None;
        self.net_classes = None;
        self.rules = None;
        if self.production.is_none() {
            self.production = Some(ProductionDialog::new(library));
        }
    }

    pub(super) fn production_subject(&self) -> Option<Subject> {
        self.production.as_ref().map(|_| Subject::Dialog)
    }

    /// The whole subject slot while the dialog is open.
    pub(super) fn production_slot(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> CardResponse<Operation> {
        let Some(mut dialog) = self.production.take() else {
            return CardResponse::output(None);
        };
        let ctx = ui.ctx().clone();
        if let Some(text) = refusal::claim(&ctx, Origin::Dialog(dialog_id())) {
            dialog.refusal = Some(text);
        }
        dialog.check(&ctx, engine);
        dialog.poll_picker();
        dialog.poll_export(&ctx);
        let stem = fabrication::base_name(engine.project(), &dialog.options)
            .unwrap_or_else(|_| "board".into());
        let output = dialog.output(&stem);
        let report = dialog.current(engine).cloned();
        let blocked = export_blocked(&dialog, report.as_ref(), output.as_ref());
        let status = dialog
            .progress
            .as_ref()
            .map(|p| format!("Step {} of {} · {}", p.step, p.total, p.label));
        let refusal_text = dialog.refusal.clone();
        let panel = DialogPanel {
            kind: "Output",
            body_rows: 24.0,
            primary_disabled: blocked.as_deref(),
            refusal: refusal_text.as_deref(),
            reset_scroll: dialog.opening,
            status: status.as_deref(),
            ..DialogPanel::new(dialog_id(), "export", "Production files", "Export")
        };
        dialog.opening = false;
        let mut op = None;
        let action = panel.show(ui, |ui| {
            op = body(
                &mut dialog,
                ui,
                engine,
                report.as_ref(),
                output.as_ref(),
                &stem,
            );
        });
        let mut close = false;
        match action {
            Some(DialogAction::Cancel) => close = true,
            Some(DialogAction::Primary) => {
                if let Some(output) = output {
                    dialog.start_export(&ctx, engine.document().clone(), output);
                }
            }
            None => {}
        }
        if op.is_some() {
            refusal::mark(&ctx, Origin::Dialog(dialog_id()));
        }
        if !close {
            self.production = Some(dialog);
        }
        CardResponse::output(op)
    }
}

/// The dialog's body; returns a waiver edit.
fn body(
    dialog: &mut ProductionDialog,
    ui: &mut Ui,
    engine: &Engine,
    report: Option<&Result<PreflightReport, String>>,
    output: Option<&PathBuf>,
    stem: &str,
) -> Option<Operation> {
    let mut op = None;
    kit::section(
        ui,
        "pcb-production-check",
        "Production check",
        true,
        None,
        |ui| {
            let options = dialog.options_list(engine);
            let mut pick = dialog.pick.clone();
            if kit::choice_row(ui, "Profile", &mut pick, &options) && pick != dialog.pick {
                dialog.pick = pick;
                dialog.waiving = Waiving::default();
            }
            match report {
                None => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(Tokens::get().font_small));
                        kit::note(ui, "Checking the design…");
                    });
                }
                Some(Err(_)) => {}
                Some(Ok(report)) => {
                    kit::value_row(ui, "Result", &report_summary(report));
                    op = findings(dialog, ui, report);
                }
            }
        },
    );
    kit::section(ui, "pcb-production-output", "Output", true, None, |ui| {
        let mut format = dialog.format;
        let formats = [
            (Format::Folder, "Folder".to_owned()),
            (Format::Zip, "One ZIP file".to_owned()),
        ];
        if kit::choice_row(ui, "Format", &mut format, &formats) && format != dialog.format {
            dialog.format = format;
            dialog.location = None;
        }
        let shown = output.map_or_else(|| "Not chosen".to_owned(), |p| p.display().to_string());
        let mut choose = false;
        kit::row(ui, "Save to", |ui| {
            choose = kit::summary_row(ui, &shown, Some("Choose…"));
        });
        if choose {
            dialog.start_pick(ui.ctx(), stem);
        }
    });
    kit::section(ui, "pcb-production-files", "Files", false, None, |ui| {
        let o = &mut dialog.options.outputs;
        kit::check_row(ui, "Gerber and job file", &mut o.gerber);
        kit::check_row(ui, "Drill and drill map", &mut o.drill);
        kit::check_row(ui, "Placement and BOM", &mut o.assembly);
        kit::check_row(ui, "PDF drawings", &mut o.drawings);
    });
    kit::section(
        ui,
        "pcb-production-placement",
        "Pick and place",
        false,
        None,
        |ui| {
            let p = &mut dialog.options.placement;
            let mut origin = match p.origin {
                PlacementOrigin::OutlineLowerLeft => 1,
                _ => 0,
            };
            let origins = [
                (0, "Board origin".to_owned()),
                (1, "Outline lower left".to_owned()),
            ];
            if kit::choice_row(ui, "Origin", &mut origin, &origins) {
                p.origin = if origin == 1 {
                    PlacementOrigin::OutlineLowerLeft
                } else {
                    PlacementOrigin::BoardOrigin
                };
            }
            let rotations = [
                (
                    BottomRotation::ViewedFromTop,
                    "Seen from the top".to_owned(),
                ),
                (
                    BottomRotation::ViewedFromBottom,
                    "Seen from below".to_owned(),
                ),
            ];
            kit::choice_row(ui, "Bottom", &mut p.bottom, &rotations);
            kit::check_row(ui, "Mirror bottom X", &mut p.mirror_bottom_x);
            kit::hint(ui, "Angles turn counter-clockwise from 0° as drawn", "");
        },
    );
    if let Some(written) = &dialog.written {
        let output = PathBuf::from(&written.output);
        kit::section(
            ui,
            "pcb-production-written",
            "Written",
            true,
            Some(written.files.len()),
            |ui| {
                kit::list_frame(ui, written.files.len(), usize::MAX, |ui, index| {
                    if let Some(file) = written.files.get(index) {
                        kit::summary_row(
                            ui,
                            &format!("{} · {}", file.name, size_text(file.bytes)),
                            None,
                        );
                    }
                });
                if kit::summary_row(
                    ui,
                    &output.display().to_string(),
                    Some(crate::library_sources::REVEAL),
                ) {
                    crate::library_sources::reveal(&output);
                }
            },
        );
    }
    op
}

/// Where the reason form opens: under a finding's row (its key) or a
/// group's header (its code).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum At {
    Row(String),
    Group(String),
}

/// The production check's waiving: ticked findings, the reason being
/// typed and where its form shows, and how the groups are shown.
#[derive(Default)]
pub(super) struct Waiving {
    /// Ticked finding keys.
    pub(super) ticked: BTreeSet<String>,
    /// Where the reason form is; `None` while it is closed.
    pub(super) at: Option<At>,
    pub(super) reason: String,
    /// The last row ticked, for a Shift-click run.
    last: Option<String>,
    /// Groups unfolded, and groups showing all their rows.
    pub(super) open: BTreeSet<String>,
    all: BTreeSet<String>,
    /// Focus the reason and scroll the form into view when next drawn.
    reveal: bool,
}

impl Waiving {
    /// Open the form at `at` and bring it into view.
    fn open_at(&mut self, at: At) {
        self.at = Some(at);
        self.reveal = true;
    }

    /// Close the form and drop the ticks; the reason is kept for the next.
    fn close(&mut self) {
        self.ticked.clear();
        self.at = None;
        self.last = None;
        self.reveal = false;
    }
}

/// A finding that can be waived now: an error or warning nobody waived.
fn waivable(item: &PreflightItem) -> bool {
    item.waiver.is_none() && matches!(item.severity, Severity::Error | Severity::Warning)
}

/// The report's findings by code, in report order (most severe first).
fn groups(report: &PreflightReport) -> Vec<(&str, Vec<&PreflightItem>)> {
    let mut out: Vec<(&str, Vec<&PreflightItem>)> = Vec::new();
    for item in &report.items {
        match out.iter_mut().find(|(code, _)| *code == item.code) {
            Some((_, items)) => items.push(item),
            None => out.push((item.code.as_str(), vec![item])),
        }
    }
    out
}

/// One line of the findings list.
enum Entry<'a> {
    /// A group's header, by index.
    Header(usize),
    /// A finding, at an indent level.
    Row(&'a PreflightItem, usize),
    /// "n of m · Show all" of a group.
    More(usize),
    /// The reason form.
    Form,
}

/// The shown lines: a group of one is its row; a larger group is a
/// header, then (unfolded) its rows bounded at [`LIST_ROWS`] with "Show
/// all". The reason form follows the row or header it was opened from, or
/// that row's header while the row is hidden.
fn entries<'a>(waiving: &Waiving, groups: &[(&str, Vec<&'a PreflightItem>)]) -> Vec<Entry<'a>> {
    let mut out = Vec::new();
    let mut placed = false;
    for (g, (code, items)) in groups.iter().enumerate() {
        let here = match &waiving.at {
            Some(At::Group(c)) => c == code,
            Some(At::Row(key)) => items.iter().any(|i| i.key == *key),
            None => false,
        };
        if let [item] = items.as_slice() {
            out.push(Entry::Row(item, 0));
            if here {
                out.push(Entry::Form);
                placed = true;
            }
            continue;
        }
        out.push(Entry::Header(g));
        let open = waiving.open.contains(*code);
        let shown = if waiving.all.contains(*code) {
            items.len()
        } else {
            LIST_ROWS.min(items.len())
        };
        let row_shown = match &waiving.at {
            Some(At::Row(key)) => open && items.iter().take(shown).any(|i| i.key == *key),
            _ => false,
        };
        if here && !row_shown {
            out.push(Entry::Form);
            placed = true;
        }
        if open {
            for item in items.iter().take(shown) {
                out.push(Entry::Row(item, 1));
                if here && row_shown && waiving.at == Some(At::Row(item.key.clone())) {
                    out.push(Entry::Form);
                    placed = true;
                }
            }
            if shown < items.len() {
                out.push(Entry::More(g));
            }
        }
    }
    if !placed && waiving.at.is_some() {
        out.push(Entry::Form);
    }
    out
}

/// What the list's rows asked for this frame.
enum Asked {
    /// A group header's tick box, fold or Waive all….
    Header(usize, PickResponse),
    /// A row's tick box or action.
    Row(String, PickResponse),
    More(usize),
}

/// The findings list; returns a waiver or restore edit.
fn findings(
    dialog: &mut ProductionDialog,
    ui: &mut Ui,
    report: &PreflightReport,
) -> Option<Operation> {
    let w = &mut dialog.waiving;
    // Keep only ticks of findings that can still be waived.
    let can: BTreeSet<&str> = report
        .items
        .iter()
        .filter(|i| waivable(i))
        .map(|i| i.key.as_str())
        .collect();
    w.ticked.retain(|k| can.contains(k.as_str()));
    if w.ticked.is_empty() {
        w.at = None;
    }
    let groups = groups(report);
    let entries = entries(w, &groups);
    let base = ui.id().with("findings");
    let mut asked = None;
    let mut form = (false, false);
    let count = w.ticked.len();
    let reveal = std::mem::take(&mut w.reveal);
    let (ticked, open, reason) = (&w.ticked, &w.open, &mut w.reason);
    kit::list_frame(ui, entries.len(), usize::MAX, |ui, index| {
        match entries.get(index) {
            Some(Entry::Header(g)) => {
                let Some((code, items)) = groups.get(*g) else {
                    return;
                };
                let can: Vec<&&PreflightItem> = items.iter().filter(|i| waivable(i)).collect();
                let on = can.iter().filter(|i| ticked.contains(&i.key)).count();
                let tick = match (can.len(), on) {
                    (0, _) => None,
                    (_, 0) => Some(Tick::Off),
                    (n, m) if n == m => Some(Tick::All),
                    _ => Some(Tick::Some),
                };
                let level = items
                    .iter()
                    .find(|i| i.waiver.is_none())
                    .map_or(Level::Ok, |i| Level::from(i.severity));
                let title = pcbcraft_core::issues::group_title(code);
                let response = kit::pick_row(
                    ui,
                    base.with(("group", *code)),
                    &PickRow {
                        level,
                        title: &title,
                        tip: "",
                        depth: 0,
                        fold: Some(open.contains(*code)),
                        count: Some(items.len()),
                        tick,
                        action: (!can.is_empty()).then_some("Waive all…"),
                    },
                );
                if response != PickResponse::default() {
                    asked = Some(Asked::Header(*g, response));
                }
            }
            Some(Entry::Row(item, depth)) => {
                let tip = row_tip(item);
                let (level, tick, action) = if item.waiver.is_some() {
                    (Level::Ok, None, Some("Restore"))
                } else if waivable(item) {
                    let on = ticked.contains(&item.key);
                    (
                        Level::from(item.severity),
                        Some(if on { Tick::All } else { Tick::Off }),
                        Some("Waive…"),
                    )
                } else {
                    (Level::from(item.severity), None, None)
                };
                let response = kit::pick_row(
                    ui,
                    base.with(("row", &item.key)),
                    &PickRow {
                        level,
                        title: &item.title,
                        tip: &tip,
                        depth: *depth,
                        fold: None,
                        count: None,
                        tick,
                        action,
                    },
                );
                if response != PickResponse::default() {
                    asked = Some(Asked::Row(item.key.clone(), response));
                }
            }
            Some(Entry::More(g)) => {
                if let Some((_, items)) = groups.get(*g) {
                    let shown = LIST_ROWS.min(items.len());
                    if kit::summary_row(
                        ui,
                        &format!("{shown} of {}", items.len()),
                        Some("Show all \u{203a}"),
                    ) {
                        asked = Some(Asked::More(*g));
                    }
                }
            }
            Some(Entry::Form) => form = reason_form(ui, reason, count, reveal),
            None => {}
        }
    });
    let mut op = None;
    match asked {
        Some(Asked::Header(g, response)) => {
            if let Some((code, items)) = groups.get(g) {
                let code = (*code).to_owned();
                if response.folded && !w.open.remove(&code) {
                    w.open.insert(code.clone());
                }
                let keys: Vec<&String> = items
                    .iter()
                    .filter(|i| waivable(i))
                    .map(|i| &i.key)
                    .collect();
                let all_on = keys.iter().all(|k| w.ticked.contains(*k));
                if response.ticked && all_on {
                    for key in keys {
                        w.ticked.remove(key);
                    }
                    if w.ticked.is_empty() {
                        w.close();
                    }
                } else if response.ticked || response.action {
                    w.ticked.extend(keys.into_iter().cloned());
                    w.open_at(At::Group(code));
                }
            }
        }
        Some(Asked::Row(key, response)) => {
            let item = report.items.iter().find(|i| i.key == key);
            if response.action && item.is_some_and(|i| i.waiver.is_some()) {
                op = Some(Operation::Fabrication(FabricationCommand::Unwaive {
                    key: key.clone(),
                }));
            } else if response.action {
                w.ticked.insert(key.clone());
                w.last = Some(key.clone());
                w.open_at(At::Row(key));
            } else if response.ticked {
                tick_row(w, &groups, &key, response.shift);
            }
        }
        Some(Asked::More(g)) => {
            if let Some((code, _)) = groups.get(g) {
                w.all.insert((*code).to_owned());
            }
        }
        None => {}
    }
    match form {
        (true, _) => {
            let keys: Vec<String> = w.ticked.iter().cloned().collect();
            let reason = w.reason.trim().to_owned();
            if !keys.is_empty() && !reason.is_empty() {
                op = Some(Operation::Fabrication(FabricationCommand::Waive {
                    keys,
                    reason,
                }));
                w.close();
            }
        }
        (false, true) => w.close(),
        (false, false) => {}
    }
    op
}

/// A row's hover after its title: what fixes it, and its waiver.
fn row_tip(item: &PreflightItem) -> String {
    let mut tip = Vec::new();
    if let Some(waiver) = &item.waiver {
        tip.push(format!("Waived: {}", waiver.reason));
    }
    if let Some(fix) = &item.fix {
        tip.push(fix.clone());
    }
    tip.join("\n")
}

/// Tick or untick the row `key`; with Shift, tick the run of its group
/// from the last row ticked to it. The form follows the row.
fn tick_row(w: &mut Waiving, groups: &[(&str, Vec<&PreflightItem>)], key: &str, shift: bool) {
    let run = shift
        .then(|| {
            let last = w.last.as_deref()?;
            let (_, items) = groups
                .iter()
                .find(|(_, items)| items.iter().any(|i| i.key == key))?;
            let a = items.iter().position(|i| i.key == last)?;
            let b = items.iter().position(|i| i.key == key)?;
            let (from, to) = (a.min(b), a.max(b));
            Some(
                items
                    .get(from..=to)?
                    .iter()
                    .filter(|i| waivable(i))
                    .map(|i| i.key.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .flatten();
    match run {
        Some(keys) => w.ticked.extend(keys),
        None if w.ticked.remove(key) => {}
        None => {
            w.ticked.insert(key.to_owned());
        }
    }
    w.last = Some(key.to_owned());
    if w.ticked.is_empty() {
        w.close();
    } else if w.ticked.contains(key) {
        w.open_at(At::Row(key.to_owned()));
    }
}

/// The reason form under the row or group it was opened from: the reason,
/// then Cancel and "Waive n". Returns (waive, cancel).
fn reason_form(ui: &mut Ui, reason: &mut String, count: usize, reveal: bool) -> (bool, bool) {
    let t = Tokens::get();
    let top = ui.cursor().top();
    ui.add_space(t.space_4);
    let field = kit::row(ui, "Reason", |ui| {
        kit::text_field(
            ui,
            "waive-reason",
            reason,
            "Why this is acceptable for this build",
            false,
        )
    });
    let ready = !reason.trim().is_empty();
    let mut out = (false, false);
    let label = if count == 1 {
        "Waive 1 finding".to_owned()
    } else {
        format!("Waive {count} findings")
    };
    kit::actions(ui, |ui| {
        if kit::primary(ui, &label, ready)
            .on_disabled_hover_text("Say why this is acceptable for this build")
            .clicked()
        {
            out.0 = true;
        }
        if solidcraft_ui_egui::ui_kit::button(
            ui,
            "Cancel",
            solidcraft_ui_egui::ui_kit::Button::Enabled(true),
        )
        .clicked()
        {
            out.1 = true;
        }
    });
    ui.add_space(t.space_4);
    if reveal {
        field.request_focus();
        let rect = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
        ui.scroll_to_rect(rect, None);
    }
    out
}

#[cfg(test)]
#[path = "production_tests.rs"]
mod tests;
