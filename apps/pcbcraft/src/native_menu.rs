//! The macOS menu bar, owned and updated on eframe's main thread, as in
//! Solidcraft's `apps/solidcraft/src/native_menu.rs`. Its items, their
//! keys and their states come from [`crate::menu_bar`] and the command
//! registry.
//!
//! It replaces winit's default menu, whose Quit (⌘Q) sends AppKit's
//! `terminate:` and ends the app at once, without asking about unsaved
//! work: this menu's Quit runs the registry's `app.quit`, the same request
//! as the window's close button. AppKit gives a key equivalent to the menu
//! before the window sees it, so a ⌘ chord the menu takes reaches the app
//! once, through the menu, also while a text field has the keyboard.
use crate::commands::Chord;
use crate::menu_bar::{self, BAR, Choice, Entry, Now, System};
use crate::recent_projects::RecentItem;
use muda::{
    AboutMetadata, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
    accelerator::{Accelerator, Code, Modifiers},
};
use std::sync::mpsc::{Receiver, sync_channel};

/// The most menu events read in one frame.
const MAX_EVENTS: usize = 64;

/// A menu item and what it shows now.
struct Item {
    item: MenuItem,
    entry: Entry,
    shown: menu_bar::State,
}

pub struct NativeMenu {
    menu: Menu,
    items: Vec<Item>,
    events: Receiver<MenuEvent>,
    /// File › Open Recent, rebuilt when the recent list changes.
    recent: Option<Recent>,
}

/// The Open Recent submenu and what it shows.
struct Recent {
    submenu: Submenu,
    /// Each project's item, in order.
    projects: Vec<(MenuId, std::path::PathBuf)>,
    clear: Item,
    shown: Option<Vec<RecentItem>>,
}

impl NativeMenu {
    pub fn new(ctx: &egui::Context, now: &Now) -> muda::Result<Self> {
        let menu = Menu::new();
        let mut items = Vec::new();
        let mut submenus = Vec::new();
        let mut recent = None;
        for (title, entries) in BAR {
            let submenu = Submenu::new(*title, true);
            for entry in *entries {
                match entry {
                    Entry::Separator => submenu.append(&PredefinedMenuItem::separator())?,
                    Entry::System(system) => submenu.append(&predefined(*system))?,
                    Entry::Recent => {
                        let Some(state) = menu_bar::state(now, menu_bar::CLEAR_RECENT) else {
                            continue;
                        };
                        let open = Submenu::new(menu_bar::RECENT_TITLE, true);
                        submenu.append(&open)?;
                        recent = Some(Recent {
                            submenu: open,
                            projects: Vec::new(),
                            clear: Item {
                                item: MenuItem::new(&state.label, false, None),
                                entry: menu_bar::CLEAR_RECENT,
                                shown: state,
                            },
                            shown: None,
                        });
                    }
                    Entry::Command(..) | Entry::Delete => {
                        let Some(state) = menu_bar::state(now, *entry) else {
                            continue;
                        };
                        let item = MenuItem::new(
                            &state.label,
                            state.enabled,
                            state.key.and_then(accelerator),
                        );
                        submenu.append(&item)?;
                        items.push(Item {
                            item,
                            entry: *entry,
                            shown: state,
                        });
                    }
                }
            }
            menu.append(&submenu)?;
            submenus.push((*title, submenu));
        }
        let (sender, events) = sync_channel(MAX_EVENTS);
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = sender.try_send(event);
            ctx.request_repaint();
        }));
        menu.init_for_nsapp();
        for (title, submenu) in &submenus {
            match *title {
                "Window" => submenu.set_as_windows_menu_for_nsapp(),
                "Help" => submenu.set_as_help_menu_for_nsapp(),
                _ => {}
            }
        }
        Ok(Self {
            menu,
            items,
            events,
            recent,
        })
    }

    /// Rebuild Open Recent when the recent projects changed: one item per
    /// project (an unavailable one says so and is removed when chosen),
    /// then Clear Menu, enabled when there is something to clear.
    pub fn update_recent(&mut self, recent: &[RecentItem]) -> muda::Result<()> {
        let Some(open) = &mut self.recent else {
            return Ok(());
        };
        if open.shown.as_deref() == Some(recent) {
            return Ok(());
        }
        // Tried once per change: a failure is reported once, not per frame.
        open.shown = Some(recent.to_vec());
        while open.submenu.remove_at(0).is_some() {}
        open.projects.clear();
        for project in recent {
            let item = MenuItem::new(&project.label, true, None);
            open.submenu.append(&item)?;
            open.projects
                .push((item.id().clone(), project.path.clone()));
        }
        if !recent.is_empty() {
            open.submenu.append(&PredefinedMenuItem::separator())?;
        }
        open.clear.item.set_enabled(!recent.is_empty());
        open.submenu.append(&open.clear.item)
    }

    /// The items chosen since the last frame, as what each does now.
    pub fn take(&mut self, now: &Now) -> Vec<Choice> {
        let chosen: Vec<MenuId> = self
            .events
            .try_iter()
            .take(MAX_EVENTS)
            .map(|event| event.id)
            .collect();
        let recent = self.recent.as_ref();
        chosen
            .iter()
            .filter_map(|id| {
                if let Some((_, path)) = recent.and_then(|r| r.projects.iter().find(|p| p.0 == *id))
                {
                    return Some(Choice::OpenRecent(path.clone()));
                }
                let clear = recent
                    .map(|r| &r.clear)
                    .filter(|clear| clear.item.id() == id);
                clear
                    .or_else(|| self.items.iter().find(|i| i.item.id() == id))
                    .map(|i| menu_bar::choose(now, i.entry))
            })
            .collect()
    }

    /// Bring every item's label, enabled state and key up to date.
    pub fn update(&mut self, now: &Now) {
        for item in &mut self.items {
            let Some(state) = menu_bar::state(now, item.entry) else {
                continue;
            };
            if state.label != item.shown.label {
                item.item.set_text(&state.label);
            }
            if state.enabled != item.shown.enabled {
                item.item.set_enabled(state.enabled);
            }
            if state.key != item.shown.key
                && item
                    .item
                    .set_accelerator(state.key.and_then(accelerator))
                    .is_err()
            {
                // Try again next frame.
                continue;
            }
            item.shown = state;
        }
    }
}

