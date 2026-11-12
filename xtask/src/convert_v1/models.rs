//! U2 3D models: model definitions, model placement on packages and a
//! part file's derived bounds and marking surfaces.
//!
//! A model's `bounds_mm` becomes `bounds_nm` (two `[x, y, z]` integer
//! arrays), its frame's `origin_offset` `origin_offset_nm`, each lead tag's
//! `contact` `contact_nm`, the marking area's `origin` and `size_mm`
//! `origin_nm` and `size_nm`, the pitch hint `pitch_nm`. A package's
//! placement `translation` becomes `translation_nm` and its `rotation_deg`
//! `rotation_mdeg` (each angle reduced to one turn: the same rotation). A
//! part-file model's `derived.bounds_mm` becomes `bounds_nm` and its
//! surfaces' planes and cylinders convert as above (`base_nm`,
//! `radius_nm`, `length_nm`). Lead fits are replies and never stored.
use super::json::{angles, bounds, items, length, lengths, object};
use crate::Result;
use serde_json::{Map, Value};

/// One definition body of `kind`; the number of values changed.
pub fn definition(kind: &str, body: &mut Map<String, Value>) -> Result<usize> {
    match kind {
        "model" => model(body),
        "package" => package(body),
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

fn model(body: &mut Map<String, Value>) -> Result<usize> {
    let mut n = bounds(body, "bounds_mm", "bounds_nm")?;
    if let Some(frame) = object(body, "frame") {
        n += lengths(frame, "origin_offset", "origin_offset_nm")?;
    }
    for lead in items(body, "leads") {
        n += lengths(lead, "contact", "contact_nm")?;
    }
    if let Some(area) = object(body, "marking_area") {
        n += plane(area)?;
    }
    if let Some(hints) = object(body, "hints") {
        n += length(hints, "pitch_mm", "pitch_nm", false)?;
    }
    Ok(n)
}

fn package(body: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    if let Some(p) = object(body, "placement") {
        n += lengths(p, "translation", "translation_nm")?
            + angles(p, "rotation_deg", "rotation_mdeg")?;
    }
    let part_file = body
        .get("model")
        .and_then(|m| m.get("source"))
        .and_then(Value::as_str)
        == Some("part_file");
    if part_file && let Some(derived) = object(body, "model").and_then(|m| object(m, "derived")) {
        n += bounds(derived, "bounds_mm", "bounds_nm")?;
        for surface in items(derived, "surfaces") {
            if let Some(geometry) = object(surface, "geometry") {
                n += self::geometry(geometry)?;
            }
        }
    }
    Ok(n)
}

/// A marking surface, by its `map` tag.
fn geometry(g: &mut Map<String, Value>) -> Result<usize> {
    match g.get("map").and_then(Value::as_str) {
        Some("plane") => plane(g),
        Some("cylinder") => Ok(lengths(g, "base", "base_nm")?
            + length(g, "radius", "radius_nm", false)?
            + length(g, "length", "length_nm", false)?),
        _ => Ok(0),
    }
}

/// A planar marking area.
fn plane(a: &mut Map<String, Value>) -> Result<usize> {
    Ok(lengths(a, "origin", "origin_nm")? + lengths(a, "size_mm", "size_nm")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::library::model3d::ModelDefinition;
    use pcbcraft_core::library::{ModelPlacement, PackageModel};
    use serde_json::json;

    /// `value`'s object with `remove` keys dropped and `old` merged in: the
    /// app's own encoding with this domain's fields in their old form.
    fn old_form(value: Value, remove: &[&str], old: Value) -> Result<Value> {
        let (Value::Object(mut o), Value::Object(old)) = (value, old) else {
            return Err("not an object".into());
        };
        for key in remove {
            o.remove(*key);
        }
        o.extend(old);
        Ok(Value::Object(o))
    }

    #[test]
    fn a_model_definition_converts_once_and_reads_back() -> Result<()> {
        let mut body = old_form(
            serde_json::to_value(ModelDefinition::default())?,
            &["bounds_nm", "frame", "leads", "marking_area", "hints"],
            json!({
                "frame": {"unit_mm": 1.0, "up": "z", "origin_offset": [0.0, 0.5, -0.1]},
                "bounds_mm": [[-1.5, -1.0, 0.0], [1.5, 1.0, 1.2]],
                "leads": [{"key": "lead-1", "designation": "1", "contact": [-1.25, 0.0, 0.0]}],
                "marking_area": {"origin": [0.0, 0.0, 1.2], "normal": [0.0, 0.0, 1.0],
                    "u_axis": [1.0, 0.0, 0.0], "size_mm": [2.0, 1.5]},
                "hints": {"mount": "smd", "pins": 2, "pitch_mm": 2.5}
            }),
        )?;
        let o = body.as_object_mut().ok_or("object")?;
        assert_eq!(definition("model", o)?, 6);
        assert_eq!(definition("model", o)?, 0, "a second run changes nothing");
        assert_eq!(
            body["bounds_nm"],
            json!([
                [-1_500_000, -1_000_000, 0],
                [1_500_000, 1_000_000, 1_200_000]
            ])
        );
        assert_eq!(
            body["frame"]["origin_offset_nm"],
            json!([0, 500_000, -100_000])
        );
        assert_eq!(body["leads"][0]["contact_nm"], json!([-1_250_000, 0, 0]));
        assert_eq!(body["marking_area"]["origin_nm"], json!([0, 0, 1_200_000]));
        assert_eq!(
            body["marking_area"]["size_nm"],
            json!([2_000_000, 1_500_000])
        );
        assert_eq!(body["hints"]["pitch_nm"], json!(2_500_000));
        let model: ModelDefinition = serde_json::from_value(body)?;
        assert_eq!(model.size_mm(), [3.0, 2.0, 1.2]);
        Ok(())
    }

    #[test]
    fn a_package_placement_and_part_file_convert() -> Result<()> {
        let model = old_form(
            serde_json::to_value(PackageModel::PartFile(Box::default()))?,
            &["derived"],
            json!({"derived": {"bounds_mm": [[-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]], "bodies": 2, "surfaces": [
                {"name": "top", "description": "the top", "geometry": {"map": "plane",
                    "origin": [0.0, 0.0, 2.0], "normal": [0.0, 0.0, 1.0], "u_axis": [1.0, 0.0, 0.0], "size_mm": [1.5, 1.5]}},
                {"name": "sleeve", "description": "round", "geometry": {"map": "cylinder",
                    "base": [0.0, 0.0, 0.5], "axis": [0.0, 0.0, 1.0], "zero": [1.0, 0.0, 0.0],
                    "radius": 2.5, "length": 7.0, "along_u": false}}
            ]}}),
        )?;
        let mut body = json!({
            "placement": {"translation": [0.0, 0.25, 0.05], "rotation_deg": [-90.0, 0.0, 450.0]},
            "model": model
        });
        let o = body.as_object_mut().ok_or("object")?;
        assert_eq!(definition("package", o)?, 8);
        assert_eq!(definition("package", o)?, 0);
        assert_eq!(
            body["placement"]["translation_nm"],
            json!([0, 250_000, 50_000])
        );
        assert_eq!(
            body["placement"]["rotation_mdeg"],
            json!([270_000, 0, 90_000])
        );
        let placement: ModelPlacement = serde_json::from_value(body["placement"].clone())?;
        assert_eq!(placement.rotation_deg(), [270.0, 0.0, 90.0]);
        let derived = &body["model"]["derived"];
        assert_eq!(
            derived["bounds_nm"],
            json!([
                [-1_000_000, -1_000_000, 0],
                [1_000_000, 1_000_000, 2_000_000]
            ])
        );
        let cylinder = &derived["surfaces"][1]["geometry"];
        assert_eq!(cylinder["base_nm"], json!([0, 0, 500_000]));
        assert_eq!(cylinder["radius_nm"], json!(2_500_000));
        assert_eq!(cylinder["length_nm"], json!(7_000_000));
        let model: PackageModel = serde_json::from_value(body["model"].clone())?;
        assert!(matches!(model, PackageModel::PartFile(_)));
        // A generated or envelope model has no derived data.
        let mut other = json!({"model": {"source": "envelope", "material": "epoxy_mould"}});
        assert_eq!(
            definition("package", other.as_object_mut().ok_or("object")?)?,
            0
        );
        Ok(())
    }
}
