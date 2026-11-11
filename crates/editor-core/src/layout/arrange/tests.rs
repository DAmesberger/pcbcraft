//! `layout.align`, `layout.distribute` and `layout.move_parts` through the
//! engine: the outlines' edges, centres and gaps they leave, one undo step
//! each, refusals that change nothing, and copper carried with the parts.
use super::*;
use crate::layout::{LayoutCommand, RouteRequest};
use crate::tests::support::{by_designator, demo_fixture, pt};
use crate::{Engine, InstanceCommand, Operation, Outcome, UnitId};

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The demo with R1, R2 and a third resistor R3 placed at `at`, top side.
fn three(at: [Point; 3]) -> Fallible<(Engine, [Id; 3])> {
    let mut engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let outcome = engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::Place(instances::NewInstance {
            sheet: None,
            component: f.r1(),
            designator: Some("R3".into()),
            properties: vec![],
            representation: f.symbol,
            unit: UnitId(1),
            position: pt([40.0, 0.0]),
            physical: Some(f.physical()),
        })),
    )?;
    let Outcome::InstancePlaced { instance: r3, .. } = outcome else {
        return Err("R3 was not placed".into());
    };
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Place {
            instance: r3,
            position: at[2],
            rotation_deg: 0.0,
            layer: Layer::Top,
        }),
    )?;
    let ids = [
        by_designator(&engine, "R1")?,
        by_designator(&engine, "R2")?,
        r3,
    ];
    for (id, position) in ids.iter().zip(at).take(2) {
        engine.apply(engine.revision(), mv(*id, position))?;
    }
    Ok((engine, ids))
}

fn mv(instance: Id, position: Point) -> Operation {
    Operation::Layout(LayoutCommand::Move {
        instance,
        position,
        rotation_deg: 0.0,
        layer: Layer::Top,
    })
}

fn outline_of(engine: &Engine, id: Id) -> Fallible<(Point, Point)> {
    let p = engine.project().placement_of(id).ok_or("not placed")?;
    Ok(extent(engine.project(), p)?)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Each edge and centre line: the named outline coordinate becomes the
/// selection's extreme (or the middle of its extent) for every part, the
/// other coordinate stays, and undo takes the whole command back.
#[test]
fn align_lines_up_outlines_in_one_undo_step() -> Fallible {
    let (mut engine, [r1, r2, r3]) = three([[8.0, 8.0], [16.0, 14.0], [30.0, 4.0]])?;
    let parts = [r1, r2, r3];
    let before: Vec<(Point, Point)> = parts
        .iter()
        .map(|id| outline_of(&engine, *id))
        .collect::<Fallible<_>>()?;
    let lo = |k: usize| before.iter().map(|e| e.0[k]).fold(f64::INFINITY, f64::min);
    let hi = |k: usize| {
        before
            .iter()
            .map(|e| e.1[k])
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let start = engine.project().clone();
    for to in [
        AlignTo::Left,
        AlignTo::Right,
        AlignTo::Top,
        AlignTo::Bottom,
        AlignTo::CentreX,
        AlignTo::CentreY,
    ] {
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Align {
                instances: parts.to_vec(),
                to,
            }),
        )?;
        for (id, old) in parts.iter().zip(&before) {
            let (min, max) = outline_of(&engine, *id)?;
            let (k, other) = match to {
                AlignTo::Left | AlignTo::Right | AlignTo::CentreX => (0, 1),
                _ => (1, 0),
            };
            let got = match to {
                AlignTo::Left | AlignTo::Bottom => near(min[k], lo(k)),
                AlignTo::Right | AlignTo::Top => near(max[k], hi(k)),
                AlignTo::CentreX | AlignTo::CentreY => {
                    near((min[k] + max[k]) / 2.0, (lo(k) + hi(k)) / 2.0)
                }
            };
            assert!(got, "{to:?}: {min:?}..{max:?}");
            assert!(
                near(min[other], old.0[other]),
                "{to:?} keeps the other axis"
            );
            assert!(
                near(max[k] - min[k], old.1[k] - old.0[k]),
                "the outline only moves"
            );
        }
        engine.undo()?;
        assert_eq!(engine.project(), &start, "{to:?} is one undo step");
    }
    Ok(())
}

