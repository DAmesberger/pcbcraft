//! The menu bar, as Solidcraft's (`apps/solidcraft/src/native_menu.rs`):
//! which registry command each item runs, where it sits, and whether it
//! is enabled and takes its key now. The macOS menu bar
//! (`native_menu.rs`) is built from it, and the in-window File menu lists
//! [`IN_WINDOW_FILE`] with the same states.
//!
//! An item takes its command's effective key as its key equivalent only
//! when the key is a ⌘ chord and runs that command where the user is
//! ([`Registry::resolve`]): a plain key (F, Delete) would be taken from
//! text fields by the system menu, and a key a tool mode takes for itself
//! stays with the tool. While Settings records a key, no item takes one.
//!
//! A chosen item ([`choose`]) runs its registry command, or, for the keys
//! a focused field owns (Undo, Redo, Cut, Copy, Paste and Select All while
//! typing) and the canvas's own Delete, hands the key back to egui:
//! the same input the field or canvas would have had without the menu,
//! never a second run of a command.
#![cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "the system menu bar is macOS-only; elsewhere only the in-window File menu reads it"
    )
)]
use crate::commands::{Action, AppCommand, Chord, Command, Context, Registry};

/// The system's own menu items (AppKit's), placed as macOS apps place them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    Fullscreen,
    BringAllToFront,
}

/// One menu entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A registry command under its menu label.
    Command(&'static str, &'static str),
    /// Delete: the key of the active context's delete command, handed to
    /// the canvas that owns it.
    Delete,
    /// File › Open Recent: a submenu of the recent projects, then
    /// [`CLEAR_RECENT`]. The menus draw it from the recent list.
    Recent,
    Separator,
    System(System),
}

/// The Open Recent submenu's title.
pub const RECENT_TITLE: &str = "Open Recent";
/// The last item of Open Recent (macOS's own wording).
pub const CLEAR_RECENT: Entry = Entry::Command("file.clear-recent", "Clear Menu");

use Entry::{Command as C, Delete, Separator as Sep};

/// The app menu.
pub const APP: &[Entry] = &[
    Entry::System(System::About),
    Sep,
    C("app.settings", "Settings…"),
    Sep,
    Entry::System(System::Services),
    Sep,
    Entry::System(System::Hide),
    Entry::System(System::HideOthers),
    Entry::System(System::ShowAll),
    Sep,
    C("app.quit", concat!("Quit ", pcbcraft_core::product_name!())),
];

pub const FILE: &[Entry] = &[
    C("file.new", "New Project"),
    C("file.open", "Open…"),
    Entry::Recent,
    Sep,
    C("file.save", "Save"),
    C("file.save-as", "Save As…"),
    Sep,
    C("file.export", "Export to Solidcraft…"),
    C("file.export-schematic-pdf", "Export Schematic PDF…"),
    Sep,
    C("library.sources", "Library Sources…"),
    Sep,
    C("window.close", "Close Window"),
];

pub const EDIT: &[Entry] = &[
    C("edit.undo", "Undo"),
    C("edit.redo", "Redo"),
    Sep,
    C("edit.cut", "Cut"),
    C("edit.copy", "Copy"),
    C("edit.paste", "Paste"),
    Delete,
    C("edit.select-all", "Select All"),
    Sep,
    C("pcb.align-left", "Align Left Edges"),
    C("pcb.align-right", "Align Right Edges"),
    C("pcb.align-top", "Align Top Edges"),
    C("pcb.align-bottom", "Align Bottom Edges"),
    C("pcb.align-centres-x", "Align Horizontal Centres"),
    C("pcb.align-centres-y", "Align Vertical Centres"),
    C("pcb.distribute-x", "Distribute Horizontally"),
    C("pcb.distribute-y", "Distribute Vertically"),
];

pub const VIEW: &[Entry] = &[
    C("workspace.component", "Component"),
    C("workspace.schematic", "Schematic"),
    C("workspace.pcb", "PCB"),
    Sep,
    C("pcb.toggle-3d", "Switch Layout and 3D"),
    C("view.fit", "Fit View"),
    Sep,
    C(BROWSER, "Show Browser"),
    C(ISSUES, "Show Issues"),
    Sep,
    C("app.palette", "Command Palette…"),
];

