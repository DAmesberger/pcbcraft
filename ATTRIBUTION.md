# Attribution

## Solidcraft agent instructions

`AGENTS.md`, `CLAUDE.md` and the instructions in `skills/pcbcraft-*/SKILL.md` adapt conventions and guidance from [Solidcraft](https://github.com/tangency-labs/solidcraft/tree/4f3ac07aec69489183185071f07d7fbcf4f51e64), revision `4f3ac07aec69489183185071f07d7fbcf4f51e64`.

Source material includes its `AGENTS.md`, `CLAUDE.md`, and skills for commands, UI, crash safety, file formats, tests, performance and ground rules. The adapted instructions replace Solidcraft-specific workflows and add Pcbcraft's electrical-library, routing, native-model and copper requirements.

Upstream copyright: **Copyright (c) 2026 SolidCraft contributors**. Upstream offers MIT OR Apache-2.0; these instruction adaptations use the MIT option, whose full notice is preserved in [licenses/solidcraft-MIT.txt](licenses/solidcraft-MIT.txt).

## Build conventions and engine dependency

The Cargo development/CI profiles, pinned geometry patches and `xtask` workflow convention are adapted from the same Solidcraft revision under the MIT option above. The implementation of Pcbcraft's commands and checks is original. The desktop links Solidcraft’s UI package and uses its bundled visual assets directly.

Pcbcraft directly depends on `solidcraft-engine` and its geometry patches at that revision. Cargo retrieves the original packages with their upstream license notices; `Cargo.lock` records resolved dependencies. Pcbcraft does not maintain a separate CAD engine or native-file writer. Record further adopted source/assets and their notices here.

Original Pcbcraft code is licensed MIT OR Apache-2.0, like Solidcraft; see [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE) and [NOTICE](NOTICE). Every workspace crate declares this license, and `cargo xtask licences --check` enforces it together with the dependency license policy.

## Embedded scripting dependency

The scripting feasibility slice embeds [Rhai](https://github.com/rhaiscript/rhai) 1.27.0 (MIT OR Apache-2.0) as a Cargo dependency. Cargo retains upstream notices; the dependency is pinned in the manifest and lockfile. Pcbcraft's quantity operations, pad-generation interface and test fixtures are original. The integration uses the documented [custom type](https://rhai.rs/book/rust/custom-types.html), [expression-only](https://rhai.rs/book/engine/expressions.html), [execution limit](https://rhai.rs/book/safety/max-operations.html) and [wasm](https://rhai.rs/book/start/builds/wasm.html) APIs. `wasm-bindgen-test` and its separately installed matching runner provide the optional Node-hosted wasm test execution.

## Desktop UI reference and reuse

The desktop links `solidcraft-ui-egui` at the same pinned revision. Fonts, icons, theme tokens, controls, `Container`, camera math and GPU renderer are upstream implementations, not lookalike assets.

Adapted source references, under the MIT option above:

- `apps/pcbcraft/src/shell.rs`: upstream `crates/ui-egui/src/toolbar.rs` (workspace switcher tile and caption, tab row, ribbon button sizing and section captions, `WORKSPACE_WIDTH` and button constants), `shell.rs` and `ui_kit/panel.rs` for floating cards.
- `apps/pcbcraft/src/kit/`: upstream `ui_kit/layout.rs` (`scope`, `row`, `status_line`, `section_count`), `ui_kit/fields.rs` (dropdown with the kit chevron, cell editor use), `ui_kit/disclosure.rs`, `ui_kit/panel.rs` (`inspector_header`, workspace chip) and `ui_kit/tree.rs` plus `browser.rs` (`tree_row`, Browser title band, `TREE_ROW_HEIGHT`). Upstream keeps these crate-private; Pcbcraft reproduces their geometry on the public tokens, fonts and icons.
- `apps/pcbcraft/src/kit/search.rs`: upstream `ui_kit/search.rs` — the command-search field (zoom icon, focus ring, key cap, expanded spacing), the sheet's menu frame and key-hint footer, group headings, the 38 pt `search_row` with its icon tile, character-boundary match highlighting and `SearchBadge` looks — adapted for the Place palette. Upstream keeps it crate-private, so it is reproduced on the public tokens, fonts, icons and `Container`. The palette's content, catalog ranking and completeness badges (`apps/pcbcraft/src/place_palette.rs`, `pcbcraft-core::catalog`) are original.
- `apps/pcbcraft/src/app/browser.rs`: the Browser's fold, selection-bar and branch-mark conventions follow upstream `browser.rs`. Its project tree (components, instances, nets, board, placements, tracks) is original.
- `apps/pcbcraft/src/app.rs` and `app/chrome.rs`: upstream application bar (`toolbar::APP_BAR_HEIGHT`, `titlebar::TRAFFIC_LIGHTS`), bundled font setup, system-theme default, close-request handling and container layout conventions.
- `apps/pcbcraft/src/file_ui.rs`: upstream `apps/solidcraft/src/main.rs` picker services and `documents.rs` unsaved-work choices; prompts use the public `ui_kit::Window`.
- `crates/automation/src/mcp.rs` and `lines.rs`: upstream `crates/mcp/src/server.rs` conventions (newline-delimited JSON-RPC, bounded lines, protocol-version negotiation, tool errors as results). Tools and their schemas are generated from Pcbcraft's own request types.
- `xtask/src/ui_containers.rs` and `layers.rs`: upstream `xtask/src/ui_containers.rs` and `layers.rs` boundary checks, reduced to Pcbcraft's crates.
- `xtask/src/licences.rs`: follows the upstream `xtask/src/licences.rs` convention of reading `cargo metadata --locked` from an xtask subcommand. The SPDX expression evaluation, GPL-family policy and reviewed-exception list are original.
- `flake.nix`: upstream `flake.nix` Linux runtime-library list; the pinned toolchain comes from `rust-overlay` (MIT).
- Editor content: upstream `ui_kit/layout.rs`, `ui_kit/fields.rs`, `dialogs_library.rs`, `tools.rs` and `drag_snap.rs` for forms and interaction conventions. Electronics commands, mapping and geometry checks are original.
- `apps/pcbcraft/src/part_editor/drawing.rs`: the symbol drawing tools follow upstream `crates/ui-egui/src/tools.rs` (`Tool`/`Kind::Draw(n)` point collection, line chaining, Esc/right-click `finish` ending the chain and then the tool, `undo_spline_point` popping a pending point, and `follow_arc_turn`/`center_arc_sweep` for a centre-point arc's direction) and `edit.repeat` (Shift+R). The tool set, rounded-corner and pin/field placement are original.
- `apps/pcbcraft/src/part_editor/snap.rs`: snapping follows upstream `crates/ui-egui/src/drag_snap.rs` (point, midpoint and on-curve targets, horizontal/vertical alignment with dashed guides, Ctrl/Cmd or Alt to suppress, short snap labels), without constraints. The priority order, pin and grid targets are original.
- `apps/pcbcraft/src/shell.rs` ribbon overflow: the » menu for groups that do not fit follows upstream `toolbar.rs`'s overflow menu.
- `apps/pcbcraft/src/shell.rs` split ribbon buttons: the icon part runs the shown variant and the label with ▾ opens the family's variants, after upstream `toolbar.rs`'s sketch families (`sketch_families`).
- `viewport.rs`: upstream `gpu` renderer API and `solidcraft-render` camera API via the engine. Navigation is adapted locally because the complete upstream event handler is private to `SolveApp`. Adaptive supersampling follows upstream `crates/ui-egui/src/viewport.rs` `supersampling` and `view_settled` (a view hash and a settle delay before drawing at 2×).
- `apps/pcbcraft/src/lighting.rs`: product-light record packing follows upstream `crates/ui-egui/src/render_workspace_light.rs` `preview()` (the `[position, kind, radiance, outer, axis, inner, reference distance², minimum distance]` layout). The studio rig itself is original.
- `crates/solidcraft-integration/src/footprint_spike/` (test-only spike F0): point-expression application follows upstream `crates/doc/src/document.rs` `Document::apply_point_values` (evaluate each coordinate as a length, range-check, fix the point), and the scratch-`Session` bridge follows the `Session::scratch` + nested `execute` pattern in upstream `crates/engine/src/cmd/sketch.rs` (rounded rectangle). Pad templates, `check_sketch`, the copper-only pass, arrays, quantizer and fixtures are original; the fixtures are synthetic geometry.
- `apps/pcbcraft/src/model_scene.rs`: the ground shadow follows upstream `crates/ui-egui/src/viewport.rs` `ground_shadow` (four fading ellipse rings of 48 segments, 80/150 peak alpha), adapted to a Z-up board. The contact shadows are original.

IBM Plex Sans and IBM Plex Mono are included by the upstream UI package. Their SIL Open Font License notices are preserved in `licenses/ibm-plex-sans-OFL.txt` and `licenses/ibm-plex-mono-OFL.txt`; original font assets and notices remain in the pinned upstream dependency. The installed Solidcraft 0.26.1 app was also inspected for visual comparison; implementation references remain pinned to the source revision above.
