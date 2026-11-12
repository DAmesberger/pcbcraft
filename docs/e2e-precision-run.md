# Precision run: save and reopen, edge cases, menus

On 6 November 2026 an agent tested Sparksmith's desktop app for correctness and polish rather than speed. The run had three parts:

- an item-by-item check of save and reopen on a real board;
- a set of edge cases;
- a review of every menu and of how easy common tasks are to find.

It follows the [second UX run](e2e-ux-run-2.md), which did not cover these.

**Setup.** The build was ea6b95f (`component-model`), installed as `~/Applications/Pcbcraft.app` in the Herdr macOS VM: a 1920 × 1200 display at 1×, with a 1380 × 900 app window. It was driven through the guest CLI because the guest MCP server was disconnected. The board was a copy of `examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft`.

**Limits.** The agent read no product source, changed no product code and made no paid API calls. Saved files were copied to the host and compared as unzipped `project.json`, with a small script that diffs by object id. Screenshots (`p…`, `e…`, `m…`) are in the agent's `target/e2e-precision/` and were not committed. The run took about 75 minutes.

## Blockers

**No data was lost in any save path.** Four save paths were each followed by a reopen and a compare against the file on disk:

- ⌘S, then ⌘Q;
- File › Save;
- Save chosen in the quit prompt (twice);
- undo after a save, then ⌘S.

Every track, via, placement, lock, parameter, wire, label and waiver came back, with exact counts. The founder's remembered failure ("only one trace survived a save") **did not reproduce**: all 13 new track segments and the new via survived every round trip.

No other blockers were found. The most serious problems are about how easy things are to find and how the menus work; they are listed under Major below.

## 1. Save and reopen, item by item

### Inventory before any edit

| Item | UI | File |
|---|---|---|
| Tracks | Browser › Tracks **201** | 201 |
| Five named tracks | Track 384 USB_DP Top 0.3 mm (19.75, 7.355)→(19.75, 8.6); 385 USB_DP Top (19.75, 8.6)→(20.75, 8.6); 395 GND Top (20, 11.45)→(20, 10.5); 400 CC2 Top 0.25 mm (21.75, 7.355)→(21.75, 9.8); 403 GND Top (16, 12.15)→(16, 13) | same |
| Vias | — | 55 |
| Pours | 4: GND Inner 1, +3V3 Inner 2, GND Bottom, GND Top | same |
| Part spot checks | U1 (20, 49) 0°; J1 (20, 0); C1 (7, 11.5) 90°; R1 (6.6, 53.3) 270°; SW1 (4.5, 42) 90° | same |
| Board | 40 × 64 mm; parameters `width` 40 mm, `height` 64 mm, `corner_r` 1.5 mm; 4 holes | same |
| Sheet 1 "Power and USB" | 9 symbols, 30 wires, 30 labels, 34 nets (sheet card) | 9 occurrences, 30 labels |
| Sheet 2 "Module and IO" | 11 symbols, 103 wires, 103 labels | 11 occurrences, 103 labels; 133 wires in all |
| Nets | 75 | 75 |
| Waivers | none | none |
| Locks | none | none |

### Edits and the expected result

| Edit | How it was done | Expected after save |
|---|---|---|
| 5 new routes, one through a via | Route tool from pads of J2 (IO8, IO9) and J3 (IO46–IO49). Each route is two segments. The IO46 route used **V** to drop a via mid-route and ended on Inner 1. | +13 segments, +1 via |
| Delete 2 tracks | Track 384 from the Browser; one GND segment (633) picked on the canvas. Both were deleted with the card's **Delete** button, because the Delete key did nothing (finding 2). | −2 tracks, giving **214** |
| Move 3 parts | R2 X → 31 (typed); R4 Y → 5 (typed); D1 dragged with the mouse | R2 (31, 10), R4 (16, 5), D1 (29, 18) |
| Lock a part | C1 **Locked** box | C1 locked |
| Board parameter | `width` 40 mm → 42 mm | 42 × 64 mm |
| Schematic wire and label | A wire between the two GND ends of R4 and R5 on sheet 1, and a GND net label on it | Sheet 1: 32 wires, 31 labels |
| Waive a finding | Production › tick "J1 ↔ R4 courtyards overlap" › reason "Prec test waiver" › **Waive 1 finding** | 1 waiver with that reason |

