//! The built-in starter tables: about 150–200 rows over every built
//! family, each cited or estimated with a reason, names that agree with
//! their lead counts, explicit size codes, and refused malformed tables.
use super::*;
use crate::library::package_form::{ChipKind, LeadSides, PackageForm};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn find<'a>(rows: &'a [StarterRow], name: &str) -> Fallible<&'a StarterRow> {
    rows.iter()
        .find(|r| r.name == name)
        .ok_or_else(|| format!("no row {name}").into())
}

#[test]
fn the_tables_parse_into_rows_over_every_family() -> Fallible<()> {
    let rows = rows().map_err(|e| e.to_owned())?;
    assert!((140..=200).contains(&rows.len()), "{} rows", rows.len());
    let mut per_family = BTreeMap::new();
    for row in rows {
        let family = row.definition.family.map(|f| f.family);
        *per_family.entry(family).or_insert(0) += 1;
        match &row.definition.citation {
            Some(c) => assert!(!c.document.trim().is_empty(), "{}", row.name),
            None => assert!(
                row.note
                    .as_deref()
                    .is_some_and(|n| n.starts_with("Estimated")),
                "an uncited row says it is estimated: {}",
                row.name
            ),
        }
        assert_eq!(row.definition.provenance.licence.as_deref(), Some(LICENCE));
        assert_eq!(row.definition.provenance.origin, None);
        assert_eq!(
            row.definition.values.range_problems(),
            Vec::<String>::new(),
            "{}",
            row.name
        );
    }
    for family in FamilyKind::ALL {
        assert!(
            per_family.get(&Some(family)).copied().unwrap_or(0) >= 5,
            "{family:?}: {per_family:?}"
        );
    }
    Ok(())
}

/// A citation without the drawing's revision and page is reported as
/// unverified, never as a checked citation.
#[test]
fn citations_without_revision_and_page_read_unverified() -> Fallible<()> {
    for row in rows().map_err(|e| e.to_owned())? {
        let Some(c) = &row.definition.citation else {
            assert_eq!(row.status(), "estimated", "{}", row.name);
            continue;
        };
        let expected = if c.checked() {
            "cited"
        } else {
            "cited_unverified"
        };
        assert_eq!(row.status(), expected, "{}", row.name);
        if !c.checked() {
            assert!(
                row.provenance_note().contains("citation unverified"),
                "{}",
                row.name
            );
        }
    }
    Ok(())
}

/// The name's pin count is the form's lead count, so a typo in a table
/// shows up as a mismatch, not as a wrong footprint.
#[test]
fn names_agree_with_lead_counts() -> Fallible<()> {
    let digits = |s: &str| -> Option<u64> {
        s.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()
    };
    let mut checked = 0;
    for row in rows().map_err(|e| e.to_owned())? {
        let leads = row.definition.values.lead_count();
        let name = &row.name;
        let family = row.definition.family.map(|f| f.family);
        // Outlines whose names carry no lead count: the registered counts
        // (a tab counts as a lead).
        let named = [
            ("SOT-223", 4),
            ("SOT-89", 4),
            ("DPAK (TO-252)", 3),
            ("D2PAK (TO-263)", 3),
        ];
        let two_lead = matches!(
            &row.definition.values,
            PackageForm::Tht(t) if leads == 2 && t.per_row() == 2
        );
        if let Some((_, count)) = named.iter().find(|(n, _)| n == name) {
            assert_eq!(leads, *count, "{name}");
        } else if name.starts_with("Chip") || family == Some(FamilyKind::Molded) || two_lead {
            assert_eq!(leads, 2, "{name}");
        } else if let Some(rest) = name
            .strip_prefix("SOT-23-")
            .or_else(|| name.strip_prefix("SC-70-"))
            .or_else(|| name.strip_prefix("TO-220-"))
            .or_else(|| name.strip_prefix("TO-92-"))
        {
            assert_eq!(digits(rest), Some(leads), "{name}");
        } else if let Some((rows_text, rest)) = name
            .strip_prefix("Pin header ")
            .and_then(|s| s.split_once('×'))
        {
            let count = digits(rows_text).zip(digits(rest)).map(|(a, b)| a * b);
            assert_eq!(count, Some(leads), "{name}");
        } else {
            let rest = name.split_once('-').map(|(_, rest)| rest);
            assert_eq!(rest.and_then(digits), Some(leads), "{name}");
        }
        checked += 1;
    }
    assert!(checked >= 180, "{checked}");
    Ok(())
}

