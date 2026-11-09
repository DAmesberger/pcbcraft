# Board sketch, dimensioned parts and housings (design)

Design for review, 9 November 2026. Base: `component-model` at 88fc2bb. Nothing here is built.

The founder asked: "can we do complex board shapes also with solidcrafts sketch solver? and use all edges/curves as dimension references to each other and to 3d parts on them?", and then for "the design (scope, data model, and which Solidcraft API it would use)". Over three rounds of review he set these directions:

1. **The outline is a closed loop.** The board is a PCB outline once its curves form a closed loop that Solidcraft can extrude. An open loop is an error outside Board shape mode.
2. **Parts are assembled, not projected.** Their seating on the board surface is given and never solved. Dimensions go to the faces, edges and points of the part's own 3D model.
3. **Only dimensioned parts are solved.** Every other part keeps its stored x, y and rotation.
4. **Parts never drive the board.**
5. **A housing drives the board.** An enclosure designed in Solidcraft can drive the outline, the mounting holes and the board's position.
6. **Solidcraft sees the same geometry.** Solidcraft must be able to dimension between a connector on the board and an opening in the housing, in either direction. The board's parts must therefore be real faces with stable names inside the Solidcraft assembly.
7. **Face references on every model,** generated and library models included.
8. **The board is a sub-assembly of the housing,** placed by ordinary mates. Cycles across the two files are detected and reported.
9. **Changes are checked both ways.** A change in either file is checked and previewed, like a library update, before it is applied on the other side. A missing housing offers relink. Housing parameters bind explicitly.
10. **No upstream proposal for live cross-file links.** The round trip uses only what Solidcraft has. Pcbcraft writes its own board `.solidcraft`, never the housing.
11. **Lost or inexact references block fab output,** with no waiver. Free-form outlines reach fab output as fitted arcs, within a tolerance that is a design rule.

## 1. Where it stands (verified in code)

- **Board solve.** Solidcraft's sketch solver solves the board sketch. `SketchBoardEvaluator` (`crates/solidcraft-integration/src/board.rs`) runs it behind the core's `BoardShapeEvaluator` (`crates/editor-core/src/layout/outline.rs`). Board curves can already be dimensioned to each other.
- **Pcbcraft's subset.** The board shares the footprint sketch subset (`footprint/check.rs`), which refuses free-form curves, links and `PatternPoint`. Loops are lines, arcs and circles. `library::Segment` is a line or a bulge arc, and fills, checks, fab output and the laminate all read it. The `outline` role is an explicit curve list that must close on every edit.
- **Parts follow edges one way.** `EdgeConstraint` (`model/board.rs`) is resolved after each operation by `layout::outline::follow`. There are no part-to-part dimensions and no 3D references.
- **Seating is data.** Each part occurrence in the native board design has one Solidcraft `planar` joint to the board (`native.rs`, `joint_params`).
  - Its offset is `board_thickness` on top, or `0 mm`, flipped, on the bottom.
  - Its values are the parameter expressions `<prefix>_x`, `_y` and `_angle`, set from `Placement` (`project.rs`, `BoardJoint`).
  - A planar joint moves as `shift(x, y, 0) · rot_z(angle)` (`doc/src/joints.rs`, `motion`), and the loop solver never moves a value an expression sets (`set_values`). So Solidcraft never solves a part's pose, z included.
- **The native board design is rebuilt on structural change.** An edit that only changes values is replayed in place, but "any other difference ends the replay and the board is built anew" (`native/replay.rs`). A new build hands out new feature ids.
- **Part models.** A `PackageModel` is `Envelope`, `Generated`, `Library` (STEP or glTF, `library/model3d.rs`) or `PartFile` (`library/package_model.rs`). A part file's history is inserted into the board design with renumbered features (`models/part_file.rs`, `insert`).

## 2. What Solidcraft offers (upstream, public)

Checked in two places: upstream `tangency-labs/solidcraft` main as checked out at `/Users/amd/dev/solidcraft` (4f3ac07; `origin/main` has moved since, so check again at repin), and our pin (`DAmesberger/solidcraft` 04d3c2f). Paths are relative to `crates/`, and exist at both unless noted.

