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

- **Desktop.** The preview recomputes the plan from the committed board on each pointer move; it is cached per revision and input, so a still pointer costs nothing. The click sends the same plan's edit set as one `layout.route_apply`. That is one engine transaction and one undo step, and it includes every shoved item. The board draws shoved tracks and vias at their new places only. The preview draws the routed step at its width in `preview_add` and outlines shoved copper round its edge in the accent colour, so it reads over a pour of its own layer (`hovering_across_the_showcase_bundle_previews_the_shove`). A head that stops short gets a warning ring and a line to the pointer.
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

**The head's corner** (`router/corner.rs`, every mode). The head is usually the end of the track the route came along, a corner placed by a click. Clicks snap to the 0.5 mm grid, and fine-pitch pins are often off it (USB-C pins sit at x.25 and x.75), so the corner lands past the pin's column. The step from there used to run back into the pin. Flipped, that was a 45° hook (an acute corner). Unflipped, it was a knee that passed the pin and came back. Now, when the step's path has a leg running back against the incoming track, the corner moves back along that track:

1. The candidate corners are the target's foot on the track (a 90° turn), and in 45° and 90° routing each point from which an axis or diagonal of the posture, turning by at most 90°, runs straight to the target. The candidate nearest the head is tried first.
2. The first candidate whose step reaches the target, with no leg turning back and no acute corner, wins. The track is shortened to it in the same edit set: one transaction, and one undo step that Backspace also takes back.
3. The track must be a board track of the step's net on its layer, unlocked, with an end at the head. Every joint on it (a pad, a via, another track) stays on the shortened part, so a head at a pad, a via or a branch never moves.
4. Steps with a layer change are left as they are.
5. When no candidate works, the plain step stands. This happens when the target lies behind the track's start, or when the head is boxed in so that the only way out turns back.

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
- Ignore proposes exactly `posture::step` (from a head with no incoming track to move);
- entry into pads (`crates/io/tests/route_entry.rs`, on the ESP32 board's USB-C receptacle J1 at 0.5 mm pitch): steps from 216 heads round CC1's and CC2's pins, at every 5° and three distances, in both 45° and 90° elbows. Then two-step routes from R4 and R5 through every grid corner between the resistor and the pin. No step into a pin has a leg that runs back or a corner sharper than 90°. A corner past the pin's column moves back to it;
- the corner moved back on the demo board: down from a track running right, past its end, moves the corner back. No turn is sharper than 90°, and the track shortens in the same edit set;
- track drag (`tests/drag.rs`): a straight run between two 45° tracks moves up 1 mm and both neighbours stretch to meet it with their far ends and identities kept; an end at a via jogs at 45° and a free end follows; another net's track is shoved and springs back, and previews never touch the board; a locked track stops the drag at 0.5 mm centre to centre, and Walkaround never pushes; `layout.track_drag` is one revision that one undo restores, the preview query agrees, and a drag that cannot move (or a non-finite offset) is refused without change.
- pair drag (`crates/automation/tests/esp32_pair_drag.rs`, night finding F7): on the ESP32 board, dragging USB_DN's long run 1 mm moves USB_DP's run by the same shift with no rule error or pair finding and the pair within its class limits, one undo restoring the file exactly; the preview plan lists both halves; with USB_DP's run locked the drag is refused naming the pair, while a short run moved 0.3 mm away from its locked partner moves alone, cleanly; the same drag twice gives the same board; `layout.track_drag` sent as JSON through the session gives the engine's board. Each failed before the change.

Desktop (`pcb_editor/shove_tests.rs`), headless through the canvas:

- M cycles the mode, which the card and the pill show;
- a click in Shove sends the shoving `layout.route_apply` and the preview hides the shoved track's old place;
- Backspace puts the shoved track back;
- Walkaround goes round, and Ignore is refused by the engine;
- the Route tool from the showcase's U1 pad 4 hovered down across the U1 → U2 bundle shoves it in the preview only, and it springs back (`hovering_across_the_showcase_bundle_previews_the_shove`); the preview outlines shoved copper in the accent;
- a Select drag of a track (`pcb_editor/track_drag_tests.rs`) shoves the other net's track live, springs it back, and on release sends one `layout.route_apply` that one undo takes back; Esc cancels it; on the ESP32 board, dragging USB_DN's long run draws USB_DP's run moved too and release sends both (`dragging_one_half_of_the_usb_pair_previews_and_moves_both`);
- (`pcb_editor/route_entry_tests.rs`) the founder's hook: on the ESP32 board, R5 pad 1, a corner click that snaps past J1 B5's column, then B5, with the last elbow flipped or not. The route enters B5 straight up from the moved corner. Backspace after a moved corner restores the shortened track exactly.

## Performance

Measured on Apple silicon (this checkout, release build), with `cargo test -p sparksmith-core --release route_plan_timings -- --ignored --nocapture`:

| Board | Index build | Walkaround per move (median / p95 / max) | Shove per move (median / p95 / max) |
| --- | --- | --- | --- |
| Showcase (10 connections × 20 pointer positions) | 0.12 ms | 0.012 / 0.048 / 0.10 ms | 0.012 / 0.27 / 0.73 ms |
| Dense: 10,000 vias at 1.27 mm and 1,500 tracks, 140 × 160 mm (400 positions sweeping across channels) | 4.4 ms | 0.049 / 0.12 / 0.15 ms | 0.29 / 3.2 / 11.6 ms |

A pair drag on the ESP32 board's USB pair (`pair_drag_preview_timings` in `crates/automation/tests/esp32_pair_drag.rs`: 60 offsets from −3 to +3 mm, optimised test profile) costs 1.95 / 4.68 ms per move (median / max), most of it the pair-finding check on the result. A track drag on the showcase (`track_drag_timings`: every unlocked top-layer track through 20 offsets from −1 to +1 mm, 700 moves, 380 of them moving more than the dragged track) costs 0.002 / 0.079 / 0.36 ms per move (median / p95 / max), so it plans on the UI thread like a route step.

The target is 16 ms per pointer move, one frame at 60 Hz. Both boards meet it with room: the worst dense-board shove is 11.6 ms. The budgets are counted (collision queries), not timed, so a preview and its commit always agree. The index is built once per revision. `a_dense_board_routes_legally_within_bounds` runs on every build: it applies every dense-board proposal through the engine and bounds each plan at 2 s, a runaway guard that also holds in debug builds.

## Track drag with shove

With **Select**, pressing on a track and dragging moves that segment along its normal with the pointer (in 0.01 mm steps). `layout::router::drag::drag_plan(project, index, request) -> RoutePlan` is pure, and the desktop preview, the release, `layout.track_drag` and `layout.track_drag_preview` all call it:

1. The segment keeps its direction and moves by the request's `offset` (mm, positive to the left of start → end).
2. At each end, a neighbour that the copper graph joins end to end, and that is the only joint there, stretches along its own direction to meet the moved segment, keeping its far end, its identity and its mid-span joints. An end nothing meets moves with the segment. Any other end (a pad, a via, several tracks, a locked or parallel neighbour) stays where it is and joins the moved segment with a jog at 45° to it (90° in the orthogonal posture). A track with copper joined in mid-span does not drag.
3. The new copper is the fixed copper of the same `shove::shove` a route step runs, with the dragged net frozen and the copper it replaces hidden. Walkaround never pushes and Ignore does not check, as for a step; the mode and *Shove vias* are the Route tool's.
4. When no room exists the segment stops at the last legal offset (bisected, as a step stops short) and the plan says why; a drag that cannot move at all proposes nothing.
5. A differential pair's track whose partner runs coupled beside it (a coupled run of the pair's coupling check) moves with that partner segment by the same shift, as one gesture and one plan: both nets are frozen, both halves' neighbours stretch, and where both halves' matching ends jog the two jogs are mitred as one bend of the pair's centre line (a short straight lead on the inner half), so the halves keep their width and gap. When the partner cannot follow (locked, held in mid-span, not coupled at that spot, the pair move adds a pair finding, or no room for both), the dragged half moves alone only if the pair gains no skew, uncoupled-length or gap finding (`length::checks` on the result); otherwise the drag is refused with a reason naming the partner and the pair, e.g. "USB_DP can't follow (the track is locked), and moving USB_DN alone breaks pair USB: halves differ by 0.828 mm; …". Moving alone within the limits was chosen over always refusing so a small tweak to one half (a fan-out leg, a short run) still works where its partner cannot follow; nothing is ever left with a clearance error.

