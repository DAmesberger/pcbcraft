//! Slice B2a: the tabbed gull-wing package family: SOT-23-3/5/6, SOT-223,
//! TO-252/263 (DPAK, D²PAK). Gull-wing leads on the two long sides with
//! their own counts and lead spacing, and an optional tab on the +y side:
//! either a wide gull-wing tab lead (SOT-223) or a flat flange under the
//! body (DPAK). The footprint template is `templates/footprint_family/
//! tabbed.rs` and the model builder `pcbcraft-solidcraft` `models/tabbed.rs`.
//!
//! Lead order (module docs of `package_form`): the −y row from −x to +x,
//! then the +y row from +x to −x, then the tab. All leads sit on one grid of
//! `pitch` centred on the body; `lead_step` says how many grid positions lie
//! between neighbouring leads of a row (SOT-23-3's −y row and DPAK's leads
//! skip the middle position: step 2).
use super::{FORM_LENGTH_LIMIT_MM, FORM_LENGTH_MIN_MM, FamilyForm, centred};
use crate::geometry::Point3;
use crate::library::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use crate::library::model3d::MarkingArea;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Largest lead step (grid positions between neighbouring leads).
pub const MAX_LEAD_STEP: u32 = 16;

/// Share of the top face a marking may use along each axis (est.: clear of
/// the rounded edges).
const MARKING_SHARE: f64 = 0.8;

/// A tab on the +y side (SOT-223, DPAK), electrically a terminal.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    /// Width along x.
    pub width: f64,
    /// Length on the seating plane along y: the tab lead's foot, or the
    /// flange's solderable length.
    pub length: f64,
    /// `None`: a gull-wing tab lead whose tip reaches the lead span like
    /// the other leads (SOT-223). `Some(p)`: a flat flange on the seating
    /// plane under the body, its outer edge `p` beyond the body's +y edge
    /// (DPAK, D²PAK).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flange: Option<f64>,
}

fn one_step() -> [u32; 2] {
    [1, 1]
}

fn is_one_step(step: &[u32; 2]) -> bool {
    *step == one_step()
}

/// Gull-wing leads with unequal counts on the two long sides and an
/// optional tab.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TabbedForm {
    /// Leads on the −y and +y sides.
    pub leads_per_side: [u32; 2],
    /// The lead grid pitch.
    pub pitch: f64,
    pub body_length: f64,
    pub body_width: f64,
    pub body_height: f64,
    pub standoff: f64,
    /// Tip-to-tip lead span across y (the −y tips at −span / 2).
    pub span: f64,
    pub lead_width: f64,
    pub lead_thickness: f64,
    pub foot_length: f64,
    pub tab: Option<Tab>,
    /// Grid positions between neighbouring leads of the −y and +y rows.
    #[serde(default = "one_step", skip_serializing_if = "is_one_step")]
    pub lead_step: [u32; 2],
}

impl TabbedForm {
    /// Original typical values (a SOT-23-3 in round numbers, from no
    /// drawing): the family's custom entry in the template gallery.
    pub fn typical() -> Self {
        Self {
            leads_per_side: [2, 1],
            pitch: 0.95,
            body_length: 2.9,
            body_width: 1.3,
            body_height: 1.0,
            standoff: 0.05,
            span: 2.4,
            lead_width: 0.4,
            lead_thickness: 0.12,
            foot_length: 0.4,
            tab: None,
            lead_step: [2, 1],
        }
    }

    /// Share of the body height above the standoff at which the leads
    /// leave the moulding (as `GullWingForm::LEAD_EXIT_SHARE`).
    pub const LEAD_EXIT_SHARE: f64 = 0.5;

    /// Height of the lead centre line where it leaves the body.
    pub fn lead_exit(&self) -> f64 {
        self.standoff + Self::LEAD_EXIT_SHARE * self.body_height
    }

    /// Room outside the body for a lead to bend down to its foot.
    pub fn bend_room(&self) -> f64 {
        self.span / 2.0 - self.foot_length - self.body_width / 2.0
    }

    /// Room outside the body for a gull-wing tab lead to bend down.
    pub fn tab_bend_room(&self) -> Option<f64> {
        let tab = self.tab?;
        tab.flange
            .is_none()
            .then(|| self.span / 2.0 - tab.length - self.body_width / 2.0)
    }

    /// The +y distance of the tab's outer edge from the centre.
    pub fn tab_edge(&self) -> Option<f64> {
        let tab = self.tab?;
        Some(match tab.flange {
            Some(p) => self.body_width / 2.0 + p,
            None => self.span / 2.0,
        })
    }

    /// Where the tab touches the seating plane (centre of its contact).
    pub fn tab_point(&self) -> Option<Point3> {
        let tab = self.tab?;
        Some([0.0, self.tab_edge()? - tab.length / 2.0, 0.0])
    }

