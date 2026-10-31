//! Severity glyphs and ink (slice K2 owns the final look).
//!
//! Contract from slice S0 (docs/sidebar-ia.md, "Message style"): Error is
//! the only red; glyphs are drawn, never typed as characters. Adapted from
//! Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/status.rs`
//! (`check_status_line`, `CheckStatus`), MIT option; see ATTRIBUTION.md.
#![allow(dead_code, reason = "S0 contract; slices K2 and A1 use it")]

use egui::{Color32, Painter, Pos2, Stroke, vec2};
use pcbcraft_core::issues::Severity;
use solidcraft_ui_egui::theme::Tokens;

/// What a glyph, toast or message means. `Ok` is the passed state of a
/// check ("No issues"); the rest follow core's [`Severity`].
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

/// The glyph's ink. Info's accent is for the glyph only; its text uses
/// `text`.
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

/// The severity glyph (× ! ○ i ✓) in a square of `size` at `center`.
pub fn severity_glyph(painter: &Painter, center: Pos2, size: f32, level: Level) {
    let ink = ink(level);
    let r = size * 0.5;
    let stroke = Stroke::new((size * 0.12).max(1.0), ink);
    let h = r * 0.45;
    match level {
        Level::ToDo => {
            painter.circle_stroke(center, r * 0.8, stroke);
        }
        Level::Error => {
            painter.circle_filled(center, r, ink.gamma_multiply(0.18));
            painter.line_segment([center + vec2(-h, -h), center + vec2(h, h)], stroke);
            painter.line_segment([center + vec2(-h, h), center + vec2(h, -h)], stroke);
        }
        Level::Warning | Level::Info => {
            painter.circle_filled(center, r, ink.gamma_multiply(0.18));
            let (top, dot) = if level == Level::Warning {
                (center + vec2(0.0, -h * 1.2), center + vec2(0.0, h * 1.3))
            } else {
                (center + vec2(0.0, -h * 0.2), center + vec2(0.0, -h * 1.3))
            };
            let bottom = if level == Level::Warning {
                center + vec2(0.0, h * 0.4)
            } else {
                center + vec2(0.0, h * 1.2)
            };
            painter.line_segment([top, bottom], stroke);
            painter.circle_filled(dot, stroke.width * 0.7, ink);
        }
        Level::Ok => {
            painter.circle_filled(center, r, ink.gamma_multiply(0.18));
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
