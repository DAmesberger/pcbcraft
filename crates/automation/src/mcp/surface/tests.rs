//! P1 evidence: the `2026-07-28` core through rmcp's own client, the same
//! server on the raw wire against a golden transcript
//! (`golden/core.json`; set `PCBCRAFT_BLESS=1` to rewrite it after an
//! intended change, then review the diff), the session thread behind the
//! gate, and the catalog wiring.
use super::catalog::{self, Unreachable};
use super::wire::SERVER_INFO;
use super::*;
use crate::gate::Mode;
use crate::session::metadata::{self, Kind};
use rmcp::model::{ClientCapabilities, ClientConfig, ErrorCode};
use rmcp::service::ServiceError;
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt};
use serde_json::json;
use std::future::Future;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

const GOLDEN: &str = "src/mcp/surface/golden/core.json";

fn block_on<F: Future>(future: F) -> Fallible<F::Output> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(future))
}

/// A server in the default mode (edits run, files ask), allowed `folder`.
fn server(folder: &Path) -> Fallible<Server> {
    let policy = Policy::new(Mode::AutoAllowEdits, [folder.to_path_buf()]);
    Ok(Server::new(Session::default(), policy)?)
}

#[derive(Clone)]
struct Client;

impl ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("pcbcraft-p1-client", "0"),
        )
    }
}

fn code(result: Result<impl std::fmt::Debug, ServiceError>) -> Fallible<ErrorCode> {
    match result {
        Err(ServiceError::McpError(error)) => Ok(error.code),
        other => Err(format!("expected a protocol error, got {other:?}").into()),
    }
}

/// rmcp's client at `2026-07-28`: discovery, the tools list with its cache
/// fields and `serverInfo`, and the spec's error codes.
#[test]
fn rmcp_client_discovers_and_lists_at_2026_07_28() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let server = server(dir.path())?;
    block_on(async move {
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (read, write) = tokio::io::split(server_io);
        let serving = tokio::spawn(server.serve(read, write));
        let client = Client
            .serve_with_lifecycle(
                client_io,
                ClientLifecycleMode::Discover {
                    preferred_versions: VERSIONS.to_vec(),
                },
            )
            .await?;
        let info = client.peer_info().ok_or("discovery gave server info")?;
        assert_eq!(info.protocol_version, ProtocolVersion::V_2026_07_28);
        assert_eq!(info.server_info.as_ref().ok_or("serverInfo")?.name, NAME);
        assert_eq!(info.instructions.as_deref(), Some(INSTRUCTIONS));
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.prompts.is_none());
        assert!(info.capabilities.resources.is_none());

        let tools = client.list_tools(None).await?;
        assert_eq!(tools.ttl_ms, Some(LIST_TTL_MS));
        assert_eq!(tools.cache_scope, Some(CacheScope::Public));
        assert!(tools.next_cursor.is_none());
        assert_eq!(tools.tools, super::tools());
        let meta = tools.meta.as_ref().ok_or("_meta")?;
        assert_eq!(meta.0[SERVER_INFO]["name"], NAME);

        let unknown = client
            .call_tool(CallToolRequestParams::new("project_inspect"))
            .await;
        assert_eq!(code(unknown)?, ErrorCode::INVALID_PARAMS);
        assert_eq!(
            code(client.list_prompts(None).await)?,
            ErrorCode::METHOD_NOT_FOUND
        );
        assert_eq!(
            code(client.list_resources(None).await)?,
            ErrorCode::METHOD_NOT_FOUND
        );
        client.cancel().await?;
        serving.await??;
        Fallible::Ok(())
    })??;
    Ok(())
}

/// rmcp's client in its `initialize` mode cannot connect: the error names
/// the version.
#[test]
fn an_initializing_client_is_told_the_version() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let server = server(dir.path())?;
    block_on(async move {
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (read, write) = tokio::io::split(server_io);
        let serving = tokio::spawn(server.serve(read, write));
        let connected = Client
            .serve_with_lifecycle(client_io, ClientLifecycleMode::Initialize)
            .await;
        let error = match connected {
            Ok(_) => return Err("a legacy client connected".into()),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("2026-07-28"), "{error}");
        serving.await??;
        Fallible::Ok(())
    })??;
    Ok(())
}

