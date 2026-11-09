# Launch video storyboard

A 2-minute usage video (target 126 s; acceptable 90–150 s) for the README and the launch post. One
continuous story on one real board: **datasheet → AI-drafted component with a photoreal 3D model →
schematic → PCB update → push-and-shove routing and a differential pair → fills → 3D fly-around →
Gerbers and pre-flight → an MCP agent doing a step**.

Every shot is recorded with [`tools/launch`](../../tools/launch/README.md): the app's own window
only, at 60 fps and backing resolution (2 px per point), driven
by a shot script so takes repeat exactly. Captions, the cursor halo and push-ins are added in post
from the same script. No voice-over is needed; captions carry the story.

## Look and conventions

- **Window:** a fixed window size (the proof takes: 1500 × 1235 pt, set by the machine's window
  tiling), floated on a dark 16:9 backdrop in post (`post.py --stage 1920x1080`); dark theme (the
  app follows the system appearance: record with macOS in Dark mode); window-only capture, so no
  other app can appear in the frame.
- **Board:** the ESP32-S31 dev board being designed now (see *Dependencies*). Until it lands,
  the 4-layer showcase (`pcbcraft-cli showcase`) stands in.
- **Captions:** IBM Plex Sans (SIL OFL, the app's own UI font), white on a 72 % dark box, lower
  centre, at most 7 words, 2.5–3.5 s each, 0.35 s fades. Feature claims in captions must match the
  README draft word for word where they quote numbers.
- **Pace:** pointer glides use an ease-in-out curve (0.5–0.9 s per move); hold 0.6 s after each
  visible result before the next action; one push-in (1.6–2.2×) per shot at most.
- **Music:** none, or a CC0 / royalty-free track with its licence and attribution recorded in
  `docs/launch/media/CREDITS.md`. No third-party logos or vendor artwork on screen; datasheets are
  shown by file name only, never as rendered Espressif pages.
- **Cuts:** 0.5 s crossfades between shots (`post.py reel`); hard cut into the end card.

## Shots

| # | Scene | What happens on screen | Duration | Caption | Feature it proves | Ready? |
|---|---|---|---|---|---|---|
| 0 | Cold open | Black, then the finished dev board in 3D, slowly orbiting under the studio light; the title fades in. | 4 s | **Pcbcraft** · *one design, components to Gerbers* | Integrated photoreal 3D | Waits: dev board; better with Realistic mode |
| 1 | Datasheet in | Component workspace start page → **Create from datasheet…**; drop the two WROOM-3 PDFs (file names only), part name, model; Start. Progress ticks up. | 8 s | *Give it the datasheets.* | AI-assisted component creation (`pcbcraft-assist`) | Works today; see note A |
| 2 | Review | Time-cut to the finished run: the review report lists values with page, table and confidence; the Pins tab shows 99 pins with their IO MUX functions. | 6 s | *Every value cited to its page.* | Review flow; pin functions | Works today |
| 3 | The part in 3D | Component 3D tab: the WROOM-3 part file with the laser-etched name on the can, the QR code and pin-1 mark; a slow orbit and a push-in onto the etching. | 10 s | *A parametric Solidcraft part, with real markings.* | Lean parts; markings D7 | Works today; Realistic mode and material graphs will lift it |
| 4 | Publish | Publish → the identity bar turns to *r1 · published*. | 3 s | *Published as an exact revision.* | Revisioned library | Works today |
| 5 | Schematic | Schematic: **P**, type `esp32`, Enter; then `10k 0603`, Shift+Enter ×2; wire EN and IO0; a net label `USB_D+`; open Pin assignment… and tick USB — the solver stages the pins. | 12 s | *Search with units. Assign pins with a solver.* | Place palette, schematic capture, pin mux | Works today on the parts catalog; the dev board's sheets wait on the esp32-board work |
| 6 | Update PCB | PCB workspace: Update PCB review shows the planned placements as ghosts; Apply; airwires appear. | 8 s | *Schematic to board, reviewed first.* | Explicit, previewed update; one undo step | Works today |
| 7 | Push and shove | Route (**R**) from a pad straight through a bundle: neighbouring tracks and a via slide aside live; **M** flips to Walkaround and back; click commits; ⌘Z shows one undo step. | 14 s | *Push and shove. Each click: one undo step.* | Push-and-shove router; atomic transactions | **Proof take recorded** on the showcase |
| 8 | Diff pair and tuning | Route a USB pair from the connector: both halves coupled at the gap, the Route card reads 90 Ω target; then **T** adds a meander to match lengths. | 10 s | *Differential pairs from the stackup’s impedance.* | High-speed routing | Works today; best on the dev board's USB pair |
| 9 | Fills | Fill (**P**): draw a GND outline over the bottom; Shift+P refills; thermals and clearances appear around pads. | 6 s | *Fills with thermals, computed off the UI thread.* | Copper fills | Works today |
| 10 | 3D fly-around | **D** to 3D; a smooth orbit with arrow keys, push-in to a chip's solder fillets and ENIG lands, a tented via; Board setup → soldermask black, matte; Layers popover strips mask, then copper layers one by one. | 16 s | *Real copper, real thickness. Same selection.* | Thickness-aware copper; product look; shared selection | **Proof take recorded** on the showcase; Realistic mode pending |
| 11 | Production | Output › Production: pre-flight report; one warning waived with a reason; Export as ZIP; the written files list (Gerber X2, Excellon, job, BOM, pick-and-place, drawings). | 10 s | *Gerbers, drills, BOM and drawings — checked first.* | Fabrication output and pre-flight | Works today |
| 12 | Agent step | Side-by-side: a terminal running Claude Code with the `pcbcraft` MCP server; prompt *"run the pre-flight and route the last unrouted net"*; the tool calls (`project_query fabrication.preflight`, `project_apply layout.route_step`, `project_save`) scroll; cut to the app reopening the file with the new track highlighted. | 14 s | *Your agent uses the same engine. And the same undo.* | One command engine across UI, CLI and MCP | Works today with reopen; see note B |
| 13 | End card | The board in 3D, frozen; text. | 5 s | **Pcbcraft** · MIT OR Apache-2.0 · *repository URL* | — | Template only |

Total: 126 s.

### Notes

- **A — the datasheet run on camera.** A live run takes minutes and costs about 5–7 USD. Record it
  once and time-cut (shot 1 → 2). For repeatable takes the desktop dialog would need the CLI's
  `--replay` option (listed in the [polish list](polish-list.md)); until then shot 1 is a one-off
  live recording or the CLI with `--replay examples/esp32-s31-wroom-3/wroom3.replay.jsonl`.
- **B — the agent shot.** The MCP server runs a headless session on a file; the desktop does not
  follow it live. The shot therefore ends on a reopen. A live link (the desktop serving or
  following the session) would make this the strongest shot; it is a polish-list item, not a
  current feature, and the caption must not imply it. The terminal is recorded as its own window
  (`launchkit record` on the terminal's pid), never the whole screen.
- **Numbers on screen.** Only measured ones: 11.6 ms worst shove move on the 10,000-via board,
  0 mm² Gerber read-back difference, 0.13 s showcase export.

## Dependencies: record the final take after these land

| Work in progress (agent) | Shots waiting |
|---|---|
| ESP32-S31 dev board (`esp32-board`) | 0, 5, 6, 7, 8, 9, 10, 11, 13 — the story's board |
| Realistic view mode and material graphs (`material-graph-design`, `mg-*`) | 0, 3, 10, 13 |
| Mechanical parts (`mech-authoring`): USB connector, enclosure-relevant parts | 0, 8, 10 |
| Part placement (`part-placement`) and layer focus (`layer-focus`) | 6 (placement flow), 10 (layer peel) |
| Live desktop ↔ MCP session (not started) | 12 (optional upgrade) |
| Datasheet replay in the desktop dialog (not started) | 1 (repeatable take) |

## Shot scripts

Shot scripts live in `tools/launch/shots/`. The proof take implements:

- `shots/route-shove.shot` — shot 7 on the showcase board;
- `shots/board-3d.shot` — shot 10 (orbit, push-in) on the showcase board;
- `shots/component-3d.shot` — shot 3 on the WROOM-3 example.

The proof media (not committed) are `route-shove-1080p.mp4`, `board-3d-1080p.mp4`,
`component-3d-1080p.mp4`, `proof-reel.mp4` and `hero.webp`/`hero.gif` in the session scratchpad's
`launch/` folder; the critique is in the [polish list](polish-list.md).

Coordinates in scripts are window points for the default window size. When the UI layout changes,
re-measure them from a still (`launchkit window PID` gives the frame; the proof stills show the
targets).
