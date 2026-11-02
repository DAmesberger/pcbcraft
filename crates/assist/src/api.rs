//! The Messages API transport: one request body in, one response out. The
//! HTTP transport reads the key from `ANTHROPIC_API_KEY` and keeps it only
//! in memory (never in a request log, a recording or an error message);
//! the replay transport answers from recorded responses, so tests and CI
//! never touch the network.
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The API version every request names.
pub const API_VERSION: &str = "2026-10-21";
/// The beta that turns on server-side refusal fallback (`fallbacks:
/// "default"`).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Longest wait for one response: a long turn over two datasheets can run
/// several minutes.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Largest response body read.
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// Where requests go.
pub trait Transport {
    /// Send one Messages request; the parsed response or an API error.
    fn send(&mut self, body: &Value) -> Result<Value>;
    /// The prompt tokens `body` would take (`messages/count_tokens`), or
    /// `None` when the transport cannot count.
    fn count_tokens(&mut self, body: &Value) -> Result<Option<u64>>;
}

/// The Claude API over HTTPS.
pub struct HttpTransport {
    agent: ureq::Agent,
    key: String,
    base: String,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl HttpTransport {
    /// A transport keyed by `ANTHROPIC_API_KEY` (and `ANTHROPIC_BASE_URL`
    /// when set). A missing key is a configuration error naming the
    /// variable, never the value.
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| Error::Config("ANTHROPIC_API_KEY is not set".into()))?;
        let base = std::env::var("ANTHROPIC_BASE_URL")
            .ok()
            .filter(|b| b.starts_with("https://"))
            .unwrap_or_else(|| "https://api.anthropic.com".into());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        Ok(Self {
            agent,
            key: key.trim().to_owned(),
            base: base.trim_end_matches('/').to_owned(),
        })
    }

    fn post(&self, path: &str, body: &Value, beta: bool) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let mut request = self
            .agent
            .post(&url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json");
        if beta {
            request = request.header("anthropic-beta", FALLBACK_BETA);
        }
        let mut response = request
            .send_json(body)
            .map_err(|e| Error::Http(redact(&e.to_string(), &self.key)))?;
        let status = response.status().as_u16();
        let value: Value = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_json()
            .map_err(|e| Error::Http(redact(&e.to_string(), &self.key)))?;
        if status >= 400 {
            let kind = value["error"]["type"].as_str().unwrap_or("error");
            let message = value["error"]["message"].as_str().unwrap_or("");
            return Err(Error::Api {
                status,
                message: redact(&format!("{kind}: {message}"), &self.key),
            });
        }
        Ok(value)
    }
}

/// `text` with every occurrence of `secret` removed.
fn redact(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_owned()
    } else {
        text.replace(secret, "[redacted]")
    }
}

impl Transport for HttpTransport {
    fn send(&mut self, body: &Value) -> Result<Value> {
        let beta = body.get("fallbacks").is_some();
        self.post("/v1/messages", body, beta)
    }

    fn count_tokens(&mut self, body: &Value) -> Result<Option<u64>> {
        // The count endpoint takes the prompt only.
        let mut prompt = json!({});
        for key in ["model", "system", "tools", "messages", "thinking"] {
            if let Some(v) = body.get(key) {
                prompt[key] = v.clone();
            }
        }
        let reply = self.post("/v1/messages/count_tokens", &prompt, false)?;
        Ok(reply["input_tokens"].as_u64())
    }
}

/// Recorded responses, answered in order; every request is kept for the
/// test to inspect.
#[derive(Debug, Default)]
pub struct ReplayTransport {
    responses: VecDeque<Value>,
    pub requests: Vec<Value>,
    /// The token count `count_tokens` answers.
    pub prompt_tokens: Option<u64>,
}

impl ReplayTransport {
    pub fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: responses.into(),
            ..Self::default()
        }
    }

    /// Responses from a JSON-lines recording (one response per line).
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut responses = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            responses.push(serde_json::from_str(line)?);
        }
        Ok(Self::new(responses))
    }
}

impl Transport for ReplayTransport {
    fn send(&mut self, body: &Value) -> Result<Value> {
        self.requests.push(body.clone());
        self.responses
            .pop_front()
            .ok_or_else(|| Error::Http("the recording has no more responses".into()))
    }

    fn count_tokens(&mut self, _body: &Value) -> Result<Option<u64>> {
        Ok(self.prompt_tokens)
    }
}

/// Wraps a transport and appends each response to a JSON-lines file, the
/// format [`ReplayTransport::from_file`] reads. Requests (which carry the
/// datasheets) are not written; the key is never part of either.
pub struct RecordingTransport<T> {
    pub inner: T,
    path: PathBuf,
}

impl<T> RecordingTransport<T> {
    pub fn new(inner: T, path: PathBuf) -> Self {
        Self { inner, path }
    }
}

impl<T: Transport> Transport for RecordingTransport<T> {
    fn send(&mut self, body: &Value) -> Result<Value> {
        let response = self.inner.send(body)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(&response)?)?;
        Ok(response)
    }

    fn count_tokens(&mut self, body: &Value) -> Result<Option<u64>> {
        self.inner.count_tokens(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_from_messages() {
        assert_eq!(
            redact("bad key sk-ant-123 here", "sk-ant-123"),
            "bad key [redacted] here"
        );
    }

    #[test]
    fn replay_answers_in_order_and_keeps_requests() -> Result<()> {
        let mut t = ReplayTransport::new(vec![json!({"id": 1}), json!({"id": 2})]);
        assert_eq!(t.send(&json!({"a": 1}))?["id"], 1);
        assert_eq!(t.send(&json!({"a": 2}))?["id"], 2);
        assert!(t.send(&json!({})).is_err());
        assert_eq!(t.requests.len(), 3);
        Ok(())
    }
}
