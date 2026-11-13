//! High-speed routing through the public engine: differential pairs,
//! pair routing, length tuning, the pair, length and impedance checks,
//! the queries, undo and the file round trip.
//!
//! The fixture is the acceptance case in miniature: a USB 2.0 pair
//! (USB_D+ / USB_D-, 90 Ω differential) between two parts on the
//! 4-layer preset, routed on the outer layer over its plane.
use super::support::*;
use crate::layout::meander::{MeanderStyle, TuneAmount, TuneRequest};
use crate::layout::pair::{PairLeg, PairRouteRequest};
use crate::layout::{self, LayoutCommand};
use crate::rules::signal::{DiffPairId, DiffPairSpec, LengthGroupSpec, SignalRules};
use crate::rules::{NetClassSpec, RulesCommand};
use crate::schematic::SchematicCommand;
use crate::{
    CopperLayer, Engine, Error, Id, InstanceCommand, Layer, Operation, Outcome, Point, Query,
    QueryResult, Result, Stackup, UnitId,
};

struct Usb {
    engine: Engine,
    pair: DiffPairId,
    /// `[positive, negative]` nets.
    nets: [Id; 2],
    /// Pad centres `[positive, negative]` at the start and the end.
    start: [Point; 2],
    end: [Point; 2],
}

fn pad_of(engine: &Engine, instance: Id, terminal: u32) -> Result<Point> {
    layout::pads(engine.project())
        .into_iter()
        .find(|p| p.terminal == ep(instance, terminal))
        .map(|p| p.position)
        .ok_or_else(|| Error::NotFound("pad".into()))
}

fn usb(signal: SignalRules) -> Result<Usb> {
    let mut engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let template = engine
        .project()
        .instance(r1)
        .cloned()
        .ok_or_else(|| Error::NotFound("R1".into()))?;
    let mut parts = Vec::new();
    for (designator, x) in [("R3", 0.0), ("R4", 30.0)] {
        apply(
            &mut engine,
            Operation::Instance(InstanceCommand::Place(crate::instances::NewInstance {
                sheet: None,
                component: template.component,
                designator: Some(designator.into()),
                properties: vec![],
                representation: template.representation,
                unit: UnitId(1),
                position: pt([x, 40.0]),
                physical: template.physical,
            })),
        )?;
        parts.push(by_designator(&engine, designator)?);
    }
    let [r3, r4] = parts[..] else {
        return Err(Error::Invalid("parts".into()));
    };
    let mut nets = Vec::new();
    for (terminal, name) in [(1, "USB_D+"), (2, "USB_D-")] {
        apply(
            &mut engine,
            Operation::Schematic(SchematicCommand::Connect {
                from: ep(r3, terminal).into(),
                to: ep(r4, terminal).into(),
            }),
        )?;
        let net = net_of(&engine, ep(r3, terminal))?;
        apply(
            &mut engine,
            Operation::Schematic(SchematicCommand::RenameNet {
                net,
                name: name.into(),
            }),
        )?;
        nets.push(net);
    }
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetBoardSize { size: [60.0, 40.0] }),
    )?;
    apply(&mut engine, Operation::Layout(LayoutCommand::Sync))?;
    for (instance, x) in [(r3, 10.0), (r4, 40.0)] {
        apply(
            &mut engine,
            Operation::Layout(LayoutCommand::Move {
                instance,
                position: [x, 25.0],
                rotation_deg: 90.0,
                layer: Layer::Top,
            }),
        )?;
    }
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetStackup(Stackup::preset(4)?)),
    )?;
    let Outcome::NetClassAdded { class } = apply(
        &mut engine,
        Operation::Rules(RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "USB".into(),
                signal,
                ..Default::default()
            },
        }),
    )?
    else {
        return Err(Error::Invalid("class".into()));
    };
    apply(
        &mut engine,
        Operation::Rules(RulesCommand::AssignNets {
            nets: nets.clone(),
            class: Some(class),
        }),
    )?;
    let Outcome::DiffPairAdded { pair } = apply(
        &mut engine,
        Operation::Rules(RulesCommand::AddDiffPair {
            pair: DiffPairSpec {
                name: "USB_D".into(),
                positive: nets[0],
                negative: nets[1],
            },
        }),
    )?
    else {
        return Err(Error::Invalid("pair".into()));
    };
    let start = [pad_of(&engine, r3, 1)?, pad_of(&engine, r3, 2)?];
    let end = [pad_of(&engine, r4, 1)?, pad_of(&engine, r4, 2)?];
    Ok(Usb {
        engine,
        pair,
        nets: [nets[0], nets[1]],
        start,
        end,
    })
}

