//! The tools an assisted run offers the model: the Sparksmith MCP contract
//! narrowed to authoring one part, executed in-process against an
//! automation [`Session`] through the gate ([`send`]: the same request
//! types and command engine the MCP server and CLI use). Each tool names
//! the MCP command it runs. The runner fills `expected_revision` from the
//! session, so the model never handles revisions, and compacts replies:
//! a mutation answers with its outcome and each draft's findings instead
//! of the whole document snapshot, and a pinout preview with change
//! counts, findings, review items and rule checks instead of every change.
//!
//! The pinout document is built in a [`Workspace`] the runner keeps:
//! `library_pinout_edit` adds header fields, peripherals, pin rows and
//! rules in chunks (no single tool call has to carry a 100-row document),
//! preview checks the working document and apply imports exactly what was
//! previewed, by its base, without the model sending the document again.
//!
//! What is deliberately absent: `library.publish`, publishing to folders,
//! saving, opening, file exports and library sources. A run only ever
//! creates drafts; publishing stays with a person (the import review gate).
use crate::model::ToolSpec;
use crate::report::ReviewReport;
use serde_json::{Map, Value, json};
use sparksmith_automation::{Caller, Policy, Session};
use sparksmith_core::library::pinout::{
    MAX_PINOUT_PERIPHERALS, MAX_PINOUT_PINS, MAX_PINOUT_RULES, MAX_PINOUT_VARIANTS,
};
use std::collections::BTreeMap;

/// Characters of one tool result sent back; longer results are cut with a
/// note (the model can ask for a narrower view).
pub const MAX_RESULT_CHARS: usize = 120_000;
/// Findings listed per draft in a compact reply.
const MAX_FINDINGS: usize = 40;
/// Findings, review items and changes a compact preview lists.
const MAX_PLAN_ITEMS: usize = 60;
/// Pin rows one `library_pinout_edit` call may carry: about 8 000 output
/// tokens of JSON, so a call never approaches a request's output limit.
pub const MAX_EDIT_PINS: usize = 40;

/// What a tool call produced.
#[derive(Debug)]
pub enum ToolOutcome {
    /// Text for the model, whether it is an error, and a one-line step
    /// for the run log when the call changed the document.
    Reply {
        text: String,
        error: bool,
        step: Option<String>,
        /// Images that go with the text (a rendered datasheet page).
        images: Vec<crate::model::Image>,
    },
    /// The model submitted its review report: the run is complete.
    Report(Box<ReviewReport>),
}

/// The runner's state across tool calls: the working pinout document and
/// what was last previewed from it.
#[derive(Clone, Debug, Default)]
pub struct Workspace {
    /// The pinout document being built, as JSON (it may be incomplete
    /// until previewed).
    pub pinout: Map<String, Value>,
    /// The target, base and document of the last successful preview.
    previewed: Option<(Value, Value, Value)>,
    /// The document of the last successful apply.
    pub applied: Option<Value>,
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        }),
    }
}

