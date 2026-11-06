# Second end-to-end UX run: USB sensor board, one hour

On 6 November 2026 an agent designed the same small USB-powered sensor board as the [first UX run](e2e-ux-run.md) in Pcbcraft's desktop app, the way a designer would: through the ribbon, the cards, the canvas and the shortcuts. The aim was again to judge the UX against the founder's bar: text that is readable, distinguishable and expands on hover when shortened; screens that are not crowded; and a next step that is obvious. This run also checked which of the first run's findings are fixed.

The app ran in the Herdr macOS VM (1920 × 1200 display at 1×, app window 1380 × 900). The build was 6e2c238 (`component-model` branch), installed as `~/Applications/Pcbcraft.app`. The budget was **one hour of design time**. The clock started at the first action in the app, 06:40:35, and the run stopped at 07:25:13, after **44 min 38 s**. The agent read no product source, changed no product code and did not use `pcbcraft-cli`. It made no paid API calls.

Screenshots referenced below (`s…png`) are in the agent's `target/e2e-ux-2/` and were not committed.

## Outcome

**The board reached Update PCB, placement, parametric holes, a GND pour, the production check, 3D, save and reopen, but not export.**

- **No findings were waived.** Export stayed disabled because 17 errors remained, so no production files were written.
- **Schematic: 7 ERC errors remain, down from 13.** All 7 are unused pins: U2 PA08, PA09, PA02, PA04 and RESET, U3 OS, and one other. A designer would mark these no-connect, but **the schematic has no way to mark a pin no-connect** (finding 1). The schematic is otherwise wired and readable: wires, GND and +3V3 power ports, six net-label names and values on every passive (`s15-issue-expanded.png`).
- **Routing was partial.** Six tracks were routed on top copper: VBUS from J1 to U1, the LED string, both pull-ups to U3, and C1 and C2 to U1. A top-layer GND pour then connected most GND pads. **19 airwires remained**, and the production check listed 10 unrouted nets.

The agent stopped at 44:38 rather than at 60 minutes for two reasons:

- The remaining 19 connections would have taken about 10 minutes through the driver, at about 30 s each with verification.
- After that, Export would still have been blocked by the 7 unconnected-pin errors. Only waiving clears them, and waiving them would be waiving to unlock Export.

The decision is recorded here so the run is not read as "ran out of time".

