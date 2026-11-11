//! Slice S2 (docs/board-sketch-references.md §5.2, §5.6): dimensioned
//! parts solved after the board, drag-to-move, pad and courtyard features,
//! expressions over board parameters, and `EdgeConstraint` replaced.
#![allow(clippy::unwrap_used)]

use super::super::tests::{
    EV, Fallible, apply, close, created_curves, edit, hosted, set, sketch, sketched,
};
use pcbcraft_core::geometry::Point;
use pcbcraft_core::ids::Id;
use pcbcraft_core::layout::LayoutCommand;
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::layout::fill::FillCommand;
use pcbcraft_core::layout::outline::{BoardEdit, BoardShapeEvaluator};
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, Expr, GeomConstraint, SketchCurveId, SketchEdit, SketchPointId, SketchRef,
};
use pcbcraft_core::library::{Outline, Segment};
use pcbcraft_core::model::board::ConstraintTarget;
use pcbcraft_core::model::board::outline_bounds;
use pcbcraft_core::model::dimension::{
    BoardRef, BoxSide, FeatureRef, PadPoint, PartAxis, PartDimensionSpec, PartRelation,
};
use pcbcraft_core::model::fill::{FillRegion, FillSpec};
use pcbcraft_core::{CopperLayer, Engine, Layer, Operation, Outcome};

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
fn the_showcase_header_is_held_by_part_dimensions() -> Fallible<()> {
    let mut engine = Engine::showcase_hosted_fixture(hosted())?;
    let project = engine.project();
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

// Edge-constraint tests replaced by their dimension equivalents
// (docs/board-sketch-references.md §5.6).

/// The board point a named edge starts at, by its sketch id.
fn edge_start(engine: &Engine, name: &str) -> Fallible<BoardRef> {
    let s = sketch(engine)?;
    let sk = crate::footprint::check::check_sketch(&s.sketch.0)
        .map_err(|p| format!("{}: {}", p.code, p.message))?;
    let c = curve(engine, name)?;
    let index = sk.curve_index(&c.0).ok_or("no curve")?;
    let Some(solidcraft_engine::sketch::CurveKind::Line { a, .. }) =
        sk.curves.get(index).map(|c| &c.kind)
    else {
        return Err("not a line".into());
    };
    let id = sk.points.get(*a).ok_or("no point")?.id.clone();
    Ok(BoardRef::Board(SketchRef::Point(SketchPointId(id))))
}

fn axis(part: Id, axis: PartAxis) -> BoardRef {
    BoardRef::Part(FeatureRef::Axis {
        target: ConstraintTarget::Placement(part),
        axis,
    })
}

fn relation(kind: PartRelation, refs: Vec<BoardRef>, value: Option<DimValue>) -> PartDimensionSpec {
    PartDimensionSpec {
        kind,
        refs,
        value,
        reference: false,
    }
}

/// `offset`, `along` and `follow_edge` of an edge constraint as
/// dimensions: 4 mm off the right edge, half the height up from its start,
/// the x axis along the edge. The part sits at (56, 20) turned 90°, follows
/// the width and height, refuses a dimension that does not evaluate and a
/// move (it is fully held), and a dimension to deleted board geometry is a
/// lost reference that keeps the pose it gave.
#[test]
fn a_part_held_by_expressions_follows_the_board() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, other) = parts(&engine)?;
    let right = edge(&engine, "right")?;
    add(
        &mut engine,
        distance(
            vec![origin(part), right.clone()],
            DimValue::Expr(Expr("4 mm".into())),
        ),
    )?;
    let start = edge_start(&engine, "right")?;
    let along = add(
        &mut engine,
        relation(
            PartRelation::Dim(DimKind::VerticalDistance),
            vec![start, origin(part)],
            Some(DimValue::Expr(Expr("height / 2".into()))),
        ),
    )?;
    add(
        &mut engine,
        relation(
            PartRelation::Dim(DimKind::Angle),
            vec![axis(part, PartAxis::X), right],
            Some(DimValue::Fixed(0.0)),
        ),
    )?;
    let (p, r, _) = pose(&engine, part)?;
    assert!(close(p, [56.0, 20.0]), "{p:?}");
    assert!((r - 90.0).abs() < 1e-6, "{r}");
    edit(
        &mut engine,
        vec![set("width", "80 mm"), set("height", "50 mm")],
    )?;
    let (p, r, _) = pose(&engine, part)?;
    assert!(close(p, [76.0, 25.0]), "{p:?}");
    assert!((r - 90.0).abs() < 1e-6, "{r}");
    // A value that does not evaluate is refused.
    let rev = engine.revision();
    let mut broken = engine
        .project()
        .part_dimensions
        .iter()
        .find(|d| d.id == along)
        .cloned()
        .ok_or("dimension")?;
    broken.value = Some(DimValue::Expr(Expr("no_such_parameter".into())));
    assert!(
        apply(
            &mut engine,
            Operation::Dimension(DimensionCommand::Edit { dimension: broken })
        )
        .is_err()
    );
    // Moving the fully held part is refused in the held words.
    let refused = move_to(&mut engine, part, [10.0, 10.0])?
        .err()
        .ok_or("moved")?
        .to_string();
    assert!(
        refused.contains("is held 4 mm from the right edge"),
        "{refused}"
    );
    assert_eq!(engine.revision(), rev);
    // A dimension to a construction line, then the line deleted: a lost
    // reference, reported, and the part keeps its pose.
    let Outcome::BoardEdited { created } = edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddLine {
                a: [30.0, 0.0],
                b: [30.0, 50.0],
                construction: true,
            },
        }],
    )?
    else {
        return Err("no edit".into());
    };
    let line = created
        .iter()
        .find(|r| matches!(r, SketchRef::Curve(_)))
        .cloned()
        .ok_or("no line")?;
    add(
        &mut engine,
        distance(
            vec![origin(other), BoardRef::Board(line.clone())],
            DimValue::Fixed(5.0),
        ),
    )?;
    let held = pose(&engine, other)?;
    assert!((held.0[0] - 25.0).abs() < 1e-6, "{held:?}");
    assert!(pcbcraft_core::layout::dimensions::checks(engine.project()).is_empty());
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::Delete { ids: vec![line] },
        }],
    )?;
    let findings = pcbcraft_core::layout::checks(engine.project());
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.code == pcbcraft_core::layout::dimensions::REFERENCE_LOST)
            .count(),
        1,
        "{findings:#?}"
    );
    assert_eq!(pose(&engine, other)?, held);
    edit(&mut engine, vec![set("width", "70 mm")])?;
    assert_eq!(pose(&engine, other)?, held);
    Ok(())
}

