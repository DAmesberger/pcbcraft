//! `library.import_from_project` and reproducible library-folder ids (gap 4
//! of the end-to-end run), driven over the JSON contract. Library content is
//! the original synthetic `EX-` catalog and the ESP32-S31-WROOM-3 example.
use super::*;
use sources::Sources;
use sparksmith_core::library::import::{PROJECT_FILES, project_lineage_id};
use sparksmith_core::{DefinitionKind, format::FILE_VERSION};
use sparksmith_io::settings::UserDirs;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MODULE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/esp32-s31-wroom-3/esp32-s31-wroom-3.sparksmith"
);

fn ok(session: &mut Session, command: &str, params: Value) -> Fallible<Value> {
    let reply = session.dispatch(json!({"command": command, "params": params}));
    assert_eq!(reply["ok"], true, "{command}: {reply}");
    Ok(reply["result"].clone())
}

/// A refusal with `code` that leaves document, revision and history alone.
fn refused(session: &mut Session, command: &str, params: Value, code: &str) -> Fallible<String> {
    let revision = session.engine().revision();
    let document = session.engine().document().clone();
    let undo = session.engine().can_undo();
    let reply = session.dispatch(json!({"command": command, "params": params}));
    assert_eq!(reply["ok"], false, "{command}: {reply}");
    assert_eq!(reply["error"]["code"], code, "{command}: {reply}");
    assert_eq!(session.engine().revision(), revision);
    assert_eq!(session.engine().document(), &document);
    assert_eq!(session.engine().can_undo(), undo);
    Ok(reply["error"]["message"].as_str().unwrap_or("").to_owned())
}

fn library_op(session: &mut Session, command: &str, params: Value) -> Fallible<Value> {
    let revision = session.engine().revision();
    ok(
        session,
        "project.apply",
        json!({
            "expected_revision": revision,
            "operation": {"editor": "Library", "operation": {"command": command, "params": params}}
        }),
    )
}

fn import_params(session: &Session, path: &str, root: DefinitionRef) -> Value {
    json!({"expected_revision": session.engine().revision(), "path": path, "root": root})
}

fn synthetic_engine() -> Fallible<Engine> {
    Ok(Engine::from_file(EditorFile {
        version: FILE_VERSION,
        project: sparksmith_core::catalog::synthetic::project_with_parts(3)?,
        drafts: Default::default(),
    })?)
}

fn synthetic_session() -> Fallible<Session> {
    Ok(Session::new(synthetic_engine()?))
}

fn latest_part(session: &Session) -> Fallible<DefinitionRef> {
    let library = &session.engine().project().library;
    let part = library.latest_parts().into_iter().next().ok_or("a Part")?;
    Ok(DefinitionRef {
        kind: DefinitionKind::Part,
        id: part.id,
        revision: part.revision,
    })
}

fn save(session: &mut Session, path: &Path) -> Fallible<String> {
    let text = path.display().to_string();
    ok(session, "project.save", json!({"path": text}))?;
    Ok(text)
}

