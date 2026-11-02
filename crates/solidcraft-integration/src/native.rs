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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Where a board joint holds an occurrence.
#[derive(Clone, Debug)]
pub enum Mount {
    /// On a board face: the top face (z = `board_thickness`, the
    /// occurrence's +z up) or hanging from the bottom face (z = 0, turned
    /// over).
    Side(Layer),
    /// Upright (the occurrence's +z up, unmirrored) with its origin at the
    /// board height `z`, a length expression: inner copper and vias.
    Height(Expr),
}

impl From<Layer> for Mount {
    fn from(side: Layer) -> Self {
        Self::Side(side)
    }
}

/// A planar joint attaching an occurrence to the board: on its top or
/// bottom face, or upright at a height inside it.
pub struct BoardJoint {
    pub board: u64,
    pub occurrence: u64,
    pub x: Expr,
    pub y: Expr,
    pub angle: Expr,
    pub mount: Mount,
}

/// A native Solidcraft design under construction or opened from disk.
pub struct Native {
    session: Session,
    /// Set by another thread to stop a build at its next command.
    cancel: Option<Arc<AtomicBool>>,
    /// Every command a build ran, when it records them ([`Self::record`]),
    /// so a later build of an edited board can update this one in place.
    log: Option<replay::Log>,
    /// A build of an edited board compared with [`Self::log`] instead of
    /// run ([`replay`]).
    replay: Option<replay::Replay>,
}

mod replay;
use replay::Log;

