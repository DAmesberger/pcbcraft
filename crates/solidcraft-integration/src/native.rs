//! The one place Pcbcraft calls Solidcraft's command API.
//!
//! Solidcraft commands take an ID and JSON parameters, and most have no
//! upstream parameter contract yet. These typed builders keep every command
//! name and parameter key in one module, so a Solidcraft API change breaks one
//! function here instead of string literals across the adapter. Tests run the
//! session with strict parameter checking for the commands that have
//! contracts.
use crate::material::Material;
use crate::pad_outline::{Contour, bulge_midpoint};
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

/// A body extruded from closed contours drawn on the active component's XY
/// plane, moved `offset` along its normal and extruded `height` along it.
/// Nesting is even-odd: a contour inside another is its hole.
pub struct ProfileBody<'a> {
    pub name: &'a str,
    pub offset: Expr,
    pub contours: &'a [Contour],
    pub height: Expr,
}

/// An origin plane of the active component. Sketch coordinates (u, v) lie
/// at [`Plane::sketch_point`]; the plane's offset and extrusions run along
/// its normal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    /// (u, v) → (u, v, offset); normal +z.
    Xy,
    /// (u, v) → (offset, u, v); normal +x.
    Yz,
    /// (u, v) → (u, offset, −v); normal +y.
    Xz,
}

impl Plane {
    fn name(self) -> &'static str {
        match self {
            Self::Xy => "XY",
            Self::Yz => "YZ",
            Self::Xz => "XZ",
        }
    }

    /// Where sketch point (u, v) lies, `offset` along the normal.
    pub fn sketch_point(self, [u, v]: [f64; 2], offset: f64) -> [f64; 3] {
        match self {
            Self::Xy => [u, v, offset],
            Self::Yz => [offset, u, v],
            Self::Xz => [u, offset, -v],
        }
    }
}

/// A rectangular body pattern: a direction, count and spacing, and an
/// optional second direction.
pub struct Pattern {
    pub dir1: [f64; 3],
    pub count1: Expr,
    pub spacing1: Expr,
    pub second: Option<([f64; 3], Expr, Expr)>,
}

/// An opaque physical look as a native file stores it.
pub struct Look<'a> {
    pub name: &'a str,
    pub srgb8: [u8; 3],
    pub metallic: f64,
    pub roughness: f64,
    /// Dielectric IOR in [1, 3] (ignored for metals, still required).
    pub ior: f64,
}

/// A joint origin on an occurrence: a point and unit z and x axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointFrame {
    pub point: [f64; 3],
    pub z: [f64; 3],
    pub x: [f64; 3],
}

impl JointFrame {
    pub const ORIGIN: Self = Self {
        point: [0.0; 3],
        z: [0.0, 0.0, 1.0],
        x: [1.0, 0.0, 0.0],
    };
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

