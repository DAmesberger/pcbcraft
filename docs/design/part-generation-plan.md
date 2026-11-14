# Part generation: one mechanism per job

Plan, 14 November 2026, against `main` at f18c0658. Read-only work: nothing was built, run or changed. Line counts are `wc -l` on `main`. Hours are agent hours and are estimates. "GP2 worktree" means the uncommitted state of `.claude/worktrees/agent-a4ad090547fc6b909`; "regen branch" means commit 63213d8f on `worktree-agent-a50d226191f12bb6f`.

## Summary

- **Generators (WASM) make every parametric part, whole.** Symbol, package, footprint, binding, fields and the 3D model come from one module call.
- **The 3D model of every part is a `.solidcraft` part file with values**, or an imported vendor model. A generator's 3D output is `{part file hash, parameter values}`; the part files ship in the generator's bundle. There is no model script.
- **GP2 should change direction.** It is building a model script (`box`, `round` steps replayed through Solidcraft), which is a sixth mechanism and a second stored model kind. About 6–10 h of rework; most of GP2 is kept.
- **Sketch footprints stay as the hand editor**, not as a generator. The in-code sketch templates go.
- **Rhai makes no geometry.** The pad script (`library.generate_pads`) is deleted now. Rhai stays only as the expression language of pin rules, rule formulas and firmware templates.
- **In-code templates are deleted family by family behind measured parity**: about 18,700 lines of core and integration code, plus about 4,500 lines of desktop gallery and pickers that are rewritten as one generator form.
- Total: roughly 210–310 agent hours, most of it the nine remaining families (GP3, GP4), which run five agents wide.

## 1. Inventory: what exists today

| # | Mechanism | Where | Size | Reached from | Good at | Weak at |
|---|---|---|---|---|---|---|
| 1 | **Rhai pad script** `library.generate_pads` | `crates/editor-core/src/scripting/{mod,tests}.rs`; the operation in `commands/library.rs:94,1019` | 620 lines | The engine operation only (so CLI and the old MCP tool list). No desktop caller, not in the datasheet skill, no recipe stored. | Quantity arithmetic with units. | Only moves existing pads; makes no pads, terminals or anything else. A proof from 29 October that nothing built on. |
| 2 | **In-code templates** | Core: `library/templates/*` (4,888: catalog, starter rows, generic, footprint, `footprint_family`), `library/package_form*` (4,925: ten typed forms), `library/samples.rs` (2,318: presets, lands, symbols), `commands/templates.rs` (995). Integration: ten family builders in `models/{chip,gull_wing,no_lead,header,tabbed,molded,grid,tht,module,usb_c,profile,named}.rs` (4,084). Data compiled in: `templates/starter/*.json` (196 rows). | about 17,200 lines, plus `package_fit` (1,466), which reads the forms | Everything: template gallery, footprint template picker, Place palette's generic families, part editor's form panel (`apps/sparksmith/src/{template_gallery,part_editor/footprint_template_picker,part_editor/model_body}.rs`), CLI, MCP (`library.templates`, `library.new_from_template`, `library.package_from_template`, `library.footprint_from_template`), the datasheet skill (steps 4 and 5). | Covers ten families today, with tested lands and parametric 3D (patterns driven by count parameters). | Code, not data: breaks "no hard-coded library data" (four allowlist entries wait for GP5). Three overlapping parts inside it: typed-pad lands (`samples::form_lands`), sketch-building footprint templates (`footprint_family`), and model builders. No versioning beyond `family_rev`. |
| 3 | **Sketch footprints** | Core `library/footprint_source.rs`, `footprint_edit.rs` (2,277); integration `footprint/*` (6,055) | 8,300 lines | Part editor's footprint canvas; `library.edit_footprint`; MCP | Drawing a footprint by hand with constraints, named parameters and arrays. Array pads already have `GenerationKey`s mapped to stable pad ids, with keyed overrides. | Not a part generator; one footprint at a time. |
| 4 | **`.solidcraft` part files** | Integration `models/part_file.rs` (1,036); automation `session/{part_files,shipped_parts}.rs`; core `PackageModel::PartFile`, `library/mechanical.rs` (861); data `library/parts/*.solidcraft` with `*.part.json` sidecars (four parts) | 2,700 lines | Desktop part-file chooser, `layout.place_part_file`, CLI `sparksmith part check`, MCP `library_check_part_file` / `library_use_part_file` / `library_set_part_values`, the datasheet skill | Real parametric 3D authored in Solidcraft, markings on named planes, datums, materials by appearance name. Measured: WROOM-3 parity 110/110 bodies at 0 mm; 45–47 ms cold, 0.27 ms cached. Solidcraft stays generic. | 3D only. Bodies carry no terminal (`part_file.rs:884` sets `terminal: None`), so leads do not highlight with their net. Values are `f64`. The sidecar's `lands` rules are a small expression language of their own. |
| 5 | **WASM generators** (ABI 1) | `crates/generator-abi` (1,194), `generator-sdk` (913), `generators` host (1,130), `generators/chip` (757), core `library/generated*` and `commands/generated.rs`, automation `session/generators.rs` (2,283 together); data `library/generators/chip.wasm` | 6,300 lines | `library.generate` (engine), `library.generate_part` and `library.generators` (session, CLI). Hidden from MCP by `NOT_TOOLS` until GP7. No desktop caller. | Data not code, sandboxed, deterministic, versioned, whole part in one call, checked on open. | One family (chip). No 3D on `main`. Pad ids follow output order. Arcs through host `f64`. Identity depends on the build host. |

