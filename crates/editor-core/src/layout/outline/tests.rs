//! Board shape and edge-constraint tests that need no sketch evaluator:
//! edge poses on lines and arcs, plain-length constraints on a rectangular
//! board following its size, refusal of moves away from a constraint,
//! alignment to a part's extent, and the unresolved-constraint finding.
//! The sketch evaluator's tests live in `pcbcraft_solidcraft::board`.
use super::*;
use crate::layout::LayoutCommand;
use crate::layout::fill::FillCommand;
use crate::library::{Outline, Segment};
use crate::model::Layer;
use crate::model::board::{ConstraintAlign, ConstraintTarget, EdgeAnchor, SolvedEdge};
use crate::model::fill::FillSpec;
use crate::{CopperLayer, Engine, Operation, Outcome};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn close(a: Point, b: Point) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

fn edge(a: Point, b: Point, bulge: f64) -> SolvedEdge {
    SolvedEdge {
        name: EdgeName("e".into()),
        a,
        b,
        bulge,
    }
}

fn spec(target: ConstraintTarget, edge: &str, along: &str, offset: &str) -> EdgeConstraintSpec {
    EdgeConstraintSpec {
        target,
        edge: EdgeName(edge.into()),
        anchor: EdgeAnchor::Start,
        along: Expr(along.into()),
        offset: Expr(offset.into()),
        align: ConstraintAlign::Origin,
        follow_edge: false,
        rotation_deg: 0.0,
    }
}

fn first_part(engine: &Engine) -> Fallible<Id> {
    Ok(engine
        .project()
        .placements
        .first()
        .ok_or("no placement")?
        .instance)
}

fn pose(engine: &Engine, id: Id) -> Fallible<(Point, f64)> {
    let p = engine
        .project()
        .placements
        .iter()
        .find(|p| p.instance == id)
        .ok_or("no placement")?;
    Ok((p.position, p.rotation_deg))
}

