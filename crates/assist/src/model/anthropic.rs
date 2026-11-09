//! Anthropic Messages. Today's behaviour, behind the neutral interface:
//! adaptive thinking, effort, the server-side fallback beta, prompt-cache
//! breakpoints on the system prompt and the last document plus automatic
//! caching of the tail, and every assistant block sent back byte for byte.
//!
//! Replay is exact by construction: [`parse_reply`] keeps a block in its
//! part's `replay` whenever the neutral fields would not rebuild it, thinking
//! blocks always, so a later request carries what the API sent. Opus 5.5
//! binds thinking to the conversation before it, and a rewrite would also
//! re-write the cache.
use super::wire::{HttpWire, Wire};
use super::{
    DocumentSource, Message, ModelBackend, ModelProfile, ModelReply, ModelRequest, Part, Role,
    StopReason,
};
use crate::usage::Usage;
use crate::{Error, Result};
use serde_json::{Value, json};
use std::time::Duration;

/// The API version every request names.
pub const API_VERSION: &str = "2026-10-21";
/// The beta that turns on server-side refusal fallback.
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Longest wait for a token count.
const COUNT_TIMEOUT: Duration = Duration::from_secs(120);

/// The Messages backend over a wire.
pub struct AnthropicBackend<W> {
    profile: ModelProfile,
    pub wire: W,
}

impl std::fmt::Debug for AnthropicBackend<HttpWire> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicBackend")
            .field("wire", &self.wire)
            .finish_non_exhaustive()
    }
}

impl<W: Wire> AnthropicBackend<W> {
    pub fn new(profile: ModelProfile, wire: W) -> Self {
        Self { profile, wire }
    }
}

/// One tool result block.
fn tool_result_block(call_id: &str, text: &str, error: bool, images: &[super::Image]) -> Value {
    let content = if images.is_empty() {
        json!(text)
    } else {
        let mut blocks = vec![json!({"type": "text", "text": text})];
        blocks.extend(images.iter().map(image_block));
        Value::Array(blocks)
    };
    json!({"type": "tool_result", "tool_use_id": call_id, "content": content, "is_error": error})
}

fn image_block(image: &super::Image) -> Value {
    json!({"type": "image", "source": {
        "type": "base64", "media_type": image.media_type, "data": image.data,
    }})
}

/// The wire block of a part, the kept block when there is one.
fn block(part: &Part) -> Value {
    match part {
        Part::Text {
            replay: Some(kept), ..
        }
        | Part::Reasoning {
            replay: Some(kept), ..
        }
        | Part::ToolCall {
            replay: Some(kept), ..
        }
        | Part::Opaque { replay: kept } => kept.clone(),
        Part::Text { text, .. } => json!({"type": "text", "text": text}),
        Part::Reasoning { text, .. } => json!({"type": "thinking", "thinking": text}),
        Part::ToolCall {
            id, name, input, ..
        } => json!({"type": "tool_use", "id": id, "name": name, "input": input}),
        Part::ToolResult {
            call_id,
            text,
            error,
            images,
        } => tool_result_block(call_id, text, *error, images),
        Part::Image { image } => image_block(image),
        Part::Document {
            title,
            context,
            source,
        } => {
            let source = match source {
                DocumentSource::Pdf { data } => {
                    json!({"type": "base64", "media_type": "application/pdf", "data": data})
                }
                DocumentSource::Url { url } => json!({"type": "url", "url": url}),
                DocumentSource::Text { text } => {
                    json!({"type": "text", "media_type": "text/plain", "data": text})
                }
            };
            json!({"type": "document", "source": source, "title": title, "context": context})
        }
    }
}

fn wire_message(message: &Message, caching: bool) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    // Reasoning without a kept block came from elsewhere and has no signature,
    // so there is nothing to send back.
    let mut content: Vec<Value> = message
        .content
        .iter()
        .filter(|p| !matches!(p, Part::Reasoning { replay: None, .. }))
        .map(block)
        .collect();
    if caching {
        // The last document's breakpoint: every later request reads the
        // datasheets from the cache.
        if let Some(last) = content.iter_mut().rev().find(|b| b["type"] == "document") {
            last["cache_control"] = json!({"type": "ephemeral"});
        }
    }
    json!({"role": role, "content": content})
}

