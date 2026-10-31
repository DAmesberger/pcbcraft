# S2 contract: manufacturer parts

Working contract for slice S2 of the [parts library](parts-library.md), 31 October 2026. It fixes the types and interfaces so that three tasks can proceed in parallel. Each body is either the simplest correct one or returns `Error::Unsupported` with "(S2 task 1)" in the message. Nothing here is a `todo!` or an `unimplemented!`. Delete this file when S2 is documented as built in `parts-library.md`.

Decisions in force: D1 (remap on import, with the origin recorded), D3 (offers belong to the Part revision), D5 (generic and exact parts), D9 (no supplier data; the user chooses manufacturer, MPN, distributor, SKU and link), D11 (alternates deferred) and D13 (labels outside revisions). Fixtures are original: `ExampleCo` and `EX-…` MPNs.

## Types (as committed)

### `ids`

- `DefinitionKind::Part` is the last variant. `PartRef { id, revision }` comes from `exact_ref!`, with `PartRef::erased()`.

### `fields` (`fields/spec.rs`, re-exported from `fields`)

- `SpecMeaning`: `ExactNominal`, `MinimumRating` or `MaximumTolerance` (serialised in snake_case).
- `RatedSpec { field: FieldRef, value: FieldValue, meaning: SpecMeaning }`. A spec matches a requirement by field lineage.
- `Requirement { field: FieldRef, label, value: FieldValue, value_type: ValueType, nominal: Option<f64> }` is plain data. `nominal` exists only for tolerances of different bases, through `tolerance_interval`.
- `pub fn satisfies(requirement: &Requirement, spec: &RatedSpec) -> Vec<Finding>`. An empty result means the spec meets the requirement. The findings are advisory and use three codes:
  - `part.spec_mismatch`: the comparison fails;
  - `part.spec_field_mismatch`: the spec belongs to another field lineage;
  - `part.spec_uncomparable`: the meaning does not apply to the value, or the comparison needs data that is absent.
- What `satisfies` already compares:
  - `ExactNominal` on quantities (relative 1e-9), integers, booleans, enums and text;
  - `MinimumRating` on quantities and integers;
  - `MaximumTolerance` on tolerances, each side, with the basis shared by one value type.
- Constants: `MAX_RATED_SPECS = 64` and `QUANTITY_EPSILON = 1e-9`.

### `library` (`library/part.rs`, re-exported)

- `PartDefinition { next_local, manufacturer, mpn, component: ComponentRef, binding: BindingRef, specs: Vec<RatedSpec>, references: Vec<Reference>, offers: Vec<DistributorOffer>, provenance: Provenance }`. `Default` sets `next_local: 1`.
- `Reference { id: u32, kind: ReferenceKind, title, url: Option, document_revision: Option, asset: Option<sha256 hex>, licence: Option, provenance }`. `ReferenceKind` is `Datasheet`, `ApplicationNote` or `Other`. A link is not a copy; `asset` names the attached copy that CE-07 keeps.
- `DistributorOffer { id: u32, distributor, sku, url: Option, packaging: Option }`, with no price or stock.
- `Provenance { origin: Option<Origin>, basis: Basis, licence: Option }`.
  - `Origin { library: String, lineage, revision, content_hash }` follows D1. `library` is text until S3 defines `LibraryId`.
  - `Basis` is `Authored` (the default), `FromReference(local id)` or `Imported`.
  - The free-text note stays in `DefinitionMeta::provenance` for every kind. Moving the other kinds to structured `Provenance` is out of S2's contract scope.
- Bounds: `MAX_REFERENCES = 32`, `MAX_OFFERS = 32` and `MAX_URL = 2048`. The other text fields are bounded by `MAX_LABEL`, and hashes by 64 bytes.
- `Definition::Part(Box<PartDefinition>)`, boxed because the body is several times larger than the other kinds and Clippy's `large_enum_variant` rule rejects it inline. Serde does not see the box. Alongside it, `Library.parts: Vec<Published<PartDefinition>>`. The field is `serde(default)` and is skipped when empty, so existing files read unchanged; there is still one format version and no migration.
- `Library::part(PartRef)` and `Library::latest_parts()`.
- `publish_rank(Part) = 7`, after Component. One `library.publish` set therefore publishes the cascade footprint → binding → component → Part atomically.
- Reference and offer IDs share `next_local`, through `local_ids`, under the usual retire rules.
- Publish findings in the contract:
  - `part.identity`: the manufacturer or MPN is not printable;
  - `reference.missing`: the pinned component or binding is absent.

### `model`

- `ComponentInstance.part: Option<PartRef>`, `serde(default)`, skipped when `None`.

