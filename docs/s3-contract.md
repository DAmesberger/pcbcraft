# Parts library S3 contract: local library folders

Working contract for the four parallel S3 tasks, 31 October 2026. It lists the types and interfaces committed up front, where they live and which files each task owns. Bodies marked **stub** return an `unsupported` error (core `Error::Unsupported`, io `crate::unsupported`) until the owning task implements them. Remove this note once `docs/parts-library.md` gains an "S3 as built" section, as S2 did.

Decisions in force: **D1** remap on import with a recorded origin; random 128-bit lineage ids inside folders. **D2** folder format below. **D7** decided: asset bytes are packed into the project file by content hash. **D13** shelves and tags live in the folder's `catalog.json`, never in revisions or projects. Pre-release format: types are replaced, there are no legacy readers.

## Types and interfaces

### Core identity (`crates/editor-core/src/ids.rs`)

| Type | Meaning |
| --- | --- |
| `LibraryId(u128)` | A folder's identity, generated once by io and stored in its manifest. JSON: 32 lower-case hex digits. `from_bytes`, `parse`, `Display`. |
| `SourceLineageId(u128)` | A lineage's identity inside a folder. Same encoding. |
| `ContentHash([u8; 32])` | SHA-256, JSON as 64 lower-case hex digits. |

The core never generates these; io draws random ids (`library_folder::new_library_id`, `new_lineage_id`) and passes them in as values.

### Core bundle (`crates/editor-core/src/library/bundle.rs`, re-exported from `library`)

- `Origin { library: LibraryId, lineage: SourceLineageId, revision, content_hash: ContentHash }`. This replaces S2's text-typed `Origin`; `library::Origin` and `PartDefinition.provenance.origin` use it.
- `OriginRecord { local: DefinitionRef, origin }`, stored in **`Library.origins`**, outside revision bodies. `validate_snapshot` (called from `library::validate`) requires every `local` to resolve and to appear at most once.
- `PackedAsset { hash, media_type, data: AssetBytes }` (base64 in JSON) is stored in **`Library.assets`** (D7). `validate_assets` checks bounds, the media type, that each hash matches its bytes, and that no hash appears twice.
- `RevisionRecord { lineage, revision, kind, meta, definition, links: Vec<Link { local: DefinitionId, lineage: SourceLineageId }>, assets: Vec<ContentHash> }` is exactly the content of `lineages/<id>/rNNNN.json`. Inside a record, references use record-local ids that `links` maps to source lineages, so a record is self-contained and its hash does not depend on any project's numbering. Locals are numbered 1, 2, … in first-use order of `references`, and `links` is sorted by `local`.
- `SourceRevision { lineage, revision }`.
- `Bundle { library, name, root: SourceRevision, records (dependencies first, root included), assets }` carries a root and its closure. It travels inline in commands.
- `RefSite { Exact { target } | Lineage { kind, id } }`, trait `RefMap { exact, lineage }`.
- Functions:
  - `rewrite_refs(&mut Definition, &mut dyn RefMap)` **stub**
  - `references(&Definition) -> Vec<RefSite>`, built on `rewrite_refs`
  - `closure(&Library, DefinitionRef) -> Vec<DefinitionRef>` **stub**
  - `content_hash(&RevisionRecord)`: SHA-256 of the record's serde JSON. Done.
  - `asset_hash(&[u8])`. Done.
  - `validate_bundle(&Bundle)`: bounds, root present, kind agreement and assets. Done; link resolution is still to add.
- Bounds: `MAX_BUNDLE_RECORDS` 1024, `MAX_LINKS` 256, `MAX_ASSETS` 256, `MAX_ASSET_BYTES` 8 MiB, `MAX_MEDIA_TYPE` 128.

### Core import (`crates/editor-core/src/library/import.rs`)

- `import(&mut Library, &Bundle, allocate) -> ImportReport { root, revisions: Vec<ImportedRevision { record: OriginRecord, reused }> }` **stub**. It dedupes by origin, maps each new source lineage to a fresh project lineage and each source revision to the next project revision of that lineage, rewrites references, appends the revisions, packs assets and records origins.
- `export(&Library, root, target: LibraryId, name, fresh: &[SourceLineageId]) -> PromotePlan { bundle, origins, used }` **stub**. It is pure and deterministic. Members whose origin is already in `target` map back to their source lineage.
- `duplicate(&Library, &mut Drafts, ComponentRef, name, allocate) -> Vec<DefinitionId>` **stub** (D6). It copies the component, its symbols and its bindings into new draft lineages, component first.
- `PlaceSource { Project { component } | Bundle { bundle } }` and `OtherRevision { Refuse (default) | KeepBoth }`. `Refuse` is a `conflict` with code `library.other_revision`, which lets the host offer Keep both / Update project copy / Cancel. "Update project copy" is `library.import` followed by the existing update preview.

