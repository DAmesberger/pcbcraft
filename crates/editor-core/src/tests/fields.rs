//! Slice 2: typed reusable fields, instance values and their updates.
//! Expected values are hand-derived from the synthetic fixture below.
use super::support::*;
use crate::completeness::CheckTarget;
use crate::fields::{
    Choice, EffectiveField, FieldDefinition, FieldSelection, FieldValue, Origin, PropertyEdit,
    PropertyInput, QuantityKind, ToleranceBasis, ValueType, tolerance_interval,
};
use crate::library::PinKind;
use crate::quantity::{UnitOfMeasure, UnitSet};
use crate::updates::{Change, FieldResolution, Resolutions};
use crate::{
    ChoiceId, ComponentRef, DefinitionKind, DefinitionRef, Engine, Error, FieldRef, Id, ObjectRef,
    QuantityKindRef, Result, Stage, format,
};

const OHMS: crate::quantity::Dimension = [2, 1, -3, -2, 0, 0, 0];
const FARADS: crate::quantity::Dimension = [-2, -1, 4, 2, 0, 0, 0];

fn units(dimension: crate::quantity::Dimension, symbol: &str, aliases: &[&str]) -> QuantityKind {
    QuantityKind {
        units: UnitSet {
            dimension,
            units: vec![UnitOfMeasure {
                symbol: symbol.into(),
                aliases: aliases.iter().map(|a| (*a).into()).collect(),
                scale: 1.0,
                si_prefixes: true,
            }],
        },
        default_unit: symbol.into(),
    }
}

fn field(value_type: ValueType) -> FieldDefinition {
    FieldDefinition {
        next_local: 3,
        value_type,
        default: None,
        required_at: None,
    }
}

struct Fields {
    resistance_kind: QuantityKindRef,
    reactance_kind: QuantityKindRef,
    resistance: FieldRef,
    capacitance: FieldRef,
    tolerance: FieldRef,
    dielectric: FieldRef,
    note: FieldRef,
    count: FieldRef,
    fitted: FieldRef,
}

/// Three quantity kinds (resistance and reactance share a dimension) and
/// one field of each value type.
fn publish_fields(engine: &mut Engine) -> Result<Fields> {
    let resistance_kind = publish_kind(engine, "Resistance", units(OHMS, "Ω", &["ohm"]))?;
    let reactance_kind = publish_kind(engine, "Reactance", units(OHMS, "Ω", &["ohm"]))?;
    let capacitance_kind = publish_kind(engine, "Capacitance", units(FARADS, "F", &[]))?;
    let resistance = publish_field(
        engine,
        "Resistance",
        FieldDefinition {
            required_at: Some(Stage::ElectricallyResolved),
            ..field(ValueType::Quantity {
                kind: resistance_kind,
                min: Some(0.0),
                max: Some(1e9),
            })
        },
    )?;
    let capacitance = publish_field(
        engine,
        "Capacitance",
        field(ValueType::Quantity {
            kind: capacitance_kind,
            min: Some(0.0),
            max: None,
        }),
    )?;
    let tolerance = publish_field(
        engine,
        "Tolerance",
        FieldDefinition {
            default: Some(FieldValue::Tolerance {
                minus: 0.1,
                plus: 0.1,
            }),
            ..field(ValueType::Tolerance {
                basis: ToleranceBasis::Relative,
            })
        },
    )?;
    let dielectric = publish_field(
        engine,
        "Dielectric",
        field(ValueType::Enum {
            choices: vec![
                Choice {
                    id: ChoiceId(1),
                    label: "C0G".into(),
                },
                Choice {
                    id: ChoiceId(2),
                    label: "X7R".into(),
                },
            ],
        }),
    )?;
    let note = publish_field(engine, "Note", field(ValueType::Text { max_bytes: 8 }))?;
    let count = publish_field(
        engine,
        "Count",
        field(ValueType::Integer { min: 0, max: 10 }),
    )?;
    let fitted = publish_field(
        engine,
        "Fitted",
        FieldDefinition {
            default: Some(FieldValue::Boolean(true)),
            ..field(ValueType::Boolean)
        },
    )?;
    Ok(Fields {
        resistance_kind,
        reactance_kind,
        resistance,
        capacitance,
        tolerance,
        dielectric,
        note,
        count,
        fitted,
    })
}

