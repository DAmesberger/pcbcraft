# Board interface

Implementation contract, slice B0, 12 November 2026. This document says how a board publishes the datums an enclosure may reference, and how those references stay stable. It records the approved design (the architecture analysis of 12 November, Part 2, sections 2.2 to 2.4, approved by the founder with every recommendation) as types, commands, findings and file formats. The founder's decisions that shape it:

- The slot interface replaces "the published design is the full board" and the persistent board design.
- Datum solids in the enclosure are acceptable for now, until Solidcraft links construction geometry.
- A connector's opening is package data, taken from the datasheet's panel cutout.
- The default export tier is the interface plus the mechanical bodies; copper on request.
- Live updates over SolidCraft's control channel are opt-in per link, and later (B7).
- Nothing Sparksmith-specific goes into Solidcraft; nothing is posted upstream.

**Status.** Contract only. The types, rules that are complete without the engine, findings, schemas and test placeholders exist (`crates/editor-core/src/interface.rs`, `crates/editor-core/src/model/interface.rs`, `crates/editor-core/src/library/datum.rs`). No transport offers an `interface.*` command yet, and `Project` does not yet hold the table. Slice B1 adds both.

**Slice B2 (package datums) is built**: the stored form, the part-file convention, the datum edits, the USB-C form's opening, the Datums section of the Component 3D tab, and the placement of a package's datums in the board frame (`interface::package`), which B1's `external_datums` rule and B3's builder read. Its gate test runs the example's USB4105 part file through `layout.place_part` on both sides at every quarter turn and at 33°, against the board Solidcraft builds (`pcbcraft-solidcraft` `board::package_datum_tests`).

## Principles

1. An enclosure references the board only through exported datums. Anything else in the published file is there to look at.
2. A datum's identity never depends on build order, feature numbers, designators or names.
3. Each value has one owner. The board owns its outline, holes and placements; the enclosure owns its walls and openings.
4. Updates are reviewed and bound to revisions, in both directions. Never silent.
5. A reference that no longer resolves is reported in the user's words and blocks fabrication output.
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
| `board.cutout` | sketch curve | — | profile | `cut.<curve id>` | automatic (`cutouts`) |
| `hole` | board hole | — | axis with diameter | `h<hole id>` | automatic (`holes`) |
| `side.height` | board | `top` or `bottom` | volume | `side.top`, `side.bottom` | automatic (`side_heights`) |
| `part.datum` | instance | the package datum's name | declared by the package | `i<instance>.<name>` | automatic when the package marks it `external` (`external_datums`); otherwise by the user |
| `part.frame` | instance | — | frame | `i<instance>.frame` | by the user |
| `part.envelope` | instance | — | volume (courtyard × body height) | `i<instance>.envelope` | by the user |
| `board.parameter` | board | the parameter's name | parameter | `board.parameter.<name>` | by the user |
| `feature` | board or instance | — | of the feature | `d<datum id>` | by the user |

**Identity.**

- A datum has a document id, `DatumId`, allocated by `Project::allocate` like every other project object and never reused.
- A datum has a **slot**, `Slot`: its position in the board's append-only slot table, from 0. A slot is never reused. Removing a datum **retires** its slot; the slot counter only grows (`interface::allocate_slot`, `interface::retire`). A board holds at most 65,536 slots over its whole history (`MAX_SLOTS`) and 4,096 live datums (`MAX_DATUMS`).
- A datum has a display name for people, derived ("J1 opening") unless the user set one. It is never identity: renaming J1, or the datum, changes the display name only.
- The **token** written into the exported design and the sidecar is built from ids only (`interface::token`). Tokens are dot-separated segments of `[A-Za-z0-9_]`, at most 320 bytes. A sketch curve id that cannot be written so is refused (`interface.token`), never rewritten.
- A cutout's owner is the sketch curve that starts its loop. Sketch curve ids are never reused, so the token survives edits to the loop's other curves.
- A feature datum's key has no name; the feature it exports (`InterfaceDatum.feature`, a `BoardRef` that is never a housing reference) tells it apart, and one feature is exported at most once.
- Package datum names are `[a-z][a-z0-9_]*`, at most 32 bytes (`DatumName`); `frame` and `envelope` are reserved, because every placement already has datums with those tokens. A board parameter's name keeps its `ParamName` syntax.

`interface::check_table` holds the table's invariants, which document decode and every interface command will check: live datums in ascending slot order, every slot allocated, no slot both live and retired or used twice, unique ids and keys, valid keys and display names, a feature only on feature datums.

## How a connector's opening is defined

Three sources, in order of preference (slice B2):

1. **A datum authored on the package**, the stored form. A package revision holds its datums as explicit geometry in the model frame: name, kind, frame, profile, direction, an optional clearance and `external`. It is revisioned with the package and works for every model kind, STEP and glTF included. For a connector the opening is the aperture the mating plug needs, from the datasheet's panel-cutout drawing, not the shell's outline. `external: true` means "meant for the enclosure": the `external_datums` rule exports it for every placement. The stored geometry uses the document's integer lengths (the units migration).

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