/// A part with its closure: one undo step, exact source revisions in the
/// origins, usable at once, and still whole after the source file is gone.
#[test]
fn a_part_imports_with_its_closure_as_one_undo_step_and_reopens_offline() -> TestResult {
    let temp = tempfile::tempdir()?;
    let mut source = synthetic_session()?;
    let root = latest_part(&source)?;
    let path = save(&mut source, &temp.path().join("source.sparksmith"))?;
    let members =
        sparksmith_core::library::bundle::closure(&source.engine().project().library, root)?;
    assert!(members.len() >= 5, "a real closure: {members:?}");

    let mut session = Session::new(Engine::default());
    let before = session.engine().document().clone();
    let revision = session.engine().revision();
    let params = import_params(&session, &path, root);
    let reply = ok(&mut session, "library.import_from_project", params)?;
    assert_eq!(reply["revision"], revision + 1, "one step");

    let library = session.engine().project().library.clone();
    assert_eq!(library.origins.len(), members.len());
    let source_library = &source.engine().project().library;
    for member in &members {
        let lineage = project_lineage_id(source_library, member.kind, member.id)?;
        let origin = library
            .origins
            .iter()
            .find(|o| o.origin.lineage == lineage && o.origin.revision == member.revision)
            .ok_or_else(|| format!("no origin of {member:?}"))?;
        assert_eq!(origin.origin.library, PROJECT_FILES);
        let copy = library.definition(origin.local).ok_or("the copy")?;
        let held = source_library.definition(*member).ok_or("the source")?;
        assert_eq!(copy.0, held.0, "the envelope is copied as it was");
    }

    // The copy works as a placed, assigned part.
    let part = sparksmith_core::PartRef::new(
        serde_json::from_value(reply["outcome"]["report"]["root"]["id"].clone())?,
        serde_json::from_value(reply["outcome"]["report"]["root"]["revision"].clone())?,
    );
    let component = library.part(part).ok_or("the Part")?.definition.component;
    library_op(
        &mut session,
        "library.place_from",
        json!({"source": {"from": "project", "component": component}, "position": [0.0, 0.0]}),
    )?;

    // One undo step takes all of it away; the redo brings all of it back.
    let now = session.engine().revision();
    ok(
        &mut session,
        "project.undo",
        json!({"expected_revision": now}),
    )?;
    let now = session.engine().revision();
    ok(
        &mut session,
        "project.undo",
        json!({"expected_revision": now}),
    )?;
    assert_eq!(session.engine().document(), &before, "nothing left");
    let now = session.engine().revision();
    ok(
        &mut session,
        "project.redo",
        json!({"expected_revision": now}),
    )?;
    assert_eq!(session.engine().project().library, library);

    // Importing again reuses every copy: no revision, no undo step.
    let at = session.engine().revision();
    let params = import_params(&session, &path, root);
    let again = ok(&mut session, "library.import_from_project", params)?;
    assert_eq!(session.engine().revision(), at);
    assert!(
        again["outcome"]["report"]["revisions"]
            .as_array()
            .is_some_and(|r| r.iter().all(|i| i["reused"] == true)),
        "{}",
        again["outcome"]
    );

    // Offline: the source file is deleted, the project reopens whole.
    let saved = save(&mut session, &temp.path().join("target.sparksmith"))?;
    std::fs::remove_file(&path)?;
    let mut reopened = Session::new(Engine::default());
    let now = reopened.engine().revision();
    ok(
        &mut reopened,
        "project.open",
        json!({"expected_revision": now, "path": saved}),
    )?;
    assert_eq!(reopened.engine().project().library, library);
    Ok(())
}

