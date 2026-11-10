# Development

How Pcbcraft is built, checked and driven from the command line. The [README](../README.md) describes the product; this page is for contributors and for anyone scripting it.

PCB design software in Rust, using Solidcraft's egui/wgpu UI and native parametric engine. Component authoring, multi-sheet schematics, PCB layout and production output share one desktop shell. PCB 3D displays physical copper and package models through Solidcraft, with shared selection and whole-net highlighting.

This is an early working editor foundation, not a manufacturing-ready release. See [current evidence and limits](editor-evidence.md).

## Build and run

`rust-toolchain.toml` pins Rust 1.96.0, Clippy, rustfmt and the wasm target. Use rustup, or the Nix dev shell, which provides exactly that toolchain:

```sh
nix develop          # optional: pinned toolchain without rustup
cargo run --locked --profile ci -p pcbcraft [FILE.pcbcraft]
cargo xtask ci
```

The app starts with two connected resistor instances. Component authoring includes Symbol, Footprint, Mapping and 3D modes; each finished edit of the five component drafts (component, symbol, package, footprint, binding) is one undo step, and Publish creates immutable project revisions. In Schematic, Place (or P) opens the Place palette: type a query such as `10k 0603`, `res>=10k` or `tol<=1%`, move with ↑/↓, press Enter to place (Shift+Enter keeps the palette open) or click a row to place it with the mouse; the properties card previews the highlighted part. Connect terminals with Wire and rename a selected wire's net. The Browser's Components section has a filter field that uses the same search. In PCB, preview/apply Update PCB, move placements and route between pads. Switch between 2D and 3D within PCB; selection and whole-net highlighting are shared.

File offers New, Open, Save, Save as and Export to Solidcraft through native dialogs; reads, writes and exports run in the background. Unsaved work is snapshotted for crash recovery and offered at the next launch. Project files contain the revisioned library snapshot and the definition drafts, so they reopen offline. Native export writes a new parametric `.solidcraft` file; package models are clearly identified envelopes.

Navigation: scroll zooms; middle-drag pans; right-drag orbits in 3D. Cmd/Ctrl+S saves, +Shift+S saves as, +O opens, +N starts a new project, +Z/+Shift+Z undo and redo.

## Architecture

