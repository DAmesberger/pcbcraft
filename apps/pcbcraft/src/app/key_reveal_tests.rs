use super::*;
use crate::app::command_dispatch::RibbonState;
use crate::commands::Chord;
use pcbcraft_core::Engine;
use pcbcraft_io::settings::RevealSettings;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn active(context: Context) -> Vec<Context> {
    crate::commands::active_contexts(context)
}

/// Badges show the key that fires here: F is Fit view in Layout, but
/// while routing the Route tool takes F (Flip the elbow), so the Fit
/// view button shows no F and the Route panel lists it. A rebinding moves
/// the badge with it.
#[test]
fn badges_resolve_context_and_shadowing() -> Result<(), String> {
    let registry = Registry::standard();
    let fit = registry.get("view.fit").ok_or("fit")?;
    let f = Chord::key(egui::Key::F).shown();
    assert_eq!(
        key_in_force(&registry, fit, &active(Context::PcbLayout)),
        Some(f.clone())
    );
    assert_eq!(
        key_in_force(&registry, fit, &active(Context::PcbRoute)),
        None
    );
    assert_eq!(
        key_in_force(&registry, fit, &active(Context::PcbTune)),
        None
    );
    let route = command_of(&registry, Workspace::Pcb, Target::Tool("route")).ok_or("route")?;
    assert_eq!(
        key_in_force(&registry, route, &active(Context::PcbLayout)).as_deref(),
        Some("R")
    );
    // The palette's ⇧⌘P is the same action as ⌘P: still a badge.
    let palette = command_of(
        &registry,
        Workspace::Pcb,
        Target::App(crate::commands::AppCommand::Palette),
    )
    .ok_or("palette")?;
    assert!(key_in_force(&registry, palette, &active(Context::PcbRoute)).is_some());
    // The Route panel (near the pointer) lists F as Flip the elbow.
    let ctx = egui::Context::default();
    let rows = crate::keys::rows(&ctx, crate::pcb_editor::ROUTE_KEYS);
    assert!(
        rows.iter()
            .any(|(key, what)| key.starts_with(&f) && what == "Flip the elbow corner"),
        "{rows:?}"
    );
    // Keys without a button: Esc and ⇧F in Layout; in Route mode the
    // mode's own keys stay in their panel.
    let free = free_rows(&registry, &active(Context::PcbLayout), false, &["view.fit"]);
    assert!(
        free.iter()
            .any(|(_, what)| what == "Cancel the tool or gesture, else deselect")
    );
    assert!(free.iter().any(|(_, what)| what == "View from bottom"));
    assert!(
        !free.iter().any(|(_, what)| what == "Fit view"),
        "has a button"
    );
    let routing = free_rows(&registry, &active(Context::PcbRoute), true, &[]);
    assert!(
        !routing
            .iter()
            .any(|(_, what)| what == "Flip the elbow corner")
    );
    assert!(
        !routing.iter().any(|(_, what)| what == "View from bottom"),
        "⇧F flips"
    );
    // Rebound, the badge shows the new key, even while routing.
    let mut rebound = registry.clone();
    rebound.rebind("view.fit", Some(Chord::key(egui::Key::K)))?;
    let fit = rebound.get("view.fit").ok_or("fit")?;
    assert_eq!(
        key_in_force(&rebound, fit, &active(Context::PcbRoute)).as_deref(),
        Some("K")
    );
    Ok(())
}

/// Every ribbon this app shows: each workspace and its modes.
fn ribbons(engine: &Engine) -> Vec<(Workspace, Context, Ribbon)> {
    let mut out = Vec::new();
    let mut part = crate::part_editor::PartEditor::default();
    out.push((
        Workspace::Part,
        part.shortcut_context(),
        part.ribbon(engine),
    ));
    for tab in ["symbol", "footprint", "pins", "details"] {
        part.on_tab(tab);
        out.push((
            Workspace::Part,
            part.shortcut_context(),
            part.ribbon(engine),
        ));
    }
    let mut schematic = crate::schematic_editor::SchematicEditor::default();
    out.push((
        Workspace::Schematic,
        Context::Schematic,
        schematic.ribbon(engine),
    ));
    let mut pcb = crate::pcb_editor::PcbEditor::default();
    out.push((Workspace::Pcb, pcb.shortcut_context(), pcb.ribbon(engine)));
    let _ = pcb.on_tool("route", engine);
    out.push((Workspace::Pcb, pcb.shortcut_context(), pcb.ribbon(engine)));
    let mut pcb = crate::pcb_editor::PcbEditor::default();
    pcb.set_3d(true);
    out.push((Workspace::Pcb, pcb.shortcut_context(), pcb.ribbon(engine)));
    out
}