pub const WINDOW: &[Entry] = &[
    Entry::System(System::Minimize),
    Entry::System(System::Zoom),
    Entry::System(System::Fullscreen),
    Sep,
    C("view.messages", "Messages…"),
    Sep,
    Entry::System(System::BringAllToFront),
];

pub const HELP: &[Entry] = &[
    C("app.shortcuts", "Keyboard Shortcuts"),
    C("app.connect-assistants", "Connect AI Assistants…"),
];

/// The macOS menu bar, left to right.
pub const BAR: &[(&str, &[Entry])] = &[
    (pcbcraft_core::product::NAME, APP),
    ("File", FILE),
    ("Edit", EDIT),
    ("View", VIEW),
    ("Window", WINDOW),
    ("Help", HELP),
];

/// The in-window File menu, as Solidcraft's: the File items, then the
/// app's own windows and Quit (Close Window is Quit in a one-window app).
pub const IN_WINDOW_FILE: &[Entry] = &[
    C("file.new", "New project"),
    C("file.open", "Open…"),
    Entry::Recent,
    Sep,
    C("file.save", "Save"),
    C("file.save-as", "Save as…"),
    Sep,
    C("file.export", "Export to Solidcraft…"),
    C("file.export-schematic-pdf", "Export schematic PDF…"),
    Sep,
    C("library.sources", "Library sources…"),
    Sep,
    C("app.settings", "Settings…"),
    C("app.shortcuts", "Keyboard shortcuts…"),
    C("app.connect-assistants", "Connect AI assistants…"),
    C("view.messages", "Messages…"),
    Sep,
    C("app.quit", "Quit"),
];

const BROWSER: &str = "view.browser";
const ISSUES: &str = "view.issues";

/// What the menus depend on, read once per frame.
pub struct Now<'a> {
    pub registry: &'a Registry,
    /// The keyboard contexts in force, most specific first.
    pub active: &'a [Context],
    /// A text field has the keyboard.
    pub typing: bool,
    /// Settings › Keyboard is recording a key: every key goes to it.
    pub recording: bool,
    pub browser_shown: bool,
    pub issues_shown: bool,
    /// Whether the command can run now (the dispatcher's availability).
    pub available: &'a dyn Fn(&Command) -> bool,
}

/// An item as the menu shows it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub label: String,
    pub enabled: bool,
    /// The key equivalent: the key the menu takes.
    pub key: Option<Chord>,
    /// The key as written beside the item (the in-window menu writes plain
    /// keys too).
    pub shown: Option<String>,
}

/// What a chosen item does.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// Run the registry command (after a focused field commits).
    Run(String),
    /// Give egui these events: the key a field or canvas owns.
    Inject(Vec<egui::Event>),
    /// Paste the clipboard's text into the focused field.
    Paste,
    /// Open this recent project (File › Open Recent).
    OpenRecent(std::path::PathBuf),
    Nothing,
}

