//! Rules that a tool, check or output used to fix in code now come from
//! the board's fab profile (docs/board-rules-and-shape.md): the via tent
//! default, the via drill floor, the legend stroke, the drill files' hole
//! sizes, the stackup's mask in the fabrication outputs and the pair gap
//! tolerance and floor.
use crate::fabrication::{FabricationOptions, drill, excellon, job};
use crate::layout::{self, BoardDrawing, LayoutCommand, PlacedItem, ViaRequest};
use crate::model::{CopperLayer, Project};
use crate::rules::{
    self, CopperObject, DrillFiles, NetClassSpec, RuleKey, RuleProfile, RuleValue, RulesCommand,
    fixtures,
};
use crate::{Engine, Operation};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn set(project: &mut Project, key: RuleKey, value: RuleValue) -> Fallible<()> {
    rules::apply(&mut project.rules, RulesCommand::SetOverride { key, value })?;
    Ok(())
}

#[test]
fn a_new_via_is_tented_as_the_profile_says() -> Fallible<()> {
    let mut project = Engine::demo()?.project().clone();
    let net = project.nets.first().map(|n| n.id).ok_or("no net")?;
    assert!(layout::class_via(&project, net, [1.0, 1.0]).tented);
    set(&mut project, RuleKey::TentVias, RuleValue::Flag(false))?;
    assert!(!layout::class_via(&project, net, [1.0, 1.0]).tented);
    Ok(())
}

#[test]
fn a_via_drill_is_held_to_the_min_drill_rule_not_a_fixed_floor() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let net = engine
        .project()
        .nets
        .first()
        .map(|n| n.id)
        .ok_or("no net")?;
    for (key, mm) in [
        (RuleKey::DrillMin, 0.05),
        (RuleKey::ViaDrillDefault, 0.08),
        (RuleKey::ViaDiameterDefault, 0.3),
    ] {
        engine.apply(
            engine.revision(),
            Operation::Rules(RulesCommand::SetOverride {
                key,
                value: RuleValue::Length(mm),
            }),
        )?;
    }
    let [w, h] = engine.project().board_size;
    let via = ViaRequest {
        drill: 0.08,
        diameter: 0.3,
        ..ViaRequest::fixture(net, [w - 1.0, h - 1.0])
    };
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::AddVia { via }),
    )?;
    assert!(engine.project().vias.iter().any(|v| v.drill == 0.08));
    Ok(())
}

#[test]
fn legend_strokes_follow_the_silkscreen_rules() -> Fallible<()> {
    let mut project = Engine::demo()?.project().clone();
    let text = layout::placed_graphics(&project)
        .into_iter()
        .find_map(|g| match (g.layer, g.item) {
            (BoardDrawing::Silkscreen(_), PlacedItem::Text(text)) => Some(text),
            _ => None,
        })
        .ok_or("no legend text")?;
    let width =
        |p: &Project| layout::legend::text_strokes(&text, &p.rules.effective().silkscreen).width;
    let share = (text.height / 7.0).max(0.1);
    assert!((width(&project) - share).abs() < 1e-12);
    set(
        &mut project,
        RuleKey::SilkLineWidthMin,
        RuleValue::Length(0.3),
    )?;
    assert_eq!(width(&project), 0.3, "never thinner than the line rule");
    set(
        &mut project,
        RuleKey::SilkTextStrokeShare,
        RuleValue::Fraction(0.5),
    )?;
    assert!((width(&project) - (0.5 * text.height).max(0.3)).abs() < 1e-12);
    Ok(())
}

/// The test prototype as a fabricator that drills plated holes 0.1 mm over
/// their finished size.
fn drilled(allowance: f64) -> Fallible<RuleProfile> {
    let mut p = fixtures::prototype()?;
    p.drill_files = DrillFiles::Drilled {
        pth_allowance: allowance,
        source: "Test fixture.".into(),
    };
    Ok(p)
}

/// Every `TnCd` tool line of a drill file: (tool, diameter).
fn tools(bytes: &[u8]) -> Vec<(String, f64)> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|l| {
            let (tool, d) = l.strip_prefix('T')?.split_once('C')?;
            Some((tool.to_owned(), d.parse().ok()?))
        })
        .collect()
}