/// The raw wire, one JSON-RPC message per line.
struct Wire {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    read: tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
}

impl Wire {
    fn open(server: Server) -> (Self, tokio::task::JoinHandle<Result<(), String>>) {
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (read, write) = tokio::io::split(server_io);
        let serving = tokio::spawn(server.serve(read, write));
        let (read, write) = tokio::io::split(client_io);
        let wire = Self {
            write,
            read: BufReader::new(read).lines(),
        };
        (wire, serving)
    }

    async fn send(&mut self, line: &str) -> Fallible<()> {
        self.write.write_all(format!("{line}\n").as_bytes()).await?;
        Ok(())
    }

    async fn reply(&mut self) -> Fallible<Value> {
        let line = self.read.next_line().await?.ok_or("the server closed")?;
        Ok(serde_json::from_str(&line)?)
    }
}

/// The `_meta` every `2026-07-28` request carries.
fn meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "wire", "version": "0"},
    })
}

fn request(id: u64, method: &str, params: Value) -> Value {
    let mut params = params;
    params["_meta"] = meta("2026-07-28");
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// The transcript: each line sent, and the reply when one is due. Lines
/// that get none (notifications, unparsable text) are followed by a request,
/// so a reply they should not have had would show as a mismatch.
fn script() -> Vec<(String, bool)> {
    let initialize = json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "legacy", "version": "0"}}});
    let mut old = request(11, "tools/list", json!({}));
    old["params"]["_meta"] = meta("2025-11-25");
    let lines = [
        // Before any request: a legacy notification is dropped, and
        // initialize is refused without ending the connection.
        (
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            false,
        ),
        (initialize.clone(), true),
        (json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}), true),
        (request(4, "server/discover", json!({})), true),
        (request(5, "tools/list", json!({})), true),
        (request(6, "tools/list", json!({"cursor": "next"})), true),
        (
            request(
                7,
                "tools/call",
                json!({"name": "project_inspect", "arguments": {}}),
            ),
            true,
        ),
        (request(8, "prompts/list", json!({})), true),
        (request(9, "resources/list", json!({})), true),
        (
            request(10, "resources/read", json!({"uri": "pcbcraft://catalog"})),
            true,
        ),
        (old, true),
        (
            json!({"jsonrpc": "2.0", "id": 12, "method": "tools/list", "params": {}}),
            true,
        ),
        (request(13, "ping", json!({})), true),
        (
            request(14, "logging/setLevel", json!({"level": "info"})),
            true,
        ),
        (request(15, "pcbcraft/unknown", json!({})), true),
        (
            request(
                16,
                "completion/complete",
                json!({
            "ref": {"type": "ref/prompt", "name": "explain"},
            "argument": {"name": "x", "value": ""}}),
            ),
            true,
        ),
        (
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            false,
        ),
        (json!({"well": "formed", "but": "no message"}), true),
        (initialize, true),
    ];
    let mut script: Vec<(String, bool)> = lines
        .into_iter()
        .map(|(line, reply)| (line.to_string(), reply))
        .collect();
    // Not JSON: ignored, as rmcp and the other official SDKs do.
    script.insert(4, ("{not json".to_owned(), false));
    script.push((request(17, "tools/list", json!({})).to_string(), true));
    script
}

/// The build's version, which `serverInfo` carries, made stable.
fn without_version(reply: Value) -> Fallible<Value> {
    let text = reply.to_string().replace(
        &format!("\"version\":\"{}\"", env!("CARGO_PKG_VERSION")),
        "\"version\":\"<version>\"",
    );
    Ok(serde_json::from_str(&text)?)
}

