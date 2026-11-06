//! Settings (⌘, / Ctrl+, and the File menu): the Keyboard section. It
//! changes the effective keymap (`crate::commands::Registry`) the palette,
//! the shortcuts sheet, tooltips, tool cards, the key dispatcher and the
//! reveal overlays all read, and the key-reveal options. Changes apply at
//! once and are kept per user (`pcbcraft_io::settings::KeyboardSettings`).
//! The Production section's "Require a reason to waive errors" sets the
//! engine's `require_waiver_reason` (kept in `UserSettings`).
//!
//! A key is recorded by clicking its cap and pressing the new chord (Esc
//! cancels). A chord another command already takes where both apply (the
//! registry's context and shadow rules) is not taken silently: the row
//! names that command and offers Swap or Cancel.
use crate::commands::{Chord, Command, Context, Registry};
use crate::keymap::Unapplied;
use crate::kit;
use pcbcraft_io::settings::{HOLD_MS_RANGE, RevealSettings};
use solidcraft_ui_egui::{theme::Tokens, ui_kit, ui_kit::Button};

const MAX_FILTER: usize = 128;

/// A recorded chord another command holds, waiting for Swap or Cancel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pending {
    pub id: String,
    pub chord: Chord,
    pub other: String,
}

/// What the user asked for in the window; the app applies it to its
/// registry ([`apply`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    /// Set the command's key (a recorded chord, or its default for Reset).
    Set(String, Option<Chord>),
    /// Give the command the pending chord and the other command its key.
    Swap,
    ResetAll,
}

#[derive(Default)]
pub(crate) struct SettingsWindow {
    open: bool,
    filter: String,
    /// The command whose next key press is recorded.
    recording: Option<String>,
    pending: Option<Pending>,
    /// Contexts whose list shows every row.
    show_all: std::collections::BTreeSet<Context>,
    /// The last refusal, shown once at the top.
    refusal: Option<String>,
}