Two things the inventory shows that the review did not:

- **The in-code 3D builders already build parametric Solidcraft designs.** They use named parameters and patterns whose counts are parameters (`gull_wing.rs:189-208`, `grid.rs:144`). The WROOM-3 part file was made by saving the module builder's construction. So turning a family builder into a part file is mostly "run it once and save it", then prove the saved file follows its parameters.
- **The ABI on `main` already says the model is a part file.** `crates/generator-abi/src/v1.rs:582` defines `Model::PartFile { asset, values }`, "a `.solidcraft` part file shipped with the generator revision". GP2 marked it "reserved and refused" and added `Model::Script` instead.

## 2. Jobs and the one mechanism for each

| Job | Mechanism | Not |
|---|---|---|
| A. A standard parametric package or part (chip, SOIC, QFP, QFN, SOT, DPAK, BGA, axial, radial, DIP, header, radio module) | **Generator.** One module per family makes the whole part. | In-code templates, package rows typed by `PackageForm`, sketch templates |
| B. The 3D body of any part | **Part file plus values** (`PackageModel::PartFile`), or an **imported vendor model** (`PackageModel::Library`: STEP or GLB with its licence recorded). | `PackageModel::Generated` (in-code builders), a model script, `PackageModel::Envelope` |
| C. A complex or irregular part from a datasheet (assistant or person) | First a generator with the datasheet's values. Where none fits: the **ordinary authoring commands** (pinout import, pad table, symbol edits, binding) with a part file for the body. That is authoring, not a generation mechanism. | A per-part Rust form (`UsbCForm`) |
| D. A mechanical part (nut, spacer, heatsink, bracket) | **Generator** for the shipped ones: the part file is its model and the generator computes the lands. A person's own `.solidcraft` file placed directly keeps `layout.place_part_file` with explicit lands. | The sidecar `lands` rules |
| E. A footprint drawn by hand | **Sketch footprint** (the editor). | — |
| F. A quick pad array | The **sketch array** by hand; the **pad table** from automation and the assistant. | Rhai `library.generate_pads` |
| G. Customising a generated part | **Draft it** (`library.draft_from`), which makes it the person's own part, then job C's commands. Changing a parameter is regenerating (regen branch, G9). | Overrides layered on generated objects (see open decision 4) |
| H. A new family from a third party or a user | **A generator bundle** in a library folder. | — |
| I. One specific shipped part that is not a family (a USB-C receptacle) | **Library data**: an authored part with its part file, as a bundle under `library/`. | A generator with one preset; a typed form |
| J. Formulas: pin rules, rule formulas, firmware templates | **Rhai**, expression only. | Any geometry |

### Why generators own job A

They are the only mechanism that meets every founder requirement at once: loaded as data, sandboxed, deterministic, identified and versioned, and making the whole part. The in-code templates fail the first requirement by definition. Part files alone make no symbol, pads or binding.

