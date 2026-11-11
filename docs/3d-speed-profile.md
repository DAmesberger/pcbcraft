# 3D build speed on the SMLib kernel: profile and options

Measured on 8 November 2026 to let the founder choose an approach. Nothing was fixed. Build 062990a (`component-model`), Solidcraft fork 21ac01f4a (solidcore a445bdaa), `ci` profile, with Pcbcraft's default `Validation::Off`. Host: Apple M5 Max, 18 cores, 128 GB, load average 5–7 from other agents' builds, so single runs vary by about ±10 %. Everything ran headless; no app was started.

## Numbers first

| What | Now | Where the time goes |
| --- | --- | --- |
| ESP32 board, open in 3D (full build) | **183–214 s** (5 runs) | **173 s (95 %) is 4 copper fills**, each one extrusion of one sketch region: 56, 44, 44 and 28 s |
| The same board without its fills | **5.5 s** | parts 1.4 s of fillets, 5 laminate slabs 0.7 s, meshing, 860 small features |
| Rotate SW1 by 90° | **247 s** (undo 207 s) | a full rebuild: a `-0` in a joint expression stops the in-place update |
| Rotate SW1, with that `-0` removed (experiment) | **0.1 s** | one parameter changed, nothing re-evaluated |
| …then the fill the turn made stale is refilled (what the app does next) | **135 s** | that one fill extruded **twice** (65 s each) |
| Make one outer track 10 µm wider | **205 s** | all 4 fills re-extruded: they come after the tracks in the timeline |
| The same, with the fills moved first in the timeline (experiment) | **3.1 s** | the tracks and laminate after the edit |
| 1,000 extra vias on the ESP32 board, no fills | **52 s** | 5 laminate slabs, 9.2 s each |
| One extruded region of N edges | 250: 0.2 s · 1,000: 3.3 s · 2,000: 17 s · 4,000: 75 s | grows as about N^2.2 |
| Old kernel (truck), same board and benchmark | **3.4 s** build · track edit 0.8 s · 1,000 extra vias 2.5 s | every fill extrusion under 28 ms; a 4,000-edge region 35 ms |

**The cause is one function.** Every slow feature is an extrusion of a large planar profile. In solidcore, `create_linear_sweep` builds the side faces and then stitches them. Each stitch edit rebuilds the notification records of the whole B-rep, so the work is quadratic in the number of faces. Pcbcraft hits it with copper fills (2,500–3,500 side faces each) and laminate slabs (one face per drilled hole).

**Three Pcbcraft-side defects multiply it:**

1. A rounding `-0` turns a 0.1 s part rotation into a full rebuild.
2. The fills sit after the parts, pads and tracks in the native timeline. Solidcraft's per-feature cache key includes the digest of the whole state before the feature, so any edit to a part or track re-extrudes every fill.
3. A redrawn fill is evaluated twice when it is updated in place.

## 1. Where the 181 s goes

The benchmark is `crates/automation/tests/build_profile.rs`, which is committed with this note. It reads `examples/esp32-s31-devboard`, builds it with `project::build_with` as the app's model worker does, and prints:

- each Solidcraft feature kind's evaluation time (`FeatureResult::ms`);
- the faces and edges of the slowest features' bodies;
- Solidcraft's own span profiler (`solidcraft-profile`, compiled in for this test only through `pcbcraft-solidcraft`'s `profile` feature).

```
nix develop -c cargo test --profile ci -p pcbcraft-automation --test build_profile -- --ignored --nocapture
```

Its environment variables pick the edit, the variant (`no-fills`, `parts=N`, `addvias=N`, …) and the synthetic `extrusion_scaling` test. The function-level profile is macOS `/usr/bin/sample` at 1 ms on the test binary, over a 202 s build: 159,931 samples on the build thread. The 18 `solidcraft-kernel` worker threads were idle almost all of that time, so the build runs on one core, as the VM showed.

**Counts (ESP32 board):**

- 20 placements, 201 tracks, 55 vias, 4 fills.
- 884 native features: 334 extrudes, 334 sketches, 175 boxes, 18 fillets, 10 rectangular patterns, 4 combines and 9 construction features.
- 977 SMLib extrusions (one per profile piece).
- 13 SMLib Booleans, all inside the part templates' combines (27 ms in all).
- 1,157 evaluated world bodies, and 1,927 drawn bodies with the display layers.
- Each of the 5 laminate slabs is one sketch with about 110 drilled holes (55 vias and the pad drills), extruded once. No hole is cut by a Boolean.

**By phase (unsampled run, 182.6 s wall):**

