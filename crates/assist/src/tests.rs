//! The tool loop against recorded responses of both backends (no network): an original
//! synthetic radio module, EXR-22, authored end to end through the tools,
//! and the caps, cancellation and refusal paths.
use super::*;
use crate::cost::PriceTable;
use crate::documents::Delivery;
use crate::model::anthropic::AnthropicBackend;
use crate::model::openai::OpenAiBackend;
use crate::model::wire::ScriptedWire;
use crate::model::{ModelBackend, ModelProfile};
use crate::tools::ToolOutcome;
use pcbcraft_automation::{Mode, Policy};
use serde_json::Value;
use std::sync::atomic::AtomicBool;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// EXR-22: 8 + 6 + 8 castellated pads at 1.27 mm and a 2 × 2 ground array
/// (terminal 23). Invented for this test.
fn pinout() -> Value {
    let mut pins = Vec::new();
    for n in 1..=22u32 {
        let (name, functions, default) = match n {
            1 | 22 => (
                format!("GND{n}"),
                json!([{"label": "gnd", "direction": "power_input", "signal": "power"}]),
                "gnd",
            ),
            2 => (
                "VCC".to_owned(),
                json!([{"label": "vcc", "direction": "power_input", "signal": "power"}]),
                "vcc",
            ),
            _ => {
                let g = n - 3;
                let mut f = vec![
                    json!({"label": "gpio_in", "direction": "input", "signal": "digital"}),
                    json!({"label": "gpio_out", "direction": "output", "signal": "digital", "drive": "push_pull"}),
                ];
                if g == 0 {
                    f.push(json!({"label": "uart0_tx", "direction": "output", "signal": "digital", "drive": "push_pull", "peripheral": "UART0", "role": "tx", "mux": "F1"}));
                }
                (format!("IO{g}"), json!(f), "gpio_in")
            }
        };
        pins.push(
            json!({"row": format!("r{n}"), "number": {"MOD": n.to_string()}, "name": name,
            "class": "electrical", "functions": functions, "default": default,
            "source": {"page": 3, "table": "Table 1"}}),
        );
    }
    pins.push(
        json!({"row": "r23", "number": {"MOD": "23"}, "name": "EPAD", "class": "electrical",
        "functions": [{"label": "gnd", "direction": "power_input", "signal": "power"}],
        "default": "gnd", "source": {"page": 9, "table": "Figure 4"}}),
    );
    json!({
        "format": 1,
        "device": {"name": "EXR-22 radio module", "reference": {"title": "EXR-22 datasheet (fictitious)", "document_revision": "1"}},
        "variants": [{"name": "MOD", "terminal_count": 23}],
        "peripherals": [{"label": "UART0", "kind": "uart", "roles": [{"label": "tx", "signal": "TX", "required_when_used": true}]}],
        "pins": pins,
        "rules": [{"label": "io0_strap", "script": "if active(\"IO0\", \"uart0_tx\") { warn(\"IO0 selects the boot mode at reset\", [\"IO0\"]) } else { ok() }", "source": {"page": 5}}],
    })
}

fn module_values() -> Value {
    json!({"family": "module", "width": 14.0, "length": 20.0, "height": 2.8, "pcb_thickness": 0.8,
        "antenna_length": 5.0, "can_size": [12.0, 12.4], "can_centre": [0.0, -2.2],
        "pitch": 1.27, "pad_size": [1.0, 0.8],
        "rows": [
            {"side": "left", "count": 8, "first": [-6.5, 2.2]},
            {"side": "bottom", "count": 6, "first": [-3.175, -9.5]},
            {"side": "right", "count": 8, "first": [6.5, 2.2 - 7.0 * 1.27]},
        ],
        "castellation": 0.5, "land_width": 0.8, "land_extend": [0.4, 0.0],
        "ground": {"rows": 2, "cols": 2, "pitch": [2.0, 2.0], "size": [1.2, 1.2], "centre": [0.0, -3.0], "number": "23"},
        "keepout": {"beyond": [0.0, 0.0, 0.0]}})
}

fn tool_turn(id: &str, name: &str, input: Value) -> Value {
    json!({"id": format!("msg_{id}"), "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [{"type": "text", "text": format!("Calling {name}.")},
                    {"type": "tool_use", "id": id, "name": name, "input": input}],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 500, "output_tokens": 1_000, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 50_000}})
}

/// A tool call under the default policy (edits run, no folders).
fn execute_open(
    session: &mut Session,
    workspace: &mut tools::Workspace,
    name: &str,
    input: &Value,
) -> ToolOutcome {
    tools::execute(session, &Policy::default(), workspace, name, input)
}

