//! Labels and power ports attached to pins place themselves: on the pin's
//! end, facing away from the symbol on every side and through every
//! rotation and mirror, clear of symbol bodies and of each other on a
//! dense pin row (docs/schematic-capture.md, "Labels").
use super::support::*;
use crate::geometry::{Bounds, Point};
use crate::ids::GraphicId;
use crate::library::{Fill, Rotation, TextMetric, graphics::body_rect};
use crate::model::{LabelKind, PortShape, PowerStyle};
use crate::schematic::{SchematicCommand as C, WireTarget, labels, placed_unit, scene};
use crate::{Engine, Error, Id, Operation, Outcome, Result, SheetId};

/// The fixture body every pin points into, mm.
const BODY: Bounds = Bounds {
    min: [-3.0, -3.0],
    max: [3.0, 3.0],
};

/// Twelve passive terminals, three on each side of a 6 mm body at the
/// 2.5 mm module: 1–3 left, 4–6 right, 7–9 top, 10–12 bottom.
fn four_sides(engine: &mut Engine) -> Result<Fixture> {
    let rows = [-2.5, 0.0, 2.5];
    let mut pins: Vec<(u32, Point)> = Vec::new();
    let sides: [fn(f64) -> Point; 4] = [|r| [-5.0, r], |r| [5.0, r], |r| [r, 5.0], |r| [r, -5.0]];
    for (side, at) in sides.iter().enumerate() {
        for (i, r) in rows.iter().enumerate() {
            pins.push(((side * 3 + i + 1) as u32, at(*r)));
        }
    }
    let names: Vec<String> = (1..=12).map(|i| format!("P{i}")).collect();
    let designations: Vec<String> = (1..=12).map(|i| i.to_string()).collect();
    let spec = Spec {
        name: "Four sides",
        terminals: names
            .iter()
            .enumerate()
            .map(|(i, n)| (i as u32 + 1, n.as_str(), passive()))
            .collect(),
        units: vec![(1, "A", pins)],
        package: designations
            .iter()
            .enumerate()
            .map(|(i, d)| (i as u32 + 1, d.as_str()))
            .collect(),
        mapping: (1..=12).map(|i| (i, i)).collect(),
        physical: true,
    };
    let f = draft(engine, &spec)?;
    let body = body_rect(GraphicId(0), BODY)?;
    with(engine, f.symbol, symbol_body, |s| {
        let id = GraphicId(s.next_local);
        s.next_local += 1;
        if let Some(unit) = s.units.first_mut() {
            unit.graphics
                .push(crate::library::Graphic { id, ..body.clone() });
        }
    })?;
    publish(engine, f.all())?;
    Ok(f)
}

