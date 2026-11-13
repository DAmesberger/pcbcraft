# Board plan and board interface: 3D and assembly architecture

Design document. The analysis was written on 12 November 2026 against Sparksmith `component-model` at 83c21083 and the Solidcraft fork pin 64bebfef0; nothing was posted upstream. Measurements: Apple M5 Max, `ci` profile, headless, load average 2–3. The prototype was a throwaway headless crate outside the repository (appendix B); its logs are quoted here and it is not kept.

Every statement below is marked by where it comes from:

- **Measured**: run today in the prototype.
- **Read**: read in code or documents, with the path.
- **Given**: from the lost earlier study, as summarised in the brief; not re-checked unless it says so.
- **Estimate** and **Recommendation**: mine.

**Decided 2026-11-12.** The founder approved every recommendation of this note: option 3 for the board's flat bodies (parts stay one Solidcraft design per package, shared by placements), the rule texts of 3.4, the slot interface in place of the full board and the persistent board design, datum solids in the linked design for now, connector openings as package data from the datasheet's panel cutout, interface plus mechanical bodies as the default export tier (copper on request), live update over the control channel as a later opt-in per link, no upstream posting, the part cache later, and `MAX_TRACKS` kept at 2,000. The staged path in 3.2 is the plan of work.

---

## Executive summary

### The question

Should the live 3D board stop being a Solidcraft model (option 3), what should Solidcraft's engine still do, and how does a board take part in a linked design assembly that follows it?

### Conclusions

1. **Option 3 is right for the live board.** The flat board bodies meshed straight from the document take 36–52 ms on the ESP32 board (measured), against 12–14 s and about 1 GB for the exact build (measured). The meshed volumes equal area × thickness to six decimals (measured). The app already consumes the 3D model only as meshes, and both exports already build their own model (read).

2. **A parity test alone does not keep two geometry paths together. A shared board plan does.** Both the mesher and the exact export must read one pure-data list of bodies (the "board plan": which prism, from which loops, between which heights, for which object). Then they can only differ in how they extrude the same loops, and that is what the parity test measures. Without the plan, option 3 is two builders that happen to agree today.

3. **The lead's linked design does not work on today's Solidcraft as described, and the prototype shows why.** Three measured facts:
   - Only solid bodies cross a Solidcraft link. Construction planes, axes, sketches and parameters do not (read). "Datums" as the lead describes them have no carrier.
   - A linked face's name is a digest of its name in the board file, which contains a feature number. Deleting one unrelated part (J1) broke a reference to another part (R5) that had not changed (measured).
   - Repeated parts are told apart only by their order. Deleting R4 moved the linked design's reference from R5 to R2 **without any warning** (measured). This is the most serious finding: a silent wrong cutout.

4. **A small interface design with stable slots fixes this without any Solidcraft change.** The prototype exports each datum as a named solid in a fixed slot; a deleted datum's slot stays used. Result (measured): J1 moved 2 mm, the linked design cutout moved 2.0000 mm; R4 deleted, R5's reference stayed on R5; J1 deleted, the cutout was reported broken. The file is 11 KB, and export plus Update Links take about 50 ms, against 25–37 s for the full board.

5. **Keep the kernel for parts, but not at board-open time.** The lead is right that parts need the kernel (parametric parts, exact faces, STEP). The lead's either/or misses the middle: evaluate a part once per revision and keep its mesh and face table. Today about 4 s of every board build is part evaluation (read, `docs/3d-speed-profile.md`, 8 November build: 5.5 s with 20 parts, 1.2 s with none; today's mechanical subset builds in 5.3 s, measured).

6. **Sparksmith still needs the engine, as a library off the interactive path.** It is needed for part models, STEP, the exact export, and reading and dry-running the linked design. "Only the solver plus an export" is not possible, because the export is the engine: a mesh body can neither be linked nor written to STEP (read). An own renderer is not justified: frames take 1 ms (given) and the viewport crates do not depend on the kernel (read).

### Where I disagree with the lead

| The lead said | Finding |
| --- | --- |
| The board exports as a component and the assembly references its datums | Only solids cross a link today; names depend on feature numbers and on the order of repeated parts. Needs the slot interface, or upstream changes. |
| Upstream needs a link source revision, an `update_links` dry run and an exactness token | The pin already stores the file's SHA-256 on the link and reports `changed`; faces already carry `exact`. A dry run can run in Sparksmith's own engine on a copy. The real gaps are different (section 2.6). |
| "Export-only" is right for the board but not for parts | Right conclusion, wrong framing: parts need the kernel per revision, not per board open. |
| 3D only adds faces to reference | It also adds heights (keepouts, collisions) and a third model, the linked design, which only the engine can evaluate. |
| It maps to RFC 0001 | RFC 0001 is a draft with nothing built. Its PCB payload is to be "defined with PCBCraft", which is upstream's own app, not Sparksmith. The plan must work on today's commands first. |
| (9 November) The published board design is the full board | Update Links on the full ESP32 board took 17.1 s, and its names are the fragile ones. Publish the interface; offer the full board as an option. |
| (Earlier study) 20,000 drills × 5 slabs in 135 ms | Not reproduced: 2.5–6.9 s with the mesher as it is; 0.53 s for one slab with 1 mm tiles. Still far faster than the kernel. The live laminate was one merged body for a while; it is one body per slab again (L1, see the [speed profile](../3d-speed-profile.md)), so the view peels it per dielectric. |

### Recommended architecture

- **Document**: the single source. Parameters, the sketched outline, placements, copper, and the list of exported datums with their slots.
- **Board plan**: pure data derived from the document; the only description of the board's flat bodies.
- **Live 3D**: plan → meshes (option 3). Parts are Solidcraft models, one per package revision, prepared in the background and cached; placed by the document's own transforms.
- **Exact board**: plan → Solidcraft commands, only for export and as the test reference.
- **Interface design**: a small `.solidcraft` with slot-stable datum bodies, optionally followed by the mechanical or full board. This is what an assembly links.
- **Links**: reviewed in both directions, bound to revisions, with Sparksmith's in-process engine doing the dry run. `.craftx` replaces the file format when upstream builds it.

### Order of work (agent hours, estimates)

| Stage | Content | Hours |
| --- | --- | --- |
| 0 | Decisions, rule text and this note (done 2026-11-12) | 2 |
| A1 | Board plan; the exact build reads it | 10–14 |
| B0–B2 | Interface contract, datum model, part-declared datums | 22–26 |
| A2 | Direct mesher behind a setting, parity gate | 14–20 |
| B3 | Interface export with slots: the prototype target | 10–12 |
| A3–A4 | Parts without a board design; the live native board removed | 14–20 |
| B4–B6 | Assembly link, two-way check, UI | 30–36 |
| A5, B7, B9 | Export tiers, live channel, VM run | 22–27 |
| Later | Part cache, scale work, `.craftx` | 28–42 |

About 124–157 hours to a reviewed two-way assembly link on a direct-meshed board. The earlier assembly plan's 14-hour "persistent board design" slice is no longer needed.

---

## Part 1. Critical review

### 1.1 Facts established

**The current pipeline (read).**

- The app's model worker hands the view meshes, pick sources and the stack, nothing else: `BuiltGeometry` in `apps/pcbcraft/src/scene.rs:121-130`.
- No non-test code in `apps/` reads the native session of a board model. The only non-test callers of the board build are the scene worker (`scene.rs:221-232`), the desktop export (`file_ui.rs:346-350`) and the automation export (`crates/automation/src/session.rs:1123`). Both exports build a fresh model.
- Display-only layers (mask, finish, silkscreen, edge bands, via domes) are already meshed straight from 2D data in `crates/solidcraft-integration/src/appearance.rs` and never saved. On the full ESP32 build today 772 of 2,186 drawn bodies are of this kind (measured; given as 776 of 2,187).
- A dimension to a part face is resolved on the part's own design, not on the board model: `crates/solidcraft-integration/src/board/model_refs.rs:1-20`.
- The document refuses more than 2,000 tracks (`crates/editor-core/src/layout.rs:19`) and 16,384 vias (`layout/via.rs:27`).

