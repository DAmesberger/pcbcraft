# Footprint evaluator spike (F0)

Status: spike, finished. The prototype was test-only code in `crates/solidcraft-integration/src/footprint_spike/` (`#[cfg(test)]`, not a product API); it was removed once the product evaluator in `crates/solidcraft-integration/src/footprint/` carried its golden-hash tests, and remains in git history (the scratch-`Session` bridge is the starting point for F6). No core type, command or UI changed. Solidcraft revision: `4f3ac07aec69489183185071f07d7fbcf4f51e64`.

It answers one question from the footprint design (slice F0): can Sparksmith drive parametric footprints with Solidcraft's public sketch solver, keep stable pad identity, and get the same result on native and wasm? **Yes.** The numbers below are measured, not targets.

## Method

The prototype uses only the `solidcraft_engine::{sketch, doc::expr}` re-exports, plus `Session` for the FD2b probe. Its pipeline follows design §2.5:

1. **Revision check.** The source's evaluator revision must equal `SOLIDCRAFT_REVISION`.
2. **`check_sketch`.** The embedded sketch is canonical JSON. The check:
   - bounds the input: ≤ 1 MiB, ≤ 4,096 points, ≤ 4,096 curves, ≤ 8,192 constraints, |coordinate| ≤ 1,000 mm, all values finite;
   - parses it, re-serializes it and compares the two as `serde_json::Value`. Any field that would be dropped, added or changed is refused;
   - enforces the footprint subset: no links (sketch, point or curve), offsets, wires, patterns, 3D, free-form curves, or pattern and offset relations;
   - requires point 0 to be the fixed origin and every id to be unique;
   - checks each curve's point indices and radius, and calls `Sketch::validate` on each constraint.
3. **Parameters.** `doc::expr::eval_params` evaluates footprint parameters and the injected package values `pkg_<name>`. Names must be identifiers. The `pkg_` prefix and Solidcraft's reserved words (units, `pi`, functions) are refused.
4. **Apply.** Point expressions are evaluated and their points fixed, mirroring `Document::apply_point_values`. Then each dimension whose Solidcraft `param` field names a parameter gets its value through `Constraint::set_value`. Together that is about 40 lines on public APIs.
5. **Solve.** `solve_after_edit_with_analysis(sk, old_values, Some(stored_positions), Analysis::Full)`. A failure, a fold or a `flips_from_stored` result is a finding.
6. **Copper-only profile pass.** On a copy of the sketch, every curve that is not a pad's role-owned copper is marked construction. Then `find_profiles` runs.
7. **Classification.** `profile_at_point(anchor)` finds the pad's profile. That profile must be bounded by exactly the pad's owned curves, and its area and geometry must match the template: a rectangle (four sides with the anchor at the diagonal midpoint) or a circle centred on the anchor.
8. **Arrays.** Each array is rigid copies of the solved seed, made outside the solver:
   - quarter turns use an exact table;
   - the layouts are dual-row, quad and grid;
   - skips are an explicit list or a centred void;
   - numbering is sequential or grid-alpha (row letters without I, O, Q, S, X and Z).
9. **Quantize and hash.** Results are quantized to 1e-6 mm (1 nm) and to micro-degrees by Sparksmith's own quantizer. Two FNV-1a hashes are computed: one of the typed pads, and one of the raw f64 bits of every solved point.

Pad identity is `PadKey::Anchor(point id)` or `PadKey::Array { array, i, j }`. It is never geometry, profile order or pad number.

**Fixtures.** All three are original synthetic geometry with round numbers, taken from no library or datasheet:

| Fixture | Contents | Driven by |
|---|---|---|
| SOIC-8 | Rectangular seed pad plus a dual-row array (2 × 4). A non-copper assembly outline drawn from `pkg_E` and `pkg_D` crosses the pads' inner ends. | `pkg_e` for pitch; row span, pad size and count as footprint parameters |
| QFN-48 + EP | Seed pad plus a quad array (4 × 12, pitch 0.5 mm), and a 5.2 mm exposed pad at the origin with a concentric non-copper drill circle | `pkg_e`, `pkg_D2` |
| BGA-256 | Circular seed land, 16 × 16 grid at 1.0 mm with the centre 4 × 4 removed: 240 lands | `pkg_e`, row and column counts |
| BGA-256 flat (comparison) | The same 240 lands drawn and dimensioned in the sketch: 240 circles, 720 dimensions, no array | — |

