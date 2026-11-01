//! Slice B1 behaviour: rows in the catalog, `new_from_template`,
//! `package_from_template` and its plan, `save_package_row` and the row
//! publish findings, all through the public engine.
use super::*;
use crate::diagnostics::Finding;
use crate::ids::{DefinitionKind, LibraryId};
use crate::library::bundle::{Bundle, RevisionRecord};
use crate::library::package_form::{ChipForm, ChipKind, LeadSides, NoLeadForm};
use crate::library::samples::Preset;
use crate::library::templates::catalog::{
    BUILT_FAMILY_REV, FamilyRef, LandValues, NewFromTemplate, PackageFromTemplate, PackagePlan,
    RowFrom, RowSource, SavePackageRow,
};
use crate::library::{Definition, DefinitionMeta, PadShape};
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
    apply(engine, LibraryCommand::Publish { drafts: vec![id] })?;
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

#[test]
fn template_queries_refuse_component_filters_and_bad_values() -> Fallible<()> {
    let engine = rows_engine()?;
    for query in [
        "kind:part",
        "has:mpn",
        "mpn:abc",
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
    apply(&mut engine, LibraryCommand::Publish { drafts: ids })?;
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
        let Some(values) = family.custom_values() else {
            assert!(family.family_rev().is_none(), "{family:?}");
            continue;
        };
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
            },
        )?;
        let published = apply(
            &mut engine,
            LibraryCommand::Publish {
                drafts: ids.clone(),
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
    // SOIC-8 to QFN-16 + EP: nine new terminals are unmapped, and the
    // eight kept mappings are flagged as reused by position.
    assert_eq!(
        codes(&previewed.findings),
        ["binding.mapping_missing", "binding.mapping_retained"]
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
    apply(&mut engine, LibraryCommand::Publish { drafts: vec![id] })?;
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
    assert!(apply(&mut engine, LibraryCommand::Publish { drafts: vec![id] }).is_err());
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
        .filter_map(|family| {
            let values = family.custom_values()?;
            Some((
                format!("custom {family:?}"),
                RowSource::Custom { family },
                row(values, &[]),
            ))
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
        assert_eq!(pads.len(), package.terminals.len(), "{name}");
        let mut landed = std::collections::BTreeSet::new();
        for pad in pads {
            let terminal = pad.terminal.and_then(|t| package.terminal(t));
            let terminal =
                terminal.ok_or(format!("{name}: pad {} lands no terminal", pad.number))?;
            assert!(landed.insert(terminal.id), "{name}: {} twice", pad.number);
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
        let published = apply(&mut engine, LibraryCommand::Publish { drafts: ids });
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
        },
    )?;
    let request = PackageFromTemplate {
        draft: ids[2],
        row: RowSource::Custom {
            family: FamilyKind::NoLead,
        },
        values: Some(qfn32()),
        replace: true,
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
