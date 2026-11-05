# End-to-end UX run through the desktop UI: USB sensor board, one hour

On 5 November 2026 an agent designed a small USB-powered sensor board in Pcbcraft's desktop app the way a designer would: through the ribbon, the cards, the canvas and the shortcuts. The aim was to judge the UX against the founder's bar: text that is readable, distinguishable and expands on hover when shortened; screens that are not crowded; and a next step that is obvious. The app ran in the Herdr macOS VM (macOS 27.2, 1920 × 1200 display at 1×, app window 1380 × 900). The build was 0f6b200 (`component-model` branch) with the `ci` profile, bundled with `apps/pcbcraft/macos/Info.plist`. The budget was **one hour of design time**. The clock started at the first action in the app, 18:34:06, and the run stopped at 19:24:09, after **50 min 3 s**, with every step reached. The agent read no product source, changed no product code and did not use `pcbcraft-cli`. It made no paid API calls.

Screenshots referenced below are in the agent's `target/e2e-ux/` and were not committed.

## Outcome

**All 12 steps were reached, and Gerbers, drill files, BOM and pick-and-place were exported.** The 30-minute run in [e2e-ui-run.md](e2e-ui-run.md) stopped at placement (step 6). The board was not finished to a fab-ready standard:

- The schematic was wired with wires, power ports and a few net labels, but it still had 13 ERC errors (unconnected MCU and sensor pins that a designer would mark no-connect, and the LED cathode).
- Routing was a token: one GND connection from J1 to J2 on top copper, through a via, then on bottom copper. 25 airwires remained.
- There was a GND pour on the bottom layer, and refill worked.
- The production check had 23 errors. All 23 were **waived** with the reason "prototype" so that Export would unlock. The exported set is therefore not manufacturable.

| Step | Reached | Time (real clock) | Notes |
|---|---|---|---|
| 1. New project and parts | Yes | 18:34:06–18:49:52, **15 min 46 s** | ⌘N and the project name took a few seconds. Seven components: a resistor, a capacitor and a 1×4 header from templates; an LED from a 0603 template; an LDO (SOT-23-5) and an MCU (SOIC-14) from templates with pasted pin tables; and an LM75B sensor whose **symbol was drawn from scratch** (8 pins with the Pin tool) on a SOIC-8 package template. **No USB-C**: searching "usb" in Open and in New from template found nothing, so J1 is a 1×4 header standing in for it. There is no LED template either (search "led" found nothing). |
| 2. Schematic | Partly, 13 ERC errors | 18:49:52–18:58:04, **8 min 12 s** | Twelve symbols placed with the Place palette (P), arranged by function: input, LDO, MCU, sensor with pull-ups, LED, programming header. About 20 wire runs (40 segments), GND, 3V3 and VBUS power ports, and five net labels (SDA, USB_DP, USB_DM, SWDIO, SWCLK) where wires would cross the sheet. |
| 3. Update PCB | Yes | 18:58:04–18:58:37, 33 s | ⇧⌘U did nothing visible from the schematic. Through PCB › Update PCB › Apply it placed 12 parts in a row, overlapping (J2 on U1 and U2), inside the 60 × 40 default board. |
| 4. Board shape and holes | Yes | 18:58:37–19:03:04, **4 min 27 s** | Width 60 → 30 was accepted at once with the parts outside. Four mounting holes at the corners, eight dimensions driven by `inset`. |
| 5. Stackup and rules | No | — | Defaults kept. |
| 6. Placement | Yes | 19:03:04–19:05:32, 2 min 28 s | Twelve drags. J2 constrained 2 mm from the bottom edge, J1 locked. |
| 7. Routing | Token | 19:05:32–19:07:04, 1 min 32 s | One GND track, V for a via and a layer change to bottom. No USB pair (none exists), and no shove was exercised. |
| 8. Pour | Yes | 19:07:04–19:07:47, 43 s | GND on bottom: 4 clicks and Enter, then Refill ran in the background. |
| 9. DRC and production check | Yes, by waiving | 19:07:47 first look; waivers 19:13:50–19:22:20, **about 8 min 30 s** | 23 errors and 14 warnings. Waived the errors one at a time. |
| 10. 3D | Yes | 19:07:55–19:08:00, **5 s** | Clean model with headers, SOICs, passives, holes and silkscreen designators (`f-3d.png`). |
| 11. Export | Yes | 19:22:20–19:22:59, 39 s | 17 files: copper, mask, paste and legend Gerbers, profile, job file, plated and non-plated Excellon, drill map and table, BOM (2 CSVs), top pick-and-place CSV, fabrication and assembly PDFs. |
| 12. Save, close, reopen | Yes | 19:08:00–19:12:00, 4 min, and 19:23:53–19:24:09 | The save panel's name field could not be typed through the driver (a **driver limit**), so the file is `~/untitled.pcbcraft`. ⌘S, the quit prompt, `open -a` and Finder double-click all worked; the reopened file had the board, holes, inset, lock, constraint, track, via, pour and schematic. |

