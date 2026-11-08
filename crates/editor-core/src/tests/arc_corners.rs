//! Arc corners (docs/schematic-wires.md, slice 4): `wire.set_radius` rounds a
//! corner by a radius that fits its segments, as one undo step, refused as a
//! whole otherwise; radii survive the path edits, rubber-banding, save and
//! reopen; the scene draws each rounded corner as its tangent arc while the
//! vertices, and so the nets, stay where they are.
use super::support::*;
use crate::geometry::{Shape, distance};
use crate::model::{Corner, FreeEnd};
use crate::schematic::{self, SchematicCommand as C, WireTarget, scene};
use crate::{Engine, Error, Id, Operation, Outcome, Point, Result, format};

fn two_pin(engine: &mut Engine) -> Result<Fixture> {
    component(
        engine,
        &Spec {
            name: "Two terminal",
            terminals: vec![(1, "A", passive()), (2, "B", passive())],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: false,
        },
    )
}

fn run(engine: &mut Engine, command: C) -> Result<Outcome> {
    apply(engine, Operation::Schematic(command))
}

/// The dogleg U1.2 (5, 0) → (20, 0) → (20, 20) → U2.1 (35, 20): segments of
/// 15, 20 and 15 mm. U2's occurrence and the wire.
fn dogleg(engine: &mut Engine) -> Result<(Id, Id)> {
    let f = two_pin(engine)?;
    let (u1, _) = place(engine, &f, "U1", 1, [0.0, 0.0])?;
    let (u2, occurrence) = place(engine, &f, "U2", 1, [40.0, 20.0])?;
    run(
        engine,
        C::DrawWire {
            sheet: None,
            from: ep(u1, 2).into(),
            corners: vec![Corner::sharp([20.0, 0.0]), Corner::sharp([20.0, 20.0])],
            to: ep(u2, 1).into(),
        },
    )?;
    Ok((occurrence, engine.project().wires[0].id))
}

fn radii(engine: &Engine, wire: Id) -> Result<Vec<f64>> {
    let wire = engine
        .project()
        .wire(wire)
        .ok_or(Error::Invalid("wire".into()))?;
    Ok(wire.corners.iter().map(|c| c.radius).collect())
}

fn set(engine: &mut Engine, wire: Id, index: usize, radius: f64) -> Result<Outcome> {
    run(
        engine,
        C::SetWireRadius {
            wire,
            index,
            radius,
        },
    )
}

/// A radius rounds the corner in one undo step, keeps the vertices and the
/// nets, and saves and reopens unchanged; 0 makes it sharp again.
#[test]
fn a_radius_rounds_a_corner_in_one_undo_step_and_survives_save() -> Result<()> {
    let mut engine = Engine::default();
    let (_, wire) = dogleg(&mut engine)?;
    let before = engine.document().clone();
    let path = engine
        .project()
        .wire(wire)
        .and_then(|w| schematic::wire_path(engine.project(), w));
    let revision = engine.revision();
    set(&mut engine, wire, 0, 5.0)?;
    assert_eq!(engine.revision(), revision + 1);
    assert_eq!(radii(&engine, wire)?, vec![5.0, 0.0]);
    let after = engine.project().wire(wire);
    assert_eq!(
        after.and_then(|w| schematic::wire_path(engine.project(), w)),
        path,
        "the vertices stay"
    );
    assert_eq!(engine.project().nets, before.project.nets);
    // Saved as `"radius"` beside the corner's point, and read back.
    let saved = serde_json::to_value(engine.project())?;
    assert_eq!(
        saved["wires"][0]["corners"],
        serde_json::json!([{ "at": [20.0, 0.0], "radius": 5.0 }, { "at": [20.0, 20.0] }])
    );
    let bytes = format::encode(&engine.to_file())?;
    let opened = Engine::from_file(format::decode(&bytes)?)?;
    assert_eq!(opened.project(), engine.project());
    set(&mut engine, wire, 0, 0.0)?;
    assert_eq!(radii(&engine, wire)?, vec![0.0, 0.0]);
    engine.undo()?;
    engine.undo()?;
    assert_eq!(engine.document(), &before);
    Ok(())
}

