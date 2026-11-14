# One capability surface: the extension ABI for agents, plugins, CLI and UI

Design, 14 November 2026, against Sparksmith `main` at f18c0658 and the Solidcraft checkout at 4f3ac07ae (read-only). Nothing was built, run or changed. Hours are agent hours and are estimates. Token and byte budgets are targets until measured. **(code)** marks what I read in source, **(doc)** what the repository's documents say.

Read beside it: `design-critique.md`, `part-generation-plan.md`, `file-format-plan.md` and `extension-abi-draft-critique.md` (an earlier draft on this same question; section 4.9 lists what is used from it and where this design differs).

Founder decisions kept as given: one typed command engine; programmatic calls never open dialogs; a failed mutation changes nothing; MCP `2026-07-28` only; `apply` keeps a plain `op` string; LLM-first with small local models; generators are sandboxed, deterministic WASM loaded as data, identified by hash and versioned; no library data in code; **one plugin ABI and one kind of plugin** (as recorded in the part-generation plan, section 3); clean room.

## Summary

The founder asked for a broader, modular ABI that serves both WASM modules and agents. The engine's typed catalog already is that ABI. What is missing is that it is described in five places, reachable through four different call shapes, and stops at the document: nothing can reach the window.

The design:

1. **One catalog of entries in modules.** Every capability is one entry (`layout.add_via`, `view.select`, `generators.generate`), declared once beside its Rust type with its class, cost and reach. Modules (`project`, `schematic`, `layout`, `library`, `generators`, `checks`, `fab`, `model3d`, `view`, `files`, `settings`, `assist`, `host`) carry a version and a digest.
2. **One front door**, `Surface::call`, which takes one envelope from one of five kinds of caller and applies one gate. UI, CLI, MCP, the built-in agent and WASM plugins are projections of it. None has a list of its own.
3. **Six verbs** carry it wherever a model is involved: `find`, `describe`, `query`, `apply`, `act`, and a new `show` for the window (selection, camera, workspace), which changes no document and takes no undo step.
4. **One plugin ABI, one kind of plugin.** Every plugin has the same exports, the same three optional imports, one description format, one host and one permission model. Each capability a plugin offers declares its effect: `none`, `read` or `edit`. A generator is a plugin whose `generate-part` capability has effect `none`: during that call the host answers nothing, so the result is a pure function of the request and is hashed. A capability with effect `read` or `edit` runs against a fork of the document, and the host applies what it did as one reviewed undo step. Approval is by bundle hash. I looked for a real reason to keep two kinds and found none (2.6).
5. **Deleted:** the old MCP server, `NOT_TOOLS`, the 64 KiB budget test, the assistant's hand-written tool list, the five metadata name lists, `Request::paths` and `NAMING_PATHS`, the `project.apply`/`query`/`schema` wrapper requests, `WINDOW_ONLY`.

About 83–124 hours for the surface, the cut-over, the window and the agent (S0–S7, S10), and 28–42 hours for the plugin ABI (S8, with the part-generation work) and for making its `read` and `edit` effects answer (S9), which should wait until the surface has been measured with agents.

---

## 1. Today's surfaces

### 1.1 What exists

| Surface | What it is | Where (code) | State |
|---|---|---|---|
| Core commands | `Operation` (168 in 12 families) and `Query` (43). `declare_classes!` forces a class and a title per variant | `crates/editor-core/src/lib.rs:61`, `permission.rs` | Solid. Every UI gesture ends in an `Operation` applied through `Engine::apply` (`apps/sparksmith/src/app.rs:329`) |
| Session requests | `Request` (49): the wrappers `project.apply`, `apply_all`, `query`, `schema`, `inspect`; files; library folders; exports; some edits that need the host (`library.generate_part`, part files) | `crates/automation/src/session.rs:305` | Solid, but a second family with its own class table, a 50-arm `paths()` match, a 50-arm `expected_revision()` match and `NAMING_PATHS` |
| Catalog metadata | 260 entries with kind, person step, long, result size, annotations | `session/metadata.rs`, `discovery.rs` | Derived, but its facts live in name lists: `REVIEWED`, `LONG`, `LINKED`, `OUTSIDE_DOCUMENT`, `NAMING_PATHS`, plus `path_samples` in the gate tests |
| Gate | `Caller::{Operator, Model, Allowed}`, `Policy {mode, folders}`, decisions Run, Ask, Refuse | `crates/automation/src/gate.rs` | Solid. Knows one kind of non-person caller. No per-module scope, no plugin |
| Old MCP server | One tool per request, `2025-06-18`, 14.8 KB of instructions, `tools/list` at 64,583 of 65,536 bytes | `mcp.rs`, `mcp/tests.rs` | **The one that ships.** `NOT_TOOLS` hides the generator requests to fit the budget |
| Five-verb MCP server | `find`, `describe`, `query`, `apply`, `act`, project handles, 2,986 bytes of tools, on `rmcp` 3.9.0 | `mcp/surface/` | Built (M2, P1, P2), **served by no binary**. P3–P9 not built (doc: mcp-surface) |
| Live channel | Solidcraft's control channel on loopback with a token file. Two methods: `engine.execute {command, params}` and `project.revision`. Busy refusal, `WINDOW_ONLY`, Ask waits for the window | `live.rs`, `apps/sparksmith/src/app/live_mcp.rs` | Solid for document requests. Nothing for the window. The channel's `project.changed` event name is declared but nothing subscribes |
| Built-in assistant | 17 hand-written tools for datasheet-to-part, each mapped by a `match` onto session requests; its own working pinout document; argument leniency for weak models | `crates/assist/src/tools.rs:83,199` | A third tool list. Runs in a headless session of its own, not on the open project. The general assistant panel is a proposal, not built (doc: assistant-panel) |
| Generator ABI 1 | No imports, four exports, canonical integer JSON, fuel 500 M, memory 32 MiB, wall 2 s, identity by hash | `crates/generator-abi`, `crates/generators` (wasmi 2.0.0) | Solid and separate: its own description format and refusal shape; cannot call anything; not reachable over MCP |
| CLI | 20 subcommands. `stdio` and `mcp` carry `Request`; the rest (`fab`, `export`, `part`, `pinout`, `firmware`, `design-part`, …) are hand-written wrappers | `apps/sparksmith-cli/src/main.rs:205` | Two shapes: a generic one and one-off subcommands with their own argument parsing |
| UI command registry | About 50 commands with contexts, chords and handlers: `AppCommand`, `RibbonEvent`, `PcbView`, `View3d`, `PcbArrange` | `apps/sparksmith/src/commands.rs` | A second registry, unrelated to the catalog. Workspace, view, layers, selection and camera are window state that no automation can read or set |

**Solidcraft, for comparison** (read in its `docs/control-protocol.md`, `mcp.md`, `agent-chat.md`, `plugins.md`, `crates/engine/src/cmd/mod.rs`, `crates/ui-egui/src/agent_chat/`):

- One flat registry of `CommandSpec {id, label, tab, panel, shortcut, enabled, run, undoable, contract}`. The toolbar, the command palette, `engine.execute`, the CLI and MCP all read it.
- A second namespace, `ui.*`, for the window: `ui.inspect`, `ui.set`, `ui.view`, `ui.camera`, screenshots, and raw input (`ui.click`, `ui.drag`) for its test scenarios.
- Its agent chat starts its own MCP server as a subprocess (`solidcraft-cli mcp --connect` to a private endpoint) and is that server's client, for ACP agents and for model endpoints alike. A gate lets the agent use seven methods only. Each message's changes are one undo step.
- `control.subscribe` sends `design.changed` lines.
- Plugins are a design, not built: WIT, wasmtime, and one import `run(command, args)` with the same JSON as MCP.

Sparksmith's typed enums and compiler-enforced classes are stronger than Solidcraft's untyped `Value` commands. Solidcraft is ahead in exactly two things: the window is on the surface, and its agent uses the public surface with nothing of its own.

### 1.2 Overlaps and conflicts

1. **Four descriptions of one engine**: old MCP tools, five-verb MCP tools, the assist tool list, CLI subcommands. Each new capability has to be added to several, and the cheapest reaction is to hide it (`NOT_TOOLS`).
2. **Three command families with three wrappers.** A document edit is reachable as `Operation` inside `project.apply`, inside `project.apply_all`, and sometimes also as a `Request` of its own (`library.set_marking`). A model has to learn which.
3. **Metadata is a second catalog.** Five name lists and a test-only list say things about entries that the entry's own declaration does not.
4. **Two permission vocabularies.** `Class` in the catalog; `WINDOW_ONLY`, the busy rule and `NOT_TOOLS` as ad hoc lists in transports.
5. **Three refusal shapes.** `Diagnostic {code, rule, message, expected_revision, actual_revision}`, `gate::Refusal {code, command, why, instead}` and the generator's `Problem {code, field, message}`.
6. **Process-wide state under a per-session engine.** `load_materials()`, the standards `OnceLock`, looks and starter data are installed once per process (critique §1), while the new MCP desk holds up to 16 projects.

### 1.3 Gaps

- **The window.** No entry reads or sets workspace, sheet, view, layers, selection or camera. "Agents control the whole program" is impossible for every caller today, the built-in assistant included.
- **A general built-in agent.** Only the datasheet workflow exists, and not on the open project.
- **Events.** No caller can hear that the document or the selection changed.
- **Long work.** No shared progress or cancel; `export::Watch` is private to the export.
- **Undo grouping for a turn.** `Engine::join_last_step` joins exactly two steps. An agent message that makes five edits is five undo steps.
- **Plugins that do anything but make a part.** No import, no caller identity, no approval.
- **Module boundaries and versions.** The catalog is one flat list with one implicit version (the build).

