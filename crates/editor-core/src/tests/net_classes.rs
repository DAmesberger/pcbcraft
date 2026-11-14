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

/// A class clearance applies between copper of different parts and
/// tracks, but not between two pads of one footprint, whose spacing the
/// footprint fixes: on the showcase a Power class of 0.3 mm over GND and
/// VCC flags none of the QFN's pads against its exposed pad, while a
/// clearance of 1 mm still flags Power copper against other parts.
#[test]
fn class_clearance_spares_pads_of_one_footprint() -> Result<()> {
    use crate::diagnostics::ObjectRef;
    let mut engine = Engine::showcase()?;
    let nets: Vec<crate::Id> = engine
        .project()
        .nets
        .iter()
        .filter(|n| n.name == "GND" || n.name == "VCC")
        .map(|n| n.id)
        .collect();
    assert_eq!(nets.len(), 2);
    let class = add_class(
        &mut engine,
        NetClassSpec {
            name: "Power".into(),
            clearance: Some(0.3),
            ..Default::default()
        },
    )?;
    assign(&mut engine, nets, Some(class))?;
    let same_part = |objects: &[ObjectRef]| match objects {
        [
            ObjectRef::Pad { instance: a, .. },
            ObjectRef::Pad { instance: b, .. },
        ] => a == b,
        _ => false,
    };
    let clearance = |engine: &Engine| -> Vec<crate::diagnostics::Finding> {
        layout::checks(engine.project(), engine.check_settings())
            .into_iter()
            .filter(|f| f.code == "board.clearance")
            .collect()
    };
    let findings = clearance(&engine);
    assert!(
        findings.iter().all(|f| !same_part(&f.objects)),
        "{findings:#?}"
    );
    let mut wide = list(&engine)?
        .classes
        .into_iter()
        .find(|c| c.class.id == class)
        .ok_or_else(|| Error::Invalid("class".into()))?
        .class;
    wide.clearance = Some(1.0);
    apply(
        &mut engine,
        Operation::Rules(RulesCommand::EditNetClass { class: wide }),
    )?;
    let findings = clearance(&engine);
    assert!(
        findings
            .iter()
            .any(|f| f.message.contains("net class Power") && !same_part(&f.objects)),
        "{findings:#?}"
    );
    assert!(findings.iter().all(|f| !same_part(&f.objects)));
    Ok(())
}

fn side(
    object: crate::rules::CopperObject,
    class: Option<NetClassId>,
) -> crate::rules::ClearanceSide {
    crate::rules::ClearanceSide { object, class }
}

/// Gap 13 of the E2E log: the two clearances resolve separately. Rule
/// values are the default profile's 0.2 mm.
#[test]
fn fill_clearance_applies_to_fills_only() -> Result<()> {
    use crate::rules::CopperObject::{Fill, Pad, Track};
    let mut engine = Engine::demo()?;
    let usb = add_class(
        &mut engine,
        NetClassSpec {
            name: "USB".into(),
            fill_clearance: Some(0.35),
            ..Default::default()
        },
    )?;
    let wide = add_class(
        &mut engine,
        NetClassSpec {
            name: "Wide".into(),
            clearance: Some(0.5),
            ..Default::default()
        },
    )?;
    let near = add_class(
        &mut engine,
        NetClassSpec {
            name: "Near".into(),
            clearance: Some(0.5),
            fill_clearance: Some(0.25),
            ..Default::default()
        },
    )?;
    let low = add_class(
        &mut engine,
        NetClassSpec {
            name: "Low".into(),
            fill_clearance: Some(0.05),
            ..Default::default()
        },
    )?;
    let rules = &engine.project().rules;
    let mm = |a, b| rules.clearance(a, b).value;
    // Fill clearance set: fills keep it, tracks and pads do not.
    assert_eq!(mm(side(Fill, None), side(Track, Some(usb))), 0.35);
    assert_eq!(mm(side(Fill, None), side(Pad, Some(usb))), 0.35);
    assert_eq!(mm(side(Track, Some(usb)), side(Pad, None)), 0.2);
    assert_eq!(mm(side(Track, Some(usb)), side(Track, None)), 0.2);
    // A fill of the class keeps it too.
    assert_eq!(mm(side(Fill, Some(usb)), side(Track, None)), 0.35);
    // Unset: the class clearance applies to fills, as before.
    assert_eq!(mm(side(Fill, None), side(Track, Some(wide))), 0.5);
    assert_eq!(mm(side(Track, Some(wide)), side(Track, None)), 0.5);
    // Set lower than the clearance: it stands in for it against fills.
    assert_eq!(mm(side(Fill, None), side(Track, Some(near))), 0.25);
    assert_eq!(mm(side(Track, Some(near)), side(Track, None)), 0.5);
    // Never below the rule.
    assert_eq!(mm(side(Fill, None), side(Track, Some(low))), 0.2);
    // The source names the class that set it.
    assert_eq!(
        rules
            .clearance(side(Fill, None), side(Track, Some(usb)))
            .source,
        crate::rules::RuleSource::NetClass {
            class: "USB".into()
        }
    );
    // The report carries both.
    let report = list(&engine)?;
    let row = report
        .classes
        .iter()
        .find(|r| r.class.id == usb)
        .ok_or_else(|| Error::Invalid("no row".into()))?;
    assert_eq!(row.clearance.value, 0.2);
    assert_eq!(row.fill_clearance.value, 0.35);
    assert_eq!(row.class.fill_clearance, Some(0.35));
    Ok(())
}

