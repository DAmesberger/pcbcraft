use super::*;
use crate::commands::Registry;
use sparksmith_core::Engine;

fn press(key: egui::Key, modifiers: egui::Modifiers) -> egui::RawInput {
    egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }],
        ..Default::default()
    }
}

/// Run one headless frame. Its texture changes are cleared: there is no
/// renderer to apply them, and egui's debug check rejects dropping them.
fn run(ctx: &egui::Context, input: egui::RawInput, add: impl FnMut(&mut egui::Ui)) {
    ctx.run_ui(input, add).textures_delta.clear();
}

/// A focused text field owns ⌘Z and plain letters; ⌘S still saves. With
/// no field focused, ⌘Z undoes the project.
#[test]
fn typing_owns_undo_and_letters_instead_of_the_project() -> Result<(), String> {
    let registry = Registry::standard();
    let undo = registry.get("edit.undo").ok_or("undo")?;
    let save = registry.get("file.save").ok_or("save")?;
    let route = registry.get("pcb.route").ok_or("route")?;
    let ctx = egui::Context::default();
    let mut text = "10k".to_owned();
    run(&ctx, Default::default(), |ui| {
        ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"))
            .request_focus();
    });
    let mut seen = (false, Vec::new());
    run(&ctx, press(egui::Key::Z, egui::Modifiers::COMMAND), |ui| {
        ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"));
        seen = (ui.ctx().egui_wants_keyboard_input(), presses(ui.ctx()));
    });
    let (typing, chords) = seen.clone();
    assert!(typing);
    let z = Chord::cmd(egui::Key::Z);
    // The field took ⌘Z itself or left it; either way the project's undo
    // does not run while it has focus.
    assert!(chords.iter().all(|c| *c == z));
    assert!(!dispatchable(undo, z, typing, false));
    assert!(dispatchable(save, Chord::cmd(egui::Key::S), typing, false));
    assert!(!dispatchable(
        route,
        Chord::key(egui::Key::R),
        typing,
        false
    ));
    assert!(
        !dispatchable(route, Chord::key(egui::Key::R), false, true),
        "menu open"
    );
    let unfocused = egui::Context::default();
    run(
        &unfocused,
        press(egui::Key::Z, egui::Modifiers::COMMAND),
        |ui| seen = (ui.ctx().egui_wants_keyboard_input(), presses(ui.ctx())),
    );
    assert_eq!(seen, (false, vec![z]));
    assert!(dispatchable(undo, z, false, false));
    assert!(dispatchable(route, Chord::key(egui::Key::R), false, false));
    Ok(())
}

/// Keys the canvas handles in place are listed but never dispatched.
#[test]
fn in_place_keys_are_not_dispatched() -> Result<(), String> {
    let registry = Registry::standard();
    let line = registry.get("symbol.draw-line").ok_or("line")?;
    assert!(!dispatchable(line, Chord::key(egui::Key::L), false, false));
    let layer = registry
        .resolve(
            Chord::key(egui::Key::V),
            &crate::commands::active_contexts(Context::PcbRoute),
        )
        .ok_or("route V")?;
    assert!(!dispatchable(layer, Chord::key(egui::Key::V), false, false));
    Ok(())
}

/// A registry command runs the same ribbon event as its button: the PCB
/// editor's Route arms, which puts its keys in force; with the 3D view on,
/// Route is refused with the ribbon's reason.
#[test]
fn ribbon_commands_dispatch_to_the_editor_and_report_why_not()
-> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::demo()?;
    let registry = Registry::standard();
    let route = registry.get("pcb.route").ok_or("route")?;
    let Action::Ribbon(Workspace::Pcb, RibbonEvent::Tool(tool)) = route.action else {
        return Err("route is a PCB ribbon tool".into());
    };
    let mut pcb = crate::pcb_editor::PcbEditor::default();
    let state = RibbonState::of(Workspace::Pcb, &pcb.ribbon(&engine));
    assert_eq!(state.allows(RibbonEvent::Tool(tool), route.context), Ok(()));
    assert!(pcb.on_tool(tool, &engine).is_none());
    assert_eq!(pcb.shortcut_context(), Context::PcbRoute);
    pcb.set_3d(true);
    assert_eq!(pcb.shortcut_context(), Context::Pcb3d);
    let state = RibbonState::of(Workspace::Pcb, &pcb.ribbon(&engine));
    let refused = state.allows(RibbonEvent::Tool(tool), route.context);
    assert!(refused.is_err_and(|why| why.contains("Layout")));
    // A tool the ribbon does not offer here names where it applies.
    let line = registry.get("shape.bs-line").ok_or("line")?;
    let refused = state.allows(RibbonEvent::Tool("bs-line"), line.context);
    assert!(refused.is_err_and(|why| why.contains("Board shape")));
    // One that applies everywhere names no place: "here" and "Everywhere"
    // would contradict each other.
    assert_eq!(
        state.allows(RibbonEvent::Tool("bs-line"), Context::Global),
        Err("Not available here".to_owned())
    );
    Ok(())
}

