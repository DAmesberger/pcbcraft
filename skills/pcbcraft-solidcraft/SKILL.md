---
name: pcbcraft-solidcraft
description: Integrate Solidcraft APIs, native parametric models, volumetric PCB copper, picking metadata or 3D performance in Pcbcraft.
---

# Shared Solidcraft engine

Read [native integration research](../../docs/solidcraft-interop-research.md) for model work and [copper research](../../docs/copper-performance-research.md) for copper/rendering/performance work. Findings are pinned observations, not proof of behavior on a newer revision.

## Native models

- Pin the tested Solidcraft revision and record required capabilities. Use its engine, evaluation and native writer. The displayed model and `.solidcraft` output share parametric definitions.
- Prove that dependencies survive saving and reopening: board thickness moves top/bottom attachments correctly; component X/Y/rotation and model dimensions recompute without Pcbcraft.
- Distinguish evaluated numeric transforms from persistent placement expressions. The inspected `occurrence.move` path stores matrices, and `component.insert_design` transfers evaluated solids; neither alone proves retained editable composition.
- Reuse native model definitions with independent occurrence parameters and stable identity mappings. Snapshot required model assets and parameter interfaces. Vendor STEP stays identified as imported geometry.
- If an API or native feature is missing, extend the shared Solidcraft engine in an isolated checkout. The compatible Solidcraft application must include it too. Unknown features cannot silently degrade to absent copper or flattened geometry.
- Keep PCB regeneration ownership explicit. Do not overwrite independently edited Solidcraft designs while refreshing a generated model.

## Copper and performance

Represent traces, pads, filled zones and plated barrels with physical thickness and stackup position. Preserve object/net/layer identity through geometry, GPU batching, picking and native persistence.

Reuse shared-engine primitives or add a native copper collection with item/chunk-local caching. Avoid whole-board unions or thousands of sequential Boolean operations in an interactive gesture. Define overlap handling so display surfaces do not flicker and exact measurements do not double-count copper. Exact aggregate solids, when needed, are evaluated through Solidcraft.

Camera/highlight changes reuse geometry. Local edits invalidate the changed objects and actual dependents. Batch GPU work without losing item-level selection; use shared meshes for repeated definitions. Long work is cancellable/revision-tagged, and stale results cannot overwrite current edits.

Physical looks live in `material.rs` and are stored on each native body (`appearance.assign` with `pbr`). Native bodies stay physical and disjoint (copper joining copper excepted); soldermask, finish, terminations and solder are display-only meshes from `appearance.rs`, never native features or saved. The viewport reads the looks back, with sources in [materials](../../docs/materials.md). Body kinds are kept in chunked `org.pcbcraft.electronics.bodies.N` metadata, because Solidcraft caps each value at 64 KiB. Highlight is recolour-only through cached blocks in `hl_slot`, and shadows go in `sketch_slot`, so neither re-uploads PBR vertices.

Measure before claiming speed. Record hardware/backend/resolution, workload, p50/p95 latency, draw calls, uploads, memory and recomputed items. Separate cold construction, warm navigation, local update, picking and save/reopen. Use the provisional targets in the plan as goals, not achieved claims; evaluate native and wasm separately.
