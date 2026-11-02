# Schematic capture: labels, power ports, sheets and printing

As built on 2 November 2026. This extends the single-sheet capture described in [editor evidence](editor-evidence.md) (symbol occurrences, wires, junctions) to designs of real size: net labels and power ports, several sheets with a hierarchy, sheet furniture with a page frame and title block, designator annotation across sheets, label/hierarchy/power ERC and vector PDF printing. Every change is a typed `SchematicCommand` applied through the one engine (`Operation::Schematic`), so the desktop, the CLI (`pcbcraft-cli apply`) and MCP (`project.apply`) share it; each is one undo step and is refused without changes on invalid input.

## Document

| Data | Where | Identity |
| --- | --- | --- |
| Sheets in page order, page size and orientation | `Project.sheets` (`Sheets { list, next }`) | `SheetId` (own allocation space; a new project has sheet 1, "Sheet 1", A4 landscape) |
| Title-block properties: title, revision, date, author, company | `Project.properties` | — |
| Labels and power ports | `Project.labels` (`Label { id, sheet, name, kind, position, rotation }`) | object `Id` |
| Sheet symbols and their pins | `Project.sheet_symbols` (`SheetSymbol { id, sheet, child, position, size, pins }`) | object `Id` (pins too) |
| Notes, lines, rectangles | `Project.drawings` (`SheetDrawing { id, sheet, item }`) | object `Id` |
| Sheet of a symbol occurrence or junction | `SymbolOccurrence.sheet`, `Junction.sheet` | — |

File format (still version 1, no migrations): every new field has a default and is omitted when it holds it (`sheets` when it is the single default sheet, `sheet` when it is sheet 1, empty lists), so a single-sheet design without labels writes exactly what it wrote before. Wire ends gain `{"label": id}` and `{"sheet_pin": id}`; a wire's sheet is that of its ends, and both ends must lie on one sheet. Sheet coordinates are millimetres with y up; every page has its top-left corner at (−30, 30) mm (`model::sheet::PAGE_TOP_LEFT`), so content near the origin, as single-sheet designs placed it, lies inside the frame.

## Labels

A label (`label.place`) is a named connection point: wires end on it like on a junction, and its name joins it to other labels. Nothing joins by drawn position.

| Kind (`kind.type`) | Joins |
| --- | --- |
| `local` | Local and hierarchical labels of the same name on the same sheet |
| `global` (off-sheet connector; `shape` input/output/bidirectional/passive) | Global labels and power ports of the same name on every sheet |
| `hierarchical` (`shape`) | Labels of the same name on its sheet, and the sheet pin of that name on the sheet symbol that uses its sheet |
| `power` (`style` ground/bar/arrow) | Like `global`: GND, +3V3, VBUS … are one net across all sheets |

Names are net names (1–64 bytes, no spaces) and compare ASCII-case-insensitively, as net names do. `label.place` may `attach` the label: to `{"wire", "at"}` (the label sits on that point and splits the wire there; at a wire end it is wired to that end), or to a terminal, junction, label or sheet pin (a wire joins them). `label.move` moves it (wires follow), `label.edit` renames it, changes its kind or turns it, `label.remove` removes it (wires through it stay joined at a junction left in its place; a single dangling wire goes).

Net identity is unchanged: nets are terminal groups with persistent IDs (`nets::reconcile`); labels, power ports and sheet pins only add connections. The PCB sees the same `Net` objects, so net classes, explicit class assignments, board sync and routing protection work across sheets unchanged. A label edit that changes a routed net's membership is refused like any direct edit; preview it through `project.preview_update` (`Change::Schematic`).

### Which name wins

A net with labels or sheet pins takes its name from them (`nets::names`). The first of them wins, ordered by kind (power port, then global label, then local label, then hierarchical label or sheet pin), then by the sheet nearest the top of the hierarchy, then by the earliest page, then alphabetically.

A local or hierarchical name that another net also carries (two sheets each with a local `EN`) is qualified with its sheet's name, `Power/EN`; anything still colliding gets `~<net id>`. A net without labels keeps its stored name, unless a labelled net now carries it, when it falls back to `N<id>`. When a net loses its last label it keeps the name it had.

