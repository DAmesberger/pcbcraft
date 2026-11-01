# Design rules, net classes, copper fills and board shape (as built)

Four founder requests are built here: configurable manufacturing parameters with profiles for different production specifications, net classes, full copper fills, and board cutouts with a flexible, sketch-based board shape on which components are placed relative to any board edge. The work started from a contract slice (types and minimal bodies) and eight parallel tasks; this note records what was built. `crates/editor-core/src/tests/board_contract.rs` still pins the invariants below.

## Invariants

- **Defaults equal the former values.** `DesignRules::default()` is the built-in profile `pcbcraft-default` ("Pcbcraft default"). It equals the former hard-coded limits: 0.2 mm copper clearance, copper anywhere inside the board (edge clearance 0), 0.25 mm drill-to-copper and hole-to-hole, 0.1 mm annular ring and min drill, 0.05 mm mask expansion, 0.1 mm mask web and silk width, 0.6 mm silk text height, 0.3/0.6 mm tented vias, a 0.3 mm route width and the land-estimator margins (gull-wing toe shift 0.1 mm, THT drill over lead 0.3 mm, THT land over drill 0.6 mm, THT land gap 0.25 mm). Silkscreen-to-board-edge clearance defaults to 0, which means it is not checked. `default_rules_equal_the_former_hardcoded_limits` pins this, and every check gives the same result as before under the default profile.
- **One typed engine.** New operations: `rules.set_profile`, `rules.set_override`, `rules.clear_override`, `net_class.add`/`edit`/`delete`/`assign`/`set_patterns`/`set_clearance`, `fill.add`/`edit`/`delete`/`refill`/`store`, `keepout.add`/`edit`/`delete`, `board.set_rectangle`, `board.start_sketch`, `board.edit`, `board.constrain`, `board.constraint_edit` and `board.constraint_delete`. New queries: `rules.effective`, `rules.profiles`, `rules.preview_profile`, `net_class.list`, `net_class.preview_patterns`, `board.shape` and `fill.status`. All of them come from `pcbcraft_core::schemas()`, so the CLI and MCP reach them through the generic `apply` and `query`. Each command is one undo step, and a failed command leaves the document unchanged. A rules command that changes nothing is not an undo step.
- **Pre-release format.** The new `Project` fields (`rules`, `board_shape`, `edge_constraints`, `fills`, `keepouts`) are serde-defaulted and skipped when empty or default. The showcase names only `fills`, because it carries two pours. There are no migrations.
- **Pure core.** Sketch solving runs behind host traits: `FootprintEvaluator` for footprints and `BoardShapeEvaluator` for the board. Hosts pass them as `Engine::apply_hosted(rev, op, Evaluators { footprint, board })`. The desktop app (`pcb_editor::board_shape::host_evaluator`) and the automation session both pass `pcbcraft_solidcraft::board::SketchBoardEvaluator`. The user profile library lives in `pcbcraft-io`.
- **Clean room.** The built-in profiles (`standard` "Standard 6/6", `fine` "Fine 4/4", `hdi` "HDI 3/3") are generic original estimates, labelled `ProfileBasis::Estimated`. Fabricator numbers enter only as user profiles (`ProfileBasis::FabricatorEntered`, with source, URL, date and notes).

## Design rules and profiles

- **Core** (`rules.rs`, `rules/builtin.rs`): `ProjectRules` holds a snapshot of the chosen profile, per-project overrides, net classes, class assignments and patterns, and class-to-class clearances.
  - Values resolve in this order: profile, then override, then net class, then class pair. A `Region` source is reserved and has an explicit precedence.
  - `RuleSource::describe()` names the source in messages, for example "Fine 4/4" or "net class Power".
  - `rules::apply` is atomic. It names the rule in every bounds refusal.
  - `rules.set_profile` refuses a built-in profile whose values differ from the shipped ones (`rules.builtin_modified`; save changed values as a user profile). It also refuses kept overrides that break the new profile (`rules.override_conflict`).
  - `rules.preview_profile` lists the rule changes, the dropped overrides and the board issues that would appear or go, matched by finding identity. A switch that would be refused is still previewed and carries its `refusal`.
