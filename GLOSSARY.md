# Pcbcraft

Proposed vocabulary for the first-edition design. These terms distinguish reusable library definitions from their use in a circuit.

## Language

**Component definition**:
A reusable electrical interface, including its terminals and their electrical roles. It can have different schematic representations and physical implementations.
_Avoid_: Symbol, manufacturer part

**Symbol**:
A schematic representation of a component definition: per unit, drawn graphics and one pin for each of its electrical terminals.

**Pin** (symbol pin):
Where a terminal connects on a symbol unit: a connection end, a direction into the body, a stub length and reading decorations. A pin is identified by its terminal and stores no electrical type and no number; its number comes from the instance's binding.
_Avoid_: Anchor

**Graphic**:
A drawn primitive (line, polyline, rectangle, circle, arc, polygon) or text of a symbol, with a stable local ID across revisions. Text refers to a designator, name, unit label or field; it is never the authority for a value.

**Authoring grid**:
The snapping grid of the library editor (2.5 mm for symbols, 0.05 mm for footprints by default). An editor setting saved with the drafts; never revision data and never a blocking rule.

**Symbol unit**:
An independently placeable portion of a component's schematic representation, such as transistor A or an amplifier's power unit. All units of one component instance share its physical implementation and purchasing identity.

**Symbol occurrence**:
The placement of one symbol unit of a component instance on a schematic sheet. It has its own position and orientation while referring to the component's electrical terminal identities.

**Electrical terminal**:
A connection point on a component's electrical interface. Terminals with the same role or name are not automatically the same connection point.

**Pin function**:
A declared capability of an electrical terminal, with its own identity, electrical direction and signal role. A multi-function terminal offers alternative functions without creating additional physical connections.

**Pin-function selection**:
The intended active function chosen for a terminal of a particular component instance. It determines that terminal's displayed function and electrical checking behavior without changing its identity or package mapping.

**Peripheral resource**:
A named functional resource within a component, such as a UART instance or ADC channel. Pin functions may refer to that resource and its signal roles, with explicit rules governing compatible selections.

**Terminal class**:
How a component definition classifies a terminal: electrical, with one or more pin functions; device NC, declared by the device as not connected; or reserved. Device-NC and reserved terminals have no functions, cannot be wired and need no project decision.

**Drive**:
How an output or bidirectional function drives its net: push-pull, open drain or open source. Inputs, passive and power functions have no drive. Electrical checks use the active function's drive, so several open-drain drivers with a pull-up are not a conflict.

**Resource role**:
A part of a peripheral resource, such as a UART's transmit line, with a capacity (how many active functions may take it at once) and whether it is required once the resource is used.

**Design default**:
The function a multi-function terminal uses when an instance makes no explicit selection. A terminal with one function is fixed and needs no default; a reset state is never a design default.

**Function resolution**:
The active function of each electrical terminal of one instance, derived from its explicit selection, its fixed function or its design default. Terminals with none are unresolved. Exceeded role capacity, an excluded pair or a function unavailable in the chosen implementation is a conflict, which commands reject; a missing required role or an unmet requirement is incomplete, which completeness reports.

**Unavailable function**:
A pin function that a particular device binding cannot provide. An instance whose active function is unavailable in a binding cannot use that binding.

**Package**:
A physical component body and its terminal arrangement, independent of the particular PCB land pattern used to mount it.
_Avoid_: Footprint

**Package body style**:
The authored construction of a package body (moulded, chip resistor or chip capacitor) that sets its 3D look; never inferred from a designator or name.

**Package terminal**:
A physical electrical connection on a package, such as a lead, ball or exposed pad, with an explicit designation.

**Footprint**:
A PCB mounting pattern for a package, including copper lands, holes, mask and paste openings, outlines and placement references.
_Avoid_: Package

**Footprint variant**:
An alternative mounting pattern for the same package, such as a hand-soldering pattern with larger pads. Each variant has its own identity and revision history.
_Avoid_: Revision when describing an alternative pattern

**Pad**:
A copper land in a footprint, associated with a package terminal when electrically connected. Several lands may belong to the same terminal without being connected by PCB copper. Its number is a display label, never its identity.

**Pad kind**:
What a pad is for: SMD and plated-through pads land a package terminal; non-plated holes and fiducials never do; a mechanical pad is copper with an explicit decision not to land one. No terminal is inferred from a pad number.
_Avoid_: Treating a terminal-less pad as unfinished when it is mechanical

**Polarity pad**:
The authored pin-1 pad of a footprint. Silkscreen or assembly markers only illustrate it; the pad is the authority.
_Avoid_: Pin-1 dot (the marker, not the rule)

**Courtyard**:
The keep-out area a placed part and its assembly tools need, drawn as closed shapes on the courtyard layer. It is authored or explicitly generated, never inferred.

**Mask web**:
The strip of solder mask between two neighbouring pad openings. A web below the estimated 0.1 mm default is an advisory lint.

**Footprint source**:
How a footprint's solved pads and drawings were derived: an opaque sketch, parameters, pad roles and arrays, interpreted only by a footprint evaluator outside the core. Consumers read only the solved data.

