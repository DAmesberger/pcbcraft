//! The grammar through its public query and command: every declared pattern
//! parses, every example builds, values stay exact, nothing is reordered.
use super::*;
use crate::diagnostics::Result;
use crate::library::templates::generic::GenericPlan;
use crate::{Engine, Evaluators, LibraryCommand, Operation, Outcome, Query, QueryResult};

fn read(engine: &Engine, query: &str) -> Vec<GenericCandidate> {
    match engine.query(Query::GenericParts {
        query: query.to_owned(),
    }) {
        Ok(QueryResult::GenericParts(report)) => report.candidates,
        other => {
            assert_eq!(format!("{other:?}"), "a report");
            Vec::new()
        }
    }
}

fn fail<T>(why: &str) -> Result<T> {
    Err(crate::Error::NotFound(why.to_owned()))
}

fn one(engine: &Engine, query: &str) -> GenericCandidate {
    let mut found = read(engine, query);
    assert_eq!(found.len(), 1, "{query}: {found:?}");
    found.remove(0)
}

fn labels(c: &GenericCandidate) -> Vec<&str> {
    c.badges.iter().map(|b| b.label.as_str()).collect()
}

fn find_or_create(engine: &mut Engine, query: &str, candidate: Option<usize>) -> Result<Outcome> {
    engine.apply_hosted(
        engine.revision(),
        Operation::Library(LibraryCommand::FindOrCreate {
            query: query.to_owned(),
            candidate,
        }),
        Evaluators::default(),
    )
}

#[test]
fn every_declared_pattern_parses_and_every_example_builds_a_valid_form() -> Result<()> {
    let engine = Engine::default();
    for family in &FAMILIES {
        assert!(!family.patterns.is_empty() && !family.examples.is_empty());
        for pattern in family.patterns {
            let parsed = parse_pattern(pattern);
            assert!(parsed.is_ok(), "{pattern}: {parsed:?}");
        }
        for example in family.examples {
            let found = read(&engine, example);
            let Some(own) = found.iter().find(|c| c.family == family.family) else {
                return fail(&format!("{example} reads as {found:?}"));
            };
            assert_eq!(own.state, GenericState::NotCreated, "{example}");
            let plan: GenericPlan = own.spec.plan().map_err(crate::Error::Invalid)?;
            assert!(plan.form.range_problems().is_empty(), "{example}");
            assert!(plan.form.within_bounds(), "{example}");
            assert!(own.would_create.is_some(), "{example}");
        }
    }
    Ok(())
}

