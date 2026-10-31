use super::*;
use crate::format::{EditorFile, FILE_VERSION};
use crate::{Engine, Query, QueryResult};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

/// A validated engine over a synthetic project of `parts` parts.
fn engine(parts: usize) -> Result<Engine> {
    Engine::from_file(EditorFile {
        version: FILE_VERSION,
        project: synthetic::project(parts)?,
        drafts: Drafts::default(),
    })
}

fn index(parts: usize) -> Result<Index> {
    let engine = engine(parts)?;
    Index::of_project(engine.project(), engine.drafts())
}

fn names(report: &SearchReport) -> Vec<&str> {
    report
        .results
        .iter()
        .map(|h| h.entry.name.as_str())
        .collect()
}

fn search(index: &Index, text: &str) -> std::result::Result<SearchReport, QueryError> {
    let query = index.parse(text)?;
    Ok(index.search(
        &query,
        &CatalogContext::default(),
        SearchOptions {
            limit: MAX_RESULTS,
            include_drafts: false,
        },
    ))
}

fn field_id(index: &Index, name: &str) -> Result<DefinitionId> {
    index
        .vocabulary()
        .fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.field)
        .ok_or_else(|| Error::NotFound(name.into()))
}

/// The SI value a term reads for one field.
fn reading(term: &Term, field: DefinitionId) -> Option<f64> {
    let tests = match term {
        Term::Value { readings, .. } => readings,
        Term::Field { tests, .. } => tests,
        _ => return None,
    };
    tests
        .iter()
        .find(|t| t.field == field)
        .and_then(|t| match t.test {
            Test::Compare { value, .. } => Some(value),
            _ => None,
        })
}

#[test]
fn synthetic_project_is_a_valid_document() -> TestResult {
    let engine = engine(24)?;
    assert_eq!(engine.project().library.components.len(), 24);
    let index = Index::of_project(engine.project(), engine.drafts())?;
    assert_eq!(index.entries().len(), 24);
    let names: Vec<&str> = index
        .vocabulary()
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["Resistance", "Capacitance", "Max voltage", "Tolerance"]
    );
    Ok(())
}

#[test]
fn rkm_takes_multiplier_letters_and_only_the_units_own_letters() -> TestResult {
    let index = index(6)?;
    let units = |name: &str| -> Result<UnitSet> {
        let field = index
            .vocabulary()
            .fields
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.kind.clone())
            .ok_or_else(|| Error::NotFound(name.into()))?;
        Ok(field.quantity.units)
    };
    let ohms = units("Resistance")?;
    let farads = units("Capacitance")?;
    assert_eq!(rkm("4k7", &ohms).as_deref(), Some("4.7k"));
    assert_eq!(rkm("4R7", &ohms).as_deref(), Some("4.7R"));
    assert_eq!(rkm("R47", &ohms).as_deref(), Some("0.47R"));
    assert_eq!(rkm("2u2", &farads).as_deref(), Some("2.2u"));
    assert_eq!(rkm("4n7F", &farads).as_deref(), Some("4.7nF"));
    // `R` is a decimal point only where the unit set declares it.
    assert_eq!(rkm("R47", &farads), None);
    assert_eq!(rkm("4R7", &farads), None);
    // A leading multiplier, plain decimals and exponents are not RKM.
    assert_eq!(rkm("k47", &ohms), None);
    assert_eq!(rkm("4.7k", &ohms), None);
    assert_eq!(rkm("1e3", &ohms), None);
    assert_eq!(rkm("10k", &ohms), None);
    Ok(())
}

use crate::quantity::UnitSet;

