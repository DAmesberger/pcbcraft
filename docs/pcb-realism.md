# PCB realism as built: stackup, vias, silkscreen, 3D layer toggles and look

The founder asked: "the PCB realism needs more work. it does not yet look good. also layers
should be visible. and we also need via examples". They meant all three kinds of visible
layer:

- the stackup at the board edge;
- silkscreen and copper visible under the mask;
- per-layer visibility toggles in 3D.

The work was built as one shared contract and four parallel tasks: core, native, look and
toggles. This note records what was built. Its rules are the usual ones:

- Copper is real geometry.
- The saved `.solidcraft` model stays physical.
- Display-only layers are never saved.
- The format is pre-release, so types are replaced and there are no legacy readers.
- One typed engine serves the UI, CLI and MCP.

Materials, heights and looks are in [materials](materials.md).

## Core (`pcbcraft-core`)

### Copper layers and the stackup

**Copper layers** (`model/stackup.rs`)

- `CopperLayer { Top, Inner(u8), Bottom }` names a copper layer. It serialises as `"Top"`,
  `{"Inner": n}` or `"Bottom"`.
- `Layer { Top, Bottom }` keeps its meaning: the board side, for placements, footprint
  drawings, mask and silkscreen.
- `CopperLayer::side()` gives the side of an outer layer and `name()` gives "Top",
  "Inner 1" or "Bottom". `From<Layer>` turns a side into its outer copper layer.

**Stackup** (`Project.stackup`)

- A `Stackup` is made of `CopperFoil`s and `Dielectric`s. A dielectric has a kind (core or
  prepreg) and a glass style.
- The default is 2 layers: 35 µm copper on a 1.6 mm 7628 core. `preset(2/4/6)` builds the
  others.
- Heights put z = 0 at the bottom of the lowest dielectric (`copper_span`,
  `dielectric_span`).
- `layout.set_stackup` refuses a stackup that drops a copper layer still carrying tracks or
  vias, and names that layer.

**Tracks**

- `Track.layer` and `RouteRequest.layer` are `CopperLayer`s. Validation refuses a track on
  a layer the stackup lacks.
- `layout.track_set` changes a track's width or copper layer. The Track card offers the
  stackup's layers.

### Vias

**Data** (`Project.vias`, `model/via.rs`)

- A via has an identity, a net, a position, a drill, a diameter, a span, a tented flag and
  a locked flag.
- Only through vias are built. `ViaSpan::Between` reserves blind and buried vias, and
  validation refuses it.

**Commands**

- `layout.add_via` returns `Outcome::ViaAdded`.
- `layout.via_move`, `layout.via_delete`, `layout.via_tent` and `layout.via_lock`.
- `layout.route_layer_change` routes to a point, places a via there and routes on, on
  another layer, as one transaction.
- `layout.set_board_size`.
- Adding or moving a via refuses clearance to another net's copper on any layer the via
  joins. Routing refuses a track too close to another net's via. The refusals read "Too
  close to a via on GND: needs 0.2 mm".

**Connectivity**

- `BoardCopper::of` includes the vias as copper on every layer, so connectivity, airwires
  and `board.clearance` run through them.
- A via that joins nothing of its net is reported as `board.via_dangling`.

**Checks** (`layout/via.rs`)

These limits are estimated prototype defaults, not a fabricator's rules:

- `board.via_annular_ring`: the annular ring is under 0.1 mm.
- `board.via_drill_clearance`: a drill is within 0.25 mm of another net's copper on any
  layer.
- `board.via_hole_spacing`: two drills are closer than 0.25 mm.

Issues classifies the three manufacturing checks as errors and a dangling via as a
warning. Each has a group title and a fix. Findings name a via through `ObjectRef::Via`,
as "via on GND".

**Board updates**

- Vias on a net the update changes, or too close to a changed pad of another net, are
  listed in `UpdatePlan.conflicting_vias`.
- Applying needs the same confirmation as conflicting tracks
  (`remove_conflicting_tracks`), and removes the vias with the tracks.
- A locked conflicting via blocks the update.
- A via detached from a pad that changed is a warning.