### `instances` (`instances/part.rs`, re-exported)

- Invariant (in `check_with`, so commands and decode share it): a pinned `PartRef` resolves in the project library. Disagreement with the instance is a **finding**, never an invariant, because an update may leave a Part behind, which must surface rather than be rejected or re-pointed.
- `requirements(&Library, &ComponentInstance) -> Result<Vec<Requirement>>` is a stub that returns `Unsupported`.
- `part_findings(&Library, &ComponentInstance) -> Vec<Finding>` is a stub that returns empty. Codes it must produce:
  - `part.component_mismatch`;
  - `part.binding_mismatch`;
  - `part.stale_component` (the Part pins an older component revision than the instance);
  - every `satisfies` code.
- `assign_part(&mut Project, &[Id], Option<PartRef>) -> Result<()>` applies to 1–`MAX_ASSIGN_INSTANCES` (256) instances. `None` clears the Part (implemented). `Some` returns `Unsupported` for now.

### `commands` / engine

- `InstanceCommand::AssignPart { instances: Vec<Id>, part: Option<PartRef> }` is serialised as `instance.assign_part`. It is atomic: when one instance disagrees, nothing changes. One Resolve-parts group is one command, so one undo step.
- `Query::Bom {}` (`project.bom`) returns `QueryResult::Bom(BomReport)`.
- `Query::ResolveGroups {}` (`library.resolve_groups`) returns `QueryResult::ResolveGroups(ResolveReport)`.
- CLI `stdio` and MCP reach both queries through `project.query`, and the command through `project.apply`. Their schemas are generated from these types.

### `completeness` (`completeness/purchasing.rs`, re-exported)

- `PurchasingState` is `NoPart`, `Unresolved` or `Resolved`.
- `PurchasingReport { state, part, manufacturer, mpn, findings }` replaces the old `InstanceReport.purchasing: String`. Its findings are advisory and are **not** copied into `InstanceReport.findings`, so existing finding lists stay unchanged.
- `purchasing(&Project, &ComponentInstance) -> PurchasingReport` returns `NoPart` without a Part. With a Part, it adds a temporary `part.unchecked` finding until task 1 lands, so nothing claims `Resolved` early.
- `BomLine { component, footprint, part, manufacturer, mpn, values: Vec<BomValue>, designators, instances, quantity, unresolved, state }` and `BomReport { lines, unresolved }`. `BomValue` is `{ field, label, text }`.
- `ResolveGroup { component, physical, footprint, requirements: Vec<Requirement>, instances, designators, query }`. `query` is the `library.search` text (`kind:part` plus the requirements as field predicates) that pre-fills Choose….
- `ResolveReport { groups }`.
- `bom(&Project)` and `resolve_groups(&Project)` are stubs that return `Unsupported`.

## Three parallel tasks

The tasks share only the types above. Agree a change to a contract type before making it, and keep it additive. Concurrent agents own `library/footprint.rs`, `layout.rs`, `pcb_editor.rs`, the Footprint parts of `part_editor.rs`, `crates/solidcraft-integration`, `schematic.rs`, the `SymbolOccurrence` model and `schematic_editor.rs`. Touch those files only with small additive hooks.

### Task 1: core rules

**Owns:**

- `crates/editor-core/src/fields/spec.rs`
- `library/part.rs`
- `instances/part.rs`
- `completeness/purchasing.rs`
- `catalog.rs` and `catalog/*`, including `synthetic.rs`
- `updates.rs` and `updates/*`
- new `tests/parts.rs`, with a `mod` line in `tests.rs`
- `library/publish.rs`, for Part-specific hooks only

**Delivers:**

- `satisfies` completed:
  - comparing tolerances across bases through `tolerance_interval` and `nominal`;
  - `MinimumRating` on ranges, if the field kinds call for it.
- Part publish rules:
  - the binding is listed by the pinned component revision;
  - each spec's field is selected by that component and its value checks against the field (`fields::check_value`);
  - one spec per field lineage;
  - a meaning that fits the value type;
  - reference and offer text and URLs bounded.
- `requirements`, `part_findings` and `assign_part` with `Some`:
  - the same component revision;
  - the binding equals `physical.binding`;
  - every requirement that has a matching spec satisfies it;
  - otherwise `Error::Conflict` naming the field.
