//! Slice B2c: the grid array form's parameter rows in the 3D tab Body
//! section: contact kind, matrix, pitches, body, contacts, the row-letter
//! alphabet and the depopulated positions by ball name ("A2 E5 E6").
use crate::kit;
use egui::Ui;
use sparksmith_core::library::package_form::GridForm;
use sparksmith_core::library::package_form::grid::{GridContact, MAX_DEPOPULATED, MAX_GRID_SIDE};
use sparksmith_core::units::Nm;
/// A starting ball height for a land grid turned into a ball grid: a
/// share of the ball diameter (est.).
const BALL_HEIGHT_SHARE: f64 = 0.75;

fn length(ui: &mut Ui, label: &str, value: &mut Nm) -> bool {
    super::nm_row(ui, label, value, super::length_range())
}

fn side(ui: &mut Ui, label: &str, value: &mut u32) -> bool {
    let mut v = f64::from(*value);
    if kit::number_row(ui, label, &mut v, "", 1.0..=f64::from(MAX_GRID_SIDE)) && v.fract() == 0.0 {
        // Bounded and whole by the range and the check above.
        *value = v as u32;
        return true;
    }
    false
}

/// The depopulated positions as ball names in matrix order.
pub(super) fn names(form: &GridForm) -> String {
    let mut positions = form.depopulated.clone();
    positions.sort_unstable();
    positions
        .iter()
        .filter_map(|[r, c]| form.ball_name(*r, *c))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ball names separated by spaces or commas as (row, column) positions,
/// each once in matrix order; `Err` names the first one that is not in
/// the grid.
pub(super) fn parse(form: &GridForm, text: &str) -> Result<Vec<[u32; 2]>, String> {
    let mut out = Vec::new();
    for name in text.split([' ', ',', ';']).filter(|s| !s.is_empty()) {
        let at = form
            .position_of(name)
            .ok_or_else(|| format!("{name} is not a position of this grid"))?;
        out.push(at);
        if out.len() > MAX_DEPOPULATED {
            return Err(format!("At most {MAX_DEPOPULATED} empty positions"));
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// The rows; returns whether a value changed.
pub(super) fn rows(ui: &mut Ui, form: &mut GridForm) -> bool {
    let mut changed = false;
    let before = form.contact;
    let options = [
        (GridContact::Ball, "Balls (BGA, WLCSP)".to_owned()),
        (GridContact::Land, "Lands (LGA)".to_owned()),
    ];
    if kit::choice_row(ui, "Contacts", &mut form.contact, &options) && form.contact != before {
        form.contact_height_nm = match form.contact {
            GridContact::Ball => {
                Nm::from_mm_f64_rounded(BALL_HEIGHT_SHARE * form.contact_diameter())
                    .unwrap_or(Nm::ZERO)
            }
            GridContact::Land => Nm::ZERO,
        };
        changed = true;
    }
    changed |= side(ui, "Rows", &mut form.rows);
    changed |= side(ui, "Columns", &mut form.cols);
    changed |= length(ui, "Pitch along a row", &mut form.pitch.x_nm);
    changed |= length(ui, "Pitch between rows", &mut form.pitch.y_nm);
    changed |= length(ui, "Body length", &mut form.body_length_nm);
    changed |= length(ui, "Body width", &mut form.body_width_nm);
    changed |= length(ui, "Body height", &mut form.body_height_nm);
    let ball = form.contact == GridContact::Ball;
    changed |= length(
        ui,
        if ball {
            "Ball diameter"
        } else {
            "Land diameter"
        },
        &mut form.contact_diameter_nm,
    );
    if ball {
        changed |= length(ui, "Ball height", &mut form.contact_height_nm);
    }
    let letters = kit::checked_text_row(
        ui,
        "Row letters",
        &mut form.row_letters,
        "ABCDEFGHJKLMNPRTUVWY",
        false,
    )
    .on_hover_text("The alphabet rows are named with; JEDEC leaves out I, O, Q, S, X and Z");
    changed |= letters.changed();
    changed |= empty_positions(ui, form);
    kit::value_row(
        ui,
        "Contacts",
        &format!("{} of {}", form.populated().len(), form.positions()),
    );
    changed
}

/// The depopulated positions as editable ball names. The text lives in
/// egui memory while the field has focus; only a list of real positions
/// is stored.
fn empty_positions(ui: &mut Ui, form: &mut GridForm) -> bool {
    let id = ui.scope_id().with("grid-empty-positions");
    let stored = names(form);
    let mut text = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| stored.clone());
    let parsed = parse(form, &text);
    let response =
        kit::checked_text_row(ui, "Empty positions", &mut text, "A2 E5 …", parsed.is_err())
            .on_hover_text("Depopulated positions by ball name; the other balls keep their names");
    let mut changed = false;
    if response.changed()
        && let Ok(positions) = parse(form, &text)
        && positions != form.depopulated
    {
        form.depopulated = positions;
        changed = true;
    }
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    if let Err(message) = parse(form, &text) {
        kit::field_error(ui, &response, &message, true);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nm(mm: f64) -> Nm {
        Nm::from_mm_f64_rounded(mm).unwrap_or_default()
    }

    #[test]
    fn empty_positions_round_trip_by_ball_name() {
        let mut form =
            GridForm::ball_grid(10, 10, nm(0.5), [5.5, 5.5, 0.6].map(nm), [0.3, 0.2].map(nm));
        assert_eq!(
            parse(&form, "e5, A2  j10;a2"),
            Ok(vec![[0, 1], [4, 4], [8, 9]])
        );
        assert!(parse(&form, "I1").is_err(), "JEDEC rows skip I");
        assert!(parse(&form, "A11").is_err());
        assert_eq!(parse(&form, ""), Ok(Vec::new()));
        form.depopulated = vec![[8, 9], [0, 1]];
        assert_eq!(names(&form), "A2 J10");
    }
}
