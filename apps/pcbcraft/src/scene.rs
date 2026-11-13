//! The 3D model pipeline: makes the board's and package previews' models
//! off the UI thread, keeps only results for the current input, and maps
//! picks back to electronics objects. Selection and highlight recolour
//! cached meshes; only a changed project or package preview rebuilds
//! geometry. The worker also packs the GPU vertices
//! ([`crate::model_scene`]), so the UI thread never walks a mesh.
//!
//! The board's model is the live view's ([`Flat::Meshed`]): its flat
//! bodies are meshed from the board plan, and each part is its package
//! revision's own design, placed by the document's transforms. The worker
//! keeps the last board model only, and makes every next board (an edit,
//! an undo, a redo, a refill) from what it kept
//! ([`pcbcraft_solidcraft::project::update`]): no earlier model is kept
//! for an undo, and no native design of the board is made. The exact model
//! is built only to export. A board's instance marking decals are made there too
//! and appended to the drawn geometry (display only, never the native
//! model); the worker keeps their per-instance cache between builds, and
//! the read and checked library model files ([`SourceCache`]), so a board
//! rebuild never re-hashes or re-reads an unchanged STEP or glTF.
use crate::model_scene::{self, ModelScene};
use crate::viewport::Viewport;
use pcbcraft_core::library::model3d::ModelDefinition;
use pcbcraft_core::library::{Definition, ModelPlacement, PackageDefinition, PackageModel};
use pcbcraft_core::{Engine, Id, Project};
use pcbcraft_solidcraft::marking::board::BoardMarkings;
use pcbcraft_solidcraft::models::imported::SourceCache;
use pcbcraft_solidcraft::project::{Flat, Source};
use pcbcraft_solidcraft::stack::Stack;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// What a model was built from. Projects compare by shared identity, so a
/// draft-only edit never rebuilds the board model.
#[derive(Clone, Debug)]
pub enum ModelKey {
    Project(Arc<Project>),
    /// A package envelope preview, keyed by its content.
    Package(Vec<u8>),
}

impl PartialEq for ModelKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Project(a), Self::Project(b)) => Arc::ptr_eq(a, b),
            (Self::Package(a), Self::Package(b)) => a == b,
            _ => false,
        }
    }
}

impl ModelKey {
    pub fn package(package: &PackageDefinition) -> Option<Self> {
        serde_json::to_vec(package).ok().map(Self::Package)
    }
    fn is_package(&self) -> bool {
        matches!(self, Self::Package(_))
    }
}

/// A package's library model as its preview builds it: the definition (a
/// published revision or the open draft's pending one) and its packed
/// asset bytes.
#[derive(Clone)]
pub struct LibraryAsset {
    pub model: ModelDefinition,
    pub bytes: pcbcraft_core::library::bundle::AssetBytes,
}

/// The library model `package` uses, resolved in `engine`'s published
/// library or, for a model draft just imported, its pending revision.
/// `Err` names what is missing; `Ok(None)` for a package without one.
pub fn library_asset(
    engine: &Engine,
    package: &PackageDefinition,
) -> Result<Option<LibraryAsset>, String> {
    let model_ref = match &package.model {
        PackageModel::Library(use_) => use_.model,
        // A part file is a Model of format `solidcraft` (docs/part-templates.md §11).
        PackageModel::PartFile(part) => part.model,
        _ => return Ok(None),
    };
    let library = &engine.project().library;
    let model = match library.model(model_ref) {
        Some(published) => published.definition.clone(),
        None => engine
            .drafts()
            .get(model_ref.id)
            .filter(|d| d.pending() == Some(model_ref.revision))
            .and_then(|d| match &d.definition {
                Definition::Model(model) => Some(model.as_ref().clone()),
                _ => None,
            })
            .ok_or_else(|| format!("The model {model_ref} is not in the library"))?,
    };
    let bytes = library
        .assets
        .iter()
        .find(|a| a.hash == model.asset.hash)
        .map(|a| a.data.clone())
        .ok_or_else(|| format!("The model file {} is not packed", model.asset.file_name))?;
    Ok(Some(LibraryAsset { model, bytes }))
}

enum Job {
    /// A board, with the flag that stops its build once a newer request
    /// replaces it.
    Project(u64, Arc<Project>, Arc<AtomicBool>),
    Package(u64, Box<PackageDefinition>, Option<Box<LibraryAsset>>),
    /// A build that panics, as a dependency's bug would.
    #[cfg(test)]
    Panic(u64),
}

impl Job {
    fn ticket(&self) -> u64 {
        match self {
            Self::Project(ticket, ..) | Self::Package(ticket, ..) => *ticket,
            #[cfg(test)]
            Self::Panic(ticket) => *ticket,
        }
    }
}

/// A built job's display geometry, pick sources and board stack.
type BuiltGeometry = Result<
    (
        Vec<pcbcraft_solidcraft::scene::Geometry>,
        Vec<Source>,
        Stack,
    ),
    String,
>;

type BoardModel = pcbcraft_solidcraft::project::NativeModel;

/// What the worker keeps between builds.
#[derive(Default)]
struct Kept {
    /// The last board model and the board it was made from, for the next
    /// board (an edit, an undo or a redo alike) to be made from.
    board: Option<(Arc<Project>, BoardModel)>,
    markings: BoardMarkings,
    sources: SourceCache,
}

/// How the worker made a board model, for `PCBCRAFT_TIMINGS`.
enum Made {
    /// The last model, updated or built anew (and why).
    Updated(pcbcraft_solidcraft::project::Rebuilt),
    /// Built: the worker kept no board model.
    Built,
}

