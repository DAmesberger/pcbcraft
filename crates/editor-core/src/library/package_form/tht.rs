//! Slice B2d: the through-hole package family: axial (resistors, diodes),
//! radial (capacitors), DIP and TO (TO-92, TO-220 style single rows). The
//! form, its range rules and lead order live here; the footprint template
//! (plated drills) is `templates/footprint_family/tht.rs` and the model
//! builder (bent and formed leads) `pcbcraft-solidcraft` `models/tht.rs`.
//!
//! The seating plane z = 0 is the board's top face; leads reach
//! `lead_below` under it into the drills. Lead order: an axial or radial
//! part's lead 1 is at −x (the anode, positive or banded end when the
//! part is polarised); a DIP runs counter-clockwise from the −x end of the
//! −y row (pin 1 bottom left, seen from above); a TO row runs from −x.
use super::{FORM_LENGTH_LIMIT_MM, FORM_LENGTH_MIN_MM, FamilyForm};
use crate::geometry::Point3;
use crate::library::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The through-hole construction.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "construction", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThtKind {
    /// Cylindrical body lying flat, leads bent down at `lead_pitch`.
    Axial {
        body_diameter: f64,
        body_length: f64,
    },
    /// Cylindrical body standing up.
    Radial {
        body_diameter: f64,
        body_height: f64,
    },
    /// Dual in-line: `pins` total in two rows `row_pitch` between centres.
    Dip {
        pins: u32,
        row_pitch: f64,
        body_length: f64,
        body_width: f64,
        body_height: f64,
    },
    /// TO-92 / TO-220 style: `pins` in one row.
    To {
        pins: u32,
        body_length: f64,
        body_width: f64,
        body_height: f64,
    },
}

/// Estimated standoffs (body underside above the board) when the form
/// gives none: an axial or radial body rests on the board; a DIP stands
/// on its lead shoulders (JEDEC MS-001 gives at least 0.38 mm); a TO body
/// sits on its lead stops (est.).
pub const AXIAL_STANDOFF_MM: f64 = 0.0;
pub const RADIAL_STANDOFF_MM: f64 = 0.0;
pub const DIP_STANDOFF_MM: f64 = 0.5;
pub const TO_STANDOFF_MM: f64 = 1.0;
/// A DIP lead's strip thickness (est.: 0.2–0.36 mm); `lead_diameter` is
/// its width.
pub const DIP_LEAD_THICKNESS_MM: f64 = 0.25;
/// Share of a DIP body's height at which the leads leave it (est.).
pub const DIP_LEAD_EXIT_SHARE: f64 = 0.3;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThtForm {
    pub kind: ThtKind,
    /// Lead pitch along a row (axial: lead to lead).
    pub lead_pitch: f64,
    pub lead_diameter: f64,
    /// Lead length below the seating plane.
    pub lead_below: f64,
    /// Body underside above the seating plane; `None` is the
    /// construction's estimate (`*_STANDOFF_MM`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standoff: Option<f64>,
    /// Centreline radius of the lead bends (axial and DIP leads, formed
    /// radial and TO leads); `None` is one lead diameter (est.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bend_radius: Option<f64>,
    /// Formed leads (radial, TO): the lead pitch where the leads leave
    /// the body, when it differs from `lead_pitch` on the board (a TO-92
    /// with 1.27 mm at the body formed out to 2.54 mm). `None` is straight
    /// leads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_pitch: Option<f64>,
}

impl ThtForm {
    /// Original typical values (an 8-pin DIP in round numbers, from no
    /// drawing): the family's custom entry in the template gallery.
    pub fn typical() -> Self {
        Self::new(
            ThtKind::Dip {
                pins: 8,
                row_pitch: 7.5,
                body_length: 9.5,
                body_width: 6.5,
                body_height: 3.5,
            },
            2.5,
            0.5,
            3.0,
        )
    }

    /// A form of `kind` with straight leads and estimated standoff and
    /// bends.
    pub fn new(kind: ThtKind, lead_pitch: f64, lead_diameter: f64, lead_below: f64) -> Self {
        Self {
            kind,
            lead_pitch,
            lead_diameter,
            lead_below,
            standoff: None,
            bend_radius: None,
            body_pitch: None,
        }
    }