#[test]
fn bare_values_read_rkm_and_si_text_through_the_fields_own_units() -> TestResult {
    let index = index(24)?;
    let resistance = field_id(&index, "Resistance")?;
    let capacitance = field_id(&index, "Capacitance")?;
    let one = |text: &str| -> std::result::Result<Term, Box<dyn std::error::Error>> {
        let mut query = index.parse(text)?;
        Ok(query.terms.pop().ok_or("no term")?)
    };
    let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() <= 1e-12 * b.abs());
    assert!(close(reading(&one("4k7")?, resistance), 4700.0));
    assert!(close(reading(&one("4.7k")?, resistance), 4700.0));
    assert!(close(reading(&one("R47")?, resistance), 0.47));
    assert!(close(reading(&one("4R7")?, resistance), 4.7));
    // R is not a capacitance letter, so R47 reads only as a resistance.
    assert_eq!(reading(&one("R47")?, capacitance), None);
    assert!(close(reading(&one("4n7")?, capacitance), 4.7e-9));
    assert!(close(reading(&one("100nF")?, capacitance), 100e-9));
    // Plain numbers and names stay text.
    assert_eq!(
        one("0603")?,
        Term::Text {
            text: "0603".into()
        }
    );
    assert_eq!(
        one("EX-R0603")?,
        Term::Text {
            text: "ex-r0603".into()
        }
    );

    // The invented name happens to contain "4k7"; "4.7k" matches the value
    // only.
    let found = search(&index, "4k7")?;
    assert_eq!(names(&found), ["EX-R0402-4K70-J"]);
    assert_eq!(found.results[0].tier, MatchTier::Substring);
    // The chip shows only the readings that found something.
    let found = search(&index, "10k")?;
    assert_eq!(found.chips, ["Resistance = 10 kΩ or 10k"]);
    let found = search(&index, "4.7k")?;
    assert_eq!(names(&found), ["EX-R0402-4K70-J"]);
    assert_eq!(found.results[0].tier, MatchTier::Field);
    let found = search(&index, "R47")?;
    assert_eq!(names(&found), ["EX-R0603-R470-F"]);
    Ok(())
}

#[test]
fn a_unit_key_refuses_a_bare_multiplier_with_the_dimension_it_lacks() -> TestResult {
    let index = index(24)?;
    let error = index.parse("V>=10k").err().ok_or("V>=10k parsed")?;
    assert_eq!(error.to_string(), "V>=10k: 10 k has no unit of voltage");
    let error = index.parse("V>=10kΩ").err().ok_or("V>=10kΩ parsed")?;
    assert_eq!(error.term, "V>=10kΩ");
    assert!(
        error.message.contains("‘kΩ’ is not a unit of voltage"),
        "{error}"
    );
    // A plain number is in the key's unit; a stated unit of the kind is fine.
    let voltage = field_id(&index, "Max voltage")?;
    for text in ["V>=25", "V>=25V", "V>=0.025kV", "kV>=0.025"] {
        let query = index.parse(text)?;
        assert_eq!(reading(&query.terms[0], voltage), Some(25.0), "{text}");
    }
    let found = search(&index, "V>=25")?;
    assert!(names(&found).iter().all(|n| n.starts_with("EX-C")));
    assert_eq!(found.total, 4, "{:?}", names(&found));
    // The engine reports the same message as a typed error.
    let engine = engine(24)?;
    let error = engine
        .query(Query::Search {
            query: "V>=10k".into(),
            limit: None,
            include_drafts: false,
        })
        .err()
        .ok_or("engine accepted V>=10k")?;
    assert_eq!(error.code(), "invalid_input");
    assert_eq!(error.to_string(), "V>=10k: 10 k has no unit of voltage");
    Ok(())
}