/// The component default for resistance is 1 kΩ.
fn selections(f: &Fields) -> Vec<FieldSelection> {
    let mut all: Vec<FieldSelection> = [
        f.resistance,
        f.capacitance,
        f.tolerance,
        f.dielectric,
        f.note,
        f.count,
        f.fitted,
    ]
    .into_iter()
    .map(select)
    .collect();
    all[0].default = Some(FieldValue::Quantity {
        si: 1000.0,
        display_unit: Some("kΩ".into()),
    });
    all
}

struct Setup {
    engine: Engine,
    fields: Fields,
    part: Fixture,
    r1: Id,
    r2: Id,
}

fn setup() -> Result<Setup> {
    let mut engine = Engine::default();
    let fields = publish_fields(&mut engine)?;
    let part = component_with_fields(
        &mut engine,
        &probe("Part", PinKind::Passive),
        selections(&fields),
    )?;
    let (r1, _) = place(&mut engine, &part, "R1", 1, [0.0, 0.0])?;
    let (r2, _) = place(&mut engine, &part, "R2", 1, [20.0, 0.0])?;
    Ok(Setup {
        engine,
        fields,
        part,
        r1,
        r2,
    })
}

fn effective(engine: &Engine, instance: Id, field: FieldRef) -> Result<EffectiveField> {
    inspect(engine, instance)?
        .properties
        .into_iter()
        .find(|f| f.field.id == field.id)
        .ok_or_else(|| Error::NotFound("field".into()))
}

fn si(field: &EffectiveField) -> Option<f64> {
    match field.value {
        Some(FieldValue::Quantity { si, .. }) => Some(si),
        _ => None,
    }
}

fn input(field: FieldRef, input: PropertyInput) -> PropertyEdit {
    PropertyEdit {
        field: field.id,
        input,
    }
}

#[test]
fn quantity_text_is_parsed_in_core_with_the_documented_precedence() -> Result<()> {
    let Setup {
        mut engine,
        fields: f,
        r1,
        ..
    } = setup()?;
    for (written, unit) in [("10k", "kΩ"), ("10 kΩ", "kΩ"), ("10000 Ω", "Ω")] {
        let returned = set_properties(&mut engine, r1, vec![text(f.resistance, written)])?;
        let resistance = effective(&engine, r1, f.resistance)?;
        assert_eq!(
            returned[0], resistance,
            "the outcome is the effective state"
        );
        assert_eq!(si(&resistance), Some(10000.0), "{written}");
        assert_eq!(resistance.origin, Origin::Assigned);
        assert_eq!(
            resistance.value,
            Some(FieldValue::Quantity {
                si: 10000.0,
                display_unit: Some(unit.into()),
            })
        );
    }
    // A bare prefix applies to the default unit: 10 mΩ.
    set_properties(&mut engine, r1, vec![text(f.resistance, "10m")])?;
    let resistance = effective(&engine, r1, f.resistance)?;
    assert!(si(&resistance).is_some_and(|v| (v - 0.01).abs() < 1e-15));
    assert_eq!(resistance.text.as_deref(), Some("10 mΩ"));

    // A suffix of another kind is rejected and nothing changes.
    set_properties(&mut engine, r1, vec![text(f.resistance, "10k")])?;
    prime_history(&mut engine)?;
    let kept = Snapshot::of(&engine);
    let error = set_properties(&mut engine, r1, vec![text(f.resistance, "10 µF")])
        .err()
        .ok_or_else(|| Error::Invalid("µF accepted".into()))?;
    assert_eq!(error.code(), "invalid_input");
    let message = error.to_string();
    assert!(
        message.contains("µF") && message.contains("Resistance"),
        "{message}"
    );
    kept.assert_kept(&engine, "rejected unit");
    assert_eq!(si(&effective(&engine, r1, f.resistance)?), Some(10000.0));
    Ok(())
}