---

## 2. Target architecture

```
                    declared once, beside the Rust types
   core: Operation · Query        automation: services         app: view entries       plugins: commands
            └──────────────┬──────────────┴──────────────┬──────────────┴──────────────┬──────┘
                           ▼
                 Catalog: modules → entries (schema, class, cost, reach, version)      surface.lock
                           │
                 Surface::call(caller, envelope)   gate · revision · turn · jobs · events
                           │
   ┌───────────┬───────────┼────────────┬───────────────┬──────────────────┐
   UI          CLI         MCP          built-in agent  WASM plugin        (later: browser `window.sparksmith`)
   typed       call/find/  six tools,   same six tool   one import `call`,
   gestures,   describe,   2026-07-28   definitions,    runs on a fork
   palette     JSON lines               in process
```

### 2.1 Entries and modules

**An entry** is one capability with a stable dotted name. There is one namespace; the three families of today become one.

| Kind | Class | Touches | Examples |
|---|---|---|---|
| `query` | Read | nothing | `project.issues`, `library.search`, `view.context` |
| `operation` | Edit | the document, at an expected revision, as an undo step | `layout.add_via`, `generators.generate` |
| `show` | Show (new) | the window only: no revision, no undo step, rebuilds nothing | `view.select`, `view.camera`, `view.open` |
| `action` | Ask | files, publishing, network, anything undo cannot take back | `files.save`, `fab.export`, `library.publish` |
| `event` | — | not callable; a topic to subscribe to | `project.changed`, `view.selection` |

`Class` gains one member, ordered `Read < Show < Edit < Ask < Never`. Never-class entries (keys, endpoints, permission modes, plugin approvals, crash reports) exist in the catalog for the UI and the operator and are unreachable for every other caller.

**A module** is the unit of identity, versioning, discovery and permission.

