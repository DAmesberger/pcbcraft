use super::*;
use crate::{Engine, Operation, parts, schematic::SchematicCommand};

fn incomplete() -> Part {
    let mut part = parts::passive_template("");
    part.binding.mappings.clear();
    part.symbol.pins[0].terminal = 999;
    part.package.body_size = [0.0, -1.0, 0.5];
    part
}

#[test]
fn legacy_v1_migration_preserves_all_ids_and_removes_no_project_data() -> Result<()> {
    let engine = Engine::demo()?;
    let before = engine.project().clone();
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("legacy.pcbcraft");
    std::fs::write(&path, serde_json::to_vec(&before)?)?;
    let migrated = read(&path)?;
    assert_eq!(migrated.version, 2);
    assert_eq!(migrated.project, before);
    assert!(migrated.authoring_draft.is_none());
    let mut restored = Engine::default();
    restored.open(&path)?;
    assert_eq!(restored.project(), &before);
    restored.save(&path)?;
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(value["version"], 2);
    assert_eq!(value["project"]["version"], 1);
    assert_eq!(read(&path)?.project, before);
    Ok(())
}

#[test]
fn incomplete_finite_draft_roundtrips_and_cli_style_save_retains_it() -> Result<()> {
    let mut engine = Engine::demo()?;
    let draft = incomplete();
    assert!(
        parts::validate_part(&draft).is_err(),
        "fixture is deliberately unfinished"
    );
    let file = engine.document(Some(draft.clone()));
    engine.replace_document(file.clone())?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("draft.pcbcraft");
    engine.save(&path)?;
    let mut opened = Engine::default();
    opened.open(&path)?;
    assert_eq!(opened.document(opened.authoring_draft().cloned()), file);
    opened.apply(
        opened.revision(),
        Operation::Schematic(SchematicCommand::Edit {
            instance: 2,
            reference: "R99".into(),
            value: "22k".into(),
        }),
    )?;
    opened.save(&path)?;
    let saved = read(&path)?;
    assert_eq!(saved.authoring_draft, Some(draft));
    assert_eq!(saved.project.instances[0].reference, "R99");
    Ok(())
}

#[test]
fn corrupt_newer_and_invalid_draft_files_leave_engine_and_history_unchanged() -> Result<()> {
    let mut engine = Engine::demo()?;
    engine.replace_document(engine.document(Some(incomplete())))?;
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::Edit {
            instance: 2,
            reference: "R99".into(),
            value: "22k".into(),
        }),
    )?;
    let before = engine.document(engine.authoring_draft().cloned());
    let revision = engine.revision();
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("bad.pcbcraft");
    let mut newer = serde_json::to_value(&before)?;
    newer["version"] = serde_json::json!(999);
    let mut feature = serde_json::to_value(&before)?;
    feature["required_features"] = serde_json::json!(["future-editor"]);
    let mut missing = serde_json::to_value(&before)?;
    missing["project"]["parts"] = serde_json::json!([]);
    let mut draft = serde_json::to_value(&before)?;
    draft["authoring_draft"]["symbol"]["body_size"] = serde_json::json!([null, 2]);
    for bytes in [
        b"broken".to_vec(),
        serde_json::to_vec(&newer)?,
        serde_json::to_vec(&feature)?,
        serde_json::to_vec(&missing)?,
        serde_json::to_vec(&draft)?,
    ] {
        std::fs::write(&path, bytes)?;
        assert!(engine.open(&path).is_err());
        assert_eq!(engine.document(engine.authoring_draft().cloned()), before);
        assert_eq!(engine.revision(), revision);
        assert!(engine.can_undo());
    }
    let mut unsafe_draft = before.clone();
    if let Some(draft) = &mut unsafe_draft.authoring_draft {
        draft.footprint.pads[0].position[0] = f64::INFINITY;
    }
    assert!(engine.replace_document(unsafe_draft).is_err());
    assert_eq!(engine.document(engine.authoring_draft().cloned()), before);
    engine.undo()?;
    assert_eq!(engine.project().instances[0].reference, "R1");
    assert!(engine.authoring_draft().is_some());
    Ok(())
}

#[test]
fn invalid_and_oversized_saves_preserve_previous_file() -> Result<()> {
    let engine = Engine::demo()?;
    let valid = engine.document(None);
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("existing.pcbcraft");
    write(&path, &valid)?;
    let original = std::fs::read(&path)?;
    let mut invalid = valid.clone();
    let mut draft = incomplete();
    draft.provenance = "x".repeat(4097);
    invalid.authoring_draft = Some(draft);
    assert!(write(&path, &invalid).is_err());
    assert_eq!(std::fs::read(&path)?, original);
    let too_big = vec![b' '; MAX_FILE_BYTES + 1];
    assert!(atomic_write(&path, &too_big).is_err());
    assert_eq!(std::fs::read(&path)?, original);
    assert!(
        encode(&"x".repeat(MAX_FILE_BYTES)).is_err(),
        "serialization is bounded before writing"
    );
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
    let mut draft = incomplete();
    draft.footprint.pads = vec![draft.footprint.pads[0].clone(); 1025];
    assert!(validate_draft(&draft).is_err());
    Ok(())
}