#[test]
fn assigned_zero_unset_and_inherit_are_distinct() -> Result<()> {
    let Setup {
        mut engine,
        fields: f,
        r1,
        ..
    } = setup()?;
    let resistance = effective(&engine, r1, f.resistance)?;
    assert_eq!(resistance.origin, Origin::ComponentDefault);
    assert_eq!(resistance.text.as_deref(), Some("1 kΩ"));
    assert_eq!(
        effective(&engine, r1, f.tolerance)?.origin,
        Origin::FieldDefault
    );
    assert_eq!(effective(&engine, r1, f.note)?.origin, Origin::NoValue);

    set_properties(&mut engine, r1, vec![text(f.resistance, "0")])?;
    let zero = effective(&engine, r1, f.resistance)?;
    assert_eq!(
        (zero.value.clone(), zero.origin),
        (
            Some(FieldValue::Quantity {
                si: 0.0,
                display_unit: None
            }),
            Origin::Assigned
        )
    );
    set_properties(
        &mut engine,
        r1,
        vec![input(f.resistance, PropertyInput::Unset)],
    )?;
    let unset = effective(&engine, r1, f.resistance)?;
    assert_eq!((unset.value, unset.origin), (None, Origin::ExplicitlyUnset));
    set_properties(
        &mut engine,
        r1,
        vec![input(f.resistance, PropertyInput::Inherit)],
    )?;
    let inherited = effective(&engine, r1, f.resistance)?;
    assert_eq!(inherited.origin, Origin::ComponentDefault);
    assert_eq!(si(&inherited), Some(1000.0));
    assert!(
        engine
            .project()
            .instance(r1)
            .is_some_and(|i| i.properties.is_empty())
    );
    Ok(())
}

#[test]
fn bounds_choices_types_and_text_length_are_rejected_atomically() -> Result<()> {
    let Setup {
        mut engine,
        fields: f,
        r1,
        part,
        ..
    } = setup()?;
    let unselected = publish_field(&mut engine, "Unselected", field(ValueType::Boolean))?;
    set_properties(&mut engine, r1, vec![text(f.count, "3")])?;
    prime_history(&mut engine)?;
    let kept_after = Snapshot::of(&engine);
    let rejected: Vec<(Vec<PropertyEdit>, &str)> = vec![
        (vec![text(f.resistance, "-5")], "invalid_input"),
        (vec![text(f.resistance, "2G")], "invalid_input"),
        (vec![text(f.count, "11")], "invalid_input"),
        (vec![text(f.count, "two")], "invalid_input"),
        (vec![text(f.dielectric, "Y5V")], "invalid_input"),
        (
            vec![input(
                f.dielectric,
                PropertyInput::Value(FieldValue::Enum(ChoiceId(9))),
            )],
            "invalid_input",
        ),
        (
            vec![input(
                f.count,
                PropertyInput::Value(FieldValue::Boolean(true)),
            )],
            "invalid_input",
        ),
        (vec![text(f.note, "too long text")], "invalid_input"),
        (vec![text(f.fitted, "maybe")], "invalid_input"),
        (vec![text(f.tolerance, "5")], "invalid_input"),
        (vec![text(f.capacitance, "10 kΩ")], "invalid_input"),
        (
            vec![input(
                f.resistance,
                PropertyInput::Value(FieldValue::Quantity {
                    si: 5.0,
                    display_unit: Some("F".into()),
                }),
            )],
            "invalid_input",
        ),
        (vec![text(unselected, "true")], "invalid_input"),
        (vec![text(f.resistance, &"9".repeat(300))], "limit_exceeded"),
        // One valid edit and one invalid edit: the batch is rejected whole.
        (
            vec![text(f.note, "ok"), text(f.count, "99")],
            "invalid_input",
        ),
        (vec![text(f.note, "a"), text(f.note, "b")], "invalid_input"),
        (vec![], "invalid_input"),
        (
            (0..65).map(|_| text(f.note, "x")).collect(),
            "invalid_input",
        ),
    ];
    for (edits, code) in rejected {
        let what = format!("{edits:?}");
        let error = set_properties(&mut engine, r1, edits)
            .err()
            .ok_or_else(|| Error::Invalid(format!("accepted {what}")))?;
        assert_eq!(error.code(), code, "{what}");
        kept_after.assert_kept(&engine, &what);
    }
    assert_eq!(
        set_properties(&mut engine, 999, vec![text(f.note, "x")])
            .err()
            .map(|e| e.code()),
        Some("not_found")
    );
    // Placement applies initial values with the same rules.
    let placed = crate::Operation::Instance(crate::InstanceCommand::Place(
        crate::instances::NewInstance {
            component: part.r1(),
            designator: "R9".into(),
            properties: vec![text(f.count, "12")],
            representation: part.symbol,
            unit: crate::UnitId(1),
            position: [40.0, 0.0],
            physical: None,
        },
    ));
    assert!(apply(&mut engine, placed).is_err());
    kept_after.assert_kept(&engine, "place with an invalid value");
    Ok(())
}