fn reply_json(outcome: ToolOutcome) -> Fallible<Value> {
    match outcome {
        ToolOutcome::Reply {
            text, error: false, ..
        } => Ok(serde_json::from_str(&text)?),
        other => Err(format!("unexpected tool outcome {other:?}").into()),
    }
}

/// The recorded responses of a complete run on an empty project.
fn script() -> Fallible<Vec<Value>> {
    script_on(Session::default())
}

/// The recorded responses of a complete run, made against a shadow
/// session (a copy of the project the run will work on) so the inputs
/// carry the real plan base and draft IDs.
fn script_on(mut shadow: Session) -> Fallible<Vec<Value>> {
    let target = json!({"target": "new", "name": "EXR-22"});
    let doc = pinout();
    let mut workspace = tools::Workspace::default();
    let mut out = vec![tool_turn("t1", "library_pinout_schema", json!({}))];
    // The document in two chunks: header, peripherals and the first rows;
    // then the remaining rows and the rules.
    let pins = doc["pins"].as_array().ok_or("pins")?;
    let (first, rest) = pins.split_at(12);
    let chunks = [
        json!({"format": doc["format"], "device": doc["device"], "variants": doc["variants"],
            "peripherals": doc["peripherals"], "pins": first}),
        json!({"pins": rest, "rules": doc["rules"]}),
    ];
    for (k, chunk) in chunks.into_iter().enumerate() {
        reply_json(execute_open(
            &mut shadow,
            &mut workspace,
            "library_pinout_edit",
            &chunk,
        ))?;
        out.push(tool_turn(&format!("t2{k}"), "library_pinout_edit", chunk));
    }
    let preview_input = json!({"target": target});
    let plan = reply_json(execute_open(
        &mut shadow,
        &mut workspace,
        "library_pinout_preview",
        &preview_input,
    ))?;
    out.push(tool_turn("t2", "library_pinout_preview", preview_input));
    let apply_input = json!({"base": plan["base"]});
    let applied = reply_json(execute_open(
        &mut shadow,
        &mut workspace,
        "library_pinout_apply",
        &apply_input,
    ))?;
    out.push(tool_turn("t3", "library_pinout_apply", apply_input));
    let id_of = |kind: &str| {
        applied["drafts"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|d| {
                d["kind"] == kind && d["name"].as_str().is_some_and(|n| n.starts_with("EXR-22"))
            })
            .map(|d| d["id"].clone())
            .ok_or_else(|| format!("no {kind} draft: {applied}"))
    };
    let package = id_of("package")?;
    let component = id_of("component")?;
    let request = json!({"request": {"draft": package, "row": {"source": "custom", "family": "module"}, "values": module_values()}});
    reply_json(execute_open(
        &mut shadow,
        &mut workspace,
        "library_package_plan",
        &request,
    ))?;
    out.push(tool_turn("t4", "library_package_plan", request.clone()));
    out.push(tool_turn("t5", "library_package_from_template", request));
    out.push(tool_turn(
        "t6",
        "library_set_marking",
        json!({"draft": component,
        "marking": {"items": [{"item": "text", "template": "EXR-22"}]}}),
    ));
    out.push(tool_turn("t7", "component_check_drafts", json!({})));
    out.push(tool_turn("t8", "submit_review", json!({"report": {
        "part": "EXR-22", "summary": "Synthetic module.",
        "sources": [{"name": "DS", "title": "EXR-22 datasheet", "revision": "1"}],
        "values": [{"item": "pitch", "value": "1.27 mm", "source": {"document": "DS", "page": 9, "location": "Figure 4"}, "confidence": "high"}],
        "open_questions": ["Ground array optional?"]}})));
    Ok(out)
}

fn datasheet(dir: &tempfile::TempDir) -> Fallible<Datasheet> {
    let path = dir.path().join("exr22.pdf");
    std::fs::write(
        &path,
        b"%PDF-1.4\n1 0 obj << /Type /Pages >> endobj\n2 0 obj << /Type /Page >> endobj\n3 0 obj <</Type/Page>> endobj\n",
    )?;
    Ok(Datasheet::File(path))
}

fn goal(sheet: Datasheet) -> Goal {
    Goal {
        name: "EXR-22".into(),
        datasheets: vec![sheet],
        notes: String::new(),
    }
}

