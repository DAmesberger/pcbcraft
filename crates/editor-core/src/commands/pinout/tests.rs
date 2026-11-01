//! Slice C3 behaviour: typed pinout preview and apply over the original
//! EXM-48 fixture, the stale-base refusal, matching by number on
//! re-import, partial acceptance and the F8 review gate.
use crate::commands::Outcome;
use crate::completeness::CheckTarget;
use crate::diagnostics::{Error, Stage};
use crate::functions::MuxCode;
use crate::ids::{DefinitionId, TerminalId};
use crate::library::pinout::{
    ChangeKind, EXM48, ImportReview, PinoutAccept, PinoutApplied, PinoutApply, PinoutDocument,
    PinoutPlan, PinoutTarget, ReviewReason,
};
use crate::library::{ComponentDefinition, Definition, DraftEdit, TerminalClass};
use crate::tests::support::{Snapshot, check};
use crate::{Engine, LibraryCommand, Operation, Query, QueryResult};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn exm48() -> Fallible<PinoutDocument> {
    Ok(serde_json::from_str(EXM48)?)
}

fn new_target() -> PinoutTarget {
    PinoutTarget::New {
        name: "EXM-48".into(),
        row: None,
    }
}

fn preview(
    engine: &Engine,
    target: &PinoutTarget,
    document: &PinoutDocument,
) -> crate::Result<PinoutPlan> {
    match engine.query(Query::PinoutPreview {
        target: target.clone(),
        document: Box::new(document.clone()),
    })? {
        QueryResult::PinoutPlan(plan) => Ok(*plan),
        other => Err(Error::Invalid(format!("unexpected result {other:?}"))),
    }
}

