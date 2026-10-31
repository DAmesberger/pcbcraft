//! Typed physical package parameters: the only source of a package's
//! `pkg_*` values and of generated 3D models. Millimetres; the seating plane
//! is z = 0 (the top of the land copper), x along the package length.
//!
//! Lead order (the order of [`PackageForm::lead_points`], matched to the
//! package's terminals by position): counter-clockwise seen from above,
//! starting at the −x end of the −y row. Two-sided forms run along −y
//! (left to right), then +y (right to left); four-sided forms continue
//! −y, +x (upward), +y, −x (downward). A no-lead exposed pad, when it is a
//! terminal, comes last. A chip's terminal 1 is at −x; a header's pins run
//! column by column from −x, row 0 at −y.
use super::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use crate::geometry::Point3;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Largest lead or pin count of one typed form.
pub const MAX_FORM_LEADS: u32 = 512;

/// Largest length a typed form stores, in mm.
pub const FORM_LENGTH_LIMIT_MM: f64 = 1000.0;

/// Smallest positive length a typed form stores, in mm (the envelope's
/// smallest size).
pub const FORM_LENGTH_MIN_MM: f64 = 0.01;

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

impl ChipForm {
    /// A chip of `size` (length, width, height) with the common
    /// termination band: a fifth of the length, at most 0.5 mm (EIA
    /// chip-resistor bands, 0.3–0.5 mm from 0603 to 1206).
    pub fn new(kind: ChipKind, size: [f64; 3]) -> Self {
        Self {
            kind,
            length: size[0],
            width: size[1],
            height: size[2],
            termination: (0.2 * size[0]).min(0.5),
        }
    }
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
    /// Plastic body height above the seating plane. The body is one pitch
    /// per pin long and one row pitch per row wide.
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
    /// plane), centred on the model origin from z = 0 up; `None` for
    /// `Generic`, whose envelope is authored. Leads below the seating plane
    /// (header pins) are not part of the envelope.
    pub fn envelope(&self) -> Option<[f64; 3]> {
        match self {
            Self::Generic => None,
            Self::Chip(c) => Some([c.length, c.width, c.height]),
            Self::GullWing(g) => Some([
                if g.sides == LeadSides::Four {
                    g.span[1].max(g.body_length)
                } else {
                    g.body_length
                },
                g.span[0].max(g.body_width),
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

    /// Leads (or terminations, terminals, pins) the form has, not counting
    /// an exposed pad; 0 for `Generic`.
    pub fn lead_count(&self) -> u64 {
        let rows = |sides: LeadSides, per: [u32; 2]| match sides {
            LeadSides::Two => 2 * u64::from(per[0]),
            LeadSides::Four => 2 * (u64::from(per[0]) + u64::from(per[1])),
        };
        match self {
            Self::Generic => 0,
            Self::Chip(_) => 2,
            Self::GullWing(g) => rows(g.sides, g.leads_per_side),
            Self::NoLead(n) => rows(n.sides, n.terminals_per_side),
            Self::Header(h) => u64::from(h.rows) * u64::from(h.pins_per_row),
        }
    }

    /// Package terminal counts the form accepts: its leads, and for a
    /// no-lead form with an exposed pad also one more (the pad as a
    /// terminal). Empty for `Generic` (any count).
    pub fn terminal_counts(&self) -> Vec<u64> {
        let leads = self.lead_count();
        match self {
            Self::Generic => Vec::new(),
            Self::NoLead(n) if n.exposed_pad.is_some() => vec![leads, leads + 1],
            _ => vec![leads],
        }
    }

    /// Where each lead touches the seating plane (z = 0) in the model frame,
    /// in lead order (module docs), then a terminal exposed pad's centre.
    /// Empty for `Generic` and for counts beyond [`MAX_FORM_LEADS`].
    pub fn lead_points(&self) -> Vec<Point3> {
        if self.lead_count() > u64::from(MAX_FORM_LEADS) {
            return Vec::new();
        }
        match self {
            Self::Generic => Vec::new(),
            Self::Chip(c) => {
                let x = c.length / 2.0 - c.termination / 2.0;
                vec![[-x, 0.0, 0.0], [x, 0.0, 0.0]]
            }
            Self::GullWing(g) => {
                let reach = [
                    g.span[0] / 2.0 - g.foot_length / 2.0,
                    g.span[1] / 2.0 - g.foot_length / 2.0,
                ];
                ring(g.sides, g.leads_per_side, g.pitch, reach)
            }
            Self::NoLead(n) => {
                let reach = [
                    n.body_width / 2.0 - n.terminal_length / 2.0,
                    n.body_length / 2.0 - n.terminal_length / 2.0,
                ];
                let mut points = ring(n.sides, n.terminals_per_side, n.pitch, reach);
                if n.exposed_pad.is_some() {
                    points.push([0.0; 3]);
                }
                points
            }
            Self::Header(h) => (0..h.pins_per_row)
                .flat_map(|c| (0..h.rows).map(move |r| (c, r)))
                .map(|(c, r)| {
                    [
                        centred(c, h.pins_per_row, h.pitch),
                        centred(r, h.rows, h.row_pitch),
                        0.0,
                    ]
                })
                .collect(),
        }
    }

    /// The form's own `pkg_*` values, beyond the envelope (`pkg_L`, `pkg_W`,
    /// `pkg_H`) and terminal count (`pkg_n`) that
    /// `footprint_source::package_values` always offers. Names follow JEDEC
    /// outline letters where one exists.
    pub fn values(&self) -> Vec<PackageValue> {
        let length = |name: &str, v: f64| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Length(v),
        };
        let count = |name: &str, v: u32| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Count(v),
        };
        match self {
            Self::Generic => Vec::new(),
            Self::Chip(c) => vec![length("T", c.height), length("a", c.termination)],
            Self::GullWing(g) => {
                let mut v = vec![
                    length("e", g.pitch),
                    count("nx", g.leads_per_side[0]),
                    count("ny", g.leads_per_side[1]),
                    length("E1", g.body_width),
                    length("E", g.span[0]),
                    length("A1", g.standoff),
                    length("A2", g.body_height),
                    length("b", g.lead_width),
                    length("c", g.lead_thickness),
                    length("Lf", g.foot_length),
                ];
                match g.sides {
                    LeadSides::Two => v.push(length("D", g.body_length)),
                    LeadSides::Four => {
                        v.push(length("D1", g.body_length));
                        v.push(length("D", g.span[1]));
                    }
                }
                v
            }
            Self::NoLead(n) => {
                let mut v = vec![
                    length("e", n.pitch),
                    count("nx", n.terminals_per_side[0]),
                    count("ny", n.terminals_per_side[1]),
                    length("D", n.body_length),
                    length("E", n.body_width),
                    length("A", n.body_height),
                    length("b", n.terminal_width),
                    length("Lt", n.terminal_length),
                ];
                if let Some([d2, e2]) = n.exposed_pad {
                    v.push(length("D2", d2));
                    v.push(length("E2", e2));
                }
                v
            }
            Self::Header(h) => vec![
                length("e", h.pitch),
                count("rows", h.rows),
                count("per_row", h.pins_per_row),
                length("row_pitch", h.row_pitch),
                length("body_h", h.body_height),
                length("pin", h.pin_side),
                length("pin_above", h.pin_above),
                length("pin_below", h.pin_below),
            ],
        }
    }

    /// Lead and terminal counts within [`MAX_FORM_LEADS`]: the persistence
    /// bound (a refusal, unlike the range findings).
    pub fn within_bounds(&self) -> bool {
        self.lead_count() <= u64::from(MAX_FORM_LEADS)
    }

    /// Range rules of the typed parameters, as finding messages (each one
    /// `package.form_range`). Empty when the form is physically consistent.
    pub fn range_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut positive = |name: &str, v: f64| {
            if !(v.is_finite() && (FORM_LENGTH_MIN_MM..=FORM_LENGTH_LIMIT_MM).contains(&v)) {
                out.push(format!(
                    "{name} must be {FORM_LENGTH_MIN_MM}–{FORM_LENGTH_LIMIT_MM} mm (is {v})."
                ));
            }
        };
        match self {
            Self::Generic => {}
            Self::Chip(c) => {
                positive("Chip length", c.length);
                positive("Chip width", c.width);
                positive("Chip height", c.height);
                positive("Termination length", c.termination);
            }
            Self::GullWing(g) => {
                positive("Pitch", g.pitch);
                positive("Body length", g.body_length);
                positive("Body width", g.body_width);
                positive("Body height", g.body_height);
                positive("Lead span", g.span[0]);
                if g.sides == LeadSides::Four {
                    positive("Lead span across x", g.span[1]);
                }
                positive("Lead width", g.lead_width);
                positive("Lead thickness", g.lead_thickness);
                positive("Foot length", g.foot_length);
            }
            Self::NoLead(n) => {
                positive("Pitch", n.pitch);
                positive("Body length", n.body_length);
                positive("Body width", n.body_width);
                positive("Body height", n.body_height);
                positive("Terminal length", n.terminal_length);
                positive("Terminal width", n.terminal_width);
                if let Some([l, w]) = n.exposed_pad {
                    positive("Exposed-pad length", l);
                    positive("Exposed-pad width", w);
                }
            }
            Self::Header(h) => {
                positive("Pitch", h.pitch);
                positive("Row pitch", h.row_pitch);
                positive("Body height", h.body_height);
                positive("Pin side", h.pin_side);
                positive("Pin length below", h.pin_below);
            }
        }
        out.extend(self.consistency_problems());
        out
    }

    /// Rules between parameters: leads fit their body edge, do not touch,
    /// reach past the body, and so on.
    fn consistency_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut rule = |ok: bool, message: &str| {
            if !ok {
                out.push(message.to_owned());
            }
        };
        // A row of `n` features `width` wide at `pitch` must fit in `edge`.
        let fits = |n: u32, pitch: f64, width: f64, edge: f64| {
            f64::from(n.saturating_sub(1)) * pitch + width <= edge + 1e-9
        };
        match self {
            Self::Generic => {}
            Self::Chip(c) => rule(
                2.0 * c.termination < c.length,
                "The two terminations must leave body between them (2 × termination < length).",
            ),
            Self::GullWing(g) => {
                let [n, m] = g.leads_per_side;
                rule(n >= 1, "Each long side needs at least one lead.");
                rule(
                    g.pitch > g.lead_width,
                    "Leads must not touch (pitch > lead width).",
                );
                rule(
                    fits(n, g.pitch, g.lead_width, g.body_length),
                    "The leads of a long side must fit along the body length.",
                );
                rule(
                    g.span[0] > g.body_width,
                    "The lead span must reach past the body width.",
                );
                rule(
                    g.foot_length <= (g.span[0] - g.body_width) / 2.0,
                    "The foot must lie outside the body (foot length ≤ (span − body width) / 2).",
                );
                rule(g.standoff >= 0.0, "The standoff must not be negative.");
                rule(
                    g.lead_thickness <= g.standoff + g.body_height,
                    "Leads must not be thicker than the package is tall.",
                );
                match g.sides {
                    LeadSides::Two => rule(
                        m == 0 && g.span[1] == 0.0,
                        "A two-sided form has no short-side leads or span (0).",
                    ),
                    LeadSides::Four => {
                        rule(m >= 1, "Each short side of a four-sided form needs a lead.");
                        rule(
                            fits(m, g.pitch, g.lead_width, g.body_width),
                            "The leads of a short side must fit along the body width.",
                        );
                        rule(
                            g.span[1] > g.body_length,
                            "The lead span across x must reach past the body length.",
                        );
                        rule(
                            g.foot_length <= (g.span[1] - g.body_length) / 2.0,
                            "The foot must lie outside the body across x.",
                        );
                    }
                }
            }
            Self::NoLead(n) => {
                let [nx, ny] = n.terminals_per_side;
                rule(nx >= 1, "Each long side needs at least one terminal.");
                rule(
                    n.pitch > n.terminal_width,
                    "Terminals must not touch (pitch > terminal width).",
                );
                rule(
                    fits(nx, n.pitch, n.terminal_width, n.body_length),
                    "The terminals of a long side must fit along the body length.",
                );
                rule(
                    2.0 * n.terminal_length < n.body_width,
                    "Opposite terminals must not meet (2 × terminal length < body width).",
                );
                let (inner_l, inner_w) = match n.sides {
                    LeadSides::Two => {
                        rule(ny == 0, "A two-sided form has no short-side terminals (0).");
                        (n.body_length, n.body_width - 2.0 * n.terminal_length)
                    }
                    LeadSides::Four => {
                        rule(
                            ny >= 1,
                            "Each short side of a four-sided form needs a terminal.",
                        );
                        rule(
                            fits(ny, n.pitch, n.terminal_width, n.body_width),
                            "The terminals of a short side must fit along the body width.",
                        );
                        rule(
                            2.0 * n.terminal_length < n.body_length,
                            "Opposite terminals must not meet across x.",
                        );
                        (
                            n.body_length - 2.0 * n.terminal_length,
                            n.body_width - 2.0 * n.terminal_length,
                        )
                    }
                };
                if let Some([l, w]) = n.exposed_pad {
                    rule(
                        l < inner_l && w < inner_w,
                        "The exposed pad must lie inside the terminals.",
                    );
                }
            }
            Self::Header(h) => {
                rule(
                    h.rows >= 1 && h.pins_per_row >= 1,
                    "A header needs at least one pin.",
                );
                rule(
                    h.pin_side < h.pitch,
                    "Pins must not touch along a row (pin side < pitch).",
                );
                rule(
                    h.pin_side < h.row_pitch,
                    "Pins must fit the body width (pin side < row pitch).",
                );
                rule(
                    h.pin_above >= 0.0,
                    "The pin length above the body must not be negative.",
                );
            }
        }
        out
    }
}