fn usb90() -> SignalRules {
    SignalRules {
        diff_impedance: Some(90.0),
        // A check needs a stated limit: without a tolerance the target
        // only sets widths.
        impedance_tolerance: Some(10.0),
        pair_max_skew: Some(0.1),
        pair_max_uncoupled: Some(5.0),
        ..Default::default()
    }
}

fn mid(a: Point, b: Point) -> Point {
    [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])]
}

/// Route the pair pad to pad along `legs` with the rules' width and gap.
fn routed(usb: &mut Usb, legs: Vec<PairLeg>) -> Result<Outcome> {
    let operation = Operation::Layout(LayoutCommand::RoutePair {
        request: PairRouteRequest {
            pair: usb.pair,
            start: usb.start,
            legs,
            end: Some(usb.end),
            width: None,
            gap: None,
        },
    });
    apply(&mut usb.engine, operation)
}

fn straight(usb: &Usb) -> Vec<PairLeg> {
    vec![PairLeg {
        layer: CopperLayer::Top,
        points: vec![mid(usb.start[0], usb.start[1]), mid(usb.end[0], usb.end[1])],
    }]
}

fn board_codes(engine: &Engine) -> Vec<String> {
    layout::checks(engine.project())
        .into_iter()
        .map(|f| f.code)
        .collect()
}

fn report(engine: &Engine) -> Result<layout::length::LengthReport> {
    match engine.query(Query::LengthReport {})? {
        QueryResult::LengthReport(r) => Ok(*r),
        _ => Err(Error::Invalid("report".into())),
    }
}

/// The pads of each part sit side by side across the route.
#[test]
fn the_fixture_has_pads_across_the_route() -> Result<()> {
    let usb = usb(usb90())?;
    assert!((usb.start[0][0] - usb.start[1][0]).abs() < 1e-9);
    assert!((usb.start[0][1] - usb.start[1][1]).abs() > 0.5);
    Ok(())
}

#[test]
fn a_routed_pair_runs_coupled_at_the_derived_gap() -> Result<()> {
    let mut usb = usb(usb90())?;
    let before = usb.engine.revision();
    let legs = straight(&usb);
    routed(&mut usb, legs)?;
    assert_eq!(usb.engine.revision(), before + 1, "one undo step");
    let project = usb.engine.project();
    let pair = project
        .rules
        .diff_pair(usb.pair)
        .cloned()
        .ok_or_else(|| Error::NotFound("pair".into()))?;
    let (width, gap) = layout::impedance::pair_width_gap(project, pair.positive, CopperLayer::Top);
    let coupling = layout::length::coupling(project, &pair);
    assert!(!coupling.coupled_runs.is_empty());
    for (_, _, layer, w, edge) in &coupling.coupled_runs {
        assert_eq!(*layer, CopperLayer::Top);
        assert!(
            (w - width).abs() < 1e-9 && (edge - gap).abs() < 1e-9,
            "{w} {edge}"
        );
    }
    // Coupled over the whole straight run but the fan-out and fan-in.
    let span = (usb.end[0][0] - usb.start[0][0]).abs();
    assert!(coupling.coupled[0] > span - 3.0, "{:?}", coupling);
    // Both halves connect their pads: no unrouted, clearance, pair or
    // impedance finding.
    let codes = board_codes(&usb.engine);
    for code in [
        "board.unrouted",
        "board.clearance",
        "pair.gap",
        "pair.skew",
        "pair.uncoupled",
        "impedance.mismatch",
    ] {
        let hits: Vec<_> = layout::checks(usb.engine.project())
            .into_iter()
            .filter(|f| {
                f.code == code
                    && f.objects
                        .iter()
                        .any(|o| matches!(o, crate::ObjectRef::Net { id } if usb.nets.contains(id)))
            })
            .collect();
        assert!(hits.is_empty(), "{code}: {hits:?} ({codes:?})");
    }
    // Undo takes the whole pair away; redo brings it back.
    let tracks = usb.engine.project().tracks.len();
    usb.engine.undo()?;
    assert!(usb.engine.project().tracks.len() < tracks);
    usb.engine.redo()?;
    assert_eq!(usb.engine.project().tracks.len(), tracks);
    Ok(())
}

