//! What the board interface is now: each datum's derived geometry, status,
//! display name and content hash, `interface.list`, and the findings for
//! datums that no longer resolve (docs/architecture/board-interface.md).
//! Read from the document only; nothing here traces a model or solves.
//!
//! Geometry is in board millimetres: x and y on the board frame, z up from
//! the bottom face, which is z = 0; the top face is at the stackup's
//! thickness.
use super::report::{DatumRow, InterfaceList, RuleState};
use super::{DatumGeometry, DatumStatus, Frame, Vec3, auto, findings, package, role_kind, token};
use crate::diagnostics::{Finding, ObjectRef, Result};
use crate::geometry::Point;
use crate::ids::{ContentHash, Id};
use crate::layout::dimensions::{FeatureGeometry, features, view};
use crate::library::datum::DatumKind;
use crate::library::footprint_source::{ParamUnit, SketchRef};
use crate::library::{Outline, Segment};
use crate::model::board::BoardShape;
use crate::model::dimension::{BoardRef, BoxSide, FeatureRef, ReferenceStatus};
use crate::model::interface::{AutoRule, DatumOwner, DatumRole, InterfaceDatum, InterfaceTable};
use crate::model::{Layer, Project};
use sha2::{Digest, Sha256};
use std::borrow::Cow;

const X: Vec3 = Vec3([1.0, 0.0, 0.0]);
const Y: Vec3 = Vec3([0.0, 1.0, 0.0]);
const UP: Vec3 = Vec3([0.0, 0.0, 1.0]);
const DOWN: Vec3 = Vec3([0.0, 0.0, -1.0]);

/// The table as the automatic rules make it on this board: the stored one,
/// matched to the board. A document that has had no `interface.*` command
/// yet stores none and shows the slots its first one will store.
pub fn table(project: &Project) -> Result<Cow<'_, InterfaceTable>> {
    Ok(match auto::reconcile(&project.interface, project)? {
        Some(table) => Cow::Owned(table),
        None => Cow::Borrowed(&project.interface),
    })
}

/// One datum as the interface shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Derived {
    pub kind: DatumKind,
    /// `None` when the datum is lost, or when its value needs the host's
    /// expression evaluator (a board parameter given as an expression).
    pub geometry: Option<DatumGeometry>,
    pub status: DatumStatus,
}

fn thickness(project: &Project) -> f64 {
    project.stackup.thickness_mm()
}

/// The board frame at height `z`, its z along +z.
fn up_frame(z: f64) -> Frame {
    Frame {
        origin: Vec3([0.0, 0.0, z]),
        x: X,
        y: Y,
    }
}

/// The board frame turned over at height `z`, its z along −z.
fn down_frame(z: f64) -> Frame {
    Frame {
        origin: Vec3([0.0, 0.0, z]),
        x: X,
        y: Vec3([0.0, -1.0, 0.0]),
    }
}

/// The height of the surface a part on `layer` sits on, and the normal
/// pointing away from the board there.
fn surface(project: &Project, layer: Layer) -> (f64, Vec3) {
    match layer {
        Layer::Top => (thickness(project), UP),
        Layer::Bottom => (0.0, DOWN),
    }
}

/// A placement's frame: its origin on the surface it sits on, x and y as
/// the part turns (and, on the bottom, mirrors), z away from the board.
fn part_frame(project: &Project, instance: Id) -> Option<Frame> {
    let placement = project.placement_of(instance)?;
    let t = view::part_transform(placement);
    let o = t.apply([0.0, 0.0]);
    let along = |p: Point| {
        let q = t.apply(p);
        Vec3([q[0] - o[0], q[1] - o[1], 0.0])
    };
    let (z, _) = surface(project, placement.layer);
    Some(Frame {
        origin: Vec3([o[0], o[1], z]),
        x: along([1.0, 0.0]),
        y: along([0.0, 1.0]),
    })
}

/// The package envelope's height of a placed part, mm.
fn body_height(project: &Project, instance: Id) -> Option<f64> {
    let instance = project.instance(instance)?;
    let physical = crate::instances::resolve(&project.library, instance)
        .ok()?
        .physical?;
    Some(physical.package.definition.envelope()[2])
}

