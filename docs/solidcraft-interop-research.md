# Native Solidcraft integration

Research snapshot: 29 October 2026, SolidCraft checkout `4f3ac07aec69489183185071f07d7fbcf4f51e64`, workspace version `0.26.0`. This note distinguishes inspected capabilities from required integration. The subsequent native probe now verifies board/component geometry and persistent placement; see [execution evidence](foundation-evidence.md). Copper and full library-model composition remain unproved.

## Required first release behavior

Sparksmith uses the Solidcraft engine directly for its board and component 3D models. The integrated PCB 3D view and saved **`.solidcraft` document** share the same native model. Solidcraft owns modelling, evaluation, feature history, tessellation and persistence. Sparksmith owns the electronics model and the mapping of board geometry, library definitions and placement parameters into that engine.

The native document must retain parameter-driven board geometry and placements that recompute after opening in Solidcraft without Sparksmith running. STEP is an optional exchange export through Solidcraft, not the native integration path. Merely wrapping imported solids in a native document does not meet the parametric requirement.

## Verified upstream capabilities and gaps

### Native engine and persistence

Solidcraft exposes a `Session` command engine over its native document. Commands, parameters and model evaluation are shared by its UI, CLI and MCP. Use these APIs and the existing native writer rather than implementing another Solidcraft serializer. The file command implementation includes save/reopen tests using `.solidcraft`. Upstream currently prefers `.sc` for new designs and supports the longer extension; explicitly test the requested `.solidcraft` path in Sparksmith. [Engine](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/engine/src/lib.rs), [file commands and tests](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/engine/src/cmd/file.rs), [native persistence](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/io/src/design.rs).

The workspace is Rust with egui/wgpu and layered engine/document/kernel/render crates. The current geometry backend is isolated, with a migration described in the roadmap. Pin a tested upstream revision and integrate through the engine boundary; do not build Sparksmith-specific kernel internals that must track that migration. [Workspace](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/Cargo.toml), [architecture](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/docs/architecture.md), [roadmap](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/ROADMAP.md).

### Persistent placement relationships need proof

`occurrence.move` accepts expressions for translation/angle, evaluates them and stores a numeric occurrence transform. Calling it with an expression is therefore not evidence that a persistent relationship to that expression survives parameter changes. This finding concerns that command path; it does not establish that no other Solidcraft facility can express the required placement. [Occurrence move implementation](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/engine/src/cmd/component.rs).

The implemented probe uses native planar joints with retained X/Y/rotation expressions and a board-thickness offset. Changing those parameters after reopening recomputes independent placements, including bottom-side orientation. No upstream extension was needed for this fixture. This does not resolve importing arbitrary model histories or parameter namespaces from external library files.

### Native model composition must retain history

`component.insert_design` supports native linked designs, but its inspected `session_bodies` implementation evaluates source solids and transfers them through an internal STEP representation into an Import feature. It carries selected metadata, materials and face naming; mesh bodies are omitted. This is a useful linked evaluated-body path, but should not be mistaken for copying the source's full editable feature history into the assembly. [Linked design implementation](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/engine/src/cmd/component_link.rs).

Prefer native definition/history composition for Sparksmith-authored models, with shared definitions for repeated parts and independent instance placements. Prove parameter scoping, definition reuse, identity mapping and dependency packaging. If upstream needs an API to support this, add that capability at the shared engine boundary rather than accepting flattening or owning a second modelling engine.

Native file compatibility does not automatically imply full electrical compatibility: the generated model contains mechanical features, component identities and placement parameters; Sparksmith remains authoritative for schematic nets and PCB routing.

### Units and coordinate conventions

Solidcraft uses millimetres, radians and Z up internally. Its occurrence move matrix input is documented as row-major, whereas its low-level STEP product writer uses column-major child placements. Keep conversions explicit and tested. Bottom-side parts require proper orientation and surface attachment, including model-to-package offsets; do not use a negative-scale shortcut for rigid placement. [Ground rules](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/skills/solidcraft-ground-rules/SKILL.md), [occurrence commands](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/engine/src/cmd/component.rs), [STEP writer](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/kernel/src/step_out.rs).

## Proposed integration contract

