//! The board contract (docs/board-rules-and-shape.md): default rules equal the
//! limits the checks used before rules were configurable, the new
//! operations are atomic and undoable, the new fields stay out of existing
//! files, and their schemas are served. Each slice tests its behaviour.
use crate::layout::fill::FillCommand;
use crate::layout::outline::BoardCommand;
use crate::library::{Outline, Segment};
use crate::model::board::{EdgeConstraintSpec, EdgeName};
use crate::model::fill::FillSpec;
use crate::rules::{
    CopperObject, DesignRules, NetClassSpec, RuleKey, RuleValue, RulesCommand, builtin,
};
use crate::{CopperLayer, Engine, Operation, Outcome, Query, QueryResult, format, layout, library};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn default_rules_equal_the_former_hardcoded_limits() {
    let r = DesignRules::default();
    let c = &r.copper.clearance;
    for a in [
        CopperObject::Track,
        CopperObject::Pad,
        CopperObject::Via,
        CopperObject::Fill,
    ] {
        for b in [
            CopperObject::Track,
            CopperObject::Pad,
            CopperObject::Via,
            CopperObject::Fill,
        ] {
            assert_eq!(c.get(a, b), layout::CLEARANCE_MM);
        }
        assert_eq!(c.get(a, CopperObject::Hole), layout::DRILL_TO_COPPER_MM);
        // The one limit that moved on purpose: 0 let fill copper reach
        // 5 µm from the edge and pass (docs/e2e-esp32-devboard.md, gap 6).
        assert_eq!(c.get(a, CopperObject::BoardEdge), 0.3);
    }
    assert_eq!(
        c.get(CopperObject::Hole, CopperObject::Hole),
        layout::HOLE_TO_HOLE_MM
    );
    assert_eq!(r.holes.via_annular_ring_min, layout::MIN_ANNULAR_RING_MM);
    assert_eq!(r.holes.via_drill_default, layout::DEFAULT_VIA_DRILL_MM);
    assert_eq!(
        r.holes.via_diameter_default,
        layout::DEFAULT_VIA_DIAMETER_MM
    );
    assert_eq!(r.mask.expansion, library::MASK_EXPANSION_MM);
    assert_eq!(r.mask.web_min, library::MIN_MASK_WEB_MM);
    assert_eq!(r.silkscreen.line_width_min, library::MIN_SILK_WIDTH_MM);
    assert_eq!(
        r.silkscreen.text_height_min,
        library::MIN_SILK_TEXT_HEIGHT_MM
    );
    assert_eq!(
        r.land.exposed_pad_clearance,
        library::samples::EXPOSED_PAD_CLEARANCE_MM
    );
    assert_eq!(builtin::default_profile().rules, r);
}

#[test]
fn existing_documents_do_not_mention_the_board_fields() -> Fallible<()> {
    // The showcase carries pours since the fill views (task fillview), so
    // it names `fills`; it still names no other board field, and a
    // document without fills does not name `fills` either.
    let text = serde_json::to_string(&Engine::showcase()?.to_file())?;
    for key in [
        "\"rules\"",
        "\"board_shape\"",
        "\"edge_constraints\"",
        "\"keepouts\"",
    ] {
        assert!(!text.contains(key), "{key}");
    }
    assert!(text.contains("\"fills\""));
    let mut engine = Engine::showcase()?;
    let mut project = engine.project().clone();
    project.fills.clear();
    let mut file = engine.to_file();
    file.project = project;
    engine = Engine::from_file(file)?;
    let text = serde_json::to_string(&engine.to_file())?;
    assert!(!text.contains("\"fills\""));
    Ok(())
}

fn square(size: f64) -> Outline {
    Outline {
        start: [1.0, 1.0],
        segments: vec![
            Segment::Line { to: [size, 1.0] },
            Segment::Line { to: [size, size] },
            Segment::Line { to: [1.0, size] },
            Segment::Line { to: [1.0, 1.0] },
        ],
    }
}

