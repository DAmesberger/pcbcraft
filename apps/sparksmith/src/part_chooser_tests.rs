//! The part-file chooser through real egui frames: its sources, the check
//! report it shows (findings with their severity), one commit per left
//! field, and the recent files it hands back to keep.
use super::*;
use sparksmith_solidcraft::models::part_file::{PartBody, PartFinding};
use sparksmith_ui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Frames {
    ctx: egui::Context,
    time: f64,
}

impl Frames {
    fn new() -> Self {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        Self { ctx, time: 0.0 }
    }

    /// One 1200 × 900 frame: the texts drawn and their centres.
    fn frame(
        &mut self,
        events: Vec<egui::Event>,
        mut add: impl FnMut(&mut Ui),
    ) -> Vec<(String, egui::Pos2)> {
        self.time += 0.1;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(1200.0, 900.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| add(ui),
        );
        output.textures_delta.clear();
        fn texts(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(text) => out.push((
                    text.galley.job.text.clone(),
                    text.pos + text.galley.size() * 0.5,
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        out
    }
}

fn has(texts: &[(String, egui::Pos2)], text: &str) -> bool {
    texts.iter().any(|(t, _)| t == text)
}

#[test]
fn the_chooser_lists_the_builtins_and_the_projects_part_files() -> TestResult {
    let engine = Engine::default();
    let shipped = part_files::builtin_parts()?.all();
    assert!(!shipped.is_empty());
    let groups = source_rows(&engine, &["/parts/nut.solidcraft".into()]);
    let names: Vec<&str> = groups.iter().map(|(g, _)| *g).collect();
    assert_eq!(names, ["Built-in", "This project", "Recent files"]);
    assert_eq!(groups[0].1.len(), shipped.len());
    for name in shipped.iter().map(|p| p.name.as_str()) {
        assert!(
            groups[0]
                .1
                .iter()
                .any(|r| r.source == PartSource::Builtin(name.into()) && r.label == name)
        );
    }
    assert!(
        groups[1].1.is_empty(),
        "an empty project holds no part file"
    );
    assert_eq!(groups[2].1[0].label, "nut.solidcraft");
    // Drawn: every built-in by name, under its group.
    let mut frames = Frames::new();
    let mut chooser = PartChooser::default();
    chooser.open(ChooserMode::Use, None, PartValues::new());
    // An area lays itself out on its first frame and paints on the next.
    frames.frame(vec![], |ui| {
        chooser.show(ui.ctx(), &engine);
    });
    let texts = frames.frame(vec![], |ui| {
        chooser.show(ui.ctx(), &engine);
    });
    assert!(
        has(&texts, &format!("BUILT-IN  {}", shipped.len())) && has(&texts, "THIS PROJECT  0"),
        "{texts:?}"
    );
    for name in shipped.iter().map(|p| p.name.as_str()) {
        assert!(has(&texts, name), "{name} listed: {texts:?}");
    }
    assert!(has(&texts, "Open file…"));
    assert!(has(&texts, "Use"));
    Ok(())
}

#[test]
fn the_place_mode_names_its_primary() -> TestResult {
    let engine = Engine::default();
    let shipped = part_files::builtin_parts()?.all();
    let mut frames = Frames::new();
    let mut chooser = PartChooser::default();
    chooser.open(ChooserMode::Place, None, PartValues::new());
    // An area lays itself out on its first frame and paints on the next.
    frames.frame(vec![], |ui| {
        chooser.show(ui.ctx(), &engine);
    });
    let texts = frames.frame(vec![], |ui| {
        chooser.show(ui.ctx(), &engine);
    });
    assert!(
        has(&texts, "Place") && has(&texts, &format!("PLACE 3D PART  {}", shipped.len())),
        "{texts:?}"
    );
    Ok(())
}

#[test]
fn a_check_result_renders_its_findings_bodies_and_timing() {
    let report = PartReport {
        key: String::new(),
        parameters: Vec::new(),
        bodies: vec![
            PartBody {
                name: "Can".into(),
                appearance: Some("nickel-silver".into()),
                material: Some(sparksmith_core::materials::MaterialId::NickelSilver),
                bounds_mm: [[0.0; 3], [1.0; 3]],
            },
            PartBody {
                name: "Odd".into(),
                appearance: Some("chartreuse".into()),
                material: None,
                bounds_mm: [[0.0; 3], [1.0; 3]],
            },
        ],
        surfaces: Vec::new(),
        datums: Vec::new(),
        pin1: None,
        points: Vec::new(),
        bounds_mm: [[-1.0, -2.0, 0.0], [1.0, 2.0, 3.0]],
        findings: vec![
            PartFinding {
                code: "part_file.material".into(),
                blocking: true,
                message: "Body Odd's appearance names no material".into(),
            },
            PartFinding {
                code: "part_file.no_marking".into(),
                blocking: false,
                message: "No marking surface".into(),
            },
        ],
        evaluate_ms: 45.4,
        ok: false,
    };
    assert_eq!(materials_line(&report), "nickel-silver ×1, 1 unmapped");
    let mut frames = Frames::new();
    let texts = frames.frame(vec![], |ui| report_rows(ui, &report));
    assert!(has(&texts, "Body Odd's appearance names no material"));
    assert!(has(&texts, "No marking surface"));
    assert!(has(&texts, "nickel-silver ×1, 1 unmapped"));
    assert!(has(&texts, "2 × 4 × 3 mm"));
    assert!(has(&texts, "45 ms"));
}

#[test]
fn a_builtin_is_checked_off_the_ui_thread_and_its_parameters_shown() -> TestResult {
    let engine = Engine::default();
    let mut frames = Frames::new();
    let mut chooser = PartChooser::default();
    chooser.open(ChooserMode::Use, None, PartValues::new());
    let mut pick = None;
    let first = frames.frame(vec![], |ui| pick = chooser.show(ui.ctx(), &engine));
    assert!(pick.is_none());
    assert!(
        !first.iter().any(|(t, _)| t.starts_with("PARAMETERS")) || chooser.evaluation().is_some()
    );
    chooser.wait();
    let texts = frames.frame(vec![], |ui| pick = chooser.show(ui.ctx(), &engine));
    let evaluation = chooser.evaluation().ok_or("evaluated")?;
    let checked = evaluation.result.as_ref().map_err(|e| e.clone())?;
    assert!(checked.report.ok);
    assert!(
        texts.iter().any(|(t, _)| t.starts_with("PARAMETERS")),
        "{texts:?}"
    );
    assert!(has(&texts, "can_height"));
    assert!(has(&texts, "esp32-s31-wroom-3.solidcraft"));
    assert!(has(&texts, "MIT OR Apache-2.0"));
    // Use returns exactly what was checked.
    let at = texts
        .iter()
        .find(|(t, _)| t == "Use")
        .map(|(_, p)| *p)
        .ok_or("no Use")?;
    let click = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frames.frame(vec![egui::Event::PointerMoved(at)], |ui| {
        pick = chooser.show(ui.ctx(), &engine);
    });
    frames.frame(vec![click(true)], |ui| {
        pick = chooser.show(ui.ctx(), &engine)
    });
    frames.frame(vec![click(false)], |ui| {
        pick = chooser.show(ui.ctx(), &engine)
    });
    let pick = pick.ok_or("Use picked")?;
    assert_eq!(pick.source, PartSource::Builtin("esp32-s31-wroom-3".into()));
    assert!(pick.values.is_empty());
    assert!(pick.file.builtin.is_some());
    assert_eq!(pick.licence, None);
    assert!(chooser.is_open(), "the caller closes it once applied");
    Ok(())
}

#[test]
fn a_left_field_commits_once() -> TestResult {
    let parameters = vec![
        PartParameter {
            name: "height".into(),
            expression: "3 mm".into(),
            unit: "mm".into(),
            value: Some(3.0),
            given: false,
        },
        PartParameter {
            name: "fins".into(),
            expression: "6".into(),
            unit: String::new(),
            value: Some(6.0),
            given: false,
        },
    ];
    let mut editing = PartValues::new();
    let mut frames = Frames::new();
    let mut out: Vec<PartValues> = Vec::new();
    // The caller keeps what was committed, as the chooser and the card do.
    let run = |frames: &mut Frames, events, editing: &mut PartValues, out: &mut Vec<_>| {
        let committed = out.last().cloned().unwrap_or_default();
        frames.frame(events, |ui| {
            out.extend(values_form(ui, "t", &parameters, editing, &committed));
        })
    };
    let texts = run(&mut frames, vec![], &mut editing, &mut out);
    assert!(has(&texts, "3 mm") && has(&texts, "6"), "{texts:?}");
    let at = texts
        .iter()
        .find(|(t, _)| t == "3 mm")
        .map(|(_, p)| *p)
        .ok_or("no field")?;
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    run(
        &mut frames,
        vec![egui::Event::PointerMoved(at)],
        &mut editing,
        &mut out,
    );
    run(&mut frames, vec![button(true)], &mut editing, &mut out);
    run(&mut frames, vec![button(false)], &mut editing, &mut out);
    run(
        &mut frames,
        vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
            egui::Event::Text("4.5".into()),
        ],
        &mut editing,
        &mut out,
    );
    assert!(out.is_empty(), "typing does not commit");
    assert_eq!(editing.get("height"), Some(&PartValue::Nm(4_500_000)));
    run(
        &mut frames,
        vec![key(egui::Key::Enter)],
        &mut editing,
        &mut out,
    );
    for _ in 0..3 {
        run(&mut frames, vec![], &mut editing, &mut out);
    }
    assert_eq!(out.len(), 1, "one commit: {out:?}");
    assert_eq!(out[0].get("height"), Some(&PartValue::Nm(4_500_000)));
    assert_eq!(
        out[0].get("fins"),
        None,
        "untouched parameters stay the file's"
    );
    Ok(())
}

#[test]
fn counts_and_numbers_are_typed_by_their_value() {
    let p = |unit: &str, value| PartParameter {
        name: "x".into(),
        expression: String::new(),
        unit: unit.into(),
        value: Some(value),
        given: false,
    };
    assert!(matches!(value_kind(&p("mm", 3.0), None), PartValue::Nm(_)));
    assert!(matches!(
        value_kind(&p("deg", 3.0), None),
        PartValue::Mdeg(_)
    ));
    assert!(matches!(value_kind(&p("", 6.0), None), PartValue::Count(_)));
    assert!(matches!(value_kind(&p("", 0.5), None), PartValue::Milli(_)));
    assert!(matches!(
        value_kind(&p("", 6.0), Some(PartValue::Milli(6000))),
        PartValue::Milli(_)
    ));
    assert_eq!(typed(PartValue::Count(0), 2.5), None);
    assert_eq!(typed(PartValue::Count(0), 7.0), Some(PartValue::Count(7)));
}

#[test]
fn recent_files_are_bounded_and_handed_back_once() {
    let mut chooser = PartChooser::default();
    chooser.set_recent(&["/a.solidcraft".into()]);
    assert_eq!(chooser.take_recent_changed(), None);
    for i in 0..12 {
        chooser.remember(format!("/p{i}.solidcraft").into());
    }
    chooser.remember("/p3.solidcraft".into());
    let kept = chooser.take_recent_changed().unwrap_or_default();
    assert_eq!(kept.len(), MAX_RECENT);
    assert_eq!(kept[0], PathBuf::from("/p3.solidcraft"));
    assert_eq!(chooser.take_recent_changed(), None);
}

#[test]
fn messages_drop_the_check_code() {
    assert_eq!(
        shown_message("part_file.licence: a part file needs its licence stated"),
        "a part file needs its licence stated"
    );
    assert_eq!(shown_message("Plain text: kept"), "Plain text: kept");
}

#[test]
fn enter_in_the_sheet_commits_the_field_and_checks_again() -> TestResult {
    let engine = Engine::default();
    let mut frames = Frames::new();
    let mut chooser = PartChooser::default();
    chooser.open(ChooserMode::Use, None, PartValues::new());
    frames.frame(vec![], |ui| {
        chooser.show(ui.ctx(), &engine);
    });
    chooser.wait();
    let texts = frames.frame(vec![], |ui| {
        chooser.show(ui.ctx(), &engine);
    });
    let at = texts
        .iter()
        .find(|(t, _)| t == "2.465 mm")
        .map(|(_, p)| *p)
        .ok_or("no can_height field")?;
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(true)],
        vec![button(false)],
        vec![
            key(egui::Key::A, egui::Modifiers::COMMAND),
            egui::Event::Text("3".into()),
        ],
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        vec![],
    ] {
        frames.frame(events, |ui| {
            chooser.show(ui.ctx(), &engine);
        });
    }
    chooser.wait();
    let evaluation = chooser.evaluation().ok_or("evaluated")?;
    assert_eq!(
        evaluation.values.get("can_height"),
        Some(&PartValue::Nm(3_000_000)),
        "Enter committed the field and the check ran at it"
    );
    Ok(())
}
