//! Net classes through the public engine: routing takes and enforces the
//! class width, assignments follow schematic merges and splits, and the
//! `net_class.*` queries describe membership without applying anything.
use super::support::*;
use crate::layout::{self, LayoutCommand, RouteRequest};
use crate::rules::netclass::{ClassBasis, NetClassReport};
use crate::rules::{NetClassId, NetClassPattern, NetClassSpec, RulesCommand};
use crate::schematic::SchematicCommand;
use crate::{Engine, Error, Layer, Operation, Outcome, Query, QueryResult, Result};

fn add_class(engine: &mut Engine, class: NetClassSpec) -> Result<NetClassId> {
    match apply(
        engine,
        Operation::Rules(RulesCommand::AddNetClass { class }),
    )? {
        Outcome::NetClassAdded { class } => Ok(class),
        _ => Err(Error::Invalid("no class added".into())),
    }
}

fn assign(engine: &mut Engine, nets: Vec<crate::Id>, class: Option<NetClassId>) -> Result<()> {
    apply(
        engine,
        Operation::Rules(RulesCommand::AssignNets { nets, class }),
    )
    .map(|_| ())
}

fn power() -> NetClassSpec {
    NetClassSpec {
        name: "Power".into(),
        track_width: Some(0.5),
        track_width_min: Some(0.4),
        via_drill: Some(0.4),
        via_diameter: Some(0.8),
        ..Default::default()
    }
}

fn route(net: crate::Id, width: f64) -> Operation {
    Operation::Layout(LayoutCommand::Route {
        request: RouteRequest {
            net,
            start: [9.2, 8.0],
            end: [14.8, 8.0],
            width,
            layer: Layer::Top.into(),
        },
    })
}

fn list(engine: &Engine) -> Result<NetClassReport> {
    match engine.query(Query::NetClasses {})? {
        QueryResult::NetClasses(report) => Ok(*report),
        _ => Err(Error::Invalid("not a net class report".into())),
    }
}

#[test]
fn routing_takes_and_enforces_the_class_width() -> Result<()> {
    let mut engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let shared = net_of(&engine, ep(r1, 2))?;
    // Unclassed nets keep today's defaults.
    assert_eq!(layout::route_width(engine.project(), shared).value, 0.3);
    let via = layout::class_via(engine.project(), shared, [1.0, 1.0]);
    assert_eq!((via.drill, via.diameter), (0.3, 0.6));
    let class = add_class(&mut engine, power())?;
    assign(&mut engine, vec![shared], Some(class))?;
    let width = layout::route_width(engine.project(), shared);
    assert_eq!(width.value, 0.5);
    assert_eq!(width.source.describe(), "net class Power");
    let via = layout::class_via(engine.project(), shared, [1.0, 1.0]);
    assert_eq!((via.drill, via.diameter, via.tented), (0.4, 0.8, true));

    let kept = Snapshot::of(&engine);
    let refused = engine.apply(engine.revision(), route(shared, 0.3));
    let Err(error) = refused else {
        return Err(Error::Invalid("a narrow route was accepted".into()));
    };
    let text = error.to_string();
    assert!(
        text.contains("0.4 mm (net class Power: min track width)"),
        "{text}"
    );
    assert!(crate::issues::style::check(&text).is_empty(), "{text}");
    kept.assert_kept(&engine, "narrow route");

    apply(&mut engine, route(shared, 0.5))?;
    let track = engine.project().tracks[0].id;
    let narrow = engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetTrack {
            id: track,
            width: Some(0.35),
            layer: None,
        }),
    );
    assert!(narrow.is_err());
    // Removing the class lifts its minimum again.
    assign(&mut engine, vec![shared], None)?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetTrack {
            id: track,
            width: Some(0.35),
            layer: None,
        }),
    )?;
    Ok(())
}

