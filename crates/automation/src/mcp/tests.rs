use super::*;
use pcbcraft_core::Engine;

fn call(server: &mut Server, id: u64, method: &str, params: Value) -> Value {
    server
        .handle(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
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
    let revision = inspect["result"]["structuredContent"]["revision"].clone();
    assert!(revision.is_u64());
    let stale = call(
        &mut server,
        2,
        "tools/call",
        json!({"name": "project_undo", "arguments": {}}),
    );
    assert_eq!(stale["result"]["isError"], true);
    assert_eq!(
        stale["result"]["structuredContent"]["code"],
        "history_empty"
    );
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
    assert_eq!(reply["result"]["structuredContent"], direct);
    Ok(())
}
