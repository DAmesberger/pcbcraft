# Generator modules

A generator is a plugin of the one plugin ABI ([extension ABI §2.6](../docs/design/extension-abi-design.md)) that offers the capability `generate-part`, of effect `none`: a pure function in a WebAssembly module, typed parameters in, a complete part body out (symbol, package, footprint, and the 3D model as one of its bundle's part files with values). Each directory here is one generator crate that builds to `wasm32-unknown-unknown`. The modules are loaded as data, never linked into the app; the host that runs them is `crates/generators` (`sparksmith-generators`).

Status: slices GP0, GP1 and GP2. A part can be made from a module through the command engine (`library.generate`, `library.generate_part`); the project embeds the module and checks the part against it on every open; the module names one of its bundle's `.solidcraft` part files and its values as the 3D model, which the host evaluates through Solidcraft like any part file ([generator plugins](../docs/generator-plugins.md)). The catalog, the gallery and the part editor do not use modules yet (GP5, GP7); the in-code templates stay until GP5. ABI 1 is open to change until the first release.

## Layout

| Path | What |
| --- | --- |
| `crates/generator-abi` | `sparksmith-generator-abi`: the ABI 1 wire types (`v1`), the canonical JSON codec, the parameter check, and the `GeneratorRuntime` trait. `no_std` + `alloc`, no generator logic. |
| `crates/generator-sdk` | `sparksmith-generator-sdk`: what a generator is written with: `Generator` and `export_generator!`, the module's allocator and panic handler, typed parameters, symbol drawing, footprint drawing (courtyard, silkscreen broken round the pads, assembly), and the golden-test harness. |
| `crates/generators` | `sparksmith-generators`: the wasmi host, with `Limits` and the output checks. |
| `crates/generators-probe` | A test module: the host itself built for wasm32, so the native tests can show the wasm32 build gives the same results. Never shipped. |
| `crates/generators-fixture` | A test plugin (`sparksmith-gen-fixture`): `generate-part` beside an `edit` command, so it may import the host functions; during `generate-part` it calls all three and draws a 45° arc. Its golden case runs natively, under wasmi and under the probe. Never shipped. |
| `generators/<name>/` | One generator: `Cargo.toml` (cdylib), `build.rs` (the part files' SHA-256 as constants), `src/`, `presets.json`, `golden.json`, `parts/` (its part files, each `<stem>.solidcraft` with a `<stem>.part.json` sidecar in the shipped part files' format: title, summary, licence, provenance). |
| `generators/modules.lock` | Each module's SHA-256 per build host, committed. |
| `library/generators/<name>/` | The shipped **bundle**, committed: `generator.json` (the manifest), `<name>.wasm` (a build the lock records) and `parts/*.solidcraft`. `build --update-lock` writes it, `check` verifies it. |

`cargo xtask layers` keeps it apart: a generator crate depends on the SDK and the ABI only, only generator crates depend on the SDK, nothing depends on a generator crate or on the probe, and the ABI crate depends on no workspace crate (so `sparksmith-core` can use its types and trait without an interpreter).

## ABI 1: the one plugin ABI

**Exports.** A module exports its memory as `memory` and four functions:

| Export | Signature | Meaning |
| --- | --- | --- |
| `sparksmith_abi` | `() -> u32` | Returns 1. |
| `sparksmith_alloc` | `(len: u32) -> u32` | A buffer for a request (or a reply the host hands back from `call`), or 0 when `len` is 0 or over 64 KiB. |
| `sparksmith_describe` | `() -> u64` | `(ptr << 32) \| len` of the canonical `Description`. |
| `sparksmith_invoke` | `(ptr: u32, len: u32) -> u64` | Reads the canonical `Invoke` (`{abi, capability, params, scope?}`) from the buffer and returns `(ptr << 32) \| len` of the response: a status byte (0 Ok, 1 Err), then the capability's result (a canonical `Generation` for `generate-part`) or `{"errors":[…]}`. |

**Imports.** At most the three functions of module `sparksmith`, all optional: `call (ptr, len) -> u64`, `log (level, ptr, len)` and `cancelled () -> u32`. Anything else, or one of these with another signature, is refused before the module runs. This host serves capabilities of effect `none` only: during such an invoke it answers `call` with the fixed refusal `plugin.pure` (`v1::pure_refusal`, written into a buffer the module allocates), drops `log` and answers `cancelled` with 0, so the result depends on the request alone. A plugin whose capabilities are all of effect `none`, a plain generator, may import nothing at all (`generator.has_imports`). The SDK's `host` module wraps the three; natively it answers as the host does in a pure invoke.

**Description**, one format for every plugin: `{abi, id, version, title, licence, deterministic, capabilities: [{name, effect, title?, params, rules, presets, part?}], needs: [{module, major, minor?, class}]}`. `effect` is `none`, `read` or `edit`; a generator's `generate-part` declares `none` and its `part` holds the families, scopes, search phrases and instance fields. A plugin offering any capability of effect `none` declares `deterministic: true`; a plugin whose capabilities are all of effect `none` has empty `needs`. The host refuses a description that breaks any of this (`generator.invalid_description`).

**Floats are refused.** The host validates every module without the WebAssembly float feature (`wasmi::Config::floats(false)`), so a plugin whose results are hashed contains no float instruction or type (`generator.float`). The ABI crate's codec has its own integer-only reader, writer and serde (de)serializer, so no float code reaches a module; the SDK computes arc ends with integer trigonometry (`draw::arc`, `draw::cos_sin`, a Taylor series in a 2^60 fixed point, exact at quarter turns).

The host checks a module before anything runs (size, a valid core module of the allowed features, imports, no start function, the four exports and the memory, memory within the limit), then gives every call a fresh instance with a fuel budget and a memory cap, decodes the reply with the canonical decoder and checks it: unique keys, every reference resolving (pads to package terminals, symbol pins and binding to component terminals), arcs whose ends lie on one circle about their centre within 1 nm, values in range, and the parts the scope requires present and no others.

**Canonical JSON.** Object keys sorted by their UTF-8 bytes, each once; no whitespace; integers only (`i64`/`u64`, shortest form, so no floats, exponents or `-0`); strings escaped as `serde_json` escapes them, otherwise literal UTF-8, and in Unicode NFC (checked by the host); absent optional fields omitted, never `null`. The decoder accepts only bytes the encoder would write. Lengths are integer nanometres and angles integer millidegrees.

**Request.** `{abi: 1, scope: component | package, params: {key: value}}`, sent inside the invoke envelope for `generate-part`. The parameters are complete: the host fills every default from the description first (`sparksmith_generators::params::complete`), so the part key never depends on a module's defaults. Parameters a project setting decides (the least chip land gap, the resistor symbol style) are declared as `context` parameters and frozen into the request.

**Generation keys (slice B1).** Every generated object has a key: terminals, units, symbol drawings (`Graphic.key`, unique within its unit), package terminals, pads, footprint drawings (unique across silkscreen and assembly). The host gives each key a local id and records the key-to-id map with the part; regenerating keeps every key's id and gives new keys new ids, so ids do not depend on the order a module emits things in (`docs/generator-plugins.md`, G4).

**Identity (slice B4).** Part key: the generator's `id` with the params hash, SHA-256 of the canonical `Request` (ABI, scope, complete parameters as recorded; `sparksmith_core::library::generated::key`). The version tells revisions of that lineage apart. Result hash: SHA-256 of the canonical `Generation` bytes (after the status byte), the integrity check. Module hash: SHA-256 of the module bytes, which the check on open runs. Bundle hash: SHA-256 of the manifest's bytes, the provenance a person approves. No part key depends on the module's bytes, so a part made from the same source is the same part on every host.

**Refusals** are `{code, field?, message}` with a sentence a person can act on: `param.unknown`, `param.missing`, `param.type`, `param.out_of_range`, `param.inconsistent` and `param.scope` for parameters; `capability.unknown` from the SDK; `generator.too_large`, `generator.invalid_module`, `generator.unsupported_feature`, `generator.float`, `generator.has_imports`, `generator.missing_export`, `generator.abi_mismatch`, `generator.out_of_fuel`, `generator.memory_limit`, `generator.trapped`, `generator.output_too_large`, `generator.request_refused`, `generator.invalid_output`, `generator.invalid_model` and `generator.invalid_description` from the host. Every host refusal ends in "It was not run." or "Nothing was changed."

**Approvals.** A person approves a plugin by its bundle hash, per user, outside any project (`sparksmith_io::plugin_approvals`, `plugin-approvals.json` in the config directory; `sparksmith plugin approvals | approve NAME | revoke SHA256`). A changed bundle of an approved plugin asks again and says what changed (version, capabilities and their effects, needs, licence, or only the files). Effect-`none` capabilities need no grant; GP9 will read the store before a third-party bundle runs on open.

## The 3D model and the bundle (GP2)

A part never exists without its 3D model, so every generation carries `model`, and the host refuses one without it (`generator.invalid_model`). The model is one of the generator's own part files with values:

```json
"model":{"part_file":{"asset":"<sha256 of the part file>","values":{"body_round":40000,"end_round":67500,"height":450000,"length":1600000,"overcoat":30000,"termination":300000,"width":800000}}}
```

- **The part file** is an ordinary parametric `.solidcraft` design that follows the part-file conventions ([part templates §11](../docs/part-templates.md)); it ships in the generator's bundle and the module names it by its SHA-256 (the crate's `build.rs` writes the hashes of `parts/` as constants). The module never runs Solidcraft; it decides which file and which values.
- **Values** are integers: nanometres for a length parameter, millidegrees for an angle, the count for a unitless one (the part file's parameter decides). The project stores them as integers too (`PartValue`: `{"nm": …}`, `{"mdeg": …}`, `{"count": …}`, `{"milli": …}`, file version 3); a part file reads each as an exact decimal (`1.6 mm`). The host refuses a value that is not a whole number, one beyond 1 m, or a name that cannot name a part parameter; at most 256 values.
- **The host** (`library.generate_part`) finds the file in the bundle (or, for a module the project embeds, in the project), evaluates it at the values through the part-file path (`sparksmith_solidcraft::models::part_file`), and refuses a blocking finding, bounds more than 10 µm off the generation's envelope, or a lead contact under no body of a lead material. `library.generate` then stores the file as a Model of format `solidcraft` (one per file, shared by every part that uses it) and the package as `PackageModel::PartFile`, so the 3D view, the exact export, markings and the load check treat it like any part file.
- **The bundle** (`library/generators/<name>/`) is the manifest `generator.json` (`format`, `name`, the module and each part file by path and SHA-256, with each part file's title, licence and provenance), the module and `parts/`. Its identity is the SHA-256 of the manifest's bytes (`library.generators` reports it as `bundle`), which names every other file by hash. `sparksmith_io::generator_bundle` reads a bundle and refuses a missing or changed file by name.

The chip generator ships four part files (resistor, capacitor, ferrite or inductor, LED), saved from the in-code chip builder's construction (`cargo test -p sparksmith-solidcraft --test author_parts -- --ignored`); evaluated at the module's values they equal the builder to 0 mm in bounds and volume for resistors and capacitors 0402–1206, a ferrite bead 0603 and an LED 0603 (`models::chip_part_tests`).

## ABI 1 changes

ABI 1 is open to change until the first release; each change is listed here, and module hashes and golden hashes are updated deliberately with it (`build --update-lock`).

| Date | Slice | Change | Effect |
| --- | --- | --- | --- |
| 11 November 2026 | GP0 | First version. | — |
| 14 November 2026 | GP2 | `Description.version` (required, from 1). `Generation.model` required; `Model::PartFile { asset, values }` defined (a bundled part file by SHA-256, integer values). Modules ship as bundles (`library/generators/<name>/`). New refusal codes `generator.invalid_model`, and `generator.invalid_description` for version 0. | A module built before GP2 is refused (no version, no model). The chip module's hash and every golden result hash changed; the lock was rewritten on macOS aarch64, which dropped the Linux x86_64 hash (the CI runner); record it again there. |
| 14 November 2026 | B1, B3, B4, S8 | The one plugin ABI: exports `sparksmith_abi`, `sparksmith_alloc`, `sparksmith_describe`, `sparksmith_invoke` (the `Invoke` envelope); the optional imports `sparksmith.call`, `log`, `cancelled`, answered as a pure invoke; the description format with capabilities, effects and needs (`id` and `title` replace `id_hint` and `name`). `Graphic.key`. `Primitive::Arc { center, start, end, clockwise }` with exact ends. Floats refused; the codec without `serde_json`. Part key by generator id. New refusal codes `generator.float`, `capability.unknown`. | Every older module is refused (`generator.missing_export`); file version 3 detaches parts recorded by one. The chip module shrank to 234 868 bytes; its hash and every golden result hash changed; the lock was rewritten on macOS aarch64, which dropped the Linux x86_64 hash: record it again there (it should now equal the macOS hash, see "Reproducibility"). |

## Build and check

```sh
cargo xtask generators build                # build every module, print sizes and hashes
cargo xtask generators build --update-lock  # … write modules.lock (the only way it changes) and library/generators/
cargo xtask generators check                # in `cargo xtask ci`: build, compare with the lock, golden cases three ways
cargo xtask generators reproduce            # two clean builds, compared with each other and the lock
cargo xtask generators measure              # the numbers below
```

Modules build in the `generator` profile (`opt-level = "s"`, LTO, one codegen unit, `panic = "abort"`, stripped, overflow checks on), each crate in its own cargo invocation with `--locked`, with the workspace and Cargo home remapped to `/src` and `/cargo` and the caller's `RUSTFLAGS` dropped. The modules are `no_std` (the SDK supplies dlmalloc and a trapping panic handler), so no `std` code or host path enters them except panic locations, which keep their source paths.

A module's bytes depend on its source to the line and column, since panic locations are kept: any edit of a generator, the SDK or the ABI changes every affected module's hash, and `check` fails until `build --update-lock` records it.

**Golden cases.** Each crate's `golden.json` holds requests with complete parameters and the result hash of each. `check` runs them natively (the crate's own test), under wasmi natively, and under the probe, which is wasmi built for wasm32 running inside the native wasmi; all three must give the recorded hashes, and the probe must also report the same fuel as the native host.

## Reproducibility (11 November 2026)

- Two clean builds on macOS (aarch64, Nix shell, rustc 1.96.0): identical bytes (`cargo xtask generators reproduce`).
- The bytes are reproducible per build host, not across hosts. The same sources and rustc 1.96.0 gave three different modules of the same size (281 021 bytes): macOS aarch64 `c400a194…`, Linux aarch64 `7068941e…` and Linux x86_64 `ce8027d9…` (official `rust:1.96.0` containers). Two Linux hosts differing only in architecture differ too, so the likely cause is the host triple, not paths or the OS: cargo mixes `rustc -vV` (which names the host) into each crate's `-C metadata`, and the 53 469 differing bytes start in the function and type tables, consistent with a reordering. The mechanism is not confirmed beyond this.
- So `modules.lock` keeps one hash per build host, and `check` compares the hash for the host it runs on: a different hash fails, a host with none recorded gets a note with the hash it built. Windows has none yet; there the source paths in panic locations also differ (backslashes).
- A module changed on one host drops the other hosts' hashes when the lock is updated; rebuild there (or in a container, as above) to record them.

**Host-independent builds (14 November 2026, slice B4).** The cause is confirmed: cargo's `-C metadata` seeds each crate's stable id, which orders the module's functions. Every generator build now runs rustc through `xtask` as its rustc wrapper (`generators::wrap_rustc`), which replaces cargo's value with a hash of what identifies the crate within the build (package name and version, crate name and types, the `--cfg` set and the target), none of which names the host. Measured on macOS aarch64: two builds whose cargo metadata differs (`__CARGO_DEFAULT_LIB_METADATA` set or not) gave identical bytes with the wrapper, and different bytes without it (the wrapper also changed the hash, as expected). Not yet measured on Linux: the Linux x86_64 hash is to be recorded on atlas-dev; if it equals the macOS hash, `modules.lock` can record one hash for every host. Either way part identity no longer depends on the build: a part key is the generator's id and parameters, and a shipped module rebuilt to other bytes at the same version makes the same part (G15).

## Measurements and limits (11 November 2026)

Measured with `cargo xtask generators measure` on an Apple M-series Mac, macOS, host built in the `ci` profile (wasmi at opt-level 2, portable dispatch), median of 21 runs.

| Module | Bytes | Features used |
| --- | --- | --- |
| chip | 281 021 (274 KiB); 283 970 naming its part files (GP2); 234 868 without `serde_json` or floats (B3, 14 November 2026) | sign-extension, bulk-memory, reference-types (floats refused since B3) |
| probe (host for wasm32, test only) | 1 772 010 | — |

The feature list is measured: each allowed feature is switched off in turn and the module validated without it. Mutable globals, multi-value and saturating float-to-int are allowed but unused; SIMD, relaxed SIMD, threads, memory64, multi-memory, tail calls, extended constants, custom page sizes, wide arithmetic and GC are refused.

| Call | Fuel | Time |
| --- | --- | --- |
| load (check and compile all functions) | — | 4.2 ms |
| describe (24 presets, 14 241 bytes) | 6 033 459 | 8.7 ms cold (load included) |
| generate, package scope (1 178 bytes) | 1 047 731 | 5.2 ms cold, 0.85 ms warm |
| generate, resistor 0603 component (2 117 bytes) | 1 479 896 | 6.0 ms cold, 1.3 ms warm |
| generate, largest case: LED 0603 (2 726 bytes) | 1 825 471 | 6.2 ms cold, 1.4 ms warm |

Requests are 248–258 bytes. Linear memory ends at 1 152 KiB on every call (the 1 MiB stack plus heap). Fuel runs at about 1.1 G per second; it grows by about 460 per response byte.

Limits (`sparksmith_generators::Limits::DEFAULT`, `v1::limits`), set from these numbers:

| Bound | Note's target | Set | Why |
| --- | --- | --- | --- |
| Module size | 2 MiB | 1 MiB | 3.7 × the chip module |
| Fuel per call | 50 M | 500 M | A response at the 1 MiB bound needs about 0.5 G; 83 × describe; about 0.5 s natively |
| Linear memory | 64 MiB | 32 MiB | 28 × the measured 1.1 MiB |
| Request | 64 KiB | 64 KiB | 250 × the measured requests; room for pin-name lists |
| Description | (1 MiB) | 256 KiB | 18 × the chip description |
| Response | 4 MiB | 1 MiB | 380 × the largest chip response; a 1 024-pad grid array projects to about 0.3 MiB |
| Per-kind counts | — | pads ≤ 1 024, terminals ≤ 1 024, graphics ≤ 4 096 per list, points ≤ 1 024, units ≤ 64, coordinates ≤ 1 m | `v1::limits` |
| Wall time | 2 s | 2 s per call (`worker::WALL`) | Fuel bounds the work; `sparksmith_generators::worker::Worker` runs each call on its own thread and refuses one past the wall time or cancelled (GP1) |

The fuel limit and the response bound are tied: raise them together. The projections for larger families are estimates until those generators exist (GP3, GP4).

## Adding a generator

1. `generators/<name>/` with a `Cargo.toml` like `chip`'s (crate type `cdylib` + `rlib`, `unsafe_code = "deny"` because the export macro allows it for `#[unsafe(no_mangle)]`, the SDK as the only dependency) and `#![cfg_attr(target_arch = "wasm32", no_std)]`.
2. Add it to the workspace members.
3. Implement `Generator` and call `export_generator!`.
4. Write `golden.json`, run `cargo test -p <crate> golden` (a mismatch prints the hash produced), check the geometry by hand-derived tests, then `cargo xtask generators build --update-lock`.