fn rectangle(min: Point, max: Point) -> Outline {
    Outline {
        start: min,
        segments: vec![
            Segment::Line {
                to: [max[0], min[1]],
            },
            Segment::Line { to: max },
            Segment::Line {
                to: [min[0], max[1]],
            },
            Segment::Line { to: min },
        ],
    }
}

/// Courtyard × body height in the part frame.
fn envelope(project: &Project, instance: Id) -> Option<DatumGeometry> {
    let frame = part_frame(project, instance)?;
    let side = |side| {
        features::geometry(
            project,
            &FeatureRef::Courtyard {
                placement: instance,
                side: Some(side),
            },
        )
        .ok()
    };
    let (Some(FeatureGeometry::Line { a, .. }), Some(FeatureGeometry::Line { a: c, .. })) =
        (side(BoxSide::Bottom), side(BoxSide::Top))
    else {
        return None;
    };
    // Bottom runs from the lower left corner, top from the upper right.
    Some(DatumGeometry::Volume {
        frame,
        loops: vec![rectangle(a, c)],
        height: body_height(project, instance)?,
    })
}

/// The tallest part on one side, over the board outline.
fn side_height(project: &Project, layer: Layer) -> DatumGeometry {
    let height = project
        .placements
        .iter()
        .filter(|p| p.layer == layer)
        .filter_map(|p| body_height(project, p.instance))
        .fold(0.0, f64::max);
    let frame = match layer {
        Layer::Top => up_frame(thickness(project)),
        Layer::Bottom => down_frame(0.0),
    };
    DatumGeometry::Volume {
        frame,
        loops: vec![project.board().outer.clone()],
        height,
    }
}

fn parameter(project: &Project, name: &str) -> Option<Option<DatumGeometry>> {
    let BoardShape::Sketch(sketch) = &project.board_shape else {
        return None;
    };
    let param = sketch.parameters.iter().find(|p| p.name.0 == name)?;
    let unit = match param.unit {
        ParamUnit::Mm => "mm",
        ParamUnit::Deg => "deg",
        ParamUnit::Count => "",
    };
    Some(
        param
            .expr
            .0
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| DatumGeometry::Parameter {
                value,
                unit: unit.into(),
            }),
    )
}

/// The kind a feature datum has: what its geometry is, or, when it is
/// lost, what the feature names.
fn feature_kind(feature: &BoardRef) -> DatumKind {
    match feature {
        BoardRef::Board(SketchRef::Point(_))
        | BoardRef::Part(
            FeatureRef::Origin { .. }
            | FeatureRef::Anchor { .. }
            | FeatureRef::Vertex { .. }
            | FeatureRef::Courtyard { side: None, .. }
            | FeatureRef::Pad {
                at: crate::model::dimension::PadPoint::Centre,
                ..
            },
        ) => DatumKind::Axis,
        _ => DatumKind::Plane,
    }
}

/// A feature's trace in the board as a datum: a point, circle or arc is an
/// axis square to the board (with the diameter of a circle or arc), a line
/// the plane through it square to the board.
fn feature_geometry(g: FeatureGeometry, z: f64, normal: Vec3) -> Option<DatumGeometry> {
    let axis = |at: Point, diameter: Option<f64>| DatumGeometry::Axis {
        point: Vec3([at[0], at[1], z]),
        direction: normal,
        diameter,
    };
    match g {
        FeatureGeometry::Point { at } => Some(axis(at, None)),
        FeatureGeometry::Circle { center, radius } => Some(axis(center, Some(2.0 * radius))),
        FeatureGeometry::Arc { a, b, bulge } => crate::model::board::arc_geometry(a, b, bulge)
            .map(|arc| axis(arc.center, Some(2.0 * arc.radius))),
        FeatureGeometry::Line { a, b } => {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let length = dx.hypot(dy);
            (length > 1e-9).then(|| DatumGeometry::Plane {
                origin: Vec3([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, z]),
                normal: Vec3([dy / length, -dx / length, 0.0]),
            })
        }
    }
}