**Solidcraft at the pin (read; details in appendix A).**

- solidcore is a pure-Rust port of SMLib with `forbid(unsafe_code)`, no C library, and a wasm32 check in upstream CI. There is no OpenCascade anywhere.
- The sketch solver crate depends on the geometry and solidcore record crates, not on the kernel wrapper, document or engine.
- The three viewport crates depend on solidcore's record and tessellation crates and the colour crate, not on the kernel wrapper or document.
- A mesh body cannot be written to STEP (the whole export fails) and is dropped from a linked design with a warning.
- A linked design crosses as STEP text of its solid bodies into one Import feature of one component. Face names are re-attached by geometry and digested (`link:<FNV-1a of the source name>`).
- `DesignLink` stores `path`, `feature`, inherited looks and `file_sha256`. `component.links` reports `changed`. `component.update_links` has no dry run.
- There is no user-assigned name for a face or edge. Construction planes, axes and points are named, but belong to their own component and do not cross a link. A sketch link cannot name another file.
- RFC 0001 exists as a draft only; no `bridge.*` or `.craftx` code exists.

**Measured today.**

| | Exact Solidcraft build | Direct mesh from the document |
| --- | --- | --- |
| ESP32 board, everything | 12.3–14.4 s build, 7.5–8.4 s save, RSS +984 MB, 1,272 features, 1.24 M triangles | laminate, tracks, fills and via barrels in 41–52 ms (five laminate slabs) or 36 ms (one slab); 0.41–0.47 M triangles, 46–53 MB as held |
| ESP32 without fills | 6.9 s, RSS +550 MB | — |
| ESP32, mechanical subset (laminate, parts, pads; no tracks, vias, fills) | 5.3–5.6 s build, 2.8 s save, RSS +331 MB, 73 KB file | — |
| 200 × 150 mm, 1,055 vias, 1,201 tracks, the ESP32's fills | not run (given: +1,000 vias cost +24.5 s and failed once) | 106 ms (one slab, 1 mm tiles); 412 ms (five slabs, 4 mm tiles) |
| 200 × 150 mm, 20,000 vias, 60,000 tracks | not possible | 1.37 s (one slab, 1 mm tiles); 3.4 s (five slabs, 1–2 mm tiles); 7.7 s (five slabs, 4 mm tiles); 16.8–32.5 M triangles, 1.8–3.6 GB as held |
| Volume check | — | laminate 3797.066004 mm³ and fills 284.973609 mm³, both equal to area × thickness to six decimals |

Two corrections to the earlier study's figures:

- **"20,000 drills × 5 slabs in 135 ms" was not reproduced.** `appearance::slab` with its 40-segment circles took 6.9 s at the mask's 4 mm tile and 2.5 s at 1–2 mm tiles. One slab at 1 mm tiles took 0.53 s. The slab mesher's cost per tile grows faster than linearly with the edges in it, so the tile size has to follow hole density. The lost prototype may have used fewer segments.
- **A first run reported fill volume 0.2 % low.** That was my prototype's error (all regions of a fill in one winding region, so copper islands inside another region's hole were cut away). One slab per region is exact. It is worth a test in the real implementation: fills must be meshed per region.

### 1.2 The lead's reasoning, point by point

**"Keep the sketch solver for the outline."** Agreed. It is already behind one trait (`BoardShapeEvaluator`), and the crate is separable from the kernel wrapper (read). No change.

**"Keep the kernel for parts."** Agreed, with a correction to the framing in 1.3 (alternative C). What the kernel does for a part:

| Need | When it runs | Could it be cached? |
| --- | --- | --- |
| Evaluate a parametric part (part file, generated, envelope) | Once per package revision and parameter set | Yes: mesh, face table, anchors |
| Name the face under a pick | At the pick | Needs the evaluated part, or a cached face index |
| Trace a named face to a line or axis in the board plane | When the part or its dimension changes | Yes: the trace needs only the face's plane or cylinder and its extent |
| Import STEP | Once per file | Yes |
| Export STEP or `.solidcraft` | On demand | No |

**"Keep the document and joint layer as the export format for assemblies."** Agreed that the export is a Solidcraft document written by Solidcraft's writer. Not agreed that the full board is the thing to publish (1.4, cost 2, and Part 2).

**"Keep the viewport and renderer."** Agreed. The founder's "own performance-optimised renderer" is not supported by the numbers: frames are 1 ms and a view switch 165 ms (given), and the cost has always been construction. What does need work is on Sparksmith's side of the renderer: mesh memory and level of detail (1.4, cost 6).

**"Don't keep the engine for the live board."** Agreed; this is option 3.

**"The document is the single source; 2D and 3D are views; 3D only adds faces to reference."** Mostly agreed. Two additions:

- 3D adds heights, not only faces: keepout heights, collisions with a linked design, the realistic view.
- The linked design is a third model that only the engine can evaluate. The engine therefore stays in the interactive loop for linked design previews (not per edit).

**"The board publishes a named, versioned interface of datums; the assembly references them."** The direction is right and is what Part 2 designs. As stated it assumes carriers that do not exist today:

1. *Datums.* Planes, axes, points, sketches and parameters do not cross `component.insert_design`. Only solid bodies do.
2. *Named.* A face in the linked design is called `F3:link:<digest>`, where the digest is of the face's name in the board file, such as `F4:start`. The 4 is a feature number. In the saved ESP32 board J1's shell is feature 4 and a resistor package's body is feature 39 (measured, `logs/names.log`); definitions built earlier shift every later number.
3. *Per connector.* The five resistors of one package are bodies `pkg15r1 body`, `pkg15r1 body (pkg15r1:2)`, … All five carry the same face names. The order is their only identity.

Measured consequences on the full-name path (`logs/retarget.log`):

| Board edit | Linked design's reference to R5's top face |
| --- | --- |
| None | on R5, at (24.0, 11.5) |
| R4 deleted (same package, placed before R5) | **on R2, at (33.0, 10.0). No warning, no error.** |
| J1 deleted as well (another package, defined earlier) | error: "the reference `F3:link:5cdd40b8252af9d8` no longer exists as it was picked; reselect it" |

The 9 November assembly design knew that names follow feature numbers and planned a persistent, edited-in-place board design to hold them still (slice S6, 14 h). It did not address the ordinal identity of repeated parts. Under option 3 there is no live board design to keep persistent, so that plan loses its footing either way.

**"Updates go through reviewed link updates, live or file-based."** Agreed. See 2.5.

**"It maps to RFC 0001, needing a link source revision, an `update_links` dry run and a reference exactness token."** Partly wrong on what is missing:

- The link already stores the source file's SHA-256 at our pin, and `component.links` says whether the file changed. (The 9 November note said there was no digest; that was true at the older pin.)
- `model.faces` already reports `exact` per face.
- A dry run does not need upstream: Sparksmith can open a copy of the linked design in its in-process engine, run `component.update_links` there and read the result. The prototype does exactly this.

And on the mapping itself:

- RFC 0001 is "Draft, open for comment … Nothing here is built yet" (read). PCB exchange is phase 5 of 7.
- Its `board.json` is "Defined with PCBCraft". That is `tangency-labs/pcbcraft`, a separate upstream app with its own issue #70 tracking the bridge. Sparksmith is a third implementer of "a contract any Craft app can implement" (RFC §6).
- Founder direction 10 of 9 November was "no upstream proposal for live cross-file links". The RFC now proposes them upstream. That direction should be revisited rather than assumed.

### 1.3 Alternatives