impl Drop for NativeMenu {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        self.menu.remove_for_nsapp();
    }
}

fn predefined(system: System) -> PredefinedMenuItem {
    match system {
        System::About => PredefinedMenuItem::about(
            Some(concat!("About ", pcbcraft_core::product_name!())),
            Some(AboutMetadata {
                name: Some(pcbcraft_core::product::NAME.into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                credits: Some(format!(
                    "Built on Solidcraft's engine (github.com/tangency-labs/solidcraft).\n{}",
                    pcbcraft_core::product::WEBSITE
                )),
                ..Default::default()
            }),
        ),
        System::Services => PredefinedMenuItem::services(None),
        System::Hide => {
            PredefinedMenuItem::hide(Some(concat!("Hide ", pcbcraft_core::product_name!())))
        }
        System::HideOthers => PredefinedMenuItem::hide_others(None),
        System::ShowAll => PredefinedMenuItem::show_all(None),
        System::Minimize => PredefinedMenuItem::minimize(None),
        System::Zoom => PredefinedMenuItem::maximize(None),
        System::Fullscreen => PredefinedMenuItem::fullscreen(None),
        System::BringAllToFront => PredefinedMenuItem::bring_all_to_front(None),
    }
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
        K::Comma => Code::Comma,
        K::Period => Code::Period,
        K::Slash => Code::Slash,
        K::Backslash => Code::Backslash,
        K::Semicolon => Code::Semicolon,
        K::Quote => Code::Quote,
        K::Minus => Code::Minus,
        K::Equals => Code::Equal,
        K::OpenBracket => Code::BracketLeft,
        K::CloseBracket => Code::BracketRight,
        K::Backtick => Code::Backquote,
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
    use crate::commands::{Context, Registry, active_contexts};

    /// Every key equivalent the bar takes by default converts to a menu
    /// accelerator, with ⌘ as META: none silently falls back to egui.
    #[test]
    fn every_menu_key_is_an_accelerator() -> Result<(), String> {
        let registry = Registry::standard();
        let active = active_contexts(Context::PcbLayout);
        let now = Now {
            registry: &registry,
            active: &active,
            typing: false,
            recording: false,
            browser_shown: true,
            issues_shown: false,
            available: &|_| true,
        };
        for (_, entries) in BAR {
            for entry in *entries {
                let Some(key) = menu_bar::state(&now, *entry).and_then(|s| s.key) else {
                    continue;
                };
                let accelerator = accelerator(key).ok_or(format!("{entry:?}: {key:?}"))?;
                assert!(
                    accelerator.modifiers().contains(Modifiers::META),
                    "{entry:?}"
                );
            }
        }
        assert_eq!(
            accelerator(Chord::cmd(egui::Key::Q)),
            Some(Accelerator::new(Modifiers::META, Code::KeyQ))
        );
        assert_eq!(
            accelerator(Chord::cmd(egui::Key::Comma)),
            Some(Accelerator::new(Modifiers::META, Code::Comma))
        );
        assert_eq!(
            accelerator(Chord::cmd(egui::Key::S).with_shift()),
            Some(Accelerator::new(
                Modifiers::META | Modifiers::SHIFT,
                Code::KeyS
            ))
        );
        assert_eq!(accelerator(Chord::key(egui::Key::Escape)), None);
        Ok(())
    }
}