### The 3D decision: generators emit part-file values, not a script

**Decision.** A generator bundle is the module plus the part files it uses. A generation's model is `{asset: sha256 of a bundled part file, values}`. The host stores it as `PackageModel::PartFile`, exactly what a person choosing a part file gets today. The module contains the logic (which file, which values, where the leads are); the part file contains the geometry.

Compared with GP2's model script:

| | Part file plus values | Model script (GP2 worktree) |
|---|---|---|
| Stored model kinds after migration | Two: part file, imported model | Three: part file, imported model, script |
| Geometry vocabulary | Solidcraft's own, complete today | A Sparksmith allowlist: `box` and `round` now; sketch, extrude, revolve, sweep, pattern and cut "each a new format" as families need them. Gull-wing leads, J-leads and connector shells need most of Solidcraft's features re-specified, checked and versioned inside ABI 1. |
| Complex parts (modules, connectors, mechanical) | Same path as simple ones | Still need part files, so two paths remain |
| Markings, datums, materials | Named planes, axes and appearances, built and tested | Each needs a new script step or rule |
| Authoring | In Solidcraft, visually, or by commands | In Rust, blind |
| Board build, cache, check | One path (`part_file::evaluate`, `PartKey` cache, `part_file.stale`) | A second path (`models/script.rs`) |
| Variable topology (a lead count, a depopulated grid) | Count parameters and patterns; several files per generator, chosen by the module; see risk 1 | Loops in the module: easy |
| Lead bodies bound to terminals | Missing today; added in slice B2 | Built |
| Self-contained module | No: the bundle carries assets | Yes |

The script is better on two rows (topology in code, a self-contained module) and it has leads bound today. It loses on the rows that decide whether five mechanisms become fewer: it adds a stored kind and a private CAD language, and the founder's complex parts still need part files beside it. The review reached the same conclusion ("define the generator's 3D output as part-file parameters").

**What the generation must carry in integers**, because the core never evaluates Solidcraft: the envelope, the seating height, the lead contacts and pin 1. ABI 1 has all four already (`PackageGeometry`). The marking surfaces and bounds (`PartDerived`) come from the host's evaluation when the part is made and are rechecked at each board build, as today.

**The check that keeps model and footprint together** moves from the script to the evaluated part file: when a part is generated, the session evaluates the part file at the values and refuses unless (a) its bounds agree with the generation's envelope within 10 µm, (b) every lead contact lies under a body with a lead material, and (c) the part-file check has no blocking finding. This is GP2's G13 with the same intent.

### Does Rhai survive?

As a part-generation mechanism, no. `scripting/mod.rs` and `library.generate_pads` are deleted (slice A1). The other three uses are not part generation and stay: pin rules (`functions/rules/sandbox.rs`), rule formulas (`rules/formula.rs`) and firmware templates (`crates/firmware/src/template.rs`). `quantity.rs` is independent of Rhai and stays. The `rhai` dependency therefore remains in `sparksmith-core` and `sparksmith-firmware`. Whether those three should share one sandbox configuration is a separate, small question.

## 3. Target architecture

**One plugin ABI** (founder). There is one ABI and one kind of plugin. A generator is a plugin that declares the pure, deterministic "generate part" capability: no imports, a fresh instance per call, fuel and memory caps, canonical integer JSON, as ABI 1 has today. Everything this plan says about "the generator ABI", "ABI 1" or `crates/generator-abi` means that capability within the one plugin ABI, whatever the crate is finally called; the plan needs no second world or plugin kind.

```
library/generators/<name>/            a generator bundle (data)
    generator.json                    id, version, licence, provenance, asset hashes
    <name>.wasm                       the module
    parts/*.solidcraft                the part files it may name
library/parts/                        part files not owned by a generator (people's parts, job I bundles)

generate (session):  module(params) -> Generation {symbol, package+geometry, footprint, binding,
                                                   fields, model: {asset, values}}
                     host checks the generation; integration evaluates the part file and
                     checks envelope, lead contacts, findings
                     core expands to revisions, keyed by generation key
stored:              ordinary revisions + PackageModel::PartFile + GeneratedPart record
                     (generator id, version, params, result hash, module hash, key -> id map)
3D:                  part_file::evaluate -> board plan, as for every part file
```

