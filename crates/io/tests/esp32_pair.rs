//! The differential pair on the shipped ESP32-S31 dev board example: its
//! USB-C receptacle joins both plug orientations, so each half holds a
//! track between pins off its signal path.
//!
//! Hand-derived from the board's tracks. Along x = 20.75 and x = 19.25 the
//! halves reach the ESD pads, then run coupled to the module:
//! B6/B7 to U3 is 4.645 + 1.563 mm; U3 to U1 is 23.657 mm on each half (the
//! positive half's corners fall 0.174 mm apart from the negative's and
//! cancel), so 29.865 mm each.
//! - The positive jumper, A6 to the T on B6's track, is 1.245 + 1.0 =
//!   2.245 mm.
//! - The negative jumper, A7 through a via, the bottom jumper and a via to
//!   B7, is 1.355 + 1.0 + 1.355 = 3.71 mm of track and two 1.635 mm barrels
//!   (when the stackup's mid-planes are as the file says), 6.98 mm.
use pcbcraft_core::diagnostics::Finding;
use pcbcraft_core::ids::{Endpoint, Id};
use pcbcraft_core::layout::length::{self, LengthReport, PairMeasured, PairRow};
use pcbcraft_core::layout::posture::Posture;
use pcbcraft_core::layout::router::{self, RouteIndex, RouteMode, RouteStepRequest};
use pcbcraft_core::layout::{self, LayoutCommand};
use pcbcraft_core::model::{CopperLayer, Project};
use pcbcraft_core::rules::RulesCommand;
use pcbcraft_core::rules::signal::{DiffPair, PairEndpoints};
use std::path::Path;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn board() -> Fallible<Project> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    Ok(pcbcraft_io::read_project(&path)?.project)
}

fn usb(p: &Project) -> Fallible<DiffPair> {
    Ok(p.rules.diff_pairs.first().cloned().ok_or("no pair")?)
}

fn row(report: &LengthReport) -> Fallible<&PairRow> {
    Ok(report.pairs.first().ok_or("no pair row")?)
}

fn net(p: &Project, name: &str) -> Fallible<Id> {
    Ok(p.nets
        .iter()
        .find(|n| n.name == name)
        .map(|n| n.id)
        .ok_or("no net")?)
}

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

fn pair_findings(findings: &[Finding]) -> Vec<&Finding> {
    findings
        .iter()
        .filter(|f| f.code.starts_with("pair."))
        .collect()
}

/// Gap 2: equal paths, no skew. The plug-orientation jumpers are tracks
/// between pins: each joins the receptacle's second contact, a pin, so it
/// is terminated copper off the path, never warned (run 3, finding 8, as
/// the founder decided) and not listed apart from the net's copper.
#[test]
fn the_receptacle_jumpers_are_off_the_path_and_not_skew() -> Fallible {
    let p = board()?;
    let report = length::report(&p);
    let usb = row(&report)?;
    assert_eq!(usb.measured, PairMeasured::Derived);
    assert!(close(usb.positive, 29.865, 0.002), "{usb:?}");
    assert!(close(usb.negative, 29.865, 0.002), "{usb:?}");
    assert!(usb.skew < 1e-3, "{usb:?}");
    let paths = usb.paths.as_ref().ok_or("no paths")?;
    assert_eq!(
        (paths[0].from.as_str(), paths[0].to.as_str()),
        ("J1.B6", "U1.43"),
        "{paths:?}"
    );
    assert_eq!(
        (paths[1].from.as_str(), paths[1].to.as_str()),
        ("J1.B7", "U1.42"),
        "{paths:?}"
    );
    assert!(paths.iter().all(|p| !p.via_fill), "{paths:?}");
    // Total copper still holds the jumpers: the old 4.735 mm difference.
    let total = |name: &str| -> Fallible<f64> {
        Ok(report
            .nets
            .iter()
            .find(|n| n.name == name)
            .ok_or("no net row")?
            .length
            .total)
    };
    assert!(
        close((total("USB_DN")? - total("USB_DP")?).abs(), 4.735, 0.002),
        "the copper difference is unchanged"
    );
    assert!(close(total("USB_DP")? - usb.positive, 2.245, 0.002));
    assert!(close(total("USB_DN")? - usb.negative, 6.98, 0.005));
    let findings = layout::checks(&p);
    assert!(pair_findings(&findings).is_empty(), "{findings:?}");
    // The report CLI and MCP callers read lists the paths and nothing off
    // them: no branches, and nothing called a stub.
    let json = serde_json::to_value(usb)?;
    assert!(json.get("branches").is_none(), "{json}");
    assert!(!json.to_string().contains("stub"), "{json}");
    Ok(())
}

