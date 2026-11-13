//! The per-source catalog index cache (S3): `<cache>/sparksmith/index/
//! <LibraryId>.json`, holding the manifest hash, each record's content hash
//! and the `SourceCatalog` built by `sparksmith_core::catalog::source_catalog`.
//! Never authoritative: a version mismatch, an unreadable file or any
//! changed record rebuilds the whole index from every record, and a
//! deleted index rebuilds identically. A cache that cannot be written (an
//! index over the 16 MiB file bound, a read-only cache directory) is not an
//! error: the catalog is used from memory and rebuilt next time. Never
//! written into a project or a library folder. Run off the UI thread.
use crate::library_folder::{
    Folder, MANIFEST_FILE, MAX_RECORD_BYTES, StoredRecord, pretty, read_bounded, read_records,
    record_files,
};
use crate::persistence::{atomic_write, read_bytes};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sparksmith_core::catalog::{SourceCatalog, source_catalog};
use sparksmith_core::ids::{ContentHash, LibraryId};
use sparksmith_core::library::bundle::{RevisionRecord, asset_hash};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The cache file's own format; any other is rebuilt, never migrated.
pub const INDEX_FORMAT: u32 = 1;

/// One cached index.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceIndex {
    pub format: u32,
    pub library: LibraryId,
    /// SHA-256 of the manifest bytes.
    pub manifest_hash: ContentHash,
    /// Content hash per record path, sorted.
    pub files: BTreeMap<String, ContentHash>,
    /// SHA-256 of each record file's bytes, sorted by path: what the
    /// freshness check compares, so a matching cache is confirmed without
    /// parsing any record.
    #[serde(default)]
    pub bytes: BTreeMap<String, ContentHash>,
    pub catalog: SourceCatalog,
}

/// How a load went, for the Browser and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexState {
    /// The cache matched the folder.
    Cached,
    /// The cache was missing, stale or unreadable and was rebuilt.
    Rebuilt,
    /// Rebuilt, but the cache file could not be written (the reason): the
    /// catalog is used from memory and rebuilt on the next scan.
    Uncached(String),
}

/// `<cache_dir>/index/<library>.json`.
pub fn index_path(cache_dir: &Path, library: LibraryId) -> PathBuf {
    cache_dir.join("index").join(format!("{library}.json"))
}

/// The folder's catalog from the cache, rebuilt and rewritten when it does
/// not match the folder.
pub fn load_or_build(cache_dir: &Path, folder: &Folder) -> Result<(SourceCatalog, IndexState)> {
    load_or_build_with(cache_dir, folder, |library, name, records| {
        Ok(source_catalog(library, name, records)?)
    })
    .map(|(index, state)| (index.catalog, state))
}

/// [`load_or_build`] with the catalog builder passed in, returning the
/// whole index. The cache matches when its format, library, manifest bytes
/// and every record file's bytes match the folder; then no record is
/// parsed. Otherwise every record is read and checked (`build` needs them
/// all) and the index is rewritten. An unreadable or foreign cache file is
/// rebuilt, never trusted. The output is a pure function of the folder, so
/// a deleted index rebuilds byte for byte.
pub fn load_or_build_with(
    cache_dir: &Path,
    folder: &Folder,
    build: impl FnOnce(LibraryId, &str, &[RevisionRecord]) -> Result<SourceCatalog>,
) -> Result<(SourceIndex, IndexState)> {
    let library = folder.manifest.library;
    let manifest_hash = asset_hash(&read_bounded(
        &folder.root.join(MANIFEST_FILE),
        MAX_RECORD_BYTES,
    )?);
    let mut bytes = BTreeMap::new();
    for (path, _, _) in record_files(folder)? {
        let hash = asset_hash(&read_bounded(&folder.root.join(&path), MAX_RECORD_BYTES)?);
        bytes.insert(path, hash);
    }
    let path = index_path(cache_dir, library);
    if let Some(index) = read_index(&path)
        && index.library == library
        && index.manifest_hash == manifest_hash
        && index.bytes == bytes
        && index.files.keys().eq(bytes.keys())
        && index.catalog.library == library
    {
        return Ok((index, IndexState::Cached));
    }
    let stored: Vec<StoredRecord> = read_records(folder)?;
    let files: BTreeMap<String, ContentHash> = stored
        .iter()
        .map(|s| (s.path.clone(), s.content_hash))
        .collect();
    if files.keys().ne(bytes.keys()) {
        return Err(Error::Invalid(
            "the library folder changed while it was being indexed; rescan".into(),
        ));
    }
    let records: Vec<RevisionRecord> = stored.into_iter().map(|s| s.record).collect();
    let catalog = build(library, &folder.manifest.name, &records)?;
    let index = SourceIndex {
        format: INDEX_FORMAT,
        library,
        manifest_hash,
        files,
        bytes,
        catalog,
    };
    match store(&path, &index) {
        Ok(()) => Ok((index, IndexState::Rebuilt)),
        Err(error) => Ok((index, IndexState::Uncached(error.to_string()))),
    }
}