/// The tool definitions, in a fixed order so the prompt prefix caches.
pub fn definitions() -> Vec<ToolSpec> {
    let object = json!({"type": "object"});
    let objects = json!({"type": "array", "items": {"type": "object"}});
    let strings = json!({"type": "array", "items": {"type": "string"}});
    let report = serde_json::to_value(schemars::schema_for!(ReviewReport)).unwrap_or(json!({}));
    vec![
        tool(
            "library_pinout_schema",
            "MCP library_pinout_schema: the JSON schema of the pinout document (format 1) you fill from the datasheet: device and reference, one variant per package with its terminal count (every pad that is a terminal, the ground array counted once), peripherals with roles, one row per pin with its number per variant, functions with explicit peripheral, role, mux code and mux table, source page and table, uncertain flags, and pin rules; and rule_api, the complete pin-rule script contract with examples (the only functions a rule may call). Call it once before writing the document.",
            json!({}),
            &[],
        ),
        tool(
            "library_pinout_edit",
            "Build the working pinout document in chunks (the runner keeps it; nothing in the project changes). Each call may set the header fields (format, device, variants), upsert peripherals by label, upsert pin rows by their row key (at most 40 rows a call: send a large pinout over several calls), upsert rules by label, and remove rows or rules; clear: true starts over. Returns the document's counts and, per variant, the pin numbers still missing. Then call library_pinout_preview without a document.",
            json!({
                "clear": {"type": "boolean"},
                "format": {"type": "integer"},
                "device": object,
                "variants": objects,
                "peripherals": objects,
                "pins": objects,
                "remove_pins": strings,
                "rules": objects,
                "remove_rules": strings,
            }),
            &[],
        ),
        tool(
            "library_pinout_preview",
            "MCP project_query library.pinout_preview: check the working pinout document (or `document`, which then replaces it) and plan its import without changing the project. target is {\"target\": \"new\", \"name\": ...} for a new part or {\"target\": \"draft\", \"id\": ...} to re-import onto the component draft. Returns the base for library_pinout_apply, change counts, findings, review items and each rule's evaluation (default verdict, samples, failures). Fix every refusal, blocking finding and rule failure with library_pinout_edit and preview again.",
            json!({"target": object, "document": object}),
            &["target"],
        ),
        tool(
            "library_pinout_apply",
            "MCP project_apply library.pinout_apply: import exactly the document last previewed. base is optional: leave it out to apply the last preview, or give the plan's base exactly (accept {\"accept\": \"all\"} unless rows are excluded). Do not send the document again. Creates component, symbol, package and binding drafts whose imported rows are all unreviewed (the review gate); rows flagged uncertain block publishing until a person clears them. Returns the outcome and each draft's findings.",
            json!({"base": {"type": ["string", "integer"]}, "accept": object, "target": object, "document": object}),
            &[],
        ),
        tool(
            "library_package_schema",
            "The JSON schema of the module package form (values of library_package_plan / library_package_from_template with row {\"source\": \"custom\", \"family\": \"module\"}), with the frame conventions. Call it before writing the values.",
            json!({}),
            &[],
        ),
        tool(
            "library_templates",
            "MCP project_query library.templates: search built-in package rows (e.g. \"qfn pins:48 pitch:0.5\"). Modules have no rows; use the module family's custom entry.",
            json!({"query": {"type": "string"}, "limit": {"type": "integer"}}),
            &["query"],
        ),
        tool(
            "library_package_plan",
            "MCP project_query library.package_plan: preview building the package draft's form, footprint draft (lands, courtyard, silkscreen, pin-1 mark, antenna keep-out) and generated 3D model from request {draft: the package draft id, row: {source: custom, family: module}, values: {family: module, ...}}. Changes nothing.",
            json!({"request": object}),
            &["request"],
        ),
        tool(
            "library_package_from_template",
            "MCP project_apply library.package_from_template: apply the previewed request as one undo step (add \"replace\": true to rebuild a footprint that already has pads). The package's terminals, and so the binding's mappings, carry over by number: a terminal keeps its identity when the new form gives its number; the plan's terminals.summary says what was kept, and package.terminals_unmatched names any number left unmatched. When the datasheet gives a pad another number than the form (an exposed pad numbered 95 where a form says EP), add \"renumber\": [{\"from\": \"95\", \"to\": \"EP\"}]; otherwise map those explicitly.",
            json!({"request": object}),
            &["request"],
        ),
        tool(
            "library_set_marking",
            "MCP library_set_marking: the component draft's display-only top marking on the can: {draft, marking: {items: [{item: text, template: \"...\"} | {item: artwork, svg: \"<svg ...>\", size_mm: [w, h], licence: \"...\"} | {item: polarity, mark: dot}], finish: laser_etch}}. Never copy vendor logos; text items are the safe choice. Add a polarity mark only where the part itself carries one.",
            json!({"draft": {"type": "integer"}, "marking": object}),
            &["draft", "marking"],
        ),
        tool(
            "library_check_part_file",
            "MCP library_check_part_file: evaluate a built-in SolidCraft part file (builtin: \"esp32-s31-wroom-3\", a shielded module body) at literal values for some of its parameters ({\"can_height\": {\"mm\": 2.465}}; units mm, deg, count, number) and report its parameters, bodies and materials, marking surfaces, bounds and convention findings. Changes nothing. Read the parameter list before choosing values.",
            json!({"builtin": {"type": "string"}, "values": object}),
            &["builtin"],
        ),
        tool(
            "library_use_part_file",
            "MCP library_use_part_file: make the package draft's 3D model the built-in part file at values taken from the datasheet's drawing (each value with its page in the review), instead of the generated model. The footprint, terminals and keep-out stay the package's. One undo step; refused on a blocking finding. Use it for a module whose body a built-in part file covers, after library_package_from_template.",
            json!({"package": {"type": "integer"}, "builtin": {"type": "string"}, "values": object}),
            &["package", "builtin"],
        ),
        tool(
            "library_set_part_values",
            "MCP library_set_part_values: new values for the part file the package draft uses (they replace the previous values). One undo step; refused on a blocking finding.",
            json!({"package": {"type": "integer"}, "values": object}),
            &["package", "values"],
        ),
        tool(
            "library_inspect_draft",
            "The full definition of one draft (component, symbol, package, footprint or binding) with its check findings. Use it to verify what was built, e.g. footprint pad positions or the package terminals.",
            json!({"id": {"type": "integer"}}),
            &["id"],
        ),
        tool(
            "library_edit",
            "MCP project_apply library.edit: replace whole draft definitions, {edits: [{id, meta, definition: {kind, body}}]} as library_inspect_draft returns them. Use it for small corrections (designator prefix, description, field values); the import review state must stay as the import left it.",
            json!({"edits": {"type": "array", "items": object}}),
            &["edits"],
        ),
        tool(
            "component_check_drafts",
            "MCP project_query component.check_drafts: every draft's publish findings (codes, messages, whether they block). Run it before submitting the review.",
            json!({}),
            &[],
        ),
        tool(
            "submit_review",
            "Finish the run: the review report a person reads before publishing. Every value you took from the datasheets with document, page, table or figure, and confidence (high: read from a table or dimensioned drawing; medium: derived arithmetically or an ambiguous dimension line; low: inferred, measured from the drawing's scale or not stated); and every open question. Never claim the part is verified.",
            json!({"report": report}),
            &["report"],
        ),
    ]
}

