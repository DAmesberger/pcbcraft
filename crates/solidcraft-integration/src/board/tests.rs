//! The board shape on Solidcraft's sketch solver: a rectangle sketch driven
//! by its parameters, fillets, cutouts, slots and mounting holes, refused
//! (over-constrained, open, unevaluable) edits, persistence and the 3D
//! laminate. Parts dimensioned to the board are tested in `parts`.
#![allow(clippy::unwrap_used)]

use super::*;
use sparksmith_core::layout::LayoutCommand;
use sparksmith_core::layout::outline::{BoardCommand, BoardEdit};
use sparksmith_core::library::footprint_source::{DimKind, SketchRef};
use sparksmith_core::model::board::{BoardShape, CutoutKind};
use sparksmith_core::{Engine, Evaluators, Layer, Operation, Outcome, Query, QueryResult, format};

pub(crate) type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(super) const EV: SketchBoardEvaluator = SketchBoardEvaluator;

pub(super) fn hosted() -> Evaluators<'static> {
    Evaluators {
        footprint: Some(&crate::footprint::SketchEvaluator),
        board: Some(&SketchBoardEvaluator),
    }
}

pub(crate) fn apply(engine: &mut Engine, op: Operation) -> sparksmith_core::Result<Outcome> {
    engine.apply_hosted(engine.revision(), op, hosted())
}

pub(super) fn edit(engine: &mut Engine, edits: Vec<BoardEdit>) -> sparksmith_core::Result<Outcome> {
    apply(engine, Operation::Board(BoardCommand::Edit { edits }))
}

pub(super) fn sketch(engine: &Engine) -> Fallible<BoardSketch> {
    match &engine.project().board_shape {
        BoardShape::Sketch(s) => Ok((**s).clone()),
        BoardShape::Rectangle => Err("not a sketch".into()),
    }
}

pub(super) fn set(name: &str, expr: &str) -> BoardEdit {
    BoardEdit::SetParameter {
        param: FootprintParam {
            name: ParamName(name.into()),
            expr: Expr(expr.into()),
            unit: ParamUnit::Mm,
            comment: String::new(),
        },
    }
}

pub(super) fn close(a: Point, b: Point) -> bool {
    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
}

pub(super) fn sketched() -> Fallible<Engine> {
    let mut engine = Engine::demo()?;
    apply(&mut engine, Operation::Board(BoardCommand::StartSketch))?;
    Ok(engine)
}