Rename propagation: `schematic.net_rename` on a labelled net renames every label and sheet pin of that net that spells its winning name (so a sheet pin and the hierarchical labels behind it rename together); it is refused if another net already has the name, so renaming never merges nets by accident. Renaming one label (`label.edit`) can deliberately join it to another net of that name. ERC `erc.label_conflict` warns when a net carries two different global names, or two different local names on one sheet.

## Power ports

A power port is a label of kind `power`: global scope, drawn as the ground bars or a supply bar or arrow (the shapes of `library::symbol_style`'s ground and power kinds). It adds no component instance, no BOM line and no footprint. It names its net with the highest priority, so `GND` stays `GND` whatever local labels say. ERC: a net with a power port, power inputs and nothing that can supply it (no power-output, output, bidirectional or passive terminal) is `erc.power_port_undriven`, reported once per net instead of once per power input.

## Sheets and hierarchy

`sheet.add`, `sheet.rename`, `sheet.set_page` (A4, A3, A2, A1, A0, Letter, Legal, Tabloid; landscape or portrait), `sheet.reorder` and `sheet.delete` (removes what is drawn on the sheet, returns its units to unplaced and removes the sheet symbol that used it; the last sheet stays). Sheet names are unique case-insensitively. `component.place` and `symbol.place_unit` take an optional `sheet`; `symbol.move_to_sheet` moves an unwired unit (a wire cannot cross sheets: join sheets with labels).

A sheet symbol (`sheet_symbol.place`, with an existing `child` or a new sheet) stands on a parent sheet for a child sheet. Sheet pins (`sheet_pin.add`/`edit`/`remove`, on the left, right, top or bottom edge) connect to the child's hierarchical labels of the same name; `sheet_symbol.sync_pins` adds a pin for every hierarchical label name the symbol lacks. Sheets without a parent symbol are top-level sheets, so a flat multi-sheet design (sheets joined by global labels and power ports) and a hierarchical one use the same model. The hierarchy is acyclic and every sheet has at most one parent symbol.

**Not supported yet: reusing one child sheet under several sheet symbols.** That needs per-path component instances (R1 on one copy, R101 on the other) and per-path net identity, which the one-instance-one-designator model does not have; `sheet_symbol.place` refuses a sheet another symbol already uses. Copy the sheet's contents instead.

## Annotation

`schematic.annotate { order, mode, numbering, sheets }` numbers designators across sheets in page order, then by position on the sheet at each instance's first placed unit (`rows`: top to bottom then left to right; `columns`: left to right then top to bottom); unplaced instances come last. An annotated designator is the component's prefix plus a number without leading zeros (`R12`). Mode `unannotated` (default) numbers only the others (`R?`, `R`) with the lowest free numbers, so existing designators stay stable; `all` renumbers every visited instance. `numbering: sheet_hundreds` starts each page at 100 × page + 1. The outcome lists every change.

## Navigation and queries

`schematic.sheets` lists the sheets in page order with parent symbol, depth and counts. `schematic.net_places { net }` lists where a net appears across sheets (labels, sheet pins, placed terminals, with sheet and position), which the desktop uses to jump between a label's other occurrences. `schematic.scene { sheet }` returns everything a sheet draws (page frame and title block, labels, ports, sheet symbols, furniture, wires, dots, no-connect crosses and units) as shapes and text in millimetres (`schematic::scene`); the canvas and the PDF export draw from it.

## Sheet furniture and title block

`drawing.add`/`edit`/`remove` place notes (multi-line text, 0.5–50 mm), lines and rectangles; they are never electrical. Every sheet has a page frame from its paper size: an outer and inner border with zone references (numbers across, letters down, about 50 mm apart) and a 110 × 32 mm title block in the bottom-right corner showing the title (the project name when empty), company, author, date, revision, "Sheet n of m", paper size, sheet name and project name. `project.set_properties` sets the title-block fields.

## ERC

| Code | Severity | When |
| --- | --- | --- |
| `erc.label_unconnected` | warning | A label or power port has no wire |
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
