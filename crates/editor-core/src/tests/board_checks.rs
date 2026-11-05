//! The board checks read the project's design rules: a profile switch, a
//! project override or a net class flips a verdict, and the finding names
//! the limit, its value and its source. Every expected gap is derived by
//! hand from the fixture coordinates.
use crate::diagnostics::{Error, Finding, Result};
use crate::geometry::Point;
use crate::ids::Id;
use crate::layout::{LayoutCommand, RouteRequest, ViaRequest};
use crate::library::{Outline, Segment};
use crate::model::board::{BoardFeatureId, BoardSketch, CutoutKind, SolvedBoard, SolvedCutout};
use crate::model::{BoardShape, CopperLayer, Project, Track, Via, ViaSpan};
use crate::rules::{
    CopperObject, NetClassSpec, ProjectRules, RuleKey, RuleProfile, RuleValue, RulesCommand,
    builtin,
};
use crate::{Engine, Operation, Outcome, Query, QueryResult};

fn track(id: Id, net: Id, start: Point, end: Point, width: f64) -> Track {
    Track {
        id,
        net,
        start,
        end,
        width,
        layer: CopperLayer::Top,
        locked: false,
    }
}

fn via(id: Id, net: Id, position: Point, drill: f64, diameter: f64) -> Via {
    Via {
        id,
        net,
        position,
        drill,
        diameter,
        span: ViaSpan::Through,
        tented: true,
        locked: false,
    }
}

/// The demo board (60 × 40 mm) without parts' copper in the way: its nets
/// and an id range above anything it allocated.
fn board() -> Result<(Project, Id, Id)> {
    let engine = Engine::demo()?;
    let project = engine.project().clone();
    let a = project.nets.first().map(|n| n.id);
    let b = project.nets.get(1).map(|n| n.id);
    let (Some(a), Some(b)) = (a, b) else {
        return Err(Error::NotFound("two demo nets".into()));
    };
    Ok((project, a, b))
}

fn with(project: &mut Project, command: RulesCommand) -> Result<()> {
    crate::rules::apply(&mut project.rules, command).map(|_| ())
}

fn override_mm(project: &mut Project, key: RuleKey, mm: f64) -> Result<()> {
    with(
        project,
        RulesCommand::SetOverride {
            key,
            value: RuleValue::Length(mm),
        },
    )
}

fn found(project: &Project, code: &str) -> Vec<Finding> {
    crate::layout::checks(project)
        .into_iter()
        .filter(|f| f.code == code)
        .collect()
}

/// A profile named `name` from the default with `edit` applied.
fn profile(name: &str, edit: impl FnOnce(&mut crate::rules::DesignRules)) -> RuleProfile {
    let mut p = builtin::default_profile();
    p.id = name.to_lowercase().replace(' ', "-");
    p.name = name.into();
    // Changed values make a user profile: `rules.set_profile` refuses a
    // built-in profile whose values differ from the shipped ones.
    p.provenance.origin = crate::rules::ProfileOrigin::User;
    edit(&mut p.rules);
    p
}

/// Two parallel 0.3 mm tracks on different nets, centre lines 0.55 mm
/// apart: a 0.25 mm copper gap.
fn parallel(project: &mut Project, a: Id, b: Id) {
    let base = project.next_id + 10;
    project.next_id = base + 10;
    project
        .tracks
        .push(track(base, a, [20.0, 30.0], [30.0, 30.0], 0.3));
    project
        .tracks
        .push(track(base + 1, b, [20.0, 30.55], [30.0, 30.55], 0.3));
}

