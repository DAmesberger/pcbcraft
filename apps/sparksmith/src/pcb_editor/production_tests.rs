//! Production files…: the dialog opens from its ribbon command, Export
//! stays disabled while an unwaived error blocks it, Waive… commits one
//! `fabrication.waive` with its reason, a whole group or a Shift-click run
//! is waived with one reason in one undo step from a form that opens
//! under the group, and the export writes the set off the UI thread.
use super::*;
use egui::{Pos2, Shape};
use sparksmith_core::Engine;
use sparksmith_core::issues::Severity;
use sparksmith_ui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn frame(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Operation>) {
    frame_at(editor, engine, ctx, events, 700.0)
}

/// One frame with the slot `width` wide.
fn frame_at(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    width: f32,
) -> (egui::FullOutput, Option<Operation>) {
    let mut command = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(width, 1400.0),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            command = editor.subject_slot(ui, engine).output;
        },
    );
    output.textures_delta.clear();
    (output, command)
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

/// The reason field's hint is painted whole, not cut short, in a slot as
/// wide as the app's Properties card.
fn hint_shows_whole(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    hint: &str,
) -> TestResult {
    let width = sparksmith_ui::theme::Tokens::get().form_card_width;
    frame_at(editor, engine, ctx, vec![], width);
    let (output, _) = frame_at(editor, engine, ctx, vec![], width);
    let output = &output;
    fn walk(shape: &Shape, hint: &str, out: &mut Vec<bool>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, hint, out)),
            Shape::Text(text) if text.galley.job.text == hint => out.push(text.galley.elided),
            _ => {}
        }
    }
    let mut elided = Vec::new();
    for shape in &output.shapes {
        walk(&shape.shape, hint, &mut elided);
    }
    match elided.as_slice() {
        [false] => Ok(()),
        [] => Err(format!("no {hint} hint in {:?}", texts(output)).into()),
        _ => Err(format!("{hint} hint cut short: {elided:?}").into()),
    }
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

fn click(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    label: &str,
) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
    let (output, _) = frame(editor, engine, ctx, vec![]);
    let point = *positions(&output, label)
        .last()
        .ok_or_else(|| format!("missing {label}"))?;
    Ok(frame(editor, engine, ctx, press(point)).1)
}

/// Rest the pointer on the painted `label` for a frame (hover actions
/// show on the next one).
fn hover(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    label: &str,
) -> Result<Pos2, Box<dyn std::error::Error>> {
    let (output, _) = frame(editor, engine, ctx, vec![]);
    let point = *positions(&output, label)
        .last()
        .ok_or_else(|| format!("missing {label}"))?;
    frame(editor, engine, ctx, vec![egui::Event::PointerMoved(point)]);
    Ok(point)
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    ctx
}

fn dialog(editor: &mut PcbEditor) -> Result<&mut ProductionDialog, Box<dyn std::error::Error>> {
    Ok(editor.production.as_mut().ok_or("the dialog is closed")?)
}

/// A report with one clearance error, its waiver read from the project.
fn report_with_error(engine: &Engine) -> PreflightReport {
    let key = "board.clearance via:1 via:2".to_owned();
    let waiver = engine.project().production.waiver(&key).cloned();
    let blocking = usize::from(waiver.is_none());
    PreflightReport {
        profile: "Standard 6/6".into(),
        items: vec![PreflightItem {
            key,
            code: "board.clearance".into(),
            severity: Severity::Error,
            title: "Via on GND too close to via on VCC".into(),
            fix: Some("Move one via".into()),
            objects: Vec::new(),
            waiver,
        }],
        errors: 1,
        warnings: 0,
        waived: 1 - blocking,
        blocking,
    }
}

fn open(engine: &Engine) -> Result<PcbEditor, Box<dyn std::error::Error>> {
    let mut editor = PcbEditor::default();
    assert!(editor.on_tool("production", engine).is_none());
    editor.open_production_in(None);
    assert_eq!(editor.subject(), Subject::Dialog);
    Ok(editor)
}

