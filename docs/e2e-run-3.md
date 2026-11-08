# Third end-to-end run: USB-C sensor board to a fab-ready export

On 6 November 2026 an agent designed a small USB-C powered sensor board in Pcbcraft's desktop app, working as a designer would, and took it through to production files. The aim was to reach Export with nothing waived, and to check save and reopen, the new features, edge cases and looks along the way. It follows the [first](e2e-ux-run.md) and [second](e2e-ux-run-2.md) UX runs, the [precision run](e2e-precision-run.md) and the [batch check](e2e-batch-check.md).

**Setup.** The build was 09556d6 (`component-model`), installed as `~/Applications/Pcbcraft.app` in the Herdr macOS VM: a 1920 × 1200 display at 1×, with a 1380 × 900 app window. It was driven through the guest CLI, because the guest MCP server was disconnected.

**Limits.** The agent read no product source, changed no product code, did not use `pcbcraft-cli` and made no paid API calls. Saved files were unzipped on the host and read as `project.json`. Screenshots (named `…png` below) are in the agent's `target/e2e-run-3/` and were not committed.

## Outcome

**Did it reach export with zero waivers? Yes.**

| | Result |
|---|---|
| Waivers | **0**. The saved project has no waiver data, and no Waive button was used. |
| ERC errors at the end | **0**. The only schematic issues are 16 "No Part chosen" notices (`q1.png`, repeated after reopening in `fin2.png`). |
| DRC errors at the end | **0**, with 0 airwires and "No issues" for the PCB (`am4.png`). |
| Production check | **16 warnings, 0 errors**: all "No Part chosen", because no manufacturer part was picked for any instance (`am4.png`). |
| Export | **"Wrote 17 production files"** at 73:58 (`ao1.png`). |
| Blockers | **None.** |

The clock started at 11:57:22. The final reopen and compare ended at 13:20:39, **83 min 17 s**, inside the 90-minute budget. Quit came at 81:02.

"Fab-ready" here means that the product's checks pass. It does not mean this board should be ordered:

- The LDO, MCU and sensor pin tables were typed from memory and recorded as unverified, and the template land patterns are labelled "cited (unverified)" or estimated.
- The Gerbers were not opened in an independent viewer. Only the drill file, drill table, BOM and placement file were read as text, and they agree with the design (see [Export](#export)).
- The USB pair is routed by hand and has 5.57 mm of skew and an open 0.5 mm stub, which the check doesn't flag (finding 8).

### The board

16 instances: USB-C receptacle (the new template) with 5.1 kΩ CC resistors, a 3.3 V LDO with input and output 1 µF caps, a 14-pin MCU with a 100 nF cap and a 10 kΩ reset pull-up, an I²C sensor with 4.7 kΩ pull-ups and a 100 nF cap, an LED (the new template) with 1 kΩ, a 1×4 programming header, and four mounting holes. The board is 60 × 40 mm on two layers, with 117 track segments, 12 vias and a GND pour on each side.

### Time per step

Times are real clock. Much of the driving was batched into scripts with fixed pauses, so these are not human times. A person would be slower at clicking and faster at seeing what went wrong.

| Step | Clock | Time | Notes |
|---|---|---|---|
| Parts | 11:57:22–12:03:56 | 6:34 | ⌘N, then eight components from templates. Pin tables were pasted for the LDO (5 of 5 rows), MCU (14 of 14) and sensor (8 of 8). About 1 min was lost to the wrong template rows (finding 1). |
| Place symbols | 12:03:56–12:10:29 | 6:33 | About 2.5 min of this was a bug in the agent's own helper script. |
| Wires | 12:10:29–12:15:59 | 5:30 | About 1 min was lost when a script clicked with the Wire tool not armed; not a product fault. |
| Power ports | 12:15:59–12:21:33 | 5:34 | Mostly turning ports the right way up (finding 3). |
| +3V3, labels, no-connects | 12:21:33–12:24:49 | 3:16 | Six marks: one by Issues row action, the rest with the tool. |
| ERC fix and values | 12:24:49–12:30:13 | 5:24 | Open-drain drive on SDA and SCL, and value fields added to the passives (findings 2 and 13). |
| Copy, paste, delete tests | 12:30:13–12:32:57 | 2:44 | |
| Update PCB | 12:32:57–12:33:16 | 0:19 | The first ⇧⌘U did nothing and the second opened the review (finding 14). |
| Holes and inset | 12:33:16–12:36:03 | 2:47 | 8 dimensions in 58 s. |
| Placement | 12:36:03–12:43:54 | 7:51 | Typed coordinates, then adjustments. |
| Routing | 12:43:54–13:05:10 | 21:16 | Includes the mid-routing save and reopen. About 6 min went on the USB_DP route (findings 4 and 5). |
| Pours and last airwire | 13:05:10–13:06:50 | 1:40 | GND on Top and Bottom, then one via for U1 pin 2. |
| Check, label review, menus | 13:06:50–13:09:23 | 2:33 | Production: 17 warnings. |
| Fixed, locked, USB pair | 13:09:23–13:10:10 | 0:47 | |
| Fix the legend warning | 13:10:10–13:10:44 | 0:34 | Moved C3's designator, which took the check to 16 warnings. The agent's first click missed the field and its typed "3" opened Place 3D part. |
| Folder and Export | 13:10:44–13:11:20 | 0:36 | |
| 3D | 13:11:20–13:11:47 | 0:27 | Opened within the 3 s pause before the capture. |
| Edge cases | 13:11:47–13:17:45 | 5:58 | |
| Final save and quit | 13:17:45–13:18:24 | 0:39 | |
| Reopen and compare | 13:18:24–13:20:39 | 2:15 | |

A few menu, delete and NC checks that the run missed were done afterwards on a throwaway project, 13:37–13:43, and are not in the clock. They are marked "after the run" below.

## 1. Save and reopen, item by item

**No difference was found, in either round.**

### ⌘S right after typing in a card field: fixed

C4's Capacitance was typed as `100 nF` and ⌘S pressed with the field still focused and Return not pressed (12:30:13). The toast read "Saved /Users/agent/USB sensor board.pcbcraft", the unsaved dot cleared, and the file on disk held C4's value: `"si": "1e-7", "display_unit": "nF"` (`n6.png`). This is the batch check's F1, and it no longer happens.

### Round 1: mid-routing, ⌘S then ⌘Q then reopen (12:58:59–12:59:18)

| Item | Before | After reopen |
|---|---|---|
| Placements, wires, labels | 16, 50, 22 | Same |
| Tracks, vias | 62, 3 | 62, 3; **all 62 tracks are in the final file with the same ids, nets, layers and endpoints** |
| Airwires, issues | 32, 11 | 32, 11 (`mid0.png`, `mid2.png`) |
| Holes, `inset` | 4, 4 mm | Same |
| File MD5 | `bba84171…` | Same; the app did not rewrite the file |

The lock and the edge constraint were not yet applied at this point, so round 2 covers them. Quit asked nothing and the Recovery folder was empty.

Three things did change, none of them data: the active layer went from Bottom back to Top, the view zoom reset, and the Browser's expansion state changed.

### Round 2: at the end, ⌘S then ⌘Q then reopen (13:17:45–13:20:39)

| Item | Before saving (UI, and file where the UI can't show it) | After reopen |
|---|---|---|
| Board | 60 × 40 mm, "Sketch · 4…", 1 part on edges | Same (`inv-post-board.png`) |
| Sheet | Sheet 1, A4 landscape, 16 symbols, 50 wires, 22 labels | Same (`inv-pre-sheet.png`, `inv-post-sheet.png`) |
| Labels | 12 GND, 6 +3V3, 2 USB_DP, 2 USB_DM | Same count in the card |
| No-connects | 6: J1 SBU1 and SBU2; U2 PA0, PA1, PA2; U3 OS | ✕ marks in place; 0 errors |
| Parts, spot checks | J1 (4, 20) 270°; U2 (30, 20) 180°; C3 (36, 22.5) 90° | Same |
| Tracks | 117 (96 Top, 21 Bottom) | Browser: 117 |
| Three tracks | 176 +3V3 Top 0.3 mm (33.81, 36) → (33.81, 32.69); 233 USB_DP Bottom 0.3 mm (5.5, 18) → (28.5, 18); 289 N1 Top 0.3 mm (15.5, 17) → (15.5, 19.5) | Track 176's card is identical; the file is unchanged |
| Vias | 12 | 12, from the file |
| Pours | GND Top, GND Bottom | Both drawn; 0 airwires |
| Holes, `inset` | 4, 4 mm | Same (`inv-post-board.png`) |
| Locked part | J2: "Locked in place", greyed fields, lock glyph, dim Browser icon | Same (`aj1.png` before) |
| Fixed part | J1: held 4 mm from the left edge, 20 mm along, edge glyph | Same |
| USB pair | 1 pair · 0 groups | Same |
| Production check | 16 warnings | 16 warnings (`fin1.png`) |
| ERC | 16 notices, 0 errors | Same (`fin2.png`) |
| File MD5 | `44220bf2…` | Same |

Pixel-comparing the card crops for J1, J2, R5, C3 and tracks 176 and 177 before and after showed differences only in a 4-pixel strip at the Issues card's top edge, not in any value.

Limits of this round:

- The final state was not re-saved after reopening, because a save with no edits proves little. Round 1 did exercise that path, since the reopened data went on to be saved with more routing.
- The UI doesn't show track endpoints (finding 15), so the endpoints above come from the file.

**Lost on reopen, not in the document:** the Save-to folder for production files goes back to "Not chosen" (finding 16), and undo history is empty, as expected.

## 2. New features

### Right-click menus

The look is good: white rounded cards with clear grouping, right-aligned key chips, separators, and a red Delete. This is a large step from the precision run, where there were no menus at all. **The one fault common to all of them: Esc doesn't close a menu, and ⌘Z acts underneath an open one** (finding 6).

| Target | Items | Look (of 10) | Items clicked and what they did |
|---|---|---|---|
| Part (schematic) (`l0-z.png`) | Rotate ⌘R, Mirror ⇧X, Edit component, No connect on all open pins, Delete… | 9 | Edit component opened the right component. The no-connect item greys out when no pin is open. Rotate, Mirror and Delete… were used by key or button rather than from the menu. |
| Part (PCB) | Rotate 90°, Flip side, Lock, Constrain to edge, Delete… | 9 | Delete… opened the removal review. The others were used from the card. |
| Pin (`j9.png`) | Mark no-connect, Start wire W | 8 | Mark no-connect worked. It covers the part it refers to. Start wire not clicked. |
| Wire | Add junction here, Delete… | 8 | Delete… opened "Disconnect wire…". A click selects the whole net, not the wire segment. Add junction not clicked. |
| Label (`sup13.png`, after the run) | Edit, Rotate ⌘R, Delete | 9 | Edit focused the Name field; Rotate turned it. Delete was checked by key. |
| Track (`ba1.png`) | Delete, Select net, Clear net highlight H | 8 | Delete worked and undid. Select net and Clear highlight not clicked. |
| Pour (`ae3.png`) | Refill, Delete | 8 | Delete removed the pour at once, and undo restored it. Refill not clicked from here. |
| Empty canvas (`sup12.png`, PCB in `af1.png`) | Place part P, Wire W, Label L, Paste ⌘V, Fit view F; PCB: Route, Via, Fit view | 9 | Paste worked, and is grey with nothing copied. The others not clicked. |
| Browser row (`sup14.png`, `sup15.png`, after the run) | Instance: Select, Locate. Component: Edit component. Folder (`sup16.png`): no menu | 7 | Shown, not clicked. An instance row offers no Delete, Rotate or Edit component, so it is much thinner than the same part on the canvas. |
| Issues row (`j7-z.png`) | Locate, Mark no-connect | 8 | Mark no-connect worked with an Undo toast. The menu covers the row it refers to. Locate not clicked. |

Nothing clicked did anything other than what it says.

### Schematic copy and paste

| Action | Result |
|---|---|
| ⌘C on C4, then the canvas menu's Paste row | C5 appeared at the clicked spot with the next free designator and its 100 nF value (`o3.png`). |
| ⌘V | C6 appeared at the pointer, which was over the title block (`o4.png`). Correct, given where the pointer was. |
| ⌘X | Opens the "Remove C6" review with a red button, rather than cutting at once (`o5.png`). Safe, though it's an extra click for an unwired part. |
| Edit › Cut in the system menu | Same review, for C5 (`o6.png`). |
| Edit › Paste, and paste after a cut | Not tested. |

Copy gives no sign that it worked, but Paste enabling in the menu confirms it.

### Wires and no-connect pins

| Case | Expected | Result |
|---|---|---|
| Wire ending on an already wired GND port (on a routed net) | Ends and connects | **Pass**: snap ring, "Drew the wire", no new errors (`at1.png`). Batch check's F2 is fixed. |
| Wire starting on an already wired +3V3 port | Connects | **Pass**: the click resolved to R4 pin 2, which sits at the same point as the port (the card reads "Start R4 pin 2"), and the wire joined +3V3 (`as1.png`, `as3.png`). |
| Wire ending on a marked NC pin (U2 PA0) | Applies and reports an error | **Pass**: drawn, with an error row "U2 terminal 'PA0' is marked no…" under "No-connect pins connected" (`k5.png`). |
| Wire ending on a device NC pin (U1 NC) | Refuses with a message and keeps drawing | **Pass**: amber toast "A device-NC or reserved terminal cannot be wired", and the wire stays live (`k3.png`). Batch check's F3 is fixed. |
| Wire starting on a device NC pin (after the run) | Refused | Starting is allowed, then ending anywhere is refused with the same toast (`sup8.png`). Safe, but it should refuse at the first click (finding 22). |
| Removing a label on a routed net | Review | **Pass**: Delete on USB_DM opened "Remove label", listing the three tracks affected, each with a "Remove that track" tick. Apply stays grey until they are confirmed. Hover shows "Track on USB_DM would join pads the net no longer joins" (`ae1.png`). Batch check's F4 is fixed. Cancelled. |
| Wires following dragged parts | Stay attached and orthogonal | **Pass** on R1, R2 and C4. |
| Free wire ends | None left | **Pass**: no "Wire ends not connected" row at the end. |

### Delete and Backspace

| Target | Result |
|---|---|
| Unwired part | Opens a Remove review. |
| Wired part | Opens a Remove review listing the wires (`p3.png`). |
| Free label (after the run) | Removes at once, and undo restores (`sup23.png`). |
| Label on a routed net | Opens the review above. |
| Wire | **Nothing, with no message**, for Backspace and for fn+Delete (`p6.png`, `sup25.png`, `sup26.png`) (finding 7). |
| PCB track | Deletes, and undo restores. |

### Label ▾ and Settings

- **Label ▾:** a real menu now, and it arms Net label. Rated 8/10. Global and Hierarchical were not chosen.
- **Settings (⌘,):** opens as a card over the canvas; Esc closes it. Clear sections. Two faults remain: the title icon is an *fx* glyph, and the last visible row ("Command palette") is cut in half by the card's bottom edge (`ax1.png`) (finding 19). Rated 7/10.
- **System Edit and Window menus:** native look, with shortcuts and correct greying (`desk2c.png`, `ay3c.png`).

### Parametric holes, fixed and locked parts

- **Holes:** 8 dimensions set to `inset` took 58 s. The sketch read "Fully defined". Changing `inset` from 3 to 5 to 4 moved all four. The exported drill file has them at (4, 36), (56, 36), (4, 4) and (56, 4), which is exact for a 60 × 40 board with a 4 mm inset.
- **Fixed part:** J1, one click on **Constrain to edge**. Small edge glyph at the part, and the card reads "Held 4 mm from the left edge".
- **Locked part:** J2, one click on **Locked**. Small lock glyph, "Locked in place", greyed fields, Rotate and Flip disabled, and a dimmed Browser icon (`aj1.png`).

The two states are easy to tell apart. The glyphs are small at board zoom.

### USB pair

The pair tools were used only to define and measure. **Suggested pair** found USB_DP and USB_DM at one click after routing, and the card then gave lengths, skew and stubs (`al2.png`). Coupled routing was **not tried**: USB-C has two pads per signal, and the agent routed the two sides by hand across layers, then added the pair. Hand routing plus the pair's measurements worked. Whether the pair router copes with a USB-C receptacle is still open.

## 3. Edge cases

| Case | Result |
|---|---|
| Undo and redo across routing | Six ⌘Z steps took a 12-segment stray route away, one finished route at a time, checked by file: 62 → 60 → 56 → 54 → 53 → 52 → 50 tracks. **Pass.** |
| Undo and redo of a track delete | Three rounds went 117 → 116 → 117, checked by file, and the saved file was **byte-identical** to the one before the delete. One earlier sequence at half-second gaps ended a step off and needed one more ⌘Z, and it did not recur at 1 s gaps. Cause unknown; it could be a dropped key from the driver. |
| Deleting a wired part (R6) | Review, then Cancel leaves it unchanged (`p3.png`). **Pass.** Esc doesn't close the review (finding 6). |
| Deleting a routed part (R5) | Review that lists every affected track segment, with "Remove R5" disabled until each is ticked; some are 0.1 mm long (`aw1.png`). **Safe but long** (finding 12). Cancelled. |
| Moving a locked part | Drag gave a red toast, "J2 is locked; unlock it to move it", and the part stayed (`aj1.png`). **Pass.** |
| Invalid field values | `abc`: red "unknown parameter `abc`". `-2 mm`: refused with "the dimensions 'width', 'height', 'inset' with 4 other constraints cannot be satisfied" (`r15.png`). A later valid `4 mm` cleared the error. The precision run's stale-error finding is fixed. **Pass.** |
| Schematic change after routing, then Update PCB | New C5 wired to +3V3 and GND. Update PCB added one placement, C5, staged beside the board. All 117 tracks stayed, with 2 airwires for the new part (`au2.png`). The **32 new issues** toast overstates it (finding 9). Undone with four ⌘Z. |
| Via on a track end | Dropping a via on a track ended the route as expected; the next route then kept the old net (finding 5). |
| Quit with unsaved work (after the run) | Prompt with Cancel, Don't Save and Save (`sup27.png`). Don't Save exited, and Recovery was empty. **Pass.** |

## 4. Findability and looks

Times are the agent's estimate of how long a designer scanning the screen would take, not a measurement. The agent had read the earlier run reports, so it was not a blank-slate user.

| Step | Found without help? | Time | Notes |
|---|---|---|---|
| New project, name | Yes | Seconds | |
| New from template, "usb" | Yes | Seconds | The USB-C row is cited to the GCT drawing, with a pad preview and a USB-C filter chip. Very good. |
| Paste a pin table | Yes | About 10 s | In Pins, not on the template page. The form is long and Preview sits below the fold. |
| Place symbols | Yes | Seconds | Palette is clean, with `sym · fp · 3D · –MPN` chips. |
| Add a value to a passive | **No** | Over a minute | Finding 2. |
| Turn a power port | **No** | Over a minute | Finding 3. |
| No-connect | Yes | Seconds | Ribbon tool, N, Issues row and pin menu. |
| Update PCB | Yes | Seconds | |
| Mounting holes, parameter, dimensions | Yes | About 20 s | All in Shape; Add parameter is at the card's bottom. |
| Lock, fix | Yes | Seconds | |
| Route | Yes | Seconds | The Route card is thorough and scrolls properly. It is tall, with 11 rows above Keys. |
| Why a route click did nothing | **No** | Minutes | Finding 4. |
| Fill | Yes | Seconds | Fill for one pour, Refill for all. |
| USB pair | Mostly | About 30 s | Board card › High speed › Edit… › Suggested. |
| Production, folder, Export | Mostly | About 10 s | Export is grey until a folder is chosen, and nothing on the card says so (finding 16). |
| 3D | Yes | Seconds | |
| Delete a wire | **No** | Over a minute | Finding 7. |

### Cut off, crowded or unclear (the founder's bar)

| Where | What | Hover? |
|---|---|---|
| Review summaries | "Adds 16 placements: J1, U1 and 14 mo…", "16 parts staged beside th…", "Removes R5, 2 wires and its PCB place…", "Adds 1 net, removes 1 wire, changes 1 …" (`q2.png`, `aw1.png`, `ae1.png`) | Not tested on the summary line |
| Review rows and footer | "Track on N27 would join p…", "Confirm each bl…", "Remove that track (Bottom, 8.9 …" | Rows: yes, full text. The tooltip covers the tick row below it (`ae1.png`). |
| Issues rows | "N26: a push-pull driver shares t…", "U2 terminal 'PA0' is marked no…", "GND: 17 pads unrou…" | Yes, full text |
| Instance position field | "-137.5 m" clipped with no ellipsis (`o4.png`) | Not tested |
| Save-to path | "/Users/age…" (`ao1.png`) | Not tested |
| PCB status line | "grid 0.5 mm · 32 airwi…" on Bottom copper (`mid0.png`) | Not tested |
| Board card | "Sketch · 4…", "2 layers · 1.…", "Green · ENIG · …" | Not tested |
| USB-C symbol | Pin numbers run into pin names: "A4B9,B4A9" over VBUS and "A1B12,B1A12" over GND (`k1-z.png`) | Finding 10 |
| Dimensions at corners | "inset = inset = 5" labels overlap, top left and bottom right (`r15.png`) | Finding 18 |
| Fit button | Sits over the title block text on the sheet: "USB sens⊡oard" (`at1.png`) | Polish |

## Findings by severity

Estimates are agent hours.

### Blockers

None.

### Major

| # | Step | What happened | What a designer expects | Fix | Estimate |
|---|---|---|---|---|---|
| 1 | Parts | **Enter in template search can create the wrong size.** "resistor 0603" lists 0603 (1608 metric) first but highlights "0201 (0603 metric)", and Enter creates the 0201 (`b1-pick.png`). "capacitor 0603" does the same. "header 1x4" creates a **1×40**. It took 8 undos and about a minute to recover. | The closest match is the Enter target. | Rank exact size and imperial codes first; rank "1x4" above "1x40"; show which code system matched. | 1–2 h |
| 2 | Values | **Template resistors and capacitors have nowhere to put a value.** The instance card says "This component shows no fields on its symbol" (`m1.png`). Getting 5.1 kΩ on R1 took: Edit component, Add field, Resistance, Publish, Update placed instances; then the same for capacitors. About 3.5 min, and the instances revert to "r2". | Passive templates come with their value field, ready to type in. | Add Resistance and Capacitance to those templates. | 1–2 h |
| 3 | Schematic | **Power ports are awkward to turn.** A port placed on a horizontal stub comes out sideways (`h4.png`), and ⌘R and the ribbon's Rotate don't apply to it, although the label menu offers Rotate ⌘R. The only way that worked was the card's Rotation list, and which value points down was found by trial. About 5.5 min for the sheet. | Ports point sensibly when placed, and ⌘R turns them. | Default GND downward and supplies upward; make ⌘R and ribbon Rotate work on ports. | 1–2 h |
| 4 | Routing | **Route clicks fail silently in Shove.** Clicking the far end of a blocked bottom-layer lane did nothing: a hairline orange preview and no message (`w5.png`). In **Ignore** mode the same click said "Too close to a pad on USB_DM: needs 0.2 mm" (`x7.png`), which was the reason all along. The name "Ignore" also suggests it would allow this. About 4 min lost. | Every refused click says why. | Show the same toast in Shove and Walkaround; rename Ignore or explain it in the dropdown. | 1–2 h |
| 5 | Routing | **Routes run on when an end click misses.** Ending on a track 0.25 mm off the grid added a corner rather than ending the route. Later clicks kept extending USB_DM across the board, 12 segments, and through a click on USB_DP's own pad. Separately, the card's **Net** stayed on GND after a via-ended stub, and it had to be set back to "From the first pad" (`v7.png`). | Track snap beats grid snap near a same-net track; a new route starts on the clicked pad's net. | Prefer same-net track snap within a tolerance; clear Net when a route ends. | 2 h |
| 6 | All menus, reviews | **Esc doesn't close context menus or review cards**, and ⌘Z acts under an open menu (`k10.png`, `ay2.png`, `p3.png`). A second right-click with a menu open doesn't reopen it either. | Esc closes the top-most thing. | Handle Esc in the menu and review layers; block document shortcuts while a menu is open. | 1 h |
| 7 | Schematic | **Delete and Backspace do nothing on a wire, with no message** (`p6.png`, `sup25.png`), while the same keys work for parts and labels. The way out is the card's "Disconnect wire…" or the menu's "Delete…". | Delete on a wire opens the same review. | Bind the keys to it; name the action the same in both places. | 1 h |
| 8 | Production | **The check says nothing about the USB pair's real problems.** With a defined pair showing 5.57 mm skew, a 12.8 mm stub and a **0.5 mm stub to open** (`al2.png`), the check reports no issue. The stub to open is dangling copper on any net. | At least a warning for dangling track ends, and for pair skew and stubs. | Add a dangling-track warning; warn on pair skew and stubs against stated defaults. | 2–3 h |
| 9 | Update PCB | **"32 new issues" for one new capacitor.** The review's list and the toast say 32, including every GND pad as "unrouted (new)". The panel then shows 2 errors and 2 notices (`au2.png`). The same panel says "GND: 18 pads unrouted" with 2 airwires in all, and earlier "GND: 17 pads unrou…" with 1 (`z18.png`). This is the second run's finding 11, unchanged. | "C5 is staged and needs 2 connections." | Count only what the update changed; say "1 connection left" rather than a pad count. | 1–2 h |
| 10 | Schematic | **The USB-C symbol's pin text collides** (`k1-z.png`), at every zoom. The long name under the symbol also runs far past its body. Founder's bar. | Numbers sit clear of names; merged pads show a short form. | Wrap or shorten combined pin numbers, and wrap the name line. | 1–2 h |

### Minor

| # | Finding | Estimate |
|---|---|---|
| 11 | At fit zoom, J1's pad names and its designator sit on top of each other on the PCB (`am4.png`). They read properly once zoomed (`x7.png`). | 1 h |
| 12 | The removal review ticks one row per track segment, including 0.1 mm corner pieces (`aw1.png`). Group by net with one "Remove these tracks" tick. | 1–2 h |
| 13 | An imported MCU's SDA and SCL default to push-pull, so a correct I²C bus raises the "Open-drain contention" error against the sensor. The check is right given the data; the fix took about 2.5 min through Edit component. A warning, or defaulting pins named SDA and SCL to open drain, would save that. | 1 h |
| 14 | The first ⇧⌘U from the schematic did nothing (`q1.png`); the second opened the review. This is the batch check's F6, still intermittent and still of unknown cause. | 1 h |
| 15 | A track's card gives Length and Ends ("open → open", "J2 pad 1 → open") but no coordinates. | 30 min |
| 16 | Export is grey until a folder is chosen, with no hint on the card; the chosen folder isn't kept across reopen (`am4.png`, `fin1.png`). | 1 h |
| 17 | After publish, the Pins tab stays on the "Unreviewed 0" filter and shows "No pin matches this filter", beside a toast saying "confirm each under Unreviewed". The Browser row says "r1 · draft" with a blue dot while the chip says "published r1" (`sup6.png`). | 1 h |
| 18 | Inset dimension labels still overlap at the corners (`r15.png`); first run's finding 8. | 1 h |
| 19 | Settings: *fx* title icon and a half-clipped last row (`ax1.png`). | 30 min |
| 20 | The sheet card says Nets 14 and the Browser says 22 for the same design, with no explanation (`q1.png`). | 30 min |
| 21 | Browser instance rows offer only Select and Locate; folder rows have no menu. | 1 h |

### Polish

| # | Finding | Estimate |
|---|---|---|
| 22 | A wire can start on a device NC pin and is only refused at its end (`sup8.png`). "device-NC or reserved terminal" is also jargon. | 30 min |
| 23 | The Fit button overlaps the title block text on the sheet (`at1.png`). | 30 min |
| 24 | In the 3D view, one corner of the board shows a white haze over C4 (`ap1.png`). It may be lighting rather than a fault. | 30 min |
| 25 | BOM: D1 has an empty Value, because the LED's colour field wasn't filled and nothing prompted for it. J1's Value repeats its long part description (see [Export](#export)). | 30 min |

### Fixed afterwards: minor and polish

Checked by headless tests only; none of these was retried in the app.

- **11. Fixed.** A pad number shows only where it fits inside its pad on screen, so J1's fine-pitch numbers no longer stack over each other and the designator at fit zoom. They return when zoomed in (`a_pad_number_shows_only_inside_its_pad`).
- **12. Fixed.** The removal review groups a net's conflicting segments into one row, "3 tracks on USB_DM would join pads the net no longer joins", with one tick, "Remove these tracks (8.9 mm in all)". A single track keeps its own row. Apply still removes them all at once (`conflicting_tracks_are_one_line_per_net`).
- **13. Fixed.** The pin-paste drive default, which made outputs and I/O push-pull, now makes an I²C line open drain: a function whose label has the word SDA or SCL (`SDA`, `I2C1_SCL`, `SCL0`; not `SCLK`). A Type cell that states a drive (`push-pull`, `open drain`) still wins, and an input drives nothing (`i2c_lines_default_to_open_drain`).
- **14. Not fixed: no cause found in the app.** Ruled out: ⇧⌘U is not a menu-bar item, so the native menu never takes it and the dispatcher's drop of a menu-delivered key can't eat it; nothing else consumes ⌘U (no canvas, palette, key reveal or context menu reads it; key tips and an open menu let ⌘ chords through or close first); Update PCB's availability is computed fresh from the project at the press, not from a cached PCB ribbon; and a press over a focused field is held one frame and then runs. The flow test `update_pcb_runs_on_the_first_press` covers the settled sheet, the frame straight after ⌘2 and a focused field, and passes. Both sightings (batch check F6 and this run) were the first press(es) of a session sent through the guest driver, so a key lost while the driver brings the window forward is the leading remaining suspect. It needs a check by hand on the desktop.
- **15. Already fixed** with the major findings (c1d7fd1): the track card shows Start and End coordinates.
- **16. Fixed.** Without a folder, the Output section says "Choose a folder to save to, then Export" (or "a ZIP file"). The chosen folder is kept per user, by project file, in `settings.json`, and the next Production files dialog starts there while the folder exists. It is not in the project: it is this machine's path, it would make the portable file dirty and undoable, and per-user paths such as recent part files already live in the settings. A project never saved has no file to keep it by (`the_card_says_export_needs_a_folder_and_the_folder_is_kept`, `production_folders_are_kept_per_project_file`).
- **17. Fixed.** The Browser called a lineage a draft whenever a draft record existed, and a published set keeps its drafts open. It now calls it a draft only when the draft differs from its base revision, as the Component chip does. Publishing also takes the Pins filter off the emptied Unreviewed list (`after_publishing_the_browser_and_pins_filter_agree`).
- **18. Fixed.** A dimension label that would land on one already drawn slides along its own dimension line, by the least whole step that clears it (`corner_dimension_labels_do_not_overlap`).
- **19. Fixed.** Settings uses Solidcraft's settings icon, in its title and in the command palette, instead of the parameters (*fx*) glyph. The half-cut row was the old Solidcraft window body, which kept the height of the card's first frame. The repinned Solidcraft offers the body all the room the screen leaves, so the card ends at the screen's bottom edge, and a row cut there scrolls into view (`the_settings_body_scrolls_to_its_last_row`).
- **20. Fixed.** One meaning everywhere: a connected net joins two or more pins. The Browser's Nets folder counts every net and says how many reach one pin ("22 · 8 single-pin"), and lists those last. The sheet card's design-wide row is now "Connected nets" in its own Design section, no longer under "On this sheet" (`the_nets_folder_counts_single_pin_nets_and_lists_them_last`, `the_sheet_counts_only_nets_joining_two_or_more_terminals`).
- **21. Fixed.** An instance row's menu adds Edit component and Delete… (the sheet's removal review) to Select and Locate. Folder rows offer the commands that add to them: Components New component…, Schematic and Sheets Add sheet, Instances Place part and Annotate, Nets Wire and Label, PCB Update PCB and Production files…, Placements Update PCB, Tracks Route. All are registry commands, with their keys, run as from the palette (`a_rows_right_click_menu_picks`).
- **22. Fixed.** The start refusal was fixed with the major findings (c1d7fd1). The wording is now plain and names the pin, at the start and the end: "U1 pin ‘NC’ must stay unconnected, so it cannot be wired". Device-NC pins stay unwirable.
- **23. Fixed.** Fit view on the sheet keeps the page above the floating navigation bar's band, so the Fit button no longer sits on the title block (`fit_keeps_the_page_clear_of_the_navigation_bar`).
- **24. Not a defect now.** The run's build had a real fault that matches the haze: a mask's underside, 20 µm under its top, shaded through it at overview pixel density, and its clear coat reflected the studio at grazing incidence as a pale wash. c49269b fixed that on the SMLib kernel and new studio. The bright band that remains is the intended mirror image of a studio strip light on the gloss lacquer ([materials](materials.md#pcb-3d-look)).
- **25. Fixed.** The production check warns "D1 has no Value" (`fabrication.no_value`) for a part whose component selects an empty standard value field: resistance, capacitance, inductance, frequency or an LED's colour, known by the field's recorded origin, never its name. The BOM's Value is unchanged otherwise: the typed fields as shown, so an LED reads its colour once filled; else the chosen Part's MPN; else nothing. The component-name fallback for parts of more than two terminals is gone, so a connector or an IC without a Part has an empty Value, never its description. The ESP32 board's fab output is byte-identical: every part there has a value or a Part (`an_empty_value_field_warns_and_an_leds_value_is_its_colour`, `the_bom_value_is_the_shown_value_never_a_passive_component_name`).

## Export

17 files, all non-empty: Gerber (copper top and bottom, mask, paste, legend, profile, drill map), job file, plated and non-plated drill, drill table, BOM, parts BOM, top placement, and fabrication and assembly PDFs.

- **Drill table:** 12 vias (0.3), 4 plated slots (J1 shield, 0.6), 4 header holes (1.005), 2 non-plated pegs (0.65), 4 mounting holes (3.2): 26 holes. These match the design.
- **BOM:** 12 lines with the typed values (5.1 kΩ, 4.7 kΩ, 1 kΩ, 10 kΩ, 1 µF, 100 nF). Every line reads "no part", because none was chosen.
- **Placement file:** all 16 parts on top, with positions and rotations matching the cards.

The Gerbers were not checked in an independent viewer, and neither PDF was opened. No board was ordered.

## Status of earlier findings

| Finding | Status |
|---|---|
| Run 2 #1: no no-connect | **Fixed.** Tool, N key, pin menu and Issues row. |
| Run 2 #2: no USB-C part | **Fixed.** Template cited to the GCT USB4105 drawing. |
| Run 2 #3: pin paste drops rows | **Fixed.** All rows imported: 5 of 5, 14 of 14, 8 of 8. |
| Run 2 #4: Drive on every function | **Fixed for the common case.** See finding 13. |
| Run 2 #5: no way to end a wire or delete a label | **Fixed.** |
| Run 2 #6: Esc in Route drops to Select | **Fixed.** One Esc keeps Route. |
| Run 2 #7: Update PCB wall of errors | **Mostly fixed** in the review ("16 parts staged", `q2.png`). The Issues panel still counted 69 "outside the outline" errors while parts sat staged (`r15.png`). |
| Run 2 #11: "12 pads unrouted" wording | **Remains.** Finding 9. |
| Run 2 #13: ⌘S on untitled | **Fixed.** Opens the panel proposing "USB sensor board". |
| Run 2 #14: no LED template | **Fixed.** The symbol shows emission arrows. |
| Precision: no context menus | **Fixed.** Esc aside (finding 6). |
| Precision: no copy and paste | **Fixed.** |
| Precision: Delete key | **Mostly fixed.** Wires remain (finding 7). |
| Precision: stale field error | **Fixed.** |
| Batch F1: ⌘S in a card field | **Fixed.** |
| Batch F2: wired power port | **Fixed.** |
| Batch F3: NC pin dead-end review | **Fixed.** |
| Batch F4: label on routed net | **Fixed.** |
| Batch F6: ⇧⌘U sometimes nothing | **Remains.** Finding 14. |
| Batch F5, F7, F8; N2–N13 | Not rechecked. |

## Comparison with runs 1 and 2

| | Run 1 (0f6b200) | Run 2 (6e2c238) | Run 3 (09556d6) |
|---|---|---|---|
| Duration | 50 min 3 s | 44 min 38 s, stopped by choice | 83 min 17 s, 73:58 to Export |
| USB connector | 1×4 header stand-in | 1×4 header stand-in | Real USB-C template |
| Parts | 15 min 46 s | 22 min 44 s | **6 min 34 s** |
| Schematic | 8 min 12 s, 13 ERC errors | 13 min 58 s, 7 ERC errors | 26 min 17 s with values, **0 ERC errors**, no free ends |
| Routing | 1 track, 25 airwires | 6 tracks + pour, 19 airwires | **117 segments, 12 vias, 2 pours, 0 airwires** |
| Production check | 23 errors, all waived | 17 errors, none waived | **0 errors, 0 waived**, 16 warnings |
| Export | 17 files, by waiving | Not reached | **17 files, no waivers** |
| Save and reopen | Reopened | Reopened intact | Two item-by-item rounds, no differences |

This is the first run to finish without waiving, and all of that run's blockers are gone. Parts became faster because pin paste no longer loses rows or needs Drive set by hand. Direct time comparisons are rough: this run scripted more of its clicks, and spent time on checks the earlier runs didn't do.

## What worked well

- **USB-C and LED templates:** cited, previewed and correct-looking in 2D and 3D (`ap1.png`).
- **The review cards** for labels and parts on routed nets are safe and specific, with no dead ends.
- **The issue lists** lead to a fix: Mark no-connect on the row, Show on review rows, full text on hover.
- **Routing feedback:** the "Routing USB_DP · 0.3 mm (W) · Bottom (2) · 45° (/) · Shove (M)" strip and the impedance row read well (`x7.png`).
- **Pours:** thermal reliefs, readable pads, and the airwire count falling straight to one.
- **3D:** opened within seconds, with a clean model, and the part cards stay available.
- **Save:** no lost value, track or setting in any path tried.

## Driver limits and agent errors (not product bugs)

- The native Save panel can't take typed text. Its proposed name was accepted, and the file was saved as `~/USB sensor board.pcbcraft`.
- The driver session ended when the app quit, so later checks used a second session label.
- Clicks sometimes needed the pointer moved first to register the pad under it, so route scripts hovered, then clicked. This wasn't seen as a product fault, but it wasn't separated from one either.
- Agent errors, included in the times: a helper-script bug during placement, clicks with no tool armed, and one click between two fields.

## Clean-up

The app was quit twice (the board with no unsaved changes, then the throwaway project with Don't Save), and no Pcbcraft process remains in the guest. The board file, production folder and temporary files were deleted in the guest; the Recovery folder is empty. `results/run3.png` and the file copies in `results/` were deleted. The VM was left running. The board was not committed.
