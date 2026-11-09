//! Slice S2 (docs/board-sketch-references.md §5.2, §5.6): dimensioned
//! parts solved after the board, drag-to-move, pad and courtyard features,
//! expressions over board parameters, and `EdgeConstraint` replaced.
//! Ignored until S2 implements them; S2 removes each `#[ignore]` without
//! weakening the expected values.
#![allow(clippy::unwrap_used)]

use super::super::tests::{Fallible, apply, close, edit, hosted, set, sketch, sketched};
use pcbcraft_core::geometry::Point;
use pcbcraft_core::ids::Id;
use pcbcraft_core::layout::LayoutCommand;
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, Expr, GeomConstraint, SketchCurveId, SketchRef,
};
use pcbcraft_core::model::board::ConstraintTarget;
use pcbcraft_core::model::dimension::{
    BoardRef, BoxSide, FeatureRef, PadPoint, PartAxis, PartDimensionSpec, PartRelation,
};
use pcbcraft_core::{Engine, Layer, Operation, Outcome};

fn curve(engine: &Engine, name: &str) -> Fallible<SketchCurveId> {
    sketch(engine)?
        .edges
        .iter()
        .find(|e| e.name.0 == name)
        .map(|e| e.curve.clone())
        .ok_or_else(|| format!("no edge {name}").into())
}

fn edge(engine: &Engine, name: &str) -> Fallible<BoardRef> {
    Ok(BoardRef::Board(SketchRef::Curve(curve(engine, name)?)))
}

fn origin(part: Id) -> BoardRef {
    BoardRef::Part(FeatureRef::Origin {
        target: ConstraintTarget::Placement(part),
    })
}

fn distance(refs: Vec<BoardRef>, value: DimValue) -> PartDimensionSpec {
    PartDimensionSpec {
        kind: PartRelation::Dim(DimKind::Distance),
        refs,
        value: Some(value),
        reference: false,
    }
}

fn add(engine: &mut Engine, dimension: PartDimensionSpec) -> pcbcraft_core::Result<Id> {
    match apply(
        engine,
        Operation::Dimension(DimensionCommand::Add { dimension }),
    )? {
        Outcome::DimensionAdded { dimension } => Ok(dimension),
        _ => Err(pcbcraft_core::Error::Invalid("no dimension added".into())),
    }
}

fn parts(engine: &Engine) -> Fallible<(Id, Id)> {
    let p = &engine.project().placements;
    Ok((
        p.first().ok_or("no part")?.instance,
        p.get(1).ok_or("no second part")?.instance,
    ))
}

fn pose(engine: &Engine, part: Id) -> Fallible<(Point, f64, Layer)> {
    engine
        .project()
        .placements
        .iter()
        .find(|p| p.instance == part)
        .map(|p| (p.position, p.rotation_deg, p.layer))
        .ok_or_else(|| "placement".into())
}

fn move_to(engine: &mut Engine, part: Id, to: Point) -> Fallible<pcbcraft_core::Result<Outcome>> {
    let (_, rotation_deg, layer) = pose(engine, part)?;
    Ok(apply(
        engine,
        Operation::Layout(LayoutCommand::Move {
            instance: part,
            position: to,
            rotation_deg,
            layer,
        }),
    ))
}

/// A part's origin 4 mm from the right edge sits at x = 56 on the 60 mm
/// board and follows the width to x = 76; its y and rotation, which no
/// dimension fixes, keep their stored values.
#[test]
#[ignore = "slice S2"]
fn a_part_dimensioned_to_an_edge_moves_with_the_board() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, _) = parts(&engine)?;
    let (p0, r0, _) = pose(&engine, part)?;
    let right = edge(&engine, "right")?;
    add(
        &mut engine,
        distance(vec![origin(part), right], DimValue::Fixed(4.0)),
    )?;
    let (p, r, _) = pose(&engine, part)?;
    assert!(close(p, [56.0, p0[1]]), "{p:?}");
    assert_eq!(r, r0);
    edit(&mut engine, vec![set("width", "80 mm")])?;
    let (p, r, _) = pose(&engine, part)?;
    assert!(close(p, [76.0, p0[1]]), "{p:?}");
    assert_eq!(r, r0);
    Ok(())
}