#[test]
fn tolerances_parse_to_fractions_and_give_hand_derived_intervals() -> Result<()> {
    let Setup {
        mut engine,
        fields: f,
        r1,
        ..
    } = setup()?;
    set_properties(
        &mut engine,
        r1,
        vec![text(f.tolerance, "±5%"), text(f.capacitance, "10 µF")],
    )?;
    let tolerance = effective(&engine, r1, f.tolerance)?;
    assert_eq!(
        tolerance.value,
        Some(FieldValue::Tolerance {
            minus: 0.05,
            plus: 0.05
        })
    );
    assert_eq!(tolerance.text.as_deref(), Some("±5%"));
    set_properties(&mut engine, r1, vec![text(f.tolerance, "−20%/+80%")])?;
    let tolerance = effective(&engine, r1, f.tolerance)?;
    let Some(FieldValue::Tolerance { minus, plus }) = tolerance.value else {
        return Err(Error::Invalid("tolerance".into()));
    };
    assert_eq!((minus, plus), (0.2, 0.8));
    let nominal = si(&effective(&engine, r1, f.capacitance)?).unwrap_or_default();
    // 10 µF − 20 % = 8 µF; 10 µF + 80 % = 18 µF.
    let (low, high) = tolerance_interval(nominal, minus, plus, &ToleranceBasis::Relative)?;
    assert!((low - 8e-6).abs() < 1e-18 && (high - 18e-6).abs() < 1e-18);
    // The formatted text reads back to the same value.
    let shown = tolerance.text.unwrap_or_default();
    set_properties(&mut engine, r1, vec![text(f.tolerance, &shown)])?;
    assert_eq!(
        effective(&engine, r1, f.tolerance)?.value,
        Some(FieldValue::Tolerance {
            minus: 0.2,
            plus: 0.8
        })
    );
    Ok(())
}

#[test]
fn instances_are_independent_and_values_survive_undo_redo_and_reopen() -> Result<()> {
    let Setup {
        mut engine,
        fields: f,
        r1,
        r2,
        ..
    } = setup()?;
    let before = engine.document().clone();
    let revision = engine.revision();
    set_properties(
        &mut engine,
        r1,
        vec![
            text(f.resistance, "4.7k"),
            text(f.dielectric, "x7r"),
            input(f.fitted, PropertyInput::Unset),
        ],
    )?;
    assert_eq!(engine.revision(), revision + 1, "one batch, one revision");
    let after = engine.document().clone();
    let r2_before: Vec<_> = inspect(&engine, r2)?.properties;
    assert!(r2_before.iter().all(|p| p.origin != Origin::Assigned));
    assert_eq!(
        effective(&engine, r1, f.dielectric)?.value,
        Some(FieldValue::Enum(ChoiceId(2)))
    );

    let reopened = Engine::from_file(format::decode(&format::encode(&engine.to_file())?)?)?;
    assert_eq!(reopened.document(), engine.document());
    let resistance = effective(&reopened, r1, f.resistance)?;
    assert_eq!(
        resistance.value,
        Some(FieldValue::Quantity {
            si: 4700.0,
            display_unit: Some("kΩ".into())
        })
    );
    assert_eq!(resistance.origin, Origin::Assigned);
    assert_eq!(
        effective(&reopened, r1, f.fitted)?.origin,
        Origin::ExplicitlyUnset
    );
    assert_eq!(inspect(&reopened, r2)?.properties, r2_before);

    engine.undo()?;
    assert_eq!(engine.document(), &before);
    engine.redo()?;
    assert_eq!(engine.document(), &after);
    Ok(())
}

type Required = (String, Vec<ObjectRef>, Option<Stage>);

