//! Pcbcraft's UI toolkit. It starts as SolidCraft's MIT/Apache interface code as Pcbcraft draws
//! with it: the UI kit's controls, the theme tokens, the icon set and code-drawn glyphs, the
//! bundled IBM Plex faces, the move triad and a few shell helpers. The project's own design
//! language grows from here; the look is unchanged for now.
//!
//! # Source and licence
//!
//! Vendored from SolidCraft (<https://github.com/tangency-labs/solidcraft>) as built from the fork
//! <https://github.com/DAmesberger/solidcraft> at revision
//! `64bebfef0a77dab8c79666114c7601c9dda55d12`: upstream `a34756830185f82da3a0e3ddd1ea25ef21111558`,
//! the last revision before SolidCraft's brand licence, plus the fork's own commits. At that
//! revision `crates/ui-kit`, `crates/theme` and `crates/ui-egui` are licensed under the MIT
//! License or the Apache License 2.0, at your option; copyright (c) 2026 SolidCraft contributors.
//! `LICENSE-MIT` and `LICENSE-APACHE` beside this crate carry those notices. The IBM Plex fonts in
//! `assets/fonts` are under the SIL Open Font License 1.1 (the `OFL.txt` beside each face).
//!
//! Only what Pcbcraft uses is kept, and the modules below name where each part came from
//! (`ATTRIBUTION.md` at the repository root has the map). Pcbcraft does not link SolidCraft's
//! brand-licensed crates (`solidcraft-ui-kit`, `solidcraft-look`, `solidcraft-theme`) or
//! `solidcraft-ui-egui`; `cargo xtask brand-crates` holds that.
//!
//! Everything the dead-code analysis could remove without breaking the build has been removed.
//! What it still reports (methods of types it believes are never built, helpers of those) is
//! reached through code the analysis cannot follow, so it stays as upstream wrote it.
#![allow(dead_code, reason = "vendored: pruned to what Pcbcraft reaches; see the crate documentation")]

mod box_select;
mod chrome;
mod dim_label;
mod font_setup;
mod icon_paint;
mod icon_set;
mod kit;
mod proj;
mod tokens;
mod triad;

/// The token contract every control draws with (SolidCraft's `crates/theme`).
pub mod theme {
    pub use crate::tokens::{Tokens, apply, is_dark, set_dark};
}

/// The UI kit's controls (SolidCraft's `crates/ui-kit`).
pub mod ui_kit {
    pub use crate::kit::actions::{Button, button};
    pub use crate::kit::container::{Container, Edge, Surface};
    pub use crate::kit::fields::cell_editor;
    pub use crate::kit::menu::{Item, LIST_W};
    pub use crate::kit::panel::Window;

    /// Weighted interface type (SolidCraft's `ui-kit/src/type_face.rs`).
    pub mod type_face {
        pub use crate::kit::type_face::{bold, medium, semibold};
    }

    /// Menu rows and frames (SolidCraft's `ui-kit/src/menu.rs`).
    pub mod menu {
        pub use crate::kit::menu::{list_row, menu_frame};
    }
}

/// Docked panel sides (SolidCraft's `ui-kit/src/frame.rs`).
pub mod frame {
    pub use crate::kit::frame::Side;
}

/// The bundled IBM Plex faces and system fallbacks (SolidCraft's `ui-egui/src/fonts.rs`).
pub mod fonts {
    pub use crate::font_setup::{load_system_fonts_in_background, setup_bundled_fonts, setup_fonts_without_waiting, system_fonts_ready};
}

/// Icon painters and the code-drawn glyphs (SolidCraft's `ui-egui/src/icons.rs`).
pub mod icons {
    pub use crate::icon_paint::{paint_icon, paint_on};
}

/// SolidCraft's icon set (SolidCraft's `ui-egui/src/icon_svg.rs` and `build.rs`).
pub mod icon_svg {
    pub use crate::icon_set::{install, resolve};
}

/// The move/rotate triad (SolidCraft's `ui-egui/src/gizmo.rs`).
pub mod gizmo {
    pub use crate::triad::{Change, Triad, handle_point, show};
}

/// The viewport's world ↔ screen mapping (SolidCraft's `ui-egui/src/viewport.rs`).
pub mod viewport {
    pub use crate::proj::Proj;
}

/// Window and crossing box selection (SolidCraft's `ui-egui/src/selection.rs`).
pub mod selection {
    pub use crate::box_select::BoxSel;
}

/// A length dimension's label (SolidCraft's `ui-egui/src/dim_view.rs`).
pub mod dim_view {
    pub use crate::dim_label::length_label;
}

/// Menu placement (SolidCraft's `ui-egui/src/context_menu.rs`).
pub mod context_menu {
    pub use crate::chrome::place;
}

/// Shortcut names (SolidCraft's `ui-egui/src/keymap.rs`).
pub mod keymap {
    pub use crate::chrome::key_name;
}

/// Command-search scoring (SolidCraft's `ui-egui/src/shortcut_box.rs`).
pub mod shortcut_box {
    pub use crate::chrome::score;
}

/// The ribbon and application bar heights (SolidCraft's `ui-egui/src/toolbar.rs`).
pub mod toolbar {
    pub use crate::chrome::{APP_BAR_HEIGHT, RIBBON_HEIGHT, TAB_ROW_HEIGHT};
}

/// The window chrome's traffic-light room (SolidCraft's `ui-kit/src/window_chrome.rs`).
pub mod window_chrome {
    pub use crate::chrome::TRAFFIC_LIGHTS;
}