/// The curves a `board.edit` reports it created.
pub(super) fn created_curves_of(outcome: Outcome) -> Vec<SketchCurveId> {
    match outcome {
        Outcome::BoardEdited { created } => created
            .into_iter()
            .filter_map(|r| match r {
                SketchRef::Curve(c) => Some(c),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn created_curves(outcome: &SketchOutcome) -> Vec<SketchCurveId> {
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
    let again = EV.evaluate(&board)?;
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

/// A parameter that names itself is refused in words, not as a cycle of
/// one ("circular reference: height → height").
#[test]
fn a_parameter_that_refers_to_itself_is_refused_in_words() -> Fallible<()> {
    let mut engine = sketched()?;
    let rev = engine.revision();
    let error = edit(&mut engine, vec![set("height", "height + 2 mm")])
        .err()
        .ok_or("accepted")?
        .to_string();
    assert!(error.contains("height can't refer to itself"), "{error}");
    assert!(
        !error.contains("circular") && !error.contains('`'),
        "{error}"
    );
    assert_eq!(engine.revision(), rev);
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
    assert_eq!(s.solved.outer_curves.len(), 8);
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
    let volume = |project: &sparksmith_core::Project| -> Fallible<f64> {
        Ok(crate::project::build(project)?
            .geometry
            .iter()
            .filter(|g| g.kind == crate::material::BodyKind::Substrate)
            .map(|g| g.volume())
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
    edit(
        &mut engine,
        vec![
            // A circle inside the outline is a cutout by itself.
            BoardEdit::Sketch {
                edit: SketchEdit::AddCircle {
                    center: [30.0, 20.0],
                    radius: 4.0,
                    construction: false,
                },
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
    let b = sparksmith_core::model::board::outline_bounds(&slot.outline);
    assert!(
        close(b.min, [9.0, 29.0]) && close(b.max, [21.0, 31.0]),
        "{b:?}"
    );

    // A circle outside the board is a second region, not a cutout: the
    // board keeps its shape and reports it (§5.1); deleting it again
    // clears the report.
    let kept = engine.project().board().into_owned();
    let outside = created_curves_of(edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddCircle {
                center: [100.0, 20.0],
                radius: 2.0,
                construction: false,
            },
        }],
    )?);
    assert_eq!(engine.project().board().into_owned(), kept);
    let invalid = |engine: &Engine| {
        sparksmith_core::layout::outline::checks(engine.project())
            .iter()
            .filter(|f| f.code == sparksmith_core::layout::outline::SHAPE_INVALID)
            .count()
    };
    assert_eq!(invalid(&engine), 1);
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::Delete {
                ids: outside.into_iter().map(SketchRef::Curve).collect(),
            },
        }],
    )?;
    assert_eq!(invalid(&engine), 0);
    assert_eq!(engine.project().board().into_owned(), kept);

    // The laminate is the outline with its cutouts, slot and hole cut.
    let laminate = laminate(engine.project()).ok_or("laminate")?;
    assert_eq!(laminate.through.len(), 3);
    let substrate = |project: &sparksmith_core::Project| -> Fallible<f64> {
        let model = crate::project::build(project)?;
        Ok(model
            .geometry
            .iter()
            .filter(|g| g.kind == crate::material::BodyKind::Substrate)
            .map(|g| g.volume())
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
    for _ in 0..5 {
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
                value: sparksmith_core::library::footprint_source::DimValue::Fixed(10.0),
            },
        }],
        // The parameter `width` drives a dimension.
        vec![BoardEdit::RemoveParameter {
            name: ParamName("width".into()),
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
    // Opening the outline is no refusal (§5.1): the edit applies, the
    // board keeps its last valid shape and size, and `board.shape_open`
    // reports it (closed_loop_tests covers closing it again).
    let board = engine.project().board().into_owned();
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::Delete {
                ids: vec![SketchRef::Curve(bottom)],
            },
        }],
    )?;
    assert!(engine.revision() > rev);
    assert_eq!(engine.project().board().into_owned(), board);
    assert_eq!(engine.project().board_size, [60.0, 40.0]);
    assert!(
        sparksmith_core::layout::outline::checks(engine.project())
            .iter()
            .any(|f| f.code == sparksmith_core::layout::outline::SHAPE_OPEN)
    );
    Ok(())
}

/// A size the sketch cannot take is refused naming the dimension that
/// drives it, never by the solver's constraint ids (k1, k2, …).
#[test]
fn an_unsolvable_size_names_its_dimension() -> Fallible<()> {
    let mut engine = sketched()?;
    let error = edit(&mut engine, vec![set("width", "-5 mm")])
        .err()
        .ok_or("refused")?
        .to_string();
    assert!(error.contains("‘width’"), "{error}");
    assert!(error.contains("cannot be satisfied"), "{error}");
    let ids = error
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.len() > 1 && w.starts_with('k') && w[1..].chars().all(|c| c.is_ascii_digit()));
    assert!(!ids, "no constraint ids: {error}");
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
    let add = SketchEdit::AddCircle {
        center: [30.0, 20.0],
        radius: 4.0,
        construction: false,
    };
    edit(&mut engine, vec![BoardEdit::Sketch { edit: add }])?;
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
    // The mask reaches the routed edge (no pull-back), so its wall
    // triangles lie on the outline as the display flattens it: inside it
    // grown by a micrometre.
    let outer = sparksmith_core::model::board::flatten(
        &board.outer,
        crate::pad_outline::FLATTEN_TOLERANCE_MM,
    );
    let grown = sparksmith_core::library::offset_polygon(&outer, 0.001).ok_or("outline")?;
    for kind in [BodyKind::MaskTop, BodyKind::MaskBottom] {
        let mask = centroids(kind);
        assert!(!mask.is_empty(), "{kind:?}");
        for c in &mask {
            assert!(
                sparksmith_core::geometry::polygon_contains(&grown, *c),
                "{kind:?} mask at {c:?}"
            );
        }
        // Nothing over the cutout, and the rounded corner is bare.
        // The mask's wall lines the cutout (flattened: its chords lie
        // within the display tolerance inside the 4 mm circle).
        let reach = 4.0 - crate::pad_outline::FLATTEN_TOLERANCE_MM;
        assert!(
            mask.iter()
                .all(|c| (c[0] - 30.0).hypot(c[1] - 20.0) > reach)
        );
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

/// Whether fill `copper` meets `pad` through a thermal relief: copper
/// along each of `relief`'s spokes, as wide as the spoke and no wider,
/// halfway across the relief gap, and none between the spokes.
fn thermal_relief(
    copper: &sparksmith_core::model::fill::FilledCopper,
    pad: &sparksmith_core::layout::PlacedPad,
    relief: &sparksmith_core::rules::ThermalRelief,
) -> bool {
    use sparksmith_core::geometry::polygon_contains;
    let inside = |p: [f64; 2]| {
        copper.regions.iter().any(|r| {
            polygon_contains(&r.outer, p) && !r.holes.iter().any(|h| polygon_contains(h, p))
        })
    };
    let frame = pad.pad_transform();
    let centre = frame.apply([0.0, 0.0]);
    let outline = pad.outline().flatten(0.001);
    let extent = pad.pad.shape.extent();
    let far = extent[0].hypot(extent[1]);
    // The point halfway across the gap in direction `deg`, offset `side`
    // across it.
    let probe = |deg: f64, side: f64| {
        let (s, c) = deg.to_radians().sin_cos();
        let mut d = 0.0;
        while d < far && polygon_contains(&outline, [centre[0] + c * d, centre[1] + s * d]) {
            d += 0.001;
        }
        let d = d + relief.gap / 2.0;
        inside([centre[0] + c * d - s * side, centre[1] + s * d + c * side])
    };
    let n = relief.spoke_count.max(1);
    let step = 360.0 / f64::from(n);
    let sign = if frame.mirror { -1.0 } else { 1.0 };
    let angle = |k: u32| frame.rotation_deg + sign * (relief.angle_deg + step * f64::from(k));
    let half = relief.spoke_width / 2.0;
    (0..n).all(|k| {
        probe(angle(k), 0.0)
            && probe(angle(k), half - 0.02)
            && probe(angle(k), -(half - 0.02))
            && !probe(angle(k), half + 0.03)
            && !probe(angle(k), -(half + 0.03))
            && !probe(angle(k) + step / 2.0, 0.0)
    })
}

/// The shipped showcase (`Engine::showcase_hosted`): a sketched board with
/// rounded corners, a round cutout and a milled slot, a Power class over
/// GND and VCC, a top GND pour, and J1 held to the left edge. Its board
/// checks are clean; J1 follows the `height` parameter, and a corner
/// radius change leaves it in place (edges run between the virtual
/// corners).
#[test]
fn the_shipped_showcase_has_its_shape_classes_and_edge_held_header() -> Fallible<()> {
    use sparksmith_core::layout::showcase::{CORNER_MM, CUTOUT, SLOT};
    let mut engine = Engine::showcase_hosted_fixture(hosted())?;
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
        .find(|f| f.layer == sparksmith_core::CopperLayer::Top)
        .ok_or("top pour")?;
    let copper = top.computed.as_ref().ok_or("top pour filled")?;
    assert!(!copper.regions.is_empty());
    // Thermals: some ground pad joins the pour through exactly the set
    // number of spokes of the set width, with the relief gap clear of
    // copper between them; the copper graph joins it to the pour.
    let relief = top
        .settings
        .thermal
        .unwrap_or(project.rules.effective().fill.thermal);
    let gnd = project
        .nets
        .iter()
        .find(|n| n.name == "GND")
        .ok_or("GND")?
        .id;
    let relieved = sparksmith_core::layout::placed_pads(project)
        .into_iter()
        .filter(|p| p.net == Some(gnd) && p.on_copper(Layer::Top))
        .find(|p| thermal_relief(copper, p, &relief))
        .ok_or("no ground pad with a thermal relief")?;
    let graph = sparksmith_core::layout::CopperGraph::of(project);
    let pad = graph
        .pads()
        .iter()
        .position(|w| w.instance == relieved.instance && w.pad == relieved.pad.id)
        .ok_or("pad in the graph")?;
    let pour = graph
        .elements()
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            matches!(e.kind, sparksmith_core::layout::connectivity::ElementKind::Fill { fill, .. } if fill == top.id)
        })
        .filter_map(|(i, _)| graph.island_of(graph.vertices_of(i).start))
        .collect::<Vec<_>>();
    assert!(pour.contains(&graph.pad_island(pad).ok_or("pad island")?));
    assert!(
        project
            .fills
            .iter()
            .all(|f| !sparksmith_core::layout::fill::is_stale(project, f))
    );
    let findings = sparksmith_core::layout::checks(project);
    assert!(findings.is_empty(), "{findings:#?}");
    let j1 = project
        .instance_by_designator("J1")
        .map(|i| i.id)
        .ok_or("J1")?;
    assert_eq!(project.part_dimensions.len(), 2);
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

fn created(outcome: Outcome) -> Fallible<Vec<SketchRef>> {
    match outcome {
        Outcome::BoardEdited { created } => Ok(created),
        other => Err(format!("unexpected {other:?}").into()),
    }
}

fn point_of(created: &[SketchRef]) -> Fallible<SketchPointId> {
    Ok(created
        .iter()
        .find_map(|r| match r {
            SketchRef::Point(p) => Some(p.clone()),
            _ => None,
        })
        .ok_or("no created point")?)
}

/// Gap 21 of the E2E log: a client takes the mounting-hole centres from the
/// outcome instead of parsing the stored sketch.
#[test]
fn board_edit_reports_what_it_created() -> Fallible<()> {
    let mut engine = sketched()?;
    let before = engine.revision();

    let made = created(edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddPoint { at: [4.0, 4.0] },
        }],
    )?)?;
    assert_eq!(made.len(), 1, "{made:?}");
    let center = point_of(&made)?;
    let stored = check::check_sketch(&sketch(&engine)?.sketch.0).map_err(Error::from)?;
    let at = stored
        .points
        .iter()
        .find(|p| p.id == center.0)
        .ok_or("the reported point is not in the sketch")?;
    assert_eq!((at.pos.x, at.pos.y), (4.0, 4.0));

    // The reported id is what the next edit needs.
    let none = created(edit(
        &mut engine,
        vec![BoardEdit::AddMountingHole {
            center: center.clone(),
            drill: Expr("2.7 mm".into()),
            plated: false,
            pad_diameter: None,
        }],
    )?)?;
    assert!(none.is_empty(), "a role creates no sketch entity: {none:?}");
    assert_eq!(engine.project().board().holes.len(), 1);

    // Several edits report together: a line's two points and curve, and
    // one more point; a parameter adds nothing.
    let made = created(edit(
        &mut engine,
        vec![
            set("margin", "2 mm"),
            BoardEdit::Sketch {
                edit: SketchEdit::AddLine {
                    a: [10.0, 10.0],
                    b: [20.0, 10.0],
                    construction: true,
                },
            },
            BoardEdit::Sketch {
                edit: SketchEdit::AddPoint { at: [30.0, 10.0] },
            },
        ],
    )?)?;
    let count = |f: fn(&SketchRef) -> bool| made.iter().filter(|r| f(r)).count();
    assert_eq!(count(|r| matches!(r, SketchRef::Point(_))), 3, "{made:?}");
    assert_eq!(count(|r| matches!(r, SketchRef::Curve(_))), 1, "{made:?}");

    // What the same edit deleted again is not reported.
    let made = created(edit(
        &mut engine,
        vec![
            BoardEdit::Sketch {
                edit: SketchEdit::AddPoint { at: [35.0, 12.0] },
            },
            BoardEdit::Sketch {
                edit: SketchEdit::AddPoint { at: [36.0, 12.0] },
            },
        ],
    )?)?;
    assert_eq!(made.len(), 2);
    let gone = made[0].clone();
    let probe = sketch(&engine)?;
    let kept = created(edit(
        &mut engine,
        vec![
            BoardEdit::Sketch {
                edit: SketchEdit::AddPoint { at: [37.0, 12.0] },
            },
            BoardEdit::Sketch {
                edit: SketchEdit::Delete {
                    ids: vec![gone.clone()],
                },
            },
        ],
    )?)?;
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(!kept.contains(&gone));
    assert_ne!(sketch(&engine)?.sketch, probe.sketch);

    // A fillet and a slot report what they drew.
    let made = created(edit(
        &mut engine,
        vec![BoardEdit::Fillet {
            corners: Vec::new(),
            radius: DimValue::Fixed(1.0),
        }],
    )?)?;
    let arcs: Vec<_> = made
        .iter()
        .filter(|r| matches!(r, SketchRef::Curve(_)))
        .collect();
    assert_eq!(arcs.len(), 4, "one arc per corner: {made:?}");
    let made = created(edit(
        &mut engine,
        vec![BoardEdit::AddSlot {
            a: [20.0, 20.0],
            b: [30.0, 20.0],
            width: DimValue::Fixed(1.0),
            plated: false,
        }],
    )?)?;
    assert_eq!(
        made.iter()
            .filter(|r| matches!(r, SketchRef::Curve(_)))
            .count(),
        4,
        "two lines and two caps: {made:?}"
    );

    // The JSON a client reads.
    let json = serde_json::to_value(Outcome::BoardEdited {
        created: vec![SketchRef::Point(center.clone())],
    })?;
    assert_eq!(json["outcome"], "board_edited");
    assert_eq!(json["created"][0]["point"], center.0);

    // A refused edit reports nothing and changes nothing.
    let revision = engine.revision();
    let file = engine.to_file();
    assert!(
        edit(
            &mut engine,
            vec![
                BoardEdit::Sketch {
                    edit: SketchEdit::AddPoint { at: [5.0, 5.0] },
                },
                set("width", "-5 mm"),
            ],
        )
        .is_err()
    );
    assert_eq!(engine.revision(), revision);
    assert_eq!(engine.to_file(), file);

    // Undo takes the edit back as one step.
    engine.undo()?;
    assert_eq!(engine.project().board().cutouts.len(), 0);
    assert!(engine.revision() > before);
    Ok(())
}

