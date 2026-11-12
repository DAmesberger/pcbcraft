//! SolidCraft's icon artwork, from `crates/ui-egui/src/icons.rs`.
//!
//! Command, menu and browser icons are SolidCraft's SVG masters ([`crate::icon_set::SET`]), drawn
//! by the kit's painters ([`crate::kit::icons`], re-exported here): colours come from the
//! default `palette.toml` roles/accents, never arbitrary caller tints. An icon with no master yet
//! draws the placeholder.
pub use crate::kit::icons::{paint_icon, paint_on};

#[path = "glyph_art/glyphs.rs"]
pub(crate) mod glyphs;

/// SolidCraft's code-drawn glyphs, as the kit draws them ([`crate::kit::icons::glyphs`]).
pub static GLYPHS: crate::kit::icons::glyphs::Glyphs = crate::kit::icons::glyphs::Glyphs {
    transport: glyphs::transport_glyph,
    badge: glyphs::badge,
    pin: glyphs::pin_icon,
    cross: glyphs::cross,
    breadcrumb: glyphs::breadcrumb,
};
