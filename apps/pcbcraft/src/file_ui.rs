//! Desktop file workflow. Native picker parenting/filtering follows Solidcraft
//! apps/solidcraft/src/main.rs::services; unsaved choices follow documents.rs.
//! Dialogs stay in the app. Reads, writes, native export and recovery run outside
//! the UI thread; every result retains its document epoch and captured content.
use crate::kit::severity::Level;
use pcbcraft_core::{Document, Engine, Project, format::EditorFile};
use pcbcraft_io::recovery::{self, RecoveryEntry, RecoveryStore};
use solidcraft_ui_egui::{
    theme::Tokens,
    ui_kit::{self, Button},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};

type Outcome<T> = Result<T, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    epoch: u64,
    revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ticket {
    id: u64,
    stamp: Stamp,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Transition {
    New,
    Open(Option<PathBuf>),
    Close,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Save,
    Open,
    Export,
    /// Print every schematic sheet to a PDF.
    SchematicPdf,
    Restore,
    Close,
}
#[derive(Clone)]
struct Flight {
    ticket: Ticket,
    kind: Kind,
    continuation: Option<Transition>,
}

/// Pure lifecycle state: a successful write can save its captured version
/// without marking later edits saved or carrying on with a destructive action.
struct Controller {
    current: Stamp,
    saved: Option<Stamp>,
    path: Option<PathBuf>,
    serial: u64,
    flight: Option<Flight>,
    prompt: Option<Transition>,
    recovery_flight: Option<Ticket>,
    recovered: Option<Stamp>,
}
impl Controller {
    fn new(revision: u64) -> Self {
        let current = Stamp { epoch: 0, revision };
        Self {
            saved: Some(current.clone()),
            current,
            path: None,
            serial: 0,
            flight: None,
            prompt: None,
            recovery_flight: None,
            recovered: None,
        }
    }
    fn dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.current)
    }
    fn observe(&mut self, revision: u64) -> bool {
        let next = Stamp {
            epoch: self.current.epoch,
            revision,
        };
        let changed = next != self.current;
        self.current = next;
        changed
    }
    fn allocate(&mut self) -> Outcome<Ticket> {
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or("File request identity exhausted")?;
        Ok(Ticket {
            id: self.serial,
            stamp: self.current.clone(),
        })
    }
    fn start(&mut self, kind: Kind, continuation: Option<Transition>) -> Outcome<Ticket> {
        if self.flight.is_some() {
            return Err("Another file operation is still running".into());
        }
        let ticket = self.allocate()?;
        self.flight = Some(Flight {
            ticket: ticket.clone(),
            kind,
            continuation,
        });
        Ok(ticket)
    }
    fn request(&mut self, transition: Transition) -> Option<Transition> {
        if self.flight.is_some() || self.prompt.is_some() {
            return None;
        }
        if self.dirty() {
            self.prompt = Some(transition);
            None
        } else {
            Some(transition)
        }
    }
    fn take_flight(&mut self, ticket: &Ticket, kind: Kind) -> Option<Flight> {
        if self
            .flight
            .as_ref()
            .is_some_and(|f| f.ticket == *ticket && f.kind == kind)
        {
            self.flight.take()
        } else {
            None
        }
    }
    fn cancel(&mut self, ticket: &Ticket, kind: Kind) {
        if let Some(flight) = self.take_flight(ticket, kind) {
            self.prompt = flight.continuation;
        }
    }
    fn saved(&mut self, ticket: &Ticket, path: PathBuf) -> Option<(bool, Option<Transition>)> {
        let flight = self.take_flight(ticket, Kind::Save)?;
        if ticket.stamp.epoch != self.current.epoch {
            return None;
        }
        self.path = Some(path);
        self.saved = Some(ticket.stamp.clone());
        let current = ticket.stamp == self.current;
        if !current {
            self.prompt = flight.continuation;
            return Some((false, None));
        }
        Some((true, flight.continuation))
    }
    fn accept(&mut self, ticket: &Ticket, kind: Kind) -> bool {
        self.take_flight(ticket, kind).is_some() && ticket.stamp == self.current
    }
    fn adopt(&mut self, revision: u64, path: Option<PathBuf>, recovered: bool) -> Outcome<()> {
        let epoch = self
            .current
            .epoch
            .checked_add(1)
            .ok_or("Document identity exhausted")?;
        self.current = Stamp { epoch, revision };
        self.saved = if recovered {
            None
        } else {
            Some(self.current.clone())
        };
        self.path = path;
        self.flight = None;
        self.prompt = None;
        self.recovered = None;
        Ok(())
    }
    fn finish_recovery(&mut self, ticket: &Ticket, success: bool, removed: bool) {
        if self.recovery_flight.as_ref() != Some(ticket) {
            return;
        }
        self.recovery_flight = None;
        if success && ticket.stamp == self.current {
            self.recovered = if removed {
                None
            } else {
                Some(ticket.stamp.clone())
            };
        }
    }
}

