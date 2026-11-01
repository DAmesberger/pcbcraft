//! Slice B2c: the grid array (BGA, LGA, WLCSP) footprint template. The
//! parent dispatches here, so the slice never edits `footprint_family.rs`.
//!
//! One round seed land at A1 (top left, −x +y) and a `Grid` array of
//! `rows` × `cols` at `pitch` × `row_pitch`, numbered by row letter and
//! column (`GridAlpha`, the alphabet the package form names its rows
//! with) and leaving out the depopulated positions as explicit skips, so
//! a missing ball keeps its gap and every other pad keeps its name. Lands
//! are solder-mask defined (SMD: the mask opening `mask_expansion` inside
//! the copper) or not (NSMD: the opening `mask_expansion` outside it).
//! Default values are estimated prototype defaults, not copied tables or
//! a compliance claim; a row can carry the manufacturer's recommended
//! lands instead.
use super::{Beyond, Builder, Pin1, e, param, pin1_dot, place_seed, smd};
use crate::diagnostics::{Error, Result};
use crate::library::PackageDefinition;
use crate::library::footprint_source::{
    ArrayLayout, Expr, FootprintEdit, MaskRuleExpr, NumberingScheme, PadTemplate, ParamUnit,
    SkipRule,
};
use crate::library::package_form::PackageForm;
use crate::library::package_form::grid::{GridContact, MAX_DEPOPULATED, NSMD_LAND_SHARE};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether the picker offers this template.
pub const AVAILABLE: bool = true;

/// The skipped letters of the JEDEC row alphabet (JESD95-1 concept).
pub const JEDEC_SKIP_LETTERS: &str = "IOQSXZ";

/// Mask clearance (NSMD) or overlap (SMD) per side, mm (estimated).
const MASK_MM: &str = "0.05";

/// How the solder mask meets each land.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GridLand {
    /// Non-solder-mask defined: the copper sets the land; the mask opening
    /// is `mask_expansion` larger all round.
    #[default]
    Nsmd,
    /// Solder-mask defined: the mask opening, `mask_expansion` inside the
    /// copper all round, sets the land.
    Smd,
}

/// The grid array (BGA, LGA, WLCSP) land pattern's driving values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GridTemplate {
    /// Pitch along a row, x (`pitch`).
    pub pitch: Expr,
    /// Pitch between rows, y (`row_pitch`).
    pub row_pitch: Expr,
    /// Rows (`rows`) and columns (`cols`), counts.
    pub rows: Expr,
    pub cols: Expr,
    /// Copper land diameter (`land`).
    pub land: Expr,
    pub land_kind: GridLand,
    /// Solder-mask clearance (NSMD) or overlap (SMD) per side
    /// (`mask_expansion`, at least 0).
    pub mask_expansion: Expr,
    /// Empty (row, column) positions, 0-based; A1 (0, 0) seeds the array
    /// and cannot be empty.
    pub depopulated: Vec<[u16; 2]>,
    /// Letters A–Z the row names skip.
    pub skip_letters: String,
    /// Courtyard excess around pads and envelope (`courtyard_excess`).
    pub courtyard_excess: Expr,
}

impl GridTemplate {
    /// The template form prefilled from the package: a grid form's pitch,
    /// matrix, depopulation and alphabet (sizes as `pkg_*` expressions),
    /// else literals for a square grid of the package's terminals.
    pub fn defaults(package: &PackageDefinition) -> Self {
        if let PackageForm::Grid(g) = &package.form {
            let land = match g.contact {
                GridContact::Ball => e(&format!("{NSMD_LAND_SHARE} * pkg_b")),
                GridContact::Land => e("pkg_b"),
            };
            return Self {
                pitch: e("pkg_e"),
                row_pitch: e("pkg_eE"),
                rows: e("pkg_ME"),
                cols: e("pkg_MD"),
                land,
                land_kind: GridLand::Nsmd,
                mask_expansion: e(MASK_MM),
                depopulated: g
                    .depopulated
                    .iter()
                    .filter_map(|[r, c]| Some([u16::try_from(*r).ok()?, u16::try_from(*c).ok()?]))
                    .collect(),
                skip_letters: g.skip_letters(),
                courtyard_excess: e(super::TEMPLATE_COURTYARD_EXCESS),
            };
        }
        let n = package.terminals.len();
        let side = (1..=64usize).find(|s| s * s == n);
        let (rows, cols) = match side {
            Some(s) => (s, s),
            None => (1, n.max(1)),
        };
        Self {
            pitch: e("0.8"),
            row_pitch: e("0.8"),
            rows: Expr(rows.to_string()),
            cols: Expr(cols.to_string()),
            land: e("0.4"),
            land_kind: GridLand::Nsmd,
            mask_expansion: e(MASK_MM),
            depopulated: Vec::new(),
            skip_letters: JEDEC_SKIP_LETTERS.to_owned(),
            courtyard_excess: e(super::TEMPLATE_COURTYARD_EXCESS),
        }
    }

