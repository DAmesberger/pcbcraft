# F1 contract: footprint types and interfaces

Status: contract, 31 October 2026. This is the shared base for the parallel F1/F2 tasks. It fixes the types and the seams; it adds as little behaviour as possible. Source design: the footprint and 3D design (F1, F2), the graphics design (its G3 content folds into F1) and the [F0 spike](footprint-spike.md).

**What changed for existing code.** The old `Pad { id, terminal, position, size }` and `FootprintDefinition.datum_offset` are gone (pre-release: replaced, no legacy reader). Every existing footprint is a rectangular top-side SMD pad built with `Pad::smd_rect`, so behaviour is unchanged for those pads. Rectangle-only consumers read `pad.shape.extent()` and `footprint.datum.offset` until their owning task makes them shape-aware.

## Types and where they live

| Types | Module | Notes |
|---|---|---|
| `FootprintDefinition { package, next_local, source, pads, graphics, polarity, datum, defaults }` | `crates/editor-core/src/library/footprint.rs` | `source: Option<Box<FootprintSource>>`; `None` is an explicit footprint. |
| `Pad`, `PadKind` (Smd, PlatedThrough, NonPlatedHole, Fiducial, Mechanical), `PadSide`, `PadShape` (Rect, RoundRect, Circle, Oblong, Custom), `CornerMask`, `Outline`, `Segment` (Line, bulge Arc), `Hole`, `HoleShape` (Round, Slot), `MaskRule`, `PasteRule`, `PasteWindow`, `PadVia`, `ApertureId`, `Datum { offset, rotation_deg }`, `PadDefaults` | same | `Pad.terminal` is `Option`; `Pad.number` is display only. Constants: `MASK_EXPANSION_MM`, `MAX_OUTLINE_SEGMENTS`, `MAX_PASTE_WINDOWS`, `MAX_PAD_VIAS`, `MAX_BULGE`. |
| `Pad::smd_rect`, `Pad::bounds`, `PadShape::{extent, set_size, canonical_rotation}`, `FootprintDefinition::{polarity_pad, envelope_outline}` | same | `canonical_rotation`: rect, oblong and fully rounded rect → [0°, 180°); circle → 0; custom and partly rounded → [0°, 360°). |
| `FootprintGraphic { id, layer, side, item }`, `DrawingLayer` (Silkscreen, Courtyard, Assembly), `Side` (Component, Opposite), `Polarity { pad, markers }`, `designator_placeholder`, `PlacedText` | `crates/editor-core/src/library/graphics.rs` | `PlacedText` moved here from `schematic` (still re-exported there) so `layout` can use it. Constants: `MAX_POLARITY_MARKERS`, `FOOTPRINT_TEXT_HEIGHT_MM`, `SILK_STROKE_MM`. |
| `FootprintSource` (with `solve: SolveSummary`), `EvaluatorRef`, `SketchBlob` (opaque canonical JSON), `SketchPointId`, `SketchCurveId`, `SketchConstraintId`, `SketchRef`, `ParamName`, `Expr`, `ParamUnit`, `FootprintParam`, `ParamValue` (Length, Angle, Count), `PackageValue`, `package_values`, `PadRole`, `PadGeometry`, `PadTemplate`, `PadTemplateParams`, `HoleRole`, `MaskRuleExpr`, `PasteRuleExpr`, `ViaGridRole`, `ArrayId`, `PadArray`, `ArrayLayout`, `Pin1Position`, `NumberingScheme`, `SkipRule`, `GenerationKey`, `KeyedPad`, `KeyedOverride`, `PadOverride`, `PadShapeExpr`, `GraphicRole`, `GraphicSource`, `DerivedGraphic`, `SolveSummary`, `SolveStatus` | `crates/editor-core/src/library/footprint_source.rs` | Dimensions bind to parameters through SolidCraft's `Constraint.param` (inside the blob), so there is no binding table. Package values are `pkg_<name>`, typed. Bounds constants: `MAX_SKETCH_BYTES`, `MAX_SKETCH_POINTS`, `MAX_SKETCH_CONSTRAINTS`, `MAX_PARAMETERS`, `MAX_ARRAYS`, `MAX_EXPR`, `MAX_FOOTPRINT_EDITS`. |
| `FootprintEdit`, `SketchEdit`, `GeomConstraint`, `DimKind`, `DimValue`, `SketchOutcome` | same | Trim, fillet, offset, split and mirror are F6 (scratch-`Session` bridge). |
| `FootprintEvaluator` trait (`evaluator`, `apply_sketch`, `evaluate`), `EvaluationRequest`, `Evaluation`, `EvaluatedPad`, `commit` | same | `commit` stores an evaluation: allocates `PadId`s for `new_keys` from `next_local`, replaces derived graphics by role id, refuses on any blocking finding and leaves the footprint unchanged. `FakeEvaluator` (`#[cfg(test)]`, `pub(crate)`) returns a fixed `Evaluation`. |
| `placement_transform(&Placement, &Datum) -> Transform2`, `placed_pads -> Vec<PlacedPad>`, `placed_graphics -> Vec<PlacedGraphic>`, `PlacedPad`, `PlacedGraphic`, `PlacedItem`, `BoardDrawing`, `MAX_PLACED_GRAPHIC_VERTICES` | `crates/editor-core/src/layout/placed.rs` (re-exported from `layout`) | `placed_pads` includes terminal-less and unconnected pads. `placed_graphics` resolves `Designator` and `Literal` text; other text content is empty for now. `layout::pads`/`WorldPad` stay as the rectangle checking approximation. |
| `Engine::apply_with(expected_revision, operation, &dyn FootprintEvaluator)` | `crates/editor-core/src/lib.rs` | Threads the evaluator into `commands::apply_library(document, command, evaluator)`, which ignores it today. `apply` passes `None`. |

