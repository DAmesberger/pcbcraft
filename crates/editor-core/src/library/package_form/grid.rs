//! Slice B2c: the grid array package family: BGA, LGA and WLCSP, with the
//! depopulation mask and the row-letter alphabet (A1 numbering) as data.
//! The form, its range rules and lead order live here; the footprint
//! template is `templates/footprint_family/grid.rs` and the model builder
//! `pcbcraft-solidcraft` `models/grid.rs`.
//!
//! Frame: x along the body length (columns, numbered 1, 2, … from −x), y
//! along the body width (rows, lettered from +y), seen from above. Ball A1
//! is at the −x, +y corner. Lead order is row-major from A1: row A from
//! −x to +x, then row B, and so on, leaving out depopulated positions. A
//! depopulated position keeps its name, so the remaining balls keep theirs
//! (A1, A3, … when A2 is empty).
use super::{FORM_LENGTH_LIMIT_MM, FORM_LENGTH_MIN_MM, FamilyForm};
use crate::geometry::Point3;
use crate::library::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GridContact {
    Ball,
    Land,
}

/// Bytes of a grid row-letter alphabet.
pub const MAX_ROW_LETTERS: usize = 64;
/// Positions one depopulation mask lists.
pub const MAX_DEPOPULATED: usize = 4096;
/// Most grid positions (rows × columns, depopulated ones included) a form
/// describes: the bound on iterating the matrix.
pub const MAX_GRID_POSITIONS: u64 = 4096;
/// Most rows or columns of one grid.
pub const MAX_GRID_SIDE: u32 = 64;
/// The JEDEC row-letter alphabet: A–Z without I, O, Q, S, X and Z (the
/// JESD95-1 convention, which skips letters that read like digits or each
/// other). Rows beyond Y continue AA, AB, …
pub const JEDEC_ROW_LETTERS: &str = "ABCDEFGHJKLMNPRTUVWY";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GridForm {
    pub contact: GridContact,
    pub rows: u32,
    pub cols: u32,
    /// Pitch along a row and between rows.
    pub pitch: [f64; 2],
    pub body_length: f64,
    pub body_width: f64,
    pub body_height: f64,
    /// Ball or land diameter.
    pub contact_diameter: f64,
    /// Ball height (standoff); 0 for lands.
    pub contact_height: f64,
    /// Empty (row, column) positions, 0-based.
    pub depopulated: Vec<[u32; 2]>,
    /// The row-letter alphabet as data (JEDEC skips I, O, Q, S, X, Z).
    pub row_letters: String,
}

impl GridForm {
    /// Original typical values (a synthetic 4 × 5 ball grid in round
    /// numbers): the family's custom entry in the template gallery.
    pub fn typical() -> Self {
        Self::ball_grid(4, 5, 0.8, [5.0, 4.0, 0.8], [0.4, 0.3])
    }

    /// A fully populated `rows` × `cols` ball grid at `pitch` on a body of
    /// `body` (length, width, height above the balls), with JEDEC row
    /// letters.
    pub fn ball_grid(rows: u32, cols: u32, pitch: f64, body: [f64; 3], ball: [f64; 2]) -> Self {
        Self {
            contact: GridContact::Ball,
            rows,
            cols,
            pitch: [pitch, pitch],
            body_length: body[0],
            body_width: body[1],
            body_height: body[2],
            contact_diameter: ball[0],
            contact_height: ball[1],
            depopulated: Vec::new(),
            row_letters: JEDEC_ROW_LETTERS.to_owned(),
        }
    }

    /// Positions in the matrix, depopulated ones included.
    pub fn positions(&self) -> u64 {
        u64::from(self.rows) * u64::from(self.cols)
    }

    /// Whether the matrix is small enough to iterate.
    fn iterable(&self) -> bool {
        self.rows <= MAX_GRID_SIDE
            && self.cols <= MAX_GRID_SIDE
            && self.positions() <= MAX_GRID_POSITIONS
    }

    /// The depopulated positions inside the matrix, without repeats.
    fn empty_set(&self) -> BTreeSet<[u32; 2]> {
        self.depopulated
            .iter()
            .copied()
            .filter(|[r, c]| *r < self.rows && *c < self.cols)
            .collect()
    }

    /// The row letters as characters, when they are a valid alphabet.
    fn alphabet(&self) -> Option<Vec<char>> {
        let letters: Vec<char> = self.row_letters.chars().collect();
        let valid = !letters.is_empty()
            && self.row_letters.len() <= MAX_ROW_LETTERS
            && letters.iter().all(char::is_ascii_uppercase)
            && letters.windows(2).all(|w| w[0] < w[1]);
        valid.then_some(letters)
    }

    /// The letters A–Z the alphabet leaves out: the footprint numbering's
    /// `skip_letters`, so pads and package terminals name rows alike.
    pub fn skip_letters(&self) -> String {
        ('A'..='Z')
            .filter(|c| !self.row_letters.contains(*c))
            .collect()
    }