### Core catalog (`crates/editor-core/src/catalog.rs`)

- `Source::Library(LibraryId)` sits beside `Source::Project`.
- `EntryKey { Project { lineage } | Library { library, lineage } }` keys favourites and recents. `CatalogEntry::key()` returns it.
- `CatalogEntry` gains:
  - `source_lineage: Option<SourceLineageId>`. On a folder entry `lineage` is catalog-local.
  - `update: Option<UpdateAvailable>`.
  - `component()` / `part()` now return `None` for folder entries; place those through `library.place_from`.
- `CatalogContext { favourites: BTreeSet<EntryKey>, recent: Vec<EntryKey>, sources: Vec<Arc<SourceCatalog>> }`. `sources` is serde-skipped; the host fills it from io. `CatalogContext::of_project(&[DefinitionId], &[DefinitionId])` serves S1 callers.
- `SourceCatalog { library, name, lineages: Vec<Link>, entries, fields: Library (quantity kinds and fields in catalog-local ids, the source's vocabulary), revisions: Vec<SourceRecordInfo { lineage, revision, kind, content_hash }> }`. It is serializable so io can cache it.
- `source_catalog(LibraryId, name, &[RevisionRecord]) -> SourceCatalog` **stub**.
- `UpdateAvailable { local, origin, source, latest, modified }`. `modified` marks a source record that no longer hashes to `origin.content_hash` (finding `library.source_modified`). `updates_available(&Library, &[Arc<SourceCatalog>])` returns empty with no sources or origins, and is otherwise a **stub**.
- `Index::with_sources(project, drafts, sources)` returns project entries followed by source entries. In this contract it uses only the project's vocabulary; merging source vocabularies and setting `update` marks belong to task 1. `library.search` and `library.facets` now use it.

### Engine (`crates/editor-core/src/lib.rs`, `commands/`)

| Wire name | Type | Outcome / result |
| --- | --- | --- |
| `library.import { bundle }` | `LibraryCommand::Import` | `Outcome::Imported { report }`. Validates, then calls `import::import` (stub). |
| `library.place_from { source, position, designator?, other_revision? }` | `LibraryCommand::PlaceFrom` | `Outcome::PlacedFrom { import?, instance, occurrence, designator }`. **Stub** handler. |
| `library.promote { root, library, name, fresh }` | `LibraryCommand::Promote` | `Outcome::Promoted { origins }`. Recomputes `export` and records the origins. |
| `library.duplicate { component, name }` | `LibraryCommand::Duplicate` | `Outcome::Duplicated { lineages }` |
| `library.promote_preview { root, library, name, fresh }` | `Query::PromotePreview` | `QueryResult::Promote(PromotePlan)` |
| `library.updates_available {}` | `Query::UpdatesAvailable` | `QueryResult::UpdatesAvailable(Vec<UpdateAvailable>)` |

Promote runs in three steps. First, `library.promote_preview` computes the plan. Then io's `write_bundle` writes it to the folder. Finally, `library.promote` with the same arguments records the origins as one undo step. The same document revision gives the same plan. Queries that need folders take `CatalogContext::sources` through `Engine::query_with`.

### io (`crates/io/src/`)

- **`library_folder.rs`**
  - Constants: `MANIFEST_FILE` (`pcbcraft-library.json`), `CATALOG_FILE` (`catalog.json`), `FOLDER_FORMAT` 1, `SUPPORTED_FEATURES` (empty), `MAX_FOLDER_RECORDS`, `MAX_RECORD_BYTES`.
  - Types:
    - `Manifest { format, library, name, required_features, default_licence }`
    - `Labels { shelf, tags }`
    - `CatalogLabels { format, lineages: BTreeMap<SourceLineageId, Labels> }`
    - `Folder { root, manifest }`
    - `StoredRecord { path, record, content_hash }`
    - `WriteReport`
    - `Disabled { Off | PathMissing | Malformed | UnsupportedFormat | UnknownFeatures }`, with `Display` giving the Browser's reason text
  - Done: `new_library_id` and `new_lineage_id` (OS random through `getrandom`), `record_path`, `asset_path`, `check_manifest`, `open_folder` (which refuses another format or an unknown feature as `unsupported`).
  - **Stubs:** `create_folder`, `read_records`, `read_bundle`, `write_bundle` (assets first; never overwrites an rN with other content), `read_labels`, `write_labels`.