/// The largest x of a fill's computed copper, if it has any.
fn copper_right(engine: &Engine, fill: sparksmith_core::Id) -> Option<f64> {
    let copper = engine.project().fill(fill)?.computed.as_ref()?;
    copper
        .regions
        .iter()
        .flat_map(|r| &r.outer)
        .map(|p| p[0])
        .reduce(f64::max)
}

/// A fill that follows the board (`region: board`, no outline of its
/// own) takes the solved outline after a board parameter edit; a refill
/// grows its copper to the new edge less the edge clearance, a drawn fill
/// stays where it was drawn, and undo restores both.
#[test]
fn a_board_following_fill_grows_with_the_board_width() -> Fallible<()> {
    use sparksmith_core::layout::fill::{FillCommand, RefillTarget};
    use sparksmith_core::model::fill::{FillRegion, FillSpec};
    let mut engine = sketched()?;
    let add = |engine: &mut Engine, spec: serde_json::Value| -> Fallible<sparksmith_core::Id> {
        let spec: FillSpec = serde_json::from_value(spec)?;
        match apply(
            engine,
            Operation::Fill(FillCommand::Add {
                fill: Box::new(spec),
            }),
        )? {
            Outcome::FillAdded { fill } => Ok(fill),
            other => Err(format!("{other:?}").into()),
        }
    };
    let islands = serde_json::json!({"islands": {"remove": "never"}});
    let follows = add(
        &mut engine,
        serde_json::json!({"name": "plane", "net": null, "layer": "Bottom",
                           "region": "board", "settings": islands}),
    )?;
    let drawn_outline = serde_json::json!({"start": [0.0, 0.0], "segments": [
        {"line": {"to": [60.0, 0.0]}}, {"line": {"to": [60.0, 40.0]}},
        {"line": {"to": [0.0, 40.0]}}, {"line": {"to": [0.0, 0.0]}}]});
    let drawn = add(
        &mut engine,
        serde_json::json!({"name": "drawn", "net": null, "layer": "Top",
                           "outline": drawn_outline, "settings": islands}),
    )?;
    let outer = |engine: &Engine| engine.project().board().outer.clone();
    let fill = |engine: &Engine, id| engine.project().fill(id).cloned().ok_or("fill");
    assert_eq!(fill(&engine, follows)?.region, FillRegion::Board);
    assert_eq!(fill(&engine, follows)?.outline, outer(&engine));
    let refill = |engine: &mut Engine| {
        apply(
            engine,
            Operation::Fill(FillCommand::Refill {
                target: RefillTarget::All,
            }),
        )
    };
    refill(&mut engine)?;
    let edge = engine.project().rules.effective().copper.clearance.get(
        sparksmith_core::rules::CopperObject::Fill,
        sparksmith_core::rules::CopperObject::BoardEdge,
    );
    assert!(edge > 0.0);
    let near = |x: Option<f64>, want: f64| x.is_some_and(|x| x <= want + 1e-6 && x >= want - 0.02);
    assert!(near(copper_right(&engine, follows), 60.0 - edge));
    let before = engine.project().clone();

    edit(&mut engine, vec![set("width", "80 mm")])?;
    assert_eq!(engine.project().board_size, [80.0, 40.0]);
    // The region follows at once; the stored copper is stale until a refill.
    assert_eq!(fill(&engine, follows)?.outline, outer(&engine));
    assert!(sparksmith_core::layout::fill::is_stale(
        engine.project(),
        &fill(&engine, follows)?
    ));
    refill(&mut engine)?;
    assert!(near(copper_right(&engine, follows), 80.0 - edge));
    // The drawn fill keeps its own outline: its copper stops where it
    // was drawn, now short of the moved edge.
    assert_eq!(
        fill(&engine, drawn)?.outline,
        before.fill(drawn).ok_or("drawn")?.outline
    );
    assert!(copper_right(&engine, drawn).is_some_and(|x| x <= 60.0 + 1e-6));

    engine.undo()?;
    engine.undo()?;
    assert_eq!(*engine.project(), before);
    assert!(near(copper_right(&engine, follows), 60.0 - edge));

    // Its outline is derived: editing it is refused, in the engine's
    // words, until the fill is made a drawn one.
    let mut moved = fill(&engine, follows)?;
    moved.outline = serde_json::from_value(serde_json::json!({"start": [5.0, 5.0], "segments": [
        {"line": {"to": [30.0, 5.0]}}, {"line": {"to": [30.0, 30.0]}},
        {"line": {"to": [5.0, 30.0]}}, {"line": {"to": [5.0, 5.0]}}]}))?;
    let rev = engine.revision();
    let refused = apply(
        &mut engine,
        Operation::Fill(FillCommand::Edit {
            fill: Box::new(moved.clone()),
        }),
    );
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| format!("{e:?}").contains("fill.follows_board")),
        "{refused:?}"
    );
    assert_eq!(engine.revision(), rev);
    moved.region = FillRegion::Drawn;
    apply(
        &mut engine,
        Operation::Fill(FillCommand::Edit {
            fill: Box::new(moved.clone()),
        }),
    )?;
    edit(&mut engine, vec![set("width", "70 mm")])?;
    assert_eq!(fill(&engine, follows)?.outline, moved.outline);
    Ok(())
}

