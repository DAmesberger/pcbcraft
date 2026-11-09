//! The first live Claude API run (`examples/esp32-s31-wroom-3/run1`: 13
//! recorded responses of `claude-opus-5-5`, converted to the neutral
//! recording format) and the hand-made
//! reference replay, both replayed through the current tools: what the
//! model would now have been told, and that the recorded usage is counted
//! exactly. No network: the datasheet is a stand-in, since a replay never
//! sends it.
use super::*;
use crate::model::record::{KeptRequest, Replay};
use crate::model::{ModelRequest, Part, anthropic};
use std::path::PathBuf;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-wroom-3")
        .join(name)
}

fn replay(name: &str) -> Fallible<(Outcome, Replay, Session)> {
    let dir = tempfile::tempdir()?;
    let mut backend = Replay::from_file(&example(name))?;
    backend.keep_requests = true;
    backend.prompt_tokens = Some(453_129);
    let mut session = Session::default();
    let goal = Goal {
        name: "ESP32-S31-WROOM-3".into(),
        datasheets: vec![datasheet(&dir)?],
        notes: String::new(),
    };
    let outcome = run(
        &goal,
        &RunConfig::default(),
        &mut backend,
        &mut session,
        &mut |_| {},
        &AtomicBool::new(false),
    )?;
    Ok((outcome, backend, session))
}

/// The Messages body a kept request was sent as.
fn wire(request: &KeptRequest) -> Value {
    anthropic::request_body(
        &RunConfig::default().profile,
        &ModelRequest {
            system: &request.system,
            tools: &request.tools,
            messages: &request.messages,
            max_output_tokens: request.max_output_tokens,
            timeout: request.timeout,
        },
    )
}

