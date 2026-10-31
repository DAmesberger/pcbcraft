//! Library sources in the desktop (parts library S3): the configured local
//! library folders, their loaded catalogs, and the per-user favourites and
//! recents, persisted through `pcbcraft_io::settings`.
//!
//! Folders are opened and indexed on a background thread; the UI only ever
//! sees finished results tagged with the scan that made them, so a slow
//! folder never freezes the editor and an older scan never replaces a newer
//! one. A source that cannot be searched stays listed with its reason.
//!
//! Document changes still go through the one engine: placing from a folder
//! is `library.place_from`, "Update project copy" is `library.import`
//! followed by the ordinary update review, promoting is
//! `library.promote_preview`, io's `write_bundle` and `library.promote`, and
//! duplicating is `library.duplicate`.
use crate::kit::{self, Tone};
use pcbcraft_core::catalog::{
    CatalogContext, CatalogEntry, EntryKey, MAX_CONTEXT, SourceCatalog, UpdateAvailable,
};
use pcbcraft_core::geometry::Point;
use pcbcraft_core::ids::{LibraryId, SourceLineageId};
use pcbcraft_core::library::Bundle;
use pcbcraft_core::library::bundle::{Direction, SourceRevision};
use pcbcraft_core::library::import::{MAX_FRESH_LINEAGES, OtherRevision, PlaceSource, PromotePlan};
use pcbcraft_core::updates::Change;
use pcbcraft_core::{
    ComponentRef, DefinitionId, Engine, LibraryCommand, Operation, Outcome, Query, QueryResult,
    Revision,
};
use pcbcraft_io::library_folder::{self as folder, Disabled, Folder};
use pcbcraft_io::library_folder_index::{self as index, IndexState};
use pcbcraft_io::settings::{self, MAX_FAVOURITES, MAX_RECENT, MAX_SOURCES, SourceSetting};
use pcbcraft_io::settings::{UserDirs, UserSettings};
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

/// The conflict code `library.place_from` refuses a different project
/// revision of the same source lineage with (`OtherRevision::Refuse`).
pub const OTHER_REVISION: &str = "library.other_revision";

/// How one configured source stands.
#[derive(Clone, Debug)]
pub enum Status {
    /// A scan is reading it.
    Loading,
    /// Searchable: its manifest passed and its index loaded.
    Ready {
        folder: Folder,
        catalog: Arc<SourceCatalog>,
        index: IndexState,
    },
    /// Listed but not searchable, with the reason io gives.
    Disabled(Disabled),
    /// The manifest passed but the folder could not be indexed, or another
    /// configured folder already holds this library.
    Failed(String),
}

/// One configured source as the Browser and the sources card list it.
#[derive(Clone, Debug)]
pub struct SourceView {
    pub setting: SourceSetting,
    /// The manifest's name, else the one last seen (kept in settings),
    /// else the folder's file name.
    pub name: String,
    pub status: Status,
}

impl SourceView {
    fn pending(setting: SourceSetting) -> Self {
        Self {
            name: setting
                .name
                .clone()
                .unwrap_or_else(|| folder_name(&setting.path)),
            setting,
            status: Status::Loading,
        }
    }

    /// Why the source is not searchable; `None` while loading or ready.
    pub fn reason(&self) -> Option<String> {
        match &self.status {
            Status::Disabled(why) => Some(why.to_string()),
            Status::Failed(why) => Some(why.clone()),
            Status::Loading | Status::Ready { .. } => None,
        }
    }

    /// The Browser's dim trailing text: an entry count, "loading" or the
    /// reason it is disabled.
    pub fn secondary(&self) -> String {
        match &self.status {
            Status::Loading => "loading…".into(),
            Status::Ready { catalog, .. } => match catalog.entries.len() {
                1 => "1 entry".into(),
                n => format!("{n} entries"),
            },
            _ => self.reason().unwrap_or_default(),
        }
    }

    /// [`Self::secondary`] short enough for a Browser row; the full reason
    /// is the row's tip and the sources card's status.
    pub fn short(&self) -> String {
        match &self.status {
            Status::Disabled(Disabled::Malformed(_)) => "unreadable manifest".into(),
            Status::Disabled(Disabled::UnknownFeatures(_)) => "unknown feature".into(),
            Status::Failed(_) => "not searchable".into(),
            _ => self.secondary(),
        }
    }

    pub fn library(&self) -> Option<LibraryId> {
        match &self.status {
            Status::Ready { folder, .. } => Some(folder.manifest.library),
            _ => None,
        }
    }

    pub fn ready(&self) -> bool {
        matches!(self.status, Status::Ready { .. })
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map_or_else(|| path.display().to_string(), str::to_owned)
}

/// Open one configured folder and load its index: the reason it is
/// disabled is typed so the Browser can show it. Blocking; scans call it
/// off the UI thread.
pub fn load(setting: &SourceSetting, cache: Option<&Path>) -> SourceView {
    let mut view = SourceView::pending(setting.clone());
    view.status = match folder::inspect_folder(&setting.path) {
        Err(why) => Status::Disabled(why),
        Ok(opened) => {
            view.name = opened.manifest.name.clone();
            if !setting.enabled {
                Status::Disabled(Disabled::Off)
            } else {
                match cache {
                    None => Status::Failed("no user cache directory for the index".into()),
                    Some(cache) => match index::load_or_build(cache, &opened) {
                        Ok((catalog, state)) => Status::Ready {
                            folder: opened,
                            catalog: Arc::new(catalog),
                            index: state,
                        },
                        Err(error) => Status::Failed(format!("index failed: {error}")),
                    },
                }
            }
        }
    };
    view
}

/// A second configured folder holding a library already listed is not
/// searched: one library id, one catalog.
fn mark_duplicates(sources: &mut [SourceView]) {
    let mut seen: Vec<(LibraryId, String)> = Vec::new();
    for source in sources.iter_mut() {
        let Some(library) = source.library() else {
            continue;
        };
        if let Some((_, first)) = seen.iter().find(|(l, _)| *l == library) {
            source.status = Status::Failed(format!("same library as {first}"));
        } else {
            seen.push((library, source.setting.path.display().to_string()));
        }
    }
}

/// A folder entry chosen in the Place palette: a component, or a Part,
/// which `library.place_from` places as its component with the Part
/// assigned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderPick {
    pub library: LibraryId,
    pub lineage: SourceLineageId,
    pub revision: Revision,
    pub name: String,
}

impl FolderPick {
    /// The pick of a folder component or Part entry; `None` for a project
    /// entry or any other kind.
    pub fn of(entry: &CatalogEntry) -> Option<Self> {
        let EntryKey::Library { library, lineage } = entry.key() else {
            return None;
        };
        matches!(
            entry.kind,
            pcbcraft_core::DefinitionKind::Component | pcbcraft_core::DefinitionKind::Part
        )
        .then_some(())?;
        Some(Self {
            library,
            lineage,
            revision: entry.revision?,
            name: entry.name.clone(),
        })
    }
}

/// What placing from a folder did.
#[derive(Clone, Debug, PartialEq)]
pub enum Placed {
    Placed {
        designator: String,
        instance: pcbcraft_core::Id,
        position: Point,
    },
    /// The project holds another revision of this source lineage: offer
    /// Keep both / Update project copy / Cancel.
    OtherRevision { message: String },
}

