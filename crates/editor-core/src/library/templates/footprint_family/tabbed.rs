//! Slice B2a: the tabbed gull-wing (SOT, DPAK) footprint template. The
//! parent dispatches here, so the slice never edits `footprint_family.rs`.
//!
//! Pads follow the form's lead order: the −y row from −x to +x, the +y row
//! from +x to −x, then the tab land. Each pad is placed on its own by
//! distances from the origin bound to named parameters (`col<d>` is `d / 2`
//! pitches, `pad_y` half the row span, `tab_y` the tab land's centre), so
//! editing `pitch`, `row_span` or a land size re-solves the footprint; the
//! lead counts and steps are fixed when the template builds. Default values
//! are estimated prototype lands (toe 0.4 mm, heel 0.2 mm, side 0.1 mm), not
//! copied tables or a compliance claim.
use super::{Beyond, Builder, Pin1, e, locate, mm, origin, param, pin1_dot, smd};
use crate::diagnostics::{Error, Result};
use crate::library::PackageDefinition;
use crate::library::footprint_source::{Expr, PadTemplate, ParamUnit};
use crate::library::package_form::{PackageForm, tabbed::MAX_LEAD_STEP};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Whether the picker offers this template.
pub const AVAILABLE: bool = true;

/// Most leads one tabbed template places (each is its own pad).
pub const MAX_TABBED_LEADS: u32 = 64;

fn sot23_leads() -> [u32; 2] {
    [2, 1]
}

fn one_step() -> [u32; 2] {
    [1, 1]
}

/// The tabbed gull-wing (SOT, DPAK) land pattern's driving values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TabbedTemplate {
    /// Lead grid pitch (`pitch`).
    pub pitch: Expr,
    /// Row centre to centre (`row_span`).
    pub row_span: Expr,
    /// Land length (`pad_l`).
    pub pad_l: Expr,
    /// Land width (`pad_w`).
    pub pad_w: Expr,
    /// Tab land length (`tab_l`).
    pub tab_l: Expr,
    /// Tab land width (`tab_w`).
    pub tab_w: Expr,
    /// Courtyard excess around pads and envelope (`courtyard_excess`).
    pub courtyard_excess: Expr,
    /// Lands on the −y and +y rows.
    #[serde(default = "sot23_leads")]
    pub leads_per_side: [u32; 2],
    /// Grid positions between neighbouring lands of each row.
    #[serde(default = "one_step")]
    pub lead_step: [u32; 2],
    /// Add the tab land (the last pad) on +y.
    #[serde(default)]
    pub tab: bool,
    /// Distance of the tab land's centre from the origin, +y (`tab_y`).
    #[serde(default)]
    pub tab_y: Expr,
}

impl TabbedTemplate {
    /// The template form prefilled from the package: expressions over the
    /// tabbed form's `pkg_*` values when the package has one, else an
    /// estimated SOT-23-3 in literals.
    pub fn defaults(package: &PackageDefinition) -> Self {
        let excess = e(super::TEMPLATE_COURTYARD_EXCESS);
        if let PackageForm::Tabbed(f) = &package.form {
            let flange = f.tab.is_some_and(|t| t.flange.is_some());
            return Self {
                pitch: e("pkg_e"),
                row_span: e("pkg_E - pkg_Lf + 0.2"),
                pad_l: e("pkg_Lf + 0.6"),
                pad_w: e("pkg_b + 0.2"),
                tab_l: if flange {
                    e("pkg_L2 + 0.4")
                } else {
                    e("pkg_L2 + 0.6")
                },
                tab_w: e("pkg_b2 + 0.2"),
                courtyard_excess: excess,
                leads_per_side: f.leads_per_side,
                lead_step: f.lead_step,
                tab: f.tab.is_some(),
                tab_y: if flange {
                    e("pkg_yt")
                } else {
                    e("pkg_yt + 0.1")
                },
            };
        }
        Self {
            pitch: e("0.95"),
            row_span: e("2.2"),
            pad_l: e("1.0"),
            pad_w: e("0.6"),
            tab_l: e("1.5"),
            tab_w: e("3.2"),
            courtyard_excess: excess,
            leads_per_side: sot23_leads(),
            lead_step: [2, 1],
            tab: false,
            tab_y: e("3.0"),
        }
    }

    /// Every driving expression, for the syntax check.
    pub(super) fn exprs(&self) -> Vec<&Expr> {
        let mut out = vec![
            &self.pitch,
            &self.row_span,
            &self.pad_l,
            &self.pad_w,
            &self.courtyard_excess,
        ];
        if self.tab {
            out.extend([&self.tab_l, &self.tab_w, &self.tab_y]);
        }
        out
    }