/// Real skew on the signal path is still found: 1 mm of detour on one
/// half's coupled run, keeping the jumpers.
#[test]
fn real_skew_on_the_path_is_still_reported() -> Fallible {
    let mut p = board()?;
    let dp = net(&p, "USB_DP")?;
    let track = p
        .tracks
        .iter()
        .filter(|t| t.net == dp)
        .max_by(|a, b| {
            pcbcraft_core::geometry::distance(a.start, a.end)
                .total_cmp(&pcbcraft_core::geometry::distance(b.start, b.end))
        })
        .map(|t| t.id)
        .ok_or("no track")?;
    // Lengthen the long run on one half by a meander.
    let index = pcbcraft_core::layout::obstacles::Obstacles::of(&p);
    let plan = pcbcraft_core::layout::meander::plan(
        &p,
        &index,
        &pcbcraft_core::layout::CopperGraph::of(&p),
        &pcbcraft_core::layout::meander::TuneRequest {
            track,
            amount: pcbcraft_core::layout::meander::TuneAmount::Add { mm: 1.0 },
            style: pcbcraft_core::layout::meander::MeanderStyle::Trombone,
            amplitude: Some(1.0),
            spacing: None,
            side: None,
        },
    )?;
    layout::apply(&mut p, LayoutCommand::ApplyRoute { edits: plan.edits })?;
    let report = length::report(&p);
    let usb = row(&report)?;
    assert!(close(usb.skew, 1.0, 0.01), "{usb:?}");
    let findings = layout::checks(&p);
    assert!(
        findings.iter().any(|f| f.code == "pair.skew"),
        "{:?}",
        pair_findings(&findings)
    );
    Ok(())
}

/// Endpoints can be set, are honoured, and survive only as terminals of
/// their own nets: measuring from A6 and A7 makes the jumpers the path, and
/// that is real skew. A pad of another net is refused and changes nothing.
#[test]
fn set_endpoints_are_measured_and_validated() -> Fallible {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    let mut engine = pcbcraft_core::Engine::from_file(pcbcraft_io::read_project(&path)?)?;
    let p = engine.project().clone();
    let pair = usb(&p)?;
    let pad = |designator: &str, number: &str| -> Fallible<Endpoint> {
        let labels = layout::pad_labels(&p);
        let instance = p
            .instances
            .iter()
            .find(|i| i.designator == designator)
            .ok_or("no part")?
            .id;
        Ok(layout::pads(&p)
            .into_iter()
            .find(|w| {
                w.instance == instance
                    && labels.get(&(w.instance, w.pad)).map(String::as_str) == Some(number)
            })
            .ok_or("no pad")?
            .terminal)
    };
    let edit = |endpoints| {
        pcbcraft_core::Operation::Rules(RulesCommand::EditDiffPair {
            pair: DiffPair {
                endpoints,
                ..pair.clone()
            },
        })
    };
    let wrong = PairEndpoints {
        positive: [pad("J1", "A7")?, pad("U1", "43")?],
        negative: [pad("J1", "A7")?, pad("U1", "42")?],
    };
    let revision = engine.revision();
    assert!(engine.apply(revision, edit(Some(wrong))).is_err());
    assert_eq!(engine.revision(), revision, "a refusal changes nothing");

    let from_a = PairEndpoints {
        positive: [pad("J1", "A6")?, pad("U1", "43")?],
        negative: [pad("J1", "A7")?, pad("U1", "42")?],
    };
    engine.apply(revision, edit(Some(from_a)))?;
    let report = length::report(engine.project());
    let usb = row(&report)?;
    assert_eq!(usb.measured, PairMeasured::Set);
    // A6 joins 1.245 mm up B6's track after 2.245 mm: 1 mm longer than B6.
    assert!(close(usb.positive, 30.865, 0.002), "{usb:?}");
    // A7 through the jumper to B7: the whole 6.98 mm jumper joins the path.
    assert!(close(usb.negative, 29.865 + 6.98, 0.01), "{usb:?}");
    assert!(close(usb.skew, 5.98, 0.01), "{usb:?}");
    assert!(
        layout::checks(engine.project())
            .iter()
            .any(|f| f.code == "pair.skew")
    );
    // Undo restores derived endpoints.
    engine.undo()?;
    assert_eq!(
        row(&length::report(engine.project()))?.measured,
        PairMeasured::Derived
    );
    Ok(())
}

/// A step of TX0 across the pair, in Shove.
fn across(p: &Project, head: [f64; 2], to: [f64; 2], width: f64) -> Fallible<RouteStepRequest> {
    Ok(RouteStepRequest {
        net: net(p, "TX0")?,
        width,
        layer: CopperLayer::Top,
        head,
        to,
        posture: Posture::Diagonal,
        flipped: false,
        via: None,
        via_layer: None,
        mode: RouteMode::Shove,
        move_vias: true,
        max_detour: None,
    })
}