pub enum Action {
    New,
    Open,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    Export,
    ExportSchematicPdf,
    Close,
}
pub enum Effect {
    Replace {
        document: Box<EditorFile>,
        path: Option<PathBuf>,
        recovered: bool,
    },
    Close,
}

enum Job {
    Read(Ticket, PathBuf),
    Write(Ticket, PathBuf, Document),
    Export(Ticket, PathBuf, Arc<Project>),
    SchematicPdf(Ticket, PathBuf, Arc<Project>),
    Recover(Ticket, Document, Option<PathBuf>),
    ClearRecovery(Ticket, bool),
    Restore(Ticket, PathBuf),
    DiscardRecovery(PathBuf),
}
enum Event {
    Startup(Outcome<Vec<RecoveryEntry>>),
    Picked(Ticket, Kind, Option<PathBuf>),
    Read(Ticket, PathBuf, Outcome<EditorFile>),
    Written(Ticket, PathBuf, Outcome<()>),
    Exported(Ticket, PathBuf, Outcome<()>),
    PdfExported(Ticket, PathBuf, Outcome<usize>),
    Recovered(Ticket, bool, bool, Outcome<()>),
    Restored(Ticket, PathBuf, Outcome<EditorFile>),
    Discarded(PathBuf, Outcome<()>),
}

