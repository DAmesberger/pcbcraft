//! Parametric footprint family templates (slice F5): pure functions that
//! turn a typed template into ordinary [`FootprintEdit`]s, applied through
//! `library.edit_footprint` exactly like user edits, so the result is an
//! editable sketch with named parameters. Templates add the courtyard and
//! assembly outline as explicit `Derived` graphic roles; nothing is ever
//! inserted automatically elsewhere. Default values are estimated
//! prototype defaults, not copied tables or a compliance claim.
//!
//! Contract (docs/f45-contract.md): the types are fixed; [`edits`] is
//! filled in by the templates task and refuses until then.
use crate::diagnostics::{Error, Result};
use crate::library::PackageDefinition;
use crate::library::footprint_source::{Expr, FootprintEdit, NumberingScheme};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The courtyard excess a template's `Derived` courtyard role uses unless
/// the author changes its parameter, in mm (estimated default).
pub const TEMPLATE_COURTYARD_EXCESS: &str = "0.25";

/// Which family a template builds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemplateKind {
    /// Two-terminal chip, MELF or moulded body.
    Chip,
    /// SOIC/SOP/SSOP/TSSOP gull-wing: two facing rows.
    GullWing,
    /// QFN/DFN: four (or two) sides plus an exposed pad.
    Qfn,
    /// Through-hole pin header or DIP.
    PinHeader,
}

impl TemplateKind {
    pub const ALL: [Self; 4] = [Self::Chip, Self::GullWing, Self::Qfn, Self::PinHeader];

    /// The picker label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Chip => "2-terminal chip",
            Self::GullWing => "SOIC / SOP",
            Self::Qfn => "QFN / DFN",
            Self::PinHeader => "Pin header",
        }
    }
}

/// A 2-terminal chip land pattern. Every size is an expression over the
/// template's own parameters and the package's `pkg_*` values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChipTemplate {
    /// Land length along the body axis (`pad_l`).
    pub pad_l: Expr,
    /// Land width across the body axis (`pad_w`).
    pub pad_w: Expr,
    /// Gap between the two lands (`gap`).
    pub gap: Expr,
    /// Rounded-rectangle corner ratio, 0–0.5 (`rr`).
    pub rr: Expr,
    /// Courtyard excess around pads and envelope (`courtyard_excess`).
    pub courtyard_excess: Expr,
    /// Add a pin-1 polarity marker (polarised chips: diodes, tantalum).
    pub polarity: bool,
}

/// Gull-wing dual-row (SOIC-like): a seed pad plus a `DualRow` array with
/// counter-clockwise numbering and a pin-1 marker.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GullWingTemplate {
    /// Pad pitch (`pitch`, default `pkg_e` once packages carry it).
    pub pitch: Expr,
    /// Pads per row (`per_row`).
    pub per_row: Expr,
    /// Centre-to-centre distance between the rows (`row_span`).
    pub row_span: Expr,
    pub pad_l: Expr,
    pub pad_w: Expr,
    pub courtyard_excess: Expr,
}

/// QFN/DFN: a `Quad` array placed per side (so `nx ≠ ny` works; `ny = 0`
/// is a DFN), an exposed pad whose terminal is chosen explicitly, paste
/// windows and optional thermal vias.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QfnTemplate {
    pub pitch: Expr,
    /// Pads on each horizontal side.
    pub nx: Expr,
    /// Pads on each vertical side.
    pub ny: Expr,
    /// Pad-centre span between the left and right rows.
    pub span_x: Expr,
    /// Pad-centre span between the bottom and top rows.
    pub span_y: Expr,
    pub pad_l: Expr,
    pub pad_w: Expr,
    /// Exposed pad size; `None` leaves it out.
    pub exposed: Option<ExposedPad>,
    pub courtyard_excess: Expr,
}