    /// A unitless parameter (a count).
    pub fn count_parameter(&mut self, name: &str, value: u32) -> Result<()> {
        self.call(
            "parameters.add",
            json!({"name": name, "expression": value.to_string(), "unit": ""}),
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

    /// Extrude closed contours into a new body (never joined to the
    /// component's other bodies, so touching copper stays separate bodies).
    pub fn profile_body(&mut self, body: ProfileBody<'_>) -> Result<()> {
        let sketch = self.sketch(body.name, body.offset, body.contours)?;
        self.call(
            "solid.extrude",
            json!({"sketch": sketch, "distance": body.height.0, "operation": "new",
                "name": body.name, "body_name": body.name}),
        )?;
        Ok(())
    }

    /// Extrude closed contours drawn on an origin `plane` of the active
    /// component into a new body named `body.name`. [`Plane::sketch_point`]
    /// says where a sketch point lies.
    pub fn plane_body(&mut self, plane: Plane, body: ProfileBody<'_>) -> Result<()> {
        let sketch = self.sketch_on(body.name, plane, body.offset, body.contours)?;
        self.call(
            "solid.extrude",
            json!({"sketch": sketch, "distance": body.height.0, "operation": "new",
                "name": body.name, "body_name": body.name}),
        )?;
        Ok(())
    }

    /// Copies of `bodies` in a rectangular pattern (counts include the
    /// originals); returns the originals, then the copies it made.
    pub fn pattern_bodies(
        &mut self,
        name: &str,
        bodies: &[&str],
        pattern: Pattern,
    ) -> Result<Vec<String>> {
        let mut params = json!({"bodies": bodies, "name": name,
            "dir1": pattern.dir1, "count1": pattern.count1.0, "spacing1": pattern.spacing1.0});
        if let (Some((dir, count, spacing)), Some(map)) = (pattern.second, params.as_object_mut()) {
            map.insert("dir2".into(), json!(dir));
            map.insert("count2".into(), count.0);
            map.insert("spacing2".into(), spacing.0);
        }
        let reply = self.call("solid.pattern.rectangular", params)?;
        let created = reply
            .get("created_bodies")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Verification(format!("pattern {name} named no bodies")))?;
        Ok(bodies
            .iter()
            .map(|b| (*b).to_owned())
            .chain(created.iter().filter_map(|n| n.as_str().map(str::to_owned)))
            .collect())
    }

    /// Cut `tools` out of `target`, keeping the tools: a moulded body
    /// around the leads it holds, the two disjoint.
    pub fn cut_keeping(&mut self, name: &str, target: &str, tools: &[&str]) -> Result<()> {
        self.call(
            "solid.combine",
            json!({"target": target, "tools": tools, "operation": "cut", "keep_tools": true,
                "name": name}),
        )?;
        Ok(())
    }

    /// Cut a cylinder standing on `base` along +z out of `targets`.
    pub fn cylinder_cut(
        &mut self,
        name: &str,
        base: [Expr; 3],
        radius: Expr,
        height: Expr,
        targets: &[&str],
    ) -> Result<()> {
        let base: Vec<Value> = base.into_iter().map(|e| e.0).collect();
        self.call(
            "solid.cylinder",
            json!({"name": name, "base": base, "radius": radius.0, "height": height.0,
                "axis": "Z", "operation": "cut", "targets": targets}),
        )?;
        Ok(())
    }

    /// The component named `name` and its first occurrence.
    pub fn find_component(&self, name: &str) -> Option<Created> {
        let doc = &self.session.doc;
        let component = doc.components.iter().find(|c| c.name == name)?.id;
        let occurrence = doc.occurrence_of(component)?.id;
        Some(Created {
            component,
            occurrence,
        })
    }

    /// Place `component` once more under the root, where its first
    /// occurrence is now; returns the new occurrence. Every occurrence
    /// shares the one definition.
    pub fn copy_occurrence(&mut self, component: u64) -> Result<u64> {
        self.activate_root()?;
        let reply = self.call("occurrence.copy", json!({"component": component}))?;
        id(&reply, "occurrence")
    }

    /// Move an occurrence rigidly by a row-major 3×4 matrix (rotation then
    /// translation) and capture the position.
    pub fn move_occurrence(&mut self, occurrence: u64, matrix: [[f64; 4]; 3]) -> Result<()> {
        self.call(
            "occurrence.move",
            json!({"occurrence": occurrence, "matrix": matrix, "capture": true}),
        )?;
        Ok(())
    }

    /// Cut closed contours on the active component's XY plane (moved
    /// `offset` along its normal) `depth` along the normal out of `targets`.
    pub fn profile_cut(
        &mut self,
        name: &str,
        offset: Expr,
        contours: &[Contour],
        depth: Expr,
        targets: &[&str],
    ) -> Result<()> {
        let sketch = self.sketch(name, offset, contours)?;
        self.call(
            "solid.extrude",
            json!({"sketch": sketch, "distance": depth.0, "operation": "cut",
                "name": name, "targets": targets}),
        )?;
        Ok(())
    }

    /// A finished sketch of closed contours; returns its feature id.
    fn sketch(&mut self, name: &str, offset: Expr, contours: &[Contour]) -> Result<u64> {
        self.sketch_on(name, Plane::Xy, offset, contours)
    }

    /// A finished sketch of closed contours on one of the active
    /// component's origin planes, moved `offset` along its normal.
    fn sketch_on(
        &mut self,
        name: &str,
        plane: Plane,
        offset: Expr,
        contours: &[Contour],
    ) -> Result<u64> {
        let reply = self.call(
            "sketch.create",
            json!({"plane": plane.name(), "offset": offset.0, "name": format!("{name} sketch")}),
        )?;
        let sketch = id(&reply, "sketch")?;
        for contour in contours {
            self.contour(contour)?;
        }
        self.call("sketch.finish", json!({}))?;
        Ok(sketch)
    }

    /// Draw one closed contour in the active sketch: a circle, a closed
    /// polyline, or runs of lines and three-point arcs joined end to end.
    fn contour(&mut self, contour: &Contour) -> Result<()> {
        let outline = match contour {
            Contour::Circle { center, radius } => {
                self.call(
                    "sketch.circle.center",
                    json!({"center": center, "radius": radius}),
                )?;
                return Ok(());
            }
            Contour::Outline(outline) => outline,
        };
        use pcbcraft_core::library::Segment;
        if outline
            .segments
            .iter()
            .all(|s| matches!(s, Segment::Line { .. }))
        {
            let mut points = vec![outline.start];
            points.extend(outline.segments.iter().map(Segment::to));
            if points.len() > 2 && points.first() == points.last() {
                points.pop();
            }
            self.call("sketch.line", json!({"points": points, "closed": true}))?;
            return Ok(());
        }
        let mut from = outline.start;
        let mut run: Vec<[f64; 2]> = vec![from];
        for segment in &outline.segments {
            match *segment {
                Segment::Line { to } => run.push(to),
                Segment::Arc { to, bulge } => {
                    self.lines(&mut run)?;
                    self.call(
                        "sketch.arc.three_point",
                        json!({"start": from, "end": to, "through": bulge_midpoint(from, to, bulge)}),
                    )?;
                    run.clear();
                    run.push(to);
                }
            }
            from = segment.to();
        }
        self.lines(&mut run)
    }

    /// Draw an open polyline run and restart it at its last point.
    fn lines(&mut self, run: &mut Vec<[f64; 2]>) -> Result<()> {
        if run.len() > 1 {
            self.call("sketch.line", json!({"points": run}))?;
        }
        if let Some(last) = run.last().copied() {
            run.clear();
            run.push(last);
        }
        Ok(())
    }

    /// Top-layer parts sit on the board's top face (flipped, offset by the
    /// board thickness); bottom-layer parts hang from its bottom face.
    pub fn board_joint(&mut self, joint: BoardJoint) -> Result<()> {
        self.board_joint_at(joint, &JointFrame::ORIGIN, None)
    }

    /// [`Self::board_joint`] holding the occurrence by `frame` (in its
    /// definition's coordinates, the occurrence unmoved) instead of its
    /// origin, raised `lift` (a length expression) off the board face.
    pub fn board_joint_at(
        &mut self,
        joint: BoardJoint,
        frame: &JointFrame,
        lift: Option<&str>,
    ) -> Result<()> {
        let top = joint.layer == Layer::Top;
        let offset = match (lift, top) {
            (None, true) => "board_thickness".to_owned(),
            (None, false) => "0 mm".to_owned(),
            (Some(lift), true) => format!("board_thickness + ({lift})"),
            (Some(lift), false) => format!("- ({lift})"),
        };
        self.call(
            "joint.create",
            json!({"type": "planar", "name": format!("placement-{}", joint.occurrence),
                "a": {"occurrence": joint.board, "point": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]},
                "b": {"occurrence": joint.occurrence, "point": frame.point, "z": frame.z, "x": frame.x},
                "flip": top, "angle": 0,
                "offset": offset,
                "values": [joint.x.0, joint.y.0, joint.angle.0]}),
        )?;
        Ok(())
    }

    /// Assign a physical look to bodies, in chunks the command accepts. The
    /// look is stored in the native file, so Solidcraft shows the same
    /// material Pcbcraft does.
    pub fn appearance(&mut self, bodies: &[&str], material: &Material) -> Result<()> {
        self.look(
            bodies,
            &Look {
                name: material.name,
                srgb8: material.srgb8(),
                metallic: material.metallic,
                roughness: material.roughness,
                ior: material.ior,
            },
        )
    }

    /// Assign an opaque physical look to bodies, in chunks the command
    /// accepts.
    pub fn look(&mut self, bodies: &[&str], look: &Look<'_>) -> Result<()> {
        let [r, g, b] = look.srgb8;
        let color = format!("#{r:02x}{g:02x}{b:02x}");
        for chunk in bodies.chunks(APPEARANCE_CHUNK) {
            self.call(
                "appearance.assign",
                json!({"bodies": chunk, "name": format!("Pcbcraft {}", look.name),
                    "color": color, "opacity": 1.0,
                    "pbr": {"metallic": look.metallic, "roughness": look.roughness,
                        "dielectric_ior": look.ior}}),
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