/// The position of item `i` of `n` at `pitch`, centred on 0.
fn centred(i: u32, n: u32, pitch: f64) -> f64 {
    (f64::from(i) - f64::from(n.saturating_sub(1)) / 2.0) * pitch
}

/// Lead points of a two- or four-sided row layout in lead order. `reach`
/// is the contact's distance from the centre across y (long sides) and
/// across x (short sides).
fn ring(sides: LeadSides, per: [u32; 2], pitch: f64, reach: [f64; 2]) -> Vec<Point3> {
    let [n, m] = per;
    let along = |i: u32, count: u32| centred(i, count, pitch);
    let mut out: Vec<Point3> = (0..n).map(|i| [along(i, n), -reach[0], 0.0]).collect();
    if sides == LeadSides::Four {
        out.extend((0..m).map(|i| [reach[1], along(i, m), 0.0]));
    }
    out.extend((0..n).rev().map(|i| [along(i, n), reach[0], 0.0]));
    if sides == LeadSides::Four {
        out.extend((0..m).rev().map(|i| [-reach[1], along(i, m), 0.0]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soic8() -> GullWingForm {
        GullWingForm {
            sides: LeadSides::Two,
            leads_per_side: [4, 0],
            pitch: 1.27,
            body_length: 4.9,
            body_width: 3.9,
            body_height: 1.45,
            standoff: 0.1,
            span: [6.0, 0.0],
            lead_width: 0.41,
            lead_thickness: 0.2,
            foot_length: 0.8,
        }
    }

    fn qfn16() -> NoLeadForm {
        NoLeadForm {
            sides: LeadSides::Four,
            terminals_per_side: [4, 4],
            pitch: 0.5,
            body_length: 3.0,
            body_width: 3.0,
            body_height: 0.85,
            terminal_length: 0.4,
            terminal_width: 0.25,
            exposed_pad: Some([1.7, 1.7]),
        }
    }

    fn header() -> HeaderForm {
        HeaderForm {
            rows: 2,
            pins_per_row: 3,
            pitch: 2.54,
            row_pitch: 2.54,
            body_height: 2.5,
            pin_side: 0.64,
            pin_above: 6.0,
            pin_below: 3.0,
        }
    }

    fn names(form: &PackageForm) -> Vec<String> {
        form.values().into_iter().map(|v| v.name.0).collect()
    }

    #[test]
    fn typical_forms_have_no_range_problems() {
        for form in [
            PackageForm::Chip(Box::new(ChipForm::new(
                ChipKind::Resistor,
                [1.6, 0.8, 0.45],
            ))),
            PackageForm::GullWing(Box::new(soic8())),
            PackageForm::NoLead(Box::new(qfn16())),
            PackageForm::Header(Box::new(header())),
        ] {
            assert_eq!(
                form.range_problems(),
                Vec::<String>::new(),
                "{}",
                form.label()
            );
            assert!(form.within_bounds());
        }
    }

    #[test]
    fn envelopes_and_lead_counts() {
        let soic = PackageForm::GullWing(Box::new(soic8()));
        assert_eq!(soic.envelope(), Some([4.9, 6.0, 1.55]));
        assert_eq!(soic.lead_count(), 8);
        let qfn = PackageForm::NoLead(Box::new(qfn16()));
        assert_eq!(qfn.envelope(), Some([3.0, 3.0, 0.85]));
        assert_eq!(qfn.terminal_counts(), vec![16, 17]);
        let h = PackageForm::Header(Box::new(header()));
        assert_eq!(h.envelope(), Some([3.0 * 2.54, 2.0 * 2.54, 8.5]));
        assert_eq!(h.lead_count(), 6);
        assert_eq!(PackageForm::Generic.envelope(), None);
        assert!(PackageForm::Generic.terminal_counts().is_empty());
    }

    #[test]
    fn lead_points_run_counter_clockwise_from_the_minus_y_row() {
        let soic = PackageForm::GullWing(Box::new(soic8()));
        let p = soic.lead_points();
        assert_eq!(p.len(), 8);
        let y = 3.0 - 0.4;
        // Pin 1 at the −x end of the −y row; pin 4 at its +x end; pin 5
        // opposite pin 4; pin 8 opposite pin 1.
        assert!((p[0][0] + 1.905).abs() < 1e-12 && (p[0][1] + y).abs() < 1e-12);
        assert!((p[3][0] - 1.905).abs() < 1e-12);
        assert_eq!(p[4], [p[3][0], -p[3][1], 0.0]);
        assert_eq!(p[7], [p[0][0], -p[0][1], 0.0]);
        let qfn = PackageForm::NoLead(Box::new(qfn16()));
        let q = qfn.lead_points();
        assert_eq!(q.len(), 17, "16 terminals, then the exposed pad");
        // Pin 5 starts the +x side at its lowest y; pin 16 ends −x lowest.
        assert!(q[4][0] > 1.0 && q[4][1] < 0.0);
        assert!(q[15][0] < -1.0 && q[15][1] < 0.0);
        assert_eq!(q[16], [0.0; 3]);
        let chip = PackageForm::Chip(Box::new(ChipForm::new(
            ChipKind::Capacitor,
            [2.0, 1.25, 0.6],
        )));
        assert_eq!(chip.lead_points(), vec![[-0.8, 0.0, 0.0], [0.8, 0.0, 0.0]]);
        let h = PackageForm::Header(Box::new(header()));
        let hp = h.lead_points();
        // Column-major: pin 2 is beside pin 1 across the rows.
        assert_eq!(hp[0], [-2.54, -1.27, 0.0]);
        assert_eq!(hp[1], [-2.54, 1.27, 0.0]);
        assert_eq!(hp[5], [2.54, 1.27, 0.0]);
    }

    #[test]
    fn values_follow_the_family() {
        assert!(names(&PackageForm::Generic).is_empty());
        assert_eq!(
            names(&PackageForm::GullWing(Box::new(soic8()))),
            [
                "pkg_e", "pkg_nx", "pkg_ny", "pkg_E1", "pkg_E", "pkg_A1", "pkg_A2", "pkg_b",
                "pkg_c", "pkg_Lf", "pkg_D"
            ]
        );
        let mut qfp = soic8();
        qfp.sides = LeadSides::Four;
        qfp.leads_per_side = [3, 3];
        qfp.body_width = 4.9;
        qfp.span = [6.9, 6.9];
        let qfp = PackageForm::GullWing(Box::new(qfp));
        assert!(
            qfp.range_problems().is_empty(),
            "{:?}",
            qfp.range_problems()
        );
        let n = names(&qfp);
        assert!(n.contains(&"pkg_D1".to_owned()) && n.contains(&"pkg_D".to_owned()));
        let qfn = names(&PackageForm::NoLead(Box::new(qfn16())));
        assert!(qfn.contains(&"pkg_D2".to_owned()) && qfn.contains(&"pkg_E2".to_owned()));
        // Every name is a Solidcraft identifier under the reserved prefix.
        for name in names(&PackageForm::Header(Box::new(header())))
            .iter()
            .chain(&qfn)
        {
            assert!(name.starts_with(PACKAGE_PREFIX));
            assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        }
    }

    #[test]
    fn inconsistent_forms_are_found() {
        let mut chip = ChipForm::new(ChipKind::Resistor, [1.0, 0.5, 0.35]);
        chip.termination = 0.6;
        assert_eq!(PackageForm::Chip(Box::new(chip)).range_problems().len(), 1);
        let mut g = soic8();
        g.pitch = 0.3; // narrower than the lead
        g.span[0] = 3.5; // inside the body
        let problems = PackageForm::GullWing(Box::new(g)).range_problems();
        assert!(problems.iter().any(|p| p.contains("touch")), "{problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("reach past")),
            "{problems:?}"
        );
        let mut g = soic8();
        g.leads_per_side = [4, 2];
        assert!(
            !PackageForm::GullWing(Box::new(g))
                .range_problems()
                .is_empty()
        );
        let mut n = qfn16();
        n.exposed_pad = Some([2.5, 2.5]);
        assert!(!PackageForm::NoLead(Box::new(n)).range_problems().is_empty());
        let mut h = header();
        h.pin_side = 3.0;
        h.pin_below = 0.0;
        assert_eq!(PackageForm::Header(Box::new(h)).range_problems().len(), 3);
        let mut h = header();
        h.pitch = f64::NAN;
        assert!(!PackageForm::Header(Box::new(h)).range_problems().is_empty());
    }

    #[test]
    fn counts_beyond_the_bound_are_refused() {
        let mut h = header();
        h.rows = 100;
        h.pins_per_row = 100;
        let form = PackageForm::Header(Box::new(h));
        assert!(!form.within_bounds());
        assert!(form.lead_points().is_empty());
        let mut h = header();
        h.rows = u32::MAX;
        h.pins_per_row = u32::MAX;
        assert!(
            !PackageForm::Header(Box::new(h)).within_bounds(),
            "no overflow"
        );
    }
}
