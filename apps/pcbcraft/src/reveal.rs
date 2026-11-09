//! Keyboard discoverability: hold ⌘ (Ctrl) to see the key of every
//! control on screen, and tap ⌥ (Alt) for ribbon key tips.
//!
//! Controls that run a command register their screen rect as they draw
//! ([`register`]); the overlay then paints every badge in one layer. The
//! per-frame list keeps its allocation, so it never grows frame by frame.
//! The hold timer ([`Hold`]) and the ⌥ tap ([`AltTap`]) only watch the
//! modifiers and the frame's events (the hold delay defaults to
//! `pcbcraft_io::settings::HOLD_MS_DEFAULT`, 500 ms; Settings › Key reveal
//! changes it): they consume nothing, so ⌘ chords,
//! ⌘-drags and ⌥-drags work as before. Key tips ([`assign_tips`]) are
//! derived from the titles and the effective keymap, so they are stable
//! and equal to a command's single-key shortcut where it has one.
use crate::commands::AppCommand;
use egui::{Rect, pos2, vec2};
use solidcraft_theme::Tokens;

/// A control that runs a command, as it registers its rect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A ribbon tool of the shown workspace, by its ribbon id.
    Tool(&'static str),
    /// A ribbon tab, by its id.
    Tab(&'static str),
    /// An app bar or navigation bar button.
    App(AppCommand),
}

/// This frame's registered controls. Cleared, not dropped, each frame.
#[derive(Clone, Default)]
pub struct Targets {
    items: Vec<(Target, Rect)>,
}

impl Targets {
    pub fn clear(&mut self) {
        self.items.clear();
    }
    pub fn push(&mut self, target: Target, rect: Rect) {
        self.items.push((target, rect));
    }
    pub fn items(&self) -> &[(Target, Rect)] {
        &self.items
    }
    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.items.capacity()
    }
}

fn slot() -> egui::Id {
    egui::Id::unique("pcbcraft-key-targets")
}

/// Start a frame's list (the app calls it before anything draws).
pub fn begin_frame(ctx: &egui::Context) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Targets>(slot()).clear());
}

/// Register a control's rect for the overlay.
pub fn register(ctx: &egui::Context, target: Target, rect: Rect) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Targets>(slot())
            .push(target, rect)
    });
}

/// Copy this frame's registrations into `out` (kept by the caller, so
/// neither list grows per frame).
pub fn copy_targets(ctx: &egui::Context, out: &mut Vec<(Target, Rect)>) {
    out.clear();
    ctx.data_mut(|d| out.extend_from_slice(d.get_temp_mut_or_default::<Targets>(slot()).items()));
}

/// The one line that tells where the reveal is: the ⌘/ sheet's first line
/// and the palette's empty state. (The label font has no ⌥ glyph, so
/// Option is named.)
pub fn hint_line() -> &'static str {
    if cfg!(target_os = "macos") {
        "Hold ⌘ to see the keys on screen · tap Option for ribbon key tips"
    } else {
        "Hold Ctrl to see the keys on screen · tap Alt for ribbon key tips"
    }
}

/// What the reveal reads of one frame's input, before anyone consumes it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub time: f64,
    /// ⌘ (Ctrl elsewhere) and no other modifier.
    pub command_only: bool,
    /// ⌥ (Alt) and no other modifier.
    pub alt_only: bool,
    pub alt: bool,
    /// A key, a click, a scroll or text this frame.
    pub interrupted: bool,
}

/// Read the modifiers and whether anything else happened this frame.
pub fn sample(ctx: &egui::Context) -> Sample {
    ctx.input(|i| {
        let m = i.modifiers;
        // On macOS ⌃ is its own key; elsewhere Ctrl is the command key.
        let extra_ctrl = m.ctrl && m.mac_cmd;
        Sample {
            time: i.time,
            command_only: m.command && !m.shift && !m.alt && !extra_ctrl,
            alt_only: m.alt && !m.shift && !m.command && !m.ctrl,
            alt: m.alt,
            interrupted: i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { pressed: true, .. }
                        | egui::Event::PointerButton { pressed: true, .. }
                        | egui::Event::Text(_)
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Zoom(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Paste(_)
                )
            }),
        }
    })
}

