//! The board shape on Solidcraft's sketch solver: a rectangle sketch driven
//! by its parameters, fillets, cutouts, slots and mounting holes, parts
//! placed relative to named edges that move with the parameters, refused
//! (over-constrained, open, unevaluable) edits, persistence and the 3D
//! laminate.
#![allow(clippy::unwrap_used)]

use super::*;
use pcbcraft_core::layout::LayoutCommand;
use pcbcraft_core::layout::outline::{BoardCommand, BoardEdit, edge_pose};
use pcbcraft_core::library::footprint_source::{DimKind, SketchRef};
use pcbcraft_core::model::board::{
    BoardShape, ConstraintAlign, ConstraintTarget, CutoutKind, EdgeAnchor, EdgeConstraintSpec,
};
use pcbcraft_core::{Engine, Evaluators, Layer, Operation, Outcome, Query, QueryResult, format};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const EV: SketchBoardEvaluator = SketchBoardEvaluator;

fn hosted() -> Evaluators<'static> {
    Evaluators {
        footprint: Some(&crate::footprint::SketchEvaluator),
        board: Some(&SketchBoardEvaluator),
    }
}

fn apply(engine: &mut Engine, op: Operation) -> pcbcraft_core::Result<Outcome> {
    engine.apply_hosted(engine.revision(), op, hosted())
}

fn edit(engine: &mut Engine, edits: Vec<BoardEdit>) -> pcbcraft_core::Result<Outcome> {
    apply(engine, Operation::Board(BoardCommand::Edit { edits }))
}

fn sketch(engine: &Engine) -> Fallible<BoardSketch> {
    match &engine.project().board_shape {
        BoardShape::Sketch(s) => Ok((**s).clone()),
        BoardShape::Rectangle => Err("not a sketch".into()),
    }
}

fn set(name: &str, expr: &str) -> BoardEdit {
    BoardEdit::SetParameter {
        param: FootprintParam {
            name: ParamName(name.into()),
            expr: Expr(expr.into()),
            unit: ParamUnit::Mm,
            comment: String::new(),
        },
    }
}

fn close(a: Point, b: Point) -> bool {
    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
}

fn sketched() -> Fallible<Engine> {
    let mut engine = Engine::demo()?;
    apply(&mut engine, Operation::Board(BoardCommand::StartSketch))?;
    Ok(engine)
}

