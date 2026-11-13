//! Slice B2d: the through-hole package family: axial (resistors, diodes),
//! radial (capacitors), DIP and TO (TO-92, TO-220 style single rows). The
//! form, its range rules and lead order live here; the footprint template
//! (plated drills) is `templates/footprint_family/tht.rs` and the model
//! builder (bent and formed leads) `sparksmith-solidcraft` `models/tht.rs`.
//!
//! The seating plane z = 0 is the board's top face; leads reach
//! `lead_below` under it into the drills. Lead order: an axial or radial
//! part's lead 1 is at −x (the anode, positive or banded end when the
//! part is polarised); a DIP runs counter-clockwise from the −x end of the
//! −y row (pin 1 bottom left, seen from above); a TO row runs from −x.
use super::{FORM_LENGTH_LIMIT_NM, FamilyForm, length_problem, typical_nm};
use crate::geometry::Point3;
use crate::library::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use crate::units::{self, Nm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The through-hole construction.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(tag = "construction", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThtKind {
    /// Cylindrical body lying flat, leads bent down at `lead_pitch`.
    Axial {
        body_diameter_nm: Nm,
        body_length_nm: Nm,
    },
    /// Cylindrical body standing up.
    Radial {
        body_diameter_nm: Nm,
        body_height_nm: Nm,
    },
    /// Dual in-line: `pins` total in two rows `row_pitch` between centres.
    Dip {
        pins: u32,
        row_pitch_nm: Nm,
        body_length_nm: Nm,
        body_width_nm: Nm,
        body_height_nm: Nm,
    },
    /// TO-92 / TO-220 style: `pins` in one row.
    To {
        pins: u32,
        body_length_nm: Nm,
        body_width_nm: Nm,
        body_height_nm: Nm,
    },
}

/// A [`ThtKind`]'s body in millimetres, for computing: the stored
/// construction through [`Nm::to_mm_f64`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThtBody {
    Axial {
        body_diameter: f64,
        body_length: f64,
    },
    Radial {
        body_diameter: f64,
        body_height: f64,
    },
    Dip {
        pins: u32,
        row_pitch: f64,
        body_length: f64,
        body_width: f64,
        body_height: f64,
    },
    To {
        pins: u32,
        body_length: f64,
        body_width: f64,
        body_height: f64,
    },
}

impl ThtKind {
    /// The construction in millimetres.
    pub fn mm(&self) -> ThtBody {
        match *self {
            Self::Axial {
                body_diameter_nm,
                body_length_nm,
            } => ThtBody::Axial {
                body_diameter: body_diameter_nm.to_mm_f64(),
                body_length: body_length_nm.to_mm_f64(),
            },
            Self::Radial {
                body_diameter_nm,
                body_height_nm,
            } => ThtBody::Radial {
                body_diameter: body_diameter_nm.to_mm_f64(),
                body_height: body_height_nm.to_mm_f64(),
            },
            Self::Dip {
                pins,
                row_pitch_nm,
                body_length_nm,
                body_width_nm,
                body_height_nm,
            } => ThtBody::Dip {
                pins,
                row_pitch: row_pitch_nm.to_mm_f64(),
                body_length: body_length_nm.to_mm_f64(),
                body_width: body_width_nm.to_mm_f64(),
                body_height: body_height_nm.to_mm_f64(),
            },
            Self::To {
                pins,
                body_length_nm,
                body_width_nm,
                body_height_nm,
            } => ThtBody::To {
                pins,
                body_length: body_length_nm.to_mm_f64(),
                body_width: body_width_nm.to_mm_f64(),
                body_height: body_height_nm.to_mm_f64(),
            },
        }
    }

    /// Every stored body length.
    fn lengths_nm(&self) -> Vec<Nm> {
        match *self {
            Self::Axial {
                body_diameter_nm,
                body_length_nm,
            } => vec![body_diameter_nm, body_length_nm],
            Self::Radial {
                body_diameter_nm,
                body_height_nm,
            } => vec![body_diameter_nm, body_height_nm],
            Self::Dip {
                row_pitch_nm,
                body_length_nm,
                body_width_nm,
                body_height_nm,
                ..
            } => vec![row_pitch_nm, body_length_nm, body_width_nm, body_height_nm],
            Self::To {
                body_length_nm,
                body_width_nm,
                body_height_nm,
                ..
            } => vec![body_length_nm, body_width_nm, body_height_nm],
        }
    }
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
/// Drill clearance over the lead's largest cross-section, mm (estimated:
/// a plated hole a few tenths larger than the lead inserts by hand or
/// machine).
pub const DRILL_CLEARANCE_MM: f64 = 0.3;
/// Copper ring added to the drill diameter, mm (estimated: a 0.3 mm
/// annular ring each side).
pub const RING_MM: f64 = 0.6;
/// Copper left between neighbouring lands, mm (estimated).
pub const LAND_GAP_MM: f64 = 0.25;
/// The smallest ring a tight pitch may shrink the land to, mm.
pub const MIN_RING_MM: f64 = 0.2;
/// Share of a DIP body's height at which the leads leave it (est.).
pub const DIP_LEAD_EXIT_SHARE: f64 = 0.3;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(deny_unknown_fields)]
pub struct ThtForm {
    #[params]
    pub kind: ThtKind,
    /// Lead pitch along a row (axial: lead to lead).
    pub lead_pitch_nm: Nm,
    pub lead_diameter_nm: Nm,
    /// Lead length below the seating plane.
    pub lead_below_nm: Nm,
    /// Body underside above the seating plane; `None` is the
    /// construction's estimate (`*_STANDOFF_MM`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standoff_nm: Option<Nm>,
    /// Centreline radius of the lead bends (axial and DIP leads, formed
    /// radial and TO leads); `None` is one lead diameter (est.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bend_radius_nm: Option<Nm>,
    /// Formed leads (radial, TO): the lead pitch where the leads leave
    /// the body, when it differs from `lead_pitch` on the board (a TO-92
    /// with 1.27 mm at the body formed out to 2.54 mm). `None` is straight
    /// leads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_pitch_nm: Option<Nm>,
}

mm_accessors!(ThtForm {
    lead_pitch: lead_pitch_nm => len,
    lead_diameter: lead_diameter_nm => len,
    lead_below: lead_below_nm => len,
    body_pitch: body_pitch_nm => opt_len,
});

impl ThtForm {
    /// Original typical values (an 8-pin DIP in round numbers, from no
    /// drawing): the family's custom entry in the template gallery.
    pub fn typical() -> Self {
        let mm = typical_nm;
        Self::new(
            ThtKind::Dip {
                pins: 8,
                row_pitch_nm: mm(7.5),
                body_length_nm: mm(9.5),
                body_width_nm: mm(6.5),
                body_height_nm: mm(3.5),
            },
            mm(2.5),
            mm(0.5),
            mm(3.0),
        )
    }

    /// A form of `kind` with straight leads and estimated standoff and
    /// bends.
    pub fn new(kind: ThtKind, lead_pitch: Nm, lead_diameter: Nm, lead_below: Nm) -> Self {
        Self {
            kind,
            lead_pitch_nm: lead_pitch,
            lead_diameter_nm: lead_diameter,
            lead_below_nm: lead_below,
            standoff_nm: None,
            bend_radius_nm: None,
            body_pitch_nm: None,
        }
    }

    /// The body underside above the seating plane.
    pub fn standoff(&self) -> f64 {
        self.standoff_nm
            .map(Nm::to_mm_f64)
            .unwrap_or(match self.kind {
                ThtKind::Axial { .. } => AXIAL_STANDOFF_MM,
                ThtKind::Radial { .. } => RADIAL_STANDOFF_MM,
                ThtKind::Dip { .. } => DIP_STANDOFF_MM,
                ThtKind::To { .. } => TO_STANDOFF_MM,
            })
    }

    /// The centreline bend radius.
    pub fn bend_radius(&self) -> f64 {
        self.bend_radius_nm
            .unwrap_or(self.lead_diameter_nm)
            .to_mm_f64()
    }

    /// The pitch where the leads leave the body.
    pub fn pitch_at_body(&self) -> f64 {
        self.body_pitch_nm.unwrap_or(self.lead_pitch_nm).to_mm_f64()
    }

    /// Whether the leads are formed (their pitch changes below the body).
    pub fn formed(&self) -> bool {
        self.body_pitch_nm.is_some_and(|p| p != self.lead_pitch_nm)
    }

    /// Leads in the row (axial, radial: 2; DIP: per side; TO: all).
    pub fn per_row(&self) -> u32 {
        match self.kind {
            ThtKind::Axial { .. } | ThtKind::Radial { .. } => 2,
            ThtKind::Dip { pins, .. } => pins / 2,
            ThtKind::To { pins, .. } => pins,
        }
    }

    /// The estimated drill and copper land diameters of the leads, mm:
    /// the drill clears the lead's largest cross-section by
    /// [`DRILL_CLEARANCE_MM`] (rounded up to 0.05 mm), the land adds a
    /// [`RING_MM`] ring but stays [`LAND_GAP_MM`] inside the pitch, keeping
    /// at least a [`MIN_RING_MM`] ring (rounded down to 0.05 mm). Estimated
    /// prototype values, not a compliance claim; the footprint template
    /// and `samples::form_lands` both use them.
    pub fn estimated_drill_and_land(&self) -> (f64, f64) {
        let d = self.lead_diameter();
        let section = match self.kind.mm() {
            ThtBody::Axial { .. } | ThtBody::Radial { .. } => d,
            ThtBody::Dip { .. } => d.hypot(DIP_LEAD_THICKNESS_MM),
            ThtBody::To { .. } => d * std::f64::consts::SQRT_2,
        };
        let drill = ((section + DRILL_CLEARANCE_MM) * 20.0).ceil() / 20.0;
        let land = (drill + RING_MM)
            .min(self.lead_pitch() - LAND_GAP_MM)
            .max(drill + MIN_RING_MM);
        (drill, (land * 20.0).floor() / 20.0)
    }

    /// Whether pin 1 is marked (a rounded-square land): DIP and TO parts.
    pub fn marks_pin_one(&self) -> bool {
        matches!(self.kind, ThtKind::Dip { .. } | ThtKind::To { .. })
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
        let mut v = vec![
            self.lead_pitch_nm,
            self.lead_diameter_nm,
            self.lead_below_nm,
        ];
        v.extend(self.standoff_nm);
        v.extend(self.bend_radius_nm);
        v.extend(self.body_pitch_nm);
        v.extend(self.kind.lengths_nm());
        v.into_iter().map(Nm::to_mm_f64).collect()
    }
    /// Body and leads above the seating plane.
    fn envelope(&self) -> Option<[f64; 3]> {
        let s = self.standoff();
        let d = self.lead_diameter();
        Some(match self.kind.mm() {
            ThtBody::Axial {
                body_diameter,
                body_length,
            } => [
                body_length.max(self.lead_pitch() + d),
                body_diameter,
                s + body_diameter,
            ],
            ThtBody::Radial {
                body_diameter,
                body_height,
            } => [
                body_diameter.max(self.lead_pitch() + d),
                body_diameter,
                s + body_height,
            ],
            ThtBody::Dip {
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
            ThtBody::To {
                pins,
                body_length,
                body_width,
                body_height,
            } => {
                let row = f64::from(pins.saturating_sub(1)) * self.lead_pitch() + d;
                [body_length.max(row), body_width.max(d), s + body_height]
            }
        })
    }
    fn lead_count(&self) -> u64 {
        match self.kind.mm() {
            ThtBody::Axial { .. } | ThtBody::Radial { .. } => 2,
            ThtBody::Dip { pins, .. } | ThtBody::To { pins, .. } => u64::from(pins),
        }
    }
    fn lead_points(&self) -> Vec<Point3> {
        let p = self.lead_pitch();
        match self.kind.mm() {
            ThtBody::Axial { .. } | ThtBody::Radial { .. } => {
                vec![[-p / 2.0, 0.0, 0.0], [p / 2.0, 0.0, 0.0]]
            }
            ThtBody::Dip {
                pins, row_pitch, ..
            } => {
                let n = pins / 2;
                let y = row_pitch / 2.0;
                let mut out: Vec<Point3> =
                    (0..n).map(|i| [Self::along(i, n, p), -y, 0.0]).collect();
                out.extend((0..n).rev().map(|i| [Self::along(i, n, p), y, 0.0]));
                out
            }
            ThtBody::To { pins, .. } => (0..pins)
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
            length("e", self.lead_pitch()),
            length("b", self.lead_diameter()),
            length("L1", self.lead_below()),
            length("A1", self.standoff()),
        ];
        match self.kind.mm() {
            ThtBody::Axial {
                body_diameter,
                body_length,
            } => v.extend([
                length("body_d", body_diameter),
                length("body_l", body_length),
            ]),
            ThtBody::Radial {
                body_diameter,
                body_height,
            } => v.extend([
                length("body_d", body_diameter),
                length("body_h", body_height),
            ]),
            ThtBody::Dip {
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
            ThtBody::To {
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
        let mut positive = |name: &str, v: Nm| out.extend(length_problem(name, v));
        positive("Lead pitch", self.lead_pitch_nm);
        positive("Lead diameter", self.lead_diameter_nm);
        positive("Lead length below the board", self.lead_below_nm);
        if let Some(r) = self.bend_radius_nm {
            positive("Bend radius", r);
        }
        if let Some(p) = self.body_pitch_nm {
            positive("Lead pitch at the body", p);
        }
        match self.kind {
            ThtKind::Axial {
                body_diameter_nm,
                body_length_nm,
            } => {
                positive("Body diameter", body_diameter_nm);
                positive("Body length", body_length_nm);
            }
            ThtKind::Radial {
                body_diameter_nm,
                body_height_nm,
            } => {
                positive("Body diameter", body_diameter_nm);
                positive("Body height", body_height_nm);
            }
            ThtKind::Dip {
                row_pitch_nm,
                body_length_nm,
                body_width_nm,
                body_height_nm,
                ..
            } => {
                positive("Row pitch", row_pitch_nm);
                positive("Body length", body_length_nm);
                positive("Body width", body_width_nm);
                positive("Body height", body_height_nm);
            }
            ThtKind::To {
                body_length_nm,
                body_width_nm,
                body_height_nm,
                ..
            } => {
                positive("Body length", body_length_nm);
                positive("Body width", body_width_nm);
                positive("Body height", body_height_nm);
            }
        }
        let mut rule = |ok: bool, message: &str| {
            if !ok {
                out.push(message.to_owned());
            }
        };
        let s = self.standoff();
        rule(
            self.standoff_nm
                .is_none_or(|s| (Nm::ZERO..=FORM_LENGTH_LIMIT_NM).contains(&s)),
            "The standoff must be 0 mm or more.",
        );
        let d = self.lead_diameter();
        let p = self.lead_pitch();
        // Each lead's bend needs a lead diameter of room.
        let bend_room = |room: f64| room > d;
        match self.kind.mm() {
            ThtBody::Axial {
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
                    self.body_pitch_nm.is_none(),
                    "Axial leads have no pitch at the body.",
                );
            }
            ThtBody::Radial { body_diameter, .. } => {
                rule(
                    self.pitch_at_body() + d <= body_diameter + 1e-9,
                    "The leads must leave the body's underside (pitch at the body + lead diameter ≤ body diameter).",
                );
                rule(
                    self.lead_pitch() > d,
                    "The leads must not touch (lead pitch > lead diameter).",
                );
                if self.formed() {
                    rule(
                        s >= 2.0 * d,
                        "Formed leads need a standoff of at least two lead diameters to bend in.",
                    );
                }
            }
            ThtBody::Dip {
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
                    self.body_pitch_nm.is_none(),
                    "DIP leads have no separate pitch at the body.",
                );
            }
            ThtBody::To {
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
    use crate::tests::support::nm;

    // Original synthetic forms with round numbers, from no library.
    fn axial() -> ThtForm {
        ThtForm::new(
            ThtKind::Axial {
                body_diameter_nm: nm(2.5),
                body_length_nm: nm(6.0),
            },
            nm(10.0),
            nm(0.6),
            nm(3.0),
        )
    }

    fn radial() -> ThtForm {
        ThtForm::new(
            ThtKind::Radial {
                body_diameter_nm: nm(5.0),
                body_height_nm: nm(11.0),
            },
            nm(2.0),
            nm(0.5),
            nm(3.0),
        )
    }

    fn dip8() -> ThtForm {
        ThtForm::new(
            ThtKind::Dip {
                pins: 8,
                row_pitch_nm: nm(7.5),
                body_length_nm: nm(9.5),
                body_width_nm: nm(6.5),
                body_height_nm: nm(3.5),
            },
            nm(2.5),
            nm(0.5),
            nm(3.0),
        )
    }

    fn to3() -> ThtForm {
        ThtForm::new(
            ThtKind::To {
                pins: 3,
                body_length_nm: nm(4.5),
                body_width_nm: nm(3.5),
                body_height_nm: nm(4.5),
            },
            nm(1.25),
            nm(0.45),
            nm(3.0),
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
        to.standoff_nm = Some(nm(2.0));
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
        f.lead_pitch_nm = nm(6.5);
        problem(f, "bend down outside");
        let mut f = axial();
        f.body_pitch_nm = Some(nm(5.0));
        problem(f, "no pitch at the body");
        let mut f = radial();
        f.lead_pitch_nm = nm(5.0);
        problem(f, "underside");
        // Formed out to 5 mm from 2 mm at the body: needs a standoff.
        let mut f = radial();
        f.lead_pitch_nm = nm(5.0);
        f.body_pitch_nm = Some(nm(2.0));
        problem(f, "standoff");
        f.standoff_nm = Some(nm(1.5));
        assert!(f.range_problems().is_empty(), "{:?}", f.range_problems());
        let mut f = dip8();
        f.kind = ThtKind::Dip {
            pins: 7,
            row_pitch_nm: nm(7.5),
            body_length_nm: nm(9.5),
            body_width_nm: nm(6.5),
            body_height_nm: nm(3.5),
        };
        problem(f, "even");
        let mut f = dip8();
        f.kind = ThtKind::Dip {
            pins: 8,
            row_pitch_nm: nm(6.6),
            body_length_nm: nm(9.5),
            body_width_nm: nm(6.5),
            body_height_nm: nm(3.5),
        };
        problem(f, "outside the body");
        let mut f = to3();
        f.lead_pitch_nm = nm(0.4);
        problem(f, "touch");
        let mut f = to3();
        f.standoff_nm = Some(nm(-1.0));
        problem(f, "standoff");
        let mut f = to3();
        // Out of range (a stored length cannot be NaN).
        f.lead_below_nm = Nm::ZERO;
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
        to.body_pitch_nm = Some(nm(1.0));
        let json = serde_json::to_string(&to).unwrap_or_default();
        assert!(json.contains("body_pitch"));
    }

    #[test]
    fn counts_beyond_the_bound_are_refused() {
        let mut f = to3();
        f.kind = ThtKind::To {
            pins: u32::MAX,
            body_length_nm: nm(4.5),
            body_width_nm: nm(3.5),
            body_height_nm: nm(4.5),
        };
        let form = PackageForm::Tht(Box::new(f));
        assert!(!form.within_bounds());
        assert!(form.lead_points().is_empty());
    }
}