**Footprint evaluator**:
The host component that interprets a footprint source: it applies sketch edits and solves the source into typed pads, drawings and the datum. The core never interprets a sketch; the desktop, CLI and MCP pass Solidcraft's sketch evaluator.

**Pad frame**:
A pad's own coordinates: origin at the pad position, axes turned by the pad rotation. Shapes, holes, vias and paste windows are stored in it.

**Mask opening**:
The gap in the solder mask over a pad: the land grown by the pad's mask expansion as a mitred offset, or none when the pad is tented. Display-only in 3D, never saved in the native model.

**Package value**:
A typed value a package offers footprint expressions under the reserved `pkg_` prefix, such as `pkg_L` (a length) or `pkg_n` (a count). Footprint parameters may not use the prefix.

**Device binding**:
An explicit association between a component definition and a package, including the mapping from electrical terminals to package terminals and the footprint variants it allows. A binding belongs to one component lineage, and component revisions list the bindings they allow.
_Avoid_: Footprint assignment when a pin mapping is also required

**Manufacturer part** (Part):
One exact orderable item: its own revisioned definition lineage holding the manufacturer as written, the complete manufacturer part number (MPN, ordering suffixes included), the exact component revision and device binding it implements, its rated specs, references, distributor offers and provenance. An instance may pin one Part revision; the Part must agree with the instance's component revision, physical choice and requirements, and a component update that leaves it behind is a finding, never a silent re-pointing. A supplier listing is an offer for that part, not its identity. An "atomic part" in other tools is a component plus exactly one Part.
_Avoid_: Part for a component definition or a catalog entry

**Rated spec**:
One value a manufacturer part is rated at, typed by a field definition and carrying a spec meaning, such as resistance 10 kΩ (exact nominal) or voltage 25 V (minimum rating). It matches an instance requirement by field lineage, never by name.

**Spec meaning**:
How a rated spec meets a requirement of the same field: exact nominal (the value is the required one), minimum rating (rated at least the required value, so a 25 V part meets a 16 V requirement) or maximum tolerance (no wider than the required tolerance on either side). One core comparison, `fields::satisfies`, applies it for search, assignment and purchasing completeness.

**Requirement** (part requirement):
An instance's effective value of one selected field, compared with the rated spec of the same field lineage when a Part is chosen.

**Distributor offer**:
A distributor's listing for an exact manufacturer part, identified by the distributor and its own SKU or product link, with optional packaging text. It is part of the Part revision and holds only identity the user chose. Price and availability are observations about that offer, not properties of the electrical component definition, and Pcbcraft does not store them.

**Reference link** (reference):
A labeled reference associated with a component, package or manufacturer part, such as a datasheet, application note or product page. A Part's reference is a record with a stable local ID, kind, title, optional URL, document revision, licence, provenance and an optional attached copy named by its SHA-256. A link identifies an external resource; it does not imply that a copy is available offline, and a missing URL removes neither the record nor its attached copy.

**Component instance**:
One component used in a project, such as R12, with its electrical connections, chosen implementation and project-specific requirements.
_Avoid_: Library component when referring to a placed instance

**Designator prefix**:
The letters a component definition declares for numbering its instances, such as R, C or U; the core assigns the lowest free number under it when a placement gives no designator.
_Avoid_: Reference class, inferring meaning from a designator

**Component property**:
A named, typed characteristic of a component, such as resistance, resistance tolerance or rated power. Its meaning includes the quantity and permitted units where applicable, independently of its displayed text.

**Property field definition**:
A reusable library definition of a component property's meaning, value type, permitted units and constraints. Component definitions select these fields, allowing their properties to be described without a built-in component-specific schema.

**Instance property value**:
The value assigned to a component property for a particular project instance, such as R12's resistance of 10 kΩ. It describes the same component in both schematic and PCB views.

**Quantity kind**:
A library definition of what a quantity measures: a dimension and the units that measure it, with a default unit. Two kinds with the same dimension, such as resistance and reactance, are different kinds; a unit symbol never establishes kind identity.

**Unit set**:
A dimension with its linear units, each with a symbol, aliases, a scale to SI and whether it accepts SI prefixes. Field parsing and scripts scale units through the same set.

**Field selection**:
A component revision's choice of one exact field revision, with an optional component default and whether the value is shown on the symbol.

**Field override**:
An instance's decision for one selected field, keyed by field lineage: an assigned value or an explicit unset. Without an override the field is inherited.

**Effective value**:
The value a field has on an instance, with its origin: assigned, component default, field default, explicitly unset or no value. It is derived, never stored. Assigned zero is a value, distinct from explicitly unset.

**Net**:
A set of electrical connection points intended to be joined. Each net has a persistent ID and an editable name; tracks refer to the ID. Connecting merges nets (the routed, else older, ID survives) and disconnecting splits them (the larger group keeps the ID). Sharing a net does not establish that PCB copper actually connects those points.

