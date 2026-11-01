//! The Design rules panel: live per-field overrides with reset, the
//! previewed profile switch as one undo step, saving the rules as a user
//! profile, and "Show rule" from a board finding.
use super::*;
use egui::{Pos2, Shape};
use pcbcraft_core::Engine;
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn frame(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Operation>) {
    let mut command = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(700.0, 1400.0),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            command = editor.subject_slot(ui, engine).output;
            let mark = refusal::take_mark(ui.ctx());
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("test-mark"), mark));
        },
    );
    output.textures_delta.clear();
    (output, command)
}

fn marked(ctx: &egui::Context) -> Option<Origin> {
    ctx.data(|d| d.get_temp::<Option<Origin>>(egui::Id::new("test-mark")))
        .flatten()
}

fn texts(output: &egui::FullOutput) -> Vec<String> {
    fn walk(shape: &Shape, out: &mut Vec<String>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            Shape::Text(text) => out.push(text.galley.job.text.clone()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for shape in &output.shapes {
        walk(&shape.shape, &mut out);
    }
    out
}

/// Centres of every text equal to `label`.
fn positions(output: &egui::FullOutput, label: &str) -> Vec<Pos2> {
    fn walk(shape: &Shape, label: &str, out: &mut Vec<Pos2>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, label, out)),
            Shape::Text(text) if text.galley.job.text == label => {
                out.push(text.pos + text.galley.size() * 0.5);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for shape in &output.shapes {
        walk(&shape.shape, label, &mut out);
    }
    out
}

fn press(point: Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![
        egui::Event::PointerMoved(point),
        button(true),
        button(false),
    ]
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// Click the text `label` (the last of several, when `last`).
fn click(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    label: &str,
) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
    let (output, _) = frame(editor, engine, ctx, vec![]);
    let point = *positions(&output, label)
        .first()
        .ok_or_else(|| format!("missing {label}"))?;
    Ok(frame(editor, engine, ctx, press(point)).1)
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    ctx
}

fn library() -> Result<(tempfile::TempDir, ProfileLibrary), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let library = ProfileLibrary::new(dir.path().join(rule_profiles::PROFILES_DIR));
    Ok((dir, library))
}

#[test]
fn commits_set_clear_or_skip_an_override() -> TestResult {
    let mut rules = ProjectRules::default();
    let key = RuleKey::MaskWebMin;
    assert!(
        commit_for(&rules, key, RuleValue::Length(0.1)).is_none(),
        "unchanged"
    );
    let Some(set) = commit_for(&rules, key, RuleValue::Length(0.15)) else {
        return Err("no override".into());
    };
    assert!(
        matches!(set, RulesCommand::SetOverride { key: k, value: RuleValue::Length(v) }
        if k == key && (v - 0.15).abs() < 1e-12)
    );
    rules::apply(&mut rules, set)?;
    // Back to the profile value removes the override instead of storing it.
    assert!(matches!(
        commit_for(&rules, key, RuleValue::Length(0.1)),
        Some(RulesCommand::ClearOverride { .. })
    ));
    assert!(commit_for(&rules, key, RuleValue::Length(0.15)).is_none());
    assert_eq!(rules_summary(&rules), "Pcbcraft default · 1 override");
    Ok(())
}

#[test]
fn values_and_provenance_read_as_text() {
    assert_eq!(value_text(RuleValue::Length(0.2)), "0.2 mm");
    assert_eq!(value_text(RuleValue::Angle(45.0)), "45°");
    assert_eq!(value_text(RuleValue::Ratio(None)), "Not checked");
    assert_eq!(value_text(RuleValue::Ratio(Some(10.0))), "10:1");
    assert_eq!(
        value_text(RuleValue::Connection(PadConnection::Thermal)),
        "Thermal relief"
    );
    assert_eq!(
        provenance_line(&builtin::fine().provenance),
        "Built-in · estimated"
    );
    let entered = ProfileProvenance {
        origin: ProfileOrigin::User,
        basis: ProfileBasis::FabricatorEntered,
        source: "Example Fab".into(),
        url: String::new(),
        date: "2026-10-30".into(),
        notes: String::new(),
    };
    assert_eq!(
        provenance_line(&entered),
        "Your profile · from fabricator data · Example Fab · 2026-10-30"
    );
    // Every label the panel shows passes the message style check.
    for key in RuleKey::all() {
        let label = sentence(&key.label());
        assert!(
            pcbcraft_core::issues::style::check(&label).is_empty(),
            "{label}"
        );
    }
}

/// Board card › Rules… docks the panel; a value typed into a field
/// commits once on Enter as one override, marked as that field's; the
/// row then offers Reset, which clears it; undo restores each step.
#[test]
fn a_field_commits_one_override_and_reset_clears_it() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert!(texts(&output).iter().any(|t| t == "Pcbcraft default"));
    assert!(click(&mut editor, &engine, &ctx, "Rules…")?.is_none());
    assert_eq!(editor.subject(), Subject::Dialog);
    let (dir, library) = library()?;
    editor.rules = Some(RulesPanel::new(Some(library), None));
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    for expected in [
        "Design rules",
        "Min track width",
        "Pcbcraft default · built-in",
    ] {
        assert!(shown.iter().any(|t| t == expected), "{expected}: {shown:?}");
    }
    // The value field is the rightmost "0.05 mm" of the row (the profile
    // column shows the same value).
    let field = positions(&output, "0.05 mm")
        .into_iter()
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .ok_or("no min track width field")?;
    frame(&mut editor, &engine, &ctx, press(field));
    let (_, none) = frame(
        &mut editor,
        &engine,
        &ctx,
        vec![
            key(egui::Key::A, egui::Modifiers::COMMAND),
            egui::Event::Text("0.08".into()),
        ],
    );
    assert!(none.is_none(), "nothing commits while typing");
    let (_, op) = frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    let op = op.ok_or("Enter did not commit")?;
    let Operation::Rules(RulesCommand::SetOverride {
        key: RuleKey::TrackWidthMin,
        value: RuleValue::Length(width),
    }) = op
    else {
        return Err(format!("unexpected {op:?}").into());
    };
    assert!((width - 0.08).abs() < 1e-12);
    assert_eq!(
        marked(&ctx),
        Some(Origin::Field(field_id(RuleKey::TrackWidthMin)))
    );
    let before = engine.revision();
    engine.apply(before, op)?;
    assert_eq!(engine.project().rules.overrides.len(), 1);
    let (output, again) = frame(&mut editor, &engine, &ctx, vec![]);
    assert!(again.is_none(), "sent once");
    let shown = texts(&output);
    assert!(shown.iter().any(|t| t == "0.08 mm"), "{shown:?}");
    assert!(shown.iter().any(|t| t == "1 project override"));
    // Reset clears the override back to the profile value.
    let reset = click(&mut editor, &engine, &ctx, "Reset")?.ok_or("Reset did nothing")?;
    assert!(matches!(
        reset,
        Operation::Rules(RulesCommand::ClearOverride {
            key: RuleKey::TrackWidthMin
        })
    ));
    engine.apply(engine.revision(), reset)?;
    assert!(engine.project().rules.overrides.is_empty());
    engine.undo()?;
    assert_eq!(engine.project().rules.overrides.len(), 1);
    engine.undo()?;
    assert!(engine.project().rules.is_default());
    // Done closes the panel; the Board card is back.
    assert!(click(&mut editor, &engine, &ctx, "Done")?.is_none());
    assert!(editor.rules.is_none());
    assert_eq!(editor.subject(), Subject::Properties);
    drop(dir);
    Ok(())
}

