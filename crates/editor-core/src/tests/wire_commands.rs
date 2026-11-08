//! The `wire.*` edit commands through the public interface
//! (docs/schematic-wires.md, slice 3): each is one undo step, is refused as a
//! whole, stores what `wire_edit` previews, and keeps the nets where it
//! should. And `wire.draw` keeps a port on its pin.
use super::support::*;
use crate::model::{Corner, FreeEnd, LabelKind};
use crate::schematic::wire_edit::{self, WireSide};
use crate::schematic::{self, SchematicCommand as C, WirePoint, WireTarget};
use crate::{Engine, Error, Id, Operation, Outcome, Point, Result, WireEnd};

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

fn free(p: Point) -> Result<WireTarget> {
    FreeEnd::new(p)
        .map(WireTarget::Free)
        .ok_or(Error::Invalid("free".into()))
}

fn corners(points: &[Point]) -> Vec<Corner> {
    points.iter().map(|&at| Corner::sharp(at)).collect()
}

/// U1 and U2 side by side and a wire from U1.2 at (5, 0) through `through`
/// to U2.1 at (35, 0) or wherever U2 is put; the wire's ID.
fn wired(engine: &mut Engine, through: &[Point]) -> Result<(Id, Id, Id)> {
    let f = two_pin(engine)?;
    let (u1, _) = place(engine, &f, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(engine, &f, "U2", 1, [40.0, 20.0])?;
    run(
        engine,
        C::DrawWire {
            sheet: None,
            from: ep(u1, 2).into(),
            corners: corners(through),
            to: ep(u2, 1).into(),
        },
    )?;
    let wire = engine.project().wires[0].id;
    Ok((u1, u2, wire))
}

fn path(engine: &Engine, wire: Id) -> Result<Vec<Point>> {
    let project = engine.project();
    project
        .wire(wire)
        .and_then(|w| schematic::wire_path(project, w))
        .ok_or(Error::Invalid("no path".into()))
}

/// The dogleg U1.2 (5, 0) → (20, 0) → (20, 20) → U2.1 (35, 20).
const DOGLEG: [Point; 2] = [[20.0, 0.0], [20.0, 20.0]];

/// Every path edit: what the preview function gives is what the command
/// stores, the nets stay, and one undo restores the wires.
#[test]
fn each_edit_stores_its_preview_in_one_undo_step_and_keeps_the_nets() -> Result<()> {
    let mut engine = Engine::default();
    let (_, _, wire) = wired(&mut engine, &DOGLEG)?;
    let before = path(&engine, wire)?;
    let attached = [false, false];
    let cases: Vec<(&str, C, Vec<Point>)> = vec![
        (
            "move a corner",
            C::MoveWireCorner {
                wire,
                index: 0,
                to: [25.0, 5.0],
                free: false,
            },
            wire_edit::move_corner(&before, attached, 0, [25.0, 5.0], false)
                .ok_or(Error::Invalid("corner".into()))?,
        ),
        (
            "move a corner freely",
            C::MoveWireCorner {
                wire,
                index: 1,
                to: [25.0, 15.0],
                free: true,
            },
            wire_edit::move_corner(&before, attached, 1, [25.0, 15.0], true)
                .ok_or(Error::Invalid("corner".into()))?,
        ),
        (
            "move a segment",
            C::MoveWireSegment {
                wire,
                index: 1,
                offset: -5.0,
            },
            wire_edit::move_segment(&before, attached, 1, -5.0)
                .ok_or(Error::Invalid("segment".into()))?,
        ),
        (
            "move the segment on a pin: it steps",
            C::MoveWireSegment {
                wire,
                index: 0,
                offset: 2.5,
            },
            wire_edit::move_segment(&before, attached, 0, 2.5)
                .ok_or(Error::Invalid("segment".into()))?,
        ),
        (
            "insert a corner: the shape does not change",
            C::InsertWireCorner {
                wire,
                at: [20.0, 10.0],
            },
            wire_edit::insert_corner(&before, [20.0, 10.0])
                .ok_or(Error::Invalid("insert".into()))?
                .0,
        ),
        (
            "remove a corner",
            C::RemoveWireCorner { wire, index: 0 },
            wire_edit::remove_corner(&before, 0).ok_or(Error::Invalid("remove".into()))?,
        ),
        (
            "set the path",
            C::SetWirePath {
                wire,
                corners: corners(&[[10.0, 0.0], [10.0, 20.0]]),
                from: None,
                to: None,
            },
            vec![[5.0, 0.0], [10.0, 0.0], [10.0, 20.0], [35.0, 20.0]],
        ),
    ];
    let wires = engine.project().wires.clone();
    let nets = engine.project().nets.clone();
    for (case, command, preview) in cases {
        let revision = engine.revision();
        run(&mut engine, command)?;
        assert_eq!(path(&engine, wire)?, preview, "{case}: preview is result");
        assert_eq!(engine.project().wires.len(), 1, "{case}");
        assert_eq!(engine.project().nets, nets, "{case}: nets");
        assert_eq!(engine.revision(), revision + 1, "{case}");
        engine.undo()?;
        assert_eq!(engine.project().wires, wires, "{case}: one undo");
    }
    Ok(())
}

/// A refused edit leaves the document as it was.
#[test]
fn a_refused_edit_changes_nothing() -> Result<()> {
    let mut engine = Engine::default();
    let (_, _, wire) = wired(&mut engine, &DOGLEG)?;
    let cases: Vec<(&str, C)> = vec![
        (
            "no such wire",
            C::RemoveWireCorner {
                wire: 9999,
                index: 0,
            },
        ),
        (
            "no such corner",
            C::MoveWireCorner {
                wire,
                index: 2,
                to: [0.0, 0.0],
                free: false,
            },
        ),
        (
            "off the sheet's range",
            C::MoveWireCorner {
                wire,
                index: 0,
                to: [20000.0, 0.0],
                free: false,
            },
        ),
        (
            "not finite",
            C::MoveWireCorner {
                wire,
                index: 0,
                to: [f64::NAN, 0.0],
                free: false,
            },
        ),
        (
            "no such segment",
            C::MoveWireSegment {
                wire,
                index: 3,
                offset: 1.0,
            },
        ),
        (
            "an offset off the sheet",
            C::MoveWireSegment {
                wire,
                index: 1,
                offset: 1e9,
            },
        ),
        (
            "an offset not finite",
            C::MoveWireSegment {
                wire,
                index: 1,
                offset: f64::INFINITY,
            },
        ),
        (
            "a new corner off the wire",
            C::InsertWireCorner {
                wire,
                at: [1.0, 1.0],
            },
        ),
        (
            "a new corner on a corner",
            C::InsertWireCorner {
                wire,
                at: [20.0, 0.0],
            },
        ),
        (
            "no such corner to remove",
            C::RemoveWireCorner { wire, index: 2 },
        ),
        (
            "no such segment to remove",
            C::RemoveWireSegment { wire, index: 3 },
        ),
        (
            "an end on the wire itself",
            C::MoveWireEnd {
                wire,
                end: WireSide::To,
                to: WireTarget::OnWire(WirePoint {
                    wire,
                    at: [20.0, 10.0],
                }),
            },
        ),
        (
            "too many corners",
            C::SetWirePath {
                wire,
                corners: corners(&vec![[1.0, 1.0]; 257]),
                from: None,
                to: None,
            },
        ),
    ];
    let wires = engine.project().wires.clone();
    let revision = engine.revision();
    for (case, command) in cases {
        assert!(run(&mut engine, command).is_err(), "{case}");
        assert_eq!(engine.project().wires, wires, "{case}");
        assert_eq!(engine.revision(), revision, "{case}");
    }
    Ok(())
}

/// A segment on the wire's own points: the path splits, with free ends at the
/// gap, one undo puts it back, and the two pins are no longer one net.
#[test]
fn removing_a_segment_splits_the_path_with_free_ends() -> Result<()> {
    let mut engine = Engine::default();
    let (u1, u2, wire) = wired(&mut engine, &DOGLEG)?;
    let wires = engine.project().wires.clone();
    assert!(engine.project().net_of(ep(u1, 2)).is_some());
    run(&mut engine, C::RemoveWireSegment { wire, index: 1 })?;
    let project = engine.project();
    assert_eq!(project.wires.len(), 2);
    let [a, b] = &project.wires[..] else {
        return Err(Error::Invalid("two wires".into()));
    };
    assert_eq!(a.id, wire, "the part before keeps the ID");
    assert_eq!(a.from, WireEnd::Terminal(ep(u1, 2)));
    assert_eq!(a.to.free_point(), Some([20.0, 0.0]));
    assert_eq!(b.from.free_point(), Some([20.0, 20.0]));
    assert_eq!(b.to, WireEnd::Terminal(ep(u2, 1)));
    let same = |p: &crate::Project| {
        p.net_of(ep(u1, 2)).map(|n| n.id) == p.net_of(ep(u2, 1)).map(|n| n.id)
            && p.net_of(ep(u1, 2)).is_some()
    };
    assert!(!same(project), "the path no longer joins them");
    engine.undo()?;
    assert_eq!(engine.project().wires, wires);
    assert!(same(engine.project()));

    // The first segment: nothing is left on the pin; the rest keeps the ID.
    run(&mut engine, C::RemoveWireSegment { wire, index: 0 })?;
    let [only] = &engine.project().wires[..] else {
        return Err(Error::Invalid("one wire".into()));
    };
    assert_eq!(only.id, wire);
    assert_eq!(only.from.free_point(), Some([20.0, 0.0]));
    engine.undo()?;

    // A single segment: the wire goes.
    run(
        &mut engine,
        C::SetWirePath {
            wire,
            corners: Vec::new(),
            from: None,
            to: None,
        },
    )?;
    run(&mut engine, C::RemoveWireSegment { wire, index: 0 })?;
    assert!(engine.project().wires.is_empty());
    Ok(())
}

/// The doc: a connection point on a path connects to it, and a corner counts
/// as on the path. A corner dragged onto a pin makes a T there; a corner
/// lying on another wire joins nothing.
#[test]
fn a_corner_landing_on_a_connection_point_joins_it_and_on_a_wire_does_not() -> Result<()> {
    let mut engine = Engine::default();
    let (u1, u2, wire) = wired(&mut engine, &DOGLEG)?;
    let f = two_pin(&mut engine)?;
    let (u3, _) = place(&mut engine, &f, "U3", 1, [60.0, 45.0])?;
    let at =
        schematic::pin_position(engine.project(), ep(u3, 1)).ok_or(Error::Invalid("pin".into()))?;
    run(
        &mut engine,
        C::MoveWireCorner {
            wire,
            index: 1,
            to: at,
            free: true,
        },
    )?;
    let project = engine.project();
    assert_eq!(
        project.wires.len(),
        2,
        "split at the pin: {:?}",
        project.wires
    );
    assert_eq!(project.wires_at(ep(u3, 1)).count(), 2);
    let net = project.net_of(ep(u1, 2)).map(|n| n.id);
    assert!(net.is_some());
    assert_eq!(project.net_of(ep(u3, 1)).map(|n| n.id), net, "U3 joined");
    assert_eq!(project.net_of(ep(u2, 1)).map(|n| n.id), net);
    engine.undo()?;
    assert_eq!(engine.project().wires.len(), 1);

    // A second wire crossed by the corner of the first: no T, no junction.
    let (u4, _) = place(&mut engine, &f, "U4", 1, [20.0, -30.0])?;
    let (u5, _) = place(&mut engine, &f, "U5", 1, [50.0, -30.0])?;
    run(
        &mut engine,
        C::DrawWire {
            sheet: None,
            from: ep(u4, 2).into(),
            corners: Vec::new(),
            to: ep(u5, 1).into(),
        },
    )?;
    let junctions = engine.project().junctions.len();
    let wires = engine.project().wires.len();
    let nets = engine.project().nets.clone();
    run(
        &mut engine,
        C::MoveWireCorner {
            wire,
            index: 0,
            to: [30.0, -30.0],
            free: true,
        },
    )?;
    assert_eq!(engine.project().junctions.len(), junctions);
    assert_eq!(engine.project().wires.len(), wires);
    assert_eq!(engine.project().nets, nets);
    Ok(())
}

/// A free end moves with `wire.move_end`, attaches to a pin or a wire, and
/// joins another free end into one path; a segment's run over a pin is a T.
#[test]
fn moving_a_free_end_attaches_and_joins() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &f, "U2", 1, [40.0, 20.0])?;
    run(
        &mut engine,
        C::DrawWire {
            sheet: None,
            from: ep(u1, 2).into(),
            corners: corners(&[[15.0, 0.0]]),
            to: free([15.0, 10.0])?,
        },
    )?;
    let wire = engine.project().wires[0].id;
    let end = WireSide::To;

    // Moved in empty space: the last segment keeps its direction.
    run(
        &mut engine,
        C::MoveWireEnd {
            wire,
            end,
            to: free([20.0, 12.5])?,
        },
    )?;
    assert_eq!(
        path(&engine, wire)?,
        wire_edit::move_end(&[[5.0, 0.0], [15.0, 0.0], [15.0, 10.0]], end, [20.0, 12.5])
    );
    engine.undo()?;

    // Onto a pin: it attaches, and the two pins are one net.
    run(
        &mut engine,
        C::MoveWireEnd {
            wire,
            end,
            to: ep(u2, 1).into(),
        },
    )?;
    let project = engine.project();
    assert_eq!(project.wires[0].to, WireEnd::Terminal(ep(u2, 1)));
    assert_eq!(
        project.net_of(ep(u1, 2)).map(|n| n.id),
        project.net_of(ep(u2, 1)).map(|n| n.id)
    );
    assert!(project.net_of(ep(u1, 2)).is_some());
    engine.undo()?;

    // Onto another wire's middle: a T with a junction.
    run(
        &mut engine,
        C::DrawWire {
            sheet: None,
            from: ep(u2, 1).into(),
            corners: Vec::new(),
            to: free([35.0, -10.0])?,
        },
    )?;
    let other = engine.project().wires[1].id;
    run(
        &mut engine,
        C::MoveWireEnd {
            wire,
            end,
            to: WireTarget::OnWire(WirePoint {
                wire: other,
                at: [35.0, 5.0],
            }),
        },
    )?;
    assert_eq!(engine.project().junctions.len(), 1);
    assert_eq!(engine.project().wires.len(), 3);
    engine.undo()?;

    // Onto that wire's free end: the two join into one path.
    run(
        &mut engine,
        C::MoveWireEnd {
            wire,
            end,
            to: free([35.0, -10.0])?,
        },
    )?;
    let project = engine.project();
    assert_eq!(project.wires.len(), 1, "{:?}", project.wires);
    assert!(project.wires[0].from.free_point().is_none());
    assert!(project.wires[0].to.free_point().is_none());
    engine.undo()?;
    assert_eq!(engine.project().wires.len(), 2);
    Ok(())
}

