# Sparksmith file formats: plan

14 November 2026. Planning only: nothing in the repository was changed, built or run beyond read-only inspection. Founder direction: no users yet, so design the formats properly now; git-diffable is very useful.

Status words used here: **measured** (I ran it on the repository today), **read** (from code or docs), **target** (not measured), **proposed** (my recommendation, not a founder decision).

## Summary

1. **A project is a tree of small text files.** On disk the tree is either a folder (the working form, for git) or a ZIP of exactly that tree (the sharing, recovery and browser form). One pure encoder in the core produces the tree; `sparksmith-io` puts it in a folder or a ZIP. The product plan already expected this ("a directory representation can support version control, with a packed form for sharing later", docs/sparksmith-plan.md).
2. **Stay with JSON, in two fixed layouts.** Collections are JSON Lines: one object per line, sorted by id. Single documents are JSON with a fixed line layout. Both are written by the canonical codec the generator ABI already has (sorted keys, integers only, NFC). No floats in any file once the units migration finishes.
3. **Derived data leaves the saved document.** Computed fill copper is 70 % of today's example file (measured). It moves to a cache keyed by its inputs.
4. **Version every break from now on**, with a converter chain that runs outside the app until the first release and inside it afterwards, and a CI gate that fails when a stored type changes without a version change.
5. Roughly **75–110 agent hours** in eleven slices, most of them parallel after a contract slice. A semantic merge tool is a separate, later 12–18 hours.

## 1. What is there today

| Fact | Source |
|---|---|
| `.sparksmith` is a ZIP: one `project.json` (pretty JSON, capped at 16 MiB) and `assets/sha256/<hash>`. Unknown entries are refused. Bytes are reproducible. | read: `crates/editor-core/src/format.rs`, `crates/io/src/persistence/container.rs` |
| `FILE_VERSION` is 1 and has never changed, though the format has changed incompatibly several times. `cargo xtask convert-v1` converts in place, idempotently, with no version to tell which files need it. | read: `format.rs`, `xtask/src/convert_v1/` |
| The ESP32 example's `project.json` is 5,138,110 bytes. Computed fill copper is 3,624,289 bytes of that (four fills). Without it: 943,205 bytes pretty, 353,647 bytes compact. | measured |
| Git sees the example as binary: its last four commits each read `Bin 529032 -> 529602 bytes`. | measured: `git log --stat` |
| Units are mixed: schematic and library points are integer nanometres, the board is f64 millimetres. The `f64-lengths` allowlist holds 1 U1, 26 U2 and 54 U3 entries. | measured: `xtask/src/f64_lengths.rs` |
| Object ids are small integers from one counter (`next_id`), nets from another (`next_net`), both saved. | read: `model.rs` |
| A library revision's content hash is the SHA-256 of `serde_json::to_vec(record)`, so it depends on Rust field order. | read: `library/bundle.rs:965` |
| Library folders are already one immutable file per revision: `lineages/<128-bit id>/r0001.json`, `assets/sha256/<hash>`, `catalog.json`. | read: `crates/io/src/library_folder.rs` |
| The board outline sketch is saved as a JSON string inside the JSON (escaped quotes); waiver keys are strings holding JSON. | measured |
| `.solidcraft` part files are pretty JSON text (`"format": "solidcraft/43"`), 2.6–156 KB. | measured |
| Recovery snapshots are the same container plus `recovery.json`, written to a private per-user folder. Snapshots of another version are listed as unreadable and never migrated. | read: `recovery.rs` |
| Settings, keymap, library manifest, catalog labels, shipped data files and the interface sidecar each carry their own `format: 1`. | read |
| Net membership and model traces are stored state, not caches: a net id follows "the group holding most of its former terminals", and a lost model reference keeps its last geometry. | read: `nets.rs`, `model/dimension.rs` |

## 2. Requirements

Founder requirements: all artefacts covered; designed properly now; git-diffable. The rest are proposed, derived from AGENTS.md and the review.

| # | Requirement | What it means concretely |
|---|---|---|
| R1 | Diffable | Text, one object per line where objects are many, one concern per file. A one-object edit is a one-line diff in one file. |
| R2 | No churn | Open then save changes no byte. Save rewrites only files whose bytes differ. No timestamps, writer versions, view state or caches in versioned files. |
| R3 | Stable order and ids | Collections sorted by id. Ids never reused. File names come from ids, never from names a user can change. |
| R4 | Deterministic bytes | One canonical encoder. The same document gives the same bytes on every platform. |
| R5 | Integer units | Lengths in nanometres, angles in millidegrees, no JSON floats. |
| R6 | Safe to merge | A merge that would corrupt the design is a conflict or a refusal that names the file and line, never a silently wrong board. Automatic merging is a later tool, not a property of the text. |
| R7 | Self-contained offline | Every referenced revision, model and generator module is in the tree, by content hash. |
| R8 | Atomic saves and recovery | A crash at any point leaves the old or the new project, never a mix. Recovery keeps unsaved work and survives a format change. |
| R9 | Scale | 10,000 vias, 50,000 tracks and libraries of 20,000 revisions without a whole-document size cap. Targets until measured. |
| R10 | Core stays pure | The encoder and decoder are in the wasm-compatible core and never touch a file. |
| R11 | Versioned | Every break changes a number. A refusal names the version and the converter. Unknown versions, features and entries are refused, never dropped. |
| R12 | Integrity | Content-addressed assets, canonical revision hashes, and a tree digest for traceability. Signatures only where something is distributed. |
| R13 | Readable and editable by tools | A person, a script or an LLM can read and change a file; every load validates fully and refuses with file and line. |

## 3. Options

### 3.1 Container

| Option | For | Against |
|---|---|---|
| Single ZIP (today) | One file to send. Atomic by one rename. | Binary to git: no diff, no merge, about 0.5 MB of new history per save of the example (measured). Whole file rewritten each save. |
| Folder only | Best diffs. Saves touch only what changed. | Easy to send incompletely. Needs multi-file atomicity. No single file for recovery, bug reports or the browser. |
| **Both, one tree (recommended)** | Folder for work, ZIP of the same tree for sharing. One encoder, two stores. | Two stores to test. Users meet two forms. |
| Database file (SQLite) | Partial writes, very large designs. | Not diffable; a second engine; hard in wasm. Nothing measured calls for it. |