    /// The x of each lead of row `row` (0: −y, 1: +y), from −x to +x.
    pub fn row_x(&self, row: usize) -> Vec<f64> {
        let n = self.leads_per_side.get(row).copied().unwrap_or(0);
        let step = f64::from(self.lead_step.get(row).copied().unwrap_or(1));
        (0..n.min(super::MAX_FORM_LEADS))
            .map(|i| centred(i, n, self.pitch) * step)
            .collect()
    }

    /// Length a row of `n` leads at `step` takes along x.
    fn row_length(&self, row: usize) -> f64 {
        let n = self.leads_per_side.get(row).copied().unwrap_or(0);
        let step = f64::from(self.lead_step.get(row).copied().unwrap_or(1));
        f64::from(n.saturating_sub(1)) * step * self.pitch + self.lead_width
    }

    /// The top face area markings use, origin at its centre, u along +x.
    pub fn marking_area(&self) -> MarkingArea {
        MarkingArea {
            origin: [0.0, 0.0, self.standoff + self.body_height],
            normal: [0.0, 0.0, 1.0],
            u_axis: [1.0, 0.0, 0.0],
            size_mm: [
                MARKING_SHARE * self.body_length,
                MARKING_SHARE * self.body_width,
            ],
        }
    }

    /// Estimated land sizes [along x, along y] per lead in lead order
    /// (toe 0.4 mm, heel 0.2 mm, side 0.1 mm; estimated prototype values,
    /// not a compliance claim), the tab last.
    pub fn land_sizes(&self) -> Vec<[f64; 2]> {
        let lead = [self.lead_width + 0.2, self.foot_length + 0.6];
        let mut out = vec![lead; usize::try_from(self.lead_count()).unwrap_or(0)];
        if let Some(tab) = self.tab {
            out.pop();
            let grow = if tab.flange.is_some() { 0.4 } else { 0.6 };
            out.push([tab.width + 0.2, tab.length + grow]);
        }
        out
    }
}