/// An arc edge (§12 note 2: no arc-length constraint at our pin): a part
/// 3 mm in from a round board's rim, on the construction line from the
/// centre to the rim's middle, its x axis square to that radius (the
/// tangent there), sits at the top, (30, 37), turned 180°.
#[test]
fn a_part_is_held_to_an_arc_edge() -> Fallible<()> {
    let mut engine = sketched()?;
    let s = sketch(&engine)?;
    let drawn = EV.apply_sketch(
        &s.sketch,
        &SketchEdit::AddArc {
            center: [30.0, 20.0],
            start: [50.0, 20.0],
            end: [10.0, 20.0],
            construction: false,
        },
    )?;
    let upper = created_curves(&drawn);
    let drawn2 = EV.apply_sketch(
        &drawn.sketch,
        &SketchEdit::AddArc {
            center: [30.0, 20.0],
            start: [10.0, 20.0],
            end: [50.0, 20.0],
            construction: false,
        },
    )?;
    let drawn3 = EV.apply_sketch(
        &drawn2.sketch,
        &SketchEdit::AddLine {
            a: [30.0, 20.0],
            b: [30.0, 40.0],
            construction: true,
        },
    )?;
    let radial = created_curves(&drawn3);
    let old: Vec<SketchRef> = s
        .solved
        .outer_curves
        .iter()
        .map(|c| SketchRef::Curve(c.clone()))
        .collect();
    let sketch_edit = |edit| BoardEdit::Sketch { edit };
    edit(
        &mut engine,
        vec![
            sketch_edit(SketchEdit::AddArc {
                center: [30.0, 20.0],
                start: [50.0, 20.0],
                end: [10.0, 20.0],
                construction: false,
            }),
            sketch_edit(SketchEdit::AddArc {
                center: [30.0, 20.0],
                start: [10.0, 20.0],
                end: [50.0, 20.0],
                construction: false,
            }),
            sketch_edit(SketchEdit::AddLine {
                a: [30.0, 20.0],
                b: [30.0, 40.0],
                construction: true,
            }),
            BoardEdit::NameEdge {
                name: pcbcraft_core::model::board::EdgeName("rim".into()),
                curve: upper.first().cloned().ok_or("arc")?,
            },
            sketch_edit(SketchEdit::Delete { ids: old }),
        ],
    )?;
    let (part, _) = parts(&engine)?;
    // Inside the rim first, so the distance measures inward.
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::Move {
            instance: part,
            position: [30.0, 30.0],
            rotation_deg: 150.0,
            layer: Layer::Top,
        }),
    )?;
    let rim = edge(&engine, "rim")?;
    let radial = BoardRef::Board(SketchRef::Curve(radial.first().cloned().ok_or("line")?));
    add(
        &mut engine,
        distance(vec![origin(part), rim], DimValue::Fixed(3.0)),
    )?;
    add(
        &mut engine,
        relation(
            PartRelation::Geom(GeomConstraint::PointOnCurve),
            vec![origin(part), radial.clone()],
            None,
        ),
    )?;
    add(
        &mut engine,
        relation(
            PartRelation::Dim(DimKind::Angle),
            vec![radial, axis(part, PartAxis::X)],
            Some(DimValue::Fixed(90.0)),
        ),
    )?;
    let (p, r, _) = pose(&engine, part)?;
    assert!(close(p, [30.0, 37.0]), "{p:?}");
    assert!((r - 180.0).abs() < 1e-6, "{r}");
    Ok(())
}

