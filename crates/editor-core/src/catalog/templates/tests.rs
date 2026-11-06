//! Slice B1 behaviour: rows in the catalog, `new_from_template`,
//! `package_from_template` and its plan, `save_package_row` and the row
//! publish findings, all through the public engine.
use super::*;
use crate::diagnostics::{Finding, ObjectRef};
use crate::ids::{DefinitionKind, FootprintRef, LibraryId};
use crate::library::bundle::{Bundle, RevisionRecord};
use crate::library::package_form::{ChipForm, ChipKind, LeadSides, NoLeadForm};
use crate::library::samples::Preset;
use crate::library::symbol_style::{self, ResistorStyle, SymbolConventions, SymbolKind};
use crate::library::templates::catalog::{
    BUILT_FAMILY_REV, FamilyRef, LandValues, NewFromTemplate, PackageFromTemplate, PackagePlan,
    Renumber, RowFrom, RowSource, SavePackageRow, TerminalMatch,
};
use crate::library::{Definition, DefinitionMeta, DraftEdit, PadShape};
use crate::{Engine, LibraryCommand, Operation, Outcome, Query, QueryResult};
use std::sync::Arc;

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn meta(name: &str) -> DefinitionMeta {
    DefinitionMeta {
        name: name.into(),
        description: String::new(),
        provenance: "Original test row.".into(),
    }
}

fn chip(kind: ChipKind, size: [f64; 3]) -> PackageForm {
    PackageForm::Chip(Box::new(ChipForm::new(kind, size)))
}

fn row(values: PackageForm, aliases: &[&str]) -> PackageRowDefinition {
    PackageRowDefinition {
        family: FamilyKind::of(&values).map(|family| FamilyRef {
            family,
            family_rev: BUILT_FAMILY_REV,
        }),
        skeleton: FamilyKind::default_skeleton(&values),
        values,
        aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
        ..PackageRowDefinition::default()
    }
}

fn code(imperial: &str, metric: &str) -> Option<SizeCode> {
    Some(SizeCode {
        imperial: imperial.into(),
        metric: metric.into(),
    })
}

fn qfn32() -> PackageForm {
    PackageForm::NoLead(Box::new(NoLeadForm {
        sides: LeadSides::Four,
        terminals_per_side: [8, 8],
        pitch: 0.5,
        body_length: 5.0,
        body_width: 5.0,
        body_height: 0.9,
        terminal_length: 0.4,
        terminal_width: 0.25,
        exposed_pad: Some([3.45, 3.45]),
    }))
}

fn apply(engine: &mut Engine, command: LibraryCommand) -> crate::Result<Outcome> {
    engine.apply(engine.revision(), Operation::Library(command))
}

/// Publish one row and return its exact revision.
fn publish_row(
    engine: &mut Engine,
    name: &str,
    definition: PackageRowDefinition,
) -> Fallible<PackageRowRef> {
    let Outcome::DraftCreated { id } = apply(
        engine,
        LibraryCommand::NewDraft {
            meta: meta(name),
            definition: Definition::PackageRow(Box::new(definition)),
        },
    )?
    else {
        return Err("a draft".into());
    };
    apply(
        engine,
        LibraryCommand::Publish {
            drafts: vec![id],
            close: false,
        },
    )?;
    Ok(PackageRowRef::new(id, crate::ids::Revision::FIRST))
}

/// The project's rows and the custom entries, without the built-in
/// starter rows (tested on their own below).
fn templates(engine: &Engine, query: &str) -> Fallible<TemplateReport> {
    templates_with(engine, query, &project_only())
}

fn project_only() -> CatalogContext {
    CatalogContext {
        without_starter_rows: true,
        ..CatalogContext::default()
    }
}

fn templates_with(
    engine: &Engine,
    query: &str,
    context: &CatalogContext,
) -> Fallible<TemplateReport> {
    match engine.query_with(
        Query::Templates {
            query: query.into(),
            limit: None,
        },
        context,
    )? {
        QueryResult::Templates(report) => Ok(report),
        other => Err(format!("unexpected {other:?}").into()),
    }
}

fn names(report: &TemplateReport) -> Vec<&str> {
    report.hits.iter().map(|h| h.name.as_str()).collect()
}

/// A project with rows of several families, both 0402 meanings and a
/// cited QFN-32 with land overrides.
fn rows_engine() -> Fallible<Engine> {
    let mut engine = Engine::default();
    let mut imperial = row(chip(ChipKind::Resistor, [1.0, 0.5, 0.35]), &["R0402"]);
    imperial.size_code = code("0402", "1005");
    publish_row(&mut engine, "Chip 0402 (1005 metric)", imperial)?;
    let mut metric = row(chip(ChipKind::Capacitor, [0.4, 0.2, 0.2]), &[]);
    metric.size_code = code("01005", "0402");
    publish_row(&mut engine, "Chip 01005 (0402 metric)", metric)?;
    publish_row(
        &mut engine,
        "SOIC-8 narrow",
        row(Preset::Soic8.form(), &["SO-8", "8-SOIC"]),
    )?;
    let mut qfn = row(qfn32(), &["VQFN-32"]);
    qfn.citation = Some(RowCitation {
        document: "Original test outline drawing QX-32".into(),
        document_revision: Some("B".into()),
        page: Some(4),
    });
    qfn.lands = Some(LandValues {
        pad_length: Some(0.7),
        pad_width: Some(0.28),
        gap: Some(4.9),
        exposed_pad: Some([3.3, 3.3]),
    });
    publish_row(&mut engine, "QFN-32 5x5 0.5 EP3.45", qfn)?;
    publish_row(&mut engine, "QFN-16 3x3", row(Preset::Qfn16.form(), &[]))?;
    Ok(engine)
}

#[test]
fn rows_and_custom_entries_are_listed_with_their_provenance() -> Fallible<()> {
    let engine = rows_engine()?;
    let all = templates(&engine, "")?;
    // Five project rows first, then the eight built families' custom
    // entries.
    assert_eq!(
        all.hits.len(),
        5 + FamilyKind::ALL.len(),
        "{:?}",
        names(&all)
    );
    assert!(all.hits[..5].iter().all(|h| h.row.is_some()));
    assert!(all.hits[5..].iter().all(TemplateHit::is_custom));
    let qfn = all
        .hits
        .iter()
        .find(|h| h.name.starts_with("QFN-32"))
        .ok_or("the QFN-32 row")?;
    assert!(qfn.cited && qfn.lands_cited);
    assert_eq!(qfn.citation.as_ref().and_then(|c| c.page), Some(4));
    let soic = all
        .hits
        .iter()
        .find(|h| h.name == "SOIC-8 narrow")
        .ok_or("soic")?;
    assert!(
        !soic.cited && !soic.lands_cited,
        "uncited rows are estimated"
    );
    assert_eq!(soic.skeleton, SkeletonHint::IcBox);
    Ok(())
}

#[test]
fn shape_filters_read_typed_values() -> Fallible<()> {
    let engine = rows_engine()?;
    let soic = templates(&engine, "soic pins:8")?;
    // The row, then the gull-wing custom entry (found by its outline
    // names, "SOIC / SOP").
    assert_eq!(names(&soic), ["SOIC-8 narrow", "Custom gull-wing"]);
    assert_eq!(soic.chips, ["“soic”", "pins = 8"]);
    // An alias matches exactly.
    assert_eq!(names(&templates(&engine, "so-8")?), ["SOIC-8 narrow"]);
    let qfn = templates(&engine, "qfn body:5x5 pitch:0.5")?;
    assert_eq!(names(&qfn), ["QFN-32 5x5 0.5 EP3.45"]);
    // Units and comparisons: 0.5 mm is 19.7 mil; pins >= 16 excludes 8.
    assert_eq!(
        names(&templates(&engine, "pitch:19.685mil pins>=16 src:project")?),
        ["QFN-16 3x3", "QFN-32 5x5 0.5 EP3.45"]
    );
    // Body sizes match either way round and with a height.
    assert_eq!(
        names(&templates(&engine, "body:3.9x4.9x1.55 src:project")?),
        ["SOIC-8 narrow"]
    );
    assert!(templates(&engine, "body:5x5x2")?.hits.is_empty());
    // An exposed pad is not a lead.
    assert!(templates(&engine, "qfn pins:33")?.hits.is_empty());
    Ok(())
}

#[test]
fn a_size_code_matches_both_meanings_unless_a_system_is_named() -> Fallible<()> {
    let engine = rows_engine()?;
    let both = templates(&engine, "pkg:0402")?;
    assert_eq!(
        names(&both),
        ["Chip 0402 (1005 metric)", "Chip 01005 (0402 metric)"],
        "imperial 0402 and metric 0402 are both listed, nothing guessed; the imperial meaning first"
    );
    assert_eq!(
        names(&templates(&engine, "pkg:i0402")?),
        ["Chip 0402 (1005 metric)"]
    );
    assert_eq!(
        names(&templates(&engine, "pkg:m0402")?),
        ["Chip 01005 (0402 metric)"]
    );
    assert_eq!(templates(&engine, "pkg:m0402")?.chips, ["pkg: 0402 metric"]);
    let shown = both
        .hits
        .iter()
        .filter_map(|h| h.size_code.as_ref())
        .map(SizeCode::label);
    assert_eq!(
        shown.collect::<Vec<_>>(),
        [
            "0402 imperial · 1005 metric",
            "01005 imperial · 0402 metric"
        ]
    );
    Ok(())
}

/// Run 3, finding 1: the row the query's size or pin count names comes
/// first, so Enter creates it; the starter tables' own names and aliases
/// (a 1×40's "1x40 2.54" prefixes "1x4") must not outrank it.
#[test]
fn the_named_size_or_pin_count_ranks_first() -> Fallible<()> {
    let engine = Engine::default();
    let top = |query: &str| -> Fallible<String> {
        let report = templates_with(&engine, query, &CatalogContext::default())?;
        Ok(names(&report).first().ok_or("a hit")?.to_string())
    };
    assert_eq!(top("resistor 0603")?, "Chip resistor 0603 (1608 metric)");
    assert_eq!(top("capacitor 0603")?, "Chip capacitor 0603 (1608 metric)");
    assert_eq!(top("header 1x4")?, "Pin header 1×4 2.54 mm");
    assert_eq!(top("1x4")?, "Pin header 1×4 2.54 mm");
    assert_eq!(top("header 1×40")?, "Pin header 1×40 2.54 mm");
    assert!(top("header 2x5")?.starts_with("Pin header 2×5 "));
    let soic = top("soic 8")?;
    assert!(soic.starts_with("SOIC-8"), "{soic}");
    Ok(())
}

