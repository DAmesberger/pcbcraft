# Copper geometry and interactive 3D performance

Source snapshot: Solidcraft `4f3ac07aec69489183185071f07d7fbcf4f51e64`, inspected 29 October 2026. This is source research and a proposed architecture, not measured Pcbcraft performance.

## Product requirement

Copper traces, pads, filled zones and plated via barrels should have actual three-dimensional geometry at their physical thickness and layer elevations. Selecting a trace or a whole net in either PCB view must identify and highlight the same electrical objects in the other view. Copper is part of the native Solidcraft model, not just a board texture or a separate exported visualization.

This requirement is compatible with GPU triangle rendering. A mesh with top, bottom and side surfaces represents real thickness; drawing those triangles does not require executing general-purpose solid booleans every frame. Electrical selection depends on persistent object/net identity attached to the geometry, not on whether a kernel has fused the whole net into one solid.

## Existing Solidcraft foundations

**Geometry reuse and highlighting.** The GPU scene supports reusable geometry blocks; unchanged blocks retain their buffers. Its palette separates colour from geometry, and highlight paths reuse existing triangles rather than copying them. Planned-body highlights can still cost a draw per body, so many copper objects must not automatically become thousands of highlight draws. [GPU scene and renderer](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/gpu/src/view.rs).

**Repeated geometry.** The viewport groups shared mesh references into instanced draws, keeps shared triangle data, and supports mesh levels of detail. This is useful for repeated component models and suitably identical pad/via shapes. It does not imply that unrelated trace segments automatically become efficient instances. [Viewport instancing](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/ui-egui/src/viewport.rs#L1304).

**Picking.** Solidcraft builds per-mesh triangle/edge indexes and caches them. Native builds prepare larger indexes on a worker while picking uses a fallback scan; the inspected wasm branch builds synchronously. Existing hit records identify CAD faces, edges and sketch objects, not electrical nets. Pcbcraft therefore needs a stable semantic mapping and a broad-phase spatial index appropriate for many copper objects, even if it reuses the triangle-index machinery. [Picking indexes](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/ui-egui/src/pick_index.rs), [hit types](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/ui-egui/src/viewport.rs#L220).

**Evaluation reuse.** Model bodies share cached meshes, display plans and measurements. However, the document evaluator traverses a feature timeline and its cache keys include the starting model state. This does not establish object-local rebuild cost for a board represented by thousands of sequential extrusions. Reuse must be measured after editing an early trace, not inferred from a warm static viewport. [Body caches and evaluator](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/doc/src/eval.rs#L1641).

These are promising building blocks, not proof that current Solidcraft handles a densely routed PCB interactively without changes. This inspection has not established an existing native copper feature or an electrical-metadata contract; both remain integration work to prove or add.

## Proposed shared engine extension

Prototype a native copper collection/feature in the shared Solidcraft engine. Store persistent per-item geometry parameters and identities; evaluate/copy/cache independently by item or bounded spatial chunk. If existing APIs already provide equivalent behavior at the measured scale, use them. Otherwise extend Solidcraft, rather than creating a second Pcbcraft modelling engine.

- A track contains its path, width, copper thickness and layer elevation; generate a closed volume with correct joins and end shapes.
- A pad contains its land geometry, thickness, elevation and any drill opening. A plated via includes the barrel and layer lands, with the hole remaining empty.
- A filled zone contains its resolved polygon boundaries, holes and thermal reliefs, extruded through copper thickness. Connectivity/fill calculations remain electronics-specific inputs to the shared geometry engine.
- Each item retains its PCB object ID, net ID and layer ID. Draw batching and level-of-detail changes must preserve the mapping from selected geometry back to the source item.

Simple extrusion volumes do not require a chain of general 3D unions. Shared engine routines can construct/render them directly from their native definitions and provide exact geometry for operations that require it. Physical same-net overlaps and adjacent regions need an explicit union/ownership policy: coincident surfaces must not flicker, material measures must not double-count overlap, and selecting an individual source trace must still work. Benchmark a local 2D footprint union plus thickness extrusion where an aggregate is required; do not fuse every net after every pointer move.

Persistent native definitions are essential. A cached display mesh is disposable acceleration data, not the only saved representation. Stock compatible Solidcraft must understand and recompute the saved copper feature. Introducing one therefore includes a shared format/API change and minimum-version requirement; that same feature implementation must ship in the compatible Solidcraft application. An opaque Pcbcraft-only blob or an unknown feature that silently loads as missing copper does not satisfy compatibility.

## Interaction without rebuilding the board

Use one selection model across 2D and 3D. A hit resolves to a PCB item; selecting its net obtains the set of member item IDs. Highlighting changes palette/selection data, without reconstructing copper or component meshes. Selecting a net is different from selecting only its physically connected island, which needs its own command.

Editing a track invalidates that track's geometry and any genuinely dependent fill regions. Camera motion, view changes and net highlighting invalidate no model geometry. Repeated component meshes stay shared. Batch drawing by layer/material/spatial chunk while preserving per-item identity; do not equate one selectable item with one GPU draw call.

Rendering must support inspection policies: ordinary depth-tested visibility, hiding/fading the substrate or mask, layer isolation, and explicit through-board net highlighting. A highlighted buried or covered trace should not appear physically exposed by accident. Thin-copper depth precision and transparency costs need dedicated tests at grazing angles and large zoom ranges.

Push-and-shove previews use speculative route edits and update affected geometry only. Cancellation restores the last committed state; asynchronous work is revision-tagged so outdated geometry cannot replace the current board. These requirements follow Solidcraft's existing rule that long work stays off the window thread. [Performance rules](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/skills/solidcraft-performance/SKILL.md).

## Measurements before architectural commitment

Build a small source-generated benchmark family: 1,000, 10,000 and 100,000 copper primitives, with separate tests for complex zone boundaries, repeated components and dense vias. Include a fully visible board and a difficult transparent/isolated-layer view. These are proposed workload sizes, not supported-capacity claims.

Measure cold load, warm 2D/3D switching, orbit frame time, hover/pick latency, whole-net highlight, one-track edit, multi-track shove preview, zone refill, native save/reopen, and parameter-driven regeneration in Solidcraft. Record CPU/GPU time, p50/p95 latency, RAM/VRAM, draw calls, triangles, bytes uploaded and the number of recomputed items. Report machine, backend, resolution and quality settings.

Suggested desktop design targets are a 16.7 ms frame budget for ordinary navigation, selection feedback within one or two frames, and a local edit preview within 50 ms on a declared reference workload. These are goals to test, not promises. Large fills may run asynchronously with visible progress; highlighting must not wait for them. Test native and wasm separately. If a one-track edit scales with the whole board, solve the invalidation/evaluation architecture before expanding the editor UI.