- **Checks** read the effective rules: copper clearance, board edge with cutouts and slots (`layout/board_edge.rs` on `Project::board()`), via drill size, annular ring, aspect ratio, drill to copper, hole to hole, mounting holes, courtyard clearance, the footprint mask-web and silkscreen lint, and the land estimators. The routing and via refusals do too.
  - Messages give the rule, its value and its source after "; ", for example "…; needs 0.25 mm (Fine 4/4: min mask web)". The issue title is the first clause, so it stays the same when the profile changes.
  - `layout/limits.rs` caches the resolved limits for one check run.
- **Desktop.** The Board card's Rules row opens the docked Design rules panel (`pcb_editor/rules_panel.rs`).
  - The panel has a profile picker with provenance and a grouped rule table (profile value, effective value, Reset) with live overrides.
  - A profile switch is previewed first. A refused switch shows its reason as a blocker. The switch commits as one `rules.set_profile`.
  - The panel also offers Save as profile and the user library: rename, delete, import and export.
  - Issues rows whose limit is a rule offer "Show rule".
- **Profile library** (`crates/io/src/rule_profiles.rs`): one `ProfileFile` (`PROFILE_FILE_FORMAT` 1) per profile, stored in `UserDirs::config/rule-profiles/`. Writes are atomic; reads are bounded (256 KiB per file, at most 256 profiles). Damaged or unsupported files are reported, never rewritten. The CLI has `rules-profile list|import|export`.

## Net classes

- **Core** (`rules/netclass.rs`):
  - Each class has a track width, a minimum width, a clearance, a via drill and diameter, and a fill connection. The diff-pair fields are reserved.
  - A class's values are checked against each other.
  - An explicit assignment beats the first matching name pattern (`*`/`?`). Nothing else assigns a class.
  - Assignments follow nets through `nets::reconcile`:
    - A survivor without its own class inherits the merged-away nets' class when they agree.
    - A net that splits off takes its origin's class.
    - Nets that are gone drop their assignment.
  - `net_class.list` reports every class with its resolved values and every net with its class and basis.
- **Routing.** `layout.route`, `route_apply`, `track_set` and `route_layer_change` refuse widths below the net's class minimum or the rule minimum. `layout::route_width` and `layout::class_via` give the class defaults. Clearance checks use the class and class-pair values.
- **Desktop.**
  - The Route tool starts at the routed net's class width; a typed width stays for that net. The Via tool and layer changes place the class's via.
  - The Track, Via and schematic Net cards have a Class row.
  - The Net classes card (`pcb_editor/net_classes.rs`) shows classes, values, class-to-class clearances, members and name patterns. It opens from the Board card's Net classes row; it and the Design rules panel replace each other in the subject slot.

## Copper fills and keepouts

- **Computation** (`layout/fill.rs`, `layout/fill/compute.rs`) is a pure, deterministic function of the project. It uses `i_overlay` (MIT OR Apache-2.0; see [attribution](../ATTRIBUTION.md)). For one fill:
  1. The fill's outline is clipped to the board outline, shrunk by the fill-to-board-edge clearance.
  2. These are subtracted at the effective rule and net-class clearances: cutouts and slots, mounting holes, keepouts, other-net copper and higher-priority fills.
  3. Same-net pads and vias connect solid or through thermal reliefs (spoke count, width, gap and angle).
  4. A minimum-width opening drops slivers.
  5. Hatching and island removal are options.
- **Results.** The copper (`FilledCopper`) is stored on the fill with the key of its inputs.
  - `inputs_key` covers only the objects within the fill's reach, so an edit elsewhere leaves the fill fresh.
  - `fill.store` refuses copper whose key is stale.
  - The fill joins (the pads, vias and tracks of each island) count in board connectivity and airwires.
