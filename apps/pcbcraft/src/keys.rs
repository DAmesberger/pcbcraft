//! The one way an editor reads a bound key: it asks for a registry command
//! by id ([`pressed`]) and never names the key itself. The app publishes
//! its effective keymap (defaults plus the user's changes) once per change
//! ([`publish`]); every canvas, card and overlay reads that same keymap,
//! so a rebinding changes what a key does and what every label says.
//!
//! Gesture keys stay with the canvas that owns the gesture (it decides
//! when they apply: the Route tool's keys only while routing), but which
//! physical key that is comes from the registry. `cargo xtask raw-keys`
//! rejects new raw key reads outside this layer.
use crate::commands::{Chord, Command, Registry};
use std::sync::{Arc, OnceLock};

/// The published keymap, in the context's temporary data.
#[derive(Clone)]
struct Published(Arc<Registry>);

fn slot() -> egui::Id {
    egui::Id::new("pcbcraft-effective-keymap")
}

/// The default keymap, for a context nothing was published to (an
/// editor driven headlessly by its own tests).
fn defaults() -> Arc<Registry> {
    static DEFAULTS: OnceLock<Arc<Registry>> = OnceLock::new();
    Arc::clone(DEFAULTS.get_or_init(|| Arc::new(Registry::standard())))
}

/// Make `registry` the keymap every reader of `ctx` sees.
pub fn publish(ctx: &egui::Context, registry: &Arc<Registry>) {
    ctx.data_mut(|d| d.insert_temp(slot(), Published(Arc::clone(registry))));
}

/// The effective keymap of `ctx`.
pub fn registry(ctx: &egui::Context) -> Arc<Registry> {
    ctx.data(|d| d.get_temp::<Published>(slot()))
        .map_or_else(defaults, |p| p.0)
}

/// Whether a key event of this frame is one of `command`'s keys; takes the
/// event if so, so no one else acts on it. A plain key never counts while
/// a text field has the keyboard. Key repeats count (a held Backspace
/// keeps unwinding).
fn take(ctx: &egui::Context, registry: &Registry, command: &Command, repeats: bool) -> bool {
    if !ctx.input(|i| i.events.iter().any(is_press)) {
        return false;
    }
    let typing = ctx.egui_wants_keyboard_input();
    ctx.input_mut(|i| {
        let at = i.events.iter().position(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                repeat,
                ..
            } => {
                let chord = Chord {
                    key: *key,
                    command: modifiers.command,
                    shift: modifiers.shift,
                    alt: modifiers.alt,
                };
                (repeats || !*repeat)
                    && chord.matches(*modifiers)
                    && (chord.command || !typing)
                    && registry.takes(command, chord)
            }
            _ => false,
        });
        at.map(|at| i.events.remove(at)).is_some()
    })
}

fn is_press(event: &egui::Event) -> bool {
    matches!(event, egui::Event::Key { pressed: true, .. })
}

/// Whether the command `id`'s key was pressed this frame (taking it).
pub fn pressed(ctx: &egui::Context, id: &str) -> bool {
    let registry = registry(ctx);
    registry
        .get(id)
        .is_some_and(|command| take(ctx, &registry, command, true))
}

/// [`pressed`] without key repeats: a held key acts once (Space turns
/// the next pad once, and holding it pans).
pub fn pressed_once(ctx: &egui::Context, id: &str) -> bool {
    let registry = registry(ctx);
    registry
        .get(id)
        .is_some_and(|command| take(ctx, &registry, command, false))
}

/// [`pressed`] for the command `prefix.name` (a tool table's `symbol` and
/// tool id), without building the id.
pub fn pressed_tool(ctx: &egui::Context, prefix: &str, name: &str) -> bool {
    let registry = registry(ctx);
    registry
        .get_parts(prefix, name)
        .is_some_and(|command| take(ctx, &registry, command, true))
}

/// The command `id`'s key as shown (`⇧W`), from the effective keymap.
pub fn shown(ctx: &egui::Context, id: &str) -> Option<String> {
    let registry = registry(ctx);
    registry.get(id).and_then(|c| registry.shortcut(c))
}

/// `text (key)` when the command `id` has a key, else `text`.
pub fn labelled(ctx: &egui::Context, text: &str, id: &str) -> String {
    match shown(ctx, id) {
        Some(key) => format!("{text} ({key})"),
        None => text.to_owned(),
    }
}

/// A key table's rows as shown now: (key, what it does). The Route
/// tool's "via to layer N" rows collapse to one `1–9` row while they keep
/// their default digits.
pub fn rows(ctx: &egui::Context, table: &[crate::commands::KeyDecl]) -> Vec<(String, String)> {
    let registry = registry(ctx);
    let mut out: Vec<(String, String)> = Vec::with_capacity(table.len());
    let mut digits = 0;
    for decl in table {
        let Some(command) = registry.get(decl.id) else {
            continue;
        };
        let key = registry.shortcut(command).unwrap_or_else(|| "—".to_owned());
        let digit = decl.id.starts_with("route.layer-") && !registry.is_custom(decl.id);
        if digit {
            digits += 1;
            if digits > 1 {
                continue;
            }
            out.push((
                "1–9".to_owned(),
                "Via to that layer of the stackup".to_owned(),
            ));
            continue;
        }
        out.push((key, decl.title.to_owned()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Chord;
    use egui::Key as K;

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

    /// The published keymap decides which key a command reads; the default
    /// key does nothing once rebound, and a read takes the event.
    #[test]
    fn a_rebound_key_moves_the_command() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut registry = Registry::standard();
        registry.rebind("route.width-next", Some(Chord::key(K::Q)))?;
        publish(&ctx, &Arc::new(registry));
        let mut seen = (false, false, false);
        ctx.run_ui(press(K::W, egui::Modifiers::NONE), |ui| {
            seen.0 = pressed(ui.ctx(), "route.width-next");
        })
        .textures_delta
        .clear();
        ctx.run_ui(press(K::Q, egui::Modifiers::NONE), |ui| {
            seen.1 = pressed(ui.ctx(), "route.width-next");
            seen.2 = pressed(ui.ctx(), "route.width-next");
        })
        .textures_delta
        .clear();
        assert_eq!(seen, (false, true, false), "W is free; Q taken once");
        assert_eq!(
            shown(&ctx, "route.width-next").as_deref(),
            Some(Chord::key(K::Q).shown().as_str())
        );
        Ok(())
    }

    /// Modifiers match exactly: ⇧W is the previous width, never the next.
    #[test]
    fn modifiers_must_match() {
        let ctx = egui::Context::default();
        let mut seen = (true, false);
        ctx.run_ui(press(K::W, egui::Modifiers::SHIFT), |ui| {
            seen = (
                pressed(ui.ctx(), "route.width-next"),
                pressed(ui.ctx(), "route.width-previous"),
            );
        })
        .textures_delta
        .clear();
        assert_eq!(seen, (false, true));
    }
}