#[test]
fn field_keys_resolve_from_data_and_report_unknown_or_ambiguous_keys() -> TestResult {
    let index = index(24)?;
    let resistance = field_id(&index, "Resistance")?;
    // Exact name, unique prefix and a unit alias all reach the field.
    for text in [
        "resistance>=10k",
        "res>=10k",
        "R>=10kΩ",
        "Ω>=10000",
        "kΩ>=10",
        "ohm>=10kohm",
    ] {
        let query = index.parse(text)?;
        assert_eq!(reading(&query.terms[0], resistance), Some(10e3), "{text}");
    }
    // A unit key refuses the bare multiplier, as for volts; `R` is a unit
    // alias of this data, so it is a unit key even though it also begins
    // the field name.
    for text in ["Ω>=10k", "R>=10k"] {
        let error = index.parse(text).err().ok_or("bare multiplier parsed")?;
        assert_eq!(error.message, "10 k has no unit of resistance", "{text}");
    }
    let error = index.parse("wattage>=1").err().ok_or("unknown key")?;
    assert!(
        error
            .message
            .starts_with("‘wattage’ is not a field or filter"),
        "{error}"
    );
    assert!(error.message.contains("Resistance"));
    assert!(error.message.contains("filters: pkg, has"), "{error}");
    // `kind:component` lists component entries; it is not the source filter.
    assert_eq!(index.parse("kind:component")?.terms, [Term::Components]);
    // `ma` is a prefix of one field only; `t` of none; `c`/`ca` only one.
    assert!(index.parse("ma>=10").is_ok());
    assert!(
        index
            .parse("tag:audio")
            .is_err_and(|e| e.message.contains("reserved"))
    );
    assert!(
        index
            .parse("R>10k..22k")
            .is_err_and(|e| e.message.contains("‘:’"))
    );
    assert!(
        index
            .parse("R>=")
            .is_err_and(|e| e.message.contains("needs a value"))
    );
    assert!(index.parse("\"unclosed").is_err());
    let long = "x ".repeat(MAX_TERMS + 1);
    assert!(
        index
            .parse(&long)
            .is_err_and(|e| e.message.contains("terms"))
    );
    Ok(())
}

/// The synthetic vocabulary with `Max voltage` renamed to `Voltage` and a
/// second field whose name also begins with `R`.
fn renamed_vocabulary(index: &Index) -> Result<Vocabulary> {
    let mut vocabulary = index.vocabulary().clone();
    let mut extra = vocabulary
        .fields
        .iter()
        .find(|f| f.name == "Max voltage")
        .cloned()
        .ok_or_else(|| Error::NotFound("Max voltage".into()))?;
    for field in &mut vocabulary.fields {
        if field.name == "Max voltage" {
            field.name = "Voltage".into();
        }
    }
    extra.name = "Rated power".into();
    extra.field = DefinitionId(u64::MAX);
    vocabulary.fields.push(extra);
    Ok(vocabulary)
}

#[test]
fn unit_keys_do_not_depend_on_which_other_fields_exist() -> TestResult {
    let index = index(24)?;
    let resistance = field_id(&index, "Resistance")?;
    let voltage = field_id(&index, "Max voltage")?;
    let renamed = renamed_vocabulary(&index)?;
    for vocabulary in [index.vocabulary(), &renamed] {
        // `V` and `R` are unit keys whatever the field names are: a field
        // named `Voltage` does not make `V>=10k` mean 10 kV, and `Rated
        // power` does not change how `R` reads.
        let error = parse_query("V>=10k", vocabulary)
            .err()
            .ok_or("V>=10k parsed")?;
        assert_eq!(error.to_string(), "V>=10k: 10 k has no unit of voltage");
        let error = parse_query("R>=10k", vocabulary)
            .err()
            .ok_or("R>=10k parsed")?;
        assert_eq!(error.to_string(), "R>=10k: 10 k has no unit of resistance");
        let query = parse_query("R>=10kΩ", vocabulary)?;
        assert_eq!(reading(&query.terms[0], resistance), Some(10e3));
        let query = parse_query("V>=25", vocabulary)?;
        assert_eq!(reading(&query.terms[0], voltage), Some(25.0));
        // A field-name key reads a bare multiplier in the default unit.
        let query = parse_query("res>=10k", vocabulary)?;
        assert_eq!(reading(&query.terms[0], resistance), Some(10e3));
    }
    // In the renamed data `volt` names the field and `r` is ambiguous.
    let query = parse_query("volt>=10k", &renamed)?;
    assert_eq!(reading(&query.terms[0], voltage), Some(10e3));
    let error = parse_query("r>=1", &renamed).err().ok_or("r>=1 parsed")?;
    assert!(
        error
            .message
            .contains("matches several fields: Rated power, Resistance"),
        "{error}"
    );
    Ok(())
}

