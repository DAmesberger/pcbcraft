//! OpenAI-compatible chat completions: Ollama, LM Studio, `llama-server`,
//! OpenAI, OpenRouter or any such URL.
//!
//! Servers differ, and local ones more than most, so parsing accepts what
//! they actually send and turns the rest into errors the model can act on:
//! reasoning under `reasoning`, `reasoning_content` or `thinking`, or inside
//! `<think>` tags in the content; tool arguments as a JSON string, an object
//! or malformed text; missing or repeated call ids; missing usage (estimated
//! from the characters and flagged). None of it panics, and a malformed call
//! is answered with an error and never run.
use super::wire::{HttpWire, Wire};
use super::{
    DocumentSource, Message, ModelBackend, ModelProfile, ModelReply, ModelRequest, Part, Role,
    StopReason,
};
use crate::usage::Usage;
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Rough characters per token, where a server reports no usage.
const CHARS_PER_TOKEN: u64 = 4;
/// Longest piece of malformed arguments quoted back to the model.
const MAX_QUOTED: usize = 400;

/// The chat completions backend over a wire.
pub struct OpenAiBackend<W> {
    profile: ModelProfile,
    pub wire: W,
    /// Names the output limit `max_completion_tokens`, once the server has
    /// said it wants that.
    completion_field: bool,
    /// Calls seen, for ids the server did not give.
    calls: u64,
}

impl std::fmt::Debug for OpenAiBackend<HttpWire> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiBackend")
            .field("wire", &self.wire)
            .finish_non_exhaustive()
    }
}

impl<W: Wire> OpenAiBackend<W> {
    pub fn new(profile: ModelProfile, wire: W) -> Self {
        Self {
            profile,
            wire,
            completion_field: false,
            calls: 0,
        }
    }
}

fn image_part(image: &super::Image) -> Value {
    json!({"type": "image_url", "image_url": {
        "url": format!("data:{};base64,{}", image.media_type, image.data),
    }})
}

/// One document as a content part.
fn document_part(title: &str, context: &str, source: &DocumentSource) -> Result<Value> {
    Ok(match source {
        DocumentSource::Text { text } => json!({"type": "text", "text": format!(
            "<document title={title:?} note={context:?}>\n{text}\n</document>"
        )}),
        DocumentSource::Pdf { data } => json!({"type": "file", "file": {
            "filename": title,
            "file_data": format!("data:application/pdf;base64,{data}"),
        }}),
        DocumentSource::Url { .. } => {
            return Err(Error::Unsupported(
                "this endpoint cannot fetch a datasheet by URL; give a local file".into(),
            ));
        }
    })
}

/// A user turn's wire messages: tool results first, as the protocol needs,
/// then whatever they cannot carry themselves (images), then the rest.
fn user_messages(content: &[Part], out: &mut Vec<Value>) -> Result<()> {
    let mut parts = Vec::new();
    let mut result_images = Vec::new();
    for part in content {
        match part {
            Part::ToolResult {
                call_id,
                text,
                error,
                images,
            } => {
                // The protocol has no error flag on a tool message.
                let text = if *error {
                    format!("Error: {text}")
                } else {
                    text.clone()
                };
                out.push(json!({"role": "tool", "tool_call_id": call_id, "content": text}));
                if !images.is_empty() {
                    result_images.push(json!({"type": "text", "text": format!(
                        "The image returned by tool call {call_id}:"
                    )}));
                    result_images.extend(images.iter().map(image_part));
                }
            }
            Part::Text { text, .. } => parts.push(json!({"type": "text", "text": text})),
            Part::Image { image } => parts.push(image_part(image)),
            Part::Document {
                title,
                context,
                source,
            } => parts.push(document_part(title, context, source)?),
            Part::Reasoning { .. } | Part::ToolCall { .. } | Part::Opaque { .. } => {}
        }
    }
    if !result_images.is_empty() {
        out.push(json!({"role": "user", "content": result_images}));
    }
    if !parts.is_empty() {
        out.push(json!({"role": "user", "content": parts}));
    }
    Ok(())
}