### 3.2 Encoding

| Option | For | Against |
|---|---|---|
| Pretty JSON (today) | Readable. | Ten lines per object, three times the bytes (measured: 5.14 MB against 1.72 MB compact). Appending to an array edits the previous line's comma. |
| **JSON Lines for collections, laid-out JSON for documents (recommended)** | One line per object. No commas between records. Streamed, bounded per line, errors name a line. Same serde types, same schemas MCP already publishes, `jq` and every language read it. The repository already uses JSON Lines for replays and transports. | Long lines for big objects. Verbose keys (cheap after compression). No comments. |
| S-expressions | Compact; familiar from other EDA tools. | A parser, printer and schema of our own; no gain over JSON Lines. |
| TOML | Good for hand-edited settings. | Poor for deep nesting and large record arrays; no canonical form. |
| RON | Close to Rust types. | Rust-only readers. |
| Custom line format | Densest. | Everything custom, including every third-party reader. |

### 3.3 How other tools behave

From public documentation and common knowledge of their behaviour only; no source was read, and none of this is verified against source.

| Tool | Behaviour | Lesson |
|---|---|---|
| KiCad | A project is a folder of text files: one board file, one file per schematic sheet, a project settings file, and a separate per-user state file. Footprint libraries are folders with one file per footprint. Boards and schematics carry copies of the library items they use. Zone fills are saved in the board file, which users widely report as noise in diffs. 3D models are usually referenced by path. Newer versions open older files, not the reverse. | Folder of text works. Keep per-user state in its own unversioned file. Keep fills out. Embed what the design needs. |
| Altium Designer | Binary documents per sheet and board with a text project file. Comparison and history come from its own compare tools and its managed server. | With binary files, diff must be a product feature. Even with text, a semantic compare is what people actually read. |
| Autodesk Fusion electronics (and EAGLE before it) | EAGLE used XML files with libraries embedded in the design. Fusion keeps designs as cloud documents with its own versions. | XML was diffable but one large file. Cloud versioning is outside this product's offline rule. |
| LibrePCB-style | A project is a directory of many small text files, one directory per element named by a unique id, with a format version file, and a zipped archive of the project for sharing. | Closest to the recommendation: ids as paths, a version marker in the tree, a ZIP for sharing. |

### 3.4 Ids

| Option | For | Against |
|---|---|---|
| **Small integers from a saved counter (today; recommended)** | Short in diffs and in LLM tool calls. Existing replays stay valid. Two branches that both add objects conflict on the counter line, which is the correct signal. | Merging two branches needs a tool that renumbers one side. |
| Random 64-bit ids | Independent additions merge without a tool. | Long ids in every line and every MCP call. Objects lose creation order, so related lines scatter. Replays need a seeded source. |
| Session prefix plus counter in one `u64` | Merges without a tool and keeps locality. | Every printed id grows. More concepts. A rare prefix collision still needs the duplicate check. |

Library lineages in folders already use random 128-bit ids, which is right there: people add parts to a shared library independently, and each addition is a new file.

## 4. Recommended design

### 4.1 Encoding rules (all artefacts)

1. UTF-8, LF, a final newline, Unicode NFC. File and folder names are lower-case ASCII `[a-z0-9._-]`, at most 64 characters, never a user's label.
2. **Canonical value encoding**: the generator ABI's codec (read: generators/README.md): keys sorted by UTF-8 bytes, each once, no whitespace, integers only, absent optionals omitted, never `null`. The core already depends on `sparksmith-generator-abi`; use that one codec, never a second. The extension-ABI plan moves it into a shared ABI crate; that is fine once a part's identity is the generator id and version (part-generation plan), and before then only if the rebuilt shipped module's bytes are unchanged, because today a part key is the module hash.
3. **Layout A, collections (`.jsonl`)** (key order and sort order are refined in 8.6): line 1 is a header `{"format":"sparksmith/tracks","version":2}`; every following line is one object in canonical encoding, sorted by id. A line is bounded (1 MiB proposed).
4. **Layout B, documents (`.json`)**: a JSON object written by one layout function: top-level members one per line, and each array the schema marks as a table one element per line, elements in canonical encoding. Reading is plain JSON parsing. The layout is a pure function of the value and the format version.
5. **Hashes** are SHA-256 over the canonical encoding of the value, never over the laid-out bytes. In a `.jsonl` file the two are the same line.
6. **Numbers**: lengths are integer nanometres, angles integer millidegrees. Scalar keys carry the unit (`width_nm`, `rotation_mdeg`). Points are `[x, y]` integer pairs. Any other non-integer is a scaled integer with its scale in the key (`_ppm`) or an exact decimal string for a value a person typed (`"si": "5100"`, as today). Until U2 and U3 finish, allowlisted fields still write floats and the "integers only" check is off for their files; when the `f64-lengths` allowlist is empty the encoder refuses floats.
7. **Defaults** are omitted, and a default is part of the format version: changing one is a version change.
8. **Nothing nested as a string.** The board sketch and waiver keys become real JSON.
9. **Refuse, never guess** stays: unknown fields, entries, versions and features refuse the file. In a folder a closed list is ignored and nothing else: `.git*`, `.DS_Store`, the save journal and the lock.

### 4.2 The project tree