#[test]
fn ranges_and_tolerances_mean_meets_not_equals() -> TestResult {
    let index = index(24)?;
    let found = search(&index, "res:4k7..22k")?;
    let mut got = names(&found);
    got.sort_unstable();
    assert_eq!(
        got,
        ["EX-R0402-4K70-J", "EX-R0603-10K0-F", "EX-R0603-22K0-D"]
    );
    // ±0.5 % and ±1 % meet "tol ≤ 1 %"; ±5 % does not.
    // Eight resistors cycle ±1 %, ±5 %, ±0.5 %: three at 1 %, two at 0.5 %.
    let found = search(&index, "tol<=1%")?;
    assert_eq!(found.total, 5);
    for hit in &found.results {
        let tolerance = hit
            .entry
            .fields
            .iter()
            .find(|f| f.label == "Tolerance")
            .ok_or("no tolerance")?;
        assert!(matches!(tolerance.value, FieldValue::Tolerance { plus, .. } if plus <= 0.01));
    }
    let tight = search(&index, "tol<1%")?;
    assert_eq!(tight.total, 2);
    assert!(search(&index, "tol:1..2").is_err());
    // Predicates and filters combine with text.
    let found = search(&index, "res>=10k pkg:0603")?;
    let mut got = names(&found);
    got.sort_unstable();
    // 100 kΩ is the only ≥ 10 kΩ part in 0402.
    assert_eq!(
        got,
        ["EX-R0603-10K0-F", "EX-R0603-1M00-J", "EX-R0603-22K0-D"]
    );
    assert_eq!(search(&index, "has:footprint")?.total, 24);
    assert_eq!(search(&index, "missing:mpn")?.total, 24);
    assert_eq!(search(&index, "has:mpn")?.total, 0);
    assert_eq!(search(&index, "src:project hdr")?.total, 8);
    Ok(())
}

#[test]
fn ranking_is_deterministic_and_follows_the_tiers() -> TestResult {
    let engine = engine(24)?;
    let project = engine.project();
    let mut entries = entries(project, engine.drafts());
    let vocabulary = Vocabulary::of(&project.library);
    let forward = Index::new(entries.clone(), vocabulary.clone())?;
    entries.reverse();
    entries.rotate_left(7);
    let shuffled = Index::new(entries, vocabulary)?;
    for text in [
        "ex",
        "ex-r",
        "10k",
        "0603",
        "res>=1k",
        "hdr",
        "capacitor",
        "",
    ] {
        let a = search(&forward, text)?;
        let b = search(&shuffled, text)?;
        assert_eq!(a, b, "{text}");
    }
    // Exact name, then name prefix, then a whole word, then a substring.
    let found = search(&forward, "ex-hdr-1x2-001")?;
    assert_eq!(found.results[0].tier, MatchTier::Exact);
    let found = search(&forward, "EX-HDR")?;
    assert!(found.results.iter().all(|h| h.tier == MatchTier::Prefix));
    assert_eq!(
        names(&found),
        [
            "EX-HDR-1X2-001",
            "EX-HDR-1X2-002",
            "EX-HDR-1X2-003",
            "EX-HDR-1X2-004",
            "EX-HDR-1X2-005",
            "EX-HDR-1X2-006",
            "EX-HDR-1X2-007",
            "EX-HDR-1X2-008"
        ]
    );
    let found = search(&forward, "hdr")?;
    assert!(found.results.iter().all(|h| h.tier == MatchTier::Word));
    let found = search(&forward, "ceramic")?;
    assert!(found.results.iter().all(|h| h.tier == MatchTier::Substring));
    // A name match outranks a field-only match of the same text.
    let found = search(&forward, "10k")?;
    assert_eq!(found.results[0].entry.name, "EX-R0603-10K0-F");
    assert_eq!(found.results[0].tier, MatchTier::Substring);
    // Boosts: used in the project, then favourite, then recent.
    let header = |n: usize| found_lineage(&forward, &format!("EX-HDR-1X2-{n:03}"));
    let context = CatalogContext {
        favourites: [header(5)?].into(),
        recent: vec![header(7)?, header(6)?],
    };
    let query = forward.parse("EX-HDR")?;
    let ranked = forward.search(&query, &context, SearchOptions::default());
    assert_eq!(
        names(&ranked)[..4],
        [
            "EX-HDR-1X2-005",
            "EX-HDR-1X2-007",
            "EX-HDR-1X2-006",
            "EX-HDR-1X2-001"
        ]
    );
    assert!(ranked.results[0].favourite);
    // The limit keeps the same leading order.
    let limited = forward.search(
        &query,
        &context,
        SearchOptions {
            limit: 2,
            include_drafts: false,
        },
    );
    assert_eq!(names(&limited), names(&ranked)[..2]);
    assert_eq!(limited.total, 8);
    Ok(())
}