| Need | API | Path |
| --- | --- | --- |
| Free-form curves | `CurveKind::{Ellipse, Spline, Conic}` (Solidcraft has no elliptical arc) | `sketch/src/model.rs` |
| Arc fitting | `Sketch::segs`; `Loop2::fit_arcs(tol)` | `sketch/src/model.rs`, `geom/src/arcfit.rs` |
| Regions, open ends | `find_drawn_profiles`, `loose_ends` | `sketch/src/profiles.rs`, `sketch/src/ends.rs` |
| A rigid frame in a sketch | `ConstraintKind::PatternPoint{p, q, directions: [(line, offset)]}` | `sketch/src/model.rs`, `sketch/src/solver.rs` |
| Holding geometry, dependencies | `solve_with(…, &Hold, …)`, `determined_by`, `SolveReport{oversized}` | `sketch/src/solver.rs` |
| Persistent names | Faces are named after the feature that made them, by feature id (`F2:top`, `F2:side:l4`). Bodies without history are named from geometry and "marked unverified". `face_on_body` (with an `inexact` flag upstream only), `edge_names`, `find_edge` | `doc/src/naming.rs` |
| A face in a plane | `project::resolve_face_indices`, `project::resolve` | `doc/src/project.rs` |
| Mates | `Joint`, `JointKind`, `Snap` on faces, circles and edges by persistent name; `NamedOrigin`; `joints::solve_holding`, `resolve_origin`, `occurrence_world` | `doc/src/joints.rs`, `doc/src/joints_relations.rs` |
| Sketch links to bodies | `Sketch.links`, `LinkSource::{Face, Edge, Vertex, Body}` with `face_name`/`edge_name`; lost links keep their last geometry | `sketch/src/link.rs`, `doc/src/project.rs` |
| Linked designs | `component.insert_design`, `update_links`, `relink`, `break_link`, `edit_in_place`; `DesignLink{path, feature, inherited}` | `engine/src/cmd/component_link.rs`, `doc/src/document.rs` |
| Parameters | `parameters.export` / `import`, a one-off copy; `Document::param_values` | `engine/src/cmd/parameters.rs`, `doc/src/params.rs` |

**How a linked design behaves** (`component_link.rs`, `session_bodies`):

- The linked design is evaluated on its own. Its **solid** world bodies are exported as STEP and imported into one Import feature of one component. A board inserted this way is one rigid component in the housing; its inner occurrences are not kept.
- **Mesh bodies are not linked** ("mesh bodies of … are not linked").
- The imported faces carry the source design's own face names (`carried_face_names`, `Feature::link_faces`). A joint or sketch link on a linked face therefore follows the source's naming through `update_links`, and reports when the face is gone.
- `update_links` re-reads a design only when its bytes changed (an in-session digest that is not saved). It returns `updated`, `unchanged` and warnings, including joints whose faces are gone or no longer solve, and it can be undone. It has no dry run.
- `DesignLink` stores no digest or revision of the source.
- The assembly cannot set a linked design's parameters.
- A linked design cannot keep references to its host assembly's geometry; edit in place only draws the assembly around it.

## 3. Who solves what

There are two files and three solves. Every value has exactly one owner.

| Value | Owner and solver | Others see it as |
| --- | --- | --- |
| Board outline, cutouts, mounting holes | `.pcbcraft`: the board sketch solve (§5.1), whose inputs may include housing references | Housing: faces of the linked board (laminate side faces, hole walls) |
| Pose of a dimensioned part | `.pcbcraft`: the parts solve (§5.2) | Board design: planar joint expressions; housing: the part's linked faces |
| Pose of any other part | `.pcbcraft`: stored `Placement` | Same as above |
| Seating (side, z, tilt) | Side and stackup (§5.3); never solved | Joint offset expression |
| Housing geometry (walls, openings, bosses) | Housing file: Solidcraft's own evaluation, whose sketches may project linked board faces | Pcbcraft: housing references, read from the housing file |
| The board's place in the housing | Housing file: Solidcraft mates on the board component | Pcbcraft: the board component's world transform, read from the housing |

**A connector and a housing opening.** Both directions use real named faces:

- **The opening follows the connector.** In Solidcraft, the housing's opening sketch projects the connector's linked faces (`LinkSource::Face` with `face_name`) and is dimensioned to them on all four sides. The housing owns that dimension.
- **The connector follows the opening.** In Pcbcraft, a part dimension names the housing's opening face as a housing reference. The `.pcbcraft` owns that dimension.

A dimension lives in the file whose geometry it drives. The cycle check (§7.3) refuses one piece of geometry being driven from both files.

**One placement, seen by both.** `Placement` → the board design's joint expressions → the board's solid bodies → the housing's linked copy after Update Links. Solidcraft never solves a part pose. Pcbcraft never solves housing geometry or the board's place in the housing; it reads them.

## 4. Scope

### In

- Closed-loop outline; splines, ellipses and conics; the open-loop error outside Board shape mode.
- Dimensioned parts (placements, and drawn fills and keepouts as `EdgeConstraint` allows today) with features:
  - origin and axes;
  - pad centres and sides;
  - courtyard;
  - authored anchors;
  - 3D faces, edges and vertices of every 3D model kind except glTF meshes (§6.3).
- Housing references: faces, edges, axes and sketch curves of the housing, in the board's frame, driving the board sketch and dimensioned parts. Explicit parameter bindings.
- Stable face names in the board `.solidcraft` that Pcbcraft writes, so the housing can reference board and part faces.
- A two-way impact check, cycle detection, update order, and relink for a missing housing.
- `EdgeConstraint` replaced.

### Out