/// Choosing another profile shows its impact first; Switch commits
/// `rules.set_profile` as one undo step and the panel returns to the
/// table, which now shows the new profile.
#[test]
fn a_profile_switch_is_previewed_then_one_undo_step() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (_dir, library) = library()?;
    editor.open_rules_in(Some(library), None);
    assert!(click(&mut editor, &engine, &ctx, "Pcbcraft default · built-in")?.is_none());
    assert!(click(&mut editor, &engine, &ctx, "Fine 4/4 · built-in")?.is_none());
    assert_eq!(editor.subject(), Subject::Review);
    assert!(
        engine.project().rules.is_default(),
        "previewing changes nothing"
    );
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("Fine 4/4 · ") && t.contains("rules change")),
        "{shown:?}"
    );
    let switch =
        click(&mut editor, &engine, &ctx, "Switch to Fine 4/4")?.ok_or("Switch did nothing")?;
    let Operation::Rules(RulesCommand::SetProfile { profile, .. }) = &switch else {
        return Err("Switch issued another command".into());
    };
    assert_eq!(**profile, builtin::fine());
    assert_eq!(marked(&ctx), Some(Origin::Dialog(preview_id())));
    let before = engine.revision();
    engine.apply(before, switch)?;
    assert_eq!(engine.project().rules.profile, builtin::fine());
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.subject(), Subject::Dialog, "back to the table");
    assert!(texts(&output).iter().any(|t| t == "Fine 4/4 · built-in"));
    engine.undo()?;
    assert!(engine.project().rules.is_default(), "one undo step");
    Ok(())
}