Switching a rule off retires the slots of the datums it made; switching it on again allocates new slots.

**By the user** (`interface.export`): a part's frame or envelope, any other package datum, any dimensionable feature ("Export to enclosure" in the Properties card and context menu, in Layout and in 3D), and board parameters.

## Commands

Document changes are `interface::InterfaceCommand`s: typed, through the one engine, one undo step each, never opening a dialog. A refused command changes nothing. Writing files is an external effect, so publishing is a host request that reads the document, writes the files and then records the publish with a document command.

| Command | Kind | Class | Does |
| --- | --- | --- | --- |
| `interface.list` | query | read | The datums with id, slot, token, display name, role, kind, source, geometry, status and whether each changed since the last publish; the rules; the retired slots; the last revision (`InterfaceList`) |
| `interface.export {target, name?}` | operation | edit | Exports a feature, package datum, part frame or envelope, or board parameter (`ExportTarget`); allocates a slot |
| `interface.unexport {datum}` | operation | edit | Retires the datum's slot; the publish preview names the enclosure features that used it |
| `interface.rename {datum, name?}` | operation | edit | Sets the display name, or with no name returns to the derived one |
| `interface.auto {rule, on}` | operation | edit | Switches an automatic rule |
| `interface.record_publish {source_id, record}` | operation | edit | Records a publish the host wrote: revision, hash, tier, per-datum hashes, and the board's interface identity on the first publish |
| `interface.publish_preview {tier?}` | host request | read | The diff against the last published revision and, with a linked enclosure, the dry run (`PublishPreview`) |
| `interface.publish {path, tier, expected_revision}` | host request | ask | Writes the design and sidecar atomically, then records the publish; refused, writing nothing, at a stale revision |
| `package.datum.add`, `edit`, `remove` | operation | edit | On a package draft (slice B2): the `add_datum`, `set_datum` and `remove_datum` edits of `library.edit_package`, in millimetres on the wire, any number of them with other package edits in one undo step. `set_datum` replaces the datum of its name and makes it authored. A repeated or unknown name, or bad geometry, refuses the batch unchanged |
| `housing.link`, `housing.preview`, `housing.update {expect_hash}`, `housing.relink`, `housing.unlink` | — | — | The enclosure side, as designed in [board sketch references](../board-sketch-references.md#7-the-housing-round-trip) (slice B4) |

`interface.publish` takes `expected_revision`, the name every revision-bound request uses, for the design note's `expect_revision`. Its tier defaults to `mechanical`.

The publish preview lists, per datum (`ChangeKind`): unchanged, moved (by an offset in millimetres and a turn in degrees), reshaped, added, retired, or status changed. The dry run (`DryRun`, slice B4) reports enclosure features, by their SolidCraft names, that move, fail, leave a joint unsolved, or reference a face that is not exported. The user's enclosure file is never touched.

## The export format

**A `.solidcraft` design, written by Solidcraft's own commands and writer** (slice B3). Its timeline has two sections.

*Section 1, the slots, always first and always in slot order.*

- Each slot uses a fixed budget of feature ids, padded: four (`SLOT_FEATURE_BUDGET`). A retired slot is made and deleted, so it still uses its ids. Solidcraft never reuses a feature id.
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

Datum solids sit in a separate component named as reference geometry, so mass, interference and bills of materials can exclude them.

*Section 2, optional, after the slots:* the mechanical board (laminate, pads, parts) or the full board (with copper). Its feature numbers may change freely; the publish preview warns when an enclosure references anything in it.

**Tiers** (`Tier`):

| Tier | Contains |
| --- | --- |
| `interface` | the slots only |
| `mechanical` (default) | the slots, laminate, pads and parts |
| `full` | the slots and every copper body |

**The sidecar,** `<name>.interface.json`, written beside the design, with the same content in the design's document metadata under a reverse-DNS key, in chunks under Solidcraft's 64 KiB value limit. Its schema is recorded in [board-interface.schema.json](board-interface.schema.json) (`sidecar`), with the stored table, the commands and the reports; a test fails when the recorded schemas and the types differ (set `PCBCRAFT_BLESS=1` to rewrite the file after a deliberate change).

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
- `revision` counts publishes whose content hash differs, from 1. Each datum has its own hash, so a diff is per datum.
- `source_id` is drawn by the host (the core never generates randomness) on the first publish and kept. Renumbering slots, a deliberate compaction, starts a new `source_id` epoch, with an impact preview saying that every enclosure reference must be picked again.

The sidecar is shaped to become the `board.json` part of the exchange file proposed upstream (RFC 0001): its `manifest.json` fields are `source_id`, `revision`, `hash` and `units`; its persistent face names are the slot faces; its reference outcomes are this design's statuses.

## Findings

`interface::findings` lists every code once (`CODES`) with whether it blocks fabrication output and whether a waiver may lift that. Messages are in the user's words, built by one function per code.

| Code | When | Blocks fabrication | Waivable |
| --- | --- | --- | --- |
| `interface.unpublished` | "3 datums changed since revision 14" | no | — |
| `interface.datum_lost` | an exported datum's owner or feature is gone; it keeps its slot | no | — |
| `interface.datum_inexact` | an exported datum was found again by geometry | no | — |
| `housing.stale` | the enclosure changed since the board read it | no | — |
| `housing.older_board` | the enclosure links an older published board | no | — |
| `housing.both_changed` | both changed since they last agreed | no | — |
| `housing.reference_lost` | an enclosure feature uses a datum that no longer resolves | yes | no |
| `housing.reference_inexact` | an enclosure feature's datum was found again only by geometry | yes | no |
| `housing.unexported_face` | an enclosure feature uses a face that is not exported | no | — |
| `housing.cycle` | the board and the enclosure depend on each other in a loop | yes | no |
| `housing.missing` | the enclosure file was not found; the board uses its snapshot | no | — |
| `housing.newer` | the enclosure was saved by a newer SolidCraft | no | — |

Lost and inexact references are never waived, as for part dimensions (`board.reference_lost`): they are re-picked, or acknowledged in an impact preview. A local edit to a board edge that the enclosure drives is refused with `housing.driven_edge` ("This edge follows the enclosure's wall. Break the link to edit it here."). Findings name the datum's owner (the instance, or the board), since `ObjectRef` has no datum variant; slice B6 decides whether locating a datum needs one.

## Where the contract lives

| Module | Holds |
| --- | --- |
| `library::datum` | `DatumKind`, `DatumName`, the reserved names and the part-file prefixes, shared by package datums and the interface |
| `model::interface` | the stored data, identity only: `InterfaceTable`, `InterfaceDatum`, `DatumId`, `Slot`, `DatumKey`, `DatumOwner`, `DatumRole`, `DatumSource`, `AutoRule`, `Tier`, `PublishedRecord`, `PublishedDatum`, and the bounds |
| `interface` (core module 11a) | the rules, geometry, commands, reports, sidecar, findings and schemas |
| `ids` | `InterfaceSourceId`, the board's random interface identity |

`interface` sits above `layout` and below `erc` in the [module map](../component-model.md#d-module-map), which `cargo xtask core-modules` enforces: it may use `geometry`, `ids`, `diagnostics`, `library`, `model`, `instances`, `layout` and `units`. `issues`, `fabrication`, `updates` and `commands` may use it.

## Slices and their tests

`crates/editor-core/src/interface/tests.rs` tests the contract's own rules. `interface/tests/skeletons.rs` holds each later slice's acceptance tests as ignored placeholders that fail; the slice removes the `ignore` and writes the body.

| Slice | Content | Placeholder tests |
| --- | --- | --- |
| B1 | the table in `Project`, automatic rules, `interface.*` commands and `interface.list` in the engine, persistence | `b1_automatic_rules_export_the_board`, `b1_switching_a_rule_retires_and_never_reuses`, `b1_export_allocates_one_slot_per_datum`, `b1_refused_exports_change_nothing`, `b1_unexport_retires_the_slot`, `b1_renames_change_the_display_name_only`, `b1_slots_survive_undo_redo_save_and_reopen`, `b1_a_deleted_owner_makes_the_datum_lost`, `b1_selection_never_changes_the_interface` |
| B2 | package datums, the part-file convention, authoring, connector templates that emit openings | `b2_a_part_file_opening_reaches_the_board_frame`, `b2_no_opening_is_inferred_from_a_name` (built); `b2_external_datums_are_exported_per_placement` waits for B1's rule, whose input `interface::package::external_datums` is tested in `package_datums` |
| B3 | the exported design with slots and tiers, the sidecar and metadata, `publish_preview` (diff only) and `publish` | `b3_the_preview_lists_the_one_moved_datum`, `b3_the_enclosure_cutout_follows_by_two_millimetres`, `b3_the_sidecar_is_byte_identical_after_reopen`, `b3_unrelated_edits_change_nothing_in_the_enclosure`, `b3_publish_records_a_revision_only_when_the_hash_changes`, `b3_publishing_the_interface_tier_is_under_100_ms` (a target until measured) |

B4 (the enclosure link and dry run), B5 (the two-way check, a test per conflict row), B6 (UI), B7 (live channel) and B9 (VM run) follow the design note.

## Open questions

- **Recording a publish in the undo history.** `interface.record_publish` is a document change, so undo can take it back while the written file stays. B3 decides whether the preview compares against the recorded hashes, the sidecar on disk, or both, and how an undone record reads.
- **Moved distances in the preview.** The stored record keeps per-datum hashes, not geometry. "Moved by 2 mm" needs the previous geometry: B3 reads it from the published sidecar, and reports only "changed" when the file is gone.
- **A datum in `ObjectRef`.** Findings name the owner for now (above).
