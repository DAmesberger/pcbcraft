//! The Design rules panel: live per-field overrides with reset, the
//! previewed profile switch as one undo step, saving the rules as a user
//! profile, and "Show rule" from a board finding.
use super::*;
use egui::{Pos2, Shape};
use sparksmith_core::Engine;
use sparksmith_core::rules::fixtures;
use sparksmith_ui::fonts;

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
                .data_mut(|d| d.insert_temp(egui::Id::unique("test-mark"), mark));
        },
    );
    output.textures_delta.clear();
    (output, command)
}

fn marked(ctx: &egui::Context) -> Option<Origin> {
    ctx.data(|d| d.get_temp::<Option<Origin>>(egui::Id::unique("test-mark")))
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
    let mut rules = fixtures::rules()?;
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
    assert_eq!(rules_summary(&rules), "Test prototype · 1 override");
    assert_eq!(rules_summary(&ProjectRules::default()), "No fab profile");
    assert!(
        commit_for(&ProjectRules::default(), key, RuleValue::Length(0.15)).is_none(),
        "no override without a profile"
    );
    Ok(())
}

#[test]
fn values_and_sources_read_as_text() -> TestResult {
    assert_eq!(value_text(RuleValue::Fraction(0.15)), "0.15");
    assert_eq!(value_text(RuleValue::Length(0.2)), "0.2 mm");
    assert_eq!(value_text(RuleValue::Angle(45.0)), "45°");
    assert_eq!(value_text(RuleValue::Ratio(None)), "Not checked");
    assert_eq!(value_text(RuleValue::Ratio(Some(10.0))), "10:1");
    // The etch checks are unset at 0, and say so.
    assert_eq!(
        rule_text(RuleKey::CopperAngleMin, RuleValue::Angle(0.0)),
        "Not checked"
    );
    assert_eq!(
        rule_text(RuleKey::SliverWidthMin, RuleValue::Length(0.0)),
        "Not checked"
    );
    assert_eq!(
        rule_text(RuleKey::CopperAngleMin, RuleValue::Angle(90.0)),
        "90°"
    );
    assert_eq!(
        rule_text(RuleKey::SilkEdgeClearance, RuleValue::Length(0.0)),
        "0 mm"
    );
    assert_eq!(
        value_text(RuleValue::Connection(PadConnection::Thermal)),
        "Thermal relief"
    );
    assert_eq!(
        source_line(&fixtures::jlcpcb_example()?.source),
        "JLCPCB · PCB Manufacturing & Assembly Capabilities · read 2026-11-11"
    );
    // Every label the panel shows passes the message style check.
    for key in RuleKey::all() {
        let label = sentence(&key.label());
        assert!(
            sparksmith_core::issues::style::check(&label).is_empty(),
            "{label}"
        );
    }
    Ok(())
}

