# Schematic wires as paths: design

**Status:** slice 1 (1a path model, 1b Wire tool) built on 6 November 2026; slices 2–5 are not. See "Slice 1 as built" at the end for where the build differs from this plan. The founder decided that a wire is one connected line made of straight and arc segments, like an SVG path, and that it must be easy to edit. This document plans that change. Items marked **Open question** need the founder's decision before the slice that depends on them starts. Estimates are agent hours.

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
- **Hit testing** still measures every wire on the sheet each frame in screen space, as before; the bounds index is left for slice 3, which adds per-segment handles.
