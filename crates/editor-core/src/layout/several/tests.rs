//! `layout.delete_objects` and `layout.lock_objects` through the engine.
use super::*;
use crate::layout::{LayoutCommand, RouteRequest};
use crate::tests::support::by_designator;
use crate::{Engine, Layer, Operation};

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The demo with a two-segment stub from R1's pad 2 to an open end.
fn stub() -> Fallible<(Engine, Vec<Id>)> {
    let mut engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Move {
            instance: r1,
            position: [10.0, 10.0],
            rotation_deg: 0.0,
            layer: Layer::Top,
        }),
    )?;
    let net = engine
        .project()
        .net_of(crate::Endpoint {
            instance: r1,
            terminal: crate::TerminalId(2),
        })
        .map(|n| n.id)
        .ok_or("no net")?;
    for (start, end) in [([11.2, 10.0], [14.0, 10.0]), ([14.0, 10.0], [14.0, 13.0])] {
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Route {
                request: RouteRequest {
                    net,
                    start,
                    end,
                    width: 0.3,
                    layer: Layer::Top.into(),
                },
            }),
        )?;
    }
    let tracks = engine.project().tracks.iter().map(|t| t.id).collect();
    Ok((engine, tracks))
}

/// Both tracks go in one undo step; a locked one refuses the whole delete
/// and changes nothing; lock and unlock take every named object at once.
#[test]
fn delete_and_lock_several_objects_in_one_undo_step() -> Fallible {
    let (mut engine, tracks) = stub()?;
    assert_eq!(tracks.len(), 2);
    let before = engine.project().clone();
    let objects = Objects {
        tracks: tracks.clone(),
        ..Default::default()
    };
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::DeleteObjects {
            objects: objects.clone(),
        }),
    )?;
    assert!(engine.project().tracks.is_empty());
    engine.undo()?;
    assert_eq!(engine.project(), &before, "one undo step");

    let r1 = by_designator(&engine, "R1")?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::LockObjects {
            objects: Objects {
                parts: vec![r1],
                tracks: vec![tracks[1]],
                ..Default::default()
            },
            locked: true,
        }),
    )?;
    let p = engine.project();
    assert!(p.placement_of(r1).is_some_and(|p| p.locked));
    assert_eq!(
        p.tracks.iter().map(|t| t.locked).collect::<Vec<_>>(),
        [false, true]
    );
    let (locked, revision) = (engine.project().clone(), engine.revision());
    let refused = engine.apply(
        revision,
        Operation::Layout(LayoutCommand::DeleteObjects { objects }),
    );
    assert!(refused.is_err());
    assert_eq!(engine.project(), &locked);
    assert_eq!(engine.revision(), revision);
    for bad in [
        Objects::default(),
        Objects {
            tracks: vec![tracks[0], tracks[0]],
            ..Default::default()
        },
        Objects {
            parts: vec![r1],
            ..Default::default()
        },
    ] {
        let delete = Operation::Layout(LayoutCommand::DeleteObjects { objects: bad });
        assert!(engine.apply(revision, delete).is_err());
        assert_eq!(engine.revision(), revision);
    }
    Ok(())
}
