# One copper graph (design)

Design for review, 8 November 2026. Base: `component-model` at 32116be. Approved by the founder with all its recommendations. Slices 1 and 2 are built; see [Slice 1 as built](#slice-1-as-built) and [Slice 2 as built](#slice-2-as-built).

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
3. their copper overlaps or touches: the exact distance between the cores, less both radii, is ≤ `JOIN_EPS` (1e-6 mm), plus the curve slack of any flattened custom arc.

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

1. **Q1: Join tolerance.** I propose exact geometry with a 1 µm tolerance (`JOIN_EPS` = 1e-6 mm), with curve slack only for custom arc pads. *Recommendation: yes.* It is far below fabrication resolution, and it replaces three different tolerances (1e-8, 1e-6, and centre-line rules).
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
- **The one joining rule** is `connectivity::build::joins`: same layer, same net, exact copper within `JOIN_EPS` = 1e-6 mm plus curve slack. Tracks and via lands are exact capsules and discs (`CopperRing::capsule`). Each outline is tested once per same-net neighbour, and a joint is made on every layer both outlines have a land on.
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

- **`fill::links` → `FillLink { fill, region, members }`.** The graph reads fresh fill islands from the joins fill compute stored, and finds where a joined track meets the island from the stored island polygon (each end within reach, else the nearest station). Slice 3 removes it, when fill islands join by the one rule.

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