/// `library.place_from` of a bundle root at `position`, one undo step.
pub fn place(
    engine: &mut Engine,
    bundle: Bundle,
    position: Point,
    other_revision: OtherRevision,
) -> Result<Placed, String> {
    let operation = Operation::Library(LibraryCommand::PlaceFrom {
        source: PlaceSource::Bundle {
            bundle: Box::new(bundle),
        },
        position,
        designator: None,
        other_revision,
    });
    match apply(engine, operation) {
        Ok(Outcome::PlacedFrom {
            designator,
            instance,
            ..
        }) => Ok(Placed::Placed {
            designator,
            instance,
            position,
        }),
        Ok(_) => Err("library.place_from returned an unexpected outcome".into()),
        Err(error) if is_other_revision(&error) => Ok(Placed::OtherRevision {
            message: choice_text(&error.to_string()),
        }),
        Err(error) => Err(error.to_string()),
    }
}

/// The refusal as the palette shows it: the code is for programs, the
/// sentence for people.
fn choice_text(message: &str) -> String {
    message
        .strip_prefix(OTHER_REVISION)
        .map(|rest| rest.trim_start_matches(':').trim_start())
        .unwrap_or(message)
        .to_owned()
}

/// `library.place_from`'s refusal of another project revision.
pub fn is_other_revision(error: &pcbcraft_core::Error) -> bool {
    matches!(error, pcbcraft_core::Error::Conflict(message) if message.contains(OTHER_REVISION))
}

/// What "Update project copy" did before its review opens.
#[derive(Clone, Debug)]
pub struct UpdateImport {
    /// The instances on another revision of the imported lineage and the
    /// imported revision; `None` when no instance needs moving.
    pub change: Option<Change>,
    /// The imported revision.
    pub target: ComponentRef,
    /// The engine revision right after the import, when it copied anything
    /// in: cancelling the review undoes exactly that step.
    pub imported: Option<u64>,
}

/// "Update project copy": `library.import` of the bundle (one undo step,
/// none when the project already holds it), then the update review of the
/// instances on another revision of the imported lineage (`only` narrows
/// them, as an instance's own Update… does). The review lists every
/// instance with a checkbox.
pub fn import_for_update(
    engine: &mut Engine,
    bundle: Bundle,
    only: Option<&[pcbcraft_core::Id]>,
) -> Result<UpdateImport, String> {
    let before = engine.revision();
    let operation = Operation::Library(LibraryCommand::Import {
        bundle: Box::new(bundle),
    });
    let report = match apply(engine, operation) {
        Ok(Outcome::Imported { report }) => report,
        Ok(_) => return Err("library.import returned an unexpected outcome".into()),
        Err(error) => return Err(error.to_string()),
    };
    let target = ComponentRef::new(report.root.id, report.root.revision);
    let instances: Vec<_> = engine
        .project()
        .instances
        .iter()
        .filter(|i| i.component.id == target.id && i.component.revision != target.revision)
        .filter(|i| only.is_none_or(|only| only.contains(&i.id)))
        .map(|i| i.id)
        .collect();
    Ok(UpdateImport {
        change: (!instances.is_empty()).then(|| Change::ComponentRevision {
            instances,
            target,
            resolutions: Default::default(),
        }),
        target,
        imported: (engine.revision() != before).then(|| engine.revision()),
    })
}

/// `library.duplicate` of a component revision into new draft lineages;
/// returns the new component lineage.
pub fn duplicate(
    engine: &mut Engine,
    component: ComponentRef,
    name: &str,
) -> Result<DefinitionId, String> {
    let operation = Operation::Library(LibraryCommand::Duplicate {
        component,
        name: name.to_owned(),
    });
    match apply(engine, operation) {
        Ok(Outcome::Duplicated { lineages }) => lineages
            .first()
            .copied()
            .ok_or_else(|| "library.duplicate made no lineage".into()),
        Ok(_) => Err("library.duplicate returned an unexpected outcome".into()),
        Err(error) => Err(error.to_string()),
    }
}

fn apply(engine: &mut Engine, operation: Operation) -> pcbcraft_core::Result<Outcome> {
    engine.apply_with(
        engine.revision(),
        operation,
        &pcbcraft_solidcraft::footprint::SketchEvaluator,
    )
}

/// The default name of a duplicate.
pub fn duplicate_name(name: &str) -> String {
    format!("{name} copy")
}

/// What the Component workspace asked for (its Library section).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentAction {
    Promote {
        component: ComponentRef,
        name: String,
    },
    Duplicate {
        component: ComponentRef,
        name: String,
    },
}

fn action_slot() -> egui::Id {
    egui::Id::new("library-sources-component-action")
}

/// The Component workspace's Library section for the published revision
/// being authored: where it came from, and Promote to library… and
/// Duplicate. A choice is left for the app (see [`take_component_action`]),
/// which owns the engine and the sources.
pub fn component_section(ui: &mut egui::Ui, engine: &Engine, component: ComponentRef, name: &str) {
    let library = &engine.project().library;
    let origin = library
        .origins
        .iter()
        .filter(|o| o.local.id == component.id)
        .max_by_key(|o| o.local.revision);
    let names: Vec<(LibraryId, String)> = ui
        .ctx()
        .data(|d| d.get_temp(egui::Id::new(SOURCE_NAMES)))
        .unwrap_or_default();
    let mut action = None;
    kit::section(ui, "library-source", "Library", true, None, |ui| {
        match origin {
            Some(record) => {
                let source = names
                    .iter()
                    .find(|(l, _)| *l == record.origin.library)
                    .map_or_else(|| record.origin.library.to_string(), |(_, n)| n.clone());
                match record.direction {
                    Direction::Promoted => {
                        kit::value_row(ui, "Source", "This project");
                        kit::value_row(
                            ui,
                            "Published",
                            &format!(
                                "{} to {source} as {}",
                                record.local.revision, record.origin.revision
                            ),
                        );
                    }
                    Direction::Imported => {
                        kit::value_row(ui, "Source", &source);
                        let text = if record.origin.revision == record.local.revision {
                            format!("{source} {}", record.origin.revision)
                        } else {
                            format!(
                                "{source} {} (project {})",
                                record.origin.revision, record.local.revision
                            )
                        };
                        kit::value_row(ui, "Copied from", &text);
                    }
                }
            }
            None => kit::value_row(ui, "Source", "This project"),
        }
        kit::actions(ui, |ui| {
            if ui_kit::button(ui, "Promote to library…", Button::Normal)
                .on_hover_text("Write this revision and what it needs to a library folder")
                .clicked()
            {
                action = Some(ComponentAction::Promote {
                    component,
                    name: name.to_owned(),
                });
            }
            if ui_kit::button(ui, "Duplicate", Button::Normal)
                .on_hover_text("Copy this component, its symbols and bindings into new drafts")
                .clicked()
            {
                action = Some(ComponentAction::Duplicate {
                    component,
                    name: duplicate_name(name),
                });
            }
        });
    });
    if let Some(action) = action {
        ui.ctx().data_mut(|d| d.insert_temp(action_slot(), action));
    }
}

/// The Component workspace's last Library action, once.
pub fn take_component_action(ctx: &egui::Context) -> Option<ComponentAction> {
    ctx.data_mut(|d| {
        let action = d.get_temp::<ComponentAction>(action_slot());
        d.remove::<ComponentAction>(action_slot());
        action
    })
}

/// Source names published for the Component workspace's Library section.
const SOURCE_NAMES: &str = "library-sources-names";

/// One finished scan.
struct Scan {
    ticket: u64,
    sources: Vec<SourceView>,
}

