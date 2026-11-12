# Keyboard shortcuts, the keymap and the command palette

Every keyboard shortcut in the desktop app resolves through one registry (`apps/pcbcraft/src/commands.rs`). Each entry has an id, a title, a [shortcut context](../GLOSSARY.md), an optional default key and an action. The registry feeds these surfaces:

- **Command palette.** ⌘P / Ctrl+P, or ⇧⌘P / Ctrl+Shift+P. It searches by title, keywords, context and id, and shows each command's key and context. With an empty query, recent commands come first and a line names the key reveal. ↑↓ skip unavailable rows, Enter runs, Esc closes. An unavailable command is dimmed, and its second line says why.
- **Menus.** The macOS menu bar and the in-window File menu show each item's key and run its command (below, under Menus).
- **Keyboard shortcuts sheet.** ⌘/ / Ctrl+/, or open it from the File menu or Help. Its first line names the key reveal. It lists every key, grouped by context, with the contexts in force listed first. The sheet has a filter.
- **Tooltips and cards.** Ribbon buttons show their key, as in "Route (R)", in the same layout as Solidcraft's ribbon tooltip: the name, a key cap and the help line. App bar buttons show their key too. Tool cards (Route, Tune, Fill, symbol and footprint tools) list their keys from the registry.
- **The key reveal.** Hold ⌘ for key badges, or tap ⌥ for ribbon key tips (below).
- **The key dispatcher.** Each key runs the command it names in the active contexts.

Every surface reads the *effective* keymap: the defaults with the user's changes from Settings. A rebinding therefore changes what the key does and every label that names it.

## One key path

A command runs the same event as its ribbon button, so document changes still go through the editor's typed operation and the one engine. A key that only means something inside a gesture stays with the canvas that owns it. Enter closing a fill outline and W changing the route width are examples. Such a key is still a registry command (`Handler::InPlace`). The editor declares it as a `KeyDecl` (id, default key cell, title) and reads it by id with `crate::keys::pressed(ctx, "route.width-next")`. It never names the physical key. The app publishes the effective keymap to the egui context (`keys::publish`) whenever it changes. The editors read it from there, and tests use the default keymap. Each direction of a cycle is its own bindable command, such as `route.width-next` (W) and `route.width-previous` (⇧W). The ten layer keys are ten registry commands of PCB › Layout, `pcb.layer-1` to `pcb.layer-10` (`pcb_editor::LAYER_KEYS`, `Action::PcbLayer`). The app's dispatcher runs them, so they are in the palette, Settings › Keyboard and the key reveal, and they work in every Layout tool. While routing they change layer through a via at the pointer, as V does. Otherwise they set the active layer that Route, Via and Fill use, with no via. A key past the board's stackup does nothing and says how many copper layers the board has. The palette lists only the layer commands of the board's own layers.

`cargo xtask raw-keys` (part of `cargo xtask ci`) parses the app and rejects raw key reads (`key_pressed`, `consume_key`, `consume_shortcut`, `key_down`, `key_released`, `num_presses`) outside the dispatch layer. The dispatch layer is `keys.rs`, `commands.rs`, `app/command_dispatch.rs`, `app/settings_window.rs` and `app/key_reveal.rs`. A counted, reasoned list allows the reads that are not command shortcuts. The check fails on a new site, on one more read in a listed file, and on a stale count. The kept exceptions are these:

- **Text fields.** Enter commits and Esc reverts the value being typed: form rows, rule values, net class, track, placement and pin fields, schematic labels and footprint dimensions.
- **Dialogs and lists.** Enter runs and Esc cancels a dialog. Arrows, Enter and Esc move through search sheets and the catalog. Tab moves between template gallery fields and pin-mux cells. ⌘D marks the highlighted Place palette row as a favourite.
- **Navigation.** A held Space pans the 3D view and the symbol and footprint canvases. Pointer-modifier navigation stays in `navigation.rs` and the viewport.
- **Gestures.** Esc ends a model triad drag.

