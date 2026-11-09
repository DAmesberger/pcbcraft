use super::*;
use pcbcraft_core::Engine;

/// The store's file, which its first snapshot reserved.
fn own(store: &RecoveryStore) -> Result<&Path> {
    store
        .path()
        .ok_or_else(|| Error::Invalid("no snapshot written".into()))
}

/// A session that never wrote a snapshot leaves no file at all, so one
/// that ends without cleaning up (a crash, a kill) leaves nothing empty.
#[test]
fn a_session_without_a_snapshot_leaves_no_file() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = RecoveryStore::new(directory.path())?;
    assert!(store.path().is_none());
    store.remove_own()?;
    drop(store);
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

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
    first.write(&document, Some(&original))?;
    second.write(&plain, None)?;
    assert_ne!(first.path(), second.path());
    let path = own(&first)?.to_path_buf();
    drop(first);
    let entries = list(directory.path())?.entries;
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
    assert_eq!(list(directory.path())?.entries.len(), 1);
    remove_entry(directory.path(), &path)?;
    assert!(list(directory.path())?.entries.is_empty());
    Ok(())
}

#[test]
fn invalid_recovery_write_preserves_previous_valid_snapshot() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = RecoveryStore::new(directory.path())?;
    let file = Engine::demo()?.to_file();
    store.write(&file, None)?;
    let before = std::fs::read(own(&store)?)?;
    let mut invalid = file;
    invalid.version = 999;
    assert!(store.write(&invalid, None).is_err());
    assert_eq!(std::fs::read(own(&store)?)?, before);
    store.remove_own()?;
    store.remove_own()?;
    store.write(&Engine::demo()?.to_file(), None)?;
    assert_eq!(list(directory.path())?.entries.len(), 1);
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
    let meta = RecoveryMeta {
        version: 999,
        original_path: None,
        saved_unix_ms: 1,
    };
    container::write(
        &newer,
        &Engine::demo()?.to_file(),
        Some(&serde_json::to_vec(&meta)?),
        Method::Stored,
    )?;
    let entries = list(directory.path())?.entries;
    assert_eq!(entries.len(), 3);
    assert_eq!(entries.iter().filter(|e| e.error.is_some()).count(), 2);
    assert!(read(&newer).is_err());
    remove_entry(directory.path(), &broken)?;
    assert_eq!(list(directory.path())?.entries.len(), 2);
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
        assert!(list(directory.path())?.entries.is_empty());
        assert!(other.exists());
    }
    Ok(())
}

/// Recovery snapshots are containers: assets survive a restore, a listing
/// reads only the document and metadata, and a damaged asset is refused at
/// restore while the listing still shows the snapshot.
#[test]
fn snapshots_carry_assets_in_the_container_and_listing_skips_them() -> Result<()> {
    use crate::persistence::container::tests::{noise, packed, with_assets};
    let directory = tempfile::tempdir()?;
    let store = RecoveryStore::new(directory.path())?;
    let asset = packed(noise(3 * 1024 * 1024, 11), "model/step");
    let document = with_assets(vec![asset.clone()])?;
    store.write(&document, None)?;
    let path = own(&store)?;
    assert_eq!(read(path)?.document, document);
    let entries = list(directory.path())?.entries;
    assert_eq!(entries.len(), 1);
    assert!(entries[0].error.is_none());

    // Damage the stored asset bytes in place.
    let mut bytes = std::fs::read(path)?;
    let at = bytes
        .windows(32)
        .position(|w| w == &asset.data[..32])
        .ok_or_else(|| Error::Invalid("stored asset bytes".into()))?;
    bytes[at + 1000] ^= 0x5A;
    std::fs::write(path, &bytes)?;
    assert!(read(path).is_err());
    let entries = list(directory.path())?.entries;
    assert_eq!(entries.len(), 1);
    assert!(entries[0].error.is_none());

    // A plain project container is not a recovery snapshot.
    let project = directory.path().join("session-project.pcbcraft-recovery");
    crate::persistence::container::write_container(&project, &document)?;
    let refused = read(&project)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(refused.contains("has no recovery.json"), "{refused}");
    Ok(())
}

/// Past the read limit the newest sessions are still listed and the
/// older ones are named, so they can be discarded; the listing never fails
/// as a whole because one session too many is left over.
#[test]
fn sessions_past_the_limit_are_named_for_discard_not_fatal() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = RecoveryStore::new(directory.path())?;
    store.write(&Engine::demo()?.to_file(), None)?;
    let first = own(&store)?.to_path_buf();
    let base = std::time::SystemTime::now() - std::time::Duration::from_secs(100_000);
    let mut paths = Vec::new();
    for n in 0..MAX_ENTRIES + 2 {
        let path = directory.path().join(format!("session-n{n:04}{SUFFIX}"));
        std::fs::copy(&first, &path)?;
        let time = base + std::time::Duration::from_secs(n as u64 * 10);
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(time)?;
        paths.push(path);
    }
    remove_entry(directory.path(), &first)?;
    let listing = list(directory.path())?;
    assert_eq!(listing.entries.len(), MAX_ENTRIES);
    assert_eq!(listing.older, vec![paths[1].clone(), paths[0].clone()]);
    assert!(
        listing
            .entries
            .iter()
            .all(|e| !listing.older.contains(&e.path))
    );
    for path in &listing.older {
        remove_entry(directory.path(), path)?;
    }
    let again = list(directory.path())?;
    assert_eq!(again.entries.len(), MAX_ENTRIES);
    assert!(again.older.is_empty());
    Ok(())
}