/// A board without a profile says so, and choosing one from the library
/// previews it and sets it as one undo step.
#[test]
fn a_board_without_a_profile_asks_for_one() -> TestResult {
    let mut engine = Engine::default();
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (_dir, library) = library()?;
    library.create(&fixtures::jlcpcb_example()?)?;
    editor.open_rules_in(Some(library), None);
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(
        shown
            .iter()
            .any(|t| t == "This board has no fab profile, so nothing is checked"),
        "{shown:?}"
    );
    assert!(click(&mut editor, &engine, &ctx, "Choose a profile…")?.is_none());
    assert!(click(&mut editor, &engine, &ctx, "JLCPCB 2-layer 1 oz")?.is_none());
    assert_eq!(editor.subject(), Subject::Review);
    let switch = click(&mut editor, &engine, &ctx, "Switch to JLCPCB 2-layer 1 oz")?
        .ok_or("Switch did nothing")?;
    engine.apply(engine.revision(), switch)?;
    assert_eq!(
        engine.project().rules.profile,
        Some(fixtures::jlcpcb_example()?)
    );
    engine.undo()?;
    assert!(engine.project().rules.profile.is_none(), "one undo step");
    Ok(())
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
    assert!(texts(&output).iter().any(|t| t == "Test prototype"));
    assert!(click(&mut editor, &engine, &ctx, "Rules…")?.is_none());
    assert_eq!(editor.subject(), Subject::Dialog);
    let (dir, library) = library()?;
    editor.rules = Some(RulesPanel::new(Some(library), None));
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    for expected in [
        "Design rules",
        "Min track width",
        "Test prototype · this board",
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
    assert_eq!(engine.project().rules, fixtures::rules()?);
    // Done closes the panel; the Board card is back.
    assert!(click(&mut editor, &engine, &ctx, "Done")?.is_none());
    assert!(editor.rules.is_none());
    assert_eq!(editor.subject(), Subject::Properties);
    drop(dir);
    Ok(())
}

/// Choosing another profile shows its impact first; Switch commits
/// `rules.set_fab_profile` as one undo step and the panel returns to the
/// table, which now shows the new profile.
#[test]
fn a_profile_switch_is_previewed_then_one_undo_step() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (_dir, library) = library()?;
    library.create(&fixtures::fine()?)?;
    editor.open_rules_in(Some(library), None);
    assert!(click(&mut editor, &engine, &ctx, "Test prototype · this board")?.is_none());
    assert!(click(&mut editor, &engine, &ctx, "Test fine")?.is_none());
    assert_eq!(editor.subject(), Subject::Review);
    assert_eq!(
        engine.project().rules,
        fixtures::rules()?,
        "previewing changes nothing"
    );
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("Test fine · ") && t.contains("rules change")),
        "{shown:?}"
    );
    let switch =
        click(&mut editor, &engine, &ctx, "Switch to Test fine")?.ok_or("Switch did nothing")?;
    let Operation::Rules(RulesCommand::SetProfile { profile, .. }) = &switch else {
        return Err("Switch issued another command".into());
    };
    assert_eq!(**profile, fixtures::fine()?);
    assert_eq!(marked(&ctx), Some(Origin::Dialog(preview_id())));
    let before = engine.revision();
    engine.apply(before, switch)?;
    assert_eq!(engine.project().rules.profile, Some(fixtures::fine()?));
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.subject(), Subject::Dialog, "back to the table");
    assert!(texts(&output).iter().any(|t| t == "Test fine"));
    engine.undo()?;
    assert_eq!(engine.project().rules, fixtures::rules()?, "one undo step");
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
        panel.preview(&engine, fixtures::fine()?, false);
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
    assert!(click(&mut editor, &engine, &ctx, "Switch to Test fine")?.is_none());
    assert!(click(&mut editor, &engine, &ctx, "Recompute")?.is_none());
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(!shown.iter().any(|t| t == "The project changed"));
    assert!(shown.iter().any(|t| t == "Keep project overrides"));
    let switch = click(&mut editor, &engine, &ctx, "Switch to Test fine")?.ok_or("no switch")?;
    engine.apply(engine.revision(), switch)?;
    assert!(
        engine.project().rules.overrides.is_empty(),
        "dropped by default"
    );
    Ok(())
}

