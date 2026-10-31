# F4+F5 contract: footprint sketch UI and templates

This contract fixes the types and seams that four parallel tasks build on. It covers slice F4 (the sketch UI in the Component Footprint tab) and slice F5 (family templates, explicit derived courtyard and assembly roles, footprint lint and board courtyard checks). Behaviour that comes later either refuses with an explicit `Unsupported` error or returns the simplest correct result. Nothing panics, and nothing is left unfinished with `todo!` or `unimplemented!`.

The governing design is the footprint design (§2–3), together with [footprint-spike.md](footprint-spike.md) and the merged F1/F2 code. The rules that do not change:

- Every gesture or form is one `library.edit_footprint` commit (or one `library.footprint_from_template`). A failure leaves the draft unchanged.
- Copper stays exact geometry. Display-only layers are never saved. The core stays pure and wasm-compatible.
- Courtyard and assembly outlines are never inferred. They exist only as authored graphics or as explicitly added `Derived` graphic roles.

## Types and where they live

| What | Where | State |
| --- | --- | --- |
| `TemplateKind`, `FootprintTemplate` (`Chip`, `GullWing`, `Qfn`, `PinHeader`), `ChipTemplate`, `GullWingTemplate`, `QfnTemplate`, `ExposedPad`, `ExposedVias`, `HeaderTemplate`, `FootprintTemplate::defaults`, `TEMPLATE_COURTYARD_EXCESS` | `crates/editor-core/src/library/templates/footprint_family.rs` | Types final. `edits()` refuses with `footprint.template_unsupported`. |
| `LibraryCommand::FootprintFromTemplate { draft, template: Box<FootprintTemplate> }` (`library.footprint_from_template`) | `crates/editor-core/src/commands/library.rs` | Final. Runs `footprint_family::edits` and then the `EditFootprint` path, so it is one undo step and needs the evaluator. The CLI and MCP schemas are generated from it. |
| `DerivedGraphic::{CourtyardFromExtents { excess, round }, AssemblyFromEnvelope}`, `GraphicRole`, `FootprintEdit::SetGraphicRole` | `library/footprint_source.rs` (F2) | Unchanged. Templates add these roles explicitly. |
| `MIN_SILK_WIDTH_MM` (0.1, est.), `MIN_SILK_TEXT_HEIGHT_MM` (0.6, est.) | `library/footprint/checks.rs`, re-exported from `library` | Constants only. The lint rules `footprint.silk_width` and `footprint.text_height` come with the templates task. |
| `PlacedCourtyard`, `placed_courtyards`, `courtyard::findings`, `COURTYARD_OVERLAP` (`board.courtyard_overlap`) | `crates/editor-core/src/layout/courtyard.rs` | `placed_courtyards` is final. `findings` returns nothing for now and is already called by `layout::checks`. |
| `WorldPad::frame()`, `WorldPad::outline()` (the exact pad outline on the board) | `crates/editor-core/src/layout.rs` | Final. Checks still use `pad_polygon` until the boardgeo task switches them. |
| `SketchView`, `ViewPoint`, `ViewCurve`, `ViewGeometry`, `ViewConstraint`, `sketch_view` | `crates/solidcraft-integration/src/footprint/view.rs` | Final. It is a read-only, id-keyed view at the stored solved positions. |
| `SolveReport`, `solve_report`, `drag_preview` (`Budget::frame`), `dimensions_using` | same file | These refuse with `Unsupported`. The dims task fills them in. |
| `FootprintTool` (with `ALL`, `id`, `from_id`; ids are prefixed `fp-`), `ArrayTool`, `ActiveLayer`, `Gesture`, `ToolState` (`arm`, `cancel`, `press`), `commit(draft, edits)` | `apps/pcbcraft/src/part_editor/footprint_tools.rs` | Vocabulary final. `press` returns `Gesture::None` for now. `on_tool` already arms the `fp-*` ids. |
| `DimInput`, `parse_input`, `input_edits`, `InlineEdit` | `apps/pcbcraft/src/part_editor/footprint_dims.rs` | Final, with tests. Drawing, picking and the inline editor are not built yet. |
| `ParamRow`, `rows(source, package_values)` | `apps/pcbcraft/src/part_editor/footprint_params.rs` | Final. Values and the "used by" column are not filled in yet. |
| `TemplatePicker` (`open`, `close`), `request(draft, template)` | `apps/pcbcraft/src/part_editor/footprint_template_picker.rs` | Final. The form UI is not built yet. |

The new app modules carry `#![expect(dead_code, …)]`. The task that wires a module removes its `expect` once everything in that module is used; the compiler reports when the expectation is no longer met.

## Parallel tasks and the files each owns

Edits to shared files stay additive: new match arms, new fields, new calls. Do not reformat or move existing code there.

### 1. tools: the sketch tool state machine

