//! Electronics → native Solidcraft. Parametric pads, traces and envelopes are
//! ordinary native features and planar joints, editable without Pcbcraft.
//!
//! The native model holds only physical bodies: the laminate, exact copper
//! and package envelopes. They are disjoint except where copper joins copper
//! (a track ending in its pad), so Solidcraft's interference checks and mass
//! properties of the saved file are physical. Soldermask, plated finish,
//! terminations and solder fillets are display-only layers
//! ([`crate::appearance`]) added to the drawn geometry, never to the file.
use crate::appearance::{self, Body, Land, Part, Path};
use crate::material::{self, BodyKind};
use crate::native::{BoardJoint, BoxBody, Expr, Native, mm};
use crate::scene::Geometry;
use crate::{Error, Result};
use pcbcraft_core::library::{FootprintDefinition, PackageDefinition};
use pcbcraft_core::{Endpoint, Fabrication, Id, Layer, PadId, Point, Project, instances, layout};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

const BOARD_THICKNESS_MM: f64 = 1.6;
const COPPER_THICKNESS_MM: f64 = 0.035;
const METADATA_KEY: &str = "org.pcbcraft.electronics";
/// Body kinds: `org.pcbcraft.electronics.bodies.0`, `.1`, … each
/// `{kind: [body names]}`, below Solidcraft's 64 KiB value limit.
const BODIES_KEY: &str = "org.pcbcraft.electronics.bodies.";
const BODIES_CHUNK_BYTES: usize = 48 * 1024;
/// Solidcraft allows 256 metadata keys; leave room for others.
const MAX_BODY_CHUNKS: usize = 200;

/// Which Pcbcraft object a native body came from. Stored in the native file's
/// metadata so identity survives reopening.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub pick_id: u64,
    pub instance: Option<Id>,
    pub pad: Option<PadId>,
    pub track: Option<Id>,
    pub net: Option<Id>,
    pub layer: Option<Layer>,
    pub label: String,
    pub component: u64,
}

pub struct NativeModel {
    native: Native,
    pub geometry: Vec<Geometry>,
    pub sources: Vec<Source>,
}

impl NativeModel {
    /// Write the model as a new `.solidcraft` file; refuses existing files.
    pub fn save(&self, path: &str) -> Result<()> {
        self.native.save_new(path)
    }
}

/// A native design under construction, recording each body's kind.
struct Builder {
    native: Native,
    bodies: BTreeMap<String, BodyKind>,
}

impl Builder {
    fn new(name: &str) -> Result<Self> {
        Ok(Self {
            native: Native::new(name)?,
            bodies: BTreeMap::new(),
        })
    }

    fn param(&mut self, name: &str, value: f64, unit: &str) -> Result<()> {
        self.native.parameter(name, format!("{value} {unit}"))
    }

    fn body(&mut self, kind: BodyKind, body: BoxBody<'_>) -> Result<()> {
        let name = body.name.to_owned();
        self.native.solid_box(body)?;
        if self.bodies.insert(name.clone(), kind).is_some() {
            return Err(Error::Verification(format!("duplicate body name {name}")));
        }
        Ok(())
    }

    /// Store each kind's physical look (one assignment per kind) and the
    /// body → kind map, then evaluate.
    fn finish(mut self, sources: Vec<Source>) -> Result<NativeModel> {
        self.native.activate_root()?;
        for kind in BodyKind::ALL {
            let names: Vec<&str> = self
                .bodies
                .iter()
                .filter(|(_, k)| **k == kind)
                .map(|(name, _)| name.as_str())
                .collect();
            // Saved bodies look the same for every fabrication; only the
            // display-only layers take the project's mask and finish.
            if !names.is_empty() {
                self.native
                    .appearance(&names, &material::material(kind, &Fabrication::default()))?;
            }
        }
        self.native
            .set_metadata(METADATA_KEY, json!({"version": 1, "sources": sources}))?;
        let chunks = kind_chunks(&self.bodies)?;
        for (index, chunk) in chunks.iter().enumerate() {
            self.native
                .set_metadata(&format!("{BODIES_KEY}{index}"), json!(chunk))?;
        }
        finish(self.native, sources)
    }
}

