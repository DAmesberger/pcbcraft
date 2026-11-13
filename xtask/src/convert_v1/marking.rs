//! U2 marking: marking specs (a component's `marking`, a Part's
//! `top_mark`, an instance's replacing `marking.replace`) and their
//! placement on model surfaces.
//!
//! Item sizes and the minimum text height become `_nm` (`height_nm`, a
//! 2D code's `size_nm`, artwork's `size_nm` array); an explicit area's
//! `centre` becomes `{"x_nm", "y_nm"}` and its `size_mm` a `size_nm` array;
//! a placement's `offset_mm` becomes the point `offset`, its turn
//! `rotation_mdeg` (reduced to one turn); a wrap's seam `seam_mdeg`
//! (reduced to one turn: the same position round the cylinder), its sweep
//! `wrap_mdeg` (not reduced: 360° stays a full turn) and its height range
//! `along_nm`. Laid-out elements are replies and never stored.
use super::json::{angle, items, length, lengths, object, point, point_as, sweep};
use crate::Result;
use serde_json::{Map, Value};

/// One definition body of `kind`; the number of values changed.
pub fn definition(kind: &str, body: &mut Map<String, Value>) -> Result<usize> {
    let key = match kind {
        "component" => "marking",
        "part" => "top_mark",
        _ => return Ok(0),
    };
    match object(body, key) {
        Some(s) => spec(s),
        None => Ok(0),
    }
}

/// Settings saved with a project's drafts.
pub fn drafts(_drafts: &mut Map<String, Value>) -> Result<usize> {
    Ok(0)
}

/// Data of this domain a project holds outside its library: each
/// instance's replacing marking.
pub fn project(project: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    for instance in items(project, "instances") {
        if let Some(spec) = object(instance, "marking").and_then(|m| object(m, "replace")) {
            n += self::spec(spec)?;
        }
    }
    Ok(n)
}

/// One marking spec and its decals.
fn spec(s: &mut Map<String, Value>) -> Result<usize> {
    let mut n = length(s, "min_text_height_mm", "min_text_height_nm", false)?;
    for item in items(s, "items") {
        let tag = item.get("item").and_then(Value::as_str).unwrap_or("");
        n += match tag {
            "text" => length(item, "height_mm", "height_nm", false)?,
            "artwork" => lengths(item, "size_mm", "size_nm")?,
            "code2d" => length(item, "size_mm", "size_nm", false)?,
            _ => 0,
        };
    }
    if let Some(area) = object(s, "area") {
        n += point(area, "centre")? + lengths(area, "size_mm", "size_nm")?;
    }
    if let Some(p) = object(s, "placement") {
        n += placement(p)?;
    }
    for decal in items(s, "decals") {
        n += spec(decal)?;
    }
    Ok(n)
}

/// A placement and its wrap.
fn placement(p: &mut Map<String, Value>) -> Result<usize> {
    let mut n =
        point_as(p, "offset_mm", "offset")? + angle(p, "rotation_deg", "rotation_mdeg", false)?;
    if let Some(w) = object(p, "wrap") {
        n += angle(w, "seam_deg", "seam_mdeg", false)?
            + sweep(w, "wrap_deg", "wrap_mdeg")?
            + lengths(w, "along_mm", "along_nm")?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sparksmith_core::library::marking::{InstanceMarking, MarkingSpec};

    fn old_spec() -> Value {
        json!({
            "items": [
                {"item": "text", "template": "A", "height_mm": 1.25},
                {"item": "text", "template": "B", "height_mm": null},
                {"item": "artwork", "svg": "00".repeat(32), "size_mm": [6.0, 4.0], "licence": "CC0-1.0"},
                {"item": "code2d", "payload": "{designator}", "size_mm": 4.5},
                {"item": "polarity", "mark": "dot"}
            ],
            "area": {"area": "explicit", "centre": [0.5, -0.25], "size_mm": [3.0, 2.0]},
            "min_text_height_mm": 0.3,
            "placement": {
                "on": {"surface": "anchor", "name": "sleeve"},
                "offset_mm": [0.0, 1.5], "rotation_deg": -90.0, "scale": 1.0,
                "mirror": false, "clip": true,
                "wrap": {"seam_deg": -30.0, "wrap_deg": 360.0, "along_mm": [1.0, 4.0], "repeat": 2}
            },
            "decals": [{"items": [{"item": "text", "template": "C", "height_mm": 1.2}], "finish": "ink_white"}]
        })
    }

    #[test]
    fn a_component_marking_converts_once_and_reads_back() -> Result<()> {
        let mut body = json!({"marking": old_spec()});
        let o = body.as_object_mut().ok_or("object")?;
        assert_eq!(definition("component", o)?, 13);
        assert_eq!(
            definition("component", o)?,
            0,
            "a second run changes nothing"
        );
        let s = &body["marking"];
        assert_eq!(s["items"][0]["height_nm"], json!(1_250_000));
        assert_eq!(s["items"][1]["height_nm"], Value::Null);
        assert_eq!(s["items"][2]["size_nm"], json!([6_000_000, 4_000_000]));
        assert_eq!(s["items"][3]["size_nm"], json!(4_500_000));
        assert_eq!(
            s["area"]["centre"],
            json!({"x_nm": 500_000, "y_nm": -250_000})
        );
        assert_eq!(s["area"]["size_nm"], json!([3_000_000, 2_000_000]));
        assert_eq!(s["min_text_height_nm"], json!(300_000));
        let p = &s["placement"];
        assert_eq!(p["offset"], json!({"x_nm": 0, "y_nm": 1_500_000}));
        assert_eq!(p["rotation_mdeg"], json!(270_000));
        assert_eq!(p["wrap"]["seam_mdeg"], json!(330_000));
        assert_eq!(
            p["wrap"]["wrap_mdeg"],
            json!(360_000),
            "a full turn stays one"
        );
        assert_eq!(p["wrap"]["along_nm"], json!([1_000_000, 4_000_000]));
        assert_eq!(s["decals"][0]["items"][0]["height_nm"], json!(1_200_000));
        let spec: MarkingSpec = serde_json::from_value(s.clone())?;
        assert_eq!(spec.placement.rotation_deg(), 270.0);
        assert_eq!(spec.placement.wrap.map(|w| w.wrap_deg()), Some(360.0));
        assert!(
            sparksmith_core::library::marking::spec_findings(
                sparksmith_core::DefinitionId(1),
                &spec
            )
            .is_empty()
        );
        Ok(())
    }

    #[test]
    fn a_part_top_mark_and_an_instance_replacement_convert() -> Result<()> {
        let mut part = json!({"top_mark": old_spec()});
        assert_eq!(
            definition("part", part.as_object_mut().ok_or("object")?)?,
            13
        );
        let _: MarkingSpec = serde_json::from_value(part["top_mark"].clone())?;
        let mut project = json!({"instances": [
            {"id": 1},
            {"id": 2, "marking": {"hide": false, "replace": old_spec()}}
        ]});
        assert_eq!(project_values(&mut project)?, 13);
        assert_eq!(project_values(&mut project)?, 0);
        let _: InstanceMarking =
            serde_json::from_value(project["instances"][1]["marking"].clone())?;
        // Other kinds are not touched.
        let mut other = json!({"marking": {"items": [{"item": "text", "height_mm": 1.0}]}});
        assert_eq!(
            definition("symbol", other.as_object_mut().ok_or("object")?)?,
            0
        );
        Ok(())
    }

    fn project_values(file: &mut Value) -> Result<usize> {
        super::project(file.as_object_mut().ok_or("object")?)
    }
}