/// The Messages backend answering `responses` (raw API JSON) in order, with
/// a token count for the first request.
fn anthropic(responses: Vec<Value>) -> AnthropicBackend<ScriptedWire> {
    let mut wire = ScriptedWire::new(responses);
    wire.count = Some(60_000);
    AnthropicBackend::new(RunConfig::default().profile, wire)
}

/// The bodies the Messages backend posted.
fn bodies(backend: &AnthropicBackend<ScriptedWire>) -> Vec<&Value> {
    backend.wire.posts.iter().map(|(_, _, b, _)| b).collect()
}

fn run_with(
    config: &RunConfig,
    backend: &mut dyn ModelBackend,
    sheet: Datasheet,
    session: &mut Session,
) -> Fallible<(Outcome, Vec<Event>)> {
    let mut log = Vec::new();
    let outcome = run(
        &goal(sheet),
        config,
        backend,
        session,
        &mut |e| log.push(e),
        &AtomicBool::new(false),
    )?;
    Ok((outcome, log))
}

#[test]
fn a_recorded_run_authors_the_module_and_submits_its_review() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut backend = anthropic(script()?);
    let mut session = Session::default();
    let (outcome, log) = run_with(
        &RunConfig::default(),
        &mut backend,
        datasheet(&dir)?,
        &mut session,
    )?;
    assert_eq!(outcome.stop, Stop::Submitted, "{log:?}");
    assert!(
        log.iter()
            .all(|e| !matches!(e, Event::Tool { error: true, .. })),
        "every tool call succeeded: {log:?}"
    );
    let report = outcome.report.ok_or("a report")?;
    assert_eq!(report.part, "EXR-22");
    // Ten responses at 500 in, 1000 out and 50 000 cache reads each:
    // 10 × (0.002 + 0.02 + 0.01) = 0.32 USD.
    assert!(
        (outcome.facts.spent_usd - 0.32).abs() < 1e-9,
        "{}",
        outcome.facts.spent_usd
    );
    assert_eq!(outcome.facts.requests, 10);
    assert_eq!(outcome.facts.request_log.len(), 10);
    assert_eq!(
        outcome.facts.request_log[1].tools,
        vec!["library_pinout_edit".to_owned()]
    );
    assert_eq!(outcome.facts.request_log[1].stop_reason, "tool_calls");
    assert_eq!(outcome.facts.cap_usd, Some(DEFAULT_MAX_USD));
    assert_eq!(outcome.facts.documents[0].delivery, Delivery::NativePdf);
    // The review lists every pin by package number and the rule's checks.
    assert_eq!(outcome.facts.pins.len(), 23);
    assert_eq!(outcome.facts.pins[0].number, "1");
    assert_eq!(outcome.facts.rules.len(), 1);
    assert!(outcome.facts.rules[0].failures.is_empty());

    // The drafts: component, symbol, package, footprint and binding.
    let drafts = &session.engine().drafts().items;
    let footprint = drafts
        .iter()
        .find_map(|d| match &d.definition {
            pcbcraft_core::library::Definition::Footprint(f) => Some(f),
            _ => None,
        })
        .ok_or("a footprint draft")?;
    assert_eq!(
        footprint.pads.len(),
        22 + 4,
        "edge lands and the ground array"
    );
    assert_eq!(footprint.keepouts.len(), 1, "the antenna keep-out");
    let pin1 = footprint
        .pads
        .iter()
        .find(|p| p.number == "1")
        .ok_or("pad 1")?;
    // The 1.4 mm land reaches 0.4 beyond the −x edge at −7: outer end −7.4,
    // centre −7.4 + 0.7.
    assert!(
        (pin1.position[0] - (-7.4 + 0.7)).abs() < 1e-9,
        "{:?}",
        pin1.position
    );
    let component = drafts
        .iter()
        .find_map(|d| match &d.definition {
            pcbcraft_core::library::Definition::Component(c) => Some(c),
            _ => None,
        })
        .ok_or("a component draft")?;
    assert!(component.marking.is_some(), "the can marking");
    assert!(
        component
            .review
            .as_ref()
            .is_some_and(|r| r.unreviewed.len() == 23),
        "every imported row waits for review"
    );
    assert!(
        session.engine().project().library.components.is_empty(),
        "nothing published"
    );

    // The first request: datasheet cached, untrusted, tools, adaptive
    // thinking, fallback; no key anywhere in a request.
    let bodies = bodies(&backend);
    let first = bodies.first().ok_or("a request")?;
    let doc = &first["messages"][0]["content"][0];
    assert_eq!(doc["type"], "document");
    assert_eq!(doc["cache_control"]["type"], "ephemeral");
    assert!(
        doc["context"]
            .as_str()
            .is_some_and(|c| c.contains("untrusted"))
    );
    assert_eq!(first["thinking"]["type"], "adaptive");
    assert_eq!(first["fallbacks"], "default");
    assert_eq!(first["max_tokens"], 32_000);
    assert_eq!(
        first["tools"].as_array().map(Vec::len),
        Some(tools::definitions().len())
    );
    // Each later request carries the previous turn's assistant content
    // unchanged and one user message of tool results.
    let last = bodies.last().ok_or("a request")?;
    assert_eq!(last["messages"].as_array().map(Vec::len), Some(1 + 2 * 9));
    for pair in bodies.windows(2) {
        let (a, b) = (
            pair[0]["messages"].as_array().ok_or("messages")?,
            pair[1]["messages"].as_array().ok_or("messages")?,
        );
        assert_eq!(&b[..a.len()], &a[..], "append-only");
    }
    assert_eq!(
        last["messages"][3]["content"],
        script()?[1]["content"],
        "assistant content goes back as it came"
    );
    // The model never sent the document again to apply it.
    let apply = bodies
        .iter()
        .flat_map(|r| r["messages"].as_array().cloned().unwrap_or_default())
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .find(|b| b["name"] == "library_pinout_apply")
        .ok_or("the apply call")?;
    assert!(apply["input"].get("document").is_none());
    Ok(())
}

