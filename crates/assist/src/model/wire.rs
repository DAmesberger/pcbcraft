//! The HTTP seam under the backends: one JSON POST in, one JSON value out.
//! [`HttpWire`] is the only code that holds a key at request time. It
//! follows no redirects (a redirect could carry the key elsewhere), bounds
//! the response, and removes the key from everything it returns.
use super::{Endpoint, Secret};
use crate::{Error, Result};
use serde_json::Value;
use std::time::Duration;

/// Largest response body read.
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// How a key is presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthStyle {
    /// `x-api-key: KEY`.
    ApiKeyHeader,
    /// `Authorization: Bearer KEY`.
    Bearer,
}

/// Where a backend's requests go.
pub trait Wire {
    /// Where this goes, for the review report; never holds a key.
    fn label(&self) -> String;
    /// POST `body` to the endpoint's `path`.
    fn post(
        &mut self,
        path: &str,
        headers: &[(&str, &str)],
        body: &Value,
        timeout: Duration,
    ) -> Result<Value>;
}

/// The real thing, over ureq.
pub struct HttpWire {
    agent: ureq::Agent,
    endpoint: Endpoint,
    key: Option<Secret>,
    auth: AuthStyle,
}

impl std::fmt::Debug for HttpWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpWire")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl HttpWire {
    pub fn new(endpoint: Endpoint, key: Option<Secret>, auth: AuthStyle) -> Result<Self> {
        if key.is_some() {
            endpoint.check_key_allowed()?;
        }
        let mut config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0);
        if endpoint.is_loopback() {
            // Requests for this computer never go through a proxy.
            config = config.proxy(None);
        }
        Ok(Self {
            agent: config.build().into(),
            endpoint,
            key,
            auth,
        })
    }

    fn clean(&self, text: &str) -> String {
        match &self.key {
            Some(key) => key.redact(text),
            None => text.to_owned(),
        }
    }
}

impl Wire for HttpWire {
    fn label(&self) -> String {
        format!("{} ({})", self.endpoint.base(), self.endpoint.location())
    }

    fn post(
        &mut self,
        path: &str,
        headers: &[(&str, &str)],
        body: &Value,
        timeout: Duration,
    ) -> Result<Value> {
        let url = format!("{}{path}", self.endpoint.base());
        let mut request = self
            .agent
            .post(&url)
            .config()
            .timeout_global(Some(timeout))
            .build()
            .header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(key) = &self.key {
            request = match self.auth {
                AuthStyle::ApiKeyHeader => request.header("x-api-key", key.expose()),
                AuthStyle::Bearer => {
                    request.header("authorization", format!("Bearer {}", key.expose()))
                }
            };
        }
        let mut response = request
            .send_json(body)
            .map_err(|e| Error::Http(self.clean(&e.to_string())))?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(Error::Api {
                status,
                message: "the endpoint redirected the request; give the final URL".into(),
            });
        }
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|e| Error::Http(self.clean(&e.to_string())))?;
        // A server that echoes the key cannot put it in a log or a recording.
        let text = self.clean(&text);
        let value = serde_json::from_str::<Value>(&text);
        if status >= 400 {
            return Err(Error::Api {
                status,
                message: error_message(value.as_ref().ok(), &text),
            });
        }
        value.map_err(|e| Error::Protocol(format!("the reply is not JSON: {e}")))
    }
}

/// The message of an error body, in any of the common shapes.
fn error_message(value: Option<&Value>, text: &str) -> String {
    const MAX: usize = 2000;
    let found = value.and_then(|v| {
        let error = &v["error"];
        if let Some(message) = error["message"].as_str() {
            let kind = error["type"].as_str().unwrap_or("error");
            Some(format!("{kind}: {message}"))
        } else if let Some(message) = error.as_str() {
            Some(message.to_owned())
        } else {
            v["message"].as_str().map(str::to_owned)
        }
    });
    let message = found.unwrap_or_else(|| text.trim().to_owned());
    message.chars().take(MAX).collect()
}

/// One POST a test script received: path, headers, body and timeout.
#[cfg(test)]
pub type Post = (String, Vec<(String, String)>, Value, Duration);

/// Canned wire replies for tests of the backends, with each request kept.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct ScriptedWire {
    pub replies: std::collections::VecDeque<Result<Value>>,
    /// (path, headers, body, timeout) of each POST.
    pub posts: Vec<Post>,
    /// What a count request answers.
    pub count: Option<u64>,
}

