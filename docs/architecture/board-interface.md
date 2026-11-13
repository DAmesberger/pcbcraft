# Board interface

Implementation contract, slice B0, 12 November 2026. This document says how a board publishes the datums a linked design may reference (any other SolidCraft design or assembly that links the board; an enclosure is one example), and how those references stay stable. It records the approved design (the architecture analysis of 12 November, Part 2, sections 2.2 to 2.4, approved by the founder with every recommendation) as types, commands, findings and file formats. The founder's decisions that shape it:

- The slot interface replaces "the published design is the full board" and the persistent board design.
- Datum solids in the linked design are acceptable for now, until Solidcraft links construction geometry.
- A connector's opening is package data, taken from the datasheet's panel cutout.
- The default export tier is the interface plus the mechanical bodies; copper on request.
- Live updates over SolidCraft's control channel are opt-in per link, and later (B7).
- Nothing Sparksmith-specific goes into Solidcraft; nothing is posted upstream.

**Status.** B0 (the contract), B1 (the datum model), B2 (package datums) and B3 (the exported design, the sidecar and the publish requests) are built. `Project::interface` holds the slot table; the automatic rules, the `interface.*` document commands and `interface.list` run through the one engine, so the UI, the CLI and MCP share them (`crates/editor-core/src/interface/{auto,apply,list}.rs`). The assembly link and the dry run (B4), the two-way check (B5) and the desktop UI (B6) are built (below).

