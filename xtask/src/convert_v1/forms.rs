//! U2 package forms: typed family forms, envelopes and package rows.
//!
//! A form is internally tagged by `family` (snake_case). Its scalar
//! lengths become `_nm` integers; `[x, y]` points and sizes become
//! `{"x_nm", "y_nm"}` under the same name; pairs and triples that are not
//! an (x, y) point (a gull-wing's spans across y and x, a module pad's size
//! across and along its edge, a land's growth, a USB-C opening, an antenna
//! keep-out's growth, the envelope) become `_nm` integer arrays; a
//! ground-pad array's turn becomes whole millidegrees.
use super::json::{angle, items, length, lengths, object, point};
use crate::Result;
use serde_json::{Map, Value};

/// One definition body of `kind`; the number of values changed.
pub fn definition(kind: &str, body: &mut Map<String, Value>) -> Result<usize> {
    match kind {
        "package" => {
            let mut n = lengths(body, "envelope", "envelope_nm")?;
            if let Some(f) = object(body, "form") {
                n += form(f)?;
            }
            Ok(n)
        }
        "package_row" => match object(body, "values") {
            Some(f) => form(f),
            None => Ok(0),
        },
        _ => Ok(0),
    }
}

/// Settings saved with a project's drafts.
pub fn drafts(_drafts: &mut Map<String, Value>) -> Result<usize> {
    Ok(0)
}

/// Data of this domain a project holds outside its library.
pub fn project(_project: &mut Map<String, Value>) -> Result<usize> {
    Ok(0)
}

/// Scalar lengths `keys` of `o`, each to `KEY_nm`.
fn scalars(o: &mut Map<String, Value>, keys: &[&str]) -> Result<usize> {
    let mut n = 0;
    for key in keys {
        n += length(o, key, &format!("{key}_nm"), false)?;
    }
    Ok(n)
}

/// Points or sizes `keys` of `o`, in place.
fn points(o: &mut Map<String, Value>, keys: &[&str]) -> Result<usize> {
    let mut n = 0;
    for key in keys {
        n += point(o, key)?;
    }
    Ok(n)
}

/// Length arrays `keys` of `o`, each to `KEY_nm`.
fn arrays(o: &mut Map<String, Value>, keys: &[&str]) -> Result<usize> {
    let mut n = 0;
    for key in keys {
        n += lengths(o, key, &format!("{key}_nm"))?;
    }
    Ok(n)
}

