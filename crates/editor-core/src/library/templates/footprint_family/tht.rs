//! Slice B2d: the through-hole axial, radial, DIP and TO footprint
//! template. The parent dispatches here, so the slice never edits
//! `footprint_family.rs`.
//!
//! Every pad is plated through with a role-owned drill (`drill`, a
//! construction circle bound to the parameter) in a round copper land
//! (`land`). A `Row` is one line of pads along x from pin 1 at −x (axial,
//! radial, TO); a `DualRow` is a DIP: pin 1 bottom left, numbered
//! counter-clockwise, as the package form orders its leads. Pin 1 is
//! marked only when asked (`pin1`: a rounded-square land through an
//! explicit array override and a silkscreen dot), never inferred. Default
//! values are estimated prototype defaults, not copied tables or a
//! compliance claim.
use super::{
    Beyond, Builder, Pin1, axis_line, constrain, created_curves, created_points, curve, dim, e, mm,
    only, param, pin1_dot, place_seed, point, smd,
};
use crate::diagnostics::{Error, Result};
use crate::library::PackageDefinition;
use crate::library::footprint::PadKind;
use crate::library::footprint_source::{
    ArrayId, ArrayLayout, DimKind, Expr, FootprintEdit, GenerationKey, GeomConstraint, HoleRole,
    NumberingScheme, PadOverride, PadShapeExpr, PadTemplate, PadTemplateParams, ParamUnit,
    SketchEdit, SkipRule,
};
use crate::library::package_form::tht::ThtKind;
use crate::library::package_form::{FamilyForm, PackageForm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether the picker offers this template.
pub const AVAILABLE: bool = true;

/// How the pads are laid out.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThtLayout {
    /// One row along x, numbered from −x (axial, radial, TO).
    #[default]
    Row,
    /// Two rows `row_pitch` apart, numbered counter-clockwise from the −x
    /// end of the −y row (DIP).
    DualRow,
}

/// The through-hole axial, radial, DIP and TO land pattern's driving values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThtTemplate {
    /// Lead pitch (`pitch`).
    pub pitch: Expr,
    /// Drill diameter (`drill`).
    pub drill: Expr,
    /// Copper land diameter (`land`).
    pub land: Expr,
    /// Courtyard excess around pads and envelope (`courtyard_excess`).
    pub courtyard_excess: Expr,
    pub layout: ThtLayout,
    /// Pads per row (`per_row`), a count.
    pub per_row: Expr,
    /// Distance between the two rows of a `DualRow` (`row_pitch`).
    pub row_pitch: Expr,
    /// Mark pin 1: a rounded-square land and a silkscreen dot (DIP, TO,
    /// polarised two-lead parts).
    pub pin1: bool,
}

impl ThtTemplate {
    /// The template form prefilled from the package: a through-hole form
    /// gives the layout, pitch (`pkg_e`, `pkg_eA`) and its lead count
    /// (`pkg_n` when the package lists one terminal per lead), and sizes
    /// the drill over the lead's section and the land within the pitch;
    /// otherwise estimated literals for a row of the package's terminals.
    pub fn defaults(package: &PackageDefinition) -> Self {
        let excess = e(super::TEMPLATE_COURTYARD_EXCESS);
        let n = package.terminals.len();
        let PackageForm::Tht(f) = &package.form else {
            return Self {
                pitch: e("2.54"),
                drill: e("0.8"),
                land: e("1.6"),
                courtyard_excess: excess,
                layout: ThtLayout::Row,
                per_row: Expr(n.max(1).to_string()),
                row_pitch: e("7.62"),
                pin1: n > 2,
            };
        };
        let matches = u64::try_from(n).ok() == Some(f.lead_count());
        let (layout, per_row, row_pitch) = match f.kind {
            ThtKind::Axial { .. } | ThtKind::Radial { .. } => (ThtLayout::Row, e("2"), e("2.54")),
            ThtKind::Dip { pins, .. } => (
                ThtLayout::DualRow,
                if matches {
                    e("pkg_n / 2")
                } else {
                    Expr((pins / 2).to_string())
                },
                e("pkg_eA"),
            ),
            ThtKind::To { pins, .. } => (
                ThtLayout::Row,
                if matches {
                    e("pkg_n")
                } else {
                    Expr(pins.to_string())
                },
                e("2.54"),
            ),
        };
        let (drill, land) = f.estimated_drill_and_land();
        let pin1 = f.marks_pin_one();
        Self {
            pitch: e("pkg_e"),
            drill: Expr(format!("{drill}")),
            land: Expr(format!("{land}")),
            courtyard_excess: excess,
            layout,
            per_row,
            row_pitch,
            pin1,
        }
    }

