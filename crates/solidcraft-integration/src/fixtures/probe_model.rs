use super::{call, id};
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use solidcraft_engine::Session;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Parameters {
    pub board_width: f64,
    pub board_depth: f64,
    pub board_thickness: f64,
    pub hole_x: f64,
    pub hole_y: f64,
    pub hole_radius: f64,
    pub package_length: f64,
    pub package_width: f64,
    pub package_height: f64,
    pub top_x: f64,
    pub top_y: f64,
    pub top_angle: f64,
    pub bottom_x: f64,
    pub bottom_y: f64,
    pub bottom_angle: f64,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            board_width: 40.0,
            board_depth: 30.0,
            board_thickness: 1.6,
            hole_x: 4.0,
            hole_y: 5.0,
            hole_radius: 1.5,
            package_length: 4.0,
            package_width: 2.0,
            package_height: 1.2,
            top_x: 12.0,
            top_y: 10.0,
            top_angle: 0.0,
            bottom_x: 26.0,
            bottom_y: 18.0,
            bottom_angle: 30.0,
        }
    }
}

impl Parameters {
    pub(crate) fn values(&self) -> BTreeMap<&'static str, f64> {
        BTreeMap::from([
            ("board_width", self.board_width),
            ("board_depth", self.board_depth),
            ("board_thickness", self.board_thickness),
            ("hole_x", self.hole_x),
            ("hole_y", self.hole_y),
            ("hole_radius", self.hole_radius),
            ("package_length", self.package_length),
            ("package_width", self.package_width),
            ("package_height", self.package_height),
            ("top_x", self.top_x),
            ("top_y", self.top_y),
            ("top_angle", self.top_angle),
            ("bottom_x", self.bottom_x),
            ("bottom_y", self.bottom_y),
            ("bottom_angle", self.bottom_angle),
        ])
    }

    pub(crate) fn expressions(&self) -> BTreeMap<&'static str, String> {
        self.values()
            .into_iter()
            .map(|(name, value)| {
                let unit = if name.ends_with("angle") { "deg" } else { "mm" };
                (name, format!("{value} {unit}"))
            })
            .collect()
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for (name, value) in self.values() {
            if !value.is_finite() || value.abs() > 1000.0 {
                return Err(Error::Invalid(format!(
                    "{name} must be finite and within ±1000"
                )));
            }
        }
        for (name, value) in [
            ("board_width", self.board_width),
            ("board_depth", self.board_depth),
            ("board_thickness", self.board_thickness),
            ("hole_radius", self.hole_radius),
            ("package_length", self.package_length),
            ("package_width", self.package_width),
            ("package_height", self.package_height),
        ] {
            if value < 0.1 {
                return Err(Error::Invalid(format!(
                    "{name} must be at least 0.1 mm in this probe"
                )));
            }
        }
        if self.hole_x - self.hole_radius <= 0.0
            || self.hole_y - self.hole_radius <= 0.0
            || self.hole_x + self.hole_radius >= self.board_width
            || self.hole_y + self.hole_radius >= self.board_depth
        {
            return Err(Error::Invalid(
                "hole must lie strictly inside the board".into(),
            ));
        }
        Ok(())
    }
}

pub(crate) fn build(parameters: &Parameters) -> Result<Session> {
    parameters.validate()?;
    let mut s = Session::default();
    call(
        &mut s,
        "file.new",
        json!({"name": "Pcbcraft native model probe"}),
    )?;
    for (name, expression) in parameters.expressions() {
        call(
            &mut s,
            "parameters.add",
            json!({"name": name, "expression": expression}),
        )?;
    }
    let board = call(
        &mut s,
        "component.create",
        json!({"name": "Board", "grounded": true}),
    )?;
    let board_occurrence = id(&board, "occurrence")?;
    call(
        &mut s,
        "solid.box",
        json!({
            "length": "board_width", "width": "board_depth", "height": "board_thickness",
            "body_name": "Substrate", "name": "Board substrate"
        }),
    )?;
    call(
        &mut s,
        "solid.cylinder",
        json!({
            "radius": "hole_radius", "height": "board_thickness",
            "base": ["hole_x", "hole_y", 0], "operation": "cut", "targets": ["Substrate"],
            "name": "Mounting hole"
        }),
    )?;
    call(
        &mut s,
        "appearance.assign",
        json!({"bodies": ["Substrate"], "color": "#226b42"}),
    )?;
    call(&mut s, "component.activate", json!({"component": "root"}))?;
    let device = call(
        &mut s,
        "component.create",
        json!({"name": "Device", "grounded": false}),
    )?;
    let component = id(&device, "component")?;
    let top = id(&device, "occurrence")?;
    // Asymmetric body and a deliberate datum offset expose axis/flip errors.
    call(
        &mut s,
        "solid.box",
        json!({
            "length": "package_length", "width": "package_width", "height": "package_height",
            "corner": [0.25, -0.5, 0.3], "body_name": "Package", "name": "Package envelope"
        }),
    )?;
    call(
        &mut s,
        "appearance.assign",
        json!({"bodies": ["Package"], "color": "#454c59"}),
    )?;
    call(&mut s, "component.activate", json!({"component": "root"}))?;
    let copy = call(
        &mut s,
        "occurrence.copy",
        json!({"component": component, "parent": "root"}),
    )?;
    let bottom = id(&copy, "occurrence")?;
    for (occurrence, prefix, flip, offset) in [
        (top, "top", true, "board_thickness"),
        (bottom, "bottom", false, "0 mm"),
    ] {
        call(
            &mut s,
            "joint.create",
            json!({
                "type": "planar", "name": format!("{prefix}_placement"),
                "a": {"occurrence": board_occurrence, "point": [0,0,0], "z": [0,0,1], "x": [1,0,0]},
                "b": {"occurrence": occurrence, "point": [0,0,0], "z": [0,0,1], "x": [1,0,0]},
                "flip": flip, "angle": 0, "offset": offset,
                "values": [format!("{prefix}_x"), format!("{prefix}_y"), format!("{prefix}_angle")]
            }),
        )?;
    }
    super::measure::verify(&s, parameters)?;
    Ok(s)
}
