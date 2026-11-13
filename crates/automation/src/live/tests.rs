//! The live channel's decisions over a plain session, and its socket end to
//! end: the token, the window's turn and the bridge's tools.
use super::*;
use crate::gate::Mode;
use sparksmith_core::Engine;

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// No token or discovery files: tests never touch the user's folders.
const TEST_APP: App = App {
    name: sparksmith_core::product::NAME,
    data_dir: || None,
    env: |_| None,
    utc: seconds,
    default_port: DEFAULT_PORT,
};

fn policy() -> Policy {
    Policy::new(Mode::AutoAllowEdits, [std::env::temp_dir()])
}

/// `engine.execute` of `layout.add_via` at `revision`.
fn add_via(session: &Session, revision: u64) -> Value {
    let net = session.engine().project().nets.first().map_or(0, |n| n.id);
    json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": {"editor": "Layout", "operation": {"command": "layout.add_via", "params": {
            "via": {"net": net, "position": [1.0, 1.0], "drill": 0.3, "diameter": 0.6, "span": "through", "tented": true}
        }}},
    }})
}

fn code(reply: &Value) -> &str {
    reply["error"]["code"].as_str().unwrap_or_default()
}

fn decide(session: &Session, busy: Option<&str>, params: &Value) -> Verdict {
    admit(session, &policy(), busy, EXECUTE, params)
}

