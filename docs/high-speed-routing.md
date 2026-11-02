# High-speed routing: controlled impedance, differential pairs and length tuning (as built)

The founder asked for controlled impedance from the stackup, differential pairs and length tuning. The acceptance case is the first real board: an ESP32-S31 module board with a USB 2.0 pair at 90 Ω differential. This note records what was built, the models and their accuracy, and the seam the push-and-shove router will reuse.

## Invariants

- **One typed engine.** New commands: `diff_pair.add`/`edit`/`delete`, `length_group.add`/`edit`/`delete` (in `Operation::Rules`), `layout.route_pair` and `layout.tune_length`. New queries: `impedance.calculate`, `impedance.table`, `diff_pair.suggest`, `layout.length_report` and `layout.tune_preview`. All come from `pcbcraft_core::schemas()`, so the CLI (`stdio`) and MCP reach them through the generic `project.apply` and `project.query` (`crates/automation/src/session/high_speed_tests.rs`). Each command is one undo step; a refused command leaves the document, revision and history unchanged (`a_refused_tuning_changes_nothing`, `pairs_survive_the_file_round_trip_and_follow_undo`).
- **Pure core geometry.** Impedance (`layout/impedance.rs`), pair geometry (`layout/pair.rs`), meanders (`layout/meander.rs`), lengths and checks (`layout/length.rs`) and the obstacle index (`layout/obstacles.rs`) are pure functions of the project. The desktop's previews and its commits call the same functions.
- **Copper stays real geometry.** Pair tracks, fan-outs and meanders are ordinary `Track`s and via pairs are ordinary `Via`s, so 2D, the checks, 3D (thickness-aware copper bodies) and the native model treat them as any other copper. Previews draw in the canvas only; selection and hover never rebuild geometry.
- **Pre-release format.** New fields are serde-defaulted and skipped when empty: `NetClass.signal`, `ProjectRules.{diff_pairs, length_groups, next_pair, next_group}`, `Dielectric.dk` and `CopperFoil.plane`. No migrations.
- **Nothing inferred.** A pair exists only through `diff_pair.add`; `diff_pair.suggest` proposes pairs from net names and the High speed card offers them, but never applies one. A layer is a reference plane only when its foil is marked; otherwise the model states that it took the neighbouring layers.

## Data

- **Net class signal rules** (`rules/signal.rs`, `SignalRules` on `NetClass.signal`): single-ended `impedance` (10–200 Ω), differential `diff_impedance` (20–300 Ω), `impedance_tolerance` (0.5–50 %, 10 % when unset), `pair_max_uncoupled` and `pair_max_skew` (mm). The class's `diff_pair_width` and `diff_pair_gap`, reserved before, now set the pair width and gap; with a differential target, whichever is unset is derived.
- **Differential pairs** (`DiffPair`): a name, a positive and a negative net. A net is in at most one pair, and the two nets differ. The positive net's class supplies the pair's rules. Pairs follow nets through merges (`rules::signal::follow_nets`, called where net-class assignments follow): a merged-away net is replaced by its survivor, and a pair whose halves became one net, or lost one, is removed.
- **Length groups** (`LengthGroup`): nets and pairs (a pair contributes both halves), an optional `target` and a `tolerance`. With a target, every routed member must lie within `target ± tolerance`; without one, within `tolerance` below the longest member. A group of one pair matches the pair's halves.
- **Stackup** (`model/stackup.rs`): `Dielectric.dk` is the fabricator's Dk; when absent, `Dielectric::permittivity` uses a generic estimate by glass style (106: 3.8, 1080: 4.0, 2116: 4.3, 7628: 4.6, typical published FR-4 values near 1 GHz, not fabricator data). `CopperFoil.plane` marks a ground or power plane. `layout.set_stackup` validates Dk in 1–20.
- **Clearance inside a pair** (`layout/limits.rs`): between the two halves of a pair, only the profile's object-pair clearance applies, not class or class-pair clearances, so a pair can run at a gap smaller than its class clearance to other nets. The profile's track-to-track clearance is the smallest pair gap the rules allow (`impedance::pair_gap_min`).

## Impedance model

