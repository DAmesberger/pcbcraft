# Sparksmith

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
A named functional resource within a component, such as a UART instance or ADC channel; also called a peripheral instance. Pin functions may refer to that resource and its signal roles, with explicit rules governing compatible selections. Its optional kind (`uart`, `spi`, …) is grouping text and never selects behaviour.

**Terminal class**:
How a component definition classifies a terminal: electrical, with one or more pin functions; device NC, declared by the device as not connected; or reserved. Device-NC and reserved terminals have no functions, cannot be wired and need no project decision.

**Drive**:
How an output or bidirectional function drives its net: push-pull, open drain or open source. Inputs, passive and power functions have no drive. Electrical checks use the active function's drive, so several open-drain drivers with a pull-up are not a conflict.

**Resource role**:
A part of a peripheral resource, such as a UART's transmit line, with a capacity (how many active functions may take it at once), whether it is required once the resource is used and optionally the datasheet's signal name for it (signal role).

**Mux code**:
The datasheet's selector for one pin function, such as an alternate-function number, stored as explicit text on the function. It is shown and exported, never parsed out of a name.

**GPIO identity**:
The port and index a datasheet gives a terminal (`A`, 9). One identity belongs to one terminal of a component.

**Pin option** / **Pin configuration**:
A pin option is a setting a terminal declares with its allowed values: pull, speed, initial level or Schmitt input. A pin configuration is an instance's chosen values for one terminal, plus an optional firmware name; it is checked against the declared options and never changes electrical checks.

**Pin rule**:
A datasheet restriction on pin-function combinations, stored in the component revision as a short sandboxed Rhai script that reads a read-only view of one assignment and returns ok, forbid, require or warn. Forbid rejects the assignment, require leaves it incomplete and warn is a note; a script that fails to compile, raises an error or exceeds its budget is a finding.

**Pinout document**:
A typed, format-versioned description of a device's pins, peripherals, functions, options and rules, filled from a datasheet by a person or an agent. Importing it is previewed as a plan, applied against that plan's base and leaves every written row unreviewed; a row flagged uncertain blocks publishing until it is corrected or cleared (import review). The review state is draft-only (publishing drops it) and the gate is advisory against automation: an agent that edits the draft can clear it, so agents clear a flag only after the person confirms the row.

**Firmware pin configuration**:
The format-1 description of one instance's configured pins (function, peripheral, role, mux code, GPIO, options and firmware name) derived from its function resolution, pin configuration and named nets, and emitted as JSON, a C header, a Zephyr overlay skeleton, Rust constants or a user template. It is a starting point, not clock, register or HAL setup.

**Design default**:
The function a multi-function terminal uses when an instance makes no explicit selection. A terminal with one function is fixed and needs no default; a reset state is never a design default.

**Function resolution**:
The active function of each electrical terminal of one instance, derived from its explicit selection, its fixed function or its design default. Terminals with none are unresolved. Exceeded role capacity, an excluded pair or a function unavailable in the chosen implementation is a conflict, which commands reject; a missing required role or an unmet requirement is incomplete, which completeness reports.

**Unavailable function**:
A pin function that a particular device binding cannot provide. An instance whose active function is unavailable in a binding cannot use that binding.

**Package**:
A physical component body and its terminal arrangement, independent of the particular PCB land pattern used to mount it.
_Avoid_: Footprint

**Package form**:
The authored, typed physical construction of a package (generic, chip, gull-wing, no-lead or pin header) with its dimensions; the only source of `pkg_*` values and of generated 3D models, never inferred from a designator or name.

**Physical material**:
A built-in material a 3D body is made of (epoxy mould compound, alumina, tin, gold, PBT, …), with cited or estimated optical values; never a colour or a vendor look.

**Generated model**:
A package's parametric 3D model built from its package form by a family builder, with a body role and a physical material on every body; one shared native definition per package revision, used by every instance.

**Library model** (3D model):
A revisioned library definition of a package's 3D model from a file (GLB for appearance, STEP for mechanical shape, `.solidcraft` for parametric): its packed asset by content hash, source frame, bounds, every body mapped explicitly to a role and material, lead tags with measured contacts, an optional marking area and provenance with redistribution. Packages share it by exact revision.
_Avoid_: Imported model, vendor model

**Lead tag**:
A body (or one face of a body) of a library model named as a lead, with its designation and its contact point, measured on the geometry and rechecked when the model is built.

**Lead map**:
The explicit pairing of a library model's lead tags with a package's terminals, stored where the package uses the model. Exact designation matches are only proposed, never inferred.

