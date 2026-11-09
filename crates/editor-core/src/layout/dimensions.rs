//! Dimensioned parts (docs/board-sketch-references.md §5.2): the
//! `layout.dimension_*` commands, the parts solve that runs after the
//! board solve, and the reference findings.
//!
//! Only parts a [`PartDimension`] names are solved; every other part keeps
//! its stored pose. Dimensioned parts are grouped into connected components
//! (two parts are connected when a dimension names both), and each
//! component an operation touched is solved by the host's board evaluator
//! ([`super::outline::BoardShapeEvaluator::solve_parts`]) in a scratch
//! Solidcraft sketch: the named board (and housing) geometry held fixed, per
//! part an origin and a construction axis, its referenced features bound to
//! that frame, and the dimensions. A degree of freedom no dimension fixes
//! keeps its stored value. A move or rotate of a dimensioned part is a
//! solver drag: the part slides along what holds it ([`landing`] previews
//! it), and a move that cannot land is refused ([`held`] says so before a
//! client tries).
//!
//! A reference whose source is gone (a deleted part, pad, anchor or board
//! curve) stops its dimension driving, so the parts keep the poses it gave,
//! and is reported by [`checks`] (`board.reference_lost`), which blocks
//! fabrication output with no waiver.
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::library::footprint_source::{
    DimKind, DimValue, SketchCurveId, SketchPointId, SketchRef, SolveStatus,
};
use crate::library::{Outline, Segment};
use crate::model::Project;
use crate::model::board::{BoardShape, BoardSketch, ConstraintTarget};
use crate::model::dimension::{
    BoardRef, FeatureRef, MAX_DIMENSION_REFS, MAX_PART_DIMENSIONS, PartAxis, PartDimension,
    PartDimensionSpec, PartRelation, ReferenceStatus,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

pub mod features;

/// Finding code: a reference whose source is gone (a deleted part, pad,
/// anchor, board curve or model face). It keeps its last geometry and
/// blocks fabrication output with no waiver (§5.5).
pub const REFERENCE_LOST: &str = "board.reference_lost";
/// Finding code: a reference found again but not provably the same
/// geometry (a renamed or unverified model face). It blocks fabrication
/// output with no waiver until acknowledged in an impact preview (§5.5).
pub const REFERENCE_INEXACT: &str = "board.reference_inexact";

/// Part-dimension mutations; each is one undo step.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum DimensionCommand {
    /// Add a part dimension and solve the parts it names; the outcome names
    /// it (`Outcome::DimensionAdded`). Refused when the parts would not
    /// solve.
    #[serde(rename = "layout.dimension_add")]
    Add { dimension: PartDimensionSpec },
    /// Replace a part dimension, by id, and re-solve.
    #[serde(rename = "layout.dimension_edit")]
    Edit { dimension: PartDimension },
    /// Delete a part dimension; the parts it named keep their poses.
    #[serde(rename = "layout.dimension_delete")]
    Delete { id: Id },
}

// A new command chooses its permission class here.
declare_classes!(DimensionCommand {
    Add "layout.dimension_add" Edit "Dimension a part",
    Edit "layout.dimension_edit" Edit "Edit a part dimension",
    Delete "layout.dimension_delete" Edit "Delete a part dimension",
});

/// What a dimension command produced for its caller.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum DimensionApplied {
    #[default]
    Nothing,
    /// The dimension `layout.dimension_add` allocated.
    Added(Id),
}

/// The refusal of a board evaluator that cannot solve dimensioned parts
/// (the trait's default).
pub fn not_yet() -> Error {
    Error::Unsupported(
        "layout.dimension: this board evaluator cannot solve dimensioned parts".into(),
    )
}

fn needs_evaluator() -> Error {
    Error::Unsupported(
        "layout.dimension: dimensioned parts need the host's sketch evaluator".into(),
    )
}

fn missing(id: Id) -> Error {
    Error::NotFound(format!("layout.dimension_missing: no part dimension {id}"))
}

/// Apply a dimension command to a candidate. The parts it names are placed
/// by [`follow`], which the engine runs after every operation; a dimension
/// that over-constrains them is refused there.
pub fn apply(
    project: &mut Project,
    command: DimensionCommand,
    _evaluator: Option<&dyn super::outline::BoardShapeEvaluator>,
) -> Result<DimensionApplied> {
    match command {
        DimensionCommand::Add { dimension } => {
            if project.part_dimensions.len() >= MAX_PART_DIMENSIONS {
                return Err(Error::Limit(format!(
                    "layout.dimension_limit: at most {MAX_PART_DIMENSIONS} part dimensions"
                )));
            }
            let id = project.allocate()?;
            let dimension = PartDimension::from_spec(id, dimension);
            check_new(project, &dimension)?;
            project.part_dimensions.push(dimension);
            Ok(DimensionApplied::Added(id))
        }
        DimensionCommand::Edit { dimension } => {
            check_new(project, &dimension)?;
            let slot = project
                .part_dimensions
                .iter_mut()
                .find(|d| d.id == dimension.id)
                .ok_or_else(|| missing(dimension.id))?;
            *slot = dimension;
            Ok(DimensionApplied::Nothing)
        }
        DimensionCommand::Delete { id } => {
            let before = project.part_dimensions.len();
            project.part_dimensions.retain(|d| d.id != id);
            if project.part_dimensions.len() == before {
                return Err(missing(id));
            }
            Ok(DimensionApplied::Nothing)
        }
    }
}