#[test]
fn a_missing_required_value_blocks_electrical_resolution() -> Result<()> {
    let Setup {
        mut engine,
        fields: f,
        r1,
        ..
    } = setup()?;
    let required = |engine: &Engine| -> Result<Vec<Required>> {
        Ok(inspect(engine, r1)?
            .findings
            .into_iter()
            .filter(|x| x.code == "instance.field_required")
            .map(|x| (x.code, x.objects, x.blocks))
            .collect())
    };
    // The component default satisfies the requirement.
    assert!(required(&engine)?.is_empty());
    set_properties(
        &mut engine,
        r1,
        vec![input(f.resistance, PropertyInput::Unset)],
    )?;
    assert_eq!(
        required(&engine)?,
        vec![(
            "instance.field_required".into(),
            vec![ObjectRef::Property {
                instance: r1,
                field: f.resistance.id
            }],
            Some(Stage::ElectricallyResolved)
        )]
    );
    assert!(!inspect(&engine, r1)?.electrically_resolved);
    set_properties(&mut engine, r1, vec![text(f.resistance, "0")])?;
    assert!(required(&engine)?.is_empty(), "assigned zero is a value");
    Ok(())
}

/// Draft the next revision of a published definition, change it and
/// publish it.
fn revise(
    engine: &mut Engine,
    r: DefinitionRef,
    change: impl FnOnce(&mut crate::library::Definition, &mut crate::library::DefinitionMeta),
) -> Result<DefinitionRef> {
    if engine.drafts().get(r.id).is_some() {
        apply(
            engine,
            crate::Operation::Library(crate::LibraryCommand::DiscardDraft { id: r.id }),
        )?;
    }
    draft_from(engine, r)?;
    let draft = engine
        .drafts()
        .get(r.id)
        .cloned()
        .ok_or_else(|| Error::NotFound("draft".into()))?;
    let (mut definition, mut meta) = (draft.definition, draft.meta);
    change(&mut definition, &mut meta);
    edit(
        engine,
        vec![crate::library::DraftEdit {
            id: r.id,
            meta,
            definition,
        }],
    )?;
    publish(engine, vec![r.id])?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("nothing published".into()))
}

/// Publish a field revision changed by `change` and a component revision
/// selecting it (or, with `drop`, not selecting the field at all).
fn revise_field(
    engine: &mut Engine,
    part: &Fixture,
    field: FieldRef,
    change: impl FnOnce(&mut FieldDefinition, &mut crate::library::DefinitionMeta),
    drop: bool,
) -> Result<ComponentRef> {
    let revised = revise(engine, field.erased(), |d, meta| {
        if let crate::library::Definition::Field(body) = d {
            change(body, meta);
        }
    })?;
    let latest = engine
        .project()
        .library
        .latest(part.component)
        .map(|(_, r)| r)
        .unwrap_or_default();
    let component = DefinitionRef {
        kind: DefinitionKind::Component,
        id: part.component,
        revision: latest,
    };
    let r = revise(engine, component, |d, _| {
        if let crate::library::Definition::Component(c) = d {
            if drop {
                c.fields.retain(|s| s.field.id != field.id);
            }
            for s in &mut c.fields {
                if s.field.id == field.id {
                    s.field = FieldRef::new(revised.id, revised.revision);
                }
            }
        }
    })?;
    Ok(ComponentRef::new(r.id, r.revision))
}

fn update(instances: Vec<Id>, target: ComponentRef, fields: Vec<FieldResolution>) -> Change {
    Change::ComponentRevision {
        instances,
        target,
        resolutions: Resolutions {
            fields,
            ..Default::default()
        },
    }
}

#[test]
fn a_label_only_field_revision_keeps_every_assignment() -> Result<()> {
    let Setup {
        mut engine,
        fields: f,
        part,
        r1,
        r2,
    } = setup()?;
    set_properties(&mut engine, r1, vec![text(f.resistance, "2.2k")])?;
    let target = revise_field(
        &mut engine,
        &part,
        f.resistance,
        |_, meta| meta.name = "Resistance value".into(),
        false,
    )?;
    let change = update(vec![r1, r2], target, vec![]);
    let plan = engine.preview_update(change.clone());
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        plan.changes
            .iter()
            .any(|c| c.contains("R1: field ‘Resistance’ is renamed ‘Resistance value’")),
        "{:?}",
        plan.changes
    );
    engine.apply_update(engine.revision(), change, false)?;
    let kept = effective(&engine, r1, f.resistance)?;
    assert_eq!(kept.label, "Resistance value");
    assert_eq!((si(&kept), kept.origin), (Some(2200.0), Origin::Assigned));
    assert_eq!(
        effective(&engine, r2, f.resistance)?.origin,
        Origin::ComponentDefault
    );
    Ok(())
}