**Project container**:
The `.pcbcraft` file: a ZIP archive of `project.json` and one raw entry per packed asset under `assets/sha256/<hash>`, read with bounds on the decompressed bytes and written atomically.

**Packed asset**:
File bytes a project carries by content hash (a model file, SVG artwork), so it reopens offline; stored as their own container entries.

**Marking**:
The lettering and marks on a package top (top mark, value code, polarity mark, colour bands): a spec on the component or a Part's top mark, overridden per instance. Display only: never in the saved native model or fabrication output.

**Value code**:
A marking item that encodes a field value with a cited scheme (3- or 4-digit resistor code, EIA-96, capacitor pF code, tantalum voltage letter, IEC 60062 colour bands); a value the scheme cannot hold exactly gives no code, never a rounded one.

**Marking decal**:
The display-only mesh of an instance's laid-out marking, 5 µm above the package's marked face, drawn with the finish's look and shown with the Markings layer toggle.

**Model placement**:
The authored rigid transform of a package's 3D model relative to its footprint datum; the seating plane (z = 0) is the top of the land copper. Fit leads to pads, quick actions and the triad each set it with one command.

**Seating plane**:
The plane a package rests on: z = 0 of the model frame, at the top of the land copper. Lead contacts lie on it; a body below it reaches into the board.

**Mechanical part**:
A part placed directly on the board from a part file (a solderable nut, spacer or heatsink): an ordinary component instance of a generated set (component, mechanical symbol, package using the part file, footprint, binding) whose package records its variant key, source and assembly flags. Its symbol unit starts unused; its pads are mechanical copper that may land terminals.
_Avoid_: Board-only object, inferring a mechanical part from a designator

**Part variant**:
One mechanical part set for one part-file revision, values, lands and assembly flags, found again by its key. Editing a placed part's parameters moves the instance to the variant of the new values, made if missing; instances never vary geometry on their own.

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

**Derived courtyard**:
A courtyard graphic a footprint source computes from its pad and body extents grown by a named excess (`CourtyardFromExtents`). It exists only because a template or the author added that role explicitly; nothing adds one automatically. An assembly outline from the package envelope is the same kind of explicit role.

**Courtyard overlap**:
The board check that two placements on the same side have courtyards sharing interior area. Touching edges is not overlap; a part without a courtyard is not checked.

**Mask web**:
The strip of solder mask between two neighbouring pad openings. A web below the design rules' minimum mask web (0.1 mm in the default profile, an estimate) is an advisory lint.

**Footprint source**:
How a footprint's solved pads and drawings were derived: an opaque sketch, parameters, pad roles and arrays, interpreted only by a footprint evaluator outside the core. Consumers read only the solved data.

**Footprint evaluator**:
The host component that interprets a footprint source: it applies sketch edits and solves the source into typed pads, drawings and the datum. The core never interprets a sketch; the desktop, CLI and MCP pass Solidcraft's sketch evaluator.

**Footprint template**:
A parametric footprint family (2-terminal chip, gull-wing SOIC/SOP, QFN/DFN, pin header, tabbed SOT/DPAK, molded SOD/SMA/MELF, BGA/LGA, through-hole) that builds into an empty footprint draft as ordinary sketch edits, with dimensions bound to named parameters, in one undo step. Its land defaults are estimated unless a package row cites a land pattern.

**Package row**:
A revisioned library record holding one typed package form of a family, with aliases, a size code, a suggested skeleton, optional lands, provenance and a citation of the public outline drawing its values were typed from. A row without a citation is *estimated*. New from template and From template build from an exact published row revision, a folder row read with its bundle, or a built-in **starter row** (the compiled-in, read-only tables every install lists).
_Avoid_: Preset (test and sample data only), wizard

**Custom entry**:
A family's built-in template entry of original typical values (estimated) that the author edits; listed with the rows, never a citation.

**Template gallery**:
The one searchable "New from template" sheet over package rows and custom entries (`library.templates`): new-component scope from New and the start page, package scope from the Footprint tab's From template.

**Start page**:
What the Component workspace shows when no component is open: catalog search, Recent and New from template…. The workspace never opens a component the user did not choose.

**Identity bar**:
The strip above every Component workspace view naming the open component (source › name), its state (published, draft, new, read-only) and origin, with back, forward and Switch.

**Read-only preview**:
A library-folder item opened in the Component workspace from a scratch engine; nothing it does reaches the project until Edit a copy in this project imports and drafts it as one undo step.
_Avoid_: Footprint wizard, generator (the Rhai generator is a different seam)

**Driving dimension**:
A sketch dimension whose value the solver must meet, set as a number or bound to a footprint parameter. A driven (reference) dimension only reports a measurement and is shown in parentheses.