fn apply(
    engine: &mut Engine,
    target: &PinoutTarget,
    document: &PinoutDocument,
    base: u64,
    accept: PinoutAccept,
) -> crate::Result<PinoutApplied> {
    let operation = Operation::Library(LibraryCommand::PinoutApply(Box::new(PinoutApply {
        target: target.clone(),
        document: document.clone(),
        base,
        accept,
    })));
    match engine.apply(engine.revision(), operation)? {
        Outcome::PinoutApplied { report } => Ok(report),
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

fn component(engine: &Engine, id: DefinitionId) -> Fallible<ComponentDefinition> {
    match engine.drafts().get(id).map(|d| &d.definition) {
        Some(Definition::Component(c)) => Ok(c.clone()),
        _ => Err("no component draft".into()),
    }
}

fn terminal_named(c: &ComponentDefinition, name: &str) -> Fallible<TerminalId> {
    c.terminals
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.id)
        .ok_or_else(|| format!("no terminal {name}").into())
}

/// Preview, then apply at the preview's base.
fn import_new(engine: &mut Engine, document: &PinoutDocument) -> Fallible<PinoutApplied> {
    let plan = preview(engine, &new_target(), document)?;
    Ok(apply(
        engine,
        &new_target(),
        document,
        plan.base,
        PinoutAccept::All,
    )?)
}

fn count(plan: &PinoutPlan, object: &str, kind: ChangeKind) -> usize {
    plan.changes
        .iter()
        .filter(|c| c.object == object && c.kind == kind)
        .count()
}

#[test]
fn the_exm48_fixture_previews_as_a_new_component_without_changing_anything() -> Fallible<()> {
    let engine = Engine::default();
    let before = Snapshot::of(&engine);
    let document = exm48()?;
    let plan = preview(&engine, &new_target(), &document)?;
    before.assert_kept(&engine, "a preview is read-only");
    assert_eq!(count(&plan, "terminal", ChangeKind::Added), 49);
    assert_eq!(count(&plan, "peripheral", ChangeKind::Added), 8);
    assert_eq!(count(&plan, "rule", ChangeKind::Added), 4);
    // QFN-48 numbers all 49 rows, LQFP-48 the 48 without the exposed pad.
    assert_eq!(count(&plan, "package_terminal", ChangeKind::Added), 49 + 48);
    let reasons: Vec<(&str, ReviewReason)> = plan
        .review
        .iter()
        .map(|r| (r.row.as_str(), r.reason))
        .collect();
    assert_eq!(
        reasons,
        vec![
            ("r47", ReviewReason::Uncertain),
            ("r49", ReviewReason::VariantsDisagree)
        ]
    );
    let codes: Vec<&str> = plan.findings.iter().map(|f| f.code.as_str()).collect();
    // F8: the flagged row blocks publishing; the rest are unreviewed.
    let uncertain: Vec<_> = plan
        .findings
        .iter()
        .filter(|f| f.code == "pinout.uncertain")
        .collect();
    assert_eq!(uncertain.len(), 1, "{codes:?}");
    assert!(uncertain.iter().all(|f| f.blocks_stage(Stage::Publish)));
    assert!(
        plan.findings
            .iter()
            .any(|f| f.code == "pinout.unreviewed" && f.blocks.is_none()),
        "{codes:?}"
    );
    // The fixture states every peripheral, role, default and count.
    for code in [
        "function.resource_unknown",
        "function.design_default",
        "function.combination",
        "function.label",
        "package.designation",
        "package.terminal_count",
        "component.pin_missing",
        "binding.mapping_missing",
        "symbol.pin_off_grid",
    ] {
        assert!(!codes.contains(&code), "{code}: {codes:?}");
    }
    // A preview made twice is the same plan.
    assert_eq!(preview(&engine, &new_target(), &document)?, plan);
    Ok(())
}

#[test]
fn apply_writes_the_drafts_as_one_undo_step_with_every_row_unreviewed() -> Fallible<()> {
    let mut engine = Engine::default();
    let start = engine.document().clone();
    let document = exm48()?;
    let applied = import_new(&mut engine, &document)?;
    // Component, symbol, then a package and a binding per variant.
    assert_eq!(applied.drafts.len(), 6);
    assert_eq!(applied.unreviewed.len(), 49);
    let id = *applied.drafts.first().ok_or("no component")?;
    let c = component(&engine, id)?;
    assert_eq!(c.terminals.len(), 49);
    assert_eq!(c.resources.len(), 8);
    assert_eq!(c.pin_rules.len(), 4);
    assert_eq!(c.designator_prefix, "U");
    let p0_2 = c
        .terminals
        .iter()
        .find(|t| t.name == "P0_2")
        .and_then(|t| t.functions())
        .ok_or("P0_2")?;
    let tx = p0_2
        .functions
        .iter()
        .find(|f| f.label == "uart0_tx")
        .ok_or("uart0_tx")?;
    assert_eq!(tx.mux, Some(MuxCode("ALT1".into())));
    let role = tx.resource_role.ok_or("explicit role")?;
    let uart0 = c
        .resources
        .iter()
        .find(|r| r.id == role.resource)
        .ok_or("UART0")?;
    assert_eq!(uart0.label, "UART0");
    assert_eq!(uart0.kind.as_deref(), Some("uart"));
    assert_eq!(uart0.role(role.role).map(|r| r.label.as_str()), Some("tx"));
    assert_eq!(
        p0_2.gpio.as_ref().map(|g| (g.port.as_str(), g.index)),
        Some(("P0", 2))
    );
    let nc = c.terminals.iter().find(|t| t.name == "NC").ok_or("NC")?;
    assert_eq!(nc.class, TerminalClass::DeviceNc);
    let review = c.review.as_deref().ok_or("review state")?;
    assert_eq!(review.unreviewed.len(), 49);
    assert_eq!(
        review.uncertain.iter().copied().collect::<Vec<_>>(),
        vec![terminal_named(&c, "P2_7")?]
    );
    let draft = engine.drafts().get(id).ok_or("draft")?;
    assert!(
        draft
            .meta
            .provenance
            .contains("EXM-48 controller datasheet")
    );
    engine.undo()?;
    assert_eq!(engine.document(), &start);
    assert!(!engine.can_undo(), "one undo step");
    Ok(())
}

#[test]
fn uncertain_rows_block_publishing_until_a_person_clears_them() -> Fallible<()> {
    let mut engine = Engine::default();
    let applied = import_new(&mut engine, &exm48()?)?;
    let id = *applied.drafts.first().ok_or("no component")?;
    let report = check(&engine, CheckTarget::Draft(id))?;
    assert!(!report.publishable);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == "pinout.uncertain" && f.blocks_stage(Stage::Publish))
    );
    let published = engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Publish {
            drafts: applied.drafts.clone(),
        }),
    );
    assert!(published.is_err(), "an uncertain row must not publish");
    // A person reviews: clear the uncertain flag, keep the rest unreviewed.
    let draft = engine.drafts().get(id).ok_or("draft")?.clone();
    let mut body = component(&engine, id)?;
    let mut review: ImportReview = body.review.as_deref().cloned().unwrap_or_default();
    review.uncertain.clear();
    body.review = Some(Box::new(review));
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Edit {
            edits: vec![DraftEdit {
                id,
                meta: draft.meta,
                definition: Definition::Component(body),
            }],
        }),
    )?;
    let report = check(&engine, CheckTarget::Draft(id))?;
    assert!(!report.findings.iter().any(|f| f.code == "pinout.uncertain"));
    let unreviewed = report
        .findings
        .iter()
        .find(|f| f.code == "pinout.unreviewed")
        .ok_or("unreviewed warning")?;
    assert_eq!(unreviewed.blocks, None);
    assert_eq!(unreviewed.objects.len(), 49);
    Ok(())
}