- Parts driving the board.
- Pcbcraft writing the housing file.
- Live cross-file links (founder decision).
- Part faces with no single trace in the board plane: sloped or free-form part faces. A housing wall with draft is read as its section at a board surface.
- Tracks, vias and copper as dimension participants.
- Panels.

## 5. Data model (Pcbcraft core)

The types are pure, serde and `JsonSchema`, with no Solidcraft dependency. The board sketch stays an opaque `SketchBlob`.

### 5.1 Outline as a closed loop

The `outline` curve list is dropped. After the board solve, the evaluator runs `find_drawn_profiles` and accepts the result when:

- exactly one region remains once construction curves are ignored;
- its outer loop does not cross itself;
- every hole lies strictly inside the outer loop, apart from the others.

The outer loop is the outline. Each hole is a cutout, unless a `CutoutRole` keyed by its curve ids makes it a slot. Otherwise the last valid `SolvedBoard` is kept, and the board reports one of:

- `board.shape_open`, with the open ends from `loose_ends`. It is advisory in Board shape mode and an error everywhere else.
- `board.shape_invalid`, naming the crossing or the touching hole.

Free-form curves reach `SolvedBoard` through `Sketch::segs` and `Loop2::fit_arcs`. The tolerance is a new design rule, `board.outline_fit` (profile, override, `rules.effective`). Its default is 0.01 mm, the chord tolerance `board.rs` uses today (`CHORD_MM`). Fills, checks and Gerber/DXF stay arc-based. `SolvedBoard` records the sketch curve behind each segment.

### 5.2 Dimensioned parts

```rust
pub struct PartDimension {
    pub id: Id,
    pub kind: PartRelation,          // DimKind or GeomConstraint
    pub refs: Vec<BoardRef>,         // 1–3
    pub value: Option<DimValue>,     // DimValue gains Expr(Expr) over board parameters
    #[serde(default)]
    pub reference: bool,             // measures only
}

pub enum BoardRef {
    Board(SketchRef),                // board-sketch point or curve, stable sketch id
    Part(FeatureRef),
    Housing(HousingRefId),           // §7.2
}

pub enum FeatureRef {
    Origin { target: ConstraintTarget },
    Pad { placement: Id, pad: PadId, at: PadPoint },
    Courtyard { placement: Id, side: Option<Side> },
    Anchor { placement: Id, name: AnchorName },
    Face { placement: Id, face: ModelFaceRef },
    Edge { placement: Id, edge: ModelEdgeRef },
    Vertex { placement: Id, edge: ModelEdgeRef, end: End },
}

/// A face of the package's own 3D model, in the model's frame (§6.2).
pub struct ModelFaceRef { pub body: BodyName, pub face: FaceName, pub pick: [f64; 3] }
```

`Project::part_dimensions` replaces `Project::edge_constraints`. Only parts named by a dimension are solved.

**The parts solve.** It runs after the board solve:

1. Group the dimensioned parts into connected components; two parts are connected when a dimension names both.
2. For each component the operation touched, build a scratch Solidcraft sketch:
   - the named board and housing geometry, held fixed;
   - per part, an origin and a construction axis;
   - the referenced features, bound to that frame with `PatternPoint` (y mirrored on the bottom side);
   - the dimensions.
3. Solve it. Each part's x, y and rotation are the solution; an unconstrained degree of freedom keeps its stored value.

Feature geometry is rebuilt from the library snapshot for each solve and never stored. A move or rotate of a dimensioned part is a solver drag, so a part with one dimension to an edge slides along it. A move that cannot land is refused in `outline::held_message`'s words.

### 5.3 Seating

The board sketch and the parts solve are 2D, so z does not exist in them. Side and stackup set the seating. It reaches the native design as the joint's offset expression (`board_thickness` / `0 mm`), and the pose as parameter expressions (§1). The design keeps it that way:

- no mates on parts;
- one planar joint per part;
- that joint's values stay expressions.

### 5.4 3D faces in the parts solve

| Feature of the model | In the board plane |
| --- | --- |
| Planar face square to the board | Its line |
| Cylinder or cone with its axis along the normal | Its centre (and radius) |
| Straight or circular edge parallel to the board | Its line or arc |
| Vertex | Its point |
| Face parallel to the board | Reference (height) dimensions only |
| Sloped or free-form face | Refused: "pick an edge of this face" |

The trace is exact, because the seating leaves only x, y and the rotation. It is taken with `naming::face_on_body` and `project::resolve_face_indices` on the evaluated model design (§6.2), in the model's frame.

### 5.5 Identity and missing references

| Reference | Identity | When its source changes |
| --- | --- | --- |
| Part | Placement instance `Id` | Deleted: `Lost` |
| Pad | `PadId` | Absent: `Lost` |
| Courtyard side | The box side in the part frame | Always resolves |
| Anchor | Authored name in the model design | Absent: `Lost` |
| Model face, edge, vertex | `ModelFaceRef` in an exact model revision | Re-found by name. Upstream `inexact`, a geometry check failing (§6.2), or an unverified name → `Inexact`; not found or no longer square → `Lost` |
| Housing reference | §7.2 | Same rules, against the housing file |
| Board curve or point | Stable sketch id | Deleted: `Lost` |

