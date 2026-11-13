//! The gate through its public functions: the decision table, the folder
//! limit, a run that was told to publish, and MCP under each mode.
use super::*;
use crate::mcp::{Server, tool_name};
use pcbcraft_core::{Engine, Operation, Query};
use std::collections::BTreeSet;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// `path` made absolute on this platform: Windows needs a drive, so
/// `/work` becomes `C:/work` there.
fn abs(path: &str) -> String {
    if cfg!(windows) {
        format!("C:{path}")
    } else {
        path.to_owned()
    }
}

fn request(command: &str, params: Value) -> Fallible<Request> {
    Ok(serde_json::from_value(
        json!({"command": command, "params": params}),
    )?)
}

fn apply(operation: Value) -> Fallible<Request> {
    request(
        "project.apply",
        json!({"expected_revision": 0, "operation": operation}),
    )
}

/// `run`, `ask` or the refusal's code.
fn verdict(policy: &Policy, request: &Request) -> String {
    match decide(policy, request) {
        Decision::Run => "run".into(),
        Decision::Ask(_) => "ask".into(),
        Decision::Refuse(refusal) => refusal.code.into(),
    }
}

const MODES: [Mode; 3] = [Mode::AskEveryTime, Mode::AutoAllowEdits, Mode::AllowAll];

#[test]
fn each_mode_treats_each_class_as_decided() -> Fallible {
    let read = request("project.inspect", json!({}))?;
    let edit = request("project.undo", json!({"expected_revision": 0}))?;
    let ask = request("project.new", json!({"expected_revision": 0}))?;
    let never = request(
        "library.favourite",
        json!({"key": {"source": "project", "lineage": 1}, "favourite": true}),
    )?;
    for (mode, expected) in [
        (Mode::AskEveryTime, ["run", "ask", "ask"]),
        (Mode::AutoAllowEdits, ["run", "run", "ask"]),
        (Mode::AllowAll, ["run", "run", "run"]),
    ] {
        let policy = Policy::new(mode, []);
        let got = [
            verdict(&policy, &read),
            verdict(&policy, &edit),
            verdict(&policy, &ask),
        ];
        assert_eq!(got, expected, "{mode:?}");
        assert_eq!(verdict(&policy, &never), "gate.never", "{mode:?}");
    }
    assert_eq!(Mode::default(), Mode::AutoAllowEdits);
    Ok(())
}

#[test]
fn project_apply_is_classed_by_its_operation() -> Fallible {
    let policy = Policy::default();
    let edit = apply(json!({"editor": "Library", "operation": {
        "command": "library.discard_draft", "params": {"id": 1}}}))?;
    assert_eq!(edit.effective_class(), Class::Edit);
    assert_eq!(verdict(&policy, &edit), "run");

    let publish = apply(json!({"editor": "Library", "operation": {
        "command": "library.publish", "params": {"drafts": []}}}))?;
    assert_eq!(publish.effective_class(), Class::Ask);
    assert_eq!(publish.subject(), "library.publish");
    assert_eq!(verdict(&policy, &publish), "ask");

    let query: Request = serde_json::from_value(
        json!({"command": "project.query", "params": {"query": "project.bom", "params": {}}}),
    )?;
    assert_eq!(query.effective_class(), Class::Read);
    assert_eq!(query.subject(), "project.bom");
    Ok(())
}

#[test]
fn a_batch_takes_its_strictest_member() -> Fallible {
    let policy = Policy::default();
    let read = request("project.inspect", json!({}))?;
    let edit = request("project.undo", json!({"expected_revision": 0}))?;
    let ask = request("project.new", json!({"expected_revision": 0}))?;
    let never = request(
        "library.source.remove",
        json!({"path": "/somewhere/library"}),
    )?;
    assert!(matches!(
        decide_batch(&policy, &[read.clone(), edit.clone()]),
        Decision::Run
    ));
    let Decision::Ask(question) = decide_batch(&policy, &[read.clone(), ask.clone(), edit]) else {
        return Err("a batch with an Ask member asks".into());
    };
    assert_eq!(question.command, "project.new");
    assert_eq!(question.class, Class::Ask);
    let Decision::Refuse(refusal) = decide_batch(&policy, &[read, ask, never]) else {
        return Err("a batch with a Never member is refused".into());
    };
    assert_eq!(refusal.code, "gate.never");
    Ok(())
}