fn square(at: Point, side: f64) -> Outline {
    Outline {
        start: at,
        segments: vec![
            Segment::Line {
                to: [at[0] + side, at[1]],
            },
            Segment::Line {
                to: [at[0] + side, at[1] + side],
            },
            Segment::Line {
                to: [at[0], at[1] + side],
            },
            Segment::Line { to: at },
        ],
    }
}

/// A drawn fill is dimensioned by its outline start and only translates:
/// its top side 1 mm under the top edge, centred on the board's width; it
/// follows the width, and deleting it leaves its two dimensions' references
/// lost.
#[test]
fn fills_translate_to_their_dimensions_and_report_when_gone() -> Fallible<()> {
    let mut engine = sketched()?;
    let Outcome::FillAdded { fill } = apply(
        &mut engine,
        Operation::Fill(FillCommand::Add {
            fill: Box::new(FillSpec {
                region: FillRegion::Drawn,
                name: "GND".into(),
                net: None,
                layer: CopperLayer::Bottom,
                outline: square([20.0, 20.0], 10.0),
                priority: 0,
                settings: Default::default(),
            }),
        }),
    )?
    else {
        return Err("no fill".into());
    };
    let at = BoardRef::Part(FeatureRef::Origin {
        target: ConstraintTarget::Fill(fill),
    });
    let (top, left) = (edge(&engine, "top")?, edge(&engine, "left")?);
    add(
        &mut engine,
        distance(vec![at.clone(), top], DimValue::Fixed(11.0)),
    )?;
    add(
        &mut engine,
        distance(
            vec![at, left],
            DimValue::Expr(Expr("width / 2 - 5 mm".into())),
        ),
    )?;
    let bounds = |engine: &Engine| {
        engine
            .project()
            .fill(fill)
            .map(|f| outline_bounds(&f.outline))
    };
    let b = bounds(&engine).ok_or("fill")?;
    assert!(
        close(b.min, [25.0, 29.0]) && close(b.max, [35.0, 39.0]),
        "{b:?}"
    );
    edit(&mut engine, vec![set("width", "80 mm")])?;
    let b = bounds(&engine).ok_or("fill")?;
    assert!(
        close(b.min, [35.0, 29.0]) && close(b.max, [45.0, 39.0]),
        "{b:?}"
    );
    apply(
        &mut engine,
        Operation::Fill(FillCommand::Delete { id: fill }),
    )?;
    let lost = pcbcraft_core::layout::checks(engine.project())
        .into_iter()
        .filter(|f| f.code == pcbcraft_core::layout::dimensions::REFERENCE_LOST)
        .count();
    assert_eq!(lost, 2);
    Ok(())
}

