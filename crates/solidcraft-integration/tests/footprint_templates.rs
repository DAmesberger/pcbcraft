//! F5 evidence: the family templates built with the real `SketchEvaluator`
//! through the same steps `library.footprint_from_template` runs. Fixtures
//! are original synthetic packages (round numbers, from no library,
//! datasheet or EDA tool).

use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::geometry::Shape;
use pcbcraft_core::ids::{PackageTerminalId, PadId};
use pcbcraft_core::library::footprint_edit::{self, AssignRule, FootprintEditReport};
use pcbcraft_core::library::footprint_source::{
    Expr, FootprintEdit, NumberingScheme, ParamName, ParamUnit,
};
use pcbcraft_core::library::graphics::{DrawingLayer, Fill, Item};
use pcbcraft_core::library::templates::footprint_family::{
    self, ExposedVias, FootprintTemplate, TemplateKind,
};
use pcbcraft_core::library::{
    FootprintDefinition, PackageDefinition, PackageTerminal, Pad, PadKind, PadShape, PasteRule,
};
use pcbcraft_solidcraft::footprint::SketchEvaluator;

const EV: SketchEvaluator = SketchEvaluator;
const TOL: f64 = 1e-6;

/// A parametric pad 10 mm from the origin takes the untyped package's
/// envelope with it, in the edit's one undo step.
#[test]
fn an_edited_footprint_carries_its_envelope_over_the_pads() -> Result<()> {
    use pcbcraft_core::geometry::Point;
    use pcbcraft_core::library::footprint_source::{PadTemplate, PadTemplateParams};
    use pcbcraft_core::library::{
        Definition, DefinitionMeta, ModelPlacement, PackageForm, PadSide,
    };
    use pcbcraft_core::{Engine, LibraryCommand, Operation, Outcome, PackageRef, Revision};
    let mut engine = Engine::default();
    let meta = |name: &str| DefinitionMeta {
        name: name.into(),
        description: String::new(),
        provenance: "original test fixture".into(),
    };
    let mut new_draft = |definition, name: &str| {
        let revision = engine.revision();
        match engine.apply(
            revision,
            Operation::Library(LibraryCommand::NewDraft {
                meta: meta(name),
                definition,
            }),
        )? {
            Outcome::DraftCreated { id } => Ok::<_, Error>(id),
            other => Err(Error::Invalid(format!("{other:?}"))),
        }
    };
    let pkg = package(1, [4.0, 3.0, 1.0]);
    assert_eq!(pkg.form, PackageForm::Generic);
    let package_id = new_draft(Definition::Package(pkg), "connector")?;
    let footprint = new_draft(
        Definition::Footprint(FootprintDefinition {
            package: PackageRef::new(package_id, Revision::FIRST),
            next_local: 1,
            ..FootprintDefinition::default()
        }),
        "connector-fp",
    )?;
    let placement = |engine: &Engine| match engine.drafts().get(package_id).map(|d| &d.definition) {
        Some(Definition::Package(p)) => p.placement,
        _ => ModelPlacement::default(),
    };
    let at: Point = [10.0, 0.0];
    let revision = engine.revision();
    engine.apply_with(
        revision,
        Operation::Library(LibraryCommand::EditFootprint {
            draft: footprint,
            edits: vec![FootprintEdit::AddPad {
                template: PadTemplate::Rect,
                at,
                params: PadTemplateParams {
                    width: Expr("1".into()),
                    height: Expr("2".into()),
                    radius_ratio: None,
                    kind: PadKind::Smd,
                    side: PadSide::Top,
                    terminal: None,
                    number: "1".into(),
                },
            }],
        }),
        &EV,
    )?;
    assert_eq!(engine.revision(), revision + 1, "one undo step");
    assert_eq!(placement(&engine).translation, [10.0, 0.0, 0.0]);
    engine.undo()?;
    assert_eq!(placement(&engine), ModelPlacement::default());
    Ok(())
}

fn package(terminals: usize, body: [f64; 3]) -> PackageDefinition {
    PackageDefinition {
        envelope: body,
        terminals: (1..=terminals)
            .map(|n| PackageTerminal {
                id: PackageTerminalId(u32::try_from(n).unwrap_or(0) + 100),
                designation: n.to_string(),
            })
            .collect(),
        next_local: u32::try_from(terminals).unwrap_or(0) + 101,
        ..PackageDefinition::default()
    }
}

fn empty() -> FootprintDefinition {
    FootprintDefinition {
        next_local: 1,
        ..FootprintDefinition::default()
    }
}

