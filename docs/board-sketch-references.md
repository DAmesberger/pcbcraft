# Board sketch, dimensioned parts and housings (design)

Design for review, 9 November 2026. Base: `component-model` at 88fc2bb. Nothing here is built.

The founder asked: "can we do complex board shapes also with solidcrafts sketch solver? and use all edges/curves as dimension references to each other and to 3d parts on them?", and then for "the design (scope, data model, and which Solidcraft API it would use)". He then set these directions:

1. **The board outline is a closed loop.** The shape is a PCB outline as soon as its curves form a closed loop that Solidcraft can extrude.
2. **Parts are assembled, not projected.** A part's 3D model sits on the board as in a Solidcraft assembly. Its seating on the board surface is given and cannot be edited. Dimensions go straight to the faces, edges and points of the part's own model.
3. **Only dimensioned parts are solved.** A part that no dimension touches keeps its plain stored x, y and rotation, as today.
4. **Parts never drive the board.** The direction is always board → dimensioned parts.
5. **A housing drives the board.** An enclosure designed in Solidcraft, with the PCB as one instance, can drive the board outline, its mounting holes and its place in the housing.

The result has two solves. The board sketch is solved first, from its own dimensions and from housing references. Then each group of dimensioned parts is solved against the solved board.

## 1. Where it stands (verified in code)

