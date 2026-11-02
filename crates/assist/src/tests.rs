//! The tool loop against recorded API responses (no network): an original
//! synthetic radio module, EXR-22, authored end to end through the tools,
//! and the caps, cancellation and refusal paths.
use super::*;
use crate::api::ReplayTransport;
use crate::tools::ToolOutcome;

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
        reply_json(tools::execute(
            &mut shadow,
            &mut workspace,
            "library_pinout_edit",
            &chunk,
        ))?;
        out.push(tool_turn(&format!("t2{k}"), "library_pinout_edit", chunk));
    }
    let preview_input = json!({"target": target});
    let plan = reply_json(tools::execute(
        &mut shadow,
        &mut workspace,
        "library_pinout_preview",
        &preview_input,
    ))?;
    out.push(tool_turn("t2", "library_pinout_preview", preview_input));
    let apply_input = json!({"base": plan["base"]});
    let applied = reply_json(tools::execute(
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
    reply_json(tools::execute(
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

#[test]
fn a_recorded_run_authors_the_module_and_submits_its_review() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut transport = ReplayTransport::new(script()?);
    transport.prompt_tokens = Some(60_000);
    let mut session = Session::default();
    let mut log = Vec::new();
    let outcome = run(
        &goal(datasheet(&dir)?),
        &Settings::default(),
        &mut transport,
        &mut session,
        &mut |e| log.push(e),
        &AtomicBool::new(false),
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
    let first = transport.requests.first().ok_or("a request")?;
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
    assert_eq!(
        first["tools"].as_array().map(Vec::len),
        Some(tools::definitions().len())
    );
    // Each later request carries the previous turn's assistant content
    // unchanged and one user message of tool results.
    let last = transport.requests.last().ok_or("a request")?;
    assert_eq!(last["messages"].as_array().map(Vec::len), Some(1 + 2 * 9));
    // The model never sent the document again to apply it.
    let apply = transport
        .requests
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
    let mut transport = ReplayTransport::new(vec![
        expensive,
        tool_turn("t2", "component_check_drafts", json!({})),
    ]);
    transport.prompt_tokens = Some(10_000);
    let settings = Settings {
        max_usd: 5.5,
        ..Settings::default()
    };
    let outcome = run(
        &goal(datasheet(&dir)?),
        &settings,
        &mut transport,
        &mut Session::default(),
        &mut |_| {},
        &AtomicBool::new(false),
    )?;
    assert!(
        matches!(outcome.stop, Stop::Budget(_)),
        "{:?}",
        outcome.stop
    );
    assert_eq!(
        transport.requests.len(),
        1,
        "the second request was never sent"
    );
    assert!(outcome.facts.spent_usd <= 5.5);
    Ok(())
}

#[test]
fn a_cancelled_run_sends_nothing_and_a_refusal_stops() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut transport = ReplayTransport::new(vec![]);
    let outcome = run(
        &goal(datasheet(&dir)?),
        &Settings::default(),
        &mut transport,
        &mut Session::default(),
        &mut |_| {},
        &AtomicBool::new(true),
    )?;
    assert_eq!(outcome.stop, Stop::Cancelled);
    assert!(transport.requests.is_empty());

    let refusal = json!({"id": "m", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [], "stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber"},
        "usage": {"input_tokens": 10, "output_tokens": 0}});
    let mut transport = ReplayTransport::new(vec![refusal]);
    let outcome = run(
        &goal(datasheet(&dir)?),
        &Settings::default(),
        &mut transport,
        &mut Session::default(),
        &mut |_| {},
        &AtomicBool::new(false),
    )?;
    assert_eq!(outcome.stop, Stop::Refused("cyber".into()));
    Ok(())
}

#[test]
fn a_tool_call_cut_off_at_max_tokens_is_not_run() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut cut = tool_turn("t1", "library_set_marking", json!({"draft": 1}));
    cut["stop_reason"] = json!("max_tokens");
    let mut transport = ReplayTransport::new(vec![cut]);
    let mut session = Session::default();
    let before = session.engine().revision();
    let outcome = run(
        &goal(datasheet(&dir)?),
        &Settings::default(),
        &mut transport,
        &mut session,
        &mut |_| {},
        &AtomicBool::new(false),
    )?;
    // The recording ends after the cut-off turn.
    assert!(matches!(outcome.stop, Stop::Failed(_)));
    assert_eq!(session.engine().revision(), before);
    let second = transport.requests.get(1).ok_or("the follow-up request")?;
    let result = &second["messages"][2]["content"][0];
    assert_eq!(result["is_error"], true);
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
    let (_, tokens) = documents(&[Datasheet::File(path)])?;
    assert_eq!(tokens, 2 * TOKENS_PER_PAGE);
    Ok(())
}

#[test]
fn non_pdf_files_and_models_without_prices_are_refused() -> Fallible {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("x.pdf");
    std::fs::write(&path, b"not a pdf")?;
    assert!(documents(&[Datasheet::File(path)]).is_err());
    assert!(documents(&[Datasheet::Url("http://x/y.pdf".into())]).is_err());
    let settings = Settings {
        model: "unpriced".into(),
        ..Settings::default()
    };
    let result = run(
        &goal(datasheet(&dir)?),
        &settings,
        &mut ReplayTransport::default(),
        &mut Session::default(),
        &mut |_| {},
        &AtomicBool::new(false),
    );
    assert!(matches!(result, Err(Error::Config(_))));
    Ok(())
}

mod board;
mod run1;
mod wroom3_binding;