fn build(
    package: &PackageDefinition,
    template: &FootprintTemplate,
) -> Result<(FootprintDefinition, FootprintEditReport)> {
    footprint_family::build(&empty(), package, template, &EV)
}

fn pad<'a>(def: &'a FootprintDefinition, number: &str) -> Result<&'a Pad> {
    def.pads
        .iter()
        .find(|p| p.number == number)
        .ok_or_else(|| Error::NotFound(format!("no pad {number}")))
}

fn at(def: &FootprintDefinition, number: &str, x: f64, y: f64) -> Result<()> {
    let p = pad(def, number)?.position;
    if (p[0] - x).abs() > TOL || (p[1] - y).abs() > TOL {
        return Err(Error::Invalid(format!(
            "pad {number} at {p:?}, expected [{x}, {y}]"
        )));
    }
    Ok(())
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < TOL
}

/// The single rectangle on `layer`.
fn rect_on(def: &FootprintDefinition, layer: DrawingLayer) -> Result<([f64; 2], [f64; 2])> {
    let shapes: Vec<&Shape> = def
        .graphics
        .iter()
        .filter(|g| g.layer == layer)
        .filter_map(|g| match &g.item {
            Item::Shape { shape, .. } => Some(shape),
            Item::Text(_) => None,
        })
        .collect();
    match shapes.as_slice() {
        [Shape::Rect { a, b, .. }] => Ok((*a, *b)),
        other => Err(Error::Invalid(format!("{layer:?}: {other:?}"))),
    }
}

fn area_of(shape: &Shape) -> f64 {
    pcbcraft_core::geometry::signed_area2(&shape.flatten(1e-4)).abs() / 2.0
}

fn lint(def: &FootprintDefinition) -> Vec<String> {
    def.lint(pcbcraft_core::DefinitionId(1))
        .into_iter()
        .map(|f| f.code)
        .collect()
}

fn set(
    def: &FootprintDefinition,
    pkg: &PackageDefinition,
    name: &str,
    expr: &str,
    unit: ParamUnit,
) -> Result<FootprintDefinition> {
    let (def, _) = footprint_edit::edit(
        def,
        pkg,
        &[FootprintEdit::SetParameter {
            name: ParamName(name.into()),
            expr: Expr(expr.into()),
            unit,
        }],
        &EV,
    )?;
    Ok(def)
}

/// Assign every terminal by designation, then the publish rules (local
/// findings) must pass.
fn publishable(def: &FootprintDefinition, pkg: &PackageDefinition) -> Result<FootprintDefinition> {
    let (def, _) = footprint_edit::assign_terminals(def, pkg, &AssignRule::ByPackageDesignation)?;
    footprint_edit::check_fresh(&def, pkg, &EV)?;
    Ok(def)
}

#[test]
fn chip_pads_follow_their_parameters_with_explicit_courtyard_and_assembly() -> Result<()> {
    // 1.6 × 0.8 mm body: pad_w = pkg_W = 0.8, gap = 1.6 − 2 × 0.3 = 1.0,
    // pad_x = (1.0 + 0.6) / 2 = 0.8.
    let pkg = package(2, [1.6, 0.8, 0.45]);
    let template = FootprintTemplate::defaults(TemplateKind::Chip, &pkg);
    let (def, report) = build(&pkg, &template)?;
    assert_eq!(report.pads.len(), 2);
    assert_eq!(def.pads.len(), 2);
    at(&def, "1", -0.8, 0.0)?;
    at(&def, "2", 0.8, 0.0)?;
    let one = pad(&def, "1")?;
    assert!(
        matches!(one.shape, PadShape::RoundRect { size, radius_ratio, .. }
            if close(size[0], 0.6) && close(size[1], 0.8) && close(radius_ratio, 0.25)),
        "{:?}",
        one.shape
    );
    assert_eq!(one.terminal, None, "terminals are an explicit choice");
    // Courtyard: pads span x ±1.1, y ±0.4; envelope ±0.8 × ±0.4; + 0.25.
    let (a, b) = rect_on(&def, DrawingLayer::Courtyard)?;
    assert!(close(a[0], -1.35) && close(b[0], 1.35), "{a:?} {b:?}");
    assert!(close(a[1], -0.65) && close(b[1], 0.65), "{a:?} {b:?}");
    let (a, b) = rect_on(&def, DrawingLayer::Assembly)?;
    assert!(close(a[0], -0.8) && close(b[1], 0.4), "{a:?} {b:?}");
    let source = def
        .source
        .as_deref()
        .ok_or_else(|| Error::NotFound("source".into()))?;
    let names: Vec<&str> = source
        .parameters
        .iter()
        .map(|p| p.name.0.as_str())
        .collect();
    for name in ["pad_l", "pad_w", "gap", "pad_x", "courtyard_excess"] {
        assert!(names.contains(&name), "{name}: {names:?}");
    }
    // Both roles are explicit, derived and named.
    assert_eq!(source.graphics.len(), 2);
    assert_eq!(def.polarity, None, "an unpolarised chip has no pin 1");
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");

    // Editing a parameter re-solves positions and the courtyard.
    let wider = set(&def, &pkg, "gap", "1.2 mm", ParamUnit::Mm)?;
    at(&wider, "1", -0.9, 0.0)?;
    at(&wider, "2", 0.9, 0.0)?;
    let (a, _) = rect_on(&wider, DrawingLayer::Courtyard)?;
    assert!(close(a[0], -1.45), "{a:?}");
    let more = set(&wider, &pkg, "courtyard_excess", "0.5 mm", ParamUnit::Mm)?;
    let (a, _) = rect_on(&more, DrawingLayer::Courtyard)?;
    assert!(close(a[0], -1.7), "{a:?}");
    publishable(&more, &pkg)?;
    Ok(())
}