fn created_curves(outcome: &SketchOutcome) -> Vec<SketchCurveId> {
    outcome
        .created
        .iter()
        .filter_map(|r| match r {
            SketchRef::Curve(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_rectangle_sketch_solves_to_the_plain_rectangle() -> Fallible<()> {
    let board = EV.rectangle([60.0, 40.0])?;
    assert_eq!(board.solve.status, SolveStatus::Solved);
    assert_eq!(board.solve.dof, 0);
    let plain = SolvedBoard::rectangle([60.0, 40.0]);
    assert_eq!(board.solved.edges, plain.edges);
    assert_eq!(board.solved.outer.segments.len(), 4);
    assert_eq!(board.solved.bounds(), plain.bounds());
    // Re-evaluating the stored result changes nothing.
    let again = EV.evaluate(&board, &[])?;
    assert_eq!(again.solved, board.solved);
    assert_eq!(again.solve, board.solve);
    Ok(())
}

#[test]
fn board_parameters_drive_the_shape_and_board_size() -> Fallible<()> {
    let mut engine = sketched()?;
    assert_eq!(engine.project().board_size, [60.0, 40.0]);
    edit(
        &mut engine,
        vec![set("width", "75 mm"), set("height", "width / 3")],
    )?;
    assert_eq!(engine.project().board_size, [75.0, 25.0]);
    let QueryResult::BoardShape(board) = engine.query(Query::BoardShape {})? else {
        return Err("board".into());
    };
    let right = board.edge(&EdgeName("right".into())).ok_or("right")?;
    assert!(close(right.a, [75.0, 0.0]) && close(right.b, [75.0, 25.0]));
    // A sketch board's size is its outline's: the plain size command is
    // refused rather than desynchronising them.
    let rev = engine.revision();
    assert!(
        engine
            .apply(
                rev,
                Operation::Layout(LayoutCommand::SetBoardSize { size: [50.0, 50.0] })
            )
            .is_err()
    );
    assert_eq!(engine.revision(), rev);
    // Back to a rectangle.
    apply(
        &mut engine,
        Operation::Board(BoardCommand::SetRectangle { size: [50.0, 30.0] }),
    )?;
    assert!(engine.project().board_shape.is_rectangle());
    Ok(())
}

#[test]
fn rounded_corners_follow_a_corner_radius_parameter() -> Fallible<()> {
    let mut engine = sketched()?;
    edit(
        &mut engine,
        vec![
            set("corner_r", "3 mm"),
            BoardEdit::Fillet {
                corners: Vec::new(),
                radius: DimValue::Param(ParamName("corner_r".into())),
            },
        ],
    )?;
    let s = sketch(&engine)?;
    assert_eq!(s.solve.status, SolveStatus::Solved, "{:?}", s.solve);
    assert_eq!(s.outline.len(), 8);
    let arcs = s
        .solved
        .outer
        .segments
        .iter()
        .filter(|s| matches!(s, Segment::Arc { .. }))
        .count();
    assert_eq!(arcs, 4);
    // The box is unchanged and the named edges still run corner to corner
    // (to the virtual corners the fillets leave).
    assert_eq!(engine.project().board_size, [60.0, 40.0]);
    let bottom = s.solved.edge(&EdgeName("bottom".into())).ok_or("bottom")?;
    assert!(
        close(bottom.a, [0.0, 0.0]) && close(bottom.b, [60.0, 0.0]),
        "{bottom:?}"
    );
    // The board no longer contains its sharp corner.
    assert!(!s.solved.contains([0.2, 0.2], 0.001));
    assert!(s.solved.contains([3.0, 3.0], 0.001));
    // The laminate in 3D has the rounded corners: it lacks exactly the
    // four corner pieces the rectangle has.
    let volume = |project: &pcbcraft_core::Project| -> Fallible<f64> {
        Ok(crate::project::build(project)?
            .geometry
            .iter()
            .filter(|g| g.kind == crate::material::BodyKind::Substrate)
            .map(|g| g.volume)
            .sum())
    };
    let mut plain = engine.project().clone();
    plain.board_shape = BoardShape::Rectangle;
    let removed = volume(&plain)? - volume(engine.project())?;
    let corner = 4.0 * (9.0 - std::f64::consts::PI * 9.0 / 4.0);
    let want = corner * engine.project().stackup.dielectric_mm();
    assert!((removed - want).abs() < 0.05 * want, "{removed} vs {want}");
    // A larger radius re-solves the same sketch.
    edit(
        &mut engine,
        vec![set("corner_r", "8 mm"), set("width", "70 mm")],
    )?;
    let s = sketch(&engine)?;
    assert_eq!(s.solve.status, SolveStatus::Solved);
    assert_eq!(engine.project().board_size, [70.0, 40.0]);
    assert!(!s.solved.contains([1.5, 1.5], 0.001));
    // A radius the lines cannot hold is refused.
    let rev = engine.revision();
    assert!(edit(&mut engine, vec![set("corner_r", "30 mm")]).is_err());
    assert_eq!(engine.revision(), rev);
    Ok(())
}

#[test]
fn cutouts_slots_and_mounting_holes_solve_and_build_in_3d() -> Fallible<()> {
    let mut engine = sketched()?;
    let s = sketch(&engine)?;
    let circle = EV.apply_sketch(
        &s.sketch,
        &SketchEdit::AddCircle {
            center: [30.0, 20.0],
            radius: 4.0,
            construction: false,
        },
    )?;
    let curves = created_curves(&circle);
    edit(
        &mut engine,
        vec![
            BoardEdit::Sketch {
                edit: SketchEdit::AddCircle {
                    center: [30.0, 20.0],
                    radius: 4.0,
                    construction: false,
                },
            },
            BoardEdit::AddCutout {
                kind: CutoutKind::Cutout,
                curves,
            },
            BoardEdit::AddSlot {
                a: [10.0, 30.0],
                b: [20.0, 30.0],
                width: DimValue::Fixed(2.0),
                plated: false,
            },
            BoardEdit::Sketch {
                edit: SketchEdit::AddPoint { at: [4.0, 4.0] },
            },
        ],
    )?;
    let s = sketch(&engine)?;
    let stored = check::check_sketch(&s.sketch.0).map_err(Error::from)?;
    let hole_point = stored
        .points
        .iter()
        .find(|p| p.pos.x == 4.0 && p.pos.y == 4.0)
        .map(|p| SketchPointId(p.id.clone()))
        .ok_or("hole point")?;
    edit(
        &mut engine,
        vec![BoardEdit::AddMountingHole {
            center: hole_point,
            drill: Expr("3.2 mm".into()),
            plated: true,
            pad_diameter: Some(Expr("6 mm".into())),
        }],
    )?;
    let board = engine.project().board().into_owned();
    assert_eq!(board.cutouts.len(), 2);
    assert_eq!(board.holes.len(), 1);
    assert!(!board.contains([30.0, 20.0], 0.001), "inside the cutout");
    assert!(!board.contains([15.0, 30.0], 0.001), "inside the slot");
    assert!(board.contains([15.0, 33.0], 0.001));
    let slot = board
        .cutouts
        .iter()
        .find(|c| matches!(c.kind, CutoutKind::Slot { .. }))
        .ok_or("slot")?;
    let b = pcbcraft_core::model::board::outline_bounds(&slot.outline);
    assert!(
        close(b.min, [9.0, 29.0]) && close(b.max, [21.0, 31.0]),
        "{b:?}"
    );

    // A cutout outside the board is refused.
    let rev = engine.revision();
    let outside = EV.apply_sketch(
        &sketch(&engine)?.sketch,
        &SketchEdit::AddCircle {
            center: [100.0, 20.0],
            radius: 2.0,
            construction: false,
        },
    )?;
    assert!(
        edit(
            &mut engine,
            vec![
                BoardEdit::Sketch {
                    edit: SketchEdit::AddCircle {
                        center: [100.0, 20.0],
                        radius: 2.0,
                        construction: false,
                    },
                },
                BoardEdit::AddCutout {
                    kind: CutoutKind::Cutout,
                    curves: created_curves(&outside),
                },
            ],
        )
        .is_err()
    );
    assert_eq!(engine.revision(), rev);

    // The laminate is the outline with its cutouts, slot and hole cut.
    let laminate = laminate(engine.project()).ok_or("laminate")?;
    assert_eq!(laminate.through.len(), 3);
    let substrate = |project: &pcbcraft_core::Project| -> Fallible<f64> {
        let model = crate::project::build(project)?;
        Ok(model
            .geometry
            .iter()
            .filter(|g| g.kind == crate::material::BodyKind::Substrate)
            .map(|g| g.volume)
            .sum())
    };
    let shaped = substrate(engine.project())?;
    let mut plain = engine.project().clone();
    plain.board_shape = BoardShape::Rectangle;
    let full = substrate(&plain)?;
    let pi = std::f64::consts::PI;
    let removed = pi * 16.0 + (10.0 * 2.0 + pi) + pi * 1.6 * 1.6;
    let thickness = engine.project().stackup.dielectric_mm();
    let want = removed * thickness;
    assert!(
        ((full - shaped) - want).abs() < 0.02 * want,
        "removed {} mm³, want {want}",
        full - shaped
    );

    // Saved and reopened unchanged; undo returns to the rectangle.
    let file = engine.to_file();
    assert_eq!(format::decode(&format::encode(&file)?)?, file);
    for _ in 0..3 {
        engine.undo()?;
    }
    assert!(engine.project().board_shape.is_rectangle());
    Ok(())
}

#[test]
fn bad_edits_are_refused_and_leave_the_board() -> Fallible<()> {
    let mut engine = sketched()?;
    let s = sketch(&engine)?;
    let bottom = s
        .edges
        .iter()
        .find(|e| e.name.0 == "bottom")
        .map(|e| e.curve.clone())
        .ok_or("bottom")?;
    let rev = engine.revision();
    let before = engine.to_file();
    let refused: Vec<Vec<BoardEdit>> = vec![
        // A second length on the bottom line conflicts with `width`.
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddDimension {
                kind: DimKind::Distance,
                refs: vec![SketchRef::Curve(bottom.clone())],
                value: pcbcraft_core::library::footprint_source::DimValue::Fixed(10.0),
            },
        }],
        // The parameter `width` drives a dimension.
        vec![BoardEdit::RemoveParameter {
            name: ParamName("width".into()),
        }],
        // Deleting an outline curve opens the outline.
        vec![BoardEdit::Sketch {
            edit: SketchEdit::Delete {
                ids: vec![SketchRef::Curve(bottom.clone())],
            },
        }],
        // An open outline.
        vec![BoardEdit::SetOutline {
            curves: vec![bottom],
        }],
        // A negative size.
        vec![set("width", "-5 mm")],
    ];
    for edits in refused {
        let label = format!("{edits:?}");
        assert!(edit(&mut engine, edits).is_err(), "{label}");
        assert_eq!(engine.revision(), rev, "{label}");
        assert_eq!(engine.to_file(), before, "{label}");
    }
    Ok(())
}

#[test]
fn parts_follow_named_edges_when_board_parameters_change() -> Fallible<()> {
    let mut engine = sketched()?;
    let part = engine
        .project()
        .placements
        .first()
        .ok_or("no part")?
        .instance;
    let Outcome::EdgeConstraintAdded { constraint } = apply(
        &mut engine,
        Operation::Board(BoardCommand::Constrain {
            constraint: EdgeConstraintSpec {
                target: ConstraintTarget::Placement(part),
                edge: EdgeName("right".into()),
                anchor: EdgeAnchor::Start,
                along: Expr("height / 2".into()),
                offset: Expr("4 mm".into()),
                align: ConstraintAlign::Origin,
                follow_edge: true,
                rotation_deg: 0.0,
            },
        }),
    )?
    else {
        return Err("no constraint".into());
    };
    let at = |engine: &Engine| {
        engine
            .project()
            .placements
            .iter()
            .find(|p| p.instance == part)
            .map(|p| (p.position, p.rotation_deg))
    };
    let (p, r) = at(&engine).ok_or("placement")?;
    assert!(close(p, [56.0, 20.0]), "{p:?}");
    assert!((r - 90.0).abs() < 1e-6);
    edit(
        &mut engine,
        vec![set("width", "80 mm"), set("height", "50 mm")],
    )?;
    let (p, _) = at(&engine).ok_or("placement")?;
    assert!(close(p, [76.0, 25.0]), "{p:?}");
    // The resolved pose is the edge pose of the solved edge.
    let board = engine.project().board().into_owned();
    let right = board.edge(&EdgeName("right".into())).ok_or("right")?;
    let pose = edge_pose(right, EdgeAnchor::Start, 25.0, 4.0)?;
    assert!(close(pose.position, p));
    // An unknown parameter in a constraint is refused.
    let mut broken = engine
        .project()
        .edge_constraints
        .iter()
        .find(|c| c.id == constraint)
        .cloned()
        .ok_or("constraint")?;
    broken.along = Expr("no_such_parameter".into());
    let rev = engine.revision();
    assert!(
        apply(
            &mut engine,
            Operation::Board(BoardCommand::EditConstraint { constraint: broken })
        )
        .is_err()
    );
    // Moving the part by hand is refused; the move leaves it in place.
    assert!(
        apply(
            &mut engine,
            Operation::Layout(LayoutCommand::Move {
                instance: part,
                position: [10.0, 10.0],
                rotation_deg: 90.0,
                layer: Layer::Top,
            })
        )
        .is_err()
    );
    assert_eq!(engine.revision(), rev);
    // Renaming the edge away leaves an unresolved-constraint finding.
    edit(
        &mut engine,
        vec![BoardEdit::UnnameEdge {
            name: EdgeName("right".into()),
        }],
    )?;
    let findings = pcbcraft_core::layout::outline::checks(engine.project());
    assert!(findings.iter().any(|f| f.code == CONSTRAINT_UNRESOLVED));
    Ok(())
}

#[test]
fn arc_board_edges_place_parts_on_a_round_board() -> Fallible<()> {
    // A round board: one circle as the outline, a named arc edge on a
    // half-circle cutout-free shape drawn from two arcs.
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
    let lower = created_curves(&drawn2);
    let old: Vec<SketchRef> = s
        .outline
        .iter()
        .map(|c| SketchRef::Curve(c.clone()))
        .collect();
    let mut outline = upper.clone();
    outline.extend(lower);
    // Arcs drawn separately meet only by position; the loop joins them.
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
            BoardEdit::SetOutline { curves: outline },
            BoardEdit::NameEdge {
                name: EdgeName("rim".into()),
                curve: upper.first().cloned().ok_or("arc")?,
            },
            sketch_edit(SketchEdit::Delete { ids: old }),
        ],
    )?;
    assert_eq!(engine.project().board_size, [50.0, 40.0]);
    let board = engine.project().board().into_owned();
    let rim = board.edge(&EdgeName("rim".into())).ok_or("rim")?;
    assert!(rim.bulge > 0.0);
    let part = engine
        .project()
        .placements
        .first()
        .ok_or("no part")?
        .instance;
    apply(
        &mut engine,
        Operation::Board(BoardCommand::Constrain {
            constraint: EdgeConstraintSpec {
                target: ConstraintTarget::Placement(part),
                edge: EdgeName("rim".into()),
                anchor: EdgeAnchor::Middle,
                along: Expr("0".into()),
                offset: Expr("3 mm".into()),
                align: ConstraintAlign::Origin,
                follow_edge: true,
                rotation_deg: 0.0,
            },
        }),
    )?;
    let p = engine
        .project()
        .placements
        .iter()
        .find(|p| p.instance == part)
        .ok_or("placement")?;
    // The top of the circle, 3 mm in, facing the tangent there (180°).
    assert!(close(p.position, [30.0, 37.0]), "{:?}", p.position);
    assert!((p.rotation_deg - 180.0).abs() < 1e-6);
    Ok(())
}

