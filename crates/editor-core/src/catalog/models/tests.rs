//! Slice D1: `library.models` filters, facets, generated entries, sources
//! and "fits this package" ranking. Fixtures are original.
use super::*;
use crate::catalog::SourceCatalog;
use crate::ids::{LibraryId, Revision};
use crate::library::PackageDefinition;
use crate::library::bundle::PackedAsset;
use crate::library::model3d::tests::{bytes, import_request, model, packed};
use crate::library::model3d::{AssetRef, LeadTag, ModelDefinition, ModelFormat};
use crate::library::package_fit;
use crate::tests::support::*;
use crate::{Engine, Outcome, Query, QueryResult};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn named(
    engine: &mut Engine,
    name: &str,
    definition: ModelDefinition,
    asset: &PackedAsset,
) -> Fallible<ModelRef> {
    let Outcome::DraftCreated { id } = apply(engine, import_request(name, asset, definition))?
    else {
        return Err("a draft".into());
    };
    let r = *publish(engine, vec![id])?.first().ok_or("published")?;
    Ok(ModelRef::new(r.id, r.revision))
}

fn asset_of(asset: &PackedAsset, media_type: &str, file: &str) -> AssetRef {
    AssetRef {
        hash: asset.hash,
        bytes: asset.data.len() as u64,
        media_type: media_type.into(),
        file_name: file.into(),
    }
}

