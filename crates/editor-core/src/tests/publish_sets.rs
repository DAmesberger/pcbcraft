//! Gaps 8, 10 and 11 of the end-to-end run, through public commands: what
//! publishing does to its drafts, the closure `component.check_drafts`
//! reports, and the Parts a Resolve group's query finds. Parts are invented
//! (`ExampleCo`, `EX-…`).
use super::support::*;
use crate::catalog::{CatalogContext, Index, SearchOptions};
use crate::completeness::{DefinitionReport, ResolveReport};
use crate::library::{Definition, DraftEdit, PartDefinition, Provenance};
use crate::{
    BindingRef, DefinitionId, DefinitionKind, Engine, Error, LibraryCommand, Operation, PartRef,
    Query, QueryResult, Result, Revision,
};
use serde_json::json;

fn publish_op(id: Vec<DefinitionId>, close: bool) -> Operation {
    Operation::Library(LibraryCommand::Publish { drafts: id, close })
}

fn all_checks(engine: &Engine) -> Result<Vec<DefinitionReport>> {
    match engine.query(Query::CheckDrafts {})? {
        QueryResult::Drafts(reports) => Ok(reports),
        other => Err(Error::Invalid(format!("unexpected report {other:?}"))),
    }
}

fn report_of(reports: &[DefinitionReport], id: DefinitionId) -> Result<&DefinitionReport> {
    reports
        .iter()
        .find(|r| r.revision.id == id)
        .ok_or_else(|| Error::NotFound(format!("no report of {id}")))
}

fn ids(report: &DefinitionReport) -> Vec<DefinitionId> {
    let mut ids: Vec<_> = report.publish_with.iter().map(|r| r.id).collect();
    ids.sort();
    ids
}

fn sorted(mut ids: Vec<DefinitionId>) -> Vec<DefinitionId> {
    ids.sort();
    ids
}

// Gap 8.

#[test]
fn a_wire_publish_closes_its_drafts_by_default_and_can_keep_them() -> Result<()> {
    let wire = |params| {
        serde_json::from_value::<Operation>(json!({
            "editor": "Library",
            "operation": {"command": "library.publish", "params": params}
        }))
        .map_err(|e| Error::Invalid(e.to_string()))
    };
    assert!(matches!(
        wire(json!({"drafts": [1]}))?,
        Operation::Library(LibraryCommand::Publish { close: true, .. })
    ));
    assert!(matches!(
        wire(json!({"drafts": [1], "close": false}))?,
        Operation::Library(LibraryCommand::Publish { close: false, .. })
    ));
    assert!(wire(json!({"drafts": [1], "close": "yes"})).is_err());
    assert!(wire(json!({"drafts": [1], "closed": true})).is_err());

    let mut engine = Engine::default();
    let fixture = draft(&mut engine, &probe("Closing", passive()))?;
    let drafts_before = engine.drafts().clone();
    let revision = engine.revision();
    apply(&mut engine, publish_op(fixture.all(), true))?;
    assert!(engine.drafts().items.is_empty(), "the set's drafts closed");
    assert_eq!(engine.revision(), revision + 1, "one step");
    assert!(
        engine.project().library.component(fixture.r1()).is_some(),
        "the revisions are published"
    );

    engine.undo()?;
    assert_eq!(engine.drafts(), &drafts_before, "undo reopens them");
    assert!(engine.project().library.component(fixture.r1()).is_none());
    engine.redo()?;
    assert!(engine.drafts().items.is_empty(), "redo closes them again");

    // Only the set closes; a draft outside it stays.
    let mut engine = Engine::default();
    let fixture = draft(&mut engine, &probe("Partial", passive()))?;
    let other = new_draft(&mut engine, DefinitionKind::Component)?;
    apply(&mut engine, publish_op(fixture.all(), true))?;
    let left: Vec<_> = engine.drafts().items.iter().map(|d| d.id).collect();
    assert_eq!(left, [other]);

    // `close: false` keeps the old behaviour: a draft of the new revision.
    let mut engine = Engine::default();
    let fixture = draft(&mut engine, &probe("Kept", passive()))?;
    apply(&mut engine, publish_op(fixture.all(), false))?;
    assert_eq!(engine.drafts().items.len(), 5);
    assert_eq!(
        engine.drafts().get(fixture.component).and_then(|d| d.base),
        Some(Revision::FIRST)
    );
    Ok(())
}