/// The courtyard as features: its bottom side on the bottom edge and its
/// centre 20 mm from the left edge put the part's extent on the edge,
/// centred at x = 20 (an edge constraint's near-side alignment).
#[test]
fn courtyard_features_put_the_part_extent_on_the_edge() -> Fallible<()> {
    let mut engine = sketched()?;
    let (part, _) = parts(&engine)?;
    let courtyard = |side| {
        BoardRef::Part(FeatureRef::Courtyard {
            placement: part,
            side,
        })
    };
    let (bottom, left) = (edge(&engine, "bottom")?, edge(&engine, "left")?);
    add(
        &mut engine,
        distance(
            vec![courtyard(Some(BoxSide::Bottom)), bottom],
            DimValue::Fixed(0.0),
        ),
    )?;
    add(
        &mut engine,
        distance(vec![courtyard(None), left], DimValue::Fixed(20.0)),
    )?;
    let project = engine.project();
    let mut pts: Vec<Point> = pcbcraft_core::layout::courtyard::placed_courtyards(project)
        .into_iter()
        .filter(|c| c.instance == part)
        .flat_map(|c| c.shape.flatten(1e-4))
        .collect();
    if pts.is_empty() {
        pts = pcbcraft_core::layout::pads(project)
            .into_iter()
            .filter(|p| p.instance == part)
            .flat_map(|p| p.outline().flatten(1e-4))
            .collect();
    }
    let min_y = pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
    let (min_x, max_x) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    assert!(min_y.abs() < 1e-3, "the extent touches the edge: {min_y}");
    assert!(((min_x + max_x) / 2.0 - 20.0).abs() < 1e-3);
    Ok(())
}

