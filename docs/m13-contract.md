# M1–M3 contract: generated 3D models with physical materials

This contract fixes the types and interfaces three parallel tasks build on. It implements slices M1–M3 of the footprint and 3D design (section B). Every body is assigned a physical material, which the product look renders. Every instance of a package revision shares one native model definition.

Each stub either has the simplest correct body or refuses explicitly, with an `*.unsupported` error code. A task that owns a stub replaces it. Field names and signatures change only with an update to this document.

## Types and where they live

| Item | Location | State |
| --- | --- | --- |
| `materials` module: `MaterialId` (24 physical materials), `MaterialCategory`, `MaterialDef`, `Coat`, `Transmission`, `SurfacePattern`, `PatternKind`, `SourceNote`, `builtin(id)` | `crates/editor-core/src/materials.rs` (new core row; may use `diagnostics`; `library`, `updates`, `commands` may use it) | `builtin` refuses `material.unsupported` |
| `Point3`, `Transform3` (`rot_x/y/z`, `intrinsic_xyz`, `then`, `apply`), exact at quarter turns | `crates/editor-core/src/geometry/frame3.rs` | done, tested |
| `PackageForm` (`Generic`, `Chip`, `GullWing`, `NoLead`, `Header`; boxed parameters), `ChipForm`, `ChipKind`, `GullWingForm`, `NoLeadForm`, `HeaderForm`, `LeadSides`, `PackageForm::{label, lengths, envelope, values}` | `crates/editor-core/src/library/package_form.rs` | `values` emits only `pkg_e`; `envelope` is a first derivation; no range findings yet |
| `PackageModel` (`Envelope { material }`, `Generated { family_rev, materials }`, `Imported(ImportedModel)`), `BodyRole`, `RoleMaterial`, `GeneratedMaterials::material`, `default_material`, `ImportedModel` (empty M4 placeholder) | `crates/editor-core/src/library/package_model.rs` | `Imported` gives the blocking finding `package.model_unsupported` |
| `ModelPlacement { translation, rotation_deg }` (intrinsic X→Y→Z, degrees), `transform`, `finite`, `quarter_turns` | same | done |
| `model_frame(&Datum, &ModelPlacement) -> Transform3` (M = D ∘ placement) | same | done, tested |
| `PackageEdit` (`SetForm`, `SetModel`, `SetPlacement`, `ApplyQuick(QuickAction)`), `QuickAction`, `Axis`, `edit_package` | same | `ApplyQuick` refuses with `package.quick_unsupported` |
| `PackageDefinition.{form, model, placement}` | `crates/editor-core/src/library/package.rs` | new required fields; `body_size`, `model_offset` and `body` remain until task 1 replaces them |
| `LibraryCommand::EditPackage { draft, edits }` (`library.edit_package`) | `crates/editor-core/src/commands/library.rs` | done: atomic, one undo step |
| `LibraryCommand::FitLeads { draft, pairs, rotation, confirm }` (`library.fit_leads`), `Outcome::LeadsFitted { fit, applied }` | same, `commands/mod.rs` | dispatches to `fit_leads` |
| `FitRotation`, `LeadPair`, `LeadResidual`, `LeadFit`, `fit_leads`, `LEAD_OFF_LAND_FRACTION` | `crates/editor-core/src/library/package_fit.rs` | `fit_leads` refuses with `package.fit_unsupported` |
| `models` module: `MODEL_BODIES_KEY` (`org.pcbcraft.electronics.models.N`), `PARAMETER_PREFIX`, `family_rev`, `GeneratedBody { name, role, material, terminal }`, `ModelDefinition`, `build_definition(&mut Native, component, &PackageDefinition)` | `crates/solidcraft-integration/src/models/mod.rs` | `build_definition` refuses with `model.unsupported` |

Frames: the footprint origin is F. The datum is D = F ∘ `Datum { offset, rotation }`, and the model frame is M = D ∘ `ModelPlacement`. z = 0 is the seating plane, at the top of the land copper. On the board, a bottom-side part is Rx(π) then Rz(θ), as `layout::placed_point` does today.

