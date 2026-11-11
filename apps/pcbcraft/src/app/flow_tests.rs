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
                position: crate::schematic_editor::pt([100.0, -100.0]),
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
                    position: crate::schematic_editor::pt([30.0, -150.0]),
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

/// `key` let go, so its next press is a new press.
fn release(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers,
    }
}

/// The toasts shown now, oldest first.
fn toasts(app: &App) -> Vec<String> {
    app.toasts.items().map(|t| t.text.clone()).collect()
}

/// ⌘Z in PCB 3D that undoes a schematic step says so, by the step's
/// title and workspace: it no longer looks as if nothing happened.
#[test]
fn undo_in_pcb_names_the_schematic_step_it_took_back() -> Fallible {
    let mut h = Harness::new()?;
    h.app.workspace = Workspace::Pcb;
    h.app.pcb.set_3d(true);
    h.frame(vec![]);
    h.frame(vec![press(egui::Key::Z, egui::Modifiers::COMMAND)]);
    assert!(
        h.app.engine.project().label(h.label).is_none(),
        "not undone"
    );
    let text = "Undid: Place a label · Schematic";
    assert_eq!(toasts(&h.app), [text]);
    assert!(said(&h.app).contains(text), "{}", said(&h.app));
    Ok(())
}

/// ⇧⌘Z says what it applied again.
#[test]
fn redo_names_the_step_it_applied_again() -> Fallible {
    let mut h = Harness::new()?;
    h.app.workspace = Workspace::Pcb;
    h.frame(vec![]);
    h.frame(vec![
        press(egui::Key::Z, egui::Modifiers::COMMAND),
        release(egui::Key::Z, egui::Modifiers::COMMAND),
    ]);
    let shift_cmd = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    h.frame(vec![press(egui::Key::Z, shift_cmd)]);
    assert!(
        h.app.engine.project().label(h.label).is_some(),
        "not redone"
    );
    assert_eq!(toasts(&h.app), ["Redid: Place a label · Schematic"]);
    Ok(())
}

/// ⌘Z after ⌘Z undoes step after step with one toast, which names the
/// latest step; Messages keeps each. (The app acts on presses, not key
/// repeats, so a key held down undoes once.)
#[test]
fn repeated_undo_replaces_its_toast() -> Fallible {
    let mut h = Harness::new()?;
    h.app.engine.apply(
        h.app.engine.revision(),
        Operation::Schematic(SchematicCommand::MoveLabel {
            label: h.label,
            position: crate::schematic_editor::pt([110.0, -100.0]),
        }),
    )?;
    h.frame(vec![]);
    let cmd = egui::Modifiers::COMMAND;
    h.frame(vec![press(egui::Key::Z, cmd), release(egui::Key::Z, cmd)]);
    assert_eq!(toasts(&h.app), ["Undid: Move a label · Schematic"]);
    h.frame(vec![press(egui::Key::Z, cmd)]);
    assert_eq!(toasts(&h.app), ["Undid: Place a label · Schematic"]);
    let said = said(&h.app);
    assert!(said.contains("Undid: Move a label · Schematic"), "{said}");
    Ok(())
}

/// The toast reads the history's own name for the step, so the two never
/// differ: a step's title and area come from the engine's history.
#[test]
fn the_undo_toast_is_the_history_title() -> Fallible {
    let mut h = Harness::new()?;
    let step = h.app.engine.undo_step().ok_or("no step to undo")?;
    assert_eq!(step.title, "Place a label");
    assert_eq!(step.area, pcbcraft_core::Area::Schematic);
    h.app.history(false);
    let toast = format!(
        "Undid: {} · {}",
        step.title,
        Workspace::from(step.area).label()
    );
    assert_eq!(toasts(&h.app), [toast]);
    assert_eq!(h.app.engine.redo_step(), Some(step));
    Ok(())
}