/// Timing of the parts solve against §9's target (any edit re-solves within
/// 16 ms with 100 dimensioned parts). Run in release:
/// `cargo test --release -p pcbcraft-solidcraft parts_solve_timing -- --ignored --nocapture`.
/// 100 parts on a 200 mm board: 50 held to the right and bottom edges, 50
/// held to one of those by a horizontal and a vertical distance (pairs), each
/// with its origin and a pad-sized line feature; solved as one request (the
/// worst case: `follow` splits it into the 50 components), once at the
/// stored poses and once after the board grew 20 mm.
#[test]
#[ignore = "timing; run in release"]
fn parts_solve_timing() -> Fallible<()> {
    use pcbcraft_core::layout::dimensions::{FeatureGeometry, PartPose, PartsRequest, SolvePart};
    use pcbcraft_core::model::dimension::PartDimension;
    let mut times = Vec::new();
    for width in [200.0, 220.0] {
        let board = EV.rectangle([width, 200.0])?;
        let right = BoardRef::Board(SketchRef::Curve(
            board
                .edges
                .iter()
                .find(|e| e.name.0 == "right")
                .ok_or("right")?
                .curve
                .clone(),
        ));
        let bottom = BoardRef::Board(SketchRef::Curve(
            board
                .edges
                .iter()
                .find(|e| e.name.0 == "bottom")
                .ok_or("bottom")?
                .curve
                .clone(),
        ));
        let mut parts = Vec::new();
        let mut dims = Vec::new();
        let side = BoardRef::Part(FeatureRef::Courtyard {
            placement: 0,
            side: Some(BoxSide::Right),
        });
        for i in 0..100u64 {
            let (row, col) = (i % 10, i / 10);
            let at = [10.0 + 18.0 * col as f64, 10.0 + 18.0 * row as f64];
            let side = match &side {
                BoardRef::Part(FeatureRef::Courtyard { side, .. }) => FeatureRef::Courtyard {
                    placement: i,
                    side: *side,
                },
                _ => return Err("feature".into()),
            };
            parts.push(SolvePart {
                target: ConstraintTarget::Placement(i),
                pose: PartPose {
                    position: at,
                    rotation_deg: 0.0,
                },
                mirrored: false,
                features: vec![
                    (
                        FeatureRef::Origin {
                            target: ConstraintTarget::Placement(i),
                        },
                        FeatureGeometry::Point { at: [0.0, 0.0] },
                    ),
                    (
                        side,
                        FeatureGeometry::Line {
                            a: [1.0, -0.5],
                            b: [1.0, 0.5],
                        },
                    ),
                ],
            });
            let dim = |id: u64, kind, refs, v: f64| PartDimension {
                id,
                kind: PartRelation::Dim(kind),
                refs,
                value: Some(DimValue::Fixed(v)),
                reference: false,
            };
            if i < 50 {
                dims.push(dim(
                    1000 + 2 * i,
                    DimKind::Distance,
                    vec![origin(i), right.clone()],
                    200.0 - at[0],
                ));
                dims.push(dim(
                    1001 + 2 * i,
                    DimKind::Distance,
                    vec![origin(i), bottom.clone()],
                    at[1],
                ));
            } else {
                let o = i - 50;
                let other = [10.0 + 18.0 * (o / 10) as f64, 10.0 + 18.0 * (o % 10) as f64];
                dims.push(dim(
                    1000 + 2 * i,
                    DimKind::HorizontalDistance,
                    vec![origin(o), origin(i)],
                    (at[0] - other[0]).abs(),
                ));
                dims.push(dim(
                    1001 + 2 * i,
                    DimKind::VerticalDistance,
                    vec![origin(o), origin(i)],
                    (at[1] - other[1]).abs(),
                ));
            }
        }
        let request = PartsRequest {
            board: &board,
            parts: &parts,
            dimensions: &dims,
            drags: &[],
        };
        let mut runs = Vec::new();
        for _ in 0..15 {
            let start = std::time::Instant::now();
            let solution = super::solve(&request)?;
            runs.push(start.elapsed().as_secs_f64() * 1e3);
            assert_eq!(solution.poses.len(), 100);
        }
        runs.sort_by(f64::total_cmp);
        times.push((width, runs[runs.len() / 2], runs[runs.len() - 1]));
    }
    for (width, median, max) in times {
        println!(
            "parts solve, 100 parts, board {width} mm: median {median:.2} ms, max {max:.2} ms"
        );
    }
    Ok(())
}

/// The showcase with its top edge, (42, 28) → (2, 28) between the corner
/// fillets, replaced by a spline through (22, 29.5) between those ends, as
/// in the second VM test drive.
fn showcase_with_a_spline_top() -> Fallible<Engine> {
    let mut engine = Engine::showcase_hosted_fixture(hosted())?;
    let top = curve(&engine, "top")?;
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::Delete {
                ids: vec![SketchRef::Curve(top)],
            },
        }],
    )?;
    edit(
        &mut engine,
        vec![BoardEdit::AddSpline {
            points: vec![[42.0, 28.0], [22.0, 29.5], [2.0, 28.0]],
            control: false,
            periodic: false,
            construction: false,
        }],
    )?;
    Ok(engine)
}