#[test]
fn a_layer_change_adds_a_via_pair_and_its_barrel_length() -> Result<()> {
    let mut usb = usb(usb90())?;
    let a = mid(usb.start[0], usb.start[1]);
    let b = mid(usb.end[0], usb.end[1]);
    let turn = [0.5 * (a[0] + b[0]), a[1]];
    routed(
        &mut usb,
        vec![
            PairLeg {
                layer: CopperLayer::Top,
                points: vec![a, turn],
            },
            PairLeg {
                layer: CopperLayer::Bottom,
                points: vec![turn, b],
            },
        ],
    )?;
    let project = usb.engine.project();
    assert_eq!(project.vias.len(), 2);
    let r = report(&usb.engine)?;
    let row = r
        .nets
        .iter()
        .find(|n| n.net == usb.nets[0])
        .ok_or_else(|| Error::NotFound("row".into()))?;
    // Top mid-plane to bottom mid-plane of the preset: 1.6 + 0.035 mm.
    assert!((row.length.vias - 1.635).abs() < 1e-9, "{:?}", row.length);
    assert_eq!(row.length.via_count, 1);
    assert!(
        !board_codes(&usb.engine).contains(&"board.via_dangling".to_owned()),
        "{:?}",
        board_codes(&usb.engine)
    );
    Ok(())
}

/// The longest straight track of `net`.
fn longest(engine: &Engine, net: Id) -> Result<Id> {
    engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.net == net)
        .max_by(|x, y| {
            crate::geometry::distance(x.start, x.end)
                .total_cmp(&crate::geometry::distance(y.start, y.end))
        })
        .map(|t| t.id)
        .ok_or_else(|| Error::NotFound("track".into()))
}

/// Lengthening one half makes the halves unequal: the skew check fires,
/// and tuning the other half to its partner's length clears it with a
/// meander that keeps clear of the partner.
#[test]
fn skew_is_found_and_a_meander_removes_it() -> Result<()> {
    let mut usb = usb(usb90())?;
    let legs = straight(&usb);
    routed(&mut usb, legs)?;
    let track = longest(&usb.engine, usb.nets[1])?;
    apply(
        &mut usb.engine,
        Operation::Layout(LayoutCommand::TuneLength {
            request: TuneRequest {
                track,
                amount: TuneAmount::Add { mm: 0.8 },
                style: MeanderStyle::Trombone,
                amplitude: Some(1.0),
                spacing: None,
                side: None,
            },
        }),
    )?;
    let r = report(&usb.engine)?;
    let pair = r
        .pairs
        .first()
        .ok_or_else(|| Error::NotFound("pair".into()))?;
    assert!(pair.skew > 0.1, "{pair:?}");
    assert!(board_codes(&usb.engine).contains(&"pair.skew".to_owned()));
    // The shorter half's longest straight segment.
    let short = if pair.positive < pair.negative {
        usb.nets[0]
    } else {
        usb.nets[1]
    };
    let target = pair.positive.max(pair.negative);
    let track = usb
        .engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.net == short)
        .max_by(|x, y| {
            crate::geometry::distance(x.start, x.end)
                .total_cmp(&crate::geometry::distance(y.start, y.end))
        })
        .map(|t| t.id)
        .ok_or_else(|| Error::NotFound("track".into()))?;
    let request = TuneRequest {
        track,
        amount: TuneAmount::Target { mm: target },
        style: MeanderStyle::Trombone,
        amplitude: Some(1.0),
        spacing: None,
        side: None,
    };
    let QueryResult::TunePreview(preview) = usb.engine.query(Query::TunePreview {
        request: request.clone(),
    })?
    else {
        return Err(Error::Invalid("preview".into()));
    };
    let before = usb.engine.revision();
    apply(
        &mut usb.engine,
        Operation::Layout(LayoutCommand::TuneLength { request }),
    )?;
    assert_eq!(usb.engine.revision(), before + 1);
    let r = report(&usb.engine)?;
    let pair = r
        .pairs
        .first()
        .ok_or_else(|| Error::NotFound("pair".into()))?;
    assert!(pair.skew < 1e-5, "{pair:?} {preview:?}");
    let codes = board_codes(&usb.engine);
    assert!(!codes.contains(&"pair.skew".to_owned()), "{codes:?}");
    assert!(!codes.contains(&"board.clearance".to_owned()), "{codes:?}");
    Ok(())
}