/// The limit is what fits both segments beside the neighbours' arcs: the
/// 15 mm segment takes 15, and once corner 1 has 12 mm of the 20 mm segment
/// between them, corner 0 takes at most 8. Past it, and for any malformed
/// radius, the command is refused and changes nothing.
#[test]
fn a_radius_that_does_not_fit_is_refused_not_reduced() -> Result<()> {
    let mut engine = Engine::default();
    let (_, wire) = dogleg(&mut engine)?;
    set(&mut engine, wire, 0, 15.0)?;
    set(&mut engine, wire, 0, 0.0)?;
    set(&mut engine, wire, 1, 12.0)?;
    let document = engine.document().clone();
    let revision = engine.revision();
    let cases: Vec<(&str, Id, usize, f64)> = vec![
        ("past what fits beside the neighbour", wire, 0, 8.5),
        ("past the segment", wire, 1, 15.5),
        ("negative", wire, 0, -1.0),
        ("not finite", wire, 0, f64::NAN),
        ("past the largest radius", wire, 0, 2000.0),
        ("no such corner", wire, 2, 1.0),
        ("no such wire", 9999, 0, 1.0),
    ];
    for (case, wire, index, radius) in cases {
        assert!(set(&mut engine, wire, index, radius).is_err(), "{case}");
        assert_eq!(engine.document(), &document, "{case}");
        assert_eq!(engine.revision(), revision, "{case}");
    }
    let refused = set(&mut engine, wire, 0, 8.5)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(refused.contains("at most 8 mm fits"), "{refused}");
    set(&mut engine, wire, 0, 8.0)?;
    assert_eq!(radii(&engine, wire)?, vec![8.0, 12.0]);
    // A corner in line between its neighbours has no turn to round.
    run(
        &mut engine,
        C::InsertWireCorner {
            wire,
            at: [10.0, 0.0],
        },
    )?;
    let document = engine.document().clone();
    assert!(set(&mut engine, wire, 0, 1.0).is_err());
    assert_eq!(engine.document(), &document);
    Ok(())
}

/// Moving a corner, a segment or the part at a wire's end keeps every
/// corner's radius with its vertex; a new corner is sharp.
#[test]
fn edits_and_rubber_banding_keep_radii() -> Result<()> {
    let mut engine = Engine::default();
    let (u2, wire) = dogleg(&mut engine)?;
    set(&mut engine, wire, 0, 2.5)?;
    set(&mut engine, wire, 1, 5.0)?;
    let cases: Vec<(&str, C, Vec<f64>)> = vec![
        (
            "a corner drag",
            C::MoveWireCorner {
                wire,
                index: 0,
                to: [25.0, 5.0],
                free: false,
            },
            // The segment on U1's pin steps through a new, sharp corner.
            vec![0.0, 2.5, 5.0],
        ),
        (
            "a segment drag",
            C::MoveWireSegment {
                wire,
                index: 1,
                offset: -5.0,
            },
            vec![2.5, 5.0],
        ),
        (
            "a new corner",
            C::InsertWireCorner {
                wire,
                at: [20.0, 10.0],
            },
            vec![2.5, 0.0, 5.0],
        ),
        (
            "rubber-banding",
            C::MoveUnit {
                occurrence: u2,
                position: [40.0, 30.0],
            },
            vec![2.5, 5.0],
        ),
    ];
    for (case, command, expected) in cases {
        run(&mut engine, command)?;
        assert_eq!(radii(&engine, wire)?, expected, "{case}");
        engine.undo()?;
    }
    // Without its first segment the wire starts free at (20, 0), and its
    // remaining corner keeps its radius.
    run(&mut engine, C::RemoveWireSegment { wire, index: 0 })?;
    assert_eq!(radii(&engine, wire)?, vec![5.0]);
    Ok(())
}

