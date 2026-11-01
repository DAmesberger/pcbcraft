//! Copper fills refill off the UI thread. After every revision the worker
//! gets the project (a shared handle, no copy) and computes the fills whose
//! inputs changed (`layout::fill::compute_stale`): on open, after an edit,
//! after undo. The results go back with the revision they belong to and are
//! stored without an undo step (`Engine::store_fills`); a result for an
//! older revision is dropped, and the newer revision is already queued.
//! The worker keeps a cache of recent results, so undo and redo reuse
//! earlier copper instead of recomputing it.
use pcbcraft_core::layout::fill::{FillCache, compute_stale};
use pcbcraft_core::model::Project;
use pcbcraft_core::model::fill::FilledCopper;
use pcbcraft_core::{Engine, Id};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

type Computed = Result<Vec<(Id, FilledCopper)>, String>;

pub struct FillWorker {
    jobs: Option<Sender<(u64, Arc<Project>)>>,
    results: Option<Receiver<(u64, Computed)>>,
    /// The last revision sent to the worker.
    sent: Option<u64>,
    /// The revision whose failure was last reported, so it is said once.
    reported: Option<u64>,
}

impl FillWorker {
    pub fn start(ctx: egui::Context) -> Self {
        let (jobs, inbox) = mpsc::channel::<(u64, Arc<Project>)>();
        let (outbox, results) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("pcbcraft-fills".into())
            .spawn(move || {
                let mut cache = FillCache::default();
                while let Ok(mut job) = inbox.recv() {
                    while let Ok(newer) = inbox.try_recv() {
                        job = newer;
                    }
                    let (revision, project) = job;
                    let computed = compute_stale(&project, &mut cache).map_err(|e| e.to_string());
                    if outbox.send((revision, computed)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        let (jobs, results) = match spawned {
            Ok(_) => (Some(jobs), Some(results)),
            // Without a thread fills stay as stored; `fill.refill` still works.
            Err(_) => (None, None),
        };
        Self {
            jobs,
            results,
            sent: None,
            reported: None,
        }
    }

    /// Store finished results and queue the current revision. Returns how
    /// many fills were stored, or a failure to report once.
    pub fn poll(&mut self, engine: &mut Engine) -> Result<usize, String> {
        let mut stored = 0;
        let mut failure = None;
        if let Some(results) = &self.results {
            loop {
                match results.try_recv() {
                    Ok((revision, Ok(copper))) => {
                        if revision == engine.revision() && !copper.is_empty() {
                            stored += engine
                                .store_fills(revision, copper)
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    Ok((revision, Err(error))) => {
                        if self.reported != Some(revision) && revision == engine.revision() {
                            self.reported = Some(revision);
                            failure = Some(format!("Fills could not be computed: {error}"));
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.results = None;
                        break;
                    }
                }
            }
        }
        let revision = engine.revision();
        if self.sent != Some(revision) {
            self.sent = Some(revision);
            if !engine.project().fills.is_empty()
                && let Some(jobs) = &self.jobs
                && jobs.send((revision, engine.project_arc().clone())).is_err()
            {
                self.jobs = None;
            }
        }
        match failure {
            Some(text) => Err(text),
            None => Ok(stored),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::layout::fill::{FillCommand, RefillTarget};
    use pcbcraft_core::library::{Outline, Segment};
    use pcbcraft_core::model::CopperLayer;
    use pcbcraft_core::model::fill::FillSpec;
    use pcbcraft_core::{Operation, Outcome};

    #[test]
    fn stale_fills_are_refilled_in_the_background_without_an_undo_step()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::showcase()?;
        let gnd = engine
            .project()
            .nets
            .iter()
            .find(|n| n.name == "GND")
            .map(|n| n.id)
            .ok_or("gnd")?;
        let outline = Outline {
            start: [0.0, 0.0],
            segments: vec![
                Segment::Line { to: [44.0, 0.0] },
                Segment::Line { to: [44.0, 28.0] },
                Segment::Line { to: [0.0, 28.0] },
                Segment::Line { to: [0.0, 0.0] },
            ],
        };
        let Outcome::FillAdded { fill } = engine.apply(
            engine.revision(),
            Operation::Fill(FillCommand::Add {
                fill: Box::new(FillSpec {
                    name: "GND".into(),
                    net: Some(gnd),
                    layer: CopperLayer::Bottom,
                    outline,
                    priority: 0,
                    settings: Default::default(),
                }),
            }),
        )?
        else {
            return Err("no fill".into());
        };
        let mut worker = FillWorker::start(egui::Context::default());
        let added = engine.revision();
        let mut stored = 0;
        for _ in 0..600 {
            stored += worker.poll(&mut engine)?;
            if stored > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(stored, 1);
        assert_eq!(engine.revision(), added + 1);
        let computed = engine
            .project()
            .fill(fill)
            .and_then(|f| f.computed.clone())
            .ok_or("not stored")?;
        // The same copper `fill.refill` stores.
        // The showcase has pours of its own, so the added fill is looked
        // up by its id.
        let fresh = pcbcraft_core::layout::fill::refill(engine.project(), RefillTarget::All)?;
        let direct = fresh
            .into_iter()
            .find(|(id, _)| *id == fill)
            .map(|(_, c)| c);
        assert_eq!(direct.as_ref(), Some(&computed));
        // Undo goes back over the fill add, not the refill.
        engine.undo()?;
        assert!(engine.project().fill(fill).is_none());
        Ok(())
    }
}