**Slice B3 (the export) is built**: the core makes the sidecar a publish writes and the preview's diff (`interface::publish`); `sparksmith-solidcraft` builds the design from that sidecar (`interface`) and opens a linked design headlessly (`linked_design`); the automation session serves `interface.publish_preview` and `interface.publish`, and `sparksmith-io` puts the files in place (`interface`). Its gate runs over the ESP32 example through the JSON transport, with a headless SolidCraft assembly (`session::interface_publish_tests`): J1 moved 2 mm by its dimension previews as exactly one moved datum, "J1 opening, 2 mm along X"; the linked design's cutout follows by 2 mm ± 1 µm with no other wall face moved and the boss on mounting hole 1 unmoved; the sidecar is byte for byte the same after reopening and in another folder; deleting R4 (whose envelope slot comes before R5's), adding a board fill and renaming J1 leave every face of the linked design where it was; deleting J1 is reported. Publishing the interface tier of the ESP32 board took 82.7 ms (median of ten, the `ci` profile, on a shared machine; the 100 ms target is for release).

**Slice B4 (the assembly link) is built**: the project stores a link to a linked design (`model::assembly_link`), the `assembly.*` document commands keep it (`interface::assembly_link`), `sparksmith-solidcraft` reads a linked design in Sparksmith's own engine and runs the dry run (`linked_design`), `sparksmith-io` reads the linked design once into a scratch copy (`assembly_link`), and the automation session serves `assembly.preview`, `assembly.update` and `assembly.follow` and the dry run inside `interface.publish_preview` (`session/assembly_link.rs`). Its gate runs over the ESP32 example through the JSON transport with an original headless SolidCraft assembly test document (`session::assembly_link_tests`): deleting J1, the publish preview's dry run names ‘USB opening’ as failing before anything is written; `assembly.status` reads "broken reference" at once, the production check lists `assembly.reference_lost`, naming the cutout and J1, as a warning, the publish is refused until it is resolved, and undo restores the document (acceptance 6, as slice P1 decided it). The board's top edge, dimensioned 0.5 mm from the far wall's inner face, and SW2, held 7 mm from that edge, follow the wall moved 1 mm: `assembly.preview` lists "Board edge top, 1 mm along Y" and "SW2, 1 mm along Y", and `assembly.update` applies it as one undo step (acceptance 7). The dry run of J1 moved 2 mm names the feature that follows: "The linked design's ‘USB opening’ follows J1 opening: it moves 2 mm along X" (acceptance 2's second half).

**Slice B6 (the desktop UI) is built** (`apps/sparksmith/src/pcb_editor/{interface_card,interface_host,datum_paint}.rs`). The Board card has an Interface section: a status chip ("Not published", "Published · revision 3", "3 datums changed since revision 14", "Broken reference · publishing is blocked"), the published design with Publish…, the assembly link with Link… or Update…, the datums as a `kit::list` grouped Board, Holes and Parts (name, kind, slot, whether a rule exports it, a Lost, Inexact or Changed badge), then "Not exported": board parameters and package datums not marked for the linked design. A row's Export or Unexport action, Enter on a candidate and ⌫ on a user datum send `interface.export` and `interface.unexport`; the folded Automatic section switches the rules (`interface.auto`). The Placement card's Interface section marks the part's frame, envelope and package datums; a datum its package marks for the linked design reads "Exported automatically". Publish… and Update… dock the two previews in the subject slot as impact reviews: the publish preview's summary ("Revision 1 → 2 · 1 datum changed"), the dry run's failing features as errors each confirmed by "Publish anyway", features on unexported faces as warnings, the tier and the file, then the datums that change and the linked design features that follow; the linked design preview's board edges, holes and parts that move, its references, or why it would be refused. The previews, the publish and the linked design read run off the UI thread on a session over a fork of the window's engine, through the session's own code (`Session::interface_write`, `Session::assembly_update_command`, the `interface.publish_preview` and `assembly.preview` requests); what changes the document comes back as the one typed command the request records, applied on the window's engine as one undo step. Datums are drawn from `interface.list` by one painter over Layout's and 3D's board-to-screen mappings: while the Board card or a preview is open, every datum but the board's own frame, faces and outline; with a part selected, its datums; a part's datum carries a chip with its name; the hovered or selected row's datum is drawn in the accent colour. Its gate is the desktop's frame tests (`pcb_editor::interface_tests`), one per state, and a publish of the demo board's interface tier through the window's worker. VM screenshots are not taken yet.

**Slice B2 (package datums) is built**: the stored form, the part-file convention, the datum edits, the USB-C form's opening, the Datums section of the Component 3D tab, and the placement of a package's datums in the board frame (`interface::package`), which B1's `external_datums` rule and B3's builder read. Its gate test runs the example's USB4105 part file through `layout.place_part` on both sides at every quarter turn and at 33°, against the board Solidcraft builds (`sparksmith-solidcraft` `board::package_datum_tests`).

## Principles

1. An assembly references the board only through exported datums. Anything else in the published file is there to look at.
2. A datum's identity never depends on build order, feature numbers, designators or names.
3. Each value has one owner. The board owns its outline, holes and placements; the linked design owns its walls and openings.
4. Updates are reviewed and bound to revisions, in both directions. Never silent.
5. A reference that no longer resolves is reported in the user's words and blocks publishing to linked designs until it is resolved (slice P1; it does not block fabrication output).
6. Solidcraft stays generic: the design below uses only shipped Solidcraft commands and its own writer.

## Datum kinds

`library::datum::DatumKind`:

| Kind | Geometry (`interface::DatumGeometry`) | Example |
| --- | --- | --- |
| `frame` | origin and two unit axes; the third is `x × y` | the board origin; a part's placement frame |
| `plane` | origin and normal | board top and bottom; a connector's mating plane |
| `axis` | point and direction, optional diameter | a mounting hole; an LED's light axis |
| `profile` | closed loops of lines and arcs in a frame's x–y plane, the outer one first | the board outline; a cutout; an opening's outline |
| `volume` | a profile and a height along the frame's z | a part's envelope; the space a plug needs |
| `parameter` | value and unit (`mm`, `deg`, or empty for a count) | board thickness; an exported board parameter |

Geometry is in board millimetres: x and y on the board frame, z up from the bottom face. It is **derived, never stored**: the document stores identity only (the slot table below), and the geometry is computed from the board and its parts each time it is needed. Every coordinate is finite and within 10 m (`MAX_COORDINATE_MM`); a profile has at most 256 loops (`MAX_LOOPS`), each within the footprint outline segment bound.

## Roles, owners and tokens

A datum's **key** is its owner, its role and, for some roles, a name (`model::interface::DatumKey`). One key gives at most one live datum. `interface::check_key` holds which owner and name each role takes.

| Role (`DatumRole`) | Owner | Name | Kind | Token | Exported |
| --- | --- | --- | --- | --- | --- |
| `board.frame` | board | — | frame | `board.frame` | automatic (`board`) |
| `board.top`, `board.bottom` | board | — | plane | `board.top`, `board.bottom` | automatic (`board`) |
| `board.thickness` | board | — | parameter | `board.thickness` | automatic (`board`) |
| `board.outline` | board | — | profile | `board.outline` | automatic (`outline`) |
| `board.cutout` | cutout (stable id) | — | profile | `cut<cutout id>` | automatic (`cutouts`) |
| `hole` | board hole | — | axis with diameter | `h<hole id>` | automatic (`holes`) |
| `side.height` | board | `top` or `bottom` | volume | `side.top`, `side.bottom` | automatic (`side_heights`) |
| `part.datum` | instance | the package datum's name | declared by the package | `i<instance>.<name>` | automatic when the package marks it `external` (`external_datums`); otherwise by the user |
| `part.frame` | instance | — | frame | `i<instance>.frame` | by the user |
| `part.envelope` | instance | — | volume (courtyard × body height) | `i<instance>.envelope` | by the user |
| `board.parameter` | board | the parameter's name | parameter | `board.parameter.<name>` | by the user |
| `feature` | board or instance | — | of the feature | `d<datum id>` | by the user |

**Identity.**

- A datum has a document id, `DatumId`, allocated from the table's own counter (`InterfaceTable::next_datum`), as nets have their own, so exporting never shifts the ids of other objects (replays and recordings name them). It is never reused, through undo and redo too.
- A datum has a **slot**, `Slot`: its position in the board's append-only slot table, from 0. A slot is never reused. Removing a datum **retires** its slot; the slot counter only grows (`interface::allocate_slot`, `interface::retire`). A board holds at most 65,536 slots over its whole history (`MAX_SLOTS`) and 4,096 live datums (`MAX_DATUMS`).
- A datum has a display name for people, derived ("J1 opening") unless the user set one. It is never identity: renaming J1, or the datum, changes the display name only.
- The **token** written into the exported design and the sidecar is built from ids only (`interface::token`). Tokens are dot-separated segments of `[A-Za-z0-9_]`, at most 320 bytes. A sketch curve id that cannot be written so is refused (`interface.token`), never rewritten.
- A cutout's owner is the cutout itself, by its stable id (`DatumOwner::Cutout`, a `BoardFeatureId`; slice P1). Every inner loop of a solved board has a role with that id (`layout::outline::identify_cutouts`): a loop keeps its role's id while it keeps any of the role's curves (the evaluator matches it by its curves, or by a curve they share when a curve was replaced), a new loop gets a new id, and ids are never reused. The token (`cut<id>`) and slot do not depend on how the loop's curves are named.
- A feature datum's key has no name; the feature it exports (`InterfaceDatum.feature`, a `BoardRef` that is never a followed face) tells it apart, and one feature is exported at most once.
- Package datum names are `[a-z][a-z0-9_]*`, at most 32 bytes (`DatumName`); `frame` and `envelope` are reserved, because every placement already has datums with those tokens. A board parameter's name keeps its `ParamName` syntax.

`interface::check_table` holds the table's invariants, which document decode and every interface command will check: live datums in ascending slot order, every slot allocated, no slot both live and retired or used twice, unique ids and keys, valid keys and display names, a feature only on feature datums.

## How a connector's opening is defined

Three sources, in order of preference (slice B2):

1. **A datum authored on the package**, the stored form. A package revision holds its datums as explicit geometry in the model frame: name, kind, frame, profile, direction, an optional clearance and `external`. It is revisioned with the package and works for every model kind, STEP and glTF included. For a connector the opening is the aperture the mating plug needs, from the datasheet's panel-cutout drawing, not the shell's outline. `external: true` means "meant for the linked design": the `external_datums` rule exports it for every placement. The stored geometry uses the document's integer lengths (the units migration).

   As built (`library::datum::PackageDatum`, in `PackageDefinition.datums`, at most 32): a `name`, a `geometry` that is a frame, plane, axis (optional diameter), profile or volume, an optional `clearance_nm`, `external`, and an `origin` saying where it came from (`authored`, `part_file` or `form`). Lengths are whole nanometres and directions unit vectors; a frame's axes are perpendicular (within 1e-6). A profile is a rectangle centred on its frame's origin with rounded corners (`rect`, `size`, `corner_radius` up to half the smaller side, so a stadium is one); free loops can be added as another shape. A volume rises along its frame's z (`x × y`): for an opening, out of the part along the plug's way in. Bad datums are the publish finding `package.datum`; the edits refuse them.
2. **A datum declared in the part file**, an authoring convenience that produces form 1. A construction plane named `opening` or `opening-<suffix>` with two size parameters (or a sketch of that name, for a free profile) gives the package datum `opening` or `opening_<suffix>`; `axis` or `axis-<suffix>` gives `axis` or `axis_<suffix>` (`PART_FILE_OPENING`, `PART_FILE_AXIS`). This follows the `marking-<name>` and `pin1` convention of [part templates](../part-templates.md); the check that reads marking surfaces reads these.

   As built (`part_file::evaluate`, the table in [part templates §11](../part-templates.md)): the plane's x is the profile's x, its normal points out of the part; the lengths `<datum>_u` and `<datum>_v` size the rectangle, `<datum>_r` rounds its corners, `<datum>_depth` makes it a volume and `<datum>_clearance` records a clearance, each optional but the size. An axis takes its diameter from `<datum>_d`. Declared datums are external, and copied into the package when the part file is used (`origin: part_file`); using another model removes them, and a datum the user authored or edited of the same name is kept. A free profile from a sketch is not built yet. The example's USB4105 part file declares `opening`: the plug's shell, which fills the 8.34 × 2.56 mm opening with round ends, 1.85 mm deep, the drawing's mating view's distance before the plug's overmold begins.

   A **typed form** may emit datums too (`PackageForm::datums`, `origin: form`), from the same drawing values as its lands. The USB-C receptacle form emits that same opening (decision 5); it follows the form's values when the form is edited.
3. **A face of the part model**, ad hoc: the user exports a face reference as a `feature` datum, traced by the existing model-reference trace with its resolved, inexact and lost states. The weakest form: STEP faces are found by geometry.

An opening is never inferred from a part's name or category.

## What is exported

**Automatically,** each rule on by default and switched with `interface.auto` (`AutoRule`):

| Rule | Exports |
| --- | --- |
| `board` | the board frame, top, bottom and thickness |
| `outline` | the board outline |
| `cutouts` | every cutout and slot |
| `holes` | every mounting hole |
| `external_datums` | every package datum marked `external`, for each placed instance |
| `side_heights` | the tallest part per side |

Switching a rule off retires the slots of the datums it made; switching it on again allocates new slots. An automatic datum whose owner goes (a deleted mounting hole) is retired the same way; undo brings it back in its slot. A user datum whose owner goes stays, lost (`interface.datum_lost`), until it is unexported.

**When the table is stored.** A project stores no table until its first `interface.*` command, so a project that never uses the interface saves, reopens and undoes exactly as before. Until then `interface.list` shows the slots that first command will store; nothing outside the document can have seen them, since only a publish writes them out. From then on the engine keeps the table matched to the board after every change (`interface::auto::follow`), and after every undo and redo keeps the slot and datum counters at their highest (`interface::auto::carry`): an undone export's slot is retired, never handed to the next export, while an automatic datum the restored board still needs keeps its slot and id.

**By the user** (`interface.export`): a part's frame or envelope, any other package datum, any dimensionable feature ("Export to linked design" in the Properties card and context menu, in Layout and in 3D), and board parameters.

## Commands

Document changes are `interface::InterfaceCommand`s: typed, through the one engine, one undo step each, never opening a dialog. A refused command changes nothing. Writing files is an external effect, so publishing is a host request that reads the document, writes the files and then records the publish with a document command.

| Command | Kind | Class | Does |
| --- | --- | --- | --- |
| `interface.list` | query (`Query::InterfaceList`) | read | The datums with id, slot, token, display name, role, kind, source, geometry, status and whether each changed since the last publish; the rules; the retired slots; the last revision (`InterfaceList`) |
| `interface.export {target, name?}` | operation | edit | Exports a feature, package datum, part frame or envelope, or board parameter (`ExportTarget`); allocates a slot |
| `interface.unexport {datum}` | operation | edit | Retires the datum's slot; the publish preview names the linked design features that used it |
| `interface.rename {datum, name?}` | operation | edit | Sets the display name, or with no name returns to the derived one |
| `interface.auto {rule, on}` | operation | edit | Switches an automatic rule |
| `interface.record_publish {source_id, record}` | operation | edit | Records a publish the host wrote: revision, hash, tier, per-datum hashes, and the board's interface identity on the first publish |
| `interface.publish_preview {tier?, path?}` | host request | read | The diff against the last published revision and, with a linked design, the dry run (`PublishPreview`). `path` is the published design whose sidecar says where each datum was |
| `interface.publish {path, tier, expected_revision}` | host request | ask | Writes the design and sidecar atomically, then records the publish; refused, writing nothing, at a stale revision |
| `package.datum.add`, `edit`, `remove` | operation | edit | On a package draft (slice B2): the `add_datum`, `set_datum` and `remove_datum` edits of `library.edit_package`, in millimetres on the wire, any number of them with other package edits in one undo step. `set_datum` replaces the datum of its name and makes it authored. A repeated or unknown name, or bad geometry, refuses the batch unchanged |
| `assembly.preview {path, design}` | host request | read (ask with its paths) | What an update (or, with another file, a relink) would change: references, board edges, holes, parts and parameters that move, in the user's words, and `expect_hash` ([the assembly link](#the-assembly-link-slice-b4)) |
| `assembly.update {path, design, expect_hash?}` | host request | ask | Links the linked design when none is linked; else applies it at `expect_hash`, relinking when `path` is another file; one undo step |
| `assembly.follow {path, design, body, face, pick, name?}` or `{…, board_param, assembly_param}` | host request | ask | Follows a linked design face (a followed face) or takes a linked design length parameter (a binding); one undo step |
| `assembly.status` | query (`Query::AssemblyStatus`) | read | The link's state from the document (no linked design, broken reference, linked) and its findings |
| `assembly.record_link`, `record_update`, `record_reference`, `remove_reference`, `record_binding`, `unbind`, `unlink` | operation (`AssemblyCommand`) | edit | The document side of the requests above, and removing a reference, a binding or the link |

`interface.publish` takes `expected_revision`, the name every revision-bound request uses, for the design note's `expect_revision`. Its tier defaults to `mechanical`.

`interface.list` derives every datum's geometry from the document (`interface::list::derive`): board frame and faces at z = 0 and the stackup's thickness, the outline and cutouts as profiles, holes as axes with their drill, a part's frame on the surface it sits on, its envelope as its courtyard box times its package's envelope height, the tallest part per side over the outline, a board parameter's value when it is a number (an expression needs the host's evaluator, so its geometry is left out), and an exported feature's trace in the board as an axis (a point, circle or arc) or a plane (a line). A datum's content hash (`interface::list::datum_hash`) covers its token, role, kind, geometry and status, not its display name; `changed` compares it with the recorded publish.

The publish preview lists, per datum (`ChangeKind`): unchanged, moved (by an offset in millimetres and a turn in degrees), reshaped, added, retired, status changed, or changed (how is not known: the published sidecar was not there to compare with), each with the sentence the user reads (`summary`: "J1 opening, 2 mm along X"; "J1 opening, removed: a linked design feature that uses it will fail"). A datum compares against the recorded publish's hash; a move is read from the published sidecar's geometry, trusted only where its datum hash is the recorded one. A datum whose owner is gone reads under the name it was published with. The dry run (`DryRun`, slice B4) reports linked design features, by their SolidCraft names, that move, fail, leave a joint unsolved, or reference a face that is not exported. The user's linked design file is never touched.

**Publishing, as built** (`interface.publish`, B3): the request's `expected_revision` is checked before anything runs, so a stale one writes nothing. The core's `interface::publish::candidate` gives the sidecar (every live datum with its derived geometry and hash, in slot order; retired slots; live datums without geometry as `unresolved_slots`; the table hash and the revision). The board's `source_id` is drawn on the first publish; publishing over files of another board's interface is refused. `sparksmith_solidcraft::interface::build` makes the design and names each datum's faces in the sidecar; the design is saved to a hidden sibling file, the sidecar written atomically beside it, and the design renamed over the published one. Then `interface.record_publish` records the publish. Like a save, it is no undo step (slice P1): it makes a new document revision but leaves the undo history as it was, and undo and redo keep the record (`interface::auto::carry`), since the files stay written. The record keeps where the design was written (`PublishedRecord::design_hint`, relative to the project's folder when the design is inside it), so the next publish defaults there after a restart; the core and the session never open it. While a reference between the board and the linked design is broken, `interface.publish` is refused with `assembly.publish_blocked`, writing nothing, and the preview lists why (`PublishPreview::blocked`).

