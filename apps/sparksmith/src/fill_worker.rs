//! Copper fills refill off the UI thread. After every revision the worker
//! gets the project (a shared handle, no copy) and computes the fills whose
//! inputs changed (`layout::fill::compute_stale`): on open, after an edit,
//! after undo. The results go back with the revision they belong to and are
//! stored without an undo step (`Engine::store_fills`); a result for an
//! older revision is dropped, and the newer revision is already queued.
//! The worker keeps a cache of recent results, so undo and redo reuse
//! earlier copper instead of recomputing it.
//!
//! An explicit refill (the Refill buttons, `fill.refill`) is computed here
//! too ([`FillWorker::refill`]): its copper comes back for the app to
//! commit as one undoable `fill.store_all`. A refill whose revision moved
//! on while it ran is computed again on the current one.
use sparksmith_core::layout::fill::{FillCache, RefillTarget, compute_stale, refill};
use sparksmith_core::model::Project;
use sparksmith_core::model::fill::FilledCopper;
use sparksmith_core::{Engine, Id};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

type Computed = Result<Vec<(Id, FilledCopper)>, String>;

enum Job {
    Stale(u64, Arc<Project>),
    Refill(u64, Arc<Project>, RefillTarget),
}

enum Done {
    Stale(u64, Computed),
    Refill(u64, RefillTarget, Computed),
}

/// What one poll brought back.
#[derive(Debug, Default)]
pub struct Polled {
    /// Stale fills stored without an undo step.
    pub stored: usize,
    /// An explicit refill's copper for the current revision, to commit as
    /// one `fill.store_all`.
    pub refilled: Option<Vec<(Id, FilledCopper)>>,
}

pub struct FillWorker {
    jobs: Option<Sender<Job>>,
    results: Option<Receiver<Done>>,
    /// The last revision sent to the worker.
    sent: Option<u64>,
    /// The revision whose failure was last reported, so it is said once.
    reported: Option<u64>,
    /// An explicit refill is running (its target).
    refilling: Option<RefillTarget>,
}