    /// Every driving expression, for the syntax check.
    pub(super) fn exprs(&self) -> Vec<&Expr> {
        vec![
            &self.pitch,
            &self.drill,
            &self.land,
            &self.courtyard_excess,
            &self.per_row,
            &self.row_pitch,
        ]
    }
}

/// A plated seed pad with its role-owned drill, then the row or dual-row
/// array, the pin-1 override and dot when asked.
pub(super) fn build(b: &mut Builder<'_>, t: &ThtTemplate) -> Result<Option<Pin1>> {
    let dual = t.layout == ThtLayout::DualRow;
    let single = super::is_literal(&t.per_row, 1.0);
    b.step(vec![
        param("pitch", t.pitch.clone(), ParamUnit::Mm),
        param("per_row", t.per_row.clone(), ParamUnit::Count),
        param("row_pitch", t.row_pitch.clone(), ParamUnit::Mm),
        param("drill", t.drill.clone(), ParamUnit::Mm),
        param("land", t.land.clone(), ParamUnit::Mm),
        mm("seed_x", "(per_row - 1) / 2 * pitch"),
        mm("seed_y", if dual { "row_pitch / 2" } else { "0" }),
    ])?;
    let guess = [
        if single { 0.0 } else { -2.0 },
        if dual { -2.0 } else { 0.0 },
    ];
    let (seed, anchor) = b.pad(
        PadTemplate::Circle,
        guess,
        PadTemplateParams {
            kind: PadKind::PlatedThrough,
            ..smd("land", "land", None, "1")
        },
    )?;
    let drawn = b.step(vec![FootprintEdit::Sketch(SketchEdit::AddCircle {
        center: guess,
        radius: 0.3,
        construction: true,
    })])?;
    let (Some(centre), Some(drill)) = (
        created_points(&drawn).into_iter().next(),
        created_curves(&drawn).into_iter().next(),
    ) else {
        return Err(Error::Invalid(
            "footprint.template_internal: no drill circle".into(),
        ));
    };
    let mut edits = vec![
        constrain(
            GeomConstraint::Coincident,
            vec![point(&anchor), point(&centre)],
        ),
        dim(DimKind::Diameter, vec![curve(&drill)], "drill"),
    ];
    let mut role = b
        .def
        .source
        .as_deref()
        .and_then(|s| s.pads.iter().find(|r| r.pad == seed))
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("template pad {seed} has no role")))?;
    role.hole = Some(HoleRole {
        curves: vec![drill],
        plated: true,
    });
    edits.push(FootprintEdit::SetPadRole { role });
    edits.extend(place_seed(b, &anchor, guess, [single, !dual])?);
    let (layout, numbering) = if dual {
        let (line, fixed) = axis_line(b, [-0.5, 0.0], [0.5, 0.0])?;
        edits.extend(fixed);
        (
            ArrayLayout::DualRow {
                per_row: e("per_row"),
                pitch: e("pitch"),
                row_span: e("row_pitch"),
                direction: line,
            },
            NumberingScheme::CounterClockwise,
        )
    } else {
        (
            ArrayLayout::Grid {
                rows: e("1"),
                cols: e("per_row"),
                pitch_x: e("pitch"),
                pitch_y: e("pitch"),
                stagger: false,
            },
            NumberingScheme::Sequential,
        )
    };
    edits.push(FootprintEdit::AddArray {
        seed,
        layout,
        numbering,
        skip: SkipRule::default(),
        first_number: String::new(),
    });
    let step = b.step(edits)?;
    if !t.pin1 {
        return Ok(None);
    }
    let array: ArrayId = only(&step.arrays, "array")?;
    b.step(vec![FootprintEdit::SetOverride {
        key: GenerationKey::Array { array, i: 0, j: 0 },
        value: PadOverride {
            shape: Some(PadShapeExpr::RoundRect {
                width: e("land"),
                height: e("land"),
                radius_ratio: e("0.25"),
            }),
            ..PadOverride::default()
        },
    }])?;
    let beyond = if dual { Beyond::Below } else { Beyond::Left };
    pin1_dot(b, seed, &anchor, guess, beyond, "land / 2").map(Some)
}