#[test]
fn copper_clearance_follows_overrides_profiles_and_net_classes() -> Result<()> {
    let (mut project, a, b) = board()?;
    parallel(&mut project, a, b);
    // 0.25 mm passes the default 0.2 mm.
    assert!(found(&project, "board.clearance").is_empty());

    let key = RuleKey::Clearance {
        a: CopperObject::Track,
        b: CopperObject::Track,
    };
    let mut overridden = project.clone();
    override_mm(&mut overridden, key, 0.3)?;
    let hits = found(&overridden, "board.clearance");
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0]
            .message
            .starts_with("Clearance < 0.3 mm: a track on ")
            && hits[0]
                .message
                .ends_with("(project override: track to track clearance)"),
        "{}",
        hits[0].message
    );

    let mut fine = project.clone();
    let p = profile("Fine Test", |r| {
        r.copper
            .clearance
            .set(CopperObject::Track, CopperObject::Track, 0.26)
    });
    with(
        &mut fine,
        RulesCommand::SetProfile {
            profile: Box::new(p),
            keep_overrides: false,
        },
    )?;
    let hits = found(&fine, "board.clearance");
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0]
            .message
            .ends_with("(Fine Test: track to track clearance)")
    );

    // A net class on one side sets its own clearance.
    let mut classed = project.clone();
    let Some(class) = crate::rules::apply(
        &mut classed.rules,
        RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "Power".into(),
                clearance: Some(0.4),
                ..NetClassSpec::default()
            },
        },
    )?
    else {
        return Err(Error::Invalid("no class".into()));
    };
    with(
        &mut classed,
        RulesCommand::AssignNets {
            nets: vec![b],
            class: Some(class),
        },
    )?;
    let hits = found(&classed, "board.clearance");
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].message.starts_with("Clearance < 0.4 mm")
            && hits[0].message.ends_with("(net class Power: clearance)"),
        "{}",
        hits[0].message
    );
    Ok(())
}

#[test]
fn routing_refuses_what_the_net_class_forbids() -> Result<()> {
    let mut engine = Engine::demo()?;
    let nets: Vec<Id> = engine.project().nets.iter().map(|n| n.id).collect();
    let (Some(&a), Some(&b)) = (nets.first(), nets.get(1)) else {
        return Err(Error::NotFound("two demo nets".into()));
    };
    let route = |net, y| RouteRequest {
        net,
        start: [20.0, y],
        end: [30.0, y],
        width: 0.3,
        layer: CopperLayer::Top,
    };
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: route(a, 30.0),
        }),
    )?;
    let Outcome::NetClassAdded { class } = engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "Power".into(),
                clearance: Some(0.4),
                ..NetClassSpec::default()
            },
        }),
    )?
    else {
        return Err(Error::Invalid("no class outcome".into()));
    };
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AssignNets {
            nets: vec![b],
            class: Some(class),
        }),
    )?;
    let before = engine.project().clone();
    // A 0.25 mm gap: enough for the default 0.2 mm, not for Power's 0.4 mm.
    let refused = engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: route(b, 30.55),
        }),
    );
    let Err(Error::Conflict(text)) = refused else {
        return Err(Error::Invalid(format!("a refusal expected: {refused:?}")));
    };
    assert!(text.ends_with(": needs 0.4 mm (net class Power)"), "{text}");
    assert_eq!(crate::issues::style::check(&text), vec![], "{text}");
    assert_eq!(engine.project(), &before);
    // 0.45 mm clears it.
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: route(b, 30.75),
        }),
    )?;
    // A via of the class keeps the class clearance too.
    // Its pad 0.3 mm below the first track (copper edge at y 29.85).
    let close = ViaRequest::through(b, [25.0, 29.25]);
    let refused = engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::AddVia { via: close }),
    );
    assert!(
        matches!(&refused, Err(Error::Conflict(t)) if t.ends_with("(net class Power)")),
        "{refused:?}"
    );
    Ok(())
}