Where the time went: **parts took 31%**, **waiving production errors took 17%**, the schematic 16% and the parametric holes 9%. Placement, routing, pour, 3D and export together took under 7 minutes. Compared with the 30-minute run: parts took a little longer per component (about 2 min 15 s each against about 1 min 35 s), because this run drew a symbol from scratch and pasted two pin tables. The schematic is much faster (8 min against 10 min for a worse result), because wires and power ports worked first time, and Update PCB, board shape and placement no longer block.

**The board was not committed.** Its land patterns are the templates' "estimated template defaults", not vendor or datasheet land patterns. The LDO, MCU and sensor pin tables were typed from memory, not checked against datasheets. The routing is a token, and the production check passes only by waiver. It does not meet the provenance bar for `examples/`, and the file was deleted from the guest after the run.

## Findings by severity

Estimates are agent hours. "Founder's bar" marks findings about cut-off text, crowding or indistinguishable states.

### Blocker

| # | Step | What happened | What a designer expects | Suggested fix | Estimate |
|---|---|---|---|---|---|
| 1 | Parts | **No USB-C part or template.** "usb" finds nothing in Open, in New from template or in the Place palette. The board's headline feature, the USB pair and its differential routing could not be tried. This is the same gap as the 30-minute and ESP32 runs. | A USB-C receptacle (USB4105 or similar) from vendor CAD, or a connector family in the template gallery. | Ship one USB-C 2.0 receptacle Part from manufacturer CAD, with its source and licence recorded, plus a USB_DP/USB_DM pair preset. | 6–10 h |

### Major