#[test]
fn template_queries_refuse_component_filters_and_bad_values() -> Fallible<()> {
    let engine = rows_engine()?;
    for query in [
        "kind:part",
        "has:mpn",
        "mpn:abc",
        "implements:12/1",
        "pins:many",
        "pitch:fast",
        "body:5",
        "body>5x5",
    ] {
        let result = engine.query(Query::Templates {
            query: query.into(),
            limit: None,
        });
        assert!(
            matches!(result, Err(Error::Invalid(_))),
            "{query}: {result:?}"
        );
    }
    let limited = engine.query(Query::Templates {
        query: String::new(),
        limit: Some(2),
    })?;
    let QueryResult::Templates(limited) = limited else {
        return Err("templates".into());
    };
    // Five project rows, every built-in starter row and one custom entry
    // per family match.
    let starters = crate::library::templates::starter::rows()
        .map_err(|e| e.to_owned())?
        .len();
    assert_eq!(
        (limited.hits.len(), limited.more),
        (2, 5 + starters + FamilyKind::ALL.len() - 2)
    );
    Ok(())
}

#[test]
fn folder_rows_are_indexed_and_searched_with_their_source_revision() -> Fallible<()> {
    let library = LibraryId(7);
    let mut definition = row(chip(ChipKind::Resistor, [1.6, 0.8, 0.45]), &["R0603"]);
    definition.size_code = code("0603", "1608");
    let record = RevisionRecord {
        lineage: SourceLineageId(42),
        revision: Revision::FIRST,
        kind: DefinitionKind::PackageRow,
        meta: meta("Chip 0603"),
        definition: Definition::PackageRow(Box::new(definition)),
        links: vec![],
        assets: vec![],
    };
    let catalog = super::super::source_catalog(library, "Packages", std::slice::from_ref(&record))?;
    assert_eq!(catalog.rows.len(), 1);
    assert!(
        catalog.entries.is_empty(),
        "rows are not components or Parts"
    );
    let context = CatalogContext {
        sources: vec![Arc::new(catalog)],
        ..project_only()
    };
    let engine = Engine::default();
    let found = templates_with(&engine, "pkg:i0603", &context)?;
    let [hit] = found.hits.as_slice() else {
        return Err(format!("one hit: {:?}", names(&found)).into());
    };
    assert_eq!(hit.source, Source::Library(library));
    assert_eq!(
        hit.source_row,
        Some(SourceRevision {
            lineage: SourceLineageId(42),
            revision: Revision::FIRST
        })
    );
    assert!(
        templates_with(&engine, "pkg:0603 src:project", &context)?
            .hits
            .is_empty()
    );
    // The host passes the folder row as a bundle; nothing is imported.
    let mut engine = Engine::default();
    let bundle = Bundle {
        library,
        name: "Packages".into(),
        root: SourceRevision {
            lineage: SourceLineageId(42),
            revision: Revision::FIRST,
        },
        records: vec![record],
        assets: vec![],
    };
    let Outcome::DraftsCreated { ids } = apply(
        &mut engine,
        LibraryCommand::NewFromTemplate(Box::new(NewFromTemplate {
            row: RowSource::Bundle {
                bundle: Box::new(bundle),
            },
            values: None,
            skeleton: SkeletonHint::TwoTerminal { polarised: false },
            name: "Resistor 0603".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        })),
    )?
    else {
        return Err("drafts".into());
    };
    assert_eq!(ids.len(), 5);
    assert!(engine.project().library.package_rows.is_empty());
    let component = engine.drafts().get(ids[0]).ok_or("component")?;
    let Definition::Component(c) = &component.definition else {
        return Err("component first".into());
    };
    assert_eq!(c.designator_prefix, "R");
    assert!(
        component.meta.provenance.contains("‘Chip 0603’"),
        "{}",
        component.meta.provenance
    );
    assert!(component.meta.provenance.contains("estimated values"));
    Ok(())
}

fn new_from(engine: &mut Engine, request: NewFromTemplate) -> Fallible<Vec<DefinitionId>> {
    match apply(engine, LibraryCommand::NewFromTemplate(Box::new(request)))? {
        Outcome::DraftsCreated { ids } => Ok(ids),
        other => Err(format!("unexpected {other:?}").into()),
    }
}

#[test]
fn a_component_from_a_cited_row_is_one_undo_step_and_publishes() -> Fallible<()> {
    let mut engine = rows_engine()?;
    let qfn = templates(&engine, "qfn pins:32")?
        .hits
        .first()
        .and_then(|h| h.row)
        .ok_or("the QFN-32 row")?;
    let before = engine.to_file();
    let revision = engine.revision();
    let ids = new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Project { row: qfn },
            values: None,
            skeleton: SkeletonHint::IcBox,
            name: "Controller QFN-32".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        },
    )?;
    assert_eq!(engine.revision(), revision + 1, "one operation");
    assert_eq!(ids.len(), 5);
    let kinds: Vec<DefinitionKind> = ids
        .iter()
        .filter_map(|id| engine.drafts().get(*id))
        .map(|d| d.definition.kind())
        .collect();
    assert_eq!(
        kinds,
        [
            DefinitionKind::Component,
            DefinitionKind::Symbol,
            DefinitionKind::Package,
            DefinitionKind::Footprint,
            DefinitionKind::Binding
        ]
    );
    let footprint = engine.drafts().get(ids[3]).ok_or("footprint")?;
    let Definition::Footprint(footprint) = &footprint.definition else {
        return Err("footprint".into());
    };
    assert_eq!(footprint.pads.len(), 33, "32 lands and the exposed pad");
    assert!(footprint.polarity.is_some(), "pin 1 is marked");
    // The cited lands override the estimated ones.
    let ep = footprint
        .pads
        .iter()
        .find(|p| p.number == "EP")
        .ok_or("EP")?;
    assert_eq!(ep.shape, PadShape::Rect { size: [3.3, 3.3] });
    let first = footprint.pads.first().ok_or("pad 1")?;
    assert_eq!(first.shape, PadShape::Rect { size: [0.28, 0.7] });
    assert!(
        (first.position[1] + 2.45).abs() < 1e-9,
        "{:?}",
        first.position
    );
    let component = engine.drafts().get(ids[0]).ok_or("component")?;
    assert!(
        component
            .meta
            .provenance
            .contains("Original test outline drawing QX-32 rev. B p. 4"),
        "{}",
        component.meta.provenance
    );
    apply(
        &mut engine,
        LibraryCommand::Publish {
            drafts: ids,
            close: false,
        },
    )?;
    engine.undo()?;
    engine.undo()?;
    assert_eq!(
        engine.to_file(),
        before,
        "one undo step removes all five drafts"
    );
    Ok(())
}

#[test]
fn every_custom_family_builds_a_publishable_component() -> Fallible<()> {
    for family in FamilyKind::ALL {
        let values = family.custom_values();
        let mut engine = Engine::default();
        let skeleton = FamilyKind::default_skeleton(&values);
        let ids = new_from(
            &mut engine,
            NewFromTemplate {
                row: RowSource::Custom { family },
                values: None,
                skeleton,
                name: format!("{} part", family.label()),
                designator_prefix: Some("X".into()),
                symbol: None,
                without_fields: false,
            },
        )?;
        let published = apply(
            &mut engine,
            LibraryCommand::Publish {
                drafts: ids.clone(),
                close: false,
            },
        );
        assert!(published.is_ok(), "{family:?}: {published:?}");
        let Some(Definition::Component(component)) = engine
            .project()
            .library
            .definition(crate::ids::DefinitionRef {
                kind: DefinitionKind::Component,
                id: ids[0],
                revision: Revision::FIRST,
            })
            .map(|(_, d)| d)
        else {
            return Err("published component".into());
        };
        assert_eq!(component.designator_prefix, "X");
    }
    Ok(())
}

#[test]
fn refused_template_requests_leave_the_document_unchanged() -> Fallible<()> {
    let mut engine = rows_engine()?;
    let soic = templates(&engine, "soic")?
        .hits
        .first()
        .and_then(|h| h.row)
        .ok_or("soic")?;
    let before = engine.to_file();
    let revision = engine.revision();
    let base = NewFromTemplate {
        row: RowSource::Project { row: soic },
        values: None,
        skeleton: SkeletonHint::IcBox,
        name: "U".into(),
        designator_prefix: None,
        symbol: None,
        without_fields: false,
    };
    let refused = [
        NewFromTemplate {
            skeleton: SkeletonHint::TwoTerminal { polarised: false },
            ..base.clone()
        },
        NewFromTemplate {
            values: Some(Preset::Qfn16.form()),
            ..base.clone()
        },
        NewFromTemplate {
            name: "  ".into(),
            ..base.clone()
        },
        NewFromTemplate {
            designator_prefix: Some("U 1".into()),
            ..base.clone()
        },
        NewFromTemplate {
            row: RowSource::Project {
                row: PackageRowRef::new(DefinitionId(999), Revision::FIRST),
            },
            ..base.clone()
        },
        NewFromTemplate {
            row: RowSource::Custom {
                family: FamilyKind::Grid,
            },
            values: Some(Preset::Header2x5.form()),
            ..base.clone()
        },
    ];
    for request in refused {
        let shown = format!("{request:?}");
        assert!(
            apply(
                &mut engine,
                LibraryCommand::NewFromTemplate(Box::new(request))
            )
            .is_err(),
            "{shown}"
        );
        assert_eq!(engine.revision(), revision);
    }
    assert_eq!(engine.to_file(), before);
    Ok(())
}

#[test]
fn edited_values_make_a_custom_package_with_estimated_lands() -> Fallible<()> {
    let mut engine = rows_engine()?;
    let qfn = templates(&engine, "qfn pins:32")?
        .hits
        .first()
        .and_then(|h| h.row)
        .ok_or("qfn")?;
    let PackageForm::NoLead(mut edited) = qfn32() else {
        return Err("no-lead".into());
    };
    edited.body_height = 0.75;
    let ids = new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Project { row: qfn },
            values: Some(PackageForm::NoLead(edited)),
            skeleton: SkeletonHint::Blank,
            name: "Thin QFN".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        },
    )?;
    let package = engine.drafts().get(ids[2]).ok_or("package")?;
    assert!(package.meta.provenance.starts_with("Custom values edited"));
    let footprint = engine.drafts().get(ids[3]).ok_or("footprint")?;
    let Definition::Footprint(footprint) = &footprint.definition else {
        return Err("footprint".into());
    };
    let ep = footprint
        .pads
        .iter()
        .find(|p| p.number == "EP")
        .ok_or("EP")?;
    assert_eq!(
        ep.shape,
        PadShape::Rect { size: [3.45, 3.45] },
        "row lands dropped"
    );
    let symbol = engine.drafts().get(ids[1]).ok_or("symbol")?;
    let Definition::Symbol(symbol) = &symbol.definition else {
        return Err("symbol".into());
    };
    assert!(
        symbol.units.iter().all(|u| u.pins.is_empty()),
        "a blank skeleton"
    );
    Ok(())
}