#[test]
fn edge_pose_runs_along_lines_and_offsets_inward() -> Fallible<()> {
    // The bottom edge of a board runs +x; inside is +y.
    let bottom = edge([0.0, 0.0], [60.0, 0.0], 0.0);
    let p = edge_pose(&bottom, EdgeAnchor::Start, 10.0, 2.0)?;
    assert!(close(p.position, [10.0, 2.0]));
    assert!(p.edge_deg.abs() < 1e-12);
    let p = edge_pose(&bottom, EdgeAnchor::Middle, -5.0, 0.0)?;
    assert!(close(p.position, [25.0, 0.0]));
    let p = edge_pose(&bottom, EdgeAnchor::Fraction(0.25), 0.0, 1.0)?;
    assert!(close(p.position, [15.0, 1.0]));
    // The right edge runs +y; inside is -x.
    let right = edge([60.0, 0.0], [60.0, 40.0], 0.0);
    let p = edge_pose(&right, EdgeAnchor::End, 0.0, 3.0)?;
    assert!(close(p.position, [57.0, 40.0]));
    assert!((p.edge_deg - 90.0).abs() < 1e-12);
    assert!(edge_pose(&bottom, EdgeAnchor::Fraction(1.5), 0.0, 0.0).is_err());
    assert!(
        edge_pose(
            &edge([1.0, 1.0], [1.0, 1.0], 0.0),
            EdgeAnchor::Start,
            0.0,
            0.0
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn edge_pose_follows_arc_length_and_the_tangent() -> Fallible<()> {
    // A counter-clockwise quarter circle of radius 10 about the origin,
    // from (10, 0) to (0, 10): bulge tan(90° / 4).
    let bulge = (std::f64::consts::FRAC_PI_2 / 4.0).tan();
    let arc = edge([10.0, 0.0], [0.0, 10.0], bulge);
    let p = edge_pose(&arc, EdgeAnchor::Middle, 0.0, 0.0)?;
    let h = 10.0 / 2f64.sqrt();
    assert!((p.position[0] - h).abs() < 1e-9 && (p.position[1] - h).abs() < 1e-9);
    assert!((p.edge_deg - 135.0).abs() < 1e-9);
    // Inward (left of a counter-clockwise arc) is toward the centre.
    let p = edge_pose(&arc, EdgeAnchor::Start, 0.0, 2.0)?;
    assert!(close(p.position, [8.0, 0.0]));
    // Arc length: a quarter of the circumference reaches the end.
    let quarter = std::f64::consts::FRAC_PI_2 * 10.0;
    let p = edge_pose(&arc, EdgeAnchor::Start, quarter, 0.0)?;
    assert!((p.position[0]).abs() < 1e-9 && (p.position[1] - 10.0).abs() < 1e-9);
    // Clockwise: inward is away from the centre.
    let cw = edge([0.0, 10.0], [10.0, 0.0], -bulge);
    let p = edge_pose(&cw, EdgeAnchor::Start, 0.0, 2.0)?;
    assert!(close(p.position, [0.0, 12.0]));
    Ok(())
}

#[test]
fn plain_lengths_read_without_an_evaluator() {
    for (text, want) in [
        ("12.5", Some(12.5)),
        ("-3 mm", Some(-3.0)),
        ("4mm", Some(4.0)),
    ] {
        assert_eq!(plain_mm(&Expr(text.into())), want, "{text}");
    }
    for text in ["width / 2", "3 in", "", "mm", "1e999"] {
        assert_eq!(plain_mm(&Expr(text.into())), None, "{text}");
    }
}

#[test]
fn a_constrained_part_follows_the_board_edge() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let part = first_part(&engine)?;
    let [w, _] = engine.project().board_size;
    let mut c = spec(ConstraintTarget::Placement(part), "right", "10", "5 mm");
    c.follow_edge = true;
    let Outcome::EdgeConstraintAdded { constraint } = engine.apply(
        engine.revision(),
        Operation::Board(BoardCommand::Constrain { constraint: c }),
    )?
    else {
        return Err("no constraint".into());
    };
    let (at, rotation) = pose(&engine, part)?;
    assert!(close(at, [w - 5.0, 10.0]), "{at:?}");
    assert!((rotation - 90.0).abs() < 1e-9);

    // The board grows: the part keeps 5 mm from the right edge.
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetBoardSize { size: [80.0, 50.0] }),
    )?;
    let (at, _) = pose(&engine, part)?;
    assert!(close(at, [75.0, 10.0]), "{at:?}");

    // Moving or turning it away from its constraint is refused unchanged.
    // The refusal says why and what to do, in the one message style, and
    // the canvas's early refusal of a drag uses the same words.
    let rev = engine.revision();
    let before = engine.to_file();
    let name = engine
        .project()
        .instance(part)
        .map(|i| i.designator.clone())
        .ok_or("no designator")?;
    let held = format!("{name} is held 5 mm from the right edge; edit or remove its constraint");
    assert_eq!(
        crate::layout::move_refusal(engine.project(), part, None).map(|e| e.to_string()),
        Some(Error::Conflict(format!("{held} to move it")).to_string())
    );
    for (position, rotation_deg, verb) in
        [([20.0, 20.0], 90.0, "move"), ([75.0, 10.0], 0.0, "turn")]
    {
        let want = format!("{held} to {verb} it");
        assert!(crate::issues::style::check(&want).is_empty(), "{want}");
        let moved = engine.apply(
            rev,
            Operation::Layout(LayoutCommand::Move {
                instance: part,
                position,
                rotation_deg,
                layer: Layer::Top,
            }),
        );
        let err = moved.err().ok_or("the move was accepted")?;
        assert!(matches!(&err, Error::Conflict(t) if *t == want), "{err:?}");
        assert_eq!(engine.revision(), rev);
        assert_eq!(engine.to_file(), before);
    }
    // Flipping it to the bottom keeps its constrained pose and is allowed.
    engine.apply(
        rev,
        Operation::Layout(LayoutCommand::Move {
            instance: part,
            position: [75.0, 10.0],
            rotation_deg: 90.0,
            layer: Layer::Bottom,
        }),
    )?;

    // Deleting the constraint frees it; undo brings the constraint back.
    engine.apply(
        engine.revision(),
        Operation::Board(BoardCommand::DeleteConstraint { id: constraint }),
    )?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Move {
            instance: part,
            position: [20.0, 20.0],
            rotation_deg: 0.0,
            layer: Layer::Top,
        }),
    )?;
    engine.undo()?;
    engine.undo()?;
    assert_eq!(engine.project().edge_constraints.len(), 1);
    Ok(())
}

#[test]
fn expressions_need_the_evaluator_and_targets_must_exist() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let part = first_part(&engine)?;
    let rev = engine.revision();
    let before = engine.to_file();
    let refused = [
        spec(
            ConstraintTarget::Placement(part),
            "right",
            "height / 2",
            "5",
        ),
        spec(ConstraintTarget::Placement(part), "no_such_edge", "1", "1"),
        spec(ConstraintTarget::Placement(Id::MAX), "right", "1", "1"),
        spec(ConstraintTarget::Fill(part), "right", "1", "1"),
    ];
    for c in refused {
        assert!(
            engine
                .apply(
                    rev,
                    Operation::Board(BoardCommand::Constrain { constraint: c })
                )
                .is_err()
        );
        assert_eq!(engine.to_file(), before);
    }
    // One constraint per target.
    let c = spec(ConstraintTarget::Placement(part), "left", "1", "1");
    engine.apply(
        rev,
        Operation::Board(BoardCommand::Constrain {
            constraint: c.clone(),
        }),
    )?;
    assert!(
        engine
            .apply(
                engine.revision(),
                Operation::Board(BoardCommand::Constrain { constraint: c })
            )
            .is_err()
    );
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