#[test]
fn the_ribbon_command_opens_the_dialog_in_the_subject_slot() -> TestResult {
    let engine = Engine::showcase()?;
    let mut editor = PcbEditor::default();
    let ribbon = editor.ribbon(&engine);
    assert!(
        ribbon
            .groups
            .iter()
            .flat_map(|g| g.tools.iter())
            .any(|t| t.id == "production")
    );
    editor.on_tool("production", &engine);
    assert_eq!(editor.subject(), Subject::Dialog);
    let ctx = context();
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    for label in ["Production files", "PRODUCTION CHECK", "OUTPUT", "Export"] {
        assert!(shown.iter().any(|t| t == label), "{label} in {shown:?}");
    }
    // Cancel closes it.
    click(&mut editor, &engine, &ctx, "Cancel")?;
    assert!(editor.production.is_none());
    Ok(())
}

/// Run 3, finding 16: without a folder the card says why Export waits; a
/// picked folder goes to the app once to keep for the project, and the
/// kept folder starts the next dialog, unless it has gone.
#[test]
fn the_card_says_export_needs_a_folder_and_the_folder_is_kept() -> TestResult {
    let engine = Engine::showcase()?;
    let ctx = context();
    let says = |output: &egui::FullOutput| {
        texts(output)
            .iter()
            .any(|t| t.starts_with("Choose a folder to save to"))
    };
    let mut editor = open(&engine)?;
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert!(says(&output), "{:?}", texts(&output));

    let tmp = tempfile::tempdir()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    sender.send(Some(tmp.path().to_path_buf()))?;
    dialog(&mut editor)?.picking = Some(receiver);
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert!(!says(&output), "a folder is chosen");
    assert_eq!(
        editor.take_chosen_production_folder().as_deref(),
        Some(tmp.path())
    );
    assert_eq!(editor.take_chosen_production_folder(), None, "once");

    // What the app hands back for this project starts the next dialog.
    let mut editor = PcbEditor {
        production_folder: Some(tmp.path().to_path_buf()),
        ..PcbEditor::default()
    };
    editor.open_production_in(None);
    assert_eq!(dialog(&mut editor)?.location.as_deref(), Some(tmp.path()));
    let mut editor = PcbEditor {
        production_folder: Some(tmp.path().join("gone")),
        ..PcbEditor::default()
    };
    editor.open_production_in(None);
    assert_eq!(dialog(&mut editor)?.location, None, "a missing folder asks");
    Ok(())
}

#[test]
fn an_unwaived_error_blocks_export_and_a_waiver_unblocks_it() -> TestResult {
    let mut engine = Engine::showcase()?;
    let mut editor = open(&engine)?;
    let ctx = context();
    let tmp = tempfile::tempdir()?;
    {
        let d = dialog(&mut editor)?;
        d.report = Some((
            engine.revision(),
            Pick::Board,
            Ok(report_with_error(&engine)),
        ));
        d.location = Some(tmp.path().to_path_buf());
    }
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(shown.iter().any(|t| t == "1 error"), "{shown:?}");
    {
        let d = dialog(&mut editor)?;
        let report = d.current(&engine).cloned();
        assert_eq!(
            export_blocked(d, report.as_ref(), d.output("x").as_ref()).as_deref(),
            Some("Fix or waive the error first")
        );
    }
    // Export is disabled: clicking it starts nothing.
    click(&mut editor, &engine, &ctx, "Export")?;
    assert!(!dialog(&mut editor)?.busy());
    // Waive… shows while the row is hovered; it asks for a reason, and
    // Waive commits it as one edit.
    hover(
        &mut editor,
        &engine,
        &ctx,
        "Via on GND too close to via on VCC",
    )?;
    assert!(click(&mut editor, &engine, &ctx, "Waive…")?.is_none());
    "Vias are 0.18 mm apart by design".clone_into(&mut dialog(&mut editor)?.waiving.reason);
    let op = click(&mut editor, &engine, &ctx, "Waive 1 finding")?.ok_or("no waiver edit")?;
    assert!(matches!(
        &op,
        Operation::Fabrication(FabricationCommand::Waive { keys, reason })
            if keys == &["board.clearance via:1 via:2"] && reason == "Vias are 0.18 mm apart by design"
    ));
    let before = engine.revision();
    engine.apply(before, op)?;
    assert_eq!(engine.project().production.waivers.len(), 1);
    // The report of the new revision has the item waived: Export enables.
    {
        let d = dialog(&mut editor)?;
        d.report = Some((
            engine.revision(),
            Pick::Board,
            Ok(report_with_error(&engine)),
        ));
        let report = d.current(&engine).cloned();
        assert_eq!(
            export_blocked(d, report.as_ref(), d.output("x").as_ref()),
            None
        );
    }
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    assert!(shown.iter().any(|t| t == "1 waived"), "{shown:?}");
    // Restore removes the waiver again.
    hover(
        &mut editor,
        &engine,
        &ctx,
        "Via on GND too close to via on VCC",
    )?;
    let op = click(&mut editor, &engine, &ctx, "Restore")?.ok_or("no restore edit")?;
    assert!(matches!(
        op,
        Operation::Fabrication(FabricationCommand::Unwaive { .. })
    ));
    engine.apply(engine.revision(), op)?;
    assert!(engine.project().production.waivers.is_empty());
    engine.undo()?;
    assert_eq!(engine.project().production.waivers.len(), 1);
    Ok(())
}