/// Three models: a two-lead STEP part with leads and a marking area; an
/// 8-pin THT glTF connector with textures that may not be redistributed;
/// an 8-pin SOIC-like STEP body under MIT whose redistribution is unknown.
fn three(engine: &mut Engine) -> Fallible<()> {
    let a = packed(&bytes("catalog-a"), "model/step");
    named(engine, "Two-lead body", model(&a), &a)?;

    let b = packed(&bytes("catalog-b"), "model/gltf-binary");
    let mut header = model(&b);
    header.asset = asset_of(&b, "model/gltf-binary", "header.glb");
    header.format = ModelFormat::Gltf;
    header.bounds_nm = crate::tests::support::nm_bounds([[-5.0, -4.0, 0.0], [5.0, 4.0, 3.0]]);
    header.leads.clear();
    header.marking_area = None;
    header
        .custom_materials
        .push(crate::library::model3d::CustomMaterial {
            name: "Housing".into(),
            base_colour: [0.1, 0.1, 0.1, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            clearcoat: None,
            transmission: None,
            ior: None,
            emissive: None,
            textures_ignored: true,
        });
    header.hints = ModelHints {
        mount: Some(Mount::Tht),
        pins: Some(8),
        pitch_nm: Some(crate::tests::support::nm(2.54)),
        family: Some("pin header".into()),
    };
    header.provenance.redistribution = Redistribution::Forbidden;
    named(engine, "Header 1x8", header, &b)?;

    let c = packed(&bytes("catalog-c"), "model/step");
    let mut soic = model(&c);
    soic.bounds_nm = crate::tests::support::nm_bounds([[-2.45, -1.95, 0.0], [2.45, 1.95, 1.75]]);
    soic.leads.clear();
    soic.marking_area = None;
    soic.hints = ModelHints {
        mount: Some(Mount::Smd),
        pins: Some(8),
        pitch_nm: Some(crate::tests::support::nm(1.27)),
        family: Some("gull-wing".into()),
    };
    soic.provenance.record.licence = Some("MIT".into());
    soic.provenance.redistribution = Redistribution::Unknown;
    named(engine, "SOIC-8 body", soic, &c)?;
    Ok(())
}

fn search(engine: &Engine, query: &str) -> Result<ModelReport> {
    super::search(
        engine.project(),
        engine.drafts(),
        &CatalogContext::default(),
        query,
        None,
        MAX_MODEL_HITS,
    )
}

fn names(report: &ModelReport) -> Vec<String> {
    report.hits.iter().map(|h| h.name.clone()).collect()
}

fn library_names(engine: &Engine, query: &str) -> Fallible<Vec<String>> {
    Ok(search(engine, query)?
        .hits
        .iter()
        .filter(|h| matches!(h.kind, ModelHitKind::Library { .. }))
        .map(|h| h.name.clone())
        .collect())
}

#[test]
fn filters_select_by_every_facet() -> Fallible<()> {
    let mut engine = Engine::default();
    three(&mut engine)?;
    let cases: [(&str, &[&str]); 17] = [
        ("kind:model pins:8", &["Header 1x8", "SOIC-8 body"]),
        ("pins:8 pitch:1.27", &["SOIC-8 body"]),
        ("pins:>=2 pins:<8", &["Two-lead body"]),
        ("pitch:50mil", &["SOIC-8 body"]),
        ("body:3.9x4.9", &["SOIC-8 body"]),
        ("body:4.9x3.9x1.75", &["SOIC-8 body"]),
        ("body:4.9x3.9x9", &[]),
        ("format:glb", &["Header 1x8"]),
        ("format:step mount:smd", &["SOIC-8 body", "Two-lead body"]),
        ("licence:forbidden", &["Header 1x8"]),
        ("license:mit", &["SOIC-8 body"]),
        ("has:leads", &["Two-lead body"]),
        ("has:marking", &["Two-lead body"]),
        ("has:textures", &["Header 1x8"]),
        ("src:library", &[]),
        ("family:gull", &["SOIC-8 body"]),
        ("header", &["Header 1x8"]),
    ];
    for (query, expected) in cases {
        assert_eq!(library_names(&engine, query)?, expected, "{query}");
    }
    // Generated entries take text, mount and has:/missing: but no data
    // filter.
    assert_eq!(
        names(&search(&engine, "mount:tht")?),
        [
            "Header 1x8",
            "Generated · pin header",
            "Generated · through-hole"
        ]
    );
    assert!(
        search(&engine, "pins:8")?
            .hits
            .iter()
            .all(|h| matches!(h.kind, ModelHitKind::Library { .. }))
    );
    let generated = search(&engine, "kind:generated")?;
    assert_eq!(
        names(&generated),
        [
            "Generated · BGA / LGA",
            "Generated · chip",
            "Generated · gull-wing",
            "Generated · module",
            "Generated · pin header",
            "Generated · QFN / DFN",
            "Generated · SOD / SMA / MELF",
            "Generated · SOT / DPAK",
            "Generated · through-hole",
            "Generated · USB-C receptacle"
        ]
    );
    assert_eq!(search(&engine, "missing:leads")?.hits.len(), 2 + 10);
    Ok(())
}

#[test]
fn bad_queries_are_refused_with_the_term() -> Fallible<()> {
    let engine = Engine::default();
    for query in [
        "kind:part",
        "colour:red",
        "pins:eight",
        "body:4",
        "pitch:-1",
        "format:wrl",
    ] {
        let Err(Error::Invalid(message)) = search(&engine, query) else {
            return Err(format!("{query} is refused").into());
        };
        assert!(message.starts_with("models.query: "), "{message}");
        assert!(message.contains(query), "{message}");
    }
    assert!(search(&engine, &"a ".repeat(MAX_QUERY)).is_err());
    assert!(search(&engine, &"a ".repeat(MAX_TERMS + 1)).is_err());
    Ok(())
}

#[test]
fn facets_count_every_match_and_the_limit_reports_more() -> Fallible<()> {
    let mut engine = Engine::default();
    three(&mut engine)?;
    let all = search(&engine, "")?;
    assert_eq!(all.hits.len(), 3 + 10);
    let f = &all.facets;
    let value = |values: &[ModelFacetValue], v: &str| {
        values.iter().find(|x| x.value == v).map_or(0, |x| x.count)
    };
    assert_eq!(value(&f.format, "step"), 2);
    assert_eq!(value(&f.format, "gltf"), 1);
    assert_eq!(value(&f.mount, "smd"), 2 + 8);
    assert_eq!(value(&f.mount, "tht"), 1 + 2);
    assert_eq!(value(&f.pins, "8"), 2);
    assert_eq!(value(&f.licence, "unknown"), 1);
    assert_eq!(
        (f.has_leads, f.has_marking, f.has_textures, f.generated),
        (1, 1, 1, 10)
    );
    let limited = super::search(
        engine.project(),
        engine.drafts(),
        &CatalogContext::default(),
        "",
        None,
        2,
    )?;
    assert_eq!((limited.hits.len(), limited.more), (2, 11));
    assert_eq!(limited.facets, all.facets, "facets cover the whole match");
    // Text tiers: an exact name first.
    assert_eq!(
        library_names(&engine, "body")?,
        ["SOIC-8 body", "Two-lead body"]
    );
    Ok(())
}

#[test]
fn only_the_latest_revision_is_listed_and_the_query_is_on_the_wire() -> Fallible<()> {
    let mut engine = Engine::default();
    let a = packed(&bytes("latest"), "model/step");
    let r1 = named(&mut engine, "Two-lead body", model(&a), &a)?;
    let mut next = model(&a);
    next.hints.pins = Some(3);
    edit(
        &mut engine,
        vec![crate::library::DraftEdit {
            id: r1.id,
            meta: meta("Two-lead body"),
            definition: crate::library::Definition::Model(Box::new(next)),
        }],
    )?;
    publish(&mut engine, vec![r1.id])?;
    let QueryResult::Models(report) = engine.query(Query::Models {
        query: "kind:model has:leads".into(),
        fits: None,
        limit: None,
    })?
    else {
        return Err("a model report".into());
    };
    assert_eq!(report.hits.len(), 1);
    let ModelHitKind::Library { model, .. } = report.hits[0].kind else {
        return Err("a library hit".into());
    };
    assert_eq!(model.revision, Revision(2));
    assert_eq!(report.hits[0].hints.pins, Some(3));
    assert_eq!(report.hits[0].body_mm, Some([4.0, 3.0, 1.0]));
    Ok(())
}

#[test]
fn source_models_are_listed_with_their_source() -> Fallible<()> {
    let mut engine = Engine::default();
    three(&mut engine)?;
    let mut source = SourceCatalog {
        library: LibraryId(5),
        name: "Shared".into(),
        ..SourceCatalog::default()
    };
    source.fields.models = engine.project().library.models[..1].to_vec().into();
    let context = CatalogContext {
        sources: vec![std::sync::Arc::new(source)],
        ..CatalogContext::default()
    };
    let report = super::search(
        &Project::default(),
        &Drafts::default(),
        &context,
        "src:library",
        None,
        10,
    )?;
    assert_eq!(names(&report), ["Two-lead body"]);
    assert_eq!(report.hits[0].source, Source::Library(LibraryId(5)));
    Ok(())
}

/// A copy of `base` with leads on `pads` (designation, datum point).
fn leads_on(base: &ModelDefinition, pads: &[(String, [f64; 2])], shift: f64) -> ModelDefinition {
    let mut m = base.clone();
    m.bounds_nm = crate::tests::support::nm_bounds([[-20.0, -20.0, 0.0], [20.0, 20.0, 1.0]]);
    m.leads = pads
        .iter()
        .enumerate()
        .map(|(i, (designation, p))| LeadTag {
            key: format!("lead-{i}"),
            designation: designation.clone(),
            contact_nm: crate::tests::support::nm3([p[0] * shift, p[1], 0.0]),
        })
        .collect();
    m.marking_area = None;
    m
}

#[test]
fn fits_rank_lead_fits_then_bounds_and_never_assign() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let package: PackageDefinition =
        match &engine.drafts().get(f.package).ok_or("package")?.definition {
            crate::library::Definition::Package(p) => p.clone(),
            _ => return Err("package".into()),
        };
    let footprint = match &engine
        .drafts()
        .get(f.footprint)
        .ok_or("footprint")?
        .definition
    {
        crate::library::Definition::Footprint(fp) => fp.clone(),
        _ => return Err("footprint".into()),
    };
    let pads: Vec<(String, [f64; 2])> = package
        .terminals
        .iter()
        .filter_map(|t| {
            let pad = footprint.pads.iter().find(|p| p.terminal == Some(t.id))?;
            Some((
                t.designation.clone(),
                package_fit::to_datum(&footprint.datum, pad.position()),
            ))
        })
        .collect();
    assert!(pads.len() >= 2, "the demo package lands two terminals");
    let base = |tag: &str| {
        let a = packed(&bytes(tag), "model/step");
        (model(&a), a)
    };
    let (m, a) = base("fit-exact");
    named(&mut engine, "Exact leads", leads_on(&m, &pads, 1.0), &a)?;
    let (m, a) = base("fit-wide");
    named(&mut engine, "Wide leads", leads_on(&m, &pads, 1.5), &a)?;
    let (mut m, a) = base("fit-untagged");
    m.leads.clear();
    m.marking_area = None;
    let [l, w, h] = package.envelope();
    m.bounds_nm =
        crate::tests::support::nm_bounds([[-l / 2.0, -w / 2.0, 0.0], [l / 2.0 + 0.3, w / 2.0, h]]);
    named(&mut engine, "Untagged body", m, &a)?;
    let before = engine.to_file();
    let report = super::search(
        engine.project(),
        engine.drafts(),
        &CatalogContext::default(),
        "",
        Some(f.package),
        MAX_MODEL_HITS,
    )?;
    let order: Vec<(&str, Option<FitScore>)> = report
        .hits
        .iter()
        .map(|h| (h.name.as_str(), h.fit))
        .collect();
    let Some(FitScore::Leads { rms_mm }) = order[0].1 else {
        return Err(format!("a lead fit first: {order:?}").into());
    };
    assert_eq!(order[0].0, "Exact leads");
    assert!(rms_mm < 1e-6, "{rms_mm}");
    assert!(
        matches!(order[1], ("Wide leads", Some(FitScore::Leads { rms_mm })) if rms_mm > 0.1),
        "{order:?}"
    );
    // The package's own generated family fills the envelope exactly.
    let own = crate::library::templates::catalog::FamilyKind::of(&package.form);
    if own.is_some() {
        assert!(
            matches!(order[2], (_, Some(FitScore::Bounds { excess_mm })) if excess_mm == 0.0),
            "{order:?}"
        );
    }
    let untagged = report
        .hits
        .iter()
        .find(|h| h.name == "Untagged body")
        .ok_or("untagged")?;
    let Some(FitScore::Bounds { excess_mm }) = untagged.fit else {
        return Err("bounds score".into());
    };
    assert!((excess_mm - 0.3).abs() < 1e-9, "{excess_mm}");
    assert!(
        report.hits.last().is_some_and(|h| h.fit.is_none()),
        "unscored last"
    );
    assert_eq!(engine.to_file(), before, "a search changes nothing");
    // A package that does not exist is named.
    assert!(
        super::search(
            engine.project(),
            engine.drafts(),
            &CatalogContext::default(),
            "",
            Some(DefinitionId(999_999)),
            10
        )
        .is_err()
    );
    Ok(())
}