/// The body → kind map split over metadata values: Solidcraft caps each
/// value at 64 KiB. Sizes are measured as serialized (escaping included).
fn kind_chunks(bodies: &BTreeMap<String, BodyKind>) -> Result<Vec<BTreeMap<BodyKind, Vec<&str>>>> {
    let mut chunks: Vec<BTreeMap<BodyKind, Vec<&str>>> = vec![BTreeMap::new()];
    let mut size = 2;
    for (name, kind) in bodies {
        // The escaped name and its comma, plus the kind's key, colon and
        // brackets when it is the chunk's first body of that kind.
        let entry = serde_json::to_string(name)?.len() + 1;
        let key = serde_json::to_string(kind)?.len() + 4;
        let new_kind = chunks.last().is_some_and(|c| !c.contains_key(kind));
        let needed = entry + if new_kind { key } else { 0 };
        if needed + 2 > BODIES_CHUNK_BYTES {
            return Err(Error::Invalid(format!("body name too long: {name}")));
        }
        if size + needed > BODIES_CHUNK_BYTES {
            chunks.push(BTreeMap::new());
            size = 2;
        }
        if let Some(chunk) = chunks.last_mut() {
            if !chunk.contains_key(kind) {
                size += key;
            }
            chunk.entry(*kind).or_default().push(name);
            size += entry;
        }
    }
    if chunks.len() > MAX_BODY_CHUNKS {
        return Err(Error::Invalid(
            "the model has too many bodies to record their kinds".into(),
        ));
    }
    Ok(chunks)
}

/// The body → kind map stored with the model.
fn body_kinds(native: &Native) -> Result<BTreeMap<String, BodyKind>> {
    let metadata = serde_json::to_value(&native.session().doc.metadata)?;
    let mut kinds = BTreeMap::new();
    for (key, value) in metadata.as_object().into_iter().flatten() {
        if !key.starts_with(BODIES_KEY) {
            continue;
        }
        let chunk: BTreeMap<BodyKind, Vec<String>> = serde_json::from_value(value.clone())?;
        for (kind, names) in chunk {
            for name in names {
                kinds.insert(name, kind);
            }
        }
    }
    if kinds.is_empty() {
        return Err(Error::Verification(
            "native model has no Pcbcraft body kinds".into(),
        ));
    }
    Ok(kinds)
}

fn finish(native: Native, sources: Vec<Source>) -> Result<NativeModel> {
    if let Some(error) = native.evaluation_error() {
        return Err(Error::Verification(error));
    }
    let kinds = body_kinds(&native)?;
    let doc = &native.session().doc;
    let mut geometry = vec![];
    for body in &native.session().world_state().bodies {
        let name = body.definition_name();
        let kind = *kinds
            .get(name)
            .ok_or_else(|| Error::Verification(format!("body {name} has no Pcbcraft kind")))?;
        let surface = doc
            .model_body_look(body)
            .ok_or_else(|| Error::Verification(format!("body {name} has no look")))?
            .opaque_surface()
            .map_err(|e| Error::Verification(e.to_string()))?
            .ok_or_else(|| Error::Verification(format!("body {name} has no physical look")))?;
        let m = body
            .measure()
            .map_err(|e| Error::Verification(e.to_string()))?;
        geometry.push(Geometry {
            item: sources
                .iter()
                .find(|s| s.component == body.component)
                .map(|s| s.pick_id),
            kind,
            surface,
            // Saved bodies' detail does not depend on the fabrication.
            detail: crate::material::detail(kind, &pcbcraft_core::Fabrication::default()),
            mesh: body.mesh(),
            volume: m.volume,
            min: [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
            max: [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
        });
    }
    Ok(NativeModel {
        native,
        geometry,
        sources,
    })
}

fn next_pick(sources: &[Source]) -> u64 {
    u64::try_from(sources.len()).map_or(u64::MAX, |n| n.saturating_add(1))
}

/// A footprint pad as placed on the board, connected or not.
struct PlacedPad<'a> {
    instance: Id,
    designator: &'a str,
    pad: &'a pcbcraft_core::library::Pad,
    footprint: &'a FootprintDefinition,
    net: Option<Id>,
    center: Point,
    rotation_deg: f64,
    layer: Layer,
}

/// Every footprint pad of every placement. Unlike `layout::pads`, pads
/// without a net are included: they are still copper on the board.
fn placed_pads(project: &Project) -> Result<Vec<PlacedPad<'_>>> {
    let net_by_terminal: BTreeMap<Endpoint, Id> = project
        .nets
        .iter()
        .flat_map(|net| net.terminals.iter().map(move |t| (*t, net.id)))
        .collect();
    let mut result = Vec::new();
    for placement in &project.placements {
        let instance = project
            .instance(placement.instance)
            .ok_or_else(|| Error::Invalid("missing pad instance".into()))?;
        let physical = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
            .ok_or_else(|| Error::Invalid("missing pad definition".into()))?;
        let footprint = &physical.footprint.definition;
        for pad in &footprint.pads {
            if result.len() >= layout::MAX_PLACED_PADS {
                return Err(Error::Invalid(format!(
                    "the board has more than {} pads",
                    layout::MAX_PLACED_PADS
                )));
            }
            let net = physical
                .binding
                .definition
                .terminal_of(pad.terminal)
                .and_then(|terminal| {
                    net_by_terminal.get(&Endpoint {
                        instance: instance.id,
                        terminal,
                    })
                })
                .copied();
            result.push(PlacedPad {
                instance: instance.id,
                designator: &instance.designator,
                pad,
                footprint,
                net,
                center: layout::placed_point(placement, pad.position, footprint.datum_offset),
                rotation_deg: placement.rotation_deg,
                layer: placement.layer,
            });
        }
    }
    Ok(result)
}