impl Made {
    /// With `PCBCRAFT_TIMINGS` set, print how board request `ticket` was
    /// made, why an update fell back to a build, and how long it took.
    fn report<T, E: std::fmt::Display>(&self, ticket: u64, took: Duration, made: &Result<T, E>) {
        if std::env::var_os("PCBCRAFT_TIMINGS").is_none() {
            return;
        }
        use pcbcraft_solidcraft::project::Rebuilt;
        let how = match (self, made) {
            (_, Err(error)) => format!("failed: {error}"),
            (Self::Updated(Rebuilt::InPlace), _) => "updated from the last model".to_owned(),
            (Self::Updated(Rebuilt::Anew(Some(why))), _) => format!("built anew: {why}"),
            (Self::Updated(Rebuilt::Anew(None)) | Self::Built, _) => "built".to_owned(),
        };
        eprintln!(
            "pcbcraft: board model {ticket} {how} in {:.2} s",
            took.as_secs_f64()
        );
    }
}

/// Build one job's display geometry and pick sources.
fn build(job: Job, kept: &mut Kept) -> (u64, BuiltGeometry) {
    match job {
        Job::Project(ticket, project, cancel) => {
            let started = std::time::Instant::now();
            let (made, how) = match kept.board.take() {
                Some((_, previous)) => match pcbcraft_solidcraft::project::update(
                    previous,
                    &project,
                    &kept.sources,
                    Some(cancel),
                ) {
                    Ok((model, how)) => (Ok(model), Made::Updated(how)),
                    Err(error) => (Err(error), Made::Built),
                },
                None => (
                    pcbcraft_solidcraft::project::build_flat(
                        &project,
                        &kept.sources,
                        Some(cancel),
                        Flat::Meshed,
                    ),
                    Made::Built,
                ),
            };
            how.report(ticket, started.elapsed(), &made);
            let built = made.map(|model| {
                // The drawn geometry shares the meshes; the model stays
                // for the next edit to update, without the decals.
                let mut geometry = model.geometry.clone();
                geometry.extend(
                    kept.markings
                        .decals(&project, &model.geometry, &model.sources),
                );
                for (instance, why) in kept.markings.failures() {
                    eprintln!("pcbcraft: instance {instance} shows no marking: {why}");
                }
                let sources = model.sources.clone();
                kept.board = Some((Arc::clone(&project), model));
                (
                    geometry,
                    sources,
                    Stack::of(&project.stackup, project.fabrication.pad_finish),
                )
            });
            (ticket, built.map_err(|e| e.to_string()))
        }
        Job::Package(ticket, package, library) => {
            let built = match (&package.model, library) {
                (PackageModel::Library(use_), Some(asset)) => {
                    pcbcraft_solidcraft::models::imported::preview(
                        &asset.model,
                        use_,
                        &asset.bytes,
                        &ModelPlacement::default(),
                    )
                    .map(|geometry| (geometry, Vec::new(), Stack::none()))
                }
                (PackageModel::PartFile(_), Some(asset)) => {
                    pcbcraft_solidcraft::project::preview_part(&package, &asset.bytes)
                        .map(|model| (model.geometry, model.sources, Stack::none()))
                }
                _ => pcbcraft_solidcraft::project::preview(&package)
                    .map(|model| (model.geometry, model.sources, Stack::none())),
            };
            (ticket, built.map_err(|e| e.to_string()))
        }
        #[cfg(test)]
        Job::Panic(_) => std::panic::resume_unwind(Box::new("a build panicked")),
    }
}

struct Built {
    ticket: u64,
    result: Result<(ModelScene, Vec<Source>), String>,
}

/// One background thread building native models. Only the newest queued
/// request is built; older ones are skipped, and a board build a newer
/// request replaced stops at its next native command. The last board
/// model stays with the thread, for the next board request to update.
struct Worker {
    sender: Sender<Job>,
    receiver: Receiver<Built>,
}

impl Worker {
    fn start(ctx: egui::Context) -> Self {
        let (sender, jobs) = mpsc::channel::<Job>();
        let (results, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut kept = Kept::default();
            // A request that came while a meshed board was made.
            let mut next: Option<Job> = None;
            loop {
                let mut job = match next.take() {
                    Some(job) => job,
                    None => match jobs.recv() {
                        Ok(job) => job,
                        Err(_) => break,
                    },
                };
                while let Ok(newer) = jobs.try_recv() {
                    job = newer;
                }
                let ticket = job.ticket();
                let board = matches!(job, Job::Project(..));
                // A build that panics (a bug in a dependency) fails its
                // request; the worker goes on with the next one, from no
                // kept model, so the view never waits on a dead worker.
                let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let (_, result) = build(job, &mut kept);
                    // A board's next model is made in a fraction of the
                    // time packing this one takes: once a newer request
                    // waits (a refill after an edit), this one is kept for
                    // that model to update, and not packed or shown.
                    if board
                        && result.is_ok()
                        && let Ok(newer) = jobs.try_recv()
                    {
                        next = Some(newer);
                        return None;
                    }
                    let started = std::time::Instant::now();
                    let result = result.and_then(|(geometry, sources, stack)| {
                        model_scene::build(geometry, &stack).map(|scene| (scene, sources))
                    });
                    Some((result, started))
                }));
                let (result, started) = match made {
                    Ok(Some(made)) => made,
                    Ok(None) => continue,
                    Err(_) => {
                        kept = Kept::default();
                        (
                            Err("The 3D model could not be built (internal error)".to_owned()),
                            std::time::Instant::now(),
                        )
                    }
                };
                if std::env::var_os("PCBCRAFT_TIMINGS").is_some() {
                    eprintln!(
                        "pcbcraft: model {ticket} packed for drawing in {:.2} s",
                        started.elapsed().as_secs_f64()
                    );
                }
                if results.send(Built { ticket, result }).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self { sender, receiver }
    }
}

pub enum Status {
    Idle,
    Building,
    Ready,
    Failed(String),
}

/// How the canvas shows a requested model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    /// Nothing of the requested kind was shown yet: a status message only.
    Blank,
    /// The last model of the same kind, under an "Updating" or error note;
    /// picks on it are ignored.
    Stale,
    /// Exactly the requested model.
    Current,
}