    /// The name of row `row` (0-based): its letter, or after the last
    /// letter two letters (AA, AB, …), as the footprint array names rows.
    pub fn row_name(&self, row: u32) -> Option<String> {
        let letters = self.alphabet()?;
        let n = letters.len();
        let mut i = usize::try_from(row).ok()?;
        let mut out = Vec::new();
        loop {
            out.push(*letters.get(i % n)?);
            if i < n {
                break;
            }
            i = i / n - 1;
        }
        Some(out.iter().rev().collect())
    }

    /// The name of position (row, column): row name and 1-based column
    /// ("A1", "AA12").
    pub fn ball_name(&self, row: u32, col: u32) -> Option<String> {
        Some(format!("{}{}", self.row_name(row)?, col.checked_add(1)?))
    }

    /// The (row, column) position a name like "C7" refers to, inside the
    /// matrix; `None` for anything else. Case-insensitive.
    pub fn position_of(&self, name: &str) -> Option<[u32; 2]> {
        let name = name.trim().to_ascii_uppercase();
        let split = name.find(|c: char| c.is_ascii_digit())?;
        let (letters, digits) = name.split_at(split);
        if letters.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let col = digits.parse::<u32>().ok()?.checked_sub(1)?;
        if col >= self.cols {
            return None;
        }
        (0..self.rows.min(MAX_GRID_SIDE))
            .find(|&r| self.row_name(r).as_deref() == Some(letters))
            .map(|r| [r, col])
    }

    /// The populated positions in lead order (row-major from A1). Empty
    /// beyond [`MAX_GRID_POSITIONS`].
    pub fn populated(&self) -> Vec<[u32; 2]> {
        if !self.iterable() {
            return Vec::new();
        }
        let empty = self.empty_set();
        (0..self.rows)
            .flat_map(|r| (0..self.cols).map(move |c| [r, c]))
            .filter(|p| !empty.contains(p))
            .collect()
    }

    /// The ball names in lead order: the designations package terminals
    /// get, matching the footprint's pad numbers. Empty when the alphabet
    /// is invalid or the matrix is beyond its bound.
    pub fn ball_names(&self) -> Vec<String> {
        if self.alphabet().is_none() {
            return Vec::new();
        }
        self.populated()
            .into_iter()
            .filter_map(|[r, c]| self.ball_name(r, c))
            .collect()
    }

    /// The contact centre of position (row, column) in the model frame.
    pub fn centre(&self, row: u32, col: u32) -> [f64; 2] {
        let along = |i: u32, n: u32, pitch: f64| {
            (f64::from(i) - f64::from(n.saturating_sub(1)) / 2.0) * pitch
        };
        [
            along(col, self.cols, self.pitch[0]),
            -along(row, self.rows, self.pitch[1]),
        ]
    }
}