/// The E2E board's trouble: a class keeping fills 0.35 mm off must still
/// route between pads 0.2 mm apart. The same value as a plain clearance is
/// refused.
#[test]
fn a_fill_clearance_leaves_a_tight_route_routable() -> Result<()> {
    let route_near_pad = |engine: &mut Engine, net| {
        // Past R2's pad, whose edge is at x 15.2 − 0.5: the track's edge is
        // 14.7 − 14.3 − 0.15 = 0.25 mm from it.
        apply(
            engine,
            Operation::Layout(LayoutCommand::Route {
                request: RouteRequest {
                    net,
                    start: [14.3, 6.0],
                    end: [14.3, 10.0],
                    width: 0.3,
                    layer: Layer::Top.into(),
                },
            }),
        )
    };
    for (spec, routes) in [
        (
            NetClassSpec {
                name: "Fill only".into(),
                fill_clearance: Some(0.35),
                ..Default::default()
            },
            true,
        ),
        (
            NetClassSpec {
                name: "All".into(),
                clearance: Some(0.35),
                ..Default::default()
            },
            false,
        ),
    ] {
        let mut engine = Engine::demo()?;
        let pad_net = layout::placed_pads(engine.project())
            .into_iter()
            .find(|p| (p.pad_transform().apply([0.0, 0.0])[0] - 15.2).abs() < 1e-6)
            .and_then(|p| p.net);
        let net = engine
            .project()
            .nets
            .iter()
            .map(|n| n.id)
            .find(|n| Some(*n) != pad_net)
            .ok_or_else(|| Error::Invalid("no other net".into()))?;
        let class = add_class(&mut engine, spec)?;
        assign(&mut engine, vec![net], Some(class))?;
        let revision = engine.revision();
        assert_eq!(route_near_pad(&mut engine, net).is_ok(), routes);
        assert_eq!(engine.revision(), revision + u64::from(routes));
    }
    Ok(())
}

#[test]
fn a_bad_fill_clearance_is_refused_and_an_unset_one_stays_off_the_wire() -> Result<()> {
    let mut engine = Engine::demo()?;
    let revision = engine.revision();
    for bad in [-0.1, f64::NAN, f64::INFINITY, 1.0e9] {
        let refused = add_class(
            &mut engine,
            NetClassSpec {
                name: "Bad".into(),
                fill_clearance: Some(bad),
                ..Default::default()
            },
        );
        assert!(refused.is_err(), "{bad}");
        assert_eq!(engine.revision(), revision);
    }
    // Set, edit, undo through the public commands.
    let id = add_class(
        &mut engine,
        NetClassSpec {
            name: "USB".into(),
            ..Default::default()
        },
    )?;
    let mut class = engine
        .project()
        .rules
        .class(id)
        .cloned()
        .ok_or_else(|| Error::Invalid("no class".into()))?;
    // Existing files and fill keys are unchanged while it is unset.
    let unset = serde_json::to_value(&class).map_err(|e| Error::Invalid(e.to_string()))?;
    assert!(unset.get("fill_clearance").is_none());
    class.fill_clearance = Some(0.4);
    apply(
        &mut engine,
        Operation::Rules(RulesCommand::EditNetClass {
            class: class.clone(),
        }),
    )?;
    assert_eq!(
        engine
            .project()
            .rules
            .class(id)
            .and_then(|c| c.fill_clearance),
        Some(0.4)
    );
    engine.undo()?;
    assert_eq!(
        engine
            .project()
            .rules
            .class(id)
            .and_then(|c| c.fill_clearance),
        None
    );
    Ok(())
}
