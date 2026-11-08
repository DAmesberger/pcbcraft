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
- Stackup rules (`Stackup::check`): an even number of copper layers from 2 to 64, exactly one
  dielectric between neighbouring foils, foils 5–210 µm, dielectrics 0.025–3.2 mm, Dk 1–20.
  Odd counts are refused: fabricators laminate copper in pairs (double-sided cores and foil), so
  an odd board is the next even build with one layer etched away, priced as that build and prone
  to warp. 64 is not a fabrication limit (production boards above about 50 layers are rare); it
  bounds the per-layer work a hostile or mistaken file can ask for. `Stackup::preset(n)` gives a
  symmetric foil build for every even count (prepreg outside, cores inside, 1.6 mm of
  dielectric up to ten layers, then 0.16 mm a layer).
- A layer's identity is its name (`Inner(n)`), so a stackup that gains or loses layers renames
  the foils below the change. `layout.set_stackup` keeps every name; `layout.edit_stackup`
  (`restack::StackupEdit`) also moves copper: each `{from, to}` move takes every track, fill,
  keepout layer and blind-via end on `from` to `to`, all at once (swaps work), and `removed`
  lists old layers no foil continues. Copper left on a removed or missing layer refuses the whole
  edit with a finding that names the layer and what it carries ("Inner 5 still carries 1 track,
  1 keepout; move or delete it first"); nothing is deleted implicitly. The desktop's Stackup
  dialog builds the edit from a `restack::StackupDraft`, which tracks the layer each foil came
  from, and offers a "Move to" choice for each removed layer that carries copper.
- A dielectric may carry the fabricator's Dk and a foil may be marked as a reference plane;
  the impedance model reads both ([high-speed routing](high-speed-routing.md#impedance-model)).

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
- A via that joins nothing of its net is reported as `board.via_dangling`, and a track end
  that meets no copper of its net as the warning `board.track_dangling`.

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
  of its own whose holes are also that layer's copper (round-capped tracks and via rings;
  the sketch unites them where they overlap, and a hole wholly inside another, such as a
  via ring under a wider track, is left out, as the sketch would take it as an island:
  `without_nested` in `project.rs`). The saved bodies stay disjoint and a board of
  thousands of vias builds in time about linear in their number. The holes go in with one
  `sketch.insert_dxf` per slab and sit where they are now; a rectangular board's outline is a
  rectangle to the `board_width` and `board_depth` parameters, so changing them in the saved
  file resizes every slab; a sketched board's outline is its solved outline, with its
  cutouts, slots and mounting holes as holes of every slab
  ([board shape](board-rules-and-shape.md#board-shape-cutouts-and-edge-relative-placement)).
  The slab heights follow the stackup parameters.
- An inner copper fill lies in its foil level too: its regions, united with the tracks, via
  rings and holes that overlap them, are holes of that level's slab (`fill::foil_level`), and
  laminate left in a clearance is a slab piece of its own (`Substrate 2 (2)`, …). See
  [copper fills](board-rules-and-shape.md#copper-fills-and-keepouts).
- Through vias are saved copper (`BodyKind::ViaCopper`): a plated barrel with an annular
  ring on every copper layer (inner rings exact circles round the drill, the circle the
  foil level leaves). Each via size is one component `via_<drill>_<diameter>` (`via_0p3_0p6`)
  built once; every via of that size is an occurrence of it, placed at the via (all but the
  first in one `occurrence.copy` with `translations`), so each size is also meshed once.
  Vias sit where they are now (no per-via position parameters).
- Sources and body kinds are saved compactly as columns (`project/identity.rs`; see Build
  cost); `saved_sources` reads them back.
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

- Layout draws every copper layer of the stackup in its own colour. The active layer (the
  one Route, Via and Fill use, set by 1–0 or the Route card) is always on top at full
  strength, in every tool and from either side. The view popover has a toggle for each
  copper layer and one for vias.
- **Paint order** (`PcbEditor::paint_board`, `layer_focus.rs`), bottom to top:
  1. the board, its grid and the far side's drawings (silkscreen, assembly, courtyard),
     dimmed;
  2. every copper layer but the active one, the farthest from the viewer first: its fills,
     tracks and surface pads, dimmed;
  3. the near side's assembly drawing;
  4. the through-hole pads, in the active layer's colour;
  5. the active layer: fills, tracks and surface pads at full strength (its fills opaque
     only when it is the near side; on the far side or inside they are a 45 % tint with
     their outline, `ACTIVE_FILL_TINT`, so the near side's pads read through);
  6. the other layers' selected and highlighted copper again, so a selection stays on top;
  7. vias and holes;
  8. keepouts;
  9. the near side's silkscreen and courtyard, courtyard overlaps, airwires, part outlines
     and labels, then pad numbers.

  Previews (the route step, the Tune meander, the Via tool's ring), the selected fill's
  handles and the update review's marks are drawn over the board.
- **Other layers.** Under All layers, the far side keeps 45 % of its colour over the board
  and the other layers 60 % (`FAR_SHADE`, `OTHER_SHADE`). Their fill copper is lighter than
  their tracks: a 22 % translucent tint with a thin outline, or its hatching or outline when
  the Fills row says so. A large pour on another layer therefore never hides the active
  layer's copper, nor the tracks under it.
- **Selected and highlighted pours** keep their copper colour and get a 2-point outline in
  the selection colour, so lighting a board-wide net (GND) never floods the board.
- **Layer focus** (`pcb.layer-focus`, ⇧L, or the view popover's Focus row) cycles
  All layers → Dim others → Active only. Dim others keeps 20 % of the other layers' colour
  and an 8 % fill tint. Active only draws and picks the active layer's copper alone, with
  the through-hole pads and the vias through it. The status line names the mode. It is
  view state like View from bottom and the Fills row: it is not saved and changes no
  document.
- **Keepouts** (board keepouts and the placed parts' footprint keepouts) are an overlay
  above all copper in every focus mode, whatever layer is active and whichever copper is
  shown: a hatched outline in the keepout colour, solid where it applies to the active layer
  and dashed elsewhere, labelled with its name and layers ("Antenna · Inner 2", "Keepout ·
  all layers"). A footprint keepout follows its part while it is dragged.
- Vias are drawn as their ring and drill: a tented via's ring under a mask tint with an
  outline, an open via's ring solid. A via through the active layer has its ring in that
  layer's colour; another via has the colour of the first shown layer it joins, dimmed as
  that layer is. A via shows while the via toggle and a copper layer it joins are shown.
- `layer_focus_tests.rs` measures this on the rasterized canvas: with each layer of the
  showcase active, from the top and from the bottom, every pixel inside the active layer's
  tracks and fill copper is that layer's full colour; each focus mode paints only the
  colours it says; keepouts are drawn after all copper in every mode.
- A click selects a via (and lights its net). Its Via card shows net, position, drill,
  pad, ring and span, with Tented and Locked toggles and Delete via in the footer. A 3D
  pick on a via body selects the via too. A finding that names only a via locates it: it
  selects the via and its net and frames it in 2D.
- Routing is continuous. Each click sends one step as one `layout.route_apply` and one
  undo step: an elbow in the chosen mode (45°, 90° or free; `layout::posture`, which the
  preview shares), and with a layer change also the via. Once the engine takes the step,
  the route continues from its end. A click on a pad of the net, or on its track or via
  copper on the routing layer in another island than the route's head (the copper graph's
  `islands_at`), finishes the route, and so do Enter and a double-click. Copper the head is
  already joined to, such as the start pad's fan-out, is a corner, not a finish. Esc or a right-click ends it, and Esc again
  returns to Select. Backspace sends one `layout.route_apply` that removes exactly the
  last step's tracks and via, and moves the head back. Any other document change ends the
  route.
- The Route card offers the mode, the width, every stackup layer, the via drill and pad,
  Tent vias and the net. It lists the keys (`route_tool::ROUTE_KEYS`). While routing, V
  (Shift+V backwards) or a layer's number (1 to 9) puts a via at the pointer, snapped to a
  track end of the net within reach or to the grid, and the route continues on that layer.
  The next click places the via. No via is added where the net's copper at the head
  already joins both layers: one island of the copper graph has copper on both there (a
  via whose span reaches both, a plated through-hole pad). `/` cycles the mode and F flips the elbow. W and D step
  through the width and via presets of the rules (`layout::route_widths`,
  `layout::via_sizes`). A via size typed below the rules' drill or annular ring is marked
  and not used. Without a route, V only changes the layer.
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

| Board | Native bodies | Before | Phase 1 | Phase 2 (butt-capped tracks) | Integrated (round caps) | Tracks put off |
| --- | --- | --- | --- | --- | --- | --- |
| Demo | 13 | 117 ms | 93 ms | 45 ms | 45 ms | 45 ms |
| Showcase (4 layers, 11 vias) | 244 | 7.0 s | 2.3 s | 1.0 s | 1.0 s | 0.96 s |
| Demo on 4 layers, 50 vias, 25 inner tracks | 292 | 30.3 s | 0.32 s | 0.14 s | 0.15 s | 0.15 s |
| Demo, 400 tracks | 413 | 13.5 s | 3.8 s | 0.39 s | 2.2 s | 0.60 s |
| Showcase, 4 layers, 1,000 vias | 5,441 | fails (200 vias did) | 7.0 s | 1.6 s | 2.4 s | 1.6 s |
| Showcase, 4 layers, 4,096 vias | 20,929 | fails (200 vias did) | 12.9 s | 2.3 s | 2.9 s | 2.0 s |
| Showcase, 4 layers, 10,000 vias | 50,444 | refused (over 4,096) | 71 s | 2.7–3.5 s | 4.0 s | 2.8 s |
| Showcase, 4 layers, 10,000 vias, 2,000 tracks | 52,186 | | | | | 8.5 s |

"Before" is the pinned `pcbcraft-product-look` build (the showcase with 50 vias took 607 s
there). Phase 1 and phase 2 were measured at a load average of about 5–40 from other builds;
the phase 2 10,000-via cell is the range of best-of-3 runs over several minutes
(`via_build_profile` in `project/tests.rs`, `PCBCRAFT_VIAS=10000`). The integrated column is
the merged branch (fork branch `pcbcraft-via2`), best of two runs at a load average of 5–8.
"Tracks put off" is the same branch after the review fixes, best of three runs (one for
4,096 vias) at a load average of about 6; the 2,000-track board is the 10,000-via board with
tracks added on the outer layers between its via rows up to the layout's 2,000
(`with_tracks` in `project/tests.rs`; `PCBCRAFT_TRACKS=2000` for `via_build_profile`).
`build_timings` fails when the 10,000-via benchmark board takes 3 s or more (best of three).

Merged with the board batch (design rules, fills, sketched board shape; 2 November 2026,
same machine, load average about 5, best of the runs): the showcase, now with its three
pours filled, 1.16 s (364 native bodies); 1,000 vias 1.4 s; 4,096 vias 1.8 s; 10,000 vias
2.5 s; 10,000 vias and 2,000 tracks 7.5 s (the via-scaling branch alone, measured the same hour: 0.85 s for the unfilled showcase, 1.4, 1.8, 2.5 and 7.6 s). The via boards leave the showcase's pours
unfilled (`via_board`: they were filled for another board and stackup).

The integrated build draws each track as its round-capped stadium (component-model's
`layout::track_shape`), a sketch and an extrusion per track instead of a parametric box.
Each track's sketch finish and extrusion used to evaluate the whole pending design first,
about 5 ms a track. The track's extrusion now takes every profile of its sketch whole
(`profiles: "all"`, `Native::whole_profile_body`), and the fork puts off finishing such a
sketch and extruding it in a batch (`deferrable` in the engine), so all the tracks are
evaluated once, with the rest of the board. Putting them off showed what each command cost
in a design of thousands of features and occurrences, which the fork no longer pays:

- drawing a sketch and creating it evaluated every parameter (a sketch with no
  dimension, expression point or pattern reads none, and a plane that names no parameter
  is resolved without them);
- a sketch held a copy of the design to cancel back to, and each line drawn copied the
  design again (in a batch the edit is made in place, as the batch goes back to its start
  should it fail; Cancel Sketch is refused there);
- the active component's frame rebuilt the placement index of every occurrence's bodies;
- each evaluated feature compared every body with every body of the state before it;
- placing the components looked up every body, sketch and occurrence for each placement.

The 10,000-via benchmark board (about 260 tracks) now builds in 2.8 s, under the 3 s
target. At the layout's 2,000 tracks it takes 8.5 s, about 3 ms a track: the extrusions
themselves (Solidcraft's B-rep extrude, one by one, about 1.2 ms each), each track's
component and joint, and features added to a timeline of thousands. The target is not met
there; see Open.

What made vias expensive, and what changed in phase 1:

- **Booleans.** The drills were one profile cut of the laminate and each inner layer's
  copper another; past Solidcraft's prism budgets these fell back to its general B-rep
  Boolean (607 s for 50 vias on the showcase) and refused 200 vias. The laminate is now
  stacked slabs extruded with their holes, and no Boolean runs.
- **A command per thing.** Sketch contours go in with one `sketch.insert_dxf`, the
  placements' parameters with one `parameters.import`, every via of a size with one
  `occurrence.copy` (`translations`) and every board joint with one `joint.create`
  (`joints`), the last two added to the Solidcraft fork for this.
- **Per-via components.** A via size is one definition placed per via.
- **Quadratic sketch steps.** Choosing a drilled plate's profiles and merging its regions
  look up a grid (`BoxIndex` in the fork's geometry crate).
- **Measuring.** A placement of a via definition reuses its definition's measure.

Phase 2 (fork branch `via2-mesh`):

- **Meshing the laminate** (51 s of 77 at 10,000 vias): the reference mesher projected every
  rim point onto its NURBS hole wall and the seam check fitted each of the 10,000 walls by
  sampling. A straight prism of 256 faces or more is now meshed from its rims in the kernel
  (`prism_mesh.rs`): each rim divided once by the same chord, angle and crowding rules (one
  division per hole shape, reused for every hole of that shape), each wall a strip, each cap
  one constrained triangulation; the same faces, edges, seam and tangent flags and, on the
  plates tested, the same triangles as the reference mesher. A crowded cap gives each half
  circle at least two segments (the reference left a hole of no area). About 0.3 s a slab.
- **Extruding the laminate** (12 s): orienting each new body by meshing it, and comparing
  every side with every profile piece; now read from the start cap, and looked up. A sketch
  plane that is XY raised along Z extrudes in place instead of rebuilding the body to move
  it; the solved sketch's profiles are reused when choosing the extrude's profiles; profile
  nesting and DXF ids are indexed.
- **One evaluation per build.** Solidcraft's `Session::defer_evaluation` /
  `evaluate_deferred` (fork): adding commands (components, boxes, parameters, metadata) run
  without evaluating or copying the design, any other command evaluates what is pending
  first, and the batch ends evaluated as one undo step. The board build runs in one such
  batch. Evaluating an expression reads only the parameters it names (every named feature
  input is one), and a parameter change is checked without copying the design.
- **Order and overlap.** The laminate is built last (its sketches hold every drill, which
  each later command would read again); the display-only layers (mask, finish,
  silkscreen, edge) are drawn on another thread while the laminate, joints and native
  finish run; a body whose mesh box is its outline box is measured only when its volume is
  read; all bodies are meshed at once on all cores.

Where the time goes now at 10,000 vias (about 3 s): the display-only layers on their own
thread about 1.7 s (`appearance::region::slab` and `via::via_layers`), in parallel with the
laminate (0.7 s) and the native finish (1.1 s: world bodies for 50,000 placements, their
copied meshes, `appearance.assign` frames); before them the generated package models
(0.7 s, fillets, the same for every board) and the parts and vias (about 0.4 s).

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

On the integrated branch (load average about 5): 1,000 vias build in 1.21 s, scene 69 ms,
first frame 23 ms, orbit 4.7 ms; 4,096 vias 1.66 s, 183 ms, 53 ms, 7.2 ms; 10,000 vias
2.80 s, 540 ms, 97 ms, 9.7 ms (6.1 M triangles, 267 MiB instanced, 6.9 MiB stream).

"First frame" is opening the view on a finished build: the scene handed to the view, its
upload, culling and the first frame. Copied into the stream, the 10,000 vias alone would
have been 11 GB of vertices (`ten_thousand_vias_hold_one_mesh_per_definition`); their
definition meshes and levels take 0.7 MB and their placements 52 bytes each. Most of the
instanced bytes at 10,000 vias are the drilled laminate and the mask openings, each hole
meshed in place.

Plan items of phase 1, all done in phase 2: vias drawn instanced (above), repeated faces
meshed once, one evaluation per build, and in-place updates for edits that change values
(below). Adding or removing a part, pad, track or via still builds the board anew.

The 3D model is prepared in the background while the board is edited in 2D (debounced
350 ms after the last change), a build that a newer board replaces stops at its next
native command, and the 3D view shows the last finished model under a small "Updating"
chip (top right) while a newer one is made, so opening the 3D view does not wait for a
model that has finished.

### Edits update the model in place

The model worker keeps the last board model it made (its native session, geometry and
display layers). `project::update` builds the edited board against it: every native
command the build issues is compared with the one recorded at that step and answered with
the recorded reply, and nothing runs (`native/replay.rs`). When the commands differ only
in values (a parameter's expression, a sketch's drawing, where a via placement stands,
recorded metadata) the differences are applied to the kept design: changed parameters in
one `parameters.change`, each changed drawing erased (what its step created) and drawn
again while the history is rolled back to the first changed sketch (so later features
are evaluated once), and each moved via placement moved (at most 256). Any other
difference (an object added or removed, a new package revision, a layer change) builds
the board anew. Tests check that every update's native bodies, display layers, sources
and saved metadata match a fresh build of the edited board.

What an edit does not reach is reused, so the design's history is ordered by how rarely
it changes: generated and library package models and via definitions first, then
envelopes, pads, tracks and vias, and the laminate last (its sketches hold every drill;
see Build cost). A library model's definition is built through the session directly
(`Native::session_mut`), so its commands are not recorded: an update passes over the whole
definition of the same model and use, as it does a generated one. Evaluation keeps every
feature before the first one an edit changes; a body whose evaluation and placement are unchanged keeps its mesh and measure
(`ModelBody::geometry_key`); a laminate slab evaluated again with the same holes and
heights keeps its mesh; and the display layers keep each side's mask, each land's finish
and each via's dome and bore whose inputs did not change (`appearance::Memo`). A part's
pad offsets in its joints are rounded to a nanometre, so moving a part is a parameter
change.

`update_timings` (`cargo test --profile ci -p pcbcraft-solidcraft update_timings --
--ignored --nocapture`) times a fresh build and three updates of each edit, best of three:

| Board | Fresh build | Outer track width | Part moved | Via moved | Inner track moved |
| --- | --- | --- | --- | --- | --- |
| Showcase | 1.0 s | 83 ms | 8 ms | 41 ms | 76 ms |
| Showcase, 4 layers, 1,000 vias | 2.4 s | 416 ms | 49 ms | 286 ms | 440 ms |
| Showcase, 4 layers, 4,096 vias | 2.9 s | 616 ms | 175 ms | 692 ms | 729 ms |
| Showcase, 4 layers, 10,000 vias | 3.9 s | 1.1 s | 431 ms | 2.0 s | 1.5 s |

Measured on the integrated branch at a load average of about 5, `ci` profile. (Before the
phase 2 meshing was merged, a via moved on the 10,000-via board took 72 s.)

Merged with the board batch (2 November 2026, same machine and profile): showcase (now with
its pours filled) fresh 1.2 s, outer track width 250 ms, part moved 11 ms, inner track
moved 230 ms; 1,000 vias 428 / 48 / 272 / 441 ms; 4,096 vias 567 / 175 / 677 / 668 ms;
10,000 vias 1.05 s / 425 ms / 1.9 s / 1.4 s. On the showcase the moved via ends 0.24 mm
into the inner plane's (stale) copper, which joins two pieces of the laminate at that
level, so that update builds anew (about 1.2 s). A redrawn slab or fill region whose
extrusion picked several profiles is rebuilt from its command (`timeline.redefine` in
`native/replay.rs`): Solidcraft stores such a pick by profile index or curve, and a
redrawn sketch numbers both anew.

An update takes the kept model (`kept.board.take()` in `apps/pcbcraft/src/scene.rs`): a
replay that fails consumes it, and a build anew that a newer edit cancels leaves none. The
next edit then builds the board anew (about 3 s on the 10,000-via board) instead of
updating it, until a build finishes. An update in place is not cancelled.

Moving a via redraws the holes of all five slabs and evaluates and meshes them again,
now about 0.3 s a slab, plus Solidcraft's `sketch.delete`, which looks up each named
entity by a linear search. Splitting the laminate into tiles cut along gaps between holes
would make a via edit remake one tile per slab. A via added or removed is still a fresh
build.

### Opening the 3D view

Opening PCB 3D shows the model prepared in the background: the scene is already packed
on the worker and published to the viewport when it arrives, so the first frame only
uploads it and draws. `PCBCRAFT_TIMINGS=1` prints the time from opening to the end of the
first frame with the board in it (`app/open_timing.rs`); `opening_3d_timings`
(`cargo test --profile ci -p pcbcraft opening_3d_timings -- --ignored --nocapture`, needs
a GPU) measures the same steps offscreen:

| Board | Prepared in the background | First frame after opening |
| --- | --- | --- |
| Showcase | 1.0 s | 13 ms |
| Showcase, 4 layers, 1,000 vias | 1.2 s | 24 ms |
| Showcase, 4 layers, 4,096 vias | 1.7 s | 53 ms |
| Showcase, 4 layers, 10,000 vias | 2.7 s | 96 ms |

Integrated branch, load average about 5, 1600 × 1000, best of five openings (each a new
viewport; the frame is read back, which the window does not do).

In the running app (`PCBCRAFT_TIMINGS=1`, the integrated branch, a 10,000-via 4-layer
board added over `pcbcraft-cli stdio`, PCB 3D opened on its finished background build),
eight openings drew their first frame in 8.2–37.4 ms (median about 23 ms).

Board checks and saved identity scale with the vias too. The copper, via and routing checks
file every shape in a grid by its bounds and compare only shapes within the largest rule of
each other (`layout/grid.rs`): every via of a 10,000-via, 2,000-track board is checked, in
about 20 ms for checks and airwires. The saved model records each source and body kind as
compact columns (`project/identity.rs`): the largest board the document allows takes about
35 KiB of Solidcraft's 256 KiB metadata, and a 10,000-via, 2,000-track board reopens with
every pad, track and via found again.

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
- The 10,000-via benchmark board builds in 2.8 s and the 3D view then shows it in a frame,
  but at the layout's 2,000 tracks the board builds in 8.5 s, about 3 ms a track (Build
  cost): the 3 s target is not met with that many tracks. Making tracks as cheap as vias
  needs their extrusions evaluated together (on all cores, or several stadia of a layer in
  one extrusion with each body's track identity kept), and fewer commands per track (its
  own component and joint).
- Tiling the laminate, so that a via edit remakes one tile per slab instead of all five
  slabs, is not done (a via moved on the 10,000-via board takes 2.0 s).
- Automation builds a large board slowly: every `project.apply` reply carries the whole
  document and every board check, so 10,000 single `layout.add_via` requests over
  `pcbcraft-cli stdio` took 230 s and wrote 14.9 GB; there is no batched via add.
- Look: THT and gull-wing solder joints are not modelled. Copper foils run along the
  whole edge (no pull-back); prepreg and core look alike at the edge; the weave's scale and
  its moiré at overview distance are in Solidcraft's shader. Seen from below, the key
  light still leaves a soft hotspot. Some showcase designators sit under or beside bodies
  (C3, R1). Silkscreen placement is not checked.
- There are no zones or pours.
- Blind and buried vias are not supported.
- The via checks use estimated defaults, not configurable rules.
