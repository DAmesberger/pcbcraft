//! Production files…: the dialog opens from its ribbon command, Export
//! stays disabled while an unwaived error blocks it, Waive… commits one
//! `fabrication.waive` with its reason, and the export writes the set off
//! the UI thread.
use super::*;
use egui::{Pos2, Shape};
use pcbcraft_core::Engine;
use pcbcraft_core::issues::Severity;
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
    // Waive… asks for a reason; Waive commits it as one edit.
    assert!(click(&mut editor, &engine, &ctx, "Waive…")?.is_none());
    if let Some((_, reason)) = &mut dialog(&mut editor)?.waiving {
        "Vias are 0.18 mm apart by design".clone_into(reason);
    }
    let op = click(&mut editor, &engine, &ctx, "Waive")?.ok_or("no waiver edit")?;
    assert!(matches!(
        &op,
        Operation::Fabrication(FabricationCommand::Waive { key, reason })
            if key == "board.clearance via:1 via:2" && reason == "Vias are 0.18 mm apart by design"
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