### Results per save path

| # | Path | UI after reopen | File compared with the previous step |
|---|---|---|---|
| 1 | ⌘S, then ⌘Q, then reopen | Tracks **214**. Board 42 × 64. C1 shows "Locked in place". Sheet 1: 9 / 32 / 31. Sheet 2: 11 / 103 / 103. Production shows "1 waived". The title had no unsaved dot, and Quit did not ask. | Tracks +15 −2 (ids 644–659 added; 384 and 633 removed): **214**. Vias **56** (656 at (34.73, 19.73), IO46). Placements changed for exactly R4, C1, R2 and D1. Wires +2, labels +1. `board_size` [42, 64] and `width` = 42 mm. `production.waivers` holds 1 waiver with its reason. The four fills were recomputed for the new outline. Nothing else changed. **Match.** |
| 2 | Edit R5 Y → 14, then File › Save | Saved; the unsaved dot cleared | R5 at 14 written |
| 3 | Undo (R5 back to 11.5), then ⌘S; redo, then undo back to the saved state, then ⌘Q | After undo and ⌘S, no dot. Redo showed the dot; undo back to the saved state cleared it, and **Quit did not ask**. | The file is **byte-identical** to save 1. **Match.** |
| 4 | Edit, then ⌘Q › **Save** in the quit prompt | — | See the note below the table. |
| 5 | Move R5 Y → 14, add a 2-segment route, then ⌘Q › **Save** | Tracks **216**, R5 (24, 14) after reopen | Tracks +2 (663, 664) and R5 moved. Nothing else changed. **Match.** |
| 6 | Edit, then ⌘Q › **Don't Save** | — | The file is **byte-identical** to save 5. The Recovery folder is **empty**. |

Step 4 did not test what it set out to. On reopening, the Browser had *Components* expanded, not *Placements* (the expansion state differs from launch to launch). The agent's clicks therefore landed on component rows: they opened the Component workspace, created two component drafts and typed into a draft, instead of moving R5. The quit prompt's Save wrote those drafts correctly (10 draft items in the file), and the board was unchanged, as expected. Step 5 then repeated the test properly. The way a single click creates a draft is finding 3.

**Earlier attempt at step 1, recorded as finding 1:** the first ⌘S, pressed right after **Waive 1 finding**, did nothing. The title kept its dot, the file kept its old timestamp, and no message appeared. A second ⌘S, after a click on the canvas, saved the file.

## 2. Edge cases