#[test]
fn via_checks_take_their_limits_from_the_rules() -> Result<()> {
    let (mut project, a, b) = board()?;
    let base = project.next_id + 10;
    project.next_id = base + 10;
    // Ring 0.15 mm; drills 0.3 mm with centres 1.0 mm apart: 0.7 mm
    // between drills, 0.4 mm between pads.
    project.vias.push(via(base, a, [20.0, 20.0], 0.3, 0.6));
    project.vias.push(via(base + 1, a, [21.0, 20.0], 0.3, 0.6));
    // A track of the other net whose copper edge is 0.6 mm above a third
    // via's drill (centre line at y 20.9, half-width 0.15 mm; drill top
    // at 20.15): 0.45 mm from that via's pad.
    project
        .tracks
        .push(track(base + 2, b, [24.0, 20.9], [26.0, 20.9], 0.3));
    project.vias.push(via(base + 3, a, [25.0, 20.0], 0.3, 0.6));
    let codes = |p: &Project| -> Vec<String> {
        crate::layout::checks(p)
            .into_iter()
            .map(|f| f.code)
            .filter(|c| c.starts_with("board.via_") && c != "board.via_dangling")
            .collect()
    };
    assert_eq!(codes(&project), Vec::<String>::new());

    let mut strict = project.clone();
    // The defaults must stay buildable under the stricter minimums.
    override_mm(&mut strict, RuleKey::ViaDiameterDefault, 0.8)?;
    override_mm(&mut strict, RuleKey::ViaDrillDefault, 0.35)?;
    override_mm(&mut strict, RuleKey::ViaAnnularRingMin, 0.2)?;
    override_mm(&mut strict, RuleKey::DrillMin, 0.35)?;
    override_mm(
        &mut strict,
        RuleKey::Clearance {
            a: CopperObject::Hole,
            b: CopperObject::Hole,
        },
        0.8,
    )?;
    override_mm(
        &mut strict,
        RuleKey::Clearance {
            a: CopperObject::Track,
            b: CopperObject::Hole,
        },
        0.65,
    )?;
    with(
        &mut strict,
        RulesCommand::SetOverride {
            key: RuleKey::AspectRatioMax,
            value: RuleValue::Ratio(Some(4.0)),
        },
    )?;
    let findings = crate::layout::checks(&strict);
    let message = |code: &str| {
        findings
            .iter()
            .find(|f| f.code == code)
            .map(|f| f.message.clone())
            .unwrap_or_default()
    };
    assert!(
        message("board.via_annular_ring")
            .ends_with("annular ring 0.150 mm is under the minimum; needs 0.2 mm (project override: min via annular ring)"),
        "{findings:#?}"
    );
    assert!(message("board.via_drill_size").ends_with(
        "drill 0.3 mm is under the minimum; needs 0.35 mm (project override: min drill)"
    ));
    assert!(
        message("board.via_hole_spacing").ends_with(
            "drills are 0.7 mm apart; needs 0.8 mm (project override: hole to hole clearance)"
        ),
        "{findings:#?}"
    );
    let drill = message("board.via_drill_clearance");
    assert!(
        drill.contains("drill is 0.6 mm from a track on ")
            && drill.ends_with("needs 0.65 mm (project override: track to hole clearance)"),
        "{findings:#?}"
    );
    // 1.6 mm default stackup over a 0.3 mm drill.
    let thickness = strict.stackup.thickness_mm();
    assert!(thickness / 0.3 > 4.0);
    assert!(
        message("board.via_aspect_ratio").contains(
            "is over the maximum; needs at most 4:1 (project override: max aspect ratio)"
        ),
        "{findings:#?}"
    );
    // Titles (the first clause) do not name the limit, so a profile
    // switch keeps the same issue rows.
    for f in findings.iter().filter(|f| f.code.starts_with("board.via_")) {
        let title = f.message.split("; ").next().unwrap_or_default();
        assert!(!title.contains("override"), "{title}");
    }
    Ok(())
}

#[test]
fn board_edge_clearance_flips_with_the_profile_and_previews() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = engine.project().nets.first().map(|n| n.id).unwrap_or(0);
    // Copper edge 0.35 mm above the bottom edge: clear of the default
    // profile's 0.3 mm.
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net,
                start: [20.0, 0.5],
                end: [30.0, 0.5],
                width: 0.3,
                layer: CopperLayer::Top,
            },
        }),
    )?;
    assert!(found(engine.project(), "board.edge_clearance").is_empty());
    let edgy = profile("Edge Test", |r| {
        for o in CopperObject::ALL {
            r.copper.clearance.set(o, CopperObject::BoardEdge, 0.5);
        }
    });
    let QueryResult::ProfileImpact(impact) = engine.query(Query::PreviewProfile {
        profile: Box::new(edgy.clone()),
        keep_overrides: false,
    })?
    else {
        return Err(Error::Invalid("no impact".into()));
    };
    assert!(
        impact
            .new_issues
            .iter()
            .any(|t| t.contains("is 0.35 mm from the board edge")),
        "{impact:#?}"
    );
    assert!(impact.resolved_issues.is_empty(), "{impact:#?}");
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::SetProfile {
            profile: Box::new(edgy),
            keep_overrides: false,
        }),
    )?;
    let hits = found(engine.project(), "board.edge_clearance");
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].message.ends_with(
            "is 0.35 mm from the board edge; needs 0.5 mm (Edge Test: track to board edge clearance)"
        ),
        "{}",
        hits[0].message
    );
    // The profile is one undo step.
    engine.undo()?;
    assert!(found(engine.project(), "board.edge_clearance").is_empty());
    Ok(())
}