/// A stale preview offers Recompute and cannot be applied.
#[test]
fn a_stale_preview_must_be_recomputed() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    editor.open_rules_in(None, None);
    if let Some(panel) = &mut editor.rules {
        panel.preview(&engine, builtin::hdi(), false);
    }
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::SetOverride {
            key: RuleKey::MaskWebMin,
            value: RuleValue::Length(0.2),
        }),
    )?;
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert!(texts(&output).iter().any(|t| t == "The project changed"));
    assert!(click(&mut editor, &engine, &ctx, "Switch to HDI 3/3")?.is_none());
    assert!(click(&mut editor, &engine, &ctx, "Recompute")?.is_none());
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(!shown.iter().any(|t| t == "The project changed"));
    assert!(shown.iter().any(|t| t == "Keep project overrides"));
    let switch = click(&mut editor, &engine, &ctx, "Switch to HDI 3/3")?.ok_or("no switch")?;
    engine.apply(engine.revision(), switch)?;
    assert!(
        engine.project().rules.overrides.is_empty(),
        "dropped by default"
    );
    Ok(())
}

/// Save as profile writes the effective rules (profile plus overrides)
/// with the entered provenance to the library, and switches the board to
/// it without changing any effective rule.
#[test]
fn save_as_profile_writes_the_library_and_switches_the_board() -> TestResult {
    let mut engine = Engine::demo()?;
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::SetOverride {
            key: RuleKey::MaskWebMin,
            value: RuleValue::Length(0.08),
        }),
    )?;
    let effective = engine.project().rules.effective();
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (_dir, library) = library()?;
    editor.open_rules_in(Some(library.clone()), None);
    assert!(click(&mut editor, &engine, &ctx, "Save as profile…")?.is_none());
    if let Some(RulesPanel {
        mode: Mode::Save(form),
        ..
    }) = &mut editor.rules
    {
        form.basis = ProfileBasis::FabricatorEntered;
        form.source = "Example Fab".into();
        form.date = "2026-10-30".into();
    } else {
        return Err("the save form is not open".into());
    }
    let op = click(&mut editor, &engine, &ctx, "Save")?.ok_or("Save did not switch")?;
    let stored = library.list()?.profiles;
    assert_eq!(stored.len(), 1);
    let saved = &stored[0];
    assert_eq!(saved.name, "Pcbcraft default copy");
    assert_eq!(saved.id, "pcbcraft-default-copy");
    assert_eq!(saved.rules, effective);
    assert_eq!(saved.provenance.basis, ProfileBasis::FabricatorEntered);
    assert_eq!(saved.provenance.origin, ProfileOrigin::User);
    engine.apply(engine.revision(), op)?;
    let rules = &engine.project().rules;
    assert_eq!(rules.profile, *saved);
    assert!(rules.overrides.is_empty());
    assert_eq!(rules.effective(), effective, "nothing else changed");
    // The library row is listed, and the picker names the board's profile.
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(
        shown
            .iter()
            .any(|t| t == "Pcbcraft default copy · fabricator data"),
        "{shown:?}"
    );
    assert!(shown.iter().any(|t| t == "Pcbcraft default copy"));
    // A second save under the same name is refused on the dialog.
    assert!(click(&mut editor, &engine, &ctx, "Save as profile…")?.is_none());
    if let Some(RulesPanel {
        mode: Mode::Save(form),
        ..
    }) = &mut editor.rules
    {
        form.name = "Pcbcraft default copy".into();
    }
    assert!(click(&mut editor, &engine, &ctx, "Save")?.is_none());
    assert!(matches!(
        &editor.rules,
        Some(RulesPanel { mode: Mode::Save(form), .. }) if form.refusal.is_some()
    ));
    assert_eq!(library.list()?.profiles.len(), 1);
    Ok(())
}

