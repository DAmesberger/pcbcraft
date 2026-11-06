//! "Create from datasheet…" on the Component workspace start page: the
//! datasheet assistant (`pcbcraft-assist`) on a background thread, with
//! progress, spend and Cancel. The run works in its own headless session
//! through the Pcbcraft tools and saves a new project of drafts with its
//! review report beside it; nothing is published, and the open project is
//! not touched.
//!
//! The model is the user's choice: Anthropic, or any OpenAI-compatible
//! endpoint (Ollama, LM Studio, `llama-server`, OpenAI, OpenRouter), with
//! what it can do declared here; nothing is detected. A key typed here goes
//! to the system keychain for that endpoint only, and is never shown,
//! written to a file or sent over plain HTTP to another computer. Without
//! one, an Anthropic run falls back to `ANTHROPIC_API_KEY`. A full settings
//! page comes with the assistant panel.
use crate::kit;
use egui::Ui;
use pcbcraft_assist::cost::PriceTable;
use pcbcraft_assist::model::profile::ANTHROPIC_BASE_URL;
use pcbcraft_assist::model::{
    self, Budget, Connection, Endpoint, KeySource, ModelProfile, Provider, Secret, keys,
};
use pcbcraft_assist::profile as run_profile;
use pcbcraft_assist::{Datasheet, Event, Goal, RunConfig};
use pcbcraft_automation::Session;
use pcbcraft_core::Engine;
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// Lines of progress kept.
const MAX_LOG: usize = 200;
/// The context offered for a new OpenAI-compatible model: a guess the user
/// corrects to what their server gives it.
const DEFAULT_CONTEXT: f64 = 32_768.0;

/// A finished run.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub stop: String,
    pub spent_usd: f64,
    /// Whether the model had a price (otherwise only tokens are shown).
    pub priced: bool,
    pub output_tokens: u64,
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
    provider: Provider,
    endpoint: String,
    model: String,
    /// Typed once, moved to the keychain on Start, never kept.
    key: String,
    context_tokens: f64,
    vision: bool,
    native_pdf: bool,
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
            provider: Provider::Anthropic,
            endpoint: ANTHROPIC_BASE_URL.into(),
            model: pcbcraft_assist::DEFAULT_MODEL.into(),
            key: String::new(),
            context_tokens: DEFAULT_CONTEXT,
            vision: false,
            native_pdf: false,
            max_usd: pcbcraft_assist::DEFAULT_MAX_USD,
            job: None,
            spent: 0.0,
            log: Vec::new(),
            result: None,
        }
    }
}

/// What a run needs, taken from the form.
struct Plan {
    goal: Goal,
    config: RunConfig,
    endpoint: Endpoint,
    key: Option<Secret>,
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