#[test]
fn a_refused_publish_closes_nothing() -> Result<()> {
    let mut engine = Engine::default();
    let fixture = draft(&mut engine, &probe("Refused", passive()))?;
    let kept = Snapshot::of(&engine);
    for set in [
        vec![],
        vec![fixture.binding],
        vec![fixture.component, fixture.component],
        vec![DefinitionId(999_999)],
    ] {
        let refused = engine.apply(kept.revision(), publish_op(set.clone(), true));
        assert!(refused.is_err(), "{set:?}");
        kept.assert_kept(&engine, &format!("{set:?}"));
    }
    Ok(())
}

/// The run's friction: a library of more parts than the draft limit holds.
/// Five drafts a part for fourteen parts is 70, past the limit of 64.
#[test]
fn a_library_larger_than_the_draft_limit_publishes_without_discarding() -> Result<()> {
    let mut engine = Engine::default();
    let names: Vec<String> = (0..14).map(|n| format!("EX-PART-{n}")).collect();
    for name in &names {
        let fixture = draft(&mut engine, &probe(name, passive()))?;
        apply(&mut engine, publish_op(fixture.all(), true))?;
        assert!(engine.drafts().items.is_empty());
    }
    assert_eq!(engine.project().library.latest_components().len(), 14);

    // With the drafts kept open, the same library hits the limit.
    let mut engine = Engine::default();
    let mut limited = None;
    for name in &names {
        match draft(&mut engine, &probe(name, passive())) {
            Ok(fixture) => {
                apply(&mut engine, publish_op(fixture.all(), false))?;
            }
            Err(error) => {
                limited = Some(error);
                break;
            }
        }
    }
    assert_eq!(limited.as_ref().map(Error::code), Some("limit_exceeded"));
    Ok(())
}

// Gap 11.

#[test]
fn the_check_names_the_closure_publish_needs() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &probe("Closure", passive()))?;
    let reports = all_checks(&engine)?;

    // Hand-derived from the fixture's references. A package names nothing;
    // a footprint names its package; a symbol its owning component, which
    // brings that component's whole set.
    let package = report_of(&reports, f.package)?;
    assert!(package.publish_with.is_empty());
    assert!(package.publishable);
    let footprint = report_of(&reports, f.footprint)?;
    assert_eq!(ids(footprint), [f.package]);
    assert!(footprint.publishable);
    let whole_set_but =
        |id: DefinitionId| sorted(f.all().into_iter().filter(|other| *other != id).collect());
    assert_eq!(
        ids(report_of(&reports, f.binding)?),
        whole_set_but(f.binding)
    );
    assert_eq!(ids(report_of(&reports, f.symbol)?), whole_set_but(f.symbol));
    assert_eq!(
        ids(report_of(&reports, f.component)?),
        whole_set_but(f.component)
    );

    // Dependencies first, as the publish order.
    let binding = report_of(&reports, f.binding)?;
    let rank = |id: DefinitionId| {
        binding
            .publish_with
            .iter()
            .position(|r| r.id == id)
            .unwrap_or(usize::MAX)
    };
    assert!(rank(f.package) < rank(f.footprint));
    assert!(
        binding
            .publish_with
            .iter()
            .all(|r| r.revision == Revision::FIRST)
    );

    // The run's refusal now names what to add, and changes nothing.
    let kept = Snapshot::of(&engine);
    let refused = engine
        .apply(kept.revision(), publish_op(vec![f.binding], true))
        .err()
        .ok_or_else(|| Error::Invalid("publish accepted a partial set".into()))?;
    assert_eq!(refused.code(), "conflict");
    let message = refused.to_string();
    assert!(
        message.contains("is missing from the project library"),
        "{message}"
    );
    assert!(
        message.contains("needs drafts it does not list"),
        "{message}"
    );
    assert!(message.contains("Closure footprint"), "{message}");
    assert!(message.contains(&f.footprint.to_string()), "{message}");
    kept.assert_kept(&engine, "partial set");

    // The reported set publishes.
    let mut set = ids(binding);
    set.push(f.binding);
    apply(&mut engine, publish_op(set, true))?;
    assert!(engine.drafts().items.is_empty());
    Ok(())
}

