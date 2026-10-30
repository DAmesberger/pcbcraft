# Pcbcraft agent instructions

Pcbcraft is a Rust electronics editor with component authoring, schematics, PCB layout and integrated 3D, using Solidcraft's technology and UI conventions. These instructions are adapted from Solidcraft; see [attribution](ATTRIBUTION.md).

## Start here

Read [ROADMAP.md](ROADMAP.md) for the active milestone and [docs/milestone-0.md](docs/milestone-0.md) before foundation work. Product scope is in [docs/pcbcraft-plan.md](docs/pcbcraft-plan.md); vocabulary is in [GLOSSARY.md](GLOSSARY.md). Distinguish user requirements from proposed scope boundaries in those documents.

The workspace layers are: `pcbcraft-core` (pure document, typed operations, the one engine), `pcbcraft-io` (atomic files, recovery), `pcbcraft-solidcraft` (native 3D via Solidcraft's engine), `pcbcraft-automation` (headless session, JSON-lines and MCP transports), and the desktop and CLI apps. `cargo xtask layers` enforces the direction. Run `cargo xtask ci` (inside `nix develop` if Rust is not installed) for formatting, dependency and UI-container boundaries, strict Clippy, native tests and headless wasm compilation. See [README.md](README.md) for commands and [editor evidence](docs/editor-evidence.md) for current capabilities and limits; scaling benchmarks and complete first-release workflows remain pending.

## Hard rules

- Application code is Rust, with a wasm-compatible core and egui/eframe/wgpu UI. Keep UI and transport dependencies outside the document, electronics geometry and checking layers.
- Reuse Solidcraft's engine for 3D construction, evaluation, tessellation and native persistence. The live 3D model and saved `.solidcraft` model share definitions and parameter relationships. STEP is an optional exchange format. Do not introduce a second 3D engine or a private native-file writer.
- Copper is real thickness-aware geometry. Preserve PCB object, net and layer identity through rendering, picking and persistence. Selection and camera changes must not rebuild geometry.
- Match Solidcraft's UI/UX: shared tokens, containers and navigation. PCB 2D/3D are modes of one design with shared selection, properties and undo.
- Route every design operation through one typed command engine shared by UI, CLI and MCP. Programmatic calls never open dialogs; failed document mutations leave the document unchanged.
- Library identity, explicit pin mapping and exact project snapshots are foundational. Apply library updates through an atomic impact preview; never infer electrical compatibility from matching names.
- Plan routing for push-and-shove from the first implementation; deliver it by release 2 if it is absent from release 1.
- Production Rust uses checked input handling and `Result`; forbid application `unsafe` and unchecked `unwrap`, `expect`, panic and unfinished-code macros. Bound input sizes and arithmetic; keep long work off the UI thread. Shared dependency internals retain their upstream policies.
- Preserve work with atomic saves and recovery. Reject unsupported versions and required features instead of silently dropping them. Until the first release, the file format has one version and no migrations or legacy readers; add explicit migrations once files are released.
- Pcbcraft is MIT OR Apache-2.0. Follow Solidcraft's independent-implementation policy: use public specifications/documentation and permissive code dependencies; do not read, copy, paraphrase or link GPL/LGPL/AGPL code. Do not read or copy KiCad (source or its symbol, footprint and 3D libraries, including CC-BY-SA data), LibrePCB, Horizon EDA, gEDA/Lepton or Fritzing, and take no code, data, text, icons or ids from commercial EDA tools. Fixtures are original. Attribute reused code, instructions and assets; preserve their notices. `cargo xtask licences --check` enforces dependency licenses; reasoning is in [pcbcraft-libraries](skills/pcbcraft-libraries/SKILL.md#clean-room-sources).
- Verify observable behavior and measured correctness. Label performance targets as targets until measured. Do not weaken expected results to make a test pass.
- Keep changes within the task. Work in an isolated checkout for any Solidcraft changes; honor that checkout's instructions. These rules do not authorize upstream pushes, issues, PRs or publication.

## Read the relevant skill before working

| Task | Instructions |
| --- | --- |
| Commands, transactions, crate boundaries, CLI or MCP | [pcbcraft-commands](skills/pcbcraft-commands/SKILL.md) |
| Panels, controls, navigation, 2D/3D selection or visible behavior | [pcbcraft-ui](skills/pcbcraft-ui/SKILL.md) |
| Solidcraft APIs, native models, copper geometry or performance | [pcbcraft-solidcraft](skills/pcbcraft-solidcraft/SKILL.md) |
| Symbols, footprints, pin mapping, library revisions, design-file changes, source provenance or licensing | [pcbcraft-libraries](skills/pcbcraft-libraries/SKILL.md) |
| Tests, benchmarks, CI, crash fixes or completion evidence | [pcbcraft-verification](skills/pcbcraft-verification/SKILL.md) |

Read only the branches relevant to the work. Maintain instructions and their enforcement together; do not copy upstream paths or commands that Pcbcraft does not implement.