#[test]
fn the_wire_matches_the_golden_transcript() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let server = server(dir.path())?;
    let transcript = block_on(async move {
        let (mut wire, serving) = Wire::open(server);
        let mut transcript = Vec::new();
        for (line, replies) in script() {
            wire.send(&line).await?;
            let reply = if replies {
                without_version(wire.reply().await?)?
            } else {
                Value::Null
            };
            let sent: Value = serde_json::from_str(&line).unwrap_or(Value::String(line));
            transcript.push(json!({"send": sent, "reply": reply}));
        }
        drop(wire);
        serving.await??;
        Fallible::Ok(Value::Array(transcript))
    })??;

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::var_os("PCBCRAFT_BLESS").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&transcript)? + "\n")?;
    }
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    assert_eq!(transcript, golden);

    // What the transcript must show, whatever the golden file says.
    let reply = |id: u64| {
        transcript
            .as_array()
            .and_then(|t| t.iter().find(|x| x["reply"]["id"] == id))
            .map(|x| x["reply"].clone())
            .unwrap_or(Value::Null)
    };
    for id in [2, 11] {
        assert_eq!(reply(id)["error"]["code"], -32022, "{}", reply(id));
        assert_eq!(
            reply(id)["error"]["data"]["supported"],
            json!(["2026-07-28"])
        );
    }
    let discover = &reply(4)["result"];
    assert_eq!(discover["resultType"], "complete");
    assert_eq!(discover["supportedVersions"], json!(["2026-07-28"]));
    assert_eq!(discover["ttlMs"], LIST_TTL_MS);
    assert_eq!(discover["cacheScope"], "public");
    assert_eq!(discover["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(discover["_meta"][SERVER_INFO]["name"], NAME);
    let list = &reply(5)["result"];
    assert_eq!(list["resultType"], "complete");
    assert_eq!(list["ttlMs"], LIST_TTL_MS);
    assert_eq!(list["cacheScope"], "public");
    assert_eq!(list["_meta"][SERVER_INFO]["name"], NAME);
    assert_eq!(reply(17)["result"], *list, "the list is the same each time");
    for (id, code) in [
        (3, -32601),
        (6, -32602),
        (7, -32602),
        (8, -32601),
        (9, -32601),
    ] {
        assert_eq!(reply(id)["error"]["code"], code, "{}", reply(id));
    }
    for (id, code) in [(10, -32601), (12, -32602), (13, -32601), (14, -32601)] {
        assert_eq!(reply(id)["error"]["code"], code, "{}", reply(id));
    }
    for (id, code) in [(15, -32601), (16, -32601)] {
        assert_eq!(reply(id)["error"]["code"], code, "{}", reply(id));
    }
    Ok(())
}

/// Every job runs on the session thread through the gate as the model: a
/// read runs, an edit runs in the default mode and publishes its revision,
/// a file Ask is refused (no one to ask yet), and Never always is.
#[test]
fn the_desk_runs_every_request_through_the_gate() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let saved = dir.path().join("p1.pcbcraft").display().to_string();
    let server = server(dir.path())?;
    block_on(async move {
        let desk = server.desk().clone();
        let mut revisions = desk.revisions();
        let (project, start) = revisions
            .borrow_and_update()
            .iter()
            .next()
            .map(|(handle, revision)| (handle.clone(), *revision))
            .ok_or("the first project")?;

        let inspect = desk
            .run(|d| d.respond(None, json!({"command": "project.inspect", "params": {}})))
            .await?;
        assert_eq!(inspect["ok"], true, "{inspect}");
        assert_eq!(inspect["result"]["revision"], start);

        let edit = json!({"command": "project.apply", "params": {
            "expected_revision": start,
            "operation": {"editor": "Layout", "operation": {
                "command": "layout.set_board_size", "params": {"size": [40.0, 30.0]}}}}});
        let applied = desk.run(move |d| d.respond(None, edit)).await?;
        assert_eq!(applied["ok"], true, "{applied}");
        revisions.changed().await?;
        assert_eq!(revisions.borrow().get(&project), Some(&(start + 1)));

        // The same edit again is stale: refused, nothing changes.
        let stale = json!({"command": "project.apply", "params": {
            "expected_revision": start,
            "operation": {"editor": "Layout", "operation": {
                "command": "layout.set_board_size", "params": {"size": [50.0, 30.0]}}}}});
        let shown = project.clone();
        let refused = desk
            .run(move |d| (d.respond(None, stale), d.revision(&shown)))
            .await?;
        assert_eq!(refused.0["ok"], false, "{}", refused.0);
        assert_eq!(refused.1, Some(start + 1));

        let save = json!({"command": "project.save", "params": {"path": saved}});
        let asked = save.clone();
        let decided = desk.run(move |d| d.decide(None, &asked)).await?;
        assert!(
            matches!(decided, Ok(crate::gate::Decision::Ask(_))),
            "{decided:?}"
        );
        let refused = desk.run(move |d| d.respond(None, save)).await?;
        assert_eq!(refused["error"]["code"], "gate.cannot_ask", "{refused}");

        let never = json!({"command": "library.source.remove", "params": {"path": "/x"}});
        let refused = desk.run(move |d| d.respond(None, never)).await?;
        assert_eq!(refused["error"]["code"], "gate.never", "{refused}");
        Fallible::Ok(())
    })??;
    assert!(!dir.path().join("p1.pcbcraft").exists());
    Ok(())
}

