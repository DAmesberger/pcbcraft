# Pcbcraft

A Rust electronics editor using Solidcraft's egui/wgpu UI and native parametric engine. Component authoring, a basic single-sheet schematic and PCB layout now share one desktop shell. PCB 3D displays physical copper and package envelopes through Solidcraft, with shared selection and whole-net highlighting.

This is an early working editor foundation, not a manufacturing-ready release. See [current evidence and limits](docs/editor-evidence.md).

## Run

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
| `pcbcraft-automation` | Headless `Session` (engine + open/save/export) shared by the JSON-lines transport and the MCP server. |
| `pcbcraft-assist` | The datasheet assistant: a Claude tool-use loop over the automation session that drafts a part from its datasheets, with a hard cost cap ([as built](docs/datasheet-assistant.md)). |
| `pcbcraft`, `pcbcraft-cli` | Desktop editor and command line. |

`cargo xtask layers` enforces this direction; `cargo xtask ui-containers` rejects raw egui windows, areas and panels outside Solidcraft's `ui_kit::Container`. `cargo xtask core-modules` enforces the core module map in [component model](docs/component-model.md). `cargo xtask licences --check` rejects dependencies that require GPL-family or unreviewed licenses, and `cargo xtask clean-room` rejects EDA library files and data in the repository. All of these run in `cargo xtask ci`.

## CLI, JSON lines and MCP

```sh
cargo run --locked --profile ci -p pcbcraft-cli -- demo example.pcbcraft
cargo run --locked --profile ci -p pcbcraft-cli -- showcase showcase.pcbcraft   # a routed 4-layer board with via examples; open it in the app
cargo run --locked --profile ci -p pcbcraft-cli -- parts parts.pcbcraft   # 24 invented EX- parts for the Place palette
cargo run --locked --profile ci -p pcbcraft-cli -- sample-library NEW_DIR [NAME]   # the same catalog as a library folder
cargo run --locked --profile ci -p pcbcraft-cli -- export example.pcbcraft example.solidcraft
cargo run --locked --profile ci -p pcbcraft-cli -- export-schematic-pdf example.pcbcraft example.pdf [--sheet ID]...   # vector PDF, one page per sheet
cargo run --locked --profile ci -p pcbcraft-cli -- commands
cargo run --locked --profile ci -p pcbcraft-cli -- stdio [FILE.pcbcraft]
cargo run --locked --profile ci -p pcbcraft-cli -- mcp [FILE.pcbcraft]
ANTHROPIC_API_KEY=… cargo run --locked --profile ci -p pcbcraft-cli -- design-part \
  --datasheet PART.pdf --name PART --out part.pcbcraft --max-usd 15   # drafts + PART.review.md; publishes nothing
```

`stdio` reads one JSON request per line (4 MiB maximum) and answers `{"ok":true,"result":…}` or `{"ok":false,"error":{"code":…,"message":…}}`. `project.apply` takes the same typed operations as the UI with an expected revision; `project.inspect`, `.undo`, `.redo`, `.new`, `.open`, `.save`, `.preview_update`, `.apply_update` and `.export_native` share the session. `mcp` serves the same requests as MCP tools (`project_apply`, …) over stdio. `commands` prints schemas generated from the Rust types.

`project.query` also answers the parts catalog ([design and query language](docs/parts-library.md)): `library.search` ranks the project's component lineages for a one-line query, `library.facets` lists the fields, packages and badges the matches carry, and `library.inspect` reports one revision. For example:

```json
{"command":"project.query","params":{"query":"library.search","params":{"query":"res>=10k pkg:0603","limit":10}}}
```

Field keys come from the project's field names and units, so a query reports `V>=10k: 10 k has no unit of voltage` rather than guessing.

Library folders ([S3](docs/parts-library.md#s3-as-built)) are plain directories of immutable revision files, shareable through git. `library.source.add` lists one in the user's settings (`PCBCRAFT_CONFIG_DIR`, `PCBCRAFT_CACHE_DIR` override the platform directories); `library.search` then includes its entries, `library.place_from_folder` imports and places in one undo step and `library.publish_to_folder` promotes a project revision to a writable folder. Imported revisions keep their origin and attached copies, so projects reopen without the folder.

## Scripting integration proof

The core contains a bounded Rhai integration for unit-aware expressions and draft pad-array generation. Units are request data; the interpreter has no component-specific property schema. `library.generate_pads` (on a footprint draft) is available inside `project.apply` through CLI/MCP, with ordinary revision checks and undo. This is a headless feasibility slice, not a new component editor or a complete library scripting format. See the [contract and limits](docs/part-library-design.md#rhai-integration-proof).

Native scripting tests run in `cargo xtask ci`. To also execute the seven scripting tests, and the footprint evaluator's golden-hash tests (native/wasm agreement), as WebAssembly under Node (rather than only compile wasm), install Node and the test runner matching the locked `wasm-bindgen` version:

```sh
cargo install wasm-bindgen-cli --version 0.2.129 --locked
cargo xtask wasm-scripting
cargo xtask wasm-footprint
```

Run these inside `nix develop` when using the Nix toolchain. The runner must be on `PATH`, or set `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER` to its executable path. This optional runtime check is separate from `cargo xtask ci`; it does not exercise browser UI.

Solidcraft and its required patches are pinned to `d53b131e3d1cf0b9489e9c048c60af787b14cc47` of the [DAmesberger/solidcraft](https://github.com/DAmesberger/solidcraft) fork (branch `pcbcraft-via2`): upstream Solidcraft `4f3ac07` plus an opt-in real-time "product look" (studio environment, clear coats, surface patterns, ambient occlusion, PBR Neutral tone mapping) that the PCB 3D view uses, and via scaling (indexed profile nesting, batched copies and joints, instanced display, deferred evaluation); Solidcraft's own app is unchanged by default. Builds use git dependencies and the lockfile; a neighboring Solidcraft checkout is not required. The native probe and copper sample (vias, zone with hole, same-net island) are acceptance tests in `pcbcraft-solidcraft`, not product features. UI source references and font licenses are recorded in [attribution](ATTRIBUTION.md).

## Scope and design

- [Product plan](docs/pcbcraft-plan.md)
- [Part library design](docs/part-library-design.md)
- [Parts library: catalog, search and slice plan](docs/parts-library.md)
- [Right sidebar and feedback: information architecture](docs/sidebar-ia.md)
- [Component-editor acceptance scenarios](docs/part-library-design.md#component-editor-acceptance-scenarios), including multi-unit symbols and structured multi-function IOs
- [Native integration research](docs/solidcraft-interop-research.md)
- [Copper performance research](docs/copper-performance-research.md)
- [Roadmap](ROADMAP.md) and [agent instructions](AGENTS.md)

The Route tool shoves other nets' tracks and vias aside, walks round obstacles or ignores them (M cycles the mode); each click is one atomic transaction and undo step, also as `layout.route_step` for the CLI and MCP ([push-and-shove](docs/push-and-shove.md)). Dragging existing tracks with shove is planned. Large-board performance, incremental native rebuilds, arbitrary Solidcraft model composition and manufacturing outputs remain future work.

## License

Pcbcraft is licensed under the [MIT License](LICENSE-MIT) or the [Apache License, Version 2.0](LICENSE-APACHE), at your option, like Solidcraft. Third-party notices are in [NOTICE](NOTICE) and [ATTRIBUTION.md](ATTRIBUTION.md). Pcbcraft is an independent clean-room implementation; it does not use code or library data from GPL-family or commercial EDA tools.