## The export format

**A `.solidcraft` design, written by Solidcraft's own commands and writer** (slice B3). Its timeline has two sections.

*Section 1, the slots, always first and always in slot order.*

- Each slot uses a fixed budget of feature ids, padded: four (`SLOT_FEATURE_BUDGET`). As built, a datum's carrier takes two (a sketch and its extrusion) and empty sketches made and deleted use up the rest; a retired or unresolved slot is all padding. Solidcraft never reuses a feature id. A carrier that needed more than the budget would fail the build rather than shift later slots.
- Because every slot's ids follow from its position, every datum's face names are the same in every build, on every machine and after every restart. No persistent native design is kept.
- A profile datum is a sketch whose curves carry explicit ids taken from the document's curve and hole ids, extruded once. Solidcraft names a side face after its sketch curve, so the board's edges and hole walls get stable names from one feature.
- How each kind is carried while only solids cross a link:

| Kind | Carrier |
| --- | --- |
| Board outline, cutouts, holes, top, bottom | one slab body `board`: its side faces, hole walls and caps |
| Opening | a prism body from the mating plane outward along the insertion direction: the space the plug occupies, usable as a cutting tool |
| Axis without a hole | a thin cylinder body |
| Volume | a prism body |
| Frame, parameter | the sidecar only, until parameters can cross a link |