Key handling that matches raw events instead of calling these methods is not caught. Examples are the model nudge arrows, the model library grid and the 3D view's arrow keys. These are navigation and placement gestures, not commands.

A focused text field keeps plain keys and its own ⌘Z, ⇧⌘Z and clipboard keys (⌘X, ⌘C, ⌘V, ⌘A: `edit.cut`, `edit.copy`, `edit.paste` and `edit.select-all`); off a field those four act on the schematic sheet's selection. The dispatcher also leaves it ⌘B; on macOS the View menu's ⌘B toggles the Browser over a field too. An open menu also keeps plain keys. A test (`the_default_keymap_has_no_conflicts`) rejects two commands that share a key where both could apply. A tool mode (Wire, Route, Fill, Tune, Board shape) may take a key from the contexts around it only when `commands::SHADOWS` lists it. Each entry names the mode, the key and the command it shadows, and a second test (`every_shadow_is_used`) rejects stale entries. The board shape sketch replaces the Layout keys entirely and keeps the global ones.

While the Route tool is armed, its keys shadow these:

- V shadows the Via tool: it places a via instead.
- F and ⇧F shadow Fit view and View from bottom: they flip the elbow instead.
- D shadows the Layout ↔ 3D switch: it cycles the via size instead.
- Esc shadows the Layout's Esc: it ends the route.
- P shadows the Fill tool: it switches pair routing off and on instead ([high-speed routing](high-speed-routing.md#differential-pair-routing)).
- M shadows Place 3D part: it cycles the router instead.

While the schematic's Wire tool is armed, F flips the elbow instead of fitting the view, A switches arc corners instead of rounding a picked wire corner, and Esc ends the wire in progress before it cancels the tool. R, Route's own key, has no meaning inside Route mode and arms Route again. While the Tune tool is armed, F flips the meander's side instead of fitting the view. The Fill tool's Esc drops its outline first. The board shape's F is Fillet.

**Quit** (`app.quit`, ⌘Q) is a registry command like the others, so it can be rebound. It makes the same request as the window's close button: unsaved work asks Save, Don't Save or Cancel first. Close Window (`window.close`, ⌘W) makes the same request, since Sparksmith has one window. egui's built-in quit key, which closed the window without asking, is turned off. Dock › Quit and logout still end the app without a prompt, keeping unsaved work for recovery.

## Menus

On macOS the menu bar (`native_menu.rs`, built with muda as in Solidcraft) replaces winit's default menu, whose Quit ended the app at once. Its items are listed in `menu_bar.rs`:

| Menu | Items |
|---|---|
| Sparksmith | About · Settings… ⌘, · Services · Hide, Hide Others, Show All · Quit ⌘Q |
| File | New Project ⌘N · Open… ⌘O · Save ⌘S · Save As… ⇧⌘S · Export to Solidcraft… ⇧⌘E · Export Schematic PDF… · Library Sources… · Close Window ⌘W |
| Edit | Undo ⌘Z · Redo ⇧⌘Z · Cut ⌘X · Copy ⌘C · Paste ⌘V · Delete · Select All ⌘A |
| View | Component ⌘1 · Schematic ⌘2 · PCB ⌘3 · Switch Layout and 3D · Fit View · Show/Hide Browser ⌘B · Show/Hide Issues ⇧⌘I · Command Palette… ⌘P |
| Window | Minimize, Zoom, Enter Full Screen · Messages… ⇧⌘M · Bring All to Front |
| Help | Keyboard Shortcuts ⌘/ |

Each item runs a registry command, and its key equivalent is that command's effective key. A rebinding moves it. An item is disabled while its command cannot run (Save while a file operation runs, Undo with nothing to undo). AppKit gives a key equivalent to the menu before the window, so these keys reach the app once, through the menu, also while a text field has the keyboard. A command chosen over a focused field waits until the field commits, as for its key. If a press of the same key also reaches egui in that frame, it is dropped.

Some keys stay with egui:

- An item takes its key only when the key is a ⌘ chord that the registry resolves to the item's command where the user is. A plain key (F for Fit View, Delete) is not a key equivalent, because the system menu would take it from text fields. A tool mode that takes a ⌘ chord for itself keeps it.
- While Settings › Keyboard records a key, no item takes one.
- Undo and Redo over a focused field, and Cut, Copy, Paste and Select All, are the field's own. The menu hands the field its input: ⌘Z, egui's copy or cut event, the clipboard's text, or ⌘A. The clipboard items are disabled when no field is focused, because the canvas has no clipboard yet. Delete hands the active canvas the key of its delete command (`pcb.delete`, `schematic.delete`, `footprint.delete` or `shape.delete`). None of these keys is one the dispatcher runs, so nothing runs twice.

The in-window **File** menu is drawn like Solidcraft's: Solidcraft's menu rows with icon, label and key, separators between groups and no prose. It lists the File items, then Settings…, Keyboard shortcuts…, Messages… and Quit. Each row runs its registry command and is enabled when the command can run. The Browser and palette are on the application bar.

Chords are named as in Solidcraft's keymap (`Ctrl+Shift+S`, where Ctrl is ⌘ on macOS). macOS shows ⌃⌥⇧⌘ glyphs, and other platforms show `Ctrl+Alt+Shift+K`. ⌘ below means Ctrl on Windows and Linux, and ⌥ means Alt.

## Changing keys: Settings › Keyboard

Settings opens with ⌘, / Ctrl+, or from the File menu. Its Keyboard section lists every command that can take a key, grouped by context. The filter searches name, key, context and id. Each list shows eight rows, then "Show all".

- **Record.** Click a command's key cap, then press the new chord. Esc cancels. The recorded key reaches nothing else.
- **Conflicts.** The registry's rules decide what counts as a conflict: the command's context, contexts nested inside it, and `SHADOWS`. A key already in use is never taken silently. The row names the command that holds it, with its context, and offers **Swap** or **Cancel**. Swap gives that command this one's old key as one change. It is refused, and nothing changes, if that key would clash with a third command.
- **Reset and Clear.** Reset returns one command to its default key, and offers Swap if another command now holds that default. Clear removes the key. **Reset all** returns to the default keymap.
- **Default or custom.** A changed command shows its name in the accent colour with a dot. The section header counts the changes.

Changes apply at once and are kept per user in `settings.json` (`pcbcraft_io::settings::KeyboardSettings`). Only the changed commands are stored, as `{ "command": "route.width-next", "chord": "Q" }`, where `null` means no key. The `format` field is versioned, and an unsupported version is refused. Bounds, printable text and duplicates are checked on read and write. At startup the saved changes are applied over the defaults (`keymap::apply`). Every changed command is freed first, so a saved swap applies in any order. An entry the app cannot apply is reported in Messages and at the top of the Keyboard section, and stays in the file. Examples are an unknown command id, an unreadable key name, a row that is not a key (`Click`) and a key another command holds. `read_keymap` and `write_keymap` read and write a keymap file that holds the changes alone. The Settings window has no import or export button yet.

## The key reveal

Both overlays can be turned off, and the hold delay set from 150 to 3000 ms, in Settings › Key reveal. The default delay is `HOLD_MS_DEFAULT`, 500 ms.

### Hold ⌘ for key badges

Hold ⌘ alone for the delay without pressing another key, clicking or scrolling. Key badges then appear on every visible control that runs a command: ribbon tools and tabs, the app bar's Save, Undo, Redo, Browser and palette buttons, and the navigation bar's Fit view. Each badge shows the key that fires *here*, resolved by the registry with its contexts and shadows. In Layout, Fit view shows F and Via shows V. While routing, both lose their badge, because Route's F and V take those keys. A rebound command shows its new key.

- A tool mode's own keys (Route, Tune, Fill) appear in a compact panel near the pointer, the same rows as its tool card.
- Keys in force that no control on screen shows appear in a small panel at the bottom left: Esc, ⇧F, H, ⌘N, ⌘O and so on, innermost context first. The panel holds at most 14 rows, then says how many more there are and names ⌘/. Its footer gives the canvas's navigation gestures.
- Releasing ⌘, pressing any key or clicking hides everything at once. The badges stay hidden until ⌘ is let go and held again.
- The reveal only watches the modifiers and consumes nothing, so ⌘ chords (⌘S, ⌘Z, ⌘P) and ⌘-drags on the canvas work as before. Controls register their rect as they draw (`reveal::register`) into a per-frame list that keeps its allocation. The overlay paints every badge and panel in one tooltip-order layer with Solidcraft's tokens and key-cap style.

### Tap ⌥ for key tips

Press and release ⌥ alone to enter key-tip mode, and tap it again to leave. Each ribbon tab and tool gets a tip of one or two letters. Typing a tip runs its command through the normal command path, so a disabled command refuses with its reason. A split button (Line ▾) opens a second level that lists its tools. When a crowded ribbon leaves a tab without a tip, the tabs move behind one **Views** entry, a second level. This happens in the Component Symbol and Footprint tabs. Backspace steps back a letter or a level. Esc, a click or a ⌘ chord leaves, and the chord still runs. Tips never open while a text field has the keyboard, where ⌥ types characters, and an open field closes them. ⌥ held with a click, a key, a scroll or another modifier is a gesture and not a tap, so ⌥-drags and Alias-style Shift+⌥ navigation are unaffected.

Tips are derived as follows (`reveal::assign_tips`):

1. A command with a single-letter shortcut in force here takes that letter as its tip, so ⌥ then V is the same as V.
2. Otherwise it takes its title's first letter while that letter is free. Tools choose before tabs. Titles of one or two letters ("3D") choose first, then ribbon order.
3. Otherwise it takes the initial of a later word, then a following consonant of the first word, while one is free. New sheet gets S, Refill gets F, and Production gets C.
4. Otherwise it takes two letters of its title. The candidates are its initials, then its first letter with each later letter, then pairs of later letters with consonants first.

Tips never use digits or letters the title lacks. Each tip is unique on its level, and no two-letter tip starts with a one-letter tip, so typing is never ambiguous. The same visible set always gets the same tips. A test checks every workspace's ribbon and its modes.

### Keys beside tool settings

A tool card's settings rows show the key that steps them as a chip at the right of the label (`kit::keyed`, reading the effective keymap, so a rebinding shows at once). The Route card shows M on Router, / on Mode, F on Flip elbow, W on Width, the active layer's digit on Layer (V past the tenth layer) and D on Via drill. While a route is in progress the pill above the canvas repeats them: "Routing GND · 0.3 mm (W) · Top (1) · 45° (/) · Shove (M)".

An armed drawing or placing tool shows a crosshair over the canvas (`kit::tool_cursor`), in Layout and in the schematic alike; Select keeps the arrow.

## Default keymap

The keymap is original. Letters are mnemonic where they can be: R route, V via, T tune, P place or fill (pour), B board shape, W wire, L label, H highlight. Solidcraft's sketch keys are kept in the sketch-like tools.

| Context | Keys |
| --- | --- |
| Everywhere | ⌘P / ⇧⌘P palette · ⌘/ shortcuts · ⌘, settings · ⌘Q quit · ⌘W close window · ⌘N new · ⌘O open · ⌘S save · ⇧⌘S save as · ⇧⌘E export to Solidcraft · ⌘Z undo · ⇧⌘Z redo · ⌘B Browser · ⇧⌘I Issues · ⇧⌘M Messages · ⌘1 / ⌘2 / ⌘3 Component / Schematic / PCB · ⇧⌘U update PCB from schematic (opens PCB) · F fit view |
| Component | ⇧⌘N new component · ⇧⌘O open component · ⌘[ / ⌘] previous / next component |
| Symbol tab | L line · ⇧L polyline · R rectangle · C circle · A arc · ⇧P polygon · ⇧T text · ⇧F field · N pin · ⌘R rotate · ⇧X mirror · ⌘D duplicate · Delete or Backspace · ⇧R repeat tool · Enter finish · Esc cancel |
| Footprint tab | N pad · ⇧N pad row · H drill · ⇧H slot · L / A / C / R draw · D dimension · X construction ↔ silkscreen · Space turn pad · ⇧R repeat · Delete or Backspace · Esc |
| Schematic | P place · W wire · N no connect · L net label · ⇧L global label · H hierarchical label · ⇧P power port · ⇧S sheet symbol · T note · ⌘R rotate (symbol or label) · ⇧X mirror · Delete removes the selected label, sheet symbol, drawing or wire (a picked wire corner goes first) · ⇧Delete or ⇧Backspace split the selected wire at the segment under the pointer · A round the picked wire corner by the Wire tool's radius, or make it sharp · Esc cancel |
| PCB (both views) | D Layout ↔ 3D · ⇧⌘L show or hide the Parts and vias band under the canvas · ⇧D dimension a part (one tool in both views; in 3D Esc and Delete are its own `pcb.dimension-cancel` and `pcb.dimension-delete`, in Layout Esc cancel and Delete) · ⇧P refill all fills |
| PCB Layout | R route · V via · T tune length · P fill · B board shape · M place 3D part ([mechanical parts](mechanical-parts.md)) · 1–9, 0 stackup layer 1–10 (the active layer; through a via while routing) · ⇧L layer focus (all layers, dim others, active only) · ⇧F view from bottom · H highlight complete net · ⌥-drag a differential pair's track to move that half alone (its partner stays; the canvas hint names ⌥ during a pair drag) · Esc cancel |
| PCB 3D | 1 front · 2 back · 3 left · 4 right · 5 top · 6 bottom · 7 home (isometric, fitted) · ⇧F fit and orbit around the selection · arrows turn the view 15° (⇧: 90°) · ⌘ arrows pan (arrows are navigation, read by the view, not rebindable) |
| Wire tool (`WIRE_KEYS`) | Click corner; a pin, label, junction, free end or wire ends the wire · double-click or Enter end here with a free end · Esc end here (with no corner: cancel; again: Select) · Backspace remove the last corner · F flip the elbow · A arc corners on or off (sticky) |
| Route tool (`ROUTE_KEYS`) | Click corner · Enter finish · Esc end (again: Select) · Backspace remove the last segment · V / ⇧V via to the next / previous layer · 1–0 (the Layout's layer keys) via to that layer · / next mode (45°, 90°, free) · F (or ⇧F) flip the elbow · W / ⇧W next / previous track width · D / ⇧D next / previous via size (⇧D instead of Dimension) · P pair routing on or off · M / ⇧M next / previous router (shove, walkaround, ignore; [push-and-shove](push-and-shove.md)) |
| Tune tool (`TUNE_KEYS`) | Click tune the segment · / trombone ↔ accordion · A / ⇧A amplitude up / down · S / ⇧S spacing wider / narrower · F flip the side |
| Fill tool (`FILL_KEYS`) | A lines ↔ arcs · Enter close · Backspace take back a point · Esc drop the outline |
| Board shape | L line · A arc · C circle · R rectangle · F fillet · D dimension · U cutout · H hole · ⇧H slot · E name edge · X construction · Delete or Backspace · Esc |
| Lists (`LIST_KEYS`, while a list has the keyboard) | ↑ / ↓ move and select · ⇧↑ / ⇧↓, ⇧Home / ⇧End extend from the anchor · ⌘↑ / ⌘↓ move the cursor only · Home / End, PgUp / PgDn jump · Space add or remove the row (on a group: fold) · ← / → fold, unfold, or go to the parent · Enter or F2 edit or locate · Esc cancel the edit, then clear the filter, then the selection · ⌘A select every shown row · / or ⌘F filter · ⌥↑ / ⌥↓ reorder (Dimensions) · Delete or Backspace delete the selected rows · letters and digits type ahead (700 ms) |

The layer focus (`pcb.layer-focus`, PCB › Layout) is a registry command run by the app's dispatcher, so it works in every Layout tool and shows in the palette, Settings › Keyboard and the key reveal. It cycles how the copper layers besides the active one show: all dimmed under it, strongly dimmed, or hidden ([PCB realism](pcb-realism.md#pcb-2d-appspcbcraft)). The view popover's Focus row sets it too, and the status line names it. It is view state and is not saved. Its default key is ⇧L (layer): ⇧S, the first choice, is the Tune tool's narrower spacing, and taking it would make the focus key do nothing while Tune is armed.

Layer and display toggles have palette commands but no default keys. They cover each copper layer a stackup can have (the palette lists the board's), vias, silkscreens, courtyards, assembly, holes, component bodies and airwires.

Route moved from X to R. The Component workspace's Open moved from ⌘P to ⇧⌘O, because ⌘P now opens the palette.

### Mnemonic audit (proposed, not applied)

These single keys are not mnemonic. Each proposal stays free of conflicts under the registry's rules, but none has been changed.

- **Footprint X: construction ↔ silkscreen.** X is borrowed from "cross out" and kept for Solidcraft's sketch construction key. Board shape uses the same X. It should stay X for Solidcraft parity.
- **Footprint H: drill, and ⇧H: slot.** H stands for "hole". Rename the commands "Hole" and "Slot hole" so the letter matches, or keep the titles.
- **Board shape U: cutout.** U has no link to the title. C is taken by Circle. O ("opening") is free in Board shape but is a key in Solidcraft's sketch keymap, which the sketch follows. Propose renaming the tool "Opening" and taking O if parity allows it; otherwise keep U.
- **Footprint N: pad.** N stands for "new pad" and matches the symbol tab's N for pin. P would be the mnemonic and is free in the Footprint tab, but it is a key in Solidcraft's sketch keymap, which the footprint sketch follows. Keep N unless parity with the symbol tab's pin key matters less.
- **Schematic H: hierarchical label.** It is acceptable as a mnemonic. ⇧H would pair it with the label family (L, ⇧L), but H alone is easier to press. Keep it.
- **PCB D: Layout ↔ 3D.** D stands for "3D". It is acceptable, but Route mode shadows it for via size. Keep it.
- **Symbol R: rectangle.** It does not clash with Route, which is in another workspace, and is mnemonic. Keep it.

Changing any of these keys would also change a key tip, because a tip equals a command's single-letter key.

## 3D navigation

The PCB 3D view has a navigation cube in its top-right corner. It follows Solidcraft's cube: click a face for that side's view, click a corner square for that corner's isometric view, or drag the cube to orbit. The house button above the cube goes home (isometric, fitted). The same views are the PCB 3D commands above (registry commands, rebindable in Settings and shown by the key reveal; 1–7 belong to PCB 3D and the layer keys 1–0 to Layout, sibling contexts that never see each other's keys), and the canvas hint at the bottom left names the trackpad gestures. The Keyboard shortcuts sheet lists every gesture under PCB › 3D (`navigation::NAVIGATION_3D`):

| Input | Trackpad | Mouse |
| --- | --- | --- |
| Orbit | Shift + two fingers | Right-drag, Shift + middle-drag, or ⌘-drag on empty space |
| Pan | Two fingers | Middle-drag, or Space + drag |
| Zoom toward the pointer | Pinch | Wheel |
| Fit | F | Middle double-click |

A plain left-drag on empty space does not orbit. In Solidcraft's Fusion preset, which Sparksmith follows, a left-drag on empty space is box selection. In PCB 3D the left button also selects and drives the move gizmo, so a bare drag that sometimes orbits and sometimes selects or moves would be ambiguous. ⌘-drag on empty space and the cube give a left-button orbit that cannot be confused with either.