/// The Messages request body of one request.
pub fn request_body(profile: &ModelProfile, request: &ModelRequest<'_>) -> Value {
    let caps = &profile.capabilities;
    let mut system = json!({"type": "text", "text": request.system});
    if caps.caching {
        system["cache_control"] = json!({"type": "ephemeral"});
    }
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
        .collect();
    let messages: Vec<Value> = request
        .messages
        .iter()
        .map(|m| wire_message(m, caps.caching))
        .collect();
    let mut body = json!({
        "model": profile.model,
        "max_tokens": request.max_output_tokens,
        "system": [system],
        "tools": tools,
        "messages": messages,
    });
    if caps.adaptive_thinking {
        body["thinking"] = json!({"type": "adaptive"});
    }
    if let Some(effort) = &profile.effort {
        body["output_config"] = json!({"effort": effort});
    }
    if caps.caching {
        // Automatic caching of the growing conversation tail.
        body["cache_control"] = json!({"type": "ephemeral"});
    }
    if caps.server_fallback {
        body["fallbacks"] = json!("default");
    }
    body
}

/// The neutral part of one response block. A block the neutral fields rebuild
/// exactly carries nothing extra, so a recording stays readable.
fn parse_block(wire: &Value) -> Part {
    let text = || wire["text"].as_str().unwrap_or_default().to_owned();
    let mut part = match wire["type"].as_str() {
        Some("text") => Part::Text {
            text: text(),
            replay: None,
        },
        Some("thinking") => Part::Reasoning {
            text: wire["thinking"].as_str().unwrap_or_default().to_owned(),
            replay: Some(wire.clone()),
        },
        Some("redacted_thinking") => Part::Reasoning {
            text: String::new(),
            replay: Some(wire.clone()),
        },
        Some("tool_use") => {
            let input = wire["input"].clone();
            let malformed = (!input.is_object()).then(|| "the tool input is not an object".into());
            Part::ToolCall {
                id: wire["id"].as_str().unwrap_or_default().to_owned(),
                name: wire["name"].as_str().unwrap_or_default().to_owned(),
                input,
                malformed,
                replay: None,
            }
        }
        _ => Part::Opaque {
            replay: wire.clone(),
        },
    };
    if block(&part) != *wire {
        let kept = Some(wire.clone());
        match &mut part {
            Part::Text { replay, .. } | Part::ToolCall { replay, .. } => *replay = kept,
            _ => {}
        }
    }
    part
}

/// A Messages response in neutral form.
pub fn parse_reply(response: &Value, requested_model: &str) -> Result<ModelReply> {
    let Some(blocks) = response["content"].as_array() else {
        return Err(Error::Protocol("the response has no content".into()));
    };
    let stop = match response["stop_reason"].as_str().unwrap_or_default() {
        "tool_use" => StopReason::ToolCalls,
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "max_tokens" | "model_context_window_exceeded" => StopReason::CutOff,
        "refusal" => StopReason::Refusal {
            category: response["stop_details"]["category"]
                .as_str()
                .unwrap_or("no category")
                .to_owned(),
        },
        other => StopReason::Other {
            reason: other.to_owned(),
        },
    };
    let n = |k: &str| response["usage"][k].as_u64().unwrap_or(0);
    Ok(ModelReply {
        model: response["model"]
            .as_str()
            .unwrap_or(requested_model)
            .to_owned(),
        content: blocks.iter().map(parse_block).collect(),
        stop,
        usage: Usage {
            input_tokens: n("input_tokens"),
            output_tokens: n("output_tokens"),
            cache_write_tokens: n("cache_creation_input_tokens"),
            cache_read_tokens: n("cache_read_input_tokens"),
        },
        usage_estimated: false,
    })
}