Datum solids sit in a separate component named as reference geometry (`Board interface (reference geometry)`), so mass, interference and bills of materials can exclude them. As built:

- The slab `board` is the outline datum's carrier, in its slot, with every cutout and mounting hole of the board; its sides are named after the board sketch's curve ids (a rectangle's after its edges, `bottom`, `right`, `top`, `left`) and its hole walls after the hole ids (`F<n>:side:h1`). Board top and bottom are its caps (`F<n>:top`, `F<n>:start`).
- A datum's own body is named by its token (`i80.opening`, `i84.envelope`); its sketch curves are `e1`, `e2`, … so its sides are `F<n>:side:e1`, …, its cap on the frame's plane `front` (`F<n>:start`) and its far cap `back`. An axis's cylinder is 1 mm long (a hole without the slab: the board's thickness), 0.2 mm across when it has no diameter; a profile that is not the board's is a 0.01 mm prism.
- Every face the sidecar names is one the built body has (a degenerate segment, such as the zero-length side of a stadium, makes no face and is not listed).
- The sidecar is also in the design's metadata under `org.sparksmith.electronics.interface` (format and chunk count) and `org.sparksmith.electronics.interface.<k>` (`{"text": …}`, 30,000 bytes each).

*Section 2, optional, after the slots:* the mechanical board (laminate, pads, parts) or the full board (with copper). Its feature numbers may change freely; the publish preview warns when an assembly references anything in it.

**Tiers** (`Tier`):

| Tier | Contains |
| --- | --- |
| `interface` | the slots only |
| `mechanical` (default) | the slots, laminate, pads and parts |
| `full` | the slots and every copper body |

Section 2 of a mechanical or full publish is checked against the board plan of that tier's board before anything is written (stage A5, `sparksmith_solidcraft::export::check`, by geometry fit since G1); the reply's `published.check` lists every body that differs, as `project.export_native` does ([development](../development.md), "Exports"). Unlike that export, a publish is not refused for a body that does not fit: its files are written and the check reported.

**The sidecar,** `<name>.interface.json`, written beside the design, with the same content in the design's document metadata under a reverse-DNS key, in chunks under Solidcraft's 64 KiB value limit. Its schema is recorded in [board-interface.schema.json](board-interface.schema.json) (`sidecar`), with the stored table, the commands and the reports; a test fails when the recorded schemas and the types differ (set `SPARKSMITH_BLESS=1` to rewrite the file after a deliberate change).

