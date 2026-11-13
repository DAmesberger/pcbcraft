use super::*;

fn create() -> Result<Probe> {
    let mut probe = Probe::default();
    probe.execute(Command::Create(Create {
        parameters: Parameters::default(),
    }))?;
    Ok(probe)
}

fn changed() -> Parameters {
    Parameters {
        board_thickness: 2.4,
        hole_x: 6.0,
        hole_y: 7.0,
        top_x: 17.0,
        top_y: 13.0,
        top_angle: 90.0,
        bottom_x: 28.0,
        bottom_y: 20.0,
        bottom_angle: -45.0,
        package_length: 5.0,
        package_height: 1.8,
        ..Parameters::default()
    }
}

#[test]
fn native_file_recomputes_with_only_the_upstream_engine()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir
        .path()
        .join("fixture.solidcraft")
        .to_string_lossy()
        .into_owned();
    let mut probe = create()?;
    probe.execute(Command::Save(FilePath { path: path.clone() }))?;
    drop(probe);
    // No Sparksmith rebuild/update code is used to regenerate this file.
    let mut upstream = Session::default();
    call(
        &mut upstream,
        "doc.open",
        json!({"path": path, "read_only": true}),
    )?;
    measure::verify(&upstream, &Parameters::default())?;
    let before = upstream
        .doc
        .occurrences
        .iter()
        .map(|o| (o.id, o.component))
        .collect::<Vec<_>>();
    call(
        &mut upstream,
        "parameters.change",
        json!({"changes": changed().expressions()}),
    )?;
    measure::verify(&upstream, &changed())?;
    let after = upstream
        .doc
        .occurrences
        .iter()
        .map(|o| (o.id, o.component))
        .collect::<Vec<_>>();
    assert_eq!(before, after);
    assert!(
        upstream
            .doc
            .features
            .iter()
            .all(|f| !matches!(f.kind, solidcraft_engine::doc::FeatureKind::Import { .. }))
    );
    let edited = dir.path().join("edited.solidcraft");
    call(
        &mut upstream,
        "file.save_as",
        json!({"path": edited, "cache": false, "preview": false}),
    )?;
    drop(upstream);
    let mut second = Session::default();
    call(
        &mut second,
        "doc.open",
        json!({"path": edited, "read_only": true}),
    )?;
    measure::verify(&second, &changed())?;
    Ok(())
}

#[test]
fn rejected_updates_and_failed_open_preserve_document_and_revision() -> Result<()> {
    let mut probe = create()?;
    let before = serde_json::to_value(probe.execute(Command::Inspect(Empty {}))?)?;
    assert!(
        probe
            .execute(Command::Update(Update {
                expected_revision: 0,
                parameters: changed()
            }))
            .is_err()
    );
    let invalid = Parameters {
        hole_x: -1.0,
        ..Parameters::default()
    };
    assert!(
        probe
            .execute(Command::Update(Update {
                expected_revision: 1,
                parameters: invalid
            }))
            .is_err()
    );
    assert!(probe.dispatch(json!({"command": "probe.update", "params": {"expected_revision": 1, "parameters": "junk"}})).is_err());
    assert!(
        probe
            .execute(Command::Open(FilePath {
                path: "/nonexistent-sparksmith-probe/file.solidcraft".into()
            }))
            .is_err()
    );
    let after = serde_json::to_value(probe.execute(Command::Inspect(Empty {}))?)?;
    assert_eq!(before, after);
    Ok(())
}

#[test]
fn undo_redo_restores_geometry_and_invalidates_stale_edits() -> Result<()> {
    let mut probe = create()?;
    let original = probe.execute(Command::Inspect(Empty {}))?;
    let reply = probe.execute(Command::Update(Update {
        expected_revision: 1,
        parameters: changed(),
    }))?;
    assert_eq!(reply.snapshot.parameters, changed());
    let undo = probe.execute(Command::Undo(Empty {}))?;
    // Compare evaluated snapshots: degrees pass through the engine's radians.
    assert_eq!(
        serde_json::to_value(&undo.snapshot)?,
        serde_json::to_value(&original.snapshot)?
    );
    assert!(undo.revision > reply.revision);
    assert!(
        probe
            .execute(Command::Update(Update {
                expected_revision: reply.revision,
                parameters: changed()
            }))
            .is_err()
    );
    let redo = probe.execute(Command::Redo(Empty {}))?;
    assert_eq!(
        serde_json::to_value(&redo.snapshot)?,
        serde_json::to_value(&reply.snapshot)?
    );
    Ok(())
}

#[test]
fn save_refuses_overwrite_and_failed_save_keeps_session()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("existing.solidcraft");
    std::fs::write(&path, b"keep this existing document")?;
    let mut probe = create()?;
    assert!(
        probe
            .execute(Command::Save(FilePath {
                path: path.to_string_lossy().into_owned()
            }))
            .is_err()
    );
    assert_eq!(std::fs::read(&path)?, b"keep this existing document");
    assert_eq!(probe.execute(Command::Inspect(Empty {}))?.revision, 1);
    Ok(())
}

#[test]
fn malformed_and_nonfinite_parameters_are_rejected() -> Result<()> {
    let mut probe = Probe::default();
    for input in [
        json!(null),
        json!({}),
        json!({"command":"unknown", "params":{}}),
        json!({"command":"probe.inspect", "params":{"unexpected":true}}),
    ] {
        assert!(probe.dispatch(input).is_err());
    }
    let invalid = Parameters {
        board_thickness: f64::NAN,
        ..Parameters::default()
    };
    assert!(
        probe
            .execute(Command::Create(Create {
                parameters: invalid
            }))
            .is_err()
    );
    assert!(probe.execute(Command::Inspect(Empty {})).is_err());
    Ok(())
}