/// By default a reason is optional: Waive commits with none, and the
/// waived row's tip reads "Waived" without an empty reason.
#[test]
fn a_waiver_without_a_reason_is_committed_by_default() -> TestResult {
    let mut engine = Engine::showcase()?;
    let mut editor = open(&engine)?;
    let ctx = context();
    dialog(&mut editor)?.report = Some((
        engine.revision(),
        Pick::Board,
        Ok(report_with_error(&engine)),
    ));
    hover(
        &mut editor,
        &engine,
        &ctx,
        "Via on GND too close to via on VCC",
    )?;
    assert!(click(&mut editor, &engine, &ctx, "Waive…")?.is_none());
    hint_shows_whole(&mut editor, &engine, &ctx, "Optional")?;
    let op = click(&mut editor, &engine, &ctx, "Waive 1 finding")?.ok_or("no waiver edit")?;
    assert!(matches!(
        &op,
        Operation::Fabrication(FabricationCommand::Waive { reason, .. }) if reason.is_empty()
    ));
    engine.apply(engine.revision(), op)?;
    let report = report_with_error(&engine);
    let item = report.items.first().ok_or("item")?;
    assert_eq!(item.waiver.as_ref().map(|w| w.reason.as_str()), Some(""));
    assert_eq!(row_tip(item), "Waived\nMove one via");
    Ok(())
}

#[test]
fn export_writes_the_set_off_the_ui_thread() -> TestResult {
    let engine = Engine::showcase()?;
    let mut editor = open(&engine)?;
    let ctx = context();
    let tmp = tempfile::tempdir()?;
    dialog(&mut editor)?.location = Some(tmp.path().to_path_buf());
    dialog(&mut editor)?.wait_check(&ctx, &engine);
    let ready = dialog(&mut editor)?
        .current(&engine)
        .and_then(|r| r.as_ref().ok())
        .is_some_and(PreflightReport::ready);
    if !ready {
        // A design the core refuses cannot be written; the dialog says why.
        return Ok(());
    }
    click(&mut editor, &engine, &ctx, "Export")?;
    assert!(dialog(&mut editor)?.busy(), "the export did not start");
    dialog(&mut editor)?.wait(&ctx);
    let written = dialog(&mut editor)?
        .written
        .clone()
        .ok_or("nothing was written")?;
    let folder = tmp.path().join(format!(
        "{}-production",
        fabrication::base_name(engine.project(), &FabricationOptions::default())?
    ));
    assert_eq!(PathBuf::from(&written.output), folder);
    assert!(folder.is_dir());
    for file in &written.files {
        assert!(folder.join(&file.name).is_file(), "{}", file.name);
    }
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert!(texts(&output).iter().any(|t| t == "WRITTEN"));
    // A second export into the same, now non-empty, folder is refused.
    click(&mut editor, &engine, &ctx, "Export")?;
    dialog(&mut editor)?.wait(&ctx);
    assert!(dialog(&mut editor)?.refusal.is_some());
    Ok(())
}

