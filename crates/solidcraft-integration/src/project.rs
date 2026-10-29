//! Electronics → native Solidcraft. Parametric pads, traces and envelopes are
//! ordinary native features and planar joints, editable without Pcbcraft.
use crate::native::{BoardJoint, BoxBody, Expr, Native, mm};
use crate::scene::Geometry;
use crate::{Error, Result};
use pcbcraft_core::{Id, Layer, Part, Project};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Native appearance of the board substrate (FR-4 green).
const SUBSTRATE_COLOR: &str = "#236447";
const BOARD_THICKNESS_MM: f64 = 1.6;
const COPPER_THICKNESS_MM: f64 = 0.035;
const METADATA_KEY: &str = "org.pcbcraft.electronics";

/// Which Pcbcraft object a native body came from. Stored in the native file's
/// metadata so identity survives reopening.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub pick_id: u64,
    pub instance: Option<Id>,
    pub pad: Option<Id>,
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

fn param(native: &mut Native, name: &str, value: f64, unit: &str) -> Result<()> {
    native.parameter(name, format!("{value} {unit}"))
}

fn finish(mut native: Native, sources: Vec<Source>) -> Result<NativeModel> {
    native.set_metadata(METADATA_KEY, json!({"version": 1, "sources": sources}))?;
    if let Some(error) = native.evaluation_error() {
        return Err(Error::Verification(error));
    }
    let mut geometry = vec![];
    for body in &native.session().world_state().bodies {
        let m = body
            .measure()
            .map_err(|e| Error::Verification(e.to_string()))?;
        geometry.push(Geometry {
            item: sources
                .iter()
                .find(|s| s.component == body.component)
                .map(|s| s.pick_id),
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

pub fn build(project: &Project) -> Result<NativeModel> {
    pcbcraft_core::validate(project).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut native = Native::new(&project.name)?;
    let s = &mut native;
    for (name, value) in [
        ("board_width", project.board_size[0]),
        ("board_depth", project.board_size[1]),
        ("board_thickness", BOARD_THICKNESS_MM),
        ("copper_thickness", COPPER_THICKNESS_MM),
    ] {
        param(s, name, value, "mm")?;
    }
    let board = s.component("PCB substrate", true)?.occurrence;
    s.solid_box(BoxBody {
        name: "Substrate",
        corner: [0.into(), 0.into(), 0.into()],
        size: [
            "board_width".into(),
            "board_depth".into(),
            "board_thickness".into(),
        ],
    })?;
    s.color(&["Substrate"], SUBSTRATE_COLOR)?;
    let mut sources = vec![];
    for placement in &project.placements {
        let instance = project
            .instance(placement.instance)
            .ok_or_else(|| Error::Invalid("missing instance".into()))?;
        let part = project
            .part(instance.part)
            .ok_or_else(|| Error::Invalid("missing part snapshot".into()))?;
        let prefix = format!("instance_{}", instance.id);
        for (suffix, value, unit) in [
            ("x", placement.position[0], "mm"),
            ("y", placement.position[1], "mm"),
            ("angle", placement.rotation_deg, "deg"),
        ] {
            param(s, &format!("{prefix}_{suffix}"), value, unit)?;
        }
        let label = format!("{} envelope", instance.reference);
        let created = s.component(&label, false)?;
        let size = part.package.body_size;
        for (axis, value) in ["length", "width", "height"].into_iter().zip(size) {
            param(s, &format!("{prefix}_{axis}"), value, "mm")?;
        }
        let offset = part.package.model_offset;
        s.solid_box(BoxBody {
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
        })?;
        s.board_joint(BoardJoint {
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
    for pad in pcbcraft_core::layout::pads(project) {
        let instance = project
            .instance(pad.instance)
            .ok_or_else(|| Error::Invalid("missing pad instance".into()))?;
        let part = project
            .part(instance.part)
            .ok_or_else(|| Error::Invalid("missing pad definition".into()))?;
        let local = part
            .footprint
            .pads
            .iter()
            .find(|p| p.id == pad.pad)
            .ok_or_else(|| Error::Invalid("missing pad".into()))?;
        let prefix = format!("instance_{}", pad.instance);
        let pp = format!("pad_{}_{}", pad.instance, pad.pad);
        let dx = local.position[0] - part.footprint.datum_offset[0];
        let dy = (local.position[1] - part.footprint.datum_offset[1])
            * if pad.layer == Layer::Top { 1.0 } else { -1.0 };
        param(s, &format!("{pp}_length"), pad.size[0], "mm")?;
        param(s, &format!("{pp}_width"), pad.size[1], "mm")?;
        let created = s.component(&pp, false)?;
        s.solid_box(BoxBody {
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
        })?;
        let x =
            format!("{prefix}_x + cos({prefix}_angle) * {dx} mm - sin({prefix}_angle) * {dy} mm");
        let y =
            format!("{prefix}_y + sin({prefix}_angle) * {dx} mm + cos({prefix}_angle) * {dy} mm");
        s.board_joint(BoardJoint {
            board,
            occurrence: created.occurrence,
            x: x.into(),
            y: y.into(),
            angle: format!("{prefix}_angle").into(),
            layer: pad.layer,
        })?;
        let component = created.component;
        sources.push(Source {
            pick_id: next_pick(&sources),
            instance: Some(pad.instance),
            pad: Some(pad.pad),
            track: None,
            net: Some(pad.net),
            layer: Some(pad.layer),
            label: format!("{} pad {}", instance.reference, pad.pad),
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
            param(s, &format!("{prefix}_{suffix}"), value, "mm")?;
        }
        let created = s.component(&prefix, false)?;
        let length = format!("sqrt(({prefix}_x1-{prefix}_x0)^2 + ({prefix}_y1-{prefix}_y0)^2)");
        let angle = format!("atan2({prefix}_y1-{prefix}_y0; {prefix}_x1-{prefix}_x0)");
        s.solid_box(BoxBody {
            name: &prefix,
            corner: [0.into(), format!("-{prefix}_width / 2").into(), 0.into()],
            size: [
                length.into(),
                format!("{prefix}_width").into(),
                "copper_thickness".into(),
            ],
        })?;
        s.board_joint(BoardJoint {
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
    finish(native, sources)
}

pub fn preview(part: &Part) -> Result<NativeModel> {
    pcbcraft_core::parts::validate_part(part).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut s = Native::new(&format!("{} — package envelope", part.name))?;
    for (name, value) in ["length", "width", "height"]
        .into_iter()
        .zip(part.package.body_size)
    {
        param(&mut s, name, value, "mm")?;
    }
    let offset = part.package.model_offset;
    s.solid_box(BoxBody {
        name: "Package envelope",
        corner: [
            format!("-length/2 + {}", mm(offset[0])).into(),
            format!("-width/2 + {}", mm(offset[1])).into(),
            Expr::from(offset[2]),
        ],
        size: ["length".into(), "width".into(), "height".into()],
    })?;
    finish(s, vec![])
}

#[cfg(test)]
mod tests;