| Step | Reached | Time (real clock) | Notes |
|---|---|---|---|
| 1. New project and parts | Yes | 06:40:35–07:03:19, **22 min 44 s** | ⌘N, then the name was typed into the Board card. Seven components. A resistor, a capacitor and a 1×4 header came from templates (about 35 s each). The LED came from the 0603 chip template; there is still no LED template (`s01-led.png`), and the result draws as a plain diode. An LDO (SOT-23-5), an MCU (SOIC-14) and an LM75B sensor (SOIC-8) came from package templates with pasted pin tables. **No USB-C**: "usb" finds nothing in Open or in New from template, so J1 is again a 1×4 header standing in. The LDO took 4.5 min: its first paste silently dropped two rows (finding 3). The MCU took 10.8 min, about 8 min of it setting a Drive on 10 functions one by one (finding 4); the driver's clicks on table rows sometimes needed repeating, which inflated this time. |
| 2. Schematic | Partly, 7 ERC errors | 07:03:19–07:17:17, **13 min 58 s** | Placement: 12 symbols with the Place palette (P), arranged by function (input and LDO across the top, MCU in the middle, sensor and pull-ups on the right, LED left, programming header bottom). ⌘R turned the passives and Shift+X mirrored the headers. Wiring: about 12 wire runs, plus GND and +3V3 power ports. Labels: SDA, SCL, USB_DM, USB_DP, SWDIO and SWCLK, at both ends of each net. Values: 100 nF, 1 µF, 4.7 kΩ and 1 kΩ typed in the instance card. The searches for a no-connect tool took about 3 min (finding 1). |
| 3. Update PCB | Yes | 07:17:17–07:17:41, 24 s | ⇧⌘U from the schematic did nothing. From the PCB workspace it opened the review, and Apply staged all 12 parts right of the board, in rows by kind, without overlap (`s17-update-pcb.png`). The review listed every pad of the staged parts as a "New error" (finding 7). |
| 4. Board shape and holes | Yes | 07:18:40–07:20:48, **2 min 8 s** | Board kept at 60 × 40. Four mounting holes (4 clicks) and an `inset` parameter added in the card. Eight dimensions, each set to `inset`, took about 70 s. Changing `inset` from 3 to 5 moved all four holes (`s19-holes-inset3.png`, `s20-holes-inset5.png`). |
| 5. Stackup and rules | No | — | Defaults kept. |
| 6. Placement | Yes | 07:17:41–07:18:40, 59 s | Twelve drags. J2 was held to the bottom edge with **Constrain to edge** (one click) and J1 was locked with the **Locked** box (`s18-placed.png`). |
| 7. Routing | Partial | 07:20:48–07:23:00, 2 min 12 s | Six tracks. There is no autorouter or "route all", as expected. Two batches of clicks were lost because Esc after a route returns to Select, so the next pad click selected the part instead of starting a route. |
| 8. Pour | Yes | 07:23:00–07:23:45, 45 s | GND on top: 4 clicks and Enter. The fill had thermal reliefs and pad clearances, and the airwire count fell from 29 to 19 (`s21-pour.png`). |
| 9. DRC and production check | Ran, not passed | 07:23:45–07:24:10, 25 s | 17 errors (10 unrouted nets, 7 unconnected terminals) and 14 warnings (`s22-production.png`). **None waived**: none is a finding a designer would accept. |
| 10. 3D | Yes | 07:24:10–07:24:15, **5 s** | Clean model with headers, SOICs, the SOT-23-5, passives, holes and tracks under the mask (`s24-3d.png`). |
| 11. Export | **No** | — | Export is disabled while errors remain. |
| 12. Save, close, reopen | Yes | 07:24:15–07:25:13, 58 s | ⌘S on the untitled project did nothing visible; File › Save opened the panel, whose name could not be typed through the driver (a **driver limit**), so the file is `~/untitled.pcbcraft`. ⌘Q quit at once, and `open -a … untitled.pcbcraft` reopened the file with everything intact: board, holes, `inset` = 5, the lock, the constraint, the tracks and the pour (`s25-reopened.png`). |

### Where the time went

| Activity | Time | Share |
|---|---|---|
| Parts | 22 min 44 s | 51% |
| Schematic | 13 min 58 s | 31% |
| Update PCB, placement, holes, routing, pour, check, 3D, save and reopen | 7 min 56 s | 18% |
| Waiving | 0 | 0% |

Within parts:

| Part | Time |
|---|---|
| Four template parts | about 2.5 min |
| LDO | 4.5 min |
| MCU | 10.8 min |
| Sensor | 3.7 min |

## Comparison with the first run

| | First run (0f6b200) | This run (6e2c238) |
|---|---|---|
| Duration | 50 min 3 s | 44 min 38 s, stopped by choice |
| Parts | 15 min 46 s | 22 min 44 s |
| Schematic | 8 min 12 s, 13 ERC errors, no values | 13 min 58 s, 7 ERC errors, values on all passives |
| Holes | 4 min 27 s | 2 min 8 s |
| Routing | 1 GND track, 25 airwires | 6 tracks plus GND pour, 19 airwires |
| Production check | 23 errors, all waived (8.5 min) | 17 errors, none waived |
| Export | 17 files | Not reached |

The first run "finished" by waiving every error; this run did not waive. Parts took longer: per-function Drive setting is new work on top of the pin-kind review, and one import silently dropped rows. The schematic took longer but is better: values, more labels, fewer errors. Every remaining error is either unrouted copper or an unused pin that the product gives no way to mark. Board steps are faster across the board.

## Status of the first run's findings

