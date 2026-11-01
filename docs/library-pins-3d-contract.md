# Component catalog, templates, pin functions and 3D library: contract (C0)

**Track "models" (D0–D6) is built and integrated (1 November 2026).** Its as-built description, including the integration choices below, is [D0–D6 as built](component-model.md#3d-model-library-project-container-and-markings-d0d6-as-built); its rows here are kept only as the record of what the fan-out owned. Integration wired D2's board hook (`project::build` builds a package's library model once per package revision), the library-model Component 3D preview (`scene::library_asset` and `imported::preview`), D5's decals into the model worker as their own display-only `BodyKind::Marking` with the D6 Markings toggle, and the import wizard's frame and contacts through D2's functions (contacts measured off the UI thread at Finish). It removed the `marking.unsupported`, `model.unsupported` and `package.model_unsupported` finding codes; `model.unsupported:` remains a refusal prefix of the integration crate for models it does not build yet (B2 families, `.solidcraft`). Tracks "libui" and "pins" still follow this contract.

**Track "pins" (C1–C7) is built and integrated** (1 November 2026): its behaviour, limits and evidence are in [pin functions as built](component-model.md#pin-functions-peripherals-rules-pinout-import-and-firmware-c1c7-as-built) and the [editor evidence](editor-evidence.md#pin-functions-c1c7), and its terms in the [glossary](../GLOSSARY.md). Its rows below record the ownership it worked under; as-built differences are noted in [Pins track as built](#pins-track-as-built). Track "libui" is still built against this contract.

Status of the rest (track "libui"): contract only. Every type, command, query, schema, finding code, module-map row, layer rule and limit below exists and compiles. Every new handler refuses with an explicit error (`Error::Unsupported` in core and firmware, `Error::Invalid("<code>: …")` in io and the integration crate, which have no `Unsupported` variant). No handler panics or uses an unfinished-code macro. The UI call sites are wired to stub modules that draw nothing, so the app looks and behaves as before. The design behind this is the "Component catalog, templates, pin functions and 3D library" design note of 1 November 2026, with its founder decisions F1–F8. **F2 is decided differently from the note's recommendation: pin rules are sandboxed Rhai scripts** (see [C1](#pin-rules-c1-rhai-scripts-f2)).

`tests/contract.rs` (core) and the automation `session/*_tests.rs` files pin the contract: the new queries and commands exist on the wire, refuse as `unsupported`, leave the document and its revision unchanged, and existing documents serialize exactly as before (every new field is `serde(default)` and skipped when empty).

## Rules for the fan-out

1. **Ownership is disjoint across tracks and across slices.** A slice edits only the files it owns (tables below) plus new files under its own paths.
2. **Shared C0 files are frozen.** Nobody edits these during the fan-out: `crates/editor-core/src/lib.rs`, `ids.rs`, `model.rs`, `commands/{mod,library}.rs`, `library/{mod,publish,component,part}.rs`, `issues/rules.rs`, `crates/automation/src/session.rs`, `apps/pcbcraft-cli/src/main.rs`, `xtask/src/{core_modules,layers,clean_room}.rs`, the workspace `Cargo.toml`, and the dispatch sites in `part_editor.rs` and `schematic_editor.rs`. If a slice needs a change there, it reports the change and slice I makes it. The finding codes below are already registered in `KNOWN_CODES`. A slice that emits only those codes never touches `issues/rules.rs`.
3. **Stubs flip in place.** A slice replaces its stub bodies and turns on its gates (`pins_tab::AVAILABLE`, the family files' `AVAILABLE` and `FAMILY_REV`, the CLI modules' `USAGE`). It never adds a dispatcher arm.
4. **Each slice records evidence the way its existing tests do.** The contract tests that assert `unsupported` belong to the slice whose stub they cover; that slice replaces them with behaviour tests.
5. Clean room, `Result`-only production code, bounded inputs and one undo step per command apply to every slice, as in AGENTS.md. Limits marked "target" stay targets until someone measures them.

## Types and where they live

### Core (`pcbcraft-core`)

| Area | Path | Types and functions |
| --- | --- | --- |
| Identity | `ids.rs` | `DefinitionKind::{Model, PackageRow}`, `ModelRef`, `PackageRowRef`, `RuleId` (component local ID) |
| Library store | `library/mod.rs` | `Definition::{Model(Box<ModelDefinition>), PackageRow(Box<PackageRowDefinition>)}`, `Library::{models, package_rows}` (serde-skipped when empty), `Library::{model, package_row}`. Publish rank: quantity kind 0, field 1, **row and model 2**, package 3, footprint 4, binding 5, symbol 6, component 7, Part 8. Raised limits: `MAX_TERMINALS` 2048, `MAX_PACKAGE_TERMINALS` 2048, `MAX_PADS` 4096 (targets) |
| Templates (B1) | `library/templates/catalog.rs` | `PackageRowDefinition { family: Option<FamilyRef>, values: PackageForm, aliases, size_code: Option<SizeCode>, lands: Option<LandValues>, skeleton: SkeletonHint, provenance, citation: Option<RowCitation> }`, `FamilyKind` (8 families, `ALL`, `of(form)`), `FamilyRef`, `RowSource::{Project, Bundle, Custom}`, `NewFromTemplate`, `PackageFromTemplate`, `PackagePlan`, `SavePackageRow`, `RowFrom`. Limits: `MAX_PACKAGE_ROWS_PER_SOURCE` 4096, `MAX_ROW_ALIASES` 64, `MAX_ALIAS` 64 |
| Families (B2) | `library/package_form.rs` + `package_form/{tabbed,molded,grid,tht}.rs` | `PackageForm::{Tabbed, Molded, Grid, Tht}` (boxed `TabbedForm`, `MoldedForm`, `GridForm`, `ThtForm`), trait `FamilyForm` (label, lengths, envelope, lead_count, terminal_counts, lead_points, values, range_problems). Every `PackageForm` method dispatches later families to `family_form()`. The stubs' `range_problems` refuse with "not built yet", so `package.form_range` blocks publishing |
| Footprint templates (B2) | `library/templates/footprint_family.rs` + `footprint_family/<family>.rs` | `TemplateKind::{Tabbed, Molded, Grid, Tht}` (`ALL` has 8; `available()` reads each file's `AVAILABLE`), `FootprintTemplate::{Tabbed, Molded, Grid, Tht}` with `<Family>Template { …, courtyard_excess }`, `defaults`, `exprs`, `build` (stub: `footprint.template_unsupported`) |
| Pin data (C2) | `functions/pin_config.rs` | `MuxCode`, `SourceSpan`, `GpioRef`, `Pull`, `Level`, `PinOption`, `PinConfig`, `PinConfigs`, `PinConfigEdit`, bounds helpers (implemented), `check_config` (stub). New fields: `Function::{mux, source}`, `TerminalFunctions::{gpio, options}`, `Resource::kind`, `Role::signal`, `ComponentInstance::pin_config`. Limits: `MAX_RESOURCES` 256, `MAX_CONSTRAINTS` 1024, `MAX_KIND` 32, `MAX_MUX_CODE` 16, `MAX_PIN_OPTIONS` 8, `MAX_PIN_CONFIG_EDITS` 256 |
| Pin rules (C1) | `functions/rules.rs` | `PinRuleScript { source }`, `PinRule { id, label, script, source }`, `RuleView` (`ViewTerminal`, `ViewFunction`, `ViewResource`), `RuleVerdict::{Ok, Forbid, Require, Warn}`, `RuleNote`, `RuleFailure` and `RuleFailureKind::{Compile, Budget, Runtime}`, `RuleOutcome`, `RuleEvaluation`, `RuleBudget`, opaque `CompiledRules`; `compile`, `evaluate`, `declaration_findings`, `within_bounds`. New: `ComponentDefinition::pin_rules`, `Conflict::Rule`, `Requirement::Rule`, `Resolution::notes` |
| Solver (C2) | `functions/suggest.rs` | `PeripheralRequest`, `SuggestRequest`, `Suggestion`, `Blocker`, `SuggestReport`, `TerminalHint`, `SuggestInput`, `suggest`. Limits: `SUGGEST_NODE_BUDGET` 100 000 (target), `MAX_SUGGESTIONS` 8 |
| Instance pins (C2) | `instances/pins.rs` | `AssignPeripheral`, `RolePlacement`; `assign_peripheral`, `set_pin_config` |
| Pin report (C2) | `pins.rs` (new module 13a) | `PinReport`, `ReportPin`, `ReportEndpoint`, `ReportFunction`; `pin_report`, `suggest` |
| Pinout import (C3) | `library/pinout.rs` | `PinoutDocument` (format 1: `PinoutDevice`, `PinoutVariant`, `PinoutPeripheral`, `PinoutRole`, `PinoutPin`, `PinoutFunction`, `PinoutOptions`, `PinoutClass`, `PinoutRule` with a Rhai `script`), `PinoutTarget`, `PinoutPlan` (`base`, `PinoutChange`, findings, `ReviewItem`/`ReviewReason`), `PinoutAccept`, `PinoutApply`, `PinoutApplied`, `ImportReview` (`ComponentDefinition::review`), `schema()`. Limits: 2048 pins, 256 peripherals, 16 variants, 1024 rules |
| Firmware IR (C4) | `firmware.rs` (new module 13b) | `PinConfigReport` (format 1), `FirmwareDevice`, `FirmwarePeripheral`, `FirmwarePin`, `NameConflict`; `pin_config`, `schema()` |
| 3D library (D1) | `library/model3d.rs` | `ModelDefinition { asset, pair, format, frame, bounds_mm, bodies, custom_materials, leads, marking_area, hints, provenance }`, `AssetRef`, `ModelFormat::{Step, Gltf, Solidcraft}`, `SourceFrame`, `UpAxis`, `ModelBody`, `ModelMaterial::{Builtin, Custom}`, `CustomMaterial`, `LeadTag`, `MarkingArea`, `ModelHints`, `Mount`, `Redistribution`, `ModelProvenance`, `LibraryModel { model, lead_map }`, `LeadMapEntry`, `ImportModel`. `PackageModel::Imported` is replaced by `PackageModel::Library(LibraryModel)`. Limits: 1024 bodies, 64 custom materials, 2048 lead tags, `BOUNDS_TOLERANCE_MM` 0.1 |
| Markings (D4) | `library/marking.rs`, `library/marking/codes.rs` | `MarkingSpec { items, area, finish, min_text_height_mm }`, `MarkingItem::{Text, Code, Artwork, Polarity, Bands}`, `Template`, `Align`, `Anchor`, `PolarityMark`, `AreaChoice`, `Finish::{LaserEtch, InkWhite}` (a further ink needs a cited material first), `InstanceMarking { hide, vars, replace }`, `MarkingLayout`/`LaidElement`/`LaidText`, `MarkingInputs`; `effective` (implemented precedence), `layout`, `spec_findings`; `CodeScheme`, `TableRef`, `encode`. New fields: `ComponentDefinition::marking`, `PartDefinition::top_mark` (both boxed), `ComponentInstance::marking`. Bundles already rewrite the field references of markings |
| Catalog | `catalog/{templates,models,preview}.rs` | B1 `TemplateHit`, `TemplateReport`, `search`; D1 `ModelHit`, `ModelHitKind`, `FitScore`, `ModelReport`, `search`; A2 `inspect_bundle` |
| Commands | `commands/{templates,pinout,model3d,marking}.rs` | Handlers only; dispatch arms are in `commands/library.rs`, `commands/instance.rs` and `lib.rs` |
| Format (D0) | `format.rs` | `CONTAINER_DOCUMENT`, `CONTAINER_ASSET_PREFIX`, `CONTAINER_MAX_ASSET_BYTES` 64 MiB, `CONTAINER_MAX_BYTES` 512 MiB (targets), `CONTAINER_MAX_ENTRIES` 4096 |

### Wire surface

| Kind | ID | Payload → result | Slice |
| --- | --- | --- | --- |
| Library command | `library.new_from_template` | `NewFromTemplate` → `Outcome::DraftsCreated { ids }` (5 drafts, one undo step) | B1 |
| Library command | `library.package_from_template` | `PackageFromTemplate` → `Outcome::PackageReplaced { plan }` | B1 |
| Library command | `library.save_package_row` | `SavePackageRow` → `Outcome::DraftCreated` | B1 |
| Library command | `library.pinout_apply` | `PinoutApply` → `Outcome::PinoutApplied { report }` | C3 |
| Library command | `library.import_model` | `ImportModel` → `Outcome::DraftCreated` | D1 |
| Instance command | `instance.assign_peripheral` | `AssignPeripheral` → `Outcome::FunctionsSelected` | C2 |
| Instance command | `instance.set_pin_config` | `{ instance, edits: [PinConfigEdit] }` | C2 |
| Instance command | `instance.set_marking` | `{ instance, marking: InstanceMarking }` | D4 |
| Query | `library.templates` | `{ query, limit? }` → `Templates(TemplateReport)` | B1 |
| Query | `library.package_plan` | `{ request: PackageFromTemplate }` → `PackagePlan` | B1 |
| Query | `library.models` | `{ query, fits?, limit? }` → `Models(ModelReport)` | D1 |
| Query | `library.pinout_preview` | `{ target, document }` → `PinoutPlan` | C3 |
| Query | `functions.suggest` | `{ instance, request: SuggestRequest }` → `SuggestPins(SuggestReport)` | C2 |
| Query | `project.pin_report` | `{ instance }` → `PinReport` | C2 |
| Query | `project.pin_config` | `{ instance }` → `PinConfig(PinConfigReport)` | C4 |
| Query | `instance.marking` | `{ instance }` → `Marking(Option<MarkingLayout>)` | D4 |
| Automation | `library.open_preview` | `BundleParams` → the `library.inspect` report of a folder item | A2 |
| Automation | `library.pinout_schema` | `{}` → `{ format, schema }` (**implemented**) | C3 |
| Automation | `firmware.export_preview`, `firmware.export` | `FirmwareParams { instance, emitter, path? }` → `Emitted` | C4 |
| Automation | `library.inspect_model_file` | `ModelFileParams { path, format }` → `ModelInspection` | D2 |
| Schemas | `schemas()` | adds `pinout` and `pin_config` | C3, C4 |

MCP tools are generated from the automation catalog, so every request above is already an MCP tool. Core commands and queries reach MCP through `project.apply` and `project.query`. C7 writes the MCP instructions for the reviewed workflow.

### Finding codes (registered in `issues/rules.rs`)

- **C1 rules:** `rule.bounds`, `rule.script` (compile error), `rule.budget`, `rule.runtime`, `rule.reference`, `rule.default_conflict`.
- **C1/C2 issues:** `config.rule_unmet` (Error, blocks ElectricallyResolved), `config.rule_note` (Warning), `config.pin_option` (an invalid pin config after a library update).
- **C2 pin data (added at integration):** `function.mux`, `function.gpio`, `function.pin_option`, `resource.kind`, `resource.signal` (publish blockers).
- **C3 import:** `pinout.stale_base` is the rule of the conflict refusing a stale plan (a refusal, not a finding).
- **C3/C5 review (F8):** `pinout.uncertain` (blocks publishing), `pinout.unreviewed` (Warning). The review state is draft bookkeeping: publishing drops it from the revision and its draft, so it never enters revision content or snapshot identity. The gate is advisory against automation: any client that can edit the draft (`library.edit`, or a re-import without the flag) can clear it, and nothing tells a person's confirmation from an agent's. Agents are instructed to clear a flag only after the person confirms the row in the conversation. A re-import into a draft without a binding matches rows to terminals by equal name only, and each such row is a `matched_by_name` review item.
- **D4 markings (built):** `marking.template`, `marking.unrepresentable` (Info), `marking.does_not_fit` (Info).
- **D1 models (built):** `model.bounds`, `model.licence`, `model.leads`, `model.body_unmapped`, `model.asset`.
- **B1 rows:** `package_row.unsupported` (contract), `package_row.family`, `package_row.range`, `package_row.alias`, `package_row.provenance`.
- Refusal prefixes, which are not findings: `template.unsupported`, `footprint.template_unsupported`, `library.preview_unsupported`. (`container.unsupported` and the pins prefixes are gone with their stubs.)

### Pin rules (C1): Rhai scripts (F2)

- **Storage.** `ComponentDefinition::pin_rules: Vec<PinRule>`. Each rule has a component-local `RuleId`, a unique label, a `PinRuleScript { source }` of at most 4 KiB, and an optional `SourceSpan`. A component has at most 1024 rules and 256 KiB of rule source in total (target). The script source is part of the revision and so part of snapshot identity. The simple pair rules (`Excludes` and `Requires`) stay typed `FunctionConstraint` data.
- **The script's interface.** A rule is a pure function of a typed, read-only `RuleView`. The view holds each terminal's name, its declared function labels, its active function with that function's peripheral and role labels, and its `PinConfig`. It also holds each peripheral's label, kind and active roles. The script body evaluates to one verdict, built with `ok()`, `forbid(msg[, terminals])`, `require(msg[, terminals])` or `warn(msg[, terminals])`. It reads the view through these helpers: `active("TERM", "function")`, `used("PERIPH")`, `role("PERIPH", "ROLE")`, `option("TERM", "pull")`, `excludes("A.f", "B.g")` and `requires("A.f", "B.g")`. C1 finalises the helper names and documents them in `functions/rules.rs`.
- **The evaluator.** `compile(&[PinRule]) -> Result<CompiledRules>` compiles once per revision and caches by source hash. `evaluate(&CompiledRules, &RuleView, RuleBudget) -> Result<RuleEvaluation>` returns the verdicts in rule order, plus a `RuleFailure` for each compile error, runtime error or exhausted budget. A failure becomes a finding: it never panics, and the document stays unchanged.
- **Verdicts in the resolution.** `forbid` becomes `Conflict::Rule` (rejected, as `Excludes` is). `require` becomes `Requirement::Rule` (`config.rule_unmet`). `warn` becomes `Resolution::notes` (`config.rule_note`).
- **Limits.** 10 000 operations per rule, 1 000 000 per resolution and 20 000 000 per `functions.suggest` (targets). Call depth 16, strings 1024 bytes, arrays 1024 items, maps 256 entries, verdict messages 512 bytes. No modules, I/O, time or randomness. The scripts are deterministic and compile for wasm.
- **The solver.** It uses the scripts as bounded predicates with a total budget and names the first rule that fails in `Blocker::rule`.

## Tracks and slices

Waves follow the design note (wave 2: A1, A2, B1, B2a, C1, C3, D0, D1, D4; wave 3: the rest of B2, B3, B4, C2, C4, D2, D5; wave 4: C5, C6, C7, D3, D6, I). Within one track, slices that share no file can run in parallel.

### Track "libui"

| Slice | Builds | Owns |
| --- | --- | --- |
| **A1** Workspace identity | Explicit open (no first-draft fallback in `WorkingSet::find`), the identity bar, the start page, switcher and history, the read-only folder preview, Edit a copy (one undo step) | `apps/pcbcraft/src/part_editor.rs` (sync, open, header, ribbon Component group; **not** the C0 dispatch lines for Pins, the gallery, the chooser and markings), `part_editor/identity.rs`, `part_editor/start.rs`, `part_editor/working_set.rs`, `apps/pcbcraft/src/app.rs` routing |
| **A2** Catalog Browser | The Component workspace's catalog tree, search, open and place actions, `catalog::preview::inspect_bundle`, `library.open_preview` | `apps/pcbcraft/src/app/browser.rs`, `app/browser_catalog.rs`, `apps/pcbcraft/src/library_sources.rs`, `crates/editor-core/src/catalog/preview.rs`, `crates/automation/src/session/preview.rs` |
| **B1** Template core | Rows in the catalog with `pins:`, `pitch:` and `body:` filters and the size-code rule (`pkg:i0402` and `pkg:m0402`), `new_from_template` (one undo step), `package_from_template` and its plan, `save_package_row`, row publish findings; `Template` and `Preset` leave the app's path | `library/templates/catalog.rs`, `commands/templates.rs`, `catalog/templates.rs`, `catalog/query.rs`, `catalog.rs` (indexing rows), `library/samples.rs` |
| **B2 (a+b)** Tabbed gull-wing, molded 2-terminal | SOT, SOT-223 and DPAK; SOD, SMA/SMB/SMC, tantalum and MELF, including the band marking area: form, range rules, footprint template, model builder and Body rows | `library/package_form/{tabbed,molded}.rs`, `library/templates/footprint_family/{tabbed,molded}.rs`, `crates/solidcraft-integration/src/models/{tabbed,molded}.rs`, `apps/pcbcraft/src/part_editor/family_rows/{tabbed,molded}.rs` |
| **B2 (c+d)** Grid array, THT | BGA, LGA and WLCSP, with A1 numbering and depopulation as data; axial, radial, DIP and TO with drill roles and lead bends | the same four paths for `grid` and `tht` |
| **B3** Starter table | About 150 cited rows over the wave-1 families, the `sample-packages` CLI writer, a provenance review list | `crates/editor-core/src/library/templates/starter/*.json` (new), `apps/pcbcraft-cli/src/sample_packages.rs` |
| **B4** Gallery UI | The gallery sheet (new-component and package scopes), previews, Edit values, Save row; the Footprint tab's From template uses it | `apps/pcbcraft/src/template_gallery.rs`, `part_editor/footprint_template_picker.rs` |

### Track "pins"

| Slice | Builds | Owns |
| --- | --- | --- |
| **C1** Rhai pin rules | The sandboxed evaluator (`compile`, `evaluate`, cache), verdicts in `resolve` (conflicts, incomplete, notes), explain, publish findings (`rule.*`), the ERC and Issues rows of `config.rule_*`, use of the raised limits | `functions/rules.rs`, `functions.rs` (resolve, bounds, declaration findings), `scripting/**` if sandbox helpers are shared, `erc.rs` and `completeness/**` for the `config.rule_*` findings |
| **C2** Peripherals, pin config, solver | Validation of `kind`, `signal`, `mux`, `gpio` and options; `assign_peripheral`; `set_pin_config`; `functions.suggest`; `project.pin_report`; carrying pin config through updates | `functions/pin_config.rs`, `functions/suggest.rs`, `instances/pins.rs`, `pins.rs`, `instances.rs` (pin config is carried through updates by the `instances` carry functions, never in `updates.rs`), `commands/instance.rs` |
| **C3** Pinout import | `pinout_preview` (plan and review items), `pinout_apply` with a stale base, `ImportReview` and the F8 gate, the CLI `pinout` subcommand | `library/pinout.rs`, `commands/pinout.rs`, `apps/pcbcraft-cli/src/pinout.rs`, `crates/automation/src/session/pinout.rs` |
| **C4** Firmware | The `project.pin_config` IR; json, c-header, zephyr-overlay and rust-consts emitters and user templates (a licence review before adding MiniJinja); export requests and the CLI | `crates/editor-core/src/firmware.rs`, `crates/firmware/**`, `crates/automation/src/session/firmware.rs`, `apps/pcbcraft-cli/src/firmware.rs` |
| **C5** Pins tab | Table, chips, peripherals view, Rhai rule editor with syntax check and explain/test panel, Paste table → preview, Unreviewed filter; the terminal card hand-off; sets `pins_tab::AVAILABLE` | `part_editor/pins_tab.rs`, `part_editor/pins_table.rs`, `part_editor/rules_editor.rs`, `part_editor/symbol_card.rs` |
| **C6** Pin-mux panel | The docked panel, suggest, cycle and apply, inline rule explanations, package mini-view, Issues locate | `apps/pcbcraft/src/pin_mux.rs`, `schematic_editor/instance_card.rs` (except the C0 call to `marking_row`) |
| **C7** Automation and MCP workflow | JSON-only scenario tests (import EXM-48 → review → apply → assign → report → export) and the MCP instructions | `crates/automation/src/session/{templates,pinout,firmware,models}_tests.rs`, `crates/automation/src/mcp.rs`, `crates/automation/src/mcp/**` |

### Track "models" (built; see the note at the top)

| Slice | Builds | Owns |
| --- | --- | --- |
| **D0** Container | The ZIP container, raised asset bounds, recovery in the container, refusal tests; switches `read_project` and `write_project` | `crates/io/src/persistence/container.rs`, `crates/io/src/persistence.rs`, `crates/io/src/recovery.rs` and `recovery/**`, `crates/io/Cargo.toml` (zip dependency, after the licence check), `crates/editor-core/src/format.rs`; in `library/bundle.rs` only the size constants at the top |
| **D1** Model kind | `ModelDefinition` rules, `PackageModel::Library` findings, the publish rank, update-plan entries, `import_model`, references and closure of models in bundles, catalog `kind:model` facets and fit ranking | `library/model3d.rs`, `commands/model3d.rs`, `catalog/models.rs` (model search indexes there, never in `catalog.rs`), `library/package_model.rs`, `library/package.rs`, `library/bundle.rs` (except the D0 constants), `library/import.rs`, `updates.rs` and `updates/**` |
| **D2** Import integration | STEP, GLB and `.solidcraft` inspection, glTF KHR extensions through `serde_json`, the frame, building into the board and the preview, lead contacts, the bounds recheck. The board hook in `project.rs` is wired at integration, because that file is being changed elsewhere now | `crates/solidcraft-integration/src/models/{imported,inspect,gltf_ext}.rs`, `crates/automation/src/session/models.rs` |
| **D3** 3D library UI | The model card, the chooser sheet with fit scores, the 6-step import wizard, thumbnails | `apps/pcbcraft/src/model_library.rs`, `part_editor/model_import.rs`, `part_editor/model_body.rs`, `part_editor/model_card.rs` |
| **D4** Marking core | Templates, codes (EIA-96, 3- and 4-digit, capacitor, voltage letter, bands) with cited tables, `layout`, `instance.set_marking`, `instance.marking`, findings | `library/marking.rs`, `library/marking/**`, `commands/marking.rs` |
| **D5** Marking display | Layout → decal mesh (text outline, SVG), finishes, per-instance cache, PCB 3D and Component 3D | `crates/solidcraft-integration/src/marking.rs`, `apps/pcbcraft/src/model_scene.rs` |
| **D6** Marking UI | The 3D tab Marking section, the instance Marking row, the layer toggle | `part_editor/marking_props.rs`, `schematic_editor/marking_row.rs`, `apps/pcbcraft/src/layer_view.rs` |

### Slice I (integration)

Slice I merges the tracks and owns the frozen C0 files above. Its work:

- Fold this document into `component-model.md`, `parts-library.md` and GLOSSARY: peripheral instance, signal role, mux code, pin rule script, package row, model, marking.
- Update the scope lines for F1 and F3: firmware pin configuration and data-driven package templates are in release 1.
- Wire D2's board hook in `solidcraft-integration/src/project.rs`.
- Remove the `*.unsupported` codes that no slice still emits.
- Gather end-to-end evidence.

## Pins track as built

Where the integration departs from or settles the contract above:

- **One rule view.** `functions::rules::view` builds the `RuleView` for the resolution, the solver and the Pins tab's explain panel; nobody keeps a copy. `ViewResource` also lists every declared role (`roles`).
- **Helpers.** C1 finalised the helpers as `active`, `active_count`, `function`, `used`, `role`, `kind`, `option`, `excludes` and `requires`; terminals by name or `#number`, labels without case.
- **Stale plans.** A changed import target is refused as `conflict` with rule `pinout.stale_base`; the base is a content token, not a document revision, so it is not `stale_revision`.
- **Review state.** A re-import whose accepted row is no longer flagged clears that row's uncertainty; a person can also clear it by editing the draft's `review`.
- **What an import creates.** For a new component: the component, a block symbol and a package and binding per variant (the bindings have no footprints yet). The EXM-48 scenario publishes the logical component and its symbol only.
- **Firmware names.** A net's name is used only when someone named it; every placed terminal sits on a net, and an unnamed `N<id>` net leaves the pin name. Templates are Rhai scripts over the IR (no MiniJinja).
- **Pin data at publication.** `declaration_problems` is wired into the component's publish findings with the five codes above.
- **Fixtures.** The automation scenario's EXM-48 (one package) and the core import tests' EXL-48 (two packages) are separate invented devices.
- **Scenario staging.** `mcp::scenario::STUBS` lists only `template.unsupported` and `model.unsupported`; `REQUIRE_COMPLETE` is set when B1 and D1 are integrated.

## C0 choices the slices inherit

- **Where the C0 call sites are.** The call sites of all three tracks are in `part_editor.rs`: the gallery and chooser sheets, the identity bar, the start page, and the Pins tab gated by `pins_tab::AVAILABLE`. `model_card.rs` calls the import wizard and the Marking section. `instance_card.rs` calls `pin_mux::entry` and `marking_row::row`. `browser.rs` calls `browser_catalog::Catalog::show`. `model_body::form_rows` dispatches later families to `family_rows`. The template picker lists only `TemplateKind::available()` kinds.
- **Recreated per frame.** `browser_catalog::Catalog` is built fresh each frame (A2 decides where its state lives). The pin-mux panel keeps its state in egui memory, so `SchematicEditor` gains no field.
- **The 3D tab's family list.** It maps later families to `Family::Later`, which the list does not offer. D3's model card and B4's gallery become the places where people pick a family.
- **What C0 implements for real.** It implements only what is shared and small: the bounds checks of the new data, marking precedence (`marking::effective`), the bundle rewrite of marking field references and library-model references, the `library.pinout_schema` request, the new clean-room gate (`.step`, `.stp`, `.glb`, `.gltf` and `.wrl` are refused unless listed in `ORIGINAL_MODELS`), and the layer rule that `pcbcraft-firmware` sits at layer 1, depends only on core and makes no file effects.
