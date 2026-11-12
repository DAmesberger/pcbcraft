//! U2 symbols: the `Shape` vocabulary, symbol graphics and pins, footprint
//! graphics (and paste windows, which are shapes), and the authoring grid.
//!
//! A stored shape becomes `ShapeNm`: points `{"x_nm", "y_nm"}`, `radius`
//! and `radii` `radius_nm` and `radii_nm`, and an arc's centre, radius,
//! start angle and sweep its rounded start and end points around its
//! centre with a direction (a whole turn ends where it starts). The shape
//! is converted by the app's own crossing, `ShapeNm::from_mm_rounded`, from
//! the old JSON read as the computing `geometry::Shape` (which keeps the
//! old form), so the rounding is the app's exactly. Item widths, text
//! anchors and heights, pins (position, length, name and number offsets),
//! a symbol's pin text height and the drafts' grid become nanometres.
use super::json::{items, length, object, point};
use crate::Result;
use pcbcraft_core::geometry::Shape;
use pcbcraft_core::library::ShapeNm;
use serde_json::{Map, Value};

/// A stored shape in the old millimetre form converted in place; 1 if it
/// changed. A shape already in nanometres does not read as the old form
/// and is left alone.
pub fn shape(value: &mut Value) -> Result<usize> {
    let Ok(old) = serde_json::from_value::<Shape>(value.clone()) else {
        return Ok(0);
    };
    let stored = ShapeNm::from_mm_rounded(&old).map_err(|e| format!("shape {value}: {e}"))?;
    *value = serde_json::to_value(stored)?;
    Ok(1)
}

/// The shape at `key`.
fn shape_at(object: &mut Map<String, Value>, key: &str) -> Result<usize> {
    match object.get_mut(key) {
        Some(value) => shape(value),
        None => Ok(0),
    }
}

fn text(text: &mut Map<String, Value>) -> Result<usize> {
    Ok(point(text, "at")? + length(text, "height", "height_nm", false)?)
}

/// A drawn item: `{"shape": {shape, width, fill}}` or `{"text": {…}}`.
fn item(item: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    if let Some(drawn) = object(item, "shape") {
        n += shape_at(drawn, "shape")? + length(drawn, "width", "width_nm", false)?;
    }
    if let Some(t) = object(item, "text") {
        n += text(t)?;
    }
    Ok(n)
}

/// Every graphic's item of the array at `key`.
fn graphics(body: &mut Map<String, Value>, key: &str) -> Result<usize> {
    let mut n = 0;
    for graphic in items(body, key) {
        if let Some(i) = object(graphic, "item") {
            n += item(i)?;
        }
    }
    Ok(n)
}

fn pin(pin: &mut Map<String, Value>) -> Result<usize> {
    let mut n = point(pin, "position")? + length(pin, "length", "length_nm", false)?;
    for label in ["name", "number"] {
        if let Some(t) = object(pin, label) {
            n += length(t, "offset", "offset_nm", false)?;
        }
    }
    Ok(n)
}