impl FillWorker {
    pub fn start(ctx: egui::Context) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, results) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("sparksmith-fills".into())
            .spawn(sparksmith_core::library::standards::carried(move || {
                let mut cache = FillCache::default();
                while let Ok(first) = inbox.recv() {
                    // Only the newest stale job matters; refills all run.
                    let mut stale = None;
                    let mut refills = Vec::new();
                    for job in std::iter::once(first).chain(inbox.try_iter()) {
                        match job {
                            Job::Stale(revision, project) => stale = Some((revision, project)),
                            Job::Refill(revision, project, target) => {
                                refills.push((revision, project, target));
                            }
                        }
                    }
                    let mut done = Vec::new();
                    for (revision, project, target) in refills {
                        let computed = refill(&project, target).map_err(|e| e.to_string());
                        done.push(Done::Refill(revision, target, computed));
                    }
                    if let Some((revision, project)) = stale {
                        let computed =
                            compute_stale(&project, &mut cache).map_err(|e| e.to_string());
                        done.push(Done::Stale(revision, computed));
                    }
                    for d in done {
                        if outbox.send(d).is_err() {
                            return;
                        }
                    }
                    ctx.request_repaint();
                }
            }));
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
            refilling: None,
        }
    }

    /// Whether explicit refills run here (the worker thread is up).
    pub fn available(&self) -> bool {
        self.jobs.is_some()
    }

    /// An explicit refill is running.
    pub fn refilling(&self) -> bool {
        self.refilling.is_some()
    }

    /// Compute `target`'s copper on `engine`'s current revision off the UI
    /// thread; [`Self::poll`] hands it back. Returns `false` when the
    /// worker is not running (the caller then applies `fill.refill`).
    pub fn refill(&mut self, engine: &Engine, target: RefillTarget) -> bool {
        let sent = self.jobs.as_ref().is_some_and(|jobs| {
            jobs.send(Job::Refill(
                engine.revision(),
                engine.project_arc().clone(),
                target,
            ))
            .is_ok()
        });
        if sent {
            self.refilling = Some(target);
        } else {
            self.jobs = None;
        }
        sent
    }

    /// Store finished stale results and queue the current revision; hand
    /// back a finished explicit refill. Fails with a message to report
    /// once.
    pub fn poll(&mut self, engine: &mut Engine) -> Result<Polled, String> {
        let mut polled = Polled::default();
        let mut failure = None;
        let mut retry = None;
        if let Some(results) = &self.results {
            loop {
                match results.try_recv() {
                    Ok(Done::Stale(revision, Ok(copper))) => {
                        if revision == engine.revision() && !copper.is_empty() {
                            polled.stored += engine
                                .store_fills(revision, copper)
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    Ok(Done::Stale(revision, Err(error))) => {
                        if self.reported != Some(revision) && revision == engine.revision() {
                            self.reported = Some(revision);
                            failure = Some(format!("Fills could not be computed: {error}"));
                        }
                    }
                    Ok(Done::Refill(revision, target, computed)) => {
                        self.refilling = None;
                        match computed {
                            Ok(copper) if revision == engine.revision() => {
                                polled.refilled = Some(copper);
                            }
                            // The board moved on while it ran: again.
                            Ok(_) => retry = Some(target),
                            Err(error) => {
                                failure = Some(format!("The fills could not be refilled: {error}"));
                            }
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
        if let Some(target) = retry {
            self.refill(engine, target);
        }
        let revision = engine.revision();
        if self.sent != Some(revision) {
            self.sent = Some(revision);
            if !engine.project().fills.is_empty()
                && let Some(jobs) = &self.jobs
                && jobs
                    .send(Job::Stale(revision, engine.project_arc().clone()))
                    .is_err()
            {
                self.jobs = None;
            }
        }
        match failure {
            Some(text) => Err(text),
            None => Ok(polled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparksmith_core::layout::fill::{FillCommand, RefillTarget};
    use sparksmith_core::library::{Outline, Segment};
    use sparksmith_core::model::CopperLayer;
    use sparksmith_core::model::fill::FillSpec;
    use sparksmith_core::{Operation, Outcome};

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
                    region: sparksmith_core::model::fill::FillRegion::Drawn,
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
            stored += worker.poll(&mut engine)?.stored;
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
        let fresh = sparksmith_core::layout::fill::refill(engine.project(), RefillTarget::All)?;
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

    /// R7-3: revisions that queue while the worker computes are not
    /// computed one by one: only the newest is, and the ones it replaced
    /// never answer.
    #[test]
    fn revisions_queued_behind_a_refill_are_skipped_but_the_newest()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::showcase()?;
        let mut worker = FillWorker::start(egui::Context::default());
        let jobs = worker.jobs.take().ok_or("no worker")?;
        let results = worker.results.take().ok_or("no worker")?;
        for revision in 1..=4u64 {
            let mut project = engine.project().clone();
            for track in &mut project.tracks {
                track.width += 0.01 * revision as f64;
            }
            jobs.send(Job::Stale(revision, Arc::new(project)))?;
        }
        let mut answered = Vec::new();
        while let Ok(done) = results.recv_timeout(std::time::Duration::from_secs(60)) {
            if let Done::Stale(revision, computed) = done {
                computed?;
                answered.push(revision);
                if revision == 4 {
                    break;
                }
            }
        }
        // The first may have started before the others came.
        assert!(answered == [4] || answered == [1, 4], "{answered:?}");
        Ok(())
    }

    /// An explicit refill runs on the worker: the UI thread gets the
    /// copper back and commits it as one undoable `fill.store_all`, the
    /// same copper `fill.refill` stores.
    #[test]
    fn an_explicit_refill_comes_back_for_one_undo_step() -> Result<(), Box<dyn std::error::Error>> {
        use sparksmith_core::layout::fill::StoredFill;
        let mut engine = Engine::showcase()?;
        let mut worker = FillWorker::start(egui::Context::default());
        // Let the background pass settle first.
        for _ in 0..100 {
            worker.poll(&mut engine)?;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let before = engine.revision();
        assert!(worker.refill(&engine, RefillTarget::All));
        assert!(worker.refilling());
        let mut refilled = None;
        for _ in 0..600 {
            if let Some(copper) = worker.poll(&mut engine)?.refilled {
                refilled = Some(copper);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let copper = refilled.ok_or("no refill")?;
        assert!(!worker.refilling());
        assert_eq!(engine.revision(), before, "nothing stored by the worker");
        let direct = refill(engine.project(), RefillTarget::All)?;
        assert_eq!(copper, direct);
        engine.apply(
            engine.revision(),
            Operation::Fill(FillCommand::StoreAll {
                fills: copper
                    .into_iter()
                    .map(|(fill, copper)| StoredFill { fill, copper })
                    .collect(),
            }),
        )?;
        assert!(engine.can_undo());
        Ok(())
    }
}
