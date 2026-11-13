//! The user's keymap between the registry and the per-user settings file
//! (`sparksmith_io::settings::KeyboardSettings`): saved changes are applied
//! over the defaults at startup, and the registry's changes are written
//! back after every edit. A saved entry the app cannot apply (an unknown
//! command, a key name it cannot read, a key another command holds) is
//! reported and kept in the file, never dropped.
use crate::commands::{Chord, Registry};
use sparksmith_io::settings::KeyBinding;

/// A saved change that could not be applied, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unapplied {
    pub binding: KeyBinding,
    pub reason: String,
}

/// Apply `bindings` over `registry`'s defaults. Every changed command is
/// freed first, so a saved swap (A takes B's key and B takes A's) applies
/// in any order; a binding that still conflicts keeps its default.
pub fn apply(registry: &mut Registry, bindings: &[KeyBinding]) -> Vec<Unapplied> {
    registry.reset_all();
    let mut unapplied = Vec::new();
    let mut parsed = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let refuse = |reason: String| Unapplied {
            binding: binding.clone(),
            reason,
        };
        let Some(command) = registry.get(&binding.command) else {
            unapplied.push(refuse(format!("No command {}", binding.command)));
            continue;
        };
        if !Registry::bindable(command) {
            unapplied.push(refuse(format!("{} is not a key", command.title)));
            continue;
        }
        let chord = match binding.chord.as_deref() {
            None => None,
            Some(text) => match Chord::parse(text) {
                Some(chord) => Some(chord),
                None => {
                    unapplied.push(refuse(format!("{text} is not a key name")));
                    continue;
                }
            },
        };
        parsed.push((binding, chord));
    }
    for (binding, _) in &parsed {
        // Freeing never conflicts.
        let _ = registry.rebind(&binding.command, None);
    }
    for (binding, chord) in parsed {
        if let Err(reason) = registry.rebind(&binding.command, chord) {
            registry.reset(&binding.command);
            unapplied.push(Unapplied {
                binding: binding.clone(),
                reason,
            });
        }
    }
    unapplied
}

/// The registry's changes as saved entries, in id order, followed by the
/// saved entries that could not be applied (kept for the user to fix).
pub fn bindings(registry: &Registry, unapplied: &[Unapplied]) -> Vec<KeyBinding> {
    let mut out: Vec<KeyBinding> = registry
        .overrides()
        .iter()
        .map(|(id, chord)| KeyBinding {
            command: id.clone(),
            chord: chord.map(Chord::name),
        })
        .collect();
    for kept in unapplied {
        if !out.iter().any(|b| b.command == kept.binding.command) {
            out.push(kept.binding.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key as K;

    fn binding(command: &str, chord: Option<&str>) -> KeyBinding {
        KeyBinding {
            command: command.into(),
            chord: chord.map(str::to_owned),
        }
    }

    /// Saved names round-trip through the registry: every default chord
    /// reads back from the name the settings store.
    #[test]
    fn chord_names_round_trip() {
        let registry = Registry::standard();
        for command in registry.commands() {
            for chord in registry.chords(command) {
                assert_eq!(Chord::parse(&chord.name()), Some(chord), "{}", command.id);
            }
        }
    }

    /// A saved swap applies whatever the order; an unknown id, an unreadable
    /// key, a row that is not a key and a real conflict are reported, keep
    /// their defaults and stay in what is written back.
    #[test]
    fn saved_changes_apply_and_bad_ones_are_reported_and_kept() {
        let mut registry = Registry::standard();
        let saved = vec![
            binding("pcb.tune", Some("B")),
            binding("pcb.board-shape", Some("T")),
            binding("route.width-next", None),
            binding("no.such", Some("Q")),
            binding("pcb.via", Some("Ctrl+Nope")),
            binding("route.click", Some("K")),
            binding("pcb.fill", Some("H")),
        ];
        let unapplied = apply(&mut registry, &saved);
        let chord = |id: &str| registry.get(id).and_then(|c| registry.chord(c));
        assert_eq!(chord("pcb.tune"), Some(Chord::key(K::B)));
        assert_eq!(chord("pcb.board-shape"), Some(Chord::key(K::T)));
        assert_eq!(chord("route.width-next"), None);
        assert_eq!(chord("pcb.via"), Some(Chord::key(K::V)), "default kept");
        assert_eq!(chord("pcb.fill"), Some(Chord::key(K::P)), "H is Highlight");
        let refused: Vec<&str> = unapplied
            .iter()
            .map(|u| u.binding.command.as_str())
            .collect();
        assert_eq!(
            refused,
            vec!["no.such", "pcb.via", "route.click", "pcb.fill"]
        );
        assert!(unapplied[3].reason.contains("Highlight"));
        assert_eq!(registry.conflicts(), vec![]);
        let written = bindings(&registry, &unapplied);
        for kept in &saved {
            assert!(
                written.iter().any(|b| b.command == kept.command),
                "{} dropped",
                kept.command
            );
        }
        // Reading what was written gives the same keymap.
        let mut again = Registry::standard();
        let reported = apply(&mut again, &written);
        assert_eq!(reported.len(), unapplied.len());
        assert_eq!(again.overrides(), registry.overrides());
    }
}