fn plan(engine: &Engine, request: &PackageFromTemplate) -> Fallible<PackagePlan> {
    match engine.query(Query::PackagePlan {
        request: Box::new(request.clone()),
    })? {
        QueryResult::PackagePlan(plan) => Ok(plan),
        other => Err(format!("unexpected {other:?}").into()),
    }
}

fn codes(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.code.as_str()).collect()
}

#[test]
fn package_from_template_previews_and_replaces_in_one_step() -> Fallible<()> {
    let mut engine = rows_engine()?;
    let ids = new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Custom {
                family: FamilyKind::GullWing,
            },
            values: None,
            skeleton: SkeletonHint::IcBox,
            name: "Bridge".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        },
    )?;
    let package = ids[2];
    let qfn = templates(&engine, "qfn-16")?
        .hits
        .first()
        .and_then(|h| h.row)
        .ok_or("qfn-16")?;
    let mut request = PackageFromTemplate {
        draft: package,
        row: RowSource::Project { row: qfn },
        values: None,
        replace: false,
        terminals: TerminalMatch::ByNumber,
        renumber: Vec::new(),
    };
    let before = engine.to_file();
    let revision = engine.revision();
    // The SOIC footprint has pads: refused without `replace`.
    let refused = apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(request.clone())),
    );
    assert!(
        matches!(&refused, Err(Error::Conflict(m)) if m.starts_with("footprint.template_not_empty")),
        "{refused:?}"
    );
    assert!(plan(&engine, &request).is_err());
    assert_eq!(
        (engine.revision(), engine.to_file()),
        (revision, before.clone())
    );
    request.replace = true;
    let previewed = plan(&engine, &request)?;
    assert_eq!(
        (
            previewed.terminals_before,
            previewed.terminals_after,
            previewed.pads_after
        ),
        (8, 17, 17)
    );
    assert_eq!(previewed.model_family_rev, Some(BUILT_FAMILY_REV));
    // SOIC-8 to QFN-16 + EP, matched by number: terminals 1–8 keep their
    // identities (and mappings); 9–16 and EP are new, named and unmapped.
    // Nothing is reused by position, so `binding.mapping_retained` (which
    // this test expected while terminals were kept by list order) is gone.
    assert_eq!(
        codes(&previewed.findings),
        ["package.terminals_unmatched", "binding.mapping_missing"]
    );
    assert_eq!(
        (
            previewed.terminals.strategy,
            previewed.terminals.kept,
            previewed.terminals.added,
            previewed.terminals.removed
        ),
        (TerminalMatch::ByNumber, 8, 9, 0)
    );
    assert!(
        previewed.findings[0]
            .message
            .contains("9, 10, 11, 12, 13, 14, 15, 16, EP"),
        "{}",
        previewed.findings[0].message
    );
    assert_eq!(engine.to_file(), before, "a plan changes nothing");
    let Outcome::PackageReplaced { plan: applied } = apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(request)),
    )?
    else {
        return Err("replaced".into());
    };
    assert_eq!(applied, previewed);
    assert_eq!(engine.revision(), revision + 1);
    let Some(Definition::Package(next)) = engine.drafts().get(package).map(|d| &d.definition)
    else {
        return Err("package".into());
    };
    assert!(matches!(next.form, PackageForm::NoLead(_)));
    assert_eq!(next.terminals.len(), 17);
    assert_eq!(
        next.terminals.last().map(|t| t.designation.as_str()),
        Some("EP")
    );
    // Kept terminals keep their identities; new ones are fresh.
    assert!(
        next.terminals
            .iter()
            .take(8)
            .zip(1..)
            .all(|(t, n)| t.id.0 == n)
    );
    let Some(Definition::Footprint(footprint)) = engine.drafts().get(ids[3]).map(|d| &d.definition)
    else {
        return Err("footprint".into());
    };
    assert_eq!(footprint.pads.len(), 17);
    assert!(
        footprint.pads.iter().all(|p| p.id.0 > 8),
        "pad identities are never reused"
    );
    engine.undo()?;
    assert_eq!(engine.to_file(), before);
    Ok(())
}

#[test]
fn a_package_row_is_saved_as_a_draft_and_publishes() -> Fallible<()> {
    let mut engine = Engine::default();
    let ids = new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Custom {
                family: FamilyKind::NoLead,
            },
            values: None,
            skeleton: SkeletonHint::IcBox,
            name: "QFN".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        },
    )?;
    let Outcome::DraftCreated { id } = apply(
        &mut engine,
        LibraryCommand::SavePackageRow(Box::new(SavePackageRow {
            from: RowFrom::Package { draft: ids[2] },
            name: "My QFN-16".into(),
            aliases: vec!["VQFN-16".into(), "vqfn-16".into()],
            provenance: Default::default(),
        })),
    )?
    else {
        return Err("a row draft".into());
    };
    let Some(Definition::PackageRow(saved)) = engine.drafts().get(id).map(|d| &d.definition) else {
        return Err("row".into());
    };
    assert_eq!(saved.aliases, ["VQFN-16"], "aliases fold to one");
    assert_eq!(
        saved.family,
        Some(FamilyRef {
            family: FamilyKind::NoLead,
            family_rev: BUILT_FAMILY_REV
        })
    );
    assert!(saved.citation.is_none(), "estimated until cited");
    apply(
        &mut engine,
        LibraryCommand::Publish {
            drafts: vec![id],
            close: false,
        },
    )?;
    assert_eq!(names(&templates(&engine, "my qfn")?), ["My QFN-16"]);
    // Generic values cannot be a row.
    let generic = apply(
        &mut engine,
        LibraryCommand::SavePackageRow(Box::new(SavePackageRow {
            from: RowFrom::Values {
                values: PackageForm::Generic,
            },
            name: "Envelope".into(),
            aliases: vec![],
            provenance: Default::default(),
        })),
    );
    assert!(generic.is_err());
    Ok(())
}

fn findings_of(definition: &PackageRowDefinition) -> Vec<String> {
    let mut out = Vec::new();
    definition.findings(DefinitionId(1), &mut out);
    out.into_iter().map(|f| f.code).collect()
}

#[test]
fn row_publish_findings_name_each_problem() -> Fallible<()> {
    assert!(findings_of(&row(Preset::Soic8.form(), &["SO-8"])).is_empty());
    let mut wrong = row(Preset::Soic8.form(), &[]);
    wrong.family = Some(FamilyRef {
        family: FamilyKind::Chip,
        family_rev: BUILT_FAMILY_REV,
    });
    assert_eq!(findings_of(&wrong), ["package_row.family"]);
    let mut newer = row(Preset::Soic8.form(), &[]);
    newer.family = newer.family.map(|f| FamilyRef { family_rev: 9, ..f });
    assert_eq!(findings_of(&newer), ["package_row.family"]);
    assert_eq!(
        findings_of(&PackageRowDefinition::default()),
        ["package_row.family"]
    );
    let range = row(chip(ChipKind::Resistor, [-1.0, 0.5, 0.3]), &[]);
    assert!(findings_of(&range).iter().all(|c| c == "package_row.range"));
    assert!(!findings_of(&range).is_empty());
    let aliases = row(Preset::Soic8.form(), &["SO-8", "so-8", " "]);
    assert_eq!(
        findings_of(&aliases),
        ["package_row.alias", "package_row.alias"]
    );
    let mut coded = row(Preset::Soic8.form(), &[]);
    coded.size_code = code("08", "");
    assert_eq!(
        findings_of(&coded),
        ["package_row.alias", "package_row.alias"]
    );
    let mut cited = row(Preset::Soic8.form(), &[]);
    cited.citation = Some(RowCitation {
        document: " ".into(),
        document_revision: None,
        page: Some(0),
    });
    assert_eq!(
        findings_of(&cited),
        ["package_row.provenance", "package_row.provenance"]
    );
    // Through the engine: a bad row does not publish, and the engine is
    // unchanged.
    let mut engine = Engine::default();
    let Outcome::DraftCreated { id } = apply(
        &mut engine,
        LibraryCommand::NewDraft {
            meta: meta("Bad"),
            definition: Definition::PackageRow(Box::new(wrong)),
        },
    )?
    else {
        return Err("draft".into());
    };
    let revision = engine.revision();
    assert!(
        apply(
            &mut engine,
            LibraryCommand::Publish {
                drafts: vec![id],
                close: false
            }
        )
        .is_err()
    );
    assert_eq!(engine.revision(), revision);
    Ok(())
}

#[test]
fn component_search_reads_package_shapes() -> Fallible<()> {
    let engine = Engine::showcase()?;
    let search = |query: &str| -> Fallible<Vec<String>> {
        match engine.query(Query::Search {
            query: query.into(),
            limit: None,
            include_drafts: false,
        })? {
            QueryResult::Search(report) => {
                Ok(report.results.into_iter().map(|h| h.entry.name).collect())
            }
            other => Err(format!("unexpected {other:?}").into()),
        }
    };
    assert_eq!(search("pins:8")?, ["Interface bridge (SOIC-8)"]);
    assert_eq!(search("pins>=10 pitch:0.5")?, ["Controller (QFN-16)"]);
    assert_eq!(search("body:3x3")?, ["Controller (QFN-16)"]);
    assert_eq!(search("pins:2 body:1.6x0.8")?, ["Resistor 0603"]);
    Ok(())
}

#[test]
fn preset_rows_are_valid_estimated_rows() {
    let rows = crate::library::templates::catalog::preset_rows();
    assert_eq!(rows.len(), Preset::ALL.len());
    for (name, definition) in rows {
        assert!(findings_of(&definition).is_empty(), "{name}");
        assert!(!definition.cited(), "{name}");
    }
}