/// Slice P1: every cutout has a stable id, so its interface datum (token
/// and slot) keeps its owner whatever its curves are named. Replacing the
/// loop's oldest line, which used to own the datum, keeps the id; deleting
/// another cutout before it renumbers nothing; a cutout drawn anew gets a
/// new id, never a used one.
#[test]
fn a_cutout_keeps_its_id_as_its_curves_change() -> Fallible<()> {
    use sparksmith_core::model::interface::{DatumOwner, DatumRole};
    let mut engine = sketched()?;
    // The board uses its interface: the table is stored from here on.
    apply(
        &mut engine,
        Operation::Interface(sparksmith_core::interface::InterfaceCommand::Auto {
            rule: sparksmith_core::model::interface::AutoRule::Holes,
            on: false,
        }),
    )?;
    let first = created_curves_of(edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddRect {
                a: [10.0, 10.0],
                b: [16.0, 16.0],
                construction: false,
            },
        }],
    )?);
    let second = created_curves_of(edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddRect {
                a: [30.0, 10.0],
                b: [36.0, 16.0],
                construction: false,
            },
        }],
    )?);
    assert_eq!(first.len(), 4);
    assert_eq!(second.len(), 4);
    // The id of the cutout around `at`, and its datum's token and slot.
    let cutout = |engine: &Engine, at: Point| -> Fallible<(u32, String, u32)> {
        let board = engine.project().board().into_owned();
        let id = board
            .cutouts
            .iter()
            .find(|c| {
                let b = sparksmith_core::model::board::outline_bounds(&c.outline);
                b.min[0] <= at[0] && at[0] <= b.max[0] && b.min[1] <= at[1] && at[1] <= b.max[1]
            })
            .and_then(|c| c.id)
            .ok_or("the cutout has its id")?;
        let table = sparksmith_core::interface::list::table(engine.project())?;
        let datum = table
            .datums
            .iter()
            .find(|d| {
                d.key.role == DatumRole::BoardCutout && d.key.owner == DatumOwner::Cutout { id }
            })
            .ok_or("the cutout's datum")?;
        Ok((
            id.0,
            sparksmith_core::interface::token(datum)?,
            datum.slot.0,
        ))
    };
    let (a, a_token, a_slot) = cutout(&engine, [13.0, 13.0])?;
    let (b, b_token, b_slot) = cutout(&engine, [33.0, 13.0])?;
    assert_ne!(a, b);
    assert_eq!(a_token, format!("cut{a}"));
    assert_eq!(b_token, format!("cut{b}"));

    // Replace the second cutout's oldest line by a new one along it: the
    // loop's curves change, its id, token and slot stay.
    let oldest = second
        .iter()
        .min_by_key(|c| (c.0.len(), c.0.clone()))
        .cloned()
        .ok_or("a line")?;
    let s = sketch(&engine)?;
    let json: serde_json::Value = serde_json::from_str(&s.sketch.0)?;
    let line = json["curves"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["id"] == oldest.0.as_str())
        .ok_or("the line")?;
    let points = json["points"].as_array().cloned().unwrap_or_default();
    let ends: Vec<Point> = ["a", "b"]
        .iter()
        .filter_map(|end| {
            let p = points.get(usize::try_from(line[end].as_u64()?).ok()?)?;
            Some([p["pos"][0].as_f64()?, p["pos"][1].as_f64()?])
        })
        .collect();
    let [from, to] = ends.as_slice() else {
        return Err(format!("the line's ends: {ends:?}").into());
    };
    edit(
        &mut engine,
        vec![
            BoardEdit::Sketch {
                edit: SketchEdit::Delete {
                    ids: vec![SketchRef::Curve(oldest.clone())],
                },
            },
            BoardEdit::Sketch {
                edit: SketchEdit::AddLine {
                    a: *from,
                    b: *to,
                    construction: false,
                },
            },
        ],
    )?;
    let curves = engine
        .project()
        .board()
        .cutouts
        .iter()
        .find(|c| c.id.map(|i| i.0) == Some(b))
        .map(|c| c.curves.clone())
        .ok_or("the cutout is still there")?;
    assert!(!curves.contains(&oldest), "the loop's curves changed");
    assert_eq!(cutout(&engine, [33.0, 13.0])?, (b, b_token.clone(), b_slot));

    // Deleting the first cutout renumbers nothing.
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::Delete {
                ids: first.iter().cloned().map(SketchRef::Curve).collect(),
            },
        }],
    )?;
    assert_eq!(engine.project().board().cutouts.len(), 1);
    assert_eq!(cutout(&engine, [33.0, 13.0])?, (b, b_token, b_slot));

    // Drawn again where the first was: a new id, never a used one.
    edit(
        &mut engine,
        vec![BoardEdit::Sketch {
            edit: SketchEdit::AddRect {
                a: [10.0, 10.0],
                b: [16.0, 16.0],
                construction: false,
            },
        }],
    )?;
    let (c, _, c_slot) = cutout(&engine, [13.0, 13.0])?;
    assert!(c != a && c != b, "a new id: {c}");
    assert_ne!(c_slot, a_slot, "slots are never reused");

    // Save and reopen: ids hold.
    let reopened = Engine::from_file(format::decode(&format::encode(&engine.to_file())?)?)?;
    assert_eq!(cutout(&reopened, [33.0, 13.0])?.0, b);
    assert_eq!(cutout(&reopened, [13.0, 13.0])?.0, c);
    Ok(())
}
