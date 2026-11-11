//! Rotating and mirroring symbol occurrences: the occurrence frame, pins and
//! wires following through `symbol.orient_unit`, one undo step, the upright
//! text rule under every orientation, and the JSON contract.
use super::support::*;
use crate::geometry::{Bounds, Point, Transform2};
use crate::instances;
use crate::library::graphics::{TextSize, upright};
use crate::library::{Item, Justify, Rotation, TextMetric};
use crate::schematic::{self, PlacedText, SchematicCommand};
use crate::{Engine, Error, Id, Operation, Result, SymbolOccurrence};
use serde_json::json;

const ROTATIONS: [Rotation; 4] = [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270];

fn close(a: Point, b: Point) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

fn close_bounds(a: Bounds, b: Bounds) -> bool {
    close(a.min, b.min) && close(a.max, b.max)
}

fn occurrence(position: Point, rotation: Rotation, mirror: bool) -> SymbolOccurrence {
    SymbolOccurrence {
        sheet: Default::default(),
        id: 1,
        instance: 1,
        unit: crate::UnitId(1),
        position: pt(position),
        rotation,
        mirror,
    }
}

fn first_occurrence(engine: &Engine, instance: Id) -> Result<SymbolOccurrence> {
    engine
        .project()
        .occurrences_of(instance)
        .next()
        .cloned()
        .ok_or_else(|| Error::NotFound("occurrence".into()))
}

fn orient(id: Id, (rotation, mirror): (Rotation, bool)) -> Operation {
    Operation::Schematic(SchematicCommand::OrientUnit {
        occurrence: id,
        rotation,
        mirror,
    })
}

const PROBES: [Point; 3] = [[3.0, 1.0], [-2.5, 4.0], [0.0, -7.5]];

#[test]
fn sheet_rotate_and_mirror_compose_as_transforms() {
    let at = [10.0, -5.0];
    // Sheet actions about the origin, composed after the current frame.
    let turn = Transform2 {
        translate: [0.0, 0.0],
        rotation_deg: 90.0,
        mirror: false,
    };
    let flip = Transform2 {
        translate: [0.0, 0.0],
        rotation_deg: 180.0,
        mirror: true,
    };
    let about = |t: &Transform2| {
        Transform2::translation([-at[0], -at[1]])
            .then(t)
            .then(&Transform2::translation(at))
    };
    for rotation in ROTATIONS {
        for mirror in [false, true] {
            let start = schematic::occurrence_transform(&occurrence(at, rotation, mirror));
            let check = |(r, m): (Rotation, bool), expected: Transform2, what: &str| {
                let got = schematic::occurrence_transform(&occurrence(at, r, m));
                for p in PROBES {
                    assert!(close(got.apply(p), expected.apply(p)), "{what}: {p:?}");
                }
            };
            let rotated = schematic::rotated_on_sheet(rotation, mirror);
            check(rotated, start.then(&about(&turn)), "rotate");
            let mirrored = schematic::mirrored_on_sheet(rotation, mirror);
            check(mirrored, start.then(&about(&flip)), "mirror");
            // Rotate then mirror, and the reverse, are each the composition
            // of the two sheet actions in that order; they differ.
            let rm = schematic::mirrored_on_sheet(rotated.0, rotated.1);
            check(
                rm,
                start.then(&about(&turn)).then(&about(&flip)),
                "rotate, mirror",
            );
            let mr = schematic::rotated_on_sheet(mirrored.0, mirrored.1);
            check(
                mr,
                start.then(&about(&flip)).then(&about(&turn)),
                "mirror, rotate",
            );
            assert_ne!(rm, mr);
            // Four quarter turns, or two mirrors, are the identity.
            let mut four = (rotation, mirror);
            for _ in 0..4 {
                four = schematic::rotated_on_sheet(four.0, four.1);
            }
            assert_eq!(four, (rotation, mirror));
            assert_eq!(
                schematic::mirrored_on_sheet(mirrored.0, mirrored.1),
                (rotation, mirror)
            );
        }
    }
    // The stored mirror reflects across the local x-axis before the turn.
    let t = schematic::occurrence_transform(&occurrence(at, Rotation::R90, true));
    assert_eq!(t.apply([1.0, 2.0]), [at[0] + 2.0, at[1] + 1.0]);
    // A left-to-right flip negates local x on the sheet.
    let (r, m) = schematic::mirrored_on_sheet(Rotation::R0, false);
    let flipped = schematic::occurrence_transform(&occurrence(at, r, m));
    assert!(close(flipped.apply([1.0, 2.0]), [at[0] - 1.0, at[1] + 2.0]));
}