impl<W: Wire> ModelBackend for AnthropicBackend<W> {
    fn label(&self) -> String {
        format!("Anthropic Messages at {}", self.wire.label())
    }

    fn send(&mut self, request: &ModelRequest<'_>) -> Result<ModelReply> {
        let body = request_body(&self.profile, request);
        let mut headers = vec![("anthropic-version", API_VERSION)];
        if body.get("fallbacks").is_some() {
            headers.push(("anthropic-beta", FALLBACK_BETA));
        }
        let response = self
            .wire
            .post("/v1/messages", &headers, &body, request.timeout)?;
        parse_reply(&response, &self.profile.model)
    }

    fn count_prompt_tokens(&mut self, request: &ModelRequest<'_>) -> Result<Option<u64>> {
        // The count endpoint takes the prompt only.
        let body = request_body(&self.profile, request);
        let mut prompt = json!({});
        for key in ["model", "system", "tools", "messages", "thinking"] {
            if let Some(v) = body.get(key) {
                prompt[key] = v.clone();
            }
        }
        let reply = self.wire.post(
            "/v1/messages/count_tokens",
            &[("anthropic-version", API_VERSION)],
            &prompt,
            COUNT_TIMEOUT,
        )?;
        Ok(reply["input_tokens"].as_u64())
    }
}

#[cfg(test)]
mod tests {
    use super::super::wire::ScriptedWire;
    use super::super::{Image, ToolSpec};
    use super::*;

    fn profile() -> ModelProfile {
        ModelProfile::anthropic("claude-opus-5-5")
    }

    const THINKING: &str = r#"{"type": "thinking", "thinking": "", "signature": "CAQS6g=="}"#;

