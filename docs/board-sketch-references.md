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
3. Solve it. Each part's x, y and rotation are the solution; an unconstrained degree of freedom keeps its stored value. A part's rotation is solved only when a relation that sets a direction names it (an angle, parallel, perpendicular, horizontal, vertical or tangent); a distance never turns a part (§17).

Feature geometry is rebuilt from the library snapshot for each solve and never stored. A move or rotate of a dimensioned part is a solver drag, so a part with one dimension to an edge slides along it. A move that cannot land is refused in `outline::held_message`'s words.

A part the solve moves or turns, or a `layout.move`, carries the copper on its pads in the same operation and undo step (`dimensions::carry`): what is attached is read off the copper graph of the document before the operation, island by island (a net's tracks and vias joined other than through a pad).

- An island whose pads all move with one motion moves rigidly with them, so the routes inside a group of parts held together keep their shape.
- An island between moved and unmoved pads, when the parts slid without turning, keeps every segment's direction, as a dragged track does: from each moved pad, a segment running along the move stretches; else, at a corner where one other segment meets it end to end, it slides and that neighbour stretches along its own line to meet it; else it slides whole with the copper at its far end. Both halves of a differential pair, parallel where they leave the part, change alike, so the pair stays coupled and its skew is kept.
- Otherwise (a turn, parts moving differently, locked copper, or a slide that would reach an unmoved pad or flip a segment), a via joined only to the part's pads moves rigidly with it, and a track end on a carried pad or via moves rigidly too, so its segment stretches and the corners beyond it stay; when stretching would take the segment off copper that meets it in mid-span, the segment stays and a new segment like it joins its old end to the carried one.

Locked copper stays. Nothing is deleted and no edit is refused for it: copper that lands too close to another net is a normal clearance finding, and fills refill as before.

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
- **UI.** The sketch view draws free-form curves (`ViewGeometry::FreeForm`). Board shape mode marks the open ends or crossings and shows the fault in the card as advisory. The selection offers Make slot / Make cutout for a hole. Board shape mode draws them with Spline (S), Ellipse (Shift+C) and Conic (Shift+A), one `board.edit` per gesture ([board shape](board-rules-and-shape.md#board-shape-cutouts-and-edge-relative-placement)).

## 13. S2 notes

S2 built dimensioned parts and removed `EdgeConstraint`, its `follow` pass and `board.constrain`, `board.constraint_edit` and `board.constraint_delete`. The shipped showcase holds J1 with two part dimensions: 6 mm from the left edge, and `height / 2 + 1 mm` from the bottom edge.

### How the solve keeps free values

`board/parts.rs` copies the solved board sketch into a scratch sketch with every point fixed and its constraints dropped. Each target gets an origin, a 10 mm construction axis, and its features bound with `PatternPoint`. Solidcraft's solve moves every point as little as it can, but that alone does not keep a free rotation exact. So the solve runs in two passes:

1. Solve the dimensions with a freedom analysis. Three helper points per target copy its x, its y and its axis direction, so `point_determined` tells which of x, y and rotation the dimensions fix.
2. Pin each free one with `VerticalPoints`, `HorizontalPoints` or `LineAngle`, at its stored value or at the value a move gave a dragged target, and solve again. Pinned values are written back exactly.

If coupled pins disagree (two parts held only to each other, one of them dragged), the pins are placed in halves, re-reading the freedom after each solve. A move "cannot land" when the dragged target's position (or rotation) does not change at all; it is refused in `dimensions::held_message`'s words.

### Deviations from the contract

1. `PartsRequest::drag: Option<PartDrag>` became `drags: &[PartDrag]`, because one operation can move several targets of one component. `PartsSolution` gained `freedom` (per target: x, y, rotation free) and `failing` (the dimensions a failed solve names).
2. The core reads the entity ids, and nothing else, from the board sketch's canonical JSON (`dimensions::board_ids`). Without them `checks` could not report a deleted board point or curve as `board.reference_lost` without the evaluator. On a rectangle board, which holds no sketch, a dimension to an unknown id is refused when the parts are solved.
3. `along` on an arc edge (§12 note 2) is a construction line in the board sketch from the arc's centre: `PointOnCurve` on that line plus a `Distance` to the arc (`PointCircleDistance`, inward when the part starts inside), with `Angle(line, axis)` for the turn (`a_part_is_held_to_an_arc_edge`). A distance between two lines (a pad side to an edge) measures from the first line's middle, square to the second, as the views show it (§17).
4. Fills and keepouts only translate. A dimension on a board-following fill is refused (`fill.follows_board`).
5. Labels for agents (`J1.pad(A6)`, `edge(left)`) are not built. Agents name curve ids from `board.shape` and pad ids from the library.

### Measured (S2), against the §9 target

`parts_solve_timing` (ignored; run with `--release -- --ignored --nocapture`) solves 100 dimensioned parts in one request: 50 held to two edges, 50 held to one of those by two distances, each with an origin and a line feature. Measured on an Apple M5 Max, release build, with a load average of 8–14 from other agents, median of 15 runs:

| Case | Median | Max | Target |
| --- | --- | --- | --- |
| Solve at the stored poses | 8.1 ms | 9.4 ms | 16 ms per edit |
| After the board grew 20 mm | 9.0 ms | 10.3 ms | 16 ms per edit |

This measures the solve alone, as one request. `follow` splits the parts into connected components and solves only the ones an edit touched, so a real edit solves less. The core's grouping and feature resolution are not timed; S11 measures the whole edit.

### For S1, S3 and later slices

- **S1 (landed first).** S2 dropped the `constraints` parameter of `BoardShapeEvaluator::evaluate` and `BoardEvaluation::constraints`. The parts solve reads the board through `check_board_sketch`, so free-form board curves can be dimensioned to. The app's `board_shape_tests.rs` lost its two edge-constraint tests.
- **S3.** `features::geometry` refuses `Face`, `Edge` and `Vertex` with `Error::Unsupported`, and `reference_status` reports them as `Lost` until S3 resolves them. Anchors already come from `surface::package_anchors`. `reference_status` is the place to add `Inexact`.

## 14. S3 notes

S3 built dimensions to the faces, edges and vertices of a part's part file (`PackageModel::PartFile`), reference status, and the library impact rows.

### How a model reference resolves

- **Behind the evaluator seam.** The core never reads a model. `BoardShapeEvaluator::trace_model` (`layout::dimensions::model::TraceRequest` → `Traced`) is implemented in `pcbcraft-solidcraft` `board/model_refs.rs`. It evaluates the part through the one part-file cache the 3D build uses (`part_file::evaluate`, which now also keeps the evaluated `ModelState`), so there is no second model load or cache.
- **Lookup.** The face is re-found by body and persistent name with `naming::locate_face_on_body`, an edge with `naming::locate_edge`, both on the part file's own design, then put in the part frame by the package's model placement (`ModelPlacement::transform`).
- **Trace** (§5.4, all in the part frame):

  | Model feature | Trace |
  | --- | --- |
  | Planar face square to the board | Its line, counter-clockwise round the part (outward normal on the right), over the face's extent |
  | Cylinder or cone with a vertical axis | Its axis point (the task asked for points; §5.4's "and radius" is not kept) |
  | Edge parallel to the board | Its line, arc or circle (`kernel::edge_circle`) |
  | Vertex (`End` of an edge) | Its point |
  | Face parallel to the board, sloped or free-form face, edge not parallel | Refused: `layout.dimension_sloped`, "pick an edge (or vertex) of it" |

- **Status.** At our pin the lookup's `FaceHit`/`EdgeHit` carry `verified` (the name follows exact history) and `on_point` (the stored pick still lies on it). `Resolved` needs both; anything less is `Inexact`, with a note that gives a point on the face to pick again. Upstream's `face_on_body` `inexact` flag is not at our pin. Not found, or no longer square, is `Lost`.
- **Discovery.** A face name the part does not have is refused with up to 12 faces square to the board, each by name with a point on it in the model frame, so an agent can pick from the refusal.

### Traces are kept in the document

`Project::model_traces` holds one `ModelTrace` per model reference: the feature, the package revision it was traced in, status, geometry and note. This is a deviation from "derived, never stored". Checks, preflight, `project.issues` and the update preview run without an evaluator, so they must read the status from the document, like the solved board. A trace is host output, never user input:

- **When it is written.** `model::refresh` runs in `dimensions::apply` and at the start of `dimensions::follow`. It keeps a trace while its package revision is unchanged, re-traces it otherwise, and drops traces no dimension names.
- **Refusals.** A new reference (in an add or edit) that does not resolve exactly is refused with the host's reason. An existing one that stops resolving becomes `Lost` or `Inexact`, keeps its last geometry, stops driving, and is reported (`board.reference_lost` / `board.reference_inexact`, no waiver).
- **Re-picking.** A new pick is a new reference, so `layout.dimension_edit` with the point the note gives makes an inexact reference `Resolved`. There is no separate acknowledge command.

### Library updates

`updates::preview` and `updates::candidate` take the board evaluator. They run `dimensions::follow` on the staged project as the engine does after the update, so the dimensioned parts move in the preview too. `model::impact` rows (`ChangeObject::Dimension`) for references that become lost or inexact, or that move, are inserted first in `items` and `changes`, past the detail limit. `Engine::preview_update_hosted` passes the evaluators; the session and the desktop's update review use it. `Engine::preview_update` passes none, and a preview that needs a solve is then blocked, as the apply would be.

### CLI and MCP

References use typed ids (`{"part": {"feature": "face", "placement": <id>, "face": {"body", "face", "pick"}}}`). The readable labels (`J1.face(…)`, `edge(left)`) are still not built.

### Example

The ESP32-S31 dev board's J1 uses an original USB4105 part file (`examples/esp32-s31-devboard/parts/usb4105.solidcraft`, authored by `author_parts`). It is held by two part dimensions:

- its shell's front face at distance 0 from the bottom edge;
- its origin at `width / 2` from the left edge.

`the_usb_c_receptacle_follows_the_board_width` widens the board from 40 mm to 50 mm and J1 moves from x = 20 to x = 25 with its face still on the edge.

### For S4 and S5

- **S4.** `layout::dimensions::model::source` refuses every model kind but `PartFile` with `layout.dimension_model`. S4 adds the named designs of generated, envelope and STEP models behind the same `trace_model`. STEP names are unverified, so their references come back `Inexact` until re-picked. If acknowledging without a new pick is wanted, it needs a stored flag on the trace or the dimension. glTF stays refused.
- **S5.** The trace geometry for display and picking is `Project::model_traces[i].geometry`, in the part frame (transform it like any feature, `layout::placement_transform`). A pick in the 3D view must give the body, the persistent face name (`naming::face_name_at_body` on the part's evaluated state) and the pick point in the model frame, which is the board pick through the inverse of the placement, datum and model placement. Selection and camera must not call `trace_model`; only `refresh` does.

## 15. S5 notes

S5 built picking in PCB 3D and the dimension display there.

### The Dimension tool

- **Arming.** 3D's ribbon has Dimension (`pcb.dimension`, Shift+D, PCB › 3D). It is a tool card in the subject slot. Esc (`pcb.dimension-cancel`) drops the picks, and a second Esc (or Done) ends the tool. Switching to Layout ends it.
- **Hover.** Hover and click ask the same question (`target_at`), so what lights is what a click picks. Edges come first, found as Solidcraft's view pick finds them (`Viewport::edges_near`): any pickable edge of a part body or of a board-level body within 6 points of the pointer, nearest first, at its point under the pointer ray, where no face in front hides it. So an edge is as easy to hit at overview zoom as close up, also from just off the body. A board-level edge counts where it runs along the outline. Otherwise the face under the pointer is highlighted the way Solidcraft pre-highlights: lifted, and outlined in `hover_edge`; the board's top or side within 8 points of the outline is that board edge. The highlight is a block in the viewport's selection layer (`viewport/surface.rs`), built from the cached mesh only when what is hovered changes. Hover raycasts the shown meshes; it never calls the evaluator or requests a model. A click on nothing says so in the tool card.
- **Picks.**
  - A part body: the board point goes back into the part frame (`model::part_point`: the seat over the side's copper, then the placement's position, turn and mirror). The new evaluator seam `BoardShapeEvaluator::pick_model` (`model_refs::pick`) names the face with `naming::face_name_at_body`, or the face's edge within reach, picked at its nearest point. `model::pick` then traces the feature once. A sloped face, or one parallel to the board, is refused in the tool card in the trace's words, before anything is committed.
  - A pad: `FeatureRef::Pad` at its centre.
  - The laminate's side, or a mask or copper film at the edge: the board curve there (`dimensions::view::board_curve_at`). For a plain rectangle, the named edges are read through the evaluator's rectangle sketch.
  - Two board edges are refused: a dimension holds a part.
- **Words.** The card names a pick by where it lies on the part, in the part frame against the package envelope (`dimensions::model::feature_term`): "J1 · front face", "J1 · top-front edge", "R5 · left end face", "Board · bottom edge". The Placement card's dimension rows and the reference findings use the same words. The persistent face or edge name stays in the stored reference only.
- **Value.** Two picks open a text field at the draft's label. It holds the distance measured now (`dimensions::view::shown`). It takes a number, a board parameter or an expression; the draft's label follows what is typed. Enter commits one `layout.dimension_add` (refs in pick order, `Distance`), which is one undo step. The new dimension is then selected. An engine refusal comes back to the card's refusal row.

### Display and selection

- The dimensions of the selected part, and the selected dimension, are drawn in the plane the part sits on. They use Solidcraft's `dim_layout_reaching`, `default_text` and `dim_view::label` ("fx:" for expressions), its `dimension`/`accent` colours and its text box. The arrowhead is reproduced, because upstream keeps it private.
- Geometry comes from the document only (`dimensions::view::reference_geometry`): the stored model traces and the solved board. A lost reference is drawn dashed in the error colour and an inexact one in the warning colour, each labelled "· lost" or "· inexact". On hover, the label shows the reference findings of `dimensions::checks`.
- **Shared selection.** `PcbEditor::dimension` is the one selection. A click on a label selects the dimension in 3D, and in Layout too, where the labels are now hit-tested. Focusing a value in the Placement card's Dimensions section also selects it. The part is selected with the dimension. Layout draws the selected dimension in the accent colour; the card puts its row on `selected_bg`.

### Tests

- `model_refs_tests::a_pick_names_the_face_or_the_edge_under_it` (host): face, edge within reach, a turned model placement, and off the model.
- `app::dimension_3d_tests::a_3d_pick_dimensions_j1_to_the_bottom_edge_in_one_undo_step` runs the whole app frame on the ESP32 example:
  - It removes J1's face dimension, frames J1 in 3D and arms the tool.
  - It hovers the shell's front rim and the laminate side. No model is requested (`SceneController::requests`), and nothing is traced or picked (a counting evaluator).
  - It clicks both, checks the value opens at `0 mm`, types `1 mm` and presses Enter.
  - J1 moves 1 mm off the edge, and the dimension is added and selected. One undo restores J1.
- `dimensions::view` unit tests cover the shown distance to a line and to an arc.

### For later slices

- Only `Distance` is made. Angles and the 3D display of non-distance relations are not drawn.
- Picks reach every model kind but glTF since S4 (§16). Housing faces are S9.
- A dimension's text cannot be dragged, and the 3D label cannot be edited in place. Values are edited in the Placement card.

## 16. S4 notes

S4 made faces, edges and vertices of every model kind but glTF dimension references: generated models, envelopes and STEP models, besides part files.

### One design per model

`layout::dimensions::model::TraceRequest` now carries a `TraceModel`: `PartFile`, `Package` (the generated model, or the envelope when the package has none, decided by the host as the board build decides it) or `Step`. The host traces each on the design the 3D build uses (`board::model_refs::ModelDesign`), from `pcbcraft-solidcraft` `models::named`:

| Model | Design | Shared with the 3D build |
| --- | --- | --- |
| Generated | The family builder's commands in the root of a new design (`models::build_standalone`): parameters and bodies by their own names (`length`, `body`, `termination 2`) | The board inserts this design (`part_file::insert_design`, the part-file insert generalized), so the board's bodies and parameters keep their names (`pkg3r1 body`, `pkg3r1_length`) and the model is built once per process |
| Envelope | One box feature, body `envelope`, from the package envelope | Not inserted: the board keeps its per-placement box, whose size is a placement parameter a value edit replays in place. The box is the same |
| STEP | One import feature with the file's bodies named by their keys in the file, then the frame's scale, turn and offset (`imported::add_import`, shared with the board build) | The file's parse: any `SourceCache` or named design holding it shares it (`imported::checked_source`); a file is parsed once while anything holds it |

Designs are cached process-wide by what they are built from (form, builder revision, materials and terminals; envelope size; file hash and frame), 256 at most, least recently used last. The design keeps the evaluated `ModelState` a trace and a pick read.

**Determinism.** `models::named` tests build the designs twice in one process and once in a fresh process (the test binary runs itself) and compare every body and face name; a golden list of the 0603 resistor's face names fails on any change in the builder or in Solidcraft's naming, which needs a new `family_rev`.

### When a name is exact

The rule of §5.5, made precise:

- **The same design.** A name in the design the reference was picked on is the name picked, whether Solidcraft verified it or not. The core stores that design's key on the trace (`ModelTrace::design`, `model::design_key`: the part file's bytes and values; the generated form, model and envelope; the STEP file's bytes and frame; not the model placement, which moves a trace and never a name), and passes `picked_here` when the package's design still has that key. A new pick is always `picked_here`. So a library revision that keeps the model keeps its references `Resolved`, for STEP too.
- **Another design.** A name that follows history (`verified`) is exact. A STEP import numbers the faces of each body in file order (`F1:face:4`), which Solidcraft reports as verified but which only holds for those bytes, so the host never trusts a STEP name in another file: it is `Inexact`, with a note giving a point to pick it again at. Generated faces Solidcraft names from geometry behave the same way.
- **The pick.** In either case the stored pick must still lie on the face or edge, or the reference is `Inexact`.

A trace that is not `Resolved` keeps the key of the design it was last picked on, so returning to that design makes it exact again and nothing else does.

### Acknowledging: a new pick, no flag

No stored acknowledgement flag was added. Picking the face again is a new reference (§14), and every note of an inexact reference offers a point on the face at least 1 µm from the stored pick (`REPICK_MM`), so `layout.dimension_edit` with that point always makes a new, `Resolved` reference. An edit that only changes the value keeps the reference and its status, so it never acknowledges silently.

### glTF

`model::source` refuses a glTF model with `layout.dimension_model`: "its 3D model is a glTF mesh, which has no faces to name; give the package a STEP model or a part file to dimension to its faces (its pads, courtyard and anchors can be dimensioned to as they are)". A reference whose package moves to a glTF model in an update becomes `Lost` with that note. A `.solidcraft` library model, which is not built, is refused the same way and pointed at part files.

### Tests

- `board/model_refs_kinds_tests.rs`: R1 of the demo, a generated 0603 resistor, held flush to the right edge by its second termination's +x face, follows the board 10 mm wider; a STEP model's face stays `Resolved` through a revision with the same file, becomes `Inexact` (stops driving, `board.reference_inexact`) with another file, and is `Resolved` again after a pick at the note's point; a glTF face is refused with the finding and nothing changes.
- `models/named_tests.rs`: names on every build, in a fresh process, the resistor's golden names, the envelope box, STEP bodies by key.

### Measured

`crates/automation/tests/model_build_timing.rs` (ignored) builds a board four times in one process with one source cache, as the model worker does: the first build is cold (every model built), the rest warm. Each board ran in a fresh process, three times, interleaved with the same test on `component-model` cda6183 (built in a copy of that tree). `ci` profile, Apple M5 Max, load average 7–20 from other agents. Medians of the three processes, ms:

| Board | Build | Before (cda6183) | S4 | Difference |
| --- | --- | --- | --- | --- |
| ESP32-S31 dev board (20 placements) | First, cold | 28,810 | 30,429 | +1,619 (+5.6 %) |
| | Later, warm | 28,500–29,700 | 28,900–29,300 | within noise |
| Showcase (9 placements) | First, cold | 5,973 | 6,478 | +505 (+8 %) |
| | Later, warm | 5,400–5,800 | 4,800–5,900 | within noise |

The cold build pays once per process for each generated model's standalone design, evaluated once at the end of its commands (`defer_evaluation`); evaluated after every command, as at first, it cost +3.7 s and +1.1 s. The board then evaluates the inserted features again, as it evaluated the builder's commands before, so a warm build costs what it did. **The first 3D open in a session is therefore about 0.5–1.6 s slower in this profile; later opens and rebuilds are not.** Envelope and STEP designs are built only when a reference is traced, never by the 3D build.

### Picking and S6

- **Picking (S5, extended).** `PickRequest` carries the same `TraceModel` as a trace, and `model_refs::pick` names the point on `ModelDesign::of`, so the 3D Dimension tool picks generated, envelope and STEP faces as it picks part-file faces, from the same caches: the board point goes back into the model frame, never through the board's own body names. A glTF part is refused before the host is asked, with the mesh finding above, which the tool card shows. `model_refs_kinds_tests::a_3d_pick_on_a_generated_end_face_holds_r1_to_the_edge` picks R1's end face at its board position, adds the dimension and widens the board; `a_3d_pick_on_a_gltf_model_is_refused_with_the_mesh_finding` checks the refusal.
- **S6.** The envelope is the one kind the board does not insert from its named design; when the board design becomes persistent, inserting the envelope design per package revision gives its faces board names too.

## 17. Fixes from the second VM test drive

- **A distance never turns a part.** The solve treated every part's rotation as an unknown and pinned only what the first freedom analysis left free. A distance from a second point of the same part to the same edge (J1's tongue edge, 0.6 mm above its shell face, at 1 mm) is satisfied by turning, so the analysis saw the rotation determined, found no conflict and turned J1 23.7°. Now a part's rotation is pinned at its stored (or moved-to) value before the first solve unless a relation that sets a direction names it (`PartRelation::orients`: angle, parallel, perpendicular, horizontal, vertical, tangent). Such a dimension is over-constraining and refused, in words that name the new dimension, the existing one and the part ("… conflicts with J1's Shell face to the bottom edge (0 mm), which already holds J1; edit or delete that dimension instead").
- **Line distances measure from the middle.** The views measure a line's distance from its middle (`dimensions::view::shown`); the solve measured from the line's start. R5's body edge runs along the board's y from 11 to 12 mm, so the tool showed 11.5 mm while the solve read 11 mm: 11 mm committed and nothing moved. The solve now holds a point at the first line's middle (`parts::from_middle`), so the value typed is the value shown.
- **Words.** Model references read "J1's Shell face" (the body as its model names it), never the persistent face name; values carry their unit. A board or footprint value that would turn the sketch inside out names the dimension by its value ("the dimension ‘corner_r’ would turn the shape inside out"), never Solidcraft's constraint ids.
- **A spline top edge on the showcase.** Deleting the top line retires the name `top` with it; J1 is dimensioned to the left and bottom edges, which keep their names and curves, so nothing is lost and `height` 28 → 34 puts J1 at (6, 18), exactly where it lands on the untouched showcase (its own `height / 2 + 1 mm`). Its eight clearance errors are its carried tracks, as on the plain board. What does change is the board: `height` is a `DistanceY` between the rectangle's right corners, which a fillet makes virtual corners held on two lines. With the top line gone, the top-right corner is held on the right line only, and the spline is drawn over new, unconstrained points, so `height` moves a lone point and no outline curve. Both now hold: a free-form curve's ends are tied with a Coincident to the existing points they are drawn on (by `board.edit` from any surface), so on a plain rectangle `height` still moves the curve; and a board parameter whose dimensions move no outline curve or mounting hole (found by re-solving it nudged by 1 %) is reported as `board.param_idle`, a warning naming the parameter. On the filleted showcase the spline's ends are tied to the fillet arcs, which nothing ties to the virtual corner, so `height` is reported. Drawn over the line instead of after deleting it, a curve whose two ends lie on the ends of a drawn line replaces that line, as in CAD sketchers: the line turns construction (out of the profile, the outline, fab output, DRC and 3D; dashed in Board shape mode) and keeps its constraints and edge name. The fillet tangencies and the virtual corner `height` holds stay, so `height` moves the curve's ends with the corners and the outline stays closed, with no `board.param_idle`. Deleting the construction line is deliberate; if that leaves a parameter idle, the warning says so.

## 18. Fixes from the third VM test drive

- **Refusals name the reference in words.** `TraceRequest::words` carries how the reference reads ("J1 · top face", `dimensions::model_feature_words`), and the host's refusals lead with it: "J1 · top face is parallel to the board; pick an edge of it", "J1 · front-right edge is not parallel to the board; pick a vertex of it". An untraced face is named by its height (top, bottom) or side, an untraced edge by the corner it is picked at. A missing face or edge reads "… is no longer on the 3D model; pick it again"; the faces an agent may pick instead, by persistent name, follow in a second sentence, which the app does not show and a stored note does not keep.
- **Whole messages.** The tool card's refusal wraps (`kit::refusal_wrapped`) instead of showing one clipped line with a tooltip over the hint. The update review's lines wrap too, with Show on the first row.
- **Flush parts win over the board edge.** Over a part's body, board edges are not candidates, so a click on J1's front face picks J1 (its face, or its own edge within reach), not the board edge J1 stands flush on. Off the parts, board edges are taken within Solidcraft's 6 points, as part edges are.