#[test]
fn assignments_follow_a_schematic_merge_and_undo() -> Result<()> {
    let mut engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let (left, shared) = (net_of(&engine, ep(r1, 1))?, net_of(&engine, ep(r1, 2))?);
    assert_ne!(left, shared);
    let class = add_class(&mut engine, power())?;
    assign(&mut engine, vec![left], Some(class))?;
    let before = engine.project().rules.clone();
    connect(&mut engine, ep(r1, 1), ep(r1, 2))?;
    let merged = net_of(&engine, ep(r1, 1))?;
    assert_eq!(merged, net_of(&engine, ep(r1, 2))?);
    let project = engine.project();
    assert_eq!(
        project.rules.class_of(merged, project.net_name(merged)),
        Some(class)
    );
    assert_eq!(project.rules.assignments.len(), 1);
    // Splitting again: the wire goes, both pieces keep the class.
    let wire = wire_between(&engine, ep(r1, 1), ep(r1, 2))?;
    apply(
        &mut engine,
        Operation::Schematic(SchematicCommand::Disconnect { wire }),
    )?;
    let project = engine.project();
    for terminal in [ep(r1, 1), ep(r1, 2)] {
        let net = net_of(&engine, terminal)?;
        assert_eq!(
            project.rules.class_of(net, project.net_name(net)),
            Some(class)
        );
    }
    engine.undo()?;
    engine.undo()?;
    assert_eq!(engine.project().rules, before);
    Ok(())
}

#[test]
fn queries_report_membership_and_preview_patterns() -> Result<()> {
    let mut engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let shared = net_of(&engine, ep(r1, 2))?;
    let class = add_class(&mut engine, power())?;
    let report = list(&engine)?;
    assert_eq!(report.classes.len(), 1);
    assert_eq!(report.classes[0].members, 0);
    assert_eq!(report.classes[0].track_width.value, 0.5);
    assert_eq!(report.nets.len(), engine.project().nets.len());

    let patterns = vec![NetClassPattern {
        pattern: "N*".into(),
        class,
    }];
    let kept = Snapshot::of(&engine);
    let QueryResult::NetClassChanges(changes) = engine.query(Query::PreviewNetClassPatterns {
        patterns: patterns.clone(),
    })?
    else {
        return Err(Error::Invalid("not a pattern preview".into()));
    };
    // Every demo net has a default "N…" name.
    assert_eq!(changes.len(), engine.project().nets.len());
    kept.assert_kept(&engine, "pattern preview");

    apply(
        &mut engine,
        Operation::Rules(RulesCommand::SetPatterns { patterns }),
    )?;
    assign(&mut engine, vec![shared], Some(class))?;
    let report = list(&engine)?;
    assert_eq!(report.classes[0].members, engine.project().nets.len());
    let basis = |net| {
        report
            .nets
            .iter()
            .find(|m| m.net == net)
            .and_then(|m| m.basis.clone())
    };
    assert_eq!(basis(shared), Some(ClassBasis::Explicit));
    let other = net_of(&engine, ep(r1, 1))?;
    assert!(matches!(basis(other), Some(ClassBasis::Pattern { .. })));
    Ok(())
}

#[test]
fn refused_class_commands_leave_the_document_unchanged() -> Result<()> {
    let mut engine = Engine::demo()?;
    let class = add_class(&mut engine, power())?;
    let kept = Snapshot::of(&engine);
    for operation in [
        RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "power".into(),
                ..Default::default()
            },
        },
        RulesCommand::AssignNets {
            nets: vec![99_999],
            class: Some(class),
        },
        RulesCommand::AssignNets {
            nets: vec![],
            class: Some(class),
        },
        RulesCommand::SetClassClearance {
            a: class,
            b: NetClassId(77),
            mm: Some(0.3),
        },
        RulesCommand::DeleteNetClass {
            class: NetClassId(77),
        },
    ] {
        let result = engine.apply(engine.revision(), Operation::Rules(operation));
        let Err(error) = result else {
            return Err(Error::Invalid("a bad class command was accepted".into()));
        };
        assert!(
            crate::issues::style::check(&error.to_string()).is_empty(),
            "{error}"
        );
        kept.assert_kept(&engine, "refused class command");
    }
    Ok(())
}

#[test]
fn net_class_queries_are_served() {
    let text = crate::schemas().to_string();
    for name in [
        "net_class.list",
        "net_class.preview_patterns",
        "track_width_min",
    ] {
        assert!(text.contains(name), "{name}");
    }
}
