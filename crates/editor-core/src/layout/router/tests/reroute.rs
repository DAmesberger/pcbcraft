//! A move whose dragged track the router cannot re-route: the edit still
//! applies, the track goes and its connection is left unrouted (an
//! airwire), never a rule violation; one undo restores the board exactly.
use super::*;
use crate::layout::tests::{SHARED, ids};
use crate::layout::{CopperGraph, checks};
use crate::library::{Outline, Segment};
use crate::model::Layer;
use crate::model::fill::{KeepoutLayers, KeepoutRules, KeepoutSpec};

type Fallible = std::result::Result<(), Box<dyn std::error::Error>>;

fn rectangle(x0: f64, y0: f64, x1: f64, y1: f64) -> Outline {
    Outline {
        start: [x0, y0],
        segments: vec![
            Segment::Line { to: [x1, y0] },
            Segment::Line { to: [x1, y1] },
            Segment::Line { to: [x0, y1] },
            Segment::Line { to: [x0, y0] },
        ],
    }
}

fn mv(instance: Id, position: [f64; 2]) -> Operation {
    Operation::Layout(LayoutCommand::Move {
        instance,
        position,
        rotation_deg: 0.0,
        layer: Layer::Top,
    })
}

#[test]
fn a_dragged_track_with_no_way_through_is_left_unrouted() -> Fallible {
    let mut engine = Engine::demo()?;
    engine.apply(engine.revision(), Operation::Layout(LayoutCommand::Sync))?;
    let (r1, r2) = ids()?;
    for (instance, position) in [(r1, [10.0, 10.0]), (r2, [25.0, 10.0])] {
        engine.apply(engine.revision(), mv(instance, position))?;
    }
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net: SHARED,
                start: [11.2, 10.0],
                end: [23.8, 10.0],
                width: 0.3,
                layer: Top,
            },
        }),
    )?;
    // A closed box of track keepouts round where R2 goes: no route can
    // reach its pads.
    for wall in [
        rectangle(21.0, 16.0, 29.0, 16.5),
        rectangle(21.0, 23.5, 29.0, 24.0),
        rectangle(21.0, 16.0, 21.5, 24.0),
        rectangle(28.5, 16.0, 29.0, 24.0),
    ] {
        engine.apply(
            engine.revision(),
            Operation::Fill(layout::fill::FillCommand::AddKeepout {
                keepout: Box::new(KeepoutSpec {
                    name: String::new(),
                    outline: wall,
                    layers: KeepoutLayers::All,
                    forbid: KeepoutRules {
                        fills: true,
                        tracks: true,
                        vias: true,
                        pads: false,
                    },
                }),
            }),
        )?;
    }
    let before = engine.project().clone();
    let errors = |p: &Project| {
        checks(p, &Default::default())
            .into_iter()
            .filter(|f| f.blocks.is_some() || f.code == "keepout.track")
            .map(|f| f.code)
            .collect::<Vec<_>>()
    };
    assert_eq!(errors(&before), Vec::<String>::new());
    assert_eq!(CopperGraph::of(&before).airwires().len(), 0);

    engine.apply(engine.revision(), mv(r2, [25.0, 20.0]))?;
    // The move applied; the track that could not follow is gone.
    assert_eq!(
        engine.project().placement_of(r2).map(|p| p.position),
        Some([25.0, 20.0])
    );
    assert!(engine.project().tracks.is_empty());
    assert_eq!(errors(engine.project()), Vec::<String>::new());
    assert_eq!(CopperGraph::of(engine.project()).airwires().len(), 1);
    let report = engine.rerouted();
    assert!(report.rerouted.is_empty());
    assert_eq!(report.unrouted, vec![before.net_name(SHARED).to_owned()]);
    assert_eq!(
        report.line().as_deref(),
        Some(format!("1 connection left unrouted ({})", before.net_name(SHARED)).as_str())
    );

    engine.undo()?;
    assert_eq!(engine.project(), &before, "one undo step");
    Ok(())
}