#[test]
fn the_cost_cap_stops_before_a_request_it_could_not_pay() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut expensive = tool_turn("t1", "library_pinout_schema", json!({}));
    // One million cache-write tokens: 5 USD at the default rates.
    expensive["usage"]["cache_creation_input_tokens"] = json!(1_000_000);
    let mut backend = anthropic(vec![
        expensive,
        tool_turn("t2", "component_check_drafts", json!({})),
    ]);
    backend.wire.count = Some(10_000);
    let mut config = RunConfig::default();
    config.budget.max_usd = 5.5;
    let (outcome, _) = run_with(
        &config,
        &mut backend,
        datasheet(&dir)?,
        &mut Session::default(),
    )?;
    assert!(
        matches!(outcome.stop, Stop::Budget(_)),
        "{:?}",
        outcome.stop
    );
    assert_eq!(
        backend.wire.posts.len(),
        1,
        "the second request was never sent"
    );
    assert!(outcome.facts.spent_usd <= 5.5);
    Ok(())
}

#[test]
fn a_cancelled_run_sends_nothing_and_a_refusal_stops() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut backend = anthropic(vec![]);
    let outcome = run(
        &goal(datasheet(&dir)?),
        &RunConfig::default(),
        &mut backend,
        &mut Session::default(),
        &mut |_| {},
        &AtomicBool::new(true),
    )?;
    assert_eq!(outcome.stop, Stop::Cancelled);
    assert!(backend.wire.posts.is_empty());

    let refusal = json!({"id": "m", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [], "stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber"},
        "usage": {"input_tokens": 10, "output_tokens": 0}});
    let mut backend = anthropic(vec![refusal]);
    let (outcome, _) = run_with(
        &RunConfig::default(),
        &mut backend,
        datasheet(&dir)?,
        &mut Session::default(),
    )?;
    assert_eq!(outcome.stop, Stop::Refused("cyber".into()));
    Ok(())
}

#[test]
fn a_tool_call_cut_off_at_max_tokens_is_not_run() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut cut = tool_turn("t1", "library_set_marking", json!({"draft": 1}));
    cut["stop_reason"] = json!("max_tokens");
    let mut backend = anthropic(vec![cut]);
    let mut session = Session::default();
    let before = session.engine().revision();
    let (outcome, _) = run_with(
        &RunConfig::default(),
        &mut backend,
        datasheet(&dir)?,
        &mut session,
    )?;
    // The recording ends after the cut-off turn.
    assert!(matches!(outcome.stop, Stop::Failed(_)));
    assert_eq!(session.engine().revision(), before);
    let second = bodies(&backend)
        .get(1)
        .copied()
        .ok_or("the follow-up request")?;
    let result = &second["messages"][2]["content"][0];
    assert_eq!(result["is_error"], true);
    // The request after a cut-off has room to finish.
    assert_eq!(second["max_tokens"], 64_000);
    Ok(())
}