- Part consistency on component updates (`updates`): an instance whose component moves to rN while its Part pins r(N−1) gets `part.stale_component` in the update preview and in `purchasing`. The Part is never re-pointed silently, and the update choice is "keep the Part (finding)" or "clear the Part".
- `purchasing` with `part.unchecked` removed.
- `bom` and `resolve_groups`, grouping as described above.
- The catalog indexes Parts:
  - `CatalogEntry.kind = Part`, with the manufacturer, MPN and spec values as entry fields;
  - `badges.mpn` true on components that have a Part;
  - `kind:part` / `kind:component` filters;
  - `mpn:` as text matched on the MPN only;
  - `CatalogEntry::component()` stays component-only.
- Synthetic Parts for `ExampleCo`, such as `EX-R0603-10K0-F`, `EX-R0603-22K0-F` and `EX-C0402-100N-25V`.
- Tests:
  - **CE-01**: 10 kΩ → 22 kΩ after an MPN is chosen gives `part.spec_mismatch`, and the Part is unchanged;
  - **CE-02**: a 16 V requirement against a 25 V part satisfies, and the reverse is a finding;
  - **CE-07**: a Reference without a URL keeps its identity and `asset`, and round-trips through save and reopen;
  - the cascade publishes in one set;
  - `assign_part` is atomic over a group.

### Task 2: desktop UI

**Owns:**

- new `apps/pcbcraft/src/part_editor/part_props.rs`, the Parts section of the Component workspace, with an additive hook in `part_editor.rs`;
- `apps/pcbcraft/src/place_palette.rs`;
- new `apps/pcbcraft/src/resolve_parts.rs`, the Resolve parts card;
- additive hooks in `schematic_editor.rs`, for the properties row and the card entry point;
- `app/chrome.rs` or the ribbon, if a button is needed.

**Delivers:**

- **Part authoring** through the kit (`kit/form.rs` controls), in a Parts section of the Component workspace:
  - manufacturer and MPN;
  - rated specs as typed rows: field, value through `fields::parse_value`, and meaning;
  - references and offers;
  - drafts published through `library.publish` together with the cascade.
- **Assign** in Schematic properties: a Part row with "Choose…" opens the Place palette in *assign mode*, pre-filled with `kind:part` and the instance's requirements (the group's `query`, or the same text built for one instance). Enter applies `instance.assign_part` and the row offers Clear. The palette's MPN badge shows real data once task 1 lands.
- **Resolve parts**: a card that lists the `library.resolve_groups` groups (designators, values, footprint and count) with Choose…, which assigns one Part to the whole group with one command and one undo step. The card has a states-and-errors view while the query is still `Unsupported`.
- Desktop tests drive the authoring, the assignment and the group assignment through the engine.
- GUI runs follow the task rules: `PCBCRAFT_RECOVERY_DIR=…/recovery-s2`, a `--profile ci` build, and processes killed by their own PID.

### Task 3: automation and docs

**Owns:**

- `crates/automation/src/mcp.rs`, for the `INSTRUCTIONS` text naming `instance.assign_part`, `project.bom` and `library.resolve_groups`;
- `crates/automation/src/session.rs`, only if needed;
- new `crates/automation/src/session/parts_tests.rs`, or additions to `session/tests.rs`;
- `apps/pcbcraft-cli` help text, if any;
- `docs/parts-library.md` (an "S2 as built" section and the slice row);
- `docs/component-model.md` (the module map, if the edges change, and the command rows for `instance.assign_part`, `project.bom` and `library.resolve_groups`);
- `GLOSSARY.md` (Part, rated spec, spec meaning, reference, distributor offer, purchasing completeness, Resolve parts);
- `docs/part-library-design.md` (CE-01, CE-02 and CE-07 status);
- `docs/editor-evidence.md`.

**Delivers:**

- An automation test over JSON. It publishes a Part fixture through `library.new_draft`, `library.edit` and `library.publish`, then runs `library.resolve_groups`, then `instance.assign_part` through `project.apply`, then `component.inspect` with `purchasing.state` equal to `resolved`, then `project.bom` with no unresolved lines, and compares the results with direct engine queries.
- A check that schemas list the new command and queries.
- Until task 1 merges, the test asserts the `unsupported` error code. Switch it to the real assertions at merge time and do not weaken it afterwards.

**Watch out:** `session::tests::pad_generator_is_available_through_the_shared_transport_contract` asserts that the generated transport catalog contains no `part.`, which guards against the retired `part.*` command names. Doc comments on schema types must therefore not contain `part.`; for example, write "the Part, without…", not "a part. No…". Finding codes such as `part.spec_mismatch` may appear in non-schema docs only. Keep the guard and, if needed, narrow it to command names in task 3 without weakening it.

**Merge order:** task 1 first, then tasks 2 and 3 rebase on it. Tasks 2 and 3 can be built and reviewed against the stubs.
