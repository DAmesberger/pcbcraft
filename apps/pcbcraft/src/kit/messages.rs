//! The Messages window: the history of every toast and refusal with the
//! full engine text, newest first, repeats folded ×n.
//!
//! Contract from slice S0 (docs/sidebar-ia.md); slice K3 owns it and opens
//! it from Help › Messages… and from a toast. Adapted from Solidcraft
//! 4f3ac07a `crates/ui-egui/src/messages.rs` (`KEEP`, `MAX_CHARS`), MIT
//! option; see ATTRIBUTION.md.
#![allow(dead_code, reason = "S0 contract; slice K3 uses it")]

use super::severity::Level;
use solidcraft_ui_egui::ui_kit;
use std::collections::VecDeque;

/// Entries kept; older ones are dropped.
pub const KEEP: usize = 200;
/// Characters kept of one entry's text.
pub const MAX_CHARS: usize = 4000;

/// One history entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub level: Level,
    /// The full text, including the raw engine wording, at most
    /// [`MAX_CHARS`] characters.
    pub text: String,
    pub count: usize,
    /// App time of the latest repeat, in seconds.
    pub at: f64,
}

/// What the window's buttons did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessagesResponse {
    pub closed: bool,
    pub copy_all: bool,
    pub cleared: bool,
}

/// The message history, newest first.
#[derive(Default)]
pub struct Messages {
    entries: VecDeque<Message>,
}

impl Messages {
    /// Record `text`; a repeat of the newest entry counts ×n.
    pub fn push(&mut self, level: Level, text: &str, now: f64) {
        let text: String = text.chars().take(MAX_CHARS).collect();
        if let Some(newest) = self.entries.front_mut()
            && newest.level == level
            && newest.text == text
        {
            newest.count = newest.count.saturating_add(1);
            newest.at = now;
            return;
        }
        self.entries.push_front(Message {
            level,
            text,
            count: 1,
            at: now,
        });
        self.entries.truncate(KEEP);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn entries(&self) -> impl Iterator<Item = &Message> {
        self.entries.iter()
    }

    /// Every entry as plain text, newest first, for Copy all.
    pub fn copy_all(&self) -> String {
        self.entries
            .iter()
            .map(|m| {
                if m.count > 1 {
                    format!("{} ×{}", m.text, m.count)
                } else {
                    m.text.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The window, while `open`.
    pub fn window(&mut self, ctx: &egui::Context) -> MessagesResponse {
        let mut out = MessagesResponse::default();
        out.closed = ui_kit::Window::new(egui::Id::new("messages"), "library", "Messages")
            .show_body(ctx, |ui| {
                for message in &self.entries {
                    let text = if message.count > 1 {
                        format!("{} ×{}", message.text, message.count)
                    } else {
                        message.text.clone()
                    };
                    super::hint(ui, &text, &text);
                }
                super::actions(ui, |ui| {
                    out.cleared =
                        ui_kit::button(ui, "Clear", ui_kit::Button::Enabled(true)).clicked();
                    out.copy_all =
                        ui_kit::button(ui, "Copy all", ui_kit::Button::Enabled(true)).clicked();
                });
            });
        out
    }
}