#[test]
fn every_set_the_check_calls_publishable_publishes_and_blocked_sets_refuse() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &probe("Every", passive()))?;
    for id in f.all() {
        let reports = all_checks(&engine)?;
        let report = report_of(&reports, id)?;
        assert!(report.publishable, "{id}");
        let mut trial = Engine::from_file(engine.to_file())?;
        let mut set = ids(report);
        set.push(id);
        let result = trial.apply(trial.revision(), publish_op(set, true));
        assert!(result.is_ok(), "{id}: {result:?}");
    }

    // A footprint that cannot publish blocks everything that needs it,
    // without giving those drafts a finding of their own.
    rewrite(&mut engine, f.footprint, |d| {
        if let Definition::Footprint(footprint) = d {
            footprint.pads.clear();
        }
    })?;
    let reports = all_checks(&engine)?;
    let footprint = report_of(&reports, f.footprint)?;
    assert!(!footprint.publishable);
    assert!(
        footprint.blocked_by.is_empty(),
        "blocked by its own finding"
    );
    let binding = report_of(&reports, f.binding)?;
    assert!(!binding.publishable);
    assert_eq!(
        binding.blocked_by.iter().map(|r| r.id).collect::<Vec<_>>(),
        [f.footprint]
    );
    let package = report_of(&reports, f.package)?;
    assert!(package.publishable, "a package does not need its footprint");
    for id in f.all() {
        let reports = all_checks(&engine)?;
        let report = report_of(&reports, id)?;
        let mut set = ids(report);
        set.push(id);
        let mut trial = Engine::from_file(engine.to_file())?;
        let result = trial.apply(trial.revision(), publish_op(set, true));
        assert_eq!(result.is_ok(), report.publishable, "{id}: {result:?}");
    }
    Ok(())
}

#[test]
fn a_reference_to_a_stored_revision_needs_no_draft() -> Result<()> {
    let mut engine = Engine::default();
    let f = component(&mut engine, &probe("Stored", passive()))?;
    // The published package's lineage has an open draft (its pending r2),
    // and a new footprint of that package pins the stored r1: nothing more
    // is needed to publish it.
    assert!(engine.drafts().get(f.package).is_some());
    let footprint = new_draft(&mut engine, DefinitionKind::Footprint)?;
    rewrite(&mut engine, footprint, |d| {
        if let Definition::Footprint(body) = d {
            body.package = crate::PackageRef::new(f.package, Revision::FIRST);
        }
    })?;
    let reports = all_checks(&engine)?;
    assert!(report_of(&reports, footprint)?.publish_with.is_empty());
    Ok(())
}

#[test]
fn the_single_draft_check_reports_the_closure_too() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &probe("Single", passive()))?;
    let one = check(
        &engine,
        crate::completeness::CheckTarget::Draft(f.footprint),
    )?;
    assert_eq!(ids(&one), [f.package]);
    let wire = serde_json::to_value(crate::QueryResult::Definition(one))?;
    assert!(wire["result"]["publish_with"].is_array(), "{wire}");
    // A report with no closure writes neither field.
    let alone = check(&engine, crate::completeness::CheckTarget::Draft(f.package))?;
    let wire = serde_json::to_value(crate::QueryResult::Definition(alone))?;
    assert!(wire["result"].get("publish_with").is_none(), "{wire}");
    assert!(wire["result"].get("blocked_by").is_none(), "{wire}");
    Ok(())
}

// Gap 10.

