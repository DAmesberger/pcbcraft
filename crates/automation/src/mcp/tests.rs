use super::*;
use pcbcraft_core::Engine;

fn call(server: &mut Server, id: u64, method: &str, params: Value) -> Value {
    server
        .handle(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        .unwrap_or(Value::Null)
}

/// The JSON a tool call answered with: its one text block.
fn payload(reply: &Value) -> Value {
    assert!(
        reply["result"].get("structuredContent").is_none(),
        "{reply}"
    );
    reply["result"]["content"][0]["text"]
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null)
}

#[test]
fn initialize_negotiates_versions_and_lists_generated_tools() {
    let mut server = Server::new(Session::default());
    let init = call(
        &mut server,
        1,
        "initialize",
        json!({"protocolVersion": "2025-03-26"}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    let init = call(
        &mut server,
        2,
        "initialize",
        json!({"protocolVersion": "1999-01-01"}),
    );
    assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert!(
        server
            .handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .is_none()
    );
    assert!(server.is_initialized());
    let tools = call(&mut server, 3, "tools/list", json!({}));
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"project_apply"));
    assert!(names.contains(&"project_export_native"));
    assert!(names.contains(&"project_query"));
    assert_eq!(names.len(), Request::catalog().len());
}

#[test]
fn tool_calls_share_the_session_and_report_failures_as_tool_errors() {
    let mut server = Server::new(Session::new(Engine::demo().unwrap_or_default()));
    let inspect = call(
        &mut server,
        1,
        "tools/call",
        json!({"name": "project_inspect", "arguments": {}}),
    );
    assert_eq!(inspect["result"]["isError"], false);
    let revision = payload(&inspect)["revision"].clone();
    assert!(revision.is_u64());
    let stale = call(
        &mut server,
        2,
        "tools/call",
        json!({"name": "project_undo", "arguments": {}}),
    );
    assert_eq!(stale["result"]["isError"], true);
    assert_eq!(payload(&stale)["code"], "history_empty");
    let unknown = call(&mut server, 3, "tools/call", json!({"name": "nope"}));
    assert_eq!(unknown["error"]["code"], INVALID_PARAMS);
    let method = call(&mut server, 4, "resources/list", json!({}));
    assert_eq!(method["error"]["code"], METHOD_NOT_FOUND);
}

#[test]
fn serve_bounds_lines_and_survives_bad_input() -> std::io::Result<()> {
    let mut server = Server::new(Session::default());
    let mut input = Vec::new();
    input.extend_from_slice(b"not json\n");
    input.extend(std::iter::repeat_n(b'x', MAX_LINE + 10));
    input.push(b'\n');
    input.extend_from_slice(&[0xff, 0xfe, b'\n']);
    input.extend_from_slice(br#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#);
    input.push(b'\n');
    let mut output = Vec::new();
    server.serve(input.as_slice(), &mut output)?;
    let replies: Vec<Value> = output
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect();
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[0]["error"]["code"], PARSE_ERROR);
    assert_eq!(replies[1]["error"]["code"], INVALID_REQUEST);
    assert_eq!(replies[2]["error"]["code"], PARSE_ERROR);
    assert_eq!(replies[3]["id"], 7);
    Ok(())
}

#[test]
fn query_tools_return_the_engine_report() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::demo()?;
    let instance = engine.project().instances[0].id;
    let direct =
        serde_json::to_value(engine.query(pcbcraft_core::Query::InspectInstance { instance })?)?;
    let mut server = Server::new(Session::new(engine));
    let reply = call(
        &mut server,
        1,
        "tools/call",
        json!({"name": "project_query", "arguments": {"query": "component.inspect", "params": {"instance": instance}}}),
    );
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert_eq!(payload(&reply), direct);
    Ok(())
}

