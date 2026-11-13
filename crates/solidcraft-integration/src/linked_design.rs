//! A SolidCraft linked design, opened in Sparksmith's own engine: the
//! headless side of the board → assembly link (docs/architecture/
//! board-interface.md, "Link updates"). It links a published board
//! interface as SolidCraft's Insert Design does, re-reads it as Update
//! Links does, and reports its bodies' faces and its features' failures.
//!
//! Slice B3 uses it to check that a linked design follows a published board
//! (a wall with a cutout projected from an opening, a boss on a mounting
//! hole). Slice B4 reads a saved linked design the board is linked to (the
//! component that links the board's design and the board frame in it, the
//! faces the board follows traced in the board plane, its parameters, the
//! features that use the board's datums) and runs the dry run of a publish
//! on a scratch copy of it.
use crate::native::{BoxBody, Native, id};
use crate::{Error, Result};
use serde_json::{Value, json};
use solidcraft_engine::doc::FeatureKind;
use solidcraft_engine::sketch::LinkSource;
use sparksmith_core::geometry::Transform3;
use sparksmith_core::ids::ContentHash;
use sparksmith_core::interface::{AssemblyEffect, EffectKind, Sidecar};
use sparksmith_core::model::assembly_link::UseKind;
use sparksmith_core::model::dimension::{BodyName, FaceName, ModelFaceRef};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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

/// A feature that does not evaluate, by its name in the linked design.
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

/// An linked design.
pub struct LinkedDesign {
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

impl LinkedDesign {
    /// A new, empty linked design.
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

// Slice B4: reading a linked design the board is linked to, and the dry run.

/// Linked design bodies whose faces a dry run compares, at most.
const MAX_DRY_RUN_BODIES: usize = 512;
/// How far (mm) a face's centroid may move and still count as unmoved.
const MOVED_MM: f64 = 1e-6;

/// The component of a linked design that links the board's published design,
/// and the board's frame in it.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardLink {
    pub component: u64,
    /// Its name in the linked design.
    pub name: String,
    /// The Import feature that holds the linked bodies.
    pub feature: u64,
    /// The linked design's world frame to the board's frame.
    pub to_board: Transform3,
    /// The linked bodies, by their names in the linked design.
    pub bodies: Vec<String>,
    /// SHA-256 of the board's design file as the linked design last read it
    /// (SolidCraft keeps it on the link), when it keeps one: the older-board
    /// check.
    pub file_sha256: Option<ContentHash>,
}

/// A linked design feature that uses a published datum, as its sketch's
/// links say, or a joint that places the board by one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Use {
    /// The feature's (or joint's) name in the linked design.
    pub feature: String,
    pub feature_id: u64,
    pub token: String,
    pub via: UseKind,
}

/// A linked design feature attached to a face of the board's design that no
/// slot carries (section 2).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unexported {
    pub feature: String,
    /// The board design's body, by its name there ("U1 model"); empty when
    /// only a face name says it is the board's.
    pub body: String,
}

/// What a reference to the board's design is, by the slot that carries it.
enum Carried<'a> {
    /// Not the board's.
    Elsewhere,
    /// A slot's: the datum's token.
    Slot(&'a str),
    /// The board's, but no slot carries it.
    Unexported,
}

/// Linked design features one dependency walk visits, at most.
const MAX_WALK: usize = 4096;

/// The published datums by what names them in the linked design: their bodies
/// (by token) and their faces (by SolidCraft's digest of the published
/// face name), with the slab's own token.
struct Slots<'a> {
    tokens: BTreeSet<&'a str>,
    by_digest: BTreeMap<String, &'a str>,
    /// The outline datum's token: every face of the slab is its slot's.
    slab: Option<&'a str>,
    linked: BTreeSet<&'a str>,
}