/// A wire.draw or an edit naming a port placed on a pin ends on the pin: the
/// port stays where it was, on its pin, and the wires join its net.
#[test]
fn naming_a_port_on_a_pin_keeps_it_on_its_pin() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin(&mut engine)?;
    let (u1, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    let (u2, _) = place(&mut engine, &f, "U2", 1, [40.0, 20.0])?;
    run(
        &mut engine,
        C::PlaceLabel {
            sheet: None,
            name: "GND".into(),
            kind: LabelKind::Power {
                style: crate::model::PowerStyle::Ground,
            },
            position: [5.0, 0.0],
            rotation: Default::default(),
            attach: Some(ep(u1, 2).into()),
        },
    )?;
    let label = engine
        .project()
        .labels
        .last()
        .map(|l| l.id)
        .ok_or(Error::Invalid("label".into()))?;
    let attached = |engine: &Engine| {
        engine
            .project()
            .label(label)
            .and_then(|l| schematic::labels::attached_pin(engine.project(), l))
    };
    assert_eq!(attached(&engine), Some(ep(u1, 2)));
    let to = WireTarget::Label(crate::model::LabelRef { label });
    run(
        &mut engine,
        C::DrawWire {
            sheet: None,
            from: ep(u2, 1).into(),
            corners: corners(&[[35.0, 0.0]]),
            to,
        },
    )?;
    assert_eq!(
        attached(&engine),
        Some(ep(u1, 2)),
        "the port stays on its pin"
    );
    let project = engine.project();
    let drawn = project
        .wires
        .iter()
        .find(|w| w.from == WireEnd::Terminal(ep(u2, 1)))
        .ok_or(Error::Invalid("drawn".into()))?;
    assert_eq!(drawn.to, WireEnd::Terminal(ep(u1, 2)), "it ends on the pin");
    assert_eq!(
        project.net_of(ep(u1, 2)).map(|n| n.id),
        project.net_of(ep(u2, 1)).map(|n| n.id)
    );
    engine.undo()?;

    // The same through `wire.move_end`.
    run(
        &mut engine,
        C::DrawWire {
            sheet: None,
            from: ep(u2, 1).into(),
            corners: corners(&[[35.0, 0.0]]),
            to: free([30.0, 0.0])?,
        },
    )?;
    let wire = engine
        .project()
        .wires
        .iter()
        .find(|w| w.from == WireEnd::Terminal(ep(u2, 1)))
        .map(|w| w.id)
        .ok_or(Error::Invalid("drawn".into()))?;
    run(
        &mut engine,
        C::MoveWireEnd {
            wire,
            end: WireSide::To,
            to,
        },
    )?;
    assert_eq!(attached(&engine), Some(ep(u1, 2)));
    Ok(())
}