fn feature(project: &Project, r: &BoardRef) -> Derived {
    let (z, normal) = match r {
        BoardRef::Part(f) => match f.target() {
            crate::model::board::ConstraintTarget::Placement(id) => project
                .placement_of(id)
                .map_or((thickness(project), UP), |p| surface(project, p.layer)),
            _ => (thickness(project), UP),
        },
        _ => (0.0, UP),
    };
    let Some((g, status)) = view::reference_geometry(project, None, r) else {
        let ids = crate::layout::dimensions::board_ids(project);
        let status = crate::layout::dimensions::reference_status(project, ids.as_ref(), r);
        return Derived {
            kind: feature_kind(r),
            geometry: None,
            status: match status {
                ReferenceStatus::Resolved => DatumStatus::Resolved,
                ReferenceStatus::Inexact | ReferenceStatus::Lost => DatumStatus::Lost,
            },
        };
    };
    let geometry = feature_geometry(g, z, normal);
    Derived {
        kind: geometry
            .as_ref()
            .map_or_else(|| feature_kind(r), DatumGeometry::kind),
        status: match (status, &geometry) {
            (_, None) | (ReferenceStatus::Lost, _) => DatumStatus::Lost,
            (ReferenceStatus::Inexact, _) => DatumStatus::Inexact,
            (ReferenceStatus::Resolved, _) => DatumStatus::Resolved,
        },
        geometry,
    }
}

/// A datum's kind, geometry and status on `project`.
pub fn derive(project: &Project, datum: &InterfaceDatum) -> Derived {
    let found = |kind, geometry: Option<DatumGeometry>| Derived {
        kind,
        status: if geometry.is_some() {
            DatumStatus::Resolved
        } else {
            DatumStatus::Lost
        },
        geometry,
    };
    let key = &datum.key;
    let kind = role_kind(key.role).unwrap_or(DatumKind::Volume);
    let t = thickness(project);
    match (&key.owner, key.role) {
        (_, DatumRole::Feature) => match &datum.feature {
            Some(r) => feature(project, r),
            None => found(DatumKind::Plane, None),
        },
        (DatumOwner::Board, DatumRole::BoardFrame) => found(
            kind,
            Some(DatumGeometry::Frame {
                frame: up_frame(0.0),
            }),
        ),
        (DatumOwner::Board, DatumRole::BoardTop) => found(
            kind,
            Some(DatumGeometry::Plane {
                origin: Vec3([0.0, 0.0, t]),
                normal: UP,
            }),
        ),
        (DatumOwner::Board, DatumRole::BoardBottom) => found(
            kind,
            Some(DatumGeometry::Plane {
                origin: Vec3([0.0, 0.0, 0.0]),
                normal: DOWN,
            }),
        ),
        (DatumOwner::Board, DatumRole::BoardThickness) => found(
            kind,
            Some(DatumGeometry::Parameter {
                value: t,
                unit: "mm".into(),
            }),
        ),
        (DatumOwner::Board, DatumRole::BoardOutline) => found(
            kind,
            Some(DatumGeometry::Profile {
                frame: up_frame(0.0),
                loops: vec![project.board().outer.clone()],
            }),
        ),
        (DatumOwner::Board, DatumRole::SideHeight) => {
            let layer = match key.name.as_deref() {
                Some("bottom") => Layer::Bottom,
                _ => Layer::Top,
            };
            found(kind, Some(side_height(project, layer)))
        }
        (DatumOwner::Board, DatumRole::BoardParameter) => {
            match parameter(project, key.name.as_deref().unwrap_or_default()) {
                Some(geometry) => Derived {
                    kind,
                    geometry,
                    status: DatumStatus::Resolved,
                },
                None => found(kind, None),
            }
        }
        (DatumOwner::Curve { id }, DatumRole::BoardCutout) => {
            let board = project.board();
            let loop_ = board
                .cutouts
                .iter()
                .find(|c| super::cutout_owner(&c.curves) == Some(id))
                .map(|c| DatumGeometry::Profile {
                    frame: up_frame(0.0),
                    loops: vec![c.outline.clone()],
                });
            found(kind, loop_)
        }
        (DatumOwner::Hole { id }, DatumRole::Hole) => {
            let board = project.board();
            let axis = board
                .holes
                .iter()
                .find(|h| h.id == *id)
                .map(|h| DatumGeometry::Axis {
                    point: Vec3([h.at[0], h.at[1], 0.0]),
                    direction: UP,
                    diameter: Some(h.drill),
                });
            found(kind, axis)
        }
        (DatumOwner::Instance { id }, DatumRole::PartFrame) => found(
            kind,
            part_frame(project, *id).map(|frame| DatumGeometry::Frame { frame }),
        ),
        (DatumOwner::Instance { id }, DatumRole::PartEnvelope) => {
            found(kind, envelope(project, *id))
        }
        (DatumOwner::Instance { id }, DatumRole::PartDatum) => {
            let name = key.name.as_deref().unwrap_or_default();
            match package::placed(project, *id)
                .into_iter()
                .find(|d| d.name.0 == name)
            {
                Some(d) => found(d.kind, d.geometry),
                None => found(kind, None),
            }
        }
        _ => found(kind, None),
    }
}