#[test]
fn drill_files_list_tool_sizes_when_the_fabricator_drills_oversize() -> Fallible<()> {
    let mut project = Engine::showcase()?.project().clone();
    let options = FabricationOptions::default();
    let finished = excellon::files(&project, &options, "b")?;
    rules::apply(
        &mut project.rules,
        RulesCommand::SetProfile {
            profile: Box::new(drilled(0.1)?),
            keep_overrides: true,
        },
    )?;
    let oversize = excellon::files(&project, &options, "b")?;
    let table = drill::table(&drill::holes(&project));
    let mut compared = 0;
    for (a, b) in finished.iter().zip(&oversize) {
        let plated = a.name.ends_with("-plated.drl");
        for ((tool, d), (_, e)) in tools(&a.bytes).iter().zip(tools(&b.bytes)) {
            let want = if plated { d + 0.1 } else { *d };
            assert!((e - want).abs() < 1e-9, "{} T{tool}: {d} → {e}", a.name);
            compared += 1;
        }
        if a.name.ends_with(".drl") {
            let text = String::from_utf8_lossy(&b.bytes);
            assert!(
                text.contains("plated holes at finished size + 0.1 mm"),
                "{text}"
            );
        }
    }
    assert_eq!(compared, table.tools.len(), "every tool");
    assert!(table.plated().count() > 0);
    Ok(())
}

#[test]
fn the_job_file_lists_the_mask_thickness_apart_from_the_board() -> Fallible<()> {
    let project = Engine::showcase()?.project().clone();
    let options = FabricationOptions::default();
    let file = job::file(&project, &options, "b", &[])?;
    let json: serde_json::Value = serde_json::from_slice(&file.bytes)?;
    let masks: Vec<&serde_json::Value> = json["MaterialStackup"]
        .as_array()
        .ok_or("no stackup")?
        .iter()
        .filter(|m| m["Type"] == "SolderMask")
        .collect();
    assert_eq!(masks.len(), 2);
    let um = project.stackup.mask_um / 1000.0;
    assert!(masks.iter().all(|m| {
        m["Thickness"]
            .as_f64()
            .is_some_and(|t| (t - um).abs() < 1e-9)
    }));
    let board = json["GeneralSpecs"]["BoardThickness"]
        .as_f64()
        .ok_or("no thickness")?;
    assert!((board - project.stackup.thickness_mm()).abs() < 1e-3);
    Ok(())
}

#[test]
fn the_pair_gap_tolerance_is_the_pair_class_rule_else_the_profile() -> Fallible<()> {
    let mut project = Engine::demo()?.project().clone();
    let net = project.nets.first().map(|n| n.id).ok_or("no net")?;
    // A tenth of the gap, at least 0.02 mm.
    assert_eq!(layout::length::gap_tolerance(&project, net, 0.1), 0.02);
    assert!((layout::length::gap_tolerance(&project, net, 0.3) - 0.03).abs() < 1e-12);
    set(
        &mut project,
        RuleKey::PairGapTolerance,
        RuleValue::Length(0.04),
    )?;
    assert_eq!(layout::length::gap_tolerance(&project, net, 0.3), 0.04);
    set(
        &mut project,
        RuleKey::PairGapToleranceShare,
        RuleValue::Fraction(0.2),
    )?;
    assert!((layout::length::gap_tolerance(&project, net, 0.3) - 0.06).abs() < 1e-12);
    let class = rules::apply(
        &mut project.rules,
        RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "USB".into(),
                signal: rules::signal::SignalRules {
                    pair_gap_tolerance: Some(0.01),
                    ..Default::default()
                },
                ..Default::default()
            },
        },
    )?
    .ok_or("no class")?;
    rules::apply(
        &mut project.rules,
        RulesCommand::AssignNets {
            nets: vec![net],
            class: Some(class),
        },
    )?;
    assert_eq!(layout::length::gap_tolerance(&project, net, 0.3), 0.01);
    Ok(())
}

#[test]
fn the_tightest_pair_gap_is_the_track_clearance() -> Fallible<()> {
    let mut project = Project {
        stackup: crate::model::Stackup::preset(4)?,
        rules: fixtures::rules()?,
        ..Default::default()
    };
    set(
        &mut project,
        RuleKey::Clearance {
            a: CopperObject::Track,
            b: CopperObject::Track,
        },
        RuleValue::Length(0.03),
    )?;
    let class = rules::apply(
        &mut project.rules,
        RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "USB".into(),
                signal: rules::signal::SignalRules {
                    diff_impedance: Some(90.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        },
    )?
    .ok_or("no class")?;
    let derived = layout::impedance::derive(&project, Some(class), CopperLayer::Top);
    let gap = derived.pair_gap.ok_or("no gap")?;
    assert!((gap - 0.03).abs() < 1e-9, "{gap}");
    Ok(())
}