#[test]
fn polarised_chip_marks_pin_one_with_a_silkscreen_dot_clear_of_copper() -> Result<()> {
    let pkg = package(2, [3.5, 2.8, 1.9]);
    let FootprintTemplate::Chip(mut chip) = FootprintTemplate::defaults(TemplateKind::Chip, &pkg)
    else {
        return Err(Error::Invalid("chip defaults".into()));
    };
    chip.polarity = true;
    chip.rr = Expr("0".into());
    let (def, _) = build(&pkg, &FootprintTemplate::Chip(chip))?;
    let one = pad(&def, "1")?;
    assert!(
        matches!(one.shape, PadShape::Rect { .. }),
        "rr 0 is a rectangle"
    );
    let polarity = def
        .polarity
        .as_ref()
        .ok_or_else(|| Error::NotFound("polarity".into()))?;
    assert_eq!(polarity.pad, one.id);
    let [marker] = polarity.markers.as_slice() else {
        return Err(Error::Invalid(format!("{polarity:?}")));
    };
    let dot = def
        .graphics
        .iter()
        .find(|g| g.id == *marker)
        .ok_or_else(|| Error::NotFound("dot".into()))?;
    assert_eq!(dot.layer, DrawingLayer::Silkscreen);
    let Item::Shape {
        shape: Shape::Circle { center, radius },
        fill,
        ..
    } = &dot.item
    else {
        return Err(Error::Invalid(format!("{:?}", dot.item)));
    };
    assert_eq!(*fill, Fill::Solid);
    assert!(close(*radius, 0.15));
    // pin1_dot = pad_l / 2 + 0.4 left of pad 1, on its axis.
    assert!(
        close(center[0], one.position[0] - 0.7) && close(center[1], 0.0),
        "{center:?}"
    );
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");
    Ok(())
}

#[test]
fn soic8_is_a_counter_clockwise_dual_row_with_pin_one_marked() -> Result<()> {
    // Defaults: per_row = pkg_n / 2 = 4, row_span = pkg_W + 1.5 = 5.4,
    // pitch 1.27.
    let pkg = package(8, [4.9, 3.9, 1.5]);
    let template = FootprintTemplate::defaults(TemplateKind::GullWing, &pkg);
    let (def, report) = build(&pkg, &template)?;
    assert_eq!(def.pads.len(), 8);
    assert_eq!(report.arrays.len(), 1);
    // Rows along the 4.9 mm body length (x), pin 1 bottom left.
    at(&def, "1", -1.905, -2.7)?;
    at(&def, "4", 1.905, -2.7)?;
    at(&def, "5", 1.905, 2.7)?;
    at(&def, "8", -1.905, 2.7)?;
    let one = pad(&def, "1")?;
    assert_eq!(def.polarity.as_ref().map(|p| p.pad), Some(one.id));
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");
    // The courtyard wraps the body (x ±2.45) and the outer pad ends
    // (y ±3.475), grown by 0.25 and rounded out to 0.05.
    let (a, b) = rect_on(&def, DrawingLayer::Courtyard)?;
    assert!(close(a[0], -2.7) && close(b[0], 2.7), "{a:?} {b:?}");
    assert!(close(a[1], -3.75) && close(b[1], 3.75), "{a:?} {b:?}");
    publishable(&def, &pkg)?;
    // Growing the row re-solves the seed and keeps the part centred.
    let grown = set(&def, &pkg, "per_row", "7", ParamUnit::Count)?;
    assert_eq!(grown.pads.len(), 14);
    at(&grown, "1", -3.81, -2.7)?;
    at(&grown, "14", -3.81, 2.7)?;
    assert_eq!(pad(&grown, "1")?.id, one.id, "pad identity survives");
    Ok(())
}