/// A new or edited dimension is well formed, names at least one part, and
/// every reference it names resolves now.
fn check_new(project: &Project, d: &PartDimension) -> Result<()> {
    if !well_formed(d) {
        return Err(Error::Invalid(format!(
            "layout.dimension: a part dimension needs 1–{MAX_DIMENSION_REFS} well-formed \
             references and a finite value"
        )));
    }
    if !d.refs.iter().any(|r| matches!(r, BoardRef::Part(_))) {
        return Err(Error::Invalid(
            "layout.dimension: a part dimension names at least one part".into(),
        ));
    }
    match (d.kind, &d.value) {
        (PartRelation::Dim(_), None) if !d.reference => {
            return Err(Error::Invalid(
                "layout.dimension: a driving dimension needs a value".into(),
            ));
        }
        (PartRelation::Geom(_) | PartRelation::PointAlong { .. }, Some(_)) => {
            return Err(Error::Invalid(
                "layout.dimension: a geometric constraint takes no value".into(),
            ));
        }
        _ => {}
    }
    let ids = board_ids(project);
    for r in &d.refs {
        match r {
            BoardRef::Housing(_) => {
                return Err(Error::Unsupported(
                    "layout.dimension: housing references are not yet supported".into(),
                ));
            }
            BoardRef::Board(SketchRef::Constraint(_)) => {
                return Err(Error::Invalid(
                    "layout.dimension: a dimension names board points and curves, not \
                     constraints"
                        .into(),
                ));
            }
            BoardRef::Board(b) => {
                if ids.as_ref().is_some_and(|ids| !ids.contains(b)) {
                    return Err(Error::NotFound(format!(
                        "layout.dimension_reference: the board sketch has no {}",
                        sketch_ref_name(b)
                    )));
                }
            }
            BoardRef::Part(f) => {
                features::geometry(project, f)?;
            }
        }
    }
    Ok(())
}

fn sketch_ref_name(r: &SketchRef) -> String {
    match r {
        SketchRef::Point(p) => format!("point `{}`", p.0),
        SketchRef::Curve(c) => format!("curve `{}`", c.0),
        SketchRef::Constraint(k) => format!("constraint `{}`", k.0),
    }
}

/// The point and curve ids of a sketch board, read from the sketch's
/// canonical JSON (only its entity ids; the core interprets nothing else of
/// it). `None` for a rectangle board (the evaluator's rectangle, whose
/// sketch the core does not hold) or a sketch it cannot read.
pub fn board_ids(project: &Project) -> Option<BTreeSet<SketchRef>> {
    #[derive(Deserialize)]
    struct Entity {
        id: String,
    }
    #[derive(Deserialize)]
    struct Entities {
        #[serde(default)]
        points: Vec<Entity>,
        #[serde(default)]
        curves: Vec<Entity>,
    }
    let BoardShape::Sketch(sketch) = &project.board_shape else {
        return None;
    };
    let e: Entities = serde_json::from_str(&sketch.sketch.0).ok()?;
    Some(
        e.points
            .into_iter()
            .map(|p| SketchRef::Point(SketchPointId(p.id)))
            .chain(
                e.curves
                    .into_iter()
                    .map(|c| SketchRef::Curve(SketchCurveId(c.id))),
            )
            .collect(),
    )
}

/// How one reference resolves now (§5.5). Slice S2 has no source of
/// `Inexact` references (model faces and housings come later).
pub fn reference_status(
    project: &Project,
    ids: Option<&BTreeSet<SketchRef>>,
    r: &BoardRef,
) -> ReferenceStatus {
    let found = match r {
        BoardRef::Board(b) => ids.is_none_or(|ids| ids.contains(b)),
        BoardRef::Housing(_) => false,
        BoardRef::Part(f) => features::geometry(project, f).is_ok(),
    };
    if found {
        ReferenceStatus::Resolved
    } else {
        ReferenceStatus::Lost
    }
}

/// Whether every reference of `d` resolves.
fn resolves(project: &Project, ids: Option<&BTreeSet<SketchRef>>, d: &PartDimension) -> bool {
    d.refs
        .iter()
        .all(|r| reference_status(project, ids, r) == ReferenceStatus::Resolved)
}

/// The words for a target: a part's designator, a fill's or keepout's
/// name.
fn target_words(project: &Project, target: ConstraintTarget) -> String {
    match target {
        ConstraintTarget::Placement(id) => super::outline::part_name(project, id),
        ConstraintTarget::Fill(id) => project
            .fill(id)
            .map_or_else(|| "A fill".into(), |f| format!("Fill {}", f.name)),
        ConstraintTarget::Keepout(id) => project
            .keepout(id)
            .map_or_else(|| "A keepout".into(), |k| format!("Keepout {}", k.name)),
    }
}