#[test]
fn a_path_outside_the_folders_is_refused_in_every_mode() -> Fallible {
    let open = |path: &str| {
        request(
            "project.open",
            json!({"expected_revision": 0, "path": path}),
        )
    };
    for mode in MODES {
        let policy = Policy::new(mode, [PathBuf::from(abs("/work/board"))]);
        for outside in [
            abs("/work/other/x.pcbcraft"),
            // A sibling that shares the folder's text is not inside it.
            abs("/work/board-evil/x.pcbcraft"),
            // `..` is refused even where it would land inside.
            abs("/work/board/../other/x.pcbcraft"),
            abs("/work/board/../board/x.pcbcraft"),
            // Relative paths depend on the process's folder.
            "x.pcbcraft".into(),
            "../x.pcbcraft".into(),
            String::new(),
        ] {
            assert_eq!(
                verdict(&policy, &open(&outside)?),
                "gate.path_outside",
                "{outside}"
            );
        }
        let inside = verdict(&policy, &open(&abs("/work/board/sub/x.pcbcraft"))?);
        let expected = if mode == Mode::AllowAll { "run" } else { "ask" };
        assert_eq!(inside, expected);
    }
    // No folders: every path is refused, and the refusal says what to do.
    let file = abs("/work/board/x.pcbcraft");
    let Decision::Refuse(refusal) = decide(&Policy::default(), &open(&file)?) else {
        return Err("no folders allow no path".into());
    };
    let message = refusal.message();
    for part in ["project.open", &file, "Instead:"] {
        assert!(message.contains(part), "{message}");
    }
    Ok(())
}

#[test]
fn naming_a_path_makes_a_request_at_least_ask() -> Fallible {
    let policy = Policy::new(Mode::AutoAllowEdits, [PathBuf::from(abs("/work"))]);
    let builtin = request(
        "library.check_part_file",
        json!({"builtin": "esp32-s31-wroom-3"}),
    )?;
    assert_eq!(builtin.effective_class(), Class::Read);
    assert_eq!(verdict(&policy, &builtin), "run");
    let file = request(
        "library.check_part_file",
        json!({"path": abs("/work/part.solidcraft")}),
    )?;
    assert_eq!(file.effective_class(), Class::Ask);
    assert_eq!(verdict(&policy, &file), "ask");
    let template = request(
        "firmware.export_preview",
        json!({"instance": 1, "emitter": {"emitter": "json"}, "template_file": abs("/elsewhere/t.rhai")}),
    )?;
    assert_eq!(verdict(&policy, &template), "gate.path_outside");
    let no_template = request(
        "firmware.export_preview",
        json!({"instance": 1, "emitter": {"emitter": "json"}}),
    )?;
    assert_eq!(verdict(&policy, &no_template), "run");
    Ok(())
}

/// A content hash for the samples that take one.
const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The paths a sample names: one, or two for the enclosure requests, which
/// name the enclosure and the board's published design.
fn sample_paths(command: &str) -> usize {
    if command.starts_with("housing.") {
        2
    } else {
        1
    }
}