```json
{
  "format": "sparksmith-board-interface/1",
  "source_id": "…32 hex digits, the board's permanent interface identity…",
  "revision": 14,
  "hash": "…SHA-256 of the canonical datum table…",
  "units": "mm",
  "tier": "interface",
  "datums": [
    {"id": 412, "slot": 2, "token": "i80.opening", "name": "J1 opening",
     "role": "part.opening", "kind": "volume", "owner": {"kind": "instance", "id": 80},
     "geometry": {"kind": "volume",
                  "frame": {"origin": [20, 0, 3.29], "x": [1, 0, 0], "y": [0, 0, 1]},
                  "loops": [{"start": [-4.5, -1.75], "segments": [{"line": {"to": [4.5, -1.75]}}, "…"]}],
                  "height": 8},
     "clearance_mm": 0.3, "status": "resolved", "hash": "…",
     "faces": {"front": "F7:start"}}
  ],
  "retired_slots": [3]
}
```

The design note's example spread a datum's frame and profile over the datum itself; the contract nests them in one `geometry` object tagged by kind, which the schema can describe exactly. A datum's `role` is the key's role, except that a package datum writes `part.<name>` (`part.opening`).

**Versioning and reading** (`interface::Sidecar::read`):

- `format` is the format name, `/` and a major version. A reader refuses a major it does not know (`interface.format_newer`) and anything that is not this format (`interface.format`). Fields may be added within a major, so the sidecar's types ignore unknown fields.
- Reading is bounded: at most 16 MiB (`interface.sidecar_size`), then `Sidecar::check` holds units `mm`, the datum and slot bounds, ascending unique slots disjoint from the retired ones, valid unique tokens, unique ids, printable names and roles, geometry that is finite, bounded and of the datum's kind, and at most 64 named faces per datum.
- `revision` counts publishes whose content hash differs, from 1. Each datum has its own hash, so a diff is per datum. The table hash covers the live datums' slots, tokens and hashes, the retired slots and the unresolved slots; display names, the tier and face names are not part of it, so renaming J1 or publishing another tier keeps the revision.
- `source_id` is drawn by the host (the core never generates randomness) on the first publish and kept. Renumbering slots, a deliberate compaction, starts a new `source_id` epoch, with an impact preview saying that every assembly reference must be picked again.

The sidecar is shaped to become the `board.json` part of the exchange file proposed upstream (RFC 0001): its `manifest.json` fields are `source_id`, `revision`, `hash` and `units`; its persistent face names are the slot faces; its reference outcomes are this design's statuses.

## The assembly link (slice B4)

The project stores at most one link (`Project::assembly_link`, `model::assembly_link::AssemblyLink`): where the linked design and the board's published design were (`assembly_hint`, `design_hint`), the linked design component that links that design and its name, the linked design file's SHA-256 when the board last read it, the interface revision it linked, the **followed faces** (`FollowedFace`: body, persistent face name, pick, display name, the trace in the board plane, status, note, and the board-sketch entity it owns), the **bindings** (`AssemblyBinding`: a board length parameter set from a linked design length parameter, with its value) and the **consumers** (`Consumer`: a linked design feature, by its name there, that uses a published datum, with the datum's id, token and published name). The link is the snapshot: the project opens, solves and checks with it when the linked design file is gone. Picks and bound values are whole nanometres; traces are `FeatureGeometry` like the model traces.

**Reading a linked design** (host). Every request names the linked design (`path`) and the published design (`design`), so the permission gate sees every file it reads; the stored hints are only defaults for the desktop and words for messages, and a request naming another design is refused. The linked design is read once, bounded at 512 MiB, hashed and copied into a scratch folder (`sparksmith_io::assembly_link::copy`), and the copy is opened read-only in Sparksmith's own engine (`LinkedDesign::open`); the user's file is never written. A file a newer SolidCraft saved is refused in the user's words (`assembly.newer`), a missing one too (`assembly.missing`: "The linked design file was not found …; the board uses the last known geometry"). The component that links the board is found by its link resolving to `design` (relative links against the user's folder, not the copy's), never by name; a board placed twice is refused. The board frame is that occurrence's world transform, inverted.

- A followed face is traced with the model-reference trace (`model_refs::trace_linked_face`): a face square to the board is its line in the board plane, a vertical cylinder its axis point; anything else is refused. A name that follows the linked design's history is found where the face is now, and its pick moves with it; a name without verified history must still hold the stored pick, or it is inexact.
- Consumers are the linked design sketches whose links project a datum's body (named by its token) or a face of the board slab that the published sidecar lists for a datum; a slab face is matched by SolidCraft's digest of the published face name (`F<feature>:link:<FNV-1a>`).

**Followed faces in the board sketch.** `assembly.follow` with a face adds the trace to the board sketch as fixed geometry the reference owns (`BoardShapeEvaluator::fixed_geometry`): a construction line between two fixed points, or a fixed point. The sketch's own edits cannot move it; board curves are dimensioned to it with the board sketch's own dimensions (`board.edit`), and parts dimensioned to those curves follow, as for any board edit. Only an update moves it. A trace that changes shape, or a face that is gone, makes the reference lost; it keeps its last geometry and stops moving. Removing the reference removes the geometry and every dimension to it; unlinking leaves it as plain fixed geometry, so the board keeps its shape.

**Updates are reviewed.** `assembly.preview` applies the read to a copy of the document (`Engine::fork`, `assembly.record_update`) and diffs the two (`interface::assembly_link::diff`): every reference unchanged, moved or lost; every named board edge moved or reshaped, mounting hole and part moved, and bound parameter changed ("Board edge top, 1 mm along Y", "SW2, 1 mm along Y"); consumers added or gone; or why the update would be refused (the board would not solve). When both files changed since they last agreed, it also lists what the board changed (`board_changes`: each datum the link uses whose content changed since the publish the linked design last agreed with, `AssemblyLink::agreed`) and the items both changed (`conflicts`: a followed face the linked design moved that is built on a datum the board changed, or a bound parameter both changed), and shows the update with the linked design winning each. It reports the file's hash as `expect_hash`; `assembly.update` reads the file again and applies it only at that hash, one undo step, and is refused, changing nothing, otherwise ("the linked design changed since the preview; preview it again"). Updating without a hash once a link exists is refused. A relink is an update from another file, previewed there.

**The dry run** (`interface.publish_preview {path, assembly_path}`). With the linked design named, the candidate interface design is built into the scratch folder and the copy's board link pointed at it (`component.relink`, in memory); the copy is evaluated and compared with itself before (`LinkedDesign::dry_run`). It reports, in the linked design's own names, the features that newly fail (with the datum they use when it is one Sparksmith published: "The linked design's ‘USB opening’ uses J1 opening, which this publish removes, so it would fail"), the joints `component.relink` reports unsolved, and the features that move: a feature moves when one of its faces leaves its plane (a wall face that only slides round a moved hole does not count), by the largest shift of its faces ("The linked design's ‘USB opening’ follows J1 opening: it moves 2 mm along X").

**Broken references block publishing** (the hard rule, as slice P1 decided it). `interface::assembly_link::checks`, part of the project's issues, raises:

- `assembly.reference_lost` for a consumer whose datum is no longer resolved on the board (retired, lost, or missing from the last publish): "The linked design's ‘USB opening’ uses J1 opening. J1 was deleted, so it would fail. Restore J1, or pick another reference in SolidCraft";
- `assembly.reference_lost` and `assembly.reference_inexact` for followed faces the last read found lost or inexact.

Both are warnings in Issues at once, and neither blocks fabrication or Gerber output. A lost reference (`assembly.reference_lost`, in either direction) blocks publishing to the linked design until it is resolved: `interface.publish` is refused with `assembly.publish_blocked`, naming it, and its preview lists it (`interface::assembly_link::publish_blockers`); the desktop's publish review shows it as an error nothing acknowledges. An inexact one blocks nothing. `assembly.status` reads "broken reference" while one exists.

## The two-way check (slice B5)

**Consumers by slot.** An assembly reference is classified by what carries it in the published design (`sparksmith_solidcraft::linked_design`, `Slots`): a datum's own body (named by its token), a face the sidecar lists for a datum (by SolidCraft's digest of the published face name), or any face of the board slab, which the outline's slot builds. Each consumer stores the slot (`Consumer::slot`), and the board checks it by that slot, which is never reused (`interface::assembly_link::broken`): the slot retired, its datum lost, or the last publish no longer carrying it is a broken reference. A sketch, or a joint that holds the board's occurrence by such a face (`UseKind::Joint`), is a consumer. A reference to any other body of the board's design (section 2: parts, laminate, pads) is no slot's: the link records it as `AssemblyLink::unexported` and the board raises `assembly.unexported_face`; the publish dry run lists it as `unexported_reference`. SolidCraft names a placed body by its package's definition ("pkg15r1 body", and "pkg15r1 body (pkg15r1:2)" where the package is placed again), so the published sidecar maps each part body to its part (`Sidecar::parts`, `PartBody { body, instance, designator }`, built from the exact model's own record of which placement is which part, `NativeModel::part_bodies`; slice P1). The host stores the part with the use (`UnexportedUse::part`), and the finding and the dry run name the part's designator ("a face of C2"), the second of two parts sharing a package by its own designator; the SolidCraft body names stay package-based.