#[test]
fn a_meander_adds_the_exact_length_and_keeps_clearance() -> Result<()> {
    let mut usb = usb(usb90())?;
    let legs = straight(&usb);
    routed(&mut usb, legs)?;
    let net = usb.nets[0];
    let before = report(&usb.engine)?
        .nets
        .iter()
        .find(|n| n.net == net)
        .map(|n| n.length.total)
        .unwrap_or_default();
    let track = usb
        .engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.net == net)
        .max_by(|x, y| {
            crate::geometry::distance(x.start, x.end)
                .total_cmp(&crate::geometry::distance(y.start, y.end))
        })
        .map(|t| t.id)
        .ok_or_else(|| Error::NotFound("track".into()))?;
    for style in [MeanderStyle::Trombone, MeanderStyle::Accordion] {
        let mut engine = Engine::from_file(usb.engine.to_file())?;
        let request = TuneRequest {
            track,
            amount: TuneAmount::Add { mm: 2.5 },
            style,
            amplitude: Some(1.5),
            spacing: None,
            side: None,
        };
        let outcome = apply(
            &mut engine,
            Operation::Layout(LayoutCommand::TuneLength { request }),
        );
        if style == MeanderStyle::Accordion {
            // Swinging to both sides runs into the partner half.
            assert!(outcome.is_err(), "{outcome:?}");
            continue;
        }
        outcome?;
        let after = report(&engine)?
            .nets
            .iter()
            .find(|n| n.net == net)
            .map(|n| n.length.total)
            .unwrap_or_default();
        assert!((after - before - 2.5).abs() < 1e-5, "{before} {after}");
        // Leg spacing: track width plus at least the track clearance.
        let codes = board_codes(&engine);
        assert!(!codes.contains(&"board.clearance".to_owned()), "{codes:?}");
    }
    Ok(())
}

#[test]
fn a_refused_tuning_changes_nothing() -> Result<()> {
    let mut usb = usb(usb90())?;
    let legs = straight(&usb);
    routed(&mut usb, legs)?;
    let track = usb
        .engine
        .project()
        .tracks
        .iter()
        .find(|t| t.net == usb.nets[0])
        .map(|t| t.id)
        .ok_or_else(|| Error::NotFound("track".into()))?;
    let kept = Snapshot::of(&usb.engine);
    for amount in [
        TuneAmount::Add { mm: 5000.0 },
        TuneAmount::Add { mm: -1.0 },
        TuneAmount::Add { mm: f64::NAN },
    ] {
        let refused = usb.engine.apply(
            usb.engine.revision(),
            Operation::Layout(LayoutCommand::TuneLength {
                request: TuneRequest {
                    track,
                    amount,
                    style: MeanderStyle::Trombone,
                    amplitude: None,
                    spacing: Some(0.01),
                    side: None,
                },
            }),
        );
        assert!(refused.is_err());
        kept.assert_kept(&usb.engine, "refused tuning");
    }
    Ok(())
}

#[test]
fn length_groups_report_and_flag_mismatches() -> Result<()> {
    let mut usb = usb(usb90())?;
    let legs = straight(&usb);
    routed(&mut usb, legs)?;
    let Outcome::LengthGroupAdded { group } = apply(
        &mut usb.engine,
        Operation::Rules(RulesCommand::AddLengthGroup {
            group: LengthGroupSpec {
                name: "USB".into(),
                nets: vec![],
                pairs: vec![usb.pair],
                target: Some(50.0),
                tolerance: 0.5,
            },
        }),
    )?
    else {
        return Err(Error::Invalid("group".into()));
    };
    let r = report(&usb.engine)?;
    let row = r
        .groups
        .iter()
        .find(|g| g.group == group)
        .ok_or_else(|| Error::NotFound("group".into()))?;
    assert!(!row.ok && row.members.len() == 2);
    let findings: Vec<_> = layout::checks(usb.engine.project())
        .into_iter()
        .filter(|f| f.code == "length.mismatch")
        .collect();
    assert_eq!(findings.len(), 2, "{findings:?}");
    for f in &findings {
        let title = f.message.split("; ").next().unwrap_or_default();
        assert!(crate::issues::style::check(title).is_empty(), "{title}");
    }
    // The Issues report classifies them.
    let QueryResult::Issues(issues) = usb.engine.query(Query::Issues {
        scope: crate::issues::IssueScope::All,
    })?
    else {
        return Err(Error::Invalid("issues".into()));
    };
    assert!(
        issues
            .rows
            .iter()
            .any(|r| r.finding.code == "length.mismatch" && r.group.title == "Length mismatch")
    );
    Ok(())
}