/// Second VM test drive, issue 5: replacing the showcase's top edge with a
/// spline retires the name `top` with its line, but J1 is dimensioned to
/// the left and bottom edges, which keep their names and curves, so no
/// reference is lost. `height` 28 → 34 then puts J1 where its own value,
/// `height / 2 + 1 mm` from the bottom edge, says: (6, 18), exactly where
/// it lands on the untouched showcase. It does not jump elsewhere.
#[test]
fn a_spline_top_edge_keeps_the_header_on_its_own_edges() -> Fallible<()> {
    let mut plain = Engine::showcase_hosted_fixture(hosted())?;
    let mut engine = showcase_with_a_spline_top()?;
    let names: Vec<String> = sketch(&engine)?
        .edges
        .iter()
        .map(|e| e.name.0.clone())
        .collect();
    assert_eq!(names, ["bottom", "right", "left"]);
    let j1 = |engine: &Engine| -> Fallible<Id> {
        Ok(engine
            .project()
            .instance_by_designator("J1")
            .ok_or("J1")?
            .id)
    };
    let id = j1(&engine)?;
    assert!(close(pose(&engine, id)?.0, [6.0, 15.0]));
    for e in [&mut plain, &mut engine] {
        edit(e, vec![set("height", "34 mm")])?;
    }
    assert!(close(pose(&engine, id)?.0, [6.0, 18.0]));
    assert_eq!(pose(&engine, id)?, pose(&plain, j1(&plain)?)?);
    assert!(pcbcraft_core::layout::dimensions::checks(engine.project()).is_empty());
    Ok(())
}

/// Second VM test drive (§17): with the showcase's top line deleted and a
/// spline drawn between the corner fillets in its place, `height` holds only the virtual
/// top-right corner, which no curve uses, so it moves no outline edge. A
/// warning says so in words, naming the parameter and no solver id; the
/// untouched showcase has none.
#[test]
fn a_parameter_that_drives_no_outline_edge_is_reported() -> Fallible<()> {
    let idle = |engine: &Engine| -> Vec<String> {
        pcbcraft_core::layout::outline::checks(engine.project())
            .into_iter()
            .filter(|f| f.code == pcbcraft_core::layout::outline::PARAM_IDLE)
            .map(|f| f.message)
            .collect()
    };
    assert!(idle(&Engine::showcase_hosted_fixture(hosted())?).is_empty());
    let engine = showcase_with_a_spline_top()?;
    assert_eq!(
        idle(&engine),
        [
            "The board parameter ‘height’ no longer drives any outline edge: the dimensions it \
          sets move nothing on the outline. Dimension an outline edge to it, or remove the \
          parameter."
        ]
    );
    let pcbcraft_core::QueryResult::Preflight(report) =
        engine.query(pcbcraft_core::Query::Preflight { profile: None })?
    else {
        return Err("preflight".into());
    };
    assert!(report.items.iter().any(|i| {
        i.code == pcbcraft_core::layout::outline::PARAM_IDLE
            && i.severity == pcbcraft_core::issues::Severity::Warning
            && !i.blocks()
    }));
    Ok(())
}

fn param_idle(engine: &Engine) -> Vec<String> {
    pcbcraft_core::layout::outline::checks(engine.project())
        .into_iter()
        .filter(|f| f.code == pcbcraft_core::layout::outline::PARAM_IDLE)
        .map(|f| f.message)
        .collect()
}

fn is_construction(engine: &Engine, id: &SketchCurveId) -> Fallible<bool> {
    let sk = crate::footprint::check::check_board_sketch(&sketch(engine)?.sketch.0)
        .map_err(|p| p.message)?;
    let c = sk.curves.iter().find(|c| c.id == id.0).ok_or("curve")?;
    Ok(c.construction)
}