#[test]
fn a_bad_pattern_says_why() {
    for bad in [
        "",
        "header {rows",
        "{a:nope}",
        "{a} {a}",
        "x{value:ohm}",
        "a b c d e f g",
    ] {
        assert!(parse_pattern(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_header_is_read_as_written_with_its_default_marked() {
    let engine = Engine::default();
    let c = one(&engine, "header 1x4");
    assert_eq!(c.name, "Pin header 1×4 2.54 mm");
    assert_eq!(labels(&c), ["rows × columns", "2.54 mm (default)"]);
    assert_eq!(c.covers_row.as_deref(), Some("Pin header 1×4 2.54 mm"));
    assert!(c.detail.contains("estimated"), "{}", c.detail);

    assert_eq!(
        labels(&one(&engine, "header 1x4 2mm")),
        ["rows × columns", "2 mm"]
    );
    assert_eq!(
        labels(&one(&engine, "HEADER 1×4 100mil")),
        ["rows × columns", "2.54 mm"]
    );
    assert_eq!(
        one(&engine, "pin header 1x4 2 mm").name,
        "Pin header 1×4 2.0 mm"
    );
}

#[test]
fn rows_and_columns_are_not_reordered() {
    let engine = Engine::default();
    let (five_by_two, two_by_five) = (one(&engine, "header 5x2"), one(&engine, "header 2x5"));
    assert_ne!(five_by_two.spec, two_by_five.spec);
    assert_eq!(five_by_two.name, "Pin header 5×2 2.54 mm");
    let rows_of = |c: &GenericCandidate| match c.spec.plan().map(|p| p.form) {
        Ok(crate::library::PackageForm::Header(h)) => Some((h.rows, h.pins_per_row)),
        _ => None,
    };
    assert_eq!(rows_of(&five_by_two), Some((5, 2)));
    assert_eq!(rows_of(&two_by_five), Some((2, 5)));
}

#[test]
fn out_of_range_is_a_clear_refusal_from_the_form() {
    let engine = Engine::default();
    for (text, says) in [
        ("header 1x9999", "at most 512 pins"),
        ("header 99999999999x4", "at most 512 pins"),
        ("header 0x4", "at least one pin"),
        ("header 1x4 0.1mm", "Pins must not touch"),
        ("resistor 0604", "No chip resistor of imperial size 0604"),
        ("led 2512", "Imperial sizes: 0603, 0805"),
        ("resistor 1608", "1608M is the metric code of imperial 0603"),
        ("resistor 9999M", "No chip resistor of metric size 9999"),
    ] {
        let c = one(&engine, text);
        assert!(
            matches!(&c.state, GenericState::Refused { reason } if reason.contains(says)),
            "{text}: {:?}",
            c.state
        );
        assert!(c.would_create.is_none());
    }
    // A pattern of slots alone is not a request for that family.
    assert!(read(&engine, "10k 0604").is_empty());
}

/// What the list offers can be built: the largest header the form holds.
#[test]
fn the_largest_offered_header_builds() -> Result<()> {
    let mut engine = Engine::default();
    assert_eq!(one(&engine, "header 1x512").state, GenericState::NotCreated);
    find_or_create(&mut engine, "header 1x512", None)?;
    assert!(one(&engine, "header 1x512").component().is_some());
    Ok(())
}

#[test]
fn values_are_typed_and_exact_decimals() {
    let engine = Engine::default();
    for (text, shown) in [
        ("10k 0603", "10 kΩ"),
        ("4k7 0603", "4.7 kΩ"),
        ("10 kΩ 0603", "10 kΩ"),
        ("10K 0603", "10 kΩ"),
        ("4R7 0603", "4.7 Ω"),
        ("resistor 0603 100", "100 Ω"),
        ("resistor 0603 2.2kohm", "2.2 kΩ"),
    ] {
        let c = one(&engine, text);
        assert_eq!(c.family, GenericFamily::Resistor, "{text}");
        assert_eq!(c.values[0].field, "Resistance");
        assert_eq!(c.values[0].text, shown, "{text}");
        assert!(labels(&c).contains(&shown), "{text}: {:?}", c.badges);
    }
    let c = one(&engine, "100nF 0402 16V");
    assert_eq!(c.family, GenericFamily::Capacitor);
    assert_eq!(c.values[0].text, "100 nF");
    assert_eq!(c.values[1].field, "Rated voltage");
    assert_eq!(c.values[1].text, "16 V");
    // Exact: not 4.7000000000000002 µH.
    assert_eq!(one(&engine, "4.7uH 0805").values[0].text, "4.7 µH");
    // The unit decides; a bare prefix reads as every kind it can be.
    let both: Vec<GenericFamily> = read(&engine, "10u 0603").iter().map(|c| c.family).collect();
    assert_eq!(both, [GenericFamily::Capacitor, GenericFamily::Inductor]);
    // The identity holds no value: 10k and 22k are one part.
    assert_eq!(one(&engine, "10k 0603").spec, one(&engine, "22k 0603").spec);
    // Not values: a code alone, a negative, words.
    for text in [
        "0603 0603",
        "-5k 0603",
        "abc 0603",
        "res>=10k",
        "10k",
        "resistor",
    ] {
        assert!(read(&engine, text).is_empty(), "{text}");
    }
}

#[test]
fn size_codes_default_to_imperial_with_the_real_dimensions() {
    let engine = Engine::default();
    let c = one(&engine, "resistor 0603");
    assert_eq!(labels(&c), ["imperial", "1.6 × 0.8 mm"]);
    assert_eq!(c.name, "Chip resistor 0603 (1608 metric)");
    assert_eq!(c.covers_row.as_deref(), Some(c.name.as_str()));

    // The same text in metric spelling is an 0201.
    for text in ["resistor 0603M", "resistor metric 0603"] {
        let c = one(&engine, text);
        assert_eq!(labels(&c), ["metric", "0.6 × 0.3 mm"], "{text}");
        assert_eq!(c.name, "Chip resistor 0201 (0603 metric)");
    }
    assert_eq!(
        one(&engine, "resistor 1608M").spec,
        one(&engine, "resistor 0603").spec
    );
}

#[test]
fn other_families_name_their_part() {
    let engine = Engine::default();
    let c = one(&engine, "led 0603 red");
    assert_eq!(c.family, GenericFamily::Led);
    assert_eq!(c.values[0].field, "Colour");
    assert_eq!(c.values[0].text, "Red");
    assert_eq!(c.spec, one(&engine, "led 0603").spec, "colour per instance");
    assert!(read(&engine, "led 0603 plaid").is_empty());

    let c = one(&engine, "diode sod123");
    assert_eq!(c.name, "Diode SOD-123");
    assert_eq!(c.covers_row, Some("SOD-123".to_owned()));

    // An inductor is built on the ferrite-bead row's body with its own symbol,
    // so that row is still listed as the bead it is.
    let c = one(&engine, "inductor 0603");
    assert_eq!(c.name, "Chip inductor 0603 (1608 metric)");
    assert_eq!(c.covers_row, None);
    assert_eq!(
        c.would_create.and_then(|r| r.designator_prefix).as_deref(),
        Some("L")
    );
}

#[test]
fn matching_is_bounded() {
    let engine = Engine::default();
    let long = "header 1x4 ".repeat(100);
    assert!(read(&engine, &long).is_empty());
    let huge = "x".repeat(10_000);
    assert!(read(&engine, &huge).is_empty());
    assert!(read(&engine, &format!("header {huge}")).is_empty());
    assert!(read(&engine, "").is_empty());
}

#[test]
fn the_same_part_is_found_not_built_again() -> Result<()> {
    let mut engine = Engine::default();
    let Outcome::GenericPart {
        component,
        created: true,
        ..
    } = find_or_create(&mut engine, "resistor 0603", None)?
    else {
        return fail("created");
    };
    let revision = engine.revision();
    assert_eq!(engine.project().library.latest_components().len(), 1);
    let published = engine
        .project()
        .library
        .component(component)
        .ok_or_else(|| crate::Error::NotFound("the part".into()))?;
    assert!(published.definition.generic.is_some());
    assert!(published.meta.provenance.contains("verify against"));
    let name = published.meta.name.clone();
    assert!(
        engine
            .project()
            .library
            .parts
            .iter()
            .all(|p| p.definition.component != component),
        "no manufacturer part until one is chosen"
    );

    // Every spelling of the same part finds it; no change, no undo step.
    for text in ["resistor 0603", "10k 0603", "resistor 1608M"] {
        let found = one(&engine, text);
        assert_eq!(found.component(), Some(component), "{text}");
        assert_eq!(found.name, name);
        assert!(found.would_create.is_none());
        assert!(
            found.detail.starts_with("In this project"),
            "{}",
            found.detail
        );
        let outcome = find_or_create(&mut engine, text, None)?;
        assert!(
            matches!(outcome, Outcome::GenericPart { created: false, component: c, .. } if c == component)
        );
        assert_eq!(engine.revision(), revision);
    }
    assert_eq!(engine.project().library.latest_components().len(), 1);

    // Another part is another component, and one undo takes a part back whole.
    find_or_create(&mut engine, "resistor 0805", None)?;
    assert_eq!(engine.project().library.latest_components().len(), 2);
    engine.undo()?;
    assert_eq!(engine.project().library.latest_components().len(), 1);
    assert_eq!(
        one(&engine, "resistor 0805").state,
        GenericState::NotCreated
    );
    engine.undo()?;
    assert!(engine.project().library.components.is_empty());
    assert!(!engine.can_undo());
    Ok(())
}

/// A template row chosen by hand is the same part a search would make.
#[test]
fn a_template_row_built_by_hand_is_found_too() -> Result<()> {
    let mut engine = Engine::default();
    let Some(request) = one(&engine, "header 1x4").would_create else {
        return fail("a request");
    };
    let by_hand = crate::library::templates::catalog::NewFromTemplate {
        designator_prefix: None,
        name: "My header".into(),
        ..request
    };
    engine.apply_hosted(
        engine.revision(),
        Operation::Library(LibraryCommand::NewFromTemplate(Box::new(by_hand.clone()))),
        Evaluators::default(),
    )?;
    assert!(matches!(
        one(&engine, "header 1x4").state,
        GenericState::Draft { .. }
    ));
    let refused = find_or_create(&mut engine, "header 1x4", None);
    assert!(
        refused.is_err_and(|e| e.to_string().contains("publish that draft")),
        "a draft is published, not built again"
    );
    engine.undo()?;

    engine.apply_hosted(
        engine.revision(),
        Operation::Library(LibraryCommand::PublishFromTemplate(Box::new(by_hand))),
        Evaluators::default(),
    )?;
    let found = one(&engine, "header 1x4");
    assert!(found.component().is_some());
    assert_eq!(found.name, "My header");
    Ok(())
}

/// Edited values and another prefix are another part.
#[test]
fn a_changed_request_is_not_the_generic_part() -> Result<()> {
    let mut engine = Engine::default();
    let Some(request) = one(&engine, "header 1x4").would_create else {
        return fail("a request");
    };
    engine.apply_hosted(
        engine.revision(),
        Operation::Library(LibraryCommand::PublishFromTemplate(Box::new(
            crate::library::templates::catalog::NewFromTemplate {
                designator_prefix: Some("P".into()),
                ..request
            },
        ))),
        Evaluators::default(),
    )?;
    assert_eq!(one(&engine, "header 1x4").state, GenericState::NotCreated);
    Ok(())
}

#[test]
fn find_or_create_refuses_what_it_cannot_settle() -> Result<()> {
    let mut engine = Engine::default();
    let before = engine.revision();
    for (text, code) in [
        ("nonsense words", "reads as no generic part"),
        ("10u 0603", "0: Chip capacitor 0603"),
        ("header 1x9999", "at most 512 pins"),
    ] {
        let error = find_or_create(&mut engine, text, None)
            .err()
            .map(|e| e.to_string());
        assert!(
            error.as_deref().is_some_and(|e| e.contains(code)),
            "{text}: {error:?}"
        );
    }
    assert!(find_or_create(&mut engine, "10u 0603", Some(7)).is_err());
    assert_eq!(engine.revision(), before, "a refusal changes nothing");
    let Outcome::GenericPart { component, .. } = find_or_create(&mut engine, "10u 0603", Some(1))?
    else {
        return fail("built");
    };
    let name = engine
        .project()
        .library
        .component(component)
        .map(|c| c.meta.name.clone());
    assert_eq!(name.as_deref(), Some("Chip inductor 0603 (1608 metric)"));
    Ok(())
}

#[test]
fn colour_reaches_the_placed_instance_through_the_outcome() -> Result<()> {
    let mut engine = Engine::default();
    let Outcome::GenericPart {
        component,
        properties,
        ..
    } = find_or_create(&mut engine, "led 0603 red", None)?
    else {
        return fail("built");
    };
    assert_eq!(properties.len(), 1, "the LED selects the Colour field");
    let published = engine
        .project()
        .library
        .component(component)
        .ok_or_else(|| crate::Error::NotFound("the part".into()))?;
    assert_eq!(properties[0].field, published.definition.fields[0].field.id);
    // No field of that name: left out, not an error.
    let Outcome::GenericPart { properties, .. } = find_or_create(&mut engine, "10k 0603", None)?
    else {
        return fail("built");
    };
    assert!(properties.iter().all(|p| {
        engine
            .project()
            .library
            .fields
            .iter()
            .any(|f| f.id == p.field)
    }));
    Ok(())
}