**A. The fast path inside Solidcraft** (a prism or copper-collection feature in the shared engine; the original proposal of `docs/copper-performance-research.md`, and options c1/c3 of `docs/3d-speed-profile.md`).

- For: one geometry path; hard rule 2 stays as written; every Solidcraft user gains.
- Against: the cost is not only the sweep. Each native command re-evaluates parameters and joints, each feature's cache key digests every earlier body, and 1,000 extra tracks cost 1.65 GB (given). A board of 60,000 objects needs a collection feature, which is a new upstream design with format and compatibility consequences. It depends on upstream review.
- Verdict: do not wait for it. Do propose the stitch fix (c1) separately, because the exact export stays slow without it.

**B. A lighter kernel for the board** (truck was about 55× faster on this workload, given).

- Against: a second kernel next to solidcore is exactly what rule 2 forbids, and prisms need no kernel at all.
- Verdict: no.

**C. Parts as cached results, kernel on demand.** The lead's "kernel for parts" becomes "kernel per part revision".

- Store with each package revision (in the project's library snapshot, as disposable derived data keyed by the part's content hash, the engine revision and the tessellation settings): the mesh, the face table (name, exactness, plane or cylinder, extent), edge polylines and declared datums.
- For: a board opens in 3D with no kernel work; a browser build can view and place without loading the kernel; dimension traces to part faces read the table. The cache is the kernel's own output, not a second implementation, so it cannot drift in kind, only go stale, which the key detects.
- Against: file size (bounded by the existing 16 MiB project limit, which already has an open decision); invalidation when the pin moves.
- Verdict: yes, as a later stage (A7). It is the prerequisite for a serious wasm target.

**D. No kernel for parts** (own exact geometry, or meshes only).

- Loses fillets, STEP import and export, history-based face names. glTF parts already show the result: no face references at all (read, `docs/board-sketch-references.md` §6.2).
- Verdict: no.

**E. OpenCascade-free options.** Moot: solidcore is pure Rust (read).

**F. A headless Solidcraft service or wasm worker.**

- The engine compiles to wasm, so a service is not needed to reach the browser.
- A separate process would isolate the kernel's memory (about 1 GB per exact board) and its failures (given: "SMLib failed (status 1001)").
- The seam already exists: the worker takes a project and returns meshes. Under the recommended design it takes a plan and part revisions.
- Verdict: keep in-process worker threads now; keep the seam data-only so a process or Web Worker boundary can be added later. Not now.

**G. The board plan as the source, Solidcraft as an export target only.** This is the recommendation for the board. For parts it is alternative C.

**H. Publish less.** Not listed by the lead, and the main new proposal here: the assembly links an interface, not the board (Part 2).

### 1.4 Hidden costs

**1. Two geometry paths diverging.** A parity test compares outputs on the boards it runs; it proves nothing about the next board. The guarantee has to be structural:

- *One plan.* A pure-data `BoardPlan`: a list of bodies, each `{source object, kind, layer, z0, z1, loops, look}`. Today this knowledge is spread through `project.rs`'s builder, interleaved with native commands. Extracting it is the first step (A1) and the largest single risk reducer.
- *Two thin consumers.* The mesher extrudes each body's loops into triangles. The exporter issues one sketch and one extrusion per body (in tiles, as today). Neither may add, drop, merge or reshape a body. An xtask check can enforce that the live path issues no native board commands and the exporter reads only the plan.
- *The parity test then has a precise job*: for every plan body, the kernel body's volume and bounding box equal the mesh's within a stated tolerance (the chord tolerance × perimeter × thickness), and the sets of bodies match by source, kind and layer. Run on every example board in CI; the exact build is 12–14 s on the ESP32.
- *A check at every export.* The exporter compares each built body's volume with the plan's area × thickness and reports a mismatch as an export finding. This covers boards no test ever saw.
- *Flattening is the one real difference.* Fills go to the kernel simplified to a 10 µm chord (`MODEL_CHORD_MM`) and in 12 mm tiles. State both in the plan, not in a consumer.

**2. The assembly link on full-board names.** Section 1.2 above. Cost in time as well (measured): the full board takes 7.5 s to save, 9.3 s to open and 17.1 s for one Update Links; the mechanical subset 2.8 s, 3.6 s and 4.9 s.

**3. Picking and dimensions when the mesh is not the B-rep.**

- Board objects: unchanged. Picks are already mesh ray casts mapped to sources (`scene::Geometry::raycast`, `project::Source`).
- Part faces: unchanged. They resolve on the part's own design.
- Board edges and holes as dimension references: these are document entities (sketch curves, hole ids), not B-rep faces. A wall triangle must map back to its outline curve. `prism` already tags each wall with its outline index (`tri_face`); `slab` tags walls only by kind, so the mesher needs a per-wall source index. Small, but required.
- Measuring between copper objects in 3D must use the exact 2D geometry and the stack heights, never the mesh. The 2D geometry is already the authority for clearance checks.

**4. The realistic view and materials.** Looks are data (`material.rs`) applied per body kind; the viewport needs meshes with normals, not a kernel. Nothing in the realistic view depends on B-rep board bodies. One trap: today a body's kind is read back from the native file's metadata. Under option 3 the plan carries kind and look, and the exporter writes them. If the exporter kept its own table, looks would drift between view and file.

**5. The browser.**

- The kernel compiles to wasm, but an exact board needs about 1 GB, a quarter of wasm32's address space, and evaluation is single-threaded (read). The direct mesh makes the board viewable there; the exact export should stay a desktop or server action until measured.
- Reading a linked design in the browser needs the whole engine in the page. The cached interface snapshot lets a project open and solve without it.

**6. Very large boards.** The mesher is fast enough; memory is the limit. Solidcraft's `Mesh` holds positions and normals as `f64` without sharing between faces, about 107 bytes per triangle. For 20,000 vias and 60,000 tracks:

| Measure | Triangles | As held |
| --- | --- | --- |
| Five laminate slabs, vias not shared | 30.7–32.5 M | 3.4–3.6 GB |
| One laminate slab, vias not shared | 16.8 M | 1.8 GB |
| One slab, vias shared (as the app already draws them) | about 10 M (estimate) | about 1.1 GB (estimate) |

Further steps, all inside the present renderer path: build inner layers only when they can be seen; fewer segments for small holes far from the camera; a compact vertex format. And `MAX_TRACKS` is 2,000, so none of this is reachable in a real document yet.

**7. STEP fidelity.** Unchanged in kind: STEP comes from the exact build. What changes is when the user pays: at export, not while editing. Consequences:

- Fills appear as several bodies (12 mm tiles), as already accepted on 10 November.
- A large board's exact export may take minutes or fail. It needs a clear progress and failure report, and a default that does not include copper.
- There is no shortcut through meshes: STEP refuses them.

**8. Version skew.** Sparksmith reads and writes `.solidcraft` with its pinned engine. A user's installed SolidCraft may be newer. A linked design saved by a newer app may be unreadable until Sparksmith repins, and Sparksmith's rule is to reject unsupported versions. Neither the lead's design nor the 9 November note mentions this. A versioned exchange file (`.craftx`) is the long-term answer; until then it is a stated limitation with a clear message.

**9. Save cost.** Saving the exact model is itself slow: 2.8 s for the mechanical subset and 7.5–8.4 s for the full board (measured). Any "publish on every edit" idea has to count it.

---

## Part 2. Design note: board interface and assembly links

A linked design is any other SolidCraft design or assembly that links the board, for example an enclosure that cuts openings for its connectors. Nothing here is built except the headless prototype (2.7).

### 2.1 Principles