/// Save as profile writes the profile with its overrides as values, under
/// the entered source, to the library, and switches the board to it
/// without changing any effective rule.
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
    assert!(click(&mut editor, &engine, &ctx, "Save as fab profile…")?.is_none());
    if let Some(RulesPanel {
        mode: Mode::Save(form),
        ..
    }) = &mut editor.rules
    {
        form.publisher = "Example Fab".into();
        form.date = "2026-10-30".into();
    } else {
        return Err("the save form is not open".into());
    }
    let op = click(&mut editor, &engine, &ctx, "Save")?.ok_or("Save did not switch")?;
    let stored = library.list()?.profiles;
    assert_eq!(stored.len(), 1);
    let saved = &stored[0];
    assert_eq!(saved.name, "Test prototype copy");
    assert_eq!(saved.id, "test-prototype-copy");
    assert_eq!(saved.rules(), &effective);
    assert_eq!(saved.source.publisher, "Example Fab");
    assert_eq!(
        saved.entry(RuleKey::MaskWebMin).map(|e| e.source.as_str()),
        Some("Project override")
    );
    engine.apply(engine.revision(), op)?;
    let rules = &engine.project().rules;
    assert_eq!(rules.profile.as_ref(), Some(saved));
    assert!(rules.overrides.is_empty());
    assert_eq!(rules.effective(), effective, "nothing else changed");
    // The library row is listed, and the picker names the board's profile.
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(
        shown
            .iter()
            .any(|t| t == "Test prototype copy · Example Fab"),
        "{shown:?}"
    );
    assert!(shown.iter().any(|t| t == "Test prototype copy"));
    // A second save under the same name is refused on the dialog.
    assert!(click(&mut editor, &engine, &ctx, "Save as fab profile…")?.is_none());
    if let Some(RulesPanel {
        mode: Mode::Save(form),
        ..
    }) = &mut editor.rules
    {
        form.name = "Test prototype copy".into();
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
fn impact_text_counts_changes_and_issues() -> TestResult {
    let impact = ProfileImpact {
        changes: rules::rule_changes(
            &fixtures::rules()?,
            &ProjectRules {
                profile: Some(fixtures::fine()?),
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
    Ok(())
}

/// An Issues row of a rule-based board finding offers "Show rule", which
/// opens the panel at that rule; other rows keep their own Fix or none.
#[test]
fn board_rule_findings_offer_show_rule() {
    use sparksmith_core::issues::{IssueArea, IssueDetail, IssueGroup, IssueRow, Severity};
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
        finding: sparksmith_core::Finding::new(code, String::new(), Vec::new()),
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

/// At the Properties column's width (the app's is about 260 points) the
/// rule rows stand one under the other at full height, as at any width:
/// they never overlap.
#[test]
fn the_rule_rows_do_not_overlap_in_the_properties_column() -> TestResult {
    let engine = Engine::demo()?;
    let ctx = context();
    sparksmith_ui::theme::apply(&ctx);
    let mut editor = PcbEditor::default();
    click(&mut editor, &engine, &ctx, "Rules…")?;
    assert_eq!(editor.subject(), Subject::Dialog);
    let mut output = None;
    for _ in 0..3 {
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(262.0, 900.0),
                )),
                ..Default::default()
            },
            |ui| {
                editor.subject_slot(ui, &engine);
            },
        );
        out.textures_delta.clear();
        output = Some(out);
    }
    let output = output.ok_or("no frame")?;
    let first = positions(&output, "Min track width");
    let second = positions(&output, "Default track width");
    let (Some(a), Some(b)) = (first.first(), second.first()) else {
        return Err(format!("rows not drawn: {:?}", texts(&output)).into());
    };
    let t = sparksmith_ui::theme::Tokens::get();
    assert!(b.y - a.y >= t.control_height, "rows {} apart", b.y - a.y);
    Ok(())
}

/// The Via, Route and Pair tools start tented as the profile's `tent vias`
/// says, until the tool's own Tented box is changed.
#[test]
fn the_via_tools_start_tented_as_the_profile_says() -> TestResult {
    let mut engine = Engine::demo()?;
    let mut editor = PcbEditor::default();
    assert!(editor.via_tented(engine.project()));
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::SetOverride {
            key: RuleKey::TentVias,
            value: RuleValue::Flag(false),
        }),
    )?;
    assert!(!editor.via_tented(engine.project()));
    editor.via_tented = Some(true);
    assert!(editor.via_tented(engine.project()), "the tool's own choice");
    Ok(())
}

