# The capability surface

The contract every caller of Sparksmith reaches: the UI, the CLI, MCP, the built-in agent and, later, plugins. It is built in slices from [the extension ABI design](design/extension-abi-design.md); [the founder's decisions](design/decisions-2026-11-14.md) win where the two differ. This page says what exists now (slices S0, S1 and S2b) and how to change it.

## Entries and modules

An **entry** is one capability with a stable dotted wire name (`layout.add_via`). Today's three families are one catalog: the core's `Operation` (through `project.apply`) and `Query` (through `project.query`), and the automation crate's `Request`. The Rust enums stay the typed implementation.

A **module** is the unit of identity and versioning. Each entry belongs to exactly one, named on its declaration. The modules are declared in `sparksmith_core::permission::MODULES`:

| Module | Holds |
|---|---|
| `project` | inspect, properties, undo, redo, the wrapper requests, BOM |
| `files` | new, open and save a project file |
| `schematic` | sheets, symbols, wires, labels, placed components (`component.*`, `instance.*`), `functions.suggest`, pin reports |
| `layout` | `layout.*`, `fill.*`, `keepout.*`, `board.*`, `diff_pair.*`, `length_group.*`, dimensions |
| `rules` | `rules.*`, `net_class.*`, `impedance.*` |
| `library` | definitions, drafts, publishing, folders and sources, updates (`project.preview_update`, `project.apply_update`, `update`), pinout import, markings |
| `generators` | `library.generators`, `library.generate`, `library.generate_part` |
| `checks` | `project.issues`, `fabrication.preflight`, the fabrication waivers |
| `fab` | fabrication output, the schematic PDF, firmware pin configuration |
| `model3d` | the exact 3D export, the board interface, the assembly link, part files |

The per-entry assignment is `crates/automation/surface.lock`, generated from the declarations: every entry appears there under exactly one module. Names were not changed; a rename is a breaking change the lock makes deliberate. `view`, `settings`, `assist` and `host` come with the slices that give them entries (S2, S5, S6).

## Declaring an entry

Every command enum lists its variants once, beside the enum, with `declare_entries!` (`crates/editor-core/src/permission.rs`):

```rust
declare_entries!(FillCommand in layout {
    Add "fill.add" Edit "Add a copper fill",
    Refill "fill.refill" Edit "Refill copper" [long],
});
declare_entries!(Request in project {
    ExportFabrication "project.export_fabrication" Ask "Export fabrication files" [in fab, long, link],
});
```

A line is `Variant "wire.name" Class "Title" [attributes]`. A variant without its line does not compile, because the macro also writes the exhaustive `class()` and `command()` matches. The attributes, comma separated, all optional:

| Attribute | Means | Default |
|---|---|---|
| `in module` | another module than the family's | the family's module |
| `short`, `long` | runs under 2 s; long enough for progress and cancel (`time`) | instant |
| `link` | the result can pass 2 KiB (`result`) | inline |
| `review` | the person reviews the impact before it applies (`person`) | none; every action adds approval |
| `reach(Files \| Settings \| Window \| Network \| Runtime)` | what it touches outside the document | none |
| `effect(Pure \| Revision \| World)` | what its answer depends on | `world` with a reach, else `revision` |
| `needs(Files \| Window \| Runtime \| Solidcraft)` | host features it needs | none; a reach of files adds `files` |
| `since(major, minor)` | the module version it arrived in | `0.1` |

The title is at most 40 bytes; the summary is the first sentence of the doc comment (for requests, the catalog's summary), at most 140 bytes. The kind is derived: Never reaches no model; Ask or a path makes an action; else a read is a query and an edit an operation.

The macro builds `ENTRIES: &[EntryDecl]` for each enum. Discovery, the MCP tool annotations, the five-verb server's metadata and `surface.lock` all read these; no list elsewhere names an entry.

### Paths

A parameter that names a file or folder has the type `sparksmith_core::permission::FilePath`, a string on the wire whose schema carries `"x-kind": "path"`. The gate finds every path a request names by walking the request with its schema (`crates/automation/src/session/paths.rs`), so a request takes a path because its parameters say so. A request that names a path reaches `files`, is at least Ask and is limited to the run's folders. The mark is removed from every schema a caller is shown, so tool lists and `project.schema` replies are what they were.

## Descriptor types

The types are in the ABI crate, `sparksmith_generator_abi::surface` (`no_std` + `alloc`, so a plugin can read what the host writes; no generator module uses them, so module bytes do not change):

- `Class`: `read < show < edit < ask < never`. `show` (the window only: no revision, no undo step) has no entries until S5.
- `Kind`: `query`, `operation`, `show`, `action`, `event`.
- `Effect` (`pure`, `revision`, `world`), `Person` (`none`, `review`, `approval`), `Time` (`instant`, `short`, `long`), `ResultSize` (`inline`, `link`), `Reach`, `HostFeature`.
- `EntryDecl` and `ModuleDecl`: what a declaration says, built in `const` context. Reaches and needs are small bit sets.
- `ModuleInfo` and `EntryInfo`: a module and an entry as described to a caller.
- `Call<P>`: the envelope (`entry`, `params` of the host's JSON type, `project`, `expected_revision`, `dry_run`, `confirm`, `turn`, `job`), and `Error`, the one error shape (`code`, `message`, `at`, `rule`, `closest`, `instead`, `expected_revision`, `actual_revision`). S2 puts them to use.
- `Lock`, `LockedModule`, `LockedEntry`: the lock file.

## `surface.lock`

`crates/automation/surface.lock` is JSON, one line per entry:

```json
{"format": 1, "modules": [
 {"id": "checks", "version": "0.1", "digest": "sha256:…", "entries": [
  {"name":"fabrication.preflight","class":"read","schema":"3f…","fields":[["profile","9a…"]],"required":[]},
  …
 ]},
 …
]}
```

- **Module digest:** SHA-256 of the module's canonical description: its entries sorted by name, each with its kind, class, effect, reach, needs, person step, time, result size, `since` and the digest of its canonical input schema.
- **Canonical schema:** the entry's parameter schema with the definitions it reaches renumbered in the order they are first referenced, without `description`, `title`, `$comment`, `examples` and `$schema`. A renamed Rust type or an edited doc comment changes nothing; a changed field does. Titles and summaries are not contract.
- **Per entry:** the class, the first 16 hex digits of the schema digest, the same per top-level parameter, and the required parameters.

`cargo xtask surface --check` (in `cargo xtask ci`) compares the catalog with the lock. It runs the automation test `session::surface::tests::the_lock_holds`, since the catalog is built from that crate's types. It fails when:

- a module's digest changed and its version did not;
- a version went back, or a module was removed;
- from 1.0, an entry was removed, a parameter removed or retyped, a parameter became required, or a class became stricter, without a major raise;
- an entry is new and its `since` is not the module's new version;
- the lock is otherwise out of date.

`cargo xtask surface --update` rewrites the lock, after the same rules pass. To change the surface, raise the module's version in `MODULES` (and `since` on a new entry), then update the lock.

**Versions before the cut-over.** Every module is `0.x` until the MCP cut-over (S3), where each starts at `1.0` (decision 12). Before 1.0 the minor is the breaking number, as in semver: any change raises the minor. From 1.0 a break raises the major and an addition the minor.

## What S1 deleted

- `metadata::REVIEWED`, `LONG`, `LINKED` and `OUTSIDE_DOCUMENT`: now `review`, `long`, `link` and `reach(…)` on the declarations.
- `Request::NAMING_PATHS`, the 50-arm `Request::paths` match and the gate tests' `path_samples`: now `FilePath` and the schema walk.
- `declare_classes!` and its `CLASSES` and `TITLES` tables, `Operation::classes()` and `Operation::titles()`: now `declare_entries!`, `ENTRIES` and `Operation::entries()`.

`Request::expected_revision` stays until the envelope carries the revision (S2). MCP behaviour, the tool lists and their annotations are unchanged.