/// The clipboard keys a focused field owns, and the event each gives it.
fn text_edit(id: &str) -> Option<TextEdit> {
    match id {
        "edit.cut" => Some(TextEdit::Cut),
        "edit.copy" => Some(TextEdit::Copy),
        "edit.paste" => Some(TextEdit::Paste),
        "edit.select-all" => Some(TextEdit::SelectAll),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum TextEdit {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// Cut, Copy, Paste or Select All: while typing they are the field's own;
/// otherwise they run where the command is available.
fn clipboard(command: &Command) -> bool {
    matches!(
        command.action,
        Action::App(AppCommand::Cut | AppCommand::Copy | AppCommand::Paste | AppCommand::SelectAll)
    )
}

/// The ⌘ key egui-winit turns into a clipboard event instead of a key press.
pub fn clipboard_chord(event: &egui::Event) -> Option<Chord> {
    match event {
        egui::Event::Cut => Some(Chord::cmd(egui::Key::X)),
        egui::Event::Copy => Some(Chord::cmd(egui::Key::C)),
        egui::Event::Paste(_) => Some(Chord::cmd(egui::Key::V)),
        _ => None,
    }
}

/// Undo or Redo: while typing they are the field's own.
fn history(command: &Command) -> Option<bool> {
    match command.action {
        Action::App(AppCommand::Undo) => Some(false),
        Action::App(AppCommand::Redo) => Some(true),
        _ => None,
    }
}

/// The key `command` takes in the menu: its effective chord when that is
/// a ⌘ chord the registry resolves to it here.
fn key_equivalent(now: &Now, command: &Command) -> Option<Chord> {
    if now.recording {
        return None;
    }
    let chord = now.registry.chord(command)?;
    (chord.command
        && now
            .registry
            .resolve(chord, now.active)
            .is_some_and(|c| c.id == command.id))
    .then_some(chord)
}

/// The command Delete hands its key to here, and that key.
fn delete_target<'r>(now: &Now<'r>) -> Option<(&'r Command, Chord)> {
    let registry = now.registry;
    now.active.iter().find_map(|context| {
        let command = registry
            .commands()
            .iter()
            .find(|c| c.context == *context && c.id.ends_with(".delete"))?;
        let chord = registry.chord(command)?;
        registry
            .resolve(chord, now.active)
            .is_some_and(|c| c.id == command.id)
            .then_some((command, chord))
    })
}

/// The label of `id`'s item: the Browser and Issues toggles say what they
/// do now.
fn label(now: &Now, id: &str, label: &str) -> String {
    let toggle = |shown: bool, what: &str| {
        if shown {
            format!("Hide {what}")
        } else {
            format!("Show {what}")
        }
    };
    match id {
        BROWSER => toggle(now.browser_shown, "Browser"),
        ISSUES => toggle(now.issues_shown, "Issues"),
        _ => label.to_owned(),
    }
}

/// The state of `entry` now; `None` for separators and system items.
pub fn state(now: &Now, entry: Entry) -> Option<State> {
    match entry {
        Entry::Command(id, text) => {
            let label = label(now, id, text);
            let Some(command) = now.registry.get(id) else {
                return Some(State {
                    label,
                    enabled: false,
                    key: None,
                    shown: None,
                });
            };
            let enabled = if clipboard(command) {
                now.typing || (now.available)(command)
            } else if history(command).is_some() && now.typing {
                true
            } else {
                (now.available)(command)
            };
            Some(State {
                label,
                enabled,
                key: key_equivalent(now, command),
                shown: now.registry.menu_shortcut(command),
            })
        }
        Entry::Delete => {
            let target = delete_target(now);
            Some(State {
                label: "Delete".to_owned(),
                enabled: target.is_some_and(|(command, _)| (now.available)(command)) && !now.typing,
                key: None,
                shown: target.map(|(_, chord)| chord.shown()),
            })
        }
        Entry::Recent | Entry::Separator | Entry::System(_) => None,
    }
}

/// A row of an in-window menu: its command, icon and state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: &'static str,
    pub icon: &'static str,
    pub state: State,
}

/// The rows of `entries` now; `None` is a separator.
pub fn rows(now: &Now, entries: &[Entry]) -> Vec<Option<Row>> {
    entries
        .iter()
        .filter_map(|&entry| match entry {
            Entry::Separator => Some(None),
            Entry::Command(id, _) => state(now, entry).map(|state| {
                Some(Row {
                    id,
                    icon: now.registry.get(id).map_or("", |c| c.icon),
                    state,
                })
            }),
            Entry::Delete | Entry::Recent | Entry::System(_) => None,
        })
        .collect()
}

/// The in-window menu's width (Solidcraft's list menus are 250).
pub const MENU_WIDTH: f32 = 250.0;

/// Draw `rows` as Solidcraft's menu rows: icon, label, the key in grey on
/// the right, thin separators between groups, a disabled row dimmed.
/// Returns the command of the row clicked.
pub fn show_rows(ui: &mut egui::Ui, rows: &[Option<Row>]) -> Option<&'static str> {
    use solidcraft_ui_egui::ui_kit::Item;
    use solidcraft_ui_egui::ui_kit::menu::list_row;
    ui.set_width(MENU_WIDTH);
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut chosen = None;
    for row in rows {
        let item = match row {
            None => Item::sep(),
            Some(row) => Item::action(row.id, &row.state.label, row.icon)
                .key(row.state.shown.as_deref().unwrap_or_default())
                .on_if(row.state.enabled),
        };
        if list_row(ui, &item).is_some()
            && let Some(row) = row
        {
            chosen = Some(row.id);
        }
    }
    chosen
}