/// A legend text over a cutout is named with the loop it runs into.
#[test]
fn a_legend_over_a_cutout_is_reported_as_one() -> Result<()> {
    use crate::fabrication::preflight;
    use crate::layout::{self, PlacedItem};
    let (mut project, _, _) = board()?;
    sketched(&mut project, false);
    let instance = project
        .placements
        .iter()
        .map(|p| p.instance)
        .find(|id| crate::instances::text::target(&project, *id, None).is_ok())
        .ok_or_else(|| Error::NotFound("a designator".into()))?;
    let graphic = crate::instances::text::target(&project, instance, None)?;
    let (at, name) = layout::placed_graphics(&project)
        .into_iter()
        .filter(|g| g.instance == instance && g.graphic == graphic)
        .find_map(|g| match g.item {
            PlacedItem::Text(t) => Some((t.at, t.text)),
            PlacedItem::Shape { .. } => None,
        })
        .ok_or_else(|| Error::NotFound("the placed designator".into()))?;
    // Its centre to the middle of the 4 × 4 mm cutout at (20, 20).
    for placement in &mut project.placements {
        if placement.instance == instance {
            placement.position[0] += 22.0 - at[0];
            placement.position[1] += 22.0 - at[1];
        }
    }
    let report = preflight::report(&project, &Default::default(), None)?;
    let titles: Vec<&str> = report
        .items
        .iter()
        .filter(|i| i.code == "fabrication.legend_outside")
        .map(|i| i.title.as_str())
        .collect();
    let expected = format!("Designator {name} runs into a cutout");
    assert!(titles.contains(&expected.as_str()), "{titles:#?}");
    Ok(())
}

/// A part may overhang the board, so its legend lines may run past the edge.
#[test]
fn a_legend_line_past_the_board_edge_is_not_reported() -> Result<()> {
    use crate::fabrication::preflight;
    use crate::layout::{self, BoardDrawing, PlacedItem};
    let mut project = Engine::showcase()?.project().clone();
    let (instance, x) = layout::placed_graphics(&project)
        .into_iter()
        .filter(|g| matches!(g.layer, BoardDrawing::Silkscreen(_)))
        .find_map(|g| match &g.item {
            PlacedItem::Shape { shape, .. } => {
                shape.flatten(0.01).first().map(|p| (g.instance, p[0]))
            }
            PlacedItem::Text(_) => None,
        })
        .ok_or_else(|| Error::NotFound("a legend line".into()))?;
    // That line's first point half a millimetre past x = 0.
    for placement in &mut project.placements {
        if placement.instance == instance {
            placement.position[0] -= x + 0.5;
        }
    }
    let crosses = layout::placed_graphics(&project)
        .into_iter()
        .filter(|g| g.instance == instance)
        .any(|g| match &g.item {
            PlacedItem::Shape { shape, .. } => shape.flatten(0.01).iter().any(|p| p[0] < 0.0),
            PlacedItem::Text(_) => false,
        });
    assert!(crosses, "the line should run past the edge");
    let report = preflight::report(&project, &Default::default(), None)?;
    let lines: Vec<&str> = report
        .items
        .iter()
        .filter(|i| i.title.contains("Legend line"))
        .map(|i| i.title.as_str())
        .collect();
    assert!(lines.is_empty(), "{lines:#?}");
    Ok(())
}

/// A new project's profile keeps copper off the edge: copper an edge rule
/// of 0 used to accept is reported, by the rule that says so.
#[test]
fn the_default_profile_reports_copper_at_the_board_edge() -> Result<()> {
    let mut engine = Engine::demo()?;
    let net = engine.project().nets.first().map(|n| n.id).unwrap_or(0);
    // Copper edge 0.25 mm above the bottom edge (0.4 less half of 0.3).
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net,
                start: [20.0, 0.4],
                end: [30.0, 0.4],
                width: 0.3,
                layer: CopperLayer::Top,
            },
        }),
    )?;
    let hits = found(engine.project(), "board.edge_clearance");
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].message.ends_with(
            "is 0.25 mm from the board edge; needs 0.3 mm (Pcbcraft default: track to board edge clearance)"
        ),
        "{}",
        hits[0].message
    );
    Ok(())
}

