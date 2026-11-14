# Generator plugins in the project (GP1, GP2)

Design note and contract, 13 November 2026 (GP2: 14 November 2026). GP0 built the module ABI, the SDK, the wasmi host and the `chip` module ([generators/README.md](../generators/README.md)). GP1 makes a part from a module through the one command engine, keeps the module in the project, and checks the part against it whenever the project is opened. GP2 gives every generated part its 3D model: one of the generator's own `.solidcraft` part files, shipped in the generator's bundle, at values the module computes, evaluated through the existing part-file path (the founder approved the part-generation plan of 14 November 2026: one mechanism per job, no model script). The catalog, the gallery and the part editor still use the in-code templates; they switch in GP5, after GP3–GP4.

## Decisions

| # | Decision | Why |
|---|---|---|
| G1 | **Identity.** A module is its SHA-256. A part's key is the module hash with the **params hash**, the SHA-256 of the canonical request (`{abi, scope, params}`, complete). The **result hash** is the SHA-256 of the canonical generation. | Complete parameters are filled from the module's description before hashing (`sparksmith_core::library::generated::complete`, the same fill as the host's `sparksmith_generators::params::complete`), so a default changing in a later module can never change a part's key. The scope is in the request, so a component and a package from the same values have different keys. |
| G2 | **Provenance beside the revisions.** `Library::generated` lists one `GeneratedPart { root, expansion, from }` per part: the root (the component, or the package of a package-scope part), every revision the expansion made, and `GeneratedFrom { module, abi, scope, params, result }`. It is not a field of the revision bodies. | Like origins: drafting a generated revision and publishing it again makes the person's own part, which claims nothing. A body field would be copied by `library.draft_from` into a revision that is no longer what the module makes. |
| G3 | **Embedding.** The module's bytes are a packed asset of media type `application/wasm`, one container entry per hash (`assets/sha256/<hash>`), so every part one module made shares one copy, and parts of two module versions keep both. The asset store's clean-up treats a module a record names as in use. | The container already stores assets by hash with offline reopen, bounds and refusal of unknown or mismatched entries ([container](component-model.md)). No new entry kind or format version. |
| G4 | **Expansion.** `library::generated::expand` maps a generation to definitions, a pure function of the generation and the five lineage references. Local ids follow the generation's order (terminals 1…n with their fixed functions n+1…2n; units each followed by its graphics; package terminals; pads, then silkscreen, assembly and courtyard drawings). The revisions are published in one step as new lineages. | The same function rebuilds a stored part for the check on load, so a part is either exactly what its module makes or refused. |
| G5 | **The core never runs a module.** `library.generate` uses the engine's host runtime (`Engine::set_generators`, a `GeneratorRuntime` from the ABI crate). The session sets a worker runtime; a host without one refuses `library.generate` (`generator.runtime`). The load check lives in `sparksmith-io`, which runs the modules through `sparksmith-generators` (moved to layer 1 beside the core; it depends on the ABI only). | The core stays file-free and wasm-compatible, and every open path (desktop, session, recovery) goes through `sparksmith_io::read_project`. |
| G6 | **Off the caller's thread.** `sparksmith_generators::worker::Worker` runs every call on its own thread under the host's fuel and memory limits, waits at most a wall time (2 s, the README's target) and watches a cancel flag. A call that times out or is cancelled is refused at once (`generator.timed_out`, `generator.cancelled`); the abandoned thread ends when the module returns or its fuel runs out, about 0.5 s natively, and its result is dropped. | wasmi has no interruption other than fuel; fuel bounds the work, the worker bounds the wait. |
| G7 | **Context parameters.** A parameter the request leaves out whose description names a project setting is filled from the project: `project.land.chip_gap_min` (the land rules) and `project.symbols.resistor_style` (the symbol conventions). Every other is the module's default. Then the parameters are frozen into the record. | The module never reads project state; the part key records what was used. |
| G8 | **Shipped modules are committed builds.** `cargo xtask generators build --update-lock` writes each generator's bundle to `library/generators/<name>/` (G10); `check` (in CI) fails when a shipped module is not a build `modules.lock` records for its generator, a bundle file differs from what the crate builds, or a file there belongs to no generator. | Module bytes differ per build host (README, "Reproducibility"). A build-time copy would give each platform's program different bytes, so the same part would have a different key on each. One committed copy keeps keys equal everywhere; the lock still proves on each host that the source builds reproducibly. |

## The 3D model (GP2)

| # | Decision | Why |
|---|---|---|
| G9 | **The model is a part file with values.** A generation's `model` is `PartFile { asset, values }`: a part file of the generator's bundle by SHA-256 and integer values for its parameters (nanometres, millidegrees, counts, by the parameter's unit). The module never runs Solidcraft; the file holds the geometry, the module the logic (which file, which values). | Solidcraft's own parametric designs, authored in Solidcraft, are the one 3D mechanism for every part, generated or not; no Sparksmith geometry language and no second stored model kind. |
| G10 | **The generator bundle.** `library/generators/<name>/` holds `generator.json` (format, name, the module and each part file by path and SHA-256, with each file's title, licence and provenance), `<name>.wasm` and `parts/`. Its identity is the SHA-256 of the manifest's bytes. `cargo xtask generators build --update-lock` writes it from the crate (`generators/<name>/parts/` with sidecars), `check` verifies it, `sparksmith_io::generator_bundle` reads it and refuses a missing or changed file by name. | One hash approves the module and every file it may name; the module names files by hash (`build.rs`), so the result hash covers which file the part uses. |
| G11 | **The host evaluates, the core stores.** `library.generate_part` asks the engine for the complete request (`commands::generator_request`), runs the module, finds the part file (the bundle, or a Model the project packs), evaluates it at the values (`part_file::evaluate`) and refuses unless (a) no finding blocks, (b) its bounds lie within 10 µm of the generation's envelope, and (c) every lead contact lies under a body of a lead material. `library.generate` takes the result (`Generate.model`: the Model definition, bytes when not packed, the values in the file's units and the derived data), checks that it is the file and the values the module names, and stores the file as a Model of format `solidcraft` (the newest one packing those bytes, or a new lineage published with the part) and the package as `PackageModel::PartFile`. | The core never evaluates a part file; it can still check that the host evaluated the right one. Two parts of one generator share one packed part file. |
| G12 | **The check on load** compares the stored package's model with the generation (the Model packs the named bytes, the values are the module's) and then the stored revisions with the expansion, the package holding its stored model. The derived data (bounds, surfaces) is the stored evaluation, rechecked by every board build (`part_file.stale`), not part of the equality. | Reopening needs no Solidcraft evaluation; a changed value or file is still refused. |
| G13 | **A model is required.** The host refuses a generation without one (`generator.invalid_model`), the core a `library.generate` without the host's evaluation (`generated.model_unevaluated`). | A part must never exist without its 3D model (founder, 14 November 2026). |
| G14 | **Generator version.** `Description.version` (u32, from 1) is the generator's version within its `id_hint`; a higher version is newer. The chip module is version 1. | What "a newer version of the same generator" means when a part is regenerated. |

**The chip part files.** `generators/chip/parts/` holds `chip-resistor`, `chip-capacitor`, `chip-ferrite` (inductors and ferrite beads) and `chip-led`, saved from the in-code chip builder's construction of a 0603 chip (`crates/solidcraft-integration/tests/author_parts.rs`, `author_chip`), parameters `length`, `width`, `height`, `termination`, `body_round`, `end_round`, plus `overcoat` (resistor) and `substrate`, `lens_round` (LED). The module computes every value as the builder does. Each passes the part-file check (findings: no marking plane, which a chip has none of; the cold evaluation over 100 ms on a loaded machine).

**Parity.** The part file evaluated at the module's values equals the in-code builder for the same parameters (`crates/solidcraft-integration/src/models/chip_part_tests.rs`, `the_generators_part_file_equals_the_in_code_chip_builder`): resistors and capacitors 0402, 0603, 0805 and 1206, a ferrite bead 0603 and an LED 0603 give the same bodies (names, roles, materials), bounds and volumes 0 mm apart as measured (the bar: bounds within 1 µm, a volume within the body's surface × 1 µm). Part-file body roles follow the material, so `role_of` now gives a resistor overcoat `other` and a clear or diffuse silicone `lens`, as the builder does.

**Refusals** (word for word in `crates/generators/tests/refusals.rs`, `missing_or_malformed_models_are_refused_in_plain_words`): no model; a part file named by something other than a SHA-256; a value beyond 1 m, not a whole number, or under a name no part parameter can have. Each ends "Nothing was changed." The core refuses a missing evaluation, another file than the module names, and other values (`library/generated/tests.rs`); the session refuses a file in neither the bundle nor the project, a blocking finding, bounds off the envelope and a lead contact under no lead body.

## Commands

- **`library.generate`** (core, Edit): `{ module: {hash, data?}, scope, params, name? }`. Bytes are needed only when the project does not embed the module yet; they must hash to `hash`. The command describes the module, completes the parameters (G7), and returns the part the project already holds with that key (`created: false`, nothing changes), or generates, expands, publishes the revisions as new lineages, embeds the module and records the part, as one undo step (`Outcome::Generated { report }`: root, revisions, key, result hash, created, the module's notes). Any refusal leaves the document unchanged.
- **`library.generate_part`** (session, Edit): `{ generator | module, scope, preset?, params, name?, expected_revision }`. Reads a shipped module by name (`ShippedLibrary::generator`, bounded at 1 MiB) or an embedded one by hash, merges a preset's values under `params`, and applies `library.generate`. The CLI (`stdio`) uses this request; the UI will build the same command. MCP tools arrive with GP7: until then MCP neither lists nor calls `library.generate_part` or `library.generators` (`NOT_TOOLS` in `crates/automation/src/mcp.rs`), which keeps `tools/list` within its 64 KiB budget.
- **`library.generators`** (session, Read): the shipped modules (name, hash, a summary of the description: parameters, presets, scopes), the embedded modules, and each generated part with its key, result hash and state.

## Checks on load

`sparksmith_io::read_project` (any full read of a container) runs, after the document's validation:

1. Structure (`library::generated::validate`, part of `Library::validate`): each record's revisions exist with the kinds of its scope, no revision is made by two parts, each key once, ABI 1, and the module packed as `application/wasm`.
2. Every asset entry's bytes hash to the hash in its name. A mismatched entry that a record names as its module is refused in a generator's words.
3. For each part: compile its module once per hash, run the recorded request, and compare. The result hash must equal the recorded one, and the stored revisions must equal the expansion exactly.

A refusal leaves the open document as it was. The refusals:

| Code | When | Message (example) |
|---|---|---|
| `generated.module_changed` | The embedded module's bytes do not hash to the recorded hash. | the generator module embedded in this project for ‘Chip resistor 1.6 × 0.8 mm’ (r1) has been changed: the project records sha256 a5cf49bc…a1e3, but the embedded bytes hash to sha256 0c6d…. The project was not opened. |
| `generated.result_changed` | The module makes a different result from the recorded parameters. | ‘…’ (r1) was recorded as generator result sha256 ac6c…, but its embedded module now makes sha256 …. The project was not opened. |
| `generated.expansion_changed` | The stored revisions differ from the expansion. | ‘…’ (r1) stored in this project differs from what its generator makes from the recorded parameters: the stored part hashes to sha256 …, the generator's expansion to sha256 …. The project was not opened. |
| `generated.unverified` | The module refuses or fails under the limits. | ‘…’ (r1) could not be checked against its generator: *the host's refusal*. The project was not opened. |
| `generated.module_missing` | A record names a module the project does not embed. | the generator module sha256 … that made ‘…’ (r1) is not embedded in this project |
| `generated.record`, `generated.abi`, `generated.params` | A malformed record. | names the part and what is wrong |

Hashes in messages are written in full. The "stored part" hash is the SHA-256 over the stored revisions' canonical JSON in expansion order; the expansion hash is the same over what the module makes now. Both exist only to tell the two apart in a message.

`library.generate` adds `generated.module` (the hash is packed as something else), `generated.module_changed` (inline bytes do not hash to their name), `generated.module_missing`, `generated.params`, `generated.name` and `generator.runtime`, and passes the module's own refusals (`param.*`) and the host's (`generator.*`) through with their codes.

## A missing or changed module

- **Missing from the program** (the generator folder is gone, or a module is no longer shipped): nothing changes. The project's embedded copy makes and checks the part; `library.generators` reports the part as `not_shipped`.
- **Changed** (the program ships another module of the same generator, matched by the description's `id_hint`): the project keeps its embedded copy and its parts stay as they are, checked against that copy. `library.generators` reports `shipped_differs` with both hashes. Nothing is ever updated silently. Making the part with the shipped module is an explicit `library.generate_part`, which creates new lineages beside the old ones; moving placed instances to them goes through the existing update preview (`project.preview_update`, `project.apply_update`), which is bound to the project revision and lists the impact. A dedicated "regenerate as a new revision of the same lineages" command is left open (see below).
- **Embedded copy changed** (bytes edited in the file): refused on open, `generated.module_changed`.

## Gate: no built-in data

`cargo xtask no-builtin-data` (alias `no-builtin-parts`, in `cargo xtask ci`) parses every Rust file under `apps/` and `crates/` and fails on `include_str!` or `include_bytes!` in non-test code, nested macros included. Test code is skipped: items under a test-only `cfg` (`test`, the `test-fixtures` feature), `#[test]` functions, files a test-only `mod` declares, and `tests/` and `benches/` folders. Generator crates are not scanned: their presets are the module's own data.

The allowlist (`xtask/src/no_builtin_data.rs`) names a source file and an included path for each entry:

- interface assets: the IBM Plex fonts and the blur shader (`crates/ui`);
- the MCP skill prompt (`crates/automation/src/mcp/prompts.rs`);
- **being moved** to `library/` by parallel changes: assistant profiles (`crates/assist`), fab profiles (`crates/io/src/rule_profiles.rs`), built-in part files (`crates/solidcraft-integration/src/models/part_file.rs`);
- **until GP5**: the starter package rows (`library/templates/starter.rs`), which the generator modules replace.

An entry that matches nothing fails the gate, so each is removed with the data it allowed.

## Limits of GP1

- **Fields.** A generation's `component.fields` (standard field keys such as `value`) and derived `fields` stay in the generation and its result hash; GP1 selects no field definitions on the component. Mapping keys to the project's field lineages comes with the catalog switch.
- **Package geometry.** The package keeps the generation's envelope with `PackageForm::Generic` and the generator's part file (GP2), the seating height as the model's lift. No footprint polarity is set from pin 1 yet.
- **Part-file bodies carry no terminal** (`part_file.rs` gives none), so a generated chip's terminations do not highlight with their nets yet (plan slice B2).
- **The chip part files' overcoat inset** is the builder's 0.04 mm, a literal in the saved file; the builder narrows it below 0.16 mm of body width, which no chip preset reaches.
- **Arcs** are exact at quarter turns; others are rounded once to nanometres through `f64` trigonometry, which may differ in the last bit between platforms. The chip module draws quarter-turn arcs only.
- **Bundles.** Promoting a generated part to a library folder copies its revisions without the record; the folder copy is an ordinary part.
- **Load cost.** Every open runs each part once: about 4 ms to compile a module and 1–2 ms per part (GP0's measurements), so a project with 100 generated parts adds roughly 0.2 s. A cache keyed by part key and result hash is the next step if that grows.

## Tests

- `crates/solidcraft-integration/src/models/chip_part_tests.rs` (GP2): the chip parity above; every bundled part file passes the part-file check.
- `crates/generators/tests/refusals.rs` (GP2): `missing_or_malformed_models_are_refused_in_plain_words`.
- `crates/editor-core/src/library/generated/tests.rs` (GP2): the part file published as one shared Model, refusals of a missing evaluation, another file and other values, a stored value edited refused on load.
- `crates/io/src/generator_bundle.rs` (GP2): `a_bundle_is_read_by_name_and_checked_file_by_file`.
- `apps/sparksmith/src/viewport/pictures.rs` (GP2): `a_generated_part_shows_its_model_in_3d`, the live view's model of a part made by `library.generate_part` and, with a GPU, its frame.
- `crates/automation/src/session/generated_model_tests.rs` (GP2): `a_generated_part_keeps_its_model_offline`.
- `crates/automation/src/session/generators_tests.rs` (the session request path the CLI uses): `a_generated_part_reopens_offline_identically`, `the_same_part_twice_is_found_not_made_again`, `refusals_change_nothing`, `a_changed_embedded_module_is_refused_naming_both_hashes`, `a_changed_stored_part_is_refused_naming_both_hashes`.
- `crates/editor-core/src/library/generated/tests.rs`: the expansion and the checks with a stand-in runtime.
- `crates/generators/src/worker.rs`: cancel, wall time, refusals through the worker.
- `crates/io/src/shipped.rs`: `generator_modules_are_read_by_name_within_the_shelf`.
- `xtask/src/no_builtin_data.rs`: `a_planted_part_file_in_program_code_fails`, `an_embed_inside_another_macro_is_found`, `test_code_is_skipped`, `allowed_embeds_pass_and_stale_entries_fail`.

## Open questions

1. Regenerating in place: should "update to the shipped module" publish new revisions of the same lineages (so `project.preview_update` shows the change per instance), rather than new lineages beside the old ones?
2. Should a project be allowed to keep parts whose module the program can no longer run (a future ABI 2 only host), opening read-only, rather than being refused?
3. Should `library.promote` carry the generated record and module into library folders, so a folder part keeps its provenance?