fn objects_of(target: ConstraintTarget) -> Vec<ObjectRef> {
    match target {
        ConstraintTarget::Placement(id) => vec![ObjectRef::Instance { id }],
        ConstraintTarget::Fill(_) | ConstraintTarget::Keepout(_) => vec![ObjectRef::Board],
    }
}

/// Reference findings: [`REFERENCE_LOST`] and [`REFERENCE_INEXACT`], one per
/// unresolved reference. A lost reference keeps the poses it gave (its
/// dimension stops driving) and blocks fabrication output.
pub fn checks(project: &Project) -> Vec<Finding> {
    let mut out = Vec::new();
    if project.part_dimensions.is_empty() {
        return out;
    }
    let ids = board_ids(project);
    for d in project.part_dimensions.iter() {
        // Whom the dimension holds: the first live target it names.
        let target = targets_of(d).find(|t| features::target_exists(project, *t));
        let who = target.map_or_else(|| "A part".into(), |t| target_words(project, t));
        for r in &d.refs {
            if reference_status(project, ids.as_ref(), r) != ReferenceStatus::Lost {
                continue;
            }
            let message = match r {
                BoardRef::Board(_) => {
                    format!("{who} is dimensioned to board geometry that was deleted")
                }
                BoardRef::Housing(_) => format!("{who} is dimensioned to a missing housing"),
                BoardRef::Part(f) if !features::target_exists(project, f.target()) => {
                    "A dimension names a part that is no longer on the board".into()
                }
                BoardRef::Part(FeatureRef::Pad { placement, .. }) => format!(
                    "{} is dimensioned by a pad it no longer has",
                    super::outline::part_name(project, *placement)
                ),
                BoardRef::Part(FeatureRef::Anchor { placement, name }) => format!(
                    "{} is dimensioned by anchor ‘{}’, which it no longer has",
                    super::outline::part_name(project, *placement),
                    name.0
                ),
                BoardRef::Part(f) => format!(
                    "{} is dimensioned by a feature it no longer has",
                    target_words(project, f.target())
                ),
            };
            let objects = target.map_or_else(|| vec![ObjectRef::Board], objects_of);
            out.push(Finding::new(REFERENCE_LOST, message, objects));
        }
    }
    out
}

/// A stable key for a target, for ordered maps.
fn key(t: ConstraintTarget) -> (u8, Id) {
    match t {
        ConstraintTarget::Placement(id) => (0, id),
        ConstraintTarget::Fill(id) => (1, id),
        ConstraintTarget::Keepout(id) => (2, id),
    }
}

/// One connected component of dimensioned targets: the targets (in first
/// mention order) and the live dimensions among them.
#[derive(Clone, Debug)]
struct Component {
    targets: Vec<ConstraintTarget>,
    dimensions: Vec<PartDimension>,
}

fn targets_of(d: &PartDimension) -> impl Iterator<Item = ConstraintTarget> + '_ {
    d.refs.iter().filter_map(|r| match r {
        BoardRef::Part(f) => Some(f.target()),
        _ => None,
    })
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while let Some(&p) = parent.get(i) {
        if p == i {
            break;
        }
        let grand = parent.get(p).copied().unwrap_or(p);
        if let Some(slot) = parent.get_mut(i) {
            *slot = grand;
        }
        i = p;
    }
    i
}