## Results

All tests pass natively. Every test except the native timing test also passes in Node (wasm).

- **Typed pads.** Centres, sizes, rotations and numbers are correct for all three fixtures. All three solve fully constrained (dof 0). The rigid-copy BGA gives exactly the same land centres and diameters as solving all 240 lands in the sketch.
- **Identity under parameter changes** (item 6):
  - **Pitch change** (`pkg_e` 1.27 → 1.0 on SOIC-8; 0.5 → 0.4 on QFN-48): same key set, new positions.
  - **SOIC count 4 → 7 per row:** every old key survives and six are added. Count 4 → 2: the survivors are exactly the keys still generated.
  - **BGA 16 → 18:** see "Problems found", item 4.
  - Anchor point ids survive every re-solve.
- **Copper-only pass is required.** Both failure modes in the design were reproduced without the filter:
  - SOIC: the assembly outline splits the pad's profile.
  - QFN: the drill disc is the innermost profile at the exposed pad's anchor.

  With the filter, both classify correctly.
- **Refusals and findings:**
  - wrong evaluator revision;
  - a parameter cycle, an unknown name, a unit mismatch, or an angle parameter driving a length;
  - a reserved, `pkg_`-prefixed or malformed name;
  - a missing package value;
  - a non-whole count;
  - an impossible size (`pad_w = -1 mm`, which gives `solve_failed`). The source is left unchanged.
- **`check_sketch` refusals:**
  - an unknown field on a point or at the top level;
  - an omitted defaulted field (the input is not canonical);
  - invalid JSON, or JSON that is not a sketch;
  - the byte, point and coordinate caps;
  - a constraint with an out-of-range index or the wrong entity kind;
  - a curve point index out of range;
  - a duplicate id;
  - a non-fixed origin;
  - 3D, a linked point, or an ellipse;
  - a sketch pattern or an associative offset made by Solidcraft's own commands.

### Timings (measured)

Machine: Apple M5 Max, 128 GB, macOS 27.2. Profile `ci` (crate opt-level 1, dependencies opt-level 2). Run single-threaded with `--test-threads=1`.

Each case has 200 evaluations. Values are p50/p95 in milliseconds.
- **same:** re-evaluate the stored source unchanged, which is what publish does. Solidcraft's per-thread `eval_params` cache hits.
- **edit:** each run changes `pkg_e` by a different amount (the land diameter for the flat case) from the stored solved state, so parameters are re-evaluated and the seed really moves.

Phases:
- **check:** `check_sketch`.
- **solve:** parameters, apply and solve.
- **extract:** copper pass and classification.
- **arrays:** expansion, quantization and hashing.

| Fixture | Sketch | Scenario | check | solve | extract | arrays | **total** |
|---|---|---|---|---|---|---|---|
| SOIC-8 | 1,844 B | same | 0.024/0.030 | 0.016/0.018 | 0.008/0.009 | 0.004/0.005 | **0.053/0.063** |
| SOIC-8 | | edit | 0.024/0.025 | 0.019/0.021 | 0.008/0.008 | 0.004/0.004 | **0.056/0.059** |
| QFN-48 + EP | 2,208 B | same | 0.030/0.033 | 0.020/0.025 | 0.014/0.016 | 0.020/0.021 | **0.085/0.097** |
| QFN-48 + EP | | edit | 0.030/0.031 | 0.023/0.025 | 0.014/0.015 | 0.020/0.021 | **0.089/0.095** |
| BGA-256 | 428 B | same | 0.006/0.006 | 0.004/0.005 | 0.002/0.003 | 0.091/0.100 | **0.105/0.116** |
| BGA-256 | | edit | 0.006/0.006 | 0.007/0.008 | 0.002/0.003 | 0.091/0.094 | **0.107/0.114** |
| BGA-256 flat | 74,107 B | same | 0.988/1.038 | 0.339/0.367 | 19.587/20.178 | 0.084/0.091 | **21.03/21.62** |
| BGA-256 flat | | edit | 0.993/1.056 | 0.369/0.398 | 21.376/21.849 | 0.084/0.094 | **22.82/23.43** |

