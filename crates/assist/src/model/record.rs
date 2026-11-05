//! Recording and replay, in one neutral format: JSON lines of
//! [`ModelReply`]. A recording made on any backend replays offline, with no
//! key and no network. Requests carry the datasheets and are not written;
//! the key is never part of a reply.
use super::{Message, ModelBackend, ModelReply, ModelRequest, ToolSpec};
use crate::{Error, Result};
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What a replay kept of a request, for tests to inspect.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptRequest {
    pub system: String,
    pub tools: Vec<ToolSpec>,
    pub messages: Vec<Message>,
    pub max_output_tokens: u32,
    pub timeout: Duration,
}

/// Recorded replies, answered in order.
#[derive(Debug, Default)]
pub struct Replay {
    replies: VecDeque<ModelReply>,
    source: String,
    /// Keep each request (they hold the datasheets, so off for real files).
    pub keep_requests: bool,
    pub requests: Vec<KeptRequest>,
    /// What a token count answers.
    pub prompt_tokens: Option<u64>,
}

impl Replay {
    pub fn new(replies: Vec<ModelReply>) -> Self {
        Self {
            replies: replies.into(),
            source: "a test script".into(),
            keep_requests: true,
            ..Self::default()
        }
    }

    /// Replies from a recording.
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut replies = Vec::new();
        for (k, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            replies.push(serde_json::from_str(line).map_err(|e| {
                Error::Config(format!(
                    "{} line {} is not a neutral reply: {e}",
                    path.display(),
                    k + 1
                ))
            })?);
        }
        Ok(Self {
            replies: replies.into(),
            source: path.display().to_string(),
            ..Self::default()
        })
    }
}

impl ModelBackend for Replay {
    fn label(&self) -> String {
        format!("a replay of {} (no API calls; spend not real)", self.source)
    }

    fn send(&mut self, request: &ModelRequest<'_>) -> Result<ModelReply> {
        if self.keep_requests {
            self.requests.push(KeptRequest {
                system: request.system.to_owned(),
                tools: request.tools.to_vec(),
                messages: request.messages.to_vec(),
                max_output_tokens: request.max_output_tokens,
                timeout: request.timeout,
            });
        }
        self.replies
            .pop_front()
            .ok_or_else(|| Error::Http("the recording has no more responses".into()))
    }

    fn count_prompt_tokens(&mut self, _request: &ModelRequest<'_>) -> Result<Option<u64>> {
        Ok(self.prompt_tokens)
    }
}

/// Wraps a backend and appends each reply to a JSON-lines file, the format
/// [`Replay::from_file`] reads.
pub struct Recording<B> {
    pub inner: B,
    path: PathBuf,
}

impl<B> Recording<B> {
    pub fn new(inner: B, path: PathBuf) -> Self {
        Self { inner, path }
    }
}

impl<B: ModelBackend + ?Sized> ModelBackend for Box<B> {
    fn label(&self) -> String {
        (**self).label()
    }

    fn send(&mut self, request: &ModelRequest<'_>) -> Result<ModelReply> {
        (**self).send(request)
    }

    fn count_prompt_tokens(&mut self, request: &ModelRequest<'_>) -> Result<Option<u64>> {
        (**self).count_prompt_tokens(request)
    }
}

impl<B: ModelBackend> ModelBackend for Recording<B> {
    fn label(&self) -> String {
        format!(
            "{}, recorded to {}",
            self.inner.label(),
            self.path.display()
        )
    }

    fn send(&mut self, request: &ModelRequest<'_>) -> Result<ModelReply> {
        let reply = self.inner.send(request)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(&reply)?)?;
        Ok(reply)
    }

    fn count_prompt_tokens(&mut self, request: &ModelRequest<'_>) -> Result<Option<u64>> {
        self.inner.count_prompt_tokens(request)
    }
}

#[cfg(test)]
mod tests {
    use super::super::wire::{AuthStyle, HttpWire};
    use super::super::{Endpoint, ModelProfile, Part, Secret, StopReason, openai};
    use super::*;
    use crate::cost::Usage;
    use std::io::Read;

    fn reply(id: &str) -> ModelReply {
        ModelReply {
            model: "m".into(),
            content: vec![Part::text(id)],
            stop: StopReason::EndTurn,
            usage: Usage::default(),
            usage_estimated: false,
        }
    }

    fn request<'a>() -> ModelRequest<'a> {
        ModelRequest {
            system: "S",
            tools: &[],
            messages: &[],
            max_output_tokens: 1,
            timeout: Duration::from_secs(20),
        }
    }

    #[test]
    fn replay_answers_in_order_and_keeps_requests() -> Result<()> {
        let mut r = Replay::new(vec![reply("1"), reply("2")]);
        assert_eq!(r.send(&request())?.content, vec![Part::text("1")]);
        assert_eq!(r.send(&request())?.content, vec![Part::text("2")]);
        assert!(r.send(&request()).is_err());
        assert_eq!(r.requests.len(), 3);
        Ok(())
    }

    #[test]
    fn a_recording_replays_and_holds_no_key_or_request() -> Result<()> {
        // A real HTTP round trip through the OpenAI-compatible backend, with
        // a key, on this computer; the server echoes the key in its reply.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let base = format!("http://{}", listener.local_addr()?);
        let server = std::thread::spawn(move || {
            use std::io::Write as _;
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut seen = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stream.read(&mut chunk) {
                seen.extend_from_slice(chunk.get(..n).unwrap_or_default());
                if n == 0 || String::from_utf8_lossy(&seen).contains("SYSTEM-END") {
                    break;
                }
            }
            let body = r#"{"model":"m","choices":[{"message":{"content":"key is sk-rec-9"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":6}}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        let wire = HttpWire::new(
            Endpoint::parse(&base)?,
            Secret::new("sk-rec-9"),
            AuthStyle::Bearer,
        )?;
        let backend = openai::OpenAiBackend::new(ModelProfile::openai_compatible("m", 1_000), wire);
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("r.jsonl");
        let mut recording = Recording::new(backend, path.clone());
        let mut asked = request();
        asked.system = "DATASHEET-TEXT SYSTEM-END";
        let reply = recording.send(&asked)?;
        let _ = server.join();
        assert_eq!(reply.usage.output_tokens, 6);
        let mut text = String::new();
        std::fs::File::open(&path)?.read_to_string(&mut text)?;
        assert!(!text.contains("sk-rec-9"), "{text}");
        assert!(text.contains("key is [redacted]"), "{text}");
        assert!(!text.contains("DATASHEET-TEXT"), "{text}");
        assert!(!recording.label().contains("sk-rec-9"));
        let mut replay = Replay::from_file(&path)?;
        assert_eq!(replay.send(&request())?, reply);
        Ok(())
    }

    #[test]
    fn a_line_in_another_format_is_refused_by_name() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("old.jsonl");
        std::fs::write(&path, "{\"content\": [], \"stop_reason\": \"end_turn\"}\n")?;
        let error = Replay::from_file(&path).err().map(|e| e.to_string());
        assert!(
            error.as_deref().is_some_and(|e| e.contains("line 1")),
            "{error:?}"
        );
        Ok(())
    }
}