/// The text of the tool results request `n` (1-based) carries.
fn results_of(backend: &Replay, n: usize) -> Fallible<Vec<String>> {
    let request = backend.requests.get(n - 1).ok_or("request")?;
    let last = request.messages.last().ok_or("messages")?;
    Ok(last
        .content
        .iter()
        .filter_map(|p| match p {
            Part::ToolResult { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect())
}

#[test]
fn the_first_live_run_replays_with_its_recorded_usage() -> Fallible {
    let (outcome, transport, _) = replay("run1/run1.recording.jsonl")?;
    assert_eq!(outcome.stop, Stop::Submitted);
    let f = &outcome.facts;
    assert_eq!(f.requests, 13);
    assert_eq!(
        (
            f.input_tokens,
            f.cache_write_tokens,
            f.cache_read_tokens,
            f.output_tokens
        ),
        (28, 595_003, 6_211_969, 150_455)
    );
    // Two replies were cut off at 32 000 output tokens (5 and 8); the
    // request after each gets room to finish, then the limit returns.
    let limits: Vec<u64> = transport
        .requests
        .iter()
        .map(|r| u64::from(r.max_output_tokens))
        .collect();
    let mut expected = vec![32_000; 13];
    expected[5] = 64_000;
    expected[8] = 64_000;
    assert_eq!(limits, expected);
    assert_eq!(f.request_log[4].stop_reason, "cut_off");
    assert_eq!(f.request_log[4].tools, vec!["library_pinout_preview!"]);
    Ok(())
}

#[test]
fn the_first_live_runs_guessed_rule_helpers_fail_at_preview() -> Fallible {
    let (outcome, transport, session) = replay("run1/run1.recording.jsonl")?;
    // The schema the model read first now carries the rule contract.
    let schema = results_of(&transport, 2)?;
    assert!(
        schema
            .iter()
            .any(|r| r.contains("rule_api") && r.contains("active_count"))
    );
    // Response 6 previewed the full 99-row document with eight rules that
    // call pin_used/function_used and end in an `if` without `else`. The
    // reply now reports each rule's failure as a blocking finding.
    let preview = results_of(&transport, 7)?.join("");
    assert!(preview.contains("rule.runtime"), "{preview}");
    assert!(preview.contains("is not a rule helper"));
    let plan: Value = serde_json::from_str(&preview)?;
    let rules = plan["rules"].as_array().ok_or("rule checks")?;
    assert_eq!(rules.len(), 8);
    assert!(
        rules
            .iter()
            .all(|r| r["failures"].as_array().is_some_and(|f| !f.is_empty())),
        "{rules:?}"
    );
    // The compact plan: counts, findings, review items and rule checks,
    // not 700 change lines.
    assert!(plan.get("changes").is_none());
    assert!(preview.len() < 20_000, "{} characters", preview.len());
    // What was applied keeps those failures as blocking findings, and the
    // review lists them.
    assert_eq!(outcome.facts.rules.len(), 8);
    assert!(outcome.facts.rules.iter().all(|r| !r.failures.is_empty()));
    let blocking: Vec<String> = tools::drafts(&session.snapshot())
        .iter()
        .flat_map(|d| d["findings"].as_array().cloned().unwrap_or_default())
        .filter(|f| f["code"] == "rule.runtime" && !f["blocks"].is_null())
        .map(|f| f["message"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(blocking.len(), 8, "{blocking:?}");
    assert_eq!(outcome.facts.pins.len(), 99);
    Ok(())
}

#[test]
fn every_request_extends_the_last_one_so_the_datasheet_cache_holds() -> Fallible {
    // Nothing earlier in the conversation is rewritten (thinking blocks
    // stay valid and the cached prefix stays byte-identical): tools and
    // system never change, and each request's messages start with the
    // previous request's messages unchanged. The datasheets carry their
    // own breakpoint in the first message.
    let (_, transport, _) = replay("run1/run1.recording.jsonl")?;
    let requests: Vec<Value> = transport.requests.iter().map(wire).collect();
    let first = requests.first().ok_or("a request")?;
    assert_eq!(
        first["messages"][0]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
    for pair in requests.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert_eq!(a["tools"], b["tools"]);
        assert_eq!(a["system"], b["system"]);
        let (ma, mb) = (
            a["messages"].as_array().ok_or("messages")?,
            b["messages"].as_array().ok_or("messages")?,
        );
        assert!(mb.len() > ma.len());
        assert_eq!(&mb[..ma.len()], &ma[..]);
    }
    Ok(())
}

#[test]
fn the_review_diff_compares_the_run_with_the_reference_by_pin() -> Fallible {
    let (_, _, live) = replay("run1/run1.recording.jsonl")?;
    let (_, _, reference) = replay("wroom3.replay.jsonl")?;
    let text = crate::pins::diff(
        &live.snapshot(),
        &reference.snapshot(),
        ("run1", "reference"),
    );
    // Same module form, so the same 107 pads and keep-out.
    assert!(text.contains("## Package form\n\nIdentical."), "{text}");
    assert!(text.contains("107 pads in run1, 107 in reference; 0 numbers differ."));
    assert!(text.contains("## Footprint keep-outs\n\nIdentical."));
    // The run added a polarity dot to the marking.
    assert!(text.contains("`items`"));
    // Pin 10 (IO4): only the run lists the DAC channel (S31 Table 2-8).
    let pin10 = text
        .split("\n- pin ")
        .find(|s| s.starts_with("10 IO4"))
        .ok_or("pin 10 differs")?;
    assert!(
        pin10.contains("only run1:") && pin10.contains("DAC_CH0"),
        "{pin10}"
    );
    Ok(())
}

#[test]
fn the_reference_replay_still_runs_and_its_rules_hold_over_their_samples() -> Fallible {
    let (outcome, _, _) = replay("wroom3.replay.jsonl")?;
    assert_eq!(outcome.stop, Stop::Submitted);
    assert_eq!(outcome.facts.rules.len(), 8);
    for rule in &outcome.facts.rules {
        assert!(rule.failures.is_empty(), "{rule:?}");
        assert!(rule.samples > 1, "{rule:?}");
    }
    let pin45 = outcome
        .facts
        .pins
        .iter()
        .find(|p| p.number == "45")
        .ok_or("pin 45")?;
    assert_eq!(pin45.name, "IO36");
    Ok(())
}
