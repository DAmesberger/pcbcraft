//! Compact replies through the public JSON contract: what a mutation reports,
//! what `verbose` returns, and that the document ends up the same either way.
use super::*;
use crate::gate::Caller;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn add_via(session: &Session, position: [f64; 2]) -> Value {
    let revision = session.engine().revision();
    let net = session
        .engine()
        .project()
        .nets
        .first()
        .map_or(0, |net| net.id);
    json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": {"editor": "Layout", "operation": {"command": "layout.add_via", "params": {
            "via": {"net": net, "position": position, "drill": 0.3, "diameter": 0.6, "span": "through", "tented": true}
        }}},
    }})
}

fn demo() -> std::result::Result<Session, Box<dyn std::error::Error>> {
    Ok(Session::new(pcbcraft_core::Engine::demo()?))
}

fn ok(reply: Value) -> std::result::Result<Value, Box<dyn std::error::Error>> {
    if reply["ok"] != true {
        return Err(format!("{}", reply["error"]).into());
    }
    Ok(reply["result"].clone())
}

/// A placed via is reported by id, not by document; undo and redo report the
/// same id removed and added again. The whole reply is small.
#[test]
fn a_mutation_reports_its_ids_and_undo_reports_them_back() -> TestResult {
    let mut session = demo()?;
    let revision = session.engine().revision();
    let request = add_via(&session, [1.0, 1.0]);
    let reply = ok(session.respond(Caller::Operator, request))?;
    assert_eq!(reply["revision"], revision + 1);
    assert_eq!(reply["outcome"]["outcome"], "via_added", "{reply}");
    let via = reply["outcome"]["via"].clone();
    assert_eq!(reply["changes"]["added"]["vias"], json!([via]), "{reply}");
    for snapshot_key in ["project", "erc", "board_checks"] {
        assert!(reply.get(snapshot_key).is_none(), "{snapshot_key}");
    }
    assert_eq!(reply["can_undo"], true);

    let undone = ok(session.respond(Caller::Operator, json!({"command": "project.undo", "params": {"expected_revision": session.engine().revision()}})))?;
    assert_eq!(
        undone["changes"]["removed"]["vias"],
        json!([via]),
        "{undone}"
    );
    assert_eq!(undone["can_redo"], true);
    let redone = ok(session.respond(Caller::Operator, json!({"command": "project.redo", "params": {"expected_revision": session.engine().revision()}})))?;
    assert_eq!(redone["changes"]["added"]["vias"], json!([via]), "{redone}");
    Ok(())
}

/// `verbose` beside the command or inside the parameters returns what
/// `dispatch` returns: the snapshot with the outcome.
#[test]
fn verbose_returns_the_snapshot_in_either_place() -> TestResult {
    let mut session = demo()?;
    let mut request = add_via(&session, [1.0, 1.0]);
    request["verbose"] = json!(true);
    let beside = ok(session.respond(Caller::Operator, request))?;
    assert_eq!(beside["outcome"]["outcome"], "via_added");
    assert_eq!(
        beside["project"],
        serde_json::to_value(session.engine().project())?
    );
    assert!(beside["board_checks"].is_array());

    let mut request = add_via(&session, [2.0, 1.0]);
    request["params"]["verbose"] = json!(true);
    let inside = ok(session.respond(Caller::Operator, request))?;
    assert!(inside["project"].is_object());

    let mut request = add_via(&session, [3.0, 1.0]);
    request["verbose"] = json!(false);
    let compact = ok(session.respond(Caller::Operator, request))?;
    assert!(compact.get("project").is_none());

    let plain = ok(session.respond(
        Caller::Operator,
        json!({"command": "project.inspect", "params": {"verbose": true}}),
    ))?;
    assert_eq!(plain, session.snapshot());
    Ok(())
}

