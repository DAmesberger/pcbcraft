//! The list keys (list-system spec §4) as registry commands of the
//! "Lists" context, so they show in the shortcut sheet and stay
//! configurable; [`inputs`] reads them for the focused list through
//! `crate::keys`, never by naming a key.
use super::nav::Input;
use crate::commands::KeyDecl;

pub const LIST_KEYS: &[KeyDecl] = &[
    KeyDecl::new("list.up", "Up", "Previous row"),
    KeyDecl::new("list.down", "Down", "Next row"),
    KeyDecl::new("list.extend-up", "Shift+Up", "Extend the selection up"),
    KeyDecl::new("list.extend-down", "Shift+Down", "Extend the selection down"),
    KeyDecl::new("list.cursor-up", "Cmd+Up", "Move the cursor up only"),
    KeyDecl::new("list.cursor-down", "Cmd+Down", "Move the cursor down only"),
    KeyDecl::new("list.home", "Home", "First row"),
    KeyDecl::new("list.end", "End", "Last row"),
    KeyDecl::new("list.extend-home", "Shift+Home", "Extend to the first row"),
    KeyDecl::new("list.extend-end", "Shift+End", "Extend to the last row"),
    KeyDecl::new("list.page-up", "PageUp", "A page up"),
    KeyDecl::new("list.page-down", "PageDown", "A page down"),
    KeyDecl::new("list.fold", "Left", "Fold, or go to the parent"),
    KeyDecl::new("list.unfold", "Right", "Unfold, or go to the first child"),
    KeyDecl::new(
        "list.toggle",
        "Space",
        "Add or remove the row; fold a group",
    ),
    KeyDecl::new(
        "list.activate",
        "Enter, F2",
        "Edit the value, or locate the object",
    ),
    KeyDecl::new(
        "list.escape",
        "Esc",
        "Cancel the edit, then clear the filter, then the selection",
    ),
    KeyDecl::new("list.select-all", "Cmd+A", "Select every shown row"),
    KeyDecl::new("list.filter", "Slash, Cmd+F", "Filter this list"),
    KeyDecl::new("list.move-up", "Alt+Up", "Move the row up"),
    KeyDecl::new("list.move-down", "Alt+Down", "Move the row down"),
    KeyDecl::new(
        "list.delete",
        "Backspace, Delete",
        "Delete the selected rows",
    ),
];

/// The list inputs pressed this frame, taking their key events. `now` is
/// the frame time (type-ahead); typed letters and digits are type-ahead.
pub fn inputs(ctx: &egui::Context, now: f64) -> Vec<Input> {
    use crate::keys::pressed_focused as on;
    let mut out = Vec::new();
    let plain = |extend, cursor_only| (extend, cursor_only);
    for (id, input) in [
        ("list.move-up", Input::MoveUp),
        ("list.move-down", Input::MoveDown),
        ("list.extend-up", up(plain(true, false))),
        ("list.extend-down", down(plain(true, false))),
        ("list.cursor-up", up(plain(false, true))),
        ("list.cursor-down", down(plain(false, true))),
        ("list.up", up(plain(false, false))),
        ("list.down", down(plain(false, false))),
        ("list.extend-home", Input::Home { extend: true }),
        ("list.extend-end", Input::End { extend: true }),
        ("list.home", Input::Home { extend: false }),
        ("list.end", Input::End { extend: false }),
        ("list.page-up", Input::PageUp { extend: false }),
        ("list.page-down", Input::PageDown { extend: false }),
        ("list.fold", Input::Left),
        ("list.unfold", Input::Right),
        ("list.toggle", Input::Space),
        ("list.activate", Input::Activate),
        ("list.escape", Input::Escape),
        ("list.select-all", Input::SelectAll),
        ("list.filter", Input::FocusFilter),
        ("list.delete", Input::Delete),
    ] {
        while on(ctx, id) {
            out.push(input.clone());
        }
    }
    let typed: String = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    });
    // Space and `/` arrive as text too; they were keys.
    let typed: String = typed
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if !typed.is_empty() && !ctx.text_edit_focused() {
        out.push(Input::Type { text: typed, now });
    }
    out
}

fn up((extend, cursor_only): (bool, bool)) -> Input {
    Input::Up {
        extend,
        cursor_only,
    }
}

fn down((extend, cursor_only): (bool, bool)) -> Input {
    Input::Down {
        extend,
        cursor_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{Chord, Registry};

    /// Every list key reads as a key name, so none silently has no key.
    #[test]
    fn every_list_key_parses() {
        let registry = Registry::standard();
        for decl in LIST_KEYS {
            let command = registry.get(decl.id);
            assert!(command.is_some(), "{} is registered", decl.id);
            let chords = command.map(|c| registry.chords(c)).unwrap_or_default();
            assert!(!chords.is_empty(), "{} ({}) has a key", decl.id, decl.keys);
        }
        assert!(Chord::parse("Alt+Up").is_some());
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

    /// ⌥↓ reorders rather than moving; ⇧↓ extends; letters type ahead.
    #[test]
    fn modifiers_pick_the_input() {
        let ctx = egui::Context::default();
        let mut got = Vec::new();
        let events = vec![
            key(egui::Key::ArrowDown, egui::Modifiers::ALT),
            key(egui::Key::ArrowDown, egui::Modifiers::SHIFT),
            key(egui::Key::ArrowDown, egui::Modifiers::NONE),
            egui::Event::Text("r1".into()),
        ];
        let _ = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| got = inputs(ui.ctx(), 1.0),
        );
        assert_eq!(
            got,
            vec![
                Input::MoveDown,
                down((true, false)),
                down((false, false)),
                Input::Type {
                    text: "r1".into(),
                    now: 1.0
                }
            ]
        );
    }
}
