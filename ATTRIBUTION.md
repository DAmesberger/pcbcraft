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

- `apps/pcbcraft/src/shell.rs`: upstream `crates/ui-egui/src/toolbar.rs`, `shell.rs`, `ui_kit/panel.rs` and `ui_kit/layout.rs` for workspace/ribbon proportions, floating cards and form spacing.
- `apps/pcbcraft/src/app.rs` and `app/chrome.rs`: upstream application bar (`toolbar::APP_BAR_HEIGHT`, `titlebar::TRAFFIC_LIGHTS`), bundled font setup, system-theme default, close-request handling and container layout conventions.
- `apps/pcbcraft/src/file_ui.rs`: upstream `apps/solidcraft/src/main.rs` picker services and `documents.rs` unsaved-work choices; prompts use the public `ui_kit::Window`.
- `crates/automation/src/mcp.rs` and `lines.rs`: upstream `crates/mcp/src/server.rs` conventions (newline-delimited JSON-RPC, bounded lines, protocol-version negotiation, tool errors as results). Tools and their schemas are generated from Pcbcraft's own request types.
- `xtask/src/ui_containers.rs` and `layers.rs`: upstream `xtask/src/ui_containers.rs` and `layers.rs` boundary checks, reduced to Pcbcraft's crates.
- `xtask/src/licenses.rs`: follows the upstream `xtask/src/licenses.rs` convention of reading `cargo metadata --locked` from an xtask subcommand. The SPDX expression evaluation, GPL-family policy and reviewed-exception list are original.
- `flake.nix`: upstream `flake.nix` Linux runtime-library list; the pinned toolchain comes from `rust-overlay` (MIT).
- Editor content: upstream `ui_kit/layout.rs`, `ui_kit/fields.rs`, `dialogs_library.rs`, `tools.rs` and `drag_snap.rs` for forms and interaction conventions. Electronics commands, mapping and geometry checks are original.
- `viewport.rs`: upstream `gpu` renderer API and `solidcraft-render` camera API via the engine. Navigation is adapted locally because the complete upstream event handler is private to `SolveApp`.

IBM Plex Sans and IBM Plex Mono are included by the upstream UI package. Their SIL Open Font License notices are preserved in `licenses/ibm-plex-sans-OFL.txt` and `licenses/ibm-plex-mono-OFL.txt`; original font assets and notices remain in the pinned upstream dependency. The installed Solidcraft 0.26.1 app was also inspected for visual comparison; implementation references remain pinned to the source revision above.