/// The founder's test drive: a track pushed across the USB pair. Before
/// the fix the halves took separate bumps; coupling fell from 21.96 mm to
/// 2.30 mm. Now every step across leaves the pair coupled at its gap with
/// its skew, moved whole or not at all, and says so when it did not move.
#[test]
fn a_track_across_the_pair_never_tears_it() -> Fallible {
    let base = board()?;
    let pair = usb(&base)?;
    let before = length::coupling(&base, &pair);
    let skew_before = row(&length::report(&base))?.skew;
    let index = RouteIndex::of(&base);
    let (mut moved, mut held) = (0, 0);
    for (y0, y1) in [(18.0, 18.0), (20.0, 20.0), (24.0, 24.0), (20.0, 24.0)] {
        for k in 0..60 {
            let x = 18.0 + 0.1 * f64::from(k);
            let plan = router::plan(&base, &index, &across(&base, [14.0, y0], [x, y1], 0.25)?);
            let mut after = base.clone();
            layout::apply(
                &mut after,
                LayoutCommand::ApplyRoute {
                    edits: plan.edits.clone(),
                },
            )?;
            let c = length::coupling(&after, &pair);
            let at = format!("y {y0}->{y1} x {x:.1}");
            for side in 0..2 {
                // A mitred 45° corner leaves the outer half 2·pitch·tan(22.5°)
                // outside the parallel overlap the check counts as coupled,
                // 0.347 mm at this pair's 0.419 mm pitch (a mitre, not a
                // tear), and a bend round an octagon has at most eight. The
                // tearing this guards against lost 3 to 20 mm.
                let bend = 8.0 * 0.347;
                assert!(
                    c.coupled.get(side)
                        >= before.coupled.get(side).map(|v| v - bend - 1e-6).as_ref(),
                    "{at}: coupled {:?} from {:?}",
                    c.coupled,
                    before.coupled
                );
            }
            assert!(c.too_close.is_empty(), "{at}: {:?}", c.too_close);
            let report = length::report(&after);
            let usb = row(&report)?;
            assert!(usb.skew <= skew_before + 0.075, "{at}: {usb:?}");
            let findings = layout::checks(&after);
            assert!(
                pair_findings(&findings).is_empty(),
                "{at}: {:?}",
                pair_findings(&findings)
            );
            let shoved_pair = after
                .tracks
                .iter()
                .any(|t| plan.shoved.contains(&t.id) && pair.has(t.net));
            if shoved_pair {
                moved += 1;
            }
            if let Some(note) = &plan.note {
                held += 1;
                assert!(note.contains("USB"), "{at}: {note}");
                assert!(!shoved_pair, "{at}: held and moved");
            }
            if !plan.reached {
                assert!(plan.blocked.is_some(), "{at}");
            }
        }
    }
    println!("moved {moved}, held {held}");
    assert!(moved > 0, "no step pushed the pair whole");
    assert!(held > 0, "no step was held back");
    Ok(())
}

/// The founder's "Issues says 44, the production check says 50": the skew
/// was in both counts, so gap 2 takes one from each. The difference is the
/// findings only production makes: the two switches' envelope models (no
/// tactile-switch family yet).
#[test]
fn issues_and_the_production_check_count_alike_but_for_production_findings() -> Fallible {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    let engine = pcbcraft_core::Engine::from_file(pcbcraft_io::read_project(&path)?)?;
    let issues = match engine.query(pcbcraft_core::Query::Issues {
        scope: pcbcraft_core::issues::IssueScope::All,
    })? {
        pcbcraft_core::QueryResult::Issues(report) => report,
        _ => return Err("issues".into()),
    };
    let preflight = match engine.query(pcbcraft_core::Query::Preflight { profile: None })? {
        pcbcraft_core::QueryResult::Preflight(report) => report,
        _ => return Err("preflight".into()),
    };
    let count = |code: &str| preflight.items.iter().filter(|i| i.code == code).count();
    assert_eq!(count("pair.skew"), 0);
    assert_eq!(count("fabrication.envelope_model"), 2);
    let only_production: Vec<&str> = preflight
        .items
        .iter()
        .map(|i| i.code.as_str())
        .filter(|c| c.starts_with("fabrication."))
        .collect();
    println!(
        "issues {:?} preflight {} {only_production:?}",
        issues.counts,
        preflight.items.len()
    );
    assert_eq!(issues.rows.len(), 0, "{:?}", issues.counts);
    assert_eq!(preflight.items.len(), 2);
    assert_eq!(only_production.len(), 2);
    Ok(())
}
