//! Electronics → native Solidcraft. Parametric pads, traces and envelopes are
//! ordinary native features and planar joints, editable without Pcbcraft.
//!
//! Besides exact copper, the model carries appearance layers: soldermask
//! slabs over the laminate, a conforming mask cap over each track, a plated
//! finish shell around each pad and tinned terminations on small packages.
//! They overlap the copper volume on purpose (see `docs/materials.md`); the
//! copper bodies themselves keep their exact volumes and identities.
use crate::material::{self, BodyKind, DEFAULT_FINISH};
use crate::native::{BoardJoint, BoxBody, Expr, Native, mm};
use crate::scene::Geometry;
use crate::{Error, Result};
use pcbcraft_core::library::{FootprintDefinition, PackageDefinition};
use pcbcraft_core::{Endpoint, Id, Layer, PadId, Point, Project, instances, layout};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

const BOARD_THICKNESS_MM: f64 = 1.6;
const COPPER_THICKNESS_MM: f64 = 0.035;
/// Soldermask over bare laminate. Thinner than real LPI (25–35 µm) so exposed
/// pads stand clearly proud of it; the optical thickness is in `material`.
const MASK_THICKNESS_MM: f64 = 0.020;
/// Soldermask above a track's copper.
const MASK_CAP_MM: f64 = 0.015;
/// ENIG nickel (3–6 µm, IPC-4552); the immersion gold is negligible.
const FINISH_THICKNESS_MM: f64 = 0.005;
/// How far a track's mask cap overhangs its copper on each side and at a free
/// end, so no visible face is coplanar with copper.
const CAP_MARGIN_MM: f64 = 0.01;
/// Shorter caps are skipped (a track lying wholly inside its pads).
const CAP_MIN_MM: f64 = 0.01;
/// Termination plating beyond the body faces (real terminations are 10–30 µm).
const TERMINATION_GROWTH_MM: f64 = 0.01;
/// Two-pad packages shorter than this read as chip parts (est. 1206 class).
const CHIP_MAX_LENGTH_MM: f64 = 3.2;
/// Height share of a non-chip termination band.
const LEAD_HEIGHT_SHARE: f64 = 0.15;
/// The core's bound on placed pads (`layout::MAX_PLACED_PADS`).
const MAX_PADS: usize = 4_096;
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
            if !names.is_empty() {
                self.native
                    .appearance(&names, &material::material(kind, DEFAULT_FINISH))?;
            }
        }
        self.native
            .set_metadata(METADATA_KEY, json!({"version": 1, "sources": sources}))?;
        // Solidcraft caps each metadata value at 64 KiB, so the body → kind
        // map is split over keys of its own, grouped by kind.
        let mut chunks: Vec<BTreeMap<BodyKind, Vec<&str>>> = vec![BTreeMap::new()];
        let mut size = 0;
        for (name, kind) in &self.bodies {
            let entry = name.len() + 4;
            if size + entry > BODIES_CHUNK_BYTES {
                chunks.push(BTreeMap::new());
                size = 0;
            }
            if let Some(chunk) = chunks.last_mut() {
                chunk.entry(*kind).or_default().push(name);
                size += entry;
            }
        }
        if chunks.len() > MAX_BODY_CHUNKS {
            return Err(Error::Invalid(
                "the model has too many bodies to record their kinds".into(),
            ));
        }
        for (index, chunk) in chunks.iter().enumerate() {
            self.native
                .set_metadata(&format!("{BODIES_KEY}{index}"), json!(chunk))?;
        }
        finish(self.native, sources)
    }
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
            if result.len() >= MAX_PADS {
                return Err(Error::Invalid(format!(
                    "the board has more than {MAX_PADS} pads"
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

/// A pad's finish rectangle in board coordinates.
struct Land {
    center: Point,
    half: Point,
    angle: f64,
    layer: Layer,
}

impl Land {
    fn local(&self, v: Point) -> Point {
        let (s, c) = self.angle.sin_cos();
        [c * v[0] + s * v[1], -s * v[0] + c * v[1]]
    }

    fn contains(&self, point: Point) -> bool {
        let q = self.local([point[0] - self.center[0], point[1] - self.center[1]]);
        (0..2).all(|i| q[i].abs() <= self.half[i] + 1e-9)
    }

    /// Where the line `point + t·direction` (unit direction) leaves this
    /// rectangle, if it crosses it ahead of `point`.
    fn exit(&self, point: Point, direction: Point) -> Option<f64> {
        let q = self.local([point[0] - self.center[0], point[1] - self.center[1]]);
        let v = self.local(direction);
        let (mut enter, mut leave) = (f64::NEG_INFINITY, f64::INFINITY);
        for i in 0..2 {
            if v[i].abs() < 1e-12 {
                if q[i].abs() > self.half[i] {
                    return None;
                }
                continue;
            }
            let a = (-self.half[i] - q[i]) / v[i];
            let b = (self.half[i] - q[i]) / v[i];
            enter = enter.max(a.min(b));
            leave = leave.min(a.max(b));
        }
        (enter <= leave && leave > 0.0).then_some(leave)
    }
}

/// How far a track's cap is trimmed back from `end` (towards unit
/// `direction`) when the end lies in a same-layer pad: far enough that the
/// whole cap width (`half_width`) has left that pad's finish.
fn trim(
    lands: &[Land],
    layer: Layer,
    end: Point,
    direction: Point,
    half_width: f64,
) -> Option<f64> {
    let normal = [-direction[1], direction[0]];
    lands
        .iter()
        .filter(|land| land.layer == layer && land.contains(end))
        .flat_map(|land| {
            [0.0, half_width, -half_width]
                .into_iter()
                .filter_map(move |o| {
                    land.exit([end[0] + normal[0] * o, end[1] + normal[1] * o], direction)
                })
        })
        .fold(None, |best: Option<f64>, t| {
            Some(best.map_or(t, |b| b.max(t)))
        })
}

/// The termination box of a pad under a package body, in the envelope's own
/// (unmirrored) footprint frame: the pad ∩ body rectangle, grown outward where
/// it reaches the body's faces. `None` when they do not overlap.
fn termination(
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
    pad: &pcbcraft_core::library::Pad,
) -> Option<([f64; 2], [f64; 2])> {
    let [length, width, _] = package.body_size;
    let offset = package.model_offset;
    let body_min = [offset[0] - length / 2.0, offset[1] - width / 2.0];
    let body_max = [offset[0] + length / 2.0, offset[1] + width / 2.0];
    let mut min = [0.0; 2];
    let mut max = [0.0; 2];
    for axis in 0..2 {
        let center = pad.position[axis] - footprint.datum_offset[axis];
        let lo = (center - pad.size[axis] / 2.0).max(body_min[axis]);
        let hi = (center + pad.size[axis] / 2.0).min(body_max[axis]);
        if (hi - lo).partial_cmp(&1e-6) != Some(std::cmp::Ordering::Greater) {
            return None;
        }
        let on = |a: f64, b: f64| (a - b).abs() < 1e-9;
        min[axis] = lo
            - if on(lo, body_min[axis]) {
                TERMINATION_GROWTH_MM
            } else {
                0.0
            };
        max[axis] = hi
            + if on(hi, body_max[axis]) {
                TERMINATION_GROWTH_MM
            } else {
                0.0
            };
    }
    Some((min, max))
}

pub fn build(project: &Project) -> Result<NativeModel> {
    pcbcraft_core::validate::project(project).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut b = Builder::new(&project.name)?;
    for (name, value) in [
        ("board_width", project.board_size[0]),
        ("board_depth", project.board_size[1]),
        ("board_thickness", BOARD_THICKNESS_MM),
        ("copper_thickness", COPPER_THICKNESS_MM),
        ("mask_thickness", MASK_THICKNESS_MM),
        ("mask_cap", MASK_CAP_MM),
        ("finish_thickness", FINISH_THICKNESS_MM),
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
    for (kind, name, z) in [
        (BodyKind::MaskTop, "Soldermask top", "board_thickness"),
        (
            BodyKind::MaskBottom,
            "Soldermask bottom",
            "0 mm - mask_thickness",
        ),
    ] {
        b.body(
            kind,
            BoxBody {
                name,
                corner: [0.into(), 0.into(), z.into()],
                size: [
                    "board_width".into(),
                    "board_depth".into(),
                    "mask_thickness".into(),
                ],
            },
        )?;
    }
    let mut sources = vec![];
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
        let size = package.body_size;
        for (axis, value) in ["length", "width", "height"].into_iter().zip(size) {
            b.param(&format!("{prefix}_{axis}"), value, "mm")?;
        }
        let offset = package.model_offset;
        b.body(
            BodyKind::PackageBody,
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
        // Chip parts are plated over their ends; other small packages show
        // a low tinned band where a lead meets the land.
        if footprint.pads.len() <= 2 {
            let chip = footprint.pads.len() == 2 && size[0].max(size[1]) < CHIP_MAX_LENGTH_MM;
            let top = if chip {
                offset[2] + size[2] + TERMINATION_GROWTH_MM
            } else {
                offset[2] + LEAD_HEIGHT_SHARE * size[2]
            };
            for pad in &footprint.pads {
                let Some((min, max)) = termination(package, footprint, pad) else {
                    continue;
                };
                if top.partial_cmp(&0.001) != Some(std::cmp::Ordering::Greater) {
                    continue;
                }
                b.body(
                    BodyKind::Termination,
                    BoxBody {
                        name: &format!("{label} termination {}", pad.id),
                        corner: [
                            mm(min[0]).into(),
                            mm(min[1]).into(),
                            "copper_thickness".into(),
                        ],
                        size: [
                            mm(max[0] - min[0]).into(),
                            mm(max[1] - min[1]).into(),
                            mm(top).into(),
                        ],
                    },
                )?;
            }
        }
        b.native.board_joint(BoardJoint {
            board,
            occurrence: created.occurrence,
            x: format!("{prefix}_x").into(),
            y: format!("{prefix}_y").into(),
            angle: format!("{prefix}_angle").into(),
            layer: placement.layer,
        })?;
        let component = created.component;
        sources.push(Source {
            pick_id: next_pick(&sources),
            instance: Some(instance.id),
            pad: None,
            track: None,
            net: None,
            layer: Some(placement.layer),
            label,
            component,
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
        // The plated finish encloses the copper (sidewalls included), so the
        // pad's visible surface is never coplanar with a track's copper.
        b.body(
            BodyKind::PadFinish,
            BoxBody {
                name: &format!("{pp} finish"),
                corner: [
                    format!("-{pp}_length / 2 - finish_thickness").into(),
                    format!("-{pp}_width / 2 - finish_thickness").into(),
                    0.into(),
                ],
                size: [
                    format!("{pp}_length + 2 * finish_thickness").into(),
                    format!("{pp}_width + 2 * finish_thickness").into(),
                    "copper_thickness + finish_thickness".into(),
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
        lands.push(Land {
            center: pad.center,
            half: [
                local.size[0] / 2.0 + FINISH_THICKNESS_MM,
                local.size[1] / 2.0 + FINISH_THICKNESS_MM,
            ],
            angle: pad.rotation_deg.to_radians(),
            layer: pad.layer,
        });
        let component = created.component;
        sources.push(Source {
            pick_id: next_pick(&sources),
            instance: Some(pad.instance),
            pad: Some(local.id),
            track: None,
            net: pad.net,
            layer: Some(pad.layer),
            label: format!("{} pad {}", pad.designator, local.id),
            component,
        });
    }
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
                    length.clone().into(),
                    format!("{prefix}_width").into(),
                    "copper_thickness".into(),
                ],
            },
        )?;
        // The mask cap ends where the track enters a pad's finish and
        // overhangs a free end. The trims are numeric: moving a pad in the
        // native file outside Pcbcraft does not move them.
        let delta = [track.end[0] - track.start[0], track.end[1] - track.start[1]];
        let numeric_length = delta[0].hypot(delta[1]);
        if numeric_length > 0.0 {
            let u = [delta[0] / numeric_length, delta[1] / numeric_length];
            let half = track.width / 2.0 + CAP_MARGIN_MM;
            let start = trim(&lands, track.layer, track.start, u, half).unwrap_or(-CAP_MARGIN_MM);
            let end = trim(&lands, track.layer, track.end, [-u[0], -u[1]], half)
                .unwrap_or(-CAP_MARGIN_MM);
            if numeric_length - start - end >= CAP_MIN_MM {
                b.body(
                    BodyKind::MaskCap,
                    BoxBody {
                        name: &format!("{prefix} mask"),
                        corner: [
                            mm(start).into(),
                            format!("-{prefix}_width / 2 - {}", mm(CAP_MARGIN_MM)).into(),
                            "copper_thickness - mask_thickness".into(),
                        ],
                        size: [
                            format!("{length} - ({})", mm(start + end)).into(),
                            format!("{prefix}_width + {}", mm(2.0 * CAP_MARGIN_MM)).into(),
                            "mask_thickness + mask_cap".into(),
                        ],
                    },
                )?;
            }
        }
        b.native.board_joint(BoardJoint {
            board,
            occurrence: created.occurrence,
            x: format!("{prefix}_x0").into(),
            y: format!("{prefix}_y0").into(),
            angle: angle.into(),
            layer: track.layer,
        })?;
        let component = created.component;
        sources.push(Source {
            pick_id: next_pick(&sources),
            instance: None,
            pad: None,
            track: Some(track.id),
            net: Some(track.net),
            layer: Some(track.layer),
            label: format!("Trace {}", track.id),
            component,
        });
    }
    b.finish(sources)
}

/// The native envelope of one package definition, draft or published.
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
    b.body(
        BodyKind::PartBody,
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
    b.finish(vec![])
}

#[cfg(test)]
mod tests;