#[test]
fn qfn16_has_an_exposed_pad_with_paste_windows_and_vias() -> Result<()> {
    // 16 perimeter pads + EP: nx = ny = (17 − 1) / 4 = 4; span = 4 + 0.6
    // − 0.8 = 3.8; EP 4 − 1.7 = 2.3.
    let pkg = package(17, [4.0, 4.0, 0.9]);
    let FootprintTemplate::Qfn(mut qfn) = FootprintTemplate::defaults(TemplateKind::Qfn, &pkg)
    else {
        return Err(Error::Invalid("qfn defaults".into()));
    };
    let ep = qfn
        .exposed
        .as_mut()
        .ok_or_else(|| Error::NotFound("EP default".into()))?;
    ep.vias = Some(ExposedVias {
        rows: Expr("3".into()),
        cols: Expr("3".into()),
        pitch: Expr("0.7".into()),
        drill: Expr("0.3".into()),
    });
    let (def, _) = build(&pkg, &FootprintTemplate::Qfn(qfn))?;
    assert_eq!(def.pads.len(), 17);
    at(&def, "1", -1.9, 0.75)?;
    at(&def, "4", -1.9, -0.75)?;
    at(&def, "5", -0.75, -1.9)?;
    at(&def, "9", 1.9, -0.75)?;
    at(&def, "16", -0.75, 1.9)?;
    let ep = pad(&def, "EP")?;
    assert_eq!(ep.terminal, None, "the EP terminal is an explicit choice");
    assert!(close(ep.position[0], 0.0) && close(ep.position[1], 0.0));
    assert!(
        matches!(ep.shape, PadShape::Rect { size } if close(size[0], 2.3) && close(size[1], 2.3)),
        "{:?}",
        ep.shape
    );
    let PasteRule::Windows(windows) = &ep.paste else {
        return Err(Error::Invalid(format!("{:?}", ep.paste)));
    };
    assert_eq!(windows.len(), 4);
    let area: f64 = windows.iter().map(|w| area_of(&w.shape)).sum();
    assert!((area - 0.6 * 2.3 * 2.3).abs() < 1e-5, "coverage {area}");
    assert_eq!(ep.vias.len(), 9);
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");
    // The paste windows follow the pad.
    let bigger = set(&def, &pkg, "ep_w", "2.6 mm", ParamUnit::Mm)?;
    let ep = pad(&bigger, "EP")?;
    let PasteRule::Windows(windows) = &ep.paste else {
        return Err(Error::Invalid(format!("{:?}", ep.paste)));
    };
    let area: f64 = windows.iter().map(|w| area_of(&w.shape)).sum();
    assert!((area - 0.6 * 2.6 * 2.3).abs() < 1e-5, "coverage {area}");
    // Terminals: the perimeter by designation, then the EP explicitly.
    let assigned = publishable(&def, &pkg)?;
    let unassigned: Vec<&str> = assigned
        .pads
        .iter()
        .filter(|p| p.terminal.is_none())
        .map(|p| p.number.as_str())
        .collect();
    assert_eq!(unassigned, vec!["EP"]);
    Ok(())
}

/// "Used by" counts every reader, not only dimensions: the quad array's
/// counts and spans and the derived courtyard's excess.
#[test]
fn parameter_readers_include_arrays_and_derived_roles() -> Result<()> {
    use pcbcraft_solidcraft::footprint::view::readers;
    let pkg = package(17, [4.0, 4.0, 0.9]);
    let (def, _) = build(&pkg, &FootprintTemplate::defaults(TemplateKind::Qfn, &pkg))?;
    let source = def
        .source
        .as_deref()
        .ok_or_else(|| Error::NotFound("a sketched QFN".into()))?;
    let of = |name: &str| readers(source, &ParamName(name.into()));
    for name in ["nx", "span_y"] {
        let r = of(name)?;
        assert!(r.roles.contains(&"array"), "{name}: {r:?}");
    }
    let r = of("courtyard_excess")?;
    assert!(r.roles.contains(&"courtyard"), "{r:?}");
    assert!(!r.is_empty());
    Ok(())
}