A lost or inexact reference keeps its last geometry, so nothing jumps. It is reported as `board.reference_lost` or `board.reference_inexact`, and blocks fab preflight with no waiver. The user re-picks the reference, or acknowledges an `Inexact` one in an impact preview, which makes it `Resolved`. Library-update and housing-update previews list these references before anything changes.

### 5.6 Commands, persistence, `EdgeConstraint`

- **Commands.** Each is one atomic undo step through the one engine.
  - `board.edit` gains `SetCutoutKind`, `AddSpline`, `AddEllipse` and `AddConic`.
  - New `layout.dimensions` (add, edit, delete) for part dimensions.
  - New `housing.*` commands (§7).
- **Persistence.** `Project::edge_constraints` and `BoardSketch::outline` are removed; `part_dimensions` and `housing` are added. The format has one version and no migration. `deny_unknown_fields` refuses old files; only the showcase used `edge_constraints`, and it is regenerated.
- **`EdgeConstraint` replaced.** Each `EdgeConstraint` becomes part dimensions:

  | `EdgeConstraint` | Part dimensions |
  | --- | --- |
  | `offset` | `PointLineDistance` (or `PointCircleDistance` for an arc edge) to the edge |
  | `along` | Distance to a construction line square to the edge at the anchor (or `ArcLength`) |
  | `anchor` | The edge's end, `Midpoint`, or `PointAlong` |
  | `align` | Origin, an explicit courtyard side, or the courtyard centre |
  | `follow_edge` | `Angle(axis, edge)` |

  A filleted edge measures between its virtual corners, as today. `layout::outline::follow` is deleted.
- **Copper and checks.** A part moved by its dimensions is an ordinary placement change for the copper graph, fills and checks. Dimensions never decide connectivity.
- **CLI and MCP.** Typed ids. The automation layer accepts `J1.pad(A6)`, `J1.face(Shell, "F3:side:l2")`, `edge(left)` and `housing.face(Case, "F12:side:l3")`, and resolves those labels when it receives the request; they are never stored.

## 6. Stable names on every model and in the board design

A housing can only keep a mate or a projection on a board face across updates if that face keeps its name in the board `.solidcraft`. Solidcraft names a face after the id of the feature that made it, so a name is stable exactly as long as that feature's id is.

### 6.1 The board design is persistent and edited in place

Today a structural edit (a part added, a track count changed) rebuilds the board design, and the rebuild hands out new feature ids (`native/replay.rs`). Once a housing links a board, Pcbcraft must keep its board design and edit it the way a designer would:

- **New objects** append features.
- **Deleted objects** delete their features. Solidcraft does not reuse or renumber feature ids, so the remaining names are unchanged.
- **Value changes** replay in place, as today.

Pcbcraft keeps the map from its own objects (instance ids, board sketch curve ids, hole ids) to the design's feature and sketch-curve ids in its namespaced metadata (`org.pcbcraft.electronics.*`, `project/identity.rs`). That is generic document metadata.

- **Laminate side faces.** These are named after the laminate sketch's curve ids (`F<extrude>:side:<curve>`). The laminate sketch is therefore edited curve by curve, following the board sketch's own curves: an edited curve keeps its laminate curve, a new one adds one, a deleted one deletes one. A board edge keeps its face name for as long as the board curve exists.
- **Parts.** A part's faces keep their names while its package revision is the same.
- **Revisions and rebuilds.** A new package revision re-inserts that part's features: new ids, new names. A forced full rebuild (another Solidcraft version, a damaged design) renames everything. Both appear in the board → housing preview as "names change: the housing's references must be picked again in Solidcraft" (§7.4).

### 6.2 Every model kind as a named design

| Model | How it gets stable names | Revision change |
| --- | --- | --- |
| `PartFile` | Its own history (as today) | Re-found by name in the new revision |
| `Generated` | Built as a standalone Solidcraft part design by its family builder, one per package revision and `family_rev`. The command sequence is deterministic, so feature ids and names are too. Cached and inserted like a part file | A builder change bumps `family_rev`: a library revision with an impact preview |
| `Envelope` | The same, as a one-feature box design | Re-found by name |
| `Library` STEP | Wrapped in a part design whose one Import feature holds the STEP bodies. For the same file bytes the import is deterministic | A new revision has new bytes. Names are re-found and then checked against the stored pick (the point must lie on the face, and the face type and normal must match). They are always reported `Inexact` until acknowledged (Solidcraft marks names of history-less bodies unverified) |
| `Library` glTF | **No faces.** Solidcraft names no faces on mesh bodies (`face_on_body` returns `None`), and a linked design leaves meshes out | Reported: such parts offer pads, courtyard and their body box only |

**What is missing upstream:**

