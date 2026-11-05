//! The macOS app menu, owned on eframe's main thread, as in Solidcraft's
//! `apps/solidcraft/src/native_menu.rs`.
//!
//! It replaces winit's default menu, whose Quit (⌘Q) sends AppKit's
//! `terminate:` and ends the app at once, without asking about unsaved
//! work. This menu's Quit runs the registry's `app.quit` instead, the same
//! request as the window's close button. Its key equivalent is that
//! command's effective key, so a rebinding in Settings › Keyboard moves it.
//! Every other key still reaches the app's own dispatcher: the menu has no
//! File or Edit items of its own.
use crate::commands::{Chord, Registry};
use muda::{
    AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
    accelerator::{Accelerator, Code, Modifiers},
};
use std::sync::mpsc::{Receiver, sync_channel};

/// The registry command the menu's Quit runs.
pub const QUIT: &str = "app.quit";

pub struct NativeMenu {
    menu: Menu,
    quit: MenuItem,
    /// The key the Quit item shows and takes.
    quit_chord: Option<Chord>,
    events: Receiver<MenuEvent>,
}

impl NativeMenu {
    pub fn new(ctx: &egui::Context, registry: &Registry) -> muda::Result<Self> {
        let menu = Menu::new();
        let application = Submenu::new("Pcbcraft", true);
        let quit_chord = effective(registry, QUIT);
        let quit = MenuItem::new("Quit Pcbcraft", true, quit_chord.and_then(accelerator));
        application.append_items(&[
            &PredefinedMenuItem::about(
                Some("About Pcbcraft"),
                Some(AboutMetadata {
                    name: Some("Pcbcraft".into()),
                    version: Some(env!("CARGO_PKG_VERSION").into()),
                    ..Default::default()
                }),
            ),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(Some("Hide Pcbcraft")),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;
        menu.append(&application)?;
        let (sender, events) = sync_channel(16);
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = sender.try_send(event);
            ctx.request_repaint();
        }));
        menu.init_for_nsapp();
        Ok(Self {
            menu,
            quit,
            quit_chord,
            events,
        })
    }

    /// The registry commands chosen in the menu since the last frame. Keeps
    /// the Quit item's key in step with `registry`.
    pub fn take_commands(&mut self, registry: &Registry) -> Vec<&'static str> {
        let chord = effective(registry, QUIT);
        if chord != self.quit_chord
            && self
                .quit
                .set_accelerator(chord.and_then(accelerator))
                .is_ok()
        {
            self.quit_chord = chord;
        }
        let quit = self.quit.id().clone();
        self.events
            .try_iter()
            .take(16)
            .filter(|event| event.id == quit)
            .map(|_| QUIT)
            .collect()
    }
}

impl Drop for NativeMenu {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        self.menu.remove_for_nsapp();
    }
}

/// The command `id`'s effective key.
fn effective(registry: &Registry, id: &str) -> Option<Chord> {
    registry.get(id).and_then(|command| registry.chord(command))
}

/// A chord as a menu key equivalent; `None` for a key the menu cannot
/// show, which then still reaches the app's dispatcher.
pub fn accelerator(chord: Chord) -> Option<Accelerator> {
    use egui::Key as K;
    let code = match chord.key {
        K::A => Code::KeyA,
        K::B => Code::KeyB,
        K::C => Code::KeyC,
        K::D => Code::KeyD,
        K::E => Code::KeyE,
        K::F => Code::KeyF,
        K::G => Code::KeyG,
        K::H => Code::KeyH,
        K::I => Code::KeyI,
        K::J => Code::KeyJ,
        K::K => Code::KeyK,
        K::L => Code::KeyL,
        K::M => Code::KeyM,
        K::N => Code::KeyN,
        K::O => Code::KeyO,
        K::P => Code::KeyP,
        K::Q => Code::KeyQ,
        K::R => Code::KeyR,
        K::S => Code::KeyS,
        K::T => Code::KeyT,
        K::U => Code::KeyU,
        K::V => Code::KeyV,
        K::W => Code::KeyW,
        K::X => Code::KeyX,
        K::Y => Code::KeyY,
        K::Z => Code::KeyZ,
        K::Num0 => Code::Digit0,
        K::Num1 => Code::Digit1,
        K::Num2 => Code::Digit2,
        K::Num3 => Code::Digit3,
        K::Num4 => Code::Digit4,
        K::Num5 => Code::Digit5,
        K::Num6 => Code::Digit6,
        K::Num7 => Code::Digit7,
        K::Num8 => Code::Digit8,
        K::Num9 => Code::Digit9,
        _ => return None,
    };
    let mut modifiers = Modifiers::empty();
    if chord.command {
        modifiers |= Modifiers::META;
    }
    if chord.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if chord.alt {
        modifiers |= Modifiers::ALT;
    }
    Some(Accelerator::new(modifiers, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The menu's Quit takes the registry's key for `app.quit`: ⌘Q by
    /// default, and the new key after a rebinding.
    #[test]
    fn quit_shows_the_registry_key() -> Result<(), String> {
        let mut registry = Registry::standard();
        let quit = effective(&registry, QUIT).ok_or("app.quit has no key")?;
        assert_eq!(quit, Chord::cmd(egui::Key::Q));
        assert_eq!(
            accelerator(quit),
            Some(Accelerator::new(Modifiers::META, Code::KeyQ))
        );
        let other = Chord::cmd(egui::Key::Q).with_shift();
        registry.rebind(QUIT, Some(other))?;
        assert_eq!(effective(&registry, QUIT), Some(other));
        assert_eq!(
            accelerator(other),
            Some(Accelerator::new(
                Modifiers::META | Modifiers::SHIFT,
                Code::KeyQ
            ))
        );
        assert_eq!(accelerator(Chord::key(egui::Key::Escape)), None);
        Ok(())
    }
}