#[test]
fn dfn8_has_pads_on_two_sides_and_no_windows_when_asked() -> Result<()> {
    let pkg = package(9, [3.0, 3.0, 0.8]);
    let FootprintTemplate::Qfn(mut dfn) = FootprintTemplate::defaults(TemplateKind::Qfn, &pkg)
    else {
        return Err(Error::Invalid("qfn defaults".into()));
    };
    dfn.nx = Expr("0".into());
    dfn.ny = Expr("4".into());
    if let Some(ep) = &mut dfn.exposed {
        ep.paste_grid = [0, 0];
    }
    let (def, _) = build(&pkg, &FootprintTemplate::Qfn(dfn))?;
    assert_eq!(def.pads.len(), 9);
    // span_x = 3 + 0.6 − 0.8 = 2.8.
    at(&def, "1", -1.4, 0.75)?;
    at(&def, "4", -1.4, -0.75)?;
    at(&def, "5", 1.4, -0.75)?;
    at(&def, "8", 1.4, 0.75)?;
    assert_eq!(pad(&def, "EP")?.paste, PasteRule::Inherit);
    Ok(())
}

#[test]
fn qfn_without_side_pads_starts_pin_one_on_the_bottom() -> Result<()> {
    let pkg = package(4, [3.0, 3.0, 0.8]);
    let FootprintTemplate::Qfn(mut qfn) = FootprintTemplate::defaults(TemplateKind::Qfn, &pkg)
    else {
        return Err(Error::Invalid("qfn defaults".into()));
    };
    qfn.nx = Expr("2".into());
    qfn.ny = Expr("0".into());
    qfn.exposed = None;
    let (def, _) = build(&pkg, &FootprintTemplate::Qfn(qfn))?;
    assert_eq!(def.pads.len(), 4);
    // span_y = 2.8; pitch 0.5.
    at(&def, "1", -0.25, -1.4)?;
    at(&def, "2", 0.25, -1.4)?;
    at(&def, "3", 0.25, 1.4)?;
    let one = pad(&def, "1")?;
    assert!(
        matches!(one.shape, PadShape::Rect { size } if close(size[0], 0.25) && close(size[1], 0.8)),
        "{:?}",
        one.shape
    );
    Ok(())
}

#[test]
fn pin_header_is_plated_with_a_rounded_pin_one_override() -> Result<()> {
    let pkg = package(4, [2.54, 10.16, 8.5]);
    let template = FootprintTemplate::defaults(TemplateKind::PinHeader, &pkg);
    let (def, _) = build(&pkg, &template)?;
    assert_eq!(def.pads.len(), 4);
    at(&def, "1", 0.0, 3.81)?;
    at(&def, "4", 0.0, -3.81)?;
    for p in &def.pads {
        assert_eq!(p.kind, PadKind::PlatedThrough);
        let hole = p
            .hole
            .as_ref()
            .ok_or_else(|| Error::NotFound("hole".into()))?;
        assert!(hole.plated);
        assert!(
            matches!(hole.shape, pcbcraft_core::library::HoleShape::Round { diameter } if close(diameter, 1.0)),
            "{:?}",
            hole.shape
        );
    }
    assert!(matches!(pad(&def, "1")?.shape, PadShape::RoundRect { .. }));
    assert!(matches!(pad(&def, "2")?.shape, PadShape::Circle { diameter } if close(diameter, 1.7)));
    assert_eq!(
        def.polarity.as_ref().map(|p| p.pad),
        Some(pad(&def, "1")?.id)
    );
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");
    publishable(&def, &pkg)?;
    let two = set(&def, &pkg, "rows", "2", ParamUnit::Count)?;
    at(&two, "1", -1.27, 3.81)?;
    at(&two, "2", 1.27, 3.81)?;
    Ok(())
}

#[test]
fn two_row_header_numbers_across_and_dip_numbers_counter_clockwise() -> Result<()> {
    let pkg = package(6, [5.08, 7.62, 8.5]);
    let FootprintTemplate::PinHeader(mut header) =
        FootprintTemplate::defaults(TemplateKind::PinHeader, &pkg)
    else {
        return Err(Error::Invalid("header defaults".into()));
    };
    header.rows = Expr("2".into());
    header.per_row = Expr("pkg_n / 2".into());
    let (def, _) = build(&pkg, &FootprintTemplate::PinHeader(header.clone()))?;
    at(&def, "1", -1.27, 2.54)?;
    at(&def, "2", 1.27, 2.54)?;
    at(&def, "6", 1.27, -2.54)?;

    let pkg = package(8, [9.6, 6.4, 3.3]);
    header.per_row = Expr("pkg_n / 2".into());
    header.row_pitch = Expr("7.62".into());
    header.numbering = NumberingScheme::CounterClockwise;
    let (def, _) = build(&pkg, &FootprintTemplate::PinHeader(header.clone()))?;
    at(&def, "1", -3.81, 3.81)?;
    at(&def, "4", -3.81, -3.81)?;
    at(&def, "5", 3.81, -3.81)?;
    at(&def, "8", 3.81, 3.81)?;
    // Counter-clockwise needs two rows.
    header.rows = Expr("3".into());
    assert!(build(&pkg, &FootprintTemplate::PinHeader(header)).is_err());
    Ok(())
}