/// Group the live (fully resolving) dimensions into connected components:
/// two targets are connected when a dimension names both.
fn components(project: &Project) -> Vec<Component> {
    let ids = board_ids(project);
    let live: Vec<&PartDimension> = project
        .part_dimensions
        .iter()
        .filter(|d| resolves(project, ids.as_ref(), d))
        .collect();
    let mut index: BTreeMap<(u8, Id), usize> = BTreeMap::new();
    let mut targets: Vec<ConstraintTarget> = Vec::new();
    let mut parent: Vec<usize> = Vec::new();
    for d in &live {
        let mut first = None;
        for t in targets_of(d) {
            let i = *index.entry(key(t)).or_insert_with(|| {
                targets.push(t);
                parent.push(parent.len());
                parent.len() - 1
            });
            match first {
                None => first = Some(i),
                Some(f) => {
                    let (a, b) = (find(&mut parent, f), find(&mut parent, i));
                    if a != b
                        && let Some(slot) = parent.get_mut(b)
                    {
                        *slot = a;
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Component> = BTreeMap::new();
    for (i, t) in targets.iter().enumerate() {
        let root = find(&mut parent, i);
        groups
            .entry(root)
            .or_insert_with(|| Component {
                targets: Vec::new(),
                dimensions: Vec::new(),
            })
            .targets
            .push(*t);
    }
    for d in live {
        let Some(t) = targets_of(d).next() else {
            continue;
        };
        let Some(&i) = index.get(&key(t)) else {
            continue;
        };
        let root = find(&mut parent, i);
        if let Some(g) = groups.get_mut(&root) {
            g.dimensions.push(d.clone());
        }
    }
    groups.into_values().collect()
}

/// A target's pose on the board now and whether it is mirrored; `None`
/// when it is gone.
fn pose_of(project: &Project, target: ConstraintTarget) -> Option<(PartPose, bool)> {
    let at = |position: Point| PartPose {
        position,
        rotation_deg: 0.0,
    };
    match target {
        ConstraintTarget::Placement(id) => project.placement_of(id).map(|p| {
            (
                PartPose {
                    position: p.position,
                    rotation_deg: p.rotation_deg,
                },
                p.layer == crate::model::Layer::Bottom,
            )
        }),
        ConstraintTarget::Fill(id) => project.fill(id).map(|f| (at(f.outline.start), false)),
        ConstraintTarget::Keepout(id) => project.keepout(id).map(|k| (at(k.outline.start), false)),
    }
}

/// The solve inputs of a component's targets in `project`, with every
/// feature its dimensions name; `None` when a target or feature is gone.
fn solve_parts(project: &Project, c: &Component) -> Option<Vec<SolvePart>> {
    let mut parts = Vec::with_capacity(c.targets.len());
    for t in &c.targets {
        let (pose, mirrored) = pose_of(project, *t)?;
        let mut features: Vec<(FeatureRef, FeatureGeometry)> = Vec::new();
        for d in &c.dimensions {
            for r in &d.refs {
                if let BoardRef::Part(f) = r
                    && f.target() == *t
                    && !features.iter().any(|(g, _)| g == f)
                {
                    features.push((f.clone(), features::geometry(project, f).ok()?));
                }
            }
        }
        parts.push(SolvePart {
            target: *t,
            pose,
            mirrored,
            features,
        });
    }
    Some(parts)
}

/// Whether nothing a dimensioned pose depends on can have changed: the
/// board, the dimensions, the targets and the library they resolve in.
fn untouched(before: &Project, after: &Project) -> bool {
    // A shared section is unchanged when both handles point at the same
    // value (an edit copies the section it writes).
    fn same<T>(a: &T, b: &T) -> bool {
        std::ptr::eq(a, b)
    }
    let (a, b) = (&before.library, &after.library);
    before.board_size == after.board_size
        && before.board_shape == after.board_shape
        && same(&*before.part_dimensions, &*after.part_dimensions)
        && same(&*before.placements, &*after.placements)
        && same(&*before.instances, &*after.instances)
        && same(&*before.fills, &*after.fills)
        && same(&*before.keepouts, &*after.keepouts)
        && same(&*a.components, &*b.components)
        && same(&*a.packages, &*b.packages)
        && same(&*a.footprints, &*b.footprints)
        && same(&*a.bindings, &*b.bindings)
}

/// The board the parts solve holds fixed: the sketch, or the evaluator's
/// sketch of a rectangle board.
pub fn board_sketch<'a>(
    project: &'a Project,
    evaluator: &dyn super::outline::BoardShapeEvaluator,
) -> Result<Cow<'a, BoardSketch>> {
    Ok(match &project.board_shape {
        BoardShape::Sketch(s) => Cow::Borrowed(&**s),
        BoardShape::Rectangle => Cow::Owned(evaluator.rectangle(project.board_size)?),
    })
}

/// How far two poses may differ and count as the same, mm or degrees.
const SAME: f64 = 1e-9;

fn same_position(a: Point, b: Point) -> bool {
    let near = |x: f64, y: f64| (x - y).abs() <= SAME * x.abs().max(y.abs()).max(1.0);
    near(a[0], b[0]) && near(a[1], b[1])
}

fn same_angle(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d <= SAME || 360.0 - d <= SAME
}

fn same_pose(a: &PartPose, b: &PartPose) -> bool {
    same_position(a.position, b.position) && same_angle(a.rotation_deg, b.rotation_deg)
}

/// What solving one component found.
struct Solved {
    parts: Vec<SolvePart>,
    drags: Vec<PartDrag>,
    solution: PartsSolution,
}

/// Solve one component as `follow` does: each target starts at its pose in
/// `before`, and a target `project` moved from there is dragged to where it
/// was put.
fn solve_component(
    before: &Project,
    project: &Project,
    c: &Component,
    board: &BoardSketch,
    evaluator: &dyn super::outline::BoardShapeEvaluator,
) -> Result<Solved> {
    let mut parts = solve_parts(project, c).ok_or_else(|| {
        Error::NotFound("layout.dimension_reference: a dimensioned part is gone".into())
    })?;
    let mut drags = Vec::new();
    for p in &mut parts {
        if let Some((stored, _)) = pose_of(before, p.target)
            && !same_pose(&stored, &p.pose)
        {
            drags.push(PartDrag {
                target: p.target,
                to: p.pose,
            });
            p.pose = stored;
        }
    }
    let solution = evaluator.solve_parts(&PartsRequest {
        board,
        parts: &parts,
        dimensions: &c.dimensions,
        drags: &drags,
    })?;
    if matches!(
        solution.status,
        SolveStatus::OverConstrained | SolveStatus::Failed
    ) {
        return Err(Error::Conflict(conflict_message(
            project,
            board,
            &c.dimensions,
            &solution,
        )));
    }
    Ok(Solved {
        parts,
        drags,
        solution,
    })
}

/// Solve the dimensioned parts an operation touched (after the board
/// solve), taking the project from `before` to `project`. A part that a
/// command moved away from where its dimensions put it is dragged by the
/// solver: it slides as far as its free degrees of freedom let it. A move
/// that cannot land (the part cannot move that way at all) is refused in
/// [`held_message`]'s words, and dimensions that do not solve together
/// refuse the operation.
pub fn follow(
    before: &Project,
    project: &mut Project,
    evaluator: Option<&dyn super::outline::BoardShapeEvaluator>,
) -> Result<()> {
    if project.part_dimensions.is_empty() || untouched(before, project) {
        return Ok(());
    }
    let board_changed =
        before.board_size != project.board_size || before.board_shape != project.board_shape;
    let mut touched = Vec::new();
    for c in components(project) {
        if !board_changed {
            let same_dims = c.dimensions.iter().all(|d| {
                before
                    .part_dimensions
                    .iter()
                    .any(|b| b.id == d.id && b == d)
            });
            let parts_now = solve_parts(project, &c);
            if same_dims && parts_now.is_some() && solve_parts(before, &c) == parts_now {
                continue;
            }
        }
        touched.push(c);
    }
    if touched.is_empty() {
        return Ok(());
    }
    let evaluator = evaluator.ok_or_else(needs_evaluator)?;
    let board = board_sketch(project, evaluator)?.into_owned();
    for c in touched {
        let Solved {
            parts,
            drags,
            solution,
        } = solve_component(before, project, &c, &board, evaluator)?;
        for drag in &drags {
            let stored = parts
                .iter()
                .find(|p| p.target == drag.target)
                .map(|p| p.pose);
            let solved = solution
                .poses
                .iter()
                .find(|(t, _)| *t == drag.target)
                .map(|(_, p)| *p);
            let (Some(stored), Some(solved)) = (stored, solved) else {
                continue;
            };
            let moved = !same_position(drag.to.position, stored.position);
            let turned = !same_angle(drag.to.rotation_deg, stored.rotation_deg);
            let verb = if moved && same_position(solved.position, stored.position) {
                Some("move")
            } else if turned && same_angle(solved.rotation_deg, stored.rotation_deg) {
                Some("turn")
            } else {
                None
            };
            if let Some(verb) = verb {
                let at = held_at(project, &board, drag.target, &solution.measured);
                return Err(Error::Conflict(held_message(
                    &target_words(project, drag.target),
                    at.as_deref(),
                    verb,
                )));
            }
        }
        for (target, pose) in &solution.poses {
            place(project, *target, pose)?;
        }
    }
    Ok(())
}

/// Put a target at a solved pose (a fill or keepout only translates).
fn place(project: &mut Project, target: ConstraintTarget, pose: &PartPose) -> Result<()> {
    if !pose.valid() {
        return Err(Error::Invalid(
            "layout.dimension: the dimensions put a part out of range".into(),
        ));
    }
    match target {
        ConstraintTarget::Placement(id) => {
            let changed = project.placement_of(id).is_some_and(|p| {
                p.position != pose.position || p.rotation_deg != pose.rotation_deg
            });
            if changed && let Some(p) = project.placements.iter_mut().find(|p| p.instance == id) {
                p.position = pose.position;
                p.rotation_deg = pose.rotation_deg;
            }
        }
        ConstraintTarget::Fill(id) => {
            let changed = project
                .fill(id)
                .is_some_and(|f| f.outline.start != pose.position);
            if changed && let Some(f) = project.fills.iter_mut().find(|f| f.id == id) {
                translate(&mut f.outline, pose.position)?;
            }
        }
        ConstraintTarget::Keepout(id) => {
            let changed = project
                .keepout(id)
                .is_some_and(|k| k.outline.start != pose.position);
            if changed && let Some(k) = project.keepouts.iter_mut().find(|k| k.id == id) {
                translate(&mut k.outline, pose.position)?;
            }
        }
    }
    Ok(())
}

/// Move an outline so it starts at `to`.
fn translate(outline: &mut Outline, to: Point) -> Result<()> {
    let d = [to[0] - outline.start[0], to[1] - outline.start[1]];
    let shift = |p: Point| [p[0] + d[0], p[1] + d[1]];
    outline.start = to;
    for s in &mut outline.segments {
        match s {
            Segment::Line { to } | Segment::Arc { to, .. } => *to = shift(*to),
        }
    }
    if outline.segments.iter().all(|s| point_valid(s.to())) {
        Ok(())
    } else {
        Err(Error::Invalid(
            "layout.dimension: the dimensions put an outline out of range".into(),
        ))
    }
}

/// The component holding `target`, if a live dimension names it.
fn component_of(project: &Project, target: ConstraintTarget) -> Option<Component> {
    components(project)
        .into_iter()
        .find(|c| c.targets.contains(&target))
}

/// Which of a dimensioned target's degrees of freedom its dimensions leave
/// free, and the solve's measured values; `None` for a target no live
/// dimension names.
pub fn freedom(
    project: &Project,
    target: ConstraintTarget,
    evaluator: &dyn super::outline::BoardShapeEvaluator,
) -> Option<Result<(PartFreedom, Measured)>> {
    let c = component_of(project, target)?;
    let solve = || -> Result<(PartFreedom, Measured)> {
        let board = board_sketch(project, evaluator)?;
        let solved = solve_component(project, project, &c, &board, evaluator)?;
        let free = solved
            .solution
            .freedom
            .iter()
            .find(|(t, _)| *t == target)
            .map(|(_, f)| *f)
            .unwrap_or_default();
        Ok((free, solved.solution.measured))
    };
    Some(solve())
}

/// Where a drag of `instance` to `to` lands: the pose the engine's
/// `layout.move` would leave it at (a dimensioned part slides along what
/// holds it), for a canvas to preview. `None` for a part no live dimension
/// names (it lands where it is put).
pub fn landing(
    project: &Project,
    instance: Id,
    to: PartPose,
    evaluator: &dyn super::outline::BoardShapeEvaluator,
) -> Option<Result<PartPose>> {
    let target = ConstraintTarget::Placement(instance);
    let c = component_of(project, target)?;
    let mut moved = project.clone();
    if let Some(p) = moved.placements.iter_mut().find(|p| p.instance == instance) {
        p.position = to.position;
        p.rotation_deg = to.rotation_deg;
    }
    let solve = || -> Result<PartPose> {
        let board = board_sketch(project, evaluator)?;
        let solved = solve_component(project, &moved, &c, &board, evaluator)?;
        solved
            .solution
            .poses
            .iter()
            .find(|(t, _)| *t == target)
            .map(|(_, p)| *p)
            .ok_or_else(|| Error::NotFound("layout.dimension: the part was not solved".into()))
    };
    Some(solve())
}

/// Why no operation may move `instance`: its dimensions fix its position
/// (in [`held_message`]'s words). `None` for a part that can still move:
/// free, or dimensioned with a position left free (a drag slides it).
/// Without an evaluator a dimensioned part cannot be solved, so cannot
/// move.
pub fn held(
    project: &Project,
    instance: Id,
    evaluator: Option<&dyn super::outline::BoardShapeEvaluator>,
) -> Option<Error> {
    let target = ConstraintTarget::Placement(instance);
    let Some(evaluator) = evaluator else {
        return component_of(project, target).map(|_| needs_evaluator());
    };
    match freedom(project, target, evaluator)? {
        Ok((free, measured)) if !free.x && !free.y => {
            let at = board_sketch(project, evaluator)
                .ok()
                .and_then(|board| held_at(project, &board, target, &measured));
            Some(Error::Conflict(held_message(
                &target_words(project, target),
                at.as_deref(),
                "move",
            )))
        }
        Ok(_) => None,
        Err(e) => Some(e),
    }
}

/// The dimensions naming `target`, live or not.
pub fn dimensions_of(project: &Project, target: ConstraintTarget) -> Vec<&PartDimension> {
    project
        .part_dimensions
        .iter()
        .filter(|d| targets_of(d).any(|t| t == target))
        .collect()
}

/// How many objects dimensions hold: all of them, or those dimensioned to
/// one board curve.
pub fn dimensioned(project: &Project, to: Option<&SketchCurveId>) -> usize {
    let mut targets = BTreeSet::new();
    for d in project.part_dimensions.iter() {
        let names = |c: &SketchCurveId| {
            d.refs
                .iter()
                .any(|r| matches!(r, BoardRef::Board(SketchRef::Curve(k)) if k == c))
        };
        if to.is_none_or(names) {
            targets.extend(targets_of(d).map(key));
        }
    }
    targets.len()
}

/// "the right edge" for the edge named `right`; a name that already says
/// "edge" is not doubled.
pub fn edge_words(name: &str) -> String {
    let name = name.trim();
    if name.to_lowercase().ends_with("edge") {
        format!("the {name}")
    } else {
        format!("the {name} edge")
    }
}

/// The named board edge a curve is, if any.
pub fn edge_name<'a>(board: &'a BoardSketch, curve: &SketchCurveId) -> Option<&'a str> {
    board
        .edges
        .iter()
        .find(|e| &e.curve == curve)
        .map(|e| e.name.0.as_str())
}

/// Where a part's dimensions hold it, as words, from its distances to
/// named board edges: "4 mm from the right edge", "on the left edge", "4 mm
/// from the right edge and 10 mm from the bottom edge". `measured` gives
/// the solved distances (mm); a distance it lacks is read from a plain
/// value, or the words say "to the right edge". `None` when no distance
/// names a board edge.
pub fn held_at(
    project: &Project,
    board: &BoardSketch,
    target: ConstraintTarget,
    measured: &[(Id, f64)],
) -> Option<String> {
    let mut words = Vec::new();
    for d in dimensions_of(project, target) {
        if d.reference || !matches!(d.kind, PartRelation::Dim(DimKind::Distance)) {
            continue;
        }
        let Some(edge) = d.refs.iter().find_map(|r| match r {
            BoardRef::Board(SketchRef::Curve(c)) => edge_name(board, c),
            _ => None,
        }) else {
            continue;
        };
        let value = measured
            .iter()
            .find(|(id, _)| *id == d.id)
            .map(|(_, v)| *v)
            .or(match &d.value {
                Some(DimValue::Fixed(v)) => Some(*v),
                _ => None,
            });
        let edge = edge_words(edge);
        words.push(match value {
            Some(v) if v.abs() <= 5e-4 => format!("on {edge}"),
            Some(v) => format!("{} mm from {edge}", super::limits::mm(v.abs())),
            None => format!("to {edge}"),
        });
    }
    match words.as_slice() {
        [] => None,
        [one] => Some(one.clone()),
        [a, b, ..] => Some(format!("{a} and {b}")),
    }
}

/// The refusal for a part its dimensions hold, as one message line: "R1 is
/// held 4 mm from the right edge; edit or delete its dimensions to move
/// it". `at` is [`held_at`]'s words; `verb` is what was refused ("move",
/// "turn"). Too long a line drops where it is held, then the name, so the
/// way out always shows.
pub fn held_message(name: &str, at: Option<&str>, verb: &str) -> String {
    let mut lines = Vec::new();
    if let Some(at) = at {
        lines.push(format!(
            "{name} is held {at}; edit or delete its dimensions to {verb} it"
        ));
    }
    let way_out = format!("edit or delete one to {verb} it");
    lines.push(format!("{name} is held by its dimensions; {way_out}"));
    lines.push(format!("It is held by its dimensions; {way_out}"));
    lines
        .into_iter()
        .find(|m| m.chars().count() <= crate::diagnostics::MAX_LINE)
        .unwrap_or_else(|| format!("Held by its dimensions; edit or delete one to {verb} it"))
}

/// A reference in words, for a refusal.
fn reference_words(project: &Project, board: &BoardSketch, r: &BoardRef) -> String {
    match r {
        BoardRef::Board(SketchRef::Curve(c)) => {
            edge_name(board, c).map_or_else(|| "a board curve".into(), edge_words)
        }
        BoardRef::Board(_) => "a board point".into(),
        BoardRef::Housing(_) => "the housing".into(),
        BoardRef::Part(f) => {
            let name = target_words(project, f.target());
            match f {
                FeatureRef::Origin { .. } => name,
                FeatureRef::Axis { axis, .. } => format!(
                    "{name}'s {} axis",
                    match axis {
                        PartAxis::X => "x",
                        PartAxis::Y => "y",
                    }
                ),
                FeatureRef::Pad { placement, pad, .. } => {
                    let label = super::pad_labels(project)
                        .get(&(*placement, *pad))
                        .cloned()
                        .unwrap_or_else(|| pad.0.to_string());
                    format!("{name} pad {label}")
                }
                FeatureRef::Courtyard { .. } => format!("{name}'s courtyard"),
                FeatureRef::Anchor { name: anchor, .. } => format!("{name}'s ‘{}’", anchor.0),
                FeatureRef::Face { .. } | FeatureRef::Edge { .. } | FeatureRef::Vertex { .. } => {
                    format!("{name}'s 3D model")
                }
            }
        }
    }
}

/// "R1 to the right edge (4)".
fn dimension_words(project: &Project, board: &BoardSketch, d: &PartDimension) -> String {
    let what = d
        .refs
        .iter()
        .map(|r| reference_words(project, board, r))
        .collect::<Vec<_>>()
        .join(" to ");
    match &d.value {
        Some(DimValue::Fixed(v)) => format!("{what} ({})", super::limits::mm(*v)),
        Some(DimValue::Param(p)) => format!("{what} ({})", p.0),
        Some(DimValue::Expr(e)) => format!("{what} ({})", e.0),
        None => what,
    }
}

/// Why a component's dimensions do not solve, naming the ones that
/// conflict.
fn conflict_message(
    project: &Project,
    board: &BoardSketch,
    dimensions: &[PartDimension],
    solution: &PartsSolution,
) -> String {
    let named: Vec<String> = dimensions
        .iter()
        .filter(|d| solution.failing.contains(&d.id))
        .take(3)
        .map(|d| dimension_words(project, board, d))
        .collect();
    let why = solution
        .findings
        .first()
        .map(|f| f.message.clone())
        .unwrap_or_default();
    match named.as_slice() {
        [] if why.is_empty() => {
            "layout.dimension_conflict: these dimensions cannot all hold".into()
        }
        [] => format!("layout.dimension_conflict: {why}"),
        [one] => format!("layout.dimension_conflict: {one} cannot hold"),
        many => format!(
            "layout.dimension_conflict: {} cannot all hold",
            many.join(" and ")
        ),
    }
}

/// Structural bounds: at most [`MAX_PART_DIMENSIONS`], 1–
/// [`MAX_DIMENSION_REFS`] references each, finite values and picks,
/// well-formed names and expressions. A reference whose source is gone is a
/// check finding ([`REFERENCE_LOST`]), not an invalid document.
pub fn validate(project: &Project) -> Result<()> {
    if project.part_dimensions.len() > MAX_PART_DIMENSIONS {
        return Err(Error::Limit(format!(
            "layout.dimension_limit: at most {MAX_PART_DIMENSIONS} part dimensions"
        )));
    }
    let mut ids = BTreeSet::new();
    for d in project.part_dimensions.iter() {
        if !ids.insert(d.id) || !well_formed(d) {
            return Err(Error::Invalid(format!(
                "layout.dimension: part dimension {} needs 1–{MAX_DIMENSION_REFS} well-formed \
                 references and a finite value",
                d.id
            )));
        }
    }
    Ok(())
}

fn well_formed(d: &PartDimension) -> bool {
    let value = match &d.value {
        None => true,
        Some(DimValue::Fixed(v)) => v.is_finite(),
        Some(DimValue::Param(name)) => name.valid(),
        Some(DimValue::Expr(expr)) => expr.valid(),
    };
    let kind = match d.kind {
        PartRelation::PointAlong { at } => at.is_finite() && (0.0..=1.0).contains(&at),
        PartRelation::Dim(_) | PartRelation::Geom(_) => true,
    };
    value
        && kind
        && (1..=MAX_DIMENSION_REFS).contains(&d.refs.len())
        && d.refs.iter().all(reference_well_formed)
}

fn reference_well_formed(r: &BoardRef) -> bool {
    let pick = |p: &[f64; 3]| p.iter().all(|v| v.is_finite());
    match r {
        BoardRef::Board(_) | BoardRef::Housing(_) => true,
        BoardRef::Part(f) => match f {
            FeatureRef::Origin { .. }
            | FeatureRef::Axis { .. }
            | FeatureRef::Pad { .. }
            | FeatureRef::Courtyard { .. } => true,
            FeatureRef::Anchor { name, .. } => name.valid(),
            FeatureRef::Face { face, .. } => {
                face.body.valid() && face.face.valid() && pick(&face.pick)
            }
            FeatureRef::Edge { edge, .. } | FeatureRef::Vertex { edge, .. } => {
                edge.body.valid() && edge.edge.valid() && pick(&edge.pick)
            }
        },
    }
}

/// Each dimension's measured value (mm or degrees), by id.
pub type Measured = Vec<(Id, f64)>;

/// A target's pose on the board: what the parts solve finds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartPose {
    pub position: Point,
    pub rotation_deg: f64,
}