    fn priced(&self) -> bool {
        self.provider == Provider::Anthropic
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
            "A model reads the datasheets and drafts the component, symbol, package, footprint and 3D model through the Pcbcraft tools. The result is a new project of unreviewed drafts with a review report; nothing is published.",
        );
        if let Some(job) = &self.job {
            if self.priced() {
                kit::value_row(
                    ui,
                    "Spent",
                    &format!("${:.2} of ${:.2}", self.spent, self.max_usd),
                );
            }
            for line in self.log.iter().rev().take(8).rev() {
                kit::note(ui, line);
            }
            if ui_kit::button(ui, "Cancel", Button::Normal)
                .on_hover_text("Stops after the current request")
                .clicked()
            {
                job.cancel.store(true, Ordering::Relaxed);
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        kit::multiline_row(ui, "Datasheets", &mut self.datasheets, false)
            .on_hover_text("One PDF path or https URL per line");
        kit::text_row(ui, "Part name", &mut self.name);
        kit::text_row(ui, "Save as", &mut self.out)
            .on_hover_text("A new .pcbcraft file; the review is written beside it");
        self.model_rows(ui);
        match &self.result {
            Some(Ok(done)) => {
                let spent = if done.priced {
                    format!("${:.2}", done.spent_usd)
                } else {
                    format!("{} output tokens", done.output_tokens)
                };
                kit::value_row(ui, "Result", &format!("{} ({spent})", done.stop));
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
                && self.out.trim().ends_with(".pcbcraft")
                && !self.model.trim().is_empty();
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

    /// Provider, endpoint, model, key and what the model can do.
    fn model_rows(&mut self, ui: &mut Ui) {
        let providers = [
            (Provider::Anthropic, "Anthropic".to_owned()),
            (
                Provider::OpenAiCompatible,
                "OpenAI-compatible (Ollama, LM Studio, OpenAI, …)".to_owned(),
            ),
        ];
        let before = self.provider;
        kit::choice_row(ui, "Provider", &mut self.provider, &providers);
        if self.provider != before {
            let fresh = Self::default();
            match self.provider {
                Provider::Anthropic => {
                    self.endpoint = fresh.endpoint;
                    self.model = fresh.model;
                }
                Provider::OpenAiCompatible => {
                    self.endpoint = "http://localhost:11434/v1".into();
                    self.model.clear();
                }
            }
        }
        kit::text_row(ui, "Endpoint", &mut self.endpoint).on_hover_text(
            "The API's base URL. Data goes there; a key goes only over HTTPS or to this computer",
        );
        match self.provider {
            Provider::Anthropic => {
                let models = [
                    ("claude-opus-5-5".to_owned(), "Claude Opus 5.5".to_owned()),
                    (
                        "claude-sonnet-5-5".to_owned(),
                        "Claude Sonnet 5.5 (cheaper)".to_owned(),
                    ),
                ];
                kit::choice_row(ui, "Model", &mut self.model, &models);
            }
            Provider::OpenAiCompatible => {
                kit::text_row(ui, "Model", &mut self.model)
                    .on_hover_text("The model name as the server knows it");
            }
        }
        kit::row(ui, "API key", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.key)
                    .password(true)
                    .hint_text("in the keychain, or none")
                    .desired_width(ui.available_width()),
            )
        })
        .on_hover_text("Stored in the system keychain for this endpoint only when you start. Leave empty for a local server, or to use the stored key");
        if self.provider == Provider::OpenAiCompatible {
            kit::number_row(
                ui,
                "Context",
                &mut self.context_tokens,
                "tokens",
                2_048.0..=10_000_000.0,
            );
            kit::check_row(ui, "Takes images (vision)", &mut self.vision);
            kit::check_row(ui, "Reads PDFs itself", &mut self.native_pdf);
            kit::note(
                ui,
                "Declare what this model can do; nothing is detected. Without PDF reading, the datasheets' text is sent, and pages are shown as images to a model that takes them.",
            );
        } else {
            kit::number_row(ui, "Cost cap", &mut self.max_usd, "USD", 0.5..=100.0);
        }
    }

    /// The run the form describes, or why it can't start.
    fn plan(&self) -> Result<Plan, String> {
        let policy = pcbcraft_assist::output_policy(std::path::Path::new(self.out.trim()))
            .map_err(|e| e.to_string())?;
        let endpoint = Endpoint::parse(&self.endpoint).map_err(|e| e.to_string())?;
        let key = Secret::new(&self.key);
        if key.is_some() {
            endpoint.check_key_allowed().map_err(|e| e.to_string())?;
        }
        let prices = PriceTable::default();
        let profile = match self.provider {
            Provider::Anthropic => ModelProfile::anthropic(self.model.trim(), &prices),
            Provider::OpenAiCompatible => {
                // The number row keeps the value within its range.
                let context = self.context_tokens.round() as u64;
                let mut p = ModelProfile::openai_compatible(self.model.trim(), context);
                p.capabilities.vision = self.vision;
                p.capabilities.native_pdf = self.native_pdf;
                p
            }
        };
        // The model selects its run profile: the user's file listing it,
        // else a built-in one, else the default.
        let folder = pcbcraft_io::settings::UserDirs::from_env()
            .ok()
            .map(|dirs| run_profile::user_folder(&dirs.config));
        let run_profile = run_profile::select(None, &profile.model, folder.as_deref())
            .map_err(|e| e.to_string())?;
        let config = RunConfig {
            profile,
            run_profile,
            budget: Budget {
                max_usd: self.max_usd,
                ..Budget::default()
            },
            prices,
            policy,
        };
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
        Ok(Plan {
            goal,
            config,
            endpoint,
            key,
        })
    }

    fn start(&mut self) {
        let plan = self.plan();
        // The key leaves the form whatever happens next.
        self.key.clear();
        let plan = match plan {
            Ok(plan) => plan,
            Err(why) => {
                self.result = Some(Err(why));
                return;
            }
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
                let result = work(plan, &out, &tx, &flag);
                let _ = tx.send(Message::Done(result));
            });
        match spawned {
            Ok(_) => self.job = Some(Job { rx, cancel }),
            Err(e) => self.result = Some(Err(format!("could not start: {e}"))),
        }
    }
}