#[test]
fn templates_refuse_a_used_draft_bad_input_and_change_nothing() -> Result<()> {
    let pkg = package(2, [1.6, 0.8, 0.45]);
    let template = FootprintTemplate::defaults(TemplateKind::Chip, &pkg);
    let (built, _) = build(&pkg, &template)?;
    let again = footprint_family::build(&built, &pkg, &template, &EV);
    assert!(
        matches!(&again, Err(Error::Conflict(m)) if m.starts_with("footprint.template_not_empty")),
        "{again:?}"
    );
    let FootprintTemplate::Chip(mut bad) = template.clone() else {
        return Err(Error::Invalid("chip defaults".into()));
    };
    bad.pad_w = Expr(" ".into());
    assert!(build(&pkg, &FootprintTemplate::Chip(bad.clone())).is_err());
    bad.pad_w = Expr("-1".into());
    assert!(build(&pkg, &FootprintTemplate::Chip(bad)).is_err());
    let FootprintTemplate::Qfn(mut qfn) = FootprintTemplate::defaults(TemplateKind::Qfn, &pkg)
    else {
        return Err(Error::Invalid("qfn defaults".into()));
    };
    if let Some(ep) = &mut qfn.exposed {
        ep.paste_grid = [9, 1];
    }
    assert!(matches!(
        build(&pkg, &FootprintTemplate::Qfn(qfn)),
        Err(Error::Limit(_))
    ));
    Ok(())
}

#[test]
fn defaults_fall_back_to_literals_without_package_values() {
    let bare = PackageDefinition::default();
    for kind in TemplateKind::ALL {
        let t = FootprintTemplate::defaults(kind, &bare);
        assert_eq!(t.kind(), kind);
        let json = serde_json::to_string(&t).unwrap_or_default();
        assert!(!json.contains("pkg_"), "{kind:?}: {json}");
    }
}

#[test]
fn every_default_template_builds_for_a_matching_package() -> Result<()> {
    for (kind, pkg) in [
        (TemplateKind::Chip, package(2, [2.0, 1.25, 0.5])),
        (TemplateKind::GullWing, package(14, [8.65, 3.9, 1.5])),
        (TemplateKind::Qfn, package(25, [5.0, 5.0, 0.9])),
        (TemplateKind::PinHeader, package(10, [2.54, 25.4, 8.5])),
    ] {
        let (def, _) = build(&pkg, &FootprintTemplate::defaults(kind, &pkg))?;
        let expected = pkg.terminals.len();
        assert_eq!(def.pads.len(), expected, "{kind:?}");
        let found = lint(&def);
        assert!(found.is_empty(), "{kind:?}: {found:?}");
        let ids: std::collections::BTreeSet<PadId> = def.pads.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), expected, "{kind:?}: unique pad identities");
    }
    Ok(())
}