impl PartPose {
    pub fn valid(&self) -> bool {
        point_valid(self.position) && self.rotation_deg.is_finite()
    }
}

/// A feature's geometry in its target's frame (part frame, before the
/// placement mirrors and turns it), mm.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "shape", deny_unknown_fields)]
pub enum FeatureGeometry {
    Point {
        at: Point,
    },
    Line {
        a: Point,
        b: Point,
    },
    /// A bulge arc from `a` to `b` (as `library::Segment::Arc`).
    Arc {
        a: Point,
        b: Point,
        bulge: f64,
    },
    Circle {
        center: Point,
        radius: f64,
    },
}

/// One target of a parts solve, with every feature its dimensions name.
#[derive(Clone, Debug, PartialEq)]
pub struct SolvePart {
    pub target: ConstraintTarget,
    /// The stored pose: the start, and what an unconstrained degree of
    /// freedom keeps.
    pub pose: PartPose,
    /// On the bottom side: the part frame's y is mirrored.
    pub mirrored: bool,
    pub features: Vec<(FeatureRef, FeatureGeometry)>,
}

/// A command's move of a dimensioned target: the solver drags it toward
/// `to`, and its free degrees of freedom take `to`'s values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartDrag {
    pub target: ConstraintTarget,
    pub to: PartPose,
}