### The showcase

`Engine::showcase()`, and the CLI command `pcbcraft-cli showcase FILE`, build a
44 × 28 mm 4-layer board. The app has no menu entry for it yet: write it with
`cargo run --locked --profile ci -p pcbcraft-cli -- showcase showcase.pcbcraft` and open
the file. It has:

- an SOIC-8, a QFN-16 and a 2×5 header;
- 0603 and 0805 passives;
- a polarised 3216 tantalum;
- a resistor on the bottom side.

It is routed on top, bottom and both inner layers through tented and open vias. The
ground traces are wide because there are no zones yet. The silkscreen has pin-1 marks.
The board and ERC checks pass with no findings.

## Native (`pcbcraft-solidcraft`)

### Saved physical bodies

- The laminate is the stack it is made of, with no Boolean: each dielectric is a slab
  extruded from the board outline with every drill as a hole (`Substrate`, `Substrate 2`,
  …), and where an inner foil lies at the top of a dielectric, the foil's level is a slab
  of its own whose holes are also that layer's copper (tracks and via rings; the sketch
  unites them where they overlap). The saved bodies stay disjoint and a board of
  thousands of vias builds in time about linear in their number. The sketches go in with
  one `sketch.insert_dxf` each. Like the drills before, the outline and the holes sit where
  they are now; the slab heights follow the stackup parameters.
- Through vias are saved copper (`BodyKind::ViaCopper`): a plated barrel with an annular
  ring on every copper layer (inner rings exact circles round the drill, the circle the
  foil level leaves). Each via size is one component `via_<drill>_<diameter>` (`via_0p3_0p6`)
  built once; every via of that size is an occurrence of it, placed at the via (all but the
  first in one `occurrence.copy` with `translations`), so each size is also meshed once.
  Vias sit where they are now (no per-via position parameters).
- Sources are saved compactly: absent fields are left out, the listed sources spill over
  `org.pcbcraft.electronics.sources.N`, and vias are recorded per definition in
  `org.pcbcraft.electronics.vias.N` (occurrence and via ids as differences, nets as runs);
  `saved_sources` reads them back.
- The stackup becomes native parameters:
  - `dielectric_N_thickness` for each dielectric;
  - `board_thickness`, their sum;
  - `copper_thickness` (top), `copper_bottom_thickness` and `copper_inner_N_thickness`.
- `BoardJoint` takes a `Mount`: a side, or a height in the board.

### Display-only bodies

These are never saved:

- A tented via shows as a mask dome. An open via shows a finished ring and a dark bore
  (`ViaBore`).
- Silkscreen films come from `layout::placed_graphics` in an original stroke font, on top
  and bottom. They are clipped at mask openings, climb over track caps and tents, and use
  the board's silkscreen colour.
- The board edge shows stackup bands (`EdgeDielectric`, `EdgeCopper`): a band for each
  dielectric, with the weave of its glass style, a line for each foil and a mask lip.
- Every display body carries its copper layer or side (`Geometry.layer`), which the 3D
  layer toggles use. The layer is per body, not per pick: a through-hole pad's opposite
  land is on the other side, and copper on no single layer (plated barrels, a via's
  finished rings) has none.