| # | Case | Expected | What happened | Severity |
|---|---|---|---|---|
| E1 | Undo and redo across a routing session | One undo per route | Three routes: tracks 216 → 222. Three ⌘Z: 216. Three ⇧⌘Z: 222. | Pass |
| E2 | Delete a part that has wires and tracks (R5) | A review listing what goes | The Delete key did nothing. The card's **Remove…** opened "Remove R5: Removes R5, 4 wires and its PCB place…". It lists 8+ track conflicts ("Track on GND would join …"), each with its own "Remove that track" tick, and **Remove R5** stays disabled until every one is ticked. Safe, but long and cut off: the summary, every row and the footer ("Confirm each bl…") are truncated (`e09-remove.png`). Cancel left the document unchanged. | Major (Delete key); Minor (truncation) |
| E3 | Move a locked part (C1) | Refused by mouse, arrow keys and typed position | Arrow keys and drag were refused, with the toast "C1 is locked; unlock it to move it". The position fields are greyed and cannot be typed into. C1 stayed at (7, 11.5) (`e12.png`). Rotate 90° and Flip are disabled. | Pass |
| E4 | Board parameter that puts copper off the board | Either allowed with DRC errors, or refused with a clear reason | `width` 20 mm or 37 mm was **refused**. The field went back to 42 mm with a red note: "via copper must lie inside the board outline" or "track copper lies outside board outline" (`e13`, `e14`). This is safe, but a designer who wants a smaller board has to delete copper first, and the note does not say which copper. | Minor |
| E5 | Invalid field values (`width`) | Clear rejection; document unchanged | `-5 mm` and `1000000 mm`: "That value is not accepted", value kept. Empty: "Type an expression". `5 kg`: "unknown parameter `kg`". `abc`: "unknown parameter `abc`". The document never changed. **Two problems.** (a) The field note is vague, while the Messages log says more: "`width` = 1000000 is out of range" and, for −5 mm, "Constraints k1, k2, … k26 cannot be satisfied", which is internal jargon. (b) After a later valid entry of 42 mm, the field **stayed red with the stale "unknown parameter `abc`"** (`e18.png`). | Minor |
| E6 | Long Unicode name | Shown in full or with a hover | The project name "Sensor-Board Ω µ 日本語 Ünïcødé …" was accepted. The Browser shortens it with "…" as expected. **CJK characters draw as empty boxes** ("□□□") in the Browser, the Board card and the title block, because the UI font has no CJK glyphs (`e17-long.png`). Hover was not tested. | Minor |
| E6b | Hover over shortened long names. Project: "ESP32-S31 sensor development board with a deliberately very long project name". File name: 170 characters. Part: ESP32-S31-WROOM-3. | The full text appears on hover wherever a name is shortened | **Browser root row** ("ESP32-S31 sensor development…"): **yes**, a tooltip shows the full name. **Browser part row** ("Espressif ESP32-S31-…"): **yes**, "Espressif ESP32-S31-WROOM-3 · Manufacturer part". **Card title** ("ESP32-S31-WR…"): **yes**. **Tab title**: a 105-character name fits; a 170-character name **runs off the right edge of the window with no ellipsis**, and hover **shows the full path**. **Board card's Project field** ("ESP32-S31 sensor de"): **no**; it is cut mid-word with no ellipsis and has no tooltip. **Component card's Parts row** ("ESP32-S31-WROOM-3 · Espressif…"): **no**; hover shows only "Open this Part". The check also reproduced finding 3: the click that selected the part created a draft. | Minor (Project field, Parts row, title with no ellipsis) |
| E7 | Rotate and mirror a part with wires attached (R4) | Wires stay attached | ⌘R and Shift+X both kept the wires attached. The value text moved to the other side. | Pass |
| E8 | Schematic change on a routed board | Review of the copper impact | A wire from CC2 to CC1 opened "Connect pins: adds 1 wire, removes 1 net, changes 1 …". It lists the tracks that would be left on the wrong net, each with "Remove that track", and **Apply update** is disabled until they are confirmed (`e21-merge.png`). Cancel left the board unchanged. Rows are cut off ("Track on CC2 would be lef…"). | Pass (truncation Minor) |
| E9 | Delete a net label | Delete key removes it | Delete and Backspace did nothing. The card's **Remove** removed it (`e06.png`). This is the second run's finding 5, still open. | Major (with E2) |
| E10 | Copy and paste (R5 in the schematic) | ⌘C / ⌘V duplicates the part | ⌘V did nothing visible. Still 20 instances, and no message. The palette has no Copy or Paste command. | Major |
| E11 | Two documents | Second window, or a prompt before replacing | `open` of a second file while the app had unsaved changes showed "Save changes before continuing?" (Cancel / Don't Save / Save). Don't Save opened the second file in the same window. Sparksmith is single-document, and this is handled safely. | Pass |
| E12 | Close the window or quit | Same prompt | The red close button showed "Save changes before quitting?", the same as ⌘Q. Cancel kept the window and its edits. | Pass |
| E13 | Don't Save, then the Recovery folder | Empty | Empty, and the file was unchanged. | Pass |
| E14 | Force-kill (`kill -9`) with unsaved edits, then relaunch | Offer recovery | The relaunch showed "Recover an earlier session?" with the name, the time ("just now"), 20 instances and the original path. It offers Discard / Keep for later / Restore. Restore brought back the unsaved route (tracks 218) and a toast: "Recovered …; save it to keep it" (`e29`, `e30`). **But ⌘S then opened the Save panel proposing "untitled" in the home folder** (`e32-savepanel.png`), not the board's name or its original folder `~/prec`. | Pass (Minor: save defaults) |
| E15 | Waived finding in the Issues panel | Shown as waived, or hidden | Production shows "6 errors · 6 warnings · 1 waived" with a green tick on the row. The **Issues panel and the toolbar badge still count the waived courtyard overlap as an error**, also after reopening (`p41`, `m11`). | Minor |
| E16 | Workspace shortcuts with a text field focused | ⌘1/⌘2/⌘3 switch | ⌘2 and ⌘3 did nothing while a card field had focus. They worked after a click on the canvas. | Minor (same cause as finding 1) |

## 3. Menus and findability

### Menu verdicts

| Menu | Verdict | What looks off | What a polished Mac app does |
|---|---|---|---|
| macOS app menu (`m03-appmenu.png`) | **Incomplete** | It has only About, Services, Hide (⌘H), Hide Others (⌥⌘H), Show All and Quit (⌘Q). There is **no Settings… (⌘,)**, and ⌘, does nothing. The system menu bar has **no File, Edit, View, Window or Help** menus. | Settings… ⌘, in the app menu, and a full menu bar: File, Edit (Undo, Redo, Cut, Copy, Paste, Delete, Select All), View, Window and Help, each item showing its shortcut. |
| In-window **File** menu (`m02-file.png`) | **Poor: the "ugly menus" are still there** | The "File" label in the title bar is dim grey on dark and almost invisible beside the save icon. Each item is drawn as a **separately bordered full-width button**, about 33 px apart, so the menu looks like a form. **No shortcuts are shown**, although ⌘N, ⌘O, ⌘S, ⇧⌘S and ⇧⌘E exist. A help sentence ("Export writes a native, parametric .solidcraft model of the board.") sits between items. The grouping mixes File items with View items (Hide Browser), Window items (Messages…, Command palette…, Keyboard shortcuts…) and Settings…. The 590 px menu covers the ribbon. | Borderless rows with hover highlight, shortcuts right-aligned in grey, thin separators between groups, about 22 px rows, a 240–280 px width, and no prose. Move Hide Browser to View, and Messages and the palette to View or Window. |
| Title-bar icons | Mixed | Save, undo, redo, a "list" icon and a search icon. The list icon toggles the Browser (`m04-list.png`), which its glyph does not suggest. | A sidebar glyph for the Browser toggle, and tooltips with shortcuts. |
| Workspace switcher | Good | It lists Component, Schematic and PCB, and switching works. | — |
| **Label ▾** (`m01-label-menu.png`) | Misleading | The ▾ suggests a menu, but a click arms the Net label tool at once. Global and Hierarchical are chosen in the card's *Type* field. | Either a real menu (Net, Global, Hierarchical, plus No-connect when it exists), or drop the ▾. |
| **Context menus** (right-click) | **Missing** | Right-click on a schematic part, wire, label and empty canvas, and on a PCB part, showed **no menu** (`m07`–`m11`). Pins and tracks were not tried separately. | A context menu per object: Rotate, Mirror, Delete, Copy, Lock, Properties; for a track, Delete track / Delete segment / Select net. |
| Command palette (`m05-palette.png`) | **Good, with two flaws** | It is clean: search field with an Esc chip, a hint line, and rows with icon, name, context line and shortcut chip. **"Command palette" appears twice** (⌘P and ⇧⌘P). Commands not available here are listed among the available ones; "Delete" shows only "Not available here; it applies in Component › …" (`m06-pal-del.png`). | Merge the two duplicates, rank available commands first, and grey or hide unavailable ones. |
| Settings (`m13-settings.png`) | Fair | It opens as a card over the canvas, and its title icon is an *fx* glyph. The last visible row ("Command palette ⌘P …") is **cut in half** by the card's bottom edge. **Esc does not close it**, and opening Keyboard shortcuts and then Messages **stacks three cards** on top of each other (`m15-messages.png`). | A separate Settings window (⌘,) with a gear icon. Esc closes; one panel at a time. |
| Keyboard shortcuts (`m14-keys.png`) | Good | Clear key chips and a filter. It has the duplicate Command palette row, and its last row is clipped. | — |
| Messages | Good | Timestamps, icons, Clear and Copy all. Some text is internal jargon ("Constraints k1 … k26 cannot be satisfied"). | Plain language: "Width −5 mm is not possible: the board would be negative." |
| Quit and close prompt (`p59`) | Fair | A clear title with Cancel / Don't Save / Save, and Save is the default. It is an in-canvas card, not a macOS sheet. The text "The project or component draft has unsaved changes." is generic. | A sheet attached to the window, naming the file: "Do you want to save the changes you made to "board"?" |
| Issues and Production lists | Fair | Rows and group titles are still cut off: "R2 pad 1 ↔ track on LED_…", "Envel… 5  Waive all…". | Wrap the text, or show the full text on hover. |

### Tasks not found within about 10 seconds

| Task | Where the agent looked | Where it is |
|---|---|---|
| Delete a selected track, part or label | Delete and Backspace keys, right-click, palette ("delete": not available here) | The card's **Delete** or **Remove…** button, at the bottom of the right card |
| Settings | App menu, ⌘, | In-window File › Settings… |
| Copy and paste a part | ⌘C/⌘V, palette, right-click | Not found |
| An Edit menu (Undo/Redo) | System menu bar | Only ⌘Z and the title-bar arrows |
| Save | File menu (the label is hard to see) | ⌘S works when no field has focus; the title-bar disk icon |

## Findings by severity

Estimates are agent hours.

### Major

| # | Finding | Fix | Estimate |
|---|---|---|---|
| 1 | **⌘S (and ⌘1–⌘3) is silently swallowed while a card field has focus.** After typing a waiver reason and clicking **Waive 1 finding**, ⌘S did not save, and the dot stayed. Someone who saves and quits by reflex is protected by the quit prompt, but the shortcut gives no feedback. | Treat ⌘-chords as app commands even when a text field has focus (commit the field first). | 1–2 h |
| 2 | **The Delete and Backspace keys do nothing** for selected tracks, parts and labels, in both PCB and Schematic. The palette says Delete is available only in Component. | Bind Delete and Backspace to delete the selection in every workspace, through the same reviews the card buttons use. | 2 h |
| 3 | **A single click on a component in the Browser switches to the Component workspace and creates a draft** ("Draft created · r2 on publish"). That marks the project unsaved and writes drafts on save. Undo removes the draft. | Selecting shows the component read-only; a draft is created only on the first edit. | 2 h |
| 4 | **No context menus anywhere.** | Add right-click menus for parts, wires, labels, tracks, vias and empty canvas, backed by the command registry. | 3–4 h |
| 5 | **The in-window File menu looks like a form** (bordered buttons, no shortcuts, mixed groups). There is no Settings… ⌘, and no Edit/View/Window/Help in the macOS menu bar. | Restyle it as a menu with shortcuts and separators. Add a native menu bar populated from the command registry. | 4–6 h |
| 6 | **Copy and paste are missing** (⌘C/⌘V do nothing, with no message). | Add them, or say "Copy is not available yet" in a toast. | 3–4 h |

### Minor

| # | Finding | Estimate |
|---|---|---|
| 7 | A stale field error stays red after a valid value is entered (E5). | 30 min |
| 8 | The field error is vague ("That value is not accepted") while the log has the reason; the log shows constraint ids (E5). | 1 h |
| 9 | CJK characters draw as boxes; add a fallback font (E6). | 1 h |
| 10 | A waived finding still counts in the Issues panel and the badge (E15). | 1 h |
| 11 | After Restore, Save proposes "untitled" in the home folder instead of the board's name and original folder (E14). | 1 h |
| 12 | Settings, Shortcuts and Messages cards stack; Esc does not close them; their bottom rows are clipped. | 1 h |
| 13 | Duplicate "Command palette" entry; unavailable commands mixed in with available ones. | 1 h |
| 14 | Label ▾ suggests a menu but arms the tool. | 30 min |
| 15 | Rows are still cut off in the Remove review, Connect pins review, Issues and Production. Founder's bar. | 1–2 h |
| 16 | Shrinking the board is refused outright when any copper would be outside, without saying which copper (E4). | 1 h |
| 17 | The Browser's expansion state differs from launch to launch, so the same click lands on a different row. | 1 h |

## Driver limits (not product bugs)

- The native Save panel did not take typed text, so the save-panel path was only opened and cancelled.
- Desktop-scope clicks on the macOS menu bar returned "unverifiable" but worked.
- Right-click was sent as a window-scoped right button. That a menu appears nowhere is consistent across six targets, so it is unlikely to be a driver fault, but a manual check would confirm it.

## Clean-up

The app was quit with Don't Save, and the Recovery folder was empty. `~/prec` (the board copies) was deleted in the guest. The host copies in `results/` and `results/prec.png` were deleted. The VM was left running.