/// Only parts a dimension names are solved: another part keeps its exact
/// stored pose through the add and a board edit.
#[test]
#[ignore = "slice S2"]
fn only_dimensioned_parts_are_solved() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, other) = parts(&engine)?;
    let before = pose(&engine, other)?;
    let right = edge(&engine, "right")?;
    add(
        &mut engine,
        distance(vec![origin(part), right], DimValue::Fixed(4.0)),
    )?;
    edit(&mut engine, vec![set("width", "80 mm")])?;
    assert_eq!(pose(&engine, other)?, before);
    Ok(())
}

/// A move is a solver drag: a part with one dimension to the right edge
/// slides along it, landing at the asked y with x still 4 mm in.
#[test]
#[ignore = "slice S2"]
fn dragging_slides_a_part_along_its_one_edge_dimension() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, _) = parts(&engine)?;
    let right = edge(&engine, "right")?;
    add(
        &mut engine,
        distance(vec![origin(part), right], DimValue::Fixed(4.0)),
    )?;
    move_to(&mut engine, part, [10.0, 12.0])??;
    let (p, _, _) = pose(&engine, part)?;
    assert!(close(p, [56.0, 12.0]), "{p:?}");
    Ok(())
}

/// A part fixed in x, y and rotation cannot be moved: the move is refused
/// in the held words ("R1 is held …") and changes nothing.
#[test]
#[ignore = "slice S2"]
fn a_move_that_cannot_land_is_refused_in_held_words() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, _) = parts(&engine)?;
    let (right, bottom) = (edge(&engine, "right")?, edge(&engine, "bottom")?);
    add(
        &mut engine,
        distance(vec![origin(part), right], DimValue::Fixed(4.0)),
    )?;
    add(
        &mut engine,
        distance(vec![origin(part), bottom.clone()], DimValue::Fixed(10.0)),
    )?;
    add(
        &mut engine,
        PartDimensionSpec {
            kind: PartRelation::Geom(GeomConstraint::Parallel),
            refs: vec![
                BoardRef::Part(FeatureRef::Axis {
                    target: ConstraintTarget::Placement(part),
                    axis: PartAxis::X,
                }),
                bottom,
            ],
            value: None,
            reference: false,
        },
    )?;
    let rev = engine.revision();
    let before = engine.to_file();
    let refused = move_to(&mut engine, part, [10.0, 30.0])?;
    let message = refused.err().ok_or("the move was not refused")?.to_string();
    assert!(message.contains("is held"), "{message}");
    assert_eq!(engine.revision(), rev);
    assert_eq!(engine.to_file(), before);
    Ok(())
}

/// An expression value evaluates over the board parameters: a quarter of
/// the width from the left edge is 15 mm, then 20 mm at 80 mm wide.
#[test]
#[ignore = "slice S2"]
fn an_expression_value_evaluates_over_board_parameters() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, _) = parts(&engine)?;
    let left = edge(&engine, "left")?;
    add(
        &mut engine,
        distance(
            vec![origin(part), left],
            DimValue::Expr(Expr("width / 4".into())),
        ),
    )?;
    assert!((pose(&engine, part)?.0[0] - 15.0).abs() < 1e-6);
    edit(&mut engine, vec![set("width", "80 mm")])?;
    assert!((pose(&engine, part)?.0[0] - 20.0).abs() < 1e-6);
    Ok(())
}

/// A reference dimension measures and never drives.
#[test]
#[ignore = "slice S2"]
fn a_reference_dimension_measures_but_never_drives() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, _) = parts(&engine)?;
    let before = pose(&engine, part)?;
    let right = edge(&engine, "right")?;
    let mut spec = distance(vec![origin(part), right], DimValue::Fixed(4.0));
    spec.reference = true;
    add(&mut engine, spec)?;
    edit(&mut engine, vec![set("width", "80 mm")])?;
    assert_eq!(pose(&engine, part)?, before);
    Ok(())
}

/// A dimension that over-constrains its part is refused atomically.
#[test]
#[ignore = "slice S2"]
fn an_over_constraining_dimension_is_refused_and_changes_nothing() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, _) = parts(&engine)?;
    let right = edge(&engine, "right")?;
    add(
        &mut engine,
        distance(vec![origin(part), right.clone()], DimValue::Fixed(4.0)),
    )?;
    let rev = engine.revision();
    let before = engine.to_file();
    assert!(
        add(
            &mut engine,
            distance(vec![origin(part), right], DimValue::Fixed(6.0))
        )
        .is_err()
    );
    assert_eq!(engine.revision(), rev);
    assert_eq!(engine.to_file(), before);
    Ok(())
}