/// `library.footprint_from_template` through the engine: one revision and
/// one undo step; a refused template changes nothing; the built draft
/// publishes once its terminals are assigned.
#[test]
fn footprint_from_template_is_one_atomic_undoable_command() -> Result<()> {
    use pcbcraft_core::library::{Definition, DefinitionMeta};
    use pcbcraft_core::{
        DefinitionId, Engine, LibraryCommand, Operation, Outcome, PackageRef, Revision,
    };
    let mut engine = Engine::default();
    let meta = |name: &str| DefinitionMeta {
        name: name.into(),
        description: String::new(),
        provenance: "original test fixture".into(),
    };
    let new_draft = |engine: &mut Engine, definition, name: &str| -> Result<DefinitionId> {
        match engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::NewDraft {
                meta: meta(name),
                definition,
            }),
        )? {
            Outcome::DraftCreated { id } => Ok(id),
            other => Err(Error::Invalid(format!("{other:?}"))),
        }
    };
    let pkg = package(8, [4.9, 3.9, 1.5]);
    let package_id = new_draft(&mut engine, Definition::Package(pkg.clone()), "soic")?;
    let footprint = new_draft(
        &mut engine,
        Definition::Footprint(FootprintDefinition {
            package: PackageRef::new(package_id, Revision::FIRST),
            next_local: 1,
            ..FootprintDefinition::default()
        }),
        "soic-fp",
    )?;
    let before = engine.drafts().clone();
    let revision = engine.revision();
    let op = |template: FootprintTemplate| {
        Operation::Library(LibraryCommand::FootprintFromTemplate {
            draft: footprint,
            template: Box::new(template),
            replace: false,
        })
    };
    // Without the host evaluator, and with a bad form, nothing changes.
    let template = FootprintTemplate::defaults(TemplateKind::GullWing, &pkg);
    assert!(engine.apply(revision, op(template.clone())).is_err());
    let FootprintTemplate::GullWing(mut bad) = template.clone() else {
        return Err(Error::Invalid("gull-wing defaults".into()));
    };
    bad.pad_w = Expr("0".into());
    assert!(
        engine
            .apply_with(revision, op(FootprintTemplate::GullWing(bad)), &EV)
            .is_err()
    );
    assert_eq!(engine.revision(), revision);
    assert_eq!(engine.drafts(), &before);

    let outcome = engine.apply_with(revision, op(template.clone()), &EV)?;
    let Outcome::FootprintEdited { report } = outcome else {
        return Err(Error::Invalid(format!("{outcome:?}")));
    };
    assert_eq!(engine.revision(), revision + 1, "one revision");
    assert_eq!(report.arrays.len(), 1);
    // Building again into the same draft is refused.
    assert!(
        engine
            .apply_with(engine.revision(), op(template), &EV)
            .is_err()
    );
    let built = engine.drafts().clone();
    let rule = |confirm| {
        Operation::Library(LibraryCommand::AssignTerminals {
            draft: footprint,
            rule: AssignRule::ByPackageDesignation,
            confirm,
        })
    };
    engine.apply(engine.revision(), rule(true))?;
    engine.apply_with(
        engine.revision(),
        Operation::Library(LibraryCommand::Publish {
            drafts: vec![package_id, footprint],
            close: false,
        }),
        &EV,
    )?;
    engine.undo()?;
    engine.undo()?;
    assert_eq!(engine.drafts(), &built);
    engine.undo()?;
    assert_eq!(engine.drafts(), &before, "the template is one undo step");
    Ok(())
}

