//! The 3D model pipeline: builds native Solidcraft models off the UI thread,
//! keeps only results for the current input, and maps picks back to
//! electronics objects. Selection and highlight recolour cached meshes; only a
//! changed project or component preview rebuilds geometry.
use crate::palette;
use crate::viewport::Viewport;
use pcbcraft_core::{Id, Part, Project};
use pcbcraft_solidcraft::{project::Source, scene::Geometry};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

/// What a model was built from. Projects compare by shared identity, so a
/// draft-only edit never rebuilds the board model.
#[derive(Clone, Debug)]
pub enum ModelKey {
    Project(Arc<Project>),
    Part(Vec<u8>),
}

impl PartialEq for ModelKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Project(a), Self::Project(b)) => Arc::ptr_eq(a, b),
            (Self::Part(a), Self::Part(b)) => a == b,
            _ => false,
        }
    }
}

impl ModelKey {
    pub fn part(part: &Part) -> Option<Self> {
        serde_json::to_vec(part).ok().map(Self::Part)
    }
    fn is_part(&self) -> bool {
        matches!(self, Self::Part(_))
    }
}

enum Job {
    Project(u64, Arc<Project>),
    Part(u64, Box<Part>),
}

struct Built {
    ticket: u64,
    result: Result<(Vec<Geometry>, Vec<Source>), String>,
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
                    Job::Part(ticket, part) => {
                        (ticket, pcbcraft_solidcraft::project::preview(&part))
                    }
                };
                let result = result
                    .map(|model| (model.geometry, model.sources))
                    .map_err(|e| e.to_string());
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

    /// Whether the viewport shows exactly `key`.
    pub fn showing(&self, key: &ModelKey) -> bool {
        self.shown.as_ref() == Some(key) && matches!(self.status, Status::Ready)
    }

    /// Ask for a model of `key` unless it is already requested.
    pub fn request(&mut self, key: ModelKey, part: Option<&Part>) {
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let Some(ticket) = self.ticket.checked_add(1) else {
            self.status = Status::Failed("model request identity exhausted".into());
            return;
        };
        let job = match (&key, part) {
            (ModelKey::Project(project), _) => Job::Project(ticket, Arc::clone(project)),
            (ModelKey::Part(_), Some(part)) => Job::Part(ticket, Box::new(part.clone())),
            (ModelKey::Part(_), None) => return,
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
                Ok((geometry, sources)) => {
                    let part = self.key.as_ref().is_some_and(ModelKey::is_part);
                    let colors = model_colors(&geometry, &sources, part);
                    let kind_changed = self.shown.as_ref().map(ModelKey::is_part)
                        != self.key.as_ref().map(ModelKey::is_part);
                    self.viewport.set_geometry(geometry, colors);
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

fn model_colors(geometry: &[Geometry], sources: &[Source], part: bool) -> Vec<[u8; 4]> {
    let colors = palette::board();
    geometry
        .iter()
        .map(|g| {
            let source = g
                .item
                .and_then(|id| sources.iter().find(|s| s.pick_id == id));
            let color = match source {
                Some(s) if s.net.is_some() => {
                    colors.copper(s.layer.unwrap_or(pcbcraft_core::Layer::Top))
                }
                Some(_) => colors.package,
                None if part => colors.part_body,
                None => colors.substrate,
            };
            color.to_array()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::{Engine, Operation, layout::LayoutCommand};

    #[test]
    fn project_keys_follow_identity_so_draft_edits_keep_the_board_model()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let key = ModelKey::Project(Arc::clone(engine.project_arc()));
        let draft = pcbcraft_core::parts::passive_template("C");
        engine.apply(
            engine.revision(),
            Operation::Part(pcbcraft_core::parts::PartCommand::SetDraft { draft }),
        )?;
        assert_eq!(key, ModelKey::Project(Arc::clone(engine.project_arc())));
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Move {
                instance: 2,
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
    fn edited_component_previews_get_new_keys() {
        let mut part = pcbcraft_core::parts::passive_template("R");
        let key = ModelKey::part(&part);
        part.package.body_size[0] += 1.0;
        assert_ne!(key, ModelKey::part(&part));
    }

    #[test]
    fn worker_builds_only_the_newest_request() -> Result<(), Box<dyn std::error::Error>> {
        let worker = Worker::start(egui::Context::default());
        let project = Arc::new(Engine::demo()?.project().clone());
        let mut broken = (*project).clone();
        broken.board_size[0] = 0.0;
        worker.sender.send(Job::Project(1, Arc::new(broken)))?;
        worker.sender.send(Job::Part(
            2,
            Box::new(pcbcraft_core::parts::passive_template("P")),
        ))?;
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