/// `library.edit_graphics` over MCP is one atomic tool call: a batch whose
/// last edit fails is reported as a tool error and changes nothing; the
/// same batch without it applies once and lists the new graphic IDs.
#[test]
fn graphic_edits_over_mcp_are_atomic() -> Result<(), Box<dyn std::error::Error>> {
    use pcbcraft_core::library::Definition;
    let engine = Engine::demo()?;
    let symbol = engine
        .drafts()
        .items
        .iter()
        .find(|d| matches!(d.definition, Definition::Symbol(_)))
        .map(|d| d.id)
        .ok_or("the demo has a symbol draft")?;
    let unit = match &engine.drafts().get(symbol).ok_or("draft")?.definition {
        Definition::Symbol(s) => s.units[0].id,
        _ => return Err("not a symbol".into()),
    };
    let mut server = Server::new(Session::new(engine));
    let before = server.session().engine().document().clone();
    let revision = server.session().engine().revision();
    let add = json!({"add": {"target": {"unit": unit}, "item": {"shape": {
        "shape": {"polyline": {"points": [[0, 0], [5, 0], [5, 5]], "radii": [0, 1, 0]}},
        "width": 0.25, "fill": "none"}}}});
    let apply = |edits: Value, revision: u64| {
        json!({"name": "project_apply", "arguments": {
            "expected_revision": revision,
            "operation": {"editor": "Library", "operation": {
                "command": "library.edit_graphics",
                "params": {"draft": symbol, "edits": edits}}}}})
    };
    let failed = call(
        &mut server,
        1,
        "tools/call",
        apply(json!([add.clone(), {"remove": {"id": 9999}}]), revision),
    );
    assert_eq!(failed["result"]["isError"], true, "{failed}");
    assert_eq!(payload(&failed)["code"], "not_found", "{failed}");
    assert_eq!(server.session().engine().document(), &before);
    assert_eq!(server.session().engine().revision(), revision);
    let applied = call(&mut server, 2, "tools/call", apply(json!([add]), revision));
    assert_eq!(applied["result"]["isError"], false, "{applied}");
    let outcome = payload(&applied)["outcome"].clone();
    assert_eq!(outcome["outcome"], "graphics_edited", "{outcome}");
    assert_eq!(outcome["added"].as_array().map(Vec::len), Some(1));
    assert_eq!(server.session().engine().revision(), revision + 1);
    Ok(())
}

/// The library-folder requests are MCP tools with generated schemas, and a
/// refused source is a tool error carrying the diagnostic code.
#[test]
fn library_folder_tools_are_listed_and_report_refusals() {
    let mut server = Server::new(Session::default());
    let tools = call(&mut server, 1, "tools/list", json!({}));
    let listed: Vec<_> = tools["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for name in [
        "library_sources",
        "library_source_add",
        "library_bundle",
        "library_place_from_folder",
        "library_import_from_folder",
        "library_publish_to_folder",
    ] {
        assert!(listed.contains(&name), "{name}");
    }
    let sources = call(
        &mut server,
        2,
        "tools/call",
        json!({"name": "library_sources", "arguments": {}}),
    );
    assert_eq!(sources["result"]["isError"], false);
    assert_eq!(payload(&sources)["sources"], json!([]));
    let missing = call(
        &mut server,
        3,
        "tools/call",
        json!({"name": "library_source_add", "arguments": {"path": "/nonexistent/pcbcraft-library"}}),
    );
    assert_eq!(missing["result"]["isError"], true);
    assert_eq!(payload(&missing)["code"], "not_found", "{missing}");
}

/// Every wire ID the instructions name (`library.pinout_apply`,
/// `project.pin_report`, …) is a request, command or query this build
/// accepts (or a registered finding code such as `pinout.uncertain`), and every tool name they name is listed, so an agent following
/// them never meets an unknown tool.
#[test]
fn the_instructions_name_only_requests_and_tools_that_exist() {
    let catalog = Request::catalog();
    let wire: String = catalog
        .iter()
        .map(|c| format!("\"{}\" {}", c.id, c.params))
        .collect();
    let tool_names: Vec<String> = catalog.iter().map(|c| tool_name(c.id)).collect();
    // Dotted words that are prose or a refusal's rule, not wire IDs;
    // finding codes are checked against the registered codes.
    let prose = ["purchasing.state", "e.g", "library.other_revision"];
    let finding = |word: &str| pcbcraft_core::issues::KNOWN_CODES.contains(&word);
    let words = INSTRUCTIONS.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'));
    let mut named = 0;
    for word in words.map(|w| w.trim_matches('.')) {
        let dotted = word.contains('.')
            && word.split('.').all(|part| {
                !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            });
        if dotted && !prose.contains(&word) && !finding(word) {
            assert!(
                wire.contains(&format!("\"{word}\"")),
                "unknown wire ID {word}"
            );
            named += 1;
        }
        let tool = ["project_", "library_", "firmware_"]
            .iter()
            .any(|prefix| word.starts_with(prefix));
        if tool {
            assert!(tool_names.iter().any(|t| t == word), "unknown tool {word}");
        }
    }
    assert!(named > 20, "the check found the IDs ({named})");
}

