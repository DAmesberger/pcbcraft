# Rename to Sparksmith

Pcbcraft was renamed Sparksmith (sparksmith.io) to avoid confusion with tangency-labs' PCBCraft, a separate project. Phase A changed every name a person sees; phase B (13 November 2026) changed the rest. The product's names come from one module, `sparksmith_core::product`, and `apps/sparksmith/src/bundle_tests.rs` checks that `macos/Info.plist` agrees with it.

## Phase B, as done

The founder's decisions of 12 November 2026 apply.

- **Crates and paths.** Every package is `sparksmith-*` (the desktop package is `sparksmith`), every module path `sparksmith_*`, the apps live in `apps/sparksmith` and `apps/sparksmith-cli`, and the skills in `skills/sparksmith-*`. The xtask checks, `flake.nix` and the workspace keys follow.
- **Binaries.** The CLI is `sparksmith`; the desktop binary is `sparksmith-desktop` (`CFBundleExecutable`), since two binaries of one workspace cannot share a name.
- **Extension.** Projects are `.sparksmith` (MIME `application/x-sparksmith`).
- **Names stored in files.** The library manifest `sparksmith-library.json`, the native-model metadata keys `org.sparksmith.electronics…`, appearance names, provenance text, the hash domains and the `convert-v1` provenance. The file format has one version and no released files, so nothing is migrated. The ESP32 dev board project changed only in its stored text (title block, rule sources) and the fill keys that hash it; its fabrication files only in the project id.
- **Generator ABI.** The exports are `sparksmith_gen_*`, and the modules `sparksmith_gen_*.wasm`.
- **Environment variables** are `SPARKSMITH_*`, with no fallback.
- **MCP.** `sparksmith://` resources and `skill://sparksmith-*`.
- **Internal names.** Thread names, egui ids, stderr prefixes and doc comments.
- **Licence holder.** "Sparksmith contributors".
- **Docs.** `docs/sparksmith-plan.md`.

## What still reads the former name

- **`.pcbcraft` files** hold the current format. They open through the same reader and version check, from the open dialog and from Finder. Save keeps the opened path. Save As and new projects write `.sparksmith`, and nothing renames a file silently. Drop `.pcbcraft` at the first release, unless released files exist by then.
- **Recovery snapshots** named `*.pcbcraft-recovery` are still listed, restored and discarded. New ones are `*.sparksmith-recovery`.
- **The keychain.** A key found only under the former service `pcbcraft-llm` is copied to `sparksmith-llm` when it is read. The former entry is kept until that connection's key is removed.
- **The per-user folders** (`UserDirs::adopt_former`, `crates/io/src/former_name.rs`) are copied once from their former names. Remove this once every install has started under the new name. The repository cannot tell when that is, so the copy stays for now.

## Names that stay

- **Historical records.** The dated run reports (`docs/e2e-*.md`, `docs/datasheet-assistant-run1.md`, the VM measurements in `docs/3d-speed-profile.md`) keep the names they ran under. So do the recorded model replies (`examples/**/*.recording.jsonl`) and the first assistant run's project (`examples/esp32-s31-wroom-3/run1/esp32-s31-wroom-3.pcbcraft`, also a test that `.pcbcraft` files open).
- **Other projects' names.** These belong to their projects: tangency-labs' PCBCraft (`tangency-labs/pcbcraft`), and the Solidcraft fork's branch and tag names (`pcbcraft-smlib-open`, `pcbcraft-via2`, `pcbcraft-product-look`, `pcbcraft-pre-part-templates`).
