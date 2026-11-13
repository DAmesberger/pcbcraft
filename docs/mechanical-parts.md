# Mechanical parts and the part-file chooser: contract

Contract, 3 November 2026. Base: `component-model` at 5e0922b, after [lean parts](part-templates.md#11-lean-parts-contract) (§11). It covers two founder-approved goals:

1. **A part-file chooser in the desktop.** The Component workspace's 3D Body section chooses a Solidcraft part file, checks it, edits its parameters and previews it, and can switch back to the generated model.
2. **Parametric 3D parts placed directly on the PCB.** Solderable SMD nuts and standoffs, press-fits and shield clips have pads. Heatsinks, spacers, brackets and light pipes are purely mechanical. Both are placed from a part file in Layout and appear in the 3D view, the BOM, pick-and-place, the assembly drawing, export and the board checks.

It follows the founder's rules: Solidcraft stays generic, footprints stay Sparksmith's, one command engine serves UI, CLI and MCP, and release 1 varies geometry per package.

## M1. Decisions

| # | Decision | Why |
|---|---|---|
| M1.1 | A directly placed part is an ordinary **component instance**. Its component, symbol, package, footprint and binding (the **mechanical part set**) are generated library definitions, published into the project snapshot. The package's model is the part file (`PackageModel::PartFile`). | It needs no new instance kind. Library identity, exact snapshots, explicit pin mapping, undo, selection, the 3D build, export, BOM, pick-and-place and DRC already handle instances. The component model's rules hold unchanged. |
| M1.2 | Each **variant** (a part file revision, its values, its lands and its assembly flags) is its own set. A variant is identified by `MechanicalPart.key`. Editing a placed part's parameters selects the existing variant with the new key, or creates it, and re-points the instance. | Instances stay package-bound (release 1: geometry per package, not per instance). Equal values share one set, one native definition (`PartKey`, L1) and one BOM line. |
| M1.3 | **Schematic: the mechanical symbol convention.** Every set has a one-unit symbol: a rounded body with the designator and the part name, and one pin per terminal (none for a purely mechanical part). An instance placed on the board starts with that unit **unused** (off-sheet). | The model requires a representation, and "unused" already means "deliberately left off the sheet". The schematic and ERC need nothing. A part whose pads land terminals can still be placed on a sheet and wired, for example to GND. Connectivity keeps a single source, the schematic. |
| M1.4 | **Footprints stay Sparksmith's.** The set's footprint is a Sparksmith footprint definition built from **lands**, which are Sparksmith data (M3). They come from one of three sources: a built-in part's land rules, the pads of a footprint the user picks, or explicit lands in the request. The part file supplies only the 3D body and never the land pattern. | This is the founder's 3 November revision. The lightest approach adds no new convention to part files and no land UI beyond a choice. |
| M1.5 | Pads are `PadKind::Mechanical` copper. With `pads: mechanical` (the default) they land no terminal. With `pads: terminals`, each pad lands its own terminal (`1`, `2`, …) and can join a net. | A mechanical pad may land a terminal (glossary, *Pad kind*), so one kind covers both. Unconnected pads need no decision and raise no ERC finding. |
| M1.6 | **Assembly flags** on the set: `bom` (default true) and `pick_and_place` (default true when the part has pads, else false). | The founder: parts with pads go into BOM and pick-and-place; purely mechanical parts go into the BOM (configurable). The flags are part of the variant, so changing one in Properties selects or creates that variant (M1.2). |
| M1.7 | The set gets a **courtyard**, assembly outline and silkscreen outline from the body's xy bounds and its lands. They are the standard footprint drawings, made explicitly when the set is generated (glossary, *Derived courtyard*). | Courtyard overlap, clearance and the assembly drawing then treat mechanical parts like any part. |
| M1.8 | The part file is a Model of format `solidcraft`. It is shared by asset hash: one Model per file revision in a project, whatever the number of variants. | It is deduplicated as `library.use_part_file` already does. |

**Trade-offs of M1.2 (variant per value set).**

- **Library size.** Each distinct value set adds one small set of five definitions. The Model (the file bytes) is shared. For the realistic case, a few nut heights and one heatsink, this is a handful of sets. The number of sets is bounded by the number of instances (4096).
- **BOM.** An M3 × 4 mm and an M3 × 6 mm nut are different purchases and get different BOM lines. This is wanted.
- **Updates.** A newer revision of the part file is not applied to placed parts automatically. Re-choosing the file in Properties makes a new variant, and the instance moves to it. A library update across all instances of a set (impact preview) uses the existing update plans once the set is promoted to a library. Release 1 adds no part-file-specific update plan.
- **Editing the set.** The generated set can be drafted and edited in the Component workspace like any component, for example to adjust its footprint. The edited package keeps its `mechanical` record and key, so later placements with the same values reuse the edited footprint. The key therefore names the variant's origin, not a guarantee that its footprint is unchanged.
- **Rejected alternatives.**
  - *Per-instance geometry* is deferred by the founder (O4).
  - *One component with one binding per variant* would make a new component revision for every new value set, and every existing instance would then face an update.

## M2. Data model (core)

```rust
// library/package.rs (the package variant; as built, see M10)
pub struct PackageDefinition {
    …
    /// Set when the core generated this package's set from a part file
    /// (`layout.place_part`). Display, dedupe and assembly flags only:
    /// electrical rules never read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mechanical: Option<Box<MechanicalPart>>,
}

// library/mechanical.rs (new)
pub struct MechanicalPart {
    /// The variant identity: SHA-256 over the canonical JSON of
    /// (asset hash, values, lands, pads, bom, pick_and_place). The core
    /// computes it; a request never supplies it.
    pub key: ContentHash,
    /// What the part was chosen as: a built-in name (`smd-nut`) or the
    /// file name (`bracket.solidcraft`). Display only.
    pub source: String,
    pub pads: PadUse,            // mechanical | terminals
    pub bom: bool,
    pub pick_and_place: bool,
}
pub enum PadUse { Mechanical, Terminals }

/// One land of a mechanical part, in the footprint frame (mm).
pub struct MechanicalLand {
    pub name: String,            // the pad number: 1–16 bytes [A-Za-z0-9_-]
    pub at: Point,
    pub shape: LandShape,
}
pub enum LandShape {
    /// Round copper, optionally around a non-plated hole (an annular land:
    /// a solderable nut or standoff).
    Circle { diameter: f64, hole: Option<f64> },
    Rect { size: Point, rotation_deg: f64 },
    /// A bare non-plated hole (a spacer's or heatsink's screw).
    Hole { diameter: f64 },
}

/// What `layout.place_part` and `layout.set_part` carry: everything the
/// core needs to find or make the variant. The host evaluated the part
/// file (the core never runs Solidcraft).
pub struct PartSpec {
    pub name: String,                 // the part's name: "SMD nut"
    pub designator_prefix: String,    // H, HS, MP, … (1–8 uppercase letters)
    pub source: String,               // M2 MechanicalPart::source
    pub model: PartModelSource,
    pub values: PartValues,           // as PartFileModel::values (L1)
    pub derived: PartDerived,         // as PartFileModel::derived (L1)
    pub lands: Vec<MechanicalLand>,   // at most 64
    #[serde(default)] pub pads: PadUse,
    #[serde(default = "yes")] pub bom: bool,
    /// Default: true when `lands` has copper.
    #[serde(default)] pub pick_and_place: Option<bool>,
}
pub enum PartModelSource {
    /// A Model revision of format solidcraft the project holds.
    Model { model: ModelRef },
    /// The file itself: packed and published as a new Model, unless the
    /// project already has a Model with this asset hash (then it is reused).
    File { name: String, definition: Box<ModelDefinition>, asset: PackedAsset },
}
```

**The generated set** for a spec whose variant does not exist yet. Lineages are allocated from the project and published as revision 1, in one transaction, through `library::publish` over a scratch draft set. The user's drafts are never touched.

| Definition | Content |
|---|---|
| Model | From `File`, unless an equal asset hash exists (M1.8). |
| Package | `PackageForm::Generic`, `envelope` = the body's bounds size, model `PartFile { model, values, derived }`, `mechanical`, default placement (part origin = footprint origin, seating plane z = 0). Terminals: one per copper land when `pads: terminals`, else none. |
| Footprint | One pad per land (M3), standard graphics over the body's xy bounds (`add_standard_graphics` over the bounds box, not the centred envelope): assembly outline, silkscreen outline, courtyard (`COURTYARD_EXCESS_MM`), designator. Datum at the origin. |
| Binding | Package terminal *n* to component terminal *n*, or no mappings. |
| Symbol | One unit `A`: a 10.16 × 5.08 mm body, designator above and component name below, one pin per terminal on the left (2.54 mm pitch). |
| Component | `designator_prefix`, passive terminals `1…N` (or none), the symbol and binding, and the name `"<name> <values>"` (for example `SMD nut height 4 mm`), limited to `MAX_LABEL`. |

**Dedupe.** The variant for a spec is the component lineage whose **latest** revision's package has `mechanical.key == key(spec)`. A placement reuses it. Two equal specs therefore never make two sets.

**Instance.** A `ComponentInstance` with `representation` = the symbol, `physical` = (binding, footprint), `unused_units = {1}`, no occurrence, and a `Placement`. Designators are numbered under the prefix as `component.place` does.

**Bounds.** At most 64 lands. Sizes: 0.05–100 mm, holes smaller than their land. Names are unique within a part. The usual library and instance limits apply.

## M3. Lands

- A **copper land** is a `Pad { kind: Mechanical, terminal: Some(n) | None, number: name, side: Top }`.
  - A circle is `PadShape::Circle`. With `hole`, it gets a round **non-plated** `Hole` at its centre (an annular SMD-nut land, copper on the component side only).
  - Paste: a plain land inherits the footprint's paste. An annular land gets four paste windows of 70° each on the ring, so the stencil never covers the hole.
  - A rectangle is `PadShape::Rect`.
- A **hole** is a `Pad { kind: NonPlatedHole, terminal: None }` whose `shape` is a circle of the hole's diameter.
- **Sources of lands**, chosen by the host:
  1. **Built-in land rules.** A built-in part ships with Sparksmith data rules over its own parameter values. Each rule is `diameter = scale × value(param) + add_mm`. They are evaluated at the chosen values.
     - SMD nut: an annulus of `outer_d + 0.6` around a hole of `thread_d + 0.2`.
     - Spacer: a hole of `inner_d`.
     - Heatsink: none.
  2. **From a footprint** (`lands_from: FootprintRef`). Each pad of a project footprint becomes a land: same position, shape (circle or rect; other shapes are refused) and hole. Its terminal is dropped. The user picks it in the chooser.
  3. **Explicit** `lands` in the request (MCP, agents, CLI).
  4. None: a purely mechanical part.

## M4. Commands

### Core (`sparksmith-core`, one undo step each, atomic)

| ID | Payload | Result |
|---|---|---|
| `layout.place_part` | `{ part: PartSpec, position, rotation_deg, layer, designator? }` | `Outcome::PartPlaced { instance, designator, component, created }`. Finds or makes the variant (M2), then places a new instance with its unit unused. Refused (nothing changes) on an invalid spec, a placement off the board, or any instance or library rule. |
| `layout.set_part` | `{ instance, part: PartSpec }` | `Outcome::PartPlaced`. Re-points a mechanical instance to the spec's variant (found or made), keeping its id, designator, placement, marking and lock. Refused for an instance that is not a mechanical part, a locked placement, or a terminal change while the unit is on a sheet or a terminal is connected. |
| `layout.remove_part` | `{ instance }` | Removes a mechanical instance that has no occurrence, with its placement. Refused if a terminal is connected (preview `Change::Instance { Remove }` instead). |

### Automation (`sparksmith-automation`: JSON lines, MCP)

| ID | Payload | Result |
|---|---|---|
| `library.part_files` (query) | `{}` | `{ builtin: [{ name, title, prefix, kind: "pads" \| "mechanical", parameters }], project: [{ model, name, file, licence }] }` |
| `library.check_part_file` (exists) | `{ path \| builtin \| model, values? }` | The L2 report; built-ins also report their evaluated `lands`. |
| `layout.place_part_file` | `{ path \| builtin \| model, values?, position, rotation_deg?, layer?, pads?, bom?, pick_and_place?, lands?, lands_from?, name?, prefix?, designator?, licence?, redistribution?, expected_revision }` | `{ revision, outcome, part: report }`. Reads and checks the file (blocking findings refuse), evaluates the land source (M3), builds the `PartSpec` and applies `layout.place_part`. |
| `layout.set_part_file_values` | `{ instance, values, pads?, bom?, pick_and_place?, expected_revision }` | The same reply. It evaluates at the new values with the instance's file and land source and applies `layout.set_part`. Omitted flags keep the instance's. |
| MCP tools | `library_part_files`, `layout_place_part_file`, `layout_set_part_file_values` | The requests above. |
| CLI | `sparksmith part list` | Built-ins with their parameters. `part check` also prints built-in lands. |

**One builder.** `sparksmith_automation::part_files` exposes the functions that build the core `Operation` for each request: `use_part_file_operation`, `set_part_values_operation`, `place_part_operation` and `set_part_operation`. They take the engine read-only and return the operation and the check report. The desktop calls them and applies the operation through its own engine, so UI, CLI and MCP issue the same typed command.

**The land source of a placed part.** For `set_part_file_values` it is read back from the variant:

- a built-in `source` re-evaluates its land rules;
- any other variant keeps its lands unchanged (copied from the current footprint).

## M5. Desktop

- **Part-file chooser** (`apps/sparksmith/src/part_chooser.rs`, one widget for both entry points). It is a Solidcraft-styled sheet with a source list on the left:
  - **Built-in**: the starter set and the WROOM-3;
  - **This project**: Models of format `solidcraft`;
  - **Recent files**: at most 8, in the app settings;
  - **Open file…**: a native dialog.

  The detail pane on the right shows:
  - the check report: blocking findings red, advisory amber;
  - the parameters as a form with units (mm, deg, count), committing on Enter or when a field is left;
  - bodies and materials;
  - the lands and pads choice.

  Evaluation runs on a worker thread, and the UI shows the last result while it waits.
- **Component workspace, 3D tab, Body section.**
  - **Part file…** opens the chooser. **Use** applies `use_part_file_operation`.
  - Once a package uses a part file, its card shows the file, its licence and the parameters form. Each committed edit is one `library.set_part_values` (one undo step).
  - **Use generated model** sets `PackageModel::Generated` for a typed form, or the envelope for a generic one.
  - The Component 3D view previews the part file through `project::preview_part`.
- **PCB Layout.**
  - The registry command `pcb.place-part`, **Place 3D part…**, has the key **M** (mnemonic *mechanical*). M is free in PCB › Layout; the route tool's own M applies only while routing. It appears in the ribbon's Layout tools, the command palette and the shortcuts sheet.
  - It opens the chooser in Place mode. **Place** puts the part at the view centre, snapped to the grid, on the side being viewed, and selects it. From then on it moves, rotates and changes side like any footprint (drag, placement card, the same commands).
- **Properties of a placed mechanical part.** The placement card gains a **3D part** section:
  - the source;
  - the parameters form, where each committed edit is one `layout.set_part`;
  - **Pads**: mechanical or terminals;
  - **In BOM** and **In pick-and-place**.

  **Remove from board** in that section removes a board-only mechanical part with `layout.remove_part`. (Layout has no Delete key for placements yet.)

## M6. Outputs and checks

| Output | Behaviour |
|---|---|
| 3D view, native `.solidcraft` export, STEP | Unchanged path. The board build defines each `PartKey` once (L3) and places one occurrence per instance; the bodies keep their materials. |
| BOM (`project.bom`, `bom.csv`, `bom-parts.csv`) | Includes instances whose package has no `mechanical` record or has `bom: true`. A mechanical line without a Part is `no part`, as for any part, until a Part is assigned. |
| Pick-and-place | Rows only for parts with `pick_and_place: true` (or no mechanical record). |
| Purchasing completeness | Instances excluded from the BOM raise no purchasing finding. |
| Assembly drawing (PDF) | Draws the assembly outline and designator like any footprint. |
| Courtyard overlap and clearance | Uses the generated courtyard. A heatsink stacked on a chip overlaps the chip's courtyard by design. Release 1 reports it, and the user waives it in Production pre-flight; a "stacked on" relation is later work. |
| Copper DRC, drills, mask, paste | Pads and holes like any footprint's: the annular land's copper, its non-plated hole in the NPTH drill file, the hole's clearance to copper. |

## M7. Built-in starter set

The parts are original parametric designs, authored through Solidcraft's commands by `tests/author_parts.rs`, under MIT OR Apache-2.0. Their dimensions are generic round numbers chosen for illustration and copy no vendor's drawing (verify against the chosen part's datasheet). They follow L2: a single component, the origin at the footprint origin, z = 0 as the seating plane, appearances named after Sparksmith materials and a `marking-top` plane.

| Built-in | Title | Parameters (default) | Material | Prefix | Lands |
|---|---|---|---|---|---|
| `smd-nut` | SMD nut | `thread_d` (3 mm), `outer_d` (5 mm), `height` (3 mm) | `tin-matte` | H | annulus `outer_d + 0.6` around a hole of `thread_d + 0.2` |
| `round-spacer` | Round spacer | `inner_d` (3.2 mm), `outer_d` (6 mm), `height` (5 mm) | `pa9t` | H | hole `inner_d` |
| `finned-heatsink` | Finned heatsink | `length` (20 mm), `width` (20 mm), `height` (10 mm), `base_height` (2 mm), `fin_count` (6), `fin_thickness` (1.2 mm) | `aluminium` | HS | none |

## M8. Work and estimates (agent-hours)

| Track | Content | Estimate |
|---|---|---|
| Core | `mechanical.rs`, the three commands, dedupe, the generated set, BOM, pick-and-place and purchasing filters, tests | 4 |
| Automation | builders, the three requests, land rules, MCP tools, CLI `part list` | 2.5 |
| Built-ins | three part files and their land rules | 2 |
| Desktop | chooser, Body section, Layout command, Properties section | 6 |
| Evidence | screenshots, editor evidence, CI | 1 |

## M9. Open for the founder

1. **Stacked parts.** Release 1 reports a heatsink on a chip as a courtyard overlap, which the user waives. Should a mechanical part be able to declare "stacks on" another instance?
2. **Lands from part files.** The rule chosen here is that footprints stay Sparksmith's. If the founder later prefers a part-file land hint (for example named construction points `pad-<n>`), it would be an additional land source (M3) with no format change.
3. **Assembly flags beyond mechanical parts.** `bom` and `pick_and_place` live on mechanical sets only. A general do-not-populate flag, or assembly variants, would replace them.

## M10. As built (3 November 2026)

- **Code.**
  - Core: `library/mechanical.rs`: `PartSpec`, `MechanicalPart`, `MechanicalLand`, `variant_key`, `find_variant`, `make_variant` (the set is published through `library::publish` over a scratch draft set), `in_bom`, `in_pick_and_place`.
  - The commands are in `layout/part.rs` (`layout.place_part`, `layout.set_part`, `layout.remove_part`), with `Outcome::PartPlaced`.
  - `instances::place_off_sheet` creates the instance with its unit unused.
  - Automation: `session/mechanical_parts.rs`: `BUILTIN_PARTS` and their land rules, `place_part_operation`, `set_part_operation`, `placed_part`, and the three requests. MCP exposes them as `library_part_files`, `layout_place_part_file` and `layout_set_part_file_values` (generated from the catalog).
  - CLI: `sparksmith part list`.
  - Desktop:
    - `part_chooser.rs`: one sheet for Use and Place, with its worker-thread `Evaluator`;
    - `part_editor/model_part.rs`: the Body section's Part file…, the parameters form and Use generated model;
    - `pcb_editor/part_place.rs`: Place 3D part… (`pcb.place-part`, key M, with its route-mode shadow) and the placement card's 3D part section: parameters, pad use, BOM and pick-and-place, and Remove from board.
- **Deviations from M2.**
  - **Where the record lives.** The variant record is `PackageDefinition.mechanical`, not a component field. A component field pushed `Definition` past Clippy's large-variant limit, and the package is the "package variant" of the founder's wording anyway. Dedupe goes through the component's first binding to its package.
  - **Publish rules.** Three rules now allow a purely mechanical set:
    - a package whose model is a part file may have no terminals;
    - a footprint of a terminal-less package may have no pads;
    - a component may have no terminals when every implementation lands a mechanical-part package.
- **Built-ins.** `smd-nut`, `round-spacer` and `finned-heatsink` are in `library/parts/`, authored by `tests/author_parts.rs`. Each is one body. The heatsink's fins are a rectangular pattern whose count is the `fin_count` parameter.

  Cold evaluation, single-threaded, Apple M5 Max under heavy load, measured once:

  | Part | Cold evaluation |
  |---|---|
  | `smd-nut` | 10.8 ms |
  | `round-spacer` | 8.5 ms |
  | `finned-heatsink` | 64.9 ms |
- **Evidence.**
  - Core tests (`tests/mechanical.rs`) cover:
    - the off-sheet instance, the generated set, the courtyard, paste windows and the non-plated hole;
    - one undo step;
    - dedupe, and one Model per file;
    - terminal pads;
    - BOM and pick-and-place flags;
    - moving an instance to a variant and back;
    - refusals that keep the document;
    - removal.
  - Automation tests cover:
    - the list;
    - placing a built-in and reusing its variant;
    - new values moving the instance;
    - explicit lands;
    - refusals;
    - the starter set on the showcase board, which builds in 3D and reaches BOM, pick-and-place and the drill table.
  - The pictures come from `write_mechanical_board_pictures` (`viewport/pictures.rs`).
- **Known limits.**
  - Round bodies' bounds come from Solidcraft's outline bounds. A cylinder's box is up to about 10 % larger than its diameter, so a round part's courtyard and assembly outline are that much larger too.
  - A heatsink stacked on a chip is a courtyard overlap (M9.1).
  - Layout has no Delete key for placements, so Remove from board is in the section.
  - The part is placed at the view centre and then moved. It does not follow the pointer.