fn worker(ctx: egui::Context, directory: PathBuf, events: Sender<Event>) -> Sender<Job> {
    let (sender, jobs) = mpsc::channel();
    std::thread::spawn(move || {
        let entries = recovery::list(&directory).map_err(|e| e.to_string());
        let store = RecoveryStore::new(&directory).map_err(|e| e.to_string());
        let startup = match &store {
            Ok(_) => entries,
            Err(error) => Err(format!("Recovery unavailable: {error}")),
        };
        if events.send(Event::Startup(startup)).is_err() {
            return;
        }
        ctx.request_repaint();
        while let Ok(job) = jobs.recv() {
            let result = match job {
                Job::Read(ticket, path) => {
                    let value = pcbcraft_io::read_project(&path).map_err(|e| e.to_string());
                    Event::Read(ticket, path, value)
                }
                Job::Write(ticket, path, document) => {
                    // The deep copy into the file form happens here, off the UI thread.
                    let result =
                        pcbcraft_io::write_project(&path, &EditorFile::from_document(&document))
                            .map_err(|e| e.to_string());
                    Event::Written(ticket, path, result)
                }
                Job::Export(ticket, path, project) => {
                    let result = path
                        .to_str()
                        .ok_or_else(|| "Native export path must be UTF-8".to_owned())
                        .and_then(|path| export_project(&project, path));
                    Event::Exported(ticket, path, result)
                }
                Job::SchematicPdf(ticket, path, project) => {
                    let result =
                        pcbcraft_io::schematic_pdf::export_schematic_pdf(&project, &[], &path)
                            .map_err(|e| e.to_string());
                    Event::PdfExported(ticket, path, result)
                }
                Job::Recover(ticket, document, origin) => {
                    let file = EditorFile::from_document(&document);
                    let result = store.as_ref().map_err(Clone::clone).and_then(|store| {
                        store
                            .write(&file, origin.as_deref())
                            .map_err(|e| e.to_string())
                    });
                    Event::Recovered(ticket, false, false, result)
                }
                Job::ClearRecovery(ticket, close) => {
                    let result = match &store {
                        Ok(store) => store.remove_own().map_err(|e| e.to_string()),
                        Err(_) => Ok(()),
                    };
                    Event::Recovered(ticket, true, close, result)
                }
                Job::Restore(ticket, path) => {
                    let result = recovery::read(&path)
                        .map(|r| r.document)
                        .map_err(|e| e.to_string());
                    Event::Restored(ticket, path, result)
                }
                Job::DiscardRecovery(path) => {
                    let result =
                        recovery::remove_entry(&directory, &path).map_err(|e| e.to_string());
                    Event::Discarded(path, result)
                }
            };
            if events.send(result).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    sender
}

pub(crate) fn export_project(project: &Project, path: &str) -> Outcome<()> {
    pcbcraft_solidcraft::project::build(project)
        .and_then(|model| model.save(path))
        .map_err(|e| e.to_string())
}

pub struct FileUi {
    controller: Controller,
    /// The current document; cloning shares its data.
    document: Document,
    captured: Option<Document>,
    picker: rfd::AsyncFileDialog,
    folder: PathBuf,
    sender: Sender<Job>,
    event_sender: Sender<Event>,
    receiver: Receiver<Event>,
    effects: Vec<Effect>,
    /// The latest file message and how it reads.
    status: Option<(Level, String)>,
    recoveries: Vec<RecoveryEntry>,
    /// The recovery the dialog would restore or discard.
    recovery_selected: usize,
    /// A restored session file, removed once this session has saved its
    /// own recovery snapshot of the restored document (so the work is never
    /// in no file at all).
    restored_source: Option<PathBuf>,
    recovery_choice_pending: bool,
    changed_at: f64,
    recovery_retry_at: f64,
    now: f64,
    /// Quitting was asked for while a file operation ran: ask once it ends.
    close_pending: bool,
    /// The window is closing with the user's answer (or nothing unsaved):
    /// the exit keeps no recovery copy.
    closed: bool,
}
impl FileUi {
    pub fn new(cc: &eframe::CreationContext<'_>, engine: &Engine) -> Self {
        let picker = rfd::AsyncFileDialog::new().set_parent(cc);
        Self::with_directory(&cc.egui_ctx, picker, engine, recovery_directory())
    }
    /// The file workflow with recovery snapshots in `directory`.
    pub(crate) fn with_directory(
        ctx: &egui::Context,
        picker: rfd::AsyncFileDialog,
        engine: &Engine,
        directory: PathBuf,
    ) -> Self {
        let (events, receiver) = mpsc::channel();
        let sender = worker(ctx.clone(), directory, events.clone());
        Self {
            controller: Controller::new(engine.revision()),
            document: engine.document().clone(),
            captured: None,
            picker,
            folder: home_directory(),
            sender,
            event_sender: events,
            receiver,
            effects: vec![],
            status: None,
            recoveries: vec![],
            recovery_selected: 0,
            restored_source: None,
            recovery_choice_pending: false,
            changed_at: 0.0,
            recovery_retry_at: 0.0,
            now: 0.0,
            close_pending: false,
            closed: false,
        }
    }
    pub fn observe(&mut self, engine: &Engine, now: f64) {
        self.now = now;
        if self.controller.observe(engine.revision()) {
            self.document = engine.document().clone();
            self.changed_at = now;
        }
    }
    pub fn dirty(&self) -> bool {
        self.controller.dirty()
    }
    pub fn path(&self) -> Option<&Path> {
        self.controller.path.as_deref()
    }
    pub fn busy(&self) -> bool {
        self.controller.flight.is_some() || self.recovery_choice_pending
    }
    pub fn prompting(&self) -> bool {
        self.controller.prompt.is_some() || !self.recoveries.is_empty()
    }
    /// The latest file message, with its severity said explicitly.
    pub fn take_status(&mut self) -> Option<(Level, String)> {
        self.status.take()
    }
    pub fn adopt(
        &mut self,
        engine: &Engine,
        path: Option<PathBuf>,
        recovered: bool,
    ) -> Outcome<()> {
        self.controller.adopt(engine.revision(), path, recovered)?;
        self.document = engine.document().clone();
        self.changed_at = self.now;
        self.clear_recovery(false);
        Ok(())
    }
    /// The app is ending without a Save or Don't Save answer (Dock › Quit,
    /// a logout, a signal): write unsaved work to this session's recovery
    /// snapshot now, waiting at most `wait`, so the next launch offers it.
    /// Returns whether the snapshot holds the current document.
    pub fn keep_for_recovery(&mut self, wait: Duration) -> bool {
        if self.closed || !self.controller.dirty() {
            return false;
        }
        if self.controller.recovered.as_ref() == Some(&self.controller.current) {
            return true;
        }
        let Ok(ticket) = self.controller.allocate() else {
            return false;
        };
        self.controller.recovery_flight = Some(ticket.clone());
        self.send(Job::Recover(
            ticket.clone(),
            self.document.clone(),
            self.controller.path.clone(),
        ));
        let deadline = std::time::Instant::now() + wait;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Ok(event) = self.receiver.recv_timeout(left) else {
                break;
            };
            let ours = matches!(&event, Event::Recovered(t, false, false, _) if *t == ticket);
            self.receive(event);
            if ours {
                break;
            }
        }
        self.controller.recovered.as_ref() == Some(&self.controller.current)
    }
    fn send(&mut self, job: Job) {
        if let Err(error) = self.sender.send(job) {
            self.controller.flight = None;
            self.controller.recovery_flight = None;
            self.status = Some((Level::Error, error.to_string()));
        }
    }
    fn begin(&mut self, kind: Kind, continuation: Option<Transition>) -> Option<Ticket> {
        match self.controller.start(kind, continuation) {
            Ok(ticket) => {
                self.captured = Some(self.document.clone());
                Some(ticket)
            }
            Err(error) => {
                self.status = Some((Level::Error, error));
                None
            }
        }
    }
    pub fn request(&mut self, action: Action) {
        if matches!(action, Action::Close) && self.closed {
            return;
        }
        if matches!(action, Action::Close) && self.controller.prompt.is_none() {
            if self.busy() {
                // A save or a file panel is still open: quit once it ends.
                self.close_pending = true;
                return;
            }
            // The startup recovery question is kept on disk for the next
            // launch, as "Keep for later" does.
            self.recoveries.clear();
        }
        if self.busy() || self.controller.prompt.is_some() || !self.recoveries.is_empty() {
            return;
        }
        match action {
            Action::Save => self.save(false, None),
            Action::SaveAs => self.save(true, None),
            Action::Export => {
                if let Some(ticket) = self.begin(Kind::Export, None) {
                    self.pick(ticket, Kind::Export);
                }
            }
            Action::ExportSchematicPdf => {
                if let Some(ticket) = self.begin(Kind::SchematicPdf, None) {
                    self.pick(ticket, Kind::SchematicPdf);
                }
            }
            action => {
                let transition = match action {
                    Action::New => Transition::New,
                    Action::Open => Transition::Open(None),
                    Action::OpenPath(path) => Transition::Open(Some(path)),
                    _ => Transition::Close,
                };
                if let Some(transition) = self.controller.request(transition) {
                    self.proceed(transition);
                }
            }
        }
    }
    fn proceed(&mut self, transition: Transition) {
        match transition {
            Transition::New => self.effects.push(Effect::Replace {
                document: Box::new(Engine::default().to_file()),
                path: None,
                recovered: false,
            }),
            Transition::Open(path) => {
                if let Some(ticket) = self.begin(Kind::Open, None) {
                    if let Some(path) = path {
                        match absolute_path(path) {
                            Ok(path) => self.send(Job::Read(ticket, path)),
                            Err(error) => {
                                self.controller.cancel(&ticket, Kind::Open);
                                self.status = Some((Level::Error, error));
                            }
                        }
                    } else {
                        self.pick(ticket, Kind::Open);
                    }
                }
            }
            Transition::Close => self.clear_recovery(true),
        }
    }
    fn save(&mut self, save_as: bool, continuation: Option<Transition>) {
        let Some(ticket) = self.begin(Kind::Save, continuation) else {
            return;
        };
        if !save_as && let Some(path) = self.controller.path.clone() {
            self.send(Job::Write(ticket, path, self.document.clone()));
        } else {
            self.pick(ticket, Kind::Save);
        }
    }
    fn pick(&mut self, ticket: Ticket, kind: Kind) {
        let dialog = self.picker.clone().set_directory(&self.folder);
        let dialog = if kind == Kind::Export {
            dialog
                .set_title("Export PCB to Solidcraft")
                .set_file_name("board.solidcraft")
                .add_filter("Solidcraft model", &["solidcraft"])
        } else if kind == Kind::SchematicPdf {
            dialog
                .set_title("Export schematic PDF")
                .set_file_name("schematic.pdf")
                .add_filter("PDF document", &["pdf"])
        } else {
            let filename = self
                .controller
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|p| p.to_str())
                .unwrap_or("untitled.pcbcraft");
            dialog
                .set_title(if kind == Kind::Open {
                    "Open Pcbcraft project"
                } else {
                    "Save Pcbcraft project"
                })
                .set_file_name(filename)
                .add_filter("Pcbcraft project", &["pcbcraft", "PCBCRAFT"])
        };
        // Construct the parented native future on the event thread, just as
        // Solidcraft services do. Awaiting it does not block egui or file I/O.
        type Picking =
            std::pin::Pin<Box<dyn std::future::Future<Output = Option<rfd::FileHandle>> + Send>>;
        let future: Picking = if kind == Kind::Open {
            Box::pin(dialog.pick_file())
        } else {
            Box::pin(dialog.save_file())
        };
        let events = self.event_sender.clone();
        std::thread::spawn(move || {
            let picked = pollster::block_on(future).map(|handle| handle.path().to_path_buf());
            let _ = events.send(Event::Picked(ticket, kind, picked));
        });
    }
    /// Pick an SVG drawing to import, with the same parented native picker.
    /// The answer (None when cancelled) arrives on the returned channel;
    /// the caller reads the file off the UI thread.
    pub fn pick_svg(&self) -> Receiver<Option<PathBuf>> {
        let dialog = self
            .picker
            .clone()
            .set_directory(&self.folder)
            .set_title("Import SVG drawing")
            .add_filter("SVG drawing", &["svg", "SVG"]);
        let future = dialog.pick_file();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let picked = pollster::block_on(future).map(|handle| handle.path().to_path_buf());
            let _ = sender.send(picked);
        });
        receiver
    }
    fn clear_recovery(&mut self, close: bool) {
        let ticket = if close {
            self.begin(Kind::Close, None)
        } else {
            self.controller.allocate().ok()
        };
        if let Some(ticket) = ticket {
            self.controller.recovery_flight = Some(ticket.clone());
            self.send(Job::ClearRecovery(ticket, close));
        }
    }
    pub fn tick(&mut self, ctx: &egui::Context) -> Vec<Effect> {
        while let Ok(event) = self.receiver.try_recv() {
            self.receive(event);
        }
        if self.close_pending && !self.busy() {
            self.close_pending = false;
            self.request(Action::Close);
        }
        // Once closing (Don't Save), no new snapshot brings the work back.
        if self.controller.dirty()
            && !self.closed
            && !self.prompting()
            && self.controller.recovery_flight.is_none()
            && self.controller.recovered.as_ref() != Some(&self.controller.current)
            && self.now >= self.recovery_retry_at
        {
            let wait = (2.0 - (self.now - self.changed_at)).max(0.0);
            if wait == 0.0 {
                if let Ok(ticket) = self.controller.allocate() {
                    self.controller.recovery_flight = Some(ticket.clone());
                    self.send(Job::Recover(
                        ticket,
                        self.document.clone(),
                        self.controller.path.clone(),
                    ));
                }
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(wait));
            }
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        std::mem::take(&mut self.effects)
    }
    fn receive(&mut self, event: Event) {
        match event {
            Event::Startup(result) => match result {
                Ok(entries) => self.recoveries = entries,
                Err(error) => self.status = Some((Level::Error, error)),
            },
            Event::Picked(ticket, kind, path) => {
                if !self
                    .controller
                    .flight
                    .as_ref()
                    .is_some_and(|f| f.ticket == ticket && f.kind == kind)
                {
                    return;
                }
                let Some(path) = path else {
                    self.controller.cancel(&ticket, kind);
                    self.captured = None;
                    self.status = Some((Level::Info, "File selection cancelled".into()));
                    return;
                };
                let path = match absolute_path(path) {
                    Ok(path) => path,
                    Err(error) => {
                        self.controller.cancel(&ticket, kind);
                        self.status = Some((Level::Error, error));
                        return;
                    }
                };
                if let Some(parent) = path.parent() {
                    self.folder = parent.to_path_buf();
                }
                match kind {
                    Kind::Save => {
                        if let Some(document) = self.captured.clone() {
                            self.send(Job::Write(ticket, path, document));
                        }
                    }
                    Kind::Open if ticket.stamp == self.controller.current => {
                        self.send(Job::Read(ticket, path))
                    }
                    Kind::Export if ticket.stamp == self.controller.current => {
                        if let Some(document) = self.captured.as_ref() {
                            let project = Arc::clone(&document.project);
                            self.send(Job::Export(ticket, path, project));
                        }
                    }
                    Kind::SchematicPdf if ticket.stamp == self.controller.current => {
                        if let Some(document) = self.captured.as_ref() {
                            let project = Arc::clone(&document.project);
                            self.send(Job::SchematicPdf(ticket, path, project));
                        }
                    }
                    _ => {
                        self.controller.cancel(&ticket, kind);
                        self.status = Some((
                            Level::Warning,
                            "The document changed while choosing a file. Please try again.".into(),
                        ));
                    }
                }
            }
            Event::Written(ticket, path, result) => match result {
                Ok(()) => {
                    if let Some((current, continuation)) =
                        self.controller.saved(&ticket, path.clone())
                    {
                        self.captured = None;
                        self.status = Some((
                            Level::Ok,
                            if current {
                                format!("Saved {}", path.display())
                            } else {
                                format!(
                                    "Saved captured version to {}; newer edits remain unsaved",
                                    path.display()
                                )
                            },
                        ));
                        if current {
                            self.clear_recovery(false);
                        }
                        if let Some(transition) = continuation {
                            self.proceed(transition);
                        }
                    }
                }
                Err(error) => {
                    self.controller.cancel(&ticket, Kind::Save);
                    self.status = Some((Level::Error, format!("Save failed: {error}")));
                }
            },
            Event::Read(ticket, path, result) => {
                if !self.controller.accept(&ticket, Kind::Open) {
                    self.status = Some((
                        Level::Warning,
                        "Open cancelled because the current document changed".into(),
                    ));
                    return;
                }
                match result {
                    Ok(document) => self.effects.push(Effect::Replace {
                        document: Box::new(document),
                        path: Some(path),
                        recovered: false,
                    }),
                    Err(error) => {
                        self.status = Some((Level::Error, format!("Open failed: {error}")))
                    }
                }
            }
            Event::PdfExported(ticket, path, result) => {
                let current = self.controller.accept(&ticket, Kind::SchematicPdf);
                self.status = Some(match result {
                    Ok(pages) if current => (
                        Level::Ok,
                        format!("Printed {pages} sheets to {}", path.display()),
                    ),
                    Ok(pages) => (
                        Level::Warning,
                        format!(
                            "Printed {pages} captured sheets to {}; the document has changed",
                            path.display()
                        ),
                    ),
                    Err(error) => (Level::Error, format!("PDF export failed: {error}")),
                });
            }
            Event::Exported(ticket, path, result) => {
                let current = self.controller.accept(&ticket, Kind::Export);
                self.status = Some(match result {
                    Ok(()) if current => (Level::Ok, format!("Exported {}", path.display())),
                    Ok(()) => (
                        Level::Warning,
                        format!(
                            "Exported captured PCB to {}; current document has changed",
                            path.display()
                        ),
                    ),
                    Err(error) => (Level::Error, format!("Export failed: {error}")),
                });
            }
            Event::Recovered(ticket, removed, close, result) => {
                // This session now holds the restored work in its own
                // snapshot: the restored session file is no longer needed.
                if result.is_ok()
                    && !removed
                    && ticket.stamp == self.controller.current
                    && let Some(source) = self.restored_source.take()
                {
                    self.send(Job::DiscardRecovery(source));
                }
                self.controller
                    .finish_recovery(&ticket, result.is_ok(), removed);
                if close {
                    let current = self.controller.accept(&ticket, Kind::Close);
                    if current && result.is_ok() {
                        self.closed = true;
                        self.effects.push(Effect::Close);
                    } else if result.is_ok() {
                        self.controller.prompt = Some(Transition::Close);
                    }
                }
                if let Err(error) = result {
                    self.status = Some((Level::Error, format!("Recovery failed: {error}")));
                    self.recovery_retry_at = self.now + 10.0;
                }
            }
            Event::Restored(ticket, path, result) => {
                self.recovery_choice_pending = false;
                if !self.controller.accept(&ticket, Kind::Restore) {
                    self.status = Some((
                        Level::Warning,
                        "Restore cancelled because the current document changed".into(),
                    ));
                    return;
                }
                match result {
                    Ok(document) => {
                        self.recoveries.retain(|e| e.path != path);
                        self.recovery_selected = 0;
                        self.restored_source = Some(path);
                        self.effects.push(Effect::Replace {
                            document: Box::new(document),
                            path: None,
                            recovered: true,
                        });
                    }
                    Err(error) => {
                        self.status = Some((Level::Error, format!("Restore failed: {error}")))
                    }
                }
            }
            Event::Discarded(path, result) => {
                self.recovery_choice_pending = false;
                match result {
                    Ok(()) => self.recoveries.retain(|e| e.path != path),
                    Err(error) => self.status = Some((Level::Error, error)),
                }
            }
        }
    }
    /// Unsaved-changes and startup-recovery prompts, in Solidcraft's system
    /// window chrome.
    pub fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(transition) = &self.controller.prompt {
            let title = match transition {
                Transition::Close => "Save changes before quitting?",
                Transition::New | Transition::Open(_) => "Save changes before continuing?",
            };
            let mut choice = None;
            ui_kit::Window::new(egui::Id::new("file-unsaved"), "status_warning", title)
                .required()
                .show_body(ctx, |ui| {
                    dialog_body(ui, |ui| {
                        crate::kit::status(
                            ui,
                            "The project or component draft has unsaved changes.",
                            crate::kit::Tone::Dim,
                        );
                        if let Some((_, status)) = &self.status {
                            crate::kit::note(ui, status);
                        }
                        crate::kit::actions(ui, |ui| {
                            if crate::kit::primary(ui, "Save", true).clicked() {
                                choice = Some(Choice::Save);
                            }
                            if ui_kit::button(ui, DONT_SAVE, Button::Normal).clicked() {
                                choice = Some(Choice::Discard);
                            }
                            if ui_kit::button(ui, "Cancel", Button::Normal).clicked() {
                                choice = Some(Choice::Cancel);
                            }
                        });
                    });
                });
            match choice {
                Some(Choice::Save) => {
                    let next = self.controller.prompt.take();
                    self.save(false, next);
                }
                Some(Choice::Discard) => {
                    if let Some(next) = self.controller.prompt.take() {
                        self.proceed(next);
                    }
                }
                Some(Choice::Cancel) => {
                    self.controller.prompt = None;
                    self.close_pending = false;
                }
                None => {}
            }
            return;
        }
        if self.recoveries.is_empty() {
            return;
        }
        self.recovery_selected = self.recovery_selected.min(self.recoveries.len() - 1);
        let Some(entry) = self.recoveries.get(self.recovery_selected).cloned() else {
            return;
        };
        let busy = self.busy();
        let mut choice = None;
        let mut selected = self.recovery_selected;
        let count = self.recoveries.len();
        let title = if count == 1 {
            "Recover an earlier session?".to_owned()
        } else {
            format!("Recover one of {count} earlier sessions?")
        };
        ui_kit::Window::new(egui::Id::new("startup-recovery"), "status_warning", &title)
            .required()
            .show_body(ctx, |ui| {
                dialog_body(ui, |ui| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                    for (at, candidate) in self.recoveries.iter().enumerate() {
                        let mut on = at == selected;
                        let label = recovery_label(candidate, now);
                        if crate::kit::check_row(ui, &label, &mut on) && on {
                            selected = at;
                        }
                    }
                    if let Some(path) = &entry.original_path {
                        crate::kit::value_row(ui, "File", &path.display().to_string());
                    }
                    if let Some(error) = &entry.error {
                        crate::kit::status(ui, error, crate::kit::Tone::Error);
                    }
                    crate::kit::note(
                        ui,
                        "Restore opens an unsaved copy of the checked session; once it is safe in this session, its old snapshot is removed. The original file is unchanged.",
                    );
                    ui.add_enabled_ui(!busy, |ui| {
                        crate::kit::actions(ui, |ui| {
                            if crate::kit::primary(ui, "Restore", entry.error.is_none()).clicked() {
                                choice = Some(Choice::Save);
                            }
                            if ui_kit::button(ui, "Keep for later", Button::Normal).clicked() {
                                choice = Some(Choice::Cancel);
                            }
                            if ui_kit::button(ui, "Discard", Button::Normal).clicked() {
                                choice = Some(Choice::Discard);
                            }
                        });
                    });
                });
            });
        self.recovery_selected = selected;
        match choice {
            Some(Choice::Save) => {
                if let Some(ticket) = self.begin(Kind::Restore, None) {
                    self.recovery_choice_pending = true;
                    self.send(Job::Restore(ticket, entry.path.clone()));
                }
            }
            Some(Choice::Cancel) => {
                // Kept on disk for the next launch; none is asked again now.
                self.recoveries.clear();
                self.recovery_selected = 0;
            }
            Some(Choice::Discard) => {
                self.recovery_choice_pending = true;
                self.send(Job::DiscardRecovery(entry.path.clone()));
            }
            None => {}
        }
    }
}

