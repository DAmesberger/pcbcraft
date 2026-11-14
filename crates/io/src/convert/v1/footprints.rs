//! U2 footprints: pads, holes, pad vias, mask and paste rules, the datum
//! and the pad defaults of a footprint body, and a package row's land
//! overrides.
//!
//! A pad's `position`, a hole's `offset`, a via's `at` and the `size` of a
//! rectangle, rounded rectangle or stadium become `{"x_nm", "y_nm"}` under
//! the same name; scalar lengths become `_nm` integers (`diameter_nm`,
//! `width_nm`, `length_nm`, `max_radius_nm`, `drill_nm`, the defaults'
//! `mask_expansion_nm` and `paste_reduction_nm`); turns become whole
//! millidegrees (`rotation_mdeg`, a slot's `angle_mdeg`). A mask expansion
//! and a paste reduction are tagged `expansion_nm` and `reduction_nm`. A
//! custom pad outline stays millimetres (U3, shared with board outlines);
//! paste windows are shapes (`symbols`). Mechanical-part lands are request
//! values, never stored: nothing to convert.
use super::json::{angle, items, length, object, point};
use crate::convert::Result;
use serde_json::{Map, Value};

/// One definition body of `kind`; the number of values changed.
pub fn definition(kind: &str, body: &mut Map<String, Value>) -> Result<usize> {
    match kind {
        "footprint" => footprint(body),
        "package_row" => match object(body, "lands") {
            Some(lands) => land_values(lands),
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

fn footprint(body: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    if let Some(datum) = object(body, "datum") {
        n += point(datum, "offset")? + angle(datum, "rotation_deg", "rotation_mdeg", false)?;
    }
    if let Some(defaults) = object(body, "defaults") {
        n += length(defaults, "mask_expansion_mm", "mask_expansion_nm", false)?
            + length(defaults, "paste_reduction_mm", "paste_reduction_nm", false)?;
    }
    for p in items(body, "pads") {
        n += pad(p)?;
    }
    Ok(n + solve_hash(body)?)
}

/// A sourced footprint's stored solve hash, recomputed over its converted
/// content (the `symbols` conversion of its graphics runs first). The
/// hash covers the serialized pads, derived graphics and datum, so a
/// format change alone would leave it stale (`footprint.stale` at
/// publish). The converted values are the exact nanometre rounding of the
/// stored ones, which the evaluator quantized to 1e-6 mm already; turns
/// rounded to whole millidegrees are folded per shape symmetry as the
/// evaluator folds them. Publish still re-solves and compares the stored
/// geometry, so a recomputed hash vouches for nothing on its own. 1 if
/// the hash or a fold changed.
fn solve_hash(body: &mut Map<String, Value>) -> Result<usize> {
    if body.get("source").is_none_or(Value::is_null) {
        return Ok(0);
    }
    let mut footprint: sparksmith_core::library::FootprintDefinition =
        serde_json::from_value(Value::Object(body.clone()))
            .map_err(|e| format!("a converted footprint does not read: {e}"))?;
    let before = footprint.pads.clone();
    for pad in &mut footprint.pads {
        pad.fold_rotation()?;
    }
    let hash = footprint.stored_result_hash()?.unwrap_or_default();
    let Some(source) = footprint.source.as_mut() else {
        return Ok(0);
    };
    if footprint.pads == before && source.solve.result_hash == hash {
        return Ok(0);
    }
    source.solve.result_hash = hash;
    body.insert("pads".into(), serde_json::to_value(&footprint.pads)?);
    body.insert("source".into(), serde_json::to_value(&footprint.source)?);
    Ok(1)
}

fn pad(pad: &mut Map<String, Value>) -> Result<usize> {
    let mut n = point(pad, "position")? + angle(pad, "rotation_deg", "rotation_mdeg", false)?;
    if let Some(shape) = object(pad, "shape") {
        n += pad_shape(shape)?;
    }
    if let Some(hole) = object(pad, "hole") {
        n += point(hole, "offset")?;
        if let Some(shape) = object(hole, "shape") {
            if let Some(round) = object(shape, "round") {
                n += length(round, "diameter", "diameter_nm", false)?;
            }
            if let Some(slot) = object(shape, "slot") {
                n += length(slot, "width", "width_nm", false)?
                    + length(slot, "length", "length_nm", false)?
                    + angle(slot, "angle_deg", "angle_mdeg", false)?;
            }
        }
    }
    // Newtype variants: `{"expansion": 0.05}` to `{"expansion_nm": 50000}`.
    if let Some(mask) = object(pad, "mask") {
        n += length(mask, "expansion", "expansion_nm", false)?;
    }
    if let Some(paste) = object(pad, "paste") {
        n += length(paste, "reduction", "reduction_nm", false)?;
    }
    for via in items(pad, "vias") {
        n += point(via, "at")? + length(via, "drill", "drill_nm", false)?;
    }
    Ok(n)
}

/// A pad shape, externally tagged; a custom outline is left as it is.
fn pad_shape(shape: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    for tag in ["rect", "round_rect", "oblong"] {
        if let Some(sized) = object(shape, tag) {
            n += point(sized, "size")?;
        }
    }
    if let Some(rounded) = object(shape, "round_rect") {
        n += length(rounded, "max_radius", "max_radius_nm", false)?;
    }
    if let Some(circle) = object(shape, "circle") {
        n += length(circle, "diameter", "diameter_nm", false)?;
    }
    Ok(n)
}

/// A package row's land overrides.
fn land_values(lands: &mut Map<String, Value>) -> Result<usize> {
    let mut n = point(lands, "exposed_pad")?;
    for key in ["pad_length", "pad_width", "gap"] {
        n += length(lands, key, &format!("{key}_nm"), false)?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sparksmith_core::library::FootprintDefinition;
    use sparksmith_core::library::templates::catalog::LandValues;
    use sparksmith_core::library::{HoleShape, MaskRule, PadShape, PasteRule};
    use sparksmith_core::units::{Angle, Nm, PointNm};

    fn p(x: i64, y: i64) -> Result<PointNm> {
        Ok(PointNm::new(Nm::new(x)?, Nm::new(y)?))
    }

    /// An old-format footprint body: a rotated rectangle, a rounded
    /// rectangle with a cap, a plated circle with a slot and two vias, a
    /// stadium and a custom outline, every mask and paste rule.
    fn old_footprint() -> Value {
        let pad = |id: u32, shape: Value, extra: Value| {
            let mut pad = json!({
                "id": id, "kind": "smd", "terminal": null, "number": id.to_string(),
                "position": [1.25, -0.5], "rotation_deg": 90.0, "side": "top",
                "shape": shape, "hole": null, "mask": "inherit", "paste": "inherit",
                "vias": []
            });
            if let (Some(pad), Some(extra)) = (pad.as_object_mut(), extra.as_object()) {
                for (k, v) in extra {
                    pad.insert(k.clone(), v.clone());
                }
            }
            pad
        };
        json!({
            "package": {"id": 2, "revision": 1},
            "next_local": 9,
            "source": null,
            "pads": [
                pad(1, json!({"rect": {"size": [1.55, 0.6]}}),
                    json!({"mask": {"expansion": 0.075}, "paste": {"reduction": 0.025}})),
                pad(2, json!({"round_rect": {"size": [1.0, 0.5], "radius_ratio": 0.25,
                    "max_radius": 0.1, "corners": {"top_left": true, "top_right": true,
                    "bottom_right": false, "bottom_left": false}}}),
                    json!({"mask": "tented", "paste": "none", "rotation_deg": -90.0})),
                pad(3, json!({"circle": {"diameter": 1.7}}), json!({
                    "kind": "plated_through",
                    "hole": {"shape": {"slot": {"width": 0.6, "length": 1.2, "angle_deg": 90.0}},
                             "plated": true, "offset": [0.1, 0.0]},
                    "vias": [{"id": 1, "at": [0.3, 0.0], "drill": 0.2},
                             {"id": 2, "at": [-0.3, 0.0], "drill": 0.2}]})),
                pad(4, json!({"oblong": {"size": [2.0, 1.0]}}), json!({
                    "kind": "non_plated_hole",
                    "hole": {"shape": {"round": {"diameter": 0.65}}, "plated": false,
                             "offset": [0.0, 0.0]},
                    "mask": {"expansion": 0.0}})),
                pad(5, json!({"custom": {"outline": {"start": [0.0, 0.0], "segments": [
                    {"line": {"to": [1.0, 0.0]}}, {"arc": {"to": [0.0, 0.0], "bulge": 1.0}}]}}}),
                    json!({"rotation_deg": 0.0})),
                pad(6, json!({"round_rect": {"size": [1.0, 1.0], "radius_ratio": 0.5,
                    "max_radius": null, "corners": {"top_left": true, "top_right": true,
                    "bottom_right": true, "bottom_left": true}}}), json!({})),
            ],
            "graphics": [],
            "polarity": null,
            "datum": {"offset": [0.5, -1.0], "rotation_deg": 270.0},
            "defaults": {"mask_expansion_mm": 0.05, "paste_reduction_mm": 0.0}
        })
    }

    #[test]
    fn converts_a_footprint_once() -> Result<()> {
        let mut body = old_footprint();
        let o = body.as_object_mut().ok_or("object")?;
        // datum 2, defaults 2; pads: 1 → 2 + size + mask + paste = 5;
        // 2 → 2 + size + cap = 4; 3 → 2 + diameter + offset + slot 3 +
        // vias 4 = 11; 4 → 2 + size + offset + diameter + mask = 6;
        // 5 → 2; 6 → 2 + size + cap = 4.
        assert_eq!(definition("footprint", o)?, 4 + 5 + 4 + 11 + 6 + 2 + 4);
        assert_eq!(
            definition("footprint", o)?,
            0,
            "a second run changes nothing"
        );
        let fp: FootprintDefinition = serde_json::from_value(body.clone())?;
        assert_eq!(fp.datum.offset, p(500_000, -1_000_000)?);
        assert_eq!(fp.datum.rotation_mdeg, Angle::new(270_000)?);
        assert_eq!(fp.defaults.mask_expansion_nm, Nm::new(50_000)?);
        let one = &fp.pads[0];
        assert_eq!(one.position, p(1_250_000, -500_000)?);
        assert_eq!(one.rotation_mdeg, Angle::new(90_000)?);
        assert_eq!(
            one.shape,
            PadShape::Rect {
                size: p(1_550_000, 600_000)?
            }
        );
        assert_eq!(one.mask, MaskRule::Expansion(Nm::new(75_000)?));
        assert_eq!(one.paste, PasteRule::Reduction(Nm::new(25_000)?));
        assert_eq!(fp.pads[1].rotation_mdeg, Angle::new(270_000)?);
        assert!(matches!(
            fp.pads[1].shape,
            PadShape::RoundRect { max_radius_nm: Some(r), .. } if r == Nm::new(100_000)?
        ));
        let hole = fp.pads[2].hole.ok_or("hole")?;
        assert_eq!(hole.offset, p(100_000, 0)?);
        assert_eq!(
            hole.shape,
            HoleShape::Slot {
                width_nm: Nm::new(600_000)?,
                length_nm: Nm::new(1_200_000)?,
                angle_mdeg: Angle::new(90_000)?
            }
        );
        assert_eq!(fp.pads[2].vias[1].at, p(-300_000, 0)?);
        assert_eq!(fp.pads[2].vias[1].drill_nm, Nm::new(200_000)?);
        assert_eq!(
            fp.pads[3].hole.map(|h| h.shape),
            Some(HoleShape::Round {
                diameter_nm: Nm::new(650_000)?
            })
        );
        assert_eq!(fp.pads[3].mask, MaskRule::Expansion(Nm::ZERO));
        assert!(
            matches!(&fp.pads[4].shape, PadShape::Custom { outline } if outline.start == [0.0, 0.0])
        );
        assert!(matches!(
            fp.pads[5].shape,
            PadShape::RoundRect {
                max_radius_nm: None,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn converts_package_row_lands() -> Result<()> {
        let mut body = json!({
            "values": {"family": "generic"},
            "lands": {"pad_length": 0.8, "pad_width": null, "gap": 1.65,
                      "exposed_pad": [5.3, 5.3]}
        });
        let o = body.as_object_mut().ok_or("object")?;
        assert_eq!(definition("package_row", o)?, 4);
        assert_eq!(definition("package_row", o)?, 0);
        let lands = body.get("lands").cloned().ok_or("lands")?;
        let lands: LandValues = serde_json::from_value(lands)?;
        assert_eq!(lands.pad_length_nm, Some(Nm::new(800_000)?));
        assert_eq!(lands.pad_width_nm, None);
        assert_eq!(lands.gap_nm, Some(Nm::new(1_650_000)?));
        assert_eq!(lands.exposed_pad, Some(p(5_300_000, 5_300_000)?));
        Ok(())
    }
}
