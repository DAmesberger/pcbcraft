---
name: pcbcraft-solidcraft
description: Integrate Solidcraft APIs, native parametric models, volumetric PCB copper, picking metadata or 3D performance in Sparksmith.
---

# Shared Solidcraft engine

Read [board plan and interface](../../docs/architecture/board-plan-and-interface.md) for the target architecture, [native integration research](../../docs/solidcraft-interop-research.md) for model work and [copper research](../../docs/copper-performance-research.md) for copper/rendering/performance work. Findings are pinned observations, not proof of behavior on a newer revision.

## Native models

- Pin the tested Solidcraft revision and record required capabilities. Use its engine, evaluation and native writer. The displayed model and `.solidcraft` output share the board plan and the part designs.
- Prove that dependencies survive saving and reopening: board thickness moves top/bottom attachments correctly; component X/Y/rotation and model dimensions recompute without Sparksmith.
- Distinguish evaluated numeric transforms from persistent placement expressions. The inspected `occurrence.move` path stores matrices, and `component.insert_design` transfers evaluated solids; neither alone proves retained editable composition.
- Reuse native model definitions with independent occurrence parameters and stable identity mappings. Snapshot required model assets and parameter interfaces. Vendor STEP stays identified as imported geometry.
- If an API or native feature is missing, extend the shared Solidcraft engine in an isolated checkout. The compatible Solidcraft application must include it too. Unknown features cannot silently degrade to absent copper or flattened geometry.
- Keep PCB regeneration ownership explicit. Do not overwrite independently edited Solidcraft designs while refreshing a generated model.

## Copper and performance

Represent traces, pads, filled zones and plated barrels with physical thickness and stackup position. Preserve object/net/layer identity through geometry, GPU batching, picking and native persistence.

The board plan is the one description of the board's flat bodies (laminate, copper, drills and films): for each body its source object, kind, layer, heights, loops and look, with flattening and tiling stated in the plan. It has two thin consumers. The live mesher extrudes each body's loops into triangles with item/chunk-local caching; the exact exporter issues Solidcraft commands for each body. Neither adds, drops, merges or reshapes a body, and neither keeps its own table of kinds or looks. `cargo xtask board-parity` (added with the board plan) compares the two, body by body: by source, kind and layer, and by geometry fit (each body's top and bottom faces against the plan's loops, its holes and heights; volume only as one more sign; `project::fit`). Every exact export runs the same fit check and refuses a model that does not fit unless asked to write it anyway. Avoid whole-board unions or thousands of sequential Boolean operations. The exact build is for export and as the test reference; after the live native board is removed its only speed requirement is that an export finishes, so do not tune it for interactive speed.

In the exporter, every Solidcraft command evaluates parameters and solves joints again, so build with one command per kind of thing (`sketch.insert_dxf`, `parameters.import`, `occurrence.copy` with `translations`, `joint.create` with `joints`), repeated parts as placements of one definition, and the laminate as extruded slabs rather than Boolean cuts ([build cost](../../docs/pcb-realism.md#build-cost-measured)). Define overlap handling so display surfaces do not flicker and exact measurements do not double-count copper. Exact aggregate solids, when needed, are evaluated through Solidcraft.

Camera/highlight changes reuse geometry. Local edits invalidate the changed objects and actual dependents. Batch GPU work without losing item-level selection; use shared meshes for repeated definitions. Long work is cancellable/revision-tagged, and stale results cannot overwrite current edits.

Physical looks live in `material.rs` and are stored on each native body (`appearance.assign` with `pbr`). Native bodies stay physical and disjoint (copper joining copper excepted); physical board bodies are meshed from the plan in the live view and built natively for export. Soldermask, finish, terminations and solder are display-only meshes from `appearance.rs`, never native features or saved. The viewport reads the looks back, with sources in [materials](../../docs/materials.md). Body kinds are kept in chunked `org.pcbcraft.electronics.bodies.N` metadata, because Solidcraft caps each value at 64 KiB. Highlight is recolour-only through cached blocks in `hl_slot`, and shadows go in `sketch_slot`, so neither re-uploads PBR vertices.

Measure before claiming speed. Record hardware/backend/resolution, workload, p50/p95 latency, draw calls, uploads, memory and recomputed items. Separate cold construction, warm navigation, local update, picking and save/reopen. Use the provisional targets in the plan as goals, not achieved claims; evaluate native and wasm separately.

## Board interface

An enclosure links the board's published interface, never the board's own bodies ([design](../../docs/architecture/board-plan-and-interface.md#part-2-design-note-board-interface-and-enclosure-links)). Each datum has a document id and a slot in an append-only table; a slot is never reused, and a retired slot still uses its feature ids. The interface file writes the slots first, in slot order, each with a fixed budget of feature ids, so face names are the same in every build. Optional mechanical or full board bodies follow the slots and may renumber freely. Tokens in the file are built from ids only (`i80.opening`, `h12`, `board`); display names sit beside them and never carry identity. Tests and tools never reference the bodies after the slots, and updates in either direction go through a reviewed preview.