/// A finished promote write.
struct Written {
    ticket: u64,
    result: Result<usize, String>,
}

/// A folder picked in the sources card: added as is, or made a new empty
/// library first (on the picker's thread).
enum Picked {
    Cancelled,
    Add(PathBuf),
    Failed(String),
}

/// What a bundle read is for.
#[derive(Clone, Debug, PartialEq)]
pub enum BundlePurpose {
    /// `library.place_from` with this policy.
    Place(OtherRevision),
    /// "Update project copy", for these instances only when given.
    Update(Option<Vec<pcbcraft_core::Id>>),
}

/// A finished bundle read.
pub struct ReadBundle {
    pub pick: FolderPick,
    pub purpose: BundlePurpose,
    pub result: Result<Bundle, String>,
}

enum Event {
    Bundle(ReadBundle),
    Scan(Scan),
    Written(Written),
    Picked(Picked),
    /// The settings writer finished the save with this sequence number.
    Saved(u64, Result<(), String>),
}

/// The open Promote to library… card.
struct Promote {
    component: ComponentRef,
    name: String,
    target: Option<LibraryId>,
    /// Lineage ids drawn once for this card, so the preview and the
    /// recorded promote use the same ones.
    fresh: Vec<SourceLineageId>,
    /// Plan at (document revision, target).
    plan: Option<(u64, LibraryId, Result<PromotePlan, String>)>,
    /// A write is in flight with this ticket.
    writing: Option<u64>,
    error: Option<String>,
}

/// Notices for the app's status chip.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

pub struct LibrarySources {
    dirs: Option<UserDirs>,
    settings: UserSettings,
    /// Why settings or user directories are unavailable.
    problem: Option<String>,
    sources: Vec<SourceView>,
    /// Bumped whenever `context()` would return something else.
    generation: u64,
    /// The latest scan's ticket; older scans are dropped.
    scan: u64,
    /// Promote writes' tickets, apart from scans.
    writes: u64,
    /// Settings saves sent to the writer thread, and the last it reported.
    saves: (u64, u64),
    saver: Option<Sender<(u64, UserSettings)>>,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    repaint: Option<egui::Context>,
    card: bool,
    picking: bool,
    promote: Option<Promote>,
    /// A promote write finished with this many new files; its origins are
    /// recorded on the next `tick`.
    finished: Option<usize>,
    notices: Vec<Notice>,
    /// Favourites and recents of project-only entries: their lineage ids
    /// name something else in every other project, so they are kept for
    /// the open project only, never in the per-user settings.
    session_favourites: Vec<EntryKey>,
    session_recent: Vec<EntryKey>,
    /// Finished bundle reads.
    bundles: Vec<ReadBundle>,
    /// `library.updates_available` at (project, document revision, generation).
    updates: Option<(usize, u64, u64, Arc<Vec<UpdateAvailable>>)>,
}

impl LibrarySources {
    /// Sources from the user's settings (`UserDirs::from_env`), scanned in
    /// the background.
    pub fn from_env(ctx: &egui::Context) -> Self {
        let mut sources = match UserDirs::from_env() {
            Ok(dirs) => Self::new(Some(dirs)),
            Err(error) => {
                let mut sources = Self::new(None);
                sources.problem = Some(format!("Library settings unavailable: {error}"));
                sources
            }
        };
        sources.repaint = Some(ctx.clone());
        sources.rescan();
        sources
    }

    /// Sources from `dirs` (or in-memory only), not yet scanned.
    pub fn new(dirs: Option<UserDirs>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut problem = None;
        let settings = match &dirs {
            Some(dirs) => settings::read_settings(&dirs.settings_path()).unwrap_or_else(|error| {
                problem = Some(format!("Library settings unreadable: {error}"));
                UserSettings::default()
            }),
            None => UserSettings::default(),
        };
        Self {
            sources: settings
                .sources
                .iter()
                .cloned()
                .map(SourceView::pending)
                .collect(),
            dirs,
            settings,
            problem,
            generation: 0,
            scan: 0,
            writes: 0,
            saves: (0, 0),
            saver: None,
            sender,
            receiver,
            repaint: None,
            card: false,
            picking: false,
            promote: None,
            finished: None,
            notices: Vec::new(),
            session_favourites: Vec::new(),
            session_recent: Vec::new(),
            bundles: Vec::new(),
            updates: None,
        }
    }