#[test]
fn board_operations_are_atomic_undoable_and_persist() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let rev = engine.revision();
    let before = engine.to_file();
    // Refused mutations change nothing.
    let bad = engine.apply(
        rev,
        Operation::Rules(RulesCommand::SetOverride {
            key: RuleKey::TrackWidthMin,
            value: RuleValue::Length(-1.0),
        }),
    );
    assert!(bad.is_err());
    assert_eq!(engine.to_file(), before);
    assert_eq!(engine.revision(), rev);

    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::SetProfile {
            profile: Box::new(builtin::fine()),
            keep_overrides: false,
        }),
    )?;
    let Outcome::NetClassAdded { class } = engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "Power".into(),
                track_width: Some(0.5),
                ..Default::default()
            },
        }),
    )?
    else {
        return Err("no class".into());
    };
    let net = engine
        .project()
        .nets
        .first()
        .map(|n| n.id)
        .ok_or("no net")?;
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AssignNets {
            nets: vec![net],
            class: Some(class),
        }),
    )?;
    let Outcome::FillAdded { fill } = engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Add {
            fill: Box::new(FillSpec {
                name: "GND".into(),
                net: Some(net),
                layer: CopperLayer::Bottom,
                outline: square(20.0),
                priority: 0,
                settings: Default::default(),
            }),
        }),
    )?
    else {
        return Err("no fill".into());
    };
    let Outcome::EdgeConstraintAdded { .. } = engine.apply(
        engine.revision(),
        Operation::Board(BoardCommand::Constrain {
            constraint: EdgeConstraintSpec {
                target: crate::model::board::ConstraintTarget::Fill(fill),
                edge: EdgeName("left".into()),
                anchor: Default::default(),
                along: crate::library::footprint_source::Expr("2 mm".into()),
                offset: crate::library::footprint_source::Expr("1 mm".into()),
                align: Default::default(),
                follow_edge: false,
                rotation_deg: 0.0,
            },
        }),
    )?
    else {
        return Err("no constraint".into());
    };
    // Refilling computes and stores the copper as one undo step (the
    // contract refused it before the fill computation existed).
    let rev = engine.revision();
    engine.apply(
        rev,
        Operation::Fill(FillCommand::Refill {
            target: crate::layout::fill::RefillTarget::All,
        }),
    )?;
    assert_eq!(engine.revision(), rev + 1);
    assert!(
        engine
            .project()
            .fill(fill)
            .is_some_and(|f| f.computed.is_some())
    );
    // Refilling a missing fill is refused and leaves the document.
    let rev = engine.revision();
    let missing = engine.apply(
        rev,
        Operation::Fill(FillCommand::Refill {
            target: crate::layout::fill::RefillTarget::One(u64::MAX),
        }),
    );
    assert!(missing.is_err());
    assert_eq!(engine.revision(), rev);

    let QueryResult::Rules(report) = engine.query(Query::RulesEffective {})? else {
        return Err("rules".into());
    };
    assert_eq!(report.profile_name, "Fine 4/4");
    let QueryResult::BoardShape(board) = engine.query(Query::BoardShape {})? else {
        return Err("board".into());
    };
    assert_eq!(board.edges.len(), 4);
    let QueryResult::ProfileImpact(impact) = engine.query(Query::PreviewProfile {
        profile: Box::new(builtin::default_profile()),
        keep_overrides: false,
    })?
    else {
        return Err("impact".into());
    };
    assert!(!impact.changes.is_empty());

    let reopened = format::decode(&format::encode(&engine.to_file())?)?;
    assert_eq!(reopened, engine.to_file());
    // Profile, class, assignment, fill, constraint and the refill.
    for _ in 0..6 {
        engine.undo()?;
    }
    assert_eq!(engine.to_file(), before);
    Ok(())
}

#[test]
fn board_schemas_are_served() {
    let text = crate::schemas().to_string();
    for name in [
        "rules.set_profile",
        "rules.set_override",
        "net_class.assign",
        "fill.add",
        "fill.store",
        "fill.store_all",
        "keepout.add",
        "board.edit",
        "board.constrain",
        "rules.preview_profile",
        "board.shape",
    ] {
        assert!(text.contains(name), "{name}");
    }
}