/// Chip rows carry both size codes explicitly, the metric code is the
/// body's length and width in tenths of a millimetre, and the codes the
/// two systems share (0402, 0603) are both listed.
#[test]
fn chip_size_codes_are_explicit_and_match_the_body() -> Fallible<()> {
    let rows = rows().map_err(|e| e.to_owned())?;
    let mut imperial = BTreeSet::new();
    let mut metric = BTreeSet::new();
    for row in rows {
        let size = row.definition.size_code.as_ref();
        let PackageForm::Chip(chip) = &row.definition.values else {
            assert!(size.is_none(), "only chips have size codes: {}", row.name);
            continue;
        };
        let size = size.ok_or_else(|| format!("{} has no size code", row.name))?;
        imperial.insert(size.imperial.clone());
        metric.insert(size.metric.clone());
        // Metric codes name the nominal size; a drawing's nominal may
        // differ by a tenth (3.1 mm for a "32" resistor length).
        let tenths = |v: f64| (v * 10.0).round();
        let code = |i: usize| {
            size.metric
                .get(2 * i..2 * i + 2)
                .and_then(|t| t.parse::<f64>().ok())
        };
        let (l, w) = (code(0).ok_or("metric code")?, code(1).ok_or("metric code")?);
        assert!(
            size.metric.len() == 4
                && (l - tenths(chip.length)).abs() <= 1.0
                && (w - tenths(chip.width)).abs() <= 1.0,
            "{}",
            row.name
        );
        assert!(
            row.definition.aliases.contains(&size.imperial),
            "{}",
            row.name
        );
    }
    for shared in ["0402", "0603"] {
        assert!(
            imperial.contains(shared) && metric.contains(shared),
            "{shared}"
        );
    }
    let r0402 = find(rows, "Chip resistor 0402 (1005 metric)")?;
    assert!(
        matches!(&r0402.definition.values, PackageForm::Chip(c) if c.kind == ChipKind::Resistor)
    );
    Ok(())
}

#[test]
fn known_outlines_keep_their_registered_values() -> Fallible<()> {
    let rows = rows().map_err(|e| e.to_owned())?;
    let soic = find(rows, "SOIC-8 3.9 mm")?;
    let PackageForm::GullWing(g) = &soic.definition.values else {
        return Err("SOIC-8 is gull-wing".into());
    };
    assert_eq!(
        (g.sides, g.leads_per_side, g.pitch),
        (LeadSides::Two, [4, 0], 1.27)
    );
    assert_eq!((g.body_length, g.body_width, g.span[0]), (4.90, 3.90, 6.00));
    assert!(
        soic.definition
            .citation
            .as_ref()
            .is_some_and(|c| c.document.contains("MS-012"))
    );
    let qfn = find(rows, "QFN-32 5×5 0.5 EP 3.45")?;
    assert_eq!(qfn.definition.values.lead_count(), 32);
    assert_eq!(qfn.definition.values.terminal_counts(), vec![32, 33]);
    assert!(
        qfn.note
            .as_deref()
            .is_some_and(|n| n.contains("exposed pad"))
    );
    let lqfp = find(rows, "LQFP-128 14×20 0.5")?;
    assert_eq!(lqfp.definition.values.envelope().map(|e| e[0]), Some(22.0));
    let header = find(rows, "Pin header 2×5 1.27 mm")?;
    assert_eq!(header.status(), "estimated");
    assert_eq!(header.definition.skeleton, SkeletonHint::Connector);
    let dip = find(rows, "DIP-8 300 mil")?;
    assert!(matches!(&dip.definition.values, PackageForm::Tht(t) if t.lead_pitch == 2.54));
    assert_eq!(find(rows, "SOT-89")?.definition.values.lead_count(), 4);
    Ok(())
}

#[test]
fn row_is_found_by_its_unique_name() -> Fallible<()> {
    assert_eq!(row("SOIC-8 3.9 mm")?.file, "gull_wing.json");
    assert!(matches!(row("No such row"), Err(Error::NotFound(_))));
    Ok(())
}

#[test]
fn malformed_tables_are_refused() -> Fallible<()> {
    let base = json!({
        "format": 1, "family": "chip", "family_rev": 1,
        "citations": {"d": {"document": "A drawing", "document_revision": null, "page": null}},
        "rows": [{"name": "R", "description": "d", "aliases": [], "skeleton": {"skeleton": "blank"},
                  "cite": "d", "values": {"family": "chip", "kind": "resistor", "length": 1.0,
                  "width": 0.5, "height": 0.35, "termination": 0.25}}]
    });
    let parse = |v: &Value| parse_table("t.json", &v.to_string());
    assert_eq!(parse(&base)?.len(), 1);
    let refused = |edit: &dyn Fn(&mut Value), why: &str| {
        let mut v = base.clone();
        edit(&mut v);
        assert!(parse(&v).is_err(), "{why}");
    };
    refused(&|v| v["format"] = json!(2), "unsupported format");
    refused(&|v| v["rows"][0]["extra"] = json!(1), "unknown key");
    refused(
        &|v| v["rows"][0]["cite"] = json!("missing"),
        "unknown citation",
    );
    refused(
        &|v| {
            v["rows"][0]["cite"] = Value::Null;
            v["citations"] = json!({});
        },
        "estimated row without a note",
    );
    refused(
        &|v| v["rows"][0]["values"]["termination"] = json!(0.6),
        "range problem in a built family",
    );
    refused(
        &|v| v["family"] = json!("header"),
        "values of another family",
    );
    refused(
        &|v| v["rows"][0]["aliases"] = json!(["x".repeat(MAX_ALIAS + 1)]),
        "alias bound",
    );
    refused(&|v| v["rows"][0]["name"] = json!(""), "empty name");
    assert!(parse_table("t.json", &" ".repeat(MAX_TABLE_BYTES + 1)).is_err());
    Ok(())
}