#[test]
fn kinds_with_equal_dimensions_are_not_interchangeable() -> Result<()> {
    let Setup {
        mut engine,
        fields: f,
        part,
        r1,
        r2,
    } = setup()?;
    set_properties(&mut engine, r1, vec![text(f.resistance, "47")])?;
    let reactance = f.reactance_kind;
    assert_ne!(reactance.id, f.resistance_kind.id);
    let target = revise_field(
        &mut engine,
        &part,
        f.resistance,
        |d, _| {
            d.value_type = ValueType::Quantity {
                kind: reactance,
                min: Some(0.0),
                max: Some(1e9),
            }
        },
        false,
    )?;
    let kept = Snapshot::of(&engine);
    let blocked = update(vec![r1, r2], target, vec![]);
    let plan = engine.preview_update(blocked.clone());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.contains("R1") && b.contains("FieldResolution")),
        "{:?}",
        plan.blockers
    );
    assert!(
        engine
            .apply_update(engine.revision(), blocked, false)
            .is_err()
    );
    kept.assert_kept(&engine, "blocked update");
    let resolved = update(
        vec![r1, r2],
        target,
        vec![FieldResolution {
            instance: r1,
            field: f.resistance.id,
            input: PropertyInput::Text("47".into()),
        }],
    );
    assert!(engine.preview_update(resolved.clone()).blockers.is_empty());
    engine.apply_update(engine.revision(), resolved, false)?;
    let field = effective(&engine, r1, f.resistance)?;
    assert_eq!(field.field.revision.0, 2);
    assert_eq!((si(&field), field.origin), (Some(47.0), Origin::Assigned));
    Ok(())
}

#[test]
fn narrowed_constraints_and_removed_fields_block_until_resolved() -> Result<()> {
    let Setup {
        mut engine,
        fields: f,
        part,
        r1,
        r2,
    } = setup()?;
    set_properties(
        &mut engine,
        r1,
        vec![text(f.count, "8"), text(f.note, "keep")],
    )?;
    set_properties(&mut engine, r2, vec![text(f.count, "2")])?;
    let narrowed = revise_field(
        &mut engine,
        &part,
        f.count,
        |d, _| d.value_type = ValueType::Integer { min: 0, max: 5 },
        false,
    )?;
    let plan = engine.preview_update(update(vec![r1, r2], narrowed, vec![]));
    assert_eq!(plan.blockers.len(), 1, "{:?}", plan.blockers);
    assert!(plan.blockers[0].contains("R1") && plan.blockers[0].contains("Count"));
    // R2's value still fits and is preserved; R1 needs a decision.
    let resolved = update(
        vec![r1, r2],
        narrowed,
        vec![FieldResolution {
            instance: r1,
            field: f.count.id,
            input: PropertyInput::Text("5".into()),
        }],
    );
    let plan = engine.preview_update(resolved.clone());
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        plan.changes
            .iter()
            .any(|c| c.contains("R1: field ‘Count’ 8 (Assigned) → 5 (Assigned)")),
        "{:?}",
        plan.changes
    );
    // A resolution that does not fit the target is rejected, not applied.
    let wrong = update(
        vec![r1, r2],
        narrowed,
        vec![FieldResolution {
            instance: r1,
            field: f.count.id,
            input: PropertyInput::Text("6".into()),
        }],
    );
    assert!(!engine.preview_update(wrong).blockers.is_empty());
    engine.apply_update(engine.revision(), resolved, false)?;
    assert_eq!(
        effective(&engine, r1, f.count)?.value,
        Some(FieldValue::Integer(5))
    );
    assert_eq!(
        effective(&engine, r2, f.count)?.value,
        Some(FieldValue::Integer(2))
    );

    // Removing a populated field blocks until the override is dropped.
    let removed = revise_field(&mut engine, &part, f.note, |_, _| {}, true)?;
    let plan = engine.preview_update(update(vec![r1, r2], removed, vec![]));
    assert!(
        plan.blockers.iter().any(|b| b.contains("‘Note’")),
        "{:?}",
        plan.blockers
    );
    let not_offered = update(
        vec![r1],
        removed,
        vec![FieldResolution {
            instance: r1,
            field: f.note.id,
            input: PropertyInput::Text("x".into()),
        }],
    );
    assert!(!engine.preview_update(not_offered).blockers.is_empty());
    let dropped = update(
        vec![r1, r2],
        removed,
        vec![FieldResolution {
            instance: r1,
            field: f.note.id,
            input: PropertyInput::Inherit,
        }],
    );
    engine.apply_update(engine.revision(), dropped, false)?;
    let r1_fields = inspect(&engine, r1)?.properties;
    assert!(r1_fields.iter().all(|p| p.field.id != f.note.id));
    assert!(
        engine
            .project()
            .instance(r1)
            .is_some_and(|i| !i.properties.contains_key(&f.note.id))
    );
    // Undo restores the previous revision pin and the override.
    engine.undo()?;
    assert_eq!(
        effective(&engine, r1, f.note)?.value,
        Some(FieldValue::Text("keep".into()))
    );
    Ok(())
}