- **Board solve.** Solidcraft's sketch solver solves the board sketch. `SketchBoardEvaluator` (`crates/solidcraft-integration/src/board.rs`) runs it behind the core's `BoardShapeEvaluator` (`crates/editor-core/src/layout/outline.rs`). Any board curve or point can already be dimensioned to any other: `SketchEdit::AddDimension` takes any `SketchRef`s (`library/footprint_source.rs`).
- **Pcbcraft's sketch subset.** The board shares the footprint sketch subset (`footprint/check.rs`, `subset`), which refuses free-form curves, links, patterns and `PatternPoint`. Loops are built from lines, arcs and circles only (`board.rs`, `pieces_of`). `library::Segment` is a line or a bulge arc, and fills, edge checks, fab output and the 3D laminate all read it.
- **The outline must close on every edit.** The `outline` role is an explicit list of curves. An edit that leaves it open, unsolved or out of range is refused (`loop_of`; [board shape](board-rules-and-shape.md#board-shape-cutouts-and-edge-relative-placement)).
- **Parts relate to the board in one direction.** An `EdgeConstraint` (`model/board.rs`) places a placement, fill or keepout. It names an edge, an anchor, `along` and `offset` expressions, an alignment, `follow_edge` and a rotation. After every operation, `layout::outline::follow` places the target. There are no part-to-part dimensions and no references to 3D geometry.
- **Seating is fixed data, not solved.** Each part occurrence in the native board model has one Solidcraft `planar` joint to the board (`native.rs`, `joint_params`).
  - Its offset is the expression `board_thickness` on top, or `0 mm`, hanging and flipped, on the bottom.
  - Its values `[x, y, angle]` are the parameter expressions `<prefix>_x`, `_y`, `_angle` (`project.rs`, `BoardJoint`), set from the core `Placement`.
  - A planar joint's motion is `shift(x, y, 0) · rot_z(angle)`, so it has no z value (`doc/src/joints.rs`, `motion`). z comes only from the offset expression.
  - Solidcraft's loop solver leaves alone every value an expression sets (`set_values`: "Values the loop solver leaves alone: those a motion link sets and those an expression does"), and Pcbcraft makes no other joints on parts.
  - So Solidcraft never solves a part's pose, z included. It reads the pose from the stackup and from `Placement`.
- **Part 3D models.** A `PackageModel` is `Envelope`, `Generated`, `Library` or `PartFile` (`library/package_model.rs`). A part file is an exact `ModelRef` revision, evaluated through `Session::opened_validating` and cached by key (`models/part_file.rs`). Only part files carry Solidcraft's persistent face and edge names.

## 2. What Solidcraft offers (upstream, public)

Checked in two places: upstream `tangency-labs/solidcraft` main as checked out at `/Users/amd/dev/solidcraft` (4f3ac07; `origin/main` has since moved, so check again at repin), and our pin (`DAmesberger/solidcraft` 04d3c2f, `~/.cargo/git/checkouts/solidcraft-258829c3ab1d7265/04d3c2f`). Paths are relative to `crates/`, and all exist at both unless noted.

| Need | API | Path |
| --- | --- | --- |
| Free-form curves | `CurveKind::{Ellipse, Spline{control, degree, periodic, nurbs}, Conic}` (Solidcraft has no elliptical arc) | `sketch/src/model.rs` |
| Curve geometry, arc fitting | `Sketch::segs(ci) -> Vec<Seg2>`; `Loop2::fit_arcs(tol)` | `sketch/src/model.rs`, `geom/src/arcfit.rs` |
| Closed regions, open ends | `find_drawn_profiles` → `Profile{region, outer_curves, hole_curves}`; `loose_ends` | `sketch/src/profiles.rs`, `sketch/src/ends.rs` |
| A rigid part frame in a sketch | `ConstraintKind::PatternPoint{p, q, directions: [(line, offset)]}`: `q = p + u·dx + u⊥·dy`, where `u` is a live line's direction | `sketch/src/model.rs`, `sketch/src/solver.rs` |
| Relations | `PointLineDistance`, `PointCircleDistance`, `LineCircleDistance`, `CircleCircleDistance`, `Angle`, `Parallel`, `Collinear`, `PointAlong`, `ArcLength`, `Smooth`; `Constraint.driven` (a reference dimension) | `sketch/src/model.rs` |
| Holding geometry | `solve_with(sk, &Hold, …)`; `SolveReport{dof, oversized, failing}` | `sketch/src/solver.rs` |
| Persistent topology | `naming::face_on_body(body, name, pick)`; upstream also returns `inexact` (split or merged face), our pin does not. `edge_names`, `find_edge` | `doc/src/naming.rs` |
| A face or edge in a plane | `project::resolve_face_indices(st, plane, LinkKind::{Project, Intersect}, body, indices)`, `project::resolve(…, LinkSource)` | `doc/src/project.rs` |
| Assembly frames | `joints::{NamedOrigin, resolve_origin, occurrence_world}`, `world_state` | `doc/src/joints.rs`, `doc/src/eval.rs` |
| Designs linked from other files | `component.insert_design`, `update_links`, `relink`, `break_link`, `edit_in_place`; `DesignLink{path, feature}` | `engine/src/cmd/component_link.rs`, `engine/src/cmd/component.rs`, `doc/src/document.rs` |
| Parameters across files | `parameters.export` / `parameters.import` only: a one-off copy with no live link | `engine/src/cmd/parameters.rs`, `doc/src/params.rs` (`import_params`) |

## 3. Scope

### In

1. **Closed-loop outline.** The board is the closed region formed by the board sketch's drawn curves (§4.1).
2. **Free-form curves.** Splines (fit, control, closed periodic), ellipses and conics, with the §2 relations.
3. **Dimensioned parts.** A placement, or a drawn fill or keepout (as `EdgeConstraint` allows today), becomes a solver frame only while a dimension or constraint touches it. Its unknowns are x, y and the rotation about the board normal.
4. **Part features.** These can be dimensioned:
   - the origin and axes;
   - pad centres and sides;
   - courtyard sides and outline;
   - authored part-file anchors (`pin1` and named construction points);
   - for part-file packages, 3D faces, edges and vertices by persistent name (§4.4).
5. **Pairings.** Board to board (as today), part to board, part to part, and a 3D face to any of them. Reference dimensions only measure.
6. **Housing references.** Faces, edges, axes and sketch curves of a Solidcraft housing design, located through a frame in that housing, drive the board sketch (§5).
7. **`EdgeConstraint` replaced.** Its data, its `follow` pass and its three commands are replaced by part dimensions (§4.7).

### Out

- **Parts driving the board**, by founder decision.
- **Other 3D relations of a part.** Heights, tilt, and mates between parts beyond the given seating. A dimension to a face parallel to the board can only be a reference dimension.
- **Faces with no single trace in the board plane** (sloped or free-form part faces). They are refused with a reason. A housing wall with draft is handled by a section at a chosen board surface (§5.3).
- **3D references on models without stable names.** `Envelope`, `Generated` and `Library` (mesh) models offer only pads, courtyard and their body box. Upstream `face_on_body` returns `None` for mesh bodies.
- **Other participants:** tracks, vias and copper.
- **Writing into the housing file.** Pcbcraft never edits it (§5.4).
- **Panels.** Several disjoint boards in one sketch are reported, not supported.

## 4. Data model

All types live in `pcbcraft-core`: pure, serde and `JsonSchema`, with no Solidcraft dependency. The board sketch stays an opaque `SketchBlob`.

### 4.1 Outline as a closed loop

The `outline` curve list is dropped. After the board solve, the evaluator runs `find_drawn_profiles` and accepts the result when:

- exactly one region remains once construction curves are ignored;
- its outer loop does not cross itself (the finding names the crossing from `intersections`);
- every hole loop lies strictly inside the outer loop, apart from the others.

The outer loop is the outline. Each hole is a cutout, unless a `CutoutRole` keyed by its curve ids marks it as a slot (plated or not).

| Sketch state | Board | Finding |
| --- | --- | --- |
| Valid region | The new `SolvedBoard` | none |
| No closed loop yet, or open ends | The last valid `SolvedBoard` kept; open ends marked from `loose_ends` | `board.shape_open`: advisory in Board shape mode, an error in checks and preflight |
| Crossing, several regions, or a hole touching the outline | The last valid shape kept | `board.shape_invalid`, with the place |

A drafting edit is no longer refused for leaving the loop open; only an unsolvable or out-of-range sketch is refused. Free-form curves reach `SolvedBoard` through `Sketch::segs` and `Loop2::fit_arcs(tol)`, with `tol` = `CHORD_MM` (0.01 mm, already used in `board.rs`; open question 3). Fills, checks, Gerber/DXF and the laminate therefore stay unchanged. Each solved loop records the sketch curve behind each segment (`SolvedBoard` gains `curves: Vec<SketchCurveId>` per loop), so a pick on the board's edge in 2D or on its side face in 3D names the same curve.

### 4.2 Dimensioned parts

Part dimensions are core data, outside the board sketch blob. The board blob holds only the board (and housing references, §5), so a part can never move it.

```rust
/// A dimension or geometric constraint that touches at least one part feature.
pub struct PartDimension {
    pub id: Id,
    pub kind: PartRelation,          // DimKind or GeomConstraint, as the board's
    pub refs: Vec<BoardRef>,         // 1–3
    pub value: Option<DimValue>,     // DimValue gains Expr(Expr) over board parameters
    #[serde(default)]
    pub reference: bool,             // measures only (Solidcraft's `driven`)
}

pub enum BoardRef {
    Board(SketchRef),                // a board-sketch point or curve, by its stable sketch id
    Part(FeatureRef),
}

pub enum FeatureRef {
    Origin { target: ConstraintTarget },                          // Placement | Fill | Keepout
    Pad { placement: Id, pad: PadId, at: PadPoint },              // Centre | Side(Side)
    Courtyard { placement: Id, side: Option<Side> },              // None: the outline
    BodyBox { placement: Id, side: Side },
    Anchor { placement: Id, name: AnchorName },                   // part-file construction point
    Face { placement: Id, body: BodyName, face: FaceName, pick: [f64; 3] },
    Edge { placement: Id, body: BodyName, edge: EdgeName3d, pick: [f64; 3] },
    Vertex { placement: Id, body: BodyName, edge: EdgeName3d, end: End },
}
```

`Project::part_dimensions: Vec<PartDimension>` replaces `Project::edge_constraints`. A part is dimensioned when some `PartDimension` names it. Every other part is plain stored data and never enters a solve.

**The parts solve.** It runs after the board solve, for the dimensioned parts only:

1. Group the dimensioned parts into connected components. Two parts are connected when one dimension names both.
2. For each component the operation touched, build a scratch Solidcraft sketch:
   - the board's solved curves and points that the component's dimensions name, held fixed;
   - per part, an origin point and a construction axis line;
   - the part's referenced features as points or curves, bound to the frame with `PatternPoint` and `directions = [(axis, local offset)]`, with a fixed `Radius` for circles and the local y mirrored on the bottom side;
   - the dimensions.
3. Solve it. Each part's x, y and rotation are the solution.

An unconstrained degree of freedom keeps its stored value, because the solver moves geometry as little as it can. The feature geometry always comes from the library snapshot: the footprint for pads and courtyard, the evaluated part file for faces. It is rebuilt for each solve and never stored, so there is a single source of truth.

This replaces the two-stage solve of the previous draft. Parts never drive the board, so the board solve is today's solve, unchanged. The parts solve is a second, separate call whose board inputs are constants. No `driven` toggling or `Hold` bookkeeping is needed. Components are small and independent (§7).

### 4.3 Seating is given, never solved

The board sketch is 2D, so z does not exist in either solve. A part's side (`Placement::layer`) and the stackup set its seating: it sits on top at `board_thickness` or hangs from the bottom at 0 mm, turned over. That reaches the native model as the planar joint's offset expression, and its `[x, y, angle]` as parameter expressions written from `Placement` (§1). Solidcraft's assembly solver therefore treats none of it as unknown. Changing the stackup moves parts in z only, never in the plane. The design keeps it so:

- part poses are never expressed as Solidcraft mates;
- Pcbcraft creates no joints on parts other than the one planar joint;
- that joint's values stay expressions.

### 4.4 3D faces of the assembled part

With z, tilt and roll given, a part-file feature enters the parts solve as follows:

| Part feature (part-file frame) | In the board plane | Example |
| --- | --- | --- |
| Planar face square to the board | Its line (exact) | USB-C shell front face flush with the board edge |
| Cylinder or cone with its axis along the normal | Its centre (and radius) | A boss or pin over a mounting hole |
| Straight or circular edge parallel to the board | Its line or arc | A housing's bottom edge |
| Vertex | Its point | A housing corner |
| Face parallel to the board | No plane geometry; a reference dimension only | A height |
| Sloped or free-form face | Refused: "pick an edge of this face" | |

The stored reference is the face by its persistent name on the part. Nothing is projected or copied into storage. Its plane form is exact because x, y and the rotation are the only unknowns. In PCB 3D, the dimension is drawn to the face.

The integration resolves a face as follows. It evaluates the part file through the existing cache (`part_file::evaluate`, which keeps the source `Document`; it would also keep the `ModelState`). It finds the face with `naming::face_on_body` and takes its trace from `project::resolve_face_indices(state, XY, LinkKind::Project, body, [index])` in the part frame, which is the footprint frame by the part-file convention. Edges and vertices use `find_edge` and `project::resolve`. Names come from the part file's own design, not from the board model, because `part_file::insert` renumbers features.

### 4.5 Identity, revisions, missing references

| Reference | Identity | When the revision changes |
| --- | --- | --- |
| Part | Placement instance `Id` | A deleted placement: `Lost` |
| Pad | `PadId` | Absent: `Lost` |
| Courtyard / body box side | The box side in the part frame | Always resolves |
| Anchor | Its authored name in the part file | Absent: `Lost`. Never matched heuristically |
| Face / edge / vertex | (body, persistent name, pick in the part frame), within an exact `ModelRef` | Re-found by name. Upstream `inexact` → `Inexact`; not found, or no longer square → `Lost` |
| Board curve or point | Stable sketch id | Deleted: `Lost` |

A lost reference keeps its last geometry. That geometry is kept per reference with the dimension as a `last: Vec<Point>` snapshot, so the part does not jump. It is reported as `board.reference_lost`; an inexact one as `board.reference_inexact`. Neither is dropped or silently re-pointed. Both block fab preflight until the user picks the reference again or deletes it. A library update's impact preview lists every reference it would make `Lost` or `Inexact`, before anything changes, and `update.apply` acknowledges them.

### 4.6 Commands, undo, persistence

- **Undo.** Each mutation is one atomic undo step through the one engine.
  - `board.edit` gains `SetCutoutKind` and the sketch edits `AddSpline`, `AddEllipse` and `AddConic`. `GeomConstraint`/`DimKind` gain the §2 relations.
  - New `layout.dimension` / `layout.dimension_edit` / `layout.dimension_delete` (or one `layout.dimensions` batch) carry `PartDimension`s and re-solve the touched component.
- **Moving a dimensioned part.** A move or rotate becomes a solver drag of its frame, so a part with one dimension to an edge slides along it. A move that cannot land is refused in today's words (`outline::held_message`). Locked parts stay refused.
- **Persistence.** `Project::edge_constraints` and `BoardSketch::outline` are removed, and `part_dimensions` is added. The format has one version and no migration. A file that still has the old fields is refused by `deny_unknown_fields`. Only the showcase uses them (`Engine::showcase_hosted`, J1 held to the left edge), and it is regenerated.

### 4.7 What replaces `EdgeConstraint`

| `EdgeConstraint` | Part dimensions |
| --- | --- |
| `target`, `edge` | The target's origin or a feature, and the named edge's curve. A filleted edge measures on a construction line between its virtual corners (today's rule) |
| `anchor` Start / End / Middle / Fraction | The edge's end, `Midpoint`, `PointAlong { at }` |
| `offset` | `PointLineDistance` to the edge (`PointCircleDistance` for an arc edge) |
| `along` | `PointLineDistance` to a construction line square to the edge through the anchor (`ArcLength` on a concentric construction arc for an arc edge) |
| `align` Origin / NearSide / Centre | The origin, a courtyard side (stored explicitly, never re-inferred), or the courtyard centre |
| `follow_edge` + `rotation_deg` | `Angle(axis, edge) = rotation`; without it, the rotation stays free and keeps its stored value |

The desktop's "Constrain to edge" creates these dimensions in one step. The Edge card, the hold mark and `layout::move_refusal` read part dimensions instead. `layout::outline::follow` is deleted.

### 4.8 One source of truth for the pose

`Placement { position, rotation_deg, layer }` stays the stored pose that copper, the copper graph, checks and fab output read. For a dimensioned part, the engine writes it from the parts solve in the same operation. The native planar joint is written from `Placement`, as today. The flow is:

board sketch → board solve → parts solve → `Placement` → joint expressions → 3D.

There is one solver per stage, and Solidcraft's assembly solver decides nothing.

### 4.9 Copper, fills, checks

- **Moved parts.** A part moved by its dimensions is the same placement change `layout.move` makes. The copper graph, fill keys (`inputs_key`) and checks see an ordinary move. A dimension never decides connectivity ([copper graph](copper-graph.md#where-connectivity-lives)).
- **Board changes** (from its own dimensions or from a housing update) change `SolvedBoard`. Board-following fills go stale until refilled, and the edge-clearance check reads the new outline.
- **New findings:** `board.shape_open`, `board.shape_invalid`, `board.reference_lost`, `board.reference_inexact`, `board.part_unsolved` (conflicting dimensions, or a component over the solver's cap, §7) and `board.housing_stale` (§5). `board.constraint_unresolved` is removed.

### 4.10 CLI and MCP

The typed commands take ids. The automation layer also accepts a compact form and resolves labels to ids when it receives the request; the labels are never stored.

```
edge(left)   curve(l7)   J1.origin   J1.pad(A6)   J1.pad(A6).side(left)   J1.courtyard.left
J1.anchor(pin1)   J1.face(Shell, "F3:side:l2")   housing.face(Case, "F12:side:l3")
```

`board.shape` reports the dimensioned parts with their remaining degrees of freedom and reference status. `board.references` lists every reference with its geometry and status, and, when asked, the faces of a part or the housing that can be referenced.

## 5. A housing drives the board

### 5.1 What upstream Solidcraft supports

Paths below are upstream main and our pin alike.

- **Assemblies of designs from other files.** `component.insert_design` adds a component linked to another `.sc` design (`engine/src/cmd/component_link.rs`; `DesignLink { path, feature }` in `doc/src/document.rs`). The linked design is evaluated on its own. Its solid bodies are copied into an Import feature: an exact B-rep, kept so the assembly opens without the file.
  - `component.update_links` reads the file again and replaces the bodies.
  - `relink` changes the file; `break_link` keeps the bodies and forgets the file.
  - Faces of linked bodies keep the linked design's names (`Feature::link_faces`), so a joint on a linked face survives an update and reports when the face is gone.
- **Mates.** Joints between occurrences (`Rigid`, `Planar`, …) and partial relations (`Parallel`, `PointOnPlane`, `LineOnPlane`, …), with origins snapped to faces, circles and edges by persistent name (`Snap`, `doc/src/joints.rs`, `joints_relations.rs`). Named joint origins (`Assembly.origins: Vec<NamedOrigin>`) are reusable frames. The loop solver places occurrences.
- **In-context editing.** `component.edit_in_place` opens a linked design in its own tab "with the assembly drawn around it" (`engine/src/cmd/component.rs`).

What is **missing** upstream (as found; I did not see these anywhere in `doc` or `engine`):

- A linked design does not take parameter values from its assembly. A linked component is a B-rep copy of the file as last saved, and the assembly cannot set its parameters.
- There is no live parameter link between files. `parameters.export` / `parameters.import` copy values once.
- A linked design cannot keep a reference to its host assembly's geometry. Edit in place only draws the assembly around it.

So in Solidcraft the dependency runs bottom-up: an assembly reads its parts, never the reverse. A housing therefore cannot push geometry into the PCB design by any upstream mechanism. Pcbcraft must read the housing.

### 5.2 Options

| Option | How | For | Against |
| --- | --- | --- | --- |
| (a) Linked parameters | Board parameters bound by explicit name to housing user parameters (`inner_w`), read from the housing file | Simple; uses existing board parameters | Only what the housing author made a parameter; no geometry, no mounting-hole positions from bosses |
| (b) Housing geometry as fixed references in the board sketch | The board sketch references housing faces, axes, edges and sketch curves, located in a housing frame, as fixed geometry. This is the part-face machinery (§4.4) in reverse: the housing is a constant and the board curves are the unknowns | Any geometry: walls drive the outline, boss axes drive mounting holes; one solver | Pcbcraft must evaluate the housing design |
| (c) Housing as master document | Pcbcraft reads the board outline from a housing sketch, and the `.pcbcraft` has no own sketch | Nothing to keep in sync | The board's source of truth leaves Pcbcraft; Pcbcraft could not edit or check the shape alone; the electrical design would depend on a mechanical file |

**Recommendation: (b), with (a) as a small subset of it.** A housing parameter can be read by explicit binding as a board-parameter value. Every reference is chosen by the user, never inferred from names.

### 5.3 Design of (b)

- **The board's frame in the housing.** The user picks a named joint origin in the housing design (`NamedOrigin`; for example a frame on the floor of the PCB bay). Pcbcraft reads its world frame with `joints::resolve_origin` on the evaluated housing. This frame is the board's origin and plane: board x/y on its xy, the board's bottom on its z = 0.
  - The housing author holds the PCB occurrence to that origin with a rigid joint.
  - Pcbcraft does not read the frame from the PCB occurrence's own mates (bosses to the board's holes). Those depend on the board's own geometry, which the housing is about to drive, and that would be a loop across two files. A housing that places the board only through the board's geometry is reported (`board.housing_frame`), not followed.
  - The board's place in the housing is therefore the housing's business. Pcbcraft coordinates stay board-local.
- **Housing references.** Each is a `HousingRef { body, face | edge | axis | sketch curve (persistent name), pick, section }` stored in `BoardSketch::housing.references`.
  - It is resolved with the same calls as part faces: `naming::face_on_body`, then `project::resolve_face_indices` / `project::resolve` on the housing's `ModelState`, in the board frame.
  - `section` is `Project` (along the board normal), or `Intersect` at the board's top or bottom surface, read from the stackup. `Intersect` gives a wall with draft its exact section where the board meets it.
  - Its geometry enters the board sketch as fixed points or curves owned by that reference, refreshed at each housing update. The board's own curves are dimensioned or constrained to them: outline offset from an inner wall, mounting-hole centre coincident with a boss axis. Housing references are board-solve inputs; parts never see them except through board curves.
- **Source of truth.** The housing geometry lives in the housing file. The `.pcbcraft` stores:
  - the link: a path relative to the project, like Solidcraft's `DesignLink`;
  - the chosen frame;
  - the references;
  - a snapshot of their last resolved geometry and the housing file's hash.

  The project opens, solves and checks without the housing file, as a Solidcraft assembly opens without its linked parts.
- **How a housing edit arrives.** Never silently.
  - The desktop watches the file and reports `board.housing_stale`: "the housing changed; review the update".
  - The update itself is an explicit command, `board.housing_preview` then `board.housing_update`, like a library update. The preview lists the board curves, holes, fills and dimensioned parts that would move, and the references that would be `Lost` or `Inexact`.
  - Applying it is one undo step. It is refused atomically if the board would no longer solve or form a closed region; the preview names why.
  - The CLI and MCP have the same two commands.
- **Conflicts and lost references.** A reference whose face or frame is gone keeps its snapshot geometry, so the board does not move, and is reported as `board.reference_lost`, which blocks fab preflight. A housing frame that cannot be resolved blocks the whole update. A housing saved by a Solidcraft version the pin cannot read is refused as unsupported (`io::read_design`), never half-read.
- **The other direction.** Pcbcraft already saves the PCB as a parametric `.solidcraft`. The housing author inserts it with `component.insert_design`, and brings in a new board with Solidcraft's own `component.update_links`. Pcbcraft never writes the housing file.
- **No fork.** Everything above is public upstream: `io::read_design`, `Session::opened_validating`, `joints::resolve_origin`, `world_state`, `naming`, `project`. No Solidcraft change is needed.
  - Optional generic upstream additions, not required: a live cross-file parameter link, and in-context references from a linked design to its assembly. Both are general CAD features, and either would make (a) or a Solidcraft-only workflow nicer.

## 6. Solidcraft gaps and where they are handled

| Gap | Handling |
| --- | --- |
| No rigid-set constraint | `PatternPoint` with `directions` holds a feature rigidly in a frame; it solved without a pattern entry (§7) |
| One connected group is capped at `MAX_COMPONENT_VARS` = 4,000 unknowns (`sketch/src/solver.rs`) | Parts solve per component; a component over the cap is `board.part_unsolved` with the solver's `oversized` count |
| The board subset refuses splines and links | A board subset separate from the footprint one (Pcbcraft code) |
| `face_on_body`'s `inexact` exists only upstream | Adopted when Pcbcraft next repins to upstream main |
| No top-down or live cross-file links (§5.1) | Pcbcraft reads the housing (§5.3) |

**Rejected alternative: Solidcraft's assembly solver as the authority for part poses.** It would need the evaluated board model, which Pcbcraft builds in the background, inside every synchronous core operation. It cannot drive a sketch. Its loop solver is bounded per group (`MAX_LOOP_VALUES` = 64, `MAX_LOOPS` = 100, `doc/src/joints.rs`). It would also add a second pose solver. The seating stays a joint whose values are expressions (§4.3).

## 7. Performance

**Measured.** A scratch benchmark, not committed: the Solidcraft sketch crate at our pin, release build, Apple M5 Max, with load average 30–49 from other agents. It built a fully dimensioned board rectangle plus N rigid parts, each with an origin, an axis and two referenced points bound by `PatternPoint`, dimensioned to the edges or chained to the previous part. Times are for the whole set solved together, which is the worst case: every part in one component with the board free.

| Dimensioned parts | First solve | Board width edit |
| --- | --- | --- |
| 10 | 0.2–0.5 ms | 0.1 ms |
| 50 | 1.6–2.0 ms | 0.6 ms |
| 100 | 5.2–5.8 ms | 2.0–2.4 ms |
| 300 | 49–54 ms | 15–18 ms |
| 500 | refused: 4,006 unknowns > 4,000 | |

With the board held (`solve_with` + `Hold`), which is the shape of the parts solve in §4.2, 500 parts solved in 7.2–9.3 ms. Each part, or each chain of 20, was then its own group.

**What that means for real boards.** A board has tens of dimensioned parts (connectors, switches, LEDs, mounting hardware), not hundreds. Resistors and capacitors stay plain data and cost nothing. The parts solve is per component and runs only for the components an operation touched, so a typical edit solves one part or a short chain. The 4,000-unknown cap is about 500 parts with two references each in one chained component. A component over it is refused with the solver's own count.

**Targets (labelled as targets, measured end to end in S5):**

- any edit re-solves within one 60 Hz frame (16 ms) on a board with 100 dimensioned parts;
- a housing update completes within 1 s, including evaluating the housing design.

The bare solver meets the first. The housing evaluation time is not measured.

**Selection and camera rebuild nothing.** The board solve runs only when the board sketch, its parameters or the housing snapshot change. The parts solve runs only for components whose dimensions, part placements or referenced library revisions changed. Hover, pick, selection and the camera read stored `SolvedBoard`, placements and cached feature geometry. Part moves reach 3D as new joint expressions through the existing in-place update path.

## 8. Slices (agent hours)

The contract commit (C0) comes first. After it, S1 and S2 can run in parallel; S3 needs S2, S4 needs S3, and S6 needs S1 and S3.

| Slice | Content | Estimate |
| --- | --- | --- |
| C0 | Contract: types, commands, schemas, finding codes, test skeletons | 2 |
| S1 Closed-loop outline, free-form curves | Derived outline, open and invalid states with the last valid shape, cutout kinds, the board subset, spline/ellipse/conic edits and tools, `fit_arcs` into `Outline`, segment→curve map, open-end markers | 9 |
| S2 Dimensioned parts | `PartDimension`, the per-component parts solve, pad/courtyard/body-box/anchor references, drag-to-move, `EdgeConstraint` removed and expanded (`follow`, commands, Edge card, hold mark, move refusal, showcase), automation reference syntax | 12 |
| S3 Part 3D references | Part-file faces, edges and vertices by persistent name, the §4.4 rules, reference status and findings, library-update impact rows | 8 |
| S4 3D picking and display | Pick a part face or the board's side face in PCB 3D to start a dimension; draw dimensions to faces in 3D; shared selection and undo with 2D | 6 |
| S5 Evidence | Timings against §7, a VM UI run (USB-C shell flush with a spline edge), editor evidence | 2 |
| **S6 Housing link (later)** | Housing link and frame, housing references in the board sketch, snapshot and hash, preview/update commands, file watch, findings, CLI/MCP | 12 |
| S7 Housing UI and evidence | Link a housing and pick its frame; pick housing faces in a 3D context view; the update review dialog; a VM run with a real enclosure | 5 |
| | **Total** (S6–S7 later: 17) | **56** |

## 9. Open questions for the founder

The directions above settle the earlier questions on two-way coupling and on parts driving edges.

1. **Pose authority.** Should the board and parts solves decide all poses, with Solidcraft's assembly keeping only the seating joint as expressions? *Recommendation: yes (§4.8, §6).*
2. **Lost references and fabrication.** Should lost or inexact references block fab output? *Recommendation: yes, with no waiver. Nothing moves meanwhile.*
3. **Spline tolerance.** Fitted arcs within 0.01 mm (`CHORD_MM`) for free-form outlines, or Bézier segments that every consumer must handle? *Recommendation: fitted arcs, with the tolerance made a design rule.*
4. **Packages without part files.** Should generated and library models offer 3D face references? *Recommendation: no; converting a package to a part file is the path ([part templates](part-templates.md)).*
5. **Open outline outside Board shape mode.** Should it be an error? *Recommendation: yes. The board keeps its last closed shape, and the error says so.*
6. **The board's frame in the housing.** A named frame in the housing, or the PCB occurrence's mates? *Recommendation: a named frame. Mates to the board's own holes would loop across files (§5.3).*
7. **How housing edits arrive.** Applied automatically, or watched and then reviewed? *Recommendation: watched, reported, and applied only through the preview and update, like library updates.*
8. **A housing file that is missing or moved.** *Recommendation: the project opens and solves on the snapshot; the link reports it as missing and offers relink, as Solidcraft's own links do.*
9. **Linked parameters, option (a).** Ship them with S6? *Recommendation: only as explicit bindings of board parameters to housing user parameters, inside S6; no name matching.*
10. **Upstream additions.** Propose a live cross-file parameter link, or in-context references, to tangency-labs? *Recommendation: not now. Pcbcraft does not need them, and an upstream PR needs your go.*
