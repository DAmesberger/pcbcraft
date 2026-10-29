# Pcbcraft part library design

Draft for review, 29 October 2026. The proposed library model supports the first edition's component authoring, schematic and PCB workflow, while leaving room for more advanced parts later.

The goal is **one understandable component workflow over reusable, precisely defined data**. Users should be able to create or select a complete component without managing several unrelated library systems. Internally, the model must distinguish electrical function, schematic representation, physical terminals, PCB geometry and purchasing information.

## What existing tools teach us

KiCad already embeds schematic symbol definitions, supports deliberate library updates and comparisons, and connects schematic symbols to board footprints through stable instance identifiers. Project independence and stable identity are therefore useful established practices, not missing features we should claim to invent. [KiCad schematic documentation](https://docs.kicad.org/10.0/en/eeschema/eeschema.html).

Altium documents a unified component model with schematic and footprint models, custom pin mapping and multiple footprint options. A coherent authoring workflow need not imply that one symbol always binds directly to one footprint. [Altium component editing](https://www.altium.com/documentation/altium-designer/components-libraries/single-component-editing), [placed component models](https://www.altium.com/documentation/altium-designer/schematic/placed-components).

LibrePCB explicitly separates symbols, components, packages and devices, uses stable references and allows different footprints for a package. That is a useful public design reference. [Library concept](https://librepcb.org/features/library-concept/), [library elements](https://docs.librepcb.org/_branches/master/librepcb/quickstart/create-library-elements/).

Pcbcraft should combine these useful ideas with Solidcraft's UI, explicit revision handling and geometry-aware update previews. This proposal is based on public documentation; it does not reuse the implementations of those EDA products.

## Proposed domain model

| Definition | Owns | Does not own |
| --- | --- | --- |
| Component definition | Electrical terminals, their functions/types, parameter definitions and constraints | PCB land dimensions or a distributor SKU |
| Symbol | Drawing, text, pin anchor positions and association to electrical terminal IDs | Independent electrical connectivity or a hard-coded package choice |
| Package | Body and physical terminal arrangement, terminal IDs/designations and mechanical datum | One universal PCB land pattern |
| Footprint | Pads, drills, mask/paste openings, courtyard, silkscreen, assembly graphics and terminal associations | The component's electrical pinout |
| Device binding | Exact component/package revisions and explicit terminal mapping | An inferred mapping based on similar names or pad count |
| Manufacturer part | Manufacturer, complete MPN, binding, rated specifications and source information | Live price/stock or the schematic position of an instance |
| Component instance | Project identity, designator, requirements/value, selected symbol/binding/footprint and optional manufacturer part | A mutable reference to whichever library version is currently installed |

A component may have several symbol representations. A package may have reflow and hand-soldering footprints. A device binding establishes the electrical meaning of the package terminals. An instance chooses one compatible footprint and a schematic representation. Manufacturer information may be added later without changing the logical circuit.

The distinction between package and footprint matters even in the first edition. A larger hand-soldering land pattern does not enlarge the physical component body. The 3D model is registered to the package's mechanical datum; each footprint records how that datum is placed relative to its layout origin. Model offsets and instance placements have explicit parameters and coordinate frames, not hidden corrections. Library snapshots pin the native model revision, its parameter interface and required assets. Package dimensions and model parameters have explicit bindings so the native `.solidcraft` assembly remains recomputable; the live 3D view uses the same Solidcraft engine model. Changes to a model parameter interface require an update preview just like pin-mapping changes.

Not every definition needs a sophisticated independent editor. In the first edition, authoring a footprint can create its package/terminal definition in the background. A user can later choose an existing package to create another footprint. Keep the model sound while keeping the normal interaction short.

## One authoring workspace

The common path is **Create Component**, starting empty, from a small template, or by duplicating/reusing existing definitions. The workspace uses Solidcraft's shell and property controls:

1. **Symbol:** draw the representation; edit a terminal table with names, electrical types and visible pin anchors. Reuse an existing symbol where suitable.
2. **Footprint:** draw or generate pads and outlines; set drills, courtyard and mechanical origin. Reuse a package or footprint where suitable.
3. **Mapping:** show symbol and footprint together, cross-highlight selections and edit the terminal mapping table. List missing, duplicate or prohibited mappings immediately.
4. **3D:** reuse or author a native Solidcraft parametric model through the Solidcraft engine, align it to the mechanical datum, and preview both board sides. Simple dimensioned envelopes use the same engine. Vendor STEP models can be imported through Solidcraft as an additional option; Pcbcraft does not own a separate 3D modelling system.
5. **Details:** optional manufacturer/MPN, rated specifications, datasheet reference, source/license information and descriptive search metadata.

The default workflow creates a complete usable component. Advanced reuse is available within it, without forcing users through a sequence of separate library applications. Search results show a symbol, footprint, 3D preview, relevant specifications and mapping completeness together.

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

For the first edition, expose single-unit symbols and ordinary packages. Preserve a model in which multiple symbol units can later refer to one component instance. Do not implement hidden power pins, implicit gate swapping or a universal pin-equivalence system in the initial editor.

## Updates must explain what changes

Library updates and schematic-to-PCB updates are related transactions, with an explicit preview and one undo step. The preview is tied to the current project revision; it must be recomputed if the project changes before application.

| Change | Required handling |
| --- | --- |
| Description, source metadata or human-readable name | Show the metadata change; retain identity and connectivity |
| Symbol graphics or pin anchor positions | Retain electrical terminal associations; show affected wires and reconnect their presentation deliberately |
| Electrical type, pin function or binding | Show old/new terminal-to-net relationships; require explicit mapping for ambiguous changes and rerun ERC |
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

Defer multi-unit authoring, alternate symbol-view switching and assembly variants, but use their scenarios to check that the first data model does not prevent them.

## Import and provenance

Component import should consume public format specifications, retain source identifiers/provenance, and report unsupported constructs. Never silently flatten a complex symbol or footprint into a plausible but electrically different one. A small supported import subset is acceptable after native authoring works.

Code licensing and library-data licensing are distinct. KiCad publishes its libraries under CC-BY-SA 4.0 with a design-output exception and separate conditions for redistributing library collections. Treat imported assets according to their actual provenance; do not assume they inherit Pcbcraft's code license. [KiCad library license](https://www.kicad.org/libraries/license/).

## Choices for review

The proposal commits early to separate electrical/physical identities, explicit mappings, immutable revisions and portable project snapshots. It keeps the UI unified and local-first, allows generic design without an MPN, and requires semantic update previews before library changes affect existing designs.

The first implementation should test this model with a passive, an IC with repeated supply terminals/exposed pad, and a connector with different footprint origins. Final file organization, advanced symbol variants and broader package support can be decided from those results.