#[test]
fn publication_checks_field_kind_and_selection_rules() -> Result<()> {
    let mut engine = Engine::default();
    let f = publish_fields(&mut engine)?;
    let blocked =
        |engine: &mut Engine, definition: crate::library::Definition| -> Result<Vec<String>> {
            let id = new_draft(engine, definition.kind())?;
            edit(
                engine,
                vec![crate::library::DraftEdit {
                    id,
                    meta: meta("Invalid"),
                    definition,
                }],
            )?;
            let report = check(engine, CheckTarget::Draft(id))?;
            let codes = report
                .findings
                .iter()
                .filter(|x| x.blocks_stage(Stage::Publish))
                .map(|x| x.code.clone())
                .collect();
            let error = publish(engine, vec![id])
                .err()
                .map(|e| e.code())
                .unwrap_or("published");
            assert_eq!(error, "conflict");
            Ok(codes)
        };
    let mut kind = units(OHMS, "Ω", &[]);
    kind.default_unit = "V".into();
    assert_eq!(
        blocked(&mut engine, crate::library::Definition::QuantityKind(kind))?,
        vec!["quantity_kind.default_unit"]
    );
    assert_eq!(
        blocked(
            &mut engine,
            crate::library::Definition::QuantityKind(units(OHMS, "1Ω", &[]))
        )?,
        vec!["quantity_kind.units"]
    );
    assert_eq!(
        blocked(
            &mut engine,
            crate::library::Definition::Field(FieldDefinition {
                default: Some(FieldValue::Integer(20)),
                ..field(ValueType::Integer { min: 0, max: 10 })
            })
        )?,
        vec!["field.default_invalid"]
    );
    assert_eq!(
        blocked(
            &mut engine,
            crate::library::Definition::Field(field(ValueType::Quantity {
                kind: QuantityKindRef::new(f.resistance.id, crate::Revision(1)),
                min: None,
                max: None,
            }))
        )?,
        vec!["reference.missing"]
    );
    assert_eq!(
        blocked(
            &mut engine,
            crate::library::Definition::Field(FieldDefinition {
                next_local: 2,
                ..field(ValueType::Enum {
                    choices: vec![Choice {
                        id: ChoiceId(2),
                        label: "A".into(),
                    }],
                })
            })
        )?,
        vec!["definition.local_id_unallocated"]
    );
    assert_eq!(
        blocked(
            &mut engine,
            crate::library::Definition::Field(FieldDefinition {
                required_at: Some(Stage::Publish),
                ..field(ValueType::Boolean)
            })
        )?,
        vec!["field.required_stage"]
    );
    // Component selections: one revision per lineage, defaults that fit.
    let mut duplicate = selections(&f);
    duplicate.push(select(f.count));
    let mut bad_default = selections(&f);
    bad_default[0].default = Some(FieldValue::Quantity {
        si: -1.0,
        display_unit: None,
    });
    for (fields, code) in [
        (duplicate, "component.duplicate_field_lineage"),
        (bad_default, "component.field_default_invalid"),
    ] {
        let part = draft(&mut engine, &probe("Bad", PinKind::Passive))?;
        with(&mut engine, part.component, component_body, |c| {
            c.fields = fields
        })?;
        let report = check(&engine, CheckTarget::Draft(part.component))?;
        assert!(
            report.findings.iter().any(|x| x.code == code),
            "{code}: {:?}",
            report.findings
        );
        assert!(publish(&mut engine, part.all()).is_err());
        assert_eq!(
            engine.project().library.components.len(),
            0,
            "nothing published"
        );
    }
    assert_eq!(engine.project().library.fields.len(), 7);
    Ok(())
}