/// The instructions walk an agent through the reviewed datasheet
/// workflow: schema, preview, review, apply, the publish gate, the
/// footprint from a package row and its model, suggest,
/// assign, pin configuration, the report and the firmware export.
#[test]
fn the_instructions_describe_the_reviewed_pinout_workflow() -> super::scenario::Fallible<()> {
    let mut server = Server::new(Session::default());
    let init = call(&mut server, 1, "initialize", json!({}));
    let text = init["result"]["instructions"].as_str().unwrap_or_default();
    let mut last = 0;
    for step in [
        "library_pinout_schema",
        "uncertain: true",
        "library.pinout_preview",
        "review items",
        "library.pinout_apply",
        "pinout.uncertain",
        "unreviewed count",
        "library.templates",
        "library.package_plan",
        "library.package_from_template",
        "footprint_bound",
        "binding.footprint_unbound",
        "library.models",
        "library.publish",
        "library.assign_terminals",
        "functions.suggest",
        "instance.assign_peripheral",
        "instance.set_pin_config",
        "project.pin_report",
        "project.pin_config",
        "firmware_export_preview",
        "firmware_export ",
        "never configures clocks",
    ] {
        let at = text[last..]
            .find(step)
            .ok_or_else(|| format!("{step} after byte {last}"))?;
        last += at;
    }
    assert!(text.contains("never infers electrical compatibility from matching names"));
    assert!(text.contains("never split a name such as USART1_TX"));
    Ok(())
}

/// The pinout and firmware requests are generated tools; the schema tool
/// answers now, and the preview through `project_query` answers the EXM-48
/// document with its one review item.
#[test]
fn the_pinout_tools_answer_over_mcp() -> super::scenario::Fallible<()> {
    use super::scenario::EXM48;
    let mut server = Server::new(Session::new(Engine::demo()?));
    let tools = call(&mut server, 1, "tools/list", json!({}));
    let listed: Vec<_> = tools["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for name in [
        "library_pinout_schema",
        "firmware_export_preview",
        "firmware_export",
    ] {
        assert!(listed.contains(&name), "{name}");
    }
    let schema = call(
        &mut server,
        2,
        "tools/call",
        json!({"name": "library_pinout_schema", "arguments": {}}),
    );
    assert_eq!(schema["result"]["isError"], false, "{schema}");
    assert_eq!(payload(&schema)["format"], 1);

    let document: Value = serde_json::from_str(EXM48)?;
    let before = server.session().engine().document().clone();
    let revision = server.session().engine().revision();
    let plan = call(
        &mut server,
        3,
        "tools/call",
        json!({"name": "project_query", "arguments": {"query": "library.pinout_preview",
            "params": {"target": {"target": "new", "name": "EXM-48"}, "document": document}}}),
    );
    assert_eq!(server.session().engine().revision(), revision);
    assert_eq!(server.session().engine().document(), &before);
    assert_eq!(plan["result"]["isError"], false, "{plan}");
    let content = &payload(&plan);
    let review = content["result"]["review"].as_array().ok_or("review")?;
    assert_eq!(review.len(), 1, "{plan}");
    assert_eq!(review[0]["row"], "r20");
    assert_eq!(review[0]["reason"], "uncertain");
    Ok(())
}