```text
esp32-s31-devboard.sparksmith/        a folder, or a ZIP holding exactly this tree
  sparksmith-project.json             format, version, project id, name, features, id counters
  properties.json                     title block
  circuit/
    instances.jsonl                   component instances (shared by schematic and board)
    nets.jsonl                        net id, name, terminals
  schematic/
    sheets.json                       sheets in page order, paper
    sheet-1.jsonl                     everything on sheet 1: occurrences, wires, junctions, labels, sheet symbols, drawings
    sheet-2.jsonl
  board/
    board.json                        outline sketch, stackup, fabrication choices
    placements.jsonl
    tracks.jsonl
    vias.jsonl
    fills.jsonl                       fill definitions only, no computed copper
    keepouts.jsonl
    dimensions.jsonl                  part dimensions and their model traces
    interface.jsonl                   published datums and the append-only slot table
    link.json                         the link to a linked design and its snapshot
  rules/
    rules.json                        overrides and net classes
    profile.json                      the fab profile snapshot
    waivers.jsonl
  library/                            the project snapshot, same layout as a library folder (4.4)
    components/59/r0001.json
    footprints/63/r0001.json
    ...
    generated.jsonl                   one record per generated part
    labels.json
  drafts/
    drafts.json                       grids
    <kind>/<id>.json                  unfinished definitions, exactly as written
  assets/sha256/<hash>                models, artwork, generator modules
```

`sparksmith-project.json`:

```json
{
"features":[],
"format":"sparksmith/project",
"name":"ESP32-S31 dev board",
"next":{"id":654,"net":175,"sheet":3},
"project":"0b5e3c1a9f6d4e2f8a7b6c5d4e3f2a1b",
"version":2
}
```