/// §17 follow-up: a spline or conic drawn over the showcase's top line
/// (between the corner fillets) replaces it. The line stays as construction,
/// out of the outline, with its tangencies and the virtual corner `height`
/// holds, so `height` moves the curve's ends with the corners and the
/// outline stays closed; no parameter is idle. Deleting the construction
/// line on purpose leaves `height` idle, which the warning reports.
#[test]
fn a_curve_over_a_filleted_edge_keeps_the_board_parameters_driving_it() -> Fallible<()> {
    use pcbcraft_core::layout::outline::{SHAPE_INVALID, SHAPE_OPEN};
    let curves = [
        BoardEdit::AddSpline {
            points: vec![[42.0, 28.0], [22.0, 29.5], [2.0, 28.0]],
            control: false,
            periodic: false,
            construction: false,
        },
        BoardEdit::AddConic {
            a: [42.0, 28.0],
            b: [2.0, 28.0],
            apex: [22.0, 31.0],
            rho: 0.5,
            construction: false,
        },
    ];
    let faults = |engine: &Engine| -> Vec<String> {
        pcbcraft_core::layout::outline::checks(engine.project())
            .into_iter()
            .filter(|f| f.code == SHAPE_OPEN || f.code == SHAPE_INVALID)
            .map(|f| f.message)
            .collect()
    };
    for curve in curves {
        let mut engine = Engine::showcase_hosted_fixture(hosted())?;
        let top = super::super::tests::created_curves_of(apply(
            &mut engine,
            Operation::Board(pcbcraft_core::layout::outline::BoardCommand::Edit {
                edits: vec![curve],
            }),
        )?);
        assert_eq!(top.len(), 1, "{top:?}");
        let line = self::curve(&engine, "top")?;
        assert!(is_construction(&engine, &line)?);
        assert!(!is_construction(&engine, &top[0])?);
        assert!(faults(&engine).is_empty(), "{:?}", faults(&engine));
        assert!(param_idle(&engine).is_empty(), "{:?}", param_idle(&engine));
        assert!(engine.project().board_size[1] >= 28.0 - 1e-6);

        edit(&mut engine, vec![set("height", "34 mm")])?;
        assert!(faults(&engine).is_empty(), "{:?}", faults(&engine));
        assert!(param_idle(&engine).is_empty(), "{:?}", param_idle(&engine));
        let board = engine.project().board().into_owned();
        let poly = pcbcraft_core::model::board::flatten(&board.outer, 1e-3);
        for end in [[42.0, 34.0], [2.0, 34.0]] {
            assert!(
                poly.iter()
                    .any(|p| (p[0] - end[0]).hypot(p[1] - end[1]) < 1e-6),
                "{end:?} not on {poly:?}"
            );
        }
        assert!(engine.project().board_size[1] >= 34.0 - 1e-6);

        edit(
            &mut engine,
            vec![BoardEdit::Sketch {
                edit: SketchEdit::Delete {
                    ids: vec![SketchRef::Curve(line)],
                },
            }],
        )?;
        assert_eq!(param_idle(&engine).len(), 1, "{:?}", param_idle(&engine));
        assert!(param_idle(&engine)[0].contains("‘height’"));
    }
    Ok(())
}

/// Construction geometry reaches no fabrication output: the showcase with a
/// construction line drawn across it writes the same bytes as the unchanged
/// showcase.
#[test]
fn a_construction_line_leaves_the_fabrication_output_unchanged() -> Fallible<()> {
    use pcbcraft_core::fabrication::{FabricationOptions, generate};
    let plain = Engine::showcase_hosted_fixture(hosted())?;
    let mut engine = Engine::showcase_hosted_fixture(hosted())?;
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddLine {
                a: [-5.0, 14.0],
                b: [50.0, 14.0],
                construction: true,
            },
        }],
    )?;
    assert_ne!(sketch(&engine)?.sketch, sketch(&plain)?.sketch);
    let options = FabricationOptions::default();
    assert_eq!(
        generate(engine.project(), &options)?,
        generate(plain.project(), &options)?
    );
    Ok(())
}
