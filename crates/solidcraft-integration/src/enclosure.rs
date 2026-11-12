//! A SolidCraft enclosure design, opened in Sparksmith's own engine: the
//! headless side of the board → enclosure link (docs/architecture/
//! board-interface.md, "Link updates"). It links a published board
//! interface as SolidCraft's Insert Design does, re-reads it as Update
//! Links does, and reports its bodies' faces and its features' failures.
//!
//! Slice B3 uses it to check that an enclosure follows a published board
//! (a wall with a cutout projected from an opening, a boss on a mounting
//! hole); slice B4 grows it into the dry run on a scratch copy.
use crate::native::{BoxBody, Native, id};
use crate::{Error, Result};
use serde_json::{Value, json};

/// One face of an evaluated body, as Solidcraft names and places it.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Its persistent name in this design (a linked face's is
    /// `F<feature>:link:<digest>`).
    pub name: String,
    /// A point on the face, which a pick at it finds.
    pub point: [f64; 3],
    pub centroid: [f64; 3],
    /// A plane face's outward normal.
    pub normal: Option<[f64; 3]>,
    pub area: f64,
}

/// What `component.update_links` did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkUpdate {
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    pub failed: Vec<String>,
    pub warnings: Vec<String>,
}

/// A feature that does not evaluate, by its name in the enclosure.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureNote {
    pub feature: String,
    pub error: String,
}

/// Whether a projected sketch adds a body or cuts the bodies it meets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    New,
    Cut,
}

/// An enclosure design.
pub struct Enclosure {
    native: Native,
}

fn vec3(value: &Value) -> Option<[f64; 3]> {
    let list = value.as_array()?;
    let mut out = [0.0; 3];
    for (slot, v) in out.iter_mut().zip(list.iter()) {
        *slot = v.as_f64()?;
    }
    (list.len() == 3).then_some(out)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.get("name").and_then(Value::as_str).map(str::to_owned))
        })
        .collect()
}

impl Enclosure {
    /// A new, empty enclosure design.
    pub fn new(name: &str) -> Result<Self> {
        Ok(Self {
            native: Native::new(name)?,
        })
    }

    /// Link the published board design at `path` as a grounded component
    /// `name`; returns the linked bodies' names.
    pub fn link(&mut self, path: &str, name: &str) -> Result<Vec<String>> {
        crate::native::validate_path(path)?;
        let reply = self.native.call(
            "component.insert_design",
            json!({"path": path, "name": name}),
        )?;
        let occurrence = id(&reply, "occurrence")?;
        self.native.call(
            "occurrence.ground",
            json!({"occurrence": occurrence, "grounded": true}),
        )?;
        Ok(strings(&reply["bodies"]))
    }

    /// Read every linked design again (SolidCraft's Update Links).
    pub fn update_links(&mut self) -> Result<LinkUpdate> {
        let reply = self.native.call("component.update_links", json!({}))?;
        Ok(LinkUpdate {
            updated: strings(&reply["updated"]),
            unchanged: strings(&reply["unchanged"]),
            failed: strings(&reply["failed"]),
            warnings: strings(&reply["warnings"]),
        })
    }

    /// The faces of `body`.
    pub fn faces(&mut self, body: &str) -> Result<Vec<Face>> {
        let reply = self.native.call("model.faces", json!({"body": body}))?;
        let mut out = Vec::new();
        for face in reply["faces"].as_array().into_iter().flatten() {
            let (Some(name), Some(point), Some(centroid)) = (
                face["name"].as_str(),
                vec3(&face["point"]),
                vec3(&face["centroid"]),
            ) else {
                return Err(Error::Verification(format!(
                    "a face without a name or place: {face}"
                )));
            };
            out.push(Face {
                name: name.to_owned(),
                point,
                centroid,
                normal: vec3(&face["plane_normal"]),
                area: face["area"].as_f64().unwrap_or(0.0),
            });
        }
        Ok(out)
    }

    /// A box body `name` from `corner`, `size` long, wide and high.
    pub fn solid_box(&mut self, name: &str, corner: [f64; 3], size: [f64; 3]) -> Result<()> {
        self.native.solid_box(BoxBody {
            name,
            corner: corner.map(Into::into),
            size: size.map(Into::into),
        })
    }

    /// A sketch `name` on the plane `[origin, x, y]` projecting the face of
    /// the linked `body` at `at` (the edges follow that face when the link
    /// is updated), extruded by `distance` along the plane's normal into a
    /// new body or cutting the bodies it meets.
    pub fn projected(
        &mut self,
        name: &str,
        plane: [[f64; 3]; 3],
        body: &str,
        at: [f64; 3],
        distance: f64,
        operation: Operation,
    ) -> Result<()> {
        let [origin, x, y] = plane;
        let reply = self.native.call(
            "sketch.create",
            json!({"plane": {"origin": origin, "x_dir": x, "y_dir": y}, "name": name}),
        )?;
        let sketch = id(&reply, "sketch")?;
        self.native.call(
            "sketch.project",
            json!({"refs": [{"face": at, "body": body}]}),
        )?;
        self.native.call("sketch.finish", json!({}))?;
        let operation = match operation {
            Operation::New => "new",
            Operation::Cut => "cut",
        };
        let mut params = json!({"sketch": sketch, "distance": distance, "operation": operation,
            "profiles": "all", "name": name});
        if operation == "new" {
            params["body_name"] = json!(name);
        }
        self.native.call("solid.extrude", params)?;
        Ok(())
    }

    /// Every feature that does not evaluate, with Solidcraft's message.
    pub fn feature_errors(&self) -> Vec<FeatureNote> {
        let session = self.native.session();
        session
            .model
            .results
            .iter()
            .filter_map(|r| {
                let error = r.error.clone()?;
                let feature = session
                    .doc
                    .features
                    .iter()
                    .find(|f| f.id == r.id)
                    .map_or_else(|| r.name.clone(), |f| f.name.clone());
                Some(FeatureNote { feature, error })
            })
            .collect()
    }
}