/// Ribbon tooltips name the shortcut: "Route (R)", with the key cap and
/// the help line; a tool without a key keeps its plain name.
#[test]
fn ribbon_tooltips_include_shortcuts() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::demo()?;
    let registry = Registry::standard();
    let mut pcb = crate::pcb_editor::PcbEditor::default();
    let mut ribbon = pcb.ribbon(&engine);
    annotate(&registry, Workspace::Pcb, &mut ribbon);
    let tools: Vec<&crate::shell::Tool> =
        ribbon.groups.iter().flat_map(|g| g.tools.iter()).collect();
    let tip = |id: &str| {
        tools
            .iter()
            .find(|t| t.id == id)
            .map(|t| crate::shell::tool_tip(t))
    };
    let route = tip("route").ok_or("route")?;
    assert_eq!(route.title, "Route (R)");
    assert_eq!(route.key.as_deref(), Some("R"));
    assert!(route.body.contains("Route copper"));
    assert_eq!(tip("via").ok_or("via")?.title, "Via (V)");
    assert_eq!(tip("fill").ok_or("fill")?.title, "Fill (P)");
    let select = tip("select").ok_or("select")?;
    assert_eq!(select.title, "Select");
    assert_eq!(select.key, None);
    // Every tool with a registry key shows it.
    for tool in &tools {
        if let Some(command) = registry.for_tool(Workspace::Pcb, tool.id)
            && let Some(key) = registry.shortcut(command)
        {
            assert!(
                crate::shell::tool_tip(tool)
                    .title
                    .ends_with(&format!("({key})"))
            );
        }
    }
    Ok(())
}

/// Finding 12 of the second UX run: ⇧⌘U is Update PCB from the schematic
/// as from the PCB workspace, and the PCB ribbon lets it run from there
/// (`run_command` enters PCB and opens the review).
#[test]
fn shift_command_u_updates_the_pcb_from_the_schematic() -> Result<(), Box<dyn std::error::Error>> {
    // R3 on the schematic but not yet on the board: an update is pending.
    let mut engine = Engine::demo()?;
    let first = engine.project().instances[0].clone();
    engine.apply(
        engine.revision(),
        sparksmith_core::Operation::Instance(sparksmith_core::InstanceCommand::Place(
            sparksmith_core::instances::NewInstance {
                sheet: None,
                component: first.component,
                designator: Some("R3".into()),
                properties: vec![],
                representation: first.representation,
                unit: sparksmith_core::UnitId(1),
                position: crate::schematic_editor::pt([30.0, 0.0]),
                physical: first.physical,
            },
        )),
    )?;
    let registry = Registry::standard();
    let chord = Chord::cmd(egui::Key::U).with_shift();
    for context in [Context::Schematic, Context::Pcb, Context::PcbLayout] {
        let command = registry
            .resolve(chord, &crate::commands::active_contexts(context))
            .ok_or_else(|| format!("⇧⌘U does nothing in {context:?}"))?;
        assert_eq!(command.id, "pcb.update", "{context:?}");
        assert!(dispatchable(command, chord, false, false), "{context:?}");
        let Action::Ribbon(Workspace::Pcb, event) = command.action else {
            return Err("Update PCB is a PCB ribbon command".into());
        };
        let mut pcb = crate::pcb_editor::PcbEditor::default();
        let state = RibbonState::of(Workspace::Pcb, &pcb.ribbon(&engine));
        assert_eq!(state.allows(event, command.context), Ok(()), "{context:?}");
    }
    Ok(())
}