## Parallel tasks and the files each owns

Each task owns the files listed. Edit another task's file only to fix a compile break the contract caused, and keep that edit minimal. A contract change (a new field or variant in the files above) goes in its own small commit, listed in that task's report so the merger can order it.

### 1. Core rules

Owns:
- `library/footprint.rs`: the bodies of `findings` (all F1 publish rules: `footprint.terminal_required`/`terminal_forbidden`, `hole_outside_land`, `paste_window_outside_pad`, shape validity), `within_bounds` and `geometry_finite`;
- `library/footprint_source.rs`: `FootprintSource::within_bounds` and the core syntax checks (`ParamName`, `Expr`, reserved `pkg_`);
- `library/graphics.rs` footprint lint (courtyard missing, polarity on more than 2 pads, mask web), `library/graphics/edit.rs` (`GraphicTarget::Footprint`, `SetLayer`, `SetPolarity`);
- `layout.rs` `validate` (enforce `MAX_PLACED_GRAPHIC_VERTICES`);
- `updates.rs` (pad diffs by `PadId`: added, retired, moved, reshaped, terminal, number, hole, mask/paste; graphic diffs; array growth and shrink in the impact preview);
- `library/samples.rs`, `catalog/synthetic.rs` and a new `library/templates/footprint.rs`;
- `docs/component-model.md`, `GLOSSARY.md`, `docs/part-library-design.md`;
- core tests in `crates/editor-core/src/tests/library.rs` and `updates/tests.rs`.

### 2. Integration: copper, mask/paste, appearance

Owns:
- `crates/solidcraft-integration/src/project.rs` and `project/tests.rs`: native copper for every `PadShape` and `Hole` (plated barrels through the stack), pad rotation and `Datum.rotation_deg`, using `layout::placement_transform`;
- `crates/solidcraft-integration/src/appearance.rs`: display-only mask and paste openings following the shapes (never saved in `.solidcraft`), 3D appearance of new kinds.

### 3. PCB 2D

Owns `apps/pcbcraft/src/pcb_editor.rs` (the PCB 2D canvas and its layer controls): drawing every shape, hole and graphic layer from `layout::placed_pads` and `layout::placed_graphics`, layer toggles, bottom-side display, picking through core queries. Add any new core hit-test query to `layout/placed.rs` in a separate commit.

### 4. Component Footprint tab

Owns `apps/pcbcraft/src/part_editor.rs` and `apps/pcbcraft/src/part_editor/` (footprint tab and inspector): editing shape, rotation, holes and mask/paste through the kit; drawing through core queries (`Pad::bounds`, `envelope_outline`, `PadShape`). Graphic editing goes through task 1's `library.edit_graphics` footprint targets.

### 5. F2: evaluator and commands

Owns:
- a new `crates/solidcraft-integration/src/footprint/` module: `SketchEvaluator` implementing `FootprintEvaluator` (`check_sketch`, parameters and typed `pkg_*`, apply, solve, copper-only profile pass, classification, holes, arrays, quantizer and hash), golden tests, the wasm run next to `wasm-scripting` in `xtask`;
- the `LibraryCommand::EditFootprint` (and template, assign-terminals) variants in `crates/editor-core/src/commands/library.rs`, their use of the evaluator, the `library.footprint_preview` query in `lib.rs`;
- CLI and MCP wiring in `crates/automation` and `apps/pcbcraft-cli` (pass `SketchEvaluator` to `apply_with`).

The spike under `crates/solidcraft-integration/src/footprint_spike/` stays reference code; task 5 may delete it once its tests are ported.

## Known gaps left for the tasks

- `layout::pads` and the native adapter still treat every pad as a rectangle of its extent; the adapter ignores pad and datum rotation (task 2).
- `placed_graphics` leaves `ComponentName`, `UnitLabel` and `Field` text empty (task 3, with core help from task 1).
- No publish finding checks pad kinds against terminals yet (task 1).
- `apply_with` accepts an evaluator that no command uses yet (task 5).
