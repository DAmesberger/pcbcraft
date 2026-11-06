use super::*;
use crate::commands::{KeyDecl, active_contexts};
use egui::Key as K;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn now<'a>(
    registry: &'a Registry,
    active: &'a [Context],
    typing: bool,
    available: &'a dyn Fn(&Command) -> bool,
) -> Now<'a> {
    Now {
        registry,
        active,
        typing,
        recording: false,
        browser_shown: true,
        issues_shown: false,
        available,
    }
}

fn all(_: &Command) -> bool {
    true
}

/// Every command entry of the bar and of the in-window File menu.
fn command_entries() -> Vec<Entry> {
    BAR.iter()
        .flat_map(|(_, entries)| entries.iter())
        .chain(IN_WINDOW_FILE)
        .copied()
        .filter(|e| matches!(e, Entry::Command(..)))
        .collect()
}

/// The item of `id` in `entries`, as shown now.
fn item(now: &Now, id: &str) -> Result<State, String> {
    command_entries()
        .into_iter()
        .find(|e| matches!(e, Entry::Command(i, _) if *i == id))
        .and_then(|e| state(now, e))
        .ok_or_else(|| format!("no item {id}"))
}

/// Every item names a registry command; its key equivalent is the
/// command's effective key where that is a ⌘ chord, and the key it shows
/// is the registry's. The finding's keys are all there.
#[test]
fn every_item_is_a_registry_command_with_its_key() -> TestResult {
    let registry = Registry::standard();
    let active = active_contexts(Context::PcbLayout);
    let now = now(&registry, &active, false, &all);
    for entry in command_entries() {
        let Entry::Command(id, _) = entry else {
            continue;
        };
        let command = registry.get(id).ok_or(format!("{id} is not a command"))?;
        let shown = state(&now, entry).ok_or(id)?;
        let chord = registry.chord(command);
        assert_eq!(
            shown.key,
            chord.filter(|c| c.command),
            "{id}: the menu takes the registry's ⌘ key"
        );
        assert_eq!(shown.shown, registry.menu_shortcut(command), "{id}");
        // The clipboard items wait for a focused field.
        assert_eq!(shown.enabled, text_edit(id).is_none(), "{id}");
    }
    for (id, key) in [
        ("app.settings", Chord::cmd(K::Comma)),
        ("app.quit", Chord::cmd(K::Q)),
        ("file.new", Chord::cmd(K::N)),
        ("file.open", Chord::cmd(K::O)),
        ("file.save", Chord::cmd(K::S)),
        ("file.save-as", Chord::cmd(K::S).with_shift()),
        ("file.export", Chord::cmd(K::E).with_shift()),
        ("window.close", Chord::cmd(K::W)),
        ("edit.undo", Chord::cmd(K::Z)),
        ("edit.redo", Chord::cmd(K::Z).with_shift()),
        ("edit.cut", Chord::cmd(K::X)),
        ("edit.copy", Chord::cmd(K::C)),
        ("edit.paste", Chord::cmd(K::V)),
        ("edit.select-all", Chord::cmd(K::A)),
        ("workspace.component", Chord::cmd(K::Num1)),
        ("workspace.schematic", Chord::cmd(K::Num2)),
        ("workspace.pcb", Chord::cmd(K::Num3)),
        ("view.browser", Chord::cmd(K::B)),
        ("view.issues", Chord::cmd(K::I).with_shift()),
        ("view.messages", Chord::cmd(K::M).with_shift()),
        ("app.palette", Chord::cmd(K::P)),
        ("app.shortcuts", Chord::cmd(K::Slash)),
    ] {
        assert_eq!(item(&now, id)?.key, Some(key), "{id}");
    }
    // A plain key is shown in the window, never taken by the system menu,
    // which would take it from text fields.
    let fit = item(&now, "view.fit")?;
    assert_eq!(fit.key, None);
    assert_eq!(fit.shown, Some(Chord::key(K::F).shown()));
    let delete = state(&now, Entry::Delete).ok_or("delete")?;
    assert_eq!(delete.key, None);
    assert_eq!(delete.shown, Some(Chord::key(K::Delete).shown()));
    Ok(())
}