impl<'a> Slots<'a> {
    fn new(link: &'a BoardLink, sidecar: &'a Sidecar) -> Self {
        let mut by_digest = BTreeMap::new();
        for d in &sidecar.datums {
            for face in d.faces.values() {
                by_digest
                    .entry(link_digest(face))
                    .or_insert(d.token.as_str());
            }
        }
        Self {
            tokens: sidecar.datums.iter().map(|d| d.token.as_str()).collect(),
            by_digest,
            slab: sidecar
                .datums
                .iter()
                .find(|d| d.role == "board.outline")
                .map(|d| d.token.as_str()),
            linked: link.bodies.iter().map(String::as_str).collect(),
        }
    }

    /// The datum a linked face name (`F<feature>:link:<digest>`) is of.
    fn by_face(&self, face: &str) -> Option<&'a str> {
        let (_, digest) = face.split_once(":link:")?;
        self.by_digest
            .get(digest.get(..16).unwrap_or(digest))
            .copied()
    }

    /// A reference to `body` (its face `face`, when it names one).
    fn carried(&self, body: &str, face: Option<&str>) -> Carried<'a> {
        if !self.linked.contains(body) {
            return Carried::Elsewhere;
        }
        if let Some(token) = self.tokens.get(body) {
            return Carried::Slot(token);
        }
        if let Some(token) = face.and_then(|f| self.by_face(f)) {
            return Carried::Slot(token);
        }
        match self.slab {
            Some(token) if body == crate::interface::SLAB => Carried::Slot(token),
            _ => Carried::Unexported,
        }
    }
}

/// Every string in a value that names a linked face.
fn linked_faces(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if text.contains(":link:") => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|v| linked_faces(v, out)),
        Value::Object(map) => map.values().for_each(|v| linked_faces(v, out)),
        _ => {}
    }
}

/// The features a non-sketch feature's own fields name: a `sketch` or
/// `profile` id, or a persistent face name (`F12:side:l3`).
fn named_features(value: &Value, out: &mut Vec<u64>) {
    match value {
        Value::String(text) => out.extend(feature_of(text)),
        Value::Array(items) => items.iter().for_each(|v| named_features(v, out)),
        Value::Object(map) => {
            for (key, v) in map {
                match (key.as_str(), v.as_u64()) {
                    ("sketch" | "profile" | "feature", Some(id)) => out.push(id),
                    _ => named_features(v, out),
                }
            }
        }
        _ => {}
    }
}

/// SolidCraft's digest of a linked design's face name, as a linked face's
/// name carries it (`F<feature>:link:<digest>`): FNV-1a over the name.
fn link_digest(name: &str) -> String {
    let digest = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{digest:016x}")
}

/// The feature id a persistent face name starts with (`F12:side:l3` → 12).
fn feature_of(face: &str) -> Option<u64> {
    let rest = face.strip_prefix('F')?;
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest.get(..end)?.parse().ok()
}