/// Every `$ref` in a value names a definition in `defs`, or one in `more`.
fn refs_of(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, inner) in object {
                match (key.as_str(), inner.as_str()) {
                    ("$ref", Some(target)) => {
                        out.push(target.trim_start_matches("#/$defs/").to_owned());
                    }
                    _ => refs_of(inner, out),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| refs_of(item, out)),
        _ => {}
    }
}

fn schema_call(server: &mut Server, arguments: Value) -> Value {
    call(
        server,
        1,
        "tools/call",
        json!({"name": "project_schema", "arguments": arguments}),
    )
}

/// The list an agent loads stays small, is valid on its own (no dangling
/// reference) and offers `verbose` everywhere.
#[test]
fn the_tool_list_is_small_and_self_contained() {
    let mut server = Server::new(Session::default());
    let list = call(&mut server, 1, "tools/list", json!({}));
    let bytes = list.to_string().len();
    // 45 KB measured; the whole-schema list was 625 KB.
    assert!(bytes < 64 * 1024, "tools/list is {bytes} bytes");
    let tools = list["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(tools.len(), Request::catalog().len());
    for tool in &tools {
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{}", tool["name"]);
        assert!(
            schema["properties"]["verbose"].is_object(),
            "{}",
            tool["name"]
        );
        let mut refs = Vec::new();
        refs_of(schema, &mut refs);
        for target in refs {
            assert!(
                schema["$defs"].get(&target).is_some(),
                "{}: {target}",
                tool["name"]
            );
        }
    }
    // `verbose` is the transport's: no command may claim it for itself.
    for info in Request::catalog() {
        assert!(
            info.params["properties"].get("verbose").is_none(),
            "{}",
            info.id
        );
    }
}

/// Every operation and query the engine's generated schema holds is named in
/// the short tool schemas and served by `project_schema`, with every
/// definition it reaches either in the reply or readable through `def`.
#[test]
fn every_operation_and_query_is_reachable_on_demand() -> super::scenario::Fallible<()> {
    let schemas = pcbcraft_core::schemas();
    let mut names = Vec::new();
    // Independent of the code under test: every tag constant in the schemas.
    fn constants(value: &Value, tag: &str, out: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if let Some(name) = object
                    .get("properties")
                    .and_then(|p| p.get(tag))
                    .and_then(|t| t.get("const"))
                    .and_then(Value::as_str)
                {
                    out.push(name.to_owned());
                }
                object.values().for_each(|inner| constants(inner, tag, out));
            }
            Value::Array(items) => items.iter().for_each(|inner| constants(inner, tag, out)),
            _ => {}
        }
    }
    let operation = schemas["operation"].clone();
    let root = operation["oneOf"].as_array().ok_or("operation oneOf")?;
    for editor in root {
        let target = editor["properties"]["operation"]["$ref"]
            .as_str()
            .and_then(|r| r.strip_prefix("#/$defs/"))
            .ok_or("editor content")?;
        constants(&operation["$defs"][target], "command", &mut names);
    }
    let operations = names.len();
    assert!(operations > 100, "found the operations ({operations})");
    constants(&schemas["query"], "query", &mut names);
    assert!(names.len() > operations + 30, "found the queries");

    let mut server = Server::new(Session::default());
    let list = call(&mut server, 1, "tools/list", json!({}));
    let text = list.to_string();
    for name in names.iter().chain(&["update".to_owned()]) {
        assert!(text.contains(name.as_str()), "{name} is not listed");
        let reply = schema_call(&mut server, json!({"name": name}));
        assert_eq!(reply["result"]["isError"], false, "{name}: {reply}");
        let answer = payload(&reply);
        let matches = answer["matches"].as_array().ok_or("matches")?;
        assert!(!matches.is_empty(), "{name}");
        for found in matches {
            assert!(found["schema"].is_object(), "{name}: {found}");
            // Walk the references, reading `def` for what did not fit.
            let mut held = found["$defs"].as_object().cloned().unwrap_or_default();
            let mut pending = Vec::new();
            refs_of(&found["schema"], &mut pending);
            let mut seen = std::collections::BTreeSet::new();
            while let Some(target) = pending.pop() {
                if !seen.insert(target.clone()) {
                    continue;
                }
                let def = match held.get(&target) {
                    Some(def) => def.clone(),
                    None => {
                        let more = found["more_defs"].to_string();
                        let read = schema_call(&mut server, json!({"name": name, "def": target}));
                        assert_eq!(
                            read["result"]["isError"], false,
                            "{name}/{target} (more_defs {more})"
                        );
                        let read = payload(&read);
                        if let Some(extra) = read["$defs"].as_object() {
                            held.extend(extra.clone());
                        }
                        read["schema"].clone()
                    }
                };
                refs_of(&def, &mut pending);
            }
        }
    }
    Ok(())
}