/// Where this run's key comes from: one typed now goes to the keychain for
/// this endpoint; otherwise a stored one; otherwise, for Anthropic, the
/// environment; otherwise none (a local server).
fn key_source(plan: &Plan) -> Result<KeySource, String> {
    if let Some(key) = &plan.key {
        keys::keychain_store(&plan.endpoint, key).map_err(|e| e.to_string())?;
        return Ok(KeySource::Keychain);
    }
    if plan.endpoint.check_key_allowed().is_ok() && keys::keychain_has(&plan.endpoint) {
        return Ok(KeySource::Keychain);
    }
    Ok(match plan.config.profile.provider {
        Provider::Anthropic => KeySource::Env("ANTHROPIC_API_KEY".into()),
        Provider::OpenAiCompatible => KeySource::None,
    })
}

/// The run on the background thread.
fn work(
    plan: Plan,
    out: &std::path::Path,
    tx: &Sender<Message>,
    cancel: &AtomicBool,
) -> Result<Finished, String> {
    for path in std::iter::once(out.to_path_buf()).chain(pcbcraft_assist::review_paths(out)) {
        if path.exists() {
            return Err(format!("{} exists; choose a new name", path.display()));
        }
    }
    let connection = Connection {
        key: key_source(&plan)?,
        endpoint: plan.endpoint.clone(),
    };
    let mut backend =
        model::connect(&plan.config.profile, &connection).map_err(|e| e.to_string())?;
    let mut session = Session::new(Engine::default());
    let _ = tx.send(Message::Line(format!(
        "Run profile {}",
        plan.config.run_profile.name
    )));
    let outcome = pcbcraft_assist::run(
        &plan.goal,
        &plan.config,
        backend.as_mut(),
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
                    output_tokens,
                    seconds,
                } => {
                    let _ = tx.send(Message::Spent(spent_usd));
                    format!("Response {n}: {stop_reason}, {output_tokens} tokens in {seconds:.0} s")
                }
                Event::Tool { name, error, step } => {
                    let status = if error { "refused" } else { "ok" };
                    format!(
                        "{name}: {status}{}",
                        step.map(|s| format!(" ({s})")).unwrap_or_default()
                    )
                }
                Event::Text(text) => text.chars().take(160).collect(),
                Event::Warning(text) => format!("Warning: {text}"),
            };
            let _ = tx.send(Message::Line(line));
        },
        cancel,
    )
    .map_err(|e| e.to_string())?;
    let [_, review] =
        pcbcraft_assist::write_outputs(&mut session, out, &outcome).map_err(|e| e.to_string())?;
    Ok(Finished {
        stop: outcome.stop.to_string(),
        spent_usd: outcome.facts.spent_usd,
        priced: outcome.facts.cap_usd.is_some(),
        output_tokens: outcome.facts.output_tokens,
        project: out.to_path_buf(),
        review,
    })
}

#[cfg(test)]
mod tests;