/// A fresh engine with the hosts' default context lists the built-in
/// starter rows: the gallery is populated with no library source added.
#[test]
fn a_fresh_engine_lists_the_built_in_starter_rows() -> Fallible<()> {
    let engine = Engine::default();
    let all = |query: &str| templates_with(&engine, query, &CatalogContext::default());
    for query in [
        "qfn 48",
        "pitch:0.5",
        "0603",
        "dip",
        "bga",
        "sot-89",
        "to-220",
        "to-92",
        "sc-70",
        "lqfp",
        "sod-123",
        "header",
    ] {
        let found = all(query)?;
        let first = found.hits.first().ok_or(format!("{query}: no hits"))?;
        assert!(first.is_starter(), "{query}: {}", first.name);
        // Custom entries come after every row hit.
        let custom = found.hits.iter().position(TemplateHit::is_custom);
        if let Some(at) = custom {
            assert!(
                found.hits[at..].iter().all(TemplateHit::is_custom),
                "{query}: {:?}",
                names(&found)
            );
        }
    }
    // Numbers in names order naturally within a tier.
    let pitch = all("pitch:0.5")?;
    let at = |name: &str| names(&pitch).iter().position(|n| *n == name);
    assert!(
        at("DFN-8 2×2 0.5") < at("DFN-10 3×3 0.5"),
        "{:?}",
        names(&pitch)
    );
    // A bare size code lists its imperial meaning first.
    let code = all("0603")?;
    let first = code.hits.first().and_then(|h| h.size_code.clone());
    assert_eq!(first.map(|c| c.imperial), Some("0603".into()));
    // A search for a family's outline name never comes back empty.
    for query in ["pdip", "wlcsp", "melf", "dpak", "son"] {
        assert!(!all(query)?.hits.is_empty(), "{query}");
    }
    // Starter hits build from their name; cited ones without a drawing
    // revision and page read unverified.
    let soic = all("soic-8")?;
    let hit = soic.hits.first().ok_or("soic")?;
    assert!(hit.cited && !hit.citation_checked, "{}", hit.name);
    assert!(
        !templates(&engine, "soic-8")?
            .hits
            .iter()
            .any(TemplateHit::is_starter)
    );
    assert!(
        all("src:project")?.hits.is_empty(),
        "starter rows are not project rows"
    );
    Ok(())
}

/// Copper of two pads of one footprint overlaps (touching is allowed).
fn overlapping(pads: &[crate::library::Pad]) -> Option<(String, String)> {
    for (i, a) in pads.iter().enumerate() {
        for b in &pads[i + 1..] {
            let (p, q) = (a.bounds(), b.bounds());
            let eps = 1e-9;
            if p.min[0] < q.max[0] - eps
                && q.min[0] < p.max[0] - eps
                && p.min[1] < q.max[1] - eps
                && q.min[1] < p.max[1] - eps
            {
                return Some((a.number.clone(), b.number.clone()));
            }
        }
    }
    None
}

/// Every family builds family-correct lands: plated holes for
/// through-hole parts, round lands smaller than the pitch and A1 names
/// for grids, a tab land for tabbed packages, no overlapping copper, pad
/// numbers equal to the terminals' designations, every terminal landed
/// once, and exactly the pads the gallery previews
/// (`samples::template_lands`).
#[test]
fn every_template_row_builds_family_correct_lands() -> Fallible<()> {
    use crate::library::{PadKind, samples};
    let mut sources: Vec<(String, RowSource, PackageRowDefinition)> = FamilyKind::ALL
        .into_iter()
        .map(|family| {
            (
                format!("custom {family:?}"),
                RowSource::Custom { family },
                row(family.custom_values(), &[]),
            )
        })
        .collect();
    for starter in crate::library::templates::starter::rows().map_err(|e| e.to_owned())? {
        sources.push((
            starter.name.clone(),
            RowSource::Starter {
                name: starter.name.clone(),
            },
            starter.definition.clone(),
        ));
    }
    assert!(sources.len() > 150);
    for (name, source, definition) in sources {
        let mut engine = Engine::default();
        let ids = new_from(
            &mut engine,
            NewFromTemplate {
                row: source,
                values: None,
                skeleton: definition.skeleton,
                name: name.clone(),
                designator_prefix: None,
                symbol: None,
                without_fields: false,
            },
        )?;
        let draft = |k: usize| {
            ids.get(k)
                .and_then(|id| engine.drafts().get(*id))
                .map(|d| d.definition.clone())
        };
        let (Some(Definition::Package(package)), Some(Definition::Footprint(footprint))) =
            (draft(2), draft(3))
        else {
            return Err(format!("{name}: package and footprint drafts").into());
        };
        if let Some(Definition::Symbol(symbol)) = draft(1) {
            for pin in symbol.units.iter().flat_map(|u| &u.pins) {
                assert!(
                    crate::library::graphics::on_grid(pin.position, engine.drafts().grid.symbol_mm),
                    "{name}: a symbol pin off the authoring grid"
                );
            }
        }
        let pads = &footprint.pads;
        // A module's ground array lands one terminal with several pads.
        let ground = match &package.form {
            PackageForm::Module(m) => usize::try_from(m.ground_pads()).unwrap_or(0),
            _ => 0,
        };
        // A receptacle's shell has four stakes, and two peg holes land
        // nothing (`a_usb_c_receptacle_…` checks them).
        let receptacle = matches!(package.form, PackageForm::UsbC(_));
        assert_eq!(
            pads.len(),
            package.terminals.len() + ground.saturating_sub(1) + if receptacle { 5 } else { 0 },
            "{name}"
        );
        let mut landed = std::collections::BTreeSet::new();
        for pad in pads.iter().filter(|_| !receptacle) {
            let terminal = pad.terminal.and_then(|t| package.terminal(t));
            let terminal =
                terminal.ok_or(format!("{name}: pad {} lands no terminal", pad.number))?;
            let shared =
                ground > 0 && pads.iter().filter(|p| p.terminal == pad.terminal).count() == ground;
            assert!(
                landed.insert(terminal.id) || shared,
                "{name}: {} twice",
                pad.number
            );
            assert_eq!(pad.number, terminal.designation, "{name}");
            let through = matches!(package.form, PackageForm::Tht(_) | PackageForm::Header(_));
            if through {
                assert_eq!(pad.kind, PadKind::PlatedThrough, "{name}");
                assert!(pad.hole.as_ref().is_some_and(|h| h.plated), "{name}");
            } else {
                assert_eq!(pad.kind, PadKind::Smd, "{name}");
                assert!(pad.hole.is_none(), "{name}");
            }
        }
        if let Some((a, b)) = overlapping(pads) {
            return Err(format!("{name}: pads {a} and {b} overlap").into());
        }
        match &package.form {
            PackageForm::Grid(g) => {
                assert_eq!(
                    pads.first().map(|p| p.number.as_str()),
                    Some("A1"),
                    "{name}"
                );
                for pad in pads {
                    let PadShape::Circle { diameter } = pad.shape else {
                        return Err(format!("{name}: a round grid land").into());
                    };
                    assert!(diameter < g.pitch[0].min(g.pitch[1]), "{name}");
                }
            }
            PackageForm::Tabbed(t) if t.tab.is_some() => {
                let (tab, lead) = (pads.last().ok_or("tab")?, pads.first().ok_or("lead")?);
                assert!(
                    tab.bounds().size()[0] > lead.bounds().size()[0],
                    "{name}: the tab land is wider than a lead's"
                );
            }
            _ => {
                for pad in pads {
                    let size = pad.bounds().size();
                    assert!(
                        !(size[0] == 1.0 && size[1] == 1.0),
                        "{name}: no placeholder 1 mm lands"
                    );
                }
            }
        }
        let terminals: Vec<_> = package.terminals.iter().map(|t| t.id).collect();
        let lands = definition.lands.as_ref();
        let preview = samples::template_lands(&package.form, &terminals, lands)
            .ok_or(format!("{name}: preview lands"))?;
        assert_eq!(&preview, pads, "{name}: the preview is what Create builds");
        let published = apply(
            &mut engine,
            LibraryCommand::Publish {
                drafts: ids,
                close: false,
            },
        );
        assert!(published.is_ok(), "{name}: {published:?}");
    }
    Ok(())
}

/// A component built from a row records the row's exact identity.
#[test]
fn built_drafts_record_the_row_identity() -> Fallible<()> {
    let mut engine = rows_engine()?;
    let qfn = templates(&engine, "qfn pins:32")?
        .hits
        .first()
        .and_then(|h| h.row)
        .ok_or("the QFN-32 row")?;
    let ids = new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Project { row: qfn },
            values: None,
            skeleton: SkeletonHint::IcBox,
            name: "U".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        },
    )?;
    let provenance = |engine: &Engine, id: DefinitionId| {
        engine
            .drafts()
            .get(id)
            .map(|d| d.meta.provenance.clone())
            .unwrap_or_default()
    };
    let expected = format!("[project row {} r{}]", qfn.id, qfn.revision);
    let text = provenance(&engine, ids[0]);
    assert!(text.contains(&expected), "{text}");
    let ids = new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Starter {
                name: "SOIC-8 3.9 mm".into(),
            },
            values: None,
            skeleton: SkeletonHint::IcBox,
            name: "U2".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        },
    )?;
    let text = provenance(&engine, ids[0]);
    assert!(
        text.contains("[built-in starter table gull_wing.json format 1]")
            && text.contains("citation unverified"),
        "{text}"
    );
    Ok(())
}

/// Replacing a package re-designates every terminal from the new form:
/// no repeated `EP`, no missing number.
#[test]
fn a_replaced_package_is_designated_from_the_new_form() -> Fallible<()> {
    let mut engine = Engine::default();
    let ids = new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Custom {
                family: FamilyKind::NoLead,
            },
            // A DFN-8 with an exposed pad: terminal 9 is `EP`.
            values: Some(PackageForm::NoLead(Box::new(NoLeadForm {
                sides: LeadSides::Two,
                terminals_per_side: [4, 0],
                pitch: 0.5,
                body_length: 3.0,
                body_width: 3.0,
                body_height: 0.75,
                terminal_length: 0.4,
                terminal_width: 0.25,
                exposed_pad: Some([1.5, 2.0]),
            }))),
            skeleton: SkeletonHint::IcBox,
            name: "U".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        },
    )?;
    let request = PackageFromTemplate {
        draft: ids[2],
        row: RowSource::Custom {
            family: FamilyKind::NoLead,
        },
        values: Some(qfn32()),
        replace: true,
        terminals: TerminalMatch::ByNumber,
        renumber: Vec::new(),
    };
    apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(request)),
    )?;
    let Some(Definition::Package(package)) =
        engine.drafts().get(ids[2]).map(|d| d.definition.clone())
    else {
        return Err("package".into());
    };
    let designations: Vec<_> = package
        .terminals
        .iter()
        .map(|t| t.designation.clone())
        .collect();
    let expected: Vec<String> = (1..=32)
        .map(|n: u32| n.to_string())
        .chain(["EP".to_owned()])
        .collect();
    assert_eq!(designations, expected);
    Ok(())
}