**Active layer**:
The layer the Footprint tab's Line, Arc, Circle and Rectangle draw on: construction (reference geometry, never output), silkscreen, assembly or courtyard.

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
A distributor's listing for an exact manufacturer part, identified by the distributor and its own SKU or product link, with optional packaging text. It is part of the Part revision and holds only identity the user chose. Price and availability are observations about that offer, not properties of the electrical component definition, and Sparksmith does not store them.

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

**Junction**:
A schematic connection point where wires meet away from a terminal, made where a wire end lands on another wire's segment. The solid junction dot is drawn wherever three or more conductors meet (a pin counts as one); it is derived from connectivity, not stored. A plain wire crossing is not a junction.
_Avoid_: Dot (as the stored object)

**Sheet**:
One page of the schematic, with its own name, paper size and page frame, in a project-wide page order. Symbol occurrences, wires, junctions, labels and sheet furniture each lie on one sheet; a wire never crosses sheets.
_Avoid_: Page (as the object), schematic (for one sheet)

**Net label**:
A named connection point on a sheet that wires end on and whose name joins it to other labels: a local label within its sheet, a global label (off-sheet connector) across all sheets, a hierarchical label to the sheet pin of the same name on its sheet's symbol. Labels name their net by a fixed priority; they never join anything by drawn position.

**Power port**:
A global net label drawn as a ground or supply symbol (GND, +3V3, VBUS). It joins every power port and global label of its name and gives its net the highest-priority name. It is not a component instance and has no BOM line or footprint.
_Avoid_: Power symbol (for the component kind of that name)

**Sheet symbol**:
A box on a parent sheet that stands for one child sheet, with sheet pins on its edges. Each sheet has at most one parent symbol; sheets without one are top-level.

**Sheet pin**:
A connection point on a sheet symbol's edge that joins the hierarchical labels of the same name on the child sheet.

**Annotation**:
Numbering designators in page and position order across sheets. By default only designators that are not yet prefix plus number change, so existing designators stay stable.

**Title block**:
The block in a page frame's corner that shows the project properties (title, revision, date, author, company), the sheet's name and "sheet n of m".

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

**Stackup**:
The board's layer build, top to bottom: copper foils and the dielectrics (core or prepreg, with a glass style) between them, plus mask and silkscreen on both sides. It sets the board and copper thicknesses; 2, 4 and 6 copper layers are offered. See [PCB realism](docs/pcb-realism.md).

**Copper layer**:
A named layer of the stackup that carries copper: Top, Inner 1…n or Bottom. Tracks lie on a copper layer; a board side (top or bottom) is the side a part, mask or silkscreen is on, and its outer copper layer is the copper layer of that side.

**Board via**:
A plated hole that joins its net's copper on every copper layer it spans, placed on the board (distinct from a pad via inside a footprint pad). It is tented (covered by mask) or open, and it is real copper in the native model. Only through vias exist yet.

**Design rules**:
The limits a board is checked and routed against: clearances between copper objects, board edges and holes, hole and via sizes, mask, silkscreen, paste, courtyard, fill and land-estimator values. A project's effective rules are its fab profile, then its rule overrides, then net classes and class-to-class clearances. Every finding names the rule, its value and where it came from.

**Fab profile**:
The file that holds a fabricator's design rules: a named, complete set of every rule, each citing its source (the fabricator's capability page and the date it was read). Sparksmith ships none; one is imported or saved by the user. A project keeps a snapshot of its fab profile, so a changed library profile does not change the project until it is switched. The rules themselves stay "design rules".
_Avoid_: Rule profile, fab preset

**Rule override**:
A project's own value for one rule, kept above its profile. A profile switch may keep or drop overrides; one that would break the new profile is refused.

**Net class**:
A named group of nets with their own track width, minimum width, clearance, via size and fill connection. A net is in a class by explicit assignment or by the first matching name pattern, never by any other inference.

**Differential pair**:
Two nets with an explicit positive and negative half, routed together at the pair gap. Its width, gap, impedance target and limits come from the positive net's class. A pair is never inferred from net names; names only suggest one.
_Avoid_: Diff net

**Length group**:
Nets and pairs whose routed lengths must match: within a tolerance of a target, or of the longest member.
_Avoid_: Match group

**Routed length**:
A net's track centre-line length plus the via barrels between the layers it uses at each via.

**Meander**:
Bumps added to one track segment to lengthen it by an exact amount: a trombone bumps to one side, an accordion swings to both.
_Avoid_: Serpentine (as a term), squiggle

