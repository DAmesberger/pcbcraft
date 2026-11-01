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
44 × 28 mm 4-layer board with:

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
  layer toggles use.

## Look

- Generated package models have named-radius native fillets:
  - moulded body corners and top edges;
  - QFN top edges;
  - the header base;
  - chip body edges and termination ends.

  These are fillets rather than rounded sketch profiles for two reasons. A profile drawn
  from numbers would lose the body's parameters. The engine also refuses to intersect two
  rounded profile extrusions.
- The studio turn is 0° and the exposure 1.1, so the overview shows a lacquer sheen on the
  mask instead of a flat board. A GPU test measures the sheen and checks that nothing clips
  to white.
- `write_showcase_pictures` writes a picture set of the showcase board:
  - the overview;
  - close-ups of the SOIC, QFN, header and passives;
  - the vias;
  - the edge stackup;
  - the bottom side.

  `write_layer_pictures` writes the layer toggles. Both are ignored tests that write to
  `PCBCRAFT_PICTURES`.

## 3D layer toggles (`apps/pcbcraft`)

- `ViewLayer` lists the toggles: components, silkscreen and mask for each side, copper for
  each stackup layer, substrate and vias. `LayerView` holds the visibility of each one.
- A Layers popover (eye icon) on the floating navigation bar holds the toggles. It lists
  the layers in board order and has Show all. The icon is outlined in the accent colour
  while any layer is hidden. PCB 2D keeps its own Layers and view popover.
- The toggles are display state only:
  - The model's PBR buffer has one run of bytes for each view layer.
  - Toggling repacks the shown bodies from the cached meshes, off the UI thread.
  - Shadows, highlight and picking follow at once.
  - No geometry is rebuilt.
- PCB 2D and 3D share one state for outer copper and silkscreen. Component 3D always shows
  every layer.

## Open

- PCB 2D draws outer-layer tracks only. It draws no board vias and no inner-layer tracks.
  It has no via tool and no stackup editor; vias and stackups are edited through commands,
  the CLI or MCP.
- There are no zones or pours.
- Blind and buried vias are not supported.
- The via checks use estimated defaults, not configurable rules.