**Core types after the migration**

- `PackageModel` = `PartFile` | `Library`. `Generated` and `Envelope` are removed. An envelope-only part uses a shipped one-box part file, so "a part never exists without its 3D model" needs no special case.
- `PackageForm` is removed. What it supplied comes from the generation and is stored on the package as plain data: envelope, lead contacts, pin 1 (it has `envelope_nm` already; lead contacts and pin 1 are added). `package_fit` reads those instead of `PackageForm::lead_points`.
- `PackageRow` keeps its role (a cited, searchable row) with a new payload: a generator id and parameter values with citations, instead of a typed form. The 196 starter rows become the generators' presets.
- `PartValue` becomes integer (nanometres, millidegrees, counts, a ratio as thousandths), which removes four fields from the `f64-lengths` allowlist and makes it equal to the ABI's `ParamValue`.
- `GeneratedPart` gains the key-to-id map and identity by generator id and version (slice B1).

**What gets deleted**

| Path | Lines | When |
|---|---|---|
| `crates/editor-core/src/scripting/{mod,tests}.rs`; `GeneratePads` in `commands/library.rs`; its tests in `tests/library.rs`, `tests/staged.rs`, `automation/src/{session,gate,live}/tests.rs`; `cargo xtask wasm-scripting` | about 800 | A1, now |
| `crates/generator-abi/src/v1/model.rs`, `crates/generator-sdk/src/model.rs`, `crates/solidcraft-integration/src/models/script*.rs`, `PackageModel::Script` (GP2 worktree, never merged) | about 970 | A2, before GP2 merges |
| `crates/solidcraft-integration/src/models/{chip,gull_wing,no_lead,header,tabbed,molded,grid,tht,module,usb_c,profile,named}.rs`; `build_definition`, `family_rev` in `models/mod.rs` | 4,084 + part of 584 | per family in C, the rest in D2 |
| `crates/editor-core/src/library/templates/` (`catalog.rs`, `starter.rs` with `starter/*.json`, `generic.rs`, `footprint.rs`, `footprint_family*`) | 4,888 | D2 |
| `crates/editor-core/src/library/package_form.rs` and `package_form/*` | 4,925 | D2 |
| `crates/editor-core/src/library/samples.rs` | 2,318 | D2 (the standard-field helpers at the end move to `standard_fields.rs`) |
| `crates/editor-core/src/commands/templates.rs`; the operations `library.new_from_template`, `library.package_from_template`, `library.footprint_from_template`; the query `library.templates` | 995 + | D2 |
| `apps/sparksmith/src/template_gallery.rs`, `part_editor/footprint_template_picker.rs`, the typed-form panels in `part_editor/model_body.rs` | about 4,500, rewritten as one generator form | D1 |
| `library/parts/*.part.json` `lands` rules and their reader in `session/shipped_parts.rs`; the land half of `library/mechanical.rs` | about 500 | E1 |
| `xtask/src/no_builtin_data.rs`: the nine `starter/*.json` allowlist entries and the part-file entry | — | D2 |

What stays: sketch footprints (all of it except `footprint_family`), `part_file.rs`, the generator crates, `pad_table.rs`, `pinout`, the marking engine, imported models.

## 4. Migration plan

Parity is always measured against the in-code result on `main` for the same inputs, by a test that runs both and is deleted together with the in-code side.

**Parity bars** (used by every family slice)

- *Footprint*: for every starter row of the family, pad count, numbers, terminals, kinds, shapes, layers and mask and paste rules are equal; pad position and size differ by at most 1 nm from `samples::template_lands_with`; courtyard, silkscreen and assembly outlines within 1 nm, or each difference listed in the slice's report and accepted.
- *Symbol and binding*: terminals, pin names, numbers, directions and unit assignment equal; graphics equal to `symbol_style` output or listed.
- *3D*: the `part_file_tests.rs` bar: the same body count, names, roles and materials; worst bounding-box difference at most 1 µm; relative volume difference at most 1e-6; marking surfaces within 1e-9 mm of `surface::anchors(form)`; every lead body bound to the same terminal as the builder's.
- *Cost* (targets until measured): generate at most 10 ms warm; part file cold evaluation at most 100 ms; no more than the in-code builder's time for the family's largest starter row.

