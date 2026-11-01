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

- Inner-layer copper sits at its stackup heights. It is cut out of the laminate once per
  layer, so the saved bodies stay disjoint.
- Through vias are saved copper (`BodyKind::ViaCopper`): a plated barrel with an annular
  ring on every copper layer. Each via is a component `via_<id>` with parameters
  `via_<id>_x` and `via_<id>_y`.
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
--nocapture`, one machine, release-like `ci` profile):

| Board | Native bodies | Drawn | Build p50 |
| --- | --- | --- | --- |
| Demo | 13 | 30 | 117 ms |
| Showcase (4 layers, 11 vias) | 240 | 459 | 7.0 s |
| Demo on 4 layers, 50 vias, 25 inner tracks | 288 | 484 | 30.3 s |
| Demo, 400 tracks | 413 | 830 | 13.5 s |

Each via is its own native component, and the laminate is cut once per layer around every
drill, so vias cost far more than tracks. 200 vias on 4 layers fail to build: Solidcraft
refuses the laminate cut ("planar tool comparison budget exceeded"). Validation allows
4,096 vias, so via counts above about 50 are an unmeasured target, not a supported size.

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
- Builds slow down steeply with vias, and 200 vias on 4 layers fail (see Build cost).
- Look: THT and gull-wing solder joints are not modelled. Copper foils run along the
  whole edge (no pull-back); prepreg and core look alike at the edge; the weave's scale and
  its moiré at overview distance are in Solidcraft's shader. Seen from below, the key
  light still leaves a soft hotspot. Some showcase designators sit under or beside bodies
  (C3, R1). Silkscreen placement is not checked.
- There are no zones or pours.
- Blind and buried vias are not supported.
- The via checks use estimated defaults, not configurable rules.
