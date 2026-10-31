//! Severity glyphs and ink.
//!
//! docs/sidebar-ia.md, "Message style": Error is the only red; glyphs are
//! drawn, never typed as characters, so they look the same at every size and
//! in every font. Adapted from Solidcraft 4f3ac07a
//! `crates/ui-egui/src/ui_kit/status.rs` (`check_status_line`, `CheckStatus`
//! and its `text_marker`) and `ui_kit/checks.rs` (`paint_check_glyph`), MIT
//! option; see ATTRIBUTION.md.
#![allow(dead_code, reason = "slice A1 wires the Issues surface")]

use egui::{Color32, Painter, Pos2, Stroke, vec2};
use pcbcraft_core::issues::Severity;
use solidcraft_ui_egui::theme::Tokens;

/// What a glyph, toast or message means, most severe first. `Ok` is the
/// passed state of a check ("No issues"); the rest follow core's
/// [`Severity`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error,
    Warning,
    ToDo,
    Info,
    Ok,
}

impl From<Severity> for Level {
    fn from(severity: Severity) -> Self {
        match severity {
            Severity::Error => Self::Error,
            Severity::Warning => Self::Warning,
            Severity::ToDo => Self::ToDo,
            Severity::Info => Self::Info,
        }
    }
}

impl Level {
    /// Every level, most severe first.
    pub const ALL: [Self; 5] = [Self::Error, Self::Warning, Self::ToDo, Self::Info, Self::Ok];

    /// The plain-text marker for copied reports and accessible text, where
    /// a glyph cannot be drawn (upstream `CheckStatus::text_marker`).
    pub fn marker(self) -> &'static str {
        match self {
            Self::Error => "×",
            Self::Warning => "!",
            Self::ToDo => "○",
            Self::Info => "i",
            Self::Ok => "✓",
        }
    }

    /// `n` issues of this level in words: "1 error", "36 to do".
    pub fn count_words(self, n: usize) -> String {
        let (one, many) = match self {
            Self::Error => ("error", "errors"),
            Self::Warning => ("warning", "warnings"),
            Self::ToDo => ("to do", "to do"),
            Self::Info => ("note", "notes"),
            Self::Ok => return "No issues".to_owned(),
        };
        format!("{n} {}", if n == 1 { one } else { many })
    }
}

/// The glyph's ink. Only [`Level::Error`] is red. Info's accent is for the
/// glyph only; its text uses `text`.
pub fn ink(level: Level) -> Color32 {
    let t = Tokens::get();
    match level {
        Level::Error => t.error,
        Level::Warning => t.warning,
        Level::ToDo => t.text_dim,
        Level::Info => t.accent,
        Level::Ok => t.form_success,
    }
}

/// The severity glyph (× ! ○ i ✓) in a square of `size` at `center`: a
/// tinted disc with the mark stroked over it; To do is a hollow ring, so it
/// reads as work still open rather than as a fault.
pub fn severity_glyph(painter: &Painter, center: Pos2, size: f32, level: Level) {
    if !(size.is_finite() && size > 0.0) {
        return;
    }
    let ink = ink(level);
    let r = size * 0.5;
    let stroke = Stroke::new((size * 0.12).max(1.0), ink);
    let h = r * 0.45;
    if level == Level::ToDo {
        painter.circle_stroke(center, r * 0.78, stroke);
        return;
    }
    painter.circle_filled(center, r, ink.gamma_multiply(0.18));
    match level {
        Level::Error => {
            painter.line_segment([center + vec2(-h, -h), center + vec2(h, h)], stroke);
            painter.line_segment([center + vec2(-h, h), center + vec2(h, -h)], stroke);
        }
        Level::Warning => {
            painter.line_segment(
                [center + vec2(0.0, -h * 1.2), center + vec2(0.0, h * 0.4)],
                stroke,
            );
            painter.circle_filled(center + vec2(0.0, h * 1.15), stroke.width * 0.7, ink);
        }
        Level::Info => {
            painter.circle_filled(center + vec2(0.0, -h * 1.15), stroke.width * 0.7, ink);
            painter.line_segment(
                [center + vec2(0.0, -h * 0.3), center + vec2(0.0, h * 1.2)],
                stroke,
            );
        }
        Level::Ok | Level::ToDo => {
            painter.line_segment(
                [center + vec2(-h, 0.0), center + vec2(-h * 0.2, h * 0.8)],
                stroke,
            );
            painter.line_segment(
                [center + vec2(-h * 0.2, h * 0.8), center + vec2(h, -h * 0.8)],
                stroke,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_core_severity_most_severe_first() {
        let mapped = [
            Severity::Error,
            Severity::Warning,
            Severity::ToDo,
            Severity::Info,
        ]
        .map(Level::from);
        assert_eq!(
            mapped,
            [Level::Error, Level::Warning, Level::ToDo, Level::Info]
        );
        assert!(Level::ALL.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn error_is_the_only_red() {
        let t = Tokens::get();
        for level in Level::ALL {
            assert_eq!(ink(level) == t.error, level == Level::Error, "{level:?}");
        }
        assert_eq!(ink(Level::ToDo), t.text_dim);
    }

    #[test]
    fn counts_read_in_plain_words() {
        assert_eq!(Level::Error.count_words(1), "1 error");
        assert_eq!(Level::Warning.count_words(2), "2 warnings");
        assert_eq!(Level::ToDo.count_words(36), "36 to do");
        assert_eq!(Level::Ok.count_words(0), "No issues");
        let markers: Vec<_> = Level::ALL.iter().map(|l| l.marker()).collect();
        assert_eq!(markers, ["×", "!", "○", "i", "✓"]);
    }

    /// Glyphs are drawn, never typed, and every level draws something.
    #[test]
    fn glyphs_are_drawn_shapes_not_text() {
        let ctx = egui::Context::default();
        for level in Level::ALL {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                severity_glyph(ui.painter(), egui::pos2(20.0, 20.0), 14.0, level);
                // A degenerate size draws nothing rather than a broken mark.
                severity_glyph(ui.painter(), egui::pos2(60.0, 20.0), f32::NAN, level);
            });
            output.textures_delta.clear();
            let shapes: Vec<_> = output
                .shapes
                .iter()
                .filter(|s| !matches!(s.shape, egui::Shape::Noop))
                .collect();
            assert!(!shapes.is_empty(), "{level:?} drew nothing");
            assert!(
                shapes
                    .iter()
                    .all(|s| !matches!(s.shape, egui::Shape::Text(_))
                        && s.shape.visual_bounding_rect().max.x < 40.0),
                "{level:?}"
            );
        }
    }
}
