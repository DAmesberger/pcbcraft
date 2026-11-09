//! Edge constraints: placements, fills and keepouts held relative to named
//! board edges, resolved after every engine operation ([`follow`]).
//!
//! Slice S2 replaces all of this with part dimensions
//! (`layout::dimensions`, docs/board-sketch-references.md §5.6) and deletes
//! this module with `model::board::EdgeConstraint`.
use super::{BoardEvaluation, BoardShapeEvaluator, part_name};
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::library::Outline;
use crate::library::footprint_source::Expr;
use crate::model::board::{
    ConstraintAlign, ConstraintTarget, EdgeAnchor, EdgeConstraintSpec, EdgeName,
    MAX_EDGE_CONSTRAINTS, SolvedEdge, arc_geometry,
};
use crate::model::{BoardShape, EdgeConstraint, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Finding code: a constraint whose edge or target is gone.
pub const CONSTRAINT_UNRESOLVED: &str = "board.constraint_unresolved";

/// How far a resolved pose may differ from the stored one and still count
/// as the same, mm or degrees.
const SAME: f64 = 1e-9;
/// Chord tolerance for alignment extents, mm.
const EXTENT_CHORD_MM: f64 = 0.01;

/// The solved values of one constraint's expressions, mm.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ConstraintValues {
    pub constraint: Id,
    pub along: f64,
    pub offset: f64,
}

fn missing(id: Id) -> Error {
    Error::NotFound(format!("board.constraint_missing: no edge constraint {id}"))
}

fn spec_of(c: &EdgeConstraint) -> EdgeConstraintSpec {
    EdgeConstraintSpec {
        target: c.target,
        edge: c.edge.clone(),
        anchor: c.anchor,
        along: c.along.clone(),
        offset: c.offset.clone(),
        align: c.align,
        follow_edge: c.follow_edge,
        rotation_deg: c.rotation_deg,
    }
}

/// A new or edited constraint names a live target and edge, and asks only
/// for what its target supports.
fn check_spec(project: &Project, c: &EdgeConstraintSpec) -> Result<()> {
    match c.target {
        ConstraintTarget::Placement(id) => {
            if !project.placements.iter().any(|p| p.instance == id) {
                return Err(Error::NotFound(format!(
                    "board.target_missing: instance {id} is not placed on the board"
                )));
            }
        }
        ConstraintTarget::Fill(id) | ConstraintTarget::Keepout(id) => {
            let fill = matches!(c.target, ConstraintTarget::Fill(_));
            let exists = if fill {
                project.fills.iter().any(|f| f.id == id)
            } else {
                project.keepouts.iter().any(|k| k.id == id)
            };
            if !exists {
                return Err(Error::NotFound(format!(
                    "board.target_missing: no {} {id}",
                    if fill { "fill" } else { "keepout" }
                )));
            }
            if c.follow_edge || c.rotation_deg != 0.0 {
                return Err(Error::Invalid(
                    "board.constraint_rotation: fills and keepouts keep their orientation; \
                     only placements follow an edge's direction"
                        .into(),
                ));
            }
        }
    }
    if !c.edge.valid() {
        return Err(Error::Invalid(format!(
            "board.edge_name: `{}` is not an edge name",
            c.edge.0
        )));
    }
    if project.board().edge(&c.edge).is_none() {
        return Err(Error::NotFound(format!(
            "board.edge_missing: the board has no edge named `{}`",
            c.edge.0
        )));
    }
    anchor_valid(c.anchor)?;
    if !(c.along.valid() && c.offset.valid() && c.rotation_deg.is_finite()) {
        return Err(Error::Invalid(
            "board.constraint: `along` and `offset` need expressions and the rotation a number"
                .into(),
        ));
    }
    Ok(())
}

fn anchor_valid(anchor: EdgeAnchor) -> Result<()> {
    match anchor {
        EdgeAnchor::Fraction(f) if !(0.0..=1.0).contains(&f) => {
            Err(Error::Invalid("board.anchor: a fraction is 0–1".into()))
        }
        _ => Ok(()),
    }
}

