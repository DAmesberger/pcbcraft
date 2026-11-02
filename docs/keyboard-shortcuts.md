# Keyboard shortcuts and the command palette

Every command the desktop app runs by name lives in one registry (`apps/pcbcraft/src/commands.rs`). Each entry has an id, a title, a [shortcut context](../GLOSSARY.md), an optional default key and an action. The registry feeds four things:

- **Command palette.** ⌘P / Ctrl+P, or ⇧⌘P / Ctrl+Shift+P. It searches by title, keywords, context and id, and shows each command's key and context. With an empty query, recent commands come first. ↑↓ skip unavailable rows, Enter runs, Esc closes. An unavailable command is dimmed, and its second line says why.
- **Keyboard shortcuts sheet.** ⌘/ / Ctrl+/, or open it from the File menu. It lists every key, grouped by context, with the contexts in force listed first. The sheet has a filter.
- **Tooltips.** Ribbon buttons show their key, as in "Route (X)", in the same layout as Solidcraft's ribbon tooltip: the name, a key cap and the help line. App bar buttons show their key too.
- **The key dispatcher.** Each key runs the command it names in the active contexts.

A command runs the same event as its ribbon button. Document changes therefore still go through the editor's typed operation and the one engine. Keys used within a gesture, such as Enter closing a fill outline or V changing the routing layer, stay with the canvas that owns the gesture. The registry lists them as `InPlace`, so they appear in the sheet and are checked for conflicts. `Registry::with_key_table` adds a tool's `(key, description)` table, such as the Route tool's, for display.

A focused text field keeps plain keys and its own ⌘Z, ⇧⌘Z and ⌘B. An open menu also keeps plain keys. A tool mode overrides its view's keys while the tool is active. The board shape sketch replaces the Layout keys and keeps the global ones. A test (`the_default_keymap_has_no_conflicts`) rejects two commands that share a key where both could apply. Chords are named the same way as in Solidcraft's keymap (`Ctrl+Shift+S`). `Registry::rebind` stores the user's changes over the defaults, ready for a preferences page; user keymaps are not offered yet.

macOS shows ⌃⌥⇧⌘ glyphs. Other platforms show `Ctrl+Alt+Shift+K`. ⌘ below means Ctrl on Windows and Linux.

## Default keymap

The keymap is original. Letters are mnemonic where they can be (X route, V via, P pour/fill, B board shape, W wire), and Solidcraft's sketch keys are kept in the sketch-like tools.

| Context | Keys |
| --- | --- |
| Everywhere | ⌘P / ⇧⌘P palette · ⌘/ shortcuts · ⌘N new · ⌘O open · ⌘S save · ⇧⌘S save as · ⇧⌘E export to Solidcraft · ⌘Z undo · ⇧⌘Z redo · ⌘B Browser · ⇧⌘I Issues · ⇧⌘M Messages · ⌘1 / ⌘2 / ⌘3 Component / Schematic / PCB · F fit view |
| Component | ⇧⌘N new component · ⇧⌘O open component · ⌘[ / ⌘] previous / next component |
| Symbol tab | L line · ⇧L polyline · R rectangle · C circle · A arc · ⇧P polygon · ⇧T text · ⇧F field · N pin · ⌘R rotate · ⇧X mirror · ⌘D duplicate · Delete · ⇧R repeat tool · Enter finish · Esc cancel |
| Footprint tab | N pad · ⇧N pad row · H drill · ⇧H slot · L / A / C / R draw · D dimension · X construction ↔ silkscreen · Space turn pad · ⇧R repeat · Delete · Esc |
| Schematic | P place · W wire · ⌘R rotate · ⇧X mirror · Esc cancel |
| PCB (both views) | D Layout ↔ 3D · ⇧P refill all fills · ⇧⌘U update PCB from schematic |
| PCB Layout | X route · V via · P fill · B board shape · ⇧F view from bottom · H highlight complete net · Esc cancel |
| Route tool | V next layer (places a via while routing) · ⇧V previous layer · Esc end the route |
| Fill tool | A lines ↔ arcs · Enter close · Backspace take back a point · Esc drop the outline |
| Board shape | L line · A arc · C circle · R rectangle · F fillet · D dimension · U cutout · H hole · ⇧H slot · E name edge · X construction · Delete · Esc |

Layer and display toggles have palette commands but no default keys: each copper layer, vias, silkscreens, courtyards, assembly, holes, component bodies and airwires.

The Component workspace's Open moved from ⌘P to ⇧⌘O because ⌘P now opens the palette.