/// Finding 3 of the UI run: ⌘S is `file.save` in every context, the board
/// shape sketch's exclusive mode included, and runs while a field is
/// focused or a menu is open.
#[test]
fn command_s_saves_everywhere() -> Result<(), String> {
    let registry = Registry::standard();
    let save = Chord::cmd(egui::Key::S);
    for context in Context::ALL {
        let command = registry
            .resolve(save, &crate::commands::active_contexts(context))
            .ok_or_else(|| format!("⌘S does nothing in {context:?}"))?;
        assert_eq!(command.id, "file.save", "{context:?}");
        assert_eq!(command.action, Action::App(AppCommand::Save));
        for (typing, popup) in [(false, false), (true, false), (false, true), (true, true)] {
            assert!(
                dispatchable(command, save, typing, popup),
                "{context:?} typing {typing} popup {popup}"
            );
        }
    }
    Ok(())
}

/// A card field that commits on focus loss, and the dispatcher after it,
/// as one app frame runs them.
struct FieldFrame {
    ctx: egui::Context,
    registry: Registry,
    text: String,
    committed: Vec<String>,
    held: AfterCommit,
    /// Each command run, with the commits made before it.
    ran: Vec<(String, Vec<String>)>,
}

impl FieldFrame {
    fn frame(&mut self, events: Vec<egui::Event>) {
        let Self {
            ctx,
            registry,
            text,
            committed,
            held,
            ran,
        } = self;
        let active = crate::commands::active_contexts(Context::PcbLayout);
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        run(ctx, input, |ui| {
            let field = ui.add(egui::TextEdit::singleline(text).id_salt("value"));
            if field.lost_focus() {
                committed.push(text.clone());
            }
            for id in held.take() {
                ran.push((id, committed.clone()));
            }
            let typing = ui.ctx().egui_wants_keyboard_input();
            for chord in presses(ui.ctx()) {
                if let Some(command) = registry
                    .resolve(chord, &active)
                    .filter(|c| dispatchable(c, chord, typing, false))
                {
                    if typing {
                        held.hold(ui.ctx(), command.id.clone());
                    } else {
                        ran.push((command.id.clone(), committed.clone()));
                    }
                }
            }
        });
    }
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

/// Precision run finding 1: ⌘S over a focused card field takes the
/// keyboard from the field, which commits on that focus loss in the next
/// frame (as Enter would); the save is held until then, so it saves the
/// committed edit. A plain letter (R, the Route tool) still types.
#[test]
fn command_s_over_a_field_commits_the_field_then_saves() {
    let mut f = FieldFrame {
        ctx: egui::Context::default(),
        registry: Registry::standard(),
        text: "10k".to_owned(),
        committed: Vec::new(),
        held: AfterCommit::default(),
        ran: Vec::new(),
    };
    let mut text = f.text.clone();
    run(&f.ctx, Default::default(), |ui| {
        ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"))
            .request_focus();
    });
    f.frame(vec![
        key(egui::Key::R, egui::Modifiers::NONE),
        egui::Event::Text("r".into()),
    ]);
    assert_eq!(f.text, "10kr", "the letter went to the field");
    assert!(f.ran.is_empty(), "the Route tool did not arm");
    assert!(f.ctx.egui_wants_keyboard_input());
    f.frame(vec![key(egui::Key::S, egui::Modifiers::COMMAND)]);
    assert!(!f.ctx.egui_wants_keyboard_input(), "the field let go");
    assert!(f.ran.is_empty() && f.committed.is_empty(), "held");
    f.frame(vec![]);
    assert_eq!(f.committed, vec!["10kr".to_owned()]);
    assert_eq!(
        f.ran,
        vec![("file.save".to_owned(), vec!["10kr".to_owned()])],
        "saved once, after the commit"
    );
    f.frame(vec![]);
    assert_eq!(f.ran.len(), 1, "run once");
    // ⌘2 with nothing focused switches at once.
    f.frame(vec![key(egui::Key::Num2, egui::Modifiers::COMMAND)]);
    assert_eq!(f.ran.len(), 2);
}

/// Precision run finding 5: ⌘S chosen through the menu bar (its key
/// equivalent, taken by the system menu before egui sees a key) over a
/// focused card field. The field commits first, then Save runs once; the
/// same key's press, had it reached egui too, is dropped, so the
/// dispatcher does not save a second time.
#[test]
fn command_s_through_the_menu_commits_the_field_then_saves_once() -> Result<(), String> {
    use crate::menu_bar::{self, Choice, Now};
    let mut f = FieldFrame {
        ctx: egui::Context::default(),
        registry: Registry::standard(),
        text: "10k".to_owned(),
        committed: Vec::new(),
        held: AfterCommit::default(),
        ran: Vec::new(),
    };
    let mut text = f.text.clone();
    run(&f.ctx, Default::default(), |ui| {
        ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"))
            .request_focus();
    });
    f.frame(vec![egui::Event::Text("0".into())]);
    assert_eq!(f.text, "10k0");
    assert!(f.ctx.egui_wants_keyboard_input());
    // The menu's ⌘S, as `App::raw_input_hook` delivers it.
    let active = crate::commands::active_contexts(Context::PcbLayout);
    let now = Now {
        registry: &f.registry,
        active: &active,
        typing: f.ctx.egui_wants_keyboard_input(),
        recording: false,
        browser_shown: true,
        issues_shown: false,
        available: &|_| true,
    };
    let save = menu_bar::FILE
        .iter()
        .copied()
        .find(|e| matches!(e, menu_bar::Entry::Command("file.save", _)))
        .ok_or("no Save item")?;
    let choice = menu_bar::choose(&now, save);
    assert_eq!(choice, Choice::Run("file.save".into()));
    let mut raw = egui::RawInput {
        events: vec![key(egui::Key::S, egui::Modifiers::COMMAND)],
        ..Default::default()
    };
    let mut runs = Vec::new();
    menu_bar::deliver(choice, &f.registry, &mut raw, &mut runs, || None);
    assert!(raw.events.is_empty(), "the key press is dropped");
    // As `App::native_menu` runs it, after this frame's dispatcher.
    f.frame(raw.events);
    assert!(f.ran.is_empty(), "the dispatcher saw no ⌘S");
    for id in runs {
        if let Some(id) = route(&f.ctx, &mut f.held, id) {
            f.ran.push((id, f.committed.clone()));
        }
    }
    assert!(f.ran.is_empty(), "held while the field has the keyboard");
    f.frame(vec![]);
    f.frame(vec![]);
    assert_eq!(f.committed, vec!["10k0".to_owned()]);
    assert_eq!(
        f.ran,
        vec![("file.save".to_owned(), vec!["10k0".to_owned()])],
        "saved once, after the commit"
    );
    // With no field focused, the menu's command runs at once.
    assert_eq!(
        route(&f.ctx, &mut f.held, "file.save".into()),
        Some("file.save".to_owned())
    );
    Ok(())
}

/// ⌘V reaches a focused field as egui-winit delivers it, and pastes there:
/// the sheet's Paste does not run. Off the field the same event is the
/// sheet's ⌘V, and ⌘X and ⌘C the same; a rebinding moves them.
#[test]
fn a_focused_field_keeps_paste_and_the_sheet_gets_it_otherwise() -> Result<(), String> {
    let registry = Registry::standard();
    let active = crate::commands::active_contexts(Context::Schematic);
    let ctx = egui::Context::default();
    let mut text = "10".to_owned();
    run(&ctx, Default::default(), |ui| {
        ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"))
            .request_focus();
    });
    let paste = egui::RawInput {
        events: vec![egui::Event::Paste("k".into())],
        ..Default::default()
    };
    let mut seen = (false, Vec::new());
    run(&ctx, paste, |ui| {
        ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"));
        seen = (ui.ctx().egui_wants_keyboard_input(), presses(ui.ctx()));
    });
    assert_eq!(text, "10k", "the field pasted");
    let (typing, chords) = seen;
    assert!(typing);
    let v = Chord::cmd(egui::Key::V);
    assert_eq!(chords, vec![v]);
    let command = registry.resolve(v, &active).ok_or("⌘V")?;
    assert_eq!(command.id, "edit.paste");
    assert!(!dispatchable(command, v, true, false), "the field's");
    assert!(dispatchable(command, v, false, false), "the sheet's");
    for (event, key, id) in [
        (egui::Event::Cut, egui::Key::X, "edit.cut"),
        (egui::Event::Copy, egui::Key::C, "edit.copy"),
    ] {
        assert_eq!(
            crate::menu_bar::clipboard_chord(&event),
            Some(Chord::cmd(key))
        );
        let command = registry.resolve(Chord::cmd(key), &active).ok_or("key")?;
        assert_eq!(command.id, id);
        assert!(!dispatchable(command, Chord::cmd(key), true, false));
        assert!(dispatchable(command, Chord::cmd(key), false, false));
    }
    Ok(())
}

/// ⌘A runs Select All off a field; a focused field keeps the key for its own
/// text.
#[test]
fn select_all_leaves_the_key_to_a_focused_field() -> Result<(), Box<dyn std::error::Error>> {
    let registry = Registry::standard();
    let active = crate::commands::active_contexts(Context::Schematic);
    let chord = Chord::cmd(egui::Key::A);
    let command = registry.resolve(chord, &active).ok_or("key")?;
    assert_eq!(command.id, "edit.select-all");
    assert!(dispatchable(command, chord, false, false));
    assert!(!dispatchable(command, chord, true, false));
    Ok(())
}

/// Cut, Copy and Paste are the schematic sheet's: elsewhere they say so and
/// stay disabled, whatever the sheet holds.
#[test]
fn the_clipboard_is_disabled_outside_the_schematic() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::demo()?;
    let ctx = egui::Context::default();
    let mut schematic = crate::schematic_editor::SchematicEditor::default();
    let project = engine.project();
    let r1 = project.instances[0].id;
    schematic.select_instance(Some(r1), project);
    schematic.copy_selection(&ctx, &engine)?;
    for app in [AppCommand::Cut, AppCommand::Copy, AppCommand::Paste] {
        assert!(clipboard_availability(Workspace::Schematic, &schematic, project, app).is_ok());
        for workspace in [Workspace::Pcb, Workspace::Part] {
            assert!(clipboard_availability(workspace, &schematic, project, app).is_err());
        }
    }
    Ok(())
}