- **Owns:** new `apps/pcbcraft/src/part_editor/footprint_tools*.rs`.
- **Minimal hooks in `part_editor.rs`:** the `ribbon` and `footprint_groups` "Sketch", "Pads" and "Layer" groups with real tool buttons; canvas dispatch from `footprint_canvas.rs` into `ToolState`.
- **Pad templates:** Rect, RoundRect, Circle and Oblong via `FootprintEdit::AddPad`. Space rotates the pad by a quarter turn.
- **Custom pad:** `ConvertToCustom`, or a profile pad.
- **Arrays:** linear, dual row, quad with per-side placement, and grid with skips, via `AddArray`.
- **Holes:** drill and slot (`HoleRole`); a bare click makes an NPTH pad.
- **Drawing:** line, arc, circle and rect on the `ActiveLayer`, through sketch edits plus a `SetGraphicRole` with `GraphicSource::Curves`.
- **Selection and editing:** selection, move (`SketchEdit::Move`/`Drag`) and snapping, reusing `drawing.rs` and `snap.rs`.
- **Keymap:** checked against Solidcraft's keymap.
- **Reads:** `pcbcraft_solidcraft::footprint::view::sketch_view` for drawing, picking and snap targets.
- **Commits:** through `footprint_tools::commit`.
- Dimension and constraint gestures are routed to `footprint_dims`.

### 2. dims: dimensions, constraints and the parameters panel

- **Owns:** new `apps/pcbcraft/src/part_editor/footprint_dims*.rs` and `footprint_params*.rs`, plus `crates/solidcraft-integration/src/footprint/view.rs`. New evaluator modules under `crates/solidcraft-integration/src/footprint/` are allowed if needed.
- **Dimensions:** driving dimensions bound through `DimValue::Param` (Solidcraft `Constraint.param`). Inline editing goes through `parse_input` and then `input_edits` (`SetDimension`, `SetParameter`).
- **Constraints:** constraint glyphs, the `Dimension` tool and the `Constrain` tools via `SketchEdit::AddDimension`/`AddConstraint`.
- **Solver feedback:** `solve_report` drives the under- and over-constrained colouring. Live drags use `drag_preview` with `Budget::frame` and never touch the document; the release commits one `SketchEdit::Drag`.
- **Parameters table:** name, expression with units, typed value and "used by" (`dimensions_using`). `pkg_*` rows are read-only. Rename, set and remove use `RenameParameter`, `SetParameter` and `RemoveParameter`.

### 3. templates: F5 core templates, roles, lint and courtyard checks

- **Owns:** `crates/editor-core/src/library/templates/footprint*.rs`, `crates/editor-core/src/library/footprint/checks.rs`, `crates/editor-core/src/layout/courtyard.rs`, and `apps/pcbcraft/src/part_editor/footprint_template_picker.rs`. Also a small "New from template" ribbon hook in `part_editor.rs`.
- **`footprint_family::edits`:** pure functions returning `Vec<FootprintEdit>` (at most `MAX_FOOTPRINT_EDITS`) for each template:
  - 2-terminal chip;
  - SOIC/SOP gull-wing: seed plus `DualRow`, counter-clockwise numbering;
  - QFN/DFN: `Quad`, an exposed pad with `PasteRuleExpr::Windows` and optional `ViaGridRole`, and an exposed-pad terminal left for an explicit choice;
  - through-hole pin header: grid or dual row, with a square pin 1 through `SetOverride`.
- **Template parameters:** each template is driven by named parameters (`SetParameter`). Defaults derive from `pkg_*` values.
- **Derived roles:** templates add explicit `Derived` courtyard roles (`CourtyardFromExtents` with an `excess` parameter) and assembly roles. They are never auto-inserted elsewhere.
- **Footprint lint:** adds `footprint.silk_width` and `footprint.text_height` to the existing rules: pad outside courtyard, silk over pad, missing courtyard (advisory), and pin-1 marker for more than 2 pads.
- **Board check:** `courtyard::findings` reports `board.courtyard_overlap` between different placements on the same side.
- **"New from template":** the Footprint tab sends `template_picker::request`.

### 4. boardgeo: real pad shapes in board checks and connectivity

- **Owns:** `crates/editor-core/src/layout.rs` (pads, checks, airwires, clearance; it does not touch `layout/courtyard.rs`), `crates/editor-core/src/layout/placed.rs`, `layout/tests.rs`, and the canvas parts of `apps/pcbcraft/src/pcb_editor.rs`.
- **Shape-aware geometry:** checks, clearance, airwires and the routing snap use `WorldPad::outline()` and `PlacedPad::outline()`, built on the core pad outlines in `library/footprint/geometry.rs`, instead of `pad_polygon` rectangles.
- **Caching:** board pad geometry is cached per document revision.
- **PCB 2D view:** zoom about the cursor.

## Not in this contract

- Trim, fillet, offset, split and mirror (F6, FD2b).
- The land calculator (F6).
- SVG import (a separate slice).