| Crate | Responsibility |
| --- | --- |
| `pcbcraft-core` | Pure, wasm-compatible document: revisioned library definitions and drafts, component instances and symbol occurrences, schematic, persistent nets, layout, impact plans, completeness findings, typed `Operation`s and `Query`s, the one `Engine` (shared undo, engine-owned drafts), file format, typed errors and check findings. |
| `pcbcraft-io` | Bounded reads, atomic writes, crash recovery. |
| `pcbcraft-solidcraft` | Builds the native Solidcraft model through a typed facade over Solidcraft's command API. |
| `pcbcraft-firmware` | Pin-configuration exports from the core's pin report (JSON, C header, Zephyr overlay skeleton, Rust constants); pure text, beside `pcbcraft-io`. |
| `pcbcraft-automation` | Headless `Session` (engine + open/save/export) shared by the JSON-lines transport and the MCP server, and the live channel into the open window ([live MCP](#live-mcp)). |
| `pcbcraft-assist` | The datasheet assistant: a tool loop over the automation session that drafts a part from its datasheets, on a model you configure (Anthropic Messages or any OpenAI-compatible endpoint, hosted or local), with token, time and request caps ([as built](datasheet-assistant.md)). |
| `pcbcraft`, `pcbcraft-cli` | Desktop editor and command line. |

`cargo xtask layers` enforces this direction; `cargo xtask ui-containers` rejects raw egui windows, areas and panels outside Solidcraft's `ui_kit::Container`. `cargo xtask core-modules` enforces the core module map in [component model](component-model.md), and `cargo xtask connectivity` that only `layout/connectivity` decides whether copper joins ([copper graph](copper-graph.md#where-connectivity-lives)). `cargo xtask f64-lengths` rejects new serialized `f64` length fields in the core's `model`, `library` and `geometry` modules: stored lengths are `units::Nm`; its allow-list holds the fields the units migration has yet to convert, and only shrinks. Each migration slice changes the one file version, so `cargo xtask convert-v1 FILE…` (not part of the app) rewrites existing `.pcbcraft` files in place, keeping the original as `FILE.pre-nm`. `cargo xtask licences --check` rejects dependencies that require GPL-family or unreviewed licenses, and `cargo xtask clean-room` rejects EDA library files and data in the repository. All of these run in `cargo xtask ci`.

## CLI, JSON lines and MCP

```sh
cargo run --locked --profile ci -p pcbcraft-cli -- demo example.pcbcraft
cargo run --locked --profile ci -p pcbcraft-cli -- showcase showcase.pcbcraft   # a routed 4-layer board with via examples; open it in the app
cargo run --locked --profile ci -p pcbcraft-cli -- parts parts.pcbcraft   # 24 invented EX- parts for the Place palette
cargo run --locked --profile ci -p pcbcraft-cli -- sample-library NEW_DIR [NAME]   # the same catalog as a library folder
cargo run --locked --profile ci -p pcbcraft-cli -- export example.pcbcraft example.solidcraft
cargo run --locked --profile ci -p pcbcraft-cli -- fab example.pcbcraft out/   # production check, then Gerber X2, Excellon, job, pick-and-place, BOM and PDF drawings
cargo run --locked --profile ci -p pcbcraft-cli -- export-schematic-pdf example.pcbcraft example.pdf [--sheet ID]...   # vector PDF, one page per sheet
cargo run --locked --profile ci -p pcbcraft-cli -- commands
cargo run --locked --profile ci -p pcbcraft-cli -- stdio [--permission-mode MODE] [--allow-folder DIR]... [FILE.pcbcraft]
cargo run --locked --profile ci -p pcbcraft-cli -- stdio --operator [FILE.pcbcraft]
cargo run --locked --profile ci -p pcbcraft-cli -- mcp [--permission-mode MODE] [--allow-folder DIR]... [FILE.pcbcraft]
cargo run --locked --profile ci -p pcbcraft-cli -- mcp --live [--port PORT]   # the project open in the desktop window
ANTHROPIC_API_KEY=… cargo run --locked --profile ci -p pcbcraft-cli -- design-part \
  --datasheet PART.pdf --name PART --out part.pcbcraft   # drafts + PART.review.md; publishes nothing
cargo run --locked --profile ci -p pcbcraft-cli -- design-part --provider openai \
  --base-url http://localhost:11434/v1 --model MODEL --context-tokens N --vision yes \
  --datasheet PART.pdf --name PART --out part.pcbcraft   # the same on a local model
```

`stdio` reads one JSON request per line (4 MiB maximum) and answers `{"ok":true,"result":…}` or `{"ok":false,"error":{"code":…,"message":…}}`. `project.apply` takes the same typed operations as the UI with an expected revision; `project.inspect`, `.undo`, `.redo`, `.new`, `.open`, `.save`, `.preview_update`, `.apply_update` and `.export_native` share the session. `mcp` serves the same requests as MCP tools (`project_apply`, …) over stdio. `commands` prints schemas generated from the Rust types.

Replies on both are compact: a mutation answers with `revision`, `outcome`, `changes` (added, changed and removed stable ids by collection, such as `{"added":{"vias":[644]}}`), `findings` (the new ones, resolved counts per code, the open total) and the changed `drafts`. `project.inspect` answers with counts, drafts and findings per code, and with `at` (a JSON pointer such as `/project/nets`) reads one piece of the document. `verbose: true`, beside `command` or inside `params`, returns the whole snapshot instead. In MCP, `tools/list` names every operation and query, and `project_schema {name}` (`project.schema`) returns one's exact schema and how to send it. `prompts/list` offers the workflows: `datasheet_to_component` (arguments `part`, `datasheets`, `notes`) is the [datasheet-to-component workflow](datasheet-assistant.md#mcp-workflow), served from `skills/pcbcraft-datasheet-part/SKILL.md`.

Every command has a permission class: read, edit, ask or never, shown in its tool description and in `project_schema`. `Session::respond` enforces the class for every model-facing transport (headless MCP, live MCP and `stdio`): the never class is always refused, and so is a path outside the run's folders. `mcp` and `stdio` are gated alike, because Pcbcraft cannot tell a script from a model; `--permission-mode` chooses how: `ask-every-time` runs only reads, `auto-allow-edits` (the default) also runs edits, and `allow-all` runs everything except the never class. Neither has a way to ask, so whatever would need asking is refused with a message saying so; `--permission-mode allow-all` is the operator's explicit choice to let ask-class requests run. Under any mode, a path must be absolute and inside an `--allow-folder` or the folder of the project named on the command line. `stdio --operator` says you are running it yourself, not a model: every class runs, including library sources and settings, with any path, as at the command line. It takes no mode or folders, and `mcp` does not accept it. Your own replays use it (`examples/esp32-s31-devboard/regenerate.sh`).

### Live MCP

An assistant can also edit the project open in the desktop window, so you watch each change and share one undo history. In Pcbcraft, choose **Help › Connect AI assistants…** (also in the in-window File menu and the command palette) and turn on **Let AI assistants edit this project**. It is off at every start. The window shows the configuration to copy:

```json
{"mcpServers": {"pcbcraft": {"command": "/path/to/pcbcraft-cli", "args": ["mcp", "--live"]}}}
```

For Claude Code: `claude mcp add pcbcraft -- /path/to/pcbcraft-cli mcp --live`.

- **Connection.** The window listens on `127.0.0.1` only, from port 27979 (or the next free one), using Solidcraft's control channel. Each time you turn it on, it makes a new token and writes it to `control/PORT.token` in the Pcbcraft config folder, readable only by you; `control/current.port` names the port. `pcbcraft-cli mcp --live [--port PORT]` reads both, so the configuration holds no secret. `PCBCRAFT_CONTROL_TOKEN` gives a token explicitly. Turning access off, or closing the window, closes every connection and removes the files.
- **Same engine.** Requests run on the UI thread, between frames, through the same `Session::respond` as the headless server, on the window's engine. The tools are the same, except that `project_new`, `project_open` and `project_save` are left to you in the window. Replies stay compact: a via placed on the demo answers in under 4 KB.
- **Revisions.** Every tool that changes the project takes `expected_revision`, inside its parameters or, for tools such as `project_undo` that have none, beside them. A stale revision is refused with `stale_revision`, so the assistant inspects again.
- **Refusals.** While you type in a field, drag, or have a menu, the command palette, a file question or an update review open, a change is refused with `live.busy` and the reason. Reads still answer.
- **Permissions.** The window's choice (reading only; reading and edits, the default; or everything except settings) decides as `--permission-mode` does. An ask-class tool, such as an export, waits for **Allow** or **Deny** in the window; the never class is refused. Paths must lie in your home folder, the temporary folder or the project's folder.
- **What you see.** The application bar shows *AI access on* or *AI assistant connected* while access is on; its × turns access off. Each assistant edit shows as a toast with Undo and a Messages line (*AI assistant edit: layout.add_via*), and undoing one says *Undid AI assistant edit*.

`project.query` also answers the parts catalog ([design and query language](parts-library.md)): `library.search` ranks the project's component lineages for a one-line query, `library.facets` lists the fields, packages and badges the matches carry, and `library.inspect` reports one revision. For example:

```json
{"command":"project.query","params":{"query":"library.search","params":{"query":"res>=10k pkg:0603","limit":10}}}
```

Field keys come from the project's field names and units, so a query reports `V>=10k: 10 k has no unit of voltage` rather than guessing.

Library folders ([S3](parts-library.md#s3-as-built)) are plain directories of immutable revision files, shareable through git. `library.source.add` lists one in the user's settings (`PCBCRAFT_CONFIG_DIR`, `PCBCRAFT_CACHE_DIR` override the platform directories); `library.search` then includes its entries, `library.place_from_folder` imports and places in one undo step and `library.publish_to_folder` promotes a project revision to a writable folder. Imported revisions keep their origin and attached copies, so projects reopen without the folder. `library.import_from_project` copies one published revision and its closure from another `.pcbcraft` file, as one undo step.

## Scripting integration proof

The core contains a bounded Rhai integration for unit-aware expressions and draft pad-array generation. Units are request data; the interpreter has no component-specific property schema. `library.generate_pads` (on a footprint draft) is available inside `project.apply` through CLI/MCP, with ordinary revision checks and undo. This is a headless feasibility slice, not a new component editor or a complete library scripting format. See the [contract and limits](part-library-design.md#rhai-integration-proof).

Native scripting tests run in `cargo xtask ci`. To also execute the seven scripting tests, and the footprint evaluator's golden-hash tests (native/wasm agreement), as WebAssembly under Node (rather than only compile wasm), install Node and the test runner matching the locked `wasm-bindgen` version:

```sh
cargo install wasm-bindgen-cli --version 0.2.129 --locked
cargo xtask wasm-scripting
cargo xtask wasm-footprint
```

Run these inside `nix develop` when using the Nix toolchain. The runner must be on `PATH`, or set `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER` to its executable path. This optional runtime check is separate from `cargo xtask ci`; it does not exercise browser UI.

Solidcraft and its required patches are pinned to `64bebfef0a77dab8c79666114c7601c9dda55d12` of the [DAmesberger/solidcraft](https://github.com/DAmesberger/solidcraft) fork (branch `pcbcraft-smlib-open`): upstream Solidcraft `a34756830` with its SMLib kernel (`solidcore`, vendored in Solidcraft), plus material graphs on upstream's shared viewport renderer with an instanced graph draw, which the PCB 3D view uses for the board's look, and via scaling (indexed profile nesting, batched copies and joints, instanced display, deferred evaluation); Solidcraft's own app is unchanged by default, except that bundled symbols now draw ahead of the system language fonts. Builds use git dependencies and the lockfile; a neighboring Solidcraft checkout is not required. The native probe and copper sample (vias, zone with hole, same-net island) are acceptance tests in `pcbcraft-solidcraft`, not product features. UI source references and font licenses are recorded in [attribution](../ATTRIBUTION.md).

## Scope and design

- [Product plan](pcbcraft-plan.md)
- [Part library design](part-library-design.md)
- [Parts library: catalog, search and slice plan](parts-library.md)
- [Right sidebar and feedback: information architecture](sidebar-ia.md)
- [Component-editor acceptance scenarios](part-library-design.md#component-editor-acceptance-scenarios), including multi-unit symbols and structured multi-function IOs
- [Native integration research](solidcraft-interop-research.md)
- [Copper performance research](copper-performance-research.md)
- [Fabrication output](fabrication-output.md)
- [LLM integration](llm-integration.md)
- [Roadmap](../ROADMAP.md) and [agent instructions](../AGENTS.md)

The Route tool shoves other nets' tracks and vias aside, walks round obstacles or ignores them (M cycles the mode); each click is one atomic transaction and undo step, also as `layout.route_step` for the CLI and MCP ([push-and-shove](push-and-shove.md)). Select drags an existing track segment with shove, also as `layout.track_drag`. Production output is built ([fabrication output](fabrication-output.md)). Large-board performance, incremental native rebuilds and arbitrary Solidcraft model composition remain future work.
