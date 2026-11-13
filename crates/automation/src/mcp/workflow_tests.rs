//! The datasheet-to-component workflow as an external agent runs it: the
//! `datasheet_to_component` prompt, then only MCP tool calls, scripted here in place
//! of a model (no network, no model). The agent's steps end at
//! `library_review_check`; publishing stays behind the import review gate
//! until the person has confirmed the rows.
use super::prompts::{self, DATASHEET_PART, DATASHEET_PART_SKILL, skill_body};
use super::scenario::{EXM48, Fallible};
use super::*;
use crate::{Mode, Policy};

fn rpc(server: &mut Server, method: &str, params: Value) -> Value {
    server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .unwrap_or(Value::Null)
}

/// One tool call: whether it failed, and its JSON payload.
fn tool(server: &mut Server, name: &str, arguments: Value) -> (bool, Value) {
    let reply = rpc(
        server,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    );
    let payload = reply["result"]["content"][0]["text"]
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null);
    (reply["result"]["isError"] != false, payload)
}

/// A tool call that must succeed.
fn ok(server: &mut Server, step: &str, name: &str, arguments: Value) -> Fallible<Value> {
    let (failed, payload) = tool(server, name, arguments);
    if failed {
        return Err(format!("{step}: {payload}").into());
    }
    Ok(payload)
}

fn revision(server: &Server) -> u64 {
    server.session().engine().revision()
}

fn query(server: &mut Server, step: &str, name: &str, params: Value) -> Fallible<Value> {
    let reply = ok(
        server,
        step,
        "project_query",
        json!({"query": name, "params": params}),
    )?;
    Ok(reply["result"].clone())
}

fn apply(server: &mut Server, step: &str, command: &str, params: Value) -> Fallible<Value> {
    let expected_revision = revision(server);
    ok(
        server,
        step,
        "project_apply",
        json!({"expected_revision": expected_revision, "operation": {"editor": "Library",
            "operation": {"command": command, "params": params}}}),
    )
}

/// Preview `document` onto `target` and apply exactly that plan.
fn import(server: &mut Server, step: &str, target: Value, document: &Value) -> Fallible<Value> {
    let plan = query(
        server,
        step,
        "library.pinout_preview",
        json!({"target": target, "document": document}),
    )?;
    apply(
        server,
        step,
        "library.pinout_apply",
        json!({"target": target, "document": document, "base": plan["base"],
            "accept": {"accept": "all"}}),
    )
}

/// The report an agent writes from the EXM-48 fixture's datasheet.
fn report(open_questions: Value) -> Value {
    json!({"report": {
        "part": "EXM-48",
        "summary": "48-pin QFN MCU: pins, functions and rules from Table 4; footprint from the QFN-48 7x7 0.5 EP 5.3 row.",
        "sources": [{"name": "DS", "title": "EXM-48 datasheet", "revision": "rev 3"}],
        "values": [
            {"item": "pin count", "value": "48 + EP", "source": {"document": "DS", "page": 11, "location": "Table 4"}, "confidence": "high"},
            {"item": "pitch", "value": "0.5 mm", "source": {"document": "DS", "page": 40, "location": "Figure 12"}, "confidence": "high"},
            {"item": "exposed pad", "value": "5.3 mm", "source": {"document": "DS", "page": 40, "location": "Figure 12"}, "confidence": "medium", "note": "read from an ambiguous dimension line"},
        ],
        "open_questions": open_questions,
    }})
}

