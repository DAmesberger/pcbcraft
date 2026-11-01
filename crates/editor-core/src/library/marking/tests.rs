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
