//! SolidCraft's icon set: the SVG masters in `assets/icons/ui/design/svg`, drawn by the kit's
//! renderer ([`crate::kit::icons`]).
//!
//! The masters, `palette.toml` (every colour role in light and dark, and the accents) and
//! `aliases.toml` are embedded at build time (see `build.rs`) as the [`SET`] the app [`install`]s
//! at startup and in its test contexts. The masters are a small, linted SVG subset
//! (`tools/lint_icons.py`); a master never carries a colour of its own: it names a palette role.
//! `palette.toml` owns defaults; theme files may override display roles and semantic ramps.
//!
//! The lookups here ([`resolve`], [`doc_for`], …) read SolidCraft's set whether or not it is
//! installed; `every_master_parses` holds that no master falls back to the placeholder.

pub use crate::kit::icons::Resolved;
use crate::kit::icons::{self, IconData, IconSet};

include!(concat!(env!("OUT_DIR"), "/icon_data.rs"));

const _: () = assert!(
    ROLE_NAMES.len() <= icons::MAX_ROLES && ACCENT_NAMES.len() <= icons::MAX_ACCENTS,
    "a theme file must be able to override every role and accent"
);

/// SolidCraft's icons.
pub static SET: IconSet = IconSet::new(IconData {
    role_names: ROLE_NAMES,
    role_source_hex: ROLE_SOURCE_HEX,
    palette: PALETTE,
    accent_names: ACCENT_NAMES,
    accents: ACCENTS,
    accent_variants: ACCENT_VARIANTS,
    masters: SVGS,
    aliases: ALIASES,
    icon_accents: ICON_ACCENTS,
    danger_accent: "constraint",
});

/// Make SolidCraft's icons and code-drawn glyphs ([`crate::icon_paint::GLYPHS`]) the ones the kit draws:
/// at startup, before the first frame, and in every test context. Calling it again does nothing.
pub fn install() {
    icons::install(&SET);
    icons::glyphs::install(&crate::icon_paint::GLYPHS);
}

/// The master and accent for an app icon id: an alias (with its accent), a master of that name,
/// or the placeholder.
pub fn resolve(name: &str) -> Resolved {
    SET.resolve(name)
}