| # | Step | What happened | What a designer expects | Suggested fix | Estimate |
|---|---|---|---|---|---|
| 2 | Production check | **Waiving is one row at a time, and the form opens below the fold.** Export stays disabled while any error remains. Each waiver is: click *Waive…* on the row, scroll to the bottom of the card (the Reason field and Waive button appear under the 37-row list, `f-waive-form-below-fold.png`), type a reason, click Waive, scroll back. 23 waivers took about 8.5 minutes, 17% of the run. There is no multi-select, no "waive all of this kind" and no way to say "prototype: skip unrouted". | Select several findings, or a whole group (for example "Unrouted connections"), and waive them with one reason. The form opens next to the row. | Group the list by kind with a group-level Waive, add shift-click selection, and open the reason inline under the row. | 3–4 h |
| 3 | Production check | **Rows are truncated with no hover, and stale tooltips cover the list.** "VBUS: 4 pads unro…", "SDA: a push-pull dr…", "U2 pin RESET unco…" are cut by the Waive button (`f-waive-squeezed.png`). A tooltip ("SWDIO: 2 | Route the connection | prototype") **stayed on screen** over other rows after the pointer left (`f-tooltip-over-row.png`), and later "Choose a Part with Resolve parts" stayed over the Output header (`f-stale-tooltip.png`). The Result line is cut too ("15 errors · 14 warnings · …"). Founder's bar. | Full text on hover, and a tooltip that disappears when the pointer leaves. | Give the finding text the row's full width and put the action in a hover or overflow button. Find the tooltip that outlives its hover (it looks like an egui tooltip kept alive by a layer id). | 2–3 h |
| 4 | Schematic | **Value text collides with neighbours.** Template symbols show the component name as value text at designator size: "Chip capacitor 0603 (1608 metric)". C1's and C2's run into each other ("metChip capacitor"), C3's runs into "MCU ATSAMD11C14A", R3's and D1's overlap ("(1608 metric)" on "LED 0603 green"), and R1 and R2, turned 90°, have their values written over U3's SDA and SCL pins and the SDA label (`f-labels.png`, `f-T-no-dot-label-overlap.png`). J1's and J2's values are cut off at the left edge of the view ("n header 1×4 2.54 mm"). The 30-minute run reported the same. Founder's bar. | A short value (100 nF, 4.7 kΩ, AP2112K-3.3) that is smaller than the designator, and placed clear of pins when the symbol turns. | **Fixed.** The slot under a symbol shows the instance's value (`instances::value_text`, the first shown field with a value, which the BOM reads too), else the Part's MPN, else the component name only for parts of more than two terminals; a two-terminal part without a value shows nothing. Values are 1.0 mm (`VALUE_TEXT_HEIGHT_MM`), below the 1.25 mm designator, drawn once. A symbol turned a quarter keeps designator and value upright beside its body, clear of its pins. Template passives still select no value field, so they show none until one is added (finding 16). | 2 h |
| 5 | Parts (IC box) | **Generated IC symbols overlap their own pin names.** On the SOIC-14 MCU the box is too narrow: "RESET" and "USB_DM" overlap ("RESETUSB_DM"), as do "SWCLK" and "SWDIO", and "PA15" touches "USB_DP" (`f-mcu-pin-names-overlap.png`). Founder's bar. | The box widens to fit the longest left and right names. | **Fixed.** `samples::fit_pin_names` widens a box until, on every row, the left and right names (0.95 cap heights per character, as printed) and a half-module gap fit; pins stay on the grid. Generated IC boxes, pin-table imports into a draft (its symbol drafts widen in the same step) and Redraw as › IC box all use it. Existing published symbols change only through a new revision: open the component, Redraw as › IC box (or re-import the pin table), publish, and update the instances. | 1–2 h |
| 6 | Parts (pins) | **Pin import still needs per-row work.** Paste table with a `Type` column accepted some kinds ("ground", "power") but not "power input", and after import every pin was flagged as an unreviewed function ("Function 'VIN' of terminal 'VIN'…", truncated, `f-issues-truncated.png`). Setting the kind for the nine MCU signal pins took 3 clicks each, about 27 clicks. | Common type words map (power input, power, input, output, I/O, passive, NC), and a selection of rows can be set at once. | Accept the usual synonyms in the Type column and add multi-row editing. | 2–3 h |
| 7 | Parts (scratch symbol) | **The first pins went inside the body.** With the Pin tool, a click on the body's top edge placed pin 1 lying along the edge, pointing into the box (`f-pin-inward.png`). After an undo, the next pin landed on the same spot as the previous one, so the numbers "1" and "2" were drawn on top of each other (`f-pins-stacked.png`). Typing the position in the card (−7.5, 5) and then clicking just outside the left and right edges gave a clean symbol (`f-scratch-symbol.png`). | A pin clicked on an edge points outward, perpendicular to that edge. Two pins cannot share a connection point. | Orient a new pin away from the nearest body edge and refuse, or offset, a pin placed on an occupied point. | 2 h |
| 8 | Parametric holes | **Dimensioning eight edges was about 50 actions.** For each dimension: pick the hole, pick the edge, click Value, select all, delete, type `inset`, Enter. The first `inset = 3` did not take on the first two tries: the typed text was lost or replaced, and it took the End key and typing again. The dimension texts at each corner sit on top of each other ("inset = 3inset = 3") and on the edge names ("left", "bottom") (`f-holes-inset3.png`). The behaviour itself was right (see "Parametric checks"). | A Mounting hole option "inset from corners" that makes the four holes and their dimensions in one step. Dimension labels placed clear of each other. | Add the corner-inset pattern to the Mounting hole tool, and offset dimension labels that would overlap. | 3 h |