    pub fn sources(&self) -> &[SourceView] {
        &self.sources
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn open_card(&mut self) {
        self.card = true;
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    fn say(&mut self, text: impl Into<String>, error: bool) {
        self.notices.push(Notice {
            text: text.into(),
            error,
        });
    }

    /// What searches use: favourites, recents and every ready catalog.
    /// Project-only marks of this session come first among the recents.
    pub fn context(&self) -> CatalogContext {
        let mut recent: Vec<EntryKey> = Vec::new();
        for key in self.session_recent.iter().chain(&self.settings.recent) {
            if !recent.contains(key) {
                recent.push(*key);
            }
        }
        recent.truncate(MAX_CONTEXT);
        CatalogContext {
            favourites: self
                .settings
                .favourites
                .iter()
                .chain(&self.session_favourites)
                .copied()
                .take(MAX_CONTEXT)
                .collect(),
            recent,
            sources: self
                .sources
                .iter()
                .filter_map(|s| match &s.status {
                    Status::Ready { catalog, .. } => Some(Arc::clone(catalog)),
                    _ => None,
                })
                .collect(),
        }
    }

    #[cfg(test)]
    pub fn is_favourite(&self, key: &EntryKey) -> bool {
        self.settings.favourites.contains(key)
    }

    /// Cmd+D in the palette: mark or unmark a favourite. A library key is
    /// persisted per user; a project-only key is kept for this project.
    pub fn toggle_favourite(&mut self, key: EntryKey) {
        let list = match key {
            EntryKey::Library { .. } => &mut self.settings.favourites,
            EntryKey::Project { .. } => &mut self.session_favourites,
        };
        if let Some(at) = list.iter().position(|k| *k == key) {
            list.remove(at);
        } else {
            list.insert(0, key);
            list.truncate(MAX_FAVOURITES);
        }
        self.marked(key);
    }

    /// A placed or picked entry, most recent first: persisted per user for
    /// a library key, kept for this project for a project-only key.
    pub fn used(&mut self, key: EntryKey) {
        let list = match key {
            EntryKey::Library { .. } => &mut self.settings.recent,
            EntryKey::Project { .. } => &mut self.session_recent,
        };
        list.retain(|k| *k != key);
        list.insert(0, key);
        list.truncate(MAX_RECENT);
        self.marked(key);
    }

    fn marked(&mut self, key: EntryKey) {
        match key {
            EntryKey::Library { .. } => self.changed(),
            EntryKey::Project { .. } => self.generation = self.generation.wrapping_add(1),
        }
    }

    /// Another document was opened: forget the project-only marks, whose
    /// lineage ids meant something in the old project only.
    pub fn forget_project(&mut self) {
        if !self.session_favourites.is_empty() || !self.session_recent.is_empty() {
            self.session_favourites.clear();
            self.session_recent.clear();
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// `library.updates_available` against the ready sources, kept until
    /// the document or the sources change.
    pub fn updates(&mut self, engine: &Engine) -> Arc<Vec<UpdateAvailable>> {
        let key = (
            Arc::as_ptr(engine.project_arc()) as usize,
            engine.revision(),
            self.generation,
        );
        if let Some((p, r, g, updates)) = &self.updates
            && (*p, *r, *g) == key
        {
            return Arc::clone(updates);
        }
        let sources = self.context().sources;
        let updates = Arc::new(
            pcbcraft_core::catalog::updates_available(&engine.project().library, &sources)
                .unwrap_or_default(),
        );
        self.updates = Some((key.0, key.1, key.2, Arc::clone(&updates)));
        updates
    }

    fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.persist();
    }

    /// Save the settings on the writer thread: one thread, so saves land in
    /// order, and a burst of changes writes only the newest.
    fn persist(&mut self) {
        let Some(dirs) = &self.dirs else {
            return;
        };
        let saver = self.saver.get_or_insert_with(|| {
            let (saver, saves) = mpsc::channel::<(u64, UserSettings)>();
            let path = dirs.settings_path();
            let events = self.sender.clone();
            std::thread::spawn(move || {
                while let Ok(mut save) = saves.recv() {
                    while let Ok(newer) = saves.try_recv() {
                        save = newer;
                    }
                    let result =
                        settings::write_settings(&path, &save.1).map_err(|e| e.to_string());
                    if events.send(Event::Saved(save.0, result)).is_err() {
                        break;
                    }
                }
            });
            saver
        });
        self.saves.0 = self.saves.0.wrapping_add(1);
        if saver.send((self.saves.0, self.settings.clone())).is_err() {
            self.problem = Some("Library settings not saved: the writer stopped".into());
        }
    }

    fn saved(&mut self, sequence: u64, result: Result<(), String>) {
        self.saves.1 = sequence;
        match result {
            Ok(()) => self.problem = None,
            Err(error) => self.problem = Some(format!("Library settings not saved: {error}")),
        }
    }

    /// Add a folder as a source (enabled, writable) and rescan.
    pub fn add(&mut self, path: PathBuf) -> Result<(), String> {
        if self.settings.sources.iter().any(|s| s.path == path) {
            return Err(format!("{} is already a library source", path.display()));
        }
        if self.settings.sources.len() >= MAX_SOURCES {
            return Err(format!("At most {MAX_SOURCES} library sources"));
        }
        self.settings.sources.push(SourceSetting {
            path,
            enabled: true,
            writable: true,
            name: None,
        });
        self.persist();
        self.rescan();
        Ok(())
    }

    pub fn remove(&mut self, at: usize) {
        if at < self.settings.sources.len() {
            self.settings.sources.remove(at);
            self.persist();
            self.rescan();
        }
    }

    pub fn set_enabled(&mut self, at: usize, enabled: bool) {
        if let Some(source) = self.settings.sources.get_mut(at) {
            source.enabled = enabled;
            self.persist();
            self.rescan();
        }
    }

    pub fn set_writable(&mut self, at: usize, writable: bool) {
        if let Some(source) = self.settings.sources.get_mut(at) {
            source.writable = writable;
            if let Some(view) = self.sources.get_mut(at) {
                view.setting.writable = writable;
            }
            self.persist();
        }
    }

    /// Re-read every source in the background. Sources show "loading" until
    /// the scan reports; only the newest scan is kept.
    pub fn rescan(&mut self) {
        self.scan = self.scan.wrapping_add(1);
        let ticket = self.scan;
        let settings = self.settings.sources.clone();
        self.sources = settings.iter().cloned().map(SourceView::pending).collect();
        self.generation = self.generation.wrapping_add(1);
        let cache = self.dirs.as_ref().map(|d| d.cache.clone());
        let sender = self.sender.clone();
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let mut sources: Vec<SourceView> = settings
                .iter()
                .map(|setting| load(setting, cache.as_deref()))
                .collect();
            mark_duplicates(&mut sources);
            let _ = sender.send(Event::Scan(Scan { ticket, sources }));
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
    }

    /// Take finished background work. Returns whether anything changed.
    pub fn receive(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.receiver.try_recv() {
            changed = true;
            match event {
                Event::Scan(scan) if scan.ticket == self.scan => self.scanned(scan.sources),
                Event::Scan(_) => {}
                Event::Written(written) => self.written(written),
                Event::Bundle(read) => self.bundles.push(read),
                Event::Picked(picked) => self.picked(picked),
                Event::Saved(sequence, result) => self.saved(sequence, result),
            }
        }
        changed
    }

    /// Block until the current scan and every settings save reported.
    #[cfg(test)]
    pub fn wait(&mut self) -> Result<(), String> {
        while self
            .sources
            .iter()
            .any(|s| matches!(s.status, Status::Loading))
            || self.saves.1 != self.saves.0
        {
            match self
                .receiver
                .recv_timeout(std::time::Duration::from_secs(20))
            {
                Ok(Event::Scan(scan)) if scan.ticket == self.scan => self.scanned(scan.sources),
                Ok(Event::Written(written)) => self.written(written),
                Ok(Event::Saved(sequence, result)) => self.saved(sequence, result),
                Ok(Event::Bundle(read)) => self.bundles.push(read),
                Ok(_) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    }

    /// A finished scan: keep each readable manifest's name in the settings,
    /// so a folder that goes missing is still listed as its library.
    fn scanned(&mut self, sources: Vec<SourceView>) {
        let mut renamed = false;
        for view in &sources {
            let Some(name) = (match &view.status {
                Status::Ready { folder, .. } => Some(folder.manifest.name.clone()),
                Status::Disabled(Disabled::Off) => Some(view.name.clone()),
                _ => None,
            }) else {
                continue;
            };
            if let Some(setting) = self
                .settings
                .sources
                .iter_mut()
                .find(|s| s.path == view.setting.path)
                && setting.name.as_deref() != Some(name.as_str())
            {
                setting.name = Some(name);
                renamed = true;
            }
        }
        self.sources = sources;
        self.generation = self.generation.wrapping_add(1);
        if renamed {
            self.persist();
        }
    }

    fn folder(&self, library: LibraryId) -> Result<&Folder, String> {
        self.sources
            .iter()
            .find_map(|s| match &s.status {
                Status::Ready { folder, .. } if folder.manifest.library == library => Some(folder),
                _ => None,
            })
            .ok_or_else(|| "That library source is not available any more".into())
    }

    /// Read the bundle of a folder entry (its revision and closure) on a
    /// worker: `read_bundle` reads only the closure's files, but that is
    /// still file I/O, kept off the UI thread. The result comes back from
    /// [`Self::take_bundles`] tagged with `purpose`.
    pub fn read_bundle(&mut self, pick: FolderPick, purpose: BundlePurpose) {
        let folder = match self.folder(pick.library) {
            Ok(folder) => folder.clone(),
            Err(error) => {
                self.bundles.push(ReadBundle {
                    pick,
                    purpose,
                    result: Err(error),
                });
                return;
            }
        };
        let sender = self.sender.clone();
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let result = folder::read_bundle(
                &folder,
                SourceRevision {
                    lineage: pick.lineage,
                    revision: pick.revision,
                },
            )
            .map_err(|e| format!("Reading {} failed: {e}", pick.name));
            let _ = sender.send(Event::Bundle(ReadBundle {
                pick,
                purpose,
                result,
            }));
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
    }

    /// Bundles read since the last call, in the order they finished.
    pub fn take_bundles(&mut self) -> Vec<ReadBundle> {
        std::mem::take(&mut self.bundles)
    }

    /// Ready sources `library.promote` may write to.
    fn writable(&self) -> Vec<(LibraryId, String)> {
        self.sources
            .iter()
            .filter(|s| s.setting.writable)
            .filter_map(|s| s.library().map(|l| (l, s.name.clone())))
            .collect()
    }

    /// Open the Promote to library… card for a component revision.
    pub fn begin_promote(&mut self, component: ComponentRef, name: String) {
        let fresh = (0..MAX_FRESH_LINEAGES)
            .map(|_| folder::new_lineage_id())
            .collect::<Result<Vec<_>, _>>();
        let (fresh, error) = match fresh {
            Ok(fresh) => (fresh, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        self.promote = Some(Promote {
            component,
            name,
            target: self.writable().first().map(|(l, _)| *l),
            fresh,
            plan: None,
            writing: None,
            error,
        });
    }

    /// `library.promote_preview` for the open card, recomputed when the
    /// document or the target changed.
    fn refresh_plan(&mut self, engine: &Engine) {
        let names = self.writable();
        let Some(promote) = &mut self.promote else {
            return;
        };
        let Some(target) = promote.target else {
            promote.plan = None;
            return;
        };
        let fresh = promote
            .plan
            .as_ref()
            .is_some_and(|(r, l, _)| *r == engine.revision() && *l == target);
        if fresh || promote.writing.is_some() {
            return;
        }
        let name = names
            .iter()
            .find(|(l, _)| *l == target)
            .map_or_else(String::new, |(_, n)| n.clone());
        let plan = match engine.query(Query::PromotePreview {
            root: promote.component.erased(),
            library: target,
            name,
            fresh: promote.fresh.clone(),
        }) {
            Ok(QueryResult::Promote(plan)) => Ok(*plan),
            Ok(_) => Err("unexpected report".into()),
            Err(error) => Err(error.to_string()),
        };
        promote.plan = Some((engine.revision(), target, plan));
    }

    /// Write the previewed bundle to the folder in the background; the
    /// origins are recorded once the write reports.
    fn start_write(&mut self) {
        let Some(promote) = &self.promote else {
            return;
        };
        let Some((_, library, Ok(plan))) = &promote.plan else {
            return;
        };
        let folder = match self.folder(*library) {
            Ok(folder) => folder.clone(),
            Err(error) => {
                if let Some(promote) = &mut self.promote {
                    promote.error = Some(error);
                }
                return;
            }
        };
        let bundle = plan.bundle.clone();
        self.writes = self.writes.wrapping_add(1);
        let ticket = self.writes;
        let sender = self.sender.clone();
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let result = folder::write_bundle(&folder, &bundle)
                .map(|report| report.written.len())
                .map_err(|e| e.to_string());
            let _ = sender.send(Event::Written(Written { ticket, result }));
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
        if let Some(promote) = &mut self.promote {
            promote.writing = Some(ticket);
            promote.error = None;
        }
    }

    fn written(&mut self, written: Written) {
        let Some(promote) = &mut self.promote else {
            return;
        };
        if promote.writing != Some(written.ticket) {
            return;
        }
        match written.result {
            Ok(count) => {
                // Recorded by `finish_promote` with the engine.
                promote.writing = None;
                promote.error = None;
                self.finished = Some(count);
            }
            Err(error) => {
                promote.writing = None;
                promote.error = Some(format!("Writing the library failed: {error}"));
            }
        }
    }
}

/// What one sources-card frame asked for.
enum CardAction {
    Remove(usize),
    Enabled(usize, bool),
    Writable(usize, bool),
    Add,
    Create,
    Rescan,
    Reveal(PathBuf),
}

/// The file manager's name for revealing a folder.
const REVEAL: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show folder"
};

/// Show a folder in the system file manager, without waiting for it.
fn reveal(path: &Path) {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg("-R");
        command
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("explorer")
    } else {
        std::process::Command::new("xdg-open")
    };
    // Spawned, never waited on: a slow file manager cannot block the UI.
    let _ = command.arg(path).spawn();
}

/// A dim layer under a card that blocks the canvas and cards beneath it;
/// a click on it closes the card. The card is kept above it.
fn scrim(ctx: &egui::Context, card: egui::Id) -> bool {
    let t = solidcraft_ui_egui::theme::Tokens::get();
    let id = card.with("scrim");
    let rect = ctx.content_rect();
    let clicked = ui_kit::Container::new(ctx, ui_kit::Surface::InputShield)
        .area(id)
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .movable(false)
        .show(ctx, |ui| {
            let (r, response) = ui.allocate_exact_size(rect.size(), egui::Sense::click());
            ui.painter()
                .rect_filled(r, 0.0, t.tab_on_fill_dark.gamma_multiply(0.25));
            response.clicked()
        })
        .inner;
    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, id));
    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, card));
    clicked
}

impl LibrarySources {
    fn picked(&mut self, picked: Picked) {
        self.picking = false;
        match picked {
            Picked::Cancelled => {}
            Picked::Add(path) => {
                if let Err(error) = self.add(path) {
                    self.say(error, true);
                }
            }
            Picked::Failed(error) => self.say(error, true),
        }
    }