- names that a user or program assigns to a face, independent of the history position;
- named faces on mesh bodies.

Pcbcraft handles the first with the persistent design (§6.1) and has no workaround for the second (decision 3).

## 7. The housing round trip

### 7.1 Linking

- **The housing's side.** The housing author inserts Pcbcraft's board design with `component.insert_design` and places it with ordinary mates (to bosses, walls or standoffs), like any sub-assembly. Pcbcraft saves the board design next to the project; the user can name its path.
- **The project's side.** The project stores a `HousingLink`:
  - `path`, relative to the project;
  - the housing component and occurrence that link Pcbcraft's board file (matched by `DesignLink.path` resolving to that file, never by name);
  - the references;
  - the bindings;
  - a snapshot: the last resolved geometry, the housing file's hash, and the board-design revision the housing was checked against.

  The project opens, solves and checks on the snapshot without the housing file.
- **The board's frame.** The board frame in the housing is the board component's world transform after the housing's own joint solve (`world_state`, `occurrence_world`). There is no special frame.

### 7.2 Housing references and bindings

A `HousingRef { id, body, face | edge | sketch curve (persistent name), pick, section }`:

- `section` is `Project` (along the board normal) or `Intersect` at the board's top or bottom surface, so a wall with draft gives its section where the board meets it.
- It is resolved with `naming` and `project` on the evaluated housing, in the board frame.
- It enters the board sketch as fixed geometry owned by the reference, so board curves (outline offset from a wall, a hole centred on a boss axis) are dimensioned to it. Part dimensions can name it too.
- It is refreshed only by a housing update.

A `HousingBinding { board_param, housing_param }` sets a board parameter from a housing user parameter (`Document::param_values`). Units are checked, and names are never matched automatically.

### 7.3 What is allowed: cycles

Each housing reference is measured in the board's frame, so it depends on how the housing places the board. That placement comes from mates on board faces. A cycle exists when a board face that places the board, or that housing geometry projects, depends in turn on housing geometry that depends on it. Pcbcraft builds the dependency graph over both files:

| Edge | Source of the edge |
| --- | --- |
| Board object → housing reference it is dimensioned to | Pcbcraft: the board sketch (`determined_by` on the solved sketch) and part dimensions |
| Housing reference → the board's placement in the housing | Always (a reference is read in the board frame) |
| Board placement → each board face a mate on the board component uses | The housing's `Assembly.joints` whose origin is on the board occurrence (`Snap::face_names`) |
| Housing face → board faces its feature's history projects | The housing's sketch `links` on linked board bodies, for the feature that made the face and the features before it in its component (conservative) |
| Board face → board object it belongs to | Pcbcraft's metadata map (§6.1) |

A cycle is reported as `housing.cycle`, naming the path in both files' words: "hole H2 follows boss 'Boss 3'; the housing places the board by hole H2". The housing update or the publish is refused until the cycle is broken. Nothing iterates, so nothing hangs. Detection is a strongly connected components search, linear in the edges.

**Allowed patterns:**

- The outline follows walls while the board is placed by its mounting holes, with the bosses following those holes.
- Holes follow bosses while the board is placed by a connector face, or by edges that do not follow the housing.
- Openings follow connectors.

**Refused:** a hole that follows a boss while the board is placed by that same hole.

### 7.4 Two-way impact check and update order

**Board → housing (`housing.publish_preview`, then `housing.publish`).** Pcbcraft writes the board design the housing links only through this step; an edit itself is never blocked. The preview reads the housing and lists every housing object that consumes board faces:

- mates on the board component;
- sketches projecting board faces.

For each consumed face it compares the face in the board design about to be written with the board design the housing was last checked against: unchanged, moved (by how much), renamed or gone (§6.1), or now in a cycle. Rows name the housing's own objects ("joint 'USB stop', sketch 'Front opening'"). Publishing writes the board design and records it as the board revision offered to the housing.

**In Solidcraft (manual).** The housing author runs Update Links. Solidcraft shows its own generic result:

- updated and unchanged links;
- warnings for joints whose faces are gone or no longer solve;
- lost sketch links marked as such.

It can be undone. Solidcraft has no dry run of Update Links, so the Pcbcraft preview is the pre-check. This is the one step in the round trip that Pcbcraft cannot do.

**Housing → board (`housing.preview`, then `housing.update`).** The desktop watches the housing file and its hash, and shows `housing.stale`. The preview re-evaluates the housing, re-resolves the references and bindings, and solves the board and the parts on a candidate. It lists:

- the outline, holes, fills and dimensioned parts that would move;
- references that would become `Lost` or `Inexact`;
- cycles;
- **whether the housing still holds an older board.** The faces the housing consumes, as they are in its linked copy, are compared with the last published board. `DesignLink` stores no source revision, so this comparison is the only way to tell, and Pcbcraft says "run Update Links in Solidcraft first".

The update is applied as one undo step. It is refused atomically if the board would not solve or close, and the preview names why.

