use super::*;
use pcbcraft_core::Engine;

#[test]
fn independent_sessions_survive_drop_and_restore_project_plus_unfinished_draft() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let original = directory.path().join("original.pcbcraft");
    std::fs::write(&original, b"original is never overwritten by recovery")?;
    let mut engine = Engine::demo()?;
    let plain = engine.to_file();
    crate::persistence::tests::unfinished(&mut engine)?;
    let document = engine.to_file();
    let first = RecoveryStore::new(directory.path())?;
    let second = RecoveryStore::new(directory.path())?;
    assert_ne!(first.path(), second.path());
    first.write(&document, Some(&original))?;
    second.write(&plain, None)?;
    let path = first.path().to_path_buf();
    drop(first);
    let entries = list(directory.path())?;
    assert_eq!(entries.len(), 2);
    let record = read(&path)?;
    assert_eq!(record.document, document);
    assert_eq!(record.original_path, Some(original.clone()));
    let mut recovered = Engine::default();
    recovered.replace_document(record.document)?;
    assert_eq!(recovered.to_file(), document);
    assert_eq!(
        std::fs::read(&original)?,
        b"original is never overwritten by recovery"
    );
    second.remove_own()?;
    assert_eq!(list(directory.path())?.len(), 1);
    remove_entry(directory.path(), &path)?;
    assert!(list(directory.path())?.is_empty());
    Ok(())
}

#[test]
fn invalid_recovery_write_preserves_previous_valid_snapshot() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = RecoveryStore::new(directory.path())?;
    let file = Engine::demo()?.to_file();
    store.write(&file, None)?;
    let before = std::fs::read(store.path())?;
    let mut invalid = file;
    invalid.version = 999;
    assert!(store.write(&invalid, None).is_err());
    assert_eq!(std::fs::read(store.path())?, before);
    store.remove_own()?;
    store.remove_own()?;
    store.write(&Engine::demo()?.to_file(), None)?;
    assert_eq!(list(directory.path())?.len(), 1);
    Ok(())
}

#[test]
fn corrupted_or_newer_records_do_not_hide_other_recoveries() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let valid = RecoveryStore::new(directory.path())?;
    valid.write(&Engine::demo()?.to_file(), None)?;
    let broken = directory.path().join("session-broken.pcbcraft-recovery");
    std::fs::write(&broken, b"broken")?;
    let newer = directory.path().join("session-newer.pcbcraft-recovery");
    let mut record = read(valid.path())?;
    record.version = 999;
    std::fs::write(&newer, serde_json::to_vec(&record)?)?;
    let entries = list(directory.path())?;
    assert_eq!(entries.len(), 3);
    assert_eq!(entries.iter().filter(|e| e.error.is_some()).count(), 2);
    assert!(read(&newer).is_err());
    remove_entry(directory.path(), &broken)?;
    assert_eq!(list(directory.path())?.len(), 2);
    Ok(())
}

#[test]
fn discard_is_limited_to_direct_regular_session_files() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let other = outside.path().join("session-outside.pcbcraft-recovery");
    std::fs::write(&other, b"preserve")?;
    assert!(remove_entry(directory.path(), &other).is_err());
    let ordinary = directory.path().join("project.pcbcraft");
    std::fs::write(&ordinary, b"preserve")?;
    assert!(remove_entry(directory.path(), &ordinary).is_err());
    assert!(other.exists());
    assert!(ordinary.exists());
    #[cfg(unix)]
    {
        let link = directory.path().join("session-link.pcbcraft-recovery");
        std::os::unix::fs::symlink(&other, &link)?;
        assert!(remove_entry(directory.path(), &link).is_err());
        assert!(read(&link).is_err());
        assert!(list(directory.path())?.is_empty());
        assert!(other.exists());
    }
    Ok(())
}