### A. Stop adding, redirect (now)

| Slice | Work | Acceptance | Hours |
|---|---|---|---|
| **A1** Delete the Rhai pad script | Remove `scripting/`, the operation, its tests and the xtask command. Update `part-library-design.md` (the proof section becomes a dated note). | `cargo xtask ci` green; `rhai` still used only by pin rules, rule formulas and firmware; the MCP tool list shrinks by one. | 2–3 |
| **A2** Redirect GP2 | Keep from the worktree: `Description.version`, "a model is required", the chip parity test, the 3D picture test, the offline test, the refusal wording. Replace `Model::Script` with `Model::PartFile`: author `chip.solidcraft` by saving the chip builder's construction (as the WROOM-3 file was made); the chip module names it with values; the expansion stores `PackageModel::PartFile`; the session evaluates and checks it (section 2). `PartDerived` is left out of the load check's equality. Drop the script types. | The chip parity cases in the worktree's test pass at 0 mm against the part file (resistor and capacitor 0402–1206, ferrite bead 0603, LED 0603). A generation without a model is refused. Reopening with the generator folder removed rebuilds the same model. `chip.solidcraft` passes `sparksmith part check`. | 6–10 |
| **A3** Bundle | `library/generators/<name>/` with `generator.json`, the module and `parts/`; `ShippedLibrary::generator` reads a bundle; `xtask generators build` hashes `parts/` into the module's constants and the manifest; `generators check` verifies them. The project packs a used part file once as a Model of format `solidcraft` (existing L1). | A bundle with a missing or changed part file is refused, naming the file. Two parts of one generator share one packed part file. | 5–8 (can be part of A2) |

### B. Close ABI 1 before a second generator exists

Every generator's golden hashes change with each of these, so they come before GP3. They are the review's three ABI points plus the one gap part files have.

| Slice | Work | Acceptance | Hours |
|---|---|---|---|
| **B1** Stable generation keys | Terminals, units, pins, pads and graphics carry keys (pads and terminals already do). The expansion keeps a key-to-id map in `GeneratedPart`; regenerating (regen branch) reuses ids by key and gives new keys new ids. | Regenerating with a module that inserts one pad in the middle leaves every other pad id unchanged, and routed tracks stay attached (a test with a stand-in runtime). | 10–14 |
| **B2** Lead bodies carry terminals | `part_file::evaluate` binds each lead-material body to the terminal whose lead contact lies under it, from the package's lead contacts; stored with the derived data. Generic: no naming convention in the part file, nothing in Solidcraft. | The WROOM-3's 107 pad bodies and the chip's two terminations highlight and pick with their nets, equal to the builders. A lead contact under no lead body is a blocking finding. | 6–8 |
| **B3** Integer-only output | The module flattens arcs, or emits exact arc records the host does not approximate; `expand.rs:280-284` loses its `f64` trigonometry. `PartValue` becomes integer. | `generated/expand.rs` and the ABI contain no float. A golden case with a 45° arc gives one hash under native, wasmi and the probe. | 6–10 |
| **B4** Identity not tied to the build host | Part key = generator id and params hash (scope included); the version distinguishes revisions of that one lineage. A parameter a newer version adds must not change the key (regen branch, G9). The result hash is the integrity check; the bundle hash (manifest, module and part files) is provenance and what a person approves. The regen branch already treats "same version, other bytes" as the same part, so this finishes what it started. `modules.lock` stays as a reproducibility report. | The same source built on macOS and Linux gives the same part key and passes the load check on both. A changed result under an unchanged version is refused, naming both. | 8–12 |

B1 and B2 touch different crates and can run in parallel; B3 and B4 both touch the ABI and `generated.rs` and should run one after the other, or as one slice. A generated part that fails its check on open keeps refusing the project, as GP1 built it (founder: refusing on a mismatch is fine while there are no users); this plan changes nothing there and does not depend on it changing.

### C. The families (GP3, GP4)

