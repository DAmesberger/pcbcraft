//! Pure parts of the family templates; builds with the real evaluator are
//! in `crates/solidcraft-integration/tests/footprint_templates.rs`.
use crate::tests::support::{pt};
use super::*;
use crate::library::PackageTerminal;
use crate::library::footprint_source::FakeEvaluator;
use crate::tests::support::nm;

fn package(n: u32, body: [f64; 3]) -> PackageDefinition {
    PackageDefinition {
        envelope_nm: body.map(nm),
        terminals: (1..=n)
            .map(|k| PackageTerminal {
                id: crate::ids::PackageTerminalId(k),
                designation: k.to_string(),
            })
            .collect(),
        ..PackageDefinition::default()
    }
}

#[test]
fn defaults_derive_counts_and_sizes_from_the_package() -> Result<()> {
    let qfn = FootprintTemplate::defaults(TemplateKind::Qfn, &package(17, [4.0, 4.0, 0.9]));
    let FootprintTemplate::Qfn(t) = qfn else {
        return Err(Error::Invalid(format!("{qfn:?}")));
    };
    assert_eq!(t.nx, e("(pkg_n - 1) / 4"));
    assert_eq!(t.span_x, e("pkg_L + 2 * 0.3 - pad_l"));
    assert!(t.exposed.is_some());
    // 18 terminals do not make four equal sides: literal counts.
    let FootprintTemplate::Qfn(t) =
        FootprintTemplate::defaults(TemplateKind::Qfn, &package(18, [4.0, 4.0, 0.9]))
    else {
        return Err(Error::Invalid("qfn".into()));
    };
    assert_eq!(t.nx, e("5"));
    let FootprintTemplate::GullWing(t) =
        FootprintTemplate::defaults(TemplateKind::GullWing, &package(7, [4.9, 3.9, 1.5]))
    else {
        return Err(Error::Invalid("gull-wing".into()));
    };
    assert_eq!(t.per_row, e("4"));
    let FootprintTemplate::Chip(t) =
        FootprintTemplate::defaults(TemplateKind::Chip, &PackageDefinition::default())
    else {
        return Err(Error::Invalid("chip".into()));
    };
    assert_eq!((t.pad_w, t.gap), (e("0.8"), e("0.4")));
    Ok(())
}

#[test]
fn derived_roles_are_explicit_courtyard_and_assembly_only() {
    let roles = derived_roles();
    let layers: Vec<DrawingLayer> = roles
        .iter()
        .filter_map(|r| match r {
            FootprintEdit::SetGraphicRole { role } => Some(role.layer),
            _ => None,
        })
        .collect();
    assert_eq!(
        layers,
        vec![DrawingLayer::Courtyard, DrawingLayer::Assembly]
    );
    assert!(roles.iter().any(|r| matches!(r,
        FootprintEdit::SetGraphicRole { role: GraphicRole {
            source: GraphicSource::Derived(DerivedGraphic::CourtyardFromExtents { excess, .. }), ..
        } } if excess.0 == "courtyard_excess")));
}

#[test]
fn locate_binds_distances_or_aligns_with_the_reference() {
    let p = SketchPointId("p9".into());
    let edits = locate(origin(), &p, Some("seed_x"), None);
    assert!(matches!(&edits[..], [
        FootprintEdit::Sketch(SketchEdit::AddDimension { kind: DimKind::HorizontalDistance, value: DimValue::Param(n), .. }),
        FootprintEdit::Sketch(SketchEdit::AddConstraint { kind: GeomConstraint::Horizontal, .. }),
    ] if n.0 == "seed_x"));
    let edits = locate(origin(), &p, None, Some("seed_y"));
    assert!(matches!(
        &edits[..],
        [
            FootprintEdit::Sketch(SketchEdit::AddConstraint {
                kind: GeomConstraint::Vertical,
                ..
            }),
            FootprintEdit::Sketch(SketchEdit::AddDimension {
                kind: DimKind::VerticalDistance,
                ..
            }),
        ]
    ));
}

#[test]
fn build_refuses_before_any_edit_and_leaves_the_draft_alone() -> Result<()> {
    let pkg = package(2, [1.6, 0.8, 0.45]);
    let fake = FakeEvaluator::default();
    let empty = FootprintDefinition {
        next_local: 1,
        ..FootprintDefinition::default()
    };
    // The fake evaluator cannot draw pads: the build fails as a whole.
    let template = FootprintTemplate::defaults(TemplateKind::Chip, &pkg);
    assert!(build(&empty, &pkg, &template, &fake).is_err());
    let FootprintTemplate::Chip(mut bad) = template else {
        return Err(Error::Invalid("chip".into()));
    };
    bad.gap = e("");
    let refused = build(&empty, &pkg, &FootprintTemplate::Chip(bad), &fake);
    assert!(
        matches!(&refused, Err(Error::Invalid(m)) if m.starts_with("footprint.template_invalid")),
        "{refused:?}"
    );
    let mut used = empty.clone();
    used.pads.push(crate::library::Pad::smd_rect(
        PadId(1),
        crate::ids::PackageTerminalId(1),
        "1",
        pt([0.0, 0.0]),
        pt([1.0, 1.0]),
    ));
    let template = FootprintTemplate::defaults(TemplateKind::Chip, &pkg);
    assert!(matches!(
        build(&used, &pkg, &template, &fake),
        Err(Error::Conflict(_))
    ));
    Ok(())
}
