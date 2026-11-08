# Push-and-shove and walkaround routing (as built)

AGENTS.md asks for routing that is planned for push-and-shove from the first implementation and delivered by release 2. This note records what was built: the three routing modes, the algorithms, the transaction, the measured timings and the limits. The implementation is original. No GPL router (KiCad's or any other) was read.

## Modes

The Route tool cycles the router with **M** (⇧M backwards). The Route card's *Router* row and the canvas pill ("Routing N2 · 0.3 mm · 45° · Shove") show the mode in use.

| Mode | What a step does about other nets' copper |
| --- | --- |
| **Shove** (default) | Pushes other nets' tracks and vias aside, recursively. Walks round what cannot move. Stops at the last legal point when no way exists. |
| **Walkaround** | Goes round tracks, vias, pads, keepouts and cutouts at the rule clearance, hugging them at 45° (or 90°). Stops at the last legal point at the board edge or when no way round exists. |
| **Ignore** | Today's behaviour: the plain posture step, which the engine refuses on a clearance violation. |

**Why Shove is the default.** Shove includes walkaround: when nothing can be pushed, it walks round the immovable copper and pushes the rest along that path, and when that fails it walks round everything. On an empty stretch it is exactly the plain step. It is therefore the more useful default, and it never commits a violation. *Shove vias* in the card, on by default, lets a shove move vias. Locked vias never move.

## One pure function, one transaction

`layout::router::plan(project, index, request) -> RoutePlan` is pure. `RouteIndex` is the board's obstacle index plus keepouts, cutouts and the board edge, built once per document revision. The plan holds:

- the edit set: the step's tracks and via, then the shoved tracks' replacements, extra or dropped segments and moved vias;
- where the head ends, and whether it reached the target;
- the mode that produced it;
- the shoved track and via identities;
- the undo edits that put the shoved copper back;
- the reason the head stopped short, if it did.

- **Desktop.** The preview recomputes the plan from the committed board on each pointer move; it is cached per revision and input, so a still pointer costs nothing. The click sends the same plan's edit set as one `layout.route_apply`. That is one engine transaction and one undo step, and it includes every shoved item. The board draws shoved tracks and vias at their new places only. The preview outlines shoved copper in the accent colour. A head that stops short gets a warning ring and a line to the pointer.
- **Spring-back.** The preview always starts from the committed board, so shoved copper returns as soon as the head moves away within the gesture (`shoved_copper_springs_back`).
- **CLI and MCP.** `layout.route_step { request }` runs the same `plan` inside the engine and applies it as one undo step. A step that cannot move the head is refused with the blocking reason, and nothing changes. Its outcome (`route_stepped`) gives the head's end, whether it reached the target and its `detour_factor`: the path over the straight line, through the via if the step places one, not counting shoved copper. A request's `max_detour` (at least 1) refuses a longer path; `layout.route_preview` then returns a plan with no edits and the reason. Desktop steps set no limit, and pair steps have neither field. `layout.route_preview { request }` returns the `RoutePlan` without applying it. The request has the net, width, layer, head, target, posture, flip, optional via and via layer, `mode` (default `shove`) and `move_vias` (default true).
- **Backspace** unwinds a step with one `layout.route_apply`. It removes the step's new tracks and via, puts shoved tracks back with their identities (`replace`, and `restore` for segments a shove merged away) and moves vias back (`move_vias`). `RouteEditSet` gained `restore` and `move_vias` for this. The engine checks both: a restored identity must be free and issued before, a locked via refuses the edit, and moved vias are clearance-checked once the tracks are in place.

The engine's `layout.route_apply` checks remain the authority. The router's collision queries use the same copper rings (`CopperRing`) and the same rules (`limits::BoardRules`, net-class and class-pair clearances, the pair-halves exception), so whatever the router finds clear, the engine accepts. The router is stricter than the engine in two places: it also keeps out of track keepouts and keeps the board-edge clearance from the outline and cutouts.

## Algorithms

**Hulls** (`router/hull.rs`). An obstacle's hull is the intersection of half-planes in eight directions (every 45°), or in four (axes only) for 90° routing. Each half-plane supports the obstacle's core grown by the clearance, the probe's half-width and a 2.1 µm margin. Any point outside the hull is therefore farther than that from the obstacle. A centre line on the hull boundary hugs the obstacle at the clearance, in the posture of the hull's edges. Two hulls merge by taking the larger support in every direction. Hulls only guide the router: every leg it keeps is checked against the exact copper.

**Walkaround** (`router/walk.rs`):

1. The head follows the posture path toward the target. When the next leg collides, the walk takes the obstacle whose hull that leg enters first.
2. It follows that hull's boundary both ways from the entry point, until the posture path to the target (either elbow) no longer enters the hull.
3. When a boundary leg collides with another obstacle, that obstacle merges into the hull and the walk retries, up to six merges.
4. Of the two senses, the shorter total path wins.
5. Each step repeats for up to 24 obstacles.
6. A final pass replaces runs of up to ten corners with clear posture paths, so the path hugs instead of following the octagons.
7. The board edge, a target inside a hull, or an exhausted budget stops the head at the farthest legal point of the blocked leg, found by bisection.

**Shove** (`router/shove.rs`):

1. The step's copper is fixed. Each collision with movable copper pushes it, and each piece that moved pushes in turn, breadth first.
2. Limits: depth 8, 96 single pushes, and 1,500 collision queries per shove.
3. A pushed track moves as its chain: tracks of one net, layer and width, joined end to end where no via, pad or third track meets. The [copper graph](copper-graph.md) decides this: a chain runs on through a track end only where that end has exactly one joint, end to end with another track whose end meets nothing else (a fill joint aside, since the pour is refilled round moved copper). The chain's ends stay put. So do its **pins**: every point where other copper joins one of its tracks in mid-span (a T, a via on the track). A result, and every corner shortcut of the cleanup, must pass through each pin; a side that does not is not taken.
4. The part of the chain inside the pusher's hull is replaced by the hull's boundary on one side or the other. The chain uses octagons for 45° and free chains and rectangles for 90° chains, so it keeps its posture.
5. The side whose result is clear of immovable copper wins, the shorter if both are. Immovable copper that a side would touch merges into the hull, and the choice is made again, up to four times.
6. Cleanup: the pushed span (and only that span) is tidied. Short legs are dropped, collinear legs merged and loops cut, and corners are replaced by posture paths that touch nothing. A result with an acute corner is refused.
7. A pushed via moves the least distance, in a 45° multiple, that clears the pusher. The chains ending on it follow with a 45° elbow that keeps their last leg's direction, each end keeping its offset from the via's centre. A via that the copper graph joins to anything else (a pad, another via, a track in mid-span) does not move: moving it would detach that copper.
8. Never moved: pads, keepouts, the board edge, locked tracks and vias, the routed net's own copper, and vias when *Shove vias* is off. A push into any of these fails the whole shove.

**Shove fallbacks** (`router::shove_plan`), in order:

1. Shove the straight step.
2. Walk round only the immovable copper, then shove along that path.
3. Walk round everything.
4. Shove along the straight step as far as room allows: the longest legal prefix, by bisection to 1/256 of the step. If the walkaround got nearer the target, its legal prefix wins instead.

No branch returns copper that collides.

**Differential pairs** (`router::pair_plan`). In Shove, both halves' tracks and vias are the fixed copper of one shove, and both nets are frozen. The fallbacks are a pair walkaround, then the longest pushable prefix. Pair walkaround walks the centre line as one envelope of 2 × width + gap and lays both halves along it with `pair::step_along`. It tries both elbow senses, plus a lead straight out of the head for two pitches so halves leaving pads fan out before the first bend. The halves are checked as the engine checks them, the gap between them at the pair rule. A layer change keeps its straight legs and is cut back short of the vias when it collides.

### Differential pairs as the pushed copper

A pair is pushed as one unit or not at all (`router/shove/pair.rs`). Pushing each half as its own chain, as Shove did before, let the halves take separate sides and bumps: a track across the ESP32 board's USB pair took its coupled length from 21.96 mm to 2.30 mm.

**The choice: move whole, else refuse.** Refusing alone would be safe, but a pair could never give way, so a dense board would stop at every pair. Moving whole needs no separate geometry because the single-chain push already works on one polyline:

1. The coupled window round the pushed segment is found: segment by segment, the halves run parallel in one direction at one distance, within the coupling check's gap tolerance.
2. Its centre line bends round a hull grown by half the pair's whole width, as one chain, with the same sides, merges and cleanup as a single chain. The window's ends keep their direction for one pitch, so the halves join their fan-outs where they left them.
3. Both halves are laid back along the new centre line at their old distance, with mitred corners. Coupling and gap hold by construction. Both halves are then checked at the exact clearances, and pushed copper pushes on as usual.

On a 45° grid the halves' extra lengths cancel across a bend's opposite corners, so skew does not change. Off the grid they cancel only roughly, and one push may add at most half the class's max skew. A class that sets no max skew allows no added skew beyond rounding (1 µm): no limit is assumed that neither a source nor the user set. (Before slice 5 of the copper graph an invented 0.05 mm stood in for a missing class limit.)

**Refused, with the pair left where it was:** a pusher reaching a stretch where the halves do not run coupled (a fan-out, a meander, a half with no partner yet), the window's end inside the pusher's hull, a bend tighter than the pitch, a locked track, a partner being routed on its own, or any via of a pair. The usual fallbacks then apply: walk round, or stop at the last legal point. `RoutePlan.note` and `PairPlan.note` say "Pair USB can't move as one here, so it stays put"; `layout.route_preview` returns it, `blocked` repeats it when the head stops short, and the desktop paints it at the pointer in the warning colour.

Shove of copper that belongs to no pair takes the same code path as before and its results do not change.

## Keys

The Route tool's keys (`ROUTE_KEYS`, registry commands in the Route context read through `crate::keys`) gain two: `route.router-mode-next` (**M**: Shove → Walkaround → Ignore) and `route.router-mode-previous` (**⇧M**), one command per cycle direction like the other Route cycles. Both are rebindable in Settings and listed in the ⌘/ sheet and the hold-⌘ panel. Neither M nor ⇧M is bound in the Layout or global contexts, so they shadow nothing. *Shove vias* has no key; it is a Route card check.

## Tests

Core (`layout/router/tests.rs`, `hull.rs`, `clean.rs`). Every proposal goes through `layout.route_apply`, and the board checks must find nothing new:

- walkaround round a via (hugging at 0.20–0.25 mm), round a track's nearer end, past R1's and R2's pads at exactly pad + clearance + half-width, and round a keepout; it stops inside the board edge;
- shove of one track: centre lines 0.5 mm apart over the head, ends and identity kept, 45° jogs;
- shove of a chain of three tracks of two nets;
- via shove: straight up, and the attached track follows; with via shoving off the via stays and the head walks round;
- locked tracks and pads make the shove fall back; a locked wall stops the head at 0.5 mm before it;
- spring-back, and previews never touching the board;
- posture cleanup: a 90° chain stays on the axes with no collinear neighbours; a 45° chain keeps 45° multiples with no acute corner;
- clearance always honoured over 30 random fields of vias and 45° tracks, in both modes (at least 100 steps; at least half arrive);
- a coupled pair shoving a track and walking round a via;
- a pair pushed as one unit (`tests/pair_shove.rs`): a track ending between the halves, a sweep of 240 heads across the pair, a pushed track passing the push on, a pair that cannot bend held with a message, a pair via that stays where another net's would move, a half being routed that leaves its partner alone, and a pair off the 45° grid that a push moves only within half its class's max skew, and not at all without one (`a_push_adds_skew_only_within_the_class_limit`). On the real board, `crates/io/tests/esp32_pair.rs` sweeps 240 steps across the USB pair;
- mid-span joints (`tests/joints.rs`): a T on a pushed track pins it, so the run bends round the head and comes back through the T, which a push without the T would have left in open board; a far pin leaves the rest free to bend; a via with a track running through it stays put, where without that track the push would have moved it clear of it;
- `layout.route_step` as one transaction: one revision, undo and redo restore every shoved track and via exactly, the preview query agrees, the plan's undo edits restore the board, and a step that cannot move is refused without change;
- Ignore proposes exactly `posture::step`.

Desktop (`pcb_editor/shove_tests.rs`), headless through the canvas:

- M cycles the mode, which the card and the pill show;
- a click in Shove sends the shoving `layout.route_apply` and the preview hides the shoved track's old place;
- Backspace puts the shoved track back;
- Walkaround goes round, and Ignore is refused by the engine.

## Performance

Measured on Apple silicon (this checkout, release build), with `cargo test -p pcbcraft-core --release route_plan_timings -- --ignored --nocapture`:

| Board | Index build | Walkaround per move (median / p95 / max) | Shove per move (median / p95 / max) |
| --- | --- | --- | --- |
| Showcase (10 connections × 20 pointer positions) | 0.12 ms | 0.012 / 0.048 / 0.10 ms | 0.012 / 0.27 / 0.73 ms |
| Dense: 10,000 vias at 1.27 mm and 1,500 tracks, 140 × 160 mm (400 positions sweeping across channels) | 4.4 ms | 0.049 / 0.12 / 0.15 ms | 0.29 / 3.2 / 11.6 ms |

The target is 16 ms per pointer move, one frame at 60 Hz. Both boards meet it with room: the worst dense-board shove is 11.6 ms. The budgets are counted (collision queries), not timed, so a preview and its commit always agree. The index is built once per revision. `a_dense_board_routes_legally_within_bounds` runs on every build: it applies every dense-board proposal through the engine and bounds each plan at 2 s, a runaway guard that also holds in debug builds.

## Track drag with shove (planned)

Dragging an existing segment or corner in Select is not delivered. The plan:

1. A drag adopts the segment's chain as a `Line`.
2. It replaces the dragged segment with one offset along its normal (or the corner with a moved vertex), keeping the neighbours' directions with 45° joins, as the via-follow elbow does.
3. It runs the same `shove::shove` with the new segments as the fixed copper and the chain's net frozen.
4. The preview and release commit share a pure `router::drag_plan`, and release sends one `layout.route_apply`.

The seam (`Line`, `State`, `shove`, the undo edits) is in place.

## Limits and follow-ups

- **Convex hulls.** Merged obstacles become one convex hull, so walkaround does not thread between close obstacles whose merged hull swallows the gap. On the dense via grid, walking across the track-filled channels mostly stops short: 6 of 400 positions arrive, against 90 for Shove. A visibility search over hull corners would route through such gaps.
- **Board outline.** A concave outer outline is not hugged; the head stops at it. Cutouts and keepouts are hulled convexly.
- **Fills.** Fills are not obstacles and never move; a refill flows round the new copper, as before.
- **Pair moved as a unit.** A pair moves only inside its coupled window; the window's fan-outs, its vias and any meander on it stay. A push has no length limit beyond the rules: a head far past the pair drags it along in the preview, coupled, and a length group warns if that breaks its target. `layout.route_step`'s outcome does not carry the note; `layout.route_preview` does.
- **Shove scope.** A chain's ends and pins never move, so a pusher reaching a chain's end at a pad or via, or a mid-span joint, fails rather than dragging the joint. A via moves only when it is the obstacle hit, not to make room for a chain. Shoves stay on the pushed copper's layer.
- **Stateless preview.** Each move plans from the committed board. A head swept along a long track re-decides the push side each frame from geometry; the shorter clear side wins, which is stable in practice but not hysteretic.
- **Pairs.** The pair router commits through `layout.route_apply` with the plan's edits. There is no serialisable `layout.route_pair_step` for CLI and MCP yet; `layout.route_pair` routes a whole pair without shove. The fan-out limits of [high-speed routing](high-speed-routing.md#limits-and-follow-ups) remain.
- **Track drag** is planned, above.