fn check_level(items: &[TipItem]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for item in items {
        if item.tip.is_empty() || !item.tip.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(format!("{} has tip {:?}", item.label, item.tip));
        }
        if !seen.insert(item.tip.clone()) {
            return Err(format!("tip {} twice", item.tip));
        }
    }
    for item in items.iter().filter(|i| i.tip.len() == 2) {
        if items
            .iter()
            .any(|o| o.tip.len() == 1 && item.tip.starts_with(&o.tip))
        {
            return Err(format!("{} starts with a one-letter tip", item.tip));
        }
    }
    Ok(())
}

/// Every ribbon command in every workspace and mode gets a tip: letters
/// only, unique on its level, never ambiguous, the same each time, and a
/// command's single-letter key where it has one (Route is R).
#[test]
fn every_visible_ribbon_command_gets_a_unique_tip() -> TestResult {
    let engine = Engine::demo()?;
    let registry = Registry::standard();
    for (workspace, context, mut ribbon) in ribbons(&engine) {
        crate::app::command_dispatch::annotate(&registry, workspace, &mut ribbon);
        let active = active(context);
        let level = ribbon_tips(&registry, workspace, &ribbon, &active, None);
        check_level(&level).map_err(|e| format!("{workspace:?} {context:?}: {e}"))?;
        assert_eq!(
            level,
            ribbon_tips(&registry, workspace, &ribbon, &active, None)
        );
        for family in level.iter().filter(|i| i.family) {
            let Target::Tool(id) = family.target else {
                continue;
            };
            let tools = ribbon_tips(&registry, workspace, &ribbon, &active, Some(id));
            assert!(!tools.is_empty(), "{id} lists its tools");
            check_level(&tools).map_err(|e| format!("{id}: {e}"))?;
        }
        for item in &level {
            if let Some(key) = command_of(&registry, workspace, item.target)
                .filter(|_| !item.family)
                .and_then(|c| {
                    registry
                        .chords(c)
                        .into_iter()
                        .find(|ch| registry.resolve(*ch, &active).is_some_and(|r| r.id == c.id))
                })
                .and_then(reveal::single_letter)
            {
                assert_eq!(item.tip, key.to_string(), "{}", item.label);
            }
        }
        if context == Context::PcbLayout {
            let route = level.iter().find(|i| i.label == "Route").ok_or("route")?;
            assert_eq!(route.tip, "R");
        }
    }
    Ok(())
}

/// Typing a tip picks its command, which runs through the command path:
/// R arms Route in Layout; in the 3D view the same tip is refused with the
/// ribbon's reason. A letter no tip starts with starts over.
#[test]
fn typing_a_tip_dispatches_it() -> TestResult {
    let engine = Engine::demo()?;
    let registry = Registry::standard();
    let mut pcb = crate::pcb_editor::PcbEditor::default();
    let mut ribbon = pcb.ribbon(&engine);
    crate::app::command_dispatch::annotate(&registry, Workspace::Pcb, &mut ribbon);
    let active = active(pcb.shortcut_context());
    let mut tips = Tips {
        items: ribbon_tips(&registry, Workspace::Pcb, &ribbon, &active, None),
        ..Tips::default()
    };
    assert_eq!(type_letter(&mut tips, 'q'), TipKey::More);
    assert!(tips.typed.is_empty(), "no tip starts with Q");
    let TipKey::Pick(item) = type_letter(&mut tips, 'r') else {
        return Err("R picks Route".into());
    };
    let command = command_of(&registry, Workspace::Pcb, item.target).ok_or("command")?;
    assert_eq!(command.id, "pcb.route");
    let Action::Ribbon(Workspace::Pcb, event @ RibbonEvent::Tool(tool)) = command.action else {
        return Err("a ribbon tool".into());
    };
    let state = RibbonState::of(Workspace::Pcb, &ribbon);
    assert_eq!(state.allows(event, command.context), Ok(()));
    assert!(pcb.on_tool(tool, &engine).is_none());
    assert_eq!(pcb.shortcut_context(), Context::PcbRoute);
    pcb.set_3d(true);
    let state = RibbonState::of(Workspace::Pcb, &pcb.ribbon(&engine));
    assert!(
        state
            .allows(event, command.context)
            .is_err_and(|why| why.contains("Layout"))
    );
    Ok(())
}

