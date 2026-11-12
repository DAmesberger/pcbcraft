//! Shell metrics, menu placement, shortcut names and command-search scoring, from SolidCraft's
//! `crates/ui-kit/src/{app_bar,ribbon,window_chrome}.rs` and
//! `crates/ui-egui/src/{toolbar,context_menu,keymap,shortcut_box}.rs` (only these items).

use egui::{Pos2, Rect, Vec2, pos2};

/// The application bar's height (`app_bar::HEIGHT`).
pub const APP_BAR_HEIGHT: f32 = 34.0;
/// The ribbon's tab row (`ribbon::TAB_ROW_HEIGHT`).
pub const TAB_ROW_HEIGHT: f32 = 30.0;
/// A ribbon group with labels (`ribbon::GROUP_HEIGHT`).
const GROUP_HEIGHT: f32 = 86.0;
/// The ribbon with labels (`toolbar::RIBBON_HEIGHT`).
pub const RIBBON_HEIGHT: f32 = TAB_ROW_HEIGHT + GROUP_HEIGHT + 1.0;
/// Room for the macOS traffic lights at the bar's left end (`window_chrome::TRAFFIC_LIGHTS`).
pub const TRAFFIC_LIGHTS: f32 = 76.0;

/// Where a menu of `size` opened at `at` goes: right of and below the point (`gap` away) when it
/// fits in `screen`, flipped to the left or above it when it would run past that edge, and
/// against the far edge when it fits on neither side. A flipped menu ends at the cursor, so it
/// stays in view even when the window reaches past the screen (#1354).
pub fn place(at: Pos2, gap: Vec2, size: Vec2, screen: Rect) -> Pos2 {
    let axis = |at: f32, gap: f32, len: f32, lo: f32, hi: f32| {
        if at + gap + len <= hi {
            at + gap
        } else if at - gap - len >= lo {
            at - gap - len
        } else {
            (hi - len).max(lo)
        }
    };
    pos2(axis(at.x, gap.x, size.x, screen.left(), screen.right()), axis(at.y, gap.y, size.y, screen.top(), screen.bottom()))
}

/// A key press as a shortcut name: `E`, `Shift+E`, `Ctrl+Alt+K`, `F5`, `Delete`.
///
/// A modifier key is the shortcut itself, and its two sides are the same shortcut (the app's
/// modifiers collapse them), so a bound Shift key is named `Shift`, not `Shift+ShiftLeft`
/// (#4582).
pub fn key_name(key: egui::Key, mods: egui::Modifiers) -> String {
    let modifier = Modifier::of(key);
    let mut s = modifier_prefix(mods, modifier);
    s.push_str(modifier.map_or_else(|| key.name(), |m| m.name()));
    s
}

/// The modifiers in a shortcut name, before the key. `modifier` is the modifier the pressed key
/// is, when it is one: a modifier key is the shortcut itself, so it is not named twice (#4582).
fn modifier_prefix(mods: egui::Modifiers, modifier: Option<Modifier>) -> String {
    let mut s = String::new();
    if (mods.command || mods.ctrl) && modifier != Some(Modifier::Ctrl) {
        s.push_str("Ctrl+");
    }
    // Keep the historical Ctrl/Cmd alias for existing bindings, but don't discard a
    // physical Control pressed together with macOS Command.
    if mods.mac_cmd && mods.ctrl && modifier != Some(Modifier::Ctrl) {
        s.push_str("Cmd+");
    }
    if mods.alt && modifier != Some(Modifier::Alt) {
        s.push_str("Alt+");
    }
    if mods.shift && modifier != Some(Modifier::Shift) {
        s.push_str("Shift+");
    }
    s
}

/// The modifier a key is, when it is one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Modifier {
    Ctrl,
    Alt,
    Shift,
}

impl Modifier {
    /// The modifier a key is: the left and right keys are the same one.
    fn of(key: egui::Key) -> Option<Self> {
        match key {
            egui::Key::ShiftLeft | egui::Key::ShiftRight => Some(Modifier::Shift),
            egui::Key::ControlLeft | egui::Key::ControlRight => Some(Modifier::Ctrl),
            egui::Key::AltLeft | egui::Key::AltRight => Some(Modifier::Alt),
            // The Super key is the app's Ctrl (the Command key on macOS).
            egui::Key::SuperLeft | egui::Key::SuperRight => Some(Modifier::Ctrl),
            _ => None,
        }
    }

    /// The modifier's name in a shortcut.
    fn name(self) -> &'static str {
        match self {
            Modifier::Ctrl => "Ctrl",
            Modifier::Alt => "Alt",
            Modifier::Shift => "Shift",
        }
    }
}

/// How well `q` matches `s` (0 = not at all): a prefix beats a word start beats a substring
/// beats the letters in order with gaps.
pub fn score(q: &str, s: &str) -> u32 {
    if q.is_empty() {
        return 1;
    }
    let s = s.to_lowercase();
    if s.starts_with(q) {
        return 1000 - s.len().min(500) as u32;
    }
    if s.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q)) {
        return 800;
    }
    if s.contains(q) {
        return 600;
    }
    // Letters in order: fewer gaps score higher.
    let mut gaps = 0u32;
    let mut it = s.chars();
    let mut last_hit = true;
    for qc in q.chars() {
        let mut found = false;
        for c in it.by_ref() {
            if c == qc {
                found = true;
                break;
            }
            if last_hit {
                gaps += 1;
            }
            last_hit = false;
        }
        if !found {
            return 0;
        }
        last_hit = true;
    }
    300u32.saturating_sub(gaps * 20).max(10)
}
