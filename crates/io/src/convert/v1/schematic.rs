//! U1 (schematic): stored f64 millimetres become integer nanometres. Each
//! `[x, y]` point becomes `{"x_nm", "y_nm"}`, and `radius`, `offset` and
//! `height` become `radius_nm`, `offset_nm` and `height_nm`.
use super::json::{items, length, point};
use crate::convert::Result;
use serde_json::{Map, Value};

/// Rewrites the schematic's stored lengths; the number of values changed.
pub fn convert_project(project: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    for occurrence in items(project, "occurrences") {
        n += point(occurrence, "position")?;
    }
    for junction in items(project, "junctions") {
        n += point(junction, "position")?;
    }
    for label in items(project, "labels") {
        n += point(label, "position")?;
    }
    for wire in items(project, "wires") {
        for end in ["from", "to"] {
            if let Some(end) = wire.get_mut(end).and_then(Value::as_object_mut) {
                n += point(end, "free")?;
            }
        }
        if let Some(corners) = wire.get_mut("corners").and_then(Value::as_array_mut) {
            for corner in corners.iter_mut().filter_map(Value::as_object_mut) {
                n += point(corner, "at")?;
                n += length(corner, "radius", "radius_nm", true)?;
            }
        }
    }
    for symbol in items(project, "sheet_symbols") {
        n += point(symbol, "position")?;
        n += point(symbol, "size")?;
        if let Some(pins) = symbol.get_mut("pins").and_then(Value::as_array_mut) {
            for pin in pins.iter_mut().filter_map(Value::as_object_mut) {
                n += length(pin, "offset", "offset_nm", false)?;
            }
        }
    }
    for drawing in items(project, "drawings") {
        let Some(item) = drawing.get_mut("item").and_then(Value::as_object_mut) else {
            continue;
        };
        match item.get("item").and_then(Value::as_str) {
            Some("text") => {
                n += point(item, "at")?;
                n += length(item, "height", "height_nm", false)?;
            }
            Some("line") => {
                n += point(item, "from")?;
                n += point(item, "to")?;
            }
            Some("rect") => {
                n += point(item, "min")?;
                n += point(item, "max")?;
            }
            other => return Err(format!("unknown drawing item {other:?}").into()),
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_schematic_lengths_once() -> Result<()> {
        let mut project = json!({
            "occurrences": [{"position": [12.7, -5.08]}],
            "junctions": [{"position": [0.1, 0.2]}],
            "labels": [{"position": [1.0, 2.0]}],
            "wires": [{
                "from": {"free": [3.3, 4.4]},
                "to": {"junction": 7},
                "corners": [{"at": [1.5, 2.5], "radius": 0.635}, {"at": [0.0, 0.0], "radius": 0.0}]
            }],
            "sheet_symbols": [{"position": [10.0, 20.0], "size": [30.0, 15.24], "pins": [{"offset": 2.54}]}],
            "drawings": [
                {"item": {"item": "text", "at": [1.0, 1.0], "height": 2.5}},
                {"item": {"item": "line", "from": [0.0, 0.0], "to": [1.0, 0.0]}},
                {"item": {"item": "rect", "min": [0.0, 0.0], "max": [1.0, 1.0]}}
            ],
            "placements": [{"position": [1.25, 2.5]}]
        });
        let map = project.as_object_mut().ok_or("object")?;
        assert_eq!(convert_project(map)?, 17);
        assert_eq!(
            project["occurrences"][0]["position"],
            json!({"x_nm": 12_700_000, "y_nm": -5_080_000})
        );
        assert_eq!(
            project["wires"][0]["from"]["free"]["y_nm"],
            json!(4_400_000)
        );
        assert_eq!(project["wires"][0]["to"], json!({"junction": 7}));
        assert_eq!(
            project["wires"][0]["corners"][0]["radius_nm"],
            json!(635_000)
        );
        assert_eq!(
            project["wires"][0]["corners"][1],
            json!({"at": {"x_nm": 0, "y_nm": 0}})
        );
        assert_eq!(
            project["sheet_symbols"][0]["pins"][0],
            json!({"offset_nm": 2_540_000})
        );
        assert_eq!(
            project["drawings"][0]["item"]["height_nm"],
            json!(2_500_000)
        );
        // Board data is a later slice's.
        assert_eq!(project["placements"][0]["position"], json!([1.25, 2.5]));
        let map = project.as_object_mut().ok_or("object")?;
        assert_eq!(convert_project(map)?, 0, "a second run changes nothing");
        Ok(())
    }

    #[test]
    fn refuses_out_of_range_lengths() {
        let mut project = json!({"labels": [{"position": [2001.0, 0.0]}]});
        let result = project
            .as_object_mut()
            .ok_or_else(|| "object".into())
            .and_then(convert_project);
        assert!(result.is_err());
    }
}