- A tented via's dome replaces a track's mask cap where it covers the cap's whole width,
  so its drill dimple (12 µm, within the cap's 15 µm over copper) shows at a track end.
  An open via's bore is closed by a dark shadow disc at the face of its finished ring, so
  a track ending at the via centre does not show as a bright floor in the hole.

## Look

- Generated package models have named-radius native fillets:
  - moulded body corners and top edges;
  - QFN top edges;
  - the header base;
  - chip body edges and termination ends.

  These are fillets rather than rounded sketch profiles for two reasons. A profile drawn
  from numbers would lose the body's parameters. The engine also refuses to intersect two
  rounded profile extrusions.
- The studio is turned 145° and the exposure is 1.1: one strip light's mirror image is a
  crisp-edged lacquer band at the overview's far end. Unturned, it washed a third of the
  board pale with a blurred edge. A GPU test measures the sheen (95th-percentile mask
  luminance 3.8× the median) and checks that nothing clips to white.
- The rim light grazes at 12° instead of 35°, so its widened lobe no longer glows across
  close-ups.
- Metals: gull-wing leads are satin bright tin (roughness 0.24), not charcoal; header pins
  are gold at 0.30, with a visible specular gradient instead of olive brass; chip
  terminations are matte tin at 0.32, not a mirror. Silkscreen grain is at half strength.
  Values and reasons are in [materials](materials.md).
- `write_showcase_pictures` writes a picture set of the showcase board:
  - the overview;
  - close-ups of the SOIC, QFN, header and passives;
  - an unobstructed tented via and an unobstructed open via (the test checks that the frame's
    centre picks the via);
  - a tented and an open via side by side;
  - an open via's barrel through the four layers in X-ray;
  - the edge stackup;
  - the bottom side.

  `write_layer_pictures` writes the layer toggles on the showcase: every layer, no top
  mask, X-ray, copper only, Inner 1 only, Inner 2 only, and no top copper or mask. Both are
  ignored tests that write to `PCBCRAFT_PICTURES` (`PCBCRAFT_PICTURES_ONLY` limits the
  showcase set to a name prefix).

## 3D layer toggles (`apps/pcbcraft`)

- `ViewLayer` lists the toggles: components, silkscreen and mask for each side, copper for
  each stackup layer, substrate and vias. `LayerView` holds the visibility of each one.
  Vias covers every plated hole (via and through-hole barrels, a via's finished rings and
  bore); a tented via's dome is its side's mask. The edge's foil lines go with the
  substrate.
- X-ray, next to Show all, shows only copper and vias, so the stackup's copper shows layer
  over layer.
- A Layers popover (eye icon) on the floating navigation bar holds the toggles. It lists
  the layers in board order and has Show all. The icon is outlined in the accent colour
  while any layer is hidden. PCB 2D keeps its own Layers and view popover.
- The toggles are display state only:
  - The model's PBR buffer has one run of bytes for each view layer.
  - Toggling repacks the shown bodies from the cached meshes, off the UI thread.
  - Shadows, highlight and picking follow at once.
  - No geometry is rebuilt.
- PCB 2D and 3D share one state for every copper layer, the vias and silkscreen.
  Component 3D always shows every layer.

## PCB 2D (`apps/pcbcraft`)

- Layout draws every copper layer of the stackup in its own colour: the far side and the
  inner layers dimmed under the near side, and the inner layer Route draws on at full
  strength over the near copper. The view popover has a toggle for each copper layer and
  one for vias.
- Vias are drawn as their ring and drill: a tented via's ring under a mask tint with an
  outline, an open via's ring solid. A via shows while the via toggle and a copper layer it
  joins are shown.
- A click selects a via (and lights its net). Its Via card shows net, position, drill,
  pad, ring and span, with Tented and Locked toggles and Delete via in the footer. A 3D
  pick on a via body selects the via too. A finding that names only a via locates it: it
  selects the via and its net and frames it in 2D.
- The Route card offers every stackup layer and Tent vias. While routing, V (Shift+V
  backwards) places a via at the pointer, snapped to a track end of the net within reach or
  the grid, and moves the route to the next layer. The next click issues one
  `layout.route_layer_change`. Without a route, V only changes the layer.
- The Via tool (ribbon, Edit group) places a through via on the net of the pad or track
  under the pointer, else on the chosen net, snapped to a track end or the grid.

## Board update review

Conflicting vias are one acknowledged line ("A via on GND conflicts with this update",
acknowledged by "Remove that via"). Acknowledging every track and via line confirms the
removal (`remove_conflicting_tracks`), so an update whose only conflicts are vias applies
from the GUI. The core warning that restated them is no longer shown twice.

## Build cost (measured)

`build_timings` (`cargo test --profile ci -p pcbcraft-solidcraft build_timings -- --ignored
--nocapture`, one 18-core machine shared with other builds, release-like `ci` profile). The
via boards are `via_board` in `project/tests.rs`: the showcase on 4 layers with that many
vias on a 1.27 mm grid and about 260 tracks (200 inner).

| Board | Native bodies | Before | After |
| --- | --- | --- | --- |
| Demo | 13 | 117 ms | 93 ms |
| Showcase (4 layers, 11 vias) | 244 | 7.0 s | 2.3 s |
| Demo on 4 layers, 50 vias, 25 inner tracks | 292 | 30.3 s | 0.32 s |
| Demo, 400 tracks | 413 | 13.5 s | 3.8 s |
| Showcase, 4 layers, 1,000 vias | 5,441 | fails (200 vias did) | 7.0 s |
| Showcase, 4 layers, 4,096 vias | 20,929 | fails (200 vias did) | 12.9 s |
| Showcase, 4 layers, 10,000 vias | 50,444 | refused (over 4,096) | 71 s |

"Before" is the pinned `pcbcraft-product-look` build (the showcase with 50 vias took 607 s
there); "after" was measured at a load average of about 15–40 from other builds, one run for
the two largest boards.

What made vias expensive, and what changed:

- **Booleans.** The drills were one profile cut of the laminate and each inner layer's
  copper another; past Solidcraft's prism budgets these fell back to its general B-rep
  Boolean (607 s for 50 vias on the showcase) and refused 200 vias. The laminate is now
  stacked slabs extruded with their holes, and no Boolean runs.
- **A command per thing.** Every Solidcraft command evaluates parameters and solves every
  joint again, so N commands cost N². Sketch contours go in with one `sketch.insert_dxf`, the
  placements' parameters with one `parameters.import`, every via of a size with one
  `occurrence.copy` (`translations`) and every board joint with one `joint.create`
  (`joints`), the last two added to the Solidcraft fork for this.
- **Per-via components.** A via was its own component with its own features, meshed on
  its own. A via size is now one definition placed per via.
- **Quadratic sketch steps.** Choosing a drilled plate's profiles compared every hole with
  every profile, and merging regions every segment with every other: both now look up a
  grid (`BoxIndex` in the fork's geometry crate). Above 10,000 profiles the profile choice
  used to take every profile, which filled every hole in; it now checks them all.
- **Measuring.** Every body was measured exactly as it was built; a placement of a via
  definition reuses its definition's measure, and a laminate slab's volume is measured only
  when read.

Where the time goes now (sampled at 10,000 vias, before the joints were batched): meshing the five laminate slabs (each
face of 10,000 hole walls is a NURBS cylinder that the reference mesher and its seam
classification sample one by one) about 40 %, extruding them (orienting each new body by
meshing it coarsely, and its planar checks) about 25 %, the remaining per-part commands
(pads, tracks: parameters evaluated again by each) about 20 %, and copying the sketch on each
sketch step. Per via, building and placing are now about linear.

### The 3D view at scale (measured)

Vias are drawn instanced. A body that places a shared definition (every via of a size: its
barrel, rings and finish; Pcbcraft's display-only tent domes and bore shadows, now made once
per via shape and placed) keeps its definition's mesh, `Geometry::mesh` with a `place`,
instead of a moved copy. The 3D view draws each definition mesh once per placement through
Solidcraft's PBR product-look path (`InstanceLook::Pbr` in the fork: the same material,
clear coat, pattern, studio and ambient occlusion as the vertex stream, from indexed
vertices), with coarser levels for placements small on screen (lit smooth, and held back
until a body is about 48 pixels across, so a glossy tent's highlight does not facet). Bodies
of 32,768 triangles or more (the laminate and mask with thousands of holes) are drawn the
same way at one placement, so each vertex is stored once and the vertex stream stays small.
Picking transforms the pointer ray into the definition's frame and returns the placement's
own identity; selection draws the selected placements again from the shared triangles; a
layer toggle publishes the same shared meshes (their GPU buffers are kept) with or without
their placements.

`via_display_timings` (`cargo test --profile ci -p pcbcraft via_display_timings -- --ignored
--nocapture`; showcase on 4 layers plus a grid of through vias, 1280 × 800, a frame read back
to the CPU each time, best of three for the first frame and of five for orbiting, load
average about 20):

| Vias | Scene (worker) | First frame | Orbit frame | Stream | Instanced (GPU) | Triangles drawn |
| --- | --- | --- | --- | --- | --- | --- |
| 1,000 | 93 ms | 29 ms | 8.8 ms | 27 MiB | 24 MiB | 0.71 M |
| 4,096 | 285 ms | 63 ms | 6.8 ms | 6.9 MiB | 105 MiB | 2.5 M |
| 10,000 | 577 ms | 102 ms | 9.6 ms | 6.9 MiB | 249 MiB | 5.9 M |

"First frame" is opening the view on a finished build: the scene handed to the view, its
upload, culling and the first frame. Copied into the stream, the 10,000 vias alone would
have been 11 GB of vertices (`ten_thousand_vias_hold_one_mesh_per_definition`); their
definition meshes and levels take 0.7 MB and their placements 52 bytes each. Most of the
instanced bytes at 10,000 vias are the drilled laminate and the mask openings, each hole
meshed in place (item 2 below).

Plan to reach thousands of vias in a few seconds and an instant 3D view:

1. Done: vias drawn instanced (above).
2. Mesh repeated faces once: the hole walls of one drill size differ only by a translation;
   a translation-keyed face-mesh cache in the kernel's reference mesher, and orientation of
   an extrusion from its profile instead of a coarse mesh.
3. One evaluation per build: a batch scope in the engine that defers parameter evaluation
   and joint solving to its end, for the remaining per-part commands.
4. Incremental rebuilds: keep the native session between builds and change only the
   features of the parts that changed (a moved via is one occurrence move, a moved track
   its parameters), instead of rebuilding the board on every edit.

The 3D model is prepared in the background while the board is edited in 2D (debounced
350 ms after the last change), a build that a newer board replaces stops at its next
native command, and the 3D view shows the last finished model under "Updating" while a
newer one builds, so opening the 3D view does not wait for a build that has finished.

## Merge points with the sidebar redesign

These edits touch files the sidebar slice restructures, so its merge should rebase them on
purpose:

- `pcb_editor.rs`: the Via tool and `Inspected::Via`, header, footer and issue-slot arms,
  layer toggles (`Layers::inner`, `vias`), 2D via and inner-layer painting, V layer change,
  via picking and Locate.
- `pcb_editor/board_card.rs` `view_menu`: one copper toggle per stackup layer and Vias;
  earlier, the silkscreen hint and the Copper row.
- `pcb_editor/route_card.rs`: stackup layers and Tent vias; `pcb_editor/track_card.rs`:
  stackup layer options. `pcb_editor/via_card.rs` and `via_tests.rs` are new.
- `update_preview.rs`: the grouped via conflict line, in `content` and its test.
- `app.rs`: a 3D via pick calls `PcbEditor::select_via`. `app/chrome.rs`: the 3D Layers
  button on the navigation bar.
- `layer_view.rs` uses `kit::form` and `kit::check_row`.

## Open

- There is no stackup editor in the app; stackups are set through commands, the CLI or MCP.
  Via drill and pad are the defaults (0.3/0.6 mm) when placed in the app.
- The GUI could not be checked live: the screen was locked during both runs, so windows drew
  nothing. The offscreen GPU pictures and the egui tests are the evidence.
- 10,000 vias build without error but take about a minute; the 3D view then shows them in
  about 0.1 s and orbits at interactive rates (see The 3D view at scale). See Build cost for
  where the build time goes and the plan.
- Look: THT and gull-wing solder joints are not modelled. Copper foils run along the
  whole edge (no pull-back); prepreg and core look alike at the edge; the weave's scale and
  its moiré at overview distance are in Solidcraft's shader. Seen from below, the key
  light still leaves a soft hotspot. Some showcase designators sit under or beside bodies
  (C3, R1). Silkscreen placement is not checked.
- There are no zones or pours.
- Blind and buried vias are not supported.
- The via checks use estimated defaults, not configurable rules.
