//! The 3D model pipeline: builds native Solidcraft models off the UI thread,
//! keeps only results for the current input, and maps picks back to
//! electronics objects. Selection and highlight recolour cached meshes; only a
//! changed project or package preview rebuilds geometry. The worker also
//! packs the GPU vertices ([`crate::model_scene`]), so the UI thread never
//! walks a mesh.
use crate::model_scene::{self, ModelScene};
use crate::viewport::Viewport;
use pcbcraft_core::library::PackageDefinition;
use pcbcraft_core::{Id, Project};
use pcbcraft_solidcraft::project::Source;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

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

enum Job {
    Project(u64, Arc<Project>),
    Package(u64, Box<PackageDefinition>),
}

struct Built {
    ticket: u64,
    result: Result<(ModelScene, Vec<Source>), String>,
}

/// One background thread building native models. Only the newest queued
/// request is built; older ones are skipped.
struct Worker {
    sender: Sender<Job>,
    receiver: Receiver<Built>,
}

impl Worker {
    fn start(ctx: egui::Context) -> Self {
        let (sender, jobs) = mpsc::channel::<Job>();
        let (results, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(mut job) = jobs.recv() {
                while let Ok(newer) = jobs.try_recv() {
                    job = newer;
                }
                let (ticket, result) = match job {
                    Job::Project(ticket, project) => {
                        (ticket, pcbcraft_solidcraft::project::build(&project))
                    }
                    Job::Package(ticket, package) => {
                        (ticket, pcbcraft_solidcraft::project::preview(&package))
                    }
                };
                let result = result.map_err(|e| e.to_string()).and_then(|model| {
                    model_scene::build(model.geometry).map(|scene| (scene, model.sources))
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
#[derive(Debug, PartialEq, Eq)]
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

pub struct SceneController {
    worker: Worker,
    pub viewport: Viewport,
    ticket: u64,
    key: Option<ModelKey>,
    shown: Option<ModelKey>,
    sources: Vec<Source>,
    status: Status,
}

impl SceneController {
    pub fn new(ctx: egui::Context, viewport: Viewport) -> Self {
        Self {
            worker: Worker::start(ctx),
            viewport,
            ticket: 0,
            key: None,
            shown: None,
            sources: vec![],
            status: Status::Idle,
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

    /// Ask for a model of `key` unless it is already requested.
    pub fn request(&mut self, key: ModelKey, package: Option<&PackageDefinition>) {
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let Some(ticket) = self.ticket.checked_add(1) else {
            self.status = Status::Failed("model request identity exhausted".into());
            return;
        };
        let job = match (&key, package) {
            (ModelKey::Project(project), _) => Job::Project(ticket, Arc::clone(project)),
            (ModelKey::Package(_), Some(package)) => {
                Job::Package(ticket, Box::new(package.clone()))
            }
            (ModelKey::Package(_), None) => return,
        };
        match self.worker.sender.send(job) {
            Ok(()) => {
                self.ticket = ticket;
                self.key = Some(key);
                self.status = Status::Building;
            }
            Err(error) => self.status = Status::Failed(error.to_string()),
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
                    let kind_changed = self.shown.as_ref().map(ModelKey::is_package)
                        != self.key.as_ref().map(ModelKey::is_package);
                    self.viewport.set_model(scene);
                    if kind_changed {
                        self.viewport.fit();
                    }
                    self.sources = sources;
                    self.shown = self.key.clone();
                    self.status = Status::Ready;
                }
                Err(error) => self.status = Status::Failed(error),
            }
        }
    }

    /// Highlight every body of the selected instance, track or net.
    pub fn highlight(&mut self, instance: Option<Id>, track: Option<Id>, net: Option<Id>) {
        let ids = self
            .sources
            .iter()
            .filter(|s| {
                s.instance.is_some() && s.instance == instance
                    || s.track.is_some() && s.track == track
                    || s.net.is_some() && s.net == net
            })
            .map(|s| s.pick_id)
            .collect();
        self.viewport.highlight(ids);
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
            package.body_size[0] += 1.0;
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
    fn edited_package_previews_get_new_keys() -> Result<(), Box<dyn std::error::Error>> {
        let mut package = demo_package()?;
        let key = ModelKey::package(&package);
        package.body_size[0] += 1.0;
        assert_ne!(key, ModelKey::package(&package));
        Ok(())
    }

    #[test]
    fn worker_builds_only_the_newest_request() -> Result<(), Box<dyn std::error::Error>> {
        let worker = Worker::start(egui::Context::default());
        let project = Arc::new(Engine::demo()?.project().clone());
        let mut broken = (*project).clone();
        broken.board_size[0] = 0.0;
        worker.sender.send(Job::Project(1, Arc::new(broken)))?;
        worker
            .sender
            .send(Job::Package(2, Box::new(demo_package()?)))?;
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