/// The unsaved-changes answer that goes on without saving. It keeps no
/// recovery copy either: the session's snapshot is removed before the
/// window closes, as in Solidcraft.
const DONT_SAVE: &str = "Don't Save";

/// One recovery as the dialog lists it: name, age and size.
fn recovery_label(entry: &RecoveryEntry, now_ms: u64) -> String {
    if entry.error.is_some() {
        return entry.project_name.clone();
    }
    let minutes = now_ms.saturating_sub(entry.saved_unix_ms) / 60_000;
    let age = match minutes {
        0 => "just now".to_owned(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => format!("{} days ago", minutes / 1440),
    };
    let instances = match entry.instances {
        1 => "1 instance".to_owned(),
        n => format!("{n} instances"),
    };
    format!("{} · {age} · {instances}", entry.project_name)
}

#[derive(Clone, Copy)]
enum Choice {
    Save,
    Discard,
    Cancel,
}

fn absolute_path(path: PathBuf) -> Outcome<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|e| e.to_string())
    }
}
fn home_directory() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(std::env::temp_dir)
}
fn recovery_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("PCBCRAFT_RECOVERY_DIR") {
        return PathBuf::from(path);
    }
    if cfg!(target_os = "macos") {
        home_directory().join("Library/Application Support/Pcbcraft/Recovery")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home_directory)
            .join("Pcbcraft/Recovery")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_directory().join(".local/state"))
            .join("pcbcraft/recovery")
    }
}