impl SettingsWindow {
    pub fn open(&mut self) {
        self.open = true;
    }
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.recording = None;
            self.pending = None;
        }
    }
    pub fn is_recording(&self) -> bool {
        self.open && self.recording.is_some()
    }

    #[cfg(test)]
    pub fn record(&mut self, id: &str) {
        self.open = true;
        self.recording = Some(id.to_owned());
    }

    #[cfg(test)]
    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    /// While recording, the frame's first key press is the new chord: every
    /// key event of the frame is taken so no canvas or command acts on it.
    /// Esc alone cancels. Runs before anything else reads keys.
    pub fn capture(&mut self, ctx: &egui::Context) -> Option<(String, Chord)> {
        if !self.is_recording() {
            return None;
        }
        let chord = ctx.input_mut(|i| {
            let first = i.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => Some(Chord {
                    key: *key,
                    command: modifiers.command,
                    shift: modifiers.shift,
                    alt: modifiers.alt,
                }),
                _ => None,
            });
            i.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Copy
                )
            });
            first
        })?;
        let id = self.recording.take()?;
        (chord != Chord::key(egui::Key::Escape)).then_some((id, chord))
    }

    /// A recorded `chord` for `id`: set at once when free, else held as
    /// [`Pending`] naming the command that has it.
    pub fn recorded(&mut self, registry: &Registry, id: String, chord: Chord) -> Option<KeyAction> {
        self.refusal = None;
        match registry.conflict_for(&id, chord) {
            Some(other) => {
                self.pending = Some(Pending {
                    id,
                    chord,
                    other: other.id.clone(),
                });
                None
            }
            None => Some(KeyAction::Set(id, Some(chord))),
        }
    }

    /// Apply `action` to `registry`; a refusal is shown in the window.
    /// Returns whether the keymap changed.
    pub fn apply(&mut self, registry: &mut Registry, action: KeyAction) -> bool {
        let result = match action {
            KeyAction::Set(id, chord) => match chord.and_then(|c| registry.conflict_for(&id, c)) {
                Some(other) => {
                    // A reset whose default another command took: offer Swap.
                    self.pending = chord.map(|chord| Pending {
                        id: id.clone(),
                        chord,
                        other: other.id.clone(),
                    });
                    return false;
                }
                None => registry.rebind(&id, chord),
            },
            KeyAction::Swap => match self.pending.take() {
                Some(p) => registry.swap(&p.id, p.chord),
                None => return false,
            },
            KeyAction::ResetAll => {
                registry.reset_all();
                Ok(())
            }
        };
        match result {
            Ok(()) => {
                self.refusal = None;
                true
            }
            Err(reason) => {
                self.refusal = Some(reason);
                false
            }
        }
    }

    /// Draw the window while open. Returns what to do with the keymap,
    /// whether the reveal settings changed and whether the waiver-reason
    /// setting changed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        registry: &Registry,
        reveal: &mut RevealSettings,
        require_waiver_reason: &mut bool,
        unapplied: &[Unapplied],
    ) -> (Option<KeyAction>, bool, bool) {
        if !self.open {
            return (None, false, false);
        }
        let mut action = None;
        let mut reveal_changed = false;
        let mut waiver_changed = false;
        let closed = ui_kit::Window::new(egui::Id::new("app-settings"), "params", "Settings")
            .show_body(ctx, |ui| {
                kit::section(ui, "settings-production", "Production", true, None, |ui| {
                    waiver_changed = kit::check_row(
                        ui,
                        "Require a reason to waive errors",
                        require_waiver_reason,
                    );
                });
                kit::section(ui, "settings-reveal", "Key reveal", true, None, |ui| {
                    reveal_changed |= reveal_rows(ui, reveal);
                });
                kit::section(ui, "settings-keyboard", "Keyboard", true, None, |ui| {
                    action = self.keyboard(ui, registry, unapplied);
                });
            });
        if closed {
            self.open = false;
            self.recording = None;
            self.pending = None;
        }
        (action, reveal_changed, waiver_changed)
    }

    fn keyboard(
        &mut self,
        ui: &mut egui::Ui,
        registry: &Registry,
        unapplied: &[Unapplied],
    ) -> Option<KeyAction> {
        let mut action = None;
        for problem in unapplied {
            kit::status(
                ui,
                &format!(
                    "Saved key for {} not applied: {}",
                    problem.binding.command, problem.reason
                ),
                kit::Tone::Warning,
            );
        }
        if let Some(reason) = &self.refusal {
            kit::status(ui, reason, kit::Tone::Error);
        }
        ui.horizontal(|ui| {
            let custom = registry.overrides().len();
            kit::note(
                ui,
                &if custom == 0 {
                    "Default keymap".to_owned()
                } else {
                    format!("{custom} changed")
                },
            );
            if ui_kit::button(ui, "Reset all", Button::Enabled(custom > 0))
                .on_hover_text("Every key back to its default")
                .clicked()
            {
                action = Some(KeyAction::ResetAll);
            }
        });
        kit::filter_field(
            ui,
            egui::Id::new("settings-keys-filter"),
            &mut self.filter,
            "Filter by name, key or context",
            false,
            MAX_FILTER,
        );
        let filter = self.filter.trim().to_lowercase();
        let mut any = false;
        for context in Context::ALL {
            let rows: Vec<&Command> = registry
                .commands()
                .iter()
                .filter(|c| c.context == context && Registry::bindable(c))
                .filter(|c| matches(registry, c, &filter))
                .collect();
            if rows.is_empty() {
                continue;
            }
            any = true;
            let all = self.show_all.contains(&context) || !filter.is_empty();
            kit::section(
                ui,
                ("settings-keys", context),
                context.label(),
                context == Context::Global || !filter.is_empty(),
                Some(rows.len()),
                |ui| {
                    let max = if all { rows.len() } else { kit::LIST_MAX_ROWS };
                    let shown = kit::list_frame(ui, rows.len(), max, |ui, index| {
                        if let Some(command) = rows.get(index)
                            && let Some(a) = self.row(ui, registry, command)
                        {
                            action = Some(a);
                        }
                    });
                    if shown.show_all {
                        self.show_all.insert(context);
                    }
                },
            );
        }
        if !any {
            kit::status(ui, "No command matches", kit::Tone::Dim);
        }
        action
    }

    /// One command: its name (with a dot when changed), its key cap (click
    /// to record), Reset and Clear; and the Swap or Cancel choice when a
    /// recorded key is taken.
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        registry: &Registry,
        command: &Command,
    ) -> Option<KeyAction> {
        let t = Tokens::get();
        let mut action = None;
        let custom = registry.is_custom(&command.id);
        ui.horizontal(|ui| {
            let name = if custom {
                egui::RichText::new(format!("{} •", command.title)).color(t.accent)
            } else {
                egui::RichText::new(&command.title)
            };
            ui.label(name).on_hover_text(if custom {
                "Changed from the default"
            } else {
                "Default key"
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if custom
                    && ui_kit::button(ui, "Reset", Button::Small)
                        .on_hover_text(match command.chord {
                            Some(chord) => format!("Back to {}", chord.shown()),
                            None => "Back to no key".to_owned(),
                        })
                        .clicked()
                {
                    action = Some(KeyAction::Set(command.id.clone(), command.chord));
                }
                if registry.chord(command).is_some()
                    && ui_kit::button(ui, "Clear", Button::Small)
                        .on_hover_text("No key")
                        .clicked()
                {
                    action = Some(KeyAction::Set(command.id.clone(), None));
                }
                let recording = self.recording.as_deref() == Some(command.id.as_str());
                let key = if recording {
                    "Press a key…".to_owned()
                } else {
                    registry.shortcut(command).unwrap_or_else(|| "—".to_owned())
                };
                let style = if recording {
                    Button::SmallSelected
                } else {
                    Button::Small
                };
                if ui_kit::button(ui, egui::RichText::new(key).monospace(), style)
                    .on_hover_text("Click, then press the new key (Esc cancels)")
                    .clicked()
                {
                    self.pending = None;
                    self.recording = (!recording).then(|| command.id.clone());
                }
            });
        });
        if let Some(pending) = self.pending.clone().filter(|p| p.id == command.id) {
            let other = registry
                .get(&pending.other)
                .map_or(pending.other.clone(), |c| {
                    format!("{} ({})", c.title, c.context.label())
                });
            kit::status(
                ui,
                &format!("{} is taken by {other}", pending.chord.shown()),
                kit::Tone::Warning,
            );
            ui.horizontal(|ui| {
                if ui_kit::button(ui, "Swap", Button::Primary)
                    .on_hover_text("Give that command this one's key")
                    .clicked()
                {
                    action = Some(KeyAction::Swap);
                }
                if ui_kit::button(ui, "Cancel", Button::Normal).clicked() {
                    self.pending = None;
                }
            });
        }
        action
    }
}