1. **An assembly references the board only through exported datums.** Anything else in the published file is for looking at.
2. **A datum's identity never depends on build order, feature numbers, designators or names.**
3. **Each value has one owner.** The board owns its outline, holes and placements. The linked design owns its walls and openings. A dimension lives in the file whose geometry it drives.
4. **Updates are reviewed and bound to revisions, in both directions. Never silent.**
5. **A reference that no longer resolves is reported in the user's words and blocks fabrication output** (founder direction 11 of 9 November).
6. **Solidcraft stays generic.** Nothing Sparksmith-specific goes into Solidcraft (founder, 3 November). Upstream asks are generic.

### 2.2 Datum kinds and naming

**Geometry kinds.**

| Kind | Geometry | Example |
| --- | --- | --- |
| Frame | origin and two axes | the board origin; a part's placement frame |
| Plane | origin and normal | board top, board bottom; a connector's mating plane |
| Axis | point and direction, optional diameter | a mounting hole; an LED's light axis; a shaft |
| Profile | closed loops of lines and arcs on a plane | the board outline; a cutout; a connector opening |
| Volume | a profile and a height along its normal | a keepout; a part's envelope; the space a plug needs |
| Parameter | name, value, unit | board thickness; width |

**Roles.**

| Role | Kind | Source |
| --- | --- | --- |
| `board.frame`, `board.top`, `board.bottom` | frame, planes | the board |
| `board.outline`, `board.cutout` | profile | the board sketch; each curve keeps its sketch curve id |
| `board.thickness` and exported board parameters | parameter | the stackup and board parameters |
| `hole` | axis with diameter | each mounting hole (board hole id) |
| `part.frame` | frame | a placement |
| `part.envelope` | volume | courtyard × body height |
| `part.<name>` | any | a datum the part's package declares (below) |
| `side.height` | volume | the tallest part per side, optionally per region |
| `feature` | plane, axis or profile | any feature the user exports: a part face, edge or vertex, a pad centre, a board curve |

**Identity.**

- A datum has a document id (`DatumId`, allocated like every other id) and a key: the owning object's id, the role and the local name. The owner is an instance id, hole id, sketch curve id or the board.
- A datum has a **slot**: its position in an append-only table stored in the project. A slot is never reused. A removed datum's slot is marked retired.
- A datum has a display name for people, derived and never used for identity: "J1 opening". Renaming J1 changes the display name only.
- The token written into the exported file is built from ids only: `i80.opening`, `h12`, `board`. The prototype shows why: the body name is part of what a Solidcraft reference stores.

**How a connector's opening is defined.** Three sources, in this order of preference:

1. **A datum authored on the package** (recommended as the stored form). A package revision holds `datums: [{name, kind, frame, profile, direction, clearance?, external}]` as explicit geometry in the model frame. It is revisioned with the package, and it works for every model kind, including STEP and glTF. For a connector the opening is the aperture the mating plug needs, taken from the datasheet's panel-cutout drawing, not the shell's own outline. `external: true` means "meant for the linked design".
2. **A datum declared in the part file** (an authoring convenience that produces form 1). The existing convention for marking surfaces is reused: a construction plane named `opening-<name>` with two size parameters, or a sketch of that name for a free profile; `axis-<name>` for an axis. This is how `marking-<name>` and `pin1` already work (`docs/part-templates.md`). The check that reads marking surfaces reads these.
3. **A face of the part model** (ad hoc). The user exports a feature that is a face reference, resolved by the existing `model_refs` trace with its `Resolved`, `Inexact` and `Lost` states. Useful, but the weakest: STEP face names are found by geometry.

An opening is never inferred from a part's name or category. That follows the rule against inferring compatibility from matching names.

### 2.3 What is exported

**Automatic** (on by default, each can be switched off):

- the board frame, top, bottom, thickness, outline and cutouts;
- every mounting hole;
- every package datum marked `external`, for each placed instance;
- the tallest-part height per side.

**Marked by the user:**

- a part's frame or envelope;
- any other package datum;
- any dimensionable feature, with "Export to linked design" in the Properties card and the context menu, in Layout and in 3D;
- board parameters.

**UI** (within the shared containers; one new card):

- an **Interface** card in the Board panel: the datums as a list (name, kind, source, status), an export toggle per row, the published revision and the link status;
- a chip on exported objects in 2D and 3D;
- the two previews (2.5) as the existing impact-preview dialog.

**Commands** (typed, through the one engine; one undo step each; programmatic calls never open dialogs):

| Command | Does |
| --- | --- |
| `interface.list` (query) | Datums with id, slot, name, kind, source, geometry, status |
| `interface.export {feature \| datum, name?}` | Exports a feature or package datum; allocates a slot |
| `interface.unexport {datum}` | Retires its slot; the preview names linked design features that use it |
| `interface.rename {datum, name}` | Display name only |
| `interface.auto {rule, on}` | Switches an automatic rule |
| `interface.publish_preview {tier?}` (query) | The diff against the last published revision, and the dry run on the linked design |
| `interface.publish {path, tier, expect_revision}` | Writes the file atomically; records the revision |
| `package.datum.add \| edit \| remove` | On a package draft |
| `assembly.link`, `assembly.preview`, `assembly.update {expect_hash}`, `assembly.relink`, `assembly.unlink` | The linked design side, as designed on 9 November |

### 2.4 The export format

**A `.solidcraft` design, written by Solidcraft's own commands and writer.** Its timeline has two sections.

*Section 1, the slots, always first and always in slot order.*

- Each slot uses a fixed number of feature ids (the prototype uses one; a real slot needs a small fixed budget, say four, padded). A retired slot is made and deleted, so it still uses its ids. Solidcraft never reuses a feature id (read; relied on by the prototype, measured).
- Because every slot's ids are fixed by its position, every datum's face names are the same in every build, on every machine, after every restart. No persistent native design has to be kept.
- A profile datum is a sketch whose curves carry explicit ids (Solidcraft's sketch commands accept them) taken from the document's curve and hole ids, extruded once. Solidcraft names a side face after its sketch curve, so the board's edges and hole walls get stable names from one feature. This part is read, not yet run.
- How each kind is carried today, when only solids cross a link:

| Kind | Carrier |
| --- | --- |
| Board outline, cutouts, holes, top, bottom | one slab body `board`; its side faces, hole walls and caps |
| Opening | a prism body from the mating plane outward along the insertion direction: the space the plug occupies, usable directly as a cutting tool |
| Axis without a hole | a thin cylinder body |
| Volume | a prism body |
| Frame, parameter | the sidecar and `params` only, until parameters can cross |

*Section 2, optional, after the slots:* the mechanical board (laminate, pads, parts) or the full board (with copper). Its feature numbers may change freely. The preview warns when an assembly references anything in it (2.5).

**Tiers.**

| Tier | Contains | ESP32, measured |
| --- | --- | --- |
| Interface | slots only | 11 KB; build 9–24 ms, save 12–28 ms, Update Links 10 ms (5 slots) |
| Mechanical (recommended default) | slots, laminate, pads, parts | build 5.3 s, save 2.8 s, Update Links 4.9 s; 73 KB |
| Full | slots and every copper body | build 12.3–14.4 s, save 7.5 s, Update Links 17.1 s; 736 KB |

**A sidecar, `<name>.interface.json`,** and the same content in the design's document metadata under a reverse-DNS key (in chunks under Solidcraft's 64 KiB value cap, as body kinds are stored today):

```json
{
  "format": "sparksmith-board-interface/1",
  "source_id": "…permanent random id of this board…",
  "revision": 14,
  "hash": "sha256 of the canonical datum table",
  "units": "mm",
  "datums": [
    {"id": 412, "slot": 2, "token": "i80.opening", "name": "J1 opening",
     "role": "part.opening", "kind": "volume", "owner": {"instance": 80},
     "frame": {"origin": [20, 0, 3.29], "normal": [0, -1, 0]},
     "profile": "…lines and arcs…", "clearance_mm": 0.3,
     "status": "resolved", "hash": "…",
     "faces": {"front": "F7:start"}}
  ],
  "retired_slots": [3]
}
```