1. **One engine-backed mechanical model.** A dedicated integration module maps a Sparksmith project revision into a native Solidcraft session. Board substrate, holes/cutouts, reusable component definitions and occurrences are built/evaluated through Solidcraft. Rendering consumes its evaluated geometry and retains Sparksmith picking identities. Traces, pads, zones and via barrels are real thickness-aware geometry in this shared model, retaining board-object/net/layer identity for picking and highlighting. Non-geometric annotations remain an application concern. Shared-engine copper features and cached/batched display geometry may be needed for scale; see [copper performance research](copper-performance-research.md).
2. **Meaningful native parameters.** Persist board and copper-layer thicknesses, copper source primitives and object/net associations, supported outline dimensions, hole positions and instance X/Y/rotation/side/standoff relationships. Scope parameters by stable identity so renaming a designator cannot break a placement expression. The precise upstream representation is chosen by the initial probe.
3. **Native library models.** Store exact Solidcraft model revisions and their parameter interfaces in the project library snapshot. Reuse native feature history where available. Simple envelopes are ordinary Solidcraft models with dimensions. Vendor STEP can be imported through Solidcraft, but imported geometry does not acquire a manufactured parametric history.
4. **Save and open.** Write `board.solidcraft` with Solidcraft's native writer and open it in the installed Solidcraft app. A missing desktop installation does not prevent saving. Package or resolve all required dependencies so a shared file can reopen independently. Return structured file/version/model-quality information to UI, CLI and MCP.
5. **Revision and undo coherence.** Sparksmith changes update the mechanical model for the same project revision. Stale background evaluations cannot overwrite newer geometry. Undo restores both electrical/layout state and its corresponding mechanical state. Do not introduce an independent user-visible undo history for the 3D view.
6. **Explicit ownership on later refresh.** Sparksmith controls the generated PCB definition; Solidcraft can edit a delivered native model independently. A later refresh must preserve user copies and distinguish generated content from enclosure work. Bidirectional editing and live synchronization are later features, not prerequisites for native parametric output.

Proposed commands such as `solidcraft.save_model` and `solidcraft.open_model` share typed contracts across UI/CLI/MCP. In-process engine use is the normal modelling path; launching or controlling the desktop app is a separate integration action, not a dependency for every PCB edit.

## Optional STEP exchange

Solidcraft already implements STEP assembly import and export. `solidcraft_io::step_assembly` maps components and occurrences into kernel products; the writer's `assembly_names_colours_round_trip` test covers names, colours and placements. Delegate optional STEP output to that exporter. [Assembly writer](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/io/src/lib.rs), [writer tests](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/crates/kernel/src/step_out.rs).

STEP is appropriate for other CAD tools and supplier model import; it does not preserve Sparksmith's complete electrical semantics or native parametric history. Import warnings and open/unsupported bodies must remain visible. [STEP capabilities and limits](https://github.com/tangency-labs/solidcraft/blob/4f3ac07aec69489183185071f07d7fbcf4f51e64/docs/step-import.md).

## Foundation probe and release acceptance

Before building the full UI, construct a small native board through the shared engine and verify:

- Board profile, thickness and holes exist as native editable features, with analytic volume checks.
- Repeated component instances reuse a definition; each has independent position/rotation parameters and stable identity mapping.
- Asymmetric parts on both sides, nonzero model offsets and a nonzero board origin have correct measured world placements.
- Save to `.solidcraft`, close, reopen in Solidcraft, change board thickness, and recompute the expected top/bottom attachment heights without Sparksmith.
- Change instance X/Y/rotation and model dimensions after reopening; the geometry follows retained dependencies rather than a frozen transform.
- Native model composition preserves intended feature history and parameter scope; imported vendor geometry is identified as imported.
- The project/file reopens offline without the original library installation, with all required model dependencies available.
- View switching, save/reopen and undo produce the same component placements as the PCB document.
- Copper has physical thickness and plated-barrel geometry; changing a trace width or copper thickness recomputes the native geometry after reopening.
- Individual copper-object identity and full-net membership survive native persistence and map correctly to display geometry. Net-aware inspection in standalone Solidcraft requires shared metadata/selection support and must be tested rather than assumed from native file support.
- Optional STEP export matches the native evaluated model geometrically, without becoming the source of that model.

These are release acceptance requirements; only the subset recorded in [foundation evidence](foundation-evidence.md) has passed. Any remaining native composition API gap is an explicit first-release integration task, not grounds to substitute a flattened STEP assembly.
