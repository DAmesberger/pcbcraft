//! The 3D model pipeline: builds native Solidcraft models off the UI thread,
//! keeps only results for the current input, and maps picks back to
//! electronics objects. Selection and highlight recolour cached meshes; only a
//! changed project or package preview rebuilds geometry. The worker also
//! packs the GPU vertices ([`crate::model_scene`]), so the UI thread never
//! walks a mesh. It keeps the last board model it made, and a board edited
//! only in values (a part, track or via moved, a size changed) updates it
//! in place ([`pcbcraft_solidcraft::project::update`]) instead of building
//! the board again. A board's instance marking decals are made there too
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
use pcbcraft_solidcraft::project::Source;
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
    let PackageModel::Library(use_) = &package.model else {
        return Ok(None);
    };
    let library = &engine.project().library;
    let model = match library.model(use_.model) {
        Some(published) => published.definition.clone(),
        None => engine
            .drafts()
            .get(use_.model.id)
            .filter(|d| d.pending() == Some(use_.model.revision))
            .and_then(|d| match &d.definition {
                Definition::Model(model) => Some(model.as_ref().clone()),
                _ => None,
            })
            .ok_or_else(|| format!("The model {} is not in the library", use_.model))?,
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
}

/// A built job's display geometry and pick sources.
type BuiltGeometry = Result<(Vec<pcbcraft_solidcraft::scene::Geometry>, Vec<Source>), String>;

/// What the worker keeps between builds.
#[derive(Default)]
struct Kept {
    /// The last board model, for the next board to update in place.
    board: Option<pcbcraft_solidcraft::project::NativeModel>,
    markings: BoardMarkings,
    sources: SourceCache,
}

/// Build one job's display geometry and pick sources.
fn build(job: Job, kept: &mut Kept) -> (u64, BuiltGeometry) {
    match job {
        Job::Project(ticket, project, cancel) => {
            let made = match kept.board.take() {
                Some(previous) => pcbcraft_solidcraft::project::update(
                    previous,
                    &project,
                    &kept.sources,
                    Some(cancel),
                )
                .map(|(model, _)| model),
                None => {
                    pcbcraft_solidcraft::project::build_with(&project, &kept.sources, Some(cancel))
                }
            };
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
                kept.board = Some(model);
                (geometry, sources)
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
                    .map(|geometry| (geometry, Vec::new()))
                }
                _ => pcbcraft_solidcraft::project::preview(&package)
                    .map(|model| (model.geometry, model.sources)),
            };
            (ticket, built.map_err(|e| e.to_string()))
        }
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
            while let Ok(mut job) = jobs.recv() {
                while let Ok(newer) = jobs.try_recv() {
                    job = newer;
                }
                let (ticket, result) = build(job, &mut kept);
                let result = result.and_then(|(geometry, sources)| {
                    model_scene::build(geometry).map(|scene| (scene, sources))
                });
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
    shown: Option<ModelKey>,
    sources: Vec<Source>,
    status: Status,
    /// The camera each kind of model (board, package) was last seen with,
    /// so switching workspaces keeps each view's framing.
    cameras: [Option<pcbcraft_solidcraft::render::Camera>; 2],
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
            shown: None,
            sources: vec![],
            status: Status::Idle,
            cameras: [None; 2],
        }
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

    /// Apply finished builds that still match the latest request.
    pub fn receive(&mut self) {
        while let Ok(built) = self.worker.receiver.try_recv() {
            if built.ticket != self.ticket {
                continue;
            }
            match built.result {
                Ok((scene, sources)) => {
                    let was = self.shown.as_ref().map(ModelKey::is_package);
                    let now = self.key.as_ref().map(ModelKey::is_package);
                    self.viewport.set_model(scene);
                    if was != now {
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
                    self.shown = self.key.clone();
                    self.status = Status::Ready;
                }
                Err(error) => self.status = Status::Failed(error),
            }
        }
    }

    /// Highlight every body of the selected instance, track, via or net;
    /// the selected object's in a stronger tint than its net's.
    pub fn highlight(
        &mut self,
        instance: Option<Id>,
        track: Option<Id>,
        via: Option<Id>,
        net: Option<Id>,
    ) {
        let mut ids = Vec::new();
        let mut selected = Vec::new();
        for s in &self.sources {
            let picked = s.instance.is_some() && s.instance == instance
                || s.track.is_some() && s.track == track
                || s.via.is_some() && s.via == via;
            if picked {
                selected.push(s.pick_id);
            }
            if picked || s.net.is_some() && s.net == net {
                ids.push(s.pick_id);
            }
        }
        self.viewport.highlight_selected(ids, selected);
    }

    /// The pick identities of every body of the selected instance, track
    /// or via (without its net).
    pub fn selected_items(
        &self,
        instance: Option<Id>,
        track: Option<Id>,
        via: Option<Id>,
    ) -> Vec<u64> {
        self.sources
            .iter()
            .filter(|s| {
                s.instance.is_some() && s.instance == instance
                    || s.track.is_some() && s.track == track
                    || s.via.is_some() && s.via == via
            })
            .map(|s| s.pick_id)
            .collect()
    }

    /// The electronics object a pick identifies.
    pub fn source(&self, pick: u64) -> Option<&Source> {
        self.sources.iter().find(|s| s.pick_id == pick)
    }
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
            package.envelope[0] += 1.0;
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
        package.envelope[0] += 1.0;
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
        let fresh = pcbcraft_solidcraft::project::build(&edited)?;
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