- **Checks:** `fill.clearance`, `fill.empty`, `fill.island_unconnected`, `fill.net_missing`, `fill.stale`, `fill.thermal_starved` and `keepout.track`/`via`/`pad`.
- **Background refill.** The desktop fill worker (`apps/pcbcraft/src/fill_worker.rs`) computes stale fills off the UI thread with `FillCache` and `compute_stale`. It stores them with `Engine::store_fills`, which adds no undo step. The `fill.refill` command is the explicit, undoable refill.
- **Measured** (1 November 2026, macOS arm64, release build): one fill of a 200 × 200 mm plane over 10,000 through vias took 1.40 s, and its inputs key 2.1 ms (`a_large_board_fills_in_seconds`, which prints both).
- **PCB 2D** (`fill_paint.rs`, `fill_tool.rs`, `fill_card.rs`):
  - Fill copper is drawn under its layer's tracks in the layer's opaque colour: solid, hatched or outlines only, set in the view popover's Fills row. It is triangulated once per copper with `spade` and cached.
  - Unfilled fills show as dashed outlines.
  - The selected fill's outline has vertex and midpoint handles. Each release is one `fill.edit`.
  - The Fill tool draws outlines with lines and three-point arcs. The Fill card edits net, layer, priority and settings, with Refill and Delete in its footer.
- **PCB 3D** (`crates/solidcraft-integration/src/project/fill.rs`):
  - Each region of fill copper is a saved `FillCopper` body, one component per fill, on its layer and as thick as its foil.
  - Inner fills are cut from the laminate.
  - Outer fills get a display-only mask cap.
- **Showcase.** The showcase has a bottom GND pour, an inner GND plane and ten stitching vias. `Engine::showcase` refills them.

## Board shape, cutouts and edge-relative placement

- **Model** (`model/board.rs`, `layout/outline.rs`):
  - `BoardShape` is a `Rectangle` (from `board_size`) or a `Sketch(BoardSketch)`. A sketched board keeps `board_size` at its solved bounding box.
  - `Project::board()` returns the solved shape: the outer outline, cutouts, slots, mounting holes and named edges. For a rectangle, the edges are bottom/right/top/left.
- **Sketch** (`crates/solidcraft-integration/src/board.rs`). `SketchBoardEvaluator` runs on the footprint sketch machinery: canonical sketch check, parameters in Solidcraft's expression language, bound dimensions and the solver.
  - `board.start_sketch` turns the rectangle into a fully constrained sketch driven by `width` and `height`.
  - `board.edit` applies sketch edits, parameters, outline/cutout/slot/mounting-hole roles, edge names, real tangent fillets and milled slots as one undo step. A fillet keeps a virtual corner, so dimensions and named edges still run corner to corner.
  - The edit is refused when the shape is unsolved, over-constrained, open or out of range.
- **Edge constraints** resolve after every operation (`layout::outline::follow`).
  - A placement, fill or keepout lands at its named edge, straight or arc. Its position is set by an anchor, `along` and `offset` expressions over board parameters, an alignment (origin, near side, or the centre of the courtyard or pads), and an optional rotation that follows the edge. Parts move when board parameters change.
  - Moving a constrained part elsewhere is refused.
  - A constraint whose edge or target is gone is reported as `board.constraint_unresolved`. A stored sketch that no longer solves is reported as `board.shape_unsolved`.
- **3D.** The laminate extrudes the solved outline and cuts its cutouts, slots and mounting holes. Fill computation and board-edge checks read the same shape.
- **Desktop** (`board_shape.rs`, `board_shape_ui.rs`, `edge_card.rs`):
  - Layout's Shape button opens the Board shape mode on the footprint editor's sketch view.
  - The tools are Line (L), Arc (A), Circle (C), Rectangle (R, with a parametric corner radius), Fillet (F), Dimension (D, bound to board parameters), Cutout (U), Slot (Shift+H), Mounting hole (H) and Name edge (E). X toggles construction geometry. Every gesture is one `board.edit`.
  - The Placement card's Board edge section has "Constrain to edge", which ties the part to the nearest named edge in place. Its edge, anchor, expressions, alignment and turn-with-edge commit live, and the canvas draws the constraint as dimensions.
  - The Board card shows the shape and edits the board parameters.

## Limits

- Push-and-shove is not part of this work; routing respects class widths and the rule minimums.
- Document validation still bounds vias by the board's bounding box (`board_size`). A via outside a sketched outline but inside its bounding box is a board-edge finding, not an invalid document.
- The Board shape mode, the Fill tool and card, the Net classes card and the Design rules panel are covered by engine and egui-frame tests (`the_host_evaluator_solves_a_cutout_gesture_on_the_engine` drives a Cutout gesture through the linked sketch engine). The integrated app has not been checked by hand.