The CLI and MCP schemas are generated from these types (`Engine::schemas`), so `library.edit_package` and `library.fit_leads` appear in them automatically.

## Parallel tasks and the files each owns

Shared files take additive edits only: `library/mod.rs` re-exports, `lib.rs`, `commands/mod.rs` `Outcome`, `xtask/src/core_modules.rs`, `docs/component-model.md` and `GLOSSARY.md`.

### 1. Core (M1)

- Fill `materials::builtin` with the cited table. Move or mirror the optical values from `solidcraft-integration/src/material.rs` and `docs/materials.md`, and keep the cited and **est.** conventions. The integration crate keeps the `BodyKind` → `MaterialId` mapping.
- Replace `body_size`, `model_offset` and `body` with `form`, an envelope and `model`/`placement`. Pre-release, so replace the types and add no legacy readers. Update every user: the samples, templates, the integration `project.rs`/`appearance.rs` readers (a mechanical rename only) and `footprint_source::package_values`, which also gains `PackageForm::values`.
- Write `PackageForm` range findings and bounds, `model_frame` users (the courtyard and 2D), and update diffs (`updates`) for form, model and placement changes.
- **Files:** `crates/editor-core/src/library/package*.rs` (except `package_fit.rs`), `crates/editor-core/src/materials.rs`, `crates/editor-core/src/geometry/frame3.rs`, `crates/editor-core/src/updates*`, `docs/materials.md`, `docs/component-model.md`.

### 2. Builders (M2)

- Implement `models::build_definition` per family:
  - chip: body and two terminations;
  - gull-wing: body, and leads with bends and standoff;
  - QFN/DFN: body, exposed pad and side terminals;
  - pin header: PBT/LCP body and gold pins.
- Build each with Solidcraft features driven by named parameters (`PARAMETER_PREFIX`).
- Store `BodyRole` and `MaterialId` per body in `MODEL_BODIES_KEY` chunks (≤ 48 KiB, ≤ 200 chunks; overflow is a refusal), and save the opaque PBR projection through `appearance.assign`.
- Build one definition per package revision in `project.rs`, used by every instance occurrence, and verify multi-occurrence reuse.
- The PCB 3D and Component 3D views render it with the fork's per-triangle product look.
- **Files:** `crates/solidcraft-integration/src/models/*.rs`, the model hookup in `project.rs`, the `MaterialId` → Look/`OpaqueSurface` mapping (a new `models/look.rs` or additive edits to `material.rs`), and `apps/pcbcraft/src/model_scene.rs` where needed.

### 3. Placement (M3)

- In the Component 3D tab, add:
  - numeric translation and rotation fields through the kit, sending `PackageEdit::SetPlacement`;
  - quick actions (implement `ApplyQuick` in `package_model::edit_package`, which is the one function this task edits in that file);
  - a triad adapter following Solidcraft's gizmo conventions;
  - point-to-point snap;
  - "fit leads to pads" (implement `package_fit::fit_leads`, a 2D Kabsch fit, then seat it);
  - the seating plane on the board;
  - findings (leads off pads, body below the board).
- Each gesture is one typed command. A drag changes only the preview transform and never rebuilds geometry.
- **Files:** `apps/pcbcraft/src/part_editor/model_*.rs`, `crates/editor-core/src/library/package_fit.rs`, the `ApplyQuick` arm of `edit_package`, and viewport preview-transform hooks (additive).

## Open points for the tasks

- `FitLeads.pairs` comes from the host, which extracts lead points through the integration crate. Task 3 may replace the pairs with a host trait in the style of `FootprintEvaluator` if that keeps CLI and MCP self-sufficient.
- `MaterialDef` has no saved form in a project snapshot yet. Task 1 decides whether the snapshot copies definitions (as the design says) or pins the app's material revision.
