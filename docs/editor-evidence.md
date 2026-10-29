# Editor and copper foundation evidence

29 October 2026, macOS arm64, Rust 1.96.0. Engine/UI and patched dependencies are pinned to Solidcraft `4f3ac07aec69489183185071f07d7fbcf4f51e64`. The reference checkout remains unchanged. This report supersedes the pending desktop/editor statements in the earlier headless report.

## Implemented boundaries

Six packages plus checks: the pure electronics core (document, typed operations, the one engine), file I/O and recovery, the native Solidcraft adapter, headless automation (JSON lines and MCP), the desktop app and the CLI; `xtask` enforces their layering and the UI container rule. Every document change from the UI, CLI and MCP is a typed core operation. Transactions validate a candidate before commit, reject stale revisions and share one undo history whose entries share unchanged data. The component draft is engine state: each finished draft edit is one undoable `part.draft_set`, and it is saved and recovered with the project.

- **Components:** author electrical terminals, symbol pin anchors/body, physical package terminals, rectangular footprint pads, datum/model offsets and explicit electrical-to-physical bindings. One electrical terminal can map to multiple physical terminals/pads. Passive/connector templates, immutable revisions and project snapshots work; existing instances retain exact revisions.
- **Schematic:** single sheet, place exact snapshots, edit reference/value, snapped move, two-terminal wiring, disconnection, persistent net IDs with editable names, and a basic electrical-check subset. Crossed wires do not imply a connection. Connecting merges nets (routed, else older, ID survives); disconnecting splits them (larger group keeps the ID). Direct edits that change a routed net's membership are rejected; the update planner keeps copper on nets that only gain terminals and lists copper on shrunk, removed or merged-away nets for explicit removal.
- **PCB:** explicit additive schematic sync preview, move/rotate/flip/lock placements, manual two-click routes and atomic routing edit sets, track locks/deletion, airwires from physical copper islands, basic edge/clearance checks and net highlighting.
- **Native models:** substrate, physical 35 µm pads/traces, package envelopes, parameter-driven positions/angles/dimensions and rigid top/bottom joints. Export uses the captured electronics project, regardless of the active editor. Generated model source metadata retains instance/pad/track/net/layer identities. Background preview results are checked against request identity and current project/draft before display.
- **Persistence:** `.pcbcraft` files (format version 1, pre-release, no migrations) include exact snapshots, mappings, nets and the component draft, with bounded reads and atomic replacement. Other versions, unknown fields and damaged references are rejected without changing the current project. File dialogs, saves, opens and exports run off the UI thread; unsaved work is written to a recovery snapshot and offered at the next launch. `.solidcraft` files are written by the actual engine without STEP or cached-only geometry.

## Solidcraft source reference

The implementation directly imports Solidcraft's fonts, icons, tokens, UI controls, `Container` and GPU renderer. Shell proportions and floating panel treatment were adapted after reading its `toolbar.rs`, `shell.rs`, `ui_kit/panel.rs` and `ui_kit/layout.rs`. Editor forms reference its fields/layout implementations. The installed Solidcraft app was also inspected alongside Pcbcraft. [Attribution](../ATTRIBUTION.md) identifies the source and retains the notices.

This is a source-aligned first shell, not complete Solidcraft UX parity. Its command search, adaptive ribbon group overflow, mature document tabs, preferences and full navigation widget are not implemented. The current action row scrolls horizontally when necessary.

## Verification

`cargo xtask ci` passed: formatting, dependency layers, UI container boundary, strict Clippy, **80 tests** (40 core, 7 io, 7 automation, 13 native adapter, 12 desktop, 1 xtask), and core/adapter compilation for `wasm32-unknown-unknown`. This is not browser execution. Remote CI has not run.

Tests independently derive physical bounds and volumes for substrate, pads, package offsets, rigid bottom flips and diagonal 3–4–5 traces with butt caps. Native save/reopen tests change board/copper thickness, instance X/Y/rotation and body length using only a fresh upstream engine, then check regenerated geometry and metadata. Adapter tests run Solidcraft with strict parameter checking (effective for commands with upstream contracts: `solid.box`, `solid.cylinder`).

The CLI was exercised end to end: `demo` and `export` write a project and a native model; `mcp` answers `initialize` and `tools/list` (ten generated tools), renames a net through `project_apply` and restores it with `project_undo`.

Actual app checks with inspected screenshots (macOS, 29 October 2026): PCB 2D with browser and properties cards; narrow window hides the browser; PCB 3D builds in the background; picking a pad selects its instance and highlights the whole net (named `N2`); loading a component template is one undo step and Undo restores the previous draft; File › New with unsaved work shows the save/discard prompt; after quitting with unsaved work the next launch offers recovery and restores the draft.

## Copper fixture and performance boundary

The copper sample (an adapter test fixture since the restructuring, no longer a workspace) has seven semantic items: connected top traces, a pad, a plated via with empty drill, a disconnected same-net island, a zone with a hole and a bottom trace. Tests cover volumetric geometry, depth-aware picking, source metadata, width edits, undo/redo and native regeneration. Selection/view changes preserve mesh `Arc` identity and the scene-build counter; GPU scene blocks reuse vertex bytes when only the palette changes.

No large-board speed claim is made. Current native construction rebuilds the project in a worker; it is not incremental or cancellable within a build. It creates separate envelope definitions per instance, although the earlier native fixture proves shared definition reuse. The 1k/10k/100k benchmark, GPU draw/upload metrics, spatial indexing and local cache invalidation remain open. Current safeguards bound the layout to 2,000 tracks and 4,096 placed pads, and stop pair checks at 100,000 comparisons with an explicit incomplete-check message.

## Remaining release work

The initial editors are intentionally narrow: rectangular SMD pads and traces; two copper layers; fixed 1.6 mm substrate and 35 µm copper; manual routes; basic electrical/clearance checks. The copper sample's vias and zone cannot yet be authored in the PCB editor. No manufacturing-ready DRC claim follows from zero basic-check messages.

Still pending: arbitrary symbol/footprint drawing, native model-history composition and reusable model assets, multi-sheet hierarchy, junction/net-label tools, configurable stackup/rules, vias/pours, manufacturer catalogs/import and manufacturing output. On macOS, Cmd+Q quits without the unsaved-work prompt (the work survives in recovery); the window close button and File actions do prompt. Solidcraft's camera navigation and app shell are private upstream, so Pcbcraft re-applies the same camera operations and rebuilds the ribbon/card helpers from public tokens and containers. Push-and-shove remains release 2 if absent from release 1; the routing strategy and atomic edit-set boundary are in place, not the solver.
