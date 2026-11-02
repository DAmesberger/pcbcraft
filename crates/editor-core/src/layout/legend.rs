//! Legend text as strokes: Pcbcraft's stroke font laid out by the core's
//! text metric (each line in its box, the upright rule applied, bottom-side
//! text reflected within its line). The silkscreen films in 3D and the
//! Gerber legend layers both draw these strokes, so both print the same ink.
use crate::geometry::{Point, cos_sin};
use crate::library::graphics::{HAlign, PlacedText, TextMetric};

mod font;

pub use font::{GRID_HEIGHT, GRID_WIDTH, glyph};

/// Text stroke width as a share of the cap height, at least the thinnest
/// legend line fabricators print (est. 0.1 mm).
pub const TEXT_STROKE_SHARE: f64 = 1.0 / 7.0;
pub const MIN_STROKE_MM: f64 = 0.1;
/// Glyph width inside its advance, and its left bearing, as cap-height
/// shares (the core metric's advance is 0.6).
pub const GLYPH_WIDTH_SHARE: f64 = 0.42;
pub const GLYPH_BEARING_SHARE: f64 = 0.09;

/// One placed text as centre lines of a round pen `width` wide. A stroke
/// of one point is a dot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextStrokes {
    pub width: f64,
    pub strokes: Vec<Vec<Point>>,
}

/// The pen strokes of placed legend text in board coordinates. Characters
/// without a glyph print nothing but keep their advance; text of no
/// positive finite height prints nothing.
pub fn text_strokes(text: &PlacedText) -> TextStrokes {
    let h = text.height;
    if !(h > 0.0 && h.is_finite()) {
        return TextStrokes::default();
    }
    let advance = TextMetric::default().advance * h;
    let width = (TEXT_STROKE_SHARE * h).max(MIN_STROKE_MM);
    let (rotation_deg, lines) = text.lines();
    let justify = text.line_justify();
    let (c, s) = cos_sin(rotation_deg);
    let sx = GLYPH_WIDTH_SHARE * h / GRID_WIDTH;
    let sy = h / GRID_HEIGHT;
    let mut strokes = Vec::new();
    for line in lines {
        let count = line.text.chars().count() as f64;
        let line_width = count * advance;
        let left = match justify.h {
            HAlign::Left => 0.0,
            HAlign::Center => -line_width / 2.0,
            HAlign::Right => -line_width,
        };
        let middle = left + line_width / 2.0;
        for (k, ch) in line.text.chars().enumerate() {
            let Some(glyph) = font::glyph(ch) else {
                continue;
            };
            let x0 = left + k as f64 * advance + GLYPH_BEARING_SHARE * h;
            for stroke in glyph {
                strokes.push(
                    stroke
                        .iter()
                        .map(|&(gx, gy)| {
                            let mut x = x0 + gx * sx;
                            // Bottom-side legend reads from below: reflected
                            // within its line's box.
                            if text.mirrored {
                                x = 2.0 * middle - x;
                            }
                            let y = gy * sy - h / 2.0;
                            [line.at[0] + c * x - s * y, line.at[1] + s * x + c * y]
                        })
                        .collect(),
                );
            }
        }
    }
    TextStrokes { width, strokes }
}