#[test]
fn orienting_moves_pins_and_their_wires_in_one_undo_step() -> Result<()> {
    let mut engine = Engine::demo()?;
    let (r1, r2) = (by_designator(&engine, "R1")?, by_designator(&engine, "R2")?);
    let wired = wire_between(&engine, ep(r1, 2), ep(r2, 1))?;
    let nets_before = engine.project().nets.clone();
    let before = engine.project().clone();
    let o = first_occurrence(&engine, r1)?;
    // R1 sits at (−12, 0) with pins at local (∓5, 0).
    assert_eq!(o.position, pt([-12.0, 0.0]));
    let pin = |e: &Engine, t| schematic::pin_position(e.project(), ep(r1, t));
    assert_eq!(pin(&engine, 1), Some([-17.0, 0.0]));

    let revision = engine.revision();
    apply(&mut engine, orient(o.id, (Rotation::R90, false)))?;
    assert_eq!(engine.revision(), revision + 1);
    assert_eq!(pin(&engine, 1), Some([-12.0, -5.0]));
    assert_eq!(pin(&engine, 2), Some([-12.0, 5.0]));
    // The wire still joins the same terminals; its drawn end is the pin.
    let wire = engine
        .project()
        .wires
        .iter()
        .find(|w| w.id == wired)
        .ok_or_else(|| Error::NotFound("wire".into()))?;
    assert_eq!((wire.from, wire.to), (ep(r1, 2).into(), ep(r2, 1).into()));
    assert_eq!(engine.project().nets, nets_before);
    let placed = schematic::placed_unit(engine.project(), o.id)?;
    for p in &placed.pins {
        assert_eq!(
            Some(p.end),
            schematic::pin_position(engine.project(), ep(r1, p.terminal.0))
        );
    }
    // Pick and fit follow the drawing: the pin is hit where it now is.
    let m = TextMetric::default();
    assert_eq!(
        placed.hit([-12.0, -4.0], 0.1, m),
        Some(crate::library::SymbolHit::Pin(crate::TerminalId(1)))
    );
    let extent = placed
        .bounds(m)
        .ok_or_else(|| Error::NotFound("bounds".into()))?;
    assert!(extent.contains([-12.0, -5.0]) && extent.contains([-12.0, 5.0]));

    // Mirror left to right on the sheet after the turn.
    let (r, mirror) = schematic::mirrored_on_sheet(Rotation::R90, false);
    apply(&mut engine, orient(o.id, (r, mirror)))?;
    assert_eq!((r, mirror), (Rotation::R90, true));
    assert_eq!(pin(&engine, 1), Some([-12.0, -5.0]));
    let rotated = engine.project().clone();
    apply(
        &mut engine,
        orient(o.id, schematic::mirrored_on_sheet(Rotation::R0, false)),
    )?;
    assert_eq!(pin(&engine, 1), Some([-7.0, 0.0]));
    assert_eq!(pin(&engine, 2), Some([-17.0, 0.0]));

    engine.undo()?;
    assert_eq!(engine.project(), &rotated);
    engine.undo()?;
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    engine.redo()?;
    engine.redo()?;
    assert_eq!(engine.project(), &rotated);

    // An unknown occurrence leaves the document and history unchanged.
    let kept = Snapshot::of(&engine);
    let missing = engine.apply(engine.revision(), orient(999_999, (Rotation::R90, true)));
    assert_eq!(missing.err().map(|e| e.code()), Some("not_found"));
    kept.assert_kept(&engine, "unknown occurrence");
    Ok(())
}

/// The box `local` text covers once placed through `t`: the image of its
/// local box.
fn image(
    t: &Transform2,
    at: Point,
    height: f64,
    rotation: f64,
    justify: Justify,
    text: &str,
) -> Bounds {
    let local =
        TextMetric::default().bounds(at, height, rotation, justify, TextSize::of(text, 1.0));
    let corners = [
        local.min,
        [local.max[0], local.min[1]],
        local.max,
        [local.min[0], local.max[1]],
    ];
    Bounds::of(corners.map(|c| t.apply(c))).unwrap_or(local)
}

fn assert_upright(placed: &PlacedText, expected: Bounds, what: &str) {
    let m = TextMetric::default();
    let size = TextSize::of(&placed.text, placed.line_spacing);
    // Readable glyphs: never reflected, never reading down or leftward.
    assert!(!placed.mirrored, "{what}");
    let (rotation, justify) = upright(placed.rotation_deg, placed.justify);
    assert!(rotation == 0.0 || rotation == 90.0, "{what}: {rotation}");
    assert_eq!(placed.lines().0, rotation, "{what}");
    // What is drawn covers exactly the image of the local box.
    let drawn = m.bounds(placed.at, placed.height, rotation, justify, size);
    assert!(
        close_bounds(drawn, expected),
        "{what}: {drawn:?} vs {expected:?}"
    );
    assert!(close_bounds(placed.bounds(m), expected), "{what}");
}