/// One sample per command that can name a path. A new one fails the
/// comparison below until it is listed here and its path reaches
/// [`Request::paths`].
fn path_samples() -> Fallible<Vec<Request>> {
    let at = "/samples/f";
    let mut samples = Vec::new();
    for (command, params) in [
        ("project.open", json!({"expected_revision": 0, "path": at})),
        ("project.save", json!({"path": at})),
        ("project.export_native", json!({"path": at})),
        ("project.export_fabrication", json!({"output": at})),
        ("library.source.add", json!({"path": at})),
        ("library.source.create", json!({"path": at, "name": "n"})),
        ("library.source.remove", json!({"path": at})),
        (
            "library.source.enable",
            json!({"path": at, "enabled": true}),
        ),
        (
            "library.import_from_project",
            json!({"expected_revision": 0, "path": at,
                "root": {"kind": "component", "id": 1, "revision": 1}}),
        ),
        (
            "firmware.export_preview",
            json!({"instance": 1, "emitter": {"emitter": "json"}, "path": at}),
        ),
        (
            "firmware.export",
            json!({"instance": 1, "emitter": {"emitter": "json"}, "path": at}),
        ),
        (
            "library.inspect_model_file",
            json!({"path": at, "format": "gltf"}),
        ),
        ("schematic.export_pdf", json!({"path": at})),
        ("library.check_part_file", json!({"path": at})),
        (
            "library.use_part_file",
            json!({"package": 1, "path": at, "expected_revision": 0}),
        ),
        ("interface.publish_preview", json!({"path": at})),
        (
            "interface.publish",
            json!({"path": at, "expected_revision": 0}),
        ),
        (
            "housing.follow",
            json!({"path": at, "design": at, "board_param": "width",
                "housing_param": "inner", "expected_revision": 0}),
        ),
        ("housing.preview", json!({"path": at, "design": at})),
        (
            "housing.update",
            json!({"path": at, "design": at, "expect_hash": HASH, "expected_revision": 0}),
        ),
    ] {
        samples.push(request(command, params)?);
    }
    samples.push(serde_json::from_value(json!({
        "command": "layout.place_part_file",
        "params": {"path": at, "position": [0.0, 0.0], "expected_revision": 0},
    }))?);
    samples.push(serde_json::from_value(json!({
        "command": "library.import_svg",
        "params": {
            "expected_revision": 0, "draft": 1, "path": at,
            "target": {"unit": 0},
        },
    }))?);
    Ok(samples)
}

fn looks_like_a_path(name: &str) -> bool {
    matches!(name, "path" | "output" | "file" | "folder" | "dir")
        || name.ends_with("_path")
        || name.ends_with("_file")
        || name.ends_with("_folder")
        || name.ends_with("_dir")
}

/// Paths of `schema`'s `properties`, its definitions included, by name.
fn path_properties(schema: &Value, out: &mut BTreeSet<String>) {
    match schema {
        Value::Object(object) => {
            if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                out.extend(properties.keys().filter(|k| looks_like_a_path(k)).cloned());
            }
            object
                .values()
                .for_each(|inner| path_properties(inner, out));
        }
        Value::Array(items) => items.iter().for_each(|inner| path_properties(inner, out)),
        _ => {}
    }
}

/// Independent of `Request::paths`: the commands whose generated schema has a
/// property that names a path are exactly the ones with a sample, and every
/// sample's path is seen, so the gate cannot miss a path a command takes.
#[test]
fn every_path_a_command_can_name_reaches_the_gate() -> Fallible {
    let samples = path_samples()?;
    let sampled: BTreeSet<&str> = samples.iter().map(Request::command).collect();
    let mut with_paths = BTreeSet::new();
    for info in Request::catalog() {
        let mut found = BTreeSet::new();
        path_properties(&info.params, &mut found);
        if !found.is_empty() {
            with_paths.insert(info.id);
        }
    }
    assert_eq!(with_paths, sampled);
    let policy = Policy::new(Mode::AllowAll, [PathBuf::from("/allowed")]);
    for sample in &samples {
        let paths = sample.paths();
        assert_eq!(
            paths,
            vec!["/samples/f"; sample_paths(sample.command())],
            "{}",
            sample.command()
        );
        assert!(
            sample.effective_class() >= Class::Ask,
            "{}",
            sample.command()
        );
        let refused = decide(&policy, sample);
        let Decision::Refuse(refusal) = refused else {
            return Err(format!("{} ran outside the folders", sample.command()).into());
        };
        assert!(
            ["gate.path_outside", "gate.never"].contains(&refusal.code),
            "{}: {}",
            sample.command(),
            refusal.code
        );
    }
    Ok(())
}