#[test]
fn pages_are_counted_for_the_estimate() -> Fallible {
    let dir = tempfile::tempdir()?;
    let Datasheet::File(path) = datasheet(&dir)? else {
        return Err("a file".into());
    };
    assert_eq!(
        pdf_pages(&std::fs::read(&path)?),
        2,
        "two pages, not the page tree"
    );
    let prepared = documents::prepare(
        &[Datasheet::File(path)],
        &RunConfig::default().profile.capabilities,
    )?;
    assert_eq!(prepared.tokens, 2 * TOKENS_PER_PAGE);
    Ok(())
}

// The gate: every tool call's request passes it with the run's policy.

#[test]
fn publishing_and_paths_outside_the_output_folder_are_refused_by_the_gate() -> Fallible {
    let dir = tempfile::tempdir()?;
    let policy = output_policy(&dir.path().join("part.pcbcraft"))?;
    let mut session = Session::default();
    let before = session.engine().revision();
    // Publishing needs a person, and a run has no one to ask.
    let reply = tools::send(
        &mut session,
        &policy,
        json!({"command": "project.apply", "params": {"expected_revision": before,
            "operation": {"editor": "Library", "operation": {
                "command": "library.publish", "params": {"drafts": []}}}}}),
    );
    assert_eq!(reply["error"]["code"], "gate.cannot_ask", "{reply}");
    // A file outside the run's folder is refused before anything is written.
    let outside = tempfile::tempdir()?;
    let target = outside.path().join("elsewhere.pcbcraft");
    let reply = tools::send(
        &mut session,
        &policy,
        json!({"command": "project.save", "params": {"path": target.to_string_lossy()}}),
    );
    assert_eq!(reply["error"]["code"], "gate.path_outside", "{reply}");
    assert!(!target.exists());
    assert_eq!(session.engine().revision(), before);
    // Queries run.
    let reply = tools::send(
        &mut session,
        &policy,
        json!({"command": "library.pinout_schema", "params": {}}),
    );
    assert_eq!(reply["ok"], true);
    Ok(())
}

#[test]
fn a_gated_tool_call_comes_back_as_a_tool_result_and_the_run_goes_on() -> Fallible {
    // Under ask-every-time, applying the pinout is an edit that needs a
    // person; with no one to ask it is refused, and the model reads why.
    let dir = tempfile::tempdir()?;
    // The whole document in one call: 23 rows, under the 40-row limit.
    let script = vec![
        tool_turn("t1", "library_pinout_edit", pinout()),
        tool_turn(
            "t2",
            "library_pinout_preview",
            json!({"target": {"target": "new", "name": "EXR-22"}}),
        ),
        tool_turn("t3", "library_pinout_apply", json!({})),
        tool_turn(
            "t4",
            "submit_review",
            json!({"report": {
            "part": "EXR-22", "summary": "Not applied: the gate refused it.",
            "sources": [{"name": "DS", "title": "EXR-22 datasheet", "revision": "1"}],
            "values": [], "open_questions": ["Apply the pinout by hand."]}}),
        ),
    ];
    let mut backend = anthropic(script);
    let config = RunConfig {
        policy: Policy::new(Mode::AskEveryTime, [dir.path().to_path_buf()]),
        ..RunConfig::default()
    };
    let mut session = Session::default();
    let (outcome, log) = run_with(&config, &mut backend, datasheet(&dir)?, &mut session)?;
    assert_eq!(outcome.stop, Stop::Submitted, "{log:?}");
    assert_eq!(
        outcome.facts.request_log[2].tools,
        vec!["library_pinout_apply!"]
    );
    assert!(
        session.engine().drafts().items.is_empty(),
        "nothing applied"
    );
    let bodies = bodies(&backend);
    let refused = &bodies[3]["messages"][6]["content"][0];
    assert_eq!(refused["type"], "tool_result");
    assert_eq!(refused["is_error"], true);
    let text = refused["content"].as_str().unwrap_or_default();
    assert!(
        text.contains("gate.cannot_ask") && text.contains("no way to ask"),
        "{text}"
    );
    Ok(())
}

#[test]
fn the_pinout_applies_without_a_base_or_with_it_as_text() -> Fallible {
    // A local model could not copy a 19-digit base: it wrote it as a float.
    // The base is optional, text is read, and a float is named as the cause.
    for (base, applies) in [(None, true), (Some("text"), true), (Some("float"), false)] {
        let mut session = Session::default();
        let mut workspace = tools::Workspace::default();
        reply_json(execute_open(
            &mut session,
            &mut workspace,
            "library_pinout_edit",
            &pinout(),
        ))?;
        let plan = reply_json(execute_open(
            &mut session,
            &mut workspace,
            "library_pinout_preview",
            &json!({"target": {"target": "new", "name": "EXR-22"}}),
        ))?;
        let exact = plan["base"].as_u64().ok_or("a base")?;
        let input = match base {
            None => json!({}),
            Some("text") => json!({"base": exact.to_string()}),
            _ => json!({"base": exact as f64}),
        };
        let outcome = execute_open(&mut session, &mut workspace, "library_pinout_apply", &input);
        match outcome {
            ToolOutcome::Reply { error, text, .. } => {
                assert_eq!(!error, applies, "{base:?}: {text}");
                if !applies && exact > (1u64 << 53) {
                    assert!(text.contains("Leave base out"), "{text}");
                }
            }
            ToolOutcome::Report(_) => return Err("a reply".into()),
        }
    }
    Ok(())
}

