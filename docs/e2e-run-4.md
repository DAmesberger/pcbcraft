# Fourth end-to-end run: USB-C LED board, parametric outline and 3D

On 13 November 2026 the lead agent designed a small USB-C powered LED board in Sparksmith's desktop app, working as a designer would. The aim was to check the whole flow after the board-plan architecture (option 3) and the 3D realism work landed: parts, schematic, a parametric board, routing, a pour, 3D, the production check, and save and reopen. It also opened the ESP32-S31 example board for 3D quality and speed.

**Setup.** The build was 201fb046 (`main`), built with the `ci` profile (opt-level 1 for workspace crates, 2 for dependencies) and installed as `~/Applications/SparksmithE2E.app` in the Herdr macOS VM: a 1600 × 1000 display at 1×, with a 1558 × 889 app window. It was driven through the guest MCP tools with foreground clicks, because background clicks don't reach egui popups. Speed figures below come from that build; a release build was not measured.

**Limits.** No product code was changed. The JLCPCB 2-layer profile was copied into the guest's `fab-profiles/` folder by hand (finding 1). Export was checked with `sparksmith stdio --operator` and `sparksmith export` on the host, against the saved file.

## Outcome

| | Result |
|---|---|
| Time | About 40 minutes from New to the reopen check, including diagnosis. |
| Board | 34 × 24 mm, 2 layers, JLCPCB 2-layer 1 oz profile; 4 M3 holes held `inset` (3 mm) from their edges; J1 held 4 mm from the left edge, J2 3.5 mm from the right. |
| Parts | J1 USB-C receptacle (template), R1 1 kΩ, D1 LED 0603, C1 10 µF, R2/R3 5.1 kΩ CC pull-downs, J2 1×2 header. |
| Schematic | 5 nets, **0 ERC errors**, 7 "No Part chosen" warnings. |
| Routing | 37 tracks, 1 via, a top GND pour with thermal reliefs. 3 connections left: the second VBUS pad and the D+/D− pad pairs on J1 (0.5 mm pitch; they need bottom-side jumpers). |
| Production check | 9 errors, 10 warnings at first; 7 errors after waiving 2. 6 of the errors are J1 template geometry (finding 3); the rest are the unrouted J1 pads. Export stays blocked, correctly, in the UI and through the CLI. |
| Save and reopen | Identical: board size, parameters, 37 tracks, the via, the pour, both waivers and the issue counts (4 errors, 2 warnings, 3 unrouted). About 4 s from launch to drawn board. |

## What worked well

- **Parametric board.** Changing `width` from 30 to 34 mm moved the right-hand holes (each bound to `inset`), carried J2 along the right edge and kept J1 on the left, all live. "Dimension to edge" on a part's card pins it to the nearest edge; typing a parameter name into a dimension binds it.
- **Parts from search.** "1k 0603", "10u 0603", "5.1k 0603", "led 0603" and "header 1x2" each create a generic part on demand.
- **Schematic.** Wires with automatic junctions, GND ports with stubs, and ERC that reaches zero errors without workarounds.
- **Update PCB review.** It shows the impact, then stages parts beside the board.
- **Fab profile preview.** It cites the source and lists what changes before it is applied.
- **Routing feedback.** "Route stopped short: Blocked by R2 pad 1 (GND). The track ends there…" never shorts nets and says how to undo. The track card shows length, ends and coordinates.
- **Pours.** Thermal reliefs on every GND pad, and an automatic refill when D1 was rotated.
- **3D.** Opens instantly on both boards; zoom is smooth. ENIG pads, satin mask with tracks visible beneath it, a metallic USB-C shell, legible silkscreen, and crisp module marking with a QR code. Selection is shared between 2D and 3D.
- **Native Save panel.** The file name can now be set through accessibility (`set_value`), which earlier runs could not do.

## Findings

### Bugs