| # | First-run finding | Status in this run |
|---|---|---|
| 1 | No USB-C part or template | **Remains.** "usb" finds nothing in Open or New from template. |
| 2 | Waiving one row at a time | **Seen fixed, not exercised.** The list is grouped by kind, with tick boxes per row and per group, and *Waive…* appears on hover (`s23-hover-row.png`). No waiver was made. |
| 3 | Truncated production rows, stale tooltips | **Fixed for rows.** Hovering "Label SDA on …" showed the full title and fix, and the tooltip closed when the pointer left. The Result line reads in full ("17 errors · 14 warnings"). **Group titles are still cut** with no hover seen: "Unrouted connecti…", "Unconnected termin…" (finding 10). |
| 4 | Value text collides | **Fixed.** Values read "100 nF", "1 µF", "4.7 kΩ", "1 kΩ", small, beside the body, and clear of pins on turned resistors. ICs and headers show their component name ("LDO AP2112K-3.3", "Pin header 1×4 2.54 mm"). Template passives show nothing until a value is typed in the card. |
| 5 | IC box overlaps pin names | **Fixed.** The 14-pin MCU box fits "USB_DP", "RESET" and "SWCLK" cleanly (`s09-combination-hover.png`). |
| 6 | Pin import needs per-row work | **Remains, and is worse in one way.** See findings 3 and 4. |
| 7 | Scratch-symbol pins point inward | Not tested. |
| 8 | Dimensioning eight edges is about 50 actions | **Better: 8 dimensions in about 70 s.** Each is a pick of the hole and the edge, then `inset` in the Value field. The **labels still overlap** at the corners: "inset = !inset = 5" top left and "inset = 5 nset = 5" bottom right (`s20-holes-inset5.png`). No corner-inset pattern. |
| 9 | Every joining wire opens a review | **Fixed.** A toast reads "Connected pins: adds 1 wire, removes 1 net, changes 1 net" with Undo (`s13-wire-through-port.png`). |
| 10 | J gave no visible junction | Not retested with J. T-junction dots now appear on their own where wires meet (J1 pin 1 to U1 and C1, `s15-issue-expanded.png`). |
| 11 | Label and port share a spot | **Mostly fixed.** Labels on adjacent MCU pins (SDA, SCL, SWCLK; USB_DP, USB_DM, SWDIO) do not overlap. |
| 12 | Update PCB overlaps parts; ⇧⌘U does nothing | **Staging fixed:** rows by kind beside the board, no overlap, and edge names centred on their edges. **⇧⌘U from the schematic still does nothing**; it works from the PCB workspace. |
| 13 | Identical truncated issue rows | **Fixed for ERC rows**: "U2 pin PA08 unconnected" leads with the difference. The component **Combination** rows still all start "Function 'PA05' of termina…", and their hover tooltip runs off the window edge, cut off (finding 9). |
| 14 | Empty Component hint cut off | **Fixed.** It wraps (`s01-led.png`). |
| 15 | Off-board DRC rows cut off | **Remains in the Update PCB review**: "New error: U1 pad 1 exte…", "New error: J1 pad 2 exten…" (`s17-update-pcb.png`). |
| 16 | BOM values | Values now exist on the instances (typed by hand). BOM not checked, because export was not reached. Asking for a value at placement is still open. |
| 17 | Drill table T1 duplicate | Not checked (no export). |
| 18 | Pour hides top layer | **Fixed.** The top GND pour leaves pads, designators and tracks readable, with clear clearance outlines (`s21-pour.png`). |
| 19 | ⌘S on untitled does nothing; panel proposes "untitled" | **Remains.** ⌘S showed nothing. File › Save proposed "untitled", and the reopened title bar reads "untitled" while the Board card says "USB sensor board". |
| 20 | Generic Finder icon | Not checked. |
| 21 | Route card keys cut off | **Fixed:** the key descriptions wrap. The card is now too tall (finding 8). |
| 22 | Template preview labels | Not checked. |
| 23 | Placement glyphs look alike | **Fixed.** The lock glyph is green and the edge-constraint glyph is dark, and they differ at board zoom (`s18-placed.png`). |

## New findings by severity

Estimates are agent hours. "Founder's bar" marks findings about cut-off text, crowding or indistinguishable states.

### Blocker