/// The display-only layers follow a sketched board: on a board with
/// rounded corners and a round cutout, the mask films lie inside the
/// outline and leave the cutout open, and the edge bands also line the
/// cutout's wall.
#[test]
fn the_mask_and_edge_bands_follow_the_sketched_shape() -> Fallible<()> {
    use crate::material::BodyKind;
    let mut engine = sketched()?;
    let s = sketch(&engine)?;
    let add = SketchEdit::AddCircle {
        center: [30.0, 20.0],
        radius: 4.0,
        construction: false,
    };
    let curves = created_curves(&EV.apply_sketch(&s.sketch, &add)?);
    edit(
        &mut engine,
        vec![
            BoardEdit::Sketch { edit: add },
            BoardEdit::AddCutout {
                kind: CutoutKind::Cutout,
                curves,
            },
        ],
    )?;
    edit(
        &mut engine,
        vec![
            set("corner_r", "5 mm"),
            BoardEdit::Fillet {
                corners: Vec::new(),
                radius: DimValue::Param(ParamName("corner_r".into())),
            },
        ],
    )?;
    let model = crate::project::build(engine.project())?;
    let board = engine.project().board().into_owned();
    let centroids = |kind: BodyKind| -> Vec<[f64; 2]> {
        model
            .geometry
            .iter()
            .filter(|g| g.kind == kind)
            .flat_map(|g| {
                let m = &g.mesh;
                m.triangles.iter().map(move |t| {
                    let p = t.map(|v| m.positions[v as usize]);
                    [
                        (p[0].x + p[1].x + p[2].x) / 3.0,
                        (p[0].y + p[1].y + p[2].y) / 3.0,
                    ]
                })
            })
            .collect()
    };
    for kind in [BodyKind::MaskTop, BodyKind::MaskBottom] {
        let mask = centroids(kind);
        assert!(!mask.is_empty(), "{kind:?}");
        for c in &mask {
            assert!(board.contains(*c, 0.001), "{kind:?} mask at {c:?}");
        }
        // Nothing over the cutout, and the rounded corner is bare.
        assert!(mask.iter().all(|c| (c[0] - 30.0).hypot(c[1] - 20.0) > 4.0));
        assert!(mask.iter().all(|c| c[0] + c[1] > 1.0));
    }
    // An edge band lines the cutout's wall, just proud of it.
    let bands: Vec<&crate::scene::Geometry> = model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::EdgeDielectric)
        .collect();
    assert!(!bands.is_empty());
    let lines_cutout = bands.iter().any(|g| {
        g.mesh.positions.iter().any(|p| {
            let r = (p.x - 30.0).hypot(p.y - 20.0);
            (r - 4.0).abs() < 0.01
        })
    });
    assert!(lines_cutout, "a band around the cutout");
    // And no band stands at the sharp corner the fillet removed.
    assert!(bands.iter().all(|g| {
        g.mesh
            .positions
            .iter()
            .all(|p| p.x.hypot(p.y) > 0.5 && (p.x - 60.0).hypot(p.y) > 0.5)
    }));
    Ok(())
}