#[test]
fn sizes_and_summaries_read_as_words() {
    assert_eq!(size_text(812), "812 B");
    assert_eq!(size_text(2048), "2.0 KB");
    assert_eq!(size_text(3 * 1024 * 1024), "3.0 MB");
    assert_eq!(report_summary(&PreflightReport::default()), "No issues");
}

/// A report like the one-hour run's: a clearance error, nine unrouted
/// nets and twelve parts without a Part, waivers read from the project.
fn report_like_the_run(engine: &Engine) -> PreflightReport {
    let item = |key: String, code: &str, severity, title: String, fix: &str| {
        let waiver = engine.project().production.waiver(&key).cloned();
        PreflightItem {
            key,
            code: code.into(),
            severity,
            title,
            fix: Some(fix.into()),
            objects: Vec::new(),
            waiver,
        }
    };
    let mut items = vec![item(
        "board.clearance via:1 via:2".into(),
        "board.clearance",
        Severity::Error,
        "Via on GND too close to via on VCC".into(),
        "Move one via",
    )];
    for net in [
        "VBUS", "USB_DP", "USB_DM", "GND", "3V3", "SWDIO", "SWCLK", "SDA", "SCL",
    ] {
        items.push(item(
            format!("board.unrouted net:{net}"),
            "board.unrouted",
            Severity::Error,
            format!("{net}: 2 pads unrouted"),
            "Route the connection",
        ));
    }
    for n in 1..=12 {
        items.push(item(
            format!("purchasing.no_part instance:{n}"),
            "purchasing.no_part",
            Severity::Warning,
            format!("R{n}: no Part"),
            "Choose a Part with Resolve parts",
        ));
    }
    items.push(item(
        "fabrication.legend_outside legend:R3".into(),
        "fabrication.legend_outside",
        Severity::Warning,
        "Legend of R3 runs into the board edge cutout near the mounting hole".into(),
        "Move the legend",
    ));
    let blocking = items.iter().filter(|i| i.blocks()).count();
    let waived = items.iter().filter(|i| i.waiver.is_some()).count();
    PreflightReport {
        profile: "Standard 6/6".into(),
        errors: 10,
        warnings: 13,
        waived,
        blocking,
        items,
    }
}

/// The topmost painted `label`'s centre.
fn at(output: &egui::FullOutput, label: &str) -> Result<Pos2, Box<dyn std::error::Error>> {
    Ok(*positions(output, label)
        .first()
        .ok_or_else(|| format!("missing {label} in {:?}", texts(output)))?)
}