### Minor

| # | Step | What happened | What a designer expects | Suggested fix | Estimate |
|---|---|---|---|---|---|
| 9 | Schematic (wire) | **Every wire that joins nets opens the Connect pins review card** ("Adds 2 wires, removes 1 net, changes …", with the summary cut off), which needs **Apply update** (`f-connect-review.png`). It came up for every such wire in this run. This is the 30-minute run's finding 4, unchanged. | Silent apply for an ordinary wire, with review only for merging two named nets. | **Fixed.** A wire that joins nets applies at once, one undo step, and a toast says "Connected pins: …" with Undo. The review opens only for a risky join: it touches routed copper (blockers, conflicting tracks or vias), or it merges two differently named nets (names other than the automatic `N<id>`, from labels, power ports or a rename; a label not yet on a net counts by its own name). | 2 h |
| 10 | Schematic (wire) | **The J (junction) key gave no visible result.** After J, a click on a wire showed no dot. The Junction ribbon button was not tried for lack of time, so this may be a key binding gap rather than a broken tool. | J then click places a dot on the wire under the pointer. | **Fixed.** J was bound; the armed tool showed the Wire card ("Wire · Start: Not started"), so it looked like nothing happened. The card now reads "Junction point" with its own hint, a click on a wire places the point and its dot, and a click on no wire says "No wire there: click on a wire or a crossing to place a junction point". | 30 min |
| 11 | Schematic (labels) | **A label and a power port can share a spot.** The SWCLK label at U2 pin 7 sits over C3's GND port text (`f-swclk-label-on-gnd.png`). That was partly the agent's placement, but nothing flagged it, and at normal zoom it reads as "SWCLKGND". | A warning, or automatic offset, when two annotations overlap. | **Fixed.** When nothing along the pin is clear, a self-placing label steps aside on a bent leader (out half a module, then a module at a time to either side); a power port turns with its leader, a ground preferring down. Limit: four labels on adjacent pins facing each other across a gap narrower than the labels can still overlap; a person's (manual) placement is never moved. | 1–2 h |
| 12 | Update PCB | **New parts are dropped in a row on top of each other** (J2 over U1 and U2, in red), and the edge names "top", "right", "left", "bottom" sit at the board's corners rather than at the middle of their edges, so "top" is at the top-left corner and "bottom" at the bottom-right (`f-update-pcb-overlap-edge-names.png`). ⇧⌘U from the schematic did nothing visible. | New parts staged beside the board without overlap, roughly in schematic order. Edge names centred on their edge. ⇧⌘U works from the schematic. | Stage new placements off-board in a non-overlapping grid seeded by the schematic. Centre edge names. Check the shortcut's context. | 3 h |
| 13 | Component and Issues cards | **Identical truncated issue rows.** "Terminals without a pin 8" listed eight rows all reading "Symbol 'Temp sensor LM75B s…" (`f-identical-issue-rows.png`). Hovering did not show the rest, so the rows could not be told apart. Founder's bar. | The distinguishing part first ("Terminal 3 (OS) has no pin"), with the full text on hover. | Put the varying part at the start of the message and add a hover tooltip to issue rows. | 1 h |
| 14 | Component (empty state) | The right card of an empty Component workspace reads "Open a component, or start a new one, on t…", cut off with no hover (`f-truncated-hint.png`). Founder's bar. | The hint wraps. | Let info lines wrap in cards. | 30 min |
| 15 | Off-board DRC | Dragging R2 off the board gave "Outside the board outline 2", with rows "R2 pad 1 extends beyond the b…" and "R2 pad 2 ext…" cut off, and no hover text. Founder's bar. | "R2 pad 1 is 4.1 mm outside the board". | Shorter message ("R2 pad 1 outside board"), plus a hover with the full text. | 30 min |
| 16 | BOM | The BOM's Value column is the component name ("Chip capacitor 0603 (1608 metric)") for all three capacitors and all three resistors, with no values (100 nF, 4.7 kΩ), because nothing in the flow asked for one. Every Part is "no part". | A Value field on each placed symbol, carried to the BOM. A clear prompt that parts are not chosen before export. | Ask for Value when placing a passive from a template, and group the BOM by value. | 2 h |
| 17 | Drill table | The drill table lists two different tools as **T1**: "T1 0.300 plated" and "T1 3.200 non-plated". Each Excellon file numbers its own tools, but the combined human-readable table is ambiguous. | Unique tool numbers in the combined table. | Number the table's tools across both files, or prefix them (P-T1, N-T1). | 30 min |
| 18 | Pour | On the Layout view the bottom pour is drawn opaque and dark blue over the whole board, so top pads and designators are hard to read (`f-production-top-hidden.png`). | A pour drawn semi-transparent, or dimmed when it is not on the active layer. | Lower the opacity of inactive-layer fills. | 1 h |
| 19 | Save | **⌘S on an untitled project did nothing visible** (one try, 19:08). File › Save opened the native panel. On the saved file, ⌘S did save (see area 4). The save panel proposed "untitled", not the project name; the name could not be typed through the driver (a driver limit), and afterwards the title bar read "untitled" while the Board card read "USB sensor board". This repeats the 30-minute run's finding 16. | ⌘S on an untitled project opens the save panel with "USB sensor board.pcbcraft". | Check ⌘S for untitled projects by hand; propose the project name. | 1 h |