/// Run one tool call against `session`.
pub fn execute(
    session: &mut Session,
    policy: &Policy,
    workspace: &mut Workspace,
    name: &str,
    input: &Value,
) -> ToolOutcome {
    let unpacked = match unpack_text_arguments(name, input) {
        Ok(unpacked) => unpacked,
        Err(why) => return reply(json!({"refused": why}), true, None),
    };
    let input = unpacked.as_ref().unwrap_or(input);
    let revision = session.engine().revision();
    let library = |command: &str, params: Value| {
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Library", "operation": {"command": command, "params": params}},
        }})
    };
    let query = |name: &str, params: Value| json!({"command": "project.query", "params": {"query": name, "params": params}});
    let request = match name {
        "library_pinout_schema" => json!({"command": "library.pinout_schema", "params": {}}),
        "library_pinout_edit" => return edit(workspace, input),
        "library_pinout_preview" => return preview(session, policy, workspace, input),
        "library_pinout_apply" => {
            let (target, document, base) = match apply_inputs(workspace, input) {
                Ok(inputs) => inputs,
                Err(why) => return reply(json!({"refused": why}), true, None),
            };
            let accept = if input["accept"].is_object() {
                input["accept"].clone()
            } else {
                json!({"accept": "all"})
            };
            workspace.applied = Some(document.clone());
            library(
                "library.pinout_apply",
                json!({"target": target, "document": document, "base": base, "accept": accept}),
            )
        }
        "library_package_schema" => return reply(package_schema(), false, None),
        "library_templates" => {
            let mut p = json!({"query": input["query"]});
            if input["limit"].is_u64() {
                p["limit"] = input["limit"].clone();
            }
            query("library.templates", p)
        }
        "library_package_plan" => {
            query("library.package_plan", json!({"request": input["request"]}))
        }
        "library_package_from_template" => {
            library("library.package_from_template", input["request"].clone())
        }
        "library_set_marking" => json!({"command": "library.set_marking", "params": {
            "expected_revision": revision, "draft": input["draft"], "marking": input["marking"],
        }}),
        "library_check_part_file" => {
            let response = send(
                session,
                policy,
                json!({"command": "library.check_part_file",
                "params": {"builtin": input["builtin"], "values": values(input)}}),
            );
            return if response["ok"] == true {
                reply(response["result"]["report"].clone(), false, None)
            } else {
                reply(response["error"].clone(), true, None)
            };
        }
        "library_use_part_file" => json!({"command": "library.use_part_file", "params": {
            "expected_revision": revision, "package": input["package"],
            "builtin": input["builtin"], "values": values(input),
        }}),
        "library_set_part_values" => json!({"command": "library.set_part_values", "params": {
            "expected_revision": revision, "package": input["package"], "values": values(input),
        }}),
        "library_inspect_draft" => return inspect(session, input),
        "library_edit" => library("library.edit", json!({"edits": input["edits"]})),
        "component_check_drafts" => {
            let snapshot = session.snapshot();
            return reply(json!({"drafts": drafts(&snapshot)}), false, None);
        }
        "submit_review" => {
            return match serde_json::from_value::<ReviewReport>(report_input(input)) {
                Ok(report) => {
                    // The MCP workflow's `library.review_check`, on the same
                    // report and drafts: one set of rules for both paths.
                    let checked =
                        sparksmith_automation::review::check(&session.snapshot(), &report);
                    if checked["accepted"] == true {
                        ToolOutcome::Report(Box::new(report))
                    } else {
                        reply(json!({"refused": checked["problems"]}), true, None)
                    }
                }
                Err(e) => reply(
                    json!({"refused": format!("not a review report: {e}"),
                        "expected_shape": report_example()}),
                    true,
                    None,
                ),
            };
        }
        other => {
            return reply(
                json!({"error": format!("unknown tool {other}")}),
                true,
                None,
            );
        }
    };
    let mutation =
        request["command"] != "project.query" && request["command"] != "library.pinout_schema";
    let response = send(session, policy, request);
    if response["ok"] != true {
        if name == "library_pinout_apply" {
            workspace.applied = None;
        }
        return reply(response["error"].clone(), true, None);
    }
    let result = &response["result"];
    if mutation {
        let step = format!(
            "{name}: revision {} ({})",
            result["revision"],
            summary(&result["outcome"])
        );
        reply(
            json!({
                "revision": result["revision"],
                "outcome": result["outcome"],
                "drafts": drafts(result),
            }),
            false,
            Some(step),
        )
    } else {
        // Query results come tagged (`{"query": …, "result": …}`).
        reply(result.get("result").unwrap_or(result).clone(), false, None)
    }
}

