# Schematic wires as paths: design

**Status:** slice 1 (1a path model, 1b Wire tool), slice 2 (rubber-band on move) and slice 3 (path editing) built on 6 November 2026, and slice 4 (arc corners) on 8 November 2026; slice 5 is not. See "Slice 1 as built" to "Slice 4 as built" at the end for where the build differs from this plan. The founder decided that a wire is one connected line made of straight and arc segments, like an SVG path, and that it must be easy to edit. This document plans that change. Items marked **Open question** need the founder's decision before the slice that depends on them starts. Estimates are agent hours.

Related: [schematic capture](schematic-capture.md) (today's wiring rules), [keyboard shortcuts](keyboard-shortcuts.md) (Route and Fill tool keys), [push-and-shove](push-and-shove.md) (the planned track drag). It follows the same independent-implementation policy as the rest of Pcbcraft: no other EDA tool's source was read.

## Today

- `model::Wire { id, from, to }` joins two connection points: a terminal, a junction, a label or a sheet pin. It is drawn as the horizontal-first L between them (`wiring::wire_route`). The bend is implicit and is not stored.
- `schematic.connect` stores a junction at each bend when it has to route around a point that is in its way (the vertical-first L, or a dogleg in `clean_route`). A bend is therefore sometimes a junction with two wires. The shipped examples store no junction ends.
- `schematic::tidy` splits a wire wherever a connection point lies on it (a T), and removes junctions left with fewer than two wires, together with their stubs. Free ends cannot exist.
- When a part moves, its wires stay orthogonal because each one is redrawn as an L. That stops working once corners are stored, so rubber-banding (below) is part of this work.

## 1. Model

### A wire is a path

```rust
pub struct Wire {
    pub id: Id,
    pub sheet: SheetId,          // stored: a path with two free ends has no object to take its sheet from
    pub from: WireEnd,
    pub to: WireEnd,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corners: Vec<Corner>,    // interior vertices, in order from `from`; at most MAX_CORNERS (256)
}

pub struct Corner {
    pub at: Point,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub radius: f64,             // 0: a sharp corner; > 0: a tangent arc of this radius rounds it
}
```

- A path's vertices are `from`'s position, then the corners, then `to`'s position. Each pair of neighbouring vertices is a straight segment. A corner with a radius is drawn as the arc that is tangent to both of its segments. The vertex stays the stored point, so dragging and rubber-banding work on vertices only, and the arc follows.
- **Open question 1, arc form.** The proposal is a rounded corner (a fillet radius at a vertex). It covers "change a corner into an arc and back, and its radius", keeps paths orthogonal, and keeps one editing model. The alternative is a free arc segment through a point (bulge form, like the Fill tool and `library::Segment::Arc`). That allows S-curves, but every edit, the rubber-banding and every command would then have to handle two kinds of segment. I recommend the fillet, and adding free arcs later only if they are needed.
- An attached end's position is never stored; it is read from its object (pin, label, sheet pin or junction), as today. Corners are stored in sheet millimetres.

### Ends

`WireEnd` gains one variant: `Free(FreeEnd { free: Point })`, written as `{"free": [x, y]}`. `WireEnd` is a map key throughout core, so it must stay `Eq`/`Ord`/`Hash`. `FreeEnd` therefore holds the point as integer nanometres and serialises it in millimetres. A free end connects to nothing, and connectivity code skips it.

### Connectivity

- A path is one conductor that joins its two ends. A corner is not a connection point, so nothing joins a corner except by ending on it.
- **A path end on another path's segment or corner** makes a T. `tidy` splits the other path there at a new junction, as it does today. The junction is drawn with a dot.
- **A junction exists only** where a person places it (J tool, `junction.place`) or where a T is made. Junctions never stand in for bends.
- **Crossings never connect**, and neither does a corner that lies on another path.
- **A connection point lying inside a path** (a pin end, label or sheet pin under a segment) connects to it, as today: a T. While drawing, the preview marks each such point with the snap ring, so the join is visible before the click.

`tidy` normalises after every edit, replacing today's `remove_stubs`:

| At an unplaced junction | Result |
| --- | --- |
| 3 or more path ends | Stays, with a dot. |
| 2 path ends | The two paths merge into one; the junction becomes a corner (dropped if the two segments are in line). |
| 1 path end | **Open question 2.** Proposal: the end becomes free and ERC warns. Today the stub is deleted silently. |
| none | Removed. |

Two free ends of different paths at the same point merge into one path (**Open question 3**; proposed yes, because ends that meet are connected). A placed junction stays while it has a path end on it.

### File format

The project format has one version and no migrations. The `Wire` record changes as shown above. There is no converter: the examples are rebuilt from their replays (`examples/esp32-s31-devboard/regenerate.sh`; the module example and the recordings in `examples/qwen3.8-flash-next` and `examples/tlv75733-local` are replayed and checked the same way). After the change, `schematic.connect` writes the horizontal-first L as one path with one corner, and writes its avoidance bends as corners rather than junctions. `examples/esp32-s31-devboard/nets.json` must come out identical: the nets must not change.

## 2. Ergonomics

The keys follow the Route and Fill tools, and handle editing follows the Fill outline (`fill_tool.rs`). Every key is a registry command (`WIRE_KEYS`). A key a mode shadows is listed in `commands::SHADOWS`, so it is rebindable and shown by the key reveal.

### Snapping (every wire gesture)

- The pointer snaps to the symbol grid (`drafts.grid.symbol_mm`, 2.5 mm).
- Within 12 px it snaps instead to a connection point (pin end, junction, label, sheet pin), to a free end, or to a point on a path (the nearest grid point on it when there is one). A pin end wins a tie. The landing gets today's accent ring and dot.
- A point on a path's corner counts as on the path.

### Drawing (Wire tool, W)

| Input | Effect |
| --- | --- |
| Click a pin, label, sheet pin, junction or free end | Starts the wire there. Starting on a free end continues that path instead of starting a new one. |
| Click a path's middle | Starts with a T there. |
| Click empty space | Starts with a free end (**Open question 4**; proposed yes). |
| Move | Preview from the last point to the pointer as a 90° elbow, horizontal first. Points it would run over and join show the snap ring. |
| Click empty space | Fixes the elbow: its bend and the click point become corners, and drawing continues from the click. |
| Click a pin, label, sheet pin, junction, free end or path | Ends the wire there and commits. A path end makes a T, and a free end merges the paths. |
| Double-click empty space | Ends the wire there with a free end and commits. |
| Enter | Ends the wire at the last fixed point (free) and commits. |
| Esc | With at least one segment fixed, ends the wire there like Enter. With none, cancels. With nothing in progress, does what Esc does today (Select). |
| Backspace | Removes the last fixed point and its elbow. With none left, cancels. |
| F | Flips the elbow (vertical first). Shadows Fit view, as in Route. |
| / | Cycles the posture: 90°, free angle (**Open question 5**: add 45° like Route? Proposed: 90° and free only). |
| A | Arc corners on or off; sticky, like the Fill tool's A. New corners get the card's radius (default 2.5 mm). |

- **Hints** (one line, short enough not to truncate):
  - idle: "Click a pin, wire or empty space to start"
  - drawing: "Click to add a corner · click a pin or wire to end · double-click or Esc ends here"
- **Card**: Wire tool, with rows Start (target name), Posture (/), Arc (A) and Radius.
- **Undo:** the finished wire is one step (`wire.draw`). Nothing is committed while drawing.

### Selecting and hovering (Select tool)

- Hovering a path highlights the whole path thinly, and the segment under the pointer a little stronger.
- A click selects the path (**Open question 6**; proposed: the whole path, not one segment). The card shows its net, its ends ("U1 pin 3 → free end"), its corner count and, for a selected corner, Sharp/Round and Radius.
- A selected path shows handles: a square at each corner and at each free end, and a dot at each segment's midpoint. An arc corner also shows a dot at the middle of its arc.

### Editing a selected path

| Gesture | Effect | Undo |
| --- | --- | --- |
| Drag a corner | Moves it. In 90° posture both neighbouring segments keep their direction: the horizontal one moves by the vertical offset and the vertical one by the horizontal offset, dragging their far vertices along. A far vertex that cannot move (an attached end) gets a new corner, so the segment steps instead of slanting. ⌥ drags freely. Esc during the drag restores it. | one per release |
| Drag a segment (anywhere on it) | Moves it along its normal. Its neighbours stretch. Where there is no neighbour (the segment ends on an attached end), a short perpendicular segment is inserted. | one |
| Double-click a segment | Inserts a sharp corner at the snapped point, without changing the shape. Drag it afterwards to make a step. | one |
| Double-click a corner, or select it and press Delete | Removes the corner and merges its two segments. The merged segment may slant; that is what was asked for. | one |
| A with a corner selected | Switches the corner between sharp and round (card radius). | one |
| Drag an arc's middle dot | Sets the radius, limited to what fits both segments. The card's Radius field sets it too. | one |
| ⇧Delete on the hovered segment | Removes that segment: the path splits into two, with free ends at the gap. This is how a path is split. | one |
| Drag a free end | Moves it (rubber-banding as for a corner). Dropped on a pin, label, sheet pin, junction or path, it attaches there. Dropped on another free end, the two paths join into one. | one |
| W, then click a free end | Extends the path from that end (see Drawing). | one per finished extension |
| Delete | Removes the whole selected path. | one |

### Moving parts, labels and sheet symbols

When a pin, label or sheet pin moves, each path attached to it follows by rubber-banding, in the same undo step as the move:

1. The first segment keeps its direction. The next corner slides along the second segment so that the first segment stays horizontal or vertical.
2. A path with no corner (one straight segment) gets one corner, which makes a horizontal-first L.
3. A segment that would shrink to nothing is dropped, and its corners merge.

The drag preview and the commit use the same core function, so what you see is what is stored. This is the same rule as the planned PCB track drag ([push-and-shove](push-and-shove.md#track-drag-with-shove-planned)): neighbours keep their directions, and preview and commit share one pure plan.

## 3. Commands

All of these are `SchematicCommand`s in the Edit class, shared by the UI, CLI and MCP. Each is one undo step and is refused as a whole. The rubber-band and corner rules live in core (`schematic::wire_edit`, pure functions), so the UI previews exactly what the command stores.

| Command | Parameters | Notes |
| --- | --- | --- |
| `schematic.connect` | `from`, `to` (targets) | Unchanged. The engine picks the path: an L, or a detour around points in its way. Bends are stored as corners. **The LLM default.** |
| `wire.draw` | `sheet?`, `from`, `corners: [{at, radius?}]`, `to` | `from` and `to` are a target or `{"free": [x, y]}`. An empty `corners` list means a straight segment. The Wire tool sends this. |
| `wire.set_path` | `wire`, `corners`, `from?`, `to?` | Replaces the geometry. Re-attaching an end is allowed. The general form behind the handle edits. |
| `wire.move_corner` | `wire`, `index`, `to`, `free?` | Rubber-bands unless `free`. |
| `wire.move_segment` | `wire`, `index`, `offset` | Segment `index` runs from vertex `index` to `index + 1`. |
| `wire.insert_corner` / `wire.remove_corner` | `wire`, `at` / `wire`, `index` | |
| `wire.set_radius` | `wire`, `index`, `radius` | 0 makes the corner sharp. |
| `wire.remove_segment` | `wire`, `index` | Splits the path. |
| `wire.move_end` | `wire`, `end: from \| to`, `to` | Moves or re-attaches an end, or joins two free ends. |
| `schematic.disconnect` | `wire` | Removes the path. Kept for the existing replays. |

**For an LLM, especially one with a small context:** name things and leave the geometry to the engine ([LLM geometry](llm-geometry.md)). `schematic.connect {from: U1.3, to: R2.1}` needs no coordinates. Paths come back in one compact line each, for example `W12 U1.3 > 40,20 > 40,35r2.5 > free 60,35`. Indices count corners from `from`, so `wire.move_segment {wire: 12, index: 1, offset: -5}` is enough to tidy a drawing. A refusal names the point in the way, as `connect` does today.

## 4. Rendering and checks

- **Scene, canvas and PDF:** `schematic::scene` replaces `wires: Vec<[Point; 3]>` with one path per wire, as a polyline with arcs flattened to the PDF's curve tolerance (or as true arcs where the PDF writer has them). The canvas, `schematic.scene` and the PDF all draw from it. A free end is drawn as a plain line end. Dots come from `junction_dots`, which is unchanged.
- **Hit testing:** the distance to each segment and arc (`WIRE_HIT_PX` = 6 px), then handles within 8 px (the Fill tool's reach). Wires on the selected sheet are indexed by bounds, as the line index does today, so hover stays cheap on large sheets.
- **ERC:** new code `erc.wire_end_free`, a warning: "Wire end not connected", with the fix "Connect the end or remove the wire". It references a new `ObjectRef::Wire { id }`, so Locate selects the path and frames the end. It is added to `KNOWN_CODES`, the group title "Wire ends not connected" and the ERC table in [schematic capture](schematic-capture.md#erc).
- **Tests:** about 17 test files count wires or junctions. A run of today's wires through bend junctions becomes one path with corners, so those counts change. Each changed count is the new model, not a weaker check, and each change says so. The net assertions (`esp32_nets`, `touching`, `junctions`) must not change. The rubber-band, `wire_edit` and tidy rules get table-driven unit tests. The tool gets synthetic-input tests in the style of `junction_tests.rs`: corners, ending on a pin, a T on a wire, a double-click free end surviving save, load and tidy, Esc keeping segments, Esc with no corner cancelling, Backspace, one undo removing the wire, and crossings not joining.

## 5. Slices

Each slice is mergeable alone and keeps `cargo xtask ci` green. Slices 1a and 1b can run in parallel once the 1a types are agreed.

| Slice | Content | Agent hours |
| --- | --- | --- |
| **1a. Path model (core)** | `Wire.sheet`, `corners` (sharp only), `WireEnd::Free`; `connect` stores corners; the tidy rules; `wire.draw`; scene, PDF and hit test over polylines; `erc.wire_end_free` and `ObjectRef::Wire`; examples regenerated; tests updated. | 5–6 |
| **1b. Wire tool** | Corners, ends, double-click, Enter, Esc, Backspace, F, the hints, the card, and one `wire.draw` per wire; tool tests. Ships drawing with corners and free ends. | 3 |
| **2. Rubber-band on move** | `wire_edit::follow_end` for parts, labels, sheet symbols and sheet pins, in the move's undo step and the drag preview. | 2–3 |
| **3. Path editing** | Path selection and hover, handles, corner and segment drags, insert and remove corner, ⇧Delete split, free-end drag and join, extending a free end with W; the `wire.*` edit commands. | 4 |
| **4. Arc corners** | `Corner.radius`, drawing the fillet in scene, PDF and hit test, A in the tool and on a selected corner, the radius drag and card field. | 2–3 |
| **5. Free posture** (optional) | `/` in the tool and ⌥ on drags. | 1 |

Total: about 17–20 agent hours. Slice 1 (1a + 1b) delivers the founder's original request. Before slice 2 lands, a moved part leaves the segment on its pin slanting; that is acceptable only briefly, so slice 2 should follow straight after.

## Founder decisions (2026-11-06)

All six recommendations were accepted:

1. **Arc form:** a rounded corner with a radius at a vertex.
2. **A junction left with one path** after a deletion keeps a free end and warns.
3. **Two free ends meeting** join into one path automatically.
4. **Starting a wire in empty space** with a free end is allowed.
5. **Postures:** 90° and free; no 45°.
6. **Selection:** a click selects the whole path; segments are edited through handles.

## Slice 1 as built

Where the build made a smaller or different choice than the plan above:

- **`Corner` has no `radius` yet.** It is `{ "at": [x, y] }`; slice 4 adds `radius` as a defaulted field, so files stay readable.
- **`wire.draw` returns no wire ID.** The tidy that follows may merge the new wire into another path (a free end continuing a path), so an ID would not be reliable. Its `from` and `to` are `WireTarget`s, which gained the `{"free": [x, y]}` form; `schematic.connect` and `label.place` refuse it.
- **Free ends meet exactly.** Free ends are held in whole nanometres, and two join only at the same point, not within a tolerance. The tool snaps them to the grid.
- **`junction.remove` with one or two wire ends** no longer refuses at a bend: the point stops being placed and settles like any junction (two wires merge into one path). At three or more ends it pairs opposite legs as before and refuses a T.
- **Validation.** The rule that two wires may not join the same pair of ends is gone, because a merge can legitimately produce such a pair; `schematic.connect` still refuses to join two already joined points. A wire may begin and end at the same point only when it has corners (a loop). After tidy, an unplaced junction has three or more wire ends and a placed one at least one, and validation checks that.
- **Edge cases of the tidy rules.** A junction whose only wire ends on it twice (a loop) leaves both ends free. A wire of no length left with a free end, such as a label's wire to its pin after the label is removed, is removed. Two free ends of one wire at one point stay apart.
- **Wire tool.** Its keys form a new keyboard context, Schematic › Wire tool (`WIRE_KEYS`), where F (Fit view) and Esc (`schematic.cancel`) are listed shadows. The card shows Start and Elbow; Posture, Arc and Radius arrive with slices 4 and 5. The success toast reads "Drew the wire: …". The drawing hint is shortened to "Click adds a corner · a pin or wire ends · double-click or Esc ends here": the planned wording was cut short on an 800-point canvas (`the_wire_hints_are_not_cut_short`).
- **Ending on a label's drawing.** A port on a pin stands off that pin on a leader, while its connection point stays the pin end. The Wire tool therefore also lands on a label's drawing, and tidy attaches a free end that lies on its lines (not its text), keeping the point reached as a corner. A label placing itself on a pin stands for that pin, so it stays on its pin. Slice 3 made `wire.draw` itself do this: naming such a label ends on its pin.
- **Blocked and reviewed wires.** A wire the engine cannot take (a device-NC or reserved pin) opens no review: a toast says why and the drawing goes on. A wire on a pin marked no-connect applies and is reported as `instance.no_connect_connected`. Cancelling a real review returns to the drawing.
- **Hit testing** still measures every wire on the sheet each frame in screen space, as before; the bounds index is left for slice 3, which adds per-segment handles.

## Slice 2 as built

- **Where it runs.** `schematic::apply` takes each wire's end positions before `symbol.move_unit`, `symbol.orient_unit`, `label.move`, `label.edit`, `sheet_symbol.move`, `sheet_symbol.resize` and `sheet_pin.edit`, and afterwards rubber-bands every wire whose attached end moved (`wire_edit::follow_moved`). It is part of the command, so it is in the same undo step, and the CLI, MCP and `Change::Schematic` previews get it too. Free ends never move.
- **The rule** (`wire_edit::follow`, pure, used by the canvas's drag preview and by the command): the corner next to a moved end slides along the second segment when the first is horizontal and the second vertical, or the other way round. Otherwise the next vertex stays: a segment that is still horizontal or vertical is kept, and one that would slant steps through one new corner that keeps the moved end's old direction (horizontal first when it had none, e.g. a slanted segment). Rule 2 of the plan is therefore "the moved end keeps its direction", not always horizontal-first. A wire whose two ends move by the same offset (between two pins of one unit, or two pins of one sheet symbol) is carried along whole. Corners that add nothing are dropped afterwards, as everywhere.
- **Corner limit.** A wire that would pass `MAX_CORNERS` keeps its corners (its end segments may then slant) rather than refusing the move.
- **Junctions cannot be moved** today (there is no `junction.move`), so nothing follows a junction; slice 3's handle edits are where that would come.
- **Preview.** The canvas previews a dragged unit's, label's or sheet symbol's wires with `wire_edit::follow`. Before, a dragged label or sheet symbol left its wires behind until the drop. A label that sits on a dragged unit's pin is not previewed following it (its wire has no length).
- A moved end's corner may now land on another connection point, which joins it as before (a T); the slide does not avoid points.

## Slice 3 as built

- **Commands.** `wire.set_path`, `wire.move_corner`, `wire.move_segment`, `wire.insert_corner`, `wire.remove_corner`, `wire.remove_segment` and `wire.move_end`, each previewed by a pure `wire_edit` function and refused as a whole. `wire.set_radius` comes with slice 4. `wire.move_segment`'s `offset` is +y for a horizontal segment, +x for a vertical one and the side of +x for a slanted one, whichever way the segment runs. `wire.remove_segment` on a segment at the end of a wire leaves one piece, not two, and on a lone segment removes the wire. The compact one-line path form in section 3 is not built; indices read off the stored `corners`.
- **A corner in line** (a new one, in-line neighbours) has no direction to keep, so the plan's rule would lift the whole line. It breaks the line into a step instead: the segment before stops at the corner's new position and turns to reach it, and the segment after keeps its direction. Moved along the line it adds nothing and is dropped. Other in-line corners next to a moved vertex move with it as one run.
- **An inserted corner stays until the next edit of that wire**, which drops it if it still adds nothing, so indices may shift. A corner drag therefore leaves no corner picked.
- **Landing.** A corner drag snaps to connection points and free ends, not to a point on a path: a corner on a path never connects, so a snap there would promise a join that does not happen. A segment drag follows the grid only. Whatever the preview runs over gets the snap ring, as in the Wire tool. A free end lands on anything the Wire tool lands on, except its own wire, where it stays free.
- **Review.** ⇧Delete, corner, segment and end edits apply at once, one undo step, without a toast. The review opens when the plan touches routed copper, or when the point landed on merges two named nets (the Wire tool's rule). A segment or corner that merely runs over a point on another named net applies without the review; the ring shows it first and undo takes it back. Refusals are toasts. Delete with no corner picked always opens the disconnect review.
- **The hint** for a selected path reads "Drag reshapes · double-click adds or drops a corner · ⇧Delete splits"; longer wordings were cut short on an 800-point canvas (`the_path_hint_is_not_cut_short`).
- **⇧Delete** needs the selected path under the pointer, and does nothing otherwise. It is `schematic.split-wire` (⇧Delete or ⇧Backspace) in the registry, in the Schematic context.
- **Handles** are a list of grabs painted and hit in one order, so slice 4 adds the arc dot as one more `Grab` and a card row for the picked corner. Squares are 7 points and reach 8; a midpoint dot shows only on a segment at least 28 points long on screen, and the segment itself stays draggable. `handles_are_not_crowded_on_short_segments` checks no overlap for one-grid staircases at 6 to 40 px/mm. ⌥ on a corner drag (`free`) came early because the command has it; ⌥ on other drags and `/` stay in slice 5.
- **Hover has no bounds index.** The canvas already walks and allocates every wire's screen path each frame to paint it, and hover adds one pass of arithmetic over those points, only while the pointer is over the canvas with the Select tool; an index would not change the frame's order. It is worth adding with a wider culling of off-screen wires, if measurement asks for it.
- **Groups.** A group selection shows no handles and keeps its drag rules. A symbol takes a drag ahead of a segment under it; a square takes it ahead of anything.
- **`schematic.connect` naming a port on a pin** still ends the port's self-placing: only `wire.draw`, `wire.move_end` and `wire.set_path` stand it in for its pin.

## Slice 4 as built

- **Model.** `Corner.radius` in mm, omitted when 0, so earlier files read unchanged. Loading refuses a radius that is negative, not finite or over 1000 mm (`schematic::MAX_CORNER_RADIUS_MM`). The vertex stays the stored point: connectivity, T-splits, rubber-banding and every edit work on vertices, and junction dots are unchanged.
- **Edits keep radii.** The `wire_edit` functions take bare points or corners (`wire_edit::Vertex`): a corner keeps its radius when an edit moves it (corner and segment drags, rubber-banding, tidy's splits and merges, paste, a group move), and a corner an edit adds is sharp (a step corner, an inserted corner, a junction that becomes a corner). A corner that becomes a wire end, as in a split at it, loses its radius.
- **What fits.** An arc reaches r·tan(turn/2) along each of its segments (r at 90°). `wire.set_radius { wire, index, radius }` refuses a radius whose reach is longer than either segment less what the neighbour's arc, as drawn, takes of it, and names the limit ("at most 8 mm fits"); it refuses any radius on a corner in line or folding back. `wire.draw` and `wire.set_path` store radii as given (0 to 1000 mm), and an edit that shortens a segment keeps them: where arcs would overlap on a segment both are drawn shrunk in proportion until they meet (`geometry::rounded_path`, as for symbol polylines), and the stored radius comes back when there is room. The Wire tool and the radius drag only send radii that fit.
- **Drawing.** `SheetScene.wires` is a `Shape::Polyline` per wire with its `radii` (empty when every corner is sharp), so `schematic.scene` returns `{"polyline": {"points", "radii"}}` instead of a bare point list. The PDF writer has no arcs, so the print flattens them to its 0.02 mm curve tolerance; the canvas flattens to a quarter point. A pick measures to the drawn path, arcs included; the hovered or dragged segment is then the nearest vertex segment, and the hover's strong line is that segment's straight part between its arcs.
- **Wire tool.** A (`wire.arc`) switches arc corners on or off and stays across wires, as the Fill tool's A does. The card gains Arc (A) and Radius (2.5 mm by default, 0.1 to 1000 mm). New corners take the card's radius, each fitted (two that would overlap on a one-grid step get 1.25 mm each) and rounded down to the micrometre, so what is stored is what is drawn. The preview draws the arcs.
- **Selected path.** A rounded corner shows a dot in the middle of its arc, 3 points in radius, when it is at least 10 points from its corner's square (for 2.5 mm, from about 10 px/mm), as the midpoint dots do. Dragging it sets the radius from the pointer's reach along the line halving the corner, in 0.5 mm steps, clamped to what fits (the drag clamps; the command would refuse); dragged onto the vertex, the corner is sharp. A press on the dot picks its corner. A (`schematic.arc-corner`) rounds the picked corner by the Wire tool card's radius, or as much as fits if less, or makes a rounded corner sharp; a corner with no room is refused with a toast. With a corner picked, the Net card's Wire section shows its Radius, committed live as `wire.set_radius`, with a refusal on the field. Each change is one undo step.
- **Keys.** `wire.arc` (A) is in `WIRE_KEYS`; `schematic.arc-corner` (A) is in the Schematic context and listed in `commands::SHADOWS`, because the Wire tool's A shadows it.
- **Not built:** free (bulge) arcs, by the founder's decision, and slice 5.