/// A project model is never shown for a package request or the other way
/// round; a shown model of the right kind stays up while a newer one builds.
pub fn display(shown: Option<&ModelKey>, requested: &ModelKey, ready: bool) -> Display {
    match shown {
        Some(shown) if shown == requested && ready => Display::Current,
        Some(shown) if shown.is_package() == requested.is_package() => Display::Stale,
        _ => Display::Blank,
    }
}

/// How long a board must stay unchanged before its 3D model is prepared in
/// the background: a drag or a burst of edits is built once, at its end.
pub const PREFETCH_DELAY: Duration = Duration::from_millis(350);

/// The debounce of background model preparation: what was last asked for,
/// and since when (seconds of input time).
#[derive(Default)]
pub struct Prefetch {
    pending: Option<(ModelKey, f64)>,
}

/// What [`Prefetch::due`] says to do now.
#[derive(Debug, PartialEq)]
pub enum Due {
    /// Already requested (or built): nothing to do.
    Nothing,
    /// Build it now.
    Build,
    /// Ask again after this long.
    Wait(Duration),
}

impl Prefetch {
    /// Whether `key`, wanted at `now`, is due to be built, given what the
    /// worker was last asked for.
    pub fn due(&mut self, key: &ModelKey, now: f64, requested: Option<&ModelKey>) -> Due {
        if requested == Some(key) {
            self.pending = None;
            return Due::Nothing;
        }
        let delay = PREFETCH_DELAY.as_secs_f64();
        match &self.pending {
            Some((pending, since)) if pending == key => {
                let left = delay - (now - since);
                if left <= 0.0 || !left.is_finite() {
                    self.pending = None;
                    Due::Build
                } else {
                    Due::Wait(Duration::from_secs_f64(left.min(delay)))
                }
            }
            _ => {
                self.pending = Some((key.clone(), now));
                Due::Wait(PREFETCH_DELAY)
            }
        }
    }
}

pub struct SceneController {
    worker: Worker,
    pub viewport: Viewport,
    prefetch: Prefetch,
    /// Stops the board build in flight once a newer request replaces it.
    cancel: Option<Arc<AtomicBool>>,
    ticket: u64,
    key: Option<ModelKey>,
    /// Requests sent to the worker and not answered yet, oldest first.
    sent: Vec<(u64, ModelKey)>,
    shown: Option<ModelKey>,
    /// The request whose model is shown.
    shown_ticket: u64,
    sources: Vec<Source>,
    status: Status,
    /// The camera each kind of model (board, package) was last seen with,
    /// so switching workspaces keeps each view's framing.
    cameras: [Option<pcbcraft_solidcraft::viewport::Camera>; 2],
    /// Another document was opened: its first board model is framed.
    fit_board: bool,
    /// Results taken from the worker, current or dropped.
    #[cfg(test)]
    received: u64,
}

impl SceneController {
    pub fn new(ctx: egui::Context, viewport: Viewport) -> Self {
        Self {
            worker: Worker::start(ctx),
            viewport,
            prefetch: Prefetch::default(),
            cancel: None,
            ticket: 0,
            key: None,
            sent: Vec::new(),
            shown: None,
            shown_ticket: 0,
            sources: vec![],
            status: Status::Idle,
            cameras: [None; 2],
            fit_board: false,
            #[cfg(test)]
            received: 0,
        }
    }