fn place(engine: &mut Engine, f: &Fixture, designator: &str, at: Point) -> Result<Id> {
    let sheet = engine.project().sheets.first();
    match apply(
        engine,
        Operation::Instance(crate::InstanceCommand::Place(
            crate::instances::NewInstance {
                sheet: Some(sheet),
                component: f.r1(),
                designator: Some(designator.into()),
                properties: vec![],
                representation: f.symbol,
                unit: crate::UnitId(1),
                position: pt(at),
                physical: Some(f.physical()),
            },
        )),
    )? {
        Outcome::InstancePlaced { instance, .. } => Ok(instance),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

fn run(engine: &mut Engine, command: C) -> Result<Outcome> {
    apply(engine, Operation::Schematic(command))
}

fn attach(engine: &mut Engine, name: &str, kind: LabelKind, instance: Id, t: u32) -> Result<Id> {
    // Placed where a client might put it, off to the right: the label
    // places itself regardless.
    let at = crate::schematic::pin_position(engine.project(), ep(instance, t))
        .ok_or_else(|| Error::Invalid("pin".into()))?;
    match run(
        engine,
        C::PlaceLabel {
            sheet: None,
            name: name.into(),
            kind,
            position: pt([at[0] + 5.0, at[1]]),
            rotation: Rotation::R0,
            attach: Some(WireTarget::Terminal(ep(instance, t))),
        },
    )? {
        Outcome::LabelPlaced { label } => Ok(label),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

/// Each label kind on each side: a dense row of three per side.
fn kinds() -> [LabelKind; 4] {
    [
        LabelKind::Global {
            shape: PortShape::Bidirectional,
        },
        LabelKind::Local,
        LabelKind::Power {
            style: PowerStyle::Ground,
        },
        LabelKind::Power {
            style: PowerStyle::Bar,
        },
    ]
}

fn overlap(a: &Bounds, b: &Bounds) -> bool {
    a.min[0] < b.max[0] && b.min[0] < a.max[0] && a.min[1] < b.max[1] && b.min[1] < a.max[1]
}

/// Every stroke and text box of a drawing.
fn ink(d: &scene::Drawing, m: TextMetric) -> Vec<Bounds> {
    d.strokes
        .iter()
        .map(|s| s.shape.bounds(s.width))
        .chain(d.texts.iter().map(|t| t.bounds(m)))
        .collect()
}

/// What overlaps on `sheet`: a label's ink over a symbol body, or two
/// labels' ink over each other.
fn label_overlaps(project: &crate::Project, sheet: SheetId) -> Vec<String> {
    let m = scene::LABEL_METRIC;
    let bodies: Vec<(String, Bounds)> = project
        .occurrences
        .iter()
        .filter(|o| o.sheet == sheet)
        .filter_map(|o| {
            let unit = placed_unit(project, o.id).ok()?;
            let name = project
                .instance(o.instance)
                .map(|i| i.designator.clone())
                .unwrap_or_default();
            Some(
                unit.shapes
                    .into_iter()
                    .filter(|s| s.fill == Fill::Body)
                    .map(move |s| (name.clone(), s.shape.bounds(0.0)))
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect();
    let drawn = scene::sheet_labels(project, sheet);
    let name = |id: Id| {
        project
            .label(id)
            .map(|l| l.name.clone())
            .unwrap_or_default()
    };
    let mut found = Vec::new();
    for (i, (id, d)) in drawn.iter().enumerate() {
        let boxes = ink(d, m);
        for (designator, body) in &bodies {
            if boxes.iter().any(|b| overlap(b, body)) {
                found.push(format!("{} ({id}) over {designator}", name(*id)));
            }
        }
        for (other, e) in drawn.iter().skip(i + 1) {
            let theirs = ink(e, m);
            if boxes.iter().any(|b| theirs.iter().any(|t| overlap(b, t))) {
                found.push(format!(
                    "{} ({id}) over {} ({other})",
                    name(*id),
                    name(*other)
                ));
            }
        }
    }
    found
}

#[test]
fn labels_on_all_four_sides_face_away_and_clear_bodies_and_each_other() -> Result<()> {
    let mut engine = Engine::default();
    let f = four_sides(&mut engine)?;
    let orientations = [
        (Rotation::R0, false),
        (Rotation::R90, false),
        (Rotation::R180, true),
        (Rotation::R270, true),
    ];
    let mut placed = Vec::new();
    for (k, (rotation, mirror)) in orientations.iter().enumerate() {
        let u = place(
            &mut engine,
            &f,
            &format!("U{}", k + 1),
            [k as f64 * 40.0, 0.0],
        )?;
        let occurrence = engine
            .project()
            .occurrences_of(u)
            .next()
            .map(|o| o.id)
            .ok_or_else(|| Error::Invalid("occurrence".into()))?;
        run(
            &mut engine,
            C::OrientUnit {
                occurrence,
                rotation: *rotation,
                mirror: *mirror,
            },
        )?;
        for (side, kind) in kinds().into_iter().enumerate() {
            for i in 0..3 {
                let t = (side * 3 + i + 1) as u32;
                let id = attach(&mut engine, &format!("N{k}_{t}"), kind, u, t)?;
                placed.push((id, u, t));
            }
        }
    }
    let project = engine.project();
    let sheet = project.sheets.first();
    let overlaps = label_overlaps(project, sheet);
    assert!(overlaps.is_empty(), "{overlaps:#?}");
    // Each sits on its pin's end and lies beyond it, away from the body.
    let m = TextMetric::default();
    let drawn = scene::sheet_labels(project, sheet);
    for (id, u, t) in placed {
        let label = project
            .label(id)
            .ok_or_else(|| Error::Invalid("label".into()))?;
        assert!(!label.manual);
        let end = crate::schematic::pin_position(project, ep(u, t))
            .ok_or_else(|| Error::Invalid("pin".into()))?;
        assert_eq!(labels::anchor(project, label), end, "{}", label.name);
        let (_, d) =
            labels::pin_frame(project, label).ok_or_else(|| Error::Invalid("frame".into()))?;
        let body = project
            .occurrences_of(u)
            .next()
            .and_then(|o| placed_unit(project, o.id).ok())
            .and_then(|unit| unit.bounds(m))
            .ok_or_else(|| Error::Invalid("unit".into()))?;
        let centre = body.center();
        let inward = [centre[0] - end[0], centre[1] - end[1]];
        assert!(
            inward[0] * d[0] + inward[1] * d[1] < 0.0,
            "{} faces out",
            label.name
        );
        let drawing = drawn
            .iter()
            .find(|(l, _)| *l == id)
            .map(|(_, d)| d)
            .ok_or_else(|| Error::Invalid("drawn".into()))?;
        let extent = drawing
            .bounds(m)
            .ok_or_else(|| Error::Invalid("ink".into()))?;
        let c = extent.center();
        // A port standing upright on a horizontal pin lies across it.
        let upright = labels::current_rotation(project, id)? == Rotation::R0
            && matches!(label.kind, LabelKind::Power { .. })
            && d[0] != 0.0;
        assert!(
            upright || (c[0] - end[0]) * d[0] + (c[1] - end[1]) * d[1] > 0.5,
            "{} lies beyond its pin",
            label.name
        );
    }
    // The wire to each label has no length left to draw.
    let scene = scene::sheet_scene(project, sheet);
    assert!(scene.wires.is_empty(), "{:?}", scene.wires);
    Ok(())
}

#[test]
fn a_moved_or_turned_label_stays_where_a_person_put_it() -> Result<()> {
    let mut engine = Engine::default();
    let f = four_sides(&mut engine)?;
    let u = place(&mut engine, &f, "U1", [0.0, 0.0])?;
    let ground = LabelKind::Power {
        style: PowerStyle::Ground,
    };
    let moved = attach(&mut engine, "GND", ground, u, 1)?;
    let turned = attach(&mut engine, "SDA", LabelKind::Local, u, 4)?;
    // Pin 1 on the left, with other pins' text beside it.
    let project = engine.project();
    let before = project
        .label(moved)
        .ok_or_else(|| Error::Invalid("label".into()))?;
    let (_, d) =
        labels::pin_frame(project, before).ok_or_else(|| Error::Invalid("frame".into()))?;
    assert_eq!(d, [-1.0, 0.0]);
    let faced = labels::current_rotation(project, moved)?;
    // In line with pin 1, so its wire runs over no other pin (which would
    // connect to it).
    run(
        &mut engine,
        C::MoveLabel {
            label: moved,
            position: pt([-20.0, -2.5]),
        },
    )?;
    run(
        &mut engine,
        C::EditLabel {
            label: turned,
            name: None,
            kind: None,
            rotation: Some(Rotation::R90),
        },
    )?;
    let project = engine.project();
    let after = project
        .label(moved)
        .ok_or_else(|| Error::Invalid("label".into()))?;
    assert!(after.manual);
    assert_eq!(labels::anchor(project, after), [-20.0, -2.5]);
    assert_eq!(after.rotation, faced, "keeps the way it faced");
    let turned_label = project
        .label(turned)
        .ok_or_else(|| Error::Invalid("label".into()))?;
    assert!(turned_label.manual);
    assert_eq!(turned_label.rotation, Rotation::R90);
    let pin4 = crate::schematic::pin_position(project, ep(u, 4))
        .ok_or_else(|| Error::Invalid("pin".into()))?;
    assert_eq!(
        labels::anchor(project, turned_label),
        pin4,
        "stays on its pin"
    );
    // Moving the symbol leaves both where they were put.
    let occurrence = project
        .occurrences_of(u)
        .next()
        .map(|o| o.id)
        .ok_or_else(|| Error::Invalid("occurrence".into()))?;
    // Moved right by 10 mm, pin 1 would land on the SDA label left where
    // pin 4 was: GND and SDA would become one net, so that move is refused.
    let kept = Snapshot::of(&engine);
    let shorted = run(
        &mut engine,
        C::MoveUnit {
            occurrence,
            position: pt([10.0, 0.0]),
        },
    );
    assert!(
        matches!(&shorted, Err(Error::Conflict(m)) if m.starts_with("net.named_merge:")),
        "{shorted:?}"
    );
    kept.assert_kept(&engine, "a move that shorts GND to SDA");
    run(
        &mut engine,
        C::MoveUnit {
            occurrence,
            position: pt([0.0, 10.0]),
        },
    )?;
    let project = engine.project();
    assert_eq!(
        project.label(moved).map(|l| l.position),
        Some(pt([-20.0, -2.5]))
    );
    assert_eq!(project.label(turned).map(|l| l.position), Some(pt(pin4)));
    // The wire from the moved port to its pin is drawn again.
    let scene = scene::sheet_scene(project, project.sheets.first());
    assert_eq!(scene.wires.len(), 2);
    Ok(())
}

#[test]
fn a_self_placing_label_follows_its_pin_and_undo_restores_it() -> Result<()> {
    let mut engine = Engine::default();
    let f = four_sides(&mut engine)?;
    let u = place(&mut engine, &f, "U1", [0.0, 0.0])?;
    let id = attach(&mut engine, "EN", LabelKind::Local, u, 8)?;
    let occurrence = engine
        .project()
        .occurrences_of(u)
        .next()
        .map(|o| o.id)
        .ok_or_else(|| Error::Invalid("occurrence".into()))?;
    let anchor = |e: &Engine| {
        e.project()
            .label(id)
            .map(|l| labels::anchor(e.project(), l))
    };
    assert_eq!(anchor(&engine), Some([0.0, 5.0]));
    run(
        &mut engine,
        C::MoveUnit {
            occurrence,
            position: pt([10.0, -10.0]),
        },
    )?;
    assert_eq!(anchor(&engine), Some([10.0, -5.0]));
    engine.undo()?;
    assert_eq!(anchor(&engine), Some([0.0, 5.0]));
    // Nothing of the placement is stored: the label keeps the position it
    // was given and is not manual.
    let label = engine
        .project()
        .label(id)
        .ok_or_else(|| Error::Invalid("label".into()))?;
    assert_eq!(label.position, pt([5.0, 5.0]));
    assert!(!label.manual);
    Ok(())
}

/// e2e UX finding 11: a label and a power port on facing pins with too
/// little room between them for both ("SWCLKGND"). Neither may lie over
/// the other, a body or a pin; the one that cannot stand off along its
/// pin steps aside on a bent leader.
#[test]
fn facing_labels_without_room_between_them_do_not_overlap() -> Result<()> {
    for gap in [4.0, 6.0, 9.0] {
        let mut engine = Engine::default();
        let f = four_sides(&mut engine)?;
        // U1's right pins end at x = 5, U2's left pins at x = 5 + gap.
        let u1 = place(&mut engine, &f, "U1", [0.0, 0.0])?;
        let u2 = place(&mut engine, &f, "U2", [10.0 + gap, 0.0])?;
        let ground = LabelKind::Power {
            style: PowerStyle::Ground,
        };
        attach(&mut engine, "GND", ground, u1, 5)?;
        attach(&mut engine, "SWCLK", LabelKind::Local, u2, 2)?;
        attach(&mut engine, "SWDIO", LabelKind::Local, u2, 1)?;
        let project = engine.project();
        let sheet = project.sheets.first();
        let overlaps = label_overlaps(project, sheet);
        assert!(overlaps.is_empty(), "gap {gap}: {overlaps:#?}");
        // No label's ink lies on another pin's stub or end.
        let m = scene::LABEL_METRIC;
        let drawn = scene::sheet_labels(project, sheet);
        for o in &project.occurrences {
            let unit = placed_unit(project, o.id)?;
            for pin in &unit.pins {
                let stub = Bounds::point(pin.end).including(pin.body_end);
                for (id, d) in &drawn {
                    let label = project
                        .label(*id)
                        .ok_or_else(|| Error::Invalid("label".into()))?;
                    if labels::anchor(project, label) == pin.end {
                        continue;
                    }
                    assert!(
                        !ink(d, m).iter().any(|b| overlap(b, &stub)),
                        "gap {gap}: {} over a pin",
                        label.name
                    );
                }
            }
        }
    }
    Ok(())
}

/// One pin on each side of the 6 mm body: 1 left, 2 right, with room around.
fn two_sides(engine: &mut Engine) -> Result<Fixture> {
    let spec = Spec {
        name: "Two sides",
        terminals: vec![(1, "P1", passive()), (2, "P2", passive())],
        units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
        package: vec![(1, "1"), (2, "2")],
        mapping: vec![(1, 1), (2, 2)],
        physical: true,
    };
    let f = draft(engine, &spec)?;
    let body = body_rect(GraphicId(0), BODY)?;
    with(engine, f.symbol, symbol_body, |s| {
        let id = GraphicId(s.next_local);
        s.next_local += 1;
        if let Some(unit) = s.units.first_mut() {
            unit.graphics
                .push(crate::library::Graphic { id, ..body.clone() });
        }
    })?;
    publish(engine, f.all())?;
    Ok(f)
}

/// Run 3, finding 3: a port on a horizontal pin comes in upright on either
/// side of the symbol, a ground below the pin's end and a supply above it,
/// and a turn starts from that. With a neighbour's pin in the stem's way it
/// faces along its own pin; on a vertical pin it runs on along the pin.
#[test]
fn ports_on_horizontal_pins_stand_upright_where_they_have_room() -> Result<()> {
    let ground = LabelKind::Power {
        style: PowerStyle::Ground,
    };
    let supply = LabelKind::Power {
        style: PowerStyle::Bar,
    };
    for (kind, name, sign) in [(ground, "GND", -1.0), (supply, "+3V3", 1.0)] {
        for terminal in [1, 2] {
            let mut engine = Engine::default();
            let f = two_sides(&mut engine)?;
            let u = place(&mut engine, &f, "U1", [0.0, 0.0])?;
            let id = attach(&mut engine, name, kind, u, terminal)?;
            let project = engine.project();
            assert_eq!(labels::current_rotation(project, id)?, Rotation::R0);
            let end = crate::schematic::pin_position(project, ep(u, terminal))
                .ok_or_else(|| Error::Invalid("pin".into()))?;
            let drawn = scene::sheet_labels(project, project.sheets.first());
            let extent = drawn
                .iter()
                .find(|(l, _)| *l == id)
                .and_then(|(_, d)| d.bounds(TextMetric::default()))
                .ok_or_else(|| Error::Invalid("ink".into()))?;
            assert!(
                (extent.center()[1] - end[1]) * sign > 1.0,
                "{name} on pin {terminal}: {extent:?} from {end:?}"
            );
        }
    }
    // The middle pin of a row: the stem would cross the next pin.
    let mut engine = Engine::default();
    let f = four_sides(&mut engine)?;
    let u = place(&mut engine, &f, "U1", [0.0, 0.0])?;
    let id = attach(&mut engine, "GND", ground, u, 2)?;
    assert_eq!(
        labels::current_rotation(engine.project(), id)?,
        labels::auto_rotation(ground, [-1.0, 0.0])
    );
    // Pin 8 on top points up: the port continues up it, not over the body.
    let id = attach(&mut engine, "GND", ground, u, 8)?;
    assert_eq!(
        labels::current_rotation(engine.project(), id)?,
        labels::auto_rotation(ground, [0.0, 1.0])
    );
    Ok(())
}
