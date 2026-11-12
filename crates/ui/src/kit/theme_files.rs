//! Colour and shape themes as files (#2255): a `solidcraft-theme/1` JSON file names a base palette (the
//! built-in Dark or Light) and sets any of its [`Tokens`] by name, so a theme can be a handful of
//! lines. Built-in themes are such files a host ships and installs ([`install_builtin`]);
//! imported ones a host keeps in its preferences, so they also work in the browser. Colour vision modes still apply on top of a theme (`Tokens::with_vision`).
//!
//! ```json
//! {"format": "solidcraft-theme/1", "name": "Mine", "base": "dark",
//!  "colors": {"panel": "#282828", "axis_x": "#fb4934", "axis_y": "#b8bb26", "axis_z": "#83a598"}}
//! ```

use std::collections::BTreeMap;

use egui::Color32;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tokens::{IconColors, Tokens};

// Older exports contain colours for retired window bands and procedural icons.
// Discard them at import; they have no token, renderer, or exported setting now.

/// The palette a theme starts from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Base {
    #[default]
    Dark,
    Light,
}

impl Base {}

/// A theme: identity, base value tables and optional declarations selecting literal overlays.
/// Colour overrides are `#rrggbb` / `#rrggbbaa` strings or lists, keyed by [`keys`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeFile {
    pub id: String,
    pub name: String,
    pub base: Base,
    pub colors: BTreeMap<String, Value>,
    /// Optional corner radii in logical UI points. Missing entries keep the base values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub radii: BTreeMap<String, u8>,
    /// Optional border roles; absent entries retain the existing surface-specific styling.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub borders: BTreeMap<String, Value>,
    /// SVG display roles and three-colour semantic ramps; omitted values keep compiled defaults.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub icon_palette: BTreeMap<String, Value>,
    /// Optional surface overrides for known ribbon tabs. Accents retain their existing tokens.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tabs: BTreeMap<String, BTreeMap<String, Value>>,
    /// Theme-owned declarations, in file order; standard ids are normalized on import.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<OptionDef>,
    /// Conditional value tables, applied in file order with later entries winning by key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<Overlay>,
    /// Levels newer than [`REQUIREMENTS_LEVEL`] disable options and overlays.
    #[serde(default = "default_requirements_level", skip_serializing_if = "is_default_requirements_level")]
    pub requires: u64,
}

fn default_requirements_level() -> u64 {
    1
}

fn is_default_requirements_level(level: &u64) -> bool {
    *level == 1
}

impl Default for ThemeFile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            base: Base::default(),
            colors: BTreeMap::new(),
            radii: BTreeMap::new(),
            borders: BTreeMap::new(),
            icon_palette: BTreeMap::new(),
            tabs: BTreeMap::new(),
            options: Vec::new(),
            overlays: Vec::new(),
            requires: default_requirements_level(),
        }
    }
}

/// Saved option values use the same JSON primitives as defaults and comparisons.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OptionValue {
    Bool(bool),
    Choice(String),
    Num(f32),
}