| Module | Holds (today's names move here; membership follows the dotted prefix) | Needs from the host |
|---|---|---|
| `project` | inspect, properties, undo, redo, revision, turns, BOM | — |
| `schematic` | `schematic.*`, `component.*` (placed components), `functions.*`, pins | — |
| `layout` | `layout.*`, `fill.*`, `keepout.*`, `board.*`, `diff_pair.*`, dimensions | board-shape evaluator |
| `rules` | `rules.*`, `net_class.*`, `impedance.*`, fab profiles | shipped library |
| `library` | definitions, drafts, publish, folders, updates, pinout import, markings | shipped library, files for folders |
| `generators` | list, describe (with the compact line form), preview, generate, regenerate, detach; plugin install and approval (Never). This is GP7's surface | plugin runtime |
| `checks` | issues, ERC, production check, waivers | — |
| `fab` | fabrication output, schematic PDF, firmware export | files |
| `model3d` | exact export, board interface, assembly link, part files | Solidcraft engine, files |
| `view` | workspace, sheet, 2D/3D, layers, selection, camera, context, screenshot | a window (or an offscreen renderer for screenshots) |
| `files` | new, open, save, recent, import from another project | files; in a window, the window's own prompts |
| `settings` | read preferences; set an allowlisted subset; everything else Never | user config |
| `assist` | documents (PDF text, page images), the working pinout document by handle, review report | — |
| `host` | modules, find, describe, jobs, events, the caller's own grant | — |
| `plugin.<id>` | the `read` and `edit` capabilities an approved plugin contributes | plugin runtime |

- **Identity:** a lowercase id. It is contract; an entry's module never changes without a major bump of both.
- **Version:** `major.minor` per module. Minor adds entries or optional fields. Major may remove or change. Every entry records `since`. A module **digest** is the SHA-256 of its canonical description (entries sorted by name, schemas canonical).
- **Enforcement:** `cargo xtask surface --check` (in `cargo xtask ci`) compares the catalog with a committed `surface.lock` (module, version, digest, and per entry: name, class, a schema digest). A changed digest without a version change fails. A removed entry, a removed or retyped field, or a stricter class without a major bump fails. `--update` rewrites the lock. This is the same ratchet as `modules.lock` and the allowlists.
- **Before the first release** modules may break freely, but every break bumps the major, so a recording, a plugin or a bug report names what it was made against. I recommend starting each module at `1.0` at the cut-over rather than `0.x`: the number costs nothing and the plugin `needs` check (2.6) wants a real major from the first day.
- **Discovery:** `host.modules` lists `{id, version, digest, title, summary, entries, available, why_not}`. A headless host reports `view` as unavailable ("no window") instead of failing calls. `host.find {text, module?, kind?, limit?}` and `host.describe {name | module, detail}` are what the `find` and `describe` verbs call.
- **Who contributes entries:** the core (operations, queries), `sparksmith-automation` (services: files, exports, folders, jobs), the desktop app (the `view` module, through a `WindowHost` trait handed to the surface the way `Evaluators` and the generator runtime are handed to the engine), and approved plugins.

### 2.2 Descriptions

One description per entry, generated from the Rust types, with three levels of detail so a 32 k model never pays for more than it uses.

| Level | What | Budget (target) | Reached by |
|---|---|---|---|
| Index | The modules: id, one line each, entry count | ≤ 1.2 KB for all modules | `describe {}` or the MCP instructions |
| Line | One entry as a single signature line plus its summary | ≤ 160 bytes per entry; a module's lines ≤ 4 KB, else paged | `find`, `describe {module}` |
| Full | JSON Schema 2020-12 of input and result, one example that validates, the person step, related guide | as today (compacted: no `default`, `format`, nullable types, `$defs` inlined) | `describe {name}` |

**The line form** is new and is what small models read. It is generated from the schema, never written by hand:

```
layout.add_via {at:[len,len], net:id, layers?:[layer,layer], drill?:len, pad?:len} -> {via:id}   edit
  Add a via at a point on a net.
```

Types in a line come from a closed vocabulary of about a dozen words (`id`, `len`, `angle`, `layer`, `text`, `int`, `bool`, `point`=`[len,len]`, `path`, `ref`, `{…}`, `[…]`, `a|b`), each defined once in the index. A nested object deeper than two levels is written `{…}` and the model asks for the full schema. A test holds every line under its budget and requires that each parses back to the entry's required fields.

**Fields of an entry description** (the full list is the `EntryInfo` struct in 3.1):

- `class`: read, show, edit, ask, never.
- `reach`: what it can touch outside the document: `window`, `files`, `network`, `settings`, `runtime` (runs a plugin). This replaces `OUTSIDE_DOCUMENT`, `takes_path` and `NAMING_PATHS`. A path-typed argument is a `FilePath` in the parameter struct, whose schema carries `"x-kind":"path"`; the gate finds paths by walking the value with the schema, so the 50-arm `Request::paths` match and its sample list go.
- `person`: none, `review` (the person owns the impact: updates, pinout import, regenerate), `approval` (every Ask).
- `cost`: `time` (instant, under 100 ms target; short, under 2 s; long, a job) and `result` (inline, within 2 KB; link).
- `effect`: `pure` (depends on its parameters only), `revision` (depends on the document at a revision), `world` (clock, files, network, window, settings). It decides what a plugin may call (2.6) and what may be cached.
- `needs`: host features (`window`, `files`, `runtime`, `solidcraft`).
- `since`, `title` (≤ 40 bytes), `summary` (≤ 140 bytes), `example`.

All of these are declared on the same line as the class, in the macro that already refuses to compile without one:

```rust
declare_entries!(LayoutCommand in layout {
    AddVia   "layout.add_via"    Edit "Add a via",
    RouteStep "layout.route_step" Edit "Route one step" [short],
    Refill   "fill.refill"       Edit "Refill copper"  [long],
});
declare_entries!(Service in fab {
    Export   "fab.export"        Ask  "Export fabrication files" [long, link, reach(files)],
});
```

Defaults (instant, inline, no person step, effect from the kind) keep most lines as short as today's. The first sentence of the doc comment stays the summary.

### 2.3 Callers and permissions

**Five kinds of caller**, named by the transport and never by the request:

| Principal | Who | Default grant |
|---|---|---|
| `Person` | The UI: a gesture, the palette, a shortcut | Everything. The UI still calls the surface, so the journal, events and undo labels are one path |
| `Operator` | `sparksmith call`, `stdio --operator`: the person's own script | Everything, as at the command line |
| `LocalAgent` | The built-in agent, in process, on the open project | Mode `auto-allow-edits`; all document modules to Edit; `view` to Show; `files`, `fab`, `model3d` to Ask (asks in the panel); `settings` read |
| `RemoteAgent` | An MCP client, headless or live, including an ACP agent in the panel | Headless: as the CLI flags say (today's `--permission-mode`, `--allow-folder`). Live: the window's mode; `view` to Show only when the person turns on "Let the assistant move my view" |
| `Plugin {hash}` | A WASM plugin run | Exactly what the person approved for that bundle hash, never more than the plugin declared |

**A grant** generalizes today's `Policy`:

```
Grant { mode, folders, modules: {module → highest class}, limits: {changes per turn, calls per turn} }
```

**One decision function**, the same rules for every caller, in this order:

1. The entry exists and its module is available on this host, else `catalog.unknown` (with nearest names) or `host.unavailable`.
2. Never is refused for everyone but Person and Operator: `gate.never`.
3. The module's ceiling: a class above `grant.modules[module]` is refused with `gate.module`, saying which module and how the person can allow it.
4. Every path lies inside the grant's folders after links are resolved: `gate.path_outside`.
5. The mode decides Run or Ask, as today. A `person: review` entry always asks with its plan.
6. In a window, anything but a query is refused while the person is typing, dragging or in a dialog: `live.busy`.
7. Limits: a turn past its change or call limit stops with `gate.turn_limit`.

**How Ask is answered** is the host's, through one trait (`Asker`): the window's Allow prompt, a card in the panel, an MCP elicitation where the client has it, or no way at all, which gives `needs_person` with what the person can do instead. The model's wording is never the control.

**Approval by hash** (plugins):

- A plugin's description declares its capabilities, each with an effect, and its `needs` (2.6). `needs` is its ceiling. What is approved is the bundle: manifest, module and part files, under one hash.
- Installing shows the person: the name and version, the publisher's text as *claimed*, the SHA-256, and the needs in plain words ("read the board and the schematic; change the board layout; no files, no network").
- The person's Allow writes one record to `<config>/plugins/approved.json`, keyed by the bundle hash, holding the granted subset. The person may grant less than asked.
- A changed bundle is another hash and asks again, showing what changed against the approved plugin of the same id (version, needs added or dropped).
- Shipped plugins are approved by the build: `library/plugins/` with a lock, as shipped generator modules are.
- Approvals, revocation and the grant itself are Never-class: no agent and no plugin can raise its own permissions or another's.
- A plugin with only effect-`none` capabilities cannot do anything but compute, so approving it is about trusting its *output*: its grant is empty. A project's embedded plugin is checked on open under the existing rules, and a mismatch refuses the project as today; GP9 adds that an unapproved third-party bundle is not run on open.
- Signatures are not part of this design. The file-format plan defines the digest a detached signature would cover; add signing when an online library exists.

### 2.4 Semantics

**The envelope** is the same for every transport:

```json
{"entry": "layout.add_via", "params": {…}, "project": "prj_…", "expected_revision": 41,
 "dry_run": false, "confirm": null, "turn": "t_…", "job": false}
```

`project`, `expected_revision`, `dry_run`, `confirm`, `turn` and `job` are fields of the envelope, never of an entry's parameters. So the 50-arm `Request::expected_revision` match goes too: an operation requires the revision because of its kind.

**Revisions.**
- Every operation, and every action that changes the document, requires `expected_revision` and is refused unchanged when stale (`stale_revision`, with both numbers). Checked once in the surface and again in the engine, as today.
- Every reply carries `revision`.
- `show` entries carry no revision: last writer wins, and the person's own input always wins (busy rule).

**Preview and apply, one mechanism.**
- `dry_run: true` runs any operation on `Engine::fork()` and answers what the real call would answer, keeping nothing (built: `Session::rehearse`).
- An entry marked `person: review` answers a dry run with a **plan**: the impact in the entry's own terms and a `plan` digest that binds the entry, its parameters, the base revision and, for anything that reads another file (a library folder, a linked design, a generator bundle), that file's revision or hash too.
- Applying with `confirm: <digest>` applies exactly that plan or nothing. A moved revision on either side gives `plan.stale` and a fresh plan.
- **Library updates** use this and nothing else: `library.update` (today's `project.preview_update` plus `project.apply_update`, merged) previews per placed instance and is bound to the project revision and the library revision it read. Regenerating a part under a newer generator version is the same entry family (`generators.regenerate`), so "never silent" holds by construction.
- Who may confirm: the person, through `Asker`. An agent may confirm a plan itself only under `allow-all` (mcp-surface Q4, unchanged).

**Undo grouping: turns.**
- A **turn** is a labelled group of edits that undo as one step. `project.turn_begin {label}` gives a handle; edits carrying it fold into the group; `project.turn_end` or the next edit by anyone else closes it.
- The engine gains `begin_group(label)` and `end_group()`, which keep the document from before the group as the one undo target. `join_last_step` becomes the two-step case of it.
- A group never hides the person's own edits: a person's edit between two agent edits closes the group, so undo order stays what happened.

| Caller | A turn is | Undo |
|---|---|---|
| Person | One gesture | One step |
| Built-in agent | One message | One step named after the message, as in Solidcraft |
| Remote agent | One `apply` batch, or a turn it opens | One step each |
| Plugin | One run | One step named after the plugin's command |

**Long work: jobs.**
- A core trait, with no transport or UI dependency: `Progress { fn report(&self, done, total, stage); fn cancelled(&self) -> bool; }`. Fills, the production check, fabrication output, exact export and library rescans take it. `export::Watch` becomes an implementation.
- An entry with `cost.time: long` answers `{job: "job_…"}` when the envelope says `job: true` (the MCP projection sets it when the client declares tasks; the UI always does). `host.job {job}` reads state and result; `host.job_cancel {job}` asks it to stop at the next check point.
- A job reads the document at a revision and its result names that revision. A result is applied only to a matching document (commands skill, unchanged).
- **Operations never become jobs.** An edit lands at the revision it was checked against or not at all. A cancelled job leaves the document unchanged.
- Jobs live in the process and say so; a finished job's result is kept for 10 minutes or until read.

**Events.**
- Topics are catalog entries of kind `event`, so they are described and versioned like everything else: `project.changed {project, revision, areas, by}`, `view.selection {ids}`, `view.changed {workspace, view}`, `checks.counts {by severity}`, `library.changed`, `host.job {job, done, total, stage, state}`.
- An event is a hint with a revision, not the data: the subscriber reads what it needs. Events are coalesced to at most 4 a second per topic (the mcp-surface figure). A subscriber that falls behind gets one `host.lagged`, and reads state again.
- Carried as: an in-process channel (UI panels, the built-in agent), id-less JSON lines on `stdio` and the live channel (Solidcraft's `control.subscribe` shape), MCP subscribable resources (P6). Plugins receive none in this version.

**List replies are tables, with one set of scope parameters** (agreed with the file-format plan, its section 8.5).
- A query that lists records answers `{"revision", "columns": […], "rows": [[…]], "more"}` in wire millimetres, columns in the record's declared field order. Measured by that planner on the ESP32 example: 18–47 % fewer tokens than keyed objects (o200k_base, an approximation for Claude).
- Every list query takes the same parameters: `ids`, `net`, `layer`, `sheet`, `kind`, `bbox` (mm), `fields`, `limit` and `after`, and `count: true` for the number alone. They are declared once as a shared type, so the line form shows them as `…scope`.
- Entries: `layout.objects`, `schematic.items`, and `library.definition` with a `part`. These replace reading the document by JSON pointer (`project.inspect {at}`) for anything large.

**Errors, one shape, in plain words.**

```json
{"code": "layout.clearance", "message": "The via would be 0.08 mm from the track on net GND; the rule asks for 0.15 mm. Nothing was changed.",
 "at": "/params/at", "rule": "clearance.copper", "closest": [], "instead": "Move it at least 0.07 mm away, or pick another layer pair.",
 "expected_revision": 41, "actual_revision": 43}
```

- `code` is stable and dotted, first segment the module or `gate`, `catalog`, `host`, `plugin`.
- `message` says what happened in the person's words and ends by saying what was or was not changed.
- `at` is a JSON pointer into the call. `instead` is what to do next. `closest` carries nearest names for a misspelt entry.
- `Diagnostic`, `gate::Refusal` and the generator's `Problem` become this one type. A plugin's own refusals pass through with the plugin's code under `plugin.<id>.`.

### 2.5 The six verbs

Where a model is the caller, the catalog is carried by six tools, one per class, so each tool's annotations are exact:

```
find     {text, module?, kind?, limit?}                       read
describe {name?, module?, detail?}                            read
query    {project?, name, params}                             read
show     {name, params}                                       changes the window, never the document
apply    {project?, expected_revision, operations:[{op, …}], dry_run?, confirm?, turn?}   undoable edit
act      {project?, name, params, expected_revision?, confirm?}    files, publishing; a person approves
```

- `show` is the one addition to the approved five. Steering the window is not a read (it changes what the person sees), not an edit (no revision, no undo) and not an action (nothing irreversible, no approval). Folding it into `act` would make `act`'s "destructive, open world, person approves" annotations untrue for a camera move; folding it into `apply` would demand a revision it does not have. Cost: about 100 tokens (target). See decision 1.
- `op` stays a plain string (founder). Line descriptions and nearest-name refusals are what a small model gets instead of an enum.
- The verbs are not only MCP's. The built-in agent is given the same six definitions (2.7), and `sparksmith stdio` accepts `{"verb": "query", …}` lines.

### 2.6 The plugin ABI: one ABI, one kind of plugin

**Founder decision: one plugin ABI, not two types.** This section starts from one ABI and tests whether everything that motivated a split fits inside it. It does. Purity is a declared effect of a capability, enforced by the host per call; it is not a second kind of module.

**The one contract** (every plugin, no exceptions):

| | |
|---|---|
| **Exports** | `sparksmith_abi() -> u32`; `sparksmith_alloc(len) -> ptr`; `sparksmith_describe() -> (ptr, len)`; `sparksmith_invoke(ptr, len) -> (ptr, len)`; `memory` |
| **Imports** | At most the three functions of module `sparksmith`: `call`, `log`, `cancelled`. A module may import fewer or none. Anything else is refused before the module runs |
| **Describe** | One description format: `{abi, id, version, title, licence, capabilities: [...], needs: [...]}` |
| **Invoke** | `{capability, params, context?}` in; a status byte, then a result or `{errors: [...]}` out |
| **Host** | One host (today's `sparksmith-generators`), one wasmi configuration, one validator, one limits table, a fresh instance per invoke |
| **Permission** | One model: `Principal::Plugin {hash}` with a grant, decided by the same function as every other caller (2.3) |

**A capability** is one thing the plugin can be asked to do. Each declares its **effect**, and the effect is the only thing that differs between what was called "a generator" and "an automation plugin":

| Declared effect | What `call` does during this invoke | Runs against | Result | Use |
|---|---|---|---|---|
| `none` | Refused, always, with the fixed error `plugin.pure`; `cancelled` answers 0; `log` is dropped | nothing | Canonical bytes, **hashed** | `generate-part`; later any other pure function (a calculator, a land-pattern rule) |
| `read` | Queries whose own effect is `pure` or `revision`, within the grant | a frozen fork at revision R | A report, findings, or named artifacts | Custom checks, BOM formats, exporters |
| `edit` | The above plus operations, within the grant | a private fork at revision R | The edits made on the fork | Commands: stitch vias, renumber, fan out |

**The founder's three questions:**

1. **How do the needs behind the split fit in one ABI?**
   - *Determinism and hash-verifiable results, checked on every open.* An invoke with effect `none` is a pure function of its request: the host gives it nothing but the request (every import answers a constant), a fresh instance, a fixed fuel budget and no clock. That is ABI 1's guarantee, reached by what the host answers rather than by what the module is. The check on open invokes only `generate-part`, so opening a project never causes an effectful run.
   - *Integer-only geometry.* A property of the `generate-part` result schema (canonical JSON, nanometres, millidegrees), checked by the host's decoder. It is a rule about one capability's output, not about a kind of plugin.
   - *Side effects and command calls.* The `call` import, answered only during invokes whose capability declares `read` or `edit`, only within the approved grant, and on a fork.
2. **Can they be expressed as declared effects within one ABI, one host, one description format and one permission model?** Yes: effect per capability, `needs` per plugin, one grant, as in the two tables above.
3. **Is a generator just a plugin that exposes `generate-part` and imports nothing effectful?** Yes. A plugin with only effect-`none` capabilities needs no grant and may import nothing at all. One plugin may also offer both: a connectors plugin with `generate-part` for the part and an `edit` command that places a row of them. Its `generate-part` is exactly as pure and as hashed as in a plugin that offers nothing else.

**Where I looked for a real reason to keep two kinds, and what I found:**

| Candidate reason | Finding |
|---|---|
| A module with a `call` import cannot be proved pure | It can. Purity is what the host answers during the invoke: a constant refusal carries no information, so the result depends on the request alone. The golden cases (native, wasmi, wasmi inside wasmi) hold unchanged, and a new test shows a module that calls `call` during `generate-part` still gives one result hash |
| Embedded in a project versus installed per user | Follows from the capability, not the plugin. A project stores a bundle only because it holds a part that `generate-part` made, and only that capability is ever invoked from a project. The plugin's other capabilities run only when the person has installed and approved it |
| Old ABIs must stay runnable so projects reopen offline | True of the one ABI. It stays cheap for the same reason: an old ABI number needs only its exports and the null host to verify parts. `read` and `edit` capabilities are served on the current number only |
| Different limits | Parameters of the effect, in one limits table (below) |
| Refusing float opcodes | A determinism rule declared per plugin, not a kind (below) |
| Different trust: a pure function is safe to run, a command is not | Approval is always by bundle hash, and what is granted is the declared `needs`, which is empty for a pure plugin. One dialog, one store |

**I do not recommend two kinds.** The single thing given up against ABI 1 is the purely structural proof "this module has no imports". It is kept as the stricter rule where it applies: a plugin that declares only effect-`none` capabilities is refused if it imports anything, so today's chip generator is checked exactly as it is now.

**The import table** is three functions and never grows. Every capability of the program is a catalog entry reached through `call`, so versioning is the catalog's job.

| Import (`sparksmith` module) | Signature | Meaning |
|---|---|---|
| `call` | `(ptr: u32, len: u32) -> u64` | One envelope `{entry, params, dry_run?}` in; `(ptr << 32) \| len` of a status byte and the reply or error out. Gated by the invoke's effect and the grant; counted |
| `log` | `(level: u32, ptr: u32, len: u32)` | A line for the run's log (64 KiB per run). Never shown as if the program said it |
| `cancelled` | `() -> u32` | 1 when the person pressed Stop or the wall time is near. Always 0 in an effect-`none` invoke |

No WASI, clock, random numbers, files, network or threads. A plugin cannot name a path: one that makes files returns artifacts, and the host saves them through the ordinary Ask path into a folder the person chooses. `show` and `action` entries are not callable from a plugin in this version.

**How a `read` or `edit` invoke runs:**

1. The person, an agent or a shortcut invokes the capability's entry (`plugin.stitch.vias`). The effective grant is the smaller of the caller's and the plugin's approved grant.
2. The host forks the engine at revision R (`Engine::fork`, which shares unchanged sections) and instantiates the module fresh.
3. Every `call` runs on the fork through `Surface::call` as `Principal::Plugin`. Operations change the fork and answer the usual compact reply, so the plugin can react to a refusal.
4. On Ok the host applies the successful operations to the live document with one `apply` at `expected_revision: R`, as one undo step named after the command, after whatever the mode asks of the person (the impact is the dry-run reply).
5. A trap, fuel or memory exhaustion, a timeout, Stop or an error result drops the fork: the document is unchanged. A document that moved meanwhile makes the run stale: unchanged, and the message says to run it again.

Every invoke is therefore a function of (plugin, parameters, document at R), with no R for effect `none`. It can be recorded, replayed and tested with goldens. There is no reentrancy.

**A plugin is a bundle.** The unit that is identified, approved and embedded is the bundle of the part-generation plan: a manifest (`generator.json` today), the module, and the `.solidcraft` part files it may name by hash. The bundle hash covers all three. The module only names asset hashes fixed at build, so naming a part file is not an effect.

**Identity** (part-generation B4, as agreed with that planner): a part's key is (plugin id, params hash), with the scope inside the hash and the hash taken over the parameters as recorded, so a parameter a newer version adds does not change the key. The plugin's version distinguishes revisions of that one lineage: the same parameters on the same version change nothing, and a newer version regenerates the part as a newer revision through the ordinary reviewed update. The result hash is the integrity check. The bundle hash is provenance and what a person approves.

**Resource limits**, one table by effect. The `none` column is today's measured setting; the other is a target to be set from measurement in S9.

| Bound | effect `none` | effect `read`, `edit` |
|---|---|---|
| Module size | 1 MiB | 4 MiB |
| Fuel per invoke | 500 M (about 0.5 s) | 5 G, the module's own work only |
| Linear memory | 32 MiB | 64 MiB |
| Wall time | 2 s | 30 s, with Stop |
| Calls per invoke | none succeed | 10,000 |
| Request / reply | 64 KiB / 1 MiB | 1 MiB / 4 MiB per call |
| Artifacts | — | 64 MiB in total |

**Determinism rules.**
- Hashed results are canonical JSON: keys sorted by UTF-8 bytes, no whitespace, integers only, NFC, absent optionals omitted; lengths in nanometres, angles in millidegrees. One codec, in the ABI crate, which the file format also uses for its hashes (file lines themselves are plain JSON in declared field order, file-format plan 8.5). The crate is moved or renamed only when part-generation B re-locks every module, or with a check that the rebuilt shipped module is byte-identical (the file-format planner's caveat: today a part key is the module hash).
- `generate-part` output is final integer geometry; the host does no floating-point expansion (part-generation B3).
- A plugin that offers any hashed capability declares `deterministic: true`, and the host validates it under the strict feature set. I recommend that this set refuse float opcodes (wasmi `Config::floats(false)`, present in the pinned 2.0.0). WebAssembly float arithmetic is deterministic except for NaN bit patterns, so the risk today is small; the refusal makes it structural. It costs more than B3 alone: the chip module uses floats today, so it and the SDK would be ported to fixed point (decision 4). GP9's "nondeterministic modules rejected" is this validation plus running each golden case twice.
- `read` and `edit` invokes exchange the command wire form (2.8). They are not hashed, and stay reproducible because every input is a reply on a frozen fork. Only `pure` and `revision` entries are callable.

**Versioning.**
- `abi`: the number of this one contract. The host keeps every ABI number runnable for effect-`none` invokes, because a project must reopen offline. `read` and `edit` capabilities are served on the current number only.
- `needs: [{"module": "layout", "major": 1, "minor": 2, "class": "edit"}, …]`: checked against the catalog before anything runs. A host that cannot meet it refuses, naming both versions. A plugin with only effect-`none` capabilities has empty `needs`.
- `id` and `version` identify a plugin to people; the bundle hash identifies it to the program.

**The check on open** is unchanged from GP1: a part that does not match its plugin refuses the project, which the founder accepts while there are no users. Opening with such parts flagged is an option, not a requirement, and nothing here depends on it. GP9 adds that an unapproved third-party bundle is not run on open until approved; it reads the approval store S8 builds.

**Not in this version:** plugins that stay resident and receive events, plugin panels or tools in the UI, plugins calling plugins.

### 2.7 The built-in agent

- The agent's tool list **is** the six verb definitions, produced by the same function that produces MCP's `tools/list`. A test asserts the bytes are equal. `assist::tools::definitions()` and its `match` are deleted.
- It calls `Surface::call` in process as `Principal::LocalAgent`. On the open project its calls go through the window's pump, the same queue the live channel feeds, so busy, revision, gate, turn and labelling are one code path with external clients. Headless (the CLI's `design-part`) it calls a session's surface directly.
- No MCP framing, no `rmcp` client, no subprocess and no socket between our own agent and our own engine. This departs from the approved mcp-surface §4, where the endpoint loop is an in-process `rmcp` client (decision 2).
- ACP agents in the panel (Claude Code, Codex, …) bring their own MCP client, so they connect through `sparksmith mcp --live` to a per-conversation channel, as designed. They and the built-in loop meet at the same `Surface::call`.
- **Workflows are data, not tools.** Datasheet-to-part is a prompt plus a skill in `library/assist/`, over entries in the `assist` module: `assist.document_text`, `assist.document_page`, `assist.pinout_edit` (the working document behind a handle with a stated lifetime), `assist.submit_review`. A **profile** is a named filter over the catalog (modules, kinds, highest class): the datasheet run's profile cannot reach publish, files or settings. Profiles are data in the shipped library, like the run profiles today.
- **Leniency for weak models** (arrays sent as text, integers a double would round) moves from `assist/tools.rs` into the verbs' argument reader, so every client gets it and it is tested once.
- **Context:** `view.context` (workspace, sheet or side shown, selection with ids, visible layers, what changed by hand since the caller's last call) is a query like any other, read at the start of each message. This is the "sees what you see" of the LLM-integration design.
- **One undo step per message**, through a turn.
- Person-owned steps show as cards in the panel through `Asker`.

### 2.8 The wire forms

Two encodings exist on purpose and stay distinct:

| | Command wire | Canonical data |
|---|---|---|
| Used for | Every envelope: agents, CLI, MCP, `report` and `command` plugins | What is hashed or stored: generator output, files, digests |
| Lengths | A number in mm, or text with a unit (`"0.25mm"`, `"10mil"`); replies give mm (code: `units/wire.rs`) | Integer nanometres |
| Why | Models and people write `0.25`, not `250000` | Bytes must be equal everywhere |

One small addition: the length text accepts `nm`, so a plugin can send exact integers (`"250000nm"`).

### 2.9 What gets deleted or merged

| Today | Becomes |
|---|---|
| `mcp.rs`: the old server, `tools()`, `live_tools()`, `INSTRUCTIONS`, `LIVE_INSTRUCTIONS`, protocol constants, its tests | Deleted at the cut-over (S3). The verbs are served from the surface |
| `NOT_TOOLS` and the 64 KiB `tools/list` budget test | Deleted. Six tools have a fixed size; entries are data behind them |
| `Request::Apply`, `ApplyAll`, `Query`, `Schema` | Deleted as entries: they are the `apply`, `query`, `describe` verbs and the envelope |
| `Request` (49), `Operation` (168), `Query` (43) as three families with three class tables | One catalog. The Rust enums stay as the typed implementation; `Request` shrinks to host services |
| `metadata.rs` lists `REVIEWED`, `LONG`, `LINKED`, `OUTSIDE_DOCUMENT`; `NAMING_PATHS`; `Request::paths`; `path_samples`; `Request::expected_revision` | Fields on the entry's declaration, a `FilePath` type, and the envelope |
| `live::WINDOW_ONLY` | `files.*` entries that, in a window, run the window's own prompts (unsaved work included) and answer when the person has decided |
| `project.preview_update` + `project.apply_update`; `interface.publish_preview` + `interface.publish`; `assembly.preview` + `assembly.update`; `firmware.export_preview` + `firmware.export` | One entry each with `dry_run` and `confirm` |
| `assist::tools::definitions`, `execute`, the pinout `Workspace` | The verbs, `assist.*` entries, a handle |
| CLI subcommands `export`, `fab`, `export-schematic-pdf`, `firmware`, `pinout`, `part`, `commands` | `sparksmith call <entry> [json]`, `find`, `describe`; the old names stay as aliases that print the equivalent call, or go |
| `Diagnostic`, `gate::Refusal`, generator `Problem` | One error type |
| `Policy`, `Caller` | `Grant`, `Principal` |
| `apps/sparksmith/src/commands.rs` actions `AppCommand`, `PcbView`, `View3d`, `PcbLayer` | `view.*` and `files.*` entries. The registry keeps what is the UI's alone (chords, contexts, icons, in-gesture keys), keyed by entry name |
| Generator ABI's own description and refusal shape | The one plugin description (a `generate-part` capability with effect `none`) and the one error shape |
| The Rhai pad script (`library.generate_pads`) | Deleted by the part-generation plan. Rhai stays an expression language only, so there is one sandbox for code: WASM |
| Process-wide standards, looks, starter data, materials | An explicit `Context` owned by the surface's session (critique §1). Required before several projects in one process can differ |

---

## 3. Interface sketches

Shapes, not final code. Names follow the repository's conventions.

### 3.1 Descriptors (ABI crate, `no_std` + `alloc`)

```rust
pub enum Class { Read, Show, Edit, Ask, Never }            // ordered; a batch takes the strictest
pub enum Kind { Query, Operation, Show, Action, Event }
pub enum Effect { Pure, Revision, World }
pub enum Person { None, Review, Approval }
pub enum Time { Instant, Short, Long }
pub enum ResultSize { Inline, Link }
pub enum Reach { Window, Files, Network, Settings, Runtime }

pub struct ModuleInfo {
    pub id: &'static str,                 // "layout"
    pub version: (u16, u16),              // major, minor
    pub digest: [u8; 32],                 // SHA-256 of the canonical description
    pub title: &'static str,
    pub summary: &'static str,            // ≤ 140 bytes
    pub needs: &'static [HostFeature],
}

pub struct EntryInfo {
    pub name: &'static str,               // "layout.add_via"
    pub module: &'static str,
    pub kind: Kind,
    pub class: Class,
    pub effect: Effect,
    pub reach: &'static [Reach],
    pub person: Person,
    pub time: Time,
    pub result: ResultSize,
    pub since: (u16, u16),
    pub title: &'static str,              // ≤ 40 bytes
    pub summary: &'static str,            // ≤ 140 bytes
    pub line: String,                     // generated signature, ≤ 160 bytes
    pub input: fn() -> Schema,
    pub output: fn() -> Schema,
    pub example: fn() -> Value,
}
```

### 3.2 The front door (`sparksmith-automation`)

```rust
pub struct Call {
    pub entry: String,
    pub params: Value,
    pub project: Option<ProjectHandle>,
    pub expected_revision: Option<u64>,
    pub dry_run: bool,
    pub confirm: Option<PlanDigest>,
    pub turn: Option<TurnId>,
    pub job: bool,
}

pub enum Principal<'a> {
    Person,
    Operator,
    LocalAgent { grant: &'a Grant },
    RemoteAgent { grant: &'a Grant, client: &'a str },
    Plugin { hash: [u8; 32], grant: &'a Grant },
}

pub struct Grant {
    pub mode: Mode,                                   // ask-every-time | auto-allow-edits | allow-all
    pub folders: Vec<PathBuf>,
    pub modules: BTreeMap<ModuleId, Class>,           // highest class per module; absent: none
    pub limits: Limits,                               // changes and calls per turn
}

pub trait Surface {
    fn modules(&self) -> Vec<ModuleStatus>;           // with `available` for this host
    fn entry(&self, name: &str) -> Option<&EntryInfo>;
    fn decide(&self, who: &Principal<'_>, call: &Call) -> Decision;      // Run | Ask(Question) | Refuse(Error)
    fn call(&mut self, cx: &mut CallCx<'_>, call: Call) -> Result<Reply, Error>;
}

pub struct CallCx<'a> {
    pub who: Principal<'a>,
    pub asker: &'a mut dyn Asker,        // how this host asks a person
    pub progress: &'a dyn Progress,      // report and cancel
    pub events: &'a dyn EventSink,
}

pub trait Asker {
    /// Allow, Deny, or `Unavailable` when this host has no way to ask.
    fn ask(&mut self, question: &Question) -> Answer;
    /// A person-owned plan: accept all, accept some rows, or decline.
    fn review(&mut self, plan: &Plan) -> ReviewAnswer;
}

pub trait Progress {                     // in sparksmith-core: no transport, no UI
    fn report(&self, done: u64, total: u64, stage: &str);
    fn cancelled(&self) -> bool;
}

pub trait EventSink { fn emit(&self, topic: &'static str, body: &Value); }

pub trait WindowHost {                   // the app gives this; headless has none
    fn context(&self, since: Option<u64>) -> ViewContext;
    fn show(&mut self, entry: &str, params: &Value) -> Result<Value, Error>;
    fn busy(&self) -> Option<String>;    // "you are dragging a part"
}

pub struct Error {
    pub code: String, pub message: String,
    pub at: Option<String>, pub rule: Option<String>,
    pub closest: Vec<String>, pub instead: Option<String>,
    pub expected_revision: Option<u64>, pub actual_revision: Option<u64>,
}
```

`Session::respond` and `Session::execute` become the implementation of `Surface::call` for a headless session. `live::admit` plus `live::run` become the window's implementation, which adds `WindowHost`.

### 3.3 A module description (what `describe {module: "layout", detail: "line"}` returns, shortened)

```json
{"module": "layout", "version": "1.3", "digest": "sha256:5c1e…",
 "summary": "Place parts, route copper, vias, fills, keepouts and the board shape.",
 "types": {"len": "mm number or text with a unit: 0.25, \"10mil\"", "id": "stable integer id", "layer": "copper layer name from project.inspect"},
 "entries": [
  {"line": "layout.move {instance:id, to:[len,len], rotation?:angle, side?:top|bottom}", "class": "edit", "summary": "Move a placed part."},
  {"line": "layout.add_via {via:{at:[len,len], net:id, …}} -> {via:id}", "class": "edit", "summary": "Add a via at a point on a net."},
  {"line": "layout.route_step {request:{…}}", "class": "edit", "time": "short", "summary": "Route one step from a pad or track end, shoving or walking round obstacles."},
  {"line": "layout.length_report {nets?:[id]} -> {rows:[…]}", "class": "read", "result": "link", "summary": "Routed length per net and pair."},
  {"line": "fill.refill {}", "class": "edit", "time": "long", "summary": "Recompute every copper fill."}
 ],
 "more": 61}
```

(The parameter shapes are illustrative; the real lines are generated from `LayoutCommand`.)

### 3.4 The MCP projection

`tools/list` on `2026-07-28` holds six tools; `apply`'s `op` is a plain string. A session:

```json
→ tools/call find {"text": "add via", "module": "layout"}
← {"hits": [{"name": "layout.add_via", "kind": "operation", "class": "edit",
             "line": "layout.add_via {via:{at:[len,len], net:id, …}} -> {via:id}", "summary": "Add a via at a point on a net."}]}

→ tools/call query {"name": "view.context", "params": {}}
← {"project": "prj_7f…", "revision": 41, "workspace": "pcb", "view": "layout",
   "selection": [{"kind": "track", "id": 412, "net": 7, "net_name": "GND"}], "layers": ["F.Cu", "B.Cu"]}

→ tools/call apply {"expected_revision": 41, "turn": "t_3a…",
                    "operations": [{"op": "layout.add_via", "via": {"at": [12.5, 8.0], "net": 7}}]}
← {"revision": 42, "changes": {"added": {"vias": [644]}}, "findings": {"new": [], "open": 3}}

→ tools/call show {"name": "view.select", "params": {"ids": [644], "frame": true}}
← {"selected": 1}

→ tools/call act {"name": "fab.export", "params": {"output": "/work/out"}}
← resultType: "input_required" (an elicitation: "Export fabrication files to /work/out?"), or in a window: waits for Allow
```

The same six definitions, byte for byte, are the built-in agent's tools. `sparksmith stdio` carries the same calls as `{"verb": "apply", …}` lines, and `sparksmith call layout.add_via '{…}' --revision 41` is the same call from a shell.

### 3.5 The plugin import and export table

```
exports   sparksmith_abi      () -> u32                      the ABI number
          sparksmith_alloc    (len: u32) -> u32              a buffer for a request, or 0
          sparksmith_describe () -> u64                      (ptr << 32) | len of the description
          sparksmith_invoke   (ptr: u32, len: u32) -> u64    status byte, then result or {"errors": […]}
          memory

imports   sparksmith.call      (ptr: u32, len: u32) -> u64   all three optional; `call` is refused with `plugin.pure` in an effect-none invoke
          sparksmith.log       (level: u32, ptr: u32, len: u32)
          sparksmith.cancelled () -> u32
```

A generator's description, in the same format, has `"capabilities": [{"name": "generate-part", "effect": "none", "params": {…}, "presets": […]}]`, `"deterministic": true` and `"needs": []`.

A plugin description:

```json
{"abi": 2, "id": "example.stitch", "version": "1.0.0", "title": "Via stitching", "licence": "MIT",
 "capabilities": [{"name": "vias", "effect": "edit", "title": "Stitch vias along tracks",
               "params": {"pitch": {"type": "length", "default": "2mm", "min": "0.5mm"},
                          "offset": {"type": "length", "default": "0.5mm"}},
               "context": ["selection"]}],
 "needs": [{"module": "project", "major": 1, "class": "read"},
           {"module": "layout", "major": 1, "minor": 2, "class": "edit"}]}
```

With the SDK a plugin author writes:

```rust
impl Command for Stitch {
    fn run(&self, cx: &mut Host, p: Params, ctx: Context) -> Result<Summary, Refusal> {
        for track in ctx.selection.tracks() {
            let t: Track = cx.query("layout.track", json!({"id": track}))?;
            for at in points_along(&t, p.pitch, p.offset) {          // integer nm
                if cx.cancelled() { return Err(Refusal::stopped()); }
                cx.apply("layout.add_via", json!({"via": {"at": at.wire(), "net": t.net}}))
                    .or_else(skip_clearance)?;                       // a refused via is skipped, not fatal
            }
        }
        Ok(Summary::text("Added vias along the selected tracks"))
    }
}
export_plugin!(Stitch);
```

### 3.6 End to end: an agent

The person selects a ground track in PCB and types, in the panel, to a 32 k local model: *"Stitch this to the bottom ground every 2 mm and show me any that failed."*

1. The panel opens a turn named after the message. The model's context holds the six verbs, the instructions (about 250 tokens, target) and the module index.
2. `query view.context` → the selection is track 412 on net GND, revision 41.
3. `find {text: "stitch vias"}` → `plugin.stitch.vias` (a command an approved plugin contributes) ranks first, `layout.add_via` second. The model picks the plugin.
4. `apply {expected_revision: 41, operations: [{op: "plugin.stitch.vias", pitch: "2mm"}]}`. The gate takes the smaller of the agent's and the plugin's grants; both allow layout edits. The plugin runs on a fork (3.7) and the host applies its 14 vias at revision 41 → 42. Reply: `changes.added.vias` with 14 ids, two new findings.
5. `query project.issues {new_since: 41}` → two vias too close to a keepout.
6. `show view.select {ids: [650, 653], frame: true}` → the window frames them. No revision change, no undo step.
7. The model answers in text. The turn closes: **one undo step, "Stitch this to the bottom ground…"**.

Had there been no plugin, step 4 would be one `apply` with 14 `layout.add_via` operations in a batch, atomically. Either way the model used nothing the UI, the CLI or an external MCP client does not have.

### 3.7 End to end: a plugin

The same command from the palette, by the person:

1. The palette lists "Stitch vias along tracks" because the plugin's hash is approved and its `needs` are met by `layout 1.3`. Its parameter form is generated from the description.
2. The host forks the engine at revision 41, instantiates the module fresh with 5 G fuel and 64 MiB, and calls `sparksmith_invoke {capability: "vias", params: {pitch: "2mm", offset: "0.5mm"}, context: {revision: 41, selection: [{kind: "track", id: 412}]}}`.
3. The module calls `call {entry: "layout.track", params: {id: 412}}` (a query on the fork), then `call {entry: "layout.add_via", …}` sixteen times. Two are refused with `layout.clearance`; the plugin skips them and goes on. A call to `fab.export` would be refused with `gate.module`: the plugin never declared it.
4. `invoke` returns Ok with a summary. The host applies the 14 successful operations to the live document at `expected_revision: 41` as one undo step, "Stitch vias along tracks". The toast offers Undo.
5. If the module traps, runs out of fuel, or the person presses Stop, the fork is dropped and nothing changed. If the person moved a part meanwhile, the run is stale: nothing changed, and the message says to run it again.

A part is made through the same export of the same ABI: `sparksmith_invoke {capability: "generate-part", params}`. Its declared effect is `none`, so the host answers no call, the plugin sees no document, and it returns canonical bytes whose hash is the result's identity.

---

## 4. Alternatives considered and rejected

### 4.1 The WebAssembly component model and WIT, instead of a custom JSON ABI

*What it offers:* typed bindings in many languages, a standard interface language, resources and handles, and it is what Solidcraft's plugin design names.

*Rejected for now, because:*
- **wasmi does not run components.** Moving to wasmtime adds a JIT (or its slower interpreter), a large dependency, and gives up the measured wasmi-in-wasmi parity that proves a browser build runs generators identically. The generator host exists, is measured and is pure Rust.
- **The catalog would be described twice.** 260 entries in JSON Schema for agents, and again in WIT for plugins, with a version story for each. The founder's request is one ABI for both; a single `call(entry, params)` import is exactly that, and agents already use it.
- **Canonical bytes are the identity.** Generators are hashed over canonical JSON. The component model's canonical ABI is a calling convention, not a canonical serialization, so the hashed form would be JSON anyway.
- **Versioning moves to the wrong place.** With typed imports every catalog change is an ABI change. With one import, the ABI is three functions that never change and the modules carry the versions.

*Kept open:* a WIT file can be generated from the catalog later for authors who want typed bindings, and the SDK already hides the wire. Solidcraft's planned import is `run(command, args)` with JSON, the same shape as `call`, so converging later is an adapter and not a redesign.

### 4.2 One generic verb, or one typed tool per command, for MCP

- **One tool per command** (today): 16.2 k tokens of fixed cost, a 64 KiB ceiling already hit, and capabilities hidden to fit. It cannot hold 260 entries, let alone plugins' commands.
- **One generic `call` tool:** smallest, but a single tool has one set of annotations, so a client cannot tell a read from a file write and would have to ask for everything or nothing.
- **One verb per class** (chosen; five approved, six proposed): each tool's annotations are exact, the fixed cost stays near 1 k tokens, and the catalog can grow without the list changing.
- The known cost is that a small model must `find` and `describe` before it can act. Line descriptions, module overviews and nearest-name refusals are the mitigation; S10 measures it on two local models before the plugin slices start.

### 4.3 The built-in agent through an MCP loopback, instead of in process

*MCP loopback* (Solidcraft's way, and mcp-surface §4's `rmcp` duplex): the panel is an MCP client of our own server.

- *For it:* parity with external clients by construction; elicitation, tasks and skills hosting come from the protocol; one fewer code path in principle.
- *Against it:* our own agent would then depend on `rmcp` (pinned exactly because it churns), a `tokio` runtime in the panel, and the stateless `2026-07-28` mechanics (sealed request state, retry rounds, subscription streams, task polling), all to cross a boundary inside one process. Every one of those exists to serve a client we do not control. The window needs `Asker`, `Progress` and `EventSink` anyway, for the live channel.
- *Chosen:* in process, on the same `Surface::call`, with the same six tool definitions and a test that the bytes are equal to MCP's. "MCP is the only AI surface" becomes "the surface is the only AI surface, and MCP is one carrier of it". ACP agents still arrive over MCP, because their client is theirs.

### 4.4 A second, wider ABI beside the catalog

A purpose-built "extension API" (document object model, getters and setters, callbacks) would be a fifth description of the same engine, with its own permissions and its own undo story. Rejected: the catalog is the ABI; everything else carries it.

### 4.5 Plugins that mutate the live document

Rejected in favour of run-on-a-fork. Direct mutation needs reentrancy rules, partial-failure rules and a way to undo half a run. A fork gives one undo step, a preview, replay and "a failed mutation changes nothing" for free, at the cost that a plugin cannot interleave with the person, which nothing needs yet.

### 4.6 Two or three kinds of plugin (generator, reader, command)

The brief started from two kinds (pure generators, automation plugins), and the earlier draft proposes three "worlds" told apart by imports. The founder decided against kinds, and on the evidence in 2.6 nothing is lost: determinism, hashing, integer geometry, the check on open, embedding, limits and trust all follow from a capability's declared effect, enforced by what one host answers during one call. A separate kind would add a second description format, a second validation path and a rule for what a plugin offering both is, and would buy only the structural "no imports" proof, which is kept as the stricter rule for plugins that declare nothing but effect `none`.

### 4.7 Raw input (click, key, drag) for agents

Solidcraft has it for test scenarios. Rejected for agents: an agent that clicks pixels bypasses every typed guarantee, and Sparksmith already tests through the accessibility harness. If a harness needs it, it is Operator-only and outside the catalog.

### 4.8 Steering the window through `act` or `apply`

See 2.5: either would make a verb's annotations untrue. A sixth verb is cheaper than a tool whose description has to explain its exceptions.

### 4.9 Against the draft critique's plan

Agreed and adopted: one registry with fields declared beside the variant; the cut-over as an early slice; a window module with no raw input; plugins run on a fork and return artifacts instead of paths; per-turn limits; a committed snapshot and a CI gate; installed plugins never embedded in projects; `settings` with an allowlist.

Different here, with the reason:

| Draft critique | This design | Why |
|---|---|---|
| Window steering through `act` with scope `window`, class edit | A `Show` class and a `show` verb | Keeps one class per verb and no revision or approval on a camera move |
| Built-in agent as an in-process MCP client | In-process calls to `Surface::call`, same tool bytes | 4.3 |
| Three WASM worlds told apart by imports | One ABI; each capability declares effect `none`, `read` or `edit` | Founder: one plugin ABI, not two types |
| Modules `0.x` until release | Majors from `1`, bumped on every break | The plugin `needs` check and recordings want a real number now |
| JSON lines keep `{"command": …}` as an alias | One envelope, no alias | No users; a second shape is what this design removes |
| `Engine::merge_steps(since)` | `begin_group` / `end_group` with a label | A group must close when the person edits in between |

---

## 5. Implementation plan

At most five agents at once. Nothing runs the desktop app on the host (VM only). Each slice ships with its tests and updates `skills/sparksmith-commands/SKILL.md`, `AGENTS.md` and the affected documents together with the enforcement it adds.

### 5.1 Slices

| # | Slice | Delivers | Acceptance | Hours | Needs |
|---|---|---|---|---|---|
| **S0** | Contract | `docs/extension-surface.md` from this design; the descriptor types (3.1) and the envelope and error types in the ABI crate; the `surface.lock` format; the module table with every existing entry assigned | The types compile for wasm32 and `no_std`. Every one of the 260 entries is listed under exactly one module in the document's table. Generator module bytes unchanged | 3–4 | — |
| **S1** | One catalog | `declare_entries!` with module, class (with `Show`), time, result, person, reach, effect; `FilePath`; the name lists, `NAMING_PATHS`, `Request::paths` and `path_samples` deleted; line descriptions generated; `cargo xtask surface --check` in CI | A variant without its declaration does not compile. No name list remains (`grep` is empty). A planted removal, a retyped field and a stricter class each fail the gate without a major bump. Every line is within 160 bytes and names the entry's required fields. Both MCP servers list the same annotations as before | 10–14 | S0 |
| **S2** | Front door | `Surface`, `Call`, `Principal`, `Grant` with per-module ceilings, `Asker`, the one `Error`; `Session::respond` reimplemented on it; `project.apply`, `apply_all`, `query`, `schema` folded into the envelope; `expected_revision` on the envelope | One decision function with a table test over every entry and every principal. A grant without `layout` refuses `layout.add_via` with `gate.module` and changes nothing. Every existing session test passes through `Surface::call`. Stale, malformed and outside-folder calls change nothing | 8–12 | S1 |
| **S2b** | Explicit context | Standards, looks, starter data and materials move from process-wide `OnceLock`s into a `Context` the session owns (critique §1) | Two sessions in one process with different standards give different results for the same operation. The test-only self-install is gone | 8–14 | — (parallel to S1) |
| **S2c** | List queries as tables | The shared scope parameters and the table reply; `layout.objects`, `schematic.items`, `library.definition {part}` | Each answers the ESP32 example's records as a table within the inline budget or a link; `count: true` returns no rows; a `bbox` and a `net` filter return exactly the records a full read filtered by hand gives; token counts recorded | 6–10 | S2; file-format F1 (declared field order) |
| **S3** | MCP cut-over | `sparksmith mcp` and `mcp --live` serve the verbs from the surface on `2026-07-28`. Person-owned steps: the window's Allow when live, `needs_person` headless. Old server, `NOT_TOOLS`, the 64 KiB test, `INSTRUCTIONS`, `live_tools`, `WINDOW_ONLY` deleted. Docs, README, Help › Connect | `tools/list` is the verbs and nothing else, within a byte budget held by a test (five measure 2,986 bytes today; six: target 3.5 KB). `generators.*` is found and callable. The golden transcripts pass. The datasheet replays grade as before. `grep NOT_TOOLS` is empty | 10–14 | S2 |
| **S4** | Turns, jobs, events | `Engine::begin_group`/`end_group`; core `Progress`; job handles for long entries; `EventSink` with coalescing; events on JSON lines and the live channel; per-turn limits | Five applies in a turn undo as one step with the turn's label; a person's edit in between splits it. Cancelling a refill and an export leaves the document byte-identical. A subscriber sees coalesced `project.changed` and the final revision. A turn past its limit stops with `gate.turn_limit` | 12–18 | S2 |
| **S5** | The window | `WindowHost`; `view.context`, `view.select`, `view.open` (workspace, sheet, 2D/3D), `view.layers`, `view.camera`, `view.screenshot`; `files.new`/`open`/`save` through the window's prompts; the UI's own view commands call the same functions; `show` verb | In the VM, over the live channel: select R5 by id and the inspector shows R5; frame a net in 3D; none of these changes the revision, adds an undo step or rebuilds geometry (asserted by counters). Refused during a drag. Headless: `host.unavailable`. The palette and the agent run `view.open` through one function | 12–18 | S2 |
| **S6** | Built-in agent on the surface | `crates/assist` calls `Surface::call` with the verb definitions; `assist.*` entries; profiles as data; leniency in the verb reader; the tool list deleted | Tool definitions equal MCP's bytes (test). The 16 recordings and the TLV replays reach the same grades. A profile test shows the datasheet run cannot reach publish, files or settings. `assist/tools.rs` has no `definitions` | 10–16 | S3 |
| **S7** | CLI | `sparksmith call`, `find`, `describe`; `stdio` takes the envelope and verb lines and emits event lines; old subcommands become aliases or go | `sparksmith call fab.export '{"output": …}'` writes the same files as `sparksmith fab`. `sparksmith describe layout` prints the module's lines. The shell completions come from the catalog | 4–6 | S2 |
| **S8** | One plugin ABI | One description format with capabilities, effects and `needs`; the four exports renamed; the null host for effect `none`; the one error shape; `Principal::Plugin`; approve by bundle hash, revoke, per-user store (the store GP9's UI uses); the chip generator is the first plugin | Done inside the part-generation plan's B slices, so modules re-lock once. A module that calls `call` during `generate-part` gets `plugin.pure` and gives one result hash natively, under wasmi and under the probe. A plugin declaring only effect `none` with any import is refused. A changed bundle asks again and shows what changed. A plugin needing `layout 2` on `layout 1` is refused before it runs, naming both. The chip goldens change only by the planned B changes | 8–12 | S1; with B1–B4 |
| **S9a** | Effect `read` | The `call` import answered on a frozen fork; `pure` and `revision` queries only; artifacts saved through Ask; SDK; one shipped example (a BOM format) | Runs within fuel, memory and wall limits. Same module, parameters and revision give the same result natively, under wasmi and under the wasm32 probe. A call to a `world` entry or outside the grant is refused and the run ends cleanly. Limits set from measurement and recorded | 10–14 | S8, S2 |
| **S9b** | Effect `edit` | Operations on the fork; impact preview; one apply at R; stale handling; palette and form from the description; one shipped example (via stitching) | One undo step named after the command. A trap, fuel exhaustion and Stop each leave the document byte-identical. An edit during the run makes it stale and changes nothing. A recorded run replays to the same operations | 10–16 | S9a, S4 |
| **S10** | Evidence | Token and byte budgets per module as tests; a small local model and Claude Code each complete a scripted task through the verbs including one `show`; property tests and a fuzzer on the envelope decoder and the plugin host; generated per-module reference | A measured table in the repository replaces every "target" in this document. The founder's small-model requirement is met or the gap is named | 5–8 | S3–S6 |

**Totals.** S0–S7 and S10: about 82–124 hours, plus S2c at 6–10. S8–S9: about 28–42 (S8 is 8–12 of it and belongs with part-generation B; porting the chip module to fixed point, if decision 4 is yes, is extra). The MCP follow-ups that stay as designed in mcp-surface, now built on the new server: P3 results and links, P4 catalog resources, skills and prompts, P5 elicitation, P11 completions, P12 review views: about 22–32 by that document's estimates. P6–P8 are covered by S4.

### 5.2 Order, with the other plans

| Round | This plan | Part generation | File format | Notes |
|---|---|---|---|---|
| 1 | **S0** | A1, A2 + A3 (GP2 redirected to part files), merge the regen branch | F0 version discipline | Contract first. Three or four agents |
| 2 | **S1**, **S2b** | B1–B4 (close the ABI), with **S8** riding B4 | F1 contract | S1 touches the command enums' declarations only; B touches the ABI crate and `generated`. The ABI crate is shared by S0/S8 and B: one agent owns it per round |
| 3 | **S2**, then **S3** and **S5** side by side, **S7** | — | F2 onward, between B and C as that plan asks | The cut-over lands here. It unblocks D3 (GP7) |
| 4 | **S4**, **S6**; P3–P5 | C: the families (GP3, GP4), five wide for three rounds | — | C wants all five agents with nothing else on library code. S4 and S6 do not touch library code, but the five-agent limit holds: either they finish before C starts, or C runs four wide |
| 5 | **S10** | D1, D2 (GP5: the host switch, calling `generators.*`; in-code templates deleted), D3 (GP7), GP6 (the `module` authoring start and the authored USB connector part) | — | GP7 becomes entries, not tools: list, describe, preview, generate, regenerate and detach are `generators.*` entries after S3, and its "compact describe for small models" is the line form of 2.2 |
| 6 | **S9a**, **S9b** | GP8 (expansions in a cache), GP9 (third-party plugins: check and test CLI, author guide, approve-by-hash UI on S8's store, no run on open until approved, nondeterministic modules rejected) | — | S9 only after S10's evidence. GP9 and S9 share the approval dialog and the author guide: one guide for one ABI. Nothing is announced before B is closed |

**Interactions to hold to:**
- **Units (U3).** Independent. The command wire keeps mm and unit text; S1's line vocabulary says `len`. Add `nm` as a length unit in S0.
- **File format.** Nothing here changes a stored byte except through S8's provenance fields (plugin id, version, bundle and module hash, ABI number), which belong to B and bump the file version under F0's rule. Approvals, grants and caches are per-user and outside the tree.
- **The canonical codec stays in the ABI crate** and is not moved, so module bytes change only when B re-locks them.
- **Check on open:** refusing a project on a mismatch stays, as the founder decided. This design does not need open-with-flag.

---

## 6. Risks

| Risk | Mitigation |
|---|---|
| **Generic verbs are harder for small models than named tools**, and `op` is a plain string by decision | Line descriptions, module overviews under a byte budget, nearest-name refusals, argument leniency for every client. S10 measures two local models **before** S9 starts, and its result gates the plugin slices |
| **The catalog becomes a second source again** when a field is added to a list under deadline | The macro refuses to compile without the fields; the lists are deleted in S1, not later; the xtask gate |
| **The cut-over lands late and blocks the rest** | S0, S1, S2b, S5's host trait and S8 do not depend on it. S3 is scoped to "serve the verbs and delete", with elicitation, links and skills as follow-ups |
| **The window moving under the person's hands** | Busy refusal, the person's input always wins, a preference for remote agents, no raw input, per-turn limits |
| **Plugins as an attack surface** | Three imports; no paths, clock, network or window; approval per hash; grants per user; nothing but generators runs from a project; fuel, memory, wall time and call counts; the host fuzzed in S10 |
| **Freezing an ABI too early** | Every break is visible in `surface.lock` and the major; the plugin slices come last; nothing is announced before the part-generation plan's B is closed and S9b has a second real example |
| **Fork cost on large boards**, paid by every dry run and plugin run | `fork()` shares sections (code). Measure on the dense benchmark board in S9a. The per-operation whole-document followers (critique §1) are the real cost and are not this plan's to fix |
| **Renaming 260 wire names by accident** | S1 assigns modules without renaming; a rename is a major bump that the gate forces to be deliberate. Recordings carry the catalog digest and are refused clearly against another |
| **Velocity and overlap with other agents** (more than thirty active in this session against a limit of five) | S0 is a contract slice; the ABI crate and `session.rs` each have one owner per round |
| **`rmcp` and protocol churn** | The surface does not depend on MCP; only S3's carrier does, pinned exactly |

**Not verified.** I did not build or run anything. The count of UI commands (about 50) is from a `grep`. The definitions of GP6 to GP9 used here are the lead's, from the original generator plan; they are not in the repository. `Config::floats` exists in the pinned wasmi 2.0.0 (read in its source); that every module the SDK builds stays free of float opcodes once the SDK avoids them is not checked. The entries `layout.track` and `project.issues {new_since}` and the parameter shapes in section 3's examples are illustrative, not read from the enums.

---

## 7. Decisions for the founder

Each with my recommendation.

1. **A sixth verb, `show`, for the window.** *Recommendation: yes.* Selection and camera are neither reads, edits nor file actions, and a verb per class keeps every tool's annotations true. The alternative is to steer the window through `act` and explain the exception in its description.
2. **The built-in agent calls the surface in process, not through an MCP loopback.** This changes the approved mcp-surface §4 for the endpoint loop only. *Recommendation: yes*, with a test that its tool definitions are MCP's bytes. ACP agents still connect over MCP.
3. **One plugin ABI, with purity as a capability's declared effect (`none`, `read`, `edit`) enforced by the host per call.** *Recommendation: yes, and no second kind.* I found no need that requires one (2.6). A generator is a plugin whose `generate-part` has effect `none`; one plugin may offer a part and a command.
4. **Refuse float opcodes in plugins that offer hashed results.** *Recommendation: yes, beside the part-generation plan's B3*, knowing it costs more than B3's 6–10 hours: the chip module and the SDK are ported to fixed point (my guess is 6–10 hours more; not measured). If that proves costly, keep floats and rely on integer-only output; the ABI is the same either way.
5. **A capability with effect `edit` works on a fork; the host applies** as one undo step. *Recommendation: yes.* The cost is that a plugin cannot interleave with the person.
6. **When to make the `call` import answer (S8 with B, then S9).** *Recommendation: S8 now with B, because it settles the one description format and the exports while modules re-lock anyway, so the ABI is never two things; S9, which only makes the `read` and `edit` effects answer, after S10 shows the surface works for agents.* An agent already covers much of what a command plugin does, with no ABI to maintain.
7. **Custom JSON ABI on wasmi, not the component model.** *Recommendation: yes*, with a generated WIT file later if authors ask. Solidcraft's planned `run(command, args)` import has the same shape, so the two can converge.
8. **May agents steer the view by default?** *Recommendation: on for the built-in agent; off for external MCP clients until the person turns it on in Connect AI assistants.* The person's input always wins.
9. **May an agent open, save or start a project in the open window?** Today: refused outright. *Recommendation: allow as Ask, through the window's own prompts*, never silently.
10. **May an agent change settings?** *Recommendation: read the non-secret ones; set an allowlist (theme, units shown, grid, shortcuts) with the person's Allow; never keys, endpoints, permission modes, plugin approvals or grants.* This is the one place to hold against "control the whole program".
11. **Where approvals and installed plugins live.** *Recommendation: per user, outside any project; generators embedded in projects as today; no other plugin ever runs from a project.*
12. **Module versions start at `1.0` at the cut-over, with `surface.lock` enforced.** *Recommendation: yes.* Breaking stays free until the first release, but every break is numbered.
13. **Update the hard rule in AGENTS.md** from "one typed command engine shared by UI, CLI and MCP" to name the built-in agent and plugins, and add `cargo xtask surface` to the gates, in the slice that builds each (S1, S2, S6, S8).
14. **Later, its own decision: mount Solidcraft's command catalog as a module** (`solidcraft.*`, discovered at run time from its registry) so an agent can author a part file through the same verbs. The part-generation plan raises the same route (its decision 6). *Recommendation: after D, 6–10 hours, only with Solidcraft unchanged.*