/// ⌥ tapped while a text field has the keyboard opens no tips (⌥ types
/// there); open tips close once a field takes the keyboard. Without a
/// field the tap opens them and a second tap closes them.
#[test]
fn text_focus_suppresses_key_tips() {
    let ctx = egui::Context::default();
    let mut text = String::new();
    let mut frame = |focus: bool, modifiers: egui::Modifiers| {
        let mut out = (reveal::Sample::default(), false);
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::ModifiersChanged(modifiers)],
                ..Default::default()
            },
            |ui| {
                let field = ui.add(egui::TextEdit::singleline(&mut text).id_salt("f"));
                if focus {
                    field.request_focus();
                } else {
                    field.surrender_focus();
                }
                out = (
                    reveal::sample(ui.ctx()),
                    ui.ctx().egui_wants_keyboard_input(),
                );
            },
        )
        .textures_delta
        .clear();
        out
    };
    let settings = RevealSettings::default();
    let mut reveal = KeyReveal::default();
    // Focus the field, then tap ⌥.
    frame(true, egui::Modifiers::NONE);
    for modifiers in [egui::Modifiers::ALT, egui::Modifiers::NONE] {
        let (sample, typing) = frame(true, modifiers);
        assert!(typing);
        assert!(!reveal.step(sample, settings, typing, false));
    }
    assert!(reveal.tips.is_none(), "typing: no tips");
    // No field: the tap opens them, the next one closes them.
    frame(false, egui::Modifiers::NONE);
    for modifiers in [egui::Modifiers::ALT, egui::Modifiers::NONE] {
        let (sample, typing) = frame(false, modifiers);
        assert!(!typing);
        reveal.step(sample, settings, typing, false);
    }
    assert!(reveal.tips.is_some(), "a tap opens key tips");
    let (sample, typing) = frame(true, egui::Modifiers::NONE);
    reveal.step(sample, settings, typing, false);
    assert!(reveal.tips.is_none(), "a focused field closes them");
    // Turned off in Settings, a tap does nothing.
    let off = RevealSettings {
        key_tips: false,
        ..settings
    };
    for modifiers in [egui::Modifiers::ALT, egui::Modifiers::NONE] {
        let (sample, typing) = frame(false, modifiers);
        reveal.step(sample, off, typing, false);
    }
    assert!(reveal.tips.is_none());
}

/// Holding ⌘ shows the badges after the delay; a ⌘S press hides them at
/// once and the press is left for the command (nothing is consumed).
#[test]
fn a_command_chord_hides_the_badges_and_still_runs() {
    let ctx = egui::Context::default();
    let settings = RevealSettings::default();
    let mut reveal = KeyReveal::default();
    let mut frame = |time: f64, mut events: Vec<egui::Event>| {
        let mut out = (false, 0);
        events.insert(0, egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
        ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                let sample = reveal::sample(ui.ctx());
                reveal.step(sample, settings, false, false);
                let keys = ui.input(|i| {
                    i.events
                        .iter()
                        .filter(|e| matches!(e, egui::Event::Key { .. }))
                        .count()
                });
                out = (reveal.badges(), keys);
            },
        )
        .textures_delta
        .clear();
        out
    };
    assert_eq!(frame(1.0, vec![]), (false, 0));
    assert_eq!(frame(1.2, vec![]), (false, 0));
    assert!(frame(1.6, vec![]).0, "shown after 500 ms");
    let save = egui::Event::Key {
        key: egui::Key::S,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    assert_eq!(
        frame(1.7, vec![save]),
        (false, 1),
        "hidden; ⌘S left for Save"
    );
    assert!(!frame(3.0, vec![]).0, "until ⌘ is let go");
}