/// The class tables and the catalog agree, so nothing falls back to Never.
#[test]
fn every_command_operation_and_query_has_a_class() -> Fallible {
    let listed: BTreeSet<&str> = Request::CLASSES.iter().map(|(name, _)| *name).collect();
    let catalog: BTreeSet<&str> = Request::catalog().iter().map(|info| info.id).collect();
    assert_eq!(listed, catalog);

    let mut session = Session::default();
    let index = session.execute(Request::Schema(Default::default()))?;
    let row_class = |row: &Value| row["class"].as_str().map(str::to_owned);
    let mut operations = 0usize;
    for (_, rows) in index["operations"].as_object().ok_or("operations")? {
        for row in rows.as_array().ok_or("rows")? {
            operations += 1;
            let name = row["name"].as_str().ok_or("name")?;
            let (_, class) = Operation::classes()
                .find(|(listed, _)| *listed == name)
                .ok_or_else(|| format!("{name} has no class"))?;
            let expected = (*class != Class::Edit).then(|| class.name().to_owned());
            assert_eq!(row_class(row), expected, "{name}");
        }
    }
    assert_eq!(operations, Operation::classes().count());
    let queries = index["queries"].as_array().ok_or("queries")?;
    assert_eq!(queries.len(), Query::CLASSES.len());
    for row in queries {
        assert_eq!(row_class(row), None, "{row}");
    }
    for row in index["commands"].as_array().ok_or("commands")? {
        assert!(row["class"].is_string(), "{row}");
    }
    Ok(())
}

#[test]
fn a_model_can_see_what_will_need_asking() -> Fallible {
    let mut session = Session::default();
    let schema = session
        .dispatch(json!({"command": "project.schema", "params": {"name": "library.publish"}}));
    assert_eq!(schema["result"]["matches"][0]["class"], "ask", "{schema}");
    let tools = crate::mcp::tools();
    let description = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .and_then(|tool| tool["description"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    assert!(description("project_save").ends_with("Permission: ask."));
    assert!(description("project_inspect").ends_with("Permission: read."));
    assert!(description("library_favourite").ends_with("Permission: never."));
    assert!(
        description("library_check_part_file").ends_with("Permission: read, or ask with a path.")
    );
    assert!(description("project_apply").contains("project_schema marks"));
    Ok(())
}

/// A session with a publishable draft and a writable library folder, in
/// `root`, as a host would have set up before a run starts.
fn host_session(root: &Path) -> Fallible<(Session, Value, Value, Value, PathBuf)> {
    let mut session = Session::new(Engine::demo()?);
    let folder = root.join("library");
    let created = session.dispatch(json!({"command": "library.source.create", "params": {
        "path": folder.display().to_string(), "name": "Host library"}}));
    assert_eq!(created["ok"], true, "{created}");
    let library = created["result"]["created"]["library"].clone();
    // The demo keeps drafts of its components: start from none.
    let drafts: Vec<Value> = session
        .engine()
        .drafts()
        .items
        .iter()
        .map(|draft| serde_json::to_value(draft.id))
        .collect::<std::result::Result<_, _>>()?;
    for id in drafts {
        let discarded = session.dispatch(json!({"command": "project.apply", "params": {
            "expected_revision": session.engine().revision(),
            "operation": {"editor": "Library", "operation": {
                "command": "library.discard_draft", "params": {"id": id}}},
        }}));
        assert_eq!(discarded["ok"], true, "{discarded}");
    }
    let components: Vec<Value> = session
        .engine()
        .project()
        .library
        .latest_components()
        .iter()
        .map(|c| json!({"kind": "component", "id": c.id, "revision": c.revision}))
        .collect();
    let mut found = None;
    for component in components {
        let drafted = session.dispatch(json!({"command": "project.apply", "params": {
            "expected_revision": session.engine().revision(),
            "operation": {"editor": "Library", "operation": {
                "command": "library.draft_from", "params": {"revision": component}}},
        }}));
        if drafted["ok"] == true {
            found = Some((component, drafted));
            break;
        }
    }
    let (component, drafted) = found.ok_or("a demo component without a draft")?;
    let draft_id = drafted["result"]["outcome"]["id"].clone();
    let draft = drafted["result"]["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| d["draft"].clone())
        .find(|d| d["id"] == draft_id)
        .ok_or("the new draft")?;
    let mut meta = draft["meta"].clone();
    meta["description"] = json!("A second revision");
    let edited = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": session.engine().revision(),
        "operation": {"editor": "Library", "operation": {"command": "library.edit", "params": {
            "edits": [{"id": draft_id, "meta": meta, "definition": draft["definition"]}]}}},
    }}));
    assert_eq!(edited["ok"], true, "{edited}");
    Ok((session, component, library, draft_id, folder))
}