/// The menus are grouped: no menu starts or ends with a separator or has
/// two in a row, and each item sits in its menu (Settings in the app menu,
/// Messages in Window, the Browser in View, Save in File).
#[test]
fn menus_are_grouped_like_a_mac_app() {
    for (title, entries) in BAR.iter().chain([&("in-window File", IN_WINDOW_FILE)]) {
        assert_ne!(entries.first(), Some(&Entry::Separator), "{title}");
        assert_ne!(entries.last(), Some(&Entry::Separator), "{title}");
        assert!(
            !entries
                .windows(2)
                .any(|w| w[0] == Entry::Separator && w[1] == Entry::Separator),
            "{title}"
        );
    }
    let menu_of = |id: &str| {
        BAR.iter()
            .find(|(_, entries)| {
                entries
                    .iter()
                    .any(|e| matches!(e, Entry::Command(i, _) if *i == id))
            })
            .map(|(title, _)| *title)
    };
    assert_eq!(menu_of("app.settings"), Some("Pcbcraft"));
    assert_eq!(menu_of("app.quit"), Some("Pcbcraft"));
    assert_eq!(menu_of("file.save"), Some("File"));
    assert_eq!(menu_of("edit.undo"), Some("Edit"));
    assert_eq!(menu_of("view.browser"), Some("View"));
    assert_eq!(menu_of("app.palette"), Some("View"));
    assert_eq!(menu_of("view.messages"), Some("Window"));
    assert_eq!(menu_of("app.shortcuts"), Some("Help"));
    let titles: Vec<&str> = BAR.iter().map(|(t, _)| *t).collect();
    assert_eq!(
        titles,
        ["Pcbcraft", "File", "Edit", "View", "Window", "Help"]
    );
    // The in-window menu keeps to File and the app's own windows.
    for id in ["view.browser", "app.palette"] {
        assert!(
            !IN_WINDOW_FILE
                .iter()
                .any(|e| matches!(e, Entry::Command(i, _) if *i == id)),
            "{id}"
        );
    }
}

/// No two items of the bar take the same key, in any context: a key
/// equivalent runs one item.
#[test]
fn no_two_items_take_the_same_key() {
    let registry = Registry::standard();
    for context in Context::ALL {
        let active = active_contexts(context);
        let now = now(&registry, &active, false, &all);
        let mut seen = std::collections::BTreeMap::new();
        for (_, entries) in BAR {
            for entry in *entries {
                if let Some(key) = state(&now, *entry).and_then(|s| s.key) {
                    let name = key.name();
                    assert!(
                        seen.insert(name.clone(), *entry).is_none(),
                        "{context:?}: {name} on two items"
                    );
                }
            }
        }
    }
}

/// Enabled follows the command's availability; Undo and Redo stay
/// enabled while typing (they are the field's then); the clipboard items
/// are enabled only while typing; Delete only where a canvas deletes and
/// nothing is being typed. A disabled item keeps showing its key.
#[test]
fn enabled_follows_availability() -> TestResult {
    let registry = Registry::standard();
    let active = active_contexts(Context::PcbLayout);
    let busy = |c: &Command| !matches!(c.id.as_str(), "file.save" | "edit.undo");
    let idle = now(&registry, &active, false, &busy);
    let save = item(&idle, "file.save")?;
    assert!(!save.enabled);
    assert_eq!(save.key, Some(Chord::cmd(K::S)));
    assert!(item(&idle, "file.open")?.enabled);
    assert!(!item(&idle, "edit.undo")?.enabled);
    for id in ["edit.cut", "edit.copy", "edit.paste", "edit.select-all"] {
        assert!(!item(&idle, id)?.enabled, "{id} without a field");
    }
    assert!(state(&idle, Entry::Delete).is_some_and(|s| s.enabled));
    let typing = now(&registry, &active, true, &busy);
    assert!(item(&typing, "edit.undo")?.enabled, "the field's undo");
    for id in ["edit.cut", "edit.copy", "edit.paste", "edit.select-all"] {
        assert!(item(&typing, id)?.enabled, "{id} in a field");
    }
    assert!(state(&typing, Entry::Delete).is_some_and(|s| !s.enabled));
    // With nothing to delete it is greyed, whatever the keyboard.
    let empty = now(&registry, &active, false, &|_| false);
    assert!(state(&empty, Entry::Delete).is_some_and(|s| !s.enabled));
    // Nothing deletes in the Global context alone.
    let global = active_contexts(Context::Global);
    let nowhere = now(&registry, &global, false, &all);
    assert!(state(&nowhere, Entry::Delete).is_some_and(|s| !s.enabled));
    // Each workspace's canvas: the Delete key of its own delete command.
    for (context, id) in [
        (Context::Schematic, "schematic.delete"),
        (Context::Footprint, "footprint.delete"),
        (Context::BoardShape, "shape.delete"),
        (Context::PcbLayout, "pcb.delete"),
    ] {
        let active = active_contexts(context);
        let here = now(&registry, &active, false, &all);
        let target = delete_target(&here).map(|(c, _)| c.id.as_str());
        assert_eq!(target, Some(id), "{context:?}");
    }
    Ok(())
}