/// Every constraint's expressions evaluated, or the first that did not.
pub(super) fn expressions_evaluated(project: &Project, evaluation: &BoardEvaluation) -> Result<()> {
    let have: BTreeSet<Id> = evaluation
        .constraints
        .iter()
        .map(|v| v.constraint)
        .collect();
    if let Some(c) = project
        .edge_constraints
        .iter()
        .find(|c| !have.contains(&c.id))
    {
        let why = evaluation
            .findings
            .iter()
            .find(|f| f.message.contains(&format!("constraint {}", c.id)))
            .map_or_else(String::new, |f| format!(": {}", f.message));
        return Err(Error::Invalid(format!(
            "board.constraint_expr: edge constraint {} does not evaluate{why}",
            c.id
        )));
    }
    Ok(())
}

/// A constrained position and direction.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EdgePose {
    pub position: Point,
    /// The edge's direction at that point, degrees.
    pub edge_deg: f64,
}

/// The point `along` mm from `anchor` on `edge` and `offset` mm along its
/// inward (left) normal. On an arc edge `along` is arc length and the
/// direction is the tangent there.
pub fn edge_pose(
    edge: &SolvedEdge,
    anchor: EdgeAnchor,
    along: f64,
    offset: f64,
) -> Result<EdgePose> {
    if !(along.is_finite() && offset.is_finite()) {
        return Err(Error::Invalid(
            "board.edge: degenerate edge or value".into(),
        ));
    }
    let arc = if edge.bulge == 0.0 {
        None
    } else {
        Some(
            arc_geometry(edge.a, edge.b, edge.bulge)
                .ok_or_else(|| Error::Invalid("board.edge: degenerate arc edge".into()))?,
        )
    };
    let len = match &arc {
        Some(arc) => arc.length(),
        None => (edge.b[0] - edge.a[0]).hypot(edge.b[1] - edge.a[1]),
    };
    if !(len.is_finite() && len > 1e-9) {
        return Err(Error::Invalid(
            "board.edge: degenerate edge or value".into(),
        ));
    }
    anchor_valid(anchor)?;
    let start = match anchor {
        EdgeAnchor::Start => 0.0,
        EdgeAnchor::Middle => len / 2.0,
        EdgeAnchor::End => len,
        EdgeAnchor::Fraction(f) => f * len,
    };
    let s = start + along;
    let (point, u) = match &arc {
        None => {
            let u = [(edge.b[0] - edge.a[0]) / len, (edge.b[1] - edge.a[1]) / len];
            ([edge.a[0] + u[0] * s, edge.a[1] + u[1] * s], u)
        }
        Some(arc) => {
            let turn = arc.sweep.signum();
            let angle = arc.start + turn * s / arc.radius;
            let (sin, cos) = angle.sin_cos();
            (arc.at(angle), [-sin * turn, cos * turn])
        }
    };
    let position = [point[0] - u[1] * offset, point[1] + u[0] * offset];
    if !point_valid(position) {
        return Err(Error::Invalid("board.edge: position out of range".into()));
    }
    Ok(EdgePose {
        position,
        edge_deg: u[1].atan2(u[0]).to_degrees(),
    })
}

