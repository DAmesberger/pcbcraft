# Pcbcraft part library design

Specification baseline, updated 30 October 2026. The component-editor workflows and seven acceptance scenarios below record the user-approved direction, including basic multi-unit symbols and structured multi-function IOs. Proposed command names and synthetic fixture dimensions are implementation/test-design choices. Specified behavior is not evidence of implementation; the Rhai proof and [editor evidence](editor-evidence.md) describe the current subset.

The goal is **one understandable component workflow over reusable, precisely defined data**. Users should be able to create or select a complete component without managing several unrelated library systems. Internally, the model must distinguish electrical function, schematic representation, physical terminals, PCB geometry and purchasing information.

## What existing tools teach us

KiCad already embeds schematic symbol definitions, supports deliberate library updates and comparisons, and connects schematic symbols to board footprints through stable instance identifiers. Project independence and stable identity are therefore useful established practices, not missing features we should claim to invent. [KiCad schematic documentation](https://docs.kicad.org/10.0/en/eeschema/eeschema.html).

Altium documents a unified component model with schematic and footprint models, custom pin mapping and multiple footprint options. A coherent authoring workflow need not imply that one symbol always binds directly to one footprint. [Altium component editing](https://www.altium.com/documentation/altium-designer/components-libraries/single-component-editing), [placed component models](https://www.altium.com/documentation/altium-designer/schematic/placed-components).

LibrePCB explicitly separates symbols, components, packages and devices, uses stable references and allows different footprints for a package. That is a useful public design reference. [Library concept](https://librepcb.org/features/library-concept/), [library elements](https://docs.librepcb.org/_branches/master/librepcb/quickstart/create-library-elements/).

Pcbcraft should combine these useful ideas with Solidcraft's UI, explicit revision handling and geometry-aware update previews. This proposal is based on public documentation; it does not reuse the implementations of those EDA products.

## Domain model

| Definition | Owns | Does not own |
| --- | --- | --- |
| Component definition | Electrical terminals, structured function alternatives and resource constraints, property-field references | PCB land dimensions or a distributor SKU |
| Symbol | Drawing, text, unit definitions, pin anchors and association to electrical terminal IDs | Independent electrical connectivity or a hard-coded package choice |
| Package | Body and physical terminal arrangement, terminal IDs/designations and mechanical datum | One universal PCB land pattern |
| Footprint | Pads, drills, mask/paste openings, courtyard, silkscreen, assembly graphics and terminal associations | The component's electrical pinout |
| Device binding | Exact component/package revisions and explicit terminal mapping | An inferred mapping based on similar names or pad count |
| Manufacturer part | Manufacturer, complete MPN, binding, rated specifications and source information | Live price/stock or the schematic position of an instance |
| Component instance | Project identity, designator, typed property values, pin-function selections, selected symbol/binding/footprint and optional manufacturer part | A mutable reference to whichever library version is currently installed |
| Symbol occurrence | Instance/unit reference, sheet, position and orientation | A second BOM item, physical placement or independent copy of electrical terminals |

A component may have several symbol representations. A package may have reflow and hand-soldering footprints. A device binding establishes the electrical meaning of the package terminals. An instance chooses one compatible footprint and a schematic representation. Manufacturer information may be added later without changing the logical circuit.

The distinction between package and footprint matters even in the first edition. A larger hand-soldering land pattern does not enlarge the physical component body. The 3D model is registered to the package's mechanical datum; each footprint records how that datum is placed relative to its layout origin. Model offsets and instance placements have explicit parameters and coordinate frames, not hidden corrections. Library snapshots pin the native model revision, its parameter interface and required assets. Package dimensions and model parameters have explicit bindings so the native `.solidcraft` assembly remains recomputable; the live 3D view uses the same Solidcraft engine model. Changes to a model parameter interface require an update preview just like pin-mapping changes.

Not every definition needs a sophisticated independent editor. In the first edition, authoring a footprint can create its package/terminal definition in the background. A user can later choose an existing package to create another footprint. Keep the model sound while keeping the normal interaction short.

## One authoring workspace

The entry point is **New Component**, opening an empty recoverable draft without requiring a component category, package or template. Drawing, selecting reusable definitions and invoking a generator are optional actions inside the workspace. Duplicate is a separate action with explicit new identity and retained provenance. The workspace uses Solidcraft's shell and property controls:

1. **Symbol:** draw the representation and its units; edit a terminal table with structured function alternatives, directions and visible pin anchors. Reuse an existing symbol where suitable.
2. **Footprint:** draw or generate pads and outlines; set drills, courtyard and mechanical origin. Reuse a package or footprint where suitable.
3. **Mapping:** show symbol and footprint together, cross-highlight selections and edit the terminal mapping table. List missing, duplicate or prohibited mappings immediately.
4. **3D:** reuse or author a native Solidcraft parametric model through the Solidcraft engine, align it to the mechanical datum, and preview both board sides. Simple dimensioned envelopes use the same engine. Vendor STEP models can be imported through Solidcraft as an additional option; Pcbcraft does not own a separate 3D modelling system.
5. **Details:** author/select reusable property fields and defaults; add optional manufacturer/MPN, typed specifications, datasheet and distributor records, source/license information and descriptive search metadata.

The workflow supports staged completion: a component can be published for schematic use before its footprint is chosen. Board placement requires a resolved physical implementation. Advanced reuse is available within the same workspace. Search results show available symbol, footprint and 3D previews, specifications and separate completeness states; absence of a physical choice is visible.

Do not label a newly imported/generated part as verified merely because it parses. Distinguish structural validation, provenance and actual datasheet review. A missing 3D model can use a clearly identified envelope; missing electrical mapping cannot be silently approximated.

## Generic design and exact parts

Support both common entry points:

- **Design by requirements:** place a resistor with value, tolerance, power and package requirements, without selecting a manufacturer. Resolve a suitable footprint before board placement. BOM output explicitly identifies unresolved purchasing choices.
- **Design with an exact part:** select a known manufacturer's MPN and its validated pinout/package binding, with its rated properties.

Choosing an exact part checks the instance's requirements. Changing a resistor from 10 kΩ to 22 kΩ cannot leave an incompatible exact MPN attached without an error or explicit replacement. The component's schematic value, electrical requirements and selected part specifications must not be unrelated editable strings.

Supplier stock, prices and SKUs are external observations, not library identity. Future sourcing refreshes must not modify geometry, pin mapping or a reproducible BOM selection. Automated alternates are postponed; matching a package name and value alone is insufficient evidence of interchangeability.

## Identity, revisions and portable projects

Names, folder locations and reference designators are labels. Definitions, terminals, pads and placed component instances have stable identities. A rename or library reorganization should not break references.

Saved revisions are immutable; edits happen in a draft and produce a new revision. The revision identifies exact dependency revisions and asset hashes. A project stores their full dependency closure: symbols, electrical definitions, packages, footprints, bindings and required 3D assets, with provenance and applicable license information. Deduplicate shared content inside the project.

The project must reopen offline on another machine after the author's installed libraries have been removed. A hash or lockfile alone is insufficient if the referenced asset bytes are unavailable. File paths and online sources can describe provenance but must not be the only copy of required design data.

Start with project-local definitions and optional local library folders. A searchable index is rebuildable; it is not the authoritative source of the project. Sharing a small library should be possible through ordinary files or version control. A hosted registry, accounts and complex dependency resolution are unnecessary for the first edition.

Make editing scope explicit: **this component's project definition**, **selected instances**, or **the source library**. Editing the source library does not update projects. A local change can later be promoted to a library revision or duplicated as an independent definition. Keep the upstream origin and revision so divergence is visible.

## Mapping and connectivity invariants

- Every electrical package terminal maps to at most one electrical terminal. One electrical terminal may map to several physical terminals, but each required physical connection remains independently checkable.
- Every package terminal is accounted for: connected, device-declared NC/reserved, or explicitly classified as non-electrical/optional where appropriate. No unmapped terminal disappears from validation.
- An intentionally unconnected project pin is different from a manufacturer-designated NC pin. The latter may prohibit a connection; the former records a circuit decision.
- Names such as GND, VCC or EP do not create an implicit connection. Equal labels are not proof of interchangeable pins or internal connectivity.
- Multiple copper lands can belong to one package terminal. Their disconnected copper regions are not considered connected solely because they share the terminal or net.
- An exposed pad is an explicit terminal when electrically meaningful. Copper, paste windows and thermal vias are separate geometry. A mechanical mounting hole need not pretend to be an electrical pin.
- PCB connectivity is proved through the supported copper model. Internal chip connections do not automatically satisfy external routing obligations or turn the chip into a jumper. Explicit jumper behavior can be designed later.

These distinctions prevent a board with several unconnected GND pads from appearing complete just because all of them map to GND. They also allow split paste apertures without inventing extra electrical pins.

The first complete component editor includes basic multi-unit symbols, explicit power units and structured multi-function IOs. Units share one component instance and physical implementation. Hidden power pins, automatic gate/pin swapping and a universal pin-equivalence system remain deferred. These requirements supersede the earlier single-unit-only scope.

## Updates must explain what changes

Library updates and schematic-to-PCB updates are related transactions, with an explicit preview and one undo step. The preview is tied to the current project revision; it must be recomputed if the project changes before application.

| Change | Required handling |
| --- | --- |
| Description, source metadata or human-readable name | Show the metadata change; retain identity and connectivity |
| Symbol graphics or pin anchor positions | Retain electrical terminal associations; show affected wires and reconnect their presentation deliberately |
| Electrical type, pin function or binding | Show old/new terminal-to-net relationships; require explicit mapping for ambiguous changes and rerun ERC |
| Instance pin-function selection | Show old/new function, direction and resource constraints; retain terminal/net/pad identities and rerun ERC without silently rerouting or renaming nets |
| Field definition, default or generator recipe | Show effective values and affected geometry; preserve explicit overrides or require a resolution; pin the accepted dependency revisions |
| Pad position, shape, drill or footprint origin | Show geometry and affected tracks, holes, clearances and model placement; invalidate affected DRC/pour results |
| Package/body/model dimensions | Show changed mechanical extents and alignment; regenerate the live 3D scene/export |
| Manufacturer part selection | Compare binding and rated specifications with project requirements; flag unresolved incompatibilities |

A useful preview says: **“U3 pad 2 moves 0.40 mm; its attached track will need repair. Pad 3 changes from OUT to GND.”** It does not merely say “update component.”

Preserve existing routing where valid. If a pad moves away from a trace, retain and flag the detached trace; do not pretend the route was repaired or silently delete copper. Do not silently retag routed copper onto a different net when a mapping changes. Show the affected connections and require an explicit repair decision. Cancelled or failed updates leave the previous working project intact.

Stable IDs do not determine whether a change is electrically safe. Correcting a printed terminal number, changing its electrical meaning, and changing its mapping are separate operations. A new library revision cannot disguise a pinout change as a harmless rename.

## First-edition test cases

These cases should shape the data model before extensive UI work:

1. Rename a component/library and reannotate U3 to U7; symbol, board placement, nets and routes keep the same component identity.
2. Switch a two-terminal part to a compatible hand-soldering footprint; terminal mapping is retained, pad moves are visible, affected tracks are flagged.
3. Use two components in the same physical package with different pinouts; reuse the footprint without inferring that their bindings are interchangeable.
4. Change a symbol's appearance or pin positions; preserve intended connectivity and explicitly update wire geometry.
5. Give an IC several GND terminals and an exposed pad; leave one physically unrouted and require the connectivity check to catch it.
6. Model one terminal with multiple copper regions and separate paste windows; do not invent electrical connectivity from membership alone.
7. Change a generic passive's value after choosing an MPN; surface a requirements mismatch.
8. Move a footprint origin or model offset; preserve the intended package placement or explicitly show the world-space change, including on the bottom side.
9. Update an installed library, remove it, and reopen an existing project offline; retain the project's original definitions and model assets.
10. Attempt an ambiguous pin remapping; fail atomically, preserve the old design, and return a structured explanation to the UI and agents.

Basic multi-unit authoring and terminal-preserving alternate-symbol switching are included in the acceptance scenarios below. Assembly variants, cross-sheet hierarchy and automatic gate/pin swapping remain separate later work.

## Import and provenance

Component import should consume public format specifications, retain source identifiers/provenance, and report unsupported constructs. Never silently flatten a complex symbol or footprint into a plausible but electrically different one. A small supported import subset is acceptable after native authoring works.

Code licensing and library-data licensing are distinct. KiCad publishes its libraries under CC-BY-SA 4.0 with a design-output exception and separate conditions for redistributing library collections. Treat imported assets according to their actual provenance; do not assume they inherit Pcbcraft's code license. [KiCad library license](https://www.kicad.org/libraries/license/).

## Accepted authoring contract

This section specifies target behavior; [component model](component-model.md) defines how its definitions, revisions, instances, fields and terminal functions are represented in the first three implementation slices. All three slices are implemented. Slice 1: revisioned definitions, drafts, exact references and multi-unit symbol occurrences replace the monolithic `Part`, with headless CE-04/CE-05 proofs. Slice 2: revisioned quantity kinds and typed fields, component field selections with defaults, and typed instance values with origins replace the free-text instance value, with headless parsing, update and persistence tests. Slice 3: terminal function arrays with direction, signal and drive, device-NC and reserved terminals, shared resources with role capacities and constraints, per-instance function selections, per-implementation availability and function-aware ERC replace `PinKind`, with headless CE-06 function tests. The single-unit authoring UI with one fixed function per terminal and without field authoring or a pin-configuration table, derived fields and the numeric Rhai pad proof do not yet implement the rest of this contract. Fixture values below are original synthetic test data, not recommended land patterns or verified manufacturer specifications.

### Definition relationships and completeness

Independent definitions have stable IDs and immutable revisions. References identify both, including field schemas, symbols, packages, footprints, bindings, generator recipes, native models and required assets. A footprint variant is a separate definition with its own revisions, grouped with other alternatives for the same package. A category tree organizes discovery but does not infer electrical compatibility or inherit fields. Reusable field sets use explicit references; changing a set creates an update to review.

One component instance owns its designator, property assignments, pin-function selections and physical choice. Symbol occurrences refer to that instance and one unit within the selected representation. Package choice is reached through an explicit device binding; a footprint references its package. Changing the symbol presentation does not select a different footprint. One physical instance produces one board placement and one BOM quantity regardless of its symbol-unit count.

The editor derives the following states and lists their blocking findings; they are not an automatic verification stamp:

| State | Requirements and permitted actions |
| --- | --- |
| Recoverable draft | Empty or unfinished definitions can be saved, recovered and undone. Invalid geometry/expressions are retained as visibly invalid draft input within resource bounds; they cannot replace valid evaluated geometry. |
| Schematic-usable definition | Stable electrical terminals; structurally valid selected symbol representation and unit coverage; resolvable field/function definitions. Package, footprint, 3D and MPN may be absent. Publish and place in a schematic. |
| Board-placeable instance | Exact binding and footprint selected; complete physical-terminal classification/mapping; supported valid pad/hole geometry. An explicitly approximate envelope may stand in for a missing detailed model. Unresolved electrical requirements remain findings. |
| Electrically resolved instance | Required units/terminals accounted for, active pin functions resolved, declared configuration constraints satisfied and required circuit fields supplied. This does not imply that all board routes or board-level checks pass. |
| Purchasing-resolved instance | An exact manufacturer part satisfies the instance's typed requirements and selected implementation. Distributor offers are optional. Generic BOM export remains possible with unresolved rows clearly marked. |
| Manufacturing readiness | Separately requires the board-level manufacturing checks/output contract. Component completeness alone never declares a board ready for fabrication. |

Reusable fields, packages and footprints may be published independently after validation of their own dependencies. A component publication may omit physical implementations. Draft persistence and publication are distinct operations; no incomplete publication should masquerade as a valid physical implementation.

### Drawing and editing behavior

Symbol graphics include lines/polylines, rectangles, circles/arcs and text. Pins have stable terminal references, position, orientation, length, label placement and electrical adornments. Pin direction and function are structured data; drawing an arrow or typing a label does not change them. Text may reference fields, designators or selected function names without becoming the authority for those values.

Footprints include rectangular, circular, oval and rounded-rectangle pads, pad rotation, circular plated drills, non-plated mounting holes, mask/paste apertures, silkscreen, assembly graphics and courtyard. Assembly body outlines and the courtyard are separately authored geometry; neither is silently inferred from pad bounds. Ordinary paste windows can be independent rectangles belonging to a pad, without inventing electrical terminals. Slots and arbitrary custom copper primitives remain outside this initial toolset.

Both canvases provide a visible configurable grid, snapping, pan/zoom, individual and box selection, numeric properties, move/rotate, duplicate and delete. A line uses start/end clicks; a polyline ends with an explicit finish action; a rectangle uses opposite corners; a circle uses center/radius; an arc uses center/start/end with visible sweep; a pin/pad/text tool previews before placement. Escape cancels the current gesture without a document edit. Completed gestures are one undo step. Input focus keeps typing shortcuts in the field. Exact keyboard bindings must follow the inspected Solidcraft navigation/control conventions rather than introducing a conflicting shortcut system.

Select graphics independently of terminal selection. Duplicating a graphic allocates a new graphic ID; duplicating a pin or pad must explicitly reuse a terminal or create/map a new terminal, never silently infer it from a copied label. Rotation transforms anchors and graphics together and preserves identity. Switching symbol, footprint, mapping and 3D views retains the active component and resolves selections through IDs. Mapping cross-highlights a terminal, its symbol anchors, physical terminals and all associated lands; each land remains individually selectable.

The editor distinguishes move-geometry from change-origin/datum. A coordinate rebase that preserves world placement and an intentional body/model displacement are separate actions with old/new previews. Hidden drawing layers cannot be accidentally edited through normal picking. Validation messages identify the offending object and offer navigation to it.

### Generic fields, values and references

A reusable field definition declares an ID/revision, label, value type, quantity-kind/unit references where applicable, bounds or enum choices, optional default, required-at stages, formatting and optional derived expression with explicit input dependencies. Core supported types are quantity, bounded integer, boolean, enum, text, range/tolerance and structured reference. The implementation knows these types, not categories such as resistor or capacitor. Users can author fields and units through forms; scripting is optional.

Component definitions select fields and may supply explicit defaults within their constraints. An instance stores overrides keyed by field identity; it displays whether a value is inherited, assigned, calculated or unset. Zero and unset are distinct. Editing from the schematic or PCB modifies the same instance. Presentation units and significant digits do not change the stored physical meaning. Quantity kinds distinguish meanings even where dimensions coincide; a unit symbol or display label alone does not establish kind identity.

Requirements and manufacturer specifications are separate typed records. Requirements declare comparison meaning, such as exact nominal value, minimum rated voltage or maximum tolerance. Selection checks these declared comparisons, not string equality. Symmetric/asymmetric tolerance carries explicit relative or absolute meaning; `5%` must not become the number 5 without percent semantics. Invalid field text remains an uncommitted edit with a diagnostic. Deliberately changing a valid requirement so a selected MPN no longer fits may be committed with an explicit unresolved-purchasing finding; no automatic MPN substitution occurs.

Renaming a field retains assignments. Adopting a new schema/default revision previews effective value changes. Explicit instance overrides are preserved if valid; removing a populated field, changing quantity kind or narrowing constraints requires explicit resolution. A selected MPN's specifications never change just because an instance requirement changes. Derived-field cycles and missing inputs produce diagnostics and block applying the dependent result, without altering the last committed values.

Reference records have a stable ID, kind, title, URL, optional document revision/date and optional attachment hash/license/provenance. Definitions/packages can link general references; exact parts own their particular datasheets. Distributor offers reference exact manufacturer parts and record distributor identity, SKU, product URL and optional packaging/order fields. Price/stock observations, if added later, require timestamps. Saving a URL performs no network fetch; links and optional attached bytes are distinguished in offline status. Attachments intentionally included in the snapshot remain available without the network.

### Multi-unit symbol semantics

A representation contains stable unit IDs and terminal-anchor assignments. Units partition terminal presentations; the initial editor requires each electrical terminal to have one owning anchor in that representation. Shared supplies appear once in an explicit power unit, rather than as duplicated hidden pins. Repeated graphical anchors for the same terminal require a later explicit alias model and must not be silently imported as distinct terminals.

`Q1A` and `Q1B` are independently placed symbol occurrences of `Q1`. Unit suffixes are display labels; reannotation changes the shared component designator without changing unit, terminal, net or pad IDs. The component browser lists placed, unplaced and explicitly unused units. A unit marked unused does not automatically mark its terminals NC: each connection decision remains explicit, including required supply connections.

Moving a unit changes only its schematic placement. Unplacing a unit preserves the component and its physical placement; connected wires require a reviewed disconnect operation with affected nets/copper shown. Removing the whole component is a distinct action listing all units, wires, board placement and affected copper. Applying either destructive edit is one transaction and cancellable before commit. An unused-unit annotation cannot conceal required electrical obligations.

Alternate symbol representations may change drawings or unit arrangement while retaining the component's electrical terminals. Switching representation previews anchor/wire changes and requires explicit unit-placement correspondence when grouping differs. Electrical identity cannot be remapped by unit order or display name. The first-release scenarios run on one sheet; cross-sheet unit placement is an extension when hierarchy is implemented.

### Native multi-function IOs

Each electrical terminal declares a **functions array**, not a slash-separated display string. A function record has a stable ID scoped to the terminal, label, electrical direction, signal kind, and drive behavior. Optional references identify a peripheral resource and signal role, availability constraints and a documented hardware selector. Fixed-function terminals use one entry. Function labels, physical pin numbers and terminal IDs are separate values.

Illustrative synthetic terminal data (not a real MCU pinout):

```json
{
  "id": "io-a",
  "name": "PA0",
  "functions": [
    {"id": "gpio-in", "label": "GPIO input", "direction": "input", "signal_kind": "digital", "drive": "none"},
    {"id": "gpio-out", "label": "GPIO output", "direction": "output", "signal_kind": "digital", "drive": "push_pull"},
    {"id": "uart-tx", "label": "UART0_TX", "direction": "output", "signal_kind": "digital", "drive": "push_pull", "resource_role": {"resource": "uart0", "role": "tx"}},
    {"id": "adc-in", "label": "ADC0_IN0", "direction": "input", "signal_kind": "analog", "drive": "none", "resource_role": {"resource": "adc0", "role": "channel0"}}
  ]
}
```

Direction supports input, output, bidirectional, passive, power input and power output. Drive behavior distinguishes none, push-pull, open-drain and open-source where applicable. Device-declared NC/reserved remains a terminal classification; a project no-connect marker is a separate design decision. Bidirectional bus functions and configurable GPIO modes must not be reduced to a generic passive pin. Invalid direction/drive combinations are rejected on definition publication.

A component instance selects at most one active function per terminal, keyed by stable terminal/function IDs. A fixed-function terminal resolves automatically. A programmable terminal without an explicit selection or library-declared design default is unresolved; a reset-state description alone is not a design default. Unresolved terminals produce a configuration finding, and ERC must not pretend that every alternative is active simultaneously or silently choose one. Simultaneous hardware modes beyond the declared single-choice model must be reported as unsupported rather than flattened.

The library editor provides a function subtable per terminal with add/remove/reorder and direction/role controls. Instance properties provide a pin-configuration table with terminal identity, package pin, selected function, direction, net and diagnostics. The symbol can show the physical/logical pin name plus the selected function; the inspector reveals all alternatives. Selecting a function updates every view of that same instance, not the source definition or other instances. Array order and label edits do not change a selection's identity.

Peripheral resources and role identities are declared data. The initial constraint vocabulary includes role-capacity limits and explicit required/excluded selections, plus availability restricted by the selected implementation and declared configuration fields. For example, a UART TX role may permit one active pin, while RX is optional; TX/RX pairing must never be universally inferred from their names. Where hardware requires compatible route groups, declare the exclusions or prerequisites explicitly. Resource constraints are scoped per component instance, not across unrelated MCUs. Incomplete library coverage must be labeled unchecked, not hardware-validated.

Changing selected function previews the old/new direction, affected ERC findings and resource constraints. It preserves terminal, net, pad and routing identities; it never renames a net or rewires copper. Structurally invalid selections and declared resource conflicts are rejected atomically. A structurally valid selection that causes a circuit-level ERC violation can be committed with the violation visible, just like other valid circuit edits. A library update removing the selected function or changing its meaning requires explicit resolution; cancel retains the old pinned revision and selection. Package/MPN changes must revalidate availability. These records describe hardware intent; firmware register/code generation is outside the initial scope.

### Generator ownership and native model relationships

A generator recipe pins source, execution API/version, input-field references and required resources. The snapshot includes all bytes required for offline regeneration, plus the last accepted generated definitions. The generator runs without document side effects and returns a revision-tagged preview. Native geometry definitions and dependencies use Solidcraft's parameter/expression facilities; Rhai orchestrates supported definition generation and component rules, not a competing geometry evaluator.

Generated objects have stable generation keys mapped to persistent object IDs. Manual edits to generated objects are recorded as explicit property overrides; hand-authored objects remain distinct. Regeneration computes a new baseline, reapplies valid overrides and displays both. If a generated object disappears, changes type or invalidates an override, applying is blocked until the user discards the override, retains/detaches the object as manual geometry, or supplies an explicit correspondence. No silent overwrite or name-based electrical remapping is permitted. Reset override is a visible undoable action.

Generators may add/remove pads only with explicit stable-terminal associations and an impact preview. Existing routing obligations are preserved or listed as affected. The current `library.generate_pads` proof only rearranges existing pads and does not satisfy recipe persistence, overrides or this add/remove contract.

Models belong to packages, aligned through explicit footprint-datum and model transforms. Native models retain parameter interfaces and dependencies; supported supplier geometry is imported through Solidcraft with unsupported features reported. Changing footprint land dimensions does not enlarge the package body or replace its model. Repeated instances share model definitions with independent placements. All seven scenarios exercise top/bottom alignment, snapshots and native reopen. Detailed appearance editing and photorealistic rendering remain a separately scoped enhancement; generic envelope quality is always labeled.

### Command and transaction coverage

The following names are **proposed command families**, not claims that endpoints already exist. Implement typed inputs/results and generate transport schemas from them. Use `Result` diagnostics, stable IDs, expected document revision, bounded inputs and the one engine for every mutation. Preview results identify their document and dependency revisions; applying stale previews fails unchanged.

| Family | Input/result and observable contract |
| --- | --- |
| `component.new`, `library.duplicate` | Create an empty draft or explicitly fork an exact definition with new identity/provenance. Return draft identity/completeness. No template/category is mandatory. |
| `library.edit`, `library.publish` | Edit typed fields, terminals/functions, graphics, units, mappings or references in a draft; publish validated immutable revisions and dependencies. Return findings and exact references; leave existing instances unchanged. |
| `component.place`, `symbol.place_unit`, `symbol.unplace_unit` | Create/reuse one component instance and place/unplace a selected unit. Return instance/occurrence IDs. Connected removals require reviewed consequences. |
| `instance.set_properties`, `instance.select_functions` | Apply typed value/function assignments, including a multi-pin configuration in one operation. Return normalized/effective values and findings; reject invalid references, dimensions or declared resource conflicts. |
| `instance.preview_implementation`, `instance.preview_symbol` | Compare exact binding/footprint or symbol choices, mappings, anchor/geometry changes and affected copper. Return an immutable impact plan; do not mutate. |
| `library.preview_update`, `generator.preview` | Compare pinned revisions or regenerate with parameters and overrides. Return value/mapping/geometry/function/asset changes and unresolved decisions. |
| `change.apply` | Validate plan revisions and explicit resolutions, then commit all affected definitions/instances as one undo step. Preserve valid copper; ambiguous remaps and unresolved destructive effects block application. |
| `model.attach`, `reference.attach` | Stage validated model/reference data through the engine. Import/read/download and opening external URLs are explicit external effects outside document undo; programmatic requests never open dialogs. |
| `component.inspect`, `component.check` | Return exact references, effective properties, unit/function state and structured findings with source IDs. Read-only calls do not create undo entries. |

Rhai values do not escape into persistence or transport schemas. Keep quantities, library definitions, symbol/footprint geometry, bindings, instance configuration and update planning in cohesive core modules; retain filesystem effects in I/O and native modeling in the Solidcraft adapter. No new crate is required just to mirror every noun. Final per-command schemas accompany implementation of each family; the `part.*` commands have been replaced by `library.*`, `component.*`, `instance.*` and `symbol.*` (see [component model](component-model.md#delivered-by-slice-1)), and `instance.set_properties` exists for typed values ([slice 2](component-model.md#delivered-by-slice-2)); `instance.select_functions` and the other families above remain proposed.

## Component editor acceptance scenarios

All seven scenarios follow the common lifecycle below. Each must pass through the normal UI and equivalent typed commands; a fixture assembled only in code does not establish editor completion. IDs and numeric values in these scenarios are deterministic test fixtures; datasheet review and manufacturing validation are separate.

### Common lifecycle and acceptance evidence

1. **Author:** New Component opens empty. Add terminal/function records and reusable fields; save/reopen an unfinished draft. Draw symbols and declare units, then publish for schematic use before any package is chosen.
2. **Implement physically:** Create/reuse packages, alternative footprint definitions and explicit bindings; add model/datum alignment and documentation. Publish exact revisions independently and make their relationship visible.
3. **Use:** Place instances/units, assign typed values and functions, wire the circuit, select physical choices, place/route the board and inspect 2D/3D. Add exact manufacturer/distributor choices without implicitly altering geometry or nets.
4. **Change:** Perform the scenario-specific property, drawing, generator, package and revision changes. Review affected connections, copper, mechanical transforms, configuration and purchasing status before application.
5. **Preserve:** Cancel a preview, apply it, undo/redo, reject a stale request, save/recover, remove access to the source library, and reopen offline. Generated recipes, attachments and required native assets must be present in the snapshot.

| Evidence ID | Required observable result for every scenario |
| --- | --- |
| CE-C1 | Real screenshots/interaction evidence of authoring and use, including a narrow window and relevant 2D/3D selections. Tools/errors remain reachable; UI behavior follows the shared shell. |
| CE-C2 | Public-command assertions of terminal/unit/pad identity, exact revisions, typed values and instance counts; UI/CLI/MCP operations agree semantically. |
| CE-C3 | Hand-derived positions, dimensions, mapping tables, intended nets and deliberate violations. Export/import agreement alone is not the oracle. |
| CE-C4 | Preview is read-only; cancellation, malformed input, stale plans, conflicts and failed scripts preserve document/revision/history. A successful logical change is one undo step; redo restores the complete result. |
| CE-C5 | Atomic save/recovery and offline reopen preserve definitions, selected functions, field meanings, recipes/overrides, source provenance and required asset bytes. Missing required bytes fail explicitly. |
| CE-C6 | Native save/reopen in compatible Solidcraft preserves geometry/placement dependencies. Analytic top/bottom bounds agree; changing exposed dimensions recomputes without Pcbcraft. Selection/camera actions do not rebuild geometry. |

### CE-01 — generic resistor

Author a two-terminal generic component from an empty draft. Select reusable resistance, relative tolerance and minimum-rated-power requirement fields. Draw rectangle and zigzag representations with identical terminal IDs. Place R1 and R2 before choosing physical implementations; assign R1 10 kΩ, ±5%, 0.125 W minimum and R2 22 kΩ. Input `10k` in the resistance field and `10000 Ω` must have the same meaning; `10 µF` must fail without changing the committed value.

Create two explicitly dimensioned synthetic packages and bindings. For package A, create a reflow footprint with 1.0 × 1.4 mm pads centered at x = ±1.2 mm and a hand-solder variant with 1.4 × 1.4 mm pads centered at x = ±1.4 mm. Each variant has its own revision history. Attach one package-A model used by both variants; package B has its own model. Supply a synthetic exact part with nominal resistance 10 kΩ and sufficient tolerance/power specifications, a datasheet record and two distributor offers.

Place R1 on the board and route to it. Switch symbol representation, then footprint variant, then package through explicit previews. Change resistance to 22 kΩ while retaining the exact 10 kΩ part and observe unresolved purchasing. Publish a new footprint revision and update only R1; R2 keeps its pins, values and pinned choices.

**Pass:** symbol changes preserve nets and physical choice; variant change reports each center moving 0.2 mm without enlarging the package/model; affected routing is evaluated using actual new copper geometry, not merely pad-center displacement. Use a separate pad move that leaves a routed endpoint outside copper to prove detached-route reporting. Package change uses explicit mappings. Values remain shared between PCB/schematic, and one instance contributes one BOM quantity. Apply CE-C1–C6.

### CE-02 — non-polarized and polarized capacitors

Create separate definitions that explicitly reuse capacitance and voltage-rating field definitions. The non-polarized fixture is 100 nF, ±10%, with a 25 V minimum-rated-voltage requirement. The polarized fixture is 10 µF with relative tolerance −20%/+80% and explicit positive/negative terminal identities. Draw polarity markings linked to terminal meaning; label changes must not swap terminals.

Give each a physical implementation and a native model with an asymmetric polarity mark. Place and rotate the polarized component on both board sides. Attach an exact part with a synthetic 16 V rating, then raise its instance requirement to 25 V. Publish a corrected symbol-label revision separately from an intentionally reversed terminal binding and inspect both updates.

**Pass:** units normalize correctly; the 10 µF tolerance interval is 8–18 µF; the 16 V part fails the 25 V requirement. Rotation/bottom transforms preserve the explicit positive terminal and model mark. Label correction retains connectivity; reversed binding is an electrical change requiring explicit review. Apply CE-C1–C6.

### CE-03 — inductor and generator ownership

Create an inductor with reusable fields for 10 µH inductance, 1 A minimum rated current and 0.2 Ω maximum DC resistance. These schemas come from library data. Add an optional calculated copper-loss field using explicitly identified current and resistance inputs; its 1 A / 0.2 Ω fixture yields 0.2 W. Missing inputs, wrong dimensions and a deliberate field-dependency cycle must be visible failures.

Use a generator with retained body/land/pitch parameters to create a two-pad footprint. The initial centers are x = ±2 mm. Manually set the right pad's y coordinate to 0.25 mm, recorded as an override. Regenerate with centers x = ±2.5 mm. Then try a recipe revision that removes that right pad or changes its generated type; resolve the resulting override/terminal conflict explicitly. The removal is a deliberate invalid-case probe, not a usable one-terminal inductor.

**Pass:** compatible regeneration changes x while retaining the explicit y override and pad/terminal IDs. Incompatible regeneration cannot silently drop the override or terminal obligation. Resetting the override and undo are explicit. Offline regeneration uses the pinned recipe and inputs; live/native geometric dependencies remain editable through Solidcraft. Apply CE-C1–C6.

### CE-04 — two transistors in one package

Author a synthetic dual-NPN component with terminals B_A, C_A, E_A, B_B, C_B and E_B, with passive ERC behavior unless a more specific supported rule is explicitly declared. Create units A and B, each with three anchors, and one six-terminal package. Declare the fixture mapping explicitly: B_A→2, C_A→1, E_A→3, B_B→5, C_B→6, E_B→4. No mapping is inferred from table order. Attach one six-lead native model.

Place Q1A, inspect unplaced Q1B, then place it elsewhere on the same sheet. Wire two independent circuits and place Q1 once on the board. Move A and B to exchanged graphical positions without swapping their functions or physical pins. Reannotate Q1 to Q7, unplace an unwired unit, then attempt to unplace a wired unit and review the disconnect. Separately preview removal of the entire component.

**Pass:** two symbol occurrences, one component instance, one board placement and one BOM quantity. The explicit six-terminal mapping survives moves/reannotation. Unplaced/unused states do not hide electrical obligations. Unit-only operations preserve the other unit and physical placement; connected removal lists affected nets/copper and can be cancelled. Apply CE-C1–C6.

### CE-05 — dual amplifier with explicit power unit

Author a synthetic dual amplifier with A+/A−/Aout, B+/B−/Bout, V+ and V−. Create amplifier units A/B and power unit P; the eight-terminal package mapping is explicit. Supply pins have one anchor each in P. There are no hidden or duplicated supply connections. Create two instances with different field assignments to verify their independence.

Place U1A and U1P; leave U1B unplaced and observe the unresolved unit state. Mark B unused and make explicit connection decisions for its terminals. Keep supplies connected as required. Place U2B without placing U2P and observe unresolved power obligations. Reassign a terminal to a different unit in a draft representation revision and preview wire-anchor consequences before adopting it.

**Pass:** A/B/P share one package identity per instance; power is not duplicated per amplifier. Unused B never suppresses required supplies or automatically applies NC decisions. Missing P cannot yield an electrically complete instance. A unit-regrouping update preserves terminal IDs and requires placement/anchor resolution where necessary. Apply CE-C1–C6.

### CE-06 — MCU-style IC with multi-function IO and exposed pad

Author an original fixture with 20 perimeter terminals plus electrical exposed terminal 21. Terminals 4 and 12 are separate ground connections, 8 and 16 are separate supply connections, and 21 is an additional required ground connection. The remaining 16 terminals belong to Digital and Analog symbol units; the five supply/ground terminals belong to Power. Bind all terminals explicitly to a generated square footprint. Give terminal 21 multiple copper lands and separate paste windows to test physical connectivity independently of terminal membership.

For perimeter terminal 1 use the structured `PA0` alternatives above. Terminal 2 supports GPIO and another UART0 TX routing alternative. Terminal 3 supports GPIO, UART0 RX and a bidirectional open-drain bus alternative. Declare UART0 TX capacity as one, with RX optional. Include an explicit incompatible route-group pair in the fixture constraints, and a second package/implementation with an unavailable alternate function. All of these are synthetic capabilities; no claims about a supplier MCU follow from this fixture.

Place U1 and U2 with independent configurations. On U1 select UART TX on terminal 1 and UART RX on terminal 3. Wire terminal 1 to an input. Change it to GPIO input and verify an input-only-net finding; change it to GPIO output and verify the output-driver rule. Connect it to another push-pull output to introduce a deliberate conflict. Select the open-drain bidirectional alternative and ensure ERC uses that drive behavior rather than treating it as push-pull; an unresolved or unsupported electrical check remains explicitly incomplete.

Attempt to select both UART TX routes on U1, then select one on U2. The first must fail atomically; the second is independent and succeeds. Apply a batch that moves U1 TX from terminal 1 to terminal 2 without an invalid intermediate commit. Reorder/rename function alternatives in a library revision, then remove the selected function in another revision. Preview both updates. Attempt a package/MPN choice lacking the selected function. Leave one ground land unrouted and verify the physical-connection finding even though all grounds share a net.

**Pass:** function alternatives persist as structured arrays with IDs/directions/roles, and selected IDs survive reorder/rename. Labels and ERC follow the selected alternative; unresolved selections are explicit. Valid function changes preserve wire/net/pad/copper identities. Missing selections, resource conflicts, implementation availability and removed functions yield specific findings/resolutions without hidden substitution. Mux intent does not claim to configure firmware. Regeneration preserves pad identities; disconnected exposed-pad lands remain independently detectable. Apply CE-C1–C6.

### CE-07 — connector with shield and mounting features

Author four signal terminals and an explicit shield terminal. Add two non-plated mechanical mounting holes with no electrical terminal identities. Create ordinary circular plated contact pads and explicitly mapped shield lands. Supply right-angle and vertical package choices with separate aligned models and footprints. Use a repeated-contact generator and record a mechanical datum distinct from the first pad and body center.

Place and route a connector on both board sides. Move synthetic mounting-hole centers from x = ±5 mm to ±6 mm in a new footprint revision. Rebase a footprint origin while preserving world placement; separately request a model displacement and inspect the difference. Switch package orientation through the physical-choice preview. Add manufacturer documentation and distributor offers; verify a missing external URL does not erase the identity or an attached datasheet copy.

**Pass:** holes create neither electrical terminals nor nets; shield connectivity is explicit. Hole-spacing change is 2 mm overall and leaves signal-terminal identity unchanged. Coordinate rebase preserves pad/model world positions, while intentional displacement reports changed bounds. Package switches retain only explicitly compatible mappings. Native top/bottom transforms and offline attachments remain correct. Apply CE-C1–C6.

### Delivery order and implementation status

Design definition references, typed fields, symbol units and terminal-function identities against CE-04–CE-06 before building CE-01 UI. Within component-editor delivery, implement CE-01 through CE-07 in order, with commands and visible behavior developed together. Keep generation, mapping and update validation reusable across cases; no component-category branches are required for their fields. This sequence does not mark the roadmap's remaining foundation/performance gates complete.

All seven full scenarios are pending. [Parts library S2](parts-library.md#s2-as-built) proves the purchasing steps of three of them headlessly; their status on 31 October 2026: CE-01's resistance change from 10 kΩ to 22 kΩ after an MPN is chosen must surface `part.spec_mismatch` with the Part unchanged (pending S2 core rules); CE-02's 16 V requirement against a 25 V rated part must satisfy and the reverse must be a finding (pending S2 core rules); CE-07's reference without a URL keeps its identity and attached copy through publish, save and reopen over JSON (automation test built; the core test is pending with the S2 core rules). The Rhai proof below supplies evidence only for a small subset of CE-03 execution/rollback and CE-C4/C5, using numeric pad output. Existing foundation tests prove additional isolated identity, update and native-model behavior. Neither makes these end-to-end scenarios complete. Cross-sheet hierarchy, automatic pin/gate swapping, firmware generation, live supplier integration and photorealistic rendering have separate scope; their absence must not be hidden by these scenarios.

## Rhai integration proof

The initial [scripting module](../crates/editor-core/src/scripting/mod.rs) embeds Rhai 1.27.0 behind ordinary Rust/serde request and result types. The [quantity arithmetic](../crates/editor-core/src/quantity.rs) is a core module independent of Rhai; scripting depends on it, and typed fields use the same unit data and scaling rule. This proves the execution seam before implementing independently revisioned field schemas, footprint choices or a new component-authoring UI. New Component remains the intended entry point; the desktop editor still starts from templates, now as a working set of five drafts.

`Script` contains `api_version: 1`, source, explicit unit sets and named inputs. A unit set has seven SI dimension exponents (length, mass, time, current, temperature, amount, luminous intensity) and its units; each unit has a symbol, optional aliases, a positive scale to SI and whether it accepts SI prefixes (`kΩ` is `k` on a prefix-accepting `Ω`). Symbols and aliases are unique across a script's sets. Quantity inputs contain a numeric value and unit text, scaled by `UnitSet::to_si`, the same rule field parsing uses; integer inputs support generator counts. No resistor or other component type is known to the interpreter. Units are explicitly supplied data, not a built-in catalog.

`evaluate_quantity` accepts expressions only and returns a checked quantity with an SI value and dimensions. For example, library-supplied inputs of 5 V and 10 kΩ make `voltage * voltage / resistance` return 0.0025 W. Addition of voltage and resistance, division by zero, exponent overflow, nonfinite results, arithmetic underflow and inexact large integer multipliers are rejected. Arithmetic uses bounded `f64`; it is not exact decimal arithmetic. Affine/logarithmic units, boolean validation expressions and dependency/cycle management are not implemented. Quantity kinds that share a dimension and field constraints exist as library data since slice 2 of the [component model](component-model.md#slice-2--typed-fields-and-quantities), but scripts receive only unit sets, not fields.

### Pad generation contract

`library.generate_pads { draft, script: Script }` is a typed operation inside `project.apply`, using its expected revision and generated JSON schema. It requires a footprint draft whose existing pads have unique nonzero IDs. The script receives a `footprint` builder with:

```text
footprint.pad(existing_pad_id, x, y, width, height)
```

All four geometry arguments are length quantities. `q(1.2, "mm")` constructs a quantity from a supplied unit set. A four-pad fixture with IDs 10–13 and a length input `pitch` can use:

```rust
for i in 0..4 {
    footprint.pad(10 + i, i * pitch, q(0.0, "mm"),
                  q(1.2, "mm"), q(1.8, "mm"));
}
```

The proof emits every existing pad exactly once and preserves each pad's ID, package-terminal association and vector order. It does not allocate terminals or pads. Missing, duplicate or unknown IDs fail. Output changes only that footprint draft; published snapshots and placed instances remain unchanged. A successful changed result is one engine undo step. A failed or stale request leaves document, revision and undo/redo history unchanged, even after successful builder calls. `generate_pads` also exposes a pure candidate-producing function for future preview consumers.

### Resource and persistence limits

Requests are limited to 16 KiB source, 64 unit sets (each of 1–32 units) and 64 inputs. Execution has a 50,000-operation cap, 16 call levels, expression depth 32, 128 variables, 16 script functions, 4 KiB strings, and 128 entries per array/map. The generator admits at most 128 existing pads, bounds callback work, and enforces the existing geometry limits. These are admission limits, not measured latency or total process-memory guarantees. Future UI consumers must run evaluation off the UI thread and discard stale results.

Module loading and clock support are disabled at build time. The API exposes no file/network/document access, and disables dynamic `eval`, printing, debugging output, anonymous functions and variable shadowing. Only explicit inputs and interpreter-local values are accessible. Callbacks return candidate data; they cannot commit document edits.

Generated draft geometry persists using the existing `.pcbcraft` format and reopens without the script. Script source, recipe inputs and interpreter-version provenance are **not yet persisted as library assets**; API version 1 identifies this invocation contract, not a completed long-term scripting compatibility promise. There is no saved live dependency graph. This numeric pad-array proof does not replace Solidcraft's retained geometric expressions or claim parametric native regeneration for script formulas.

### Verification

The [seven core scenarios](../crates/editor-core/src/scripting/tests.rs) run natively and through `wasm-bindgen-test` under Node: electrical arithmetic, custom unit data and version refusal, dimensional/numeric failures, expression restrictions, four-pad generation with mapping/persistence/undo, rollback after partial output, and malformed/stale/resource-limit failures. A [transport test](../crates/automation/src/session/tests.rs) checks JSON dispatch and schema discovery through the session shared by CLI and MCP. Run `cargo xtask ci` for the aggregate native and wasm-compilation gate; run `cargo xtask wasm-scripting` for actual wasm execution with the runner prerequisites in [README](../README.md#scripting-integration-proof). Browser UI execution and performance measurements remain outside this proof.

Verified 29 October 2026 on macOS arm64, Rust 1.96.0 from `nix develop`: `cargo xtask ci` passed formatting, layer/container checks, strict Clippy, all 88 native tests and headless wasm compilation. `cargo xtask wasm-scripting` passed all seven scenarios under Node 24.21.0 with wasm-bindgen-test-runner 0.2.129. No desktop UI changes or interactive browser tests were part of this proof.

Re-verified 30 October 2026 after slice 2 moved the arithmetic to `crate::quantity` and replaced unit definitions with unit sets: `cargo xtask wasm-scripting` passed the same seven scenarios under Node 24.21.0 with wasm-bindgen-test-runner 0.2.129, and `cargo xtask ci` passed (see [editor evidence](editor-evidence.md#verification)).