/// A rebinding moves the item's key and the key it shows; while Settings
/// records a key, no item takes one; a key an inner context takes for
/// itself stays with it there; the Browser and Issues items say what
/// they do.
#[test]
fn keys_follow_the_keymap_and_the_context() -> TestResult {
    let mut registry = Registry::standard();
    registry.rebind("file.save", Some(Chord::cmd(K::J)))?;
    let active = active_contexts(Context::PcbLayout);
    let moved = now(&registry, &active, false, &all);
    let save = item(&moved, "file.save")?;
    assert_eq!(save.key, Some(Chord::cmd(K::J)));
    assert_eq!(save.shown, Some(Chord::cmd(K::J).shown()));
    registry.rebind("file.save", None)?;
    let none = now(&registry, &active, false, &all);
    assert_eq!(item(&none, "file.save")?.key, None);
    let registry = Registry::standard();
    let recording = Now {
        recording: true,
        ..now(&registry, &active, false, &all)
    };
    for entry in command_entries() {
        assert_eq!(state(&recording, entry).and_then(|s| s.key), None);
    }
    // A tool mode that takes ⌘S for itself: the menu leaves it the key there.
    const KEYS: &[KeyDecl] = &[KeyDecl::new("route.test-save", "Ctrl+S", "Mode's own ⌘S")];
    let shadowed = Registry::standard().with_keys(Context::PcbRoute, KEYS);
    let route = active_contexts(Context::PcbRoute);
    assert_eq!(
        item(&now(&shadowed, &route, false, &all), "file.save")?.key,
        None
    );
    assert_eq!(
        item(&now(&shadowed, &active, false, &all), "file.save")?.key,
        Some(Chord::cmd(K::S))
    );
    let shown = now(&registry, &active, false, &all);
    assert_eq!(item(&shown, "view.browser")?.label, "Hide Browser");
    assert_eq!(item(&shown, "view.issues")?.label, "Show Issues");
    Ok(())
}

/// Choosing an item: a command runs through the registry; over a field
/// Undo, Redo and the clipboard items give the field its own input;
/// Delete gives the canvas its key; a disabled item does nothing.
#[test]
fn choosing_an_item_runs_its_command_or_hands_the_key_back() {
    let registry = Registry::standard();
    let active = active_contexts(Context::PcbLayout);
    let idle = now(&registry, &active, false, &all);
    let typing = now(&registry, &active, true, &all);
    let save = C("file.save", "Save");
    assert_eq!(choose(&idle, save), Choice::Run("file.save".into()));
    assert_eq!(choose(&typing, save), Choice::Run("file.save".into()));
    let undo = C("edit.undo", "Undo");
    assert_eq!(choose(&idle, undo), Choice::Run("edit.undo".into()));
    let cmd = |key, shift| {
        let mut modifiers = egui::Modifiers::COMMAND;
        if shift {
            modifiers |= egui::Modifiers::SHIFT;
        }
        press(key, modifiers)
    };
    assert_eq!(
        choose(&typing, undo),
        Choice::Inject(vec![cmd(K::Z, false)])
    );
    assert_eq!(
        choose(&typing, C("edit.redo", "Redo")),
        Choice::Inject(vec![cmd(K::Z, true)])
    );
    assert_eq!(
        choose(&typing, C("edit.copy", "Copy")),
        Choice::Inject(vec![egui::Event::Copy])
    );
    assert_eq!(
        choose(&typing, C("edit.cut", "Cut")),
        Choice::Inject(vec![egui::Event::Cut])
    );
    assert_eq!(choose(&typing, C("edit.paste", "Paste")), Choice::Paste);
    assert_eq!(
        choose(&typing, C("edit.select-all", "Select All")),
        Choice::Inject(vec![cmd(K::A, false)])
    );
    assert_eq!(choose(&idle, C("edit.copy", "Copy")), Choice::Nothing);
    assert_eq!(
        choose(&idle, Entry::Delete),
        Choice::Inject(vec![press(K::Delete, egui::Modifiers::NONE)])
    );
    assert_eq!(choose(&typing, Entry::Delete), Choice::Nothing);
    let none = |_: &Command| false;
    assert_eq!(
        choose(&now(&registry, &active, false, &none), save),
        Choice::Nothing
    );
    assert_eq!(choose(&idle, Entry::System(System::About)), Choice::Nothing);
}