- **Against the targets:** the design's targets are ≤ 5 ms (SOIC/QFN) and ≤ 20 ms (BGA seed plus array). The measured results are 50–200× below them.
- **The flat comparison** misses the 20 ms target. Almost all of its time is `find_profiles` over 240 circles. This confirms that repetition must stay outside the solver and the profile pass.
- **wasm (Node v24.21.0, same machine):** `Instant` is unavailable on `wasm32-unknown-unknown` and `Date.now()` has 1 ms resolution, so only means over many runs are reported. Each is about 1.5× the native p50 for the same scenario.

  | Fixture | Mean (same scenario) |
  |---|---|
  | SOIC-8 | 0.086 ms (2,000 runs) |
  | QFN-48 + EP | 0.127 ms (2,000 runs) |
  | BGA-256 | 0.159 ms (2,000 runs) |
  | BGA-256 flat | 52.5 ms (50 runs) |
- **Memory** was not measured. A counting global allocator would need `unsafe`, which the workspace forbids. As a proxy, the canonical sketch JSON is 0.4–2.2 KB for the array fixtures and 74 KB for the flat BGA.

### Determinism and native/wasm agreement

- **Across runs:** 21 evaluations per fixture give identical pads, identical quantized and raw-bit hashes, and identical canonical `sketch_out`. Re-evaluating the stored solved state reproduces the hash, which is the publish "stale" check.
- **Across targets:** `results_match_golden_hashes_on_this_target` asserts golden hashes recorded natively on macOS arm64. It passes in Node (`wasm32-unknown-unknown`, `wasm-bindgen-test-runner`). Both the quantized hashes and the raw f64-bit hashes of every solved point are identical, so for these fixtures the solver agrees bit for bit.
  - Not checked: x86_64 or Linux natively, and browsers other than Node's V8.
- **To reproduce the wasm run:**

  ```
  CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test -p sparksmith-solidcraft --lib --profile ci --target wasm32-unknown-unknown --target-dir target/wasm-test footprint_spike
  ```

  F2's product evaluator (`crates/solidcraft-integration/src/footprint/`) carries the same check as `results_match_golden_hashes_on_this_target`; `cargo xtask wasm-footprint` runs its tests in Node.

## FD2b: trim, fillet, offset and mirror without the fork

The probe (`bridge.rs`) creates a throwaway `Session::default()` and runs `sketch.create {plane: XY}`. It then writes the footprint sketch into the scratch document with `Arc::make_mut(&mut session.doc).sketch_mut(id)`, runs the public `Session::execute("sketch.…")` and reads the sketch back.

- **Works natively and on wasm.**
  - Fillet adds one arc.
  - Trim deletes an uncrossed line, or splits the SOIC assembly outline where it crosses pad 1.
  - Mirror about the sketch's own construction line adds four lines.
  - Offset followed by `sketch.offset.break` gives plain lines.

  All of these results pass `check_sketch`.
- **Cost (native, small sketch, including `Session::default()`):** each call is p50 0.04–0.09 ms, p95 ≤ 0.11 ms.
  - fillet 0.064/0.073;
  - trim 0.036/0.038;
  - mirror 0.071/0.110;
  - offset 0.090/0.100.
- **Required care:**
  - Sparksmith's dimension bindings (`Constraint.param`) and point expressions name footprint parameters that the scratch document does not have. Left attached, the command fails: `point assembly.p0 X: unknown parameter pkg_E` was observed. The bridge therefore detaches both and re-attaches them by id. A command that deletes an expression-driven point is refused.
  - Fillet and offset bind their new dimension to a parameter they invent in the scratch document (`d1`, with expression `0.2 mm`). The bridge reports it, so the caller must adopt it under a footprint parameter name, which can collide with a user name such as `d1`.
  - Mirror about a sketch axis (`line: "y"`) projects the axis in as a **link**, which the subset refuses. Only the sketch's own lines are allowed as mirror lines.
  - Offset is **associative** (an `OffsetChain`), which is refused. Breaking one output dissolves the whole chain, and the outputs are then under-constrained (dof 4).
  - New ids come from Solidcraft's counters (`l2`, `p1`, `k10`). Role propagation must read the reply's `created`, `trimmed` and `deleted` lists.
  - On wasm, `Session::execute`'s `catch_unwind` does not help, because panics abort there. The same is true of any direct solver call (not exercised).