#[test]
fn non_pdf_files_and_models_without_prices_are_refused() -> Fallible {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("x.pdf");
    std::fs::write(&path, b"not a pdf")?;
    let caps = RunConfig::default().profile.capabilities;
    assert!(documents::prepare(&[Datasheet::File(path)], &caps).is_err());
    assert!(documents::prepare(&[Datasheet::Url("http://x/y.pdf".into())], &caps).is_err());
    let config = RunConfig {
        profile: ModelProfile::anthropic("unpriced", &PriceTable::default()),
        ..RunConfig::default()
    };
    let result = run(
        &goal(datasheet(&dir)?),
        &config,
        &mut anthropic(vec![]),
        &mut Session::default(),
        &mut |_| {},
        &AtomicBool::new(false),
    );
    assert!(matches!(result, Err(Error::Config(_))));
    Ok(())
}

// The same run on an OpenAI-compatible endpoint: a local model, its
// reasoning, a malformed call, extracted text and a rendered page.

/// A chat completion carrying one tool call, as Ollama sends it.
fn chat_call(id: &str, name: &str, arguments: &str) -> Value {
    json!({"id": "chatcmpl-1", "object": "chat.completion", "model": "local-27b",
        "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": "", "reasoning": format!("I should call {name}."),
            "tool_calls": [{"id": id, "type": "function",
                "function": {"name": name, "arguments": arguments}}]}}],
        "usage": {"prompt_tokens": 20_000, "completion_tokens": 800}})
}

/// The Messages script, as chat completions.
fn chat_script(responses: &[Value]) -> Vec<Value> {
    responses
        .iter()
        .flat_map(|r| r["content"].as_array().cloned().unwrap_or_default())
        .filter(|b| b["type"] == "tool_use")
        .map(|b| {
            chat_call(
                b["id"].as_str().unwrap_or_default(),
                b["name"].as_str().unwrap_or_default(),
                &b["input"].to_string(),
            )
        })
        .collect()
}

fn local_config(context: u64) -> RunConfig {
    let mut profile = ModelProfile::openai_compatible("local-27b", context);
    profile.capabilities.vision = true;
    RunConfig {
        profile,
        ..RunConfig::default()
    }
}

fn openai(responses: Vec<Value>, config: &RunConfig) -> OpenAiBackend<ScriptedWire> {
    OpenAiBackend::new(config.profile.clone(), ScriptedWire::new(responses))
}

/// A text datasheet: a pin table on page 1 and a drawing on page 2.
fn text_datasheet(dir: &tempfile::TempDir) -> Fallible<Datasheet> {
    let path = dir.path().join("exr22.pdf");
    let table = format!(
        "EXR-22 PIN TABLE {}",
        "PIN 1 GND PIN 2 VCC PIN 3 IO0 ".repeat(12)
    );
    std::fs::write(&path, documents::tests::text_pdf(&[&table, ""]))?;
    Ok(Datasheet::File(path))
}

