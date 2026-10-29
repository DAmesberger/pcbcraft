use super::*;
use pcbcraft_core::{Engine, Operation, parts, schematic::SchematicCommand};

#[test]
fn saved_project_reopens_offline_with_exact_revisions_and_draft() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("offline.pcbcraft");
    let mut engine = Engine::demo()?;
    let mut draft = parts::passive_template("unfinished");
    draft.binding.mappings.clear();
    engine.apply(
        engine.revision(),
        Operation::Part(parts::PartCommand::SetDraft { draft }),
    )?;
    write_project(&path, &engine.to_file())?;
    let reopened = Engine::from_file(read_project(&path)?)?;
    assert_eq!(reopened.to_file(), engine.to_file());
    assert!(!reopened.can_undo());
    Ok(())
}

#[test]
fn invalid_files_are_refused_without_side_effects() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("bad.pcbcraft");
    let mut engine = Engine::demo()?;
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::Edit {
            instance: 2,
            reference: "R10".into(),
            value: "22k".into(),
        }),
    )?;
    let before = engine.to_file();
    let mut newer = serde_json::to_value(&before)?;
    newer["version"] = serde_json::json!(999);
    for bytes in [b"not json".to_vec(), serde_json::to_vec(&newer)?] {
        std::fs::write(&path, bytes)?;
        assert!(read_project(&path).is_err());
    }
    let large = std::fs::File::create(&path)?;
    large.set_len(MAX_FILE_BYTES as u64 + 1)?;
    assert!(matches!(read_project(&path), Err(Error::TooLarge(_))));
    assert!(read_project(&dir.path().join("wrong.json")).is_err());
    assert_eq!(engine.to_file(), before);
    Ok(())
}

#[test]
fn failed_saves_preserve_existing_target_and_leave_no_temporary_file() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let engine = Engine::demo()?;
    let valid = engine.to_file();
    let path = dir.path().join("existing.pcbcraft");
    write_project(&path, &valid)?;
    let original = std::fs::read(&path)?;
    let mut invalid = valid.clone();
    invalid.project.name = String::new();
    assert!(write_project(&path, &invalid).is_err());
    assert!(atomic_write(&path, &vec![b' '; MAX_FILE_BYTES + 1]).is_err());
    assert_eq!(std::fs::read(&path)?, original);
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);

    let occupied = dir.path().join("occupied.pcbcraft");
    std::fs::create_dir(&occupied)?;
    std::fs::write(occupied.join("sentinel"), b"preserve")?;
    assert!(write_project(&occupied, &valid).is_err());
    assert_eq!(std::fs::read(occupied.join("sentinel"))?, b"preserve");
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 2);
    Ok(())
}