    /// The body underside above the seating plane.
    pub fn standoff(&self) -> f64 {
        self.standoff.unwrap_or(match self.kind {
            ThtKind::Axial { .. } => AXIAL_STANDOFF_MM,
            ThtKind::Radial { .. } => RADIAL_STANDOFF_MM,
            ThtKind::Dip { .. } => DIP_STANDOFF_MM,
            ThtKind::To { .. } => TO_STANDOFF_MM,
        })
    }

    /// The centreline bend radius.
    pub fn bend_radius(&self) -> f64 {
        self.bend_radius.unwrap_or(self.lead_diameter)
    }

    /// The pitch where the leads leave the body.
    pub fn pitch_at_body(&self) -> f64 {
        self.body_pitch.unwrap_or(self.lead_pitch)
    }

    /// Whether the leads are formed (their pitch changes below the body).
    pub fn formed(&self) -> bool {
        self.body_pitch.is_some_and(|p| p != self.lead_pitch)
    }

    /// Leads in the row (axial, radial: 2; DIP: per side; TO: all).
    pub fn per_row(&self) -> u32 {
        match self.kind {
            ThtKind::Axial { .. } | ThtKind::Radial { .. } => 2,
            ThtKind::Dip { pins, .. } => pins / 2,
            ThtKind::To { pins, .. } => pins,
        }
    }

    /// The x of lead `i` of a row of `n` at `pitch`, centred.
    pub fn along(i: u32, n: u32, pitch: f64) -> f64 {
        (f64::from(i) - f64::from(n.saturating_sub(1)) / 2.0) * pitch
    }
}

