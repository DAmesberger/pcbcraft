//! File › Export to SolidCraft…: the tier choice, the export off the UI
//! thread with its progress and Stop, and what it found (slice A5 of
//! docs/architecture/board-plan-and-interface.md).
//!
//! The window asks for the tier first: *mechanical* (the laminate, the pads
//! and the parts; the default, as the founder chose) or *full* (with every
//! track, via and fill). The save panel follows ([`super::file_ui`] owns
//! it). The export then runs on its own thread through
//! `pcbcraft_solidcraft::export::board`, the call `project.export_native`
//! makes for the CLI and MCP, on the document as it was when the file was
//! chosen; saving, recovery and editing go on meanwhile. The window shows
//! the stage and the commands run and offers Stop, which ends the export
//! at its next command and writes nothing. The result says where the
//! model was written and whether every body was built as the board plans
//! it; a body that differs is named.
use crate::kit::severity::Level;
use pcbcraft_core::Project;
use pcbcraft_solidcraft::export::{self, Report, Tier, Watch};
use pcbcraft_ui::ui_kit::{self, Button};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Differing bodies the result names; the rest are counted.
const NAMED: usize = 3;

/// The window's title.
pub(crate) const TITLE: &str = "Export to SolidCraft";
/// The choice's and the running export's buttons.
pub(crate) const EXPORT: &str = "Export…";
pub(crate) const STOP: &str = "Stop";

/// The tiers the window offers, as it names them.
const TIERS: [(Tier, &str, &str); 2] = [
    (
        Tier::Mechanical,
        "Mechanical: laminate, pads and parts",
        "For an enclosure and for fit checks. Quick even on large boards.",
    ),
    (
        Tier::Full,
        "Full: with every track, via and fill",
        "Every copper body as well. Large boards take minutes.",
    ),
];

struct Running {
    watch: Arc<Watch>,
    receiver: Receiver<Result<Report, String>>,
    path: PathBuf,
    /// The document exported.
    project: Arc<Project>,
    /// Seconds (the app's clock) when it started.
    started: f64,
}

/// What the export window shows and runs.
pub(crate) struct ModelExport {
    ctx: egui::Context,
    /// The window is open to choose; the tier chosen.
    choosing: Option<Tier>,
    /// The tier of the export whose file is being chosen.
    pub(crate) chosen: Tier,
    running: Option<Running>,
    status: Option<(Level, String)>,
    now: f64,
}

/// What the window's answer asks the file workflow to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// Ask where to write.
    Pick,
}

impl ModelExport {
    pub(crate) fn new(ctx: &egui::Context) -> Self {
        Self {
            ctx: ctx.clone(),
            choosing: None,
            chosen: Tier::Mechanical,
            running: None,
            status: None,
            now: 0.0,
        }
    }

    /// File › Export to SolidCraft…: open the choice, unless an export runs.
    pub(crate) fn open(&mut self) {
        if self.running.is_some() {
            self.status = Some((
                Level::Warning,
                "An export to SolidCraft is still running".into(),
            ));
            return;
        }
        self.choosing = Some(self.chosen);
    }

    /// Whether an export is running.
    #[cfg(test)]
    pub(crate) fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Whether the window is open (to choose, or showing an export).
    #[cfg(test)]
    pub(crate) fn showing(&self) -> bool {
        self.choosing.is_some() || self.running.is_some()
    }

    /// Stop the running export at its next command; it writes nothing.
    pub(crate) fn stop(&mut self) {
        if let Some(running) = &self.running {
            running.watch.cancel();
        }
    }

    /// The latest export message.
    pub(crate) fn take_status(&mut self) -> Option<(Level, String)> {
        self.status.take()
    }

    /// Export `project` at the chosen tier to `path` on a thread of its own.
    pub(crate) fn start(&mut self, project: Arc<Project>, path: PathBuf) {
        if self.running.is_some() {
            self.status = Some((
                Level::Warning,
                "An export to SolidCraft is still running".into(),
            ));
            return;
        }
        let watch = Watch::new();
        let (sender, receiver) = mpsc::channel();
        let tier = self.chosen;
        let thread_watch = Arc::clone(&watch);
        let thread_project = Arc::clone(&project);
        let thread_path = path.clone();
        let repaint = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("pcbcraft-model-export".into())
            .spawn(move || {
                let result = thread_path
                    .to_str()
                    .ok_or_else(|| "The export path must be UTF-8".to_owned())
                    .and_then(|path| {
                        export::board(&thread_project, tier, path, &thread_watch)
                            .map_err(|e| e.to_string())
                    });
                let _ = sender.send(result);
                repaint.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.running = Some(Running {
                    watch,
                    receiver,
                    path,
                    project,
                    started: self.now,
                });
            }
            Err(error) => {
                self.status = Some((Level::Error, format!("The export could not start: {error}")));
            }
        }
    }

