# PCB realism contract: stackup, vias, silkscreen, 3D layer toggles and look

This is the shared contract for the PCB realism slice. It records the types and interfaces
that four parallel tasks build on, where they live and which files each task owns. The
founder's request was: "the PCB realism needs more work. it does not yet look good. also
layers should be visible. and we also need via examples". Visible layers means all three
of: the stackup shown at the board edge, silkscreen and copper visible under the mask, and
per-layer visibility toggles in 3D.

The rules still hold. Copper is real geometry. The saved `.solidcraft` model stays
physical. Display-only layers are never saved. The format is pre-release: types are
replaced and there are no legacy readers. One typed engine serves the UI, CLI and MCP, and
the core stays pure and wasm-compatible.

## Types in the contract

### Core (`pcbcraft-core`)

| Type / item | Where | Contract body |
| --- | --- | --- |
| `CopperLayer { Top, Inner(u8), Bottom }` | `crates/editor-core/src/model/stackup.rs` | Complete. A named copper layer whose identity is its name. It serialises as `"Top"`, `{"Inner": n}` and `"Bottom"`. It provides `side()`, `name()`, `From<Layer>` and `PartialEq<Layer>`. |
| `Layer { Top, Bottom }` | `model.rs` | Unchanged meaning: the **board side** (placement side, footprint drawing side, mask or silkscreen side). It now also derives `Ord` and `Hash`. |
| `Track.layer: CopperLayer` | `model.rs` | Changed type. Inner layers are valid when the stackup has them. `layout::validate` rejects a track on a layer the stackup lacks. |
| `RouteRequest.layer: CopperLayer` | `layout.rs` | Changed type. |
| `WorldPad::on(impl Into<CopperLayer>)` | `layout.rs` | Accepts a side or a copper layer. A pad's `layer` is still its side. |
| `Stackup`, `CopperFoil`, `Dielectric`, `DielectricKind`, `GlassStyle`, `OUNCE_UM`, `COPPER_LAYER_COUNTS` | `model/stackup.rs` | Complete. `Stackup::default()` is 2 layers of 35 µm (1 oz) copper on a 1.6 mm 7628 core, with mask and silkscreen on both sides. `preset(2/4/6)`, `layers()`, `has()`, `index()`, `dielectric_mm()`, `thickness_mm()`, `copper_span(layer)`, `dielectric_span(i)` and `check()` are provided. Heights use z = 0 at the bottom face of the lowest dielectric. Bottom copper is below 0 and top copper is above `dielectric_mm()`. Inner foils sit at the top of the dielectric below them. This matches today's native model: substrate 0..1.6, top copper 1.6..1.635. |
| `Project.stackup: Stackup`, `Project.vias: Vec<Via>` | `model.rs` | New required fields. `Project::via(id)` is provided. |
| `Via`, `ViaSpan { Through, Between { from, to } }` | `model/via.rs` | Data is complete. A via has an id, net, position, drill, diameter, span, tented flag and locked flag. It provides `on(layer, layers)` and `annular_ring()`. `Between` reserves blind and buried vias; validation refuses it as `Unsupported`. |
| `layout::via::{ViaRequest, LayerChangeRequest, MAX_VIAS, DEFAULT_VIA_*}` | `crates/editor-core/src/layout/via.rs` | `ViaRequest::through(net, at)` creates a 0.3/0.6 mm tented via. `validate` checks the count bound, identity, net, drill < pad, the pad inside the board and through vias only. |
| Commands `layout.set_stackup`, `layout.add_via`, `layout.via_move`, `layout.via_delete`, `layout.via_tent`, `layout.via_lock` | `LayoutCommand` in `layout.rs` | Working. Each command is atomic and runs through `apply_staged` and `validate`. |
| Command `layout.route_layer_change { request: LayerChangeRequest }` | `layout.rs` | Returns `Unsupported` for now. It will route to `request.end`, place `via` there and route on `to_layer` to `end`. |
| `Outcome::ViaAdded { via }` | `commands/mod.rs` | Returned by `layout.add_via`. `layout::apply_staged` now returns `Result<Option<Id>>` (the created via). |
| `Engine::showcase()` | `lib.rs` | Returns `Unsupported` for now. The real body belongs in `library/samples.rs`. |
| CLI `showcase NEW_FILE.pcbcraft` | `apps/pcbcraft-cli/src/main.rs` | Wired to `Engine::showcase()`. |
| Validation | `validate.rs` | Via ids share the global identity space with other objects. |