#[test]
fn a_local_model_authors_the_module_from_text_and_a_page_view() -> Fallible {
    let dir = tempfile::tempdir()?;
    let config = local_config(131_072);
    let mut replies = vec![
        // A look at the drawing page first, then one malformed call that the
        // model sends again.
        chat_call(
            "v1",
            "datasheet_view_page",
            r#"{"document": "exr22.pdf", "page": 2}"#,
        ),
        chat_call("bad", "library_pinout_schema", r#"{"oops": "#),
    ];
    replies.extend(chat_script(&script()?));
    let mut backend = openai(replies, &config);
    let mut session = Session::default();
    let (outcome, log) = run_with(&config, &mut backend, text_datasheet(&dir)?, &mut session)?;
    assert_eq!(outcome.stop, Stop::Submitted, "{log:?}");
    assert_eq!(outcome.report.ok_or("a report")?.part, "EXR-22");
    let f = &outcome.facts;
    assert_eq!(f.requests, 12);
    assert_eq!(f.request_log[1].tools, vec!["library_pinout_schema!"]);
    assert_eq!(f.cap_usd, None, "no price: spend not tracked");
    assert!(f.spent_usd.abs() < 1e-12);
    assert_eq!(f.output_tokens, 12 * 800);
    assert_eq!(f.documents[0].delivery, Delivery::TextAndPageViews);
    assert_eq!(f.documents[0].pages_viewed, vec![2]);
    assert_eq!(f.pins.len(), 23);
    assert!(f.provider.contains("OpenAI"));
    let md = report::markdown(None, f);
    assert!(
        md.contains("pages rendered as images when the model asked"),
        "{md}"
    );
    assert!(md.contains("it viewed pages [2]"), "{md}");
    assert!(md.contains("spend is not tracked"), "{md}");

    let posts = &backend.wire.posts;
    let first = &posts[0].2;
    assert_eq!(posts[0].0, "/chat/completions");
    let opening = first["messages"][1]["content"]
        .as_array()
        .ok_or("the first user message")?;
    let datasheet_text = opening[0]["text"].as_str().unwrap_or_default();
    assert!(datasheet_text.contains("=== page 1 ===") && datasheet_text.contains("PIN 2 VCC"));
    let instructions = opening[1]["text"].as_str().unwrap_or_default();
    assert!(
        instructions.contains("datasheet_view_page"),
        "{instructions}"
    );
    assert!(first["tools"].as_array().is_some_and(|t| {
        t.iter()
            .any(|t| t["function"]["name"] == "datasheet_view_page")
    }));
    // The output limit is the model's, near what its context allows.
    assert_eq!(first["max_tokens"], 32_000);
    // The page went back as an image after its tool message.
    let second = posts[1].2["messages"].as_array().ok_or("messages")?;
    let n = second.len();
    assert_eq!(second[n - 2]["role"], "tool");
    assert!(
        second[n - 1]["content"][1]["image_url"]["url"]
            .as_str()
            .is_some_and(|u| u.starts_with("data:image/png;base64,"))
    );
    // The malformed call was answered with an error and never run; its
    // arguments went back as an empty object.
    let third = posts[2].2["messages"].as_array().ok_or("messages")?;
    let tool_reply = third.last().ok_or("a tool reply")?;
    assert_eq!(tool_reply["role"], "tool");
    assert!(
        tool_reply["content"].as_str().is_some_and(|c| c
            .starts_with("Error: This call was not run")
            && c.contains("not valid JSON")),
        "{tool_reply}"
    );
    assert_eq!(
        third[third.len() - 2]["tool_calls"][0]["function"]["arguments"],
        "{}"
    );
    // Reasoning never goes back to the server, and the history only grows.
    assert!(
        !posts
            .iter()
            .any(|p| p.2.to_string().contains("I should call"))
    );
    for pair in posts.windows(2) {
        let (a, b) = (
            pair[0].2["messages"].as_array().ok_or("messages")?,
            pair[1].2["messages"].as_array().ok_or("messages")?,
        );
        assert_eq!(&b[..a.len()], &a[..]);
    }
    Ok(())
}

fn cut_reply() -> Value {
    json!({"model": "local-27b", "choices": [{"finish_reason": "length", "message": {
        "role": "assistant", "content": "", "reasoning": "Thinking about pin 1...",
        "tool_calls": [{"id": "c", "type": "function",
            "function": {"name": "library_pinout_edit", "arguments": "{\"pins\": [{\"row\""}}]}}],
        "usage": {"prompt_tokens": 12_000, "completion_tokens": 8_192}})
}

#[test]
fn replies_cut_off_at_the_limit_are_reported_never_used() -> Fallible {
    let dir = tempfile::tempdir()?;
    // A small context: 16 384 output tokens a request, 24 576 after a cut.
    let config = local_config(32_768);
    let mut backend = openai(vec![cut_reply(), cut_reply(), cut_reply()], &config);
    let mut session = Session::default();
    let before = session.engine().revision();
    let (outcome, log) = run_with(&config, &mut backend, text_datasheet(&dir)?, &mut session)?;
    let Stop::CutOff(why) = &outcome.stop else {
        return Err(format!("{:?}", outcome.stop).into());
    };
    assert!(why.contains("3 replies in a row"), "{why}");
    assert!(outcome.report.is_none());
    assert_eq!(session.engine().revision(), before, "nothing ran");
    assert!(
        log.iter()
            .filter(|e| matches!(e, Event::Tool { error: true, .. }))
            .count()
            == 2,
        "the first two cut-off calls were answered with errors; the third ended the run"
    );
    assert_eq!(outcome.facts.request_log[0].stop_reason, "cut_off");
    // Never more than the context leaves: the limit after a cut is clamped
    // to what fits beside the prompt.
    let limits: Vec<u64> = backend
        .wire
        .posts
        .iter()
        .map(|p| p.2["max_tokens"].as_u64().unwrap_or(0))
        .collect();
    assert_eq!(limits[0], 16_384, "{limits:?}");
    assert!(limits[1] > limits[0], "more room after a cut: {limits:?}");
    // 12 000 prompt tokens and more: never the full 24 576 beside them.
    assert!(
        limits[1..].iter().all(|l| *l + 12_000 <= 32_768),
        "{limits:?}"
    );
    Ok(())
}

#[test]
fn a_prompt_too_large_for_the_context_says_why_before_sending() -> Fallible {
    let dir = tempfile::tempdir()?;
    let config = local_config(8_000);
    let mut backend = openai(vec![], &config);
    let result = run_with(
        &config,
        &mut backend,
        text_datasheet(&dir)?,
        &mut Session::default(),
    );
    let Err(error) = result else {
        return Err("expected a refusal".into());
    };
    let text = error.to_string();
    assert!(text.contains("does not fit the 8000 tokens"), "{text}");
    assert!(backend.wire.posts.is_empty());
    Ok(())
}

#[test]
fn token_and_time_caps_bind_a_model_without_a_price() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut config = local_config(131_072);
    // The first request alone could take 32 000 output tokens.
    config.budget.max_total_tokens = 30_000;
    let mut backend = openai(chat_script(&script()?), &config);
    let (outcome, _) = run_with(
        &config,
        &mut backend,
        text_datasheet(&dir)?,
        &mut Session::default(),
    )?;
    assert!(
        matches!(&outcome.stop, Stop::Budget(why) if why.contains("30000 tokens")),
        "{:?}",
        outcome.stop
    );
    assert!(backend.wire.posts.is_empty());

    let mut config = local_config(131_072);
    config.budget.max_duration = std::time::Duration::ZERO;
    let mut backend = openai(chat_script(&script()?), &config);
    let (outcome, _) = run_with(
        &config,
        &mut backend,
        text_datasheet(&dir)?,
        &mut Session::default(),
    )?;
    assert!(
        matches!(&outcome.stop, Stop::Budget(why) if why.contains("seconds")),
        "{:?}",
        outcome.stop
    );
    // Each request waits no longer than the run has left.
    let mut config = local_config(131_072);
    config.budget.max_duration = std::time::Duration::from_secs(600);
    let mut backend = openai(chat_script(&script()?), &config);
    run_with(
        &config,
        &mut backend,
        text_datasheet(&dir)?,
        &mut Session::default(),
    )?;
    assert!(
        backend
            .wire
            .posts
            .iter()
            .all(|p| p.3 <= std::time::Duration::from_secs(600))
    );
    Ok(())
}