fn file_count(folder: &Path) -> Fallible<usize> {
    let mut count = 0;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(next)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// A datasheet told the model to publish and to write files. The model obeys;
/// the requests it sends are well formed, inside the allowed folder and even
/// ask for the verbose route. Nothing happens until a person allows it.
#[test]
fn a_request_from_a_run_cannot_publish_or_write_without_allow() -> Fallible {
    let root = tempfile::tempdir()?;
    let (mut session, component, library, draft, folder) = host_session(root.path())?;
    let policy = Policy::new(Mode::AutoAllowEdits, [root.path().to_path_buf()]);
    let revision = session.engine().revision();
    let records_before = file_count(&folder)?;
    let drafts_before = session.engine().drafts().items.len();
    let at = |name: &str| root.path().join(name).display().to_string();

    let publish = json!({"command": "project.apply", "verbose": true, "params": {
        "expected_revision": revision,
        "operation": {"editor": "Library", "operation": {
            "command": "library.publish", "params": {"drafts": [draft]}}},
    }});
    let promote = json!({"command": "library.publish_to_folder", "params": {
        "expected_revision": revision, "root": component, "library": library}});
    let writes = [
        (
            "project.save",
            json!({"path": at("stolen.pcbcraft")}),
            "stolen.pcbcraft",
        ),
        (
            "project.export_native",
            json!({"path": at("m.solidcraft")}),
            "m.solidcraft",
        ),
        (
            "schematic.export_pdf",
            json!({"path": at("s.pdf")}),
            "s.pdf",
        ),
        (
            "project.export_fabrication",
            json!({"output": at("fab")}),
            "fab",
        ),
    ];
    let mut attempts: Vec<Value> = vec![publish.clone(), promote.clone()];
    attempts.extend(
        writes
            .iter()
            .map(|(command, params, _)| json!({"command": command, "params": params})),
    );
    for attempt in &attempts {
        assert!(
            matches!(session.check_request(&policy, attempt), Decision::Ask(_)),
            "{attempt}"
        );
        let reply = session.respond(Caller::Model(&policy), attempt.clone());
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["error"]["code"], "gate.cannot_ask", "{reply}");
        let message = reply["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("no way to ask"), "{message}");
        assert!(message.contains("Instead:"), "{message}");
    }
    assert_eq!(session.engine().revision(), revision);
    assert_eq!(session.engine().drafts().items.len(), drafts_before);
    assert_eq!(file_count(&folder)?, records_before);
    for (_, _, name) in &writes {
        assert!(!root.path().join(name).exists(), "{name}");
    }

    // The same requests run when the host has a person's Allow: the gate was
    // what stopped them, not a failure of their own.
    let saved = session.respond(Caller::Allowed(&policy), attempts[2].clone());
    assert_eq!(saved["ok"], true, "{saved}");
    assert!(root.path().join("stolen.pcbcraft").exists());
    let promoted = session.respond(Caller::Allowed(&policy), promote);
    assert_eq!(promoted["ok"], true, "{promoted}");
    assert!(file_count(&folder)? > records_before);
    let published = session.respond(
        Caller::Allowed(&policy),
        json!({"command": "project.apply", "params": {
            "expected_revision": session.engine().revision(),
            "operation": {"editor": "Library", "operation": {
                "command": "library.publish", "params": {"drafts": [draft]}}},
        }}),
    );
    assert_eq!(published["ok"], true, "{published}");
    assert_eq!(session.engine().drafts().items.len(), drafts_before - 1);
    Ok(())
}

#[test]
fn edits_run_inside_the_turn_and_settings_are_refused_with_no_way_around() -> Fallible {
    let root = tempfile::tempdir()?;
    let (mut session, _, _, draft, folder) = host_session(root.path())?;
    let policy = Policy::new(Mode::AllowAll, [root.path().to_path_buf()]);
    let revision = session.engine().revision();
    let sources = session.sources().list().len();
    let reply = session.respond(
        Caller::Model(&policy),
        json!({"command": "library.source.remove",
            "params": {"path": folder.display().to_string()}}),
    );
    assert_eq!(reply["error"]["code"], "gate.never", "{reply}");
    assert_eq!(session.sources().list().len(), sources);

    let reply = session.respond(
        Caller::Model(&Policy::default()),
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Library", "operation": {
                "command": "library.discard_draft", "params": {"id": draft}}},
        }}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(session.engine().revision(), revision + 1);
    Ok(())
}