/// Refusals are the same envelope and change nothing, and a stale revision
/// or a malformed request is still refused.
#[test]
fn a_refused_request_answers_with_its_diagnostic_and_changes_nothing() -> TestResult {
    let mut session = demo()?;
    let before = session.engine().document().clone();
    let revision = session.engine().revision();
    let mut stale = add_via(&session, [1.0, 1.0]);
    stale["params"]["expected_revision"] = json!(revision + 5);
    let reply = session.respond(Caller::Operator, stale);
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "stale_revision");
    assert_eq!(session.engine().document(), &before);

    let reply = session.respond(
        Caller::Operator,
        json!({"command": "project.apply", "params": {"nonsense": 1}}),
    );
    assert_eq!(reply["error"]["code"], "malformed_request");
    let reply = session.respond(
        Caller::Operator,
        json!({"command": "project.apply", "params": {"verbose": true, "nonsense": 1}}),
    );
    assert_eq!(reply["error"]["code"], "malformed_request");
    assert_eq!(session.engine().document(), &before);
    assert_eq!(session.engine().revision(), revision);
    Ok(())
}

/// A via touching no copper is a deliberate `board.via_dangling`: the reply
/// lists that finding against the new via, the open total moves by what
/// appeared and went, and undo counts it resolved. A finding that was already
/// open is not repeated.
#[test]
fn findings_report_what_appeared_and_what_went() -> TestResult {
    let mut session = demo()?;
    let open = |session: &Session| {
        let project = session.engine().project();
        layout::checks(project).len() + erc::checks(project).len()
    };
    let resolved = |findings: &Value| -> usize {
        findings["resolved"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, count)| count.as_u64())
            .sum::<u64>() as usize
    };
    let open_before = open(&session);
    let request = add_via(&session, [1.0, 1.0]);
    let reply = ok(session.respond(Caller::Operator, request))?;
    let findings = &reply["findings"];
    let new = findings["new"].as_array().ok_or("new findings")?;
    let about_via = json!({"kind": "via", "id": reply["outcome"]["via"]});
    assert!(
        new.iter().any(|f| f["code"] == "board.via_dangling"
            && f["objects"]
                .as_array()
                .is_some_and(|objects| objects.contains(&about_via))),
        "{reply}"
    );
    assert!(findings.get("omitted").is_none(), "{reply}");
    assert_eq!(
        open_before + new.len() - resolved(findings),
        open(&session),
        "{reply}"
    );
    assert_eq!(findings["open"], open(&session));

    let undone = ok(session.respond(Caller::Operator, json!({"command": "project.undo", "params": {"expected_revision": session.engine().revision()}})))?;
    assert_eq!(
        undone["findings"]["resolved"]["board.via_dangling"], 1,
        "{undone}"
    );
    assert_eq!(undone["findings"]["open"], open_before, "{undone}");
    Ok(())
}

/// A changed draft is listed with its findings; other drafts are not. A
/// draft-only edit runs no design check, so it reports no design findings.
#[test]
fn only_the_changed_draft_is_reported() -> TestResult {
    let mut session = demo()?;
    let revision = session.engine().revision();
    let count = session.engine().drafts().items.len();
    assert!(count > 0, "the demo has drafts");
    let existing = serde_json::to_value(session.engine().drafts())?;
    let symbol = existing["items"]
        .as_array()
        .and_then(|items| items.iter().find(|d| d["definition"]["kind"] == "symbol"))
        .ok_or("the demo has a symbol draft")?;
    let mut meta = symbol["meta"].clone();
    meta["name"] = json!("Only this");
    let reply = ok(session.respond(Caller::Operator, json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": {"editor": "Library", "operation": {"command": "library.new_draft", "params": {
            "meta": meta,
            "definition": symbol["definition"],
        }}},
    }})))?;
    let id = reply["outcome"]["id"].clone();
    assert_eq!(
        reply["changes"]["added"]["drafts.items"],
        json!([id]),
        "{reply}"
    );
    let drafts = reply["drafts"].as_array().ok_or("drafts")?;
    assert_eq!(drafts.len(), 1, "{reply}");
    assert_eq!(drafts[0]["id"], id);
    assert_eq!(drafts[0]["name"], "Only this");
    assert_eq!(drafts[0]["kind"], "symbol");
    assert!(drafts[0]["findings"].is_array());
    assert!(reply.get("findings").is_none(), "{reply}");
    Ok(())
}