    /// Another project was opened (or a new one made): the board camera
    /// kept was the previous project's, so the new board's first model is
    /// framed instead. Later models of it keep the camera.
    ///
    /// Nothing of the previous project is shown again: a build in flight
    /// stops at its next native command, a result still to come (an update
    /// always finishes) is dropped, and the view shows the building state
    /// until the opened project's own model arrives.
    pub fn document_opened(&mut self) {
        self.cameras[0] = None;
        self.fit_board = true;
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        match self.ticket.checked_add(1) {
            Some(ticket) => {
                self.ticket = ticket;
                self.status = Status::Idle;
            }
            None => self.status = Status::Failed("model request identity exhausted".into()),
        }
        self.key = None;
        self.sent.clear();
        self.shown = None;
        self.sources.clear();
        self.prefetch = Prefetch::default();
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// How the viewport can show `key` now.
    pub fn display(&self, key: &ModelKey) -> Display {
        display(
            self.shown.as_ref(),
            key,
            matches!(self.status, Status::Ready),
        )
    }

    /// Ask for a model of `key` unless it is already requested. A package
    /// preview of a library model needs its `library` asset
    /// ([`library_asset`]), resolved only for a new request; a missing one
    /// is shown as the failure.
    pub fn request(
        &mut self,
        key: ModelKey,
        package: Option<&PackageDefinition>,
        library: impl FnOnce() -> Result<Option<LibraryAsset>, String>,
    ) {
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let Some(ticket) = self.ticket.checked_add(1) else {
            self.status = Status::Failed("model request identity exhausted".into());
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let job = match (&key, package) {
            (ModelKey::Project(project), _) => {
                Job::Project(ticket, Arc::clone(project), Arc::clone(&cancel))
            }
            (ModelKey::Package(_), Some(package)) => match library() {
                Ok(library) => {
                    Job::Package(ticket, Box::new(package.clone()), library.map(Box::new))
                }
                Err(error) => {
                    self.ticket = ticket;
                    self.key = Some(key);
                    self.status = Status::Failed(error);
                    return;
                }
            },
            (ModelKey::Package(_), None) => return,
        };
        match self.worker.sender.send(job) {
            Ok(()) => {
                if let Some(previous) = self.cancel.replace(cancel) {
                    previous.store(true, Ordering::Relaxed);
                }
                self.ticket = ticket;
                self.sent.push((ticket, key.clone()));
                self.key = Some(key);
                self.status = Status::Building;
            }
            Err(error) => self.status = Status::Failed(error.to_string()),
        }
    }

    /// Prepare the board's model in the background while it is edited in
    /// 2D, once it has stayed unchanged for [`PREFETCH_DELAY`], so opening
    /// the 3D view shows it at once (or the last one, updating). Returns
    /// when to look again, if the board is still settling.
    pub fn prefetch(&mut self, key: ModelKey, now: f64) -> Option<Duration> {
        match self.prefetch.due(&key, now, self.key.as_ref()) {
            Due::Nothing => None,
            Due::Build => {
                self.request(key, None, || Ok(None));
                None
            }
            Due::Wait(wait) => Some(wait),
        }
    }

    /// Apply finished builds of the latest request, and of an earlier one
    /// of the same document and kind that is newer than the model shown.
    pub fn receive(&mut self) {
        while let Ok(built) = self.worker.receiver.try_recv() {
            #[cfg(test)]
            {
                self.received += 1;
            }
            // The request this answers, and those the worker skipped before it.
            let at = self
                .sent
                .partition_point(|(ticket, _)| *ticket <= built.ticket);
            let answered = self.sent.drain(..at).next_back();
            let current = built.ticket == self.ticket;
            // An earlier request of this document, finished while a newer
            // one is made, is newer than the model shown: it is shown, still
            // under the "Updating" note (an edit's model updated in place is
            // shown while its refilled fills are built in the background).
            let key = match answered {
                Some((ticket, key)) if ticket == built.ticket => key,
                _ => continue,
            };
            let kind = |k: &ModelKey| k.is_package();
            let earlier = !current
                && built.ticket > self.shown_ticket
                && self.key.as_ref().map(kind) == Some(kind(&key));
            if !current && !earlier {
                continue;
            }
            match built.result {
                Ok((scene, sources)) => {
                    let was = self.shown.as_ref().map(ModelKey::is_package);
                    let now = Some(kind(&key));
                    self.viewport.set_model(scene);
                    if self.fit_board && now == Some(false) {
                        if was == Some(true) {
                            self.cameras[1] = Some(self.viewport.camera());
                        }
                        self.fit_board = false;
                        self.viewport.fit();
                    } else if was != now {
                        let slot = |package: bool| usize::from(package);
                        if let Some(was) = was {
                            self.cameras[slot(was)] = Some(self.viewport.camera());
                        }
                        match now.and_then(|now| self.cameras[slot(now)]) {
                            Some(camera) => self.viewport.set_camera(camera),
                            None => self.viewport.fit(),
                        }
                    }
                    self.sources = sources;
                    self.shown = Some(key);
                    self.shown_ticket = built.ticket;
                    if current {
                        self.status = Status::Ready;
                    }
                }
                Err(error) if current => self.status = Status::Failed(error),
                // An earlier request's failure: the newer one decides.
                Err(_) => {}
            }
        }
    }

    /// Highlight every body of the selected instance, track, via or net;
    /// the selected object's in a stronger tint than its net's. `lit` (what
    /// a table row hovers) takes the net's tint. An overlay only: the model
    /// is not rebuilt.
    pub fn highlight(
        &mut self,
        chosen: &crate::pcb_editor::Group,
        net: Option<Id>,
        lit: &crate::pcb_editor::Group,
    ) {
        let mut ids = Vec::new();
        let mut selected = Vec::new();
        // What following the net keeps solid: the net's bodies and the
        // selected ones, while a net or a track or via is chosen (not what
        // a row only lights).
        let mut focus = Vec::new();
        let mut copper = net.is_some();
        for s in &self.sources {
            let picked = picks(s, chosen);
            let on_net = s.net.is_some() && s.net == net;
            if picked {
                selected.push(s.pick_id);
                copper |= s.track.is_some() || s.via.is_some();
            }
            if picked || on_net {
                focus.push(s.pick_id);
            }
            if picked || on_net || picks(s, lit) {
                ids.push(s.pick_id);
            }
        }
        self.viewport.highlight_selected(ids, selected);
        self.viewport
            .set_focus(if copper { focus } else { Vec::new() });
    }

    /// The pick identities of every body of the selected parts, tracks,
    /// vias and fills (without their nets).
    pub fn selected_items(&self, chosen: &crate::pcb_editor::Group) -> Vec<u64> {
        self.sources
            .iter()
            .filter(|s| picks(s, chosen))
            .map(|s| s.pick_id)
            .collect()
    }

    /// How many models were asked for: each one a build or an update.
    #[cfg(test)]
    pub fn requests(&self) -> u64 {
        self.ticket
    }

    /// The electronics object a pick identifies.
    pub fn source(&self, pick: u64) -> Option<&Source> {
        self.sources.iter().find(|s| s.pick_id == pick)
    }
}

/// Whether the body `s` belongs to a chosen part, track, via or fill.
pub(crate) fn picks(s: &Source, chosen: &crate::pcb_editor::Group) -> bool {
    s.instance.is_some_and(|id| chosen.parts.contains(&id))
        || s.track.is_some_and(|id| chosen.tracks.contains(&id))
        || s.via.is_some_and(|id| chosen.vias.contains(&id))
        || s.fill.is_some_and(|id| chosen.fills.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::library::{Definition, DraftEdit};
    use pcbcraft_core::{DefinitionKind, Engine, LibraryCommand, Operation, layout::LayoutCommand};

    fn demo_package() -> Result<PackageDefinition, Box<dyn std::error::Error>> {
        Ok(Engine::demo()?.project().library.packages[0]
            .definition
            .clone())
    }

    #[test]
    fn project_keys_follow_identity_so_draft_edits_keep_the_board_model()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let key = ModelKey::Project(Arc::clone(engine.project_arc()));
        let mut draft = engine
            .drafts()
            .items
            .iter()
            .find(|d| d.definition.kind() == DefinitionKind::Package)
            .cloned()
            .ok_or("demo package draft")?;
        if let Definition::Package(package) = &mut draft.definition {
            package.envelope_nm[0] =
                package.envelope_nm[0].checked_add(pcbcraft_core::units::Nm::new(1_000_000)?)?;
        }
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::Edit {
                edits: vec![DraftEdit {
                    id: draft.id,
                    meta: draft.meta,
                    definition: draft.definition,
                }],
            }),
        )?;
        assert_eq!(key, ModelKey::Project(Arc::clone(engine.project_arc())));
        let instance = engine.project().instances[0].id;
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Move {
                instance,
                position: [12.0, 12.0],
                rotation_deg: 0.0,
                layer: pcbcraft_core::Layer::Top,
            }),
        )?;
        assert_ne!(key, ModelKey::Project(Arc::clone(engine.project_arc())));
        engine.undo()?;
        assert_eq!(key, ModelKey::Project(Arc::clone(engine.project_arc())));
        Ok(())
    }

    #[test]
    fn stale_models_stay_up_only_for_their_own_kind() -> Result<(), Box<dyn std::error::Error>> {
        let project = ModelKey::Project(Arc::new(Engine::demo()?.project().clone()));
        let newer = ModelKey::Project(Arc::new(Engine::demo()?.project().clone()));
        let package = ModelKey::package(&demo_package()?).ok_or("package key")?;
        assert_eq!(display(None, &project, false), Display::Blank);
        assert_eq!(display(Some(&project), &project, true), Display::Current);
        assert_eq!(display(Some(&project), &project, false), Display::Stale);
        assert_eq!(display(Some(&project), &newer, true), Display::Stale);
        // A Part preview never shows the board, nor the board a preview.
        assert_eq!(display(Some(&project), &package, true), Display::Blank);
        assert_eq!(display(Some(&package), &project, true), Display::Blank);
        Ok(())
    }

    #[test]
    fn a_settling_board_is_prepared_once_after_it_stops_changing()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = ModelKey::Project(Arc::new(Engine::demo()?.project().clone()));
        let edited = ModelKey::Project(Arc::new(Engine::demo()?.project().clone()));
        let mut prefetch = Prefetch::default();
        let delay = PREFETCH_DELAY.as_secs_f64();
        // Each change restarts the wait; nothing is built mid-gesture.
        assert_eq!(prefetch.due(&first, 0.0, None), Due::Wait(PREFETCH_DELAY));
        assert!(matches!(
            prefetch.due(&first, delay / 2.0, None),
            Due::Wait(_)
        ));
        assert_eq!(
            prefetch.due(&edited, delay * 0.9, None),
            Due::Wait(PREFETCH_DELAY)
        );
        assert!(matches!(
            prefetch.due(&edited, delay * 1.5, None),
            Due::Wait(_)
        ));
        // Unchanged for the delay: built once, then never asked again.
        assert_eq!(prefetch.due(&edited, delay * 2.0, None), Due::Build);
        assert_eq!(
            prefetch.due(&edited, delay * 3.0, Some(&edited)),
            Due::Nothing
        );
        assert_eq!(
            prefetch.due(&edited, delay * 9.0, Some(&edited)),
            Due::Nothing
        );
        Ok(())
    }

    #[test]
    fn edited_package_previews_get_new_keys() -> Result<(), Box<dyn std::error::Error>> {
        let mut package = demo_package()?;
        let key = ModelKey::package(&package);
        package.envelope_nm[0] =
            package.envelope_nm[0].checked_add(pcbcraft_core::units::Nm::new(1_000_000)?)?;
        assert_ne!(key, ModelKey::package(&package));
        Ok(())
    }

    /// The worker keeps the board it built and makes the next board from
    /// it: an edited board's model is the edited board's.
    #[test]
    fn worker_makes_an_edited_board_from_the_last_one() -> Result<(), Box<dyn std::error::Error>> {
        let worker = Worker::start(egui::Context::default());
        let project = Engine::demo()?.project().clone();
        let mut edited = project.clone();
        let moved = edited.placements[0].instance;
        edited.placements[0].position[0] += 2.0;
        let wait = |ticket: u64| -> Result<Built, Box<dyn std::error::Error>> {
            loop {
                let built = worker
                    .receiver
                    .recv_timeout(std::time::Duration::from_secs(60))?;
                if built.ticket == ticket {
                    return Ok(built);
                }
            }
        };
        let flag = || Arc::new(AtomicBool::new(false));
        worker
            .sender
            .send(Job::Project(1, Arc::new(project), flag()))?;
        let first = wait(1)?.result?;
        worker
            .sender
            .send(Job::Project(2, Arc::new(edited.clone()), flag()))?;
        let (scene, sources) = wait(2)?.result?;
        let fresh = pcbcraft_solidcraft::project::build_flat(
            &edited,
            &SourceCache::default(),
            None,
            Flat::Meshed,
        )?;
        assert_eq!(sources.len(), first.1.len());
        assert_eq!(scene.geometry.len(), fresh.geometry.len());
        // The moved part's body is where the edited board has it.
        let pick = sources
            .iter()
            .find(|s| s.instance == Some(moved) && s.pad.is_none())
            .map(|s| s.pick_id)
            .ok_or("moved part")?;
        let min_x = |geometry: &[pcbcraft_solidcraft::scene::Geometry]| {
            geometry
                .iter()
                .filter(|g| g.item == Some(pick) && !g.kind.is_appearance())
                .map(|g| g.min[0])
                .fold(f64::MAX, f64::min)
        };
        assert!((min_x(&scene.geometry) - min_x(&fresh.geometry)).abs() < 1e-6);
        assert!((min_x(&scene.geometry) - min_x(&first.0.geometry) - 2.0).abs() < 1e-6);
        Ok(())
    }

    /// An undo needs no kept model: the worker keeps the last board model
    /// only, and the board an undo or redo returns to is made from it, as
    /// any edit's. The undone board shares every mesh with the edited one
    /// but the track the undo changed back, whose mesh is made again (no
    /// earlier model held it), and it draws the board as it was.
    #[test]
    fn an_undo_is_made_from_the_last_model_and_keeps_no_other()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = Arc::new(Engine::showcase()?.project().clone());
        let mut edited = (*project).clone();
        let track = edited.tracks.first_mut().ok_or("a track")?;
        track.width += 0.05;
        let edited = Arc::new(edited);
        let flag = || Arc::new(AtomicBool::new(false));
        let mut kept = Kept::default();
        let mut made = |board: &Arc<Project>| {
            build(Job::Project(0, Arc::clone(board), flag()), &mut kept)
                .1
                .map(|(geometry, _, _)| geometry)
        };
        // The drawn bodies' meshes, by identity.
        let meshes = |geometry: &[pcbcraft_solidcraft::scene::Geometry]| {
            geometry
                .iter()
                .filter(|g| !g.kind.is_appearance())
                .map(|g| Arc::as_ptr(&g.mesh) as usize)
                .collect::<std::collections::HashSet<_>>()
        };
        let boxes = |geometry: &[pcbcraft_solidcraft::scene::Geometry]| {
            geometry
                .iter()
                .filter(|g| !g.kind.is_appearance())
                .map(|g| format!("{:?} {:?} {:?} {:?}", g.kind, g.item, g.min, g.max))
                .collect::<Vec<_>>()
        };
        let first = made(&project)?;
        let second = made(&edited)?;
        let undone = made(&project)?;
        assert_eq!(boxes(&undone), boxes(&first), "the board as it was");
        let (first, second, undone) = (meshes(&first), meshes(&second), meshes(&undone));
        assert_eq!(
            undone.difference(&second).count(),
            1,
            "made from the edited board's model: the track's mesh alone is new"
        );
        assert!(
            undone.difference(&first).count() >= 1,
            "not an earlier model shown again"
        );
        assert!(
            kept.board.as_ref().is_some_and(
                |(board, model)| Arc::ptr_eq(board, &project) && model.native().is_none()
            ),
            "one board model kept, with no native design of the board"
        );
        Ok(())
    }

    /// The worker meshes the board's flat bodies from the board plan: the
    /// same objects are drawn and picked as from the exact model, the
    /// laminate is one body, and a track edit keeps every other mesh.
    #[test]
    fn the_worker_meshes_the_boards_flat_bodies() -> Result<(), Box<dyn std::error::Error>> {
        use pcbcraft_solidcraft::material::BodyKind;
        let project = Arc::new(Engine::showcase()?.project().clone());
        let mut edited = (*project).clone();
        let track = edited.tracks.first_mut().ok_or("a track")?;
        track.width += 0.05;
        let edited = Arc::new(edited);
        let flag = || Arc::new(AtomicBool::new(false));
        let mut kept = Kept::default();
        let made = |board: &Arc<Project>, kept: &mut Kept| {
            build(Job::Project(0, Arc::clone(board), flag()), kept)
                .1
                .map(|(geometry, sources, _)| (geometry, sources))
        };
        let copper = |kind: BodyKind| {
            matches!(
                kind,
                BodyKind::Substrate
                    | BodyKind::PadCopper
                    | BodyKind::CopperTrack
                    | BodyKind::ViaCopper
                    | BodyKind::FillCopper
            )
        };
        // What each flat body is picked as, once each.
        let picked =
            |(geometry, sources): &(Vec<pcbcraft_solidcraft::scene::Geometry>, Vec<Source>)| {
                let mut out: Vec<String> = geometry
                    .iter()
                    .filter(|g| copper(g.kind))
                    .map(|g| {
                        let source = g.item.and_then(|p| sources.iter().find(|s| s.pick_id == p));
                        format!(
                            "{:?} {}",
                            g.kind,
                            source.map_or("board", |s| s.label.as_str())
                        )
                    })
                    .collect();
                out.sort();
                out.dedup();
                out
            };
        let exact = pcbcraft_solidcraft::project::build(&project)?;
        let exact = (exact.geometry, exact.sources);
        let meshed = made(&project, &mut kept)?;
        assert_eq!(
            kept.board.as_ref().map(|(_, m)| m.flat()),
            Some(Flat::Meshed)
        );
        assert_eq!(picked(&meshed), picked(&exact));
        let laminate = |geometry: &[pcbcraft_solidcraft::scene::Geometry]| {
            geometry
                .iter()
                .filter(|g| g.kind == BodyKind::Substrate)
                .count()
        };
        assert!(laminate(&exact.0) > 1, "the exact model's slabs");
        assert_eq!(laminate(&meshed.0), 1, "the laminate drawn whole");
        // A track made wider: every flat mesh but that track's is kept.
        let meshes = |geometry: &[pcbcraft_solidcraft::scene::Geometry]| {
            geometry
                .iter()
                .filter(|g| copper(g.kind))
                .map(|g| Arc::as_ptr(&g.mesh) as usize)
                .collect::<std::collections::HashSet<_>>()
        };
        let wider = made(&edited, &mut kept)?;
        let (before, after) = (meshes(&meshed.0), meshes(&wider.0));
        assert_eq!(
            after.difference(&before).count(),
            1,
            "the wider track's mesh alone is new"
        );
        Ok(())
    }

    /// Demo run 3, item 1: an edit's model, updated in place, is shown as
    /// soon as it is made, although the board changed again meanwhile (its
    /// stale fills refilled and stored) and that board's model is still
    /// being made: under the "Updating" note, until the newer one comes.
    /// A model older than the one shown never replaces it.
    #[test]
    fn an_earlier_requests_model_is_shown_while_a_newer_one_is_made()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut scene = SceneController::new(
            egui::Context::default(),
            crate::viewport::Viewport::detached(),
        );
        // The worker's side, answered by hand.
        let (sender, jobs) = mpsc::channel::<Job>();
        let (results, receiver) = mpsc::channel();
        scene.worker = Worker { sender, receiver };
        let board = |dx: f64| -> Result<Arc<Project>, Box<dyn std::error::Error>> {
            let mut project = Engine::demo()?.project().clone();
            project.placements[0].position[0] += dx;
            Ok(Arc::new(project))
        };
        let (edited, refilled) = (board(1.0)?, board(2.0)?);
        let made = |project: &Arc<Project>| -> Result<Built, Box<dyn std::error::Error>> {
            let job = Job::Project(0, Arc::clone(project), Arc::new(AtomicBool::new(false)));
            let (_, built) = build(job, &mut Kept::default());
            let (geometry, sources, stack) = built?;
            Ok(Built {
                ticket: 0,
                result: Ok((model_scene::build(geometry, &stack)?, sources)),
            })
        };
        let key = |project: &Arc<Project>| ModelKey::Project(Arc::clone(project));
        scene.request(key(&edited), None, || Ok(None));
        scene.request(key(&refilled), None, || Ok(None));
        assert_eq!(jobs.try_iter().count(), 2);
        assert_eq!(scene.display(&key(&refilled)), Display::Blank);
        // The edit's model comes first.
        results.send(Built {
            ticket: 1,
            ..made(&edited)?
        })?;
        scene.receive();
        assert_eq!(scene.shown, Some(key(&edited)));
        assert_eq!(scene.display(&key(&refilled)), Display::Stale);
        assert!(matches!(scene.status(), Status::Building));
        // Then the refilled board's.
        results.send(Built {
            ticket: 2,
            ..made(&refilled)?
        })?;
        scene.receive();
        assert_eq!(scene.display(&key(&refilled)), Display::Current);
        // An answer to a request older than the one shown is dropped.
        scene.request(key(&edited), None, || Ok(None));
        scene.request(key(&refilled), None, || Ok(None));
        results.send(Built {
            ticket: 4,
            ..made(&refilled)?
        })?;
        results.send(Built {
            ticket: 3,
            ..made(&edited)?
        })?;
        scene.receive();
        assert_eq!(scene.display(&key(&refilled)), Display::Current);
        Ok(())
    }

    /// A build that panics fails its request, and the worker builds the
    /// next one: the view never waits forever ("Updating") on a dead worker.
    #[test]
    fn a_panicking_build_fails_and_the_worker_goes_on() -> Result<(), Box<dyn std::error::Error>> {
        let worker = Worker::start(egui::Context::default());
        worker.sender.send(Job::Panic(1))?;
        let wait = std::time::Duration::from_secs(30);
        let failed = worker.receiver.recv_timeout(wait)?;
        assert_eq!(failed.ticket, 1);
        assert!(failed.result.is_err());
        let project = Arc::new(Engine::demo()?.project().clone());
        worker
            .sender
            .send(Job::Project(2, project, Arc::new(AtomicBool::new(false))))?;
        let built = worker.receiver.recv_timeout(wait)?;
        assert_eq!(built.ticket, 2);
        assert!(built.result.is_ok());
        Ok(())
    }

    /// R7-3: boards requested while one is made are not made one by one:
    /// only the newest of them is, and the ones it replaced never answer.
    #[test]
    fn boards_queued_behind_a_build_are_skipped_but_the_newest()
    -> Result<(), Box<dyn std::error::Error>> {
        let worker = Worker::start(egui::Context::default());
        let mut board = Engine::demo()?.project().clone();
        let flag = || Arc::new(AtomicBool::new(false));
        for ticket in 1..=4u64 {
            board.placements[0].position[0] += 1.0;
            worker
                .sender
                .send(Job::Project(ticket, Arc::new(board.clone()), flag()))?;
        }
        let mut answered = Vec::new();
        while let Ok(built) = worker
            .receiver
            .recv_timeout(std::time::Duration::from_secs(60))
        {
            built.result?;
            answered.push(built.ticket);
            if built.ticket == 4 {
                break;
            }
        }
        // The first may have started before the others came.
        assert!(answered == [4] || answered == [1, 4], "{answered:?}");
        Ok(())
    }

    #[test]
    fn worker_builds_only_the_newest_request() -> Result<(), Box<dyn std::error::Error>> {
        let worker = Worker::start(egui::Context::default());
        let project = Arc::new(Engine::demo()?.project().clone());
        let mut broken = (*project).clone();
        broken.board_size[0] = 0.0;
        worker.sender.send(Job::Project(
            1,
            Arc::new(broken),
            Arc::new(AtomicBool::new(false)),
        ))?;
        worker
            .sender
            .send(Job::Package(2, Box::new(demo_package()?), None))?;
        let mut last = None;
        while let Ok(built) = worker
            .receiver
            .recv_timeout(std::time::Duration::from_secs(30))
        {
            let done = built.ticket == 2;
            last = Some(built);
            if done {
                break;
            }
        }
        let last = last.ok_or("no build finished")?;
        assert_eq!(last.ticket, 2);
        assert!(last.result.is_ok());
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod fixtures;

#[cfg(test)]
mod open_tests {
    use super::*;
    use pcbcraft_solidcraft::material::BodyKind;

    type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

    /// Ask for `key` as the app does each frame and take finished builds,
    /// until it is shown.
    fn show(scene: &mut SceneController, key: &ModelKey) -> Fallible<()> {
        for _ in 0..12_000 {
            scene.request(key.clone(), None, || Ok(None));
            scene.receive();
            if let Status::Failed(error) = scene.status() {
                return Err(error.clone().into());
            }
            if scene.display(key) == Display::Current {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err("the model was never shown".into())
    }

    /// Demo run item 7: the board camera kept belongs to the project it
    /// framed. An opened project's first model is framed; its later models
    /// (an edit) keep the camera the person left.
    #[test]
    fn an_opened_project_is_framed_and_its_edits_keep_the_camera() -> Fallible<()> {
        let mut scene = SceneController::new(
            egui::Context::default(),
            crate::viewport::Viewport::detached(),
        );
        let demo = ModelKey::Project(Arc::new(Engine::demo()?.project().clone()));
        show(&mut scene, &demo)?;
        let framed = scene.viewport.camera();
        // The person looks somewhere far off the demo board.
        let mut away = framed;
        away.target.x += 500.0;
        away.distance *= 0.1;
        scene.viewport.set_camera(away);
        scene.document_opened();
        let opened = Engine::showcase()?;
        let opened_key = ModelKey::Project(Arc::clone(opened.project_arc()));
        show(&mut scene, &opened_key)?;
        let fitted = scene.viewport.camera();
        assert_ne!(fitted, away, "the opened board was not framed");
        // An edit of the opened board keeps the camera as it is.
        let mut edited = opened.project().clone();
        edited.placements[0].position[0] += 1.0;
        let mut kept = fitted;
        kept.yaw += 0.3;
        scene.viewport.set_camera(kept);
        show(&mut scene, &ModelKey::Project(Arc::new(edited)))?;
        assert_eq!(scene.viewport.camera(), kept);
        Ok(())
    }

    /// Demo run 3, item 2: another project opened while the previous
    /// project's model is still being made never shows that model. The
    /// view shows the building state until the opened project's own model
    /// arrives, and the previous project's result is dropped.
    #[test]
    fn an_opened_project_never_shows_the_previous_projects_model() -> Fallible<()> {
        let mut scene = SceneController::new(
            egui::Context::default(),
            crate::viewport::Viewport::detached(),
        );
        let demo = Engine::demo()?;
        show(
            &mut scene,
            &ModelKey::Project(Arc::clone(demo.project_arc())),
        )?;
        // An edit of the previous project: its model is in the making.
        let mut edited = demo.project().clone();
        edited.placements[0].position[0] += 1.0;
        let edited = ModelKey::Project(Arc::new(edited));
        scene.request(edited.clone(), None, || Ok(None));
        assert_eq!(scene.display(&edited), Display::Stale);
        scene.document_opened();
        let opened = Engine::showcase()?;
        let key = ModelKey::Project(Arc::clone(opened.project_arc()));
        // Before the opened project's model is even asked for, nothing of
        // the previous project is shown, also once its model arrives.
        let received = scene.received;
        for _ in 0..12_000 {
            scene.receive();
            assert_eq!(scene.display(&key), Display::Blank);
            assert_eq!(scene.display(&edited), Display::Blank);
            if scene.received > received {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(scene.received > received, "the edit's model never came");
        assert!(scene.shown.is_none(), "the previous project's model");
        assert!(scene.sources.is_empty(), "the previous project's picks");
        for _ in 0..12_000 {
            scene.request(key.clone(), None, || Ok(None));
            scene.receive();
            match scene.display(&key) {
                Display::Current => break,
                shown => assert_eq!(shown, Display::Blank, "another project's model"),
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(scene.display(&key), Display::Current);
        // What is shown is the opened board's model, and picks name its
        // objects.
        let fresh = pcbcraft_solidcraft::project::build_flat(
            opened.project(),
            &SourceCache::default(),
            None,
            Flat::Meshed,
        )?;
        let model = scene.viewport.shown_model().ok_or("no model")?;
        assert_eq!(scene.sources.len(), fresh.sources.len());
        assert_eq!(
            model
                .geometry
                .iter()
                .filter(|g| !g.kind.is_appearance())
                .count(),
            fresh
                .geometry
                .iter()
                .filter(|g| !g.kind.is_appearance())
                .count()
        );
        Ok(())
    }

    fn component_body(kind: BodyKind) -> bool {
        matches!(
            kind,
            BodyKind::ModelBody
                | BodyKind::PackageBody
                | BodyKind::ChipResistor
                | BodyKind::ChipCapacitor
        )
    }

    /// Opening a file and entering PCB 3D, as the app does it: the demo
    /// board was shown before, the opened board is built on the worker
    /// (from the demo board's model) and published, and the renderer takes
    /// the published scene. Without any edit, every placed part's body is
    /// in the model and drawn: the WROOM-3 module's board, can and antenna
    /// and every showcase part.
    #[test]
    fn an_opened_board_shows_its_component_bodies_without_an_edit() -> Fallible<()> {
        let boards = [
            ("WROOM-3", super::fixtures::wroom_board()?),
            ("showcase", Engine::showcase()?),
        ];
        for (name, engine) in boards {
            let mut scene = SceneController::new(
                egui::Context::default(),
                crate::viewport::Viewport::detached(),
            );
            let demo = ModelKey::Project(Arc::new(Engine::demo()?.project().clone()));
            show(&mut scene, &demo)?;
            scene.viewport.take_published();
            let key = ModelKey::Project(Arc::clone(engine.project_arc()));
            show(&mut scene, &key)?;
            let published = scene
                .viewport
                .take_published()
                .ok_or("the opened board was never published")?;
            let model = scene.viewport.shown_model().ok_or("no model")?;
            let drawn: std::collections::HashSet<u64> =
                published.instanced.iter().map(|m| m.key).collect();
            let project = engine.project();
            assert!(!project.placements.is_empty(), "{name}");
            for placement in &project.placements {
                let picks: Vec<u64> = scene
                    .sources
                    .iter()
                    .filter(|s| s.instance == Some(placement.instance) && s.pad.is_none())
                    .map(|s| s.pick_id)
                    .collect();
                let bodies: Vec<usize> = model
                    .geometry
                    .iter()
                    .enumerate()
                    .filter(|(_, g)| {
                        component_body(g.kind) && g.item.is_some_and(|i| picks.contains(&i))
                    })
                    .map(|(i, _)| i)
                    .collect();
                assert!(
                    !bodies.is_empty(),
                    "{name}: instance {} has a body",
                    placement.instance
                );
                for body in bodies {
                    let group = model
                        .instanced
                        .iter()
                        .find(|g| g.bodies.contains(&body))
                        .ok_or("a placed body is drawn instanced")?;
                    assert!(
                        drawn.contains(&group.mesh.key),
                        "{name}: the {:?} of instance {} is in the published scene",
                        model.geometry[body].kind,
                        placement.instance
                    );
                }
            }
            if name == "WROOM-3" {
                let can = model
                    .geometry
                    .iter()
                    .filter(|g| g.kind == BodyKind::ModelBody)
                    .count();
                assert_eq!(can, 3, "the module's board, can and antenna");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod coplanar_tests;

#[cfg(test)]
mod edit_timing_tests;

#[cfg(test)]
mod memory_tests;