/// Two parts a dimension joins solve as one component: B 10 mm left of A,
/// A 4 mm from the right edge, both follow the width.
#[test]
#[ignore = "slice S2"]
fn parts_dimensioned_to_each_other_solve_together() -> Fallible<()> {
    let mut engine = sketched()?;
    let (a, b) = parts(&engine)?;
    let right = edge(&engine, "right")?;
    add(
        &mut engine,
        distance(vec![origin(a), right], DimValue::Fixed(4.0)),
    )?;
    add(
        &mut engine,
        PartDimensionSpec {
            kind: PartRelation::Dim(DimKind::HorizontalDistance),
            refs: vec![origin(b), origin(a)],
            value: Some(DimValue::Fixed(10.0)),
            reference: false,
        },
    )?;
    assert!((pose(&engine, b)?.0[0] - 46.0).abs() < 1e-6);
    edit(&mut engine, vec![set("width", "80 mm")])?;
    assert!((pose(&engine, a)?.0[0] - 76.0).abs() < 1e-6);
    assert!((pose(&engine, b)?.0[0] - 66.0).abs() < 1e-6);
    Ok(())
}

/// A pad side is a feature in the part frame: with the pad's right side
/// 1 mm from the right edge, the placed pad's copper ends at x = 59.
#[test]
#[ignore = "slice S2"]
fn a_pad_side_is_dimensioned_in_the_part_frame() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, _) = parts(&engine)?;
    let pad = pcbcraft_core::layout::pads(engine.project())
        .into_iter()
        .find(|p| p.instance == part)
        .ok_or("pad")?
        .pad;
    let right = edge(&engine, "right")?;
    add(
        &mut engine,
        distance(
            vec![
                BoardRef::Part(FeatureRef::Pad {
                    placement: part,
                    pad,
                    at: PadPoint::Side(BoxSide::Right),
                }),
                right,
            ],
            DimValue::Fixed(1.0),
        ),
    )?;
    let placed = pcbcraft_core::layout::pads(engine.project())
        .into_iter()
        .find(|p| p.instance == part && p.pad == pad)
        .ok_or("pad")?;
    let max_x = placed
        .outline()
        .flatten(1e-4)
        .iter()
        .map(|q| q[0])
        .fold(f64::MIN, f64::max);
    assert!((max_x - 59.0).abs() < 1e-3, "{max_x}");
    Ok(())
}

/// Deleting a dimension keeps the pose it gave; the part then stays put
/// when the board changes. Adding is one undo step.
#[test]
#[ignore = "slice S2"]
fn dimension_commands_are_undo_steps_and_delete_keeps_the_pose() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, _) = parts(&engine)?;
    let before = pose(&engine, part)?;
    let right = edge(&engine, "right")?;
    add(
        &mut engine,
        distance(vec![origin(part), right.clone()], DimValue::Fixed(4.0)),
    )?;
    engine.undo()?;
    assert!(engine.project().part_dimensions.is_empty());
    assert_eq!(pose(&engine, part)?, before);
    let id = add(
        &mut engine,
        distance(vec![origin(part), right], DimValue::Fixed(4.0)),
    )?;
    let held = pose(&engine, part)?;
    apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Delete { id }),
    )?;
    assert_eq!(pose(&engine, part)?, held);
    edit(&mut engine, vec![set("width", "80 mm")])?;
    assert_eq!(pose(&engine, part)?, held);
    Ok(())
}

/// The showcase holds J1 with part dimensions instead of an edge
/// constraint, in the same place: (6, 15), centred on the left edge as the
/// height changes.
#[test]
#[ignore = "slice S2"]
fn the_showcase_header_is_held_by_part_dimensions() -> Fallible<()> {
    let mut engine = Engine::showcase_hosted(hosted())?;
    let project = engine.project();
    assert!(project.edge_constraints.is_empty());
    let j1 = project
        .instance_by_designator("J1")
        .map(|i| i.id)
        .ok_or("J1")?;
    assert!(project.part_dimensions.iter().any(|d| {
        d.refs.iter().any(
            |r| matches!(r, BoardRef::Part(f) if f.target() == ConstraintTarget::Placement(j1)),
        )
    }));
    assert!(close(pose(&engine, j1)?.0, [6.0, 15.0]));
    edit(&mut engine, vec![set("corner_r", "3 mm")])?;
    assert!(close(pose(&engine, j1)?.0, [6.0, 15.0]));
    edit(&mut engine, vec![set("height", "32 mm")])?;
    assert!(close(pose(&engine, j1)?.0, [6.0, 17.0]));
    Ok(())
}