/// Every request a tool sends to the session goes through the gate, as a
/// model's: there is no one to ask in a run yet, so Ask is refused with a
/// reason the model can read, and Never always is. The runner reads full
/// results (each draft's findings), so it asks for them.
pub fn send(session: &mut Session, policy: &Policy, mut request: Value) -> Value {
    request["verbose"] = json!(true);
    session.respond(Caller::Model(policy), request)
}

/// `input` with each argument the tool declares as an array or object, but
/// that arrived as JSON text, read as meant: some models send long arrays
/// as strings (a 4 000-character `edits` list was refused nine times for
/// it). `Ok(None)` when nothing needed reading. Text that should be an
/// array or object but is not valid JSON is refused with where it broke: a
/// server's tool parser leaves a long call with unbalanced brackets as text,
/// and the model needs that, not "expected a sequence", to fix it.
fn unpack_text_arguments(name: &str, input: &Value) -> Result<Option<Value>, String> {
    let (Some(fields), Some(spec)) = (
        input.as_object(),
        definitions().into_iter().find(|t| t.name == name),
    ) else {
        return Ok(None);
    };
    let mut out = fields.clone();
    let mut changed = false;
    for (key, value) in fields {
        let (Some(text), Some(kind)) = (
            value.as_str(),
            spec.input_schema["properties"][key]["type"].as_str(),
        ) else {
            continue;
        };
        if kind != "array" && kind != "object" {
            continue;
        }
        let trimmed = text.trim_start();
        if !trimmed.starts_with('[') && !trimmed.starts_with('{') {
            continue;
        }
        let parsed = serde_json::from_str::<Value>(text).map_err(|e| {
            let before: usize = text
                .split_inclusive('\n')
                .take(e.line().saturating_sub(1))
                .map(str::len)
                .sum();
            let at = (before + e.column().saturating_sub(1)).min(text.len());
            let from = (at.saturating_sub(60)..=at)
                .find(|i| text.is_char_boundary(*i))
                .unwrap_or(0);
            let to = (at..=text.len().min(at + 20))
                .rev()
                .find(|i| text.is_char_boundary(*i))
                .unwrap_or(at);
            format!(
                "{key} arrived as text that is not valid JSON ({e}, near …{}⟨here⟩{}…). Count the brackets and send {key} as a JSON {kind}, not a string; a smaller call is easier to get right",
                &text[from..at],
                &text[at..to]
            )
        })?;
        if (kind == "array") == parsed.is_array() && (kind == "object") == parsed.is_object() {
            out.insert(key.clone(), parsed);
            changed = true;
        }
    }
    Ok(changed.then_some(Value::Object(out)))
}

/// The report a `submit_review` call carries. Local models often send its
/// fields without the `report` wrapper, and its lists as JSON text: both
/// are read as meant, so the model is refused only for what is wrong.
fn report_input(input: &Value) -> Value {
    let mut report = if input["report"].is_object() {
        input["report"].clone()
    } else if let Some(text) = input["report"].as_str() {
        serde_json::from_str(text).unwrap_or(Value::Null)
    } else if input.get("part").is_some() {
        input.clone()
    } else {
        Value::Null
    };
    if let Some(fields) = report.as_object_mut() {
        for key in ["sources", "values", "open_questions"] {
            let parsed = fields
                .get(key)
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .filter(Value::is_array);
            if let Some(list) = parsed {
                fields.insert(key.to_owned(), list);
            }
        }
    }
    report
}

/// A small valid report, shown with a refusal so the model sees every field
/// name at once instead of one serde error at a time.
fn report_example() -> Value {
    json!({"report": {
        "part": "PART-NAME",
        "summary": "What was built and what to check first.",
        "sources": [{"name": "DS", "title": "Datasheet title", "revision": "Rev. C"}],
        "values": [{"item": "pad pitch", "value": "0.95 mm",
            "source": {"document": "DS", "page": 35, "location": "Figure 9-2"},
            "confidence": "high", "note": ""}],
        "open_questions": ["A question a reviewer must settle."],
    }})
}

/// A part-file tool's `values`, an empty object when absent.
fn values(input: &Value) -> Value {
    if input["values"].is_object() {
        input["values"].clone()
    } else {
        json!({})
    }
}