/// The exposed (thermal) pad of a QFN template.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExposedPad {
    pub width: Expr,
    pub height: Expr,
    /// Paste window grid `[rows, cols]`; the windows cover `paste_cov` of
    /// the pad area.
    pub paste_grid: [u16; 2],
    /// Paste coverage, 0–1 (`paste_cov`).
    pub paste_cov: Expr,
    /// Thermal via grid; `None` adds none.
    pub vias: Option<ExposedVias>,
}

/// Pad-owned thermal vias on the exposed pad.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExposedVias {
    pub rows: Expr,
    pub cols: Expr,
    pub pitch: Expr,
    pub drill: Expr,
}

/// Through-hole header or DIP: a grid or dual row of plated pads; pin 1
/// differs through an explicit array override, never by inference.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HeaderTemplate {
    pub pitch: Expr,
    pub rows: Expr,
    pub per_row: Expr,
    /// Distance between rows.
    pub row_pitch: Expr,
    pub drill: Expr,
    /// Copper land diameter.
    pub land: Expr,
    pub numbering: NumberingScheme,
    /// Pin 1 as a rounded rectangle (through a `SetOverride`).
    pub square_pin1: bool,
}

/// One family template with its driving values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FootprintTemplate {
    Chip(ChipTemplate),
    GullWing(GullWingTemplate),
    Qfn(QfnTemplate),
    PinHeader(HeaderTemplate),
}

fn e(text: &str) -> Expr {
    Expr(text.into())
}

impl FootprintTemplate {
    pub fn kind(&self) -> TemplateKind {
        match self {
            Self::Chip(_) => TemplateKind::Chip,
            Self::GullWing(_) => TemplateKind::GullWing,
            Self::Qfn(_) => TemplateKind::Qfn,
            Self::PinHeader(_) => TemplateKind::PinHeader,
        }
    }

    /// The template form prefilled for `kind` (estimated defaults; the
    /// templates task derives them from the package's `pkg_*` values).
    pub fn defaults(kind: TemplateKind, _package: &PackageDefinition) -> Self {
        let excess = e(TEMPLATE_COURTYARD_EXCESS);
        match kind {
            TemplateKind::Chip => Self::Chip(ChipTemplate {
                pad_l: e("0.6"),
                pad_w: e("pkg_W"),
                gap: e("pkg_L - 2 * 0.3"),
                rr: e("0.25"),
                courtyard_excess: excess,
                polarity: false,
            }),
            TemplateKind::GullWing => Self::GullWing(GullWingTemplate {
                pitch: e("1.27"),
                per_row: e("pkg_n / 2"),
                row_span: e("5.4"),
                pad_l: e("1.55"),
                pad_w: e("0.6"),
                courtyard_excess: excess,
            }),
            TemplateKind::Qfn => Self::Qfn(QfnTemplate {
                pitch: e("0.5"),
                nx: e("pkg_n / 4"),
                ny: e("pkg_n / 4"),
                span_x: e("pkg_L"),
                span_y: e("pkg_W"),
                pad_l: e("0.8"),
                pad_w: e("0.25"),
                exposed: None,
                courtyard_excess: excess,
            }),
            TemplateKind::PinHeader => Self::PinHeader(HeaderTemplate {
                pitch: e("2.54"),
                rows: e("1"),
                per_row: e("pkg_n"),
                row_pitch: e("2.54"),
                drill: e("1.0"),
                land: e("1.7"),
                numbering: NumberingScheme::Sequential,
                square_pin1: true,
            }),
        }
    }
}

/// The edits that build `template` into an empty footprint draft:
/// `SetParameter`s for its named values, template pads and arrays, and
/// explicit `Derived` courtyard and assembly roles. Pure; the caller
/// applies them through `library.edit_footprint` (or
/// `library.footprint_from_template`) as one undo step.
pub fn edits(
    template: &FootprintTemplate,
    _package: &PackageDefinition,
) -> Result<Vec<FootprintEdit>> {
    Err(Error::Unsupported(format!(
        "footprint.template_unsupported: the {} template is not available yet",
        template.kind().label()
    )))
}