/// Dialog content inset like the kit's cards, in the kit's form scope.
fn dialog_body(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::get();
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(t.space_12 as i8, t.space_8 as i8))
        .show(ui, |ui| {
            ui.set_min_width(360.0);
            crate::kit::form(ui, add);
        });
}

#[cfg(test)]
#[path = "file_ui_tests.rs"]
mod quit_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Recoveries are told apart by age and size, not only by name.
    #[test]
    fn recovery_rows_name_age_and_instances() {
        let entry = RecoveryEntry {
            path: "/r/session-1.json".into(),
            original_path: None,
            saved_unix_ms: 1_000_000,
            project_name: "Untitled".into(),
            instances: 4,
            error: None,
        };
        assert_eq!(
            recovery_label(&entry, 1_000_000 + 5 * 60_000),
            "Untitled · 5 min ago · 4 instances"
        );
        assert_eq!(
            recovery_label(&entry, 1_000_000 + 3 * 3_600_000),
            "Untitled · 3 h ago · 4 instances"
        );
        assert_eq!(
            recovery_label(&entry, 0),
            "Untitled · just now · 4 instances"
        );
    }

    #[test]
    fn a_save_marks_only_its_captured_revision_saved() -> Outcome<()> {
        let mut controller = Controller::new(1);
        assert!(!controller.dirty());
        controller.observe(2);
        assert!(controller.dirty());
        let ticket = controller.start(Kind::Save, None)?;
        controller.observe(3);
        let saved = controller.saved(&ticket, PathBuf::from("/tmp/x.pcbcraft"));
        assert_eq!(saved.map(|(current, _)| current), Some(false));
        assert!(controller.dirty(), "the newer edit stays unsaved");
        Ok(())
    }

    #[test]
    fn destructive_transitions_wait_for_a_choice_when_dirty() -> Outcome<()> {
        let mut controller = Controller::new(1);
        assert_eq!(controller.request(Transition::New), Some(Transition::New));
        controller.observe(2);
        assert_eq!(controller.request(Transition::Close), None);
        assert_eq!(controller.prompt, Some(Transition::Close));
        controller.adopt(5, None, false)?;
        assert!(!controller.dirty());
        assert!(controller.prompt.is_none());
        Ok(())
    }
}
