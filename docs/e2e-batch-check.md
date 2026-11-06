# Fix batch check in the real app

Checked on 6 November 2026 against the build of commit c64d273, in the Herdr macOS VM (1380 × 900 window), driven by the CLI. The test board was a copy of `examples/esp32-s31-devboard`. Screenshots named below are in `target/e2e-batch/` and are not committed. There were no crashes or hangs.

| Item | Result |
| --- | --- |
| 1. Schematic wires | **Partial**: two failures, F2 and F3 |
| 2. No-connect | Pass |
| 3. Menu bar | **Partial**: ⌘S with a focused field fails (F1) |
| 4. Delete and Backspace | **Partial**: a label on a routed net can't be deleted (F4) |
| 5. Browser | Pass |
| 6. Run 2 fixes | **Partial**: ⇧⌘U did nothing on 2 of 8 presses (F6) |
| 7. Minor fixes | **Partial**: palette not dimmed (F5), shrink message cut off (F7) |
| 8. Pin import | Pass, with a default-mapping problem (N1) |
| 9. Recovery | Pass |

## Failures

### F1. ⌘S in a card field doesn't save the new value (High)

I typed a new `width` and pressed ⌘S with the field still focused. The toast said "Saved captured version to …; newer edits remain unsaved", and the unsaved dot stayed.

- The saved file still held the old value (`"expr": "40 mm"`), read from the file in the guest. Waiting 2 s before ⌘S made no difference.
- A second ⌘S wrote `"43 mm"` and cleared the dot.
- The toast is honest, but a person who types a value and presses ⌘S expects it saved, and the toast is cut off before "remain unsaved".

Screenshots: `save1.png`, `msg2.png`, `s1c.png`, `s1t.png`, `s2t.png`.

### F2. A wire can't end on a power port that is already wired (High)

While drawing, these ports show no snap ring, and a click adds a corner instead of ending the wire. The wire carries on.

- Seen in all 6 attempts: U2's +3V3 (three clicks), C2's GND and C1's GND (one click each), and C2's +3V3 (hover only).
- Ending with Esc on the port leaves a wire that visibly touches the port but warns "wire end not connected".
- Free ports and labels with no wire work: the free GND port shows the ring and connects, and so does the free SIGNAL label.

Screenshots: `pp1c.png`, `pp2c.png`, `pp3c.png`, `pp4c.png`, `pp5c.png`, `wp1c.png`, `wp2c.png`, `wr2c.png`, `wr3.png`, `wr5c.png`; working cases `pw3c.png`, `pw4c.png`, `pw5c.png`.

### F3. Ending a wire on U2's NC pin opens a dead-end review (High)

The pin shows a snap ring and accepts the click. A "Draw wire" Review card then opens with "No changes" and "This update needs a decision before it can apply".

- No decision is offered, Apply stays disabled, and the footer "Fix the errors…" is cut off.
- Cancel is the only way out, and it throws away the whole wire. Reproduced twice.

Expected: either refuse the pin before the click, or say what is wrong and keep the drawing. Screenshots: `w12c.png`, `w13c.png`, `w14c.png`, `nc0c.png`, `nc1.png`.

### F4. A label on a routed net can't be deleted (Medium)

Backspace on the VBUS power port is refused with the toast "This connectivity change affects routed copper", and the label stays.

- Details opens Messages, which tells the person to use `project.preview_update` and `project.apply_update`. Those are internal command names, and the UI offers no way to do this from here.
- The selection is lost after the refusal.
- Deleting a free label works with Backspace and with Delete, and undo restores it. Delete and Backspace themselves work; only the routed case is blocked.

Screenshots: `l1c.png`, `l5c.png`, `dtc.png`; working case `l6c.png`, `l7c.png`, `l8c.png`.

### F5. Unavailable palette commands aren't dimmed (Medium)

They do come last, but they look the same as available ones. In the PCB Layout view, the darkest text pixel of "Back view", "Front view", "Left view" and "Select" (Schematic) measured 37–42, against 37–39 for available "Tune length" and "View from bottom". Screenshot: `ps3.png`.

### F6. ⇧⌘U from the schematic sometimes does nothing (Medium, cause unknown)