/// The shipped showcase (`Engine::showcase_hosted`): a sketched board with
/// rounded corners, a round cutout and a milled slot, a Power class over
/// GND and VCC, a top GND pour, and J1 held to the left edge. Its board
/// checks are clean; J1 follows the `height` parameter, and a corner
/// radius change leaves it in place (edges run between the virtual
/// corners).
#[test]
fn the_shipped_showcase_has_its_shape_classes_and_edge_held_header() -> Fallible<()> {
    use pcbcraft_core::layout::showcase::{CORNER_MM, CUTOUT, SLOT};
    let mut engine = Engine::showcase_hosted(hosted())?;
    let s = sketch(&engine)?;
    // The outline is driven by its parameters; the cutout and slot stand
    // where they were drawn, undimensioned.
    assert_eq!(s.solve.status, SolveStatus::UnderConstrained);
    let board = engine.project().board().into_owned();
    let arcs = board
        .outer
        .segments
        .iter()
        .filter(|s| matches!(s, Segment::Arc { .. }))
        .count();
    assert_eq!(arcs, 4, "rounded corners");
    assert_eq!(engine.project().board_size, [44.0, 28.0]);
    assert!(!board.contains([0.2, 0.2], 0.001));
    assert!(board.contains([CORNER_MM, CORNER_MM], 0.001));
    assert_eq!(board.cutouts.len(), 2);
    assert!(!board.contains(CUTOUT.0, 0.001), "the cutout");
    let slot_middle = [(SLOT.0[0] + SLOT.1[0]) / 2.0, SLOT.0[1]];
    assert!(!board.contains(slot_middle, 0.001), "the slot");
    assert!(
        board
            .cutouts
            .iter()
            .any(|c| matches!(c.kind, CutoutKind::Slot { plated: false }))
    );
    let project = engine.project();
    let power = project
        .rules
        .net_classes
        .iter()
        .find(|c| c.name == "Power")
        .ok_or("Power class")?;
    for name in ["GND", "VCC"] {
        let net = project.nets.iter().find(|n| n.name == name).ok_or("net")?;
        assert_eq!(project.rules.class_of(net.id, &net.name), Some(power.id));
    }
    let top = project
        .fills
        .iter()
        .find(|f| f.layer == pcbcraft_core::CopperLayer::Top)
        .ok_or("top pour")?;
    let copper = top.computed.as_ref().ok_or("top pour filled")?;
    assert!(!copper.regions.is_empty());
    assert!(
        copper.joins.iter().any(|j| matches!(
            j,
            pcbcraft_core::model::fill::FillJoin::Pad {
                connection: pcbcraft_core::rules::PadConnection::Thermal,
                ..
            }
        )),
        "thermals"
    );
    assert!(
        project
            .fills
            .iter()
            .all(|f| !pcbcraft_core::layout::fill::is_stale(project, f))
    );
    let findings = pcbcraft_core::layout::checks(project);
    assert!(findings.is_empty(), "{findings:#?}");
    let j1 = project
        .instance_by_designator("J1")
        .map(|i| i.id)
        .ok_or("J1")?;
    assert_eq!(project.edge_constraints.len(), 1);
    let at = |engine: &Engine| {
        engine
            .project()
            .placements
            .iter()
            .find(|p| p.instance == j1)
            .map(|p| p.position)
    };
    assert!(close(at(&engine).ok_or("J1 placed")?, [6.0, 15.0]));
    edit(&mut engine, vec![set("corner_r", "3 mm")])?;
    assert!(close(at(&engine).ok_or("J1")?, [6.0, 15.0]));
    edit(&mut engine, vec![set("height", "32 mm")])?;
    assert!(close(at(&engine).ok_or("J1")?, [6.0, 17.0]));
    Ok(())
}
