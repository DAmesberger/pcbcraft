# Editor and copper foundation evidence

29 October 2026, macOS arm64, Rust 1.96.0. Engine/UI and patched dependencies are pinned to Solidcraft `4f3ac07aec69489183185071f07d7fbcf4f51e64`. The reference checkout remains unchanged. This report supersedes the pending desktop/editor statements in the earlier headless report.

## Implemented boundaries

Five packages separate the electronics command/document core, native Solidcraft adapter, desktop app, CLI and checks. All committed editor changes use the typed core operations. Transactions validate a candidate before commit, reject stale revisions, and share undo/redo. The GUI's unpublished component draft is local form state, published explicitly.

- **Components:** author electrical terminals, symbol pin anchors/body, physical package terminals, rectangular footprint pads, datum/model offsets and explicit electrical-to-physical bindings. One electrical terminal can map to multiple physical terminals/pads. Passive/connector templates, immutable revisions and project snapshots work; existing instances retain exact revisions.
- **Schematic:** single sheet, place exact snapshots, edit reference/value, snapped move, two-terminal wiring, disconnection, stable terminal-based nets and a basic electrical-check subset. Crossed wires do not imply a connection. Changes that would invalidate routed net membership are rejected.
- **PCB:** explicit additive schematic sync preview, move/rotate/flip/lock placements, manual two-click routes and atomic routing edit sets, track locks/deletion, airwires from physical copper islands, basic edge/clearance checks and net highlighting.
- **Native models:** substrate, physical 35 µm pads/traces, package envelopes, parameter-driven positions/angles/dimensions and rigid top/bottom joints. Export uses the captured electronics project, regardless of the active editor. Generated model source metadata retains instance/pad/track/net/layer identities. Background preview results are checked against request identity and current project/draft before display.
- **Persistence:** version-1 `.pcbcraft` files include exact snapshots and mappings, bounded reads and atomic replacement. Unsupported versions/unknown required fields and damaged references are rejected without changing the current project. `.solidcraft` files are written by the actual engine without STEP or cached-only geometry.

## Solidcraft source reference

The implementation directly imports Solidcraft's fonts, icons, tokens, UI controls, `Container` and GPU renderer. Shell proportions and floating panel treatment were adapted after reading its `toolbar.rs`, `shell.rs`, `ui_kit/panel.rs` and `ui_kit/layout.rs`. Editor forms reference its fields/layout implementations. The installed Solidcraft app was also inspected alongside Pcbcraft. [Attribution](../ATTRIBUTION.md) identifies the source and retains the notices.

This is a source-aligned first shell, not complete Solidcraft UX parity. Its command search, adaptive ribbon group overflow, mature document tabs, preferences and full navigation widget are not implemented. The current action row scrolls horizontally when necessary.

## Verification

`cargo xtask ci` passed: formatting, workspace dependency rules, strict Clippy, **41 tests** (23 core, 13 native adapter, 4 desktop integration, 1 CLI), and core/adapter compilation for `wasm32-unknown-unknown`. This is not browser execution. Remote CI has not run.

Tests independently derive physical bounds and volumes for substrate, pads, package offsets, rigid bottom flips and diagonal 3–4–5 traces with butt caps. Native save/reopen tests change board/copper thickness, instance X/Y/rotation and body length using only a fresh upstream engine, then check regenerated geometry and metadata. Curved fixture measurements use a tolerance tied to the upstream tessellation tolerance; they are not asserted to be exact analytic mass properties.

The standalone pinned Solidcraft CLI opened `artifacts/editor-demo.solidcraft`. Its report (`artifacts/editor-upstream.json`) confirms seven closed bodies: a 60×40×1.6 mm substrate (3840 mm³), two envelopes and four 1×1.4×0.035 mm pads (0.049 mm³ each). Adapter tests additionally exercise traces and bottom placements.

Actual app checks and inspected screenshots:

1. Open all three editors; inspect Solidcraft fonts, icons, ribbon, browser and properties cards.
2. Edit R1 from 10k to 22k; Apply and Undo restore the observable schematic value.
3. Publish a connector snapshot, place it in the schematic and preview/apply one additional PCB placement.
4. Route R1 pad 2 to R2 pad 1; track count becomes one and the two unrouted-pad reports disappear. Switch to native 3D; the routed copper and its connected pads retain whole-net highlight.
5. Edit File paths and save a project. Export from the component workspace to `artifacts/editor-ui.solidcraft`; the standalone Solidcraft CLI reopens seven PCB/part/pad bodies, proving it exported the PCB rather than the active draft. A discovered popup-close-on-input bug was fixed with `CloseOnClickOutside`.
6. Increase UI zoom using egui's keyboard shortcut until the browser collapses; inspect PCB and component layouts with reduced logical canvas width. OS window resize attempts did not change the captured window, so this verifies constrained layout through UI scaling, not a physical 900×680 OS window.

Screenshots live under ignored `artifacts/`: `solidcraft-reference.jpeg`, `editors-component-scaled.jpeg`, `editors-schematic.jpeg`, `editors-route-2d.jpeg`, `editors-route-3d.jpeg`, `editors-scaled.jpeg` and `file-menu-fixed.jpeg`. Earlier workflow screenshots include the copper diagnostic tab, subsequently moved into File.

## Copper fixture and performance boundary

The separate copper sample has seven semantic items: connected top traces, a pad, a plated via with empty drill, a disconnected same-net island, a zone with a hole and a bottom trace. Tests cover volumetric geometry, depth-aware picking, source metadata, width edits, undo/redo and native regeneration. Selection/view changes preserve mesh `Arc` identity and the scene-build counter; GPU scene blocks reuse vertex bytes when only the palette changes.

No large-board speed claim is made. Current native construction rebuilds the project in a worker; it is not incremental or cancellable within a build. It creates separate envelope definitions per instance, although the earlier native fixture proves shared definition reuse. The 1k/10k/100k benchmark, GPU draw/upload metrics, spatial indexing and local cache invalidation remain open. Current safeguards bound the layout to 2,000 tracks and 4,096 placed pads, and stop pair checks at 100,000 comparisons with an explicit incomplete-check message.

## Remaining release work

The initial editors are intentionally narrow: rectangular SMD pads and traces; two copper layers; fixed 1.6 mm substrate and 35 µm copper; manual routes; basic electrical/clearance checks. The copper sample's vias and zone cannot yet be authored in the PCB editor. No manufacturing-ready DRC claim follows from zero basic-check messages.

Still pending: arbitrary symbol/footprint drawing, native model-history composition and reusable model assets, multi-sheet hierarchy, junction/net-label tools, configurable stackup/rules, vias/pours, full revision-update impact plans, manufacturer catalogs/import, manufacturing output, file dialogs, recovery and unsaved-work prompts. Save and Publish are explicit; unpublished drafts and unsaved project changes are not recovered on exit. Push-and-shove remains release 2 if absent from release 1; the routing strategy and atomic edit-set boundary are in place, not the solver.
