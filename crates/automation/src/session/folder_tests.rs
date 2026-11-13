//! Library folders (S3) over the JSON transport: sources in per-user
//! settings, refused manifests listed with their reason, favourites that
//! persist, and the evidence scenario (install a folder, place from it,
//! publish r+1 in the folder, update only R1, delete the folder, reopen
//! offline) driven through `Session::dispatch` only.
//!
//! The library content is the original synthetic `EX-` catalog.
use super::*;
use sources::Sources;
use sparksmith_io::library_folder::{FOLDER_FORMAT, MANIFEST_FILE};
use sparksmith_io::settings::{SourceSetting, UserDirs, UserSettings, write_settings};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Per-user directories inside a temporary directory.
fn dirs(root: &Path) -> UserDirs {
    UserDirs {
        config: root.join("config"),
        cache: root.join("cache"),
    }
}

fn user_session(engine: Engine, dirs: &UserDirs) -> Fallible<Session> {
    Ok(Session::with_sources(
        engine,
        Sources::with_dirs(dirs.clone())?,
    ))
}

fn ok(session: &mut Session, command: &str, params: Value) -> Fallible<Value> {
    let reply = session.dispatch(json!({"command": command, "params": params}));
    assert_eq!(reply["ok"], true, "{command}: {reply}");
    Ok(reply["result"].clone())
}

fn refused(session: &mut Session, command: &str, params: Value, code: &str) -> Value {
    let revision = session.engine().revision();
    let document = session.engine().document().clone();
    let reply = session.dispatch(json!({"command": command, "params": params}));
    assert_eq!(reply["ok"], false, "{command}: {reply}");
    assert_eq!(reply["error"]["code"], code, "{command}: {reply}");
    assert_eq!(session.engine().revision(), revision);
    assert_eq!(session.engine().document(), &document);
    reply["error"].clone()
}

fn query(session: &mut Session, name: &str, params: Value) -> Fallible<Value> {
    let result = ok(
        session,
        "project.query",
        json!({"query": name, "params": params}),
    )?;
    Ok(result["result"].clone())
}

fn manifest(dir: &Path, text: &str) -> Fallible<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(MANIFEST_FILE), text)?;
    Ok(())
}

/// A manifest of this build's format with `features` required.
fn manifest_json(format: u32, features: &[&str]) -> String {
    json!({
        "format": format,
        "library": "000000000000000000000000000000aa",
        "name": "Refused",
        "required_features": features,
    })
    .to_string()
}

#[test]
fn catalog_lists_the_folder_requests_with_generated_schemas() -> TestResult {
    let catalog = Request::catalog();
    let schema = serde_json::to_string(&schemas())?;
    for id in [
        "library.sources",
        "library.source.add",
        "library.source.create",
        "library.source.remove",
        "library.source.enable",
        "library.rescan",
        "library.favourite",
        "library.bundle",
        "library.import_from_folder",
        "library.place_from_folder",
        "library.publish_to_folder",
    ] {
        let command = catalog
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| format!("{id} is missing"))?;
        assert!(command.params.is_object(), "{id}");
        assert!(schema.contains(id), "{id} is in the request schema");
    }
    // The core operations and queries the requests compose are generated
    // into the document schema too.
    for id in [
        "library.import",
        "library.place_from",
        "library.promote",
        "library.duplicate",
        "library.promote_preview",
        "library.updates_available",
        "other_revision",
    ] {
        assert!(schema.contains(id), "{id} is in the schema");
    }
    Ok(())
}

/// Settings that cannot be read are refused, and a session started without
/// them (as the CLI does) refuses source changes instead of keeping them in
/// memory and reporting them saved. The file is never written.
#[test]
fn unreadable_settings_refuse_source_changes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let dirs = dirs(temp.path());
    std::fs::create_dir_all(&dirs.config)?;
    let text = "{\"format\":1,\"sources\":[";
    std::fs::write(dirs.settings_path(), text)?;
    let error = Sources::with_dirs(dirs.clone())
        .err()
        .ok_or("unreadable settings were read")?;
    let mut session = Session::with_sources(
        Engine::default(),
        Sources::unavailable(error.diagnostic().message),
    );
    let folder = temp.path().join("team");
    manifest(&folder, &manifest_json(FOLDER_FORMAT, &[]))?;
    let error = refused(
        &mut session,
        "library.source.add",
        json!({"path": folder}),
        "io_error",
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("nothing was saved")),
        "{error}"
    );
    assert_eq!(
        ok(&mut session, "library.sources", json!({}))?["sources"],
        json!([])
    );
    assert_eq!(std::fs::read_to_string(dirs.settings_path())?, text);
    Ok(())
}

