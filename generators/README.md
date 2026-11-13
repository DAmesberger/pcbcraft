# Generator modules

A generator is a pure function in a WebAssembly module: typed parameters in, a complete part body out (symbol, package and footprint; the 3D model description arrives with slice GP2). Each directory here is one generator crate that builds to `wasm32-unknown-unknown`. The modules are loaded as data, never linked into the app; the host that runs them is `crates/generators` (`sparksmith-generators`).

Status: slice GP0, the foundation. The modules are not yet used by the catalog, the gallery or the part editor (GP1, GP5, GP7); the in-code templates stay until GP5. ABI 1 is open to change until the first release.

## Layout

| Path | What |
| --- | --- |
| `crates/generator-abi` | `sparksmith-generator-abi`: the ABI 1 wire types (`v1`), the canonical JSON codec, the parameter check, and the `GeneratorRuntime` trait. `no_std` + `alloc`, no generator logic. |
| `crates/generator-sdk` | `sparksmith-generator-sdk`: what a generator is written with: `Generator` and `export_generator!`, the module's allocator and panic handler, typed parameters, symbol drawing, footprint drawing (courtyard, silkscreen broken round the pads, assembly), and the golden-test harness. |
| `crates/generators` | `sparksmith-generators`: the wasmi host, with `Limits` and the output checks. |
| `crates/generators-probe` | A test module: the host itself built for wasm32, so the native tests can show the wasm32 build gives the same results. Never shipped. |
| `generators/<name>/` | One generator: `Cargo.toml` (cdylib), `src/`, `presets.json`, `golden.json`. |
| `generators/modules.lock` | Each module's SHA-256 per build host, committed. |

`cargo xtask layers` keeps it apart: a generator crate depends on the SDK and the ABI only, only generator crates depend on the SDK, nothing depends on a generator crate or on the probe, and the ABI crate depends on no workspace crate (so `sparksmith-core` can use its types and trait without an interpreter).

## ABI 1

A module imports nothing and exports its memory as `memory` and four functions:

| Export | Signature | Meaning |
| --- | --- | --- |
| `sparksmith_gen_abi` | `() -> u32` | Returns 1. |
| `sparksmith_gen_alloc` | `(len: u32) -> u32` | A buffer for the request, or 0 when `len` is 0 or over 64 KiB. |
| `sparksmith_gen_describe` | `() -> u64` | `(ptr << 32) \| len` of the canonical `Description`. |
| `sparksmith_gen_generate` | `(ptr: u32, len: u32) -> u64` | Reads the canonical `Request` from the buffer and returns `(ptr << 32) \| len` of the response: a status byte (0 Ok, 1 Err), then the canonical `Generation` or `{"errors":[…]}`. |

The host checks a module before anything runs (size, a valid core module of the allowed features, no imports, no start function, the four exports and the memory, memory within the limit), then gives every call a fresh instance with a fuel budget and a memory cap, decodes the reply with the canonical decoder and checks it: unique keys, every reference resolving (pads to package terminals, symbol pins and binding to component terminals), values in range, and the parts the scope requires present and no others.

**Canonical JSON.** Object keys sorted by their UTF-8 bytes, each once; no whitespace; integers only (`i64`/`u64`, shortest form, so no floats, exponents or `-0`); strings as `serde_json` escapes them, otherwise literal UTF-8, and in Unicode NFC (checked by the host); absent optional fields omitted, never `null`. The decoder accepts only bytes the encoder would write. Lengths are integer nanometres and angles integer millidegrees.

**Request.** `{abi: 1, scope: component | package, params: {key: value}}`. The parameters are complete: the host fills every default from the description first (`sparksmith_generators::params::complete`), so the part key never depends on a module's defaults. Parameters a project setting decides (the least chip land gap, the resistor symbol style) are declared as `context` parameters and frozen into the request.

**Identity.** Module hash: SHA-256 of the module bytes. Result hash: SHA-256 of the canonical `Generation` bytes (after the status byte). The part key (module hash with the params hash) is GP1's.

**Refusals** are `{code, field?, message}` with a sentence a person can act on: `param.unknown`, `param.missing`, `param.type`, `param.out_of_range`, `param.inconsistent` and `param.scope` for parameters; `generator.too_large`, `generator.invalid_module`, `generator.unsupported_feature`, `generator.has_imports`, `generator.missing_export`, `generator.abi_mismatch`, `generator.out_of_fuel`, `generator.memory_limit`, `generator.trapped`, `generator.output_too_large`, `generator.request_refused` and `generator.invalid_output` from the host. Every host refusal ends in "It was not run." or "Nothing was changed."

## Build and check

```sh
cargo xtask generators build                # build every module, print sizes and hashes
cargo xtask generators build --update-lock  # … and write modules.lock (the only way it changes)
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
- So `modules.lock` keeps one hash per build host, and `check` compares the hash for the host it runs on: a different hash fails, a host with none recorded gets a note with the hash it built. The lock records macOS aarch64, Linux aarch64 and Linux x86_64 (the CI runner). Windows has none yet; there the source paths in panic locations also differ (backslashes).
- A module changed on one host drops the other hosts' hashes when the lock is updated; rebuild there (or in a container, as above) to record them.

## Measurements and limits (11 November 2026)

Measured with `cargo xtask generators measure` on an Apple M-series Mac, macOS, host built in the `ci` profile (wasmi at opt-level 2, portable dispatch), median of 21 runs.

| Module | Bytes | Features used |
| --- | --- | --- |
| chip | 281 021 (274 KiB) | sign-extension, bulk-memory, reference-types, floats |
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
| Wall time | 2 s | not enforced in GP0 | Fuel bounds the work; the worker thread with cancellation comes with GP1 |

The fuel limit and the response bound are tied: raise them together. The projections for larger families are estimates until those generators exist (GP3, GP4).

## Adding a generator

1. `generators/<name>/` with a `Cargo.toml` like `chip`'s (crate type `cdylib` + `rlib`, `unsafe_code = "deny"` because the export macro allows it for `#[unsafe(no_mangle)]`, the SDK as the only dependency) and `#![cfg_attr(target_arch = "wasm32", no_std)]`.
2. Add it to the workspace members.
3. Implement `Generator` and call `export_generator!`.
4. Write `golden.json`, run `cargo test -p <crate> golden` (a mismatch prints the hash produced), check the geometry by hand-derived tests, then `cargo xtask generators build --update-lock`.
