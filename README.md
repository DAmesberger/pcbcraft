# Pcbcraft

A Rust electronics editor using Solidcraft's egui/wgpu UI and native parametric engine. Component authoring, a basic single-sheet schematic and PCB layout now share one desktop shell. PCB 3D displays physical copper and package envelopes through Solidcraft, with shared selection and whole-net highlighting.

This is an early working editor foundation, not a manufacturing-ready release. See [current evidence and limits](docs/editor-evidence.md).

## Run

Install Rust via rustup. `rust-toolchain.toml` pins Rust 1.96.0, Clippy, rustfmt and the wasm target.

```sh
cargo run --locked --profile ci -p pcbcraft
cargo xtask ci
```

The app starts with two connected resistor instances. Component authoring includes Symbol, Footprint, Mapping and 3D modes; Publish creates an immutable project snapshot. In Schematic, choose a snapshot and Place part, then connect terminals with Wire. In PCB, preview/apply Update PCB, move placements and route between pads. Switch between 2D and 3D within PCB. Select a pad/trace and enable complete-net highlighting to inspect the same net in either view.

File provides editable absolute paths to save `.pcbcraft` projects and export native `.solidcraft` models. Project files contain the exact component snapshots needed to reopen offline. Native export uses a new filename and retains parameter-driven positions and geometry; it is not a STEP wrapper. Package models currently are clearly identified envelopes.

Navigation: scroll zooms; middle-drag pans; right-drag orbits in 3D. Cmd/Ctrl + plus/minus adjusts interface scale; Cmd/Ctrl + 0 resets it. Use Fit after changing the viewport. Save/Undo/Redo are in the application bar. Component drafts are published explicitly; save the project after publishing.

## CLI and checks

```sh
cargo run --locked --profile ci -p pcbcraft-cli -- project-demo example.pcbcraft
cargo run --locked --profile ci -p pcbcraft-cli -- project-export example.pcbcraft example.solidcraft
cargo run --locked --profile ci -p pcbcraft-cli -- commands
cargo run --locked --profile ci -p pcbcraft-cli -- stdio
```

`stdio` accepts one JSON request per line (64 KiB maximum). `project.apply` uses the same typed operations as the UI, with an expected revision; `project.inspect`, `.undo`, `.redo`, `.save` and `.open` share that session. `commands` generates schemas from Rust types. This transport is JSONL, not an MCP server.

`cargo xtask ci` runs formatting, dependency boundaries, strict Clippy, native tests and headless core/adapter wasm compilation. `ci-native` omits wasm. Browser execution and remote CI have not been verified.

Solidcraft and its required patches are pinned to `4f3ac07aec69489183185071f07d7fbcf4f51e64`. Builds use git dependencies and the lockfile; a neighboring Solidcraft checkout is not required. UI source references and font licenses are recorded in [attribution](ATTRIBUTION.md).

## Native foundation fixtures

```sh
cargo run --locked --profile ci -p pcbcraft-cli -- probe fixture.solidcraft
cargo run --locked --profile ci -p pcbcraft-cli -- copper copper.solidcraft
solidcraft-cli eval fixture.solidcraft
solidcraft-cli eval examples/native-regenerate.json --in fixture.solidcraft
```

Use new output filenames. The native probe tests repeated definitions, top/bottom parametric placement and independent regeneration. The separate copper sample includes tracks, pads, via/barrel/drill, a zone with a hole and a disconnected same-net island. Those via/zone features are not yet authorable in the PCB editor. The sample is available from File for inspection.

## Scope and design

- [Product plan](docs/pcbcraft-plan.md)
- [Part library design](docs/part-library-design.md)
- [Native integration research](docs/solidcraft-interop-research.md)
- [Copper performance research](docs/copper-performance-research.md)
- [Roadmap](ROADMAP.md) and [agent instructions](AGENTS.md)

Push-and-shove remains a release-2 requirement if absent from release 1. Manual routing already passes through an atomic route edit set and strategy interface. Large-board performance, incremental native rebuilds, arbitrary Solidcraft model composition, revision-update impact plans, recovery and manufacturing outputs remain future work.