fn square(min: Point, max: Point) -> Outline {
    Outline {
        start: min,
        segments: vec![
            Segment::Line {
                to: [max[0], min[1]],
            },
            Segment::Line { to: max },
            Segment::Line {
                to: [min[0], max[1]],
            },
            Segment::Line { to: min },
        ],
    }
}

/// A 60 × 40 mm sketched board with a notch cut from its top edge
/// (x 40–50, y 30–40, by the outline) and a 4 × 4 mm cutout at (20, 20).
fn sketched(project: &mut Project, slot: bool) {
    let outer = Outline {
        start: [0.0, 0.0],
        segments: [
            [60.0, 0.0],
            [60.0, 40.0],
            [50.0, 40.0],
            [50.0, 30.0],
            [40.0, 30.0],
            [40.0, 40.0],
            [0.0, 40.0],
            [0.0, 0.0],
        ]
        .into_iter()
        .map(|to| Segment::Line { to })
        .collect(),
    };
    let solved = SolvedBoard {
        outer,
        cutouts: vec![SolvedCutout {
            id: BoardFeatureId(1),
            kind: if slot {
                CutoutKind::Slot { plated: false }
            } else {
                CutoutKind::Cutout
            },
            outline: square([20.0, 20.0], [24.0, 24.0]),
        }],
        holes: Vec::new(),
        edges: Vec::new(),
    };
    project.board_shape = BoardShape::Sketch(Box::new(BoardSketch {
        evaluator: Default::default(),
        sketch: Default::default(),
        parameters: Vec::new(),
        outline: Vec::new(),
        cutouts: Vec::new(),
        holes: Vec::new(),
        edges: Vec::new(),
        next_feature: 2,
        solve: Default::default(),
        solved,
    }));
}

#[test]
fn copper_must_stay_out_of_cutouts_notches_and_their_clearance() -> Result<()> {
    let (mut project, a, _) = board()?;
    sketched(&mut project, false);
    // First with no edge clearance asked of tracks: only crossings count.
    override_mm(
        &mut project,
        RuleKey::Clearance {
            a: CopperObject::Track,
            b: CopperObject::BoardEdge,
        },
        0.0,
    )?;
    let base = project.next_id + 10;
    project.next_id = base + 10;
    // Through the notch (y 30–40 between x 40 and 50) at y = 35.
    project
        .tracks
        .push(track(base, a, [38.0, 35.0], [45.0, 35.0], 0.3));
    // Into the cutout.
    project
        .tracks
        .push(track(base + 1, a, [18.0, 22.0], [21.0, 22.0], 0.3));
    // 0.25 mm below the cutout's lower edge (y 20): centre at 19.6.
    project
        .tracks
        .push(track(base + 2, a, [20.0, 19.6], [24.0, 19.6], 0.3));
    // Well clear of everything.
    project
        .tracks
        .push(track(base + 3, a, [5.0, 5.0], [10.0, 5.0], 0.3));
    let messages = |p: &Project, code: &str| -> Vec<String> {
        found(p, code).into_iter().map(|f| f.message).collect()
    };
    let outside = messages(&project, "board.outside_outline");
    assert_eq!(outside.len(), 2, "{outside:#?}");
    assert!(
        outside
            .iter()
            .any(|m| m.ends_with("extends beyond board outline"))
    );
    assert!(outside.iter().any(|m| m.ends_with("extends into a cutout")));
    assert!(messages(&project, "board.edge_clearance").is_empty());

    override_mm(
        &mut project,
        RuleKey::Clearance {
            a: CopperObject::Track,
            b: CopperObject::BoardEdge,
        },
        0.3,
    )?;
    let close = messages(&project, "board.edge_clearance");
    assert_eq!(close.len(), 1, "{close:#?}");
    assert!(
        close[0].ends_with(
            "is 0.25 mm from a cutout; needs 0.3 mm (project override: track to board edge clearance)"
        ),
        "{close:#?}"
    );
    // A slot is named as one.
    let mut slotted = project.clone();
    sketched(&mut slotted, true);
    assert!(
        messages(&slotted, "board.edge_clearance")[0].contains("from a slot"),
        "{:#?}",
        messages(&slotted, "board.edge_clearance")
    );
    // A track along the notch's inner corner region but on the board is
    // clear: x 30–38 at y 35 stays left of the notch by 1.85 mm.
    let mut clear = project.clone();
    clear.tracks.retain(|t| t.id == base + 3);
    clear
        .tracks
        .push(track(base + 4, a, [30.0, 35.0], [38.0, 35.0], 0.3));
    assert!(messages(&clear, "board.outside_outline").is_empty());
    assert!(messages(&clear, "board.edge_clearance").is_empty());
    Ok(())
}