- **The fork alternative.** The commands live in `engine/src/cmd/sketch_modify.rs` (2,911 lines) plus pattern, support and free-form helpers (about 750 lines). They depend on the engine's `edit()` helper, document parameters and `ProfileSel`. Exporting them from `solidcraft_sketch` means refactoring about 3.7k lines across a crate boundary, then carrying a permanent fork diff or getting upstream agreement.

**Recommendation for FD2b: use the scratch-`Session` bridge** for F6. Wrap it in a small typed integration-crate API:
- an allowlist of commands and options (no axis mirror lines; offset always followed by break);
- detach and re-attach of bindings;
- parameter adoption with renaming;
- role propagation from reply ids;
- pinned contract tests like these.

It is cheap, works on wasm, and needs no fork. Revisit the fork export only if the JSON contract churns across Solidcraft bumps, or if an operation cannot be expressed through commands.

## Problems found

1. **The canonical round trip is strict by design.** The input must be byte-for-value what serde writes; for example, `fixed: false` cannot be omitted. Sparksmith therefore must store only `serde_json::to_string(&sketch)` output.

   A Solidcraft bump that adds a serialized field will refuse every stored sketch until an explicit re-save. That is the intended behaviour (design §2.7), but it needs a re-save command and a CI corpus check.
2. **`Vec2` serializes as `[x, y]`,** not `{x, y}`. This only matters to anyone writing sketch JSON by hand.
3. **Repeated evaluation hits `eval_params`' per-thread cache.** Benchmarks must vary parameters (the "edit" scenario) to measure a cold path. Results are unaffected.
4. **A centre void re-centres on growth.** BGA 16 → 18 moves the void from rows and columns 6–9 to 7–10. The test pins this:
   - seven old keys are retired (row 10 and column 10 inside the new void);
   - the old void's row 6 and column 6 come back as new keys.

   Identity is kept for every key still generated, but growth retires pads. The update preview must list them.
5. **Rect rotation is only meaningful mod 180°.** Row 1 of the SOIC reports 180°, which is the same land as 0°. Unless rotation is canonicalized per shape symmetry, diffs will show spurious rotation changes.
6. **The spike's quad layout rotates whole rows about the array centre.** That is only correct when `nx == ny` and `span_x == span_y`. A non-square QFN (design `Quad { nx, ny, span_x, span_y }`) needs per-side placement: rotate the seed's shape, then translate to that side's own first position.
7. **Package values were injected as lengths only.** `pkg_n` and other counts need unit-less injection.
8. **`find_profiles` grows faster than linearly.** 240 circles cost about 20 ms natively and 52 ms on wasm. A hand-drawn footprint with hundreds of custom pads would exceed the targets.

## Design changes suggested for F1/F2

- **F1 source model:**
  - Reuse Solidcraft's own `Constraint.param` field for the dimension → parameter binding. It already carries a name, survives bridge commands by constraint id, and needs no separate binding table in `FootprintSource`.
  - Package values carry their kind (length, angle or count). `PackageForm` emits typed `pkg_*` values.
- **F1 `Pad`:** canonical rotation per shape symmetry:
  - Rect, RoundRect and Oblong: [0°, 180°);
  - Circle: 0;
  - Custom: [0°, 360°).
- **F2 evaluator:**
  - Keep the pipeline as prototyped. The glue is small: point expressions plus dimension values come to about 40 lines, not 60.
  - Store only canonical sketch JSON.
  - Add a fixture-corpus check that runs `check_sketch` over stored sketches on every Solidcraft bump.
  - Add an `xtask` wasm job that runs the evaluator tests in Node against golden hashes. The agreement held bit for bit here, so the golden-hash test is cheap insurance.
- **F2/F3 copper pass:** keep all repetition out of the sketch. For large custom-drawn footprints, run the copper pass per connected copper cluster (bounding-box grouping), then check cluster overlap separately (`footprint.pad_shape_unclassified` for shared profiles).
- **F3 arrays:**
  - Use per-side placement for quads (see item 6).
  - Decide whether skips are relative to the centre (they re-centre and retire ids on growth, as now) or to the edges. Either way, list retirements in the impact preview. The spike shows identity by `(array, i, j)` holds across pitch and count changes.
- **F6 (FD2b):** the bridge, with the safeguards above.
- **Performance targets:** the 5 ms and 20 ms targets have large headroom. The drag-preview target (16 ms) was not exercised; F4 should measure `solve_with(…, Budget::frame())` on the seed only.