/// An assistant turn: its text and calls. Reasoning is not sent back: these
/// servers neither need nor, in some cases, accept it. A malformed call goes
/// back with empty arguments, because some servers parse arguments when they
/// render the history and fail on bad JSON; the error result carries the text.
fn assistant_message(content: &[Part]) -> Value {
    let mut text = String::new();
    let mut calls = Vec::new();
    for part in content {
        match part {
            Part::Text { text: t, .. } => text.push_str(t),
            Part::ToolCall {
                id,
                name,
                input,
                malformed,
                ..
            } => {
                let arguments = if malformed.is_some() {
                    "{}".to_owned()
                } else {
                    input.to_string()
                };
                calls.push(json!({"id": id, "type": "function",
                    "function": {"name": name, "arguments": arguments}}));
            }
            _ => {}
        }
    }
    let mut message = json!({"role": "assistant", "content": text});
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls);
    }
    message
}

fn wire_messages(system: &str, messages: &[Message]) -> Result<Vec<Value>> {
    let mut out = vec![json!({"role": "system", "content": system})];
    for message in messages {
        match message.role {
            Role::User => user_messages(&message.content, &mut out)?,
            Role::Assistant => out.push(assistant_message(&message.content)),
        }
    }
    Ok(out)
}

/// The chat completions body of one request.
pub fn request_body(
    profile: &ModelProfile,
    request: &ModelRequest<'_>,
    completion_field: bool,
) -> Result<Value> {
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|t| {
            json!({"type": "function", "function": {
                "name": t.name, "description": t.description, "parameters": t.input_schema,
            }})
        })
        .collect();
    let mut body = json!({
        "model": profile.model,
        "messages": wire_messages(request.system, request.messages)?,
        "stream": false,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    let field = if completion_field {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    body[field] = json!(request.max_output_tokens);
    if let Some(effort) = &profile.effort {
        body["reasoning_effort"] = json!(effort);
    }
    Ok(body)
}

/// A reply's text, whether a plain string or a list of parts.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// `<think>…</think>` at the start of the content, for servers that leave
/// reasoning in the text. An unclosed tag is all reasoning: the output limit
/// fell inside it, and there is no answer in it.
fn split_think(text: &str) -> (String, String) {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix("<think>") else {
        return (String::new(), text.to_owned());
    };
    match rest.split_once("</think>") {
        Some((reasoning, answer)) => (reasoning.trim().to_owned(), answer.trim_start().to_owned()),
        None => (rest.trim().to_owned(), String::new()),
    }
}

fn quoted(text: &str) -> String {
    let cut: String = text.chars().take(MAX_QUOTED).collect();
    if cut.len() < text.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

/// A call's arguments: the input, or why it cannot be used. Anything but a
/// JSON object is malformed. Empty text is an empty object, which is how
/// servers write a call with no arguments.
fn parse_arguments(arguments: &Value) -> (Value, Option<String>) {
    let not_object = |got: &str| {
        (
            Value::Null,
            Some(format!(
                "the arguments must be a JSON object, but they were {}",
                quoted(got)
            )),
        )
    };
    match arguments {
        Value::Object(_) => (arguments.clone(), None),
        Value::Null => (json!({}), None),
        Value::String(text) if text.trim().is_empty() => (json!({}), None),
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(input @ Value::Object(_)) => (input, None),
            Ok(_) => not_object(text),
            Err(e) => (
                Value::Null,
                Some(format!(
                    "the arguments were not valid JSON ({e}): {}",
                    quoted(text)
                )),
            ),
        },
        other => not_object(&other.to_string()),
    }
}

/// The neutral calls of a reply, each with a unique id.
fn parse_calls(message: &Value, counter: &mut u64) -> Vec<Part> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for call in message["tool_calls"].as_array().into_iter().flatten() {
        *counter += 1;
        let given = call["id"].as_str().unwrap_or_default();
        let id = if given.is_empty() || seen.contains(given) {
            format!("call_{counter}")
        } else {
            given.to_owned()
        };
        seen.insert(id.clone());
        let function = &call["function"];
        let name = function["name"].as_str().unwrap_or_default().to_owned();
        let (input, mut malformed) = parse_arguments(&function["arguments"]);
        if name.is_empty() {
            malformed = Some("the call names no tool".into());
        }
        out.push(Part::ToolCall {
            id,
            name,
            input,
            malformed,
            replay: None,
        });
    }
    out
}

fn estimated_tokens(chars: u64) -> u64 {
    chars.div_ceil(CHARS_PER_TOKEN)
}

/// A chat completion in neutral form. `prompt_chars` stands in for the prompt
/// tokens of a server that reports no usage.
pub fn parse_reply(
    response: &Value,
    requested_model: &str,
    prompt_chars: u64,
    counter: &mut u64,
) -> Result<ModelReply> {
    // OpenRouter and others report a failed generation inside a 200.
    if let Some(error) = response.get("error").filter(|e| !e.is_null()) {
        let message = error["message"]
            .as_str()
            .or_else(|| error.as_str())
            .unwrap_or("the server reported an error");
        return Err(Error::Api {
            status: error["code"]
                .as_u64()
                .and_then(|c| u16::try_from(c).ok())
                .unwrap_or(200),
            message: message.to_owned(),
        });
    }
    let Some(choice) = response["choices"].get(0) else {
        return Err(Error::Protocol("the response has no choices".into()));
    };
    let message = &choice["message"];
    let (think, answer) = split_think(&content_text(&message["content"]));
    let reasoning = ["reasoning", "reasoning_content", "thinking"]
        .iter()
        .find_map(|k| message[*k].as_str().filter(|s| !s.is_empty()))
        .map_or(think, str::to_owned);
    let calls = parse_calls(message, counter);
    let finish = choice["finish_reason"].as_str().unwrap_or_default();
    let refusal = message["refusal"].as_str().filter(|r| !r.is_empty());
    let stop = match (finish, calls.is_empty(), refusal) {
        ("length", ..) => StopReason::CutOff,
        ("content_filter", ..) => StopReason::Refusal {
            category: "content_filter".into(),
        },
        (_, _, Some(why)) => StopReason::Refusal {
            category: why.chars().take(200).collect(),
        },
        // Some servers say `stop` beside a tool call.
        (_, false, _) => StopReason::ToolCalls,
        ("stop" | "tool_calls" | "function_call" | "", true, _) => StopReason::EndTurn,
        (other, true, _) => StopReason::Other {
            reason: other.to_owned(),
        },
    };
    let mut content = Vec::new();
    if !reasoning.is_empty() {
        content.push(Part::Reasoning {
            text: reasoning,
            replay: None,
        });
    }
    if !answer.is_empty() {
        content.push(Part::text(answer));
    }
    content.extend(calls);
    let usage = &response["usage"];
    let reported = usage["prompt_tokens"].is_u64() || usage["completion_tokens"].is_u64();
    let usage = if reported {
        let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0);
        let cached = usage["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .unwrap_or(0)
            .min(prompt);
        Usage {
            input_tokens: prompt - cached,
            output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
            cache_write_tokens: 0,
            cache_read_tokens: cached,
        }
    } else {
        let written: u64 = content
            .iter()
            .map(|p| match p {
                Part::Reasoning { text, .. } | Part::Text { text, .. } => text.len() as u64,
                Part::ToolCall { input, .. } => input.to_string().len() as u64,
                _ => 0,
            })
            .sum();
        Usage {
            input_tokens: estimated_tokens(prompt_chars),
            output_tokens: estimated_tokens(written),
            ..Usage::default()
        }
    };
    Ok(ModelReply {
        model: response["model"]
            .as_str()
            .unwrap_or(requested_model)
            .to_owned(),
        content,
        stop,
        usage,
        usage_estimated: !reported,
    })
}

impl<W: Wire> ModelBackend for OpenAiBackend<W> {
    fn label(&self) -> String {
        format!("OpenAI-compatible endpoint {}", self.wire.label())
    }

    fn send(&mut self, request: &ModelRequest<'_>) -> Result<ModelReply> {
        let body = request_body(&self.profile, request, self.completion_field)?;
        let path = "/chat/completions";
        let response = match self.wire.post(path, &[], &body, request.timeout) {
            Err(Error::Api {
                status: 400,
                message,
            }) if !self.completion_field && message.contains("max_completion_tokens") => {
                // OpenAI's newer models take the limit under another name.
                self.completion_field = true;
                let body = request_body(&self.profile, request, true)?;
                self.wire.post(path, &[], &body, request.timeout)?
            }
            other => other?,
        };
        let prompt_chars = body["messages"].to_string().len() as u64;
        parse_reply(
            &response,
            &self.profile.model,
            prompt_chars,
            &mut self.calls,
        )
    }

    fn count_prompt_tokens(&mut self, _request: &ModelRequest<'_>) -> Result<Option<u64>> {
        // No such endpoint in this protocol.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::super::wire::ScriptedWire;
    use super::super::{Image, ToolSpec};
    use super::*;
    use std::time::Duration;

    fn profile() -> ModelProfile {
        ModelProfile::openai_compatible("local", 100_000)
    }

    fn parse(response: &Value) -> Result<ModelReply> {
        let mut counter = 0;
        parse_reply(response, "local", 4_000, &mut counter)
    }

    fn reply(message: Value, finish: &str) -> Value {
        json!({"model": "local", "choices": [{"message": message, "finish_reason": finish}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20}})
    }

    fn only_call(reply: &ModelReply) -> Option<(&Value, Option<&String>)> {
        reply.content.iter().find_map(|p| match p {
            Part::ToolCall {
                input, malformed, ..
            } => Some((input, malformed.as_ref())),
            _ => None,
        })
    }

    #[test]
    fn reasoning_arrives_under_each_name_and_is_not_an_answer() -> Result<()> {
        for key in ["reasoning", "reasoning_content", "thinking"] {
            let mut message = json!({"role": "assistant", "content": "Answer."});
            message[key] = json!("Let me think.");
            let reply = parse(&reply(message, "stop"))?;
            assert_eq!(
                reply.content,
                vec![
                    Part::Reasoning {
                        text: "Let me think.".into(),
                        replay: None
                    },
                    Part::text("Answer."),
                ],
                "{key}"
            );
        }
        // Tags in the content, closed and cut off inside.
        let got = parse(&reply(
            json!({"content": "<think>hm</think>\nAnswer."}),
            "stop",
        ))?;
        assert_eq!(got.content.len(), 2);
        assert_eq!(got.content[1], Part::text("Answer."));
        let got = parse(&reply(json!({"content": "<think>still going"}), "length"))?;
        assert_eq!(got.stop, StopReason::CutOff);
        assert!(matches!(got.content.as_slice(), [Part::Reasoning { .. }]));
        Ok(())
    }

    #[test]
    fn reasoning_is_not_sent_back() -> Result<()> {
        let messages = [Message {
            role: Role::Assistant,
            content: vec![
                Part::Reasoning {
                    text: "secret plan".into(),
                    replay: None,
                },
                Part::text("Done."),
            ],
        }];
        let wire = wire_messages("S", &messages)?;
        assert_eq!(wire[1], json!({"role": "assistant", "content": "Done."}));
        Ok(())
    }

    #[test]
    fn malformed_arguments_become_an_error_never_a_panic() -> Result<()> {
        let call = |arguments: Value| json!({"content": "", "tool_calls": [{"id": "c1", "function": {"name": "t", "arguments": arguments}}]});
        for bad in [
            json!("{\"a\": 1"),
            json!("not json at all"),
            json!("[1, 2]"),
            json!("3"),
            json!(3),
            json!(true),
            json!([1]),
        ] {
            let got = parse(&reply(call(bad.clone()), "tool_calls"))?;
            let (input, malformed) =
                only_call(&got).ok_or_else(|| Error::Protocol("a call".into()))?;
            assert!(malformed.is_some(), "{bad}");
            assert!(input.is_null(), "{bad}");
        }
        // A string of JSON, a bare object, nothing at all and an empty string.
        for (good, expected) in [
            (json!("{\"a\": 1}"), json!({"a": 1})),
            (json!({"a": 1}), json!({"a": 1})),
            (Value::Null, json!({})),
            (json!(""), json!({})),
        ] {
            let got = parse(&reply(call(good.clone()), "tool_calls"))?;
            let (input, malformed) =
                only_call(&got).ok_or_else(|| Error::Protocol("a call".into()))?;
            assert!(malformed.is_none(), "{good}");
            assert_eq!(input, &expected);
        }
        // No name is malformed too, and the quoted text is bounded.
        let long = "x".repeat(10_000);
        let got = parse(&reply(
            json!({"tool_calls": [{"function": {"arguments": long}}]}),
            "stop",
        ))?;
        let (_, malformed) = only_call(&got).ok_or_else(|| Error::Protocol("a call".into()))?;
        assert!(malformed.is_some_and(|m| m.len() < 1000));
        Ok(())
    }

    #[test]
    fn a_malformed_call_goes_back_with_empty_arguments() -> Result<()> {
        let messages = [Message {
            role: Role::Assistant,
            content: vec![Part::ToolCall {
                id: "c1".into(),
                name: "t".into(),
                input: Value::Null,
                malformed: Some("bad".into()),
                replay: None,
            }],
        }];
        let wire = wire_messages("S", &messages)?;
        assert_eq!(wire[1]["tool_calls"][0]["function"]["arguments"], "{}");
        Ok(())
    }

    #[test]
    fn missing_and_repeated_ids_are_made_unique() -> Result<()> {
        let got = parse(&reply(
            json!({"tool_calls": [
                {"function": {"name": "a", "arguments": "{}"}},
                {"id": "x", "function": {"name": "b", "arguments": "{}"}},
                {"id": "x", "function": {"name": "c", "arguments": "{}"}},
            ]}),
            "tool_calls",
        ))?;
        let ids: BTreeSet<&String> = got
            .content
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 3, "{got:?}");
        Ok(())
    }

    #[test]
    fn finish_reasons_map_and_length_wins_over_a_partial_call() -> Result<()> {
        let call =
            json!({"tool_calls": [{"id": "c", "function": {"name": "t", "arguments": "{\"a\""}}]});
        assert_eq!(
            parse(&reply(call.clone(), "length"))?.stop,
            StopReason::CutOff
        );
        assert_eq!(
            parse(&reply(call, "tool_calls"))?.stop,
            StopReason::ToolCalls
        );
        let call =
            json!({"tool_calls": [{"id": "c", "function": {"name": "t", "arguments": "{}"}}]});
        // Ollama's stop beside a call.
        assert_eq!(parse(&reply(call, "stop"))?.stop, StopReason::ToolCalls);
        let text = json!({"content": "x"});
        assert_eq!(
            parse(&reply(text.clone(), "stop"))?.stop,
            StopReason::EndTurn
        );
        assert_eq!(
            parse(&reply(text.clone(), "content_filter"))?.stop,
            StopReason::Refusal {
                category: "content_filter".into()
            }
        );
        assert_eq!(
            parse(&reply(text, "odd"))?.stop,
            StopReason::Other {
                reason: "odd".into()
            }
        );
        let refused = json!({"content": null, "refusal": "I cannot."});
        assert!(matches!(
            parse(&reply(refused, "stop"))?.stop,
            StopReason::Refusal { .. }
        ));
        Ok(())
    }

    #[test]
    fn a_bad_response_is_an_error_not_a_panic() {
        assert!(parse(&json!({})).is_err());
        assert!(parse(&json!({"choices": []})).is_err());
        assert!(parse(&json!({"choices": null})).is_err());
        let e = parse(&json!({"error": {"message": "overloaded", "code": 503}}));
        assert!(matches!(e, Err(Error::Api { status: 503, .. })), "{e:?}");
        let e = parse(&json!({"error": "gone"}));
        assert!(matches!(e, Err(Error::Api { .. })), "{e:?}");
    }

    #[test]
    fn usage_is_read_with_its_cached_tokens_or_estimated_and_flagged() -> Result<()> {
        let got = parse(
            &json!({"choices": [{"message": {"content": "x"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20,
                "prompt_tokens_details": {"cached_tokens": 60}}}),
        )?;
        assert_eq!(got.usage.input_tokens, 40);
        assert_eq!(got.usage.cache_read_tokens, 60);
        assert_eq!(got.usage.output_tokens, 20);
        assert!(!got.usage_estimated);
        let got = parse(
            &json!({"choices": [{"message": {"content": "12345678"}, "finish_reason": "stop"}]}),
        )?;
        assert!(got.usage_estimated);
        assert_eq!(got.usage.input_tokens, 1_000, "4 000 prompt characters");
        assert_eq!(got.usage.output_tokens, 2, "8 reply characters");
        Ok(())
    }

    #[test]
    fn the_request_carries_tools_results_and_page_images_in_protocol_order() -> Result<()> {
        let tools = [ToolSpec {
            name: "t".into(),
            description: "d".into(),
            input_schema: json!({"type": "object"}),
        }];
        let image = Image {
            media_type: "image/png".into(),
            data: "QQ==".into(),
        };
        let messages = [
            Message {
                role: Role::User,
                content: vec![
                    Part::Document {
                        title: "a.pdf".into(),
                        context: "untrusted".into(),
                        source: DocumentSource::Text {
                            text: "pin 1".into(),
                        },
                    },
                    Part::text("Go."),
                ],
            },
            Message {
                role: Role::Assistant,
                content: vec![Part::ToolCall {
                    id: "c1".into(),
                    name: "t".into(),
                    input: json!({"a": 1}),
                    malformed: None,
                    replay: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![
                    Part::ToolResult {
                        call_id: "c1".into(),
                        text: "refused".into(),
                        error: true,
                        images: vec![image],
                    },
                    Part::text("Next."),
                ],
            },
        ];
        let request = ModelRequest {
            system: "S",
            tools: &tools,
            messages: &messages,
            max_output_tokens: 777,
            timeout: Duration::from_secs(1),
        };
        let body = request_body(&profile(), &request, false)?;
        assert_eq!(body["max_tokens"], 777);
        assert_eq!(body["stream"], false);
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
        let wire = body["messages"]
            .as_array()
            .ok_or(Error::Protocol("m".into()))?;
        assert_eq!(wire[0]["role"], "system");
        let first = wire[1]["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            first.contains("pin 1") && first.contains("untrusted"),
            "{first}"
        );
        assert_eq!(
            wire[2]["tool_calls"][0]["function"]["arguments"],
            "{\"a\":1}"
        );
        assert_eq!(wire[3]["role"], "tool");
        assert_eq!(wire[3]["tool_call_id"], "c1");
        assert_eq!(wire[3]["content"], "Error: refused");
        // A tool message carries text only: the image follows it, before
        // the turn's other text.
        assert_eq!(wire[4]["role"], "user");
        assert_eq!(
            wire[4]["content"][1]["image_url"]["url"],
            "data:image/png;base64,QQ=="
        );
        assert_eq!(wire[5]["content"][0]["text"], "Next.");
        Ok(())
    }

    #[test]
    fn a_url_document_says_why_it_cannot_be_sent() {
        let messages = [Message {
            role: Role::User,
            content: vec![Part::Document {
                title: "x".into(),
                context: "c".into(),
                source: DocumentSource::Url {
                    url: "https://x/y.pdf".into(),
                },
            }],
        }];
        assert!(matches!(
            wire_messages("S", &messages),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn the_output_limit_moves_to_its_other_name_when_the_server_asks() -> Result<()> {
        let mut wire = ScriptedWire::default();
        wire.replies.push_back(Err(Error::Api {
            status: 400,
            message: "Use max_completion_tokens instead.".into(),
        }));
        wire.replies
            .push_back(Ok(reply(json!({"content": "ok"}), "stop")));
        let mut backend = OpenAiBackend::new(profile(), wire);
        let request = ModelRequest {
            system: "S",
            tools: &[],
            messages: &[],
            max_output_tokens: 777,
            timeout: Duration::from_secs(1),
        };
        backend.send(&request)?;
        let (path, _, first, _) = &backend.wire.posts[0];
        assert_eq!(path, "/chat/completions");
        assert_eq!(first["max_tokens"], 777);
        let (_, _, second, _) = &backend.wire.posts[1];
        assert_eq!(second["max_completion_tokens"], 777);
        assert!(second.get("max_tokens").is_none());
        Ok(())
    }
}