/// The menu's ⌘S and a press of ⌘S in the same input run Save once: the
/// press is dropped. Other keys stay; injected events and pasted text
/// join the input.
#[test]
fn a_menu_command_and_its_key_run_once() {
    let registry = Registry::standard();
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let mut raw = egui::RawInput {
        events: vec![
            key(K::S, egui::Modifiers::COMMAND),
            key(K::B, egui::Modifiers::COMMAND),
            key(K::S, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT),
        ],
        ..Default::default()
    };
    let mut runs = Vec::new();
    deliver(
        Choice::Run("file.save".into()),
        &registry,
        &mut raw,
        &mut runs,
        || None,
    );
    assert_eq!(runs, vec!["file.save".to_owned()]);
    assert_eq!(
        raw.events,
        vec![
            key(K::B, egui::Modifiers::COMMAND),
            key(K::S, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT),
        ],
        "only ⌘S is dropped"
    );
    deliver(
        Choice::Inject(vec![egui::Event::Copy]),
        &registry,
        &mut raw,
        &mut runs,
        || None,
    );
    deliver(Choice::Paste, &registry, &mut raw, &mut runs, || {
        Some("10k".into())
    });
    deliver(Choice::Paste, &registry, &mut raw, &mut runs, || None);
    assert_eq!(runs.len(), 1);
    assert_eq!(
        raw.events[2..],
        [egui::Event::Copy, egui::Event::Paste("10k".into())]
    );
}

/// The in-window File menu, as Solidcraft's: plain rows about a control
/// high, each key right-aligned in the row, separators between groups,
/// about 250 points wide, no prose. With `PCBCRAFT_TEXT_PICTURES=<dir>` it
/// writes `file-menu.png`.
#[test]
fn the_in_window_file_menu_is_rows_with_keys() -> TestResult {
    use crate::kit::text::tests::{Screen, painted};
    let registry = Registry::standard();
    let active = active_contexts(Context::PcbLayout);
    let busy = |c: &Command| c.id != "file.export-schematic-pdf";
    let rows = rows(&now(&registry, &active, false, &busy), IN_WINDOW_FILE);
    let size = egui::vec2(320.0, 520.0);
    let screen = Screen::new(size);
    let chosen = std::cell::Cell::new(None);
    let mut draw = |ui: &mut egui::Ui| {
        egui::Frame::menu(ui.style()).show(ui, |ui| {
            if let Some(id) = show_rows(ui, &rows) {
                chosen.set(Some(id));
            }
        });
    };
    screen.run(vec![], &mut draw);
    let out = screen.run(vec![], &mut draw);
    screen.picture("file-menu", &out, 2.0)?;
    let texts = painted(&out);
    let at = |text: &str| {
        texts
            .iter()
            .find(|(t, ..)| t == text)
            .map(|(_, rect, ..)| *rect)
            .ok_or(format!("{text} is not painted"))
    };
    let save = at("Save")?;
    let save_as = at("Save as…")?;
    let open = at("Open…")?;
    let new = at("New project")?;
    let pitch = open.center().y - new.center().y;
    assert!((20.0..=30.0).contains(&pitch), "row pitch {pitch}");
    let key = at(&Chord::cmd(K::S).shown())?;
    assert!(
        (key.center().y - save.center().y).abs() < 1.0,
        "the key is on its row"
    );
    assert!(
        key.left() > save.right() + 100.0,
        "the key is right-aligned"
    );
    at(&Chord::cmd(K::S).with_shift().shown())?;
    at(&Chord::cmd(K::Comma).shown())?;
    // A separator between Save as… and Export: a wider gap.
    let export = at("Export to Solidcraft…")?;
    assert!(export.center().y - save_as.center().y > pitch + 5.0);
    let menu_width = texts
        .iter()
        .map(|(_, rect, ..)| rect.right())
        .fold(0.0_f32, f32::max);
    assert!(menu_width <= MENU_WIDTH + 20.0, "{menu_width}");
    assert!(
        !texts.iter().any(|(t, ..)| t.contains("parametric")),
        "no prose between items"
    );
    // A click on Save chooses file.save; a disabled row chooses nothing.
    screen.run(
        vec![
            egui::Event::PointerMoved(save.center()),
            egui::Event::PointerButton {
                pos: save.center(),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        &mut draw,
    );
    screen.run(
        vec![egui::Event::PointerButton {
            pos: save.center(),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
        &mut draw,
    );
    assert_eq!(chosen.take(), Some("file.save"));
    let pdf = at("Export schematic PDF…")?;
    for pressed in [true, false] {
        screen.run(
            vec![
                egui::Event::PointerMoved(pdf.center()),
                egui::Event::PointerButton {
                    pos: pdf.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            &mut draw,
        );
    }
    assert_eq!(chosen.get(), None, "a disabled row does nothing");
    Ok(())
}