`board/tracks.jsonl` (the first is the example's track 394 in nanometres):

```json
{"format":"sparksmith/tracks","version":2}
{"id":394,"layer":"top","net":6,"path":[[19750000,7355000],[19750000,8600000]],"width_nm":300000}
{"id":395,"layer":"top","net":6,"path":[[19750000,8600000],[21000000,9850000]],"width_nm":300000}
```

`schematic/sheet-1.jsonl` (records sorted by kind, then id):

```json
{"format":"sparksmith/sheet","sheet":1,"version":2}
{"at":[5000000,-45000000],"id":120,"kind":"label","name":"VBUS","style":{"style":"arrow","type":"power"}}
{"at":[20000000,-50000000],"id":81,"instance":80,"kind":"occurrence","unit":1}
{"from":{"label":120},"id":121,"kind":"wire","to":{"instance":80,"terminal":3}}
```

Notes:

- **A sheet's file is named by its id**, so renaming or reordering a sheet edits one line of `sheets.json` and moves no file.
- **Order.** Collections are sorted by id on write. Where order is meaning (sheets, stackup layers, outline segments) it is an array inside one record. Slice F2 audits every `Shared<Vec<T>>` for which case it is. Keeping collections sorted by id in memory also gives the binary-search lookup the review asked for.
- **Counters** stay in the manifest. Two branches that both add objects conflict on the `next` line by design (R6).
- **Every file carries the tree's version.** A tree whose files disagree was merged from a branch saved by another version, and is refused with that explanation.
- **Tracks** get a `path` so arcs fit without another break; the arc representation itself belongs to U3 (proposed: end point and mid point, integers, no bulge float).
- **Bounds** move from one 16 MiB document to per-file and per-tree limits: 4 MiB per document file (the library folder's limit today), 64 MiB per `.jsonl` file, the existing asset and container limits. All targets.

### 4.3 Derived and local data

| Data | Today | Proposed |
|---|---|---|
| Computed fill copper | In the document, 70 % of the file | Not saved. Kept in memory and in the user cache folder under the fill's existing inputs key. A cold open refills off the UI thread. Fabrication export always computes from the inputs. |
| Generated-part check results | Re-run on every open | Cache keyed by module, params, result and stored-revision hash, in the user cache folder. |
| Library search index | User cache folder | Unchanged. |
| View state (zoom, visible layers, selection filter, open sheet) | Partly settings | User state folder keyed by project id. Never in the tree. |
| Net terminals, model traces, the fab profile snapshot | Document | Stay: they are stored state (see section 1). |

A ZIP carries no caches in this version. If a measured cold open is too slow, add an optional `cache/` entry that is display-only and checked against its key.

### 4.4 Library: revisions, folders, bundles, part files

One layout for a user or team library folder, the project snapshot and the shipped parts shelf:

```text
<library>/
  sparksmith-library.json             format, version, library id, name, features, default licence
  components/<lineage>/r0001.json     one immutable file per revision, never rewritten
  symbols/<lineage>/r0001.json
  packages/ footprints/ bindings/ parts/ models/ fields/ quantity-kinds/ package-rows/
  catalog.json                        shelves and tags by lineage; mutable, not revisioned
  assets/sha256/<hash>
```

- The lineage's kind becomes a folder (a lineage has exactly one kind, so the path is stable). A pull request then reads "footprints/…/r0003.json added".
- A lineage is a 128-bit id (32 hex characters) in a library folder and the project's small integer in the project snapshot. Import still remaps and records the origin (decision D1, unchanged).
- A revision file is layout B with tables per kind: one terminal, pin, pad, graphic or mapping per line. Immutable files are never diffed in place; the layout serves `git diff --no-index`, review of new files and size. Compact encoding takes the 1.4–1.7 MB symbol at the limits to roughly a third (target, from the measured 3:1 ratio), and one file per revision removes the "ten such revisions fill a project" ceiling entirely.
- **Content hash** becomes the SHA-256 of the canonical encoding (rule 5), after U2 makes library geometry integer. It no longer depends on Rust field order.
- **Bundle**: the ZIP of a library subtree holding the closure of one root, same rules as a project ZIP. The JSON bundle `library.import` takes over MCP stays as the wire form.
- **Pruning**: `library.prune` (already proposed in the component model) removes revisions nothing references. Explicit, previewed, one undo step. Git keeps the history.
- **Part files** stay `.solidcraft`, written only by Solidcraft's writer, stored as assets by hash. The `.part.json` sidecar becomes a canonical document with integer units (`add_mm: 0.6` becomes `add_nm: 600000`).
- **Generated parts**: `library/generated.jsonl`, one line per part: root, the revisions it made, the part key (generator id and params hash, with the scope inside the hash), the generator version (which distinguishes revisions of that lineage), the bundle hash as provenance (manifest, module and part files), ABI number, complete params, result hash and the generation-key to id map. A parameter change is a one-line diff. The expanded revisions stay stored, so a file opens and can be read by other tools without running a module.

### 4.5 Binaries and git

- Assets are content-addressed, immutable and written once, so git stores each once and they never churn.
- **Plain git by default.** Typical assets are small (the example's largest is 156 KB; the chip module is 281 KB).
- The app writes a `.gitattributes` into a new folder project: `* -text` (no line-ending conversion) and `assets/sha256/* binary`, with the Git LFS line present but commented out.
- When an asset file is a Git LFS pointer instead of the bytes, the refusal says so and names `git lfs pull`, rather than reporting a hash mismatch.
- Shipped generator modules stay committed builds (decision G8). The part-generation plan's generator bundle folder (`library/generators/<name>/` with `generator.json`, `<name>.wasm` and `parts/*.solidcraft`) gives each binary a text description, so a change to a module is readable in a diff; a project keeps the module and each part file once by hash.

### 4.6 Saving, recovery and concurrency

- **Core**: `format::tree::encode(&Document) -> FileSet` and `decode(FileSet) -> Document`, pure, bounded, wasm-compatible. A `FileSet` is a sorted map from path to bytes, asset bytes shared as today. Because the project keeps each collection behind a shared pointer, the encoder skips collections that are the same allocation as at the last save.
- **Engine unchanged.** Commands still mutate the in-memory document through the one engine. Open and save are the only places that see files, and UI, CLI and MCP reach them through the same `read_project` and `write_project`, which accept a folder or a ZIP.
- **ZIP store**: today's container code, holding the tree instead of one document. One atomic rename.
- **Folder store, journaled**: write each changed file to a sibling temporary and sync it; write a journal naming every replacement and deletion with hashes, atomically (this is the commit point); rename the files into place; delete the journal. Opening a folder with a journal finishes the save first. Only changed files are written.
- **External changes**: the store remembers each file's hash at load. If a file changed on disk since (a `git pull`, a hand edit), saving stops and offers reload or keep mine through a preview; nothing is overwritten silently. A lock file stops two sessions saving one folder.
- **Recovery** stays a private single file per session: the ZIP form plus `recovery.json`. Asset bytes go once into a shared store in the recovery folder instead of into every snapshot. Recovery snapshots go through the converter chain like projects, so a format change never strands unsaved work.

### 4.7 Versions and migrations

Proposed replacement for the "one version, no migrations" rule in AGENTS.md:

1. **Every incompatible change raises `version`**, starting at 2 now (1 is ambiguous).
2. **Optional capabilities are named features.** A file lists a feature only when it uses it; a build that does not know it refuses and names it. A design that does not use a new feature stays readable by older builds.
3. **A converter chain** of pure steps `vN -> vN+1` over a `FileSet`. Before the first release it runs as `sparksmith convert` (the present `convert-v1` becomes step 1) and the app reads only the current version, naming the command when it refuses. From the first release the same chain runs in the app: the file opens converted, the conversion is announced, and nothing is written until the user saves; the original is kept beside it.
4. **Golden trees per version** are kept in the repository forever. Each step is tested: old golden converts, opens, and re-saves byte for byte.
5. **`cargo xtask format-lock`**: a committed fingerprint of every stored type's schema and layout tables. CI fails when the fingerprint changes without a version or feature change.

### 4.8 The other artefacts

| Artefact | Recommendation |
|---|---|
| Shipped library (`library/`) | Stays one JSON file per item. Apply the encoding rules (integer units, canonical layout), require the file stem to equal the item's id, and gate with `sparksmith fmt --check` and a schema check per shelf. Prompts stay Markdown. The parts shelf becomes an ordinary read-only library folder when GP5 retires the templates, so shipped, user and team libraries share one reader. |
| Settings | Split per-user files: `settings.json` for preferences (library sources, keymap, list filters, density), which a person may sync or version, and `state.json` for machine state (recent files, panel sizes). Versioned and converted in the app. A file that cannot be read is renamed aside and named, never dropped. |
| Interface sidecar | Keep `<name>.interface.json` and its own major-version rule (other tools read it, so additions within a major stay allowed). Move to nanometres and one datum per line, sorted by slot. |
| Fabrication output | Derived, outside the project tree, deterministic as today. Add `fab-manifest.json`: each file's SHA-256, the project's tree digest, format version and profile id. `sparksmith fab --check` can then prove committed outputs match the design. |
| Tree digest | `sparksmith hash`: SHA-256 over the sorted list of (path, file hash). Computed, never stored in the tree, because a stored list of hashes would conflict on every merge. |
| Signatures | None in the tree; git signs commits and tags. For distributed libraries and third-party generator modules, a detached signature over the tree digest. Define the digest now, add signing when an online library exists. |

### 4.9 Tools the format enables

- `sparksmith fmt`, `check`, `pack`, `unpack`, `convert`, `hash`.
- `sparksmith diff A B`: a semantic summary ("R3 moved 2.5 mm; 4 tracks added on GND; footprint 63 r1 to r2"), usable as a git diff driver. Text diffs of coordinates are exact but hard to read; this is what reviewers will use.
- Later: `sparksmith merge` as a git merge driver, loading base, ours and theirs through the engine, renumbering one side's new ids and running validation.

## 5. Slices

Estimates are agent hours. F1 is the contract; F2 to F4 are serial in the core; the rest run in parallel, at most five at a time.

| # | Slice | Acceptance | Hours |
|---|---|---|---|
| F0 | Version discipline: version 2, refusals name the converter, `format-lock` gate, `convert` chain with `convert-v1` as step 1, AGENTS.md rule updated | A stored struct changed without a version change fails CI. A version 1 file is refused naming the command. Examples and fixtures converted. | 4–6 |
| F1 | Contract: `docs/file-format.md`, layout A and B writers on the existing canonical codec, `FileSet` type, table annotations | Property tests: decode of encode is identity; the layout re-parses to the same value; same bytes on macOS and Linux; compiles for wasm. Generator module bytes unchanged. | 6–9 |
| F2 | Derived data out and order: computed fills to a cache, collections sorted by id, order audit, sketch and waiver keys as real JSON | Open then save is byte-identical. A refill with unchanged inputs changes no byte. The ESP32 Gerbers are byte-identical before and after. The example document drops from 5.1 MB to under 1 MB. | 8–12 |
| F3 | Tree encode and decode in the core, ZIP store on the tree, per-file bounds replace the 16 MiB cap | Round-trip property test. Unknown entry refused by name. Mixed-version tree refused with the merge explanation. Open and save measured on the example and on a synthetic 10,000-via, 50,000-track board. | 10–14 |
| F4 | Folder store: journaled save, changed-files-only, lock, external-change detection, `.gitattributes`, LFS pointer message | A fault injected after every file operation reopens as exactly the old or the new project. An unchanged save writes no file. Moving one track changes one line in one file. A checkout under an open project is detected. | 10–14 |
| F5 | App and CLI: open a folder, Save As folder or single file, `pack`, `unpack`, `fmt`, `check`, recent projects, examples as folders | Checked in the VM: create, save as folder, commit, edit, diff, share as one file, reopen. Programmatic calls open no dialog. | 6–9 |
| F6 | Recovery on the tree: shared asset store, folder projects, conversion of snapshots | Kill during editing restores the work for a folder and a ZIP project. A snapshot of the previous version restores. | 4–6 |
| F7 | Library layout: kind folders, canonical content hash, project snapshot in the same layout, `library.prune` | Import and export round-trip. Hash independent of field order (test reorders a struct's fields). Two people adding parts merge without conflict. Needs U2 for the hash. | 8–12 |
| F8 | Shipped data: integer units, stem equals id, format and schema gate, generator bundle folders | Every file under `library/` passes `fmt --check` and its schema. Coordinate with the data agents now working in `library/`. | 5–8 |
| F9 | Settings split, sidecar in nanometres, fab manifest, tree digest | Old settings convert. `fab --check` passes on the example and fails after an edit. | 5–8 |
| F10 | `sparksmith diff` and the git diff driver | The summary of a known edit set on the example matches a golden text. | 8–12 |
| F11 | Later: `sparksmith merge` driver | Two branches that each add parts and tracks merge to a board that validates; a real conflict is reported by object. | 12–18 |
| F12 | At first release: the chain moves into the app | A previous-version project opens converted and saves only on request. | 4–6 |

### Interaction with the units migration

- F0 comes first. U2 and U3 then each raise the version and add a chain step, instead of extending an idempotent converter with no version.
- F1 to F6 do not depend on units: files whose fields are still allowlisted write floats.
- F7's canonical hash needs U2 (library geometry in integers).
- U3 changes every board record. Do U3, arcs and the `path` form of a track in one version step, so board files change shape once.
- When the allowlist is empty, turn on "integers only" in the encoder and delete the gate's allowlist.

### Interaction with generator plugins

- The file format and the ABI share one canonical codec, in the shared ABI crate the extension-ABI plan creates. The command wire (JSON, millimetre numbers or unit text) stays a separate encoding and is not what files hold.
- Each generated-part record stores what the part-generation plan defines: the part key (generator id and params hash, with the scope inside the hash), the generator version (which distinguishes revisions of that lineage), the bundle hash as provenance (manifest, module and part files, not the module alone), ABI number, complete params, result hash and the generation-key to id map. F7 fixes the canonical revision hash only after that plan's B1 (keyed ids) and B3 (integer `PartValue`). Approvals, grants and the result cache are per-user state, never in the tree. Automation plugins are never embedded, so the tree needs no entry kind for them.
- The part-generation plan removes `PackageForm`, `PackageModel::Generated` and `::Envelope`, the typed `PackageRow` payload and `PackageModel::Script`. The format treats definition bodies as opaque until that lands, and each of its ABI and migration slices raises the file version, so F0 goes before them.
- This plan changes no Operation or Query wire name, keeps the revision counter, and keeps one document per project: folder or ZIP is decided below `read_project` and `write_project`. The one new behaviour a caller can see is a save refused because files changed on disk.
- GP2 (3D): the generator's model output should be parameters for a part file stored as an asset by hash, not a second geometry description. The tree needs nothing new for it.
- Keyed local ids (the review's point) change how ids are assigned, not the layout. Settle them before F7 fixes the canonical hash, because both change every generated part's stored bytes.
- The check cache (4.3) and "open as unverified" (decision 5 below) remove wasm execution from the open path.
- GP5: when templates go, the shipped parts shelf becomes a library folder (F8).
- Open question 3 of the GP1 note (carry the generated record when promoting to a folder) becomes simple: a library folder may hold `generated.jsonl` and the module asset like a project does.

## 6. Decisions for the founder

Answered on 14 November; the answers, and what they change, are in section 8.

| # | Decision | Recommendation |
|---|---|---|
| 1 | Which form a new project is saved in | Folder by default; "Share as single file" makes the ZIP. On macOS the folder can be registered as a package so it looks and opens like one document. |
| 2 | One extension for both forms, or two | One: `.sparksmith` is a folder or a ZIP of that folder, told apart on open. |
| 3 | Ids: small integers with a merge tool, or random ids that merge by themselves | Keep small integers. Diffing is the stated need; concurrent edits of one board are rare and need engine-level validation in any case. |
| 4 | Replace "one version, no migrations" with 4.7 | Yes. It costs a few hours and makes every file and bug report identifiable. |
| 5 | A generated part that fails its check: refuse the project, or open it marked unverified and block publishing and fabrication | Open it marked. The state is derived, not saved. (The review raised this; it is outside the file format but decides whether opening runs modules.) |
| 6 | Binaries in git | Plain git by default, Git LFS opt-in by one line, generator modules stay committed. |
| 7 | Caches inside a shared ZIP | No. Refill on open; revisit if a measured cold open is too slow. |
| 8 | Old revisions nothing references | Keep until an explicit, previewed `library.prune`; offer it when sharing a single file. |
| 9 | Drafts in the versioned tree | Yes, under `drafts/`. They are unfinished work. |
| 10 | JSON family or s-expressions | JSON Lines and JSON. |
| 11 | Signatures | Define the tree digest now; add signing with the online library. |
| 12 | Shipped parts shelf as an ordinary library folder | Yes, with GP5. |
| 13 | Split tracks per layer | No, until a measurement asks for it. |

## 7. Not verified

- Which collections have meaningful order (F2's audit).
- That fill computation is deterministic across platforms; today's computed copper holds 32-bit float values. Fabrication reproducibility depends on it and it belongs to U3.
- All sizes and times for large boards and libraries are targets. Only the ESP32 example was measured, and only for size.
- The behaviour of other EDA tools is from public knowledge, not checked against their sources or current releases.
- macOS package registration for a folder project, and how the folder form behaves in the Windows file dialog, need a check in the VM.

## 8. Founder answers and the measured encoding evaluation (14 November, later)

This section records the founder's answers and supersedes sections 4 to 6 where it says so.

### 8.1 Answers

| # | Answer | Effect on this plan |
|---|---|---|
| 1 | Folder by default, "Share as single file" | As planned. The macOS package part conflicts with answer 2; see 8.2. |
| 2 | First: folders carry no extension. Then, once the package rule was known: folders carry the extension | `<name>.sparksmith`, a macOS package; see 8.2. Section 4.2 stands. |
| 3 | Small integer ids | As planned. |
| 4 | Version every break | Decided. Slice F0 goes first. |
| 5 | A generated part that fails its check refuses the project | Decided; my recommendation to open it marked is withdrawn. The check cache (4.3) still applies: a part whose module, params, result and stored hashes match a recorded pass is not run again. |
| 6, 8, 9, 11, 12 | As recommended | No change. |
| 7 | No caches in a shared file; missing caches are rebuilt by a background task | 4.3, with the rebuild as a job that never blocks opening. |
| 13 | One tracks file, organised for scoped reading | See 8.6. |
| 10 | Open | See 8.3 to 8.7. |

### 8.2 A folder without an extension, and the macOS package

**Resolved by the founder (14 November): the folder carries the extension.** A project folder is `<name>.sparksmith`, registered as a macOS package (option B below), as section 4.2 first had it. What follows from that:

- The manifest inside keeps the name `sparksmith-project.json` (4.2); the `project.sparksmith` marker of option A is not used.
- The shared single file is also `<name>.sparksmith`, the ZIP of the folder. The reader tells a folder from a ZIP when opening.
- On Windows and Linux the project is an ordinary folder with a dot in its name, opened through Open Folder, Open Recent, a drop on the window, or the command line.
- Library folders: the founder allows the same if packaging makes sense. Proposed: leave them plain folders. People browse and review a library's files in git, and a package hides them in Finder; a library is added in settings, never double-clicked.
- The package registration still needs its check in the VM (in F5).

The analysis below is kept as the record of why.

Not tested; this is from how the platforms are documented to behave, and needs a check in the VM.

- macOS shows a folder as one document (a package) in two cases. One: its **extension** belongs to a type an app declares as a package. Two: the folder has the Finder **bundle flag** set.
- A type is attached to a file by its extension (or a MIME type). There is no rule of the form "a folder that contains this file".
- The bundle flag works without an extension, but it is an extended attribute. Git does not store it, and a plain ZIP, a Windows machine or some sync services drop it. After a clone the project would be an ordinary folder again, and the flag alone does not say which app opens it.
- Windows Explorer and Linux file managers have no package concept. A folder is always a folder there.

So a package and "no extension" cannot both hold reliably. Options:

| Option | Double-click | In git and on Windows or Linux | Cost |
|---|---|---|---|
| **A. Plain folder, recognised by a marker file that carries the extension (recommended)** | Double-click `project.sparksmith` inside the folder, on every platform | A normal folder, browsable everywhere | The folder does not look like one document in Finder |
| B. Folder named `Name.sparksmith` | The folder itself, on macOS only | A folder with a dot in its name | Breaks answer 2. Finder hides the contents, which git users want to see |
| C. Plain folder with the bundle flag | The folder, on the machine that set the flag | Lost on clone, copy and sync | Not dependable |

Recommended (A):

- A project folder is named after the project and nothing else. It is a project because it holds the manifest, named **`project.sparksmith`** (the small JSON text of 4.2; this replaces the name `sparksmith-project.json`).
- The shared single file is **`Name.sparksmith`**, the ZIP. One registered extension and one icon. The reader tells the two apart by the first bytes, as it already checks the ZIP signature. A manifest opened without its folder's files says so and names "Share as single file".
- Ways in: double-click the manifest; File › Open accepts a folder or a manifest; drop a folder on the window or the Dock icon; Open Recent; `sparksmith <folder>`.
- A library folder stays a plain folder with `sparksmith-library.json`; libraries are added in settings, not double-clicked.

If the single-document look in Finder matters more than answer 2, option B is the only dependable way to get it.

### 8.3 What was measured, and how

- **Data**: the ESP32 example, converted to the proposed record shapes: its 201 tracks, 55 vias, 20 placements, the 217 items of sheet 2, and its largest symbol, footprint and component revisions.
- **Bytes and tokens**: counted on those slices. Tokens are from `tiktoken` (`o200k_base`), run offline. That is OpenAI's tokenizer, so it is an approximation for Claude; no Anthropic tokenizer was available offline. Read the ratios, not the absolute counts.
- **Speed**: a throwaway Rust program in the scratchpad (release build, rustc 1.96.0, Apple M5 Max, one thread, best of 7 runs) on 50,000 tracks made by repeating the example's 201 with shifted positions and new ids. Crates: serde_json 1.0, ron 0.12.3, csv 1.4.0, toml 1.2.1, rmp-serde 1.3.1, postcard 1.1.3. Every format was read back and compared equal.
- Not measured: YAML and s-expression speed (bytes and tokens only), and real model behaviour on each encoding. Readability below is my judgement from the excerpts.
- The scripts and outputs are in `/private/tmp/claude-501/-Users-amd-dev-sparksmith/baf6d421-89fd-41f2-b550-ad7602bf2ca6/scratchpad/enc/`.

### 8.4 Results

Tracks. Tokens and bytes per record are from the 201 real tracks; file size and times are for 50,000.

| Encoding | Tokens per track | Against keyed JSON Lines | 50,000 tracks | Write | Parse |
|---|---|---|---|---|---|
| Pretty JSON (today's style) | 73.0 | 1.87 | 10.5 MB | 7.3 ms | 8.2 ms |
| TOML, mm | 46.6 | 1.19 | 5.0 MB | 14.1 ms | 64.7 ms |
| YAML flow, one per line, mm | 42.6 | 1.09 | not built | not measured | not measured |
| S-expression, one per line, mm | 40.6 | 1.04 | not built | not measured | not measured |
| **JSON Lines, keyed, nm** | 39.1 | 1.00 | 5.2 MB | 3.5 ms | 7.2 ms |
| JSON Lines, keyed, mm | 38.6 | 0.99 | 4.3 MB | 7.3 ms | 8.4 ms |
| RON, one per line, nm | 36.1 | 0.92 | 4.6 MB | 14.0 ms | 40.7 ms |
| RON, one per line, mm | 35.6 | 0.91 | 3.7 MB | 20.0 ms | 49.8 ms |
| RON pretty, mm | 56.6 | 1.45 | 8.7 MB | 20.3 ms | 55.6 ms |
| JSON rows, columns named once, nm | 28.1 | 0.72 | 3.4 MB | 2.1 ms | 5.0 ms |
| JSON rows, columns named once, mm | 28.7 | 0.74 | 2.6 MB | 6.0 ms | 5.8 ms |
| Tab-separated with a header row, nm | 24.5 | 0.63 | 2.9 MB | 3.5 ms | 4.7 ms |
| Tab-separated with a header row, mm | 25.1 | 0.64 | 2.1 MB | 10.6 ms | 5.2 ms |
| MessagePack with keys (binary, reference) | none | none | 3.3 MB | 2.9 ms | 3.6 ms |
| Postcard (binary, reference) | none | none | 1.3 MB | 1.0 ms | 1.3 ms |

Tokens against keyed JSON Lines in nanometres, other slices:

| Slice | Pretty JSON | RON compact, mm | Rows or column tables, mm | Tab-separated, mm |
|---|---|---|---|---|
| Vias (55) | 1.67 | 0.88 | 0.60 | 0.59 |
| Placements (20) | 1.77 | 0.77 | 0.53 | 0.44 |
| Sheet 2 (217 items, three kinds) | 1.98 | 0.86 | 0.60 | not sensible: three shapes, nested ends |
| Symbol revision (6,400 tokens keyed) | 1.81 | 0.84 | 0.74 | not sensible |
| Footprint revision (7,444 tokens keyed) | 1.89 | 0.98 | 0.72 | not sensible |
| Component revision (24,237 tokens keyed) | 1.77 | 0.96 | 0.82 | not sensible |

Laid-out JSON (layout B, one table row per line) costs 1 to 2 % more tokens than the same document on one line.

What the numbers say:

1. **Today's pretty JSON costs 1.7 to 2.0 times the tokens** of one record per line, and twice the bytes. Leaving it is the largest single gain and every candidate gets it.
2. **Naming the columns once is the only large further saving**: 18 to 47 % fewer tokens than keyed lines. Dropping key quotes (RON, YAML, s-expressions) saves 2 to 23 %, about 9 % on tracks, or costs tokens.
3. **Millimetres against nanometres barely changes tokens** (within 2 % on tracks and vias; digits tokenise at about three per token) though it saves 19 to 38 % of bytes.
4. **Speed does not separate the JSON and tab-separated forms**: 50,000 tracks parse in 5 to 8 ms. RON takes 41 to 50 ms and TOML 65 ms, six to nine times slower. All are small against a project open, so speed argues against RON and TOML only mildly.
5. **Repository size does not separate them either**: after zlib (what git stores) the 201 tracks are 2.0 to 2.6 KB in every one-per-line form, and 2.9 KB as pretty JSON.

Excerpts (the first tracks):

```text
JSON Lines, keyed, nm
{"id":394,"net":6,"layer":"top","width_nm":300000,"path":[[19750000,7355000],[19750000,8600000]]}
{"id":395,"net":6,"layer":"top","width_nm":300000,"path":[[19750000,8600000],[20750000,8600000]]}
{"id":396,"net":6,"layer":"top","width_nm":300000,"path":[[20750000,7355000],[20750000,12000000]]}

JSON Lines, keyed, mm
{"id":394,"net":6,"layer":"top","width":0.3,"path":[[19.75,7.355],[19.75,8.6]]}
{"id":395,"net":6,"layer":"top","width":0.3,"path":[[19.75,8.6],[20.75,8.6]]}
{"id":396,"net":6,"layer":"top","width":0.3,"path":[[20.75,7.355],[20.75,12]]}

RON, one per line, mm
(id:394,net:6,layer:top,width:0.3,path:[(19.75,7.355),(19.75,8.6)])
(id:395,net:6,layer:top,width:0.3,path:[(19.75,8.6),(20.75,8.6)])
(id:396,net:6,layer:top,width:0.3,path:[(20.75,7.355),(20.75,12)])

JSON rows, columns named once, mm
{"columns":["id","net","layer","width","path"]}
[394,6,"top",0.3,[[19.75,7.355],[19.75,8.6]]]
[395,6,"top",0.3,[[19.75,8.6],[20.75,8.6]]]

Tab-separated with a header row, mm
id	net	layer	width	x1	y1	x2	y2
394	6	top	0.3	19.75	7.355	19.75	8.6
395	6	top	0.3	19.75	8.6	20.75	8.6

YAML flow, mm
{id: 394, layer: top, net: 6, path: [[19.75, 7.355], [19.75, 8.6]], width: 0.3}

S-expression, mm
((id 394) (layer top) (net 6) (path ((19.75 7.355) (19.75 8.6))) (width 0.3))

TOML, mm (five lines a track)
[[tracks]]
id = 394
net = 6
```

Judgement on each:

| Encoding | Diff and merge | Readable to a model | Verdict |
|---|---|---|---|
| Keyed JSON Lines | One changed object is one changed line. A line means the same alone, so a diff hunk or a search hit needs no header. | High: every value is named. Plain `grep '"net":6,'` selects by net; `grep '^{"id":394,'` finds an object. Any sandbox parses it. | **On disk** |
| RON, one per line | Same as keyed lines. | High, slightly cleaner. Not parsable by `jq`, Python or the scripts an agent writes; a second codec beside the canonical one. | Rejected: 9 % fewer tokens does not pay for it |
| JSON rows / tab-separated | Same line behaviour, but a changed line does not say which column changed, and a new column rewrites every line once. | Good with the header in view; poor without it: `394` and `6` are indistinguishable in a diff hunk or a search hit. Tab-separated cannot nest (paths with arcs, wire ends). | **In query replies**, where the header always travels with the rows |
| Pretty JSON, RON pretty, TOML | Many lines per object; merges split objects. | Readable but about twice the tokens. | Rejected |
| YAML flow, s-expressions | Same as keyed lines. | Fine, but no token gain over JSON here, and a parser of our own or a deprecated crate. | Rejected |
| Binary | No diff. | None. | Not for versioned data |

### 8.5 The query side: how an agent reads only what it needs

The principle, and the extension-ABI design's (its section 2.5): **an agent reads the design through `query`, not through files.** The document is in memory, and a query answers from it by id, net, layer, sheet or region, in the command wire's millimetres. Measured on 50,000 tracks: filtering the loaded document by net takes 25 µs; re-reading the file for the same answer takes 5.7 ms even when only `net` is parsed.

What that implies:

- **The reply format and the file format are separate choices.** Token cost is paid in replies, so replies take the cheapest form that is still unambiguous; files take the form that is safest to diff, search and repair.
- **Replies are tables.** A query that returns records answers `{"revision":…, "columns":[…], "rows":[[…],…], "more":…}`, lengths in mm. That is the 18 to 47 % saving measured above, and the header is always in the same reply. Heterogeneous results (a sheet's labels, occurrences and wires) are one table per kind.
- **One declared field order per record type** gives the key order of a line on disk, the column order of a reply and the order in generated documentation. It is the Rust struct's field order, which serde already writes.
- **Every list query takes the same scope and budget parameters** (proposed; the names belong to the extension-ABI catalog):

| Parameter | Meaning |
|---|---|
| `ids` | Exact objects |
| `net`, `layer`, `sheet`, `kind` | Equality filters |
| `bbox` | A window in mm; objects that touch it |
| `fields` | Only these columns |
| `limit`, `after` | A page; replies say whether more remain |
| `count: true` | Only how many match, so an agent can size a read before making it |

- Proposed entries, as catalog queries, not new verbs: `layout.objects` (tracks, vias, pads, fills, keepouts in a scope), `schematic.items` (one sheet or one net), `library.definition` with a `part` (pins, pads, graphics, mappings) and ids, so a 24,000-token component is never read whole to see one pin. The window layer of docs/llm-geometry.md is `layout.objects` with a `bbox`; which spatial encoding wins there is that benchmark's question, not this plan's.
- **In memory**: an id index per collection (sorted by id, binary search) and a spatial grid for `bbox`, built on load and kept by the engine. Nothing of this is saved: an index file would be derived data that changes on every save.
- **Without a running session** (a coding agent in the repository): `sparksmith query <project> <name> '<params>'` loads the tree and answers once. The tree parses fast enough for that: 7 ms for 50,000 tracks (measured); the whole open is a target until F3 measures it.
- **Raw files are the fallback**, for git, review and repair. They stay usable for a model because each line is self-describing and searchable.

### 8.6 Recommendation for decisions 10 and 13

**Decision 10: JSON Lines with keyed objects on disk; column tables in query replies. Not RON.**

- Against today's file, keyed lines halve the tokens (1.87 to 1.00 on tracks) and the bytes, and with the fills gone the example's document shrinks from 5.1 MB to under 1 MB.
- RON would save a further 9 % of tokens on tracks (2 to 23 % across slices) and parses six times slower (41 ms against 7 ms for 50,000 tracks). It needs a second codec beside the canonical one that hashes and the generator ABI use, and nothing outside Rust reads it, including the short scripts agents write.
- Positional rows save 18 to 47 %, but that saving is taken in query replies, where agents actually read. On disk a positional line is not self-describing in a diff or a search hit.
- Lengths stay integer nanometres on disk: millimetres would save bytes but under 2 % of tokens, and integers keep one exact codec. Replies use millimetres, as the command wire already does.
- Library revisions and other single documents stay laid-out JSON (1 to 2 % token cost over one line). Agents read parts of them through `library.definition`, as tables.

Two changes to section 4.1 follow:

1. A line's keys are written in the record's **declared order, identity first** (`id`, then references such as `net` and `layer`, then geometry), not sorted. Hashes are still taken over the canonical sorted encoding (rule 5), so nothing about identity changes. This is what makes `grep '^{"id":394,'` work.
2. The canonical codec is used for hashes and for the generator ABI; file lines are plain `serde_json` in declared order with the same value rules (integers only, no `null`, NFC).

**Decision 13: one `board/tracks.jsonl`, sorted by net and then id.** The same for vias.

- A net's copper is one contiguous block, so a raw read scoped to a net is one search and one short range, and a diff groups changes by net.
- Two branches that route different nets insert lines in different places and merge cleanly. Sorted by id alone, every new track lands at the end of the file and any two branches conflict there.
- A line moves only when its net changes, and that line is in the diff in any case.
- Lookup by id in the file is an anchored search (milliseconds on 5 MB). The app never needs it: it loads the file once (7 ms for 50,000 tracks) and indexes in memory.
- No split by layer and no index file. Region queries go through `layout.objects`.

Other collections keep the orders of 4.2: placements by instance, sheet items by kind then id, everything else by id.

### 8.7 Effect on the slices

| Slice | Change | Hours |
|---|---|---|
| F1 | Adds the declared field order, the per-collection sort keys and the table reply shape to the contract | +1–2 |
| F5 | Register `<name>.sparksmith` folders as a macOS package; open a folder or a ZIP; the VM check of 8.2 | +2–3 |
| Not a format slice | The scope and budget parameters, table replies and the three list queries are slice S2c of the extension-ABI design (6–10 hours there), which lands after F1 because it needs the declared field order fixed | none here |
