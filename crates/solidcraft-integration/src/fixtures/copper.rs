//! Native copper acceptance fixture. Geometry belongs to Solidcraft; electrical
//! identities belong to this adapter and survive in native informational metadata.
mod build;
#[cfg(test)]
mod tests;

use super::call;
use crate::scene::Geometry;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_engine::{Session, doc, geom::Vec3};
use std::sync::Arc;

const META: &str = "org.pcbcraft.copper-fixture";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: u64,
    pub label: String,
    pub net: String,
    pub layer: String,
    pub kind: String,
    pub component: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    items: Vec<Item>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Layout,
    Solid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum Command {
    #[serde(rename = "board.inspect")]
    Inspect {},
    #[serde(rename = "board.select")]
    Select { item: Option<u64> },
    #[serde(rename = "board.highlight_net")]
    HighlightNet { net: Option<String> },
    #[serde(rename = "board.view")]
    View { mode: ViewMode },
    #[serde(rename = "board.width")]
    Width {
        expected_revision: u64,
        width_mm: f64,
    },
    #[serde(rename = "board.undo")]
    Undo {},
    #[serde(rename = "board.redo")]
    Redo {},
    #[serde(rename = "board.save")]
    Save { path: String },
    #[serde(rename = "board.open")]
    Open { path: String },
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub revision: u64,
    pub scene_builds: u64,
    pub items: Vec<Item>,
    pub selected: Option<u64>,
    pub highlighted_net: Option<String>,
    pub highlighted_items: Vec<u64>,
    pub mode: ViewMode,
    pub trace_width_mm: f64,
    pub copper_thickness_mm: f64,
    pub can_undo: bool,
    pub can_redo: bool,
}

pub struct Board {
    session: Session,
    items: Vec<Item>,
    geometry: Vec<Geometry>,
    revision: u64,
    scene_builds: u64,
    selected: Option<u64>,
    net: Option<String>,
    mode: ViewMode,
    undo: Vec<Arc<doc::Document>>,
    redo: Vec<Arc<doc::Document>>,
}

impl Board {
    pub fn fixture() -> Result<Self> {
        Self::from_session(build::fixture()?)
    }

    fn from_session(session: Session) -> Result<Self> {
        let (items, geometry) = evaluate(&session)?;
        Ok(Self {
            session,
            items,
            geometry,
            revision: 1,
            scene_builds: 1,
            selected: None,
            net: None,
            mode: ViewMode::Layout,
            undo: vec![],
            redo: vec![],
        })
    }

    pub fn highlighted(&self, id: u64) -> bool {
        self.selected == Some(id)
            || self
                .items
                .iter()
                .any(|i| i.id == id && self.net.as_deref() == Some(&i.net))
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            revision: self.revision,
            scene_builds: self.scene_builds,
            items: self.items.clone(),
            selected: self.selected,
            highlighted_net: self.net.clone(),
            highlighted_items: self
                .items
                .iter()
                .filter(|i| self.highlighted(i.id))
                .map(|i| i.id)
                .collect(),
            mode: self.mode,
            trace_width_mm: length(&self.session, "trace_width")?,
            copper_thickness_mm: length(&self.session, "copper_thickness")?,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
        })
    }
    /// Normal picking is depth-tested against every visible body, including substrate.
    /// Hiding substrate is an explicit inspection mode, never implied by net selection.
    pub fn pick(&self, origin: Vec3, direction: Vec3, hide_substrate: bool) -> Option<u64> {
        self.geometry
            .iter()
            .filter(|g| !(hide_substrate && g.item.is_none()))
            .filter_map(|g| {
                g.mesh
                    .raycast(origin, direction)
                    .map(|(distance, _)| (distance, g.item))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .and_then(|(_, id)| id)
    }
    pub fn execute(&mut self, command: Command) -> Result<Snapshot> {
        match command {
            Command::Inspect {} => {}
            Command::Select { item } => {
                if item.is_some_and(|id| !self.items.iter().any(|i| i.id == id)) {
                    return Err(Error::Invalid("unknown copper object".into()));
                }
                self.selected = item;
            }
            Command::HighlightNet { net } => {
                if net
                    .as_ref()
                    .is_some_and(|n| !self.items.iter().any(|i| &i.net == n))
                {
                    return Err(Error::Invalid("unknown net".into()));
                }
                self.net = net;
            }
            Command::View { mode } => self.mode = mode,
            Command::Width {
                expected_revision,
                width_mm,
            } => {
                if expected_revision != self.revision {
                    return Err(Error::Invalid(format!(
                        "stale revision: expected {expected_revision}, current {}",
                        self.revision
                    )));
                }
                if !width_mm.is_finite() || !(0.2..=1.5).contains(&width_mm) {
                    return Err(Error::Invalid(
                        "fixture trace width must be 0.2–1.5 mm".into(),
                    ));
                }
                let mut candidate = Session::new((*self.session.doc).clone());
                call(
                    &mut candidate,
                    "parameters.change",
                    json!({"changes":{"trace_width":format!("{width_mm} mm")}}),
                )?;
                self.replace(candidate, true)?;
            }
            Command::Undo {} | Command::Redo {} => {
                let undo = matches!(command, Command::Undo {});
                let history = if undo { &self.undo } else { &self.redo };
                let document = history
                    .last()
                    .ok_or_else(|| Error::Invalid("history is empty".into()))?
                    .clone();
                let prior = self.session.doc.clone();
                self.replace(Session::new((*document).clone()), false)?;
                if undo {
                    self.undo.pop();
                    self.redo.push(prior);
                } else {
                    self.redo.pop();
                    self.undo.push(prior);
                }
            }
            Command::Save { path } => {
                crate::native::validate_path(&path)?;
                if std::path::Path::new(&path).try_exists()? {
                    return Err(Error::Invalid("output exists; choose a new path".into()));
                }
                let mut copy = Session::new((*self.session.doc).clone());
                call(
                    &mut copy,
                    "file.save_as",
                    json!({"path":path,"format":"container","cache":false,"preview":false}),
                )?;
            }
            Command::Open { path } => {
                crate::native::validate_path(&path)?;
                let mut candidate = Session::default();
                call(
                    &mut candidate,
                    "doc.open",
                    json!({"path":path,"read_only":true}),
                )?;
                self.replace(candidate, false)?;
                self.undo.clear();
                self.redo.clear();
                self.selected = None;
                self.net = None;
            }
        }
        self.snapshot()
    }
    fn replace(&mut self, candidate: Session, remember: bool) -> Result<()> {
        let (items, geometry) = evaluate(&candidate)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("revision exhausted".into()))?;
        if remember {
            if self.undo.len() == 100 {
                self.undo.remove(0);
            }
            self.undo.push(self.session.doc.clone());
            self.redo.clear();
        }
        self.session = candidate;
        self.items = items;
        self.geometry = geometry;
        self.revision = revision;
        self.scene_builds += 1;
        Ok(())
    }
}