fn to_transform(m: &solidcraft_engine::doc::Mat) -> Transform3 {
    // Solidcraft's matrices are column-major: m[column][row].
    Transform3 {
        rotation: std::array::from_fn(|i| std::array::from_fn(|j| m[j][i])),
        translation: [m[3][0], m[3][1], m[3][2]],
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn mm_words(v: f64) -> String {
    sparksmith_core::layout::limits::mm(v)
}

/// "2 mm along X" for an offset.
fn offset_words(d: [f64; 3]) -> String {
    let big: Vec<usize> = (0..3).filter(|k| d[*k].abs() > MOVED_MM).collect();
    match big.as_slice() {
        [k] => format!(
            "{} mm along {}",
            mm_words(d[*k].abs()),
            ["X", "Y", "Z"].get(*k).copied().unwrap_or("X")
        ),
        _ => format!(
            "{} mm",
            mm_words((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt())
        ),
    }
}

impl LinkedDesign {
    /// Open a saved linked design read-only (a copy the caller made, so
    /// nothing the user saved is ever touched). `true` with it when a newer
    /// SolidCraft saved it: what this build cannot read is then missing.
    pub fn open(path: &str) -> Result<(Self, bool)> {
        let (native, reply) = Native::open_reporting(path)?;
        let newer = reply.get("newer_file").is_some_and(|v| !v.is_null());
        Ok((Self { native }, newer))
    }

    /// Save this design as a new file (test fixtures).
    pub fn save(&self, path: &str) -> Result<()> {
        self.native.save_new(path)
    }

    /// The component that links `design` (the board's published design),
    /// matched by its link resolving to that file, never by name. Link
    /// paths relative to the linked design resolve against `folder`, where the
    /// user keeps it.
    pub fn board_link(&self, folder: &Path, design: &Path) -> Result<BoardLink> {
        let session = self.native.session();
        let doc = &session.doc;
        // Opened from a copy, SolidCraft reads a relative link against the
        // copy's folder; it is the user's folder that holds the design.
        let opened_in = session
            .path
            .as_deref()
            .and_then(|p| Path::new(p).parent().map(Path::to_path_buf));
        let mut found = doc.links.iter().filter(|(_, link)| {
            let stored = Path::new(&link.path);
            let relative = match &opened_in {
                Some(at) if stored.is_absolute() => stored.strip_prefix(at).ok(),
                _ => None,
            };
            let resolved = match relative {
                Some(rest) => folder.join(rest),
                None if stored.is_relative() => folder.join(stored),
                None => stored.to_path_buf(),
            };
            same_file(&resolved, design)
        });
        let (component, link) = found.next().ok_or_else(|| {
            Error::Invalid(format!(
                "assembly.board: the linked design does not link this board's published design \
                 ({}); insert it in SolidCraft with Insert Design, or relink",
                design.display()
            ))
        })?;
        if found.next().is_some() {
            return Err(Error::Invalid(
                "assembly.board: the assembly links the board's design twice; keep one".into(),
            ));
        }
        let mut occurrences = doc.occurrences.iter().filter(|o| o.component == *component);
        let occurrence = occurrences.next().ok_or_else(|| {
            Error::Invalid("assembly.board: the board's component is not placed".into())
        })?;
        if occurrences.next().is_some() {
            return Err(Error::Invalid(
                "assembly.board: the board is placed more than once in the linked design; the board \
                 frame must be one"
                    .into(),
            ));
        }
        let world = solidcraft_engine::doc::joints::occurrence_world(doc, occurrence.id);
        let inverse = solidcraft_engine::doc::mat_inverse(&world).ok_or_else(|| {
            Error::Invalid("assembly.board: the board's placement cannot be inverted".into())
        })?;
        let name = doc
            .components
            .iter()
            .find(|c| c.id == *component)
            .map_or_else(|| "Board".to_owned(), |c| c.name.clone());
        let bodies = doc
            .features
            .iter()
            .find(|f| f.id == link.feature)
            .map(|f| f.body_names.clone())
            .unwrap_or_default();
        Ok(BoardLink {
            component: *component,
            name,
            feature: link.feature,
            to_board: to_transform(&inverse),
            bodies,
            file_sha256: link.file_sha256.as_deref().and_then(ContentHash::parse),
        })
    }

    /// Trace the face `face` of linked design body `body` (picked at `pick`,
    /// linked design frame) in the board plane: a face square to the board as
    /// its line, a vertical cylinder as its axis point. With it, a point on
    /// the face where it is now (the pick, unless the face moved).
    pub fn trace(
        &self,
        link: &BoardLink,
        body: &str,
        face: &str,
        pick: [f64; 3],
    ) -> sparksmith_core::diagnostics::Result<(
        sparksmith_core::layout::dimensions::model::Traced,
        [f64; 3],
    )> {
        let state = self
            .native
            .world_state()
            .map_err(|e| sparksmith_core::diagnostics::Error::Invalid(e.to_string()))?;
        let reference = ModelFaceRef {
            body: BodyName(body.to_owned()),
            face: FaceName(face.to_owned()),
            pick,
        };
        let words = format!("the linked design's face ‘{face}’ of ‘{body}’");
        crate::board::model_refs::trace_linked_face(&state, &reference, &words, &link.to_board)
    }

    /// The value of the linked design's user parameter `name` in mm, when it is
    /// a length.
    pub fn parameter_mm(&self, name: &str) -> Option<f64> {
        let (values, _) = self.native.session().doc.param_values();
        let value = values.get(name)?;
        (value.len == 1 && value.ang == 0).then_some(value.v)
    }

    /// Set the linked design's user parameter `name` (test fixtures).
    pub fn set_parameter(&mut self, name: &str, expression: &str) -> Result<()> {
        self.native.parameter(name, expression.to_owned())
    }

    /// Every linked design feature whose sketch projects or intersects a body
    /// of the linked board that a slot of `sidecar` carries (a datum's own
    /// body, named by its token; a face the sidecar lists for a datum; any
    /// face of the board slab, the outline's slot), and every joint that
    /// places the board by a face a slot carries: the consumers, by slot.
    pub fn uses(&self, link: &BoardLink, sidecar: &Sidecar) -> Vec<Use> {
        let slots = Slots::new(link, sidecar);
        let mut out = BTreeSet::new();
        for feature in &self.native.session().doc.features {
            let FeatureKind::Sketch { sketch, .. } = &feature.kind else {
                continue;
            };
            for l in &sketch.links {
                let Some(body) = link_body(&l.source) else {
                    continue;
                };
                if let Carried::Slot(token) = slots.carried(body, l.face_name.as_deref()) {
                    out.insert(Use {
                        feature: feature.name.clone(),
                        feature_id: feature.id,
                        token: token.to_owned(),
                        via: UseKind::Sketch,
                    });
                }
            }
        }
        out.extend(self.joint_uses(link, &slots).0);
        out.into_iter().collect()
    }

    /// Every linked design feature (and joint) attached to a face of the
    /// linked board that no slot carries: section 2's parts, laminate and
    /// pads, whose names may move to another body when the board changes.
    pub fn unexported(&self, link: &BoardLink, sidecar: &Sidecar) -> Vec<Unexported> {
        let slots = Slots::new(link, sidecar);
        let mut out = BTreeSet::new();
        for feature in &self.native.session().doc.features {
            let FeatureKind::Sketch { sketch, .. } = &feature.kind else {
                continue;
            };
            for l in &sketch.links {
                let Some(body) = link_body(&l.source) else {
                    continue;
                };
                if let Carried::Unexported = slots.carried(body, l.face_name.as_deref()) {
                    out.insert(Unexported {
                        feature: feature.name.clone(),
                        body: body.to_owned(),
                    });
                }
            }
        }
        out.extend(self.joint_uses(link, &slots).1);
        out.into_iter().collect()
    }

    /// The joints that hold the board's occurrence by one of its faces:
    /// those on a face a slot carries, and those on one no slot carries.
    fn joint_uses(&self, link: &BoardLink, slots: &Slots<'_>) -> (Vec<Use>, Vec<Unexported>) {
        let doc = &self.native.session().doc;
        let board_occurrences: BTreeSet<u64> = doc
            .occurrences
            .iter()
            .filter(|o| o.component == link.component)
            .map(|o| o.id)
            .collect();
        let mut uses = Vec::new();
        let mut unexported = Vec::new();
        for joint in &doc.assembly.joints {
            if joint.suppressed {
                continue;
            }
            for origin in [&joint.a, &joint.b] {
                if !board_occurrences.contains(&origin.part()) {
                    continue;
                }
                let mut faces = Vec::new();
                if let Ok(snap) = serde_json::to_value(&origin.snap) {
                    linked_faces(&snap, &mut faces);
                }
                for face in faces {
                    match slots.by_face(&face) {
                        Some(token) => uses.push(Use {
                            feature: joint.name.clone(),
                            feature_id: joint.id,
                            token: token.to_owned(),
                            via: UseKind::Joint,
                        }),
                        None => unexported.push(Unexported {
                            feature: joint.name.clone(),
                            body: String::new(),
                        }),
                    }
                }
            }
        }
        (uses, unexported)
    }

    /// The sketch whose plane a face lies on, when that is all that places
    /// it: the start cap of an extrude (`F<n>:start`) whose sketch is on a
    /// fixed plane (an origin plane, or one given by numbers). That
    /// sketch's projections shape the profile in the plane, the face's
    /// extent, never where the face is.
    fn plane_of(&self, face: &str) -> Option<u64> {
        let (_, role) = face.split_once(':')?;
        if role != "start" {
            return None;
        }
        let doc = &self.native.session().doc;
        let made = feature_of(face)?;
        let FeatureKind::Extrude { sketch, .. } = &doc.features.iter().find(|f| f.id == made)?.kind
        else {
            return None;
        };
        let FeatureKind::Sketch { plane, .. } =
            &doc.features.iter().find(|f| f.id == sketch.0)?.kind
        else {
            return None;
        };
        use solidcraft_engine::doc::PlaneRef;
        matches!(
            plane,
            PlaneRef::Origin { .. } | PlaneRef::Custom { .. } | PlaneRef::Placed { .. }
        )
        .then_some(sketch.0)
    }

    /// The board's datums the linked design geometry under the face `face`
    /// (a persistent name) is built on: walking back from the feature that
    /// made the face through the sketches, faces and bodies it is made
    /// from to every reference a slot carries, and every joint that places
    /// the board (the board frame, and the face's trace with it, depends
    /// on those). The linked design half of the cycle graph. A datum that
    /// only shapes the profile of the sketch whose fixed plane the face
    /// lies on sets the face's extent, not its position: it reads as
    /// [`UseKind::Extent`], unless the face is built on it another way too.
    pub fn built_on(&self, link: &BoardLink, sidecar: &Sidecar, face: &str) -> Vec<Use> {
        let slots = Slots::new(link, sidecar);
        let doc = &self.native.session().doc;
        let mut out: BTreeSet<Use> = self.joint_uses(link, &slots).0.into_iter().collect();
        let plane = self.plane_of(face);
        // Each feature with whether what it is built on reaches the face
        // only through the plane sketch's profile (its extent).
        let mut queue: Vec<(u64, bool)> =
            feature_of(face).map(|f| (f, false)).into_iter().collect();
        let mut seen = BTreeSet::new();
        while let Some((id, extent)) = queue.pop() {
            if seen.len() >= MAX_WALK || !seen.insert((id, extent)) {
                continue;
            }
            let extent = extent || Some(id) == plane;
            let via = if extent {
                UseKind::Extent
            } else {
                UseKind::Sketch
            };
            let Some(feature) = doc.features.iter().find(|f| f.id == id) else {
                continue;
            };
            if feature.id == link.feature {
                continue;
            }
            match &feature.kind {
                FeatureKind::Sketch { sketch, .. } => {
                    for l in &sketch.links {
                        let Some(body) = link_body(&l.source) else {
                            continue;
                        };
                        match slots.carried(body, l.face_name.as_deref()) {
                            Carried::Slot(token) => {
                                out.insert(Use {
                                    feature: feature.name.clone(),
                                    feature_id: feature.id,
                                    token: token.to_owned(),
                                    via,
                                });
                            }
                            Carried::Unexported => {}
                            Carried::Elsewhere => match l.face_name.as_deref().and_then(feature_of)
                            {
                                Some(made) => queue.push((made, extent)),
                                // A whole body: every feature that shaped it.
                                None => queue.extend(
                                    doc.features
                                        .iter()
                                        .filter(|f| f.body_names.iter().any(|b| b == body))
                                        .map(|f| (f.id, extent)),
                                ),
                            },
                        }
                    }
                }
                FeatureKind::Extrude { sketch, .. } => queue.push((sketch.0, extent)),
                kind => {
                    if let Ok(value) = serde_json::to_value(kind) {
                        let mut named = Vec::new();
                        named_features(&value, &mut named);
                        queue.extend(named.into_iter().map(|f| (f, extent)));
                        let mut faces = Vec::new();
                        linked_faces(&value, &mut faces);
                        for face in faces {
                            if let Some(token) = slots.by_face(&face) {
                                out.insert(Use {
                                    feature: feature.name.clone(),
                                    feature_id: feature.id,
                                    token: token.to_owned(),
                                    via,
                                });
                            }
                        }
                    }
                }
            }
        }
        // A datum the face is built on by position too is a position use.
        let placed: BTreeSet<String> = out
            .iter()
            .filter(|u| u.via != UseKind::Extent)
            .map(|u| u.token.clone())
            .collect();
        out.into_iter()
            .filter(|u| u.via != UseKind::Extent || !placed.contains(&u.token))
            .collect()
    }

    /// The faces of every linked design body that is not the board's, by
    /// persistent name, with their centroid and area.
    fn own_faces(&mut self, link: &BoardLink) -> Result<BTreeMap<String, Face>> {
        let state = self.native.world_state()?;
        let names: Vec<String> = state
            .bodies
            .iter()
            .filter(|b| b.component != link.component && !b.body.is_mesh())
            .map(|b| b.name.clone())
            .take(MAX_DRY_RUN_BODIES)
            .collect();
        let mut out = BTreeMap::new();
        for name in names {
            for face in self.faces(&name)? {
                out.insert(face.name.clone(), face);
            }
        }
        Ok(out)
    }

    fn feature_name(&self, id: u64) -> Option<String> {
        self.native
            .session()
            .doc
            .features
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.name.clone())
    }

    /// The sketch a feature is made from (an extrude's), or itself.
    fn source_sketch(&self, id: u64) -> u64 {
        match self
            .native
            .session()
            .doc
            .features
            .iter()
            .find(|f| f.id == id)
            .map(|f| &f.kind)
        {
            Some(FeatureKind::Extrude { sketch, .. }) => sketch.0,
            _ => id,
        }
    }

    /// The dry run (docs/architecture/board-interface.md, "Link updates"):
    /// point this linked design's board link at the candidate design (as
    /// `component.relink` does), evaluate, and report in the linked design's
    /// own names the features that would move, fail or leave a joint
    /// unsolved. `published` is the sidecar the assembly links now, which
    /// says which datum each feature uses; `candidate` the one about to be
    /// published. Run it on a copy: the design is changed in memory.
    pub fn dry_run(
        &mut self,
        link: &BoardLink,
        design: &str,
        published: &Sidecar,
        candidate: &Sidecar,
    ) -> Result<Vec<AssemblyEffect>> {
        let uses = self.uses(link, published);
        let unexported = self.unexported(link, published);
        let before_errors: BTreeSet<String> = self
            .feature_errors()
            .into_iter()
            .map(|e| e.feature)
            .collect();
        let before = self.own_faces(link)?;
        let relinked = self.native.call(
            "component.relink",
            json!({"component": link.component, "path": design}),
        );
        let mut effects = Vec::new();
        let reply = match relinked {
            Ok(reply) => reply,
            Err(error) => {
                // The board's own bodies would not evaluate: everything
                // that uses them fails.
                let text = error.to_string();
                for u in &uses {
                    effects.push(failure(u, &text, published, candidate));
                }
                return Ok(effects);
            }
        };
        let mut reported = BTreeSet::new();
        for note in self.feature_errors() {
            if before_errors.contains(&note.feature) || !reported.insert(note.feature.clone()) {
                continue;
            }
            effects.push(match uses.iter().find(|u| u.feature == note.feature) {
                Some(u) => failure(u, &note.error, published, candidate),
                None => AssemblyEffect {
                    feature: note.feature.clone(),
                    effect: EffectKind::Fails,
                    datum: None,
                    message: format!(
                        "The linked design's ‘{}’ would fail: {}",
                        note.feature, note.error
                    ),
                },
            });
        }
        for joint in strings(&reply["joint_errors"]) {
            effects.push(AssemblyEffect {
                feature: joint.clone(),
                effect: EffectKind::JointUnsolved,
                datum: None,
                message: format!("Joint ‘{joint}’ would no longer solve"),
            });
        }
        // Features on faces no slot carries may follow another body after
        // the publish, whatever the dry run shows now.
        let mut named = BTreeSet::new();
        for u in unexported {
            if !named.insert(u.feature.clone()) {
                continue;
            }
            // The part the body is, as the sidecar names it: a package's
            // body is named by its package, not by its part.
            let part = match published.part_of(&u.body) {
                Some(p) => p.designator.clone(),
                None if u.body.is_empty() => "the board".to_owned(),
                None => format!("the board's ‘{}’", u.body),
            };
            effects.push(AssemblyEffect {
                feature: u.feature.clone(),
                effect: EffectKind::UnexportedReference,
                datum: None,
                message: sparksmith_core::interface::findings::assembly_unexported_face(
                    &format!("‘{}’", u.feature),
                    &part,
                    Vec::new(),
                )
                .message,
            });
        }
        let after = self.own_faces(link)?;
        // A feature moved when one of its faces left its plane (or, not
        // being a plane, moved at all); a face that only slid within its
        // plane (a wall face around a moved hole) does not count, but its
        // shift still says how far the feature went.
        let mut shifted: BTreeMap<u64, ([f64; 3], bool)> = BTreeMap::new();
        for (name, a) in &before {
            let (Some(b), Some(feature)) = (after.get(name), feature_of(name)) else {
                continue;
            };
            let d = [
                b.centroid[0] - a.centroid[0],
                b.centroid[1] - a.centroid[1],
                b.centroid[2] - a.centroid[2],
            ];
            if d.iter().all(|v| v.abs() <= MOVED_MM) {
                continue;
            }
            let left = a
                .normal
                .is_none_or(|n| (n[0] * d[0] + n[1] * d[1] + n[2] * d[2]).abs() > MOVED_MM);
            let entry = shifted.entry(feature).or_insert((d, false));
            if d.iter().map(|v| v.abs()).sum::<f64>() > entry.0.iter().map(|v| v.abs()).sum() {
                entry.0 = d;
            }
            entry.1 |= left;
        }
        let moved: BTreeMap<u64, [f64; 3]> = shifted
            .into_iter()
            .filter(|(_, (_, left))| *left)
            .map(|(f, (d, _))| (f, d))
            .collect();
        for (feature, d) in moved {
            let name = self
                .feature_name(feature)
                .unwrap_or_else(|| format!("F{feature}"));
            if reported.contains(&name) {
                continue;
            }
            let sketch = self.source_sketch(feature);
            let used = uses
                .iter()
                .find(|u| u.feature_id == sketch || u.feature_id == feature);
            let named = used.and_then(|u| published.datums.iter().find(|d| d.token == u.token));
            let message = match (used, named) {
                (_, Some(datum)) => format!(
                    "The linked design's ‘{name}’ follows {}: it moves {}",
                    datum.name,
                    offset_words(d)
                ),
                (Some(u), None) => format!(
                    "The linked design's ‘{name}’ follows {}: it moves {}",
                    u.token,
                    offset_words(d)
                ),
                (None, None) => format!("The linked design's ‘{name}’ moves {}", offset_words(d)),
            };
            effects.push(AssemblyEffect {
                feature: name,
                effect: EffectKind::Moves,
                datum: named.map(|d| d.id),
                message,
            });
        }
        Ok(effects)
    }
}

/// The body a sketch link names.
fn link_body(source: &LinkSource) -> Option<&str> {
    match source {
        LinkSource::Face { body, .. }
        | LinkSource::Edge { body, .. }
        | LinkSource::Vertex { body, .. }
        | LinkSource::Body { body } => Some(body.as_str()),
        _ => None,
    }
}

/// A feature that would fail, in the user's words: the datum it uses and
/// why.
fn failure(u: &Use, error: &str, published: &Sidecar, candidate: &Sidecar) -> AssemblyEffect {
    let named = published.datums.iter().find(|d| d.token == u.token);
    let name = named.map_or_else(|| u.token.clone(), |d| d.name.clone());
    let gone = !candidate.datums.iter().any(|d| d.token == u.token);
    let message = if gone {
        format!(
            "The linked design's ‘{}’ uses {name}, which this publish removes, so it would fail",
            u.feature
        )
    } else {
        format!(
            "The linked design's ‘{}’ uses {name} and would fail: {error}",
            u.feature
        )
    };
    AssemblyEffect {
        feature: u.feature.clone(),
        effect: EffectKind::Fails,
        datum: named.map(|d| d.id),
        message,
    }
}