One slice per family: a generator (footprint, symbol, package geometry, binding, fields, presets from the starter rows) and its part files, saved from the in-code builder and made to follow their parameters. Each ends by deleting that family's model builder and switching `library.models` to the generator's file.

| Family | Notes | Hours |
|---|---|---|
| gull-wing (SOIC, SOP, SSOP, QFP) | Two files: two-sided, four-sided. | 14–20 |
| no-lead (DFN, QFN) | Variants with and without an exposed pad: a parameter or two files. | 14–20 |
| header | Rows × columns pattern. | 8–12 |
| tabbed (SOT, DPAK) | Unequal sides; likely one file per outline class. | 14–20 |
| molded two-terminal (SOD, SMA, tantalum, MELF) | — | 10–14 |
| grid (BGA, LGA, WLCSP) | Depopulated grids are risk 1. | 14–22 |
| through-hole (axial, radial, DIP, TO) | Four outline classes, four files. | 16–24 |
| module | The WROOM-3 part file exists; the pad rows must become parametric (today they are "as drawn"). | 12–18 |
| USB-C receptacle | Job I: an authored library part with a part file, not a generator. | 6–10 |

Total 108–160 h, at most five in parallel. Acceptance for each: the parity bars over every starter row of the family, with the numbers in the slice's report; the family's builder file deleted; `cargo xtask ci` green.

### D. Switch the surfaces and delete (GP5, GP7)

| Slice | Work | Acceptance | Hours |
|---|---|---|---|
| **D1** One generator form | Catalog search, the template gallery, the Place palette's generic families and the part editor's package panel use `library.generators` and `library.generate_part`; the parameter form is built from the module's `Description` (types, ranges, units, rules, presets). `generic-families.json` names generator ids. | Every example in `generic-families.json` builds a part through a generator. The scripted part of the VM run (place a resistor, a QFN, a header from the palette) passes. No desktop code names a `PackageForm` variant. | 20–30 |
| **D2** Delete the in-code templates | The paths in section 3; `PackageForm` and `PackageModel::Generated` removed; `PackageRow`'s payload retyped; `package_fit` reads stored lead contacts; the allowlist entries removed. | `cargo xtask no-builtin-data` has no "until GP5" or part-file entry. `grep -r PackageForm crates apps` is empty. The example projects open and their boards pass `cargo xtask board-parity`. | 12–18 |
| **D3** MCP and the assistant (GP7) | `library.generators` and `library.generate_part` as MCP operations (through the five-verb surface, so no byte budget applies; `NOT_TOOLS` removed). The datasheet skill's steps 4 and 5 become: find a generator, give the drawing's values, review; otherwise author by commands. Remove `library.templates`, `library.package_from_template` from the skill and the prompt. | The recorded WROOM-3 and TLV757 assistant runs replay to parts equal to today's under the parity bars. | 8–12 |

### E. Loose ends

| Slice | Work | Hours |
|---|---|---|
| **E1** Mechanical parts as a generator | A `mechanical` generator for the nut, spacer and heatsink, with lands computed in the module; the sidecar `lands` rules removed. `layout.place_part_file` with a person's own file and explicit lands is unchanged. | 8–12 |
| **E2** Envelope as a part file | A shipped one-box part file replaces `PackageModel::Envelope`. | 3–5 |
| **E3** Documents | `component-model.md`, `part-library-design.md`, `parts-library.md`, `part-templates.md` sections on forms, rows and builders are dated as superseded; `generator-plugins.md` and `generators/README.md` describe the bundle; GLOSSARY entries for *generator*, *generator bundle*, *part file*, *template* (now only a firmware and marking word). | 4–6 |

### Sequence with the other work

