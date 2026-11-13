# Milestone 0 — prove the shared foundation

The first executable result is a small Sparksmith window showing one board in 2D and 3D, with selectable volumetric copper and native parametric Solidcraft save/reopen. It proves the expensive and difficult boundaries before building complete editors.

The product requirements remain in [the general plan](sparksmith-plan.md). This milestone deliberately uses a small programmatically constructed fixture; it does not replace the first release's component, schematic and PCB editors.

## 0A — reproducible workspace and gates

Bootstrap the minimum Rust workspace needed for a document/command fixture, Solidcraft integration, desktop shell and checks. Use Solidcraft's Rust/egui/wgpu base and pin a tested upstream revision. Start from the inspected `4f3ac07aec69489183185071f07d7fbcf4f51e64` unless newer source resolves a demonstrated gap; record why the baseline changes.

Test dependency integration immediately. The Solidcraft workspace has patched dependencies, so confirm required patches and feature flags in a clean Sparksmith build rather than assuming a git dependency inherits them. A neighboring checkout can support development, but reproducible builds must not depend on an undocumented absolute path.

Implement a small `cargo xtask ci` entry point and connect it to CI once a repository host is configured. Initially cover formatting, strict Clippy, meaningful native tests and wasm checks for applicable libraries. Add executable dependency-layer, command-schema, asset/license and UI-container checks as those surfaces appear. Keep `AGENTS.md` and command documentation accurate about implemented gates.

Configure production panic/unchecked-input linting and forbidden application unsafe code. Keep test-only helpers separate. Preserve applicable licenses for adopted source and assets.

**Done:** a fresh checkout builds the fixture target and runs the implemented checks with documented commands; Solidcraft dependency resolution is reproducible; checks have real coverage rather than placeholders.

## 0B — native parametric model proof

Construct a two-layer rectangular board with a hole, a repeated component definition and asymmetric components on both sides. Use Solidcraft's engine to create board geometry and native component models. Include independently editable placement parameters and model-to-package offsets.

Save `fixture.solidcraft` through its native writer. Reopen in the matching standalone Solidcraft build, change board thickness, a hole position, one component's X/Y/rotation and a model dimension. Measure the result against explicit expected placements and dimensions. Verify the two occurrences share geometry definitions while retaining independent placement.

The current move/link APIs do not by themselves prove persistent dependencies or preserved history. Identify suitable existing features or implement the smallest required shared-engine extension in an isolated Solidcraft checkout, following its local instructions. Keep the baseline checkout unchanged. Record required commits/capabilities for both applications. Local source work does not imply upstream publication.

**Done:** the native file recomputes correctly without Sparksmith and without inaccessible model assets. A frozen model saved with a different extension is a failed probe. Record any upstream gap rather than substituting STEP.

## 0C — real copper and shared selection

Add connected tracks, pads, a through via and a simple filled zone with a hole/clearance. Include a deliberately disconnected object on the same net. Copper geometry uses actual thickness/elevation and an empty drill with plated barrel where applicable.

Use the same source object/net/layer IDs in the 2D and 3D presentations. Selecting a trace resolves its properties; highlighting its net highlights all members, including the disconnected island. Inspect the physically connected island separately. Switching views preserves selection and uses the same committed design revision.

Test existing Solidcraft facilities first. Where required, add a compact native copper feature to the shared engine with retained parameters, identity and local cache invalidation. Any new native feature must reopen and recompute in the compatible Solidcraft application too.

Build a minimal shell from Solidcraft's tokens, controls, containers and navigation. Keep the fixture, trace-width edit, highlight action and view toggle callable through the same engine commands as the UI. The shell is the start of the real UI, not a separate web mockup.

**Done:** inspected screenshots/recording show physical copper, correct picking and shared selection in both views; native save/reopen retains geometry parameters and identities. Selection/camera counters show zero geometry rebuilds.

## 0D — measure scaling before expanding

Generate deterministic 1k/10k/100k copper-primitive cases and separate cases for dense vias, repeated components and complex zone holes. Record machine/backend/resolution and the latency, geometry-rebuild, draw-call, upload and memory metrics in [the performance research](copper-performance-research.md).

Measure cold construction/opening, warm orbit/view switching, hover/picking, full-net highlighting, one early and one late trace edit, a multi-track speculative edit, zone refill, and native save/reopen. Simulating the edit set exercises the future shove boundary; it does not count as implementing the shove algorithm.

Use the plan's provisional 60 Hz navigation and p95 under 50 ms feedback goals on a declared reference workload. The 100k case is a scaling probe. If local updates grow with unrelated board size, resolve the invalidation problem before adding editor breadth. Keep expensive completion latency separate from UI feedback latency.

**Done:** a reproducible report includes actual results, workload/hardware definitions and remaining bottlenecks. No benchmark values are invented or borrowed from upstream CAD workloads.

## 0E — library and routing data proof

Use a passive, a connector and an IC with repeated supply terminals/exposed pad. Create their component/symbol/package/footprint/binding definitions through fixture commands. Pin exact revisions and required assets in a project snapshot.

Exercise rename/reannotation, compatible footprint changes, an ambiguous mapping change, and moving a pad away from a track. Verify stable identities, an intelligible impact preview, atomic cancellation/application, undo and offline reopening. Unrouted same-net terminals remain detectable.

Implement the minimal routing topology and speculative edit transaction used by manual routes now and push-and-shove later: segment splitting/moving, chain/junction connectivity, fixed obstacles and locked copper. Do not build the full shove solver in this probe unless the scope is explicitly expanded.

**Done:** tests exercise these invariants through the command engine; evidence shows the model can support the planned editors and second-release shove behavior without substituting names for identity.

## Handoff to editor development

Deliver the runnable fixture, checked commands, native sample, visual evidence, benchmark report and a short list of any unresolved engine/model decisions. Mark completed steps in [ROADMAP.md](../ROADMAP.md). Then build component authoring and schematic capture on the proven boundaries.

This milestone is in progress. The headless Rust workspace, aggregate checks and native parametric probe are implemented. See [foundation evidence](foundation-evidence.md) for completed checks and remaining work. The desktop shell, copper fixture and initial editors are now implemented; see [editor evidence](editor-evidence.md). Large-board scaling, incremental updates and complete library-update/routing proofs remain pending. Editor breadth proceeded at the user’s explicit request; it does not complete the unmeasured performance gate.