/// One definition body of `kind`; the number of values changed.
pub fn definition(kind: &str, body: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    match kind {
        "symbol" => {
            n += length(body, "pin_text_height", "pin_text_height_nm", false)?;
            for unit in items(body, "units") {
                n += graphics(unit, "graphics")?;
                for p in items(unit, "pins") {
                    n += pin(p)?;
                }
            }
        }
        "footprint" => {
            n += graphics(body, "graphics")?;
            for pad in items(body, "pads") {
                if let Some(paste) = object(pad, "paste") {
                    for window in items(paste, "windows") {
                        n += shape_at(window, "shape")?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(n)
}

/// Settings saved with a project's drafts: the authoring grid.
pub fn drafts(drafts: &mut Map<String, Value>) -> Result<usize> {
    let Some(grid) = object(drafts, "grid") else {
        return Ok(0);
    };
    Ok(length(grid, "symbol_mm", "symbol_nm", false)?
        + length(grid, "footprint_mm", "footprint_nm", false)?)
}

/// Data of this domain a project holds outside its library: none.
pub fn project(_project: &mut Map<String, Value>) -> Result<usize> {
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::library::graphics::Grid;
    use pcbcraft_core::library::{FootprintDefinition, SymbolDefinition};
    use serde_json::json;

    fn convert(kind: &str, mut body: Value) -> Result<(usize, Value)> {
        let o = body.as_object_mut().ok_or("object")?;
        let n = definition(kind, o)?;
        Ok((n, body))
    }

    #[test]
    fn symbols_convert_once_and_read_back() -> Result<()> {
        let old = json!({
            "component": 7,
            "next_local": 9,
            "pin_text_height": 1.25,
            "units": [{
                "id": 1,
                "label": "A",
                "graphics": [
                    {"id": 2, "item": {"shape": {
                        "shape": {"rect": {"a": [-2.5, -1.0], "b": [2.5, 1.0], "radius": 0.25}},
                        "width": 0.25, "fill": "body"}}},
                    {"id": 3, "item": {"shape": {
                        "shape": {"arc": {"center": [0.0, 0.0], "radius": 1.0,
                                          "start_deg": 45.0, "sweep_deg": -90.0}},
                        "width": 0.1, "fill": "none"}}},
                    {"id": 4, "item": {"shape": {
                        "shape": {"polyline": {"points": [[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]],
                                               "radii": [0.0, 0.5, 0.0]}},
                        "width": 0.25, "fill": "none"}}},
                    {"id": 5, "item": {"text": {
                        "content": "designator", "at": [0.0, 1.5], "height": 1.25,
                        "rotation": "r0", "justify": {"h": "center", "v": "bottom"},
                        "visible": true, "line_spacing": 1.6}}}
                ],
                "pins": [{
                    "terminal": 1, "position": [-5.0, 0.0], "direction": "right",
                    "length": 2.5, "decorations": {"inverted": false, "clock": false},
                    "name": {"visible": false, "offset": 0.5},
                    "number": {"visible": true, "offset": 0.5}
                }]
            }]
        });
        let (n, new) = convert("symbol", old)?;
        assert_eq!(n, 13);
        let unit = &new["units"][0];
        assert_eq!(new["pin_text_height_nm"], 1_250_000);
        assert_eq!(
            unit["graphics"][0]["item"]["shape"]["shape"],
            json!({"rect": {
                "a": {"x_nm": -2_500_000, "y_nm": -1_000_000},
                "b": {"x_nm": 2_500_000, "y_nm": 1_000_000},
                "radius_nm": 250_000
            }})
        );
        // cos 45° = 0.70710678…: the ends round to whole nanometres.
        assert_eq!(
            unit["graphics"][1]["item"]["shape"]["shape"],
            json!({"arc": {
                "start": {"x_nm": 707_107, "y_nm": 707_107},
                "end": {"x_nm": 707_107, "y_nm": -707_107},
                "centre": {"x_nm": 0, "y_nm": 0},
                "direction": "clockwise"
            }})
        );
        assert_eq!(
            unit["graphics"][2]["item"]["shape"]["shape"]["polyline"]["radii_nm"],
            json!([0, 500_000, 0])
        );
        assert_eq!(unit["graphics"][3]["item"]["text"]["height_nm"], 1_250_000);
        assert_eq!(unit["pins"][0]["length_nm"], 2_500_000);
        assert_eq!(unit["pins"][0]["number"]["offset_nm"], 500_000);
        serde_json::from_value::<SymbolDefinition>(new.clone())?;
        // A second run changes nothing.
        let (again, same) = convert("symbol", new.clone())?;
        assert_eq!((again, same), (0, new));
        Ok(())
    }

    #[test]
    fn footprints_convert_graphics_and_paste_windows() -> Result<()> {
        let mut body = serde_json::to_value(FootprintDefinition::default())?;
        body["graphics"] = json!([{
            "id": 3, "layer": "silkscreen", "side": "component",
            "item": {"shape": {
                "shape": {"circle": {"center": [0.1, -0.2], "radius": 0.2}},
                "width": 0.0, "fill": "solid"}}
        }, {
            "id": 4, "layer": "courtyard", "side": "component",
            "item": {"shape": {
                "shape": {"arc": {"center": [0.0, 0.0], "radius": 0.5,
                                  "start_deg": 0.0, "sweep_deg": 360.0}},
                "width": 0.05, "fill": "none"}}
        }]);
        let mut pad = serde_json::to_value(pcbcraft_core::library::Pad::smd_rect_mm(
            pcbcraft_core::ids::PadId(1),
            pcbcraft_core::ids::PackageTerminalId(1),
            "1",
            [0.0, 0.0],
            [1.0, 1.0],
        )?)?;
        pad["paste"] = json!({"windows": [{"id": 1, "shape":
            {"polygon": {"points": [[0.0, 0.0], [0.25, 0.0], [0.0, 0.25]]}}}]});
        body["pads"] = json!([pad]);
        let (n, new) = convert("footprint", body)?;
        assert_eq!(n, 5);
        assert_eq!(
            new["graphics"][0]["item"]["shape"],
            json!({"shape": {"circle": {"center": {"x_nm": 100_000, "y_nm": -200_000},
                                        "radius_nm": 200_000}},
                   "width_nm": 0, "fill": "solid"})
        );
        // A whole turn ends where it starts.
        assert_eq!(
            new["graphics"][1]["item"]["shape"]["shape"]["arc"]["end"],
            json!({"x_nm": 500_000, "y_nm": 0})
        );
        assert_eq!(
            new["pads"][0]["paste"]["windows"][0]["shape"]["polygon"]["points"][1],
            json!({"x_nm": 250_000, "y_nm": 0})
        );
        serde_json::from_value::<FootprintDefinition>(new.clone())?;
        assert_eq!(convert("footprint", new)?.0, 0);
        Ok(())
    }

    #[test]
    fn the_grid_converts_once() -> Result<()> {
        let mut drafts_json =
            json!({"items": [], "grid": {"symbol_mm": 2.54, "footprint_mm": 0.05}});
        let o = drafts_json.as_object_mut().ok_or("object")?;
        assert_eq!(drafts(o)?, 2);
        assert_eq!(drafts(o)?, 0);
        let grid: Grid = serde_json::from_value(drafts_json["grid"].clone())?;
        assert_eq!(grid.symbol_nm.get(), 2_540_000);
        assert_eq!(grid.footprint_nm.get(), 50_000);
        Ok(())
    }
}