#[test]
fn a_whole_group_is_waived_with_one_reason_in_one_undo_step() -> TestResult {
    let mut engine = Engine::showcase()?;
    // Settings: "Require a reason to waive errors".
    engine.require_waiver_reason(true);
    let mut editor = open(&engine)?;
    let ctx = context();
    dialog(&mut editor)?.report = Some((
        engine.revision(),
        Pick::Board,
        Ok(report_like_the_run(&engine)),
    ));
    // The findings read as groups: the nine unrouted nets are one row.
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let shown = texts(&output);
    for label in [
        "Unrouted connections",
        "No Part chosen",
        "Via on GND too close to via on VCC",
    ] {
        assert!(shown.iter().any(|t| t == label), "{label} in {shown:?}");
    }
    assert!(
        !shown.iter().any(|t| t == "VBUS: 2 pads unrouted"),
        "folded"
    );
    // Waive all… on the group opens the reason form right under it.
    let header = hover(&mut editor, &engine, &ctx, "Unrouted connections")?;
    assert!(click(&mut editor, &engine, &ctx, "Waive all…")?.is_none());
    assert_eq!(dialog(&mut editor)?.waiving.ticked.len(), 9);
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let reason = at(&output, "Reason")?;
    let next = at(&output, "No Part chosen")?;
    assert!(
        header.y < reason.y && reason.y < next.y,
        "the form is under its group, before the next: {header:?} {reason:?} {next:?}"
    );
    hint_shows_whole(&mut editor, &engine, &ctx, "Required")?;
    // The reason is required by the setting: without one, Waive does
    // nothing, and the engine refuses such a waiver unchanged.
    assert!(click(&mut editor, &engine, &ctx, "Waive 9 findings")?.is_none());
    let revision = engine.revision();
    let keys: Vec<String> = dialog(&mut editor)?
        .waiving
        .ticked
        .iter()
        .cloned()
        .collect();
    let unreasoned = Operation::Fabrication(FabricationCommand::Waive {
        keys,
        reason: String::new(),
    });
    assert!(engine.apply(revision, unreasoned).is_err());
    assert_eq!(engine.revision(), revision);
    assert!(engine.project().production.waivers.is_empty());
    "prototype".clone_into(&mut dialog(&mut editor)?.waiving.reason);
    let op = click(&mut editor, &engine, &ctx, "Waive 9 findings")?.ok_or("no waiver edit")?;
    let Operation::Fabrication(FabricationCommand::Waive { keys, reason }) = &op else {
        return Err(format!("not a waiver: {op:?}").into());
    };
    assert_eq!(keys.len(), 9);
    assert!(keys.iter().all(|k| k.starts_with("board.unrouted ")));
    assert_eq!(reason, "prototype");
    assert!(dialog(&mut editor)?.waiving.at.is_none(), "the form closes");
    engine.apply(engine.revision(), op)?;
    assert_eq!(engine.project().production.waivers.len(), 9);
    engine.undo()?;
    assert!(
        engine.project().production.waivers.is_empty(),
        "one undo step"
    );
    Ok(())
}

#[test]
fn a_shift_click_ticks_a_run_and_the_form_follows_the_row() -> TestResult {
    let engine = Engine::showcase()?;
    let report = report_like_the_run(&engine);
    let groups = groups(&report);
    let mut w = Waiving::default();
    tick_row(&mut w, &groups, "purchasing.no_part instance:2", false);
    assert_eq!(w.at, Some(At::Row("purchasing.no_part instance:2".into())));
    tick_row(&mut w, &groups, "purchasing.no_part instance:6", true);
    assert_eq!(w.ticked.len(), 5, "R2 to R6: {:?}", w.ticked);
    assert_eq!(w.at, Some(At::Row("purchasing.no_part instance:6".into())));
    // A plain click unticks one; the last untick closes the form.
    tick_row(&mut w, &groups, "purchasing.no_part instance:6", false);
    assert_eq!(w.ticked.len(), 4);
    for n in 2..=5 {
        tick_row(
            &mut w,
            &groups,
            &format!("purchasing.no_part instance:{n}"),
            false,
        );
    }
    assert!(w.ticked.is_empty());
    assert!(w.at.is_none());
    // A row of a folded group keeps its form under the group's header.
    w.ticked.insert("purchasing.no_part instance:12".into());
    w.at = Some(At::Row("purchasing.no_part instance:12".into()));
    let lines = entries(&w, &groups);
    let form = lines.iter().position(|e| matches!(e, Entry::Form));
    let header = lines
        .iter()
        .position(|e| matches!(e, Entry::Header(g) if groups[*g].0 == "purchasing.no_part"));
    assert_eq!(form, header.map(|h| h + 1));
    Ok(())
}

/// The dialog with the run's findings and the unrouted group unfolded.
fn run_dialog(engine: &Engine) -> Result<PcbEditor, Box<dyn std::error::Error>> {
    let mut editor = open(engine)?;
    let d = dialog(&mut editor)?;
    d.report = Some((
        engine.revision(),
        Pick::Board,
        Ok(report_like_the_run(engine)),
    ));
    d.waiving.open.insert("board.unrouted".into());
    Ok(editor)
}