/// One connected component of dimensioned targets to solve.
#[derive(Clone, Copy, Debug)]
pub struct PartsRequest<'a> {
    /// The board after its solve; a rectangle board as the evaluator's
    /// rectangle sketch. Its points and curves are held fixed.
    pub board: &'a BoardSketch,
    pub parts: &'a [SolvePart],
    /// The dimensions among them, with their values unevaluated (the host
    /// evaluates expressions against the board parameters). Only resolving
    /// ones: lost references never reach the solve.
    pub dimensions: &'a [PartDimension],
    /// The targets the operation moved.
    pub drags: &'a [PartDrag],
}

/// Which of a target's degrees of freedom its dimensions leave free (in
/// board axes; on a slanted edge both x and y read free).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PartFreedom {
    pub x: bool,
    pub y: bool,
    pub rotation: bool,
}

/// What a parts solve found.
#[derive(Clone, Debug, PartialEq)]
pub struct PartsSolution {
    /// One pose per requested target.
    pub poses: Vec<(ConstraintTarget, PartPose)>,
    /// Each dimension's measured value (mm or degrees), reference ones
    /// included.
    pub measured: Vec<(Id, f64)>,
    pub status: SolveStatus,
    /// Per target, what its dimensions leave free.
    pub freedom: Vec<(ConstraintTarget, PartFreedom)>,
    /// The dimensions a failed solve could not satisfy.
    pub failing: Vec<Id>,
    pub findings: Vec<Finding>,
}

#[cfg(test)]
mod tests;
