# Fabrication output (as built)

The founder asked for a full development cycle that ends in production files a board house can build from. This note records what was built: the formats, the conventions and the limits.

The Gerber writer follows Ucamco's public Gerber Layer Format Specification. The job file follows Ucamco's public Gerber Job File format, and the drill files follow the public Excellon description. All of the code is original. No other EDA tool's reader, writer or viewer was consulted, and the verification reader is original too.

## One command

- **Automation:** the `project.export_fabrication` request takes `{output, zip, profile, options}`. It runs the production check and then writes a new or empty folder, or one new `.zip`. It returns the pre-flight report, every file with its size, and the time each step took. It is a file effect: it adds no revision and no undo step.
- **CLI:** `pcbcraft-cli fab FILE.pcbcraft OUT_DIR|OUT.zip [--zip] [--profile ID] [--date ISO|none]`. Files are named after the project file. A blocked export prints the title of each unwaived error, up to 100 and then how many remain.
- **MCP:** the same request reaches MCP through the generic catalog.
- **Desktop:** PCB workspace → ribbon Output → Production, or "Production files…" in the command palette.
  - The dialog shows a profile picker, Folder or ZIP output, an output toggle for each group and the placement convention.
  - It lists the pre-flight report, computed on a worker, grouped by kind of finding. Tick findings (Shift-click ticks a run, a group's box ticks the whole group), or use a row's or a group's Waive…. The reason form opens under that row or group, and Waive applies one optional reason to every ticked finding as one undo step. A waived finding offers Restore.
  - Export runs on a worker and shows progress; the result lists the written files.
- **Shared path:** all of these call `pcbcraft_io::fabrication::export`. The pure generation is `pcbcraft_core::fabrication::generate`, the PDF drawings are `pcbcraft_io::fab_drawings`, and `pcbcraft_io::pdf` is a small generic PDF canvas that schematic printing can reuse.
- **Writing:** a folder is written to a sibling staging folder and then renamed into place. A ZIP is written atomically, entries in set order with a fixed timestamp.

## Production check (`fabrication.preflight`)

The check reports every project issue under the chosen fabrication profile. Overrides and net classes are kept; a profile that conflicts with an override is itself an error. It adds production findings to those issues. The query's `profile` is an id of the user's profile library (as for an export) or a whole profile; omitted, the project's own rules apply. A board without a rule profile is blocked (`rules.no_profile`, not waivable).

**Errors** (they block export):
- unrouted connections and unplaced parts;
- stale fills, including fills computed under other rules;
- fills never filled;
- an unsolved outline;
- parts without a footprint;
- plated board slots that are not straight slots;
- every board design-rule error.

**Warnings:**
- parts with only an envelope 3D model;
- parts whose definitions have unpublished draft changes (the review gate for library changes);
- legend over a pad's mask opening;
- legend text past the board outline or over a cutout or slot (`fabrication.legend_outside`), one finding per text, naming it ("Designator J2 runs outside the board");
- legend text closer to those edges than the rules' silkscreen edge clearance, when one is set (`fabrication.legend_edge`). Mounting holes are not edges for either check. Legend lines are not checked: a part may overhang the board and draw its outline past the edge;
- purchasing to-dos such as a missing Part;
- component-area findings.

**Waivers:**
- An unwaived error blocks export, and nothing is written.
- `fabrication.waive {keys, reason?}` waives one or more findings with one optional reason, and `fabrication.unwaive {key}` restores one. Each is a document operation and one undo step. A list with one empty or unprintable key is refused whole. The desktop setting "Require a reason to waive errors" (Settings → Production, off by default) makes the app's engine refuse a waiver without a reason, from the dialog and the in-app assistant alike; headless CLI and MCP sessions keep the reason optional.
- Waivers are saved in the project (`Project.production.waivers`, keyed by finding code plus the objects it names). A finding that changes identity needs a new waiver.

## Files

`STEM` is the base name (the project file's stem in the CLI).

| File | Content |
| --- | --- |
| `STEM-copper-lN-top/innerK/bottom.gbr` | `Copper,Ln,Top/Inr/Bot`, positive |
| `STEM-mask-top/bottom.gbr` | `Soldermask,Top/Bot`, **negative** (the image is the openings) |
| `STEM-paste-top/bottom.gbr` | `Paste`, positive; only when the side has paste |
| `STEM-legend-top/bottom.gbr` | `Legend`, positive; only when printed and not empty |
| `STEM-profile.gbr` | `Profile,NP`: outline, cutouts and non-plated slots with exact arcs |
| `STEM-plated.drl`, `STEM-non-plated.drl` | Excellon, plated and non-plated separately |
| `STEM-drill-map.gbr`, `STEM-drill-table.txt` | `Drillmap`: one symbol per tool; the text table |
| `STEM.gbrjob` | Job file: stackup, thickness, finish, outer design rules, files in layer order |
| `STEM-pos-top/bottom.csv` | Pick-and-place per populated side |
| `STEM-bom.csv`, `STEM-bom-parts.csv` | BOM grouped as `project.bom` groups it; one row per part |
| `STEM-fabrication.pdf`, `STEM-assembly.pdf` | Drawings, A3 landscape |

### Gerber X2 conventions

- **Format:** `%FSLAX46Y46*%` and `%MOMM*%` (1 nm), absolute coordinates, `G75` multi-quadrant arcs, `M02`. Unchanged coordinates are left out; `D01`/`D02`/`D03` are always written.
- **Apertures:** each is defined once, numbered from D10 in first-use order, with its `.AperFunction` set by `%TA` before its `%AD`.
- **File attributes:** `.GenerationSoftware`, `.CreationDate` (only when asked), `.ProjectId` (the project name set by `project.rename`, a stable GUID derived from the name, revision 1), `.SameCoordinates`, `.FileFunction` and `.FilePolarity`.
- **Copper order:**
  1. Fill regions, largest first: each region dark, its holes clear (`%LPC`). An island inside another region's hole is smaller, so it is drawn after that hole is cleared; nothing drawn after the fills is erased.
  2. Tracks, as round-aperture `D01` draws, so their ends are exactly round.
  3. Vias (`ViaPad`).
  4. Plated mounting-hole rings (`WasherPad`).
  5. Pads (`SMDPad,CuDef`, `ComponentPad`, `FiducialPad,Local`).
- **Pads:**
  - Circles, axis-aligned rectangles and obrounds are standard apertures.
  - Rotated rectangles and obrounds, and rounded rectangles (any corner mask, bottom-side reflection applied), are literal-value macros of exact primitive-21 rectangles and primitive-1 circles.
  - Custom outlines are regions with exact `G02`/`G03` arcs; a grown custom outline follows the core's mitred offset.
- **Object attributes:** `.N` (net; empty for no net), `.P` (designator, pad number) and `.C` (designator).
- **Mask:**
  - Pads open by their mask rule; `Inherit` uses the effective design rules' mask expansion, not the footprint default.
  - Untented vias and mounting holes open by the same expansion.
  - Through-hole and NPTH pads open on both sides.
- **Paste:** `Inherit` shrinks by the rules' paste reduction, for surface pads without a hole; explicit reductions and windows are kept as authored.
- **Legend:**
  - Footprint silkscreen strokes are drawn with a round pen, with exact arcs; filled shapes are regions.
  - Text uses Pcbcraft's stroke font (`layout::legend`, now shared with the 3D silkscreen films).
  - The legend is not clipped at mask openings; the check warns instead.
  - Text an instance moved (`instance.set_text_placement`) prints where it was moved. The offset is in the footprint's own axes, so it turns and flips with the part; the turn is added in quarter turns. An offset is at most 1000 mm. The placement stays with the instance through new revisions of the same footprint, and goes when its footprint or text does.

### Excellon conventions

- **Header:** `M48`, X2 attributes as `; #@! TF.…` comments, `FMAT,2`, `METRIC`, and one `TnnCd.ddd` tool table per file, each tool preceded by its `TA.AperFunction` comment.
- **Body:** `%`, `G90`, `G05`, then decimal-point coordinates at 3 decimals, so no zero-suppression convention applies. The file ends with `T0` and `M30`.
- **Slots:** pad slots and plated straight board slots are canned `G85` slots with the tool's width.
- **Tools:** one per (plating, diameter to 1 µm, hit or slot, function), numbered from 1 across both files, plated first (T1–T3 plated, T4 non-plated), so a number names one tool in the drill table, the drill map, the fabrication drawing and either file. Excellon numbers tools per file, but a fabricator who loads both files into one CAM job, or reads the combined drill table, then never sees two tools called T1.

### Pick-and-place

- **Defaults:**
  - Each row gives the placement origin (the footprint datum) in mm from the board origin, the same origin the Gerber files use.
  - Rotation is degrees counter-clockwise seen from the top, with 0° the footprint as drawn in the library.
  - Bottom parts are written at their board angle.
- **Fab-specific conventions:**
  - Origin: the outline's lower-left corner or any point.
  - Bottom angle seen from below: `(180 − rotation) mod 360`, optionally with bottom x negated.
  - Rotation offsets by package-name pattern.
- **CSV format:** RFC 4180 quoting, LF line ends, UTF-8 without BOM.
- **Value column** (pick-and-place and both BOMs): the instance's value text, every typed field with a value as the instance shows it ("10 µF 16 V", "10 kΩ ±1%"), so every rating the part was specified with is kept. Without one: the chosen Part's MPN, else the component name for a part of more than two terminals (an IC's name is its identity), else empty. A passive's component name ("Chip capacitor 0603 (1608 metric)") is a package description the Package column already carries, so it is never written as a value; its Status reads "no part".

### Drawings

- **Fabrication drawing:**
  - the outline at a standard scale with overall dimensions;
  - every hole drawn with its tool's drill-map symbol;
  - the drill table and the stackup table;
  - notes from the effective rules: thickness, copper, minimum track and gap, drill, rings, mask, legend;
  - a title block.
- **Assembly drawing:** one page per populated side, with bodies, grey pads, designators and red pin-1 dots. The bottom page is viewed from below and is labelled as mirrored.

## Verification (measured 2 November 2026, macOS arm64, `ci` profile)

`fabrication::tests` holds an original Gerber/Excellon reader and a bit rasteriser that samples pixel centres. They are tested against hand-derived areas: circle and stadium flashes and draws, rotated macros, cut-in and clear regions, arcs, and Excellon slots.

`expected` renders the core's own geometry independently, from placed pads, track capsules, via discs, fill regions, mask and paste rules, the solved outline and the holes. `compare` reads every written layer back and takes the XOR against that rendering at 10 µm. Both boards below are compared.

| Board | Layers | Largest XOR |
| --- | --- | --- |
| Showcase (`Engine::showcase`): 4 layers, bottom and inner fills | 4 copper, 2 masks, 2 pastes, profile, plated and non-plated drill | 0 mm² on every layer |
| Sketched fixture: 2 mm filleted corners, a round cutout, a non-plated and a plated slot, a plated ring hole and a bare hole, refilled fills | same | 0.0001 mm² on copper; 0.0009 mm² on the profile and plated drill (1 µm drill rounding) |

The 10,000-via plane (200 × 200 mm, one bottom fill) is generated in 0.12 s (23.8 MB, 13 files). Its bottom copper, top mask and plated drill read back with 0 XOR at 20 µm. The target was seconds.

The shipped showcase's whole CLI export, with the check, drawings and atomic write, takes 0.13 s; most of that is fsync.

The comparison window is offset by a non-round margin. Two holes of one fill can share an edge exactly on a grid-aligned pixel centre. An even-odd fill and clear-polarity regions then sample that edge differently: a measurement artefact of 0.1 mm² on the aligned grid, and 0 when offset.

Format tests check attributes, polarities, D-code reuse, macro text, escapes, determinism (same bytes twice; a date only when asked) and the job file's layer order and thickness. Automation tests export the shipped (sketched) showcase as a folder and as a ZIP. They also cover a refused used folder, and an export blocked under another profile (stale fills) with nothing written, then unblocked by waivers that survive save and reopen and undo one step each.

## Limits and follow-ups

- **Out of scope:** IPC-2581 and ODB++ are follow-ups.
- **Holes:** blind and buried vias are not modelled, so there are no drill-pair files. A plated board slot that is not straight is refused by the check.
- **Legend:** it is not clipped at mask openings or the outline (the fabricator clips; the check warns). The desktop's Designator section moves the silkscreen designator only; the command takes any footprint text.
- **Assembly variants:** there are no DNP flags or variants in the model; every part is fitted.
- **Profile check:** the check compares profiles only by id. A changed user profile file is read fresh at each export.
- **Drawings:** text widths use Helvetica's metrics; bold text is measured with the regular widths.
- **Desktop dialog:** the pre-flight time on large boards was not measured; the check runs on a worker. Cancelling during an export closes the dialog while the worker finishes writing.
