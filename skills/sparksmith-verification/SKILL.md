---
name: sparksmith-verification
description: Add or run Sparksmith tests, benchmarks and CI; verify crash fixes, geometry changes or milestone completion.
---

# Verification and evidence

Run `cargo xtask ci` for the current aggregate gate; see [development](../../docs/development.md) for prerequisites and narrower checks. The wasm gate checks compilation, not browser execution. [Milestone 0](../../docs/milestone-0.md) defines the remaining acceptance evidence; report unimplemented gates explicitly.

Choose evidence by the changed behavior:

| Behavior | Required evidence |
| --- | --- |
| Commands and transactions | Observable result; rejection leaves state unchanged; undo/redo; malformed input |
| Library changes | Mapping/identity invariants; affected routing; offline snapshot reopening |
| Electrical/geometry checks | Hand-derived expected nets, gaps, dimensions and known deliberate violations |
| Native Solidcraft integration | Save/reopen in compatible Solidcraft; change parameters and measure regeneration without Sparksmith |
| UI | Real interaction and inspected screenshots/recording; scenario coverage for the affected behavior |
| Performance | Reproducible workload and machine/settings; latency distributions and invalidation/upload counters |
| Crash/data-loss fix | Reproducer and regression; file/transaction state remains recoverable |

For whole-app UI flows, drive the headless app through its accessibility tree with the `Ax` harness (`apps/sparksmith/src/app/ax_harness.rs`, flows in `ax_tests.rs`): find widgets by role and name, click and type with real events. A widget a test cannot find by name needs an accessible label, not a pixel position.

Run focused checks during development and the implemented aggregate gate before merging or handing off executable changes. Broaden testing when a shared boundary changes. Documentation-only work needs link/skill validation, not a full CAD build. Do not add superficial tests that only mirror implementation text.

Expected values come from analytic reasoning, public specifications or independent permitted references. An implementation's own export/reimport agreement alone does not establish manufacturing correctness. Keep known limitations explicit; never adjust expected values just to match a wrong result.

Place regression tests with their area. Performance reports distinguish targets, historical upstream numbers and measurements of this checkout. Report unavailable platforms, unrun checks and remaining API gaps accurately. A milestone is complete only when its exit evidence exists, not when its command list or UI skeleton exists.
