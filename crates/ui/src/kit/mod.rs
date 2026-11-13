//! The UI kit: tokenised dense controls (rows, fields, menus, panels, windows), drawn from plain
//! data the host passes or installs (its icon set, glyphs and text), returning what the user chose.
//! Geometry, typography and colours come from [`crate::tokens::Tokens`].
//!
//! Vendored from SolidCraft's `crates/ui-kit` (see the crate documentation for the source revision
//! and licence); only the controls Sparksmith draws with are kept.

pub(crate) mod frame;
pub(crate) mod icons;
pub(crate) mod menu;
pub(crate) mod type_face;

pub(crate) mod actions;
pub(crate) mod assistant;
pub(crate) mod assistant_entry;
pub(crate) mod attention;
pub(crate) mod blur;
pub(crate) mod checks;
pub(crate) mod choice;
pub(crate) mod color;
pub(crate) mod compare;
pub(crate) mod confirm;
pub(crate) mod container;
pub(crate) mod dash;
pub(crate) mod details;
pub(crate) mod disclosure;
pub(crate) mod drag;
pub(crate) mod fields;
pub(crate) mod file_view;
pub(crate) mod framing;
pub(crate) mod history;
pub(crate) mod history_group;
pub(crate) mod layout;
pub(crate) mod look_chain;
pub(crate) mod metrics;
pub(crate) mod name_list;
pub(crate) mod navigation;
pub(crate) mod navigation_menu;
pub(crate) mod navigation_settings;
pub(crate) mod panel;
pub(crate) mod panel_host;
pub(crate) mod preferences;
pub(crate) mod preferences_cards;
pub(crate) mod preferences_layout;
pub(crate) mod preferences_small;
pub(crate) mod radial;
pub(crate) mod recent_colors;
pub(crate) mod reports;
pub(crate) mod review;
pub(crate) mod search;
pub(crate) mod settings_search;
pub(crate) mod shortcuts;
pub(crate) mod sphere;
pub(crate) mod stages;
pub(crate) mod status;
pub(crate) mod table;
pub(crate) mod text;
pub(crate) mod text_menu;
pub(crate) mod theme_files;
pub(crate) mod theme_options;
pub(crate) mod tree;
pub(crate) mod value_focus;
pub(crate) mod viewport_status;
pub(crate) mod wheel;

pub(crate) use actions::*;
pub(crate) use checks::*;
pub(crate) use confirm::Confirm;
pub(crate) use container::input_blocked;
pub(crate) use container::{BackingTooltipExt, Container, DockEdges, LayoutMode, Surface};
pub(crate) use disclosure::*;
pub(crate) use fields::*;
pub(crate) use file_view::Icon;
pub(crate) use layout::*;
pub(crate) use menu::LIST_W;
pub(crate) use menu::{Menu, MenuItem};
pub(crate) use navigation::*;
pub(crate) use navigation_menu::*;
pub(crate) use panel::Window;
pub(crate) use panel::panel_header_hover;
pub(crate) use preferences::*;
pub(crate) use preferences_small::*;
pub(crate) use reports::*;
pub(crate) use shortcuts::*;
pub(crate) use sphere::{FinishKind, Sphere, sphere_thumbnail};

pub(crate) use layout::label as row_label;