#[test]
fn text_stays_upright_and_covers_its_image_in_every_orientation() -> Result<()> {
    let mut engine = Engine::demo()?;
    show_demo_pin_numbers(&mut engine)?;
    let r1 = by_designator(&engine, "R1")?;
    let o = first_occurrence(&engine, r1)?;
    let instance = engine
        .project()
        .instance(r1)
        .cloned()
        .ok_or_else(|| Error::NotFound("R1".into()))?;
    let symbol = instances::resolve(&engine.project().library, &instance)?
        .symbol
        .definition
        .clone();
    let unit = symbol.units[0].clone();
    // Pin names and numbers are drawn at the symbol's pin text height.
    let height = symbol.pin_text_height_nm.to_mm_f64();
    for rotation in ROTATIONS {
        for mirror in [false, true] {
            apply(&mut engine, orient(o.id, (rotation, mirror)))?;
            let what = format!("{}° mirror {mirror}", rotation.degrees());
            let occurrence = first_occurrence(&engine, r1)?;
            let t = schematic::occurrence_transform(&occurrence);
            let placed = schematic::placed_unit(engine.project(), o.id)?;
            for text in &placed.texts {
                let local = unit
                    .graphics
                    .iter()
                    .find(|g| Some(g.id) == text.graphic)
                    .and_then(|g| match &g.item {
                        Item::Text(local) => Some(local),
                        Item::Shape { .. } => None,
                    })
                    .ok_or_else(|| Error::NotFound("text graphic".into()))?;
                if matches!(rotation, Rotation::R90 | Rotation::R270) {
                    // Turned a quarter, the designator and value stay
                    // horizontal beside the body, clear of it and of the
                    // pins, which now run up and down.
                    let m = TextMetric::default();
                    let b = text.bounds(m);
                    assert_eq!(text.rotation_deg, 0.0, "{what}: {}", text.text);
                    let body = placed.shapes[0].shape.bounds(placed.shapes[0].width);
                    assert!(
                        b.max[0] < body.min[0] || b.min[0] > body.max[0],
                        "{what}: {} {b:?} over the body {body:?}",
                        text.text
                    );
                    for pin in &placed.pins {
                        let stub = Bounds::point(pin.end).including(pin.body_end);
                        assert!(
                            b.max[0] < stub.min[0] || b.min[0] > stub.max[0],
                            "{what}: {} over a pin",
                            text.text
                        );
                    }
                    continue;
                }
                let expected = image(
                    &t,
                    local.at.to_mm_f64(),
                    text.height,
                    local.rotation.degrees(),
                    local.justify,
                    &text.text,
                );
                assert_upright(text, expected, &format!("{what}: {}", text.text));
            }
            assert!(placed.pins.iter().all(|p| p.number.is_some()), "{what}");
            for (pin, local) in placed.pins.iter().zip(&unit.pins) {
                if let Some(name) = &pin.name {
                    let (at, r, justify) = local.name_anchor();
                    let expected = image(&t, at, height, r, justify, &name.text);
                    assert_upright(name, expected, &format!("{what}: pin name"));
                }
                if let Some(number) = &pin.number {
                    let (at, r, justify) = local.number_anchor(0.0);
                    let expected = image(&t, at, height, r, justify, &number.text);
                    assert_upright(number, expected, &format!("{what}: pin number"));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn orient_unit_round_trips_as_json_and_rejects_bad_params() -> Result<()> {
    let mut engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let o = first_occurrence(&engine, r1)?;
    let request = json!({"editor": "Schematic", "operation": {
        "command": "symbol.orient_unit",
        "params": {"occurrence": o.id, "rotation": "r270", "mirror": true}
    }});
    let operation: Operation =
        serde_json::from_value(request.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
    assert_eq!(
        serde_json::to_value(&operation).map_err(|e| Error::Invalid(e.to_string()))?,
        request
    );
    apply(&mut engine, operation)?;
    let saved = crate::format::encode(&engine.to_file())?;
    let value: serde_json::Value =
        serde_json::from_slice(&saved).map_err(|e| Error::Invalid(e.to_string()))?;
    let stored = &value["project"]["occurrences"][0];
    assert_eq!(stored["rotation"], "r270");
    assert_eq!(stored["mirror"], true);
    let reopened = crate::format::decode(&saved)?;
    assert_eq!(reopened, engine.to_file());
    for bad in [
        json!({"occurrence": o.id, "rotation": "r45", "mirror": false}),
        json!({"occurrence": o.id, "rotation": 90, "mirror": false}),
        json!({"occurrence": o.id, "rotation": "r90"}),
        json!({"occurrence": o.id, "rotation": "r90", "mirror": false, "about": [0, 0]}),
    ] {
        let request = json!({"editor": "Schematic", "operation": {
            "command": "symbol.orient_unit", "params": bad
        }});
        assert!(
            serde_json::from_value::<Operation>(request).is_err(),
            "{bad}"
        );
    }
    // An occurrence missing its orientation is not a current document.
    let mut old = value.clone();
    if let Some(o) = old["project"]["occurrences"][0].as_object_mut() {
        o.remove("rotation");
        o.remove("mirror");
    }
    let bytes = serde_json::to_vec(&old).map_err(|e| Error::Invalid(e.to_string()))?;
    assert!(crate::format::decode(&bytes).is_err());
    Ok(())
}
