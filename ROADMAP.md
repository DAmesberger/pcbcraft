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
- [x] Initial component forms for symbol pins/body, rectangular footprint pads and mappings, with immutable local snapshots; complete authoring scenarios remain pending below.
- [x] Single-sheet terminal wiring, schematic properties and basic electrical checks.
- [x] Explicit schematic-to-board sync, placement, manual routing and physical-island checks.
- [x] Parametric PCB/pad/trace/envelope `.solidcraft` generation and independent engine regeneration tests.
- [x] Atomic `.pcbcraft` save, offline reopen, shared command engine and undo/redo.
- [x] One typed engine for UI, CLI and MCP; persistent net identity; engine-owned definition drafts; file dialogs, background saves and crash recovery.
- [ ] Complete first-release editors, library updates and manufacturing workflow.

## Component-editor acceptance

The [accepted authoring contract and scenarios](docs/part-library-design.md#accepted-authoring-contract) define target behavior, not current capability. New Component is the entry point; component categories and templates are optional. Basic multi-unit symbols and structured multi-function IOs with instance-level selection belong to the first complete editor. Establish unit/function identities against CE-04–CE-06 before implementing the CE-01 UI, then deliver the scenarios in order within the component-editor phase. The [component model](docs/component-model.md) is the implementation contract for the definition, typed-field and terminal-function slices that precede CE-01. Slice 1 (revisioned definitions, drafts, exact references and multi-unit symbol occurrences) is implemented, and the headless CE-04/CE-05 steps listed in the [editor evidence](docs/editor-evidence.md#verification) pass through the engine. Slice 2 (revisioned quantity kinds and typed fields, component field selections and typed instance values with reviewed field updates) is implemented headlessly, with text editing of symbol-shown fields in the schematic inspector; fields are not yet authored in the component editor. Slice 3 (terminal function arrays, device-NC and reserved classes, shared resources with capacity and constraints, per-instance function selections, availability per implementation and function-aware ERC) is implemented headlessly, with the headless CE-06 function steps listed in the editor evidence; the desktop still authors one fixed function per terminal and has no pin-configuration table. The scenario boxes below stay open until their UI, automation and native-model evidence exists.

- [ ] CE-01: generic resistor, reusable typed fields, alternate symbols and independently revisioned physical choices.
- [ ] CE-02: non-polarized/polarized capacitors, tolerance semantics, polarity and manufacturer requirements.
- [ ] CE-03: inductor, derived fields, generator recipes, manual overrides and offline regeneration.
- [ ] CE-04: dual transistor, independently placed units and one physical component.
- [ ] CE-05: dual amplifier, explicit power unit and unused-unit handling.
- [ ] CE-06: MCU-style IC, structured pin-function arrays/directions, resource constraints, repeated supply connections and exposed-pad obligations.
- [ ] CE-07: connector, electrical shield, mechanical holes, package variants and model datums.

Open decision before the first release: a symbol at every graphics limit encodes to about 1.2–1.4 MB of pretty JSON, so only eleven to thirteen such revisions fit the 16 MiB project file. Choose a compact encoding, pruning of unreferenced revisions or lower vertex limits; see [encoded size](docs/component-model.md) in the component model.

All scenarios require the common UI/automation, transactional, offline and native-model evidence in the spec. The [Rhai integration proof](docs/part-library-design.md#rhai-integration-proof) passes its bounded arithmetic/generation tests, but does not complete any full scenario. Cross-sheet hierarchy, automatic gate/pin swapping and firmware generation remain separate work.

## Subsequent milestones

1. Component authoring, project snapshots, single-sheet schematic and ERC.
2. Schematic-to-board updates, placement, routing, DRC and integrated 3D.
3. Pours, manufacturing output, native Solidcraft delivery, updates and recovery; complete the first-release workflow.
4. Release 2: interactive push-and-shove if not delivered in release 1. Extend other features according to real board needs.

## Current hardening sequence

At the user's request, execute in this order:

1. **Component and schematic updates:** revision-bound impact previews, explicit affected-copper handling and one-step undo. Implemented, including live airwire preview during component dragging; tests cover stale/cancel/locked/failed updates.
2. **File handling and recovery:** native dialogs, unsaved-work transitions, portable unpublished drafts and crash recovery. Implemented; macOS Cmd+Q still quits without the unsaved-work prompt (work survives in recovery) until a native app menu routes Quit through the close request, as Solidcraft's does.
3. **Performance:** measure representative boards, then remove whole-model reconstruction from local edits. Keep timings and remaining limits explicit.

Parametric design is an additional user requirement: author footprints and board geometry using Solidcraft's parameter/expression facilities. Placement references must support board edges and other components, with independently selectable placement origin, body center and pad-center anchors. Rotation and bottom-side transforms must be explicit. Persist dependencies and stable electrical identities, reject cycles/missing references, and preview footprint-driven routing changes. This follows the hardening sequence; numeric editor fields alone do not satisfy it.