/// A revision is not renumbered in its origin: root r2 of a lineage copies
/// as the project's r1 and records r2; a Part pinned to r1 keeps r1 when r2
/// exists; a later r3 becomes the same project lineage's next revision.
#[test]
fn exact_source_revisions_are_kept_across_imports() -> TestResult {
    let temp = tempfile::tempdir()?;
    let mut source = synthetic_session()?;
    let part = latest_part(&source)?;
    let library = &source.engine().project().library;
    let pinned = library
        .part(sparksmith_core::PartRef::new(part.id, part.revision))
        .ok_or("the Part")?
        .definition
        .component;
    assert_eq!(pinned.revision, Revision::FIRST);
    let new_revision = |session: &mut Session, note: &str| -> Fallible<DefinitionRef> {
        let library = &session.engine().project().library;
        let revision = library.latest(pinned.id).ok_or("the lineage")?.1;
        let at = DefinitionRef {
            kind: DefinitionKind::Component,
            id: pinned.id,
            revision,
        };
        library_op(session, "library.draft_from", json!({"revision": at}))?;
        let draft = session
            .engine()
            .drafts()
            .get(pinned.id)
            .cloned()
            .ok_or("the draft")?;
        let mut meta = draft.meta.clone();
        meta.description = note.into();
        library_op(
            session,
            "library.edit",
            json!({"edits": [{"id": draft.id, "meta": meta, "definition": draft.definition}]}),
        )?;
        library_op(session, "library.publish", json!({"drafts": [pinned.id]}))?;
        assert!(session.engine().drafts().items.is_empty());
        Ok(DefinitionRef {
            revision: Revision(revision.0 + 1),
            ..at
        })
    };
    let r2 = new_revision(&mut source, "second")?;
    let path = save(&mut source, &temp.path().join("source.sparksmith"))?;
    let source_lineage = project_lineage_id(
        &source.engine().project().library,
        DefinitionKind::Component,
        pinned.id,
    )?;

    // Only r2: r1 of that lineage does not come along.
    let mut session = Session::new(Engine::default());
    let params = import_params(&session, &path, r2);
    let reply = ok(&mut session, "library.import_from_project", params)?;
    let origin_of = |session: &Session, revision: u32| {
        session
            .engine()
            .project()
            .library
            .origins
            .iter()
            .find(|o| o.origin.lineage == source_lineage && o.origin.revision == Revision(revision))
            .copied()
    };
    let copied = origin_of(&session, 2).ok_or("r2's origin")?;
    assert_eq!(copied.local.revision, Revision::FIRST, "project numbering");
    assert!(origin_of(&session, 1).is_none());
    assert_eq!(
        reply["outcome"]["report"]["root"]["id"],
        json!(copied.local.id)
    );

    // Its r1 cannot follow now, older than what is held; nothing changes.
    let params = import_params(
        &session,
        &path,
        DefinitionRef {
            revision: Revision::FIRST,
            ..r2
        },
    );
    let message = refused(
        &mut session,
        "library.import_from_project",
        params,
        "conflict",
    )?;
    assert!(message.contains("older than the r2"), "{message}");

    // Into a fresh project, the Part keeps the exact r1 it pins.
    let mut session = Session::new(Engine::default());
    let params = import_params(&session, &path, part);
    let reply = ok(&mut session, "library.import_from_project", params)?;
    let local = origin_of(&session, 1).ok_or("r1's origin")?;
    let copy: sparksmith_core::PartRef = sparksmith_core::PartRef::new(
        serde_json::from_value(reply["outcome"]["report"]["root"]["id"].clone())?,
        serde_json::from_value(reply["outcome"]["report"]["root"]["revision"].clone())?,
    );
    let body = session
        .engine()
        .project()
        .library
        .part(copy)
        .ok_or("the copy")?;
    assert_eq!(body.definition.component.id, local.local.id);
    assert_eq!(body.definition.component.revision, local.local.revision);
    assert!(origin_of(&session, 2).is_none(), "r2 is not in the closure");

    // A newer revision of the file's lineage appends to the same copy, and
    // the file's edits in between did not change the lineage's identity.
    let r3 = new_revision(&mut source, "third")?;
    let path = save(&mut source, &temp.path().join("renamed.sparksmith"))?;
    let params = import_params(&session, &path, r3);
    ok(&mut session, "library.import_from_project", params)?;
    let third = origin_of(&session, 3).ok_or("r3's origin")?;
    assert_eq!(third.local.id, local.local.id, "the same project lineage");
    assert_eq!(third.local.revision, Revision(2), "its next revision");
    Ok(())
}

/// A replay gets equal ids: the same requests on equal files save the same
/// project, with no random source involved.
#[test]
fn importing_from_a_project_replays_to_an_equal_document() -> TestResult {
    let temp = tempfile::tempdir()?;
    let mut source = synthetic_session()?;
    let root = latest_part(&source)?;
    let path = save(&mut source, &temp.path().join("source.sparksmith"))?;
    let run = || -> Fallible<String> {
        let mut session = Session::new(Engine::default());
        let params = import_params(&session, &path, root);
        ok(&mut session, "library.import_from_project", params)?;
        Ok(serde_json::to_string(&session.engine().to_file())?)
    };
    assert_eq!(run()?, run()?);
    Ok(())
}