/// A missing folder and manifests of another format, with an unknown
/// required feature or malformed are refused by `library.source.add` and
/// change nothing; configured in settings they stay listed with the reason.
#[test]
fn refused_manifests_are_listed_with_their_reason() -> TestResult {
    let temp = tempfile::tempdir()?;
    let dirs = dirs(temp.path());
    let future = temp.path().join("future");
    let featured = temp.path().join("featured");
    let broken = temp.path().join("broken");
    let missing = temp.path().join("missing");
    // Settings refuse a repeated path, so "turned off" gets its own folder.
    let off = temp.path().join("off");
    manifest(&future, &manifest_json(FOLDER_FORMAT + 1, &[]))?;
    manifest(&off, &manifest_json(FOLDER_FORMAT + 1, &[]))?;
    manifest(
        &featured,
        &manifest_json(FOLDER_FORMAT, &["future-feature"]),
    )?;
    manifest(&broken, "{\"format\": 1, \"library\": ")?;

    let mut session = user_session(Engine::default(), &dirs)?;
    let add = |path: &Path| json!({"path": path.display().to_string()});
    refused(
        &mut session,
        "library.source.add",
        add(&missing),
        "not_found",
    );
    let error = refused(
        &mut session,
        "library.source.add",
        add(&future),
        "unsupported",
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("unsupported format v2")),
        "{error}"
    );
    let error = refused(
        &mut session,
        "library.source.add",
        add(&featured),
        "unsupported",
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("future-feature")),
        "{error}"
    );
    refused(
        &mut session,
        "library.source.add",
        add(&broken),
        "invalid_input",
    );
    assert_eq!(
        ok(&mut session, "library.sources", json!({}))?["sources"],
        json!([])
    );
    assert!(
        !dirs.settings_path().exists(),
        "a refused source writes no settings"
    );

    // Configured by hand (or by another build), each stays listed.
    let setting = |path: &Path, enabled| SourceSetting {
        path: path.to_path_buf(),
        enabled,
        writable: false,
        name: None,
    };
    write_settings(
        &dirs.settings_path(),
        &UserSettings {
            sources: vec![
                setting(&missing, true),
                setting(&future, true),
                setting(&featured, true),
                setting(&broken, true),
                setting(&off, false),
            ],
            ..UserSettings::default()
        },
    )?;
    let mut session = user_session(Engine::default(), &dirs)?;
    let listed = ok(&mut session, "library.sources", json!({}))?;
    let reasons: Vec<&str> = listed["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            assert_eq!(s["state"], "disabled", "{s}");
            s["reason"].as_str().unwrap_or_default()
        })
        .collect();
    assert_eq!(reasons.len(), 5, "{listed}");
    assert_eq!(reasons[0], "path missing");
    assert_eq!(reasons[1], "unsupported format v2");
    assert_eq!(reasons[2], "requires unsupported features: future-feature");
    assert!(reasons[3].starts_with("unreadable manifest: "), "{listed}");
    assert_eq!(reasons[4], "turned off");
    // Disabled sources add nothing to searches.
    let search = query(&mut session, "library.search", json!({"query": "EX-"}))?;
    assert_eq!(search["total"], 0);
    Ok(())
}