    /// Pick a folder without blocking the UI: the native dialog's future is
    /// made here, on the event thread, and awaited on a worker, as the file
    /// workflow does. `create` makes it an empty library first.
    fn pick(&mut self, create: bool) {
        if self.picking {
            return;
        }
        self.picking = true;
        let dialog = rfd::AsyncFileDialog::new().set_title(if create {
            "Choose an empty folder for the new library"
        } else {
            "Add a library folder"
        });
        let future = dialog.pick_folder();
        let sender = self.sender.clone();
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let picked = match pollster::block_on(future).map(|h| h.path().to_path_buf()) {
                None => Picked::Cancelled,
                Some(path) if create => match folder::create_folder(&path, &folder_name(&path)) {
                    Ok(_) => Picked::Add(path),
                    Err(error) => Picked::Failed(format!(
                        "Creating a library in {} failed: {error}",
                        path.display()
                    )),
                },
                Some(path) => Picked::Add(path),
            };
            let _ = sender.send(Event::Picked(picked));
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
    }

    /// Per frame: take background results, keep the promote preview current
    /// and record a finished promote's origins (`library.promote`, one undo
    /// step, at the revision it was previewed at).
    pub fn tick(&mut self, ctx: &egui::Context, engine: &mut Engine, busy: bool) {
        self.receive();
        let names: Vec<(LibraryId, String)> = self
            .sources
            .iter()
            .filter_map(|s| s.library().map(|l| (l, s.name.clone())))
            .collect();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(SOURCE_NAMES), names));
        // A file operation in flight owns the document; record afterwards.
        if !busy && self.finished.take().is_some() {
            self.finish_promote(engine);
        }
        self.refresh_plan(engine);
    }

    fn finish_promote(&mut self, engine: &mut Engine) {
        let Some(promote) = &self.promote else {
            return;
        };
        let Some((revision, library, Ok(plan))) = &promote.plan else {
            return;
        };
        let operation = Operation::Library(LibraryCommand::Promote {
            root: promote.component.erased(),
            library: *library,
            name: plan.bundle.name.clone(),
            fresh: promote.fresh.clone(),
        });
        let source = plan.bundle.name.clone();
        let records = plan.bundle.records.len();
        match engine.apply_with(
            *revision,
            operation,
            &pcbcraft_solidcraft::footprint::SketchEvaluator,
        ) {
            Ok(_) => {
                self.say(
                    format!(
                        "Promoted {} to {source} ({records} revision{})",
                        promote.name,
                        if records == 1 { "" } else { "s" }
                    ),
                    false,
                );
                self.promote = None;
                self.rescan();
            }
            Err(error) => {
                if let Some(promote) = &mut self.promote {
                    promote.error = Some(format!(
                        "Written to {source}, but the project was not updated: {error}"
                    ));
                }
                self.rescan();
            }
        }
    }

    /// The sources card and the Promote to library… card.
    pub fn dialogs(&mut self, ctx: &egui::Context) {
        if self.card {
            self.sources_card(ctx);
        }
        if self.promote.is_some() {
            self.promote_card(ctx);
        }
    }

    fn sources_card(&mut self, ctx: &egui::Context) {
        let mut action = None;
        let settings_path = self.dirs.as_ref().map(UserDirs::settings_path);
        let id = egui::Id::new("library-sources-card");
        if scrim(ctx, id) {
            self.card = false;
            return;
        }
        let close = ui_kit::Window::new(id, "library", "Library sources").show_body(ctx, |ui| {
            body(ui, |ui| {
                if let Some(problem) = &self.problem {
                    kit::status(ui, problem, Tone::Error);
                }
                if self.sources.is_empty() {
                    kit::status(
                        ui,
                        "No library folders yet. Add a folder that holds a pcbcraft-library.json, or make a new one.",
                        Tone::Dim,
                    );
                }
                for (at, source) in self.sources.iter().enumerate() {
                    ui.push_id(("library-source", at), |ui| {
                        kit::section(ui, "source", &source.name, true, None, |ui| {
                            let path = source.setting.path.display().to_string();
                            kit::value_row(ui, "Folder", &folder_name(&source.setting.path));
                            match &source.status {
                                Status::Ready { index, .. } => {
                                    kit::value_row(
                                        ui,
                                        "Status",
                                        &format!(
                                            "{} · {}",
                                            source.secondary(),
                                            match index {
                                                IndexState::Cached => "index cached",
                                                IndexState::Rebuilt => "index rebuilt",
                                                IndexState::Uncached(_) => "index not cached",
                                            }
                                        ),
                                    );
                                }
                                Status::Loading => {
                                    kit::value_row(ui, "Status", "loading…");
                                }
                                _ => {
                                    kit::status(
                                        ui,
                                        &format!("Not searchable: {}", source.secondary()),
                                        Tone::Warning,
                                    );
                                }
                            }
                            let mut enabled = source.setting.enabled;
                            if kit::check_row(ui, "Search this source", &mut enabled) {
                                action = Some(CardAction::Enabled(at, enabled));
                            }
                            let mut writable = source.setting.writable;
                            if kit::check_row(ui, "Promote may write here", &mut writable) {
                                action = Some(CardAction::Writable(at, writable));
                            }
                            kit::actions(ui, |ui| {
                                if ui_kit::button(ui, "Remove", Button::Normal)
                                    .on_hover_text("Stop using this folder; its files stay on disk")
                                    .clicked()
                                {
                                    action = Some(CardAction::Remove(at));
                                }
                                if ui_kit::button(ui, REVEAL, Button::Normal)
                                    .on_hover_text(&path)
                                    .clicked()
                                {
                                    action = Some(CardAction::Reveal(source.setting.path.clone()));
                                }
                            });
                        });
                    });
                }
                kit::note(
                    ui,
                    "Sources, favourites and recents are yours, kept in your user settings, never in a project.",
                )
                .on_hover_text(
                    settings_path
                        .as_ref()
                        .map_or_else(String::new, |p| p.display().to_string()),
                );
                kit::rule(ui);
                kit::actions(ui, |ui| {
                    if kit::primary(ui, "Add folder…", !self.picking).clicked() {
                        action = Some(CardAction::Add);
                    }
                    if ui_kit::button(ui, "New library…", Button::Enabled(!self.picking))
                        .on_hover_text("Make an empty library in a folder you choose")
                        .clicked()
                    {
                        action = Some(CardAction::Create);
                    }
                    if ui_kit::button(ui, "Rescan", Button::Normal)
                        .on_hover_text("Re-read every folder and refresh its index")
                        .clicked()
                    {
                        action = Some(CardAction::Rescan);
                    }
                });
            });
        });
        if close {
            self.card = false;
        }
        match action {
            Some(CardAction::Remove(at)) => self.remove(at),
            Some(CardAction::Enabled(at, on)) => self.set_enabled(at, on),
            Some(CardAction::Writable(at, on)) => self.set_writable(at, on),
            Some(CardAction::Add) => self.pick(false),
            Some(CardAction::Create) => self.pick(true),
            Some(CardAction::Rescan) => self.rescan(),
            Some(CardAction::Reveal(path)) => reveal(&path),
            None => {}
        }
    }

    fn promote_card(&mut self, ctx: &egui::Context) {
        let writable = self.writable();
        let Some(promote) = &mut self.promote else {
            return;
        };
        let mut write = false;
        let mut cancel = false;
        let mut sources = false;
        let title = format!("Promote {} to a library", promote.name);
        let close = ui_kit::Window::new(egui::Id::new("library-promote"), "library", &title)
            .show_body(ctx, |ui| {
                body(ui, |ui| {
                    kit::value_row(
                        ui,
                        "Component",
                        &format!("{} · {}", promote.name, promote.component.revision),
                    );
                    if writable.is_empty() {
                        kit::status(
                            ui,
                            "No library source accepts writes. Add a library folder and allow promoting to it.",
                            Tone::Warning,
                        );
                        kit::actions(ui, |ui| {
                            sources = ui_kit::button(ui, "Library sources…", Button::Normal)
                                .clicked();
                        });
                    } else {
                        let options: Vec<(Option<LibraryId>, String)> = writable
                            .iter()
                            .map(|(l, n)| (Some(*l), n.clone()))
                            .collect();
                        kit::choice_row(ui, "Library", &mut promote.target, &options);
                    }
                    match &promote.plan {
                        Some((_, _, Ok(plan))) => {
                            let records = plan.bundle.records.len();
                            let new = plan.origins.len();
                            kit::value_row(
                                ui,
                                "Revisions",
                                &format!("{records} · {new} to write"),
                            );
                            kit::section(
                                ui,
                                "promote-records",
                                "What is written",
                                true,
                                Some(records),
                                |ui| {
                                    let written: std::collections::BTreeSet<_> = plan
                                        .origins
                                        .iter()
                                        .map(|o| (o.origin.lineage, o.origin.revision))
                                        .collect();
                                    for record in &plan.bundle.records {
                                        let state = if written
                                            .contains(&(record.lineage, record.revision))
                                        {
                                            "new"
                                        } else {
                                            "already in the library"
                                        };
                                        kit::value_row(
                                            ui,
                                            kind_label(record.kind),
                                            &format!(
                                                "{} · {} · {state}",
                                                record.meta.name, record.revision
                                            ),
                                        );
                                    }
                                },
                            );
                            kit::note(
                                ui,
                                "Each revision is written once as its own file; a file already in the library with equal content is kept. The project records where each revision went, as one undo step.",
                            );
                        }
                        Some((_, _, Err(error))) => {
                            kit::status(ui, error, Tone::Error);
                        }
                        None => {}
                    }
                    if promote.writing.is_some() {
                        kit::status(ui, "Writing to the library…", Tone::Note);
                    }
                    if let Some(error) = &promote.error {
                        kit::status(ui, error, Tone::Error);
                    }
                    kit::rule(ui);
                    let ready = promote.writing.is_none()
                        && matches!(promote.plan, Some((_, _, Ok(_))));
                    kit::actions(ui, |ui| {
                        write = kit::primary(ui, "Promote", ready).clicked() && ready;
                        cancel = ui_kit::button(ui, "Cancel", Button::Normal).clicked();
                    });
                });
            });
        if sources {
            self.card = true;
        }
        // Closing would abandon recording the origins of a write in flight,
        // so the card stays until the write reports.
        if (close || cancel) && promote.writing.is_none() {
            self.promote = None;
        } else if write {
            self.start_write();
        }
    }
}

