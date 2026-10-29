# Headless foundation evidence

Historical report for the initial headless probe. Later desktop, copper and editor work is recorded in [editor evidence](editor-evidence.md); the pending statements below describe this earlier stage.

Implementation date: 29 October 2026. Tested on macOS arm64 with Rust 1.96.0. Solidcraft engine and geometry patches are pinned to `4f3ac07aec69489183185071f07d7fbcf4f51e64`; no upstream source changes were needed.

## Implemented

- Three workspace packages: a headless Solidcraft adapter, JSONL/probe CLI and `xtask` checks. Dependencies resolve from git/crates.io with a checked-in lockfile, without an adjacent checkout.
- Typed commands and generated JSON schemas, revision-checked parameter updates, staged validation before document commit, undo/redo and structured errors. File saves are separate external effects and refuse existing destinations.
- Solidcraft-native substrate box, cylindrical hole cut and asymmetric package box. Two occurrences reuse one component definition. Native planar joints retain independent X/Y/rotation expressions, top attachment to board thickness and bottom orientation. A nonzero package offset exercises the attachment frame.
- Native container save without a geometry cache or preview, fresh-engine reopen and regeneration, followed by another native save/reopen. There are no imported-geometry features in the fixture.

## Checks completed

`cargo xtask ci` passed: rustfmt, workspace dependency boundaries, strict Clippy, five native regression tests and adapter compilation for `wasm32-unknown-unknown`.

The tests cover analytic dimensions/volume/centroids after native regeneration, shared definition and occurrence identity, stale revisions, invalid and nonfinite inputs, failed open, save-overwrite rejection, and geometry restoration with undo/redo. The JSONL executable was additionally exercised for schema output, open/inspect, malformed-input recovery and its 64 KiB request limit.

The generated file is `artifacts/fixture.solidcraft` (ignored build output). Reproduce it with the README command using a new filename. Default substrate dimensions are 40 × 30 × 1.6 mm; the 3 mm diameter hole is centered at (4, 5) mm. Its measured volume is 1908.69214533 mm³, agreeing with `(40 × 30 − π × 1.5²) × 1.6`. Scalar checks use tolerance `1e-5 + abs(expected) × 1e-6` in the measured units, covering floating-point/geometry evaluation error.

The component definition starts at local (0.25, −0.5, 0.3) mm and measures 4 × 2 × 1.2 mm. Top placement is (12, 10, 1.6) mm; bottom placement is (26, 18, 0) mm with 30° rotation and a proper rigid flip. The top model spans Z = 1.9…3.1 mm and bottom Z = −1.5…−0.3 mm. These offsets are intentional test geometry, not complete electronic components.

`examples/native-regenerate.json` changes the thickness to 2.4 mm, hole center to (6, 7), top placement to (17, 13) at 90°, bottom placement to (28, 20) at −45°, and shared model dimensions to 5 × 2 × 1.8 mm. Expected substrate volume is 2863.038217995 mm³; each package volume is 18 mm³. Expected top bounds are (15.5, 13.25, 2.7)…(17.5, 18.25, 4.5) mm. Bottom Z bounds are −2.1…−0.3 mm. Tests independently derive all rotated bounds and centroids rather than comparing export output with itself.

## Independent Solidcraft executable

Built the standalone `solidcraft-cli` from the unchanged pinned checkout with its own lockfile, `--locked --profile ci --no-default-features`. Its build artifacts are in Pcbcraft's `target/solidcraft-check`; the Pcbcraft dependency build remains independent of that checkout.

Both standalone commands documented in the README passed. Reports are in `artifacts/upstream-original.json` and `artifacts/upstream-regenerated.json`. Separate analytic checks against both reports confirmed three closed bodies, no failed features, expected substrate volume/centroid and expected top/bottom package volumes and rotated world bounds. The second process regenerated the saved native history without Pcbcraft code.

Solidcraft's standalone GPU snapshot command also succeeded, and the resulting `artifacts/fixture.png` was visually inspected: board thickness, open mounting hole, top package and bottom package are visible. The substrate is temporarily translucent for this diagnostic view. This is a model rendering, not evidence of a Pcbcraft UI or interactive performance.

```sh
solidcraft-cli snapshot fixture.solidcraft --out fixture.png \
  --width 1000 --height 700 --view iso --no-grid --no-axes \
  --body Substrate:opacity=0.6
```

## Still pending

No desktop interaction or UI parity is claimed yet. The wasm result is a compile check, not a browser execution test, and remote CI has not run.

The probe uses fixture names to find known definitions/joints and a fixed board origin. It is not the electronics document or library identity scheme. Arbitrary external native-model history composition, parameter scoping, project asset snapshots, nonzero board origins and export refresh ownership remain to be implemented/tested.

Copper layers/traces/pads/vias/zones, object/net picking, the egui shell, scaling benchmarks, library transactions and routing are next milestone work. No interactive performance claim follows from this small headless fixture. Updates currently reconstruct a candidate Solidcraft session for rollback safety; that is not the eventual incremental routing/rendering path.