/// A change at the current revision runs and answers compactly; at an older
/// one, without one, or while the person holds the window it is refused and
/// nothing changes. Reads run while the person is busy.
#[test]
fn changes_need_the_current_revision_and_a_free_window() -> Fallible {
    let mut session = Session::new(Engine::demo()?);
    let revision = session.engine().revision();
    let before = session.engine().document().clone();

    let stale = decide(
        &session,
        None,
        &add_via(&session, revision.saturating_sub(1)),
    );
    let Verdict::Reply(stale) = stale else {
        return Err("a stale change was admitted".into());
    };
    assert_eq!(code(&stale), "stale_revision", "{stale}");

    let undo = json!({"command": "project.undo"});
    let Verdict::Reply(missing) = decide(&session, None, &undo) else {
        return Err("an undo without a revision was admitted".into());
    };
    assert_eq!(code(&missing), "live.revision_required", "{missing}");

    let Verdict::Reply(busy) = decide(
        &session,
        Some("The person is typing"),
        &add_via(&session, revision),
    ) else {
        return Err("a change was admitted while the person types".into());
    };
    assert_eq!(code(&busy), "live.busy", "{busy}");
    assert!(
        busy["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("typing"))
    );

    let read = json!({"command": "project.inspect"});
    let Verdict::Run(admitted) = decide(&session, Some("The person is typing"), &read) else {
        return Err("a read was refused while the person types".into());
    };
    assert_eq!(run(&mut session, &admitted)["ok"], true);
    assert_eq!(session.engine().document(), &before);
    assert_eq!(session.engine().revision(), revision);

    let Verdict::Run(admitted) = decide(&session, None, &add_via(&session, revision)) else {
        return Err("a current change was not admitted".into());
    };
    assert_eq!(admitted.subject(), "layout.add_via");
    let reply = run(&mut session, &admitted);
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(
        reply["result"]["changes"]["added"]["vias"].is_array(),
        "{reply}"
    );
    // The review measured 5.4 MB per edit for a whole-project reply.
    assert!(
        reply.to_string().len() < 4 * 1024,
        "{} bytes",
        reply.to_string().len()
    );
    assert_eq!(session.engine().revision(), revision + 1);

    // Undo takes its revision in its own parameters.
    let undo = json!({"command": "project.undo", "params": {"expected_revision": revision + 1}});
    let Verdict::Run(admitted) = decide(&session, None, &undo) else {
        return Err("a current undo was not admitted".into());
    };
    assert_eq!(run(&mut session, &admitted)["ok"], true);
    assert_eq!(session.engine().document(), &before);
    Ok(())
}

/// The window's own file requests are refused; an Ask request is held for
/// the person; a Never one is refused; an unknown method says what exists.
#[test]
fn the_window_decides_files_and_asks() -> Fallible {
    let session = Session::new(Engine::demo()?);
    let revision = session.engine().revision();
    for command in WINDOW_ONLY {
        let request = json!({"command": command, "params": {"expected_revision": revision}});
        let Verdict::Reply(reply) = decide(&session, None, &request) else {
            return Err(format!("{command} was admitted").into());
        };
        assert_eq!(code(&reply), "live.window_only", "{reply}");
    }
    let path = std::env::temp_dir().join("live-export.solidcraft");
    let export = json!({"command": "project.export_native",
        "params": {"path": path.display().to_string()}});
    let Verdict::Ask(admitted, question) = decide(&session, None, &export) else {
        return Err("an export was not held for the person".into());
    };
    assert_eq!(admitted.class(), Class::Ask);
    assert_eq!(question.command, "project.export_native");

    let never = json!({"command": "library.favourite",
        "params": {"key": {"source": "project", "lineage": 1}, "favourite": true}});
    let Verdict::Reply(never) = decide(&session, None, &never) else {
        return Err("a never-class request was admitted".into());
    };
    assert_eq!(code(&never), "gate.never", "{never}");
    let Verdict::Reply(unknown) = admit(&session, &policy(), None, "ui.click", &json!({})) else {
        return Err("an unknown method was admitted".into());
    };
    assert_eq!(code(&unknown), "live.unknown_method");
    let Verdict::Reply(now) = admit(&session, &policy(), Some("busy"), REVISION, &json!({})) else {
        return Err("the revision was not answered".into());
    };
    assert_eq!(now["result"]["revision"], revision);
    Ok(())
}

/// A held change runs only if the project has not moved while it waited.
#[test]
fn an_allowed_change_is_checked_again() -> Fallible {
    let mut session = Session::new(Engine::demo()?);
    let revision = session.engine().revision();
    let Verdict::Run(admitted) = decide(&session, None, &add_via(&session, revision)) else {
        return Err("not admitted".into());
    };
    // The person edits meanwhile.
    let mine = add_via(&session, revision);
    assert_eq!(session.respond(Caller::Operator, mine)["ok"], true);
    let reply = run(&mut session, &admitted);
    assert_eq!(code(&reply), "stale_revision", "{reply}");
    assert_eq!(session.engine().revision(), revision + 1);
    Ok(())
}

/// Over the socket: a wrong token is refused before anything reaches the
/// window; with the token, the window's answer comes back, and the bridge's
/// lost connection says so.
#[test]
fn the_socket_needs_the_token_and_waits_for_the_window() -> Fallible {
    let endpoint = Endpoint::start(TEST_APP, Listen::Exact(0), || {})?;
    let port = endpoint.port();
    let token = |text: &str| ClientToken {
        token: text.to_owned(),
        file: None,
        app: TEST_APP,
    };
    let mut wrong = Client::with_token(port, token(&"0".repeat(64)));
    let refused = wrong.call(REVISION, json!({}));
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.starts_with("unauthorized")),
        "{refused:?}"
    );
    assert!(endpoint.next().is_none());

    let session = Session::new(Engine::demo()?);
    let mut client = Client::with_token(port, token(endpoint.token()));
    let asked = std::thread::spawn(move || client.call(REVISION, json!({})));
    let request = loop {
        if let Some(request) = endpoint.next() {
            break request;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(request.method, REVISION);
    assert_eq!(endpoint.connections(), 1);
    let Verdict::Reply(reply) = admit(&session, &policy(), None, &request.method, &request.params)
    else {
        return Err("the revision was not answered".into());
    };
    request.reply.send(reply)?;
    let answered = asked.join().map_err(|_| "client thread")??;
    assert_eq!(answered["result"]["revision"], session.engine().revision());
    drop(endpoint);
    let mut late = Client::with_token(port, token("x"));
    assert!(late.call(REVISION, json!({})).is_err());
    Ok(())
}

/// The bridge's tools: no window-only file requests, and each change takes
/// `expected_revision` once, from its own parameters.
#[test]
fn live_tools_take_their_own_revision() {
    let tools = crate::mcp::live_tools();
    let tool = |name: &str| tools.iter().find(|t| t["name"] == name).cloned();
    assert!(tool("project_open").is_none() && tool("project_save").is_none());
    let undo = tool("project_undo").unwrap_or_default();
    assert!(
        undo["inputSchema"]["required"]
            .as_array()
            .is_some_and(|r| r.contains(&json!("expected_revision"))),
        "{undo}"
    );
    let apply = tool("project_apply").unwrap_or_default();
    let required = apply["inputSchema"]["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        required
            .iter()
            .filter(|r| *r == "expected_revision")
            .count(),
        1,
        "{apply}"
    );
    let inspect = tool("project_inspect").unwrap_or_default();
    assert!(inspect["inputSchema"]["properties"]["expected_revision"].is_null());
}

/// I-A1: live, `library.source.add` is refused in every mode the window
/// offers, before it is held for the person, and leaves the sources as they
/// were.
#[test]
fn the_window_never_adds_a_library_source() -> Fallible {
    let session = Session::new(Engine::demo()?);
    let request = json!({"command": "library.source.add",
        "params": {"path": std::env::temp_dir().join("lib").display().to_string()}});
    for mode in [Mode::AskEveryTime, Mode::AutoAllowEdits, Mode::AllowAll] {
        let policy = Policy::new(mode, [std::env::temp_dir()]);
        let Verdict::Reply(reply) = admit(&session, &policy, None, EXECUTE, &request) else {
            return Err(format!("{mode:?}: library.source.add was admitted").into());
        };
        assert_eq!(code(&reply), "gate.never", "{mode:?}: {reply}");
    }
    assert!(session.sources().list().is_empty());
    Ok(())
}

/// I-A2: a read that names a path takes no revision. It asks, and on the
/// person's Allow it runs: it is never refused for a revision it cannot
/// send.
#[test]
fn a_read_naming_a_path_runs_on_allow() -> Fallible {
    let mut session = Session::new(Engine::demo()?);
    let dir = tempfile::tempdir_in(std::env::temp_dir())?;
    let part = dir.path().join("smd-nut.solidcraft");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../library/parts/smd-nut.solidcraft"
        ),
        &part,
    )?;
    let check = json!({"command": "library.check_part_file",
        "params": {"path": part.display().to_string()}});
    let Verdict::Ask(admitted, question) = decide(&session, None, &check) else {
        return Err("a path-named read was not held for the person".into());
    };
    assert_eq!(question.command, "library.check_part_file");
    let reply = run(&mut session, &admitted);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["report"]["ok"], true, "{reply}");

    let preview = json!({"command": "firmware.export_preview", "params": {
        "instance": 1, "emitter": {"emitter": "json"},
        "template_file": dir.path().join("t.rhai").display().to_string()}});
    let Verdict::Ask(admitted, _) = decide(&session, None, &preview) else {
        return Err("a preview naming a template was not held".into());
    };
    let reply = run(&mut session, &admitted);
    assert!(
        !code(&reply).starts_with("live.") && !code(&reply).starts_with("gate."),
        "{reply}"
    );
    Ok(())
}