/// The words for a placed part: its designator, or that it is gone.
fn part(project: &Project, id: Id) -> String {
    project
        .instance(id)
        .map_or_else(|| "A deleted part".into(), |i| i.designator.clone())
}

/// "J1 frame", or "A deleted part's frame".
fn part_words(project: &Project, id: Id, what: &str) -> String {
    match project.instance(id) {
        Some(i) => format!("{} {what}", i.designator),
        None => format!("A deleted part's {what}"),
    }
}

/// The name people read for a datum: the one the user gave, or one derived
/// from what it belongs to ("J1 opening", "Board top"). Never identity.
pub fn display_name(project: &Project, datum: &InterfaceDatum) -> String {
    if let Some(name) = &datum.name {
        return name.clone();
    }
    let key = &datum.key;
    let name = key.name.as_deref().unwrap_or_default();
    match (&key.owner, key.role) {
        (_, DatumRole::Feature) => match &datum.feature {
            Some(BoardRef::Part(f)) if f.is_model() => {
                crate::layout::dimensions::model_feature_words(project, f)
            }
            Some(BoardRef::Part(f)) => match f.target() {
                crate::model::board::ConstraintTarget::Placement(id) => {
                    format!("{} feature", part(project, id))
                }
                _ => "Fill or keepout feature".into(),
            },
            Some(BoardRef::Board(SketchRef::Point(_))) => "Board point".into(),
            _ => "Board edge".into(),
        },
        (_, DatumRole::BoardFrame) => "Board frame".into(),
        (_, DatumRole::BoardTop) => "Board top".into(),
        (_, DatumRole::BoardBottom) => "Board bottom".into(),
        (_, DatumRole::BoardThickness) => "Board thickness".into(),
        (_, DatumRole::BoardOutline) => "Board outline".into(),
        (_, DatumRole::BoardParameter) => format!("Board parameter {name}"),
        (_, DatumRole::SideHeight) if name == "bottom" => "Bottom side height".into(),
        (_, DatumRole::SideHeight) => "Top side height".into(),
        (DatumOwner::Curve { id }, DatumRole::BoardCutout) => {
            let board = project.board();
            let at = board
                .cutouts
                .iter()
                .position(|c| super::cutout_owner(&c.curves) == Some(id));
            match (at, at.and_then(|i| board.cutouts.get(i)).map(|c| c.kind)) {
                (Some(i), Some(crate::model::board::CutoutKind::Slot { .. })) => {
                    format!("Slot {}", i + 1)
                }
                (Some(i), _) => format!("Cutout {}", i + 1),
                (None, _) => "A deleted cutout".into(),
            }
        }
        (DatumOwner::Hole { id }, DatumRole::Hole) => {
            match project.board().holes.iter().position(|h| h.id == *id) {
                Some(i) => format!("Mounting hole {}", i + 1),
                None => "A deleted mounting hole".into(),
            }
        }
        (DatumOwner::Instance { id }, DatumRole::PartFrame) => part_words(project, *id, "frame"),
        (DatumOwner::Instance { id }, DatumRole::PartEnvelope) => {
            part_words(project, *id, "envelope")
        }
        (DatumOwner::Instance { id }, DatumRole::PartDatum) => {
            part_words(project, *id, &name.replace('_', " "))
        }
        _ => "Datum".into(),
    }
}