/// A key press as egui gets it from the keyboard.
fn press(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    let modifiers = if modifiers.command && cfg!(target_os = "macos") {
        modifiers | egui::Modifiers::MAC_CMD
    } else {
        modifiers
    };
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// What choosing `entry` does now. A disabled item does nothing.
pub fn choose(now: &Now, entry: Entry) -> Choice {
    if !state(now, entry).is_some_and(|s| s.enabled) {
        return Choice::Nothing;
    }
    match entry {
        Entry::Command(id, _) => {
            let field = now.typing || !now.registry.get(id).is_some_and(clipboard);
            if let Some(edit) = text_edit(id).filter(|_| field) {
                return match edit {
                    TextEdit::Cut => Choice::Inject(vec![egui::Event::Cut]),
                    TextEdit::Copy => Choice::Inject(vec![egui::Event::Copy]),
                    TextEdit::Paste => Choice::Paste,
                    TextEdit::SelectAll => {
                        Choice::Inject(vec![press(egui::Key::A, egui::Modifiers::COMMAND)])
                    }
                };
            }
            match now.registry.get(id) {
                // The field's own undo keys (egui's, not the registry's).
                Some(command) if now.typing && history(command).is_some() => {
                    let redo = history(command) == Some(true);
                    let mut modifiers = egui::Modifiers::COMMAND;
                    if redo {
                        modifiers |= egui::Modifiers::SHIFT;
                    }
                    Choice::Inject(vec![press(egui::Key::Z, modifiers)])
                }
                Some(_) => Choice::Run(id.to_owned()),
                None => Choice::Nothing,
            }
        }
        Entry::Delete => match delete_target(now) {
            Some((_, chord)) => Choice::Inject(vec![press(chord.key, chord.modifiers())]),
            None => Choice::Nothing,
        },
        Entry::Recent | Entry::Separator | Entry::System(_) => Choice::Nothing,
    }
}

/// Deliver a chosen item into this frame's input: injected events join
/// it; a command to run is queued in `runs`, and a press of its key in
/// the same input is dropped, so the dispatcher cannot run it a second
/// time. `paste` reads the clipboard.
pub fn deliver(
    choice: Choice,
    registry: &Registry,
    raw: &mut egui::RawInput,
    runs: &mut Vec<String>,
    paste: impl FnOnce() -> Option<String>,
) {
    /// The most commands queued at once.
    const MAX_RUNS: usize = 16;
    match choice {
        Choice::Run(id) => {
            if let Some(chord) = registry.get(&id).and_then(|c| registry.chord(c)) {
                raw.events.retain(|event| {
                    !matches!(event, egui::Event::Key { key, pressed: true, modifiers, .. }
                        if *key == chord.key && chord.matches(*modifiers))
                        && clipboard_chord(event) != Some(chord)
                });
            }
            if runs.len() < MAX_RUNS {
                runs.push(id);
            }
        }
        Choice::Inject(events) => raw.events.extend(events),
        Choice::Paste => {
            if let Some(text) = paste().filter(|t| !t.is_empty()) {
                raw.events.push(egui::Event::Paste(text));
            }
        }
        // The app opens it (`App::open_recent`) before the frame runs.
        Choice::OpenRecent(_) | Choice::Nothing => {}
    }
}

#[cfg(test)]
#[path = "menu_bar_tests.rs"]
mod tests;