**Update order.** Pcbcraft publish → Update Links in Solidcraft (manual) → housing edits and save → Pcbcraft housing preview and update.

- Each step crosses the file boundary once, in one direction. Pcbcraft never loops automatically.
- In an acyclic graph a change settles after as many rounds as the longest chain that alternates between the files. One preview says when another round is needed: "publishing this moves hole H2, which the housing's boss follows".

**What each side shows:**

- **Pcbcraft.** A housing status in the Board card and Issues: in sync, housing changed, housing holds an older board, cycle, or missing (with Relink). The two previews. Reference status on each dimension.
- **Solidcraft.** Only its generic link, joint and sketch-link reports; nothing Pcbcraft-specific.

### 7.5 Missing or moved files

- **A missing or moved housing.** The project opens on the snapshot and reports `housing.missing`, with Relink (`housing.relink { path }`, previewed like an update).
- **A moved board design.** The housing's own `component.relink` handles it on the Solidcraft side.

### 7.6 No fork

Everything Pcbcraft does here uses public upstream APIs: `io::read_design`, `Session::opened_validating`, `world_state`, `joints`, `naming`, `project`, `Sketch.links` and `DesignLink`. Plus its own board design, written through Solidcraft commands. Solidcraft gets nothing Pcbcraft-specific.

## 8. Upstream gaps and how the round trip copes

| Gap (upstream as found) | Effect | Handling |
| --- | --- | --- |
| No live cross-file links; the assembly cannot drive a linked design | Housing → board needs Pcbcraft to read the housing | §7 (founder: no upstream proposal now) |
| `update_links` is manual and has no dry run | The Solidcraft side cannot be previewed in Solidcraft | Pcbcraft's publish preview; Solidcraft's undo |
| `DesignLink` stores no source digest or revision | Pcbcraft cannot read which board the housing holds | Comparing the consumed faces (§7.4) |
| Face names follow history position; no assigned names | Rebuilds rename faces | Persistent board design (§6.1) |
| No faces on mesh bodies; meshes are not linked | glTF parts cannot be referenced or seen by the housing | Reported; decision 3 |
| A linked design is flattened into one component | The housing sees the board as one rigid body, not a sub-assembly of parts | Enough for mates and projections, because part poses are Pcbcraft's |
| `face_on_body`'s `inexact` only upstream | — | Adopted when Pcbcraft next repins |

Rejected alternative, kept from the earlier draft: Solidcraft's assembly solver deciding part poses. It would need the evaluated board design inside every core operation, cannot drive a sketch, and is bounded per loop group (`MAX_LOOP_VALUES` = 64, `MAX_LOOPS` = 100, `doc/src/joints.rs`). The founder's requirement that Solidcraft can dimension to Pcbcraft's parts is met through linked faces instead (§3).

## 9. Performance

**Measured.** A scratch benchmark, not committed: the Solidcraft sketch crate at our pin, release build, Apple M5 Max, with load average 30–49 from other agents. Each part had an origin, an axis and two `PatternPoint` features; all parts were solved together with the board free, which is the worst case.

| Dimensioned parts | Solve | Board width edit |
| --- | --- | --- |
| 10 | 0.2–0.5 ms | 0.1 ms |
| 50 | 1.6–2.0 ms | 0.6 ms |
| 100 | 5.2–5.8 ms | 2.0–2.4 ms |
| 500 | refused: 4,006 unknowns > `MAX_COMPONENT_VARS` 4,000 | |

With the board held, 500 parts solved in 7.2–9.3 ms. A real board has tens of dimensioned parts, and each edit solves only the components it touched.

**Not measured:**

- evaluating a housing design;
- the cycle-graph build;
- Solidcraft's Update Links on a full board, which exports every solid body as STEP (`session_bodies`).

**Targets** (labelled as targets, measured in S11):

- any edit re-solves within 16 ms with 100 dimensioned parts;
- a housing preview finishes within 1 s;
- publishing the board design finishes within the time a full native build takes today.

**Selection and camera rebuild nothing.** The solves run only when their inputs change. Hover, pick and the camera read the stored results.

## 10. Slices (agent hours)

C0 comes first; dependencies are listed per slice.

| Slice | Content | Needs | Estimate |
| --- | --- | --- | --- |
| C0 | Contract: types, commands, findings, schemas, test skeletons | — | 3 |
| S1 | Closed-loop outline, free-form curves, `board.outline_fit` rule, open-loop error | C0 | 9 |
| S2 | Dimensioned parts: `PartDimension`, the parts solve, pad/courtyard/anchor features, drag-to-move, `EdgeConstraint` replaced | C0 | 12 |
| S3 | Part-file face, edge and vertex references; reference status; library impact rows | S2 | 6 |
| S4 | Named designs for generated, envelope and STEP models; revision checks; the glTF finding | S3 | 10 |
| S5 | PCB 3D picking and dimension display | S3 | 6 |
| S6 | Persistent board design: structural edits in place, laminate curve map, identity metadata, publish | S1 | 14 |
| S7 | Housing link: reading the housing, references, bindings, snapshot, preview and update, relink, missing | S1, S2 | 10 |
| S8 | Two-way check: consumer detection, face diff against the published revision, the older-board check, the cycle graph | S6, S7 | 8 |
| S9 | Housing UI: status, both previews, picking housing faces in a 3D context view | S7, S8 | 6 |
| S10 | VM run: a real enclosure in Solidcraft, the full round trip including Update Links | S9 | 4 |
| S11 | Measurements against §9 | S8 | 3 |
| | **Total** | | **91** |

