use super::*;
use crate::commands::Registry;
use pcbcraft_core::Engine;

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
        pcbcraft_core::Operation::Instance(pcbcraft_core::InstanceCommand::Place(
            pcbcraft_core::instances::NewInstance {
                sheet: None,
                component: first.component,
                designator: Some("R3".into()),
                properties: vec![],
                representation: first.representation,
                unit: pcbcraft_core::UnitId(1),
                position: [30.0, 0.0],
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