#[test]
fn fills_translate_to_their_edge_and_report_when_gone() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let Outcome::FillAdded { fill } = engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Add {
            fill: Box::new(FillSpec {
                region: crate::model::fill::FillRegion::Drawn,
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
    let mut c = spec(ConstraintTarget::Fill(fill), "top", "0", "1");
    c.anchor = EdgeAnchor::Middle;
    c.align = ConstraintAlign::NearSide;
    // Fills keep their orientation.
    let mut turning = c.clone();
    turning.follow_edge = true;
    assert!(
        engine
            .apply(
                engine.revision(),
                Operation::Board(BoardCommand::Constrain {
                    constraint: turning
                })
            )
            .is_err()
    );
    engine.apply(
        engine.revision(),
        Operation::Board(BoardCommand::Constrain { constraint: c }),
    )?;
    // The top edge runs right to left; its middle is (30, 40). The fill's
    // side nearest the edge sits 1 mm inside it, centred on the middle.
    let [w, h] = engine.project().board_size;
    let f = engine.project().fills.first().ok_or("no fill")?;
    let bounds = crate::model::board::outline_bounds(&f.outline);
    assert!(close(bounds.max, [w / 2.0 + 5.0, h - 1.0]), "{bounds:?}");
    assert!(close(bounds.min, [w / 2.0 - 5.0, h - 11.0]), "{bounds:?}");
    assert!(checks(engine.project()).is_empty());

    engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Delete { id: fill }),
    )?;
    let findings = checks(engine.project());
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings.first().map(|f| f.code.as_str()),
        Some(CONSTRAINT_UNRESOLVED)
    );
    // Reported through project issues too.
    assert!(
        crate::layout::checks(engine.project())
            .iter()
            .any(|f| f.code == CONSTRAINT_UNRESOLVED)
    );
    Ok(())
}

#[test]
fn near_side_alignment_puts_the_part_extent_on_the_offset() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let part = first_part(&engine)?;
    let mut c = spec(ConstraintTarget::Placement(part), "bottom", "20", "0");
    c.align = ConstraintAlign::NearSide;
    engine.apply(
        engine.revision(),
        Operation::Board(BoardCommand::Constrain { constraint: c }),
    )?;
    let extents = placement_extents(engine.project());
    let pts = extents.get(&part).ok_or("no extent")?;
    let min_y = pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
    let (min_x, max_x) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    assert!(min_y.abs() < 1e-9, "the extent touches the edge: {min_y}");
    assert!(((min_x + max_x) / 2.0 - 20.0).abs() < 1e-9);
    Ok(())
}

/// The held refusal reads as a sentence about the part and its edge, in
/// one message line however long the names: the distance goes first, then
/// the edge, never the way out.
#[test]
fn the_held_refusal_fits_one_line_and_keeps_the_way_out() {
    let right = EdgeName("right".into());
    let way_out = |verb: &str| format!("; edit or remove its constraint to {verb} it");
    assert_eq!(
        held_message("J1", &right, Some(2.0), "move"),
        format!("J1 is held 2 mm from the right edge{}", way_out("move"))
    );
    assert_eq!(
        held_message("J1", &right, Some(0.0), "move"),
        format!("J1 is held on the right edge{}", way_out("move"))
    );
    assert_eq!(
        held_message("J10", &right, Some(-1.5), "move"),
        format!("J10 is held 1.5 mm past the right edge{}", way_out("move"))
    );
    assert_eq!(
        held_message("J1", &EdgeName("USB edge".into()), None, "move"),
        format!("J1 is held to the USB edge{}", way_out("move"))
    );
    // "CONN12 is held 12.25 mm from the bottom edge; …" is 86 characters:
    // the distance goes, the edge and the way out stay.
    assert_eq!(
        held_message("CONN12", &EdgeName("bottom".into()), Some(12.25), "turn"),
        format!("CONN12 is held to the bottom edge{}", way_out("turn"))
    );
    let long = held_message(
        "CONNECTOR_LONG_NAME_7",
        &EdgeName("a very long edge name indeed".into()),
        Some(12.25),
        "move",
    );
    assert!(long.ends_with(&way_out("move")), "{long}");
    for text in [long, held_message("J1", &right, Some(2.0), "turn")] {
        assert!(text.chars().count() <= crate::issues::MAX_TITLE, "{text}");
        assert!(crate::issues::style::check(&text).is_empty(), "{text}");
    }
}