S1 and S2 can run in parallel after C0, then S3, S6 and S7.

## 11. Decisions

The founder accepted all three recommendations (2026-11-09):

1. **The published board design is the full board.** Update Links on it is measured in S11; a mechanical subset is considered only if that is too slow.
2. **Stable names stay on Pcbcraft's side** through the persistent board design (§6.1). Proposing generic assigned face names upstream may be considered later, separately.
3. **glTF (mesh) models stay the one kind without face references,** with a clear finding that steers authors to STEP or part files.

## 12. C0 handoff

C0 is the contract for S1 and S2. Every path that needs either slice refuses with a typed `Error::Unsupported` ("… not yet supported"); nothing panics.

### What C0 put in place

- **Types.** `model::dimension` (new): `PartDimension`, `PartDimensionSpec`, `PartRelation`, `BoardRef`, `FeatureRef`, `PadPoint`, `BoxSide`, `PartAxis`, `End`, `ModelFaceRef`, `ModelEdgeRef`, the name newtypes, `HousingRefId` and `ReferenceStatus`. `Project::part_dimensions` sits beside `edge_constraints`. `DimValue::Expr(Expr)` is added; footprint and board-sketch dimensions refuse it.
- **Commands.** `board.edit` has `SetCutoutKind`, `AddSpline`, `AddEllipse` and `AddConic`; the free-form ones reach `BoardShapeEvaluator::add_curve` with a `FreeFormCurve`. The new `Operation::Dimension` family has `layout.dimension_add`, `layout.dimension_edit` and `layout.dimension_delete`, with `Outcome::DimensionAdded`. There is no new query: `board.shape` and `project.issues` cover S1 and S2.
- **Solve seam.** `BoardShapeEvaluator::solve_parts(&PartsRequest) -> PartsSolution` (`layout::dimensions`). The engine calls `layout::dimensions::follow` right after `layout::outline::follow`. `dimensions::validate` and `dimensions::checks` are wired into `layout::validate` and `layout::checks`.
- **Findings.** `board.shape_open` and `board.shape_invalid` (`layout::outline`), and `board.reference_lost` and `board.reference_inexact` (`layout::dimensions`). All four are classified as errors with group titles and fixes, and pre-flight blocks on them. `fabrication::preflight::NO_WAIVER` already keeps the two reference codes from being waived; `fabrication.waive` refuses them.
- **Edge constraints moved, unchanged.** All edge-constraint code is in `layout/outline/edge_constraints.rs`, re-exported from `layout::outline` under the same names. The integration crate's evaluation of edge-constraint expressions is in `board/parts.rs` (`edge_constraint_values`).
- **Test skeletons.** `#[ignore = "slice S1"]` / `#[ignore = "slice S2"]` tests state the expected behaviour. Each slice removes its ignores and must not weaken the expected values.

### Doc adjustments made in C0

1. `PartRelation` also has `PointAlong { at }`, because Pcbcraft's `GeomConstraint` has no `PointAlong` and it carries a fraction. `FeatureRef` also has `Axis { target, axis }`, for "origin and axes" (§4) and `Angle(axis, edge)` (§5.6).
2. Solidcraft at our pin has no arc-length constraint, so §5.6's "(or `ArcLength`)" is not available. S2 maps `along` on an arc edge to `PointAlong` or to an angle about the arc centre.
3. The dimension commands are their own operation family, because the parts solve needs the board evaluator, and `LayoutCommand` is applied without it. The wire names keep the `layout.` prefix of §5.6.
4. `board.shape_open` and `board.shape_invalid` start with `FATAL_PREFIX` (`board.shape_`), but per §5.1 they are reported, not refused. S1 exempts them from the refusal. The stored "why the curves are not a board" state (open ends, crossing) is S1's to add to `BoardSketch`.
5. The rule's key on the wire is `{"rule":"board_outline_fit"}`, because `RuleKey` names are snake_case identifiers. `board.outline_fit` stays its name in prose.
6. Existing tests that contradict the approved design are replaced, not weakened: S1 changes the open-outline cases of `bad_edits_are_refused_and_leave_the_board` (§5.1), and S2 replaces the edge-constraint tests (§5.6).

### File ownership