/// How the promote preview names a record's kind.
fn kind_label(kind: pcbcraft_core::DefinitionKind) -> &'static str {
    use pcbcraft_core::DefinitionKind as K;
    match kind {
        K::Component => "Component",
        K::Symbol => "Symbol",
        K::Package => "Package",
        K::Footprint => "Footprint",
        K::Binding => "Binding",
        K::QuantityKind => "Quantity kind",
        K::Field => "Field",
        K::Part => "Part",
    }
}

/// A dialog body inset like the file workflow's.
fn body(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let t = solidcraft_ui_egui::theme::Tokens::get();
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(t.space_12 as i8, t.space_8 as i8))
        .show(ui, |ui| {
            ui.set_min_width(380.0);
            kit::form(ui, add);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::catalog::EntryKey;

    fn manifest(library: u128, name: &str) -> String {
        format!(
            "{{\"format\":{},\"library\":\"{:032x}\",\"name\":\"{name}\",\"required_features\":[]}}",
            folder::FOLDER_FORMAT,
            library
        )
    }

    fn write(dir: &Path, text: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join(folder::MANIFEST_FILE), text)
    }

    fn setting(path: PathBuf, enabled: bool) -> SourceSetting {
        SourceSetting {
            path,
            enabled,
            writable: true,
            name: None,
        }
    }

    /// A source that cannot be searched is listed with io's reason, never
    /// dropped: a missing folder, a corrupt manifest, another format, an
    /// unknown required feature and a source turned off.
    #[test]
    fn unusable_sources_keep_their_typed_reason() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let cache = dir.path().join("cache");
        let missing = load(&setting(dir.path().join("gone"), true), Some(&cache));
        assert!(matches!(
            missing.status,
            Status::Disabled(Disabled::PathMissing)
        ));
        assert_eq!(missing.name, "gone");
        assert_eq!(missing.secondary(), "path missing");

        let corrupt = dir.path().join("corrupt");
        write(&corrupt, "{ not json")?;
        let view = load(&setting(corrupt, true), Some(&cache));
        assert!(matches!(
            view.status,
            Status::Disabled(Disabled::Malformed(_))
        ));
        assert!(view.secondary().starts_with("unreadable manifest: "));

        let future = dir.path().join("future");
        write(
            &future,
            &manifest(1, "Future").replace(
                "\"required_features\":[]",
                "\"required_features\":[\"sketch-v9\"]",
            ),
        )?;
        let view = load(&setting(future, true), Some(&cache));
        assert_eq!(
            view.reason().as_deref(),
            Some("requires unsupported features: sketch-v9")
        );

        let newer = dir.path().join("newer");
        write(
            &newer,
            &manifest(2, "Newer").replace(
                &format!("\"format\":{}", folder::FOLDER_FORMAT),
                "\"format\":99",
            ),
        )?;
        let view = load(&setting(newer, true), Some(&cache));
        assert_eq!(view.reason().as_deref(), Some("unsupported format v99"));

        let off = dir.path().join("off");
        write(&off, &manifest(3, "Team parts"))?;
        let view = load(&setting(off, false), Some(&cache));
        assert_eq!(view.name, "Team parts", "the manifest names it");
        assert_eq!(view.reason().as_deref(), Some("turned off"));
        Ok(())
    }

    /// Two configured folders with one library id: the second is not
    /// searched and says which folder holds it.
    #[test]
    fn a_second_folder_of_the_same_library_is_not_searched() {
        let ready = |path: &str| {
            let root = PathBuf::from(path);
            SourceView {
                setting: setting(root.clone(), true),
                name: "Team".into(),
                status: Status::Ready {
                    folder: Folder {
                        root,
                        manifest: folder::Manifest {
                            format: folder::FOLDER_FORMAT,
                            library: LibraryId(5),
                            name: "Team".into(),
                            required_features: vec![],
                            default_licence: None,
                        },
                    },
                    catalog: Arc::new(SourceCatalog::default()),
                    index: IndexState::Rebuilt,
                },
            }
        };
        let mut sources = vec![ready("/a"), ready("/b")];
        mark_duplicates(&mut sources);
        assert!(sources[0].ready());
        assert_eq!(sources[1].reason().as_deref(), Some("same library as /a"));
    }

    /// Favourites and recents of library items are keyed by their library
    /// identity, bounded, persisted per user and read back by the next
    /// session. A project-only entry's key names something else in every
    /// project, so it ranks for the open project only: never written, and
    /// forgotten when another document opens. Every change bumps the
    /// generation the palette refreshes on.
    #[test]
    fn favourites_and_recents_persist_per_user() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let dirs = UserDirs {
            config: dir.path().join("config"),
            cache: dir.path().join("cache"),
        };
        let folder_key = EntryKey::Library {
            library: LibraryId(1),
            lineage: SourceLineageId(2),
        };
        let project_key = EntryKey::Project {
            lineage: DefinitionId(3),
        };
        let mut sources = LibrarySources::new(Some(dirs.clone()));
        let before = sources.generation();
        sources.toggle_favourite(folder_key);
        assert!(sources.generation() != before);
        sources.used(folder_key);
        for lineage in 10..10 + MAX_RECENT as u128 {
            sources.used(EntryKey::Library {
                library: LibraryId(1),
                lineage: SourceLineageId(lineage),
            });
        }
        sources.used(folder_key);
        let before = sources.generation();
        sources.used(project_key);
        sources.toggle_favourite(project_key);
        assert!(sources.generation() != before);
        let context = sources.context();
        assert_eq!(context.recent.first(), Some(&project_key), "ranked now");
        assert!(context.favourites.contains(&project_key));
        sources.wait()?;
        assert_eq!(sources.problem(), None);

        let again = LibrarySources::new(Some(dirs.clone()));
        assert!(again.is_favourite(&folder_key));
        let context = again.context();
        assert_eq!(context.recent.len(), MAX_RECENT);
        assert_eq!(context.recent.first(), Some(&folder_key));
        assert_eq!(
            context.recent.iter().filter(|k| **k == folder_key).count(),
            1,
            "a recent is listed once"
        );
        assert!(context.favourites.contains(&folder_key));
        assert!(
            !context.recent.contains(&project_key) && !context.favourites.contains(&project_key),
            "project-only marks are never kept per user"
        );
        let stored = settings::read_settings(&dirs.settings_path())?;
        assert!(
            stored
                .recent
                .iter()
                .chain(&stored.favourites)
                .all(|k| matches!(k, EntryKey::Library { .. }))
        );

        sources.forget_project();
        assert!(!sources.context().recent.contains(&project_key));
        let mut again = again;
        again.toggle_favourite(folder_key);
        again.wait()?;
        assert!(!LibrarySources::new(Some(dirs)).is_favourite(&folder_key));
        Ok(())
    }

    /// Adding a folder persists the source and scans it off the UI thread;
    /// the same folder twice is refused; removing it keeps its files.
    #[test]
    fn sources_are_added_scanned_and_removed() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let dirs = UserDirs {
            config: dir.path().join("config"),
            cache: dir.path().join("cache"),
        };
        let library = dir.path().join("team");
        write(&library, &manifest(0x42, "Team parts"))?;
        let mut sources = LibrarySources::new(Some(dirs.clone()));
        sources.add(library.clone())?;
        assert!(matches!(sources.sources()[0].status, Status::Loading));
        assert!(sources.add(library.clone()).is_err());
        sources.wait()?;
        let view = &sources.sources()[0];
        assert_eq!(view.name, "Team parts");
        assert!(
            !matches!(view.status, Status::Disabled(_) | Status::Loading),
            "a valid manifest is never disabled: {:?}",
            view.status
        );
        let reread = settings::read_settings(&dirs.settings_path())?;
        assert_eq!(reread.sources.len(), 1);
        sources.set_enabled(0, false);
        sources.wait()?;
        assert_eq!(sources.sources()[0].reason().as_deref(), Some("turned off"));
        assert!(sources.context().sources.is_empty());
        sources.remove(0);
        sources.wait()?;
        assert!(sources.sources().is_empty());
        assert!(library.join(folder::MANIFEST_FILE).exists());
        assert!(
            settings::read_settings(&dirs.settings_path())?
                .sources
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn folder_picks_are_component_entries_of_a_folder() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::from_file(pcbcraft_core::format::EditorFile {
            version: pcbcraft_core::format::FILE_VERSION,
            project: pcbcraft_core::catalog::synthetic::project(2)?,
            drafts: Default::default(),
        })?;
        let QueryResult::Search(report) = engine.query(Query::Search {
            query: String::new(),
            limit: None,
            include_drafts: false,
        })?
        else {
            return Err("not a search".into());
        };
        let mut entry = report.results.first().ok_or("an entry")?.entry.clone();
        assert_eq!(FolderPick::of(&entry), None, "a project entry");
        entry.source = pcbcraft_core::catalog::Source::Library(LibraryId(4));
        entry.source_lineage = Some(SourceLineageId(5));
        entry.revision = Some(Revision(2));
        assert_eq!(
            FolderPick::of(&entry),
            Some(FolderPick {
                library: LibraryId(4),
                lineage: SourceLineageId(5),
                revision: Revision(2),
                name: entry.name.clone(),
            })
        );
        entry.kind = pcbcraft_core::DefinitionKind::Part;
        assert!(
            FolderPick::of(&entry).is_some(),
            "a folder Part places its component with the Part assigned"
        );
        entry.kind = pcbcraft_core::DefinitionKind::Symbol;
        assert_eq!(FolderPick::of(&entry), None);
        Ok(())
    }

    #[test]
    fn the_other_revision_choice_shows_the_sentence_without_the_code() {
        assert_eq!(
            choice_text("library.other_revision: the project holds r1; Team offers r2."),
            "the project holds r1; Team offers r2."
        );
        assert_eq!(choice_text("something else"), "something else");
    }

    #[test]
    fn the_other_revision_refusal_is_recognised_by_its_code() {
        assert!(is_other_revision(&pcbcraft_core::Error::Conflict(format!(
            "{OTHER_REVISION}: the project holds r1 of this lineage"
        ))));
        assert!(!is_other_revision(&pcbcraft_core::Error::Conflict(
            "locked".into()
        )));
        assert!(!is_other_revision(&pcbcraft_core::Error::Invalid(
            OTHER_REVISION.into()
        )));
    }

    /// The bundle of `root` promoted from `author` to library 7.
    fn promoted(
        author: &mut Engine,
        root: pcbcraft_core::DefinitionRef,
        from: u128,
    ) -> Result<Bundle, Box<dyn std::error::Error>> {
        let fresh: Vec<SourceLineageId> = (from..from + 64).map(SourceLineageId).collect();
        let QueryResult::Promote(plan) = author.query(Query::PromotePreview {
            root,
            library: LibraryId(7),
            name: "Sample Parts".into(),
            fresh: fresh.clone(),
        })?
        else {
            return Err("not a promote plan".into());
        };
        apply(
            author,
            Operation::Library(LibraryCommand::Promote {
                root,
                library: LibraryId(7),
                name: "Sample Parts".into(),
                fresh,
            }),
        )?;
        Ok(plan.bundle)
    }

    /// "Update project copy" reports the import it made, so cancelling the
    /// review undoes exactly it; narrowed to one instance, the review moves
    /// only that one. A revision already in the project reports no import.
    #[test]
    fn update_project_copy_reports_its_import_for_cancel() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut author = Engine::demo()?;
        let component = author
            .project()
            .library
            .latest_components()
            .first()
            .map(|c| ComponentRef::new(c.id, c.revision))
            .ok_or("a component")?;
        let first = promoted(&mut author, component.erased(), 1)?;
        let mut design = Engine::default();
        let mut placed = Vec::new();
        for x in [0.0, 30.0] {
            match place(&mut design, first.clone(), [x, 0.0], OtherRevision::Refuse)? {
                Placed::Placed { instance, .. } => placed.push(instance),
                other => return Err(format!("unexpected {other:?}").into()),
            }
        }
        let Outcome::Published { revisions } = apply(
            &mut author,
            Operation::Library(LibraryCommand::Publish {
                drafts: vec![component.id],
            }),
        )?
        else {
            return Err("not published".into());
        };
        let second = promoted(&mut author, revisions[0], 100)?;
        // Placing the newer revision is refused with the plain sentence.
        match place(
            &mut design,
            second.clone(),
            [60.0, 0.0],
            OtherRevision::Refuse,
        )? {
            Placed::OtherRevision { message } => {
                assert!(message.starts_with("This project uses "), "{message}");
                assert!(
                    message.contains("from Sample Parts; the library now has r2."),
                    "{message}"
                );
            }
            other => return Err(format!("unexpected {other:?}").into()),
        }
        let before = design.revision();
        let kept = design.project().clone();
        let only = [placed[0]];
        let import = import_for_update(&mut design, second.clone(), Some(&only))?;
        assert_eq!(import.imported, Some(design.revision()));
        assert_ne!(design.revision(), before);
        match &import.change {
            Some(Change::ComponentRevision {
                instances, target, ..
            }) => {
                assert_eq!(instances, &vec![placed[0]], "only the chosen instance");
                assert_eq!(target.revision, Revision(2));
            }
            other => return Err(format!("unexpected {other:?}").into()),
        }
        // Cancel: the app undoes the import, and the project is as before.
        design.undo()?;
        assert_eq!(design.project(), &kept);
        // Imported and kept, a second Update project copy copies nothing.
        import_for_update(&mut design, second.clone(), None)?;
        let again = import_for_update(&mut design, second, None)?;
        assert_eq!(again.imported, None, "nothing new to undo");
        match &again.change {
            Some(Change::ComponentRevision { instances, .. }) => assert_eq!(instances.len(), 2),
            other => return Err(format!("unexpected {other:?}").into()),
        }
        Ok(())
    }

    /// The icons the Browser and cards use exist in Solidcraft's set.
    #[test]
    fn library_icons_resolve() {
        for name in [
            "library",
            "folder",
            "status_warning",
            "component",
            "list",
            "star",
            "update",
        ] {
            assert!(
                !solidcraft_ui_egui::icon_svg::resolve(name).placeholder,
                "{name}"
            );
        }
    }
}