impl FamilyForm for ThtForm {
    fn label(&self) -> &'static str {
        "Through-hole"
    }
    fn lengths(&self) -> Vec<f64> {
        let mut v = vec![self.lead_pitch, self.lead_diameter, self.lead_below];
        v.extend(self.standoff);
        v.extend(self.bend_radius);
        v.extend(self.body_pitch);
        match self.kind {
            ThtKind::Axial {
                body_diameter,
                body_length,
            } => v.extend([body_diameter, body_length]),
            ThtKind::Radial {
                body_diameter,
                body_height,
            } => v.extend([body_diameter, body_height]),
            ThtKind::Dip {
                row_pitch,
                body_length,
                body_width,
                body_height,
                ..
            } => v.extend([row_pitch, body_length, body_width, body_height]),
            ThtKind::To {
                body_length,
                body_width,
                body_height,
                ..
            } => v.extend([body_length, body_width, body_height]),
        }
        v
    }
    /// Body and leads above the seating plane.
    fn envelope(&self) -> Option<[f64; 3]> {
        let s = self.standoff();
        let d = self.lead_diameter;
        Some(match self.kind {
            ThtKind::Axial {
                body_diameter,
                body_length,
            } => [
                body_length.max(self.lead_pitch + d),
                body_diameter,
                s + body_diameter,
            ],
            ThtKind::Radial {
                body_diameter,
                body_height,
            } => [
                body_diameter.max(self.lead_pitch + d),
                body_diameter,
                s + body_height,
            ],
            ThtKind::Dip {
                row_pitch,
                body_length,
                body_width,
                body_height,
                ..
            } => [
                body_length,
                body_width.max(row_pitch + DIP_LEAD_THICKNESS_MM),
                s + body_height,
            ],
            ThtKind::To {
                pins,
                body_length,
                body_width,
                body_height,
            } => {
                let row = f64::from(pins.saturating_sub(1)) * self.lead_pitch + d;
                [body_length.max(row), body_width.max(d), s + body_height]
            }
        })
    }
    fn lead_count(&self) -> u64 {
        match self.kind {
            ThtKind::Axial { .. } | ThtKind::Radial { .. } => 2,
            ThtKind::Dip { pins, .. } | ThtKind::To { pins, .. } => u64::from(pins),
        }
    }
    fn lead_points(&self) -> Vec<Point3> {
        let p = self.lead_pitch;
        match self.kind {
            ThtKind::Axial { .. } | ThtKind::Radial { .. } => {
                vec![[-p / 2.0, 0.0, 0.0], [p / 2.0, 0.0, 0.0]]
            }
            ThtKind::Dip {
                pins, row_pitch, ..
            } => {
                let n = pins / 2;
                let y = row_pitch / 2.0;
                let mut out: Vec<Point3> =
                    (0..n).map(|i| [Self::along(i, n, p), -y, 0.0]).collect();
                out.extend((0..n).rev().map(|i| [Self::along(i, n, p), y, 0.0]));
                out
            }
            ThtKind::To { pins, .. } => (0..pins)
                .map(|i| [Self::along(i, pins, p), 0.0, 0.0])
                .collect(),
        }
    }
    /// `e` (lead pitch), `b` (lead diameter or width), `L1` (lead below
    /// the board), `A1` (standoff), and per construction the body: axial
    /// `body_d`, `body_l`; radial `body_d`, `body_h`; DIP `eA` (row
    /// pitch), `D`, `E1`, `A2`; TO `D`, `E`, `A2`.
    fn values(&self) -> Vec<PackageValue> {
        let length = |name: &str, v: f64| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Length(v),
        };
        let mut v = vec![
            length("e", self.lead_pitch),
            length("b", self.lead_diameter),
            length("L1", self.lead_below),
            length("A1", self.standoff()),
        ];
        match self.kind {
            ThtKind::Axial {
                body_diameter,
                body_length,
            } => v.extend([
                length("body_d", body_diameter),
                length("body_l", body_length),
            ]),
            ThtKind::Radial {
                body_diameter,
                body_height,
            } => v.extend([
                length("body_d", body_diameter),
                length("body_h", body_height),
            ]),
            ThtKind::Dip {
                row_pitch,
                body_length,
                body_width,
                body_height,
                ..
            } => v.extend([
                length("eA", row_pitch),
                length("D", body_length),
                length("E1", body_width),
                length("A2", body_height),
            ]),
            ThtKind::To {
                body_length,
                body_width,
                body_height,
                ..
            } => v.extend([
                length("D", body_length),
                length("E", body_width),
                length("A2", body_height),
            ]),
        }
        v
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
        positive("Lead pitch", self.lead_pitch);
        positive("Lead diameter", self.lead_diameter);
        positive("Lead length below the board", self.lead_below);
        if let Some(r) = self.bend_radius {
            positive("Bend radius", r);
        }
        if let Some(p) = self.body_pitch {
            positive("Lead pitch at the body", p);
        }
        match self.kind {
            ThtKind::Axial {
                body_diameter,
                body_length,
            } => {
                positive("Body diameter", body_diameter);
                positive("Body length", body_length);
            }
            ThtKind::Radial {
                body_diameter,
                body_height,
            } => {
                positive("Body diameter", body_diameter);
                positive("Body height", body_height);
            }
            ThtKind::Dip {
                row_pitch,
                body_length,
                body_width,
                body_height,
                ..
            } => {
                positive("Row pitch", row_pitch);
                positive("Body length", body_length);
                positive("Body width", body_width);
                positive("Body height", body_height);
            }
            ThtKind::To {
                body_length,
                body_width,
                body_height,
                ..
            } => {
                positive("Body length", body_length);
                positive("Body width", body_width);
                positive("Body height", body_height);
            }
        }
        let mut rule = |ok: bool, message: &str| {
            if !ok {
                out.push(message.to_owned());
            }
        };
        let s = self.standoff();
        rule(
            s.is_finite() && (0.0..=FORM_LENGTH_LIMIT_MM).contains(&s),
            "The standoff must be 0 mm or more.",
        );
        let d = self.lead_diameter;
        let p = self.lead_pitch;
        // Each lead's bend needs a lead diameter of room.
        let bend_room = |room: f64| room > d;
        match self.kind {
            ThtKind::Axial {
                body_diameter,
                body_length,
            } => {
                rule(
                    bend_room((p - body_length) / 2.0),
                    "The leads must bend down outside the body ((lead pitch − body length) / 2 > lead diameter).",
                );
                rule(
                    d < body_diameter,
                    "The leads must be thinner than the body (lead diameter < body diameter).",
                );
                rule(
                    self.body_pitch.is_none(),
                    "Axial leads have no pitch at the body.",
                );
            }
            ThtKind::Radial { body_diameter, .. } => {
                rule(
                    self.pitch_at_body() + d <= body_diameter + 1e-9,
                    "The leads must leave the body's underside (pitch at the body + lead diameter ≤ body diameter).",
                );
                rule(
                    self.lead_pitch > d,
                    "The leads must not touch (lead pitch > lead diameter).",
                );
                if self.formed() {
                    rule(
                        s >= 2.0 * d,
                        "Formed leads need a standoff of at least two lead diameters to bend in.",
                    );
                }
            }
            ThtKind::Dip {
                pins,
                row_pitch,
                body_length,
                body_width,
                ..
            } => {
                rule(
                    pins >= 2 && pins.is_multiple_of(2),
                    "A DIP has an even number of pins (2 or more).",
                );
                rule(p > d, "The leads must not touch (lead pitch > lead width).");
                let n = pins / 2;
                rule(
                    f64::from(n.saturating_sub(1)) * p + d <= body_length + 1e-9,
                    "The leads of a row must fit along the body length.",
                );
                rule(
                    (row_pitch - body_width) / 2.0 > DIP_LEAD_THICKNESS_MM,
                    "The rows must lie outside the body with room for the lead bend ((row pitch − body width) / 2 > lead thickness).",
                );
                rule(
                    self.body_pitch.is_none(),
                    "DIP leads have no separate pitch at the body.",
                );
            }
            ThtKind::To {
                pins,
                body_length,
                body_width,
                ..
            } => {
                rule(pins >= 1, "A TO package needs at least one lead.");
                rule(
                    pins < 2 || p > d,
                    "The leads must not touch (lead pitch > lead diameter).",
                );
                let at_body = f64::from(pins.saturating_sub(1)) * self.pitch_at_body() + d;
                rule(
                    at_body <= body_length + 1e-9,
                    "The leads must leave the body's underside along its length.",
                );
                rule(
                    pins < 2 || self.pitch_at_body() > d,
                    "The leads must not touch where they leave the body.",
                );
                rule(
                    d <= body_width,
                    "The leads must fit the body's width (lead diameter ≤ body width).",
                );
                if self.formed() {
                    rule(
                        s >= 2.0 * d,
                        "Formed leads need a standoff of at least two lead diameters to bend in.",
                    );
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::PackageForm;

    // Original synthetic forms with round numbers, from no library.
    fn axial() -> ThtForm {
        ThtForm::new(
            ThtKind::Axial {
                body_diameter: 2.5,
                body_length: 6.0,
            },
            10.0,
            0.6,
            3.0,
        )
    }

    fn radial() -> ThtForm {
        ThtForm::new(
            ThtKind::Radial {
                body_diameter: 5.0,
                body_height: 11.0,
            },
            2.0,
            0.5,
            3.0,
        )
    }

    fn dip8() -> ThtForm {
        ThtForm::new(
            ThtKind::Dip {
                pins: 8,
                row_pitch: 7.5,
                body_length: 9.5,
                body_width: 6.5,
                body_height: 3.5,
            },
            2.5,
            0.5,
            3.0,
        )
    }

    fn to3() -> ThtForm {
        ThtForm::new(
            ThtKind::To {
                pins: 3,
                body_length: 4.5,
                body_width: 3.5,
                body_height: 4.5,
            },
            1.25,
            0.45,
            3.0,
        )
    }

    #[test]
    fn typical_forms_have_no_range_problems() {
        for f in [axial(), radial(), dip8(), to3()] {
            assert_eq!(f.range_problems(), Vec::<String>::new(), "{f:?}");
        }
    }

    #[test]
    fn lead_points_follow_the_construction() {
        assert_eq!(
            axial().lead_points(),
            vec![[-5.0, 0.0, 0.0], [5.0, 0.0, 0.0]]
        );
        let dip = dip8().lead_points();
        assert_eq!(dip.len(), 8);
        // Pin 1 bottom left; pin 4 bottom right; pin 5 above pin 4; pin 8
        // above pin 1.
        assert_eq!(dip[0], [-3.75, -3.75, 0.0]);
        assert_eq!(dip[3], [3.75, -3.75, 0.0]);
        assert_eq!(dip[4], [3.75, 3.75, 0.0]);
        assert_eq!(dip[7], [-3.75, 3.75, 0.0]);
        assert_eq!(
            to3().lead_points(),
            vec![[-1.25, 0.0, 0.0], [0.0, 0.0, 0.0], [1.25, 0.0, 0.0]]
        );
    }

    #[test]
    fn envelopes_include_the_standoff() {
        assert_eq!(axial().envelope(), Some([10.6, 2.5, 2.5]));
        assert_eq!(dip8().envelope(), Some([9.5, 7.75, 4.0]));
        let mut to = to3();
        to.standoff = Some(2.0);
        assert_eq!(to.envelope(), Some([4.5, 3.5, 6.5]));
        assert_eq!(
            PackageForm::Tht(Box::new(dip8())).terminal_counts(),
            vec![8]
        );
    }

    #[test]
    fn values_name_each_construction() {
        let names =
            |f: ThtForm| -> Vec<String> { f.values().into_iter().map(|v| v.name.0).collect() };
        assert_eq!(
            names(dip8()),
            [
                "pkg_e", "pkg_b", "pkg_L1", "pkg_A1", "pkg_eA", "pkg_D", "pkg_E1", "pkg_A2"
            ]
        );
        assert!(names(axial()).contains(&"pkg_body_l".to_owned()));
        assert!(names(radial()).contains(&"pkg_body_h".to_owned()));
    }

    #[test]
    fn inconsistent_forms_are_found() {
        let problem = |f: ThtForm, needle: &str| {
            let p = f.range_problems();
            assert!(p.iter().any(|m| m.contains(needle)), "{needle}: {p:?}");
        };
        let mut f = axial();
        f.lead_pitch = 6.5;
        problem(f, "bend down outside");
        let mut f = axial();
        f.body_pitch = Some(5.0);
        problem(f, "no pitch at the body");
        let mut f = radial();
        f.lead_pitch = 5.0;
        problem(f, "underside");
        // Formed out to 5 mm from 2 mm at the body: needs a standoff.
        let mut f = radial();
        f.lead_pitch = 5.0;
        f.body_pitch = Some(2.0);
        problem(f, "standoff");
        f.standoff = Some(1.5);
        assert!(f.range_problems().is_empty(), "{:?}", f.range_problems());
        let mut f = dip8();
        f.kind = ThtKind::Dip {
            pins: 7,
            row_pitch: 7.5,
            body_length: 9.5,
            body_width: 6.5,
            body_height: 3.5,
        };
        problem(f, "even");
        let mut f = dip8();
        f.kind = ThtKind::Dip {
            pins: 8,
            row_pitch: 6.6,
            body_length: 9.5,
            body_width: 6.5,
            body_height: 3.5,
        };
        problem(f, "outside the body");
        let mut f = to3();
        f.lead_pitch = 0.4;
        problem(f, "touch");
        let mut f = to3();
        f.standoff = Some(-1.0);
        problem(f, "standoff");
        let mut f = to3();
        f.lead_below = f64::NAN;
        problem(f, "below the board");
    }

    #[test]
    fn optional_forming_fields_are_left_out_when_absent() {
        let json = serde_json::to_string(&dip8()).unwrap_or_default();
        assert!(
            !json.contains("standoff") && !json.contains("body_pitch"),
            "{json}"
        );
        let back: ThtForm = serde_json::from_str(&json).unwrap_or(axial());
        assert_eq!(back, dip8());
        let mut to = to3();
        to.body_pitch = Some(1.0);
        let json = serde_json::to_string(&to).unwrap_or_default();
        assert!(json.contains("body_pitch"));
    }

    #[test]
    fn counts_beyond_the_bound_are_refused() {
        let mut f = to3();
        f.kind = ThtKind::To {
            pins: u32::MAX,
            body_length: 4.5,
            body_width: 3.5,
            body_height: 4.5,
        };
        let form = PackageForm::Tht(Box::new(f));
        assert!(!form.within_bounds());
        assert!(form.lead_points().is_empty());
    }
}