    /// Take the result of a finished export; `current` says whether the
    /// document is still the one exported.
    pub(crate) fn poll(&mut self, now: f64, current: &Arc<Project>) {
        self.now = now;
        let Some(running) = &self.running else {
            return;
        };
        let result = match running.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.ctx
                    .request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Err(TryRecvError::Disconnected) => Err("The export stopped unexpectedly".into()),
        };
        let Some(running) = self.running.take() else {
            return;
        };
        let stopped = running.watch.is_cancelled();
        let changed = !Arc::ptr_eq(&running.project, current);
        self.status = Some(outcome(&running, result, stopped, changed));
    }

    /// The window: the tier choice, or the running export.
    /// Returns [`Answer::Pick`] when the user asked to choose the file.
    pub(crate) fn show(&mut self, ctx: &egui::Context) -> Option<Answer> {
        if let Some(running) = &self.running {
            let progress = running.watch.progress();
            let seconds = (self.now - running.started).max(0.0);
            let file = running
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut stop = false;
            ui_kit::Window::new(egui::Id::unique("model-export"), "export", TITLE)
                .required()
                .show_body(ctx, |ui| {
                    super::file_ui::dialog_body(ui, |ui| {
                        crate::kit::value_row(ui, "File", &file);
                        crate::kit::value_row(ui, "Tier", tier_name(self.chosen));
                        crate::kit::status(
                            ui,
                            &format!("{} · {seconds:.0} s", progress.text()),
                            crate::kit::Tone::Dim,
                        );
                        crate::kit::note(
                            ui,
                            "You can keep working; the export uses the board as it was when it started.",
                        );
                        crate::kit::actions(ui, |ui| {
                            let enabled = !progress.cancelled;
                            if ui
                                .add_enabled_ui(enabled, |ui| {
                                    ui_kit::button(ui, STOP, Button::Normal)
                                })
                                .inner
                                .clicked()
                            {
                                stop = true;
                            }
                        });
                    });
                });
            if stop {
                self.stop();
            }
            return None;
        }
        let mut tier = self.choosing?;
        let mut answer = None;
        let mut cancel = false;
        let closed = ui_kit::Window::new(egui::Id::unique("model-export"), "export", TITLE)
            .show_body(ctx, |ui| {
                super::file_ui::dialog_body(ui, |ui| {
                    for (option, label, note) in TIERS {
                        let mut on = tier == option;
                        if crate::kit::check_row(ui, label, &mut on) && on {
                            tier = option;
                        }
                        crate::kit::note(ui, note);
                    }
                    crate::kit::note(
                        ui,
                        "Every body is checked against the board before the file is written.",
                    );
                    crate::kit::actions(ui, |ui| {
                        if crate::kit::primary(ui, EXPORT, true).clicked() {
                            answer = Some(Answer::Pick);
                        }
                        if ui_kit::button(ui, "Cancel", Button::Normal).clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        self.chosen = tier;
        if answer.is_some() || cancel || closed {
            self.choosing = None;
        } else {
            self.choosing = Some(tier);
        }
        answer
    }

    /// Wait for the running export to end (tests).
    #[cfg(test)]
    pub(crate) fn wait(&mut self, current: &Arc<Project>) {
        for _ in 0..12_000 {
            if self.running.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            self.poll(self.now, current);
        }
    }
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Interface => "Interface",
        Tier::Mechanical => "Mechanical",
        Tier::Full => "Full",
    }
}

/// The message a finished export leaves.
fn outcome(
    running: &Running,
    result: Result<Report, String>,
    stopped: bool,
    changed: bool,
) -> (Level, String) {
    let path = running.path.display();
    let report = match result {
        Ok(report) => report,
        Err(_) if stopped => {
            return (
                Level::Info,
                "Export to SolidCraft stopped; nothing was written".into(),
            );
        }
        Err(error) => return (Level::Error, format!("Export failed: {error}")),
    };
    let captured = if changed {
        "; the board has changed since it started"
    } else {
        ""
    };
    if report.differs() {
        let lines = report.lines(&running.project);
        let mut named: Vec<String> = lines.iter().take(NAMED).cloned().collect();
        let count = report.difference_count();
        if count > named.len() {
            named.push(format!("and {} more", count - named.len()));
        }
        return (
            Level::Warning,
            format!(
                "Exported {path}, but {count} {} the board: {}{captured}",
                if count == 1 {
                    "body differs from"
                } else {
                    "bodies differ from"
                },
                named.join("; ")
            ),
        );
    }
    let checked = report.check.as_ref().map_or(0, |c| c.built);
    (
        Level::Ok,
        format!(
            "Exported {path} ({}, {} bodies; all {checked} board bodies as planned){captured}",
            tier_name(report.tier).to_lowercase(),
            report.bodies
        ),
    )
}

#[cfg(test)]
#[path = "model_export_tests.rs"]
mod tests;