fn found_lineage(index: &Index, name: &str) -> Result<DefinitionId> {
    index
        .entries()
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.lineage)
        .ok_or_else(|| Error::NotFound(name.into()))
}

#[test]
fn facets_come_from_the_fields_the_results_carry() -> TestResult {
    let index = index(24)?;
    let all = index.facets(&index.parse("")?, false);
    assert_eq!(all.total, 24);
    let labels: Vec<&str> = all.fields.iter().map(|f| f.label.as_str()).collect();
    assert_eq!(
        labels,
        ["Capacitance", "Max voltage", "Resistance", "Tolerance"]
    );
    let resistance = &all.fields[2];
    assert_eq!(resistance.entries, 8);
    assert_eq!(resistance.kind.as_deref(), Some("Resistance"));
    assert_eq!(resistance.min.as_deref(), Some("470 mΩ"));
    assert_eq!(resistance.max.as_deref(), Some("1 MΩ"));
    assert_eq!(resistance.values.len(), 8);
    // Equal counts list in text order.
    let packages: Vec<(&str, usize)> = all
        .packages
        .iter()
        .map(|p| (p.text.as_str(), p.count))
        .collect();
    assert_eq!(packages, [("0402", 8), ("0603", 8), ("TH 2.54", 8)]);
    assert_eq!(all.badges.symbol, 24);
    assert_eq!(all.badges.mpn, 0);
    // Headers carry no fields, so a header query offers no field facet.
    let headers = index.facets(&index.parse("hdr")?, false);
    assert_eq!(headers.total, 8);
    assert!(headers.fields.is_empty());
    assert_eq!(headers.packages[0].text, "TH 2.54");
    let resistors = index.facets(&index.parse("R>=1")?, false);
    let labels: Vec<&str> = resistors.fields.iter().map(|f| f.label.as_str()).collect();
    assert_eq!(labels, ["Resistance", "Tolerance"]);
    Ok(())
}

#[test]
fn draft_only_lineages_are_counted_as_hidden_unless_asked_for() -> TestResult {
    use crate::{LibraryCommand, Operation, Outcome};
    let mut engine = engine(3)?;
    let outcome = engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::NewDraft {
            meta: DefinitionMeta {
                name: "EX-DRAFT-PART".into(),
                ..Default::default()
            },
            definition: crate::library::Definition::empty(DefinitionKind::Component),
        }),
    )?;
    let Outcome::DraftCreated { id } = outcome else {
        return Err("no draft".into());
    };
    let search = |include_drafts| {
        engine.query(Query::Search {
            query: "ex".into(),
            limit: None,
            include_drafts,
        })
    };
    let QueryResult::Search(report) = search(false)? else {
        return Err("not a search report".into());
    };
    assert_eq!(report.total, 3);
    assert_eq!(
        report.hidden,
        [Hidden {
            reason: "unpublished draft".into(),
            count: 1
        }]
    );
    let QueryResult::Search(report) = search(true)? else {
        return Err("not a search report".into());
    };
    let draft = report
        .results
        .iter()
        .find(|h| h.entry.lineage == id)
        .ok_or("draft missing")?;
    assert!(draft.entry.draft && draft.entry.revision.is_none());
    assert!(report.hidden.is_empty());
    Ok(())
}