/// Localized text belongs to the theme, with English as the fallback.
pub type OptionText = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceValue {
    pub id: String,
    pub label: OptionText,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OptionKind {
    Bool {
        default: bool,
    },
    Choice {
        default: String,
        values: Vec<ChoiceValue>,
    },
    Range {
        default: f32,
        min: f32,
        max: f32,
        step: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
}

/// Explicit theme-local continuous effects; custom option ids and ordering remain authored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionEffect {
    SurfaceBrightness,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OptionDef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<OptionEffect>,
    #[serde(flatten)]
    pub kind: OptionKind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub label: OptionText,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub help: OptionText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub advanced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Condition {
    Eq {
        opt: String,
        eq: OptionValue,
    },
    Range {
        opt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gte: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lte: Option<f32>,
    },
}

impl Condition {}

/// Overlay tables have exactly the same keys and bounds as the theme's base tables.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThemeLayer {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub radii: BTreeMap<String, u8>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub borders: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub icon_palette: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    pub when: Condition,
    #[serde(flatten)]
    pub values: ThemeLayer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionNoticeKind {
    UnknownOption,
    WrongType,
    UnknownChoice,
    NonFinite,
    Clamped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionNotice {
    pub id: String,
    pub kind: OptionNoticeKind,
}

impl ThemeFile {}

/// The settable colours: a key, and where it lives in [`Tokens`]. `one` keys take a colour,
/// `many` keys a list of exactly as many colours as the token has; `icons.*` are the colours
/// viewport glyphs draw of their own. The destructuring below fails to build when a token is added
/// without a key here, so every colour stays themeable.
macro_rules! theme_keys {
    (one: [$($one:ident),* $(,)?], many: [$($many:ident),* $(,)?],
     icons: [$($ione:ident),* $(,)?] $(,)?) => {
        /// Every key a theme file may set, in the order an export writes them.
        pub fn keys() -> Vec<&'static str> {
            let mut k = vec![$(stringify!($one),)* $(stringify!($many),)*];
            k.extend([$(concat!("icons.", stringify!($ione)),)*]);
            k
        }

        /// Stable identity of resolved colours, including custom themes with the same base/id.
        /// Share the token inventory with import/export so added colour tokens cannot be missed.
        pub fn color_key(t: &Tokens) -> u64 {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            $(t.$one.hash(&mut h);)*
            $(t.$many.hash(&mut h);)*
            $(t.icons.$ione.hash(&mut h);)*
            h.finish()
        }

        pub fn slot<'a>(t: &'a mut Tokens, key: &str) -> Option<&'a mut [Color32]> {
            match key {
                $(stringify!($one) => Some(std::slice::from_mut(&mut t.$one)),)*
                $(stringify!($many) => Some(&mut t.$many[..]),)*
                $(concat!("icons.", stringify!($ione)) => Some(std::slice::from_mut(&mut t.icons.$ione)),)*
                _ => None,
            }
        }

        #[allow(dead_code)]
        fn every_colour_has_a_key(t: &Tokens) {
            // Not colours: the hover lift is a grey step, the analysis colours belong to shaders.
            let Tokens {
                $($one: _,)* $($many: _,)*
                hover_face_lift: _, analysis: _, icons: _, borders: _,
                file_action_height: _, file_back_height: _, file_recovery_icon: _, file_grid_gap: _,
                file_nav_width: _, file_nav_height: _, file_tile_height: _, file_tile_icon: _, file_recent_height: _,
                file_sample_image_height: _, file_content_inset_x: _, file_content_inset_y: _, file_section_gap: _, font_start_title: _,
                space_2: _, space_3: _, space_4: _, space_6: _, space_8: _, space_10: _,
                space_12: _, space_14: _, space_16: _, space_24: _, radius_sm: _, radius_md: _,
                radius_lg: _, radius_xl: _, font_badge: _, font_micro: _, font_panel: _, font_small: _,
                font_caption: _, font_body: _, font_label: _, font_button: _, font_heading: _, font_triad: _,
                font_compass: _, font_subtitle: _, font_intro: _, font_page_heading: _, font_article_heading: _, font_title: _, font_display: _,
                control_height: _,  control_radius: _, form_label_width: _, form_gap_y: _, form_footer_height: _, form_header_height: _,
                form_card_width: _, form_icon_size: _, form_close_size: _, form_inset: _, form_icon_inset: _, form_section_icon_size: _,
                form_checkbox_height: _, form_button_height: _, sketch_palette_width: _, sketch_mode_size: _, sketch_mode_radius: _, sketch_mode_border: _,
                sketch_palette_alpha: _, sketch_mode_tint: _, sketch_footer_icon_size: _, font_section_tracking: _,
                figure_ground: _, figure_line: _, figure_highlight: _, figure_shadow: _, figure_face_top: _, figure_face_left: _, figure_face_right: _, figure_card: _, figure_white: _, figure_black: _, figure_accent: _, figure_reference: _, figure_axis_x: _, figure_axis_y: _, figure_axis_z: _, figure_ok: _, figure_warn: _, figure_conflict: _, pref_well: _, pref_note: _, pref_note_fill: _, pref_on_accent: _, pref_ok: _, pref_ok_fill: _, pref_disabled: _,
            } = t;
            let IconColors { $($ione: _,)* } = &t.icons;
        }
    };
}

