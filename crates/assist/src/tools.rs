//! The tools an assisted run offers the model: the Pcbcraft MCP contract
//! narrowed to authoring one part, executed in-process against an
//! automation [`Session`] through [`Session::dispatch`] (the same request
//! types and command engine the MCP server and CLI use). Each tool names
//! the MCP command it runs. The runner fills `expected_revision` from the
//! session, so the model never handles revisions, and compacts replies:
//! a mutation answers with its outcome and each draft's findings instead
//! of the whole document snapshot.
//!
//! What is deliberately absent: `library.publish`, publishing to folders,
//! saving, opening, file exports and library sources. A run only ever
//! creates drafts; publishing stays with a person (the import review gate).
use crate::report::ReviewReport;
use pcbcraft_automation::Session;
use serde_json::{Value, json};

/// Characters of one tool result sent back; longer results are cut with a
/// note (the model can ask for a narrower view).
pub const MAX_RESULT_CHARS: usize = 120_000;
/// Findings listed per draft in a compact reply.
const MAX_FINDINGS: usize = 40;

/// What a tool call produced.
#[derive(Debug)]
pub enum ToolOutcome {
    /// Text for the model, whether it is an error, and a one-line step
    /// for the run log when the call changed the document.
    Reply {
        text: String,
        error: bool,
        step: Option<String>,
    },
    /// The model submitted its review report: the run is complete.
    Report(Box<ReviewReport>),
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

/// The tool definitions, in a fixed order so the prompt prefix caches.
pub fn definitions() -> Vec<Value> {
    let object = json!({"type": "object"});
    let report = serde_json::to_value(schemars::schema_for!(ReviewReport)).unwrap_or(json!({}));
    vec![
        tool(
            "library_pinout_schema",
            "MCP library_pinout_schema: the JSON schema of the pinout document (format 1) you fill from the datasheet: device and reference, one variant per package with its terminal count (every pad that is a terminal, the ground array counted once), peripherals with roles, one row per pin with its number per variant, functions with explicit peripheral, role and mux code, source page and table, uncertain flags, and Rhai pin rules. Call it once before writing the document.",
            json!({}),
            &[],
        ),
        tool(
            "library_pinout_preview",
            "MCP project_query library.pinout_preview: check a pinout document and plan its import without changing anything. target is {\"target\": \"new\", \"name\": ...} for a new part or {\"target\": \"draft\", \"id\": ...} to re-import onto the component draft. Returns the plan (changes, review items, findings) and its base token for library_pinout_apply. Fix every refusal and blocking finding before applying.",
            json!({"target": object, "document": object}),
            &["target", "document"],
        ),
        tool(
            "library_pinout_apply",
            "MCP project_apply library.pinout_apply: import the previewed document with the plan's base (accept {\"accept\": \"all\"}). Creates component, symbol, package and binding drafts whose imported rows are all unreviewed (the review gate); rows flagged uncertain block publishing until a person clears them. Returns the outcome and each draft's findings.",
            json!({"target": object, "document": object, "base": {"type": "integer"}, "accept": object}),
            &["target", "document", "base"],
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
            "MCP project_apply library.package_from_template: apply the previewed request as one undo step (add \"replace\": true to rebuild a footprint that already has pads).",
            json!({"request": object}),
            &["request"],
        ),
        tool(
            "library_set_marking",
            "MCP library_set_marking: the component draft's display-only top marking on the can: {draft, marking: {items: [{item: text, template: \"...\"} | {item: artwork, svg: \"<svg ...>\", size_mm: [w, h], licence: \"...\"} | {item: polarity, mark: dot}], finish: laser_etch}}. Never copy vendor logos; text items are the safe choice.",
            json!({"draft": {"type": "integer"}, "marking": object}),
            &["draft", "marking"],
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
pub fn execute(session: &mut Session, name: &str, input: &Value) -> ToolOutcome {
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
        "library_pinout_preview" => query(
            "library.pinout_preview",
            json!({"target": input["target"], "document": input["document"]}),
        ),
        "library_pinout_apply" => library(
            "library.pinout_apply",
            json!({
                "target": input["target"],
                "document": input["document"],
                "base": input["base"],
                "accept": if input["accept"].is_object() { input["accept"].clone() } else { json!({"accept": "all"}) },
            }),
        ),
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
        "library_inspect_draft" => return inspect(session, input),
        "library_edit" => library("library.edit", json!({"edits": input["edits"]})),
        "component_check_drafts" => {
            let snapshot = session.snapshot();
            return reply(json!({"drafts": drafts(&snapshot)}), false, None);
        }
        "submit_review" => {
            return match serde_json::from_value::<ReviewReport>(input["report"].clone()) {
                Ok(report) => {
                    let problems = report.problems();
                    if problems.is_empty() {
                        ToolOutcome::Report(Box::new(report))
                    } else {
                        reply(json!({"refused": problems}), true, None)
                    }
                }
                Err(e) => reply(
                    json!({"refused": format!("not a review report: {e}")}),
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
    let response = session.dispatch(request);
    if response["ok"] != true {
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

fn reply(value: Value, error: bool, step: Option<String>) -> ToolOutcome {
    let mut text = value.to_string();
    if text.len() > MAX_RESULT_CHARS {
        let mut end = MAX_RESULT_CHARS;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("… [cut: the result exceeded the tool reply limit; ask for a narrower view]");
    }
    ToolOutcome::Reply { text, error, step }
}

/// A short description of an outcome for the run log.
fn summary(outcome: &Value) -> String {
    match outcome {
        Value::Object(map) => map.keys().take(4).cloned().collect::<Vec<_>>().join(", "),
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
        "schema": schemars::schema_for!(pcbcraft_core::library::package_form::ModuleForm),
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
            assert!(names.insert(t["name"].as_str().unwrap_or_default().to_owned()));
            assert_eq!(t["input_schema"]["type"], "object");
            assert_eq!(t["input_schema"]["additionalProperties"], false);
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
        assert!(matches!(
            execute(&mut session, "library_publish", &json!({})),
            ToolOutcome::Reply { error: true, .. }
        ));
        assert!(matches!(
            execute(
                &mut session,
                "submit_review",
                &json!({"report": {"part": "x"}})
            ),
            ToolOutcome::Reply { error: true, .. }
        ));
    }
}
