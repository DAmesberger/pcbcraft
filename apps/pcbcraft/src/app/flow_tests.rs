//! The whole app's frame, headless: one ⌘S over a focused card field writes
//! the typed value (e2e batch F1), by key and through the menu bar, and
//! ⇧⌘U runs on its first press after a workspace change (F6).
use super::*;
use crate::commands::Registry;
use pcbcraft_core::model::LabelKind;
use pcbcraft_core::schematic::SchematicCommand;
use solidcraft_ui_egui::fonts;

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Harness {
    app: App,
    ctx: egui::Context,
    dir: tempfile::TempDir,
    frame: eframe::Frame,
    label: pcbcraft_core::Id,
}

impl Harness {
    /// The demo with one label, whose card field commits on focus loss, on
    /// the Schematic workspace, with a project file.
    fn new() -> Fallible<Self> {
        let dir = tempfile::tempdir()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut engine = Engine::demo()?;
        engine.apply(
            engine.revision(),
            Operation::Schematic(SchematicCommand::PlaceLabel {
                sheet: None,
                name: "FIRST".into(),
                kind: LabelKind::Local,
                position: [100.0, -100.0],
                rotation: Default::default(),
                attach: None,
            }),
        )?;
        let label = engine
            .project()
            .labels
            .last()
            .map(|l| l.id)
            .ok_or("label")?;
        let mut files = FileUi::with_directory(
            &ctx,
            rfd::AsyncFileDialog::new(),
            &engine,
            dir.path().to_path_buf(),
        );
        files.adopt(&engine, Some(dir.path().join("board.pcbcraft")), false)?;
        let mut app = App::with_parts(
            &ctx,
            viewport::Viewport::detached(),
            Parts {
                engine,
                files,
                sources: LibrarySources::new(None),
                registry: Arc::new(Registry::standard()),
                keymap_problems: Vec::new(),
                messages: Messages::default(),
                browser_width: chrome::BROWSER_WIDTH,
                #[cfg(target_os = "macos")]
                native_menu: None,
            },
        );
        app.workspace = Workspace::Schematic;
        crate::keys::publish(&ctx, &app.registry);
        Ok(Self {
            app,
            ctx,
            dir,
            frame: eframe::Frame::_new_kittest(),
            label,
        })
    }

    fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("board.pcbcraft")
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1380.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        let Self {
            app, ctx, frame, ..
        } = self;
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame))
            .textures_delta
            .clear();
    }

    /// Select the label, give its name field the keyboard and type over it.
    fn type_in_the_field(&mut self, text: &str) -> Fallible {
        self.app
            .schematic
            .edit_label(self.app.engine.project(), self.label);
        self.frame(vec![]);
        self.frame(vec![]);
        assert!(self.ctx.text_edit_focused(), "the field has no keyboard");
        self.frame(vec![
            press(egui::Key::A, egui::Modifiers::COMMAND),
            egui::Event::Text(text.into()),
        ]);
        assert!(self.ctx.text_edit_focused());
        Ok(())
    }

    fn saved_name(&self) -> Fallible<String> {
        let saved = pcbcraft_io::read_project(&self.path())?;
        let label = saved.project.label(self.label).ok_or("label")?;
        Ok(label.name.clone())
    }

    /// Frames until the file workflow is idle.
    fn until_saved(&mut self) -> Fallible {
        for _ in 0..500 {
            self.frame(vec![]);
            if !self.app.files.busy() && self.path().exists() {
                self.frame(vec![]);
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err("the save did not finish".into())
    }
}

fn press(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// The messages said so far.
fn said(app: &App) -> String {
    app.messages
        .entries()
        .map(|m| m.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_saved(h: &Harness) -> Fallible {
    assert_eq!(h.saved_name()?, "TYPED", "the file holds the old value");
    assert!(!h.app.files.dirty(), "the unsaved dot stayed");
    let said = said(&h.app);
    assert!(!said.contains("remain unsaved"), "{said}");
    Ok(())
}

/// F1: one ⌘S by key over a focused field.
#[test]
fn one_command_s_by_key_saves_the_typed_value() -> Fallible {
    let mut h = Harness::new()?;
    h.type_in_the_field("TYPED")?;
    h.frame(vec![press(egui::Key::S, egui::Modifiers::COMMAND)]);
    h.until_saved()?;
    assert_saved(&h)
}

/// F1: one ⌘S through the menu bar over a focused field, as
/// `raw_input_hook` queues it: the system menu takes the key.
#[cfg(target_os = "macos")]
#[test]
fn one_command_s_through_the_menu_saves_the_typed_value() -> Fallible {
    let mut h = Harness::new()?;
    h.type_in_the_field("TYPED")?;
    h.app.menu_runs.push("file.save".into());
    h.frame(vec![]);
    h.until_saved()?;
    assert_saved(&h)
}

/// F6: ⇧⌘U from the schematic opens the review on the first press, also
/// straight after coming from another workspace and with a field focused.
#[test]
fn update_pcb_runs_on_the_first_press() -> Fallible {
    let shift_cmd = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    let pending = |h: &mut Harness| -> Fallible {
        // A part on the schematic that is not on the board yet.
        let first = h.app.engine.project().instances[0].clone();
        h.app.engine.apply(
            h.app.engine.revision(),
            Operation::Instance(pcbcraft_core::InstanceCommand::Place(
                pcbcraft_core::instances::NewInstance {
                    sheet: None,
                    component: first.component,
                    designator: Some("R77".into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: pcbcraft_core::UnitId(1),
                    position: [30.0, -150.0],
                    physical: first.physical,
                },
            )),
        )?;
        Ok(())
    };
    // Settled on the schematic.
    let mut h = Harness::new()?;
    pending(&mut h)?;
    h.frame(vec![]);
    h.frame(vec![]);
    h.frame(vec![press(egui::Key::U, shift_cmd)]);
    assert_eq!(h.app.workspace, Workspace::Pcb, "settled");
    assert!(h.app.pcb.reviewing(), "settled: no review");
    // In the very frame after arriving from the PCB workspace.
    let mut h = Harness::new()?;
    pending(&mut h)?;
    h.app.workspace = Workspace::Pcb;
    h.frame(vec![]);
    h.frame(vec![press(egui::Key::Num2, egui::Modifiers::COMMAND)]);
    assert_eq!(h.app.workspace, Workspace::Schematic, "⌘2");
    h.frame(vec![press(egui::Key::U, shift_cmd)]);
    assert_eq!(h.app.workspace, Workspace::Pcb, "after ⌘2");
    assert!(h.app.pcb.reviewing(), "after ⌘2: no review");
    // With a card field holding the keyboard: held one frame, then run.
    let mut h = Harness::new()?;
    pending(&mut h)?;
    h.type_in_the_field("TYPED")?;
    h.frame(vec![press(egui::Key::U, shift_cmd)]);
    h.frame(vec![]);
    assert_eq!(h.app.workspace, Workspace::Pcb, "over a field");
    assert!(h.app.pcb.reviewing(), "over a field: no review");
    Ok(())
}

/// F4: Messages shows the engine's text in full, and an engine refusal is
/// written for people; internal command names belong to the MCP instructions
/// and the session's schema, outside what this scan reads.
#[test]
fn no_internal_command_names_reach_a_person() -> Fallible {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
        Ok(())
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    walk(&root.join("src"), &mut files)?;
    walk(&root.join("../../crates/editor-core/src"), &mut files)?;
    let mut found = Vec::new();
    for file in files {
        let name = file.to_string_lossy();
        // The style checker's own list and its test name them on purpose.
        if name.ends_with("issues/style.rs")
            || name.ends_with("issues/tests.rs")
            || name.ends_with("flow_tests.rs")
        {
            continue;
        }
        for (number, line) in std::fs::read_to_string(&file)?.lines().enumerate() {
            if !line.trim_start().starts_with("//")
                && (line.contains("project.preview_update")
                    || line.contains("project.apply_update"))
            {
                found.push(format!("{name}:{}", number + 1));
            }
        }
    }
    assert_eq!(found, Vec::<String>::new());
    Ok(())
}