impl FamilyForm for TabbedForm {
    fn label(&self) -> &'static str {
        "Tabbed gull-wing"
    }

    fn lengths(&self) -> Vec<f64> {
        let mut v = vec![
            self.pitch,
            self.body_length,
            self.body_width,
            self.body_height,
            self.standoff,
            self.span,
            self.lead_width,
            self.lead_thickness,
            self.foot_length,
        ];
        if let Some(tab) = self.tab {
            v.extend([tab.width, tab.length]);
            v.extend(tab.flange);
        }
        v
    }

    fn envelope(&self) -> Option<[f64; 3]> {
        let half_y = (self.span / 2.0)
            .max(self.body_width / 2.0)
            .max(self.tab_edge().unwrap_or(0.0));
        Some([
            self.body_length,
            2.0 * half_y,
            self.standoff + self.body_height,
        ])
    }

    fn lead_count(&self) -> u64 {
        let [n, m] = self.leads_per_side;
        u64::from(n) + u64::from(m) + u64::from(self.tab.is_some())
    }

    fn lead_points(&self) -> Vec<Point3> {
        if self.lead_count() > u64::from(super::MAX_FORM_LEADS) {
            return Vec::new();
        }
        let reach = self.span / 2.0 - self.foot_length / 2.0;
        let mut out: Vec<Point3> = self
            .row_x(0)
            .into_iter()
            .map(|x| [x, -reach, 0.0])
            .collect();
        out.extend(self.row_x(1).into_iter().rev().map(|x| [x, reach, 0.0]));
        out.extend(self.tab_point());
        out
    }

    fn values(&self) -> Vec<PackageValue> {
        let length = |name: &str, v: f64| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Length(v),
        };
        let count = |name: &str, v: u32| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Count(v),
        };
        let mut v = vec![
            length("e", self.pitch),
            count("n1", self.leads_per_side[0]),
            count("n2", self.leads_per_side[1]),
            count("k1", self.lead_step[0]),
            count("k2", self.lead_step[1]),
            length("D", self.body_length),
            length("E1", self.body_width),
            length("E", self.span),
            length("A1", self.standoff),
            length("A2", self.body_height),
            length("b", self.lead_width),
            length("c", self.lead_thickness),
            length("Lf", self.foot_length),
        ];
        if let (Some(tab), Some([_, y, _])) = (self.tab, self.tab_point()) {
            v.push(length("b2", tab.width));
            v.push(length("L2", tab.length));
            // The tab contact's centre lies on +y; a centred contact is 0.
            v.push(length("yt", y.max(0.0)));
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
        positive("Pitch", self.pitch);
        positive("Body length", self.body_length);
        positive("Body width", self.body_width);
        positive("Body height", self.body_height);
        positive("Lead span", self.span);
        positive("Lead width", self.lead_width);
        positive("Lead thickness", self.lead_thickness);
        positive("Foot length", self.foot_length);
        if let Some(tab) = self.tab {
            positive("Tab width", tab.width);
            positive("Tab length", tab.length);
        }
        let mut rule = |ok: bool, message: &str| {
            if !ok {
                out.push(message.to_owned());
            }
        };
        let [n, m] = self.leads_per_side;
        rule(n >= 1, "The −y side needs at least one lead.");
        rule(
            m >= 1 || self.tab.is_some(),
            "The +y side needs a lead or a tab.",
        );
        rule(
            self.lead_step
                .iter()
                .all(|k| (1..=MAX_LEAD_STEP).contains(k)),
            "A lead step must be 1–16 grid positions.",
        );
        for (row, (count, side)) in [(n, "−y"), (m, "+y")].into_iter().enumerate() {
            let step = f64::from(self.lead_step[row]);
            if count >= 2 {
                rule(
                    step * self.pitch > self.lead_width,
                    &format!("The {side} leads must not touch (step × pitch > lead width)."),
                );
            }
            if count >= 1 {
                rule(
                    self.row_length(row) <= self.body_length + 1e-9,
                    &format!("The {side} leads must fit along the body length."),
                );
            }
        }
        rule(
            self.standoff.is_finite() && self.standoff >= 0.0,
            "The standoff must not be negative.",
        );
        rule(
            self.span > self.body_width,
            "The lead span must reach past the body width.",
        );
        rule(
            self.foot_length <= (self.span - self.body_width) / 2.0,
            "The foot must lie outside the body (foot length ≤ (span − body width) / 2).",
        );
        let t = self.lead_thickness;
        rule(
            self.lead_exit() - t / 2.0 > t && t < self.body_height,
            "Leads must fit the body height: thinner than the body, and leaving it (at mid-height) more than 1.5 × lead thickness above the seating plane.",
        );
        rule(
            self.bend_room() > t,
            "The span must leave more than a lead thickness outside the body for the lead to bend down to its foot ((span − body width) / 2 − foot length > lead thickness).",
        );
        if let Some(tab) = self.tab {
            rule(
                m == 0,
                "A tab takes the whole +y side: the +y side has no other leads (0).",
            );
            rule(
                tab.width <= self.body_length + 1e-9,
                "The tab must fit along the body length.",
            );
            match tab.flange {
                None => rule(
                    self.tab_bend_room().is_some_and(|room| room > t),
                    "The tab lead's foot must leave more than a lead thickness outside the body to bend ((span − body width) / 2 − tab length > lead thickness).",
                ),
                Some(p) => {
                    rule(
                        p.is_finite() && (0.0..=FORM_LENGTH_LIMIT_MM).contains(&p),
                        "The flange protrusion must be 0 mm or more.",
                    );
                    rule(
                        tab.length <= self.body_width + p,
                        "The flange must end under the body (tab length ≤ body width + protrusion).",
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

    /// Original fixtures: round numbers in the range of the outlines, from
    /// no library or drawing.
    fn sot23_3() -> TabbedForm {
        TabbedForm {
            leads_per_side: [2, 1],
            pitch: 0.95,
            body_length: 2.9,
            body_width: 1.3,
            body_height: 1.0,
            standoff: 0.05,
            span: 2.4,
            lead_width: 0.4,
            lead_thickness: 0.12,
            foot_length: 0.4,
            tab: None,
            lead_step: [2, 1],
        }
    }

    fn sot223() -> TabbedForm {
        TabbedForm {
            leads_per_side: [3, 0],
            pitch: 2.3,
            body_length: 6.5,
            body_width: 3.5,
            body_height: 1.6,
            standoff: 0.05,
            span: 7.0,
            lead_width: 0.7,
            lead_thickness: 0.25,
            foot_length: 0.9,
            tab: Some(Tab {
                width: 3.0,
                length: 0.9,
                flange: None,
            }),
            lead_step: [1, 1],
        }
    }

    fn dpak() -> TabbedForm {
        TabbedForm {
            leads_per_side: [2, 0],
            pitch: 2.3,
            body_length: 6.5,
            body_width: 6.0,
            body_height: 2.3,
            standoff: 0.0,
            span: 11.0,
            lead_width: 0.8,
            lead_thickness: 0.5,
            foot_length: 1.5,
            tab: Some(Tab {
                width: 5.3,
                length: 5.5,
                flange: Some(1.0),
            }),
            lead_step: [2, 1],
        }
    }

    #[test]
    fn typical_forms_are_consistent() {
        for f in [sot23_3(), sot223(), dpak()] {
            let form = PackageForm::Tabbed(Box::new(f));
            assert_eq!(form.range_problems(), Vec::<String>::new(), "{f:?}");
            assert!(form.within_bounds());
        }
    }

    #[test]
    fn sot23_3_leads_sit_on_the_grid_and_run_counter_clockwise() {
        let form = PackageForm::Tabbed(Box::new(sot23_3()));
        assert_eq!(form.lead_count(), 3);
        assert_eq!(form.terminal_counts(), vec![3]);
        let p = form.lead_points();
        let y = 1.2 - 0.2;
        assert_eq!(p.len(), 3);
        assert!((p[0][0] + 0.95).abs() < 1e-12 && (p[0][1] + y).abs() < 1e-12);
        assert!((p[1][0] - 0.95).abs() < 1e-12 && (p[1][1] + y).abs() < 1e-12);
        assert!(p[2][0].abs() < 1e-12 && (p[2][1] - y).abs() < 1e-12);
        assert_eq!(form.envelope(), Some([2.9, 2.4, 1.05]));
    }

    #[test]
    fn sot23_5_top_row_skips_the_middle() {
        let mut f = sot23_3();
        f.leads_per_side = [3, 2];
        f.lead_step = [1, 2];
        let p = PackageForm::Tabbed(Box::new(f)).lead_points();
        let x: Vec<f64> = p.iter().map(|p| (p[0] * 100.0).round() / 100.0).collect();
        assert_eq!(x, [-0.95, 0.0, 0.95, 0.95, -0.95]);
    }

    #[test]
    fn the_tab_comes_last_at_its_contact() {
        let p = PackageForm::Tabbed(Box::new(sot223())).lead_points();
        assert_eq!(p.len(), 4);
        assert_eq!(p[3], [0.0, 3.5 - 0.45, 0.0]);
        let d = dpak();
        let p = PackageForm::Tabbed(Box::new(d)).lead_points();
        assert_eq!(p.len(), 3);
        // Outer flange edge 3 + 1 = 4 mm, contact centred 2.75 inside it.
        assert!((p[2][1] - 1.25).abs() < 1e-12);
        assert!((p[0][0] + 2.3).abs() < 1e-12 && (p[1][0] - 2.3).abs() < 1e-12);
        let env = d.envelope().unwrap_or_default();
        assert!((env[1] - 11.0).abs() < 1e-12);
    }

    #[test]
    fn values_name_the_tab() {
        let names: Vec<String> = PackageForm::Tabbed(Box::new(sot223()))
            .values()
            .into_iter()
            .map(|v| v.name.0)
            .collect();
        for n in ["pkg_e", "pkg_n1", "pkg_E", "pkg_b2", "pkg_L2", "pkg_yt"] {
            assert!(names.iter().any(|x| x == n), "{n}: {names:?}");
        }
        assert!(names.iter().all(|n| n.starts_with(PACKAGE_PREFIX)));
    }

    #[test]
    fn inconsistent_forms_are_found() {
        let problems = |f: TabbedForm| PackageForm::Tabbed(Box::new(f)).range_problems();
        let mut f = sot23_3();
        f.lead_step = [1, 1];
        f.pitch = 0.3;
        assert!(problems(f).iter().any(|p| p.contains("touch")));
        let mut f = sot223();
        f.leads_per_side = [3, 1];
        assert!(problems(f).iter().any(|p| p.contains("whole +y side")));
        let mut f = sot23_3();
        f.leads_per_side = [2, 0];
        assert!(problems(f).iter().any(|p| p.contains("lead or a tab")));
        let mut f = sot223();
        if let Some(t) = &mut f.tab {
            t.length = 1.6;
        }
        assert!(problems(f).iter().any(|p| p.contains("tab lead")));
        let mut f = dpak();
        if let Some(t) = &mut f.tab {
            t.length = 8.0;
        }
        assert!(problems(f).iter().any(|p| p.contains("flange")));
        let mut f = sot23_3();
        f.lead_step = [0, 1];
        assert!(problems(f).iter().any(|p| p.contains("step")));
        let mut f = sot23_3();
        f.span = f64::NAN;
        assert!(!problems(f).is_empty());
    }

    #[test]
    fn json_without_new_fields_reads_with_defaults() -> Result<(), serde_json::Error> {
        let f: TabbedForm = serde_json::from_str(
            r#"{"leads_per_side":[3,3],"pitch":0.95,"body_length":2.9,"body_width":1.6,
            "body_height":1.1,"standoff":0.05,"span":2.8,"lead_width":0.4,
            "lead_thickness":0.12,"foot_length":0.45,"tab":null}"#,
        )?;
        assert_eq!(f.lead_step, [1, 1]);
        let text = serde_json::to_string(&f)?;
        assert!(!text.contains("lead_step"));
        Ok(())
    }
}