    /// The counts' own rules, before any edit.
    fn check(&self) -> Result<()> {
        let invalid = |what: &str| {
            Err(Error::Invalid(format!(
                "footprint.template_invalid: {what}"
            )))
        };
        let [n, m] = self.leads_per_side;
        if n == 0 {
            return invalid("the −y row needs at least one land");
        }
        if m == 0 && !self.tab {
            return invalid("the +y side needs a land or the tab");
        }
        if n.saturating_add(m) > MAX_TABBED_LEADS {
            return Err(Error::Limit(format!(
                "footprint.template_invalid: a tabbed template places at most {MAX_TABBED_LEADS} leads"
            )));
        }
        if !self
            .lead_step
            .iter()
            .all(|k| (1..=MAX_LEAD_STEP).contains(k))
        {
            return invalid("a lead step is 1–16 grid positions");
        }
        Ok(())
    }

    /// Each land in pad order: its row (0: −y, 1: +y) and its doubled x
    /// offset in pitches (`d`: the land sits `d / 2` pitches from the
    /// centre line).
    fn lands(&self) -> Vec<(usize, i64)> {
        let mut out = Vec::new();
        for (row, reverse) in [(0usize, false), (1, true)] {
            let n = i64::from(self.leads_per_side[row]);
            let k = i64::from(self.lead_step[row]);
            let mut ds: Vec<i64> = (0..n).map(|i| (2 * i - (n - 1)) * k).collect();
            if reverse {
                ds.reverse();
            }
            out.extend(ds.into_iter().map(|d| (row, d)));
        }
        out
    }
}

fn column(d: i64) -> String {
    format!("col{}", d.unsigned_abs())
}

/// Build the template's pads: one land per lead and the tab land.
pub(super) fn build(b: &mut Builder<'_>, t: &TabbedTemplate) -> Result<Option<Pin1>> {
    t.check()?;
    let lands = t.lands();
    let mut first = vec![
        param("pitch", t.pitch.clone(), ParamUnit::Mm),
        param("row_span", t.row_span.clone(), ParamUnit::Mm),
        param("pad_l", t.pad_l.clone(), ParamUnit::Mm),
        param("pad_w", t.pad_w.clone(), ParamUnit::Mm),
        mm("pad_y", "row_span / 2"),
    ];
    let columns: BTreeSet<u64> = lands
        .iter()
        .filter(|(_, d)| *d != 0)
        .map(|(_, d)| d.unsigned_abs())
        .collect();
    for d in &columns {
        first.push(mm(&format!("col{d}"), &format!("{d} / 2 * pitch")));
    }
    if t.tab {
        first.extend([
            param("tab_l", t.tab_l.clone(), ParamUnit::Mm),
            param("tab_w", t.tab_w.clone(), ParamUnit::Mm),
            param("tab_y", t.tab_y.clone(), ParamUnit::Mm),
        ]);
    }
    b.step(first)?;
    // Guesses put each pad on its side of the origin (distances are
    // unsigned); the dimensions then solve the real positions.
    let mut placed = Vec::new();
    let mut edits = Vec::new();
    for (k, (row, d)) in lands.iter().enumerate() {
        let y = if *row == 0 { -1.5 } else { 1.5 };
        // At most 64 lands and steps of 16: |d| ≤ 1008, exact in f64.
        let guess = [*d as f64 * 0.5, y];
        let number = (k + 1).to_string();
        let (pad, anchor) = b.pad(
            PadTemplate::Rect,
            guess,
            smd("pad_w", "pad_l", None, &number),
        )?;
        let col = (*d != 0).then(|| column(*d));
        edits.extend(locate(origin(), &anchor, col.as_deref(), Some("pad_y")));
        placed.push((pad, anchor, guess));
    }
    if t.tab {
        let number = (lands.len() + 1).to_string();
        let (_, anchor) = b.pad(
            PadTemplate::Rect,
            [0.0, 3.0],
            smd("tab_w", "tab_l", None, &number),
        )?;
        edits.extend(locate(origin(), &anchor, None, Some("tab_y")));
    }
    b.step(edits)?;
    let (pad, anchor, guess) = placed
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("footprint.template_internal: no pin 1".into()))?;
    pin1_dot(b, pad, &anchor, guess, Beyond::Below, "pad_l / 2").map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lands_follow_the_form_s_lead_order() {
        let mut t = TabbedTemplate::defaults(&PackageDefinition::default());
        assert_eq!(t.lands(), vec![(0, -2), (0, 2), (1, 0)]);
        t.leads_per_side = [3, 2];
        t.lead_step = [1, 2];
        assert_eq!(t.lands(), vec![(0, -2), (0, 0), (0, 2), (1, 2), (1, -2)]);
    }

    #[test]
    fn bad_counts_are_refused_before_any_edit() {
        let mut t = TabbedTemplate::defaults(&PackageDefinition::default());
        t.leads_per_side = [2, 0];
        assert!(t.check().is_err());
        t.tab = true;
        assert!(t.check().is_ok());
        t.lead_step = [0, 1];
        assert!(t.check().is_err());
        t.lead_step = [1, 1];
        t.leads_per_side = [60, 10];
        assert!(matches!(t.check(), Err(Error::Limit(_))));
    }

    #[test]
    fn exprs_cover_the_tab_only_when_present() {
        let mut t = TabbedTemplate::defaults(&PackageDefinition::default());
        assert_eq!(t.exprs().len(), 5);
        t.tab = true;
        assert_eq!(t.exprs().len(), 8);
    }
}
