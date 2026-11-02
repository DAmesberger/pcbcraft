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
        assert!(written.iter().any(|n| n == expected), "{expected} in {written:?}");
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
    // A waiver needs a reason.
    let revision = session.engine().revision();
    let unreasoned = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": {"editor": "Fabrication", "operation": {
            "command": "fabrication.waive", "params": {"key": blocking[0], "reason": " "}}},
    }}));
    assert_eq!(unreasoned["ok"], false);
    assert_eq!(session.engine().revision(), revision);
    for key in &blocking {
        let revision = session.engine().revision();
        ok(
            &mut session,
            json!({"command": "project.apply", "params": {
                "expected_revision": revision,
                "operation": {"editor": "Fabrication", "operation": {
                    "command": "fabrication.waive",
                    "params": {"key": key, "reason": "fabricator refills planes"}}},
            }}),
        )?;
    }
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
        json!({"command": "project.open", "params": {"path": file.display().to_string()}}),
    )?;
    assert_eq!(
        reopened.engine().project().production.waivers.len(),
        blocking.len()
    );
    // Each waiver was one undo step.
    for _ in 0..blocking.len() {
        ok(&mut session, json!({"command": "project.undo", "params": {}}))?;
    }
    assert!(session.engine().project().production.waivers.is_empty());
    // An unknown profile is refused.
    let unknown = session.dispatch(export(&dir.path().join("x"), false, Some("no-such-profile")));
    assert_eq!(unknown["ok"], false);
    Ok(())
}
