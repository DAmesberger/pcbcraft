# Pcbcraft roadmap

The [product plan](docs/pcbcraft-plan.md) defines release scope. This file tracks execution; unchecked work is not implemented.

## Current milestone — prove the foundation

- [x] Inspect and pin the Solidcraft research baseline.
- [x] Record product, library, native integration and copper-performance requirements.
- [x] Adapt repository agent rules and task skills with upstream attribution.
- [x] Bootstrap the Rust workspace and executable checks.
- [x] Create native board/component geometry with retained placement expressions; test regeneration in a fresh upstream engine session.
- [x] Prove native parametric Solidcraft model composition and independent reopening for the board/component fixture.
- [ ] Prove volumetric copper, shared 2D/3D selection and measured local updates.
- [ ] Prove library identity/mapping updates and routing transactions on a small fixture.

Implement in the order and against the acceptance evidence in [Milestone 0](docs/milestone-0.md). An isolated prototype is not a shipped capability; record remaining shared-engine gaps explicitly.

The native fixture proof is complete; see [foundation evidence](docs/foundation-evidence.md). The copper sample, desktop shell and first component/schematic/PCB editors are implemented; see [editor evidence](docs/editor-evidence.md). Editor development proceeded in parallel at the user’s request. Milestone 0 remains open: scaling/local-invalidation benchmarks and complete library-update/routing proofs are not yet satisfied.

## Working editor foundation

- [x] Shared Solidcraft UI dependency, source-referenced shell, fonts, controls and renderer.
- [x] Component symbol/footprint/mapping authoring and immutable local snapshots.
- [x] Single-sheet terminal wiring, schematic properties and basic electrical checks.
- [x] Explicit schematic-to-board sync, placement, manual routing and physical-island checks.
- [x] Parametric PCB/pad/trace/envelope `.solidcraft` generation and independent engine regeneration tests.
- [x] Atomic `.pcbcraft` save, offline reopen, shared command engine and undo/redo.
- [ ] Complete first-release editors, library updates, recovery and manufacturing workflow.

## Subsequent milestones

1. Component authoring, project snapshots, single-sheet schematic and ERC.
2. Schematic-to-board updates, placement, routing, DRC and integrated 3D.
3. Pours, manufacturing output, native Solidcraft delivery, updates and recovery; complete the first-release workflow.
4. Release 2: interactive push-and-shove if not delivered in release 1. Extend other features according to real board needs.

## Current hardening sequence

At the user's request, execute in this order:

1. **Component and schematic updates:** revision-bound impact previews, explicit affected-copper handling and one-step undo. Implemented, including live airwire preview during component dragging; tests cover stale/cancel/locked/failed updates.
2. **File handling and recovery:** native dialogs, unsaved-work transitions, portable unpublished drafts and crash recovery. In progress.
3. **Performance:** measure representative boards, then remove whole-model reconstruction from local edits. Keep timings and remaining limits explicit.

Parametric design is an additional user requirement: author footprints and board geometry using Solidcraft's parameter/expression facilities. Placement references must support board edges and other components, with independently selectable placement origin, body center and pad-center anchors. Rotation and bottom-side transforms must be explicit. Persist dependencies and stable electrical identities, reject cycles/missing references, and preview footprint-driven routing changes. This follows the hardening sequence; numeric editor fields alone do not satisfy it.