/// Run 4, finding 1: a new project asks for its fab profile with the
/// built-in JLCPCB 2-layer 1 oz preselected (the 4-layer one beside it);
/// Use profile sets it as one undo step and closes the panel. Edit shape
/// then starts, and the Route card shows the profile's via size without
/// an error.
#[test]
fn a_new_project_asks_for_a_profile_with_jlcpcb_preselected() -> TestResult {
    let mut engine = Engine::default();
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (_dir, library) = library()?;
    // Without a profile, Edit shape is refused.
    let hosted = || sparksmith_core::Evaluators {
        footprint: None,
        board: super::super::board_shape::host_evaluator(),
    };
    let start = || Operation::Board(sparksmith_core::layout::outline::BoardCommand::StartSketch);
    let mut unshaped = Engine::default();
    assert!(
        unshaped
            .apply_hosted(unshaped.revision(), start(), hosted())
            .is_err()
    );
    editor.open_rules_in(Some(library), None);
    editor.ask_for_profile();
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    for text in ["Choose a fab profile", "JLCPCB 2-layer 1 oz", "Use profile"] {
        assert!(shown.iter().any(|t| t == text), "{text}: {shown:?}");
    }
    // The picker lists both built-ins.
    assert!(click(&mut editor, &engine, &ctx, "JLCPCB 2-layer 1 oz")?.is_none());
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(
        shown.iter().any(|t| t == "JLCPCB 4-layer 1 oz"),
        "{shown:?}"
    );
    // Clicking the choice again closes its list.
    assert!(click(&mut editor, &engine, &ctx, "JLCPCB 2-layer 1 oz")?.is_none());
    let use_it =
        click(&mut editor, &engine, &ctx, "Use profile")?.ok_or("Use profile did nothing")?;
    assert_eq!(marked(&ctx), Some(Origin::Dialog(choose_id())));
    engine.apply(engine.revision(), use_it)?;
    let builtin = rule_profiles::builtin(rule_profiles::DEFAULT_BUILTIN).ok_or("built-in")?;
    assert_eq!(engine.project().rules.profile.as_ref(), Some(&builtin));
    assert_eq!(builtin, fixtures::jlcpcb_example()?, "the shipped file");
    frame(&mut editor, &engine, &ctx, vec![]);
    assert!(editor.rules.is_none(), "the panel closes once it is set");
    assert_eq!(editor.subject(), Subject::Properties);
    // Asked once: a later frame does not ask again.
    frame(&mut editor, &engine, &ctx, vec![]);
    assert!(editor.rules.is_none());

    // The Route card shows the profile's via size, without an error.
    editor.on_tool("route", &engine);
    frame(&mut editor, &engine, &ctx, vec![]);
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(!shown.iter().any(|t| t == "Enter 0.1 to 6 mm"), "{shown:?}");
    for size in ["0.3 mm", "0.45 mm"] {
        assert!(shown.iter().any(|t| t == size), "{size}: {shown:?}");
    }
    // The Board card's Edit shape… starts the outline sketch.
    let mut editor = PcbEditor::default();
    let edit = click(&mut editor, &engine, &ctx, "Edit shape…")?.ok_or("Edit shape did nothing")?;
    engine.apply_hosted(engine.revision(), edit, hosted())?;
    assert!(matches!(
        engine.project().board_shape,
        sparksmith_core::BoardShape::Sketch(_)
    ));
    engine.undo()?;
    engine.undo()?;
    assert!(engine.project().rules.profile.is_none(), "one undo step");
    Ok(())
}

/// Cancel (Later) leaves the board without a profile and closes the
/// panel; a board that has a profile is never asked.
#[test]
fn later_closes_the_question_and_a_profiled_board_is_not_asked() -> TestResult {
    let engine = Engine::default();
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (_dir, library) = library()?;
    editor.open_rules_in(Some(library), None);
    editor.ask_for_profile();
    assert!(click(&mut editor, &engine, &ctx, "Cancel")?.is_none());
    assert!(editor.rules.is_none());
    assert!(engine.project().rules.profile.is_none());

    let profiled = Engine::demo()?;
    let mut editor = PcbEditor::default();
    editor.ask_for_profile();
    frame(&mut editor, &profiled, &ctx, vec![]);
    assert!(editor.rules.is_none(), "not asked");
    Ok(())
}