/// The demo with a VBUS port on R1's pin 2, whose wire runs to R2's pin 1,
/// and a GND port on R2's pin 2: the VBUS wire and its middle.
fn vbus_and_gnd(
    engine: &mut Engine,
) -> Result<(pcbcraft_core::Id, pcbcraft_core::geometry::Point), Box<dyn std::error::Error>> {
    let project = engine.project();
    let (r1, r2) = (project.instances[0].id, project.instances[1].id);
    let (from, to) = (
        pcbcraft_core::WireEnd::from(flow_ep(r1, 2)),
        pcbcraft_core::WireEnd::from(flow_ep(r2, 1)),
    );
    let wire = project
        .wires
        .iter()
        .find(|w| (w.from, w.to) == (from, to) || (w.from, w.to) == (to, from))
        .map(|w| w.id)
        .ok_or("the demo's wire")?;
    let a = pcbcraft_core::schematic::pin_position(project, flow_ep(r1, 2)).ok_or("pin")?;
    let b = pcbcraft_core::schematic::pin_position(project, flow_ep(r2, 1)).ok_or("pin")?;
    for (name, style, at) in [
        ("VBUS", pcbcraft_core::PowerStyle::Bar, flow_ep(r1, 2)),
        ("GND", pcbcraft_core::PowerStyle::Ground, flow_ep(r2, 2)),
    ] {
        engine.apply(
            engine.revision(),
            Operation::Schematic(SchematicCommand::PlaceLabel {
                sheet: None,
                name: name.into(),
                kind: LabelKind::Power { style },
                position: crate::schematic_editor::pt([0.0, 0.0]),
                rotation: Default::default(),
                attach: Some(pcbcraft_core::schematic::WireTarget::Terminal(at)),
            }),
        )?;
    }
    Ok((wire, [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]))
}

fn flow_ep(instance: pcbcraft_core::Id, terminal: u32) -> pcbcraft_core::Endpoint {
    pcbcraft_core::Endpoint {
        instance,
        terminal: pcbcraft_core::TerminalId(terminal),
    }
}

/// Z4: whatever tool sends it, a schematic edit that core refuses as a
/// named-net merge opens the review instead of a refusal toast, and the
/// document is unchanged until the review applies.
#[test]
fn a_named_net_merge_refused_by_core_opens_the_review() -> Fallible {
    let mut h = Harness::new()?;
    let (wire, middle) = vbus_and_gnd(&mut h.app.engine)?;
    let before = h.app.engine.document().clone();
    let messages = h.app.messages.len();
    h.app.apply(
        &h.ctx,
        Origin::Canvas,
        Some(Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "GND".into(),
            kind: LabelKind::Power {
                style: pcbcraft_core::PowerStyle::Ground,
            },
            position: crate::schematic_editor::pt(middle),
            rotation: Default::default(),
            attach: Some(pcbcraft_core::schematic::WireTarget::OnWire(
                pcbcraft_core::schematic::WirePoint {
                    wire,
                    at: crate::schematic_editor::pt(middle),
                },
            )),
        })),
    );
    assert!(h.app.schematic.reviewing(), "no review: {}", said(&h.app));
    assert_eq!(h.app.engine.document(), &before);
    assert_eq!(h.app.messages.len(), messages, "a refusal was logged");
    assert!(h.app.toasts.is_empty(), "a refusal toast");
    Ok(())
}

/// Demo run item 11: the Production dialog belongs to the project it
/// checked; opening another project closes it, so the previous project's
/// written files are never shown for the new one.
#[test]
fn opening_another_project_closes_the_production_dialog() -> Fallible {
    let mut h = Harness::new()?;
    h.app.workspace = Workspace::Pcb;
    assert!(h.app.pcb.on_tool("production", &h.app.engine).is_none());
    h.frame(vec![]);
    assert!(h.app.pcb.production_open());
    let other = Engine::showcase()?.to_file();
    h.app
        .replace(other, Some(h.dir.path().join("other.pcbcraft")), false);
    h.frame(vec![]);
    assert!(!h.app.pcb.production_open(), "{}", said(&h.app));
    Ok(())
}

/// Night round 3 (R3-3): ⌘P finds the Board card's setup dialogs by name
/// and by the words a designer types ("drc", "stackup", "net class",
/// "diff pair"), and running one opens that dialog on the PCB, from
/// another workspace and over an open Production dialog.
#[test]
fn the_palette_opens_the_board_setup_dialogs() -> Fallible {
    use crate::commands::PcbDialog as D;
    let mut h = Harness::new()?;
    for (query, id, dialog) in [
        ("drc", "pcb.rules", D::Rules),
        ("design rules", "pcb.rules", D::Rules),
        ("stackup", "pcb.stackup", D::Stackup),
        ("net class", "pcb.net-classes", D::NetClasses),
        ("diff pair", "pcb.high-speed", D::HighSpeed),
        ("high speed", "pcb.high-speed", D::HighSpeed),
    ] {
        h.app.workspace = Workspace::Pcb;
        let active = h.app.active_contexts();
        let hits = h.app.registry.search(query, &[], &active);
        // Letters in order ("drc" in "Draw arc") do not bury it.
        assert!(
            hits.iter().all(|c| !c.id.starts_with("symbol.draw")),
            "{query}: {:?}",
            hits.iter().map(|c| c.id.as_str()).collect::<Vec<_>>()
        );
        let first = hits.first().map(|c| c.id.clone());
        assert_eq!(first.as_deref(), Some(id), "{query}");
        h.app.workspace = Workspace::Schematic;
        h.app.pcb.on_tool("production", &h.app.engine);
        let ctx = h.ctx.clone();
        h.app.run_command(&ctx, id);
        h.frame(vec![]);
        assert_eq!(h.app.workspace, Workspace::Pcb, "{query}");
        assert_eq!(h.app.pcb.open_dialog_now(), Some(dialog), "{query}");
        assert!(!h.app.pcb.production_open(), "{query}");
    }
    Ok(())
}