/// I-A3: the wait comes from the effective class: a request inside
/// `project.apply` that asks, a read that names a path and, under
/// ask-every-time, an edit may all wait for the person, so none times out
/// while it is held; a plain read keeps the short wait.
#[test]
fn a_request_that_may_wait_for_the_person_gets_the_long_wait() -> Fallible {
    let session = Session::new(Engine::demo()?);
    let apply = |command: &str, params: Value| {
        json!({"command": "project.apply", "params": {
            "expected_revision": 0,
            "operation": {"editor": "Library", "operation": {"command": command, "params": params}},
        }})
    };
    let publish = apply("library.publish", json!({"drafts": [1]}));
    let path_read = json!({"command": "library.check_part_file",
        "params": {"path": "/tmp/x.solidcraft"}});
    let edit = add_via(&session, 0);
    let read = json!({"command": "project.inspect"});
    let summary = |params: &Value| LiveProtocol.summary(EXECUTE, params);
    for (name, params) in [
        ("publish", &publish),
        ("path read", &path_read),
        ("edit", &edit),
    ] {
        assert!(summary(params).long, "{name}: {params}");
    }
    assert!(matches!(summary(&publish).effect, Effect::Changes));
    assert!(!summary(&read).long);
    assert!(matches!(summary(&read).effect, Effect::ReadsOnly));
    assert!(!LiveProtocol.summary(REVISION, &json!({})).long);
    Ok(())
}