### Polish

| # | Step | What happened | Suggested fix | Estimate |
|---|---|---|---|---|
| 20 | Finder | A `.pcbcraft` file shows a blank generic document icon in Finder (`f-finder-icon.png`). | Add a document icon to the bundle (`CFBundleTypeIconFile`). | 1 h |
| 21 | Route card | The Keys list in the Route card is cut off ("Corner; a pad or track …", "Finish the route (or do…", "End the route (or right-…") with no hover (`f-route-continues.png`). Founder's bar. | Wrap the key descriptions, or show the full text on hover. | 30 min |
| 22 | Template gallery | The template preview's labels are cut off: "Body L × …" (`f-template-preview-labels.png`). The "cited?" badge is unexplained. | Widen the label column or wrap; explain "cited?" on hover. | 30 min |
| 23 | Placement glyphs | The constrained and locked glyphs are about 10 px squares at the top-right of the part. Up close they differ (a dimension mark against a lock), but at board zoom they read as two similar small squares, and they are not colour-coded (`f-constrained-card.png`). | Give them distinct colours, as the card's state row already does. | 30 min |

## The five recently changed areas

1. **Net labels: works, mostly looks right.** Local labels are rounded tags in small text (it looked close to 1.0 mm, about the size of pin numbers), on the pin line and facing outward: SDA, USB_DP, USB_DM and SWDIO to the right of U2's right-hand pins, SWDIO and SWCLK to the right of J2's pins, and SDA to the left of U3's pin 1. None overlapped its own symbol body (`f-labels.png`). Problems came from other text: R1's and R2's value text runs over U3's SDA label, and SWCLK sits on C3's GND port (findings 4 and 11). **Global labels (pointed flags) were not tested**: the run used only local labels.
2. **Wires: partly confirmed.**
   - Works: a wire ending on a pin end connects (the Connect pins review shows the net change); the snap marker is a blue ring on the pin end (`f-wire-snap.png`); the unconnected-pin circle is small and sits just outside the pin end.
   - Not confirmed: a T-junction dot. Where R2's pull-up wire meets the SCL wire, no dot was visible, but R1's rotated value text lies over that point, so the run could not tell (`f-T-no-dot-label-overlap.png`). L corners drew cleanly. Crossings were not specifically checked.
   - Did not work as tried: J then click on a wire placed no visible junction (finding 10).
