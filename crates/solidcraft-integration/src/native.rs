//! The one place Pcbcraft calls Solidcraft's command API.
//!
//! Solidcraft commands take an ID and JSON parameters, and most have no
//! upstream parameter contract yet. These typed builders keep every command
//! name and parameter key in one module, so a Solidcraft API change breaks one
//! function here instead of string literals across the adapter. Tests run the
//! session with strict parameter checking for the commands that have
//! contracts.
use crate::material::Material;
use crate::{Error, Result};
use pcbcraft_core::Layer;
use serde_json::{Value, json};
use solidcraft_engine::Session;
use std::path::Path;

/// Body names per `appearance.assign` call.
const APPEARANCE_CHUNK: usize = 1024;

/// A Solidcraft expression: a number (millimetres where a length is expected)
/// or an expression string such as `"board_thickness"` or `"2 mm + x"`.
#[derive(Clone, Debug)]
pub struct Expr(Value);

impl From<f64> for Expr {
    fn from(value: f64) -> Self {
        Self(json!(value))
    }
}
impl From<i32> for Expr {
    fn from(value: i32) -> Self {
        Self(json!(value))
    }
}
impl From<&str> for Expr {
    fn from(value: &str) -> Self {
        Self(json!(value))
    }
}
impl From<String> for Expr {
    fn from(value: String) -> Self {
        Self(json!(value))
    }
}

pub fn mm(value: f64) -> String {
    format!("{value} mm")
}

/// Identity of a created component and its occurrence in the root assembly.
#[derive(Clone, Copy, Debug)]
pub struct Created {
    pub component: u64,
    pub occurrence: u64,
}

/// An axis-aligned box in the active component.
pub struct BoxBody<'a> {
    pub name: &'a str,
    pub corner: [Expr; 3],
    pub size: [Expr; 3],
}

/// A planar joint attaching an occurrence to the board's top or bottom face.
pub struct BoardJoint {
    pub board: u64,
    pub occurrence: u64,
    pub x: Expr,
    pub y: Expr,
    pub angle: Expr,
    pub layer: Layer,
}

/// A native Solidcraft design under construction or opened from disk.
pub struct Native {
    session: Session,
}

impl Native {
    fn with(session: Session) -> Self {
        let mut native = Self { session };
        if cfg!(test) {
            native.session.param_check = solidcraft_engine::schema::ParamCheck::Strict;
        }
        native
    }

    pub fn new(name: &str) -> Result<Self> {
        let mut native = Self::with(Session::default());
        native.call("file.new", json!({"name": name}))?;
        Ok(native)
    }

    /// Open a `.solidcraft` design read-only.
    pub fn open(path: &str) -> Result<Self> {
        validate_path(path)?;
        let mut native = Self::with(Session::default());
        native.call("doc.open", json!({"path": path, "read_only": true}))?;
        Ok(native)
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn parameter(&mut self, name: &str, expression: impl Into<String>) -> Result<()> {
        self.call(
            "parameters.add",
            json!({"name": name, "expression": expression.into()}),
        )?;
        Ok(())
    }

    pub fn change_parameters(&mut self, changes: Value) -> Result<()> {
        self.call("parameters.change", json!({"changes": changes}))?;
        Ok(())
    }

    /// Create a component under the root and make it active.
    pub fn component(&mut self, name: &str, grounded: bool) -> Result<Created> {
        self.activate_root()?;
        let reply = self.call(
            "component.create",
            json!({"name": name, "grounded": grounded}),
        )?;
        Ok(Created {
            component: id(&reply, "component")?,
            occurrence: id(&reply, "occurrence")?,
        })
    }

    pub fn activate_root(&mut self) -> Result<()> {
        self.call("component.activate", json!({"component": "root"}))?;
        Ok(())
    }

    pub fn solid_box(&mut self, body: BoxBody<'_>) -> Result<()> {
        let [length, width, height] = body.size.map(|e| e.0);
        let corner: Vec<Value> = body.corner.into_iter().map(|e| e.0).collect();
        self.call(
            "solid.box",
            json!({"name": body.name, "body_name": body.name, "corner": corner,
                "length": length, "width": width, "height": height}),
        )?;
        Ok(())
    }

    /// Top-layer parts sit on the board's top face (flipped, offset by the
    /// board thickness); bottom-layer parts hang from its bottom face.
    pub fn board_joint(&mut self, joint: BoardJoint) -> Result<()> {
        let top = joint.layer == Layer::Top;
        self.call(
            "joint.create",
            json!({"type": "planar", "name": format!("placement-{}", joint.occurrence),
                "a": {"occurrence": joint.board, "point": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]},
                "b": {"occurrence": joint.occurrence, "point": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]},
                "flip": top, "angle": 0,
                "offset": if top { "board_thickness" } else { "0 mm" },
                "values": [joint.x.0, joint.y.0, joint.angle.0]}),
        )?;
        Ok(())
    }

    /// Assign a physical look to bodies, in chunks the command accepts. The
    /// look is stored in the native file, so Solidcraft shows the same
    /// material Pcbcraft does.
    pub fn appearance(&mut self, bodies: &[&str], material: &Material) -> Result<()> {
        let [r, g, b] = material.srgb8();
        let color = format!("#{r:02x}{g:02x}{b:02x}");
        for chunk in bodies.chunks(APPEARANCE_CHUNK) {
            self.call(
                "appearance.assign",
                json!({"bodies": chunk, "name": format!("Pcbcraft {}", material.name),
                    "color": color, "opacity": 1.0,
                    "pbr": {"metallic": material.metallic, "roughness": material.roughness,
                        "dielectric_ior": material.ior}}),
            )?;
        }
        Ok(())
    }

    pub fn set_metadata(&mut self, key: &str, value: Value) -> Result<()> {
        self.call("document.metadata.set", json!({"values": {key: value}}))?;
        Ok(())
    }

    /// The first failed feature, if evaluation did not succeed.
    pub fn evaluation_error(&self) -> Option<String> {
        self.session
            .model
            .results
            .iter()
            .find(|f| f.error.is_some())
            .map(|f| format!("{}: {:?}", f.name, f.error))
    }

    /// Save a copy as a native container. Saving is an external effect, never
    /// an edit of this design, and refuses to overwrite an existing file.
    pub fn save_new(&self, path: &str) -> Result<()> {
        validate_path(path)?;
        if Path::new(path).try_exists()? {
            return Err(Error::Invalid(
                "output exists; choose a new .solidcraft filename".into(),
            ));
        }
        let mut copy = Self::with(Session::new((*self.session.doc).clone()));
        copy.call(
            "file.save_as",
            json!({"path": path, "format": "container", "cache": false, "preview": false}),
        )?;
        Ok(())
    }

    /// Escape hatch for commands without a typed builder yet (test fixtures).
    pub(crate) fn call(&mut self, command: &str, params: Value) -> Result<Value> {
        self.session
            .execute(command, &params)
            .map_err(|error| Error::Engine {
                command: command.into(),
                message: error.to_string(),
            })
    }
}

pub fn validate_path(path: &str) -> Result<()> {
    if path.len() > 4095
        || Path::new(path)
            .extension()
            .is_none_or(|ext| ext != "solidcraft")
    {
        return Err(Error::Invalid(
            "use a .solidcraft path shorter than 4096 bytes".into(),
        ));
    }
    Ok(())
}

pub(crate) fn id(reply: &Value, key: &str) -> Result<u64> {
    reply
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Verification(format!("missing {key} in {reply}")))
}