#[test]
fn a_narrow_pair_gap_and_an_off_target_width_are_found() -> Result<()> {
    let mut usb = usb(usb90())?;
    let legs = straight(&usb);
    let (width, gap) =
        layout::impedance::pair_width_gap(usb.engine.project(), usb.nets[0], CopperLayer::Top);
    // Twice the width: the impedance drops well below 90 Ω, still at the
    // gap.
    apply(
        &mut usb.engine,
        Operation::Layout(LayoutCommand::RoutePair {
            request: PairRouteRequest {
                pair: usb.pair,
                start: usb.start,
                legs: legs.clone(),
                end: Some(usb.end),
                width: Some(2.0 * width),
                gap: Some(gap),
            },
        }),
    )?;
    assert!(board_codes(&usb.engine).contains(&"impedance.mismatch".to_owned()));
    usb.engine.undo()?;
    // A wider class gap: routing at the clearance is closer than the
    // pair gap.
    let class = usb.engine.project().rules.net_classes.first().cloned();
    let mut class = class.ok_or_else(|| Error::NotFound("class".into()))?;
    class.diff_pair_gap = Some(gap + 0.15);
    class.signal.diff_impedance = None;
    apply(
        &mut usb.engine,
        Operation::Rules(RulesCommand::EditNetClass { class }),
    )?;
    apply(
        &mut usb.engine,
        Operation::Layout(LayoutCommand::RoutePair {
            request: PairRouteRequest {
                pair: usb.pair,
                start: usb.start,
                legs,
                end: Some(usb.end),
                width: Some(width),
                gap: Some(gap),
            },
        }),
    )?;
    assert!(board_codes(&usb.engine).contains(&"pair.gap".to_owned()));
    Ok(())
}

#[test]
fn pairs_survive_the_file_round_trip_and_follow_undo() -> Result<()> {
    let mut usb = usb(usb90())?;
    let legs = straight(&usb);
    routed(&mut usb, legs)?;
    let bytes = crate::format::encode(&usb.engine.to_file())?;
    let reopened = Engine::from_file(crate::format::decode(&bytes)?)?;
    assert_eq!(reopened.project().rules, usb.engine.project().rules);
    assert_eq!(reopened.project().tracks, usb.engine.project().tracks);
    assert_eq!(report(&reopened)?, report(&usb.engine)?);
    // Deleting the pair is one undo step.
    apply(
        &mut usb.engine,
        Operation::Rules(RulesCommand::DeleteDiffPair { pair: usb.pair }),
    )?;
    assert!(usb.engine.project().rules.diff_pairs.is_empty());
    usb.engine.undo()?;
    assert_eq!(usb.engine.project().rules.diff_pairs.len(), 1);
    // A pair of a missing net is refused and changes nothing.
    let kept = Snapshot::of(&usb.engine);
    let refused = usb.engine.apply(
        usb.engine.revision(),
        Operation::Rules(RulesCommand::AddDiffPair {
            pair: DiffPairSpec {
                name: "Ghost".into(),
                positive: 9_999,
                negative: usb.nets[0],
            },
        }),
    );
    assert!(refused.is_err());
    kept.assert_kept(&usb.engine, "pair of a missing net");
    Ok(())
}

#[test]
fn the_queries_reach_the_calculator_suggestions_and_table() -> Result<()> {
    let usb = usb(usb90())?;
    let QueryResult::PairSuggestions(suggested) = usb.engine.query(Query::SuggestPairs {})? else {
        return Err(Error::Invalid("suggest".into()));
    };
    // USB_D+/USB_D- is already a pair.
    assert!(suggested.iter().all(|s| s.positive != usb.nets[0]));
    let QueryResult::ImpedanceTable(table) = usb.engine.query(Query::ImpedanceTable {})? else {
        return Err(Error::Invalid("table".into()));
    };
    assert_eq!(table.layers.len(), 4);
    let top = table
        .classes
        .first()
        .and_then(|c| c.layers.first())
        .ok_or_else(|| Error::NotFound("row".into()))?;
    assert!(
        (top.zdiff.unwrap_or_default() - 90.0).abs() < 0.01,
        "{top:?}"
    );
    // The wire form CLI and MCP send.
    let query: Query = serde_json::from_value(serde_json::json!({
        "query": "impedance.calculate",
        "params": {"request": {"on": "layer", "layer": "Top", "width": 0.2, "gap": 0.2}}
    }))?;
    let QueryResult::Impedance(z) = usb.engine.query(query)? else {
        return Err(Error::Invalid("calculate".into()));
    };
    assert!(z.zdiff.is_some() && z.z0 > 30.0 && z.z0 < 80.0, "{z:?}");
    let operation: Operation = serde_json::from_value(serde_json::json!({
        "editor": "Rules",
        "operation": {"command": "length_group.add", "params": {"group": {
            "name": "Bus", "nets": [usb.nets[0]], "tolerance": 0.2
        }}}
    }))?;
    let mut engine = usb.engine;
    assert!(matches!(
        engine.apply(engine.revision(), operation)?,
        Outcome::LengthGroupAdded { .. }
    ));
    Ok(())
}

fn branch(usb: &mut Usb, net: Id, from: Point, to: Point) -> Result<()> {
    apply(
        &mut usb.engine,
        Operation::Layout(LayoutCommand::Route {
            request: layout::RouteRequest {
                net,
                start: from,
                end: to,
                width: 0.2,
                layer: CopperLayer::Top,
            },
        }),
    )?;
    Ok(())
}

