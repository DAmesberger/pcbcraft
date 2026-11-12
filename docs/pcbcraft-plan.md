# Sparksmith first edition plan

Product plan, updated 30 October 2026. This combines established user requirements with proposed scope boundaries. Implementation is partial; see [editor evidence](editor-evidence.md). The [component-editor acceptance scenarios](part-library-design.md#component-editor-acceptance-scenarios) define the accepted component workflow.

Sparksmith should deliver one complete electronics workflow: **create a component → draw a schematic → place and route a PCB → inspect the same PCB in 3D → export manufacturing files and a native parametric `.solidcraft` assembly**. Each editor starts with a small, usable toolset. The first release is complete across this workflow, rather than deep in only one editor.

The UI and interaction style should match Solidcraft. Altium informs the integrated PCB workflow and switchable 3D view; Solidcraft supplies the visual language, shell, controls and navigation.

## Requirements established in this discussion

- Same technology base and agent-oriented development principles as Solidcraft.
- Component creation, including schematic symbols and footprints.
- A basic schematic editor in the first edition.
- A PCB editor whose 3D view is a mode of the same design, with real thickness-aware copper geometry and identical trace/net selection and highlighting in 2D and 3D.
- Native `.solidcraft` output with parametric board geometry and component placements, generated using the Solidcraft engine directly. STEP is an optional exchange export.
- Solidcraft owns 3D model construction, evaluation and persistence; Sparksmith supplies electronics-specific data and integration.
- Push-and-shove is a priority: include it in the first release if feasible, otherwise deliver it in the second release with its architecture prepared from the start.
- A deliberately designed part-library model.
- New Component authoring, data-defined typed fields and units, independently revisioned footprint choices, basic multi-unit symbols, structured per-pin function alternatives and instance selections, and datasheet/distributor references.
- A general plan for review before implementation.

## What to carry over from Solidcraft

The inspected checkout is [tangency-labs/solidcraft at 4f3ac07](https://github.com/tangency-labs/solidcraft/tree/4f3ac07aec69489183185071f07d7fbcf4f51e64), cloned into `/Users/amd/dev/solidcraft`. Its workspace declares Rust 2024, Rust 1.95, egui/eframe 0.36 and wgpu 30. These are the inspected versions, not a promise to track every upstream update. [Dependency manifest](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/Cargo.toml).

Carry over these architectural commitments:

- Rust application code, native desktop and a WebAssembly-compatible core.
- egui/eframe UI and wgpu rendering.
- One command engine for the UI, CLI, scripts and MCP, with explicit input/output contracts.
- A headless document model and checks, independent of the UI.
- Atomic, undoable document changes; rejected commands leave the document intact.
- Shared theme tokens, common panel/container components and one navigation input path.
- Enforced dependency boundaries, hostile-input tests, measured reference cases and real UI evidence.

These are existing Solidcraft rules, not assumptions inferred from its screenshots. [Agent rules](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/AGENTS.md), [command design](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/skills/solidcraft-commands/SKILL.md), [UI rules](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/skills/solidcraft-ui/SKILL.md).

Reuse selected infrastructure with attribution and pinned dependencies. Solidcraft's UI crate currently depends on its CAD engine; it is not a standalone application shell that can simply be dropped into Sparksmith. Start with a small adaptation of its shell, tokens, containers and navigation patterns. Extract shared Craft crates when both applications have concrete requirements. Do not begin by building a general application framework. [UI crate dependencies](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/ui-egui/Cargo.toml).

## First useful release

Target simple rigid two-layer boards with ordinary SMD and through-hole components. A reference project should be a modest sensor or interface board, roughly 15–30 components, containing at least one component authored entirely in Sparksmith. These counts describe the acceptance example, not hard product limits.

| Area | Include in the first edition | Postpone |
| --- | --- | --- |
| Component authoring | New Component workspace with Symbol, Footprint, Mapping, 3D and Details; reusable typed fields/units; single- and multi-unit symbols, alternate representations, structured pin functions/directions and resource constraints, peripherals with mux codes, GPIO and pin options, pin rules as sandboxed Rhai scripts and a reviewed datasheet pinout import; pin table; data-driven package templates: cited package rows over eight families (chip, gull-wing, no-lead, header, tabbed, molded, grid array, through-hole) that build the package, its footprint and generated model; lines/polylines, rectangles, circles/arcs and text; pad arrays; rectangular/round/oval/rounded-rectangle SMD pads, circular plated holes and mounting holes; mask/paste, courtyard and reference/value text | Interactive package wizards (data-driven templates replace them), arbitrary custom pad primitives, slotted drills, simulation models, automatic gate/pin swapping |
| Libraries | Project-local definitions, optional installed local libraries, category browsing/search, reusable field definitions, independent revisions and footprint variants, complete snapshots including generator recipes/assets, explicit updates, provenance, typed manufacturer specifications and datasheet/distributor links | Hosted registry, live supplier APIs, organizational approval systems, automatic alternates, multi-level inheritance |
| Schematic | Single sheet; component placement; orthogonal wires; junctions; net labels; visible power connections; rotate/mirror; move and connection-preserving drag; annotation; typed properties; no-connect markers; basic ERC | Hierarchical sheets, buses, repeated channels, gate/pin swapping, simulation |
| Schematic to PCB | Shared component identity; initial board population; reviewed updates for added/removed components, mappings and nets; cross-selection; stale-board indication | Bidirectional circuit editing, automatic electrical conflict resolution |
| PCB layout | Two copper layers; simple closed polygon outlines/cutouts; placement, rotate and flip; snapping; ratsnest; net highlighting; manual 45° routing with clearance preview; through vias; track edit/delete; simple ground pours with clearance and thermal reliefs | Push-and-shove reserved for release 2 if absent from release 1; autorouting, differential pairs, length tuning, blind/buried vias, rigid-flex, HDI, RF tools, panelization |
| Checks | ERC; mapping completeness; shorts; unconnected pads; copper clearance; width; drill/annular ring; board-edge clearance; basic mask/silkscreen checks; stale-zone detection | Signal/power integrity, impedance solving, comprehensive fabrication-rule languages |
| 3D mode | Same document, component IDs, selection, undo and properties; orbit/top/bottom views; board thickness and holes; volumetric traces, pads, copper zones and plated via barrels; individual copper-object and full-net picking/highlighting; mask/silkscreen appearance; Solidcraft-native parametric component models and engine-generated envelopes; vendor STEP imported through Solidcraft | Photorealistic rendering, arbitrary 3D routing, rigid-flex animation, full mechanical collision solver |
| Output | Native project save; Gerber and drill files; BOM and placement CSV with explicit origin/rotation conventions; native parametric `.solidcraft` assembly; Open in Solidcraft; optional STEP export through Solidcraft; firmware pin-configuration export (JSON IR, C header, Zephyr overlay skeleton, Rust constants, user templates; no clocks or HAL) | ODB++, IPC-2581, manufacturing portals, live ECAD/MCAD synchronization |
| Reliability and automation | Undo/redo, autosave/recovery, atomic saves, explicit format versioning; CLI and MCP for every implemented design operation; native release and continuous wasm checks | In-app agent chat, collaborative editing, plugins and broad platform packaging polish |

Component authoring is a real editor in this scope. Templates accelerate it but do not replace drawing symbols, creating footprints or mapping pins. A small curated starter library supports the demo. General KiCad project import can follow later; optional symbol/footprint import should support a documented subset and report unsupported features, rather than silently dropping them.

Push-and-shove is a committed second-release feature if it cannot ship in the first release. The first release can use manual routing with clearance preview and invalid-commit prevention, but it must already have a routing topology, obstacle queries and transactional edit interface that support moving existing tracks. Existing violations created by placement or library changes remain visible. Moving a routed component may leave tracks needing repair in the first edition; the editor must expose that condition accurately. See the routing plan below.

## The library foundation

The detailed contract and seven end-to-end scenarios are in [Part library design](part-library-design.md); terms are in the [glossary](../GLOSSARY.md). Basic multi-unit authoring and instance-level pin-function selection are first-edition requirements, distinct from later multi-sheet hierarchy.

The central choice is to separate **electrical meaning, schematic drawing, physical terminals, PCB land patterns and purchasing information**, while presenting them through one component-authoring workspace. A footprint name alone is not proof of compatible pin mapping. An MPN is optional during design, and choosing one must not silently alter the circuit.

Every project stores the complete set of exact definitions and required assets it uses. Editing an installed library cannot change an existing board. Updates show changes to electrical functions, mappings, pad geometry, model placement and affected routing before applying as one undoable operation.

Two essentials belong in the first implementation: stable identity across schematic and PCB, and explicit connection obligations for every physical terminal. Several pins named GND still need their required PCB connections. More advanced presentations and sourcing services can build on that foundation later.

## A Solidcraft experience across all editors

Use one application window and one project session. Component, Schematic and PCB are task workspaces within that shell. The PCB's **2D/3D control is a view toggle within the PCB workspace**, not a separate workspace or exported model viewer.

Use Solidcraft's theme tokens, typography, spacing, container chrome, command search, tool help, property controls and navigation conventions. Workspace changes replace relevant tools and content while retaining the shell. Prefer a consistent project/browser panel and contextual properties panel over independent editor windows. Initial panel layouts should be fixed and coherent; arbitrary docking is unnecessary for the first release.

Changing PCB view preserves selection and focus. Selecting U3 in 3D shows the same U3 properties and highlights its schematic instance. Selecting a trace, pad or via in 3D identifies the original board object; highlighting its net highlights all associated copper on both sides, including disconnected same-net islands, without claiming they are physically connected. Net highlighting and connected-copper inspection are separate operations. Hide soldermask, dim components or isolate copper layers/nets when needed to inspect obscured routing; normal picking respects visible geometry, with explicit inspection modes for hidden objects. Property edits and component placement changes can use the same commands in either view, with movement constrained to the board plane. Routing stays in the 2D toolset initially. Switching back reveals the same live document without a refresh/export step.

Visual consistency is an acceptance criterion: review actual screens for component creation, wiring, routing, 3D selection and update previews beside the inspected Solidcraft UI. Matching the technology alone will not produce matching UX.

## Proposed architecture

Keep the early workspace small, with explicit responsibilities rather than one crate per feature:

| Boundary | Responsibility |
| --- | --- |
| `geom` | PCB coordinates, robust 2D primitives, intersections, offsets and spatial queries |
| `doc` | Library definitions and revisions, project snapshots, schematic connectivity, board objects, stable identity and serialization model |
| `checks` | ERC, DRC, connectivity and component validation over document snapshots |
| `render` | Shared 2D/3D object/net selection and picking identities; consume Solidcraft-evaluated copper/component geometry and shared rendering infrastructure; non-geometric annotations |
| `solidcraft-integration` | Map board geometry, library model revisions and placement parameters into a native Solidcraft engine session; preserve history and identity; use upstream evaluation and persistence |
| `io` | Native Sparksmith persistence and manufacturing output; delegate native Solidcraft persistence and optional STEP export to the Solidcraft integration |
| `engine` | Commands, queries, transactions, undo, document revisions and background-job coordination |
| `ui-egui`, `mcp`, apps | Thin interactive and automation clients; GPU/window/process integration at the outside |

Lower layers never depend on UI or transport code; `doc` depends on geometry, readers/checks/exporters consume the document, and `engine` coordinates them. Use Solidcraft engine/document/kernel APIs directly behind `solidcraft-integration`, pinned to a tested revision. Sparksmith must not develop or maintain a competing 3D modeller, B-rep kernel, tessellator or Solidcraft file writer. PCB connectivity, 2D copper operations and routing remain electronics-specific; they supply geometry and parameters to Solidcraft for the shared 3D representation. The live 3D view and saved `.solidcraft` document use the same evaluated native model.

Use bounded integer coordinates for authoritative PCB geometry, with explicit unit conversions and checked arithmetic. Render in floating point. Use the same pad/track/zone definitions for DRC and fabrication output, with documented numerical tolerances. Validate a suitable permissively licensed Rust polygon implementation before committing to it.

Schematic connectivity is the source of intended electrical nets. The board stores physical realization and its synchronized schematic revision. Updating the board computes a change plan, preserves matching instances, applies explicitly, and reruns affected checks. Long-running checks and pour jobs carry their source revision; stale results cannot overwrite newer edits.

Choose a portable, versioned native project representation that supports useful diffs and contains required library revisions/assets. A directory representation can support version control, with a packed form for sharing later. The exact extension and on-disk split should be settled by the first save/reopen prototype, not become an early format-design project.

## Native Solidcraft integration

Provide **Save Solidcraft Model** and **Open in Solidcraft** in the first edition. Produce a real `.solidcraft` document using Solidcraft's engine and native writer, preserving board features, named parameters, reusable component definitions and parametric placements. A STEP import wrapped in a native filename does not satisfy this requirement. The inspected upstream defaults to `.sc` and also supports `.solidcraft`; verify the requested extension in the compatibility tests.

Sparksmith owns circuit meaning, board layout and the mapping into Solidcraft. Solidcraft owns 3D model creation, evaluation, feature history, tessellation and native persistence. Generate board profiles, thickness, holes and cutouts using its modelling facilities. Use native component models where available, and create dimensioned envelopes through the same engine. Vendor STEP models remain supported through Solidcraft's importer, with their imported geometry clearly distinguished from native editable history.

Store meaningful board and instance parameters: thickness, outline dimensions where applicable, hole locations, component X/Y/rotation, side, standoff and model-to-package transform. Retain their relationships in the saved document. For example, changing board thickness must move top-mounted components with the top face, while bottom placements remain attached to the correct face. Repeated components share model definitions but have independent placement parameters and stable Sparksmith identity mappings.

The board's flat bodies (laminate, copper, drills and films) are described once by the board plan, derived from the document. The live PCB 3D view meshes that plan directly, and part models come from their own Solidcraft designs, placed by the document's transforms. Saving for Solidcraft is a construction from the same plan through Solidcraft's engine, checked against it body by body; there are no separate placement rules for export ([board plan and interface](architecture/board-plan-and-interface.md)). On reopening in Solidcraft, users must be able to change the exposed parameters and recompute without Sparksmith running. Parametric placement and geometry fidelity are release requirements, not optional follow-up enhancements.

The inspected source supports native persistence and assembly operations, but the occurrence-move command evaluates expressions into stored matrices. Its linked-design path also transfers evaluated solids rather than the full source feature history. Therefore, prototype persistent placement dependencies and native model composition first. Use existing suitable Solidcraft features where proven, or extend the shared engine APIs to support the required behavior; do not hide the gap behind numeric transforms or a new private 3D engine. See [integration research](solidcraft-interop-research.md).

Sparksmith remains authoritative for electrical and PCB layout changes. Exported native models can be edited independently in Solidcraft; those edits do not automatically modify the PCB. Refreshing a generated model must not silently overwrite user-edited copies. Later linked enclosure updates and bidirectional conflict resolution are distinct from the first release's native parametric compatibility.

STEP remains an optional interoperability export, produced through Solidcraft's exporter. Real copper geometry is in the first-release scope and the native model: traces, pads and zones have stackup-defined thickness, and plated holes/vias have barrel geometry. Solder joints and photorealistic materials remain later work. Rendering copper as flat decoration does not satisfy the requirement. See [copper performance research](copper-performance-research.md).

## Copper geometry and performance

Maintain authoritative PCB object identity, net identity and layer identity through Solidcraft evaluation, display batching, picking and native persistence. A trace may contribute several rendered pieces, and a render batch may contain many traces; neither operation may discard those associations. Full-net highlighting updates selection/style data and reuses geometry. Switching views changes the camera and presentation, not the electrical model or geometry history.

Use shared Solidcraft engine primitives/features for copper. Tracks can be thickness-aware swept planar profiles, pads extruded land shapes, zones extruded filled polygons with holes, and barrels annular geometry. The GPU displays cached triangulated surfaces with real top, bottom and side faces. Rendering triangles does not imply flat or non-parametric source geometry. Avoid assigning every short routing segment a costly independent general-purpose Boolean history or unioning the entire board on each edit. If needed, implement a compact, parameterized copper feature in the shared Solidcraft engine, retaining its source primitives and object/net membership; Sparksmith must not own a separate modeller or tessellator. Any new native copper feature, parameter representation or electrical selection metadata must also be supported by the compatible Solidcraft release; saving a Sparksmith-only opaque feature does not deliver standalone native compatibility. Version the shared capability and report unsupported target versions explicitly.

Electrical clearance/connectivity and manufacturing checks use authoritative 2D copper geometry plus layer/via connectivity. The 3D model derives from those same definitions and stackup dimensions. Where exact unioned B-rep copper is required for an operation or exchange export, compute it through Solidcraft on demand with documented tolerances. Interactive rendering, hover and highlighting must not depend on whole-board Boolean unions. Internal overlaps in a display decomposition must not be mistaken for valid unioned volume, exposed surfaces or connectivity.

Cache unchanged geometry and GPU buffers, instance repeated shapes, and batch drawing while retaining per-object/net selection. Invalidate only affected features/regions and their actual dependencies. A shove updates affected track chains; a component move updates its placement and dependent geometry. Pour filling and expensive exact evaluation run outside the UI path with revision-tagged results. Immediate previews must be identifiable as previews; finalized native saves and manufacturing exports use a consistent committed revision.

Make performance a foundation milestone, before growing the editor feature set. Benchmark a small acceptance board and synthetic stress cases at 1,000, 10,000 and 100,000 copper primitives, including long traces, dense vias, large zones with many holes and repeated component models. The larger cases are scalability probes, not first-release capacity promises. Record hardware, resolution, triangle counts, draw calls, upload bytes, memory, cold build/open time, native file size and incremental update cost.

Provisional interaction goals on an explicitly recorded reference machine are a 60 Hz orbit/pan target (about 16.7 ms per frame), p95 pick/highlight feedback below 50 ms, and p95 local routing-preview feedback below 50 ms on the chosen release workload. Measure warm view switching separately from first geometry construction, and report expensive pour/evaluation completion separately from UI responsiveness. These are targets, not measured results. Require counters proving that camera and selection changes cause no geometry rebuild, and that a local trace edit does not rebuild the whole board. Finalize workload/hardware thresholds from the initial benchmark results.

## Routing architecture and second release

Design for push-and-shove before implementing the first routing tool. Keep routing algorithms separate from the interaction controller and use a common request/result contract for manual, walk-around and shove strategies. The routing model needs stable track/via identities, connected chains and junctions, net membership, layer spans, widths, clearances and locked objects. Spatial queries and exact clearance checks must use the same authoritative geometry as DRC.

A routing gesture operates on a speculative edit set. It can insert, split, move, replace or remove affected track segments while preserving intended connectivity. Preview the complete result, then validate and commit it as one undoable transaction. Cancel, an impossible shove or an outdated document revision restores/leaves the previous design intact. Deterministic work limits and cancellation keep interaction responsive. Manual routing in release 1 uses this same mechanism.

**Release 2 must deliver interactive push-and-shove if release 1 does not.** Initial scope is ordinary single-ended 45° tracks on the supported two-layer boards, obstacle avoidance and connected-track dragging. Fixed pads, board boundaries, keepouts and locked copper are respected; vias can be treated as fixed obstacles initially, with movable-via shove following separately. Shoving propagates through affected neighboring tracks and refuses safely when no legal result exists. Differential-pair shove, length tuning and autorouting remain later work.

Acceptance covers narrow corridors, chained shoves across several nets, T-junctions, fixed vias, locked tracks, impossible moves, existing violations and exact undo/redo. No shove may silently break a connection, change a net or introduce an unreported clearance violation. Add fixtures for these cases in the initial routing architecture, then enable the shove strategy against them in release 2. Record interaction latency on representative boards before declaring it usable.

## Agentic development rules

The adapted [AGENTS.md](../AGENTS.md) and linked task skills now define the working rules. [Milestone 0](milestone-0.md) implements their executable CI gates. The principles are:

1. Every design operation has a typed command contract, structured errors and queryable results. UI, CLI and MCP run the same implementation; programmatic calls never open dialogs.
2. Validate before mutation. Document changes are atomic and undoable. Interactive drags form sensible undo groups. Export/launch operations report their external effects separately from document undo.
3. Keep UI code thin; enforce dependencies, theme tokens, shared containers and navigation in CI.
4. Treat files and command parameters as untrusted input. Avoid unchecked panics and unsafe application code; test malformed, oversized and invalid inputs.
5. Check measured results, not just success replies: connectivity, physical clearances, manufacturing geometry, assembly placements and save/reopen behavior.
6. Adopt independent implementation and asset attribution rules, and permissive code dependencies. Public documentation and format specifications are reference sources. Do not copy commercial assets or read/copy incompatible implementation code.
7. Generate command documentation and schemas from the registry. Make migration changes explicit and test older project files.
8. Keep changes focused with reproducible evidence. Run repository checks and applicable tests before merging; include actual screenshots/recordings for visible behavior.

The PCB reference suite should include analytically known gaps and drill dimensions, correct and incorrect pin mappings, deliberately disconnected same-net pads, copper islands, changed library pads, bottom-side component transforms, and manufacturing output checked independently against the intended geometry. A count of commands or tests is not proof of a complete workflow.

## Milestones with demonstrable exit criteria

| Milestone | Deliverable | Exit criterion |
| --- | --- | --- |
| 0 — Prove the foundations | Solidcraft-style shell; draft library/document model; command registry; routing-topology, volumetric copper performance and native Solidcraft engine experiments | Create a tiny component definition, save/reopen it, place a simple board in 2D/3D, and open a native `.solidcraft` assembly, change thickness/placement parameters and recompute correctly; prove the difficult mapping and geometry cases below |
| 1 — Create and connect | Component authoring, project library snapshots, single-sheet schematic and basic ERC | Author a new component, map its terminals, place and wire it, derive the expected nets, undo/reopen, and reproduce through commands |
| 2 — Place and route | Board update, placement, ratsnest, manual routing, vias, initial DRC and live 3D | Complete the reference board; detect deliberate shorts/opens; show the same selected component, copper object and highlighted net across the relevant schematic/PCB/3D views |
| 3 — Produce and revise | Ground pours/thermals, complete scoped checks, fabrication exports, Solidcraft handoff, semantic library updates and recovery | Export and independently inspect manufacturing geometry; revise a component and schematic without silent wiring changes; reopen offline with original libraries removed |
| First edition acceptance | The complete reference workflow through the normal UI and automation | A user can create their own part and manufacture the supported board; the native parametric Solidcraft model matches the PCB and recomputes independently; limitations are documented |

Milestone 0 should challenge four risks immediately: library identity/mapping changes on a routed sample; robust copper clearance/zone geometry; scalable volumetric copper display/picking/incremental updates; and native parametric Solidcraft composition, including top/bottom placement relationships. Estimate implementation effort after those probes. Solidcraft's historical agent-hour estimates are not a transferable schedule for a new EDA core.

The second release prioritizes push-and-shove if it did not ship in the first edition; it is not an unscheduled backlog item. Additional improvements follow actual boards: broader component/footprint primitives, multi-sheet hierarchy, more copper layers, better import, and linked Solidcraft enclosure updates. Basic multi-unit symbols belong to the first edition. Hosted libraries, high-speed design, simulation and collaboration are separate later expansions.

## Review decisions

The user requirements above are fixed inputs. The following are proposed scope boundaries for review:

- First release supports single-sheet schematics and two-layer rigid boards.
- Component creation is unified, with reusable definitions and exact project snapshots underneath.
- The initial library supports local use and optional MPNs without a mandatory cloud account.
- Routing architecture supports push-and-shove from the start; the feature ships by release 2, or earlier if feasible.
- Native parametric `.solidcraft` generation and direct Solidcraft engine use are first-release requirements; STEP is optional. Live cross-application synchronization follows later.
- Desktop is the first polished delivery, with wasm compatibility checked from the beginning.

Planning and the agent-instruction bootstrap are complete. Product implementation and executable CI gates have not started; [ROADMAP.md](../ROADMAP.md) tracks the next work. No upstream modifications or publishing were performed.
