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

The app starts with two connected resistor instances. Component authoring includes Symbol, Footprint, Mapping and 3D modes; each finished edit of the component draft is one undo step, and Publish creates an immutable project snapshot. In Schematic, choose a snapshot and Place part, connect terminals with Wire, and rename a selected wire's net. In PCB, preview/apply Update PCB, move placements and route between pads. Switch between 2D and 3D within PCB; selection and whole-net highlighting are shared.

File offers New, Open, Save, Save as and Export to Solidcraft through native dialogs; reads, writes and exports run in the background. Unsaved work is snapshotted for crash recovery and offered at the next launch. Project files contain the exact component snapshots and the component draft, so they reopen offline. Native export writes a new parametric `.solidcraft` file; package models are clearly identified envelopes.

Navigation: scroll zooms; middle-drag pans; right-drag orbits in 3D. Cmd/Ctrl+S saves, +Shift+S saves as, +O opens, +N starts a new project, +Z/+Shift+Z undo and redo.

## Architecture

| Crate | Responsibility |
| --- | --- |
| `pcbcraft-core` | Pure, wasm-compatible document: parts, schematic, persistent nets, layout, impact plans, typed `Operation`s, the one `Engine` (shared undo, engine-owned draft), file format, typed errors and check findings. |
| `pcbcraft-io` | Bounded reads, atomic writes, crash recovery. |
| `pcbcraft-solidcraft` | Builds the native Solidcraft model through a typed facade over Solidcraft's command API. |
| `pcbcraft-automation` | Headless `Session` (engine + open/save/export) shared by the JSON-lines transport and the MCP server. |
| `pcbcraft`, `pcbcraft-cli` | Desktop editor and command line. |

`cargo xtask layers` enforces this direction; `cargo xtask ui-containers` rejects raw egui windows, areas and panels outside Solidcraft's `ui_kit::Container`.

## CLI, JSON lines and MCP

```sh
cargo run --locked --profile ci -p pcbcraft-cli -- demo example.pcbcraft
cargo run --locked --profile ci -p pcbcraft-cli -- export example.pcbcraft example.solidcraft
cargo run --locked --profile ci -p pcbcraft-cli -- commands
cargo run --locked --profile ci -p pcbcraft-cli -- stdio [FILE.pcbcraft]
cargo run --locked --profile ci -p pcbcraft-cli -- mcp [FILE.pcbcraft]
```

`stdio` reads one JSON request per line (4 MiB maximum) and answers `{"ok":true,"result":…}` or `{"ok":false,"error":{"code":…,"message":…}}`. `project.apply` takes the same typed operations as the UI with an expected revision; `project.inspect`, `.undo`, `.redo`, `.new`, `.open`, `.save`, `.preview_update`, `.apply_update` and `.export_native` share the session. `mcp` serves the same requests as MCP tools (`project_apply`, …) over stdio. `commands` prints schemas generated from the Rust types.

Solidcraft and its required patches are pinned to `4f3ac07aec69489183185071f07d7fbcf4f51e64`. Builds use git dependencies and the lockfile; a neighboring Solidcraft checkout is not required. The native probe and copper sample (vias, zone with hole, same-net island) are acceptance tests in `pcbcraft-solidcraft`, not product features. UI source references and font licenses are recorded in [attribution](ATTRIBUTION.md).

## Scope and design

- [Product plan](docs/pcbcraft-plan.md)
- [Part library design](docs/part-library-design.md)
- [Native integration research](docs/solidcraft-interop-research.md)
- [Copper performance research](docs/copper-performance-research.md)
- [Roadmap](ROADMAP.md) and [agent instructions](AGENTS.md)

Push-and-shove remains a release-2 requirement if absent from release 1. Manual routing already passes through an atomic route edit set and strategy interface. Large-board performance, incremental native rebuilds, arbitrary Solidcraft model composition and manufacturing outputs remain future work.