fn draft_of(server: &mut Server, id: u64) -> Fallible<Value> {
    let snapshot = ok(
        server,
        "inspect",
        "project_inspect",
        json!({"verbose": true}),
    )?;
    snapshot["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| d["draft"]["id"] == id)
        .cloned()
        .ok_or_else(|| format!("draft {id}").into())
}

#[test]
fn the_prompt_is_listed_and_returns_the_workflow() -> Fallible<()> {
    let mut server = Server::new(Session::default(), Policy::default());
    let init = rpc(&mut server, "initialize", json!({}));
    assert!(
        init["result"]["capabilities"]["prompts"].is_object(),
        "{init}"
    );
    let listed = rpc(&mut server, "prompts/list", json!({}));
    let prompt = listed["result"]["prompts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == DATASHEET_PART)
        .ok_or_else(|| format!("{DATASHEET_PART} listed: {listed}"))?;
    let required: Vec<&str> = prompt["arguments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["required"] == true)
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert_eq!(required, ["part", "datasheets"]);

    let got = rpc(
        &mut server,
        "prompts/get",
        json!({"name": DATASHEET_PART, "arguments": {"part": "EXM-48",
            "datasheets": "exm48.pdf\nhttps://example.com/exm48-errata.pdf", "notes": "QFN-48 only"}}),
    );
    let message = &got["result"]["messages"][0];
    assert_eq!(message["role"], "user", "{got}");
    let text = message["content"]["text"].as_str().unwrap_or_default();
    // The workflow first, word for word, then the task.
    let (workflow, task) = text.split_once("## Task\n").ok_or("a task section")?;
    assert_eq!(
        workflow.trim_end(),
        skill_body(DATASHEET_PART_SKILL).trim_end()
    );
    assert!(task.contains("\"EXM-48\"") && task.contains("- exm48.pdf"));
    assert!(
        task.contains("- https://example.com/exm48-errata.pdf") && task.contains("QFN-48 only")
    );
    for step in [
        "No guessing",
        "library_pinout_schema",
        "library.pinout_preview",
        "library.pinout_apply",
        "library.package_plan",
        "library.package_from_template",
        "component.check_drafts",
        "library_review_check",
        "Verification checklist",
    ] {
        assert!(workflow.contains(step), "{step}");
    }

    for bad in [
        json!({"name": "other", "arguments": {"part": "X", "datasheets": "a.pdf"}}),
        json!({"name": DATASHEET_PART, "arguments": {"part": "X"}}),
        json!({"name": DATASHEET_PART, "arguments": {"part": 3, "datasheets": "a.pdf"}}),
    ] {
        let refused = rpc(&mut server, "prompts/get", bad.clone());
        assert_eq!(refused["error"]["code"], INVALID_PARAMS, "{bad}: {refused}");
    }
    Ok(())
}

/// One file is the source of both: the prompt serves the skill file in the
/// repository, read now, without its front matter; and the workflow names
/// only tools and wire IDs this build has.
#[test]
fn the_skill_file_and_the_prompt_are_one_source() -> Fallible<()> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../skills/sparksmith-datasheet-part/SKILL.md"
    );
    let on_disk = std::fs::read_to_string(path)?;
    assert_eq!(on_disk, DATASHEET_PART_SKILL);
    assert!(on_disk.starts_with("---\nname: sparksmith-datasheet-part\ndescription: "));
    let body = skill_body(&on_disk);
    assert!(body.starts_with("# Create a component from datasheets"));
    assert!(!body.contains("\n---\n") && !body.contains("name: sparksmith"));
    let got = prompts::get(&json!({"name": DATASHEET_PART,
        "arguments": {"part": "P", "datasheets": "a.pdf"}}))
    .map_err(|e| e.to_string())?;
    let text = got["messages"][0]["content"]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(text.starts_with(body));
    let named = super::tests::names_only_what_exists(body);
    assert!(named >= 8, "the workflow names its requests ({named})");
    Ok(())
}

/// The whole workflow over MCP with no model: schema, preview, apply,
/// footprint, checks, the review check refusing an incomplete report and
/// accepting the fixed one, and publishing refused by the review gate until
/// the person has confirmed the rows.
#[test]
fn a_scripted_session_completes_the_workflow_through_the_review_gate() -> Fallible<()> {
    // Publishing is Ask-class: allow-all stands in for the person's Allow,
    // so the test reaches the review gate that must still refuse it.
    let mut server = Server::new(
        Session::default(),
        Policy::new(Mode::AllowAll, Vec::<std::path::PathBuf>::new()),
    );
    rpc(&mut server, "initialize", json!({}));
    let schema = ok(&mut server, "schema", "library_pinout_schema", json!({}))?;
    assert_eq!(schema["format"], 1);

    // The agent forgot one row's source.
    let complete: Value = serde_json::from_str(EXM48)?;
    let mut document = complete.clone();
    let r2 = document["pins"]
        .as_array_mut()
        .and_then(|pins| pins.iter_mut().find(|p| p["row"] == "r2"))
        .ok_or("row r2")?;
    let name = r2["name"].clone();
    r2["source"] = Value::Null;
    let applied = import(
        &mut server,
        "import",
        json!({"target": "new", "name": "EXM-48"}),
        &document,
    )?;
    let drafts: Vec<u64> = applied["outcome"]["report"]["drafts"]
        .as_array()
        .ok_or_else(|| format!("the import's drafts: {applied}"))?
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    let component = drafts.first().copied().ok_or("the component draft")?;
    let symbol = drafts.get(1).copied().ok_or("the symbol draft")?;

    // An early review check: no open question for the uncertain row, and
    // the imported pin without a source page, named.
    let before = server.session().engine().document().clone();
    let checked = ok(
        &mut server,
        "review check",
        "library_review_check",
        report(json!([])),
    )?;
    assert_eq!(server.session().engine().document(), &before, "a read");
    assert_eq!(checked["accepted"], false, "{checked}");
    let problems = checked["problems"].to_string();
    assert!(problems.contains("uncertain"), "{problems}");
    let pin = name.as_str().unwrap_or_default();
    assert!(
        problems.contains("source page") && problems.contains(pin),
        "{problems}"
    );

    // The agent restores the source (before the footprint, which renumbers
    // pad 49 as EP) and asks about the uncertain row.
    import(
        &mut server,
        "re-import",
        json!({"target": "draft", "id": component}),
        &complete,
    )?;
    // The footprint from a starter row; 49 is the form's EP.
    let binding = drafts
        .iter()
        .copied()
        .find(|&id| {
            draft_of(&mut server, id).is_ok_and(|d| d["draft"]["definition"]["kind"] == "binding")
        })
        .ok_or("the binding draft")?;
    let package = draft_of(&mut server, binding)?["draft"]["definition"]["body"]["package"]["id"]
        .as_u64()
        .ok_or("the package draft")?;
    let rows = query(
        &mut server,
        "templates",
        "library.templates",
        json!({"query": "qfn pins:48 pitch:0.5 body:7x7"}),
    )?;
    let starter = rows["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|h| h["starter"].as_str().filter(|n| n.contains("EP 5.3")))
        .ok_or_else(|| format!("a starter row: {rows}"))?
        .to_owned();
    let request = json!({"draft": package, "row": {"source": "starter", "name": starter},
        "renumber": [{"from": "49", "to": "EP"}]});
    let plan = query(
        &mut server,
        "package plan",
        "library.package_plan",
        json!({"request": request}),
    )?;
    assert_eq!(plan["footprint_created"], true, "{plan}");
    let built = apply(
        &mut server,
        "package from template",
        "library.package_from_template",
        request,
    )?;
    let footprint = built["outcome"]["plan"]["footprint"]
        .as_u64()
        .ok_or_else(|| format!("the footprint: {built}"))?;
    query(
        &mut server,
        "check drafts",
        "component.check_drafts",
        json!({}),
    )?;

    let checked = ok(
        &mut server,
        "review check again",
        "library_review_check",
        report(json!([
            "Row r20 (PB4): is AF1 SPI1_MISO? The scan is unreadable on page 14."
        ])),
    )?;
    assert_eq!(checked["accepted"], true, "{checked}");
    assert_eq!(checked["pins"]["uncertain"], 1, "{checked}");
    assert_eq!(checked["pins"]["unreviewed"], 49, "{checked}");
    let markdown = checked["markdown"].as_str().unwrap_or_default();
    assert!(markdown.contains("| exposed pad | 5.3 mm | DS p.40 Figure 12 | Medium |"));
    assert!(markdown.contains("- [ ] Row r20") && markdown.contains("not published"));
    assert!(!markdown.contains('$'));

    // Publishing is refused while a row is uncertain, and changes nothing.
    let set = json!({"drafts": [component, symbol, package, footprint, binding]});
    let before = revision(&server);
    let (failed, refusal) = tool(
        &mut server,
        "project_apply",
        json!({"expected_revision": before, "operation": {"editor": "Library",
            "operation": {"command": "library.publish", "params": set}}}),
    );
    assert!(failed && refusal["code"] == "conflict", "{refusal}");
    assert!(
        refusal.to_string().contains("flagged uncertain"),
        "{refusal}"
    );
    assert_eq!(revision(&server), before);

    // The person checks every row in the Pins tab and confirms them (their
    // step, in the window): the review state goes, and they publish.
    let draft = draft_of(&mut server, component)?;
    let mut body = draft["draft"]["definition"]["body"].clone();
    body["review"] = Value::Null;
    body["designator_prefix"] = json!("U");
    apply(
        &mut server,
        "the person confirms",
        "library.edit",
        json!({"edits": [{"id": component, "meta": draft["draft"]["meta"],
            "definition": {"kind": "component", "body": body}}]}),
    )?;
    let published = apply(&mut server, "the person publishes", "library.publish", set)?;
    assert!(
        published["outcome"]["revisions"]
            .as_array()
            .is_some_and(|r| r.len() == 5),
        "{published}"
    );
    Ok(())
}

/// The check is a read: a server that asks for every edit still runs it.
#[test]
fn the_review_check_passes_the_strictest_gate() {
    let policy = Policy::new(Mode::AskEveryTime, Vec::<std::path::PathBuf>::new());
    let mut server = Server::new(Session::default(), policy);
    let (failed, checked) = tool(&mut server, "library_review_check", report(json!([])));
    assert!(!failed, "{checked}");
    assert_eq!(checked["accepted"], false, "no drafts yet: {checked}");
}
