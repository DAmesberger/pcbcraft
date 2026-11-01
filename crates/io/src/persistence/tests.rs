use super::*;
use pcbcraft_core::{
    DefinitionKind, Engine, InstanceCommand, LibraryCommand, Operation,
    library::{Definition, DefinitionMeta},
};

/// An empty component draft and the demo binding draft emptied of mappings.
pub(crate) fn unfinished(engine: &mut Engine) -> Result<()> {
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::NewDraft {
            meta: DefinitionMeta::default(),
            definition: Definition::empty(DefinitionKind::Component),
        }),
    )?;
    let mut binding = engine
        .drafts()
        .items
        .iter()
        .find(|d| d.definition.kind() == DefinitionKind::Binding)
        .cloned()
        .ok_or_else(|| Error::Invalid("demo binding draft".into()))?;
    if let Definition::Binding(b) = &mut binding.definition {
        b.mappings.clear();
    }
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Edit {
            edits: vec![pcbcraft_core::library::DraftEdit {
                id: binding.id,
                meta: binding.meta,
                definition: binding.definition,
            }],
        }),
    )?;
    Ok(())
}

#[test]
fn saved_project_reopens_offline_with_exact_revisions_and_drafts() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("offline.pcbcraft");
    let mut engine = Engine::demo()?;
    unfinished(&mut engine)?;
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
    let r1 = engine.project().instances[0].id;
    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::SetDesignator {
            instance: r1,
            designator: "R10".into(),
        }),
    )?;
    let before = engine.to_file();
    let mut newer = serde_json::to_value(&before)?;
    newer["version"] = serde_json::json!(999);
    for bytes in [b"not json".to_vec(), serde_json::to_vec(&newer)?] {
        std::fs::write(&path, bytes)?;
        assert!(read_project(&path).is_err());
    }
    // A container whose document is of a newer version.
    container::tests::raw_zip(
        &path,
        &[(
            format::CONTAINER_DOCUMENT,
            &serde_json::to_vec(&newer)?,
            zip::CompressionMethod::Deflated,
        )],
    )?;
    assert!(matches!(
        read_project(&path),
        Err(Error::Document(pcbcraft_core::Error::Unsupported(_)))
    ));
    let large = std::fs::File::create(&path)?;
    large.set_len(format::CONTAINER_MAX_BYTES + 1)?;
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

#[test]
fn atomic_create_never_replaces_an_existing_file() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("pins.h");
    atomic_create(&path, b"first")?;
    assert_eq!(std::fs::read(&path)?, b"first");
    let again = atomic_create(&path, b"second");
    assert!(
        matches!(&again, Err(crate::Error::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "{again:?}"
    );
    assert_eq!(std::fs::read(&path)?, b"first");
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
    Ok(())
}
