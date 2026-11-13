//! The list tokens: every size, colour role and duration a list draws
//! with, in one place (list-system spec §2, direction A "Ledger", with
//! direction C's layer and net colours). Colour roles map onto the theme's
//! tokens where one exists; the cross-highlight pair is the one new role,
//! with light and dark variants as the board palette's are. No list
//! hard-codes any of these.
//!
//! The user's row density and reduced-motion setting are published into
//! the egui context once per change ([`publish`]), as the keymap is, so
//! every list reads the same values.
use super::super::severity::{self, Level};
use egui::Color32;
use sparksmith_ui::theme::{self as theme, Tokens};

/// Row density (Settings › Lists). Comfortable adds 6 pt to every row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    #[default]
    Compact,
    Comfortable,
}

/// The user's list settings as the lists read them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prefs {
    pub density: Density,
    /// Every duration is 0: state changes are immediate.
    pub reduced_motion: bool,
}

fn slot() -> egui::Id {
    egui::Id::unique("sparksmith-list-prefs")
}

/// Make `prefs` what every list of `ctx` reads.
pub fn publish(ctx: &egui::Context, prefs: Prefs) {
    ctx.data_mut(|d| d.insert_temp(slot(), prefs));
}

/// The published list settings (the defaults when none were published).
pub fn prefs(ctx: &egui::Context) -> Prefs {
    ctx.data(|d| d.get_temp::<Prefs>(slot()))
        .unwrap_or_default()
}

/// Every list size, colour and duration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListTokens {
    pub row_h: f32,
    pub pad_x: f32,
    pub indent: f32,
    /// Between cells of a row.
    pub gap: f32,
    /// Between table columns.
    pub column_gap: f32,
    pub header_h: f32,
    pub icon: f32,
    pub chip_h: f32,
    pub chip_r: f32,
    /// The selection and cross-highlight bars.
    pub bar_w: f32,
    pub bar_inset: f32,
    pub grip_w: f32,
    pub fold_w: f32,
    pub swatch: f32,
    /// UI text, small text, group captions.
    pub fs: f32,
    pub fs_s: f32,
    pub fs_xs: f32,
    /// Numbers: mono, half a point under the UI size.
    pub fs_num: f32,
    pub caption_tracking: f32,
    /// The most of a row the value column takes.
    pub value_share: f32,
    /// The least of a row the name keeps before the value may grow.
    pub name_min_share: f32,

    pub surface: Color32,
    pub surface_2: Color32,
    pub hover: Color32,
    pub sel: Color32,
    pub sel_line: Color32,
    /// Cross-highlight: never like selection (a bar on the right, in yellow).
    pub xh: Color32,
    pub xh_line: Color32,
    pub fg: Color32,
    pub fg_dim: Color32,
    pub fg_faint: Color32,
    pub line: Color32,
    pub line_strong: Color32,
    pub focus: Color32,
    pub flash: Color32,
    pub shimmer: Color32,
    pub layer_top: Color32,
    pub layer_bottom: Color32,

    /// Seconds; 0 with reduced motion.
    pub motion_fast: f32,
    pub motion: f32,
    pub motion_slow: f32,
    pub motion_flip: f32,
    pub flash_time: f32,
}

impl ListTokens {
    /// The tokens for `ctx`'s published settings and the current theme.
    pub fn get(ctx: &egui::Context) -> Self {
        Self::with(prefs(ctx))
    }