/// A replacement keeps a terminal (and the binding's mapping of it) only
/// where the new form gives its number, whatever the list order; a
/// number left over on either side is reported and stays unmapped,
/// never filled by position.
#[test]
fn terminals_carry_over_by_number_and_a_mismatch_is_reported() -> Fallible<()> {
    let mut engine = Engine::default();
    let ids = bridge(&mut engine)?;
    // The SOIC-8's terminals numbered in reverse list order (as an import
    // whose rows are not in pin order leaves them), and its first terminal
    // given a number the no-lead form does not have.
    let draft = engine.drafts().get(ids[2]).ok_or("package")?.clone();
    let Definition::Package(mut package) = draft.definition else {
        return Err("package".into());
    };
    let numbers = ["X", "7", "6", "5", "4", "3", "2", "1"];
    for (terminal, number) in package.terminals.iter_mut().zip(numbers) {
        terminal.designation = number.into();
    }
    let ids_by_number: Vec<(String, u32)> = package
        .terminals
        .iter()
        .map(|t| (t.designation.clone(), t.id.0))
        .collect();
    apply(
        &mut engine,
        LibraryCommand::Edit {
            edits: vec![DraftEdit {
                id: ids[2],
                meta: draft.meta,
                definition: Definition::Package(package),
            }],
        },
    )?;
    let request = PackageFromTemplate {
        draft: ids[2],
        row: RowSource::Custom {
            family: FamilyKind::NoLead,
        },
        values: Some(qfn32()),
        replace: true,
        terminals: TerminalMatch::ByNumber,
        renumber: Vec::new(),
    };
    let before = engine.to_file();
    let previewed = plan(&engine, &request)?;
    assert_eq!(engine.to_file(), before, "a plan changes nothing");
    // A renumbering must name one current and one new number; else it is
    // refused and nothing changes.
    for (from, to) in [("Z", "8"), ("X", "Z"), ("", "8")] {
        let mut bad = request.clone();
        bad.renumber = vec![Renumber {
            from: from.into(),
            to: to.into(),
        }];
        let refused = apply(
            &mut engine,
            LibraryCommand::PackageFromTemplate(Box::new(bad)),
        );
        assert!(
            matches!(&refused, Err(Error::Invalid(m)) if m.starts_with("template.renumber")),
            "{refused:?}"
        );
        assert_eq!(engine.to_file(), before);
    }
    assert_eq!(
        (
            previewed.terminals.kept,
            previewed.terminals.added,
            previewed.terminals.removed,
            previewed.terminals.renumbered
        ),
        (7, 26, 1, 0)
    );
    assert_eq!(
        previewed.terminals.summary,
        "Terminals matched by number: 7 kept, 26 added, 1 removed."
    );
    assert_eq!(
        codes(&previewed.findings),
        [
            "package.terminals_unmatched",
            "binding.mapping_missing",
            "binding.mapping_unknown"
        ]
    );
    let unmatched = &previewed.findings[0].message;
    assert!(
        unmatched.contains("new terminal(s) 8, 9, 10,")
            && unmatched.contains("and 10 more")
            && unmatched.contains("current terminal(s) X are not in the new form"),
        "{unmatched}"
    );
    apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(request)),
    )?;
    let Some(Definition::Package(next)) = engine.drafts().get(ids[2]).map(|d| d.definition.clone())
    else {
        return Err("package".into());
    };
    // Numbers 1–7 keep the identities they had, now in lead order; the
    // terminal numbered X is gone and 8 is new (not X's identity).
    for (number, id) in &ids_by_number {
        let kept = next.terminals.iter().find(|t| t.id.0 == *id);
        if number == "X" {
            assert!(kept.is_none(), "X is removed");
        } else {
            assert_eq!(kept.map(|t| t.designation.as_str()), Some(number.as_str()));
        }
    }
    let eight = next
        .terminals
        .iter()
        .find(|t| t.designation == "8")
        .ok_or("8")?;
    assert!(ids_by_number.iter().all(|(_, id)| *id != eight.id.0));
    assert_eq!(
        next.terminals.first().map(|t| t.designation.as_str()),
        Some("1"),
        "lead order"
    );
    Ok(())
}

/// Position is an explicit request option: it keeps the terminals in list
/// order, renumbers them from the new form and says so.
#[test]
fn matching_by_position_is_explicit_and_flagged() -> Fallible<()> {
    let mut engine = Engine::default();
    let ids = bridge(&mut engine)?;
    let draft = engine.drafts().get(ids[2]).ok_or("package")?.clone();
    let Definition::Package(mut package) = draft.definition else {
        return Err("package".into());
    };
    package.terminals.reverse();
    let first = package.terminals.first().map(|t| t.id).ok_or("terminal")?;
    apply(
        &mut engine,
        LibraryCommand::Edit {
            edits: vec![DraftEdit {
                id: ids[2],
                meta: draft.meta,
                definition: Definition::Package(package),
            }],
        },
    )?;
    let request = PackageFromTemplate {
        draft: ids[2],
        row: RowSource::Custom {
            family: FamilyKind::GullWing,
        },
        values: None,
        replace: true,
        terminals: TerminalMatch::ByPosition,
        renumber: Vec::new(),
    };
    let previewed = plan(&engine, &request)?;
    assert_eq!(previewed.terminals.strategy, TerminalMatch::ByPosition);
    // Eight kept in list order, every one with a new number.
    assert_eq!(
        previewed.terminals.summary,
        "Terminals matched by position: 8 kept, 0 added, 0 removed, 8 renumbered."
    );
    assert_eq!(codes(&previewed.findings), ["binding.mapping_retained"]);
    apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(request)),
    )?;
    let Some(Definition::Package(next)) = engine.drafts().get(ids[2]).map(|d| d.definition.clone())
    else {
        return Err("package".into());
    };
    let one = next.terminals.first().ok_or("terminal")?;
    assert_eq!((one.id, one.designation.as_str()), (first, "1"));
    Ok(())
}

/// A gull-wing working set from its custom entry, with its footprint draft
/// discarded and the binding's footprint list cleared (as a pinout import
/// leaves it), and the request that rebuilds its package from the same
/// family.
fn without_footprint(engine: &mut Engine) -> Fallible<(Vec<DefinitionId>, PackageFromTemplate)> {
    let ids = bridge(engine)?;
    apply(engine, LibraryCommand::DiscardDraft { id: ids[3] })?;
    let binding = engine.drafts().get(ids[4]).ok_or("binding")?.clone();
    let Definition::Binding(mut body) = binding.definition else {
        return Err("binding".into());
    };
    body.footprints.clear();
    apply(
        engine,
        LibraryCommand::Edit {
            edits: vec![DraftEdit {
                id: ids[4],
                meta: binding.meta,
                definition: Definition::Binding(body),
            }],
        },
    )?;
    let request = PackageFromTemplate {
        draft: ids[2],
        row: RowSource::Custom {
            family: FamilyKind::GullWing,
        },
        values: None,
        replace: false,
        terminals: TerminalMatch::ByNumber,
        renumber: Vec::new(),
    };
    Ok((ids, request))
}

/// The five drafts of a gull-wing part from its custom entry.
fn bridge(engine: &mut Engine) -> Fallible<Vec<DefinitionId>> {
    new_from(
        engine,
        NewFromTemplate {
            row: RowSource::Custom {
                family: FamilyKind::GullWing,
            },
            values: None,
            skeleton: SkeletonHint::IcBox,
            name: "Bridge".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        },
    )
}

#[test]
fn a_missing_footprint_is_created_and_bound_in_one_step() -> Fallible<()> {
    let mut engine = Engine::default();
    let (ids, request) = without_footprint(&mut engine)?;
    let before = engine.to_file();
    let revision = engine.revision();
    let previewed = plan(&engine, &request)?;
    assert!(previewed.footprint_created);
    assert_eq!(previewed.footprint_bound, Some(ids[4]));
    assert_eq!(previewed.pads_after, 8);
    assert!(
        !codes(&previewed.findings).contains(&"binding.footprint_unbound"),
        "{:?}",
        previewed.findings
    );
    assert_eq!(engine.to_file(), before, "a plan changes nothing");
    let Outcome::PackageReplaced { plan: applied } = apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(request)),
    )?
    else {
        return Err("replaced".into());
    };
    assert_eq!(applied, previewed);
    assert_eq!(engine.revision(), revision + 1, "one undo step");
    let created = applied.footprint.ok_or("the created footprint")?;
    let draft = engine.drafts().get(created).ok_or("footprint draft")?;
    assert_eq!(draft.base, None, "a new lineage");
    let Definition::Footprint(footprint) = &draft.definition else {
        return Err("footprint".into());
    };
    assert_eq!(footprint.package.id, ids[2]);
    assert_eq!(footprint.pads.len(), 8);
    let Definition::Package(package) = &engine.drafts().get(ids[2]).ok_or("package")?.definition
    else {
        return Err("package".into());
    };
    let first = package.terminals.first().map(|t| t.id);
    let polarity = footprint.polarity.as_ref().ok_or("pin 1 is marked")?;
    assert_eq!(
        footprint.pad(polarity.pad).map(|p| p.terminal),
        Some(first),
        "pin 1 lands the first terminal"
    );
    let Definition::Binding(binding) = &engine.drafts().get(ids[4]).ok_or("binding")?.definition
    else {
        return Err("binding".into());
    };
    assert_eq!(
        binding.footprints,
        [FootprintRef::new(created, Revision::FIRST)]
    );
    // The working set publishes as it is: the created footprint is valid.
    let mut set = ids.clone();
    set[3] = created;
    apply(
        &mut engine,
        LibraryCommand::Publish {
            drafts: set,
            close: false,
        },
    )?;
    engine.undo()?;
    engine.undo()?;
    assert_eq!(engine.to_file(), before, "one undo takes it all back");
    Ok(())
}

#[test]
fn a_created_footprint_without_one_binding_reports_what_to_bind() -> Fallible<()> {
    let mut engine = Engine::default();
    let (ids, request) = without_footprint(&mut engine)?;
    // A second binding draft of the package: which one uses the footprint
    // is the author's choice.
    let binding = engine.drafts().get(ids[4]).ok_or("binding")?.clone();
    let Outcome::DraftCreated { id: second } = apply(
        &mut engine,
        LibraryCommand::NewDraft {
            meta: meta("Bridge alternate binding"),
            definition: binding.definition.clone(),
        },
    )?
    else {
        return Err("second binding".into());
    };
    let several = plan(&engine, &request)?;
    assert!(several.footprint_created);
    assert_eq!(several.footprint_bound, None);
    let unbound = several
        .findings
        .iter()
        .find(|f| f.code == "binding.footprint_unbound")
        .ok_or("binding.footprint_unbound")?;
    for id in [ids[4], second] {
        assert!(
            unbound.objects.contains(&ObjectRef::Definition { id }),
            "{unbound:?}"
        );
    }
    let revision = engine.revision();
    apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(request.clone())),
    )?;
    assert_eq!(engine.revision(), revision + 1);
    let created = several.footprint.ok_or("footprint")?;
    assert!(engine.drafts().get(created).is_some());
    for id in [ids[4], second] {
        let Some(Definition::Binding(b)) = engine.drafts().get(id).map(|d| &d.definition) else {
            return Err("binding".into());
        };
        assert!(b.footprints.is_empty(), "nothing is bound by guesswork");
    }
    // With no binding draft at all, the finding says so.
    engine.undo()?;
    for id in [second, ids[4]] {
        apply(&mut engine, LibraryCommand::DiscardDraft { id })?;
    }
    let none = plan(&engine, &request)?;
    assert_eq!(none.footprint_bound, None);
    assert!(codes(&none.findings).contains(&"binding.footprint_unbound"));
    Ok(())
}