`layout::impedance::structure` finds each layer's references: the nearest foils marked as planes above and below it, or, when no foil is marked, its neighbouring copper layers (`LayerLine.assumed_references`). An outer layer over a plane is surface **microstrip**; an inner layer between two planes is **stripline** (offset when the heights differ); an inner layer with one plane is **embedded microstrip** under the dielectric out to the board surface. Heights are the dielectric between the trace's faces and the planes from the stackup's z-model (`Stackup::copper_span`), copper thickness is the layer's foil, and the dielectric constant is the thickness-weighted average of the dielectrics between the trace and its plane(s). Copper layers lying between a trace and its plane are ignored.

The formulas are our own implementation of published equations (`layout::impedance::formulas`):

| Structure | Equations | Accuracy we state |
| --- | --- | --- |
| Microstrip, Z0 and εeff | Hammerstad and Jensen, "Accurate Models for Microstrip Computer-Aided Design", IEEE MTT-S 1980, with their strip-thickness correction | About 1 % in the quasi-static limit (matches Pozar's example to 0.04 %, see below) |
| Stripline, Z0 | Wheeler, "Transmission-Line Properties of a Strip Line Between Parallel Planes", IEEE MTT-26, 1978, with his thickness correction | About 1 %: within 0.5 % of Cohn's exact zero-thickness solution for 0.1 ≤ w/b ≤ 3 (worst 0.48 %; the test holds it to 1 %) |
| Offset stripline | Parallel combination of the two centred striplines of plane spacing 2·h1 + t and 2·h2 + t, as given in IPC-2141-style references and Wadell's *Transmission Line Design Handbook* (1991) | A few per cent; not checked against a field solver here |
| Embedded microstrip | Hammerstad–Jensen air impedance divided by √εr′, εr′ = εr·(1 − e^(−1.55·H/h)) (IPC-2141-style), kept between the surface microstrip's εeff and εr | About 5 % |
| Edge-coupled microstrip and embedded microstrip, Zdiff | IPC-2141: Zdiff = 2·Z0·(1 − 0.48·e^(−0.96·s/h)) | About 5–10 % |
| Edge-coupled stripline, Zdiff | IPC-2141: Zdiff = 2·Z0·(1 − 0.347·e^(−2.9·s/b)) | About 5–10 % |

**Not modelled:** frequency dispersion and losses (results are quasi-static), solder mask over outer traces (it typically lowers a microstrip's impedance by a few ohms), etch trapezoid, copper roughness, glass-weave effects and the Dk's frequency dependence. Differential results inherit the coupling approximations' error. Use the values to choose a starting geometry and confirm controlled-impedance layers with the fabricator's field solver.

**Reference values** (`layout/impedance/tests.rs`):

| Case | Reference | Pcbcraft |
| --- | --- | --- |
| Microstrip, d = 1.27 mm, εr = 2.20, W/d = 3.081, t = 0 | 50 Ω, εe = 1.87 (Pozar, *Microwave Engineering*, Example 3.7) | 50.02 Ω, εe = 1.881 (test tolerance 1 %) |
| Microstrip in air, u = 1, t = 0 | 126.4 Ω (hand evaluation: 59.96 Ω · ln(6 + √5)), εe = 1 exactly | 126.4 Ω, εe = 1 |
| Stripline, b = 3.2 mm, εr = 2.20, W = 2.66 mm, t = 0 | 50 Ω (Pozar, Example 3.5) | Cohn's exact form 49.95 Ω (test tolerance 0.5 %); Wheeler 49.84 Ω |
| Stripline, 0.1 ≤ w/b ≤ 3, t = 0, εr = 4 | Cohn's exact K(k)/K(k′) solution (evaluated by the arithmetic-geometric mean) | Wheeler within 0.48 % |
| Coupled microstrip, Z0 = 50 Ω, s = h = 0.2 mm | 81.62 Ω (hand evaluation of the IPC-2141 equation) | 81.62 Ω |
| Coupled stripline, Z0 = 50 Ω, s = 0.2 mm, b = 0.4 mm | 91.86 Ω (hand evaluation) | 91.86 Ω |

**Derived widths** (`impedance::derive`, `impedance.table`). For a class with a single-ended target, the track width that gives it on each layer (bisection on the formula). For a differential target: with the class's pair width and gap both set, the impedance they give; with only the gap set, the width; with only the width set (or a single-ended width), the gap; with neither, the tightest gap the rules allow sets the width. A derived width below the class's minimum width, above the 5 mm routing limit, a gap below the track clearance, or a target no width reaches, is a problem line naming the layer and limit ("Top needs 12.1 mm for 20.0 Ω, wider than routing allows"); the nearest value within the rules is used. For the acceptance case, the 4-layer preset's top layer (35 µm over 0.2 mm 2116 prepreg, Dk 4.3 estimated, the next layer as reference) gives a 90 Ω pair at a 0.2 mm gap (the default profile's track clearance) and 0.30 mm width (`a_90_ohm_usb_pair_derives_its_width_and_gap`); a 50 Ω single-ended track there is 0.36 mm, and 0.27 mm on Inner 1 as offset stripline. These rest on the estimated Dk; enter the fabricator's stackup values before ordering.

## Differential pair routing

**Geometry** (`layout/pair.rs`). Both halves follow one centre line offset by half the pitch (width plus gap) to either side; corners are mitred, so the halves stay parallel at the gap in 45°, 90° and free postures (`posture::path`, shared with single routing). The positive half keeps its side of the centre line through the route. Where the halves leave pads or vias they fan out on 45° legs to the pitch (`fan_advance`), and a click that lands on a pad of the pair fans in to both pads. Where the pads are closer than the pair width plus the pair gap, the fan legs neck down to fit (`fan_width`, no narrower than the class minimum). A layer change places two vias side by side across the route, the positive one on its half's side, at a pitch wide enough for both via pads at the via clearance and for each half to pass the other half's via at the track-to-via clearance (`via_pitch`, rounded up to 0.01 mm with 5 µm to spare); the pair fans into and out of them. Width and gap follow the class on each layer, derived from its impedance target.

**Continuation.** A step that continues a coupled run extends the previous step's last two tracks to the new mitre (`RouteEditSet.replace`), so a corner between two clicks is exact; Backspace restores them.

**Route tool.** A click on a pad of a pair (either half) starts both halves from that pad and the partner's nearest pad (on the same part first). Each click is one step and one `layout.route_apply`; the preview draws the same `pair::step`. V, Shift+V and 1–9 change layer through a via pair; / and F change the posture and elbow; Backspace unwinds a step; Enter or Esc ends; a click on a pad of the pair finishes. P switches pair routing off (and on) to route one net of a pair. The Route card shows the pair's width and gap, the layer's structure and impedance, the class target and any problem line.

**`layout.route_pair`** routes a whole pair for CLI and MCP callers: start pads, a centre line as legs per layer (a via pair where consecutive legs change layer), optional end pads, optional width and gap overrides. It is one undo step and passes the same clearance checks as `layout.route_apply`.

## Length

**Length model** (`layout/length.rs`). A net's routed length is the sum of its tracks' centre-line lengths plus, for each via, the barrel between the outermost copper layers on which the net's tracks reach the via: the distance between those foils' mid-planes in the stackup. On the 4-layer preset, a top-to-bottom via adds 1.635 mm (1.6 mm of dielectric plus half of each 35 µm foil). Not counted: a through via's stub beyond the layers it joins, paths inside pads, and via-in-pad joins. For a two-pin net this is the pad-to-pad length; for a branched net, the total copper length.

**Targets.** A net's target is the most demanding of its length groups' targets (explicit, or the longest member) and, when its pair has a skew limit, its partner's length.

**Tuning** (`layout/meander.rs`). `plan` meanders one straight track segment to add an exact length: trombone (bumps to one side) or accordion (a serpentine to both sides), with 45° chamfered corners. The straight lead-in and lead-out keep the meander one leg spacing off the segment's joints. The fewest bumps whose height stays within the amplitude limit are found by bisection, then the height by bisection on the built polyline, so the added length is exact to under a micrometre (`the_meander_adds_exactly_the_solved_length`; through the engine within 10 µm, `a_meander_adds_the_exact_length_and_keeps_clearance`). The leg spacing is at least the track width plus the track clearance (default: the width plus the larger of the clearance and two widths). Every new segment is checked against other nets' copper at the rules' clearance through the obstacle index, and against the board; a trombone tries the left side, then the right, then lower amplitudes. The track keeps its identity as the first segment. `layout.tune_length` takes the amount to add or the net's target length; `layout.tune_preview` returns the plan without applying it.

**Tune tool** (T). Hovering a segment previews the meander that brings its net to its target, with a live readout at the pointer ("USB_D+ 41.2 / 42 mm (−0.8)"); a click commits it. The Tune card shows the net's length and target, the meander (added length, bumps, height) or why none fits, and the settings: style, amplitude, side and an own target. / switches the style, A and Shift+A raise and lower the amplitude, S and Shift+S widen and narrow the spacing, F flips the side.

## Checks

`layout::length::checks` runs with the board checks (`layout::checks`), so Issues, the CLI and MCP list the findings, each classified in `issues/rules.rs` (warnings):

| Code | Finding |
| --- | --- |
| `pair.skew` | The halves differ by more than the class's max skew (both routed) |
| `pair.uncoupled` | A half runs uncoupled longer than the class's max uncoupled length |
| `pair.gap` | Parallel stretches of the halves closer than the pair gap (worst per layer) |
| `length.mismatch` | A routed group member outside its window |
| `impedance.mismatch` | A track (single-ended target) or coupled stretch (differential target) outside the tolerance, worst per net and layer; or a target on a layer without a reference plane |

**Length report** (`layout.length_report`, the High speed card's Lengths section): every routed, paired or grouped net with its length (tracks, vias, total), target, tolerance and deviation; every pair's halves, skew and uncoupled length; every group's target, window and worst deviation.

## Desktop

- **Net classes card:** a High speed section per class with the impedance targets, tolerance, pair width, gap and limits, then each layer's derived width, gap and impedance with its problem lines.
- **Design rules panel:** an Impedance section with every layer's structure (height, Dk, notes when references are assumed or Dk estimated) and every class with a target, layer by layer.
- **Route card:** the pair, its width and gap, the layer's structure and computed impedance, the class target and any problem line. With a single-ended target, the route width follows the derived width of the current layer.
- **High speed card** (Board card's High speed row): pairs with skew, suggested pairs with Add, length groups (match longest or an explicit target, tolerance, members, add the highlighted net), and the length report. Rows highlight their net.
- **Tune tool** and its card, above.

The obstacle index for the Tune tool is built once per document revision, a meander is planned only when the hovered segment, the revision or a setting changes, and the length report is computed once per revision. Planning one segment takes a bounded handful of polyline evaluations and grid queries; it is not yet measured on a 10,000-via board (see Limits).

## The push-and-shove seam

The modules are built so the push-and-shove router reuses them instead of duplicating them:

- `RouteEditSet` is the transactional proposal: `add`, `replace`, `remove`, `vias` and `remove_vias` commit together or not at all. Pair continuation and meanders already use `replace`.
- `posture::step` and `pair::step` turn a gesture into a proposal; a shove strategy takes that proposal and adds the displaced copper's `replace` entries before the commit.
- `obstacles::Obstacles` is the obstacle query: every other net's track, via and pad with its layers and exact outline, filed in a grid. `hits` returns the obstacles a proposed segment violates (by index, with their kind and identity), which is what a shove step moves; `conflict` turns the first into a refusal.
- `meander::plan` and `pair::route` are proposals too: a shove strategy can make room for a meander instead of refusing it.

## Limits and follow-ups

- Impedance is quasi-static and ignores solder mask, etch and roughness; offset stripline and the coupled forms are approximations (see the table). A 2D field solver would be the accurate follow-up.
- A stackup editor in the desktop is not part of this work: Dk and plane flags enter through `layout.set_stackup` (CLI and MCP).
- Pair routing does not push other copper, does not route around obstacles and does not keep the pair coupled through a bend tighter than its pitch. A pad-to-pad pair whose pads lie along the route direction fans out across the partner pad and is refused by the clearance check.
- Tuning meanders one straight segment per click; it does not spread a meander over several segments or tune a whole pair at once, and the accordion needs room on both sides (it is refused next to the partner half).
- Length counts via barrels between the layers used, not stubs; pad-internal and package (die) lengths are not included.
- Checks and the length report run on the UI thread with the other board checks; neither is measured at 10,000 vias yet.