/// Two configured folders holding one library id (a copied folder): the
/// first is searched, the second is listed as failed and never searched.
#[test]
fn a_second_folder_of_the_same_library_is_not_searched() -> TestResult {
    let root = tempfile::tempdir()?;
    let dirs = dirs(root.path());
    let first = root.path().join("first");
    let copy = root.path().join("copy");
    manifest(&first, &manifest_json(FOLDER_FORMAT, &[]))?;
    manifest(&copy, &manifest_json(FOLDER_FORMAT, &[]))?;
    let setting = |path: &Path| SourceSetting {
        path: path.to_path_buf(),
        enabled: true,
        writable: false,
        name: None,
    };
    write_settings(
        &dirs.settings_path(),
        &UserSettings {
            sources: vec![setting(&first), setting(&copy)],
            ..UserSettings::default()
        },
    )?;
    let mut session = user_session(Engine::default(), &dirs)?;
    let listed = ok(&mut session, "library.sources", json!({}))?;
    assert_eq!(listed["sources"][0]["state"], "ready", "{listed}");
    assert_eq!(listed["sources"][1]["state"], "failed", "{listed}");
    let message = listed["sources"][1]["error"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(message.contains("same library as"), "{listed}");
    // Adding the copy again through the request is refused outright.
    let mut fresh = Session::with_sources(Engine::default(), Sources::default());
    ok(
        &mut fresh,
        "library.source.add",
        json!({"path": first.display().to_string()}),
    )?;
    let reply = fresh.dispatch(json!({
        "command": "library.source.add",
        "params": {"path": copy.display().to_string()},
    }));
    assert_eq!(reply["ok"], false, "{reply}");
    Ok(())
}

/// Favourites are keyed by `EntryKey`, persist in the user's settings
/// (never in a project) and come back in a new session.
#[test]
fn favourites_persist_per_user() -> TestResult {
    let temp = tempfile::tempdir()?;
    let dirs = dirs(temp.path());
    let key = json!({
        "source": "library",
        "library": "0000000000000000000000000000000b",
        "lineage": "0000000000000000000000000000000c",
    });
    let mut session = user_session(Engine::demo()?, &dirs)?;
    let revision = session.engine().revision();
    let marked = ok(
        &mut session,
        "library.favourite",
        json!({"key": key, "favourite": true}),
    )?;
    assert_eq!(marked["favourites"], json!([key]));
    assert_eq!(
        session.engine().revision(),
        revision,
        "not a document change"
    );
    assert!(!session.engine().can_undo());

    let mut again = user_session(Engine::demo()?, &dirs)?;
    assert_eq!(
        serde_json::to_value(&again.sources().settings().favourites)?,
        json!([key])
    );
    let cleared = ok(
        &mut again,
        "library.favourite",
        json!({"key": key, "favourite": false}),
    )?;
    assert_eq!(cleared["favourites"], json!([]));
    let text = std::fs::read_to_string(dirs.settings_path())?;
    assert!(text.ends_with('\n'));
    Ok(())
}

/// Publishing needs a ready, writable source and the current revision; a
/// refusal writes nothing and changes nothing.
#[test]
fn publish_to_folder_refuses_unknown_sources_and_stale_revisions() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let component = session
        .engine()
        .project()
        .library
        .latest_components()
        .first()
        .map(|c| DefinitionRef {
            kind: sparksmith_core::DefinitionKind::Component,
            id: c.id,
            revision: c.revision,
        })
        .ok_or("the demo has a component")?;
    let library = "000000000000000000000000000000dd";
    let revision = session.engine().revision();
    refused(
        &mut session,
        "library.publish_to_folder",
        json!({"expected_revision": revision, "root": component, "library": library}),
        "not_found",
    );
    refused(
        &mut session,
        "library.publish_to_folder",
        json!({"expected_revision": revision + 1, "root": component, "library": library}),
        "stale_revision",
    );
    refused(
        &mut session,
        "library.place_from_folder",
        json!({
            "expected_revision": revision,
            "library": library,
            "lineage": "000000000000000000000000000000ee",
            "position": [0.0, 0.0],
        }),
        "not_found",
    );
    Ok(())
}

/// One component entry of a folder in `library.search` results.
fn folder_component(session: &mut Session, library: &Value, text: &str) -> Fallible<Value> {
    let search = query(
        session,
        "library.search",
        json!({"query": text, "limit": 500}),
    )?;
    search["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|hit| hit["entry"].clone())
        .find(|entry| entry["source"]["library"] == *library && entry["kind"] == "component")
        .ok_or_else(|| format!("no folder component for {text}: {search}").into())
}

fn instance<'a>(snapshot: &'a Value, designator: &str) -> Fallible<&'a Value> {
    snapshot["project"]["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["designator"] == designator)
        .ok_or_else(|| format!("no {designator}").into())
}