/// `project.inspect` answers with counts that match the document; `at` reads
/// one part of the snapshot; a pointer that names nothing is refused.
#[test]
fn inspect_summarizes_and_reads_one_part() -> TestResult {
    let mut session = demo()?;
    let reply = ok(session.respond(
        Caller::Operator,
        json!({"command": "project.inspect", "params": {}}),
    ))?;
    let project = session.engine().project();
    let counts = &reply["summary"]["counts"];
    assert_eq!(counts["tracks"], project.tracks.len());
    assert_eq!(counts["instances"], project.instances.len());
    assert_eq!(
        counts["library.components"],
        project.library.components.len()
    );
    assert_eq!(
        reply["summary"]["drafts"].as_array().map(Vec::len),
        Some(session.engine().drafts().items.len())
    );
    assert_eq!(
        reply["summary"]["findings"]["open"],
        layout::checks(project).len() + erc::checks(project).len()
    );
    assert!(reply.get("project").is_none());

    let nets = ok(session.respond(
        Caller::Operator,
        json!({"command": "project.inspect", "params": {"at": "/project/nets"}}),
    ))?;
    assert_eq!(
        nets["value"],
        serde_json::to_value(&session.engine().project().nets)?
    );
    assert_eq!(nets["revision"], session.engine().revision());
    // The in-process path reads the same part.
    let direct =
        ok(session
            .dispatch(json!({"command": "project.inspect", "params": {"at": "/project/nets"}})))?;
    assert_eq!(direct, nets);

    let missing = session.respond(
        Caller::Operator,
        json!({"command": "project.inspect", "params": {"at": "/project/nothing"}}),
    );
    assert_eq!(missing["error"]["code"], "not_found");
    Ok(())
}

/// Opening or starting a project replaces the document: a summary, not a
/// list of everything that differs.
#[test]
fn replacing_the_document_answers_with_a_summary() -> TestResult {
    let mut session = demo()?;
    let profile = pcbcraft_core::rules::fixtures::prototype()?;
    let reply = ok(session.respond(Caller::Operator, json!({"command": "project.new", "params": {"expected_revision": session.engine().revision(), "profile": profile}})))?;
    assert_eq!(reply["summary"]["counts"]["tracks"], 0);
    // The new board pins the profile it was given.
    assert_eq!(session.engine().project().rules.profile, Some(profile));
    assert!(reply.get("changes").is_none(), "{reply}");
    Ok(())
}

/// A file effect answers as before, without document changes.
#[test]
fn a_request_that_changes_nothing_reports_no_changes() -> TestResult {
    let mut session = demo()?;
    let reply = ok(session.respond(
        Caller::Operator,
        json!({"command": "library.sources", "params": {}}),
    ))?;
    assert_eq!(reply, json!({"sources": []}));
    Ok(())
}

/// The ESP32 board's whole replay through `respond` builds the file the same
/// replay builds through `dispatch`, byte for byte, with no request refused,
/// and no reply carries the document. Saving is left out: the test writes
/// nothing into the example.
#[test]
fn the_board_replay_builds_the_same_document_with_compact_replies() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let replay = std::fs::read_to_string(
        root.join("examples/esp32-s31-devboard/esp32-s31-devboard.replay.jsonl"),
    )?;
    let mut compact = Session::default();
    let mut full = Session::default();
    let mut largest = 0;
    for line in replay.lines() {
        let mut request: Value = serde_json::from_str(line)?;
        if request["command"] == "project.save" {
            continue;
        }
        // Paths the replay reads are relative to the repository root.
        if request["command"] == "project.open" || request["command"] == "library.use_part_file" {
            let path = request["params"]["path"].as_str().ok_or("path")?;
            request["params"]["path"] = json!(root.join(path).display().to_string());
        }
        let reply = compact.respond(Caller::Operator, request.clone());
        assert_eq!(reply["ok"], true, "{line}: {reply}");
        assert!(reply["result"].get("project").is_none(), "{line}");
        // `rules.fab_profiles` answers whole profiles by design (the
        // board's and the built-in JLCPCB 2-layer, about 12 KB each); it is
        // no document reply, so it has its own bound.
        let size = reply.to_string().len();
        if request["params"]["query"] == "rules.fab_profiles" {
            assert!(size < 32 * 1024, "{line}: {size} bytes");
        } else {
            largest = largest.max(size);
        }
        let reply = full.dispatch(request);
        assert_eq!(reply["ok"], true, "{line}: {reply}");
    }
    assert_eq!(compact.engine().revision(), full.engine().revision());
    assert_eq!(
        serde_json::to_vec(&compact.engine().to_file())?,
        serde_json::to_vec(&full.engine().to_file())?
    );
    // Measured 9 243 bytes (docs/e2e-esp32-devboard.md).
    assert!(largest < 16 * 1024, "largest reply {largest} bytes");
    Ok(())
}