/// The run's real case: the module authored in its own project, published
/// there and saved, is copied into a board project.
#[test]
fn the_example_module_imports_once_published_in_its_project() -> TestResult {
    let temp = tempfile::tempdir()?;
    let mut module = Session::new(Engine::default());
    let now = module.engine().revision();
    ok(
        &mut module,
        "project.open",
        json!({"expected_revision": now, "path": MODULE}),
    )?;
    let drafts = module.engine().drafts().clone();
    let component = drafts
        .items
        .iter()
        .find(|d| d.definition.kind() == DefinitionKind::Component)
        .ok_or("the module's component draft")?;
    let root = DefinitionRef {
        kind: DefinitionKind::Component,
        id: component.id,
        revision: Revision::FIRST,
    };

    // Unpublished, it is refused by name; a draft has no immutable revision.
    let mut board = Session::new(Engine::default());
    let params = import_params(&board, MODULE, root);
    let message = refused(
        &mut board,
        "library.import_from_project",
        params,
        "not_found",
    )?;
    assert!(message.contains("unpublished draft"), "{message}");
    assert!(message.contains(&component.meta.name), "{message}");

    let ids: Vec<_> = drafts.items.iter().map(|d| d.id).collect();
    library_op(&mut module, "library.publish", json!({"drafts": ids}))?;
    assert!(module.engine().drafts().items.is_empty());
    let path = save(&mut module, &temp.path().join("module.sparksmith"))?;

    let params = import_params(&board, &path, root);
    let reply = ok(&mut board, "library.import_from_project", params)?;
    let root_copy: DefinitionRef =
        serde_json::from_value(reply["outcome"]["report"]["root"].clone())?;
    let library = &board.engine().project().library;
    let source = &module.engine().project().library;
    let copied = library.definition(root_copy).ok_or("the copy")?;
    assert_eq!(copied.0.name, component.meta.name);
    assert_eq!(
        library.origins.len(),
        sparksmith_core::library::bundle::closure(source, root)?.len()
    );
    // Terminal identities and the pin data come across unchanged.
    let (
        Some((_, sparksmith_core::library::Definition::Component(ours))),
        Some((_, sparksmith_core::library::Definition::Component(theirs))),
    ) = (library.definition(root_copy), source.definition(root))
    else {
        return Err("component bodies".into());
    };
    assert_eq!(ours.terminals, theirs.terminals);
    assert_eq!(ours.resources, theirs.resources);
    assert_eq!(ours.implementations.len(), theirs.implementations.len());

    // A placed copy reopens with the source gone.
    library_op(
        &mut board,
        "library.place_from",
        json!({"source": {"from": "project",
                          "component": {"id": root_copy.id, "revision": root_copy.revision}},
               "position": [0.0, 0.0]}),
    )?;
    let saved = save(&mut board, &temp.path().join("board.sparksmith"))?;
    std::fs::remove_file(&path)?;
    let mut reopened = Session::new(Engine::default());
    let now = reopened.engine().revision();
    ok(
        &mut reopened,
        "project.open",
        json!({"expected_revision": now, "path": saved}),
    )?;
    assert_eq!(reopened.engine().project().instances.len(), 1);
    Ok(())
}

