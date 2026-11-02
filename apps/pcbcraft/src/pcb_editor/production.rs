//! Production files…: the docked dialog that checks the board for
//! fabrication and writes the production set (docs/fabrication-output.md).
//!
//! The dialog picks the fabrication profile to check against (the board's
//! own rules, a built-in profile or one of the user's), shows the
//! pre-flight report (`fabrication.preflight`) with a Waive… or Restore
//! action per finding, and writes Gerber, drill, job, assembly and drawing
//! files as a folder or one ZIP through `pcbcraft_io::fabrication::export`,
//! the call the CLI and automation use too. Waiving is a document edit
//! (`fabrication.waive` / `fabrication.unwaive`, one undo step each);
//! writing files is an external effect and is not undone.
//!
//! The pre-flight check and the export run off the UI thread on a snapshot
//! of the document (shared handles, no copy). A report computed for an
//! older revision or another profile is dropped and computed again; an
//! export keeps the snapshot it started from.
use super::PcbEditor;
use crate::kit::{
    self,
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
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Pre-flight rows shown before "Show all".
const LIST_ROWS: usize = 8;

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
    /// The finding whose waiver reason is being typed.
    waiving: Option<(String, String)>,
    show_all: bool,
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
            waiving: None,
            show_all: false,
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
                dialog.waiving = None;
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

/// The findings list with Waive… and Restore; returns a waiver edit.
fn findings(
    dialog: &mut ProductionDialog,
    ui: &mut Ui,
    report: &PreflightReport,
) -> Option<Operation> {
    let mut op = None;
    let items: Vec<&PreflightItem> = report.items.iter().collect();
    let max = if dialog.show_all {
        usize::MAX
    } else {
        LIST_ROWS
    };
    let mut start_waive = None;
    let shown = kit::list_frame(ui, items.len(), max, |ui, index| {
        let Some(item) = items.get(index) else {
            return;
        };
        let t = Tokens::get();
        ui.horizontal(|ui| {
            let size = t.font_label;
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(size + t.space_4, t.control_height),
                egui::Sense::hover(),
            );
            let level = if item.waiver.is_some() {
                Level::Ok
            } else {
                Level::from(item.severity)
            };
            kit::severity_glyph(ui.painter(), rect.center(), size, level);
            let (text, action) = match &item.waiver {
                Some(w) => (format!("{} · waived: {}", item.title, w.reason), "Restore"),
                None => (item.title.clone(), "Waive…"),
            };
            let waivable = item.waiver.is_some()
                || matches!(item.severity, Severity::Error | Severity::Warning);
            let clicked = kit::summary_row(ui, &text, waivable.then_some(action));
            if let Some(fix) = &item.fix {
                ui.interact(ui.min_rect(), ui.id().with("fix"), egui::Sense::hover())
                    .on_hover_text(fix);
            }
            if clicked {
                if item.waiver.is_some() {
                    op = Some(Operation::Fabrication(FabricationCommand::Unwaive {
                        key: item.key.clone(),
                    }));
                } else {
                    start_waive = Some(item.key.clone());
                }
            }
        });
    });
    if shown.show_all {
        dialog.show_all = true;
    }
    if let Some(key) = start_waive {
        dialog.waiving = Some((key, String::new()));
    }
    let mut done = None;
    if let Some((key, reason)) = &mut dialog.waiving {
        let title = report
            .items
            .iter()
            .find(|i| i.key == *key)
            .map_or_else(String::new, |i| i.title.clone());
        kit::value_row(ui, "Finding", &title);
        kit::text_row(ui, "Reason", reason);
        let ready = !reason.trim().is_empty();
        ui.horizontal(|ui| {
            if kit::primary(ui, "Waive", ready)
                .on_disabled_hover_text("Say why this is acceptable for this build")
                .clicked()
            {
                done = Some(Some(Operation::Fabrication(FabricationCommand::Waive {
                    key: key.clone(),
                    reason: reason.trim().to_owned(),
                })));
            }
            if solidcraft_ui_egui::ui_kit::button(
                ui,
                "Cancel",
                solidcraft_ui_egui::ui_kit::Button::Enabled(true),
            )
            .clicked()
            {
                done = Some(None);
            }
        });
    }
    if let Some(waive) = done {
        dialog.waiving = None;
        if waive.is_some() {
            op = waive;
        }
    }
    op
}

#[cfg(test)]
#[path = "production_tests.rs"]
mod tests;
