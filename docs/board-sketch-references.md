# Board sketch with assembled parts (design)

Design for review, 9 November 2026. Base: `component-model` at 88fc2bb. Nothing here is built.

The founder asked: "can we do complex board shapes also with solidcrafts sketch solver? and use all edges/curves as dimension references to each other and to 3d parts on them?", and then for "the design (scope, data model, and which Solidcraft API it would use)". He then set two directions:

1. **The board outline is a closed loop.** The shape is a PCB outline as soon as its curves form a closed loop that Solidcraft can extrude.
2. **Parts are assembled, not projected.** A part's 3D model sits on the board the way a part sits in a Solidcraft assembly. One mate is already set and cannot be edited: the part's seating plane lies on the board's top or bottom surface. Dimensions go straight to the faces, edges and points of the part's own 3D model, not to a projected outline.

The result: one board sketch whose curves, including splines and ellipses, can be dimensioned to each other and to the pads, courtyards and 3D faces of the parts on the board. A part either is placed by those dimensions or itself drives the board's edges.

## 1. Where it stands (verified in code)

- **Solver.** Solidcraft's sketch solver solves the board sketch. `SketchBoardEvaluator` (`crates/solidcraft-integration/src/board.rs`) runs it behind the core's `BoardShapeEvaluator` (`crates/editor-core/src/layout/outline.rs`). Any board curve or point can already be dimensioned to any other: `SketchEdit::AddDimension` takes any `SketchRef`s (`library/footprint_source.rs`).
- **Curves and relations Pcbcraft allows.** The board shares the footprint sketch subset (`footprint/check.rs`, `subset`). That subset refuses free-form curves, links, patterns and `PatternPoint`. Loops are built from lines, arcs and circles only (`board.rs`, `pieces_of`). `library::Segment` is a line or a bulge arc, and everything downstream reads it: fills, edge checks, fab output and the 3D laminate. The sketch bounds are the footprint ones: 4,096 points, 8,192 constraints and 1 MiB (`footprint_source.rs`, `MAX_SKETCH_*`).
- **The outline must be closed on every edit.** The `outline` role is an explicit list of curves. An edit that leaves it open, unsolved or out of range is refused (`board.rs`, `loop_of`; [board shape](board-rules-and-shape.md#board-shape-cutouts-and-edge-relative-placement)).
- **Parts relate to the board in one direction only.** An `EdgeConstraint` (`model/board.rs`) places a placement, fill or keepout. It names an edge, an anchor, `along` and `offset` expressions, an alignment, `follow_edge` and a rotation. After every operation, `layout::outline::follow` places the target. A part never drives an edge. There are no part-to-part dimensions and no references to 3D geometry.
- **The fixed mate already exists in 3D.** Every part occurrence in the native board model has one Solidcraft `planar` joint to the board (`native.rs`, `joint_params`). On top, the joint is flipped with offset `board_thickness`. On the bottom, the part hangs from z = 0. The joint's values `[x, y, angle]` are written from the core `Placement`. So the stackup already moves parts in z, and the core placement is already the authority.
- **Part 3D models.** A `PackageModel` is `Envelope`, `Generated`, `Library` or `PartFile` (`library/package_model.rs`). A part file is an exact `ModelRef` revision. It is evaluated through `Session::opened_validating`, cached by key, and inserted once per definition with shared occurrences (`models/part_file.rs`). Only part files carry Solidcraft's persistent face and edge names.

## 2. What Solidcraft offers (upstream, public)

Checked in two places: upstream `tangency-labs/solidcraft` main as checked out at `/Users/amd/dev/solidcraft` (4f3ac07), and our pin (`DAmesberger/solidcraft` 04d3c2f, `~/.cargo/git/checkouts/solidcraft-258829c3ab1d7265/04d3c2f`). Paths are relative to `crates/`.

| Need | Upstream API | Path |
| --- | --- | --- |
| Free-form curves | `CurveKind::{Ellipse, Spline{control, degree, periodic, nurbs}, Conic}` (Solidcraft has no elliptical arc) | `sketch/src/model.rs` |
| Curve geometry, arc fitting | `Sketch::segs(ci) -> Vec<Seg2>`; `Loop2::fit_arcs(tol)` (biarcs, tangent-continuous) | `sketch/src/model.rs`, `geom/src/arcfit.rs` |
| Closed regions, open ends | `find_profiles` / `find_drawn_profiles` → `Profile{region: Region2, outer_curves, hole_curves}`; `loose_ends` → `EndReport` | `sketch/src/profiles.rs`, `sketch/src/ends.rs` |
| Rigid bodies in a sketch | `ConstraintKind::PatternPoint{p, q, directions: [(line, offset)]}`: `q = p + u·dx + u⊥·dy`, where `u` is a live line's direction | `sketch/src/model.rs`, residual in `sketch/src/solver.rs` |
| Relations | `PointLineDistance`, `PointCircleDistance`, `LineCircleDistance`, `CircleCircleDistance`, `Angle`, `LineAngle`, `Parallel`, `Collinear`, `PointAlong`, `ArcLength`, `Smooth` (G2), `Constraint.driven` (a reference dimension that measures and does not constrain) | `sketch/src/model.rs` |
| Staged solving | `solve_with(sk, &Hold, reference, Analysis, budget)`; `SolveReport{dof, oversized, failing}` | `sketch/src/solver.rs` |
| Persistent topology | `naming::face_names`, `face_on_body(body, name, pick) -> (index, point, inexact)`, `edge_names`, `find_edge` | `doc/src/naming.rs` |
| A face or edge as plane geometry | `project::resolve_face_indices(st, plane, LinkKind, body, indices)`, `project::resolve(…, LinkSource::Edge/Vertex…)` → `Resolved{geom: Vec<LinkGeom>}` | `doc/src/project.rs` |
| Assembly | `Occurrence`; `joints::{Joint, JointKind::Planar, JointOrigin, Snap::{FaceCenter, CircleCenter, Edge, Point}}`; relations `Parallel`, `PointOnPlane`, `LineOnPlane`…; `joints::solve_holding` | `doc/src/assembly.rs`, `doc/src/joints.rs`, `doc/src/joints_relations.rs` |

**Drift between the pin and upstream.** At our pin, `naming::face_on_body` returns `(usize, Vec3)`. Upstream it adds an `inexact` flag ("split, or merged into another"). `project::refresh_links` and `naming::find_edge` also differ in signature. This design uses the upstream forms. It must not use anything present only in the fork.

**Nothing in this design needs a new Solidcraft API.** Section 5 lists the gaps and what Pcbcraft does about each.

## 3. Scope

### In

1. **Closed-loop outline.** The board is the closed region that the board sketch's drawn (non-construction) curves form. Its outer loop is the outline, and its inner loops are cutouts or slots. While the curves form no valid region, the board keeps its last valid shape and the editor shows where the loop is open (§4.1).
2. **Free-form curves.** Fit and control splines (closed periodic ones included), ellipses and conics, in the outline, in cutouts and as construction geometry, with the relations listed in §2.
3. **Parts as sketch participants.** Each placed part (and each drawn fill or keepout, as `EdgeConstraint` allows today) can enter the board sketch as a rigid frame with three unknowns: x, y and rotation about the board normal. The fixed seating mate removes the other three degrees of freedom; it is not stored in the sketch and cannot be edited.
4. **Part features.** These can be dimensioned:
   - the part's origin and axes;
   - pad centres and pad sides;
   - courtyard sides and outline;
   - authored part-file anchors (`pin1` and named construction points, as `EvaluatedPart::points` already reads);
   - for part-file packages, **3D faces, edges and vertices of the part's own model**, by persistent name (§4.3).
5. **Every pairing.** Board curve to board curve (as today), part to board curve, part to part, and a 3D face to a board curve or to another part's feature.
6. **Which side drives.** Each participant is either `Solved` (its pose is unknown and dimensions place it; it never moves the board) or `Fixed` (its pose is data; its features are constants that can drive board curves). A reference dimension (`driven`) only measures.
7. **Replacing `EdgeConstraint`.** Its data, its `follow` pass and its three commands are replaced by sketch dimensions on participants (§4.6).

### Out

- **Coupled solving.** A part that is both solved and drives the board (open question 2).
- **Other 3D relations.** Any relation outside the board plane: heights, tilt, and mates between parts beyond the fixed seat. Under the fixed mate these are constants. A dimension to a face parallel to the board (a height) is allowed only as a reference dimension.
- **Faces with no single trace in the board plane:** sloped or free-form faces. They are refused with a reason; there is no silhouette fallback.
- **3D references on models without stable names.** `Envelope`, `Generated` and `Library` (mesh) models offer only pads, courtyard and their body box. Upstream `face_on_body` returns `None` for mesh bodies.
- **Other participants.** Tracks, vias and copper (routing has its own engine), and board dimensions that change a part's parameters.
- **Panels.** Several disjoint boards in one sketch are reported, not supported.

## 4. Data model

All types live in `pcbcraft-core` (`model/board.rs`, `layout/outline.rs`): pure, serde and `JsonSchema`, with no Solidcraft dependency. The sketch stays an opaque `SketchBlob`, which the core never interprets.

### 4.1 Outline as a closed loop

The `outline` curve list is dropped. After every solve, the evaluator runs `find_drawn_profiles` on the solved sketch and accepts the result when:

- exactly one region remains once construction curves are ignored;
- its outer loop does not cross itself (a crossing appears as several regions; the finding names the crossing point from `intersections`);
- every hole loop lies strictly inside the outer loop, apart from the others.

The outer loop is the outline. Each hole loop is a cutout, unless a `CutoutRole` marks it as a slot (plated or not). The role is keyed by its curve ids, as today, so a loop's kind survives edits.

| Sketch state | Board | Finding |
| --- | --- | --- |
| Valid region | The new `SolvedBoard` | none |
| No closed loop yet, or open ends | The last valid `SolvedBoard` kept; the editor marks the open ends from `loose_ends` | `board.shape_open`: advisory while the Board shape mode is open, an error in checks and preflight |
| Crossing, several regions, hole touching the outline | The last valid shape kept | `board.shape_invalid`, with the place |

A drafting edit is therefore no longer refused for leaving the loop open. It is refused only when the sketch does not solve or leaves the coordinate range, as today. A new project's rectangle is one valid region, so existing behaviour is unchanged.

**Extrudable.** `SolvedBoard` stays lines and bulge arcs, so fills, edge checks, Gerber/DXF output and the laminate are untouched. A spline, ellipse or conic enters the outline through `Sketch::segs` and `Loop2::fit_arcs(tol)`. `tol` is the chord tolerance `board.rs` already uses (`CHORD_MM` = 0.01 mm; open question 4). Each outline and cutout segment records the sketch curve it came from (`SolvedBoard` gains `curves: Vec<SketchCurveId>` per loop). A pick on the board's side face in 3D, or on a segment in 2D, therefore names the same curve.

### 4.2 Participants and references

```rust
/// A placement, fill or keepout in the board sketch as a rigid frame.
pub struct Participant {
    pub target: ConstraintTarget,       // existing: Placement(Id) | Fill(Id) | Keepout(Id)
    pub drive: Drive,
    /// The frame in the sketch: origin point and a construction axis line from it.
    pub origin: SketchPointId,
    pub axis: SketchCurveId,
}

pub enum Drive {
    /// Pose unknown (x, y, rotation); dimensions place it; it never moves the board.
    Solved,
    /// Pose is the placement's; its features are constants that may drive board curves.
    Fixed,
}

/// One part feature in the sketch: what it is, and the sketch entities that stand for it.
pub struct FeatureReference {
    pub id: ReferenceId,                // board-local, never reused
    pub feature: FeatureRef,
    pub entities: Vec<SketchRef>,       // points/curves the dimensions attach to
    pub status: ReferenceStatus,
}

pub enum FeatureRef {
    Origin { target: ConstraintTarget },
    Pad { placement: Id, pad: PadId, at: PadPoint },             // Centre | Side(Side)
    Courtyard { placement: Id, side: Option<Side> },             // None: the outline
    BodyBox { placement: Id, side: Side },                       // any package model
    Anchor { placement: Id, name: AnchorName },                  // part-file construction point
    Face { placement: Id, body: BodyName, face: FaceName, pick: [f64; 3] },
    Edge { placement: Id, body: BodyName, edge: EdgeName3d, pick: [f64; 3] },
    Vertex { placement: Id, body: BodyName, edge: EdgeName3d, end: End },
}

pub enum ReferenceStatus { Resolved, Inexact { why: String }, Lost { why: String } }
```

`BoardSketch` gains `participants` and `references`. Both are serde-defaulted and skipped when empty.

**Single source of truth.** The geometry of a feature always comes from the library snapshot: from the footprint for pads and courtyard, and from the evaluated part file for faces. Before each solve, the evaluator writes it into the reference's entities, in the participant's frame. Those entities are bound with `PatternPoint` and `directions = [(axis, local offset)]`, so the feature moves rigidly with its part. A circle's radius gets a fixed `Radius` constraint. For a `Fixed` participant, the points are simply fixed. Bottom-side parts mirror the local y offset, as the bottom mount flips the occurrence. The positions stored in the blob are only the last result, which is how Solidcraft's own links work (`sketch/src/link.rs`: "the sketch only stores the link and the geometry it produced last time").

Only features someone has referenced enter the sketch. An unreferenced pad costs nothing.

### 4.3 The assembled part, and how 3D faces enter a 2D solve

The part is a Solidcraft occurrence held by its fixed planar joint (§1). That joint leaves exactly x, y and the rotation about the board normal. With z, tilt and roll fixed, a feature of the part model enters the board solve in one of these ways:

| Part feature (part-file frame) | In the board plane | Example |
| --- | --- | --- |
| Planar face square to the board | Its line (an exact face seen edge-on) | USB-C shell front face flush with the board edge |
| Cylindrical or conical face with its axis along the normal | Its centre (and radius) | A boss or a pin centred on a mounting hole |
| Straight or circular edge parallel to the board | Its line or arc | The bottom edge of a housing |
| Vertex | Its point | A housing corner |
| Face parallel to the board | Nothing in the plane; its height is a constant | Reference (height) dimensions only |
| Anything else (sloped, free-form) | Refused: "pick an edge of this face" | |

**This is not the projection approach.** The stored reference is the 3D face by its persistent name on the assembled part; no silhouette and no outline copy is stored. The plane form is exact rather than approximate, because the fixed mate makes x, y and the rotation the only unknowns, and a distance from a square face to a board edge is the same at every height. In PCB 3D, the dimension is drawn to the face itself.

**Resolving a face.** The integration evaluates the part file through the existing cache (`part_file::evaluate`, which today keeps the source `Document`; it would also keep its `ModelState`). It then finds the face with `naming::face_on_body(body, face, pick)` and gets its trace from `project::resolve_face_indices(state, XY plane, LinkKind::Project, body, [index])`, in the part's own frame. Edges and vertices use `naming::find_edge` and `project::resolve` with `LinkSource::Edge`/`Vertex`. The part file's convention (origin = footprint origin, z = 0 = seating plane; `models/part_file.rs`) makes that frame the footprint frame.

Names are taken from the part file's own design, not from the board model. Once a part is inserted into the board, its feature ids are renumbered (`part_file::insert`), so names read there would not be stable.

### 4.4 Stable identity, revisions, missing references

| Reference | Identity | When the part or library revision changes |
| --- | --- | --- |
| Participant | Placement instance `Id` (designators are labels) | A deleted placement makes its references `Lost` |
| Pad | `PadId` (stable across footprint revisions) | Pad absent in the new revision: `Lost` |
| Courtyard / body box side | The side of the box in the part frame | Always resolves; the geometry follows the revision |
| Anchor | The authored construction point name in the part file | Absent: `Lost`. This is an authored identity, never matched heuristically |
| Face / edge / vertex | (body name, Solidcraft persistent name, pick point in the part frame), within an exact `ModelRef` | Re-found by name in the new revision. Upstream `inexact` (split or merged face) → `Inexact`; not found, or no longer square to the board → `Lost` |

A **lost** reference keeps its last geometry as a constant. Its part therefore does not jump and the board does not change, which matches how Solidcraft treats a lost link. It is reported as `board.reference_lost`, naming the part, the feature and the reason. `Inexact` is reported as `board.reference_inexact`. Neither is ever dropped or silently re-pointed. Both block fabrication preflight until the user picks the reference again or deletes it (open question 3).

A **library update's impact preview** lists every reference the update would make `Lost` or `Inexact`, before anything changes. `update.apply` acknowledges them like the other impact rows. This is the atomic impact preview rule, applied to sketch references.

### 4.5 Commands, undo, persistence

- **Undo.** Everything is a `board.edit`, one undo step and atomic. New `BoardEdit` variants: `AddParticipant { target, drive }`, `SetDrive`, `RemoveParticipant`, `Reference { feature } → created ids`, `RemoveReference`, `SetCutoutKind`. `SketchEdit` gains `AddSpline { points, control, degree, closed, construction }`, `AddEllipse { center, major_end, minor_radius, construction }` and `AddConic`. `GeomConstraint`/`DimKind` gain the §2 relations the board needs.
- **One-step dimensions.** `BoardEdit::Dimension { kind, a: BoardRef, b: BoardRef, value }`, where `BoardRef` is `Sketch(SketchRef) | Feature(FeatureRef)`, creates any missing participant (as `Solved`) and reference in the same step. A client never needs to know sketch ids.
- **Expressions.** `DimValue` gains `Expr(Expr)`, evaluated with the board parameters by the evaluator. This covers what `EdgeConstraint::along` and `offset` do today (`height / 2 - 1 mm`).
- **Moving a solved part.** A move or rotate of a `Solved` part becomes a solver drag of its frame: the part goes as near the target as its dimensions allow, so a part with one dimension to an edge slides along that edge. A move that cannot land is refused in today's words (`outline::held_message`). A locked part is a `Fixed` participant whose move is refused, as today.
- **Persistence.** All of this lives inside `board_shape` in the project file. `Project::edge_constraints` and `BoardSketch::outline` are removed. The format has one version and no migration: a file that still has them is refused by `deny_unknown_fields`. The only data that uses them is the showcase (`Engine::showcase_hosted`, J1 held to the left edge), which is regenerated. The sketch bounds become the board's own, up to Solidcraft's (`MAX_POINTS` 20,000, `MAX_CONSTRAINTS` 40,000), with the byte cap measured in slice S5.

### 4.6 What replaces `EdgeConstraint`

| `EdgeConstraint` | Sketch equivalent |
| --- | --- |
| `target` | `Participant { drive: Solved }` |
| `edge` (named) | The `NamedEdge`'s curve. A filleted edge measures on a construction line between its virtual corners, so a corner-radius change leaves the part in place (today's rule) |
| `anchor` Start / End / Middle / Fraction | Coincident to an end; `Midpoint`; `PointAlong { at }` |
| `offset` | `PointLineDistance` from the reference point to the edge (`PointCircleDistance` for an arc edge) |
| `along` | `PointLineDistance` to a construction line square to the edge through the anchor (`ArcLength` on a concentric construction arc for an arc edge) |
| `align` Origin / NearSide / Centre | The origin, a courtyard side, or the courtyard centre. `NearSide` is stored as the explicit side it resolved to; it is never re-inferred |
| `follow_edge` + `rotation_deg` | `Angle(axis, edge) = rotation`; without it, the rotation stays a free unknown that keeps its stored value |

The desktop's "Constrain to edge" emits this expansion as one `board.edit`. The Edge card, the hold mark and `layout::move_refusal` then read participants and references instead of `EdgeConstraint`. `layout::outline::follow` is deleted: the solve itself produces the poses.

### 4.7 One source of truth for the pose

`Placement { position, rotation_deg, layer }` stays the stored pose that copper, the copper graph, footprints, checks and fab output read. For a `Solved` part, the engine writes it from the board solve inside the same operation, as `follow` does today and as `SolvedBoard` is stored. The native model's planar joint values are written from `Placement` (`joint_params`, unchanged). The flow is:

board sketch → solve → `Placement` → planar joint `[x, y, angle]` → 3D.

Solidcraft's assembly solver never decides a pose, so there is one solver and one stored result. Changing the stackup moves parts in z through the joint's `board_thickness` offset, never in the plane.

### 4.8 Consequences for copper, fills and checks

- **Moved parts.** A solved part's move is the same placement change `layout.move` makes, with the same effect on tracks at its pads. The copper graph, fill keys (`inputs_key`) and checks see an ordinary placement change. The sketch never decides connectivity: two pads made coincident by a dimension are joined only if their copper is ([copper graph](copper-graph.md#where-connectivity-lives)).
- **Board edges.** A board edge driven by a `Fixed` part changes `SolvedBoard`. Board-following fills follow it and go stale until refilled. The edge-clearance check reads the new outline.
- **New findings:** `board.shape_open`, `board.shape_invalid`, `board.reference_lost`, `board.reference_inexact`, and `board.part_unsolved` (a solved part's dimensions conflict, or its group is too large, §6). `board.constraint_unresolved` is removed.

### 4.9 CLI and MCP

The typed commands take ids (`Id`, `PadId`, persistent face names). The automation layer also accepts a compact form and resolves labels to ids when it receives the request. These labels are never stored.

```
edge(left)                 curve(l7)              J1.origin
J1.pad(A6)                 J1.pad(A6).side(left)   J1.courtyard.left
J1.anchor(pin1)            J1.face(Shell, "F3:side:l2")
```

`board.shape` reports participants with their drive mode, remaining degrees of freedom and reference status. `board.references` lists each reference with its geometry and status. When asked, it also lists the faces of a part that can be referenced (those in §4.3 that have a trace). An agent can then dimension "J1's shell front face 0.5 mm past edge(right)" in one `board.edit`.

## 5. Solidcraft: gaps and where they are handled

Nothing in this design needs a Pcbcraft-specific Solidcraft API or a fork. The gaps:

| Gap | Upstream? | Handling |
| --- | --- | --- |
| No first-class rigid set in the sketch | `PatternPoint` with `directions` already holds a point rigidly in a live line's frame (it solved with no pattern entry, §6) | Pcbcraft side. A generic `Rigid` constraint upstream would only save rows; not needed |
| `LinkSource` has no variant for geometry the host supplies | Links are resolved by the document crate against its own model | Pcbcraft side: fixed points or `PatternPoint` entities plus the `FeatureReference` table. A generic `LinkSource::External { key }` could be proposed upstream later; not needed |
| One connected group is capped at `MAX_COMPONENT_VARS` = 4,000 unknowns (dense LM; `sketch/src/solver.rs`) | By design upstream | Pcbcraft stages the solve (§6) so parts are separate groups |
| The board's own subset refuses splines, `PatternPoint` and links | Pcbcraft code (`footprint/check.rs`) | A board subset separate from the footprint subset |
| `inexact` from `face_on_body` | Upstream only, not at our pin | Use the upstream form when Pcbcraft next repins to upstream main |

**Rejected alternative: Solidcraft's assembly solver as the authority.** Each dimension would become a relation joint (`Parallel`, `PointOnPlane`, `LineOnPlane` with `offset`) between the part's occurrence and the board laminate's side faces, solved by `joints::solve_holding`. It was rejected for four reasons:

1. It needs the evaluated board model, which Pcbcraft builds in the background, inside every synchronous core operation.
2. It places occurrences but cannot drive a sketch, so a part could never set a board edge.
3. Its loop solver is bounded per group (`MAX_LOOP_VALUES` = 64 and `MAX_LOOPS` = 100 in `doc/src/joints.rs`), and it counts degrees of freedom only for 256 occurrences (`MAX_DOF_OCCURRENCES`).
4. It would put a second pose solver beside the board sketch.

Open question 1 asks the founder to confirm this choice.

## 6. Performance

**Measured.** A scratch benchmark (not committed; Solidcraft sketch crate at our pin, release build, Apple M5 Max, load average 30–49 from other agents) built a fully dimensioned rectangle and N rigid parts. Each part had an origin, an axis line and K referenced points bound by `PatternPoint`. Each part was dimensioned to the board edges, or chained to the previous part in rows of 20.

| Solve | Parts × refs | First solve | Width edit |
| --- | --- | --- | --- |
| Joined (one sketch, one solve) | 100 × 2 | 5.2–5.8 ms | 2.0–2.4 ms |
| | 300 × 2 | 49–54 ms | 15–18 ms |
| | 200 × 6 | 166 ms | 53 ms |
| | 500 × 2, 300 × 6 | **refused**: 4,006 / 4,806 unknowns > 4,000 | |
| Staged (board held, `solve_with` + `Hold`) | 500 × 2 | 7.2–9.3 ms | |
| | 2,000 × 2 | 36–47 ms | |
| | 2,000 × 6 | 144–178 ms | |

The joined solve grows faster than linearly and hits the solver's group cap near 500 parts. Staging keeps each part its own small group.

**Staged solve (design).** One evaluation runs two stages:

1. **Stage 1:** the board curves and `Fixed` participants. Every constraint that touches a `Solved` participant is marked `driven` (it measures only), and those participants are held.
2. **Stage 2:** the board and `Fixed` entities are held, and the `Solved` participants are solved. A part is its own group unless part-to-part dimensions chain it to others. A chain larger than the solver's cap is `board.part_unsolved`, carrying the solver's `oversized` count and naming the parts.

The board's degrees of freedom come from stage 1, and each part's from stage 2.

**Targets (labelled as targets, to be measured end to end in S5):**

- a board edit with 300 solved parts, two references each, re-solves within one 60 Hz frame (16 ms);
- 2,000 parts re-solve within 200 ms.

The bare staged solver meets both above. Not yet measured are the reference refresh, the part-file face lookups (cached per part key, as `part_file::evaluate` caches) and the sketch blob's canonical round-trip check (`footprint/check.rs` serialises and compares).

**Selection and camera rebuild nothing.** The solve runs only in an engine operation that changes the board sketch, a participant's placement or a referenced library revision (`inputs_changed` widened accordingly). Hover, pick, selection and the camera read the stored `SolvedBoard`, the placements and the cached reference geometry. Part moves reach 3D as new joint values through the existing in-place update path. Outline changes reach it through the laminate's existing redraw.

## 7. Slices (agent hours)

A contract commit (C0) comes first: the types of §4, findings, schemas and failing tests. After C0, S1 and S2 can run in parallel; S3 needs S2, and S4 needs S3.

| Slice | Content | Estimate |
| --- | --- | --- |
| C0 | Contract: types, commands, schemas, finding codes, test skeletons | 2 |
| S1 Closed-loop outline and free-form curves | Derived outline from `find_drawn_profiles`, open and invalid states with the last valid shape, cutout kinds by curve, the board subset with spline/ellipse/conic, `fit_arcs` into `Outline`, segment→curve map, open-end markers and the Spline/Ellipse tools in Board shape mode | 9 |
| S2 Parts in the solve | Participants, frames by `PatternPoint`, pad/courtyard/body-box/anchor references, the staged solve, drive modes, drag-to-move, `EdgeConstraint` removed and expanded (`follow`, three commands, Edge card, hold mark, move refusal, showcase), automation reference syntax | 16 |
| S3 3D references | Part-file face/edge/vertex references by persistent name, the §4.3 trace rules, reference status and findings, the library-update impact rows, a repin for upstream `face_on_body` | 8 |
| S4 3D picking and display | Pick a part face or the board's side face in PCB 3D to start a dimension; draw dimensions to faces in 3D; shared selection and undo with 2D | 6 |
| S5 Evidence | End-to-end timings against the §6 targets, board sketch bounds, a VM UI run (dimension a USB-C shell flush with a spline edge), editor evidence | 3 |
| | **Total** | **44** |

## 8. Open questions for the founder

1. **Pose authority.** Should the board sketch solver decide every pose, with Solidcraft's assembly keeping only the fixed seating mate? Or should Solidcraft's assembly relations decide it? *Recommendation: the sketch solver (§4.7, §5).* It is synchronous, measured to scale (§6), can let a part drive an edge, and keeps one solver. The 3D faces are still referenced by name on the assembled part.
2. **Two-way coupling.** Should one part both be placed by dimensions and drive the board? *Recommendation: not in this work.* Each part is `Solved` or `Fixed`. Coupling forces one joined group, which the measurements cap near 500 parts with two references. It can be added later as a third drive mode, limited to the parts that need it.
3. **Lost references and fabrication.** Should a lost or inexact reference block fab output? *Recommendation: yes, a blocking preflight finding that cannot be waived.* The part keeps its last pose, so nothing moves meanwhile.
4. **Spline tolerance.** Should free-form outline curves reach fills, checks, Gerber and the laminate as arcs fitted within 0.01 mm (`CHORD_MM`, already used for board inside tests)? Or should `Segment` gain Béziers, which every consumer would then have to handle? *Recommendation: fitted arcs now.* The tolerance should be a design rule, so a fabricator profile can tighten it.
5. **Packages without part files.** Should `Generated` and `Library` models offer 3D face references? *Recommendation: no.* They offer pads, courtyard and the body box. 3D references come with converting a package to a part file, which is the planned path ([part templates](part-templates.md)).
6. **Open outline in checks.** Should an open loop be an error everywhere outside Board shape mode? *Recommendation: yes.* The board keeps its last closed shape, and the error says so.