/// Write the cache file. Its failure never fails the load.
fn store(path: &Path, index: &SourceIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(path, &pretty(index)?)
}

/// A cached index, or `None` when it is missing, unreadable or in another
/// format: all of which simply rebuild.
fn read_index(path: &Path) -> Option<SourceIndex> {
    let bytes = read_bytes(path).ok()?;
    let index: SourceIndex = serde_json::from_slice(&bytes).ok()?;
    (index.format == INDEX_FORMAT).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_folder::folder_tests::{bundle, field};
    use crate::library_folder::{create_folder, record_path, write_bundle};
    use sparksmith_core::ids::{Revision, SourceLineageId};

    /// The real catalog builder, as `load_or_build` uses it.
    fn listing(
        library: LibraryId,
        name: &str,
        records: &[RevisionRecord],
    ) -> Result<SourceCatalog> {
        Ok(sparksmith_core::catalog::source_catalog(
            library, name, records,
        )?)
    }

    fn untouched(_: LibraryId, _: &str, _: &[RevisionRecord]) -> Result<SourceCatalog> {
        Err(Error::Invalid("a matching cache must not rebuild".into()))
    }

    fn sample() -> Result<(tempfile::TempDir, tempfile::TempDir, Folder)> {
        let library = tempfile::tempdir()?;
        let cache = tempfile::tempdir()?;
        let folder = create_folder(library.path(), "Bench")?;
        write_bundle(
            &folder,
            &bundle(
                &folder,
                vec![field(1, 1, "Tolerance"), field(2, 1, "Voltage")],
                vec![],
            ),
        )?;
        Ok((library, cache, folder))
    }

    #[test]
    fn a_matching_cache_is_used_and_a_deleted_one_rebuilds_identically() -> Result<()> {
        let (_library, cache, folder) = sample()?;
        let (built, state) = load_or_build_with(cache.path(), &folder, listing)?;
        assert_eq!(state, IndexState::Rebuilt);
        assert_eq!(built.files.len(), 2);
        assert_eq!(built.catalog.revisions.len(), 2);
        let path = index_path(cache.path(), folder.manifest.library);
        let first = std::fs::read(&path)?;

        let (cached, state) = load_or_build_with(cache.path(), &folder, untouched)?;
        assert_eq!(state, IndexState::Cached);
        assert_eq!(cached, built);

        std::fs::remove_file(&path)?;
        let (rebuilt, state) = load_or_build_with(cache.path(), &folder, listing)?;
        assert_eq!(state, IndexState::Rebuilt);
        assert_eq!(rebuilt, built);
        assert_eq!(std::fs::read(&path)?, first, "byte for byte");
        Ok(())
    }

    #[test]
    fn a_changed_folder_or_a_bad_cache_rebuilds() -> Result<()> {
        let (library, cache, folder) = sample()?;
        let (built, _) = load_or_build_with(cache.path(), &folder, listing)?;
        let path = index_path(cache.path(), folder.manifest.library);

        // A modified rN: its content hash in the index follows the file.
        let record = library
            .path()
            .join(record_path(SourceLineageId(2), Revision(1)));
        let text = std::fs::read_to_string(&record)?;
        std::fs::write(&record, text.replace("Voltage", "Voltage (edited)"))?;
        let (edited, state) = load_or_build_with(cache.path(), &folder, listing)?;
        assert_eq!(state, IndexState::Rebuilt);
        let key = record_path(SourceLineageId(2), Revision(1));
        assert_ne!(edited.files.get(&key), built.files.get(&key));
        assert_eq!(
            edited
                .files
                .get(&record_path(SourceLineageId(1), Revision(1))),
            built
                .files
                .get(&record_path(SourceLineageId(1), Revision(1)))
        );

        // A new revision.
        write_bundle(
            &folder,
            &bundle(&folder, vec![field(1, 2, "Tolerance r2")], vec![]),
        )?;
        let (grown, state) = load_or_build_with(cache.path(), &folder, listing)?;
        assert_eq!(state, IndexState::Rebuilt);
        assert_eq!(grown.files.len(), 3);

        // A renamed library (manifest change).
        let manifest = library.path().join(crate::library_folder::MANIFEST_FILE);
        let text = std::fs::read_to_string(&manifest)?;
        std::fs::write(&manifest, text.replace("Bench", "Bench two"))?;
        let renamed = crate::library_folder::open_folder(library.path())?;
        let (index, state) = load_or_build_with(cache.path(), &renamed, listing)?;
        assert_eq!(state, IndexState::Rebuilt);
        assert_eq!(index.catalog.name, "Bench two");

        // Garbage and foreign-format caches are rebuilt, never trusted.
        for junk in ["not json", "{\"format\":99}"] {
            std::fs::write(&path, junk)?;
            let (_, state) = load_or_build_with(cache.path(), &renamed, listing)?;
            assert_eq!(state, IndexState::Rebuilt);
        }
        let mut stale: SourceIndex = serde_json::from_slice(&std::fs::read(&path)?)?;
        stale.format = INDEX_FORMAT + 1;
        std::fs::write(&path, serde_json::to_vec(&stale)?)?;
        let (_, state) = load_or_build_with(cache.path(), &renamed, listing)?;
        assert_eq!(state, IndexState::Rebuilt);
        Ok(())
    }

    /// An index that cannot be cached (larger than the 16 MiB file bound,
    /// or an unwritable cache directory) still loads from memory.
    #[test]
    fn an_uncachable_index_still_loads() -> Result<()> {
        let (_library, cache, folder) = sample()?;
        let huge = |library: LibraryId, _: &str, records: &[RevisionRecord]| {
            let mut catalog = listing(library, "Bench", records)?;
            catalog.name = "x".repeat(sparksmith_core::format::MAX_FILE_BYTES + 1);
            Ok(catalog)
        };
        let (index, state) = load_or_build_with(cache.path(), &folder, huge)?;
        assert!(matches!(state, IndexState::Uncached(_)), "{state:?}");
        assert_eq!(index.catalog.revisions.len(), 2);
        assert!(!index_path(cache.path(), folder.manifest.library).exists());

        let blocked = cache.path().join("not-a-directory");
        std::fs::write(&blocked, "a file")?;
        let (catalog, state) = load_or_build(&blocked, &folder)?;
        assert!(matches!(state, IndexState::Uncached(_)), "{state:?}");
        assert_eq!(catalog.revisions.len(), 2);
        Ok(())
    }

    /// Folder indexing and searching at scale. Run with
    /// `cargo test --release -p sparksmith-io library_folder_index::tests::bench -- --ignored --nocapture`.
    /// Targets (not gates, not yet measured on CI hardware): a cold index
    /// of 1 000 component lineages ≤ 2 s, a cached load ≤ 200 ms, and
    /// `Index::with_sources` plus one search (what each keystroke costs the
    /// palette) ≤ 30 ms.
    #[test]
    #[ignore = "benchmark; run explicitly in release mode"]
    fn bench_index_and_search_a_large_folder() -> Result<()> {
        use sparksmith_core::catalog::{CatalogContext, Index, SearchOptions};
        use sparksmith_core::library::import;
        use std::sync::Arc;
        use std::time::Instant;
        let library_dir = tempfile::tempdir()?;
        let cache = tempfile::tempdir()?;
        let folder = create_folder(library_dir.path(), "Bench")?;
        let mut project = sparksmith_core::catalog::synthetic::project(1_000, &sparksmith_core::NewBoard::fixture())?;
        let roots: Vec<_> = project
            .library
            .latest_components()
            .iter()
            .map(|c| sparksmith_core::ids::DefinitionRef {
                kind: sparksmith_core::DefinitionKind::Component,
                id: c.id,
                revision: c.revision,
            })
            .collect();
        let started = Instant::now();
        for root in roots {
            let fresh = (0..64)
                .map(|_| crate::library_folder::new_lineage_id())
                .collect::<Result<Vec<_>>>()?;
            let plan = import::export(
                &project.library,
                root,
                folder.manifest.library,
                "Bench",
                &fresh,
            )?;
            project.library.origins.extend(plan.origins.iter().copied());
            write_bundle(&folder, &plan.bundle)?;
        }
        let files = crate::library_folder::record_files(&folder)?.len();
        println!(
            "wrote {files} records in {:.0} ms",
            started.elapsed().as_secs_f64() * 1e3
        );
        let started = Instant::now();
        let (catalog, state) = load_or_build(cache.path(), &folder)?;
        println!(
            "cold index ({state:?}): {:.0} ms, {} entries",
            started.elapsed().as_secs_f64() * 1e3,
            catalog.entries.len()
        );
        let started = Instant::now();
        let (_, state) = load_or_build(cache.path(), &folder)?;
        println!(
            "cached load ({state:?}): {:.0} ms",
            started.elapsed().as_secs_f64() * 1e3
        );
        let sources = vec![Arc::new(catalog)];
        let empty = sparksmith_core::model::Project::default();
        let drafts = sparksmith_core::library::Drafts::default();
        for text in ["10k", "res>=10k pkg:0603", "ex-c"] {
            let started = Instant::now();
            let index = Index::with_sources(&empty, &drafts, &sources)?;
            let query = index
                .parse(text)
                .map_err(|e| Error::Invalid(format!("{e:?}")))?;
            let report = index.search(&query, &CatalogContext::default(), SearchOptions::default());
            println!(
                "with_sources + search {text:?}: {:.1} ms, {} matches",
                started.elapsed().as_secs_f64() * 1e3,
                report.total
            );
        }
        Ok(())
    }

    #[test]
    fn index_paths_are_per_library() {
        let path = index_path(Path::new("/cache"), LibraryId(0xab));
        assert_eq!(
            path,
            Path::new("/cache/index/000000000000000000000000000000ab.json")
        );
    }

    /// The real catalog builder: a deleted index rebuilds to an equal
    /// catalog. Same assertion as above, through `load_or_build`.
    #[test]
    fn the_real_catalog_rebuilds_identically() -> Result<()> {
        let (_library, cache, folder) = sample()?;
        let (built, state) = load_or_build(cache.path(), &folder)?;
        assert_eq!(state, IndexState::Rebuilt);
        let (cached, state) = load_or_build(cache.path(), &folder)?;
        assert_eq!(state, IndexState::Cached);
        assert_eq!(cached, built);
        let path = index_path(cache.path(), folder.manifest.library);
        let first = std::fs::read(&path)?;
        std::fs::remove_file(&path)?;
        let (rebuilt, state) = load_or_build(cache.path(), &folder)?;
        assert_eq!(state, IndexState::Rebuilt);
        assert_eq!(rebuilt, built);
        assert_eq!(std::fs::read(&path)?, first);
        Ok(())
    }
}