The plan's `end` is the moved segment's midpoint, and `shoved` lists the dragged and stretched tracks with other nets' pushed tracks. The desktop draws the dragged net's moved copper in `preview_add` and shoved copper outlined in the accent, from the committed board on every move, so pushed copper springs back; a drag that stops short gets the warning ring and a line to the pointer. Release sends the edit set as one `layout.route_apply`, one undo step with every moved item, and Esc cancels. `layout.track_drag { request }` applies the plan as one undo step and reports `track_dragged` (the offset reached and whether it is the requested one); a drag that cannot move is refused with the reason, and nothing changes. The request has the track, `offset`, `posture` (default 45°), `mode` (default `shove`) and `move_vias` (default true).

Dragging a corner (one vertex of a chain) is not built.

## Re-routing dragged copper

When parts move, by `layout.move` (a drag or a turn) or because a board parameter or a dimension moved them, `dimensions::carry` carries the copper on their pads ([board sketch references](board-sketch-references.md#52-dimensioned-parts)). It then hands the carried tracks and vias to `layout::router::reroute::dragged`, inside the same operation, so the re-route is part of the one undo step and every surface (desktop, CLI, MCP, the update preview) shares it.

1. **What is re-routed.** Carried copper that is too close to other copper or a keepout (the router's `hits`, which the engine's checks agree with) or no longer reaches a pad it joined; unlocked copper the operation left in place that the moved parts' pads now come too close to (clear before); and, when the pair now breaks its class's uncoupled-length or skew limit, a pair half's carried track that ran coupled with its partner before and no longer does, with the partner's tracks it ran beside. Everything else stays as carried.
2. **Connections.** One net's carried copper joined into one piece (by the copper graph) is a connection. A pair's two connections are one job. Jobs run in order of their smallest identity, each on the board the previous left.
3. **Keep what is fine.** First only the offending tracks and vias are removed. The places the removed copper met (pads, vias, kept track ends) are joined nearest first where the remaining copper does not join them already, one route step each: Shove on the 45° posture, from either end with either elbow. A pair's two gaps are one pair step (`pair_plan`) at the pair's width and gap, fanning out of one end and into the other. When that fails, all of the job's carried copper is removed and routed the same way.
4. **No route.** A job whose copper offends a rule and that cannot be routed has its carried copper removed: the connection is left unrouted (an airwire). A pair re-routed only for its coupling keeps the new route only when it runs less uncoupled, with no more skew than it had or its class allows; otherwise it stays as carried.
5. **The report.** `Engine::rerouted` names each re-routed and unrouted connection by net. The desktop toasts its line ("Re-routed 2 connections", "Re-routed 6 connections; 1 left unrouted (USB_DP)"), with Undo; `project_apply` and `project_apply_all` replies (CLI and MCP) carry it as `rerouted` (`rerouted`, `unrouted` and `summary`) when there is one.

Every attempt is a pure function of the board, so the same edit gives the same board.

Tests: `crates/automation/tests/esp32_reroute.rs` on the ESP32 board: 40 → 41 mm keeps the USB pair coupled (before: 21.03 mm uncoupled); a pair re-routed by hand stays clear at 40 → 42 mm (before: 6 clearance errors, pair gap, skew and uncoupled findings, the R7-1 case); U2 turned 37° re-routes VBUS and GND (before: 4 clearance errors); the same edits twice give the same board; the reply carries the report. Each checks the refilled board for rule errors and the pair's class limits, and that one undo restores the file exactly. `layout/router/tests/reroute.rs`: a track whose part moves into a closed box of keepouts is left unrouted with no finding, and undo restores it. The desktop toast is `app::flow_tests::a_reroute_after_a_board_edit_is_one_toast`. J1 held 2 mm from the edge now re-routes VBUS and GND, which the carry alone left with 3 clearance errors, so `esp32_dimension_model` compares that update with a fresh build as a set of bodies, as it does for the widened board.

Measured with `cargo test --release -p sparksmith-automation --test esp32_reroute reroute_timings -- --ignored --nocapture` (Apple silicon, the whole edit through the engine, 15 runs):

| Edit | Re-routed | Edit with re-route (median / max) | Carry only, before this change (median / max) |
| --- | --- | --- | --- |
| Width 40 → 41 mm | USB pair | 8.9 / 13.3 ms | 3.4 / 7.8 ms |
| Width 40 → 42 mm (nothing offends) | none | 4.9 / 5.1 ms | 3.3 / 3.4 ms |
| Width 40 → 42 mm, pair routed by hand | USB pair | 8.0 / 8.4 ms | 3.4 / 3.8 ms |
| U2 turned 37° | VBUS, GND | 10.1 / 10.4 ms | 2.6 / 2.7 ms |

The re-route costs 4–8 ms on this board; checking a move that needs none costs about 1.6 ms. An extreme case, the module U1 turned 37° (nearly every connection offends), took about 0.3 s in the optimised test profile. The edit runs once per commit, not per pointer move: a part drag previews its airwires (`carried_airwires`) and re-routes on release.

## Limits and follow-ups

- **Convex hulls.** Merged obstacles become one convex hull, so walkaround does not thread between close obstacles whose merged hull swallows the gap. On the dense via grid, walking across the track-filled channels mostly stops short: 6 of 400 positions arrive, against 90 for Shove. A visibility search over hull corners would route through such gaps.
- **Board outline.** A concave outer outline is not hugged; the head stops at it. Cutouts and keepouts are hulled convexly.
- **Fills.** Fills are not obstacles and never move; a refill flows round the new copper, as before.
- **Pair moved as a unit.** A pair moves only inside its coupled window; the window's fan-outs, its vias and any meander on it stay. A push has no length limit beyond the rules: a head far past the pair drags it along in the preview, coupled, and a length group warns if that breaks its target. `layout.route_step`'s outcome does not carry the note; `layout.route_preview` does.
- **Shove scope.** A chain's ends and pins never move, so a pusher reaching a chain's end at a pad or via, or a mid-span joint, fails rather than dragging the joint. A via moves only when it is the obstacle hit, not to make room for a chain. Shoves stay on the pushed copper's layer.
- **Stateless preview.** Each move plans from the committed board. A head swept along a long track re-decides the push side each frame from geometry; the shorter clear side wins, which is stable in practice but not hysteretic.
- **Pairs.** The pair router commits through `layout.route_apply` with the plan's edits. There is no serialisable `layout.route_pair_step` for CLI and MCP yet; `layout.route_pair` routes a whole pair without shove. The fan-out limits of [high-speed routing](high-speed-routing.md#limits-and-follow-ups) remain.
- **Track drag.** A dragged segment does not walk round what it cannot push; it stops at the last legal offset. Corner drags are not built. A pair drag whose ends jog needs an offset of at least about 0.13 mm (USB on the ESP32 board) for the inner half's mitred jog to have a length; below that the half alone is tried and usually refused. There is no modifier to drag one half of a coupled pair on its own.
- **Re-routing dragged copper.** Gaps are routed on their own layer only: a gap whose ends are on different layers, or a removed via that joins other copper, is not routed (so a carried via too close to another net is removed, and its connection left unrouted). A pair is routed as a pair only when each half leaves one gap; otherwise its halves are routed one by one. A turned part's fan-out is re-routed by the same steps, not re-planned as a fan-out.