/// The dialog in a 320-point column, as in the one-hour run: every text
/// cut short shows in full on hover. With `SPARKSMITH_TEXT_PICTURES=<dir>`
/// it writes the grouped list, a hovered row and the group's reason form.
#[test]
fn production_check_text_cut_short_shows_in_full_on_hover() -> TestResult {
    use crate::kit::text::tests::{Screen, cuts_without_hover, tooltips_at_once};
    let engine = Engine::showcase()?;
    let mut editor = run_dialog(&engine)?;
    let screen = Screen::new(egui::vec2(320.0, 1100.0));
    let run = |editor: &mut PcbEditor, events: Vec<egui::Event>| {
        screen.run(events, |ui| {
            editor.subject_slot(ui, &engine);
        })
    };
    run(&mut editor, vec![]);
    let rest = run(&mut editor, vec![]);
    screen.picture("production-grouped", &rest, 2.0)?;
    let (cut, missing) = cuts_without_hover(&screen.ctx, |events| run(&mut editor, events));
    assert!(cut >= 1, "the narrow column cuts a row: {cut}");
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");
    // A hovered row: its action shows, and its title's tooltip.
    tooltips_at_once(&screen.ctx);
    let out = run(&mut editor, vec![]);
    let row = *positions(&out, "VBUS: 2 pads unrouted")
        .first()
        .ok_or("no VBUS row")?;
    run(&mut editor, vec![egui::Event::PointerMoved(row)]);
    let hovered = run(&mut editor, vec![]);
    screen.picture("production-row-hover", &hovered, 2.0)?;
    // Waive all… on the No Part group: the form under its header.
    let header = *positions(&hovered, "No Part chosen")
        .first()
        .ok_or("no No Part group")?;
    run(&mut editor, vec![egui::Event::PointerMoved(header)]);
    let out = run(&mut editor, vec![]);
    let waive = *positions(&out, "Waive all…")
        .last()
        .ok_or("no Waive all…")?;
    run(&mut editor, press(waive));
    "prototype".clone_into(&mut dialog(&mut editor)?.waiving.reason);
    run(&mut editor, vec![egui::Event::PointerGone]);
    let form = run(&mut editor, vec![]);
    assert!(texts(&form).iter().any(|t| t == "Waive 12 findings"));
    screen.picture("production-waive-group-form", &form, 2.0)?;
    Ok(())
}

/// In the run's narrow column (240 points less the card inset), a group's
/// title is never cut at rest: it wraps to a second line, its count after
/// it. (Hovered, Waive all… may cut it; its hover then shows it in full.)
#[test]
fn group_titles_fit_a_narrow_column() -> TestResult {
    use crate::kit::text::tests::{Screen, cuts};
    let engine = Engine::showcase()?;
    let mut editor = run_dialog(&engine)?;
    let screen = Screen::new(egui::vec2(224.0, 1100.0));
    let run = |editor: &mut PcbEditor, events: Vec<egui::Event>| {
        screen.run(events, |ui| {
            editor.subject_slot(ui, &engine);
        })
    };
    run(&mut editor, vec![egui::Event::PointerGone]);
    let rest = run(&mut editor, vec![]);
    screen.picture("production-narrow-groups", &rest, 2.0)?;
    let report = report_like_the_run(&engine);
    let titles: Vec<String> = groups(&report)
        .iter()
        .filter(|(_, items)| items.len() > 1)
        .map(|(code, _)| sparksmith_core::issues::group_title(code))
        .collect();
    assert!(titles.len() >= 2, "{titles:?}");
    let cut: Vec<String> = cuts(&rest).into_iter().map(|c| c.text).collect();
    let shown = texts(&rest);
    for title in &titles {
        assert!(!cut.contains(title), "{title} is cut: {cut:?}");
        assert!(shown.contains(title), "{title} is not shown: {shown:?}");
    }
    Ok(())
}
