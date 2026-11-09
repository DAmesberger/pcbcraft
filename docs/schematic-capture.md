# Schematic capture: labels, power ports, sheets and printing

As built on 2 November 2026. This extends the single-sheet capture described in [editor evidence](editor-evidence.md) (symbol occurrences, wires, junctions) to designs of real size: net labels and power ports, several sheets with a hierarchy, sheet furniture with a page frame and title block, designator annotation across sheets, label/hierarchy/power ERC and vector PDF printing. Every change is a typed `SchematicCommand` applied through the one engine (`Operation::Schematic`), so the desktop, the CLI (`pcbcraft-cli apply`) and MCP (`project.apply`) share it; each is one undo step and is refused without changes on invalid input.

## Document

| Data | Where | Identity |
| --- | --- | --- |
| Sheets in page order, page size and orientation | `Project.sheets` (`Sheets { list, next }`) | `SheetId` (own allocation space; a new project has sheet 1, "Sheet 1", A4 landscape) |
| Title-block properties: title, revision, date, author, company | `Project.properties` | — |
| Labels and power ports | `Project.labels` (`Label { id, sheet, name, kind, position, rotation, manual }`) | object `Id` |
| Sheet symbols and their pins | `Project.sheet_symbols` (`SheetSymbol { id, sheet, child, position, size, pins }`) | object `Id` (pins too) |
| Notes, lines, rectangles | `Project.drawings` (`SheetDrawing { id, sheet, item }`) | object `Id` |
| Sheet of a symbol occurrence or junction | `SymbolOccurrence.sheet`, `Junction.sheet` | — |

File format (still version 1, no migrations): every new field has a default and is omitted when it holds it (`sheets` when it is the single default sheet, `sheet` when it is sheet 1, empty lists), so a single-sheet design without labels writes exactly what it wrote before. Wire ends gain `{"label": id}`, `{"sheet_pin": id}` and `{"free": [x, y]}`. A wire stores its `sheet` and its `corners` ([schematic wires](schematic-wires.md)); its attached ends lie on that sheet. Sheet coordinates are millimetres with y up; every page has its top-left corner at (−30, 30) mm (`model::sheet::PAGE_TOP_LEFT`), so content near the origin, as single-sheet designs placed it, lies inside the frame.

## Wires, connection points and junctions

The schematic connects what it shows, as other schematic editors do. Connection points are pin ends, junction points, labels and power ports, and sheet pins.

