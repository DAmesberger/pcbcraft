# The Sparksmith file format

This page is the normative contract for Sparksmith's project and library files. The reasons, the options that were weighed and the measurements behind them are in the [file-format plan](design/file-format-plan.md). The [founder decisions of 14 November 2026](design/decisions-2026-11-14.md) win over both.

## Status

| Part | State |
|---|---|
| Versions, features, converter chain, `format-lock` gate | Built (F0). See [file versions](development.md#file-versions). |
| This contract, both layouts, the tree in memory (`FileSet`), declared field order and sort keys, the table reply type | Built (F1), in `sparksmith_core::format::{files, layout, records, table}`. **Nothing saves through it yet.** |
| Fills out of the document, order audit, sketch and waiver keys as real JSON | F2 |
| Encoding and decoding the whole document as a tree; a ZIP of the tree replaces today's single `project.json` | F3 |
| The folder store: journaled saves, changed files only, lock, external-change detection | F4 |
| Library folders in this layout, canonical revision hashes | F7 |

Until F3 lands, a `.sparksmith` file is still today's ZIP holding one `project.json` and `assets/sha256/<hash>` (`sparksmith_core::format`, `sparksmith_io::persistence::container`). Everything below describes the tree that replaces it.

Words: **must** and **never** are rules a reader enforces by refusing the file. **Target** marks a limit that is not yet measured.

## 1. The project

A project is a tree of files. It is stored in one of two forms with the same extension:

- **A folder** named `<name>.sparksmith`, the working form, for git. macOS registers the extension as a package, so Finder shows the folder as one document. On Windows and Linux it is an ordinary folder with a dot in its name.
- **A ZIP** named `<name>.sparksmith` that holds exactly the same tree: "Share as single file", recovery snapshots, bug reports and the browser. A ZIP never holds caches.

A reader tells the two apart when it opens the path: a folder, or a file starting with the ZIP signature. The tree is the same in both forms, so its [digest](#7-hashes) is too.

### 1.1 The tree

```text
<name>.sparksmith/
  sparksmith-project.json        the manifest (section 5)
  properties.json                title block
  circuit/
    instances.jsonl              component instances, shared by schematic and board
    nets.jsonl                   net id, name, terminals
  schematic/
    sheets.json                  sheets in page order
    sheet-<id>.jsonl             everything drawn on one sheet, named by the sheet's id
  board/
    board.json                   size, outline shape, stackup, fabrication choices
    placements.jsonl
    tracks.jsonl
    vias.jsonl
    fills.jsonl                  fill definitions only, never computed copper
    keepouts.jsonl
    dimensions.jsonl             part dimensions
    model-traces.jsonl           the 3D model faces and edges the dimensions name
    interface.jsonl              published datums and the append-only slot table
    link.json                    the link to a linked design and its snapshot
  rules/
    rules.json                   overrides and net classes
    profile.json                 the fab profile snapshot
    waivers.jsonl
  library/                       the project's library snapshot, laid out as a library folder (section 6)
    generated.jsonl              one record per generated part
  drafts/
    drafts.json                  draft settings (grids)
    <kind>/<id>.json             unfinished definitions, exactly as written
  assets/sha256/<hash>           models, artwork, generator modules
```

- **Names come from ids, never from labels a person can change.** Renaming or reordering a sheet edits one line of `sheets.json` and moves no file.
- **Order that means something** (sheets, stackup layers, outline segments) is an array inside one record or document. Every other collection is sorted by its key (section 4), so the order of creation or editing never shows in a file.
- **Id counters** live in the manifest. Two branches that both add objects conflict on its `next` line, which is the intended signal.
- **No caches, view state or timestamps** are in the tree: computed fill copper, check results, search indexes, zoom, visible layers and the open sheet live in per-user folders keyed by the project id.
- A **folder** may also hold the closed list `.git*`, `.DS_Store`, the save journal and the lock file. A reader ignores those and nothing else. Any other file or folder is refused by name, never dropped.

### 1.2 Paths

Every path in the tree, folders included:

- is lower-case ASCII `[a-z0-9._-]`, names separated by `/`;
- has names of 1 to 64 characters, none starting with a dot;
- is at most 255 characters and 8 levels deep;
- ends in `.jsonl` (layout A), `.json` (layout B), or is `assets/sha256/<64 lower-case hex digits>`, the SHA-256 of the asset's bytes.

`sparksmith_core::format::files::file_kind` checks these rules.

## 2. The two layouts

Both are JSON, written by one encoder from the stored Rust types, so `jq`, Python and the scripts an agent writes read them directly.

### 2.1 Layout A: collections (`.jsonl`)

A collection is a file of many records of one concern.

```json
{"format":"sparksmith/tracks","version":2}
{"id":394,"net":6,"layer":"Top","width":0.3,"start":[19.75,7.355],"end":[19.75,8.6],"locked":false}
{"id":396,"net":6,"layer":"Top","width":0.3,"start":[20.75,7.355],"end":[20.75,8.6],"locked":false}
{"id":12,"net":9,"layer":"Top","width":0.3,"start":[1.0,7.355],"end":[1.0,8.6],"locked":false}
```

- **Line 1 is the header**: `format` (`sparksmith/<collection>`), then `version`, then the collection's own header members in declared order (a sheet file adds `"sheet":<id>`).
- **Every further line is one record**, a JSON object with its keys in the record type's [declared order](#4-field-order-and-sort-keys), the id first. A one-object edit is a one-line diff, and `grep '^{"id":394,'` finds an object.
- **Records are sorted** by the collection's sort key. No two records share a whole sort key, and no two share an id.
- A **mixed collection** (one sheet's file) names each record's type in a `type` field, second after the id, and is sorted by type, then id.
- An empty collection is the header alone.

`write_collection` and `read_collection` in `sparksmith_core::format::layout` implement this layout.

### 2.2 Layout B: documents (`.json`)

A document is one object of one concern.

```json
{
  "format":"sparksmith/sheets",
  "version":2,
  "sheets":[
    {"id":1,"name":"Power"},
    {"id":3,"name":"MCU"}
  ]
}
```

- `{` on the first line, `}` on the last.
- One member per line, indented by two spaces: `format`, `version`, then the document's members in declared order. Members are separated by a comma at the end of the line.
- A member the layout marks as a **table** (an array of rows) puts each element on its own line, indented by four spaces, with `]` on a line of its own at two spaces. An empty table is `[]` on the member's line.
- Everything inside a member or a row is written on one line by the value rules below.

`write_document` and `read_document` implement this layout. Library revisions, the manifest and other single documents use it.

## 3. Value rules

These hold for every value in both layouts.

1. **Text**: UTF-8 without a byte order mark, every string and key in Unicode NFC. Lines end with LF only, the last line included; a CR anywhere is refused (on Windows, check git's `core.autocrlf`; the project's `.gitattributes` sets `* -text`). No empty lines.
2. **Inside a record**: no whitespace; objects nested in a record have their keys sorted by UTF-8 bytes, each key once. Strings are escaped as `serde_json` escapes them.
3. **No `null`.** An absent optional value is left out. A `null` inside an array is refused, since it cannot mean "absent" there.
4. **Defaults are left out** where the type says so, and a default is part of the format version: changing one is a version change.
5. **Numbers** are integers. Lengths are integer nanometres, angles integer millidegrees, and a key that holds a unit says so (`width_nm`, `rotation_mdeg`). Points are `[x, y]` integer pairs. Any other non-integer is a scaled integer with its scale in the key (`_ppm`) or an exact decimal string for a value a person typed (`"si": "5100"`).
   **Until the units migration finishes** (U2, U3), a file that holds a field on the `f64-lengths` allowlist allows floats, written in `serde_json`'s shortest round-trip form. Every other file refuses them. The [declaration table](#4-field-order-and-sort-keys) says which files allow floats today; when the allowlist is empty, every file refuses them.
6. **Nothing is nested as a string.** A value that is JSON is written as JSON, never as a string holding JSON. (Today the board sketch and waiver keys break this; F2 changes them. The rule is not checked by the encoder, because a person's text may look like JSON.)
7. **Depth**: at most 64 levels of arrays and objects, as in the canonical codec.

## 4. Field order and sort keys

Each stored record type has one **declared field order**: identity first (the id, or the key that identifies the record), then the record type in a mixed file, then references (net, instance, layer), names, sizes, geometry and flags. It is the key order on disk, the column order of a [table reply](#8-table-replies) and the order in generated documentation. A field the declaration does not list cannot be written, so adding a stored field means declaring it here.

The declarations are data in `sparksmith_core::format::records`, and the encoder uses them. Tests pin the orders of the main types and check that each declaration covers exactly the fields of its Rust type.

| File | Record fields, in order | Sorted by | Floats today |
|---|---|---|---|
| `circuit/instances.jsonl` | `id`, `component`, `designator`, `representation`, `physical`, `part`, `properties`, `unused_units`, `no_connect`, `functions`, `pin_config`, `marking`, `text_placements` | `id` | allowed |
| `circuit/nets.jsonl` | `id`, `name`, `terminals` | `id` | refused |
| `schematic/sheet-<id>.jsonl` | `type` `drawing`: `id`, `type`, `item`<br>`junction`: `id`, `type`, `position`, `placed`<br>`label`: `id`, `type`, `name`, `kind`, `position`, `rotation`, `manual`<br>`occurrence`: `id`, `type`, `instance`, `unit`, `position`, `rotation`, `mirror`<br>`sheet_symbol`: `id`, `type`, `child`, `position`, `size`, `pins`<br>`wire`: `id`, `type`, `from`, `to`, `corners` | `type`, then `id` | refused |
| `board/placements.jsonl` | `instance`, `layer`, `position`, `rotation_deg`, `locked` | `instance` | allowed |
| `board/tracks.jsonl` | `id`, `net`, `layer`, `width`, `start`, `end`, `locked` | `net`, then `id` | allowed |
| `board/vias.jsonl` | `id`, `net`, `span`, `drill`, `diameter`, `position`, `tented`, `locked` | `net`, then `id` | allowed |
| `board/fills.jsonl` | `id`, `net`, `layer`, `name`, `region`, `priority`, `settings`, `outline`, `locked` | `id` | allowed |
| `board/keepouts.jsonl` | `id`, `name`, `layers`, `forbid`, `outline` | `id` | allowed |
| `board/dimensions.jsonl` | `id`, `kind`, `refs`, `value`, `reference` | `id` | allowed |
| `rules/waivers.jsonl` | `key`, `reason` | `key` | refused |

| Document | Members after `format` and `version`, in order | Tables |
|---|---|---|
| `sparksmith-project.json` | `features`, `name`, `material_table`, `next` | none |
| `properties.json` | `title`, `revision`, `date`, `author`, `company` | none |
| `schematic/sheets.json` | `sheets` | `sheets` |
| `board/board.json` | `size`, `shape`, `stackup`, `fabrication` | none |
| `sparksmith-library.json` | `features`, `library`, `name`, `default_licence` | none |

Notes:

- **Tracks and vias are sorted by net, then id.** A net's copper is one block of lines: a raw read scoped to a net is one search and one short range, a diff groups changes by net, and two branches that route different nets insert lines in different places and merge cleanly. A line moves only when its net changes. There is one tracks file, not one per layer.
- **A sheet file** holds what the records' `sheet` field holds today, so its records do not repeat it; the header names the sheet.
- **Computed fill copper is not a declared field.** A fill that still carries it cannot be written; F2 moves it to a cache.
- A sort key is made of integers and texts. Integers sort by value, texts by UTF-8 bytes, an integer before a text.
- Field names and values are today's. When U3 changes the board records (integer nanometres, `width_nm`, tracks as a `path` that can hold arcs), these declarations change in the same version step.
- Files the tables do not list yet (`interface.jsonl`, `link.json`, `rules.json`, `profile.json`, `generated.jsonl`, `model-traces.jsonl`, drafts and library revisions) get their declarations with the slice that writes them.

## 5. The manifest

`sparksmith-project.json` identifies the tree and holds what belongs to the whole project:

```json
{
  "format":"sparksmith/project",
  "version":2,
  "features":["…"],
  "name":"ESP32-S31 dev board",
  "material_table":1,
  "next":{"id":654,"net":175,"sheet":3}
}
```

- `format` and `version` are the tree's. **Every file in the tree carries the same version.** A file of another version was saved by another build, most often merged in from another branch; it is refused with that explanation, and the project is converted as a whole (`sparksmith convert`).
- `features` lists the [named optional features](development.md#file-versions) the project uses, sorted, and is left out when it uses none. A reader refuses a feature it does not know, naming it.
- `next` holds the id counters: objects and definition lineages (`id`), nets (`net`) and sheets (`sheet`).
- `material_table` is the built-in material table revision the library snapshot's package materials are read from.

## 6. Library folders

A user or team library folder, the project's `library/` snapshot and, with GP5, the shipped parts shelf share one layout. A library folder is a macOS package too.

```text
<library>/
  sparksmith-library.json            format, version, library id, name, features, default licence
  components/<lineage>/r0001.json    one immutable file per revision, never rewritten
  symbols/<lineage>/r0001.json
  packages/ footprints/ bindings/ parts/ models/ fields/ quantity-kinds/ package-rows/
  catalog.json                       shelves and tags by lineage; mutable, not revisioned
  generators/<name>/                 a generator bundle: generator.json, the module and its part files
  generated.jsonl                    one record per generated part
  assets/sha256/<hash>
```

- A lineage's kind is a folder: a lineage has one kind, so its path never changes, and a pull request reads "footprints/…/r0003.json added".
- A lineage is a 128-bit id (32 hex digits) in a library folder and the project's small integer in a project snapshot. Import remaps it and records the origin.
- A revision file is layout B with tables per kind: one terminal, pin, pad, graphic or mapping per line.
- Old revisions nothing references stay until an explicit, previewed `library.prune`.
- Part files stay `.solidcraft`, written only by Solidcraft's writer and stored as assets by hash.

Today's library folders (`sparksmith_io::library_folder`) still use `lineages/<id>/r0001.json` and a `format: 1` manifest; F7 moves them to this layout.

## 7. Hashes

Hashes never use the file layout. They use the **canonical sorted form**: the encoding of `sparksmith_generator_abi::codec`, with keys sorted by UTF-8 bytes, each once, no whitespace, integers only, absent values left out and strings in NFC. A record's line on disk has its keys in declared order, so it is not its canonical form; the canonical form is computed from the value.

- **Asset**: the SHA-256 of its bytes, which is also its path.
- **Revision content hash** (F7, after U2 makes library geometry integer): the SHA-256 of the revision's canonical form, independent of Rust field order and of the layout.
- **Generated part result hash**: the SHA-256 of the generation's canonical bytes, as the generator ABI defines it.
- **Tree digest**: the SHA-256 of a listing with one line per file, in path order (paths compared as bytes): the file's SHA-256 as 64 lower-case hex digits, one space, the path, LF. It is written as 64 lower-case hex digits. The digest is computed, never stored in the tree, because a stored list of hashes would conflict on every merge. `FileSet::digest` computes it. Signing, when it comes, signs the digest.

## 8. Table replies

Agents read the design through scoped queries, not through files. A query that lists records answers with a table: column names once, then one row per record.

```json
{"revision":41,"columns":["id","net","layer","width","start","end","locked"],
 "rows":[[4,6,"Top",0.3,[0,0],[1,0],null]],"more":3}
```

- `revision`: the document revision the rows were read at.
- `columns`: in the record type's declared field order, whatever order a query asks for them in. Without a field list, every declared column at least one row has.
- `rows`: one value per column; `null` where a record leaves the value out. Replies are not files, so `null` is allowed here.
- `more`: how many matching records the reply leaves out after its last row; 0 when it is complete.
- Lengths are in the command wire's millimetres, not the file's units. A query over a mixed collection answers one table per record type.

The type is `sparksmith_core::format::table::Table`. The queries, their shared scope parameters and paging are extension-ABI slice S2c.

## 9. Bounds

All are targets until F3 measures a large board.

| Limit | Value |
|---|---|
| One line of a collection, with its LF | under 1 MiB |
| One collection file | 64 MiB |
| One document file | 4 MiB |
| One asset | 64 MiB |
| One tree, every file once | 512 MiB (the container limit) |
| Files in one tree | 65,536 |

There is no cap on the size of a design beyond these. The limits bound untrusted input, and `FileSet` and both readers enforce them before allocating more.

## 10. Reading

- **Refuse, never guess.** A reader accepts exactly the bytes the writer writes for the value it reads, so opening and saving changes no byte. Unknown files, fields, record types, versions and features refuse the file; nothing is dropped.
- A refusal names the file, the line and what is wrong: `board/tracks.jsonl, line 3: the field "id" comes after "net"; the declared order is id, net, layer, width, start, end, locked`.
- A file that a person, script or agent edited into a different but valid spelling (spacing, key order, `0.30`) is refused with its line, and `sparksmith fmt` (F5) will rewrite it into the written form.
- Every load validates the whole document after reading it, as today.