/// "Show rule" on a board finding opens the panel at its group, even a
/// folded one, and marks the rule's row.
#[test]
fn show_rule_opens_the_folded_group_at_its_rule() -> TestResult {
    let engine = Engine::demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (output, _) = {
        editor.open_rules_in(None, None);
        frame(&mut editor, &engine, &ctx, vec![])
    };
    assert!(
        !texts(&output).iter().any(|t| t == "Min via annular ring"),
        "Holes & vias starts folded"
    );
    editor.open_rules_in(None, Some(RuleTopic::ViaAnnularRing));
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert!(texts(&output).iter().any(|t| t == "Min via annular ring"));
    assert_eq!(
        topic_for("board.via_annular_ring"),
        Some(RuleTopic::ViaAnnularRing)
    );
    assert_eq!(topic_for("board.unrouted"), None);
    for (code, topic) in TOPICS {
        if let Some(key) = topic.key() {
            assert_eq!(key.group(), topic.group(), "{code}");
        }
    }
    Ok(())
}

#[test]
fn impact_text_counts_changes_and_issues() {
    let impact = ProfileImpact {
        changes: rules::rule_changes(
            &ProjectRules::default(),
            &ProjectRules {
                profile: builtin::fine(),
                ..ProjectRules::default()
            },
        )
        .changes,
        dropped_overrides: vec![RuleKey::MaskWebMin],
        new_issues: vec!["Clearance R1 ↔ R2".into()],
        resolved_issues: Vec::new(),
        refusal: None,
    };
    let summary = impact_summary("Fine 4/4", &impact);
    assert!(summary.starts_with("Fine 4/4 · "), "{summary}");
    assert!(summary.ends_with("rules change · 1 new issue"), "{summary}");
    let groups = impact_groups(&impact);
    let titles: Vec<_> = groups.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Rules that change", "Overrides dropped", "New issues"]
    );
    assert!(
        groups[0]
            .lines
            .iter()
            .any(|l| l == "Min track width: 0.05 mm → 0.1016 mm"),
        "{:?}",
        groups[0].lines
    );
}

/// An Issues row of a rule-based board finding offers "Show rule", which
/// opens the panel at that rule; other rows keep their own Fix or none.
#[test]
fn board_rule_findings_offer_show_rule() {
    use pcbcraft_core::issues::{IssueArea, IssueDetail, IssueGroup, IssueRow, Severity};
    let row = |code: &str| IssueRow {
        severity: Severity::Error,
        area: IssueArea::Board,
        group: IssueGroup {
            code: code.into(),
            key: code.into(),
            title: "Clearance".into(),
        },
        title: "Clearance R1 ↔ R2".into(),
        detail: IssueDetail::default(),
        finding: pcbcraft_core::Finding::new(code, String::new(), Vec::new()),
        also: Default::default(),
    };
    match crate::issues::fix(&row("board.via_hole_spacing")) {
        Some(crate::issues::Fix::Open {
            label: "Show rule",
            dialog: crate::issues::FixDialog::DesignRules(RuleTopic::HoleSpacing),
        }) => {}
        other => panic_free_fail(&format!("{other:?}")),
    }
    assert!(crate::issues::fix(&row("board.unrouted")).is_none());
    assert_eq!(
        crate::issues::fix(&row("board.unplaced")).map(|f| f.label()),
        Some("Update PCB…")
    );
}

/// Fail the test without the denied `panic!` family.
fn panic_free_fail(what: &str) {
    assert!(what.is_empty(), "unexpected fix {what}");
}
