# Persistent component model

Implementation contract, 30 October 2026. This document says how the [accepted authoring contract](part-library-design.md#accepted-authoring-contract) is represented in the document, the engine and its consumers. It covers exactly three slices:

1. **Definitions, revisions, instances and units.** This slice replaces the monolithic `Part`, the `part.*` commands and `Instance.position`.
2. **Typed fields and quantities.** This slice replaces `Instance.value: String` and the scripting proof's `UnitDefinition`.
3. **Terminal functions and selections.** This slice replaces `PinKind`.

**Status (30 October 2026):** slices 1, 2 and 3 are implemented headlessly as described here, with the recorded deviations for [slice 1](#slice-1-implementation-notes), [slice 2](#slice-2-implementation-notes) and [slice 3](#slice-3-implementation-notes). The desktop keeps one fixed function per terminal; the pin-configuration table is later CE UI work. The slices add no feature scope beyond the contract. When something is deferred, this document says so and names the scenario that owns it.

All fixtures are original synthetic data. Behavior comes from this project's specification, general electronics conventions and public specifications. No GPL/LGPL/AGPL EDA implementation, library or proprietary tool data is consulted (see [attribution](../ATTRIBUTION.md)).

## Ground rules for all three slices

- **One model, replaced in place.** Each slice deletes the type it supersedes and updates every consumer in the same change. `Part`/`PartRef`/`PartCommand` go in slice 1; `Instance.value` and `UnitDefinition` in slice 2; `PinKind` in slice 3. Old and new models never coexist. The `.pcbcraft` format keeps `FILE_VERSION = 1`, with no migrations or legacy readers. Files written before a slice fail to decode as malformed (`deny_unknown_fields`) and are never partially read.
- **One engine.** Every mutation is an `Operation` applied by `Engine::apply` at an expected revision. Every read-only report is a `Query` answered by `Engine::query`. A failure leaves document, revision and undo/redo history unchanged. UI, CLI and MCP send the same operations and queries. Programmatic calls never open dialogs.
- **Core stays pure.** `pcbcraft-core` gains no filesystem, UI, transport, Solidcraft or randomness dependency. Rhai stays inside `scripting`. Its values never appear in persisted or transported types.
- **Each rule has one owner.** Every domain rule is one named function in one module ([module map](#d-module-map)). Command handlers, the update planner and document validation call that function; none restates it.
- **No category branches.** No code tests for "resistor", "transistor", "MCU" or similar. Every behavior is driven by declared data: terminals, units, fields and functions.
- **Bounded input.** Every new collection and string has an explicit limit, checked before allocation-heavy work (see [limits](#limits)). Production code has no `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`unsafe`.

## (a) What is pinned, by whom

### Identity

| Identity | Type | Allocated by | Scope |
| --- | --- | --- | --- |
| Definition lineage | `DefinitionId(Id)` | `Project::allocate` when a new draft is created | Project. One lineage has exactly one kind. |
| Revision | `Revision(u32)`, ≥ 1 | `library::publish` (base + 1, or 1) | Lineage |
| Exact reference | `ComponentRef`, `SymbolRef`, `PackageRef`, `FootprintRef`, `BindingRef` (slice 1); `QuantityKindRef`, `FieldRef` (slice 2), each `{ id: DefinitionId, revision: Revision }` | — | — |
| Local IDs | Component: `TerminalId` (slice 1), `FunctionId`, `ResourceId`, `RoleId` (slice 3). Symbol: `UnitId`. Package: `PackageTerminalId`. Footprint: `PadId`. Field: `ChoiceId` (slice 2). All `u32` newtypes. | The owning definition's single `next_local` counter, shared by all its local ID kinds | Definition lineage, stable across revisions. Unique across all kinds within one definition. |
| Project objects | `Id` (instances, symbol occurrences, wires, tracks) | `Project::allocate` | Project |
| Nets | `Id` | `Project::allocate_net` (unchanged) | Project |

`DefinitionId` is an opaque newtype, which provides the library-folder seam. Shared local library folders will need collision-free identities across projects. That work can then change the representation behind the newtype without changing any consumer. Collision-free identity is not part of these slices (see [open questions](#open-questions-and-deferrals)).

`FunctionId` is unique per component lineage, not per terminal. A `FunctionKey { terminal, function }` is therefore redundant by design: publication and selection both validate that the function belongs to the named terminal, so a key can never pair a terminal with another terminal's function.

**Local IDs never reuse a retired value.** Every definition with local IDs carries `next_local: u32`. Publishing a revision fails when:

- Any local ID in the revision, of any kind, is not below that revision's own `next_local`.
- An ID absent from the base revision is below the base's `next_local`. That would be a deleted ID reused with a possibly different meaning, which [the spec forbids disguising](part-library-design.md#updates-must-explain-what-changes).
- `next_local` is lower than the base's.

The first rule closes the gap the other two leave: without it, r2 could publish ID 10 with `next_local` 5, r3 could remove 10, and r4 could re-add 10 with a new meaning while passing the base check.

The illustrative JSON in the spec uses string IDs (`"io-a"`, `"gpio-in"`). These slices use typed numeric local IDs with separate labels, consistent with existing `Endpoint`, net and pad identities. Labels are display text and never identity.

### Reference graph (acyclic)

```text
ComponentInstance ──pins──▶ ComponentRef ──pins──▶ SymbolRef*        (allowed representations)
      │                          │        ├─pins──▶ BindingRef*       (allowed implementations)
      │                          │        └─pins──▶ FieldRef*         (slice 2, selected fields)
      │ chooses (lineage IDs,    │
      │ resolved through the     BindingRef ──pins──▶ PackageRef
      │ pinned component)        │          ├─pins──▶ FootprintRef*   (allowed footprint variants)
      │                          │          └─owner─▶ component DefinitionId (lineage, not revision)
      ▼                          FootprintRef ──pins──▶ PackageRef (must equal the binding's)
  representation: symbol         SymbolRef ──owner──▶ component DefinitionId (lineage)
  physical: { binding, footprint } FieldRef ──pins──▶ QuantityKindRef (slice 2, quantity/tolerance/range types)
```

| Pinner | Pins exactly | Records lineage only | Never pins |
| --- | --- | --- | --- |
| Component revision | Allowed representations (`Vec<SymbolRef>`, at most one per symbol lineage); allowed implementations (`Vec<BindingRef>`, at most one per binding lineage); slice 2: selected fields (`Vec<FieldSelection { field: FieldRef, … }>`, at most one per field lineage) | — | Packages or footprints directly |
| Symbol revision | — | Owning component lineage, so its anchor terminal IDs are interpreted in that component's namespace | Component revision |
| Binding revision | `package: PackageRef`; allowed footprints `Vec<FootprintRef>`, at most one per footprint lineage, each pinning the same `PackageRef` | Owning component lineage. A binding can be listed only by revisions of that component, so a package shared by two components never shares bindings. | Component revision |
| Footprint revision | `package: PackageRef` | — | — |
| Package revision | — (native model assets are deferred; the envelope stays inline) | — | — |
| Field revision (slice 2) | `QuantityKindRef` where its value type needs one | — | — |
| Component instance | `component: ComponentRef` | `representation: DefinitionId` (a symbol listed by the pinned component); `physical: Option<PhysicalChoice { binding: DefinitionId, footprint: DefinitionId }>` (a binding listed by the pinned component and a footprint listed by that binding) | Anything not reachable through its component revision |
| Symbol occurrence | `instance: Id`, `unit: UnitId` of the instance's resolved representation | — | Its own electrical terminals |

The instance stores one exact pin, its component revision. Its choices are lineage IDs. `instances::resolve(&Library, &ComponentInstance)` turns them into exact revisions by walking the graph above. The per-lineage uniqueness rules in the table are publish rules, so each lineage choice has at most one candidate. There is exactly one source of truth for which exact symbol, binding, footprint and package an instance uses, so no redundant exact references can disagree.

Symbols and bindings are owned by one component lineage, so a symbol is not reused across components in these slices. This narrows the spec's "reuse an existing symbol where suitable" deliberately (see [open questions](#open-questions-and-deferrals)).

**Spec ambiguity resolved: direction of the binding pin.** The domain table says a binding names "exact component/package revisions", but a component must also pin which bindings it allows. Pinning in both directions would form a cycle. These slices choose this direction:

- A component revision pins its allowed bindings.
- A binding pins its package and allowed footprints, and records only its component lineage.

This makes "which physical implementations this component revision allows" a property of the pinned component revision. It also gives staged completion for free: a revision with no implementations is schematic-usable and not board-placeable. A binding still cannot float between components, because it is owned by one lineage. The component publication check also requires the mapping to cover that revision's terminals exactly.

The cost is a **revision cascade**. A new footprint revision is adopted through a new binding revision, which is adopted through a new component revision. One `library.publish` set publishes the cascade atomically. Instances then adopt it through one reviewed component-revision update. CE-01 ("publish a new footprint revision and update only R1") becomes: publish {footprint r2, binding r2, component r2}, then update R1 to component r2. R2 keeps component r1 and every choice that it pins.

### Project library snapshot

`Project.library: Library` holds every published revision of every kind: `components`, `symbols`, `packages`, `footprints`, `bindings` and, from slice 2, `quantity_kinds` and `fields`. Each is a `Vec<Published<T>>` with `Published<T> { id, revision, meta: DefinitionMeta, definition: T }`. `DefinitionMeta { name, description, provenance }` is the envelope common to every kind and is owned by `library`; definition bodies carry no metadata of their own.

Revisions are immutable. Publishing appends and never rewrites. The snapshot is the transitive closure by construction: document validation requires every exact reference in every published revision, and every instance pin, to resolve to a stored revision of the correct kind. A project therefore reopens offline with no other source. Unreferenced old revisions stay in the snapshot. A `library.prune` command is proposed later, not part of these slices.

### What a library update changes

- `library.publish` changes only the library. Instances, occurrences, wires, nets, placements and tracks are untouched, and existing instances keep their pins.
- An instance changes revision only through `project.preview_update` / `project.apply_update` with `Change::ComponentRevision { instances, target: ComponentRef, resolutions }`. The target must have the same component lineage. The plan diffs everything reachable from the old and new pins:
  - Units and anchors (slice 1).
  - Binding mapping, footprint pads, datum and package envelope (slice 1, as today's part-revision plan).
  - Field schemas, defaults and effective values (slice 2).
  - Terminal functions and selections (slice 3).
- A plan is **blocked** when an instance's current choice is not offered by the target, until `resolutions` supplies an explicit replacement. Examples: its representation, binding or footprint lineage is no longer listed; an occurrence's unit no longer exists in the same symbol lineage; an override no longer fits; a selected function is gone. Nothing is inferred from names, order or pad count.
- **Unit IDs are lineage-local.** Unit 2 of symbol X and unit 2 of symbol Y are unrelated. When a `ChoiceResolution` changes the representation lineage, the plan is blocked until every occurrence and every `unused_units` entry of that instance has an explicit `UnitResolution`. A unit is never kept by numeric match across lineages.
- Within one symbol lineage, a unit ID keeps its meaning. A target revision that removes a unit with an occurrence blocks until a `UnitResolution` is given. A target that removes a unit listed in `unused_units` lists that flag's removal in the plan; it does not block, because no presentation or connectivity is lost.
- **Regrouping never disconnects silently.** When a terminal's anchor moves to another unit and the terminal is wired, the plan is blocked unless the target unit is placed. A `RegroupResolution` either places the target unit at a given position or explicitly accepts disconnecting the listed wires. There is no default.
- Cancel means not applying, so the old pins remain. Apply is one undo step. A stale plan fails without changes, as today.

## (b) Incomplete drafts and completeness

### One type, optional parts

Drafts and published revisions use **the same definition types**. A draft may have:

- Empty or partial collections: no terminals, no units, a binding without mappings.
- Empty names.
- Duplicate local IDs.
- References to revisions that do not exist yet.

Only persistence bounds and finite geometry are enforced on a draft (`library::check_bounds`). A NaN or out-of-range coordinate is rejected even in a draft, because it cannot be persisted meaningfully. A structurally contradictory but finite draft is kept as visible draft input.

`Document` becomes `{ project: Arc<Project>, drafts: Arc<Drafts> }`. `Drafts { items: Vec<Draft> }` stores `Draft { id: DefinitionId, base: Option<Revision>, meta: DefinitionMeta, definition: Definition }`. There is at most one draft per lineage. `Definition` is the kind-tagged enum over the definition bodies. A draft's pending revision is `base + 1`, or `1` when it has no base.

Drafts stay outside `Project`, as `authoring_draft` does today. Draft edits therefore neither revalidate the project nor invalidate the board scene key. Undo history covers both halves.

Creating a new lineage draft is the one draft operation that touches the project: it reserves a `DefinitionId` from `Project::allocate`. That keeps a single identity allocator and lets drafts reference each other's pending revisions as ordinary exact references before anything is published.

### Persistence and recovery

`EditorFile { version: 1, project: Project, drafts: Drafts }` replaces `authoring_draft: Option<Part>`. Saving and recovery are unchanged in `pcbcraft-io`, which serializes `EditorFile` atomically and in bounded form. Drafts therefore survive save, crash recovery and offline reopen exactly as written, including unfinished content.

Decode validates the project fully. For drafts it checks only bounds, kind and identity consistency (lineage allocated, kind consistent with any published revisions of the same lineage, base revision exists).

### Derived completeness, no stored stamp

No definition or instance stores a "valid", "verified" or "complete" flag. Completeness is computed on demand from findings. `Finding` gains `blocks: Option<Stage>` and the predicate `Finding::blocks_stage(Stage) -> bool`:

```rust
pub enum Stage { Publish, BoardPlaceable, ElectricallyResolved }
```

A state holds when no current finding blocks it. Purchasing resolution is not computed in these slices: there is no manufacturer-part model yet, and reports say so explicitly. Manufacturing readiness is out of scope. These are the states from the spec:

| Spec state | Derived as | Checked by |
| --- | --- | --- |
| Recoverable draft | Any draft passing `check_bounds` | `library::check_bounds` |
| Schematic-usable definition | A published component revision. Publishing it requires no `Stage::Publish` findings for that component and its representations. | `library::publish_findings` |
| Board-placeable instance | No `BoardPlaceable` findings: physical choice present and resolvable, binding/footprint complete for the pinned component | `completeness::instance` |
| Electrically resolved instance | No `ElectricallyResolved` findings: every unit placed or marked unused; every electrical terminal accounted for (`nets::accounted_for`); no required power input marked no-connect; (slice 2) required fields supplied; (slice 3) every function resolved and resource requirements satisfied | `completeness::instance` |

Function availability in the chosen binding is **not** a finding. It is an invariant (`instances::check_instance`): a state that violates it is rejected by direct commands and blocks plans, so it can never be observed. ERC findings (`erc.*`) and board findings (`board.*`) have `blocks: None`. They describe circuit and board state, not component completeness, as the spec requires.

### What `library.publish` validates

`library.publish { drafts }` takes 1–32 draft IDs. It publishes them as one transaction in dependency order: quantity kinds, then fields, packages, footprints, bindings, symbols and finally components. A reference may point to an existing revision or to another draft's pending revision in the same set. The command fails with `conflict` and publishes nothing if any of these hold:

- Any finding satisfies `blocks_stage(Stage::Publish)`.
- A base is not the latest revision (stale draft).
- A pending reference cannot be satisfied.

Afterwards, each published draft becomes a draft of its new revision, ready for the next edit, as `part.publish` behaves today.

Publish rules per kind are owned by `library::publish_findings` and use domain validators from `fields` and `functions`. Document decode validates every published revision with **the same** function and rejects it on exactly the findings that satisfy `blocks_stage(Stage::Publish)`, the predicate `library.publish` uses. Advisory publish findings (`blocks: None`) never make a saved file fail to reopen. No rule is written twice.

- **Every kind with local IDs:** the [local-ID rules](#identity).
- **Component:**
  - 1–256 terminals with unique nonzero IDs and printable names.
  - ≥ 1 representation, at most one per symbol lineage. Each listed symbol has the same owner lineage and gives every terminal exactly one anchor.
  - Implementations at most one per binding lineage. Each listed binding has the same owner lineage, maps every terminal to at least one package terminal and maps no unknown terminal.
  - Slice 2: field selections are unique per field lineage, and defaults satisfy their field.
  - Slice 3: function declarations are valid (`functions::declaration_findings`); every `FunctionKey` in constraints and in listed bindings' `unavailable` names a function of its own terminal; the default resolution (fixed functions and design defaults, no explicit selections) has no conflicts under the component's capacity and `Excludes` constraints; no listed binding marks a fixed function or a design default unavailable (`functions::binding_findings`).
- **Symbol:** 1–64 units with unique IDs and labels; anchors with unique terminals across all units; finite geometry; positive body.
- **Package:** 1–512 terminals with unique IDs and unique printable designations; positive finite envelope.
- **Footprint:** 1–1024 pads with unique IDs and positive finite size; every pad's package terminal exists in the pinned package; every package terminal has ≥ 1 pad; finite datum.
- **Binding:**
  - Every package terminal of the pinned package maps to exactly one electrical terminal, and no unknown package terminal is mapped.
  - 1–16 footprints, at most one per footprint lineage, each pinning the same package revision.
  - Slice 3: `unavailable` keys are unique. Whether they name real functions is checked by the owning component's publication, which has the declarations.
- **Field and quantity kind (slice 2):** see [slice 2](#slice-2--typed-fields-and-quantities).

The slice 3 default-resolution rule guarantees that `component.place` with no explicit selections, and any physical choice the component lists, starts in a conflict-free state. A component that could never be placed is rejected at publication.

A component may be published with **no implementations**. Packages, footprints and bindings may be published independently once their own dependencies validate. Publication never stamps verification. `provenance` remains free text, separate from structural validity.

## (c) Generator and override seam (deferred to CE-03)

These slices only keep the seam open. They do not implement recipes, generation keys or overrides.

- **Persistent IDs are separate from generation keys.** `PadId` (and later other generated object IDs) are persistent local IDs from `next_local`. A future generator returns objects keyed by a recipe-defined `GenerationKey` (string, bounded). It never returns `PadId`s.
- **Where the mapping lives.** A future `GeneratorRecipe` definition kind (`Definition::Recipe`, with its own revisions and pinned source/API/input fields) is pinned by the footprint revision it produced. The footprint revision then carries `generated: Option<GeneratedFootprint { recipe: RecipeRef, keys: Vec<(GenerationKey, PadId)>, overrides: Vec<PadOverride { pad: PadId, property, value }> }>` inside `library::footprint`. The mapping and overrides are part of the immutable revision that the snapshot preserves. Neither is instance state.
- **What these slices do.** `part.generate_pads` is renamed `library.generate_pads { draft, script }`. It operates on a footprint draft with the existing proof semantics: it emits each existing `PadId` exactly once, preserves package-terminal associations and allocates nothing. It does not persist script source or create a recipe.

## (d) Module map

### Core modules (`crates/editor-core/src`)

Each row is one cohesive concern with a narrow public surface. "May use" is the complete allowed set of `crate::` imports, including names reached through crate-root re-exports. `cargo xtask core-modules` (part of `cargo xtask ci`) enforces this table; change the table in `xtask/src/core_modules.rs` and here together. The list is acyclic: a module uses only modules with a lower number. Two consequences are deliberate:

- **Data before rules.** `model` holds the project aggregate, including instance and occurrence data, with no rules. The rule modules above it (`instances`, `nets`, `schematic`, `layout`) take `&Project`, so a rule that needs whole-project state has an owner below `validate`, `updates` and `commands`.
- **Payloads live where they execute.** A command payload type lives in the lowest module that can execute it. `SchematicCommand` and `LayoutCommand` stay in `schematic` and `layout` with an `apply(&mut Project, command, EditPolicy)` that holds their rules, as today. The project carries its library snapshot, so rule functions that take `&Project` need no separate `&Library`. `LibraryCommand` and `InstanceCommand` span several domain modules, so they live in `commands`, which only dispatches. `updates` never names a `commands` type.

Command handlers in `commands` orchestrate: they look up, clone a candidate, call domain functions and commit. They hold no domain rules. `Engine::apply` ends every candidate with `nets::reconcile` and `validate::edited`, which is `validate::project` except that the library snapshot is revalidated only when it changed.

| # | Module | Responsibility | Owns (types) | Public functions | May use |
| --- | --- | --- | --- | --- | --- |
| 0 | `geometry` (slice 1 addition) | Plain 2D coordinate vocabulary and persistence bounds | `Point` | `point_valid`, `finite_within`, `size_valid` | — |
| 1 | `ids` | Identity vocabulary, and the one display-label rule | `Id`, `DefinitionId`, `Revision`, `DefinitionKind`, exact `*Ref`s, `DefinitionRef` (kind-tagged), local ID newtypes, `FunctionKey`, `Endpoint`, `MAX_LABEL` | `Revision::next` (checked), `printable` (slice 3) | — |
| 2 | `diagnostics` | Command errors and findings | `Error`, `Diagnostic`, `Finding`, `ObjectRef` (gains `Definition`, `LibraryObject`, `Occurrence`, `Unit`; slice 2: `Property`), `Stage` | `Error::code`, `Finding::new`, `Finding::blocking`, `Finding::blocks_stage` | `ids` |
| 3 | `quantity` (slice 2, moved from `scripting/quantity.rs`) | Checked dimensional arithmetic and the one unit-scaling and unit-text rule over **data-supplied** units | `Dimension`, `Quantity`, `UnitOfMeasure`, `UnitSet`, `ResolvedUnit`, `Parsed`, `SI_PREFIXES` | `Quantity::{new, checked_add, checked_sub, checked_mul, checked_div, scale, scale_integer, value, dimension}`, `UnitSet::{check, unit, resolve, to_si}`, `parse(text, &UnitSet, default_unit)`, `format(si, &UnitSet, default_unit, display_unit)`, `format_number` | `diagnostics` |
| 4 | `fields` (slice 2) | Value types and value semantics | `QuantityKind`, `FieldDefinition`, `ValueType`, `Choice`, `ToleranceBasis`, `FieldValue`, `FieldSelection`, `FieldOverride`, `InstanceFields`, `PropertyEdit`, `PropertyInput`, `EffectiveField`, `Origin` | `parse_value`, `check_value`, `apply_input`, `compatible`, `effective`, `unmet`, `display`, `tolerance_interval`, `definition_findings`, `kind_findings` | `ids`, `diagnostics`, `quantity` |
| 5 | `functions` (slice 3) | Terminal function declarations, selection resolution and resource constraints | `TerminalFunctions`, `Function`, `Direction`, `SignalKind`, `Drive`, `ResourceRole`, `Resource`, `Role`, `FunctionConstraint`, `FunctionSelections`, `FunctionEdit`, `TerminalDeclaration`, `Resolution`, `ActiveFunction`, `ActiveOrigin`, `Conflict`, `Requirement` | `declaration_findings`, `binding_findings`, `resolve`, `apply_edits`, `declarations_within_bounds`, `TerminalFunctions::{fixed, implicit, within_bounds}` | `ids`, `diagnostics` |
| 6 | `library` (`mod`, `component`, `symbol`, `package`, `footprint`, `binding`, `drafts`, `publish`, `samples`) | Definition data, the snapshot store, drafts and publication | `DefinitionMeta`, `Definition`, `Library`, `Published<T>`, `FieldView` (slice 2), `ComponentDefinition`, `Terminal`, `TerminalClass` (slice 3), `SymbolDefinition`, `Unit`, `Anchor`, `PackageDefinition`, `PackageTerminal`, `FootprintDefinition`, `Pad`, `BindingDefinition`, `TerminalMap`, `Drafts`, `Draft` | `Library::{component, symbol, package, footprint, binding, field, quantity_kind, field_view, latest}`, `check_bounds`, `publish_findings`, `publish`, `validate`, `drafts::{create, edit, discard}`, `samples::two_terminal` | `geometry`, `ids`, `diagnostics`, `quantity`, `fields`, `functions` |
| 7 | `model` | Document aggregate: data only, no rules | `Project`, `Document`, `ComponentInstance`, `PhysicalChoice`, `SymbolOccurrence`, `Wire`, `Net`, `Placement`, `Track`, `Layer`, `Point`, `EditPolicy` | `Project::{allocate, allocate_net, instance, occurrence, occurrences_of, wires_at, placement_of, net, net_of, net_name}` (lookups only) | `geometry`, `ids`, `diagnostics`, `library`, `fields`, `functions` |
| 8 | `instances` | Instance configuration rules and choice resolution | `Resolved`, `PhysicalView`, `UnitState`, `UnitTarget`, `RegroupAction` | `resolve`, `owning_unit`, `unit_states`, `function_resolution`, `effective_fields`, `display_label`, `check_instance`, `check_designators`, `place`, `remove`, `set_designator`, `set_unit_unused`, `set_no_connect`, `set_properties`, `carry_properties` (slice 2), `select_functions`, `carry_functions` (slice 3), `carry_units`, `regroup`, `wired_terminals` (cleanup) | `geometry`, `ids`, `diagnostics`, `library`, `model`, `fields`, `functions` |
| 9 | `nets` | Persistent net identity and connectivity | `NetChanges` | `terminals`, `components`, `accounted_for`, `reconcile`, `validate`, `name_valid` | `ids`, `diagnostics`, `library`, `model`, `instances` |
| 10 | `schematic` | Capture rules: wires, occurrences, pin geometry | `SchematicCommand` | `apply`, `connect`, `disconnect`, `rename_net`, `place_unit`, `move_unit`, `unplace_unit`, `detach_instance`, `pin_position`, `validate`, `protect_routed_nets` | `geometry`, `ids`, `diagnostics`, `library`, `model`, `instances`, `nets` |
| 11 | `layout` | Placement, routing, board checks | `LayoutCommand`, `WorldPad`, `RouteRequest`, `RouteEditSet`, `ManualRouter` | `apply`, `pads`, `sync_preview`, `place`, `route`, `set_physical`, `detach_instance`, `checks`, `airwires`, `validate` | `geometry`, `ids`, `diagnostics`, `library`, `model`, `instances` |
| 12 | `erc` (extracted from `schematic::checks`) | Net-level electrical rules over active terminal behavior | — | `checks` | `ids`, `diagnostics`, `library`, `model`, `instances`, `functions`, `nets` |
| 13 | `completeness` | Derived instance and definition states | `InstanceReport`, `UnitReport`, `DefinitionReport`, `CheckTarget` | `instance`, `definition`, `drafts` | `ids`, `diagnostics`, `library`, `model`, `instances`, `fields`, `functions`, `nets` |
| 14 | `catalog` (`mod`, `query`, `synthetic`; [parts library S1](parts-library.md)) | Derived search entries, the one-line query language, deterministic ranking and data-derived facets; pure and read-only | `CatalogEntry`, `EntryField`, `Badges`, `Source`, `Index`, `CatalogQuery`, `Term`, `FieldTest`, `Test`, `QueryError`, `Vocabulary`, `CatalogContext`, `SearchReport`, `FacetReport`, `ItemReport` | `entries`, `parse_query`, `rkm`, `Index::{search, facets}`, `inspect`, `synthetic::{project, index}` | `ids`, `diagnostics`, `quantity`, `fields`, `library`, `model`, `instances`, `completeness` |
| 15 | `scripting` | Bounded Rhai execution only | `Script`, `Input` | `evaluate_quantity`, `generate_pads` | `ids`, `diagnostics`, `quantity`, `library` |
| 16 | `validate` | Whole-document invariants, aggregating domain validators | — | `project`, `document` | `ids`, `diagnostics`, `library`, `model`, `instances`, `nets`, `schematic`, `layout` |
| 17 | `updates` | Revision-bound impact plans and consequential edits | `Change`, `InstanceChange`, `Resolutions`, `ChoiceResolution`, `UnitResolution`, `RegroupResolution`, `FieldResolution` (slice 2), `FunctionResolution` (slice 3), `ApplyUpdate`, `UpdatePlan` | `preview`, `candidate`, `stage` | 0–16 |
| 18 | `commands` (`library`, `instance`) | Wire-level library and instance command families; dispatch only | `LibraryCommand`, `InstanceCommand`, `Outcome` | `apply_library`, `apply_instance` | 0–17 |
| 19 | `format` | `.pcbcraft` version and bounded encoding | `EditorFile` | `decode`, `encode`, `encode_bounded` | `diagnostics`, `model`, `library`, `validate` |
| 20 | crate root (`lib.rs`) | Engine, `Operation`, `Query`, schema discovery | `Engine`, `Operation`, `Query`, `QueryResult` | `Engine::{apply, query, query_with, preview_update, apply_update, undo, redo, …}`, `schemas` | all |

**Rule owners.** Each project-level rule is one function. Direct commands, `updates::stage`/`candidate` and `validate::project` all call it:

| Rule | Owner | Called from |
| --- | --- | --- |
| Choices offered by the pinned component; unit IDs exist in the resolved representation; `Placed` and `Unused` exclusive; no-connect only on `Electrical` terminals; every wire endpoint's owning unit placed, `Electrical` and not project no-connect (so no-connect and wiring exclude each other); field values valid (slice 2); selections structurally valid and conflict-free, including availability in the chosen binding (slice 3) | `instances::check_instance(&Project, InstanceId)` | Every `instances::*` edit, `schematic::{connect, place_units}`, `layout::set_physical`, `updates::candidate`, `validate::project` |
| Designators unique case-insensitively and well formed | `instances::check_designators(&Project)` | `instances::{place, set_designator}`, `validate::project` |
| Unplacing a unit: direct only when no wire touches its terminals; consequential removes and lists those wires | `schematic::unplace_unit(&mut Project, occurrence, EditPolicy)` | `SchematicCommand::UnplaceUnit` with `Direct` or (through `Change::Schematic`) `Consequential`; dropped units in a component-revision update |
| Creating a symbol occurrence within `MAX_OCCURRENCES` | `instances::add_occurrence` (unchecked constructor), composed by `schematic::place_units` and `instances::place`, which then check the instance | `SchematicCommand::PlaceUnit`, `component.place`, `RegroupAction::PlaceUnit` |
| Removing the wires on some terminals of one instance: direct only when none is wired | `schematic::disconnect_terminals(&mut Project, instance, terminals, EditPolicy)` | Component-revision updates (terminals removed or made NC, `RegroupAction::AcceptDisconnect`) |
| A terminal may carry wires or a project no-connect: it exists and is not device NC | `instances::wirable(&ComponentDefinition, TerminalId)` | `instances::retarget`, component-revision updates |
| Pinning an instance to another revision of its lineage, dropping no-connect markers on terminals that are no longer wirable | `instances::retarget(&mut Project, instance, Retarget)` | Component-revision updates, which first unplace dropped units and disconnect unwirable terminals, then check the instance |
| Carrying occurrences and unused flags to a target representation (same-lineage keep, cross-lineage resolution, each target unit claimed once) and the wired-terminal regroup decision | `instances::carry_units`, `instances::regroup` (pure; `updates` applies the result through `schematic` and `instances::retarget`) | Component-revision updates |
| A draft's pending revision: first revision of a new lineage, or the next of a base that is still the latest | `library::pending_revision` (and `library::stageable`, which adds bounds and the finding a draft that cannot be staged carries) | `library.publish` through `library::candidate`, `component.check`, `component.check_drafts` |
| Removing an instance's occurrences: direct only without wires | `schematic::detach_instance(&mut Project, instance, EditPolicy)` | `updates::stage` |
| Removing an instance's placement and tracks: direct only without a placement | `layout::detach_instance(&mut Project, instance, EditPolicy)` | `updates::stage` |
| Changing the physical choice: direct only without a board placement | `layout::set_physical(&mut Project, instance, choice, EditPolicy)` | `updates::stage` |
| Terminal accounted for: on a net with ≥ 2 terminals, or project no-connect | `nets::accounted_for(&Project, Endpoint)` | `completeness::instance` |

`updates::stage(&mut Project, &InstanceChange, EditPolicy)` is the single composition of consequential instance edits. `InstanceChange::Remove` runs `schematic::detach_instance`, `layout::detach_instance` and `instances::remove` in that order; `InstanceChange::SetPhysical` runs `layout::set_physical`. `commands` serves `component.remove` and `instance.set_physical` by calling `updates::stage` with `EditPolicy::Direct`; `project.apply_update` calls it with `EditPolicy::Consequential`. The guard and the composition are therefore written once.

Notes on the boundaries:

- `quantity` knows nothing about Rhai. `scripting` registers `quantity::Quantity` operations and keeps its `Script`/`Input` types and pad-specific `millimetres` conversion. Rhai values never leave `scripting`.
- `fields` owns value meaning: parsing, checking, defaults versus overrides and formatting. `library` stores field definitions but calls `fields` to check them. `instances` calls `fields` to check and resolve `InstanceFields`.
- `functions` owns function meaning and the single `resolve` algorithm. Its inputs are plain `functions` types; it never sees `library` or `model` types. `instances::function_resolution(&Library, &ComponentInstance)` is the one place that builds those inputs from the pinned component and chosen binding. Three modules consume the output, each for a different purpose:
  - `instances::check_instance` rejects structural selection errors and the `conflicts` bucket.
  - `completeness` reports the `unresolved` and `incomplete` buckets as findings.
  - `erc` applies drive rules to the `active` bucket.

  Each rule is implemented once, in `functions`.
- `nets`, `erc` and `completeness` read terminal and component declarations directly through `library`, which their "may use" sets list. They read definitions only through the exact references that `instances::resolve` returns.
- `model` is an aggregate with no rules; its public functions are lookups. `EditPolicy { Direct, Consequential }` lives there because `schematic`, `layout` and `updates` share it.
- `parts.rs` is deleted; its rules move to `library::publish` and its templates to `library::samples`. `schematic::checks` moves to `erc`.
- Intra-crate module dependencies are not enforced by `cargo xtask layers`. This table is the reviewed contract (see [open questions](#open-questions-and-deferrals)).

### Crates outside core

| Crate | Slice | Change | Why |
| --- | --- | --- | --- |
| `crates/io` | 1 | Fixture and test updates only. `EditorFile` with `drafts` flows through existing atomic save/recovery. | File effects stay here. The new shape is a core type. |
| `crates/solidcraft-integration` | 1 | `project::build` resolves package envelope, footprint pads and datum through `instances::resolve`. `preview(&Part)` becomes `preview(&PackageDefinition)`. | Native modeling stays in the adapter. It reads resolved definitions and never the library internals. |
| `crates/automation` | 1–3 | Session returns `outcome` with each applied operation; the snapshot reports `drafts` with their findings; the `project.query` request carries a core `Query` (`component.inspect`, `component.check`, `component.check_drafts`) unchanged to `Engine::query`; MCP instructions and the catalog text are updated. The catalog's false "older versions migrate" claim is corrected. | Transports translate. Schemas are generated from the core types. No report is assembled outside core. |
| `apps/pcbcraft` | 1–3 | The part editor edits a working set of drafts (component, one symbol, package, footprint, binding) and commits through `library.edit`/`library.publish`. The schematic editor places through `component.place`, draws every occurrence and shows unit states read-only from `Engine::query`. The scene keys native previews by package. Slice 2 edits fields marked `show_on_symbol` through `instance.set_properties`. Slice 3 maps today's terminal-kind dropdown to one fixed function. | The desktop keeps its current capability on the new model. New multi-unit/function UI belongs to the CE UI work. |
| `apps/pcbcraft-cli` | 1 | Comment wording only. | Thin transport. |
| `xtask` | — | No change. | Crate layers are unchanged. No new crate is created. |

## (e) Command families

All mutations are `Operation` variants: `Library(LibraryCommand)`, `Instance(InstanceCommand)`, `Schematic(SchematicCommand)`, `Layout(LayoutCommand)` and `Update(Box<ApplyUpdate>)`. `Operation::Part` is removed. `Engine::apply` returns `Result<Outcome>`, where `Outcome` is one of:

- `None`
- `DraftCreated { id }`
- `Published { revisions: Vec<DefinitionRef> }`
- `InstancePlaced { instance, occurrence }`
- `UnitPlaced { occurrence }`
- `PropertiesSet { effective: Vec<EffectiveField> }` (slice 2)
- `FunctionsSelected { resolution: functions::Resolution }` (slice 3)

Every successful mutation is one undo step.

Read-only reports are `Query` variants answered by `Engine::query(&self, Query) -> Result<QueryResult>`: `Query::InspectInstance { instance }` returns `completeness::InstanceReport`, `Query::CheckDefinition { target: CheckTarget }` returns `completeness::DefinitionReport` and `Query::CheckDrafts {}` returns one `DefinitionReport` per draft, in draft order, from a single staging. Queries never create undo entries or change the revision. CLI, MCP and the desktop call `Engine::query` and present its result unchanged.

### Delivered by slice 1

The spec's proposed `component.new` is served by `library.new_draft` with a component body. No separate command exists, so there is one way to create a lineage.

| Command | Family | Input | Result | Contract |
| --- | --- | --- | --- | --- |
| `library.new_draft` | `LibraryCommand` | `{ meta: DefinitionMeta, definition: Definition }` | `DraftCreated` | New lineage of any kind. The body may be empty. No category or template. |
| `library.draft_from` | `LibraryCommand` | `{ revision: DefinitionRef }` | `DraftCreated` | Draft of an existing lineage with `base` = that revision. Fails if a draft of that lineage exists. |
| `library.edit` | `LibraryCommand` | `{ edits: Vec<DraftEdit { id, meta, definition }> }` (1–16) | `None` | Replaces draft bodies atomically. The kind must match. One gesture that touches several definitions (for example, a terminal plus its anchor and mapping) is one undo step. |
| `library.discard_draft` | `LibraryCommand` | `{ id }` | `None` | Removes the draft. A reserved new-lineage ID stays allocated. |
| `library.publish` | `LibraryCommand` | `{ drafts: Vec<DefinitionId> }` (1–32) | `Published` | [Rules above](#what-librarypublish-validates). |
| `library.generate_pads` | `LibraryCommand` | `{ draft, script }` | `None` | Replaces `part.generate_pads`, with the same proof semantics on a footprint draft. |
| `component.place` | `InstanceCommand` | `{ component: ComponentRef, designator, value, representation: DefinitionId, unit: UnitId, position, physical: Option<PhysicalChoice> }` | `InstancePlaced` | Creates one instance and its first occurrence (`instances::place`). |
| `component.remove` | `InstanceCommand` | `{ instance }` | `None` | `updates::stage(Remove, Direct)`: direct only when the instance has no wires and no placement. Otherwise `conflict`, directing the caller to `Change::Instance`. |
| `instance.set_designator` | `InstanceCommand` | `{ instance, designator }` | `None` | Unique case-insensitively; 1–64 bytes, no whitespace or control characters. |
| `instance.set_value` | `InstanceCommand` | `{ instance, value }` | `None` | Slice 1 only; removed in slice 2. |
| `instance.set_unit_unused` | `InstanceCommand` | `{ instance, unit, unused }` | `None` | Rejected while the unit is placed. Never changes no-connect markers. |
| `instance.set_no_connect` | `InstanceCommand` | `{ instance, terminals: Vec<TerminalId>, no_connect }` (1–256) | `None` | Project decision. Rejected for wired terminals by `instances::check_instance`. |
| `instance.set_physical` | `InstanceCommand` | `{ instance, choice: Option<PhysicalChoice> }` | `None` | `updates::stage(SetPhysical, Direct)`: direct only when the instance has no board placement. Otherwise use `Change::Instance`. |
| `symbol.place_unit` | `SchematicCommand` | `{ instance, unit, position }` | `UnitPlaced` | Rejected if the unit is already placed or marked unused. |
| `symbol.move_unit` | `SchematicCommand` | `{ occurrence, position }` | `None` | Presentation only. |
| `symbol.unplace_unit` | `SchematicCommand` | `{ occurrence }` | `None` | Direct only when no wire touches the unit's terminals. Otherwise `conflict`, directing the caller to `Change::Schematic`. |
| `schematic.connect`, `schematic.disconnect`, `schematic.net_rename` | `SchematicCommand` | Unchanged | `None` | Both endpoint units must be placed; device-NC and project no-connect terminals cannot be wired (`instances::check_instance`). |
| `layout.*` | `LayoutCommand` | Unchanged | `None` | `layout.sync` places only board-placeable instances. A placement requires a resolvable physical choice. |
| `project.preview_update` / `project.apply_update` | `Update` | `Change::{ComponentRevision { instances, target, resolutions }, Instance { edit: InstanceChange }, Schematic { command: SchematicCommand }, SyncPcb {}}` | `UpdatePlan` / `None` | `InstanceChange` is `Remove { instance }` or `SetPhysical { instance, choice }`, staged with `EditPolicy::Consequential`. `Change::Schematic` stages a schematic command the same way (for example, unplacing a wired unit). Plans list removed wires, affected nets and placements, and conflicting tracks, using the existing plan fields. |
| `component.inspect` | `Query::InspectInstance` | `{ instance }` | `InstanceReport` | Exact pins and resolved choices; per-unit `Placed { occurrence }`/`Unplaced`/`Unused`; `occurrences`, `board_placements` (0 or 1), `bom_quantity` (always 1 per instance); completeness states and findings. |
| `component.check` | `Query::CheckDefinition` | `{ target: CheckTarget::{Draft(DefinitionId), Revision(DefinitionRef)} }` | `DefinitionReport` | Findings with `blocks` for exactly the named draft or published revision. |
| `component.check_drafts` | `Query::CheckDrafts` | `{}` | `Vec<DefinitionReport>` (`QueryResult::Drafts`) | `component.check` of every draft, staged once; used by session snapshots and the component editor. |

`Resolutions` (slice 1) contains:

- `ChoiceResolution { instance, representation: Option<DefinitionId>, physical: Option<Option<PhysicalChoice>> }`
- `UnitResolution { instance, unit: UnitId, target: UnitTarget::{Unit(UnitId), Drop} }`. `unit` is the current unit in the old representation; `Unit` names a unit of the target representation. It carries that unit's occurrence and its `unused_units` entry, whichever exists, to the target unit, which must not already be claimed. `Drop` removes the occurrence (listing its wires) or clears the unused flag.
- `RegroupResolution { instance, terminal, action: RegroupAction::{PlaceUnit { position }, AcceptDisconnect} }`.

### Delivered by slice 2

| Command | Family | Input | Result | Contract |
| --- | --- | --- | --- | --- |
| `library.new_draft` / `library.edit` / `library.publish` | `LibraryCommand` | `Definition::QuantityKind`, `Definition::Field` | As above | Fields and quantity kinds are library data with their own revisions. |
| `instance.set_properties` | `InstanceCommand` | `{ instance, edits: Vec<PropertyEdit { field: DefinitionId, input: PropertyInput::{Text(String), Value(FieldValue), Inherit, Unset} }> }` (1–64) | `PropertiesSet` | Atomic batch. Text is parsed in core. Any invalid edit rejects the whole batch. Replaces `instance.set_value`. |
| `component.place` | `InstanceCommand` | Gains `properties: Vec<PropertyEdit>` and loses `value` | `InstancePlaced` | — |
| `Change::ComponentRevision` | `Update` | `Resolutions` gains `FieldResolution { instance, field, input: PropertyInput }` | `UpdatePlan` | — |

### Delivered by slice 3

| Command | Family | Input | Result | Contract |
| --- | --- | --- | --- | --- |
| `instance.select_functions` | `InstanceCommand` | `{ instance, edits: Vec<FunctionEdit { terminal, function: Option<FunctionId> }> }` (1–256) | `FunctionsSelected` | Atomic batch. `None` clears an explicit selection. The final state is validated once, so a batch can move a role between terminals with no intermediate commit. |
| `instance.set_physical`, `Change::Instance { SetPhysical }` | `InstanceCommand`, `Update` | Unchanged | — | Also rejected or blocked when a selected function is unavailable in the target binding (`instances::check_instance`). |
| `Change::ComponentRevision` | `Update` | `Resolutions` gains `FunctionResolution { instance, terminal, function: Option<FunctionId> }` | `UpdatePlan` | — |

### Proposed families that remain proposed

| Family | Status after these slices |
| --- | --- |
| `library.duplicate` | Proposed. Forking needs lineage-reference remapping and provenance (CE-01). |
| `instance.preview_symbol` (representation switch) | Proposed for CE-01. The data supports it via `ChoiceResolution` and `UnitResolution`, but no switch command ships. |
| `instance.preview_implementation` | Covered by `project.preview_update` with `Change::Instance { SetPhysical }`. The dedicated name, geometry-aware affected-copper evaluation and package-switch mapping review remain CE-01. |
| `library.preview_update` (library-level cascade preview), `library.prune`, `library.adopt` (dependent-draft helper) | Proposed. |
| `generator.preview`, recipes, overrides | Proposed, CE-03 ([seam above](#c-generator-and-override-seam-deferred-to-ce-03)). |
| `change.apply` | Not introduced. `project.apply_update` remains the apply path. |
| `model.attach`, `reference.attach`, manufacturer parts, distributor offers, requirement comparisons | Proposed (CE-01, CE-02, CE-07). |
| Fine-grained `library.edit` operations (add terminal, move anchor) | Proposed with the CE UI work. These slices replace whole draft bodies. |

## Slice 1 — definitions, revisions, instances and units

**Replaces:**

- The `Part`, `PartRef`, `Component`, `Symbol`, `SymbolPin`, `Package`, `Footprint`, `Binding` and `Instance` types in `model.rs`.
- `parts.rs` and `PartCommand`.
- `SchematicCommand::{Place, Move, Edit, Remove}`, replaced by `component.place`, `symbol.*`, `instance.set_designator`/`set_value` and `component.remove`.
- `Change::PartRevision`.
- `Document.draft` and `EditorFile.authoring_draft`.
- `parts::passive_template`/`connector_template`, which become `library::samples`.
- `erc.unconnected_input`. A lone terminal is reported once, by `completeness` as `instance.terminal_unaccounted`; ERC no longer restates it.

`Terminal { id: TerminalId, name, kind: PinKind }` moves into `library::component` unchanged until slice 3.

**Instance model.** `model::ComponentInstance` has these fields:

- `id`
- `component: ComponentRef`
- `designator`
- `value: String` (until slice 2)
- `representation: DefinitionId`
- `physical: Option<PhysicalChoice>`
- `unused_units: BTreeSet<UnitId>`
- `no_connect: BTreeSet<TerminalId>`

`model::SymbolOccurrence { id, instance, unit, position }` replaces `Instance.position`. Each (instance, unit) has at most one occurrence, on one sheet. Unit states are derived by `instances::unit_states`:

- `Placed` if an occurrence exists.
- `Unused` if listed in `unused_units`.
- `Unplaced` otherwise.

`Placed` and `Unused` are exclusive (`instances::check_instance`).

**Electrical invariants:**

- Every terminal of every instance has exactly one net, whatever its unit's state. This is unchanged from today.
- A wire endpoint's owning unit must be placed. A wire endpoint must not be project no-connect or device NC (`instances::check_instance`).
- `schematic::pin_position` is the occurrence position plus the terminal's anchor in its owning unit.
- `layout::pads` resolves the binding and footprint through `instances::resolve`. A placement exists only for a resolvable physical choice.

Designator display is `designator` plus the unit label (`Q7` + `A` → `Q7A`) when the representation has more than one unit. The label is presentation only.

**Demo and templates.** `Engine::demo` builds its sample library through the public commands, then places, connects and syncs as today. `library::samples::two_terminal(name)` returns a draft set (component, one-unit symbol, package, footprint, binding). That is generic data, not a category.

### Slice 1 implementation notes

Where the code had to settle something this contract left open, it chose the option that keeps concerns separate. These choices are now part of the contract:

- **`geometry` module.** `Point`, `point_valid` and `size_valid` live in a new `geometry` module below `ids`, because `library` (6) stores coordinates and cannot use `model` (7). `model` re-exports `Point`. `geometry` uses no other module.
- **Unit bodies.** `SymbolDefinition { component, next_local, units }`; each `Unit { id, label, body_size, anchors }` has its own body, since units are drawn independently. Bindings allocate no local IDs and have no `next_local`.
- **Field names.** `PackageTerminal.designation`, `TerminalMap { package_terminal, terminal }`, `BindingDefinition { component, package, mappings, footprints }`, `FootprintDefinition { package, next_local, pads, datum_offset }`. `ObjectRef::LibraryObject { definition, local }` names any local object of a definition.
- **Advisory finding.** A component revision without implementations carries `component.no_implementation` with `blocks: None`; it publishes, reopens and is placeable.
- **Draft checks.** `component.check` on a draft evaluates its pending revision against the snapshot plus every other current draft's pending revision, so references inside a publish set resolve as they would on publish. `library::candidate` stages them, the same staging `library.publish` uses. A draft whose base is stale, invalid or out of bounds is not staged; its report carries only the blocking `definition.stale_base`, `definition.base_invalid`, `definition.revisions_exhausted` or `definition.bounds` finding and is never answered from a stored revision with the same number. `component.check_drafts` stages every draft once and reports them all.
- **Error codes.** Blocking findings make `library.publish` fail with `conflict` and document decode fail with `invalid_input`. `library.draft_from` accepts any stored revision as the base; publishing a non-latest base is the stale `conflict`.
- **One-pass validation.** `instances::check_all` runs `check_instance`'s single rule body for every instance with one pass over occurrences and wires; `validate::project` calls it instead of looping `check_instance`.
- **Routed-net protection.** `Engine::apply` calls `schematic::protect_routed_nets` after `nets::reconcile` for every operation except `Update`; reviewed updates list affected copper instead.
- **`layout::apply` takes no `EditPolicy`.** Layout commands have no consequential form; the consequential layout edits are `layout::set_physical` and `layout::detach_instance`, called by `updates::stage`.
- **Payload types.** `component.place` carries `instances::NewInstance`. `Query` is tagged `query`/`params` with the command names `component.inspect`/`component.check`; `QueryResult` is tagged `report`/`result`; `CheckTarget` is tagged `target`/`id`. `UnitTarget` serializes as `{"unit": n}` or `"drop"`. In `ChoiceResolution`, an absent `physical` keeps the current choice, `null` clears it and a value replaces it.
- **Resolution checks.** A resolution naming an instance outside the update, or a `UnitResolution` for a unit with neither an occurrence nor an unused flag, is `invalid_input`. A `RegroupResolution` for a terminal that needs none is ignored. A dropped unit takes the wires of the terminals it drew in the current representation.
- **Samples.** `library::samples` has `two_terminal` and `two_terminal_header` (the desktop's Connector template). Both take `SampleRefs`, the lineages and the revisions they pin of each other; neither is a category.
- **Inspection report.** `InstanceReport` also carries `display` labels per unit, `board_placeable`, `electrically_resolved` and `purchasing: "not computed: …"`.
- **Desktop.** The part editor authors one working set of five drafts. Publishing always publishes all five, so unchanged siblings get identical new revisions. In a project without drafts, a template creates its lineages with five `library.new_draft` calls and one `library.edit`, which are six undo steps, not one. Applying designator and value together is two commands.

## Slice 2 — typed fields and quantities

**Quantity module.** Move `scripting/quantity.rs` to `crate::quantity` and make its checked operations public. `scripting` imports it, and the reverse import is forbidden.

**One unit type.** `UnitOfMeasure { symbol, aliases: Vec<String>, scale: f64, si_prefixes: bool }` replaces `UnitDefinition { symbol, dimension, scale }`. A unit carries no dimension of its own. `UnitSet { dimension: Dimension, units: Vec<UnitOfMeasure> }` supplies it for a group of units, and `UnitSet::to_si(value, symbol)` is the single scaling rule. `fields::QuantityKind` embeds one `UnitSet`. `scripting::Script.units` becomes `Vec<UnitSet>`, and `scripting::Input::Quantity { value, unit }` is scaled through `UnitSet::to_si`; a unit symbol must be unique across a script's sets. The scripting proof and field parsing therefore share one type and one rule.

`parse(text, &UnitSet, default_unit)` follows these rules:

- **Grammar:** `number [whitespace] [suffix]`, where `number` is a decimal with optional sign and exponent and `.` as the separator.
- **Suffix resolution, first match wins:**
  1. The suffix is a unit symbol or alias.
  2. The suffix is an SI prefix followed by a prefix-accepting unit symbol.
  3. The suffix is a bare SI prefix applied to the default unit.
  4. There is no suffix, so the default unit applies.
- **Examples:** with a resistance kind whose default unit is `Ω`, `10k`, `10 kΩ` and `10000 Ω` all parse to SI value 10000. `10m` gives 0.01 Ω. Rule 1 would make it 10 m for a length kind with unit `m`.
- **Prefix table:** the SI prefixes from p to G, including `µ` and the ASCII alias `u`.
- **Rejections:** any other suffix, such as `µF` for resistance, is rejected as `invalid_input` naming the unit and kind. Non-finite results or results beyond the existing ±1e100 bound are rejected too.
- **Deferred:** RKM notation such as `4k7`, affine units and logarithmic units.

**Field data** (`fields` bodies, stored as library definitions with `DefinitionMeta` in the library envelope):

- `QuantityKind { units: UnitSet, default_unit }` is a definition with identity. Two kinds with the same dimension are different kinds. A unit symbol never establishes kind identity.
- `FieldDefinition { value_type: ValueType, default: Option<FieldValue>, required_at: Option<Stage> }`, where `ValueType` is one of:
  - `Quantity { kind: QuantityKindRef, min: Option<f64>, max: Option<f64> }` (SI)
  - `Integer { min: i64, max: i64 }`
  - `Boolean`
  - `Enum { choices: Vec<Choice { id: ChoiceId, label }> }`
  - `Text { max_bytes: u16 }` (≤ 256)
  - `Tolerance { basis: Relative | Absolute { kind: QuantityKindRef } }`
  - `Range { kind: QuantityKindRef }`
- `FieldValue` is one of:
  - `Quantity { si: f64, display_unit: Option<String> }`
  - `Integer(i64)`
  - `Boolean(bool)`
  - `Enum(ChoiceId)`
  - `Text(String)`
  - `Tolerance { minus: f64, plus: f64 }` (fractions for relative: `5%` is 0.05 with percent semantics; SI for absolute)
  - `Range { min: f64, max: f64 }`

  `display_unit` is presentation and never changes `si`.
- `ComponentDefinition.fields: Vec<FieldSelection { field: FieldRef, default: Option<FieldValue>, show_on_symbol: bool }>`. The component default overrides the field default and must satisfy the field's constraints.
- `ComponentInstance.properties: InstanceFields` replaces `value`. It is a `BTreeMap<DefinitionId (field lineage), FieldOverride::{Assigned(FieldValue), Unset}>`. A missing key means inherited.
- `fields::effective` returns `EffectiveField { field, value: Option<FieldValue>, origin: Assigned | ComponentDefault | FieldDefault | ExplicitlyUnset | NoValue }`. Assigned zero is a value and is distinct from `ExplicitlyUnset`.
- A required field with no value produces a finding blocking its `required_at` stage.
- Overrides are keyed by field lineage, so a field revision that only renames its label (`DefinitionMeta.name`) keeps every assignment.

**Updates.** In a component-revision plan, a field-revision change reports effective-value changes per instance. If a field kind changes, constraints narrow so an override no longer fits, or a populated field is removed, the plan is blocked until a `FieldResolution` is supplied. Valid overrides are preserved.

**Deferred.** These are not part of slice 2:

- **Derived fields (CE-03).** The seam is a future `derivation: Option<Derivation { expression, inputs: Vec<FieldRef> }>` on `FieldDefinition`, independent of the value type, evaluated by `scripting::evaluate_quantity` with an explicit dependency graph and cycle detection. No such field exists yet, and `Origin::Calculated` is not emitted.
- Structured reference fields.
- Requirement comparison semantics and manufacturer specifications (CE-01, CE-02).

### Slice 2 implementation notes

Where the code had to settle something this contract left open, it chose the option that keeps concerns separate. These choices are now part of the contract:

- **Checked operation names.** The quantity operations are `Quantity::{checked_add, checked_sub, checked_mul, checked_div}`. Strict Clippy rejects inherent `add`/`sub`/`mul`/`div` methods that mimic the operator traits; the fallible operations follow the standard library's `checked_*` naming. Fields are private, so every value is constructed through `Quantity::new`.
- **One resolution rule.** `UnitSet::resolve(text)` applies parse rules 1 and 2 (an exact symbol or alias, then an SI prefix on the symbol or an alias of a prefix-accepting unit) and returns the factor and canonical spelling; `UnitSet::to_si` and `parse` both use it, and `parse` adds rules 3 and 4. A recorded `display_unit` is the canonical spelling (`10kohm` records `kΩ`; a bare `10k` records `kΩ`; no suffix records none). `UnitSet::check` is the one validity rule for a unit set: 1–32 units, at most 8 aliases each, scales within 1e-30–1e30, unique texts and a dimension within ±16. Unit texts that the grammar could not recognise after a number (empty, whitespace or control characters, `/`, `..`, a leading digit, sign or `.`, or an exponent-like `e3`) are rejected.
- **Prefixes.** The table is p, n, µ (with `u` and Greek `μ` accepted as spellings), m, c, d, da, h, k, M and G. Formatting without a recorded display unit uses the engineering subset p–G. Quantity text is limited to 128 bytes and property text to 256 bytes before parsing (`limit_exceeded`).
- **Text forms of other types.** Booleans accept true/false/yes/no; enumerations accept a choice label, case-insensitively; tolerances accept `±5%`, `+/-5%`, `5%` or `-20%/+80%` (also with U+2212 and `ppm`) for relative, and quantities of the named kind for absolute; ranges accept `min .. max`. `fields::display` formats values in a form `parse_value` reads back.
- **`FieldDefinition.next_local`.** Enumeration choice IDs are local IDs, so field definitions carry `next_local` and follow the [local-ID rules](#identity). Quantity kinds have no local IDs. `Choice { id, label }` and `ToleranceBasis::{Relative, Absolute { kind }}` are the named forms of the contract's choices and tolerance basis.
- **Publish order and findings.** Quantity kinds and fields come first in a publish set. Their findings are `quantity_kind.units`, `quantity_kind.default_unit` (the default unit must be a unit symbol), `field.value_type`, `field.choice_label`, `field.default_invalid`, `field.required_stage` (`required_at` may not be `Publish`) and `reference.missing`; components add `component.duplicate_field_lineage` and `component.field_default_invalid`. `fields::definition_findings` and `fields::kind_findings` own these rules; `library::publish_findings` resolves the kind and aggregates them, so decode and publication apply the same rules.
- **Effective fields carry presentation.** `EffectiveField { field: FieldRef, label, value, origin, text, show_on_symbol, required_at }`: the exact field revision selected by the pinned component, the field name, the formatted text and the selection's display flag, so transports and the desktop present values without reformatting. `component.inspect` reports them as `InstanceReport.properties`. A required field without a value is finding `instance.field_required` on `ObjectRef::Property { instance, field }`, blocking its `required_at` stage.
- **Rule owners.** `fields::apply_input` applies one `PropertyInput`; `instances::set_properties` and `component.place` (`NewInstance.properties`, default empty) look up the selected field and kind and call it for each edit, failing the batch on the first invalid edit with a message naming the instance, field and quantity kind. `instances::check_instance` checks every override: its field lineage is selected by the pinned component and an assigned value satisfies that field revision. `Library::field_view` resolves a field revision with its kind.
- **Updates.** `instances::carry_properties` owns the update rule. An assigned value blocks when the target no longer selects its field, when the value type or quantity-kind lineage changes (`fields::compatible`), or when it no longer satisfies the target field. An explicit `Unset` on a removed field is dropped and listed. A supplied `FieldResolution` is applied as a property edit against the target field even when none is needed, and must itself be valid; `Inherit` also resolves a removed field. Plans report effective-value changes and field renames per instance (`R1: field ‘Count’ 8 (Assigned) → 5 (Assigned).`).
- **Demo data.** `library::samples::resistance_field` supplies a resistance quantity kind (Ω with aliases `ohm` and `R`) and a non-negative quantity field; `Engine::demo` selects it on the sample component with `show_on_symbol` and places R1 and R2 with `10k`.
- **Desktop.** The schematic inspector edits the fields marked `show_on_symbol` as text through one `instance.set_properties` batch; an emptied box sends `Inherit`. It shows the rejection message from `fields::parse_value` while typing; the engine still decides on apply and its error appears in the status line. Symbols and the sheet list show the effective texts. Fields are not yet authored in the component editor; drafts of fields and kinds are edited through `library.edit`.

## Slice 3 — terminal functions and selections

**Declaration** (`functions` types, stored in `library::component`):

- `Terminal { id, name, class: TerminalClass }` replaces `kind: PinKind`. `TerminalClass` is one of:
  - `Electrical(functions::TerminalFunctions)`, where `TerminalFunctions { functions: Vec<Function>, design_default: Option<FunctionId> }` (1–32 functions) is the one declaration type owned by `functions`
  - `DeviceNc`
  - `Reserved`
- `Function { id: FunctionId, label, direction, signal: SignalKind, drive: Drive, resource_role: Option<ResourceRole { resource, role }> }`, with:
  - `Direction` = `Input` | `Output` | `Bidirectional` | `Passive` | `PowerInput` | `PowerOutput`
  - `SignalKind` = `Digital` | `Analog` | `Power` | `Unspecified`
  - `Drive` = `None` | `PushPull` | `OpenDrain` | `OpenSource`
- **Valid combinations:**
  - `Input`, `Passive`, `PowerInput` and `PowerOutput` require `Drive::None`.
  - `Output` and `Bidirectional` require a drive other than `None`.
  - `SignalKind::Power` is valid only with the two power directions, and the power directions require it.
  - Other combinations are rejected at publication.
- **Resource rules:** `ComponentDefinition.resources: Vec<Resource { id, label, roles: Vec<Role { id, label, capacity: u16, required_when_used: bool }> }>` and `constraints: Vec<FunctionConstraint::{Excludes { a: FunctionKey, b: FunctionKey }, Requires { selected: FunctionKey, requires: FunctionKey }}>`. Constraints are scoped to one instance.
- `BindingDefinition.unavailable: Vec<FunctionKey>` declares functions that are unavailable in that implementation.
- **Device NC versus project no-connect:**
  - `DeviceNc` and `Reserved` are definition classifications. They have no functions, cannot be wired and cannot take a project no-connect marker, which would be redundant.
  - `ComponentInstance.no_connect` is a project decision on an `Electrical` terminal.

**Selection and resolution.** `ComponentInstance.functions: FunctionSelections` maps `TerminalId` to `FunctionId`. The resolver's whole input is plain `functions` data:

```rust
pub struct TerminalDeclaration<'a> {
    pub terminal: TerminalId,
    pub functions: Option<&'a TerminalFunctions>, // None for DeviceNc and Reserved
}

pub fn resolve(
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    constraints: &[FunctionConstraint],
    unavailable: Option<&[FunctionKey]>, // None without a physical choice
    selections: &FunctionSelections,
) -> Result<Resolution>;
```

It returns `invalid_input` for structural errors: an unknown terminal, a function that does not belong to its terminal, or a selection on a non-`Electrical` terminal. It decides each terminal's active function in this order:

1. Explicit selection.
2. The only function of a fixed-function terminal.
3. `design_default`. A reset-state description is never a default.
4. Otherwise the terminal is unresolved.

`Resolution` has four buckets:

| Bucket | Contains | Handling |
| --- | --- | --- |
| `active` | The active function per terminal | Used by ERC; returned by `FunctionsSelected` |
| `unresolved` | Terminals with no active function | Finding `config.function_unresolved`, blocking `ElectricallyResolved` |
| `conflicts` | Role capacity exceeded; an `Excludes` pair both active; unavailable in the chosen binding | Invariant: **rejected** by `instances::check_instance`, so by `instance.select_functions` and `set_physical`, and blocked in plans. Never a finding. |
| `incomplete` | A role with `required_when_used` absent while another role of its resource is active; an unmet `Requires` | Finding `config.requirement_unmet`, blocking `ElectricallyResolved` |

Selections are keyed by IDs, so reordering or relabeling functions in a new revision preserves them.

**ERC** (`erc::checks` replaces the `PinKind` rules; findings have `blocks: None`). For each net, the rules use active functions only:

| Code | Condition |
| --- | --- |
| `erc.driver_conflict` | Two or more push-pull or open-source drivers (`Output`/`Bidirectional` with that drive). |
| `erc.open_drain_contention` | At least one push-pull driver together with at least one open-drain driver. |
| `erc.input_only_net` | ≥ 2 terminals, at least one `Input`, and no `Output`, `Bidirectional` or `Passive` terminal. |
| `erc.undriven_power_input` | A `PowerInput` with no `PowerOutput` on its net. Retained from today. |
| `erc.unchecked_terminal` | Emitted for each unresolved terminal on the net. That terminal is excluded from drive rules, so ERC never assumes all alternatives at once and never picks one. |

A terminal alone on its net is not an ERC condition. It is reported once, by `completeness`, as `instance.terminal_unaccounted` through `nets::accounted_for`.

Several open-drain drivers with a passive pull-up produce no finding. This is the observable proof that drive behavior, not direction alone, is used. In completeness, a `PowerInput` function with a project no-connect is `instance.required_terminal_no_connect`, so an obligation cannot be hidden.

### Slice 3 implementation notes

Where the code had to settle something this contract left open, it chose the option that keeps concerns separate. These choices are now part of the contract:

- **Shapes.** `TerminalClass` serializes externally tagged: `{"electrical": {"functions": […], "design_default": n}}`, `"device_nc"` or `"reserved"`, so a device-NC or reserved terminal with functions is not representable and decodes as malformed. `ComponentDefinition` gains `resources` and `constraints`; `BindingDefinition` gains `unavailable`; `ComponentInstance` gains `functions: FunctionSelections` (a map from terminal ID to function ID). `FunctionConstraint` is tagged `excludes`/`requires`.
- **One local-ID namespace.** Terminal, function, resource and role IDs all come from the component's `next_local` and follow the [local-ID rules](#identity). Duplicate function IDs are therefore the existing `definition.local_id_invalid` finding; `functions` does not restate uniqueness.
- **Resolution buckets.** `Resolution { active, unresolved, conflicts, incomplete }`. `ActiveFunction` carries the function's label, direction, signal, drive and `origin` (`ActiveOrigin::{Selected, Fixed, DesignDefault}`), so ERC needs no second lookup. `Conflict::{Capacity, Excluded, Unavailable}` and `Requirement::{RoleMissing, Requires}` carry typed keys and a message. `TerminalFunctions::implicit` is the one fixed-or-default rule, used by `resolve` and by `binding_findings`.
- **Publication findings** (`functions::declaration_findings`, `functions::binding_findings`, aggregated by `library::publish_findings`): `function.count`, `function.label` (unique per terminal), `function.combination`, `function.design_default`, `function.resource_unknown`, `resource.label`, `resource.role_count`, `resource.role_label`, `resource.role_capacity` (≥ 1), `function.constraint_key`, `function.default_conflict`, `binding.unavailable_unknown`, `binding.unavailable_required`; the binding's own `binding.unavailable_duplicate`.
- **Rule owners.** `instances::function_resolution` is the only place that builds `resolve` input. `instances::check_instance` rejects structural selection errors as `invalid_input` and any conflict as `conflict`; `layout::set_physical`, `updates::candidate` and decode therefore reject or block an unavailable function without a separate check. `instances::carry_functions` owns the update rule: a selection is kept when the target still offers that function on that terminal, otherwise the plan is blocked until a `FunctionResolution` names the terminal; resolutions are applied as selection edits and the result is checked once. Plans list function additions, removals, relabels (`U1 terminal 2: function 46 ‘uart-tx-alt’ is relabeled ‘uart-tx-b’; selections are kept by ID.`), behavior changes and selection changes; a reorder alone lists nothing.
- **Completeness.** `config.function_unresolved` names `ObjectRef::Terminal` of each unresolved terminal, including project no-connect terminals; `config.requirement_unmet` names the instance and the terminals whose functions take part. `instance.required_terminal_no_connect` uses the active function's `PowerInput` direction. `InstanceReport.functions` reports the resolution.
- **ERC.** `erc.multiple_outputs` is replaced by `erc.driver_conflict`; power outputs are no longer counted as drivers. `erc.driver_conflict`, `erc.open_drain_contention`, `erc.input_only_net` and `erc.unchecked_terminal` apply to nets with at least two terminals, because a lone terminal is a completeness condition. `erc.undriven_power_input` applies to every net, as before. `erc.unchecked_terminal` names the net and the terminal.
- **Labels.** `ids::printable` and `ids::MAX_LABEL` are the one label rule for `library`, `fields` and `functions`, replacing two private copies.
- **Fixed-function constructor.** `TerminalFunctions::fixed(id, direction)` builds one function with the conventional behavior of a direction (push-pull for driving directions, power signal for power directions). The samples, the desktop and the test fixtures use it; it is data construction, not a category.
- **Samples.** `two_terminal` terminals 1–2 carry fixed passive functions 3–4, so the sample component's `next_local` is 5.
- **Desktop.** The part editor's terminal dropdown offers passive, input, output, power input, power output and device NC. Choosing one replaces the terminal's class with one fixed function, keeping the ID of an existing single function or allocating one from `next_local`. A terminal with several functions shows its function count. Selections and multi-function authoring have no desktop UI yet.
- **Boxed report.** `QueryResult::Instance` holds a `Box<InstanceReport>` because the report grew with `functions`; serialization is unchanged.
- **Cleanup tests.** The CE-06 "unrouted ground land" row is asserted: with lands 21 and 22 routed, land 23 of the same terminal and net stays a separate island reported by `board.unrouted`, and routing it clears the finding. Separate tests cover a `Requires` constraint (`config.requirement_unmet` until the prerequisite is active, with undo/redo) and `erc.undriven_power_input` (a lone power input, two power inputs, then a power output on the net).

**Deferred.** These are not part of slice 3:

- Availability restricted by configuration fields.
- Simultaneous multi-mode hardware (reported as unsupported if declared).
- MPN availability checks.
- The pin-configuration table UI.
- Firmware generation.

## (f) Scenario walk-throughs

Each step names the commands and the data that represent it. Headless proofs drive these through `Engine::apply`, `Engine::query` and the automation session. UI evidence (CE-C1) and native-model evidence (CE-C6) for full scenarios remain with the CE delivery. Coordinates are millimetres.

### CE-04 — two transistors in one package (slice 1 proof)

**Fixture anchors.** Unit A (id 1) anchors terminal 1 at (−5, 0), 2 at (5, 2.5) and 3 at (5, −2.5). Unit B (id 2) anchors 4, 5 and 6 at the same three offsets.

| Step | Representation |
| --- | --- |
| Author terminals | `library.new_draft` with a component body; `library.edit` with terminals B_A=1, C_A=2, E_A=3, B_B=4, C_B=5, E_B=6, all `Passive` (slice 3: one passive function each). |
| Units A, B | Symbol draft owned by the component, with the fixture anchors above. |
| Six-terminal package and explicit mapping | Package terminals with designations "1"–"6"; footprint with 6 pads pinning the package; binding with mappings 1→"2", 2→"1", 3→"3", 4→"5", 5→"6", 6→"4" listing that footprint; component lists symbol and binding. One `library.publish` of all five drafts. |
| Native model | Package envelope only. `model.attach` is deferred. |
| Place Q1A; Q1B unplaced | `component.place { unit: A, position: (10, 10) }` → `InstancePlaced`. `component.inspect` shows A `Placed`, B `Unplaced`, finding `instance.unit_unplaced` (blocks `ElectricallyResolved`). |
| Place B elsewhere | `symbol.place_unit { unit: B, position: (30, 10) }`. `pin_position`: 1 (5, 10), 2 (15, 12.5), 3 (15, 7.5), 4 (25, 10), 5 (35, 12.5), 6 (35, 7.5). |
| Two circuits, one board placement | `schematic.connect` from A's and B's terminals to two other fixture instances; `instance.set_physical`; `layout.sync` → exactly one `Placement` for Q1. Inspect: `occurrences = 2`, `board_placements = 1`, `bom_quantity = 1`. |
| Exchange unit positions | `symbol.move_unit` A to (30, 10), then B to (10, 10). Endpoints, nets, pads and mapping are unchanged. `pin_position`: 1 (25, 10), 2 (35, 12.5), 3 (35, 7.5), 4 (5, 10), 5 (15, 12.5), 6 (15, 7.5). |
| Reannotate | `instance.set_designator Q7`. Instance, occurrence, unit, terminal, net, pad and track IDs are unchanged; labels become Q7A/Q7B. |
| Unplace an unwired unit | Ambiguity resolved: both units are wired at this point, so the proof first removes unit B's wires with `schematic.disconnect` (ordinary edits), then `symbol.unplace_unit` B succeeds directly. B's physical pads and placement remain. It is then re-placed. |
| Attempt to unplace a wired unit | `symbol.unplace_unit` A → `conflict`; document, revision and history are unchanged. `project.preview_update Change::Schematic { UnplaceUnit }` lists exactly A's wires, affected nets and conflicting tracks. Not applying is cancel. Applying is one undo step, and undo/redo restore and reapply it. |
| Preview whole-component removal | `component.remove` → `conflict`. `Change::Instance { Remove }` lists both occurrences, all wires, the placement and affected tracks. Cancel by not applying. |

### CE-05 — dual amplifier with power unit (slice 1 proof; field independence in slice 2)

**Fixture.** Terminals A+=1, A−=2, Aout=3, B+=4, B−=5, Bout=6, V+=7, V−=8. V+/V− are `PowerInput` (slice 3: `PowerInput` function); the others are fixed. Units A {1, 2, 3}, B {4, 5, 6}, P {7, 8}. Each supply terminal has exactly one anchor, in P. 8-terminal package with an explicit synthetic mapping. Probe components: a one-terminal passive probe and a one-terminal `PowerOutput` supply source.

| Step | Representation |
| --- | --- |
| Two independent instances | U1 and U2 have independent `value` (slice 1) or `properties` (slice 2). Editing one leaves the other's effective values unchanged. |
| U1A + U1P, B unplaced | Place A and P. Inspect shows B `Unplaced` and `instance.unit_unplaced`. |
| Mark B unused; explicit decisions | `instance.set_unit_unused { B, true }`. B's terminals still have `instance.terminal_unaccounted` findings until `instance.set_no_connect { [4, 5, 6] }`. Unused never sets no-connect automatically. |
| Wire U1 | Wire terminals 1, 2 and 3 each to its own passive probe. Wire V+ (7) to a supply source on net VP and V− (8) to a second supply source on net VN, so each net has two terminals. U1 has no `ElectricallyResolved` findings. |
| U2B without U2P | Place only U2's unit B and wire 4, 5 and 6 each to a passive probe. Exact `ElectricallyResolved` findings: `instance.unit_unplaced` for A and P; `instance.terminal_unaccounted` for 1, 2, 3, 7 and 8. Marking A and P unused and setting no-connect on 1, 2, 3, 7 and 8 leaves exactly `instance.required_terminal_no_connect` for 7 and 8, so U2 can never become electrically resolved without P. Power is not duplicated per amplifier unit. |
| Regroup a terminal in a draft revision | `library.draft_from` the symbol and `library.draft_from` the component. In the symbol draft, move terminal 6's anchor from unit B to unit A; in the component draft, change the representation to the pending symbol r2. Publish symbol r2 and component r2 together. `Change::ComponentRevision { [U1], target }` preview reports "U1 terminal 6 moves from unit B to unit A" with its anchor change. Terminal ID 6, its no-connect marker and nets are unchanged. |
| Regroup with a wired terminal | The same plan for U2, whose terminal 6 is wired and whose unit A is now unused, is blocked. It applies only with `RegroupResolution { U2, 6, PlaceUnit { position } }` (which also clears A's unused flag, as `Placed` and `Unused` are exclusive) or `AcceptDisconnect`, which lists the removed wire. If a unit with an occurrence disappears, the plan is blocked until a `UnitResolution` is given. |

### CE-06 — multi-function IO and exposed pad (slice 3 proof; pads from slice 1)

**Fixture.** The synthetic component has these terminals and units:

- 1–20 on the perimeter, plus 21.
- Power unit {4, 8, 12, 16, 21}: 4, 12 and 21 are ground and 8 and 16 supply, with `PowerInput` / `Power` functions.
- Digital unit {1, 2, 3, 5, 6, 7, 9, 10}. Analog unit {11, 13, 14, 15, 17, 18, 19, 20}.
- Terminal 1: functions gpio-in, gpio-out, uart-tx (uart0/tx), adc-in (adc0/ch0). No design default.
- Terminal 2: gpio-in, gpio-out, uart-tx-alt (uart0/tx). No design default.
- Terminal 3: gpio-in, gpio-out, uart-rx (uart0/rx), bus-od (`Bidirectional`, `OpenDrain`, bus0/sda). No design default.
- Terminals 5, 6, 7, 9 and 10: gpio-in and gpio-out, design default gpio-in.
- Terminals 11, 13–15 and 17–20: one fixed analog-in function (`Input`, `Analog`, `Drive::None`).
- Resources:
  - uart0: tx {capacity 1, `required_when_used`}, rx {capacity 1, optional}.
  - adc0: ch0 {capacity 1, optional}.
  - bus0: sda {capacity 1, optional}.
- Constraint `Excludes { (1, adc-in), (3, bus-od) }` is the synthetic incompatible route-group pair.

gpio-in is `Input`/`Digital`/`None`; gpio-out, uart-tx and uart-tx-alt are `Output`/`Digital`/`PushPull`; uart-rx is `Input`/`Digital`/`None`. The default resolution leaves 1–3 unresolved and has no conflicts, so the fixture publishes.

**Physical implementations.** Binding B1 maps all 21 terminals and has footprint pads, three of them on package terminal 21. Binding B2 declares `unavailable: [(3, bus-od)]`. Probe fixtures are one-terminal components with a fixed input, a fixed push-pull output, a fixed open-drain output and a passive terminal.

| Step | Representation and hand-derived expectation |
| --- | --- |
| U1, U2 independent | Two instances with B1. Selections on U1 do not appear in U2's report. |
| U1: uart-tx on 1, uart-rx on 3 | One `instance.select_functions` batch → `FunctionsSelected`; no conflicts. |
| Wire 1 to an input probe | uart-tx is a push-pull output driving an input, so there is no `erc.*` finding on that net. |
| Change 1 to gpio-in | `erc.input_only_net` on that net. Also `config.requirement_unmet`, because uart0 is used (rx on 3) without tx. Expected, and hand-derived from the fixture. |
| Change 1 to gpio-out | The output-driver rule holds: one push-pull driver plus an input, so there is no `erc.*` finding on the net. |
| Connect a push-pull probe to the same net | `erc.driver_conflict`. |
| Select bus-od on 3; net with an open-drain probe and a passive pull-up | No finding. Adding a push-pull probe yields `erc.open_drain_contention`, not `erc.driver_conflict`. This shows that ERC uses the open-drain drive. Then select uart-rx on 3 again. |
| Unresolved terminal | Wire U2 terminal 1, which has no selection or default, to a passive probe: `config.function_unresolved` for U2 terminal 1 plus `erc.unchecked_terminal` on its net. |
| Both TX routes on U1 | Batch {1: uart-tx, 2: uart-tx-alt} → rejected (tx capacity 1); revision and history unchanged. |
| One TX on U2 | {2: uart-tx-alt} on U2 succeeds. |
| Atomic move of U1 TX 1→2 | First `instance.select_functions` {1: uart-tx} on U1. Then submit the batch ordered [2: uart-tx-alt, 1: gpio-out]. Applying it edit by edit would exceed tx capacity after the first edit; the batch succeeds, advancing the revision by exactly one and adding exactly one undo step. Undo restores 1: uart-tx with 2 unselected. |
| Reorder/rename functions | Component r2 reorders terminal 2's functions and relabels uart-tx-alt. The plan shows label changes only, with no blocker, and selections are retained by ID. |
| Remove the selected function | Component r3 removes uart-tx-alt. The plan is blocked for U1 and U2 until a `FunctionResolution` is supplied. Cancel retains r1 and the selection. |
| Implementation lacking the function | Select bus-od on U1 terminal 3. `instance.set_physical { B2 }` on U1 → rejected. On a placed instance, the `Change::Instance { SetPhysical }` plan is blocked. |
| Unrouted ground land | Route two of terminal 21's three lands. The existing physical-island check reports `board.unrouted` for the third, although all grounds share one net. |
| Deferred parts | Paste windows, the generated square footprint (CE-03 seam), MPN choice and the native model. |

### Spec ambiguities resolved

1. **Binding pin direction.** Component pins bindings; bindings record component lineage ([section (a)](#reference-graph-acyclic)).
2. **Footprint choice.** A binding lists allowed footprint variants of its package revision. The instance chooses a binding and one of its footprints by lineage.
3. **ID form.** Typed numeric local IDs replace the spec's illustrative string IDs. `FunctionId` is component-scoped; a `FunctionKey` must pair it with its own terminal.
4. **CE-04 "unplace an unwired unit".** The proof disconnects B first through ordinary commands.
5. **CE-06 open-drain alternative.** It is on terminal 3, not terminal 1, so the open-drain ERC proof uses terminal 3.
6. **Decisions for unused-unit terminals.** These are project no-connect markers on the instance. No-connect never satisfies a required power input.
7. **Wiring an unplaced unit.** It is impossible: a wire endpoint requires its unit to be placed. Unplacing a wired unit goes through the planner.
8. **"Accounted for".** A terminal is accounted for when it is connected (net size ≥ 2) or marked project no-connect (`nets::accounted_for`). Device-NC and reserved terminals need nothing.
9. **Conflicts versus incompleteness.** Capacity, exclusion and availability conflicts are rejected invariants. A missing prerequisite or required role is a finding, so stepwise edits remain possible.
10. **Unused versus placed.** They are exclusive. Placing an unused unit is rejected until the flag is cleared.
11. **NC level.** Device NC is classified on the component terminal. Package-level non-electrical terminals and holes are CE-07.
12. **Purchasing state.** Not computed in these slices, and reported as not computed.
13. **Bare prefix and parse precedence** for `10k` / `10m`, as specified in slice 2.
14. **File version.** It stays 1. Older development files are refused as malformed.
15. **`component.new`.** Served by `library.new_draft` with a component body.
16. **Symbol reuse.** Symbols are owned by one component lineage in these slices ([open questions](#open-questions-and-deferrals)).

## (g) Test plan

Expected values are hand-derived from the synthetic fixtures above. Tests drive public commands and queries (`Engine::apply`, `Engine::query`, `Session::dispatch`) and assert observable state. Implementation details are not asserted.

### Slice 1

- **Publication:**
  - Atomic five-definition publish.
  - Any single blocking finding publishes nothing, and revision, history and drafts are unchanged. Cases: missing mapping; an anchor for an unknown terminal; a binding of another lineage; a footprint of another package revision; a dangling reference; a reused retired ID; a local ID at or above the revision's own `next_local`; a component listing two revisions of one symbol lineage; a component listing two revisions of one binding lineage; a binding listing two revisions of one footprint lineage.
  - A stale base gives `conflict`.
  - Decode rejects a file whose published revision has a `Publish`-blocking finding, and reopens one whose revision has only advisory findings.
- **Drafts:**
  - An empty component draft and a contradictory finite draft survive `encode`/`decode` and the recovery round trip unchanged.
  - NaN and oversize drafts are rejected.
  - `component.check` with `Draft` and with `Revision` targets reports the findings of exactly that target, with `blocks`, and nothing is stored as a stamp.
- **Staged completion:**
  - A component published with no implementations can be placed.
  - `layout.sync` skips it, with finding `instance.no_physical_choice` blocking `BoardPlaceable`.
- **CE-04 and CE-05:** the walk-throughs above, with exact counts, IDs, positions and finding sets. Undo/redo of each mutation, and save/reopen equality of all IDs, pins, unit states and markers.
- **Ported update tests:** the existing `updates/tests.rs` cases (geometry change preserves copper, mapping change lists track removal, label-only warning, unrelated definition blocked, locked copper, newly device-NC). Also a choice that is no longer offered, blocked until a resolution is supplied.
- **Unit resolution:** a representation-lineage change is blocked for an instance with an occurrence and for one with only an `unused_units` entry, even when the target symbol has a unit with the same numeric ID; each applies with explicit `UnitResolution`s. A same-lineage revision removing an unused unit lists the flag's removal without blocking.
- **Regroup:** a terminal moved into an unplaced unit while wired blocks the plan; each `RegroupAction` applies with the expected wires and occurrences.
- **Atomic failure and history:** stale revision, malformed JSON and unknown IDs on every new command leave revision and history unchanged.
- **ERC port:** the existing unconnected-input test becomes a completeness test asserting `instance.terminal_unaccounted` and no `erc.*` finding for the lone terminal.
- **Transport:**
  - The session test places a component and inspects it through JSON.
  - `schemas()` includes the new operations, `Query` and `EditorFile.drafts`.
  - MCP `tools/list` includes `project_query`, which carries `component.inspect`, `component.check` and `component.check_drafts`.
  - The CLI and MCP results, including query reports, equal the direct engine result for the same request sequence.
- **Native integration:** the existing native build and regeneration tests are ported to resolved definitions, and the package-preview test is updated.
- **Desktop:** the existing editor unit tests are ported (commit once, template load is undoable, draft edits keep the board model key). Manual inspection confirms unchanged capability. This is not CE-C1 evidence.

### Slice 2

- **Quantity:** the moved tests still pass. `scripting` tests pass against `crate::quantity` with `Script.units` ported to `UnitSet`; no `UnitDefinition` remains.
- **Parsing:**
  - `10k`, `10 kΩ` and `10000 Ω` give `si = 10000`.
  - `10 µF` on resistance is rejected, and the committed value, revision and history are unchanged.
  - `0` gives `Assigned(0)` with `origin = Assigned`, distinct from `Unset` (`origin = ExplicitlyUnset`) and from `Inherit` (`origin = ComponentDefault`).
  - Bounds, unknown enum choice and text length are rejected.
- **Tolerance:** `±5%` is minus = plus = 0.05. `−20%/+80%` on 10 µF gives the interval 8 µF – 18 µF.
- **Kind identity:** two kinds with equal dimension are not interchangeable. A field revision that changes its kind blocks the update until a `FieldResolution` is supplied.
- **Independence and persistence:**
  - Two instances have independent overrides.
  - A field label change keeps the assignment.
  - Save/reopen preserves `si`, `display_unit` and origin.
  - Undo/redo of the batch.
- **Required fields:** a missing required field blocks `ElectricallyResolved`.
- **Transport:** `instance.set_properties` via the session returns the same `effective` values as the engine.

### Slice 3

- **Publication:** rejects `Input`+`PushPull`, `Output`+`None`, `Power` signal with a non-power direction, duplicate function IDs, a `design_default` not in the list, an unknown resource or role, `DeviceNc` with functions, a `FunctionKey` whose function belongs to another terminal, design defaults that together exceed a role's capacity, and a binding marking a fixed function or design default unavailable.
- **CE-06 cases:** every row above with the exact finding codes and objects.
- **Capacity rejection** leaves revision and history unchanged. The ordered batch move is one revision step and one undo step.
- **Update preview:** reorder/rename is retained, and removal is blocked until resolved.
- **Availability:** rejected for direct `set_physical`, blocked in the plan, and never reported as a `BoardPlaceable` finding.
- **Wiring rules:**
  - A device-NC terminal cannot be wired.
  - A project no-connect cannot be set on device-NC.
  - A wired terminal cannot be marked no-connect, and a no-connect terminal cannot be wired; both are rejected with the same `instances::check_instance` diagnostic.
- **ERC baseline:** passive-only nets produce no findings. An unresolved terminal gives `erc.unchecked_terminal`.
- **Persistence:** save/reopen preserves arrays, IDs and selections. The JSON schema exposes the structured function arrays.

### Gate

Each slice ends with `cargo xtask ci` passing: fmt, layers, ui-containers, strict Clippy, native tests and wasm compilation. Slice 2 also runs `cargo xtask wasm-scripting`. Results are recorded in [editor evidence](editor-evidence.md) and the [Rhai proof](part-library-design.md#rhai-integration-proof) with date and toolchain.

## Limits

| Item | Limit |
| --- | --- |
| Published revisions per kind | 4096 |
| Drafts | 64 |
| Publish set | 32 |
| Terminals per component | 256 |
| Units per symbol | 64 |
| Package terminals | 512 |
| Pads per footprint | 1024 |
| Footprints per binding | 16 |
| Fields per component | 64 |
| Units of measure per kind | 32 |
| Enum choices | 128 |
| Functions per terminal | 32 |
| Resources per component | 64 |
| Roles per resource | 16 |
| Constraints per component | 256 |
| Unavailable functions per binding | 1024 |
| Edits per `instance.select_functions` | 256 |
| Instances | 4096 |
| Occurrences | 16384 |
| Edits per batch | as listed per command |
| Labels and names | ≤ 256 bytes, printable |

These are admission limits, not measured performance.

## Open questions and deferrals

| Item | Status | Owner |
| --- | --- | --- |
| Cross-component symbol reuse | Deferred. Symbols and bindings belong to one component lineage, so the spec's "reuse an existing symbol where suitable" is narrowed to reuse within a lineage and to forking. Sharing a symbol between components needs a terminal-mapping layer between the symbol's anchors and each component's terminals, with its own update rules. No accepted scenario requires it. | Revisit with `library.duplicate` (CE-01) or shared library folders |
| Collision-free `DefinitionId`s across projects | Deferred. The opaque newtype keeps the representation replaceable. | Shared library folders |
| Intra-crate module dependency table | Not enforced by `cargo xtask layers`, which checks crates. Until a module-level check exists, the [module map](#d-module-map) is a reviewed contract. A follow-up could extend `xtask layers` to reject `use crate::` edges outside the table. | Slice 1 review |