theme_keys! {
    one: [
        nav_hover, nav_press, nav_dim, nav_viewport, nav_tip, nav_tip_ink, nav_tip_key_border, nav_field,
        file_dim, file_recovery_well, file_background, file_surface, file_border, file_image_tint, file_rail, file_rail_ink, file_rail_hover,
        file_primary, file_primary_ink, file_primary_border, file_recovery, file_recovery_ink, file_recovery_border,
        app_bar, toolbar, panel, panel_header, border, text, text_dim, accent, accent_soft, sketch_accent, error, warning,
        icon, timeline, hover, sel_face, sel_edge, sel_edge_rim, hover_edge, hover_edge_halo, sel_profile,
        sel_profile_edge, hover_profile, hover_profile_edge, sel_vertex, origin_plane, origin_plane_edge, origin_plane_hover,
        origin_point, axis_x, axis_y, axis_z, construction_plane, construction_geom, body, body_edge, manipulator, joint_manipulator, joint_axis,
        preview_add, preview_new, preview_sketch, preview_cut, cube_face, cube_border, cube_text, cube_hover, viewport_top,
        viewport_bottom, grid, grid_major, body_selected, sketch, sketch_fixed, sketch_halo, sketch_locked, sketch_conflict, dimension, dimension_driven,
        sketch_point, sketch_point_free, dialog_bg, dialog_border, chip, section_cap,
        profile_fill, glyph_bg, glyph_edge, glyph_text, overlay, rubber_band, app_bar_text, app_bar_hover, timeline_item,
        timeline_item_error, timeline_marker, field, box_window, box_crossing, sketch_projected, sketch_lost,
        sketch_construction, sketch_centerline, bend_up, bend_down, interference, comb_envelope, comb_teeth,
        sketch_ink, sketch_soft, surface_fill, surface_ink, surface_soft, mesh_fill, mesh_ink, mesh_soft, form_fill, form_tab_ink, form_soft, sheet_fill, sheet_ink, sheet_soft, render_fill, render_ink, render_soft, tab_on_fill_dark,
        app_bar_separator, surface_accent, mesh_accent, form_accent, render_accent, on_accent, agent_ok, agent_busy, app_bar_hover_subtle,
        app_bar_hover_button, app_bar_hover_strong, app_bar_tab_active,
        form_ink, form_dim, form_primary, form_primary_fill, form_error, form_error_fill, form_success, form_success_fill, dialog_body, dialog_footer, dialog_rule, control_fill, control_border, dialog_label, selected_ink, selected_bg,
    ],
    many: [continuity, logo, section_hatch],
    icons: [highlight, agent, marker_halo],
}

// The application mark now has three faces and no outline. Older theme exports and
// saved preferences carry a fourth, retired outline colour; keep their face colours.

impl OptionDef {}

/// A theme's base tables with its matching overlays merged in, and the options and values that chose them.
struct Merged {
    file: ThemeFile,
    options: Vec<OptionDef>,
    values: BTreeMap<String, OptionValue>,
}

/// A theme file the desktop app follows (#7573): another tool writes it, and the app reloads it
/// when its modified time or size changes. Only a read that parses completely is applied; while
/// the file is being written, broken, missing or too big, the last good theme stays on and
/// [`Follow::error`] says why.
#[derive(Clone, Debug, Default)]
pub struct Follow {
    /// The `--theme-file` launch flag's path: followed for this run in place of the preference,
    /// never saved.
    pub launch: Option<String>,
    /// The path the state below belongs to.
    path: String,
    /// The file's modified time and size at the last read.
    stamp: Option<(Option<std::time::SystemTime>, u64)>,
    /// When (`egui` input time, seconds) the file was last looked at.
    checked: Option<f64>,
    /// The last theme that read completely.
    theme: Option<ThemeFile>,
    /// Why the last read was not applied.
    error: Option<String>,
    /// Whether the window had the focus last frame (gaining it re-reads the file).
    pub focused: bool,
}

/// What a [`Follow::poll`] changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowChange {
    /// Nothing new.
    None,
    /// The file read completely; its theme is on now.
    Loaded(String),
    /// A read failed for a new reason (reported once; the last good theme stays on).
    Failed(String),
}

impl Follow {}