- **A wire that runs over a connection point connects to it**, at the wire's end or anywhere along a leg. The engine normalises the wiring after every edit (`schematic::tidy`): a point inside a wire's path splits the wire there into pieces that draw the same path (the point then carries a T), and a point on another wire's end is joined to it. So placing or moving a part whose pin lands on a wire joins it, in the same undo step. Every view, ERC, the netlist and Update PCB read this one stored topology.
- **Wires that only cross do not connect.** Put a junction point on the crossing to join them.
- **A wire is a path** from its first end through its corners to its second end ([schematic wires](schematic-wires.md)). A corner is not a connection point: nothing joins at a corner except an end lying on it.
- **A wire end on another wire's middle or corner** (`schematic.connect` or `wire.draw` to `{wire, at}`, or a free end lying there) splits that wire at a new junction point and joins it, with a dot. Free ends of two wires that meet join the two into one path; three or more meet at a junction point. A free end on a pin, label, sheet pin or junction attaches to it.
- **Junction points settle after every edit.** One left with two wires (after a wire is removed, or a placed point removed) becomes a corner of the one path the two merge into. One left with a single wire leaves that wire a free end, which ERC reports (`erc.wire_end_free`).
- **`wire.draw { sheet?, from, corners, to }`** draws a wire along the given path, each end a target or `{"free": [x, y]}`. It does not route around anything: connection points it runs over join it. A label placing itself on a pin stands for that pin, so naming it leaves it on the pin. The desktop's Wire tool sends one per wire.
- **Junction points** (`junction.place { sheet, position }`, the Junction tool, J) go on a wire or on a crossing and join every wire there; one placed on a lone straight wire shows as a dot, ready for a wire to end on it. `junction.remove` takes one away: with one or two wire ends it is no longer placed and settles as above (two wires become one path); where more pass through it, each pair leaving it in opposite directions becomes one path (a crossing stops joining). At a T it is refused: remove a wire instead. Off every wire, or on an existing connection point, placing is refused.
- **In the desktop**, J (or Junction on the ribbon) arms the Junction tool, whose card reads "Junction point". A click on a wire or a crossing places the point, drawn as a dot; a click on no wire places nothing and a toast says so.
- **The Wire tool** (W) draws corner by corner: a click on a pin, label, sheet pin, junction, free end or wire starts or ends the wire there (a click on a wire's middle makes a T); a click in empty space starts with a free end or fixes the 90° elbow (horizontal first; F flips it) and its click point as corners. A switches arc corners on or off: new corners are rounded by the card's Radius (2.5 mm by default), each as large as fits. Double-click or Enter ends the wire at the last fixed point with a free end; Esc does the same, or cancels a wire with no corner; Backspace takes back the last corner. While drawing, connection points the path would run over are ringed, since they will join it. Nothing is committed while drawing.
- **Editing a wire** (Select tool): a click selects the whole path and shows a square at each corner and free end and a dot at each segment's middle. Drag a corner or segment to reshape it, its neighbours staying horizontal or vertical; double-click a segment to add a corner, or a corner to remove it; Delete removes a picked corner, else opens the disconnect review for the wire; ⇧Delete splits the wire at the segment under the pointer; drag a free end to move it, or onto a pin, label, sheet pin, junction, wire or free end to attach or join there. A rounded corner shows a dot in the middle of its arc: dragging it sets the radius. A rounds the picked corner, or makes it sharp, and the card's Radius field sets it. The commands are `wire.set_path`, `wire.move_corner`, `wire.move_segment`, `wire.insert_corner`, `wire.remove_corner`, `wire.set_radius`, `wire.remove_segment` and `wire.move_end` ([schematic wires](schematic-wires.md#3-commands)). A corner or segment landing on a connection point joins it, as any path does.
- **Drawing a wire that joins nets** applies at once, one undo step, and a toast says what changed ("Drew the wire: …", with Undo). The update review opens only for a risky join (`UpdatePlan::needs_review`): one that touches routed copper (the plan has blockers, conflicting tracks or vias), or one that merges named nets (below). Joining VBUS to GND by a stray wire therefore asks first; joining a pin to a labelled net does not.
- **Named nets never merge silently.** Two nets that each carry a name of their own (a label, power port or sheet pin, `nets::name_sources`) and share no name merge only through a reviewed update. The rule is in core (`nets::named_merges`), so every edit from the desktop, CLI or MCP follows it, whatever joins the nets: a label or power port placed or renamed, a wire end, a junction, a paste, a sheet pin or a moved symbol. Sent directly, the edit is refused (`net.named_merge`), naming both nets and what joins them, and nothing changes. `project.preview_update` lists the merge in `merges_named` and as a warning ("Joins VBUS and GND into one net, GND"), and `project.apply_update` with `merge_named_nets: true` applies it. The merged net keeps the routed net's ID, or else the oldest. In the desktop, a refused edit opens that review, and its Apply asks for the merge. Joining an unnamed net to a named one, or two nets of the same name, stays direct.
- **Dots** mark where three or more conductors meet (a pin counts as one), and every junction point placed with `junction.place` (stored as `placed`). Corners and free ends have none.
- **A new wire never connects by accident.** `schematic.connect` (and the wire `label.place` draws to its attach target) takes the horizontal-first L when it runs over no connection point but its own ends. Otherwise it takes the vertical-first L, else the shortest three-legged dogleg whose middle leg runs up to 30 mm beyond either end on a 2.5 mm pitch. Its bends are corners of the one wire, and none may lie on another wire, where it would read as a T. When no such path exists the command is refused, naming the point in the way, and the document is unchanged.
- **Existing documents.** A document saved before this rule may hold wires drawn over pins they did not connect to. The first edit normalises it, and that can join nets. On routed nets, the edit is refused like any other change to routed connectivity, until the update is previewed. Both shipped examples were rebuilt so their drawings are honest and their nets are unchanged (`examples/esp32-s31-devboard/nets.json` holds that netlist, and a test compares against it).

## Labels

A label (`label.place`) is a named connection point: wires end on it like on a junction, and its name joins it to other labels. Labels never join one another by drawn position. Like any connection point, though, a label lying on a wire connects to that wire (see above).

| Kind (`kind.type`) | Joins |
| --- | --- |
| `local` | Local and hierarchical labels of the same name on the same sheet |
| `global` (off-sheet connector; `shape` input/output/bidirectional/passive) | Global labels and power ports of the same name on every sheet |
| `hierarchical` (`shape`) | Labels of the same name on its sheet, and the sheet pin of that name on the sheet symbol that uses its sheet |
| `power` (`style` ground/bar/arrow) | Like `global`: GND, +3V3, VBUS … are one net across all sheets |

Names are net names (1–64 bytes, no spaces) and compare ASCII-case-insensitively, as net names do. `label.place` may `attach` the label: to `{"wire", "at"}` (the label sits on that point and splits the wire there; at a wire end it is wired to that end), or to a terminal, junction, label or sheet pin (a wire joins them). `label.move` moves it (wires follow), `label.edit` renames it, changes its kind or turns it, `label.remove` removes it (wires through it stay joined at a junction left in its place, which settles as above: two wires become one path, and a single wire keeps a free end there).

### Placement on a pin

A label or power port placed with `attach` to a terminal places itself; nothing of that placement is stored. While it is not `manual` and its only wire goes to a pin on its sheet (`schematic::labels::attached_pin`), its connection point is that pin's end (`labels::anchor`, which every wire end, drag and hit test uses), so the wire to it has no length and is not drawn. It faces away from the symbol along the pin, from where the pin meets the body to its end, after the occurrence's rotation and mirror (`labels::outward`): a flag or tag runs outward on the pin's line with its name reading left to right or bottom to top, and on a vertical pin a supply port's bar or arrow or a ground's bars lie beyond the pin's end. On a horizontal pin a power port stands upright on the pin's end instead, a ground pointing down and a supply up, where that is clear; on a dense row, where its stem would run through the next pin, it lies beyond the pin's end like the others. `schematic::scene::sheet_labels` is the one placement the canvas, `schematic.scene` and the PDF draw from. It places the self-placing labels of a sheet row by row. Each stands off its pin along a wire-width leader in 2.5 mm steps (up to six) until its ink, with a 0.2 mm gap, clears every symbol's body, texts, pin names, numbers and stubs, and every label already placed. On a dense row of ground ports, whose 3 mm bars are wider than the 2.5 mm pitch, the ports alternate between two depths. A port pointing up or down from a row whose names would reach across each other reads its name along its stem. Label text is measured at 0.95 cap heights per character (`scene::LABEL_METRIC`, Helvetica capitals as printed), wider than the core metric's 0.6, so flags fit their names and the clearance holds in print. Placement follows the pins: moving or turning a symbol moves its labels, and an existing document is placed this way when it is drawn.

When nothing along the pin is clear (a label or port faces it across a short gap, as an MCU pin and a capacitor's GND port can), the label steps aside instead: its leader runs a fifth to four fifths of a module beyond the pin's end, then turns, a module at a time to either side (up to six), until the label clears everything above. A power port turns with its leader, so its stem runs aside and its symbol ends it; a ground tries down first, a supply up. The leader is drawing only; the connection point stays on the pin's end. Four labels on adjacent pins facing each other across a gap narrower than they are can still overlap.

A person's placement wins: `label.move` and a `label.edit` that turns the label set `manual` (omitted from the file when false). A moved label keeps the way it faced and stays where it was dropped, with its wire drawn to the pin. A turned label stays on the pin's end at the new rotation. A label placed free or on a wire is `manual` from the start. There is no command yet to hand a manual label back to automatic placement.

Label, port and sheet-pin text is 1.0 mm, the pin name and number height (`library::graphics::PIN_TEXT_HEIGHT_MM`). Every net label has an outline 2 mm tall, so outlines on adjacent 2.5 mm pins keep a gap and a name's underscores clear the edge: a global label is the pointed flag of its port shape (a square-cornered box when passive), a local label a tag with rounded corners, both stroked at the sheet's line width like power ports. The name runs along the outline, centred.

A pin name or number longer than its pin reaches past the pin's end. A self-placing label starts beyond it, on a leader that continues the pin, so the number sits over a line and the port or flag starts after it.

Net identity is unchanged: nets are terminal groups with persistent IDs (`nets::reconcile`); labels, power ports and sheet pins only add connections. The PCB sees the same `Net` objects, so net classes, explicit class assignments, board sync and routing protection work across sheets unchanged. A label edit that changes a routed net's membership is refused like any direct edit; preview it through `project.preview_update` (`Change::Schematic`).

### Which name wins

A net with labels or sheet pins takes its name from them (`nets::names`). The first of them wins, ordered by kind (power port, then global label, then local label, then hierarchical label or sheet pin), then by the sheet nearest the top of the hierarchy, then by the earliest page, then alphabetically.

A local or hierarchical name that another net also carries (two sheets each with a local `EN`) is qualified with its sheet's name, `Power/EN`; anything still colliding gets `~<net id>`. A net without labels keeps its stored name, unless a labelled net now carries it, when it falls back to `N<id>`. When a net loses its last label it keeps the name it had.

Rename propagation: `schematic.net_rename` on a labelled net renames every label and sheet pin of that net that spells its winning name (so a sheet pin and the hierarchical labels behind it rename together); it is refused if another net already has the name, so renaming never merges nets by accident. Renaming one label (`label.edit`) can deliberately join it to another net of that name. ERC `erc.label_conflict` warns when a net carries two different global names, or two different local names on one sheet.

## Symbol text: designator and value

A symbol's value slot (a `ComponentName` text, which every generated symbol has under its body) shows the instance's value, never a description: the first field the component shows on the symbol that has a value (`instances::value_text`, "100 nF", "10 kΩ"; the BOM's value column reads the same function), else the chosen Part's MPN, else the component name for any part but a two-terminal one: it identifies an IC ("AP2112K-3.3") or a rail symbol ("GND"). A two-terminal part with no value shows none: "Chip capacitor 0603 (1608 metric)" is a description. The value is drawn at `VALUE_TEXT_HEIGHT_MM` (1.0 mm, smaller than the 1.25 mm designator) and is not stacked a second time below the symbol. A symbol turned a quarter keeps its designator and value upright and beside the body, reading away from it, instead of along its pins where they ran over a neighbour's pins and labels.

Generated IC boxes are as wide as their pin names (`samples::fit_pin_names`): on each row the left and right names, measured as printed (0.95 cap heights per character), their offsets and a half-module gap fit inside, and the box grows by whole modules on both sides so pins stay on the grid. A pin table import into a component draft widens its symbol drafts in the same step when the new names no longer fit, and Redraw as › IC box redraws an existing symbol to fit; a published symbol changes only through a new revision, as always.

## Power ports

A power port is a label of kind `power`: global scope, drawn as the ground bars or a supply bar or arrow (the shapes of `library::symbol_style`'s ground and power kinds). It adds no component instance, no BOM line and no footprint. It names its net with the highest priority, so `GND` stays `GND` whatever local labels say. ERC: a net with a power port, power inputs and nothing that can supply it (no power-output, output, bidirectional or passive terminal) is `erc.power_port_undriven`, reported once per net instead of once per power input.

## Sheets and hierarchy

`sheet.add`, `sheet.rename`, `sheet.set_page` (A4, A3, A2, A1, A0, Letter, Legal, Tabloid; landscape or portrait), `sheet.reorder` and `sheet.delete` (removes what is drawn on the sheet, returns its units to unplaced and removes the sheet symbol that used it; the last sheet stays). Sheet names are unique case-insensitively. `component.place` and `symbol.place_unit` take an optional `sheet`; `symbol.move_to_sheet` moves an unwired unit (a wire cannot cross sheets: join sheets with labels).

A sheet symbol (`sheet_symbol.place`, with an existing `child` or a new sheet) stands on a parent sheet for a child sheet. Sheet pins (`sheet_pin.add`/`edit`/`remove`, on the left, right, top or bottom edge) connect to the child's hierarchical labels of the same name; `sheet_symbol.sync_pins` adds a pin for every hierarchical label name the symbol lacks. Sheets without a parent symbol are top-level sheets, so a flat multi-sheet design (sheets joined by global labels and power ports) and a hierarchical one use the same model. The hierarchy is acyclic and every sheet has at most one parent symbol.

**Not supported yet: reusing one child sheet under several sheet symbols.** That needs per-path component instances (R1 on one copy, R101 on the other) and per-path net identity, which the one-instance-one-designator model does not have; `sheet_symbol.place` refuses a sheet another symbol already uses. Copy the sheet's contents instead.

## Annotation

`schematic.annotate { order, mode, numbering, sheets }` numbers designators across sheets in page order, then by position on the sheet at each instance's first placed unit (`rows`: top to bottom then left to right; `columns`: left to right then top to bottom); unplaced instances come last. An annotated designator is the component's prefix plus a number without leading zeros (`R12`). Mode `unannotated` (default) numbers only the others (`R?`, `R`) with the lowest free numbers, so existing designators stay stable; `all` renumbers every visited instance. `numbering: sheet_hundreds` starts each page at 100 × page + 1. The outcome lists every change.

## Copy and paste

The query `schematic.copy { sheet, objects }` reads symbols, labels, drawings and wires into a self-contained clip, which keeps what it needs after the objects are gone (a cut). A wire comes along when it is named or when both its ends are on named symbols or labels; an end on anything else becomes a free end where it is. `schematic.paste { sheet, clip, offset }` recreates the clip as one undo step. A symbol becomes a new instance of the same component with the next free designator ([annotation](#annotation)) and keeps its field values and other instance choices, with its units on that sheet; units elsewhere stay unplaced. A label keeps its name, so it joins that net as any label does. Sheet symbols and sheet pins are not copied.

The desktop keeps a clip as its buffer. ⌘C and ⌘X work on the selection, one object or a group. ⌘V pastes at the pointer; off the canvas, or again at the same point, it moves on two grid steps from the last paste. What was pasted stays selected. Cut removes as Delete does, so a symbol, wire or group goes through the review. A focused text field keeps these keys. PCB Layout has no copy or paste: parts come from the schematic.

## Selecting several objects

With the Select tool, a drag from empty canvas draws a rectangle, by Solidcraft's rule: left to right takes what is wholly inside, right to left what it touches. ⇧ adds a rectangle to the selection, and ⇧-click adds or removes one object. ⌘A takes everything on the sheet when no text field has focus. Symbols (with all their units on the sheet), labels, power ports, drawings and wires can be selected; sheet symbols and pins cannot join a group.

`schematic.arrange { sheet, objects, action }` moves (`by`), turns (`about`) or mirrors (`about`) the group as one undo step; the desktop turns about the middle of the group's pins and vertices, snapped to the symbol grid. A wire with both ends in the group keeps its shape, and any other wire on a moved object rubber-bands. An end is in the group when it is on a named symbol or label, on a label placing itself on a named symbol's pin, on a junction a named wire ends on, or a free end of a named wire. So junctions come along with wires only. A group's Delete is the reviewed update `Change::RemoveObjects { objects }`, listing everything it removes.

## Navigation and queries

`schematic.sheets` lists the sheets in page order with parent symbol, depth and counts. `schematic.net_places { net }` lists where a net appears across sheets (labels, sheet pins, placed terminals, with sheet and position), which the desktop uses to jump between a label's other occurrences. `schematic.scene { sheet }` returns everything a sheet draws (page frame and title block, labels, ports, sheet symbols, furniture, wires, dots, no-connect crosses and units) as shapes and text in millimetres (`schematic::scene`); the canvas and the PDF export draw from it.

## Sheet furniture and title block

`drawing.add`/`edit`/`remove` place notes (multi-line text, 0.5–50 mm), lines and rectangles; they are never electrical. Every sheet has a page frame from its paper size: an outer and inner border with zone references (numbers across, letters down, about 50 mm apart) and a 110 × 32 mm title block in the bottom-right corner showing the title (the project name when empty), company, author, date, revision, "Sheet n of m", paper size, sheet name and project name. `project.set_properties` sets the title-block fields, and `project.rename` the project name (the Sheet card's Project field; one undo step, and the name fabrication outputs carry).

## ERC

| Code | Severity | When |
| --- | --- | --- |
| `erc.label_unconnected` | warning | A label or power port has no wire |
| `erc.wire_end_free` | warning | A wire end is attached to nothing (group "Wire ends not connected"; fix: connect the end or remove the wire). It names the wire (`ObjectRef::Wire`), so Locate selects it and frames the end |
| `erc.global_label_single` | warning | A global label's name appears on no other global label or power port |
| `erc.hierarchy_mismatch` | warning | A sheet pin has no hierarchical label of its name on the child sheet, or a hierarchical label has no sheet pin on its sheet's symbol |
| `erc.label_conflict` | warning | A net carries two different global names, or two different local names on one sheet |
| `erc.power_port_undriven` | warning | A net with a power port and power inputs has no possible source |

The existing net rules (driver conflicts, input-only nets, undriven power inputs) run over the cross-sheet nets.

## Printing

`pcbcraft-io::schematic_pdf` writes a vector PDF through the shared PDF canvas `pcbcraft-io::pdf` (pdf-writer, MIT/Apache-2.0; also used by the fabrication drawings) with one page per sheet at its paper size, frame and title block included, from `schematic::scene`. A selection of sheets prints in page order. A sheet symbol is a link to its child sheet's page. Exposed as the CLI `export-schematic-pdf`, the automation request `schematic.export_pdf` (an external effect, not an undo step) and File › Export schematic PDF… in the desktop.

## Follow-ups

- Buses (bus wires, bus entries, vector names such as `D[0..7]`) are not implemented.
- Reusable child sheets instanced more than once (see above).
- Labels do not yet draw with per-label font size or justification overrides.
- Sheet symbols and sheet pins cannot join a group selection yet, as they cannot be copied.
- Arrow keys do not nudge sheet objects, one or several.
- A pasted object does not follow the pointer until clicked; it is pasted in place and selected.