#[test]
fn import_from_project_refusals_change_nothing() -> TestResult {
    let temp = tempfile::tempdir()?;
    let mut source = synthetic_session()?;
    let root = latest_part(&source)?;
    let path = save(&mut source, &temp.path().join("source.sparksmith"))?;
    let mut session = synthetic_session()?;
    let command = "library.import_from_project";

    // Stale, before and after the file is read.
    let revision = session.engine().revision();
    let stale = json!({"expected_revision": revision + 7, "path": path, "root": root});
    refused(&mut session, command, stale, "stale_revision")?;

    // A missing file, and bytes that are not a project.
    let missing = temp.path().join("missing.sparksmith").display().to_string();
    let params = import_params(&session, &missing, root);
    refused(&mut session, command, params, "io_error")?;
    let broken = temp.path().join("broken.sparksmith");
    std::fs::write(&broken, b"not a project")?;
    let params = import_params(&session, &broken.display().to_string(), root);
    let reply = session.dispatch(json!({"command": command, "params": params}));
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(session.engine().revision(), revision);

    // An unknown root lists what the file holds, with ids to use.
    let unknown = DefinitionRef {
        revision: Revision(99),
        ..root
    };
    let params = import_params(&session, &path, unknown);
    let message = refused(&mut session, command, params, "not_found")?;
    assert!(message.contains("It holds:"), "{message}");
    assert!(
        message.contains(&format!("part {} r1", root.id.0)),
        "{message}"
    );

    // A wrong kind is not found: the kind is part of the identity.
    let params = import_params(
        &session,
        &path,
        DefinitionRef {
            kind: DefinitionKind::Symbol,
            ..root
        },
    );
    refused(&mut session, command, params, "not_found")?;

    // Malformed requests.
    for params in [
        json!({"expected_revision": revision, "path": path}),
        json!({"expected_revision": revision, "root": root}),
        json!({"path": path, "root": root}),
        json!({"expected_revision": revision, "path": path, "root": root, "extra": 1}),
    ] {
        let reply = session.dispatch(json!({"command": command, "params": params}));
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(session.engine().revision(), revision);
    }

    // Two files that disagree about one revision: the second is refused,
    // never trusted over the copy held. An edited copy of the file that
    // keeps r1 of the lineage but rewrites a later revision.
    let mut first = synthetic_session()?;
    let lineage = root.id;
    let at = DefinitionRef {
        revision: Revision::FIRST,
        ..root
    };
    let bump = |session: &mut Session, note: &str| -> Fallible<DefinitionRef> {
        library_op(session, "library.draft_from", json!({"revision": at}))?;
        let draft = session
            .engine()
            .drafts()
            .get(lineage)
            .cloned()
            .ok_or("draft")?;
        let mut meta = draft.meta.clone();
        meta.description = note.into();
        library_op(
            session,
            "library.edit",
            json!({"edits": [{"id": draft.id, "meta": meta, "definition": draft.definition}]}),
        )?;
        library_op(session, "library.publish", json!({"drafts": [lineage]}))?;
        Ok(DefinitionRef {
            revision: Revision(2),
            ..at
        })
    };
    let r2 = bump(&mut first, "one way")?;
    let a = save(&mut first, &temp.path().join("a.sparksmith"))?;
    let mut second = synthetic_session()?;
    bump(&mut second, "another way")?;
    let b = save(&mut second, &temp.path().join("b.sparksmith"))?;
    let mut target = Session::new(Engine::default());
    let params = import_params(&target, &a, r2);
    ok(&mut target, command, params)?;
    let params = import_params(&target, &b, r2);
    let message = refused(&mut target, command, params, "conflict")?;
    assert!(message.contains("no longer matches the copy"), "{message}");
    Ok(())
}

/// The saved documents of one run, and the folder's files with their bytes.
type Replayed = (String, Vec<(String, Vec<u8>)>);

