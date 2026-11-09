//! `project.issues` golden tests: the JSON-lines and MCP transports return
//! exactly the issue list the core query gives the desktop, pinned for the
//! demo project in `golden/issues_demo.json`. Set `PCBCRAFT_BLESS=1` to
//! rewrite the golden file after an intended change, then review the diff.
use super::*;
use crate::mcp::Server;
use pcbcraft_core::issues::style;

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const GOLDEN: &str = "src/session/golden/issues_demo.json";

fn request(scope: Value) -> Value {
    json!({"command": "project.query", "params": {"query": "project.issues", "params": {"scope": scope}}})
}

/// The golden reply, or the current one written as the new golden file
/// when blessing.
fn golden(current: &Value) -> Fallible<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::var_os("PCBCRAFT_BLESS").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(current)? + "\n")?;
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(&path)?)?)
}

#[test]
fn json_lines_issues_of_the_demo_match_the_golden_file() -> Fallible<()> {
    let mut session = Session::new(Engine::demo()?);
    let reply = session.dispatch(request(json!({"kind": "all"})));
    assert_eq!(reply["ok"], true, "{reply}");
    let result = &reply["result"];
    assert_eq!(result, &golden(result)?);
    assert_eq!(result["report"], "issues");
    // The transport passes the core report through unchanged.
    let direct = session.engine().query(pcbcraft_core::Query::Issues {
        scope: pcbcraft_core::issues::IssueScope::All,
    })?;
    assert_eq!(result, &serde_json::to_value(direct)?);
    let rows = result["result"]["rows"].as_array().ok_or("rows")?;
    assert!(!rows.is_empty());
    for row in rows {
        let title = row["title"].as_str().ok_or("title")?;
        assert_eq!(style::check(title), Vec::new(), "{title}");
    }
    Ok(())
}

#[test]
fn mcp_issues_of_the_demo_match_the_golden_file() -> Fallible<()> {
    let mut server = Server::new(
        Session::new(Engine::demo()?),
        crate::gate::Policy::default(),
    );
    let reply = server
        .handle(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "project_query",
                "arguments": {"query": "project.issues", "params": {"scope": {"kind": "all"}}}
            }
        }))
        .ok_or("no reply")?;
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    let content: Value = serde_json::from_str(
        reply["result"]["content"][0]["text"]
            .as_str()
            .ok_or("no text")?,
    )?;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    assert_eq!(content, expected);
    Ok(())
}

#[test]
fn an_area_scope_counts_the_other_areas_and_bad_scopes_are_refused() -> Fallible<()> {
    let mut session = Session::new(Engine::demo()?);
    let board = session.dispatch(request(json!({"kind": "area", "area": "board"})));
    assert_eq!(board["ok"], true, "{board}");
    let report = &board["result"]["result"];
    let rows = report["rows"].as_array().ok_or("rows")?;
    assert!(rows.iter().all(|r| r["area"] == "board"));
    let elsewhere = report["elsewhere"].as_array().ok_or("elsewhere")?;
    assert!(
        elsewhere.iter().any(|a| a["area"] == "schematic"),
        "{report}"
    );
    let unknown = session.dispatch(request(json!({"kind": "area", "area": "garden"})));
    assert_eq!(unknown["ok"], false);
    assert_eq!(unknown["error"]["code"], "malformed_request");
    Ok(())
}

#[test]
fn a_refusal_names_its_rule_beside_the_message() {
    let error = pcbcraft_core::Error::Conflict(
        "footprint.explicit: this footprint has explicit pads".into(),
    );
    let reply = envelope(Err(error.into()));
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "conflict");
    assert_eq!(reply["error"]["rule"], "footprint.explicit");
    assert_eq!(
        reply["error"]["message"],
        "This footprint has explicit pads"
    );
    // A refusal without a rule carries no rule field.
    let plain = envelope(Err(
        pcbcraft_core::Error::History("Nothing to undo".into()).into()
    ));
    assert!(plain["error"].get("rule").is_none(), "{plain}");
}