fn part(engine: &mut Engine, mpn: &str, f: &Fixture) -> Result<PartRef> {
    let id = new_draft(engine, DefinitionKind::Part)?;
    let body = PartDefinition {
        next_local: 1,
        manufacturer: "ExampleCo".into(),
        mpn: mpn.into(),
        component: f.r1(),
        binding: BindingRef::new(f.binding, Revision::FIRST),
        specs: vec![],
        references: vec![],
        offers: vec![],
        provenance: Provenance::default(),
        top_mark: None,
    };
    edit(
        engine,
        vec![DraftEdit {
            id,
            meta: meta(mpn),
            definition: Definition::Part(Box::new(body)),
        }],
    )?;
    apply(engine, publish_op(vec![id], true))?;
    Ok(PartRef::new(id, Revision::FIRST))
}

fn found(engine: &Engine, query: &str) -> Result<Vec<PartRef>> {
    let index = Index::of_project(engine.project(), engine.drafts())?;
    let parsed = index.parse(query).map_err(Error::from)?;
    let hits = index.search(
        &parsed,
        &CatalogContext::default(),
        SearchOptions::default(),
    );
    Ok(hits.results.iter().filter_map(|h| h.entry.part()).collect())
}

#[test]
fn a_field_less_groups_query_finds_only_that_components_parts() -> Result<()> {
    let mut engine = Engine::default();
    let led = component(&mut engine, &probe("LED", passive()))?;
    let switch = component(&mut engine, &probe("Switch", passive()))?;
    let led_part = part(&mut engine, "EX-LED-1", &led)?;
    let switch_part = part(&mut engine, "EX-SW-1", &switch)?;
    place(&mut engine, &led, "D1", 1, [0.0, 0.0])?;
    place(&mut engine, &switch, "S1", 1, [10.0, 0.0])?;

    let QueryResult::ResolveGroups(ResolveReport { groups }) =
        engine.query(Query::ResolveGroups {})?
    else {
        return Err(Error::Invalid("unexpected report".into()));
    };
    assert_eq!(groups.len(), 2);
    assert_eq!(
        groups[0].query,
        format!("kind:part implements:{}/1", led.component.0)
    );
    assert_eq!(found(&engine, &groups[0].query)?, [led_part]);
    assert_eq!(found(&engine, &groups[1].query)?, [switch_part]);

    // What the group finds, the group can be assigned.
    apply(
        &mut engine,
        Operation::Instance(crate::InstanceCommand::AssignPart {
            instances: groups[0].instances.clone(),
            part: Some(led_part),
        }),
    )?;
    Ok(())
}

#[test]
fn implements_is_exact_identity_and_malformed_values_are_named() -> Result<()> {
    let mut engine = Engine::default();
    let led = component(&mut engine, &probe("LED", passive()))?;
    let led_part = part(&mut engine, "EX-LED-1", &led)?;
    let id = led.component.0;

    // Exact revision, lineage, another revision, another lineage. A
    // component row implements nothing, so it never matches.
    assert_eq!(found(&engine, &format!("implements:{id}/1"))?, [led_part]);
    assert_eq!(found(&engine, &format!("implements:{id}"))?, [led_part]);
    assert!(found(&engine, &format!("implements:{id}/2"))?.is_empty());
    assert!(found(&engine, &format!("implements:{}/1", led.symbol.0))?.is_empty());
    let index = Index::of_project(engine.project(), engine.drafts())?;
    let hits = index.search(
        &index
            .parse(&format!("implements:{id}"))
            .map_err(Error::from)?,
        &CatalogContext::default(),
        SearchOptions::default(),
    );
    assert_eq!(hits.results.len(), 1, "the Part, not the component it pins");

    // The name never stands in for the identity: `implements:LED` refuses.
    for bad in [
        "implements:LED",
        "implements:",
        "implements:0",
        "implements:1/0",
        "implements:1/2/3",
        "implements:1/x",
        "implements:-1",
    ] {
        let error = index.parse(bad).err();
        assert!(
            error
                .as_ref()
                .is_some_and(|e| e.to_string().contains("implements")),
            "{bad}: {error:?}"
        );
    }
    assert!(index.parse("implements>=1").is_err(), "a filter takes `:`");
    Ok(())
}