/// The first placed part with a 10 × 4 body, its copper unchanged, at `x`
/// mid-board in y.
fn with_large_body(project: &mut Project, x: f64, rotation: f64) -> Result<String> {
    let placement = project
        .placements
        .first_mut()
        .ok_or_else(|| Error::NotFound("a placement".into()))?;
    let id = placement.instance;
    placement.position = [x, 25.0];
    placement.rotation_deg = rotation;
    let instance = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("the instance".into()))?;
    let designator = instance.designator.clone();
    let physical = crate::instances::resolve(&project.library, instance)?
        .physical
        .ok_or_else(|| Error::NotFound("a footprint".into()))?;
    let package = physical.package.id;
    project.board_size = [50.0, 50.0];
    for published in &mut project.library.packages {
        if published.id == package {
            published.definition.envelope = [10.0, 4.0, 1.0];
        }
    }
    Ok(designator)
}

/// A part may overhang the board: only its copper is judged against the edge.
#[test]
fn a_body_past_the_outline_is_not_a_finding_while_its_copper_is_clear() -> Result<()> {
    let (mut project, _, _) = board()?;
    project.tracks.clear();
    project.vias.clear();
    // The body is x ±5 about the placement: at x = 6 it starts at 1.
    with_large_body(&mut project, 6.0, 0.0)?;
    let inside = crate::layout::checks(&project).len();
    // At x = 4 it starts at −1, while the 0805's copper stays on the board.
    with_large_body(&mut project, 4.0, 0.0)?;
    let past = crate::layout::checks(&project);
    assert_eq!(past.len(), inside, "{past:#?}");
    Ok(())
}

#[test]
fn courtyard_clearance_is_reported_only_when_the_rule_asks() -> Result<()> {
    let (mut project, _, _) = board()?;
    let before = crate::layout::courtyard::findings(&project);
    assert!(
        !before
            .iter()
            .any(|f| f.code == crate::layout::courtyard::COURTYARD_CLEARANCE),
        "{before:#?}"
    );
    // The demo's two resistors sit 8 mm apart: far closer than 20 mm.
    override_mm(&mut project, RuleKey::CourtyardClearance, 20.0)?;
    let after = crate::layout::courtyard::findings(&project);
    let close: Vec<_> = after
        .iter()
        .filter(|f| f.code == crate::layout::courtyard::COURTYARD_CLEARANCE)
        .collect();
    assert!(!close.is_empty(), "{after:#?}");
    assert!(
        close[0]
            .message
            .ends_with("needs 20 mm (project override: courtyard clearance)"),
        "{}",
        close[0].message
    );
    Ok(())
}

/// A 3.2 mm mounting hole at (30, 20) on the demo board with no parts and
/// one 0.25 mm track on `net` along y = `y` from x 25 to 35. The hole's
/// edge is 1.6 mm from its centre and the track's 0.125 mm from its
/// centre line, so the gap is `y - 20 - 1.725` mm.
fn hole_and_track(y: f64) -> Result<(Project, Id)> {
    let (mut project, net, _) = board()?;
    project.placements.clear();
    project.tracks.clear();
    project.vias.clear();
    with_hole(&mut project, [30.0, 20.0]);
    let id = project.allocate()?;
    project
        .tracks
        .push(track(id, net, [25.0, y], [35.0, y], 0.25));
    Ok((project, net))
}

/// Make `project`'s board a solved sketch of its rectangle with one 3.2 mm
/// mounting hole at `at`.
fn with_hole(project: &mut Project, at: Point) {
    use crate::model::board::SolvedMountingHole;
    let mut solved = SolvedBoard::rectangle(project.board_size);
    solved.holes.push(SolvedMountingHole {
        id: BoardFeatureId(1),
        at,
        drill: 3.2,
        plated: false,
        pad_diameter: None,
    });
    project.board_shape = BoardShape::Sketch(Box::new(BoardSketch {
        evaluator: Default::default(),
        sketch: Default::default(),
        parameters: Vec::new(),
        outline: Vec::new(),
        cutouts: Vec::new(),
        holes: Vec::new(),
        edges: Vec::new(),
        next_feature: 2,
        solve: Default::default(),
        solved,
    }));
}