#[test]
fn reimporting_into_the_draft_keeps_terminals_by_number() -> Fallible<()> {
    let mut engine = Engine::default();
    let mut document = exm48()?;
    let applied = import_new(&mut engine, &document)?;
    let id = *applied.drafts.first().ok_or("no component")?;
    let first = component(&engine, id)?;
    let target = PinoutTarget::Draft { id };
    // The same document again changes nothing.
    let plan = preview(&engine, &target, &document)?;
    assert!(plan.changes.is_empty(), "{:?}", plan.changes);
    // Rename the row numbered 11 and move its TX to another mux code: the
    // terminal keeps its ID, the function keeps its ID, both are changes,
    // and the name contradicts the draft.
    let pin = document
        .pins
        .iter_mut()
        .find(|p| p.row == "r11")
        .ok_or("r11")?;
    pin.name = "P0_2X".into();
    for f in &mut pin.functions {
        if f.label == "uart0_tx" {
            f.mux = Some("ALT7".into());
        }
    }
    let plan = preview(&engine, &target, &document)?;
    let kept = terminal_named(&first, "P0_2")?;
    assert!(plan.changes.iter().any(|c| c.object == "terminal"
        && c.kind == ChangeKind::Changed
        && c.terminal == Some(kept)));
    assert!(plan.changes.iter().any(|c| c.object == "function"
        && c.kind == ChangeKind::Changed
        && c.terminal == Some(kept)));
    assert!(
        plan.review
            .iter()
            .any(|r| r.row == "r11" && r.reason == ReviewReason::ContradictsDraft)
    );
    let applied = apply(
        &mut engine,
        &target,
        &document,
        plan.base,
        PinoutAccept::All,
    )?;
    assert_eq!(applied.drafts, vec![id]);
    assert_eq!(applied.unreviewed, vec!["r11".to_owned()]);
    let after = component(&engine, id)?;
    assert_eq!(terminal_named(&after, "P0_2X")?, kept);
    let old_tx = first
        .terminal(kept)
        .and_then(|t| t.functions())
        .and_then(|f| f.functions.iter().find(|x| x.label == "uart0_tx"))
        .map(|f| f.id);
    let new_tx = after
        .terminal(kept)
        .and_then(|t| t.functions())
        .and_then(|f| f.functions.iter().find(|x| x.label == "uart0_tx"))
        .map(|f| (f.id, f.mux.clone()));
    assert_eq!(new_tx, old_tx.map(|id| (id, Some(MuxCode("ALT7".into())))));
    assert_eq!(after.next_local, first.next_local, "no new identities");
    Ok(())
}

#[test]
fn a_stale_plan_is_refused_and_changes_nothing() -> Fallible<()> {
    let mut engine = Engine::default();
    let document = exm48()?;
    let applied = import_new(&mut engine, &document)?;
    let id = *applied.drafts.first().ok_or("no component")?;
    let target = PinoutTarget::Draft { id };
    let plan = preview(&engine, &target, &document)?;
    // Someone edits the draft after the preview.
    let draft = engine.drafts().get(id).ok_or("draft")?.clone();
    let mut body = component(&engine, id)?;
    body.designator_prefix = "IC".into();
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Edit {
            edits: vec![DraftEdit {
                id,
                meta: draft.meta,
                definition: Definition::Component(body),
            }],
        }),
    )?;
    let before = Snapshot::of(&engine);
    let refused = apply(
        &mut engine,
        &target,
        &document,
        plan.base,
        PinoutAccept::All,
    );
    assert!(
        matches!(&refused, Err(Error::Conflict(m)) if m.starts_with("pinout.stale_base")),
        "{refused:?}"
    );
    before.assert_kept(&engine, "a stale plan");
    // A fresh preview applies.
    let plan = preview(&engine, &target, &document)?;
    apply(
        &mut engine,
        &target,
        &document,
        plan.base,
        PinoutAccept::All,
    )?;
    Ok(())
}