/// The role as the sidecar writes it: the key's role, except that a
/// package datum writes `part.<name>` (`part.opening`).
pub fn role_text(datum: &InterfaceDatum) -> Result<String> {
    if datum.key.role == DatumRole::PartDatum {
        return Ok(format!(
            "part.{}",
            datum.key.name.as_deref().unwrap_or_default()
        ));
    }
    let value = serde_json::to_value(datum.key.role)?;
    Ok(value.as_str().unwrap_or_default().to_owned())
}

/// A datum's content hash: SHA-256 of the canonical JSON of its token,
/// role, kind, geometry and status. The display name is not part of it,
/// so a rename changes no hash.
pub fn datum_hash(row: &DatumRow) -> Result<ContentHash> {
    let canonical = serde_json::to_vec(&serde_json::json!({
        "token": row.token,
        "role": row.role,
        "kind": row.kind,
        "geometry": row.geometry,
        "status": row.status,
    }))?;
    Ok(ContentHash(Sha256::digest(&canonical).into()))
}

/// Every live datum of `table` as a row, in slot order.
pub fn rows(project: &Project, table: &InterfaceTable) -> Result<Vec<DatumRow>> {
    let mut out = Vec::with_capacity(table.datums.len());
    for datum in &table.datums {
        let derived = derive(project, datum);
        let mut row = DatumRow {
            id: datum.id,
            slot: datum.slot,
            token: token(datum)?,
            name: display_name(project, datum),
            role: role_text(datum)?,
            kind: derived.kind,
            source: datum.source,
            geometry: derived.geometry,
            status: derived.status,
            changed: true,
        };
        if let Some(record) = &table.published {
            let hash = datum_hash(&row)?;
            row.changed = !record
                .datums
                .iter()
                .any(|p| p.id == datum.id && p.slot == datum.slot && p.hash == hash);
        }
        out.push(row);
    }
    Ok(out)
}

/// `interface.list`: the board's interface as it stands.
pub fn list(project: &Project) -> Result<InterfaceList> {
    let table = table(project)?;
    let datums = rows(project, &table)?;
    let retired_since = table.published.as_ref().map_or(0, |record| {
        record
            .datums
            .iter()
            .filter(|p| table.datum(p.id).is_none())
            .count()
    });
    let changed = datums.iter().filter(|r| r.changed).count() + retired_since;
    Ok(InterfaceList {
        source_id: table.source_id,
        rules: AutoRule::ALL
            .into_iter()
            .map(|rule| RuleState {
                rule,
                on: table.rule_on(rule),
            })
            .collect(),
        datums,
        retired: table.retired.clone(),
        published: table.published.as_ref().map(|r| r.revision),
        changed,
    })
}

/// The object a datum's finding names: its part, or the board.
fn owner_object(project: &Project, datum: &InterfaceDatum) -> Option<ObjectRef> {
    match &datum.key.owner {
        DatumOwner::Instance { id } => project
            .instance(*id)
            .map(|_| ObjectRef::Instance { id: *id }),
        _ => Some(ObjectRef::Board),
    }
}

/// `interface.datum_lost` and `interface.datum_inexact`, one per exported
/// datum that does not resolve exactly.
pub fn checks(project: &Project) -> Vec<Finding> {
    let Ok(table) = table(project) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for datum in &table.datums {
        let status = derive(project, datum).status;
        if status == DatumStatus::Resolved {
            continue;
        }
        let name = display_name(project, datum);
        let owner = owner_object(project, datum);
        out.push(match status {
            DatumStatus::Inexact => findings::datum_inexact(&name, owner),
            _ => findings::datum_lost(&name, owner),
        });
    }
    out
}