fn matches(registry: &Registry, command: &Command, filter: &str) -> bool {
    filter.is_empty()
        || command.title.to_lowercase().contains(filter)
        || command.id.to_lowercase().contains(filter)
        || command.context.label().to_lowercase().contains(filter)
        || registry
            .shortcut(command)
            .is_some_and(|k| k.to_lowercase().contains(filter))
}

/// The reveal options: hold ⌘ for badges (and after how long), tap ⌥ for
/// key tips. Returns whether one changed.
fn reveal_rows(ui: &mut egui::Ui, reveal: &mut RevealSettings) -> bool {
    let command = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl"
    };
    let alt = if cfg!(target_os = "macos") {
        "Option"
    } else {
        "Alt"
    };
    let mut changed = kit::check_row(
        ui,
        &format!("Hold {command} to show the keys of what is on screen"),
        &mut reveal.hold_badges,
    );
    let mut ms = f64::from(reveal.hold_ms);
    let range = f64::from(*HOLD_MS_RANGE.start())..=f64::from(*HOLD_MS_RANGE.end());
    if kit::number_row(ui, "Hold for", &mut ms, "ms", range) {
        let ms = ms.round().clamp(
            f64::from(*HOLD_MS_RANGE.start()),
            f64::from(*HOLD_MS_RANGE.end()),
        );
        // In range by the clamp; the cast keeps the whole milliseconds.
        let ms = ms as u32;
        changed |= ms != reveal.hold_ms;
        reveal.hold_ms = ms;
    }
    changed |= kit::check_row(
        ui,
        &format!("Tap {alt} for key tips on the ribbon"),
        &mut reveal.key_tips,
    );
    changed
}

impl super::App {
    /// A key being recorded in Settings: taken before anything else reads
    /// keys, then set or held for Swap.
    pub(super) fn record_key(&mut self, ctx: &egui::Context) {
        if let Some((id, chord)) = self.settings_window.capture(ctx)
            && let Some(action) = self.settings_window.recorded(&self.registry, id, chord)
        {
            self.key_action(ctx, action);
        }
    }