#[test]
fn excluded_rows_and_rules_are_not_imported() -> Fallible<()> {
    let mut engine = Engine::default();
    let document = exm48()?;
    let plan = preview(&engine, &new_target(), &document)?;
    let accept = PinoutAccept::Except(vec!["r47".into(), "rule:swd_as_gpio".into()]);
    let applied = apply(&mut engine, &new_target(), &document, plan.base, accept)?;
    let id = *applied.drafts.first().ok_or("no component")?;
    let c = component(&engine, id)?;
    assert_eq!(c.terminals.len(), 48);
    assert!(c.terminals.iter().all(|t| t.name != "P2_7"));
    assert_eq!(c.pin_rules.len(), 3);
    // Nothing uncertain was taken, so nothing blocks on F8.
    assert!(c.review.as_deref().is_some_and(|r| r.uncertain.is_empty()));
    let report = check(&engine, CheckTarget::Draft(id))?;
    assert!(!report.findings.iter().any(|f| f.code == "pinout.uncertain"));
    Ok(())
}

#[test]
fn document_problems_are_findings_and_unreadable_documents_refuse() -> Fallible<()> {
    let engine = Engine::default();
    let before = Snapshot::of(&engine);
    let mut document = exm48()?;
    // Findings: an undeclared peripheral, a default that is not a
    // function, a number used twice and a stated count that disagrees.
    let pins = &mut document.pins;
    if let Some(pin) = pins.iter_mut().find(|p| p.row == "r9") {
        if let Some(f) = pin.functions.iter_mut().find(|f| f.label == "uart0_cts") {
            f.peripheral = Some("UART9".into());
        }
        pin.default = Some("gpio_hiz".into());
    }
    if let Some(pin) = pins.iter_mut().find(|p| p.row == "r10") {
        pin.number.insert("LQFP-48".into(), "9".into());
    }
    if let Some(variant) = document.variants.first_mut() {
        variant.terminal_count = 50;
    }
    let plan = preview(&engine, &new_target(), &document)?;
    let codes: Vec<&str> = plan.findings.iter().map(|f| f.code.as_str()).collect();
    for code in [
        "function.resource_unknown",
        "function.design_default",
        "package.designation",
        "package.terminal_count",
    ] {
        assert!(codes.contains(&code), "{code}: {codes:?}");
    }
    // Refusals: format, duplicate rows, undeclared variants, unknown
    // excepted rows.
    let base = exm48()?;
    let mut wrong_format = base.clone();
    wrong_format.format = 2;
    assert!(matches!(
        preview(&engine, &new_target(), &wrong_format),
        Err(Error::Unsupported(_))
    ));
    let mut duplicate = base.clone();
    if let Some(pin) = duplicate.pins.get_mut(1) {
        pin.row = "r1".into();
    }
    assert!(matches!(
        preview(&engine, &new_target(), &duplicate),
        Err(Error::Invalid(_))
    ));
    let mut undeclared = base.clone();
    if let Some(pin) = undeclared.pins.first_mut() {
        pin.number.insert("SOIC-8".into(), "1".into());
    }
    assert!(matches!(
        preview(&engine, &new_target(), &undeclared),
        Err(Error::Invalid(_))
    ));
    let mut engine = engine;
    let plan = preview(&engine, &new_target(), &base)?;
    let refused = apply(
        &mut engine,
        &new_target(),
        &base,
        plan.base,
        PinoutAccept::Except(vec!["r999".into()]),
    );
    assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
    before.assert_kept(&engine, "refused documents");
    Ok(())
}

#[test]
fn the_fixture_is_its_own_schema_and_a_round_trip() -> Fallible<()> {
    let document = exm48()?;
    let text = serde_json::to_string(&document)?;
    let back: PinoutDocument = serde_json::from_str(&text)?;
    assert_eq!(back, document);
    let schema = crate::schemas();
    assert!(schema["pinout"]["properties"]["pins"].is_object());
    Ok(())
}