- **`library_folder_index.rs`**: `INDEX_FORMAT`, `SourceIndex { format, library, manifest_hash, files, catalog }`, `IndexState { Cached | Rebuilt }`, `index_path(cache_dir, LibraryId)` (`<cache>/index/<id>.json`). `load_or_build` is a **stub**.
- **`settings.rs`**: `UserSettings { format, sources: Vec<SourceSetting { path, enabled, writable }>, favourites: Vec<EntryKey>, recent: Vec<EntryKey> }`, with bounds `MAX_SOURCES` 64, `MAX_FAVOURITES` 256 and `MAX_RECENT` 32. `read_settings` (a missing file gives the defaults; another format is refused) and `write_settings` (atomic, pretty JSON) are done. `UserDirs { config, cache }` is done; `UserDirs::from_env` is a **stub** that will honour `PCBCRAFT_CONFIG_DIR` and `PCBCRAFT_CACHE_DIR`.

Dependencies added: `sha2` 0.10 (core) and `getrandom` 0.3 (io), both MIT OR Apache-2.0 and both already in the lock file through Solidcraft.

## Parallel tasks and the files each owns

Keep edits to shared files additive. Other concurrent slices (F4/F5 footprint UI, M1–M3 generated 3D, SVG import) touch the Component workspace and footprint files, so avoid those files.

### (1) Core

**Owns:**
- `crates/editor-core/src/library/bundle*.rs`, `library/import*.rs` and their tests
- the S3 additions in `catalog.rs`: `source_catalog`, `updates_available`, `Index::with_sources` with vocabulary merging and `update` marks, and the ranking over `EntryKey`
- the S3 handlers in `commands/library.rs`
- the S3 queries in `lib.rs`
- core tests under `crates/editor-core/src/tests/` (new file `library_folders.rs`)

**Delivers:**
- the complete reference-rewrite visitor over every definition kind
- `closure`
- link resolution in `validate_bundle`
- `import`, with dedupe by origin, revision mapping, asset packing and origins
- `export`
- `duplicate`
- `library.place_from` as one transaction, using `OtherRevision`
- update-available findings, including `library.source_modified`
- content-hash handling for packed assets, with `Reference.asset` resolving in `Library.assets` or being reported
- the import/export round-trip property test (generated closures of every kind; import, then export, gives equal records and hashes)

### (2) io

**Owns:** `crates/io/src/library_folder*.rs`, `crates/io/src/settings*.rs` and their tests.

**Delivers:**
- the folder reader and writer: manifest refusal, immutable revision files, the asset store, `catalog.json` labels and atomic writes
- git-friendly output: stable key order, pretty JSON with a final newline, one file per revision
- the index cache in the user cache directory, which rebuilds on mismatch, rebuilds identically after deletion, and rescans only changed lineages
- `UserDirs::from_env`

**Evidence owned:** a corrupt or unknown-feature manifest is rejected; a modified rN is detected by hash; a deleted index rebuilds identically. Index tests need task 1's `source_catalog`. Until integration, write them against `StoredRecord` hashes and mark the catalog equality assertion to run once `source_catalog` lands. Never weaken it.

### (3) UI (desktop)

**Owns:** new `apps/pcbcraft/src/library_sources*.rs`, and additive changes to `place_palette.rs` and the Browser code that lists components.

**Delivers:**
- library sources in the Browser: each source listed, and a disabled source shown with its `Disabled` reason instead of vanishing
- Place palette rows from folders under a `LIBRARY · <name>` caption, with update-available marks
- Enter sends `library.place_from`; on `library.other_revision` the palette shows the inline Keep both / Update project copy / Cancel choice
- `Promote to library…` and `Duplicate` actions in the Component workspace, added through a small hook in `part_editor.rs` and implemented in `library_sources*.rs`
- a sources settings card (add folder, rescan)
- favourites (Cmd+D in the palette) and recents persisted through `settings.rs`, keyed by `EntryKey`

Folder loading and indexing run off the UI thread.

### (4) Automation, CLI and docs

**Owns:**
- `crates/automation/src/session*` additions: a session-level source registry, `library.sources` listing, and passing `CatalogContext::sources` into queries
- `crates/automation/src/mcp*` additions
- `apps/pcbcraft-cli` additions
- automation tests
- docs: `docs/parts-library.md` "S3 as built", the component-model rows, `GLOSSARY.md` (Library source, Library folder, Shelf, Tag, Catalog index, Origin) and `docs/editor-evidence.md`

**Delivers:**
- schemas generated for every new command and query
- a CLI subcommand that creates a sample folder library from `catalog::synthetic`
- JSON-only automation tests for the evidence scenario: install the folder, place from it, publish r+1 in the folder, update only R1, delete the folder, reopen offline
- removal of this contract note in the final docs pass

## Evidence required for S3

- The scenario: install a folder, place from it, publish r+1 in the folder, update only R1, delete the folder, reopen the project offline (task 4, on tasks 1 and 2).
- A corrupt or unknown-feature manifest is rejected, a modified rN is detected by hash, and a deleted index rebuilds identically (task 2).
- The import/export round-trip property test (task 1).