/// Mounting-hole clearance measures the gap from the drilled edge to the
/// copper's edge (hand-derived from the fixture), says "overlaps" rather
/// than "0 mm", and its Issues row names the copper in a short title.
#[test]
fn a_mounting_hole_measures_its_gap_to_copper_edge_to_edge() -> Result<()> {
    // 0.3 mm apart: over the 0.25 mm hole-to-copper default, no finding.
    let (clear, _) = hole_and_track(20.0 + 1.725 + 0.3)?;
    assert!(found(&clear, "board.hole_clearance").is_empty());
    // 0.128 mm apart, as the end-to-end run reported: a finding that
    // gives the gap within the hole polygon's chord tolerance.
    let (close, net) = hole_and_track(20.0 + 1.725 + 0.128)?;
    let findings = found(&close, "board.hole_clearance");
    assert_eq!(findings.len(), 1, "{findings:?}");
    let name = close.net_name(net).to_owned();
    let message = &findings[0].message;
    let prefix = "A mounting hole is ";
    let gap: f64 = message
        .strip_prefix(prefix)
        .and_then(|rest| rest.split_once(" mm from a track on "))
        .and_then(|(gap, rest)| rest.starts_with(&name).then_some(gap))
        .and_then(|gap| gap.parse().ok())
        .ok_or_else(|| Error::Invalid(format!("unexpected message {message}")))?;
    assert!(
        (gap - 0.128).abs() <= crate::library::PAD_ARC_TOLERANCE_MM,
        "{message}"
    );
    assert!(
        message.ends_with("needs 0.25 mm (Pcbcraft default: track to hole clearance)"),
        "{message}"
    );
    // Copper over the hole overlaps it: no "0 mm".
    let (over, _) = hole_and_track(20.5)?;
    let findings = found(&over, "board.hole_clearance");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0]
            .message
            .starts_with(&format!("A mounting hole overlaps a track on {name}; ")),
        "{}",
        findings[0].message
    );
    // The Issues row: a short title that names the copper, in its group.
    let report =
        crate::issues::report(&close, &Default::default(), &crate::issues::IssueScope::All)?;
    let row = report
        .rows
        .iter()
        .find(|r| r.group.code == "board.hole_clearance")
        .ok_or_else(|| Error::Invalid("no hole row".into()))?;
    assert_eq!(row.title, format!("Hole too close to track on {name}"));
    assert_eq!(row.group.title, "Too close to a mounting hole");
    // A hole drilled through a pad names the part and the pad number.
    let (mut project, _, _) = board()?;
    project.tracks.clear();
    project.vias.clear();
    let pad = crate::layout::placed_pads(&project)
        .into_iter()
        .find(|p| {
            p.pad.number == "1"
                && project
                    .instance(p.instance)
                    .is_some_and(|i| i.designator == "R1")
        })
        .ok_or_else(|| Error::NotFound("R1 pad 1".into()))?;
    with_hole(&mut project, pad.center());
    let findings = found(&project, "board.hole_clearance");
    let message = findings
        .iter()
        .map(|f| f.message.as_str())
        .find(|m| m.contains("R1 pad 1"))
        .ok_or_else(|| Error::Invalid(format!("no R1 pad 1 finding: {findings:?}")))?;
    assert!(
        message.starts_with("A mounting hole overlaps R1 pad 1 on "),
        "{message}"
    );
    Ok(())
}

#[test]
fn the_default_rules_keep_every_former_verdict() -> Result<()> {
    // The showcase and demo boards check exactly as with the former
    // constants: the default project rules are the default profile.
    let (project, _, _) = board()?;
    assert_eq!(project.rules, ProjectRules::default());
    let mut explicit = project.clone();
    with(
        &mut explicit,
        RulesCommand::SetProfile {
            profile: Box::new(builtin::default_profile()),
            keep_overrides: false,
        },
    )?;
    assert_eq!(
        crate::layout::checks(&project),
        crate::layout::checks(&explicit)
    );
    Ok(())
}