/// The point on a coupled track of `net`, halfway along its longest one.
fn on_run(usb: &Usb, net: Id) -> Result<Point> {
    usb.engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.net == net)
        .max_by(|a, b| {
            crate::geometry::distance(a.start, a.end)
                .total_cmp(&crate::geometry::distance(b.start, b.end))
        })
        .map(|t| mid(t.start, t.end))
        .ok_or_else(|| Error::NotFound("track".into()))
}

/// A two-pin half measures exactly its total copper; a branch off it that
/// ends at no pad is net copper off the path, in neither skew nor uncoupled
/// length. Its open end is a stub: the board check reports it as dangling
/// copper, and no pair finding names it. Hand-derived: the branch runs
/// straight for 4 mm, the net's total less its path.
#[test]
fn an_open_branch_is_not_skew_and_the_dangling_check_reports_it() -> Result<()> {
    let mut usb = usb(usb90())?;
    let legs = straight(&usb);
    routed(&mut usb, legs)?;
    let before = report(&usb.engine)?;
    let row = before
        .pairs
        .first()
        .ok_or_else(|| Error::NotFound("row".into()))?;
    assert_eq!(row.measured, layout::length::PairMeasured::Derived);
    assert!(row.skew < 1e-9, "{row:?}");
    let copper = layout::length::net_lengths(usb.engine.project());
    let total = copper.get(&usb.nets[0]).map_or(0.0, |l| l.total);
    assert!(
        (row.positive - total).abs() < 1e-9,
        "{} {total}",
        row.positive
    );
    let uncoupled = row.uncoupled;

    let net = usb.nets[0];
    let at = on_run(&usb, net)?;
    // Away from the partner.
    let away = if at[1] > mid(usb.start[0], usb.start[1])[1] {
        4.0
    } else {
        -4.0
    };
    let before: Vec<Id> = usb.engine.project().tracks.iter().map(|t| t.id).collect();
    branch(&mut usb, net, at, [at[0], at[1] + away])?;
    let spur: Vec<Id> = usb
        .engine
        .project()
        .tracks
        .iter()
        .map(|t| t.id)
        .filter(|id| !before.contains(id))
        .collect();
    let r = report(&usb.engine)?;
    let row = r
        .pairs
        .first()
        .ok_or_else(|| Error::NotFound("row".into()))?;
    assert!(row.skew < 1e-9, "{row:?}");
    assert!((row.uncoupled - uncoupled).abs() < 1e-9, "{row:?}");
    let net_row = r
        .nets
        .iter()
        .find(|n| n.net == usb.nets[0])
        .ok_or_else(|| Error::NotFound("net".into()))?;
    assert!((net_row.length.total - total - 4.0).abs() < 1e-9);
    assert_eq!(net_row.path, Some(row.positive));
    assert!((net_row.length.total - row.positive - 4.0).abs() < 1e-9);
    let findings = layout::checks(usb.engine.project());
    assert!(
        !findings.iter().any(|f| f.code.starts_with("pair.")),
        "{findings:?}"
    );
    let dangling: Vec<_> = findings
        .iter()
        .filter(|f| f.code == "board.track_dangling")
        .collect();
    let [end] = dangling.as_slice() else {
        return Err(Error::Invalid(format!("{findings:?}")));
    };
    assert!(
        spur.iter()
            .any(|&id| end.objects.contains(&crate::ObjectRef::Track { id })),
        "{end:?}"
    );
    // The tuning target compares paths, too.
    assert!(net_row.delta.is_none_or(|d| d.abs() < 1e-9), "{net_row:?}");
    Ok(())
}