/// A plain length in mm (`12.5`, `-3 mm`), which the core reads without
/// the host's evaluator.
pub(super) fn plain_mm(expr: &Expr) -> Option<f64> {
    let text = expr.0.trim();
    let number = text.strip_suffix("mm").map_or(text, str::trim_end);
    if number.is_empty()
        || !number
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
    {
        return None;
    }
    number.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Every constraint's (along, offset) in mm: plain lengths read in core,
/// anything else through the host's evaluator.
fn values(
    project: &Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Result<BTreeMap<Id, (f64, f64)>> {
    let plain: Option<BTreeMap<Id, (f64, f64)>> = project
        .edge_constraints
        .iter()
        .map(|c| Some((c.id, (plain_mm(&c.along)?, plain_mm(&c.offset)?))))
        .collect();
    if let Some(plain) = plain {
        return Ok(plain);
    }
    let evaluator = evaluator.ok_or_else(|| {
        Error::Unsupported(
            "board.evaluator: edge constraints with expressions need the host's sketch evaluator"
                .into(),
        )
    })?;
    let board = match &project.board_shape {
        BoardShape::Rectangle => evaluator.rectangle(project.board_size)?,
        BoardShape::Sketch(sketch) => (**sketch).clone(),
    };
    let evaluation = evaluator.evaluate(&board, &project.edge_constraints)?;
    expressions_evaluated(project, &evaluation)?;
    Ok(evaluation
        .constraints
        .iter()
        .map(|v| (v.constraint, (v.along, v.offset)))
        .collect())
}

/// What each constrained target's pose depends on, to skip resolution
/// when an operation touched none of it.
fn inputs_changed(before: &Project, after: &Project) -> bool {
    if before.board_size != after.board_size
        || before.board_shape != after.board_shape
        || before.edge_constraints != after.edge_constraints
    {
        return true;
    }
    after.edge_constraints.iter().any(|c| match c.target {
        ConstraintTarget::Placement(id) => {
            let a = before.placements.iter().find(|p| p.instance == id);
            let b = after.placements.iter().find(|p| p.instance == id);
            a != b
        }
        ConstraintTarget::Fill(id) => {
            before.fills.iter().find(|f| f.id == id).map(|f| &f.outline)
                != after.fills.iter().find(|f| f.id == id).map(|f| &f.outline)
        }
        ConstraintTarget::Keepout(id) => {
            before
                .keepouts
                .iter()
                .find(|k| k.id == id)
                .map(|k| &k.outline)
                != after
                    .keepouts
                    .iter()
                    .find(|k| k.id == id)
                    .map(|k| &k.outline)
        }
    })
}

/// Unit direction of an edge pose and its inward normal.
fn frame(pose: &EdgePose) -> (Point, Point) {
    let (sin, cos) = pose.edge_deg.to_radians().sin_cos();
    ([cos, sin], [-sin, cos])
}

/// Where the reference point (`align`) of points `pts`, given relative to
/// the target's origin, lies relative to that origin.
fn reference(pts: &[Point], align: ConstraintAlign, u: Point, n: Point) -> Point {
    if align == ConstraintAlign::Origin || pts.is_empty() {
        return [0.0, 0.0];
    }
    let dot = |p: &Point, d: Point| p[0] * d[0] + p[1] * d[1];
    let (mut s0, mut s1, mut t0, mut t1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in pts {
        let (s, t) = (dot(p, u), dot(p, n));
        s0 = s0.min(s);
        s1 = s1.max(s);
        t0 = t0.min(t);
        t1 = t1.max(t);
    }
    let s = (s0 + s1) / 2.0;
    let t = match align {
        ConstraintAlign::NearSide => t0,
        _ => (t0 + t1) / 2.0,
    };
    [u[0] * s + n[0] * t, u[1] * s + n[1] * t]
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= SAME * a.abs().max(b.abs()).max(1.0)
}

fn same_angle(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d <= 1e-9 || 360.0 - d <= 1e-9
}

/// Place every constrained target where its constraint puts it, after an
/// operation took the project from `before` to `project`. A constraint
/// whose edge or target is gone is left for [`checks`]. Refuses, in the
/// words of [`held_message`], an operation that moved a constrained part
/// away from its constrained pose, and expressions that do not evaluate.
pub fn follow(
    before: &Project,
    project: &mut Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    if project.edge_constraints.is_empty() || !inputs_changed(before, project) {
        return Ok(());
    }
    let values = values(project, evaluator)?;
    let board = project.board().into_owned();
    // Poses first, with every followed rotation, so alignment extents are
    // taken at the final orientation.
    let mut poses = Vec::new();
    for c in &project.edge_constraints {
        let (Some(edge), Some(&(along, offset))) = (board.edge(&c.edge), values.get(&c.id)) else {
            continue;
        };
        let pose = edge_pose(edge, c.anchor, along, offset).map_err(|e| {
            Error::Invalid(format!("board.constraint: edge constraint {}: {e}", c.id))
        })?;
        poses.push((c.clone(), pose));
    }
    // The rotations the operation left, before any follows its edge.
    let commanded: BTreeMap<Id, f64> = project
        .placements
        .iter()
        .map(|p| (p.instance, p.rotation_deg))
        .collect();
    for (c, pose) in &poses {
        if let ConstraintTarget::Placement(id) = c.target
            && c.follow_edge
            && let Some(p) = project.placements.iter_mut().find(|p| p.instance == id)
        {
            let rotation = (pose.edge_deg + c.rotation_deg).rem_euclid(360.0);
            if !same_angle(p.rotation_deg, rotation) {
                p.rotation_deg = rotation;
            }
        }
    }
    let aligned = poses.iter().any(|(c, _)| {
        matches!(c.target, ConstraintTarget::Placement(_)) && c.align != ConstraintAlign::Origin
    });
    let extents = if aligned {
        placement_extents(project)
    } else {
        BTreeMap::new()
    };
    for (c, pose) in &poses {
        let (u, n) = frame(pose);
        match c.target {
            ConstraintTarget::Placement(id) => {
                let Some(index) = project.placements.iter().position(|p| p.instance == id) else {
                    continue;
                };
                let Some(p) = project.placements.get_mut(index) else {
                    continue;
                };
                let local: Vec<Point> = extents
                    .get(&id)
                    .map(|pts| {
                        pts.iter()
                            .map(|q| [q[0] - p.position[0], q[1] - p.position[1]])
                            .collect()
                    })
                    .unwrap_or_default();
                let r = reference(&local, c.align, u, n);
                let position = [pose.position[0] - r[0], pose.position[1] - r[1]];
                if !point_valid(position) {
                    return Err(Error::Invalid(format!(
                        "board.constraint: edge constraint {} puts its part out of range",
                        c.id
                    )));
                }
                let old = before.placements.iter().find(|q| q.instance == id);
                let commanded = commanded.get(&id).copied().unwrap_or(p.rotation_deg);
                let moved = old.is_some_and(|q| q.position != p.position);
                let turned = old.is_some_and(|q| q.rotation_deg != commanded);
                let lands = near(position[0], p.position[0]) && near(position[1], p.position[1]);
                if moved && !lands
                    || c.follow_edge
                        && turned
                        && !same_angle(
                            (pose.edge_deg + c.rotation_deg).rem_euclid(360.0),
                            commanded,
                        )
                {
                    let verb = if moved && !lands { "move" } else { "turn" };
                    let offset = values.get(&c.id).map(|v| v.1);
                    return Err(Error::Conflict(held_message(
                        &part_name(project, id),
                        &c.edge,
                        offset,
                        verb,
                    )));
                }
                if !lands {
                    p.position = position;
                }
            }
            ConstraintTarget::Fill(id) => {
                if let Some(f) = project.fills.iter_mut().find(|f| f.id == id) {
                    place_outline(&mut f.outline, c.align, pose, u, n)?;
                }
            }
            ConstraintTarget::Keepout(id) => {
                if let Some(k) = project.keepouts.iter_mut().find(|k| k.id == id) {
                    place_outline(&mut k.outline, c.align, pose, u, n)?;
                }
            }
        }
    }
    Ok(())
}

/// "the right edge" for the edge named `right`; a name that already says
/// "edge" is not doubled.
fn edge_words(edge: &EdgeName) -> String {
    let name = edge.0.trim();
    if name.to_lowercase().ends_with("edge") {
        format!("the {name}")
    } else {
        format!("the {name} edge")
    }
}

/// Where an edge constraint holds a part, as words: "2 mm from the right
/// edge", "on the right edge", "1.5 mm past the right edge", or "to the
/// right edge" when `offset` (the solved distance off the edge, mm) is not
/// known.
pub fn held_at(edge: &EdgeName, offset: Option<f64>) -> String {
    let edge = edge_words(edge);
    match offset {
        Some(o) if o.abs() <= 5e-4 => format!("on {edge}"),
        Some(o) if o < 0.0 => format!("{} mm past {edge}", crate::layout::limits::mm(-o)),
        Some(o) => format!("{} mm from {edge}", crate::layout::limits::mm(o)),
        None => format!("to {edge}"),
    }
}

/// The refusal for a part an edge constraint holds, as one message line:
/// "J1 is held 2 mm from the right edge; edit or remove its constraint to
/// move it". `offset` is the constraint's solved distance off the edge;
/// `verb` is what was refused ("move", "turn"). Too long a line drops the
/// distance, then the edge, so the way out always shows.
pub fn held_message(name: &str, edge: &EdgeName, offset: Option<f64>, verb: &str) -> String {
    let way_out = format!("edit or remove its constraint to {verb} it");
    [
        format!("{name} is held {}; {way_out}", held_at(edge, offset)),
        format!("{name} is held {}; {way_out}", held_at(edge, None)),
        format!("{name} is held to a board edge; {way_out}"),
        format!("It is held to a board edge; {way_out}"),
    ]
    .into_iter()
    .find(|m| m.chars().count() <= crate::diagnostics::MAX_LINE)
    .unwrap_or_else(|| format!("Held to a board edge; {way_out}"))
}

/// Why no operation may move or turn `instance` away from where an edge
/// constraint puts it, in the words [`follow`] refuses with; `None` for a
/// part no resolvable constraint holds. The distance shows when the
/// constraint's expressions evaluate (plain lengths always do; expressions
/// need `evaluator`).
pub fn held(
    project: &Project,
    instance: Id,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Option<Error> {
    let c = project
        .edge_constraints
        .iter()
        .find(|c| c.target == ConstraintTarget::Placement(instance))?;
    project.board().edge(&c.edge)?;
    let offset = plain_mm(&c.offset).or_else(|| {
        values(project, evaluator)
            .ok()
            .and_then(|v| v.get(&c.id).map(|v| v.1))
    });
    Some(Error::Conflict(held_message(
        &part_name(project, instance),
        &c.edge,
        offset,
        "move",
    )))
}

/// Translate an outline so its reference point (its start for
/// [`ConstraintAlign::Origin`]) lands on the pose.
fn place_outline(
    outline: &mut Outline,
    align: ConstraintAlign,
    pose: &EdgePose,
    u: Point,
    n: Point,
) -> Result<()> {
    let origin = outline.start;
    let pts: Vec<Point> = crate::model::board::flatten(outline, EXTENT_CHORD_MM)
        .into_iter()
        .map(|p| [p[0] - origin[0], p[1] - origin[1]])
        .collect();
    let r = reference(&pts, align, u, n);
    let delta = [
        pose.position[0] - r[0] - origin[0],
        pose.position[1] - r[1] - origin[1],
    ];
    if delta.iter().all(|d| d.abs() <= SAME) {
        return Ok(());
    }
    let shift = |p: Point| [p[0] + delta[0], p[1] + delta[1]];
    outline.start = shift(outline.start);
    for s in &mut outline.segments {
        match s {
            crate::library::Segment::Line { to } | crate::library::Segment::Arc { to, .. } => {
                *to = shift(*to);
            }
        }
    }
    let finite = point_valid(outline.start) && outline.segments.iter().all(|s| point_valid(s.to()));
    if !finite {
        return Err(Error::Invalid(
            "board.constraint: an edge constraint puts its outline out of range".into(),
        ));
    }
    Ok(())
}

/// Board-coordinate points of each placed part's extent: its closed
/// courtyard shapes, or its pads' copper when it has no courtyard.
pub(super) fn placement_extents(project: &Project) -> BTreeMap<Id, Vec<Point>> {
    let mut out: BTreeMap<Id, Vec<Point>> = BTreeMap::new();
    for c in crate::layout::courtyard::placed_courtyards(project) {
        out.entry(c.instance)
            .or_default()
            .extend(c.shape.flatten(EXTENT_CHORD_MM));
    }
    let mut pads: BTreeMap<Id, Vec<Point>> = BTreeMap::new();
    for pad in crate::layout::pads(project) {
        if !out.contains_key(&pad.instance) {
            pads.entry(pad.instance)
                .or_default()
                .extend(pad.outline().flatten(EXTENT_CHORD_MM));
        }
    }
    out.extend(pads);
    out
}

/// `board.constrain`: add a constraint; returns its identity.
pub(super) fn add(project: &mut Project, constraint: EdgeConstraintSpec) -> Result<Id> {
    check_spec(project, &constraint)?;
    if project.edge_constraints.len() >= MAX_EDGE_CONSTRAINTS {
        return Err(Error::Limit(format!(
            "board.constraint_limit: at most {MAX_EDGE_CONSTRAINTS} edge constraints"
        )));
    }
    if project
        .edge_constraints
        .iter()
        .any(|c| c.target == constraint.target)
    {
        return Err(Error::Conflict(
            "board.constrained: the target already has an edge constraint".into(),
        ));
    }
    let id = project.allocate()?;
    project
        .edge_constraints
        .push(EdgeConstraint::from_spec(id, constraint));
    Ok(id)
}

/// `board.constraint_edit`: replace a constraint, by id.
pub(super) fn edit(project: &mut Project, constraint: EdgeConstraint) -> Result<()> {
    check_spec(project, &spec_of(&constraint))?;
    if project
        .edge_constraints
        .iter()
        .any(|c| c.id != constraint.id && c.target == constraint.target)
    {
        return Err(Error::Conflict(
            "board.constrained: the target already has an edge constraint".into(),
        ));
    }
    let slot = project
        .edge_constraints
        .iter_mut()
        .find(|c| c.id == constraint.id)
        .ok_or_else(|| missing(constraint.id))?;
    *slot = constraint;
    Ok(())
}

/// `board.constraint_delete`.
pub(super) fn delete(project: &mut Project, id: Id) -> Result<()> {
    let before = project.edge_constraints.len();
    project.edge_constraints.retain(|c| c.id != id);
    if project.edge_constraints.len() == before {
        return Err(missing(id));
    }
    Ok(())
}

/// Constraints whose edge or target is gone ([`CONSTRAINT_UNRESOLVED`]).
pub(super) fn checks(project: &Project, out: &mut Vec<Finding>) {
    if project.edge_constraints.is_empty() {
        return;
    }
    let board = project.board();
    for c in &project.edge_constraints {
        let (target, objects) = match c.target {
            ConstraintTarget::Placement(id) => (
                project
                    .placements
                    .iter()
                    .any(|p| p.instance == id)
                    .then(|| {
                        project
                            .instance(id)
                            .map_or_else(|| id.to_string(), |i| i.designator.clone())
                    }),
                vec![ObjectRef::Instance { id }],
            ),
            ConstraintTarget::Fill(id) => (
                project
                    .fills
                    .iter()
                    .find(|f| f.id == id)
                    .map(|f| format!("fill {}", f.name)),
                vec![ObjectRef::Board],
            ),
            ConstraintTarget::Keepout(id) => (
                project
                    .keepouts
                    .iter()
                    .find(|k| k.id == id)
                    .map(|k| format!("keepout {}", k.name)),
                vec![ObjectRef::Board],
            ),
        };
        let message = match (target, board.edge(&c.edge)) {
            (None, _) => format!(
                "Edge constraint {} places an object that is no longer on the board",
                c.id
            ),
            (Some(t), None) => format!(
                "{t} is constrained to edge `{}`, which the board no longer has",
                c.edge.0
            ),
            (Some(_), Some(_)) => continue,
        };
        out.push(Finding::new(CONSTRAINT_UNRESOLVED, message, objects));
    }
}

/// Constraints within the limit, unique per target, expressions well
/// formed.
pub(super) fn validate(project: &Project) -> Result<()> {
    if project.edge_constraints.len() > MAX_EDGE_CONSTRAINTS {
        return Err(Error::Limit(format!(
            "board.constraint_limit: at most {MAX_EDGE_CONSTRAINTS} edge constraints"
        )));
    }
    let mut targets = BTreeSet::new();
    for c in &project.edge_constraints {
        let target = match c.target {
            ConstraintTarget::Placement(id) => (0u8, id),
            ConstraintTarget::Fill(id) => (1, id),
            ConstraintTarget::Keepout(id) => (2, id),
        };
        if !targets.insert(target)
            || !c.along.valid()
            || !c.offset.valid()
            || !c.rotation_deg.is_finite()
            || anchor_valid(c.anchor).is_err()
            || c.edge.0.is_empty()
            || c.edge.0.len() > crate::ids::MAX_LABEL
        {
            return Err(Error::Invalid(
                "board.constraint: one constraint per target, with a named edge and well-formed expressions"
                    .into(),
            ));
        }
    }
    Ok(())
}