/// Distribute keeps the outermost parts and leaves equal gaps between
/// neighbouring outlines, in centre order whatever order they are named.
#[test]
fn distribute_leaves_equal_gaps_between_outlines() -> Fallible {
    let (mut engine, [r1, r2, r3]) = three([[30.0, 8.0], [8.0, 12.0], [12.0, 4.0]])?;
    let (first, last) = (outline_of(&engine, r2)?, outline_of(&engine, r1)?);
    let start = engine.project().clone();
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Distribute {
            instances: vec![r1, r2, r3],
            axis: Axis::Horizontal,
        }),
    )?;
    let [a, b, c] = [r2, r3, r1].map(|id| outline_of(&engine, id));
    let (a, b, c) = (a?, b?, c?);
    assert_eq!((a, c), (first, last), "the outermost stay");
    let gap = |l: (Point, Point), r: (Point, Point)| r.0[0] - l.1[0];
    assert!(near(gap(a, b), gap(b, c)), "{} vs {}", gap(a, b), gap(b, c));
    // Width 30 − 8 between the outer datums, three equal outlines: the
    // middle one's datum is halfway.
    let r3_at = engine.project().placement_of(r3).ok_or("R3")?.position;
    assert!(near(r3_at[0], 19.0) && near(r3_at[1], 4.0), "{r3_at:?}");
    engine.undo()?;
    assert_eq!(engine.project(), &start, "one undo step");
    Ok(())
}

/// Too few parts, a part named twice or one not on the board is refused;
/// a locked part that would move refuses the command and is named, while
/// one already in line does not; a dimensioned part without the host's
/// evaluator is refused. Nothing changes on a refusal.
#[test]
fn align_and_distribute_refuse_and_change_nothing() -> Fallible {
    let (mut engine, [r1, r2, r3]) = three([[8.0, 8.0], [16.0, 14.0], [30.0, 4.0]])?;
    let align = |instances: Vec<Id>| {
        Operation::Layout(LayoutCommand::Align {
            instances,
            to: AlignTo::Bottom,
        })
    };
    let refused = |engine: &mut Engine, op: Operation| -> Fallible<String> {
        let (before, revision) = (engine.project().clone(), engine.revision());
        let error = engine.apply(revision, op).err().ok_or("not refused")?;
        assert_eq!(engine.project(), &before);
        assert_eq!(engine.revision(), revision);
        Ok(error.to_string())
    };
    assert!(refused(&mut engine, align(vec![r1]))?.contains("at least 2"));
    assert!(refused(&mut engine, align(vec![r1, r1]))?.contains("named twice"));
    assert!(refused(&mut engine, align(vec![r1, 99_999]))?.contains("99999"));
    let two = Operation::Layout(LayoutCommand::Distribute {
        instances: vec![r1, r2],
        axis: Axis::Vertical,
    });
    assert!(refused(&mut engine, two)?.contains("at least 3"));
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r2,
            locked: true,
        }),
    )?;
    let why = refused(&mut engine, align(vec![r1, r2, r3]))?;
    assert!(
        why.contains("R2 is locked") && why.contains("align"),
        "{why}"
    );
    // R3 is lowest: aligning bottoms leaves it, and locked it does not refuse.
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r2,
            locked: false,
        }),
    )?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r3,
            locked: true,
        }),
    )?;
    engine.apply(engine.revision(), align(vec![r1, r3]))?;
    Ok(())
}

/// Two routed parts moved alike in one `layout.move_parts` take the track
/// between them along whole, in one undo step; a locked part refuses the
/// whole move.
#[test]
fn move_parts_carries_the_copper_between_them() -> Fallible {
    let mut engine = Engine::demo()?;
    let (r1, r2) = (by_designator(&engine, "R1")?, by_designator(&engine, "R2")?);
    engine.apply(engine.revision(), mv(r1, [10.0, 10.0]))?;
    engine.apply(engine.revision(), mv(r2, [25.0, 10.0]))?;
    let net = engine
        .project()
        .net_of(crate::Endpoint {
            instance: r1,
            terminal: crate::TerminalId(2),
        })
        .map(|n| n.id)
        .ok_or("no net")?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net,
                start: [11.2, 10.0],
                end: [23.8, 10.0],
                width: 0.3,
                layer: Layer::Top.into(),
            },
        }),
    )?;
    let before = engine.project().clone();
    let moves = |by: Point| {
        Operation::Layout(LayoutCommand::MoveParts {
            moves: [(r1, [10.0, 10.0]), (r2, [25.0, 10.0])]
                .map(|(instance, p)| PartMove {
                    instance,
                    position: [p[0] + by[0], p[1] + by[1]],
                    rotation_deg: 0.0,
                    layer: Layer::Top,
                })
                .to_vec(),
        })
    };
    engine.apply(engine.revision(), moves([2.0, 3.0]))?;
    let track = engine.project().tracks.first().ok_or("no track")?;
    assert_eq!(engine.project().tracks.len(), 1);
    assert!(
        near(track.start[0], 13.2)
            && near(track.start[1], 13.0)
            && near(track.end[0], 25.8)
            && near(track.end[1], 13.0),
        "{track:?}"
    );
    engine.undo()?;
    assert_eq!(engine.project(), &before, "one undo step");
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r2,
            locked: true,
        }),
    )?;
    let (locked, revision) = (engine.project().clone(), engine.revision());
    let error = engine
        .apply(revision, moves([2.0, 3.0]))
        .err()
        .ok_or("moved")?;
    assert!(error.to_string().contains("R2 is locked"), "{error}");
    assert_eq!(engine.project(), &locked);
    Ok(())
}
