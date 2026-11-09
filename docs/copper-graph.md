# One copper graph (design)

Design for review, 8 November 2026. Base: `component-model` at 32116be. Approved by the founder with all its recommendations. All five slices are built; see [Slice 1 as built](#slice-1-as-built) to [Slice 5 as built](#slice-5-as-built), and [Where connectivity lives](#where-connectivity-lives) for the final state.

The founder's requirement: "single copper graph. there is no excuse for duplication. everything needs to be single source of truth." His ruling on pairs: "what is a pin-to-pin link other than a track between two pins". So this design keeps no separate link or branch concept. Copper off a pair's signal path is ordinary net copper. Where it ends at a pin, it is a track between pins. Where it ends at no pad, it is a stub.

This document covers what decides connectivity today and where those deciders disagree, the one model that replaces them, its lifetime and cost, how each consumer becomes a query, which results change and why, the migration slices, and the open questions.

## 1. Inventory: every place that decides "these copper items are joined"

Nine places decide joining. A further five touch copper or connectivity but decide nothing about joining; they are listed so that none is missed.

### Deciders

| # | Where | What it decides | Joining rule | Consumers |
| --- | --- | --- | --- | --- |
| D1 | `BoardCopper::checks`, `layout/copper.rs` | `board.unrouted` (a net's pads in more than one island) and `board.via_dangling` (a via touching nothing) | Union-find over pads, vias and tracks in a `BoundsGrid`. Two shapes join if they share a net, share a layer (or either is `through`) and `CopperRing::touches` holds (gap ≤ 1e-8 mm plus curve slack, track spines exact). **Every via counts as `through`, whatever its `ViaSpan`.** Fill islands are unioned from `fill::links`. Stops at `MAX_CHECK_PAIRS` (10⁶) and emits `board.check_limit`. | `layout::checks` → Issues query, CLI and MCP `board_checks`, preflight (`board.unrouted` blocks), Update PCB findings diff, desktop `PcbEditor::issues` |
| D1b | Same function, `board.track_dangling` | A track end is open | A separate local test, not the union-find: a disc of the track's width at each end must touch some other same-net shape on its layer, or a through shape. **A track joined to any fill is skipped entirely.** A via counts as a joint even if it joins nothing else. | Issues |
| D2 | `BoardCopper::airwires`, `layout/copper.rs` | Which pads are in different islands, and the guide lines between them | The D1 scan repeated with `TOUCH_REACH_MM`, and the same fill union. At the pair limit it **stops silently** and leaves extra airwires, with no finding. Islands are chained in pad order, not by distance. | Desktop airwires per revision, the drag preview (`BoardCopper::moved`), the status-bar count, tests |
| D3 | `graph()`, `layout/length/path.rs` | A pair half's signal path, endpoints, branches, path spans | Its own rules. A track end joins another track if the end is within 1e-6 mm of that track's **centre line**. Otherwise an unjoined end joins if it is within the other track's half-width. **A crossing in mid-span never joins.** A via joins if the centre line passes within the via radius + 1e-6, on a layer `via.on` allows. A pad joins through `touches`, at the nearest leg only. A via in a pad joins it. **Fills are ignored.** The half is refused past 2,048 tracks or 64 pads (it is then measured by total copper). O(n²) in the half's tracks. | Pair skew and uncoupled length, `signal_length` (tune targets, group lengths, `meander::wanted`), the length report, the High speed card |
| D4 | `net_lengths`, `layout/length.rs` | Which vias add a barrel to a net's length, and which layers it spans | A via joins every same-net track whose **centre line** passes within the via radius + 1e-6, **on any layer** (no `via.on` check). O(vias × the net's tracks). | Net rows of the length report, length groups, `signal_length` for unpaired nets, the tune target |
| D5 | Fill compute step 5, `layout/fill/compute.rs` | Which fill islands are joined (and so kept), and the stored `FillJoin`s | A pad is probed at its centre or along its spokes. A via is probed the same way. A track is probed by **sampling its centre line** at ≤ 65 points, so copper that overlaps only with its edge does not join. | Island removal, `fill.island_unconnected`, the persisted `FilledCopper::joins` → `fill::links` → D1 and D2 |
| D5b | `fill::links`, `layout/fill/checks.rs` | Whether a fill's joins count | **A stale fill joins nothing** until it is refilled. | D1, D2 |
| D6 | `plan_update` impact, `updates.rs` (around line 2840) | Update PCB: which tracks or vias are "detached from" a changed pad, and which tracks are conflicts | `track_touches_pad` and `via_touches_pad`, pairwise, before and after, for each changed pad × every track and via (budget 100,000). This repeats D1's touch rule outside any index. **`NetChanged` marks every track of a net that loses a terminal** ("would join pads the net no longer joins") without asking which pads that copper joins. | The update preview, `update.apply` acknowledgements |
| D7 | `Ctx::track_ends_at`, `Ctx::joint_at` (`router/world.rs`), `adopt_line` and `follow` (`router/shove.rs`) | Where a shoved chain ends, and which tracks follow a moved via | Track ends coincide within 1 µm. A via joins only at its exact centre (1 µm). A pad joins if the point is inside its polygon. **A joint in mid-span (a T, or a via on the segment) is invisible.** | Shove, pair shove |
| D8 | `joins_net_copper`, `head_joins_layers` (`apps/pcbcraft/src/pcb_editor/route_tool.rs`) | Whether a route click finishes the route, and whether a layer change needs a via | A point lies on a same-net track (on the layer) or inside a same-net via disc (**on any layer**), excluding the route's own copper. It finishes on the start island too, and ignores fills. | Desktop route tool |
| D9 | `meander::plan` (`layout/meander.rs`) | That a track's joints are its two ends ("The ends are exactly the track's, so its joints stay joined") | An implicit assumption. A T or via in mid-span is not looked for. | Tune tool, `layout.tune` |

### Non-deciders

These are listed so that none is missed:

- **Pair continuation** (`layout/pair.rs`, 1e-9 mm end match) extends the previous track geometrically and decides nothing about connectivity.
- **The 3D scene** (`solidcraft-integration`) decides nothing. `FillContact` sets a foil's level, and bodies are disjoint except where copper meets copper.
- **Production and preflight** only read `board.unrouted` from D1.
- **Clearance during routing** (`apply_routes`, `via::check_clearance`) uses the same rings, but it decides gaps, not joins.
- **Schematic nets** (`nets.rs` union-find) decide logical nets, not copper.

### Duplicated work per revision (desktop)

`PcbEditor` builds `BoardCopper` and runs D2 and D1. The Issues query runs `layout::checks` again, so D1 and D5b run a second time. The High speed card runs D3 and D4. `signal_length` reruns `Measured::of`, which is D3 for every pair plus D4, on every call. `tune_target` and `meander::wanted` call it.

### Disagreements, confirmed

I confirmed these with a throwaway test on the demo board: R1 at (10, 10) and R2 at (25, 10), net 2 joining R1.2 and R2.1, 0.3 mm tracks and 0.6/0.3 mm vias. The pair measure was run with both halves on net 2. The test is not committed.

| Case | D1 and D2 (checks, airwires) | D3 (pair graph) | D4 (net length) | Right answer |
| --- | --- | --- | --- | --- |
| C0 baseline: one straight track | routed | path R1.2→R2.1, 12.6 mm | 12.6 mm | all agree |
| C1: two tracks cross in mid-span (an X) | routed; 2 dangling overhangs | **no path** (measured by copper) | 16.175 mm total | routed. The path runs through the crossing, and the overhangs are stubs. |
| C2: track ends overlap a via ring 0.35 mm off its centre | routed, nothing dangling | **no path** | **barrel 1.635 mm, one via missing** (should be 3.27) | routed. Both vias carry the signal. |
| C3 (the known via gap): a track ends at a via that joins nothing else | unrouted, but **no `track_dangling` and no `via_dangling`** | no path | — | the track plus the via is a stub and should be reported |
| C4 (latent: commands refuse `ViaSpan::Between`): a blind via Top–In1 under a Bottom track | **routed** | no path | **barrel counted to Bottom** (3.27) | unrouted. The via does not reach Bottom. |
| C5: a fill joins the pads, and a track runs from inside the fill to an open end outside it | routed, **`track_dangling` = 0** | no path ("copper joined only through a fill has no path") | — | routed, with the open end reported as a stub |
| C6: two tracks overlap side by side (centres 0.2 mm apart); neither end lies on the other's copper | routed | **no path** | 13.6 mm | routed |

Disagreements found by reading the code (not executed):

- **D2 and D1 at the pair limit:** D1 reports `board.check_limit`. D2 silently draws extra airwires.
- **D5 and an exact rule:** a same-net track whose edge (but not its centre line) overlaps an island does not join it. On a track over 64 widths long, a narrow neck between two sample points is missed.
- **D6:** a track "detached from" a pad may still be joined to it through other copper, and every track on a net that loses a terminal is flagged, including tracks joining only the pads that stay.
- **D7 and D9:** shoving or tuning a track ignores a mid-span T or via. Shove usually fails safe, because the T track is hit as well and cannot move its fixed end. A meander replaces the whole segment and can detach a mid-span joint.
- **D8:** a click on the route's own start island counts as finishing the route, and a via counts as joining the routing layer even when its span does not reach that layer.

## 2. The model

### Where it lives

The model lives in `layout/connectivity.rs` and a `layout/connectivity/` folder (`graph.rs`, `index.rs`, `queries.rs`, `tests.rs`). It sits inside `layout`, which may use `rules`, `geometry`, `ids`, `diagnostics`, `library`, `model` and `instances`. Everything it needs is in that set, so `cargo xtask core-modules` needs no map change. `updates` and `issues` are above `layout` and may call it.

`BoardCopper` stays as the home of the clearance and edge checks, but it no longer computes connectivity. `CopperRing` stays as the shape type.

### Elements

An element is one piece of copper on one layer. Every element carries its `ObjectRef` (identity), its net and its copper layer.

| Element | Geometry (exact) | Notes |
| --- | --- | --- |
| Track segment | Capsule: centre segment ⊕ disc of half-width | It already has an exact `spine` in `CopperRing`. |
| Track arc (when tracks gain arcs) | Arc ⊕ disc | Reserved variant. Its joints carry the arc-length position. |
| Via land, one per layer in its span | Disc (centre ⊕ radius) | One element per layer the via reaches (`Via::on`), joined by **barrel edges** carrying the mid-plane distance. A through via on 4 layers is 4 lands and 3 barrels. |
| Pad land, one per copper layer | Rect, rounded rect, circle or oblong, exactly as "convex core ⊕ disc" (a rounded rect is the inner rect ⊕ r, an oblong is a segment ⊕ r). A custom outline is a polygon with curve slack. | A plated through pad is one land per layer, joined by zero-length barrel edges. A pad's net is its terminal's net. |
| Fill island | Polygon with holes (`FilledRegion`), slack 0, because the stored polygon is the fabricated copper | One element per island of a **fresh** fill. A stale fill contributes nothing (open question Q3). |

Generalising `CopperRing::spine` into a core (point, segment or convex polygon) plus a radius makes every built-in pad shape exact. Only custom arcs keep the 2 µm slack.

### One joining rule

Two elements **join** when all three hold:

1. they are on the same copper layer (lands make layer spans explicit, so nothing is "through" any more);
2. they are on the same net;
3. their copper overlaps or touches: the exact distance between the cores, less both radii, is ≤ `JOIN_TOLERANCE_MM` (1e-3 mm, 1 µm), plus the curve slack of any flattened custom arc.

That one rule replaces all nine deciders. The tolerance is 1 µm: far below anything a fab resolves, and above double rounding on mm coordinates. Today's 1e-8 is tighter than the coordinates' own rounding after rotation, and three of the copies already use 1e-6.

**Different nets never join.** Overlapping copper of two nets is a short, which the clearance check reports. The graph never merges nets.

**Each joint carries positions.** A joint between a track and anything else records the **station** `s` on the track: the distance from its start to the point where the other copper's core is nearest the track's centre line, clamped to [0, L]. Rules for that point:

- **Track end to another element:** the end's own station (0 or L), plus the station of the nearest point on the other track. This matches D3's exact centre-line joints when the end lies on the centre line, and its fallback when it lies on the copper.
- **Track crossing a track:** the crossing point on both tracks. When the centre lines do not cross (side-by-side overlap, C6), use the nearest points of the two segments.
- **Track on a via or pad land:** the station nearest the land's centre. For a pad, that is the pad's terminal point, which keeps D3's "last leg into the pad" behaviour without a special rule: the stretch beyond the station is a zero-length or short spur inside the pad's copper and is not path.
- **Fill island to anything:** no station on the fill (a fill has no length), but the station on the track.

So a track becomes a chain of vertices at its sorted, deduplicated stations, with edges of exact centre-line length between them. Paths and lengths are read off directly, as D3 does today.

### Graph shape

`CopperGraph` has three parts:

- **Vertices:** each track station, one per via land, one per pad land, and one per fill island. A pad land's vertex is its **terminal** (`Endpoint`).
- **Edges** of four kinds:
  - **Track stretch** `{track, lo, hi}`, with length `hi − lo`.
  - **Barrel** `{via, from_layer, to_layer}`, with the mid-plane distance (D4's and D3's length model, unchanged).
  - **Joint**, with length 0, between a track station and the element it meets, or between two lands.
  - **Plated hole**, with length 0, between a through pad's lands. Pad-internal length is not counted, as today.
- **Islands:** union-find over the same joints. Each element and vertex has an island id. Each island knows its net, its terminals, and its open ends (below).

Per-net subgraphs are slices of one board graph: vertices and edges are stored grouped by net, with an offset table. Pair measurement runs shortest paths on the half's slice, exactly as D3 does now, but on joints from the shared rule.

Fill islands are vertices with zero-length joint edges. A path may run through a fill, but it adds no track length through it. A pair "joined only through a fill" now has a path whose fill hop is reported (open question Q4).

### Dangling (stubs) on the graph

An **open end** is a track end vertex (station 0 or L) whose only edge is its own track stretch. Nothing else meets it. That is local, as today, but read from the joints.

A **stub** is the part of an island that does not lie on any path between two terminals. More precisely:

1. In the island's graph, repeatedly remove leaf vertices that are not terminals, along with their edges.
2. Whatever is removed is copper that leads to no pad. Its open end, or the via it stops at, is reported.

This resolves both gaps at once:

- **The via gap (C3):** a via land is a vertex like any other, not a termination. A track ending at a via that joins nothing else is pruned together with the via: one `board.track_dangling` naming the track, the end at the via, and the via.
- **Fill-joined tracks (C5):** a track joined to a fill is no longer exempt. If its far end is a leaf, it is pruned and reported.

A fill island is never pruned as a stub. A pour is meant to be copper that reaches no pad over most of its area. An island with no joints at all is the fill's own `fill.island_unconnected`, unchanged.

**`board.via_dangling`** becomes a via that, after pruning, keeps no edge to any terminal (its island holds no terminal), or that joins nothing at all. It is one finding per via, as today. A via pruned as part of a track stub is named in that stub's finding instead of a second one (open question Q2).

**Branches.** Under the founder's ruling, copper off a signal path that ends at pins is a track between pins and raises nothing. Copper that ends at no pad is a stub, found by the pruning above on every net, paired or not. The pair code needs no branch concept: the stub test is net-wide, and the pair consumer reads only its path.

### Unrouted and airwires

- **`board.unrouted`:** a net whose terminals lie in more than one island. It is one finding per pad, as today.
- **Airwires:** per net, a minimum spanning tree over the islands. The weight between two islands is the shortest distance between their terminals' centres. With k islands that is k − 1 airwires, the same count as today, but each joins the nearest pads instead of the first pads in document order (open question Q5). Islands without terminals (a floating track) get no airwire; they are stubs.

## 3. Lifetime and performance

### Built once per revision, owned by the engine

`Engine` gains a lazily built `Derived` cache: a `OnceCell<Arc<CopperGraph>>` that is reset whenever `revision` changes. That covers apply, undo, redo and replace. Every core caller that has an `Engine` reads `engine.copper_graph()`. Pure functions that take a `&Project` (tests, `update` staging of a candidate) call `CopperGraph::of(&project)`. That is the same builder; it is only uncached.

The desktop stops building its own `BoardCopper` for connectivity, and the Issues query stops recomputing it. The cache is wasm-safe: no threads, and `Arc` only so that the desktop may hand the graph to a worker later.

**Incremental update is not proposed for the first cut.** The measurements below put the full build within budget. The design keeps the seam for it: elements are keyed by `ObjectRef`, the index is a uniform grid, and a route step touches a handful of tracks and vias. A later `CopperGraph::edited(&self, &RouteEditSet)` could re-join only the elements within reach of the changed bounds, re-run union-find on the affected nets, and keep the rest. Add it only if a measurement on the 10,000-via board shows the full build costs more than a frame (open question Q6).

**The drag preview** (`BoardCopper::moved`) becomes `CopperGraph::moved(instance, delta)`. It translates that placement's pad lands and re-joins only them and their neighbours in the grid. It does not rebuild the graph.

### Index and bounds

- **Build.** A single `BoundsGrid` (the existing `layout/grid.rs`) is keyed per layer, with cells sized to the median element. Candidate pairs come from same-layer, same-net cells only, so different-net neighbours never cost a narrow-phase test. The current scans test every nearby pair, because the same scan serves clearance. Separating connectivity from clearance makes connectivity cheaper.
- **Narrow phase.** Exact core distance: segment–segment, point–segment or polygon distance, with the bounds rejection first. That is the code already in `CopperRing::gap`.
- **Limits.** Element counts are already bounded by the document (`MAX_TRACKS`, `MAX_VIAS` 16,384, `MAX_FILLS` 512, fill vertex caps). The joint count is bounded by `MAX_JOINTS` = 4 × elements. Past it the build returns a `limit` result. Consumers then report `board.check_limit` (checks and airwires alike: the D2 silent stop goes away), and pair measurement falls back to total copper, as today. Shortest paths are Dijkstra per half: O((V + E) log V) instead of today's O(n²) build.
- **Memory.** The element is 24 bytes of identity and net, plus 48 bytes of geometry for tracks, vias and pads; fill polygons are referenced, not copied. A vertex is 16 bytes, and an edge 32 bytes. For 10,000 through vias on 4 layers (40,000 lands, 30,000 barrels), 2,000 tracks, 2,000 pad lands and fills: about 45,000 elements (≈ 3.2 MB), about 50,000 vertices (≈ 0.8 MB) and about 90,000 edges (≈ 2.9 MB), so **under 8 MB** plus the grid. That is a target to measure in slice 1, not a measurement.

### Measured today

The existing benchmark's board (`via/tests.rs::via_board`) has 10,000 through vias and 2,000 tracks on 4 layers. Measurements were taken on an Apple M5 Max, with the `test` profile (pcbcraft-core at opt-level 1, dependencies at 2), best of 5:

| Work | Time |
| --- | --- |
| `BoardCopper::of` | 1.6 ms |
| `BoardCopper::checks` (connectivity, clearance, edge, via and courtyard checks together) | 23.2 ms |
| `BoardCopper::airwires` (D2 alone) | 5.0 ms |
| `net_lengths` (D4) | **26.7 ms**: O(vias × the net's tracks) |
| `layout::checks` | 27.8 ms |
| Existing benchmark `ten_thousand_vias_are_all_checked_within_a_budget` (checks + airwires) | 27.4 ms |
| `path::measure` (D3), one half of 256, 1,024 and 2,048 collinear tracks | 2.0, 18.2 and **61.2 ms** (quadratic) |
| `BoardCopper::checks` on those boards | 1.4, 10.9 and 22.4 ms |

The airwire scan (5 ms) is the cost of the connectivity part of today's checks. The graph build is expected to sit near that, since it does the same joins once, with stations. Net length becomes a sum over edges per net in O(E). Pair measurement becomes Dijkstra on a slice that is already built. Slice 1's exit is the real number, with targets of ≤ 10 ms to build the graph on the benchmark board and ≤ 1 ms per pair. Those are targets until measured. The edit cost then drops below today's, because the desktop and the Issues query share one build instead of two.

### The 3D scene

The 3D scene does not read the graph to build geometry. Copper bodies come from the document, as today, and are not rebuilt on selection or camera changes. The graph only adds **highlight sets**: "this net", "this island", "this stub", "this pair path". These are lists of `ObjectRef` (or track stretches `{track, lo, hi}` for a path) that the scene draws through its existing highlight blocks (`Instanced::highlight`). Selecting a net, or hovering a stub finding, changes the highlight set only. The graph is per revision and is never rebuilt by a view change either.

## 4. Consumers as queries

| Consumer | Today | Query on the graph |
| --- | --- | --- |
| `board.unrouted` | D1 union-find | `graph.islands_of(net)` with more than one terminal-bearing island → one finding per pad |
| `board.track_dangling` | D1b local disc test | `graph.stubs()` → one finding per stub: its tracks, its open end or via, its length |
| `board.via_dangling` | D1 `joined` flag | `graph.dangling_vias()` (Q2) |
| Airwires and the status-bar count | D2 | `graph.airwires()`, the minimum spanning tree per net; `graph.moved(..).airwires()` during a drag |
| Net length | D4 | `graph.net_length(net)`: the sum of track stretches plus barrels between the outermost layers the net's tracks use at each via. That is the same length model, but read off the joints. |
| Pair measurement | D3 | `graph.pair_path(half, endpoints)`: Dijkstra on the half's slice. It returns `HalfPath { from, to, tracks, vias, spans }`. The `branches` field is gone (below). |
| Signal length, tune target, group lengths | `Measured::of` per call | `graph.signal_length(net)`, cached per revision with the graph |
| Fill island removal and `fill.island_unconnected` | D5 probes | Fill compute calls `connectivity::joins(island, element)`, the **same primitive**, so a track joins by copper overlap rather than centre-line samples. `FillJoin` keeps its thermal data (spokes, connection), which fabrication and the spoke checks need. Its `region` is no longer read for connectivity, because the graph joins the fresh fill's islands itself. |
| Update PCB "detached" warnings | D6 pairwise touches | Stage the candidate, then `CopperGraph::of(candidate)` (one build, indexed). Warn for a terminal whose island changed: a terminal joined before that sits in a different island, or in none, after the update. The 100,000-pair budget goes. |
| Update PCB "would join pads the net no longer joins" | D6: every track on a changed net | For each changed net, the candidate graph's islands: a track is a conflict only if its island holds terminals that now belong to **different** nets, or terminals of a removed net. Copper of a net that only lost a pad elsewhere stays. |
| Shove chain ends | D7 end and centre coincidence | `graph.joints_on(track)`: a chain continues through a station only if it has degree 2 and the other edge is a same-width track stretch. A mid-span joint pins the chain at that station. |
| Shoved via followers | D7 exact centre | `graph.tracks_at(via_land)` |
| Meander | D9 assumption | `graph.joints_on(track)`: the meander's straight lead-in and lead-out also keep clear of mid-span stations, or it is refused ("A joint at 3.2 mm; tune between joints"). |
| Route finish and via need (desktop) | D8 | `graph.element_at(point, layer, net)` → its island. Finish when that island is not the head's island. `graph.layers_at(point, net)` decides whether the head already spans both layers. |

### Pair consumers without branches (the founder's ruling)

These pair-side items go:

- The `PairBranch` type, `HalfPath::branches`, `PairRow::branches` (from the length report, the CLI and MCP), the `Graph::branches` code in `path.rs`, and the High speed card's branch rows ("Link to J1.A7 · 6.98 mm", "Branch to no pin · …").

**No consumer truly needs them.** Uncoupled length reads the path's `spans`, not the branches. Skew reads the path totals. A net's total copper is already in `NetRow.length`, next to `NetRow.path`. Stub detection is net-wide on the graph. The one candidate was the Tune tool's "does not stop a meander on a branch" gap. It needs "is this track stretch on the path?", which `spans` answers.

**The pair card shows** the measured path (from-pin, to-pin, length) for each half, the skew and the uncoupled length, and nothing else. A stub on a pair half shows in Issues as `board.track_dangling`, like any other net's.

## 5. Behaviour changes

Every change below is a correction, with its reason. None weakens an expected result.

| Result | Before | After | Why the new answer is right |
| --- | --- | --- | --- |
| Track to a via that joins nothing (C3) | no finding | `board.track_dangling` naming the track and the via | The copper ends at no pad: a stub by the approved definition. A via is a joint, not a termination. |
| Fill-joined track with an open far end (C5) | no finding | `board.track_dangling` | Same definition. The fill exemption hid real stubs. |
| A dangling track and its via | can raise both `via_dangling` and `track_dangling`, or neither | one stub finding | One fault, one finding (Q2). |
| Pair over an X crossing (C1), a side-by-side overlap (C6) or an off-centre via (C2) | measured by total copper, no path | measured along the path | The copper conducts. The checks already call it routed, so the pair now agrees with them. |
| Net length with an off-centre via (C2) | barrel omitted | barrel counted | The via carries the net between those layers. |
| Pair joined through a fill | "no path" | a path, with its fill hop marked (Q4) | It conducts. Whether a fill hop belongs in a high-speed path is a design-rule question, not a measurement one. |
| Blind or buried vias (C4, latent) | joined on every layer; barrel counted to any layer | joined only within their span | That is the copper. It lands before `ViaSpan::Between` is enabled. |
| Fill islands touched by a track's edge only | removed (`Unconnected`) or reported unconnected | kept and joined | Copper overlap is connection. |
| Airwires | first pads in document order | nearest pads between islands (Q5) | Same count; the shortest guide is the one a designer routes. |
| Pair limit | `check_limit` from checks; silent extra airwires | `board.check_limit` once, with every consumer marked incomplete | No silent disagreement. |
| Update PCB | every track of a net losing a terminal is a conflict; "detached" warnings per touching pad | only copper that would join terminals of different nets; detached means the island changed | The old rule asks for removals that no electrical change needs. The new rule is what the message already claims. |
| Branch rows and `branches` in the length report | listed | gone | Founder's ruling. |
| Shove and meander with a mid-span joint | the joint may be detached (meander), or the shove fails late | a chain is pinned at the joint; a meander keeps clear or is refused | Neither may silently break connectivity. |

Tests whose expectations change (by reading; slice owners confirm):

- `tests/high_speed.rs`: `a_pin_to_pin_branch_is_a_link_and_raises_nothing` and `the_receptacle_jumpers_are_links_and_not_skew` drop the `branches` assertions and keep "no finding" and "0 skew". `an_open_branch_is_not_skew_and_the_dangling_check_reports_it` keeps its skew, uncoupled and `track_dangling` assertions, drops the `open.length` and `pins` assertions, and asserts the 4 mm instead through `NetRow.length.total − path`.
- `crates/io/tests/esp32_pair.rs`: the `branches.len() == 2` and JSON `branches` assertions go. It asserts 0 mm skew, and the two 29.865 mm paths stay.
- `apps/pcbcraft/src/pcb_editor/pair_tests.rs::the_high_speed_card_lists_the_path_and_the_branches`: rewritten to assert the path rows only, and renamed.
- `layout/fill/tests.rs` around line 438: "stale joins are dropped" is unchanged (Q3 keeps the policy). Island tests may keep islands they used to drop if a track joins them by edge only; the fixture decides.
- `layout/tests.rs` and `via/tests.rs` airwire tests: counts are unchanged. Tests that compare pad pairs must expect the nearest pairs.
- `updates` tests asserting `NetChanged` for every track of a changed net: these change to the copper that actually joins the moved terminals.

## 6. Migration slices

Each slice lands green on `cargo xtask ci`, deletes what it replaces in the same change, and leaves no parallel path behind. Estimates are agent hours.

| Slice | Content | Deletes | Hours |
| --- | --- | --- | --- |
| **S1: the graph, checks and airwires** | `layout/connectivity` with elements, the joining primitive, the per-layer index, joints with stations, islands, stubs, MST airwires and `moved`. `Engine::copper_graph()` with the per-revision cache. `BoardCopper::checks` reads islands and stubs from it; airwires and the drag preview use it; the desktop and the Issues query share the one build. Benchmark tests: build time and memory on `via_board(10_000, 2_000)`, plus the C1–C6 fixtures as permanent tests. | The union-find, `find`, `union`, `join_fills`, `fill_key` and the D1b disc test in `copper.rs`; `BoardCopper::airwires` and `BoardCopper::moved` (replaced); `PcbEditor`'s own connectivity build | 6–8 |
| **S2: lengths and pairs, branches removed** | `net_length`, `signal_length` and `pair_path` as graph queries; `path.rs` keeps only endpoint selection (set, derived, longest) over the graph. `PairBranch`, `branches`, the card's branch rows and their tests go; the pair card shows path, skew and uncoupled length. Docs: `high-speed-routing.md` "Copper off the signal path" rewritten. | `graph()`, `pad_joints`, `Stations`, `Meets`, `Graph::branches`, `MAX_PATH_TRACKS`/`MAX_PATH_PADS` (the graph's limit takes over), the via scan in `net_lengths`, `PairBranch` | 4–6 |
| **S3: fills on the one rule** | Fill compute's island step calls the connectivity primitive; `FillJoin.region` is no longer read for connectivity; fresh fill islands become graph elements. | The centre-line sampling in step 5; `fill::links` | 3–4 |
| **S4: Update PCB** | The candidate graph decides "detached" and the conflicting tracks. | The pairwise `track_touches_pad` and `via_touches_pad` loops and the 100,000 budget; `track_touches_pad` itself if nothing else uses it | 3–4 |
| **S5: router, meander, route tool** | `joints_on`, `tracks_at`, `element_at` and `layers_at`; chain adoption, via follow, meander margins and the desktop's finish and via decisions use them. | `Ctx::track_ends_at`, `Ctx::joint_at`; `joins_net_copper` and `head_joins_layers` in `route_tool.rs` | 4–5 |

**Total: 20–27 agent hours.** S1 must land first. S2, S3 and S4 depend only on S1 and can run in parallel (the user's limit is 5 agents). S5 can also follow S1 directly. The S1 contract (`CopperGraph`'s query signatures) is fixed in S1's first commit so the others can fan out against it.

After S5, the only joining rule is `connectivity::joins`. `cargo xtask` could enforce that cheaply: a `layout`-scoped grep gate that rejects new calls to `CopperRing::touches` outside `connectivity` (open question Q7).

### Risks

- **Performance at 10,000 vias.** Per-layer lands quadruple the via elements on 4 layers. Mitigation: same-net, same-layer cells; one land vector per via with layer bits; measure in S1 before the consumers move. If the build exceeds about 10 ms, add incremental update (Q6) before S2.
- **Results moving at the edges.** C1, C2, C5 and C6 change established results. The S1 and S2 fixtures make each change explicit, and the ESP32 reference board must stay at 0 issues and 0 mm skew.
- **Endpoint derivation in pairs.** The set, derived and longest selection must give the same pads on the ESP32 board. That board is the regression gate.
- **Fill semantics.** Stale fills and fill hops in paths are policy (Q3, Q4); S3 waits for the answers.
- **A concurrent stub change.** The `copper-graph-stubs` agent is active in this session. If it closes the via gap inside `BoardCopper` first, S1 replaces that code with the graph's stub pruning and keeps its tests.
- **Shove behaviour.** Pinning chains at mid-span joints can make some shoves fail that succeed today, correctly, because those shoves would detach copper. The random-obstacle shove test must stay green.

## 7. Open questions for the founder

1. **Q1: Join tolerance.** I propose exact geometry with a 1 µm tolerance (`JOIN_TOLERANCE_MM` = 1e-3 mm), with curve slack only for custom arc pads. *Recommendation: yes.* It is far below fabrication resolution, and it replaces three different tolerances (1e-8, 1e-6, and centre-line rules).
2. **Q2: One finding for a stub and its via?** When a stub ends at a via, report one `board.track_dangling` naming both, and keep `board.via_dangling` only for a via that joins nothing at all, or whose island holds no terminal. *Recommendation: yes.* One fault, one finding.
3. **Q3: Stale fills.** Today a stale fill joins nothing, so airwires appear as soon as a pad leaves the pour. Should the graph keep that, or join through the stored (stale) copper? *Recommendation: keep it.* Stale copper is not the board that will be fabricated after the required refill, and `fill.stale` already blocks production.
4. **Q4: Paths through a fill.** Should a pair path that hops through a fill be measured (with the fill hop flagged, length 0), or refused ("no path")? *Recommendation: measure it and show "via fill" on the path row.* Refusing hides a conducting route.
5. **Q5: Airwires as a minimum spanning tree.** Same count, nearest pads instead of document order. *Recommendation: yes.* It is what a designer routes next and costs O(k log k) per net.
6. **Q6: Incremental update.** Only if S1 measures the full build above about 10 ms on the 10,000-via board. *Recommendation: defer.* Keep the `ObjectRef`-keyed seam so it can be added without changing a query.
7. **Q7: Enforcement.** Add an `xtask` gate so that nothing outside `layout/connectivity` calls the touch primitive. *Recommendation: yes, in S5.* It turns "single source of truth" into a check.
8. **Q8: Update PCB conflicts.** Should only the copper that would join terminals of different nets be offered for removal, instead of every track on the changed net? *Recommendation: yes.* That is what the message already says, and the rest of the copper stays valid under persistent net IDs.

## Slice 1 as built

Built on `component-model` at ce259f8, 8 November 2026.

### What exists

- **`layout/connectivity.rs`** with `connectivity/build.rs` (elements, the joining rule, joints with stations, vertices, edges, islands), `connectivity/queries.rs` (stubs, findings, airwires, drag preview) and `connectivity/tests.rs`. `cargo xtask core-modules` needed no map change.
- **Elements:** one per track; one land per layer a via reaches (`Via::on`, so a `ViaSpan::Between` via has lands only within its span), joined by barrel edges; one land per layer a pad reaches, a plated pad's lands joined by plated edges; one per island of a fresh fill.
- **The one joining rule** is `connectivity::build::joins`: same layer, same net, exact copper within `JOIN_TOLERANCE_MM` = 1e-3 mm (1 µm) plus curve slack. Slice 1 built it as `JOIN_EPS` = 1e-6 mm, 1 nm, by mistake; see [the tolerance correction](#tolerance-correction). Tracks and via lands are exact capsules and discs (`CopperRing::capsule`). Each outline is tested once per same-net neighbour, and a joint is made on every layer both outlines have a land on.
- **Stations:** a track end whose copper meets the other element joins at that end (0 or L); otherwise the track joins at its nearest station (a crossing, the nearest points of two segments, or the station nearest a land's centre). Stations within 1 µm merge. A track is a chain of vertices at its stations, joined by stretch edges.
- **Stubs:** dead ends that are not pads are peeled until none is left; pads and fill islands are never peeled. An island with no pad is removed whole, so a floating loop is caught too. A via that keeps a land on copper leading to a pad is not part of a stub; what hangs off it is a separate stub.
- **Findings** (`CopperGraph::findings`): `board.unrouted` per pad of a net whose pads lie in more than one island (message unchanged); one `board.track_dangling` per stub with tracks, naming its tracks, then its vias, then the net, with its open ends and the vias it ends at ("Track on N2 (Top) ends at (20, 20) mm and a via at (25, 25) mm joining nothing"); `board.via_dangling` for a via no stub names that joins nothing ("joins no copper of its net") or whose island holds no pad ("reaches no pad of its net"). A partial graph reports one `board.check_limit` and nothing else.
- **Airwires** (`CopperGraph::airwires`): per net, Prim's minimum spanning tree over the islands, each airwire joining the nearest pads of the tree and a new island. Ties go to the earlier pad, so the result depends only on which pads share an island.
- **Drag preview** (`CopperGraph::moved_airwires`): the graph's island forest without the moved pads' joints, plus joints found again for the moved outlines against their neighbours in the net's grid. The graph is not rebuilt. Fill joins stay as stored, as the old preview kept them.
- **Engine cache:** `Engine::copper_graph()` builds the graph on first use per revision (a `OnceLock<Arc<CopperGraph>>` reset whenever the revision changes: apply, undo, redo, replace and refill). The Issues query (`issues::report_on`) and the desktop (`PcbEditor::graph`, for its checks, airwires and drag preview) read the same build. Pure callers use `CopperGraph::of`, `layout::checks` and `layout::airwires`; `layout::checks_with` takes a graph.
- **Limits:** the joining scan stops past `MAX_CHECK_PAIRS` candidate pairs or 4 joints per element. The partial graph is marked; the checks report `board.check_limit`; airwires are drawn from what was joined and the check says they are incomplete. Nothing stops silently.

### Deleted

From `layout/copper.rs`: the union-find (`find`, `union`), `join_fills`, `fill_key`, the fill-link field, the same-net touch branch of the scan, the D1b dangling-end disc test, `BoardCopper::airwires` (D2) and `BoardCopper::moved`. `BoardCopper` now does placement, edge and clearance checks only. The desktop's own connectivity build is gone; it holds the engine's graph.

### Temporary adapter

- **`fill::links` → `FillLink { fill, region, members }`.** The graph reads fresh fill islands from the joins fill compute stored, and finds where a joined track meets the island from the stored island polygon (each end within reach, else the nearest station). Slice 3 removed it: fill islands now join by the one rule.

No other adapter was needed: pair measurement (D3), net length (D4), Update PCB (D6), the router (D7), the route tool (D8) and the meander (D9) decide nothing that slice 1 changed, and stay as they are until slices 2, 4 and 5.

### Deviations from the design

- **Pads keep their flattened outline.** The design's "convex core ⊕ disc" for exact round pads was not built; circle, oblong and rounded pads still join within `CURVE_SLACK_MM`, as before. Tracks and vias are exact.
- **Barrel edges carry no length yet.** Slice 2, which moves lengths onto the graph, adds the mid-plane distance.
- **The joint limit is the per-element budget** (`MAX_JOINTS_PER_ELEMENT` = 4), as designed, with no separate constant.

### Behaviour changes and test corrections

Every disagreement case is a test in `connectivity/tests.rs` with the right answer: `c0_a_straight_track_joins_the_pads`, `c1_tracks_crossing_mid_span_join_and_their_overhangs_are_stubs` (routed; two stubs), `c2_track_ends_off_a_via_centre_join_it` (routed; nothing open), `c3_a_track_to_a_lone_via_is_one_stub` (unrouted; one stub naming track and via; no `via_dangling`), `c4_a_blind_via_joins_only_the_layers_it_spans` (unrouted), `c5_a_track_leaving_a_fill_to_an_open_end_is_a_stub` (routed through the fill; the open end is a stub; stale, the fill joins nothing), `c6_tracks_overlapping_side_by_side_join` (routed; no stub). Further tests cover a branch to no pad, dangling vias, the nearest-pad airwires, the partial graph and the engine cache.

C4 shows one more consequence of the stub rule: with the blind via open, the copper from the blind via on to R2.1 (a Bottom track, a through via and a Top track) reaches one pad only, so it is one stub, as is the Top track and blind via from R1.2.

Changed expectations in existing tests, each a correction:

- `tests/board_checks.rs::a_dangling_track_end_warns_and_a_connected_one_does_not`. Two floating tracks joined end to end were two findings, one per track; they are now **one** stub finding naming both tracks and both far ends. One fault, one finding. A via added at the last open end used to close it, leaving one finding for the first track; the via joins nothing else, so it is a joint, not a termination, and is now named in the one stub finding ("… and a via at (25, 25) mm"), with no `board.via_dangling`. This is the via gap (C3) closed.
- `layout/tests.rs::moved_copper_gives_the_same_airwires_as_a_moved_placement` now calls `CopperGraph::moved_airwires`. Its expected airwires are unchanged. The assertion that the moved `BoardCopper`'s pads equal the rebuilt ones went with `BoardCopper::moved`; the moved pads are still compared, inside the airwires the test checks.
- `layout/via/tests.rs::ten_thousand_vias_are_all_checked_within_a_budget` times the graph build, its findings, the airwires and the board checks separately. Its assertions are unchanged.

No other expectation changed. The ESP32 reference board stays at 0 issues, and its fab output is byte-identical (`regenerate.sh`).

### Measured

On `via_board(10_000, 2_000)` (10,000 through vias on 4 layers, 2,000 tracks, the showcase's pours), `test` profile, Apple M5 Max shared with other builds, best of 5 in each of three runs (they agree within 0.3 ms):

| Work | Time | Target |
| --- | --- | --- |
| `CopperGraph::of` (40,000 via lands, the pads, 2,000 tracks, fill joins) | 5.5 ms (7.0 ms with other builds running) | ≤ 10 ms: met |
| `CopperGraph::findings` (peel, stubs, unrouted, dangling vias) | 3.7 ms | — |
| `CopperGraph::airwires` | 5 µs | — |
| `BoardCopper::checks` with a fresh graph (graph, clearance, edge, via and courtyard checks) | 23.5 ms | — |

The first version filed every land separately and built in about 30 ms; testing each outline once and joining its lands on the common layers brought it under the target. Incremental update (Q6) is not needed and was not built. The board checks with the graph (23.5 ms) cost about what the old checks did (23.2 ms, plus 5.0 ms for the separate airwire scan), and the graph now also finds stations, stubs and per-layer lands. The engine shares one build between the Issues query and the desktop editor, where there were two scans per revision. Memory was not measured.

## Slice 2 as built

Built on `component-model` at 3ce267d, 8 November 2026.

### What exists

- **Barrel lengths on the graph.** The graph holds each copper layer's foil mid-plane (`mid_planes`), and `CopperGraph::edge_length` gives every edge its length: a stretch's centre line, a barrel's mid-plane distance, 0 for joints and plated holes.
- **Net length** (`CopperGraph::net_lengths`, in `connectivity/lengths.rs`): each net's track centre lines plus, at each via, the barrel between the outermost of its lands that a track of the net joins. The length model is unchanged; only "a track reaches the via" is now the graph's joint (copper overlap, within the via's span) instead of the centre-line test. `layout::length::net_lengths` builds the graph and calls it.
- **Pair measurement** (`CopperGraph::net_paths`, `NetPaths`): one net's vertices, stretches, barrels and joints as a weighted graph, searched by Dijkstra. Where a track meets a via or pad land, the path enters the land at the track's station nearest the land's centre, unless another track meeting the land carries on from there nearer the centre (a virtual vertex splits the stretch when no station lies there). This is the old pair graph's "last leg into the pad" rule, now applied over the graph's joints and to vias too; it decides only where a joined track's length stops, never which copper is joined. Fill hops are zero-length edges, and a path using one is marked `via_fill` (`HalfPath::via_fill`, `PairPath::via_fill` in the length report, "via fill" on the High speed card's path row).
- **Endpoint selection stays in `layout/length/path.rs`** (set, derived between the farthest shared parts, longest pad-to-pad), now over `NetPaths`. The derived choice runs one search per shared part instead of one per pair of parts.
- **One graph per caller.** `layout::checks_with` passes its graph to `length::checks`; `Query::LengthReport` uses `Engine::copper_graph()` (`length::report_with`); the High speed card and the Tune tool read the engine's or the editor's graph (`signal_length_with`, `tune_target_with`). Pure callers (`report`, `signal_length`, `tune_target`, `net_lengths`) build it.
- **Bounds.** A partial graph measures pairs by total copper. The path searches of one pair settle at most `MAX_PATH_WORK` = 4,000,000 vertices (a target bound, work rather than a count of tracks or pads); past it the pair is measured by total copper.
- **The pair card** shows each half's path (length, from-pin, to-pin), the skew and the uncoupled length. Uncoupled length reads the path's own track stretches (`HalfPath::spans`), as before.

### Deleted

From `layout/length/path.rs`: the pair graph (`graph()`, `pad_joints`, `Stations`, `Meets`, `along_track`, its `Graph`, `Edge` and `Kind`, `Graph::branches`), its joining rules (centre-line joints at 1e-6 mm, the copper fallback for unjoined ends, the via and via-in-pad tests) and the `MAX_PATH_TRACKS` (2,048) and `MAX_PATH_PADS` (64) caps. From `layout/length.rs`: net length's own via-barrel scan and `mid_z`. `PairBranch`, `HalfPath::branches` and `PairRow::branches` (length report, CLI and MCP) are gone, and with them the High speed card's "Link to …" and "Branch to no pin · …" rows. No parallel joining rule remains in lengths or pairs.

Two corrections in the graph build (`connectivity/build.rs`), both about where joints sit, neither about which copper is joined:

- A track's last station is set back to its exact length after stations within 1 µm merge, so its stretches always sum to the track's length.
- **Soft end joints.** A track end that lies on another track's copper but off its centre line is a soft joint, kept only when that end is joined exactly to nothing else (the old pair graph's rule). Slice 1 joined every such end, so a leg shorter than the track width joined the track before it at its far end as well, and a path cut the leg out: the Tune tool's trombone, whose chamfer legs are 0.114 mm at 0.3 mm width, measured 0.228 mm short (`pair_tests::the_tune_tool_meanders_to_the_partner_length` failed until this was fixed; its expectation is unchanged). When dropping soft joints would leave two overlapping tracks in different islands, they join at their nearest stations instead, so islands, stubs and the checks are unchanged. Test: `a_short_leg_is_not_cut_short_through_the_track_before_it`.

### Behaviour changes and test corrections

New tests in `connectivity/lengths/tests.rs`, each with a hand-derived answer: `c1_a_pair_path_runs_through_a_mid_span_crossing` (path 13.4/17.4 of both tracks; before: no path), `c2_off_centre_vias_carry_the_path_and_their_barrels` (12 mm of track and two 1.635 mm barrels; net length counts both barrels, which the centre-line rule missed; before: no path), `c6_a_pair_path_crosses_side_by_side_overlap` (13.4 mm; before: no path), `a_path_enters_a_pad_at_its_centre` (a last leg whose corner lies on the pad, and a track overshooting the centre, both 13.4 mm), `a_path_through_a_fill_is_measured_and_marked` (10.2 mm, `via_fill`; before: no path), and the two timings below.

Changed expectations in existing tests, each a correction under the founder's ruling that a pin-to-pin link is a track between two pins:

- `tests/high_speed.rs::an_open_branch_is_not_skew_and_the_dangling_check_reports_it`: the `branches` assertions (one branch, 4 mm, no pins) go with the field. The 4 mm is asserted as the net's total less its path, and the stub's track is found among the tracks the branch added. Skew, uncoupled and `board.track_dangling` assertions are unchanged.
- `tests/high_speed.rs::a_pin_to_pin_branch_is_a_link_and_raises_nothing`, renamed `…_is_a_track_between_pins_and_raises_nothing`: the link's 4 mm and pin are asserted as the net's total less its path; "no finding" and "0 skew" are unchanged.
- `tests/high_speed.rs::an_unfinished_half_is_measured_by_its_copper`: drops `branches.is_empty()`; the rest is unchanged.
- `crates/io/tests/esp32_pair.rs`: `the_receptacle_jumpers_are_links_and_not_skew`, renamed `…_are_off_the_path_and_not_skew`, asserts the jumpers' 2.245 mm and 6.98 mm as each net's total less its path, that the report JSON has no `branches`, and that neither path runs via a fill. `real_skew_on_the_path_is_still_reported` drops `branches.len() == 2`. Paths, 0 mm skew and set endpoints are unchanged.
- `apps/pcbcraft/src/pcb_editor/pair_tests.rs::the_high_speed_card_lists_the_path_and_the_branches`, renamed `…_lists_the_paths_and_not_the_copper_off_them`: asserts the path rows, Skew and Uncoupled, and that no Branch, Link or Stub row is shown.

No other expectation changed. On the ESP32 reference board the whole length report (34 nets, the USB pair's 29.866 mm paths, 0 mm skew, 7.907 mm uncoupled, the groups) is bit-identical to slice 1's, and the board stays at 0 issues. Its fab output and `.pcbcraft` are byte-identical (`regenerate.sh`).

### Measured

`test` profile, Apple M5 Max shared with other builds, best of 5. The collinear halves are 1 mm tracks (0.49 mm at 2,048, so the chain stays within 1 m); the pair has both halves on that net, as the design's cases were run.

| Work | Before (design §3) | Now |
| --- | --- | --- |
| Pair measurement, halves of 256, 1,024 and 2,048 collinear tracks | 2.0, 18.2 and 61.2 ms (quadratic; refused past 2,048) | 0.16, 0.60 and 1.30 ms on the graph; 0.33, 1.21 and 2.55 ms with the graph's build |
| Net lengths on `via_board(10_000, 2_000)` | 26.7 ms (O(vias × the net's tracks)) | 0.28 ms on the graph; 7.1 ms with the graph's build |
| A pair of that board's two via nets (20,000 via lands per half) | not measured | 4.3 ms on the graph |

The graph's build on that board, soft joints included, measured 6.6 ms in the same run (slice 1: 5.5 ms; target ≤ 10 ms). Pair measurement is now linear in the half's copper (Dijkstra on its slice), and the engine shares the graph with the checks, so the length report and pair checks add no second connectivity build.

## Slice 3 as built

Built on `component-model` at a13196d, 8 November 2026.

### What exists

- **One rule for fill copper.** `connectivity::islands_joined(index, ring)` (in `connectivity/build.rs`) returns the islands of an indexed fill that a piece of copper joins: its copper lies inside an island, or within `JOIN_TOLERANCE_MM` (plus its curve slack) of an island's edge, or the island lies inside it. The stored fill polygon is the fabricated copper, so it carries no slack. This is the graph's joining rule for a polygon with holes. The outlines it is given come from the same constructors the graph uses: `connectivity::track_copper`, `connectivity::via_copper` and `CopperRing::of_placed_pad` (the same outline as `CopperRing::of_pad`).
- **Fill compute, step 5,** marks an island as joined when `islands_joined` finds same-net copper on the fill's layer touching it: pads on that layer, vias and tracks. Unjoined islands are removed by the island setting, as before. The probes are deleted. That covers centre-line sampling at 65 points or fewer, the solid probe at the centre and at the outline pulled 5 % inward, and the spoke probe at mid-gap and just past the gap. The thermal relief geometry itself (the obstacle and its spokes) is unchanged.
- **The graph** makes one element per island of each fresh fill (`fill::fresh`: the fill has a net and its stored key is current). It joins each one, through the net's grid and `islands_joined`, to every outline of the net with a land on the fill's layer. A track's stations on an island are each end whose copper reaches the island, else the station nearest the island's edges, found through the fill's `RegionIndex`. **Stale fills join nothing until they are refilled** (Q3, approved).
- **`fill.island_unconnected`** is a graph finding (`CopperGraph::findings`): per fresh fill, the islands with no joint. Only a fill that keeps every island can have them, because compute removes the rest by the same rule. A partial graph reports only `board.check_limit`.
- **`RegionIndex`** (`fill/geom.rs`) gained `filled` (built from stored islands) and `edges_near` (the edges in the cells a box covers). `segment_distance` uses `edges_near`. The segment-to-segment distance is one function, `geometry::segments_gap`, used by `CopperRing::gap`, `RegionIndex` and the graph.

### Deleted

- `fill::links`, `FillLink` and the graph's `fill_joints` adapter that read them.
- Fill compute's `probe_spokes`, `probe_solid`, `ray_exit`, the track centre-line sampling, the renumbering of joins after island removal, and the old `fill.island_unconnected` count from stored joins in `fill/checks.rs`.
- **From the file format:** `FilledCopper::joins` and the `FillJoin` type (`Pad` and `Via` with `connection`, `spokes` and `region`, and `Track` with `region`). Each was a stored second answer to "what does this fill join", which the graph now derives. Before removing them I checked every reader. Gerber, the 3D scene and the fill rebuild read none of them. The spoke count lost its only reader when `fill.thermal_starved` was removed (gap 17 of the ESP32 log). `connection` is a function of the fill settings, the net class and the rules. The spoke geometry is in the copper polygons. `FilledCopper` keeps `key`, `regions` and `removed_islands` (the fill card shows the count). Pcbcraft is pre-release with one format version, so files written before this change and still carrying `joins` are refused by `deny_unknown_fields`; a refill writes the new form. The validation that a join named an existing island went with the field. The MCP and CLI schemas derive from the types, so they changed with them. No hand-written schema listed the fields.

### Behaviour changes and test corrections

The one rule disagrees with the old probes wherever copper overlaps an island without a probe point inside it. New tests in `connectivity/tests.rs`:

- `a_track_overlapping_an_island_only_at_its_edge_joins_it`: an island starting 0.1 mm above a 0.3 mm track's centre line overlaps only the track's edge. Centre-line sampling removed it; it is now kept, joined and reported by nothing. At 0.16 mm it is clear of the copper and still removed.
- `a_via_land_overlapping_an_island_joins_it`: a solidly joined via whose land overlaps an island by 0.01 mm, its centre outside. The solid probe (centre, then the outline pulled 0.015 mm inward) missed it and removed the island; it is now kept and joined. Without the via, the island is removed, or kept and reported as `fill.island_unconnected` when the fill keeps every island.

These two are the fragments the ESP32 board now keeps (below): copper overlapping only a via land, or only a track's edge.

Changed tests, each a correction, none weaker:

- `fill/tests.rs::pads_vias_and_tracks_of_the_net_join_the_fill_with_their_connection_style` read the stored joins. It now asserts through the graph that every GND via and some GND track join the pour. It checks the reliefs in the copper: some via has all four spokes, no via has copper between its spokes, and the header's through-hole ground pin has copper along a spoke halfway across its relief gap and none between spokes. It also asserts that no island is unconnected. The old test only checked the first via's spokes when the stored count happened to be 4, so this is stronger.
- `a_plane_connects_what_its_track_used_to`: "stale joins are dropped" now asserts that the graph joins nothing to the stale plane and something to the fresh one.
- `a_fill_can_join_its_vias_solidly`: the stored `connection` and `region` gave way to the graph join and the copper. The solid vias join, as before. The other fill's thermal via shows exactly two spokes with clear gaps, because the 0.8 mm strip leaves no room for the two spokes across it.
- `a_large_board_fills_in_seconds`: the 5,000 joined vias are counted on the graph after the fill is stored.
- `solidcraft-integration` `board/tests.rs::the_shipped_showcase_has_its_shape_classes_and_edge_held_header` asserted a stored thermal `FillJoin::Pad`. It now finds a GND pad the pour meets through exactly the set number of spokes, each as wide as the set spoke width and no wider (probed ±0.02 mm inside and ±0.03 mm outside the spoke's edges, halfway across the gap), with no copper between spokes. It also asserts that the graph puts that pad and the pour in one island. `project/fill/tests.rs` and the desktop's `fill_tests.rs` only dropped `joins: vec![]` from their fixtures.

### The ESP32 reference board

It stays at 0 issues (`project.issues`). Its fills change by islands added, and nothing else:

| Fill | Kept / removed islands before | After |
| --- | --- | --- |
| GND plane (In1) | 1 / 0 | 1 / 0 |
| 3V3 plane (In2) | 1 / 0 | 1 / 0 |
| GND bottom | 1 / 1 | 2 / 0 |
| GND top | 9 / 25 | 29 / 5 |

No existing island was lost or altered: every old region is in the new list, byte for byte. Each of the 21 new islands is a fragment of about 0.3 × 0.4 mm, joined by copper overlap:

- 18 are thermal-relief spoke pieces at GND vias 424, 428, 436, 455, 480, 485, 487, 489 and 492, cut off from the pour by other-net clearance. Each overlaps its via's land, by up to 0.15 mm, and usually also the GND track ending at that via.
- 2 on Top overlap only the edges of GND tracks 407 and 408.
- 1 on Bottom touches a GND pad and no via or track.

Under the old probes, a spoke piece joined only if copper lay both mid-gap and beyond the gap, and a track joined only along its centre line. Under the one rule they touch same-net copper, so they are joined and kept. This is the design's row "Fill islands touched by a track's edge only: removed → kept and joined". They are electrically harmless.

In `fab/`, only the two outer copper Gerbers change: `copper-l1-top.gbr` gains 509 lines (20 regions) and `copper-l4-bottom.gbr` gains 22 (1 region). Every other fab file is byte-identical. In the `.pcbcraft`, the only changes are the removed `joins` lists (33, 5, 33 and 120 entries), the added regions and the two `removed_islands` counts; every key and every other entry is unchanged. The committed schematic PDF was left as it was.

### Open fill-policy question (not built)

- **Q9: Redundant fragments.** Should a fill drop islands that add no connection: copper joined only to things already connected without it, such as the ESP32's spoke pieces? A minimum island area would do much the same. Either belongs as a fill setting (like `IslandRemoval::BelowArea`), not as a connectivity rule. The graph keeps deciding what joins.

### Measured

Measured against a build of the base (a13196d), alternating base and new runs, on an Apple M5 Max shared with other agents' builds:

| Work | Base | Slice 3 |
| --- | --- | --- |
| ESP32 board, `fill.refill` of all four fills through `pcbcraft-cli stdio` (`ci` profile; best / median of 8, three rounds) | 81–82 / 82–84 ms | 81–83 / 83–87 ms |
| 10,000-via plane, `a_large_board_fills_in_seconds` (one fill, 5,000 joined vias; `test` profile; best / median of 6) | 1.75 / 2.0 s | 1.83 / 1.94 s |
| `via_board(10_000, 2_000)`: `CopperGraph::of` (best / median of 6) | 5.9 / 6.3 ms | 7.8 / 8.3 ms |
| The same board: board checks with the graph | 25.0 / 28.0 ms | 28.0 / 29.4 ms |

Refill time is unchanged within the noise on both boards. The first version tested each pad outline edge against every fill edge in the pad's bounds and cost about 2 ms more per ESP32 refill. Querying the fill's index once per outline edge brought fill compute's island step back to the old probes' cost: about 1.4 ms against 1.2 ms, of which about 0.6 ms is building the region index in both. Graph build costs about 1.8 ms more on the 10,000-via board, because the graph now indexes each fresh fill and joins its islands itself instead of reading stored joins. It stays within the 10 ms target.

## Slice 4 as built

Built on `component-model` at 3ce267d, 8 November 2026.

### What exists

- **The updated board's graph.** `updates::preview` builds `CopperGraph::with_pads(&next, carried)`: the staged board's copper, with every pad on the net its copper had before the update (a pad new to the board keeps its new net). Each island is then one net's copper and the pads it would join after the update. `CopperGraph::of` is now `with_pads(project, layout::pads(project))`; nothing else in the builder changed.
- **Two additive queries** in `connectivity/islands.rs`: `CopperGraph::islands()` (per island: its net, pads, tracks and vias) and `CopperGraph::copper_at_pad(index)` (the tracks and vias joined to a pad directly).
- **Conflicts, by the approved rule (Q8).** A track or via is offered for removal when:
  - its net no longer exists after the update ("Track on N would be left without its net", unchanged); or
  - its island holds a pad whose net after the update is not the copper's net, on a net the update changes electrically. Every track and via of that island is offered ("Track on N would join pads the net no longer joins"), and the conflict now names that pad, so the review's per-net line locates it.

  A net that only loses a terminal keeps its copper.
- **Detached and retired warnings** come from the two graphs. The current revision's graph is the engine's cached one, passed into `preview` and `candidate`; the "before" findings reuse it too. A track or via that joined a changed pad directly before is "detached" only when it no longer shares the pad's island after the update. Copper still joined through other copper is not warned about. A pad that leaves the board warns "A track on N loses P; it stays for rerouting" for each track that touched it.
- **Clearance** to moved pads stays a clearance test: `layout::pad_clearance_conflicts` compares each changed pad only with tracks and vias near it, through a `BoundsGrid` sized by the largest clearance.
- **The removal review** (one acknowledged line per net in `update_preview.rs`) reads `plan.conflicts`, which this graph now decides; it needed no change.

### Deleted

From `updates.rs`: the pairwise loops over every changed pad × every track and via, their `track_touches_pad` and `via_touches_pad` tests, and the 100,000-pair budget with its blocker ("Too much copper to check"). The blanket `NetChanged` marking of every track and via of a changed net is also gone. `layout::track_touches_pad` and `layout::via_touches_pad` are deleted; nothing else used them.

### Changed expectations, each a correction

- `updates/tests.rs::disconnect_and_remove_plan_wires_placements_and_conflicts_atomically`, second half. Removing R1 used to offer the shared track for removal. Now it is kept, with the "loses R1 pad 2" warning. Apply works without the acknowledgement and keeps the track. After the update, N2 is R2.1 alone, so the track joins one pad of its own net. It only lost a terminal.
- `updates/tests.rs::newly_device_declared_nc_removes_singleton_net_copper_only_explicitly` is renamed `newly_device_declared_nc_keeps_copper_that_joins_no_other_net`. Declaring R1.1 no-connect leaves it on its own net, so the track on it joins that one pad before and after. The old rule offered it only because the terminal's class changed.
- `updates/tests/review_data.rs::a_removal_is_listed_by_area_kind_and_object_with_refs`. For the same reason as the first item, the removal has no conflict. The conflict-field assertions (net, layer, 5.6 mm, `NetChanged`, ack, line) now run on disconnecting the wire, a real conflict. There the conflict names the pad on the other net (it was `None`).
- `layout/via/tests.rs::electrical_changes_to_a_net_with_vias_are_reviewed_updates`. The lone via joined no pad, so it no longer conflicts when the net splits. The test now routes R1.2 to R2.1 through two vias and keeps the lone via. The two route vias and four tracks are offered (a real conflict), and the lone via survives the apply.
- `tests/ce04.rs::ce04_two_units_share_one_package_placement_and_identity`. The stub routed off Q1.1's pad joins that pad alone. Unplacing unit A, or removing Q1, used to offer it for removal; now the stub stays, and applying the unplace keeps it. It joins no pad of another net.
- `apps/pcbcraft/src/update_preview.rs::an_update_with_only_via_conflicts_applies_once_acknowledged`. Its via moves from open board (12, 20), where it joined no pad and now stays, into R2.1's pad. R2.1 goes to another net when the wire is disconnected, so the via is a real conflict, and it is the only one.
- `layout/tests.rs::round_pad_clearance_holds_either_side_of_the_rule` asserts that the track and pad share no island of the graph, in place of the deleted `track_touches_pad`.

New tests:

- `a_split_offers_only_the_copper_joining_both_sides`: disconnecting N2 offers only the track that joins R1.2 and R2.1, naming the pad on the other net. A floating N2 track elsewhere stays; the old rule offered it too.
- `a_net_losing_an_unrouted_terminal_keeps_its_copper`: R3.1 joins N2 and then leaves it, and nothing is offered. The old rule offered the routed track.

The real conflicts stay offered: a remapped pad, a moved foreign pad (clearance), a renumbered terminal, a disconnected wire, and the vias of a split route. The ESP32 reference board stays at 0 issues, and `regenerate.sh` reproduces its `.pcbcraft` and fab output byte for byte. The schematic PDF was left as committed.

### Measured

These timings are on `via_board(10_000, 2_000)`, test profile, Apple M5 Max shared with other builds, best of 5 (`an_update_preview_on_ten_thousand_vias_stays_quick`). The update disconnects a wire of the net that has 5,000 vias:

| Work | Before slice 4 | After |
| --- | --- | --- |
| `CopperGraph::with_pads` of the updated board | — | 6.6–7.8 ms (the plain build is 7.6 ms in the same run) |
| `Engine::preview_update`, both boards' checks included | 92.7 ms | 80.6 ms |
| Offered for removal | 985 tracks, 5,003 vias (every one on the net) | 14 tracks, 3 vias (the copper joining the split pads) |

The preview builds one extra graph; it reuses the engine's graph for the current board and its findings. Most of the preview is the two boards' full checks, as before.


## Slice 5 as built

Built on `component-model` at 804cb20, 8 November 2026.

### What exists

- **Joint queries** in `connectivity/joints.rs`:
  - `CopperGraph::joints_on(track)`: every joint on a board track, in station order. Each `TrackJoint` gives the station, the point on the centre line, whether it is at an end, what it meets (`Meets::Track { index, station, end }`, `Via`, `Pad` or `Fill`) and `end_to_end` (the two tracks' ends are one point, a polyline corner).
  - `CopperGraph::joints_of_via(via)`: what joins a via on any of its lands. Its own barrel does not count.
  - `CopperGraph::islands_at(point, layer, net)`: the islands of a net's tracks, vias and pads on a layer whose copper holds a point, by the one joining rule.
  - `connectivity::keeps_joint(polyline, at)`: whether a new centre line still passes through a joint's point, so it keeps the joint. The other copper met the old copper within its half-width of that point.

  These replace the design's `joints_on`, `tracks_at`, `element_at` and `layers_at`. `tracks_at` is `joints_of_via`, filtered to tracks, and `element_at` with `layers_at` is `islands_at`.
- **Shove chains** (`router/shove.rs::adopt_line`) are read off the graph. A chain runs on through a track end only where the graph has exactly one joint there. That joint must be end to end with a track of the same width whose end meets nothing else. A joint with a fill is ignored here and below, because the pour is refilled round moved copper.
- **Pins.** Every mid-span joint on a chain's tracks (a T, a via on the track, a crossing) becomes one of the chain's pins (`Line::pins`). A shoved chain must pass through each pin, which `keeps_joint` checks. This applies to single chains (`push_line`), to both halves of a pushed pair (`pair::bend`) and to every corner shortcut of the cleanup (`tidy`), so the cleanup cannot cut a pin off. A side that loses a pin is not taken. When neither side keeps it, the push fails, and so does the shove.
- **Via followers** (`follow`) come from `joints_of_via`. The tracks joined at an end follow the via, and each end keeps its offset from the via's centre, so an off-centre end follows as well. A via that is joined to anything else does not move, because moving it would detach that copper. That covers a pad, another via and a track in mid-span.
- **Router graph.** `RouteIndex` holds the revision's graph. `RouteIndex::with_graph` takes the engine's graph (`layout.route_preview`) or the editor's (the desktop Route tool). `RouteIndex::of` builds the graph on the first shove that needs one (`layout.route_step`, tests).
- **The meander** (`meander::plan`, now given the graph) finds the track's mid-span joints with `joints_on` and goes in the longest stretch between them. Its straight lead-in and lead-out keep it a leg spacing off each joint, and the joints stay on the straight leads. When no stretch is long enough, the tuning is refused and names the joints: "Copper joins this segment at 2.5, 5, 7.5 mm from its start; tune between joints, which needs at least …". `meander::wanted` measures on the same graph. `layout.tune_length` builds the graph, `layout.tune_preview` uses the engine's, and the Tune tool uses the editor's (`PcbEditor::revision_graph`).
- **Route tool.** A click finishes the route when it lands on copper of the net, on the routing layer, in another island than the head's (`islands_at`; the helper is now `reaches_another_island`). A layer change needs no via when one island has copper at the head on both layers (`head_spans_layers`). A click on a pad of the net still finishes the route, as before.
- **Pair skew** (`router/shove/pair.rs`). `DEFAULT_SKEW_MM` = 0.05 is gone. With a class max skew, a push may add half of it (`ADDED_SKEW_SHARE`, unchanged). With none, it may add nothing beyond rounding (`meander::LENGTH_TOLERANCE_MM`, 1 µm), so a push that would add skew is refused.
- **One definition of a pad's copper.** `PadKind::copper()` (library) answers `PadCopper::None`, `OneSide` or `Through`, and `layout::pad_copper_on(kind, side, layer)` turns that into copper layers. `PlacedPad::on_copper` and `WorldPad::on` call it, and the via-to-pad layer test (`via::shares_layer`) asks `WorldPad::on`. `layout::pads` leaves out a pad with no copper, so the graph never has a land for a bare hole. The 3D model reads `PadKind::copper()` and `PlacedPad::on_copper` as well.
- **Enforcement:** `cargo xtask connectivity`, part of `cargo xtask ci` (below).

### Deleted

- `Ctx::track_ends_at` and `Ctx::joint_at` (`router/world.rs`), together with the centre-coincidence chain and follower tests in `adopt_line` and `follow`.
- The desktop's own joining rules in `joins_net_copper` and `head_joins_layers`: the same-net track and via-disc tests, and the pad test through the desktop's hit outlines.
- The meander's implicit "the ends are the only joints".
- `DEFAULT_SKEW_MM`.
- Five copies of the pad-layer rule: fill compute's `pad_on`, fabrication's `pad_on`, its test oracle's `pad_on`, the desktop's `pad_on` and the 3D model's `pad_outline::Copper` with `copper()`. The desktop's copy counted a bare hole as copper on every layer.
- `CopperRing::touches`, the last touch predicate outside the module, which only one test used. That test now checks the tangent gap directly. `TOUCH_REACH_MM`, the clearance reach in `copper.rs`, is renamed `SLACK_REACH_MM`, since it has nothing to do with touching.
- The `project` parameters that only the old route-tool helpers needed: `change_layer_to`, `next_layer` and `select_layer`.

### Enforcement: `cargo xtask connectivity`

Joining means "same net, and the copper meets". Clearance is the opposite question: "different nets, and the copper comes closer than a rule". The check (`xtask/src/connectivity.rs`) parses every crate's and app's sources outside `layout/connectivity` and skips test code. It rejects:

- the joining tolerances by name (`JOIN_TOLERANCE_MM`, `JOIN_REACH_MM`);
- a function that both compares a net for equality (`a.net == b`, `net == Some(n)`) and tests copper for contact. A contact test is a comparison with a contact tolerance (a float literal from 1e-8 up to and including 0.001 mm, the joining tolerance) or a call to `touches`, `track_contains` or `polygon_contains`.

A clearance test compares nets for inequality and gaps against a rule, so it passes. Literals below 1e-8 test float identity, such as finding a track a step just wrote by its end point or skipping a degenerate segment, so they pass too. The first version flagged three functions of that kind (`pair_tool::settle_pair`, `pair::route`, `length::pair_coupling`), which is why the lower bound exists. Counted, reasoned exceptions name functions that match the shape but decide nothing. A stale exception fails the check. None is needed today.

On the code before slices 1 to 5, the check finds D1 and D2 (`BoardCopper::checks`, `BoardCopper::airwires`), D7 (`track_ends_at`, `joint_at`) and D8's `joins_net_copper`. It misses D8's `head_joins_layers`, which compared a via's distance with its radius and called the desktop's pad hit test, and D6, which called named helpers. A rule that caught those would also flag picking code. Its unit tests cover a track-end coincidence, a point on a track of the net, a clearance test, a pick, float identity and test code.

### Behaviour changes and test corrections

Every change is a correction. No existing expectation changed: the router, pair, meander, high-speed, fill, Update PCB, desktop and ESP32 tests pass unchanged.

- **Shove with a mid-span joint.** Before, a T or a via on a pushed track was invisible to the chain, so a push could bend the track off it. Now the chain is pinned there. New tests in `router/tests/joints.rs`:
  - `a_shove_keeps_a_t_on_the_pushed_track_joined`. A 1.5 mm head pushes a run at (25–31, 19.6). Without the branch, the run's ramp back down misses (32.3, 20). With the branch it comes back down before the T and passes through it, and every track of the net stays in one island.
  - `a_pinned_chain_still_bends_elsewhere`. With the T at (38, 20), the run still bends round the head and passes through the T.
  - `a_shove_does_not_move_a_via_a_track_runs_through`. Without the through track, the via moves 0.65 mm, past its radius plus that track's half-width. With it, the via stays, the head goes round, and the via and both tracks stay in one island.
- **Meander with a mid-span joint.** Before, the meander was centred on the segment and could bump over a via, detaching it. New tests in `meander.rs`:
  - `a_meander_keeps_a_via_in_mid_span_joined`. A via at x = 30 on a 20–40 mm track: the whole-segment meander bumps over it, and the planned one keeps it on a straight lead. After `layout.tune_length`, everything is still one island, and the added length is exact.
  - `joints_too_close_together_refuse_the_meander`. Three T's at 2.5 mm spacing on a 10 mm track refuse the meander, naming the joints.
- **Route tool.** A click on copper already joined to the head, such as the start pad's fan-out, used to finish the route. It is now a corner (`a_click_on_the_routes_own_island_does_not_finish_it`). A via whose span misses the routing layer no longer counts as joining it. That case is latent, since commands refuse `ViaSpan::Between`.
- **Pair skew without a class limit.** Before, a push could add up to 0.025 mm of skew to a pair whose class set no limit, half of an invented 0.05 mm. It now adds none. `a_push_adds_skew_only_within_the_class_limit` builds a pair at 20°, off the 45° grid, where every push adds about 0.09–0.14 mm of skew:
  - with a 1 mm class limit, pushes move it and add more than 0.001 mm and at most 0.5 mm;
  - with 0.1 mm, no push adds more than 0.05 mm;
  - with no limit, no push moves it.

  Pairs on the 45° grid still shove without a limit, with skew unchanged within 1e-6 mm, as the existing pair tests assert.
- **A bare hole carrying a terminal** (a footprint error that `footprint.terminal_forbidden` reports) was a land on its side in the graph. It is now no copper anywhere (`placed/tests.rs::a_bare_hole_has_no_copper_on_any_layer`).

The ESP32 reference board stays at 0 issues (`project.issues`: 0 errors, 0 warnings). `regenerate.sh` reproduces its `.pcbcraft` and `fab/` byte for byte. The schematic PDF it rewrote was restored to the committed file.

### Measured

`route_plan_timings` (`router/tests/perf.rs`), `ci` profile (opt-level 1, dependencies 2), Apple M5 Max shared with other agents' builds, three runs each. The timing now builds the copper graph once per board and hands it to the index, as the engine and the editor do, and reports the build separately.

| Work | Before (804cb20) | Slice 5 |
| --- | --- | --- |
| Dense board (10,000 vias, 1,500 tracks), Shove: median / p95 / max over 400 moves | 333–334 µs / 3.92–3.98 ms / 6.79–6.90 ms | 336–341 µs / 3.90–4.06 ms / 6.78–6.86 ms |
| Dense board, Walkaround: median / p95 | 57 µs / 134–136 µs | 57–58 µs / 132–135 µs |
| Showcase, Shove: median / p95 / max over 200 moves | 13.6 µs / 311–317 µs / 887–899 µs | 13.8–14.5 µs / 312–318 µs / 0.93–1.03 ms |
| Showcase, Walkaround: median / p95 | 13.6–14.0 µs / 55 µs | 13.7–14.1 µs / 54–55 µs |
| Dense board: shoving moves / moves reached | 334 / 160 | 334 / 160 |
| Copper graph build (once per revision, shared) | — | dense 3.6 ms; showcase 0.5 ms |

Routing and shove speed are unchanged within the noise, and the sweep shoves and reaches exactly as before. The graph is built once per revision and shared with the checks, so the router adds no build where the engine or the editor already holds one. `layout.route_step` builds it on its first shove (3.6 ms on the dense board).

## Tolerance correction

Slices 1 to 5 built the joining tolerance as `JOIN_EPS` = 1e-6 mm, which is 1 nm, not the approved 1 µm, and the sections above that said "1 µm" were wrong about the code. The meander's `LENGTH_TOLERANCE_MM` was 1e-6 mm as well. At 1 nm, joins could break once coordinates are rounded to whole nanometres. Both are now 1e-3 mm (1 µm), and the joining tolerance is named `JOIN_TOLERANCE_MM`. Every use of it is a "same place" tolerance and now works at 1 µm: the joining rule (`build::joins`, `islands_joined`, fill stations), a track end on another track's centre line (an exact rather than a soft joint), a soft joint anchored by an exact one at the same station, stations merging along a track, a station at a track end, `keeps_joint`, end-to-end corners and the path tree's pad stations. Through `LENGTH_TOLERANCE_MM`, a tuning request for 1 µm or less is already at its target, and a pair push without a class skew limit may add up to 1 µm of skew. Float-identity epsilons (1e-9, 1e-12) are unchanged. `cargo xtask connectivity` now counts a literal of exactly 0.001 mm as a contact tolerance: before, its range stopped short of it.

`copper_joins_within_a_micrometre_and_not_beyond` puts two tracks end to end with 0.5 µm of copper gap (joined: routed, no stub) and 2 µm (not joined: unrouted, two stubs). No other test changed its result. The meander tests keep asserting the solved length to within 1e-6 mm, the solver's precision, rather than the looser tolerance.

## Where connectivity lives

Whether copper touches or joins is decided in one place: `crates/editor-core/src/layout/connectivity` (`connectivity.rs`, `build.rs`, `islands.rs`, `joints.rs`, `lengths.rs`, `queries.rs`). The rule is `build::joins`: same layer, same net, copper within `JOIN_TOLERANCE_MM` (1 µm) plus curve slack. `islands_joined` applies the same rule to fill copper. The graph is built once per revision (`Engine::copper_graph`, or `CopperGraph::of` for pure callers). Every consumer asks it:

- the board checks and Issues (`findings`, `stubs`);
- airwires and the drag preview;
- net lengths, pair paths and signal length;
- fill compute (`islands_joined`);
- Update PCB (`with_pads`, `islands`, `copper_at_pad`);
- shove chains and via followers (`joints_on`, `joints_of_via`);
- the meander (`joints_on`);
- the Route tool (`islands_at`).

Which copper layers a pad has is decided once too: `PadKind::copper()` and `layout::pad_copper_on`. `cargo xtask connectivity` rejects joining code anywhere else. What remains outside the module are clearance tests (different nets against a rule), picking (what lies under the pointer) and polyline bookkeeping (finding a track a step just wrote by its end point). None of them decides connectivity.
