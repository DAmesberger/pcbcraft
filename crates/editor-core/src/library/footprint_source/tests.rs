use crate::tests::support::{mdeg, pt};
use super::*;
use crate::library::{CornerMask, PadShape};
use crate::tests::support::nm;

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
        datum: None,
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
        pad: Pad::smd_rect(PadId(0), PackageTerminalId(1), "1", pt([0.0, 0.0]), pt([1.0, 1.0])),
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
    let rect = PadShape::Rect { size: pt([1.0, 2.0]) };
    assert_eq!(rect.canonical_rotation(mdeg(180.0)), mdeg(0.0));
    assert_eq!(rect.canonical_rotation(mdeg(-90.0)), mdeg(90.0));
    assert_eq!(
        PadShape::Circle { diameter_nm: nm(1.0) }.canonical_rotation(mdeg(37.0)),
        mdeg(0.0)
    );
    let d_shape = PadShape::RoundRect {
        size: pt([1.0, 2.0]),
        radius_ratio: 0.5,
        max_radius_nm: None,
        corners: CornerMask {
            top_left: true,
            top_right: true,
            bottom_right: false,
            bottom_left: false,
        },
    };
    assert_eq!(d_shape.canonical_rotation(mdeg(180.0)), mdeg(180.0));
    assert_eq!(d_shape.canonical_rotation(mdeg(-90.0)), mdeg(270.0));
}

#[test]
fn package_values_are_typed_and_prefixed() {
    let package = PackageDefinition {
        envelope_nm: [nm(2.0), nm(1.25), nm(0.6)],
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

#[test]
fn a_shrunk_array_retires_its_keys_and_still_passes_the_key_rule() -> Result<()> {
    let mut footprint = FootprintDefinition {
        next_local: 3,
        source: Some(Box::new(source())),
        ..FootprintDefinition::default()
    };
    let grown = Evaluation {
        pads: vec![evaluated(key(0)), evaluated(key(1)), evaluated(key(2))],
        new_keys: vec![key(1), key(2)],
        ..Evaluation::default()
    };
    commit(&mut footprint, grown)?;
    assert_eq!(footprint.source.as_ref().map(|s| s.keys.len()), Some(3));
    let shrunk = Evaluation {
        pads: vec![evaluated(key(0))],
        ..Evaluation::default()
    };
    commit(&mut footprint, shrunk)?;
    let keys: Vec<_> = footprint
        .source
        .as_ref()
        .map(|s| s.keys.iter().map(|k| k.key).collect())
        .unwrap_or_default();
    assert_eq!(keys, vec![key(0)], "retired keys are pruned");
    let mut findings = Vec::new();
    footprint.local_findings(crate::ids::DefinitionId(1), &mut findings);
    assert!(
        !findings
            .iter()
            .any(|f| f.message.starts_with("Array key") && f.message.contains("no stored pad")),
        "{findings:?}"
    );
    // Growing again allocates fresh ids: retired ids never return.
    let regrown = Evaluation {
        pads: vec![evaluated(key(0)), evaluated(key(1))],
        new_keys: vec![key(1)],
        ..Evaluation::default()
    };
    assert_eq!(commit(&mut footprint, regrown)?, vec![PadId(5)]);
    Ok(())
}

#[test]
fn folding_a_turn_keeps_hole_vias_and_paste_windows_in_place() -> Result<()> {
    use crate::geometry::{Shape, Transform2};
    use crate::library::{ApertureId, Hole, HoleShape, PadVia, PasteRule, PasteWindow, ShapeNm};
    let mut pad = Pad::smd_rect(PadId(1), PackageTerminalId(1), "1", pt([1.0, 2.0]), pt([2.0, 1.0]));
    pad.rotation_mdeg = mdeg(180.0);
    pad.hole = Some(Hole {
        shape: HoleShape::Slot {
            width_nm: nm(0.3),
            length_nm: nm(0.8),
            angle_mdeg: mdeg(30.0),
        },
        plated: true,
        offset: pt([0.4, 0.1]),
    });
    pad.vias = vec![PadVia {
        id: ApertureId(1),
        at: pt([-0.5, 0.2]),
        drill_nm: nm(0.2),
    }];
    pad.paste = PasteRule::Windows(vec![PasteWindow {
        id: ApertureId(2),
        shape: crate::tests::support::shape_nm(Shape::Circle {
            center: [0.6, 0.0],
            radius: 0.1,
        }),
    }]);
    let world = |p: &Pad, v: [f64; 2]| {
        Transform2 {
            translate: p.position(),
            rotation_deg: p.rotation_deg(),
            mirror: false,
        }
        .apply(v)
    };
    let before = pad.clone();
    pad.fold_rotation()?;
    assert_eq!(pad.rotation_deg(), 0.0);
    let hole = |p: &Pad| p.hole.map(|h| h.offset).unwrap_or_default();
    assert_eq!(world(&pad, hole(&pad).to_mm_f64()), world(&before, hole(&before).to_mm_f64()));
    assert_eq!(
        world(&pad, pad.vias[0].at.to_mm_f64()),
        world(&before, before.vias[0].at.to_mm_f64())
    );
    assert!(matches!(
        pad.hole.map(|h| h.shape),
        Some(HoleShape::Slot { angle_mdeg, .. }) if angle_mdeg == mdeg(30.0)
    ));
    let centre = |p: &Pad| match &p.paste {
        PasteRule::Windows(w) => match w[0].shape {
            ShapeNm::Circle { center, .. } => center.to_mm_f64(),
            _ => [f64::NAN; 2],
        },
        _ => [f64::NAN; 2],
    };
    assert_eq!(world(&pad, centre(&pad)), world(&before, centre(&before)));
    // A D-shape has no 180° symmetry: nothing folds.
    let mut d = before.clone();
    d.shape = PadShape::RoundRect {
        size: pt([2.0, 1.0]),
        radius_ratio: 0.5,
        max_radius_nm: None,
        corners: CornerMask {
            top_left: true,
            top_right: false,
            bottom_right: false,
            bottom_left: true,
        },
    };
    let unfolded = d.clone();
    d.fold_rotation()?;
    assert_eq!(d, unfolded);
    Ok(())
}
