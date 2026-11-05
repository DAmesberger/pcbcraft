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
    changed |= land_rows(ui, form);
    changed
}

/// The datasheet's land pattern over the estimates: off, the form's estimate
/// shows; on, the rows start from that estimate. The first lead's land is
/// every lead's, and the tab's land comes last.
fn land_rows(ui: &mut Ui, form: &mut TabbedForm) -> bool {
    let sizes = form.land_sizes();
    let lead = sizes.first().copied().unwrap_or([1.0, 1.0]);
    let estimated_span = form.span - form.foot_length + 0.2;
    let mut changed = false;
    let mut datasheet = form.lands.size.is_some() || form.lands.span.is_some();
    if kit::check_row(ui, "Datasheet land pattern", &mut datasheet) {
        form.lands.size = datasheet.then_some(lead);
        form.lands.span = datasheet.then_some(estimated_span);
        changed = true;
    }
    if let Some(size) = &mut form.lands.size {
        changed |= length(ui, "Land width", &mut size[0]);
        changed |= length(ui, "Land length", &mut size[1]);
    }
    if let Some(span) = &mut form.lands.span {
        changed |= length(ui, "Land row span", span);
    }
    if form.tab.is_some() {
        let tab = sizes.last().copied().unwrap_or([1.0, 1.0]);
        let mut own = form.lands.tab_size.is_some();
        if kit::check_row(ui, "Datasheet tab land", &mut own) {
            form.lands.tab_size = own.then_some(tab);
            changed = true;
        }
        if let Some(size) = &mut form.lands.tab_size {
            changed |= length(ui, "Tab land width", &mut size[0]);
            changed |= length(ui, "Tab land length", &mut size[1]);
        }
    }
    if datasheet {
        kit::hint(
            ui,
            "Lands rebuild when the template applies",
            "These values set the pads the next Create or Apply builds",
        );
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
            lands: Default::default(),
        }
    }

    /// The labels the rows draw in one frame of `form`.
    fn drawn(form: &mut TabbedForm) -> Vec<String> {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let mut out = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            land_rows(ui, form);
        });
        output.textures_delta.clear();
        fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        out
    }

    #[test]
    fn the_land_rows_show_only_what_the_form_overrides() {
        let mut f = sot23_5();
        let shown = drawn(&mut f);
        assert!(
            shown.iter().any(|t| t == "Datasheet land pattern"),
            "{shown:?}"
        );
        assert!(!shown.iter().any(|t| t == "Land width"), "{shown:?}");
        assert!(
            !shown.iter().any(|t| t == "Datasheet tab land"),
            "{shown:?}"
        );
        f.lands.size = Some([0.6, 1.1]);
        f.lands.span = Some(2.6);
        let shown = drawn(&mut f);
        for row in ["Land width", "Land length", "Land row span"] {
            assert!(shown.iter().any(|t| t == row), "{row}: {shown:?}");
        }
        set_tab(&mut f, TabKind::Lead);
        let shown = drawn(&mut f);
        assert!(shown.iter().any(|t| t == "Datasheet tab land"), "{shown:?}");
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