#[cfg(test)]
impl ScriptedWire {
    pub fn new(replies: Vec<Value>) -> Self {
        Self {
            replies: replies.into_iter().map(Ok).collect(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
impl Wire for ScriptedWire {
    fn label(&self) -> String {
        "a test script".into()
    }

    fn post(
        &mut self,
        path: &str,
        headers: &[(&str, &str)],
        body: &Value,
        timeout: Duration,
    ) -> Result<Value> {
        if path.ends_with("count_tokens") {
            return match self.count {
                Some(n) => Ok(serde_json::json!({"input_tokens": n})),
                None => Err(Error::Http("no count".into())),
            };
        }
        self.posts.push((
            path.to_owned(),
            headers
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect(),
            body.clone(),
            timeout,
        ));
        self.replies
            .pop_front()
            .unwrap_or_else(|| Err(Error::Http("the recording has no more responses".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A one-shot HTTP server on this computer: answers `reply` once and
    /// returns the request text it received.
    fn server(reply: String) -> Result<(String, std::thread::JoinHandle<String>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let base = format!("http://{}", listener.local_addr()?);
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            if let Ok((mut stream, _)) = listener.accept() {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    seen.extend_from_slice(chunk.get(..n).unwrap_or_default());
                    let text = String::from_utf8_lossy(&seen);
                    if let Some(split) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if seen.len() >= split + 4 + length {
                            break;
                        }
                    }
                }
                let _ = stream.write_all(reply.as_bytes());
            }
            String::from_utf8_lossy(&seen).into_owned()
        });
        Ok((base, handle))
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    const LONG: Duration = Duration::from_secs(20);

    #[test]
    fn a_real_post_carries_the_key_only_in_its_header_and_hides_an_echoed_key() -> Result<()> {
        let (base, handle) = server(http(
            "200 OK",
            r#"{"echo": "you sent sk-secret-1", "ok": true}"#,
        ))?;
        let mut wire = HttpWire::new(
            Endpoint::parse(&base)?,
            Secret::new("sk-secret-1"),
            AuthStyle::Bearer,
        )?;
        let reply = wire.post("/chat", &[("x-extra", "1")], &json!({"a": 1}), LONG)?;
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["echo"], "you sent [redacted]");
        let seen = handle.join().unwrap_or_default().to_ascii_lowercase();
        assert!(seen.starts_with("post /chat "), "{seen}");
        assert!(seen.contains("authorization: bearer sk-secret-1"), "{seen}");
        assert!(seen.contains("x-extra: 1"), "{seen}");
        assert!(!format!("{wire:?}").contains("sk-secret-1"));
        Ok(())
    }

    #[test]
    fn an_error_body_is_reported_without_the_key() -> Result<()> {
        let (base, handle) = server(http(
            "401 Unauthorized",
            r#"{"error": {"type": "auth", "message": "bad key sk-secret-2"}}"#,
        ))?;
        let mut wire = HttpWire::new(
            Endpoint::parse(&base)?,
            Secret::new("sk-secret-2"),
            AuthStyle::ApiKeyHeader,
        )?;
        let error = wire.post("/x", &[], &json!({}), LONG).err();
        let text = error.as_ref().map(ToString::to_string).unwrap_or_default();
        assert!(
            matches!(error, Some(Error::Api { status: 401, .. })),
            "{text}"
        );
        assert!(text.contains("bad key [redacted]"), "{text}");
        assert!(!text.contains("sk-secret-2"));
        let seen = handle.join().unwrap_or_default().to_ascii_lowercase();
        assert!(seen.contains("x-api-key: sk-secret-2"), "{seen}");
        Ok(())
    }

    #[test]
    fn a_redirect_is_not_followed() -> Result<()> {
        let (base, handle) = server(
            "HTTP/1.1 307 Temporary Redirect\r\nlocation: http://192.0.2.1/steal\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                .to_owned(),
        )?;
        let mut wire = HttpWire::new(
            Endpoint::parse(&base)?,
            Secret::new("sk-secret-3"),
            AuthStyle::ApiKeyHeader,
        )?;
        let error = wire.post("/x", &[], &json!({}), LONG).err();
        assert!(
            matches!(error, Some(Error::Api { status: 307, .. })),
            "{error:?}"
        );
        let _ = handle.join();
        Ok(())
    }

    #[test]
    fn a_non_json_reply_is_an_error_not_a_panic() -> Result<()> {
        let (base, handle) = server(http("200 OK", "<html>gateway</html>"))?;
        let mut wire = HttpWire::new(Endpoint::parse(&base)?, None, AuthStyle::Bearer)?;
        let error = wire.post("/x", &[], &json!({}), LONG).err();
        assert!(matches!(error, Some(Error::Protocol(_))), "{error:?}");
        let _ = handle.join();
        Ok(())
    }

    #[test]
    fn error_shapes_of_local_servers_are_read() {
        let v = json!({"error": "model not found"});
        assert_eq!(error_message(Some(&v), ""), "model not found");
        let v = json!({"error": {"message": "too long", "type": "invalid"}});
        assert_eq!(error_message(Some(&v), ""), "invalid: too long");
        assert_eq!(error_message(None, " bad gateway "), "bad gateway");
    }
}
