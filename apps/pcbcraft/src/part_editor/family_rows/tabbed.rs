//! Slice B2a: the tabbed form's parameter rows in the 3D tab Body section
//! (SOT-23, SOT-223, DPAK): lead counts and spacing per side, body, leads,
//! and the tab (a gull-wing tab lead or a flat flange). Range problems show
//! as package findings in Issues, as for the other families.
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::package_form::tabbed::{MAX_LEAD_STEP, Tab, TabbedForm};
use pcbcraft_core::library::package_form::{
    FORM_LENGTH_LIMIT_MM, FORM_LENGTH_MIN_MM, MAX_FORM_LEADS,
};

fn length(ui: &mut Ui, label: &str, value: &mut f64) -> bool {
    kit::number_row(
        ui,
        label,
        value,
        "mm",
        FORM_LENGTH_MIN_MM..=FORM_LENGTH_LIMIT_MM,
    )
}

fn count(ui: &mut Ui, label: &str, value: &mut u32, min: u32, max: u32) -> bool {
    let mut v = f64::from(*value);
    if kit::number_row(ui, label, &mut v, "", f64::from(min)..=f64::from(max)) && v.fract() == 0.0 {
        // Whole and within `min..=max` by the range and the check above.
        *value = v as u32;
        return true;
    }
    false
}

/// What sits on the +y side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabKind {
    None,
    Lead,
    Flange,
}

fn tab_kind(form: &TabbedForm) -> TabKind {
    match form.tab {
        None => TabKind::None,
        Some(Tab { flange: None, .. }) => TabKind::Lead,
        Some(Tab {
            flange: Some(_), ..
        }) => TabKind::Flange,
    }
}

/// `form` with the tab changed to `kind`: a new tab starts from the body
/// (estimated), and a tab takes the whole +y side.
fn set_tab(form: &mut TabbedForm, kind: TabKind) {
    let width = (0.5 * form.body_length).max(FORM_LENGTH_MIN_MM);
    form.tab = match kind {
        TabKind::None => {
            if form.leads_per_side[1] == 0 {
                form.leads_per_side[1] = 1;
            }
            None
        }
        TabKind::Lead => Some(Tab {
            width,
            length: form.foot_length,
            flange: None,
        }),
        TabKind::Flange => Some(Tab {
            width,
            length: 0.8 * form.body_width,
            flange: Some(0.15 * form.body_width),
        }),
    };
    if kind != TabKind::None {
        form.leads_per_side[1] = 0;
    }
}

/// The rows of a tabbed form; returns whether a value changed.
pub(super) fn rows(ui: &mut Ui, form: &mut TabbedForm) -> bool {
    let mut changed = false;
    changed |= count(
        ui,
        "Leads, −y side",
        &mut form.leads_per_side[0],
        1,
        MAX_FORM_LEADS,
    );
    changed |= count(
        ui,
        "Lead step, −y",
        &mut form.lead_step[0],
        1,
        MAX_LEAD_STEP,
    );
    let mut kind = tab_kind(form);
    let options = [
        (TabKind::None, "Leads".to_owned()),
        (TabKind::Lead, "Tab lead (SOT-223)".to_owned()),
        (TabKind::Flange, "Flange (DPAK)".to_owned()),
    ];
    if kit::choice_row(ui, "+y side", &mut kind, &options) {
        set_tab(form, kind);
        changed = true;
    }
    if form.tab.is_none() {
        changed |= count(
            ui,
            "Leads, +y side",
            &mut form.leads_per_side[1],
            1,
            MAX_FORM_LEADS,
        );
        changed |= count(
            ui,
            "Lead step, +y",
            &mut form.lead_step[1],
            1,
            MAX_LEAD_STEP,
        );
    }
    changed |= length(ui, "Pitch", &mut form.pitch);
    changed |= length(ui, "Body length", &mut form.body_length);
    changed |= length(ui, "Body width", &mut form.body_width);
    changed |= length(ui, "Body height", &mut form.body_height);
    changed |= kit::number_row(
        ui,
        "Standoff",
        &mut form.standoff,
        "mm",
        0.0..=FORM_LENGTH_LIMIT_MM,
    );
    changed |= length(ui, "Span", &mut form.span);
    changed |= length(ui, "Lead width", &mut form.lead_width);
    changed |= length(ui, "Lead thickness", &mut form.lead_thickness);
    changed |= length(ui, "Foot length", &mut form.foot_length);
    if let Some(tab) = &mut form.tab {
        changed |= length(ui, "Tab width", &mut tab.width);
        changed |= length(ui, "Tab length", &mut tab.length);
        if let Some(p) = &mut tab.flange {
            changed |= kit::number_row(
                ui,
                "Flange beyond body",
                p,
                "mm",
                0.0..=FORM_LENGTH_LIMIT_MM,
            );
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sot23_5() -> TabbedForm {
        TabbedForm {
            leads_per_side: [3, 2],
            pitch: 0.95,
            body_length: 2.9,
            body_width: 1.6,
            body_height: 1.1,
            standoff: 0.05,
            span: 2.8,
            lead_width: 0.4,
            lead_thickness: 0.12,
            foot_length: 0.45,
            tab: None,
            lead_step: [1, 2],
        }
    }

    #[test]
    fn a_tab_takes_the_plus_y_side_and_giving_it_back_restores_a_lead() {
        let mut f = sot23_5();
        set_tab(&mut f, TabKind::Flange);
        assert_eq!(tab_kind(&f), TabKind::Flange);
        assert_eq!(f.leads_per_side, [3, 0]);
        set_tab(&mut f, TabKind::Lead);
        assert_eq!(tab_kind(&f), TabKind::Lead);
        set_tab(&mut f, TabKind::None);
        assert_eq!((f.tab, f.leads_per_side), (None, [3, 1]));
    }
}