/// Night round 4 Q2: a refused card action names what was refused and on
/// what ("Can't rotate J1: …"), not only the engine's reason; an operation
/// whose editor named nothing is named by its title. Refused, the document
/// is unchanged.
#[test]
fn a_refusal_toast_names_the_action_and_the_part() -> Fallible {
    let mut h = Harness::new()?;
    let r1 = h
        .app
        .engine
        .project()
        .instance_by_designator("R1")
        .ok_or("R1")?
        .id;
    let placement = h
        .app
        .engine
        .project()
        .placements
        .iter()
        .find(|p| p.instance == r1)
        .ok_or("R1 is not placed")?
        .clone();
    let far = || {
        Some(Operation::Layout(
            pcbcraft_core::layout::LayoutCommand::Move {
                instance: r1,
                position: [5000.0, 9.0],
                rotation_deg: placement.rotation_deg,
                layer: placement.layer,
            },
        ))
    };
    let before = h.app.engine.document().clone();
    crate::refusal::name_action(&h.ctx, "rotate R1");
    h.app.apply(&h.ctx, Origin::Command, far());
    assert_eq!(h.app.engine.document(), &before);
    let shown: Vec<_> = h.app.toasts.items().map(|t| t.text.clone()).collect();
    assert_eq!(
        shown,
        ["Can't rotate R1: R1 must stay within -1000 to 2000 mm on each axis"]
    );
    h.app.toasts.dismiss_errors();
    h.app.apply(&h.ctx, Origin::Canvas, far());
    let shown: Vec<_> = h.app.toasts.items().map(|t| t.text.clone()).collect();
    assert_eq!(
        shown,
        ["Can't move a placed part: R1 must stay within -1000 to 2000 mm on each axis"]
    );
    Ok(())
}

/// A board edit whose moved parts drag copper that the engine re-routes
/// says so in one toast, with Undo: on the ESP32 board, 40 → 41 mm moves
/// J1 and U3 and re-routes the USB pair as a pair.
#[test]
fn a_reroute_after_a_board_edit_is_one_toast() -> Fallible {
    use pcbcraft_core::layout::outline::{BoardCommand, BoardEdit};
    use pcbcraft_core::library::footprint_source::{Expr, FootprintParam, ParamName, ParamUnit};
    let mut h = Harness::new()?;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    h.app.engine = Engine::from_file(pcbcraft_io::read_project(&path)?)?;
    let width = Operation::Board(BoardCommand::Edit {
        edits: vec![BoardEdit::SetParameter {
            param: FootprintParam {
                name: ParamName("width".into()),
                expr: Expr("41 mm".into()),
                unit: ParamUnit::Mm,
                comment: String::new(),
            },
        }],
    });
    h.app.apply(&h.ctx, Origin::Command, Some(width));
    let shown: Vec<_> = h
        .app
        .toasts
        .items()
        .map(|t| (t.text.clone(), t.actions.clone()))
        .collect();
    assert_eq!(
        shown,
        [(
            "Re-routed 2 connections".to_owned(),
            vec![ToastAction::Undo]
        )]
    );
    Ok(())
}

/// Night finding N7: the project file just opened joins File › Open
/// Recent; a recent file that is gone is removed when chosen, with a toast,
/// and nothing is opened.
#[test]
fn open_recent_keeps_the_project_and_drops_a_missing_file() -> Fallible {
    let mut h = Harness::new()?;
    h.frame(vec![]);
    assert_eq!(h.app.sources.recent_projects(), [h.path()]);
    let gone = h.dir.path().join("gone.pcbcraft");
    h.app.sources.remember_project(&gone);
    assert_eq!(h.app.sources.recent_projects(), [gone.clone(), h.path()]);
    h.app.open_recent(gone);
    assert_eq!(h.app.sources.recent_projects(), [h.path()]);
    assert!(!h.app.files.busy(), "nothing is opened");
    let said = said(&h.app);
    assert!(
        said.contains("gone.pcbcraft is no longer there; removed from Open Recent"),
        "{said}"
    );
    Ok(())
}