pub fn build(project: &Project) -> Result<NativeModel> {
    pcbcraft_core::validate::project(project).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut b = Builder::new(&project.name)?;
    for (name, value) in [
        ("board_width", project.board_size[0]),
        ("board_depth", project.board_size[1]),
        ("board_thickness", BOARD_THICKNESS_MM),
        ("copper_thickness", COPPER_THICKNESS_MM),
    ] {
        b.param(name, value, "mm")?;
    }
    let board = b.native.component("PCB substrate", true)?.occurrence;
    b.body(
        BodyKind::Substrate,
        BoxBody {
            name: "Substrate",
            corner: [0.into(), 0.into(), 0.into()],
            size: [
                "board_width".into(),
                "board_depth".into(),
                "board_thickness".into(),
            ],
        },
    )?;
    let mut sources = vec![];
    let mut parts = vec![];
    for placement in &project.placements {
        let instance = project
            .instance(placement.instance)
            .ok_or_else(|| Error::Invalid("missing instance".into()))?;
        let physical = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
            .ok_or_else(|| {
                Error::Invalid("placed instance has no physical implementation".into())
            })?;
        let package = &physical.package.definition;
        let footprint = &physical.footprint.definition;
        let prefix = format!("instance_{}", instance.id);
        for (suffix, value, unit) in [
            ("x", placement.position[0], "mm"),
            ("y", placement.position[1], "mm"),
            ("angle", placement.rotation_deg, "deg"),
        ] {
            b.param(&format!("{prefix}_{suffix}"), value, unit)?;
        }
        let label = format!("{} envelope", instance.designator);
        let created = b.native.component(&label, false)?;
        for (axis, value) in ["length", "width", "height"]
            .into_iter()
            .zip(package.body_size)
        {
            b.param(&format!("{prefix}_{axis}"), value, "mm")?;
        }
        let offset = package.model_offset;
        let body = Body::of(package);
        b.body(
            body.kind(),
            BoxBody {
                name: &label,
                corner: [
                    format!("- {prefix}_length / 2 + {}", mm(offset[0])).into(),
                    format!("- {prefix}_width / 2 + {}", mm(offset[1])).into(),
                    format!("copper_thickness + {}", mm(offset[2])).into(),
                ],
                size: [
                    format!("{prefix}_length").into(),
                    format!("{prefix}_width").into(),
                    format!("{prefix}_height").into(),
                ],
            },
        )?;
        b.native.board_joint(BoardJoint {
            board,
            occurrence: created.occurrence,
            x: format!("{prefix}_x").into(),
            y: format!("{prefix}_y").into(),
            angle: format!("{prefix}_angle").into(),
            layer: placement.layer,
        })?;
        let pick = next_pick(&sources);
        parts.push(Part {
            pick: Some(pick),
            package,
            footprint: Some(footprint),
            body,
            placement: Some(placement),
        });
        sources.push(Source {
            pick_id: pick,
            instance: Some(instance.id),
            pad: None,
            track: None,
            net: None,
            layer: Some(placement.layer),
            label,
            component: created.component,
        });
    }
    let pads = placed_pads(project)?;
    let mut lands = Vec::with_capacity(pads.len());
    for pad in &pads {
        let prefix = format!("instance_{}", pad.instance);
        let pp = format!("pad_{}_{}", pad.instance, pad.pad.id);
        let local = pad.pad;
        let footprint = pad.footprint;
        let dx = local.position[0] - footprint.datum_offset[0];
        let dy = (local.position[1] - footprint.datum_offset[1])
            * if pad.layer == Layer::Top { 1.0 } else { -1.0 };
        b.param(&format!("{pp}_length"), local.size[0], "mm")?;
        b.param(&format!("{pp}_width"), local.size[1], "mm")?;
        let created = b.native.component(&pp, false)?;
        b.body(
            BodyKind::PadCopper,
            BoxBody {
                name: &pp,
                corner: [
                    format!("-{pp}_length / 2").into(),
                    format!("-{pp}_width / 2").into(),
                    0.into(),
                ],
                size: [
                    format!("{pp}_length").into(),
                    format!("{pp}_width").into(),
                    "copper_thickness".into(),
                ],
            },
        )?;
        let x =
            format!("{prefix}_x + cos({prefix}_angle) * {dx} mm - sin({prefix}_angle) * {dy} mm");
        let y =
            format!("{prefix}_y + sin({prefix}_angle) * {dx} mm + cos({prefix}_angle) * {dy} mm");
        b.native.board_joint(BoardJoint {
            board,
            occurrence: created.occurrence,
            x: x.into(),
            y: y.into(),
            angle: format!("{prefix}_angle").into(),
            layer: pad.layer,
        })?;
        let pick = next_pick(&sources);
        lands.push(Land {
            pick,
            center: pad.center,
            half: [local.size[0] / 2.0, local.size[1] / 2.0],
            angle: pad.rotation_deg.to_radians(),
            layer: pad.layer,
        });
        sources.push(Source {
            pick_id: pick,
            instance: Some(pad.instance),
            pad: Some(local.id),
            track: None,
            net: pad.net,
            layer: Some(pad.layer),
            label: format!("{} pad {}", pad.designator, local.id),
            component: created.component,
        });
    }
    let mut paths = Vec::with_capacity(project.tracks.len());
    for track in &project.tracks {
        let prefix = format!("trace_{}", track.id);
        for (suffix, value) in [
            ("x0", track.start[0]),
            ("y0", track.start[1]),
            ("x1", track.end[0]),
            ("y1", track.end[1]),
            ("width", track.width),
        ] {
            b.param(&format!("{prefix}_{suffix}"), value, "mm")?;
        }
        let created = b.native.component(&prefix, false)?;
        let length = format!("sqrt(({prefix}_x1-{prefix}_x0)^2 + ({prefix}_y1-{prefix}_y0)^2)");
        let angle = format!("atan2({prefix}_y1-{prefix}_y0; {prefix}_x1-{prefix}_x0)");
        b.body(
            BodyKind::CopperTrack,
            BoxBody {
                name: &prefix,
                corner: [0.into(), format!("-{prefix}_width / 2").into(), 0.into()],
                size: [
                    length.into(),
                    format!("{prefix}_width").into(),
                    "copper_thickness".into(),
                ],
            },
        )?;
        b.native.board_joint(BoardJoint {
            board,
            occurrence: created.occurrence,
            x: format!("{prefix}_x0").into(),
            y: format!("{prefix}_y0").into(),
            angle: angle.into(),
            layer: track.layer,
        })?;
        let pick = next_pick(&sources);
        paths.push(Path {
            pick,
            start: track.start,
            end: track.end,
            width: track.width,
            layer: track.layer,
        });
        sources.push(Source {
            pick_id: pick,
            instance: None,
            pad: None,
            track: Some(track.id),
            net: Some(track.net),
            layer: Some(track.layer),
            label: format!("Trace {}", track.id),
            component: created.component,
        });
    }
    let mut model = b.finish(sources)?;
    let layers = appearance::board(
        &appearance::Board {
            width: project.board_size[0],
            depth: project.board_size[1],
            thickness: BOARD_THICKNESS_MM,
            copper: COPPER_THICKNESS_MM,
            fabrication: project.fabrication,
        },
        &lands,
        &paths,
        &parts,
    )?;
    model.geometry.extend(layers);
    Ok(model)
}