3. **Placement states: works and reads well.**
   - Glyphs: J2 (constrained) shows a dimension glyph, J1 (locked) a lock glyph, free parts nothing. They are small (finding 23).
   - Card state rows: "Held 2 mm from the bottom edge" with **Free part** for J2, with greyed Position fields and its Board edge section; "Locked in place" for J1, with greyed fields and a ticked Locked box (`f-constrained-card.png`, `f-locked-refusal.png`).
   - Drag refusals: clear toasts naming the reason and the fix: "J2 is held 2 mm from the bottom edge; edit or remove its constraint to move it" and "J1 is locked; unlock it to move it" (`f-constrained-refusal.png`, `f-locked-refusal.png`).
   - Shrinking: 60 → 30 mm width was accepted with parts outside, no refusal. A part dragged off the board is allowed, and DRC reports it under "Outside the board outline" (truncated, finding 15).
4. **Quit and save: works.**
   - ⌘Q with unsaved changes asked "Save changes before quitting?" with **Cancel**, **Don't Save** and **Save** (`f-quit-prompt.png`). Cancel kept the app open and dirty.
   - The window's close button asked the same. **Save** wrote the file (its timestamp changed) and quit.
   - At the end, ⌘Q with **Don't Save** quit, and the recovery folder was left empty.
   - ⌘Q with nothing unsaved quit at once.
   - The real ⌘S shortcut saved an already-saved file: the file size changed from 37 039 to 36 993 bytes after changing `inset`. On an untitled project, ⌘S showed nothing (finding 19).
   - The app menu's Quit and Dock › Quit were **not tested** for lack of time.
5. **Opening from Finder: works.**
   - `open -a Pcbcraft ~/untitled.pcbcraft` launched the app with the file, and a toast confirmed "Opened /Users/agent/untitled.pcbcraft".
   - Double-clicking the file in Finder launched Pcbcraft and opened it, with board, schematic and parameters intact.
   - The file has a generic icon (finding 20).

## Founder's parametric checks

- **Parametric holes: work.** Four holes, eight dimensions to the four edges, all driven by `inset`. The sketch read "Fully defined". Changing `inset` from 3 to 5 in Board parameters moved all four holes at once (`f-holes-inset5.png`), and ⌘Z put them back at 3. The cost is the click count and the overlapping dimension labels (finding 8).
- **Fixed and locked parts: work.** J2 constrained to the bottom edge, J1 locked. Both are visibly distinct from a free part through the glyph and the card. Dragging either is refused with a toast that says why and what to do. This fixes the 30-minute run's findings 8–10.

## What worked well

- **The Place palette and the schematic tools.** P, type, Enter, click placed each symbol. ⌘R turned the pull-ups. Wires, power ports and labels attached where clicked, and labels faced outward on their own.
- **Board shape is no longer a trap.** The board shrank to 30 × 40 with parts outside it, and the parts' off-board state is just a DRC finding.
- **Placement constraints.** The state row, the glyphs and the refusal toasts are clear, and the wording is the same in the card and the toast.
- **Routing and pour basics.** Route with a via and a layer change took seconds. The Route card names the net, width, layer and router mode, and lists the keys. Fill then Refill ran in the background with an Undo toast.
- **3D in 5 seconds**, with a clean model.
- **Export.** One click wrote a complete, sensibly named production folder: Gerber X2 layers, job file, plated and non-plated drills, drill map, BOM, pick-and-place and PDFs.
- **Safety.** The quit prompt, Save from the prompt, ⌘S and reopening from Finder or `open -a` all behaved as a Mac user expects. No work was lost.

## Driver limits (not product bugs)

- The native Save panel's name field did not take typed text, so the file kept the name "untitled". The native folder chooser accepted Return for its default folder.
- `scripts/vm/guest` key delivery fails while a native panel holds focus unless the panel's own window id is targeted.
- The guest MCP server was unavailable this session, so all driving went through the CLI with full-size captures.
