//! `project.export_fabrication` through the JSON request contract, on the
//! shipped showcase (sketched board, cutout, milled slot): a folder and a
//! ZIP, refusal of a used folder and of unwaived errors with nothing
//! written, and waivers that unblock export, survive save and reopen and
//! undo as one step each.
use super::*;

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn ok(session: &mut Session, request: Value) -> Fallible<Value> {
    let reply = session.dispatch(request);
    if reply["ok"] == true {
        Ok(reply["result"].clone())
    } else {
        Err(format!("refused: {}", reply["error"]).into())
    }
}

fn export(output: &Path, zip: bool, profile: Option<&str>) -> Value {
    json!({"command": "project.export_fabrication", "params": {
        "output": output.display().to_string(),
        "zip": zip,
        "profile": profile,
        "options": {"base_name": "showcase"},
    }})
}

fn names(result: &Value) -> Vec<String> {
    result["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .filter_map(|f| f["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_showcase_exports_as_a_folder_and_a_zip() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let mut session = Session::new(showcase()?);
    let revision = session.engine().revision();
    let folder = dir.path().join("fab");
    let result = ok(&mut session, export(&folder, false, None))?;
    assert_eq!(result["preflight"]["blocking"], 0);
    let written = names(&result);
    for expected in [
        "showcase-copper-l1-top.gbr",
        "showcase-copper-l2-inner1.gbr",
        "showcase-copper-l3-inner2.gbr",
        "showcase-copper-l4-bottom.gbr",
        "showcase-mask-top.gbr",
        "showcase-mask-bottom.gbr",
        "showcase-paste-top.gbr",
        "showcase-legend-top.gbr",
        "showcase-profile.gbr",
        "showcase-plated.drl",
        "showcase-drill-map.gbr",
        "showcase-drill-table.txt",
        "showcase.gbrjob",
        "showcase-pos-top.csv",
        "showcase-bom.csv",
        "showcase-fabrication.pdf",
        "showcase-assembly.pdf",
    ] {
        assert!(
            written.iter().any(|n| n == expected),
            "{expected} in {written:?}"
        );
        assert!(folder.join(expected).is_file(), "{expected} on disk");
    }
    // The shipped board's milled slot and round cutout are in the profile
    // (arcs), not in the drill files.
    let profile = std::fs::read_to_string(folder.join("showcase-profile.gbr"))?;
    assert!(profile.contains("G03*") && profile.contains("%TF.FileFunction,Profile,NP*%"));
    // A used folder is refused and left alone.
    let again = session.dispatch(export(&folder, false, None));
    assert_eq!(again["ok"], false);
    assert_eq!(std::fs::read_dir(&folder)?.count(), written.len());
    // One ZIP with the same entries, in order.
    let zip_path = dir.path().join("fab.zip");
    let zipped = ok(&mut session, export(&zip_path, true, None))?;
    assert_eq!(names(&zipped), written);
    let bytes = std::fs::read(&zip_path)?;
    assert!(bytes.starts_with(b"PK\x03\x04"));
    for name in &written {
        assert!(
            bytes.windows(name.len()).any(|w| w == name.as_bytes()),
            "{name} in the archive"
        );
    }
    // Export is a file effect: no revision, no undo step.
    assert_eq!(session.engine().revision(), revision);
    Ok(())
}

/// `project.rename` over the JSON contract names every output, and one
/// undo step takes it back.
#[test]
fn the_project_name_reaches_the_production_files() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let mut session = Session::new(showcase()?);
    let was = session.engine().project().name.clone();
    let revision = session.engine().revision();
    let renamed = ok(
        &mut session,
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Schematic", "operation": {
                "command": "project.rename", "params": {"name": "Dev board A"}}},
        }}),
    )?;
    assert_eq!(renamed["revision"], revision + 1);
    let stale = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": {"editor": "Schematic", "operation": {
            "command": "project.rename", "params": {"name": "Late"}}},
    }}));
    assert_eq!(stale["ok"], false, "{stale}");
    let malformed = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision + 1,
        "operation": {"editor": "Schematic", "operation": {
            "command": "project.rename", "params": {"title": "Wrong field"}}},
    }}));
    assert_eq!(malformed["ok"], false, "{malformed}");
    assert_eq!(session.engine().project().name, "Dev board A");

    let out = dir.path().join("named");
    ok(&mut session, export(&out, false, None))?;
    let mut gerbers = 0;
    for entry in std::fs::read_dir(&out)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "gbr") {
            let text = std::fs::read_to_string(&path)?;
            assert!(
                text.contains("%TF.ProjectId,Dev board A,"),
                "{}",
                path.display()
            );
            assert!(!text.contains(&was), "{}", path.display());
            gerbers += 1;
        }
        if path.to_string_lossy().ends_with("-drill-table.txt") {
            let text = std::fs::read_to_string(&path)?;
            assert!(text.starts_with("Drill table: Dev board A\n"), "{text}");
        }
    }
    assert!(gerbers > 0);
    let now = session.engine().revision();
    ok(
        &mut session,
        json!({"command": "project.undo", "params": {"expected_revision": now}}),
    )?;
    assert_eq!(session.engine().project().name, was);
    Ok(())
}

