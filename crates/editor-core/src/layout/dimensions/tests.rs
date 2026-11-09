//! Part dimensions without a sketch evaluator: the wire form and bounds of
//! the contract (slice C0), and the reference findings and their
//! no-waiver rule (slice S2, ignored until it lands). The parts solve's
//! tests live in `pcbcraft_solidcraft::board::parts`.
use super::*;
use crate::fabrication::FabricationCommand;
use crate::ids::PadId;
use crate::library::footprint_source::{DimKind, Expr, SketchCurveId, SketchRef};
use crate::model::dimension::{
    AnchorName, BodyName, BoxSide, FaceName, ModelFaceRef, PadPoint, ReferenceStatus,
};
use crate::{Engine, Operation};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn to_right(part: Id) -> PartDimension {
    PartDimension {
        id: 900,
        kind: PartRelation::Dim(DimKind::Distance),
        refs: vec![
            BoardRef::Part(FeatureRef::Origin {
                target: ConstraintTarget::Placement(part),
            }),
            BoardRef::Board(SketchRef::Curve(SketchCurveId("l2".into()))),
        ],
        value: Some(DimValue::Fixed(4.0)),
        reference: false,
    }
}

fn with(dimensions: Vec<PartDimension>) -> Project {
    let mut project = Project {
        next_id: 1000,
        ..Project::default()
    };
    *project.part_dimensions = dimensions;
    project
}

/// The wire form agents and files use: externally tagged relations and
/// references, a tagged feature, the value omitted for a constraint.
#[test]
fn a_part_dimension_has_a_stable_wire_form() -> Fallible<()> {
    let json = serde_json::to_value(to_right(5))?;
    assert_eq!(
        json,
        serde_json::json!({
            "id": 900,
            "kind": {"dim": "distance"},
            "refs": [
                {"part": {"feature": "origin", "target": {"kind": "placement", "id": 5}}},
                {"board": {"curve": "l2"}}
            ],
            "value": {"fixed": 4.0}
        })
    );
    let back: PartDimension = serde_json::from_value(json)?;
    assert_eq!(back, to_right(5));
    let pad = BoardRef::Part(FeatureRef::Pad {
        placement: 5,
        pad: PadId(2),
        at: PadPoint::Side(BoxSide::Left),
    });
    assert_eq!(
        serde_json::to_value(&pad)?,
        serde_json::json!({"part": {"feature": "pad", "placement": 5, "pad": 2, "at": {"side": "left"}}})
    );
    let expr = DimValue::Expr(Expr("width / 4".into()));
    assert_eq!(
        serde_json::to_value(&expr)?,
        serde_json::json!({"expr": "width / 4"})
    );
    assert_eq!(
        serde_json::to_value(ReferenceStatus::Inexact)?,
        serde_json::json!("inexact")
    );
    Ok(())
}

/// Structural bounds refuse malformed dimensions; a dangling reference is
/// not malformed (it is a check finding).
#[test]
fn validate_bounds_part_dimensions() {
    assert!(validate(&with(vec![to_right(5)])).is_ok());
    let mut four = to_right(5);
    four.refs = vec![four.refs[0].clone(); 4];
    let mut none = to_right(5);
    none.refs.clear();
    let mut nan = to_right(5);
    nan.value = Some(DimValue::Fixed(f64::NAN));
    let mut along = to_right(5);
    along.kind = PartRelation::PointAlong { at: 1.5 };
    let mut blank = to_right(5);
    blank.refs[0] = BoardRef::Part(FeatureRef::Anchor {
        placement: 5,
        name: AnchorName(" ".into()),
    });
    let mut pick = to_right(5);
    pick.refs[0] = BoardRef::Part(FeatureRef::Face {
        placement: 5,
        face: ModelFaceRef {
            body: BodyName("Shell".into()),
            face: FaceName("F3:side:l2".into()),
            pick: [0.0, f64::INFINITY, 0.0],
        },
    });
    let mut empty_expr = to_right(5);
    empty_expr.value = Some(DimValue::Expr(Expr(String::new())));
    for bad in [four, none, nan, along, blank, pick, empty_expr] {
        assert!(validate(&with(vec![bad.clone()])).is_err(), "{bad:?}");
    }
    assert!(validate(&with(vec![to_right(5), to_right(6)])).is_err());
}

/// A lost or inexact reference cannot be waived for fabrication output.
#[test]
fn reference_findings_cannot_be_waived() -> Fallible<()> {
    for code in [REFERENCE_LOST, REFERENCE_INEXACT] {
        let mut engine = Engine::demo()?;
        let rev = engine.revision();
        let refused = engine.apply(
            rev,
            Operation::Fabrication(FabricationCommand::Waive {
                keys: vec![format!("{code} []")],
                reason: "checked".into(),
            }),
        );
        assert!(refused.is_err(), "{code}");
        assert_eq!(engine.revision(), rev);
    }
    Ok(())
}

/// A dimension naming a pad the part no longer has reports
/// `board.reference_lost` (the part keeps its pose) and blocks
/// fabrication output.
#[test]
#[ignore = "slice S2"]
fn a_lost_pad_reference_is_reported_and_blocks_preflight() -> Fallible<()> {
    let engine = Engine::demo()?;
    let mut project = engine.project().clone();
    let part = project.placements.first().ok_or("no part")?.instance;
    let before = project.placements.first().cloned();
    let mut lost = to_right(part);
    lost.refs[0] = BoardRef::Part(FeatureRef::Pad {
        placement: part,
        pad: PadId(9999),
        at: PadPoint::Centre,
    });
    lost.id = project.allocate()?;
    project.part_dimensions.push(lost);
    let findings = checks(&project);
    assert_eq!(
        findings.iter().filter(|f| f.code == REFERENCE_LOST).count(),
        1,
        "{findings:#?}"
    );
    assert_eq!(project.placements.first().cloned(), before);
    let report = crate::fabrication::preflight::report(&project, &engine.document().drafts, None)?;
    assert!(
        report
            .items
            .iter()
            .any(|i| i.code == REFERENCE_LOST && i.blocks())
    );
    Ok(())
}