/// An agent can act on what the schema tool says: fill the `send` shape with
/// schema-conforming parameters and the operation applies; the compact reply
/// names the new via.
#[test]
fn the_schema_reply_says_how_to_send_the_operation() -> super::scenario::Fallible<()> {
    let mut server = Server::new(Session::new(Engine::demo()?));
    let reply = schema_call(&mut server, json!({"name": "layout.add_via"}));
    let answer = payload(&reply);
    let found = &answer["matches"][0];
    assert_eq!(found["kind"], "operation");
    assert_eq!(found["editor"], "Layout");
    assert!(found["$defs"]["ViaRequest"].is_object(), "{found}");
    let mut arguments = found["send"].clone();
    arguments["expected_revision"] = json!(server.session().engine().revision());
    arguments["operation"]["operation"]["params"] = json!({"via": {
        "net": server.session().engine().project().nets[0].id,
        "position": [1.0, 1.0], "drill": 0.3, "diameter": 0.6, "span": "through", "tented": true,
    }});
    let applied = call(
        &mut server,
        2,
        "tools/call",
        json!({"name": "project_apply", "arguments": arguments}),
    );
    assert_eq!(applied["result"]["isError"], false, "{applied}");
    let done = payload(&applied);
    assert_eq!(
        done["changes"]["added"]["vias"],
        json!([done["outcome"]["via"]])
    );
    assert!(applied.to_string().len() < 8 * 1024, "{applied}");

    // The tool's own name works, as does a query's; unknown names are refused.
    let tool = payload(&schema_call(&mut server, json!({"name": "project_query"})));
    assert_eq!(tool["matches"][0]["kind"], "command");
    let query = payload(&schema_call(&mut server, json!({"name": "library.search"})));
    assert_eq!(query["matches"][0]["kind"], "query");
    assert_eq!(query["matches"][0]["send"]["query"], "library.search");
    let unknown = schema_call(&mut server, json!({"name": "layout.nope"}));
    assert_eq!(unknown["result"]["isError"], true);
    assert_eq!(payload(&unknown)["code"], "not_found");
    let orphan = schema_call(&mut server, json!({"def": "Via"}));
    assert_eq!(payload(&orphan)["code"], "invalid_input");
    let missing = schema_call(
        &mut server,
        json!({"name": "layout.add_via", "def": "Nope"}),
    );
    assert_eq!(payload(&missing)["code"], "not_found");

    let index = payload(&schema_call(&mut server, json!({})));
    assert!(
        index["operations"]["Layout"]
            .as_array()
            .is_some_and(|l| l.iter().any(|row| row["name"] == "layout.add_via"))
    );
    Ok(())
}

/// The snapshot goes out only when asked for, and then once.
#[test]
fn verbose_over_mcp_returns_the_snapshot_once() -> super::scenario::Fallible<()> {
    let mut server = Server::new(Session::new(Engine::demo()?));
    let brief = call(
        &mut server,
        1,
        "tools/call",
        json!({"name": "project_inspect", "arguments": {}}),
    );
    assert!(payload(&brief).get("project").is_none(), "{brief}");
    let full = call(
        &mut server,
        2,
        "tools/call",
        json!({"name": "project_inspect", "arguments": {"verbose": true}}),
    );
    let body = payload(&full);
    assert!(body["project"].is_object(), "{full}");
    assert_eq!(full["result"]["content"].as_array().map(Vec::len), Some(1));
    Ok(())
}
