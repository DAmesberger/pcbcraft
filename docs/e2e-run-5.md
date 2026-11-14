# Fifth end-to-end run: LDO board on the shipped library

On 14 November 2026 the lead agent designed a small LDO power board in Sparksmith's desktop app, working as a designer would. The aim was to check the whole flow after the night's changes:
- every rule, profile, material, standard, assistant and part-file value moved out of the code into the shipped `library/` folder;
- generated parts (GP1);
- the fixes from run 4.

The board: a USB-C receptacle (J1), an LDO in SOT-23-5 (U1), two 1 µF capacitors, an LED with its 1 kΩ resistor, two 5.1 kΩ CC pull-downs and a 1×4 header.

**Setup.** The build was `main` at d3ee4123, built with the `ci` profile and run as `Sparksmith.app` in the Herdr macOS VM, with `library/` copied into `Contents/Resources/library` (688 KB). Full CI on that commit: 3,151 passed, 0 failed, 58 ignored. Export was checked with `sparksmith stdio --operator` on the host, against the saved file.

## Outcome

| | Result |
|---|---|
| Time | About 25 minutes from New to the reopen check. |
| Start-up | The app found the shipped library in its bundle and started normally. A new project's fab profile prompt preselected JLCPCB 2-layer 1 oz from `library/defaults/new-project.json`. |
| Schematic | 9 parts, 11 nets, **0 errors**, 9 "No Part chosen" notes. Power ports, net labels and wires. |
| Board | 30 × 22 mm (from 60 × 40 by its parameters), every part placed on the board. |
| Routing | 3V3, VBUS and the LED net routed with the shove router; GND as a top pour with thermal reliefs. Left: J1's fine-pitch pads, as in run 4. |
| 3D | Opens instantly. Every look comes from the shipped library: green mask over the pour, ENIG, a metal USB-C shell, gold header pins, SOT-23 feet with solder fillets, designators clear of the bodies. No interface overlays by default. The hint reads "Right-drag to orbit · middle-drag to pan · wheel to zoom" after mouse use. |
| Save and reopen | Identical in the desktop and through the CLI (1.5 s): 9 placements, 37 tracks, the pour, the profile and the same findings. |
| Production check | 12 errors: J1 template geometry (6, kept by design, see run 4 finding 3), unrouted J1 pads and one dangling stub. Export is blocked, correctly. |

## What worked well

- **Start-up from data.** Profiles, stackup, materials, standards and part files load from the bundle with no visible difference from the build that had them compiled in.
- **The fab profile** is applied before any drawing, so Edit shape and the Route card's via size (0.3 / 0.45 mm) are ready at once.
- **Power ports** attach to a pin in one click; typing a name makes another net (VBUS, 3V3, GND).
- **The shove router** took VBUS around the 3V3 track without help. A blocked GND route stopped short with a clear message and shorted nothing.
- **Net pickers** name unnamed nets by their pins ("N20 · D1 A, R1 2").
- **One toast at a time** throughout placement and routing.

## Findings

1. **Search finds nothing for "ldo sot-23-5"; "sot-23-5" alone works.** A word the search grammar doesn't know (here "ldo") makes the whole query fail, instead of matching what it can and saying which word it ignored.
2. **Provenance text shows an internal file name.** The USB-C part reads "[built-in starter table usb_c.json format 1]" in its card.
3. **Board parameter fields need a double-click to edit.** A single or triple click on `width` in the Board card does nothing visible; other number fields take a click.
4. **Saved projects are readable by their owner only (mode 0600).** The atomic save keeps the temporary file's permissions, so a project saved on a shared drive or in a repository can't be read by other users.
5. **Not reproduced: power port labels crowding.** At low zoom the VBUS, GND and VBUS ports on pins 1–3 of U1 looked stacked. Measured in core geometry with the same sequence, the closest pair keeps 0.40 mm, the minimum label spacing, and none touch, across rotations, mirrors and port styles.
6. **The 2D board shows the "J1 opening" interface datum by default**, with lines reaching past the board edge. 3D hides the board interface unless asked (run 4 finding 18); 2D should match.

## Speed

| Operation | Time |
|---|---|
| Any edit, route step, refill | instant to the eye |
| Open in 3D | instant |
| CLI open of the saved project | 1.5 s |
| Reopen in the desktop app | about 4 s from launch to drawn board |

## Clean-up

The app was quit and the test files deleted from the guest and the shared folder. The guest display stays at 1600 × 1000.
