//! Typed physical package parameters: the only source of a package's
//! `pkg_*` values and of generated 3D models. Millimetres; the seating plane
//! is z = 0 (the top of the land copper), x along the package length.
//!
//! M1-M3 contract (`docs/m13-contract.md`): the families and their fields
//! are fixed; range rules and the full `pkg_*` list belong to the M1 core
//! task and may grow, field names may not change without a contract update.
use super::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Largest lead or pin count of one typed form.
pub const MAX_FORM_LEADS: u32 = 512;

/// A package's physical form. `Generic` has only the authored envelope.
/// Family parameters are boxed so definitions stay small in commands.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "family", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackageForm {
    #[default]
    Generic,
    /// Two-terminal rectangular chip (resistor, capacitor, inductor).
    Chip(Box<ChipForm>),
    /// Gull-wing leads on two (SOIC, SOP, SSOP) or four (QFP) sides.
    GullWing(Box<GullWingForm>),
    /// No-lead with side terminals on two (DFN, SON) or four (QFN) sides,
    /// optionally with an exposed pad.
    NoLead(Box<NoLeadForm>),
    /// Straight through-hole pin header.
    Header(Box<HeaderForm>),
}

/// Which chip construction the body is: sets default materials.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChipKind {
    /// Thick-film resistor: alumina substrate, black overcoat on top.
    Resistor,
    /// Multilayer ceramic capacitor.
    Capacitor,
    /// Ferrite chip inductor or bead.
    Inductor,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChipForm {
    pub kind: ChipKind,
    /// Body length (terminal to terminal), width and height.
    pub length: f64,
    pub width: f64,
    pub height: f64,
    /// Length of each wrapped end termination along the body.
    pub termination: f64,
}

/// Lead rows: two sides (along ±y edges, leads pointing ±y) or four.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeadSides {
    Two,
    Four,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GullWingForm {
    pub sides: LeadSides,
    /// Leads on each long side (the ±y edges) and, for `Four`, on each
    /// short side (the ±x edges; 0 for `Two`).
    pub leads_per_side: [u32; 2],
    pub pitch: f64,
    /// Moulded body length (x), width (y) and height above its underside.
    pub body_length: f64,
    pub body_width: f64,
    pub body_height: f64,
    /// Gap between the seating plane and the body underside.
    pub standoff: f64,
    /// Tip-to-tip lead span across y and (for `Four`) across x.
    pub span: [f64; 2],
    pub lead_width: f64,
    pub lead_thickness: f64,
    /// Flat foot length on the seating plane.
    pub foot_length: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NoLeadForm {
    pub sides: LeadSides,
    /// Terminals on each long side (±y edges) and short side (±x; 0 for `Two`).
    pub terminals_per_side: [u32; 2],
    pub pitch: f64,
    pub body_length: f64,
    pub body_width: f64,
    pub body_height: f64,
    /// Terminal length inward from the body edge, and width along it.
    pub terminal_length: f64,
    pub terminal_width: f64,
    /// Exposed-pad length and width, centred, if any.
    pub exposed_pad: Option<[f64; 2]>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HeaderForm {
    pub rows: u32,
    pub pins_per_row: u32,
    /// Pin pitch along a row and between rows.
    pub pitch: f64,
    pub row_pitch: f64,
    /// Plastic body height above the seating plane.
    pub body_height: f64,
    /// Square pin side.
    pub pin_side: f64,
    /// Pin length above the body top and below the seating plane.
    pub pin_above: f64,
    pub pin_below: f64,
}

impl PackageForm {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Generic => "Generic",
            Self::Chip(_) => "Chip",
            Self::GullWing(_) => "Gull-wing",
            Self::NoLead(_) => "No-lead",
            Self::Header(_) => "Pin header",
        }
    }

    /// Every length the form stores, for finiteness checks.
    pub fn lengths(&self) -> Vec<f64> {
        match self {
            Self::Generic => Vec::new(),
            Self::Chip(c) => vec![c.length, c.width, c.height, c.termination],
            Self::GullWing(g) => vec![
                g.pitch,
                g.body_length,
                g.body_width,
                g.body_height,
                g.standoff,
                g.span[0],
                g.span[1],
                g.lead_width,
                g.lead_thickness,
                g.foot_length,
            ],
            Self::NoLead(n) => {
                let mut v = vec![
                    n.pitch,
                    n.body_length,
                    n.body_width,
                    n.body_height,
                    n.terminal_length,
                    n.terminal_width,
                ];
                v.extend(n.exposed_pad.iter().flatten());
                v
            }
            Self::Header(h) => vec![
                h.pitch,
                h.row_pitch,
                h.body_height,
                h.pin_side,
                h.pin_above,
                h.pin_below,
            ],
        }
    }

    /// The typed form's envelope (length, width, height above the seating
    /// plane); `None` for `Generic`, whose envelope is authored. Contract
    /// body: the M1 core task owns the exact derivation.
    pub fn envelope(&self) -> Option<[f64; 3]> {
        match self {
            Self::Generic => None,
            Self::Chip(c) => Some([c.length, c.width, c.height]),
            Self::GullWing(g) => Some([
                if g.sides == LeadSides::Four {
                    g.span[1]
                } else {
                    g.body_length
                },
                g.span[0],
                g.standoff + g.body_height,
            ]),
            Self::NoLead(n) => Some([n.body_length, n.body_width, n.body_height]),
            Self::Header(h) => Some([
                h.pitch * f64::from(h.pins_per_row),
                h.row_pitch * f64::from(h.rows),
                h.body_height + h.pin_above,
            ]),
        }
    }

    /// The form's own `pkg_*` values (beyond the envelope and count that
    /// `footprint_source::package_values` always offers). Contract body:
    /// the pitch only; the M1 core task completes the list.
    pub fn values(&self) -> Vec<PackageValue> {
        let pitch = match self {
            Self::Generic | Self::Chip(_) => None,
            Self::GullWing(g) => Some(g.pitch),
            Self::NoLead(n) => Some(n.pitch),
            Self::Header(h) => Some(h.pitch),
        };
        pitch
            .into_iter()
            .map(|e| PackageValue {
                name: ParamName(format!("{PACKAGE_PREFIX}e")),
                value: ParamValue::Length(e),
            })
            .collect()
    }
}