/// Recorded library and lineage ids make a whole folder session reproducible:
/// two runs in different folders write equal records and save equal projects.
#[test]
fn caller_supplied_folder_ids_replay_to_equal_folders_and_projects() -> TestResult {
    const LIBRARY: &str = "aaaabbbbccccddddeeeeffff00001111";
    let temp = tempfile::tempdir()?;
    let fresh: Vec<String> = (1..=64).map(|n| format!("{n:032x}")).collect();
    let run = |name: &str| -> Fallible<Replayed> {
        let root = temp.path().join(name);
        let dirs = UserDirs {
            config: root.join("config"),
            cache: root.join("cache"),
        };
        let folder = root.join("library");
        let mut session = Session::with_sources(synthetic_engine()?, Sources::with_dirs(dirs)?);
        let part = latest_part(&session)?;
        let created = ok(
            &mut session,
            "library.source.create",
            json!({"path": folder.display().to_string(), "name": "Replay", "library": LIBRARY}),
        )?;
        assert_eq!(created["created"]["library"], LIBRARY);
        let revision = session.engine().revision();
        let published = ok(
            &mut session,
            "library.publish_to_folder",
            json!({"expected_revision": revision, "root": part,
                   "library": LIBRARY, "fresh": fresh}),
        )?;
        let root_lineage = published["root"]["lineage"].clone();
        // The ids came from the request, in closure order.
        assert!(
            fresh.iter().any(|f| json!(f) == root_lineage),
            "{root_lineage}"
        );
        let mut design = Session::with_sources(
            Engine::default(),
            Sources::with_dirs(UserDirs {
                config: root.join("config"),
                cache: root.join("cache"),
            })?,
        );
        ok(
            &mut design,
            "library.import_from_folder",
            json!({"expected_revision": 0, "library": LIBRARY, "lineage": root_lineage}),
        )?;
        let mut files = Vec::new();
        let mut stack = vec![folder.clone()];
        while let Some(directory) = stack.pop() {
            for entry in std::fs::read_dir(directory)? {
                let path = entry?.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let relative = path.strip_prefix(&folder)?.display().to_string();
                    files.push((relative, std::fs::read(&path)?));
                }
            }
        }
        files.sort();
        Ok((
            format!(
                "{}\n{}",
                serde_json::to_string(&session.engine().to_file())?,
                serde_json::to_string(&design.engine().to_file())?
            ),
            files,
        ))
    };
    let first = run("one")?;
    let second = run("two")?;
    assert!(first.1.len() > 3, "records and a manifest");
    assert_eq!(first.0, second.0, "equal projects, origins included");
    assert_eq!(first.1, second.1, "equal folders, byte for byte");
    Ok(())
}

#[test]
fn folder_id_refusals_write_nothing() -> TestResult {
    let temp = tempfile::tempdir()?;
    let dirs = UserDirs {
        config: temp.path().join("config"),
        cache: temp.path().join("cache"),
    };
    let mut session = Session::with_sources(synthetic_engine()?, Sources::with_dirs(dirs)?);
    let folder = temp.path().join("library");

    // The id that names project files is not a folder's.
    let reserved = temp.path().join("reserved");
    let reply = session.dispatch(json!({"command": "library.source.create", "params": {
        "path": reserved.display().to_string(), "name": "No", "library": PROJECT_FILES,
    }}));
    assert_eq!(reply["error"]["code"], "invalid_input", "{reply}");
    assert!(!reserved.join("sparksmith-library.json").exists());

    let created = ok(
        &mut session,
        "library.source.create",
        json!({"path": folder.display().to_string(), "name": "Few"}),
    )?;
    let library = created["created"]["library"].clone();

    // Too few lineage ids refuse before anything is written, and a repeated
    // or malformed id is refused.
    let part = latest_part(&session)?;
    let revision = session.engine().revision();
    for (fresh, code) in [
        (json!(["00000000000000000000000000000001"]), "invalid_input"),
        (
            json!([
                "00000000000000000000000000000001",
                "00000000000000000000000000000001"
            ]),
            "invalid_input",
        ),
    ] {
        refused(
            &mut session,
            "library.publish_to_folder",
            json!({"expected_revision": revision, "root": part,
                   "library": library, "fresh": fresh}),
            code,
        )?;
    }
    let reply = session.dispatch(json!({"command": "library.publish_to_folder", "params": {
        "expected_revision": revision, "root": part, "library": library, "fresh": ["xyz"],
    }}));
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(!folder.join("lineages").exists(), "nothing was written");
    Ok(())
}