/// Hold ⌘ alone for a while to reveal; any key, click or scroll while it
/// is held hides the badges until ⌘ is let go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hold {
    since: Option<f64>,
    spoiled: bool,
    shown: bool,
}

impl Hold {
    /// Advance by one frame; returns whether the badges show.
    pub fn update(&mut self, sample: Sample, delay_s: f64) -> bool {
        if !sample.command_only {
            *self = Self::default();
            return false;
        }
        if sample.interrupted {
            self.spoiled = true;
        }
        if self.spoiled {
            self.shown = false;
            return false;
        }
        let since = *self.since.get_or_insert(sample.time);
        self.shown = sample.time - since >= delay_s;
        self.shown
    }

    /// Seconds until the badges would show, while ⌘ is held untouched.
    pub fn wake_in(&self, now: f64, delay_s: f64) -> Option<f64> {
        let since = self.since.filter(|_| !self.spoiled && !self.shown)?;
        Some((since + delay_s - now).max(0.0))
    }
}

/// A tap of ⌥ alone: pressed and let go with nothing in between.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AltTap {
    down: bool,
    spoiled: bool,
}

impl AltTap {
    /// Advance by one frame; returns whether ⌥ was just tapped.
    pub fn update(&mut self, sample: Sample) -> bool {
        if !self.down {
            if sample.alt_only {
                self.down = true;
                self.spoiled = sample.interrupted;
            }
            return false;
        }
        if sample.alt {
            // Another modifier, a key, a click or a scroll with ⌥ held is
            // a gesture (⌥-drag, Alias navigation), not a tap.
            self.spoiled |= sample.interrupted || !sample.alt_only;
            return false;
        }
        self.down = false;
        let tapped = !self.spoiled && !sample.interrupted;
        self.spoiled = false;
        tapped
    }
}

/// One command to tip: its title and its single-letter shortcut in force
/// here, if it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TipEntry<'a> {
    pub title: &'a str,
    pub key: Option<char>,
    /// Chooses after the others (a ribbon's tabs after its tools).
    pub late: bool,
}