/// `replace: true` clears the draft and builds in its place as one
/// revision and one undo step: fresh pad identities, terminals unassigned,
/// authored graphics and pad defaults kept. A refused replace
/// changes nothing; on an empty draft it equals a plain build.
#[test]
fn footprint_from_template_replace_is_one_atomic_undo_step() -> Result<()> {
    use pcbcraft_core::ids::GraphicId;
    use pcbcraft_core::library::{Definition, DefinitionMeta, designator_placeholder};
    use pcbcraft_core::{
        DefinitionId, Engine, LibraryCommand, Operation, Outcome, PackageRef, Revision,
    };
    let mut engine = Engine::default();
    let meta = |name: &str| DefinitionMeta {
        name: name.into(),
        description: String::new(),
        provenance: "original test fixture".into(),
    };
    let new_draft = |engine: &mut Engine, definition, name: &str| -> Result<DefinitionId> {
        match engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::NewDraft {
                meta: meta(name),
                definition,
            }),
        )? {
            Outcome::DraftCreated { id } => Ok(id),
            other => Err(Error::Invalid(format!("{other:?}"))),
        }
    };
    let pkg = package(8, [4.9, 3.9, 1.5]);
    let package_id = new_draft(&mut engine, Definition::Package(pkg.clone()), "soic")?;
    let mut start = FootprintDefinition {
        package: PackageRef::new(package_id, Revision::FIRST),
        next_local: 2,
        ..FootprintDefinition::default()
    };
    start.defaults.mask_expansion_mm = 0.07;
    start.graphics.push(designator_placeholder(
        GraphicId(1),
        [0.0, 3.0],
        DrawingLayer::Silkscreen,
    ));
    let footprint = new_draft(&mut engine, Definition::Footprint(start.clone()), "soic-fp")?;
    let op = |template: &FootprintTemplate, replace| {
        Operation::Library(LibraryCommand::FootprintFromTemplate {
            draft: footprint,
            template: Box::new(template.clone()),
            replace,
        })
    };
    let fp = |engine: &Engine| -> Result<FootprintDefinition> {
        match engine.drafts().get(footprint).map(|d| &d.definition) {
            Some(Definition::Footprint(f)) => Ok(f.clone()),
            _ => Err(Error::NotFound("footprint draft".into())),
        }
    };

    // On an empty draft, replace is exactly a build.
    let gull = FootprintTemplate::defaults(TemplateKind::GullWing, &pkg);
    let (plain, _) = footprint_family::build(&start, &pkg, &gull, &EV)?;
    let (rebuilt, _) = footprint_family::rebuild(&start, &pkg, &gull, &EV)?;
    assert_eq!(plain, rebuilt);
    engine.apply_with(engine.revision(), op(&gull, true), &EV)?;
    assert_eq!(fp(&engine)?, plain);
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::AssignTerminals {
            draft: footprint,
            rule: AssignRule::ByPackageDesignation,
            confirm: true,
        }),
    )?;
    let assigned = engine.drafts().clone();
    let old = fp(&engine)?;
    assert!(old.pads.iter().all(|p| p.terminal.is_some()));

    // Without replace a used draft is refused; a bad form with replace
    // and a missing evaluator change nothing either.
    let qfn = FootprintTemplate::defaults(TemplateKind::Qfn, &pkg);
    let revision = engine.revision();
    let refused = engine.apply_with(revision, op(&qfn, false), &EV);
    assert!(
        matches!(&refused, Err(Error::Conflict(m)) if m.starts_with("footprint.template_not_empty")),
        "{refused:?}"
    );
    let FootprintTemplate::Qfn(mut bad) = qfn.clone() else {
        return Err(Error::Invalid("qfn defaults".into()));
    };
    bad.pad_w = Expr("0".into());
    assert!(
        engine
            .apply_with(revision, op(&FootprintTemplate::Qfn(bad), true), &EV)
            .is_err()
    );
    assert!(engine.apply(revision, op(&qfn, true)).is_err());
    assert_eq!(engine.revision(), revision);
    assert_eq!(engine.drafts(), &assigned);

    // Replace: one revision, the QFN's pads with fresh identities and no
    // terminals, the authored designator and the footprint settings kept.
    engine.apply_with(revision, op(&qfn, true), &EV)?;
    assert_eq!(engine.revision(), revision + 1, "one revision");
    let new = fp(&engine)?;
    let (expected, _) = footprint_family::build(&footprint_family::cleared(&old), &pkg, &qfn, &EV)?;
    assert_eq!(new, expected);
    assert!(!new.pads.is_empty());
    assert!(new.pads.iter().all(|p| p.terminal.is_none()));
    let old_ids: std::collections::BTreeSet<PadId> = old.pads.iter().map(|p| p.id).collect();
    assert!(
        new.pads.iter().all(|p| !old_ids.contains(&p.id)),
        "fresh pads"
    );
    assert!(new.next_local > old.next_local);
    assert!(new.graphics.iter().any(|g| g.id == GraphicId(1)));
    assert_eq!(new.defaults, start.defaults);
    let polarity = new.polarity.as_ref().map(|p| p.pad);
    assert!(polarity.is_some_and(|pad| new.pads.iter().any(|p| p.id == pad)));
    // Only the new roles' graphics remain besides the authored one.
    let roles: Vec<_> = new
        .source
        .as_deref()
        .map(|s| s.graphics.iter().map(|g| g.id).collect())
        .unwrap_or_default();
    assert!(
        new.graphics
            .iter()
            .all(|g| g.id == GraphicId(1) || roles.contains(&g.id))
    );

    engine.undo()?;
    assert_eq!(engine.drafts(), &assigned, "replace is one undo step");
    engine.redo()?;
    assert_eq!(fp(&engine)?, new);
    Ok(())
}

/// A draft's silkscreen designator that the template's pads would cover
/// moves just above them, off copper.
#[test]
fn a_template_keeps_the_designator_off_its_pads() -> Result<()> {
    use pcbcraft_core::ids::GraphicId;
    use pcbcraft_core::library::designator_placeholder;
    let pkg = package(17, [4.0, 4.0, 0.9]);
    let mut draft = empty();
    draft.graphics.push(designator_placeholder(
        GraphicId(1),
        [-1.0, 1.9],
        DrawingLayer::Silkscreen,
    ));
    draft.next_local = 2;
    let template = FootprintTemplate::defaults(TemplateKind::Qfn, &pkg);
    let (def, _) = footprint_family::build(&draft, &pkg, &template, &EV)?;
    let top = def
        .pads
        .iter()
        .map(|p| p.bounds().max[1])
        .fold(f64::MIN, f64::max);
    let Some(Item::Text(text)) = def.graphics.first().map(|g| &g.item) else {
        return Err(Error::NotFound("the designator".into()));
    };
    assert!(
        text.at[1] - text.height / 2.0 > top,
        "{:?} over {top}",
        text.at
    );
    Ok(())
}