/// The scene draws a rounded corner as the arc tangent to both segments:
/// from (15, 0) to (20, 5) about (15, 5) for 5 mm at (20, 0). The printed
/// polyline stays within the PDF's curve tolerance of the arc, and the sharp
/// vertex is no longer on the drawn wire.
#[test]
fn the_scene_draws_the_tangent_arc() -> Result<()> {
    let mut engine = Engine::default();
    let (_, wire) = dogleg(&mut engine)?;
    set(&mut engine, wire, 0, 5.0)?;
    let project = engine.project();
    let scene = scene::sheet_scene(project, project.sheets.first());
    let [shape] = scene.wires.as_slice() else {
        return Err(Error::Invalid("one wire".into()));
    };
    let Shape::Polyline { points, radii } = shape else {
        return Err(Error::Invalid("a polyline".into()));
    };
    assert_eq!(
        points,
        &vec![[5.0, 0.0], [20.0, 0.0], [20.0, 20.0], [35.0, 20.0]]
    );
    assert_eq!(radii, &vec![0.0, 5.0, 0.0, 0.0]);
    let tolerance = 0.02;
    let drawn = shape.flatten(tolerance);
    let near = |p: Point| drawn.iter().any(|q| distance(*q, p) < 1e-9);
    assert!(near([15.0, 0.0]) && near([20.0, 5.0]), "the tangent points");
    assert!(!near([20.0, 0.0]), "the vertex is cut off");
    let centre = [15.0, 5.0];
    let arc: Vec<Point> = drawn
        .iter()
        .copied()
        .filter(|p| p[0] > 15.0 && p[1] < 5.0)
        .collect();
    assert!(arc.len() >= 4, "{arc:?}");
    for w in arc.windows(2) {
        let middle = [(w[0][0] + w[1][0]) / 2.0, (w[0][1] + w[1][1]) / 2.0];
        assert!((distance(w[0], centre) - 5.0).abs() < 1e-9);
        assert!(5.0 - distance(middle, centre) <= tolerance + 1e-9);
    }
    // What a pick measures: on the arc, not at the vertex.
    let middle = [15.0 + 5.0 / 2f64.sqrt(), 5.0 - 5.0 / 2f64.sqrt()];
    assert!(shape.distance(middle, false) < 1e-9);
    assert!(shape.distance([20.0, 0.0], false) > 2.0);
    Ok(())
}

/// `wire.draw` stores the radii it is given, and refuses a malformed one.
#[test]
fn wire_draw_takes_radii_and_refuses_a_malformed_one() -> Result<()> {
    let mut engine = Engine::default();
    two_pin(&mut engine)?;
    let free = |p: Point| {
        FreeEnd::new(p)
            .map(WireTarget::Free)
            .ok_or(Error::Invalid("free".into()))
    };
    let draw = |radius: f64| -> Result<C> {
        Ok(C::DrawWire {
            sheet: None,
            from: free([0.0, 0.0])?,
            corners: vec![Corner {
                at: [10.0, 0.0],
                radius,
            }],
            to: free([10.0, 10.0])?,
        })
    };
    let document = engine.document().clone();
    for radius in [-1.0, f64::INFINITY, 1001.0] {
        assert!(run(&mut engine, draw(radius)?).is_err(), "{radius}");
        assert_eq!(engine.document(), &document);
    }
    run(&mut engine, draw(2.5)?)?;
    let corners = &engine.project().wires[0].corners;
    assert_eq!(
        corners,
        &vec![Corner {
            at: [10.0, 0.0],
            radius: 2.5
        }]
    );
    Ok(())
}

#[test]
fn set_radius_reads_its_documented_json() -> Result<()> {
    let json = r#"{"command":"wire.set_radius","params":{"wire":1,"index":0,"radius":2.5}}"#;
    let command: C = serde_json::from_str(json).map_err(|e| Error::Invalid(e.to_string()))?;
    let back = serde_json::to_string(&command).map_err(|e| Error::Invalid(e.to_string()))?;
    assert_eq!(back, json);
    Ok(())
}