    /// The Settings window while open.
    pub(super) fn settings(&mut self, ctx: &egui::Context) {
        let mut keyboard = self.sources.keyboard().clone();
        let mut require_reason = self.sources.require_waiver_reason();
        let (action, reveal_changed, waiver_changed) = self.settings_window.show(
            ctx,
            &self.registry,
            &mut keyboard.reveal,
            &mut require_reason,
            &self.keymap_problems,
        );
        if reveal_changed && let Err(reason) = self.sources.set_keyboard(keyboard) {
            self.say(crate::kit::severity::Level::Warning, reason);
        }
        if waiver_changed {
            // The engine refuses a waiver without a reason in every path of
            // this app (panels, assistant), not only at the Waive button.
            self.sources.set_require_waiver_reason(require_reason);
            self.engine.require_waiver_reason(require_reason);
        }
        if let Some(action) = action {
            self.key_action(ctx, action);
        }
    }

    /// Apply a keymap change, then publish and keep the new keymap.
    pub(super) fn key_action(&mut self, ctx: &egui::Context, action: KeyAction) {
        let registry = std::sync::Arc::make_mut(&mut self.registry);
        if self.settings_window.apply(registry, action) {
            self.keymap_changed(ctx);
        }
    }

    /// Every reader sees the new keymap; the settings keep it.
    fn keymap_changed(&mut self, ctx: &egui::Context) {
        crate::keys::publish(ctx, &self.registry);
        let mut keyboard = self.sources.keyboard().clone();
        keyboard.bindings = crate::keymap::bindings(&self.registry, &self.keymap_problems);
        if let Err(reason) = self.sources.set_keyboard(keyboard) {
            self.say(crate::kit::severity::Level::Warning, reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key as K;

    /// One frame with a key press: what the window captured and how many
    /// events are left for the canvases and the key dispatcher.
    fn press(
        ctx: &egui::Context,
        window: &mut SettingsWindow,
        key: K,
    ) -> (Option<(String, Chord)>, usize) {
        let mut out = (None, 0);
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                out.0 = window.capture(ui.ctx());
                out.1 = ui.input(|i| i.events.len());
            },
        )
        .textures_delta
        .clear();
        out
    }

    /// Clicking a key cap records the next chord, which no one else sees;
    /// a free chord is set at once, Esc cancels, and a taken chord names
    /// its holder and waits for Swap.
    #[test]
    fn recording_sets_cancels_or_offers_a_swap() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut registry = Registry::standard();
        let mut window = SettingsWindow::default();
        window.record("route.width-next");
        let (captured, left) = press(&ctx, &mut window, K::Q);
        assert_eq!(left, 0, "the key reaches nothing else");
        let (id, chord) = captured.ok_or("captured")?;
        let action = window.recorded(&registry, id, chord).ok_or("free key")?;
        assert!(window.apply(&mut registry, action));
        let width = registry.get("route.width-next").ok_or("width")?;
        assert_eq!(registry.chord(width), Some(Chord::key(K::Q)));

        window.record("pcb.fill");
        let (captured, _) = press(&ctx, &mut window, K::Escape);
        assert_eq!(captured, None, "Esc cancels");
        assert!(!window.is_recording());

        window.record("pcb.tune");
        let (captured, _) = press(&ctx, &mut window, K::B);
        let (id, chord) = captured.ok_or("captured")?;
        assert_eq!(window.recorded(&registry, id, chord), None);
        assert_eq!(
            window.pending().map(|p| p.other.as_str()),
            Some("pcb.board-shape")
        );
        assert!(window.apply(&mut registry, KeyAction::Swap));
        let chord = |r: &Registry, id: &str| r.get(id).and_then(|c| r.chord(c));
        assert_eq!(chord(&registry, "pcb.tune"), Some(Chord::key(K::B)));
        assert_eq!(chord(&registry, "pcb.board-shape"), Some(Chord::key(K::T)));
        // Reset to a default another command now holds offers the swap back.
        let reset = KeyAction::Set("pcb.tune".into(), Some(Chord::key(K::T)));
        assert!(!window.apply(&mut registry, reset));
        assert_eq!(
            window.pending().map(|p| p.other.as_str()),
            Some("pcb.board-shape")
        );
        assert!(window.apply(&mut registry, KeyAction::Swap));
        assert!(window.apply(&mut registry, KeyAction::ResetAll));
        assert!(registry.overrides().is_empty());
        Ok(())
    }
}