/// Letters of `title`, upper case, by word.
fn words(title: &str) -> Vec<Vec<char>> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| {
            w.chars()
                .filter(char::is_ascii_alphabetic)
                .map(|c| c.to_ascii_uppercase())
                .collect::<Vec<char>>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

fn vowel(c: char) -> bool {
    matches!(c, 'A' | 'E' | 'I' | 'O' | 'U')
}

/// One-letter candidates of a title after its first letter, best first:
/// the initials of its later words, then the following consonants of its
/// first word.
fn singles(title: &str) -> Vec<char> {
    let words = words(title);
    let mut out: Vec<char> = Vec::new();
    let later_initials = words.iter().skip(1).filter_map(|w| w.first().copied());
    let first_word = words.first().map_or(&[][..], |w| &w[..]);
    let consonants = first_word.iter().skip(1).copied().filter(|c| !vowel(*c));
    for c in later_initials.chain(consonants) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Two-letter candidates of a title, best first, every letter from the
/// title: the initials of its words; its first letter with the following
/// consonants, then any later letter; then pairs of later letters
/// (consonants first), for a title whose first letter is another
/// command's one-letter tip.
fn pairs(title: &str) -> Vec<[char; 2]> {
    let words = words(title);
    let letters: Vec<char> = words.iter().flatten().copied().collect();
    let mut out: Vec<[char; 2]> = Vec::new();
    let add = |pair: [char; 2], out: &mut Vec<[char; 2]>| {
        if pair[0] != pair[1] && !out.contains(&pair) {
            out.push(pair);
        }
    };
    let initials: Vec<char> = words.iter().filter_map(|w| w.first().copied()).collect();
    for (i, a) in initials.iter().enumerate() {
        for b in initials.iter().skip(i + 1) {
            add([*a, *b], &mut out);
        }
    }
    let Some(&first) = letters.first() else {
        return out;
    };
    for &c in letters.iter().skip(1).filter(|c| !vowel(**c)) {
        add([first, c], &mut out);
    }
    for &c in letters.iter().skip(1) {
        add([first, c], &mut out);
    }
    let mut ordered: Vec<char> = letters.iter().copied().filter(|c| !vowel(*c)).collect();
    ordered.extend(letters.iter().copied().filter(|c| vowel(*c)));
    for (i, a) in ordered.iter().enumerate() {
        for b in ordered.iter().skip(i + 1) {
            add([*a, *b], &mut out);
        }
    }
    for a in &ordered {
        for b in &ordered {
            add([*a, *b], &mut out);
        }
    }
    out
}

/// Key tips for `entries`, in their order (docs/keyboard-shortcuts.md,
/// "Key tips"):
/// 1. a command with a single-letter shortcut here takes that letter;
/// 2. otherwise its title's first letter, while free (late entries choose
///    last, titles of one or two letters first, then the given order);
/// 3. otherwise the initials of its later words, then the following
///    consonants of its first word, while free;
/// 4. otherwise two letters of its title ([`pairs`]).
///
/// No tip is a digit or a letter the title lacks, every tip is unique,
/// and no two-letter tip starts with a one-letter tip, so typing is never
/// ambiguous. The same entries always get the same tips. An entry left
/// without a tip (a title with no letters) gets an empty string.
pub fn assign_tips(entries: &[TipEntry<'_>]) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut tips: Vec<Option<String>> = vec![None; entries.len()];
    let mut taken = BTreeSet::new();
    for (tip, entry) in tips.iter_mut().zip(entries) {
        if let Some(key) = entry.key.map(|k| k.to_ascii_uppercase())
            && key.is_ascii_alphabetic()
            && taken.insert(key)
        {
            *tip = Some(key.to_string());
        }
    }
    // Late entries choose last; titles of one or two letters ("3D" has
    // only D) before the rest; then the given order.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&i| {
        let entry = entries.get(i);
        let mut letters: Vec<char> =
            entry.map_or_else(Vec::new, |e| words(e.title).into_iter().flatten().collect());
        letters.sort_unstable();
        letters.dedup();
        (entry.is_some_and(|e| e.late), letters.len().min(3), i)
    });
    for &i in &order {
        if let (Some(tip @ None), Some(entry)) = (tips.get_mut(i), entries.get(i))
            && let Some(&first) = words(entry.title).first().and_then(|w| w.first())
            && taken.insert(first)
        {
            *tip = Some(first.to_string());
        }
    }
    for &i in &order {
        if let (Some(tip @ None), Some(entry)) = (tips.get_mut(i), entries.get(i))
            && let Some(letter) = singles(entry.title)
                .into_iter()
                .find(|c| !taken.contains(c))
        {
            taken.insert(letter);
            *tip = Some(letter.to_string());
        }
    }
    let mut doubles: BTreeSet<[char; 2]> = BTreeSet::new();
    for &i in &order {
        if let (Some(tip @ None), Some(entry)) = (tips.get_mut(i), entries.get(i))
            && let Some(pair) = pairs(entry.title)
                .into_iter()
                .find(|p| !taken.contains(&p[0]) && !doubles.contains(p))
        {
            doubles.insert(pair);
            *tip = Some(pair.iter().collect());
        }
    }
    tips.into_iter().map(Option::unwrap_or_default).collect()
}

/// The letter a chord is, when it is one unmodified letter key.
pub fn single_letter(chord: crate::commands::Chord) -> Option<char> {
    if chord.command || chord.shift || chord.alt {
        return None;
    }
    let name = chord.key.name();
    let mut chars = name.chars();
    let c = chars.next()?;
    (chars.next().is_none() && c.is_ascii_alphabetic()).then(|| c.to_ascii_uppercase())
}

/// A key cap painted at `rect`'s position (Solidcraft's ribbon tooltip
/// cap, `shell::key_cap`), centred on `at`; `strong` for key tips.
pub fn paint_cap(painter: &egui::Painter, at: egui::Pos2, text: &str, strong: bool) -> Rect {
    let t = Tokens::get();
    let font = egui::FontId::monospace(t.font_body);
    let ink = if strong { t.text } else { t.text_dim };
    let galley = painter.layout_no_wrap(text.to_owned(), font, ink);
    let size = galley.size() + vec2(10.0, 2.0);
    let rect = Rect::from_center_size(at, size);
    let radius = t.radius_sm.saturating_sub(1);
    painter.rect(
        rect,
        radius,
        t.panel_header,
        if strong {
            egui::Stroke::new(1.0, t.accent)
        } else {
            t.tertiary_border(t.border)
        },
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + vec2(5.0, 1.0), galley, ink);
    rect
}

/// A small floating list of (key, what it does) rows with a caption,
/// painted at `anchor` (its top-left), kept inside `screen`.
pub fn paint_panel(
    painter: &egui::Painter,
    screen: Rect,
    anchor: egui::Pos2,
    caption: &str,
    rows: &[(String, String)],
    footer: Option<&str>,
) -> Rect {
    let t = Tokens::get();
    let pad = t.space_8;
    let row_h = t.font_body + t.space_8;
    let key_font = egui::FontId::monospace(t.font_body);
    let text_font = egui::FontId::proportional(t.font_body);
    let key_w = rows
        .iter()
        .map(|(k, _)| {
            painter
                .layout_no_wrap(k.clone(), key_font.clone(), t.text_dim)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
        + 10.0;
    let text_w = rows
        .iter()
        .map(|(_, w)| {
            painter
                .layout_no_wrap(w.clone(), text_font.clone(), t.text)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
        .min(320.0);
    let caption_h = t.font_small + t.space_8;
    let footer_h = footer.map_or(0.0, |f| (f.lines().count() as f32) * caption_h + t.space_4);
    let size = vec2(
        pad * 3.0 + key_w + text_w,
        pad * 2.0 + caption_h + rows.len() as f32 * row_h + footer_h,
    );
    let mut rect = Rect::from_min_size(anchor, size);
    // Keep it on screen.
    let shift = vec2(
        (screen.right() - rect.right()).min(0.0) + (screen.left() - rect.left()).max(0.0),
        (screen.bottom() - rect.bottom()).min(0.0) + (screen.top() - rect.top()).max(0.0),
    );
    rect = rect.translate(shift);
    painter.rect(
        rect,
        t.radius_md,
        t.toolbar,
        t.container_border(t.border),
        egui::StrokeKind::Inside,
    );
    painter.text(
        pos2(rect.left() + pad, rect.top() + pad),
        egui::Align2::LEFT_TOP,
        caption,
        egui::FontId::proportional(t.font_small),
        t.text_dim,
    );
    let mut y = rect.top() + pad + caption_h;
    for (key, what) in rows {
        paint_cap(
            painter,
            pos2(rect.left() + pad + key_w / 2.0, y + row_h / 2.0),
            key,
            false,
        );
        let text = painter.layout(what.clone(), text_font.clone(), t.text, text_w);
        painter.galley(
            pos2(
                rect.left() + pad * 2.0 + key_w,
                y + (row_h - text.size().y) / 2.0,
            ),
            text,
            t.text,
        );
        y += row_h;
    }
    if let Some(footer) = footer {
        painter.text(
            pos2(rect.left() + pad, y + t.space_4),
            egui::Align2::LEFT_TOP,
            footer,
            egui::FontId::proportional(t.font_small),
            t.text_dim,
        );
    }
    rect
}

#[cfg(test)]
#[path = "reveal_tests.rs"]
mod tests;
