//! AI-assisted component design from datasheets, on any configured model.
//!
//! A run gives a model the datasheets and a goal with the Pcbcraft tool set
//! ([`tools`]) and executes every tool call in-process against an automation
//! [`Session`]: the same typed requests and command engine the MCP server,
//! CLI and desktop use. The run ends with drafts in the session and a
//! [`report::ReviewReport`] (every value with its page and table, confidence
//! and open questions). Nothing is published: the drafts carry the import
//! review state and wait for a person.
//!
//! The model layer ([`model`]) is provider-neutral: a conversation of
//! messages, tool calls and results, images and documents; capabilities
//! declared per configured model; Anthropic Messages and OpenAI-compatible
//! backends; and one recording format. A model that can't read PDFs gets
//! text and rendered pages ([`documents`]), and the review says which.
//!
//! Caps are part of the loop ([`run()`]): requests, output tokens, total
//! tokens and time, the same for every model, hosted or local, each checked
//! before a request against the worst case of the next one. Pcbcraft keeps
//! no prices and no spend: billing is the provider's. Keys come from the system
//! keychain per connection or a named environment variable, and are never
//! written anywhere ([`model::keys`]).
//!
//! This crate sits above `pcbcraft-automation` and below the apps
//! (`cargo xtask layers`); the HTTP client stays out of the core.
pub mod documents;
pub mod model;
pub mod pins;
pub mod profile;
mod prompt;
pub mod report;
mod run;
pub mod tools;
pub mod usage;

use pcbcraft_automation::Session;
use report::{ReviewReport, RunFacts};
use serde_json::json;
use std::path::PathBuf;

pub use documents::{MAX_DATASHEET_BYTES, TOKENS_PER_PAGE, pdf_pages};
pub use prompt::SYSTEM;
pub use run::{RunConfig, output_policy, run};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration: {0}")]
    Config(String),
    /// A feature that can't work on the configured model, and why.
    #[error("not possible with this model: {0}")]
    Unsupported(String),
    #[error("HTTP: {0}")]
    Http(String),
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },
    /// The server answered, but not in the shape its protocol has.
    #[error("unexpected reply: {0}")]
    Protocol(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The default model.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// A datasheet: a local PDF or an https URL the API fetches.
#[derive(Clone, Debug, PartialEq)]
pub enum Datasheet {
    File(PathBuf),
    Url(String),
}

impl Datasheet {
    /// `https://…` is a URL; anything else a file path.
    pub fn parse(text: &str) -> Self {
        if text.starts_with("https://") {
            Self::Url(text.to_owned())
        } else {
            Self::File(PathBuf::from(text))
        }
    }
}

/// What to build.
#[derive(Clone, Debug)]
pub struct Goal {
    /// The part name the drafts take.
    pub name: String,
    pub datasheets: Vec<Datasheet>,
    /// Extra instructions (variant, package, what to include).
    pub notes: String,
}

/// Progress of a run.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Request `n` is about to go out with an estimated prompt size.
    Request { n: u32, prompt_tokens: u64 },
    /// Its response: why it stopped, its output tokens (reasoning
    /// included) and how long it took.
    Response {
        n: u32,
        stop_reason: String,
        output_tokens: u64,
        seconds: f64,
    },
    /// A tool ran.
    Tool {
        name: String,
        error: bool,
        step: Option<String>,
    },
    /// Text the model wrote.
    Text(String),
    /// Something a person should know about this run's result.
    Warning(String),
}

/// Why a run stopped.
#[derive(Clone, Debug, PartialEq)]
pub enum Stop {
    /// The model submitted its review report.
    Submitted,
    /// A cap would be exceeded by the next request.
    Budget(String),
    /// Replies kept stopping at the output limit.
    CutOff(String),
    Cancelled,
    /// The model declined.
    Refused(String),
    /// The model ended without a report.
    Ended,
    /// The model kept sending a call that was refused, unchanged.
    Repeated(String),
    /// The API or transport failed.
    Failed(String),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Submitted => write!(f, "review report submitted"),
            Self::Budget(why) => write!(f, "cap reached: {why}"),
            Self::CutOff(why) => write!(f, "cut off: {why}"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Refused(why) => write!(f, "the model declined: {why}"),
            Self::Ended => write!(f, "the model ended without a report"),
            Self::Repeated(why) => write!(f, "stopped a repeating call: {why}"),
            Self::Failed(why) => write!(f, "failed: {why}"),
        }
    }
}

/// The result of a run: the report (if submitted) and what the runner
/// measured. The drafts are in the session.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub report: Option<ReviewReport>,
    pub stop: Stop,
    pub facts: RunFacts,
}

/// `NAME.review.json` and `NAME.review.md` beside the project `out`.
pub fn review_paths(out: &std::path::Path) -> [PathBuf; 2] {
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    [
        out.with_file_name(format!("{stem}.review.json")),
        out.with_file_name(format!("{stem}.review.md")),
    ]
}

/// Save the run's drafts as a new project at `out` (`project.save`) and
/// write the review beside it as JSON and Markdown, refusing to replace
/// any existing file. File effects, not document changes. Returns the
/// review paths.
pub fn write_outputs(
    session: &mut Session,
    out: &std::path::Path,
    outcome: &Outcome,
) -> Result<[PathBuf; 2]> {
    let reviews = review_paths(out);
    for path in std::iter::once(out).chain(reviews.iter().map(PathBuf::as_path)) {
        if path.try_exists()? {
            return Err(Error::Config(format!(
                "{} exists; choose a new output name",
                path.display()
            )));
        }
    }
    let saved = session.dispatch(json!({"command": "project.save",
        "params": {"path": out.to_string_lossy()}}));
    if saved["ok"] != true {
        return Err(Error::Config(format!("saving failed: {}", saved["error"])));
    }
    let json_report = json!({
        "report": outcome.report,
        "run": outcome.facts,
        "project": saved["result"],
    });
    let [json_path, md_path] = &reviews;
    write_new(
        json_path,
        serde_json::to_string_pretty(&json_report)?.as_bytes(),
    )?;
    write_new(
        md_path,
        report::markdown(outcome.report.as_ref(), &outcome.facts).as_bytes(),
    )?;
    Ok(reviews)
}

/// Create `path` with `bytes`, refusing an existing file.
pub(crate) fn write_new(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