### Integration (`pcbcraft-solidcraft`)

| Type / item | Where | Contract body |
| --- | --- | --- |
| `BodyKind::{ViaCopper, Silkscreen, EdgeDielectric, EdgeCopper}` | `material.rs` | `ViaCopper` is saved copper with the copper look. `Silkscreen`, `EdgeDielectric` and `EdgeCopper` are appearance kinds (`is_appearance`). Silkscreen is matte epoxy ink in the board's `silkscreen_colour`; the ink values are estimates. `EdgeDielectric` uses the FR-4 look and has no weave detail yet. |
| `Geometry.layer: Option<CopperLayer>` | `scene.rs` | The copper layer a body lies on, or the side, as its outer copper layer, that a mask or silkscreen film covers. It is `None` for board-spanning bodies and component bodies. `project::finish_with` fills it from the body's `Source`. Appearance `push` sets `None` for now. |
| `Source.layer: Option<CopperLayer>`, `Source.via: Option<Id>` | `project.rs` | Changed type plus a new field. |
| `silkscreen::films(project, board, lands) -> Result<Vec<Geometry>>`, `INK_THICKNESS_MM` | `silkscreen.rs` | Returns no films for now. |
| `stackup_edge::bands(stackup, board) -> Result<Vec<Geometry>>`, `EDGE_PROUD_MM` | `stackup_edge.rs` | Returns no bands for now. |
| `project::build` | `project.rs` | Refuses, as `Invalid` with an explicit message, a non-default stackup, any via, and inner-layer tracks. That stays until the native task builds them. |

### App (`pcbcraft`)

| Type / item | Where | Contract body |
| --- | --- | --- |
| `ViewLayer { Bodies, Silkscreen(Layer), Mask(Layer), Copper(CopperLayer), Substrate, Vias }`, `ViewLayer::of(kind, layer)` | `apps/pcbcraft/src/layer_view.rs` | Complete classification of every `BodyKind`. |
| `LayerView` (`visible`, `set`, `shows(&Geometry)`, `substrate_hidden(bool)`) | `layer_view.rs` | Complete. 3D picking already filters through it, reproducing today's Hide-substrate behaviour. |

`pcb_editor.rs` changed in five lines only (`CopperLayer` conversions). PCB 2D still draws
outer-layer tracks only.

## Parallel tasks and the files each owns

Shared files take additive edits only. A concurrent sidebar redesign is restructuring
`pcb_editor.rs` cards, the `app/chrome.rs` column and status, the kit, `part_editor/*_card.rs`,
the schematic cards, `update_preview.rs`, core issues/messages and update-plan data, and
`issues.rs`.

### (1) Core: stackup, vias, routing, checks and the showcase

Owns `crates/editor-core/src/{model.rs, model/*, layout.rs, layout/*}`, any new
`stackup*` or `via*` modules, `library/samples.rs` (showcase), `lib.rs` (`Engine::showcase`)
and `apps/pcbcraft-cli`.

- `layout.set_stackup` refuses a stackup that drops a layer still in use; tracks and vias are already checked against the stackup.
- `layout.route_layer_change` routes, places a via and routes on as one transaction.
- Net connectivity through vias: `BoardCopper` joins copper of the via's net on every layer the via spans.
- Checks: annular ring, drill-to-copper clearance, via-to-via clearance and via clearance to other-net copper.
- Update and impact data for vias and the stackup.
- CLI and MCP schemas regenerate from the types.
- `Engine::showcase()` and CLI `showcase`: a 4-layer board with an SOIC-8, a QFN-16, a 2x5 header, 0603 and 0805 passives and a polarised part. It is routed on top and bottom through tented and open through vias, with silkscreen designators and outlines from the footprints. It uses a GND pour if zones exist, otherwise wide traces.

### (2) Native: real copper and display layers

Owns `crates/solidcraft-integration/src/{project.rs, project/*, appearance.rs, appearance/*,
material.rs, silkscreen*.rs, stackup_edge*.rs}` and `apps/pcbcraft/src/model_scene.rs`
where needed.