fn length(session: &Session, name: &str) -> Result<f64> {
    session
        .doc
        .eval(name, doc::expr::Kind::Length)
        .map_err(|e| Error::Verification(e.to_string()))
}

fn evaluate(session: &Session) -> Result<(Vec<Item>, Vec<Geometry>)> {
    let metadata = serde_json::to_value(&session.doc.metadata)?;
    let manifest: Manifest = serde_json::from_value(
        metadata
            .get(META)
            .cloned()
            .ok_or_else(|| Error::Invalid("not a Pcbcraft copper fixture".into()))?,
    )?;
    if manifest.version != 1 || manifest.items.len() != 7 {
        return Err(Error::Invalid("unsupported copper fixture manifest".into()));
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut components = std::collections::BTreeSet::new();
    for item in &manifest.items {
        if !ids.insert(item.id)
            || !components.insert(item.component)
            || !session
                .doc
                .components
                .iter()
                .any(|c| c.id == item.component)
        {
            return Err(Error::Verification(
                "invalid copper identity mapping".into(),
            ));
        }
    }
    for name in ["trace_width", "copper_thickness", "board_thickness"] {
        let value = length(session, name)?;
        if !value.is_finite() || value <= 0.0 || value > 10.0 {
            return Err(Error::Verification(format!("invalid {name}")));
        }
    }
    if let Some(result) = session.model.results.iter().find(|r| r.error.is_some()) {
        return Err(Error::Verification(format!(
            "{}: {:?}",
            result.name, result.error
        )));
    }
    let world = session.world_state();
    let mut geometry = vec![];
    for body in &world.bodies {
        let m = body
            .measure()
            .map_err(|e| Error::Verification(e.to_string()))?;
        if !m.volume.is_finite() || m.volume <= 0.0 {
            return Err(Error::Verification("copper must be a closed volume".into()));
        }
        let source = manifest
            .items
            .iter()
            .find(|i| i.component == body.component);
        let kind = match source.map(|i| i.kind.as_str()) {
            Some("Pad") => crate::material::BodyKind::PadCopper,
            // Trace, via and zone copper.
            Some(_) => crate::material::BodyKind::CopperTrack,
            None => crate::material::BodyKind::Substrate,
        };
        // The fixture's native file stores colours only, not physical looks,
        // so its surfaces come from the kind's look rather than the file.
        geometry.push(Geometry {
            item: source.map(|i| i.id),
            kind,
            layer: None,
            surface: crate::material::material(kind, &pcbcraft_core::Fabrication::default())
                .surface()?,
            detail: crate::material::detail(kind, &pcbcraft_core::Fabrication::default()),
            mesh: body.mesh(),
            volume: m.volume,
            min: [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
            max: [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
        });
    }
    for item in &manifest.items {
        if !geometry.iter().any(|g| g.item == Some(item.id)) {
            return Err(Error::Verification("missing copper geometry".into()));
        }
    }
    Ok((manifest.items, geometry))
}