**The older-board check.** SolidCraft keeps the SHA-256 of a linked design file on the link (`DesignLink::file_sha256`). The host reads it with the linked design (`BoardLink::file_sha256`, stored as `AssemblyLink::board_sha256`), and a publish records the SHA-256 of the file it wrote (`PublishedRecord::design_sha256`); `interface.record_publish` keeps the files earlier publishes wrote (`PublishedRecord::earlier`, at most 64, oldest first, the board's own record: a host-supplied list is refused), so the board names the revision a linked design holds (`interface::assembly_link::agreement`). A republish with no datum changed keeps its revision whatever bytes it wrote. A link without a hash claims nothing. A linked design that holds an older board raises `assembly.older_board` ("The linked design still uses board revision 1. Run Update Links in SolidCraft, then check again"), an advisory. When the linked design's own file changed as well, `assembly.status` reads "both changed" (`assembly.both_changed`: "The board and the linked design both changed since they last agreed. Preview the update and choose which side wins where they conflict"). The update is not refused for that (slice P1): `assembly.preview` lists both sides' changes and the items both changed (`interface::assembly_link::conflicts`), and `assembly.update` (`assembly.record_update`) takes a side for each in `resolve` (`Resolution { item, wins: board | linked_design }`). The board winning keeps its trace or parameter as it is; the linked design winning takes its change. An update that leaves a conflict without a side, or names an item that is no conflict, is refused, changing nothing.

**The cycle graph.** Each followed face records the board datums its linked design geometry is built on (`FollowedFace::built_on`), read with the linked design (`LinkedDesign::built_on`): from the feature that made the followed face, back through the sketches, faces and bodies it is made from, to every reference a slot carries, and every joint that places the board (the board frame, and every trace with it, depends on those). That is the edge linked design cutout ← board datum. The edge board outline ← linked design sketch is the board's own: after every edit that changes the board, its parts, its datums or the link, the engine moves each reference built on a datum by a small probe on a copy, solves the board and its dimensioned parts again, and compares every datum (`interface::assembly_link::follow_cycles`). A datum that follows the reference and is one it is built on is a loop, stored as `AssemblyLink::cycles` and raised as `assembly.cycle` ("Board outline follows the linked design's Far wall · inner face, and the linked design builds ‘Far wall’ from Board outline. Break the loop: …"), an error that no waiver lifts. A loop through extents only is a warning, `assembly.extent_cycle` (slice P1): when the followed face is the start face of an extrude whose sketch lies on a fixed plane, and the datum reaches it only through that sketch's projections, the datum shapes the face's extent, never its position (`UseKind::Extent`), so the loop settles. A datum the face is also built on by position is a position use. Only references built on a datum are probed, so a link with none costs nothing.

**The driven edge.** A board edit that names the geometry a followed face owns to move, drag or delete it is refused before it runs, and one that changed it anyway is refused after (`interface::assembly_link::guard_edits`, `guard_driven`, with the evaluator's `fixed_geometry_of`): `assembly.driven_edge`, "This edge follows the linked design's wall. Break the link to edit it here". Only the `assembly.*` commands move it.

**Every row of the conflict table has a test.**

| Situation | Code | Test |
| --- | --- | --- |
| The linked design holds an older board | `assembly.older_board` | `b5_the_linked_design_holds_an_older_board_until_it_updates_links` (automation, ESP32); `b5_a_linked_design_holding_an_older_board_is_reported`, `b5_the_board_remembers_its_published_files` (core) |
| Both sides changed | `assembly.both_changed` | `b5_both_changed_shows_both_sides` (automation); `b5_both_changed_asks_for_the_preview` (core); `when_both_changed_the_user_chooses_which_side_wins` (`sparksmith-solidcraft`) |
| A datum the linked design uses is gone | `assembly.reference_lost` | `b4_deleting_j1_breaks_the_cutout_and_blocks_publishing` (automation); `b4_deleting_the_part_breaks_the_linked_design_reference`, `b5_consumers_are_checked_by_slot` (core); `a_broken_reference_holds_the_publish` (desktop) |
| A reference is on an unexported face | `assembly.unexported_face` | `b5_a_feature_on_an_unexported_face_is_named` (automation, mechanical tier, and core) |
| The board's driven outline was edited locally | `assembly.driven_edge` | `b5_a_board_edit_never_moves_what_the_linked_design_drives` (`sparksmith-solidcraft`); `b5_a_loop_between_board_and_linked_design_is_found` (automation) |
| A cycle | `assembly.cycle` | `b5_a_loop_between_board_and_linked_design_blocks_fabrication` (`sparksmith-solidcraft`); `b5_a_loop_between_board_and_linked_design_is_found` (automation, the linked design half: extent and position uses) |
| A cycle through extents only | `assembly.extent_cycle` | `an_extent_only_loop_is_a_warning` (`sparksmith-solidcraft`); `b5_a_loop_between_board_and_linked_design_is_found` (automation) |
| The linked design is missing or moved | `assembly.missing` | `b4_a_missing_linked_design_keeps_the_snapshot` (automation) |
| The linked design is from a newer SolidCraft | `assembly.newer` | `b5_a_linked_design_from_a_newer_solidcraft_is_refused` (automation) |

## Findings

`interface::findings` lists every code once (`CODES`) with whether it blocks fabrication output, whether a waiver may lift that, and whether it blocks publishing to the linked design. Messages are in the user's words, built by one function per code.

| Code | When | Blocks fabrication | Waivable | Blocks publishing |
| --- | --- | --- | --- | --- |
| `interface.unpublished` | "3 datums changed since revision 14" | no | — | no |
| `interface.datum_lost` | an exported datum's owner or feature is gone; it keeps its slot | no | — | no |
| `interface.datum_inexact` | an exported datum was found again by geometry | no | — | no |
| `assembly.stale` | the linked design changed since the board read it | no | — | no |
| `assembly.older_board` | the assembly links an older published board | no | — | no |
| `assembly.both_changed` | both changed since they last agreed | no | — | no |
| `assembly.reference_lost` | a linked design feature uses a datum that no longer resolves, or a followed face is lost | no | — | yes |
| `assembly.reference_inexact` | a followed face was found again only by geometry | no | — | no |
| `assembly.unexported_face` | a linked design feature uses a face that is not exported | no | — | no |
| `assembly.cycle` | the board and the linked design depend on each other's positions in a loop | yes | no | no |
| `assembly.extent_cycle` | the loop runs through extents only, and settles | no | — | no |
| `assembly.missing` | the linked design file was not found; the board uses its snapshot | no | — | no |
| `assembly.newer` | the linked design was saved by a newer SolidCraft | no | — | no |

A broken reference is a warning in Issues at once (slice P1): it is re-picked in SolidCraft, or what it used is restored, before the next publish; fabrication and Gerber output go ahead. A position loop is never waived. Slice B4 raises `assembly.reference_lost` and `assembly.reference_inexact` (above) and reports `assembly.missing` and `assembly.newer` as refusals of the request that reads the file; slice B5 raises `assembly.older_board`, `assembly.both_changed`, `assembly.unexported_face` and `assembly.cycle` (above), and `assembly.stale` with the file's hash. A local edit to a board edge that the linked design drives is refused with `assembly.driven_edge` ("This edge follows the linked design's wall. Break the link to edit it here."). Findings name the datum's owner (the instance, or the board), since `ObjectRef` has no datum variant; slice B6 decides whether locating a datum needs one.

## Where the contract lives

| Module | Holds |
| --- | --- |
| `library::datum` | `DatumKind`, `DatumName`, the reserved names and the part-file prefixes, shared by package datums and the interface |
| `model::interface` | the stored data, identity only: `InterfaceTable`, `InterfaceDatum`, `DatumId`, `Slot`, `DatumKey`, `DatumOwner`, `DatumRole`, `DatumSource`, `AutoRule`, `Tier`, `PublishedRecord`, `PublishedDatum`, and the bounds |
| `interface` (core module 11a) | the rules, geometry, commands, reports, sidecar, findings and schemas; `interface::publish`, the candidate sidecar, its record and the preview's diff |
| `sparksmith_solidcraft::interface`, `sparksmith_solidcraft::linked_design` | the exported design built from the sidecar; a SolidCraft assembly opened headlessly (link, Update Links, faces, failures) |
| `sparksmith_io::interface` | the sidecar's path and reading, the identity drawn on the first publish, the files put in place atomically |
| `sparksmith_automation` session (`session/interface.rs`, `session/assembly_link.rs`) | `interface.publish_preview` (with the dry run) and `interface.publish`; `assembly.preview`, `assembly.update` and `assembly.follow` |
| `model::assembly_link`, `interface::assembly_link` | the assembly link's stored data; its commands, findings, status and the update preview's diff |
| `sparksmith_io::assembly_link` | the linked design read once, hashed and copied into a scratch folder |
| `ids` | `InterfaceSourceId`, the board's random interface identity |

`interface` sits above `layout` and below `erc` in the [module map](../component-model.md#d-module-map), which `cargo xtask core-modules` enforces: it may use `geometry`, `ids`, `diagnostics`, `library`, `model`, `instances`, `layout` and `units`. `issues`, `fabrication`, `updates` and `commands` may use it.

## Slices and their tests

`crates/editor-core/src/interface/tests.rs` tests the contract's own rules. `interface/tests/skeletons.rs` holds each later slice's acceptance tests as ignored placeholders that fail; the slice removes the `ignore` and writes the body.

| Slice | Content | Placeholder tests |
| --- | --- | --- |
| B1 (built; `interface/tests/b1.rs`, and `session/interface_tests.rs` in the automation crate over the ESP32 example) | the table in `Project`, automatic rules, `interface.*` commands and `interface.list` in the engine, persistence | `b1_automatic_rules_export_the_board`, `b1_switching_a_rule_retires_and_never_reuses`, `b1_export_allocates_one_slot_per_datum`, `b1_refused_exports_change_nothing`, `b1_unexport_retires_the_slot`, `b1_renames_change_the_display_name_only`, `b1_slots_survive_undo_redo_save_and_reopen`, `b1_a_deleted_owner_makes_the_datum_lost`, `b1_selection_never_changes_the_interface` |
| B2 (built; `interface/tests/package_datums.rs`, and `board::package_datum_tests` in `sparksmith-solidcraft`) | package datums, the part-file convention, authoring, connector templates that emit openings | `b2_a_part_file_opening_reaches_the_board_frame`, `b2_external_datums_are_exported_per_placement`, `b2_no_opening_is_inferred_from_a_name` |
| B3 (built; `interface/tests/b3.rs` in the core on the demo board, `interface::tests` in `sparksmith-solidcraft`, and `session::interface_publish_tests` in the automation crate over the ESP32 example) | the exported design with slots and tiers, the sidecar and metadata, `publish_preview` (diff only) and `publish` | `b3_the_preview_lists_the_one_moved_datum`, `b3_the_linked_design_cutout_follows_by_two_millimetres`, `b3_the_sidecar_is_byte_identical_after_reopen`, `b3_unrelated_edits_change_nothing_in_the_linked_design`, `b3_publish_records_a_revision_only_when_the_hash_changes`, `b3_deleting_j1_is_reported`, `b3_publish_at_a_stale_revision_writes_nothing`, `b3_the_mechanical_tier_keeps_the_slots`, `b3_publishing_the_interface_tier_is_under_100_ms` (ignored; 82.7 ms measured in the `ci` profile, the target is for release) |

| B4 (built; `interface/tests/b4.rs` in the core on the demo board, `board::fixed_tests` in `sparksmith-solidcraft`, and `session::assembly_link_tests` in the automation crate over the ESP32 example with a headless SolidCraft assembly) | the assembly link, references, bindings, snapshot, preview, update, relink; the dry run | `b4_deleting_j1_breaks_the_cutout_and_blocks_publishing` (acceptance 6), `b4_the_board_follows_the_linked_design_wall` (acceptance 7), `b4_the_dry_run_names_the_feature_that_follows`, `b4_a_missing_linked_design_keeps_the_snapshot`; in the core `b4_deleting_the_part_breaks_the_linked_design_reference`, `b4_a_datum_missing_from_the_publish_is_broken`, `b4_lost_and_inexact_references_are_reported`, `b4_refused_link_commands_change_nothing`, `b4_the_link_survives_save_and_reopen`; `a_board_edge_follows_the_fixed_line_a_reference_owns` |

| B5 (built; `interface/tests/b5.rs` in the core, `board::fixed_tests` in `sparksmith-solidcraft`, and `session::two_way_tests` in the automation crate over the ESP32 example) | consumers by slot, the older-board check by the link's hash, the cycle graph, the driven edge | a test per row of the conflict table (above) |
| B6 (built; `pcb_editor::interface_tests` in the desktop app) | the Interface section, the Placement card's Interface section, the two docked previews, datums drawn in Layout and 3D | `the_interface_section_lists_datums_by_group_with_kind_and_slot`, `the_status_reads_published_then_stale_then_broken`, `a_row_marks_a_datum_for_export_and_unmarks_it`, `the_placement_card_marks_a_parts_frame_for_export`, `an_automatic_rule_is_switched_from_its_check`, `the_publish_preview_waits_then_shows_changes_and_the_dry_run`, `a_stale_or_failed_publish_preview_offers_to_compute_it_again`, `the_linked_design_preview_lists_what_moves_or_why_it_is_refused`, `publish_writes_the_design_off_the_ui_thread_and_records_it`, `datums_are_drawn_in_layout_and_3d_by_one_painter`, `selecting_a_row_selects_its_datum` |
| P1 (built; `interface/tests/p1.rs`, `b4.rs` and `b5.rs` in the core, `board::tests` and `board::fixed_tests` in `sparksmith-solidcraft`, `session::interface_publish_tests`, `assembly_link_tests` and `two_way_tests` in the automation crate, `pcb_editor::interface_tests` in the desktop app) | the link policy: broken references warn and hold the publish; both sides changed is chosen per item; a publish is no undo step and previews against the file on disk; extent-only loops warn; the project keeps where it published; cutouts have stable ids; the sidecar names the part of each part body | `b4_deleting_j1_breaks_the_cutout_and_blocks_publishing`, `a_broken_reference_holds_the_publish`; `when_both_changed_the_user_chooses_which_side_wins`, `b5_both_changed_shows_both_sides`, `the_linked_design_preview_lists_what_moves_or_why_it_is_refused`; `publishing_is_no_undo_step`, `the_publish_preview_compares_against_the_file_on_disk`, `b3_the_preview_lists_the_one_moved_datum`; `an_extent_only_loop_is_a_warning`; `the_project_keeps_where_it_last_published`, `the_project_keeps_where_it_published`, `the_last_publish_path_is_the_default_after_a_restart`; `a_cutout_keeps_its_id_as_its_curves_change`; `a_sidecar_maps_part_bodies_to_their_parts`, `a_body_of_a_shared_package_names_its_own_part`, `a_feature_on_a_repeated_package_names_its_own_part` |

B7 (live channel) and B9 (VM run) follow the design note.

## Open questions

- **Recording a publish in the undo history** (settled in B3, changed in P1). A publish, like a save, is no undo step. The preview compares against the published sidecar on disk, what the linked design sees, and against the record only when no file can be read.
- **Moved distances in the preview** (settled in B3). Read from the published sidecar given as `path`; without it a changed datum reads as `changed`.
- **The linked design feature that follows** (acceptance 2's second half; settled in B4): the dry run names it.
- **Fewer requests than the design note's five** (B4). The MCP tool list has a size budget (64 KiB) that B3 left almost full, so linking, updating and relinking are one request, `assembly.update`, told apart by whether a link exists and whether `path` is the linked file; following a face and binding a parameter are one, `assembly.follow`; and the status, which needs no file, is the `assembly.status` query. Unlinking and removing a reference or binding are document operations. Every request still says which file it reads.
- **The intersect section** of a followed face (`section` in §7.2 of the board-sketch note): only the projection along the board's normal is built.
- **Bodies named by their part in SolidCraft** (follow-up, slice P1). SolidCraft has no way to name an occurrence, so a shared package's bodies keep package-based names and the sidecar maps them to parts. A generic occurrence-name feature upstream would let the published design name them "J1 body" itself; nothing is opened upstream yet.
- **A datum in `ObjectRef`.** Findings name the owner for now (above). B6 did not need one: a datum is selected in its own list (`PcbEditor`'s interface state), lit in Layout and 3D, and a finding locates its owner.
