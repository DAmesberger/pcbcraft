//! Part dimensions without a sketch evaluator: the wire form and bounds of
//! the contract, the reference findings and their no-waiver rule, the
//! refusals that need no solve, and the held refusal's words. The parts
//! solve's tests live in `pcbcraft_solidcraft::board::parts`.
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

/// A dimension must name live objects and at least one part, a driving
/// dimension needs a value and a constraint takes none; each refusal
/// leaves the document unchanged. A valid dimension needs the host's
/// sketch evaluator to solve its part, so without one it is refused too.
#[test]
fn bad_dimensions_and_a_missing_evaluator_are_refused() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let part = engine
        .project()
        .placements
        .first()
        .ok_or("no part")?
        .instance;
    let rev = engine.revision();
    let before = engine.to_file();
    let spec = |d: PartDimension| PartDimensionSpec {
        kind: d.kind,
        refs: d.refs,
        value: d.value,
        reference: d.reference,
    };
    let mut gone = to_right(Id::MAX);
    gone.refs[0] = BoardRef::Part(FeatureRef::Origin {
        target: ConstraintTarget::Placement(Id::MAX),
    });
    let mut fill = to_right(part);
    fill.refs[0] = BoardRef::Part(FeatureRef::Origin {
        target: ConstraintTarget::Fill(part),
    });
    let mut board_only = to_right(part);
    board_only.refs.remove(0);
    let mut no_value = to_right(part);
    no_value.value = None;
    let mut valued = to_right(part);
    valued.kind = PartRelation::Geom(crate::library::footprint_source::GeomConstraint::Coincident);
    let mut housing = to_right(part);
    housing.refs[1] = BoardRef::Housing(crate::model::dimension::HousingRefId(1));
    let mut pad = to_right(part);
    pad.refs[0] = BoardRef::Part(FeatureRef::Pad {
        placement: part,
        pad: PadId(9999),
        at: PadPoint::Centre,
    });
    for bad in [
        gone,
        fill,
        board_only,
        no_value,
        valued,
        housing,
        pad,
        to_right(part),
    ] {
        let refused = engine.apply(
            rev,
            Operation::Dimension(DimensionCommand::Add {
                dimension: spec(bad.clone()),
            }),
        );
        assert!(refused.is_err(), "{bad:?}");
        assert_eq!(engine.revision(), rev);
        assert_eq!(engine.to_file(), before);
    }
    // Editing or deleting a dimension that does not exist is refused.
    for missing in [
        DimensionCommand::Edit {
            dimension: to_right(part),
        },
        DimensionCommand::Delete { id: 900 },
    ] {
        assert!(engine.apply(rev, Operation::Dimension(missing)).is_err());
        assert_eq!(engine.to_file(), before);
    }
    Ok(())
}

/// Lost references stop driving: a part whose only dimension lost its
/// pad is not in any solve, so unrelated edits need no evaluator and the
/// part keeps its pose.
#[test]
fn a_lost_reference_stops_driving() -> Fallible<()> {
    let engine = Engine::demo()?;
    let mut project = engine.project().clone();
    let part = project.placements.first().ok_or("no part")?.instance;
    let mut lost = to_right(part);
    lost.refs[0] = BoardRef::Part(FeatureRef::Pad {
        placement: part,
        pad: PadId(9999),
        at: PadPoint::Centre,
    });
    *project.part_dimensions = vec![lost];
    let before = project.clone();
    project.board_size = [80.0, 40.0];
    follow(&before, &mut project, None)?;
    assert_eq!(project.placements, before.placements);
    Ok(())
}

