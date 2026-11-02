//! D4 marking behaviour: layout units over plain inputs, then the engine
//! surface (`instance.set_marking`, `instance.marking`, publish findings).
use super::codes::{CodeScheme, TANTALUM_TABLE, TableRef};
use super::*;
use crate::diagnostics::Error;
use crate::ids::Revision;
use crate::tests::support::*;
use crate::{Engine, InstanceCommand, Operation, Query, QueryResult};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const VALUE: DefinitionId = DefinitionId(70);
const TOLERANCE: DefinitionId = DefinitionId(71);

fn field_ref(id: DefinitionId) -> FieldRef {
    FieldRef::new(id, Revision::FIRST)
}

fn inputs(area: [f64; 2], value: Option<(&str, f64)>) -> MarkingInputs {
    let mut inputs = MarkingInputs {
        designator: "R1".into(),
        area_mm: area,
        ..Default::default()
    };
    if let Some((text, si)) = value {
        inputs.fields.insert(VALUE, (text.into(), Some(si)));
    }
    inputs
}

fn code(scheme: CodeScheme) -> MarkingSpec {
    MarkingSpec {
        items: vec![MarkingItem::Code {
            scheme,
            field: field_ref(VALUE),
        }],
        ..Default::default()
    }
}

fn auto_resistor() -> CodeScheme {
    CodeScheme::Auto {
        schemes: vec![
            CodeScheme::Eia96,
            CodeScheme::Resistor4Digit,
            CodeScheme::Resistor3Digit,
        ],
    }
}