**No-connect marker**:
A project decision to leave a particular connection point unconnected. It is distinct from a manufacturer's declaration that a package terminal must not be connected.

**Definition lineage**:
The stable identity of one library definition across all its revisions, such as one footprint. A lineage has exactly one kind (component, symbol, package, footprint, binding, quantity kind or field).

**Definition draft**:
An editable, possibly incomplete body of one definition lineage, saved and recovered with the project. Publishing turns it into a new immutable revision; nothing marks a draft as verified.

**Local ID**:
An ID that is unique only inside one definition lineage, such as a terminal, pin-function, resource, role, unit, package-terminal, pad or enumeration-choice ID. Each revision allocates new local IDs from its `next_local` counter; an ID retired by an earlier revision is never reused, so it cannot come back with a different meaning.

**Publish set**:
Several drafts published together as one transaction, in dependency order (quantity kind, field, package, footprint, binding, symbol, component). References inside the set may name each other's pending revisions; any blocking finding publishes nothing. Publishing a changed definition together with the revisions that must reference it is a _revision cascade_.

**Representation**:
A symbol lineage a component revision offers for drawing it. An instance chooses one; its units are that symbol's units.

**Implementation**:
A device-binding lineage a component revision offers for building it physically. Together with one of the binding's footprints it forms the instance's physical choice.

**Exact reference**:
A reference to one lineage at one revision. A component instance pins exactly one component revision; its symbol and implementation choices are lineages resolved through that revision.

**Physical choice**:
The binding and footprint lineages an instance uses, offered by its pinned component revision. An instance without one can be drawn and wired but not placed on the board.

**Unit state**:
Whether a symbol unit of an instance is placed (has an occurrence), deliberately unused or unplaced. Placed and unused exclude each other; unused never implies a no-connect decision.

**Completeness finding**:
A finding that blocks a derived stage: publication, board placement or electrical resolution. A state holds when no current finding blocks it; it is never stored.

**Library revision**:
A specific saved version of a library definition. Later edits produce a different revision of that definition; alternative definitions such as footprint variants retain separate identities.

**Project library snapshot**:
The exact library definitions and assets needed to reproduce a project's symbols, footprints, mappings and component models.

**Board update**:
A reviewed transfer of schematic or component changes into the PCB, preserving the identity of existing component instances.

**Fabrication attributes**:
The board finish choices a fabricator builds: soldermask colour and finish, pad surface finish and silkscreen colour. They are saved in the design; the 3D view draws them, but they do not change the native Solidcraft model.

**Mechanical export**:
A geometric representation of the board and its components for enclosure and assembly work. It does not contain the complete electrical design.

**Native Solidcraft model**:
A Solidcraft design containing the board and component models, with editable parameters, features and placement relationships. It is the 3D model used by Pcbcraft and saved for direct use in Solidcraft.

**Push-and-shove routing**:
Interactive routing that moves existing tracks to make room while preserving their connections and obeying the board rules and locked-object constraints.

**Library source**:
Where catalog entries come from. The project library snapshot is the only source today; local library folders are planned. A source is authoritative only for new imports, never for a project that already holds a revision.

**Catalog entry**:
One searchable lineage at its latest published revision in one library source, derived from library data and never stored: its name, description, effective field defaults, package names, completeness badges and how many instances use it. A lineage that exists only as a draft has an entry that cannot be placed.
_Avoid_: Part (a part is an exact manufacturer part)

**Catalog query**:
One line of free text, reserved filters (`pkg:`, `has:`, `missing:`, `src:`) and field predicates, parsed in core against the library's field definitions and ANDed.

**Field predicate**:
A query term that compares a typed field value by meaning, such as `res>=10k`, `tol<=1%` or `C:1n..100n`. Its key resolves through the data's field names and units; its value is parsed in the field's own units, and a comparison means "meets", not "equals".

**RKM shorthand**:
Writing the decimal point as a multiplier or unit letter, as in `4k7` for 4.7 k or `R47` for 0.47 Ω. Unit letters come from the field's unit set, so `R` reads as a decimal point only where the data declares `R`.

**Facet**:
A filter offered because the current results carry it: a field with its values and range, a package name or a completeness badge. Facets are derived from the results, never from a fixed category list.
_Avoid_: Category

**Completeness badge**:
One separately shown property of a catalog entry: a symbol, a footprint, a 3D envelope or a manufacturer part number. Badges are never combined into a single "complete" flag.

**Place palette**:
The Schematic editor's searchable sheet for finding a component revision in the catalog and placing it, previewed in the properties card. It runs the same `library.search` query as the CLI and MCP and places through `component.place`.

**Purchasing completeness**:
Whether an instance can be bought as designed, derived and never stored: no part (generic), unresolved (a Part is chosen but its component revision, binding or a requirement disagrees, or it is stale) or resolved. Its findings are advisory and block no other stage.

**Resolve parts**:
The view and query (`library.resolve_groups`) that groups instances without an agreeing Part by component revision, footprint and identical requirements, as the BOM groups them, so that one Part is assigned to a whole group with one `instance.assign_part`, one undo step.