1. **Now, in parallel:** A1; A2 with A3 (the GP2 agent); merge the regen branch (it is compatible: it needs only `Description.version`, which A2 keeps).
2. **Then B1–B4** (two or three agents), after the file-format plan's slice F0 (file version 2, a bump on every break, the schema-fingerprint gate; 4–6 h), since every B slice raises the file version. The format plan's canonical revision hash (its F7) is fixed only after B1 and B3, because keyed ids and integer `PartValue` change every generated part's stored bytes. The "generate part" capability is declared closed for the families at the end of B.
3. **File-format work** (the other planner) fits best **between B and C**: B settles the shape of `GeneratedPart` and `PartValue`, and C–D remove `PackageForm`, `PackageModel::Generated` and the typed `PackageRow` payload. If the format work must start earlier, it should treat a definition body as opaque and not design per-variant files for the types in section 3's delete list. Notes for that planner: a generator bundle is a folder of a JSON manifest, a binary module and JSON part files (`.solidcraft` is JSON and diffs already); a project stores the module and each part file once by hash; every slice in B and D2 is an incompatible file change and should bump the file version, as the review recommends.
4. **C** five wide, about three rounds.
5. **D1, D3** in parallel, **D2** last. **E** any time after B.

GP6 is not described in the repository; I assume it is the distribution of generators through library folders (`library.promote` carrying the record and the bundle, open question 2 of `generator-plugins.md`). It fits after D2 and is not estimated here.

| Phase | Hours |
|---|---|
| A | 13–21 |
| B | 30–44 |
| C | 108–160 |
| D | 40–60 |
| E | 15–23 |
| **Total** | **206–308** |

## 5. Risks and decisions for the founder

Each with my recommendation.

1. **GP2's direction. Recommendation: change it now (slice A2).** It is uncommitted, the rework is 6–10 h, and every family built on the script would raise the cost of leaving it. What is lost: the module no longer carries its whole output in itself, and variable topology moves from loops in the module to parameters in the part file.

2. **Variable topology in part files (the main technical risk).** Counts are covered by pattern parameters, as the builders do today. Two cases are not: a depopulated ball grid with an arbitrary map, and packages with individual pins missing. Options: (a) the module picks among several part files and the rest is parameters, which covers pins missing by outline class; (b) a generic "pattern with skipped instances" in Solidcraft, a usual CAD feature that can go upstream (6–10 h in an isolated fork checkout, needs your go); (c) show the full grid and say so. **Recommendation: (a) everywhere, (b) for grids when the grid family is built; never (c) silently.** I did not confirm what the pinned fork's patterns can skip; `pattern_suppress_tests.rs` covers a suppressed seed, not single instances.

3. **Is a single part a generator?** The USB-C receptacle and the WROOM-3 are parts, not families. **Recommendation: ship them as library data (job I)**; keep `module` as a generator because the assistant uses it with a drawing's values for any radio module.

4. **Overrides on generated objects.** `part-library-design.md` (CE-03) specifies manual overrides that survive regeneration. GP1 chose the opposite: a generated part is exactly what its module makes, and drafting it makes it yours. **Recommendation: keep GP1's rule and drop the override layer from the specification.** Stable keys (B1) still protect routed boards through regeneration; overrides would add a merge step to every regeneration for a need that "change the parameter" or "draft it" already meets. This closes CE-03 differently from how it was written, so it needs your yes.

5. **Sketch footprint templates go.** After D2 a generated footprint is plain pads, and "edit a parameter" means regenerating, not re-solving a sketch. A person who wants a constraint-driven footprint draws one. **Recommendation: accept.** The alternative (generators emitting sketches) keeps a second footprint output and puts the Solidcraft sketch format inside the ABI.

6. **How the assistant makes a body no shipped part file covers.** Today it picks a shipped file. Options: more generic shipped files (box with leads, can on board); or letting the assistant author a `.solidcraft` file through Solidcraft's own command surface, bounded, producing an asset once. **Recommendation: generic shipped files now; the authoring route after D, as its own decision.** It would be an authoring tool, not a stored format, so it would not be a sixth mechanism.

7. **Third-party generator authors need Solidcraft** to make part files. **Recommendation: accept**; it is the founder's stated direction for part models, and simple generators can reuse a small set of generic part files copied into their bundle.

8. **Footprint parity will not be bit-exact.** The in-code lands use `f64`; modules use integers. **Recommendation: the 1 nm bar above**, with each larger difference listed and decided per family rather than tuned away.

9. **Do not announce the plugin ABI before the end of B**, as the review says: keys, identity and integer output all change what a part key means.

10. **Agent pacing.** Phase C wants five agents at once for three rounds; that is the stated limit, with nothing else running against the library code.