Six of 8 presses opened the Update PCB review correctly. The two that did nothing were the first two presses, and neither showed a visible reaction. All 6 after that worked, 5 of them with the same key delivery, so I can't say whether the app or the driver lost the first two. Screenshots: `up1.png`, `up4.png` (no effect); `up5k.png`, `uk1k.png`, `utr1.png`–`utr4.png` (worked).

### F7. The shrink message is cut off with no way to expand it (Low)

Shrinking the board does name the copper in the way, and it refuses the change. But the card shows "100 tracks on USB_DP, USB_DN and 19 more nets and 27 vias on USB_DN, GND and…". Hovering it showed no tooltip. The full sentence, with the "would lie outside 20 × 64 mm" reason, is only in Messages. Screenshots: `sh1.png`, `sh2c.png`, `m4c.png`.

### F8. A production group title breaks mid-word on hover (Low)

Group titles aren't cut off at rest; they wrap. On hover, "Waive all…" takes the room and the title becomes "Uncon / ected … 3". Screenshots: `prod1.png` (rest), `wa2.png` (hover).

## New problems

| # | Severity | Problem | Screenshot |
| --- | --- | --- | --- |
| N1 | Medium | The pin table's Type column maps to Function by default, so types become functions named "power input" unless the person remaps the column to Pin type. Once mapped, "ground" reads as "Power input power", and the table's Class column still says "Electrical" for every pin. | `dm3.png`, `paste9.png`, `paste10.png` |
| N2 | Medium | In the title bar, "File" and the disabled icons are dark on dark and almost invisible at rest. | `titlebar.png`, `s1t.png` |
| N3 | Medium | The component state chip ("new · never published", "published r1") is dark text on saturated blue. It is hard to read and looks like selected text. | `paste10.png`, `br2.png` |
| N4 | Low | Messages logged "Not available here; it applies in Everywhere", which contradicts itself. I didn't pin down which key caused it. | `m4c.png` |
| N5 | Low | Toast text runs up to the box edge with no padding. | `n7.png`, `w10.png` |
| N6 | Low | A T onto an existing wire reports "Drew the wire: adds 2 wires" for one drawn wire. | `dtc.png` |
| N7 | Low | The Keyboard shortcuts panel lists "Command palette" twice, for ⌘P and ⇧⌘P, with nothing to tell the rows apart. | `shortcuts.png` |
| N8 | Low | Edit › Delete is enabled with nothing selected. | `editc.png` |
| N9 | Low | Pin import's Preview stays disabled until Datasheet is filled in. Only a hover tooltip says so, and the field looks optional. | `paste3.png`, `dm2c.png` |
| N10 | Low | The ribbon has uneven gaps either side of Label ▾. | `e2c.png` |
| N11 | Low | In the PCB status line the ⇧ in "All layers (⇧ L)" is oversized, and the Route tool's line is cut off at "grid 0.5 …". | `a2.png`, `r1.png` |
| N12 | Low | MCU preview says "To review 5" for an 8-row table, because it counts the earlier rows. | `mcu3.png` |
| N13 | Low | A long Japanese name in the symbol preview overlaps the bottom edge of the symbol body. | `jp1.png` |

## Passes

### 1. Schematic wires

- Corners by clicking empty space: pass (`w4c.png`). The Wire card's Elbow row updates with F (`w4k.png`).
- Ending on a pin: pass on U4 OUT, with the snap ring first (`p1c.png`, `p2c.png`). F2 and F3 are the exceptions.
- Double-click, Esc and Enter each end a wire with a free end: pass (`p3c.png`, `w9c.png`, `p4c.png`).
- Esc with no corner cancels, and a second Esc returns to Select: pass (`e1c.png`, `e2c.png`).
- Backspace removes the last corner, and F flips the elbow: pass (`w4c.png`, `w5c.png`, `w6c.png`).
- A free end shows "Wire end at (167.5, -67.5) mm on Power and USB is not connected" under "Wire ends not connected": pass. The row is cut off and hover shows all of it (`h1c.png`).
- Crossing wires don't join, and a wire ending on another makes a T with a dot: pass (`t1c.png`). This also works on a wire of a routed net (`ta2.png`).
- Dragging a part: wires follow and stay horizontal and vertical. Pass on U4 and C2 (`d1c.png`, `dr2.png`).
- One undo removes a wire: pass (`w11c.png`).
- Hint line: both hints fit without cutting off. "Click adds a corner · a pin or wire ends · double-click or Esc ends here" reads fine, though "a pin or wire ends" is terse.