impl Native {
    fn with(session: Session) -> Self {
        let mut native = Self {
            session,
            cancel: None,
            log: None,
            replay: None,
        };
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

    /// Stop at the next command once `cancel` is set ([`Error::Cancelled`]).
    pub fn cancel_with(&mut self, cancel: Arc<AtomicBool>) {
        self.cancel = Some(cancel);
    }

    /// Record every command from now on, for [`Self::replaying`].
    pub(crate) fn record(&mut self) {
        self.log = Some(Log::default());
    }

    /// This design (built with [`Self::record`]) for a build of an edited
    /// board: every command the build issues is compared with the one the
    /// design was built with at that step and answered with its recorded
    /// reply, nothing runs, until [`Self::settle`] applies what changed.
    /// A command that differs in more than values (a parameter's
    /// expression, a sketch's drawing, where a placement stands, recorded
    /// metadata) fails the build with [`Error::Verification`]: the board
    /// is then built anew.
    pub(crate) fn replaying(mut self) -> Option<Self> {
        let log = self.log.take()?;
        self.replay = Some(replay::Replay::new(log));
        // The flag that could stop the build this design came from.
        self.cancel = None;
        Some(self)
    }

    /// Whether this design is being replayed ([`Self::replaying`]).
    pub(crate) fn is_replaying(&self) -> bool {
        self.replay.is_some()
    }

    /// The position in the command stream (recorded or replayed).
    pub(crate) fn step(&self) -> usize {
        match (&self.replay, &self.log) {
            (Some(replay), _) => replay.step(),
            (None, Some(log)) => log.len(),
            (None, None) => 0,
        }
    }

    /// Mark the commands from `start` to now as the definition `name`.
    pub(crate) fn mark(&mut self, name: &str, start: usize) {
        let end = self.step();
        if let Some(log) = &mut self.log {
            log.mark(name, start, end);
        }
        if let Some(replay) = &mut self.replay {
            replay.mark(name, start, end);
        }
    }

    /// Replaying, pass over the commands that built the definition `name`
    /// (built from the same immutable source, so they are the same);
    /// `false` if they are not next.
    pub(crate) fn pass(&mut self, name: &str) -> bool {
        self.replay.as_mut().is_some_and(|r| r.pass(name))
    }

    /// End a replay: apply what changed to the design (the changed
    /// parameters in one command, each changed sketch redrawn, each moved
    /// placement moved) and keep the new command stream. Recording, does
    /// nothing. Returns how many commands the update ran.
    pub(crate) fn settle(&mut self) -> Result<usize> {
        let Some(replay) = self.replay.take() else {
            return Ok(0);
        };
        // An update is short and leaves the design consistent only once
        // done: a newer request does not stop it.
        self.cancel = None;
        let (log, ran) = replay.apply(self)?;
        self.log = Some(log);
        Ok(ran)
    }

    /// Drop the design's undo history: a built model is never undone, and
    /// each step holds a whole design.
    pub(crate) fn forget_history(&mut self) {
        self.session.undo.clear();
        self.session.redo.clear();
    }

    /// Activate a component of the root by id.
    pub fn activate(&mut self, component: u64) -> Result<()> {
        self.call("component.activate", json!({"component": component}))?;
        Ok(())
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Put off evaluating the design after each command until [`Native::evaluate_deferred`]
    /// (Solidcraft's `Session::defer_evaluation`): a board built in hundreds of commands is
    /// evaluated a few times instead of after every one.
    /// Replaying ([`Self::replaying`]), nothing runs and nothing is put off.
    pub fn defer_evaluation(&mut self) -> Result<()> {
        if self.is_replaying() {
            return Ok(());
        }
        self.session
            .defer_evaluation()
            .map_err(|error| Error::Engine {
                command: "defer evaluation".into(),
                message: error.to_string(),
            })
    }

    /// Evaluate what [`Native::defer_evaluation`] put off, as one undo step; fails, leaving the
    /// design as it was before, when a feature added meanwhile fails.
    pub fn evaluate_deferred(&mut self) -> Result<()> {
        if self.is_replaying() {
            return Ok(());
        }
        self.session
            .evaluate_deferred()
            .map_err(|error| Error::Engine {
                command: "evaluate deferred".into(),
                message: error.to_string(),
            })
    }

    /// The session itself, for adding an import feature from bytes
    /// already read (`models::imported`): Solidcraft's import commands
    /// read files by path, and a packed library asset has none. What it
    /// changes is not recorded ([`Self::record`]): a replay passes over the
    /// whole definition built with it ([`Self::pass`]).
    pub(crate) fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    pub fn parameter(&mut self, name: &str, expression: impl Into<String>) -> Result<()> {
        self.call(
            "parameters.add",
            json!({"name": name, "expression": expression.into()}),
        )?;
        Ok(())
    }

    /// Add or change many parameters in one command, from a JSON list of
    /// `{name, expression}` rows (all or nothing).
    pub fn import_parameters(&mut self, rows: &str) -> Result<()> {
        self.call("parameters.import", json!({"text": rows}))?;
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

    /// [`Self::profile_body`] for contours that do not nest (a track's
    /// stadium): every profile is extruded whole (`profiles: "all"`), so in
    /// a batch of put-off evaluation ([`Self::defer_evaluation`]) the
    /// sketch and its extrusion are evaluated with the rest of the board
    /// at its end instead of evaluating the pending design each. A contour
    /// inside another would be filled, not a hole: use
    /// [`Self::profile_body`] for those.
    pub fn whole_profile_body(&mut self, body: ProfileBody<'_>) -> Result<()> {
        let sketch = self.sketch(body.name, body.offset, body.contours)?;
        self.call(
            "solid.extrude",
            json!({"sketch": sketch, "distance": body.height.0, "operation": "new",
                "profiles": "all", "name": body.name, "body_name": body.name}),
        )?;
        Ok(())
    }

    /// [`Self::profile_body`] for many contours (a laminate slab and its
    /// thousands of drills): the contours go into the sketch in one
    /// command, so building it does not grow with the square of their
    /// number. Overlapping contours are one boundary (the sketch's own
    /// arrangement unites them); nesting is even-odd. Returns the bodies
    /// made: `body.name`, then `body.name (2)`, … when the profile is in
    /// pieces (laminate left in a fill's clearance).
    pub fn batched_body(&mut self, body: ProfileBody<'_>) -> Result<Vec<String>> {
        let reply = self.call(
            "sketch.create",
            json!({"plane": Plane::Xy.name(), "offset": body.offset.0,
                "name": format!("{} sketch", body.name)}),
        )?;
        let sketch = id(&reply, "sketch")?;
        let drawing = dxf(body.contours)?;
        self.call(
            "sketch.insert_dxf",
            json!({"text": drawing, "tolerance": DXF_JOIN_MM}),
        )?;
        self.call("sketch.finish", json!({}))?;
        let reply = self.call(
            "solid.extrude",
            json!({"sketch": sketch, "distance": body.height.0, "operation": "new",
                "name": body.name, "body_name": body.name}),
        )?;
        Ok(created_bodies(&reply, body.name))
    }

    /// A sketched board's slab: [`Self::batched_body`]'s holes inside
    /// `outline`, drawn in a command of its own so that redrawing the holes
    /// (an update) keeps the outline's curves, by which the extrusion picks
    /// its profile.
    pub fn outlined_slab(
        &mut self,
        body: ProfileBody<'_>,
        outline: &Contour,
    ) -> Result<Vec<String>> {
        let reply = self.call(
            "sketch.create",
            json!({"plane": Plane::Xy.name(), "offset": body.offset.0,
                "name": format!("{} sketch", body.name)}),
        )?;
        let sketch = id(&reply, "sketch")?;
        if !body.contours.is_empty() {
            let drawing = dxf(body.contours)?;
            self.call(
                "sketch.insert_dxf",
                json!({"text": drawing, "tolerance": DXF_JOIN_MM}),
            )?;
        }
        let drawing = dxf(std::slice::from_ref(outline))?;
        self.call(
            "sketch.insert_dxf",
            json!({"text": drawing, "tolerance": DXF_JOIN_MM}),
        )?;
        self.call("sketch.finish", json!({}))?;
        let reply = self.call(
            "solid.extrude",
            json!({"sketch": sketch, "distance": body.height.0, "operation": "new",
                "name": body.name, "body_name": body.name}),
        )?;
        Ok(created_bodies(&reply, body.name))
    }

    /// A board slab: [`Self::batched_body`]'s holes inside the board
    /// outline, a rectangle from the origin to the length parameters
    /// `size` (`board_width`, `board_depth`), so the saved slab follows the
    /// board size.
    pub fn board_slab(&mut self, body: ProfileBody<'_>, size: [&str; 2]) -> Result<Vec<String>> {
        let reply = self.call(
            "sketch.create",
            json!({"plane": Plane::Xy.name(), "offset": body.offset.0,
                "name": format!("{} sketch", body.name)}),
        )?;
        let sketch = id(&reply, "sketch")?;
        if !body.contours.is_empty() {
            let drawing = dxf(body.contours)?;
            self.call(
                "sketch.insert_dxf",
                json!({"text": drawing, "tolerance": DXF_JOIN_MM}),
            )?;
        }
        let [w, d] = size;
        self.call(
            "sketch.line",
            json!({"points": [[0, 0], [w, 0], [w, d], [0, d]], "closed": true}),
        )?;
        self.call("sketch.finish", json!({}))?;
        let reply = self.call(
            "solid.extrude",
            json!({"sketch": sketch, "distance": body.height.0, "operation": "new",
                "name": body.name, "body_name": body.name}),
        )?;
        Ok(created_bodies(&reply, body.name))
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

    /// Round the edges of `body` passing through `edges` (one point on
    /// each, in the active component's frame) to `radius`. The edges are
    /// kept by the engine's naming history, so the round follows the
    /// body's driving parameters.
    pub fn fillet(
        &mut self,
        name: &str,
        body: &str,
        edges: &[[f64; 3]],
        radius: Expr,
    ) -> Result<()> {
        self.call(
            "solid.fillet",
            json!({"name": name, "body": body, "edges": edges, "radius": radius.0}),
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

    /// Place `component` once more under the root, moved `by` (world
    /// axes) from where its first occurrence is now, in one command;
    /// returns the new occurrence.
    pub fn copy_occurrence_by(&mut self, component: u64, by: [f64; 3]) -> Result<u64> {
        self.activate_root()?;
        let reply = self.call(
            "occurrence.copy",
            json!({"component": component, "translate": by}),
        )?;
        id(&reply, "occurrence")
    }

    /// Place `component` once per offset (world axes) from where its first
    /// occurrence is now, all in one command; returns the new occurrences
    /// in the offsets' order.
    pub fn copy_occurrences(&mut self, component: u64, offsets: &[[f64; 3]]) -> Result<Vec<u64>> {
        if offsets.is_empty() {
            return Ok(Vec::new());
        }
        self.activate_root()?;
        let reply = self.call(
            "occurrence.copy",
            json!({"component": component, "translations": offsets}),
        )?;
        reply
            .get("occurrences")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Verification(format!("copies named no occurrences: {reply}")))?
            .iter()
            .map(|v| {
                v.as_u64()
                    .ok_or_else(|| Error::Verification(format!("bad occurrence id {v}")))
            })
            .collect()
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
    /// board thickness); bottom-layer parts hang from its bottom face; a
    /// [`Mount::Height`] occurrence stands upright at its height.
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
        self.call("joint.create", joint_params(&joint, frame, lift))?;
        Ok(())
    }

    /// Many [`Self::board_joint_at`] joints made and solved together in
    /// one command (one by one, each solves every joint made before it).
    pub fn board_joints(
        &mut self,
        joints: &[(BoardJoint, JointFrame, Option<String>)],
    ) -> Result<()> {
        for chunk in joints.chunks(MAX_JOINTS_PER_CALL) {
            let list: Vec<Value> = chunk
                .iter()
                .map(|(joint, frame, lift)| joint_params(joint, frame, lift.as_deref()))
                .collect();
            self.call("joint.create", json!({"joints": list}))?;
        }
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
        if self
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
        {
            return Err(Error::Cancelled);
        }
        if let Some(replay) = &mut self.replay {
            return replay.next(command, params);
        }
        let reply = self.run(command, &params)?;
        if let Some(log) = &mut self.log {
            log.push(command, params, reply.clone());
        }
        Ok(reply)
    }

    /// Run one command on the session, unrecorded.
    fn run(&mut self, command: &str, params: &Value) -> Result<Value> {
        self.session
            .execute(command, params)
            .map_err(|error| Error::Engine {
                command: command.into(),
                message: error.to_string(),
            })
    }
}

/// Most joints one `joint.create` makes (Solidcraft's limit a design).
const MAX_JOINTS_PER_CALL: usize = 10_000;

/// `joint.create`'s parameters for a [`BoardJoint`] holding `frame` of the
/// occurrence, raised `lift` off the board face.
fn joint_params(joint: &BoardJoint, frame: &JointFrame, lift: Option<&str>) -> Value {
    let (top, offset) = match (&joint.mount, lift) {
        (Mount::Side(Layer::Top), None) => (true, json!("board_thickness")),
        (Mount::Side(Layer::Bottom), None) => (false, json!("0 mm")),
        (Mount::Side(Layer::Top), Some(lift)) => {
            (true, json!(format!("board_thickness + ({lift})")))
        }
        (Mount::Side(Layer::Bottom), Some(lift)) => (false, json!(format!("- ({lift})"))),
        (Mount::Height(z), None) => (true, z.0.clone()),
        (Mount::Height(z), Some(lift)) => {
            let z = match &z.0 {
                Value::String(e) => e.clone(),
                other => format!("{other} mm"),
            };
            (true, json!(format!("({z}) + ({lift})")))
        }
    };
    json!({"type": "planar", "name": format!("placement-{}", joint.occurrence),
        "a": {"occurrence": joint.board, "point": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]},
        "b": {"occurrence": joint.occurrence, "point": frame.point, "z": frame.z, "x": frame.x},
        "flip": top, "angle": 0,
        "offset": offset,
        "values": [joint.x.0, joint.y.0, joint.angle.0]})
}

/// How close two drawn points must be to be one sketch point in a batched
/// sketch: far below any copper or drill feature, above rounding.
/// The bodies an extrusion's reply says it made, or `name` alone when it
/// names none (a reply put off in a batch).
fn created_bodies(reply: &Value, name: &str) -> Vec<String> {
    let made: Vec<String> = reply
        .get("created_bodies")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if made.is_empty() {
        vec![name.to_owned()]
    } else {
        made
    }
}

const DXF_JOIN_MM: f64 = 1e-6;

/// Most bytes of one batched drawing (Solidcraft refuses drawings over 64 MiB).
const MAX_DRAWING_BYTES: usize = 48 << 20;

/// Closed contours as an ASCII DXF drawing in millimetres (circles, and
/// closed polylines whose arcs keep their bulge), the form Solidcraft's
/// sketch insert reads in one command. Numbers are written in Rust's
/// shortest round-trip form, so every coordinate is read back exactly.
fn dxf(contours: &[Contour]) -> Result<String> {
    use pcbcraft_core::library::Segment;
    use std::fmt::Write;
    let mut out = String::from("0\nSECTION\n2\nENTITIES\n");
    let finite = |v: f64| {
        v.is_finite()
            .then_some(v)
            .ok_or_else(|| Error::Invalid("a sketch contour is not finite".into()))
    };
    for contour in contours {
        match contour {
            Contour::Circle { center, radius } => {
                let (x, y, r) = (finite(center[0])?, finite(center[1])?, finite(*radius)?);
                let _ = write!(out, "0\nCIRCLE\n8\n0\n10\n{x}\n20\n{y}\n40\n{r}\n");
            }
            Contour::Outline(outline) => {
                // Vertex k carries the bulge of the segment from it to the
                // next; the closing segment ends at the start again.
                let mut vertices = vec![(outline.start, 0.0)];
                for segment in &outline.segments {
                    let (to, bulge) = match *segment {
                        Segment::Line { to } => (to, 0.0),
                        Segment::Arc { to, bulge } => (to, bulge),
                    };
                    if let Some(last) = vertices.last_mut() {
                        last.1 = bulge;
                    }
                    vertices.push((to, 0.0));
                }
                // A closed polyline repeats no point: drop a final point
                // back at the start, keeping the bulge before it.
                if vertices.len() > 2 && vertices.last().map(|v| v.0) == Some(outline.start) {
                    vertices.pop();
                }
                let _ = write!(out, "0\nLWPOLYLINE\n8\n0\n90\n{}\n70\n1\n", vertices.len());
                for ([x, y], bulge) in vertices {
                    let (x, y, bulge) = (finite(x)?, finite(y)?, finite(bulge)?);
                    let _ = write!(out, "10\n{x}\n20\n{y}\n");
                    if bulge != 0.0 {
                        let _ = write!(out, "42\n{bulge}\n");
                    }
                }
            }
        }
        if out.len() > MAX_DRAWING_BYTES {
            return Err(Error::Invalid("the sketch has too many contours".into()));
        }
    }
    out.push_str("0\nENDSEC\n0\nEOF\n");
    Ok(out)
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