    #[test]
    fn every_block_round_trips_byte_for_byte() -> Result<()> {
        let blocks: Value = serde_json::from_str(&format!(
            r#"[{THINKING},
            {{"type": "redacted_thinking", "data": "abc"}},
            {{"type": "text", "text": "Calling.", "citations": null}},
            {{"type": "text", "text": "plain"}},
            {{"type": "tool_use", "id": "t1", "name": "x", "input": {{"a": [1, 2]}}}},
            {{"type": "tool_use", "id": "t2", "name": "y", "input": {{}}, "caller": {{"type": "direct"}}}},
            {{"type": "server_tool_use", "id": "s1", "name": "z", "input": {{}}}}]"#
        ))?;
        let reply = parse_reply(
            &json!({"content": blocks, "stop_reason": "tool_use", "model": "m"}),
            "m",
        )?;
        let rebuilt: Vec<Value> = reply.content.iter().map(block).collect();
        assert_eq!(Value::Array(rebuilt), blocks);
        // Only what the neutral fields cannot rebuild is kept twice.
        assert!(matches!(&reply.content[3], Part::Text { replay: None, .. }));
        assert!(matches!(
            &reply.content[4],
            Part::ToolCall { replay: None, .. }
        ));
        assert!(matches!(
            &reply.content[0],
            Part::Reasoning {
                replay: Some(_),
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn the_neutral_recording_keeps_thinking_for_replay() -> Result<()> {
        // A thinking block survives a recording's round trip unchanged.
        let thinking: Value = serde_json::from_str(THINKING)?;
        let reply = parse_reply(
            &json!({"content": [thinking.clone()], "stop_reason": "end_turn", "model": "m"}),
            "m",
        )?;
        let line = serde_json::to_string(&reply)?;
        let back: ModelReply = serde_json::from_str(&line)?;
        assert_eq!(block(&back.content[0]), thinking);
        Ok(())
    }

    #[test]
    fn the_request_is_todays_request() -> Result<()> {
        let tools = [ToolSpec {
            name: "t".into(),
            description: "d".into(),
            input_schema: json!({"type": "object"}),
        }];
        let messages = [Message {
            role: Role::User,
            content: vec![
                Part::Document {
                    title: "a.pdf".into(),
                    context: "untrusted".into(),
                    source: DocumentSource::Pdf {
                        data: "QQ==".into(),
                    },
                },
                Part::Document {
                    title: "b".into(),
                    context: "untrusted".into(),
                    source: DocumentSource::Url {
                        url: "https://x/y.pdf".into(),
                    },
                },
                Part::text("Create it."),
            ],
        }];
        let request = ModelRequest {
            system: "S",
            tools: &tools,
            messages: &messages,
            max_output_tokens: 32_000,
            timeout: Duration::from_secs(1),
        };
        let expected = json!({
            "model": "claude-opus-5-5",
            "max_tokens": 32_000,
            "system": [{"type": "text", "text": "S", "cache_control": {"type": "ephemeral"}}],
            "tools": [{"name": "t", "description": "d", "input_schema": {"type": "object"}}],
            "messages": [{"role": "user", "content": [
                {"type": "document", "title": "a.pdf", "context": "untrusted",
                 "source": {"type": "base64", "media_type": "application/pdf", "data": "QQ=="}},
                {"type": "document", "title": "b", "context": "untrusted",
                 "source": {"type": "url", "url": "https://x/y.pdf"},
                 "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "Create it."},
            ]}],
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "high"},
            "cache_control": {"type": "ephemeral"},
            "fallbacks": "default",
        });
        assert_eq!(request_body(&profile(), &request), expected);
        // The beta header goes with the fallback, the path and version as ever.
        let mut backend = AnthropicBackend::new(
            profile(),
            ScriptedWire::new(vec![json!({"content": [], "stop_reason": "end_turn"})]),
        );
        backend.send(&request)?;
        let (path, headers, _, _) = &backend.wire.posts[0];
        assert_eq!(path, "/v1/messages");
        assert!(headers.contains(&("anthropic-version".into(), API_VERSION.into())));
        assert!(headers.contains(&("anthropic-beta".into(), FALLBACK_BETA.into())));
        Ok(())
    }

    #[test]
    fn capabilities_not_declared_are_not_sent() {
        let mut p = profile();
        p.capabilities.caching = false;
        p.capabilities.adaptive_thinking = false;
        p.capabilities.server_fallback = false;
        p.effort = None;
        let messages = [Message {
            role: Role::User,
            content: vec![Part::Document {
                title: "a".into(),
                context: "c".into(),
                source: DocumentSource::Pdf {
                    data: "QQ==".into(),
                },
            }],
        }];
        let body = request_body(
            &p,
            &ModelRequest {
                system: "S",
                tools: &[],
                messages: &messages,
                max_output_tokens: 10,
                timeout: Duration::from_secs(1),
            },
        );
        for key in ["thinking", "output_config", "cache_control", "fallbacks"] {
            assert!(body.get(key).is_none(), "{key}");
        }
        assert!(body["system"][0].get("cache_control").is_none());
        assert!(
            body["messages"][0]["content"][0]
                .get("cache_control")
                .is_none()
        );
    }

    #[test]
    fn stops_and_a_page_image_in_a_tool_result_map() -> Result<()> {
        let reply = |stop: &str| {
            parse_reply(
                &json!({"content": [], "stop_reason": stop, "stop_details": {"category": "cyber"}}),
                "m",
            )
        };
        assert_eq!(reply("max_tokens")?.stop, StopReason::CutOff);
        assert_eq!(reply("end_turn")?.stop, StopReason::EndTurn);
        assert_eq!(reply("tool_use")?.stop, StopReason::ToolCalls);
        assert_eq!(
            reply("refusal")?.stop,
            StopReason::Refusal {
                category: "cyber".into()
            }
        );
        assert!(parse_reply(&json!({"error": "x"}), "m").is_err());
        let result = block(&Part::ToolResult {
            call_id: "c".into(),
            text: "page".into(),
            error: false,
            images: vec![Image {
                media_type: "image/png".into(),
                data: "QQ==".into(),
            }],
        });
        assert_eq!(result["content"][1]["source"]["media_type"], "image/png");
        Ok(())
    }
}
