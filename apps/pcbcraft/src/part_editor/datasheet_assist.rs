//! "Create from datasheet…" on the Component workspace start page: the
//! datasheet assistant (`pcbcraft-assist`) on a background thread, with
//! progress, spend and Cancel. The run works in its own headless session
//! through the Pcbcraft tools and saves a new project of drafts with its
//! review report beside it; nothing is published, and the open project is
//! not touched. The API key comes from `ANTHROPIC_API_KEY` in the
//! environment and is never shown or stored.
use crate::kit;
use egui::Ui;
use pcbcraft_assist::api::HttpTransport;
use pcbcraft_assist::{Datasheet, Event, Goal, Settings};
use pcbcraft_automation::Session;
use pcbcraft_core::Engine;
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// Lines of progress kept.
const MAX_LOG: usize = 200;

/// A finished run.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub stop: String,
    pub spent_usd: f64,
    pub project: PathBuf,
    pub review: PathBuf,
}

enum Message {
    Line(String),
    Spent(f64),
    Done(Result<Finished, String>),
}

struct Job {
    rx: Receiver<Message>,
    cancel: Arc<AtomicBool>,
}

/// The form, the running job and its log.
pub struct DatasheetAssist {
    pub open: bool,
    datasheets: String,
    name: String,
    out: String,
    model: String,
    max_usd: f64,
    job: Option<Job>,
    spent: f64,
    log: Vec<String>,
    result: Option<Result<Finished, String>>,
}

impl Default for DatasheetAssist {
    fn default() -> Self {
        Self {
            open: false,
            datasheets: String::new(),
            name: String::new(),
            out: String::new(),
            model: pcbcraft_assist::DEFAULT_MODEL.into(),
            max_usd: pcbcraft_assist::DEFAULT_MAX_USD,
            job: None,
            spent: 0.0,
            log: Vec::new(),
            result: None,
        }
    }
}

impl DatasheetAssist {
    /// Take the background thread's messages.
    fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let mut done = None;
        while let Ok(message) = job.rx.try_recv() {
            match message {
                Message::Line(line) => {
                    self.log.push(line);
                    if self.log.len() > MAX_LOG {
                        self.log.remove(0);
                    }
                }
                Message::Spent(usd) => self.spent = usd,
                Message::Done(result) => done = Some(result),
            }
        }
        if let Some(result) = done {
            self.result = Some(result);
            self.job = None;
        }
    }

    /// The form and progress; on the start page under New.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        if !self.open {
            return;
        }
        kit::search_heading(ui, "Create from datasheet", 0);
        kit::note(
            ui,
            "Claude reads the datasheets and drafts the component, symbol, package, footprint and 3D model through the Pcbcraft tools. The result is a new project of unreviewed drafts with a review report; nothing is published.",
        );
        if let Some(job) = &self.job {
            kit::value_row(ui, "Spent", &format!("${:.2} of ${:.2}", self.spent, self.max_usd));
            for line in self.log.iter().rev().take(8).rev() {
                kit::note(ui, line);
            }
            if ui_kit::button(ui, "Cancel", Button::Normal)
                .on_hover_text("Stops after the current request")
                .clicked()
            {
                job.cancel.store(true, Ordering::Relaxed);
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        kit::multiline_row(ui, "Datasheets", &mut self.datasheets, false)
            .on_hover_text("One PDF path or https URL per line");
        kit::text_row(ui, "Part name", &mut self.name);
        kit::text_row(ui, "Save as", &mut self.out)
            .on_hover_text("A new .pcbcraft file; the review is written beside it");
        let models = [
            ("claude-opus-5-5".to_owned(), "Claude Opus 5.5".to_owned()),
            ("claude-sonnet-5-5".to_owned(), "Claude Sonnet 5.5 (cheaper)".to_owned()),
        ];
        kit::choice_row(ui, "Model", &mut self.model, &models);
        kit::number_row(ui, "Cost cap", &mut self.max_usd, "USD", 0.5..=100.0);
        match &self.result {
            Some(Ok(done)) => {
                kit::value_row(ui, "Result", &format!("{} (${:.2})", done.stop, done.spent_usd));
                kit::value_row(ui, "Project", &done.project.display().to_string());
                kit::value_row(ui, "Review", &done.review.display().to_string());
            }
            Some(Err(message)) => {
                kit::refusal(ui, message);
            }
            None => {}
        }
        ui.horizontal(|ui| {
            let ready = !self.datasheets.trim().is_empty()
                && !self.name.trim().is_empty()
                && self.out.trim().ends_with(".pcbcraft");
            if ui
                .add_enabled_ui(ready, |ui| ui_kit::button(ui, "Start", Button::Primary))
                .inner
                .clicked()
            {
                self.start();
            }
            if ui_kit::button(ui, "Close", Button::Ghost).clicked() {
                self.open = false;
            }
        });
    }

    fn start(&mut self) {
        let goal = Goal {
            name: self.name.trim().to_owned(),
            datasheets: self
                .datasheets
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(Datasheet::parse)
                .collect(),
            notes: String::new(),
        };
        let settings = Settings {
            model: self.model.clone(),
            max_usd: self.max_usd,
            ..Settings::default()
        };
        let out = PathBuf::from(self.out.trim());
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        self.log.clear();
        self.spent = 0.0;
        self.result = None;
        let spawned = std::thread::Builder::new()
            .name("datasheet-assist".into())
            .spawn(move || {
                let result = work(&goal, &settings, &out, &tx, &flag);
                let _ = tx.send(Message::Done(result));
            });
        match spawned {
            Ok(_) => self.job = Some(Job { rx, cancel }),
            Err(e) => self.result = Some(Err(format!("could not start: {e}"))),
        }
    }
}

/// The run on the background thread.
fn work(
    goal: &Goal,
    settings: &Settings,
    out: &std::path::Path,
    tx: &Sender<Message>,
    cancel: &AtomicBool,
) -> Result<Finished, String> {
    for path in std::iter::once(out.to_path_buf()).chain(pcbcraft_assist::review_paths(out)) {
        if path.exists() {
            return Err(format!("{} exists; choose a new name", path.display()));
        }
    }
    let mut transport = HttpTransport::from_env().map_err(|e| e.to_string())?;
    let mut session = Session::new(Engine::default());
    let mut outcome = pcbcraft_assist::run(
        goal,
        settings,
        &mut transport,
        &mut session,
        &mut |event| {
            let line = match event {
                Event::Request { n, prompt_tokens } => {
                    format!("Request {n} (~{prompt_tokens} prompt tokens)")
                }
                Event::Response {
                    n,
                    stop_reason,
                    spent_usd,
                } => {
                    let _ = tx.send(Message::Spent(spent_usd));
                    format!("Response {n}: {stop_reason}")
                }
                Event::Tool { name, error, step } => {
                    let status = if error { "refused" } else { "ok" };
                    format!("{name}: {status}{}", step.map(|s| format!(" ({s})")).unwrap_or_default())
                }
                Event::Text(text) => text.chars().take(160).collect(),
            };
            let _ = tx.send(Message::Line(line));
        },
        cancel,
    )
    .map_err(|e| e.to_string())?;
    outcome.facts.transport = "the Claude API (desktop)".into();
    let [_, review] =
        pcbcraft_assist::write_outputs(&mut session, out, &outcome).map_err(|e| e.to_string())?;
    Ok(Finished {
        stop: outcome.stop.to_string(),
        spent_usd: outcome.facts.spent_usd,
        project: out.to_path_buf(),
        review,
    })
}

#[cfg(test)]
mod tests;