/// A branch from a pair half to a third pin of its net is a track between
/// pins: it is terminated and raises nothing, neither a pair finding nor
/// dangling copper, and it is off the path. Hand-derived: R5's pad 1 is
/// placed 4 mm straight off the run, so the net holds 4 mm beyond its path.
#[test]
fn a_pin_to_pin_branch_is_a_track_between_pins_and_raises_nothing() -> Result<()> {
    let mut usb = usb(usb90())?;
    let r3 = by_designator(&usb.engine, "R3")?;
    let template = usb
        .engine
        .project()
        .instance(r3)
        .cloned()
        .ok_or_else(|| Error::NotFound("R3".into()))?;
    apply(
        &mut usb.engine,
        Operation::Instance(InstanceCommand::Place(crate::instances::NewInstance {
            sheet: None,
            component: template.component,
            designator: Some("R5".into()),
            properties: vec![],
            representation: template.representation,
            unit: UnitId(1),
            position: pt([15.0, 80.0]),
            physical: template.physical,
        })),
    )?;
    let r5 = by_designator(&usb.engine, "R5")?;
    apply(
        &mut usb.engine,
        Operation::Schematic(SchematicCommand::Connect {
            from: ep(r5, 1).into(),
            to: ep(r3, 1).into(),
        }),
    )?;
    apply(&mut usb.engine, Operation::Layout(LayoutCommand::Sync))?;
    // Clear of the route until the pair is routed.
    apply(
        &mut usb.engine,
        Operation::Layout(LayoutCommand::Move {
            instance: r5,
            position: [25.0, 5.0],
            rotation_deg: 0.0,
            layer: Layer::Top,
        }),
    )?;
    let legs = straight(&usb);
    routed(&mut usb, legs)?;
    let net = usb.nets[0];
    let at = on_run(&usb, net)?;
    let away = if at[1] > mid(usb.start[0], usb.start[1])[1] {
        4.0
    } else {
        -4.0
    };
    // Unrotated, R5's pads lie along x: pad 2 sits beside the link's end.
    let pin = [at[0], at[1] + away];
    let origin = usb
        .engine
        .project()
        .placements
        .iter()
        .find(|p| p.instance == r5)
        .map(|p| p.position)
        .ok_or_else(|| Error::NotFound("R5 placement".into()))?;
    let pad = pad_of(&usb.engine, r5, 1)?;
    apply(
        &mut usb.engine,
        Operation::Layout(LayoutCommand::Move {
            instance: r5,
            position: [pin[0] - (pad[0] - origin[0]), pin[1] - (pad[1] - origin[1])],
            rotation_deg: 0.0,
            layer: Layer::Top,
        }),
    )?;
    let pad = pad_of(&usb.engine, r5, 1)?;
    assert!(
        crate::geometry::distance(pad, pin) < 1e-9,
        "{pad:?} {pin:?}"
    );
    branch(&mut usb, net, at, pin)?;
    let r = report(&usb.engine)?;
    let row = r
        .pairs
        .first()
        .ok_or_else(|| Error::NotFound("row".into()))?;
    assert_eq!(row.measured, layout::length::PairMeasured::Derived);
    assert!(row.skew < 1e-9, "{row:?}");
    let net_row = r
        .nets
        .iter()
        .find(|n| n.net == net)
        .ok_or_else(|| Error::NotFound("net".into()))?;
    assert!(
        (net_row.length.total - row.positive - 4.0).abs() < 1e-9,
        "{net_row:?} {row:?}"
    );
    // The demo board's own nets stay unrouted; the pair's are complete.
    let pair_net = |f: &crate::Finding| {
        usb.nets
            .iter()
            .any(|&id| f.objects.contains(&crate::ObjectRef::Net { id }))
    };
    let findings = layout::checks(usb.engine.project());
    assert!(
        !findings.iter().any(|f| f.code.starts_with("pair.")
            || f.code == "board.track_dangling"
            || (f.code == "board.unrouted" && pair_net(f))),
        "{findings:?}"
    );
    Ok(())
}

/// A half routed part of the way has no path: both halves are measured by
/// total copper, as a route in progress always was.
#[test]
fn an_unfinished_half_is_measured_by_its_copper() -> Result<()> {
    let mut usb = usb(usb90())?;
    let (p, n) = (usb.start[0], usb.start[1]);
    let nets = usb.nets;
    branch(&mut usb, nets[0], p, [p[0] + 3.0, p[1]])?;
    branch(&mut usb, nets[1], n, [n[0] + 5.0, n[1]])?;
    let r = report(&usb.engine)?;
    let row = r
        .pairs
        .first()
        .ok_or_else(|| Error::NotFound("row".into()))?;
    assert_eq!(row.measured, layout::length::PairMeasured::Copper);
    assert!(row.paths.is_none(), "{row:?}");
    assert!((row.skew - 2.0).abs() < 1e-9, "{row:?}");
    Ok(())
}

/// Endpoints are saved only when set: an unset pair's file is unchanged.
#[test]
fn pair_endpoints_round_trip_and_stay_out_of_unset_files() -> Result<()> {
    let usb = usb(usb90())?;
    let pair = usb
        .engine
        .project()
        .rules
        .diff_pair(usb.pair)
        .cloned()
        .ok_or_else(|| Error::NotFound("pair".into()))?;
    let text = serde_json::to_string(&pair).map_err(|e| Error::Invalid(e.to_string()))?;
    assert!(!text.contains("endpoints"), "{text}");
    let set = crate::rules::signal::DiffPair {
        endpoints: Some(crate::rules::signal::PairEndpoints {
            positive: [ep(1, 1), ep(2, 1)],
            negative: [ep(1, 2), ep(2, 2)],
        }),
        ..pair
    };
    let text = serde_json::to_string(&set).map_err(|e| Error::Invalid(e.to_string()))?;
    let back: crate::rules::signal::DiffPair =
        serde_json::from_str(&text).map_err(|e| Error::Invalid(e.to_string()))?;
    assert_eq!(back, set);
    Ok(())
}

