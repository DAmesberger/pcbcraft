use super::*;
use crate::library::{CornerMask, PadShape};

fn source() -> FootprintSource {
    FootprintSource {
        evaluator: EvaluatorRef::default(),
        sketch: SketchBlob("{}".into()),
        parameters: Vec::new(),
        pads: Vec::new(),
        arrays: Vec::new(),
        graphics: Vec::new(),
        keys: vec![KeyedPad {
            key: GenerationKey::Array {
                array: ArrayId(1),
                i: 0,
                j: 0,
            },
            pad: PadId(2),
        }],
        overrides: Vec::new(),
        solve: SolveSummary::default(),
    }
}

fn key(i: u16) -> GenerationKey {
    GenerationKey::Array {
        array: ArrayId(1),
        i,
        j: 0,
    }
}

fn evaluated(k: GenerationKey) -> EvaluatedPad {
    EvaluatedPad {
        key: Some(k),
        pad: Pad::smd_rect(PadId(0), PackageTerminalId(1), "1", [0.0, 0.0], [1.0, 1.0]),
    }
}

#[test]
fn commit_keeps_known_keys_and_allocates_new_ones() -> Result<()> {
    let mut footprint = FootprintDefinition {
        next_local: 3,
        source: Some(Box::new(source())),
        ..FootprintDefinition::default()
    };
    let fake = FakeEvaluator {
        evaluation: Evaluation {
            pads: vec![evaluated(key(0)), evaluated(key(1))],
            new_keys: vec![key(1)],
            sketch_out: SketchBlob("{\"solved\":true}".into()),
            ..Evaluation::default()
        },
    };
    let request = EvaluationRequest {
        source: footprint
            .source
            .as_ref()
            .ok_or(Error::NotFound("src".into()))?,
        package: &PackageDefinition::default(),
        package_values: &[],
        previous: &footprint,
    };
    let evaluation = fake.evaluate(request)?;
    let allocated = commit(&mut footprint, evaluation)?;
    assert_eq!(allocated, vec![PadId(3)]);
    assert_eq!(footprint.next_local, 4);
    let ids: Vec<_> = footprint.pads.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![PadId(2), PadId(3)]);
    assert_eq!(
        footprint.source.map(|s| s.sketch),
        Some(SketchBlob("{\"solved\":true}".into()))
    );
    Ok(())
}

#[test]
fn a_blocking_finding_leaves_the_footprint_unchanged() {
    let mut footprint = FootprintDefinition {
        source: Some(Box::new(source())),
        ..FootprintDefinition::default()
    };
    let before = footprint.clone();
    let evaluation = Evaluation {
        pads: vec![evaluated(key(0))],
        findings: vec![crate::library::publish::issue(
            "footprint.solve_failed",
            "does not solve".into(),
            Vec::new(),
        )],
        ..Evaluation::default()
    };
    assert!(commit(&mut footprint, evaluation).is_err());
    assert_eq!(footprint, before);
}

#[test]
fn rotation_is_canonical_per_shape_symmetry() {
    let rect = PadShape::Rect { size: [1.0, 2.0] };
    assert_eq!(rect.canonical_rotation(180.0), 0.0);
    assert_eq!(rect.canonical_rotation(-90.0), 90.0);
    assert_eq!(
        PadShape::Circle { diameter: 1.0 }.canonical_rotation(37.0),
        0.0
    );
    let d_shape = PadShape::RoundRect {
        size: [1.0, 2.0],
        radius_ratio: 0.5,
        max_radius: None,
        corners: CornerMask {
            top_left: true,
            top_right: true,
            bottom_right: false,
            bottom_left: false,
        },
    };
    assert_eq!(d_shape.canonical_rotation(180.0), 180.0);
    assert_eq!(d_shape.canonical_rotation(-90.0), 270.0);
}

#[test]
fn package_values_are_typed_and_prefixed() {
    let package = PackageDefinition {
        body_size: [2.0, 1.25, 0.6],
        ..PackageDefinition::default()
    };
    let values = package_values(&package);
    assert!(values.iter().all(|v| v.name.is_package() && v.name.valid()));
    assert!(values.contains(&PackageValue {
        name: ParamName("pkg_n".into()),
        value: ParamValue::Count(0),
    }));
    assert!(!ParamName("pkg.e".into()).valid());
    assert!(!ParamName("1a".into()).valid());
}

#[test]
fn parameter_names_and_expressions_have_core_syntax() {
    for (name, valid, package) in [
        ("pitch", true, false),
        ("_e2", true, false),
        ("pkg_L", true, true),
        ("2e", false, false),
        ("a-b", false, false),
        ("", false, false),
        ("pkg", true, false),
    ] {
        let n = ParamName(name.into());
        assert_eq!(n.valid(), valid, "{name}");
        assert_eq!(n.is_package(), package, "{name}");
    }
    assert!(!ParamName("x".repeat(MAX_LABEL + 1)).valid());
    assert!(Expr("pkg_e * 2 + 0.1 mm".into()).valid());
    for bad in ["", "   ", "1\n+2", &"1".repeat(MAX_EXPR + 1)] {
        assert!(!Expr(bad.into()).valid(), "{bad:?}");
    }
}

#[test]
fn every_stored_expression_is_bounded() {
    let mut s = source();
    s.graphics.push(GraphicRole {
        id: GraphicId(5),
        layer: DrawingLayer::Courtyard,
        side: Side::Component,
        source: GraphicSource::Derived(DerivedGraphic::CourtyardFromExtents {
            excess: Expr("0.25 mm".into()),
            round: Expr("0.05 mm".into()),
        }),
        width: Expr("0.05 mm".into()),
        fill: Fill::None,
    });
    assert_eq!(s.expressions().len(), 3);
    assert!(s.within_bounds());
    if let Some(GraphicSource::Derived(DerivedGraphic::CourtyardFromExtents { round, .. })) =
        s.graphics.first_mut().map(|g| &mut g.source)
    {
        round.0 = "1".repeat(MAX_EXPR + 1);
    }
    assert!(!s.within_bounds(), "a nested expression is bounded too");
}