/// Many callers at once are served one at a time, each once.
#[test]
fn concurrent_jobs_run_one_at_a_time() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let server = server(dir.path())?;
    let ran = block_on(async move {
        let desk = server.desk().clone();
        let calls = (0..desk::QUEUE * 2).map(|i| {
            let desk = desk.clone();
            async move { desk.run(move |d| (i, d.revisions().len())).await }
        });
        let mut ran = Vec::new();
        for call in calls.collect::<Vec<_>>() {
            ran.push(tokio::spawn(call));
        }
        let mut answers = Vec::new();
        for call in ran {
            answers.push(call.await??);
        }
        Fallible::Ok(answers)
    })??;
    let indices: Vec<_> = ran.iter().map(|(i, _)| *i).collect();
    assert_eq!(indices, (0..desk::QUEUE * 2).collect::<Vec<_>>());
    Ok(())
}

#[test]
fn a_session_reply_becomes_a_tool_result() {
    let ok = outcome(&json!({"ok": true, "result": {"revision": 3}}));
    assert_eq!(ok.is_error, Some(false));
    let text = ok.content[0].as_text().map(|t| t.text.clone());
    assert_eq!(text.as_deref(), Some(r#"{"revision":3}"#));
    let failed = outcome(&Unreachable::Unknown("nothing".into()).reply());
    assert_eq!(failed.is_error, Some(true));
    let text = failed.content[0]
        .as_text()
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(text.contains("catalog.unknown"), "{text}");
}

/// The catalog is the M2 metadata: every entry but Never resolves through
/// its own kind only, in catalog order, and Never is unknown.
#[test]
fn the_catalog_resolves_each_entry_by_its_kind() {
    let all = metadata::entries();
    let reachable: Vec<_> = catalog::reachable().map(|e| e.name.as_str()).collect();
    let expected: Vec<_> = all
        .iter()
        .filter(|e| e.class != pcbcraft_core::Class::Never)
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(reachable, expected);
    assert!(reachable.len() < all.len(), "some entries are Never");
    for entry in all {
        match entry.kind {
            None => assert_eq!(
                catalog::resolve(Kind::Action, &entry.name).map(|e| e.name.as_str()),
                Err(Unreachable::Unknown(entry.name.clone()))
            ),
            Some(kind) => {
                let found = catalog::resolve(kind, &entry.name).map(|e| e.name.as_str());
                assert_eq!(found, Ok(entry.name.as_str()));
                for other in [Kind::Query, Kind::Operation, Kind::Action] {
                    if other != kind {
                        let refused = catalog::resolve(other, &entry.name);
                        let code = refused.map(|e| e.name.as_str()).map_err(|e| e.code());
                        assert_eq!(code, Err("catalog.other_kind"));
                    }
                }
            }
        }
    }
    assert!(matches!(
        catalog::resolve(Kind::Query, "project.save"),
        Err(Unreachable::OtherKind {
            kind: Kind::Action,
            ..
        })
    ));
}

#[test]
fn the_instructions_hold_their_budget() {
    assert!(
        INSTRUCTIONS.len() <= crate::mcp::INSTRUCTIONS_TARGET,
        "{}",
        INSTRUCTIONS.len()
    );
}
