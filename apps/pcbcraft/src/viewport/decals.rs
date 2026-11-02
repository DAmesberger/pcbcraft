//! Component 3D marking decals (D7): made off the UI thread from the shown
//! model's cached bodies, drawn as instanced PBR meshes with the model
//! (through its display transform, in the Markings layer), and replaced
//! when the marking changes. The model is never rebuilt for a marking
//! edit: only the decal meshes are made again, keyed by what draws.
use super::Viewport;
use crate::model_scene::{self, Instanced};
use pcbcraft_solidcraft::scene::{Geometry, Place};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// What makes a model's decals from its bodies: the decal geometry, and
/// why any decal was left out. Pure: run on a worker thread.
pub type DecalBuild = Arc<dyn Fn(&[Geometry]) -> (Vec<Geometry>, Vec<String>) + Send + Sync>;

/// A marking to show: its identity (what draws) and how it is made.
#[derive(Clone)]
pub struct DecalRequest {
    pub key: u64,
    pub build: DecalBuild,
}

/// A decal build running off the UI thread: the request and model
/// generation it is for.
struct Job {
    key: u64,
    generation: u64,
    result: Receiver<(Vec<Geometry>, Vec<String>)>,
}

/// The decals of the shown model.
#[derive(Default)]
pub(super) struct Decals {
    request: Option<DecalRequest>,
    job: Option<Job>,
    /// The decal bodies drawn, each at the identity placement so they are
    /// drawn instanced and follow the display transform.
    pub(super) geometry: Vec<Geometry>,
    pub(super) meshes: Vec<Instanced>,
    /// Which request and model generation the drawn decals are of.
    shown: Option<(u64, u64)>,
    problems: Vec<String>,
    /// Decal builds started, for evidence that a marking edit rebuilds
    /// only its decals.
    pub(super) builds: u64,
}

impl Viewport {
    /// Show `request`'s decals on the model, or none. A request already
    /// shown or building is not made again.
    pub fn set_decals(&mut self, request: Option<DecalRequest>) {
        match request {
            None => {
                let had = !self.decals.meshes.is_empty();
                self.decals = Decals {
                    builds: self.decals.builds,
                    ..Decals::default()
                };
                if had {
                    self.publish_instanced();
                }
            }
            Some(request) => {
                if self.decals.request.as_ref().map(|r| r.key) == Some(request.key) {
                    return;
                }
                self.decals.request = Some(request);
                self.start_decals();
            }
        }
    }

    /// Build the requested decals on the shown model off the UI thread.
    pub(super) fn start_decals(&mut self) {
        let (Some(request), Some(model)) = (&self.decals.request, &self.model) else {
            return;
        };
        let wanted = (request.key, self.generation);
        if self.decals.shown == Some(wanted)
            || self
                .decals
                .job
                .as_ref()
                .is_some_and(|j| (j.key, j.generation) == wanted)
        {
            return;
        }
        // The bodies share their meshes: a cheap copy for the worker.
        let geometry = model.geometry.clone();
        let build = Arc::clone(&request.build);
        let (send, result) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("pcb-3d-decals".into())
            .spawn(move || {
                let _ = send.send(build(&geometry));
            });
        if spawned.is_ok() {
            self.decals.builds = self.decals.builds.saturating_add(1);
            self.decals.job = Some(Job {
                key: wanted.0,
                generation: wanted.1,
                result,
            });
        }
    }

    /// Take finished decals if they are still current; whether a build is
    /// still running.
    pub(super) fn receive_decals(&mut self) -> bool {
        let Some(job) = &self.decals.job else {
            return false;
        };
        let current = self.decals.request.as_ref().map(|r| r.key) == Some(job.key)
            && job.generation == self.generation;
        match job.result.try_recv() {
            Ok((geometry, problems)) if current => {
                let shown = (job.key, job.generation);
                self.decals.job = None;
                self.show_decals(geometry, problems, shown);
                false
            }
            Ok(_) | Err(TryRecvError::Disconnected) => {
                self.decals.job = None;
                // A stale result: build the current request instead.
                self.start_decals();
                self.decals.job.is_some()
            }
            Err(TryRecvError::Empty) => true,
        }
    }

    fn show_decals(&mut self, geometry: Vec<Geometry>, problems: Vec<String>, shown: (u64, u64)) {
        let placed: Vec<Geometry> = geometry
            .into_iter()
            .map(|g| Geometry {
                place: Some(Place::IDENTITY),
                ..g
            })
            .collect();
        // Decal meshes are small; a failure to group them draws none.
        self.decals.meshes = model_scene::instance_bodies(&placed).unwrap_or_default();
        self.decals.geometry = placed;
        self.decals.problems = problems;
        self.decals.shown = Some(shown);
        self.publish_instanced();
    }

    /// The key of the decals asked for, if any.
    pub fn decals_key(&self) -> Option<u64> {
        self.decals.request.as_ref().map(|r| r.key)
    }

    /// Why decals of the shown marking were left out.
    #[cfg(test)]
    pub fn decal_problems(&self) -> &[String] {
        &self.decals.problems
    }

    /// Whether decals are being made.
    #[cfg(test)]
    pub fn decals_building(&self) -> bool {
        self.decals.job.is_some()
    }

    /// The model was replaced: its decals are made again on it.
    pub(super) fn model_changed_decals(&mut self) {
        if !self.decals.meshes.is_empty() {
            self.decals.meshes.clear();
            self.decals.geometry.clear();
            self.decals.shown = None;
        }
        self.start_decals();
    }

    /// Republish the instanced meshes (the model's and the decals') over
    /// the published scene, without repacking its vertex stream.
    pub(super) fn publish_instanced(&mut self) {
        let instanced = self.instanced();
        if let Ok(mut slot) = self.slot.lock() {
            match slot.as_mut() {
                Some(scene) => scene.instanced = instanced,
                None => {
                    *slot = Some(solidcraft_ui_egui::gpu::GpuScene {
                        instanced,
                        ..Default::default()
                    });
                }
            }
            self.key = model_scene::next_key();
        }
    }

    /// The decal meshes as drawn now, for tests.
    #[cfg(test)]
    pub(crate) fn decal_geometry(&self) -> &[Geometry] {
        &self.decals.geometry
    }

    /// Decal builds started so far, for tests.
    #[cfg(test)]
    pub(crate) fn decal_builds(&self) -> u64 {
        self.decals.builds
    }
}
