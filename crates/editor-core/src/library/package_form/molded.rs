//! Slice B2b: the molded two-terminal package family: SOD-123/323/523,
//! SMA/SMB/SMC, molded tantalum A–E, MELF and MiniMELF. Terminal 1 is at
//! −x; a polarised form carries a band on the body's top at terminal 1,
//! whose area ([`MoldedForm::band_area`]) and the remaining marking area
//! ([`MoldedForm::marking_area`]) are data. The footprint template is
//! `templates/footprint_family/molded.rs` and the model builder
//! `pcbcraft-solidcraft` `models/molded.rs`.
use super::{FamilyForm, length_problem, typical_nm};
use crate::geometry::Point3;
use crate::library::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use crate::library::model3d::MarkingArea;
use crate::units::{self, Nm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Leadframe thickness of flat and bent leads, and of J-leads (est.:
/// small-signal SOD leadframes are 0.1–0.15 mm, power J-lead and tantalum
/// frames 0.15–0.3 mm), each at most a quarter of the body height.
pub const FLAT_LEAD_MM: f64 = 0.12;
pub const J_LEAD_MM: f64 = 0.15;
/// The gap under a bent-lead body (est.).
pub const BENT_STANDOFF_MM: f64 = 0.05;
/// The polarity band's share of the body length and its inset from the
/// body end (est.: a laser or ink band of a fifth to a sixth of the body).
pub const BAND_SHARE: f64 = 0.15;
pub const BAND_INSET_SHARE: f64 = 0.08;
/// Share of the body width a band or marking spans (est.).
const ACROSS_SHARE: f64 = 0.8;

/// How the two terminals are formed.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MoldedLead {
    /// Flat leads under the body ends (SOD-123F, SOD-523, SMA flat).
    Flat,
    /// Leads leaving the body ends at mid-height and bending down to a
    /// flat foot (SOD-123, SOD-323).
    Bent,
    /// J-leads folded down the end faces and under the body (SMA/SMB/SMC,
    /// tantalum).
    JLead,
    /// Cylindrical end caps (MELF, MiniMELF).
    Melf,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(deny_unknown_fields)]
pub struct MoldedForm {
    pub lead: MoldedLead,
    /// Moulded body length (x), width (y) and height above the seating
    /// plane. A MELF body is round: its width and height are its diameter.
    pub body_length_nm: Nm,
    pub body_width_nm: Nm,
    pub body_height_nm: Nm,
    /// Overall length, terminal tip to tip.
    pub span_nm: Nm,
    /// Terminal contact length on the seating plane, from the tip inward
    /// (a MELF cap's length).
    pub terminal_length_nm: Nm,
    /// Terminal width across y (a MELF cap's diameter).
    pub terminal_width_nm: Nm,
    /// A polarity band at terminal 1, as data.
    pub polarised: bool,
}

mm_accessors!(MoldedForm {
    body_length: body_length_nm => len,
    body_width: body_width_nm => len,
    body_height: body_height_nm => len,
    span: span_nm => len,
    terminal_length: terminal_length_nm => len,
    terminal_width: terminal_width_nm => len,
});

impl MoldedForm {
    /// Original typical values (a SOD-123-sized body in round numbers, from
    /// no drawing): the family's custom entry in the template gallery.
    pub fn typical() -> Self {
        Self {
            lead: MoldedLead::Bent,
            body_length_nm: typical_nm(2.7),
            body_width_nm: typical_nm(1.6),
            body_height_nm: typical_nm(1.1),
            span_nm: typical_nm(3.7),
            terminal_length_nm: typical_nm(0.3),
            terminal_width_nm: typical_nm(0.6),
            polarised: true,
        }
    }

    /// The lead (leadframe) thickness the model and the rules use; 0 for a
    /// MELF.
    pub fn lead_thickness(&self) -> f64 {
        let quarter = self.body_height() / 4.0;
        match self.lead {
            MoldedLead::Flat | MoldedLead::Bent => FLAT_LEAD_MM.min(quarter),
            MoldedLead::JLead => J_LEAD_MM.min(quarter),
            MoldedLead::Melf => 0.0,
        }
    }

    /// Height of the moulded body's underside.
    pub fn body_bottom(&self) -> f64 {
        match self.lead {
            MoldedLead::Flat | MoldedLead::JLead => 0.0,
            MoldedLead::Bent => BENT_STANDOFF_MM.min(self.body_height() / 10.0),
            MoldedLead::Melf => ((self.terminal_width() - self.body_width()) / 2.0).max(0.0),
        }
    }

    /// Height of the body's top.
    pub fn body_top(&self) -> f64 {
        match self.lead {
            MoldedLead::Melf => self.body_bottom() + self.body_width(),
            _ => self.body_height(),
        }
    }

    /// Height of a bent or J-lead's centre line where it leaves the body.
    pub fn lead_exit(&self) -> f64 {
        let bottom = self.body_bottom();
        bottom + 0.5 * (self.body_height() - bottom)
    }

    /// The x range of body visible on top between the terminals, from −x.
    fn top_span(&self) -> [f64; 2] {
        let half = match self.lead {
            MoldedLead::Melf => {
                (self.span() / 2.0 - self.terminal_length()).min(self.body_length() / 2.0)
            }
            _ => self.body_length() / 2.0,
        };
        [-half, half]
    }

    /// The polarity band on the top at terminal 1 (−x), origin at its
    /// centre, u along +x; `None` unless polarised.
    pub fn band_area(&self) -> Option<MarkingArea> {
        if !self.polarised {
            return None;
        }
        let [lo, _] = self.top_span();
        let width = BAND_SHARE * self.body_length();
        let start = lo + BAND_INSET_SHARE * self.body_length();
        MarkingArea::from_mm(
            [start + width / 2.0, 0.0, self.body_top()],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [width, ACROSS_SHARE * self.body_width()],
        )
        .ok()
    }

    /// The top area left for text markings: the visible top beside the
    /// band, origin at its centre, u along +x. Computed from the stored
    /// form, it is always storable; `None` only if it were not.
    pub fn marking_area(&self) -> Option<MarkingArea> {
        let [mut lo, hi] = self.top_span();
        let inset = BAND_INSET_SHARE * self.body_length();
        if let Some(band) = self.band_area() {
            lo = band.origin()[0] + band.size()[0] / 2.0;
        } else {
            lo += inset;
        }
        let hi = hi - inset;
        MarkingArea::from_mm(
            [(lo + hi) / 2.0, 0.0, self.body_top()],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [(hi - lo).max(0.0), ACROSS_SHARE * self.body_width()],
        )
        .ok()
    }

    /// Estimated land sizes [along x, along y] of terminals 1 and 2 (toe
    /// 0.4 mm, heel 0.2 mm, side 0.1 mm; estimated prototype values, not a
    /// compliance claim).
    pub fn land_sizes(&self) -> Vec<[f64; 2]> {
        vec![[self.terminal_length() + 0.6, self.terminal_width() + 0.2]; 2]
    }
}

impl FamilyForm for MoldedForm {
    fn label(&self) -> &'static str {
        "Molded two-terminal"
    }

    fn lengths(&self) -> Vec<f64> {
        vec![
            self.body_length(),
            self.body_width(),
            self.body_height(),
            self.span(),
            self.terminal_length(),
            self.terminal_width(),
        ]
    }

    fn envelope(&self) -> Option<[f64; 3]> {
        let (width, height) = match self.lead {
            MoldedLead::Melf => {
                let d = self.terminal_width().max(self.body_width());
                (d, d)
            }
            _ => (
                self.body_width().max(self.terminal_width()),
                self.body_height(),
            ),
        };
        Some([self.span().max(self.body_length()), width, height])
    }

    fn lead_count(&self) -> u64 {
        2
    }

    fn lead_points(&self) -> Vec<Point3> {
        let x = self.span() / 2.0 - self.terminal_length() / 2.0;
        vec![[-x, 0.0, 0.0], [x, 0.0, 0.0]]
    }

    fn values(&self) -> Vec<PackageValue> {
        let length = |name: &str, v: f64| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Length(v),
        };
        vec![
            length("D", self.span()),
            length("D1", self.body_length()),
            length("E", self.body_width()),
            length("A", self.body_height()),
            length("Lt", self.terminal_length()),
            length("bt", self.terminal_width()),
        ]
    }

    fn range_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut positive = |name: &str, v: Nm| out.extend(length_problem(name, v));
        positive("Body length", self.body_length_nm);
        positive("Body width", self.body_width_nm);
        positive("Body height", self.body_height_nm);
        positive("Overall length", self.span_nm);
        positive("Terminal length", self.terminal_length_nm);
        positive("Terminal width", self.terminal_width_nm);
        let mut rule = |ok: bool, message: &str| {
            if !ok {
                out.push(message.to_owned());
            }
        };
        let t = self.lead_thickness();
        let (bl, span, tl) = (self.body_length(), self.span(), self.terminal_length());
        rule(
            2.0 * tl < span,
            "The two terminals must not meet (2 × terminal length < overall length).",
        );
        match self.lead {
            MoldedLead::Melf => {
                rule(
                    (self.body_height() - self.body_width()).abs() <= 1e-6,
                    "A MELF body is round: its height must equal its width.",
                );
                rule(
                    self.terminal_width() >= self.body_width(),
                    "A MELF cap must be at least as wide as the body (terminal width ≥ body width).",
                );
                rule(
                    bl <= span + 1e-9 && bl >= span - 2.0 * tl - 1e-9,
                    "A MELF body must reach both caps and end within the overall length (overall − 2 × terminal length ≤ body length ≤ overall).",
                );
            }
            MoldedLead::Flat | MoldedLead::Bent | MoldedLead::JLead => {
                rule(
                    self.terminal_width() <= self.body_width() + 1e-9,
                    "The terminals must not be wider than the body.",
                );
                rule(
                    span > bl,
                    "The terminals must reach past the body ends (overall length > body length).",
                );
            }
        }
        let exit = self.lead_exit();
        match self.lead {
            MoldedLead::Melf | MoldedLead::Flat => {}
            MoldedLead::Bent => {
                let room = (span - bl) / 2.0 - tl;
                rule(
                    tl <= (span - bl) / 2.0,
                    "A bent lead's foot must lie outside the body (terminal length ≤ (overall − body length) / 2).",
                );
                rule(
                    room > t,
                    "A bent lead needs more than its thickness outside the body to bend down ((overall − body length) / 2 − terminal length > lead thickness).",
                );
                rule(tl > t, "The foot must be longer than the lead thickness.");
                rule(
                    exit - t / 2.0 > t,
                    "The body must be tall enough for the lead to leave it above the seating plane.",
                );
            }
            MoldedLead::JLead => {
                rule(
                    (span - bl) / 2.0 > 1.5 * t,
                    "A J-lead needs room beside the body end ((overall − body length) / 2 > 1.5 × lead thickness).",
                );
                rule(
                    tl > 2.0 * t,
                    "A J-lead's foot must be longer than twice the lead thickness.",
                );
                rule(
                    exit - t / 2.0 > 1.5 * t,
                    "The body must be tall enough for the J-lead to fold down its end.",
                );
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

    /// Original fixtures in the range of the outlines (round numbers).
    fn sod123() -> MoldedForm {
        MoldedForm {
            lead: MoldedLead::Bent,
            body_length_nm: nm(2.7),
            body_width_nm: nm(1.6),
            body_height_nm: nm(1.1),
            span_nm: nm(3.7),
            terminal_length_nm: nm(0.3),
            terminal_width_nm: nm(0.6),
            polarised: true,
        }
    }

    fn sma() -> MoldedForm {
        MoldedForm {
            lead: MoldedLead::JLead,
            body_length_nm: nm(4.3),
            body_width_nm: nm(2.6),
            body_height_nm: nm(2.2),
            span_nm: nm(5.2),
            terminal_length_nm: nm(1.1),
            terminal_width_nm: nm(1.4),
            polarised: true,
        }
    }

    fn sod523() -> MoldedForm {
        MoldedForm {
            lead: MoldedLead::Flat,
            body_length_nm: nm(1.2),
            body_width_nm: nm(0.8),
            body_height_nm: nm(0.6),
            span_nm: nm(1.6),
            terminal_length_nm: nm(0.3),
            terminal_width_nm: nm(0.3),
            polarised: true,
        }
    }

    fn minimelf() -> MoldedForm {
        MoldedForm {
            lead: MoldedLead::Melf,
            body_length_nm: nm(3.0),
            body_width_nm: nm(1.4),
            body_height_nm: nm(1.4),
            span_nm: nm(3.5),
            terminal_length_nm: nm(0.4),
            terminal_width_nm: nm(1.5),
            polarised: true,
        }
    }

    #[test]
    fn typical_forms_are_consistent() {
        for f in [sod123(), sma(), sod523(), minimelf()] {
            let form = PackageForm::Molded(Box::new(f));
            assert_eq!(form.range_problems(), Vec::<String>::new(), "{f:?}");
            assert_eq!(form.terminal_counts(), vec![2]);
        }
    }

    #[test]
    fn terminal_one_is_at_minus_x() {
        let p = PackageForm::Molded(Box::new(sma())).lead_points();
        assert_eq!(p, vec![[-2.05, 0.0, 0.0], [2.05, 0.0, 0.0]]);
        assert_eq!(
            PackageForm::Molded(Box::new(minimelf())).envelope(),
            Some([3.5, 1.5, 1.5])
        );
    }

    #[test]
    fn the_band_lies_at_terminal_one_and_the_marking_beside_it() -> crate::diagnostics::Result<()> {
        let missing = || crate::diagnostics::Error::Invalid("no area".into());
        let f = sma();
        let band = f.band_area().ok_or_else(missing)?;
        let mark = f.marking_area().ok_or_else(missing)?;
        let (bo, bs, mo, ms) = (band.origin(), band.size(), mark.origin(), mark.size());
        assert!(bo[0] < 0.0 && mo[0] > bo[0]);
        let band_hi = bo[0] + bs[0] / 2.0;
        let mark_lo = mo[0] - ms[0] / 2.0;
        // Each area is stored to the nanometre: they meet within a rounding.
        assert!((band_hi - mark_lo).abs() <= 1e-6);
        assert!(mo[0] + ms[0] / 2.0 < f.body_length() / 2.0);
        assert_eq!(bo[2], 2.2);
        let mut plain = f;
        plain.polarised = false;
        assert!(plain.band_area().is_none());
        // A MELF band sits on the glass between the caps.
        let m = minimelf();
        let band = m.band_area().ok_or_else(missing)?;
        let (bo, bs) = (band.origin(), band.size());
        assert!(bo[0] - bs[0] / 2.0 >= -(3.5 / 2.0 - 0.4));
        assert!((bo[2] - (0.05 + 1.4)).abs() < 1e-12);
        Ok(())
    }

    #[test]
    fn inconsistent_forms_are_found() {
        let problems = |f: MoldedForm| PackageForm::Molded(Box::new(f)).range_problems();
        let mut f = minimelf();
        f.body_height_nm = nm(1.0);
        assert!(problems(f).iter().any(|p| p.contains("round")));
        let mut f = minimelf();
        f.terminal_width_nm = nm(1.2);
        assert!(problems(f).iter().any(|p| p.contains("cap")));
        let mut f = sod123();
        f.terminal_length_nm = nm(0.6);
        assert!(problems(f).iter().any(|p| p.contains("outside the body")));
        let mut f = sma();
        f.span_nm = nm(4.5);
        assert!(problems(f).iter().any(|p| p.contains("room beside")));
        let mut f = sod523();
        f.terminal_length_nm = nm(0.9);
        assert!(problems(f).iter().any(|p| p.contains("meet")));
        let mut f = sod523();
        f.terminal_width_nm = nm(1.0);
        assert!(problems(f).iter().any(|p| p.contains("wider")));
        let mut f = sma();
        f.body_width_nm = nm(-1.0);
        assert!(!problems(f).is_empty());
    }
}