/// A folder Part's `implements` ids are catalog-local: the same number must
/// never be read as the project's lineage of that number.
#[test]
fn implements_never_matches_a_folder_entry() -> TestResult {
    let temp = tempfile::tempdir()?;
    let dirs = UserDirs {
        config: temp.path().join("config"),
        cache: temp.path().join("cache"),
    };
    let mut author = Session::with_sources(synthetic_engine()?, Sources::with_dirs(dirs.clone())?);
    let folder = temp.path().join("library").display().to_string();
    publish_library(&mut author, &folder, "Parts")?;
    let mut design = Session::with_sources(Engine::default(), Sources::with_dirs(dirs)?);
    let search = |session: &mut Session, text: &str| -> Fallible<Value> {
        let found = ok(
            session,
            "project.query",
            json!({"query": "library.search", "params": {"query": text}}),
        )?;
        Ok(found["result"]["results"].clone())
    };
    let parts = search(&mut design, "kind:part")?;
    let hit = parts
        .as_array()
        .and_then(|hits| hits.first())
        .ok_or("the folder lists Parts")?;
    assert!(hit["entry"]["source_lineage"].is_string(), "{hit}");
    let implements = hit["entry"]["implements"]["id"]
        .as_u64()
        .ok_or("implements")?;
    let found = search(&mut design, &format!("kind:part implements:{implements}"))?;
    assert_eq!(found, json!([]), "a folder entry never matches");
    Ok(())
}

/// `library.publish` over JSON: the default closes, and the snapshot shows
/// the closure `component.check_drafts` reports.
#[test]
fn publish_closes_over_json_and_check_drafts_reports_the_closure() -> TestResult {
    let mut module = Session::new(Engine::default());
    let now = module.engine().revision();
    ok(
        &mut module,
        "project.open",
        json!({"expected_revision": now, "path": MODULE}),
    )?;
    let ids: Vec<_> = module
        .engine()
        .drafts()
        .items
        .iter()
        .map(|d| d.id)
        .collect();
    assert!(ids.len() > 2);

    let found = ok(
        &mut module,
        "project.query",
        json!({"query": "component.check_drafts", "params": {}}),
    )?;
    let reports = found["result"].as_array().ok_or("reports")?.clone();
    let binding = reports
        .iter()
        .find(|r| r["kind"] == "binding")
        .ok_or("the binding")?;
    let needed = binding["publish_with"].as_array().ok_or("publish_with")?;
    assert!(
        needed.iter().any(|r| r["kind"] == "footprint"),
        "the footprint draft the run could not see: {binding}"
    );

    // Listing exactly the reported set publishes it; the rest stays open.
    let mut set: Vec<Value> = needed.iter().map(|r| r["id"].clone()).collect();
    set.push(binding["revision"]["id"].clone());
    let binding_id = binding["revision"]["id"].clone();
    let alone = module.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": module.engine().revision(),
        "operation": {"editor": "Library", "operation": {
            "command": "library.publish", "params": {"drafts": [binding_id]}}}
    }}));
    assert_eq!(alone["error"]["code"], "conflict", "{alone}");
    assert!(
        alone["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("needs drafts it does not list")),
        "{alone}"
    );
    let reply = library_op(&mut module, "library.publish", json!({"drafts": set}))?;
    assert_eq!(
        reply["drafts"].as_array().map(Vec::len),
        Some(ids.len() - set.len()),
        "the published drafts closed"
    );
    let now = module.engine().revision();
    ok(
        &mut module,
        "project.undo",
        json!({"expected_revision": now}),
    )?;
    assert_eq!(module.engine().drafts().items.len(), ids.len());
    Ok(())
}