/// Upsert `items` into the array `key` of `doc`, matching on `field`.
fn upsert(
    doc: &mut Map<String, Value>,
    key: &str,
    field: &str,
    items: &[Value],
    limit: usize,
) -> Result<(usize, usize), String> {
    let list = doc
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(list) = list else {
        return Err(format!("{key} is not a list"));
    };
    let (mut added, mut replaced) = (0, 0);
    for item in items {
        let Some(id) = item[field].as_str() else {
            return Err(format!("each of {key} needs a text {field}"));
        };
        match list.iter_mut().find(|x| x[field].as_str() == Some(id)) {
            Some(slot) => {
                *slot = item.clone();
                replaced += 1;
            }
            None => {
                list.push(item.clone());
                added += 1;
            }
        }
    }
    if list.len() > limit {
        return Err(format!("a pinout lists at most {limit} {key}"));
    }
    Ok((added, replaced))
}

fn remove(doc: &mut Map<String, Value>, key: &str, field: &str, ids: &[Value]) -> usize {
    let Some(Value::Array(list)) = doc.get_mut(key) else {
        return 0;
    };
    let before = list.len();
    list.retain(|x| !ids.iter().any(|id| x[field] == *id));
    before - list.len()
}

/// `library_pinout_edit`: change the working document, all or nothing.
fn edit(workspace: &mut Workspace, input: &Value) -> ToolOutcome {
    let mut doc = if input["clear"] == true {
        Map::new()
    } else {
        workspace.pinout.clone()
    };
    let list = |key: &str| input[key].as_array().cloned().unwrap_or_default();
    let pins = list("pins");
    if pins.len() > MAX_EDIT_PINS {
        return reply(
            json!({"refused": format!("send at most {MAX_EDIT_PINS} pin rows a call; split the rows over several calls")}),
            true,
            None,
        );
    }
    let mut changes = BTreeMap::new();
    let outcome = (|| -> Result<(), String> {
        for key in ["format", "device"] {
            if !input[key].is_null() {
                doc.insert(key.to_owned(), input[key].clone());
            }
        }
        if let Some(variants) = input["variants"].as_array() {
            if variants.len() > MAX_PINOUT_VARIANTS {
                return Err(format!("at most {MAX_PINOUT_VARIANTS} variants"));
            }
            doc.insert("variants".into(), Value::Array(variants.clone()));
        }
        let peripherals = list("peripherals");
        let (a, r) = upsert(
            &mut doc,
            "peripherals",
            "label",
            &peripherals,
            MAX_PINOUT_PERIPHERALS,
        )?;
        changes.insert("peripherals added", a);
        changes.insert("peripherals replaced", r);
        let (a, r) = upsert(&mut doc, "pins", "row", &pins, MAX_PINOUT_PINS)?;
        changes.insert("pins added", a);
        changes.insert("pins replaced", r);
        let (a, r) = upsert(&mut doc, "rules", "label", &list("rules"), MAX_PINOUT_RULES)?;
        changes.insert("rules added", a);
        changes.insert("rules replaced", r);
        changes.insert(
            "pins removed",
            remove(&mut doc, "pins", "row", &list("remove_pins")),
        );
        changes.insert(
            "rules removed",
            remove(&mut doc, "rules", "label", &list("remove_rules")),
        );
        Ok(())
    })();
    if let Err(why) = outcome {
        return reply(
            json!({"refused": why, "note": "the working document is unchanged"}),
            true,
            None,
        );
    }
    changes.retain(|_, n| *n > 0);
    workspace.pinout = doc;
    reply(
        json!({"changed": changes, "document": document_summary(&workspace.pinout)}),
        false,
        None,
    )
}

/// Counts of the working document and, per variant, the pin numbers it
/// does not yet list (when the variant numbers its terminals 1…N).
fn document_summary(doc: &Map<String, Value>) -> Value {
    let count = |key: &str| doc.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    let pins = doc
        .get("pins")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut variants = Map::new();
    for v in doc
        .get("variants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = v["name"].as_str() else {
            continue;
        };
        let total = v["terminal_count"].as_u64().unwrap_or(0);
        let mut listed: BTreeMap<String, usize> = BTreeMap::new();
        for p in &pins {
            if let Some(n) = p["number"][name].as_str() {
                *listed.entry(n.to_owned()).or_default() += 1;
            }
        }
        let missing: Vec<u64> = (1..=total.min(MAX_PINOUT_PINS as u64))
            .filter(|n| !listed.contains_key(&n.to_string()))
            .collect();
        let repeated: Vec<&String> = listed
            .iter()
            .filter(|(_, c)| **c > 1)
            .map(|(n, _)| n)
            .collect();
        variants.insert(
            name.to_owned(),
            json!({"terminal_count": total, "numbered": listed.len(), "missing": ranges(&missing), "repeated": repeated}),
        );
    }
    json!({
        "has_header": doc.contains_key("format") && doc.contains_key("device") && doc.contains_key("variants"),
        "pins": pins.len(),
        "peripherals": count("peripherals"),
        "rules": count("rules"),
        "variants": variants,
    })
}