#[test]
fn a_model_declared_without_tools_says_why() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut config = local_config(131_072);
    config.profile.capabilities.tool_calling = false;
    let result = run_with(
        &config,
        &mut openai(vec![], &config),
        text_datasheet(&dir)?,
        &mut Session::default(),
    );
    assert!(
        matches!(&result, Err(e) if e.to_string().contains("not possible with this model") && e.to_string().contains("tool calling")),
        "{:?}",
        result.err()
    );
    Ok(())
}

#[test]
fn a_shrinking_prompt_count_warns_of_truncation() -> Fallible {
    let dir = tempfile::tempdir()?;
    let config = local_config(131_072);
    let mut replies = chat_script(&script()?);
    // The server suddenly counts far fewer prompt tokens.
    replies[2]["usage"]["prompt_tokens"] = json!(2_000);
    let mut backend = openai(replies, &config);
    let (outcome, log) = run_with(
        &config,
        &mut backend,
        text_datasheet(&dir)?,
        &mut Session::default(),
    )?;
    assert_eq!(
        outcome.facts.warnings.len(),
        1,
        "{:?}",
        outcome.facts.warnings
    );
    assert!(outcome.facts.warnings[0].contains("Request 3"));
    assert!(log.iter().any(|e| matches!(e, Event::Warning(_))));
    let md = report::markdown(None, &outcome.facts);
    assert!(md.contains("**Warning:** Request 3"), "{md}");
    Ok(())
}

mod board;
mod local_run;
mod run1;
mod wroom3_binding;