/// Set the package and footprint drafts' identity counters of a working
/// set (an imported or hand-edited draft may carry any counter).
fn set_counters(
    engine: &mut Engine,
    ids: &[DefinitionId],
    package: u32,
    footprint: u32,
) -> Fallible<()> {
    let mut edits = Vec::new();
    for id in [ids[2], ids[3]] {
        let mut draft = engine.drafts().get(id).ok_or("draft")?.clone();
        match &mut draft.definition {
            Definition::Package(p) => p.next_local = package,
            Definition::Footprint(f) => f.next_local = footprint,
            _ => return Err("package and footprint".into()),
        }
        edits.push(DraftEdit {
            id,
            meta: draft.meta,
            definition: draft.definition,
        });
    }
    apply(engine, LibraryCommand::Edit { edits })?;
    Ok(())
}

#[test]
fn package_from_template_allocates_valid_ids_from_any_counter() -> Fallible<()> {
    let mut engine = Engine::default();
    let ids = bridge(&mut engine)?;
    let held: Vec<u32> = match &engine.drafts().get(ids[3]).ok_or("footprint")?.definition {
        Definition::Footprint(f) => f
            .pads
            .iter()
            .map(|p| p.id.0)
            .chain(f.graphics.iter().map(|g| g.id.0))
            .collect(),
        _ => return Err("footprint".into()),
    };
    // Counters of 0: a naive allocation would hand out identity 0 and
    // reuse the identities the drafts hold.
    set_counters(&mut engine, &ids, 0, 0)?;
    let qfn = PackageFromTemplate {
        draft: ids[2],
        row: RowSource::Custom {
            family: FamilyKind::NoLead,
        },
        values: None,
        replace: true,
        terminals: TerminalMatch::ByNumber,
        renumber: Vec::new(),
    };
    apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(qfn.clone())),
    )?;
    let Some(Definition::Package(package)) = engine.drafts().get(ids[2]).map(|d| &d.definition)
    else {
        return Err("package".into());
    };
    assert!(package.terminals.len() > 8, "new terminals were allocated");
    let mut seen = std::collections::BTreeSet::new();
    for terminal in &package.terminals {
        assert!(terminal.id.0 > 0 && terminal.id.0 < package.next_local);
        assert!(seen.insert(terminal.id.0), "unique terminals");
    }
    let Some(Definition::Footprint(footprint)) = engine.drafts().get(ids[3]).map(|d| &d.definition)
    else {
        return Err("footprint".into());
    };
    let mut seen = std::collections::BTreeSet::new();
    let locals = footprint
        .pads
        .iter()
        .map(|p| p.id.0)
        .chain(footprint.graphics.iter().map(|g| g.id.0));
    for local in locals {
        assert!(local > 0 && local < footprint.next_local, "{local}");
        assert!(!held.contains(&local), "{local} retired with the old pads");
        assert!(seen.insert(local), "unique locals");
    }
    // Rebuilt in its own family from counters of 0, the set publishes:
    // publication checks every local identity.
    engine.undo()?;
    apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(PackageFromTemplate {
            row: RowSource::Custom {
                family: FamilyKind::GullWing,
            },
            ..qfn.clone()
        })),
    )?;
    apply(
        &mut engine,
        LibraryCommand::Publish {
            drafts: ids.clone(),
            close: false,
        },
    )?;

    // An exhausted counter is refused and changes nothing.
    let mut engine = Engine::default();
    let ids = bridge(&mut engine)?;
    set_counters(&mut engine, &ids, 9, u32::MAX)?;
    let before = engine.to_file();
    let revision = engine.revision();
    let refused = apply(
        &mut engine,
        LibraryCommand::PackageFromTemplate(Box::new(qfn)),
    );
    assert!(matches!(refused, Err(Error::Limit(_))), "{refused:?}");
    assert_eq!((engine.revision(), engine.to_file()), (revision, before));
    Ok(())
}

/// The estimated lands of every no-lead and chip starter row keep the
/// minimum solder-mask web (`footprint.mask_web`) with the default mask
/// rules, corner pads of the four-sided QFNs included. (Gull-wing rows at
/// 0.65 mm pitch and below still raise it; see editor evidence.)
#[test]
fn no_lead_and_chip_starter_rows_keep_the_mask_web() -> Fallible<()> {
    let mut checked = 0;
    for starter in crate::library::templates::starter::rows().map_err(|e| e.to_owned())? {
        if !matches!(
            starter.definition.values,
            PackageForm::NoLead(_) | PackageForm::Chip(_)
        ) {
            continue;
        }
        let mut engine = Engine::default();
        let ids = new_from(
            &mut engine,
            NewFromTemplate {
                row: RowSource::Starter {
                    name: starter.name.clone(),
                },
                values: None,
                skeleton: starter.definition.skeleton,
                name: starter.name.clone(),
                designator_prefix: None,
                symbol: None,
                without_fields: false,
            },
        )?;
        let Some(Definition::Footprint(footprint)) =
            engine.drafts().get(ids[3]).map(|d| &d.definition)
        else {
            return Err(format!("{}: footprint", starter.name).into());
        };
        let webs: Vec<String> = footprint
            .lint(ids[3])
            .into_iter()
            .filter(|f| f.code == "footprint.mask_web")
            .map(|f| f.message)
            .collect();
        assert!(webs.is_empty(), "{}: {webs:?}", starter.name);
        checked += 1;
    }
    assert!(checked > 40, "{checked}");
    Ok(())
}

/// QFN-48 7 × 7 mm at 0.5 mm pitch: the corner lands (pins 1 and 48, and
/// the other three corners) give up the same heel length, the least that
/// leaves 0.2 mm of copper (a 0.1 mm web inside 0.05 mm mask openings);
/// toes stay, every land still covers its terminal, and the lands between
/// the corners are unchanged.
#[test]
fn qfn48_corner_lands_keep_the_mask_web() -> Fallible<()> {
    use crate::library::samples;
    use crate::library::{MASK_EXPANSION_MM, MIN_MASK_WEB_MM};
    let starter = crate::library::templates::starter::row("QFN-48 7×7 0.5 EP 5.3")?;
    let PackageForm::NoLead(form) = &starter.definition.values else {
        return Err("a no-lead row".into());
    };
    let terminals: Vec<_> = (1..=49).map(crate::ids::PackageTerminalId).collect();
    let pads =
        samples::template_lands(&starter.definition.values, &terminals, None).ok_or("lands")?;
    let pad = |number: &str| {
        pads.iter()
            .find(|p| p.number == number)
            .ok_or(format!("pad {number}"))
    };
    let gap = |a: &str, b: &str| -> Fallible<f64> {
        let (a, b) = (pad(a)?.bounds(), pad(b)?.bounds());
        let dx = (a.min[0] - b.max[0]).max(b.min[0] - a.max[0]).max(0.0);
        let dy = (a.min[1] - b.max[1]).max(b.min[1] - a.max[1]).max(0.0);
        Ok(dx.hypot(dy))
    };
    let need = MIN_MASK_WEB_MM + 2.0 * MASK_EXPANSION_MM;
    for (a, b) in [("1", "48"), ("12", "13"), ("24", "25"), ("36", "37")] {
        let g = gap(a, b)?;
        assert!(g >= need - 1e-9 && g < need + 1e-3, "{a}–{b}: {g}");
    }
    let length = |n: &str| -> Fallible<f64> {
        let size = pad(n)?.bounds().size();
        Ok(size[0].max(size[1]))
    };
    let (corner, middle) = (length("1")?, length("6")?);
    assert!(corner < middle, "the corner land is shortened");
    for n in ["12", "13", "24", "25", "36", "37", "48"] {
        assert!((length(n)? - corner).abs() < 1e-9, "pad {n} matches pad 1");
    }
    // Pins 1–12 run along the bottom side: toes stay at the middle lands'
    // toe, and the land still spans the terminal (body edge inwards by the
    // terminal length).
    let (one, six) = (pad("1")?.bounds(), pad("6")?.bounds());
    assert!(
        (one.min[1] - six.min[1]).abs() < 1e-9,
        "toe kept: {one:?} {six:?}"
    );
    let inner = -form.body_width / 2.0 + form.terminal_length;
    assert!(one.max[1] >= inner, "the land covers its terminal: {one:?}");
    // A web of exactly 0.1 mm passes the lint; 1 µm less is reported.
    let mut footprint = crate::library::FootprintDefinition {
        pads,
        next_local: 50,
        ..Default::default()
    };
    let webs = |f: &crate::library::FootprintDefinition| {
        f.lint(DefinitionId(1))
            .iter()
            .filter(|f| f.code == "footprint.mask_web")
            .count()
    };
    assert_eq!(webs(&footprint), 0);
    let six = footprint
        .pads
        .iter_mut()
        .find(|p| p.number == "6")
        .ok_or("pad 6")?;
    six.position[0] += 0.001;
    assert_eq!(webs(&footprint), 1, "pads 6 and 7 at 0.099 mm");
    Ok(())
}

/// The symbol and component drafts `new_from` built from a starter row.
fn starter_build(
    engine: &mut Engine,
    name: &str,
    prefix: Option<&str>,
    symbol: Option<SymbolKind>,
) -> Fallible<(crate::library::SymbolDefinition, String)> {
    let row = crate::library::templates::starter::row(name)?;
    let ids = new_from(
        engine,
        NewFromTemplate {
            row: RowSource::Starter { name: name.into() },
            values: None,
            skeleton: row.definition.skeleton,
            name: name.into(),
            designator_prefix: prefix.map(str::to_owned),
            symbol,
            without_fields: false,
        },
    )?;
    let draft = |k: usize| {
        ids.get(k)
            .and_then(|id| engine.drafts().get(*id))
            .map(|d| d.definition.clone())
    };
    match (draft(0), draft(1)) {
        (Some(Definition::Component(c)), Some(Definition::Symbol(s))) => {
            Ok((s, c.designator_prefix))
        }
        _ => Err(format!("{name}: component and symbol drafts").into()),
    }
}