#[test]
fn saving_without_a_path_counts_as_naming_the_current_file() -> Fallible {
    let root = tempfile::tempdir()?;
    let elsewhere = tempfile::tempdir()?;
    let file = elsewhere.path().join("board.pcbcraft");
    let mut session = Session::new(Engine::demo()?);
    let saved = session.dispatch(
        json!({"command": "project.save", "params": {"path": file.display().to_string()}}),
    );
    assert_eq!(saved["ok"], true, "{saved}");
    let before = std::fs::read(&file)?;
    let policy = Policy::new(Mode::AllowAll, [root.path().to_path_buf()]);
    let request = json!({"command": "project.save", "params": {}});
    let reply = session.respond(Caller::Model(&policy), request.clone());
    assert_eq!(reply["error"]["code"], "gate.path_outside", "{reply}");
    assert_eq!(std::fs::read(&file)?, before);
    let policy = Policy::new(Mode::AllowAll, [elsewhere.path().to_path_buf()]);
    let reply = session.respond(Caller::Model(&policy), request);
    assert_eq!(reply["ok"], true, "{reply}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_link_cannot_lead_out_of_an_allowed_folder() -> Fallible {
    let allowed = tempfile::tempdir()?;
    let secret = tempfile::tempdir()?;
    std::os::unix::fs::symlink(secret.path(), allowed.path().join("door"))?;
    std::os::unix::fs::symlink(
        secret.path().join("not-yet"),
        allowed.path().join("dangling.pcbcraft"),
    )?;
    let session = Session::default();
    let policy = Policy::new(Mode::AllowAll, [allowed.path().to_path_buf()]);
    let check = |path: PathBuf| {
        session.check_request(
            &policy,
            &json!({"command": "project.save", "params": {"path": path.display().to_string()}}),
        )
    };
    for path in [
        allowed.path().join("door/stolen.pcbcraft"),
        allowed.path().join("dangling.pcbcraft"),
    ] {
        let Decision::Refuse(refusal) = check(path.clone()) else {
            return Err(format!("{} passed", path.display()).into());
        };
        assert_eq!(refusal.code, "gate.path_outside");
    }
    // A file not yet made, in a folder not yet made, is still inside.
    assert!(matches!(
        check(allowed.path().join("new/sub/board.pcbcraft")),
        Decision::Run
    ));
    // The allowed folder may itself be reached through a link.
    let via = tempfile::tempdir()?;
    let link = via.path().join("work");
    std::os::unix::fs::symlink(allowed.path(), &link)?;
    let policy = Policy::new(Mode::AllowAll, [link]);
    let reply = session.check_request(
        &policy,
        &json!({"command": "project.save", "params": {
            "path": allowed.path().join("x.pcbcraft").display().to_string()}}),
    );
    assert!(matches!(reply, Decision::Run), "{reply:?}");
    Ok(())
}

#[test]
fn a_malformed_request_is_refused_with_its_parse_error() -> Fallible {
    let session = Session::default();
    let Decision::Refuse(refusal) = session.check_request(
        &Policy::default(),
        &json!({"command": "project.save", "params": {"path": 7}}),
    ) else {
        return Err("a malformed request does not run".into());
    };
    assert_eq!(refusal.code, "malformed_request");
    assert_eq!(refusal.command, "project.save");
    assert!(refusal.message().contains("project_schema"));
    Ok(())
}

#[test]
fn a_denied_question_answers_the_model() -> Fallible {
    let Decision::Ask(question) = decide(
        &Policy::default(),
        &request("project.new", json!({"expected_revision": 0}))?,
    ) else {
        return Err("project.new asks".into());
    };
    assert_eq!(question.denied().code, "gate.denied");
    assert!(question.denied().message().contains("denied"));
    let wire = serde_json::to_value(Decision::Ask(question))?;
    assert_eq!(wire["decision"], "ask");
    assert_eq!(wire["class"], "ask");
    Ok(())
}

fn tool_call(server: &mut Server, id: u64, command: &str, arguments: Value) -> (bool, Value) {
    let reply = server
        .handle(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": tool_name(command), "arguments": arguments}}))
        .unwrap_or(Value::Null);
    let text = reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    (
        reply["result"]["isError"] == true,
        serde_json::from_str(text).unwrap_or(Value::Null),
    )
}

/// MCP has no way to ask, so its gate refuses what would need asking; each
/// mode moves that line, and a server without one allows edits only.
#[test]
fn mcp_runs_under_each_mode_and_allows_edits_by_default() -> Fallible {
    let root = tempfile::tempdir()?;
    let file = |name: &str| root.path().join(name).display().to_string();
    for (mode, undo_runs, save_runs) in [
        (Mode::AskEveryTime, false, false),
        (Mode::AutoAllowEdits, true, false),
        (Mode::AllowAll, true, true),
    ] {
        let name = format!("{}.pcbcraft", mode.name());
        let policy = Policy::new(mode, [root.path().to_path_buf()]);
        let mut server = Server::new(Session::new(Engine::demo()?), policy);

        let (failed, _) = tool_call(&mut server, 1, "project.inspect", json!({}));
        assert!(!failed, "{mode:?}: reads always run");

        let (failed, reply) = tool_call(
            &mut server,
            2,
            "project.undo",
            json!({"expected_revision": 0}),
        );
        if undo_runs {
            // It reaches the engine: the demo has history or says it has none.
            assert_ne!(reply["code"], "gate.cannot_ask", "{mode:?}: {reply}");
        } else {
            assert!(failed);
            assert_eq!(reply["code"], "gate.cannot_ask", "{mode:?}: {reply}");
        }

        let (failed, reply) =
            tool_call(&mut server, 3, "project.save", json!({"path": file(&name)}));
        assert_eq!(failed, !save_runs, "{mode:?}: {reply}");
        assert_eq!(root.path().join(&name).exists(), save_runs, "{mode:?}");
        if !save_runs {
            assert_eq!(reply["code"], "gate.cannot_ask", "{mode:?}: {reply}");
        }

        let (failed, reply) = tool_call(
            &mut server,
            4,
            "project.save",
            json!({"path": "/elsewhere/x.pcbcraft"}),
        );
        assert!(failed);
        assert_eq!(reply["code"], "gate.path_outside", "{mode:?}: {reply}");

        let (failed, reply) = tool_call(
            &mut server,
            5,
            "library.source.remove",
            json!({"path": file("x")}),
        );
        assert!(failed);
        assert_eq!(reply["code"], "gate.never", "{mode:?}: {reply}");
    }

    // I-A1: the CLI's server without a mode, its project's folder allowed:
    // edits run, a save asks and so is refused.
    let policy = Policy::new(Mode::default(), [root.path().to_path_buf()]);
    let mut server = Server::new(Session::new(Engine::demo()?), policy);
    let (failed, reply) = tool_call(
        &mut server,
        1,
        "project.save",
        json!({"path": file("default.pcbcraft")}),
    );
    assert!(failed, "{reply}");
    assert_eq!(reply["code"], "gate.cannot_ask", "{reply}");
    assert!(!root.path().join("default.pcbcraft").exists());
    Ok(())
}

/// I-A1: `library.source.add` writes the person's own settings, so a model
/// never runs it: not over a headless MCP server in its default mode or in
/// allow-all, not over gated JSON lines, not even after a person's Allow.
/// The settings stay unwritten and the source list empty. The request
/// itself is valid: the operator's own script runs it.
#[test]
fn a_model_never_adds_a_library_source() -> Fallible {
    use crate::session::sources::Sources;
    let root = tempfile::tempdir()?;
    let folder = root.path().join("library");
    let created = Session::default().dispatch(json!({"command": "library.source.create",
        "params": {"path": folder.display().to_string(), "name": "Shared"}}));
    assert_eq!(created["ok"], true, "{created}");
    let dirs = pcbcraft_io::settings::UserDirs {
        config: root.path().join("config"),
        cache: root.path().join("cache"),
    };
    let user_session = || -> Fallible<Session> {
        Ok(Session::with_sources(
            Engine::default(),
            Sources::with_dirs(dirs.clone())?,
        ))
    };
    let settings_written = || dirs.config.exists();
    let add = json!({"path": folder.display().to_string()});
    let allow_all = Policy::new(Mode::AllowAll, [root.path().to_path_buf()]);

    for policy in [Policy::default(), allow_all.clone()] {
        let mut server = Server::new(user_session()?, policy.clone());
        let (failed, reply) = tool_call(&mut server, 1, "library.source.add", add.clone());
        assert!(failed, "{:?}: {reply}", policy.mode);
        assert_eq!(reply["code"], "gate.never", "{:?}: {reply}", policy.mode);
        assert!(server.session().sources().list().is_empty());
        assert!(!settings_written(), "{:?}", policy.mode);
    }

    let request = json!({"command": "library.source.add", "params": add});
    let mut session = user_session()?;
    for caller in [Caller::Model(&allow_all), Caller::Allowed(&allow_all)] {
        let reply = session.respond(caller, request.clone());
        assert_eq!(reply["error"]["code"], "gate.never", "{caller:?}: {reply}");
        assert!(session.sources().list().is_empty());
        assert!(!settings_written(), "{caller:?}");
    }
    let reply = session.respond(Caller::Operator, request);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(session.sources().list().len(), 1);
    assert!(settings_written());
    Ok(())
}

/// I-A1: the server instructions never tell a model to call a request,
/// operation or query of the Never class, which every model is refused.
#[test]
fn the_instructions_name_no_never_class_command() {
    let instructions = format!(
        "{}{}",
        crate::mcp::LIVE_INSTRUCTIONS,
        crate::mcp::INSTRUCTIONS
    );
    let never = Request::CLASSES
        .iter()
        .chain(Operation::classes())
        .chain(Query::CLASSES)
        .filter(|(_, class)| *class == Class::Never)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    assert!(never.contains(&"library.source.add"), "{never:?}");
    for name in never {
        for spelling in [name.to_owned(), tool_name(name)] {
            assert!(
                !instructions.contains(&spelling),
                "the instructions name {spelling}, which is never run for a model"
            );
        }
    }
}

/// I-A22: what the catalog says about asking with a path (tool
/// descriptions, schema rows) is exactly the requests whose
/// [`Request::paths`] reach the gate.
#[test]
fn the_catalog_asks_with_a_path_where_the_gate_sees_one() -> Fallible {
    let sampled: BTreeSet<&str> = path_samples()?.iter().map(Request::command).collect();
    let declared: BTreeSet<&str> = Request::NAMING_PATHS.iter().copied().collect();
    assert_eq!(declared, sampled);
    for info in Request::catalog_shared() {
        assert_eq!(
            info.takes_path,
            sampled.contains(info.id),
            "{}: takes_path disagrees with Request::paths",
            info.id
        );
    }
    Ok(())
}