1. **New projects have no fab profile, and Sparksmith ships none.** Edit shape refuses to start ("Choose a profile…" first), and the Route card shows Via drill and Via pad as "0 mm" errors ("Enter 0.1 to 6 mm"). A new user can't draw an outline without importing a profile file first.
2. **The Route card's via size stays at 0 mm after a profile is applied.** With the JLCPCB profile set, both fields still showed the error. The via the router placed was correct (0.3 mm drill, 0.45 mm pad), so only the card's seed value is wrong.
3. **The USB-C template fails JLCPCB rules as placed.** Pads 17 and 18 have holes 0.175 mm from pads A1B12 and B1A12, and the four shield pads have a 0.2 mm annular ring: 6 errors on a board that uses only templates.
4. **Not reproduced: the production panel closed during a waive.** With a track selected, the Production files card gave way to the track's card after a second group was ticked. An egui-frame test couldn't reproduce it. The likely cause is that the waive form didn't fit in the ~100 px findings list, so the run's next click landed on the footer's Cancel, which sits where the form's button was expected. See finding 15.
5. **Resizing the board strands staged parts.** After the board went from the default 60 mm to 30 mm, the parts staged by Update PCB stayed beside the old outline, far off the board and partly behind the side panel. Fit view frames only the board, so they were out of sight.
6. **The schematic canvas is clipped along the Browser column.** Zoomed in, J1's body was not drawn left of the Browser card's right edge, even below the card, while its pin circles still showed.
7. **The project name stays "Untitled" after Save As.** The file was saved as `usb-led-e2e.pcbcraft`, but the Board card's Project field (which names the production files) still read "Untitled" after reopening.
8. **The startup recovery dialog shows two identical "Unreadable recovery" entries** with a raw serde message underneath.
9. **A junction dot was left behind.** After C1 was moved, the dot stayed at the old T and the new T showed none. ERC and the net count were unaffected.

### UX

10. The schematic opens zoomed into the sheet's corner, so the first part lands on the border.
11. J1's value text under the symbol is the whole five-line template description.
12. Pin names such as "A4B9" are cut off and run into the pin circles.
13. Nets without a label are named N2…N11 in the PCB's fill and route pickers, with no hint such as the pins they join. Finding CC1 and CC2 took two failed routes.
14. "R" doesn't rotate a part in the PCB view; ⌘R does in the schematic, and the card's Rotate 90° works.
15. The production panel's findings list is about 100 px tall and truncates group names; the rules table's value column clips ("0.1 ı").
16. Toasts stack up to two or three deep during routing.
17. Mounting holes disappear from the 2D view at low zoom.

### 3D realism

18. The orange enclosure envelope box and the hole circles are drawn over the realistic view by default, and the Layers popover has no switch for them.
19. The ESP32-S31-WROOM-3's shield can renders as matte off-white plastic rather than a metal can.
20. No solder fillets, and weak contact shadows under parts.
21. R2's designator is printed under the part body, so only the "2" shows in 3D.
22. Shift-drag with a mouse didn't orbit in this test; only the trackpad hint is shown.

### Speed

| Operation | Board | Time |
|---|---|---|
| Any UI edit, route step, refill | both | instant to the eye |
| Open in 3D | both | instant |
| Launch and reopen | USB LED | about 4 s |
| `project.export_fabrication` (19 files) | ESP32 | 0.14 s |
| `sparksmith export --tier full` | USB LED | 2.9 s (build 1.5 s, check 0.5 s, save 0.9 s) |
| `sparksmith export --tier full` | ESP32 | 25.9 s (build 14.0 s, check 4.4 s, save 7.3 s) |

The native export is the one slow path. These are `ci`-profile timings.

## Clean-up

The app was quit; no Sparksmith process remains in the guest. The test projects were deleted from the guest home and from `results/`. The JLCPCB 2-layer profile stays in the guest's `fab-profiles/`. The guest display was left at 1600 × 1000.