#[test]
fn inspect_reports_the_latest_revision_and_what_it_pins() -> TestResult {
    let engine = Engine::demo()?;
    let lineage = engine.project().instances[0].component.id;
    let QueryResult::Lineage(report) = engine.query(Query::InspectLineage {
        lineage,
        revision: None,
    })?
    else {
        return Err("not a lineage report".into());
    };
    assert!(report.latest);
    assert_eq!(report.entry.revision, Some(Revision::FIRST));
    assert_eq!(report.entry.used, 2);
    assert_eq!(report.revisions.len(), 1);
    assert_eq!(report.symbols[0].units, ["A"]);
    assert_eq!(report.implementations[0].footprints[0].pads, 2);
    assert!(report.entry.badges.symbol && report.entry.badges.footprint);
    assert!(report.entry.badges.model && !report.entry.badges.mpn);
    let missing = engine.query(Query::InspectLineage {
        lineage,
        revision: Some(Revision(9)),
    });
    assert!(missing.is_err_and(|e| e.code() == "not_found"));
    // Queries leave the document, revision and history unchanged.
    let before = engine.document().clone();
    let revision = engine.revision();
    engine.query(Query::Facets {
        query: "R>=1".into(),
        include_drafts: false,
    })?;
    assert_eq!(engine.document(), &before);
    assert_eq!(engine.revision(), revision);
    assert!(!engine.can_undo());
    Ok(())
}

/// Keystroke-to-results latency of core `search` on 10 k–50 k synthetic
/// entries. Run with
/// `cargo test --release -p pcbcraft-core catalog::tests::bench -- --ignored --nocapture`.
/// Target (not a gate): ≤ 10 ms p95 at 50 k entries, warm.
#[test]
#[ignore = "benchmark; run explicitly in release mode"]
fn bench_search_on_synthetic_entries() -> TestResult {
    use std::time::Instant;
    const RUNS: usize = 60;
    let queries = [
        "10k",
        "res>=10k pkg:0603",
        "tol<=1%",
        "ex-c",
        "hdr",
        "4k7",
        "capacitor 50V",
    ];
    for count in [10_000, 50_000] {
        let built = Instant::now();
        let index = synthetic::index(count)?;
        let build = built.elapsed();
        let bytes: usize = index
            .entries
            .iter()
            .map(|e| {
                std::mem::size_of::<CatalogEntry>()
                    + e.name.capacity()
                    + e.description.capacity()
                    + e.packages.iter().map(|p| p.capacity() + 24).sum::<usize>()
                    + e.fields
                        .iter()
                        .map(|f| {
                            std::mem::size_of::<EntryField>()
                                + f.label.capacity()
                                + f.text.capacity()
                        })
                        .sum::<usize>()
            })
            .sum::<usize>()
            + index
                .folded
                .iter()
                .map(|f| {
                    std::mem::size_of::<Folded>()
                        + f.name.capacity()
                        + f.lineage.capacity()
                        + f.other.capacity()
                        + f.fields.capacity()
                        + f.words.iter().map(|w| w.capacity() + 24).sum::<usize>()
                })
                .sum::<usize>();
        println!(
            "entries {count}: index build {:.1} ms, approx. {:.1} MB",
            build.as_secs_f64() * 1e3,
            bytes as f64 / 1e6
        );
        for text in queries {
            let mut times = Vec::with_capacity(RUNS);
            let mut total = 0;
            for _ in 0..RUNS {
                let start = Instant::now();
                let query = index.parse(text)?;
                let report =
                    index.search(&query, &CatalogContext::default(), SearchOptions::default());
                times.push(start.elapsed().as_secs_f64() * 1e3);
                total = report.total;
            }
            times.sort_by(f64::total_cmp);
            let p50 = times[RUNS / 2];
            let p95 = times[RUNS * 95 / 100];
            println!("  {text:<18} matches {total:>6}  p50 {p50:>6.2} ms  p95 {p95:>6.2} ms");
        }
    }
    Ok(())
}