/// Milestone 0E / test case 9 through JSON only: install a folder, place R1
/// and R2 from it, publish r+1 in the folder from the authoring project,
/// update only R1, detect a modified record by hash, delete the folder and
/// reopen the project offline.
#[test]
fn install_place_publish_update_delete_and_reopen_offline() -> TestResult {
    let temp = tempfile::tempdir()?;
    let folder = temp.path().join("team-library");
    let folder_text = folder.display().to_string();
    let project_path = temp.path().join("board.sparksmith");
    let designer_dirs = dirs(&temp.path().join("designer"));

    // The author writes the synthetic catalog to a new folder.
    let mut author = Session::new(Engine::from_file(EditorFile {
        version: sparksmith_core::format::FILE_VERSION,
        project: sparksmith_core::catalog::synthetic::project_with_parts(6)?,
        drafts: Drafts::default(),
    })?);
    let sample = publish_library(&mut author, &folder_text, "Team library")?;
    let library = sample["library"].clone();
    assert!(
        sample["records_written"]
            .as_array()
            .is_some_and(|r| !r.is_empty()),
        "{sample}"
    );
    assert!(folder.join(MANIFEST_FILE).is_file());
    assert_eq!(sample["sources"][0]["state"], "ready", "{sample}");
    assert_eq!(sample["sources"][0]["writable"], true);
    // Every project revision the author promoted now has an origin.
    let origins = author.snapshot()["project"]["library"]["origins"].clone();
    assert!(
        origins.as_array().is_some_and(|o| !o.is_empty()),
        "{origins}"
    );
    // Publishing again writes nothing: every record is unchanged.
    let again = publish_library_again(&mut author, &library)?;
    assert_eq!(again, 0, "re-promoting writes no record");

    // Install: the designer adds the folder (read-only) to their settings.
    let mut designer = user_session(Engine::default(), &designer_dirs)?;
    let added = ok(
        &mut designer,
        "library.source.add",
        json!({"path": folder_text}),
    )?;
    assert_eq!(added["added"]["state"], "ready", "{added}");
    assert_eq!(added["added"]["library"], library);
    assert_eq!(added["added"]["writable"], false);
    assert_eq!(added["added"]["index"], "rebuilt");
    let entry = folder_component(&mut designer, &library, "EX-R")?;
    let lineage = entry["source_lineage"].clone();
    assert!(lineage.is_string(), "{entry}");

    // Place R1 and R2 from the folder; the second reuses the first copy.
    let revision = designer.engine().revision();
    let place = |revision: u64, x: f64| {
        json!({
            "expected_revision": revision,
            "library": library,
            "lineage": lineage,
            "position": [x, 0.0],
        })
    };
    let first = ok(
        &mut designer,
        "library.place_from_folder",
        place(revision, 0.0),
    )?;
    assert_eq!(first["outcome"]["outcome"], "placed_from", "{first}");
    let first_import = first["outcome"]["import"].clone();
    let local = first_import["root"].clone();
    assert_eq!(local["revision"], 1, "{first_import}");
    assert!(
        first_import["revisions"]
            .as_array()
            .is_some_and(|r| r.iter().all(|i| i["reused"] == false)),
        "{first_import}"
    );
    let components_after_first = first["project"]["library"]["components"].clone();
    let second = ok(
        &mut designer,
        "library.place_from_folder",
        place(first["revision"].as_u64().ok_or("revision")?, 10.0),
    )?;
    if let Some(import) = second["outcome"]["import"].as_object() {
        assert!(
            import["revisions"]
                .as_array()
                .is_some_and(|r| r.iter().all(|i| i["reused"] == true)),
            "{second}"
        );
    }
    assert_eq!(
        second["project"]["library"]["components"], components_after_first,
        "an equal origin is reused, never copied twice"
    );
    let r1 = instance(&second, "R1")?.clone();
    let r2 = instance(&second, "R2")?.clone();
    assert_eq!(r1["component"], r2["component"]);
    assert_eq!(r1["component"]["id"], local["id"]);
    // Placing from a folder records a per-user recent, not a project field.
    assert!(
        designer.sources().settings().recent.len() == 1,
        "the placed lineage is recent"
    );
    assert!(
        updates_available(&mut designer)?.is_empty(),
        "nothing newer yet"
    );

    // Publish r+1 in the folder: the author drafts the same lineage,
    // changes its description, publishes and promotes it.
    let author_root = author_component(&author, &library, &lineage)?;
    let drafted = apply(
        &mut author,
        json!({"editor": "Library", "operation": {
            "command": "library.draft_from", "params": {"revision": author_root},
        }}),
    )?;
    let draft_id = drafted["outcome"]["id"].clone();
    let draft = drafted["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| d["draft"].clone())
        .find(|d| d["id"] == draft_id)
        .ok_or("the new draft")?;
    let mut meta = draft["meta"].clone();
    meta["description"] = json!("Synthetic resistor, second revision");
    apply(
        &mut author,
        json!({"editor": "Library", "operation": {"command": "library.edit", "params": {"edits": [
            {"id": draft_id, "meta": meta, "definition": draft["definition"]},
        ]}}}),
    )?;
    let published = apply(
        &mut author,
        json!({"editor": "Library", "operation": {
            "command": "library.publish", "params": {"drafts": [draft_id]},
        }}),
    )?;
    let revision_two = json!({
        "kind": "component",
        "id": author_root["id"],
        "revision": 2,
    });
    assert!(
        published["project"]["library"]["components"]
            .to_string()
            .contains("Synthetic resistor, second revision")
    );
    let author_revision = author.engine().revision();
    let promoted = ok(
        &mut author,
        "library.publish_to_folder",
        json!({
            "expected_revision": author_revision,
            "root": revision_two,
            "library": library,
        }),
    )?;
    let lineage_text = lineage.as_str().ok_or("lineage")?;
    let r0002 = format!("lineages/{lineage_text}/r0002.json");
    assert!(
        promoted["written"]["records"]
            .as_array()
            .is_some_and(|r| r.iter().any(|p| p == r0002.as_str())),
        "{promoted}"
    );
    assert!(folder.join(&r0002).is_file());
    assert!(
        folder
            .join(format!("lineages/{lineage_text}/r0001.json"))
            .is_file(),
        "r1 stays"
    );

    // The designer rescans and sees the update for the copied lineage.
    let rescanned = ok(&mut designer, "library.rescan", json!({}))?;
    assert_eq!(rescanned["sources"][0]["state"], "ready", "{rescanned}");
    let updates = updates_available(&mut designer)?;
    assert_eq!(updates.len(), 1, "{updates:?}");
    assert_eq!(updates[0]["local"], local);
    assert_eq!(updates[0]["latest"], 2);
    assert_eq!(updates[0]["modified"], json!([]));

    // Update only R1: import r2 as the next project revision of the same
    // lineage, then preview and apply the revision change for R1 alone.
    let designer_revision = designer.engine().revision();
    let imported = ok(
        &mut designer,
        "library.import_from_folder",
        json!({
            "expected_revision": designer_revision,
            "library": library,
            "lineage": lineage,
            "revision": 2,
        }),
    )?;
    let target = imported["outcome"]["report"]["root"].clone();
    assert_eq!(target["id"], local["id"], "same project lineage");
    assert_eq!(target["revision"], 2);
    let change = json!({"kind": "ComponentRevision", "params": {
        "instances": [r1["id"]],
        "target": {"id": target["id"], "revision": target["revision"]},
    }});
    let preview = ok(
        &mut designer,
        "project.preview_update",
        json!({"change": change}),
    )?;
    assert!(
        preview["blockers"].as_array().is_some_and(Vec::is_empty),
        "{preview}"
    );
    let updated = ok(
        &mut designer,
        "project.apply_update",
        json!({
            "expected_revision": preview["revision"],
            "change": preview["change"],
            "remove_conflicting_tracks": false,
        }),
    )?;
    assert_eq!(instance(&updated, "R1")?["component"]["revision"], 2);
    assert_eq!(instance(&updated, "R2")?["component"]["revision"], 1);
    assert!(updates_available(&mut designer)?.is_empty());

    // A modified rN is detected by hash, never trusted.
    let path = folder.join(&r0002);
    let text = std::fs::read_to_string(&path)?;
    std::fs::write(
        &path,
        text.replace("second revision", "second revision, edited"),
    )?;
    ok(&mut designer, "library.rescan", json!({}))?;
    let updates = updates_available(&mut designer)?;
    assert_eq!(updates.len(), 1, "{updates:?}");
    assert_eq!(updates[0]["modified"], json!([2]), "the edited r2");

    // Save, delete the folder and reopen offline in a new session.
    ok(
        &mut designer,
        "project.save",
        json!({"path": project_path.display().to_string()}),
    )?;
    let saved = designer.engine().document().clone();
    std::fs::remove_dir_all(&folder)?;
    let mut offline = user_session(Engine::default(), &designer_dirs)?;
    let sources = ok(&mut offline, "library.sources", json!({}))?;
    assert_eq!(sources["sources"][0]["state"], "disabled");
    assert_eq!(sources["sources"][0]["reason"], "path missing");
    let reopened = ok(
        &mut offline,
        "project.open",
        json!({"expected_revision": 0, "path": project_path.display().to_string()}),
    )?;
    assert_eq!(offline.engine().document(), &saved);
    assert_eq!(instance(&reopened, "R1")?["component"]["revision"], 2);
    assert_eq!(instance(&reopened, "R2")?["component"]["revision"], 1);
    assert!(
        reopened["project"]["library"]["origins"]
            .as_array()
            .is_some_and(|o| o.len() >= 2),
        "origins travel in the project"
    );
    for designator in ["R1", "R2"] {
        let id = instance(&reopened, designator)?["id"].clone();
        let report = query(&mut offline, "component.inspect", json!({"instance": id}))?;
        assert!(report["component"].is_object(), "{report}");
    }
    assert!(updates_available(&mut offline)?.is_empty());
    let search = query(&mut offline, "library.search", json!({"query": "EX-R"}))?;
    assert!(
        search["results"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|hit| hit["entry"]["source"] == "project"),
        "{search}"
    );
    Ok(())
}

fn apply(session: &mut Session, operation: Value) -> Fallible<Value> {
    let revision = session.engine().revision();
    ok(
        session,
        "project.apply",
        json!({"expected_revision": revision, "operation": operation}),
    )
}

fn updates_available(session: &mut Session) -> Fallible<Vec<Value>> {
    let result = query(session, "library.updates_available", json!({}))?;
    Ok(result.as_array().cloned().unwrap_or_default())
}

/// The author's project revision whose origin is `lineage` in `library`.
fn author_component(author: &Session, library: &Value, lineage: &Value) -> Fallible<Value> {
    let origins = author.snapshot()["project"]["library"]["origins"].clone();
    origins
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| o["origin"]["library"] == *library && o["origin"]["lineage"] == *lineage)
        .map(|o| o["local"].clone())
        .ok_or_else(|| format!("no author copy of {lineage}: {origins}").into())
}