mod diff {
    use super::super::reply::tests_support::{changes_of, delta};
    use pcbcraft_core::Finding;
    use serde_json::json;

    #[test]
    fn collections_are_diffed_by_identity() {
        let before = json!({
            "next_id": 5,
            "tracks": [{"id": 1, "w": 1}, {"id": 2, "w": 1}, {"id": 3, "w": 1}],
            "placements": [{"instance": 7, "x": 0}],
            "library": {"parts": [{"id": 4, "revision": 1, "m": "a"}]},
            "board_size": [10, 10],
        });
        let after = json!({
            "next_id": 9,
            "tracks": [{"id": 1, "w": 2}, {"id": 2, "w": 1}, {"id": 8, "w": 1}],
            "placements": [{"instance": 7, "x": 3}],
            "library": {"parts": [{"id": 4, "revision": 1, "m": "a"}, {"id": 4, "revision": 2, "m": "b"}]},
            "board_size": [20, 10],
            "fills": [{"id": 6}],
        });
        assert_eq!(
            changes_of(&before, &after),
            json!({
                "added": {"tracks": [8], "fills": [6], "library.parts": [{"id": 4, "revision": 2}]},
                "changed": {"tracks": [1], "placements": [7]},
                "removed": {"tracks": [3]},
                "fields": ["board_size"],
            })
        );
    }

    /// An emptied collection is left out when serialized; a reorder has no
    /// element to name, so it is reported as the collection's field.
    #[test]
    fn emptied_and_reordered_collections_are_still_reported() {
        let before = json!({"fills": [{"id": 6}], "sheets": [{"id": 1}, {"id": 2}]});
        let after = json!({"sheets": [{"id": 2}, {"id": 1}]});
        assert_eq!(
            changes_of(&before, &after),
            json!({"removed": {"fills": [6]}, "fields": ["sheets"]})
        );
        assert_eq!(changes_of(&before, &before), serde_json::Value::Null);
    }

    #[test]
    fn long_lists_are_cut_and_counted() {
        let old: Vec<_> = (0..1000).map(|i| json!({"id": i, "w": 1})).collect();
        let new: Vec<_> = (0..1000).map(|i| json!({"id": i, "w": 2})).collect();
        let changes = changes_of(&json!({"tracks": old}), &json!({"tracks": new}));
        assert_eq!(
            changes["changed"]["tracks"].as_array().map(Vec::len),
            Some(200)
        );
        assert_eq!(changes["omitted"], 800);
    }

    #[test]
    fn repeated_findings_count_each_time_and_long_lists_are_cut() {
        let finding = |code: &str, text: &str| Finding::new(code, text.to_owned(), vec![]);
        let old = [finding("a", "x"), finding("a", "x"), finding("b", "y")];
        let new = [finding("a", "x"), finding("c", "z")];
        let out = delta(&old, &new);
        assert_eq!(
            out["new"],
            json!([{"code": "c", "message": "z", "objects": []}])
        );
        assert_eq!(out["resolved"], json!({"a": 1, "b": 1}));
        assert_eq!(out["open"], 2);
        assert!(out.get("omitted").is_none());

        let many: Vec<_> = (0..100).map(|i| finding("c", &format!("{i}"))).collect();
        let out = delta(&[], &many);
        assert_eq!(out["new"].as_array().map(Vec::len), Some(20));
        assert_eq!(out["omitted"], 80);
        assert_eq!(out["open"], 100);
    }
}