**Copper fill**:
A copper region of one net on one copper layer, drawn as an outline or following the board outline (its stored region), and computed by the core from the board around it: it keeps its clearances, joins its net's pads with solid or thermal connections and yields to higher-priority fills. Its computed copper is stored with the key of its inputs, and it is stale when they change.
_Avoid_: Zone

**Keepout**:
An outline on chosen copper layers where tracks, vias, pads or fill copper are not allowed.

**Board shape**:
The board's outline: a rectangle or a parametric sketch whose dimensions are bound to board parameters, with cutouts, milled slots and mounting holes. The 3D laminate, fills and board-edge checks all use the solved shape.

**Named edge**:
An edge of the board shape with a name (bottom, right, top and left for a rectangle) that edge constraints refer to.

**Edge constraint**:
A placement, fill or keepout tied to a named edge by an anchor, a distance along the edge and an offset from it (expressions over board parameters), so it follows the edge when the board shape changes.

**Display-only layer**:
A body the 3D view draws for realism but never saves: soldermask and via tents, plated finish, open-via bores, silkscreen films, edge stackup bands, terminations, solder and marking decals. The saved native model holds only the physical bodies.

**Layer view**:
Which layers PCB 3D shows, toggled from the navigation bar's Layers popover. It is display state: hiding a layer filters what is drawn and picked and never rebuilds geometry. Outer copper and silkscreen share their state with PCB 2D.

**Mechanical export**:
A geometric representation of the board and its components for enclosure and assembly work. It does not contain the complete electrical design.

**Native Solidcraft model**:
A Solidcraft design containing the board and component models, with editable parameters, features and placement relationships. It is the 3D model used by Sparksmith and saved for direct use in Solidcraft.

**Board model update**:
How the model worker makes the native model of an edited board from the last one it built: when the edit changes only values (a parameter, a sketch's drawing, where a via placement stands) they are changed in the kept design and only what they reach is evaluated and meshed again; any other edit builds the board anew. The result is the model a fresh build makes. See [PCB realism](docs/pcb-realism.md#edits-update-the-model-in-place).

**Placed definition mesh**:
The one mesh a shared native definition (a via size, a package revision) is drawn from, with each placement's rigid move. The 3D view draws it instanced, once per placement with that placement's identity; the saved model and its geometry stay those of every placement.

**Push-and-shove routing**:
Interactive routing that moves existing tracks to make room while preserving their connections and obeying the board rules and locked-object constraints. The Route tool's Shove mode; see [push-and-shove](docs/push-and-shove.md).

**Walkaround**:
The routing mode in which the route head goes round other nets' copper, keepouts and cutouts at the rule clearance, hugging them in its posture, and stops at the last legal point when no way round exists. Shove falls back to it.

**Library source**:
Where catalog entries come from: the project library snapshot, and the library folders listed in the user's settings. A folder source is read-only or writable, and one that cannot be read stays listed with its reason (path missing, unsupported format, unknown required feature). A source is authoritative only for new imports, never for a project that already holds a revision.

**Library folder**:
A directory of library items shared as plain files or through git: a `pcbcraft-library.json` manifest (format version, random 128-bit library id, name, required features), one immutable file per revision at `lineages/<lineage id>/rNNNN.json`, asset bytes at `assets/sha256/<hash>` and the mutable `catalog.json` of shelves and tags. Lineage ids inside a folder are random, so two authors adding lineages never collide. A manifest of another format or with an unknown required feature is refused, never read partly.
_Avoid_: Library project, library file

**Shelf**:
A label grouping a folder's lineages for browsing, stored in the folder's `catalog.json`. Not revisioned, never copied into a project and never identity.
_Avoid_: Category

**Tag**:
A free label on a lineage, kept like a shelf in the owning source's label map. Many tags may apply; none is identity.

**Catalog index**:
A per-user cache of one library folder's catalog entries and record hashes, kept in the user cache directory. Never authoritative: a deleted or stale index is rebuilt identically from the folder, and it is never written into a project or a folder.

**Origin**:
Where an imported project revision came from: library id, source lineage, source revision and the content hash of the source record. Imports dedupe on it, update-available marks compare with it, and a source record that no longer hashes to it is reported as modified. Origins are stored beside the project library, outside revision bodies, so a project reopens offline without its folders.

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

**Command palette**:
The desktop app's searchable list of every runnable command (⌘P / Ctrl+P), with its shortcut and context; it runs a command through the same path as its ribbon button or key. Distinct from the Place palette, which finds components.
_Avoid_: Quick open, launcher

**Shortcut context**:
Where a command's key applies: everywhere, a workspace, one of its views, or an armed tool's mode (Route, Fill, Board shape). A tool mode's keys override its view's while it is active; two commands may not share a key where both could apply.
_Avoid_: Scope, keymap layer