/// The edit commands' names and shapes, as the CLI and MCP send them.
#[test]
fn the_edit_commands_read_their_documented_json() -> Result<()> {
    let cases = [
        r#"{"command":"wire.set_path","params":{"wire":1,"corners":[{"at":[1.0,2.0]}],"to":{"free":[3.0,4.0]}}}"#,
        r#"{"command":"wire.move_corner","params":{"wire":1,"index":0,"to":[1.0,2.0],"free":true}}"#,
        r#"{"command":"wire.move_segment","params":{"wire":1,"index":1,"offset":-5.0}}"#,
        r#"{"command":"wire.insert_corner","params":{"wire":1,"at":[1.0,2.0]}}"#,
        r#"{"command":"wire.remove_corner","params":{"wire":1,"index":0}}"#,
        r#"{"command":"wire.remove_segment","params":{"wire":1,"index":0}}"#,
        r#"{"command":"wire.move_end","params":{"wire":1,"end":"to","to":{"free":[1.0,2.0]}}}"#,
    ];
    for json in cases {
        let command: C =
            serde_json::from_str(json).map_err(|e| Error::Invalid(format!("{json}: {e}")))?;
        let back = serde_json::to_string(&command).map_err(|e| Error::Invalid(e.to_string()))?;
        assert_eq!(back, json);
    }
    Ok(())
}