fn texts(layout: &MarkingLayout) -> Vec<&str> {
    layout
        .elements
        .iter()
        .filter_map(|e| match e {
            LaidElement::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect()
}

fn finding_codes(layout: &MarkingLayout) -> Vec<&str> {
    layout.findings.iter().map(|f| f.code.as_str()).collect()
}

/// Every laid text box lies inside the area minus its margin.
fn inside(layout: &MarkingLayout, area: [f64; 2]) -> bool {
    let margin = AREA_MARGIN * area[0].min(area[1]) - 1e-9;
    layout.elements.iter().all(|e| match e {
        LaidElement::Text(t) => {
            let width = t.text.chars().count() as f64 * TEXT_ADVANCE * t.height_mm;
            t.origin[0] >= -area[0] / 2.0 + margin
                && t.origin[0] + width <= area[0] / 2.0 - margin
                && t.origin[1] >= -area[1] / 2.0 + margin
                && t.origin[1] + t.height_mm <= area[1] / 2.0 - margin
        }
        _ => true,
    })
}

// 0603 resistor top between terminations: 1.6 − 2 × 0.3 by 0.8 mm.
const AREA_0603: [f64; 2] = [1.0, 0.8];
// 0402: 1.0 − 2 × 0.2 by 0.5 mm.
const AREA_0402: [f64; 2] = [0.6, 0.5];

#[test]
fn auto_codes_pick_eia96_then_digits_and_fit_the_area() -> Fallible<()> {
    let spec = code(auto_resistor());
    let ten_k = layout(&spec, &inputs(AREA_0603, Some(("10 kΩ", 10e3))))?;
    assert_eq!(texts(&ten_k), ["01C"]);
    assert!(ten_k.findings.is_empty());
    assert!(inside(&ten_k, AREA_0603));
    // 4.7 kΩ has no EIA-96 code: the 4-digit code is too wide for an 0603
    // at 0.3 mm (4 × 0.6 × 0.3 = 0.72 mm < 0.9 mm fits); it is drawn.
    let four_k7 = layout(&spec, &inputs(AREA_0603, Some(("4.7 kΩ", 4.7e3))))?;
    assert_eq!(texts(&four_k7), ["4701"]);
    // On an 0402 the 4-digit code does not fit (0.72 mm > 0.54 mm) but the
    // 3-digit one does (0.54 mm).
    let small = layout(&spec, &inputs(AREA_0402, Some(("4.7 kΩ", 4.7e3))))?;
    assert_eq!(texts(&small), ["472"]);
    assert!(inside(&small, AREA_0402));
    Ok(())
}

#[test]
fn an_unrepresentable_value_draws_nothing_with_an_info_finding() -> Fallible<()> {
    let layout = layout(
        &code(CodeScheme::Eia96),
        &inputs(AREA_0603, Some(("10.3 kΩ", 10.3e3))),
    )?;
    assert!(layout.elements.is_empty());
    assert_eq!(finding_codes(&layout), ["marking.unrepresentable"]);
    let finding = &layout.findings[0];
    assert_eq!(
        finding.message,
        "R1: No EIA-96 code for 10.3 kΩ; nothing is drawn."
    );
    assert_eq!(finding.blocks, None, "advisory");
    // No value at all is unrepresentable too.
    let unset = super::layout(&code(CodeScheme::Eia96), &inputs(AREA_0603, None))?;
    assert!(unset.elements.is_empty());
    assert_eq!(finding_codes(&unset), ["marking.unrepresentable"]);
    Ok(())
}

#[test]
fn a_code_too_wide_at_the_minimum_height_does_not_fit() -> Fallible<()> {
    // 4.99 kΩ is only 4-digit (or EIA-96) representable; force 4-digit on a
    // tiny area: 4 × 0.6 × 0.3 = 0.72 mm > 0.36 mm.
    let layout = layout(
        &code(CodeScheme::Resistor4Digit),
        &inputs([0.4, 0.4], Some(("4.99 kΩ", 4.99e3))),
    )?;
    assert!(layout.elements.is_empty());
    assert_eq!(finding_codes(&layout), ["marking.does_not_fit"]);
    Ok(())
}

#[test]
fn changing_the_value_changes_the_marking() -> Fallible<()> {
    let spec = code(auto_resistor());
    let a = layout(&spec, &inputs(AREA_0603, Some(("10 kΩ", 10e3))))?;
    let b = layout(&spec, &inputs(AREA_0603, Some(("4.99 kΩ", 4.99e3))))?;
    assert_eq!((texts(&a), texts(&b)), (vec!["01C"], vec!["68B"]));
    Ok(())
}

#[test]
fn text_templates_stack_lines_and_render_variables() -> Fallible<()> {
    let spec = MarkingSpec {
        items: vec![MarkingItem::Text {
            template: Template("{mpn}\n{var:date_code}".into()),
            height_mm: None,
            align: Align::Centre,
        }],
        ..Default::default()
    };
    let area = [3.9, 4.9];
    let mut with = inputs(area, None);
    with.mpn = Some("EX358".into());
    with.vars.insert("date_code".into(), "2611".into());
    let laid = layout(&spec, &with)?;
    assert_eq!(texts(&laid), ["EX358", "2611"]);
    assert!(inside(&laid, area));
    let [LaidElement::Text(first), LaidElement::Text(second)] = laid.elements.as_slice() else {
        return Err("two lines".into());
    };
    assert!(first.origin[1] > second.origin[1], "top to bottom");
    assert_eq!(first.height_mm, second.height_mm, "one auto height");
    assert!(first.height_mm <= layout::MAX_AUTO_TEXT_HEIGHT_MM);
    // An unset variable renders as nothing; the empty line is skipped.
    with.vars.clear();
    assert_eq!(texts(&super::layout(&spec, &with)?), ["EX358"]);
    // Without a Part there is no MPN: nothing, with a finding.
    with.mpn = None;
    let none = super::layout(&spec, &with)?;
    assert!(none.elements.is_empty());
    assert_eq!(finding_codes(&none), ["marking.unrepresentable"]);
    Ok(())
}

#[test]
fn lines_that_do_not_fit_are_dropped_from_the_end() -> Fallible<()> {
    let spec = MarkingSpec {
        items: vec![MarkingItem::Text {
            template: Template("A1\nB2\nC3".into()),
            height_mm: None,
            align: Align::Start,
        }],
        ..Default::default()
    };
    // 0.9 mm tall: two lines of 0.3 mm plus a 0.15 mm gap fit in 0.81 mm,
    // three do not.
    let laid = layout(&spec, &inputs([2.0, 0.9], None))?;
    assert_eq!(texts(&laid), ["A1", "B2"]);
    assert_eq!(finding_codes(&laid), ["marking.does_not_fit"]);
    assert!(inside(&laid, [2.0, 0.9]));
    Ok(())
}

#[test]
fn polarity_bands_take_room_from_the_text() -> Fallible<()> {
    let spec = MarkingSpec {
        items: vec![
            MarkingItem::Polarity {
                mark: PolarityMark::Band,
                at: Anchor::Start,
            },
            MarkingItem::Code {
                scheme: CodeScheme::CapacitorPf3Digit,
                field: field_ref(VALUE),
            },
            MarkingItem::Code {
                scheme: CodeScheme::VoltageLetter {
                    table: TableRef(TANTALUM_TABLE.into()),
                },
                field: field_ref(TOLERANCE),
            },
        ],
        finish: Finish::InkWhite,
        ..Default::default()
    };
    // A 3216 tantalum top: 3.2 − 2 × 0.5 by 1.6 mm.
    let area = [2.2, 1.6];
    let mut values = inputs(area, Some(("10 µF", 10e-6)));
    values.fields.insert(TOLERANCE, ("16 V".into(), Some(16.0)));
    let laid = layout(&spec, &values)?;
    assert_eq!(laid.finish, Finish::InkWhite);
    assert_eq!(texts(&laid), ["106", "C"]);
    let Some(LaidElement::Polarity {
        origin, size_mm, ..
    }) = laid.elements.first()
    else {
        return Err("band first".into());
    };
    let band_end = origin[0] + size_mm[0] / 2.0;
    assert!(
        (origin[0] - size_mm[0] / 2.0 + area[0] / 2.0).abs() < 1e-12,
        "on the edge"
    );
    for element in &laid.elements {
        if let LaidElement::Text(t) = element {
            assert!(t.origin[0] > band_end, "text clear of the band");
        }
    }
    Ok(())
}

#[test]
fn colour_bands_and_artwork() -> Fallible<()> {
    let spec = MarkingSpec {
        items: vec![
            MarkingItem::Bands {
                value: field_ref(VALUE),
                tolerance: Some(field_ref(TOLERANCE)),
            },
            MarkingItem::Artwork {
                svg: ContentHash([7; 32]),
                size_mm: [1.0, 0.5],
                anchor: Anchor::Pin1Corner,
                licence: None,
            },
        ],
        ..Default::default()
    };
    let area = [6.0, 2.3];
    let mut values = inputs(area, Some(("4.7 kΩ", 4.7e3)));
    values.fields.insert(TOLERANCE, ("±5%".into(), Some(0.05)));
    let laid = layout(&spec, &values)?;
    assert!(laid.findings.is_empty(), "{:?}", laid.findings);
    assert!(matches!(
        laid.elements.as_slice(),
        [LaidElement::Bands { colours, .. }, LaidElement::Artwork { .. }]
            if colours == &vec![4, 7, 2, codes::BAND_GOLD]
    ));
    // A ±3 % tolerance has no colour: nothing, with a finding.
    values.fields.insert(TOLERANCE, ("±3%".into(), Some(0.03)));
    let laid = layout(&spec, &values)?;
    assert_eq!(laid.elements.len(), 1, "only the artwork");
    assert_eq!(finding_codes(&laid), ["marking.unrepresentable"]);
    // Artwork larger than the area does not fit.
    let big = MarkingSpec {
        items: vec![MarkingItem::Artwork {
            svg: ContentHash([7; 32]),
            size_mm: [9.0, 0.5],
            anchor: Anchor::Centre,
            licence: None,
        }],
        ..Default::default()
    };
    assert_eq!(
        finding_codes(&layout(&big, &values)?),
        ["marking.does_not_fit"]
    );
    Ok(())
}

#[test]
fn no_area_draws_nothing_and_non_finite_sizes_are_errors() -> Fallible<()> {
    let spec = code(auto_resistor());
    let laid = layout(&spec, &inputs([0.0, 0.0], Some(("10 kΩ", 10e3))))?;
    assert!(laid.elements.is_empty());
    assert_eq!(finding_codes(&laid), ["marking.unrepresentable"]);
    assert!(layout(&spec, &inputs([f64::NAN, 1.0], None)).is_err());
    let empty = layout(&MarkingSpec::default(), &inputs(AREA_0603, None))?;
    assert_eq!(empty, MarkingLayout::default());
    Ok(())
}

#[test]
fn spec_findings_check_templates_schemes_and_sizes() {
    let owner = DefinitionId(1);
    let good = MarkingSpec {
        items: vec![
            MarkingItem::Text {
                template: Template("{mpn} {field:D70|code3}".into()),
                height_mm: Some(0.5),
                align: Align::Centre,
            },
            MarkingItem::Code {
                scheme: auto_resistor(),
                field: field_ref(VALUE),
            },
        ],
        ..Default::default()
    };
    assert!(spec_findings(owner, &good).is_empty());
    let bad = MarkingSpec {
        items: vec![
            MarkingItem::Text {
                template: Template("{nope}".into()),
                height_mm: Some(-1.0),
                align: Align::Centre,
            },
            MarkingItem::Code {
                scheme: CodeScheme::Auto {
                    schemes: vec![auto_resistor()],
                },
                field: field_ref(VALUE),
            },
            MarkingItem::Code {
                scheme: CodeScheme::VoltageLetter {
                    table: TableRef("vendor".into()),
                },
                field: field_ref(VALUE),
            },
            MarkingItem::Artwork {
                svg: ContentHash([0; 32]),
                size_mm: [0.0, 1.0],
                anchor: Anchor::Centre,
                licence: None,
            },
        ],
        area: AreaChoice::Explicit {
            centre: [0.0, f64::NAN],
            size_mm: [1.0, 1.0],
        },
        min_text_height_mm: Some(0.0),
        ..Default::default()
    };
    let findings = spec_findings(owner, &bad);
    assert_eq!(findings.len(), 7, "{findings:#?}");
    assert!(findings.iter().all(|f| f.code == "marking.template"
        && f.blocks_stage(Stage::Publish)
        && f.objects == vec![ObjectRef::Definition { id: owner }]));
}

#[test]
fn precedence_is_instance_then_part_then_component() {
    let a = code(CodeScheme::Eia96);
    let b = code(CodeScheme::Resistor3Digit);
    let c = code(CodeScheme::Resistor4Digit);
    let mut instance = InstanceMarking::default();
    assert_eq!(effective(&instance, Some(&b), Some(&c)), Some(&b));
    assert_eq!(effective(&instance, None, Some(&c)), Some(&c));
    instance.replace = Some(a.clone());
    assert_eq!(effective(&instance, Some(&b), Some(&c)), Some(&a));
    instance.hide = true;
    assert_eq!(effective(&instance, Some(&b), Some(&c)), None);
}

// ---- Engine surface -------------------------------------------------

struct Board {
    engine: Engine,
    instance: crate::ids::Id,
    value: FieldRef,
    fixture: Fixture,
}

/// A generic resistor (Generic package, 4 × 3 mm envelope) with a
/// resistance field and the auto resistor code as its marking, placed once
/// with `value`.
fn board(value: &str) -> Fallible<Board> {
    let mut engine = Engine::default();
    let kind = publish_kind(
        &mut engine,
        "Resistance",
        crate::fields::QuantityKind {
            units: crate::quantity::UnitSet {
                dimension: [2, 1, -3, -2, 0, 0, 0],
                units: vec![crate::quantity::UnitOfMeasure {
                    symbol: "Ω".into(),
                    aliases: vec!["ohm".into()],
                    scale: 1.0,
                    si_prefixes: true,
                }],
            },
            default_unit: "Ω".into(),
        },
    )?;
    let field = publish_field(
        &mut engine,
        "Resistance",
        crate::fields::FieldDefinition {
            value_type: crate::fields::ValueType::Quantity {
                kind,
                min: Some(0.0),
                max: None,
            },
            ..Default::default()
        },
    )?;
    let spec = probe("Resistor", passive());
    let fixture = draft(&mut engine, &spec)?;
    with(&mut engine, fixture.component, component_body, |c| {
        c.fields = vec![select(field)];
        c.marking = Some(Box::new(MarkingSpec {
            items: vec![MarkingItem::Code {
                scheme: auto_resistor(),
                field,
            }],
            ..Default::default()
        }));
    })?;
    publish(&mut engine, fixture.all())?;
    let (instance, _) = place(&mut engine, &fixture, "R1", 1, [0.0, 0.0])?;
    set_properties(&mut engine, instance, vec![text(field, value)])?;
    Ok(Board {
        engine,
        instance,
        value: field,
        fixture,
    })
}

fn marking_of(engine: &Engine, instance: crate::ids::Id) -> Fallible<Option<MarkingLayout>> {
    match engine.query(Query::Marking { instance })? {
        QueryResult::Marking(layout) => Ok(layout),
        other => Err(format!("unexpected {other:?}").into()),
    }
}

#[test]
fn the_instance_marking_follows_its_value() -> Fallible<()> {
    let mut b = board("10k")?;
    let laid = marking_of(&b.engine, b.instance)?.ok_or("marked")?;
    assert_eq!(texts(&laid), ["01C"]);
    set_properties(&mut b.engine, b.instance, vec![text(b.value, "10.3k")])?;
    let laid = marking_of(&b.engine, b.instance)?.ok_or("marked")?;
    // 10.3 kΩ: not E96, but 4-digit `1032` fits the 4 × 3 mm envelope top.
    assert_eq!(texts(&laid), ["1032"]);
    set_properties(&mut b.engine, b.instance, vec![text(b.value, "10.375k")])?;
    let laid = marking_of(&b.engine, b.instance)?.ok_or("marked")?;
    assert!(laid.elements.is_empty());
    assert_eq!(finding_codes(&laid), ["marking.unrepresentable"]);
    assert_eq!(
        laid.findings[0].objects,
        vec![ObjectRef::Instance { id: b.instance }]
    );
    Ok(())
}

#[test]
fn set_marking_hides_replaces_and_sets_variables_in_one_step() -> Fallible<()> {
    let mut b = board("10k")?;
    let before = b.engine.revision();
    let marking = InstanceMarking {
        hide: false,
        vars: [("date_code".to_string(), "2611".to_string())].into(),
        replace: Some(MarkingSpec {
            items: vec![MarkingItem::Text {
                template: Template("{designator} {var:date_code}".into()),
                height_mm: None,
                align: Align::Centre,
            }],
            ..Default::default()
        }),
    };
    apply(
        &mut b.engine,
        Operation::Instance(InstanceCommand::SetMarking {
            instance: b.instance,
            marking: marking.clone(),
        }),
    )?;
    assert!(b.engine.revision() > before);
    assert_eq!(
        b.engine.project().instance(b.instance).map(|i| &i.marking),
        Some(&marking)
    );
    let laid = marking_of(&b.engine, b.instance)?.ok_or("marked")?;
    assert_eq!(texts(&laid), ["R1 2611"]);
    // The choice persists through the file.
    let file = serde_json::to_string(&b.engine.to_file())?;
    assert!(file.contains("\"date_code\""));
    b.engine.undo()?;
    assert!(
        b.engine
            .project()
            .instance(b.instance)
            .is_some_and(|i| i.marking.is_default())
    );
    apply(
        &mut b.engine,
        Operation::Instance(InstanceCommand::SetMarking {
            instance: b.instance,
            marking: InstanceMarking {
                hide: true,
                ..Default::default()
            },
        }),
    )?;
    assert_eq!(marking_of(&b.engine, b.instance)?, None);
    Ok(())
}

#[test]
fn set_marking_refuses_bad_input_and_changes_nothing() -> Fallible<()> {
    let mut b = board("10k")?;
    let snapshot = Snapshot::of(&b.engine);
    let bad = [
        InstanceMarking {
            vars: [("Date Code".to_string(), "1".to_string())].into(),
            ..Default::default()
        },
        InstanceMarking {
            vars: (0..9).map(|i| (format!("v{i}"), "x".to_string())).collect(),
            ..Default::default()
        },
        InstanceMarking {
            vars: [("lot".to_string(), "x".repeat(33))].into(),
            ..Default::default()
        },
        InstanceMarking {
            replace: Some(MarkingSpec {
                items: vec![MarkingItem::Text {
                    template: Template("{unclosed".into()),
                    height_mm: None,
                    align: Align::Centre,
                }],
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    for marking in bad {
        let result = b.engine.apply(
            b.engine.revision(),
            Operation::Instance(InstanceCommand::SetMarking {
                instance: b.instance,
                marking,
            }),
        );
        assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
        snapshot.assert_kept(&b.engine, "refused marking");
    }
    let missing = b.engine.apply(
        b.engine.revision(),
        Operation::Instance(InstanceCommand::SetMarking {
            instance: 999_999,
            marking: InstanceMarking::default(),
        }),
    );
    assert!(missing.is_err());
    snapshot.assert_kept(&b.engine, "unknown instance");
    Ok(())
}

#[test]
fn an_unmarked_component_has_no_marking_and_specs_gate_publishing() -> Fallible<()> {
    let mut b = board("10k")?;
    // A new component revision with a malformed marking does not publish
    // (the published draft stays open for the next revision).
    with(&mut b.engine, b.fixture.component, component_body, |c| {
        c.marking = Some(Box::new(MarkingSpec {
            items: vec![MarkingItem::Text {
                template: Template("{field:x}".into()),
                height_mm: None,
                align: Align::Centre,
            }],
            ..Default::default()
        }));
    })?;
    assert!(publish(&mut b.engine, vec![b.fixture.component]).is_err());
    // The probe fixture without a marking is unmarked.
    let mut engine = Engine::default();
    let fixture = component(&mut engine, &probe("Plain", passive()))?;
    let (instance, _) = place(&mut engine, &fixture, "U1", 1, [0.0, 0.0])?;
    assert_eq!(marking_of(&engine, instance)?, None);
    Ok(())
}

// ---- SVG artwork ----------------------------------------------------

/// Original artwork: a filled square.
const SQUARE_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2mm" height="2mm" viewBox="0 0 2 2"><rect x="0" y="0" width="2" height="2"/></svg>"#;

fn svg_asset(text: &str) -> crate::library::PackedAsset {
    crate::library::PackedAsset {
        hash: crate::library::bundle::asset_hash(text.as_bytes()),
        media_type: ARTWORK_MEDIA_TYPE.into(),
        data: crate::library::bundle::AssetBytes::new(text.as_bytes().to_vec()),
    }
}

fn artwork_item(hash: ContentHash, licence: Option<&str>) -> MarkingItem {
    MarkingItem::Artwork {
        svg: hash,
        size_mm: [1.0, 1.0],
        anchor: Anchor::Centre,
        licence: licence.map(str::to_owned),
    }
}

/// The component draft's `library.edit` with `spec` as its marking.
fn marking_edit(
    engine: &Engine,
    id: DefinitionId,
    spec: MarkingSpec,
) -> Fallible<crate::library::DraftEdit> {
    let draft = engine.drafts().get(id).ok_or("draft")?;
    let mut definition = draft.definition.clone();
    component_body(&mut definition)?.marking = Some(Box::new(spec));
    Ok(crate::library::DraftEdit {
        id,
        meta: draft.meta.clone(),
        definition,
    })
}

fn pack(
    edits: Vec<crate::library::DraftEdit>,
    artwork: Vec<crate::library::PackedAsset>,
) -> Operation {
    Operation::Library(crate::LibraryCommand::PackArtwork { edits, artwork })
}

/// `library.pack_artwork`: the marking edit and the SVG pack as one undo
/// step; publishing needs the licence; the published artwork is part of
/// the snapshot (a file without it is refused), of the instance's layout
/// and of the component's bundle, which carries the SVG to another
/// project.
#[test]
fn svg_artwork_packs_with_its_edit_and_publishes_with_a_licence() -> Fallible<()> {
    let mut b = board("10k")?;
    let asset = svg_asset(SQUARE_SVG);
    let spec = |licence| MarkingSpec {
        items: vec![artwork_item(asset.hash, licence)],
        ..Default::default()
    };
    let component = b.fixture.component;
    let before = b.engine.to_file();
    let revision = b.engine.revision();
    // Refusals change nothing.
    let mut not_svg = asset.clone();
    not_svg.media_type = "image/png".into();
    let other = svg_asset("<svg xmlns=\"http://www.w3.org/2000/svg\"/>");
    let mut binary = svg_asset("x");
    binary.data = crate::library::bundle::AssetBytes::new(vec![0xff, 0xfe]);
    binary.hash = crate::library::bundle::asset_hash(&[0xff, 0xfe]);
    let edit = marking_edit(&b.engine, component, spec(None))?;
    for (artwork, why) in [
        (vec![not_svg], "not SVG"),
        (vec![other], "not named"),
        (vec![binary], "not text"),
        (vec![], "nothing staged"),
    ] {
        let attempt = b.engine.apply(revision, pack(vec![edit.clone()], artwork));
        assert!(attempt.is_err(), "{why}");
        assert_eq!(b.engine.to_file(), before, "{why}");
    }
    apply(&mut b.engine, pack(vec![edit.clone()], vec![asset.clone()]))?;
    assert_eq!(b.engine.revision(), revision + 1, "one step");
    assert!(
        b.engine
            .project()
            .library
            .assets
            .iter()
            .any(|a| a.hash == asset.hash && a.media_type == ARTWORK_MEDIA_TYPE)
    );
    b.engine.undo()?;
    assert_eq!(b.engine.to_file(), before, "one undo removes edit and SVG");
    b.engine.redo()?;
    // Without a licence the artwork blocks publishing.
    let refused = publish(&mut b.engine, vec![component]);
    let Err(error) = refused else {
        return Err("published artwork without a licence".into());
    };
    assert!(error.to_string().contains("licence"), "{error}");
    with(&mut b.engine, component, component_body, |c| {
        c.marking = Some(Box::new(spec(Some("CC0-1.0"))));
    })?;
    let published = publish(&mut b.engine, vec![component])?;
    let target = published
        .iter()
        .find(|r| r.id == component)
        .ok_or("component revision")?;
    // The artwork is in the snapshot: a file without the SVG is refused.
    let mut file = b.engine.to_file();
    file.project.library.assets.retain(|a| a.hash != asset.hash);
    let Err(error) = Engine::from_file(file) else {
        return Err("a file without its artwork reopened".into());
    };
    let error = error.to_string();
    assert!(
        error.contains(&asset.hash.to_string()) && error.contains("not packed"),
        "{error}"
    );
    // The component's bundle carries the SVG, and another project gets it.
    let library = b.engine.project().library.clone();
    let fresh: Vec<crate::ids::SourceLineageId> =
        (1..=32).map(crate::ids::SourceLineageId).collect();
    let plan = crate::library::import::export(
        &library,
        *target,
        crate::ids::LibraryId(7),
        "Shared",
        &fresh,
    )?;
    let record = plan
        .bundle
        .records
        .iter()
        .find(|r| r.kind == crate::ids::DefinitionKind::Component)
        .ok_or("component record")?;
    assert_eq!(record.assets, vec![asset.hash]);
    assert!(plan.bundle.assets.iter().any(|a| a.hash == asset.hash));
    let mut other = crate::library::Library::default();
    let mut next = 1000u64;
    let mut allocate = || {
        next += 1;
        Ok(DefinitionId(next))
    };
    crate::library::import::import(&mut other, &plan.bundle, &mut allocate)?;
    assert!(
        other
            .assets
            .iter()
            .any(|a| a.hash == asset.hash && a.media_type == ARTWORK_MEDIA_TYPE)
    );
    crate::library::validate(&other)?;
    Ok(())
}

/// An SVG that no draft, revision or instance names any more leaves with
/// the command that dropped it, in the same undo step.
#[test]
fn unnamed_artwork_leaves_with_its_draft() -> Fallible<()> {
    let mut b = board("10k")?;
    let asset = svg_asset(SQUARE_SVG);
    let component = b.fixture.component;
    let spec = MarkingSpec {
        items: vec![artwork_item(asset.hash, Some("CC0-1.0"))],
        ..Default::default()
    };
    let edit = marking_edit(&b.engine, component, spec)?;
    apply(&mut b.engine, pack(vec![edit], vec![asset.clone()]))?;
    let packed = |e: &Engine| {
        e.project()
            .library
            .assets
            .iter()
            .any(|a| a.hash == asset.hash)
    };
    assert!(packed(&b.engine));
    // Removing the item from the draft drops the SVG.
    with(&mut b.engine, component, component_body, |c| {
        c.marking = None;
    })?;
    assert!(!packed(&b.engine), "an edit that stops naming it drops it");
    b.engine.undo()?;
    assert!(packed(&b.engine), "undo brings it back");
    apply(
        &mut b.engine,
        Operation::Library(crate::LibraryCommand::DiscardDraft { id: component }),
    )?;
    assert!(!packed(&b.engine), "discarding the draft drops it");
    b.engine.undo()?;
    assert!(packed(&b.engine));
    Ok(())
}

const CAPACITANCE: DefinitionId = DefinitionId(72);
const VOLTAGE: DefinitionId = DefinitionId(73);

fn electrolytic_spec() -> MarkingSpec {
    MarkingSpec {
        items: vec![MarkingItem::Electrolytic {
            capacitance: field_ref(CAPACITANCE),
            voltage: Some(field_ref(VOLTAGE)),
            stripe: true,
        }],
        finish: Finish::Ink {
            colour: [230, 230, 225],
        },
        ..Default::default()
    }
}

fn electrolytic_inputs(chart: Chart, area: [f64; 2]) -> MarkingInputs {
    let mut inputs = MarkingInputs {
        designator: "C1".into(),
        area_mm: area,
        chart,
        ..Default::default()
    };
    inputs
        .fields
        .insert(CAPACITANCE, ("100u".into(), Some(100e-6)));
    inputs.fields.insert(VOLTAGE, ("25".into(), Some(25.0)));
    inputs
}

fn stripe(layout: &MarkingLayout) -> Option<([f64; 2], [f64; 2])> {
    layout.elements.iter().find_map(|e| match e {
        LaidElement::Polarity {
            mark: PolarityMark::Stripe,
            origin,
            size_mm,
        } => Some((*origin, *size_mm)),
        _ => None,
    })
}

/// A radial electrolytic's sleeve, wrapped a full turn from the default
/// seam (the chart centre opposite angle 0): the stripe sits at angle 0,
/// the negative lead's side, which is the chart's ±edge, and the µF and V
/// lines centre opposite it.
#[test]
fn an_electrolytic_marking_follows_its_values_with_the_stripe_on_the_negative_side() -> Fallible<()>
{
    let r = 2.5;
    let w = std::f64::consts::TAU * r;
    let chart = Chart::Wrap {
        radius_mm: r,
        centre: std::f64::consts::PI,
        span: std::f64::consts::TAU,
        along_u: false,
    };
    let laid = layout(&electrolytic_spec(), &electrolytic_inputs(chart, [w, 6.0]))?;
    assert_eq!(texts(&laid), ["100µF", "25V"]);
    let (origin, size) = stripe(&laid).ok_or("stripe")?;
    // Angle 0 is half a turn from the centre: u = πr, the chart's edge.
    assert!((origin[0].abs() - std::f64::consts::PI * r).abs() < 1e-9);
    assert!((size[0] - super::layout::STRIPE_TURN_SHARE * w).abs() < 1e-9);
    assert!((size[1] - 6.0).abs() < 1e-12);
    // The text keeps clear of the stripe round the turn.
    for e in &laid.elements {
        if let LaidElement::Text(t) = e {
            let width = t.text.chars().count() as f64 * TEXT_ADVANCE * t.height_mm;
            let (lo, hi) = (t.origin[0], t.origin[0] + width);
            assert!(hi < w / 2.0 - size[0] / 2.0 && lo > -w / 2.0 + size[0] / 2.0);
        }
    }
    // A new value is a new marking.
    let mut inputs = electrolytic_inputs(chart, [w, 6.0]);
    inputs
        .fields
        .insert(CAPACITANCE, ("4.7u".into(), Some(4.7e-6)));
    inputs.fields.insert(VOLTAGE, ("63".into(), Some(63.0)));
    assert_eq!(
        texts(&layout(&electrolytic_spec(), &inputs)?),
        ["4.7µF", "63V"]
    );
    // A seam turned a quarter round moves the stripe with angle 0.
    let turned = Chart::Wrap {
        radius_mm: r,
        centre: std::f64::consts::FRAC_PI_2,
        span: std::f64::consts::TAU,
        along_u: false,
    };
    let laid = layout(&electrolytic_spec(), &electrolytic_inputs(turned, [w, 6.0]))?;
    let (origin, _) = stripe(&laid).ok_or("stripe")?;
    assert!((origin[0] + std::f64::consts::FRAC_PI_2 * r).abs() < 1e-9);
    // A half wrap facing away from angle 0 cannot show the stripe.
    let away = Chart::Wrap {
        radius_mm: r,
        centre: std::f64::consts::PI,
        span: std::f64::consts::PI,
        along_u: false,
    };

    let laid = layout(
        &electrolytic_spec(),
        &electrolytic_inputs(away, [w / 2.0, 6.0]),
    )?;
    assert!(stripe(&laid).is_none());
    assert!(finding_codes(&laid).contains(&"marking.unrepresentable"));
    // Flat (an SMD can's top): a band at the +x end.
    let laid = layout(
        &electrolytic_spec(),
        &electrolytic_inputs(Chart::Flat, [6.0, 6.0]),
    )?;
    let (origin, _) = stripe(&laid).ok_or("flat stripe")?;
    assert!(origin[0] > 2.0);
    // No capacitance: nothing drawn, said why.
    let mut missing = electrolytic_inputs(chart, [w, 6.0]);
    missing.fields.remove(&CAPACITANCE);
    let laid = layout(&electrolytic_spec(), &missing)?;
    assert!(laid.elements.is_empty());
    assert_eq!(finding_codes(&laid), ["marking.unrepresentable"]);
    Ok(())
}

#[test]
fn a_2d_code_encodes_its_payload_and_keeps_its_quiet_zone_clear_of_text() -> Fallible<()> {
    let spec = MarkingSpec {
        items: vec![
            MarkingItem::Text {
                template: Template("MODEL-3".into()),
                height_mm: None,
                align: Align::Centre,
            },
            MarkingItem::Code2d {
                symbology: Symbology::Qr,
                payload: Template("{designator}:{var:lot}".into()),
                size_mm: 4.0,
                anchor: Anchor::End,
                ecc: Ecc::Medium,
            },
        ],
        ..Default::default()
    };
    let mut inputs = inputs([16.0, 8.0], None);
    inputs.vars.insert("lot".into(), "A7".into());
    let laid = layout(&spec, &inputs)?;
    let (symbol, origin, module) = laid
        .elements
        .iter()
        .find_map(|e| match e {
            LaidElement::Matrix {
                symbol,
                origin,
                module_mm,
            } => Some((symbol.clone(), *origin, *module_mm)),
            _ => None,
        })
        .ok_or("code")?;
    assert_eq!(
        symbol,
        super::matrix::encode(Symbology::Qr, "R1:A7", Ecc::Medium)?
    );
    assert!((module * f64::from(symbol.size) - 4.0).abs() < 1e-12);
    // At the +x end with its quiet zone inside the area.
    let quiet = 4.0 * module;
    assert!((origin[0] + 2.0 + quiet - 8.0).abs() < 1e-9);
    // The text stays left of the zone.
    for e in &laid.elements {
        if let LaidElement::Text(t) = e {
            let width = t.text.chars().count() as f64 * TEXT_ADVANCE * t.height_mm;
            assert!(t.origin[0] + width < origin[0] - 2.0 - quiet + 1e-9);
        }
    }
    // Too big for the area: nothing drawn, said so.
    let mut big = spec.clone();
    if let Some(MarkingItem::Code2d { size_mm, .. }) = big.items.get_mut(1) {
        *size_mm = 7.9;
    }
    let laid = layout(&big, &inputs)?;
    assert!(finding_codes(&laid).contains(&"marking.does_not_fit"));
    // Modules below the smallest drawn: unrepresentable.
    if let Some(MarkingItem::Code2d { size_mm, .. }) = big.items.get_mut(1) {
        *size_mm = 1.0;
    }
    let laid = layout(&big, &inputs)?;
    assert!(finding_codes(&laid).contains(&"marking.unrepresentable"));
    // Publish findings check the payload template and the size.
    let mut bad = spec.clone();
    bad.items[1] = MarkingItem::Code2d {
        symbology: Symbology::Qr,
        payload: Template("{nope}".into()),
        size_mm: 0.0,
        anchor: Anchor::Centre,
        ecc: Ecc::Low,
    };
    assert_eq!(spec_findings(VALUE, &bad).len(), 2);
    assert!(spec_findings(VALUE, &spec).is_empty());
    Ok(())
}

/// Each decal lays out in its own surface's region seen through its
/// placement, and their findings gather on the marking.
#[test]
fn decals_lay_out_on_their_own_surfaces() -> Fallible<()> {
    let text = |t: &str| MarkingItem::Text {
        template: Template(t.into()),
        height_mm: None,
        align: Align::Centre,
    };
    let spec = MarkingSpec {
        items: vec![text("CAN")],
        placement: Placement {
            on: SurfaceRef::Anchor {
                name: "can_top".into(),
            },
            ..Placement::default()
        },
        finish: Finish::LaserEtchMetal,
        decals: vec![MarkingSpec {
            items: vec![text("PCB")],
            placement: Placement {
                on: SurfaceRef::Anchor {
                    name: "pcb_top".into(),
                },
                rotation_deg: 90.0,
                ..Placement::default()
            },
            finish: Finish::InkWhite,
            ..Default::default()
        }],
        ..Default::default()
    };
    let region = |s: &MarkingSpec| match &s.placement.on {
        SurfaceRef::Anchor { name } if name == "can_top" => Some(([20.0, 20.0], Chart::Flat)),
        SurfaceRef::Anchor { name } if name == "pcb_top" => Some(([22.0, 4.0], Chart::Flat)),
        _ => None,
    };
    let laid = layout_with(&spec, &inputs([0.0, 0.0], None), &region)?;
    assert_eq!(texts(&laid), ["CAN"]);
    assert_eq!(laid.finish, Finish::LaserEtchMetal);
    assert_eq!(laid.decals.len(), 1);
    let pcb = &laid.decals[0];
    assert_eq!(texts(pcb), ["PCB"]);
    assert_eq!(pcb.finish, Finish::InkWhite);
    assert_eq!(pcb.placement.rotation_deg, 90.0);
    // Turned a quarter, the 22 × 4 strip lays out 4 wide and 22 tall:
    // three letters fit only small.
    assert!(inside(pcb, [4.0, 22.0]));
    // An unknown anchor lays out in no area, with a finding.
    let lost = MarkingSpec {
        decals: vec![MarkingSpec {
            items: vec![text("X")],
            placement: Placement {
                on: SurfaceRef::Anchor {
                    name: "nowhere".into(),
                },
                ..Placement::default()
            },
            ..Default::default()
        }],
        ..spec.clone()
    };
    let laid = layout_with(&lost, &inputs([0.0, 0.0], None), &region)?;
    assert!(laid.decals[0].elements.is_empty());
    assert_eq!(finding_codes(&laid), ["marking.unrepresentable"]);
    // Bounds: decals do not nest, and there are at most MAX_DECALS.
    let mut nested = spec.clone();
    nested.decals[0].decals.push(MarkingSpec::default());
    assert!(!nested.within_bounds());
    assert!(!spec_findings(VALUE, &nested).is_empty());
    let mut many = spec.clone();
    many.decals = vec![MarkingSpec::default(); MAX_DECALS + 1];
    assert!(!many.within_bounds());
    // The spec round-trips through JSON with its placement and decals.
    let back: MarkingSpec = serde_json::from_str(&serde_json::to_string(&spec)?)?;
    assert_eq!(back, spec);
    Ok(())
}