/// Publish every latest Part and component again; returns records written.
fn publish_library_again(author: &mut Session, library: &Value) -> Fallible<usize> {
    let roots: Vec<DefinitionRef> = author
        .engine()
        .project()
        .library
        .latest_components()
        .iter()
        .map(|c| DefinitionRef {
            kind: sparksmith_core::DefinitionKind::Component,
            id: c.id,
            revision: c.revision,
        })
        .collect();
    let mut written = 0;
    for root in roots {
        let author_revision = author.engine().revision();
        let reply = ok(
            author,
            "library.publish_to_folder",
            json!({
                "expected_revision": author_revision,
                "root": root,
                "library": library,
            }),
        )?;
        written += reply["written"]["records"].as_array().map_or(0, Vec::len);
    }
    Ok(written)
}

/// Reserve a component's five drafts and fill them from `preset`'s
/// generated-model sample, as the Component workspace's templates do.
fn packaged_engine(
    preset: sparksmith_core::library::samples::Preset,
) -> Fallible<(Engine, [DefinitionId; 5])> {
    use sparksmith_core::library::samples::{self, SampleRefs};
    use sparksmith_core::library::{Definition, DefinitionMeta};
    use sparksmith_core::{
        BindingRef, DefinitionKind, FootprintRef, Outcome, PackageRef, SymbolRef,
    };
    let mut engine = Engine::default();
    let mut ids = Vec::new();
    for kind in [
        DefinitionKind::Component,
        DefinitionKind::Symbol,
        DefinitionKind::Package,
        DefinitionKind::Footprint,
        DefinitionKind::Binding,
    ] {
        if let Outcome::DraftCreated { id } = engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta::default(),
                definition: Definition::empty(kind),
            }),
        )? {
            ids.push(id);
        }
    }
    let [component, symbol, package, footprint, binding] = ids[..] else {
        return Err("draft reservation failed".into());
    };
    let first = Revision::FIRST;
    let edits = samples::packaged(
        preset.label(),
        preset,
        SampleRefs {
            component,
            symbol: SymbolRef::new(symbol, first),
            package: PackageRef::new(package, first),
            footprint: FootprintRef::new(footprint, first),
            binding: BindingRef::new(binding, first),
        },
    )?;
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Edit { edits }),
    )?;
    Ok((engine, [component, symbol, package, footprint, binding]))
}