    /// Every driving expression, for the syntax check.
    pub(super) fn exprs(&self) -> Vec<&Expr> {
        vec![
            &self.pitch,
            &self.row_pitch,
            &self.rows,
            &self.cols,
            &self.land,
            &self.mask_expansion,
            &self.courtyard_excess,
        ]
    }

    /// The data checks the expressions cannot express.
    fn check(&self) -> Result<()> {
        let invalid = |m: &str| Err(Error::Invalid(format!("footprint.template_invalid: {m}")));
        if self.depopulated.len() > MAX_DEPOPULATED {
            return Err(Error::Limit(format!(
                "footprint.template_invalid: a grid lists at most {MAX_DEPOPULATED} depopulated positions"
            )));
        }
        if self.depopulated.contains(&[0, 0]) {
            return invalid(
                "the grid's array is seeded at A1 (row 0, column 0), which cannot be depopulated",
            );
        }
        let letters: Vec<char> = self.skip_letters.chars().collect();
        if letters.len() > 25
            || !letters.iter().all(char::is_ascii_uppercase)
            || letters.windows(2).any(|w| w[0] >= w[1])
        {
            return invalid(
                "skip letters are distinct capitals A–Z in alphabetical order, leaving at least one letter",
            );
        }
        Ok(())
    }
}

/// The seed land at A1, its mask rule and the lettered grid array.
pub(super) fn build(b: &mut Builder<'_>, t: &GridTemplate) -> Result<Option<Pin1>> {
    t.check()?;
    b.step(vec![
        param("pitch", t.pitch.clone(), ParamUnit::Mm),
        param("row_pitch", t.row_pitch.clone(), ParamUnit::Mm),
        param("rows", t.rows.clone(), ParamUnit::Count),
        param("cols", t.cols.clone(), ParamUnit::Count),
        param("land", t.land.clone(), ParamUnit::Mm),
        param("mask_expansion", t.mask_expansion.clone(), ParamUnit::Mm),
        param("seed_x", e("(cols - 1) / 2 * pitch"), ParamUnit::Mm),
        param("seed_y", e("(rows - 1) / 2 * row_pitch"), ParamUnit::Mm),
    ])?;
    let guess = [-2.0, 2.0];
    let (seed, anchor) = b.pad(PadTemplate::Circle, guess, smd("land", "land", None, "A1"))?;
    let mut role = b
        .def
        .source
        .as_deref()
        .and_then(|s| s.pads.iter().find(|r| r.pad == seed))
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("template pad {seed} has no role")))?;
    role.mask = MaskRuleExpr::Expansion(match t.land_kind {
        GridLand::Nsmd => e("mask_expansion"),
        GridLand::Smd => e("0 - mask_expansion"),
    });
    let one = |x: &Expr| super::is_literal(x, 1.0);
    let mut edits = vec![FootprintEdit::SetPadRole { role }];
    edits.extend(place_seed(b, &anchor, guess, [one(&t.cols), one(&t.rows)])?);
    edits.push(FootprintEdit::AddArray {
        seed,
        layout: ArrayLayout::Grid {
            rows: e("rows"),
            cols: e("cols"),
            pitch_x: e("pitch"),
            pitch_y: e("row_pitch"),
            stagger: false,
        },
        numbering: NumberingScheme::GridAlpha {
            skip_letters: t.skip_letters.clone(),
        },
        skip: SkipRule {
            explicit: t.depopulated.clone(),
            ..SkipRule::default()
        },
        first_number: String::new(),
    });
    b.step(edits)?;
    pin1_dot(b, seed, &anchor, guess, Beyond::Left, "land / 2").map(Some)
}