/// A key the menu hands back to egui (Undo or Redo over a field, the
/// clipboard keys, Delete) is never one the dispatcher runs: the field or
/// canvas acts on it, and only once.
#[test]
fn keys_the_menu_hands_back_are_not_dispatched() {
    use crate::menu_bar::{self, BAR, Choice, Now};
    let registry = Registry::standard();
    let mut handed = 0;
    for context in [Context::PcbLayout, Context::Schematic, Context::Footprint] {
        let active = crate::commands::active_contexts(context);
        for typing in [false, true] {
            let now = Now {
                registry: &registry,
                active: &active,
                typing,
                recording: false,
                browser_shown: true,
                issues_shown: false,
                available: &|_| true,
            };
            for entry in BAR.iter().flat_map(|(_, e)| e.iter()).copied() {
                let Choice::Inject(events) = menu_bar::choose(&now, entry) else {
                    continue;
                };
                for event in events {
                    let egui::Event::Key { key, modifiers, .. } = event else {
                        continue;
                    };
                    handed += 1;
                    let chord = Chord {
                        key,
                        command: modifiers.command,
                        shift: modifiers.shift,
                        alt: modifiers.alt,
                    };
                    if let Some(command) = registry.resolve(chord, &active) {
                        assert!(
                            !dispatchable(command, chord, typing, false),
                            "{context:?} {entry:?}: {} would run twice",
                            command.id
                        );
                    }
                }
            }
        }
    }
    assert!(handed >= 9, "Undo, Redo, Select All and Delete: {handed}");
}

/// Night finding N7: the palette lists the recent projects a query names,
/// newest first, each running Open Recent's open; an empty query lists
/// none, and a missing file says it is removed when chosen.
#[test]
fn the_palette_lists_the_recent_projects_a_query_names() {
    use std::path::PathBuf;
    let paths = [
        PathBuf::from("/w/board.sparksmith"),
        PathBuf::from("/w/old.sparksmith"),
    ];
    let missing = std::collections::BTreeSet::from([paths[1].clone()]);
    let projects = crate::recent_projects::items(&paths, &missing);
    assert!(recent_rows(&projects, " ").is_empty());
    let ids = |query: &str| -> Vec<String> {
        recent_rows(&projects, query)
            .into_iter()
            .map(|r| r.id)
            .collect()
    };
    assert_eq!(ids("recent"), ["file.open-recent:0", "file.open-recent:1"]);
    assert_eq!(ids("open BOARD"), ["file.open-recent:0"]);
    assert!(ids("schematic").is_empty());
    let old = recent_rows(&projects, "old");
    assert_eq!(
        old.first().map(|r| r.detail.as_str()),
        Some("Removed when chosen")
    );
}