/// The native envelope of one package definition, draft or published, with
/// the same terminations and overcoat a placed chip part shows.
pub fn preview(package: &PackageDefinition) -> Result<NativeModel> {
    if !package.envelope_valid() {
        return Err(Error::Invalid(
            "package envelope dimensions must be finite and positive".into(),
        ));
    }
    let mut b = Builder::new("Package envelope")?;
    for (name, value) in ["length", "width", "height"]
        .into_iter()
        .zip(package.body_size)
    {
        b.param(name, value, "mm")?;
    }
    let offset = package.model_offset;
    let body = Body::of(package);
    let kind = match body {
        Body::Moulded => BodyKind::PartBody,
        chip => chip.kind(),
    };
    b.body(
        kind,
        BoxBody {
            name: "Package envelope",
            corner: [
                format!("-length/2 + {}", mm(offset[0])).into(),
                format!("-width/2 + {}", mm(offset[1])).into(),
                Expr::from(offset[2]),
            ],
            size: ["length".into(), "width".into(), "height".into()],
        },
    )?;
    let mut model = b.finish(vec![])?;
    model.geometry.extend(appearance::preview(&Part {
        pick: None,
        package,
        footprint: None,
        body,
        placement: None,
    })?);
    Ok(model)
}

#[cfg(test)]
mod tests;
