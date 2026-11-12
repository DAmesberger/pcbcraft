//! Code-drawn glyphs: small control marks the kit draws (transport, badges, pin, cross,
//! breadcrumb).
//!
//! Like the icons, they are the host's artwork: the host fills a `static` [`Glyphs`] with its
//! painters and [`install`]s it once, before the first frame and in its test contexts. Before
//! that every glyph draws nothing.
use std::sync::OnceLock;

use egui::{Color32, Painter, Pos2, Rect, Stroke};

/// The host's glyph painters.
#[derive(Clone, Copy)]
pub struct Glyphs {
    /// A history transport mark centred on the point: `first`, `back`, `forward`, `play`, `pause`
    /// or (any other name) last.
    pub transport: fn(&Painter, Pos2, &str, Color32),
    /// A small status badge (`lock`, …) in the rectangle.
    pub badge: fn(&Painter, Rect, &str, Color32),
    /// A pin in the rectangle; `bool` is whether it is pinned.
    pub pin: fn(&Painter, Rect, bool, Color32),
    /// A cross filling the rectangle.
    pub cross: fn(&Painter, Rect, Stroke),
    /// A path separator centred on the point.
    pub breadcrumb: fn(&Painter, Pos2, Color32),
}

static INSTALLED: OnceLock<&'static Glyphs> = OnceLock::new();

/// Install the host's glyphs, once per process. Installing the same glyphs again does nothing; a
/// different set is ignored and logged.
pub fn install(glyphs: &'static Glyphs) {
    if let Err(ignored) = INSTALLED.set(glyphs)
        && let Some(installed) = INSTALLED.get()
        && !std::ptr::eq(*installed, ignored)
    {
        log::warn!("glyphs are already installed: a second, different set is ignored");
    }
}

fn installed() -> Option<&'static Glyphs> {
    INSTALLED.get().copied()
}

/// [`Glyphs::transport`].
pub fn transport_glyph(p: &Painter, c: Pos2, what: &str, col: Color32) {
    if let Some(g) = installed() {
        (g.transport)(p, c, what, col);
    }
}