| Phase | Time | Notes |
| --- | --- | --- |
| Pcbcraft building the native document (its own code) | ~0.5 s | `build_on` outside native calls: pads, contours, fills' loops, identity |
| Solidcraft command handling (self) | 0.85 s | 2,600+ commands; `sketch.create` alone is 0.28 s |
| Solidcraft `doc evaluate` (self) | 0.6 s | timeline bookkeeping |
| **Extrude features** | **177.1 s** | |
| · 4 fill regions (`fill_640…643_0`) | **172.8 s** | 3,517 / 3,169 / 3,143 / 2,547 faces → 56.3 / 44.4 / 43.8 / 28.4 s |
| · 5 laminate slabs | 2.3 s | the 2 inner foil levels are in pieces (0.9–1.0 s each); the 3 dielectric slabs take 0.14 s each |
| · 201 tracks, pads that are not boxes, barrels, module antenna | ~2.0 s | |
| Fillet features (part templates' edge rounds, one per package) | 1.4 s | |
| Box, sketch and combine features | 0.3 s | |
| Kernel meshing (`kernel mesh`, all threads) | 3.9 s CPU | about 1.8 s on the build thread (meshes features read during evaluation), the rest in parallel in `finish_with` |
| Display layers (mask, finish, silkscreen, edge), from 2D data | ~0.1 s | on their own thread, alongside the laminate |
| Render upload and first frame | not re-measured | `esp32_display_timings` needs the GPU app crate; it was measured at 8–37 ms on a 10,000-via board ([editor evidence](editor-evidence.md)) |

**By Solidcraft span category, same run:** solidcore 178.2 s (96.9 %), kernel 3.9 s, command 0.85 s, doc 0.6 s, feature 0.3 s. The one span `solidcore smlib_extrude` is 195.6 s of the 202 s sampled run.

**Top functions on the build thread, inclusive (sampled run; % of 159,931 samples):**

| Inclusive | Function |
| --- | --- |
| 99.3 % | `solidcraft_doc::eval::Model::evaluate` |
| 98.1 % | `solidcraft_kernel::build::extrude_with` → `smlib_sweeps::imp::extrude` |
| 96.7 % | `solidcore::sweep::api_brep::create_linear_sweep` |
| 95.1 % | `pcbcraft_solidcraft::project::fill::build` → `Native::batched_body` (the fills) |
| 73.3 % | `solidcore::topology::brep::p5::stitch_and_orient_with_identity` → `heal::stitch … do_stitching` |
| 46.6 % | `heal::stitch::Stitch::edit_glue_vertices` |
| 44.9 % | `topology::brep::p3::with_kernel_callbacks` (one call per glue edit) |
| 42.8 % | `topology::topology::NotifyRecords::reset_to_keys` (**all self time**) |
| 37.1 % | `topology::brep::p3::reseed_records` |
| 26.0 % | `heal::stitch::Stitch::edit_glue_edges` |
| 20.0 % | `primitive_creation::primitives::create_face_id` → `make_face_with_curves` |
| 12.6 % | `topology::brep::p3::make_wire_edge_vertex` |
| 8.4 % | `topology::brep::p3::seed_records_once` |
| 2.2 % | `pcbcraft_solidcraft::project::laminate` |
| 0.9 % | `solidcraft_kernel::body::Body::mesh_untimed` |
| 0.6 % | `solidcore::blend::fillet_executive … do_filleting` |
| 0.4 % | `solidcraft_geom::tess::EvalSurface::derivs` (surface evaluation) |

The self time is per-edit bookkeeping:

- `reset_to_keys` 42.8 %;
- `seed_records_once` 7.9 %;
- the iterator and `brep_entities` walks over every entity, about 10 %;
- `BTreeMap` bulk builds 3.5 %;
- `check_keys_once` 2.5 %.

That is about **76 % of the build** spent re-walking the whole B-rep's records once per stitch or edge edit. `reset_to_keys` walks every key of the record map on every call. It is called through `with_kernel_callbacks` for each vertex or edge glue, so a region of N side faces does N glue edits of O(N) each.

The earlier single profile blamed surface evaluation at about 90 ms per body. This profile does not support that. Surface evaluation (`EvalSurface::derivs` and the basis functions) is about 1 s of the 182 s, and the part bodies are a small share.

## 2. Scaling

**One extrusion against its size** (`extrusion_scaling`: one region, extruded 35 µm, as `batched_body` extrudes a fill):

| Side faces | 16 | 32 | 64 | 125 | 250 | 500 | 1,000 | 2,000 | 4,000 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| One outline of N edges | 4 ms | 7 ms | 18 ms | 56 ms | 0.2 s | 0.69 s | 3.3 s | 17.2 s | 74.7 s |
| A square with N/4 square holes | 5 ms (20) | 8 ms (36) | 21 ms (68) | 65 ms (132) | 0.22 s (260) | 0.76 s | 3.5 s | 17.5 s | |

Each doubling costs 3.2–5.2×, about N^2.2. Holes and outline edges cost the same: what counts is the faces of the one body. The real fills fit the curve: fill_643 has 3,517 faces and took 56–72 s. At small sizes the cost per face is about 0.2 ms, which is the floor a linear stitch would approach. The old kernel managed about 0.01 ms per face (section 4).

**Against vias** (`addvias=N` on the ESP32 board without fills; every via adds one hole to each of the 5 slabs):

| Vias | 55 | 180 | 305 | 555 | 1,055 |
| --- | --- | --- | --- | --- | --- |
| Build | 5.9 s | 7.5 s | 10.4 s | 19.5 s | 51.9 s |
| One dielectric slab | 0.14 s (115 faces) | 0.45 s (238) | 1.0 s (363) | 2.8 s (613) | 9.2 s (1,113) |

The slabs are quadratic in the same way. Extrapolated, a 10,000-via slab would take tens of minutes, against the 3 s bar the old kernel met for the whole board. The vias' own copper is linear and cheap: one definition per size, placed by occurrences.

**Against parts** (`parts=N`, no fills):

| Placements | 0 | 5 | 10 | 15 | 20 |
| --- | --- | --- | --- | --- | --- |
| Build | 1.2 s | 2.3 s | 2.8 s | 2.9 s | 5.5 s |

Parts grow about linearly. Each new package definition costs a few hundred milliseconds, mainly the template's edge-round fillets (0.22–0.29 s per package), and a definition is built once however many instances use it. The step from 15 to 20 is the module and the THT parts. Their drills add holes to every slab (from 87 to 115 faces), and their part files add bodies.

**Answer: linear per body, but quadratic within one body.** There is no huge Boolean (13 small ones in all). The quadratic is inside solidcore's sweep of each large planar profile, and Pcbcraft makes a few very large ones: the fills, and slabs that hold every drill.

## 3. Why one part rotation rebuilds the whole board

The board is one native document (`project::build`), but edits do not start from scratch. `project::update` replays the recorded command sequence against the previous design ([`native/replay.rs`](../crates/solidcraft-integration/src/native/replay.rs)). Changed values (parameters, sketch drawings, occurrence moves) are applied to it. Solidcraft then re-evaluates from the first feature the edit changed, and every body it did not change keeps its mesh. Three things defeat this on the ESP32 board.

**(a) The rotation falls back to a full build because of a signed zero.** The turn's update reported `Anew("… the board changed in more than values at native step 2673: joint.create changed")`. The debug output shows the only difference in SW1's two pad joints:

```
was … cos(instance_104_angle) * -1.7 mm - sin(instance_104_angle) * -0 mm …
now … cos(instance_104_angle) * -1.7 mm - sin(instance_104_angle) * 0 mm …
```

The pad offset is rounded to a nanometre (`pad.offset().map(|v| (v * 1e9).round() / 1e9)` in `project.rs`). After a 90° turn, `(-1e-16 * 1e9).round()` is `-0.0` and prints as `-0`. `joint.create` is not a value the replay may change, so the whole board is built anew. Undo gives the same result. With `+ 0.0` added after the rounding (an experiment, reverted), both the turn and the undo update in place in **0.1 s** (`InPlace(1)`).

**(b) A fill the edit made stale is extruded twice.** In the app, the fill worker then refills the GND pour around SW1 (`compute_stale`: 1 fill, new regions). That fill's sketch is redrawn. Its extrusion picked its profile in a way that a redrawn sketch renumbers, so the replay rebuilds it with `timeline.redefine`. That redefine evaluated the timeline twice: two `doc evaluate` spans of 65 s and 67 s. The update took **135 s** for one 65 s extrusion.

**(c) Caching is per feature, but keyed on everything before the feature.** Solidcraft's `Model::evaluate` keeps results up to the first changed feature. After that, each feature's cache key (`cache_key` in `solidcraft-doc/src/eval.rs`) includes `state.digest()`: the name and content key of every body made so far. A fill extrusion reads nothing but its own sketch. Yet a track widened by 10 µm changes the track's body, so every later feature misses the cache. Those features are all 4 fills and the laminate: **205 s**, `InPlace(6)`. With `fill::build` moved before the parts in `build_on` (an experiment, reverted), the same edit took **3.1 s**. No revision stamp is involved. The key is simply the whole preceding state.

**What Solidcraft offers for incremental work today:**

- Reuse up to the first changed feature (the chain fingerprint).
- A per-feature result cache keyed on (feature, its parameters, the document context, the digest of the state before it, validation mode).
- `placement_reuse`: placements that only move reuse their geometry. That is why vias and part occurrences move cheaply.
- `Model::prime`: results evaluated ahead on another thread.
- Deferred evaluation (`defer_evaluation`) for batches.
- Meshes kept by `geometry_key`.

There is no dependency-aware invalidation: a feature that does not read an earlier body still misses its cache when that body changes. Pcbcraft can work around that only through the order of the timeline.

## 4. What the old kernel did differently

The old kernel was truck, at Solidcraft d53b131 before the SMLib repin (`4a88fc2`). It built an extrusion with truck's `builder::tsweep`. That function makes the swept solid's topology directly: each profile edge becomes one side face, and the vertices and edges are shared by construction. There is no stitching or healing pass and no per-edit notification records, so the work is linear in the profile's edges. A large swept profile was also oriented and meshed through a prism fast path (`Body::new_swept`, `prism_mesh`), not by a general face-by-face tessellation.

The SMLib path, `solidcore::sweep::create_linear_sweep`, builds the side faces one by one (`create_face_id`, each new wire seeding records) and then stitches them into a shell. That stitch is where 73 % of the build goes. So what we lost is not exactness: both kernels made the same planar faces. We lost **construction by topology**. The new kernel rebuilds the sweep the way a general modeller heals imported faces, and its bookkeeping is quadratic.

**Measured on the old kernel.** The same benchmark was run on 447d03e, the last commit before the repin: a source export built in the scratchpad, not a checkout. It used that commit's ESP32 example, which has the same 20 placements, 201 tracks, 55 vias and 4 fills and builds the same 884 features and 1,157 bodies. Only the kernel differs.

| | Old kernel (truck) | SMLib | Ratio |
| --- | --- | --- | --- |
| ESP32 build | 3.4 s (evaluation 1.4 s) | 183–214 s | ~55× |
| Extrude features in all | 0.65 s | 177 s | ~270× |
| Largest fill extrusion | < 28 ms (not in the 12 slowest) | 56–72 s | > 2,000× |
| Fillet features | 0.50 s | 1.4 s | 2.8× |
| Track 10 µm wider (update) | 0.8 s | 205 s | |
| Turn SW1 (update) | 3.3 s, also `Anew` from the same `-0` | 247 s | |
| Turn SW1 then refill (update) | 0.9 s | (135 s with the `-0` fixed) | |
| 1,000 extra vias, no fills | 2.5 s | 51.9 s | 21× |
| One region of 250 / 1,000 / 4,000 / 16,000 edges | 7 / 24 / 35 / 218 ms | 0.2 / 3.3 / 75 s / — | linear against N^2.2 |

The `-0` bug and the fill order predate the repin. The old kernel's 3 s rebuild hid them.

## 5. Options

Gains are measured where marked; the rest are bounded by this profile. Effort is in agent hours.

| | Option | Expected gain | Effort | Risk | Fits AGENTS.md? |
| --- | --- | --- | --- | --- | --- |
| a1 | **Fix the `-0`** in pad joint offsets (normalise after rounding) | Rotate or undo a part with no stale fill: 247 s → **0.1 s** (measured) | 1 h, with a regression test that a 90° turn updates in place | Low | Yes |
| a2 | **Fills first** in the native timeline (before parts, pads and tracks; the laminate stays last) | Part or track edits stop re-extruding fills: 205 s → **3.1 s** (measured) | 2–4 h: the order and the identity, replay and pick tests | Low–medium: pick order and saved identity change once; no released files exist | Yes |
| a3 | **Evaluate a redrawn fill once** (redefine before evaluating, or under deferred evaluation) | A refilled fill: 135 s → about 65 s (bounded: one of the two measured evaluations) | 2–4 h | Low | Yes |
| a | a1 + a2 + a3 together | Edits that leave the fills alone: 0.1–3 s. A refill still costs one fill's extrusion (28–65 s) until c1 or d1 | 5–9 h | Low | Yes |
| b | **Instant 3D from Pcbcraft's 2D data**, the kernel solid built in the background | First picture in about the time of the display layers, which are already built from 2D (~0.1 s) plus the part meshes. The kernel build still runs; its CPU time is unchanged | 12–20 h: stand-in copper, laminate and fills from 2D polygons, picking by source, swapped per body as the kernel finishes | Medium: two pictures of one board for a while; the stand-in must look and pick the same | **Only as a transient stand-in**, as the translucent box is today. The live model must become the kernel model, which is the saved `.solidcraft`. A permanent 2D-drawn view would be a second geometry path |
| c1 | **solidcore: make stitch bookkeeping incremental** (reseed only the entities an edit touched, not `reset_to_keys` over all) | Bounded by the profile: ≥ 76 % of the build is that bookkeeping. At the measured 0.2 ms per face, the ESP32 fills would take about 2.5 s (from 173 s) and a 1,000-hole slab about 0.2 s (from 9.2 s) | 8–16 h in solidcore, with its own tests | Medium: core topology code in a dependency; needs upstream review | Yes (no second engine). **Upstream contribution: needs the founder's go** |
| c2 | **Solidcraft: per-feature cache keyed on what the feature reads** (a new-body extrude reads its sketch and parameters, not earlier bodies) | Makes a2 unnecessary and helps every Solidcraft user; same gain as a2 here | 6–12 h | Medium–high: cache correctness across all feature kinds | Yes. **Upstream: needs the go** |
| c3 | **Solidcraft kernel: a prism fast path** that builds a planar extrusion's B-rep topology directly, as truck's `tsweep` did, instead of sweep and stitch | Like c1 for extrusions only; the old kernel built this board in 3.4 s, with a 4,000-edge region in 35 ms | 12–24 h | Medium: a second construction path for one feature kind inside Solidcraft's kernel crate | Yes (still one engine). **Upstream: needs the go** |
| d1 | **Tile big planar bodies**: cut each fill region and each laminate slab into pieces of at most about 64 side faces, each its own extrusion | Bounded by the synthetic curve (64 faces = 18 ms): ESP32 fills 173 s → about 3.5 s; a 1,000-via slab 9.2 s → about 0.3 s. Combined with a2, a refill re-extrudes only the tiles after the changed one | 6–10 h: tiling in 2D, one body per tile, identity, mask and mass checks | Medium: more bodies in the saved file and in STEP (each fill and slab in pieces). Tiles touch along internal faces, which is copper joining copper | Yes. Copper stays real, thickness-aware geometry |
| d2 | **Fewer edges**: fit arcs to fill boundaries (clearances around round pads and vias), or coarsen `MODEL_CHORD_MM` from 10 µm | Gain is about (edge ratio)^2.2: halving the edges gives about 4.5×. Not measured on this board | Arcs 6–10 h; chord 1 h | Low for the chord (a look trade-off); medium for arcs | Yes |
| d3 | **Holes as one pattern feature** (cut a via pattern from the slab by a Boolean) | Not measured. Slabs are built without Booleans today because Booleans were slower (`c8a4793`). It would not reduce the faces the stitch handles | 4–8 h to measure | Medium | Yes |

**Must every via be cut into the live B-rep?** In the current design no via is cut. Each laminate slab is extruded with every drill as a hole in its sketch, and via copper is one definition per size placed as occurrences. The drills must be in the **saved and exported** model for the file to be physical: correct mass, no interference with the barrels, and correct STEP. The **live view** does not need them before the kernel finishes: the mask, finish and via domes are drawn from 2D data already. AGENTS.md says the live 3D model and the saved `.solidcraft` share definitions. So the kernel model with its drills must become the live model in the end. Holes drawn only from 2D are acceptable only as a stand-in while the kernel builds (option b). Selection and camera changes rebuild nothing in any option here.

## Recommendation

1. **Now, Pcbcraft only, no upstream:** a1 + a2 + a3 (5–9 h), then d1 (6–10 h). Expected on the ESP32 board:
   - open in 3D in about 9 s, from 183 s (5.5 s for the board without fills, plus about 3.5 s of tiled fills);
   - part and track edits in 0.1–3 s;
   - a refill in a few seconds.

   The 1,000-via board would build in a few seconds again. Every gain except d1's is measured above.
2. **Propose c1 to solidcore** through the founder. It removes the root cause for every Solidcraft user: any planar body with thousands of faces, imported or built. It would also let d1 be undone later if the tiles are unwanted in STEP. If upstream declines, c3 is the fallback inside Solidcraft.
3. **Keep b for the "opens instantly" target.** Even 9 s is not instant. A 2D-drawn stand-in, replaced as the kernel model arrives, is the only way to show the first frame in well under a second without breaking the shared-definitions rule. Do it after 1, so that the stand-in is short-lived.

## Decisions for the founder

1. Approve the Pcbcraft-side fixes a1, a2 and a3. They are small, low-risk and measured.
2. Accept **tiled fills and laminate slabs** (d1) in the saved `.solidcraft` and in STEP: several bodies per fill and per slab, instead of one. If not, the build speed waits on c1 or c3.
3. **Go or no-go for an upstream solidcore contribution (c1).** If go, decide the channel (issue first, or a PR from the fork) and who reviews. Until then, nothing is posted.
4. Whether "3D opens instantly" justifies the 2D stand-in (b), with its brief period of two pictures of one board.
5. Whether the live view may show drills from 2D data while the kernel model is building. The saved and exported model always has them cut.

## Reproducing

- Build and edits: `PCBCRAFT_PROFILE_EDIT=1` (part turn and undo), `=track` (track width), and `PCBCRAFT_PROFILE_REFILL=1` (refill after the turn), with the command above.
- Scaling: `PCBCRAFT_PROFILE_VARIANT=parts=N` with `PCBCRAFT_PROFILE_NOFILLS=1`, or `PCBCRAFT_PROFILE_VARIANT=addvias=N`; and `extrusion_scaling` with `PCBCRAFT_SCALING_EDGES` and `PCBCRAFT_SCALING_SHAPE=outline|holes`.
- Function profile: run the test binary directly and attach `/usr/bin/sample <pid> 1500 1 -mayDie -file out.txt`. The test binary's symbols are enough; neither samply nor `cargo flamegraph` is installed; `xctrace` is available but was not needed.
- The `-0` and fills-first results are experiments: one-line changes, measured and reverted. Neither is in this commit. The only code in it is the benchmark and the off-by-default `profile` feature of `pcbcraft-solidcraft` (a re-export of Solidcraft's profiler), which the benchmark turns on as a dev-dependency feature.

## Update, 9 November 2026: board width, undo

Build bdc78b4 (`component-model`), Solidcraft fork 04d3c2ffc (solidcore c0d3cc0124 with the stitch fix, after the `-0` fix and fills-first), same host and `ci` profile, load average 7–30 from other agents' builds: single runs vary by up to ±20 %. Best of three, headless. `PCBCRAFT_PROFILE_EDIT=width` is the board's `width` parameter 1 mm more through the engine (`board.edit`) and its undo, each followed as the app follows it: the model updated with the fills' stored copper, the stale fills refilled, the model updated again.

**Where the ESP32 build goes now (26.5–32 s).** Solidcraft spans: `solidcore smlib_extrude` 72 % (the four fill regions, 2,547–3,517 side faces each, are 17–21 s of it), `kernel mesh` 14 %, fillets 5 %, commands and evaluation bookkeeping 9 %. The build runs on one core; Solidcraft evaluates features one after another, and the 18 kernel worker threads idle. A `sample` of the build thread (25 s, 16,716 samples) inside the SMLib extrusions:

| Share of the build | Where in solidcore |
| --- | --- |
| 76 % | `create_linear_sweep` (all extrusions) |
| 46 % | `stitch_and_orient_with_identity` (sewing the side faces into a shell) |
| · 32 % | `Stitch::edit_glue_vertices` → `glue_with_notification_identity` → `with_kernel_callbacks` (self time; per-glue bookkeeping over the whole B-rep) |
| · 13 % | `Stitch::edit_glue_edges` → `test_closure_split_region_attempt` → `has_lamina_edge` → `face::edges` |
| 26 % | `create_planar_faces_with_3d_curves` (the two caps of each extrusion, one loop per hole) |
| · 22 % | `create_faces_with_curves`: each loop added (`make_vertex_loop`, `remove_loop`) calls `face_notify`, which walks every edge-use and vertex-use of the face: quadratic in a cap's loops |
| · 1.6 % | `point_3d_classify` |

**What an edit and its undo cost, and why.** The width edit moves every fill (they follow the board), so every fill is refilled and its sketch redrawn. A redrawn region's extrusion picked its profile by curves the redraw replaces, so the update redefines it (`timeline.redefine`). Two Pcbcraft-side costs were measured and are fixed:

1. Each redefinition ran with the history rolled forward, so it evaluated every feature after it again: the refill update ran 2,878 SMLib extrusions where the build runs 1,028. The redefinitions now run while the history is still rolled back, and the update keeps every result it evaluates (`Model::keep`) until the history is rolled forward once: 948 extrusions, and 3 s of meshing instead of 8 s.
2. An undo back to a board the app's model worker made updated the last model back to it, as long as the edit. The worker now keeps its last three earlier board models (copies sharing the evaluated design and meshes, `NativeModel::snapshot`), and an undo or redo back to one shows it again without building or updating. (Since 11 November it keeps one, and never the board an edit had before the fill worker refilled it: no history step returns to that board, and each kept model holds about 0.9 GB of evaluated bodies on this board; see `apps/pcbcraft/src/scene/memory_tests.rs`.)

| ESP32 board | Before | After |
| --- | --- | --- |
| Open in 3D (build) | 26.6 s | 26.5 s (unchanged; nothing here touches the build) |
| Width +1 mm: update with stale copper, then refilled | 3.6 + 47.6 = 51.3 s | 4.0 + 40.6 = 44.7 s |
| Undo, through `project::update` | 46.4 s | 38.8 s |
| Undo in the app (model worker) | the update above | the earlier model shown again; no build (keeping each copy costs 4 ms) |

A redefinition still evaluates its extrusion twice: once as the trial constructor on a scratch copy, once as the redefined feature (another feature id, so another cache key). For the fills that is about 17 s of the 40 s refill update. The showcase (build 4.4 / 4.7 s; outer track 559 / 616 ms; part 24 / 26 ms; inner track 516 / 601 ms) and the 1,000-via board (build 28.9 / 32.2 s; outer track 27.0 / 29.5 s; part 422 / 451 ms) did not change beyond the load (7 before, 15 after).

**What remains, by share, and the smallest generic fix:**

- solidcore, 46 % of the build: stitching a swept shell does whole-B-rep bookkeeping per glue. Fix: sew a linear sweep's side faces by construction (each profile edge's side face shares its edges with its neighbours), or make the per-glue notification incremental.
- solidcore, 22 %: adding a loop to a face notifies the whole face. Fix: notify only the entities the new loop adds. (The `large-face-loops` branch's `point_3d_classify` fix is 1.6 % on this board.)
- Solidcraft, about 17 s of an edit that refills the fills: `timeline.redefine` evaluates the feature twice. Fix: keep the trial's evaluation for the redefined feature (the same definition and the same state before it), or a way to re-pick an extrusion's default profiles after its sketch is redrawn without a trial.
- Solidcraft: evaluation is single-threaded; independent new-body extrusions (the four fills) could be evaluated in parallel.

**Measured option, not applied:** the fills' model chord (`MODEL_CHORD_MM`) at 25 µm instead of 10 µm builds the ESP32 board in 16.0 s (fills 6.5 s instead of about 17 s; 1,505–2,300 faces each). It changes the saved and displayed copper outline by up to 25 µm, not the document's copper or fabrication output; it needs the founder's call.

## Update, 10 November 2026: an edit in the app against headless

Build b8066c8 (`component-model`), same host and `ci` profile, load average 4–6. The app's J1 edit (its dimension 0 → 1 mm, ESP32 board, 3D open) took about 28 s in the VM against 3.7 s headless. The app-side benchmark `apps/pcbcraft/src/scene/edit_timing_tests.rs` runs it through the app's own model worker and fill worker (`PCBCRAFT_TIMINGS=1` prints each board model the worker makes, how, and why an update fell back to a build):

| Step | Time | How |
| --- | --- | --- |
| The edit's model | 3.4–3.6 s | updated in place, 61 commands, as headless |
| The fill worker refills the 4 fills J1's move made stale; the board changes again | — | the GND pour goes from 29 to 31 regions |
| The refilled board's model | 21.4–23.1 s | built anew: "sketch.create where component.activate ran" (each region is a sketch and an extrusion of its own, so another number of regions is another design) |

The worker made the edit's model in place, but the app dropped it: by the time it was ready, the refilled board had been requested, and only the newest request's model was shown. J1 then moved on screen only with the rebuild, after 25–27 s.

**Fixed:** a finished model of an earlier request of the same project is shown (under the "Updating" note, picks ignored) when it is newer than the model shown. J1 now moves in **3.7 s**, as headless, and the refilled fills follow at 26.9 s.

**What remains: the refill, 86 % of the edit's 27 s.** Updating the refilled fills in place would not be cheaper today. Each of the 4 fills changed around J1's pads, so each would be redrawn and redefined, and a redefinition evaluates its extrusion twice (the width edit's refill update above: 40.6 s against a 26.5 s build). The smallest correct fix is upstream: keep the trial's evaluation for a redefined extrusion (Solidcraft), or make solidcore's sweep stitching linear. Both need the founder's go. Tiling the fills (option d1) would make both the build and the refill a few seconds.

## Update, 10 November 2026: a board width change in place

Same host and `ci` profile, load average 12–15. `PCBCRAFT_PROFILE_EDIT=width` (ESP32 board, 40 → 41 mm through the engine).

**Why the width edit was built anew.** Not the outline: the laminate's outline is a sketch the update redraws, and the mask, silkscreen, edge and fill films are drawn from 2D data. The engine's carry (`dimensions::carry`) adds a track that joins a track stretched to the new edge (`trace_648`), and the update compared the build's command stream step by step: `parameters.import changed: added trace_648_x0 …`, so any added or removed track built the board anew. Undo removes the track and fell back the same way.

**Fixed:** the commands that build one track are a *unit* of the replay (`Native::unit`). A unit the recorded build did not have runs on the design as it comes; its features go at the end of the history, where evaluating them reads nothing evaluated before. A recorded unit the build passes over is deleted (`component.delete`, which takes its features, occurrence and joints). The aggregate commands follow: parameters added (before the units that use them) or dropped (`parameters.delete`, once their users are gone), the new joints made, looks assigned again, identity metadata written again, and a removed body's look dropped. The design's evaluation is put off while the units run and done once. Tests compare the result with a fresh build as sets of bodies (kind, source, layer, box and volume) and sources and saved identity without native ids, since an added object's component takes the design's next id.

| ESP32 board | Before | After |
| --- | --- | --- |
| Width +1 mm (carry adds a track), stale fills | 27.7 s, built anew | **4.4 s**, in place (73 commands) |
| …then the 4 fills refilled | 50.0 s in place | 42.6 s in place (unchanged: see the refill) |

## Update, 10 November 2026: tracks and vias added or removed by routing

`PCBCRAFT_PROFILE_EDIT=route`: a routed step on the ESP32 board (an outer track and a via at its end) added, then removed. Before, any added or removed track or via built the board anew (the full build, 27–36 s under this load). Tracks are units since the width fix. Vias are placements of one definition per size, copied in one command per size; a via added or removed changes that command's number of copies. Now every recorded copy that stands where a copy is asked for is kept, the others are deleted (`selection.delete`), and the new ones are copied on the design; a via removed where the definition's first occurrence stood moves that occurrence to the next via, as a fresh build places it. A via of a new size still needs a new definition and is built anew.

| ESP32 board | Before | After |
| --- | --- | --- |
| Routed step added (track and via), stale fills | built anew, 27–36 s | **2.1 s**, in place |
| …removed again | built anew | **2.2 s**, in place |

## Update, 10 November 2026: refills in tiles

`PCBCRAFT_PROFILE_EDIT=move`: J1 moved 0.5 mm, then the four fills it made stale refilled, as the app's fill worker does; same host, load average 5–25.

**Why a refill was built anew.** Each fill region was one sketch and one extrusion, numbered by its place in the fill's copper; a refill that split or joined a region (J1's move takes the GND pour from 29 to 31 regions) changed the command stream, and even with the same regions every changed region was redrawn and redefined (`timeline.redefine`, two evaluations of a 2,500–3,500-face extrusion).

**What was chosen, and why.** The fills are built in **tiles**: each fill's regions are cut along a fixed 12 mm grid of the board (an exact cut on a nanometre grid, `i_overlay`'s integer overlay, so a cell's pieces depend only on the copper in that cell), and each loop is simplified to the model's 10 µm chord with every point on the cell's border kept, so neighbouring tiles meet along the same cut edges (copper joining copper). A tile is a replay unit known by its fill, cell and place in the cell, and by a digest of its outline: a refill deletes the tiles whose copper changed and builds them again at the end of the history; the others are passed over. No profile pick is involved, so nothing of Solidcraft's own profile-pick rule is reproduced, and nothing in the Solidcraft fork changed. Deletions roll the history back to the first feature they take, so the design is evaluated once, when it is rolled forward. The other way to stop a region-count change forcing a rebuild, region units without tiles, would still re-extrude a whole 3,500-face region for a change around one pad; tiles make the build cheaper too, because extruding one profile costs more than linearly in its edges.

| ESP32 board | Before | Tiles of 4 mm | 8 mm | **12 mm (chosen)** |
| --- | --- | --- | --- | --- |
| Open in 3D (build) | 34.4 s (27 s at lower load) | 25.5 s | 17.4 s | **15.0–16.1 s** |
| J1 moved 0.5 mm, stale fills | 7.2 s | 3.5 s | 3.3 s | **3.3 s** |
| …the 4 stale fills refilled | 35.9 s, built anew | 13.4 s | 8.4 s* | **8.9 s** (7.3 s*) |
| Outer track 10 µm wider | — | — | — | **2.3 s** (4.9 s*) |
| Width +1 mm, then all 4 fills refilled | 4.4 + 42.6 s | — | — | **4.9 + 11.0 s** |

\* the fills placed just before the laminate instead of first; kept first, since the edit's own update (what moves first on screen) is the faster one there.

More tiles cost more than their smaller extrusions save: Solidcraft's evaluation bookkeeping grows with every body made before a feature (`doc evaluate` self time 8.0 s at 4 mm, 2.3 s at 12 mm), while the extrusions stay about 4.3 s at every size.

**What remains.** A refill still evaluates every feature after the first tile it deletes: the later tiles, parts, pads, tracks and the laminate (1,128 SMLib extrusions in the J1 refill, the inner foil-level slabs 0.6 s each), because Solidcraft keys each feature's cached result on the digest of every body made before it. Only a generic Solidcraft change removes that: a feature's cache key built from what it reads (a new-body extrusion reads its sketch and parameters), option c2 above. It needs the founder's go and was not made.

**In the VM** (8 cores, `ci` profile, `PcbcraftSpeedCheck.app` at d08cbb3 with `PCBCRAFT_TIMINGS`; the model worker's own lines), ESP32 example, 3D view open:

| | Before (VM, 10 November, earlier runs) | After |
| --- | --- | --- |
| Open in 3D (build) | about 26 s | 17.7 s |
| Width 40 → 41 mm from the board card (carry adds a track) | full rebuild, about 26 s | 5.3 s, in place (81 commands) |
| …its 4 fills refilled | rebuilt, 23–27 s | 11.4 s, in place (283 commands) |
| R2 moved 0.5 mm from the placement card | — | 0.16 s, in place (1 command) |
| …its stale fills refilled | rebuilt, 23–27 s | 6.0 s, in place (8 commands) |

Selecting R2 and switching between Layout and 3D made no board model. The app's own benchmark (`edit_timing_tests`, host, J1 0 → 1 mm): J1 shown at 4.4 s, the refilled model at 14.2 s (was 26.9 s; the refill updated in place in 9.7 s, was built anew in 21–23 s).

## Update, 11 November 2026: what an updated model held

Same host and `ci` profile, load average 30–130 (other agents' builds), so times here are not comparable; memory is live heap from `heap -s`.

**The ~380 MB an updated model held beyond a fresh build** was Pcbcraft's. Each body's lazy volume (`Volume::lazy` in `project.rs`) captured the evaluated body, and an update takes every unchanged body's geometry from the last model, volume and all. So the updated model kept the last model's bodies wherever the update re-evaluated one: laminate slabs of the same inputs, for example. The geometry the 3D view shows kept them too. A lazy volume now reaches its body through the model that made it (a weak handle to the model's world state), and an update measures what it takes from the last model on its own bodies. Geometry handed on holds no evaluated body: an unmeasured volume reads NaN once its model is gone. Only tests read volumes.

| ESP32 board | Before | After |
| --- | --- | --- |
| One width edit and its refill, the updated model (`model_memory`) | +1,278 MB (fresh build of the same board: +912 MB) | +904 MB (fresh build: +846–912 MB) |
| App, 3D open, 4 board edits (`memory_tests`, one earlier model kept) | live 1.91 → 2.22 → 2.31 → 2.28 GB; RSS 3.32 GB | live 1.77 → 1.80 → 1.79 → 1.80 GB; RSS 2.92 GB |
| …then 4 undos | live up to 2.28 GB; RSS 3.70 GB | live up to 1.82 GB; RSS 3.40 GB |

The rest is one board model (~0.9 GB of solidcore B-rep) for the current board and one for the undo. The undo depth stays the founder's call (R8-1 in the night findings): each further instant undo step still costs about 0.9 GB on this board.

**Where an edit's update goes now** (`PCBCRAFT_PROFILE_EDIT=width`, load ~50): build 26.5 s; width +1 mm with stale fills 8.8 s, 736 SMLib extrusions; the refill 18.5 s, 1,188 extrusions against 1,322 in the build, of which the two redefined foil-level slabs' evaluations are 9.3 s; the undo 20.4 s, 1,193 extrusions. Almost every feature after the first changed tile is evaluated again, because Solidcraft keys each feature's cached result on every body made before it. That is Solidcraft's to change (option c2; upstream's read-based key #6295 is measured on the exported board separately).
