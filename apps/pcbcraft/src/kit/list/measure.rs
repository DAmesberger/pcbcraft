//! Measured widths: the decimal-aligned number slots and the value column,
//! measured over every row (not only the shown ones), so values line up
//! and are never cut. [`split`] is pure; the rest asks egui's fonts.
use super::tokens::ListTokens;
use egui::{FontId, Ui};

/// A number as shown in three slots: integer part, fraction and unit
/// ("-12" ".700" "mm"). Values line up on the decimal point.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Number<'a> {
    pub int: &'a str,
    pub frac: &'a str,
    pub unit: &'a str,
}

/// Split a shown value ("12.7 mm", "-3 °", "90°") into its slots; `None`
/// for text that does not start with a number ("Held", "width / 2").
pub fn split(text: &str) -> Option<Number<'_>> {
    let text = text.trim();
    let bytes = text.as_bytes();
    let i = if text.starts_with(['-', '+']) {
        1
    } else if text.starts_with('\u{2212}') {
        '\u{2212}'.len_utf8()
    } else {
        0
    };
    let digits = |from: usize| {
        bytes
            .get(from..)
            .map_or(0, |b| b.iter().take_while(|c| c.is_ascii_digit()).count())
    };
    let int = digits(i);
    if int == 0 {
        return None;
    }
    let int_end = i + int;
    let mut end = int_end;
    if bytes.get(end) == Some(&b'.') {
        end += 1 + digits(end + 1);
    }
    let unit = text.get(end..)?.trim();
    // A unit is short and has no spaces or operators ("width / 2" is not one).
    if unit.len() > 8 || unit.contains(|c: char| c.is_whitespace() || "+-*/()".contains(c)) {
        return None;
    }
    Some(Number {
        int: text.get(..int_end)?,
        frac: text.get(int_end..end)?,
        unit,
    })
}

/// The slot widths shared by every number of one column.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Slots {
    pub int: f32,
    pub frac: f32,
    pub unit: f32,
}

impl Slots {
    pub fn width(&self, gap: f32) -> f32 {
        self.int
            + self.frac
            + if self.unit > 0.0 {
                gap + self.unit
            } else {
                0.0
            }
    }
}

fn width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    ui.painter()
        .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
        .size()
        .x
}

/// The number font of a list.
pub fn mono(l: &ListTokens) -> FontId {
    FontId::monospace(l.fs_num)
}

/// The slots that fit every one of `values`: the fraction slot is at least
/// `.000` wide, so values line up even when they differ in decimals.
///
/// Figures are tabular mono, so the widest integer and fraction are the
/// ones with the most characters: only those (per leading sign, which may
/// come from another font) and each distinct unit are laid out, and a
/// column of 10 000 vias costs a handful of layouts.
pub fn slots<'a>(ui: &Ui, l: &ListTokens, values: impl IntoIterator<Item = &'a str>) -> Slots {
    let font = mono(l);
    let small = FontId::monospace(l.fs_s);
    // The longest text per (slot, leading character), by characters.
    let mut longest: std::collections::HashMap<(u8, Option<char>), &str> = Default::default();
    let mut keep = |slot: u8, text: &'a str| {
        if text.is_empty() {
            return;
        }
        let at = longest.entry((slot, text.chars().next())).or_insert(text);
        if text.chars().count() > at.chars().count() {
            *at = text;
        }
    };
    let mut units: Vec<&str> = Vec::new();
    for value in values {
        match split(value) {
            Some(n) => {
                keep(0, n.int);
                keep(1, n.frac);
                if !units.contains(&n.unit) && units.len() < 64 {
                    units.push(n.unit);
                }
            }
            None => keep(0, value.trim()),
        }
    }
    let mut s = Slots {
        frac: width(ui, ".000", &font),
        ..Default::default()
    };
    for ((slot, _), text) in longest {
        let w = width(ui, text, &font);
        if slot == 0 {
            s.int = s.int.max(w);
        } else {
            s.frac = s.frac.max(w);
        }
    }
    s.unit = widest(ui, &small, units);
    s
}

/// The widest of `texts` in `font`.
pub fn widest<'a>(ui: &Ui, font: &FontId, texts: impl IntoIterator<Item = &'a str>) -> f32 {
    texts
        .into_iter()
        .map(|t| width(ui, t, font))
        .fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_split_on_the_decimal_point() {
        let n = |int, frac, unit| Some(Number { int, frac, unit });
        assert_eq!(split("12.700 mm"), n("12", ".700", "mm"));
        assert_eq!(split("-3 mm"), n("-3", "", "mm"));
        assert_eq!(split("90°"), n("90", "", "°"));
        assert_eq!(split("\u{2212}1.5 mm"), n("\u{2212}1", ".5", "mm"));
        assert_eq!(split("4.7"), n("4", ".7", ""));
        assert_eq!(split("Held"), None);
        assert_eq!(split("width / 2"), None);
        assert_eq!(split("2 * pitch"), None);
        assert_eq!(split(""), None);
    }
}