**Versioning.**

- `format` has a major version. A reader rejects a major it does not know. Fields may be added within a major.
- `revision` counts publishes whose content hash differs. Each datum has its own hash, so a diff is per datum.
- Renumbering slots (a deliberate compaction) is a new `source_id` epoch with an impact preview that says every assembly reference must be picked again.

**Relation to RFC 0001's exchange file.** The sidecar is shaped to become the `board.json` part:

| RFC 0001 | This design |
| --- | --- |
| `manifest.json`: `source_id`, `revision`, hash, units | the same four fields |
| `bodies/<n>.step` or `.record` | the tier's solid bodies, through Solidcraft's exporter |
| `faces.json`: opaque persistent names, `exact`, aliases | names built from ids (`i80/opening/front`), status as `exact`. Exporter-chosen names make the slots unnecessary |
| `params.json` | exported board parameters, read-only |
| `board.json`: outline, cutouts, holes with names, placements and heights, keep-outs | the datum table, which adds named openings, axes and user-exported features |
| Outcomes exact, merged, split, by geometry, missing | adopted as the reference statuses on both sides |

### 2.5 Link updates

**Board → linked design, file-based.** J1 moves; the cutout follows.

1. The user edits the board. Nothing is written; an edit is never blocked. The Interface card shows "3 datums changed since revision 14".
2. `interface.publish_preview` lists, per datum: unchanged, moved (by how much), reshaped, added, retired, or status changed.
3. If a linked design is linked, the preview also runs a **dry run**: Sparksmith copies the linked design file to a scratch folder, opens it in its own engine, points the copy's link at the candidate interface file, runs `component.update_links` and reads the result. It reports, in the linked design's own names: features that moved (the cutout's faces before and after), features that failed, joints that no longer solve, and references to section-2 bodies ("this sketch is on a face that is not exported; export it to keep it stable"). The user's linked design file is not touched.
4. The user confirms. `interface.publish` writes the file atomically and records revision 15 and its hash.
5. In SolidCraft the user runs Update Links (one undoable step).

**Linked design → board outline, file-based.** As designed on 9 November (`docs/board-sketch-references.md` §7), unchanged in substance:

1. The project stores a link: the linked design's path hint, its file hash, the references (`FollowedFace`), the parameter bindings and a snapshot of the last resolved geometry. The project opens and solves on the snapshot without the linked design.
2. Sparksmith notices the linked design's hash changed and shows "linked design changed".
3. `assembly.preview` evaluates the linked design in-process, resolves the references in the board's frame, solves the board sketch and the dimensioned parts on a candidate, and lists what would move, what would be lost or inexact, and cycles.
4. `assembly.update {expect_hash}` applies it as one undo step, or refuses atomically and says why.

**Live, both apps open.** The file stays the contract; the channel only carries notices, as RFC 0001 recommends.

- Sparksmith connects to SolidCraft's control channel (loopback, per-launch token file) and calls `control.subscribe`. A `design.changed` event for the linked design document marks the link stale at once, instead of at the next save.
- After a publish, Sparksmith can offer "Update in SolidCraft now": it sends `engine.execute component.update_links` over the channel. This is off unless the user turns it on per link, and it always follows the reviewed preview. The step is one undo step in SolidCraft.
- When RFC 0001's session messages exist (`exchange.changed`, `exchange.request`), they replace the direct command.

**Conflicts and broken references, in plain words.** Sparksmith can always translate, because it knows the digest of every name it published.

| Situation | How it is detected | What the user reads |
| --- | --- | --- |
| The linked design holds an older board | the SHA-256 on the linked design's link is not the last published file's | "The linked design still uses board revision 13. Run Update Links in SolidCraft, then check again." |
| Both sides changed | the above, and the linked design's own hash changed | "The board and the linked design both changed since they last agreed. Update the linked design first; nothing was applied." |
| A datum the linked design uses is gone | the dry run's failed feature names a digest Sparksmith published | "The linked design's sketch 'USB opening' uses J1's opening. J1 was deleted, so the cutout would fail. Restore J1, or pick another opening in SolidCraft." |
| A reference is on an unexported face | the dry run finds a link whose digest is not a slot's | "Sketch 'Lid rib' is attached to a face of U1 that is not exported. It may move to another part when the board changes. Export it?" |
| The board's driven outline was edited locally | the curve is owned by a reference | "This edge follows the linked design's wall. Break the link to edit it here." |
| A cycle | the dependency graph over both files | "Hole H2 follows boss 'Boss 3', and the linked design places the board by hole H2." |
| The linked design is missing or moved | the path does not resolve | "The linked design file was not found. The board uses the last known geometry." with Relink |
| The linked design is from a newer SolidCraft | the reader refuses the version | "This linked design was saved by a newer SolidCraft than this Sparksmith can read." |

A lost or inexact reference blocks fabrication output without a waiver.

### 2.6 What is needed

**On the Solidcraft side: nothing for the first version.** The prototype uses only shipped commands. The following are generic, upstreamable improvements, each mapped to RFC 0001:

| # | Ask | Why | RFC 0001 item |
| --- | --- | --- | --- |
| U1 | A reference must not move to another body when repeated bodies are renumbered; report it instead | The silent retarget (measured). A defect report in its own right | §4 outcomes: "never dropped quietly" |
| U2 | Exporter-chosen persistent names for faces and bodies, kept across the link | Removes the slot padding; lets real part faces be stable | `faces.json`: "receivers treat them as opaque strings" |
| U3 | Linked construction geometry and sketches with named curves | Datums without phantom solids | §4: "a linked body and sketch"; `board.json` |
| U4 | `component.update_links {dry_run}` | A preview inside SolidCraft, without Sparksmith | `bridge.links` reporting `changed` and missing references |
| U5 | Update Links lists lost sketch links, and names the source's own name, not a digest | Today the result's `warnings` omit them; the error shows `F3:link:216d…` (measured) | §4: "naming the face and where it was" |
| U6 | A permanent source id and revision on a link, next to the hash | Tell "older" from "different" | `design-source-id`, link `{source_id, revision, hash}` |
| U7 | Parameters across a link, read-only | Thickness and exported board parameters in the linked design's expressions | `params.json` |
| U8 | A display label separate from a body's identity | "J1 opening" in the browser while the identity stays `i80.opening` | `meta.json` |

**On the Sparksmith side,** mapped to the option-3 plan:

| Need | Uses |
| --- | --- |
| Datum model, slot table, commands | the core document; independent of 3D |
| Package datums and the part-file convention | the library; the part evaluation that already reads marking surfaces |
| Board slab and volumes for the slots | the board plan (A1) |
| The interface builder | Solidcraft commands, like the exact exporter, sharing its code for the tiers |
| The dry run and the linked design reader | the in-process engine, on a worker |
| Reference display in 3D | the direct mesh view; datums drawn from the document, not from the file |

### 2.7 The prototype target and its acceptance criteria

**Already shown headlessly today** (`prototype/src/link.rs`, logs in `prototype/logs/`):

| Criterion | Full-name path (`link.log`, `retarget.log`) | Slot interface (`iface.log`) |
| --- | --- | --- |
| J1 moved 2 mm in the board | model updated in place, 0.29 s | interface rebuilt, 9 ms |
| The exported file is updated | saved in 2.8 s | saved in 12 ms |
| The linked design re-evaluates with the cutout moved 2 mm | yes: centre x 20.0000 → 22.0000; Update Links 4.9 s | yes: 20.0000 → 22.0000; Update Links 10 ms |
| The same board built afresh keeps the reference | yes | yes |
| An unrelated part deleted leaves other references alone | **no**: silent retarget, or broken | yes: R5's reference stayed at (24.0, 11.5) |
| A broken reference is reported | yes, as a feature error with a digest; not in Update Links' own result | the same; Sparksmith must translate |

Limits of the prototype: datums are boxes made by hand from the placement, not from package datums; the board is edited by changing the document's data, not through typed commands; the linked design is a session in the same process, not the SolidCraft app.

**Acceptance for the real slice (B3 and B4).**

1. Through typed commands only: the placement move of J1 by 2 mm, `interface.publish_preview`, `interface.publish`.
2. The preview lists exactly one moved datum, "J1 opening, 2 mm along X", and one linked design feature that follows, by its SolidCraft name.
3. A headless SolidCraft test document (a wall with a cutout projected from the opening, and a boss mated to a mounting hole) re-evaluates after `component.update_links` with the cutout moved 2.000 mm ± 1 µm and no other face of the wall moved.
4. The same after closing and reopening the project, and on a second machine, with the same face names (compare the sidecars byte for byte).
5. Deleting an unrelated part, adding a pour and renaming J1 change nothing in the linked design.
6. Deleting J1: the preview names the cutout that would fail, before anything is written. After publishing anyway, `assembly_link` status reads "broken reference", fabrication output is blocked, and undo restores everything.
7. The linked design → board direction: the wall moved 1 mm in the test document; `assembly.preview` lists the outline edge and the parts held to it; `assembly.update` applies one undo step.
8. Publishing the interface tier takes under 100 ms on the ESP32 board (target; 21–52 ms in the prototype with 5 slots).
9. A VM run with the real SolidCraft app repeats 3 and 7 by hand.

### 2.8 Slices

| Slice | Content | Needs | Hours | Gate |
| --- | --- | --- | --- | --- |
| B0 | Contract: datum and slot types, commands, findings, schemas, glossary, test skeletons | — | 4 | Reviewed by the founder |
| B1 | Datum model in the core, automatic rules, slot table, `interface.*` commands, persistence | B0 | 10–12 | Slots survive undo, redo, save and reopen; retired slots never reused |
| B2 | Package datums; the part-file convention; authoring in the package editor; connector templates emit openings | B0 | 8–10 | A USB-C part file's opening reaches a placed instance in the board frame, both sides, all rotations |
| B3 | Interface builder with slots and tiers; sidecar and metadata; `publish_preview` (diff only) and `publish` | B1, B2, A1 | 10–12 | Acceptance 1–5 and 8 |
| B4 | Assembly link: reading, references, bindings, snapshot, `assembly.preview` and `update`, relink; the in-process dry run | B3 | 14–16 | Acceptance 6 and 7 |
| B5 | Two-way check: consumers by slot, the older-board check by link hash, the cycle graph | B4 | 6–8 | Each row of the conflict table has a test |
| B6 | UI: Interface card, export actions, status, both previews, datums drawn in 2D and 3D | B4 | 10–12 | VM screenshots reviewed |
| B7 | Live: control-channel subscription, stale notice, optional "Update in SolidCraft now" | B5 | 8–10 | Both apps in the VM |
| B8 | `.craftx` writer and reader | upstream RFC phases 0–1 | 8–12 | Blocked on upstream |
| B9 | VM run with a real linked design; measurements | B6 | 6–7 | Acceptance 9; timings recorded |

B0–B6 and B9: 68–81 hours. The 9 November plan's S6 (14 h) is dropped; its S7–S10 (28 h) are B4–B6 and B9.

### 2.9 Risks

| Risk | Likelihood | Effect | Handling |
| --- | --- | --- | --- |
| Slot ids are not as stable as the prototype suggests (another Solidcraft version allocates ids differently) | Low within a pin; real across repins | Every assembly reference breaks at once | A golden test of the sidecar's face names in CI; a repin that changes them is an explicit, previewed epoch change. U2 removes the risk |
| Datum solids confuse the linked design (mass, interference, bill of materials) | Certain to be noticed | Annoyance; wrong mass if not excluded | A separate component named as reference geometry; openings sit in free space; U3 removes them |
| Users reference unexported faces in SolidCraft anyway | High | The silent retarget | The dry run finds and names them; one click exports the face |
| The linked design is too slow to evaluate in-process for a preview | Unknown; not measured | Slow previews | Off the UI thread, cancellable, with the snapshot as fallback; measure in B9 |
| Version skew between the pin and the installed app | Medium | A linked design cannot be read | A clear message; `.craftx` later |
| RFC 0001 lands differently from this design | Medium | Rework of the sidecar | The sidecar is versioned and small; decide whether to help shape `board/1` |

---

## Part 3. Recommendation

### 3.1 Target architecture

```
                         ┌──────────────────────────────────────────────┐
                         │ Document (the project file)     single source │
                         │ parameters · sketched outline · placements    │
                         │ copper · library snapshot · datums and slots  │
                         │ assembly link and its snapshot               │
                         └───────┬───────────────────────┬──────────────┘
                                 │                       │
                     ┌───────────▼──────────┐   ┌────────▼─────────────┐
                     │ Board plan (pure)    │   │ Part revisions        │
                     │ bodies: object, kind,│   │ one Solidcraft design │
                     │ layer, z, loops, look│   │ per package revision  │
                     └───┬─────────────┬────┘   └───┬───────────┬──────┘
                         │             │            │ evaluate  │
              mesh (ms)  │             │ commands   │ once,     │
                         │             │ (seconds)  │ cached    │
        ┌────────────────▼───┐   ┌─────▼────────────▼────┐      │
        │ Live 3D scene      │◄──┤ Solidcraft engine      │      │
        │ board meshes       │   │ (library, on workers)  │      │
        │ + part meshes      │   │ part evaluation · STEP │      │
        │ + display layers   │   │ exact board · interface│      │
        │ viewport, renderer │   │ assembly read, dry run │      │
        └────────────────────┘   └─────┬──────────────────┘      │
                                       │ Solidcraft's writer     │
                         ┌─────────────▼──────────────┐          │
                         │ board.solidcraft            │◄─────────┘
                         │ 1 slots (stable datums)     │   linked by
                         │ 2 mechanical or full bodies │──────────────► assembly.solidcraft
                         │ + interface sidecar         │◄────────────── read back, reviewed
                         └─────────────────────────────┘
```

| Concern | Owner | Engine's part |
| --- | --- | --- |
| Board outline solve, dimensioned parts | Document, through Solidcraft's sketch solver | the solver crate only |
| Laminate, copper, drills, films in the live view | Board plan → mesher | none |
| Part models | Library revisions as Solidcraft designs | evaluation once per revision; cached |
| Part placement in the live view | Document transforms | none (the export writes joint expressions) |
| Dimensions to part faces | Document, traced on the part's design or its cached face table | naming and tracing |
| Realistic view | Looks as data; Solidcraft viewport | renderer only |
| Exact board, STEP, `.solidcraft` | Exporter from the plan | everything |
| Interface | Document datums → interface builder | commands and writer |
| Linked design reading and dry run | Sparksmith, in-process | everything |
| Connectivity | The copper graph alone | none |

### 3.2 Staged path

Stages A are option 3; stages B are the interface (2.8). Hours are estimates.

| Order | Stage | Content | Hours | Gate |
| --- | --- | --- | --- | --- |
| 1 | 0 | Founder decisions; rule and skill text; this note into `docs/` | 2 | Approved; done 2026-11-12 |
| 2 | A1 | Extract the board plan from `project.rs`; the exact build consumes it; nothing visible changes | 10–14 | The exact build from the plan equals today's on every example board, as sets of bodies by kind, source, layer, box and volume |
| 3 | B0, B1, B2 | Interface contract, datum model, package datums (parallel with A2) | 22–26 | 2.8 |
| 3 | A2 | Mesher for plan bodies; the scene worker uses it behind a setting; picking identity; per-body memo | 14–20 | Parity on every example board; ESP32 board bodies in under 100 ms (target); a width edit shown in under 150 ms after the refill (target) |
| 4 | B3 | Interface export: the prototype target | 10–12 | Acceptance 1–5, 8 |
| 5 | A3 | Parts placed by the document's transforms from their own designs; no board-level native session in the live path | 8–12 | Every placement matches the exact export's joint-solved pose within 1 µm, both sides, all rotations |
| 6 | A4 | Remove the live native board and its kept undo models; the direct mesh becomes the only live path | 6–8 | 3D open on the ESP32 under 300 MB (target; about 1.8 GB live today, read in `docs/3d-speed-profile.md`); undo needs no kept model |
| 7 | B4, B5 | Assembly link and two-way check | 20–24 | Acceptance 6, 7; conflict table |
| 8 | B6 | UI | 10–12 | VM review |
| 9 | A5 (built) | Export tiers, progress and cancel, the per-export volume check | 8–10 | A full export reports any body that differs from the plan (`export::tests::a_full_export_reports_a_body_that_differs_from_the_plan`) |
| 9a | G1 (built) | The volume check replaced by a geometry-fit check: each body's top and bottom faces against the plan's loops (exact difference area, maximum deviation), its holes and heights, volume only as one more sign; laminate slabs checked by outline; an unfit export refused unless written anyway | 6–8 | A dropped or moved laminate hole is caught and refused (`export::tests::a_dropped_or_moved_laminate_hole_is_caught`, `an_export_that_does_not_fit_is_refused_unless_written_anyway`); every example board fits (`cargo xtask board-parity`) |
| 10 | B7, B9 | Live channel; VM run and measurements | 14–17 | Acceptance 9 |
| Later | A6 | Scale: lazy inner layers, compact meshes, the laminate's tile size following hole density; then lift `MAX_TRACKS` | 10–16 | 20,000 vias and 60,000 tracks under 1 GB and 1 s (target) |
| Later | A7 | Persisted part cache (mesh, face table, datums) | 10–14 | A board opens in 3D with no kernel call |
| Later | B8 | `.craftx` | 8–12 | Upstream |

Why this order:

- **A1 first.** Both the mesher and the interface builder read the plan, and it changes nothing visible, so it can be verified against today's build exactly.
- **B3 before A3–A4.** The interface is small, independent of the live view, and removes the reason to keep a persistent native board. Once it works, deleting the live native board (A4) loses nothing the linked design plan needed.
- **A4 only after parity has run on every example.** Until then the exact build stays available as the live path behind the setting, which is also the fallback.
- **Upstream's stitch fix (option c1 of the speed profile) only with a separate go; nothing is posted upstream for now.** It does not block anything here, but it bounds the exact export, which is the one slow step left.

Totals: stages 0 to 10 are about 124–157 hours; the later stages 28–42.

### 3.3 Risks of the whole plan

| Risk | Handling |
| --- | --- |
| The plan extraction disturbs the tuned in-place replay | A1 keeps today's command stream byte for byte where it can; the replay tests are the gate |
| The mesher and the kernel flatten curves differently | Flattening and tiling are stated in the plan; parity tolerances derive from them |
| Mesh memory on large boards | Not a release-1 risk while `MAX_TRACKS` is 2,000; A6 before lifting it |
| Exact export fails on large boards | Mechanical tier as the default; a clear failure report; the upstream stitch fix |
| The team keeps "fixing" the exact build for speed | After A4 its only speed requirement is "an export finishes"; say so in the skill |

### 3.4 Rules and decisions that change

Adopted on 12 November 2026. `AGENTS.md`, `skills/pcbcraft-solidcraft/SKILL.md` and `docs/pcbcraft-plan.md` carry these texts; `cargo xtask board-parity` is added with the board plan (A1).

**AGENTS.md hard rule 2.** Former text:

> Reuse Solidcraft's engine for 3D construction, evaluation, tessellation and native persistence. The live 3D model and saved `.solidcraft` model share definitions and parameter relationships. STEP is an optional exchange format. Do not introduce a second 3D engine or a private native-file writer.

Adopted text:

> Reuse Solidcraft's engine for part models, exact construction, STEP and native persistence. Do not introduce a second kernel or a private native-file writer. The board's flat bodies (laminate, copper, drills and films) are prisms of the document's 2D geometry, described once by the board plan. The live 3D view meshes the plan directly; the exact `.solidcraft` board is built from the same plan for export and as the test reference. Neither may add, drop or reshape a body, and `cargo xtask board-parity` holds them together. The saved model keeps the parameters and placement relationships that recompute without Sparksmith. STEP is an optional exchange format.

**Hard rule 3.** Former: "Copper is real thickness-aware geometry. Preserve PCB object, net and layer identity through rendering, picking and persistence. Selection and camera changes must not rebuild geometry." Adopted, one clause added:

> Copper is real thickness-aware geometry. Preserve PCB object, net and layer identity through the board plan, rendering, picking and the exported model. Selection and camera changes must not rebuild geometry.

**A new hard rule.**

> An assembly references a board only through its published interface: datums with ids and slots that survive edits, rebuilds and renames. Link updates in either direction go through a preview bound to both files' revisions and are never silent. A reference that no longer resolves is named in the user's words and blocks fabrication output.

**The "Start here" paragraph** gains `cargo xtask board-parity` in the list of enforced boundaries.

**`skills/pcbcraft-solidcraft/SKILL.md`.**

- "The displayed model and `.solidcraft` output share parametric definitions" → "share the board plan and the part designs".
- "Reuse shared-engine primitives or add a native copper collection …" → the board plan and its two consumers; the engine guidance (one command per kind, tiles) now applies to the exporter.
- "Native bodies stay physical and disjoint … display-only meshes from `appearance.rs`, never native features or saved" stays, with "physical board bodies are meshed from the plan in the live view and built natively for export".
- A new section for the interface: slots first, id-based tokens, never reference section-2 bodies from tests or tools.

**`docs/pcbcraft-plan.md`,** section "Native Solidcraft integration": "The live PCB 3D view consumes this engine-evaluated model. Saving for Solidcraft serializes that same model; there is no separate export reconstruction with different placement rules" → the export is a construction from the same plan, checked against it.

**Earlier decisions.**

| Decision | Change |
| --- | --- |
| 5 November: document-mesh rejected; "one Solidcraft file per part, shared instances; the 3D editor is basically Solidcraft" | Reversed for the board's flat bodies only. One Solidcraft design per part, shared by placements, stays |
| 9 November, decision 1: the published board design is the full board | Replaced: the interface is published; the mechanical tier is the default companion; the full board on request |
| 9 November, decision 2: stable names through a persistent board design (S6) | Replaced by slots; S6 dropped |
| 9 November, direction 10: no upstream proposal for live cross-file links | Kept: no upstream posting (12 November), although upstream now has RFC 0001. U1–U8 stay unposted |
| 9 November, decision 3: glTF models have no face references | Unchanged for faces. Package datums give glTF parts openings and axes all the same |
| 10 November: fills in 12 mm tiles in the saved model and STEP | Unchanged; now affects the export only |
| 3 November: nothing Sparksmith-specific in Solidcraft | Unchanged; honoured by the slot design |

---

## Appendix

### A. Solidcraft facts at pin 64bebfef0, with sources

Paths are inside the pinned checkout (`~/.cargo/git/checkouts/solidcraft-258829c3ab1d7265/64bebfe/`).

| Fact | Source |
| --- | --- |
| solidcore: "a pure-Rust port of NVIDIA's SMLib", `forbid(unsafe_code)`, Apache-2.0, builds for wasm32 | `solidcore/crates/solidcore/Cargo.toml`; `solidcore/crates/solidcore/src/lib.rs:40`; `solidcore/README.md:31` |
| Upstream CI checks every layered crate for wasm32 | `xtask/src/main.rs:466-490`; `.github/workflows/ci.yml` |
| `solidcraft-sketch` depends on `solidcore-record` and `solidcraft-geom` only | `crates/sketch/Cargo.toml` |
| Viewport crates depend on `solidcore-record`, `solidcore-tess`, `solidcraft-color` | `crates/viewport*/Cargo.toml` |
| All workspace crates declare MIT OR Apache-2.0 at this pin. The brand restriction on the UI kit is upstream policy that Sparksmith's `cargo xtask brand-crates` enforces; not re-verified here | `Cargo.toml:9` |
| "SMLib failed (status 1001)": 1001 is `SM_ERR`, the generic error | `crates/smlib/src/lib.rs:174`; `solidcore/crates/solidcore/src/error.rs:514` |
| A linked design crosses as STEP text of its non-mesh bodies into one Import feature of one component | `crates/engine/src/cmd/component_link.rs:281-348, 585-597` |
| Mesh bodies are not linked | `component_link.rs:294-296` |
| Face names are carried by sampling geometry, then digested with FNV-1a into `link:<digest>` | `crates/doc/src/naming.rs:1188-1220`; `crates/doc/src/lineage.rs:473-514` |
| `DesignLink { path, feature, inherited, file_sha256 }`; no revision or source id | `crates/doc/src/document.rs:3188-3228` |
| `component.update_links` has no dry run; returns `updated`, `unchanged`, `failed`, `warnings` | `component_link.rs:59-68, 626-757` |
| `component.links` reports `changed` | `component_link.rs:107-116` |
| Upstream tests: a joint and a sketch on a linked face follow it through Update Links; a gone face is reported | `component_link.rs:1315-1445` |
| No user-assigned face or edge names. Named origins and construction features are a component's own | `crates/doc/src/naming.rs:1-16`; `crates/doc/src/joints.rs:248-253`; `crates/doc/src/assembly.rs:224` |
| Sketch link sources name the same document only | `crates/sketch/src/link.rs:45-96` |
| The host cannot set a linked design's parameters | `docs/book/src/guide/assemblies.md` |
| Metadata under reverse-DNS keys; 64 KiB per value, 256 KiB in all | `crates/doc/src/metadata.rs:100-145` |
| Control channel: JSON lines on loopback, per-launch token file, `control.subscribe` → `design.changed`, `engine.execute` | `docs/control-protocol.md` |
| STEP export requires B-rep for every body; a mesh fails the export | `crates/kernel/src/step_out.rs:83-86` |
| The per-feature cache key includes the digest of the whole state before it; features are evaluated one after another | `crates/doc/src/eval.rs:733-781, 1668-1681` |
| No bridge code; RFCs 0001–0003 are drafts | `docs/rfcs/` |
| RFC 0003: solidcore's `ops` is to become "the complete public modelling API", usable outside SolidCraft | `docs/rfcs/0003-solidcore-consolidation.md` |

Upstream discussion read through `gh` (not posted to): RFC 0001 in `tangency-labs/solidcraft`; issues #153 and #70 in `tangency-labs/pcbcraft`. #70 states that it "supplies PCBcraft-specific payloads" for RFC 0001 and that `board/1` is theirs to define.

### B. Prototype

A standalone crate in a temporary folder outside the repository; not kept. Its layout is recorded so the runs can be redone.

| File | Content |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `build.sh` | A standalone crate with path dependencies on the repo's crates and the pinned engine |
| `src/main.rs` | `mesh` (direct mesh timings and volumes), `exact <full\|mech\|nofills>` |
| `src/link.rs` | `explore`, `link` (full-name path), `names`, `retarget`, `iface` (slot interface) |
| `logs/mesh.log`, `exact.log`, `explore.log`, `link.log`, `names.log`, `retarget.log`, `iface.log` | Output of each run |
| `out/*.solidcraft` | The saved designs |

To run: `./build.sh`, then `target/nix/ci/arch-proto <subcommand>` in the repository checkout.

One thing to know: the repo's Nix shell sets `CARGO_TARGET_DIR=target/nix` relative to the working directory, so the prototype was compiled into the repo checkout's git-ignored build cache (`target/nix/ci/arch-proto`). No tracked file was touched.

**Key log lines.**

```
MESH esp32 [tile 4 mm, 5 laminate slab(s)]: laminate 7.0 ms, tracks 0.2 ms, fills 31.9 ms, vias(unshared) 1.9 ms, total 41.1 ms | 297 bodies, 474398 triangles
  laminate volume 3797.066004 vs area*t 3797.066004; fill volume 284.973609 vs area*t 284.973609
MESH 200x150 + 20k vias + 60k tracks [tile 1 mm, 1 laminate slab(s)]: laminate 530.7 ms, tracks 67.8 ms, fills 39.3 ms, vias(unshared) 727.8 ms, total 1366.0 ms | 16751906 triangles, ~1803.7 MB mesh

EXACT mech: build 5.34 s, save 2.82 s, file 0.07 MB | 480 features, 470 world bodies | rss 32 -> 363 MB
EXACT full: build 14.39 s, save 8.43 s, file 0.74 MB | 1272 features, 1414 world bodies, 2186 drawn bodies, 1242056 triangles | rss 32 -> 1016 MB

[3] J1 +2 mm: board model update 0.29 s (in place, 23 commands), saved in 2.84 s
    component.update_links 4.85 s -> {"failed":[],"unchanged":[],"updated":["Board"], …}
    cutout: 4 faces, centre x 22.0000 z 3.3650 (moved +2.0000 mm)
[5] full board built in 12.27 s, saved in 7.54 s
    component.update_links 17.08 s
[6] J1 deleted: … "ERROR feature 5 'USB opening': USB opening: the reference `F3:link:216d1340ccb3719d` no longer exists as it was picked; reselect it"

reference: body 'pkg15r1 body (pkg15r1:2)' top face "F3:link:5cdd40b8252af9d8" at [24.0, 11.5, 2.055]
R4 (the placement before R5) deleted: update_links warnings [only the antenna meshing note] ; projection 4 curves centred at (33.000, 10.000); notes []

[1] interface design: 5 slots, built 0.024 s, saved 0.028 s, 11073 bytes
[3] J1 +2 mm, R4 deleted: interface built 0.009 s, saved 0.012 s, update_links 0.010 s
    cutout 4 faces, centre x 22.0000 (moved +2.0000); R5 projection 4 curves centred at (24.000, 11.500); notes []
[4] J1 deleted: cutout 0 faces; R5 projection 4 curves centred at (24.000, 11.500); notes ["ERROR feature 5 'USB opening': … no longer exists as it was picked; reselect it", …]
```

### C. What was not measured or checked

- Evaluating a real linked design in-process, and the dry run's cost on one.
- The slot design with sketched profiles and explicit curve ids (read as possible; the prototype used boxes).
- Whether a part file linked inside the interface file keeps its own face names through two links. If it does, a real part's faces can be exported with stable names without waiting for U2.
- Slot id stability across Solidcraft revisions.
- Anything in the browser.
- The GUI. Nothing was run on the host except the headless prototype.
- The perf fork branches' effect on the export (given: 22 s → 9.6 s for an exported-board evaluation).
- The full ESP32 exact build under the earlier study's conditions; today's 12.3–14.4 s compares with its 15–16 s.

### D. Terms used here

| Term | Meaning |
| --- | --- |
| Board plan | The pure-data list of the board's flat bodies derived from the document: object, kind, layer, heights, loops, look |
| Exact board | The Solidcraft model built from the plan through native commands |
| Interface | The datums a board publishes for a linked design |
| Datum | One published piece of reference geometry with a stable id and slot |
| Slot | A datum's fixed position in the append-only table that fixes its feature ids in the exported file |
| Tier | How much of the board follows the slots in the exported file: nothing, the mechanical bodies, or everything |
| Linked design (assembly) | The Solidcraft design that links the board; "assembly" in the 9 November note and in command names |