- Inner-layer copper sits at `Stackup::copper_span` heights. `BoardJoint` takes a z or layer instead of a side.
- Via barrels have plating thickness and annular rings on every connected layer, as `BodyKind::ViaCopper` with `Source.via`. They are real saved copper.
- Board thickness and copper thickness come from the stackup. Remove the `build` refusals.
- Display-only layers:
  - A tented via shows as a domed mask bump. An open via shows a finished ring and a dark drill.
  - Board-edge stackup bands per the stackup. `EdgeDielectric` gets the weave detail; extend the detail test's list rather than weakening it.
  - Silkscreen films (G5) from `layout::placed_graphics`, clipped at mask openings, top and bottom.
- Set `Geometry.layer` on every appearance body: mask caps, finish, silkscreen and edge copper.

### (3) Look: models, lighting and pictures

Owns `crates/solidcraft-integration/src/models/*.rs` (profile and builders),
`apps/pcbcraft/src/lighting.rs`, `apps/pcbcraft/src/viewport/pictures.rs`, and `material.rs`
and `docs/materials.md` for justified tweaks only (coordinate with task 2 in `material.rs`).

- Rounded profiles (lines plus arcs) for chip terminations and bodies, moulded bodies, QFN bodies and header bodies. Add a `rounded_rect(centre, width, height, radius) -> Result<Contour>` helper in `models/profile.rs`, built like `bent_strip`.
- Keep bodies disjoint and the lead-point test passing.
- Round or chamfer the board edge if that is cheap.
- Tune the overview: studio turn, key and fill, exposure, without the old smear.
- Add a showcase picture set: overview; SOIC, QFN, header and passive close-ups; tented and open vias; the edge stackup; the bottom side. Write it to `/private/tmp/claude-501/-Users-amd-dev-pcbcraft/17b68a65-4f80-4764-aec6-ceb135ce7274/scratchpad/look/v3/`.

### (4) Toggles: per-layer visibility in 3D

Owns `apps/pcbcraft/src/viewport.rs`, `viewport/*` except `pictures.rs`, `layer_view.rs`,
`model_scene.rs` segments and draw lists, and minimal hooks in the `app/chrome.rs` navigation
bar and `pcb_editor.rs`.

- Replace `Viewport::hide_substrate` with a `LayerView`.
- Changes only filter draw lists, slots, shadows, highlight and picking. Geometry is never rebuilt.
- A compact layer popover on the floating navigation bar replaces the substrate toggle.
- PCB 2D's copper, silkscreen and mask toggles share the same state where they overlap.

## Merge order

Each task builds on this commit. Expected conflicts are small:

- (2) and (4) both touch `model_scene.rs`. (2) adds kinds and layers; (4) changes segmentation.
- (2) and (3) both touch `material.rs`.
- (1) and (2) meet at the stackup and via data; read only the contract types above.

## Contract changes made by the native task (2)

Additive except where noted; every caller is inside `pcbcraft-solidcraft` unless named.

- `BodyKind::ViaBore` (new, appearance): the dark sleeve lining an open via's bore. `ViewLayer::of` classifies it with `ViaCopper` as `Vias` (one line in `layer_view.rs`).
- `BodyKind::EdgeDielectric` now has the weave detail (strength per glass style from `stackup_edge::weave_strength`); `BodyKind::Silkscreen` has the silkscreen grain.
- `native::BoardJoint.layer: Layer` is replaced by `mount: Mount`, with `Mount::Side(Layer)` (`From<Layer>`) and `Mount::Height(Expr)` (upright at a board height).
- `appearance::Board` replaces `copper` with `copper_top`/`copper_bottom` and adds `mask_top`/`mask_bottom` and `Board::of(project)`; `appearance::board` takes the vias (`ViaMark`) too.
- `silkscreen::films(project, &Legend { board, lands, paths, vias, picks })` replaces `films(project, board, lands)`: films climb track caps and tents and share their part's pick.
- `appearance::union(loops)` (new): the outline of a union of loops, used for the inner-copper laminate cuts.
- Native parameters: `dielectric_N_thickness` (multilayer), `board_thickness` (their sum), `copper_thickness` (top), `copper_bottom_thickness`, `copper_inner_N_thickness`; vias are components `via_<id>` with `via_<id>_x`/`_y`.
- Edge mask lips are `MaskCap` bodies without a pick identity, standing outside the routed outline.