/// The held refusal reads as a sentence about the part and where its
/// dimensions hold it, in one message line however long the names: where
/// it is held goes first, never the way out.
#[test]
fn the_held_refusal_fits_one_line_and_keeps_the_way_out() {
    let way_out = |verb: &str| format!("; edit or delete one to {verb} it");
    assert_eq!(
        held_message("J1", Some("2 mm from the right edge"), "move"),
        "J1 is held 2 mm from the right edge; edit or delete its dimensions to move it"
    );
    assert_eq!(
        held_message("J1", None, "turn"),
        format!("J1 is held by its dimensions{}", way_out("turn"))
    );
    assert_eq!(edge_words("USB edge"), "the USB edge");
    assert_eq!(edge_words("right"), "the right edge");
    // Two distances to named edges do not fit with a long name: where it
    // is held goes, the name and the way out stay.
    let at = "12.25 mm from the bottom edge and 3.5 mm from the left edge";
    let long = held_message("CONN12", Some(at), "move");
    assert_eq!(
        long,
        format!("CONN12 is held by its dimensions{}", way_out("move"))
    );
    let longer = held_message(
        "CONNECTOR_LONG_NAME_7_WITH_MORE_WORDS_IN_IT",
        Some(at),
        "move",
    );
    assert!(longer.ends_with(&way_out("move")), "{longer}");
    for text in [
        long,
        longer,
        held_message("J1", Some("on the left edge"), "turn"),
    ] {
        assert!(text.chars().count() <= crate::issues::MAX_TITLE, "{text}");
        assert!(crate::issues::style::check(&text).is_empty(), "{text}");
    }
}

/// Demo run 2, finding 4: a 3D model reference reads in a designer's
/// words from where it lies on the part, never by its persistent name.
#[test]
fn model_references_read_as_sides_of_the_part() -> Fallible<()> {
    use crate::model::dimension::{End, ModelEdgeName, ModelEdgeRef};
    let engine = Engine::demo()?;
    let project = engine.project();
    let placement = project.placements.first().ok_or("a placed part")?;
    let part = placement.instance;
    let instance = project.instance(part).ok_or("its instance")?;
    let designator = instance.designator.clone();
    let physical = crate::instances::resolve(&project.library, instance)?
        .physical
        .ok_or("a package")?;
    let package = &physical.package.definition;
    let frame = package.placement.transform();
    let [l, w, h] = package.envelope();
    assert!(
        l >= 1.5 * w,
        "the demo part is long in x: {:?}",
        package.envelope()
    );
    // The envelope's centre and top in the part frame.
    let c = frame.apply([0.0, 0.0, h / 2.0]);
    // The model point a part-frame point comes from: the rigid frame
    // undone.
    let back = |p: [f64; 3]| -> [f64; 3] {
        let d: [f64; 3] = std::array::from_fn(|k| p[k] - frame.translation[k]);
        std::array::from_fn(|j| (0..3).map(|i| frame.rotation[i][j] * d[i]).sum())
    };
    let face = |pick: [f64; 3]| FeatureRef::Face {
        placement: part,
        face: ModelFaceRef {
            body: BodyName("body".into()),
            face: FaceName("F1:+x".into()),
            pick,
        },
    };
    let edge = |pick: [f64; 3]| ModelEdgeRef {
        body: BodyName("body".into()),
        edge: ModelEdgeName("F1:-y|F1:+z".into()),
        pick,
    };
    let (x1, y0) = (c[0] + l / 2.0, c[1] - w / 2.0);
    // The +x end, square to the board: a line along y.
    let end = FeatureGeometry::Line {
        a: [x1, c[1] - w / 2.0],
        b: [x1, c[1] + w / 2.0],
    };
    let words = model_feature_words_with(project, &face(back([x1, c[1], c[2]])), Some(&end));
    assert_eq!(words, format!("{designator} · right end face"));
    // The front side's top edge: a line along x at −y, up high.
    let front = FeatureGeometry::Line {
        a: [c[0] - l / 2.0, y0],
        b: [c[0] + l / 2.0, y0],
    };
    let top = back([c[0], y0, c[2] + h / 2.0]);
    let edge_ref = FeatureRef::Edge {
        placement: part,
        edge: edge(top),
    };
    let words = model_feature_words_with(project, &edge_ref, Some(&front));
    assert_eq!(words, format!("{designator} · top-front edge"));
    let vertex = FeatureRef::Vertex {
        placement: part,
        edge: edge(top),
        end: End::Start,
    };
    let words = model_feature_words_with(project, &vertex, Some(&front));
    assert_eq!(words, format!("{designator} · start of the top-front edge"));
    // A vertical edge at the front-left corner traces a point.
    let corner = FeatureGeometry::Point {
        at: [c[0] - l / 2.0, y0],
    };
    let upright = FeatureRef::Edge {
        placement: part,
        edge: edge(back([c[0] - l / 2.0, y0, c[2]])),
    };
    let words = model_feature_words_with(project, &upright, Some(&corner));
    assert_eq!(words, format!("{designator} · front-left edge"));
    Ok(())
}