| # | Step | What happened | What a designer expects | Suggested fix | Estimate |
|---|---|---|---|---|---|
| 1 | Schematic | **There is no way to mark a pin no-connect.** No Wire, Label or Power entry does it: the Label menu has Net, Global and Hierarchical label only (`s26-label-menu.png`). The command palette finds nothing for "no-connect" and only unrelated commands for "nc" (`s16-no-nc-command.png`). Right-clicking a pin shows no menu. The ERC row says "Connect the terminal or mark it no-connect", but offers only a locate chip. The seven unused pins stay **errors** in the production check, so a correct design cannot export without waiving them. | A no-connect (×) tool on the ribbon and in the palette (Q or X in other tools), plus a quick action on the ERC row. | Add a schematic no-connect marker as a typed command, accepted by ERC and the production check, with a "Mark no-connect" button on unconnected-terminal rows. | 3–4 h |
| 2 | Parts | **No USB-C part or template** (first run's finding 1, unchanged). | — | — | 6–10 h |

### Major

| # | Step | What happened | What a designer expects | Suggested fix | Estimate |
|---|---|---|---|---|---|
| 3 | Parts (pin import) | **Paste table silently dropped rows.** A five-row LDO table with a `Type` column of "power input", "ground", "input", "nc" and "power output" imported **3** pins (GND, EN, NC). The toast said only "Imported 3 pin rows; confirm each under Unreviewed" (`s05-ldo-pins.png`), with no word on the two rows dropped, and the Issues card then reported a missing mapping (`s06-ldo-issues.png`). Retyping the types as "power" imported all five. | Usual synonyms accepted. A row the importer cannot read is kept with a warning, or the paste is refused with the line named; it is never dropped quietly. | Map "power input/output", "pwr", "I/O", "bidir" and "open drain". Report rejected rows by line number in the preview before Import. | 2 h |
| 4 | Parts (functions) | **Every imported signal needs its Drive set by hand.** After importing the MCU with a Function column, 10 "Combination" errors read "Function 'PA05' of terminal 'PA05': bidirectional with digital signal and no [drive]". Each was cleared by clicking the row, opening Drive and picking Push-pull: 3 actions × 10 rows. The sensor needed the same for SDA and OS. With about half the time spent on driver retries, the MCU still took about 8 minutes. | A default (push-pull for outputs and bidirectional pins) or a `Drive` column in the paste, and multi-row editing. | Accept a `Drive` column, default bidirectional and output pins to push-pull with a warning rather than an error, and allow setting a column over selected rows. | 2–3 h |
| 5 | Schematic | **No obvious way to end a wire in empty space or to delete a stray label.** A label click that missed a pin left "Label SDA on Sheet 1 has no wire" (a warning to the end of the run). With the label selected, Delete did not visibly remove it, and the card's **Remove** button was the way out. A wire run must end on a pin, wire or port; ending in space (double-click or Esc, as in other tools) was not discoverable from the Wire card, which says only "Click a terminal or wire to start a wire" and "Crossing wires are not joined". | Double-click or Esc ends a wire at the last corner. Delete removes the selected label. | Put "Double-click or Esc ends the wire" in the Wire card's keys, and bind Delete for labels as for symbols. | 1–2 h |
| 6 | Routing | **Esc after a route drops to Select**, so the next pad click selects the part and the following clicks move nothing. Two batches of route clicks were lost this way. The Route card's keys say "End the route (or right-click); again: Select", which is accurate but easy to miss. | Esc ends the current route and keeps the Route tool armed. A second Esc leaves it. | Already specified in the Route card; check that one Esc after a *finished* route (Enter or a pad of the net) also keeps the tool. | 1 h |
| 7 | Update PCB | **The review is a wall of pad errors.** Applying Update PCB with all parts staged beside the board listed about 49 rows such as "New error: U1 pad 1 exte…", each cut off with a Show button, and a toast said "90 new issues after this update" (`s17-update-pcb.png`). These are the expected result of staging off-board, not errors in the update. Founder's bar. | Staged parts are not reported as off-board errors until placed, or are summarised as one line ("12 parts staged beside the board"). | Treat staged (never placed) parts as exempt from the outline check, or group the rows by part with one summary line. | 2 h |

### Minor

| # | Step | What happened | What a designer expects | Suggested fix | Estimate |
|---|---|---|---|---|---|
| 8 | Route card | **The Route card overflows the window.** With the Route tool active, the card's top edge rose above the ribbon to the title bar, covering the window's top-right, and its list still ended in "8 of 16 · Show all". Founder's bar. | The card stays inside the right column and scrolls. | Clamp the card to the panel height and scroll its body. | 1 h |
| 9 | Component Issues | **Hover tooltips are cut at the window edge.** Hovering "Function 'PA05' of termina…" showed a tooltip that itself ran off the right edge: "…bidirectional with digital signal and no…" (`s09-combination-hover.png`). Founder's bar. | Tooltips wrap within the window. | Give tooltips a maximum width and wrap them. | 30 min |
| 10 | Production check | **Group titles truncated**: "Unrouted connecti… 10", "Unconnected termin… 7" in the 240 px column, even though the count could move under the title (`s22-production.png`). Founder's bar. | The group title in full; the count after it or on a second line. | Wrap group titles, or show the title on hover like rows. | 30 min |
| 11 | Production check | "GND: 12 pads unrouted" appeared after a top pour that visibly reached most GND pads, and the airwire count fell from 29 to 19. The wording does not say whether 12 is the net's pad count or the unrouted ones. | "GND: 3 of 12 pads unconnected". | Report connected/total, and recheck after a fill finishes. | 1 h |
| 12 | Shortcut | **⇧⌘U does nothing in the Schematic workspace** (first run's open item). | Update PCB from either workspace, opening the PCB review. | Bind it app-wide. | 30 min |
| 13 | Save | **⌘S on an untitled project does nothing visible; File › Save proposes "untitled"** (first run's finding 19). | ⌘S opens the save panel with "USB sensor board.pcbcraft". | Same as before. | 1 h |
| 14 | LED | No LED template; an LED made from the 0603 chip template draws as a plain diode, without the emission arrows that tell a designer it is an LED. | An LED template (0603, 0805) with the LED symbol. | Add an LED symbol kind to the chip template. | 1 h |

### Polish

| # | Step | What happened | Suggested fix | Estimate |
|---|---|---|---|---|
| 15 | Component Function card | The LM75B OS function card showed "Mux code AF7, ALT2" and "Mux table LP IO MUX" (`s10-sensor-published.png`). If these are placeholders, they read as real values for a part that has no mux. | Grey, clearly hinted placeholders, or hide mux fields for parts without peripherals. | 30 min |
| 16 | First dimension | The first dimension on a hole showed "2.9452" in the Value field (the measured distance to four decimals). Setting it to `inset` was then easy, but the raw measurement reads oddly. | Show measured values to the grid's precision (2.95). | 30 min |
| 17 | Command palette | "nc" matches commands such as "Junction point" and "Coincident" (substring match), and lists commands not available in the current workspace first ("Not available here; applies in Component › Footprint") (`s16-no-nc-command.png`). | Rank available commands first and prefer word starts. | 1 h |

## Founder's parametric checks

- **Parametric holes: work.** Four holes, eight dimensions to the four edges, all set to `inset`. The sketch read "Fully defined". Changing `inset` from 3 to 5 in the card moved all four holes at once, and the value survived save and reopen. Labels still overlap at the corners (first run's finding 8).
- **Fixed and locked parts: work, with distinct signs.** J2 was held to the bottom edge with one click on **Constrain to edge**, and shows a dark edge glyph. J1 was locked with the **Locked** box: it shows a green lock glyph, its card reads "Locked in place" with greyed position fields. Both survived save and reopen.

## What worked well

- **The schematic reads like a designer's sheet** (`s15-issue-expanded.png`):
  - values on passives in small text;
  - T-junction dots;
  - outward-facing net labels;
  - GND and +3V3 ports;
  - generated IC boxes that fit their pin names.
- **The issue lists say what to do.** For example "U2 terminal 'RESET' is neither connected nor marked no-connect · Connect the terminal or mark it no-connect", with a chip that locates the pin.
- **Update PCB staging and placement.** Parts arrived in tidy rows beside the board, and twelve drags placed them. Constrain to edge and Locked are one click each.
- **Board shape.** Holes with parameters took about two minutes in all.
- **Pour.** It took four clicks and Enter, had real thermals and clearances, and the board stays readable.
- **3D in 5 seconds**, with a clean model.
- **Quit and reopen.** No prompt when saved, and everything came back.

## Driver limits (not product bugs)

- The native Save panel's name field did not take typed text, so the file kept the name "untitled". Return accepted the panel's default.
- The guest MCP server was disconnected, so all driving went through the CLI. Clicks on component table rows sometimes did not register right after a dropdown closed and had to be repeated; this inflated the MCU's Drive-setting time.
- Hover was driven by moving the real guest pointer (`move_cursor`, desktop scope), which worked for tooltips.

The board was not committed. Its land patterns are template defaults, and the LDO, MCU and sensor pin tables were typed from memory and recorded as unverified. The file and recovery folder were deleted from the guest after the run.
