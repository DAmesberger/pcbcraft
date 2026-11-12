//! The kit's type API: interface weights, the family and fallback names a host installs its faces
//! under, the script checks and the on-demand CJK font state. It holds no font data and loads
//! nothing: the host embeds and installs the faces.
//!
//! Host contract: before the kit draws, the context's fonts bind the weight families. A host with
//! faces of its own binds them in its definitions (see [`with_weight_families`]); a host with none
//! calls [`install_weight_families`] once, and the kit draws every weight with egui's default
//! faces.

use egui::{FontFamily, FontId, RichText};

/// Name of the medium (500) interface family, and of its face in a host's definitions.
pub const MEDIUM_FAMILY: &str = "ui-medium";

/// Name of the semibold (600) interface family, and of its face in a host's definitions.
pub const SEMIBOLD_FAMILY: &str = "ui-semibold";

/// Name of the bold (700) interface family, and of its face in a host's definitions.
pub const BOLD_FAMILY: &str = "ui-bold";

fn medium_family() -> FontFamily {
    FontFamily::Name(MEDIUM_FAMILY.into())
}

fn semibold_family() -> FontFamily {
    FontFamily::Name(SEMIBOLD_FAMILY.into())
}

fn bold_family() -> FontFamily {
    FontFamily::Name(BOLD_FAMILY.into())
}

/// The interface families, regular first: Proportional, then medium, semibold and bold.
pub fn sans_families() -> [FontFamily; 4] {
    [FontFamily::Proportional, medium_family(), semibold_family(), bold_family()]
}

/// Medium (500) interface text, retaining the language fallbacks and user override.
pub fn medium(size: f32) -> FontId {
    FontId::new(size, medium_family())
}

/// Semibold (600) interface text, retaining the language fallbacks and user override.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, semibold_family())
}

/// Bold (700) interface text, retaining the language fallbacks and user override.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, bold_family())
}

/// Select a real interface weight without changing the text's size or colour.
pub trait RichTextExt {
    fn medium(self) -> Self;
    fn semibold(self) -> Self;
    fn bold_face(self) -> Self;
}

impl RichTextExt for RichText {
    fn medium(self) -> Self {
        self.family(medium_family())
    }
    fn semibold(self) -> Self {
        self.family(semibold_family())
    }
    fn bold_face(self) -> Self {
        self.family(bold_family())
    }
}

/// Name of the CJK fallback entry in [`egui::FontDefinitions`].
pub const CJK_FALLBACK: &str = "cjk_fallback";

/// Name of the Chinese fallback entry in [`egui::FontDefinitions`], after [`CJK_FALLBACK`].
/// A Japanese-only CJK font (Meiryo, Hiragino Gothic) lacks simplified characters, so
/// Chinese falls through to this one per glyph.
pub const CHINESE_FALLBACK: &str = "chinese_fallback";

/// Name of the chosen interface font in [`egui::FontDefinitions`], first in every sans
/// family when one is set (#1290).
pub const UI_FONT: &str = "ui_font";

/// Name of the Hangul fallback entry in [`egui::FontDefinitions`], after the CJK ones.
pub const HANGUL_FALLBACK: &str = "hangul_fallback";

/// Name of the Thai fallback entry in [`egui::FontDefinitions`], after the CJK ones (#1902).
pub const THAI_FALLBACK: &str = "thai_fallback";

/// Name of the Hebrew fallback entry in [`egui::FontDefinitions`], last (#3457).
pub const HEBREW_FALLBACK: &str = "hebrew_fallback";

/// Name of the Arabic fallback entry in [`egui::FontDefinitions`]: first of the fallbacks for an
/// Arabic UI, last otherwise. It stays after the Latin interface font, which keeps drawing spaces,
/// digits and Latin text, so each Arabic word is one shaped run (see [`solidcraft_ui_text::bidi`]).
pub const ARABIC_FALLBACK: &str = "arabic_fallback";

/// egui's built-in monospace face, also the last fallback of every sans family (#2955).
pub const SYMBOL_FALLBACK: &str = "Hack";

/// UI languages whose text needs a CJK script font on the web.
pub const CJK_LANGUAGES: &[&str] = &["ja", "zh", "zh-TW", "ko"];

/// Whether the UI language needs a CJK font (the web app fetches one only then).
pub fn needs_cjk_font(language: &str) -> bool {
    CJK_LANGUAGES.contains(&language)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CjkFontStatus {
    #[default]
    Unloaded,
    Loading,
    Installed,
    Failed,
}

/// The fallback entries in the order they join each family for a UI language.
///
/// Chinese goes first for `zh`: a Japanese font (Meiryo, Yu Gothic) lacks the simplified
/// characters, so the rest of the word falls through to the Chinese font per glyph — and
/// epaint centres each face's own line height in the row, so glyphs from two faces of very
/// different height (Meiryo ≈ 2.0 em, Microsoft YaHei ≈ 1.4 em) sit at different heights
/// inside one word. One face for the whole word keeps them on one baseline.
/// Thai, Hebrew and Arabic come last whatever the language: no other face draws them. Arabic
/// goes first for `ar` instead: a CJK font such as Arial Unicode also has Arabic letters, without
/// the shaping data that joins them.
pub fn fallback_order(language: &str) -> [&'static str; 6] {
    match language {
        "zh" | "zh-TW" => [CHINESE_FALLBACK, CJK_FALLBACK, HANGUL_FALLBACK, THAI_FALLBACK, HEBREW_FALLBACK, ARABIC_FALLBACK],
        "ko" => [HANGUL_FALLBACK, CJK_FALLBACK, CHINESE_FALLBACK, THAI_FALLBACK, HEBREW_FALLBACK, ARABIC_FALLBACK],
        "ar" => [ARABIC_FALLBACK, CJK_FALLBACK, CHINESE_FALLBACK, HANGUL_FALLBACK, THAI_FALLBACK, HEBREW_FALLBACK],
        _ => [CJK_FALLBACK, CHINESE_FALLBACK, HANGUL_FALLBACK, THAI_FALLBACK, HEBREW_FALLBACK, ARABIC_FALLBACK],
    }
}
