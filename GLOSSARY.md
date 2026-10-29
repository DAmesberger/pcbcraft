# Pcbcraft

Proposed vocabulary for the first-edition design. These terms distinguish reusable library definitions from their use in a circuit.

## Language

**Component definition**:
A reusable electrical interface, including its terminals and their electrical roles. It can have different schematic representations and physical implementations.
_Avoid_: Symbol, manufacturer part

**Symbol**:
A schematic representation of a component definition, with graphical pin anchors associated with its electrical terminals.

**Electrical terminal**:
A connection point on a component's electrical interface. Terminals with the same role or name are not automatically the same connection point.

**Package**:
A physical component body and its terminal arrangement, independent of the particular PCB land pattern used to mount it.
_Avoid_: Footprint

**Package terminal**:
A physical electrical connection on a package, such as a lead, ball or exposed pad, with an explicit designation.

**Footprint**:
A PCB mounting pattern for a package, including copper lands, holes, mask and paste openings, outlines and placement references.
_Avoid_: Package

**Pad**:
A copper land in a footprint, associated with a package terminal when electrically connected. Several lands may belong to the same terminal without being connected by PCB copper.

**Device binding**:
An explicit association between a component definition and a package, including the mapping from electrical terminals to package terminals.
_Avoid_: Footprint assignment when a pin mapping is also required

**Manufacturer part**:
An exact manufacturer's part number, associated with its device binding and rated specifications. A supplier listing is an offer for that part, not its identity.

**Component instance**:
One component used in a project, such as R12, with its electrical connections, chosen implementation and project-specific requirements.
_Avoid_: Library component when referring to a placed instance

**Net**:
A set of electrical connection points intended to be joined. Each net has a persistent ID and an editable name; tracks refer to the ID. Connecting merges nets (the routed, else older, ID survives) and disconnecting splits them (the larger group keeps the ID). Sharing a net does not establish that PCB copper actually connects those points.

**No-connect marker**:
A project decision to leave a particular connection point unconnected. It is distinct from a manufacturer's declaration that a package terminal must not be connected.

**Library revision**:
A specific saved version of a library definition. Later edits produce a different revision.

**Project library snapshot**:
The exact library definitions and assets needed to reproduce a project's symbols, footprints, mappings and component models.

**Board update**:
A reviewed transfer of schematic or component changes into the PCB, preserving the identity of existing component instances.

**Mechanical export**:
A geometric representation of the board and its components for enclosure and assembly work. It does not contain the complete electrical design.

**Native Solidcraft model**:
A Solidcraft design containing the board and component models, with editable parameters, features and placement relationships. It is the 3D model used by Pcbcraft and saved for direct use in Solidcraft.

**Push-and-shove routing**:
Interactive routing that moves existing tracks to make room while preserving their connections and obeying the board rules and locked-object constraints.