/// "1-5, 9, 12-14".
fn ranges(numbers: &[u64]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut it = numbers.iter().copied().peekable();
    while let Some(start) = it.next() {
        let mut end = start;
        while it.peek() == Some(&(end + 1)) {
            end += 1;
            it.next();
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
    }
    out.join(", ")
}

/// `library_pinout_preview`: the working document (or the given one,
/// which replaces it) through `library.pinout_preview`, compacted.
fn preview(
    session: &mut Session,
    policy: &Policy,
    workspace: &mut Workspace,
    input: &Value,
) -> ToolOutcome {
    if let Some(document) = input["document"].as_object() {
        workspace.pinout = document.clone();
    }
    if workspace.pinout.is_empty() {
        return reply(
            json!({"refused": "the working pinout document is empty: build it with library_pinout_edit first"}),
            true,
            None,
        );
    }
    let document = Value::Object(workspace.pinout.clone());
    let response = send(
        session,
        policy,
        json!({"command": "project.query", "params": {
            "query": "library.pinout_preview",
            "params": {"target": input["target"], "document": document},
        }}),
    );
    if response["ok"] != true {
        workspace.previewed = None;
        return reply(response["error"].clone(), true, None);
    }
    let result = &response["result"];
    let plan = result.get("result").unwrap_or(result);
    workspace.previewed = Some((input["target"].clone(), plan["base"].clone(), document));
    reply(compact_plan(plan), false, None)
}

/// The target and document `library_pinout_apply` imports: the given
/// ones, or the last preview's, refusing a working document changed since.
fn apply_inputs(workspace: &Workspace, input: &Value) -> Result<(Value, Value, Value), String> {
    if input["document"].is_object() {
        return Ok((
            input["target"].clone(),
            input["document"].clone(),
            input["base"].clone(),
        ));
    }
    let Some((target, base, document)) = &workspace.previewed else {
        return Err("preview the pinout first (library_pinout_preview)".into());
    };
    if *document != Value::Object(workspace.pinout.clone()) {
        return Err("the working document changed since the last preview; preview it again".into());
    }
    let given = &input["base"];
    let same = match given {
        Value::Null => true,
        Value::String(text) => text.trim().parse::<u64>().ok() == base.as_u64(),
        other => other == base,
    };
    if !same {
        let float = if given.is_f64() {
            " (it arrived as a rounded number: an integer this long loses digits as a float)"
        } else {
            ""
        };
        return Err(format!(
            "base {given} is not the last preview's base {base}{float}. Leave base out to apply exactly what was last previewed"
        ));
    }
    let target = if input["target"].is_object() {
        input["target"].clone()
    } else {
        target.clone()
    };
    Ok((target, document.clone(), base.clone()))
}

/// A pinout plan without per-object detail: change counts (the changes
/// themselves when few), findings and review items without object lists,
/// and the rule checks.
pub fn compact_plan(plan: &Value) -> Value {
    let changes = plan["changes"].as_array().cloned().unwrap_or_default();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for c in &changes {
        let key = format!(
            "{} {}",
            c["object"].as_str().unwrap_or("?"),
            c["kind"].as_str().unwrap_or("?")
        );
        *counts.entry(key).or_default() += 1;
    }
    let findings: Vec<Value> = plan["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| json!({"code": f["code"], "message": f["message"], "blocks": f["blocks"]}))
        .collect();
    let review = plan["review"].as_array().cloned().unwrap_or_default();
    let mut out = json!({
        "base": plan["base"],
        "change_counts": counts,
        "findings": findings.iter().take(MAX_PLAN_ITEMS).collect::<Vec<_>>(),
        "review": review.iter().take(MAX_PLAN_ITEMS).collect::<Vec<_>>(),
        "rules": plan["rules"].as_array().cloned().unwrap_or_default(),
    });
    if changes.len() <= MAX_PLAN_ITEMS {
        out["changes"] = Value::Array(
            changes
                .iter()
                .map(|c| json!({"kind": c["kind"], "object": c["object"], "row": c["row"], "summary": c["summary"]}))
                .collect(),
        );
    }
    for (key, n) in [("findings", findings.len()), ("review", review.len())] {
        if n > MAX_PLAN_ITEMS {
            out[format!("{key}_not_listed")] = json!(n - MAX_PLAN_ITEMS);
        }
    }
    out
}

fn reply(mut value: Value, error: bool, step: Option<String>) -> ToolOutcome {
    integers_as_text(&mut value);
    let mut text = value.to_string();
    if text.len() > MAX_RESULT_CHARS {
        let mut end = MAX_RESULT_CHARS;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("… [cut: the result exceeded the tool reply limit; ask for a narrower view]");
    }
    ToolOutcome::Reply {
        text,
        error,
        step,
        images: Vec::new(),
    }
}

/// Largest integer a JSON reader that parses numbers as doubles keeps exact.
const MAX_EXACT_INTEGER: u64 = 1 << 53;

/// Integers beyond [`MAX_EXACT_INTEGER`] as text, so a model (or a server
/// that parses JSON numbers as doubles) can't round them: a model once
/// copied a preview's 19-digit base as a float and was refused 54 times.
fn integers_as_text(value: &mut Value) {
    match value {
        Value::Number(n) => {
            let big = n.as_u64().is_some_and(|v| v > MAX_EXACT_INTEGER)
                || n.as_i64()
                    .is_some_and(|v| v.unsigned_abs() > MAX_EXACT_INTEGER);
            if big {
                *value = Value::String(n.to_string());
            }
        }
        Value::Array(items) => items.iter_mut().for_each(integers_as_text),
        Value::Object(map) => map.values_mut().for_each(integers_as_text),
        _ => {}
    }
}

/// A short description of an outcome for the run log: its keys, and how
/// a package replacement matched the terminals.
fn summary(outcome: &Value) -> String {
    match outcome {
        Value::Object(map) => {
            let keys = map.keys().take(4).cloned().collect::<Vec<_>>().join(", ");
            match map
                .get("plan")
                .and_then(|p| p["terminals"]["summary"].as_str())
            {
                Some(terminals) => format!("{keys}; {terminals}"),
                None => keys,
            }
        }
        Value::String(s) => s.clone(),
        other => other.to_string().chars().take(80).collect(),
    }
}

/// Each draft of a snapshot as id, kind, name, publishable and findings.
pub fn drafts(snapshot: &Value) -> Vec<Value> {
    snapshot["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            let findings: Vec<Value> = d["check"]["result"]["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .take(MAX_FINDINGS)
                .map(|f| json!({"code": f["code"], "message": f["message"], "blocks": f["blocks"]}))
                .collect();
            json!({
                "id": d["draft"]["id"],
                "kind": d["draft"]["definition"]["kind"],
                "name": d["draft"]["meta"]["name"],
                "publishable": d["check"]["result"]["publishable"],
                "findings": findings,
            })
        })
        .collect()
}

fn inspect(session: &Session, input: &Value) -> ToolOutcome {
    let snapshot = session.snapshot();
    let found = snapshot["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| d["draft"]["id"] == input["id"]);
    match found {
        Some(d) => reply(d.clone(), false, None),
        None => reply(
            json!({"error": format!("no draft {}", input["id"])}),
            true,
            None,
        ),
    }
}

fn package_schema() -> Value {
    json!({
        "frame": "Seen from above; x across the module (width), y along it (length); the antenna end is +y; the origin is the outline centre; millimetres. Rows number counter-clockwise: left rows downwards (-y), bottom rows towards +x, right rows upwards, top rows towards -x; pads number from 1 through the rows in the order listed, then the ground array (one terminal), then each single pad.",
        "request": {"draft": "the package draft id", "row": {"source": "custom", "family": "module"}, "values": {"family": "module", "...": "the fields below"}},
        "schema": schemars::schema_for!(sparksmith_core::library::package_form::ModuleForm),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_closed_object_schema_and_a_unique_name() {
        let tools = definitions();
        let mut names = std::collections::BTreeSet::new();
        for t in &tools {
            assert!(names.insert(t.name.clone()));
            assert_eq!(t.input_schema["type"], "object");
            assert_eq!(t.input_schema["additionalProperties"], false);
        }
        // Nothing that publishes, saves or touches files.
        for name in &names {
            assert!(
                !name.contains("publish") && !name.contains("save") && !name.contains("export")
            );
        }
    }

    #[test]
    fn unknown_tools_and_bad_reports_are_errors() {
        let mut session = Session::default();
        let mut workspace = Workspace::default();
        assert!(matches!(
            execute(
                &mut session,
                &Policy::default(),
                &mut workspace,
                "library_publish",
                &json!({})
            ),
            ToolOutcome::Reply { error: true, .. }
        ));
        assert!(matches!(
            execute(
                &mut session,
                &Policy::default(),
                &mut workspace,
                "submit_review",
                &json!({"report": {"part": "x"}})
            ),
            ToolOutcome::Reply { error: true, .. }
        ));
    }

    /// A session with one empty component draft, so a cited report passes
    /// the review check.
    fn with_a_component_draft() -> Session {
        let mut session = Session::default();
        let body = serde_json::to_value(sparksmith_core::library::ComponentDefinition::default())
            .unwrap_or_default();
        let revision = session.engine().revision();
        let reply = session.respond(
            Caller::Operator,
            json!({"command": "project.apply", "params": {"expected_revision": revision,
                "operation": {"editor": "Library", "operation": {"command": "library.new_draft",
                    "params": {"meta": {"name": "PART-NAME", "description": "", "provenance": ""},
                        "definition": {"kind": "component", "body": body}}}}}}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        session
    }

    #[test]
    fn a_report_the_review_check_refuses_is_not_submitted() {
        // The same check as MCP library.review_check: no component draft.
        let mut session = Session::default();
        let mut workspace = Workspace::default();
        let outcome = execute(
            &mut session,
            &Policy::default(),
            &mut workspace,
            "submit_review",
            &report_example(),
        );
        let ToolOutcome::Reply {
            text, error: true, ..
        } = outcome
        else {
            panic_free_fail();
            return;
        };
        assert!(text.contains("no component draft"), "{text}");
        // A value without its table, figure or section is refused too.
        let mut session = with_a_component_draft();
        let mut report = report_example();
        report["report"]["values"][0]["source"]["location"] = json!("");
        let outcome = execute(
            &mut session,
            &Policy::default(),
            &mut workspace,
            "submit_review",
            &report,
        );
        let ToolOutcome::Reply {
            text, error: true, ..
        } = outcome
        else {
            panic_free_fail();
            return;
        };
        assert!(
            text.contains("needs its table, figure or section"),
            "{text}"
        );
    }

    #[test]
    fn a_report_without_its_wrapper_or_with_lists_as_text_is_read() {
        let mut session = with_a_component_draft();
        let mut workspace = Workspace::default();
        let report = report_example()["report"].clone();
        let flat_and_stringified = json!({
            "part": report["part"], "summary": report["summary"],
            "sources": report["sources"].to_string(),
            "values": report["values"].to_string(),
            "open_questions": report["open_questions"].to_string(),
        });
        for input in [json!({"report": report.clone()}), flat_and_stringified] {
            let outcome = execute(
                &mut session,
                &Policy::default(),
                &mut workspace,
                "submit_review",
                &input,
            );
            assert!(matches!(outcome, ToolOutcome::Report(_)), "{outcome:?}");
        }
        // A wrong field name is refused with the whole expected shape.
        let outcome = execute(
            &mut session,
            &Policy::default(),
            &mut workspace,
            "submit_review",
            &json!({"report": {"part": "X", "summary": "s", "sources": [], "values": [{"name": "x"}]}}),
        );
        let ToolOutcome::Reply {
            text, error: true, ..
        } = outcome
        else {
            panic_free_fail();
            return;
        };
        assert!(
            text.contains("expected_shape") && text.contains("open_questions"),
            "{text}"
        );
    }

    fn panic_free_fail() {
        assert_eq!("a refusal", "something else");
    }

    #[test]
    fn arrays_and_objects_sent_as_text_are_read_as_meant() {
        let input = json!({"edits": "[{\"id\": 5}]", "other": "x"});
        assert_eq!(
            unpack_text_arguments("library_edit", &input),
            Ok(Some(json!({"edits": [{"id": 5}], "other": "x"})))
        );
        // Text that is not the declared kind stays as it came.
        assert_eq!(
            unpack_text_arguments("library_edit", &json!({"edits": "{}"})),
            Ok(None)
        );
        assert_eq!(
            unpack_text_arguments("library_templates", &json!({"query": "[1]"})),
            Ok(None)
        );
        // Unbalanced brackets, as a server's parser leaves a long call: the
        // refusal shows where it broke.
        let broken = json!({"edits": "[{\"id\": 5, \"pads\": [{\"size\": [0.6, 1.1]}}]"});
        let why = unpack_text_arguments("library_edit", &broken)
            .err()
            .unwrap_or_default();
        assert!(
            why.contains("not valid JSON") && why.contains("[0.6, 1.1]}⟨here⟩}]"),
            "{why}"
        );
    }

    #[test]
    fn integers_a_double_would_round_reach_the_model_as_text() {
        let ToolOutcome::Reply { text, .. } = reply(
            json!({"base": 2_049_172_786_040_315_623_u64, "ids": [3, -9_007_199_254_740_993_i64], "pitch": 0.95}),
            false,
            None,
        ) else {
            return;
        };
        assert_eq!(
            text,
            r#"{"base":"2049172786040315623","ids":[3,"-9007199254740993"],"pitch":0.95}"#
        );
    }

    #[test]
    fn missing_numbers_are_listed_as_ranges() {
        assert_eq!(ranges(&[1, 2, 3, 5, 7, 8]), "1-3, 5, 7-8");
        assert_eq!(ranges(&[]), "");
    }
}