    pub fn with(prefs: Prefs) -> Self {
        let t = Tokens::get();
        let dark = theme::is_dark();
        let board = crate::palette::board();
        let comfortable = if prefs.density == Density::Comfortable {
            6.0
        } else {
            0.0
        };
        let time = |s: f32| if prefs.reduced_motion { 0.0 } else { s };
        let (xh, xh_line) = if dark {
            (
                Color32::from_rgb(58, 49, 19),
                Color32::from_rgb(240, 192, 64),
            )
        } else {
            (
                Color32::from_rgb(255, 241, 191),
                Color32::from_rgb(212, 154, 0),
            )
        };
        Self {
            row_h: 26.0 + comfortable,
            pad_x: 10.0,
            indent: 14.0,
            gap: t.space_6,
            column_gap: t.space_12,
            header_h: 28.0,
            icon: 16.0,
            chip_h: 17.0,
            chip_r: 4.0,
            bar_w: 2.0,
            bar_inset: 4.0,
            grip_w: 12.0,
            fold_w: 14.0,
            swatch: 9.0,
            fs: t.font_label,
            fs_s: t.font_small,
            fs_xs: 10.0,
            fs_num: t.font_label - 0.5,
            caption_tracking: 0.06,
            value_share: 0.45,
            name_min_share: 0.45,

            surface: t.panel,
            surface_2: t.panel_header,
            hover: t.hover,
            sel: t.selected_bg,
            sel_line: t.accent,
            xh,
            xh_line,
            fg: t.form_ink,
            fg_dim: t.form_dim,
            fg_faint: t.text_dim,
            line: t.border,
            line_strong: t.border.gamma_multiply(1.6),
            focus: t.form_primary,
            flash: if dark {
                Color32::from_rgb(90, 71, 16)
            } else {
                Color32::from_rgb(255, 232, 163)
            },
            shimmer: t.hover,
            layer_top: board.copper_top,
            layer_bottom: board.copper_bottom,

            motion_fast: time(0.09),
            motion: time(0.17),
            motion_slow: time(0.26),
            motion_flip: time(0.22),
            flash_time: time(0.9),
        }
    }

    /// A severity's ink and its soft fill (chips, badges).
    pub fn severity(&self, level: Level) -> (Color32, Color32) {
        let ink = severity::ink(level);
        (ink, ink.gamma_multiply(0.18))
    }

    /// A board side's identity colour (C's side swatch).
    pub fn layer(&self, layer: sparksmith_core::Layer) -> Color32 {
        match layer {
            sparksmith_core::Layer::Top => self.layer_top,
            sparksmith_core::Layer::Bottom => self.layer_bottom,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comfortable_adds_six_points_and_reduced_motion_zeroes_durations() {
        let compact = ListTokens::with(Prefs::default());
        let comfy = ListTokens::with(Prefs {
            density: Density::Comfortable,
            reduced_motion: false,
        });
        assert_eq!(compact.row_h, 26.0);
        assert_eq!(comfy.row_h, 32.0);
        let reduced = ListTokens::with(Prefs {
            density: Density::Compact,
            reduced_motion: true,
        });
        assert!(compact.motion > 0.0 && compact.flash_time > 0.0);
        for d in [
            reduced.motion_fast,
            reduced.motion,
            reduced.motion_slow,
            reduced.motion_flip,
            reduced.flash_time,
        ] {
            assert_eq!(d, 0.0);
        }
    }

    #[test]
    fn prefs_published_to_a_context_reach_its_lists() {
        let ctx = egui::Context::default();
        assert_eq!(ListTokens::get(&ctx).row_h, 26.0);
        publish(
            &ctx,
            Prefs {
                density: Density::Comfortable,
                reduced_motion: true,
            },
        );
        assert_eq!(ListTokens::get(&ctx).row_h, 32.0);
        assert_eq!(ListTokens::get(&ctx).motion, 0.0);
    }

    /// Text roles read on the list surface, the hover and the selection;
    /// faint text (units, counts) at least 3:1 (spec §5).
    #[test]
    fn text_roles_keep_their_contrast() {
        for dark in [false, true] {
            theme::set_dark(dark);
            let l = ListTokens::with(Prefs::default());
            for back in [l.surface, l.hover, l.sel] {
                assert!(
                    crate::kit::contrast(l.fg, back) >= 4.5,
                    "fg on {back:?} (dark {dark})"
                );
                assert!(
                    crate::kit::contrast(l.fg_faint, back) >= 3.0,
                    "faint on {back:?} (dark {dark})"
                );
            }
            assert_ne!(l.sel_line, l.xh_line, "cross-highlight is not selection");
        }
        theme::set_dark(false);
    }
}
