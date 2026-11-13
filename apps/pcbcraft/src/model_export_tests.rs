//! Export to SolidCraft in headless egui frames: the tier choice, an
//! export running off the UI thread with its progress, Stop, and the
//! result's message.
use super::*;
use pcbcraft_core::Engine;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Harness {
    ctx: egui::Context,
    export: ModelExport,
    project: Arc<Project>,
    answers: Vec<Answer>,
    now: f64,
}

impl Harness {
    fn new() -> Fallible<Self> {
        let ctx = egui::Context::default();
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        let export = ModelExport::new(&ctx);
        let project = Arc::new(Engine::showcase()?.project().clone());
        Ok(Self {
            ctx,
            export,
            project,
            answers: Vec::new(),
            now: 0.0,
        })
    }

    /// One frame: the result, then the window. Returns what it painted.
    fn frame(&mut self, events: Vec<egui::Event>) -> Vec<(String, egui::Pos2)> {
        let Self {
            ctx,
            export,
            project,
            answers,
            now,
        } = self;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            time: Some(*now),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            export.poll(*now, project);
            answers.extend(export.show(ui.ctx()));
        });
        output.textures_delta.clear();
        let mut painted = Vec::new();
        for shape in &output.shapes {
            texts(&shape.shape, &mut painted);
        }
        painted
    }

    /// What a newly opened window paints (egui lays it out unseen first).
    fn shown(&mut self) -> Vec<(String, egui::Pos2)> {
        self.frame(vec![]);
        self.frame(vec![])
    }

    fn click(&mut self, label: &str) -> Fallible {
        let painted = self.shown();
        let at = painted
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, at)| *at)
            .ok_or_else(|| format!("no {label} among {painted:?}"))?;
        self.frame(vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        self.frame(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        Ok(())
    }
}

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

fn shows(painted: &[(String, egui::Pos2)], text: &str) -> bool {
    painted.iter().any(|(t, _)| t == text)
}

/// The window offers the two tiers, mechanical chosen first; picking the
/// full tier and Export… asks for the file with that tier.
#[test]
fn the_window_offers_the_tiers_and_asks_for_the_file() -> Fallible {
    let mut h = Harness::new()?;
    h.export.open();
    let painted = h.shown();
    assert!(shows(&painted, TITLE), "{painted:?}");
    assert!(shows(&painted, "Mechanical: laminate, pads and parts"));
    assert!(shows(&painted, "Full: with every track, via and fill"));
    assert_eq!(h.export.chosen, Tier::Mechanical);
    h.click("Full: with every track, via and fill")?;
    assert_eq!(h.export.chosen, Tier::Full);
    h.click(EXPORT)?;
    assert_eq!(h.answers, vec![Answer::Pick]);
    assert!(!h.export.showing(), "the save panel follows");
    Ok(())
}

/// A full export runs off the UI thread, showing its stage and Stop while
/// frames go on, and ends with the file written and every board body as
/// planned.
#[test]
fn an_export_runs_off_the_ui_thread_and_reports_the_check() -> Fallible {
    let mut h = Harness::new()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("board.solidcraft");
    h.export.chosen = Tier::Full;
    h.export.start(Arc::clone(&h.project), path.clone());
    assert!(h.export.busy());
    let painted = h.shown();
    assert!(shows(&painted, TITLE), "{painted:?}");
    assert!(shows(&painted, STOP), "{painted:?}");
    assert!(shows(&painted, "Full"), "{painted:?}");
    let project = Arc::clone(&h.project);
    h.export.wait(&project);
    let (level, message) = h.export.take_status().ok_or("a result")?;
    assert_eq!(level, Level::Ok, "{message}");
    assert!(message.starts_with("Exported "), "{message}");
    assert!(message.contains("(full, "), "{message}");
    assert!(message.contains("board bodies as planned"), "{message}");
    assert!(path.exists());
    assert!(!h.export.showing());
    Ok(())
}

/// Stop ends the export at its next command: nothing is written, and the
/// message says so.
#[test]
fn stop_ends_the_export_and_writes_nothing() -> Fallible {
    let mut h = Harness::new()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("board.solidcraft");
    h.export.chosen = Tier::Full;
    h.export.start(Arc::clone(&h.project), path.clone());
    h.click(STOP)?;
    let project = Arc::clone(&h.project);
    h.export.wait(&project);
    let (level, message) = h.export.take_status().ok_or("a result")?;
    assert_eq!(level, Level::Info, "{message}");
    assert_eq!(message, "Export to SolidCraft stopped; nothing was written");
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    Ok(())
}