/// One typed form, by its `family` tag.
pub fn form(f: &mut Map<String, Value>) -> Result<usize> {
    let family = f
        .get("family")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    const BODY: [&str; 3] = ["body_length", "body_width", "body_height"];
    let mut n = 0;
    match family.as_str() {
        "chip" => n += scalars(f, &["length", "width", "height", "termination"])?,
        "gull_wing" => {
            n += scalars(f, &["pitch", "standoff", "lead_width", "lead_thickness"])?;
            n += scalars(f, &BODY)?;
            n += scalars(f, &["foot_length"])?;
            n += arrays(f, &["span"])?;
        }
        "no_lead" => {
            n += scalars(f, &["pitch", "terminal_length", "terminal_width"])?;
            n += scalars(f, &BODY)?;
            n += points(f, &["exposed_pad"])?;
        }
        "header" => {
            n += scalars(
                f,
                &[
                    "pitch",
                    "row_pitch",
                    "body_height",
                    "pin_side",
                    "pin_above",
                    "pin_below",
                ],
            )?;
        }
        "tabbed" => {
            n += scalars(
                f,
                &[
                    "pitch",
                    "standoff",
                    "span",
                    "lead_width",
                    "lead_thickness",
                    "foot_length",
                ],
            )?;
            n += scalars(f, &BODY)?;
            if let Some(tab) = object(f, "tab") {
                n += scalars(tab, &["width", "length", "flange"])?;
            }
            if let Some(lands) = object(f, "lands") {
                n += points(lands, &["size", "tab_size"])?;
                n += scalars(lands, &["span"])?;
            }
        }
        "molded" => {
            n += scalars(f, &["span", "terminal_length", "terminal_width"])?;
            n += scalars(f, &BODY)?;
        }
        "grid" => {
            n += points(f, &["pitch"])?;
            n += scalars(f, &["contact_diameter", "contact_height"])?;
            n += scalars(f, &BODY)?;
        }
        "tht" => {
            n += scalars(
                f,
                &[
                    "lead_pitch",
                    "lead_diameter",
                    "lead_below",
                    "standoff",
                    "bend_radius",
                    "body_pitch",
                ],
            )?;
            if let Some(kind) = object(f, "kind") {
                n += scalars(kind, &["body_diameter", "row_pitch"])?;
                n += scalars(kind, &BODY)?;
            }
        }
        "module" => {
            n += scalars(
                f,
                &[
                    "width",
                    "length",
                    "height",
                    "pcb_thickness",
                    "antenna_length",
                    "pitch",
                    "castellation",
                    "land_width",
                ],
            )?;
            n += points(f, &["can_size", "can_centre"])?;
            n += arrays(f, &["pad_size", "land_extend"])?;
            for row in items(f, "rows") {
                n += points(row, &["first"])?;
            }
            if let Some(ground) = object(f, "ground") {
                n += points(ground, &["pitch", "size", "centre"])?;
                n += angle(ground, "rotation_deg", "rotation_mdeg", false)?;
            }
            for single in items(f, "singles") {
                n += points(single, &["centre", "size"])?;
            }
            if let Some(keepout) = object(f, "keepout") {
                n += arrays(keepout, &["beyond"])?;
            }
        }
        "usb_c" => {
            n += scalars(
                f,
                &[
                    "width",
                    "depth",
                    "height",
                    "opening_z",
                    "shell_thickness",
                    "pitch",
                    "land_y",
                    "land_length",
                    "signal_land_width",
                    "power_land_width",
                    "vbus_x",
                    "gnd_x",
                    "tab_x",
                    "stake_length",
                    "peg_x",
                    "peg_y",
                    "peg_diameter",
                    "peg_hole",
                ],
            )?;
            n += arrays(f, &["opening"])?;
            for tab in items(f, "tabs") {
                n += scalars(tab, &["y"])?;
                n += points(tab, &["slot", "land"])?;
            }
        }
        // `generic` has no lengths; an unknown family fails the read-back.
        _ => {}
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use pcbcraft_core::library::PackageForm;
    use serde_json::json;

    fn converted(kind: &str, mut body: Value) -> (Value, usize) {
        let n = definition(kind, body.as_object_mut().unwrap()).unwrap();
        let again = definition(kind, body.as_object_mut().unwrap()).unwrap();
        assert_eq!(again, 0, "a second run changes nothing");
        (body, n)
    }

    fn form_of(old: Value) -> PackageForm {
        let (body, n) = converted("package_row", json!({ "values": old }));
        assert!(n > 0);
        serde_json::from_value(body["values"].clone()).unwrap()
    }

    #[test]
    fn a_package_converts_its_envelope_and_form() {
        let old = json!({
            "next_local": 3,
            "terminals": [],
            "form": {"family": "chip", "kind": "resistor", "length": 1.6, "width": 0.8,
                     "height": 0.45, "termination": 0.32000000000000006},
            "envelope": [1.6, 0.8, 0.45],
            "model": {"source": "envelope", "material": "epoxy_mould"},
            "placement": {"offset": [0.0, 0.0, 0.0], "rotation_deg": [0.0, 0.0, 0.0]},
        });
        let (body, n) = converted("package", old);
        assert_eq!(n, 5);
        assert_eq!(body["envelope_nm"], json!([1_600_000, 800_000, 450_000]));
        assert_eq!(body["form"]["termination_nm"], 320_000);
        // The model and placement belong to other domains; the form and
        // envelope read as the app's.
        let form: PackageForm = serde_json::from_value(body["form"].clone()).unwrap();
        assert_eq!(
            form.envelope_nm().unwrap().map(|e| e[0].get()),
            Some(1_600_000)
        );
        let envelope: [pcbcraft_core::units::Nm; 3] =
            serde_json::from_value(body["envelope_nm"].clone()).unwrap();
        assert_eq!(envelope[2].get(), 450_000);
    }

    #[test]
    fn every_family_reads_back_as_the_app_types() {
        let forms = [
            json!({"family": "gull_wing", "sides": "two", "leads_per_side": [4, 0],
                   "pitch": 1.27, "body_length": 4.9, "body_width": 3.9, "body_height": 1.45,
                   "standoff": 0.1, "span": [6.0, 0.0], "lead_width": 0.41,
                   "lead_thickness": 0.2, "foot_length": 0.6}),
            json!({"family": "no_lead", "sides": "four", "terminals_per_side": [4, 4],
                   "pitch": 0.5, "body_length": 3.0, "body_width": 3.0, "body_height": 0.85,
                   "terminal_length": 0.4, "terminal_width": 0.25, "exposed_pad": [1.7, 1.7]}),
            json!({"family": "no_lead", "sides": "two", "terminals_per_side": [4, 0],
                   "pitch": 0.5, "body_length": 2.0, "body_width": 2.0, "body_height": 0.75,
                   "terminal_length": 0.35, "terminal_width": 0.25, "exposed_pad": null}),
            json!({"family": "header", "rows": 2, "pins_per_row": 5, "pitch": 2.54,
                   "row_pitch": 2.54, "body_height": 2.5, "pin_side": 0.64, "pin_above": 6.0,
                   "pin_below": 3.0}),
            json!({"family": "tabbed", "leads_per_side": [2, 0], "pitch": 2.3,
                   "body_length": 6.5, "body_width": 6.0, "body_height": 2.3, "standoff": 0.0,
                   "span": 11.0, "lead_width": 0.8, "lead_thickness": 0.5, "foot_length": 1.5,
                   "tab": {"width": 5.3, "length": 5.5, "flange": 1.0}, "lead_step": [2, 1],
                   "lands": {"size": [1.0, 2.0], "span": 9.5, "tab_size": [5.8, 6.0]}}),
            json!({"family": "molded", "lead": "bent", "body_length": 2.7, "body_width": 1.6,
                   "body_height": 1.1, "span": 3.7, "terminal_length": 0.3,
                   "terminal_width": 0.6, "polarised": true}),
            json!({"family": "grid", "contact": "ball", "rows": 4, "cols": 5,
                   "pitch": [0.8, 0.8], "body_length": 5.0, "body_width": 4.0,
                   "body_height": 0.8, "contact_diameter": 0.4, "contact_height": 0.3,
                   "depopulated": [[0, 1]], "row_letters": "ABCD"}),
            json!({"family": "tht", "kind": {"construction": "dip", "pins": 8,
                   "row_pitch": 7.5, "body_length": 9.5, "body_width": 6.5, "body_height": 3.5},
                   "lead_pitch": 2.5, "lead_diameter": 0.5, "lead_below": 3.0,
                   "standoff": 0.5, "body_pitch": 2.0}),
            json!({"family": "tht", "kind": {"construction": "axial", "body_diameter": 2.5,
                   "body_length": 6.0}, "lead_pitch": 10.0, "lead_diameter": 0.6,
                   "lead_below": 3.0}),
            json!({"family": "module", "width": 18.0, "length": 25.0, "height": 3.2,
                   "pcb_thickness": 0.8, "antenna_length": 6.0, "can_size": [16.6, 17.4],
                   "can_centre": [0.0, -2.5], "pitch": 1.27, "pad_size": [1.2, 0.9],
                   "rows": [{"side": "left", "count": 14, "first": [-8.4, 5.5]}],
                   "castellation": 0.6, "land_width": 0.9, "land_extend": [0.3, 0.0],
                   "ground": {"rows": 3, "cols": 3, "pitch": [1.6, 1.6], "size": [1.1, 1.1],
                              "centre": [0.0, -2.0], "rotation_deg": -45.0, "number": "39"},
                   "singles": [{"number": "40", "centre": [8.0, -11.0], "size": [1.0, 1.0]}],
                   "keepout": {"beyond": [0.0, 1.5, 2.0]}}),
            json!({"family": "usb_c", "width": 8.94, "depth": 7.35, "height": 3.31,
                   "opening": [8.34, 2.56], "opening_z": 1.73, "shell_thickness": 0.3,
                   "pitch": 0.5, "land_y": 3.68, "land_length": 1.15, "signal_land_width": 0.3,
                   "power_land_width": 0.6, "vbus_x": 2.4, "gnd_x": 3.2, "tab_x": 4.32,
                   "tabs": [{"y": -1.075, "slot": [0.6, 1.4], "land": [1.0, 1.8]},
                            {"y": 3.105, "slot": [0.6, 1.7], "land": [1.0, 2.1]}],
                   "stake_length": 0.95, "peg_x": 2.89, "peg_y": 2.605, "peg_diameter": 0.5,
                   "peg_hole": 0.65}),
        ];
        for old in forms {
            let form = form_of(old.clone());
            // The wire form of the converted form is the old millimetres.
            let wire = serde_json::to_value(
                pcbcraft_core::library::package_form::PackageFormParams::from(form),
            )
            .unwrap();
            let mut expected = old.clone();
            if expected["family"] == "module" {
                // The turn reads back reduced to one turn.
                expected["ground"]["rotation_deg"] = json!(315.0);
            }
            assert_eq!(wire, expected);
        }
    }

    #[test]
    fn spot_values() {
        let PackageForm::Module(m) = form_of(json!({"family": "module", "width": 18.0,
            "length": 25.0, "height": 3.2, "pcb_thickness": 0.8, "antenna_length": 0.0,
            "can_size": [16.6, 17.4], "can_centre": [0.0, -2.5], "pitch": 1.27,
            "pad_size": [1.2, 0.9], "rows": [], "land_width": 0.9,
            "ground": {"rows": 1, "cols": 1, "pitch": [1.6, 1.6], "size": [1.1, 1.1],
                       "centre": [0.0, -2.0], "number": "1"}}))
        else {
            panic!("a module");
        };
        assert_eq!(m.can_centre.y_nm.get(), -2_500_000);
        assert_eq!(m.pad_size_nm[1].get(), 900_000);
        assert_eq!(m.pitch_nm.get(), 1_270_000);
        assert_eq!(m.ground.map(|g| g.rotation_mdeg.millidegrees()), Some(0));
    }
}