| Files | S1 | S2 |
| --- | --- | --- |
| `editor-core/src/layout/outline.rs` | Owns | May only delete the three constraint variants of `BoardCommand` with their `declare_classes!` rows and match arms, the `edge_constraints` module line, re-exports and calls, and `BoardEvaluation::constraints`; may change `evaluate`'s `constraints` parameter. |
| `editor-core/src/layout/outline/shape_tests.rs` (new) | Owns | — |
| `editor-core/src/layout/outline/edge_constraints.rs`, `outline/tests.rs` | — | Owns (deletes or ports) |
| `editor-core/src/model/board.rs` | Owns | May only delete `EdgeConstraint`, `EdgeConstraintSpec`, `EdgeAnchor`, `ConstraintAlign` and `MAX_EDGE_CONSTRAINTS`. `ConstraintTarget` stays: `model::dimension` uses it. |
| `editor-core/src/rules.rs`, `rules/tests.rs`, app `pcb_editor/rules_panel.rs` | Owns (the `board_outline_fit` rule) | — |
| `editor-core/src/model/dimension.rs`, `layout/dimensions.rs`, `layout/dimensions/` | — | Owns |
| `solidcraft-integration/src/board.rs` | Owns | May only change the `constraints` parameter of `evaluate` and `run` with their call sites, and the `parts::edge_constraint_values` line. |
| `solidcraft-integration/src/board/closed_loop_tests.rs` | Owns | — |
| `solidcraft-integration/src/board/parts.rs`, `board/parts/tests.rs` | — | Owns |
| `solidcraft-integration/src/board/tests.rs` (shared helpers are `pub(super)`) | Owns the shape tests | Owns the edge-constraint tests and the showcase test's constraint lines |
| `editor-core/src/layout/showcase.rs` | `board_edits` | `j1_constraint` |
| `editor-core/src/model.rs`, `validate.rs`, `lib.rs`, `commands/mod.rs`, `layout/fill.rs`, `layout.rs` (`held`), `tests/board_contract.rs` | — | Only to remove `edge_constraints`, `EdgeConstraintAdded`, `board.constrain` and `held` |
| App `pcb_editor/board_shape.rs`, `board_shape_ui.rs`, `board_shape_tests.rs` | Owns | May only change the lines that read `edge_constraints` |
| App `pcb_editor/edge_card.rs`, `hold_mark.rs`, `hold_tests.rs`, `pcb_editor.rs` (constraint painting) | — | Owns |
| `automation` label resolution (`J1.pad(A6)`, `edge(left)`; a new module) | — | Owns |
| `issues/rules.rs`, `fabrication/preflight.rs` | Done in C0; touch only for a new code | Same |
| `automation/src/mcp/surface/golden/core.json` | Regenerate with `PCBCRAFT_BLESS=1` after rebasing; never hand-merge | Same |
| This document | Its own slice notes | Same |

Whichever slice lands second rebases onto the first. The other slice's ignored tests stay ignored until that slice lands.

### S1 notes

- **The outline.** `BoardSketch::outline` and `BoardEdit::{SetOutline, AddCutout, RemoveCutout}` are gone. The evaluator takes the outline and the holes from `find_drawn_profiles`; a profile must have whole curve boundaries (`Profile::whole_curve_boundaries`), so crossings and holes that touch the outline or each other are `board.shape_invalid` and are named by where the search cut the curves. Roles exist only for slots (`SetCutoutKind`, keyed by the whole curve set). A slot role that no longer matches a hole is `board.shape_invalid`.
- **Stored state.** `BoardSketch` gains `fault: Option<ShapeFault>` (kind, message, points), which drives the findings and the canvas markers, and `outline_fit`: the tolerance the stored solve was fitted at. `layout::outline::validate` requires it to equal the effective rule. A rules change re-solves the board in the same operation (`layout::outline::refit`, called from the engine's rules arm). The trait's `evaluate` signature is unchanged, so S2's call sites are untouched.
- **Solved board.** `SolvedBoard::outer_curves` and `SolvedCutout::curves` record the sketch curve behind each segment. `SolvedCutout::id` is the slot role's id, or `None` for a plain cutout.
- **Free-form curves.** The board sketch admits `Ellipse`, `Conic` and `Spline` without explicit knots (cubic, 2–500 points; `check_board_sketch`). Footprints still refuse them. Each `exact_pieces` span is fitted with `Seg2::fit_arcs`. A loop that would need more than `MAX_OUTLINE_SEGMENTS` (512) spans is refused and names the rule. The rule's range is 0.001–1 mm.
- **Superseded tests.** In `board/tests.rs`, the circle outside the board in `cutouts_slots_and_mounting_holes_solve_and_build_in_3d` is now reported as a second region rather than refused (§5.1).
- **UI.** The sketch view draws free-form curves (`ViewGeometry::FreeForm`). Board shape mode marks the open ends or crossings and shows the fault in the card as advisory. The selection offers Make slot / Make cutout for a hole. There are no spline, ellipse or conic drawing tools yet; the commands are reachable through CLI and MCP.