impl FamilyForm for GridForm {
    fn label(&self) -> &'static str {
        "Grid array"
    }
    fn lengths(&self) -> Vec<f64> {
        vec![
            self.pitch[0],
            self.pitch[1],
            self.body_length,
            self.body_width,
            self.body_height,
            self.contact_diameter,
            self.contact_height,
        ]
    }
    /// The body on its balls: length, width, ball height + body height.
    fn envelope(&self) -> Option<[f64; 3]> {
        Some([
            self.body_length,
            self.body_width,
            self.contact_height + self.body_height,
        ])
    }
    /// Populated positions: rows × columns less the depopulated ones
    /// inside the matrix (repeats counted once).
    fn lead_count(&self) -> u64 {
        let empty = u64::try_from(self.empty_set().len()).unwrap_or(u64::MAX);
        self.positions().saturating_sub(empty)
    }
    fn lead_points(&self) -> Vec<Point3> {
        self.populated()
            .into_iter()
            .map(|[r, c]| {
                let [x, y] = self.centre(r, c);
                [x, y, 0.0]
            })
            .collect()
    }
    /// JEDEC outline letters: `e` and `eE` (pitch along a row and between
    /// rows), `MD` and `ME` (columns and rows), `D`, `E` (body), `A`
    /// (seated height), `A1` (ball height), `b` (ball or land diameter).
    fn values(&self) -> Vec<PackageValue> {
        let length = |name: &str, v: f64| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Length(v),
        };
        let count = |name: &str, v: u32| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Count(v),
        };
        vec![
            length("e", self.pitch[0]),
            length("eE", self.pitch[1]),
            count("MD", self.cols),
            count("ME", self.rows),
            length("D", self.body_length),
            length("E", self.body_width),
            length("A", self.contact_height + self.body_height),
            length("A1", self.contact_height),
            length("b", self.contact_diameter),
        ]
    }
    fn range_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut positive = |name: &str, v: f64| {
            if !(v.is_finite() && (FORM_LENGTH_MIN_MM..=FORM_LENGTH_LIMIT_MM).contains(&v)) {
                out.push(format!(
                    "{name} must be {FORM_LENGTH_MIN_MM}–{FORM_LENGTH_LIMIT_MM} mm (is {v})."
                ));
            }
        };
        positive("Pitch along a row", self.pitch[0]);
        positive("Pitch between rows", self.pitch[1]);
        positive("Body length", self.body_length);
        positive("Body height", self.body_height);
        positive("Body width", self.body_width);
        positive("Contact diameter", self.contact_diameter);
        match self.contact {
            GridContact::Ball => positive("Ball height", self.contact_height),
            GridContact::Land => {
                if self.contact_height != 0.0 {
                    out.push(format!(
                        "A land grid has no ball height (0, is {}).",
                        self.contact_height
                    ));
                }
            }
        }
        let mut rule = |ok: bool, message: &str| {
            if !ok {
                out.push(message.to_owned());
            }
        };
        rule(
            (1..=MAX_GRID_SIDE).contains(&self.rows) && (1..=MAX_GRID_SIDE).contains(&self.cols),
            &format!("A grid has 1–{MAX_GRID_SIDE} rows and 1–{MAX_GRID_SIDE} columns."),
        );
        rule(
            self.positions() <= MAX_GRID_POSITIONS,
            &format!("A grid has at most {MAX_GRID_POSITIONS} positions."),
        );
        let d = self.contact_diameter;
        rule(
            (self.cols < 2 || d < self.pitch[0]) && (self.rows < 2 || d < self.pitch[1]),
            "Contacts must not touch (diameter < pitch).",
        );
        let fits = |n: u32, pitch: f64, edge: f64| {
            f64::from(n.saturating_sub(1)) * pitch + d <= edge + 1e-9
        };
        rule(
            fits(self.cols, self.pitch[0], self.body_length),
            "The columns must fit along the body length ((columns − 1) × pitch + diameter ≤ length).",
        );
        rule(
            fits(self.rows, self.pitch[1], self.body_width),
            "The rows must fit across the body width ((rows − 1) × row pitch + diameter ≤ width).",
        );
        if self.contact == GridContact::Ball {
            rule(
                self.contact_height < d,
                "A ball is lower than it is wide (ball height < diameter).",
            );
        }
        rule(
            self.alphabet().is_some(),
            &format!(
                "Row letters are 1–{MAX_ROW_LETTERS} distinct capitals A–Z in alphabetical order (JEDEC: {JEDEC_ROW_LETTERS})."
            ),
        );
        rule(
            self.depopulated.len() <= MAX_DEPOPULATED,
            &format!("A depopulation mask lists at most {MAX_DEPOPULATED} positions."),
        );
        rule(
            self.depopulated
                .iter()
                .all(|[r, c]| *r < self.rows && *c < self.cols),
            "Every depopulated position lies inside the grid.",
        );
        let unique: BTreeSet<_> = self.depopulated.iter().collect();
        rule(
            unique.len() == self.depopulated.len(),
            "A depopulated position is listed once.",
        );
        rule(self.lead_count() >= 1, "A grid keeps at least one contact.");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::PackageForm;

    /// An original synthetic 4 × 5 ball grid (round numbers).
    fn bga() -> GridForm {
        GridForm::ball_grid(4, 5, 0.8, [5.0, 4.0, 0.8], [0.4, 0.3])
    }

    fn names(form: &GridForm) -> Vec<String> {
        form.values().into_iter().map(|v| v.name.0).collect()
    }

    #[test]
    fn a_full_grid_counts_envelopes_and_names_from_a1() {
        let g = bga();
        assert!(g.range_problems().is_empty(), "{:?}", g.range_problems());
        assert_eq!(g.lead_count(), 20);
        assert_eq!(g.envelope(), Some([5.0, 4.0, 1.1]));
        let names = g.ball_names();
        assert_eq!(names.first().map(String::as_str), Some("A1"));
        assert_eq!(names.get(5).map(String::as_str), Some("B1"));
        assert_eq!(names.last().map(String::as_str), Some("D5"));
        // A1 at −x, +y; row A runs to +x; row B lies below it.
        let p = g.lead_points();
        assert!((p[0][0] + 1.6).abs() < 1e-12 && (p[0][1] - 1.2).abs() < 1e-12);
        assert!((p[4][0] - 1.6).abs() < 1e-12 && (p[4][1] - p[0][1]).abs() < 1e-12);
        assert!((p[5][1] - (p[0][1] - 0.8)).abs() < 1e-12);
    }

    #[test]
    fn depopulation_keeps_names_and_skips_jedec_letters() {
        let mut g = GridForm::ball_grid(10, 10, 0.5, [5.5, 5.5, 0.6], [0.3, 0.2]);
        // A centre void of 2 × 2 and a missing corner beside A1.
        g.depopulated = vec![[4, 4], [4, 5], [5, 4], [5, 5], [0, 1]];
        assert!(g.range_problems().is_empty(), "{:?}", g.range_problems());
        assert_eq!(g.lead_count(), 95);
        let names = g.ball_names();
        assert_eq!(names.len(), 95);
        assert_eq!(&names[..2], ["A1", "A3"]);
        // JEDEC skips I: row 8 is J; row 9 is K.
        assert_eq!(g.row_name(7).as_deref(), Some("H"));
        assert_eq!(g.row_name(8).as_deref(), Some("J"));
        assert!(!names.contains(&"E5".to_owned()) && names.contains(&"E4".to_owned()));
        assert_eq!(g.position_of("e6"), Some([4, 5]));
        assert_eq!(g.position_of("I1"), None);
        assert_eq!(g.position_of("A11"), None);
        assert_eq!(g.skip_letters(), "IOQSXZ");
        // Beyond Y the rows run AA, AB, …
        let wide = GridForm::ball_grid(22, 2, 1.0, [3.0, 23.0, 1.0], [0.5, 0.4]);
        assert_eq!(wide.row_name(19).as_deref(), Some("Y"));
        assert_eq!(wide.row_name(20).as_deref(), Some("AA"));
        assert_eq!(wide.position_of("AB2"), Some([21, 1]));
    }

    #[test]
    fn values_follow_jedec_letters() {
        let n = names(&bga());
        assert_eq!(
            n,
            [
                "pkg_e", "pkg_eE", "pkg_MD", "pkg_ME", "pkg_D", "pkg_E", "pkg_A", "pkg_A1", "pkg_b"
            ]
        );
        for name in n {
            assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        }
    }

    #[test]
    fn land_grids_have_no_ball_height() {
        let mut lga = bga();
        lga.contact = GridContact::Land;
        assert!(!lga.range_problems().is_empty());
        lga.contact_height = 0.0;
        assert!(
            lga.range_problems().is_empty(),
            "{:?}",
            lga.range_problems()
        );
        assert_eq!(lga.envelope(), Some([5.0, 4.0, 0.8]));
    }

    #[test]
    fn inconsistent_grids_are_found() {
        let problem = |f: GridForm, needle: &str| {
            let p = f.range_problems();
            assert!(p.iter().any(|m| m.contains(needle)), "{needle}: {p:?}");
        };
        let mut g = bga();
        g.contact_diameter = 0.9;
        problem(g, "touch");
        let mut g = bga();
        g.body_length = 3.0;
        problem(g, "along the body length");
        let mut g = bga();
        g.depopulated = vec![[0, 0], [0, 0]];
        problem(g, "listed once");
        let mut g = bga();
        g.depopulated = vec![[4, 0]];
        problem(g, "inside the grid");
        let mut g = bga();
        g.row_letters = "ABA".into();
        problem(g, "Row letters");
        let mut g = bga();
        g.row_letters = "abc".into();
        problem(g, "Row letters");
        let mut g = bga();
        g.depopulated = (0..4).flat_map(|r| (0..5).map(move |c| [r, c])).collect();
        problem(g, "at least one contact");
        let mut g = bga();
        g.contact_height = 0.5;
        problem(g, "lower than");
        let mut g = bga();
        g.pitch[1] = f64::NAN;
        problem(g, "Pitch between rows");
    }

    #[test]
    fn huge_grids_are_bounded_without_iterating() {
        let mut g = bga();
        g.rows = u32::MAX;
        g.cols = u32::MAX;
        assert!(g.lead_points().is_empty());
        assert!(g.ball_names().is_empty());
        assert!(!g.range_problems().is_empty());
        let form = PackageForm::Grid(Box::new(g));
        assert!(!form.within_bounds());
        // A 22 × 22 grid fits the matrix bound but not the form's lead
        // bound: refused by `within_bounds`, its points withheld.
        let big = PackageForm::Grid(Box::new(GridForm::ball_grid(
            23,
            23,
            1.0,
            [24.0, 24.0, 1.5],
            [0.6, 0.5],
        )));
        assert_eq!(big.lead_count(), 529);
        assert!(!big.within_bounds());
        assert!(big.lead_points().is_empty());
    }

    #[test]
    fn the_package_form_dispatches_to_the_grid() {
        let form = PackageForm::Grid(Box::new(bga()));
        assert_eq!(form.label(), "Grid array");
        assert_eq!(form.terminal_counts(), vec![20]);
        assert_eq!(form.lead_points().len(), 20);
        assert!(form.range_problems().is_empty());
        let json = serde_json::to_string(&form).unwrap_or_default();
        let back: PackageForm = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(back, form);
    }
}