/// The query takes a profile id as the export does, with the same result
/// as the whole profile; an unknown id and a malformed one are refused.
#[test]
fn the_production_check_takes_a_profile_id() -> Fallible<()> {
    let mut session = Session::new(showcase()?);
    let query = |profile: Value| {
        json!({"command": "project.query", "params": {
            "query": "fabrication.preflight",
            "params": {"profile": profile},
        }})
    };
    let by_id = ok(&mut session, query(json!("std-6-6")))?;
    let whole = ok(
        &mut session,
        query(serde_json::to_value(pcbcraft_core::rules::builtin::by_id(
            "std-6-6",
        ))?),
    )?;
    assert_eq!(by_id, whole);
    // The report names the profile it checked under.
    assert_eq!(by_id["result"]["profile"], "Standard 6/6");
    assert!(by_id["result"]["blocking"].as_u64() > Some(0));

    let revision = session.engine().revision();
    let unknown = session.dispatch(query(json!("no-such-profile")));
    assert_eq!(unknown["ok"], false, "{unknown}");
    for malformed in [json!(6), json!({"id": "std-6-6"})] {
        let reply = session.dispatch(query(malformed));
        assert_eq!(reply["ok"], false, "{reply}");
    }
    assert_eq!(session.engine().revision(), revision);

    // The engine alone knows the built-in profiles.
    let engine = session.engine();
    let refused = engine.query(pcbcraft_core::Query::Preflight {
        profile: Some(pcbcraft_core::fabrication::ProfileChoice::Id("mine".into())),
    });
    assert!(refused.is_err());
    Ok(())
}

#[test]
fn unwaived_errors_block_export_and_waivers_are_saved_and_undoable() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let mut session = Session::new(showcase()?);
    // Under the Standard 6/6 profile the fills were computed under other
    // rules: they are stale, an error that blocks production.
    let out = dir.path().join("blocked");
    let refused = session.dispatch(export(&out, false, Some("std-6-6")));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        refused["error"].to_string().contains("fabrication.blocked"),
        "{refused}"
    );
    assert!(!out.exists(), "nothing is written");
    let report = ok(
        &mut session,
        json!({"command": "project.query", "params": {
            "query": "fabrication.preflight",
            "params": {"profile": pcbcraft_core::rules::builtin::by_id("std-6-6")},
        }}),
    )?;
    let blocking: Vec<String> = report["result"]["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .filter(|i| i["severity"] == "error" && i.get("waiver").is_none())
        .filter_map(|i| i["key"].as_str().map(str::to_string))
        .collect();
    assert!(!blocking.is_empty());
    assert_eq!(report["result"]["blocking"], blocking.len());
    // A reason is optional: without one the waiver keeps an empty reason.
    let revision = session.engine().revision();
    ok(
        &mut session,
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Fabrication", "operation": {
                "command": "fabrication.waive", "params": {"keys": [blocking[0]]}}},
        }}),
    )?;
    let waivers = &session.engine().project().production.waivers;
    assert_eq!(waivers.len(), 1);
    assert_eq!(waivers[0].reason, "");
    let now = session.engine().revision();
    ok(
        &mut session,
        json!({"command": "project.undo", "params": {"expected_revision": now}}),
    )?;
    // One key that is not printable refuses the whole list.
    let revision = session.engine().revision();
    let mut bad = blocking.clone();
    bad.push("bad\u{7}key".into());
    let refused = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": {"editor": "Fabrication", "operation": {
            "command": "fabrication.waive", "params": {"keys": bad, "reason": "prototype"}}},
    }}));
    assert_eq!(refused["ok"], false);
    assert_eq!(session.engine().revision(), revision);
    assert!(session.engine().project().production.waivers.is_empty());
    // Every blocking finding is waived with one reason in one command.
    let revision = session.engine().revision();
    ok(
        &mut session,
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Fabrication", "operation": {
                "command": "fabrication.waive",
                "params": {"keys": blocking, "reason": "fabricator refills planes"}}},
        }}),
    )?;
    let result = ok(&mut session, export(&out, false, Some("std-6-6")))?;
    assert_eq!(result["preflight"]["blocking"], 0);
    assert_eq!(result["preflight"]["waived"], blocking.len());
    // Waivers are saved with the project.
    let file = dir.path().join("waived.pcbcraft");
    ok(
        &mut session,
        json!({"command": "project.save", "params": {"path": file.display().to_string()}}),
    )?;
    let mut reopened = Session::new(Engine::default());
    ok(
        &mut reopened,
        json!({"command": "project.open", "params": {"expected_revision": 0, "path": file.display().to_string()}}),
    )?;
    assert_eq!(
        reopened.engine().project().production.waivers.len(),
        blocking.len()
    );
    // The waivers were one undo step.
    let now = session.engine().revision();
    ok(
        &mut session,
        json!({"command": "project.undo", "params": {"expected_revision": now}}),
    )?;
    assert!(session.engine().project().production.waivers.is_empty());
    // An unknown profile is refused.
    let unknown = session.dispatch(export(
        &dir.path().join("x"),
        false,
        Some("no-such-profile"),
    ));
    assert_eq!(unknown["ok"], false);
    Ok(())
}

/// A host that requires a reason (the app's setting) refuses a waiver
/// without one, changing nothing; with a reason it is kept.
#[test]
fn a_required_reason_refuses_a_waiver_without_one() -> Fallible<()> {
    let mut engine = Engine::default();
    engine.require_waiver_reason(true);
    let mut session = Session::new(engine);
    let waive = |revision: u64, reason: Value| {
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Fabrication", "operation": {
                "command": "fabrication.waive", "params": {"keys": ["k"], "reason": reason}}},
        }})
    };
    let revision = session.engine().revision();
    for reason in [json!(" "), json!("")] {
        let refused = session.dispatch(waive(revision, reason));
        assert_eq!(refused["ok"], false, "{refused}");
        assert!(
            refused["error"]
                .to_string()
                .contains("fabrication.waiver_reason"),
            "{refused}"
        );
        assert_eq!(session.engine().revision(), revision);
        assert!(session.engine().project().production.waivers.is_empty());
    }
    ok(&mut session, waive(revision, json!("prototype")))?;
    assert_eq!(
        session.engine().project().production.waivers[0].reason,
        "prototype"
    );
    Ok(())
}