/// The generated drawing of `kind` for `symbol`'s owner and pins.
fn expected(
    symbol: &crate::library::SymbolDefinition,
    kind: SymbolKind,
    conventions: SymbolConventions,
) -> Fallible<crate::library::SymbolDefinition> {
    let mut terminals: Vec<_> = symbol
        .units
        .iter()
        .flat_map(|u| u.pins.iter().map(|p| p.terminal))
        .collect();
    terminals.sort();
    Ok(symbol_style::generate(
        kind,
        conventions,
        symbol.component,
        &terminals,
    )?)
}

/// Every two-terminal starter row draws its kind's symbol and takes its
/// prefix: chip capacitors are capacitors (never resistor rectangles),
/// tantalum and radial rows polarised capacitors, ferrite chips beads,
/// molded and axial diode rows diodes.
#[test]
fn two_terminal_starter_rows_draw_their_kinds() -> Fallible<()> {
    let mut seen = std::collections::BTreeSet::new();
    for starter in crate::library::templates::starter::rows().map_err(|e| e.to_owned())? {
        if !matches!(
            starter.definition.skeleton,
            SkeletonHint::TwoTerminal { .. }
        ) {
            continue;
        }
        let name = starter.name.as_str();
        let (kind, prefix) =
            if name.starts_with("Chip resistor") || name.starts_with("Axial resistor") {
                (SymbolKind::Resistor, "R")
            } else if name.starts_with("Chip capacitor") {
                (SymbolKind::Capacitor, "C")
            } else if name.starts_with("Chip ferrite bead") {
                (SymbolKind::FerriteBead, "FB")
            } else if name.starts_with("Chip LED") {
                (SymbolKind::Led, "D")
            } else if name.starts_with("Tantalum") || name.starts_with("Radial") {
                (SymbolKind::PolarisedCapacitor, "C")
            } else {
                (SymbolKind::Diode, "D")
            };
        let mut engine = Engine::default();
        let (symbol, designator) = starter_build(&mut engine, name, None, None)?;
        assert_eq!(designator, prefix, "{name}");
        assert_eq!(
            symbol,
            expected(&symbol, kind, SymbolConventions::default())?,
            "{name}"
        );
        seen.insert(format!("{kind:?}"));
    }
    assert_eq!(seen.len(), 6, "{seen:?}");
    Ok(())
}

/// The user's prefix names the kind (a `D` on a tantalum row draws a
/// diode); an explicit symbol wins (an NPN on SOT-23); the project's
/// conventions choose the resistor style; a symbol whose terminal count
/// differs from the package's is refused with the document unchanged.
#[test]
fn prefix_explicit_symbol_and_conventions_choose_the_drawing() -> Fallible<()> {
    let mut engine = Engine::default();
    let (diode, _) = starter_build(&mut engine, "Tantalum case A (3216-18)", Some("D"), None)?;
    assert_eq!(
        diode,
        expected(&diode, SymbolKind::Diode, SymbolConventions::default())?
    );
    let (npn, prefix) = starter_build(&mut engine, "SOT-23-3", None, Some(SymbolKind::NpnBjt))?;
    assert_eq!(
        npn,
        expected(&npn, SymbolKind::NpnBjt, SymbolConventions::default())?
    );
    assert_eq!(
        prefix, "U",
        "an explicit symbol leaves the skeleton's prefix"
    );
    let us = SymbolConventions {
        resistor: ResistorStyle::Us,
    };
    apply(
        &mut engine,
        LibraryCommand::SetSymbolConventions { conventions: us },
    )?;
    assert_eq!(engine.drafts().symbols, us);
    let (zigzag, _) = starter_build(&mut engine, "Chip resistor 0603 (1608 metric)", None, None)?;
    assert_eq!(zigzag, expected(&zigzag, SymbolKind::Resistor, us)?);
    // The setting is saved with the file.
    assert_eq!(Engine::from_file(engine.to_file())?.drafts().symbols, us);

    let before = engine.to_file();
    let refused = starter_build(
        &mut engine,
        "SOIC-8 3.9 mm",
        None,
        Some(SymbolKind::Capacitor),
    );
    let message = refused.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(message.contains("needs 2 terminals"), "{message}");
    assert_eq!(engine.to_file(), before);
    Ok(())
}

/// The rows `query` finds on a fresh install (the built-in starter rows).
fn starter_hits(engine: &Engine, query: &str) -> Fallible<Vec<String>> {
    let report = templates_with(engine, query, &CatalogContext::default())?;
    Ok(report
        .hits
        .iter()
        .filter(|h| h.is_starter())
        .map(|h| h.name.clone())
        .collect())
}

/// Blocking and other findings the drafts report, as `code: message`.
fn draft_findings(engine: &Engine) -> Fallible<Vec<String>> {
    let QueryResult::Drafts(checks) = engine.query(Query::CheckDrafts {})? else {
        return Err("not the draft checks".into());
    };
    Ok(checks
        .iter()
        .flat_map(|c| &c.findings)
        .map(|f| format!("{}: {}", f.code, f.message))
        .collect())
}

const USB_C: &str = "USB-C receptacle, USB 2.0, 16 contacts, top mount (GCT USB4105)";

/// The designer's path: search finds the receptacle, Create builds it with
/// USB-named pins, the A and B rows' duplicates carried by one terminal,
/// the drawing's lands and its provenance, and it publishes.
#[test]
fn a_usb_c_receptacle_is_found_built_mapped_and_published() -> Fallible<()> {
    let mut engine = Engine::default();
    for query in ["usb", "USB-C", "usb-c", "type-c", "Type-C", "connector"] {
        assert!(
            starter_hits(&engine, query)?.iter().any(|n| n == USB_C),
            "{query} finds the receptacle"
        );
    }
    assert_eq!(
        starter_hits(&engine, "usb")?.first().map(String::as_str),
        Some(USB_C)
    );
    let report = templates_with(&engine, "usb", &CatalogContext::default())?;
    let hit = report
        .hits
        .iter()
        .find(|h| h.name == USB_C)
        .ok_or("the row")?;
    assert!(hit.cited && hit.citation_checked && hit.lands_cited);
    assert_eq!(hit.skeleton, SkeletonHint::Connector);

    let row = crate::library::templates::starter::row(USB_C)?;
    let ids = new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Starter { name: USB_C.into() },
            values: None,
            skeleton: row.definition.skeleton,
            name: "USB-C".into(),
            designator_prefix: None,
            symbol: None,
            without_fields: false,
        },
    )?;
    let draft = |k: usize| {
        ids.get(k)
            .and_then(|id| engine.drafts().get(*id))
            .ok_or_else(|| format!("draft {k}"))
    };
    let Definition::Component(component) = &draft(0)?.definition else {
        return Err("component".into());
    };
    assert_eq!(component.designator_prefix, "J");
    let pins: Vec<(&str, Vec<&str>)> = component
        .terminals
        .iter()
        .map(|t| {
            let labels = match &t.class {
                crate::library::TerminalClass::Electrical(f) => {
                    f.functions.iter().map(|f| f.label.as_str()).collect()
                }
                _ => Vec::new(),
            };
            (t.name.as_str(), labels)
        })
        .collect();
    assert_eq!(
        pins,
        [
            ("VBUS", vec!["power output"]),
            ("CC1", vec!["passive"]),
            ("CC2", vec!["passive"]),
            ("D+", vec!["passive"]),
            ("D-", vec!["passive"]),
            ("SBU1", vec!["passive"]),
            ("SBU2", vec!["passive"]),
            ("GND", vec!["passive"]),
            ("SHIELD", vec!["passive"]),
        ]
    );
    let terminal = |name: &str| {
        component
            .terminals
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.id)
    };
    let Definition::Package(package) = &draft(2)?.definition else {
        return Err("package".into());
    };
    let Definition::Binding(binding) = &draft(4)?.definition else {
        return Err("binding".into());
    };
    // Hand-read from the Type-C pinout, pad by pad.
    let carried = [
        ("A1B12", "GND"),
        ("A4B9", "VBUS"),
        ("B8", "SBU2"),
        ("A5", "CC1"),
        ("B7", "D-"),
        ("A6", "D+"),
        ("A7", "D-"),
        ("B6", "D+"),
        ("A8", "SBU1"),
        ("B5", "CC2"),
        ("B4A9", "VBUS"),
        ("B1A12", "GND"),
        ("S", "SHIELD"),
    ];
    assert_eq!(package.terminals.len(), carried.len());
    assert_eq!(binding.mappings.len(), carried.len());
    for (designation, name) in carried {
        let id = package
            .terminals
            .iter()
            .find(|t| t.designation == designation)
            .map(|t| t.id)
            .ok_or(designation)?;
        assert_eq!(binding.terminal_of(id), terminal(name), "{designation}");
    }

    let Definition::Footprint(footprint) = &draft(3)?.definition else {
        return Err("footprint".into());
    };
    use crate::library::PadKind;
    let kind = |k: PadKind| footprint.pads.iter().filter(|p| p.kind == k).count();
    assert_eq!(kind(PadKind::Smd), 12);
    assert_eq!(kind(PadKind::PlatedThrough), 4);
    assert_eq!(kind(PadKind::NonPlatedHole), 2);
    let polarity = footprint.polarity.as_ref().ok_or("the marked land")?;
    assert_eq!(
        footprint.pad(polarity.pad).map(|p| p.number.as_str()),
        Some("A1B12")
    );
    let pad = |number: &str| {
        footprint
            .pads
            .iter()
            .find(|p| p.number == number)
            .ok_or_else(|| format!("pad {number}"))
    };
    // The drawing: VBUS at ±2.40, GND at ±3.20, 0.60 and 0.30 wide lands
    // 1.15 long, 3.68 from the outline centre (7.355 from the board edge).
    assert_eq!(pad("A4B9")?.position, [-2.4, 3.68]);
    assert_eq!(pad("B1A12")?.position, [3.2, 3.68]);
    assert_eq!(pad("A6")?.position, [-0.25, 3.68]);
    assert_eq!(pad("A6")?.shape, PadShape::Rect { size: [0.3, 1.15] });
    assert_eq!(pad("A1B12")?.shape, PadShape::Rect { size: [0.6, 1.15] });
    assert_eq!(
        pad("S")?.hole.as_ref().map(|h| h.shape),
        Some(crate::library::HoleShape::Slot {
            width: 0.6,
            length: 1.4,
            angle_deg: 90.0
        })
    );
    assert!(
        footprint
            .pads
            .iter()
            .filter(|p| p.kind == PadKind::NonPlatedHole)
            .all(|p| p.terminal.is_none() && p.hole.as_ref().is_some_and(|h| !h.plated))
    );

    let provenance = &draft(0)?.meta.provenance;
    for text in [
        "GCT USB4105",
        "rev. B4",
        "p. 1",
        "lands from the cited drawing",
    ] {
        assert!(provenance.contains(text), "{provenance}");
    }
    let Definition::Symbol(symbol) = &draft(1)?.definition else {
        return Err("symbol".into());
    };
    assert_eq!(symbol.units.iter().map(|u| u.pins.len()).sum::<usize>(), 9);

    let findings = draft_findings(&engine)?;
    assert_eq!(findings, Vec::<String>::new());
    apply(
        &mut engine,
        LibraryCommand::Publish {
            drafts: ids,
            close: false,
        },
    )?;
    Ok(())
}