/// A second export is refused while one runs.
#[test]
fn one_export_at_a_time() -> Fallible {
    let mut h = Harness::new()?;
    let dir = tempfile::tempdir()?;
    h.export
        .start(Arc::clone(&h.project), dir.path().join("a.solidcraft"));
    h.export.open();
    let (level, message) = h.export.take_status().ok_or("a refusal")?;
    assert_eq!(level, Level::Warning);
    assert!(message.contains("still running"), "{message}");
    h.export.stop();
    let project = Arc::clone(&h.project);
    h.export.wait(&project);
    Ok(())
}

/// A body that differs from the plan is named in the result.
#[test]
fn a_differing_body_is_named_in_the_message() -> Fallible {
    let h = Harness::new()?;
    let track = h.project.tracks.first().ok_or("a track")?;
    let difference = export::Difference {
        problem: export::Problem::BuiltNotPlanned,
        kind: pcbcraft_solidcraft::material::BodyKind::CopperTrack,
        source: export::Source::Track { track: track.id },
        layer: Some(pcbcraft_core::CopperLayer::Top),
        body: None,
        measure: pcbcraft_solidcraft::project::plan::Measure {
            min: [0.0; 3],
            max: [1.0; 3],
            volume: Some(1.0),
        },
        instead: Vec::new(),
        fit: None,
    };
    let report = export::report(
        Tier::Full,
        10,
        Some(export::Check {
            planned: 9,
            built: 10,
            differences: vec![difference],
            ..Default::default()
        }),
        Default::default(),
    );
    let (_, receiver) = mpsc::channel();
    let running = Running {
        watch: Watch::new(),
        receiver,
        path: PathBuf::from("/x/board.solidcraft"),
        project: Arc::clone(&h.project),
        started: 0.0,
    };
    let (level, message) = outcome(&running, Ok(report), false, false);
    assert_eq!(level, Level::Warning);
    assert_eq!(
        message,
        format!(
            "Exported /x/board.solidcraft, but 1 body differs from the board: \
             Track {} on the top layer: the model has a track copper the board does not",
            track.id
        )
    );
    Ok(())
}

/// The window offers to write a model whose bodies do not fit, off each
/// time it opens; ticked, the export is asked to write anyway.
#[test]
fn the_window_offers_to_write_anyway_off_by_default() -> Fallible {
    let mut h = Harness::new()?;
    h.export.open();
    let painted = h.shown();
    assert!(shows(&painted, ANYWAY), "{painted:?}");
    assert!(!h.export.write_anyway);
    h.click(ANYWAY)?;
    assert!(h.export.write_anyway);
    h.click(EXPORT)?;
    assert_eq!(h.answers, vec![Answer::Pick]);
    assert!(h.export.write_anyway, "kept for the export that follows");
    h.export.open();
    assert!(!h.export.write_anyway, "off again when the window opens");
    Ok(())
}

/// A refused export says nothing was written, names the body that does
/// not fit in the user's words, and how to write it anyway.
#[test]
fn a_refused_export_names_the_body_and_the_option() -> Fallible {
    let h = Harness::new()?;
    let track = h.project.tracks.first().ok_or("a track")?;
    let fit = pcbcraft_solidcraft::project::plan::Fit {
        area_mm2: 0.05,
        allowed_area_mm2: 1e-4,
        deviation_mm: 0.05,
        planned_holes: 0,
        built_holes: [0, 0],
        planned_z: [1.6, 1.635],
        built_z: [1.6, 1.635],
        volume_change: Some(0.1),
    };
    let difference = export::Difference {
        problem: export::Problem::PlannedNotBuilt,
        kind: pcbcraft_solidcraft::material::BodyKind::CopperTrack,
        source: export::Source::Track { track: track.id },
        layer: Some(pcbcraft_core::CopperLayer::Top),
        body: Some("track".into()),
        measure: pcbcraft_solidcraft::project::plan::Measure {
            min: [0.0; 3],
            max: [1.0; 3],
            volume: Some(1.0),
        },
        instead: Vec::new(),
        fit: Some(fit),
    };
    let report = export::report(
        Tier::Full,
        0,
        Some(export::Check {
            planned: 9,
            built: 9,
            differences: vec![difference],
            ..Default::default()
        }),
        Default::default(),
    );
    let (_, receiver) = mpsc::channel();
    let running = Running {
        watch: Watch::new(),
        receiver,
        path: PathBuf::from("/x/board.solidcraft"),
        project: Arc::clone(&h.project),
        started: 0.0,
    };
    let (level, message) = outcome(
        &running,
        Err(Failure::Unfit(Box::new(report))),
        false,
        false,
    );
    assert_eq!(level, Level::Error);
    assert_eq!(
        message,
        format!(
            "Nothing was written: 1 body of the model does not fit the board: \
             Track {} on the top layer: its track copper was built in another shape: \
             its outline is off by up to 0.0500 mm (0.0500 mm² differs), \
             its volume is +10.000 %. To write the model all the same, export again with \
             \u{201c}{ANYWAY}\u{201d} on",
            track.id
        )
    );
    Ok(())
}