/// Skew is checked only against a limit the pair's net class sets: with
/// none, no `pair.skew` however large the skew; within a class limit, none;
/// past it, one that leads with the pair and cites the class. Lengths are the
/// meander's added amount.
#[test]
fn skew_is_checked_only_against_a_class_limit() -> Result<()> {
    let limit = |mm| SignalRules {
        pair_max_skew: Some(mm),
        ..SignalRules::default()
    };
    for (signal, added, findings, cites) in [
        (SignalRules::default(), 20.0, 0, ""),
        (limit(5.0), 3.0, 0, ""),
        (limit(2.0), 3.0, 1, "net class USB: max skew"),
    ] {
        let mut usb = usb(signal)?;
        let legs = straight(&usb);
        routed(&mut usb, legs)?;
        assert!(!board_codes(&usb.engine).contains(&"pair.skew".to_owned()));
        let track = longest(&usb.engine, usb.nets[1])?;
        apply(
            &mut usb.engine,
            Operation::Layout(LayoutCommand::TuneLength {
                request: TuneRequest {
                    track,
                    amount: TuneAmount::Add { mm: added },
                    style: MeanderStyle::Trombone,
                    amplitude: Some(1.0),
                    spacing: None,
                    side: None,
                },
            }),
        )?;
        let r = report(&usb.engine)?;
        let pair = r
            .pairs
            .first()
            .ok_or_else(|| Error::NotFound("pair".into()))?;
        assert!((pair.skew - added).abs() < 0.05, "{pair:?}");
        let skews: Vec<_> = layout::checks(usb.engine.project())
            .into_iter()
            .filter(|f| f.code == "pair.skew")
            .collect();
        assert_eq!(skews.len(), findings, "{skews:?}");
        for skew in skews {
            assert!(skew.message.starts_with("USB_D"), "{}", skew.message);
            assert!(skew.message.contains(cites), "{}", skew.message);
        }
    }
    Ok(())
}

/// Run 3, finding 8: the production check warns on a pair's open copper as
/// dangling, whatever limits its class sets, and on skew only past a limit
/// its class sets. The skew is 3.5 mm, past 2 mm.
#[test]
fn the_production_check_warns_on_open_pair_copper_and_skew() -> Result<()> {
    use crate::fabrication::preflight;
    use crate::issues::Severity;
    for (signal, warned) in [
        (SignalRules::default(), false),
        (
            SignalRules {
                pair_max_skew: Some(2.0),
                ..SignalRules::default()
            },
            true,
        ),
    ] {
        let mut usb = usb(signal)?;
        let legs = straight(&usb);
        routed(&mut usb, legs)?;
        let net = usb.nets[0];
        let at = on_run(&usb, net)?;
        let away = if at[1] > mid(usb.start[0], usb.start[1])[1] {
            4.0
        } else {
            -4.0
        };
        branch(&mut usb, net, at, [at[0], at[1] + away])?;
        let warnings = |engine: &Engine, code: &str| -> Result<Vec<String>> {
            let report = preflight::report(engine.project(), &Default::default(), None)?;
            Ok(report
                .items
                .into_iter()
                .filter(|i| i.code == code && i.severity == Severity::Warning)
                .map(|i| i.title)
                .collect())
        };
        let open = warnings(&usb.engine, "board.track_dangling")?;
        let [end] = open.as_slice() else {
            return Err(Error::Invalid(format!("{open:?}")));
        };
        assert!(end.starts_with("Track on USB_D+"), "{end}");
        // Skew 3.5 mm: warned only past the class's limit.
        let track = longest(&usb.engine, usb.nets[1])?;
        apply(
            &mut usb.engine,
            Operation::Layout(LayoutCommand::TuneLength {
                request: TuneRequest {
                    track,
                    amount: TuneAmount::Add { mm: 3.5 },
                    style: MeanderStyle::Trombone,
                    amplitude: Some(1.0),
                    spacing: None,
                    side: None,
                },
            }),
        )?;
        let skews = warnings(&usb.engine, "pair.skew")?;
        assert_eq!(skews.len(), usize::from(warned), "{skews:?}");
    }
    Ok(())
}