/// Run 3, finding 2: a passive template brings its typed value fields, all
/// settable per instance, imported in the template's own undo step.
#[test]
fn chip_passives_carry_their_value_fields() -> Fallible<()> {
    for (name, expected, typed) in [
        (
            "Chip resistor 0603 (1608 metric)",
            ["Resistance", "Rated voltage", "Tolerance"],
            "5.1 kΩ",
        ),
        (
            "Chip capacitor 0603 (1608 metric)",
            ["Capacitance", "Rated voltage", "Tolerance"],
            "100 nF",
        ),
    ] {
        let mut engine = Engine::default();
        let before = engine.to_file();
        let ids = {
            let row = crate::library::templates::starter::row(name)?;
            new_from(
                &mut engine,
                NewFromTemplate {
                    row: RowSource::Starter { name: name.into() },
                    values: None,
                    skeleton: row.definition.skeleton,
                    name: name.into(),
                    designator_prefix: None,
                    symbol: None,
                    without_fields: false,
                },
            )?
        };
        let draft = ids
            .first()
            .and_then(|id| engine.drafts().get(*id))
            .ok_or("component draft")?;
        let Definition::Component(component) = &draft.definition else {
            return Err("component".into());
        };
        let library = &engine.project().library;
        let selected: Vec<&str> = component
            .fields
            .iter()
            .filter_map(|s| library.field(s.field))
            .map(|f| f.meta.name.as_str())
            .collect();
        assert_eq!(selected, expected, "{name}");
        assert!(component.fields.iter().all(|s| s.show_on_symbol), "{name}");
        let value = component.fields.first().ok_or("value field")?;
        let view = library.field_view(value.field).ok_or("field view")?;
        let parsed =
            crate::fields::parse_value(typed, &view.field.definition.value_type, view.kind_body())?;
        assert!(matches!(parsed, crate::fields::FieldValue::Quantity { .. }));
        assert_eq!(draft_findings(&engine)?, Vec::<String>::new(), "{name}");
        engine.undo()?;
        assert_eq!(engine.to_file(), before, "{name}: one undo step");
    }
    Ok(())
}

/// The designer's path for an LED: search finds it, Create draws the LED
/// (not a plain diode) with its cathode and anode named and pad 1 marked,
/// the drawing's lands, and the typed colour and forward voltage fields.
#[test]
fn a_chip_led_is_found_drawn_marked_and_carries_its_fields() -> Fallible<()> {
    for (name, land, gap) in [
        ("Chip LED 0603 (1608 metric)", [0.8, 0.8], 0.825),
        ("Chip LED 0805 (2012 metric)", [1.25, 1.1], 1.175),
    ] {
        let mut engine = Engine::default();
        for query in ["led", "LED", "light", "diode", "chip led"] {
            assert!(
                starter_hits(&engine, query)?.iter().any(|n| n == name),
                "{query} finds {name}"
            );
        }
        assert_eq!(
            starter_hits(&engine, "led")?
                .first()
                .map(|n| n.starts_with("Chip LED")),
            Some(true)
        );

        let before = engine.to_file();
        let (symbol, prefix) = starter_build(&mut engine, name, None, None)?;
        assert_eq!(prefix, "D");
        assert_eq!(
            symbol,
            expected(&symbol, SymbolKind::Led, SymbolConventions::default())?,
            "the LED's own drawing"
        );
        let plain = expected(&symbol, SymbolKind::Diode, SymbolConventions::default())?;
        assert_ne!(symbol, plain, "told apart from a plain diode");

        let drafts: Vec<_> = engine
            .drafts()
            .items
            .iter()
            .map(|d| (d.id, d.definition.clone()))
            .collect();
        let find = |pick: fn(&Definition) -> bool| {
            drafts
                .iter()
                .find(|(_, d)| pick(d))
                .map(|(_, d)| d)
                .ok_or("draft")
        };
        let Definition::Component(component) = find(|d| matches!(d, Definition::Component(_)))?
        else {
            return Err("component".into());
        };
        let names: Vec<&str> = component
            .terminals
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, ["K", "A"], "terminal 1 is the cathode, as drawn");

        // Colour and forward voltage, typed, from the project's copies.
        let library = &engine.project().library;
        let selected: Vec<&str> = component
            .fields
            .iter()
            .filter_map(|s| library.field(s.field))
            .map(|f| f.meta.name.as_str())
            .collect();
        assert_eq!(selected, ["Colour", "Forward voltage"]);
        assert!(component.fields.iter().all(|s| s.show_on_symbol));
        let voltage = component.fields.get(1).ok_or("forward voltage")?;
        let view = library.field_view(voltage.field).ok_or("field view")?;
        let typed = crate::fields::parse_value(
            "2.2 V",
            &view.field.definition.value_type,
            view.kind_body(),
        )?;
        assert!(matches!(typed, crate::fields::FieldValue::Quantity { .. }));

        let Definition::Footprint(footprint) = find(|d| matches!(d, Definition::Footprint(_)))?
        else {
            return Err("footprint".into());
        };
        let polarity = footprint.polarity.as_ref().ok_or("the marked pad")?;
        let first = footprint.pads.first().ok_or("pad 1")?;
        assert_eq!(polarity.pad, first.id);
        assert_eq!(first.position, [-gap, 0.0], "the cathode at −x");
        assert_eq!(first.shape, PadShape::Rect { size: land });
        let legend = polarity
            .markers
            .iter()
            .filter_map(|id| footprint.graphics.iter().find(|g| g.id == *id))
            .all(|g| g.layer == crate::library::graphics::DrawingLayer::Silkscreen);
        assert!(
            legend && !polarity.markers.is_empty(),
            "marked on the legend"
        );

        let Definition::Package(package) = find(|d| matches!(d, Definition::Package(_)))? else {
            return Err("package".into());
        };
        assert!(matches!(&package.form, PackageForm::Chip(c) if c.kind == ChipKind::Led));

        let ids: Vec<DefinitionId> = drafts.iter().map(|(id, _)| *id).collect();
        let findings = draft_findings(&engine)?;
        assert_eq!(findings, Vec::<String>::new(), "{name}");
        apply(
            &mut engine,
            LibraryCommand::Publish {
                drafts: ids,
                close: false,
            },
        )?;
        engine.undo()?;
        engine.undo()?;
        assert_eq!(
            engine.to_file(),
            before,
            "one undo step, the fields included"
        );

        // A second LED reuses the imported fields.
        starter_build(&mut engine, name, None, None)?;
        let fields = engine.project().library.fields.len();
        starter_build(&mut engine, name, None, None)?;
        assert_eq!(engine.project().library.fields.len(), fields);
    }
    Ok(())
}

/// A request naming the LED symbol on another row keeps that row's fields:
/// the fields follow the construction, not a drawing choice.
#[test]
fn an_led_symbol_on_another_row_keeps_that_rows_fields() -> Fallible<()> {
    let mut engine = Engine::default();
    let name = "Chip capacitor 0603 (1608 metric)";
    let row = crate::library::templates::starter::row(name)?;
    let ids = new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Starter { name: name.into() },
            values: None,
            skeleton: row.definition.skeleton,
            name: name.into(),
            designator_prefix: Some("D".into()),
            symbol: Some(SymbolKind::Led),
            without_fields: false,
        },
    )?;
    let Some(Definition::Component(component)) = ids
        .first()
        .and_then(|id| engine.drafts().get(*id))
        .map(|d| &d.definition)
    else {
        return Err("component".into());
    };
    let library = &engine.project().library;
    let first = component
        .fields
        .first()
        .and_then(|s| library.field(s.field));
    assert_eq!(first.map(|f| f.meta.name.as_str()), Some("Capacitance"));
    Ok(())
}

/// A client that types its own fields asks for none: nothing is imported.
#[test]
fn without_fields_imports_nothing() -> Fallible<()> {
    let mut engine = Engine::default();
    let name = "Chip resistor 0603 (1608 metric)";
    let row = crate::library::templates::starter::row(name)?;
    new_from(
        &mut engine,
        NewFromTemplate {
            row: RowSource::Starter { name: name.into() },
            values: None,
            skeleton: row.definition.skeleton,
            name: name.into(),
            designator_prefix: None,
            symbol: None,
            without_fields: true,
        },
    )?;
    assert!(engine.project().library.fields.is_empty());
    Ok(())
}

/// `library.publish_from_template` publishes all five definitions in one
/// undo step and leaves no draft; every built-in row builds a component that
/// way, so the Place palette can create any of them without a form.
#[test]
fn publish_from_template_is_one_atomic_step() -> crate::diagnostics::Result<()> {
    let rows = starter::rows().map_err(|why| Error::Invalid(why.to_owned()))?;
    let mut engine = Engine::default();
    for row in rows {
        let before = engine.project().library.components.len();
        let request = NewFromTemplate {
            row: RowSource::Starter {
                name: row.name.clone(),
            },
            values: None,
            skeleton: row.definition.skeleton,
            name: row.name.clone(),
            designator_prefix: None,
            symbol: None,
        };
        let outcome = apply(
            &mut engine,
            LibraryCommand::PublishFromTemplate(Box::new(request)),
        )?;
        let Outcome::Published { revisions } = outcome else {
            return Err(Error::Invalid(format!("{}: {outcome:?}", row.name)));
        };
        assert_eq!(revisions.len(), 5, "{}", row.name);
        assert_eq!(engine.project().library.components.len(), before + 1);
        assert!(engine.drafts().items.is_empty(), "{}", row.name);
        engine.undo()?;
        assert_eq!(engine.project().library.components.len(), before);
        assert!(engine.drafts().items.is_empty());
    }

    // A refused request changes nothing and takes no undo step.
    let revision = engine.revision();
    let refused = apply(
        &mut engine,
        LibraryCommand::PublishFromTemplate(Box::new(NewFromTemplate {
            row: RowSource::Starter {
                name: "No such row".into(),
            },
            values: None,
            skeleton: SkeletonHint::Blank,
            name: "Nothing".into(),
            designator_prefix: None,
            symbol: None,
        })),
    );
    assert!(refused.is_err());
    assert_eq!(engine.revision(), revision);
    assert!(engine.project().library.components.is_empty());
    Ok(())
}