### 2. No-connect

- N arms the tool, and the hint explains it (`n1c.png`).
- A click marks a pin and a second click clears it (`n3c.png`, `n4c.png`).
- A click on the symbol body marks the undecided pins (`n5c.png`), and Issues then flags power inputs marked no-connect (`n5i.png`).
- U2's NC pin answers "That pin needs no mark: the part leaves it unconnected" (`n2c.png`).
- The Issues row action "Mark no-connect" works, with an Undo toast (`n6.png`, `n7.png`).
- The ✕ is neat and easy to tell from the open-pin circle, though thin and grey.

### 3. Menu bar

- All six menus appear with their shortcuts (`menuc-Pcbcraft.png`, `menuc-File.png`, `editc.png`, `menuc-View.png`, `menuc-Window.png`, `menuc-Help.png`).
- Undo and Redo grey out with nothing to undo (`editc.png`). With a field focused, Cut, Copy and Paste enable and Delete greys out (`me1c.png`).
- ⌘, opens Settings (`settings.png`).
- ⌘W and ⌘Q prompt with unsaved changes (`cmdw.png`, `cmdq.png`). ⌘W's dialog says "before quitting", which is true for the only window.
- ⌘A, ⌘C and ⌘V work in a field; the clipboard read back "40 mm" (`f3.png`).
- ⌘Z in a field undoes the text and leaves the board alone (`f1.png`, `f2.png`, `zfc.png`). ⌘Z outside a field undoes the board.
- Edit › Delete deleted the selected track (`trk.png`, `del1.png`).
- In-window File menu: neat, about 7/10. Grouping, alignment and shortcuts are clean. It loses points because Save, Save as and both exports share one disk icon, Shortcuts and Messages share one list icon, and its "File" title is hard to see (N2) (`infile.png`).

### 4. Delete and Backspace

Both keys delete a selected PCB track: the airwire count goes to 1 and the Board card returns (`a2.png`, `a3.png`). Schematic labels are under F4.

### 5. Browser

A single click selects with no workspace switch and no unsaved dot (`br1.png`). A double-click opens the component (`br2.png`).

### 6. Run 2 fixes

- Esc in Route after a finished route keeps Route, and a second Esc goes to Select (`r2t.png`, `r3t.png`, `r4t.png`).
- The Route card stays inside its panel; with Issues open its Keys section runs below the fold (`r1.png`).
- Tooltips stay inside the window, including at the right edge and bottom right (`tt1c.png`, `tt2c.png`, `h1c.png`, `wv4c.png`).

### 7. Minor fixes

- Label ▾ opens a clear menu, and the main part arms the last-used kind (`lb2c.png`, `lb3k.png`, `lb4k.png`).
- Settings, Shortcuts and Messages open one at a time, and Esc closes each (`settings.png`, `shortcuts.png`, `messages.png`, `pa1.png`–`pa4.png`).
- A long project name shows "…" and the full name on hover (`ln1z.png`, `ln1c.png`).
- 基板テスト renders correctly as a component name, as a project name, in the Browser and on the symbol (`jp1.png`, `jp2c.png`, `wr1.png`).
- Waived findings leave the badges. PCB's badge went from 1 to-do to none, the schematic's warnings from 12 to 11, and the check reads "3 errors · 22 warnings · 2 waived" (`wa1.png`, `wa2.png`, `wa3.png`).
- The palette has no duplicate rows in what I could check. It lists 60 rows per search ("56 more: refine the search"), and I compared the empty search and "e". "Select" appears twice in "e", once for PCB and once for Schematic, as two separate commands.

### 8. Pin import

- The 5-row LDO table imported all 5 rows, and the toast says "Imported 5 pin rows; confirm each under Unreviewed". It doesn't state outright that nothing was dropped, but nothing was (`paste10.png`).
- The MCU-like table imported 8 rows with 15 functions and no Combination errors (`mcu3.png`, `mcu4.png`). Its other errors are correct: this was an 8-pin table on a 5-pad SOT-23-5.

### 9. Recovery

Before quitting, the Recovery folder held `session-LqUGiW.pcbcraft-recovery`. After ⌘Q and Don't Save, the app had exited and the folder was empty.