/// M1–M3 × S3: a package whose generated model has a body-role material
/// override and an authored placement is promoted to a folder through io,
/// then imported into another project from the folder's files. The copy's
/// form, envelope, model (builder revision and materials) and placement
/// equal the author's, and its origin is recorded.
#[test]
fn a_generated_package_model_round_trips_through_a_folder() -> TestResult {
    use sparksmith_core::PackageRef;
    use sparksmith_core::library::samples::Preset;
    use sparksmith_core::library::{
        BodyRole, Definition, GeneratedMaterials, ModelPlacement, PackageEdit, PackageModel,
        RoleMaterial,
    };
    use sparksmith_core::materials::MaterialId;
    let temp = tempfile::tempdir()?;
    let folder = temp.path().join("models");
    let folder_text = folder.display().to_string();

    let (mut engine, drafts) = packaged_engine(Preset::Soic8)?;
    let [component, _, package, _, _] = drafts;
    let family_rev = match engine.drafts().get(package).map(|d| &d.definition) {
        Some(Definition::Package(p)) => match &p.model {
            PackageModel::Generated { family_rev, .. } => *family_rev,
            other => return Err(format!("not generated: {other:?}").into()),
        },
        _ => return Err("no package draft".into()),
    };
    let placement = ModelPlacement::from_mm([0.02, -0.01, 0.0], [0.0, 0.0, 0.0])?;
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::EditPackage {
            draft: package,
            edits: vec![
                PackageEdit::SetModel(PackageModel::Generated {
                    family_rev,
                    materials: GeneratedMaterials {
                        roles: vec![RoleMaterial {
                            role: BodyRole::Lead,
                            material: MaterialId::Gold,
                        }],
                    },
                }),
                PackageEdit::SetPlacement(placement),
            ],
        }),
    )?;
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Publish {
            drafts: drafts.to_vec(),
            close: false,
        }),
    )?;
    let authored = engine
        .project()
        .library
        .package(PackageRef::new(package, Revision::FIRST))
        .ok_or("no published package")?
        .definition
        .clone();
    assert_eq!(authored.placement, placement);

    let mut author = Session::new(engine);
    let sample = publish_library(&mut author, &folder_text, "Models")?;
    let library = sample["library"].clone();
    let origin = |session: &Session, id: DefinitionId| {
        session
            .engine()
            .project()
            .library
            .origins
            .iter()
            .find(|o| o.local.id == id)
            .map(|o| o.origin)
    };
    let package_origin = origin(&author, package).ok_or("package not promoted")?;
    let component_origin = origin(&author, component).ok_or("component not promoted")?;
    // The record on disk carries the generated model and its override.
    let record = std::fs::read_to_string(
        folder.join(format!("lineages/{}/r0001.json", package_origin.lineage)),
    )?;
    assert!(
        record.contains("\"generated\"") && record.contains("gold"),
        "{record}"
    );

    let mut designer = user_session(Engine::default(), &dirs(&temp.path().join("designer")))?;
    let added = ok(
        &mut designer,
        "library.source.add",
        json!({"path": folder_text}),
    )?;
    assert_eq!(added["added"]["state"], "ready", "{added}");
    let revision = designer.engine().revision();
    let imported = ok(
        &mut designer,
        "library.import_from_folder",
        json!({
            "expected_revision": revision,
            "library": library,
            "lineage": component_origin.lineage,
            "revision": component_origin.revision,
        }),
    )?;
    let report = &imported["outcome"]["report"];
    assert!(report["root"].is_object(), "{imported}");
    let held = &designer.engine().project().library;
    let copy = held
        .origins
        .iter()
        .find(|o| o.origin.lineage == package_origin.lineage)
        .ok_or("the package's origin is not recorded")?;
    assert_eq!(copy.origin, package_origin);
    let copied = held
        .package(PackageRef::new(copy.local.id, copy.local.revision))
        .ok_or("the package was not imported")?
        .definition
        .clone();
    assert_eq!(copied, authored);
    assert!(matches!(
        &copied.model,
        PackageModel::Generated { materials, .. }
            if materials.roles == [RoleMaterial { role: BodyRole::Lead, material: MaterialId::Gold }]
    ));
    Ok(())
}
